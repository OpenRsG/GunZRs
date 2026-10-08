//! RealSpace ELU models (`.elu`). Layouts and evidence: `docs/formats.md`.
//!
//! Coordinates are returned as stored: left-handed, Y up, centimetres; every matrix is
//! D3D row-vector style (`p' = p * M`, translation in the last row), row-major `[f32; 16]`.

use std::io::{self, ErrorKind};

const MAGIC: u32 = 0x0107_F060;

#[derive(Clone, Debug, Default)]
pub struct Material {
    /// Material id referenced by [`Node::material`].
    pub id: i32,
    /// Sub-material index within `id`, or -1 for the (parent) material itself.
    pub sub: i32,
    /// For a parent material: number of `sub` materials that precede it in the list.
    pub sub_count: i32,
    pub ambient: [f32; 4],
    pub diffuse: [f32; 4],
    pub specular: [f32; 4],
    pub power: f32,
    /// Diffuse texture file name (may be `.bmp`/`.tga` with a `.dds` twin; effect ELUs may use
    /// the `txa <frames> <ms> <file>` animation syntax). Empty for parent materials.
    pub texture: String,
    pub alpha_texture: String,
    pub two_sided: bool,
    pub additive: bool,
    /// Alpha-test reference (0..=255), 0 = off. Only stored from version 0x5007 on.
    pub alpha_ref: i32,
}

#[derive(Clone, Copy, Debug)]
pub struct Face {
    /// Indices into [`Node::positions`], file winding.
    pub pos: [u32; 3],
    /// Per-corner texture coordinates (u, v), D3D convention (v down).
    pub uv: [[f32; 2]; 3],
    pub face_normal: [f32; 3],
    /// Per-corner normals. Computed (area-weighted) for versions that do not store them.
    pub normals: [[f32; 3]; 3],
    /// Sub-material index; meaningful when the node's material has `sub_count > 0`.
    pub sub_material: i32,
    /// Second per-face integer (unknown purpose, observed 0..=7 or more).
    pub group: i32,
}

#[derive(Clone, Debug)]
pub struct Influence {
    pub bone: String,
    pub weight: f32,
    /// Vertex position in the bone's local space (`pos * inverse(bone world)`).
    pub offset: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    /// Empty for root nodes.
    pub parent: String,
    /// World (bind) transform of the node.
    pub world: [f32; 16],
    /// Unknown second matrix stored next to `world` (versions >= 0x5004).
    pub aux: [f32; 16],
    /// `positions` are in node-local space: `world_pos = pos * world`.
    pub positions: Vec<[f32; 3]>,
    pub faces: Vec<Face>,
    /// Per-vertex colours (versions >= 0x5005, usually empty).
    pub colors: Vec<[f32; 3]>,
    /// Material id, -1 for none.
    pub material: i32,
    /// Empty (rigid node) or one entry per position, weights summing to 1, 1..=4 influences.
    pub skin: Vec<Vec<Influence>>,
}

#[derive(Clone, Debug)]
pub struct Elu {
    pub version: u32,
    pub materials: Vec<Material>,
    pub nodes: Vec<Node>,
}

impl Elu {
    /// Index into `materials` of material `id`, resolved to sub-material `sub` when the
    /// parent material has sub-materials. `None` if the id is missing.
    pub fn material_index(&self, id: i32, sub: i32) -> Option<usize> {
        let parent = self.materials.iter().find(|m| m.id == id && m.sub == -1)?;
        let want = if parent.sub_count > 0 {
            sub.clamp(0, parent.sub_count - 1)
        } else {
            -1
        };
        self.materials
            .iter()
            .position(|m| m.id == id && m.sub == want)
    }
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.into())
}

