//! Game modes beyond plain deathmatch (`menu::Mode`): melee-only gladiator loadouts, round
//! modes (Elimination, Assassinate, Duel, Tournament, Spy: nobody respawns until the round is
//! decided, a ready countdown, a round-win screen, spectating while dead), the Berserker hunt,
//! Gunman's random weapons, spawn protection, team/duel spawn points and the training dummies.
//! Match limits and the end screen are `session.rs`.
//! Constants marked *inferred* are not in the retail data (`docs/formats.md`, "Game modes").

use crate::{
    actor::{ActorData, ActorSpawner, ActorSpec, DEFAULT_LOADOUT, PlayerSetup},
    bot::BotAhead,
    col::MapCollision,
    combat::{is_melee, rnd},
    game::{
        Arsenal, Damage, Dead, Equip, Killed, Loadout, NewRound, Player, Protected, Score, SpawnAt,
        Spectate, Team, Vip, Vitals,
    },
    item::{Items, SPY_BAG, SPY_ICE, SPY_MINE, SPY_STUN, WeaponKind},
    level::Level,
    menu::Mode,
    mrs::Vfs,
    session::{Clock, HOLD, ROUND_SECS, Rules},
    view::{SCALE, to_bevy},
};
use bevy::{
    ecs::query::QueryData,
    prelude::*,
    text::Justify,
    ui::{GlobalZIndex, UiTargetCamera},
};
use std::collections::{HashMap, VecDeque};

/// Seconds the round-win screen stays before the next round starts (*inferred*).
const OVER_SECS: f32 = 4.0;
/// Seconds "FIGHT!" stays up after the countdown.
const FIGHT_SECS: f32 = 1.5;
/// Training dummies and how far in front of the player they stand (metres).
const DUMMY_AT: [f32; 4] = [4.0, 6.5, 9.0, 12.0];
/// Berserker's health and armour (*inferred*: three times the normal pair).
const BERSERKER_VITALS: (f32, f32) = (300.0, 150.0);
/// What an actor spawns with (`actor.rs`); a tracker or a former berserker goes back to it.
const NORMAL_VITALS: (f32, f32) = (100.0, 50.0);
/// Seconds the "spies located" banner stays up (*inferred*; Blitzkrieg's message `viewTime`).
const NOTICE_SECS: f32 = 4.0;
/// Spy items (zitem ids, observed): flashbang and smoke grenade; the rest are `item::SPY_*`.
const FLASHBANG: u32 = 2200001;
const SMOKE: u32 = 2200002;
/// Gunman draws one melee weapon and one gun per life: the first named item of each kind
/// (*inferred* choice; the data has no list).
const BLADES: [WeaponKind; 3] = [
    WeaponKind::Katana,
    WeaponKind::Dagger,
    WeaponKind::DoubleKatana,
];
const GUNS: [WeaponKind; 10] = [
    WeaponKind::Pistol,
    WeaponKind::PistolX2,
    WeaponKind::Revolver,
    WeaponKind::RevolverX2,
    WeaponKind::Smg,
    WeaponKind::SmgX2,
    WeaponKind::Shotgun,
    WeaponKind::MachineGun,
    WeaponKind::Rifle,
    WeaponKind::Rocket,
];

pub struct ModesPlugin;

impl Plugin for ModesPlugin {
    fn build(&self, app: &mut App) {
        let mode = |m: Mode| resource_exists::<Rules>.and_then(move |r: Res<Rules>| r.mode == m);
        app.init_resource::<Round>()
            .init_resource::<Spectate>()
            .add_message::<NewRound>()
            .add_systems(Startup, spawn_table.run_if(resource_exists::<Rules>))
            .add_systems(Startup, spy_setup.run_if(mode(Mode::Spy)))
            .add_systems(PostStartup, dummies.run_if(mode(Mode::Training)))
            .add_systems(
                Update,
                (
                    gladiator,
                    dead_added,
                    rounds.run_if(|r: Res<Rules>| r.mode.rounds()),
                    respawn_spots.run_if(|r: Res<Rules>| !r.mode.rounds()),
                    die_at.run_if(resource_exists::<DieAt>),
                    protect,
                    spectate.run_if(|r: Res<Rules>| r.mode.rounds()),
                    overlay.run_if(|r: Res<Rules>| r.mode.rounds()),
                    duel_damage.run_if(|r: Res<Rules>| r.mode == Mode::DuelTournament),
                    berserker.run_if(|r: Res<Rules>| r.mode == Mode::Berserker),
                    gunman.run_if(|r: Res<Rules>| r.mode == Mode::Gunman),
                )
                    .chain()
                    .run_if(resource_exists::<Rules>),
            );
    }

    /// Gunman and Spy spawn every actor with their whole weapon pool; the mode then equips
    /// the part an actor carries ([`Equip`]).
    fn finish(&self, app: &mut App) {
        let pool = {
            let world = app.world();
            let (Some(rules), Some(data)) = (
                world.get_resource::<Rules>(),
                world.get_resource::<ActorData>(),
            ) else {
                return;
            };
            match rules.mode {
                Mode::Gunman => BLADES
                    .iter()
                    .chain(&GUNS)
                    .filter_map(|k| kind_item(&data.items, *k))
                    .collect(),
                Mode::Spy => [
                    DEFAULT_LOADOUT.as_slice(),
                    &[FLASHBANG, SMOKE, SPY_BAG, SPY_ICE, SPY_STUN, SPY_MINE],
                ]
                .concat(),
                Mode::Blitzkrieg => crate::blitz::arsenal(&data.items),
                _ => return,
            }
        };
        info!("arsenal: {pool:?}");
        app.insert_resource(Arsenal(pool));
    }
}

/// Headless runs: the player dies when the match clock reaches this many seconds.
#[derive(Resource)]
pub struct DieAt(pub f32);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    /// Countdown; everybody stands at their spawn, protected.
    #[default]
    Ready,
    Live,
    /// The round-win screen.
    Over,
    /// No more rounds (match decided, or too few actors to play rounds).
    Done,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Side {
    Red,
    Blue,
}

impl Side {
    fn team(self) -> Team {
        match self {
            Side::Red => Team::Red,
            Side::Blue => Team::Blue,
        }
    }
}

/// Round state of the round modes.
#[derive(Resource, Default)]
pub struct Round {
    /// 1-based; 0 before the first round starts.
    pub n: u32,
    pub phase: Phase,
    /// Seconds in the current phase.
    pub t: f32,
    /// Rounds won by Red / Blue (team modes; Spy: trackers / spies).
    pub wins: [u32; 2],
    /// Spy: rounds the player's side won / lost (the match ends on either reaching the limit).
    pub mine: [u32; 2],
    /// Set once the rounds themselves decide the match (the tournament bracket): the headline
    /// of the end screen.
    pub verdict: Option<&'static str>,
    /// The round-win screen's headline and second line.
    title: String,
    detail: String,
    /// Banner over a live round until `.0` seconds into it: (until, text for a spy, text for
    /// a tracker).
    notice: (f32, String, String),
    /// Duel: waiting line; the first two fight (Red / Blue), the winner stays in front.
    /// Tournament: the contestants still in, the winner goes to the back, the loser is out.
    queue: VecDeque<Entity>,
    /// Tournament: damage each fighter dealt in this duel (a time-out goes to the higher).
    dealt: Vec<(Entity, f32)>,
    spy: SpyRound,
}

