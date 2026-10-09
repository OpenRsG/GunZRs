//! Dynamic Duels (`--mode dynduel`): one room, several one-on-one duels at the same time. The
//! winner of a duel stays, the loser queues (and watches), the next in the queue challenges the
//! winner. Port design (no retail data; the retail duel is one arena only): every arena is a
//! "phase" of the same map. A fighter carries `Team::Duel(arena)`, which makes actors of other
//! arenas friendly to it (no damage, bullets and rockets pass through, bots ignore them, no body
//! collision, `game::friendly`/`apart`) and this module hides them from the camera. Arena
//! count is players / 2 (rounded down); an odd player starts in the queue. Constants marked
//! *inferred* are not in the retail data, see `docs/formats.md`, "Game modes".

use super::{Spawns, pick};
use crate::{
    game::{Dead, Player, Protected, Score, SpawnAt, Spectate, Team, Vitals},
    session::{Clock, HOLD, Rules},
};
use bevy::prelude::*;
use std::collections::{HashMap, VecDeque};

/// Seconds an arena waits after a duel before the next one starts (*inferred*): the loser's
/// body lies where it fell, then the challenger and the winner appear at their sides.
const NEXT_SECS: f32 = 3.0;
/// Seconds before the first duels start (*inferred*).
const START_SECS: f32 = 1.0;
/// Seconds after a duel starts before deaths count: the fighters' respawn `Dead` is still on.
const SETTLE_SECS: f32 = 0.5;

#[derive(Clone, Copy)]
enum Stage {
    /// Seconds until the arena's (next) duel starts.
    Wait(f32),
    /// Seconds into the duel.
    Fight(f32),
}

struct Arena {
    fighters: [Entity; 2],
    stage: Stage,
    /// The loser of the last duel, lying visible until the next duel starts.
    out: Option<Entity>,
}

/// Who fights where and who waits.
#[derive(Default)]
pub(super) struct Duels {
    arenas: Vec<Arena>,
    /// Losers, longest wait first. Not in any arena (a loser alone in the queue is its own
    /// challenger: the rematch).
    queue: VecDeque<Entity>,
    /// Duels won in a row.
    streak: HashMap<Entity, u32>,
    /// The arena the camera shows while the player is not fighting.
    watch: usize,
}

impl Duels {
    /// `players / 2` arenas, filled in order; the odd one out queues.
    fn new(players: &[Entity]) -> Self {
        let n = players.len() / 2;
        Self {
            arenas: players
                .chunks_exact(2)
                .map(|p| Arena {
                    fighters: [p[0], p[1]],
                    stage: Stage::Wait(START_SECS),
                    out: None,
                })
                .collect(),
            queue: players[2 * n..].iter().copied().collect(),
            ..default()
        }
    }

    fn arena_of(&self, e: Entity) -> Option<usize> {
        self.arenas.iter().position(|a| a.fighters.contains(&e))
    }

    /// `winner` of arena `a` stays, the other fighter goes to the back of the queue and the
    /// queue's head takes its place. Returns (loser, challenger).
    fn decide(&mut self, a: usize, winner: Entity) -> (Entity, Entity) {
        let arena = &mut self.arenas[a];
        let slot = usize::from(arena.fighters[0] == winner);
        let loser = arena.fighters[slot];
        self.queue.push_back(loser);
        let challenger = self.queue.pop_front().unwrap_or(loser);
        arena.fighters[slot] = challenger;
        arena.stage = Stage::Wait(NEXT_SECS);
        arena.out = Some(loser);
        *self.streak.entry(winner).or_default() += 1;
        self.streak.remove(&loser);
        (loser, challenger)
    }
}

