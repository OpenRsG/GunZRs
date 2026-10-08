//! Rockets, grenades and consumables: `Fire` of a rocket launcher / grenade / medikit item becomes
//! a flying projectile (splash damage, knockback, explosion effect) or a heal. Retail data gives
//! damage, delay, magazine, grenade radius/duration and potion power/duration; speeds, gravity,
//! fuse and splash falloff are *inferred* (docs/formats.md "Combat").

use crate::{
    actor::ActorData,
    col::MapCollision,
    combat::{EYE, Fx, Lifetime, SELF_BLAST, Vfx, shape},
    effect::FxAssets,
    game::{
        Afflict, Blast, Bot, Damage, Dead, Fire, HitShape, Player, Protected, Push, Team, Vitals,
        friendly,
    },
    item::WeaponKind,
    spy::{self, Mine},
};
use bevy::prelude::*;
use std::f32::consts::FRAC_PI_2;

/// Rocket speed, m/s.
const ROCKET_SPEED: f32 = 30.0;
/// Rocket blast radius, m (rocket items carry no radius).
const ROCKET_RADIUS: f32 = 3.5;
/// Seconds before a rocket that hit nothing disappears.
const ROCKET_LIFE: f32 = 8.0;
/// Seconds between smoke-trail puffs.
const TRAIL_EVERY: f32 = 0.04;
/// Grenade throw speed (m/s), upward bias of the throw direction, gravity (m/s²), bounce factor
/// along the surface normal and friction along the surface.
pub(crate) const THROW_SPEED: f32 = 10.0;
pub(crate) const THROW_LIFT: f32 = 0.3;
pub(crate) const GRAVITY: f32 = 14.0;
pub(crate) const BOUNCE: f32 = 0.45;
pub(crate) const FRICTION: f32 = 0.6;
/// Seconds between the throw button and the release (the throw animation's wind-up), and the
/// fuse after the release (flashbang `handweaponlife` is 1500 ms; frag has none).
pub(crate) const THROW_DELAY: f32 = 0.3;
pub(crate) const FUSE: f32 = 1.5;
/// Knockback of a blast at its centre: m/s away from it and m/s up. The lift is what launches a
/// victim into the blast-fall animation (the actor controller launches from 5 m/s up).
const BLAST_PUSH: f32 = 9.0;
const BLAST_LIFT: f32 = 8.0;
/// Frag radius when the item has no `handweaponcolldist`.
pub(crate) const FRAG_RADIUS: f32 = 4.0;
/// Points of a medikit / repair kit come from `worlditem.xml` (`Weapon::kit_points`).

/// A smoke grenade's cloud: a sphere around the entity's `Transform` that blocks line of sight
/// (see [`smoke_blocks`]) until its `Lifetime` ends.
#[derive(Component)]
pub struct SmokeCloud {
    pub radius: f32,
}

/// Whether a smoke cloud sits on the sight line `from` -> `to` (distance of the segment to the
/// cloud's centre under its radius). Independent of the map collision, for bots' sight checks.
pub fn smoke_blocks<'a>(
    clouds: impl IntoIterator<Item = (&'a Transform, &'a SmokeCloud)>,
    from: Vec3,
    to: Vec3,
) -> bool {
    let seg = to - from;
    let len2 = seg.length_squared().max(1e-6);
    clouds.into_iter().any(|(t, c)| {
        let near = from + seg * ((t.translation - from).dot(seg) / len2).clamp(0.0, 1.0);
        near.distance(t.translation) < c.radius
    })
}

#[derive(Component)]
pub struct Projectile {
    kind: WeaponKind,
    item: u32,
    owner: Entity,
    /// Velocity; while winding up, the unit throw direction.
    vel: Vec3,
    /// Seconds until the release (grenades) / flight time left (rockets).
    wind: f32,
    fuse: f32,
    trail: f32,
    at_rest: bool,
}

impl Projectile {
    /// A blast of `item` (kind `kind`) at the entity's position on the next step: a mine going off.
    pub(crate) fn blast(kind: WeaponKind, item: u32, owner: Entity) -> Self {
        Self {
            kind,
            item,
            owner,
            vel: Vec3::ZERO,
            wind: 0.0,
            fuse: 0.0,
            trail: 0.0,
            at_rest: true,
        }
    }
}

