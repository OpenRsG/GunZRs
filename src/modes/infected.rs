//! Infected (`--mode infected`): one random actor turns zombie when a round goes live, a
//! survivor a zombie kills respawns as a zombie, and the round is won by the survivors at the
//! timer or by the zombies when nobody is left. The mode is the port's own design from the
//! public feature description; every number is *inferred* (`docs/formats.md`, "Infected").
//! Zombies are Red and survivors Blue, so `friendly()` and the bots' targeting need no change.

use super::*;
use crate::game::{Mods, Push};

/// A zombie's health and armour (*inferred*: twice the normal pair).
const ZOMBIE_VITALS: (f32, f32) = (200.0, 100.0);
/// A zombie's run speed relative to a melee runner (*inferred*).
const ZOMBIE_RUN: f32 = 1.2;
/// Extra horizontal knockback of a zombie's hit, m/s (*inferred*; the blades' own flinch is 1.5
/// and knockdown 3.5).
const ZOMBIE_PUSH: f32 = 6.0;
/// Seconds a dead zombie waits to rise again (*inferred*).
const ZOMBIE_RESPAWN: f32 = 3.0;
const TAG: &str = " [ZOMBIE]";

/// A zombie and the loadout it gets back as a survivor next round.
#[derive(Component)]
pub struct Zombie {
    kit: Vec<u32>,
    current: usize,
}

/// `Some(winner)` once the round is decided (the same shape as `modes::outcome`): zombies
/// win when no survivor is left, survivors when the timer runs out with one standing. Nothing
/// is decided before patient zero exists.
fn outcome(survivors: usize, zombies: usize, timeup: bool) -> Option<(Option<Side>, bool)> {
    match (survivors, zombies, timeup) {
        (_, 0, _) => None,
        (0, _, _) => Some((Some(Side::Red), false)),
        (_, _, true) => Some((Some(Side::Blue), true)),
        _ => None,
    }
}

/// The side that wins the round of `rules`/`round` given the actors' state.
pub(super) fn decide(rules: &Rules, round: &Round, seats: &Seats) -> Option<(Option<Side>, bool)> {
    let count = |t: Team, alive: bool| {
        seats
            .iter()
            .filter(|s| s.team == Some(&t) && (!alive || s.dead.is_none()))
            .count()
    };
    outcome(
        count(Team::Blue, true),
        count(Team::Red, false),
        round.t >= rules.round_secs,
    )
}

fn name_of(n: &Name) -> &str {
    n.as_str().trim_end_matches(TAG)
}

/// Turns `e` into a zombie: Red, a blade only, more health, faster, tagged.
#[allow(clippy::too_many_arguments)]
fn infect(
    commands: &mut Commands,
    equip: &mut MessageWriter<Equip>,
    data: &ActorData,
    e: Entity,
    name: &mut Name,
    vitals: &mut Vitals,
    load: &Loadout,
    full: bool,
    why: &str,
) {
    let melee = load.slots.iter().find(|s| {
        data.items
            .get(s.item)
            .and_then(|i| i.weapon.as_ref())
            .is_some_and(|w| is_melee(w.kind))
    });
    if let Some(m) = melee {
        equip.write(Equip {
            actor: e,
            items: vec![(m.item, None)],
            current: 0,
        });
    }
    commands.entity(e).insert((
        Team::Red,
        Zombie {
            kit: load.slots.iter().map(|s| s.item).collect(),
            current: load.current,
        },
        Mods {
            run: ZOMBIE_RUN,
            ..default()
        },
    ));
    (vitals.max_hp, vitals.max_ap) = ZOMBIE_VITALS;
    if full {
        (vitals.hp, vitals.ap) = ZOMBIE_VITALS;
    }
    info!("infected: {} -> zombie ({why})", name_of(name));
    *name = Name::new(format!("{}{TAG}", name_of(name)));
}

