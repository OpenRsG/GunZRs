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
            .add_message::<CameraShake>()
            .add_message::<PlaySound>()
            .add_message::<Impact>()
            .add_message::<ActionRequest>()
            .add_message::<Equip>()
            .add_message::<Reward>()
            .add_message::<SpawnNpc>()
            .add_message::<NpcState>()
            .init_resource::<Routes>()
            .add_message::<QuestLoot>()
            .add_message::<Afflict>()
            .add_plugins(crate::hud::HudPlugin)
            .add_plugins(crate::audio::AudioPlugin)
            .add_plugins(crate::music::MusicPlugin)
            .add_plugins(crate::actor::ActorPlugin)
            .add_plugins(crate::combat::CombatPlugin)
            .add_plugins(crate::spy::SpyPlugin)
            .add_plugins(crate::melee::MeleePlugin)
            .add_plugins(crate::bot::BotPlugin)
            .add_plugins(crate::pickup::PickupPlugin)
            .add_plugins(crate::session::SessionPlugin)
            .add_plugins(crate::controls::ControlsPlugin)
            .add_plugins(crate::profile::ProfilePlugin)
            .add_plugins(crate::quest::QuestPlugin)
            .add_plugins(crate::clan::ClanPlugin)
            .add_plugins(crate::npc::NpcPlugin)
            .add_plugins(crate::blitz::BlitzPlugin)
            .add_plugins(crate::killcam::KillcamPlugin)
            .add_plugins(crate::net::NetPlugin)
            .add_plugins(crate::perf::PerfPlugin);
    }
}

/// XP, bounty and Blitzkrieg medals for the local profile (`profile.rs` also pays kills and the
/// match result by itself): write one for anything else that earns, e.g. a finished quest.
#[derive(Message, Clone, Copy, Debug, Default)]
pub struct Reward {
    pub xp: u32,
    pub bounty: u32,
    pub medals: u32,
}

/// Quest -> `npc.rs`: spawn one monster. `id` is a `<NPC id>` of `system/npc.xml` or an
/// `<ACTOR name>` of `npc2.xml`; `pos` the feet in Bevy metres; `hp_scale` multiplies its
/// max HP/AP; `drop` names its `droptable.xml` set (empty: the NPC's own).
#[derive(Message, Clone, Debug, Default)]
pub struct SpawnNpc {
    pub id: String,
    pub pos: Vec3,
    pub yaw: f32,
    pub hp_scale: f32,
    pub drop: String,
    pub boss: bool,
    /// Side of the monster; `None`: from the actor's name (`*_red` is Red, the rest Blue).
    pub team: Option<Team>,
    /// [`Routes`] id a soldier marches along (0: none).
    pub route: u32,
}

/// `ROUTE` id of `blitzkrieg.xml` -> its waypoints (Bevy metres), written by `blitz.rs` and read
/// by `npc.rs` for `runWaypointsAlongRoute`.
#[derive(Resource, Default)]
pub struct Routes {
    pub paths: std::collections::HashMap<u32, Vec<Vec3>>,
    /// `ENHANCE_NPC`: the share the waves' health grows by (0 = none).
    pub boost: f32,
}

/// Per-actor multipliers a mode sets (Blitzkrieg's honor upgrades, classes and buildings); combat
/// applies `dealt` of the attacker (`vs_buildings` too against a `building`) and `taken` of the
/// target (`vs_actors` too when the attacker is a player or bot), the actor controller stretches
/// the gun delay by `shot_delay` and the run speed by `run`. All 1 (`building` false) = none.
#[derive(Component, Clone, Copy, Debug)]
pub struct Mods {
    pub dealt: f32,
    pub taken: f32,
    pub vs_actors: f32,
    pub shot_delay: f32,
    pub run: f32,
    pub vs_buildings: f32,
    pub building: bool,
}

impl Default for Mods {
    fn default() -> Self {
        Self {
            dealt: 1.0,
            taken: 1.0,
            vs_actors: 1.0,
            shot_delay: 1.0,
            run: 1.0,
            vs_buildings: 1.0,
            building: false,
        }
    }
}

impl Mods {
    /// Multiplier on a blow of an attacker (given whether it is a player or bot) against `self`.
    pub fn against(&self, from_actor: bool) -> f32 {
        self.taken * if from_actor { self.vs_actors } else { 1.0 }
    }

