//! Simulated blood (`Settings::realistic_blood`, on by default): every [`Vfx::Blood`] sprays
//! droplets that fly under gravity, are drawn as velocity-stretched streaks, hit the map
//! (one [`MapCollision`] ray per droplet step) and leave splatter decals shaped by the impact
//! angle; splatters on walls run down in drips, corpses lie in spreading pools and badly hurt
//! actors leave drops behind. With the setting off, `combat.rs` spawns the retail `sfx/blood0N.tga`
//! sprites and `hud.rs` the retail blood marks instead. All textures are procedural
//! (`docs/formats.md`, "Simulated blood"); everything is **inferred** port design.
//!
//! Budget: [`MAX_DROPS`] droplets in flight, [`MAX_STAINS`] stains (pools and drips included,
//! oldest recycled first) and [`STAINS_PER_FRAME`] new stains per frame.

use crate::{
    col::MapCollision,
    combat::{Vfx, rnd},
    game::{Dead, MapEntity, Npc, Player, Settings, Vitals},
};
use bevy::{
    asset::RenderAssetUsages,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};
use std::{collections::VecDeque, f32::consts::TAU};

const MAX_DROPS: usize = 320;
const MAX_MIST: usize = 40;
const MAX_STAINS: usize = 480;
const MAX_DRIPS: usize = 24;
const STAINS_PER_FRAME: u32 = 24;
/// Droplet physics: gravity (m/s²) and air drag (1/s); a droplet is dropped after this many seconds.
const GRAVITY: f32 = 9.8;
const DRAG: f32 = 0.35;
const DROP_LIFE: f32 = 4.0;
/// Distance a stain floats off its surface (m), and the extra lift cycled over stains so that
/// overlapping ones keep a stable order.
const LIFT: f32 = 0.004;
/// Damage that makes one unit of spray; the spray is clamped to this many units.
const SPRAY_UNIT: f32 = 25.0;
const SPRAY_MAX: f32 = 3.0;
/// A stain at least this large on a wall (m) may start a drip, up to this fraction of them.
const DRIP_MIN: f32 = 0.07;
const DRIP_CHANCE: f32 = 0.5;
/// Blood below this HP fraction drips off the actor, up to `BLEED_RATE` drops/s at 0 HP.
const BLEED_BELOW: f32 = 0.5;
const BLEED_RATE: f32 = 4.0;
/// Seconds a pool waits for the corpse to land, and its time constant while spreading (it is
/// 95% grown 1.8 s later).
const POOL_DELAY: f32 = 0.4;
const POOL_SPREAD: f32 = 0.6;
/// Seconds after which a stain moves to its next, darker stage ([`tint`]).
const AGE: [f32; 2] = [15.0, 60.0];

/// Blood colour (sRGB) of a fresh splash and of its thick, dark middle: textures are baked a
/// little brown and tinted red while fresh, so that ageing is a plain tint change.
const FRESH: [f32; 3] = [0.6, 0.06, 0.05];
const DARK: [f32; 3] = [0.28, 0.02, 0.025];

/// One stain texture as three materials: fresh, a quarter minute old, a minute old.
type Stages = [Handle<StandardMaterial>; 3];

/// Texture tint of ageing stage `i`: bright red, then darker, then dark rusty brown.
fn tint(i: usize) -> Color {
    [
        Color::srgb(1.0, 0.6, 0.6),
        Color::srgb(0.8, 0.85, 0.9),
        Color::srgb(0.58, 1.0, 1.0),
    ][i]
}

pub struct GorePlugin;

impl Plugin for GorePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup).add_systems(
            Update,
            (spray, fly, mist, drips, pools, bleed, age)
                .chain()
                .run_if(resource_exists::<Gore>)
                .run_if(resource_exists::<MapCollision>),
        );
    }
}

/// Shared meshes, materials and the stain queue.
#[derive(Resource)]
struct Gore {
    quad: Handle<Mesh>,
    drop: Handle<StandardMaterial>,
    mist: Handle<StandardMaterial>,
    drip: Stages,
    /// Round splatters, and ones whose spikes point along +X (oblique hits).
    round: Vec<Stages>,
    forward: Vec<Stages>,
    pools: Vec<Stages>,
    stains: VecDeque<Entity>,
    seed: u32,
    /// Stains made so far (cycles the lift), new stains left this frame, drips alive.
    made: u32,
    budget: u32,
    drips: usize,
    /// Seconds of the current frame, for stamping stains.
    now: f32,
}

