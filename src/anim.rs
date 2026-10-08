//! Bevy playback of `.elu.ani` files: an [`Animator`] on a model root drives the descendants
//! whose `Name` equals an animation node name. Layouts: `docs/formats.md`.
//!
//! Space: ELU/ANI are left-handed; they are mirrored into Bevy by `S = diag(1, 1, -1)`
//! (positions `(x, y, -z)`, matrices `S M^T S`), in centimetres. Scaling to metres is the
//! model root's job (`view::SCALE`), so animated local translations stay in centimetres.

use crate::{
    ani::{Ani, FPS, Key, Kind, Node, TICKS_PER_FRAME, VertexTrack},
    elu::{Elu, Node as EluNode},
    model::node_batches,
    view::SCALE,
};
use bevy::prelude::*;
use std::{collections::HashMap, sync::Arc};

pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, animate);
    }
}

/// How an animation behaves at its last frame (character XML `motion_loop_type`). Values in
/// `man01.xml` (`woman01.xml` alike): `lastframe` 410, `loop` 186, `onceidle` 164,
/// `onceLowerbody` 59, `lonceidle` 2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Loop {
    /// `loop`.
    Wrap,
    /// `lastframe`: stop on the last frame.
    Hold,
    /// `onceidle`, `lonceidle` (only `runRW` of one motion type; no other difference found):
    /// plays once, then the actor returns to idle ([`Animator::finished`]); holds meanwhile.
    OnceIdle,
    /// `onceLowerbody`: plays once as the upper-body layer ([`Animator::set_upper`]) while the
    /// legs keep their locomotion. Observed: gun `attackS`/`reload`/`load` and 2hdagger
    /// `attack1/2` carry no leg keys (upper-body-only clips); knife/sword ones key both halves.
    OnceLower,
}

impl Loop {
    /// `loop` wraps; `onceidle`/`lonceidle` and `onceLowerbody` as above; anything else
    /// (`lastframe`) holds.
    pub fn from_xml(s: &str) -> Self {
        match s {
            "loop" => Loop::Wrap,
            "onceidle" | "lonceidle" => Loop::OnceIdle,
            "onceLowerbody" => Loop::OnceLower,
            _ => Loop::Hold,
        }
    }

    pub fn wraps(self) -> bool {
        self == Loop::Wrap
    }
}

/// Alpha multiplier from a node's visibility keys, set on the animated entity (when keyed).
/// `Visibility` is also switched to hidden at 0.
#[derive(Component, Clone, Copy, Debug)]
pub struct NodeAlpha(pub f32);

/// The skeleton root: the 3ds Max biped centre of mass, a direct child of the model root.
const ROOT: &str = "Bip01";
/// Bones of this node's subtree (spine1/2, neck, head, clavicles, arms, hands, `eq_w*` attach
/// nodes) form the upper body; the rest (`Bip01`, `Footsteps`, pelvis, `Bip01 Spine` and the
/// legs) the lower body. **Observed** in `man-set-000.elu`: both `Bip01 L/R Thigh` hang off
/// `Bip01 Spine`, so the hierarchy only separates the halves from `Spine1` up; every
/// upper-body-only clip keys `Spine1`, `Spine2`, `Head` and the arms, none keys the calves.
const SPLIT: &str = "Bip01 Spine1";
/// Bones that carry the aim pitch (above [`SPLIT`], so the legs stay put).
const SPINE: [&str; 2] = ["Bip01 Spine1", "Bip01 Spine2"];
/// Share of the aim pitch per [`SPINE`] bone (**inferred**; sums to 1).
const SPINE_SHARE: [f32; 2] = [0.5, 0.5];

/// Model-space horizontal root position (Bevy axes, cm) at `secs`.
fn root_xz(node: &Node, secs: f32) -> Vec2 {
    sample(
        &node.pos,
        secs * FPS * TICKS_PER_FRAME as f32,
        pos,
        Vec3::lerp,
    )
    .map_or(Vec2::ZERO, |p| Vec2::new(p.x, p.z))
}

/// Horizontal movement of `node` between two clip times as an [`root_delta`] vector.
fn root_move(node: &Node, t0: f32, t1: f32) -> Vec3 {
    let d = root_xz(node, t1) - root_xz(node, t0);
    // Model root faces +Z; the actor model is turned by PI so it faces -Z.
    Vec3::new(-d.x, 0.0, -d.y) * SCALE
}

