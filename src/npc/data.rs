//! Parsers for the quest-monster data files: `system/npc.xml` (classic monsters, by id),
//! `system/npc2.xml` (challenge-quest actors, by name), `system/zskill.xml`,
//! `system/zactoraction.xml`, `system/aifsm.xml` and the `model/npc*.xml` registries. Layouts and
//! what each attribute is believed to mean: `docs/formats.md` "Quest monsters".
//! Lengths stay in the files' own units (cm, ms) here; `npc.rs` converts.

use crate::mrs::Vfs;
use roxmltree::{Document, Node};
use std::{
    collections::HashMap,
    io::{self, ErrorKind},
    sync::Arc,
};

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(ErrorKind::InvalidData, msg.into())
}

fn text(vfs: &Vfs, path: &str) -> io::Result<String> {
    let bytes = vfs.read(path)?;
    Ok(String::from_utf8_lossy(&bytes)
        .trim_start_matches('\u{feff}')
        .to_string())
}

fn doc<'a>(text: &'a str, what: &str) -> io::Result<Document<'a>> {
    Document::parse(text).map_err(|e| bad(format!("{what}: {e}")))
}

/// Numeric attribute, 0 when absent (the files omit what does not apply).
fn num(n: Node, a: &str) -> f32 {
    n.attribute(a)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0)
}

fn flag(n: Node, a: &str) -> bool {
    n.attribute(a)
        .is_some_and(|v| v.eq_ignore_ascii_case("true"))
}

fn string(n: Node, a: &str) -> String {
    n.attribute(a).unwrap_or_default().to_string()
}

/// `"x y z"` of three numbers.
fn vec3(n: Node, a: &str) -> [f32; 3] {
    let mut it = n
        .attribute(a)
        .unwrap_or_default()
        .split_whitespace()
        .map(|v| v.parse().unwrap_or(0.0));
    [
        it.next().unwrap_or(0.0),
        it.next().unwrap_or(0.0),
        it.next().unwrap_or(0.0),
    ]
}

// ---------------------------------------------------------------------------------------------
// system/npc.xml

/// `<AI_VALUE>`: seconds for the 1..=5 `int` / `agility` steps.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AiValue {
    pub intelligence: [f32; 5],
    pub agility: [f32; 5],
    /// `<SHAKING pathfinding_update attack_update>` seconds.
    pub path_update: f32,
    pub attack_update: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NpcDef {
    pub id: u32,
    /// `STR:NPC_NAME_11` -> strings.xml id `NPC_NAME_11`.
    pub name_key: String,
    pub mesh: String,
    pub scale: f32,
    pub grade: String,
    pub max_hp: f32,
    pub max_ap: f32,
    pub int: u32,
    pub agility: u32,
    /// Degrees.
    pub view_angle: f32,
    pub dc: u32,
    /// 1 melee-minded, 2 caster / gunner.
    pub offense: u32,
    pub dying_secs: f32,
    pub radius_cm: f32,
    pub height_cm: f32,
    pub never_pushed: bool,
    pub never_blasted: bool,
    /// Melee reach (cm) and the zitem the blow is struck with (0: none).
    pub range_cm: f32,
    pub weapon: u32,
    pub speed_cm: f32,
    /// Turn rate, rad/s.
    pub rotate: Option<f32>,
    pub skills: Vec<u32>,
    pub drop: String,
}

pub fn parse_npcs(xml: &str) -> io::Result<(AiValue, Vec<NpcDef>)> {
    let d = doc(xml, "npc.xml")?;
    let mut ai = AiValue::default();
    let mut npcs = Vec::new();
    for n in d.root_element().children().filter(|n| n.is_element()) {
        match n.tag_name().name() {
            "AI_VALUE" => {
                for c in n.children().filter(|c| c.is_element()) {
                    let table = match c.tag_name().name() {
                        "SHAKING" => {
                            ai.path_update = num(c, "pathfinding_update");
                            ai.attack_update = num(c, "attack_update");
                            continue;
                        }
                        "INTELLIGENCE" => &mut ai.intelligence,
                        "AGILITY" => &mut ai.agility,
                        _ => continue,
                    };
                    for t in c.children().filter(|t| t.has_tag_name("TIME")) {
                        let step = num(t, "step") as usize;
                        let secs = t.text().and_then(|v| v.trim().parse().ok());
                        match (table.get_mut(step.wrapping_sub(1)), secs) {
                            (Some(slot), Some(v)) => *slot = v,
                            _ => return Err(bad(format!("npc.xml: bad AI_VALUE step {step}"))),
                        }
                    }
                }
            }
            "NPC" => npcs.push(npc(n)?),
            _ => {}
        }
    }
    Ok((ai, npcs))
}

