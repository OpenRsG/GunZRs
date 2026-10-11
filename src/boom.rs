//! Realistic explosions (**not retail**, `gfx::Knob::Explosions`; 0 = RETAIL, as ORIGINAL has it:
//! only the game's own `rocket_effect` / `ef_exgrenade` sprite). Every [`Boom`] of a rocket, frag
//! grenade or mine (`projectile::fly`) is drawn, instead of that sprite, as: a blinding HDR flash, a
//! few additive white-yellow core puffs, orange emissive flame lobes and black soot rolling out around
//! them (alpha-blended, not emissive), all crisp-edged billboards whose noise texture is eroded further
//! each step of their life so they break up and curl, soot turning dark grey and a larger grey plume
//! that rises and lingers for seconds, streak-stretched sparks, lumpy tumbling debris with smoke
//! trails, dust racing out along a floor, a shock ring and a scorch decal on the nearest surface; a
//! flashbang or stun grenade keeps its sprite and gets the flash, sparks and ring. The light is
//! `light.rs`, the shake `hud::Shake` (through [`CameraShake`]). Everything is procedural
//! (`docs/formats.md` "Explosions"): shared meshes, and per puff kind a material per step of its
//! life ([`puff_ramp`]: eroded texture + tint), so a frame changes no material. Budget:
//! [`MAX_PUFFS`] billboards, [`MAX_SPARKS`] streaks, [`MAX_CHUNKS`] chunks, [`MAX_SCARS`] scorch
//! marks (oldest recycled); the rest despawns by itself.

use crate::{
    col::{Hit, MapCollision},
    combat::rnd,
    game::{CameraShake, MapEntity},
    gfx::Knob,
    gore::{paint, smooth},
    light::knob,
    profile::Profile,
};
use bevy::{mesh::VertexAttributeValues, prelude::*};
use std::{collections::VecDeque, f32::consts::TAU};

/// An explosion at `at` of `radius` metres. `fire`: a rocket, frag or mine (all the layers);
/// otherwise a flashbang or stun grenade (flash, sparks and a ring).
#[derive(Message, Clone, Copy)]
pub struct Boom {
    pub at: Vec3,
    pub radius: f32,
    pub fire: bool,
}

const MAX_PUFFS: usize = 260;
const MAX_SPARKS: usize = 700;
const MAX_CHUNKS: usize = 90;
const MAX_SCARS: usize = 16;
/// Spark and chunk gravity (m/s²).
const SPARK_G: f32 = 14.0;
const CHUNK_G: f32 = 22.0;
/// Materials per colour ramp.
const STEPS: usize = 20;
/// Side of a puff texture in texels.
const TEX: u32 = 96;

pub struct BoomPlugin;

impl Plugin for BoomPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Boom>()
            .add_systems(Startup, setup)
            .add_systems(
                Update,
                (burst, puffs, rings, sparks, chunks)
                    .run_if(resource_exists::<Look>)
                    .run_if(resource_exists::<MapCollision>),
            );
    }
}

// ---- the ramps ----

/// `(position 0..1, value)` keys of a ramp, linear and HDR.
type Keys = [(f32, [f32; 4])];

fn sample<const N: usize>(keys: &[(f32, [f32; N])], f: f32) -> [f32; N] {
    let i = keys.iter().position(|k| k.0 >= f).unwrap_or(keys.len() - 1);
    if i == 0 {
        return keys[0].1;
    }
    let (a, b) = (keys[i - 1], keys[i]);
    let t = ((f - a.0) / (b.0 - a.0)).clamp(0.0, 1.0);
    std::array::from_fn(|c| a.1[c] + (b.1[c] - a.1[c]) * t)
}

/// One material per step of a ramp over a particle's life.
struct Ramp(Vec<Handle<StandardMaterial>>);

impl Ramp {
    fn at(&self, f: f32) -> &Handle<StandardMaterial> {
        &self.0[(f.clamp(0.0, 1.0) * (self.0.len() - 1) as f32).round() as usize]
    }
}

// Indices into `Look::ramps`: the puffs first (`puff_ramp`, two textures where noted), then the rest.
const CORE: usize = 0;
const FLAME: usize = 1; // and 2
const SOOT: usize = 3; // and 4
const SMOKE: usize = 5; // and 6
const DUST: usize = 7;
const FLASH: usize = 8;
const SHOCK: usize = 9;
const SPARK: usize = 10;