/// Horizontal displacement of the root bone `Bip01` between clip times `t0` and `t1` (seconds,
/// clamped to the clip) in metres, in the actor frame (-Z forward, +X right, `y` 0). Retail
/// melee clips lunge by it (katana `attack1` ~1 m forward); locomotion and tumbles have none.
pub fn root_delta(ani: &Ani, t0: f32, t1: f32) -> Vec3 {
    ani.nodes
        .iter()
        .find(|n| n.name == ROOT)
        .map_or(Vec3::ZERO, |n| root_move(n, t0, t1))
}

/// An upper-body clip played over the main one.
struct Upper {
    ani: Arc<Ani>,
    time: f32,
    looping: Loop,
    targets: Option<Resolved>,
    /// Mix with the main clip, `0..=1`.
    weight: f32,
    /// Seconds to fade in and out (0: instant).
    fade: f32,
    releasing: bool,
}

/// A cross-fade from the pose shown when a clip was replaced.
struct Fade {
    total: f32,
    left: f32,
    from: HashMap<Entity, Transform>,
    /// Targets of the replaced clip until the first tick after the switch captures `from`.
    old: Option<Resolved>,
}

#[derive(Component)]
pub struct Animator {
    pub ani: Arc<Ani>,
    pub time: f32,
    pub speed: f32,
    pub looping: Loop,
    /// The ELU the model was spawned from; only needed to play vertex tracks.
    pub elu: Option<Arc<Elu>>,
    /// Keep `Bip01` at its frame-0 horizontal position so the visual stays on its entity; the
    /// caller moves the entity by [`Animator::take_root_motion`].
    pub root_lock: bool,
    /// Radians, positive aims up; bent through the spine after sampling (bone clips only).
    pub aim_pitch: f32,
    /// Playback speed of the upper-body clip (on top of `speed`); [`Animator::set_upper`]
    /// resets it to 1, so set it after that call (reload clips stretched to the weapon's time).
    pub upper_speed: f32,
    /// `None` until the model hierarchy exists.
    targets: Option<Resolved>,
    upper: Option<Upper>,
    fade: Option<Fade>,
    root_acc: Vec3,
}

impl Animator {
    pub fn new(ani: Arc<Ani>, looping: Loop) -> Self {
        Self {
            ani,
            time: 0.0,
            speed: 1.0,
            looping,
            elu: None,
            root_lock: false,
            aim_pitch: 0.0,
            upper_speed: 1.0,
            targets: None,
            upper: None,
            fade: None,
            root_acc: Vec3::ZERO,
        }
    }

    /// Replaces the main clip, restarting at 0 and cross-fading from the pose on screen over
    /// `blend` seconds (0: hard cut). No blend time is stored in the data (**inferred**).
    pub fn play(&mut self, ani: Arc<Ani>, looping: Loop, blend: f32) {
        let old = self.targets.take();
        let pending = self.fade.take().and_then(|f| f.old);
        self.fade = (blend > 0.0)
            .then_some(old)
            .flatten()
            .or(pending)
            .map(|old| Fade {
                total: blend,
                left: blend,
                from: HashMap::new(),
                old: Some(old),
            });
        self.ani = ani;
        self.looping = looping;
        self.time = 0.0;
    }

    /// Plays `ani` over the upper body (`Bip01 Spine1` subtree) from its start, fading in over
    /// `fade` seconds; a clip that does not wrap releases itself (fading out) at its end.
    pub fn set_upper(&mut self, ani: Arc<Ani>, looping: Loop, fade: f32) {
        self.upper_speed = 1.0;
        self.upper = Some(Upper {
            ani,
            time: 0.0,
            looping,
            targets: None,
            weight: self.upper.as_ref().map_or(0.0, |u| u.weight),
            fade,
            releasing: false,
        });
    }

    /// Fades the upper-body clip out over `fade` seconds.
    pub fn clear_upper(&mut self, fade: f32) {
        if let Some(u) = &mut self.upper {
            u.releasing = true;
            u.fade = fade;
        }
    }

    /// No upper-body clip, or it has played to its end (it may still be fading out).
    pub fn upper_done(&self) -> bool {
        self.upper.as_ref().is_none_or(|u| {
            u.releasing || (!u.looping.wraps() && u.time >= u.ani.max_frame as f32 / FPS)
        })
    }

