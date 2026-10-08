//! Spy-mode items besides the stock grenades (`docs/formats.md`, "Spy"): the effects of frost
//! bullets and stun grenades, and the antipersonnel mine (placed by `projectile::launch`,
//! triggered here by proximity, then a blast like a frag grenade).

use crate::{
    game::{Afflict, Bot, Dead, NewRound, Player, Team, Vitals, friendly},
    item::{SPY_MINE, WeaponKind},
    modes::Spy,
    projectile::Projectile,
};
use bevy::prelude::*;

/// A frost bullet slows to half for 7 s. **Inferred**: the retail Slow skill (`zskill.xml` 151 / 165,
/// `mod.speed` 50, `effecttime` 7000) is the only slow in the data.
pub const FROST_SLOW: f32 = 0.5;
pub const FROST_SECS: f32 = 7.0;
/// A stun grenade stuns for 3 s. **Inferred** from the retail Stun skill (`zskill.xml` 351,
/// `effecttime` 3000); tip 2206 only says "briefly".
pub const STUN_SECS: f32 = 3.0;
/// A mine goes off when an enemy comes this close (m, flat). **Inferred**.
pub const MINE_TRIGGER: f32 = 1.5;
/// Seconds a mine needs to arm, so that its layer can walk off it. **Inferred**.
pub const MINE_ARM: f32 = 1.0;

pub fn frost(target: Entity, by: Entity) -> Afflict {
    Afflict {
        target,
        by,
        secs: FROST_SECS,
        slow: FROST_SLOW,
        stun: false,
        root: false,
        dot: 0.0,
    }
}

pub fn stun(target: Entity, by: Entity) -> Afflict {
    Afflict {
        target,
        by,
        secs: STUN_SECS,
        slow: 1.0,
        stun: true,
        root: false,
        dot: 0.0,
    }
}

/// A placed mine (its entity sits on the floor).
#[derive(Component)]
pub struct Mine {
    owner: Entity,
    arm: f32,
}

impl Mine {
    pub fn new(owner: Entity) -> Self {
        Self {
            owner,
            arm: MINE_ARM,
        }
    }
}

pub struct SpyPlugin;

impl Plugin for SpyPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (trigger, show, clear));
    }
}

type Sides<'w, 's> = Query<'w, 's, (Option<&'static Team>, Has<Bot>)>;

/// Whether an enemy of the mine's layer stands within [`MINE_TRIGGER`] of `at`.
fn tripped(at: Vec3, owner: Entity, sides: &Sides, who: &[(Entity, Vec3)]) -> bool {
    let mine = sides.get(owner).ok().map(|(t, b)| (t.copied(), b));
    who.iter().any(|&(e, p)| {
        let d = p - at;
        e != owner
            && Vec2::new(d.x, d.z).length() < MINE_TRIGGER
            && d.y.abs() < 1.5
            && !sides
                .get(e)
                .is_ok_and(|(t, b)| mine.is_some_and(|m| friendly(m, (t.copied(), b))))
    })
}

/// Arms mines and sets off those with an enemy close: the mine becomes a [`Projectile`] that
/// detonates at once (same blast, radius and knockback as the item's frag stand-in).
fn trigger(
    mut commands: Commands,
    time: Res<Time>,
    mut mines: Query<(Entity, &GlobalTransform, &mut Mine)>,
    actors: Query<(Entity, &GlobalTransform), (With<Vitals>, Without<Dead>)>,
    sides: Sides,
) {
    let who: Vec<(Entity, Vec3)> = actors.iter().map(|(e, g)| (e, g.translation())).collect();
    for (e, g, mut m) in &mut mines {
        m.arm -= time.delta_secs();
        let at = g.translation();
        if m.arm > 0.0 || !tripped(at, m.owner, &sides, &who) {
            continue;
        }
        info!("t={:.2} mine: goes off at {at:.1}", time.elapsed_secs());
        commands.spawn((
            Projectile::blast(WeaponKind::Mine, SPY_MINE, m.owner),
            Transform::from_translation(at + Vec3::Y * 0.3),
            Visibility::Hidden,
        ));
        commands.entity(e).despawn();
    }
}

/// "The Spy can also see mines that have been installed" (tip 2207): mines show to a spy
/// player, and to the player who laid them (**inferred**: you know where you put them).
fn show(
    mut mines: Query<(&Mine, &mut Visibility)>,
    player: Query<(Entity, Has<Spy>), With<Player>>,
) {
    let (me, spy) = player.single().unwrap_or((Entity::PLACEHOLDER, false));
    for (m, mut v) in &mut mines {
        *v = if spy || m.owner == me {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
}

/// Mines last one round.
fn clear(
    mut commands: Commands,
    mut rounds: MessageReader<NewRound>,
    mines: Query<Entity, With<Mine>>,
) {
    if rounds.read().count() > 0 {
        for e in &mines {
            commands.entity(e).despawn();
        }
    }
}
