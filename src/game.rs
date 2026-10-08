//! Shared gameplay contract for `gunz-play`: components and messages every gameplay module
//! (actor, collision, combat, hud) agrees on. Space is Bevy's: metres, Y up.

use bevy::prelude::*;

/// Registers the shared messages; each gameplay module adds its own plugin line here.
pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Fire>()
            .add_message::<Damage>()
            .add_message::<Killed>()
            .add_message::<ActorSound>()
            .add_message::<Blocked>()
            .add_message::<Blast>()
            .add_message::<Impact>()
            .add_message::<ActionRequest>()
            .add_plugins(crate::hud::HudPlugin)
            .add_plugins(crate::audio::AudioPlugin)
            .add_plugins(crate::actor::ActorPlugin)
            .add_plugins(crate::combat::CombatPlugin)
            .add_plugins(crate::melee::MeleePlugin)
            .add_plugins(crate::bot::BotPlugin)
            .add_plugins(crate::pickup::PickupPlugin)
            .add_plugins(crate::session::SessionPlugin)
            .add_plugins(crate::perf::PerfPlugin);
    }
}

/// An explosion/detonation at `at` (rocket, grenade). `sound` is the stem of a
/// `sound/effect/<sound>.wav` for the HUD to play there.
#[derive(Message, Clone, Debug)]
pub struct Blast {
    pub at: Vec3,
    pub sound: &'static str,
}

/// A shot or blade struck map geometry (not an actor): bullet holes, sparks, impact sounds.
/// `normal` is the unit surface normal facing the shooter; `blade` is a melee blow.
#[derive(Message, Clone, Debug)]
pub struct Impact {
    pub point: Vec3,
    pub normal: Vec3,
    pub blade: bool,
}

/// The locally controlled actor.
#[derive(Component)]
pub struct Player;

/// An AI-controlled actor.
#[derive(Component)]
pub struct Bot;

/// Side in team deathmatch. Actors without a `Team` play free-for-all: bots only target the
/// player and never hurt each other.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Team {
    Red,
    Blue,
}

/// Whether two actors (each as its `Team` and whether it is a `Bot`) are allies: no damage
/// between them (callers still let an actor hurt itself) and bots never target them.
pub fn friendly(a: (Option<Team>, bool), b: (Option<Team>, bool)) -> bool {
    match (a.0, b.0) {
        (Some(x), Some(y)) => x == y,
        _ => a.1 && b.1,
    }
}

/// Player preferences the menu writes (`gunz-play --sens`).
#[derive(Resource, Clone, Debug)]
pub struct Settings {
    /// Mouse look, radians per pixel.
    pub sensitivity: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            sensitivity: 0.0025,
        }
    }
}

/// Present while the game is frozen (pause menu, match over): the player input system writes
/// a neutral `Intent` and ignores the mouse. `Time<Virtual>` is paused at the same time.
#[derive(Resource)]
pub struct Frozen;

/// What an actor wants to do this frame. Player input or bot AI writes it; the actor
/// controller consumes it (movement, animation, weapon use).
#[derive(Component, Default, Clone, Debug)]
pub struct Intent {
    /// Desired move in the actor's yaw frame: x right, y forward, each in -1..=1.
    pub walk: Vec2,
    /// Facing yaw (radians, Bevy Y-up, 0 = -Z) and aim pitch.
    pub yaw: f32,
    pub pitch: f32,
    pub jump: bool,
    /// Primary attack held (fire / slash).
    pub attack: bool,
    pub reload: bool,
    /// Weapon slot to switch to.
    pub slot: Option<usize>,
    /// Guard held (right mouse; melee weapons only): see [`Guarding`].
    pub guard: bool,
    /// Taunt key held (the controller acts on the press): plays the weapon's `taunt` clip.
    pub taunt: bool,
}

/// Present on an actor while its melee guard is up. Combat must block melee damage against
/// it (and write [`Blocked`] so the actor plays the block animation).
#[derive(Component, Debug)]
pub struct Guarding;

/// A guarding actor blocked a melee blow; written by combat, read by the actor controller.
#[derive(Message, Clone, Debug)]
pub struct Blocked(pub Entity);

/// Asks the actor controller to play a full-body action: `clip` is looked up in the actor's
/// current weapon motion type (`attack1`, `uppercut`, `jump_slash1`, `charge`, `guard_block1`,
/// `damage`, ...). The actor stops doing whatever it did (slash, reload, tumble) and `Acting`
/// appears on it until the clip ends or a cancel (jump, dash, guard) interrupts it after
/// `cancel_from`. Unknown clips are ignored.
#[derive(Message, Clone, Debug)]
pub struct ActionRequest {
    pub actor: Entity,
    pub clip: &'static str,
    /// Playback speed (1 = as authored).
    pub speed: f32,
    pub moving: ActionMove,
    /// Seconds into the action (at `speed`) from which a jump/tumble/guard cancels it
    /// (`f32::INFINITY` = never).
    pub cancel_from: f32,
}

