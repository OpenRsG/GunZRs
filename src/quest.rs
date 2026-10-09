//! Offline quest play (`gunz-play --mode quest --scenario NAME [--dice N] [--sacrifice A,B]`): the
//! retail quest data (`system/scenario.xml`, `scenario2.xml`, `questmap.xml`, `survivalmap.xml`,
//! `sacrificetable.xml`, `droptable.xml`, `zquestitem.xml`, `npc.xml`) is parsed into a [`Plan`] (a chain of sectors,
//! each a map with NPC groups), and [`QuestPlugin`] plays it: NPCs are spawned in waves at the
//! map's `spawn_npc_*` dummies (through `game::SpawnNpc`, `npc.rs` makes them), a cleared sector
//! opens its `linkNN` portal, and walking into it (or a timer) swaps the map in-process for the
//! next sector. Formats and *inferred* constants: `docs/formats.md`, "Quest".

use crate::{
    actor::{ActorData, PlayerSetup},
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
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

/// NPCs of a standard sector at quest level 0, plus [`PER_QL`] for every further level
/// (*inferred*: the data only gives the NPC sets).
const BASE_NPCS: u32 = 8;
const PER_QL: u32 = 2;
/// Bonus XP/BP of a challenge quest cleared within its `good_time_sec`, as a share of what its
/// sectors paid (*inferred*: the data only gives the recommended time).
const GOOD_TIME_BONUS: f32 = 0.25;
/// Faces of the quest die: every `scenario.xml` scenario has one `<MAP dice>` for each of 1..=6
/// (**observed**).
const DICE_SIDES: u32 = 6;
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

/// `<SECTOR id title melee_spawn range_spawn>`; the map is the directory `title` (lower case)
/// under `quest/maps/`. `melee_spawn`/`range_spawn` (**observed**: 15 in every sector) are taken
/// as how many NPCs of that kind may live at once (*inferred* meaning).
#[derive(Clone, Debug, PartialEq)]
pub struct Sector {
    pub id: u32,
    pub title: String,
    pub melee_spawn: u32,
    pub range_spawn: u32,
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
    /// `<SACRI_ITEM itemid>` of a special scenario: the two items it costs.
    pub sacri: Vec<u32>,
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
    /// `good_time_sec`: the recommended clear time, seconds.
    pub good_secs: u32,
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
                    melee_spawn: num(s, "melee_spawn"),
                    range_spawn: num(s, "range_spawn"),
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
            sacri: s
                .children()
                .filter(|n| n.has_tag_name("SACRI_ITEM"))
                .map(|n| num(n, "itemid"))
                .collect(),
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
            good_secs: num(s, "good_time_sec"),
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

/// One `<ITEM>` of `sacrificetable.xml` (**observed**): at quest level `ql` the page
/// `default_item` opens a standard quest, the `special` items draw the boss `npc`.
#[derive(Clone, Debug, PartialEq)]
pub struct Sacrifice {
    pub ql: u32,
    pub default_item: u32,
    pub special: [u32; 2],
    pub npc: String,
}

pub fn parse_sacrifice(xml: &str) -> Result<Vec<Sacrifice>, String> {
    Ok(doc(xml)?
        .descendants()
        .filter(|n| n.has_tag_name("ITEM"))
        .map(|n| Sacrifice {
            ql: num(n, "ql"),
            default_item: num(n, "default_item_id"),
            special: [num(n, "special_item_id1"), num(n, "special_item_id2")],
            npc: text(n, "significant_npc"),
        })
        .collect())
}

/// One `<ITEM>` of `zquestitem.xml`.
#[derive(Clone, Debug, PartialEq)]
pub struct QItem {
    pub id: u32,
    pub name: String,
    /// The English `QITEM_DESC_<id>`; empty where the data has Korean only.
    pub desc: String,
    pub kind: String,
    /// `level` (**observed**: 5/10/15/20 on the four pages, else 0).
    pub level: u32,
    pub price: u32,
    /// `secrifice="1"`: may go into a sacrifice slot.
    pub sacrifice: bool,
}

/// Quest items by id.
#[derive(Clone, Default, Debug)]
pub struct QItems(pub BTreeMap<u32, QItem>);

/// English names (**inferred** translations) of the quest items whose `QITEM_NAME_<id>` is Korean
/// in `strings.xml` and in all ten locale directories (none has an English name for them), plus
/// 210001, which has no string at all (named after its `monbible` type).
const KOREAN_NAMES: [(u32, &str); 20] = [
    (200005, "Small Skull"),
    (200006, "Large Skull"),
    (200007, "Mysterious Skull"),
    (200010, "Giant Remains"),
    (200019, "Skeleton Doll"),
    (200023, "Rabbit Doll"),
    (200024, "Teddy Bear"),
    (200025, "Cursed Teddy Bear"),
    (200028, "Devil's Dictionary"),
    (200029, "Scryder's Roster, Part 1"),
    (200030, "Scryder's Roster, Part 2"),
    (200031, "Blessed Cross"),
    (200032, "Cursed Cross"),
    (200034, "Talking Pebble"),
    (200035, "Ice Crystal"),
    (200040, "Superion's Sword"),
    (200041, "Aneramon's Sword"),
    (200042, "Lich's Tail"),
    (200043, "Pampow's Ice Sword"),
    (210001, "Monster Bible"),
];

impl QItems {
    pub fn parse(items: &str, strings: &str) -> Result<Self, String> {
        let strs: HashMap<String, String> = doc(strings)?
            .descendants()
            .filter(|n| n.has_tag_name("STR"))
            .filter_map(|n| Some((n.attribute("id")?.to_owned(), n.text()?.trim().to_owned())))
            .collect();
        Ok(Self(
            doc(items)?
                .descendants()
                .filter(|n| n.has_tag_name("ITEM"))
                .map(|n| {
                    let id = num(n, "id");
                    // the HUD font has no Hangul: only ASCII strings are used
                    let ascii = |k: &str| strs.get(&format!("{k}_{id}")).filter(|v| v.is_ascii());
                    let name = ascii("QITEM_NAME")
                        .cloned()
                        .or_else(|| {
                            let k = KOREAN_NAMES.iter().find(|k| k.0 == id)?;
                            Some(k.1.to_owned())
                        })
                        .unwrap_or_else(|| format!("item {id}"));
                    let item = QItem {
                        id,
                        name,
                        desc: ascii("QITEM_DESC").cloned().unwrap_or_default(),
                        kind: text(n, "type"),
                        level: num(n, "level"),
                        price: num(n, "price"),
                        sacrifice: num(n, "secrifice") == 1,
                    };
                    (id, item)
                })
                .collect(),
        ))
    }

    pub fn name(&self, id: u32) -> String {
        self.0
            .get(&id)
            .map_or_else(|| format!("item {id}"), |i| i.name.clone())
    }
}

/// `droptable.xml`: set name -> quest level -> (item id, rate, `rent_period` in hours or 0 for a
/// permanent item). Sets listed twice under one name (`G181`) are merged.
#[derive(Default, Debug)]
pub struct Drops(HashMap<String, HashMap<u32, Vec<Drop3>>>);

/// One `<ITEM>` of a drop set: id, rate, rent hours.
type Drop3 = (String, f32, u32);

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
                let rent = i
                    .attribute("rent_period")
                    .and_then(|r| r.parse().ok())
                    .unwrap_or(0);
                items.push((text(i, "id"), rate, rent));
            }
        }
    }
    Ok(out)
}

