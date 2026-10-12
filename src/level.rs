//! Retail map rendering shared by the `gunz` map viewer and `gunz-play`: the lightmapped map
//! material and mesh batches per (material, lightmap). Props and sky live in [`crate::props`].

use crate::{
    map,
    mrs::Vfs,
    phase::MatchSystems,
    props,
    view::{self, SCALE, decode, to_bevy},
};
use bevy::{
    asset::{RenderAssetUsages, embedded_asset},
    ecs::schedule::InternedScheduleLabel,
    image::{ImageAddressMode, ImageSampler, ImageSamplerDescriptor},
    mesh::{Indices, MeshVertexBufferLayoutRef, PrimitiveTopology},
    pbr::{MaterialPipeline, MaterialPipelineKey},
    prelude::*,
    render::render_resource::{
        AsBindGroup, Extent3d, RenderPipelineDescriptor, SpecializedMeshPipelineError,
        TextureDimension, TextureFormat,
    },
    shader::ShaderRef,
};
use std::collections::HashMap;

/// Retail lightmaps have a median of ~0.13; ×4 (D3D MODULATE4X-style) matches the game's look.
/// Chosen by comparing ×2/×4 renders, not read from the packed executable.
pub(crate) const LIGHTMAP_SCALE: f32 = 4.0;

/// The mounted archives and the loaded map.
#[derive(Resource)]
pub struct Level {
    pub vfs: Vfs,
    pub map: map::Map,
}

impl Level {
    /// `spawn*` dummies as (feet position in Bevy metres, facing direction in Bevy space).
    pub fn spawn_points(&self) -> Vec<(Vec3, Vec3)> {
        let dummy = |d: &map::Dummy| {
            (
                Vec3::from(to_bevy(d.pos)) * SCALE,
                Vec3::from(to_bevy(d.dir)).normalize_or(Vec3::NEG_Z),
            )
        };
        let spawns: Vec<_> = self
            .map
            .dummies
            .iter()
            .filter(|d| d.name.starts_with("spawn") && !d.name.starts_with("spawn_npc"))
            .map(dummy)
            .collect();
        if spawns.is_empty() {
            self.map.dummies.first().map(dummy).into_iter().collect()
        } else {
            spawns
        }
    }

    /// Where a picture of the map is taken from, as (eye, direction) in Bevy space: of every
    /// spawn point at eye height, looking where the map's designers aimed it (and 20 and 40
    /// degrees to either side), the view that stays open longest over the 90 degrees the lens
    /// sees, measured on the drawn polygons (the collision mesh has invisible walls). A ray
    /// that hits nothing sees sky or void, so it counts as a wall 2 m away; hits count up to
    /// 25 m.
    pub fn vantage(&self) -> (Vec3, Vec3) {
        let map = &self.map;
        let tris: Vec<[Vec3; 3]> = map
            .polygons
            .iter()
            .flat_map(|p| {
                let v = &map.vertices[p.first as usize..(p.first + p.count) as usize];
                let at = |i: usize| Vec3::from(to_bevy(v[i].pos)) * SCALE;
                (1..v.len().saturating_sub(1)).map(move |i| [at(0), at(i), at(i + 1)])
            })
            .collect();
        let mut best = (-1.0, (Vec3::Y * 1.6, Vec3::NEG_Z));
        for (feet, dir) in self.spawn_points() {
            let eye = feet + Vec3::Y * 1.6;
            for turn in [0.0f32, -20.0, 20.0, -40.0, 40.0] {
                let yaw = (-dir.x).atan2(-dir.z) + turn.to_radians();
                let mut open = 0.0;
                for side in [-35.0f32, -17.0, 0.0, 17.0, 35.0] {
                    for pitch in [-12.0f32, 0.0, 12.0] {
                        let ray = Quat::from_euler(
                            EulerRot::YXZ,
                            yaw + side.to_radians(),
                            pitch.to_radians(),
                            0.0,
                        ) * Vec3::NEG_Z;
                        open += nearest(&tris, eye, ray).map_or(2.0, |d| d.min(25.0));
                    }
                }
                if open > best.0 {
                    best = (open, (eye, Quat::from_rotation_y(yaw) * Vec3::NEG_Z));
                }
            }
        }
        best.1
    }
}