/// Black body: dark ember, red, orange, yellow, white-yellow (temperature 0..1), HDR.
const HEAT: [(f32, [f32; 3]); 5] = [
    (0.0, [0.02, 0.008, 0.004]),
    (0.2, [0.5, 0.08, 0.01]),
    (0.45, [2.6, 0.7, 0.08]),
    (0.75, [6.0, 3.2, 0.6]),
    (1.0, [9.0, 7.5, 4.0]),
];
/// Soot: black-brown first, dark grey by the end of its life.
const SOOT_KEYS: [(f32, [f32; 3]); 4] = [
    (0.0, [0.012, 0.009, 0.007]),
    (0.3, [0.02, 0.017, 0.015]),
    (0.7, [0.07, 0.068, 0.066]),
    (1.0, [0.12, 0.12, 0.12]),
];
/// The plume: dark, then thin grey.
const SMOKE_KEYS: [(f32, [f32; 3]); 4] = [
    (0.0, [0.025, 0.02, 0.016]),
    (0.1, [0.05, 0.046, 0.043]),
    (0.4, [0.12, 0.118, 0.115]),
    (1.0, [0.2, 0.2, 0.2]),
];
const FLASH_KEYS: [(f32, [f32; 4]); 3] = [
    (0.0, [10.0, 7.0, 3.5, 1.0]),
    (0.25, [6.0, 4.0, 2.0, 0.8]),
    (1.0, [1.0, 0.5, 0.2, 0.0]),
];
const SHOCK_KEYS: [(f32, [f32; 4]); 2] = [(0.0, [5.0, 4.0, 3.0, 0.9]), (1.0, [1.0, 0.8, 0.6, 0.0])];
const SPARK_KEYS: [(f32, [f32; 4]); 3] = [
    (0.0, [5.0, 3.0, 1.0, 1.0]),
    (0.4, [2.5, 1.0, 0.25, 1.0]),
    (1.0, [0.8, 0.1, 0.02, 1.0]),
];

/// Colour of a black body at temperature `t` (0..1). The `*_shade` functions take a puff's life
/// `f` and `inner` (0 at its eroding rim, 1 deep inside) and return linear HDR.
fn heat(t: f32) -> [f32; 3] {
    sample(&HEAT, t.clamp(0.0, 1.0))
}

/// Flame: hottest in the middle of a puff, cooling with age, a dark red rim.
fn flame_shade(f: f32, inner: f32) -> [f32; 3] {
    heat((1.0 - f).powf(2.4) * (0.1 + 0.9 * inner.powf(0.9)))
}

fn core_shade(f: f32, inner: f32) -> [f32; 3] {
    heat((1.0 - f).powf(0.6) * (0.55 + 0.45 * inner))
}

/// A dark body, lighter in the middle than at the rim, glowing orange inside for the `glow` first
/// stretch of its life.
fn ember(base: [f32; 3], glow: f32, inner: f32) -> [f32; 3] {
    let lit = 0.15 + 0.85 * inner;
    let g = glow.max(0.0).powi(2) * inner * inner;
    [
        base[0] * lit + 3.0 * g,
        base[1] * lit + 0.9 * g,
        base[2] * lit + 0.12 * g,
    ]
}

fn soot_shade(f: f32, inner: f32) -> [f32; 3] {
    ember(sample(&SOOT_KEYS, f), 0.3 * (1.0 - f / 0.3), inner)
}

fn smoke_shade(f: f32, inner: f32) -> [f32; 3] {
    ember(sample(&SMOKE_KEYS, f), 0.3 * (1.0 - f / 0.2), inner)
}

fn dust_shade(_: f32, inner: f32) -> [f32; 3] {
    let l = 0.6 + 0.4 * inner;
    [0.5 * l, 0.42 * l, 0.33 * l]
}

/// How a kind of puff looks over its life. Its texture is the puff's density cut at a threshold
/// that rises from `erode.0` to `erode.1` as `f^curve` (alpha = smoothstep over `edge`), coloured
/// by `shade` (linear HDR, divided by `gain` into the texture and multiplied back by the material).
struct Shape {
    mode: AlphaMode,
    erode: (f32, f32),
    curve: f32,
    edge: f32,
    gain: f32,
    fade: &'static [(f32, [f32; 1])],
    shade: fn(f32, f32) -> [f32; 3],
}

/// Hot core: additive, white-yellow, gone in a third of a second.
const CORE_SHAPE: Shape = Shape {
    mode: AlphaMode::Add,
    erode: (0.08, 0.95),
    curve: 1.0,
    edge: 0.18,
    gain: 9.0,
    fade: &[(0.0, [1.0]), (0.6, [0.8]), (1.0, [0.0])],
    shade: core_shade,
};
/// Flame lobes: opaque, emissive, breaking up as they cool.
const FLAME_SHAPE: Shape = Shape {
    mode: AlphaMode::Blend,
    erode: (0.08, 0.95),
    curve: 1.2,
    edge: 0.07,
    gain: 9.0,
    fade: &[(0.0, [1.0]), (0.85, [1.0]), (1.0, [0.0])],
    shade: flame_shade,
};
/// Soot rolling out around the fire: alpha-blended, not emissive.
const SOOT_SHAPE: Shape = Shape {
    mode: AlphaMode::Blend,
    erode: (0.25, 0.85),
    curve: 1.3,
    edge: 0.1,
    gain: 2.0,
    fade: &[
        (0.0, [0.0]),
        (0.1, [0.0]),
        (0.2, [0.9]),
        (0.6, [0.85]),
        (1.0, [0.0]),
    ],
    shade: soot_shade,
};
/// The plume: shows once the fireball has peaked.
const SMOKE_SHAPE: Shape = Shape {
    mode: AlphaMode::Blend,
    erode: (0.1, 0.8),
    curve: 1.2,
    edge: 0.15,
    gain: 2.0,
    fade: &[
        (0.0, [0.0]),
        (0.08, [0.0]),
        (0.2, [0.75]),
        (0.5, [0.6]),
        (1.0, [0.0]),
    ],
    shade: smoke_shade,
};
const DUST_SHAPE: Shape = Shape {
    mode: AlphaMode::Blend,
    erode: (0.05, 0.5),
    curve: 1.0,
    edge: 0.3,
    gain: 1.0,
    fade: &[(0.0, [0.0]), (0.1, [0.4]), (0.5, [0.3]), (1.0, [0.0])],
    shade: dust_shade,
};