impl Drops {
    /// The item `table` drops at quest level `ql` for a uniform roll `r` in 0..1: the item rates
    /// of a set add up to at most 1 (observed, all 147 sets), so one roll walks the cumulative
    /// rates and falls through to "nothing". The level's own set, else the nearest lower one.
    pub fn roll(&self, table: &str, ql: u32, r: f32) -> Option<(&str, u32)> {
        let levels = self.0.get(table)?;
        let items = (0..=ql)
            .rev()
            .find_map(|q| levels.get(&q))
            .or_else(|| levels.values().next())?;
        let mut acc = 0.0;
        items
            .iter()
            .find(|(_, rate, _)| {
                acc += rate;
                r < acc
            })
            .map(|(id, _, rent)| (id.as_str(), *rent))
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
    sacrifice: Vec<Sacrifice>,
    pub items: QItems,
}

/// What a scenario costs and what [`Catalog::admit`] lets through.
#[derive(Debug, PartialEq)]
pub struct Admit {
    /// The scenario that will be played (a sacrifice pair switches to its special scenario).
    pub scenario: String,
    /// Quest items the start consumes.
    pub spend: Vec<u32>,
}

impl Catalog {
    pub fn load(vfs: &crate::mrs::Vfs) -> Result<Self, String> {
        let read = |p: &str| {
            vfs.read(p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .map_err(|e| format!("{p}: {e}"))
        };
        let mut c = Self::parse(
            &read("system/questmap.xml")?,
            &read("system/survivalmap.xml")?,
            &read("system/scenario.xml")?,
            &read("system/scenario2.xml")?,
            &read("system/npc.xml")?,
            &read("system/sacrificetable.xml")?,
        )?;
        c.items = QItems::parse(
            &read("system/zquestitem.xml")?,
            &read("system/strings.xml")?,
        )?;
        Ok(c)
    }

    pub fn parse(
        questmap: &str,
        survival: &str,
        scenario: &str,
        scenario2: &str,
        npc: &str,
        sacrifice: &str,
    ) -> Result<Self, String> {
        Ok(Self {
            mapsets: parse_mapsets(questmap)?,
            survival: parse_mapsets(survival)?,
            scenarios: parse_scenarios(scenario)?,
            challenges: parse_challenges(scenario2)?,
            npcs: parse_npcs(npc)?,
            sacrifice: parse_sacrifice(sacrifice)?,
            items: QItems::default(),
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
            .map(|m| format!("Survival {}", m.title));
        std.chain(ch).chain(surv).collect()
    }

    fn find(&self, name: &str) -> Option<Found<'_>> {
        let want = name.trim().to_ascii_lowercase();
        let by_name = |t: &str| t.to_ascii_lowercase() == want;
        let id = Some(want.clone());
        self.scenarios
            .iter()
            .find(|s| by_name(&s.title) || s.id.map(|i| i.to_string()) == id)
            .map(Found::Scenario)
            .or_else(|| {
                let c = self.challenges.iter();
                let mut c = c.filter(|c| {
                    by_name(&format!("Challenge {}", c.id)) || c.id.to_string() == want
                });
                c.next().map(Found::Challenge)
            })
            .or_else(|| {
                let mut m = self.survival.iter();
                let m = m.find(|m| by_name(&format!("Survival {}", m.title)));
                m.map(Found::Survival)
            })
    }

    /// The plan of scenario `name`. `dice` picks a `<MAP dice>` of a `scenario.xml` scenario
    /// (`None`: roll it); `seed` drives the roll and the random NPC picks.
    pub fn plan(&self, name: &str, dice: Option<u32>, seed: u32) -> Result<Plan, String> {
        match self.find(name) {
            Some(Found::Scenario(def)) => self.plan_standard(def, dice, seed),
            Some(Found::Challenge(c)) => Ok(Self::plan_challenge(c)),
            Some(Found::Survival(m)) => self.plan_survival(m, seed),
            None => Err(format!(
                "unknown scenario {name:?}; known: {}",
                self.names().join(", ")
            )),
        }
    }

    /// The special scenario whose two `SACRI_ITEM`s are exactly the items in `slots`.
    pub fn special_for(&self, slots: &[u32]) -> Option<String> {
        let sorted = |v: &[u32]| {
            let mut v: Vec<u32> = v.iter().copied().filter(|&i| i != 0).collect();
            v.sort();
            v
        };
        let slots = sorted(slots);
        self.scenarios
            .iter()
            .find(|d| d.id.is_some() && sorted(&d.sacri) == slots)
            .map(|d| d.title.clone())
    }

    /// The boss `sacrificetable.xml` says a special item draws (`significant_npc`).
    pub fn draws(&self, item: u32) -> Option<&str> {
        let row = self.sacrifice.iter().find(|r| r.special.contains(&item));
        row.map(|r| r.npc.as_str())
    }

    /// May `name` (`None`: the first scenario) start with the quest items in the two sacrifice
    /// slots `sac` (0 = empty), out of the player's `have`, at character `level`?
    ///
    /// Rules: a slot pair that is a special scenario's two `SACRI_ITEM`s (**observed**,
    /// `scenario.xml`) switches to that scenario; a standard quest of level n >= 2 costs the
    /// `default_item_id` of the `sacrificetable.xml` row of level n (the Torn Pages; levels 0 and 1
    /// are free, **observed**: their `default_item_id` is 0); a page needs its `level` (5/10/15/20)
    /// and a challenge quest its `level_limit` (*inferred*: as the character level). Only
    /// `secrifice="1"` items go into a slot. The start spends the items (*inferred*).
    pub fn admit(
        &self,
        name: Option<&str>,
        sac: [u32; 2],
        have: &BTreeMap<u32, u32>,
        level: u32,
    ) -> Result<Admit, String> {
        let slots: Vec<u32> = sac.into_iter().filter(|&i| i != 0).collect();
        for &id in &slots {
            let want = slots.iter().filter(|&&s| s == id).count() as u32;
            if !self.items.0.get(&id).is_some_and(|i| i.sacrifice) {
                return Err(format!("{} cannot be sacrificed", self.items.name(id)));
            }
            if have.get(&id).copied().unwrap_or(0) < want {
                return Err(format!("you do not have {} x{want}", self.items.name(id)));
            }
        }
        let title = match (self.special_for(&slots), name) {
            (Some(t), _) => t,
            (None, Some(n)) => n.to_owned(),
            (None, None) => self.names().swap_remove(0),
        };
        let (items, need_level) = match self.find(&title) {
            Some(Found::Scenario(s)) if s.id.is_some() => (s.sacri.clone(), 0),
            Some(Found::Scenario(s)) => {
                let page = self
                    .sacrifice
                    .iter()
                    .find(|r| r.ql == s.ql && r.default_item != 0);
                let page = page.map_or(0, |r| r.default_item);
                let level = self.items.0.get(&page).map_or(0, |i| i.level);
                (if page == 0 { vec![] } else { vec![page] }, level)
            }
            Some(Found::Challenge(c)) => (vec![], c.level),
            Some(Found::Survival(_)) => (vec![], 0),
            None => return Err(format!("unknown scenario {title:?}")),
        };
        let mut left = slots;
        for it in &items {
            let at = left.iter().position(|s| s == it);
            let Some(at) = at else {
                let names: Vec<String> = items.iter().map(|&i| self.items.name(i)).collect();
                return Err(format!("{title} needs {}", names.join(" + ")));
            };
            left.remove(at);
        }
        if level < need_level {
            return Err(format!("{title} needs level {need_level}"));
        }
        Ok(Admit {
            scenario: title,
            spend: items,
        })
    }

    fn plan_standard(
        &self,
        def: &ScenarioDef,
        dice: Option<u32>,
        seed: u32,
    ) -> Result<Plan, String> {
        let mut rng = seed | 1;
        let map = match dice {
            Some(d) => def
                .maps
                .iter()
                .find(|m| m.dice == d)
                .ok_or(format!("{}: no dice {d}", def.title))?,
            None if def.maps.is_empty() => return Err(format!("{}: no maps", def.title)),
            // the roll: every retail scenario has one `<MAP>` per face 1..=6 of the die
            None => {
                // small seeds give alike first xorshift outputs: scramble first
                let mut roll = seed.wrapping_mul(0x9E37_79B1) | 1;
                next(&mut roll);
                &def.maps[next(&mut roll) as usize % def.maps.len()]
            }
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
                    caps: (set.sectors[s].melee_spawn, set.sectors[s].range_spawn),
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
            dice: map.dice,
            good_secs: 0,
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
                // `challengequest` has no `melee_spawn`/`range_spawn`: no cap
                caps: (u32::MAX, u32::MAX),
            })
            .collect();
        Plan {
            name: format!("Challenge {}", c.id),
            xp: 0,
            bp: 0,
            ql: 0,
            players: c.players.max(1),
            reward_item: c.reward_item,
            dice: 0,
            good_secs: c.good_secs,
            sectors,
        }
    }

    /// Survival: the loop of the survival map set (each sector's first link leads on), played
    /// [`SURVIVAL_SECTORS`] times with the NPC sets of standard quest level 1, 2, ... 5. A map
    /// set with no standard quests of its own (Dungeon) gets [`skeleton_sets`] and the XP/BP of
    /// the first map set's quest of that level (no Dungeon XP/BP exists in the data).
    fn plan_survival(&self, m: &MapSet, seed: u32) -> Result<Plan, String> {
        let mut rng = seed | 1;
        let mut at = 0;
        let mut sectors = Vec::new();
        for i in 0..SURVIVAL_SECTORS {
            let ql = (1 + i as u32 / 2).min(5);
            let std = |own: bool| {
                let mut quests = self.scenarios.iter();
                quests.find(|s| s.id.is_none() && s.ql == ql && (!own || s.mapset == m.title))
            };
            let def = std(true).or_else(|| std(false));
            let def = def.ok_or(format!("no standard quest level {ql} for {}", m.title))?;
            let sets = if def.mapset == m.title {
                def.maps[0].sets.clone()
            } else {
                skeleton_sets(ql)
            };
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
                groups: self.groups(&sets, ql, None, &mut rng),
                jaco: None,
                caps: (m.sectors[at].melee_spawn, m.sectors[at].range_spawn),
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
            dice: 0,
            good_secs: 0,
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

enum Found<'a> {
    Scenario(&'a ScenarioDef),
    Challenge(&'a Challenge),
    Survival(&'a MapSet),
}

/// NPC sets of Survival Dungeon: the skeleton family `S`, ids 31..=35. Evidence: the family tens digit
/// is the `MAPSET id` (`G` 1x = Mansion 1, `K` 2x = Prison 2 in `scenario.xml`; `survivalmap.xml`'s
/// Dungeon is mapset 3, **observed**), no scenario uses ids 31..39 and `strings.xml` calls 31..35 the
/// undead clan's soldiers/mages/captain/giant/corpse (`NPC_NAME_31..35`, **observed**); 36 `Lich Pawn`
/// is "spawned from the Lich" (`NPC_DESC_36`), a boss minion that no wave lists, and 37..39 are bosses.
/// **External**: the public Dungeon Quest page lists Skeleton, Mage, Knight, Champion, Wizard as the
/// wave monsters (https://gunz.fandom.com/wiki/Dungeon_Quest). Members per level (3, 4, then all 5 from
/// level 2) copy the goblin sets `G0x`/`G1x`/`G2x` (**inferred**: the sets themselves are server-side).
fn skeleton_sets(ql: u32) -> Vec<String> {
    (1..=(3 + ql).min(5)).map(|n| format!("S{ql}{n}")).collect()
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
    /// Most NPCs of the kind (melee, ranged) alive at once: the sector's `melee_spawn` /
    /// `range_spawn`.
    pub caps: (u32, u32),
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
    /// Item granted on a cleared challenge quest (a `scenario2.xml` `reward_item`, 0 = none). The 3000xxx
    /// ids are defined nowhere in the client data (see docs/formats.md, Quest), so it is kept as given.
    pub reward_item: u32,
    /// The `<MAP dice>` rolled or chosen (0: the quest has no dice).
    pub dice: u32,
    /// Recommended clear time, seconds (challenge quest; 0: none).
    pub good_secs: u32,
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
    /// Quest item names and the NPCs that shoot (counted against `Stage::caps`).
    items: QItems,
    ranged: HashSet<String>,
    /// World item amounts.
    world: HashMap<String, (ItemKind, u32)>,
    stage: usize,
    phase: Phase,
    t: f32,
    queue: Vec<Pending>,
    since_spawn: f32,
    jaco_t: f32,
    kills: u32,
    loot: Vec<(u32, u32)>,
    /// Rental drops (shop item id, `rent_period` hours), one entry per pickup.
    rented: Vec<(u32, u32)>,
    swap: bool,
    rng: u32,
    banner: String,
    /// Player plus allied bots (challenge HP balancing).
    party: u32,
    /// Seconds the player has been dead.
    dead_t: f32,
    /// Seconds since the quest began (against `Plan::good_secs`).
    elapsed: f32,
}

impl Quest {
    pub fn new(
        vfs: &crate::mrs::Vfs,
        cat: &Catalog,
        plan: Plan,
        bots: usize,
        seed: u32,
    ) -> Result<Self, String> {
        let read = |p: &str| {
            vfs.read(p)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .map_err(|e| format!("{p}: {e}"))
        };
        if plan.dice > 0 {
            println!(
                "quest: dice roll {} of {DICE_SIDES}: {} over {} sectors",
                plan.dice,
                plan.name,
                plan.sectors.len()
            );
        }
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
            items: cat.items.clone(),
            ranged: cat.npcs.ranged.clone(),
            world,
            stage: 0,
            phase: Phase::Intro,
            t: 0.0,
            queue: Vec::new(),
            since_spawn: 0.0,
            jaco_t: 0.0,
            kills: 0,
            loot: Vec::new(),
            rented: Vec::new(),
            swap: false,
            rng: seed | 1,
            banner: String::new(),
            party: 1 + bots as u32,
            dead_t: 0.0,
            elapsed: 0.0,
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

    fn add_loot(&mut self, id: u32, rent: u32) {
        if rent > 0 {
            self.rented.push((id, rent));
            return;
        }
        match self.loot.iter_mut().find(|l| l.0 == id) {
            Some(l) => l.1 += 1,
            None => self.loot.push((id, 1)),
        }
    }

    fn item_name(&self, id: u32) -> String {
        self.items.name(id)
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
    /// `rent_period` hours of a rental drop, 0 = permanent.
    rent: u32,
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
            (
                allies,
                kills,
                drive,
                collect,
                banner,
                change_sector,
                wait_room,
            )
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
    if quest.plan.dice > 0 {
        clock.note += &format!("  DICE {}", quest.plan.dice);
    }
    if quest.plan.good_secs > 0 {
        clock.note += &format!(
            "  TIME {}/{}",
            mmss(quest.elapsed),
            mmss(quest.plan.good_secs as f32)
        );
    }
    let p = player.single().ok();
    if matches!(quest.phase, Phase::Won | Phase::Lost) {
        return;
    }
    quest.elapsed += dt;
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
            // a kind (melee, ranged) has at most the sector's `melee_spawn`/`range_spawn` alive
            let room = quest.queue.last().is_some_and(|p| {
                let ranged = quest.ranged.contains(&p.spawn.id);
                let alive = npcs
                    .iter()
                    .filter(|(n, dead)| !dead && quest.ranged.contains(&n.id) == ranged)
                    .count() as u32;
                let caps = quest.stage().caps;
                alive < if ranged { caps.1 } else { caps.0 }
            });
            if room && quest.since_spawn >= SPAWN_GAP {
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
        reward.write(Reward {
            xp,
            bounty: bp,
            ..default()
        });
    }
    println!(
        "quest: sector {} cleared at {} ({} kills, reward {xp} XP {bp} BP)",
        quest.stage + 1,
        mmss(quest.elapsed),
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
                ..default()
            });
        }
        if quest.plan.reward_item != 0 {
            let id = quest.plan.reward_item;
            quest.add_loot(id, 0);
        }
        let good = quest.plan.good_secs as f32;
        if good > 0.0 && quest.elapsed <= good {
            let sum = |f: fn(&Stage) -> u32| {
                quest.plan.sectors.iter().map(f).sum::<u32>() as f32 * GOOD_TIME_BONUS
            };
            let (xp, bounty) = (sum(|s| s.xp) as u32, sum(|s| s.bp) as u32);
            reward.write(Reward {
                xp,
                bounty,
                ..default()
            });
            println!(
                "quest: cleared in {} within the good time {}: bonus {xp} XP {bounty} BP",
                mmss(quest.elapsed),
                mmss(good)
            );
        }
    }
    loot.write(QuestLoot {
        items: quest.loot.clone(),
        rented: quest.rented.clone(),
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

fn mmss(secs: f32) -> String {
    format!("{}:{:02}", secs as u32 / 60, secs as u32 % 60)
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
        // tables not in `droptable.xml` (challenge `C1`, `C2`; skeleton `S31`..`S39`): hp, ap or ammo (*inferred*)
        let item = match quest.drops.0.contains_key(table) {
            true => quest
                .drops
                .roll(table, quest.plan.ql, r)
                .map(|(s, rent)| (s.to_owned(), rent)),
            false => ["hp1", "ap1", "mag1"]
                .get((r * 4.0) as usize)
                .map(|s| ((*s).to_owned(), 0)),
        };
        let Some((item, rent)) = item else { continue };
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
                        rent: 0,
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
                        rent,
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
        println!(
            "quest: {} dropped {item} at ({:.1}, {:.1}) t={:.0}",
            npc.id, at.x, at.z, quest.elapsed
        );
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
            quest.add_loot(id, d.rent);
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

/// During a sector's intro the player waits at the map's `wait_pos_NN` dummy (**observed**: one
/// per quest map, on a spot overlooking the hall, far from the `spawn_solo`s; the use is
/// *inferred* from the name) and drops to the first `spawn_solo` when the NPCs start to come.
/// A `--at` / `--yaw` start is left alone.
fn wait_room(
    quest: Res<Quest>,
    level: Res<Level>,
    setup: Option<Res<PlayerSetup>>,
    mut player: Query<(&mut Transform, &mut Intent, &mut Motor), With<Player>>,
    mut seen: Local<Option<(usize, Phase)>>,
) {
    let now = (quest.stage, quest.phase);
    if *seen == Some(now) || setup.is_some_and(|s| s.at.is_some() || s.yaw.is_some()) {
        return;
    }
    let Ok((mut tf, mut intent, mut motor)) = player.single_mut() else {
        return;
    };
    *seen = Some(now);
    let spot = match now.1 {
        Phase::Intro => level
            .map
            .dummies
            .iter()
            .find(|d| d.name.to_ascii_lowercase().starts_with("wait_pos"))
            .map(|d| {
                let pos = Vec3::from(to_bevy(d.pos)) * SCALE;
                (pos, yaw_of(Vec3::from(to_bevy(d.dir))))
            }),
        Phase::Fight => level.spawn_points().first().map(|&(p, d)| (p, yaw_of(d))),
        _ => None,
    };
    if let Some((pos, yaw)) = spot {
        tf.translation = pos + Vec3::Y * 0.1;
        intent.yaw = yaw;
        motor.vel = Vec3::ZERO;
    }
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
          <SACRI_ITEM itemid="200018" />
          <MAP dice="1" key_sector="102" key_npc="16" boss="true"><NPCSET_ARRAY>G61</NPCSET_ARRAY>
            <JACO count="2" tick="5" min_npc="0" max_npc="13"><NPC npcid="2011" rate="0.2" /><NPC npcid="2012" rate="0.3" /></JACO></MAP>
        </SPECIAL_SCENARIO></XML>"#;
    const CHALLENGE: &str = r#"<XML><SCENARIO map_id="101" name="UI_STAGE_101" reward_item="3000101" players="4" level_limit="01" good_time_sec="480">
        <SECTOR map="G_Easy_1" xp="200" bp="20"><SPAWN postag="1" num="8" actor="knifeman" drop="C1"/></SECTOR>
        <SECTOR map="G_Easy_6" xp="1500" bp="50"><SPAWN postag="boss" num="1" actor="robot" drop="" adjustplayernum="true"/></SECTOR>
        </SCENARIO></XML>"#;
    const NPC: &str = r#"<XML><NPC id="11" offensetype="1"/><NPC id="12" offensetype="2"/><NPC id="13" offensetype="1"/><NPC id="16" offensetype="1"/></XML>"#;
    const SACRIFICE: &str = r#"<XML>
        <ITEM map="" ql="1" default_item_id="0" special_item_id1="200008" special_item_id2="0" significant_npc="goblin chief" />
        <ITEM map="" ql="2" default_item_id="200001" special_item_id1="0" special_item_id2="0" significant_npc="" /></XML>"#;
    const QITEMS: &str = r#"<XML>
        <ITEM id="200001" type="page" level="5" price="100" secrifice="1"/>
        <ITEM id="200008" type="skull" level="0" price="40" secrifice="1"/>
        <ITEM id="200011" type="fresh" level="0" price="10" secrifice="0"/>
        <ITEM id="200018" type="necklace" level="0" price="100" secrifice="1"/>
        <ITEM id="200019" type="doll" level="0" price="200" secrifice="1"/></XML>"#;
    const STRINGS: &str = r#"<XML><STR id="QITEM_NAME_200001">Torn Page I</STR>
        <STR id="QITEM_NAME_200008">Goblin Skull</STR><STR id="QITEM_NAME_200011">Ore Fragment</STR>
        <STR id="QITEM_NAME_200018">Grimsk's Necklace</STR><STR id="QITEM_NAME_200019">스켈레톤 인형</STR></XML>"#;

    fn catalog() -> Catalog {
        let mut c = Catalog::parse(MAPS, MAPS, SCENARIO, CHALLENGE, NPC, SACRIFICE).unwrap();
        c.items = QItems::parse(QITEMS, STRINGS).unwrap();
        c
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
        assert_eq!(d.roll("G11", 0, 0.1), Some(("hp1", 0)));
        assert_eq!(d.roll("G11", 0, 0.4), Some(("200011", 0)));
        assert_eq!(d.roll("G11", 1, 0.9), None);
        assert_eq!(d.roll("G11", 5, 0.9), Some(("ap1", 0)));
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

    #[test]
    fn quest_item_names_are_english() {
        let c = catalog();
        assert_eq!(c.items.name(200008), "Goblin Skull");
        // Korean in the strings: the inferred table, else the id
        assert_eq!(c.items.name(200019), "Skeleton Doll");
        assert_eq!(c.items.name(999), "item 999");
    }

    #[test]
    fn sacrifice_unlocks_levels_and_special_scenarios() {
        let c = catalog();
        let have = BTreeMap::from([(200001, 1), (200008, 1), (200018, 1)]);
        // QL2 costs the page, and the page needs level 5
        let page = [200001, 0];
        let std = |sac, level| c.admit(Some("Quest Mansion QL2"), sac, &have, level);
        assert_eq!(std(page, 5).unwrap().spend, [200001]);
        assert_eq!(std(page, 4), Err("Quest Mansion QL2 needs level 5".into()));
        assert_eq!(
            std([0, 0], 9),
            Err("Quest Mansion QL2 needs Torn Page I".into())
        );
        // the special pair, in either order, switches the scenario
        let a = c.admit(Some("Quest Mansion QL2"), [200018, 200008], &have, 1);
        assert_eq!(a.unwrap().scenario, "Goblin King");
        // a lone skull is not the pair; the special scenario by name needs both
        let lone = c.admit(Some("Goblin King"), [200008, 0], &have, 1);
        assert_eq!(
            lone,
            Err("Goblin King needs Goblin Skull + Grimsk's Necklace".into())
        );
        assert_eq!(c.draws(200008), Some("goblin chief"));
        // items must be owned and sacrificable; the challenge needs its level
        assert!(c.admit(None, [200011, 0], &have, 1).is_err());
        assert!(c.admit(None, [200019, 0], &have, 1).is_err());
        assert!(c.admit(Some("Challenge 101"), [0, 0], &have, 1).is_ok());
    }

    #[test]
    fn dice_roll_picks_a_map_and_is_seeded() {
        let c = catalog();
        let name = "Quest Mansion QL2";
        let rolls: HashSet<u32> = (1..40)
            .map(|s| c.plan(name, None, s).unwrap().dice)
            .collect();
        assert_eq!(rolls, HashSet::from([1, 2]));
        let (a, b) = (
            c.plan(name, None, 5).unwrap(),
            c.plan(name, None, 5).unwrap(),
        );
        assert_eq!(a.dice, b.dice);
        assert_eq!(c.plan(name, Some(1), 5).unwrap().dice, 1);
        assert!(c.plan(name, Some(6), 5).is_err());
    }

    #[test]
    fn challenge_keeps_its_recommended_time_and_dungeon_survival_uses_skeletons() {
        let c = catalog();
        assert_eq!(c.plan("Challenge 101", None, 1).unwrap().good_secs, 480);
        assert_eq!(skeleton_sets(1), ["S11", "S12", "S13", "S14"]);
        assert_eq!(skeleton_sets(5), ["S51", "S52", "S53", "S54", "S55"]);
        assert_eq!(set_npc("S13").as_deref(), Some("33"));
    }

    /// With the retail extract (`.local/extract`): every scenario plans, every sector map exists.
    #[test]
    fn retail_scenarios_plan() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(".local/extract");
        let read = |p: &str| std::fs::read_to_string(dir.join(p));
        let (Ok(a), Ok(b), Ok(c), Ok(d), Ok(e), Ok(f), Ok(g), Ok(h)) = (
            read("system/questmap.xml"),
            read("system/survivalmap.xml"),
            read("system/scenario.xml"),
            read("system/scenario2.xml"),
            read("system/npc.xml"),
            read("system/sacrificetable.xml"),
            read("system/zquestitem.xml"),
            read("system/strings.xml"),
        ) else {
            return;
        };
        let mut cat = Catalog::parse(&a, &b, &c, &d, &e, &f).unwrap();
        cat.items = QItems::parse(&g, &h).unwrap();
        // all 45 + 1 quest items have an English name
        assert!(
            cat.items
                .0
                .values()
                .all(|i| i.name.is_ascii() && !i.name.starts_with("item "))
        );
        // the Goblin King's two items open it
        let have = BTreeMap::from([(200008, 1), (200018, 1)]);
        let king = cat.admit(None, [200008, 200018], &have, 1).unwrap();
        assert_eq!(
            (king.scenario.as_str(), king.spend.len()),
            ("Goblin King", 2)
        );
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
            if n == "Survival Dungeon" {
                let ids = p
                    .sectors
                    .iter()
                    .flat_map(|s| &s.groups)
                    .map(|g| g.id.as_str());
                for id in ids {
                    assert!(
                        ["31", "32", "33", "34", "35"].contains(&id),
                        "Lich Pawn or boss: {id}"
                    );
                }
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
            dice: 0,
            good_secs: 0,
            sectors: vec![],
        };
        let quest = Quest {
            plan,
            drops: Drops::default(),
            items: QItems::parse(QITEMS, STRINGS).unwrap(),
            ranged: HashSet::new(),
            world: HashMap::new(),
            stage: 0,
            phase: Phase::Fight,
            t: 0.0,
            queue: vec![],
            since_spawn: 0.0,
            jaco_t: 0.0,
            kills: 0,
            loot: vec![],
            rented: vec![],
            swap: false,
            rng: 1,
            banner: String::new(),
            party: 1,
            dead_t: 0.0,
            elapsed: 0.0,
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
                rent: 0,
                base: 0.5,
            },
            near(0.3),
        ));
        app.world_mut().spawn((
            Drop {
                id: Some(200012),
                rent: 0,
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
                rent: 0,
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
