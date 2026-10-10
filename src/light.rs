//! Lighting of the GRAPHICS panel (`gfx::Knob::{ActorLight, DynLight, Shadow}`, all 0 in ORIGINAL):
//! actors lit from the map (their materials switch from unlit to lit, each actor carries a fill
//! light coloured by the lightmap around it, a rim light stands behind them), muzzle flashes,
//! explosions and sparks as short point lights that `map.wgsl` and the lit actors both read, and
//! a soft contact shadow under every actor. Values and limits: `docs/formats.md` "Graphics".

use crate::{
    col::MapCollision,
    combat::Vfx,
    game::{Blast, Dead, Motor, Settings},
    gfx::Knob,
    level::{LIGHTMAP_SCALE, Level},
    profile::Profile,
    view::{SCALE, decode, to_bevy},
};
use bevy::{
    asset::RenderAssetUsages,
    camera::visibility::InheritedVisibility,
    image::ImageSampler,
    light::{AmbientLight, DirectionalLight, PointLight},
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::collections::HashMap;

pub struct LightPlugin;

impl Plugin for LightPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, (probes, blob_assets)).add_systems(
            Update,
            (rig, actor_materials, fills, flashes, blobs)
                .chain()
                .run_if(resource_exists::<Settings>),
        );
    }
}

fn knob(profile: &Option<Res<Profile>>, k: Knob) -> f32 {
    profile
        .as_ref()
        .map_or(crate::gfx::Graphics::default().get(k), |p| {
            p.graphics.get(k)
        })
}

// ---- the lightmap as light probes ----

const CELL: f32 = 3.0;
/// Radius, in metres, of the lightmap polygons that light an actor.
const REACH: f32 = 4.0;

/// Average lightmap colour of every polygon of the map (linear, as drawn with white texture),
/// in a grid of [`CELL`] metre cells.
#[derive(Resource)]
struct Probes {
    cells: HashMap<IVec3, Vec<(Vec3, Vec3)>>,
    mean: Vec3,
}

fn cell(p: Vec3) -> IVec3 {
    (p / CELL).floor().as_ivec3()
}

impl Probes {
    /// The light around `p`: polygons within [`REACH`] weighted by closeness, the map's mean
    /// where there are none (and a dim floor, so nobody is pitch black).
    fn at(&self, p: Vec3) -> Vec3 {
        let (mut sum, mut weight) = (Vec3::ZERO, 0.0);
        let c = cell(p);
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for &(at, color) in self
                        .cells
                        .get(&(c + IVec3::new(dx, dy, dz)))
                        .into_iter()
                        .flatten()
                    {
                        let w = (1.0 - at.distance(p) / REACH).max(0.0);
                        sum += color * w * w;
                        weight += w * w;
                    }
                }
            }
        }
        let c = if weight > 0.05 {
            sum / weight
        } else {
            self.mean
        };
        // lightmaps are meant to be multiplied by 4 and read in gamma; actors are not
        c.max(Vec3::splat(0.04)).powf(0.6)
    }
}

fn probes(mut commands: Commands, level: Res<Level>) {
    let map = &level.map;
    // texel lookup: raw lightmap values as the shader sees them (gamma-space numbers)
    let maps: Vec<Option<Image>> = map
        .lightmaps
        .iter()
        .map(|b| decode(b, "bmp", false, ImageSampler::linear()))
        .collect();
    let texel = |lm: u32, uv: Vec2| -> Vec3 {
        let Some(Some(img)) = maps.get(lm as usize) else {
            return Vec3::ONE;
        };
        let x = ((uv.x * img.width() as f32) as u32).min(img.width() - 1);
        let y = ((uv.y * img.height() as f32) as u32).min(img.height() - 1);
        let Ok(c) = img.get_color_at(x, y) else {
            return Vec3::ONE;
        };
        let c = c.to_linear();
        // `map.wgsl`: lightmap x scale, clamped, then gamma to linear
        Vec3::new(c.red, c.green, c.blue).map(|v| (v * LIGHTMAP_SCALE).min(1.0).powf(2.2))
    };
    let mut cells: HashMap<IVec3, Vec<(Vec3, Vec3)>> = HashMap::new();
    let (mut total, mut n) = (Vec3::ZERO, 0.0);
    for p in &map.polygons {
        if map.lightmaps.is_empty() || p.count < 3 {
            continue;
        }
        let vs = &map.vertices[p.first as usize..(p.first + p.count) as usize];
        let at = vs
            .iter()
            .map(|v| Vec3::from(to_bevy(v.pos.map(|c| c * SCALE))))
            .sum::<Vec3>()
            / vs.len() as f32;
        let centre = vs.iter().map(|v| Vec2::from(v.lm_uv)).sum::<Vec2>() / vs.len() as f32;
        let color = (texel(p.lightmap, centre)
            + vs.iter()
                .take(3)
                .map(|v| texel(p.lightmap, Vec2::from(v.lm_uv)))
                .sum::<Vec3>())
            / (vs.len().min(3) as f32 + 1.0);
        cells.entry(cell(at)).or_default().push((at, color));
        total += color;
        n += 1.0;
    }
    let mean = if n > 0.0 { total / n } else { Vec3::splat(0.5) };
    commands.insert_resource(Probes { cells, mean });
}