/// Draws patient zero when a round goes live, turns the survivors a zombie kills, and makes
/// everyone a survivor again (loadout back) when a new round begins.
#[allow(clippy::too_many_arguments)]
pub(super) fn spread(
    mut commands: Commands,
    mut seed: Local<u32>,
    mut picked: Local<u32>,
    round: Res<Round>,
    data: Res<ActorData>,
    mut new_round: MessageReader<NewRound>,
    mut killed: MessageReader<Killed>,
    mut equip: MessageWriter<Equip>,
    mut actors: Query<
        (
            Entity,
            &mut Name,
            &mut Vitals,
            &Loadout,
            Option<&Zombie>,
            Has<Dead>,
        ),
        With<Score>,
    >,
) {
    if new_round.read().count() > 0 {
        for (e, mut name, mut v, _, zombie, _) in &mut actors {
            commands.entity(e).insert(Team::Blue);
            let Some(z) = zombie else { continue };
            commands.entity(e).remove::<(Zombie, Mods)>();
            (v.max_hp, v.max_ap) = NORMAL_VITALS;
            (v.hp, v.ap) = NORMAL_VITALS;
            *name = Name::new(name_of(&name).to_owned());
            equip.write(Equip {
                actor: e,
                items: z.kit.iter().map(|&i| (i, None)).collect(),
                current: z.current,
            });
        }
    }
    if round.phase != Phase::Live {
        killed.clear();
        return;
    }
    if *picked != round.n {
        *picked = round.n;
        let mut alive: Vec<Entity> = actors.iter().filter(|a| !a.5).map(|a| a.0).collect();
        alive.sort_unstable();
        if *seed == 0 {
            // like the quest roll: `GUNZ_SEED=N` fixes it, otherwise the clock; small seeds
            // give alike first xorshift outputs, so scramble
            let s: u32 = std::env::var("GUNZ_SEED")
                .ok()
                .and_then(|s| s.parse().ok())
                .unwrap_or_else(|| crate::profile::wall().subsec_nanos().max(1));
            *seed = s.wrapping_mul(0x9E37_79B1) | 1;
        }
        if !alive.is_empty() {
            let e = alive[(rnd(&mut seed) * alive.len() as f32) as usize % alive.len()];
            if let Ok((_, mut name, mut v, load, ..)) = actors.get_mut(e) {
                infect(
                    &mut commands,
                    &mut equip,
                    &data,
                    e,
                    &mut name,
                    &mut v,
                    load,
                    true,
                    "patient zero",
                );
            }
        }
    }
    for k in killed.read() {
        let Ok(killer) = actors.get(k.killer) else {
            continue;
        };
        if k.killer == k.victim || killer.4.is_none() {
            continue;
        }
        let killer = name_of(killer.1).to_owned();
        if let Ok((_, mut name, mut v, load, None, _)) = actors.get_mut(k.victim) {
            infect(
                &mut commands,
                &mut equip,
                &data,
                k.victim,
                &mut name,
                &mut v,
                load,
                false,
                &format!("bitten by {killer}"),
            );
        }
    }
}

/// Zombies' hits knock the victim back, dead zombies rise at the spawn farthest from the
/// survivors, and the HUD line shows the counts.
pub(super) fn upkeep(
    spawns: Res<Spawns>,
    round: Res<Round>,
    mut clock: ResMut<Clock>,
    mut commands: Commands,
    mut hits: MessageReader<Damage>,
    pushes: Query<&Push>,
    places: Query<&Transform>,
    actors: Query<(&Transform, Has<Zombie>, Has<Dead>), With<Score>>,
    mut fallen: Query<(Entity, &mut Dead), With<Zombie>>,
    zombies: Query<(), With<Zombie>>,
) {
    for d in hits.read() {
        let (Ok(from), Ok(to)) = (places.get(d.attacker), places.get(d.target)) else {
            continue;
        };
        if round.phase != Phase::Live
            || d.amount <= 0.0
            || !zombies.contains(d.attacker)
            || zombies.contains(d.target)
        {
            continue;
        }
        let away = (to.translation - from.translation) * Vec3::new(1.0, 0.0, 1.0);
        let up = pushes.get(d.target).map_or(0.0, |p| p.0.y);
        commands
            .entity(d.target)
            .insert(Push(away.normalize_or_zero() * ZOMBIE_PUSH + Vec3::Y * up));
    }
    if round.phase == Phase::Live {
        let mut others: Option<Vec<Vec3>> = None;
        for (e, mut dead) in &mut fallen {
            if dead.respawn < HOLD / 2.0 {
                continue;
            }
            dead.respawn = ZOMBIE_RESPAWN;
            let others = others.get_or_insert_with(|| {
                actors
                    .iter()
                    .filter(|a| !a.1 && !a.2)
                    .map(|a| a.0.translation)
                    .collect()
            });
            let (pos, dir) = pick(&spawns.solo, others);
            commands.entity(e).insert(SpawnAt { pos, dir });
        }
    }
    let zombie_count = actors.iter().filter(|a| a.1).count();
    let survivors = actors.iter().filter(|a| !a.1 && !a.2).count();
    let text = format!(
        "SURVIVORS {survivors}   ZOMBIES {zombie_count}   ROUND {}",
        round.n.max(1)
    );
    if clock.note != text {
        clock.note = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_is_won_by_the_last_side_standing() {
        // before patient zero nothing is decided, even at the timer
        assert!(outcome(4, 0, false).is_none());
        assert!(outcome(4, 0, true).is_none());
        // a running round
        assert!(outcome(3, 1, false).is_none());
        // everyone infected: zombies win before the timer
        assert!(outcome(0, 4, false) == Some((Some(Side::Red), false)));
        // a survivor at the timer: survivors win
        assert!(outcome(1, 3, true) == Some((Some(Side::Blue), true)));
        // nobody left to survive at the timer is still the zombies'
        assert!(outcome(0, 2, true) == Some((Some(Side::Red), false)));
    }
}
