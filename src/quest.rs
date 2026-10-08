//! Offline quest play (`gunz-play --mode quest --scenario NAME [--dice N]`): the retail quest
//! data (`system/scenario.xml`, `scenario2.xml`, `questmap.xml`, `survivalmap.xml`,
//! `droptable.xml`, `zquestitem.xml`, `npc.xml`) is parsed into a [`Plan`] (a chain of sectors,
//! each a map with NPC groups), and [`QuestPlugin`] plays it: NPCs are spawned in waves at the
//! map's `spawn_npc_*` dummies (through `game::SpawnNpc`, `npc.rs` makes them), a cleared sector
//! opens its `linkNN` portal, and walking into it (or a timer) swaps the map in-process for the
//! next sector. Formats and *inferred* constants: `docs/formats.md`, "Quest".

use crate::{
    actor::ActorData,
    bot::BotCount,
    col::MapCollision,
    combat::yaw_of,
    game::{
        Bot, Dead, Intent, Killed, MapEntity, Motor, Npc, Player, QuestLoot, Reward, Score,
        SpawnNpc, Team,
    },
    level::{self, Level},
    map, modes,
    pickup::{ItemKind, WorldItem},
    props,
    session::{Clock, HOLD, Rules, freeze},
    view::{SCALE, to_bevy},
};
use bevy::{prelude::*, ui::UiTargetCamera};
use std::collections::{HashMap, HashSet, VecDeque};

/// NPCs of a standard sector at quest level 0, plus [`PER_QL`] for every further level
/// (*inferred*: the data only gives the NPC sets).
const BASE_NPCS: u32 = 8;
const PER_QL: u32 = 2;
/// Most NPCs alive at once; the rest of a sector's NPCs follow as these die (*inferred*).
const MAX_LIVE: usize = 8;
/// Seconds between two spawns of the wave queue (*inferred*).
const SPAWN_GAP: f32 = 0.4;
/// Seconds of "SECTOR n" before the first NPC spawns (*inferred*).
const INTRO_SECS: f32 = 3.0;
/// Seconds a cleared sector's portal stays open before it pulls everyone through (*inferred*).
const OPEN_SECS: f32 = 30.0;
/// Horizontal reach of a portal, metres (*inferred*).
const PORTAL_R: f32 = 1.2;
/// Seconds the player lies dead before the quest is lost (*inferred*).
const FAIL_SECS: f32 = 2.5;
/// Reach of a drop, metres (*inferred*).
const PICKUP_R: f32 = 0.9;
/// Hit points of an NPC grow by this much per quest level above 1 (*inferred*).
const HP_PER_QL: f32 = 0.25;
/// A challenge-quest or survival sector pays its XP/BP (survival: the standard quest's XP/BP of
/// that level over this many, *inferred*).
const SURVIVAL_SHARE: u32 = 4;
/// Survival plays this many sectors (the data gives only the loop, *inferred*).
const SURVIVAL_SECTORS: usize = 10;

// ---------------------------------------------------------------- data

/// `<MAPSET>` of `questmap.xml` / `survivalmap.xml`.
#[derive(Clone, Debug, PartialEq)]
pub struct MapSet {
    pub title: String,
    pub sectors: Vec<Sector>,
}

/// `<SECTOR id title>`; the map is the directory `title` (lower case) under `quest/maps/`.
#[derive(Clone, Debug, PartialEq)]
pub struct Sector {
    pub id: u32,
    pub title: String,
    pub links: Vec<Link>,
}

/// `<LINK name>`: the portal dummy `name` of the map leads to one of `targets` (sector titles).
#[derive(Clone, Debug, PartialEq)]
pub struct Link {
    pub name: String,
    pub targets: Vec<String>,
}

/// `<JACO>`: the boss keeps calling reinforcements.
#[derive(Clone, Debug, PartialEq)]
pub struct Jaco {
    pub count: u32,
    pub tick: f32,
    pub max_npc: u32,
    pub npcs: Vec<(String, f32)>,
}

/// One `<MAP dice>` of a `scenario.xml` scenario.
#[derive(Clone, Debug, PartialEq)]
pub struct MapDef {
    pub dice: u32,
    pub key_sector: u32,
    pub key_npc: Option<String>,
    pub sets: Vec<String>,
    pub jaco: Option<Jaco>,
}

/// `<STANDARD_SCENARIO>` / `<SPECIAL_SCENARIO>`.
#[derive(Clone, Debug, PartialEq)]
pub struct ScenarioDef {
    pub title: String,
    pub id: Option<u32>,
    pub ql: u32,
    pub mapset: String,
    pub xp: u32,
    pub bp: u32,
    pub maps: Vec<MapDef>,
}

/// `<SPAWN>` of a challenge-quest sector: `num` NPCs `actor` at the `spawn_npc_<postag>` dummies.
#[derive(Clone, Debug, PartialEq)]
pub struct Spawn {
    pub postag: String,
    pub num: u32,
    pub actor: String,
    pub drop: String,
    pub adjust: bool,
}

/// `<SCENARIO>` of `scenario2.xml` (the challenge quest).
#[derive(Clone, Debug, PartialEq)]
pub struct Challenge {
    pub id: u32,
    pub reward_item: u32,
    pub players: u32,
    pub level: u32,
    pub sectors: Vec<(String, u32, u32, Vec<Spawn>)>,
}

fn doc(text: &str) -> Result<roxmltree::Document<'_>, String> {
    roxmltree::Document::parse(text.trim_start_matches('\u{feff}')).map_err(|e| e.to_string())
}