fn npc(n: Node) -> io::Result<NpcDef> {
    let child = |tag| n.children().find(|c| c.has_tag_name(tag));
    let id = n
        .attribute("id")
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| bad("npc.xml: <NPC> without id"))?;
    let col = child("COLLISION");
    let attack = child("ATTACK");
    let speed = child("SPEED");
    let flags = child("FLAG");
    let mesh = string(n, "meshname");
    if mesh.is_empty() {
        return Err(bad(format!("npc.xml: NPC {id} has no meshname")));
    }
    Ok(NpcDef {
        id,
        name_key: string(n, "name").trim_start_matches("STR:").to_string(),
        mesh,
        scale: vec3(n, "scale")[0].max(0.01),
        grade: string(n, "grade"),
        max_hp: num(n, "max_hp"),
        max_ap: num(n, "max_ap"),
        int: num(n, "int") as u32,
        agility: num(n, "agility") as u32,
        view_angle: num(n, "view_angle"),
        dc: num(n, "dc") as u32,
        offense: num(n, "offensetype") as u32,
        dying_secs: num(n, "dyingtime"),
        radius_cm: col.map_or(30.0, |c| num(c, "radius")),
        height_cm: col.map_or(150.0, |c| num(c, "height")),
        never_pushed: flags.is_some_and(|f| flag(f, "never_pushed")),
        never_blasted: flags.is_some_and(|f| flag(f, "never_blasted")),
        range_cm: attack.map_or(100.0, |a| num(a, "range")),
        weapon: attack.map_or(0, |a| num(a, "weaponitem_id") as u32),
        speed_cm: speed.map_or(300.0, |s| num(s, "default")),
        rotate: speed
            .and_then(|s| s.attribute("rotate"))
            .and_then(|v| v.parse().ok()),
        skills: n
            .children()
            .filter(|c| c.has_tag_name("SKILL"))
            .map(|c| num(c, "id") as u32)
            .collect(),
        drop: child("DROP").map_or(String::new(), |d| string(d, "table")),
    })
}

// ---------------------------------------------------------------------------------------------
// system/npc2.xml

#[derive(Clone, Debug, PartialEq)]
pub struct ActorDef {
    pub name: String,
    pub model: String,
    pub fsm: String,
    pub max_hp: f32,
    pub max_ap: f32,
    pub radius_cm: f32,
    pub height_cm: f32,
    pub speed_cm: f32,
    /// rad/s.
    pub rotspeed: f32,
    pub groggy_recover: f32,
    pub never_blasted: bool,
    pub boss: bool,
    /// `sound.die` stem (path as written).
    pub sound_die: String,
    /// `type` (`barricade`, `radar`, `knifeman`, `honor_item`, ...); empty for the quest actors.
    pub kind: String,
}

pub fn parse_actors(xml: &str) -> io::Result<Vec<ActorDef>> {
    let d = doc(xml, "npc2.xml")?;
    d.descendants()
        .filter(|n| n.has_tag_name("ACTOR"))
        .map(|n| {
            let name = string(n, "name");
            if name.is_empty() || n.attribute("model").is_none() {
                return Err(bad("npc2.xml: <ACTOR> without name/model"));
            }
            Ok(ActorDef {
                model: string(n, "model"),
                fsm: string(n, "ai.fsm"),
                max_hp: num(n, "max_hp"),
                max_ap: num(n, "max_ap"),
                radius_cm: num(n, "collision.radius"),
                height_cm: num(n, "collision.height"),
                speed_cm: num(n, "speed"),
                rotspeed: num(n, "rotspeed"),
                groggy_recover: num(n, "groggyRecoverPerSec"),
                never_blasted: flag(n, "neverblasted"),
                boss: flag(n, "boss"),
                sound_die: string(n, "sound.die"),
                kind: string(n, "type"),
                name,
            })
        })
        .collect()
}

// ---------------------------------------------------------------------------------------------
// system/zskill.xml

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Skill {
    pub id: u32,
    pub name: String,
    /// Reuse delay, ms.
    pub delay_ms: f32,
    /// `hitcheck`: a flying missile (else an area / self effect).
    pub missile: bool,
    pub guidable: bool,
    /// cm/s.
    pub velocity: f32,
    pub lifetime_ms: f32,
    /// Missile radius, cm.
    pub colradius: f32,
    pub knockback: f32,
    pub effect_type: u32,
    /// Cast start to effect, ms.
    pub start_ms: f32,
    pub effect_ms: f32,
    /// Metres: reach, inner reach, cone angle (degrees, > 360 = all around).
    pub area: f32,
    pub area_min: f32,
    pub angle: f32,
    pub damage: f32,
    pub dot: f32,
    /// Percent of normal speed the target is slowed to (100 = no slow) and rooted.
    pub speed_pct: f32,
    pub root: bool,
    pub heal: f32,
    /// `mod.criticalrate` as a probability (0..1).
    pub crit: f32,
    /// `camera.power`, `camera.duration` (s), `camera.range` (cm); all 0 when the skill shakes
    /// nothing.
    pub camera: [f32; 3],
    /// `castinganimation` N plays clip `special_attack<N>` (0: the melee clip).
    pub cast_anim: u32,
    pub trail: String,
    pub trail_scale: f32,
    pub start_pos: u32,
    pub cast_effect: String,
    pub cast_pre_effect: String,
    pub cast_offset_cm: [f32; 3],
    pub sound_explosion: String,
    pub resist: u32,
    /// Extra missiles: (delay s after the previous, yaw offset rad).
    pub repeats: Vec<(f32, f32)>,
}

