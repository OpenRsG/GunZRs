//! Combat: `Fire` -> hitscan (guns) -> `Damage` -> armour/health -> `Dead`, kill tally and
//! hit/muzzle effects; blades are `melee.rs`, rockets/grenades `projectile.rs`. Rules marked
//! *inferred* are not in the retail data (the executable is packed): docs/formats.md "Combat".

use crate::{
    actor::ActorData,
    ani,
    anim::Loop,
    col::MapCollision,
    effect::{self, EffectDef, FxAssets, Loader},
    elu,
    game::{
        Bot, Damage, Dead, Fire, Impact, Killed, Player, Protected, Push, Score, Team, Vitals,
        friendly,
    },
    item::WeaponKind,
    level::Level,
    mrs::Vfs,
    projectile, view,
};
use bevy::{image::ImageSampler, prelude::*};
use std::{
    collections::HashMap,
    f32::consts::{FRAC_PI_2, TAU},
    sync::{Arc, OnceLock},
};

/// Seconds a dead actor waits before the actor controller respawns it.
const RESPAWN_SECS: f32 = 5.0;
/// Hit capsule of an actor (feet at the transform origin), metres.
pub const HIT_RADIUS: f32 = 0.35;
pub const HIT_HEIGHT: f32 = 1.8;
/// Eye height above the feet (same as the actor controller's).
pub(crate) const EYE: f32 = 1.55;
/// Hitscan reach (the retail guns have no range attribute; maps are smaller than this).
const GUN_RANGE: f32 = 200.0;
/// Pellets per shotgun shot. *Inferred*: zitem.xml has no pellet count, but a 13 damage shotgun
/// with a 950 ms delay only makes sense per pellet.
const SHOTGUN_PELLETS: u32 = 12;
/// Base spread cone radius per metre per `ctrl_ability` point. *Inferred*: `ctrl_ability` grows
/// with inaccuracy (pistol 10, rifle 15, dual SMG 75, shotgun 60).
const SPREAD_PER_CTRL: f32 = 0.001;
/// Pellets of a shotgun scatter this many times wider than a single bullet of the same
/// `ctrl_ability` (*inferred*): 0.12/m = a 1.2 m radius at 10 m, so a shotgun is a close-range gun.
const PELLET_SPREAD: f32 = 2.0;
/// Spread multipliers on top of the base (*inferred*): +`RUN_SPREAD` at full running speed,
/// +`AIR_SPREAD` in the air, +`HEAT_SPREAD` at full heat. Heat builds `HEAT_PER_CTRL` per
/// `ctrl_ability` point with every shot and cools `HEAT_COOL` per second, so a rifle (ctrl 15,
/// 13 shots/s) saturates in about a second while a pistol (ctrl 10, 5 shots/s) never heats up.
const RUN_SPREAD: f32 = 1.5;
const AIR_SPREAD: f32 = 1.0;
const HEAT_SPREAD: f32 = 2.0;
const HEAT_PER_CTRL: f32 = 0.01;
const HEAT_COOL: f32 = 1.0;
/// Vertical speed (m/s) above which an actor counts as airborne for spread (*inferred*; the
/// controller's grounded flag is private, and stairs also trip this briefly).
const AIR_SPEED: f32 = 3.0;
/// Rate (1/s) at which an actor's sampled running/airborne state follows its real motion.
const SPREAD_FOLLOW: f32 = 10.0;
/// Fraction of a blast's damage the shooter takes from their own rocket or grenade. *Inferred*:
/// nothing in the data says whether GunZ hurts you; half keeps a rocket jump costly but not fatal.
pub(crate) const SELF_BLAST: f32 = 0.5;
/// Seconds after switching to a weapon before it may fire or swing (*inferred*; the data has no
/// draw time and there is no draw clip). Read by the actor controller.
pub const SWITCH_DELAY: f32 = 0.3;
/// zeffect.xml knockback is read as cm/s of horizontal velocity. *Inferred*.
const KNOCKBACK_UNIT: f32 = 0.01;
pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Vfx>()
            .add_systems(Startup, (init_fx, projectile::init))
            .add_systems(
                Update,
                (
                    track_spread,
                    resolve_fire,
                    projectile::launch,
                    projectile::fly,
                    projectile::regen,
                    apply_damage,
                    spawn_elu_fx,
                    spawn_sprites,
                    update_sprites,
                    projectile::flash_overlay,
                    expire,
                )
                    .chain(),
            );
    }
}

