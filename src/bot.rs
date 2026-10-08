//! Bot AI: spawns `--bots N` bots and steers them through `Intent` (docs/formats.md "Bots").
//! A bot hunts the nearest enemy (the player; in team games the other team), routing over the
//! [`Nav`] floor graph, fights with the weapon that suits the range, strafes, hops and tumbles,
//! throws grenades at enemies behind cover or in a group, butterflies (guard cancel between combo
//! blows), and falls back to the spawn point farthest from the enemy when hurt.

use crate::{
    actor::{ActorData, ActorSpawner, ActorSpec, DEFAULT_LOADOUT},
    col::MapCollision,
    combat::{EYE, HIT_RADIUS, is_melee, rnd, yaw_of},
    game::{Acting, Bot, Dead, Guarding, Intent, Loadout, Status, Team, Vitals, friendly},
    item::{Items, Weapon, WeaponKind},
    level::Level,
    nav::{Kind, Nav, Search, Step, walkable},
    pickup::{ItemKind, WorldItem},
    projectile::{
        BOUNCE, FRAG_RADIUS, FRICTION, FUSE, Flashed, GRAVITY, SmokeCloud, THROW_DELAY, THROW_LIFT,
        THROW_SPEED, smoke_blocks,
    },
    view::{SCALE, to_bevy},
};
use bevy::prelude::*;
use std::f32::consts::{PI, TAU};

/// Bots see an enemy up to this far (metres) when nothing blocks the ray.
const SIGHT: f32 = 80.0;

/// Number of bots to spawn at startup (inserted by `gunz-play --bots N`).
#[derive(Resource)]
pub struct BotCount(pub usize);

/// Spawn the bots this many metres in front of the player instead of at spawn points
/// (`gunz-play --bots-ahead M`; for reproducible tests).
#[derive(Resource)]
pub struct BotAhead(pub f32);

/// Difficulty 0..=1 (default 0.5): scales reaction time, aim error, turn rate, how often bots
/// tumble and hop, and how early they retreat.
#[derive(Resource)]
pub struct BotSkill(pub f32);

/// Spawn points: where idle bots wander and hurt bots retreat to.
#[derive(Resource)]
struct Spawns(Vec<Vec3>);

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostStartup, spawn_bots)
            .add_systems(PreUpdate, bot_ai);
    }
}

/// Bot state: steering timers and the route it follows.
#[derive(Component)]
struct BotAi {
    rng: u32,
    yaw: f32,
    /// Seconds since an enemy was last seen.
    lost: f32,
    /// Seconds before a bot that just spotted an enemy starts attacking.
    react: f32,
    strafe: f32,
    strafe_t: f32,
    burst_t: f32,
    firing: bool,
    hop_t: f32,
    tumble_t: f32,
    /// Double-tap in progress: direction in the facing frame and seconds since it began.
    tap: Option<(Vec2, f32)>,
    switch_t: f32,
    /// Preferred side (+1 left / -1 right) to turn towards when a wall is in the way.
    side: f32,
    /// Route to `goal` and the index of the step being walked; replanned every second or so.
    path: Vec<Step>,
    next: usize,
    /// Route search in progress and the goal it was started for.
    search: Option<Search>,
    search_goal: Vec3,
    path_t: f32,
    /// The goal is a straight walk away.
    direct: bool,
    /// Where an enemy-less or retreating bot is going.
    goal: Option<Vec3>,
    goal_t: f32,
    /// Whether `goal` is a retreat spot.
    fleeing: bool,
    check_t: f32,
    check_pos: Vec3,
    /// Node being walked to at the last stall check.
    check_node: Option<u32>,
    stuck: u32,
    log_t: f32,
    /// Seconds the guard stays up, and seconds before the next decision to raise it.
    guard_t: f32,
    parry_cd: f32,
    /// Pickup being walked to, seconds spent on it, and seconds items are ignored after giving up.
    item: Option<Vec3>,
    item_t: f32,
    no_item: f32,
    /// Heading and seconds left of a side wall run in the air.
    air: Option<(Vec3, f32)>,
    /// Grenade throw in progress, and seconds until the next one may be considered.
    throw: Option<Throw>,
    throw_cd: f32,
    /// Seconds a bot that threw a grenade still stands (the fuse), so it does not walk into it.
    hold: f32,
    /// Butterfly guard cancel in progress (seconds in) and seconds until the next may start.
    bf: Option<f32>,
    bf_cd: f32,
    /// Rooted or stunned last frame (the route is replanned when it ends).
    held: bool,
    /// A mine is being laid (seconds since it began), and seconds until the next one may be.
    lay: Option<f32>,
    lay_cd: f32,
}

impl BotAi {
    fn new(i: usize, yaw: f32, pos: Vec3) -> Self {
        Self {
            rng: 0x1234_5679u32.wrapping_mul(i as u32 + 1) | 1,
            yaw,
            lost: f32::MAX,
            // Spawn grace: a new bot idles for a moment before it engages.
            react: 2.5,
            strafe: 1.0,
            strafe_t: 0.0,
            burst_t: 0.0,
            firing: false,
            hop_t: 2.0,
            tumble_t: 3.0,
            tap: None,
            switch_t: 0.0,
            side: if i % 2 == 0 { 1.0 } else { -1.0 },
            path: Vec::new(),
            next: 0,
            path_t: 0.0,
            search: None,
            search_goal: Vec3::ZERO,
            direct: false,
            goal: None,
            goal_t: 0.0,
            fleeing: false,
            check_t: 0.0,
            check_pos: pos,
            check_node: None,
            stuck: 0,
            log_t: 0.0,
            guard_t: 0.0,
            parry_cd: 0.0,
            item: None,
            item_t: 0.0,
            no_item: 0.0,
            air: None,
            // Spawn grace: no grenade in the first seconds.
            throw: None,
            throw_cd: 3.0,
            hold: 0.0,
            bf: None,
            bf_cd: 0.0,
            held: false,
            lay: None,
            // Spawn grace, as for grenades.
            lay_cd: 4.0,
        }
    }
}

