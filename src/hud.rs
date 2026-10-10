//! `gunz-play` HUD (Bevy UI over the 3D view) and visual feedback: bullet-hole/blood decals,
//! damage-direction arcs, low-health vignette, explosion screen shake. HUD textures come from
//! `interface/default/*.png`, decals from `sfx/`. Sound lives in `audio.rs`. Format notes:
//! `docs/formats.md` (HUD and sound).

use crate::{
    actor::ActorData,
    col::MapCollision,
    game::{Blast, CameraShake, Damage, Impact, Killed, Player},
    level::Level,
    mrs::Vfs,
    shop::{Icons, ShopData},
    view::decode,
};
use bevy::{image::ImageSampler, prelude::*, transform::TransformSystems};
use std::collections::VecDeque;

mod feed;
mod fx;
mod panels;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Marks>()
            .add_message::<Notice>()
            .init_resource::<Hurt>()
            .init_resource::<Shake>()
            .add_plugins((fx::plugin, feed::plugin))
            .add_systems(
                Update,
                (
                    load.run_if(resource_exists::<Level>.and_then(resource_exists::<ActorData>))
                        .run_if(not(resource_exists::<Hud>)),
                    (
                        panels::spawn_ui.run_if(not(any_with_component::<Root>)),
                        track,
                        panels::vitals,
                        panels::reticle,
                        panels::weapon,
                        panels::scores,
                        panels::banner,
                        crate::menu::fit_hud,
                        panels::touch_layout,
                        panels::earned,
                        indicators,
                    )
                        .chain()
                        .run_if(resource_exists::<Hud>),
                    decals
                        .run_if(resource_exists::<Decals>)
                        .run_if(resource_exists::<MapCollision>),
                ),
            )
            // The shake edits the camera's global transform after propagation, so the camera
            // controller keeps writing its plain transform and nothing accumulates.
            .add_systems(PostUpdate, shake_camera.after(TransformSystems::Propagate));
    }
}

/// Immutable HUD data, loaded once the `Level` and `ActorData` (items) exist: the item icons of
/// the weapon strip.
#[derive(Resource)]
struct Hud {
    shop: ShopData,
    icons: Icons,
}

/// Seconds left of the transient overlays.
#[derive(Resource, Default)]
struct Marks {
    hit: f32,
    kill: f32,
    flash: f32,
    /// Centre-screen notice ("You killed X") and its seconds left.
    notice: (f32, String),
}

/// A centre-screen line for the player (e.g. the sensitivity keys' new value).
#[derive(Message)]
pub struct Notice(pub String);

/// Recent hits on the player: seconds left, the attacker and where the hit came from.
#[derive(Resource, Default)]
struct Hurt(Vec<(f32, Entity, Vec3)>);

/// Camera shake "trauma" in 0..=1 (offset and tilt grow with its square, decaying linearly).
#[derive(Resource, Default)]
struct Shake(f32);

/// Bullet-hole and blood-mark decals: unlit textured quads laid on the hit surface, the
/// oldest recycled past [`MAX_DECALS`].
#[derive(Resource)]
struct Decals {
    quad: Handle<Mesh>,
    bullet: Vec<Handle<StandardMaterial>>,
    blood: Vec<Handle<StandardMaterial>>,
    spawned: VecDeque<Entity>,
    seed: u32,
}