/// A `shape`'s materials over a puff's life, each with its own eroded texture of `seed`'s density.
fn puff_ramp(
    images: &mut Assets<Image>,
    materials: &mut Assets<StandardMaterial>,
    s: &Shape,
    seed: u32,
) -> Ramp {
    let rho = density(seed);
    let n = TEX as usize;
    Ramp(
        (0..STEPS)
            .map(|i| {
                let f = i as f32 / (STEPS - 1) as f32;
                let th = s.erode.0 + (s.erode.1 - s.erode.0) * f.powf(s.curve);
                let tex = paint(TEX, TEX, |u, v| {
                    let r = rho[(v * n as f32) as usize * n + (u * n as f32) as usize];
                    let c = (s.shade)(f, ((r - th) / 0.8).clamp(0.0, 1.0));
                    [
                        srgb(c[0] / s.gain),
                        srgb(c[1] / s.gain),
                        srgb(c[2] / s.gain),
                        smooth(th, th + s.edge, r),
                    ]
                });
                materials.add(StandardMaterial {
                    base_color: Color::LinearRgba(LinearRgba::new(
                        s.gain,
                        s.gain,
                        s.gain,
                        sample(s.fade, f)[0],
                    )),
                    base_color_texture: Some(images.add(tex)),
                    unlit: true,
                    alpha_mode: s.mode,
                    cull_mode: None,
                    ..default()
                })
            })
            .collect(),
    )
}

// ---- shared assets ----

#[derive(Resource)]
struct Look {
    quad: Handle<Mesh>,
    streak: Handle<Mesh>,
    rocks: Vec<Handle<Mesh>>,
    ramps: Vec<Ramp>,
    chunks: Vec<Handle<StandardMaterial>>,
    scorch: Handle<StandardMaterial>,
    scars: VecDeque<Entity>,
    seed: u32,
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut ramps: Vec<Ramp> = [
        (&CORE_SHAPE, 0xC0E1),
        (&FLAME_SHAPE, 0xF1E1),
        (&FLAME_SHAPE, 0xF2E2),
        (&SOOT_SHAPE, 0x5001),
        (&SOOT_SHAPE, 0x5002),
        (&SMOKE_SHAPE, 0x5A01),
        (&SMOKE_SHAPE, 0x5A02),
        (&DUST_SHAPE, 0x5A03),
    ]
    .into_iter()
    .map(|(s, seed)| puff_ramp(&mut images, &mut materials, s, seed))
    .collect();
    let mut ramp = |tex: Option<Image>, mode: AlphaMode, bias: f32, keys: &Keys, steps: usize| {
        let tex = tex.map(|t| images.add(t));
        Ramp(
            (0..steps)
                .map(|i| {
                    let c = sample(keys, i as f32 / (steps - 1) as f32);
                    materials.add(StandardMaterial {
                        base_color: Color::LinearRgba(LinearRgba::new(c[0], c[1], c[2], c[3])),
                        base_color_texture: tex.clone(),
                        unlit: true,
                        alpha_mode: mode,
                        cull_mode: None,
                        depth_bias: bias,
                        ..default()
                    })
                })
                .collect(),
        )
    };
    ramps.extend([
        ramp(Some(glow_image()), AlphaMode::Add, 0.0, &FLASH_KEYS, STEPS),
        ramp(Some(ring_image()), AlphaMode::Add, 5.0, &SHOCK_KEYS, STEPS),
        ramp(None, AlphaMode::Add, 0.0, &SPARK_KEYS, 8),
    ]);
    let scorch = images.add(scorch_image());
    let chunks = [[0.1, 0.09, 0.08], [0.16, 0.14, 0.12], [0.2, 0.15, 0.11]]
        .map(|c| {
            materials.add(StandardMaterial {
                base_color: Color::srgb(c[0], c[1], c[2]),
                perceptual_roughness: 1.0,
                ..default()
            })
        })
        .to_vec();
    commands.insert_resource(Look {
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
        streak: meshes.add(Cuboid::new(0.02, 0.02, 0.1)),
        rocks: (0..3).map(|i| meshes.add(rock(0x20C0 + i))).collect(),
        ramps,
        chunks,
        scorch: materials.add(StandardMaterial {
            base_color: Color::linear_rgba(0.02, 0.018, 0.016, 1.0),
            base_color_texture: Some(scorch),
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            cull_mode: None,
            depth_bias: 3.0,
            ..default()
        }),
        scars: VecDeque::new(),
        seed: 0xB00B_5EED,
    });
}

// ---- components ----

/// A camera-facing billboard (fireball, smoke, flash) moving under drag and lift.
#[derive(Component)]
struct Puff {
    vel: Vec3,
    age: f32,
    life: f32,
    /// Diameter at birth and at death, and the exponent of the ease-out between.
    size: (f32, f32),
    ease: i32,
    drag: f32,
    lift: f32,
    roll: f32,
    spin: f32,
    ramp: usize,
}