/// A blood droplet in flight; `size` is its radius (m).
#[derive(Component)]
struct Droplet {
    vel: Vec3,
    size: f32,
    age: f32,
}

/// A puff of blood mist: grows and shrinks over `life`.
#[derive(Component)]
struct Mist {
    vel: Vec3,
    size: f32,
    age: f32,
    life: f32,
}

/// A drip running down a wall from `top` along `down` (unit, on the wall), `len` of `max` metres long.
#[derive(Component)]
struct Drip {
    top: Vec3,
    down: Vec3,
    normal: Vec3,
    len: f32,
    max: f32,
    width: f32,
}

/// A pool under a corpse: `age` runs from `-POOL_DELAY`, `radius` is its final radius (m).
#[derive(Component)]
struct Pool {
    age: f32,
    radius: f32,
    aspect: f32,
}

/// A stain that darkens with age: when it was made, its current stage and its materials.
#[derive(Component)]
struct Stain {
    born: f32,
    stage: usize,
    set: Stages,
}

fn setup(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let mut stages = |image: Image, bias: f32| -> Stages {
        let tex = images.add(image);
        std::array::from_fn(|i| {
            materials.add(StandardMaterial {
                base_color: tint(i),
                base_color_texture: Some(tex.clone()),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: None,
                depth_bias: bias,
                ..default()
            })
        })
    };
    let (drip, round, forward, pools) = (
        stages(drip_image(), 2.0),
        (1..=4)
            .map(|i| stages(splat_image(0x51AB_0000 + i, false), 2.0))
            .collect(),
        (1..=3)
            .map(|i| stages(splat_image(0xF04D_0000 + i, true), 2.0))
            .collect(),
        (1..=2)
            .map(|i| stages(pool_image(0x9001_0000 + i), 1.0))
            .collect(),
    );
    // Droplets and mist live too briefly to age: one fresh material each.
    let [drop, _, _] = stages(streak_image(), 0.0);
    let [mist, _, _] = stages(mist_image(), 0.0);
    commands.insert_resource(Gore {
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
        drop,
        mist,
        drip,
        round,
        forward,
        pools,
        stains: VecDeque::new(),
        seed: 0xB10D_5EED,
        made: 0,
        budget: 0,
        drips: 0,
        now: 0.0,
    });
}

impl Gore {
    fn rnd(&mut self) -> f32 {
        rnd(&mut self.seed)
    }

    fn pick(&mut self, set: fn(&Gore) -> &Vec<Stages>) -> Stages {
        let n = set(self).len();
        let i = (self.rnd() * n as f32) as usize % n;
        set(self)[i].clone()
    }

    /// Remembers a stain; the oldest past [`MAX_STAINS`] is despawned.
    fn keep(&mut self, commands: &mut Commands, e: Entity) {
        self.stains.push_back(e);
        if self.stains.len() > MAX_STAINS
            && let Some(old) = self.stains.pop_front()
        {
            commands.entity(old).try_despawn();
        }
    }