const MAX_DECALS: usize = 128;
/// Bullet hole size (m) and blood-mark size range (m) (**inferred**).
const HOLE_SIZE: f32 = 0.22;
const BLOOD_SIZE: (f32, f32) = (0.5, 0.9);
/// How far behind a hit actor a wall still gets a blood mark (m).
const BLOOD_REACH: f32 = 2.5;
/// Distance from the surface a decal floats at (m), against z-fighting.
const DECAL_LIFT: f32 = 0.006;
/// Seconds a damage-direction arc stays up, and how many can show at once.
const HURT_LIFE: f32 = 1.5;
const HURT_SLOTS: usize = 3;
/// Below this HP fraction the screen edges turn red (**inferred**).
const LOW_HP: f32 = 0.3;
/// Shake trauma lost per second; trauma of a blast at the camera's feet; blast range (m).
const SHAKE_DECAY: f32 = 1.6;
const SHAKE_BLAST: f32 = 0.7;
const SHAKE_RANGE: f32 = 18.0;
/// Largest shake offset (m) and tilt (rad) at trauma 1: kept gentle.
const SHAKE_OFFSET: f32 = 0.08;
const SHAKE_TILT: f32 = 0.012;

/// `interface/default/<name>` (file name with extension) as sRGB UI art; `None` if absent.
pub fn try_image(vfs: &Vfs, images: &mut Assets<Image>, name: &str) -> Option<Handle<Image>> {
    let bytes = vfs.read(&format!("interface/default/{name}")).ok()?;
    let img = decode(
        &bytes,
        name.rsplit('.').next()?,
        true,
        ImageSampler::linear(),
    )?;
    Some(images.add(img))
}

/// `sfx/<name>` (tga/bmp) as sRGB art; the bullet-hole and blood-mark decal textures.
fn sfx_image(vfs: &Vfs, images: &mut Assets<Image>, name: &str) -> Option<Handle<Image>> {
    let bytes = vfs.read(&format!("sfx/{name}")).ok()?;
    let img = decode(
        &bytes,
        name.rsplit('.').next()?,
        true,
        ImageSampler::linear(),
    )?;
    Some(images.add(img))
}

fn load(
    mut commands: Commands,
    level: Res<Level>,
    data: Res<ActorData>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let vfs = &level.vfs;
    let mut decal = |names: Vec<String>| -> Vec<Handle<StandardMaterial>> {
        names
            .iter()
            .map(|n| {
                let tex = sfx_image(vfs, &mut images, n)
                    .unwrap_or_else(|| panic!("sfx/{n} missing or undecodable"));
                materials.add(StandardMaterial {
                    base_color_texture: Some(tex),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    cull_mode: None,
                    depth_bias: 2.0,
                    ..default()
                })
            })
            .collect()
    };
    commands.insert_resource(Decals {
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
        bullet: decal(
            (1..=2)
                .map(|i| format!("gz_sfx_shotgun_bulletmark0{i}.tga"))
                .collect(),
        ),
        blood: decal((1..=5).map(|i| format!("blood-mark0{i}.tga")).collect()),
        spawned: VecDeque::new(),
        seed: 0x2545_F491,
    });
    let shop = ShopData::load(vfs, &data.items).unwrap_or_else(|e| panic!("hud: shop data: {e}"));
    let icons = Icons::load(vfs, &shop, &mut images);
    commands.insert_resource(Hud { shop, icons });
}

#[derive(Component)]
struct Root;

/// Red screen-edge vignette shown at low health.
#[derive(Component)]
struct LowHp;

/// Damage-direction arc `i` (index into [`Hurt`]).
#[derive(Component)]
struct Indicator(usize);

/// HP fill colours by remaining fraction; the tier thresholds are inferred (retail lists four
/// colours `INDEX 0..3` white, yellow, orange, red without the rule that picks one).
const HP_TIERS: [([u8; 3], [u8; 3]); 4] = [
    ([212, 212, 212], [230, 230, 230]),
    ([232, 190, 58], [255, 235, 60]),
    ([232, 128, 58], [255, 179, 60]),
    ([207, 60, 56], [216, 81, 29]),
];

fn tier(frac: f32) -> usize {
    match frac {
        f if f >= 0.75 => 0,
        f if f >= 0.5 => 1,
        f if f >= 0.25 => 2,
        _ => 3,
    }
}