impl Round {
    /// The two duellists of the current (or next) duel.
    pub fn duelists(&self) -> Option<(Entity, Entity)> {
        Some((*self.queue.front()?, *self.queue.get(1)?))
    }
}

/// Actor spawn points of the map: `spawn_solo_*`, `spawn_team1_*` (Red), `spawn_team2_*`
/// (Blue); a map without a kind uses every `spawn*` dummy for it. Which side is which team
/// is *inferred*.
#[derive(Resource)]
struct Spawns {
    solo: Vec<(Vec3, Vec3)>,
    red: Vec<(Vec3, Vec3)>,
    blue: Vec<(Vec3, Vec3)>,
}

impl Spawns {
    fn of(&self, team: Option<Team>, teams: bool) -> &[(Vec3, Vec3)] {
        match (team, teams) {
            (Some(Team::Red), true) => &self.red,
            (Some(Team::Blue), true) => &self.blue,
            _ => &self.solo,
        }
    }
}

/// Also rerun by `quest.rs` when a sector swaps the map.
pub fn spawn_table(mut commands: Commands, level: Res<Level>) {
    let named = |prefix: &str| -> Vec<(Vec3, Vec3)> {
        level
            .map
            .dummies
            .iter()
            .filter(|d| d.name.starts_with(prefix))
            .map(|d| {
                (
                    Vec3::from(to_bevy(d.pos)) * SCALE,
                    Vec3::from(to_bevy(d.dir)).normalize_or(Vec3::NEG_Z),
                )
            })
            .collect()
    };
    let all = level.spawn_points();
    let or_all = |v: Vec<_>| if v.is_empty() { all.clone() } else { v };
    commands.insert_resource(Spawns {
        solo: or_all(named("spawn_solo")),
        red: or_all(named("spawn_team1")),
        blue: or_all(named("spawn_team2")),
    });
}

/// The spawn point farthest from every position in `others`.
fn pick(list: &[(Vec3, Vec3)], others: &[Vec3]) -> (Vec3, Vec3) {
    let near = |p: Vec3| {
        others
            .iter()
            .map(|q| p.distance(*q))
            .fold(f32::MAX, f32::min)
    };
    list.iter()
        .copied()
        .max_by(|a, b| near(a.0).total_cmp(&near(b.0)))
        .unwrap_or((Vec3::ZERO, Vec3::NEG_Z))
}

/// Gladiator: a melee slot only.
fn gladiator(rules: Res<Rules>, mut q: Query<&mut Loadout, Added<Loadout>>) {
    if rules.mode.melee_only() {
        for mut l in &mut q {
            l.slots.truncate(1);
            l.current = 0;
        }
    }
}

/// Combat inserts every `Dead` with its own wait; the mode decides how long it really is.
fn dead_added(rules: Res<Rules>, mut q: Query<&mut Dead, Added<Dead>>) {
    for mut d in &mut q {
        // 0 = "respawn now" (round start), left alone
        if d.respawn > 0.0 {
            d.respawn = if rules.mode.rounds() {
                HOLD
            } else {
                rules.respawn
            };
        }
    }
}

/// A training dummy: an inert actor (no AI) that respawns where it stood.
#[derive(Component)]
struct Home;

/// Training: dummies on the floor in front of the player.
fn dummies(
    mut spawner: ActorSpawner,
    col: Res<MapCollision>,
    player: Query<&Transform, With<Player>>,
) {
    let Ok(p) = player.single() else { return };
    let (fwd, right) = (*p.forward(), *p.right());
    let eye = p.translation + Vec3::Y * 1.5;
    for (i, d) in DUMMY_AT.into_iter().enumerate() {
        // The first lateral offset that has level floor and a clear line to the player.
        let spot = [1.5, -1.5, 0.0]
            .into_iter()
            .cycle()
            .skip(i)
            .take(3)
            .find_map(|side| {
                let at = p.translation + fwd * d + right * side;
                let floor = col.raycast(at + Vec3::Y * 2.0, Vec3::NEG_Y, 6.0)?;
                let to = eye - (floor.point + Vec3::Y * 1.5);
                let level = (floor.point.y - p.translation.y).abs() < 1.0;
                (level && col.raycast(eye, -to, to.length() - 0.3).is_none())
                    .then_some((floor.point, to))
            });
        let Some((pos, to)) = spot else { continue };
        let dir = Vec3::new(to.x, 0.0, to.z).normalize_or(Vec3::NEG_Z);
        let e = spawner.spawn(ActorSpec {
            name: format!("Dummy {}", i + 1),
            pos: pos + Vec3::Y * 0.1,
            yaw: f32::atan2(-dir.x, -dir.z),
            woman: i % 2 == 1,
            loadout: vec![],
            outfit: None,
            bot: true,
        });
        spawner
            .commands
            .entity(e)
            .insert((Home, SpawnAt { pos, dir }));
        info!("training dummy {} at {pos:.1?} ({d} m ahead)", i + 1);
    }
}

/// Deathmatch modes: while an actor waits to respawn, keep its spawn point (its team's side,
/// the one farthest from everyone alive) up to date.
fn respawn_spots(
    mut commands: Commands,
    spawns: Res<Spawns>,
    rules: Res<Rules>,
    dead: Query<(Entity, &Dead, Option<&Team>), Without<Home>>,
    alive: Query<&Transform, (With<Vitals>, Without<Dead>)>,
) {
    let mut others: Option<Vec<Vec3>> = None;
    for (e, d, team) in &dead {
        if d.respawn > 0.6 {
            continue;
        }
        let others = others.get_or_insert_with(|| alive.iter().map(|t| t.translation).collect());
        let (pos, dir) = pick(spawns.of(team.copied(), rules.mode.teams()), others);
        commands.entity(e).insert(SpawnAt { pos, dir });
    }
}

fn die_at(
    die: Res<DieAt>,
    clock: Res<Clock>,
    rules: Res<Rules>,
    mut commands: Commands,
    mut player: Query<(Entity, &mut Vitals, &mut Score), (With<Player>, Without<Dead>)>,
) {
    if clock.elapsed < die.0 {
        return;
    }
    commands.remove_resource::<DieAt>();
    for (e, mut v, mut s) in &mut player {
        v.hp = 0.0;
        s.deaths += 1;
        commands.entity(e).insert(Dead {
            respawn: rules.respawn.max(0.1),
        });
    }
}

/// Spawn protection: blinks the actor while `Protected` lasts, then lifts it. Deathmatch
/// respawns get `Rules::protect` seconds (round starts are handled by [`begin`]).
fn protect(
    time: Res<Time>,
    rules: Res<Rules>,
    mut commands: Commands,
    mut revived: RemovedComponents<Dead>,
    homes: Query<(), With<Home>>,
    mut q: Query<(Entity, &mut Protected, &mut Visibility)>,
) {
    if !rules.mode.rounds() {
        // an NPC corpse is despawned after its death animation: nothing left to revive
        for e in revived.read() {
            if commands.get_entity(e).is_err() {
                continue;
            }
            commands.entity(e).remove::<SpawnAt>();
            if rules.protect > 0.0 && !homes.contains(e) {
                commands.entity(e).insert(Protected(rules.protect));
            }
        }
    }
    let dt = time.delta_secs();
    for (e, mut p, mut vis) in &mut q {
        p.0 -= dt;
        let want = if p.0 <= 0.0 || (p.0 * 8.0) as i32 % 2 == 0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        if p.0 <= 0.0 {
            commands.entity(e).remove::<Protected>();
        }
    }
}