    /// A splatter of `size` metres at `at` on a surface with `normal` (facing the viewer) from
    /// blood arriving with velocity `vel`: oblique hits stretch along the surface component of
    /// `vel` and get the spiked-forward textures; a big one on a wall may run down as a drip.
    fn stain(&mut self, commands: &mut Commands, at: Vec3, normal: Vec3, vel: Vec3, size: f32) {
        if self.budget == 0 {
            return;
        }
        self.budget -= 1;
        let cos = (-vel.normalize_or_zero().dot(normal)).clamp(0.0, 1.0);
        let stretch = (1.0 / cos.max(0.4)).min(2.5);
        let along = (vel - normal * vel.dot(normal)).try_normalize();
        let x = along.unwrap_or_else(|| normal.any_orthonormal_vector());
        let mut rotation = Quat::from_mat3(&Mat3::from_cols(x, normal.cross(x), normal));
        let oblique = stretch > 1.45 && along.is_some();
        if !oblique && stretch < 1.15 {
            rotation *= Quat::from_rotation_z(self.rnd() * TAU);
        }
        let material = self.pick(if oblique {
            |g| &g.forward
        } else {
            |g| &g.round
        });
        let lift = LIFT + 0.0004 * (self.made % 16) as f32;
        self.made = self.made.wrapping_add(1);
        let e = commands
            .spawn((
                MapEntity,
                Mesh3d(self.quad.clone()),
                MeshMaterial3d(material[0].clone()),
                Stain {
                    born: self.now,
                    stage: 0,
                    set: material,
                },
                Transform {
                    translation: at + normal * lift,
                    rotation,
                    scale: Vec3::new(size * stretch, size, 1.0),
                },
            ))
            .id();
        self.keep(commands, e);
        if normal.y.abs() < 0.4
            && size > DRIP_MIN
            && self.drips < MAX_DRIPS
            && self.rnd() < DRIP_CHANCE
        {
            self.drips += 1;
            let side = normal.cross(Vec3::Y).normalize_or_zero();
            let drip = Drip {
                top: at + normal * (lift + 0.0005) + side * (self.rnd() - 0.5) * size * 0.6,
                down: (Vec3::NEG_Y + normal * normal.y).normalize_or(Vec3::NEG_Y),
                normal,
                len: 0.01,
                max: (0.12 + 0.5 * self.rnd()) * (size / 0.12).clamp(0.6, 1.6),
                width: 0.01 + size * 0.07,
            };
            let e = commands
                .spawn((
                    MapEntity,
                    Mesh3d(self.quad.clone()),
                    MeshMaterial3d(self.drip[0].clone()),
                    Stain {
                        born: self.now,
                        stage: 0,
                        set: self.drip.clone(),
                    },
                    drip.transform(),
                    drip,
                ))
                .id();
            self.keep(commands, e);
        }
    }
}

impl Drip {
    fn transform(&self) -> Transform {
        let up = -self.down;
        Transform {
            translation: self.top + self.down * (self.len / 2.0),
            rotation: Quat::from_mat3(&Mat3::from_cols(up.cross(self.normal), up, self.normal)),
            scale: Vec3::new(self.width, self.len, 1.0),
        }
    }
}

/// Unit vector a bit off `v` (`spread` 0 = `v`).
fn scatter(v: Vec3, spread: f32, seed: &mut u32) -> Vec3 {
    let r = Vec3::new(rnd(seed) - 0.5, rnd(seed) - 0.5, rnd(seed) - 0.5) * 2.0;
    (v + r * spread).normalize_or(v)
}

/// A droplet's long axis (its velocity) turned toward the camera: Y = `y`, Z as near the
/// direction to the camera as the axis allows.
fn facing(y: Vec3, to_cam: Vec3) -> Quat {
    let z = (to_cam - y * to_cam.dot(y))
        .try_normalize()
        .unwrap_or_else(|| y.any_orthonormal_vector());
    Quat::from_mat3(&Mat3::from_cols(y.cross(z), y, z))
}

