//! Map collision from `.RS.col` (a solid-leaf BSP, layout in `docs/formats.md`), exposed as
//! a triangle soup with a BVH. Everything public is in Bevy space: metres, Y up.
//!
//! The BSP's leaf cells are convex and flagged air/solid; each lists the faces on its boundary.
//! Loading keeps exactly the faces that separate air from solid, oriented towards the air, so
//! sweeps work against real surfaces with exact edge/vertex contacts.

use crate::{
    mrs::Vfs,
    view::{SCALE, to_bevy},
};
use bevy::prelude::*;
use std::io::{self, ErrorKind};

const COL_ID: u32 = 0x5050_178f;
const NONE: u32 = u32::MAX;
const MAX_DEPTH: usize = 4096;
/// Distance (cm) a face is probed to either side to tell which side is air.
const PROBE_CM: f32 = 0.5;
/// Triangles smaller than this (m²) are slivers and dropped.
const MIN_AREA: f32 = 1e-8;
const LEAF_TRIS: usize = 4;

/// Gap kept between a moving capsule and surfaces.
const SKIN: f32 = 0.002;
/// Highest ledge a grounded mover steps onto (also the downhill snap distance).
pub const STEP: f32 = 0.55;
/// Shortest horizontal distance a step-up probes ahead.
const STEP_REACH: f32 = 0.1;
/// Surfaces with `normal.y` at least this are floors (about 45°).
pub const WALKABLE: f32 = 0.7;
/// A sphere resting on the edge of a floor triangle still stands while the contact normal
/// tilts no further than this (`normal.y`), i.e. its centre is within ~0.95 radius of the edge.
const EDGE_STAND: f32 = 0.3;
const GROUND_PROBE: f32 = 0.05;
const MAX_SPHERES: usize = 8;

/// A surface contact. `normal` is unit and faces the origin of the ray or the moving sphere.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub distance: f32,
    pub point: Vec3,
    pub normal: Vec3,
    /// Normal of the triangle touched; differs from `normal` when a sphere met an edge or
    /// corner (the contact normal then points from that edge to the sphere centre).
    pub surface: Vec3,
}

impl Hit {
    /// Can something stand here: a floor triangle, and not just grazed by a sphere's side.
    fn floor(&self) -> bool {
        self.surface.y >= WALKABLE && self.normal.y >= EDGE_STAND
    }
}

/// Result of [`MapCollision::slide_move`].
#[derive(Clone, Copy, Debug)]
pub struct Move {
    /// New feet position.
    pub pos: Vec3,
    /// Standing on a walkable surface (`ground_normal.y >= WALKABLE`).
    pub grounded: bool,
    pub ground_normal: Option<Vec3>,
    /// Normal of a steep non-ceiling surface the horizontal move ran into.
    pub wall: Option<Vec3>,
    /// The upward move was stopped by a ceiling.
    pub ceiling: bool,
}

struct Tri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    /// Unit normal facing the air.
    n: Vec3,
}

impl Tri {
    fn centroid(&self) -> Vec3 {
        (self.a + self.b + self.c) / 3.0
    }

    /// Whether `p`, already on the triangle's plane, lies inside (winding independent).
    fn contains(&self, p: Vec3) -> bool {
        let (v0, v1, v2) = (self.b - self.a, self.c - self.a, p - self.a);
        let (d00, d01, d11) = (v0.dot(v0), v0.dot(v1), v1.dot(v1));
        let (d20, d21) = (v2.dot(v0), v2.dot(v1));
        let den = d00 * d11 - d01 * d01;
        if den.abs() < 1e-18 {
            return false;
        }
        let u = (d11 * d20 - d01 * d21) / den;
        let v = (d00 * d21 - d01 * d20) / den;
        const EPS: f32 = 1e-4;
        u >= -EPS && v >= -EPS && u + v <= 1.0 + EPS
    }

    /// Möller–Trumbore, two-sided; distance along the unit `dir`.
    fn ray(&self, origin: Vec3, dir: Vec3) -> Option<f32> {
        let (e1, e2) = (self.b - self.a, self.c - self.a);
        let p = dir.cross(e2);
        let det = e1.dot(p);
        if det.abs() < 1e-12 {
            return None;
        }
        let s = origin - self.a;
        let u = s.dot(p) / det;
        let q = s.cross(e1);
        let v = dir.dot(q) / det;
        let t = e2.dot(q) / det;
        (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t >= 0.0).then_some(t)
    }