    /// Seconds into the upper-body clip.
    pub fn upper_time(&self) -> Option<f32> {
        self.upper.as_ref().map(|u| u.time)
    }

    /// Jumps the upper-body clip to `secs`.
    pub fn seek_upper(&mut self, secs: f32) {
        if let Some(u) = &mut self.upper {
            u.time = secs;
        }
    }

    /// A clip that does not wrap has reached its last frame (`Loop::OnceIdle`: back to idle).
    pub fn finished(&self) -> bool {
        !self.looping.wraps() && self.time >= self.duration()
    }

    /// Root movement ([`root_delta`] frame) of the main clip since the last call.
    pub fn take_root_motion(&mut self) -> Vec3 {
        std::mem::take(&mut self.root_acc)
    }

    /// Animation length in seconds.
    pub fn duration(&self) -> f32 {
        self.ani.max_frame as f32 / FPS
    }

    fn tick(&self) -> f32 {
        self.time * FPS * TICKS_PER_FRAME as f32
    }
}

const S: Mat4 = Mat4::from_diagonal(Vec4::new(1.0, 1.0, -1.0, 1.0));

/// D3D row-major row-vector matrix -> Bevy column-vector matrix.
fn matrix(m: &[f32; 16]) -> Mat4 {
    S * Mat4::from_cols_array(m) * S // from_cols_array of row-major data is M^T
}

fn pos([x, y, z]: [f32; 3]) -> Vec3 {
    Vec3::new(x, y, -z)
}

/// D3DX quaternion `[x, y, z, w]` (verified against bone bind matrices) mirrored by `S`:
/// a reflection flips the rotation axis.
fn rot([x, y, z, w]: [f32; 4]) -> Quat {
    Quat::from_xyzw(-x, -y, z, w)
}

/// Key value at `tick` (clamped to the first/last key), converted by `conv` and interpolated
/// by `lerp` in the converted space.
fn sample<T: Copy, U>(
    keys: &[Key<T>],
    tick: f32,
    conv: impl Fn(T) -> U,
    lerp: impl Fn(U, U, f32) -> U,
) -> Option<U> {
    let (first, last) = (keys.first()?, keys.last()?);
    if tick <= first.tick as f32 {
        return Some(conv(first.value));
    }
    if tick >= last.tick as f32 {
        return Some(conv(last.value));
    }
    let i = keys.partition_point(|k| (k.tick as f32) <= tick);
    let (a, b) = (&keys[i - 1], &keys[i]);
    Some(lerp(
        conv(a.value),
        conv(b.value),
        (tick - a.tick as f32) / (b.tick - a.tick) as f32,
    ))
}

/// Local transform of `node` at `tick`; `fallback` supplies channels without keys.
pub fn sample_transform(node: &Node, tick: f32, fallback: Transform) -> Transform {
    let mut t = fallback;
    if let Some(p) = sample(&node.pos, tick, pos, Vec3::lerp) {
        t.translation = p;
    }
    if let Some(q) = sample(&node.rot, tick, rot, Quat::slerp) {
        t.rotation = q;
    }
    if let Some(m) = sample(
        &node.tm,
        tick,
        |m| Transform::from_matrix(matrix(&m)),
        blend,
    ) {
        t = m;
    }
    t
}

pub fn sample_alpha(node: &Node, tick: f32) -> Option<f32> {
    sample(&node.vis, tick, |a| a, |a, b, f| a + (b - a) * f)
}

/// A named entity driven by one animation node.
struct Target {
    e: Entity,
    /// Transform when no key applies.
    fallback: Transform,
    /// Below the [`SPLIT`] bone: the part an upper-body layer replaces.
    upper: bool,
}

/// An animation matched to a model hierarchy.
struct Resolved {
    /// Per ani node: the entities carrying that name.
    nodes: Vec<Vec<Target>>,
    /// Index of the [`ROOT`] node.
    root: Option<usize>,
    /// Indices of the [`SPINE`] nodes.
    spine: [Option<usize>; 2],
}