// ---- actors lit from the map ----

#[derive(Component)]
struct Rim;

/// The fill light of an actor: colour eased towards what [`Probes`] says.
#[derive(Component)]
struct Fill {
    actor: Entity,
    color: Vec3,
}

/// The camera's ambient and the rim light behind the actors; both only while actors are lit.
fn rig(
    profile: Option<Res<Profile>>,
    camera: Query<(Entity, &Transform, Has<AmbientLight>), With<Camera3d>>,
    mut rim: Query<(Entity, &mut Transform), (With<Rim>, Without<Camera3d>)>,
    mut commands: Commands,
) {
    let Ok((cam, tf, has_ambient)) = camera.single() else {
        return;
    };
    let k = knob(&profile, Knob::ActorLight);
    if k == 0.0 {
        if has_ambient {
            commands.entity(cam).remove::<AmbientLight>();
        }
        for (e, _) in &rim {
            commands.entity(e).despawn();
        }
        return;
    }
    if !has_ambient {
        // scene-wide floor under the fills; units are cd/m2 (bevy's default exposure)
        commands.entity(cam).insert(AmbientLight {
            color: Color::WHITE,
            brightness: 220.0,
            affects_lightmapped_meshes: true,
        });
    }
    // light that travels towards the camera from above and a little to the side: it only catches
    // the edges that turn away from the viewer
    let dir = (-tf.forward().as_vec3() * 0.75 + Vec3::NEG_Y * 0.5 + tf.right().as_vec3() * 0.3)
        .normalize();
    let looking = Transform::IDENTITY.looking_to(dir, Vec3::Y);
    match rim.single_mut() {
        Ok((_, mut t)) => t.rotation = looking.rotation,
        Err(_) => {
            commands.spawn((
                Rim,
                DirectionalLight {
                    color: Color::srgb(0.75, 0.85, 1.0),
                    illuminance: 3500.0 * k,
                    shadow_maps_enabled: false,
                    ..default()
                },
                looking,
            ));
        }
    }
}

/// Unlit -> lit for the materials of everything under an actor (models and their weapons), and
/// back; additive ones (glows) stay as they are. Materials are shared, so this is per handle.
fn actor_materials(
    profile: Option<Res<Profile>>,
    meshes: Query<(Entity, Ref<MeshMaterial3d<StandardMaterial>>)>,
    parents: Query<&ChildOf>,
    actors: Query<(), With<Motor>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut was: Local<bool>,
) {
    let on = knob(&profile, Knob::ActorLight) > 0.0;
    let toggled = on != *was;
    *was = on;
    if !toggled && !on {
        return;
    }
    for (e, m) in &meshes {
        if !(toggled || m.is_added() || m.is_changed()) {
            continue;
        }
        let mut up = e;
        let mut under = false;
        for _ in 0..8 {
            if actors.contains(up) {
                under = true;
                break;
            }
            match parents.get(up) {
                Ok(p) => up = p.parent(),
                Err(_) => break,
            }
        }
        if !under {
            continue;
        }
        let Some(mat) = materials.get(&m.0) else {
            continue;
        };
        if matches!(mat.alpha_mode, AlphaMode::Add | AlphaMode::Premultiplied) || mat.unlit == !on {
            continue;
        }
        if let Some(mut mat) = materials.get_mut(&m.0) {
            mat.unlit = !on;
            if on {
                mat.perceptual_roughness = 0.85;
                mat.reflectance = 0.12;
                mat.metallic = 0.0;
            }
        }
    }
}