/// A flat ring on a surface growing from `size.0` to `size.1` metres across.
#[derive(Component)]
struct Ring {
    age: f32,
    life: f32,
    size: (f32, f32),
    ramp: usize,
}

#[derive(Component)]
struct Spark {
    vel: Vec3,
    age: f32,
    life: f32,
    bounces: u8,
    rest: bool,
}

#[derive(Component)]
struct Chunk {
    vel: Vec3,
    spin: Vec3,
    size: f32,
    age: f32,
    life: f32,
    /// Seconds to the next puff of its smoke trail.
    trail: f32,
    rest: bool,
}

// ---- geometry helpers ----

/// The nearest map surface within `reach` metres along the six axes. The floor ray starts a
/// little above `at`: a rocket stops with its centre almost in the floor.
fn surface(col: &MapCollision, at: Vec3, reach: f32) -> Option<Hit> {
    let floor = col
        .raycast(at + Vec3::Y * 0.1, Vec3::NEG_Y, reach + 0.1)
        .map(|h| Hit {
            distance: h.distance - 0.1,
            ..h
        });
    [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::NEG_Z, Vec3::Y]
        .iter()
        .filter_map(|&d| col.raycast(at, d, reach))
        .chain(floor)
        .min_by(|a, b| a.distance.total_cmp(&b.distance))
}

/// `pos` moved by `step`, stopped `margin` metres short of a map surface (whose normal is
/// returned too).
fn travel(col: &MapCollision, pos: Vec3, step: Vec3, margin: f32) -> (Vec3, Option<Vec3>) {
    let len = step.length();
    if len < 1e-5 {
        return (pos, None);
    }
    match col.raycast(pos, step, len + margin) {
        Some(h) => (
            pos + step / len * (h.distance - margin).clamp(0.0, len),
            Some(h.normal),
        ),
        None => (pos + step, None),
    }
}

fn unit(s: &mut u32) -> Vec3 {
    let z = rnd(s) * 2.0 - 1.0;
    let a = rnd(s) * TAU;
    let q = (1.0 - z * z).sqrt();
    Vec3::new(q * a.cos(), z, q * a.sin())
}

/// A random direction on the side of `n`, pulled toward it by `tight` (0 = uniform).
fn hemi(s: &mut u32, n: Vec3, tight: f32) -> Vec3 {
    let v = unit(s);
    let v = if v.dot(n) < 0.0 {
        v - 2.0 * v.dot(n) * n
    } else {
        v
    };
    (v + n * tight).normalize_or(n)
}

/// A quad's rotation lying on a surface with normal `n`, turned by `roll`.
fn flat(n: Vec3, roll: f32) -> Quat {
    let x = n.any_orthonormal_vector();
    Quat::from_mat3(&Mat3::from_cols(x, n.cross(x), n)) * Quat::from_rotation_z(roll)
}

/// The surface an explosion sits on.
struct Site {
    /// Its normal (up in the open).
    n: Vec3,
    on_surface: bool,
}

impl Site {
    /// A random direction away from the surface, or any up-leaning one in the open.
    fn away(&self, s: &mut u32, tight: f32) -> Vec3 {
        if self.on_surface {
            hemi(s, self.n, tight)
        } else {
            (unit(s) + Vec3::Y * 0.3).normalize()
        }
    }
}

fn spawn_puff(commands: &mut Commands, look: &Look, at: Vec3, p: Puff) {
    commands.spawn((
        Mesh3d(look.quad.clone()),
        MeshMaterial3d(look.ramps[p.ramp].at(0.0).clone()),
        Transform::from_translation(at).with_scale(Vec3::splat(p.size.0)),
        p,
    ));
}