/// Bounds-checked little-endian reader.
struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn bytes(&mut self, n: usize) -> io::Result<&'a [u8]> {
        let end = self
            .pos
            .checked_add(n)
            .filter(|&e| e <= self.buf.len())
            .ok_or_else(|| {
                bad(format!(
                    "read of {n} bytes at {} overruns {}",
                    self.pos,
                    self.buf.len()
                ))
            })?;
        let s = &self.buf[self.pos..end];
        self.pos = end;
        Ok(s)
    }
    fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    /// Non-negative count, capped against the remaining bytes (`min_size` per element).
    fn count(&mut self, min_size: usize) -> io::Result<usize> {
        let at = self.pos;
        let n = self.i32()?;
        if n < 0 || (n as usize).saturating_mul(min_size) > self.buf.len() - self.pos {
            return Err(bad(format!("count {n} at {at} exceeds data")));
        }
        Ok(n as usize)
    }
    fn vec<const N: usize>(&mut self) -> io::Result<[f32; N]> {
        let mut v = [0.0; N];
        for x in &mut v {
            *x = self.f32()?;
        }
        Ok(v)
    }
    /// Fixed-size NUL-padded string field.
    fn fixed(&mut self, n: usize) -> io::Result<String> {
        let s = self.bytes(n)?;
        let len = s.iter().position(|&b| b == 0).unwrap_or(n);
        Ok(String::from_utf8_lossy(&s[..len]).into_owned())
    }
    fn u8(&mut self) -> io::Result<u8> {
        Ok(self.bytes(1)?[0])
    }
    fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    /// `u16` length-prefixed string.
    fn str16(&mut self) -> io::Result<String> {
        let n = self.u16()? as usize;
        Ok(String::from_utf8_lossy(self.bytes(n)?).into_owned())
    }
}

/// Field layout differences between ELU versions 0x5004..=0x5007, 0x11 and 0.
struct Layout {
    /// Size of the name / alpha-texture fields in a material.
    name_len: usize,
    /// Number of trailing material ints: two-sided, additive, alpha reference.
    flag_ints: usize,
    /// Node header carries the decomposed-transform block and the auxiliary matrix.
    full_header: bool,
    /// Faces carry a `group` int; per-face + per-corner normals and vertex colours are stored.
    normals: bool,
    /// Version 0: `u16`-prefixed strings, u16 counts, unaligned stream.
    packed: bool,
}

fn layout(version: u32) -> Option<Layout> {
    let l = |name_len, flag_ints, full_header, normals, packed| Layout {
        name_len,
        flag_ints,
        full_header,
        normals,
        packed,
    };
    match version {
        0 => Some(l(0, 0, true, true, true)),
        0x11 => Some(l(40, 0, false, false, false)),
        0x5004 => Some(l(40, 2, true, false, false)),
        0x5005 => Some(l(40, 2, true, true, false)),
        0x5006 => Some(l(256, 2, true, true, false)),
        0x5007 => Some(l(256, 3, true, true, false)),
        _ => None,
    }
}

fn material(r: &mut Reader, l: &Layout) -> io::Result<Material> {
    let (id, sub) = (r.i32()?, r.i32()?);
    let (ambient, diffuse, specular) = (r.vec()?, r.vec()?, r.vec()?);
    let power = r.f32()?;
    let sub_count = r.i32()?;
    if l.packed {
        let (texture, alpha_texture) = (r.str16()?, r.str16()?);
        // 8 trailing bytes: bytes 0..2 and 6..8 vary per file without pattern (undecoded);
        // bytes 2..6 are an i32 that is 0 or 100 and is taken as the alpha reference.
        let t = r.bytes(8)?;
        let alpha_ref = i32::from_le_bytes(t[2..6].try_into().unwrap());
        return Ok(Material {
            id,
            sub,
            sub_count,
            ambient,
            diffuse,
            specular,
            power,
            texture,
            alpha_texture,
            two_sided: false,
            additive: false,
            alpha_ref,
        });
    }
    let texture = r.fixed(l.name_len)?;
    let alpha_texture = r.fixed(l.name_len)?;
    let mut flags = [0; 3];
    for f in &mut flags[..l.flag_ints] {
        *f = r.i32()?;
    }
    Ok(Material {
        id,
        sub,
        sub_count,
        ambient,
        diffuse,
        specular,
        power,
        texture,
        alpha_texture,
        two_sided: flags[0] != 0,
        additive: flags[1] != 0,
        alpha_ref: flags[2],
    })
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        v
    }
}

/// Area-weighted smooth normals for versions that store none.
fn compute_normals(positions: &[[f32; 3]], faces: &mut [Face]) {
    let mut acc = vec![[0.0f32; 3]; positions.len()];
    for f in faces.iter_mut() {
        let [a, b, c] = f.pos.map(|i| positions[i as usize]);
        let n = cross(sub(b, a), sub(c, a));
        f.face_normal = normalize(n);
        for i in f.pos {
            for k in 0..3 {
                acc[i as usize][k] += n[k];
            }
        }
    }
    for f in faces {
        f.normals = f.pos.map(|i| normalize(acc[i as usize]));
    }
}

