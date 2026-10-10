//! Actor controller for `gunz-play`: spawns characters with their weapons, drives them from
//! [`Intent`] (movement with map collision, GunZ moves, weapon use, animation, death and
//! respawn) and provides the player's input, script and third-person camera.
//!
//! Space: an actor entity's `Transform` is its FEET, yaw about +Y (0 = facing -Z). The
//! character model is a child turned half a turn (ELU models face +Z) and twisted toward the
//! move direction when strafing.
//!
//! Animation: one [`Animator`] per model. The state picks the main clip (run, tumble, a
//! melee/hit/blast action, ...), shots/reloads/weapon draws/gun flinches play on the upper-body
//! layer so the legs keep running, and the spine carries the aim pitch. Melee/hit/blast/taunt
//! actions move the capsule by the clip's root bone (`root_lock`: the model stays glued to the
//! capsule). All clips are parsed at startup ([`ActorData`]), never mid-match. Other modules
//! ask for clips with [`ActionRequest`] and read [`Acting`]/[`Motor`] back.
//!
//! Movement constants (**observed** unless noted) come from public community replays
//! (`.gzr`, `docs/formats.md`: 10 Hz position + velocity of every player): run 10 m/s with a
//! melee weapon and 9 m/s with a gun, the same in every direction, jump 9 m/s with 25 m/s^2
//! gravity, wall kick 3 m/s out and 14 m/s up, terminal speed 30 m/s. No retail data file
//! holds them (`npc2.xml` has per-NPC `speed` of 400..840 cm/s). Tumble speed and the wall-run
//! climb profile are still inferred from the animations (see the constants).

use crate::{
    ani::{Ani, FPS},
    anim::{Animator, Loop},
    character::{self, Character, Look, Wardrobe},
    col::MapCollision,
    combat::{SWITCH_DELAY, Vfx},
    elu,
    game::*,
    item::{Items, WeaponKind},
    level::Level,
    model::{self, Textures},
    mrs::Vfs,
    view::{self, Shot},
};
use bevy::{
    ecs::system::SystemParam,
    input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll},
    mesh::skinning::SkinnedMeshInverseBindposes,
    prelude::*,
};
use std::{collections::HashMap, f32::consts::PI, io, sync::Arc};

/// Katana, revolver, rifle (zitem ids): slots 0, 1, 2.
pub const DEFAULT_LOADOUT: [u32; 3] = [2010000, 2050000, 2100001];

pub const RADIUS: f32 = 0.35;
pub const HEIGHT: f32 = 1.75;
const EYE: f32 = 1.55;
/// Run speed with a melee weapon in hand, any direction (**observed**: 999-1000 cm/s in 59
/// public replays; forward, backward and strafing alike, so no slower backwards speed).
pub const RUN: f32 = 10.0;
/// Run speed factor with a gun in hand (**observed**: 899-900 cm/s in the gun slots, rocket
/// carriers included, so a `limitspeed` of 90 is this factor, not an extra one).
const GUN_RUN: f32 = 0.9;
/// Takeoff speed of a jump from the ground: apex 1.62 m (**observed**: 900 cm/s from 42 000
/// jumps; melee and guns alike).
pub const JUMP: f32 = 9.0;
/// **Observed**: dv/dt of every airborne sample is -2500 cm/s^2 (463 000 samples, 59 replays).
pub const GRAVITY: f32 = 25.0;
/// Terminal fall speed (**observed**: vertical speed is clamped at exactly -3000 cm/s).
pub const FALL: f32 = 30.0;
const TUMBLE: f32 = 9.0;
/// Wall kick: horizontal speed away from the wall and takeoff speed up (**observed**: the
/// horizontal speed after a kick starts at 300 cm/s, the takeoff solved from two samples is
/// 1400 cm/s; 2800 kicks).
pub const WALL_OUT: f32 = 3.0;
pub const WALL_UP: f32 = 14.0;
/// Seconds a wall run lasts (along a side wall / up a wall) before the actor starts falling:
/// the lengths of `runLW`/`runRW` (60 frames) and `runW` (18 frames).
pub const WALL_RUN_SIDE: f32 = 2.0;
pub const WALL_RUN_UP: f32 = 0.6;
/// Fraction of gravity felt while running along a wall (**observed**: dv/dt -250 cm/s^2 in the
/// side wall-run states) / after a wall run ended (**observed**: full gravity, no speed limit
/// below `FALL`).
pub const RUN_GRAVITY: f32 = 0.1;
pub const SLIDE_GRAVITY: f32 = 1.0;
/// Vertical push (m/s) from which a `Push` launches the actor (uppercut, rocket and grenade
/// blasts push with `y >= 6` near the centre). **Inferred.**
const BLAST_PUSH: f32 = 5.0;
/// Seconds the actor lies after the `blast_drop` clip ended before `blast_stand`. **Inferred.**
const LIE: f32 = 0.35;
/// Air control of a launched actor (fraction of `RUN`) while it falls (`blast_airmove`).
/// **Inferred.**
const BLAST_STEER: f32 = 0.5;
/// An emote or taunt can be cancelled by a jump, dash or step after this many seconds.
/// **Inferred.**
const TAUNT_CANCEL: f32 = 0.5;
/// Emotes: clip name (**observed**, `man01.xml`/`woman01.xml`: every motion type has `bow wave
/// cry laugh dance`; `taunt` is the `T` key) and its keyboard key (**inferred**). Looping
/// clips play one cycle.
pub const EMOTES: [(&str, KeyCode); 5] = [
    ("bow", KeyCode::F5),
    ("wave", KeyCode::F6),
    ("cry", KeyCode::F7),
    ("laugh", KeyCode::F8),
    ("dance", KeyCode::F9),
];
/// A trigger click this recent still fires once the shot delay is over. **Inferred.**
const CLICK_BUFFER: f32 = 0.12;
/// Seconds in the air beyond which touching ground makes the landing thud. **Inferred.**
const LAND_AIR: f32 = 0.35;
/// Seconds without wall contact after which a wall run ends.
pub const WALL_LOSE: f32 = 0.15;
/// A wall run needs at least this much air under the feet (a step against a leaning stair
/// riser also lifts the capsule for a few frames).
pub const WALL_MIN_HEIGHT: f32 = 0.5;
/// Seconds within which a second tap of a direction is a tumble.
const DOUBLE_TAP: f32 = 0.3;
/// Seconds after touching a wall in the air during which a jump is a wall kick.
pub const WALL_GRACE: f32 = 0.15;
const CAM_DIST: f32 = 3.0;
/// Radius of the sphere the camera sweeps, so it stays clear of walls.
pub const CAM_RADIUS: f32 = 0.25;
/// Height of the player's camera pivot (the aim ray starts here), above the head.
const CAM_HEIGHT: f32 = 1.75;
const CAM_SHOULDER: f32 = 0.35;

/// Order of the actor systems; other modules order theirs against these (a melee system that
/// writes `ActionRequest` runs after `Input` and before `Drive`).
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum ActorSet {
    Input,
    Drive,
    Camera,
}

pub struct ActorPlugin;

impl Plugin for ActorPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<PlayerSetup>()
            .add_systems(Startup, (spawn_player, spawn_camera))
            .configure_sets(
                Update,
                (ActorSet::Input, ActorSet::Drive, ActorSet::Camera).chain(),
            )
            .add_systems(
                Update,
                (
                    equip.in_set(ActorSet::Input),
                    player_input.in_set(ActorSet::Input),
                    status.in_set(ActorSet::Input).after(player_input),
                    drive.in_set(ActorSet::Drive),
                    follow_camera.in_set(ActorSet::Camera),
                ),
            );
    }
}

/// Who the player is and where they start (`None` = first spawn point).
#[derive(Resource, Default)]
pub struct PlayerSetup {
    /// Feet position, Bevy metres.
    pub at: Option<Vec3>,
    /// Facing yaw in radians.
    pub yaw: Option<f32>,
    pub woman: bool,
    /// zitem ids for the loadout slots; empty = [`DEFAULT_LOADOUT`].
    pub loadout: Vec<u32>,
    pub look: Look,
    /// Shown name; empty = "Player".
    pub name: String,
}

/// Items, characters, spawn points and every character animation.
#[derive(Resource)]
pub struct ActorData {
    pub items: Items,
    men: Character,
    women: Character,
    /// `[man, woman]`: what bots pick their clothes from.
    wardrobes: [Wardrobe; 2],
    /// Seeds the bots' clothes ([`ActorData::random_look`]); 1 until the game sets it.
    pub seed: u64,
    pub(crate) spawns: Vec<(Vec3, Vec3)>,
    /// Actors falling below this height are put back on a spawn point.
    fall_limit: f32,
    /// `[man, woman][motion type][name]`: all parsed at startup so that no clip is ever
    /// read, inflated and parsed in the middle of a match (that was a visible hitch).
    clips: [HashMap<u32, HashMap<String, Clip>>; 2],
}

/// A character animation resolved for a motion type.
struct Clip {
    ani: Arc<Ani>,
    looping: Loop,
    /// Verbatim `sound` attribute (`man_jump`, `fx_dash`), empty if none.
    sound: String,
    secs: f32,
}