/// The player's camera follows a living actor while the player is dead in a round mode
/// (teammates first in team modes); Space or a click cycles.
fn spectate(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut spec: ResMut<Spectate>,
    player: Query<(Entity, Option<&Team>, Has<Dead>), With<Player>>,
    alive: Query<(Entity, Option<&Team>), (With<Vitals>, Without<Dead>)>,
) {
    let Ok((me, my_team, dead)) = player.single() else {
        return;
    };
    if !dead {
        if spec.0.is_some() {
            spec.0 = None;
        }
        return;
    }
    let mut seen: Vec<Entity> = alive.iter().filter(|a| a.0 != me).map(|a| a.0).collect();
    let mates: Vec<Entity> = alive
        .iter()
        .filter(|a| a.0 != me && my_team.is_some() && a.1 == my_team)
        .map(|a| a.0)
        .collect();
    if !mates.is_empty() {
        seen = mates;
    }
    seen.sort_unstable();
    let next = keys.just_pressed(KeyCode::Space) || mouse.just_pressed(MouseButton::Left);
    let cur = spec.0.and_then(|c| seen.iter().position(|e| *e == c));
    let at = match (cur, next) {
        (Some(i), true) => Some((i + 1) % seen.len()),
        (Some(i), false) => Some(i),
        (None, _) => (!seen.is_empty()).then_some(0),
    };
    let want = at.map(|i| seen[i]);
    if spec.0 != want {
        spec.0 = want;
    }
}

/// A VIP's name before the `[VIP]` tag.
#[derive(Component)]
struct VipTag(String);

#[derive(QueryData)]
#[query_data(mutable)]
struct Seat {
    e: Entity,
    name: &'static mut Name,
    team: Option<&'static Team>,
    player: Has<Player>,
    dead: Option<&'static mut Dead>,
    vitals: &'static mut Vitals,
    score: &'static mut Score,
    vip: Option<&'static VipTag>,
    spy: Has<Spy>,
}

type Seats<'w, 's> = Query<'w, 's, Seat>;

impl Side {
    fn of(t: Team) -> Side {
        if t == Team::Red {
            Side::Red
        } else {
            Side::Blue
        }
    }
}

/// Whether rounds can run at all: team modes need both teams (and every actor a team yet),
/// the duel two fighters, Spy its minimum of players. Builds the duel queue (the player first).
fn can_play(rules: &Rules, round: &mut Round, spy: Option<&SpyCfg>, seats: &Seats) -> bool {
    let ok = if rules.mode.duel() {
        let mut ids: Vec<(bool, Entity)> = seats.iter().map(|s| (!s.player, s.e)).collect();
        ids.sort_unstable();
        round.queue = ids.into_iter().map(|i| i.1).collect();
        round.queue.len() >= 2
    } else if let Some(cfg) = spy {
        seats.iter().count() as u32 >= cfg.min_players
    } else {
        if seats.iter().any(|s| s.team.is_none()) {
            return false; // `session::teams` has not run yet
        }
        let red = seats.iter().filter(|s| s.team == Some(&Team::Red)).count();
        red > 0 && red < seats.iter().count()
    };
    if !ok {
        warn!(
            "{}: not enough actors for rounds, free play",
            rules.mode.name()
        );
        round.phase = Phase::Done;
    }
    ok
}

/// Starts a round: everyone who fights respawns at its team's side (the duel's benched
/// actors lie hidden), protected through the countdown. `keep`: the first round of a run
/// with a fixed player/bot placement (`--at`, `--bots-ahead`) leaves living actors where
/// they are. Spy: draws the spies, everybody starts as one team (nobody can be hurt until
/// the spies are located) with the role's health.
fn begin(
    rules: &Rules,
    spawns: &Spawns,
    spy: Option<&SpyCfg>,
    round: &mut Round,
    keep: bool,
    commands: &mut Commands,
    seats: &mut Seats,
) {
    round.n += 1;
    round.phase = Phase::Ready;
    round.t = 0.0;
    round.notice.0 = 0.0;
    round.dealt.clear();
    if let Some((lo, hi)) = spy.and_then(|c| c.players)
        && !(lo..=hi).contains(&seats.iter().count().try_into().unwrap_or(0))
    {
        warn!("spymaplist.xml lists this map for {lo}-{hi} players");
    }
    round.spy.revealed = false;
    let duel = rules.mode.duel();
    let total = seats.iter().count() as u32;
    let spies = spy.map(|c| pick_roles(c, round, seats)).unwrap_or_default();
    if spy.is_some() {
        let who: Vec<String> = spies
            .iter()
            .filter_map(|e| seats.get(*e).ok())
            .map(|s| {
                s.vip
                    .map_or_else(|| s.name.as_str().to_owned(), |t| t.0.clone())
            })
            .collect();
        info!("round {}: spies {who:?} of {total}", round.n);
    }
    round.spy.spies = spies;
    let fighting: Vec<Entity> = if duel {
        round.queue.iter().take(2).copied().collect()
    } else {
        seats.iter().map(|s| s.e).collect()
    };
    let (mut taken, mut members): (Vec<Vec3>, [Vec<Entity>; 2]) = (Vec::new(), Default::default());
    for mut s in &mut seats.iter_mut() {
        if let Some(tag) = s.vip {
            *s.name = Name::new(tag.0.clone());
            commands.entity(s.e).remove::<(Vip, VipTag)>();
        }
        let Some(seat) = fighting.iter().position(|e| *e == s.e) else {
            match s.dead.as_mut() {
                Some(d) => d.respawn = HOLD,
                None => drop(commands.entity(s.e).insert(Dead { respawn: HOLD })),
            }
            commands.entity(s.e).insert(Visibility::Hidden);
            continue;
        };
        // The duel's two fighters are Red and Blue, so bots fight each other and the board
        // shows the sides.
        let team = if duel {
            let t = [Team::Red, Team::Blue][seat];
            commands.entity(s.e).insert(t);
            Some(t)
        } else if spy.is_some() {
            commands.entity(s.e).insert(Team::Red);
            Some(Team::Red)
        } else {
            s.team.copied()
        };
        if let Some(t) = team {
            members[Side::of(t) as usize].push(s.e);
        }
        if let Some(cfg) = spy {
            let is_spy = round.spy.spies.contains(&s.e);
            let (hp, ap) = if is_spy {
                (cfg.row(total).hpap, cfg.row(total).hpap)
            } else {
                NORMAL_VITALS
            };
            *s.vitals = Vitals {
                hp,
                ap,
                max_hp: hp,
                max_ap: ap,
            };
            if is_spy {
                commands.entity(s.e).insert(Spy);
            } else {
                commands.entity(s.e).remove::<(Spy, Located)>();
            }
        }
        if !keep || s.dead.is_some() {
            let (pos, dir) = pick(spawns.of(team, rules.mode.teams() || duel), &taken);
            taken.push(pos);
            info!("  {} ({team:?}) spawns at {pos:.1?}", s.name.as_str());
            match s.dead.as_mut() {
                Some(d) => d.respawn = 0.0,
                None => drop(commands.entity(s.e).insert(Dead { respawn: 0.0 })),
            }
            commands.entity(s.e).insert(SpawnAt { pos, dir });
        }
        commands.entity(s.e).insert((
            Protected(rules.ready + rules.protect),
            Visibility::Inherited,
        ));
    }
    if rules.mode == Mode::Assassinate {
        for (i, m) in members.iter().enumerate() {
            if m.is_empty() {
                continue;
            }
            let vip = m[(round.n as usize * 5 + i * 3) % m.len()];
            if let Ok(mut s) = seats.get_mut(vip) {
                let base = s.name.as_str().to_owned();
                *s.name = Name::new(format!("{base} [VIP]"));
                commands.entity(vip).insert((Vip, VipTag(base)));
            }
        }
    }
}