    /// Multiplier an attacker with `self` puts on its blows against `target`.
    pub fn dealing(&self, target: Option<&Mods>) -> f32 {
        self.dealt
            * if target.is_some_and(|t| t.building) {
                self.vs_buildings
            } else {
                1.0
            }
    }
}

/// Forces a state-machine monster into its state named `state` (the `radar_*` machines'
/// `summon_zealot` reinforcements of Blitzkrieg).
#[derive(Message, Clone, Debug)]
pub struct NpcState {
    pub npc: Entity,
    pub state: String,
}

/// A spawned quest monster (hostile `Team::Blue`); `quest.rs` counts the living ones and rolls
/// `drop` where one dies.
#[derive(Component, Clone, Debug)]
pub struct Npc {
    pub id: String,
    pub drop: String,
    pub boss: bool,
    /// `npc2.xml` `type` (`barricade`, `radar`, `knifeman`, ...); empty for the classic monsters.
    pub kind: String,
}

/// The hit capsule (metres, feet at the transform origin) of an actor that is not
/// human-sized (quest monsters, `npc.rs`). Combat/melee/blasts use [`HIT_RADIUS`]-sized
/// capsules when it is absent.
///
/// [`HIT_RADIUS`]: crate::combat::HIT_RADIUS
#[derive(Component, Clone, Copy, Debug)]
pub struct HitShape {
    pub radius: f32,
    pub height: f32,
}

/// Level geometry and props of the loaded map: `quest.rs` despawns them when a sector swaps
/// the map.
#[derive(Component)]
pub struct MapEntity;

/// Items collected in a finished quest, for the profile: `zquestitem.xml` ids with counts, and
/// rented shop items (`zitem.xml` id, `rent_period` hours).
#[derive(Message, Clone, Debug)]
pub struct QuestLoot {
    pub items: Vec<(u32, u32)>,
    pub rented: Vec<(u32, u32)>,
}

/// Asks `audio.rs` to play the retail sound `sound/effect/<stem>.wav` once at `at` (metres):
/// 3D with the range `effect.xml` gives it, or 2D when that file says so. `stem` is a bare
/// file stem or the relative form the data files use (`quest/goblin/Goblin_attack`); case is
/// ignored. Unknown stems are warned about once. For a sound that follows an actor, an
/// `ActorSound { cue: Cue::Anim(stem) }` does the same at its position.
#[derive(Message, Clone, Debug)]
pub struct PlaySound {
    pub stem: String,
    pub at: Vec3,
}

/// An explosion/detonation at `at` (rocket, grenade). `sound` is the stem of a
/// `sound/effect/<sound>.wav` for the HUD to play there.
#[derive(Message, Clone, Debug)]
pub struct Blast {
    pub at: Vec3,
    pub sound: &'static str,
}

/// A heavy hit or skill shakes the player's camera when within `range` metres of `at`; `trauma`
/// (0..=1) is the strongest shake (see `hud::Shake`).
#[derive(Message, Clone, Copy, Debug)]
pub struct CameraShake {
    pub at: Vec3,
    pub trauma: f32,
    pub range: f32,
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

/// A human actor whose input arrives over the LAN (`net.rs`): on the host a joined player, on a
/// client the host's player and the other clients. Neither `Player` nor `Bot`; it aims like a
/// player.
#[derive(Component)]
pub struct Remote;

/// Side in team deathmatch. Actors without a `Team` play free-for-all: bots only target the
/// player and never hurt each other.
#[derive(Component, Clone, Copy, PartialEq, Eq, Debug)]
pub enum Team {
    Red,
    Blue,
    /// Dynamic Duels: the actor fights in this arena. Arenas share the map, so actors of
    /// different arenas are "friendly" (no damage, bullets pass through, bots ignore them) and
    /// never collide ([`apart`]); inside one arena the two fighters are enemies.
    Duel(u8),
}

/// Whether two actors are in different Dynamic Duels arenas: out of each other's reach.
pub fn apart(a: Option<Team>, b: Option<Team>) -> bool {
    matches!((a, b), (Some(Team::Duel(x)), Some(Team::Duel(y))) if x != y)
}

/// Whether two actors (each as its `Team` and whether it is a `Bot`) are allies: no damage
/// between them (callers still let an actor hurt itself) and bots never target them.
pub fn friendly(a: (Option<Team>, bool), b: (Option<Team>, bool)) -> bool {
    match (a.0, b.0) {
        (Some(Team::Duel(_)), Some(Team::Duel(_))) => apart(a.0, b.0),
        (Some(x), Some(y)) => x == y,
        _ => a.1 && b.1,
    }
}

/// Player preferences (the pause menu's toggles, `profile::OPTS`). Mouse and keys: the
/// profile's `controls::Controls`.
#[derive(Resource, Clone, Debug)]
pub struct Settings {
    /// Background-music loudness, 0..=1 (`music.rs`; **inferred** default).
    pub music: f32,
    /// Ramping kill sounds on consecutive kills (iGunZ `/killsounds` idea).
    pub kill_sounds: bool,
    /// Play the player's own `.wav` on every hit the player lands (iGunZ `/hitsound` idea).
    pub hit_sound: bool,
    /// Fixed, non-random bullet spread pattern.
    pub static_spread: bool,
    /// Teammates' HP/AP bars and ammo above their heads.
    pub team_bars: bool,
    /// Blood splatter on screen when the player is hurt.
    pub screen_blood: bool,
    /// Orbitable kill camera on the killer after death.
    pub killcam: bool,
    /// Touch-screen aim assist strength, 0 (off) ..= 1; the browser's touch controls set it.
    pub aim_assist: f32,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            music: 0.5,
            kill_sounds: true,
            hit_sound: false,
            static_spread: false,
            team_bars: true,
            screen_blood: true,
            killcam: true,
            aim_assist: 0.0,
        }
    }
}