/// Blinded by a flashbang; `total` is the initial duration (the HUD fades on `left / total`).
#[derive(Component)]
pub struct Flashed {
    pub left: f32,
    pub total: f32,
}

/// Heal/repair over time (potions), points per second.
#[derive(Component)]
pub struct Regen {
    hp: f32,
    ap: f32,
    left: f32,
}

#[derive(Component)]
pub(crate) struct FlashOverlay;

#[derive(Resource)]
pub(crate) struct Assets3d {
    rocket: Handle<Mesh>,
    rocket_material: Handle<StandardMaterial>,
}

pub(crate) fn init(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    commands.insert_resource(Assets3d {
        rocket: meshes.add(Capsule3d::new(0.05, 0.35)),
        rocket_material: materials.add(StandardMaterial {
            base_color: Color::srgb(0.35, 0.37, 0.4),
            metallic: 0.6,
            ..default()
        }),
    });
    commands.spawn((
        FlashOverlay,
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.0)),
        GlobalZIndex(1000),
    ));
}

/// `Fire` of a rocket launcher, grenade or consumable.
pub(crate) fn launch(
    mut fires: MessageReader<Fire>,
    data: Res<ActorData>,
    col: Res<MapCollision>,
    assets3d: Res<Assets3d>,
    mut fx: ResMut<Fx>,
    mut assets: FxAssets,
    mut vfx: MessageWriter<Vfx>,
    mut commands: Commands,
    mut actors: Query<(&GlobalTransform, &mut Vitals, Option<&Name>), Without<Dead>>,
    time: Res<Time>,
) {
    for f in fires.read() {
        let Some(w) = data.items.get(f.item).and_then(|i| i.weapon.as_ref()) else {
            continue;
        };
        let Ok((g, mut v, name)) = actors.get_mut(f.shooter) else {
            continue;
        };
        let feet = g.translation();
        let name = name.map_or("?", |n| n.as_str());
        let dir = f.dir.normalize_or_zero();
        let chest = feet + Vec3::Y * (EYE - 0.25);
        match w.kind {
            WeaponKind::Rocket => {
                // Fly from the chest to whatever the aim ray hits (the camera ray starts off to
                // the shoulder, so a straight-ahead rocket would miss the crosshair).
                let aim = col
                    .raycast(f.origin, dir, 200.0)
                    .map_or(f.origin + dir * 100.0, |h| h.point);
                let start = chest + dir * 0.3;
                info!(
                    "t={:.2} rocket: {name} from {:.1}, {:.1}, {:.1} toward {:.1}, {:.1}, {:.1}",
                    time.elapsed_secs(),
                    start.x,
                    start.y,
                    start.z,
                    aim.x,
                    aim.y,
                    aim.z
                );
                let to = aim - start;
                let d = if to.length() > 2.0 && to.dot(dir) > 0.0 {
                    to.normalize()
                } else {
                    dir
                };
                commands
                    .spawn((
                        Projectile {
                            kind: w.kind,
                            item: f.item,
                            owner: f.shooter,
                            vel: d * ROCKET_SPEED,
                            wind: ROCKET_LIFE,
                            fuse: 0.0,
                            trail: 0.0,
                            at_rest: false,
                        },
                        Transform::from_translation(start).looking_to(d, Vec3::Y),
                        Visibility::default(),
                    ))
                    .with_child((
                        Mesh3d(assets3d.rocket.clone()),
                        MeshMaterial3d(assets3d.rocket_material.clone()),
                        Transform::from_rotation(Quat::from_rotation_x(FRAC_PI_2)),
                    ));
            }
            WeaponKind::Frag | WeaponKind::Flashbang | WeaponKind::Smoke | WeaponKind::Stun => {
                let path = match w.kind {
                    WeaponKind::Frag => "model/weapon/grenade/grenade01.elu",
                    WeaponKind::Flashbang => "model/weapon/grenade/flashbang01.elu",
                    WeaponKind::Stun => "model/weapon/grenade/spy_stungrenade.elu",
                    _ => "model/weapon/grenade/smoke01.elu",
                };
                let model = fx.model(path, Transform::default(), &mut assets, &mut commands);
                commands
                    .spawn((
                        Projectile {
                            kind: w.kind,
                            item: f.item,
                            owner: f.shooter,
                            vel: dir,
                            wind: THROW_DELAY,
                            fuse: w
                                .life
                                .filter(|_| {
                                    matches!(w.kind, WeaponKind::Flashbang | WeaponKind::Stun)
                                })
                                .map_or(FUSE, |ms| ms as f32 / 1000.0),
                            trail: 0.0,
                            at_rest: false,
                        },
                        Transform::from_translation(chest),
                        Visibility::Hidden,
                    ))
                    .add_child(model);
            }
            WeaponKind::Mine => {
                // Laid on the floor just ahead of the layer (else under it); needs a floor within 3 m.
                let ahead = Vec3::new(dir.x, 0.0, dir.z).normalize_or_zero();
                let Some(h) = [0.8, 0.0]
                    .into_iter()
                    .find_map(|d| col.raycast(feet + ahead * d + Vec3::Y * 0.8, Vec3::NEG_Y, 3.0))
                else {
                    continue;
                };
                info!(
                    "t={:.2} mine: {name} lays one at {:.1}",
                    time.elapsed_secs(),
                    h.point
                );
                let model = fx.model(
                    "model/weapon/item/spy_landmine.elu",
                    Transform::default(),
                    &mut assets,
                    &mut commands,
                );
                commands
                    .spawn((
                        Mine::new(f.shooter),
                        Transform::from_translation(h.point),
                        Visibility::Hidden,
                    ))
                    .add_child(model);
            }
            WeaponKind::Medikit | WeaponKind::RepairKit => {
                let heal = w.kind == WeaponKind::Medikit;
                let v = &mut *v;
                let (cur, max) = if heal {
                    (&mut v.hp, v.max_hp)
                } else {
                    (&mut v.ap, v.max_ap)
                };
                let before = *cur;
                *cur = (*cur + w.kit_points.unwrap_or(0) as f32).min(max);
                info!(
                    "t={:.2} {}: {name} {} {before:.0} -> {:.0}",
                    time.elapsed_secs(),
                    if heal { "medikit" } else { "repair kit" },
                    if heal { "hp" } else { "ap" },
                    *cur
                );
                vfx.write(Vfx::Facing {
                    name: if heal {
                        "ef_heal_instant"
                    } else {
                        "ef_repair_instant"
                    },
                    at: feet,
                    axis: None,
                    scale: 1.0,
                });
            }
            WeaponKind::Potion => {
                let heal = w.damage_type.as_deref() != Some("repair");
                let rate = w.item_power.unwrap_or(0) as f32;
                let secs = w.damage_time.unwrap_or(0) as f32;
                commands.entity(f.shooter).insert(Regen {
                    hp: if heal { rate } else { 0.0 },
                    ap: if heal { 0.0 } else { rate },
                    left: secs,
                });
                info!(
                    "t={:.2} potion: {name} {} +{rate:.0}/s for {secs:.0} s",
                    time.elapsed_secs(),
                    if heal { "hp" } else { "ap" },
                );
                let (begin, over) = if heal {
                    ("ef_heal_overtime_begin", "ef_heal_overtime")
                } else {
                    ("ef_repair_overtime_begin", "ef_repair_overtime")
                };
                for n in [begin, over] {
                    vfx.write(Vfx::Facing {
                        name: n,
                        at: feet,
                        axis: None,
                        scale: 1.0,
                    });
                }
            }
            _ => {}
        }
    }
}