// ---- spawning ----

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn burst(
    mut booms: MessageReader<Boom>,
    profile: Option<Res<Profile>>,
    mut look: ResMut<Look>,
    col: Res<MapCollision>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    live: (
        Query<(), With<Puff>>,
        Query<(), With<Spark>>,
        Query<(), With<Chunk>>,
    ),
    mut shake: MessageWriter<CameraShake>,
    mut commands: Commands,
) {
    let k = knob(&profile, Knob::Explosions);
    if k <= 0.0 {
        booms.clear();
        return;
    }
    let mut room = [
        MAX_PUFFS.saturating_sub(live.0.iter().count()),
        MAX_SPARKS.saturating_sub(live.1.iter().count()),
        MAX_CHUNKS.saturating_sub(live.2.iter().count()),
    ];
    let eye = camera.iter().next().map(|c| c.translation());
    let mut seed = look.seed;
    let s = &mut seed;
    for b in booms.read() {
        let r = b.radius.clamp(1.5, 6.0);
        let hit = surface(&col, b.at, (r * 0.45).max(1.2));
        let site = Site {
            n: hit.map_or(Vec3::Y, |h| h.normal),
            on_surface: hit.is_some(),
        };
        // the flash sits a little toward the camera so the floor and walls do not cut it
        let toward = eye.map_or(Vec3::ZERO, |e| e - b.at);
        let lead = toward.normalize_or_zero() * (r * 0.4).min(toward.length() * 0.5);
        let (flash_at, _) = travel(&col, b.at, lead, 0.2);
        let (life, big) = if b.fire { (0.2, 1.0) } else { (0.35, 0.7) };
        if room[0] > 0 {
            room[0] -= 1;
            let flash = Puff {
                vel: Vec3::ZERO,
                age: 0.0,
                life,
                size: (r * 0.8 * big, r * 1.8 * big),
                ease: 3,
                drag: 0.0,
                lift: 0.0,
                roll: rnd(s) * TAU,
                spin: 0.0,
                ramp: FLASH,
            };
            spawn_puff(&mut commands, &look, flash_at, flash);
        }
        if b.fire {
            // 1: the hot core, additive white-yellow and gone in a third of a second
            let n = 3.min(room[0]);
            room[0] -= n;
            for _ in 0..n {
                let d = site.away(s, 0.3);
                let (at, _) = travel(&col, b.at, d * r * 0.12 * rnd(s), 0.2);
                let p = Puff {
                    vel: d * (0.5 + rnd(s)),
                    age: 0.0,
                    life: 0.3 + 0.15 * rnd(s),
                    size: (r * 0.3, r * 0.65 * (0.8 + 0.4 * rnd(s))),
                    ease: 2,
                    drag: 2.0,
                    lift: 1.0,
                    roll: rnd(s) * TAU,
                    spin: (rnd(s) - 0.5) * 2.0,
                    ramp: CORE,
                };
                spawn_puff(&mut commands, &look, at, p);
            }
            // 2: orange flame lobes, a few opaque eroding puffs thrown out and rising
            let n = ((8.0 + 10.0 * k) as usize).min(room[0]);
            room[0] -= n;
            for _ in 0..n {
                let d = site.away(s, 0.3);
                let (at, _) = travel(&col, b.at, d * r * 0.15 * rnd(s), 0.2);
                let p = Puff {
                    vel: d * (2.5 + 4.5 * rnd(s)) * r / 3.5,
                    age: 0.0,
                    life: 0.8 + 0.7 * rnd(s),
                    size: (
                        r * 0.3 * (0.7 + 0.5 * rnd(s)),
                        r * 0.75 * (0.7 + 0.5 * rnd(s)),
                    ),
                    ease: 2,
                    drag: 3.2,
                    lift: 2.5,
                    roll: rnd(s) * TAU,
                    spin: (rnd(s) - 0.5) * 3.0,
                    ramp: FLAME + (rnd(s) * 2.0) as usize % 2,
                };
                spawn_puff(&mut commands, &look, at, p);
            }
            // 3: soot rolling around the outside, black first, dark grey in a second or two
            let n = ((6.0 + 10.0 * k) as usize).min(room[0]);
            room[0] -= n;
            for _ in 0..n {
                let d = (site.away(s, 0.15) + Vec3::Y * 0.4).normalize();
                let (at, _) = travel(&col, b.at, d * r * (0.3 + 0.3 * rnd(s)), 0.2);
                let p = Puff {
                    vel: d * (3.0 + 3.5 * rnd(s)) * r / 3.5,
                    age: 0.0,
                    life: 1.6 + 1.4 * rnd(s),
                    size: (
                        r * 0.3 * (0.7 + 0.5 * rnd(s)),
                        r * 0.65 * (0.7 + 0.5 * rnd(s)),
                    ),
                    ease: 2,
                    drag: 2.2,
                    lift: 0.9,
                    roll: rnd(s) * TAU,
                    spin: (rnd(s) - 0.5) * 1.6,
                    ramp: SOOT + (rnd(s) * 2.0) as usize % 2,
                };
                spawn_puff(&mut commands, &look, at, p);
            }
            // 4: the plume, bigger and lighter, rising, spreading, lingering
            let n = ((5.0 + 6.0 * k) as usize).min(room[0]);
            room[0] -= n;
            for _ in 0..n {
                let side = unit(s) * Vec3::new(1.0, 0.0, 1.0);
                let up = site.n.lerp(Vec3::Y, 0.6).normalize();
                let (at, _) = travel(&col, b.at, unit(s) * r * 0.3 * rnd(s), 0.2);
                let p = Puff {
                    vel: side * (0.8 + 2.0 * rnd(s)) + up * (1.0 + 2.5 * rnd(s)),
                    age: 0.0,
                    life: 4.0 + 2.5 * rnd(s),
                    size: (
                        r * 0.45 * (0.6 + 0.5 * rnd(s)),
                        r * 0.95 * (0.6 + 0.5 * rnd(s)),
                    ),
                    ease: 2,
                    drag: 1.1,
                    lift: 0.6,
                    roll: rnd(s) * TAU,
                    spin: (rnd(s) - 0.5) * 0.8,
                    ramp: SMOKE + (rnd(s) * 2.0) as usize % 2,
                };
                spawn_puff(&mut commands, &look, at, p);
            }
        }
        // sparks and embers
        let n = (if b.fire {
            30.0 + 90.0 * k
        } else {
            20.0 + 30.0 * k
        } as usize)
            .min(room[1]);
        room[1] -= n;
        for _ in 0..n {
            let vel = site.away(s, 0.2) * (6.0 + 20.0 * rnd(s)) * (r / 3.5).max(0.6);
            commands.spawn((
                Mesh3d(look.streak.clone()),
                MeshMaterial3d(look.ramps[SPARK].at(0.0).clone()),
                Transform::from_translation(b.at + site.n * 0.1),
                Spark {
                    vel,
                    age: 0.0,
                    life: 0.5 + 1.3 * rnd(s),
                    bounces: 0,
                    rest: false,
                },
            ));
        }
        if b.fire {
            // lumps of the surroundings, tumbling and trailing smoke
            let n = ((4.0 + 8.0 * k) as usize).min(room[2]);
            room[2] -= n;
            for _ in 0..n {
                let vel = site.away(s, 0.4) * (5.0 + 9.0 * rnd(s));
                let size = 0.4 + 1.0 * rnd(s);
                let rock = look.rocks[(rnd(s) * 3.0) as usize % 3].clone();
                commands.spawn((
                    Mesh3d(rock),
                    MeshMaterial3d(look.chunks[(rnd(s) * 3.0) as usize % 3].clone()),
                    Transform::from_translation(b.at + site.n * 0.15)
                        .with_rotation(Quat::from_scaled_axis(unit(s) * rnd(s) * TAU))
                        .with_scale(Vec3::splat(size)),
                    Chunk {
                        vel,
                        spin: unit(s) * (4.0 + 8.0 * rnd(s)),
                        size,
                        age: 0.0,
                        life: 2.5 + rnd(s),
                        trail: 0.02 * rnd(s),
                        rest: false,
                    },
                ));
            }
        }
        // rings and scorch on the surface
        if let Some(h) = hit {
            let ring = |commands: &mut Commands,
                        look: &Look,
                        size: (f32, f32),
                        life: f32,
                        ramp: usize,
                        lift: f32| {
                commands.spawn((
                    Mesh3d(look.quad.clone()),
                    MeshMaterial3d(look.ramps[ramp].at(0.0).clone()),
                    Transform::from_translation(h.point + h.normal * lift)
                        .with_rotation(flat(h.normal, 0.0))
                        .with_scale(Vec3::splat(size.0)),
                    Ring {
                        age: 0.0,
                        life,
                        size,
                        ramp,
                    },
                ));
            };
            // dust races out along a floor
            if b.fire && h.normal.y > 0.6 {
                let nd = (8.0 + 6.0 * k) as usize;
                for i in 0..nd.min(room[0]) {
                    let a = (i as f32 + rnd(s)) / nd as f32 * TAU;
                    let d = Vec3::new(a.cos(), 0.0, a.sin());
                    let p = Puff {
                        vel: d * (5.0 + 4.0 * rnd(s)) * r / 3.5,
                        age: 0.0,
                        life: 1.2 + 0.8 * rnd(s),
                        size: (r * 0.25, r * 0.7 * (0.6 + 0.4 * rnd(s))),
                        ease: 2,
                        drag: 2.2,
                        lift: 0.4,
                        roll: rnd(s) * TAU,
                        spin: rnd(s) - 0.5,
                        ramp: DUST,
                    };
                    spawn_puff(&mut commands, &look, h.point + h.normal * 0.3 + d * 0.5, p);
                }
                room[0] -= nd.min(room[0]);
            }
            if k > 0.75 || !b.fire {
                ring(&mut commands, &look, (r * 0.5, r * 2.4), 0.28, SHOCK, 0.03);
            }
            if b.fire {
                let e = commands
                    .spawn((
                        MapEntity,
                        Mesh3d(look.quad.clone()),
                        MeshMaterial3d(look.scorch.clone()),
                        Transform::from_translation(h.point + h.normal * 0.01)
                            .with_rotation(flat(h.normal, rnd(s) * TAU))
                            .with_scale(Vec3::splat(r * 1.5 * (0.8 + 0.4 * rnd(s)))),
                    ))
                    .id();
                look.scars.push_back(e);
                if look.scars.len() > MAX_SCARS
                    && let Some(old) = look.scars.pop_front()
                {
                    commands.entity(old).try_despawn();
                }
            }
        }
        if b.fire {
            shake.write(CameraShake {
                at: b.at,
                trauma: 0.6 + 0.4 * k,
                range: 12.0 + 4.0 * r,
            });
        }
    }
    look.seed = seed;
}