/// Distance along `dir` to the nearest triangle in `tris`, both sides (Moller-Trumbore).
fn nearest(tris: &[[Vec3; 3]], from: Vec3, dir: Vec3) -> Option<f32> {
    tris.iter()
        .filter_map(|&[a, b, c]| {
            let (e1, e2) = (b - a, c - a);
            let p = dir.cross(e2);
            let det = e1.dot(p);
            if det.abs() < 1e-9 {
                return None;
            }
            let s = from - a;
            let u = s.dot(p) / det;
            let q = s.cross(e1);
            let v = dir.dot(q) / det;
            let t = e2.dot(q) / det;
            (u >= 0.0 && v >= 0.0 && u + v <= 1.0 && t > 0.0).then_some(t)
        })
        .min_by(f32::total_cmp)
}

/// Registers the map material and its embedded shader. The map and its props are spawned by
/// systems in `.0`: `Startup` in the viewer, [`MatchStart`](crate::phase::MatchStart) in a match.
pub struct LevelPlugin(pub InternedScheduleLabel);

impl Plugin for LevelPlugin {
    /// Needs [`crate::effect::FxPlugin`] (props use its clock, `txa` and animation systems) and
    /// a [`Level`] resource inserted before `.0` runs.
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<MapMaterial>::default());
        embedded_asset!(app, "map.wgsl");
        app.add_systems(self.0, (spawn_level, props::spawn_props))
            .add_systems(Update, props::flutter.in_set(MatchSystems));
    }
}

#[derive(Asset, TypePath, AsBindGroup, Clone)]
#[bind_group_data(MapMaterialKey)]
pub struct MapMaterial {
    #[uniform(0)]
    params: Vec4,
    #[texture(1)]
    #[sampler(2)]
    pub(crate) diffuse: Handle<Image>,
    #[texture(3)]
    #[sampler(4)]
    lightmap: Handle<Image>,
    alpha_mode: AlphaMode,
    two_sided: bool,
}

#[repr(C)]
#[derive(Eq, PartialEq, Hash, Copy, Clone)]
pub struct MapMaterialKey {
    two_sided: bool,
}

impl From<&MapMaterial> for MapMaterialKey {
    fn from(m: &MapMaterial) -> Self {
        Self {
            two_sided: m.two_sided,
        }
    }
}

impl Material for MapMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://gunz/map.wgsl".into()
    }
    fn alpha_mode(&self) -> AlphaMode {
        self.alpha_mode
    }
    fn enable_prepass() -> bool {
        false
    }
    fn enable_shadows() -> bool {
        false
    }
    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        key: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        if key.bind_group_data.two_sided {
            descriptor.primitive.cull_mode = None;
        }
        Ok(())
    }
}

