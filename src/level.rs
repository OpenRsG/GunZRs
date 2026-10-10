//! Retail map rendering shared by the `gunz` map viewer and `gunz-play`: the lightmapped map
//! material and mesh batches per (material, lightmap). Props and sky live in [`crate::props`].

use crate::{
    map,
    mrs::Vfs,
    props,
    view::{self, SCALE, decode, to_bevy},
};
use bevy::{
    asset::{RenderAssetUsages, embedded_asset},
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
}

/// Registers the map material and its embedded shader.
pub struct LevelPlugin;

impl Plugin for LevelPlugin {
    /// Needs [`crate::effect::FxPlugin`] (props use its clock, `txa` and animation systems) and
    /// a [`Level`] resource inserted before the app runs.
    fn build(&self, app: &mut App) {
        app.add_plugins(MaterialPlugin::<MapMaterial>::default());
        embedded_asset!(app, "map.wgsl");
        app.add_systems(Startup, (spawn_level, props::spawn_props))
            .add_systems(Update, props::flutter);
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