fn spray(
    mut vfx: MessageReader<Vfx>,
    settings: Res<Settings>,
    col: Res<MapCollision>,
    mut gore: ResMut<Gore>,
    mut commands: Commands,
    live: Query<(), With<Droplet>>,
    fog: Query<(), With<Mist>>,
    time: Res<Time>,
) {
    let gore = &mut *gore;
    gore.now = time.elapsed_secs();
    gore.budget = STAINS_PER_FRAME;
    let (mut drops, mut puffs) = (live.iter().count(), fog.iter().count());
    for v in vfx.read() {
        let Vfx::Blood { point, dir, amount } = *v else {
            continue;
        };
        if !settings.realistic_blood {
            continue;
        }
        let d = dir.normalize_or(Vec3::Y);
        let k = (amount / SPRAY_UNIT).clamp(0.3, SPRAY_MAX);
        let speed_k = 0.6 + 0.4 * k.min(2.0);
        let n = ((12.0 + 14.0 * k) as usize).min(MAX_DROPS - drops.min(MAX_DROPS));
        drops += n;
        for _ in 0..n {
            // A fifth of the drops spatter back toward the shooter, slower.
            let back = gore.rnd() < 0.22;
            let (along, slow) = if back { (-d, 0.5) } else { (d, 1.0) };
            let g = &mut gore.seed;
            let vel = scatter(along + Vec3::Y * 0.25, 0.7, g)
                * (1.5 + 7.5 * rnd(g) * rnd(g).max(0.3))
                * speed_k
                * slow;
            let size = (0.008 + 0.022 * rnd(g).powi(2)) * (0.8 + 0.2 * k);
            commands.spawn((
                MapEntity,
                Mesh3d(gore.quad.clone()),
                MeshMaterial3d(gore.drop.clone()),
                Transform::from_translation(point + scatter(Vec3::ZERO, 1.0, g) * 0.06),
                Droplet {
                    vel,
                    size,
                    age: 0.0,
                },
            ));
        }
        for _ in 0..((2.0 + 2.0 * k) as usize).min(MAX_MIST - puffs.min(MAX_MIST)) {
            puffs += 1;
            let g = &mut gore.seed;
            commands.spawn((
                MapEntity,
                Mesh3d(gore.quad.clone()),
                MeshMaterial3d(gore.mist.clone()),
                Transform::from_translation(point + scatter(Vec3::ZERO, 1.0, g) * 0.1)
                    .with_scale(Vec3::ZERO),
                Mist {
                    vel: d * 1.5 + scatter(Vec3::ZERO, 1.0, g) * 0.8,
                    size: 0.12 + 0.12 * rnd(g),
                    age: 0.0,
                    life: 0.4 + 0.3 * rnd(g),
                },
            ));
        }
        // The spray leaving the body marks the wall behind it at once.
        if amount >= 12.0
            && let Some(h) = col.raycast(point, d, 2.5)
        {
            let size = (0.35 + 0.15 * k) * (0.8 + 0.4 * gore.rnd());
            gore.stain(&mut commands, h.point, h.normal, d * 8.0, size);
        }
    }
}

/// Droplets: gravity and drag, one ray per step; a hit leaves a stain sized by the droplet and
/// how fast it came.
fn fly(
    mut commands: Commands,
    time: Res<Time>,
    col: Res<MapCollision>,
    mut gore: ResMut<Gore>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut drops: Query<(Entity, &mut Transform, &mut Droplet)>,
) {
    let dt = time.delta_secs();
    let cam = camera.iter().next().map(|c| c.translation());
    for (e, mut t, mut d) in &mut drops {
        d.age += dt;
        d.vel.y -= GRAVITY * dt;
        d.vel *= 1.0 - DRAG * dt;
        let step = d.vel * dt;
        if d.age > DROP_LIFE || t.translation.y < -100.0 {
            commands.entity(e).despawn();
            continue;
        }
        if let Some(h) = col.raycast(t.translation, step, step.length()) {
            // Specks are mostly lost on the way: only some leave a mark.
            if d.size > 0.009 || gore.rnd() < 0.4 {
                let speed = d.vel.length();
                let size = d.size * (5.5 + 1.1 * speed) * (0.7 + 0.6 * gore.rnd());
                gore.stain(&mut commands, h.point, h.normal, d.vel, size);
            }
            commands.entity(e).despawn();
            continue;
        }
        t.translation += step;
        let long = 2.0 * d.size + step.length();
        t.scale = Vec3::new(2.0 * d.size, long, 1.0);
        if let (Some(cam), Some(y)) = (cam, d.vel.try_normalize()) {
            t.rotation = facing(y, cam - t.translation);
        }
    }
}

fn mist(
    mut commands: Commands,
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut puffs: Query<(Entity, &mut Transform, &mut Mist)>,
) {
    let dt = time.delta_secs();
    let face = camera.iter().next().map(|c| c.rotation());
    for (e, mut t, mut m) in &mut puffs {
        m.age += dt;
        let f = m.age / m.life;
        if f >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        m.vel *= 1.0 - 3.0 * dt;
        t.translation += m.vel * dt;
        t.scale = Vec3::splat(m.size * 4.0 * f * (1.0 - f));
        if let Some(r) = face {
            t.rotation = r;
        }
    }
}

/// Drips creep down the wall, slowing as they near their length.
fn drips(
    mut commands: Commands,
    time: Res<Time>,
    mut gore: ResMut<Gore>,
    mut running: Query<(Entity, &mut Transform, &mut Drip)>,
) {
    let dt = time.delta_secs();
    gore.drips = running.iter().count();
    for (e, mut t, mut d) in &mut running {
        d.len += (d.max - d.len) * 0.5 * dt;
        *t = d.transform();
        if d.max - d.len < 0.004 {
            commands.entity(e).remove::<Drip>();
        }
    }
}