fn node(r: &mut Reader, l: &Layout) -> io::Result<Node> {
    let (name, parent) = if l.packed {
        (r.str16()?, r.str16()?)
    } else {
        (r.fixed(40)?, r.fixed(40)?)
    };
    let (world, aux);
    if l.packed {
        // 155 floats: an axis, a scale matrix, the world matrix (floats 19..35), its inverse and
        // further copies of the same decomposition; only the world matrix is used.
        let h = r.vec::<155>()?;
        world = h[19..35].try_into().unwrap();
        aux = [0.0; 16];
    } else {
        world = r.vec::<16>()?;
        aux = if l.full_header {
            r.bytes(11 * 4)?; // decomposed rotation/scale of the node (axis-angle), not used
            r.vec()?
        } else {
            [0.0; 16]
        };
    }
    let nv = r.count(12)?;
    let positions = (0..nv)
        .map(|_| r.vec())
        .collect::<io::Result<Vec<[f32; 3]>>>()?;
    let nf = r.count(52)?;
    let mut faces = Vec::with_capacity(nf);
    for _ in 0..nf {
        let mut pos = [0u32; 3];
        for p in &mut pos {
            let i = r.i32()?;
            if i < 0 || i as usize >= nv {
                return Err(bad(format!("{name}: vertex index {i} of {nv}")));
            }
            *p = i as u32;
        }
        let mut uv = [[0.0; 2]; 3];
        for c in &mut uv {
            let [u, v, _w] = r.vec()?;
            *c = [u, v];
        }
        let sub_material = r.i32()?;
        let group = if l.full_header { r.i32()? } else { 0 };
        faces.push(Face {
            pos,
            uv,
            face_normal: [0.0; 3],
            normals: [[0.0; 3]; 3],
            sub_material,
            group,
        });
    }
    if l.normals {
        for f in &mut faces {
            f.face_normal = r.vec()?;
            for n in &mut f.normals {
                *n = r.vec()?;
            }
        }
    } else {
        compute_normals(&positions, &mut faces);
    }
    let colors = if l.normals {
        let n = r.count(12)?;
        (0..n).map(|_| r.vec()).collect::<io::Result<Vec<_>>>()?
    } else {
        Vec::new()
    };
    let material = r.i32()?;
    let np = r.count(if l.packed { 23 } else { 244 })?;
    if np != 0 && np != nv {
        return Err(bad(format!("{name}: {np} skin records for {nv} vertices")));
    }
    let mut skin = Vec::with_capacity(np);
    for _ in 0..np {
        skin.push(if l.packed {
            influences_packed(r, &name)?
        } else {
            influences(r, &name)?
        });
    }
    Ok(Node {
        name,
        parent,
        world,
        aux,
        positions,
        faces,
        colors,
        material,
        skin,
    })
}

/// One 244-byte skin record: 4 bone names, 4 weights, 4 ints (always 0), influence count,
/// 4 bone-space offsets.
fn influences(r: &mut Reader, node: &str) -> io::Result<Vec<Influence>> {
    let names = [r.fixed(40)?, r.fixed(40)?, r.fixed(40)?, r.fixed(40)?];
    let weights = r.vec::<4>()?;
    r.bytes(16)?;
    let n = r.i32()?;
    let offsets = [r.vec::<3>()?, r.vec()?, r.vec()?, r.vec()?];
    if !(1..=4).contains(&n) {
        return Err(bad(format!("{node}: {n} skin influences")));
    }
    let n = n as usize;
    let sum: f32 = weights[..n].iter().sum();
    if names[..n].iter().any(String::is_empty) || (sum - 1.0).abs() > 0.01 {
        return Err(bad(format!(
            "{node}: skin record names/weights inconsistent"
        )));
    }
    Ok((0..n)
        .map(|i| Influence {
            bone: names[i].clone(),
            weight: weights[i],
            offset: offsets[i],
        })
        .collect())
}