/// Starts arena `a`'s duel: the loser's body goes, both fighters respawn at opposite sides.
fn start(commands: &mut Commands, spawns: &Spawns, a: usize, arena: &mut Arena) {
    if let Some(l) = arena.out.take()
        && !arena.fighters.contains(&l)
    {
        commands.entity(l).remove::<Team>();
    }
    let red = pick(&spawns.red, &[]);
    let blue = pick(&spawns.blue, &[red.0]);
    for (e, (pos, dir)) in arena.fighters.into_iter().zip([red, blue]) {
        commands.entity(e).insert((
            Team::Duel(a as u8),
            Dead { respawn: 0.0 },
            SpawnAt { pos, dir },
        ));
    }
    arena.stage = Stage::Fight(0.0);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn dynduel(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    rules: Res<Rules>,
    spawns: Res<Spawns>,
    mut commands: Commands,
    mut state: Local<Option<Duels>>,
    mut clock: ResMut<Clock>,
    mut spec: ResMut<Spectate>,
    names: Query<&Name>,
    mut scores: Query<&mut Score>,
    mut dead: Query<&mut Dead>,
    mut actors: Query<
        (
            Entity,
            Option<&Team>,
            Has<Player>,
            Has<Dead>,
            Has<Protected>,
            &Vitals,
            &mut Visibility,
        ),
        With<Score>,
    >,
) {
    let name = |e| names.get(e).map_or("?", |n| n.as_str());
    if state.is_none() {
        let mut ids: Vec<(bool, Entity)> = actors.iter().map(|a| (!a.2, a.0)).collect();
        if ids.len() < 2 {
            return;
        }
        ids.sort_unstable();
        let ids: Vec<Entity> = ids.into_iter().map(|i| i.1).collect();
        let duels = Duels::new(&ids);
        let queued: Vec<&str> = duels.queue.iter().map(|e| name(*e)).collect();
        info!(
            "dynduel: {} players, {} arenas, queue {queued:?}",
            ids.len(),
            duels.arenas.len()
        );
        for e in &duels.queue {
            commands.entity(*e).insert(Dead { respawn: HOLD });
        }
        *state = Some(duels);
    }
    let Some(duels) = state.as_mut() else { return };
    let (now, dt) = (time.elapsed_secs(), time.delta_secs());
    let gone = |e| actors.get(e).is_ok_and(|q| q.3);
    let strength = |e| actors.get(e).map_or(0.0, |q| q.5.hp + q.5.ap);

    for a in 0..duels.arenas.len() {
        let [x, y] = duels.arenas[a].fighters;
        match duels.arenas[a].stage {
            Stage::Wait(t) if t > dt => duels.arenas[a].stage = Stage::Wait(t - dt),
            Stage::Wait(_) => {
                start(&mut commands, &spawns, a, &mut duels.arenas[a]);
                info!(
                    "t={now:.2} dynduel: arena {}: {} vs {}",
                    a + 1,
                    name(x),
                    name(y)
                );
            }
            Stage::Fight(t) => {
                duels.arenas[a].stage = Stage::Fight(t + dt);
                if t < SETTLE_SECS {
                    continue;
                }
                let timed_out = t >= rules.round_secs && !gone(x) && !gone(y);
                let winner = match (gone(x), gone(y)) {
                    (_, true) => x,
                    (true, false) => y,
                    // a time-out goes to the one with more health + armour (ties: the first)
                    (false, false) if timed_out => {
                        if strength(y) > strength(x) {
                            y
                        } else {
                            x
                        }
                    }
                    (false, false) => continue,
                };
                let (loser, challenger) = duels.decide(a, winner);
                if timed_out {
                    // credited like a kill; the loser is out of the arena alive
                    if let Ok(mut s) = scores.get_mut(winner) {
                        s.kills += 1;
                    }
                    if let Ok(mut s) = scores.get_mut(loser) {
                        s.deaths += 1;
                    }
                    commands.entity(loser).insert(Dead { respawn: HOLD });
                }
                let queued: Vec<&str> = duels.queue.iter().map(|e| name(*e)).collect();
                info!(
                    "t={now:.2} dynduel: arena {}: {} beats {} (streak {}){}; {} challenges in {NEXT_SECS} s, queue {queued:?}",
                    a + 1,
                    name(winner),
                    name(loser),
                    duels.streak[&winner],
                    if timed_out { " on time" } else { "" },
                    name(challenger)
                );
            }
        }
    }
    // Everyone out of a duel stays dead until a duel takes them in (`start` sets 0).
    for mut d in &mut dead {
        if d.respawn > 0.0 {
            d.respawn = HOLD;
        }
    }

    // What the player sees: its own arena while it fights, else the arena it watches
    // (Space or a click: the next one), through the camera of a living fighter there.
    let me = actors.iter().find(|a| a.2).map(|a| a.0);
    let mine = me.and_then(|m| duels.arena_of(m));
    let fighting = mine.is_some() && me.is_some_and(|m| !gone(m));
    if let (true, Some(a)) = (fighting, mine) {
        duels.watch = a;
    } else if (keys.just_pressed(KeyCode::Space) || mouse.just_pressed(MouseButton::Left))
        && !duels.arenas.is_empty()
    {
        duels.watch = (duels.watch + 1) % duels.arenas.len();
    }
    let view = duels.watch;
    let want = if fighting {
        None
    } else {
        duels.arenas.get(view).and_then(|ar| {
            ar.fighters
                .into_iter()
                .find(|e| Some(*e) != me && !gone(*e))
        })
    };
    if spec.0 != want {
        spec.0 = want;
    }
    for (_, team, _, _, protected, _, mut vis) in &mut actors {
        let shown = matches!(team, Some(Team::Duel(k)) if usize::from(*k) == view);
        // a protected actor blinks by itself (`protect`)
        let want = if shown {
            if protected {
                continue;
            }
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
    }
    let arenas = duels.arenas.len();
    let text = match (fighting, mine) {
        (true, Some(a)) => format!(
            "ARENA {}/{arenas}   STREAK {}",
            a + 1,
            me.and_then(|m| duels.streak.get(&m)).copied().unwrap_or(0)
        ),
        _ => {
            let queue = match me.and_then(|m| duels.queue.iter().position(|e| *e == m)) {
                Some(i) => format!("QUEUE #{}", i + 1),
                None => "YOU ARE NEXT".into(),
            };
            format!("ARENA {}/{arenas} WATCHING   {queue}", view + 1)
        }
    };
    if clock.note != text {
        clock.note = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn players(n: usize) -> Vec<Entity> {
        let mut world = World::new();
        (0..n).map(|_| world.spawn_empty().id()).collect()
    }

    #[test]
    fn arenas_are_half_the_players_and_losers_queue() {
        let p = players(5);
        let mut d = Duels::new(&p);
        assert_eq!(d.arenas.len(), 2);
        assert_eq!(d.queue, [p[4]]);
        // arena 0: p0 beats p1, the queued p4 challenges, p1 queues
        assert_eq!(d.decide(0, p[0]), (p[1], p[4]));
        assert_eq!(d.arenas[0].fighters, [p[0], p[4]]);
        assert_eq!(d.queue, [p[1]]);
        // arena 1: p3 beats p2, p1 (queued first) challenges
        assert_eq!(d.decide(1, p[3]), (p[2], p[1]));
        assert_eq!(d.arenas[1].fighters, [p[1], p[3]]);
        assert_eq!(d.queue, [p[2]]);
        // the challenger beats the winner: it keeps its side, the old winner queues
        assert_eq!(d.decide(0, p[4]), (p[0], p[2]));
        assert_eq!(d.arenas[0].fighters, [p[2], p[4]]);
        assert_eq!(d.queue, [p[0]]);
        assert_eq!((d.arena_of(p[4]), d.arena_of(p[0])), (Some(0), None));
        // streaks: p0 won once and lost, p3 won once, p4 won once
        assert_eq!(d.streak.get(&p[0]), None);
        assert_eq!(d.streak.get(&p[3]), Some(&1));
        assert_eq!(d.streak.get(&p[4]), Some(&1));
    }

    #[test]
    fn an_empty_queue_means_a_rematch() {
        let p = players(2);
        let mut d = Duels::new(&p);
        assert_eq!((d.arenas.len(), d.queue.len()), (1, 0));
        assert_eq!(d.decide(0, p[1]), (p[0], p[0]));
        assert_eq!(d.arenas[0].fighters, [p[0], p[1]]);
        assert_eq!(d.decide(0, p[1]), (p[0], p[0]));
        assert_eq!(d.streak.get(&p[1]), Some(&2));
    }
}