// ---- motion ----

fn puffs(
    time: Res<Time>,
    col: Res<MapCollision>,
    look: Res<Look>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut q: Query<(
        Entity,
        &mut Transform,
        &mut Puff,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(0.05);
    let face = camera
        .iter()
        .next()
        .map_or(Quat::IDENTITY, |c| c.rotation());
    for (e, mut t, mut p, mut m) in &mut q {
        p.age += dt;
        let f = p.age / p.life;
        if f >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        let size = p.size.0 + (p.size.1 - p.size.0) * (1.0 - (1.0 - f).powi(p.ease));
        let drag = (1.0 - p.drag * dt).max(0.0);
        p.vel = p.vel * drag + Vec3::Y * p.lift * dt;
        let (to, hit) = travel(&col, t.translation, p.vel * dt, size * 0.2);
        if let Some(n) = hit {
            // slides along the surface
            let vn = n * p.vel.dot(n);
            p.vel -= vn;
        }
        t.translation = to;
        t.rotation = face * Quat::from_rotation_z(p.roll + p.spin * p.age);
        t.scale = Vec3::splat(size);
        let h = look.ramps[p.ramp].at(f);
        if m.0 != *h {
            m.0 = h.clone();
        }
    }
}

fn rings(
    time: Res<Time>,
    look: Res<Look>,
    mut q: Query<(
        Entity,
        &mut Transform,
        &mut Ring,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(0.05);
    for (e, mut t, mut r, mut m) in &mut q {
        r.age += dt;
        let f = r.age / r.life;
        if f >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        t.scale = Vec3::splat(r.size.0 + (r.size.1 - r.size.0) * (1.0 - (1.0 - f).powi(3)));
        let h = look.ramps[r.ramp].at(f);
        if m.0 != *h {
            m.0 = h.clone();
        }
    }
}

fn sparks(
    time: Res<Time>,
    col: Res<MapCollision>,
    look: Res<Look>,
    mut q: Query<(
        Entity,
        &mut Transform,
        &mut Spark,
        &mut MeshMaterial3d<StandardMaterial>,
    )>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(0.05);
    for (e, mut t, mut s, mut m) in &mut q {
        s.age += dt;
        let f = s.age / s.life;
        if f >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        if !s.rest {
            s.vel.y -= SPARK_G * dt;
            s.vel *= 1.0 - 0.6 * dt;
            let (to, hit) = travel(&col, t.translation, s.vel * dt, 0.02);
            t.translation = to;
            if let Some(n) = hit {
                let vn = n * s.vel.dot(n);
                s.vel = (s.vel - vn) * 0.7 - vn * 0.35;
                s.bounces += 1;
                if s.vel.length() < 1.5 || s.bounces > 3 {
                    (s.rest, s.vel) = (true, Vec3::ZERO);
                }
            }
            if s.vel.length_squared() > 0.04 {
                t.rotation = Transform::IDENTITY.looking_to(s.vel, Vec3::Y).rotation;
            }
        }
        let k = 1.0 - f;
        t.scale = Vec3::new(k, k, (1.0 + s.vel.length() * 0.12) * k);
        let h = look.ramps[SPARK].at(f);
        if m.0 != *h {
            m.0 = h.clone();
        }
    }
}

fn chunks(
    time: Res<Time>,
    col: Res<MapCollision>,
    look: Res<Look>,
    live: Query<(), With<Puff>>,
    mut q: Query<(Entity, &mut Transform, &mut Chunk)>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(0.05);
    let mut room = MAX_PUFFS.saturating_sub(live.iter().count());
    for (e, mut t, mut c) in &mut q {
        c.age += dt;
        if c.age >= c.life {
            commands.entity(e).despawn();
            continue;
        }
        if !c.rest {
            c.vel.y -= CHUNK_G * dt;
            let (to, hit) = travel(&col, t.translation, c.vel * dt, 0.06);
            t.translation = to;
            t.rotation = Quat::from_scaled_axis(c.spin * dt) * t.rotation;
            if let Some(n) = hit {
                let vn = n * c.vel.dot(n);
                c.vel = (c.vel - vn) * 0.6 - vn * 0.35;
                c.spin *= 0.6;
                if c.vel.length() < 1.2 {
                    (c.rest, c.vel) = (true, Vec3::ZERO);
                }
            }
            // a trail of smoke while it flies, glowing at first; the first 40 puffs stay free
            // for the fireball
            c.trail -= dt;
            if c.trail <= 0.0 && c.age < 0.9 && room > 40 {
                c.trail = 0.1;
                room -= 1;
                let p = Puff {
                    vel: Vec3::ZERO,
                    age: 0.0,
                    life: 0.9,
                    size: (0.2 + 0.15 * c.size, 0.6 + 0.4 * c.size),
                    ease: 2,
                    drag: 1.0,
                    lift: 0.5,
                    roll: c.age * 53.0,
                    spin: 0.4,
                    ramp: SMOKE + (c.age * 100.0) as usize % 2,
                };
                spawn_puff(&mut commands, &look, t.translation, p);
            }
        }
        // shrinks away over the last half second
        t.scale = Vec3::splat(c.size * ((c.life - c.age) * 2.0).min(1.0));
    }
}

// ---- procedural textures (sRGB RGBA8; colour lives in the material, the texture is a grey
// shape under transparent texels) ----

fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x27d4_eb2d)
        ^ (y as u32).wrapping_mul(0x1656_67b1)
        ^ seed.wrapping_mul(0x9e37_79b9);
    h ^= h >> 15;
    h = h.wrapping_mul(0x85eb_ca6b);
    h ^= h >> 13;
    h = h.wrapping_mul(0xc2b2_ae35);
    h ^= h >> 16;
    (h & 0x00ff_ffff) as f32 / 16_777_216.0
}

