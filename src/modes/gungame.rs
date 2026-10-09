//! Gun Game (`--mode gungame`): free for all up a fixed weapon ladder. Every actor starts on
//! step 1; each kill moves the killer one step up and swaps its weapon at once, a melee kill
//! also drops the victim one step, and a kill from the last step wins the match. Port design
//! (no retail data): the order and the rules are *inferred*, see `docs/formats.md`, "Game modes".

use super::kind_item;
use crate::{
    actor::ActorData,
    combat::is_melee,
    game::{Arsenal, Equip, Killed, Loadout, Player, Team},
    item::{Items, WeaponKind},
    session::{Clock, freeze},
};
use bevy::prelude::*;

/// The ladder, first step first: heavy guns down to pistols, then the blades (*inferred*).
/// The last step's weapon is also every actor's melee backup.
const LADDER: [WeaponKind; 12] = [
    WeaponKind::Rocket,
    WeaponKind::MachineGun,
    WeaponKind::Rifle,
    WeaponKind::Shotgun,
    WeaponKind::SmgX2,
    WeaponKind::Smg,
    WeaponKind::RevolverX2,
    WeaponKind::Revolver,
    WeaponKind::PistolX2,
    WeaponKind::Pistol,
    WeaponKind::Dagger,
    WeaponKind::Katana,
];

/// The ladder as zitem ids (it is also the [`Arsenal`], so every actor spawns with all of
/// them). `GUNZ_GUNGAME_STEPS=N` keeps only the first N-1 steps and the last, for short test
/// matches.
pub(super) fn ladder(items: &Items) -> Vec<u32> {
    let mut l: Vec<u32> = LADDER.iter().filter_map(|k| kind_item(items, *k)).collect();
    let short = std::env::var("GUNZ_GUNGAME_STEPS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .filter(|&n| n >= 2 && n < l.len());
    if let Some(n) = short {
        let last = l.pop().unwrap_or_default();
        l.truncate(n - 1);
        l.push(last);
    }
    l
}

/// An actor's ladder step (0-based).
#[derive(Component)]
pub(super) struct Step(usize);

/// Steps after a kill: (killer's, victim's, the killer won). A kill from the `last` step wins;
/// a melee kill demotes the victim (not below the first step).
fn advance(killer: usize, victim: usize, melee: bool, last: usize) -> (usize, usize, bool) {
    (
        (killer + 1).min(last),
        victim.saturating_sub(usize::from(melee)),
        killer >= last,
    )
}

/// The step's weapon, plus the last step's blade as backup unless that is the step itself.
fn arm(equip: &mut MessageWriter<Equip>, ladder: &[u32], actor: Entity, step: usize) {
    let mut items = vec![(ladder[step], None)];
    if step + 1 < ladder.len() {
        items.push((ladder[ladder.len() - 1], None));
    }
    equip.write(Equip {
        actor,
        items,
        current: 0,
    });
}

#[allow(clippy::too_many_arguments)]
pub(super) fn gungame(
    mut commands: Commands,
    ladder: Res<Arsenal>,
    data: Res<ActorData>,
    mut killed: MessageReader<Killed>,
    new: Query<Entity, (Added<Loadout>, Without<Step>)>,
    mut steps: Query<(&mut Step, &Name, Has<Player>)>,
    mut equip: MessageWriter<Equip>,
    mut clock: ResMut<Clock>,
    mut vtime: ResMut<Time<Virtual>>,
) {
    let ladder = &ladder.0;
    let Some(last) = ladder.len().checked_sub(1) else {
        return;
    };
    let title = |step: usize| {
        data.items
            .get(ladder[step])
            .and_then(|i| i.name.as_deref())
            .unwrap_or("?")
    };
    for e in &new {
        // `Team::Duel(0)` for all: actors of one arena are enemies (`game::friendly`), so the
        // bots fight each other too, not just the player.
        commands.entity(e).insert((Step(0), Team::Duel(0)));
        arm(&mut equip, ladder, e, 0);
    }
    for k in killed.read() {
        if k.killer == k.victim || clock.over.is_some() {
            continue;
        }
        let melee = data
            .items
            .get(k.item)
            .and_then(|i| i.weapon.as_ref())
            .is_some_and(|w| is_melee(w.kind));
        let (Ok(a), Ok(b)) = (steps.get(k.killer), steps.get(k.victim)) else {
            continue;
        };
        let (up, down, won) = advance(a.0.0, b.0.0, melee, last);
        let (killer, you, was, victim) = (a.1.to_string(), a.2, b.0.0, b.1.to_string());
        if won {
            info!(
                "gungame: {killer} wins with the last step ({})",
                title(last)
            );
            clock.over = Some(if you { "VICTORY" } else { "DEFEAT" }.into());
            freeze(&mut commands, &mut vtime, true);
            return;
        }
        if let Ok(mut s) = steps.get_mut(k.killer) {
            s.0.0 = up;
        }
        info!(
            "gungame: {killer} promoted to step {}/{} ({})",
            up + 1,
            last + 1,
            title(up)
        );
        arm(&mut equip, ladder, k.killer, up);
        if down != was {
            if let Ok(mut s) = steps.get_mut(k.victim) {
                s.0.0 = down;
            }
            info!("gungame: {victim} demoted to step {}", down + 1);
            arm(&mut equip, ladder, k.victim, down);
        }
    }
    let n = last + 1;
    let me = steps.iter().find(|s| s.2).map_or(1, |s| s.0.0 + 1);
    let lead = steps
        .iter()
        .max_by_key(|s| s.0.0)
        .map_or(String::new(), |s| {
            format!("LEADER {} {}/{n}", s.1, s.0.0 + 1)
        });
    let text = format!("STEP {me}/{n}   {lead}");
    if clock.note != text {
        clock.note = text;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn promotion_demotion_win() {
        // A gun kill promotes the killer only.
        assert_eq!(advance(0, 3, false, 4), (1, 3, false));
        // A melee kill also demotes the victim, but not below the first step.
        assert_eq!(advance(2, 3, true, 4), (3, 2, false));
        assert_eq!(advance(2, 0, true, 4), (3, 0, false));
        // The step before the last reaches it without winning; a kill from the last one wins.
        assert_eq!(advance(3, 1, false, 4), (4, 1, false));
        assert_eq!(advance(4, 1, false, 4), (4, 1, true));
    }
}