type Targets<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static GlobalTransform, Option<&'static HitShape>),
    (With<Vitals>, Without<Dead>),
>;

/// The point of an actor's capsule axis nearest to `at` (what a blast measures its distance to)
/// and the capsule radius.
fn nearest(g: &GlobalTransform, hs: Option<&HitShape>, at: Vec3) -> (Vec3, f32) {
    let (r, height) = shape(hs);
    let feet = g.translation();
    (
        feet + Vec3::Y * (at.y - feet.y).clamp(r, (height - r).max(r)),
        r,
    )
}

/// Damage and knockback of a blast at `at` with `radius` and centre damage `damage`; falls off
/// linearly from the capsule surface to 0 at `radius`. Walls between centre and chest shield.
fn splash(
    at: Vec3,
    radius: f32,
    damage: f32,
    p: &Projectile,
    col: &MapCollision,
    targets: &Targets,
    protected: &Query<(), With<Protected>>,
    damage_out: &mut MessageWriter<Damage>,
    vfx: &mut MessageWriter<Vfx>,
    commands: &mut Commands,
    names: &Query<&Name>,
    time: &Time,
) {
    let name = |e| names.get(e).map_or("?", |n| n.as_str());
    for (e, g, hs) in targets {
        // Spawn protection: neither hurt nor pushed.
        if protected.contains(e) {
            continue;
        }
        let (c, r) = nearest(g, hs, at);
        let v = c - at;
        let dist = v.length();
        let factor = (1.0 - (dist - r).max(0.0) / radius).clamp(0.0, 1.0);
        if factor <= 0.0 {
            continue;
        }
        let d = v.normalize_or(Vec3::Y);
        if col
            .raycast(at, d, dist)
            .is_some_and(|h| h.distance < dist - 0.4)
        {
            continue;
        }
        let flat = Vec3::new(d.x, 0.0, d.z).normalize_or_zero();
        let push = (flat * BLAST_PUSH + Vec3::Y * BLAST_LIFT) * factor;
        commands.entity(e).insert(Push(push));
        // You take part of your own blast (`SELF_BLAST`, inferred).
        let amount = damage * factor * if e == p.owner { SELF_BLAST } else { 1.0 };
        info!(
            "t={:.2} blast: {} -> {} {amount:.0} ({:.1} m of {radius:.1} m)",
            time.elapsed_secs(),
            name(p.owner),
            name(e),
            dist
        );
        damage_out.write(Damage {
            target: e,
            attacker: p.owner,
            amount,
            item: p.item,
            point: c,
            dir: d,
            pierce: None,
        });
        vfx.write(Vfx::Blood { point: c, dir: d });
    }
}