/// Horizontal movement while an action plays.
#[derive(Clone, Copy, Debug)]
pub enum ActionMove {
    /// Rooted.
    Locked,
    /// The actor travels as the clip's root bone does (lunges, dashes, stand-ups).
    Root,
    /// Walking with this fraction of the normal control.
    Control(f32),
}

/// On an actor while an [`ActionRequest`] plays; `time` runs 0..`secs` (playback seconds), so
/// melee can time its hit frame from it.
#[derive(Component, Clone, Debug)]
pub struct Acting {
    pub clip: &'static str,
    pub time: f32,
    pub secs: f32,
    pub cancel_from: f32,
}

/// Snapshot of an actor's motion state, written by the actor controller on every actor each
/// frame (the end state of the previous one) for melee/AI/audio to read.
#[derive(Component, Default, Clone, Debug)]
pub struct Motor {
    pub grounded: bool,
    pub vel: Vec3,
    /// Seconds since a tumble (dash) began; `None` when not tumbling.
    pub tumble: Option<f32>,
    /// Wall run or wall kick in progress.
    pub wall: bool,
    /// In the blast chain (launched, falling, lying, getting up).
    pub blast: bool,
    pub woman: bool,
}

/// Health and armour, in GunZ points.
#[derive(Component, Clone, Debug)]
pub struct Vitals {
    pub hp: f32,
    pub ap: f32,
    pub max_hp: f32,
    pub max_ap: f32,
}

/// Present while an actor is dead; `respawn` counts down in seconds.
#[derive(Component)]
pub struct Dead {
    pub respawn: f32,
}

/// Spawn protection (set by `session`): combat ignores all damage against an actor carrying
/// this; `.0` is the seconds left. The actor blinks while it lasts.
#[derive(Component, Debug)]
pub struct Protected(pub f32);

/// Where this actor respawns next (set by `session` from the map's `spawn_*` dummies: team
/// sides, duel ends, training-dummy home). The actor controller uses it instead of picking
/// the spawn point farthest from everyone, when present.
#[derive(Component, Clone, Copy, Debug)]
pub struct SpawnAt {
    pub pos: Vec3,
    pub dir: Vec3,
}

/// The actor the camera follows instead of the player (spectating while dead in a round
/// mode); `None` = the player.
#[derive(Resource, Default)]
pub struct Spectate(pub Option<Entity>);

/// Assassinate: the team's VIP (its `Name` carries a `[VIP]` tag). Bots may prefer it.
#[derive(Component, Debug)]
pub struct Vip;

/// A round of a round mode begins (everyone respawns): item spawners refill.
#[derive(Message, Clone, Copy, Debug)]
pub struct NewRound;

/// Weapon items an actor carries (zitem ids) with ammo per slot.
#[derive(Component, Clone, Debug)]
pub struct Loadout {
    pub slots: Vec<Slot>,
    pub current: usize,
}

#[derive(Clone, Debug)]
pub struct Slot {
    pub item: u32,
    /// Rounds in the magazine and in reserve; both 0 for melee.
    pub magazine: u32,
    pub reserve: u32,
}

/// An actor used its current weapon (a shot, a slash). `origin`/`dir` are the aim ray in
/// world space (camera ray for the player, eye ray for bots).
#[derive(Message, Clone, Debug)]
pub struct Fire {
    pub shooter: Entity,
    pub item: u32,
    pub origin: Vec3,
    pub dir: Vec3,
}

/// Damage dealt to an actor, owned by combat: the weapon's piercing ratio decides how it splits
/// between armour and health. `item` is the zitem id of the weapon (kill feed, piercing).
#[derive(Message, Clone, Debug)]
pub struct Damage {
    pub target: Entity,
    pub attacker: Entity,
    pub amount: f32,
    pub item: u32,
    pub point: Vec3,
    pub dir: Vec3,
}

/// Kill/death tally of an actor (put on every actor; combat increments it).
#[derive(Component, Default, Clone, Debug)]
pub struct Score {
    pub kills: u32,
    pub deaths: u32,
}

/// An actor was killed (`killer == victim` for a suicide). Kill feed source.
#[derive(Message, Clone, Debug)]
pub struct Killed {
    pub victim: Entity,
    pub killer: Entity,
    pub item: u32,
}

/// Knockback impulse (m/s, world space) inserted/added by combat on a hit actor. The actor
/// controller adds it to its velocity and removes the component.
#[derive(Component, Default, Clone, Debug)]
pub struct Push(pub Vec3);

/// Sound cue raised by the actor controller; hud owns the sound files and plays them at the
/// actor's position. (Shots and slashes sound off `Fire`, hits off `Damage`, deaths off
/// `Killed`.)
#[derive(Message, Clone, Debug)]
pub struct ActorSound {
    pub actor: Entity,
    pub cue: Cue,
}

#[derive(Clone, Debug)]
pub enum Cue {
    /// A foot hit the ground (run cycle).
    Footstep { left: bool },
    /// Reload began with weapon `item`.
    Reload { item: u32 },
    /// Trigger pulled on an empty magazine with weapon `item`.
    DryFire { item: u32 },
    /// Verbatim `sound` attribute of the animation that just started (`man_jump`, `fx_dash`).
    Anim(String),
}