    /// First contact of a sphere (`radius`, centre `from`) moving by `d`: (fraction in 0..=1,
    /// contact point, normal facing the sphere). Only front faces block; a sphere already
    /// overlapping is blocked only while it moves closer.
    fn sweep(&self, from: Vec3, d: Vec3, r: f32) -> Option<(f32, Vec3, Vec3)> {
        let s0 = self.n.dot(from - self.a);
        let sv = self.n.dot(d);
        // behind the plane, or never within `r` of it
        if s0 < 0.0 || (s0 >= r && s0 + sv >= r) {
            return None;
        }
        if sv < 0.0 {
            let t = ((r - s0) / sv).max(0.0);
            if t <= 1.0 {
                let q = from + d * t;
                let p = q - self.n * self.n.dot(q - self.a);
                if self.contains(p) {
                    return Some((t, p, self.n));
                }
            }
        }
        let v = [self.a, self.b, self.c];
        let mut best: Option<(f32, Vec3, Vec3)> = None;
        for i in 0..3 {
            for h in [
                sweep_point(from, d, r, v[i]),
                sweep_edge(from, d, r, v[i], v[(i + 1) % 3]),
            ]
            .into_iter()
            .flatten()
            {
                if best.is_none_or(|b| h.0 < b.0) {
                    best = Some(h);
                }
            }
        }
        best
    }
}

fn sweep_point(from: Vec3, d: Vec3, r: f32, p: Vec3) -> Option<(f32, Vec3, Vec3)> {
    let m = from - p;
    let (a, b, c) = (d.dot(d), m.dot(d), m.dot(m) - r * r);
    if c <= 0.0 {
        return (b < 0.0).then(|| (0.0, p, m.normalize_or(Vec3::Y)));
    }
    if b >= 0.0 || a < 1e-12 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / a;
    (t <= 1.0).then(|| (t, p, (m + d * t).normalize_or(Vec3::Y)))
}

fn sweep_edge(from: Vec3, d: Vec3, r: f32, p: Vec3, q: Vec3) -> Option<(f32, Vec3, Vec3)> {
    let e = q - p;
    let ee = e.dot(e);
    if ee < 1e-12 {
        return None;
    }
    let m = from - p;
    let (me, de) = (m.dot(e), d.dot(e));
    // distance to the infinite line, as a quadratic in t
    let a = d.dot(d) - de * de / ee;
    let b = m.dot(d) - me * de / ee;
    let c = m.dot(m) - me * me / ee - r * r;
    if c <= 0.0 {
        let f = me / ee;
        return ((0.0..=1.0).contains(&f) && b < 0.0).then(|| {
            let pt = p + e * f;
            (0.0, pt, (from - pt).normalize_or(Vec3::Y))
        });
    }
    if a < 1e-12 || b >= 0.0 {
        return None;
    }
    let disc = b * b - a * c;
    if disc < 0.0 {
        return None;
    }
    let t = (-b - disc.sqrt()) / a;
    let f = (me + de * t) / ee;
    if t > 1.0 || !(0.0..=1.0).contains(&f) {
        return None;
    }
    let pt = p + e * f;
    Some((t, pt, (from + d * t - pt).normalize_or(Vec3::Y)))
}

struct BvhNode {
    min: Vec3,
    max: Vec3,
    /// Leaf (`count > 0`): first triangle. Inner: index of the right child (left is `self + 1`).
    first: u32,
    count: u32,
}

/// Collision world of one map.
#[derive(Resource)]
pub struct MapCollision {
    tris: Vec<Tri>,
    nodes: Vec<BvhNode>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.into())
}

/// BSP node in GunZ space (cm, Z up).
struct TreeNode {
    plane: [f32; 4],
    pos: u32,
    neg: u32,
    solid: bool,
}

struct Cursor<'a> {
    d: &'a [u8],
    o: usize,
}

impl Cursor<'_> {
    fn take<const N: usize>(&mut self) -> io::Result<[u8; N]> {
        let b = self
            .d
            .get(self.o..self.o + N)
            .ok_or_else(|| bad("col: truncated"))?;
        self.o += N;
        Ok(b.try_into().unwrap())
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.take::<1>()?[0])
    }
    fn u32(&mut self) -> io::Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.take()?))
    }
    fn flag(&mut self, what: &str) -> io::Result<bool> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            v => Err(bad(format!("col: {what} flag {v}"))),
        }
    }
}

