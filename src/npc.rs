//! Quest monsters: `SpawnNpc` -> a hostile `Team::Blue` entity with `Vitals`, a `HitShape`, an
//! animated model from `model/npc*.xml` and an AI, so that guns, blades, blasts and bots treat it
//! like any other actor (combat damages anything with `Vitals`; this module only adds the
//! behaviour). Two families, both documented in `docs/formats.md` "Quest monsters":
//! - `npc.xml` monsters (goblins, kobolds, skeletons, palmpoas, golem; spawn id `"11"`): a chase /
//!   melee / skill loop built from the XML stats and `zskill.xml` skills;
//! - `npc2.xml` actors (guerrillas, lab robots, bosses; spawn id `"knifeman"`): the `aifsm.xml`
//!   state machine driving `zactoraction.xml` actions, executed as data.
//!
//! `gunz-play MAP --npc NAME[,NAME..]` spawns some ahead of the player (headless checks).

mod data;
mod fsm;

use crate::{
    actor::{ActorData, GRAVITY},
    ani::FPS,
    anim::{AnimEvents, Animator, Loop},
    col::MapCollision,
    combat::{HIT_HEIGHT, HIT_RADIUS, Vfx, rnd, yaw_of},
    elu::{self, Elu},
    game::{
        Afflict, Blocked, Bot, CameraShake, Damage, Dead, Guarding, HitShape, Mods, Motor, Npc,
        NpcState, PlaySound, Player, Protected, Push, Routes, SpawnNpc, Team, Vitals, friendly,
    },
    item::Items,
    level::Level,
    model::{self, Textures},
    nav::{Nav, walkable},
    view::{SCALE, to_bevy},
};
use bevy::{ecs::system::SystemParam, mesh::skinning::SkinnedMeshInverseBindposes, prelude::*};
use data::{Action, ActorDef, Data, Ev, Fsm, Func, ModelDef, Next, NpcDef, Skill};
use std::{
    cell::RefCell,
    collections::HashMap,
    f32::consts::{PI, TAU},
    sync::Arc,
};

/// Seconds between AI decisions: `npc.xml` `<SHAKING pathfinding_update attack_update>` (both
/// 0.1, **observed**; the state machines use the same step, **inferred**). `dice:N` is a share of
/// the state's stored random number, see `fsm::next`.
const TICK: f32 = 0.1;
/// How far a classic monster notices a player or bot (m). **Inferred**: `view_angle` is the only
/// sight datum in `npc.xml`.
const AGGRO: f32 = 45.0;
/// A monster notices a player within this many metres even with no line of sight (a hit on the
/// other side of a wall or floor, footsteps). **Inferred**: the data has no hearing range.
const HEARING: f32 = 12.0;
/// Damage multiplier of a critical hit (`mod.criticalrate` of `zskill.xml` is the chance; the
/// multiplier is not in the data). **Inferred.**
const CRIT: f32 = 1.5;
/// `camera.power` x `camera.duration` that shakes the camera at full trauma (the strongest skills
/// in `zskill.xml`: 3.0 x 1.5). **Inferred** mapping onto the HUD's trauma shake.
const SHAKE_FULL: f32 = 4.5;
/// Fraction of a classic melee clip at which the blow lands. **Inferred**: no hit time in the data.
const MELEE_AT: f32 = 0.45;
/// Melee damage of a classic monster with no `weaponitem_id` (palmpoas). **Inferred.**
const BARE_DAMAGE: f32 = 10.0;
/// Seconds between two flinches of a classic monster. **Inferred.**
const FLINCH_EVERY: f32 = 1.5;
/// Turn rate (rad/s) of a classic monster whose `<SPEED>` has no `rotate`. **Inferred.**
const TURN: f32 = 6.0;
/// Widest capsule a monster walks with (m), so big bosses still fit through doors. **Inferred.**
const MOVE_RADIUS: f32 = 0.6;
/// A classic caster stops this far (m) from its target to cast. **Inferred.**
const CASTER_HOLD: f32 = 7.0;
/// How far (m) a slow or stun skill with no area of its own (zskill 151, 165, 351) reaches its
/// target. **Inferred**: the data has no range for them.
const STATUS_RANGE: f32 = 12.0;
/// Seconds a root lasts when its skill has no `effecttime` (missiles: zskill 261-263, 381);
/// **inferred** from the Massive Swing roots, whose `effecttime` is 1000 ms.
const ROOT_SECS: f32 = 1.0;
/// A route waypoint counts as reached within this many metres. **Inferred.**
const ROUTE_REACH: f32 = 2.5;
/// Missiles of a classic skill with no `lifetime` give up after this long (s). **Inferred.**
const MISSILE_LIFE: f32 = 6.0;
/// Blast radius (m), fuse (s), knockback (m/s away, m/s up) and gravity of a monster grenade;
/// `itemid` 40505/40506 of `zactoraction.xml` are not in `zitem.xml`. **Inferred** (frag-like).
const GRENADE_RADIUS: f32 = 3.5;
const GRENADE_FUSE: f32 = 1.5;
const BLAST_PUSH: f32 = 9.0;
const BLAST_LIFT: f32 = 8.0;
const LOB_GRAVITY: f32 = 14.0;
/// Bounce factor of a rolling grenade.
const BOUNCE: f32 = 0.45;
/// Seconds a corpse stays when it has no `dyingtime` (actors).
const CORPSE: f32 = 4.0;
/// A dying actor whose state machine never reaches `__die` is put down after this long (s).
const DIE_FORCE: f32 = 1.5;
/// zskill.xml `knockback` is read as cm/s of horizontal velocity, like zeffect.xml's. **Inferred.**
const KNOCK_UNIT: f32 = 0.01;
/// Share of its health below which a classic monster heals itself. **Inferred.**
const HEAL_BELOW: f32 = 0.7;

pub struct NpcPlugin;

impl Plugin for NpcPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Summon>()
            .add_systems(Startup, init)
            .add_systems(
                Update,
                (demo, spawn, command, intake, run, fly, corpses)
                    .chain()
                    .run_if(resource_exists::<Roster>.and_then(resource_exists::<ActorData>)),
            );
    }
}

/// `gunz-play --npc NAME[,NAME..] [--bots-ahead M]`: monsters spawned `ahead` metres in front of
/// the player once it has landed.
#[derive(Resource)]
pub struct NpcDemo {
    pub names: Vec<String>,
    pub ahead: f32,
}

// ---------------------------------------------------------------------------------------------
// Data

/// A model's clips, parsed once.
struct Clip {
    ani: Arc<crate::ani::Ani>,
    looping: Loop,
    secs: f32,
}

struct ModelData {
    elu: Arc<Elu>,
    dir: String,
    clips: HashMap<String, Clip>,
}

impl ModelData {
    fn secs(&self, clip: &str) -> Option<f32> {
        self.clips.get(clip).map(|c| c.secs)
    }
}

#[derive(Resource)]
struct Roster {
    data: Data,
    events: AnimEvents,
    models: HashMap<String, Arc<ModelData>>,
    /// Missile / grenade body and its material per `resisttype` (0 none, 1 fire, 2 ice,
    /// 3 lightning, 4 poison; 5 bullet).
    orb: Handle<Mesh>,
    orb_material: Vec<Handle<StandardMaterial>>,
}

fn init(
    mut commands: Commands,
    level: Res<Level>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let data = Data::load(&level.vfs).unwrap_or_else(|e| panic!("quest monsters: {e}"));
    let events = AnimEvents::load(&level.vfs).unwrap_or_else(|e| panic!("animationevent.xml: {e}"));
    info!(
        "npc: {} monsters, {} actors, {} skills, {} actions, {} state machines",
        data.npcs.len(),
        data.actors.len(),
        data.skills.len(),
        data.actions.len(),
        data.fsms.len()
    );
    let mut glow = |r: f32, g: f32, b: f32| {
        materials.add(StandardMaterial {
            base_color: Color::srgb(r, g, b),
            emissive: LinearRgba::rgb(r * 4.0, g * 4.0, b * 4.0),
            unlit: true,
            ..default()
        })
    };
    let orb_material = vec![
        glow(0.9, 0.9, 1.0),
        glow(1.0, 0.45, 0.1),
        glow(0.4, 0.8, 1.0),
        glow(1.0, 0.95, 0.3),
        glow(0.4, 1.0, 0.3),
        glow(1.0, 0.85, 0.5),
    ];
    commands.insert_resource(Roster {
        data,
        events,
        models: HashMap::new(),
        orb: meshes.add(Sphere::new(0.12)),
        orb_material,
    });
}

/// Reads a model's ELU and every clip once.
fn load_model(vfs: &crate::mrs::Vfs, def: &ModelDef) -> std::io::Result<ModelData> {
    let elu = elu::load(&vfs.read(&def.elu)?)?;
    let mut clips = HashMap::new();
    for (name, file, looping) in &def.clips {
        match vfs.read(file).and_then(|b| crate::ani::load(&b)) {
            Ok(ani) => {
                clips.insert(
                    name.clone(),
                    Clip {
                        secs: ani.max_frame as f32 / FPS,
                        ani: Arc::new(ani),
                        looping: Loop::from_xml(looping),
                    },
                );
            }
            Err(e) => warn!("{file}: {e}"),
        }
    }
    Ok(ModelData {
        elu: Arc::new(elu),
        dir: def.dir.clone(),
        clips,
    })
}

enum Kind {
    Classic(NpcDef),
    Actor { fsm: Arc<Fsm>, groggy_recover: f32 },
}