// ---------------------------------------------------------------------------------------------
// Pure rules

/// Fraction of a hit that goes to health even while armour remains (*inferred*; the retail data
/// has no piercing ratio): blades cut through armour, shotgun pellets are mostly stopped by it.
fn piercing(k: WeaponKind) -> f32 {
    use WeaponKind::*;
    match k {
        Katana | Dagger | DoubleKatana => 0.7,
        Shotgun => 0.3,
        Rifle | MachineGun => 0.6,
        _ => 0.5,
    }
}

/// `amount` splits by `pierce`: that share hurts health, the rest hurts armour; whatever armour
/// cannot take falls through to health.
fn absorb(v: &mut Vitals, amount: f32, pierce: f32) {
    let to_ap = (amount * (1.0 - pierce)).min(v.ap);
    v.ap -= to_ap;
    v.hp -= amount - to_ap;
}

/// What an actor's weapons know about its motion: how fast it runs, how airborne it is (both
/// 0..1, smoothed) and how hot its trigger finger is (0..1, builds per shot, cools with time).
#[derive(Component, Default)]
struct Spread {
    last: Vec3,
    run: f32,
    air: f32,
    heat: f32,
    /// Time of the latest shot (heat cools from there).
    shot: f32,
}

impl Spread {
    /// Cone radius per metre for a weapon with `ctrl_ability` `ctrl` at time `now`.
    fn cone(&self, ctrl: f32, now: f32) -> f32 {
        let heat = (self.heat - (now - self.shot) * HEAT_COOL).max(0.0);
        ctrl * SPREAD_PER_CTRL
            * (1.0 + RUN_SPREAD * self.run + AIR_SPREAD * self.air + HEAT_SPREAD * heat)
    }

    fn shoot(&mut self, ctrl: f32, now: f32) {
        self.heat =
            ((self.heat - (now - self.shot) * HEAT_COOL).max(0.0) + ctrl * HEAT_PER_CTRL).min(1.0);
        self.shot = now;
    }
}

/// The hand of a dual-wielded gun that took an actor's latest shot (`true` = left); the shots
/// alternate, right first. Read by the actor controller to animate the matching arm.
#[derive(Component, Default)]
pub struct Hand(pub bool);

/// Samples every actor's speed into its [`Spread`].
fn track_spread(
    mut commands: Commands,
    time: Res<Time>,
    mut actors: Query<(Entity, &GlobalTransform, Option<&mut Spread>), With<Vitals>>,
) {
    let dt = time.delta_secs();
    for (e, g, s) in &mut actors {
        let p = g.translation();
        let Some(mut s) = s else {
            commands.entity(e).insert(Spread {
                last: p,
                ..default()
            });
            continue;
        };
        if dt > 0.0 {
            let v = (p - s.last) / dt;
            let k = 1.0 - (-SPREAD_FOLLOW * dt).exp();
            let run = (Vec2::new(v.x, v.z).length() / crate::actor::RUN).min(1.0);
            let air = f32::from(v.y.abs() > AIR_SPEED);
            (s.run, s.air) = (s.run + (run - s.run) * k, s.air + (air - s.air) * k);
        }
        s.last = p;
    }
}

/// Distance along the unit ray `o + t d` to the vertical capsule standing on `f`;
/// `Some(0)` when `o` is inside.
pub(crate) fn ray_capsule(o: Vec3, d: Vec3, f: Vec3) -> Option<f32> {
    let (r, lo, hi) = (HIT_RADIUS, f.y + HIT_RADIUS, f.y + HIT_HEIGHT - HIT_RADIUS);
    let sphere = |c: Vec3| {
        let m = o - c;
        let (b, c) = (m.dot(d), m.length_squared() - r * r);
        if c <= 0.0 {
            return Some(0.0);
        }
        let disc = b * b - c;
        (b < 0.0 && disc >= 0.0).then(|| -b - disc.sqrt())
    };
    let mut t = [Vec3::new(f.x, lo, f.z), Vec3::new(f.x, hi, f.z)]
        .into_iter()
        .filter_map(sphere)
        .fold(f32::MAX, f32::min);
    let (m, dx) = (Vec2::new(o.x - f.x, o.z - f.z), Vec2::new(d.x, d.z));
    let (a, b, c) = (dx.length_squared(), m.dot(dx), m.length_squared() - r * r);
    if c <= 0.0 && (lo..=hi).contains(&o.y) {
        t = 0.0;
    } else if a > 1e-8 && b * b - a * c >= 0.0 {
        let s = (-b - (b * b - a * c).sqrt()) / a;
        if s >= 0.0 && (lo..=hi).contains(&(o.y + d.y * s)) {
            t = t.min(s);
        }
    }
    (t < f32::MAX).then_some(t)
}