/// Smooth value noise in 0..1.
fn noise(x: f32, y: f32, seed: u32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (smooth(0.0, 1.0, x - xi), smooth(0.0, 1.0, y - yi));
    let (xi, yi) = (xi as i32, yi as i32);
    let top = hash(xi, yi, seed) * (1.0 - fx) + hash(xi + 1, yi, seed) * fx;
    let bottom = hash(xi, yi + 1, seed) * (1.0 - fx) + hash(xi + 1, yi + 1, seed) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Four octaves of [`noise`], 0..1 around 0.5.
fn fbm(x: f32, y: f32, seed: u32) -> f32 {
    let (mut sum, mut amp, mut freq) = (0.0, 0.5, 1.0);
    for o in 0..4 {
        sum += amp * noise(x * freq, y * freq, seed + o);
        amp *= 0.5;
        freq *= 2.0;
    }
    sum / 0.9375
}

/// A puff's density: 1.2 at the heart falling to 0 at the rim of the quad, broken up by domain-warped
/// noise into lobes and creases. A puff is this field cut at a threshold that rises over its life
/// ([`puff_ramp`]), so it breaks up and curls away instead of fading.
fn density(seed: u32) -> Vec<f32> {
    let n = TEX as usize;
    (0..n * n)
        .map(|i| {
            let u = ((i % n) as f32 + 0.5) / n as f32;
            let v = ((i / n) as f32 + 0.5) / n as f32;
            let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0;
            let w = fbm(u * 3.0 + 7.0, v * 3.0, seed);
            let b = fbm(u * 5.0 + w * 2.5, v * 5.0 - w * 2.5, seed + 11);
            // billows: the creases between lobes are where the threshold bites first
            let lobes = 1.0 - (2.0 * b - 1.0).abs();
            (1.2 * (1.0 - d) + 1.0 * (lobes - 0.6)) * smooth(1.0, 0.85, d)
        })
        .collect()
}

/// Linear to sRGB, for texels the GPU decodes back.
fn srgb(x: f32) -> f32 {
    if x <= 0.003_130_8 {
        12.92 * x
    } else {
        1.055 * x.powf(1.0 / 2.4) - 0.055
    }
}

/// The flash: a soft white glow.
fn glow_image() -> Image {
    paint(64, 64, |u, v| {
        let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0;
        [1.0, 1.0, 1.0, (1.0 - d).max(0.0).powf(2.2)]
    })
}

/// A thin ring at 0.82 of the half width with a ragged edge.
fn ring_image() -> Image {
    paint(128, 128, |u, v| {
        let (x, y) = (u - 0.5, v - 0.5);
        let d = (x * x + y * y).sqrt() * 2.0;
        let n = fbm(u * 8.0, v * 8.0, 0x71C5);
        let edge = smooth(0.64, 0.82, d) * smooth(1.0, 0.856, d);
        [1.0, 1.0, 1.0, edge * (0.5 + 0.7 * n)]
    })
}

/// Soot: a dark mottled blot with ragged edges and a few radial streaks.
fn scorch_image() -> Image {
    paint(128, 128, |u, v| {
        let (x, y) = (u - 0.5, v - 0.5);
        let d = (x * x + y * y).sqrt() * 2.0;
        let n = fbm(u * 6.0, v * 6.0, 0x5C0C);
        let streak = noise(y.atan2(x) * 3.0 + 9.0, d * 1.5, 0x5C0D);
        let edge = d + (n - 0.5) * 0.7 + (streak - 0.5) * 0.4;
        [1.0, 1.0, 1.0, smooth(1.0, 0.0, edge) * (0.85 + 0.15 * n)]
    })
}

/// A lump of debris: a small icosphere with every corner pushed in or out and squashed, flat shaded.
fn rock(seed: u32) -> Mesh {
    let mut m = Sphere::new(0.07).mesh().ico(1).expect("a small icosphere");
    if let Some(VertexAttributeValues::Float32x3(p)) = m.attribute_mut(Mesh::ATTRIBUTE_POSITION) {
        for v in p {
            // by position, so a corner shared by several faces moves once
            let q = v.map(|c| (c * 1000.0).round() as i32);
            let g = 0.55 + 0.9 * hash(q[0], q[1] ^ q[2].wrapping_mul(31), seed);
            *v = [v[0] * g * 1.3, v[1] * g * 0.7, v[2] * g];
        }
    }
    // flat faces, so a tumbling lump reads as a solid
    m.duplicate_vertices();
    m.compute_flat_normals();
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ramp_runs_through_its_keys() {
        assert_eq!(sample(&SPARK_KEYS, 0.0), SPARK_KEYS[0].1);
        let end = sample(&SPARK_KEYS, 1.0);
        assert!(
            end.iter()
                .zip(SPARK_KEYS[2].1)
                .all(|(a, b)| (a - b).abs() < 1e-6)
        );
        let mid = sample(&FLASH_KEYS, 0.125);
        assert!(
            (mid[0] - 8.0).abs() < 1e-4,
            "halfway between the first two keys"
        );
    }

    #[test]
    fn a_puff_is_solid_in_the_middle_and_empty_at_the_corners() {
        let n = TEX as usize;
        let rho = density(0xF1E1);
        assert!(rho[n / 2 * n + n / 2] > 0.9);
        assert!(rho[0] <= 0.0 && rho[n * n - 1] <= 0.0);
        // eroding removes texels, never adds them
        let alive = |th: f32| rho.iter().filter(|&&r| r > th).count();
        assert!(alive(0.2) > alive(0.5) && alive(0.5) > alive(0.9));
    }

    #[test]
    fn debris_and_sparks_leave_the_surface_side() {
        let mut s = 0x1234_5678;
        for _ in 0..200 {
            assert!(hemi(&mut s, Vec3::Y, 0.3).y > 0.0);
        }
    }
}