/// What one monster is, resolved from the XML at spawn.
struct Spec {
    id: String,
    name: String,
    model: String,
    scale: f32,
    /// Hit capsule (m); it walks with a narrower one ([`MOVE_RADIUS`]).
    radius: f32,
    height: f32,
    speed: f32,
    rot: f32,
    never_pushed: bool,
    never_blasted: bool,
    dying: f32,
    hp: f32,
    ap: f32,
    boss: bool,
    drop: String,
    die_sound: String,
    team: Team,
    /// `npc2.xml` `type` (empty for the classic monsters).
    type_name: String,
    kind: Kind,
}

fn resolve(data: &Data, req: &SpawnNpc) -> Option<Spec> {
    let k = if req.hp_scale > 0.0 {
        req.hp_scale
    } else {
        1.0
    };
    if let Ok(id) = req.id.parse::<u32>() {
        let d = data.npcs.get(&id)?;
        return Some(Spec {
            id: req.id.clone(),
            // The shipped strings name some monsters in Korean only (the HUD font has no Hangul):
            // those show the mesh name.
            name: data
                .strings
                .get(&d.name_key)
                .filter(|s| s.is_ascii())
                .cloned()
                .unwrap_or_else(|| d.mesh.clone()),
            model: d.mesh.clone(),
            scale: d.scale,
            radius: d.radius_cm * 0.01,
            height: d.height_cm * 0.01,
            speed: d.speed_cm * 0.01,
            rot: d.rotate.unwrap_or(TURN),
            never_pushed: d.never_pushed,
            never_blasted: d.never_blasted,
            dying: d.dying_secs,
            hp: d.max_hp * k,
            ap: d.max_ap * k,
            boss: req.boss || d.grade == "boss",
            drop: if req.drop.is_empty() {
                d.drop.clone()
            } else {
                req.drop.clone()
            },
            die_sound: String::new(),
            team: req.team.unwrap_or(Team::Blue),
            type_name: String::new(),
            kind: Kind::Classic(d.clone()),
        });
    }
    let a: &ActorDef = data.actors.get(&req.id)?;
    Some(Spec {
        id: req.id.clone(),
        name: a.name.clone(),
        model: a.model.clone(),
        scale: 1.0,
        radius: a.radius_cm * 0.01,
        height: a.height_cm * 0.01,
        speed: a.speed_cm * 0.01,
        rot: a.rotspeed.max(0.5),
        never_pushed: false,
        never_blasted: a.never_blasted,
        dying: CORPSE,
        hp: a.max_hp * k,
        ap: a.max_ap * k,
        boss: req.boss || a.boss,
        drop: req.drop.clone(),
        die_sound: a.sound_die.clone(),
        team: req.team.unwrap_or(if a.name.ends_with("_red") {
            Team::Red
        } else {
            Team::Blue
        }),
        type_name: a.kind.clone(),
        kind: Kind::Actor {
            fsm: data.fsms.get(&a.fsm)?.clone(),
            groggy_recover: a.groggy_recover,
        },
    })
}

// ---------------------------------------------------------------------------------------------
// The brain

/// What an actor is playing: an action, with its events due at `t` seconds.
struct Running {
    action: Arc<Action>,
    t: f32,
    next: usize,
    secs: f32,
    /// Looping clip: it never ends by itself.
    endless: bool,
}

#[derive(Component)]
struct Brain {
    spec: Arc<Spec>,
    model: Arc<ModelData>,
    root: Entity,
    /// `lhand`, `rhand`, `head` bone entities and where they were last frame.
    parts: [Option<Entity>; 3],
    part_at: [Option<Vec3>; 3],
    yaw: f32,
    vy: f32,
    grounded: bool,
    kick: Vec3,
    speed: f32,
    goal: f32,
    accel: f32,
    heading: Vec3,
    face: Option<f32>,
    target: Option<Entity>,
    attacker: Option<Entity>,
    hurt: f32,
    groggy: f32,
    path: Vec<Vec3>,
    pi: usize,
    path_t: f32,
    failed: bool,
    orbit: f32,
    act: Option<Running>,
    playing: String,
    clip_t: f32,
    clip_ev: Vec<(f32, String)>,
    clip_next: usize,
    clock: f32,
    state: usize,
    entered: f32,
    seen: Vec<f32>,
    ready: HashMap<u32, f32>,
    melee_ready: f32,
    flinch_ready: f32,
    idle_for: f32,
    seed: u32,
    /// The state's stored random number (`dice` functions), see `fsm::next`.
    dice: f32,
    /// Seconds since the death sequence began.
    dying: Option<f32>,
    dead_for: f32,
    /// `GUNZ_NPC_HOLD=SECS`: stand idle this long after spawning (headless shots of the idle pose).
    hold_until: f32,
    /// The waypoints it marches along (`runWaypointsAlongRoute`) and the next one.
    route: Vec<Vec3>,
    ri: usize,
    /// A state a mode forces it into ([`NpcState`]) and whether its entry action has begun.
    forced: Option<String>,
    started: bool,
}

/// A player, bot or monster (of another side) a monster may fight.
#[derive(Clone, Copy)]
struct Foe {
    e: Entity,
    feet: Vec3,
    face: Vec3,
    guarding: bool,
    protected: bool,
    team: Option<Team>,
    bot: bool,
    /// Hit capsule (metres).
    radius: f32,
    height: f32,
    /// A building or crate: no blood.
    solid: bool,
}

impl Foe {
    fn chest(&self) -> Vec3 {
        self.feet + Vec3::Y * 1.2f32.min(self.height * 0.67)
    }
}

type Hostiles<'w, 's> = Query<
    'w,
    's,
    (
        Entity,
        &'static GlobalTransform,
        Option<&'static Team>,
        Has<Bot>,
        Has<Guarding>,
        Has<Protected>,
        Option<&'static HitShape>,
        Option<&'static Npc>,
    ),
    (With<Vitals>, Without<Dead>),
>;

fn foes(q: &Hostiles) -> Vec<Foe> {
    q.iter()
        .map(|(e, g, team, bot, guarding, protected, shape, npc)| Foe {
            e,
            feet: g.translation(),
            face: g.rotation() * Vec3::NEG_Z,
            guarding,
            protected,
            team: team.copied(),
            bot,
            radius: shape.map_or(HIT_RADIUS, |s| s.radius),
            height: shape.map_or(HIT_HEIGHT, |s| s.height),
            solid: npc.is_some_and(|n| {
                matches!(
                    n.kind.as_str(),
                    "barricade" | "radar" | "guardian" | "honor_item"
                )
            }),
        })
        .collect()
}

/// Everything an AI step reads.
struct Cx<'a> {
    now: f32,
    dt: f32,
    roster: &'a Roster,
    items: &'a Items,
    col: &'a MapCollision,
    nav: Option<&'a Nav>,
    foes: &'a [Foe],
    /// Living summons per summoner.
    summons: &'a HashMap<Entity, u32>,
    /// Heals cast this frame: (where, radius m, hp), applied to every monster of the caster's side
    /// in reach.
    heals: &'a RefCell<Vec<(Vec3, f32, f32, Team)>>,
}

#[derive(SystemParam)]
struct Out<'w> {
    damage: MessageWriter<'w, Damage>,
    afflict: MessageWriter<'w, Afflict>,
    blocked: MessageWriter<'w, Blocked>,
    vfx: MessageWriter<'w, Vfx>,
    sound: MessageWriter<'w, PlaySound>,
    summon: MessageWriter<'w, Summon>,
    shake: MessageWriter<'w, CameraShake>,
}

/// A summoner's request, spawned like a `SpawnNpc` with a link back (for `SummonLess`).
#[derive(Message)]
struct Summon {
    by: Entity,
    req: SpawnNpc,
}

#[derive(Component)]
struct Summoned(Entity);

fn play(out: &mut Out, stem: &str, at: Vec3) {
    if !stem.is_empty() {
        let stem = stem.replace('\\', "/");
        // `lab_chaser_summon` (chaser_summon, chaser_summon2) has no wav in the data: the lab
        // bots' only other summon cue stands in. **Inferred** (`lab_tower_summon`, same dust puff).
        let stem = if stem.ends_with("lab_chaser_summon") {
            "challenge_quest/researchlab/lab_tower_summon".into()
        } else {
            stem
        };
        out.sound.write(PlaySound { stem, at });
    }
}