/// Startup system: one mesh per (material, lightmap) pair; polygons are triangle fans.
pub fn spawn_level(
    mut commands: Commands,
    level: Res<Level>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<MapMaterial>>,
) {
    let (vfs, map) = (&level.vfs, &level.map);
    // Prefer map textures when a name exists in several archives.
    let by_name = view::file_index(vfs, "maps/");

    // A map without `.RS.lm` is drawn with a single white lightmap texel and scale 1: diffuse only.
    let lightmap_scale = if map.lightmaps.is_empty() {
        1.0
    } else {
        LIGHTMAP_SCALE
    };
    let lightmaps: Vec<Handle<Image>> = if map.lightmaps.is_empty() {
        let white = Image::new_fill(
            Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[255; 4],
            TextureFormat::Rgba8Unorm,
            RenderAssetUsages::RENDER_WORLD,
        );
        vec![images.add(white)]
    } else {
        map.lightmaps
            .iter()
            .map(|bmp| {
                decode(bmp, "bmp", false, ImageSampler::linear())
                    .map_or_else(Handle::default, |i| images.add(i))
            })
            .collect()
    };
    let repeat = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        ..ImageSamplerDescriptor::linear()
    });
    // Only materials some polygon uses are loaded: retail XMLs list unused ones with dangling
    // textures (Ruin `gzd_map_Ruin_moon.dds`, Snow Town `hide` -> `../town/`).
    let mut used = vec![false; map.materials.len()];
    for p in &map.polygons {
        used[p.material as usize] = true;
    }
    let diffuse: Vec<Handle<Image>> = map
        .materials
        .iter()
        .zip(&used)
        .map(|(m, &used)| {
            let Some(name) = m.diffuse_map.as_ref().filter(|_| used) else {
                return Handle::default();
            };
            let Some(path) = view::texture_path(vfs, &by_name, &map.dir, name) else {
                warn!("{}: texture {name} not found", m.name);
                return Handle::default();
            };
            let ext = path.rsplit('.').next().unwrap();
            vfs.read(&path)
                .ok()
                .and_then(|b| decode(&b, ext, false, repeat.clone()))
                .map_or_else(Handle::default, |i| images.add(i))
        })
        .collect();

    #[derive(Default)]
    struct Batch {
        pos: Vec<[f32; 3]>,
        normal: Vec<[f32; 3]>,
        uv: Vec<[f32; 2]>,
        lm: Vec<[f32; 2]>,
        idx: Vec<u32>,
    }
    let mut batches: HashMap<(u32, u32), Batch> = HashMap::new();
    for p in &map.polygons {
        let b = batches.entry((p.material, p.lightmap)).or_default();
        let base = b.pos.len() as u32;
        for v in &map.vertices[p.first as usize..(p.first + p.count) as usize] {
            b.pos.push(to_bevy(v.pos.map(|c| c * SCALE)));
            b.normal.push(to_bevy(v.normal));
            b.uv.push(v.uv);
            b.lm.push(v.lm_uv);
        }
        for i in 1..p.count - 1 {
            b.idx.extend([base, base + i, base + i + 1]);
        }
    }
    for ((material, lightmap), b) in batches {
        let m = &map.materials[material as usize];
        let mesh = Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::RENDER_WORLD,
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, b.pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, b.normal)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, b.uv)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, b.lm)
        .with_inserted_indices(Indices::U32(b.idx));
        let alpha_mode = if m.additive {
            AlphaMode::Add
        } else if m.opacity {
            AlphaMode::Blend
        } else {
            AlphaMode::Opaque
        };
        commands.spawn((
            crate::game::MapEntity,
            Mesh3d(meshes.add(mesh)),
            MeshMaterial3d(materials.add(MapMaterial {
                params: Vec4::new(
                    if m.alpha_test { 0.5 } else { 0.0 },
                    lightmap_scale,
                    if m.additive { 1.0 } else { 0.0 },
                    0.0,
                ),
                diffuse: diffuse[material as usize].clone(),
                lightmap: lightmaps[lightmap as usize].clone(),
                alpha_mode,
                two_sided: m.two_sided,
            })),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_finds_the_nearest_triangle_from_either_side() {
        let tri = |z| {
            [
                Vec3::new(-1.0, -1.0, z),
                Vec3::new(1.0, -1.0, z),
                Vec3::new(0.0, 1.0, z),
            ]
        };
        let tris = [tri(-5.0), tri(-3.0)];
        assert_eq!(nearest(&tris, Vec3::ZERO, Vec3::NEG_Z), Some(3.0));
        assert_eq!(nearest(&tris, Vec3::ZERO, Vec3::Z), None);
        assert_eq!(
            nearest(&tris, Vec3::new(0.0, 0.0, -4.0), Vec3::Z),
            Some(1.0)
        );
    }
}