/// Parses every animation file of `ch` on all cores and indexes the entries by motion type
/// and name.
fn clip_table(vfs: &Vfs, ch: &Character) -> HashMap<u32, HashMap<String, Clip>> {
    let mut files: Vec<&str> = ch.animations.iter().map(|a| a.file.as_str()).collect();
    files.sort_unstable();
    files.dedup();
    let load = |f: &str| {
        vfs.read(f)
            .and_then(|b| crate::ani::load(&b))
            .map_err(|e| warn!("{f}: {e}"))
            .ok()
            .map(Arc::new)
    };
    // The browser build has no threads.
    let loaded: HashMap<&str, Arc<Ani>> = if cfg!(target_arch = "wasm32") {
        files.iter().filter_map(|&f| Some((f, load(f)?))).collect()
    } else {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
        std::thread::scope(|s| {
            let jobs: Vec<_> = files
                .chunks(files.len().div_ceil(threads).max(1))
                .map(|chunk| {
                    s.spawn(move || {
                        chunk
                            .iter()
                            .filter_map(|&f| Some((f, load(f)?)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            jobs.into_iter()
                .flat_map(|j| j.join().expect("animation loader panicked"))
                .collect()
        })
    };
    let mut table: HashMap<u32, HashMap<String, Clip>> = HashMap::new();
    for a in &ch.animations {
        if let Some(ani) = loaded.get(a.file.as_str()) {
            table.entry(a.motion_type).or_default().insert(
                a.name.clone(),
                Clip {
                    secs: ani.max_frame as f32 / FPS,
                    ani: ani.clone(),
                    looping: Loop::from_xml(&a.loop_type),
                    sound: a.sound.clone(),
                },
            );
        }
    }
    table
}

impl ActorData {
    pub fn new(level: &Level) -> io::Result<Self> {
        let men = character::load(&level.vfs, "heroman1")?;
        let women = character::load(&level.vfs, "herowoman1")?;
        let clips = [clip_table(&level.vfs, &men), clip_table(&level.vfs, &women)];
        let items = Items::load(&level.vfs)?;
        Ok(Self {
            wardrobes: [
                Wardrobe::new(&men, &items, false),
                Wardrobe::new(&women, &items, true),
            ],
            seed: 1,
            items,
            men,
            women,
            fall_limit: level
                .spawn_points()
                .iter()
                .map(|s| s.0.y - 40.0)
                .fold(f32::MAX, f32::min),
            spawns: level.spawn_points(),
            clips,
        })
    }

    /// Random clothes for a bot; `salt` tells the bots of one match apart.
    pub fn random_look(&self, woman: bool, salt: u64) -> Look {
        self.wardrobes[woman as usize].random(self.seed ^ salt.wrapping_mul(0x2545_f491_4f6c_dd1d))
    }

    /// A new map was loaded (quest sectors): take its spawn points and fall limit.
    pub fn rebase(&mut self, level: &Level) {
        self.spawns = level.spawn_points();
        self.fall_limit = self
            .spawns
            .iter()
            .map(|s| s.0.y - 40.0)
            .fold(f32::MAX, f32::min);
    }

    pub(crate) fn character(&self, woman: bool) -> &Character {
        if woman { &self.women } else { &self.men }
    }

    /// Length in seconds (at speed 1) of animation `clip` of weapon motion type `motion`.
    pub fn clip_secs(&self, woman: bool, motion: u32, clip: &str) -> Option<f32> {
        self.clip(woman, motion, clip).map(|c| c.secs)
    }

    /// Animation `name` of weapon motion type `motion`; `None` if the character has none.
    /// Grenades and medikits (motion 6, 8) have `load` where the rest have `reload`.
    fn clip(&self, woman: bool, motion: u32, name: &str) -> Option<&Clip> {
        let of = self.clips[woman as usize].get(&motion)?;
        of.get(name)
            .or_else(|| of.get("load").filter(|_| name == "reload"))
    }
}

/// Stages of being launched (a `Push` with a large vertical part, see [`BLAST_PUSH`]).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Blasted {
    /// `blast`: thrown up and back.
    Rise,
    /// `blast_fall` (`blast_airmove` while steering) until the ground is reached.
    Fall,
    /// `blast_drop`: hitting the floor and lying there.
    Drop,
    /// `blast_stand`: getting up.
    Stand,
}

/// What the actor is doing besides free movement.
#[derive(Clone, Copy)]
enum State {
    Free,
    Tumble {
        dir: Vec3,
        anim: &'static str,
        left: f32,
        total: f32,
    },
    Wall {
        anim: &'static str,
        left: f32,
    },
    /// Running along the wall behind normal `n` (`side` -1 wall on the left, +1 right) or up
    /// it (`side` 0, facing it). While `left > 0` the running clip plays, then its `_down` one.
    WallRun {
        n: Vec3,
        side: f32,
        left: f32,
    },
    /// A clip played as a full-body action (an [`ActionRequest`] or a taunt): `t` of `total`
    /// seconds elapsed (`total` is infinite for a looping clip).
    Action {
        clip: &'static str,
        t: f32,
        total: f32,
        speed: f32,
        moving: ActionMove,
        cancel_from: f32,
    },
    Reload {
        left: f32,
    },
    Blast {
        stage: Blasted,
        /// Seconds in this stage.
        t: f32,
        /// Thrown by a dagger blow: the `blast_dagger` / `blast_drop_dagger` clips play
        /// instead of `blast` / `blast_drop` (**inferred** use of the dagger variants).
        dagger: bool,
    },
    Dead,
}

#[derive(Component)]
pub struct Actor {
    pub woman: bool,
    /// Character model root (carries the [`Animator`]).
    model: Entity,
    /// Weapon model roots per loadout slot (two for dual weapons).
    weapons: Vec<Vec<Entity>>,
    /// Every weapon item the actor spawned with, parallel to `weapons`; [`Equip`] picks from it.
    pub(crate) kit: Vec<u32>,
    /// What it spawned wearing (`ActorSpec::look`).
    pub(crate) look: Look,
    /// Loadout slot -> index into `kit`/`weapons` (identity until an [`Equip`]).
    carry: Vec<usize>,
    /// Slot whose weapon models are visible.
    shown: usize,
    vel: Vec3,
    grounded: bool,
    state: State,
    /// Main animation playing: (weapon motion type, name).
    playing: (u32, &'static str),
    /// Start `playing` over even if unchanged.
    restart: bool,
    /// Upper-body clip playing over it (`attackS`, `reload`, `load`, `damage`).
    upper: Option<&'static str>,
    /// Playback rate of the upper clip (reloads are stretched to the weapon's reload time).
    upper_rate: f32,
    /// A shot / weapon draw / gun-hit flinch started this frame: start the upper clip over.
    shot: bool,
    draw: bool,
    flinch: Option<&'static str>,
    /// Last tap time and held flag per direction (forward, back, left, right).
    taps: [f32; 4],
    held: [bool; 4],
    prev_jump: bool,
    prev_attack: bool,
    /// Time of the last trigger click (a click during the shot delay still fires).
    clicked: f32,
    prev_emote: Option<&'static str>,
    /// Last wall touched in the air: outward normal and time.
    wall: Option<(Vec3, f32)>,
    /// Per loadout slot: next time its weapon may fire / the empty-magazine click may sound.
    ready: Vec<f32>,
    dry: f32,
    /// Half run-cycles seen so far, for footsteps.
    step: i32,
    /// Smoothed strafe twist of the model (radians).
    twist: f32,
    /// Smoothed aim pitch put on the spine.
    pitch: f32,
    die: &'static str,
    /// Already wall-ran since last touching the ground.
    wall_spent: bool,
    /// Next hit reaction clip (alternates `damage` / `damage2`).
    hurt: bool,
    /// Seconds spent in the air (landing sound).
    air: f32,
}

/// What to spawn: `pos` is the feet position in Bevy metres.
pub struct ActorSpec {
    pub name: String,
    pub pos: Vec3,
    pub yaw: f32,
    pub woman: bool,
    /// zitem ids for slots 0..; empty = [`DEFAULT_LOADOUT`].
    pub loadout: Vec<u32>,
    pub look: Look,
    /// `Bot` (AI writes `Intent`) or `Player` (gunz-play input does).
    pub bot: bool,
}

/// Spawns actors. Use as a system parameter: `spawner.spawn(ActorSpec { .. })`.
#[derive(SystemParam)]
pub struct ActorSpawner<'w, 's> {
    pub commands: Commands<'w, 's>,
    level: Res<'w, Level>,
    data: Res<'w, ActorData>,
    meshes: ResMut<'w, Assets<Mesh>>,
    bindposes: ResMut<'w, Assets<SkinnedMeshInverseBindposes>>,
    images: ResMut<'w, Assets<Image>>,
    standard: ResMut<'w, Assets<StandardMaterial>>,
    arsenal: Option<Res<'w, Arsenal>>,
}

impl ActorSpawner<'_, '_> {
    /// Random clothes for a bot ([`ActorData::random_look`]).
    pub fn random_look(&self, woman: bool, salt: u64) -> Look {
        self.data.random_look(woman, salt)
    }

    pub fn spawn(&mut self, spec: ActorSpec) -> Entity {
        let vfs = &self.level.vfs;
        let ch = self.data.character(spec.woman);
        let look = spec.look.fit(ch);
        let mut textures = Textures::new(vfs, "model/");
        let mut body = |look: &Look| {
            character::spawn(
                &mut self.commands,
                &mut self.meshes,
                &mut self.bindposes,
                &mut self.images,
                &mut self.standard,
                &mut textures,
                vfs,
                ch,
                look,
                Transform::from_rotation(Quat::from_rotation_y(PI)),
            )
        };
        // a piece that cannot be read (a browser download that failed) leaves the base body
        let body = body(&look)
            .or_else(|e| {
                warn!("{}'s clothes: {e}", spec.name);
                body(&Look::default())
            })
            .unwrap_or_else(|e| panic!("character {}: {e}", ch.name));
        let ids = match (&self.arsenal, spec.loadout.is_empty()) {
            (Some(a), _) => a.0.clone(),
            (None, true) => DEFAULT_LOADOUT.to_vec(),
            (None, false) => spec.loadout,
        };
        let kit = ids.clone();
        let (mut slots, mut weapons) = (Vec::new(), Vec::new());
        for (i, id) in ids.into_iter().enumerate() {
            let item = self
                .data
                .items
                .get(id)
                .unwrap_or_else(|| panic!("no item {id}"));
            let w = item
                .weapon
                .as_ref()
                .unwrap_or_else(|| panic!("item {id} is not a weapon"));
            let wm = self
                .data
                .items
                .model(item)
                .unwrap_or_else(|| panic!("item {id} has no model"));
            let elu = elu::load(
                &vfs.read(&wm.elu)
                    .unwrap_or_else(|e| panic!("{}: {e}", wm.elu)),
            )
            .unwrap_or_else(|e| panic!("{}: {e}", wm.elu));
            let dir = &wm.elu[..wm.elu.rfind('/').map_or(0, |i| i + 1)];
            let mut material =
                |m: &elu::Material| textures.standard(&mut self.images, &mut self.standard, dir, m);
            let mut roots = Vec::new();
            for dummy in w.kind.dummies() {
                let Some(&node) = body.nodes.get(*dummy) else {
                    warn!("character has no node {dummy}");
                    continue;
                };
                // The shell-casing helper node is an ejected-shell effect, not part of the gun.
                let m = model::spawn_elu(
                    &mut self.commands,
                    &mut self.meshes,
                    &mut self.bindposes,
                    &elu,
                    &mut material,
                    Transform::IDENTITY,
                    |n| !n.name.starts_with("empty_cartridge"),
                );
                m.attach(&mut self.commands, node);
                if i != 0 {
                    self.commands.entity(m.root).insert(Visibility::Hidden);
                }
                roots.push(m.root);
            }
            weapons.push(roots);
            slots.push(fresh_slot(&self.data.items, id));
        }
        let first = gear(&self.data.items, slots[0].item);
        let idle = self
            .data
            .clip(spec.woman, first.motion, "idle")
            .unwrap_or_else(|| panic!("no idle clip for motion type {}", first.motion));
        self.commands
            .entity(body.root)
            .insert(Animator::new(idle.ani.clone(), idle.looping));
        let ready = vec![0.0; slots.len()];
        let actor = self
            .commands
            .spawn((
                Name::new(spec.name),
                Transform::from_translation(spec.pos)
                    .with_rotation(Quat::from_rotation_y(spec.yaw)),
                Visibility::default(),
                Actor {
                    woman: spec.woman,
                    model: body.root,
                    weapons,
                    carry: (0..kit.len()).collect(),
                    kit,
                    look,
                    shown: 0,
                    vel: Vec3::ZERO,
                    grounded: false,
                    state: State::Free,
                    playing: (first.motion, "idle"),
                    restart: false,
                    upper: None,
                    upper_rate: 1.0,
                    shot: false,
                    draw: false,
                    flinch: None,
                    taps: [f32::MIN; 4],
                    held: [false; 4],
                    prev_jump: false,
                    prev_attack: false,
                    clicked: f32::MIN,
                    prev_emote: None,
                    wall: None,
                    ready,
                    dry: 0.0,
                    step: -1,
                    twist: 0.0,
                    pitch: 0.0,
                    die: "die",
                    wall_spent: false,
                    hurt: false,
                    air: 0.0,
                },
                Motor {
                    woman: spec.woman,
                    ..default()
                },
                Intent {
                    yaw: spec.yaw,
                    ..default()
                },
                Vitals {
                    hp: 100.0,
                    ap: 50.0,
                    max_hp: 100.0,
                    max_ap: 50.0,
                },
                Loadout { slots, current: 0 },
                Score::default(),
            ))
            .add_child(body.root)
            .id();
        if spec.bot {
            self.commands.entity(actor).insert(Bot);
        } else {
            self.commands.entity(actor).insert(Player);
        }
        actor
    }
}

/// Weapon stats the controller needs.
#[derive(Clone, Copy)]
struct Gear {
    melee: bool,
    /// Hold the trigger to keep firing (rifle, SMG, machine gun); the rest need a click per shot.
    auto: bool,
    motion: u32,
    /// Seconds between shots / swings.
    delay: f32,
    /// Seconds a reload takes: the character's reload clip length (set in `drive`).
    reload: f32,
    magazine: u32,
    reserve: u32,
    /// `limitspeed` / 100 (without it: 1.0 for melee, `GUN_RUN` for guns): run speed factor
    /// while this weapon is in hand.
    speed: f32,
    /// No `limitwall`: wall runs and wall kicks allowed.
    wall: bool,
}

fn is_melee(kind: WeaponKind) -> bool {
    matches!(
        kind,
        WeaponKind::Katana | WeaponKind::Dagger | WeaponKind::DoubleKatana | WeaponKind::SpyCase
    )
}

/// Whether zitem `id` is a melee weapon (`false` for anything else, e.g. an explosion's item).
fn melee_item(items: &Items, id: u32) -> bool {
    items
        .get(id)
        .and_then(|i| i.weapon.as_ref())
        .is_some_and(|w| is_melee(w.kind))
}

fn item_name(items: &Items, id: u32) -> &str {
    items.get(id).and_then(|i| i.name.as_deref()).unwrap_or("?")
}

fn gear(items: &Items, id: u32) -> Gear {
    let w = items
        .get(id)
        .and_then(|i| i.weapon.as_ref())
        .unwrap_or_else(|| panic!("item {id} is not a weapon"));
    Gear {
        melee: is_melee(w.kind),
        auto: w.kind.automatic(),
        motion: w.kind.motion_type(),
        delay: w.delay as f32 / 1000.0,
        reload: w.reload_secs(),
        magazine: w.magazine,
        reserve: w
            .max_bullet
            .unwrap_or(w.magazine * 4)
            .saturating_sub(w.magazine),
        speed: w
            .limit_speed
            .map_or(if is_melee(w.kind) { 1.0 } else { GUN_RUN }, |p| {
                p as f32 / 100.0
            }),
        wall: !w.limit_wall,
    }
}

/// Applies [`Equip`]: re-picks the actor's loadout from the weapons it spawned with.
fn equip(
    data: Res<ActorData>,
    mut requests: MessageReader<Equip>,
    mut actors: Query<(&mut Actor, &mut Loadout)>,
) {
    for r in requests.read() {
        let Ok((mut a, mut load)) = actors.get_mut(r.actor) else {
            continue;
        };
        let (mut carry, mut slots) = (Vec::new(), Vec::new());
        for &(id, count) in &r.items {
            let Some(k) = a.kit.iter().position(|k| *k == id) else {
                continue;
            };
            let mut s = fresh_slot(&data.items, id);
            if let Some(n) = count {
                s.magazine = n.min(s.magazine);
                s.reserve = n - s.magazine;
            }
            carry.push(k);
            slots.push(s);
        }
        if carry.is_empty() {
            continue;
        }
        load.slots = slots;
        load.current = r.current.min(carry.len() - 1);
        a.carry = carry;
        a.shown = usize::MAX;
    }
}

fn fresh_slot(items: &Items, id: u32) -> Slot {
    let g = gear(items, id);
    Slot {
        item: id,
        magazine: g.magazine,
        reserve: g.reserve,
    }
}

pub(crate) fn yaw_of(dir: Vec3) -> f32 {
    f32::atan2(-dir.x, -dir.z)
}

fn approach(cur: Vec3, target: Vec3, max: f32) -> Vec3 {
    let d = target - cur;
    let len = d.length();
    if len <= max {
        target
    } else {
        cur + d / len * max
    }
}

/// Aim ray: the player's is the third-person camera's centre ray (starting at the shoulder
/// pivot in front of the camera), a bot's starts at its eye.
fn aim(feet: Vec3, intent: &Intent, player: bool) -> (Vec3, Vec3) {
    let rot = Quat::from_euler(EulerRot::YXZ, intent.yaw, intent.pitch, 0.0);
    let shoulder = if player {
        rot * Vec3::X * CAM_SHOULDER
    } else {
        Vec3::ZERO
    };
    let height = if player { CAM_HEIGHT } else { EYE };
    (feet + Vec3::Y * height + shoulder, rot * Vec3::NEG_Z)
}

/// The spawn point farthest from every other actor.
pub(crate) fn pick_spawn(
    spawns: &[(Vec3, Vec3)],
    me: Entity,
    others: &[(Entity, Vec3)],
) -> (Vec3, Vec3) {
    let near = |p: Vec3| {
        others
            .iter()
            .filter(|(e, _)| *e != me)
            .map(|(_, q)| p.distance(*q))
            .fold(f32::MAX, f32::min)
    };
    spawns
        .iter()
        .copied()
        .max_by(|a, b| near(a.0).total_cmp(&near(b.0)))
        .unwrap_or((Vec3::ZERO, Vec3::NEG_Z))
}

fn spawn_player(mut spawner: ActorSpawner, setup: Res<PlayerSetup>) {
    let (pos, dir) = spawner
        .data
        .spawns
        .first()
        .copied()
        .unwrap_or((Vec3::ZERO, Vec3::NEG_Z));
    spawner.spawn(ActorSpec {
        name: match setup.name.as_str() {
            "" => "Player".into(),
            n => n.into(),
        },
        pos: setup.at.unwrap_or(pos + Vec3::Y * 0.1),
        yaw: setup.yaw.unwrap_or_else(|| yaw_of(dir)),
        woman: setup.woman,
        loadout: setup.loadout.clone(),
        look: setup.look,
        bot: false,
    });
}

fn spawn_camera(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    shot: Option<Res<Shot>>,
) {
    view::spawn_camera(
        &mut commands,
        &mut images,
        shot.is_some(),
        Vec3::ZERO,
        Vec3::NEG_Z,
    );
}

/// Held controls of one frame, from the keyboard or a script.
#[derive(Clone, Copy, Default)]
struct Held {
    fwd: bool,
    back: bool,
    left: bool,
    right: bool,
    jump: bool,
    attack: bool,
    reload: bool,
    slot: Option<usize>,
    guard: bool,
    taunt: bool,
    emote: Option<&'static str>,
    /// Scripts hold Tab (scoreboard) by pressing it in `ButtonInput<KeyCode>`.
    tab: bool,
}

/// Scripted player input for headless runs (`gunz-play --script`): `;`-separated steps
/// `KEYS:SECONDS` run one after another, or `yaw=DEG` / `pitch=DEG` (instant). `KEYS` is
/// `+`-joined from `w a s d jump attack guard reload taunt bow wave cry laugh dance tab 1..9
/// wait` (the emote keys are F5-F9 on the keyboard).
#[derive(Resource)]
pub struct Script {
    steps: Vec<Step>,
    idx: usize,
    start: f32,
    entered: bool,
    /// Seconds of simulation before the script starts (lets the actor land and pipelines warm up).
    pub lead: f32,
}

struct Step {
    held: Held,
    secs: f32,
    yaw: Option<f32>,
    pitch: Option<f32>,
}

impl Script {
    pub fn parse(text: &str) -> Result<Self, String> {
        let mut steps = Vec::new();
        for part in text.split(';').map(str::trim).filter(|p| !p.is_empty()) {
            let mut step = Step {
                held: Held::default(),
                secs: 0.0,
                yaw: None,
                pitch: None,
            };
            if let Some((k, v)) = part.split_once('=') {
                let deg: f32 = v.parse().map_err(|_| format!("bad number in {part:?}"))?;
                match k {
                    "yaw" => step.yaw = Some(deg.to_radians()),
                    "pitch" => step.pitch = Some(deg.to_radians()),
                    _ => return Err(format!("unknown setting {k:?}")),
                }
            } else {
                let (keys, secs) = part
                    .split_once(':')
                    .ok_or(format!("missing `:` in {part:?}"))?;
                step.secs = secs
                    .parse()
                    .map_err(|_| format!("bad seconds in {part:?}"))?;
                for key in keys.split('+') {
                    match key {
                        "w" => step.held.fwd = true,
                        "s" => step.held.back = true,
                        "a" => step.held.left = true,
                        "d" => step.held.right = true,
                        "jump" => step.held.jump = true,
                        "attack" => step.held.attack = true,
                        "guard" => step.held.guard = true,
                        "taunt" => step.held.taunt = true,
                        "reload" => step.held.reload = true,
                        "tab" => step.held.tab = true,
                        "wait" => {}
                        n => match (n.parse::<usize>(), EMOTES.iter().find(|e| e.0 == n)) {
                            (Ok(n @ 1..=9), _) => step.held.slot = Some(n - 1),
                            (_, Some(&(clip, _))) => step.held.emote = Some(clip),
                            _ => return Err(format!("unknown key {n:?}")),
                        },
                    }
                }
            }
            steps.push(step);
        }
        Ok(Self {
            steps,
            idx: 0,
            start: 0.0,
            entered: false,
            lead: 0.0,
        })
    }

    /// Total length in seconds.
    pub fn secs(&self) -> f32 {
        self.steps.iter().map(|s| s.secs).sum()
    }

    /// Controls held at `now`; applies `yaw=`/`pitch=` steps to `look` as they are reached.
    fn held(&mut self, now: f32, look: &mut (f32, f32)) -> Held {
        let now = now - self.lead;
        if now < 0.0 {
            return Held::default();
        }
        loop {
            let Some(step) = self.steps.get(self.idx) else {
                return Held::default();
            };
            if !self.entered {
                look.0 = step.yaw.unwrap_or(look.0);
                look.1 = step.pitch.unwrap_or(look.1);
                self.entered = true;
            }
            if now < self.start + step.secs {
                return step.held;
            }
            self.start += step.secs;
            self.idx += 1;
            self.entered = false;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn player_input(
    time: Res<Time>,
    mut keys: ResMut<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    settings: Res<Settings>,
    frozen: Option<Res<Frozen>>,
    script: Option<ResMut<Script>>,
    player: Single<(&mut Intent, &Loadout), With<Player>>,
) {
    let (mut intent, load) = player.into_inner();
    let mut look = (intent.yaw, intent.pitch);
    let held = match script {
        _ if frozen.is_some() => Held::default(),
        Some(mut s) => {
            let held = s.held(time.elapsed_secs(), &mut look);
            if held.tab {
                keys.press(KeyCode::Tab);
            } else {
                keys.release(KeyCode::Tab);
            }
            held
        }
        None => {
            look.0 -= motion.delta.x * settings.sensitivity;
            look.1 -= motion.delta.y * settings.sensitivity;
            let slot = [
                KeyCode::Digit1,
                KeyCode::Digit2,
                KeyCode::Digit3,
                KeyCode::Digit4,
                KeyCode::Digit5,
            ]
            .iter()
            .position(|&k| keys.pressed(k))
            .or_else(|| {
                let n = load.slots.len();
                (scroll.delta.y != 0.0 && n > 0).then(|| {
                    if scroll.delta.y > 0.0 {
                        (load.current + n - 1) % n
                    } else {
                        (load.current + 1) % n
                    }
                })
            });
            Held {
                fwd: keys.pressed(KeyCode::KeyW),
                back: keys.pressed(KeyCode::KeyS),
                left: keys.pressed(KeyCode::KeyA),
                right: keys.pressed(KeyCode::KeyD),
                jump: keys.pressed(KeyCode::Space),
                attack: buttons.pressed(MouseButton::Left),
                guard: buttons.pressed(MouseButton::Right),
                reload: keys.pressed(KeyCode::KeyR),
                slot,
                taunt: keys.pressed(KeyCode::KeyT),
                emote: EMOTES.iter().find(|e| keys.pressed(e.1)).map(|e| e.0),
                tab: false,
            }
        }
    };
    let axis = |pos: bool, neg: bool| pos as i8 as f32 - neg as i8 as f32;
    intent.walk =
        Vec2::new(axis(held.right, held.left), axis(held.fwd, held.back)).clamp_length_max(1.0);
    intent.yaw = look.0;
    intent.pitch = look.1.clamp(-1.3, 1.3);
    intent.jump = held.jump;
    intent.attack = held.attack;
    intent.guard = held.guard;
    intent.taunt = held.taunt;
    intent.emote = held.emote;
    intent.reload = held.reload;
    intent.slot = held.slot;
}

/// Seconds the player's camera takes for one turn around the corpse (on top of the mouse).
const DEATH_ORBIT_PERIOD: f32 = 14.0;
/// Distance of the death camera from the corpse's chest.
const DEATH_DIST: f32 = 3.8;

/// Over-the-shoulder camera behind the player, pulled in front of walls. While dead it
/// orbits the corpse (the mouse still turns it, and it drifts slowly on its own), looking
/// slightly down at it.
fn follow_camera(
    time: Res<Time>,
    spectate: Option<Res<Spectate>>,
    player: Query<Entity, With<Player>>,
    actors: Query<(&Transform, &Intent, Has<Dead>), Without<Camera3d>>,
    col: Res<MapCollision>,
    mut camera: Single<&mut Transform, (With<Camera3d>, Without<Player>)>,
    mut dead_for: Local<f32>,
) {
    let target = spectate
        .and_then(|s| s.0)
        .filter(|e| actors.contains(*e))
        .or_else(|| player.single().ok());
    let Some((tf, intent, dead)) = target.and_then(|e| actors.get(e).ok()) else {
        return;
    };
    if dead {
        *dead_for += time.delta_secs();
        let rot = Quat::from_euler(
            EulerRot::YXZ,
            intent.yaw + *dead_for * 2.0 * PI / DEATH_ORBIT_PERIOD,
            (intent.pitch - 0.3).clamp(-1.2, 0.6),
            0.0,
        );
        let focus = tf.translation + Vec3::Y * 0.9;
        let back = rot * Vec3::Z;
        let dist = match col.sweep_sphere(focus, focus + back * DEATH_DIST, CAM_RADIUS) {
            Some(h) => h.distance.max(0.3),
            None => DEATH_DIST,
        };
        camera.translation = focus + back * dist;
        camera.rotation = rot;
        return;
    }
    *dead_for = 0.0;
    let (pivot, dir) = aim(tf.translation, intent, true);
    // duck under a low ceiling (a wall run can take the head to it)
    let height = match col.raycast(
        tf.translation + Vec3::Y * EYE,
        Vec3::Y,
        CAM_HEIGHT - EYE + 0.3,
    ) {
        Some(h) => (h.point.y - tf.translation.y - 0.3).max(EYE),
        None => CAM_HEIGHT,
    };
    let pivot = pivot - Vec3::Y * (CAM_HEIGHT - height);
    let head = tf.translation + Vec3::Y * height;
    let side = (pivot - head).normalize_or_zero();
    let pivot = match col.raycast(head, side, CAM_SHOULDER + 0.1) {
        Some(h) => head + side * (h.distance - 0.1).max(0.0),
        None => pivot,
    };
    // a sphere, not a ray: keeps the near plane off walls seen at a grazing angle
    let dist = match col.sweep_sphere(pivot, pivot - dir * CAM_DIST, CAM_RADIUS) {
        Some(h) => h.distance.max(0.3),
        None => CAM_DIST,
    };
    camera.translation = pivot - dir * dist;
    camera.rotation = Quat::from_euler(EulerRot::YXZ, intent.yaw, intent.pitch, 0.0);
}

/// Seconds a cross-fade into clip `name` takes. The data has no blend times (**inferred**).
fn blend_for(name: &str) -> f32 {
    match name {
        "idle" | "run" | "runB" | "jumpU" | "jumpD" => 0.12,
        n if n.starts_with("tumble") || n.starts_with("jumpwall") => 0.0,
        _ => 0.05,
    }
}

/// Climb speed (m/s) `t` seconds into an up-wall run: the vertical speed of the feet in
/// `runW` (**observed**, 3.3 m over 0.6 s: slow start, 9-10 m/s mid-run, taper).
pub fn climb(t: f32) -> f32 {
    const P: [(f32, f32); 8] = [
        (0.0, 3.0),
        (0.1, 2.0),
        (0.2, 3.5),
        (0.28, 6.0),
        (0.33, 9.0),
        (0.5, 9.5),
        (0.55, 4.0),
        (0.6, 3.0),
    ];
    let i = P.partition_point(|p| p.0 <= t).clamp(1, P.len() - 1);
    let ((t0, v0), (t1, v1)) = (P[i - 1], P[i]);
    v0 + (v1 - v0) * ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
}

fn begin_reload(
    a: &mut Actor,
    e: Entity,
    g: &Gear,
    s: &Slot,
    name: &str,
    now: f32,
    sound: &mut MessageWriter<ActorSound>,
) {
    a.state = State::Reload { left: g.reload };
    info!(
        "t={now:.2} reload: {name} item {} {:.1} s (mag {}+{})",
        s.item, g.reload, s.magazine, s.reserve
    );
    sound.write(ActorSound {
        actor: e,
        cue: Cue::Reload { item: s.item },
    });
}

/// Merges [`Afflict`]s into [`Status`], ticks it (paying its damage over time) and keeps the
/// [`Intent`] of a rooted actor from walking or jumping and that of a stunned one from doing
/// anything but turning; `drive` slows the run and plays the `stun` clip. The status goes with
/// the actor's death.
fn status(
    mut commands: Commands,
    time: Res<Time>,
    mut hits: MessageReader<Afflict>,
    mut damage: MessageWriter<Damage>,
    mut vfx: MessageWriter<Vfx>,
    mut rounds: MessageReader<NewRound>,
    mut actors: Query<(
        Entity,
        &GlobalTransform,
        &Motor,
        &mut Intent,
        Option<&mut Status>,
        Option<&Name>,
        Has<Dead>,
    )>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs().min(0.05));
    let mut fresh: HashMap<Entity, Status> = HashMap::new();
    // A new round wipes every effect (spy rounds respawn everybody).
    let reset = rounds.read().count() > 0;
    for a in hits.read() {
        let Ok((_, g, _, _, st, name, dead)) = actors.get_mut(a.target) else {
            continue;
        };
        if dead {
            continue;
        }
        match st {
            Some(mut s) => s.add(a),
            None => fresh.entry(a.target).or_default().add(a),
        }
        info!(
            "t={now:.2} status: {} <- slow x{:.2}, stun {}, root {}, dot {:.0} for {:.1} s",
            name.map_or("?", |n| n.as_str()),
            a.slow,
            a.stun,
            a.root,
            a.dot,
            a.secs
        );
        // The retail `ef_stun` (stars) and `ef_slow_dam` (zskill 351, 151) where the hit landed.
        for (on, name) in [
            (a.stun, "ef_stun"),
            (a.slow < 1.0 && !a.stun, "ef_slow_dam"),
        ] {
            if on {
                vfx.write(Vfx::Named {
                    name: name.into(),
                    at: Transform::from_translation(g.translation() + Vec3::Y * 1.0),
                });
            }
        }
    }
    for (e, s) in fresh {
        commands.entity(e).insert(s);
    }
    for (e, g, motor, mut intent, st, name, dead) in &mut actors {
        let Some(mut s) = st else { continue };
        let due = s.tick(dt);
        if s.slow_left > 0.0 && (now * 2.0).floor() != ((now - dt) * 2.0).floor() {
            debug!(
                "t={now:.2} status: {} slowed x{:.2}, ground speed {:.2} m/s",
                name.map_or("?", |n| n.as_str()),
                s.speed(),
                Vec2::new(motor.vel.x, motor.vel.z).length()
            );
        }
        if due > 0.0 {
            damage.write(Damage {
                target: e,
                attacker: s.dot_by.unwrap_or(e),
                amount: due,
                item: 0,
                point: g.translation() + Vec3::Y,
                dir: Vec3::Y,
                pierce: None,
            });
        }
        if dead || reset || s.over() {
            commands.entity(e).remove::<Status>();
        }
        if s.rooted() {
            intent.walk = Vec2::ZERO;
            intent.jump = false;
        }
        if s.stunned() {
            (intent.attack, intent.reload, intent.guard, intent.taunt) =
                (false, false, false, false);
            (intent.slot, intent.emote) = (None, None);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    mut commands: Commands,
    time: Res<Time>,
    col: Res<MapCollision>,
    data: Res<ActorData>,
    mut actors: Query<(
        Entity,
        &mut Transform,
        &mut Actor,
        &mut Intent,
        &mut Loadout,
        &mut Vitals,
        &mut Motor,
        Option<&SpawnAt>,
        Has<Dead>,
        Has<Player>,
        Option<&Status>,
        Option<&Mods>,
        Has<Remote>,
    )>,
    mut dead: Query<&mut Dead>,
    pushes: Query<&Push>,
    mut visibility: Query<&mut Visibility>,
    mut models: Query<&mut Transform, Without<Actor>>,
    mut animators: Query<&mut Animator>,
    mut acting: Query<&mut Acting>,
    mut fire: MessageWriter<Fire>,
    mut sound: MessageWriter<ActorSound>,
    mut requests: MessageReader<ActionRequest>,
    mut damages: MessageReader<Damage>,
    teams: Query<&Team>,
) {
    let (now, dt) = (time.elapsed_secs(), time.delta_secs().min(0.05));
    let reqs: Vec<ActionRequest> = requests.read().cloned().collect();
    let hits: Vec<(Entity, u32)> = damages
        .read()
        .filter(|d| d.amount > 0.0)
        .map(|d| (d.target, d.item))
        .collect();
    let others: Vec<(Entity, Vec3)> = actors.iter().map(|q| (q.0, q.1.translation)).collect();
    // Living actors block each other (not across Dynamic Duels arenas): whoever moves is
    // pushed out of the others' capsules.
    let bodies: Vec<(Entity, Vec3, Option<Team>)> = actors
        .iter()
        .filter(|q| !q.8)
        .map(|q| (q.0, q.1.translation, teams.get(q.0).ok().copied()))
        .collect();
    for (
        e,
        mut tf,
        mut a,
        mut intent,
        mut load,
        mut vitals,
        mut motor,
        spawn_at,
        is_dead,
        is_player,
        status,
        mods,
        remote,
    ) in &mut actors
    {
        let a = &mut *a;
        let woman = a.woman;
        if a.ready.len() < load.slots.len() {
            a.ready.resize(load.slots.len(), 0.0);
        }

        // Death and respawn.
        let mut respawn = false;
        if is_dead {
            if !matches!(a.state, State::Dead) {
                a.state = State::Dead;
                a.restart = true;
                a.die = ["die", "die2", "die3", "die4"][(now * 1000.0) as usize % 4];
            }
            if let Ok(mut d) = dead.get_mut(e) {
                d.respawn -= dt;
                respawn = d.respawn <= 0.0;
            }
        } else if matches!(a.state, State::Dead) {
            a.state = State::Free;
        }
        if respawn || tf.translation.y < data.fall_limit {
            let (p, d) =
                spawn_at.map_or_else(|| pick_spawn(&data.spawns, e, &others), |s| (s.pos, s.dir));
            tf.translation = p + Vec3::Y * 0.1;
            intent.yaw = yaw_of(d);
            a.vel = Vec3::ZERO;
            a.state = State::Free;
            a.restart = true;
        }
        if respawn {
            vitals.hp = vitals.max_hp;
            vitals.ap = vitals.max_ap;
            for s in &mut load.slots {
                *s = fresh_slot(&data.items, s.item);
            }
            commands.entity(e).remove::<Dead>();
        }
        let alive = !matches!(a.state, State::Dead);

        // Weapon switching and visibility.
        if alive
            && let Some(s) = intent.slot
            && s < load.slots.len()
            && matches!(a.state, State::Free | State::Reload { .. })
        {
            load.current = s;
        }
        if a.shown != load.current {
            let (from, to) = (a.shown, load.current);
            let cancelled = matches!(a.state, State::Reload { .. });
            if cancelled {
                a.state = State::Free;
            }
            let to_gear = gear(&data.items, load.slots[to].item);
            info!(
                "t={now:.2} switch: {} slot {from} -> {to} (run x{:.2}, wall moves {}){}",
                item_name(&data.items, load.slots[to].item),
                to_gear.speed,
                if to_gear.wall { "on" } else { "off" },
                match load.slots.get(from) {
                    Some(s) if cancelled => format!(
                        " (reload cancelled, mag {}/{})",
                        s.magazine,
                        gear(&data.items, s.item).magazine
                    ),
                    _ => String::new(),
                }
            );
            for (i, roots) in a.weapons.iter().enumerate() {
                for &r in roots {
                    if let Ok(mut v) = visibility.get_mut(r) {
                        *v = if i == a.carry[to] {
                            Visibility::Inherited
                        } else {
                            Visibility::Hidden
                        };
                    }
                }
            }
            a.shown = to;
            a.restart = true;
            a.draw = true;
            a.shot = false;
            a.ready[to] = a.ready[to].max(now + SWITCH_DELAY);
        }
        let slot = load.current;
        let item = load.slots[slot].item;
        let mut g = gear(&data.items, item);
        // zitem `reloadtime` (3..10) is not seconds: revolver 8 vs its 1.33 s clip, rifle 6 vs
        // 2.0 s, no common ratio. The reload clip length is the reload time.
        if let Some(c) = ["reload", "load"]
            .iter()
            .find_map(|n| data.clip(woman, g.motion, n))
        {
            g.reload = c.secs;
        }
        let (origin, dir) = aim(tf.translation, &intent, is_player || remote);
        let facing = Quat::from_rotation_y(intent.yaw);
        let (fwd, right) = (facing * Vec3::NEG_Z, facing * Vec3::X);
        let walk = if alive { intent.walk } else { Vec2::ZERO };
        let jump_edge = intent.jump && !a.prev_jump;
        a.prev_jump = intent.jump;
        let emote = if intent.taunt {
            Some("taunt")
        } else {
            intent.emote
        };
        let emote_edge = emote.filter(|_| alive && emote != a.prev_emote);
        a.prev_emote = emote;
        if intent.attack && !a.prev_attack {
            a.clicked = now;
        }
        a.prev_attack = intent.attack;

        // Actions other modules ask for (never over a blast or a corpse).
        if alive
            && !matches!(a.state, State::Blast { .. })
            && let Some(r) = reqs.iter().rev().find(|r| r.actor == e)
            && let Some(c) = data.clip(woman, g.motion, r.clip)
        {
            let speed = r.speed.max(0.05);
            a.state = State::Action {
                clip: r.clip,
                t: 0.0,
                total: if c.looping.wraps() {
                    f32::INFINITY
                } else {
                    c.secs / speed
                },
                speed,
                moving: r.moving,
                cancel_from: r.cancel_from,
            };
            a.restart = true;
        }

        // Stunned: the looping `stun` clip plays exactly as long as the status.
        let stunned = status.is_some_and(Status::stunned);
        let stun_clip = matches!(a.state, State::Action { clip: "stun", .. });
        if alive
            && stunned
            && !stun_clip
            && !matches!(a.state, State::Blast { .. })
            && data.clip(woman, g.motion, "stun").is_some()
        {
            a.state = State::Action {
                clip: "stun",
                t: 0.0,
                total: f32::INFINITY,
                speed: 1.0,
                moving: ActionMove::Locked,
                cancel_from: f32::INFINITY,
            };
            a.restart = true;
        } else if !stunned && stun_clip {
            a.state = State::Free;
        }

        // Hit reaction: a bullet flinches the upper body (blade hits are melee.rs's).
        if alive
            && matches!(a.state, State::Free)
            && let Some(&(_, by)) = hits.iter().find(|h| h.0 == e)
            && !melee_item(&data.items, by)
        {
            a.flinch = Some(if a.hurt { "damage2" } else { "damage" });
            a.hurt = !a.hurt;
        }

        // State timers and transitions.
        match &mut a.state {
            State::Tumble { left, .. } | State::Wall { left, .. } => {
                *left -= dt;
                if *left <= 0.0
                    || (matches!(a.state, State::Wall { .. })
                        && a.grounded
                        && now > a.wall.map_or(0.0, |w| w.1) + 0.1)
                {
                    a.state = State::Free;
                }
            }
            State::WallRun { left, .. } => {
                *left -= dt;
                let lost = now - a.wall.map_or(f32::MIN, |w| w.1) > WALL_LOSE;
                if a.grounded || lost || (*left > 0.0 && walk.y < 0.2) {
                    a.state = State::Free;
                }
            }
            State::Action {
                clip,
                t,
                total,
                cancel_from,
                ..
            } => {
                *t += dt;
                // an emote ends when the actor walks off (after `cancel_from`)
                let walked_off = *t >= *cancel_from
                    && walk.length() > 0.5
                    && (*clip == "taunt" || EMOTES.iter().any(|e| e.0 == *clip));
                if *t >= *total || walked_off {
                    a.state = State::Free;
                }
            }
            State::Reload { left } => {
                *left -= dt;
                if *left <= 0.0 {
                    let s = &mut load.slots[slot];
                    let take = (g.magazine - s.magazine).min(s.reserve);
                    s.magazine += take;
                    s.reserve -= take;
                    info!(
                        "t={now:.2} reload done: {} mag {}",
                        item_name(&data.items, item),
                        s.magazine
                    );
                    a.state = State::Free;
                }
            }
            State::Blast { stage, t, dagger } => {
                *t += dt;
                let secs = |n: &str| data.clip(woman, g.motion, n).map_or(0.5, |c| c.secs);
                let (rise, drop) = if *dagger {
                    ("blast_dagger", "blast_drop_dagger")
                } else {
                    ("blast", "blast_drop")
                };
                let next = match *stage {
                    Blasted::Rise if a.grounded && *t > 0.1 => Some(Some(Blasted::Drop)),
                    Blasted::Rise if !a.grounded && *t >= secs(rise) => Some(Some(Blasted::Fall)),
                    Blasted::Fall if a.grounded => Some(Some(Blasted::Drop)),
                    Blasted::Drop if *t >= secs(drop) + LIE => Some(Some(Blasted::Stand)),
                    Blasted::Stand if *t >= secs("blast_stand") => Some(None),
                    _ => None,
                };
                match next {
                    Some(Some(s)) => {
                        (*stage, *t) = (s, 0.0);
                        if s == Blasted::Drop {
                            sound.write(ActorSound {
                                actor: e,
                                cue: Cue::Anim("man_jump".into()),
                            });
                        }
                    }
                    Some(None) => a.state = State::Free,
                    None => {}
                }
            }
            _ => {}
        }

        // Jump, dash and wall kick may start from rest or from an action's recovery.
        let cancelable = match a.state {
            State::Free => true,
            State::Action { t, cancel_from, .. } => t >= cancel_from,
            _ => false,
        };

        // Double-tap a direction on the ground: tumble that way.
        let pressed = [walk.y > 0.5, walk.y < -0.5, walk.x < -0.5, walk.x > 0.5];
        for d in 0..4 {
            if pressed[d] && !a.held[d] {
                if now - a.taps[d] < DOUBLE_TAP
                    && a.grounded
                    && (cancelable || matches!(a.state, State::Reload { .. }))
                {
                    let (dir, anim) = [
                        (fwd, "tumbleF"),
                        (-fwd, "tumbleB"),
                        (-right, "tumbleL"),
                        (right, "tumbleR"),
                    ][d];
                    let secs = data.clip(woman, g.motion, anim).map_or(0.6, |c| c.secs);
                    a.state = State::Tumble {
                        dir,
                        anim,
                        left: secs,
                        total: secs,
                    };
                    a.restart = true;
                }
                a.taps[d] = now;
            }
            a.held[d] = pressed[d];
        }

        // Jump and wall kick.
        if alive
            && jump_edge
            && (cancelable || matches!(a.state, State::Reload { .. }))
            && a.grounded
        {
            a.vel.y = JUMP;
            a.grounded = false;
            if matches!(a.state, State::Action { .. }) {
                a.state = State::Free;
            }
        } else if alive
            && jump_edge
            && (cancelable || matches!(a.state, State::WallRun { .. }))
            && !a.grounded
            && let Some((n, t)) = a.wall
            && g.wall
            && now - t < WALL_GRACE
        {
            let facing = fwd.dot(n);
            let anim = if facing < -0.6 {
                "jumpwallF"
            } else if facing > 0.6 {
                "jumpwallB"
            } else if right.dot(n) > 0.0 {
                "jumpwallL"
            } else {
                "jumpwallR"
            };
            let secs = data.clip(woman, g.motion, anim).map_or(0.6, |c| c.secs);
            a.state = State::Wall { anim, left: secs };
            a.restart = true;
            a.vel = n * WALL_OUT + Vec3::Y * WALL_UP;
            a.wall = None;
            sound.write(ActorSound {
                actor: e,
                cue: Cue::Anim("hangonwall".into()),
            });
        }

        // Wall run: in the air, forward held, touching a wall (once per jump).
        if alive
            && matches!(a.state, State::Free)
            && !a.grounded
            && !a.wall_spent
            && g.wall
            && walk.y > 0.5
            && !jump_edge
            && let Some((n, t)) = a.wall
            && now - t < 0.1
            && fwd.dot(n) < 0.3
            && col
                .raycast(
                    tf.translation + Vec3::Y * 0.05,
                    Vec3::NEG_Y,
                    WALL_MIN_HEIGHT,
                )
                .is_none()
        {
            let side = if fwd.dot(n) < -0.7 {
                0.0
            } else if right.dot(n) > 0.0 {
                -1.0
            } else {
                1.0
            };
            a.state = State::WallRun {
                n,
                side,
                left: if side == 0.0 {
                    WALL_RUN_UP
                } else {
                    WALL_RUN_SIDE
                },
            };
            a.wall_spent = true;
            a.restart = true;
            a.vel.y = if side == 0.0 {
                a.vel.y.max(climb(0.0))
            } else {
                a.vel.y.min(2.0)
            };
            sound.write(ActorSound {
                actor: e,
                cue: Cue::Anim("hangonwall".into()),
            });
        }

        // Taunt and emotes: the weapon's clip of that name, standing.
        if let Some(clip) = emote_edge
            && a.grounded
            && matches!(a.state, State::Free)
            && let Some(c) = data.clip(woman, g.motion, clip)
        {
            a.state = State::Action {
                clip,
                t: 0.0,
                total: c.secs,
                speed: 1.0,
                moving: ActionMove::Locked,
                cancel_from: TAUNT_CANCEL,
            };
            a.restart = true;
        }

        // Reload.
        let s = &load.slots[slot];
        let can_reload = !g.melee && s.magazine < g.magazine && s.reserve > 0;
        if alive && matches!(a.state, State::Free) && can_reload && intent.reload {
            begin_reload(a, e, &g, s, item_name(&data.items, item), now, &mut sound);
        }

        // Attack (melee swings and the guard belong to melee.rs).
        if alive && !g.melee && matches!(a.state, State::Free) {
            let trigger = if g.auto {
                intent.attack
            } else {
                now - a.clicked < CLICK_BUFFER
            };
            if trigger && now >= a.ready[slot] {
                a.clicked = f32::MIN;
                let s = &mut load.slots[slot];
                if s.magazine > 0 {
                    s.magazine -= 1;
                    a.ready[slot] = now + g.delay * mods.map_or(1.0, |m| m.shot_delay);
                    a.shot = true;
                    fire.write(Fire {
                        shooter: e,
                        item,
                        origin,
                        dir,
                    });
                } else if now >= a.dry {
                    a.dry = now + 0.4;
                    sound.write(ActorSound {
                        actor: e,
                        cue: Cue::DryFire { item },
                    });
                    if can_reload {
                        begin_reload(a, e, &g, s, item_name(&data.items, item), now, &mut sound);
                    }
                }
            }
        }

        // Movement.
        let mut an = animators.get_mut(a.model).ok();
        let root = an.as_mut().map_or(Vec3::ZERO, |an| an.take_root_motion());
        let root_world = if alive { facing } else { tf.rotation } * root;
        let root_moves = matches!(
            a.state,
            State::Action {
                moving: ActionMove::Root,
                ..
            } | State::Blast {
                stage: Blasted::Stand,
                ..
            } | State::Dead
        );
        let control = match a.state {
            State::Free => 1.0,
            State::Reload { .. } => 0.8,
            State::Action {
                moving: ActionMove::Control(c),
                ..
            } => c,
            _ => 0.0,
        };
        let speed = RUN
            * g.speed
            * control
            * status.map_or(1.0, Status::speed)
            * mods.map_or(1.0, |m| m.run);
        let wish = (right * walk.x + fwd * walk.y) * speed;
        let mut hv = Vec3::new(a.vel.x, 0.0, a.vel.z);
        match a.state {
            State::Tumble {
                dir, left, total, ..
            } => hv = dir * TUMBLE * (0.4 + 0.6 * left / total),
            State::Wall { .. } => {}
            State::WallRun { n, side, left } => {
                let along = (fwd - n * fwd.dot(n)).normalize_or_zero();
                let speed = if left > 0.0 { RUN } else { RUN * 0.5 };
                hv = if side == 0.0 {
                    Vec3::ZERO
                } else {
                    along * speed
                } - n * 1.5;
            }
            State::Dead => hv = approach(hv, Vec3::ZERO, 20.0 * dt),
            State::Blast {
                stage: Blasted::Fall,
                ..
            } if walk != Vec2::ZERO => {
                let steer = (right * walk.x + fwd * walk.y) * RUN * BLAST_STEER;
                hv = approach(hv, steer, 8.0 * dt);
            }
            State::Blast {
                stage: Blasted::Drop | Blasted::Stand,
                ..
            } => hv = approach(hv, Vec3::ZERO, 18.0 * dt),
            State::Blast { .. } => {}
            _ if a.grounded => hv = approach(hv, wish, 60.0 * dt),
            _ if wish != Vec3::ZERO => hv = approach(hv, wish, 10.0 * dt),
            _ => {}
        }
        if let Ok(p) = pushes.get(e) {
            hv += Vec3::new(p.0.x, 0.0, p.0.z);
            if p.0.y > 0.0 {
                a.vel.y = p.0.y;
                a.grounded = false;
            }
            if alive && p.0.y >= BLAST_PUSH {
                a.state = State::Blast {
                    stage: Blasted::Rise,
                    t: 0.0,
                    dagger: hits.iter().any(|&(who, by)| {
                        who == e
                            && data.items.get(by).is_some_and(|i| {
                                i.weapon
                                    .as_ref()
                                    .is_some_and(|w| w.kind == WeaponKind::Dagger)
                            })
                    }),
                };
            }
            commands.entity(e).remove::<Push>();
        }
        let (gravity, fall) = match a.state {
            State::WallRun { side, left, .. } if left > 0.0 && side == 0.0 => {
                a.vel.y = climb(WALL_RUN_UP - left);
                (0.0, FALL)
            }
            State::WallRun { side, left, .. } if left > 0.0 && side != 0.0 => (RUN_GRAVITY, FALL),
            State::WallRun { .. } => (SLIDE_GRAVITY, FALL),
            _ => (1.0, FALL),
        };
        a.vel.y = (a.vel.y - GRAVITY * gravity * dt).max(-fall);
        let mut delta = Vec3::new(hv.x, a.vel.y, hv.z) * dt;
        if root_moves {
            delta += root_world;
        }
        if a.grounded && a.vel.y <= 0.0 {
            delta.y = delta.y.min(-0.05);
        }
        let mv = col.slide_move(tf.translation, delta, RADIUS, HEIGHT);
        let moved_up = mv.pos.y - tf.translation.y;
        tf.translation = mv.pos;
        if alive {
            let mine = teams.get(e).ok().copied();
            for &(_, p, _) in bodies.iter().filter(|b| b.0 != e && !apart(mine, b.2)) {
                let d = Vec3::new(tf.translation.x - p.x, 0.0, tf.translation.z - p.z);
                let len = d.length();
                if len < 2.0 * RADIUS && (tf.translation.y - p.y).abs() < HEIGHT {
                    let n = if len > 1e-3 { d / len } else { right };
                    let out =
                        col.slide_move(tf.translation, n * (2.0 * RADIUS - len), RADIUS, HEIGHT);
                    tf.translation = out.pos;
                }
            }
        }
        if let Some(n) = mv.wall {
            let n = Vec3::new(n.x, 0.0, n.z).normalize_or_zero();
            hv -= n * hv.dot(n).min(0.0);
            if !mv.grounded {
                a.wall = Some((n, now));
            }
        }
        a.vel = Vec3::new(hv.x, a.vel.y, hv.z);
        if mv.grounded {
            a.vel.y = a.vel.y.max(0.0);
        } else if a.vel.y > 0.0 && moved_up < delta.y * 0.5 {
            a.vel.y = 0.0;
        }
        if mv.grounded {
            // thud of a real fall (not a hop or a step down)
            if !a.grounded && a.air > LAND_AIR && !matches!(a.state, State::Blast { .. }) {
                sound.write(ActorSound {
                    actor: e,
                    cue: Cue::Anim("man_jump".into()),
                });
            }
            a.air = 0.0;
            a.wall_spent = false;
        } else {
            a.air += dt;
        }
        a.grounded = mv.grounded;

        // The camera keeps orbiting a corpse; the body keeps the yaw it died with.
        if alive {
            tf.rotation = facing;
        }

        // Face the model toward the strafe direction (forward hemisphere only).
        let twist_to = match a.state {
            State::WallRun { n, side, .. } => {
                let face = if side == 0.0 {
                    -n
                } else {
                    fwd - n * fwd.dot(n)
                };
                (yaw_of(face) - intent.yaw + PI).rem_euclid(2.0 * PI) - PI
            }
            State::Free | State::Reload { .. }
                if a.grounded && walk.length() > 0.1 && walk.y >= 0.0 =>
            {
                -f32::atan2(walk.x, walk.y) * 0.5
            }
            _ => 0.0,
        };
        a.twist += (twist_to - a.twist) * (10.0 * dt).min(1.0);
        if let Ok(mut m) = models.get_mut(a.model) {
            m.rotation = Quat::from_rotation_y(PI + a.twist);
        }

        // Animation: the main clip from the state, an upper-body clip over it (shot, reload,
        // draw, flinch) while the legs keep running.
        let standing = a.grounded && walk.length() < 0.1;
        let name = match a.state {
            State::Dead => a.die,
            State::Tumble { anim, .. } | State::Wall { anim, .. } => anim,
            State::Action { clip, .. } => clip,
            State::Blast { stage, dagger, .. } => match stage {
                Blasted::Rise if dagger => "blast_dagger",
                Blasted::Rise => "blast",
                Blasted::Fall if walk != Vec2::ZERO => "blast_airmove",
                Blasted::Fall => "blast_fall",
                Blasted::Drop if dagger => "blast_drop_dagger",
                Blasted::Drop => "blast_drop",
                Blasted::Stand => "blast_stand",
            },
            State::WallRun { n, side, left } => match (side, left > 0.0) {
                (s, true) if s < 0.0 => "runLW",
                (s, true) if s > 0.0 => "runRW",
                (_, true) => "runW",
                (s, false) if s < 0.0 => "runLW_down",
                (s, false) if s > 0.0 => "runRW_down",
                _ if fwd.dot(n) > 0.3 => "runW_downB",
                _ => "runW_downF",
            },
            _ if !a.grounded => {
                if a.vel.y > 0.0 {
                    "jumpU"
                } else {
                    "jumpD"
                }
            }
            _ if standing => "idle",
            _ if walk.y < -0.1 => "runB",
            _ => "run",
        };
        let want = (g.motion, name);
        let hspeed = Vec2::new(a.vel.x, a.vel.z).length();
        let upper_ok = alive && matches!(a.state, State::Free | State::Reload { .. });
        let start = if !upper_ok {
            None
        } else if matches!(a.state, State::Reload { .. }) {
            (a.upper != Some("reload")).then_some("reload")
        } else if a.draw {
            Some("load")
        } else if a.shot && !g.melee {
            Some("attackS")
        } else {
            a.flinch
        };
        a.shot = false;
        a.draw = false;
        a.flinch = None;
        let reload_clip = a.upper == Some("reload");
        if let Some(an) = an.as_mut() {
            an.root_lock = true;
            if want != a.playing || a.restart {
                a.restart = false;
                a.playing = want;
                a.step = -1;
                match data.clip(woman, want.0, want.1) {
                    Some(c) => {
                        if !c.sound.is_empty() && name != "jumpD" {
                            sound.write(ActorSound {
                                actor: e,
                                cue: Cue::Anim(c.sound.clone()),
                            });
                        }
                        an.play(c.ani.clone(), c.looping, blend_for(name));
                        if !matches!(a.state, State::Free | State::Reload { .. }) {
                            if is_player {
                                info!(
                                    "t={now:.2} clip {name} (motion {}) {:.2}s",
                                    g.motion, c.secs
                                );
                            } else {
                                debug!("t={now:.2} {e} clip {name} (motion {})", g.motion);
                            }
                        }
                    }
                    None => warn!(
                        "character has no animation {name} for motion type {}",
                        g.motion
                    ),
                }
            } else if a.grounded
                && matches!(name, "run" | "runB")
                && matches!(a.state, State::Free | State::Reload { .. })
            {
                let half = (an.time / an.duration().max(0.01) * 2.0) as i32;
                if half != a.step {
                    if a.step >= 0 {
                        sound.write(ActorSound {
                            actor: e,
                            cue: Cue::Footstep {
                                left: half % 2 == 0,
                            },
                        });
                    }
                    a.step = half;
                }
            }
            // Playback rate: run clips play as authored (20 frames, two steps, 0.667 s) at full
            // run speed and slow down with the ground speed; the feet slide (the old foot-locked
            // rate of about 2.4x looked far too quick, user report). Actions use their own speed.
            an.speed = match a.state {
                State::Action { speed, .. } => speed,
                State::Free | State::Reload { .. } if matches!(name, "run" | "runB") => {
                    (hspeed / RUN).clamp(0.6, 1.0)
                }
                _ => 1.0,
            };
            // Upper layer.
            if let Some(clip) = start
                && let Some(c) = data.clip(woman, g.motion, clip)
            {
                an.set_upper(c.ani.clone(), c.looping, 0.05);
                a.upper = Some(clip);
                a.upper_rate = 1.0;
            } else if a.upper.is_some()
                && (!upper_ok || (reload_clip && !matches!(a.state, State::Reload { .. })))
            {
                an.clear_upper(0.08);
                a.upper = None;
            } else if a.upper.is_some() && an.upper_done() {
                a.upper = None;
            }
            an.upper_speed = a.upper_rate / an.speed.max(0.1);
            // Aim pitch on the spine (guns only), eased so a pose change does not snap.
            let pitch_to = if upper_ok && !g.melee {
                intent.pitch
            } else {
                0.0
            };
            a.pitch += (pitch_to - a.pitch) * (12.0 * dt).min(1.0);
            an.aim_pitch = a.pitch.clamp(-1.0, 1.0);
        }

        // What other modules read: the motion snapshot and the running action.
        motor.grounded = a.grounded;
        motor.vel = a.vel;
        motor.tumble = match a.state {
            State::Tumble { left, total, .. } => Some(total - left),
            _ => None,
        };
        motor.wall = matches!(a.state, State::Wall { .. } | State::WallRun { .. });
        motor.blast = matches!(a.state, State::Blast { .. });
        motor.woman = woman;
        match a.state {
            State::Action {
                clip,
                t,
                total,
                cancel_from,
                ..
            } => {
                if let Ok(mut ac) = acting.get_mut(e) {
                    *ac = Acting {
                        clip,
                        time: t,
                        secs: total,
                        cancel_from,
                    };
                } else {
                    commands.entity(e).insert(Acting {
                        clip,
                        time: t,
                        secs: total,
                        cancel_from,
                    });
                }
            }
            _ if acting.contains(e) => {
                commands.entity(e).remove::<Acting>();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_steps_run_in_order() {
        let mut s = Script::parse("w:1.0;yaw=90;jump+d:0.5;wait:0.2").unwrap();
        let mut look = (0.0, 0.0);
        assert!(s.held(0.5, &mut look).fwd);
        assert_eq!(look.0, 0.0);
        let h = s.held(1.2, &mut look);
        assert!(h.jump && h.right && !h.fwd);
        assert!((look.0 - 90f32.to_radians()).abs() < 1e-6);
        assert!(!s.held(1.6, &mut look).jump);
        assert!(Script::parse("q:1").is_err());
    }

    #[test]
    fn script_emotes() {
        let s = Script::parse("wave:1;taunt+dance:1").unwrap();
        assert_eq!(s.steps[0].held.emote, Some("wave"));
        assert_eq!(s.steps[1].held.emote, Some("dance"));
    }
}