/// Matches animation nodes to named descendants of `root`. A bone's fallback transform is the
/// ANI's own frame-0 local (`inverse(parent base) * base`): unkeyed channels are not always the
/// ELU bind pose (`man_knife_idle`: `Bip01 R Hand`, `Bip01 Footsteps`).
fn resolve(
    root: Entity,
    ani: &Ani,
    children: &Query<&Children>,
    parents: &Query<&ChildOf>,
    names: &Query<&Name>,
    transforms: &Query<&Transform>,
) -> Resolved {
    let mut by_name: HashMap<&str, Vec<Entity>> = HashMap::new();
    for e in std::iter::once(root).chain(children.iter_descendants(root)) {
        if let Ok(n) = names.get(e) {
            by_name.entry(n.as_str()).or_default().push(e);
        }
    }
    let base_of = |name: &str| ani.nodes.iter().find(|n| n.name == name)?.base;
    let under_split = |mut e: Entity| {
        while e != root {
            if names.get(e).is_ok_and(|n| n.as_str() == SPLIT) {
                return true;
            }
            match parents.get(e) {
                Ok(p) => e = p.parent(),
                Err(_) => break,
            }
        }
        false
    };
    let index = |name: &str| ani.nodes.iter().position(|n| n.name == name);
    Resolved {
        nodes: ani
            .nodes
            .iter()
            .map(|n| {
                by_name
                    .get(n.name.as_str())
                    .into_iter()
                    .flatten()
                    .filter_map(|&e| {
                        let mut t = *transforms.get(e).ok()?;
                        if let Some(base) = n.base {
                            let parent = parents.get(e).ok()?.parent();
                            let local = if parent == root {
                                Some(matrix(&base))
                            } else {
                                let pb = base_of(names.get(parent).ok()?.as_str());
                                pb.map(|pb| matrix(&pb).inverse() * matrix(&base))
                            };
                            if let Some(l) = local {
                                t = Transform::from_matrix(l);
                            }
                        }
                        Some(Target {
                            e,
                            fallback: t,
                            upper: under_split(e),
                        })
                    })
                    .collect()
            })
            .collect(),
        root: index(ROOT),
        spine: SPINE.map(index),
    }
}

/// Model-space matrix of `e`: its sampled world matrix when keyed this tick, else its
/// parent's world times its current local transform.
fn world_of(
    e: Entity,
    root: Entity,
    keyed: &HashMap<Entity, Mat4>,
    parents: &Query<&ChildOf>,
    transforms: &Query<&Transform>,
) -> Mat4 {
    if e == root {
        return Mat4::IDENTITY;
    }
    if let Some(m) = keyed.get(&e) {
        return *m;
    }
    let local = transforms.get(e).map_or(Mat4::IDENTITY, |t| t.to_matrix());
    match parents.get(e) {
        Ok(p) => world_of(p.parent(), root, keyed, parents, transforms) * local,
        Err(_) => local,
    }
}

/// Node-local vertex positions of `node` at `tick`, ELU space. Retail vertex tracks are mostly
/// stored in model space (`local * world`, 92 of 117) but some match the local positions
/// (`model/worlditem/ef_prop`); frame 0 tells which.
fn vertex_positions(track: &VertexTrack, elu: &EluNode, tick: f32) -> Vec<[f32; 3]> {
    let at = |f: usize| &track.positions[f * track.vertex_count..][..track.vertex_count];
    let i = track.ticks.partition_point(|&t| t as f32 <= tick);
    let lerp = |a: &[f32; 3], b: &[f32; 3], f: f32| Vec3::from(*a).lerp(Vec3::from(*b), f);
    let positions: Vec<Vec3> = if i == 0 {
        at(0).iter().map(|&p| p.into()).collect()
    } else if i == track.ticks.len() {
        at(i - 1).iter().map(|&p| p.into()).collect()
    } else {
        let f = (tick - track.ticks[i - 1] as f32) / (track.ticks[i] - track.ticks[i - 1]) as f32;
        at(i - 1)
            .iter()
            .zip(at(i))
            .map(|(a, b)| lerp(a, b, f))
            .collect()
    };
    let world = Mat4::from_cols_array(&elu.world);
    let local_space = at(0)
        .iter()
        .zip(&elu.positions)
        .all(|(a, b)| Vec3::from(*a).distance(Vec3::from(*b)) < 1e-2);
    let to_local = if local_space {
        Mat4::IDENTITY
    } else {
        world.inverse()
    };
    positions
        .iter()
        .map(|&p| to_local.transform_point3(p).to_array())
        .collect()
}