/// A pool under every actor that dies, growing on the floor beneath.
fn pools(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    col: Res<MapCollision>,
    mut gore: ResMut<Gore>,
    dead: Query<&GlobalTransform, (Added<Dead>, With<Vitals>, Without<Npc>)>,
    mut growing: Query<(Entity, &mut Transform, &mut Pool)>,
) {
    let dt = time.delta_secs();
    for (e, mut t, mut p) in &mut growing {
        p.age += dt;
        let r = p.radius * (1.0 - (-p.age.max(0.0) / POOL_SPREAD).exp());
        // The blob fills 0.6 of the quad.
        t.scale = Vec3::new(r * 2.0 / 0.6, r * 2.0 / 0.6 * p.aspect, 1.0);
        if p.age > POOL_SPREAD * 5.0 {
            commands.entity(e).remove::<Pool>();
        }
    }
    if !settings.realistic_blood {
        return;
    }
    for g in &dead {
        let Some(h) = col.raycast(g.translation() + Vec3::Y * 0.5, Vec3::NEG_Y, 3.0) else {
            continue;
        };
        debug!("pool at {:?}", h.point);
        let (roll, aspect, radius) = (
            gore.rnd() * TAU,
            0.85 + 0.3 * gore.rnd(),
            0.5 + 0.25 * gore.rnd(),
        );
        let set = gore.pick(|g| &g.pools);
        let e = commands
            .spawn((
                MapEntity,
                Mesh3d(gore.quad.clone()),
                MeshMaterial3d(set[0].clone()),
                Stain {
                    born: gore.now,
                    stage: 0,
                    set,
                },
                Transform {
                    translation: h.point + h.normal * (LIFT + 0.0002),
                    rotation: Quat::from_rotation_arc(Vec3::Z, h.normal)
                        * Quat::from_rotation_z(roll),
                    scale: Vec3::ZERO,
                },
                Pool {
                    age: -POOL_DELAY,
                    radius,
                    aspect,
                },
            ))
            .id();
        gore.keep(&mut commands, e);
    }
}

/// Stains darken through [`tint`]'s stages as they age (checked twice a second).
fn age(
    time: Res<Time>,
    mut next: Local<f32>,
    mut stains: Query<(&mut Stain, &mut MeshMaterial3d<StandardMaterial>)>,
) {
    let now = time.elapsed_secs();
    if now < *next && *next - now < 1.0 {
        return;
    }
    *next = now + 0.5;
    for (mut s, mut m) in &mut stains {
        let stage = AGE.iter().filter(|&&a| now - s.born > a).count();
        if stage != s.stage {
            s.stage = stage;
            m.0 = s.set[stage].clone();
        }
    }
}

/// Hurt actors (not the player: the screen shows that) shed drops that land where they walked.
fn bleed(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    mut gore: ResMut<Gore>,
    live: Query<(), With<Droplet>>,
    hurt: Query<(&GlobalTransform, &Vitals), (Without<Dead>, Without<Player>, Without<Npc>)>,
) {
    if !settings.realistic_blood {
        return;
    }
    let gore = &mut *gore;
    let mut room = MAX_DROPS.saturating_sub(live.iter().count());
    for (g, v) in &hurt {
        let frac = v.hp / v.max_hp.max(1.0);
        let rate = ((BLEED_BELOW - frac) / BLEED_BELOW).clamp(0.0, 1.0) * BLEED_RATE;
        if room == 0 || gore.rnd() >= rate * time.delta_secs() {
            continue;
        }
        room -= 1;
        let s = &mut gore.seed;
        let off = scatter(Vec3::ZERO, 1.0, s) * 0.2;
        commands.spawn((
            MapEntity,
            Mesh3d(gore.quad.clone()),
            MeshMaterial3d(gore.drop.clone()),
            Transform::from_translation(
                g.translation() + Vec3::new(off.x, 0.7 + 0.6 * rnd(s), off.z),
            ),
            Droplet {
                vel: off,
                size: 0.007 + 0.006 * rnd(s),
                age: 0.0,
            },
        ));
    }
}

// ---------------------------------------------------------------------------------------------
// Procedural textures (sRGB RGBA8, the blood colour kept under transparent texels so that
// filtering leaves no dark fringe)

