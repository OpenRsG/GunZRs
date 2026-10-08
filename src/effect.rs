//! Effects: `txa` flip-book textures, the `sfx/effect_list.xml` registry, and Bevy spawning of
//! effect/prop ELUs with animation, flip-books and fades. Layouts: `docs/formats.md`.

use crate::{
    ani::Ani,
    anim::{AnimPlugin, Animator, Loop, NodeAlpha},
    elu::Elu,
    model::{self, Model, Textures},
    mrs::Vfs,
    view::{self, SCALE},
};
use bevy::{
    ecs::system::SystemParam,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    mesh::skinning::SkinnedMeshInverseBindposes,
    prelude::*,
};
use std::{collections::HashMap, sync::Arc};

/// `txa <frames> <ms> <first_frame_file>` texture name: a looping flip-book.
/// Frame 0 is the file literally named `txa N MS stemDD.ext`; frame `i` is `stem` + (`DD`+`i`
/// zero-padded to the same width) + `.ext` in the same directory.
#[derive(Clone, Debug, PartialEq)]
pub struct TexAnim {
    pub frames: u32,
    pub period: f32,
    stem: String,
    first: u32,
    width: usize,
    ext: String,
}

impl TexAnim {
    /// Parses the last path component of a texture reference; `None` if it is not a `txa` name.
    pub fn parse(name: &str) -> Option<Self> {
        let file = name.rsplit(['/', '\\']).next()?;
        let mut it = file.splitn(4, ' ');
        if !it.next()?.eq_ignore_ascii_case("txa") {
            return None;
        }
        let frames: u32 = it.next()?.parse().ok().filter(|&n| n > 0)?;
        let ms: u32 = it.next()?.parse().ok().filter(|&n| n > 0)?;
        let (stem, ext) = it.next()?.rsplit_once('.')?;
        let width = stem.bytes().rev().take_while(u8::is_ascii_digit).count();
        let (stem, digits) = stem.split_at(stem.len() - width);
        Some(Self {
            frames,
            period: ms as f32 / 1000.0,
            stem: stem.into(),
            first: digits.parse().ok()?,
            width,
            ext: ext.into(),
        })
    }

    /// File name of frame `i` (`i >= 1`; frame 0 is the `txa ...` file itself).
    pub fn frame_name(&self, i: u32) -> String {
        format!(
            "{}{:0w$}.{}",
            self.stem,
            self.first + i,
            self.ext,
            w = self.width
        )
    }

    /// Frame shown `t` seconds after the start.
    pub fn frame_at(&self, t: f32) -> usize {
        ((t / self.period * self.frames as f32).floor() as i64).rem_euclid(self.frames as i64)
            as usize
    }
}

/// Decoded frames of a [`TexAnim`], to be swapped into a material by the viewer.
#[derive(Component, Clone)]
pub struct TexFrames {
    pub anim: TexAnim,
    pub frames: Vec<Handle<Image>>,
}

impl TexFrames {
    pub fn current(&self, t: f32) -> &Handle<Image> {
        &self.frames[self.anim.frame_at(t)]
    }
}

/// Loads every frame of the animation whose first frame resolved to `first_path`.
/// Missing frames are an error (all retail `txa` sets are complete).
pub fn load_frames(
    vfs: &Vfs,
    by_name: &HashMap<String, String>,
    first_path: &str,
    anim: TexAnim,
    images: &mut Assets<Image>,
    srgb: bool,
    sampler: &ImageSampler,
) -> Result<TexFrames, String> {
    let dir = first_path
        .rsplit_once('/')
        .map_or(String::new(), |(d, _)| format!("{d}/"));
    let mut frames = Vec::new();
    for i in 0..anim.frames {
        let path = if i == 0 {
            first_path.to_string()
        } else {
            let name = anim.frame_name(i);
            view::texture_path(vfs, by_name, &dir, &name)
                .ok_or_else(|| format!("txa frame {name} not found"))?
        };
        let ext = path.rsplit('.').next().unwrap();
        let bytes = vfs.read(&path).map_err(|e| format!("{path}: {e}"))?;
        let image = view::decode(&bytes, ext, srgb, sampler.clone())
            .ok_or_else(|| format!("{path}: undecodable"))?;
        frames.push(images.add(image));
    }
    Ok(TexFrames { anim, frames })
}