/// `Some(winner)` (`None` inside: a draw) once the round is decided, and whether the clock
/// decided it: a side is out when all its actors are dead (Assassinate: its VIP is dead; Duel:
/// its fighter is dead; Spy: Red are the trackers, Blue the spies); at the time limit the side
/// with more actors alive wins (duel: more health + armour, tournament: more damage dealt,
/// Spy: the spies, they survived).
fn outcome(rules: &Rules, round: &Round, seats: &Seats) -> Option<(Option<Side>, bool)> {
    let duel = round.duelists().filter(|_| rules.mode.duel());
    let alive = |t: Team| {
        seats
            .iter()
            .filter(|s| s.team == Some(&t) && s.dead.is_none())
            .count()
    };
    let spies = |on: bool| {
        seats
            .iter()
            .filter(|s| s.spy == on && s.dead.is_none())
            .count()
    };
    let (red_out, blue_out) = match duel {
        Some((a, b)) => {
            let gone = |e| seats.get(e).is_ok_and(|s| s.dead.is_some());
            (gone(a), gone(b))
        }
        None if rules.mode == Mode::Spy => (spies(false) == 0, spies(true) == 0),
        None => {
            let vip_down = |t: Team| {
                rules.mode == Mode::Assassinate
                    && seats
                        .iter()
                        .any(|s| s.team == Some(&t) && s.vip.is_some() && s.dead.is_some())
            };
            (
                alive(Team::Red) == 0 || vip_down(Team::Red),
                alive(Team::Blue) == 0 || vip_down(Team::Blue),
            )
        }
    };
    match (red_out, blue_out) {
        (true, true) => return Some((None, false)),
        (true, false) => return Some((Some(Side::Blue), false)),
        (false, true) => return Some((Some(Side::Red), false)),
        _ => {}
    }
    if round.t < rules.round_secs {
        return None;
    }
    if rules.mode == Mode::Spy {
        return Some((Some(Side::Blue), true));
    }
    let dealt = |e: Entity| round.dealt.iter().find(|d| d.0 == e).map_or(0.0, |d| d.1);
    let strength = |side: Side| match duel {
        Some((a, b)) => {
            let e = if side == Side::Red { a } else { b };
            if rules.mode == Mode::DuelTournament {
                dealt(e)
            } else {
                seats.get(e).map_or(0.0, |s| s.vitals.hp + s.vitals.ap)
            }
        }
        None => alive(side.team()) as f32,
    };
    let (r, b) = (strength(Side::Red), strength(Side::Blue));
    let winner = if r > b {
        Some(Side::Red)
    } else if b > r {
        Some(Side::Blue)
    } else {
        None
    };
    Some((winner, true))
}

/// Scores the round, writes the round-win screen text and (duel) rotates the queue; the
/// tournament drops the loser from it and settles `Round::verdict` when the player is out or
/// one contestant is left.
fn conclude(
    rules: &Rules,
    round: &mut Round,
    winner: Option<Side>,
    by_time: bool,
    seats: &mut Seats,
) {
    let duel = round.duelists().filter(|_| rules.mode.duel());
    let tournament = rules.mode == Mode::DuelTournament;
    let spy = rules.mode == Mode::Spy;
    // A timed-out duel's winner is credited like a kill.
    if let (Some((a, b)), Some(w), true) = (duel, winner, by_time)
        && let Ok(mut s) = seats.get_mut(if w == Side::Red { a } else { b })
    {
        s.score.kills += 1;
    }
    let name = |seats: &Seats, e: Entity| {
        seats
            .get(e)
            .map_or_else(|_| "?".to_owned(), |s| s.name.as_str().to_owned())
    };
    // Who the player is rooting for: its team, or its seat in the duel.
    let mine = match duel {
        Some((a, b)) => seats
            .get(a)
            .ok()
            .filter(|s| s.player)
            .map(|_| Side::Red)
            .or_else(|| seats.get(b).ok().filter(|s| s.player).map(|_| Side::Blue)),
        None => seats
            .iter()
            .find(|s| s.player)
            .and_then(|s| s.team.copied())
            .map(Side::of),
    };
    let who = |side: Side| match (duel, side) {
        (Some((a, _)), Side::Red) => name(seats, a),
        (Some((_, b)), Side::Blue) => name(seats, b),
        (None, Side::Red) if spy => "TRACKERS".into(),
        (None, Side::Blue) if spy => "SPIES".into(),
        (None, Side::Red) => "RED TEAM".into(),
        (None, Side::Blue) => "BLUE TEAM".into(),
    };
    round.title = match (winner, mine) {
        (None, _) => "ROUND DRAWN".into(),
        (Some(w), Some(m)) if w == m => "ROUND WON".into(),
        (Some(_), Some(_)) => "ROUND LOST".into(),
        (Some(w), None) => format!("{} WINS", who(w).to_uppercase()),
    };
    match (duel, winner) {
        (Some((a, b)), w) => {
            let (x, y) = (
                round.queue.pop_front().unwrap_or(a),
                round.queue.pop_front().unwrap_or(b),
            );
            let mut out = false;
            if tournament {
                match w {
                    Some(side) => {
                        let (win, lose) = if side == Side::Red { (x, y) } else { (y, x) };
                        round.queue.push_back(win);
                        out = seats.get(lose).is_ok_and(|s| s.player);
                        if round.queue.len() == 1 {
                            let player_won = seats.get(win).is_ok_and(|s| s.player);
                            round.verdict = Some(if player_won { "VICTORY" } else { "DEFEAT" });
                        }
                    }
                    // A drawn bout is fought again.
                    None => {
                        round.queue.push_front(y);
                        round.queue.push_front(x);
                    }
                }
            } else {
                let (stay, go) = match w {
                    Some(Side::Red) => (Some(x), Some(y)),
                    Some(Side::Blue) => (Some(y), Some(x)),
                    None => (None, None),
                };
                match (stay, go) {
                    (Some(s), Some(g)) => {
                        round.queue.push_front(s);
                        round.queue.push_back(g);
                    }
                    _ => round.queue.extend([x, y]),
                }
            }
            let next = round
                .duelists()
                .map(|(p, q)| format!("Next: {} vs {}", name(seats, p), name(seats, q)))
                .unwrap_or_default();
            round.detail = match w {
                Some(w) if tournament => match (round.verdict, out) {
                    (Some(_), _) => format!("{} wins the tournament", who(w)),
                    (None, true) => format!("You are out of the tournament    {next}"),
                    (None, false) => format!(
                        "{} advances ({})    {next}",
                        who(w),
                        if by_time {
                            "dealt more damage"
                        } else {
                            "knockout"
                        }
                    ),
                },
                Some(w) => format!(
                    "{} {}    {next}",
                    who(w),
                    if by_time {
                        "wins on health"
                    } else {
                        "wins the duel"
                    }
                ),
                None if tournament => format!("Drawn: fight again    {next}"),
                None => format!("Both go to the back of the line    {next}"),
            };
        }
        (None, w) => {
            if let Some(w) = w {
                round.wins[w as usize] += 1;
                if let Some(m) = mine.filter(|_| spy) {
                    round.mine[usize::from(w != m)] += 1;
                }
            }
            let (red, blue) = if spy {
                ("TRACKERS", "SPIES")
            } else {
                ("RED", "BLUE")
            };
            round.detail = format!(
                "{}    {red} {} : {} {blue}",
                match w {
                    Some(w) if spy => format!("{} win the round", who(w)),
                    Some(w) => format!("{} wins the round", who(w)),
                    None => "Nobody wins the round".into(),
                },
                round.wins[0],
                round.wins[1]
            );
            if spy {
                let ids: Vec<String> = seats
                    .iter()
                    .filter(|s| s.spy)
                    .map(|s| {
                        s.vip
                            .map_or_else(|| s.name.as_str().to_owned(), |t| t.0.clone())
                    })
                    .collect();
                round.detail += &format!("\nSpy's Identity: {}", ids.join(", "));
            }
        }
    }
    round.phase = Phase::Over;
    round.t = 0.0;
}