pub(crate) fn smooth(a: f32, b: f32, x: f32) -> f32 {
    let t = ((x - a) / (b - a)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// An RGBA image from `f(u, v)` (0..1 from the top left) returning `[r, g, b, a]` in 0..=1.
pub(crate) fn paint(w: u32, h: u32, f: impl Fn(f32, f32) -> [f32; 4]) -> Image {
    let mut data = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let c = f((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            data.extend(c.map(|v| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8));
        }
    }
    Image::new(
        Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    )
}

/// A splatter: a wobbly core, thin tapering spikes and satellite droplets, darker and glossy in
/// the middle. `forward` points the spikes along +X (the direction of an oblique hit).
pub(crate) fn splat_image(seed: u32, forward: bool) -> Image {
    let mut s = seed | 1;
    let mut r = || rnd(&mut s);
    let (ph1, ph2) = (r() * TAU, r() * TAU);
    let core = 0.2 + 0.06 * r();
    // Spikes: axis, half width and length.
    let spikes: Vec<(Vec2, f32, f32)> = (0..16)
        .map(|_| {
            let a = if forward {
                (r() - 0.5) * 1.5
            } else {
                r() * TAU
            };
            let len = ((0.12 + 0.5 * r() * r()) * if forward { 1.2 } else { 1.0 }).min(0.62);
            (Vec2::from_angle(a), 0.015 + 0.03 * r(), len)
        })
        .collect();
    // Satellite droplets beyond the tips of most spikes: centre and radius.
    let dots: Vec<(Vec2, f32)> = spikes
        .iter()
        .filter_map(|&(d, _, len)| {
            (r() < 0.7).then(|| {
                (
                    d * (core + len + 0.05 + 0.2 * r()).min(0.95),
                    0.012 + 0.03 * r(),
                )
            })
        })
        .collect();
    paint(128, 128, |u, v| {
        let p = Vec2::new(u - 0.5, v - 0.5) * 2.0;
        let (rad, th) = (p.length(), p.y.atan2(p.x));
        let edge = core * (1.0 + 0.16 * (3.0 * th + ph1).sin() + 0.1 * (5.0 * th + ph2).sin());
        let mut a = smooth(edge + 0.03, edge - 0.03, rad);
        for &(d, w, len) in &spikes {
            let along = p.dot(d);
            let t = ((along - core * 0.6) / (len + core * 0.4)).max(0.0);
            if along > 0.0 && t < 1.0 {
                let half = w * (1.0 - t);
                a = a.max(smooth(
                    half + 0.015,
                    (half - 0.015).max(0.0),
                    p.perp_dot(d).abs(),
                ));
            }
        }
        for &(c, rr) in &dots {
            a = a.max(smooth(rr + 0.012, rr, (p - c).length()));
        }
        let depth = (1.0 - rad / edge).clamp(0.0, 1.0);
        let gloss = (-((p.x + 0.07).powi(2) + (p.y + 0.09).powi(2)) / 0.006).exp()
            * 0.45
            * smooth(edge, edge * 0.6, rad);
        let c = mix(
            mix(FRESH, DARK, depth.powf(1.5) * 0.75),
            [0.95, 0.5, 0.5],
            gloss,
        );
        [c[0], c[1], c[2], a * (0.82 + 0.12 * depth)]
    })
}

/// A droplet in flight, long axis up the image: round head, tapering tail, a highlight.
fn streak_image() -> Image {
    paint(16, 64, |u, v| {
        let cx = (u - 0.5).abs() * 2.0;
        let cap = if v < 0.12 {
            (1.0 - ((0.12 - v) / 0.12).powi(2)).sqrt()
        } else {
            1.0
        };
        let w = (1.0 - 0.75 * v) * cap;
        let a = smooth(w, w - 0.4, cx) * (1.0 - 0.6 * v);
        let c = mix(
            FRESH,
            [0.9, 0.35, 0.35],
            smooth(0.4, 0.0, cx) * (1.0 - v) * 0.6,
        );
        [c[0], c[1], c[2], a]
    })
}

fn mist_image() -> Image {
    paint(32, 32, |u, v| {
        let d = ((u - 0.5).powi(2) + (v - 0.5).powi(2)).sqrt() * 2.0;
        [0.55, 0.03, 0.04, (1.0 - d).max(0.0).powi(2) * 0.5]
    })
}

/// A drip stretched to its length: faint at the top, a thin line, a teardrop bead at the bottom.
fn drip_image() -> Image {
    paint(8, 64, |u, v| {
        let cx = (u - 0.5).abs() * 2.0;
        let w = if v < 0.85 {
            0.35
        } else {
            let t = (v - 0.92) / 0.08;
            0.35 + 0.65 * (1.0 - t * t).max(0.0).sqrt()
        };
        let a = smooth(w, w - 0.3, cx) * smooth(0.0, 0.1, v);
        let c = mix(FRESH, DARK, 0.3 * (1.0 - cx));
        let c = mix(
            c,
            [0.95, 0.5, 0.5],
            smooth(0.5, 0.0, cx) * smooth(0.9, 0.95, v) * 0.5,
        );
        [c[0], c[1], c[2], a * 0.92]
    })
}

/// A pool: a lumpy blob with a few tongues and satellite drops, lighter and thin at the rim, dark
/// and glossy in the middle. Its nominal radius is 0.6 of the half-width.
fn pool_image(seed: u32) -> Image {
    let mut s = seed | 1;
    let mut r = || rnd(&mut s);
    let ph: [f32; 4] = std::array::from_fn(|_| r() * TAU);
    // Tongues: angle, angular half width, extra length.
    let tongues: Vec<(f32, f32, f32)> = (0..5)
        .map(|_| (r() * TAU, 0.2 + 0.2 * r(), 0.06 + 0.12 * r()))
        .collect();
    let dots: Vec<(Vec2, f32)> = (0..5)
        .map(|_| {
            (
                Vec2::from_angle(r() * TAU) * (0.72 + 0.2 * r()),
                0.015 + 0.03 * r(),
            )
        })
        .collect();
    paint(128, 128, |u, v| {
        let p = Vec2::new(u - 0.5, v - 0.5) * 2.0;
        let (rad, th) = (p.length(), p.y.atan2(p.x));
        let mut edge = 0.6
            * (1.0
                + 0.14 * (3.0 * th + ph[0]).sin()
                + 0.1 * (5.0 * th + ph[1]).sin()
                + 0.06 * (8.0 * th + ph[2]).sin()
                + 0.04 * (13.0 * th + ph[3]).sin());
        for &(a, w, len) in &tongues {
            let d = (th - a + std::f32::consts::PI).rem_euclid(TAU) - std::f32::consts::PI;
            edge += len * (-(d / w).powi(2)).exp();
        }
        let mut a = smooth(edge, edge - 0.05, rad);
        for &(c, rr) in &dots {
            a = a.max(smooth(rr + 0.01, rr, (p - c).length()));
        }
        let depth = (1.0 - rad / edge).clamp(0.0, 1.0);
        // Two soft highlights and a lighter meniscus along the rim.
        let spot = |x: f32, y: f32, k: f32| (-((p.x - x).powi(2) + (p.y - y).powi(2)) / k).exp();
        let gloss = spot(-0.2, -0.22, 0.04) * 0.4
            + spot(-0.16, -0.2, 0.004) * 0.6
            + spot(0.25, 0.15, 0.02) * 0.22
            + smooth(edge - 0.14, edge - 0.04, rad) * smooth(edge, edge - 0.04, rad) * 0.25;
        let c = mix(
            mix([0.45, 0.04, 0.04], [0.1, 0.004, 0.012], depth.powf(0.5)),
            [0.95, 0.55, 0.5],
            gloss.min(0.7),
        );
        [c[0], c[1], c[2], a * 0.94]
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn textures_and_basis() {
        // A splatter is solid in the middle and clear in the corners, forward or not.
        for forward in [false, true] {
            let img = splat_image(7, forward);
            let data = img.data.as_ref().unwrap();
            let alpha = |x: usize, y: usize| data[(y * 128 + x) * 4 + 3];
            assert!(alpha(64, 64) > 180 && alpha(0, 0) == 0, "{forward}");
        }
        // A droplet's quad has its long axis on the velocity and faces the camera.
        let v = Vec3::new(1.0, -2.0, 0.5).normalize();
        let to_cam = Vec3::new(0.0, 3.0, 5.0);
        let q = facing(v, to_cam);
        assert!((q * Vec3::Y - v).length() < 1e-4);
        assert!((q * Vec3::Z).dot(to_cam) > 0.0);
    }
}