fn spawn_bots(
    mut spawner: ActorSpawner,
    level: Res<Level>,
    col: Res<MapCollision>,
    count: Option<Res<BotCount>>,
    ahead: Option<Res<BotAhead>>,
    player: Query<&Transform, With<crate::game::Player>>,
) {
    let n = count.map_or(0, |c| c.0);
    if n == 0 {
        return;
    }
    let points = level.spawn_points();
    let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
    for v in &level.map.vertices {
        let p = Vec3::from(to_bevy(v.pos)) * SCALE;
        (min, max) = (min.min(p), max.max(p));
    }
    let t = std::time::Instant::now();
    let nav = Nav::new(&col, min, max);
    info!(
        "bot nav: {} floor nodes in {:.2}s (map box {min} .. {max})",
        nav.nodes.len(),
        t.elapsed().as_secs_f32()
    );
    spawner.commands.insert_resource(nav);
    spawner
        .commands
        .insert_resource(Spawns(points.iter().map(|p| p.0).collect()));
    for i in 0..n {
        let (pos, yaw) = match (ahead.as_ref(), player.single()) {
            (Some(a), Ok(p)) => {
                // +0.35: the player's aim ray is offset to the right by the camera shoulder.
                let side = (i as f32 - (n - 1) as f32 / 2.0) * 1.5 + 0.35;
                let pos = p.translation + *p.forward() * a.0 + *p.right() * side;
                (pos, yaw_of(p.translation - pos))
            }
            _ => {
                let (pos, dir) = points[(i + 1) % points.len()];
                (pos, yaw_of(dir))
            }
        };
        let bot = spawner.spawn(ActorSpec {
            name: format!("Bot {}", i + 1),
            pos,
            yaw,
            woman: i % 2 == 1,
            loadout: [&DEFAULT_LOADOUT[..], &[BOT_GRENADE, BOT_SMOKE]].concat(),
            bot: true,
            outfit: None,
        });
        spawner.commands.entity(bot).insert(BotAi::new(i, yaw, pos));
    }
}

/// Rotates `cur` towards `goal` by at most `max` radians.
fn turn(cur: f32, goal: f32, max: f32) -> f32 {
    let d = (goal - cur + PI).rem_euclid(TAU) - PI;
    cur + d.clamp(-max, max)
}

/// Distance (metres) a gun is best used from. *Inferred* from the weapon classes.
fn gun_range(k: WeaponKind) -> Option<f32> {
    use WeaponKind::*;
    Some(match k {
        Shotgun => 5.0,
        Smg | SmgX2 => 8.0,
        Pistol | PistolX2 => 9.0,
        Revolver | RevolverX2 => 11.0,
        MachineGun => 13.0,
        Rocket => 15.0,
        Rifle => 16.0,
        _ => return None,
    })
}

/// Slot to fight with at `dist`: a blade within a few metres (or with every gun empty), else
/// the gun with ammo whose best range is nearest.
fn pick_weapon(items: &Items, load: &Loadout, dist: f32) -> usize {
    let mut best = (load.current, f32::MAX);
    for (i, s) in load.slots.iter().enumerate() {
        let Some(w) = items.get(s.item).and_then(|i| i.weapon.as_ref()) else {
            continue;
        };
        let score = if is_melee(w.kind) {
            if dist < 3.5 {
                0.0
            } else {
                2.0 + (dist - 3.5) * 3.0
            }
        } else if let Some(r) = gun_range(w.kind).filter(|_| s.magazine + s.reserve > 0) {
            (dist - r).abs() + if dist < 3.0 { 6.0 } else { 0.0 }
        } else {
            continue;
        };
        if score < best.1 {
            best = (i, score);
        }
    }
    best.0
}

/// Whether a melee clip is a blow (the part of its motion that can still hit).
fn is_blow(clip: &str) -> bool {
    ["attack", "slash", "uppercut", "jump_slash"]
        .iter()
        .any(|p| clip.starts_with(p))
}

/// Distance from `p` to the segment `a`-`b`.
fn seg_dist(a: Vec3, b: Vec3, p: Vec3) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-6)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

/// Bots carry the default loadout plus this frag grenade (zitem `2200007`, `item_source` DEFAULT;
/// *inferred* choice).
const BOT_GRENADE: u32 = 2200007;
/// Bots also carry a smoke bomb (zitem `2200002`, the `SMOKE` of `modes.rs`; *inferred* choice).
const BOT_SMOKE: u32 = 2200002;
/// A hurt bot throws its smoke this far (metres) towards the enemy it is running from, so the
/// cloud (radius 0.6 x the item's blast radius) stands between them. *Inferred*.
const SMOKE_AHEAD: f32 = 4.0;
/// `plan_throw`'s radius for a smoke bomb: it must land within 0.6 x this of the point
/// `SMOKE_AHEAD` in front and more than this from the thrower. *Inferred*.
const SMOKE_PLAN: f32 = 3.0;