/// Folds this frame's messages into the transient overlays and the kill feed.
#[allow(clippy::too_many_arguments)]
fn track(
    real: Res<Time<Real>>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    mut blasts: MessageReader<Blast>,
    mut notices: MessageReader<Notice>,
    mut shakes: MessageReader<CameraShake>,
    players: Query<&GlobalTransform, With<Player>>,
    transforms: Query<&GlobalTransform>,
    names: Query<&Name>,
    mut marks: ResMut<Marks>,
    mut hurt: ResMut<Hurt>,
    mut shake: ResMut<Shake>,
) {
    // Overlays fade on real time so a pause does not freeze a red flash on screen.
    let dt = real.delta_secs();
    marks.hit = (marks.hit - dt).max(0.0);
    marks.kill = (marks.kill - dt).max(0.0);
    marks.flash = (marks.flash - dt).max(0.0);
    marks.notice.0 = (marks.notice.0 - dt).max(0.0);
    shake.0 = (shake.0 - SHAKE_DECAY * dt).max(0.0);
    for h in &mut hurt.0 {
        h.0 -= dt;
    }
    hurt.0.retain(|h| h.0 > 0.0);
    for d in damage.read() {
        if players.contains(d.attacker) && d.attacker != d.target {
            marks.hit = 0.2;
        }
        if players.contains(d.target) {
            marks.flash = 0.4;
            if d.attacker != d.target {
                let from = transforms
                    .get(d.attacker)
                    .map_or(d.point - d.dir * 3.0, |t| t.translation());
                if let Some(h) = hurt.0.iter_mut().find(|h| h.1 == d.attacker) {
                    (h.0, h.2) = (HURT_LIFE, from);
                } else {
                    if hurt.0.len() >= HURT_SLOTS {
                        hurt.0.remove(0);
                    }
                    hurt.0.push((HURT_LIFE, d.attacker, from));
                }
            }
        }
    }
    for b in blasts.read() {
        if let Ok(p) = players.single() {
            let near = (1.0 - p.translation().distance(b.at) / SHAKE_RANGE).clamp(0.0, 1.0);
            shake.0 = (shake.0 + SHAKE_BLAST * near * near).min(1.0);
        }
    }
    for s in shakes.read() {
        if let Ok(p) = players.single() {
            let near = (1.0 - p.translation().distance(s.at) / s.range.max(0.1)).clamp(0.0, 1.0);
            shake.0 = shake.0.max(s.trauma * near);
        }
    }
    for k in killed.read() {
        if players.contains(k.killer) && k.killer != k.victim {
            let name = names.get(k.victim).map_or("?", |n| n.as_str());
            marks.kill = 0.8;
            marks.notice = (2.0, format!("You killed {name}"));
        }
    }
    for n in notices.read() {
        marks.notice = (1.5, n.0.clone());
    }
}

/// The red arcs around the crosshair pointing at whoever hurt the player, relative to the
/// camera's heading (0 = ahead, clockwise).
fn indicators(
    hurt: Res<Hurt>,
    transforms: Query<&GlobalTransform>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut arcs: Query<(&Indicator, &mut UiTransform, &mut Visibility, &Children)>,
    mut segs: Query<(&mut BackgroundColor, &mut BoxShadow)>,
) {
    let Ok(cam) = camera.single() else {
        return;
    };
    let (fwd, right) = (cam.forward().xz(), cam.right().xz());
    for (Indicator(i), mut t, mut v, kids) in &mut arcs {
        let Some((left, attacker, from)) = hurt.0.get(*i) else {
            if *v != Visibility::Hidden {
                *v = Visibility::Hidden;
            }
            continue;
        };
        let from = transforms.get(*attacker).map_or(*from, |t| t.translation());
        let to = (from - cam.translation()).xz();
        t.rotation = Rot2::radians(to.dot(right).atan2(to.dot(fwd)));
        *v = Visibility::Inherited;
        // fully lit, then fading over the last 0.75 s
        let fade = (left / 0.75).min(1.0);
        let mid = (kids.len() as f32 - 1.0) / 2.0;
        for (n, k) in kids.iter().enumerate() {
            if let Ok((mut c, mut glow)) = segs.get_mut(k) {
                let taper = 1.0 - ((n as f32 - mid).abs() / (mid + 1.0));
                c.0 = Color::srgba(1.0, 0.18, 0.14, fade * (0.35 + 0.6 * taper));
                glow.0[0].color = Color::srgba(1.0, 0.1, 0.05, fade * 0.5 * taper);
            }
        }
    }
}