/// Present while the game is frozen (pause menu, match over): the player input system writes
/// a neutral `Intent` and ignores the mouse. `Time<Virtual>` is paused at the same time.
#[derive(Resource)]
pub struct Frozen;

/// Present when the player plays with the browser page's touch controls (`web.rs`): the HUD
/// keeps clear of the on-screen buttons.
#[derive(Resource)]
pub struct TouchScreen;

/// A mode's pre-match screen (Blitzkrieg's class select) holds the match: the game is frozen
/// like a pause, but without the pause menu and without Esc resuming it.
#[derive(Resource)]
pub struct Hold;

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
    /// Emote clip to play (`bow`, `wave`, `cry`, `laugh`, `dance`, or `taunt`), held; the
    /// controller acts on a change to a new value (standing, free actors only).
    pub emote: Option<&'static str>,
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

/// Weapon items every actor spawns with when present (a mode's whole weapon pool: Gunman, Spy);
/// it replaces the loadout of the spawn spec. [`Equip`] then picks what an actor carries.
#[derive(Resource, Clone, Debug)]
pub struct Arsenal(pub Vec<u32>);

/// Re-picks an actor's loadout from its [`Arsenal`]: the items become its slots in that order
/// (ammo refilled, or, with `Some(n)`, exactly `n` rounds/grenades in total), `current` the
/// selected one. Unknown items are skipped.
#[derive(Message, Clone, Debug)]
pub struct Equip {
    pub actor: Entity,
    pub items: Vec<(u32, Option<u32>)>,
    pub current: usize,
}

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
    /// Share of the hit that goes to health (`0` = armour soaks it first, `1` = armour is
    /// bypassed); `None` = the weapon's own, by its `item`.
    pub pierce: Option<f32>,
}

/// Seconds between damage-over-time ticks of a [`Status`].
pub const DOT_TICK: f32 = 0.5;

/// Something that slows, stuns, roots or burns an actor (an NPC skill, a frost bullet, a stun
/// grenade, a blizzard): `actor.rs` merges it into the target's [`Status`].
#[derive(Message, Clone, Copy, Debug)]
pub struct Afflict {
    pub target: Entity,
    pub by: Entity,
    /// Seconds every effect below lasts.
    pub secs: f32,
    /// Run-speed factor (`1` = no slow).
    pub slow: f32,
    /// No moving, jumping, shooting or slashing; plays the `stun` clip.
    pub stun: bool,
    /// No moving or jumping (attacks stay allowed).
    pub root: bool,
    /// Extra damage spread evenly over `secs` (0 = none).
    pub dot: f32,
}

/// Status effects running on an actor; `actor.rs` merges [`Afflict`]s into it, ticks it and
/// removes it once everything ran out (or the actor died).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq)]
pub struct Status {
    /// Run-speed factor while `slow_left` > 0.
    pub slow: f32,
    pub slow_left: f32,
    pub stun: f32,
    pub root: f32,
    /// Damage per second, seconds left, the dealer, the time to the next tick and the damage
    /// accrued since the last one.
    pub dot: f32,
    pub dot_left: f32,
    pub dot_by: Option<Entity>,
    tick: f32,
    owed: f32,
}