fn turn(cur: f32, goal: f32, max: f32) -> f32 {
    let d = (goal - cur + PI).rem_euclid(TAU) - PI;
    cur + d.clamp(-max, max)
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

fn sees(col: &MapCollision, from: Vec3, to: Vec3) -> bool {
    let v = to - from;
    col.raycast(from, v, v.length())
        .is_none_or(|h| h.distance >= v.length() - 0.3)
}

impl Brain {
    fn forward(&self) -> Vec3 {
        Quat::from_rotation_y(self.yaw) * Vec3::NEG_Z
    }

    fn foe(&self, cx: &Cx, e: Option<Entity>) -> Option<Foe> {
        let e = e?;
        cx.foes.iter().find(|f| f.e == e).copied()
    }

    fn hostile(&self, f: &Foe) -> bool {
        // A crate belongs to nobody (and is not worth a soldier's time).
        !f.protected
            && (f.team.is_some() || !f.solid)
            && !friendly((Some(self.spec.team), false), (f.team, f.bot))
    }

    /// The nearest hostile within `max` metres (and `dy` metres of height difference).
    fn nearest(&self, cx: &Cx, pos: Vec3, max: f32, dy: Option<f32>) -> Option<Foe> {
        cx.foes
            .iter()
            .filter(|f| self.hostile(f))
            .filter(|f| dy.is_none_or(|h| (f.feet.y - pos.y).abs() <= h))
            .map(|f| (f.feet.distance(pos), f))
            .filter(|(d, _)| *d <= max)
            .min_by(|a, b| a.0.total_cmp(&b.0))
            .map(|(_, f)| *f)
    }

    /// World position of the bone `part` names (`lhand`, `rhand`, `head`), else the chest.
    fn muzzle(&self, feet: Vec3, part: &str) -> Vec3 {
        let i = match part {
            "lhand" => 0,
            "rhand" => 1,
            "head" => 2,
            _ => {
                return feet + Vec3::Y * self.spec.height * 0.6 + self.forward() * self.spec.radius;
            }
        };
        self.part_at[i].unwrap_or(feet + Vec3::Y * self.spec.height * 0.6)
    }

    /// Plays clip `name` (when it exists and is not already playing, or `force`).
    fn play(&mut self, anim: &mut Animator, name: &str, blend: f32, force: bool) -> bool {
        let Some(c) = self.model.clips.get(name) else {
            return false;
        };
        if force || self.playing != name {
            anim.play(c.ani.clone(), c.looping, blend);
            self.playing = name.to_string();
            self.clip_t = 0.0;
            self.clip_next = 0;
            self.clip_ev.clear();
        }
        true
    }

    /// The `animationevent.xml` sounds of `clip` (classic monsters only: keyed by NPC id).
    fn clip_sounds(&mut self, cx: &Cx, clip: &str) {
        if let Kind::Classic(d) = &self.spec.kind {
            self.clip_ev = cx
                .roster
                .events
                .get(d.id, clip)
                .iter()
                .map(|e| (e.secs, e.sound.clone()))
                .collect();
        }
    }

    /// Starts `action`: its clip, its timeline.
    fn start(&mut self, anim: &mut Animator, action: Arc<Action>, cx: &Cx) {
        let last = action.events.last().map_or(0.0, |e| e.0);
        let (secs, endless) =
            self.model
                .clips
                .get(&action.animation)
                .map_or((last + 0.1, false), |c| {
                    // The classic monsters' attack clips are authored `loop`; their actions still end.
                    let looping = c.looping.wraps() && matches!(self.spec.kind, Kind::Actor { .. });
                    (c.secs.max(last + 0.05), looping)
                });
        if self.play(anim, &action.animation, 0.1, true) {
            self.clip_sounds(cx, &action.animation);
        }
        self.act = Some(Running {
            action,
            t: 0.0,
            next: 0,
            secs,
            endless,
        });
    }

    fn end_action(&self) -> bool {
        self.act
            .as_ref()
            .is_none_or(|a| !a.endless && a.t >= a.secs)
    }

    fn steering(&mut self, goal: f32, heading: Vec3, face: Option<f32>) {
        (self.goal, self.heading, self.face) = (goal, heading, face);
    }

    fn stop(&mut self) {
        self.goal = 0.0;
    }

    /// Heads for `to`: straight when the floor is continuous, else along a route of the nav graph
    /// (when the map has one).
    fn go(&mut self, cx: &Cx, from: Vec3, to: Vec3, speed: f32) {
        let direct = walkable(cx.col, from, to);
        let mut want = to;
        if !direct && let Some(nav) = cx.nav {
            if self.path_t <= 0.0 || self.path.is_empty() {
                self.path = nav
                    .route(from, to)
                    .iter()
                    .map(|s| nav.nodes[s.node as usize])
                    .collect();
                self.pi = 0;
                self.path_t = 0.6;
                self.failed = self.path.is_empty();
                if self.failed {
                    debug!(
                        "t={:.2} npc: {} no route {from:?} -> {to:?}",
                        cx.now, self.spec.name
                    );
                }
            }
            while self.pi < self.path.len() && flat(self.path[self.pi] - from).length() < 0.7 {
                self.pi += 1;
            }
            want = self.path.get(self.pi).copied().unwrap_or(to);
        } else {
            self.path.clear();
            self.failed = false;
        }
        let d = flat(want - from).normalize_or_zero();
        self.steering(speed, d, Some(yaw_of(d)));
    }

    /// Along its route: the next waypoint not reached yet; the last one is held.
    fn march(&mut self, cx: &Cx, from: Vec3, speed: f32) {
        while self.ri + 1 < self.route.len()
            && flat(self.route[self.ri] - from).length() < ROUTE_REACH
        {
            self.ri += 1;
            self.path.clear();
        }
        match self.route.get(self.ri) {
            Some(&to)
                if self.ri + 1 < self.route.len() || flat(to - from).length() > ROUTE_REACH =>
            {
                self.go(cx, from, to, speed);
            }
            _ => self.stop(),
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Spawning

fn demo(
    demo: Option<Res<NpcDemo>>,
    mut done: Local<bool>,
    player: Query<(&Transform, &Motor), With<Player>>,
    col: Res<MapCollision>,
    mut spawn: MessageWriter<SpawnNpc>,
) {
    let Some(d) = demo else { return };
    let Ok((p, m)) = player.single() else { return };
    if *done || !m.grounded {
        return;
    }
    *done = true;
    debug!("npc demo: player at {:?}", p.translation);
    let n = d.names.len() as f32;
    for (i, name) in d.names.iter().enumerate() {
        let side = (i as f32 - (n - 1.0) / 2.0) * 3.0 + 0.35;
        let at = p.translation + *p.forward() * d.ahead + *p.right() * side;
        // The floor within a step or two of the player's own height.
        let at = col
            .raycast(
                Vec3::new(at.x, p.translation.y + 1.5, at.z),
                Vec3::NEG_Y,
                3.0,
            )
            .map_or(at, |h| h.point);
        spawn.write(SpawnNpc {
            id: name.clone(),
            pos: at,
            yaw: yaw_of(p.translation - at),
            hp_scale: 1.0,
            drop: String::new(),
            boss: false,
            ..default()
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn spawn(
    mut msgs: MessageReader<SpawnNpc>,
    mut summons: MessageReader<Summon>,
    mut roster: ResMut<Roster>,
    level: Res<Level>,
    col: Res<MapCollision>,
    nav: Option<Res<Nav>>,
    time: Res<Time>,
    routes: Res<Routes>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut images: ResMut<Assets<Image>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
) {
    let reqs: Vec<(Option<Entity>, SpawnNpc)> = msgs
        .read()
        .map(|r| (None, r.clone()))
        .chain(summons.read().map(|s| (Some(s.by), s.req.clone())))
        .collect();
    if reqs.is_empty() {
        return;
    }
    // Monsters route through the same floor graph as bots; the map has one once either needs it.
    if nav.is_none() {
        let (mut min, mut max) = (Vec3::MAX, Vec3::MIN);
        for v in &level.map.vertices {
            let p = Vec3::from(to_bevy(v.pos)) * SCALE;
            (min, max) = (min.min(p), max.max(p));
        }
        let t = std::time::Instant::now();
        let n = Nav::new(&col, min, max);
        info!(
            "npc nav: {} floor nodes in {:.2}s",
            n.nodes.len(),
            t.elapsed().as_secs_f32()
        );
        commands.insert_resource(n);
    }
    let vfs = &level.vfs;
    let mut textures = Textures::new(vfs, "model/");
    for (by, mut req) in reqs {
        if req.route != 0 {
            // `ENHANCE_NPC`: the waves grow stronger as the match goes on.
            let k = if req.hp_scale > 0.0 {
                req.hp_scale
            } else {
                1.0
            };
            req.hp_scale = k * (1.0 + routes.boost);
        }
        let Some(spec) = resolve(&roster.data, &req) else {
            warn!("npc: no monster {:?} in npc.xml or npc2.xml", req.id);
            continue;
        };
        if !roster.models.contains_key(&spec.model) {
            let loaded = roster
                .data
                .model(vfs, &spec.model)
                .and_then(|d| load_model(vfs, &d));
            match loaded {
                Ok(m) => {
                    roster.models.insert(spec.model.clone(), Arc::new(m));
                }
                Err(e) => {
                    warn!("npc: {} model {}: {e}", spec.id, spec.model);
                    continue;
                }
            }
        }
        let md = roster.models[&spec.model].clone();
        let dir = md.dir.clone();
        let mut material =
            |m: &elu::Material| textures.standard(&mut images, &mut standard, &dir, m);
        let body = model::spawn_elu(
            &mut commands,
            &mut meshes,
            &mut bindposes,
            &md.elu,
            &mut material,
            Transform::from_rotation(Quat::from_rotation_y(PI)).with_scale(Vec3::splat(spec.scale)),
            |_| true,
        );
        let idle = md
            .clips
            .get("idle")
            .or_else(|| md.clips.values().next())
            .unwrap_or_else(|| panic!("npc {}: model {} has no clips", spec.id, spec.model));
        let mut animator = Animator::new(idle.ani.clone(), idle.looping);
        animator.elu = Some(md.elu.clone());
        animator.root_lock = true;
        commands.entity(body.root).insert(animator);
        let part = |n: &str| body.nodes.get(n).copied();
        let now = time.elapsed_secs();
        let mut seed = 0x9E37_79B9u32 ^ (now * 1000.0) as u32 ^ req.pos.x.to_bits();
        let (fsm_states, entry) = match &spec.kind {
            Kind::Actor { fsm, .. } => (fsm.states.len(), fsm.entry),
            Kind::Classic(_) => (0, 0),
        };
        let mut ready = HashMap::new();
        if let Kind::Classic(d) = &spec.kind {
            for id in &d.skills {
                let delay = roster
                    .data
                    .skills
                    .get(id)
                    .map_or(0.0, |s| s.delay_ms * 0.001);
                // Not everything at first sight (**inferred**).
                ready.insert(*id, now + delay * (0.25 + 0.25 * rnd(&mut seed)));
            }
        }
        let brain = Brain {
            model: md,
            root: body.root,
            parts: [
                part("Bip01 L Hand"),
                part("Bip01 R Hand"),
                part("Bip01 Head"),
            ],
            part_at: [None; 3],
            yaw: req.yaw,
            vy: 0.0,
            grounded: false,
            kick: Vec3::ZERO,
            speed: 0.0,
            goal: 0.0,
            accel: f32::INFINITY,
            heading: Vec3::ZERO,
            face: None,
            target: None,
            attacker: None,
            hurt: 0.0,
            groggy: 0.0,
            path: Vec::new(),
            pi: 0,
            path_t: 0.0,
            failed: false,
            orbit: 1.0,
            act: None,
            playing: "idle".into(),
            clip_t: 0.0,
            clip_ev: Vec::new(),
            clip_next: 0,
            clock: rnd(&mut seed) * TICK,
            state: entry,
            entered: now,
            seen: vec![f32::NEG_INFINITY; fsm_states],
            ready,
            melee_ready: now + 1.0,
            flinch_ready: 0.0,
            idle_for: 0.0,
            seed,
            dice: 0.0,
            dying: None,
            dead_for: 0.0,
            hold_until: now
                + std::env::var("GUNZ_NPC_HOLD")
                    .ok()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0.0),
            route: routes.paths.get(&req.route).cloned().unwrap_or_default(),
            ri: 0,
            forced: None,
            started: false,
            spec: Arc::new(spec),
        };
        let s = brain.spec.clone();
        let e = commands
            .spawn((
                Name::new(s.name.clone()),
                Transform::from_translation(req.pos).with_rotation(Quat::from_rotation_y(req.yaw)),
                Visibility::default(),
                Npc {
                    id: s.id.clone(),
                    drop: s.drop.clone(),
                    boss: s.boss,
                    kind: s.type_name.clone(),
                },
                Vitals {
                    hp: s.hp,
                    ap: s.ap,
                    max_hp: s.hp,
                    max_ap: s.ap,
                },
                HitShape {
                    radius: s.radius,
                    height: s.height,
                },
                brain,
            ))
            .add_child(body.root)
            .id();
        if s.type_name != "honor_item" {
            commands.entity(e).insert(s.team);
        }
        info!(
            "npc: spawned {} ({}) at {:.1?}: hp {:.0} ap {:.0}, scale {}, {}{}",
            s.name,
            s.id,
            req.pos,
            s.hp,
            s.ap,
            s.scale,
            match &s.kind {
                Kind::Classic(d) => format!("{} skills", d.skills.len()),
                Kind::Actor { fsm, .. } => format!("FSM {}", fsm.name),
            },
            if req.route != 0 {
                format!(", {:?} route {}", s.team, req.route)
            } else {
                String::new()
            }
        );
        if let Some(by) = by {
            commands.entity(e).insert(Summoned(by));
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Taking hits

/// `game::NpcState`: a mode sends a state machine into a named state.
fn command(mut msgs: MessageReader<NpcState>, mut npcs: Query<&mut Brain>) {
    for m in msgs.read() {
        if let Ok(mut b) = npcs.get_mut(m.npc) {
            b.forced = Some(m.state.clone());
        }
    }
}

fn intake(
    mut dmg: MessageReader<Damage>,
    mut npcs: Query<&mut Brain>,
    mods: Query<&Mods>,
    humans: Query<(), Or<(With<Player>, With<Bot>)>>,
    pushes: Query<(Entity, &Push), With<Npc>>,
    mut commands: Commands,
) {
    for d in dmg.read() {
        if let Ok(mut b) = npcs.get_mut(d.target) {
            // A building's wear shows what it takes after its resistances.
            let k = mods
                .get(d.target)
                .map_or(1.0, |m| m.against(humans.contains(d.attacker)));
            b.groggy += d.amount * k;
            b.hurt += d.amount;
            b.attacker = Some(d.attacker);
        }
    }
    for (e, p) in &pushes {
        if let Ok(mut b) = npcs.get_mut(e) {
            if !b.spec.never_pushed {
                b.kick += flat(p.0);
            }
            if !b.spec.never_blasted && p.0.y > 5.0 {
                b.vy = p.0.y * 0.6;
            }
        }
        commands.entity(e).remove::<Push>();
    }
}

// ---------------------------------------------------------------------------------------------
// Thinking, acting, moving

#[allow(clippy::too_many_arguments)]
fn run(
    time: Res<Time>,
    roster: Res<Roster>,
    data: Res<ActorData>,
    col: Res<MapCollision>,
    nav: Option<Res<Nav>>,
    mut out: Out,
    mut commands: Commands,
    mut npcs: Query<(Entity, &mut Brain, &mut Transform, &mut Vitals, Has<Dead>), With<Npc>>,
    hostiles: Hostiles,
    globals: Query<&GlobalTransform>,
    mut animators: Query<&mut Animator>,
    summoned: Query<&Summoned, Without<Dead>>,
) {
    let (dt, now) = (time.delta_secs().min(0.05), time.elapsed_secs());
    let foes = foes(&hostiles);
    let mut summons: HashMap<Entity, u32> = HashMap::new();
    for s in &summoned {
        *summons.entry(s.0).or_default() += 1;
    }
    let heals = RefCell::new(Vec::new());
    let cx = Cx {
        now,
        dt,
        roster: &roster,
        items: &data.items,
        col: &col,
        nav: nav.as_deref(),
        foes: &foes,
        summons: &summons,
        heals: &heals,
    };
    for (e, mut b, mut tf, v, dead) in &mut npcs {
        let Ok(mut anim) = animators.get_mut(b.root) else {
            continue;
        };
        let b = &mut *b;
        let root = anim.take_root_motion();
        let pos = tf.translation;
        b.part_at = b
            .parts
            .map(|p| p.and_then(|e| globals.get(e).ok()).map(|g| g.translation()));
        if dead && b.dying.is_none() {
            b.dead_for += dt;
            // A classic monster just dies; an actor waits for its state machine's `__die`.
            if b.dead_for > DIE_FORCE || matches!(b.spec.kind, Kind::Classic(_)) {
                begin_death(b, &mut anim, &cx, &mut out, pos);
            }
        }
        b.clock += dt;
        b.path_t -= dt;
        b.clip_t += dt;
        while let Some((t, s)) = b.clip_ev.get(b.clip_next)
            && *t <= b.clip_t
        {
            play(&mut out, s, pos + Vec3::Y);
            b.clip_next += 1;
        }
        if let Some(t) = &mut b.dying {
            *t += dt;
        } else if dead || cx.now >= b.hold_until {
            if let Kind::Actor { groggy_recover, .. } = &b.spec.kind {
                b.groggy = (b.groggy - groggy_recover * dt).max(0.0);
            }
            if b.clock >= TICK {
                b.clock -= TICK;
                think(b, &mut anim, &cx, &mut out, e, pos, v.hp, v.max_hp);
            }
            steer(b, &cx, pos);
            timeline(b, &cx, &mut out, &mut commands, e, pos, dead);
        }
        let alive = b.dying.is_none() && !dead;
        let moves = b.dying.is_none() && b.act.as_ref().is_some_and(|a| a.action.moving);
        locomote(
            b,
            &cx,
            &mut tf,
            if moves { root } else { Vec3::ZERO },
            alive,
        );
        // The clip when no action plays: `run` while it moves, else `idle`.
        if alive && b.act.is_none() {
            let clip = if b.speed > 0.3 { "run" } else { "idle" };
            b.play(&mut anim, clip, 0.15, false);
        }
    }
    npcs_heal(&mut npcs, &heals);
}

/// Applies the heals cast this frame to every monster of the caster's side in reach.
fn npcs_heal(
    npcs: &mut Query<(Entity, &mut Brain, &mut Transform, &mut Vitals, Has<Dead>), With<Npc>>,
    heals: &RefCell<Vec<(Vec3, f32, f32, Team)>>,
) {
    for (at, radius, hp, team) in heals.borrow().iter() {
        for (_, b, tf, mut v, dead) in npcs.iter_mut() {
            if !dead && b.spec.team == *team && tf.translation.distance(*at) <= *radius {
                v.hp = (v.hp + hp).min(v.max_hp);
            }
        }
    }
}

fn begin_death(b: &mut Brain, anim: &mut Animator, cx: &Cx, out: &mut Out, pos: Vec3) {
    b.dying = Some(0.0);
    b.act = None;
    b.stop();
    b.speed = 0.0;
    let clip = if b.model.clips.contains_key("die2") && rnd(&mut b.seed) < 0.5 {
        "die2"
    } else {
        "die"
    };
    b.play(anim, clip, 0.05, true);
    b.clip_sounds(cx, clip);
    if matches!(b.spec.kind, Kind::Actor { .. }) {
        play(out, &b.spec.die_sound, pos + Vec3::Y);
    }
    info!("t={:.2} npc: {} dies ({clip})", cx.now, b.spec.name);
}

/// Despawns corpses once their dying time is over.
fn corpses(mut commands: Commands, q: Query<(Entity, &Brain)>) {
    for (e, b) in &q {
        if b.dying.is_some_and(|t| t >= b.spec.dying.max(0.5)) {
            commands.entity(e).despawn();
        }
    }
}

// ---------------------------------------------------------------------------------------------
// AI

#[allow(clippy::too_many_arguments)]
fn think(
    b: &mut Brain,
    anim: &mut Animator,
    cx: &Cx,
    out: &mut Out,
    me: Entity,
    pos: Vec3,
    hp: f32,
    max_hp: f32,
) {
    let spec = b.spec.clone();
    match &spec.kind {
        Kind::Classic(d) => {
            if hp > 0.0 {
                classic(b, anim, cx, pos, d, hp / max_hp.max(1.0));
            }
        }
        Kind::Actor { fsm, .. } => machine(b, anim, cx, out, me, pos, hp, fsm),
    }
}

fn angle_to(b: &Brain, from: Vec3, to: Vec3) -> f32 {
    b.forward()
        .angle_between(flat(to - from).normalize_or(b.forward()))
}

/// A classic monster's decision: target, skill, blow, or the way to the target.
fn classic(b: &mut Brain, anim: &mut Animator, cx: &Cx, pos: Vec3, d: &NpcDef, hp_frac: f32) {
    // Target: whoever hurt it last, else the one it sees within AGGRO or hears within HEARING
    // (through floors and walls; it then follows the nav graph). The foe it already has is kept
    // while within AGGRO * 1.5, sight or not, so a detour round a wall does not drop it.
    let mut tgt = b
        .foe(cx, b.target)
        .or_else(|| b.foe(cx, b.attacker))
        .filter(|f| f.feet.distance(pos) < AGGRO * 1.5);
    if tgt.is_none() {
        tgt = b.nearest(cx, pos, AGGRO, None).filter(|f| {
            f.feet.distance(pos) < HEARING || sees(cx.col, pos + Vec3::Y * 1.2, f.chest())
        });
    }
    b.target = tgt.map(|f| f.e);
    let hurt = std::mem::take(&mut b.hurt);
    if b.act.is_some() && b.end_action() {
        b.act = None;
    }
    let busy = b.act.as_ref().is_some_and(|a| a.action.name != "flinch");
    let Some(foe) = tgt else {
        b.stop();
        b.idle_for += TICK;
        if b.act.is_none()
            && b.idle_for > 4.0 + 4.0 * rnd(&mut b.seed)
            && b.model.clips.contains_key("neglect1")
        {
            // An idle fidget (the clips are named `neglect*`; **inferred** to be idle animations).
            b.idle_for = 0.0;
            let a = Arc::new(Action {
                name: "fidget".into(),
                animation: "neglect1".into(),
                moving: false,
                events: Vec::new(),
            });
            b.start(anim, a, cx);
        }
        return;
    };
    b.idle_for = 0.0;
    let to = foe.feet - pos;
    let dist = flat(to).length();
    let aim = angle_to(b, pos, foe.feet);
    // Flinch (not bosses, not while mid-attack).
    if hurt > 0.0 && !b.spec.boss && !busy && cx.now >= b.flinch_ready {
        let clip = if rnd(&mut b.seed) < 0.5 {
            "melee_attacked1"
        } else {
            "melee_attacked2"
        };
        if b.model.clips.contains_key(clip) {
            b.flinch_ready = cx.now + FLINCH_EVERY;
            let a = Arc::new(Action {
                name: "flinch".into(),
                animation: clip.into(),
                moving: false,
                events: Vec::new(),
            });
            b.start(anim, a, cx);
            b.stop();
            return;
        }
    }
    b.face = Some(yaw_of(to));
    if busy {
        b.stop();
        return;
    }
    let reach = d.range_cm * 0.01;
    let turn_tol = d.view_angle.max(25.0).to_radians();
    let sees_it = sees(cx.col, pos + Vec3::Y * 1.2, foe.chest());
    // Skills first.
    for id in &d.skills {
        let Some(s) = cx.roster.data.skills.get(id) else {
            continue;
        };
        if b.ready.get(id).is_some_and(|t| *t > cx.now) || !usable(s, dist, aim, sees_it, hp_frac) {
            continue;
        }
        b.ready.insert(*id, cx.now + s.delay_ms * 0.001);
        let a = skill_action(s, &b.model);
        b.start(anim, a, cx);
        b.stop();
        return;
    }
    // The blow.
    if dist <= reach + HIT_RADIUS && aim <= turn_tol && cx.now >= b.melee_ready {
        let ai = &cx.roster.data.ai;
        let interval = ai
            .agility
            .get((d.agility as usize).wrapping_sub(1))
            .copied()
            .unwrap_or(1.0);
        b.melee_ready = cx.now + interval;
        let a = melee_action(d, cx.items, &b.model);
        b.start(anim, a, cx);
        b.stop();
        return;
    }
    // Close in (casters hold back when they have a missile to throw).
    let caster = d.offense == 2
        && d.skills
            .iter()
            .any(|id| cx.roster.data.skills.get(id).is_some_and(|s| s.missile));
    let hold = if caster { CASTER_HOLD } else { reach * 0.8 };
    if dist > hold {
        let speed = b.spec.speed;
        b.go(cx, pos, foe.feet, speed);
    } else {
        b.stop();
    }
}

/// Whether classic skill `s` is worth casting now (**inferred** from the XML fields).
fn usable(s: &Skill, dist: f32, aim: f32, sees: bool, hp_frac: f32) -> bool {
    if s.missile {
        return sees && (2.0..=30.0).contains(&dist) && aim < 0.5;
    }
    if s.effect_type == 6 {
        return hp_frac < HEAL_BELOW; // not `heal > 0`: the blizzards carry a mod.heal too
    }
    if s.damage <= 0.0 {
        // Slow and stun (zskill 151, 165, 351): the target in sight within reach.
        return afflict(s, Entity::PLACEHOLDER, Entity::PLACEHOLDER).is_some()
            && sees
            && dist <= STATUS_RANGE;
    }
    if s.effect_type == 2 {
        return sees && dist <= 12.0;
    }
    let cone = s.angle >= 360.0 || aim <= (s.angle * 0.5).to_radians();
    dist >= s.area_min && dist <= s.area && cone
}

/// A classic monster's melee blow: the weapon item's damage and swing at `MELEE_AT` of the clip.
fn melee_action(d: &NpcDef, items: &Items, m: &ModelData) -> Arc<Action> {
    let w = items.get(d.weapon).and_then(|i| i.weapon.as_ref());
    let secs = m.secs("melee_attack").unwrap_or(1.0);
    Arc::new(Action {
        name: "melee_attack".into(),
        animation: "melee_attack".into(),
        moving: false,
        events: vec![(
            secs * MELEE_AT,
            Ev::Melee {
                damage: w.map_or(BARE_DAMAGE, |w| w.damage as f32),
                range_cm: d.range_cm,
                angle: w.and_then(|w| w.angle).map_or(90.0, |a| a as f32),
                uppercut: false,
                thrust: false,
                sound: String::new(),
                pierce: None,
            },
        )],
    })
}

/// The cast of skill `s`: clip `special_attack<castinganimation>` (else the melee clip) with its
/// events at `effectstarttime`.
fn skill_action(s: &Skill, m: &ModelData) -> Arc<Action> {
    let clip = format!("special_attack{}", s.cast_anim);
    let animation = if s.cast_anim > 0 && m.clips.contains_key(&clip) {
        clip
    } else {
        "melee_attack".to_string()
    };
    Arc::new(Action {
        name: format!("skill {}", s.id),
        animation,
        moving: false,
        events: skill_events(s),
    })
}

fn skill_events(s: &Skill) -> Vec<(f32, Ev)> {
    let t0 = s.start_ms * 0.001;
    let mut ev = Vec::new();
    if !s.cast_pre_effect.is_empty() {
        ev.push((
            0.0,
            Ev::Effect {
                mesh: s.cast_pre_effect.clone(),
                part: String::new(),
                pos_cm: [0.0; 3],
                scale: 1.0,
            },
        ));
    }
    if !s.cast_effect.is_empty() {
        ev.push((
            t0,
            Ev::Effect {
                mesh: s.cast_effect.clone(),
                part: String::new(),
                pos_cm: s.cast_offset_cm,
                scale: 1.0,
            },
        ));
    }
    if s.missile {
        ev.push((
            t0,
            Ev::Missile {
                skill: s.id,
                yaw: 0.0,
            },
        ));
        // Each REPEAT follows the previous one by its delay (**inferred**: cumulative).
        let mut t = t0;
        for (delay, yaw) in &s.repeats {
            t += delay;
            ev.push((
                t,
                Ev::Missile {
                    skill: s.id,
                    yaw: *yaw,
                },
            ));
        }
    } else if s.effect_type == 6 {
        ev.push((t0, Ev::Heal(s.id)));
    } else {
        ev.push((t0, Ev::Area(s.id)));
    }
    ev.sort_by(|a, b| a.0.total_cmp(&b.0));
    ev
}

// ---- state machine actors ---------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn machine(
    b: &mut Brain,
    anim: &mut Animator,
    cx: &Cx,
    out: &mut Out,
    me: Entity,
    pos: Vec3,
    hp: f32,
    fsm: &Fsm,
) {
    if !b.started {
        // The entry state's action begins at once (a radar's first wave).
        b.started = true;
        b.dice = rnd(&mut b.seed);
        let entry = fsm.states[b.state].action.as_ref();
        if let Some(a) = entry.and_then(|a| cx.roster.data.actions.get(a)) {
            b.start(anim, a.clone(), cx);
        }
    }
    // Per-tick functions.
    for f in &fsm.states[b.state].funcs {
        let found = match f {
            Func::FindTarget => b.nearest(cx, pos, f32::MAX, None),
            Func::FindInHeight(cm) => b.nearest(cx, pos, f32::MAX, Some(cm * 0.01)),
            Func::FindInDist(cm) => b.nearest(cx, pos, cm * 0.01, None),
            Func::Dice => {
                b.dice = rnd(&mut b.seed);
                continue;
            }
            _ => continue,
        };
        b.target = found.map(|f| f.e);
    }
    let foe = b.foe(cx, b.target);
    if foe.is_none() {
        b.target = None;
    }
    let eye = pos + Vec3::Y * b.spec.height * 0.7;
    let (dist, look, elev, height, sight) = foe.map_or((f32::MAX, 180.0, 0.0, 0.0, false), |f| {
        let to = f.chest() - eye;
        (
            (f.feet.distance(pos) - (f.radius - HIT_RADIUS).max(0.0)).max(0.0) * 100.0,
            angle_to(b, pos, f.feet).to_degrees(),
            to.y.atan2(flat(to).length()).to_degrees(),
            (f.feet.y - pos.y) * 100.0,
            sees(cx.col, eye, f.chest()),
        )
    });
    let yaw = b.yaw;
    let empty = |angle: f32, cm: f32| {
        let d = Quat::from_rotation_y(yaw - angle.to_radians()) * Vec3::NEG_Z;
        walkable(cx.col, pos, pos + d * cm * 0.01)
    };
    let sense = fsm::Sense {
        elapsed_ms: (cx.now - b.entered) * 1000.0,
        hp,
        groggy: b.groggy,
        has_target: foe.is_some(),
        dist,
        sees: sight,
        look,
        elevation: elev,
        height,
        end_action: b.end_action(),
        path_failed: b.failed,
        summons: cx.summons.get(&me).copied().unwrap_or(0),
        empty: &empty,
        dice: b.dice,
    };
    let forced = b
        .forced
        .take()
        .and_then(|n| fsm.states.iter().position(|s| s.name == n));
    let now = cx.now;
    let seen = &b.seen;
    let since = |i: usize| (now - seen[i]) * 1000.0;
    let Some(next) = forced
        .map(Next::State)
        .or_else(|| fsm::next(fsm, b.state, &sense, &since))
    else {
        return;
    };
    match next {
        Next::Die => begin_death(b, anim, cx, out, pos),
        Next::State(i) => {
            for f in &fsm.states[b.state].exit {
                apply_enter(b, f, cx, pos);
            }
            debug!(
                "t={:.2} npc: {} {} -> {}",
                cx.now, b.spec.name, fsm.states[b.state].name, fsm.states[i].name
            );
            b.state = i;
            b.entered = cx.now;
            b.seen[i] = cx.now;
            b.accel = f32::INFINITY;
            for f in &fsm.states[i].enter {
                apply_enter(b, f, cx, pos);
            }
            match fsm.states[i]
                .action
                .as_ref()
                .and_then(|a| cx.roster.data.actions.get(a))
            {
                Some(a) => b.start(anim, a.clone(), cx),
                None => b.act = None,
            }
        }
    }
}

/// Entry / exit functions.
fn apply_enter(b: &mut Brain, f: &Func, cx: &Cx, pos: Vec3) {
    let foe = b.foe(cx, b.target);
    match f {
        Func::ReduceGroggy(n) => b.groggy = (b.groggy - n).max(0.0),
        Func::FaceToTarget => {
            if let Some(f) = foe {
                b.yaw = yaw_of(f.feet - pos);
            }
        }
        Func::FaceToAttacker => {
            if let Some(f) = b.foe(cx, b.attacker).or(foe) {
                b.yaw = yaw_of(f.feet - pos);
            }
        }
        Func::BuildWaypoints => {
            b.path.clear();
            b.pi = 0;
            b.path_t = 0.0;
            b.failed = false;
        }
        Func::ClearWaypoints => {
            b.path.clear();
            b.pi = 0;
            b.failed = false;
        }
        Func::TurnOrbit => b.orbit = -b.orbit,
        Func::Dice => b.dice = rnd(&mut b.seed),
        _ => {}
    }
}

/// The state's continuous functions: turning and running.
fn steer(b: &mut Brain, cx: &Cx, pos: Vec3) {
    let spec = b.spec.clone();
    let Kind::Actor { fsm, .. } = &spec.kind else {
        return;
    };
    let foe = b.foe(cx, b.target);
    b.stop();
    b.face = None;
    for f in &fsm.states[b.state].funcs {
        match (f, foe) {
            (Func::Accel(a), _) => b.accel = a * 0.01,
            (Func::RotateToTarget, Some(f)) => b.face = Some(yaw_of(f.feet - pos)),
            (Func::FaceToTarget, Some(f)) => b.yaw = yaw_of(f.feet - pos),
            (Func::RunWaypoints, Some(f)) => b.go(cx, pos, f.feet, spec.speed),
            (Func::RunRoute, _) => b.march(cx, pos, spec.speed),
            (Func::Orbit(cm), Some(f)) => {
                let radial = flat(f.feet - pos).normalize_or_zero();
                let tangent = Vec3::new(-radial.z, 0.0, radial.x) * b.orbit;
                let dir = (tangent + radial * 0.1).normalize_or_zero();
                b.steering(cm * 0.01, dir, Some(yaw_of(radial)));
            }
            _ => {}
        }
    }
}

/// Moves the capsule: turn, speed, kick, root motion, gravity, collision.
fn locomote(b: &mut Brain, cx: &Cx, tf: &mut Transform, root: Vec3, alive: bool) {
    let dt = cx.dt;
    if alive && let Some(g) = b.face {
        b.yaw = turn(b.yaw, g, b.spec.rot * dt);
    }
    let rooted = b.act.as_ref().is_some_and(|a| a.action.moving);
    let goal = if alive && !rooted { b.goal } else { 0.0 };
    b.speed = if b.accel.is_finite() {
        let step = b.accel * dt;
        b.speed + (goal - b.speed).clamp(-step, step)
    } else {
        goal
    };
    let walk = if alive {
        b.heading * b.speed
    } else {
        Vec3::ZERO
    };
    let travel = Quat::from_rotation_y(b.yaw) * root * b.spec.scale;
    if !b.grounded {
        b.vy -= GRAVITY * dt;
    }
    let delta = (walk + b.kick) * dt + travel + Vec3::Y * b.vy * dt;
    b.kick *= (1.0 - 6.0 * dt).max(0.0);
    let radius = b.spec.radius.min(MOVE_RADIUS);
    let mut m = cx
        .col
        .slide_move(tf.translation, delta, radius, b.spec.height.min(2.0));
    let want = flat(delta).length();
    if goal > 0.0 && flat(m.pos - tf.translation).length() < want * 0.3 {
        // Wedged on a seam the capsule sweep catches but the map does not show (a goblin stood
        // 2 m from the player in Mansion): the first of a few headings off the intended one that
        // makes headway takes over. **Inferred** remedy; the data has no walking rules.
        for deg in [30.0f32, -30.0, 60.0, -60.0, 90.0, -90.0] {
            let d = Quat::from_rotation_y(deg.to_radians()) * delta;
            let alt = cx
                .col
                .slide_move(tf.translation, d, radius, b.spec.height.min(2.0));
            if flat(alt.pos - tf.translation).length() >= want * 0.5 {
                m = alt;
                break;
            }
        }
    }
    tf.translation = m.pos;
    b.grounded = m.grounded;
    if m.grounded && b.vy < 0.0 {
        b.vy = 0.0;
    }
    tf.rotation = Quat::from_rotation_y(b.yaw);
}

// ---------------------------------------------------------------------------------------------
// Action timelines

fn timeline(
    b: &mut Brain,
    cx: &Cx,
    out: &mut Out,
    commands: &mut Commands,
    me: Entity,
    pos: Vec3,
    dead: bool,
) {
    let Some(a) = &mut b.act else { return };
    a.t += cx.dt;
    let (action, t) = (a.action.clone(), a.t);
    loop {
        let Some(r) = b.act.as_mut() else { break };
        let Some((at, ev)) = action.events.get(r.next) else {
            break;
        };
        if *at > t {
            break;
        }
        r.next += 1;
        // The dead only make noise.
        if !dead || matches!(ev, Ev::Effect { .. } | Ev::Sound(_)) {
            fire(b, ev, cx, out, commands, me, pos);
        }
    }
}

fn fire(
    b: &Brain,
    ev: &Ev,
    cx: &Cx,
    out: &mut Out,
    commands: &mut Commands,
    me: Entity,
    pos: Vec3,
) {
    let foe = b.foe(cx, b.target);
    let fwd = b.forward();
    let aim = foe.map_or(pos + Vec3::Y * 1.2 + fwd * 10.0, |f| f.chest());
    let item = match &b.spec.kind {
        Kind::Classic(d) => d.weapon,
        Kind::Actor { .. } => 0,
    };
    match ev {
        Ev::Effect {
            mesh,
            part,
            pos_cm,
            scale,
        } => {
            if mesh.is_empty() {
                return;
            }
            let base = if part.is_empty() {
                pos
            } else {
                b.muzzle(pos, part)
            };
            // x runs forward, y sideways, z up (**inferred** from `castingeffectAddPos="780 0 0"`
            // matching the 5.8-10.2 m band of the same skill).
            let local = Vec3::new(pos_cm[1], pos_cm[2], -pos_cm[0]) * 0.01 * b.spec.scale;
            out.vfx.write(Vfx::Named {
                name: mesh.clone(),
                at: Transform {
                    translation: base + Quat::from_rotation_y(b.yaw) * local,
                    rotation: Quat::from_rotation_y(b.yaw),
                    scale: Vec3::splat(*scale * b.spec.scale.max(1.0)),
                },
            });
        }
        Ev::Sound(s) => play(out, s, pos + Vec3::Y),
        Ev::Melee {
            damage,
            range_cm,
            angle,
            uppercut,
            thrust,
            sound,
            pierce,
        } => {
            play(out, sound, pos + Vec3::Y);
            let reach = range_cm * 0.01;
            let half = (angle.max(16.0) * 0.5).to_radians().min(PI);
            let centre = pos + Vec3::Y;
            for f in cx.foes.iter().filter(|f| b.hostile(f)) {
                let to = flat(f.feet - pos);
                let dist = to.length();
                if dist - f.radius > reach || (f.feet.y - pos.y).abs() > b.spec.height.max(2.0) {
                    continue;
                }
                if dist > f.radius && fwd.angle_between(to) > half {
                    continue;
                }
                if !sees(cx.col, centre, f.chest()) {
                    continue;
                }
                let d = (f.chest() - centre).normalize_or(fwd);
                if f.guarding && f.face.angle_between(-fwd) <= 60f32.to_radians() {
                    out.blocked.write(Blocked(f.e));
                    out.vfx.write(Vfx::Elu {
                        name: "ef_sword_flash",
                        at: Transform::from_translation(f.chest()),
                    });
                    continue;
                }
                let push = if *uppercut {
                    flat(d).normalize_or_zero() * 2.0 + Vec3::Y * 8.0
                } else if *thrust {
                    flat(d).normalize_or_zero() * 6.0
                } else {
                    Vec3::ZERO
                };
                hurt(out, commands, me, f, *damage, item, *pierce, centre, push);
            }
        }
        Ev::Shot {
            damage,
            sound,
            mesh,
            speed_cm,
            radius_cm,
            dirmod,
            part,
            thrust,
            pierce,
        } => {
            play(out, sound, pos + Vec3::Y);
            let from = b.muzzle(pos, part);
            let right = fwd.cross(Vec3::Y);
            let d = ((aim - from).normalize_or(fwd)
                + right * dirmod[0]
                + Vec3::Y * dirmod[1]
                + fwd * dirmod[2])
                .normalize_or(fwd);
            let mut m = Missile::new(me, &b.spec, d * speed_cm * 0.01, *damage, radius_cm * 0.01);
            (m.trail, m.scale, m.knock) = (mesh.clone(), 0.5, if *thrust { 6.0 } else { 0.0 });
            m.pierce = *pierce;
            commands.spawn(m.bundle(cx.roster, from, 5));
        }
        Ev::Grenade {
            damage,
            force,
            pitch,
            yaw,
            pos_cm,
            sound,
            impact,
            pierce,
        } => {
            play(out, sound, pos + Vec3::Y);
            let from = pos + Vec3::Y * (pos_cm[2] * 0.01).max(1.0) + fwd * 0.3;
            let heading = Quat::from_rotation_y(b.yaw - yaw.to_radians()) * Vec3::NEG_Z;
            let p = pitch.to_radians();
            let v = (heading * p.cos() + Vec3::Y * p.sin()) * force * 0.01;
            let mut m = Missile::new(me, &b.spec, v, *damage, 0.15);
            m.gravity = LOB_GRAVITY;
            m.splash = GRENADE_RADIUS;
            m.impact = *impact;
            m.fuse = GRENADE_FUSE;
            m.boom = "we_grenade_explosion".into();
            m.pierce = *pierce;
            commands.spawn(m.bundle(cx.roster, from, 0));
        }
        Ev::Summon {
            name,
            range_cm,
            angle,
            route,
            drop,
        } => {
            let at = pos
                + Quat::from_rotation_y(b.yaw - angle.to_radians()) * Vec3::NEG_Z * range_cm * 0.01;
            out.summon.write(Summon {
                by: me,
                req: SpawnNpc {
                    id: name.clone(),
                    pos: at,
                    yaw: b.yaw,
                    hp_scale: 1.0,
                    drop: drop.clone(),
                    team: Some(b.spec.team),
                    route: *route,
                    ..default()
                },
            });
        }
        Ev::Missile { skill, yaw } => {
            let Some(s) = cx.roster.data.skills.get(skill) else {
                return;
            };
            let from = b.muzzle(pos, "");
            let d = Quat::from_rotation_y(-yaw) * (aim - from).normalize_or(fwd);
            let mut m = Missile::new(
                me,
                &b.spec,
                d * s.velocity * 0.01,
                s.damage,
                s.colradius * 0.01,
            );
            if s.lifetime_ms > 0.0 {
                m.life = s.lifetime_ms * 0.001;
            }
            (m.trail, m.scale, m.knock) = (
                s.trail.clone(),
                s.trail_scale * 0.5,
                s.knockback * KNOCK_UNIT,
            );
            m.boom = s.sound_explosion.clone();
            m.guide = s.guidable.then_some(b.target).flatten();
            m.status = afflict(s, me, Entity::PLACEHOLDER);
            m.crit = s.crit;
            commands.spawn(m.bundle(cx.roster, from, s.resist as usize));
        }
        Ev::Area(skill) => {
            let Some(s) = cx.roster.data.skills.get(skill) else {
                return;
            };
            // A cone band in front of it, or (blizzard) a disc at the target.
            let ground = s.effect_type == 2;
            let centre = if ground {
                foe.map_or(pos, |f| f.feet)
            } else {
                pos
            };
            let mut hit = 0;
            for f in cx.foes.iter().filter(|f| b.hostile(f)) {
                let to = flat(f.feet - centre);
                let dist = to.length();
                let ok = if s.area <= 0.0 {
                    // No area: the skill's own target (slow, stun).
                    b.target == Some(f.e)
                        && to.length() <= STATUS_RANGE
                        && sees(cx.col, pos + Vec3::Y, f.chest())
                } else if ground {
                    dist <= s.area
                } else {
                    dist >= s.area_min.min(s.area)
                        && dist <= s.area
                        && (s.angle >= 360.0
                            || fwd.angle_between(to.normalize_or(fwd))
                                <= (s.angle * 0.5).to_radians())
                        && sees(cx.col, pos + Vec3::Y, f.chest())
                };
                if !ok {
                    continue;
                }
                let d = flat(f.feet - pos).normalize_or(fwd);
                if s.damage > 0.0 {
                    let k = crit(s.crit, cx.now, f.e);
                    if k > 1.0 {
                        debug!("t={:.2} npc: {} {} crits", cx.now, b.spec.name, s.name);
                    }
                    hurt(
                        out,
                        commands,
                        me,
                        f,
                        s.damage * k,
                        item,
                        None,
                        pos + Vec3::Y,
                        d * s.knockback * KNOCK_UNIT,
                    );
                }
                if let Some(a) = afflict(s, me, f.e) {
                    out.afflict.write(a);
                }
                hit += 1;
            }
            debug!("t={:.2} npc: {} {} hits {hit}", cx.now, b.spec.name, s.name);
            if s.camera[2] > 0.0 {
                debug!(
                    "t={:.2} npc: {} {} shakes the camera (power {} x {} s within {} cm)",
                    cx.now, b.spec.name, s.name, s.camera[0], s.camera[1], s.camera[2]
                );
                out.shake.write(CameraShake {
                    at: pos,
                    trauma: (s.camera[0] * s.camera[1] / SHAKE_FULL).min(1.0),
                    range: s.camera[2] * 0.01,
                });
            }
            play(out, &s.sound_explosion, pos);
        }
        Ev::Heal(skill) => {
            if let Some(s) = cx.roster.data.skills.get(skill) {
                cx.heals
                    .borrow_mut()
                    .push((pos, s.area.max(0.5), s.heal, b.spec.team));
            }
        }
    }
}

/// [`CRIT`] when a `mod.criticalrate` roll (seeded by the clock and the victim) succeeds, else 1.
fn crit(rate: f32, now: f32, victim: Entity) -> f32 {
    let mut seed =
        ((now * 1000.0) as u32 ^ (victim.to_bits() as u32).wrapping_mul(2_654_435_761)) | 1;
    if rnd(&mut seed) < rate { CRIT } else { 1.0 }
}

/// One blow on `f`: damage, blood and the knockback push.
#[allow(clippy::too_many_arguments)]
fn hurt(
    out: &mut Out,
    commands: &mut Commands,
    by: Entity,
    f: &Foe,
    amount: f32,
    item: u32,
    pierce: Option<f32>,
    from: Vec3,
    push: Vec3,
) {
    let dir = (f.chest() - from).normalize_or(Vec3::NEG_Z);
    out.damage.write(Damage {
        target: f.e,
        attacker: by,
        amount,
        item,
        point: f.chest(),
        dir,
        pierce,
    });
    if f.solid {
        out.vfx.write(Vfx::Spark {
            point: f.chest(),
            normal: -dir,
        });
    } else {
        out.vfx.write(Vfx::Blood {
            point: f.chest(),
            dir,
        });
    }
    if push != Vec3::ZERO {
        commands.entity(f.e).insert(Push(push));
    }
}

/// What skill `s` does to its victims besides hurting them: its slow (`mod.speed` below 100),
/// stun (the retail Stun skill is the one casting `ef_stun`: zskill 351), root (`mod.root`) and
/// damage over time (`mod.dot`, spread over `effecttime`), all lasting `effecttime`; `None`
/// when it has none of them.
fn afflict(s: &Skill, by: Entity, target: Entity) -> Option<Afflict> {
    let stun = s.cast_pre_effect == "ef_stun";
    let slow = (s.speed_pct / 100.0).min(1.0);
    (slow < 1.0 || stun || s.root || s.dot > 0.0).then(|| Afflict {
        target,
        by,
        secs: if s.effect_ms > 0.0 {
            s.effect_ms / 1000.0
        } else {
            ROOT_SECS
        },
        slow,
        stun,
        root: s.root,
        dot: s.dot,
    })
}

// ---------------------------------------------------------------------------------------------
// Missiles and grenades

#[derive(Component)]
struct Missile {
    owner: Entity,
    team: Team,
    name: String,
    vel: Vec3,
    damage: f32,
    radius: f32,
    life: f32,
    /// Effect-list name spawned along the flight (empty: none).
    trail: String,
    scale: f32,
    puff: f32,
    knock: f32,
    /// Sound stem of the burst.
    boom: String,
    guide: Option<Entity>,
    gravity: f32,
    fuse: f32,
    splash: f32,
    impact: bool,
    /// Share of a hit that reaches health (`None`: the weapon's own).
    pierce: Option<f32>,
    /// Slow, stun, root, dot of the skill, applied to what it hits.
    status: Option<Afflict>,
    /// `mod.criticalrate` of the skill: chance of [`CRIT`] times the damage on a direct hit.
    crit: f32,
}

impl Missile {
    fn new(owner: Entity, spec: &Spec, vel: Vec3, damage: f32, radius: f32) -> Self {
        Self {
            owner,
            team: spec.team,
            name: spec.name.clone(),
            vel,
            damage,
            radius: radius.max(0.1),
            life: MISSILE_LIFE,
            trail: String::new(),
            scale: 1.0,
            puff: 0.0,
            knock: 0.0,
            boom: String::new(),
            guide: None,
            gravity: 0.0,
            fuse: f32::INFINITY,
            splash: 0.0,
            impact: true,
            pierce: None,
            status: None,
            crit: 0.0,
        }
    }

    /// The entity: a glowing orb (`resist` colours it) that the trail effect follows.
    fn bundle(
        self,
        r: &Roster,
        at: Vec3,
        resist: usize,
    ) -> (
        Transform,
        Mesh3d,
        MeshMaterial3d<StandardMaterial>,
        Visibility,
        Missile,
    ) {
        (
            Transform::from_translation(at).with_scale(Vec3::splat(
                // The glow is a core inside the trail effect: never wider than its scale (the
                // golem rocket's 90 cm `colradius` is only the hit test). **Inferred.**
                (self.radius / 0.12).min(self.scale).clamp(0.6, 8.0),
            )),
            Mesh3d(r.orb.clone()),
            MeshMaterial3d(r.orb_material[resist.min(r.orb_material.len() - 1)].clone()),
            Visibility::default(),
            self,
        )
    }
}

fn fly(
    time: Res<Time>,
    col: Res<MapCollision>,
    mut commands: Commands,
    mut out: Out,
    mut shots: Query<(Entity, &mut Transform, &mut Missile)>,
    hostiles: Hostiles,
) {
    let dt = time.delta_secs().min(0.05);
    let foes = foes(&hostiles);
    for (e, mut tf, mut m) in &mut shots {
        m.life -= dt;
        m.fuse -= dt;
        if m.life <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        let pos = tf.translation;
        if let Some(g) = m.guide.and_then(|g| foes.iter().find(|f| f.e == g)) {
            let speed = m.vel.length();
            let dir = m.vel / speed.max(0.01);
            let k = (2.0 * dt).min(1.0);
            m.vel = (dir * (1.0 - k) + (g.chest() - pos).normalize_or(dir) * k).normalize_or_zero()
                * speed;
        }
        m.vel.y -= m.gravity * dt;
        let step = m.vel * dt;
        let len = step.length().max(1e-4);
        let wall = col.raycast(pos, step, len);
        let mut best: Option<(f32, Foe)> = None;
        for f in foes
            .iter()
            .filter(|f| !f.protected && !friendly((Some(m.team), false), (f.team, f.bot)))
        {
            let s = ((f.chest() - pos).dot(step) / (len * len)).clamp(0.0, 1.0);
            let q = pos + step * s;
            let (dx, dy) = (flat(q - f.feet).length(), q.y - f.feet.y);
            if dx <= m.radius + f.radius
                && dy >= -m.radius
                && dy <= f.height + m.radius
                && best.is_none_or(|b| s < b.0)
            {
                best = Some((s, *f));
            }
        }
        let wall_at = wall.as_ref().map(|h| h.distance / len);
        let mut burst = None;
        if let Some((s, f)) = best.filter(|(s, _)| wall_at.is_none_or(|w| *s <= w)) {
            burst = Some((pos + step * s, Some(f), None));
        } else if let Some(h) = &wall {
            if m.impact {
                burst = Some((h.point, None, Some(h.normal)));
            } else {
                // A rolling grenade bounces.
                let v = m.vel;
                m.vel = (v - h.normal * v.dot(h.normal) * 2.0) * BOUNCE;
                tf.translation = h.point + h.normal * 0.1;
            }
        } else {
            tf.translation += step;
        }
        m.puff -= dt;
        if m.puff <= 0.0 && !m.trail.is_empty() {
            m.puff = 0.07;
            out.vfx.write(Vfx::Named {
                name: m.trail.clone(),
                at: Transform::from_translation(tf.translation).with_scale(Vec3::splat(m.scale)),
            });
        }
        let Some((at, who, normal)) =
            burst.or((m.fuse <= 0.0).then_some((tf.translation, None, None)))
        else {
            continue;
        };
        if m.splash > 0.0 {
            for f in foes
                .iter()
                .filter(|f| !f.protected && !friendly((Some(m.team), false), (f.team, f.bot)))
            {
                let (lo, hi) = (f.radius.min(f.height * 0.5), (f.height - f.radius).max(0.0));
                let c = f.feet + Vec3::Y * (at.y - f.feet.y).clamp(lo, hi.max(lo));
                let v = c - at;
                let dist = v.length();
                let factor = (1.0 - (dist - f.radius).max(0.0) / m.splash).clamp(0.0, 1.0);
                if factor <= 0.0
                    || col
                        .raycast(at, v, dist)
                        .is_some_and(|h| h.distance < dist - 0.4)
                {
                    continue;
                }
                let d = v.normalize_or(Vec3::Y);
                let push =
                    (flat(d).normalize_or_zero() * BLAST_PUSH + Vec3::Y * BLAST_LIFT) * factor;
                hurt(
                    &mut out,
                    &mut commands,
                    m.owner,
                    f,
                    m.damage * factor,
                    0,
                    m.pierce,
                    at,
                    push,
                );
            }
            out.vfx.write(Vfx::Elu {
                name: "grenade_effect",
                at: Transform::from_translation(at).with_scale(Vec3::splat(m.splash * 0.25)),
            });
        } else if let Some(f) = who {
            let d = m.vel.normalize_or(Vec3::NEG_Z);
            let push = flat(d).normalize_or_zero() * m.knock;
            hurt(
                &mut out,
                &mut commands,
                m.owner,
                &f,
                m.damage * crit(m.crit, time.elapsed_secs(), f.e),
                0,
                m.pierce,
                at - d,
                push,
            );
            if let Some(a) = m.status {
                out.afflict.write(Afflict { target: f.e, ..a });
            }
        } else if let Some(normal) = normal {
            out.vfx.write(Vfx::Spark { point: at, normal });
        }
        play(&mut out, &m.boom, at);
        debug!(
            "t={:.2} npc: {} missile bursts at {at:.1?}",
            time.elapsed_secs(),
            m.name
        );
        commands.entity(e).despawn();
    }
}

// ---------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skill_events_follow_effect_start_and_repeats() {
        let s = Skill {
            id: 162,
            missile: true,
            start_ms: 1200.0,
            cast_effect: "x".into(),
            cast_pre_effect: "pre".into(),
            repeats: vec![(0.5, 0.2), (0.5, -0.2)],
            ..Default::default()
        };
        let ev = skill_events(&s);
        let times: Vec<f32> = ev.iter().map(|e| e.0).collect();
        assert_eq!(times, vec![0.0, 1.2, 1.2, 1.7, 2.2]);
        assert!(matches!(ev[4].1, Ev::Missile { yaw, .. } if yaw == -0.2));
        let heal = Skill {
            heal: 25.0,
            effect_type: 6,
            start_ms: 2000.0,
            ..Default::default()
        };
        assert!(matches!(skill_events(&heal)[0].1, Ev::Heal(_)));
        let hit = Skill {
            damage: 5.0,
            ..Default::default()
        };
        assert!(matches!(skill_events(&hit)[0].1, Ev::Area(_)));
    }

    #[test]
    fn classic_skill_use() {
        let cone = Skill {
            damage: 100.0,
            area: 10.2,
            area_min: 5.8,
            angle: 33.0,
            ..Default::default()
        };
        assert!(usable(&cone, 8.0, 0.1, true, 1.0));
        assert!(!usable(&cone, 3.0, 0.1, true, 1.0), "inside the inner band");
        assert!(!usable(&cone, 8.0, 0.6, true, 1.0), "outside the cone");
        let bolt = Skill {
            missile: true,
            damage: 15.0,
            ..Default::default()
        };
        assert!(usable(&bolt, 10.0, 0.0, true, 1.0) && !usable(&bolt, 10.0, 0.0, false, 1.0));
        let heal = Skill {
            heal: 25.0,
            effect_type: 6,
            ..Default::default()
        };
        assert!(usable(&heal, 3.0, 0.0, true, 0.5) && !usable(&heal, 3.0, 0.0, true, 1.0));
        let slow = Skill {
            speed_pct: 50.0,
            ..Default::default()
        };
        assert!(usable(&slow, 3.0, 0.0, true, 1.0));
        assert!(!usable(&slow, 3.0, 0.0, false, 1.0) && !usable(&slow, 20.0, 0.0, true, 1.0));
    }

    #[test]
    fn skills_afflict_their_victims() {
        let (a, b) = (Entity::PLACEHOLDER, Entity::PLACEHOLDER);
        // Slow (151): half speed for effecttime, no stun.
        let slow = Skill {
            speed_pct: 50.0,
            effect_ms: 7000.0,
            ..Default::default()
        };
        let s = afflict(&slow, a, b).unwrap();
        assert_eq!(
            (s.slow, s.secs, s.stun, s.root, s.dot),
            (0.5, 7.0, false, false, 0.0)
        );
        // Stun (351): the `ef_stun` skill, 65 % speed for 3 s.
        let stun = Skill {
            speed_pct: 65.0,
            effect_ms: 3000.0,
            cast_pre_effect: "ef_stun".into(),
            ..Default::default()
        };
        assert!(afflict(&stun, a, b).unwrap().stun);
        // A blizzard roots (1 s without effecttime) and burns; it is not a heal.
        let blizzard = Skill {
            effect_type: 2,
            root: true,
            dot: 30.0,
            heal: 50.0,
            damage: 30.0,
            area: 5.0,
            ..Default::default()
        };
        let s = afflict(&blizzard, a, b).unwrap();
        assert_eq!((s.root, s.dot, s.secs), (true, 30.0, 1.0));
        assert!(matches!(skill_events(&blizzard)[0].1, Ev::Area(_)));
        let plain = Skill {
            speed_pct: 100.0,
            ..Default::default()
        };
        assert!(afflict(&plain, a, b).is_none());
    }
}