/// Animation time source: wall time, or a fixed time for reproducible `--shot` renders.
#[derive(Resource)]
pub struct Clock(pub Option<f32>);

impl Clock {
    pub fn now(&self, time: &Time) -> f32 {
        self.0.unwrap_or_else(|| time.elapsed_secs())
    }
}

/// Removes `FLAG VALUE` (a number) from `args`. `Err` means the flag had no valid value.
pub fn take_f32_arg(args: &mut Vec<String>, flag: &str) -> Result<Option<f32>, ()> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    let value = args.get(i + 1).and_then(|v| v.parse().ok()).ok_or(())?;
    args.drain(i..=i + 1);
    Ok(Some(value))
}

/// Removes `FLAG X,Y,Z` from `args`. `Err` means the flag had no valid value.
pub fn take_vec3_arg(args: &mut Vec<String>, flag: &str) -> Result<Option<[f32; 3]>, ()> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    let mut it = args
        .get(i + 1)
        .ok_or(())?
        .split(',')
        .map(|v| v.parse::<f32>());
    let v = [it.next(), it.next(), it.next()];
    let [Some(Ok(x)), Some(Ok(y)), Some(Ok(z))] = v else {
        return Err(());
    };
    if it.next().is_some() {
        return Err(());
    }
    args.drain(i..=i + 1);
    Ok(Some([x, y, z]))
}

/// One `<AddEffectElu>` of `sfx/effect_list.xml`: a model plus its `play` animation.
#[derive(Clone, Debug)]
pub struct EffectDef {
    pub name: String,
    /// VFS path of the `.elu`.
    pub model: String,
    /// VFS path of the `.elu.ani` and whether it loops (`loop`; otherwise `lastframe` holds).
    pub animation: Option<(String, bool)>,
    /// `<AddParticle name dummy_name>`: particle emitter named `name` at ELU node `dummy_name`.
    pub particle: Option<(String, String)>,
}

/// Reads `sfx/effect_list.xml`. Names are not unique (`ef_sworddam_ice` appears twice);
/// use [`find`] for first-match lookup.
pub fn load_list(vfs: &Vfs) -> std::io::Result<Vec<EffectDef>> {
    let bad = |m: String| std::io::Error::new(std::io::ErrorKind::InvalidData, m);
    let text = String::from_utf8(vfs.read("sfx/effect_list.xml")?)
        .map_err(|e| bad(format!("effect_list.xml: {e}")))?;
    let doc =
        roxmltree::Document::parse(&text).map_err(|e| bad(format!("effect_list.xml: {e}")))?;
    let path = |f: &str| {
        let p = format!("sfx/{}", crate::mrs::normalize(f));
        vfs.exists(&p)
            .then_some(p)
            .ok_or_else(|| bad(format!("effect_list.xml: {f} missing")))
    };
    let mut defs = Vec::new();
    for e in doc.descendants().filter(|n| n.has_tag_name("AddEffectElu")) {
        let attr = |n: roxmltree::Node<'_, '_>, k: &str| {
            n.attribute(k).map(str::to_string).ok_or_else(|| {
                bad(format!(
                    "effect_list.xml: <{}> without {k}",
                    n.tag_name().name()
                ))
            })
        };
        let child = |tag: &str| e.children().find(|c| c.has_tag_name(tag));
        let name = attr(e, "name")?;
        let model = child("AddBaseModel").ok_or_else(|| bad(format!("{name}: no AddBaseModel")))?;
        let animation = match child("AddAnimation") {
            Some(a) => Some((
                path(&attr(a, "filename")?)?,
                a.attribute("motion_loop_type") == Some("loop"),
            )),
            None => None,
        };
        let particle = match child("AddParticle") {
            Some(p) => Some((attr(p, "name")?, attr(p, "dummy_name")?)),
            None => None,
        };
        defs.push(EffectDef {
            name,
            model: path(&attr(model, "filename")?)?,
            animation,
            particle,
        });
    }
    Ok(defs)
}