/// Moves projectiles, bounces grenades, detonates on contact / fuse.
pub(crate) fn fly(
    mut commands: Commands,
    time: Res<Time>,
    data: Res<ActorData>,
    col: Res<MapCollision>,
    mut projectiles: Query<(Entity, &mut Transform, &mut Projectile, &mut Visibility)>,
    targets: Targets,
    names: Query<&Name>,
    players: Query<(), With<Player>>,
    protected: Query<(), With<Protected>>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut damage: MessageWriter<Damage>,
    mut vfx: MessageWriter<Vfx>,
    mut afflict: MessageWriter<Afflict>,
    sides: Query<(Option<&Team>, Has<Bot>)>,
    mut blast: MessageWriter<Blast>,
) {
    let dt = time.delta_secs().min(0.05);
    for (e, mut tf, mut p, mut vis) in &mut projectiles {
        let pos = tf.translation;
        let Some(w) = data.items.get(p.item).and_then(|i| i.weapon.as_ref()) else {
            commands.entity(e).despawn();
            continue;
        };
        let mut boom = None;
        match p.kind {
            WeaponKind::Rocket => {
                p.wind -= dt;
                let step = p.vel * dt;
                let (len, d) = (step.length(), step.normalize_or(Vec3::NEG_Z));
                let mut best = col.raycast(pos, d, len + 0.05).map(|h| h.distance);
                for (a, g, hs) in &targets {
                    if a == p.owner && p.wind > ROCKET_LIFE - 0.2 {
                        continue;
                    }
                    if let Some(t) = crate::combat::ray_capsule(pos, d, g.translation(), shape(hs))
                        && t <= len
                        && best.is_none_or(|b| t < b)
                    {
                        best = Some(t);
                    }
                }
                if let Some(t) = best {
                    boom = Some(pos + d * (t - 0.05).max(0.0));
                } else if p.wind <= 0.0 {
                    commands.entity(e).despawn();
                    continue;
                } else {
                    tf.translation = pos + step;
                    p.trail -= dt;
                    if p.trail <= 0.0 {
                        p.trail = TRAIL_EVERY;
                        // The smoke ELU is a streak along its local Y, widest at the tail.
                        vfx.write(Vfx::Facing {
                            name: "rocket_smoke",
                            at: pos,
                            axis: Some(-d),
                            scale: 1.0,
                        });
                    }
                }
            }
            _ => {
                if p.wind > 0.0 {
                    // Winding up: held at the thrower's chest, released at the end of the wind-up.
                    p.wind -= dt;
                    if let Ok((_, g, _)) = targets.get(p.owner) {
                        tf.translation = g.translation() + Vec3::Y * (EYE - 0.25);
                    }
                    if p.wind <= 0.0 {
                        let d = (p.vel + Vec3::Y * THROW_LIFT).normalize();
                        tf.translation += p.vel * 0.5;
                        p.vel = d * THROW_SPEED;
                        *vis = Visibility::Inherited;
                    }
                    continue;
                }
                p.fuse -= dt;
                if p.fuse <= 0.0 {
                    boom = Some(pos);
                } else if !p.at_rest {
                    p.vel.y -= GRAVITY * dt;
                    let to = pos + p.vel * dt;
                    match col.sweep_sphere(pos, to, 0.1) {
                        Some(h) => {
                            let n = h.normal;
                            tf.translation = pos + (to - pos).normalize_or_zero() * h.distance;
                            let vn = n * p.vel.dot(n);
                            p.vel = (p.vel - vn) * FRICTION - vn * BOUNCE;
                            if p.vel.length() < 1.0 && n.y > 0.7 {
                                p.vel = Vec3::ZERO;
                                p.at_rest = true;
                            }
                        }
                        None => tf.translation = to,
                    }
                    tf.rotate_local_x(p.vel.length() * dt * 2.0);
                }
            }
        }
        let Some(at) = boom else { continue };
        info!(
            "t={:.2} detonation: {:?} of {} at {:.1}, {:.1}, {:.1}",
            time.elapsed_secs(),
            p.kind,
            names.get(p.owner).map_or("?", |n| n.as_str()),
            at.x,
            at.y,
            at.z
        );
        let (radius, amount) = match p.kind {
            WeaponKind::Rocket => (ROCKET_RADIUS, w.damage as f32),
            _ => (
                w.coll_dist.map_or(FRAG_RADIUS, |c| c as f32 * 0.01),
                w.damage as f32,
            ),
        };
        // The sfx ELUs are authored at arbitrary sizes (the game scales them): `scale` is
        // chosen so the fireball spans about the blast radius (*inferred*).
        let (effect, sound, scale) = match p.kind {
            WeaponKind::Rocket => ("rocket_effect", "fx_explosion01", 1.0),
            WeaponKind::Frag | WeaponKind::Mine => ("ef_exgrenade", "we_grenade_explosion", 16.0),
            WeaponKind::Flashbang | WeaponKind::Stun => {
                ("ef_gre_ex", "we_flashbang_explosion", 4.0)
            }
            _ => ("ef_gunsmoke", "we_gasgrenade_explosion", 2.0),
        };
        vfx.write(Vfx::Facing {
            name: effect,
            at,
            axis: None,
            scale,
        });
        blast.write(Blast { at, sound });
        match p.kind {
            WeaponKind::Stun => {
                // Stuns whoever the grenade's thrower is not friendly with, within its radius
                // and in the open (the retail tip 2206: "briefly stunned").
                let mine = sides.get(p.owner).ok().map(|(t, b)| (t.copied(), b));
                for (a, g, hs) in &targets {
                    let (c, _) = nearest(g, hs, at);
                    let v = c - at;
                    let dist = v.length();
                    let ally = a == p.owner
                        || sides
                            .get(a)
                            .is_ok_and(|(t, b)| mine.is_some_and(|m| friendly(m, (t.copied(), b))));
                    if ally
                        || dist > radius
                        || col
                            .raycast(at, v, dist)
                            .is_some_and(|h| h.distance < dist - 0.4)
                    {
                        continue;
                    }
                    afflict.write(spy::stun(a, p.owner));
                }
            }
            WeaponKind::Flashbang => {
                let secs = w.state_time.unwrap_or(2000) as f32 / 1000.0;
                let look = camera.iter().next().map(|c| c.forward());
                for (a, g, hs) in &targets {
                    let (c, _) = nearest(g, hs, at);
                    let v = c - at;
                    let dist = v.length();
                    if dist > radius
                        || col
                            .raycast(at, v, dist)
                            .is_some_and(|h| h.distance < dist - 0.4)
                    {
                        continue;
                    }
                    // The player is only blinded in proportion to how much the flash is in view.
                    let seen = match (look, players.contains(a)) {
                        (Some(l), true) => {
                            ((at - c).normalize_or_zero().dot(*l) * 0.5 + 0.5).max(0.3)
                        }
                        _ => 1.0,
                    };
                    let total = secs * seen * (1.0 - dist / radius * 0.5);
                    info!(
                        "t={:.2} flashbang: blinds {} for {total:.1} s",
                        time.elapsed_secs(),
                        names.get(a).map_or("?", |n| n.as_str())
                    );
                    commands.entity(a).insert(Flashed { left: total, total });
                }
            }
            WeaponKind::Smoke => {
                let (radius, secs) = (radius * 0.6, w.state_time.unwrap_or(11000) as f32 / 1000.0);
                vfx.write(Vfx::Smoke {
                    point: at,
                    radius,
                    secs,
                });
                commands.spawn((
                    SmokeCloud { radius },
                    Lifetime(secs),
                    Transform::from_translation(at),
                ));
            }
            _ => splash(
                at,
                radius,
                amount,
                &p,
                &col,
                &targets,
                &protected,
                &mut damage,
                &mut vfx,
                &mut commands,
                &names,
                &time,
            ),
        }
        commands.entity(e).despawn();
    }
}