/// A leaf face as stored: three vertices and the normal, GunZ space.
type Face = ([Vec3; 3], Vec3);

fn read_node(
    c: &mut Cursor,
    nodes: &mut Vec<TreeNode>,
    faces: &mut Vec<(bool, Face)>,
    depth: usize,
) -> io::Result<u32> {
    if depth > MAX_DEPTH {
        return Err(bad("col: tree too deep"));
    }
    let plane = [c.f32()?, c.f32()?, c.f32()?, c.f32()?];
    let solid = c.flag("solid")?;
    let idx = nodes.len() as u32;
    nodes.push(TreeNode {
        plane,
        pos: NONE,
        neg: NONE,
        solid,
    });
    if c.flag("positive")? {
        nodes[idx as usize].pos = read_node(c, nodes, faces, depth + 1)?;
    }
    if c.flag("negative")? {
        nodes[idx as usize].neg = read_node(c, nodes, faces, depth + 1)?;
    }
    let n = c.u32()? as usize;
    if n > (c.d.len() - c.o) / 48 {
        return Err(bad("col: polygon count exceeds file"));
    }
    for _ in 0..n {
        let mut f = [0f32; 12];
        for v in &mut f {
            *v = c.f32()?;
        }
        let v = |i: usize| Vec3::new(f[i], f[i + 1], f[i + 2]);
        faces.push((solid, ([v(0), v(3), v(6)], v(9))));
    }
    Ok(idx)
}

/// Is the point (GunZ cm) inside solid? Space outside the tree counts as solid.
fn solid_at(nodes: &[TreeNode], p: Vec3) -> bool {
    let mut i = 0;
    loop {
        let n = &nodes[i];
        if n.pos == NONE && n.neg == NONE {
            return n.solid;
        }
        let side = n.plane[0] * p.x + n.plane[1] * p.y + n.plane[2] * p.z + n.plane[3] >= 0.0;
        match if side { n.pos } else { n.neg } {
            NONE => return true,
            next => i = next as usize,
        }
    }
}

impl MapCollision {
    /// Loads `<rs_path>.col`, e.g. `maps/mansion/mansion.rs` -> `maps/mansion/mansion.rs.col`.
    pub fn load(vfs: &Vfs, rs_path: &str) -> io::Result<Self> {
        let data = vfs.read(&format!("{rs_path}.col"))?;
        let mut c = Cursor { d: &data, o: 0 };
        if c.u32()? != COL_ID {
            return Err(bad("col: bad id"));
        }
        if c.u32()? != 0 {
            return Err(bad("col: unsupported version"));
        }
        let (node_count, face_count) = (c.u32()? as usize, c.u32()? as usize);
        let (mut nodes, mut faces) = (
            Vec::with_capacity(node_count),
            Vec::with_capacity(face_count),
        );
        read_node(&mut c, &mut nodes, &mut faces, 0)?;
        if c.o != data.len() || nodes.len() != node_count || faces.len() != face_count {
            return Err(bad("col: size or counts do not match the tree"));
        }

        let bevy = |v: Vec3| Vec3::from(to_bevy(v.into()));
        let mut tris = Vec::new();
        for (_, ([a, b, c], n)) in faces {
            if (n.length() - 1.0).abs() > 1e-3 {
                return Err(bad("col: non-unit face normal"));
            }
            let (centre, toward) = ((a + b + c) / 3.0, n * PROBE_CM);
            // keep faces with air on exactly one side, normal towards the air
            let n = match (
                solid_at(&nodes, centre + toward),
                solid_at(&nodes, centre - toward),
            ) {
                (false, true) => n,
                (true, false) => -n,
                _ => continue,
            };
            let t = Tri {
                a: bevy(a) * SCALE,
                b: bevy(b) * SCALE,
                c: bevy(c) * SCALE,
                n: bevy(n),
            };
            if (t.b - t.a).cross(t.c - t.a).length() > 2.0 * MIN_AREA {
                tris.push(t);
            }
        }
        Ok(Self::build(tris))
    }

    fn build(mut tris: Vec<Tri>) -> Self {
        let mut nodes = Vec::new();
        if !tris.is_empty() {
            build_bvh(&mut tris, 0, &mut nodes);
        }
        Self { tris, nodes }
    }