/// First definition named `name` (case-insensitive, as zactoraction.xml/zskill.xml spell them).
pub fn find<'a>(defs: &'a [EffectDef], name: &str) -> Option<&'a EffectDef> {
    defs.iter().find(|d| d.name.eq_ignore_ascii_case(name))
}

/// Clock, `.elu.ani` playback, `txa` frame swapping and per-node fades for spawned ELUs.
/// The field fixes the animation time (for reproducible `--shot` renders); `None` = wall time.
pub struct FxPlugin(pub Option<f32>);

impl Plugin for FxPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Clock(self.0))
            .init_resource::<TxaRegistry>()
            .add_plugins(AnimPlugin)
            .add_systems(Update, (tag_txa, flip_txa, fade_nodes).chain());
    }
}

/// `txa` flip-books by the material they replace.
#[derive(Resource, Default)]
struct TxaRegistry(HashMap<AssetId<StandardMaterial>, TexFrames>);

#[derive(SystemParam)]
pub struct FxAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    bindposes: ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
    images: ResMut<'w, Assets<Image>>,
    materials: ResMut<'w, Assets<StandardMaterial>>,
    txa: ResMut<'w, TxaRegistry>,
    clock: Res<'w, Clock>,
}

/// Spawns ELU effects (and map props) as unlit models with their animation attached.
pub struct Loader<'a> {
    vfs: &'a Vfs,
    textures: Textures<'a>,
    by_name: HashMap<String, String>,
    repeat: ImageSampler,
}

impl<'a> Loader<'a> {
    /// `prefer`: VFS prefix whose textures win when a name exists in several archives.
    pub fn new(vfs: &'a Vfs, prefer: &str) -> Self {
        Self {
            vfs,
            textures: Textures::new(vfs, prefer),
            by_name: view::file_index(vfs, prefer),
            repeat: ImageSampler::Descriptor(ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..ImageSamplerDescriptor::linear()
            }),
        }
    }

    /// Spawns `elu` (textures resolved against VFS directory `dir`) and starts `ani`.
    pub fn spawn(
        &mut self,
        fx: &mut FxAssets,
        commands: &mut Commands,
        dir: &str,
        elu: &Arc<Elu>,
        ani: Option<(Arc<Ani>, Loop)>,
        placement: Transform,
    ) -> Model {
        let (vfs, by_name, repeat) = (self.vfs, &self.by_name, &self.repeat);
        let textures = &mut self.textures;
        let model = model::spawn_elu(
            commands,
            &mut fx.meshes,
            &mut fx.bindposes,
            elu,
            &mut |m| {
                let mut m = m.clone();
                // Version-0 ELUs store no material flags. 177 of the 180 `.bmp` materials of
                // version >= 0x5004 sfx are additive (exceptions: empty_cartridge01, ef_methor),
                // so version-0 `.bmp` materials are drawn additive and two-sided (inferred).
                if elu.version == 0 && m.texture.to_ascii_lowercase().ends_with(".bmp") {
                    (m.additive, m.two_sided) = (true, true);
                }
                let handle = textures.standard(&mut fx.images, &mut fx.materials, dir, &m);
                // Textures with an alpha channel (`.tga`) blend unless alpha-tested.
                if !m.additive && m.alpha_ref == 0 && !m.alpha_texture.is_empty() {
                    if let Some(mut std) = fx.materials.get_mut(&handle) {
                        std.alpha_mode = AlphaMode::Blend;
                    }
                }
                if let Some(anim) = TexAnim::parse(&m.texture) {
                    let frames = view::texture_path(vfs, by_name, dir, &m.texture)
                        .ok_or_else(|| format!("{} not found", m.texture))
                        .and_then(|p| {
                            load_frames(vfs, by_name, &p, anim, &mut fx.images, true, repeat)
                        });
                    match frames {
                        Ok(f) => drop(fx.txa.0.insert(handle.id(), f)),
                        Err(e) => warn!("{dir}: {e}"),
                    }
                }
                handle
            },
            placement,
            |_| true,
        );
        if let Some((ani, looping)) = ani {
            let mut animator = Animator::new(ani, looping);
            animator.elu = Some(elu.clone());
            if let Some(t) = fx.clock.0 {
                animator.time = t;
                animator.speed = 0.0;
            }
            commands.entity(model.root).insert(animator);
        }
        model
    }
}