/// xorshift32 in `[0, 1)`.
pub(crate) fn rnd(s: &mut u32) -> f32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    (*s >> 8) as f32 / (1u32 << 24) as f32
}

/// `dir` perturbed uniformly inside a cone of radius `spread` per metre.
fn jitter(dir: Vec3, spread: f32, s: &mut u32) -> Vec3 {
    let u = dir.any_orthonormal_vector();
    let v = dir.cross(u);
    let (a, r) = (rnd(s) * TAU, rnd(s).sqrt() * spread);
    (dir + u * a.cos() * r + v * a.sin() * r).normalize()
}

/// Yaw (Intent convention: 0 = -Z) of a direction.
pub(crate) fn yaw_of(d: Vec3) -> f32 {
    (-d.x).atan2(-d.z)
}

pub(crate) fn is_melee(k: WeaponKind) -> bool {
    matches!(
        k,
        WeaponKind::Katana | WeaponKind::Dagger | WeaponKind::DoubleKatana
    )
}

fn is_gun(k: WeaponKind) -> bool {
    use WeaponKind::*;
    matches!(
        k,
        Pistol | PistolX2 | Revolver | RevolverX2 | Smg | SmgX2 | Shotgun | MachineGun | Rifle
    )
}

fn muzzle_effect(k: WeaponKind) -> Option<&'static str> {
    use WeaponKind::*;
    match k {
        Pistol | PistolX2 | Revolver | RevolverX2 => Some("flame_pistol"),
        Smg | SmgX2 | Rifle => Some("flame_rifle"),
        MachineGun => Some("flame_mg"),
        Shotgun => Some("flame_shotgun"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------------------------
// Effects

/// Visual effect requests produced by combat and consumed by the two effect systems.
#[derive(Message)]
pub(crate) enum Vfx {
    /// Muzzle flash `name` pointing along `dir` at the shooter's `muzzle_flash` weapon nodes
    /// (else at `fallback`).
    Muzzle {
        shooter: Entity,
        name: &'static str,
        dir: Vec3,
        fallback: Vec3,
        /// Dual guns: only the muzzle of this hand flashes (`true` = left).
        hand: Option<bool>,
    },
    /// Effect-list effect `name` at `at`.
    Elu {
        name: &'static str,
        at: Transform,
    },
    Blood {
        point: Vec3,
        dir: Vec3,
    },
    Spark {
        point: Vec3,
        normal: Vec3,
    },
    /// Grey smoke cloud of `radius` metres lasting `secs` (smoke grenade).
    Smoke {
        point: Vec3,
        radius: f32,
        secs: f32,
    },
    /// Effect-list effect `name` at `at`, its flat sprite turned toward the camera.
    Facing {
        name: &'static str,
        at: Vec3,
        /// Streak axis (the sprite's local Y) to rotate about instead of a full billboard.
        axis: Option<Vec3>,
        scale: f32,
    },
}

struct Cached {
    elu: Arc<elu::Elu>,
    ani: Option<(Arc<ani::Ani>, Loop)>,
    life: f32,
}

/// The effect loader borrows its VFS for as long as it lives, so combat mounts its own.
static EFFECT_VFS: OnceLock<Vfs> = OnceLock::new();

#[derive(Resource)]
pub(crate) struct Fx {
    vfs: &'static Vfs,
    loader: Loader<'static>,
    defs: Vec<EffectDef>,
    cache: HashMap<&'static str, Option<Cached>>,
    /// zeffect.xml `id` -> knockback.
    knockback: HashMap<u32, f32>,
    quad: Handle<Mesh>,
    blood: Vec<Handle<Image>>,
    smoke: Handle<Image>,
    /// Parsed non-effect ELUs (grenade models) by VFS path.
    models: HashMap<String, Arc<elu::Elu>>,
    spark: Handle<Image>,
}

fn init_fx(
    mut commands: Commands,
    level: Res<Level>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
) {
    let root = level
        .vfs
        .archives
        .iter()
        .map(|a| &a.path)
        .min_by_key(|p| p.components().count())
        .and_then(|p| p.parent())
        .expect("no archives mounted");
    let vfs = EFFECT_VFS.get_or_init(|| {
        Vfs::mount(root).unwrap_or_else(|e| panic!("mount {}: {e}", root.display()))
    });
    let mut texture = |name: &str| {
        let bytes = vfs
            .read(&format!("sfx/{name}"))
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        let ext = name.rsplit('.').next().unwrap();
        let image = view::decode(&bytes, ext, true, ImageSampler::linear())
            .unwrap_or_else(|| panic!("{name}: undecodable"));
        images.add(image)
    };
    let zeffect = String::from_utf8(
        vfs.read("system/zeffect.xml")
            .unwrap_or_else(|e| panic!("{e}")),
    )
    .expect("zeffect.xml is not UTF-8");
    let zeffect = roxmltree::Document::parse(zeffect.trim_start_matches('\u{feff}'))
        .unwrap_or_else(|e| panic!("zeffect.xml: {e}"));
    let knockback = zeffect
        .descendants()
        .filter(|n| n.has_tag_name("EFFECT"))
        .filter_map(|n| {
            Some((
                n.attribute("id")?.parse().ok()?,
                n.attribute("knockback")?.parse().ok()?,
            ))
        })
        .collect();
    commands.insert_resource(Fx {
        vfs,
        loader: Loader::new(vfs, "sfx/"),
        defs: {
            let mut defs = effect::load_list(vfs).unwrap_or_else(|e| panic!("{e}"));
            // `rocket_smoke_effect` is commented out of effect_list.xml; its ELU is the trail.
            defs.push(EffectDef {
                name: "rocket_smoke".into(),
                model: "sfx/ef_rocket_smoke.elu".into(),
                animation: Some(("sfx/ef_rocket_smoke.elu.ani".into(), false)),
                particle: None,
            });
            defs
        },
        cache: HashMap::new(),
        knockback,
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
        blood: (1..=5)
            .map(|i| texture(&format!("blood0{i}.tga")))
            .collect(),
        spark: texture("ef_gz_spark.bmp"),
        smoke: texture("smoke01.tga"),
        models: HashMap::new(),
    });
}

impl Fx {
    /// Horizontal knockback in m/s of a hit by `w`: its zeffect.xml `knockback` (`effect_id`).
    pub(crate) fn knockback_of(&self, w: &crate::item::Weapon) -> f32 {
        w.effect_id
            .and_then(|id| self.knockback.get(&id))
            .map_or(0.0, |k| k * KNOCKBACK_UNIT)
    }

    /// Spawns effect-list effect `name` at `at`; it despawns itself when its animation ends.
    pub(crate) fn spawn(
        &mut self,
        name: &'static str,
        at: Transform,
        assets: &mut FxAssets,
        commands: &mut Commands,
    ) {
        let (vfs, defs) = (self.vfs, &self.defs);
        let cached = self.cache.entry(name).or_insert_with(|| {
            let Some(def) = effect::find(defs, name) else {
                warn!("effect {name} not in effect_list.xml");
                return None;
            };
            let load = || -> std::io::Result<Cached> {
                let elu = Arc::new(elu::load(&vfs.read(&def.model)?)?);
                let ani = match &def.animation {
                    Some((p, looping)) => Some((
                        Arc::new(ani::load(&vfs.read(p)?)?),
                        if *looping { Loop::Wrap } else { Loop::Hold },
                    )),
                    None => None,
                };
                let life = ani
                    .as_ref()
                    .map_or(0.5, |(a, _)| a.max_frame as f32 / 160.0 + 0.1);
                Ok(Cached { elu, ani, life })
            };
            load().map_err(|e| warn!("effect {name}: {e}")).ok()
        });
        let Some(c) = cached else { return };
        let model = self
            .loader
            .spawn(assets, commands, "sfx/", &c.elu, c.ani.clone(), at);
        commands.entity(model.root).insert(Lifetime(c.life));
    }

    /// Spawns the rigid ELU model at VFS `path` (textures from its own directory) at `at`.
    pub(crate) fn model(
        &mut self,
        path: &str,
        at: Transform,
        assets: &mut FxAssets,
        commands: &mut Commands,
    ) -> Entity {
        let vfs = self.vfs;
        let elu = self
            .models
            .entry(path.to_owned())
            .or_insert_with(|| {
                Arc::new(
                    vfs.read(path)
                        .and_then(|b| elu::load(&b))
                        .unwrap_or_else(|e| panic!("{path}: {e}")),
                )
            })
            .clone();
        let dir = path.rsplit_once('/').map_or("", |(d, _)| d);
        self.loader
            .spawn(assets, commands, &format!("{dir}/"), &elu, None, at)
            .root
    }
}

#[derive(Component)]
pub(crate) struct Lifetime(pub f32);

fn expire(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &mut Lifetime)>) {
    for (e, mut l) in &mut q {
        l.0 -= time.delta_secs();
        if l.0 <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

fn spawn_elu_fx(
    mut vfx: MessageReader<Vfx>,
    mut fx: ResMut<Fx>,
    mut assets: FxAssets,
    mut commands: Commands,
    children: Query<&Children>,
    names: Query<&Name>,
    globals: Query<&GlobalTransform>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    parents: Query<&ChildOf>,
    visibility: Query<&Visibility>,
) {
    for v in vfx.read() {
        match *v {
            Vfx::Elu { name, at } => fx.spawn(name, at, &mut assets, &mut commands),
            Vfx::Facing {
                name,
                at,
                axis,
                scale,
            } => {
                // Effect ELUs are flat sprites authored facing +Z; turn that side to the camera
                // (about `axis`, the sprite's local Y, when it is a streak).
                let eye = camera
                    .iter()
                    .next()
                    .map_or(at + Vec3::Z, |c| c.translation());
                let to = (eye - at).normalize_or(Vec3::Z);
                let rotation = match axis {
                    None => Transform::IDENTITY.looking_to(-to, Vec3::Y).rotation,
                    Some(a) => {
                        let y = a.normalize();
                        let z = (to - y * to.dot(y))
                            .try_normalize()
                            .unwrap_or(y.any_orthonormal_vector());
                        Quat::from_mat3(&Mat3::from_cols(y.cross(z), y, z))
                    }
                };
                let at = Transform {
                    translation: at,
                    rotation,
                    scale: Vec3::splat(scale),
                };
                fx.spawn(name, at, &mut assets, &mut commands);
            }
            Vfx::Muzzle {
                shooter,
                name,
                dir,
                fallback,
                hand,
            } => {
                // Only the weapon in hand: the models of the other slots are hidden.
                let shown = |mut e: Entity| {
                    loop {
                        if visibility.get(e).is_ok_and(|v| *v == Visibility::Hidden) {
                            return false;
                        }
                        match parents.get(e) {
                            Ok(p) if e != shooter => e = p.parent(),
                            _ => return true,
                        }
                    }
                };
                // The `muzzle_flash` node's local +Y runs along the barrel in every weapon ELU
                // (observed); the flash ELU is a one-sided star around its local +Z (centre
                // 0.3 m along it), so its +Z goes to node -Y: front toward the shooter behind
                // the gun, centred on the muzzle.
                let mut at: Vec<(Vec3, Quat)> = children
                    .iter_descendants(shooter)
                    .filter(|e| {
                        names
                            .get(*e)
                            .is_ok_and(|n| n.as_str().eq_ignore_ascii_case("muzzle_flash"))
                    })
                    .filter(|e| shown(*e))
                    .filter_map(|e| globals.get(e).ok())
                    .map(|g| {
                        let r = g.rotation();
                        (
                            g.translation() + r * Vec3::Y * 0.15,
                            r * Quat::from_rotation_x(FRAC_PI_2),
                        )
                    })
                    .collect();
                if let (Some(left), Ok(me)) = (hand, globals.get(shooter)) {
                    let right = me.rotation() * Vec3::X;
                    let side = |p: &(Vec3, Quat)| (p.0 - me.translation()).dot(right);
                    at.sort_by(|a, b| side(a).total_cmp(&side(b)));
                    let one = if left { at.first() } else { at.last() };
                    at = one.copied().into_iter().collect();
                }
                if at.is_empty() {
                    let facing = Transform::IDENTITY.looking_to(dir, Vec3::Y).rotation;
                    at.push((fallback, facing));
                }
                for (translation, rotation) in at {
                    // Half size: the authored flash is a metre-wide star.
                    let at = Transform {
                        translation,
                        rotation,
                        scale: Vec3::splat(0.5),
                    };
                    fx.spawn(name, at, &mut assets, &mut commands);
                }
            }
            _ => {}
        }
    }
}

/// A camera-facing particle quad (blood, sparks) that grows and fades out.
#[derive(Component)]
struct Sprite {
    age: f32,
    life: f32,
    size: (f32, f32),
    vel: Vec3,
}

fn spawn_sprites(
    mut vfx: MessageReader<Vfx>,
    fx: Res<Fx>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
    mut seed: Local<u32>,
) {
    if *seed == 0 {
        *seed = 0x9E37_79B9;
    }
    let mut sprite = |tex: &Handle<Image>, color: Color, additive: bool, at: Vec3, s: Sprite| {
        let material = materials.add(StandardMaterial {
            base_color: color,
            base_color_texture: Some(tex.clone()),
            unlit: true,
            cull_mode: None,
            alpha_mode: if additive {
                AlphaMode::Add
            } else {
                AlphaMode::Blend
            },
            ..default()
        });
        commands.spawn((
            Mesh3d(fx.quad.clone()),
            MeshMaterial3d(material),
            Transform::from_translation(at).with_scale(Vec3::splat(s.size.0)),
            s,
        ));
    };
    for v in vfx.read() {
        match *v {
            Vfx::Blood { point, dir } => {
                for _ in 0..6 {
                    let tex = &fx.blood
                        [(rnd(&mut seed) * fx.blood.len() as f32) as usize % fx.blood.len()];
                    let kick = Vec3::new(
                        rnd(&mut seed) - 0.5,
                        rnd(&mut seed) - 0.3,
                        rnd(&mut seed) - 0.5,
                    );
                    let size = (0.15 + 0.1 * rnd(&mut seed), 0.5 + 0.3 * rnd(&mut seed));
                    sprite(
                        tex,
                        Color::linear_rgba(2.0, 0.3, 0.3, 1.0),
                        false,
                        point + kick * 0.15,
                        Sprite {
                            age: 0.0,
                            life: 0.7,
                            size,
                            vel: dir * 2.0 + kick * 2.0,
                        },
                    );
                }
            }
            Vfx::Spark { point, normal } => sprite(
                &fx.spark,
                Color::WHITE,
                true,
                point + normal * 0.05,
                Sprite {
                    age: 0.0,
                    life: 0.3,
                    size: (0.25, 0.6),
                    vel: normal * 0.3,
                },
            ),
            Vfx::Smoke {
                point,
                radius,
                secs,
            } => {
                for _ in 0..14 {
                    let off = Vec3::new(
                        rnd(&mut seed) - 0.5,
                        0.5 * rnd(&mut seed),
                        rnd(&mut seed) - 0.5,
                    ) * radius;
                    sprite(
                        &fx.smoke,
                        Color::srgb(0.75, 0.75, 0.75),
                        false,
                        point + off,
                        Sprite {
                            age: 0.0,
                            life: secs * (0.7 + 0.3 * rnd(&mut seed)),
                            size: (radius * 0.5, radius * 1.1),
                            vel: off * 0.05,
                        },
                    );
                }
            }
            _ => {}
        }
    }
}

fn update_sprites(
    mut commands: Commands,
    time: Res<Time>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut sprites: Query<(
        Entity,
        &mut Transform,
        &mut Sprite,
        &MeshMaterial3d<StandardMaterial>,
    )>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let face = camera.iter().next().map(|c| c.rotation());
    for (e, mut t, mut s, m) in &mut sprites {
        s.age += time.delta_secs();
        let f = s.age / s.life;
        if f >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        t.translation += s.vel * time.delta_secs();
        s.vel *= 1.0 - 3.0 * time.delta_secs();
        t.scale = Vec3::splat(s.size.0 + (s.size.1 - s.size.0) * f);
        if let Some(r) = face {
            t.rotation = r;
        }
        if let Some(mut mat) = materials.get_mut(&m.0) {
            mat.base_color = mat.base_color.with_alpha(1.0 - f);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Fire -> Damage

/// `Fire` of a gun: spread cone, then one hitscan ray per pellet. Blades are `melee.rs`,
/// rockets, grenades and consumables `projectile.rs`.
fn resolve_fire(
    mut fires: MessageReader<Fire>,
    mut damage: MessageWriter<Damage>,
    mut vfx: MessageWriter<Vfx>,
    mut impacts: MessageWriter<Impact>,
    mut commands: Commands,
    data: Res<ActorData>,
    fx: Res<Fx>,
    col: Res<MapCollision>,
    time: Res<Time>,
    actors: Query<
        (Entity, &GlobalTransform, Option<&Team>, Has<Bot>),
        (With<Vitals>, Without<Dead>),
    >,
    mut spreads: Query<(&mut Spread, Has<Player>)>,
    mut hands: Query<&mut Hand>,
    protected: Query<(), With<Protected>>,
    mut seed: Local<u32>,
) {
    if *seed == 0 {
        *seed = 0x2545_F491;
    }
    for f in fires.read() {
        let Some(w) = data.items.get(f.item).and_then(|i| i.weapon.as_ref()) else {
            continue;
        };
        let dir = f.dir.normalize_or_zero();
        if dir == Vec3::ZERO || !is_gun(w.kind) {
            continue;
        }
        let amount = w.damage as f32;
        let mine = actors
            .get(f.shooter)
            .ok()
            .map(|(_, _, t, b)| (t.copied(), b));
        let ally = |t: Option<&Team>, b: bool| mine.is_some_and(|m| friendly(m, (t.copied(), b)));
        let shooter_pos = actors
            .get(f.shooter)
            .map_or(f.origin, |(_, g, ..)| g.translation());
        let flat = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
        let push = fx.knockback_of(w);
        // (target, point, direction, damage) of every actor hit by this fire.
        let mut hits: Vec<(Entity, Vec3, Vec3, f32)> = Vec::new();
        let now = time.elapsed_secs();
        let ctrl = w.ctrl_ability.unwrap_or(0) as f32;
        let (mut spread, mut player) = (0.0, false);
        if let Ok((mut s, p)) = spreads.get_mut(f.shooter) {
            (spread, player) = (s.cone(ctrl, now), p);
            s.shoot(ctrl, now);
        }
        let pellets = if w.kind == WeaponKind::Shotgun {
            spread *= PELLET_SPREAD;
            SHOTGUN_PELLETS
        } else {
            1
        };
        let (mut landed, mut reach) = (0, f32::MAX);
        for _ in 0..pellets {
            let d = jitter(dir, spread, &mut seed);
            let wall = col.raycast(f.origin, d, GUN_RANGE);
            let mut best = wall.as_ref().map_or(GUN_RANGE, |h| h.distance);
            let mut who = None;
            for (e, g, team, bot) in &actors {
                if let Some(dist) = ray_capsule(f.origin, d, g.translation())
                    .filter(|t| e != f.shooter && *t < best && !ally(team, bot))
                {
                    (best, who) = (dist, Some(e));
                }
            }
            match (who, wall) {
                (Some(e), _) => {
                    landed += 1;
                    reach = reach.min(best);
                    // One damage message per target: the pellets of a shotgun add up.
                    match hits.iter_mut().find(|h| h.0 == e) {
                        Some(h) => h.3 += amount,
                        None => hits.push((e, f.origin + d * best, d, amount)),
                    }
                }
                (None, Some(h)) => {
                    vfx.write(Vfx::Spark {
                        point: h.point,
                        normal: h.normal,
                    });
                    impacts.write(Impact {
                        point: h.point,
                        normal: h.normal,
                        blade: false,
                    });
                }
                (None, None) => {}
            }
        }
        if player {
            info!(
                "t={now:.2} shot: item {} {:?} pellets {pellets} spread {spread:.4}/m ({:.2} m wide at 10 m), {landed} landed{}",
                f.item,
                w.kind,
                spread * 10.0,
                if landed > 0 {
                    format!(" (nearest {reach:.1} m)")
                } else {
                    String::new()
                }
            );
        }
        // Dual guns fire one hand at a time, right first.
        let hand = w.kind.dual().then(|| {
            let left = hands.get(f.shooter).is_ok_and(|h| !h.0);
            match hands.get_mut(f.shooter) {
                Ok(mut h) => h.0 = left,
                Err(_) => {
                    commands.entity(f.shooter).insert(Hand(left));
                }
            }
            left
        });
        if let Some(name) = muzzle_effect(w.kind) {
            vfx.write(Vfx::Muzzle {
                shooter: f.shooter,
                name,
                dir,
                fallback: shooter_pos + Vec3::Y * 1.3 + flat * 0.6,
                hand,
            });
        }
        for (target, point, d, amount) in hits {
            // Spawn protection absorbs the hit whole: no damage, blood or knockback.
            if protected.contains(target) {
                continue;
            }
            damage.write(Damage {
                target,
                attacker: f.shooter,
                amount,
                item: f.item,
                point,
                dir: d,
            });
            vfx.write(Vfx::Blood { point, dir: d });
            if push > 0.0 {
                commands.entity(target).insert(Push(flat * push));
            }
        }
    }
}

fn apply_damage(
    mut msgs: MessageReader<Damage>,
    data: Res<ActorData>,
    mut vitals: Query<&mut Vitals, Without<Dead>>,
    mut scores: Query<&mut Score>,
    protected: Query<(), With<Protected>>,
    names: Query<&Name>,
    sides: Query<(Option<&Team>, Has<Bot>)>,
    mut killed: MessageWriter<Killed>,
    time: Res<Time>,
    mut commands: Commands,
) {
    let name = |e| names.get(e).map_or("?", |n| n.as_str());
    for d in msgs.read() {
        // Your own blast hurts you; anybody else's side is protected from friendly fire.
        if d.attacker != d.target
            && let (Ok((ta, ba)), Ok((tt, bt))) = (sides.get(d.attacker), sides.get(d.target))
            && friendly((ta.copied(), ba), (tt.copied(), bt))
        {
            continue;
        }
        let Ok(mut v) = vitals.get_mut(d.target) else {
            continue;
        };
        if v.hp <= 0.0 || protected.contains(d.target) {
            continue;
        }
        let pierce = data
            .items
            .get(d.item)
            .and_then(|i| i.weapon.as_ref())
            .map_or(0.5, |w| piercing(w.kind));
        let (ap, hp) = (v.ap, v.hp);
        absorb(&mut v, d.amount, pierce);
        info!(
            "t={:.2} damage: {} -> {} {:.0} (pierce {pierce}; ap {ap:.0} -> {:.0}, hp {hp:.0} -> {:.0})",
            time.elapsed_secs(),
            name(d.attacker),
            name(d.target),
            d.amount,
            v.ap,
            v.hp.max(0.0)
        );
        if v.hp > 0.0 {
            continue;
        }
        v.hp = 0.0;
        commands.entity(d.target).insert(Dead {
            respawn: RESPAWN_SECS,
        });
        if let Ok(mut s) = scores.get_mut(d.target) {
            s.deaths += 1;
        }
        if d.attacker != d.target
            && let Ok(mut s) = scores.get_mut(d.attacker)
        {
            s.kills += 1;
        }
        info!(
            "t={:.2} kill: {} killed {}",
            time.elapsed_secs(),
            name(d.attacker),
            name(d.target)
        );
        killed.write(Killed {
            victim: d.target,
            killer: d.attacker,
            item: d.item,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capsule_and_armour() {
        let f = Vec3::new(0.0, 1.0, -5.0);
        // Straight at the body: surface at z = -5 + HIT_RADIUS.
        let t = ray_capsule(Vec3::new(0.0, 2.0, 0.0), Vec3::NEG_Z, f).unwrap();
        assert!((t - (5.0 - HIT_RADIUS)).abs() < 1e-4, "{t}");
        // Over the head, beside the body, behind the origin and straight down onto the head.
        assert!(ray_capsule(Vec3::new(0.0, 3.0, 0.0), Vec3::NEG_Z, f).is_none());
        assert!(ray_capsule(Vec3::new(1.0, 2.0, 0.0), Vec3::NEG_Z, f).is_none());
        assert!(ray_capsule(Vec3::new(0.0, 2.0, -9.0), Vec3::NEG_Z, f).is_none());
        let t = ray_capsule(Vec3::new(0.0, 5.0, -5.0), Vec3::NEG_Y, f).unwrap();
        assert!((t - (5.0 - 1.0 - HIT_HEIGHT)).abs() < 1e-4, "{t}");
        // Armour takes the non-piercing share, the rest (and what armour cannot hold) hurts health.
        let mut v = Vitals {
            hp: 100.0,
            ap: 20.0,
            max_hp: 100.0,
            max_ap: 50.0,
        };
        absorb(&mut v, 30.0, 0.5);
        assert_eq!((v.ap, v.hp), (5.0, 85.0));
        absorb(&mut v, 30.0, 0.5);
        assert_eq!((v.ap, v.hp), (0.0, 60.0));
        let s = Spread {
            run: 1.0,
            heat: 1.0,
            shot: 3.0,
            ..default()
        };
        assert_eq!(
            s.cone(10.0, 3.0),
            10.0 * SPREAD_PER_CTRL * (1.0 + RUN_SPREAD + HEAT_SPREAD)
        );
        assert_eq!(
            s.cone(10.0, 10.0),
            10.0 * SPREAD_PER_CTRL * (1.0 + RUN_SPREAD)
        );
    }
}