/// The round flow: Ready (countdown) -> Live -> Over (round-win screen) -> the next round,
/// until `Rules::kill_limit` round wins (the clock then ends the match) or the tournament
/// bracket is decided.
#[allow(clippy::too_many_arguments)]
fn rounds(
    time: Res<Time>,
    rules: Res<Rules>,
    clock: Res<Clock>,
    spawns: Res<Spawns>,
    spy: Option<Res<SpyCfg>>,
    setup: Option<Res<PlayerSetup>>,
    ahead: Option<Res<BotAhead>>,
    mut round: ResMut<Round>,
    mut commands: Commands,
    mut new_round: MessageWriter<NewRound>,
    mut equip: MessageWriter<Equip>,
    mut seats: Seats,
) {
    if clock.over.is_some() || round.phase == Phase::Done {
        return;
    }
    let spy = spy.as_deref();
    round.t += time.delta_secs();
    match round.phase {
        Phase::Ready if round.n == 0 => {
            if spy.is_none_or(|c| round.t >= c.select) && can_play(&rules, &mut round, spy, &seats)
            {
                let keep = setup.is_some_and(|s| s.at.is_some()) || ahead.is_some();
                begin(
                    &rules,
                    &spawns,
                    spy,
                    &mut round,
                    keep,
                    &mut commands,
                    &mut seats,
                );
                new_round.write(NewRound);
                info!("round 1 begins ({})", rules.mode.name());
            }
        }
        Phase::Ready if round.t >= rules.ready => {
            round.phase = Phase::Live;
            round.t = 0.0;
            if let Some(cfg) = spy {
                arm(cfg, &seats, &mut equip);
            }
        }
        Phase::Live => {
            if let Some(cfg) = spy
                && !round.spy.revealed
                && round.t >= cfg.open
            {
                reveal(&mut round, &mut seats, &mut commands);
            }
            if let Some((winner, by_time)) = outcome(&rules, &round, &seats) {
                conclude(&rules, &mut round, winner, by_time, &mut seats);
                info!("round {} over: {} | {}", round.n, round.title, round.detail);
            }
        }
        Phase::Over if round.t >= spy.map_or(OVER_SECS, |c| c.finish_wait) => {
            let top = if rules.mode == Mode::Spy {
                round.mine[0].max(round.mine[1])
            } else if rules.mode.duel() {
                seats.iter().map(|s| s.score.kills).max().unwrap_or(0)
            } else {
                round.wins[0].max(round.wins[1])
            };
            if round.verdict.is_some() || rules.kill_limit.is_some_and(|n| top >= n) {
                round.phase = Phase::Done;
            } else {
                begin(
                    &rules,
                    &spawns,
                    spy,
                    &mut round,
                    false,
                    &mut commands,
                    &mut seats,
                );
                new_round.write(NewRound);
                info!("round {} begins", round.n);
            }
        }
        _ => {}
    }
}

#[derive(Component)]
struct Overlay;

#[derive(Component)]
struct Backdrop;

#[derive(Component, Clone, Copy, PartialEq)]
enum Line {
    Title,
    Sub,
    Spectate,
}

/// Round banner (countdown, FIGHT!, round-win screen) and the spectator line.
#[allow(clippy::too_many_arguments)]
fn overlay(
    mut commands: Commands,
    camera: Query<Entity, With<Camera3d>>,
    round: Res<Round>,
    clock: Res<Clock>,
    rules: Res<Rules>,
    spec: Res<Spectate>,
    names: Query<&Name>,
    vips: Query<(&Name, Has<Player>, Option<&Team>), With<Vip>>,
    player: Query<Option<&Team>, With<Player>>,
    spy_player: Query<Has<Spy>, With<Player>>,
    shown: Query<(), With<Overlay>>,
    mut lines: Query<(&Line, &mut Text, &mut TextColor)>,
    mut backdrop: Query<&mut BackgroundColor, With<Backdrop>>,
) {
    if shown.is_empty() {
        let Ok(camera) = camera.single() else { return };
        commands
            .spawn((
                Overlay,
                UiTargetCamera(camera),
                GlobalZIndex(5),
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    flex_direction: FlexDirection::Column,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    padding: UiRect {
                        top: px(70),
                        bottom: px(120),
                        ..default()
                    },
                    ..default()
                },
            ))
            .with_children(|r| {
                r.spawn((
                    Backdrop,
                    Node {
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::Center,
                        padding: UiRect::axes(px(60), px(10)),
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                ))
                .with_children(|b| {
                    for (line, size) in [(Line::Title, 56.0), (Line::Sub, 24.0)] {
                        b.spawn((
                            line,
                            Text::new(""),
                            TextFont::from_font_size(size),
                            TextColor(Color::WHITE),
                            TextShadow::default(),
                            TextLayout {
                                justify: Justify::Center,
                                ..default()
                            },
                        ));
                    }
                });
                r.spawn((
                    Line::Spectate,
                    Text::new(""),
                    TextFont::from_font_size(22.0),
                    TextColor(Color::WHITE),
                    TextShadow::default(),
                ));
            });
        return;
    }
    let name = |e: Entity| names.get(e).map_or("?", |n| n.as_str()).to_owned();
    let gold = Color::srgb(1.0, 0.85, 0.3);
    let (title, sub, color, back) = match round.phase {
        // The end screen has its own headline.
        _ if clock.over.is_some() => (String::new(), String::new(), Color::WHITE, Color::NONE),
        Phase::Ready if round.n > 0 => {
            let mine = player.single().ok().flatten();
            let intro = match rules.mode {
                m if m.duel() => round.duelists().map_or(String::new(), |(a, b)| {
                    let next = round
                        .queue
                        .get(2)
                        .map_or(String::new(), |&e| format!("   next: {}", name(e)));
                    format!("{} vs {}{next}", name(a), name(b))
                }),
                Mode::Assassinate => {
                    let own = vips.iter().find(|v| v.2 == mine);
                    let foe = vips.iter().find(|v| v.2 != mine);
                    match (own, foe) {
                        (Some(o), _) if o.1 => "You are your team's VIP - stay alive!".into(),
                        (Some(o), Some(f)) => {
                            format!("Kill their VIP {}, protect yours ({})", f.0, o.0)
                        }
                        _ => String::new(),
                    }
                }
                Mode::Spy if spy_player.single().is_ok_and(|s| s) => {
                    "You are a SPY: grenades only - stay alive until time runs out".into()
                }
                Mode::Spy => format!(
                    "You are a TRACKER: hunt the {} hidden spy(ies)",
                    round.spy.spies.len()
                ),
                _ => "Eliminate the other team - no respawn until the round ends".into(),
            };
            let left = (rules.ready - round.t).ceil().max(1.0);
            (
                if rules.mode == Mode::DuelTournament {
                    round.stage()
                } else {
                    format!("ROUND {}", round.n)
                },
                format!("{intro}\n{left}"),
                Color::WHITE,
                Color::NONE,
            )
        }
        Phase::Live if round.t < round.notice.0 => (
            "SPIES LOCATED".into(),
            if spy_player.single().is_ok_and(|s| s) {
                round.notice.1.clone()
            } else {
                round.notice.2.clone()
            },
            Color::srgb(1.0, 0.4, 0.3),
            Color::NONE,
        ),
        Phase::Live if round.t < FIGHT_SECS => (
            "FIGHT!".into(),
            String::new(),
            Color::srgb(1.0, 0.4, 0.3),
            Color::NONE,
        ),
        Phase::Over | Phase::Done if round.n > 0 => {
            let color = match round.title.as_str() {
                "ROUND WON" => gold,
                "ROUND LOST" => Color::srgb(0.9, 0.25, 0.2),
                _ => Color::WHITE,
            };
            (
                round.title.clone(),
                round.detail.clone(),
                color,
                Color::srgba(0.0, 0.0, 0.0, 0.55),
            )
        }
        _ => (String::new(), String::new(), Color::WHITE, Color::NONE),
    };
    let watching = spec.0.map_or(String::new(), |e| {
        format!("SPECTATING  {}     Space / click: next", name(e))
    });
    for (line, mut text, mut tc) in &mut lines {
        let (s, c) = match line {
            Line::Title => (&title, color),
            Line::Sub => (&sub, Color::WHITE),
            Line::Spectate => (&watching, Color::WHITE),
        };
        if text.0 != *s {
            text.0.clone_from(s);
        }
        if tc.0 != c {
            tc.0 = c;
        }
    }
    for mut bg in &mut backdrop {
        if bg.0 != back {
            bg.0 = back;
        }
    }
}