    pub fn triangle_count(&self) -> usize {
        self.tris.len()
    }

    /// First surface along `dir` within `max` metres. Two-sided: the normal faces the origin.
    pub fn raycast(&self, origin: Vec3, dir: Vec3, max: f32) -> Option<Hit> {
        let dir = dir.try_normalize()?;
        let inv = dir.recip();
        let (mut best, mut tri) = (max, None);
        let mut stack = [0u32; 64];
        let mut sp = 0;
        if !self.nodes.is_empty() {
            stack[0] = 0;
            sp = 1;
        }
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            let (t1, t2) = ((node.min - origin) * inv, (node.max - origin) * inv);
            let enter = t1.min(t2).max_element().max(0.0);
            if t1.max(t2).min_element() < enter || enter > best {
                continue;
            }
            if node.count == 0 {
                stack[sp] = stack[sp].wrapping_add(1);
                stack[sp + 1] = node.first;
                sp += 2;
            } else {
                for t in &self.tris[node.first as usize..(node.first + node.count) as usize] {
                    if let Some(d) = t.ray(origin, dir).filter(|&d| d <= best) {
                        (best, tri) = (d, Some(t));
                    }
                }
            }
        }
        tri.map(|t| Hit {
            distance: best,
            point: origin + dir * best,
            normal: if t.n.dot(dir) > 0.0 { -t.n } else { t.n },
            surface: if t.n.dot(dir) > 0.0 { -t.n } else { t.n },
        })
    }

    /// First surface a sphere meets moving from `from` to `to`. `distance` is how far the
    /// centre travels before touching.
    pub fn sweep_sphere(&self, from: Vec3, to: Vec3, radius: f32) -> Option<Hit> {
        self.sweep(from, to - from, radius, &[0.0])
    }

    /// Sweeps spheres centred `from + Y * offset` for each offset.
    fn sweep(&self, from: Vec3, d: Vec3, r: f32, offsets: &[f32]) -> Option<Hit> {
        if self.nodes.is_empty() {
            return None;
        }
        let top = offsets.iter().copied().fold(0.0, f32::max);
        let lo = from.min(from + d) - Vec3::splat(r);
        let hi = from.max(from + d) + Vec3::splat(r) + Vec3::Y * top;
        let mut best: Option<((f32, Vec3, Vec3), Vec3)> = None;
        let mut stack = [0u32; 64];
        let mut sp = 1;
        while sp > 0 {
            sp -= 1;
            let node = &self.nodes[stack[sp] as usize];
            if node.min.cmpgt(hi).any() || node.max.cmplt(lo).any() {
                continue;
            }
            if node.count == 0 {
                stack[sp] = stack[sp].wrapping_add(1);
                stack[sp + 1] = node.first;
                sp += 2;
            } else {
                for t in &self.tris[node.first as usize..(node.first + node.count) as usize] {
                    for &o in offsets {
                        if let Some(h) = t.sweep(from + Vec3::Y * o, d, r)
                            // on a tie (a riser's top edge is also its tread's edge) report the flatter surface
                            && best.is_none_or(|b| {
                                h.0 < b.0.0 - 1e-5 || (h.0 < b.0.0 + 1e-5 && t.n.y > b.1.y)
                            })
                        {
                            best = Some((h, t.n));
                        }
                    }
                }
            }
        }
        let len = d.length();
        best.map(|((t, point, normal), surface)| Hit {
            distance: t * len,
            point,
            normal,
            surface,
        })
    }

    /// Sweeps an upright capsule (`feet` position, `radius`, total `height`) by `d`.
    fn sweep_capsule(&self, feet: Vec3, d: Vec3, r: f32, h: f32) -> Option<Hit> {
        let (offsets, n) = capsule_offsets(r, h);
        self.sweep(feet, d, r, &offsets[..n])
    }

    /// Collide-and-slide `d` (up to four contacts). Returns the end position and the
    /// contact normals met, in order.
    fn slide(&self, mut pos: Vec3, mut d: Vec3, r: f32, h: f32) -> (Vec3, [Vec3; 4], usize) {
        let mut planes = [Vec3::ZERO; 4];
        let mut n = 0;
        while n < 4 {
            let len = d.length();
            if len < 1e-6 {
                break;
            }
            let Some(hit) = self.sweep_capsule(pos, d, r, h) else {
                pos += d;
                break;
            };
            let dir = d / len;
            pos += dir * (hit.distance - SKIN).max(0.0);
            planes[n] = hit.normal;
            n += 1;
            d = clip(dir * (len - hit.distance).max(0.0), &planes[..n]);
        }
        (pos, planes, n)
    }

    /// Moves an upright capsule standing at `feet` by `delta` (gravity included by the caller):
    /// slides along walls, steps up ledges of `STEP` while grounded, and snaps down slopes.
    pub fn slide_move(&self, feet: Vec3, delta: Vec3, radius: f32, height: f32) -> Move {
        let grounded0 = delta.y <= 0.0
            && self
                .sweep_capsule(feet, Vec3::NEG_Y * GROUND_PROBE, radius, height)
                .is_some_and(|h| h.floor());
        let flat = Vec3::new(delta.x, 0.0, delta.z);
        let (mut pos, planes, n) = self.slide(feet, flat, radius, height);
        let blocked = |planes: &[Vec3]| planes.iter().copied().find(|p| p.y.abs() < WALKABLE);
        let mut wall = blocked(&planes[..n]);

        if grounded0 && wall.is_some() {
            let up = self
                .sweep_capsule(feet, Vec3::Y * STEP, radius, height)
                .map_or(STEP, |h| (h.distance - SKIN).max(0.0));
            // A walker that just hit a riser has no speed: probe at least `STEP_REACH` ahead,
            // or the raised capsule would land on the lower floor again.
            let len = flat.length();
            let ahead = if len < STEP_REACH {
                flat / len * STEP_REACH
            } else {
                flat
            };
            let (top, ..) = self.slide(feet + Vec3::Y * up, ahead, radius, height);
            let landed = self
                .sweep_capsule(top, Vec3::NEG_Y * (up + GROUND_PROBE), radius, height)
                .filter(Hit::floor);
            let reach = |p: Vec3| Vec2::new(p.x - feet.x, p.z - feet.z).length();
            if let Some(h) = landed {
                let stepped = top - Vec3::Y * (h.distance - SKIN).max(0.0);
                // any progress counts: a walker that just hit a riser has lost its speed
                if reach(stepped) > reach(pos) + 1e-4 {
                    (pos, wall) = (stepped, None);
                }
            }
        }

        let mut out = Move {
            pos,
            grounded: false,
            ground_normal: None,
            wall,
            ceiling: false,
        };
        if delta.y > 0.0 {
            let (p, planes, n) = self.slide(pos, Vec3::Y * delta.y, radius, height);
            out.pos = p;
            out.ceiling = planes[..n].iter().any(|p| p.y < 0.0);
            return out;
        }
        let snap = if grounded0 { STEP } else { 0.0 };
        let reach = -delta.y + snap;
        match self.sweep_capsule(pos, Vec3::NEG_Y * reach, radius, height) {
            Some(h) if h.floor() => {
                out.pos.y = pos.y - (h.distance - SKIN).max(0.0);
                out.grounded = true;
                out.ground_normal = Some(h.surface);
            }
            // steep surface: keep sliding down it, never snap
            Some(_) => out.pos = self.slide(pos, Vec3::Y * delta.y, radius, height).0,
            None => out.pos.y = pos.y + delta.y,
        }
        out
    }
}