pub fn parse_skills(xml: &str) -> io::Result<HashMap<u32, Skill>> {
    let d = doc(xml, "zskill.xml")?;
    let mut out = HashMap::new();
    for n in d.descendants().filter(|n| n.has_tag_name("SKILL")) {
        let id = num(n, "id") as u32;
        let s = Skill {
            id,
            name: string(n, "name"),
            delay_ms: num(n, "delay"),
            missile: flag(n, "hitcheck"),
            guidable: flag(n, "guidable"),
            velocity: num(n, "velocity"),
            lifetime_ms: num(n, "lifetime"),
            colradius: num(n, "colradius"),
            knockback: num(n, "knockback"),
            effect_type: num(n, "effecttype") as u32,
            start_ms: num(n, "effectstarttime"),
            effect_ms: num(n, "effecttime"),
            area: num(n, "effectarea"),
            area_min: num(n, "effectareamin"),
            angle: num(n, "effectangle"),
            damage: num(n, "mod.damage"),
            dot: num(n, "mod.dot"),
            speed_pct: n
                .attribute("mod.speed")
                .and_then(|v| v.parse().ok())
                .unwrap_or(100.0),
            root: flag(n, "mod.root"),
            heal: num(n, "mod.heal"),
            crit: num(n, "mod.criticalrate") * 0.01,
            camera: [
                num(n, "camera.power"),
                num(n, "camera.duration"),
                num(n, "camera.range"),
            ],
            cast_anim: num(n, "castinganimation") as u32,
            trail: string(n, "traileffect"),
            trail_scale: n
                .attribute("traileffectscale")
                .and_then(|v| v.parse().ok())
                .unwrap_or(1.0),
            start_pos: num(n, "effect_startpos_type") as u32,
            cast_effect: string(n, "castingeffect"),
            cast_pre_effect: string(n, "castingpreeffect"),
            cast_offset_cm: vec3(n, "castingeffectAddPos"),
            sound_explosion: string(n, "sound.explosion"),
            resist: num(n, "resisttype") as u32,
            repeats: n
                .children()
                .filter(|c| c.has_tag_name("REPEAT"))
                .map(|c| (num(c, "delay"), vec3(c, "angle")[2]))
                .collect(),
        };
        if out.insert(id, s).is_some() {
            return Err(bad(format!("zskill.xml: duplicate skill {id}")));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// system/zactoraction.xml

/// One timed happening inside an action (`delay` of the XML child, in seconds).
#[derive(Clone, Debug, PartialEq)]
pub enum Ev {
    Effect {
        mesh: String,
        part: String,
        pos_cm: [f32; 3],
        scale: f32,
    },
    Sound(String),
    Melee {
        damage: f32,
        range_cm: f32,
        angle: f32,
        uppercut: bool,
        thrust: bool,
        sound: String,
        /// `pierce` percent / 100: the share of the blow that reaches health (`None`: absent).
        pierce: Option<f32>,
    },
    Shot {
        damage: f32,
        sound: String,
        mesh: String,
        speed_cm: f32,
        radius_cm: f32,
        /// Added to the unit aim direction before normalising.
        dirmod: [f32; 3],
        part: String,
        thrust: bool,
        pierce: Option<f32>,
    },
    Grenade {
        damage: f32,
        /// Launch speed, cm/s; pitch above the horizon and yaw off the aim, degrees.
        force: f32,
        pitch: f32,
        yaw: f32,
        pos_cm: [f32; 3],
        sound: String,
        /// `grenadetype` 3 bursts on first contact, 0 rolls until its fuse is out.
        impact: bool,
        pierce: Option<f32>,
    },
    Summon {
        name: String,
        range_cm: f32,
        angle: f32,
        /// `route` of `blitzkrieg.xml` the summoned soldier marches along (0: none).
        route: u32,
        drop: String,
    },
    /// The three below are built by `npc.rs` for a `zskill.xml` skill of a classic monster:
    /// a missile (yaw offset in radians), an area / ground effect, a heal.
    Missile {
        skill: u32,
        yaw: f32,
    },
    Area(u32),
    Heal(u32),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Action {
    pub name: String,
    /// Key into the model's `AddAnimation` names.
    pub animation: String,
    /// `movinganimation`: the clip's root motion moves the actor.
    pub moving: bool,
    /// Sorted by time.
    pub events: Vec<(f32, Ev)>,
}

/// `pierce` attribute (percent) as a ratio.
fn pierce(n: Node) -> Option<f32> {
    n.attribute("pierce")?
        .trim()
        .parse::<f32>()
        .ok()
        .map(|p| p / 100.0)
}

pub fn parse_actions(xml: &str) -> io::Result<HashMap<String, Arc<Action>>> {
    let d = doc(xml, "zactoraction.xml")?;
    let mut out = HashMap::new();
    for a in d.descendants().filter(|n| n.has_tag_name("ACTION")) {
        let name = string(a, "name");
        let mut events = Vec::new();
        for e in a.children().filter(|c| c.is_element()) {
            let at = num(e, "delay") / 1000.0;
            let ev = match e.tag_name().name() {
                "EFFECT" => Ev::Effect {
                    mesh: string(e, "mesh"),
                    part: string(e, "posparts"),
                    pos_cm: vec3(e, "posmod"),
                    scale: e
                        .attribute("scale")
                        .and_then(|v| v.parse().ok())
                        .unwrap_or(1.0),
                },
                "SOUND" => Ev::Sound(string(e, "sound")),
                "MELEESHOT" => Ev::Melee {
                    damage: num(e, "damage"),
                    range_cm: num(e, "range"),
                    angle: num(e, "angle"),
                    uppercut: flag(e, "uppercut"),
                    thrust: flag(e, "thrust"),
                    sound: string(e, "sound"),
                    pierce: pierce(e),
                },
                "RANGESHOT" => Ev::Shot {
                    damage: num(e, "damage"),
                    sound: string(e, "sound"),
                    mesh: string(e, "mesh"),
                    speed_cm: num(e, "speed"),
                    radius_cm: num(e, "collradius"),
                    dirmod: vec3(e, "dirmod"),
                    part: string(e, "posparts"),
                    thrust: flag(e, "thrust"),
                    pierce: pierce(e),
                },
                "GRENADESHOT" => Ev::Grenade {
                    damage: num(e, "damage"),
                    force: num(e, "force"),
                    pitch: num(e, "yaxis"),
                    yaw: num(e, "zaxis"),
                    pos_cm: vec3(e, "posmod"),
                    sound: string(e, "sound"),
                    impact: num(e, "grenadetype") == 3.0,
                    pierce: pierce(e),
                },
                "SUMMON" => Ev::Summon {
                    name: string(e, "name"),
                    range_cm: num(e, "range"),
                    angle: num(e, "angle"),
                    route: num(e, "route") as u32,
                    drop: string(e, "drop"),
                },
                other => return Err(bad(format!("zactoraction.xml: {name}: unknown <{other}>"))),
            };
            events.push((at, ev));
        }
        events.sort_by(|x, y| x.0.total_cmp(&y.0));
        let act = Action {
            animation: string(a, "animation"),
            moving: flag(a, "movinganimation"),
            events,
            name: name.clone(),
        };
        if out.insert(name.clone(), Arc::new(act)).is_some() {
            return Err(bad(format!("zactoraction.xml: duplicate action {name}")));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// system/aifsm.xml

/// A transition condition (a `TRANS cond` is the AND of its comma-separated terms).
#[derive(Clone, Debug, PartialEq)]
pub enum Cond {
    /// `groggyGreater:N`
    Groggy(f32),
    /// `hpEqual:N`
    Hp(f32),
    /// `dice:N`: a share of N in the state's stored random number (see `fsm::next`).
    Dice(f32),
    /// `timeElapsedSinceEntered:ms`
    Elapsed(f32),
    EndAction,
    /// `distTarget:min;max` (cm)
    Dist(f32, f32),
    CanSee,
    CannotSee,
    HasTarget,
    NoTarget,
    Default,
    FailedPath,
    /// `isEmptySpace:angle;dist`: free floor that far (cm) in the direction `angle` degrees
    /// clockwise from the facing.
    EmptySpace(f32, f32),
    /// `angleTargetHeight:min;max`: elevation of the target above the horizon, degrees.
    Elevation(f32, f32),
    /// `lookAtTarget:deg`: the target is within this angle of the facing.
    LookAt(f32),
    /// `SummonLess:N`: fewer than N living summons.
    SummonLess(u32),
    /// `TargetHeightHigher:cm`
    Higher(f32),
}

/// A state function / entry function.
#[derive(Clone, Debug, PartialEq)]
pub enum Func {
    FindTarget,
    /// `findTargetInHeight:cm`: nearest target within that height difference.
    FindInHeight(f32),
    /// `findTargetInDist:cm`
    FindInDist(f32),
    Dice,
    RotateToTarget,
    FaceToTarget,
    FaceToAttacker,
    BuildWaypoints,
    ClearWaypoints,
    RunWaypoints,
    /// `runWaypointsAlongRoute`: march along the route of the `SpawnNpc` (Blitzkrieg's lanes),
    /// whether or not a target is known.
    RunRoute,
    /// `runAlongTargetOrbital:cm/s`
    Orbit(f32),
    TurnOrbit,
    /// `speedAccel:cm/s^2`
    Accel(f32),
    ReduceGroggy(f32),
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Next {
    State(usize),
    /// The built-in `__die`.
    Die,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Trans {
    pub conds: Vec<Cond>,
    pub next: Next,
}

#[derive(Clone, Debug, PartialEq)]
pub struct State {
    pub name: String,
    pub cooltime_ms: f32,
    pub action: Option<String>,
    pub funcs: Vec<Func>,
    pub enter: Vec<Func>,
    pub exit: Vec<Func>,
    pub trans: Vec<Trans>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Fsm {
    pub name: String,
    pub entry: usize,
    pub states: Vec<State>,
}

fn range(arg: &str, term: &str) -> io::Result<(f32, f32)> {
    let (a, b) = arg
        .split_once(';')
        .ok_or_else(|| bad(format!("aifsm.xml: {term}: want min;max")))?;
    Ok((
        a.trim().parse().map_err(|_| bad(term.to_string()))?,
        b.trim().parse().map_err(|_| bad(term.to_string()))?,
    ))
}

fn parse_cond(term: &str) -> io::Result<Cond> {
    let (name, arg) = term.split_once(':').unwrap_or((term, ""));
    let one = || {
        arg.trim()
            .parse::<f32>()
            .map_err(|_| bad(format!("aifsm.xml: {term}: want a number")))
    };
    Ok(match name {
        "groggyGreater" => Cond::Groggy(one()?),
        "hpEqual" => Cond::Hp(one()?),
        "dice" => Cond::Dice(one()?),
        "timeElapsedSinceEntered" => Cond::Elapsed(one()?),
        "endAction" => Cond::EndAction,
        "distTarget" => {
            let (a, b) = range(arg, term)?;
            Cond::Dist(a, b)
        }
        "canSeeTarget" => Cond::CanSee,
        "cannotSeeTarget" => Cond::CannotSee,
        "hasTarget" => Cond::HasTarget,
        "hasNoTarget" => Cond::NoTarget,
        "default" => Cond::Default,
        "FailedBuildWayPoints" => Cond::FailedPath,
        "isEmptySpace" => {
            let (a, b) = range(arg, term)?;
            Cond::EmptySpace(a, b)
        }
        "angleTargetHeight" => {
            let (a, b) = range(arg, term)?;
            Cond::Elevation(a, b)
        }
        "lookAtTarget" => Cond::LookAt(one()?),
        "SummonLess" => Cond::SummonLess(one()? as u32),
        "TargetHeightHigher" => Cond::Higher(one()?),
        _ => return Err(bad(format!("aifsm.xml: unknown condition {term:?}"))),
    })
}

fn parse_funcs(list: &str) -> io::Result<Vec<Func>> {
    list.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .map(|term| {
            let (name, arg) = term.split_once(':').unwrap_or((term, ""));
            let one = || {
                arg.trim()
                    .parse::<f32>()
                    .map_err(|_| bad(format!("aifsm.xml: {term}: want a number")))
            };
            Ok(match name {
                "findTarget" => Func::FindTarget,
                "findTargetInHeight" => Func::FindInHeight(one()?),
                "findTargetInDist" => Func::FindInDist(one()?),
                "dice" => Func::Dice,
                "rotateToTarget" => Func::RotateToTarget,
                "faceToTarget" => Func::FaceToTarget,
                "faceToLastestAttacker" => Func::FaceToAttacker,
                "buildWaypointsToTarget" => Func::BuildWaypoints,
                "clearWaypoints" => Func::ClearWaypoints,
                "runWaypoints" => Func::RunWaypoints,
                "runWaypointsAlongRoute" => Func::RunRoute,
                "runAlongTargetOrbital" => Func::Orbit(one()?),
                "turnOrbitalDirection" => Func::TurnOrbit,
                "speedAccel" => Func::Accel(one()?),
                "reduceGroggy" => Func::ReduceGroggy(one()?),
                _ => return Err(bad(format!("aifsm.xml: unknown function {term:?}"))),
            })
        })
        .collect()
}

pub fn parse_fsms(xml: &str) -> io::Result<HashMap<String, Arc<Fsm>>> {
    let d = doc(xml, "aifsm.xml")?;
    let mut out = HashMap::new();
    for f in d.descendants().filter(|n| n.has_tag_name("FSM")) {
        let name = string(f, "name");
        let states: Vec<Node> = f.children().filter(|c| c.has_tag_name("STATE")).collect();
        let index: HashMap<&str, usize> = states
            .iter()
            .enumerate()
            .filter_map(|(i, s)| Some((s.attribute("name")?, i)))
            .collect();
        let entry = *index
            .get(f.attribute("entrystate").unwrap_or_default())
            .ok_or_else(|| bad(format!("aifsm.xml: FSM {name}: bad entrystate")))?;
        let mut built = Vec::new();
        for s in &states {
            let sname = string(*s, "name");
            let trans = s
                .children()
                .filter(|t| t.has_tag_name("TRANS"))
                .map(|t| {
                    let next = match t.attribute("next").unwrap_or_default() {
                        "__die" => Next::Die,
                        n => Next::State(*index.get(n).ok_or_else(|| {
                            bad(format!("aifsm.xml: {name}/{sname}: no state {n:?}"))
                        })?),
                    };
                    let conds = t
                        .attribute("cond")
                        .unwrap_or("default")
                        .split(',')
                        .map(|c| parse_cond(c.trim()))
                        .collect::<io::Result<_>>()?;
                    Ok(Trans { conds, next })
                })
                .collect::<io::Result<_>>()?;
            built.push(State {
                cooltime_ms: num(*s, "cooltime"),
                action: s.attribute("action").map(str::to_string),
                funcs: parse_funcs(s.attribute("func").unwrap_or_default())?,
                enter: parse_funcs(s.attribute("enterfunc").unwrap_or_default())?,
                exit: parse_funcs(s.attribute("exitfunc").unwrap_or_default())?,
                trans,
                name: sname,
            });
        }
        let fsm = Fsm {
            name: name.clone(),
            entry,
            states: built,
        };
        if out.insert(name.clone(), Arc::new(fsm)).is_some() {
            return Err(bad(format!("aifsm.xml: duplicate FSM {name}")));
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------------------------
// model/npc.xml, model/npc2.xml and the per-monster model XMLs

/// A monster model: skeleton+mesh ELU and its named clips (`<AddAnimation>`).
#[derive(Clone, Debug)]
pub struct ModelDef {
    pub elu: String,
    /// VFS directory (ending in `/`) the textures resolve against.
    pub dir: String,
    /// `(name, VFS path, motion_loop_type)`
    pub clips: Vec<(String, String, String)>,
}

/// `(name, xml path)` of every `AddXml` of a `model/npc*.xml` registry.
pub fn parse_registry(xml: &str) -> io::Result<Vec<(String, String)>> {
    let d = doc(xml, "model/npc.xml")?;
    Ok(d.descendants()
        .filter(|n| n.has_tag_name("AddXml"))
        .filter_map(|n| {
            Some((
                n.attribute("name")?.to_string(),
                crate::mrs::normalize(n.attribute("filename")?),
            ))
        })
        .collect())
}

/// Parses one model XML. Clips whose file is missing are left out (`goblinG` names one that the
/// retail data does not contain); `exists` says whether a VFS path exists.
pub fn parse_model(xml: &str, path: &str, exists: impl Fn(&str) -> bool) -> io::Result<ModelDef> {
    let d = doc(xml, path)?;
    let dir = path
        .rsplit_once('/')
        .map_or(String::new(), |(d, _)| format!("{d}/"));
    let join = |f: &str| format!("{dir}{}", crate::mrs::normalize(f));
    let base = d
        .descendants()
        .find(|n| n.has_tag_name("AddBaseModel"))
        .and_then(|n| n.attribute("filename"))
        .ok_or_else(|| bad(format!("{path}: no AddBaseModel")))?;
    let elu = join(base);
    if !exists(&elu) {
        return Err(bad(format!("{path}: {elu} not in archives")));
    }
    let clips = d
        .descendants()
        .filter(|n| n.has_tag_name("AddAnimation"))
        .filter_map(|n| {
            let file = join(n.attribute("filename")?);
            exists(&file).then(|| (string(n, "name"), file, string(n, "motion_loop_type")))
        })
        .collect();
    Ok(ModelDef { elu, dir, clips })
}

/// Everything the quest monsters are made of.
pub struct Data {
    pub ai: AiValue,
    pub npcs: HashMap<u32, NpcDef>,
    pub actors: HashMap<String, ActorDef>,
    pub skills: HashMap<u32, Skill>,
    pub actions: HashMap<String, Arc<Action>>,
    pub fsms: HashMap<String, Arc<Fsm>>,
    /// Model name -> XML path.
    pub models: HashMap<String, String>,
    /// strings.xml `id` -> text.
    pub strings: HashMap<String, String>,
}

/// English names for the monsters whose `strings.xml` entry is Korean in every locale dir
/// (`system/*/strings.xml`; `interface/monsterillust` holds only pictures). **Inferred**:
/// translated from the Korean string (리쟈드 = lizard, 팜포우/팜포아 = "Pampow", Korean spelling).
const ENGLISH_NAMES: &[(&str, &str)] = &[
    ("NPC_NAME_21", "Lizard"),
    ("NPC_NAME_22", "Lizard Shaman"),
    ("NPC_NAME_23", "Lizard Captain"),
    ("NPC_NAME_24", "Lizard King"),
    ("NPC_NAME_25", "Lizard King (Boss)"),
    ("NPC_NAME_26", "Broken Golem"),
    ("NPC_NAME_31", "Skeleton"),
    ("NPC_NAME_32", "Skeleton Mage"),
    ("NPC_NAME_33", "Skeleton Captain"),
    ("NPC_NAME_34", "Giant Skeleton"),
    ("NPC_NAME_35", "Cursed Corpse"),
    ("NPC_NAME_36", "Lich Pawn"),
    ("NPC_NAME_37", "Superion (Boss)"),
    ("NPC_NAME_38", "Anelamon (Boss)"),
    ("NPC_NAME_39", "Lich (Boss)"),
    ("NPC_NAME_41", "Pampow Baby"),
    ("NPC_NAME_42", "Pampoa"),
    ("NPC_NAME_44", "Pampow"),
    ("NPC_NAME_45", "Cursed Pampow"),
    ("NPC_NAME_46", "Captain Pampoa (Boss)"),
    ("NPC_NAME_47", "Pampow (Boss)"),
    ("NPC_NAME_48", "Pampow Baby"),
];

impl Data {
    pub fn load(vfs: &Vfs) -> io::Result<Self> {
        let (ai, npcs) = parse_npcs(&text(vfs, "system/npc.xml")?)?;
        let mut models = HashMap::new();
        for reg in ["model/npc.xml", "model/npc2.xml"] {
            models.extend(parse_registry(&text(vfs, reg)?)?);
        }
        let strings = text(vfs, "system/strings.xml")?;
        let mut strings: HashMap<String, String> = doc(&strings, "strings.xml")?
            .descendants()
            .filter(|n| n.has_tag_name("STR"))
            .filter_map(|n| Some((n.attribute("id")?.to_owned(), n.text()?.to_owned())))
            .collect();
        // No locale ships these names in Latin script, so overlay the English table.
        for (key, en) in ENGLISH_NAMES {
            if strings.get(*key).is_none_or(|s| !s.is_ascii()) {
                strings.insert((*key).to_owned(), (*en).to_owned());
            }
        }
        Ok(Self {
            ai,
            npcs: npcs.into_iter().map(|n| (n.id, n)).collect(),
            actors: parse_actors(&text(vfs, "system/npc2.xml")?)?
                .into_iter()
                .map(|a| (a.name.clone(), a))
                .collect(),
            skills: parse_skills(&text(vfs, "system/zskill.xml")?)?,
            actions: parse_actions(&text(vfs, "system/zactoraction.xml")?)?,
            fsms: parse_fsms(&text(vfs, "system/aifsm.xml")?)?,
            models,
            strings,
        })
    }

    /// Reads and parses the model XML registered as `name`.
    pub fn model(&self, vfs: &Vfs, name: &str) -> io::Result<ModelDef> {
        let path = self
            .models
            .get(name)
            .ok_or_else(|| bad(format!("model {name:?} not in model/npc.xml or npc2.xml")))?;
        parse_model(&text(vfs, path)?, path, |p| vfs.exists(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NPC: &str = r#"<XML id="npccatalogue">
      <AI_VALUE>
        <SHAKING pathfinding_update="0.1" attack_update="0.1" speed="0.2" />
        <INTELLIGENCE><TIME step="1">0.4</TIME><TIME step="2">0.6</TIME><TIME step="3">1</TIME><TIME step="4">2</TIME><TIME step="5">3</TIME></INTELLIGENCE>
        <AGILITY><TIME step="1">0.2</TIME><TIME step="2">0.5</TIME><TIME step="3">1</TIME><TIME step="4">2</TIME><TIME step="5">3</TIME></AGILITY>
      </AI_VALUE>
      <NPC id="16" name="STR:NPC_NAME_16" meshname="goblinK" scale="2.5 2.5 2.5" grade="boss" max_hp="9100" max_ap="8960" int="3" agility="5" view_angle="20" dc="50" offensetype="1" dyingtime="8">
        <COLLISION radius="50" height="200" tremble="5" />
        <FLAG never_blasted="true" never_pushed="true" />
        <ATTACK type="melee" range="450" weaponitem_id="300016" />
        <SPEED default="400" rotate="4.712" />
        <SKILL id="161" /><SKILL id="162" />
        <DROP table="G16" />
      </NPC>
    </XML>"#;

    #[test]
    fn npc_xml() {
        let (ai, npcs) = parse_npcs(NPC).unwrap();
        assert_eq!(ai.intelligence, [0.4, 0.6, 1.0, 2.0, 3.0]);
        assert_eq!(ai.agility[0], 0.2);
        assert_eq!(ai.attack_update, 0.1);
        let k = &npcs[0];
        assert_eq!(
            (k.id, k.mesh.as_str(), k.scale, k.max_hp),
            (16, "goblinK", 2.5, 9100.0)
        );
        assert_eq!(
            (k.radius_cm, k.height_cm, k.range_cm, k.weapon),
            (50.0, 200.0, 450.0, 300016)
        );
        assert!(k.never_blasted && k.never_pushed);
        assert_eq!(
            (k.skills.as_slice(), k.rotate, k.drop.as_str()),
            (&[161, 162][..], Some(4.712), "G16")
        );
        assert_eq!(k.name_key, "NPC_NAME_16");
    }

    #[test]
    fn skill_and_action_and_fsm() {
        let skills = parse_skills(
            r#"<XML><SKILL id="152" name="Fire Missile" hitcheck="true" velocity="4000" delay="4000"
                colradius="35" mod.damage="50" castinganimation="2" traileffect="fireball" traileffectscale="2.7">
                <REPEAT delay="0.0" angle="0.0 0.0 0.1" /><REPEAT delay="0.5" angle="0.0 0.0 -0.1" /></SKILL></XML>"#,
        )
        .unwrap();
        let s = &skills[&152];
        assert!(s.missile && s.damage == 50.0 && s.cast_anim == 2 && s.speed_pct == 100.0);
        assert_eq!(s.repeats, vec![(0.0, 0.1), (0.5, -0.1)]);

        let actions = parse_actions(
            r#"<XML><ACTION name="slash" animation="slash" movinganimation="true">
                <SOUND delay="500" sound="x/y" />
                <MELEESHOT delay="433" damage="12" range="160" angle="140" pierce="0" sound="s" /></ACTION></XML>"#,
        )
        .unwrap();
        let a = &actions["slash"];
        assert!(a.moving);
        assert_eq!(a.events[0].0, 0.433);
        assert!(
            matches!(a.events[0].1, Ev::Melee { damage, range_cm, .. } if damage == 12.0 && range_cm == 160.0)
        );
        // `pierce="0"` is a ratio of 0, not "absent".
        assert!(matches!(a.events[0].1, Ev::Melee { pierce: Some(p), .. } if p == 0.0));

        let fsms = parse_fsms(
            r#"<XML><FSM name="k" entrystate="find">
                <STATE name="find" cooltime="0" action="idle" func="findTarget,speedAccel:1200">
                  <TRANS cond="hpEqual:0" next="__die"/>
                  <TRANS cond="canSeeTarget,distTarget:0;3000,hasTarget" next="slash"/>
                </STATE>
                <STATE name="slash" cooltime="3000" action="slash" enterfunc="faceToTarget,reduceGroggy:20">
                  <TRANS cond="endAction" next="find"/></STATE></FSM></XML>"#,
        )
        .unwrap();
        let f = &fsms["k"];
        assert_eq!(
            f.states[0].funcs,
            vec![Func::FindTarget, Func::Accel(1200.0)]
        );
        assert_eq!(f.states[0].trans[0].next, Next::Die);
        assert_eq!(
            f.states[0].trans[1].conds,
            vec![Cond::CanSee, Cond::Dist(0.0, 3000.0), Cond::HasTarget]
        );
        assert_eq!(f.states[0].trans[1].next, Next::State(1));
        assert_eq!(
            f.states[1].enter,
            vec![Func::FaceToTarget, Func::ReduceGroggy(20.0)]
        );
        assert!(parse_fsms(r#"<XML><FSM name="k" entrystate="a"><STATE name="a" cooltime="0"><TRANS cond="nope" next="a"/></STATE></FSM></XML>"#).is_err());
    }

    /// The retail files, when the extract is present (`.local/extract`): every one parses and
    /// every cross-reference resolves.
    #[test]
    fn retail_cross_references() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".local/extract");
        let read = |p: &str| {
            std::fs::read_to_string(dir.join(p))
                .map(|t| t.trim_start_matches('\u{feff}').to_string())
        };
        let Ok(npc) = read("system/npc.xml") else {
            return;
        };
        let (_, npcs) = parse_npcs(&npc).unwrap();
        let actors = parse_actors(&read("system/npc2.xml").unwrap()).unwrap();
        let skills = parse_skills(&read("system/zskill.xml").unwrap()).unwrap();
        let actions = parse_actions(&read("system/zactoraction.xml").unwrap()).unwrap();
        let fsms = parse_fsms(&read("system/aifsm.xml").unwrap()).unwrap();
        assert_eq!((npcs.len(), actors.len(), fsms.len()), (76, 48, 38));
        for n in &npcs {
            for s in &n.skills {
                assert!(skills.contains_key(s), "NPC {} skill {s}", n.id);
            }
        }
        for a in &actors {
            let f = fsms
                .get(&a.fsm)
                .unwrap_or_else(|| panic!("{}: no FSM {}", a.name, a.fsm));
            for s in &f.states {
                if let Some(act) = &s.action {
                    assert!(
                        actions.contains_key(act),
                        "{}/{}: no action {act}",
                        f.name,
                        s.name
                    );
                }
            }
        }
    }
}