fn rand(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed >> 8) as f32 / (1u32 << 24) as f32
}

/// Bullet holes where `Impact`s hit the map, blood marks on the wall or floor behind a hit
/// actor (a ray along the hit direction).
fn decals(
    mut commands: Commands,
    mut decals: ResMut<Decals>,
    col: Res<MapCollision>,
    mut impacts: MessageReader<Impact>,
    mut damage: MessageReader<Damage>,
    settings: Res<crate::game::Settings>,
) {
    let dec = &mut *decals;
    let mut marks: Vec<(Vec3, Vec3, f32, bool)> = Vec::new();
    for i in impacts.read().filter(|i| !i.blade) {
        marks.push((i.point, i.normal, HOLE_SIZE, false));
    }
    let mut bled: Vec<Entity> = Vec::new();
    // The simulated blood (`gore.rs`) leaves its own stains.
    for d in damage.read().filter(|_| !settings.realistic_blood) {
        if bled.contains(&d.target) {
            continue;
        }
        bled.push(d.target);
        if let Some(h) = col.raycast(d.point, d.dir, BLOOD_REACH) {
            let size = BLOOD_SIZE.0 + rand(&mut dec.seed) * (BLOOD_SIZE.1 - BLOOD_SIZE.0);
            marks.push((h.point, h.normal, size, true));
        }
    }
    for (at, normal, size, blood) in marks {
        let set = if blood { &dec.blood } else { &dec.bullet };
        let material = set[(rand(&mut dec.seed) * set.len() as f32) as usize % set.len()].clone();
        let roll = rand(&mut dec.seed) * std::f32::consts::TAU;
        let e = commands
            .spawn((
                crate::game::MapEntity,
                Mesh3d(dec.quad.clone()),
                MeshMaterial3d(material),
                Transform {
                    translation: at + normal * DECAL_LIFT,
                    rotation: Quat::from_rotation_arc(Vec3::Z, normal)
                        * Quat::from_rotation_z(roll),
                    scale: Vec3::splat(size),
                },
            ))
            .id();
        dec.spawned.push_back(e);
        if dec.spawned.len() > MAX_DECALS
            && let Some(old) = dec.spawned.pop_front()
        {
            commands.entity(old).despawn();
        }
    }
}

/// Explosion shake: a short offset and tilt of the camera as rendered (see the plugin).
fn shake_camera(
    shake: Res<Shake>,
    time: Res<Time<Real>>,
    mut camera: Query<(&Transform, &mut GlobalTransform), With<Camera3d>>,
) {
    if shake.0 <= 0.0 {
        return;
    }
    let (a, t) = (shake.0 * shake.0, time.elapsed_secs());
    let wobble = |f: f32, p: f32| (t * f + p).sin();
    for (tf, mut g) in &mut camera {
        let local = Transform {
            translation: Vec3::new(wobble(37.0, 0.0), wobble(41.0, 1.3), 0.0) * SHAKE_OFFSET * a,
            rotation: Quat::from_euler(
                EulerRot::XYZ,
                wobble(29.0, 2.1) * SHAKE_TILT * a,
                wobble(31.0, 0.7) * SHAKE_TILT * a,
                wobble(23.0, 4.0) * SHAKE_TILT * a,
            ),
            ..default()
        };
        *g = GlobalTransform::from(*tf) * local;
    }
}

fn set(text: &mut Text, s: String) {
    if text.0 != s {
        text.0 = s;
    }
}