/// Linear blend of two transforms (slerp for rotation); `w` in `0..=1`.
fn blend(a: Transform, b: Transform, w: f32) -> Transform {
    Transform {
        translation: a.translation.lerp(b.translation, w),
        rotation: a.rotation.slerp(b.rotation, w),
        scale: a.scale.lerp(b.scale, w),
    }
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Time `t + dt` handled per loop mode over a clip of `end` seconds.
fn advance(t: f32, dt: f32, end: f32, looping: Loop) -> f32 {
    if looping.wraps() && end > 0.0 {
        (t + dt).rem_euclid(end)
    } else {
        (t + dt).clamp(0.0, end)
    }
}

/// Model-space rotation of the parent chain above `e` (the animator root excluded).
fn parent_rotation(
    e: Entity,
    root: Entity,
    parents: &Query<&ChildOf>,
    transforms: &Query<&mut Transform>,
) -> Quat {
    let mut q = Quat::IDENTITY;
    let mut cur = e;
    while let Ok(p) = parents.get(cur) {
        cur = p.parent();
        if cur == root {
            break;
        }
        q = transforms.get(cur).map_or(Quat::IDENTITY, |t| t.rotation) * q;
    }
    q
}

fn animate(
    time: Res<Time>,
    mut animators: Query<(Entity, &mut Animator)>,
    children: Query<&Children>,
    parents: Query<&ChildOf>,
    names: Query<&Name>,
    mut transforms: Query<&mut Transform>,
    mut visibility: Query<&mut Visibility>,
    mesh_handles: Query<&Mesh3d>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (root, mut a) in &mut animators {
        let a = &mut *a;
        if a.targets.is_none() {
            let r = resolve(
                root,
                &a.ani,
                &children,
                &parents,
                &names,
                &transforms.as_readonly(),
            );
            if r.nodes.iter().all(Vec::is_empty) {
                continue; // hierarchy not spawned yet
            }
            a.targets = Some(r);
        }
        // A clip switch with a cross-fade: remember the pose being left, before it is overwritten.
        if let Some(f) = a.fade.as_mut()
            && let Some(old) = f.old.take()
        {
            for t in old.nodes.iter().flatten() {
                if let Ok(tf) = transforms.get(t.e) {
                    f.from.insert(t.e, *tf);
                }
            }
        }
        let end = a.duration();
        let prev = a.time;
        a.time = advance(a.time, dt * a.speed, end, a.looping);
        let r = a.targets.as_ref().unwrap();
        if let Some(node) = r.root.map(|i| &a.ani.nodes[i]) {
            let d = if a.time < prev {
                root_move(node, prev, end) + root_move(node, 0.0, a.time)
            } else {
                root_move(node, prev, a.time)
            };
            a.root_acc += d;
        }
        // Upper-body layer: advance, resolve, release at the end of a clip that plays once.
        if let Some(u) = a.upper.as_mut() {
            if u.targets.is_none() {
                let r = resolve(
                    root,
                    &u.ani,
                    &children,
                    &parents,
                    &names,
                    &transforms.as_readonly(),
                );
                u.targets = Some(r);
            }
            let uend = u.ani.max_frame as f32 / FPS;
            u.time = advance(u.time, dt * a.speed * a.upper_speed, uend, u.looping);
            if !u.looping.wraps() && u.time >= uend {
                u.releasing = true;
            }
            let step = if u.fade > 0.0 { dt / u.fade } else { 1.0 };
            u.weight = if u.releasing {
                (u.weight - step).max(0.0)
            } else {
                (u.weight + step).min(1.0)
            };
        }
        if a.upper
            .as_ref()
            .is_some_and(|u| u.releasing && u.weight <= 0.0)
        {
            a.upper = None;
        }
        let fade_w = match a.fade.as_mut() {
            Some(f) if f.left > dt => {
                f.left -= dt;
                Some(smooth(1.0 - f.left / f.total))
            }
            _ => {
                a.fade = None;
                None
            }
        };
        let tick = a.tick();
        let (a, r) = (&*a, a.targets.as_ref().unwrap());
        let from = a.fade.as_ref().map(|f| &f.from);
        // Transform tracks hold model-space matrices; turn them into parent-relative locals
        // once every keyed node's world matrix is known.
        let mut keyed = HashMap::new();
        if a.ani.kind == Kind::Transform {
            for (node, ts) in a.ani.nodes.iter().zip(&r.nodes) {
                for t in ts {
                    keyed.insert(t.e, sample_transform(node, tick, t.fallback).to_matrix());
                }
            }
        }
        for (i, (node, ts)) in a.ani.nodes.iter().zip(&r.nodes).enumerate() {
            let alpha = sample_alpha(node, tick);
            for t in ts {
                let e = t.e;
                match a.ani.kind {
                    Kind::Bone => {
                        let mut s = sample_transform(node, tick, t.fallback);
                        if a.root_lock && r.root == Some(i) && !node.pos.is_empty() {
                            let h = root_xz(node, 0.0);
                            (s.translation.x, s.translation.z) = (h.x, h.y);
                        }
                        if let (Some(w), Some(from)) = (fade_w, from)
                            && let Some(f) = from.get(&e)
                        {
                            s = blend(*f, s, w);
                        }
                        if let Ok(mut tf) = transforms.get_mut(e) {
                            *tf = s;
                        }
                    }
                    Kind::Transform => {
                        let parent = parents.get(e).map_or(root, ChildOf::parent);
                        let pw =
                            world_of(parent, root, &keyed, &parents, &transforms.as_readonly());
                        if let Ok(mut tf) = transforms.get_mut(e) {
                            *tf = Transform::from_matrix(pw.inverse() * keyed[&e]);
                        }
                    }
                    Kind::Vertex => {
                        let (Some(track), Some(elu)) = (&node.vertex, &a.elu) else {
                            continue;
                        };
                        let Some(en) = elu.nodes.iter().find(|n| n.name == node.name) else {
                            continue;
                        };
                        let mut en = en.clone();
                        if en.positions.len() != track.vertex_count {
                            continue;
                        }
                        en.positions = vertex_positions(track, &en, tick);
                        let batches = node_batches(elu, &en, None);
                        let handles = children
                            .get(e)
                            .into_iter()
                            .flatten()
                            .filter_map(|&c| mesh_handles.get(c).ok());
                        for (h, batch) in handles.zip(batches) {
                            if let Some(mut m) = meshes.get_mut(&h.0) {
                                *m = batch.mesh;
                            }
                        }
                    }
                }
                if let Some(al) = alpha {
                    commands.entity(e).insert(NodeAlpha(al));
                    if let Ok(mut v) = visibility.get_mut(e) {
                        *v = if al > 0.0 {
                            Visibility::Inherited
                        } else {
                            Visibility::Hidden
                        };
                    }
                }
            }
        }
        if a.ani.kind != Kind::Bone {
            continue;
        }
        // Upper-body layer: the masked bones take the upper clip, mixed by its weight.
        if let Some(u) = &a.upper
            && let Some(ur) = &u.targets
            && u.ani.kind == Kind::Bone
        {
            let utick = u.time * FPS * TICKS_PER_FRAME as f32;
            let w = smooth(u.weight);
            for (node, ts) in u.ani.nodes.iter().zip(&ur.nodes) {
                for t in ts.iter().filter(|t| t.upper) {
                    if let Ok(mut tf) = transforms.get_mut(t.e) {
                        *tf = blend(*tf, sample_transform(node, utick, t.fallback), w);
                    }
                }
            }
        }
        // Aim pitch: a model-space rotation about the lateral axis, split over the spine; the
        // bones above (arms, head, weapons) follow. Model forward is +Z, so up is -X rotation.
        if a.aim_pitch != 0.0 {
            for (bone, share) in r.spine.iter().zip(SPINE_SHARE) {
                for t in bone.map_or(&[][..], |i| &r.nodes[i]) {
                    let q = parent_rotation(t.e, root, &parents, &transforms);
                    if let Ok(mut tf) = transforms.get_mut(t.e) {
                        let d = Quat::from_rotation_x(-a.aim_pitch * share);
                        tf.rotation = q.inverse() * d * q * tf.rotation;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ani::Kind;
    use bevy::{asset::AssetPlugin, time::TimeUpdateStrategy};
    use std::f32::consts::FRAC_PI_2;
    use std::time::Duration;

    fn key<T>(frame: u32, value: T) -> Key<T> {
        Key {
            tick: frame * TICKS_PER_FRAME,
            value,
        }
    }

    /// Bone clip of `frames` frames whose nodes are `(name, pos keys, rot keys)`.
    type Track = (&'static str, Vec<Key<[f32; 3]>>, Vec<Key<[f32; 4]>>);
    fn clip(frames: u32, nodes: Vec<Track>) -> Arc<Ani> {
        let nodes = nodes
            .into_iter()
            .map(|(n, pos, rot)| Node {
                name: n.into(),
                pos,
                rot,
                ..Node::default()
            })
            .collect();
        Arc::new(Ani {
            version: 0x1003,
            max_frame: frames,
            kind: Kind::Bone,
            nodes,
        })
    }

    const TURN: [f32; 4] = [0.0, 0.0, 0.707_106_8, 0.707_106_8]; // 90 degrees about z
    const NONE: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

    fn angle(app: &App, e: Entity) -> f32 {
        app.world()
            .get::<Transform>(e)
            .unwrap()
            .rotation
            .to_axis_angle()
            .1
    }

    /// Root -> `Bip01` -> `Bip01 Spine1`, one frame (1/30 s) per update.
    fn rig(first: Arc<Ani>) -> (App, Entity, Entity, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), AnimPlugin))
            .init_asset::<Mesh>()
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(
                1.0 / 30.0,
            )));
        let w = app.world_mut();
        let root = w
            .spawn((Transform::default(), Animator::new(first, Loop::Hold)))
            .id();
        let bip = w
            .spawn((Name::new("Bip01"), Transform::default(), ChildOf(root)))
            .id();
        let spine = w
            .spawn((
                Name::new("Bip01 Spine1"),
                Transform::default(),
                ChildOf(bip),
            ))
            .id();
        (app, root, bip, spine)
    }

    fn run(app: &mut App, frames: u32) {
        for _ in 0..frames {
            app.update();
        }
    }

    #[test]
    fn fade_layer_and_root_motion() {
        let still = clip(
            30,
            vec![("Bip01 Spine1", vec![], vec![key(0, NONE), key(30, NONE)])],
        );
        let turned = clip(
            30,
            vec![("Bip01 Spine1", vec![], vec![key(0, TURN), key(30, TURN)])],
        );
        let (mut app, root, bip, spine) = rig(still.clone());
        run(&mut app, 3);
        assert!(angle(&app, spine) < 1e-3);

        // Cross-fade: half way through 0.5 s the pose is between the clips, then it is the new one.
        app.world_mut()
            .get_mut::<Animator>(root)
            .unwrap()
            .play(turned.clone(), Loop::Hold, 0.5);
        run(&mut app, 8);
        let mid = angle(&app, spine);
        assert!(mid > 0.2 && mid < 1.4, "mid-fade angle {mid}");
        run(&mut app, 20);
        assert!((angle(&app, spine) - FRAC_PI_2).abs() < 1e-3);

        // Upper layer replaces `Bip01 Spine1` (and below) only, then releases itself at its end.
        let upper = clip(
            5,
            vec![
                (
                    "Bip01",
                    vec![key(0, [0.0, 50.0, 0.0]), key(5, [0.0, 50.0, 0.0])],
                    vec![],
                ),
                ("Bip01 Spine1", vec![], vec![key(0, NONE), key(5, NONE)]),
            ],
        );
        app.world_mut()
            .get_mut::<Animator>(root)
            .unwrap()
            .set_upper(upper, Loop::OnceLower, 0.0);
        run(&mut app, 3);
        assert!(angle(&app, spine) < 1e-3, "upper clip drives the spine");
        assert!(app.world().get::<Transform>(bip).unwrap().translation.y != 50.0);
        run(&mut app, 6);
        assert!(app.world().get::<Animator>(root).unwrap().upper_done());
        assert!(
            (angle(&app, spine) - FRAC_PI_2).abs() < 1e-3,
            "main clip again"
        );

        // Root motion: 100 cm along ani -z over 10 frames is 1 m forward (-Z); the visual stays.
        let lunge = clip(
            10,
            vec![(
                "Bip01",
                vec![key(0, [0.0, 90.0, 0.0]), key(10, [0.0, 90.0, -100.0])],
                vec![],
            )],
        );
        {
            let mut a = app.world_mut().get_mut::<Animator>(root).unwrap();
            a.root_lock = true;
            a.play(lunge, Loop::Hold, 0.0);
            a.take_root_motion();
        }
        run(&mut app, 14);
        let moved = app
            .world_mut()
            .get_mut::<Animator>(root)
            .unwrap()
            .take_root_motion();
        assert!(
            (moved - Vec3::new(0.0, 0.0, -1.0)).length() < 1e-3,
            "root motion {moved}"
        );
        let t = app.world().get::<Transform>(bip).unwrap().translation;
        assert!(t.x == 0.0 && t.z == 0.0 && t.y > 89.0, "locked root {t}");
    }
}