/// One point light per actor, a metre and a half towards the camera and above the chest,
/// in the colour of the lightmap around the actor.
fn fills(
    profile: Option<Res<Profile>>,
    probes: Option<Res<Probes>>,
    time: Res<Time>,
    camera: Query<&Transform, With<Camera3d>>,
    actors: Query<(Entity, &GlobalTransform), With<Motor>>,
    mut lights: Query<(Entity, &mut Transform, &mut PointLight, &mut Fill), Without<Camera3d>>,
    mut commands: Commands,
) {
    let k = knob(&profile, Knob::ActorLight);
    let (Some(probes), Ok(cam), true) = (probes, camera.single(), k > 0.0) else {
        for (e, ..) in &lights {
            commands.entity(e).despawn();
        }
        return;
    };
    let mut have = Vec::new();
    for (e, mut tf, mut light, mut fill) in &mut lights {
        let Ok((_, g)) = actors.get(fill.actor) else {
            commands.entity(e).despawn();
            continue;
        };
        have.push(fill.actor);
        let feet = g.translation();
        let towards = (cam.translation - feet).with_y(0.0).normalize_or_zero();
        tf.translation = feet + towards * 1.2 + Vec3::Y * 1.9;
        let want = probes.at(feet + Vec3::Y);
        let a = 1.0 - (-4.0 * time.delta_secs()).exp();
        fill.color = fill.color.lerp(want, a);
        light.color = Color::linear_rgb(fill.color.x, fill.color.y, fill.color.z);
        light.intensity = 150_000.0 * k;
    }
    for (a, g) in &actors {
        if have.contains(&a) {
            continue;
        }
        let color = probes.at(g.translation() + Vec3::Y);
        commands.spawn((
            Fill { actor: a, color },
            PointLight {
                color: Color::linear_rgb(color.x, color.y, color.z),
                intensity: 150_000.0 * k,
                range: 4.5,
                shadow_maps_enabled: false,
                // the map's own lightmaps are the room's light; this is for the actors
                affects_lightmapped_mesh_diffuse: false,
                ..default()
            },
            Transform::from_translation(g.translation() + Vec3::Y * 1.9),
        ));
    }
}

// ---- flashes ----

/// A short point light: `peak` lumens falling off over `life` seconds.
#[derive(Component)]
struct Flash {
    t: f32,
    life: f32,
    peak: f32,
}

const MAX_FLASHES: usize = 14;

fn flashes(
    profile: Option<Res<Profile>>,
    time: Res<Time>,
    mut vfx: MessageReader<Vfx>,
    mut blasts: MessageReader<Blast>,
    transforms: Query<&GlobalTransform>,
    mut lights: Query<(Entity, &mut PointLight, &mut Flash)>,
    mut commands: Commands,
) {
    let k = knob(&profile, Knob::DynLight);
    let mut live = 0;
    for (e, mut light, mut f) in &mut lights {
        f.t += time.delta_secs();
        if k == 0.0 || f.t >= f.life {
            commands.entity(e).despawn();
            continue;
        }
        live += 1;
        light.intensity = f.peak * k * (1.0 - f.t / f.life).powi(2);
    }
    if k == 0.0 {
        return;
    }
    let mut spawn = |at: Vec3, color: Color, peak: f32, range: f32, life: f32| {
        if live >= MAX_FLASHES {
            return;
        }
        live += 1;
        commands.spawn((
            Flash { t: 0.0, life, peak },
            PointLight {
                color,
                intensity: peak * k,
                range,
                shadow_maps_enabled: false,
                ..default()
            },
            Transform::from_translation(at),
        ));
    };
    let fire = Color::srgb(1.0, 0.72, 0.38);
    for v in vfx.read() {
        match *v {
            Vfx::Muzzle {
                shooter,
                dir,
                fallback,
                ..
            } => {
                let at = transforms.get(shooter).map_or(fallback, |g| {
                    g.translation() + Vec3::Y * 1.3 + dir.normalize_or_zero() * 0.7
                });
                spawn(at, fire, 300_000.0, 8.0, 0.08);
            }
            Vfx::Spark { point, normal } => {
                spawn(
                    point + normal * 0.15,
                    Color::srgb(1.0, 0.85, 0.6),
                    60_000.0,
                    3.0,
                    0.07,
                );
            }
            Vfx::Elu { name, at } if name.contains("flash") => {
                spawn(
                    at.translation,
                    Color::srgb(0.8, 0.9, 1.0),
                    120_000.0,
                    4.0,
                    0.08,
                );
            }
            _ => {}
        }
    }
    for b in blasts.read() {
        spawn(
            b.at + Vec3::Y * 0.4,
            Color::srgb(1.0, 0.55, 0.2),
            4_000_000.0,
            16.0,
            0.6,
        );
    }
}

// ---- contact shadows ----

#[derive(Resource)]
struct BlobAssets {
    mesh: Handle<Mesh>,
    /// Shadow darkness steps, faintest first.
    steps: [Handle<StandardMaterial>; 5],
}

#[derive(Component)]
struct Blob(Entity);