/// Sphere centre heights (above the feet) covering a capsule with no gap wider than `r`.
fn capsule_offsets(r: f32, h: f32) -> ([f32; MAX_SPHERES], usize) {
    let span = (h - 2.0 * r).max(0.0);
    let n = ((span / r).ceil() as usize).clamp(1, MAX_SPHERES - 1);
    let mut o = [0.0; MAX_SPHERES];
    if span == 0.0 {
        o[0] = r;
        return (o, 1);
    }
    for (i, v) in o.iter_mut().enumerate().take(n + 1) {
        *v = r + span * i as f32 / n as f32;
    }
    (o, n + 1)
}

/// Removes from `v` the parts pointing into any of `planes`; along a crease follows the edge.
fn clip(v: Vec3, planes: &[Vec3]) -> Vec3 {
    let mut out = v;
    for p in planes {
        out -= *p * out.dot(*p).min(0.0);
    }
    for (i, p) in planes.iter().enumerate() {
        if out.dot(*p) < -1e-4 {
            let crease = p.cross(planes[(i + 1) % planes.len()]).normalize_or_zero();
            return crease * v.dot(crease);
        }
    }
    out
}

fn build_bvh(tris: &mut [Tri], base: usize, nodes: &mut Vec<BvhNode>) {
    let (mut min, mut max) = (Vec3::INFINITY, Vec3::NEG_INFINITY);
    for t in tris.iter() {
        for v in [t.a, t.b, t.c] {
            (min, max) = (min.min(v), max.max(v));
        }
    }
    let idx = nodes.len();
    nodes.push(BvhNode {
        min,
        max,
        first: base as u32,
        count: tris.len() as u32,
    });
    if tris.len() <= LEAF_TRIS {
        return;
    }
    let e = max - min;
    let axis = if e.x >= e.y && e.x >= e.z {
        0
    } else if e.y >= e.z {
        1
    } else {
        2
    };
    let mid = tris.len() / 2;
    tris.select_nth_unstable_by(mid, |a, b| {
        a.centroid()[axis].total_cmp(&b.centroid()[axis])
    });
    let (l, r) = tris.split_at_mut(mid);
    build_bvh(l, base, nodes);
    nodes[idx].first = nodes.len() as u32;
    nodes[idx].count = 0;
    build_bvh(r, base + mid, nodes);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 10 m floor at y=0 and a wall at z=-2 (both facing the origin side).
    fn world() -> MapCollision {
        let tri = |a: [f32; 3], b: [f32; 3], c: [f32; 3], n: [f32; 3]| Tri {
            a: a.into(),
            b: b.into(),
            c: c.into(),
            n: n.into(),
        };
        let mut t = vec![
            tri([-5., 0., -5.], [5., 0., 5.], [5., 0., -5.], [0., 1., 0.]),
            tri([-5., 0., -5.], [-5., 0., 5.], [5., 0., 5.], [0., 1., 0.]),
            tri([-5., 0., -2.], [5., 0., -2.], [5., 5., -2.], [0., 0., 1.]),
            tri([-5., 0., -2.], [5., 5., -2.], [-5., 5., -2.], [0., 0., 1.]),
        ];
        // a 0.2 m ledge at z in [-1, -0.5] to step onto
        t.push(tri(
            [-5., 0.2, -1.],
            [5., 0.2, -0.5],
            [5., 0.2, -1.],
            [0., 1., 0.],
        ));
        t.push(tri(
            [-5., 0.2, -1.],
            [-5., 0.2, -0.5],
            [5., 0.2, -0.5],
            [0., 1., 0.],
        ));
        t.push(tri(
            [-5., 0., -0.5],
            [5., 0.2, -0.5],
            [5., 0., -0.5],
            [0., 0., 1.],
        ));
        t.push(tri(
            [-5., 0., -0.5],
            [-5., 0.2, -0.5],
            [5., 0.2, -0.5],
            [0., 0., 1.],
        ));
        MapCollision::build(t)
    }

    #[test]
    fn ray_sweep_and_slide() {
        let w = world();
        let h = w.raycast(Vec3::new(1., 1., 1.), Vec3::NEG_Y, 10.).unwrap();
        assert!((h.distance - 1.).abs() < 1e-5 && h.normal == Vec3::Y);
        let h = w
            .sweep_sphere(Vec3::new(0., 1., 2.), Vec3::new(0., 1., -4.), 0.3)
            .unwrap();
        assert!(h.normal.z > 0.99, "{h:?}");
        // stands on the floor, is blocked by the wall, climbs the 0.2 m ledge
        let m = w.slide_move(
            Vec3::new(0., 0., 1.),
            Vec3::new(0., -0.01, -0.05),
            0.35,
            1.8,
        );
        assert!(m.grounded && m.pos.y.abs() < 0.01);
        let m = w.slide_move(
            Vec3::new(0., 0., -1.64),
            Vec3::new(0., -0.01, -5.),
            0.35,
            1.8,
        );
        assert!(m.pos.z > -1.66 && m.wall.is_some(), "{m:?}");
        let m = w.slide_move(
            Vec3::new(0., 0., -0.1),
            Vec3::new(0., -0.01, -0.9),
            0.35,
            1.8,
        );
        assert!((m.pos.y - 0.2).abs() < 0.01 && m.grounded, "{m:?}");
    }
}