fn num(n: roxmltree::Node, attr: &str) -> u32 {
    n.attribute(attr)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

fn text(n: roxmltree::Node, attr: &str) -> String {
    n.attribute(attr).unwrap_or("").trim().to_owned()
}

pub fn parse_mapsets(xml: &str) -> Result<Vec<MapSet>, String> {
    let d = doc(xml)?;
    Ok(d.descendants()
        .filter(|n| n.has_tag_name("MAPSET"))
        .map(|m| MapSet {
            title: text(m, "title"),
            sectors: m
                .children()
                .filter(|n| n.has_tag_name("SECTOR"))
                .map(|s| Sector {
                    id: num(s, "id"),
                    title: text(s, "title"),
                    links: s
                        .children()
                        .filter(|n| n.has_tag_name("LINK"))
                        .map(|l| Link {
                            name: text(l, "name"),
                            targets: l
                                .children()
                                .filter(|t| t.has_tag_name("TARGET"))
                                .map(|t| text(t, "sector"))
                                .collect(),
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect())
}

pub fn parse_scenarios(xml: &str) -> Result<Vec<ScenarioDef>, String> {
    let d = doc(xml)?;
    Ok(d.root_element()
        .children()
        .filter(|n| n.has_tag_name("STANDARD_SCENARIO") || n.has_tag_name("SPECIAL_SCENARIO"))
        .map(|s| ScenarioDef {
            title: text(s, "title"),
            id: s.attribute("id").and_then(|v| v.parse().ok()),
            ql: num(s, "QL"),
            mapset: text(s, "mapset"),
            xp: num(s, "XP"),
            bp: num(s, "BP"),
            maps: s
                .children()
                .filter(|n| n.has_tag_name("MAP"))
                .map(|m| MapDef {
                    dice: num(m, "dice"),
                    key_sector: num(m, "key_sector"),
                    key_npc: m.attribute("key_npc").map(str::to_owned),
                    sets: m
                        .children()
                        .find(|n| n.has_tag_name("NPCSET_ARRAY"))
                        .and_then(|n| n.text())
                        .map(|t| t.trim().split('/').map(str::to_owned).collect())
                        .unwrap_or_default(),
                    jaco: m.children().find(|n| n.has_tag_name("JACO")).map(|j| Jaco {
                        count: num(j, "count"),
                        tick: num(j, "tick") as f32,
                        max_npc: num(j, "max_npc"),
                        npcs: j
                            .children()
                            .filter(|n| n.has_tag_name("NPC"))
                            .map(|n| {
                                let rate = n.attribute("rate").and_then(|r| r.parse().ok());
                                (text(n, "npcid"), rate.unwrap_or(0.0))
                            })
                            .collect(),
                    }),
                })
                .collect(),
        })
        .collect())
}

pub fn parse_challenges(xml: &str) -> Result<Vec<Challenge>, String> {
    let d = doc(xml)?;
    Ok(d.root_element()
        .children()
        .filter(|n| n.has_tag_name("SCENARIO"))
        .map(|s| Challenge {
            id: num(s, "map_id"),
            reward_item: num(s, "reward_item"),
            players: num(s, "players"),
            level: num(s, "level_limit"),
            sectors: s
                .children()
                .filter(|n| n.has_tag_name("SECTOR"))
                .map(|c| {
                    let spawns = c
                        .children()
                        .filter(|n| n.has_tag_name("SPAWN"))
                        .map(|p| Spawn {
                            postag: text(p, "postag"),
                            num: num(p, "num"),
                            actor: text(p, "actor"),
                            drop: text(p, "drop"),
                            adjust: p.attribute("adjustplayernum") == Some("true"),
                        })
                        .collect();
                    (text(c, "map"), num(c, "xp"), num(c, "bp"), spawns)
                })
                .collect(),
        })
        .collect())
}

/// `droptable.xml`: set name -> quest level -> (item id, rate). Sets listed twice under one
/// name (`G181`) are merged.
#[derive(Default, Debug)]
pub struct Drops(HashMap<String, HashMap<u32, Vec<(String, f32)>>>);

pub fn parse_drops(xml: &str) -> Result<Drops, String> {
    let d = doc(xml)?;
    let mut out = Drops::default();
    for set in d.descendants().filter(|n| n.has_tag_name("DROPSET")) {
        for lv in set.children().filter(|n| n.has_tag_name("ITEMSET")) {
            let items = out
                .0
                .entry(text(set, "name"))
                .or_default()
                .entry(num(lv, "QL"))
                .or_default();
            for i in lv.children().filter(|n| n.has_tag_name("ITEM")) {
                let rate = i
                    .attribute("rate")
                    .and_then(|r| r.parse().ok())
                    .unwrap_or(0.0);
                items.push((text(i, "id"), rate));
            }
        }
    }
    Ok(out)
}

impl Drops {
    /// The item `table` drops at quest level `ql` for a uniform roll `r` in 0..1: the item rates
    /// of a set add up to at most 1 (observed, all 147 sets), so one roll walks the cumulative
    /// rates and falls through to "nothing". The level's own set, else the nearest lower one.
    pub fn roll(&self, table: &str, ql: u32, r: f32) -> Option<&str> {
        let levels = self.0.get(table)?;
        let items = (0..=ql)
            .rev()
            .find_map(|q| levels.get(&q))
            .or_else(|| levels.values().next())?;
        let mut acc = 0.0;
        items
            .iter()
            .find(|(_, rate)| {
                acc += rate;
                r < acc
            })
            .map(|(id, _)| id.as_str())
    }
}

/// The NPC ids of `npc.xml` and which of them shoot (`offensetype="2"`; *inferred*: every one
/// with that value is a gunner, wizard or archer by name).
#[derive(Default, Debug)]
pub struct Npcs {
    pub known: HashSet<String>,
    pub ranged: HashSet<String>,
    /// `grade="boss"`: only a scenario's `key_npc` spawns them (the sets name 5 members where
    /// the family has 4 non-boss NPCs, *inferred*).
    pub bosses: HashSet<String>,
}

pub fn parse_npcs(xml: &str) -> Result<Npcs, String> {
    let d = doc(xml)?;
    let mut out = Npcs::default();
    for n in d
        .descendants()
        .filter(|n| n.has_tag_name("NPC") && n.has_attribute("id"))
    {
        let id = text(n, "id");
        if n.attribute("offensetype") == Some("2") {
            out.ranged.insert(id.clone());
        }
        if n.attribute("grade") == Some("boss") {
            out.bosses.insert(id.clone());
        }
        out.known.insert(id);
    }
    Ok(out)
}

/// Every parsed quest file.
pub struct Catalog {
    mapsets: Vec<MapSet>,
    survival: Vec<MapSet>,
    scenarios: Vec<ScenarioDef>,
    challenges: Vec<Challenge>,
    npcs: Npcs,
}

impl Catalog {
    pub fn load(vfs: &crate::mrs::Vfs) -> Result<Self, String> {
        let read = |p: &str| {
            vfs.read(p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .map_err(|e| format!("{p}: {e}"))
        };
        Self::parse(
            &read("system/questmap.xml")?,
            &read("system/survivalmap.xml")?,
            &read("system/scenario.xml")?,
            &read("system/scenario2.xml")?,
            &read("system/npc.xml")?,
        )
    }

    pub fn parse(
        questmap: &str,
        survival: &str,
        scenario: &str,
        scenario2: &str,
        npc: &str,
    ) -> Result<Self, String> {
        Ok(Self {
            mapsets: parse_mapsets(questmap)?,
            survival: parse_mapsets(survival)?,
            scenarios: parse_scenarios(scenario)?,
            challenges: parse_challenges(scenario2)?,
            npcs: parse_npcs(npc)?,
        })
    }

    /// Every playable scenario name, standard quests first. A name picks a scenario case-
    /// insensitively; a bare number picks a special scenario id or challenge `map_id`.
    pub fn names(&self) -> Vec<String> {
        let std = self.scenarios.iter().map(|s| s.title.clone());
        let ch = self
            .challenges
            .iter()
            .map(|c| format!("Challenge {}", c.id));
        let surv = self
            .survival
            .iter()
            .filter(|m| self.survival_ok(m))
            .map(|m| format!("Survival {}", m.title));
        std.chain(ch).chain(surv).collect()
    }

    /// Survival needs the NPC sets of the standard quests of its map set.
    fn survival_ok(&self, m: &MapSet) -> bool {
        self.scenarios
            .iter()
            .any(|s| s.mapset == m.title && s.id.is_none())
    }

    /// The plan of scenario `name`. `dice` picks a `<MAP dice>` of a `scenario.xml` scenario
    /// (default: the last one, the longest route); `seed` drives the random NPC picks.
    pub fn plan(&self, name: &str, dice: Option<u32>, seed: u32) -> Result<Plan, String> {
        let want = name.trim().to_ascii_lowercase();
        let by_name = |t: &str| t.to_ascii_lowercase() == want;
        if let Some(def) = self
            .scenarios
            .iter()
            .find(|s| by_name(&s.title) || s.id.map(|i| i.to_string()) == Some(want.clone()))
        {
            return self.plan_standard(def, dice, seed);
        }
        if let Some(c) = self
            .challenges
            .iter()
            .find(|c| by_name(&format!("Challenge {}", c.id)) || c.id.to_string() == want)
        {
            return Ok(Self::plan_challenge(c));
        }
        if let Some(m) = self
            .survival
            .iter()
            .find(|m| by_name(&format!("Survival {}", m.title)))
        {
            return self.plan_survival(m, seed);
        }
        Err(format!(
            "unknown scenario {name:?}; known: {}",
            self.names().join(", ")
        ))
    }

    fn plan_standard(
        &self,
        def: &ScenarioDef,
        dice: Option<u32>,
        seed: u32,
    ) -> Result<Plan, String> {
        let map = match dice {
            Some(d) => def
                .maps
                .iter()
                .find(|m| m.dice == d)
                .ok_or(format!("{}: no dice {d}", def.title))?,
            None => def.maps.last().ok_or(format!("{}: no maps", def.title))?,
        };
        let set = self
            .mapsets
            .iter()
            .find(|m| m.title == def.mapset)
            .ok_or(format!("no mapset {}", def.mapset))?;
        let key = set
            .sectors
            .iter()
            .position(|s| s.id == map.key_sector)
            .ok_or(format!("{}: key sector {}", def.title, map.key_sector))?;
        let route = route(set, 0, key).ok_or(format!(
            "{}: no route to sector {}",
            def.title, map.key_sector
        ))?;
        let mut rng = seed | 1;
        let sectors = route
            .iter()
            .enumerate()
            .map(|(i, &s)| {
                let last = i + 1 == route.len();
                let boss = last.then(|| map.key_npc.clone()).flatten();
                Stage {
                    map: set.sectors[s].title.to_ascii_lowercase(),
                    link: route
                        .get(i + 1)
                        .map(|&n| link_to(&set.sectors[s], &set.sectors[n].title)),
                    xp: 0,
                    bp: 0,
                    groups: self.groups(&map.sets, def.ql, boss.as_deref(), &mut rng),
                    jaco: boss.and(map.jaco.clone()),
                }
            })
            .collect();
        Ok(Plan {
            name: def.title.clone(),
            xp: def.xp,
            bp: def.bp,
            ql: def.ql,
            players: 1,
            reward_item: 0,
            sectors,
        })
    }

    fn plan_challenge(c: &Challenge) -> Plan {
        let sectors = c
            .sectors
            .iter()
            .enumerate()
            .map(|(i, (map, xp, bp, spawns))| Stage {
                map: map.to_ascii_lowercase(),
                link: (i + 1 < c.sectors.len()).then(|| "link01".to_owned()),
                xp: *xp,
                bp: *bp,
                groups: spawns
                    .iter()
                    .map(|s| Group {
                        id: s.actor.clone(),
                        count: s.num,
                        tag: if s.postag == "boss" {
                            Tag::Boss
                        } else {
                            Tag::Pos(s.postag.clone())
                        },
                        drop: s.drop.clone(),
                        hp_scale: 1.0,
                        adjust: s.adjust,
                    })
                    .collect(),
                jaco: None,
            })
            .collect();
        Plan {
            name: format!("Challenge {}", c.id),
            xp: 0,
            bp: 0,
            ql: 0,
            players: c.players.max(1),
            reward_item: c.reward_item,
            sectors,
        }
    }

    /// Survival: the loop of the survival map set (each sector's first link leads on), played
    /// [`SURVIVAL_SECTORS`] times with the NPC sets of standard quest level 1, 2, ... 5.
    fn plan_survival(&self, m: &MapSet, seed: u32) -> Result<Plan, String> {
        let mut rng = seed | 1;
        let mut at = 0;
        let mut sectors = Vec::new();
        for i in 0..SURVIVAL_SECTORS {
            let ql = (1 + i as u32 / 2).min(5);
            let def = self
                .scenarios
                .iter()
                .find(|s| s.mapset == m.title && s.id.is_none() && s.ql == ql)
                .ok_or(format!("no standard quest level {ql} for {}", m.title))?;
            let next = m.sectors[at]
                .links
                .first()
                .and_then(|l| l.targets.first())
                .and_then(|t| {
                    m.sectors
                        .iter()
                        .position(|s| s.title.eq_ignore_ascii_case(t))
                });
            sectors.push(Stage {
                map: m.sectors[at].title.to_ascii_lowercase(),
                link: Some(
                    m.sectors[at]
                        .links
                        .first()
                        .map_or(String::new(), |l| l.name.clone()),
                )
                .filter(|_| i + 1 < SURVIVAL_SECTORS),
                xp: def.xp / SURVIVAL_SHARE,
                bp: def.bp / SURVIVAL_SHARE,
                groups: self.groups(&def.maps[0].sets, ql, None, &mut rng),
                jaco: None,
            });
            at = next.ok_or(format!(
                "{}: sector {} links nowhere",
                m.title, m.sectors[at].title
            ))?;
        }
        Ok(Plan {
            name: format!("Survival {}", m.title),
            xp: 0,
            bp: 0,
            ql: 3,
            players: 1,
            reward_item: 0,
            sectors,
        })
    }

    /// The NPCs of one standard sector: `BASE_NPCS + PER_QL * ql` picked from the NPC sets (half
    /// as many next to a boss), grouped by NPC.
    fn groups(&self, sets: &[String], ql: u32, boss: Option<&str>, rng: &mut u32) -> Vec<Group> {
        let ids: Vec<String> = sets
            .iter()
            .filter_map(|s| set_npc(s))
            .filter(|i| self.npcs.known.contains(i) && !self.npcs.bosses.contains(i))
            .collect();
        let hp_scale = 1.0 + HP_PER_QL * ql.saturating_sub(1) as f32;
        let mut n = BASE_NPCS + PER_QL * ql;
        let mut out = Vec::new();
        if let Some(b) = boss {
            out.push(Group {
                id: b.to_owned(),
                count: 1,
                tag: Tag::Boss,
                drop: String::new(),
                hp_scale: 1.0,
                adjust: false,
            });
            n /= 2;
        }
        let mut counts: Vec<(String, u32)> = Vec::new();
        for _ in 0..if ids.is_empty() { 0 } else { n } {
            let id = &ids[next(rng) as usize % ids.len()];
            match counts.iter_mut().find(|c| &c.0 == id) {
                Some(c) => c.1 += 1,
                None => counts.push((id.clone(), 1)),
            }
        }
        for (id, count) in counts {
            let tag = if self.npcs.ranged.contains(&id) {
                Tag::Range
            } else {
                Tag::Melee
            };
            out.push(Group {
                id,
                count,
                tag,
                drop: String::new(),
                hp_scale,
                adjust: false,
            });
        }
        out
    }
}

/// The names for the menu's scenario stepper (first = default); empty if the files are missing.
pub fn scenario_names(vfs: &crate::mrs::Vfs) -> Vec<String> {
    Catalog::load(vfs).map(|c| c.names()).unwrap_or_default()
}

/// xorshift32.
fn next(s: &mut u32) -> u32 {
    *s ^= *s << 13;
    *s ^= *s >> 17;
    *s ^= *s << 5;
    *s
}

/// Uniform 0..1.
fn unit(s: &mut u32) -> f32 {
    (next(s) >> 8) as f32 / (1u32 << 24) as f32
}

/// The `npc.xml` id of an `NPCSET_ARRAY` entry like `G11` (*inferred*: the sets are not in the
/// data, but `G11`..`G15`, `K21`.. are the `<DROP table>` names of NPC ids 11.., 21..): family
/// letter (`G` goblin 10, `K` kobold 20, `S` skeleton 30, `P` palmpow 40), quest level digit,
/// member number. Level 0 uses the weak variants of the 15x/16x/17x ids.
fn set_npc(set: &str) -> Option<String> {
    let b = set.as_bytes();
    let n: u32 = set.get(2..)?.parse().ok()?;
    let ql = (*b.get(1)? as char).to_digit(10)?;
    let base = match b[0] {
        b'G' => 10,
        b'K' => 20,
        b'S' => 30,
        b'P' => 40,
        _ => return None,
    };
    let weak = match b[0] {
        b'G' => 150,
        b'K' => 160,
        b'S' => 170,
        _ => base,
    };
    Some((if ql == 0 { weak } else { base } + n).to_string())
}

/// Sector indices of the shortest route `from` -> `to` over the links (breadth first).
fn route(set: &MapSet, from: usize, to: usize) -> Option<Vec<usize>> {
    let mut prev: HashMap<usize, usize> = HashMap::new();
    let mut seen = HashSet::from([from]);
    let mut todo = VecDeque::from([from]);
    while let Some(at) = todo.pop_front() {
        if at == to {
            let mut path = vec![at];
            while let Some(&p) = prev.get(path.last()?) {
                path.push(p);
            }
            path.reverse();
            return Some(path);
        }
        for t in set.sectors[at].links.iter().flat_map(|l| &l.targets) {
            if let Some(i) = set
                .sectors
                .iter()
                .position(|s| s.title.eq_ignore_ascii_case(t))
                && seen.insert(i)
            {
                prev.insert(i, at);
                todo.push_back(i);
            }
        }
    }
    None
}

/// The portal dummy of `from` that leads to the sector titled `to` ("" if its link is unnamed).
fn link_to(from: &Sector, to: &str) -> String {
    let hit = from
        .links
        .iter()
        .find(|l| l.targets.iter().any(|t| t.eq_ignore_ascii_case(to)));
    hit.or(from.links.first())
        .map_or(String::new(), |l| l.name.clone())
}

// ---------------------------------------------------------------- plan

/// Where a group's NPCs appear: the `spawn_npc_melee*` / `spawn_npc_range*` / `spawn_npc_boss*`
/// dummies of the quest maps, or `spawn_npc_<postag>` of the challenge maps.
#[derive(Clone, Debug, PartialEq)]
pub enum Tag {
    Melee,
    Range,
    Boss,
    Pos(String),
}

/// `count` NPCs `id` (an `npc.xml` id or `npc2.xml` actor name).
#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub id: String,
    pub count: u32,
    pub tag: Tag,
    pub drop: String,
    pub hp_scale: f32,
    /// Challenge `adjustplayernum`: the hit points follow the party size.
    pub adjust: bool,
}

/// One sector of a quest.
#[derive(Clone, Debug, PartialEq)]
pub struct Stage {
    /// Map directory name.
    pub map: String,
    /// Portal dummy to the next sector (`None` on the last sector; "" = the first `link*`).
    pub link: Option<String>,
    /// Paid when this sector is cleared (challenge quest, survival).
    pub xp: u32,
    pub bp: u32,
    pub groups: Vec<Group>,
    pub jaco: Option<Jaco>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub name: String,
    /// Paid when the last sector is cleared (standard quests).
    pub xp: u32,
    pub bp: u32,
    /// Quest level, picks the drop-table level.
    pub ql: u32,
    /// Party size the challenge quest is balanced for.
    pub players: u32,
    /// Item granted on a cleared challenge quest (a shop item id, 0 = none).
    pub reward_item: u32,
    pub sectors: Vec<Stage>,
}

// ---------------------------------------------------------------- play

#[derive(Clone, Copy, PartialEq, Debug)]
enum Phase {
    Intro,
    Fight,
    Open,
    Won,
    Lost,
}

struct Pending {
    spawn: SpawnNpc,
}

/// The running quest (inserted by `gunz-play`).
#[derive(Resource)]
pub struct Quest {
    plan: Plan,
    drops: Drops,
    /// Quest item names (`QITEM_NAME_<id>` of `strings.xml`) and world item amounts.
    names: HashMap<u32, String>,
    world: HashMap<String, (ItemKind, u32)>,
    stage: usize,
    phase: Phase,
    t: f32,
    queue: Vec<Pending>,
    since_spawn: f32,
    jaco_t: f32,
    kills: u32,
    loot: Vec<(u32, u32)>,
    swap: bool,
    rng: u32,
    banner: String,
    /// Player plus allied bots (challenge HP balancing).
    party: u32,
    /// Seconds the player has been dead.
    dead_t: f32,
}

impl Quest {
    pub fn new(vfs: &crate::mrs::Vfs, plan: Plan, bots: usize, seed: u32) -> Result<Self, String> {
        let read = |p: &str| {
            vfs.read(p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .map_err(|e| format!("{p}: {e}"))
        };
        let strings = read("system/strings.xml")?;
        let names = doc(&strings)?
            .descendants()
            .filter_map(|n| {
                let id = n
                    .attribute("id")?
                    .strip_prefix("QITEM_NAME_")?
                    .parse()
                    .ok()?;
                Some((id, n.text()?.trim().to_owned()))
            })
            .collect();
        let items = read("system/worlditem.xml")?;
        let world = doc(&items)?
            .descendants()
            .filter(|n| n.has_tag_name("WORLDITEM"))
            .filter_map(|n| {
                let field = |t: &str| {
                    n.children()
                        .find(|c| c.has_tag_name(t))?
                        .text()
                        .map(str::trim)
                };
                let kind = match field("TYPE")? {
                    "hp" => ItemKind::Hp,
                    "ap" => ItemKind::Ap,
                    "bullet" => ItemKind::Bullet,
                    _ => return None,
                };
                Some((
                    n.attribute("name")?.to_owned(),
                    (kind, field("AMOUNT")?.parse().ok()?),
                ))
            })
            .collect();
        Ok(Self {
            plan,
            drops: parse_drops(&read("system/droptable.xml")?)?,
            names,
            world,
            stage: 0,
            phase: Phase::Intro,
            t: 0.0,
            queue: Vec::new(),
            since_spawn: 0.0,
            jaco_t: 0.0,
            kills: 0,
            loot: Vec::new(),
            swap: false,
            rng: seed | 1,
            banner: String::new(),
            party: 1 + bots as u32,
            dead_t: 0.0,
        })
    }

    pub fn first_map(&self) -> &str {
        &self.plan.sectors[0].map
    }

    fn stage(&self) -> &Stage {
        &self.plan.sectors[self.stage]
    }

    fn last(&self) -> bool {
        self.stage + 1 == self.plan.sectors.len()
    }

    fn add_loot(&mut self, id: u32) {
        match self.loot.iter_mut().find(|l| l.0 == id) {
            Some(l) => l.1 += 1,
            None => self.loot.push((id, 1)),
        }
    }

    /// The `strings.xml` name; the HUD font has no Hangul, so those show the id.
    fn item_name(&self, id: u32) -> String {
        match self.names.get(&id) {
            Some(n) if n.is_ascii() => n.clone(),
            _ => format!("item {id}"),
        }
    }

    /// Queues the current sector's NPCs, the boss first, at the map's `spawn_npc_*` dummies.
    fn begin_stage(&mut self, level: &Level) {
        let dummies = |keep: &dyn Fn(&str) -> bool| -> Vec<(Vec3, f32)> {
            level
                .map
                .dummies
                .iter()
                .filter(|d| keep(&d.name.to_ascii_lowercase()))
                .map(|d| {
                    let dir = Vec3::from(to_bevy(d.dir));
                    (Vec3::from(to_bevy(d.pos)) * SCALE, yaw_of(dir))
                })
                .collect()
        };
        let mut queue = Vec::new();
        let groups = self.stage().groups.clone();
        for g in groups
            .iter()
            .filter(|g| g.tag == Tag::Boss)
            .chain(groups.iter().filter(|g| g.tag != Tag::Boss))
        {
            let mut spots = dummies(&|n| match &g.tag {
                Tag::Melee => n.starts_with("spawn_npc_melee"),
                Tag::Range => n.starts_with("spawn_npc_range"),
                Tag::Boss => n.starts_with("spawn_npc_boss"),
                Tag::Pos(p) => n == format!("spawn_npc_{p}"),
            });
            if spots.is_empty() {
                spots = dummies(&|n| n.starts_with("spawn_npc_"));
            }
            if spots.is_empty() {
                spots = level
                    .spawn_points()
                    .into_iter()
                    .map(|(p, d)| (p, yaw_of(d)))
                    .collect();
            }
            for i in (1..spots.len()).rev() {
                spots.swap(i, next(&mut self.rng) as usize % (i + 1));
            }
            let scale = if g.adjust {
                self.party as f32 / self.plan.players as f32
            } else {
                g.hp_scale
            };
            for k in 0..g.count as usize {
                let (pos, yaw) = spots[k % spots.len()];
                // a second lap around the spots stands a little aside (*inferred*)
                let aside = Vec3::X * 0.7 * (k / spots.len()) as f32;
                queue.push(Pending {
                    spawn: SpawnNpc {
                        id: g.id.clone(),
                        pos: pos + aside + Vec3::Y * 0.05,
                        yaw,
                        hp_scale: scale,
                        drop: g.drop.clone(),
                        boss: g.tag == Tag::Boss,
                        ..default()
                    },
                });
            }
        }
        queue.reverse();
        let total = queue.len();
        self.queue = queue;
        self.phase = Phase::Intro;
        self.t = 0.0;
        self.since_spawn = 0.0;
        self.jaco_t = 0.0;
        println!(
            "quest: sector {}/{} {} ({} NPCs queued)",
            self.stage + 1,
            self.plan.sectors.len(),
            self.stage().map,
            total
        );
    }
}

/// The open portal of a cleared sector.
#[derive(Component)]
struct Portal;

/// A dropped quest/shop item (hp/ap/ammo drops are `WorldItem`s and collected by `pickup.rs`).
#[derive(Component)]
struct Drop {
    id: Option<u32>,
    base: f32,
}

#[derive(Component)]
struct Banner;

pub struct QuestPlugin;

impl Plugin for QuestPlugin {
    fn build(&self, app: &mut App) {
        let on = resource_exists::<Quest>;
        app.add_systems(PostStartup, start.run_if(on)).add_systems(
            Update,
            (allies, kills, drive, collect, banner, change_sector)
                .chain()
                .run_if(on),
        );
    }
}

/// Dead actors stay dead for the sector; the first map's NPCs are queued once the map exists
/// (`change_sector` reruns `PostStartup`, hence the guard).
fn start(
    mut quest: ResMut<Quest>,
    mut rules: ResMut<Rules>,
    level: Res<Level>,
    mut done: Local<bool>,
) {
    if std::mem::replace(&mut *done, true) {
        return;
    }
    rules.respawn = HOLD;
    quest.begin_stage(&level);
}

/// The player and the bot allies fight for Red, the NPCs are Blue.
fn allies(mut commands: Commands, new: Query<Entity, (Added<Score>, Without<Team>, Without<Npc>)>) {
    for e in &new {
        commands.entity(e).insert(Team::Red);
    }
}

#[allow(clippy::too_many_arguments)]
fn drive(
    time: Res<Time>,
    mut quest: ResMut<Quest>,
    mut clock: ResMut<Clock>,
    mut vtime: ResMut<Time<Virtual>>,
    mut commands: Commands,
    mut spawn: MessageWriter<SpawnNpc>,
    mut reward: MessageWriter<Reward>,
    mut loot: MessageWriter<QuestLoot>,
    level: Res<Level>,
    player: Query<(&Transform, Has<Dead>), With<Player>>,
    npcs: Query<(&Npc, Has<Dead>)>,
    portal: Query<Entity, With<Portal>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let dt = time.delta_secs();
    quest.t += dt;
    quest.since_spawn += dt;
    let live = npcs.iter().filter(|n| !n.1).count();
    let boss_alive = npcs.iter().any(|n| n.0.boss && !n.1);
    let (stage_n, stages) = (quest.stage + 1, quest.plan.sectors.len());
    let name = quest.stage().map.clone();
    clock.note = format!(
        "SECTOR {stage_n}/{stages}  NPC {}",
        live + quest.queue.len()
    );
    let p = player.single().ok();
    if matches!(quest.phase, Phase::Won | Phase::Lost) {
        return;
    }
    quest.dead_t = if p.is_some_and(|p| p.1) {
        quest.dead_t + dt
    } else {
        0.0
    };
    if quest.dead_t > FAIL_SECS {
        // the player fell: the quest is lost once the body has lain there a moment
        end(
            &mut quest,
            &mut clock,
            &mut vtime,
            &mut commands,
            false,
            &mut reward,
            &mut loot,
        );
        return;
    }
    match quest.phase {
        Phase::Intro => {
            quest.banner = format!("SECTOR {stage_n}/{stages}\n{name}");
            if quest.t >= INTRO_SECS {
                quest.phase = Phase::Fight;
                quest.t = 0.0;
            }
        }
        Phase::Fight => {
            quest.banner.clear();
            if live < MAX_LIVE && quest.since_spawn >= SPAWN_GAP && !quest.queue.is_empty() {
                let next = quest.queue.pop().unwrap();
                spawn.write(next.spawn);
                quest.since_spawn = 0.0;
            }
            if let Some(j) = quest.stage().jaco.clone()
                && boss_alive
                && live < j.max_npc as usize
            {
                quest.jaco_t += dt;
                if quest.jaco_t >= j.tick {
                    quest.jaco_t = 0.0;
                    for _ in 0..j.count {
                        let id = pick_weighted(&j.npcs, &mut quest.rng);
                        let spots: Vec<_> = level
                            .map
                            .dummies
                            .iter()
                            .filter(|d| d.name.to_ascii_lowercase().starts_with("spawn_npc_melee"))
                            .collect();
                        if let (Some(id), false) = (id, spots.is_empty()) {
                            let d = spots[next(&mut quest.rng) as usize % spots.len()];
                            spawn.write(SpawnNpc {
                                id,
                                pos: Vec3::from(to_bevy(d.pos)) * SCALE + Vec3::Y * 0.05,
                                yaw: yaw_of(Vec3::from(to_bevy(d.dir))),
                                hp_scale: 1.0,
                                drop: String::new(),
                                boss: false,
                                ..default()
                            });
                        }
                    }
                }
            }
            // a spawn written this frame is not an entity yet: wait a beat after the last one
            if quest.queue.is_empty() && live == 0 && quest.since_spawn > 1.0 {
                cleared(
                    &mut quest,
                    &mut commands,
                    &mut meshes,
                    &mut materials,
                    &level,
                    &mut reward,
                );
                if quest.last() {
                    end(
                        &mut quest,
                        &mut clock,
                        &mut vtime,
                        &mut commands,
                        true,
                        &mut reward,
                        &mut loot,
                    );
                }
            }
        }
        Phase::Open => {
            let left = OPEN_SECS - quest.t;
            quest.banner = format!("SECTOR CLEARED\nportal closes in {:.0}s", left.max(0.0));
            let at = portal.single().ok().and_then(|_| p);
            let through = at.is_some_and(|(tf, _)| {
                level_portal(&level, quest.stage().link.as_deref()).is_some_and(|c| {
                    (tf.translation - c).xz().length() < PORTAL_R
                        && (tf.translation.y - c.y).abs() < 2.5
                })
            });
            if through || left <= 0.0 {
                for e in &portal {
                    commands.entity(e).despawn();
                }
                quest.swap = true;
            }
        }
        Phase::Won | Phase::Lost => {}
    }
}

/// A weighted pick of `(id, rate)`; the rates need not add up to 1.
fn pick_weighted(list: &[(String, f32)], rng: &mut u32) -> Option<String> {
    let total: f32 = list.iter().map(|l| l.1).sum();
    let mut r = unit(rng) * total;
    list.iter()
        .find(|l| {
            r -= l.1;
            r < 0.0
        })
        .map(|l| l.0.clone())
}

fn level_portal(level: &Level, link: Option<&str>) -> Option<Vec3> {
    let link = link?.to_ascii_lowercase();
    let mut links = level
        .map
        .dummies
        .iter()
        .filter(|d| d.name.to_ascii_lowercase().starts_with("link"));
    let d = if link.is_empty() {
        links.next()
    } else {
        level
            .map
            .dummies
            .iter()
            .find(|d| d.name.to_ascii_lowercase() == link)
            .or(links.next())
    }?;
    Some(Vec3::from(to_bevy(d.pos)) * SCALE)
}

/// A sector is cleared: pay a sector reward, open the portal.
fn cleared(
    quest: &mut Quest,
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    level: &Level,
    reward: &mut MessageWriter<Reward>,
) {
    let (xp, bp) = (quest.stage().xp, quest.stage().bp);
    if xp + bp > 0 {
        reward.write(Reward { xp, bounty: bp });
    }
    println!(
        "quest: sector {} cleared ({} kills, reward {xp} XP {bp} BP)",
        quest.stage + 1,
        quest.kills
    );
    if quest.last() {
        return;
    }
    quest.phase = Phase::Open;
    quest.t = 0.0;
    if let Some(at) = level_portal(level, quest.stage().link.as_deref()) {
        commands.spawn((
            Portal,
            Mesh3d(meshes.add(Cylinder::new(PORTAL_R * 0.8, 2.4))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: Color::srgba(0.3, 0.9, 1.0, 0.28),
                emissive: LinearRgba::rgb(0.2, 0.8, 1.0),
                alpha_mode: AlphaMode::Blend,
                unlit: true,
                ..default()
            })),
            Transform::from_translation(at + Vec3::Y * 1.2),
        ));
    }
}

/// The quest is over: pay the reward, hand the loot to the profile, show the end screen.
fn end(
    quest: &mut Quest,
    clock: &mut Clock,
    vtime: &mut Time<Virtual>,
    commands: &mut Commands,
    won: bool,
    reward: &mut MessageWriter<Reward>,
    loot: &mut MessageWriter<QuestLoot>,
) {
    quest.phase = if won { Phase::Won } else { Phase::Lost };
    if won {
        if quest.plan.xp + quest.plan.bp > 0 {
            reward.write(Reward {
                xp: quest.plan.xp,
                bounty: quest.plan.bp,
            });
        }
        if quest.plan.reward_item != 0 {
            let id = quest.plan.reward_item;
            quest.add_loot(id);
        }
    }
    loot.write(QuestLoot {
        items: quest.loot.clone(),
    });
    let items: Vec<String> = quest
        .loot
        .iter()
        .map(|l| format!("{} x{}", quest.item_name(l.0), l.1))
        .collect();
    println!(
        "quest: {} {} (sector {}/{}, {} kills, loot: {})",
        quest.plan.name,
        if won { "CLEARED" } else { "FAILED" },
        quest.stage + 1,
        quest.plan.sectors.len(),
        quest.kills,
        if items.is_empty() {
            "none".into()
        } else {
            items.join(", ")
        }
    );
    clock.over = Some(if won { "QUEST CLEARED" } else { "QUEST FAILED" }.into());
    freeze(commands, vtime, true);
}

/// A slain NPC rolls its drop table where it fell.
fn kills(
    mut quest: ResMut<Quest>,
    mut killed: MessageReader<Killed>,
    npcs: Query<(&Npc, &Transform)>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for k in killed.read() {
        let Ok((npc, tf)) = npcs.get(k.victim) else {
            continue;
        };
        quest.kills += 1;
        let table = if npc.drop.is_empty() {
            "G11"
        } else {
            npc.drop.as_str()
        };
        let r = unit(&mut quest.rng);
        // challenge-quest tables (`C1`, `C2`) are not in `droptable.xml`: hp, ap or ammo (*inferred*)
        let item = match quest.drops.0.contains_key(table) {
            true => quest.drops.roll(table, quest.plan.ql, r).map(str::to_owned),
            false => ["hp1", "ap1", "mag1"]
                .get((r * 4.0) as usize)
                .map(|s| (*s).to_owned()),
        };
        let Some(item) = item else { continue };
        let at = tf.translation + Vec3::Y * 0.05;
        let sphere = Mesh3d(meshes.add(Sphere::new(0.16)));
        let mut glow = |c: LinearRgba| {
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: c.into(),
                emissive: c * 3.0,
                unlit: true,
                ..default()
            }))
        };
        let world = quest.world.get(&world_name(&item)).copied();
        match (world, item.parse::<u32>()) {
            (Some((kind, amount)), _) => {
                let c = match kind {
                    ItemKind::Hp => LinearRgba::rgb(1.0, 0.1, 0.1),
                    ItemKind::Ap => LinearRgba::rgb(0.1, 1.0, 0.2),
                    ItemKind::Bullet => LinearRgba::rgb(1.0, 0.8, 0.1),
                };
                commands.spawn((
                    Drop {
                        id: None,
                        base: at.y + 0.3,
                    },
                    WorldItem {
                        kind,
                        amount,
                        respawn: f32::MAX,
                        cooldown: 0.0,
                    },
                    sphere,
                    glow(c),
                    Transform::from_translation(at + Vec3::Y * 0.3),
                ));
            }
            (None, Ok(id)) => {
                commands.spawn((
                    Drop {
                        id: Some(id),
                        base: at.y + 0.5,
                    },
                    sphere,
                    glow(LinearRgba::rgb(0.4, 0.8, 1.0)),
                    Transform::from_translation(at + Vec3::Y * 0.5),
                ));
            }
            _ => warn!(
                "quest: drop {item:?} of {} is neither a world item nor an id",
                npc.id
            ),
        }
        println!("quest: {} dropped {item}", npc.id);
    }
}

/// `hp1` -> `hp01`, `mag1` -> `bullet01` (`droptable.xml` -> `worlditem.xml` names, *inferred*).
fn world_name(item: &str) -> String {
    let (kind, n) = item.split_at(
        item.find(|c: char| c.is_ascii_digit())
            .unwrap_or(item.len()),
    );
    let kind = if kind == "mag" { "bullet" } else { kind };
    format!("{kind}{n:0>2}")
}

/// Drops bob; a taken `WorldItem` drop disappears; the player collects quest items.
fn collect(
    time: Res<Time>,
    mut quest: ResMut<Quest>,
    mut commands: Commands,
    player: Query<&Transform, (With<Player>, Without<Drop>)>,
    mut drops: Query<(Entity, &Drop, &mut Transform, Option<&WorldItem>)>,
) {
    let now = time.elapsed_secs();
    for (e, d, mut tf, world) in &mut drops {
        tf.translation.y = d.base + 0.08 * (now * 3.0).sin();
        if world.is_some_and(|w| w.cooldown > 0.0) {
            commands.entity(e).despawn();
            continue;
        }
        let (Some(id), Ok(p)) = (d.id, player.single()) else {
            continue;
        };
        let off = tf.translation - p.translation;
        if off.xz().length() < PICKUP_R && off.y.abs() < 2.5 {
            quest.add_loot(id);
            quest.banner = format!("{} picked up", quest.item_name(id));
            println!("quest: picked up {} ({id})", quest.item_name(id));
            commands.entity(e).despawn();
        }
    }
}

/// The centred sector banner (also used for pickups; they replace each other).
fn banner(
    mut commands: Commands,
    quest: Res<Quest>,
    camera: Query<Entity, With<Camera3d>>,
    mut text: Query<&mut Text, With<Banner>>,
) {
    if let Ok(mut t) = text.single_mut() {
        if t.0 != quest.banner {
            t.0 = quest.banner.clone();
        }
        return;
    }
    let Ok(camera) = camera.single() else { return };
    commands
        .spawn((
            UiTargetCamera(camera),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                top: px(110),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|r| {
            r.spawn((
                Banner,
                Text::new(""),
                TextFont::from_font_size(40.0),
                TextColor(Color::srgb(1.0, 0.85, 0.3)),
                TextShadow::default(),
                TextLayout {
                    justify: Justify::Center,
                    ..default()
                },
            ));
        });
}

/// Swaps the map for the next sector: drop everything of the old one, load the new `Level` and
/// `MapCollision`, rerun the map-dependent startup systems (level, props, spawn table, then
/// the whole `PostStartup`: allied bots, nav, items), stand the player on the first spawn.
fn change_sector(world: &mut World) {
    if !world.resource::<Quest>().swap {
        return;
    }
    let next = {
        let mut q = world.resource_mut::<Quest>();
        q.swap = false;
        q.stage += 1;
        q.stage().map.clone()
    };
    let doomed: Vec<Entity> = world
        .query_filtered::<Entity, Or<(
            With<MapEntity>,
            With<Npc>,
            With<Drop>,
            With<Portal>,
            With<Bot>,
        )>>()
        .iter(world)
        .collect();
    let items: Vec<Entity> = world
        .query_filtered::<Entity, With<WorldItem>>()
        .iter(world)
        .collect();
    for e in doomed.into_iter().chain(items) {
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
    let (rs, col, map) = {
        let vfs = &world.resource::<Level>().vfs;
        let rs = map::find_rs(vfs, &next).unwrap_or_else(|| panic!("no map named {next}"));
        let col = MapCollision::load(vfs, &rs).unwrap_or_else(|e| panic!("{rs} collision: {e}"));
        let map = map::load(vfs, &rs).unwrap_or_else(|e| panic!("{rs}: {e}"));
        (rs, col, map)
    };
    println!("quest: loading sector map {rs}");
    world.resource_mut::<Level>().map = map;
    world.insert_resource(col);
    // the old map's nav: bots rebuild it in `PostStartup`, `npc.rs` when it needs one
    world.remove_resource::<crate::nav::Nav>();
    world.resource_scope(|w, mut data: Mut<ActorData>| data.rebase(w.resource::<Level>()));
    let _ = world.run_system_cached(level::spawn_level);
    let _ = world.run_system_cached(props::spawn_props);
    let _ = world.run_system_cached(modes::spawn_table);
    world.run_schedule(PostStartup);
    let spawn = world.resource::<Level>().spawn_points().first().copied();
    if let (Some((pos, dir)), Ok((mut tf, mut intent, mut motor))) = (
        spawn,
        world
            .query_filtered::<(&mut Transform, &mut Intent, &mut Motor), With<Player>>()
            .single_mut(world),
    ) {
        tf.translation = pos + Vec3::Y * 0.1;
        intent.yaw = yaw_of(dir);
        motor.vel = Vec3::ZERO;
    }
    let bots = world.resource::<BotCount>().0;
    world.resource_scope(|w, mut q: Mut<Quest>| {
        q.party = 1 + bots as u32;
        q.begin_stage(w.resource::<Level>());
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAPS: &str = r#"<XML><MAPSET id="1" title="Mansion">
        <SECTOR id="101" title="Mansion_Hall1"><LINK name="link01"><TARGET sector="Mansion_Hall2"/><TARGET sector="Mansion_Passage1"/></LINK></SECTOR>
        <SECTOR id="102" title="Mansion_Hall2"><LINK name="link01"><TARGET sector="Mansion_Hall1"/></LINK><LINK name="link02"><TARGET sector="Mansion_Room1"/></LINK></SECTOR>
        <SECTOR id="104" title="Mansion_Room1"><LINK><TARGET sector="Mansion_Hall2"/></LINK></SECTOR>
        <SECTOR id="107" title="Mansion_Passage1"><LINK name="link01"><TARGET sector="Mansion_Hall1"/></LINK></SECTOR>
        </MAPSET></XML>"#;
    const SCENARIO: &str = r#"<XML>
        <STANDARD_SCENARIO QL="2" title="Quest Mansion QL2" DC="1" mapset="Mansion" XP="360" BP="60">
          <MAP dice="1" key_sector="101"><NPCSET_ARRAY>G21/G22</NPCSET_ARRAY></MAP>
          <MAP dice="2" key_sector="104"><NPCSET_ARRAY>G21/G22/G23</NPCSET_ARRAY></MAP>
        </STANDARD_SCENARIO>
        <SPECIAL_SCENARIO id="11" title="Goblin King" QL="1" DC="1" mapset="Mansion" XP="5000" BP="500">
          <SACRI_ITEM itemid="200008" />
          <MAP dice="1" key_sector="102" key_npc="16" boss="true"><NPCSET_ARRAY>G61</NPCSET_ARRAY>
            <JACO count="2" tick="5" min_npc="0" max_npc="13"><NPC npcid="2011" rate="0.2" /><NPC npcid="2012" rate="0.3" /></JACO></MAP>
        </SPECIAL_SCENARIO></XML>"#;
    const CHALLENGE: &str = r#"<XML><SCENARIO map_id="101" name="UI_STAGE_101" reward_item="3000101" players="4" level_limit="01" good_time_sec="480">
        <SECTOR map="G_Easy_1" xp="200" bp="20"><SPAWN postag="1" num="8" actor="knifeman" drop="C1"/></SECTOR>
        <SECTOR map="G_Easy_6" xp="1500" bp="50"><SPAWN postag="boss" num="1" actor="robot" drop="" adjustplayernum="true"/></SECTOR>
        </SCENARIO></XML>"#;
    const NPC: &str = r#"<XML><NPC id="11" offensetype="1"/><NPC id="12" offensetype="2"/><NPC id="13" offensetype="1"/><NPC id="16" offensetype="1"/></XML>"#;

    fn catalog() -> Catalog {
        Catalog::parse(MAPS, MAPS, SCENARIO, CHALLENGE, NPC).unwrap()
    }

    #[test]
    fn standard_quest_walks_the_links_to_the_key_sector() {
        let c = catalog();
        let p = c.plan("quest mansion ql2", Some(2), 7).unwrap();
        let maps: Vec<_> = p.sectors.iter().map(|s| s.map.as_str()).collect();
        assert_eq!(maps, ["mansion_hall1", "mansion_hall2", "mansion_room1"]);
        // hall1 -> hall2 through its link01, hall2 -> room1 through link02, room1 is last
        let links: Vec<_> = p.sectors.iter().map(|s| s.link.as_deref()).collect();
        assert_eq!(links, [Some("link01"), Some("link02"), None]);
        assert_eq!((p.xp, p.bp, p.ql), (360, 60, 2));
        // G21..G23 -> npc.xml ids 11..13 (12 shoots, so it spawns at the range dummies);
        // 8 + 2*2 NPCs, hit points x1.25 at level 2
        let g = &p.sectors[0].groups;
        assert_eq!(g.iter().map(|g| g.count).sum::<u32>(), 12);
        assert!(
            g.iter()
                .all(|g| g.hp_scale == 1.25 && (g.id != "12" || g.tag == Tag::Range))
        );
    }

    #[test]
    fn special_quest_has_a_boss_in_the_key_sector_and_reinforcements() {
        let p = catalog().plan("11", None, 3).unwrap();
        assert_eq!(p.sectors.len(), 2);
        assert!(
            p.sectors[0].jaco.is_none() && p.sectors[0].groups.iter().all(|g| g.tag != Tag::Boss)
        );
        let last = &p.sectors[1];
        assert_eq!(last.groups[0].id, "16");
        assert_eq!(last.groups[0].tag, Tag::Boss);
        assert_eq!(
            last.jaco.as_ref().unwrap().npcs[1],
            ("2012".to_owned(), 0.3)
        );
    }

    #[test]
    fn challenge_sectors_pay_and_chain() {
        let c = catalog();
        assert_eq!(c.names()[2], "Challenge 101");
        let p = c.plan("Challenge 101", None, 1).unwrap();
        assert_eq!((p.players, p.reward_item), (4, 3000101));
        assert_eq!(
            (p.sectors[0].map.as_str(), p.sectors[0].xp, p.sectors[0].bp),
            ("g_easy_1", 200, 20)
        );
        assert_eq!(p.sectors[0].link.as_deref(), Some("link01"));
        assert_eq!(p.sectors[0].groups[0].tag, Tag::Pos("1".into()));
        assert_eq!(p.sectors[1].groups[0].tag, Tag::Boss);
        assert!(p.sectors[1].groups[0].adjust && p.sectors[1].link.is_none());
        assert!(c.plan("nonsense", None, 1).is_err());
    }

    #[test]
    fn drop_roll_is_cumulative_and_levelled() {
        let d = parse_drops(
            r#"<XML><DROPSET id="11" name="G11"><ITEMSET QL="0"><ITEM id="hp1" rate="0.20"/><ITEM id="200011" rate="0.30"/></ITEMSET>
               <ITEMSET QL="3"><ITEM id="ap1" rate="1.0"/></ITEMSET></DROPSET></XML>"#,
        )
        .unwrap();
        assert_eq!(d.roll("G11", 0, 0.1), Some("hp1"));
        assert_eq!(d.roll("G11", 0, 0.4), Some("200011"));
        assert_eq!(d.roll("G11", 1, 0.9), None);
        assert_eq!(d.roll("G11", 5, 0.9), Some("ap1"));
        assert_eq!(d.roll("none", 0, 0.1), None);
        assert_eq!(world_name("hp1"), "hp01");
        assert_eq!(world_name("mag1"), "bullet01");
    }

    #[test]
    fn npc_sets_name_npc_ids() {
        assert_eq!(set_npc("G14").as_deref(), Some("14"));
        assert_eq!(set_npc("G02").as_deref(), Some("152"));
        assert_eq!(set_npc("K53").as_deref(), Some("23"));
        assert_eq!(set_npc("X11"), None);
    }

    /// With the retail extract (`.local/extract`): every scenario plans, every sector map exists.
    #[test]
    fn retail_scenarios_plan() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".local/extract");
        let read = |p: &str| std::fs::read_to_string(dir.join(p));
        let (Ok(a), Ok(b), Ok(c), Ok(d), Ok(e)) = (
            read("system/questmap.xml"),
            read("system/survivalmap.xml"),
            read("system/scenario.xml"),
            read("system/scenario2.xml"),
            read("system/npc.xml"),
        ) else {
            return;
        };
        let cat = Catalog::parse(&a, &b, &c, &d, &e).unwrap();
        let names = cat.names();
        assert!(names.len() >= 30, "{names:?}");
        for n in names {
            let p = cat.plan(&n, None, 1).unwrap_or_else(|e| panic!("{n}: {e}"));
            for s in &p.sectors {
                let (q, ch) = (
                    dir.join("quest/maps").join(&s.map),
                    dir.join("challengequest/maps").join(&s.map),
                );
                assert!(q.is_dir() || ch.is_dir(), "{n}: no map {}", s.map);
                assert!(!s.groups.is_empty(), "{n}: {} has no NPCs", s.map);
            }
        }
    }

    /// The player walking over a quest-item drop collects it; a taken world-item drop vanishes.
    #[test]
    fn drops_are_collected_or_cleaned_up() {
        let plan = Plan {
            name: "t".into(),
            xp: 0,
            bp: 0,
            ql: 0,
            players: 1,
            reward_item: 0,
            sectors: vec![],
        };
        let quest = Quest {
            plan,
            drops: Drops::default(),
            names: HashMap::from([(200011, "Ore Fragment".to_owned())]),
            world: HashMap::new(),
            stage: 0,
            phase: Phase::Fight,
            t: 0.0,
            queue: vec![],
            since_spawn: 0.0,
            jaco_t: 0.0,
            kills: 0,
            loot: vec![],
            swap: false,
            rng: 1,
            banner: String::new(),
            party: 1,
            dead_t: 0.0,
        };
        let mut app = App::new();
        app.add_plugins(MinimalPlugins)
            .insert_resource(quest)
            .add_systems(Update, collect);
        app.world_mut()
            .spawn((Player, Transform::from_xyz(0.0, 0.0, 0.0)));
        let near = |x: f32| Transform::from_xyz(x, 0.5, 0.0);
        app.world_mut().spawn((
            Drop {
                id: Some(200011),
                base: 0.5,
            },
            near(0.3),
        ));
        app.world_mut().spawn((
            Drop {
                id: Some(200012),
                base: 0.5,
            },
            near(5.0),
        ));
        let taken = WorldItem {
            kind: ItemKind::Hp,
            amount: 10,
            respawn: f32::MAX,
            cooldown: 1.0,
        };
        app.world_mut().spawn((
            Drop {
                id: None,
                base: 0.5,
            },
            taken,
            near(9.0),
        ));
        app.update();
        let q = app.world().resource::<Quest>();
        assert_eq!(q.loot, [(200011, 1)]);
        assert_eq!(q.banner, "Ore Fragment picked up");
        // only the far quest item is left
        assert_eq!(
            app.world_mut().query::<&Drop>().iter(app.world()).count(),
            1
        );
    }
}