/// Potions: tick heal/repair over time.
pub(crate) fn regen(
    mut commands: Commands,
    time: Res<Time>,
    mut q: Query<(Entity, &mut Regen, &mut Vitals, Has<Dead>)>,
) {
    let dt = time.delta_secs().min(0.05);
    for (e, mut r, mut v, dead) in &mut q {
        let step = dt.min(r.left);
        v.hp = (v.hp + r.hp * step).min(v.max_hp);
        v.ap = (v.ap + r.ap * step).min(v.max_ap);
        r.left -= dt;
        if r.left <= 0.0 || dead {
            commands.entity(e).remove::<Regen>();
        }
    }
}

/// Fades the white overlay with the player's blindness and ticks everyone's down.
pub(crate) fn flash_overlay(
    mut commands: Commands,
    time: Res<Time>,
    mut flashed: Query<(Entity, &mut Flashed, Has<Player>)>,
    mut overlay: Single<&mut BackgroundColor, With<FlashOverlay>>,
) {
    let mut alpha = 0.0;
    for (e, mut f, player) in &mut flashed {
        f.left -= time.delta_secs();
        if f.left <= 0.0 {
            commands.entity(e).remove::<Flashed>();
        } else if player {
            // Fully white for the first half, then fading out.
            alpha = (f.left / f.total * 2.0).min(1.0);
        }
    }
    overlay.0 = Color::srgba(1.0, 1.0, 1.0, alpha);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smoke_blocks_only_on_the_line() {
        let cloud = (
            Transform::from_xyz(0.0, 1.0, -5.0),
            SmokeCloud { radius: 1.5 },
        );
        let sees = |a: Vec3, b: Vec3| !smoke_blocks([(&cloud.0, &cloud.1)], a, b);
        assert!(
            !sees(Vec3::Y, Vec3::new(0.0, 1.0, -10.0)),
            "straight through"
        );
        assert!(
            sees(Vec3::Y, Vec3::new(5.0, 1.0, -10.0)),
            "passes 2 m beside it"
        );
        assert!(
            sees(Vec3::Y, Vec3::new(0.0, 1.0, -3.0)),
            "target stops short of it"
        );
        assert!(
            !sees(Vec3::new(0.0, 1.0, -5.0), Vec3::new(9.0, 1.0, 9.0)),
            "viewer inside"
        );
    }
}
