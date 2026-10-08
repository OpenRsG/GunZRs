//! Game modes beyond plain deathmatch (`menu::Mode`): melee-only gladiator loadouts, round
//! modes (Elimination, Assassinate, Duel: nobody respawns until the round is decided, a ready
//! countdown, a round-win screen, spectating while dead), spawn protection, team/duel spawn
//! points and the training dummies. Match limits and the end screen are `session.rs`.
//! Constants marked *inferred* are not in the retail data (`docs/formats.md`, "Game modes").

use crate::{
    actor::{ActorSpawner, ActorSpec, PlayerSetup},
    bot::BotAhead,
    col::MapCollision,
    game::{
        Dead, Loadout, NewRound, Player, Protected, Score, SpawnAt, Spectate, Team, Vip, Vitals,
    },
    level::Level,
    menu::Mode,
    session::{Clock, HOLD, Rules},
    view::{SCALE, to_bevy},
};
use bevy::{
    ecs::query::QueryData,
    prelude::*,
    text::Justify,
    ui::{GlobalZIndex, UiTargetCamera},
};
use std::collections::VecDeque;

/// Seconds the round-win screen stays before the next round starts (*inferred*).
const OVER_SECS: f32 = 4.0;
/// Seconds "FIGHT!" stays up after the countdown.
const FIGHT_SECS: f32 = 1.5;
/// Training dummies and how far in front of the player they stand (metres).
const DUMMY_AT: [f32; 4] = [4.0, 6.5, 9.0, 12.0];

pub struct ModesPlugin;

impl Plugin for ModesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Round>()
            .init_resource::<Spectate>()
            .add_message::<NewRound>()
            .add_systems(Startup, spawn_table.run_if(resource_exists::<Rules>))
            .add_systems(
                PostStartup,
                dummies.run_if(
                    resource_exists::<Rules>.and_then(|r: Res<Rules>| r.mode == Mode::Training),
                ),
            )
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
                )
                    .chain()
                    .run_if(resource_exists::<Rules>),
            );
    }
}

/// Headless runs: the player dies when the match clock reaches this many seconds.
#[derive(Resource)]
pub struct DieAt(pub f32);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// Countdown; everybody stands at their spawn, protected.
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
#[derive(Resource)]
pub struct Round {
    /// 1-based; 0 before the first round starts.
    pub n: u32,
    pub phase: Phase,
    /// Seconds in the current phase.
    pub t: f32,
    /// Rounds won by Red / Blue (team modes).
    pub wins: [u32; 2],
    /// The round-win screen's headline and second line.
    title: String,
    detail: String,
    /// Duel: waiting line; the first two fight (Red / Blue), the winner stays in front.
    queue: VecDeque<Entity>,
}