/// Version 0 skin record: `u8` influence count (1..=4), then per influence `i32` bone index
/// (into the file's node list), `f32` weight, bone-space offset, `u16`-prefixed bone name.
fn influences_packed(r: &mut Reader, node: &str) -> io::Result<Vec<Influence>> {
    let n = r.u8()?;
    if !(1..=4).contains(&n) {
        return Err(bad(format!("{node}: {n} skin influences")));
    }
    let mut out = Vec::with_capacity(n as usize);
    for _ in 0..n {
        let _index = r.i32()?;
        let weight = r.f32()?;
        let offset = r.vec()?;
        out.push(Influence {
            bone: r.str16()?,
            weight,
            offset,
        });
    }
    let sum: f32 = out.iter().map(|i| i.weight).sum();
    if out.iter().any(|i| i.bone.is_empty()) || (sum - 1.0).abs() > 0.01 {
        return Err(bad(format!(
            "{node}: skin record names/weights inconsistent"
        )));
    }
    Ok(out)
}

pub fn load(bytes: &[u8]) -> io::Result<Elu> {
    let mut r = Reader { buf: bytes, pos: 0 };
    let (magic, version) = (r.i32()? as u32, r.i32()? as u32);
    if magic != MAGIC {
        return Err(bad(format!("elu: magic {magic:#x}")));
    }
    let l = layout(version).ok_or_else(|| bad(format!("elu: unsupported version {version:#x}")))?;
    let (materials, nodes);
    if l.packed {
        // The i32 count slots hold -1; real counts are u16, and materials follow the nodes.
        if (r.i32()?, r.i32()?) != (-1, -1) {
            return Err(bad("elu v0: header counts are not -1"));
        }
        let nn = r.u16()? as usize;
        nodes = (0..nn)
            .map(|_| node(&mut r, &l))
            .collect::<io::Result<Vec<_>>>()?;
        let nm = r.u16()? as usize;
        materials = (0..nm)
            .map(|_| material(&mut r, &l))
            .collect::<io::Result<Vec<_>>>()?;
    } else {
        let mat_min = 64 + 2 * l.name_len + 4 * l.flag_ints;
        let nm = r.count(mat_min)?;
        let nn = r.count(80 + 64 + 4 + 4 + 4)?;
        materials = (0..nm)
            .map(|_| material(&mut r, &l))
            .collect::<io::Result<Vec<_>>>()?;
        nodes = (0..nn)
            .map(|_| node(&mut r, &l))
            .collect::<io::Result<Vec<_>>>()?;
    }
    if r.pos != bytes.len() {
        return Err(bad(format!(
            "elu: {} trailing bytes after {} nodes",
            bytes.len() - r.pos,
            nodes.len()
        )));
    }
    Ok(Elu {
        version,
        materials,
        nodes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ints(out: &mut Vec<u8>, v: &[i32]) {
        v.iter().for_each(|x| out.extend(x.to_le_bytes()));
    }
    fn floats(out: &mut Vec<u8>, v: &[f32]) {
        v.iter().for_each(|x| out.extend(x.to_le_bytes()));
    }
    fn fixed(out: &mut Vec<u8>, s: &str, n: usize) {
        out.extend(s.as_bytes());
        out.resize(out.len() + n - s.len(), 0);
    }

    /// Synthetic version 0x11 file: one material, one triangle node (no stored normals).
    fn v11(trailing: bool) -> Vec<u8> {
        let mut b = Vec::new();
        ints(&mut b, &[MAGIC as i32, 0x11, 1, 1, 0, -1]);
        floats(&mut b, &[0.5; 12]);
        floats(&mut b, &[20.0]);
        ints(&mut b, &[0]);
        fixed(&mut b, "tex.dds", 40);
        fixed(&mut b, "", 40);
        fixed(&mut b, "tri", 40);
        fixed(&mut b, "", 40);
        floats(
            &mut b,
            &[
                1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
            ],
        );
        ints(&mut b, &[3]);
        floats(&mut b, &[0., 0., 0., 1., 0., 0., 0., 1., 0.]);
        ints(&mut b, &[1, 0, 1, 2]);
        floats(&mut b, &[0.; 9]);
        ints(&mut b, &[0, 0, 0]);
        if trailing {
            b.push(0);
        }
        b
    }

    #[test]
    fn old_version_computes_normals_and_rejects_trailing_bytes() {
        let e = load(&v11(false)).unwrap();
        assert_eq!(e.materials[0].texture, "tex.dds");
        assert_eq!(e.nodes[0].name, "tri");
        assert_eq!(e.nodes[0].faces[0].normals[0], [0.0, 0.0, 1.0]);
        assert!(load(&v11(true)).is_err());
    }
}