impl Round {
    /// Tournament stage from the contestants still in (the two fighters included).
    pub fn stage(&self) -> String {
        match self.queue.len() {
            0..=2 => "FINAL".into(),
            3..=4 => "SEMIFINAL".into(),
            5..=8 => "QUARTERFINAL".into(),
            n => format!("ROUND OF {n}"),
        }
    }
}

/// Tournament: adds up the damage each duelist deals (a time-out goes to the one who dealt
/// more, as the retail tip says; the raw weapon damage of every hit that was not absorbed
/// by spawn protection, *approximate*).
fn duel_damage(
    mut hits: MessageReader<Damage>,
    mut round: ResMut<Round>,
    protected: Query<(), With<Protected>>,
) {
    for d in hits.read() {
        let Some((a, b)) = round.duelists() else {
            continue;
        };
        let pair = (d.attacker == a && d.target == b) || (d.attacker == b && d.target == a);
        if round.phase != Phase::Live || !pair || d.amount <= 0.0 || protected.contains(d.target) {
            continue;
        }
        match round.dealt.iter_mut().find(|x| x.0 == d.attacker) {
            Some(x) => x.1 += d.amount,
            None => round.dealt.push((d.attacker, d.amount)),
        }
    }
}

/// Berserker: the one actor everybody else hunts. It is its own team (Red) and the rest are
/// Blue, so nobody but the berserker can be hurt by it and the others never hurt each other.
#[derive(Component)]
pub struct Berserker;

const TAG: &str = " [BERSERKER]";

/// Crowns (`on`) or dethrones an actor: team, health and name tag.
fn crown(
    commands: &mut Commands,
    vitals: &mut Query<&mut Vitals>,
    names: &mut Query<&mut Name>,
    e: Entity,
    on: bool,
) {
    let (team, (hp, ap)) = if on {
        (Team::Red, BERSERKER_VITALS)
    } else {
        (Team::Blue, NORMAL_VITALS)
    };
    commands.entity(e).insert(team);
    if on {
        commands.entity(e).insert(Berserker);
    } else {
        commands.entity(e).remove::<Berserker>();
    }
    if let Ok(mut v) = vitals.get_mut(e) {
        (v.max_hp, v.max_ap) = (hp, ap);
        if on {
            (v.hp, v.ap) = (hp, ap);
        } else {
            (v.hp, v.ap) = (v.hp.min(hp), v.ap.min(ap));
        }
    }
    if let Ok(mut n) = names.get_mut(e) {
        let base = n.as_str().trim_end_matches(TAG).to_owned();
        *n = Name::new(if on { format!("{base}{TAG}") } else { base });
    }
}

/// Picks the first berserker (a bot, else the player) once two actors exist, and passes the
/// crown to whoever kills the berserker.
fn berserker(
    mut commands: Commands,
    mut killed: MessageReader<Killed>,
    fresh: Query<Entity, (With<Score>, Without<Team>)>,
    all: Query<(Entity, Has<Player>), With<Score>>,
    boss: Query<(), With<Berserker>>,
    mut vitals: Query<&mut Vitals>,
    mut names: Query<&mut Name>,
) {
    for e in &fresh {
        commands.entity(e).insert(Team::Blue);
    }
    if boss.is_empty() && all.iter().count() >= 2 {
        let first = all
            .iter()
            .filter(|a| !a.1)
            .map(|a| a.0)
            .min()
            .or_else(|| all.iter().map(|a| a.0).min());
        if let Some(e) = first {
            crown(&mut commands, &mut vitals, &mut names, e, true);
            info!(
                "berserker: {} starts as the berserker",
                names
                    .get(e)
                    .map_or("?", |n| n.as_str().trim_end_matches(TAG))
            );
        }
    }
    for k in killed.read() {
        if k.killer != k.victim && boss.contains(k.victim) {
            crown(&mut commands, &mut vitals, &mut names, k.victim, false);
            crown(&mut commands, &mut vitals, &mut names, k.killer, true);
            info!(
                "berserker: {} killed the berserker and becomes it",
                names
                    .get(k.killer)
                    .map_or("?", |n| n.as_str().trim_end_matches(TAG))
            );
        }
    }
}

/// The first named item of `kind` with a model and a real damage (zitem lists debug items with
/// `damage="1"`).
fn kind_item(items: &Items, kind: WeaponKind) -> Option<u32> {
    items
        .weapons()
        .find(|i| {
            i.name.is_some()
                && items.model(i).is_some()
                && i.weapon
                    .as_ref()
                    .is_some_and(|w| w.kind == kind && w.damage > 1)
        })
        .map(|i| i.id)
}