/// Whether slot `i` holds a smoke bomb with one left.
fn is_smoke(load: &Loadout, data: &ActorData, i: usize) -> bool {
    load.slots.get(i).is_some_and(|s| {
        s.magazine > 0
            && data
                .items
                .get(s.item)
                .and_then(|it| it.weapon.as_ref())
                .is_some_and(|w| w.kind == WeaponKind::Smoke)
    })
}
/// Seconds a butterfly holds the guard, which must end before the blade's next blow is ready
/// (katana: strike 0.17 s, ready 0.33 s) or the held click would become the guard's uppercut.
/// *Inferred* from `melee.rs`.
const BUTTERFLY_GUARD: f32 = 0.1;
/// Seconds a bot holds a mine before it clicks (the draw delay and a little more), and how
/// long it keeps trying (a reload may come first). *Inferred*.
const LAY_AFTER: f32 = 0.6;
const LAY_GIVE_UP: f32 = 8.0;

/// A grenade throw: the slot, seconds since it began, the second the button was pressed, and
/// whether it is a smoke bomb thrown at a point in front of the bot (not at the enemy).
#[derive(Clone, Copy)]
struct Throw {
    slot: usize,
    t: f32,
    release: f32,
    smoke: bool,
}

/// Where a grenade released from `chest` along unit `dir` ends up after `fuse` seconds: the same
/// step as `projectile::fly` (gravity, a 0.1 m sphere sweep, bounce and friction, rest on a floor).
fn grenade_landing(col: &MapCollision, chest: Vec3, dir: Vec3, fuse: f32) -> Vec3 {
    const DT: f32 = 1.0 / 30.0;
    let mut pos = chest + dir * 0.5;
    let mut vel = (dir + Vec3::Y * THROW_LIFT).normalize() * THROW_SPEED;
    for _ in 0..(fuse / DT) as u32 {
        vel.y -= GRAVITY * DT;
        let to = pos + vel * DT;
        let Some(h) = col.sweep_sphere(pos, to, 0.1) else {
            pos = to;
            continue;
        };
        pos += (to - pos).normalize_or_zero() * h.distance;
        let vn = h.normal * vel.dot(h.normal);
        vel = (vel - vn) * FRICTION - vn * BOUNCE;
        if vel.length() < 1.0 && h.normal.y > 0.7 {
            break;
        }
    }
    pos
}

/// The pitch that lands a grenade thrown from `feet` along `yaw` within 60 % of its `radius` of
/// the target's body at `target` (feet), and not within `radius` of the thrower; `None` when no
/// throw does (a wall in the way, too far). Tries pitches -0.3..1.1 rad.
fn plan_throw(
    col: &MapCollision,
    feet: Vec3,
    target: Vec3,
    yaw: f32,
    fuse: f32,
    radius: f32,
) -> Option<f32> {
    let chest = feet + Vec3::Y * (EYE - 0.25);
    (0..15)
        .filter_map(|i| {
            let pitch = -0.3 + i as f32 * 0.1;
            let dir = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0) * Vec3::NEG_Z;
            let land = grenade_landing(col, chest, dir, fuse);
            let err = land.distance(target + Vec3::Y * 0.9);
            (err < 0.6 * radius && land.distance(feet) > radius).then_some((err, pitch))
        })
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|b| b.1)
}

/// (blast radius, fuse seconds) of a throwable that hurts or blinds, as `projectile::launch`
/// reads them; `None` for anything else.
fn grenade_stats(w: &Weapon) -> Option<(f32, f32)> {
    let fuse = match w.kind {
        WeaponKind::Frag => FUSE,
        WeaponKind::Flashbang | WeaponKind::Stun => w.life.map_or(FUSE, |ms| ms as f32 / 1000.0),
        _ => return None,
    };
    Some((w.coll_dist.map_or(FRAG_RADIUS, |c| c as f32 * 0.01), fuse))
}

/// Nearest ready pickup within 40 m that the bot needs: health below 70 %, armour below half, or a
/// gun with no spare magazine.
fn wanted_item(
    items: &Query<(&WorldItem, &Transform)>,
    pos: Vec3,
    v: &Vitals,
    load: &Loadout,
    data: &ActorData,
) -> Option<Vec3> {
    let dry = load.slots.iter().any(|s| {
        s.reserve == 0
            && data
                .items
                .get(s.item)
                .and_then(|i| i.weapon.as_ref())
                .is_some_and(|w| gun_range(w.kind).is_some())
    });
    let flat = |p: Vec3| Vec2::new(p.x - pos.x, p.z - pos.z).length();
    items
        .iter()
        .filter(|(w, t)| {
            w.useful_for(v, load, data)
                && match w.kind {
                    ItemKind::Hp => v.hp < 0.7 * v.max_hp,
                    ItemKind::Ap => v.ap < 0.5 * v.max_ap,
                    ItemKind::Bullet => dry,
                }
                && flat(t.translation) < 40.0
                && (t.translation.y - pos.y).abs() < 8.0
        })
        .min_by(|a, b| flat(a.1.translation).total_cmp(&flat(b.1.translation)))
        .map(|(_, t)| t.translation)
}