impl Status {
    /// Merges `a`: slows keep the stronger factor, every timer the longer time left.
    pub fn add(&mut self, a: &Afflict) {
        if a.slow < 1.0 {
            self.slow = if self.slow_left > 0.0 {
                self.slow.min(a.slow)
            } else {
                a.slow
            };
            self.slow_left = self.slow_left.max(a.secs);
        }
        if a.stun {
            self.stun = self.stun.max(a.secs);
        }
        if a.root {
            self.root = self.root.max(a.secs);
        }
        if a.dot > 0.0 {
            if self.dot_left <= 0.0 {
                self.tick = DOT_TICK;
            }
            let rate = a.dot / a.secs.max(0.1);
            self.dot = if self.dot_left > 0.0 {
                self.dot.max(rate)
            } else {
                rate
            };
            self.dot_left = self.dot_left.max(a.secs);
            self.dot_by = Some(a.by);
        }
    }

    /// Run-speed factor.
    pub fn speed(&self) -> f32 {
        if self.slow_left > 0.0 { self.slow } else { 1.0 }
    }

    pub fn stunned(&self) -> bool {
        self.stun > 0.0
    }

    /// Cannot walk or jump (a stun roots too).
    pub fn rooted(&self) -> bool {
        self.root > 0.0 || self.stun > 0.0
    }

    pub fn over(&self) -> bool {
        self.slow_left <= 0.0 && self.stun <= 0.0 && self.root <= 0.0 && self.dot_left <= 0.0
    }

    /// Advances the timers by `dt`; returns the damage-over-time to deal now (every
    /// [`DOT_TICK`], and what is left when it ends).
    pub fn tick(&mut self, dt: f32) -> f32 {
        for t in [&mut self.slow_left, &mut self.stun, &mut self.root] {
            *t = (*t - dt).max(0.0);
        }
        if self.dot_left <= 0.0 {
            return 0.0;
        }
        let step = dt.min(self.dot_left);
        self.dot_left -= step;
        self.owed += self.dot * step;
        self.tick -= dt;
        if self.tick > 0.0 && self.dot_left > 0.0 {
            return 0.0;
        }
        self.tick = DOT_TICK;
        std::mem::take(&mut self.owed)
    }
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
    /// A gun bullet in the head took the last health (kill feed's headshot mark).
    pub head: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    fn hit(secs: f32, slow: f32, stun: bool, dot: f32) -> Afflict {
        Afflict {
            target: Entity::PLACEHOLDER,
            by: Entity::PLACEHOLDER,
            secs,
            slow,
            stun,
            root: false,
            dot,
        }
    }

    #[test]
    fn timers_merge_and_run_out() {
        let mut s = Status::default();
        assert!(s.over() && s.speed() == 1.0 && !s.rooted());
        s.add(&hit(7.0, 0.5, false, 0.0));
        // The Stun skill (zskill 351): 65 % speed and a stun for 3 s. The slower factor and the
        // longer time of the two slows win; the stun runs on its own clock and roots.
        s.add(&hit(3.0, 0.65, true, 0.0));
        assert_eq!((s.speed(), s.slow_left, s.stun), (0.5, 7.0, 3.0));
        assert!(s.stunned() && s.rooted());
        for _ in 0..31 {
            s.tick(0.1);
        }
        assert!(!s.stunned() && !s.rooted() && s.speed() == 0.5 && !s.over());
        for _ in 0..40 {
            s.tick(0.1);
        }
        assert!(s.over() && s.speed() == 1.0);
        // A slow that ended does not weaken the next, weaker one.
        s.add(&hit(2.0, 0.8, false, 0.0));
        assert_eq!(s.speed(), 0.8);
    }

    #[test]
    fn damage_over_time_pays_its_total() {
        let mut s = Status::default();
        s.add(&hit(3.0, 1.0, false, 30.0));
        let (mut total, mut ticks) = (0.0, 0);
        for _ in 0..40 {
            let d = s.tick(0.1);
            if d > 0.0 {
                (total, ticks) = (total + d, ticks + 1);
            }
        }
        assert!((total - 30.0).abs() < 1e-3, "{total}");
        assert!((6..=7).contains(&ticks), "{ticks}");
        assert!(s.over());
    }
}