/// Gunman: a random melee weapon and gun for every actor on spawn and on every respawn.
fn gunman(
    mut seed: Local<u32>,
    arsenal: Res<Arsenal>,
    data: Res<ActorData>,
    mut revived: RemovedComponents<Dead>,
    new: Query<Entity, Added<Loadout>>,
    names: Query<&Name>,
    mut equip: MessageWriter<Equip>,
) {
    let who: Vec<Entity> = new.iter().chain(revived.read()).collect();
    if who.is_empty() {
        return;
    }
    if *seed == 0 {
        *seed = 0x9e37_79b9;
    }
    let blade = |id: u32| {
        data.items
            .get(id)
            .and_then(|i| i.weapon.as_ref())
            .is_some_and(|w| is_melee(w.kind))
    };
    let (blades, guns): (Vec<u32>, Vec<u32>) = arsenal.0.iter().copied().partition(|&i| blade(i));
    if blades.is_empty() || guns.is_empty() {
        return;
    }
    let title = |id: u32| {
        data.items
            .get(id)
            .and_then(|i| i.name.as_deref())
            .unwrap_or("?")
    };
    for e in who {
        let b = blades[(rnd(&mut seed) * blades.len() as f32) as usize % blades.len()];
        let g = guns[(rnd(&mut seed) * guns.len() as f32) as usize % guns.len()];
        info!(
            "gunman: {} gets {} + {}",
            names.get(e).map_or("?", |n| n.as_str()),
            title(b),
            title(g)
        );
        equip.write(Equip {
            actor: e,
            items: vec![(b, None), (g, None)],
            current: 1,
        });
    }
}

/// Marker of a spy (Spy mode); its role lasts one round.
#[derive(Component)]
pub(crate) struct Spy;

/// A spy whose position was triangulated: trackers see a marker, refreshed every
/// `spy::PING_SECS`.
#[derive(Component)]
pub(crate) struct Located;

/// Spy bookkeeping across rounds.
#[derive(Default)]
struct SpyRound {
    /// Selection rating per actor; the highest ratings become spies.
    rating: HashMap<Entity, f32>,
    spies: Vec<Entity>,
    /// The spies' positions are known (and the trackers may hurt them).
    revealed: bool,
}

/// One `SPY_TABLE` row of `system/spymode.xml`, for a match of `total` players.
#[derive(Clone, Debug, PartialEq)]
struct SpyRow {
    total: u32,
    spies: u32,
    /// A spy's health and armour (each).
    hpap: f32,
    /// Flashbangs (`LIGHT`), frost bullets (`ICE`) and smoke bombs (`SMOKE`) a spy carries.
    light: u32,
    ice: u32,
    smoke: u32,
}

#[derive(Resource, Clone, Debug, PartialEq)]
struct SpyCfg {
    min_players: u32,
    /// `DefaultRating`, `SelectedRating`, `MaximumRating` of `SELECT_SPY`.
    rating: (f32, f32, f32),
    /// `RounFinishWaitTime`: seconds the round result stays up.
    finish_wait: f32,
    rows: Vec<SpyRow>,
    /// Stun grenades and mines a tracker carries (`TRACER_TABLE`).
    stun: u32,
    mine: u32,
    /// `MinimumRating` of `SELECT_SPY`.
    min_rating: f32,
    /// `selectSpyTime`: seconds after the match starts when the first spies are drawn
    /// (*inferred* reading of the comment "spies are chosen selectSpyTime seconds after the game
    /// starts"; later rounds draw at their start).
    select: f32,
    /// `minPlayers`/`maxPlayers` of the map's `spymaplist.xml` row, if listed.
    players: Option<(u32, u32)>,
    /// Seconds into a round when the spies are located: the row's `spyOpenTime`
    /// (*inferred* one fifth of the round for an unlisted map: that is the ratio in every row).
    open: f32,
}

impl SpyCfg {
    /// The row for `total` players, clamped to the table's range.
    fn row(&self, total: u32) -> &SpyRow {
        let (lo, hi) = (self.rows[0].total, self.rows[self.rows.len() - 1].total);
        let total = total.clamp(lo, hi);
        self.rows
            .iter()
            .find(|r| r.total == total)
            .unwrap_or(&self.rows[0])
    }
}

fn attr<T: std::str::FromStr>(n: roxmltree::Node, name: &str) -> Result<T, String> {
    n.attribute(name)
        .and_then(|v| v.trim().parse().ok())
        .ok_or_else(|| format!("<{}> lacks a valid {name}", n.tag_name().name()))
}

/// `system/spymode.xml`.
fn parse_spy(xml: &str) -> Result<SpyCfg, String> {
    let doc = roxmltree::Document::parse(xml).map_err(|e| e.to_string())?;
    let node = |tag: &str| {
        doc.descendants()
            .find(|n| n.has_tag_name(tag))
            .ok_or_else(|| format!("no <{tag}>"))
    };
    let sel = node("SELECT_SPY")?;
    let rows = doc
        .descendants()
        .filter(|n| n.has_tag_name("SPY_TABLE"))
        .map(|n| {
            Ok(SpyRow {
                total: attr(n, "TotalCount")?,
                spies: attr(n, "SpyCount")?,
                hpap: attr(n, "HPAP")?,
                light: attr(n, "LIGHT")?,
                ice: attr(n, "ICE")?,
                smoke: attr(n, "SMOKE")?,
            })
        })
        .collect::<Result<Vec<SpyRow>, String>>()?;
    if rows.is_empty() {
        return Err("no <SPY_TABLE>".into());
    }
    Ok(SpyCfg {
        min_players: attr(node("BASE")?, "minPlayer")?,
        rating: (
            attr(sel, "DefaultRating")?,
            attr(sel, "SelectedRating")?,
            attr(sel, "MaximumRating")?,
        ),
        min_rating: attr(sel, "MinimumRating")?,
        select: attr(sel, "selectSpyTime")?,
        players: None,
        // The attribute really is spelt "RounFinishWaitTime" in the retail file.
        finish_wait: attr(sel, "RounFinishWaitTime")?,
        rows,
        stun: attr(node("TRACER_TABLE")?, "STUN")?,
        mine: attr(node("TRACER_TABLE")?, "MINE")?,
        open: 0.0,
    })
}

/// The `spymaplist.xml` row of a map.
#[derive(Clone, Copy)]
struct SpyMap {
    limit: f32,
    open: f32,
    min: u32,
    max: u32,
}

/// The Spy map list row for the map in `dir` (`maps/<folder>/`), if it is listed:
/// `system/map.xml` names the id, `system/spymaplist.xml` has the row.
fn spy_map(vfs: &Vfs, dir: &str) -> Option<SpyMap> {
    let folder = dir
        .trim_end_matches('/')
        .rsplit('/')
        .next()?
        .to_ascii_lowercase();
    let read = |p: &str| -> Option<String> {
        let text = String::from_utf8(vfs.read(p).ok()?).ok()?;
        Some(text.trim_start_matches('\u{feff}').to_owned())
    };
    let (maps, list) = (read("system/map.xml")?, read("system/spymaplist.xml")?);
    let maps = roxmltree::Document::parse(&maps).ok()?;
    let id = maps
        .descendants()
        .find(|n| {
            n.has_tag_name("MAP")
                && n.attribute("MapName")
                    .is_some_and(|m| m.eq_ignore_ascii_case(&folder))
        })?
        .attribute("id")?
        .to_owned();
    let list = roxmltree::Document::parse(&list).ok()?;
    let row = list
        .descendants()
        .find(|n| n.has_tag_name("SPY_MAP") && n.attribute("id") == Some(id.as_str()))?;
    Some(SpyMap {
        limit: attr(row, "limitTime").ok()?,
        open: attr(row, "spyOpenTime").ok()?,
        min: attr(row, "minPlayers").ok()?,
        max: attr(row, "maxPlayers").ok()?,
    })
}