/// Chase and shoot the nearest enemy; see the module docs. Bots always know where enemies are.
#[allow(clippy::too_many_arguments)]
fn bot_ai(
    time: Res<Time>,
    skill: Option<Res<BotSkill>>,
    data: Res<ActorData>,
    col: Res<MapCollision>,
    nav: Option<ResMut<Nav>>,
    spawns: Option<Res<Spawns>>,
    foes: Query<(Option<&Acting>, Has<Guarding>)>,
    items: Query<(&WorldItem, &Transform)>,
    clouds: Query<(&Transform, &SmokeCloud)>,
    actors: Query<
        (Entity, &GlobalTransform, Option<&Team>, Has<Bot>),
        (With<Vitals>, Without<Dead>),
    >,
    mut bots: Query<
        (
            Entity,
            &Name,
            &GlobalTransform,
            &mut Intent,
            &mut BotAi,
            &Loadout,
            &Vitals,
            Option<&Team>,
            Has<Dead>,
            Has<Flashed>,
            Option<&Status>,
        ),
        With<Bot>,
    >,
) {
    let (Some(mut nav), Some(spawns)) = (nav, spawns) else {
        return;
    };
    let dt = time.delta_secs();
    let skill = skill.map_or(0.5, |s| s.0.clamp(0.0, 1.0));
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    // A* is the one heavy step: one replan starts per frame and all searches share a budget of
    // node expansions (about 2 ms), so a long search spreads over several frames.
    let (mut routed, mut planned, mut budget) = (false, false, 2500usize);
    for (me, name, g, mut intent, mut ai, loadout, vitals, team, dead, blind, status) in &mut bots {
        if dead {
            (intent.attack, intent.jump, intent.reload, intent.walk) =
                (false, false, false, Vec2::ZERO);
            (
                ai.path.clear(),
                ai.tap = None,
                ai.goal = None,
                ai.search = None,
            );
            continue;
        }
        let pos = g.translation();
        // Rooted or stunned: the controller holds it still, which must not read as being stuck;
        // the route is replanned when it is free again.
        let held = status.is_some_and(Status::rooted);
        if held {
            (ai.check_t, ai.check_pos, ai.stuck) = (0.0, pos, 0);
        } else if ai.held {
            ai.path_t = 0.0;
        }
        ai.held = held;
        let eye = pos + Vec3::Y * EYE;
        let mine = (team.copied(), true);
        let foe = actors
            .iter()
            .filter(|(e, _, t, b)| *e != me && !friendly(mine, (t.copied(), *b)))
            .map(|(e, g, ..)| (e, g.translation()))
            .min_by(|a, b| {
                a.1.distance_squared(pos)
                    .total_cmp(&b.1.distance_squared(pos))
            });
        let target = foe.map(|f| f.1);
        let (foe_acting, foe_guard) = foe
            .and_then(|f| foes.get(f.0).ok())
            .unwrap_or((None, false));
        let me_acting = foes.get(me).ok().and_then(|f| f.0);
        let dist = target.map_or(f32::MAX, |t| flat(t - pos).length());

        // Mines (spy trackers): one every 6-12 s while the enemy is not close. The bot switches
        // to the mine, stands still for the draw and clicks once (`lay_fire` below).
        ai.lay_cd -= dt;
        let mine_slot = loadout.slots.iter().position(|s| {
            s.magazine + s.reserve > 0
                && data
                    .items
                    .get(s.item)
                    .and_then(|i| i.weapon.as_ref())
                    .is_some_and(|w| w.kind == WeaponKind::Mine)
        });
        ai.lay = ai.lay.map(|t| t + dt);
        if ai.lay.is_none()
            && ai.lay_cd <= 0.0
            && mine_slot.is_some()
            && ai.throw.is_none()
            && !blind
            && ai.react <= 0.0
            && dist > 6.0
        {
            ai.lay = Some(0.0);
        }
        if ai
            .lay
            .is_some_and(|t| t > LAY_GIVE_UP || mine_slot.is_none())
        {
            (ai.lay, ai.lay_cd) = (None, 6.0);
        }

        // Weapon by range (at most one switch a second).
        ai.switch_t -= dt;
        let want = match mine_slot.filter(|_| ai.lay.is_some()) {
            Some(slot) => slot,
            None if target.is_some() && ai.switch_t <= 0.0 && ai.throw.is_none() => {
                pick_weapon(&data.items, loadout, dist)
            }
            None => loadout.current,
        };
        intent.slot = (want != loadout.current).then(|| {
            ai.switch_t = 1.0;
            debug!(
                "bot {name}: weapon slot {} -> {want} at {dist:.1} m",
                loadout.current
            );
            want
        });
        let weapon = loadout
            .slots
            .get(loadout.current)
            .and_then(|s| data.items.get(s.item))
            .and_then(|i| i.weapon.as_ref());
        let (melee, reach, pref) = match weapon {
            Some(w) if is_melee(w.kind) => {
                let r = w.range.unwrap_or(150) as f32 * 0.01;
                (true, r + HIT_RADIUS - 0.1, r * 0.5)
            }
            Some(w) => (false, 35.0, gun_range(w.kind).unwrap_or(9.0)),
            None => (false, 35.0, 9.0),
        };
        // Only blades and guns are fired by the trigger logic below; a grenade is thrown by the
        // throw sequence.
        let usable = melee || weapon.is_some_and(|w| gun_range(w.kind).is_some());

        // Sight: a flashed bot sees nothing, smoke and walls block the line.
        let seen = target.filter(|t| {
            let v = *t + Vec3::Y * 1.1 - eye;
            let d = v.length();
            !blind
                && d < SIGHT
                && col
                    .raycast(eye, v / d, d)
                    .is_none_or(|h| h.distance >= d - 0.4)
                && !smoke_blocks(clouds.iter(), eye, eye + v)
        });
        if seen.is_some() {
            if ai.lost > 1.0 {
                ai.react = ai.react.max((0.6 + 0.6 * rnd(&mut ai.rng)) * (1.5 - skill));
            }
            ai.lost = 0.0;
        } else {
            ai.lost += dt;
        }

        // Grenades. Considered when the enemy is 5-14 m away (out of melee range) and either
        // behind cover (no sight line) or grouped with another enemy, at most one plan per frame
        // and every few seconds. The sequence equips the grenade, turns to the enemy, clicks once
        // at the planned pitch and stands still while it is thrown: `Intent` only, the actor
        // controller and `projectile::launch` do the rest as for the player.
        ai.throw_cd -= dt;
        let mut press = None;
        let stats = |slot: usize| {
            let w = data
                .items
                .get(loadout.slots.get(slot)?.item)?
                .weapon
                .as_ref()?;
            grenade_stats(w)
        };
        // A bot hurt in the enemy's sight and with a smoke bomb left throws it (below).
        let smoke_slot = (seen.is_some()
            && (4.0..20.0).contains(&dist)
            && vitals.hp < vitals.max_hp * (0.3 + 0.3 * skill))
            .then(|| (0..loadout.slots.len()).find(|&i| is_smoke(loadout, &data, i)))
            .flatten();
        if let Some(mut th) = ai.throw {
            th.t += dt;
            if target.is_none() || blind || th.t > 3.0 || th.t > th.release + 0.9 {
                ai.throw = None;
                (ai.switch_t, ai.throw_cd) = (0.0, 10.0 + 6.0 * rnd(&mut ai.rng));
            } else {
                if loadout.current != th.slot {
                    intent.slot = Some(th.slot);
                } else if th.release == f32::MAX
                    && th.t >= 0.5
                    && let Some(t) = target
                    && let Some((radius, fuse)) = if th.smoke {
                        Some((SMOKE_PLAN, FUSE))
                    } else {
                        stats(th.slot)
                    }
                {
                    let yaw = yaw_of(flat(t - pos));
                    let at = if th.smoke {
                        pos + flat(t - pos).normalize_or_zero() * SMOKE_AHEAD
                    } else {
                        t
                    };
                    if ((yaw - ai.yaw + PI).rem_euclid(TAU) - PI).abs() < 0.1 {
                        match plan_throw(&col, pos, at, yaw, fuse, radius) {
                            Some(pitch) => {
                                (th.release, press) = (th.t, Some(pitch));
                                // A smoke bomb needs no waiting: the retreat goes on.
                                ai.hold = THROW_DELAY + if th.smoke { 0.3 } else { fuse + 0.2 };
                            }
                            None => th.t = f32::MAX,
                        }
                    }
                }
                ai.throw = Some(th);
            }
        } else if ai.throw_cd <= 0.0
            && !planned
            && !blind
            && ai.react <= 0.0
            && (5.0..14.0).contains(&dist)
            && smoke_slot.is_none()
            && let Some(t) = target
            && let Some(slot) = (0..loadout.slots.len())
                .find(|&i| loadout.slots[i].magazine > 0 && stats(i).is_some())
            && let Some((radius, fuse)) = stats(slot)
        {
            planned = true;
            ai.throw_cd = 1.0;
            let cover = seen.is_none();
            let grouped = actors
                .iter()
                .filter(|(e, g, tm, b)| {
                    *e != me
                        && !friendly(mine, (tm.copied(), *b))
                        && flat(g.translation() - t).length() < 0.8 * radius
                })
                .count()
                >= 2;
            if (cover || grouped)
                && plan_throw(&col, pos, t, yaw_of(flat(t - pos)), fuse, radius).is_some()
            {
                debug!(
                    "bot {name}: grenade slot {slot} at {dist:.1} m ({})",
                    if cover { "behind cover" } else { "grouped" }
                );
                ai.throw = Some(Throw {
                    slot,
                    t: 0.0,
                    release: f32::MAX,
                    smoke: false,
                });
            }
        } else if ai.throw_cd <= 0.0
            && !planned
            && !blind
            && ai.react <= 0.0
            && let Some(t) = target
            && let Some(slot) = smoke_slot
        {
            // Hurt in the enemy's sight: put a smoke cloud between us (the retreat then runs
            // behind it; `smoke_blocks` cuts the bots' own sight the same way).
            planned = true;
            ai.throw_cd = 1.0;
            let ahead = pos + flat(t - pos).normalize_or_zero() * SMOKE_AHEAD;
            if plan_throw(&col, pos, ahead, yaw_of(flat(t - pos)), FUSE, SMOKE_PLAN).is_some() {
                debug!(
                    "bot {name}: smoke slot {slot} at {dist:.1} m (hp {:.0})",
                    vitals.hp
                );
                ai.throw = Some(Throw {
                    slot,
                    t: 0.0,
                    release: f32::MAX,
                    smoke: true,
                });
            }
        }

        // Goal: the enemy; when hurt, the spawn point that gets us farthest from the enemy (none:
        // fight on); a random spawn point when there is no enemy.
        let hurt = target.is_some() && vitals.hp < vitals.max_hp * (0.15 + 0.3 * skill);
        ai.goal_t -= dt;
        if hurt != ai.fleeing
            || ai.goal_t <= 0.0
            || ai.goal.is_none() && !hurt
            || ai.goal.is_some_and(|p| flat(p - pos).length() < 1.5) && !hurt
        {
            (ai.fleeing, ai.path_t) = (hurt, 0.0);
            ai.goal_t = 6.0 + 6.0 * rnd(&mut ai.rng);
            ai.goal = match target.filter(|_| hurt) {
                Some(t) => spawns
                    .0
                    .iter()
                    .filter(|p| p.distance(t) > pos.distance(t) + 10.0)
                    .max_by(|a, b| {
                        let s = |p: &Vec3| p.distance(t) - 0.5 * p.distance(pos);
                        s(a).total_cmp(&s(b))
                    })
                    .copied(),
                None => spawns
                    .0
                    .get((rnd(&mut ai.rng) * spawns.0.len() as f32) as usize)
                    .copied(),
            };
        }
        // Hurt bots run until the enemy is on top of them.
        let flee = hurt && ai.goal.is_some() && dist > 3.0;
        // A bot short of health, armour or ammo detours to a pickup unless the enemy is close;
        // it gives up on one it cannot reach after 15 s.
        ai.no_item -= dt;
        let mut item = (dist > 6.0 && ai.no_item <= 0.0)
            .then(|| wanted_item(&items, pos, vitals, loadout, &data))
            .flatten();
        ai.item_t = if item.is_some() && item == ai.item {
            ai.item_t + dt
        } else {
            0.0
        };
        if ai.item_t > 15.0 {
            (item, ai.no_item) = (None, 20.0);
        }
        if item != ai.item {
            (ai.item, ai.path_t) = (item, 0.0);
        }
        let goal = match target {
            _ if item.is_some() => item,
            Some(t) if !flee => Some(t),
            _ => ai.goal,
        };

        // Route: replanned every second or so; a search for a goal that moved on is dropped.
        ai.path_t -= dt;
        if ai.search.is_some() && goal.is_none_or(|g| g.distance(ai.search_goal) > 4.0) {
            ai.search = None;
        }
        if let Some(goal) = goal.filter(|_| ai.path_t <= 0.0 && ai.search.is_none() && !routed) {
            routed = true;
            ai.path_t = 0.8 + 0.4 * rnd(&mut ai.rng);
            ai.direct = walkable(&col, pos, goal);
            ai.search = nav.search(pos, goal).filter(|_| !ai.direct);
            ai.search_goal = goal;
            if ai.search.is_none() {
                ai.path.clear();
                ai.next = 0;
            }
        }
        if let Some(mut q) = ai.search.take() {
            match nav.advance(&mut q, &mut budget) {
                Some(path) => (ai.path, ai.next) = (path, 0),
                None => ai.search = Some(q),
            }
        }
        let node = |nav: &Nav, s: Step| nav.nodes[s.node as usize];
        // Skip nodes already behind us, then walk towards the farthest of the next few that is
        // a straight walk; reaching a takeoff node triggers the jump.
        while ai.next + 1 < ai.path.len()
            && ai.path[ai.next + 1].kind == Kind::Walk
            && flat(node(&nav, ai.path[ai.next + 1]) - pos).length()
                <= flat(node(&nav, ai.path[ai.next + 1]) - node(&nav, ai.path[ai.next])).length()
        {
            ai.next += 1;
        }
        let mut hop = false;
        let mut way: Option<(Vec3, bool, u32)> = None;
        while ai.next < ai.path.len() {
            let mut t = ai.next;
            while t - ai.next < 4
                && ai.path.get(t + 1).is_some_and(|s| s.kind == Kind::Walk)
                && walkable(&col, pos, node(&nav, ai.path[t + 1]))
            {
                t += 1;
            }
            let takeoff = ai.path.get(t + 1).is_some_and(|s| s.kind == Kind::Jump);
            // A jump starts where the simulated run-up did, not at the node.
            let np = if takeoff {
                ai.path[t + 1].takeoff
            } else {
                node(&nav, ai.path[t])
            };
            let r = if takeoff { 0.4 } else { 0.6 };
            if flat(np - pos).length() < r && (np.y - pos.y).abs() < 1.2 {
                ai.next = t + 1;
                hop |= takeoff;
                if takeoff && ai.path[t + 1].turn != Vec3::ZERO {
                    ai.air = Some((ai.path[t + 1].turn, ai.path[t + 1].hold));
                    debug!(
                        "bot {name}: side wall run from {pos:.1} heading {:.2}",
                        ai.path[t + 1].turn
                    );
                }
            } else {
                // Ledge guard stays on unless a drop or jump is imminent.
                let guard = ai.path[t].kind == Kind::Walk
                    && ai.path.get(t + 1).is_none_or(|s| s.kind == Kind::Walk);
                way = Some((np, guard, ai.path[t].node));
                break;
            }
        }
        let have_path = !ai.path.is_empty();

        // Where to face and which way to move (world direction).
        let face: f32;
        let mut mv = Vec3::ZERO;
        let mut strafe = 0.0;
        let mut guard = true;
        let fight = seen.is_some() && ai.direct && dist <= pref + 1.0 && !flee && item.is_none();
        if let Some(t) = target.filter(|_| fight) {
            // In range and in the open: hold the distance and strafe.
            let to = flat(t - pos);
            face = yaw_of(to);
            if !melee && dist < pref - 3.0 {
                mv = -to.normalize_or_zero();
            }
            ai.strafe_t -= dt;
            if ai.strafe_t <= 0.0 {
                ai.strafe = if rnd(&mut ai.rng) < 0.5 { -1.0 } else { 1.0 };
                ai.strafe_t = 0.8 + 1.7 * rnd(&mut ai.rng);
            }
            strafe = ai.strafe * if melee { 0.4 } else { 1.0 };
            if melee && dist > reach * 0.6 {
                mv = to.normalize_or_zero();
            }
        } else {
            let head = match (way, goal) {
                (Some((np, g, _)), _) => {
                    guard = g;
                    flat(np - pos)
                }
                // Route exhausted short of an unreachable goal: stand and watch.
                (None, _) if have_path => Vec3::ZERO,
                (None, Some(g)) => flat(g - pos),
                (None, None) => Vec3::ZERO,
            };
            mv = head.normalize_or_zero();
            face = match seen {
                Some(t) => yaw_of(flat(t - pos)),
                None if mv != Vec3::ZERO => yaw_of(mv),
                None => ai.yaw,
            };
            if !have_path && mv != Vec3::ZERO {
                // No route: feelers pick the nearest heading with no wall within 1.2 m at
                // knee-to-chest height and floor 1.5 m ahead.
                let free = |yaw: f32| {
                    let d = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
                    col.raycast(pos + Vec3::Y * 0.6, d, 1.2).is_none()
                        && col
                            .raycast(pos + d * 1.5 + Vec3::Y * 0.5, Vec3::NEG_Y, 2.5)
                            .is_some()
                };
                let base = yaw_of(mv);
                let yaw = [0.0, 0.5, 1.0, 1.6, 2.4]
                    .into_iter()
                    .flat_map(|o| [base + o * ai.side, base - o * ai.side])
                    .find(|y| free(*y))
                    .unwrap_or(base + 2.4 * ai.side);
                mv = Quat::from_rotation_y(yaw) * Vec3::NEG_Z;
            }
        }

        // Turn, then express the wanted motion in the facing frame.
        // A side wall run (nav `Step::turn`) holds its heading in the air so that the controller
        // meets the wall at a glancing angle.
        if let Some((h, left)) = ai.air {
            ai.air = (left > dt).then_some((h, left - dt));
            (mv, guard, strafe) = (h, false, 0.0);
        }
        let face = match (target, ai.air) {
            (_, Some((h, _))) => yaw_of(h),
            (Some(t), _) if ai.throw.is_some() => yaw_of(flat(t - pos)),
            _ => face,
        };
        ai.yaw = turn(ai.yaw, face, (3.0 + 6.0 * skill) * dt);
        let (s, c) = ai.yaw.sin_cos();
        let mut walk = Vec2::new(
            mv.dot(Vec3::new(c, 0.0, -s)),
            mv.dot(Vec3::new(-s, 0.0, -c)),
        );
        walk.x += strafe;
        walk = walk.clamp_length_max(1.0);

        // Stuck: no progress for 0.8 s while trying to move (no node reached and no nearer to
        // the one being walked to; without a route, no displacement) -> hop and switch sides;
        // the second time in a row, distrust the route's link and plan again.
        let wants_move = walk != Vec2::ZERO && ai.react <= 0.0;
        ai.check_t += dt;
        if ai.check_t >= 0.8 {
            let moved = Vec2::new(pos.x - ai.check_pos.x, pos.z - ai.check_pos.z).length();
            let stalled = match way {
                Some((np, _, id)) => {
                    ai.check_node == Some(id)
                        && flat(np - ai.check_pos).length() - flat(np - pos).length() < 0.2
                }
                None => moved < 0.25,
            };
            ai.check_node = way.map(|w| w.2);
            if stalled && wants_move {
                ai.side = -ai.side;
                hop = true;
                ai.stuck += 1;
                if ai.stuck >= 2 {
                    if ai.next > 0
                        && let (Some(a), Some(b)) = (ai.path.get(ai.next - 1), ai.path.get(ai.next))
                    {
                        nav.mark_broken(a.node, b.node);
                    }
                    (ai.path_t, ai.stuck) = (0.0, 0);
                }
            } else {
                ai.stuck = 0;
            }
            (ai.check_t, ai.check_pos) = (0.0, pos);
        }
        if ai.react > 0.0 {
            // Just spotted an enemy: turn, but stand still until the reaction time is over.
            walk = Vec2::ZERO;
        }

        // Combat moves: occasional hops and double-tap tumbles (sideways, or back when close).
        ai.hop_t -= dt;
        if fight && ai.hop_t <= 0.0 {
            ai.hop_t = (1.5 + 2.5 * rnd(&mut ai.rng)) / (0.3 + skill);
            hop |= rnd(&mut ai.rng) < skill + 0.2;
        }
        ai.tumble_t -= dt;
        // K-style: a blade closes the last few metres with a forward dash and slashes out of it.
        let dash = melee && !flee && seen.is_some() && ai.direct && (3.0..8.0).contains(&dist);
        if (fight || dash || flee && seen.is_some()) && ai.tap.is_none() && ai.tumble_t <= 0.0 {
            ai.tumble_t = (1.5 + 2.5 * rnd(&mut ai.rng)) / (0.4 + skill);
            let dir = if dash {
                Vec2::Y
            } else if dist < 4.0 && !melee {
                Vec2::NEG_Y
            } else {
                Vec2::new(ai.strafe, 0.0)
            };
            let world = Quat::from_rotation_y(ai.yaw) * Vec3::new(dir.x, 0.0, -dir.y);
            let clear = col.raycast(pos + Vec3::Y * 0.6, world, 3.0).is_none()
                && col
                    .raycast(pos + world * 2.5 + Vec3::Y * 0.5, Vec3::NEG_Y, 2.5)
                    .is_some();
            if clear && rnd(&mut ai.rng) < 0.3 + 0.6 * skill {
                debug!("bot {name}: tumble {dir}");
                ai.tap = Some((dir, 0.0));
            }
        }
        let mut slash = false;
        if let Some((dir, t)) = ai.tap.as_mut() {
            *t += dt;
            slash = *dir == Vec2::Y && *t >= 0.35;
            // press, release, press again within the controller's double-tap window
            walk = if *t < 0.08 || (0.16..0.5).contains(t) {
                *dir
            } else {
                Vec2::ZERO
            };
            if *t >= 0.5 {
                ai.tap = None;
            }
            guard = false;
        }
        if blind {
            // Flashed: stumble about at half speed, no jumps.
            walk = (walk + Vec2::new(ai.strafe, 0.0)).clamp_length_max(1.0) * 0.5;
            hop = false;
        }

        ai.hold -= dt;
        if ai.throw.is_some() || ai.hold > 0.0 || ai.lay.is_some() {
            (walk, hop, ai.tap) = (Vec2::ZERO, false, None);
        }
        // Never walk off a ledge (strafing and backing up included) unless the route says so.
        let wish = Vec3::new(walk.x * c - walk.y * s, 0.0, -walk.x * s - walk.y * c);
        if guard
            && wish != Vec3::ZERO
            && col
                .raycast(
                    pos + wish.normalize() * 1.5 + Vec3::Y * 0.5,
                    Vec3::NEG_Y,
                    2.5,
                )
                .is_none()
        {
            walk = Vec2::ZERO;
            ai.strafe = -ai.strafe;
        }
        intent.walk = walk;
        intent.jump = hop;
        intent.yaw = ai.yaw;
        intent.pitch = seen.map_or(0.0, |t| {
            let c = t + Vec3::Y * 1.1;
            (c.y - eye.y).atan2(Vec2::new(c.x - eye.x, c.z - eye.z).length())
        });
        if !melee && ai.throw.is_none() {
            // Aim error: weaker bots are worse shots.
            let e = 1.5 - skill;
            intent.yaw += (rnd(&mut ai.rng) - 0.5) * 0.18 * e;
            intent.pitch += (rnd(&mut ai.rng) - 0.5) * 0.1 * e;
        }
        if let Some(pitch) = press {
            intent.pitch = pitch;
        }

        ai.react -= dt;
        // Trigger. A melee bot raises its guard against a swing, answers it with the guard's
        // uppercut, slashes out of a forward dash, and holds `attack` (a charge, which ignores
        // the guard) against a guarding enemy; a gun bot holds fire when a friend stands in line.
        ai.guard_t -= dt;
        ai.parry_cd -= dt;
        let swung = foe_acting.is_some_and(|a| is_blow(a.clip) && a.time < a.secs * 0.7);
        if melee && seen.is_some() && dist <= reach + 1.0 && swung && ai.parry_cd <= 0.0 {
            ai.parry_cd = 0.8;
            if rnd(&mut ai.rng) < 0.3 + 0.6 * skill {
                ai.guard_t = 0.7;
                debug!("bot {name}: guard against {:?}", foe_acting.map(|a| a.clip));
            }
        }
        // Butterfly: right after its own slash lands (the cancel window of the blow, `Acting`
        // `cancel_from`), a melee bot raises the guard for `BUTTERFLY_GUARD` seconds, which
        // `melee.rs` takes as the guard cancel of the recovery, then lets the held click start the
        // next combo blow. Rolled once per blow: 15 % + 50 % x skill (*inferred*).
        ai.bf_cd -= dt;
        if let Some(t) = ai.bf {
            ai.bf = (t + dt < BUTTERFLY_GUARD).then_some(t + dt);
        } else if melee
            && seen.is_some()
            && dist <= reach + 1.0
            && ai.bf_cd <= 0.0
            && let Some(a) = me_acting
            && matches!(a.clip, "attack1" | "attack2" | "attack3" | "attack4")
            && a.time >= a.cancel_from
        {
            ai.bf_cd = 0.6;
            if rnd(&mut ai.rng) < 0.15 + 0.5 * skill {
                (ai.bf, ai.bf_cd) = (Some(0.0), 1.2);
                debug!("bot {name}: butterfly after {} at {:.2}s", a.clip, a.time);
            }
        }
        let butterfly = ai.bf.is_some();
        intent.guard = butterfly || melee && ai.guard_t > 0.0;
        let engaged = seen.is_some() && dist <= reach && ai.react <= 0.0;
        ai.burst_t -= dt;
        if ai.burst_t <= 0.0 {
            ai.firing = !ai.firing;
            ai.burst_t = if ai.firing {
                0.3 + 0.5 * rnd(&mut ai.rng)
            } else {
                (0.6 + 0.8 * rnd(&mut ai.rng)) * (1.5 - skill)
            };
        }
        let clear = melee
            || foe.is_none_or(|(_, t)| {
                let aim = t + Vec3::Y * 1.1;
                !actors.iter().any(|(e, g, tm, b)| {
                    e != me
                        && friendly(mine, (tm.copied(), b))
                        && seg_dist(eye, aim, g.translation() + Vec3::Y) < 0.7
                })
            });
        intent.attack = !blind
            && usable
            && !butterfly
            && (engaged && (melee || ai.firing) && clear && !(intent.guard && swung)
                || slash && dist <= reach + 2.5);
        if melee && foe_guard && engaged && !blind && !butterfly {
            intent.attack = true;
        }
        if press.is_some() {
            intent.attack = true;
        }
        // Lay the mine once it is in hand and loaded.
        if let (Some(t), Some(s)) = (ai.lay, mine_slot)
            && loadout.current == s
            && t >= LAY_AFTER
            && loadout.slots[s].magazine > 0
        {
            debug!("bot {name}: lays a mine");
            (intent.attack, ai.lay, ai.lay_cd, ai.switch_t) =
                (true, None, 6.0 + 6.0 * rnd(&mut ai.rng), 0.0);
        }
        intent.reload = loadout
            .slots
            .get(loadout.current)
            .is_some_and(|s| !melee && s.magazine == 0);

        ai.log_t -= dt;
        if ai.log_t <= 0.0 {
            ai.log_t = 2.0;
            debug!(
                "bot {name} {team:?}: pos {pos:.1} dist {dist:.1} dy {:.1} {} path {}/{} {:?}->{:.1?} way {:?} walk {:.1} stuck {}",
                target.map_or(0.0, |t| t.y - pos.y),
                if flee {
                    "flee"
                } else if fight {
                    "fight"
                } else if seen.is_some() {
                    "see"
                } else {
                    "hunt"
                },
                ai.next,
                ai.path.len(),
                ai.path.get(ai.next).map(|s| s.kind),
                ai.path.get(ai.next).map(|s| nav.nodes[s.node as usize]),
                way.map(|w| (w.0, w.1)),
                intent.walk,
                ai.stuck,
            );
        }
    }
}