impl Default for Round {
    fn default() -> Self {
        Self {
            n: 0,
            phase: Phase::Ready,
            t: 0.0,
            wins: [0; 2],
            title: String::new(),
            detail: String::new(),
            queue: VecDeque::new(),
        }
    }
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

fn spawn_table(mut commands: Commands, level: Res<Level>) {
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
        for e in revived.read() {
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
    vitals: &'static Vitals,
    score: &'static mut Score,
    vip: Option<&'static VipTag>,
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
/// the duel two fighters. Builds the duel queue (the player first).
fn can_play(rules: &Rules, round: &mut Round, seats: &Seats) -> bool {
    let ok = if rules.mode == Mode::Duel {
        let mut ids: Vec<(bool, Entity)> = seats.iter().map(|s| (!s.player, s.e)).collect();
        ids.sort_unstable();
        round.queue = ids.into_iter().map(|i| i.1).collect();
        round.queue.len() >= 2
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
/// they are.
fn begin(
    rules: &Rules,
    spawns: &Spawns,
    round: &mut Round,
    keep: bool,
    commands: &mut Commands,
    seats: &mut Seats,
) {
    round.n += 1;
    round.phase = Phase::Ready;
    round.t = 0.0;
    let duel = rules.mode == Mode::Duel;
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
        } else {
            s.team.copied()
        };
        if let Some(t) = team {
            members[Side::of(t) as usize].push(s.e);
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
/// its fighter is dead); at the time limit the side with more actors alive (duel: more
/// health + armour) wins.
fn outcome(rules: &Rules, round: &Round, seats: &Seats) -> Option<(Option<Side>, bool)> {
    let duel = round.duelists().filter(|_| rules.mode == Mode::Duel);
    let alive = |t: Team| {
        seats
            .iter()
            .filter(|s| s.team == Some(&t) && s.dead.is_none())
            .count()
    };
    let (red_out, blue_out) = match duel {
        Some((a, b)) => {
            let gone = |e| seats.get(e).is_ok_and(|s| s.dead.is_some());
            (gone(a), gone(b))
        }
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
    let strength = |side: Side| match duel {
        Some((a, b)) => seats
            .get(if side == Side::Red { a } else { b })
            .map_or(0.0, |s| s.vitals.hp + s.vitals.ap),
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

/// Scores the round, writes the round-win screen text and (duel) rotates the queue.
fn conclude(
    rules: &Rules,
    round: &mut Round,
    winner: Option<Side>,
    by_time: bool,
    seats: &mut Seats,
) {
    let duel = round.duelists().filter(|_| rules.mode == Mode::Duel);
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
            let next = round
                .duelists()
                .map(|(p, q)| format!("Next: {} vs {}", name(seats, p), name(seats, q)))
                .unwrap_or_default();
            round.detail = match w {
                Some(w) => format!(
                    "{} {}    {next}",
                    who(w),
                    if by_time {
                        "wins on health"
                    } else {
                        "wins the duel"
                    }
                ),
                None => format!("Both go to the back of the line    {next}"),
            };
        }
        (None, w) => {
            if let Some(w) = w {
                round.wins[w as usize] += 1;
            }
            round.detail = format!(
                "{}    RED {} : {} BLUE",
                match w {
                    Some(w) => format!("{} wins the round", who(w)),
                    None => "Nobody wins the round".into(),
                },
                round.wins[0],
                round.wins[1]
            );
        }
    }
    round.phase = Phase::Over;
    round.t = 0.0;
}

/// The round flow: Ready (countdown) -> Live -> Over (round-win screen) -> the next round,
/// until `Rules::kill_limit` round wins (the clock then ends the match).
#[allow(clippy::too_many_arguments)]
fn rounds(
    time: Res<Time>,
    rules: Res<Rules>,
    clock: Res<Clock>,
    spawns: Res<Spawns>,
    setup: Option<Res<PlayerSetup>>,
    ahead: Option<Res<BotAhead>>,
    mut round: ResMut<Round>,
    mut commands: Commands,
    mut new_round: MessageWriter<NewRound>,
    mut seats: Seats,
) {
    if clock.over.is_some() || round.phase == Phase::Done {
        return;
    }
    round.t += time.delta_secs();
    match round.phase {
        Phase::Ready if round.n == 0 => {
            if can_play(&rules, &mut round, &seats) {
                let keep = setup.is_some_and(|s| s.at.is_some()) || ahead.is_some();
                begin(&rules, &spawns, &mut round, keep, &mut commands, &mut seats);
                new_round.write(NewRound);
                info!("round 1 begins ({})", rules.mode.name());
            }
        }
        Phase::Ready if round.t >= rules.ready => {
            round.phase = Phase::Live;
            round.t = 0.0;
        }
        Phase::Live => {
            if let Some((winner, by_time)) = outcome(&rules, &round, &seats) {
                conclude(&rules, &mut round, winner, by_time, &mut seats);
                info!("round {} over: {} | {}", round.n, round.title, round.detail);
            }
        }
        Phase::Over if round.t >= OVER_SECS => {
            let top = if rules.mode == Mode::Duel {
                seats.iter().map(|s| s.score.kills).max().unwrap_or(0)
            } else {
                round.wins[0].max(round.wins[1])
            };
            if rules.kill_limit.is_some_and(|n| top >= n) {
                round.phase = Phase::Done;
            } else {
                begin(
                    &rules,
                    &spawns,
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
    rules: Res<Rules>,
    spec: Res<Spectate>,
    names: Query<&Name>,
    vips: Query<(&Name, Has<Player>, Option<&Team>), With<Vip>>,
    player: Query<Option<&Team>, With<Player>>,
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
        Phase::Ready if round.n > 0 => {
            let mine = player.single().ok().flatten();
            let intro = match rules.mode {
                Mode::Duel => round.duelists().map_or(String::new(), |(a, b)| {
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
                _ => "Eliminate the other team - no respawn until the round ends".into(),
            };
            let left = (rules.ready - round.t).ceil().max(1.0);
            (
                format!("ROUND {}", round.n),
                format!("{intro}\n{left}"),
                Color::WHITE,
                Color::NONE,
            )
        }
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