/// Loads `spymode.xml` and sets the round time from the map's `limitTime` (unless the round
/// time was changed from its default).
fn spy_setup(mut commands: Commands, mut rules: ResMut<Rules>, level: Res<Level>) {
    let xml = level
        .vfs
        .read("system/spymode.xml")
        .unwrap_or_else(|e| panic!("system/spymode.xml: {e}"));
    let xml = String::from_utf8(xml).unwrap_or_else(|e| panic!("system/spymode.xml: {e}"));
    let mut cfg = parse_spy(xml.trim_start_matches('\u{feff}'))
        .unwrap_or_else(|e| panic!("system/spymode.xml: {e}"));
    let map = spy_map(&level.vfs, &level.map.dir);
    match map {
        Some(m) if rules.round_secs == ROUND_SECS => rules.round_secs = m.limit,
        Some(_) => {}
        None => warn!(
            "{}: not in spymaplist.xml, keeping the round time",
            level.map.dir
        ),
    }
    match map {
        // `spyOpenTime` is a fifth of `limitTime` in every row, so a changed round time keeps the ratio.
        Some(m) => {
            cfg.open = m.open / m.limit * rules.round_secs;
            cfg.players = Some((m.min, m.max));
        }
        None => cfg.open = rules.round_secs / 5.0,
    }
    info!(
        "spy mode: round {:.0} s, spies located after {:.0} s, drawn {:.0} s into the match (selectSpyTime), at least {} players",
        rules.round_secs, cfg.open, cfg.select, cfg.min_players
    );
    commands.insert_resource(cfg);
}

/// The indices of the `n` highest ratings (ties broken by a hash of the index and `salt`),
/// leaving at least one actor out.
fn pick_spies(ratings: &[f32], n: usize, salt: u32) -> Vec<usize> {
    let tie = |i: usize| (i as u32 ^ salt.wrapping_mul(0x9e37_79b9)).wrapping_mul(0x85eb_ca6b) >> 7;
    let mut idx: Vec<usize> = (0..ratings.len()).collect();
    idx.sort_by(|&a, &b| ratings[b].total_cmp(&ratings[a]).then(tie(a).cmp(&tie(b))));
    idx.truncate(n.min(ratings.len().saturating_sub(1)));
    idx
}

/// Draws the round's spies. Ratings (*inferred* use of `spymode.xml`'s `SELECT_SPY`): all start
/// at `DefaultRating`; after a round last round's spies drop to `SelectedRating`, everybody
/// else rises half the way to `MaximumRating`; the highest ratings are the next spies.
fn pick_roles(cfg: &SpyCfg, round: &mut Round, seats: &Seats) -> Vec<Entity> {
    let (default, selected, max) = cfg.rating;
    let ids: Vec<(Entity, bool)> = seats.iter().map(|s| (s.e, s.spy)).collect();
    for (e, was_spy) in &ids {
        let r = round.spy.rating.entry(*e).or_insert(default);
        if round.n > 1 {
            *r = if *was_spy {
                selected
            } else {
                (*r + (max - default) / 2.0).min(max).max(cfg.min_rating)
            };
        }
    }
    let ratings: Vec<f32> = ids.iter().map(|(e, _)| round.spy.rating[e]).collect();
    let n = cfg.row(ids.len() as u32).spies as usize;
    pick_spies(&ratings, n, round.n)
        .into_iter()
        .map(|i| ids[i].0)
        .collect()
}

/// The round goes live. Spies carry the spy case (the `BAG`, a blade), frost bullets, smoke bombs
/// and flashbangs and no conventional weapon; trackers the normal guns plus stun grenades and
/// mines.
fn arm(cfg: &SpyCfg, seats: &Seats, equip: &mut MessageWriter<Equip>) {
    let row = cfg.row(seats.iter().count() as u32);
    for s in seats.iter() {
        let items: Vec<(u32, Option<u32>)> = if s.spy {
            vec![
                (SPY_BAG, None),
                (SPY_ICE, Some(row.ice)),
                (SMOKE, Some(row.smoke)),
                (FLASHBANG, Some(row.light)),
            ]
        } else {
            DEFAULT_LOADOUT
                .iter()
                .map(|&i| (i, None))
                .chain([(SPY_STUN, Some(cfg.stun)), (SPY_MINE, Some(cfg.mine))])
                .collect()
        };
        equip.write(Equip {
            actor: s.e,
            items,
            current: 0,
        });
    }
}

/// The spies' positions become known: they are Blue from now on (trackers may hurt them),
/// tagged `[SPY]`, and both sides get the banner.
fn reveal(round: &mut Round, seats: &mut Seats, commands: &mut Commands) {
    round.spy.revealed = true;
    let mut names = Vec::new();
    for mut s in &mut seats.iter_mut() {
        if !s.spy {
            continue;
        }
        let base = s.name.as_str().to_owned();
        *s.name = Name::new(format!("{base} [SPY]"));
        commands
            .entity(s.e)
            .insert((Team::Blue, VipTag(base.clone()), Located));
        names.push(base);
    }
    info!("round {}: spies located: {names:?}", round.n);
    round.notice = (
        round.t + NOTICE_SECS,
        "Your location has been compromised; avoid the Trackers to survive!".into(),
        format!(
            "The Spies' locations have been successfully triangulated; you must hurry!\n{}",
            names.join(", ")
        ),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spy_table_and_roles() {
        let xml = r#"<XML id="SpyMode"><BASE minPlayer="4"/>
            <SELECT_SPY selectSpyTime="10" DefaultRating="1000" SelectedRating="500" MinimumRating="500" MaximumRating="1500" RounFinishWaitTime="3"/>
            <SPY_TABLE TotalCount="4" SpyCount="1" HPAP="50" BAG="1" LIGHT="2" ICE="4" SMOKE="8"/>
            <SPY_TABLE TotalCount="5" SpyCount="1" HPAP="100" BAG="1" LIGHT="3" ICE="5" SMOKE="10"/>
            <TRACER_TABLE STUN="2" MINE="10"/></XML>"#;
        let cfg = parse_spy(xml).unwrap();
        assert_eq!((cfg.min_players, cfg.finish_wait, cfg.stun), (4, 3.0, 2));
        assert_eq!((cfg.mine, cfg.row(5).ice, cfg.row(4).ice), (10, 5, 4));
        assert_eq!(cfg.rating, (1000.0, 500.0, 1500.0));
        assert_eq!(cfg.row(5).hpap, 100.0);
        // Out-of-range player counts use the nearest row.
        assert_eq!((cfg.row(2).total, cfg.row(40).total), (4, 5));
        assert!(parse_spy("<XML><BASE minPlayer=\"4\"/></XML>").is_err());
        // The highest ratings become spies, never everybody.
        let picked = pick_spies(&[1000.0, 500.0, 1500.0, 1000.0], 2, 1);
        assert!(picked.contains(&2) && !picked.contains(&1) && picked.len() == 2);
        assert_eq!(pick_spies(&[1.0, 1.0], 5, 0).len(), 1);
    }
}