fn blob_assets(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    const N: u32 = 32;
    let mut px = Vec::with_capacity((N * N * 4) as usize);
    for y in 0..N {
        for x in 0..N {
            let d = Vec2::new(x as f32 + 0.5, y as f32 + 0.5).distance(Vec2::splat(N as f32 / 2.0))
                / (N as f32 / 2.0);
            let a = (1.0 - d).clamp(0.0, 1.0);
            px.extend([255, 255, 255, (a * a * (3.0 - 2.0 * a) * 255.0) as u8]);
        }
    }
    let tex = images.add(Image::new(
        Extent3d {
            width: N,
            height: N,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        px,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ));
    let steps = std::array::from_fn(|i| {
        materials.add(StandardMaterial {
            base_color: Color::srgba(0.0, 0.0, 0.0, (i + 1) as f32 / 5.0 * 0.9),
            base_color_texture: Some(tex.clone()),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            fog_enabled: false,
            depth_bias: 1.0,
            ..default()
        })
    });
    commands.insert_resource(BlobAssets {
        mesh: meshes.add(Rectangle::new(1.0, 1.0)),
        steps,
    });
}

/// A dark soft disc on the floor under each actor, smaller and fainter the higher it is.
fn blobs(
    profile: Option<Res<Profile>>,
    assets: Option<Res<BlobAssets>>,
    col: Option<Res<MapCollision>>,
    actors: Query<(Entity, &GlobalTransform, &InheritedVisibility, Has<Dead>), With<Motor>>,
    mut shadows: Query<(
        Entity,
        &Blob,
        &mut Transform,
        &mut MeshMaterial3d<StandardMaterial>,
        &mut Visibility,
    )>,
    mut commands: Commands,
) {
    let k = knob(&profile, Knob::Shadow);
    let (Some(assets), Some(col), true) = (assets, col, k > 0.0) else {
        for (e, ..) in &shadows {
            commands.entity(e).despawn();
        }
        return;
    };
    let mut have = Vec::new();
    for (e, blob, mut tf, mut material, mut vis) in &mut shadows {
        let Ok((_, g, shown, dead)) = actors.get(blob.0) else {
            commands.entity(e).despawn();
            continue;
        };
        have.push(blob.0);
        let feet = g.translation();
        let hit = col.raycast(feet + Vec3::Y * 0.5, Vec3::NEG_Y, 4.0);
        let (Some(h), true) = (hit, shown.get()) else {
            *vis = Visibility::Hidden;
            continue;
        };
        let height = (feet.y - h.point.y).max(0.0);
        // faint at 2.5 m up, none beyond; a corpse's shadow is wider and weaker
        let fade = (1.0 - height / 2.5).clamp(0.0, 1.0) * if dead { 0.6 } else { 1.0 };
        let step = ((fade * k * 5.0).ceil() as usize).min(5);
        if step == 0 {
            *vis = Visibility::Hidden;
            continue;
        }
        *vis = Visibility::Inherited;
        let want = &assets.steps[step - 1];
        if material.0 != *want {
            material.0 = want.clone();
        }
        tf.translation = h.point + h.normal * 0.02;
        tf.rotation = Quat::from_rotation_arc(Vec3::Z, h.normal);
        tf.scale = Vec3::splat((if dead { 1.6 } else { 1.15 }) + height * 0.25);
    }
    for (a, ..) in &actors {
        if !have.contains(&a) {
            commands.spawn((
                Blob(a),
                Mesh3d(assets.mesh.clone()),
                MeshMaterial3d(assets.steps[0].clone()),
                Transform::default(),
                Visibility::Hidden,
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_actor_takes_the_colour_of_the_lightmap_beside_it() {
        let (red, blue) = (Vec3::new(1.0, 1.0, 1.0), Vec3::new(20.0, 1.0, 1.0));
        let mut cells: HashMap<IVec3, Vec<(Vec3, Vec3)>> = HashMap::new();
        cells.entry(cell(red)).or_default().push((red, Vec3::X));
        cells.entry(cell(blue)).or_default().push((blue, Vec3::Z));
        let probes = Probes {
            cells,
            mean: Vec3::splat(0.5),
        };
        let near_red = probes.at(red + Vec3::X * 0.5);
        assert!(near_red.x > 0.9 && near_red.z < 0.3, "{near_red}");
        let near_blue = probes.at(blue);
        assert!(near_blue.z > 0.9 && near_blue.x < 0.3, "{near_blue}");
        // nothing in reach: the map's mean, never black
        assert_eq!(probes.at(Vec3::splat(100.0)), Vec3::splat(0.5f32.powf(0.6)));
    }
}