/// Bounding sphere (centre, radius) in metres of the bind pose, in the model root's frame.
pub fn bounds(elu: &Elu) -> (Vec3, f32) {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for n in elu.nodes.iter().filter(|n| !model::is_bone(&n.name)) {
        let m = model::bevy_matrix(&n.world);
        for p in &n.positions {
            let p = m.transform_point3(Vec3::new(p[0], p[1], -p[2]));
            (lo, hi) = (lo.min(p), hi.max(p));
        }
    }
    if lo.x > hi.x {
        return (Vec3::ZERO, 1.0);
    }
    (
        (lo + hi) * 0.5 * SCALE,
        ((hi - lo) * 0.5 * SCALE).length().max(0.05),
    )
}

/// Gives every mesh whose material is a registered `txa` material its frames.
fn tag_txa(
    mut commands: Commands,
    registry: Res<TxaRegistry>,
    added: Query<
        (Entity, &MeshMaterial3d<StandardMaterial>),
        Added<MeshMaterial3d<StandardMaterial>>,
    >,
) {
    for (e, m) in &added {
        if let Some(frames) = registry.0.get(&m.id()) {
            commands.entity(e).insert(frames.clone());
        }
    }
}

fn flip_txa(
    clock: Res<Clock>,
    time: Res<Time>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    animated: Query<(&MeshMaterial3d<StandardMaterial>, &TexFrames)>,
) {
    let t = clock.now(&time);
    for (material, frames) in &animated {
        let frame = frames.current(t);
        if materials
            .get(material)
            .is_some_and(|m| m.base_color_texture.as_ref() != Some(frame))
        {
            materials.get_mut(material).unwrap().base_color_texture = Some(frame.clone());
        }
    }
}

/// Marks a mesh entity that owns a private copy of its material (needed for per-node fades).
#[derive(Component)]
struct OwnMaterial;

/// Applies the animation's per-node visibility (alpha) to the node's meshes: unlit alpha
/// multiplies the colour for additive materials and the opacity for blended ones.
fn fade_nodes(
    mut commands: Commands,
    nodes: Query<(&NodeAlpha, &Children), Changed<NodeAlpha>>,
    mut meshes: Query<(&mut MeshMaterial3d<StandardMaterial>, Has<OwnMaterial>)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (alpha, children) in &nodes {
        for &child in children {
            let Ok((mut handle, own)) = meshes.get_mut(child) else {
                continue;
            };
            if !own {
                let Some(copy) = materials.get(&handle.0).cloned() else {
                    continue;
                };
                handle.0 = materials.add(copy);
                commands.entity(child).insert(OwnMaterial);
            }
            if let Some(mut m) = materials.get_mut(&handle.0) {
                m.base_color = Color::srgba(1.0, 1.0, 1.0, alpha.0);
                if alpha.0 < 1.0 && m.alpha_mode == AlphaMode::Opaque {
                    m.alpha_mode = AlphaMode::Blend;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn txa_names() {
        let a = TexAnim::parse("../Mansion/txa 30 2000 fire_a00.dds").unwrap();
        assert_eq!((a.frames, a.period), (30, 2.0));
        assert_eq!(a.frame_name(7), "fire_a07.dds");
        assert_eq!(a.frame_at(0.0), 0);
        assert_eq!(a.frame_at(1.99), 29);
        assert_eq!(a.frame_at(2.0), 0);
        let b = TexAnim::parse("txa 91 10000 GZ_League_CountBar_00000.png").unwrap();
        assert_eq!(b.frame_name(12), "GZ_League_CountBar_00012.png");
        assert!(TexAnim::parse("fire_a00.dds").is_none());
    }
}
