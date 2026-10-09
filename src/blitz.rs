//! Blitzkrieg (`--mode blitzkrieg`, retail id 13, map `blitzkrieg`): two sides, each with a radar
//! that sends waves of soldiers down the `route_*` lanes, twelve barricades, a guardian and the
//! players and bots in between. `system/blitzkrieg.xml` is the rule book ([`Cfg`]); the soldiers,
//! radars and barricades are `npc2.xml` actors driven by `aifsm.xml` through `npc.rs` (they march
//! along [`Routes`], fight whatever is hostile and a radar's machine summons the waves). This
//! module places the objectives, pays and spends honor, applies the building rules, calls the
//! reinforcements and ends the match. Rules *inferred* rather than read are marked; the long form
//! is `docs/formats.md` "Blitzkrieg".
//!
//! At the start of the match (`CLASS_SELECT_TIME`) the player picks one of the nine classes of
//! `CLASS_TABLE` (six have a `CLASS_BOOK` item, three do not); bots pick for themselves. The
//! screens (class select, minimap, reward) are `blitz/ui.rs`.
//!
//! Controls: `F` opens the upgrade panel, Up/Down choose, Enter buys (the honor is spent per step
//! of [`Cfg::up`]); bots buy by themselves. Headless checks: `GUNZ_BLITZ_BUY="SECS:N,.."` buys
//! upgrade N (1-6) for the player at match second SECS, `GUNZ_BLITZ_HP=K` scales the objectives'
//! health, `GUNZ_BLITZ_CLASS=N` (1-9) picks the player's class without the screen,
//! `GUNZ_BLITZ_SELECT=1` shows the class screen in a `--shot` run (which otherwise skips it, with
//! no class and the default weapons) and `GUNZ_BLITZ_SKIP=SECS` starts the match SECS seconds
//! in (clock, honor income and soldier enhancement), to reach the reward's minimum time.

mod ui;

use crate::{
    actor::{ActorData, DEFAULT_LOADOUT, PlayerSetup},
    bot::BotAhead,
    col::MapCollision,
    combat::{Vfx, is_melee, rnd, yaw_of},
    game::{
        Afflict, Bot, Damage, Dead, Equip, Frozen, Hold, Killed, Loadout, Mods, Npc, NpcState,
        PlaySound, Player, Reward, Routes, Score, SpawnNpc, Team, Vitals,
    },
    item::{Items, WeaponKind},
    level::Level,
    menu::Mode,
    session::{Clock, PROTECT_SECS, RESPAWN_SECS, Rules, freeze},
    view::{SCALE, Shot, to_bevy},
};
use bevy::{
    prelude::*,
    ui::{GlobalZIndex, UiTargetCamera},
};
use roxmltree::{Document, Node as Xml};
use std::collections::{HashMap, VecDeque};

/// Seconds a banner (a purchase, an honor gain) stays up. *Inferred* from the
/// `EVENT_MESSAGE viewTime="4"`.
const BANNER_SECS: f32 = 4.0;
/// Seconds a fire enchant burns (`UPGRADE fireDamageDuration`, observed 4.0).
const FIRE_SECS: f32 = 4.0;
/// Vertical reach (m) of a radar's or barricade's zone. *Inferred* (the file gives a distance).
const ZONE_HEIGHT: f32 = 6.0;
/// What a bot buys first, as indices into [`UPGRADES`] (*inferred*: toughness, then damage).
const BOT_ORDER: [usize; 6] = [2, 0, 1, 4, 3, 5];
/// Most players of one side that may share a class (message 2116 "You cannot select more than 3
/// of the same classes"; the Korean text reads "3 or more", so the limit is 3 or 2).
const SAME_CLASS: u8 = 3;
/// The nine classes: name, `CLASS_TABLE` element and `CLASS_BOOK` attribute (`None`: the table
/// has the class but the book does not). Elements and attributes are **observed**; the names are
/// the keys capitalised, no string in any locale names a class (the data has no class text).
/// Hunter, Slaughter and Trickster come last: their book item does not exist (message 2115 says
/// a class needs a book), so retail may never have let a player pick them; they are playable
/// here because the table gives them stats.
const CLASSES: [(&str, &str, Option<&str>); 9] = [
    ("Gladiator", "GLADIATOR", Some("gladiator")),
    ("Duelist", "DUELIST", Some("duelist")),
    ("Incinerator", "INCINERATOR", Some("incinerator")),
    ("Combat Officer", "COMBATOFFICER", Some("combatofficer")),
    ("Assassin", "ASSASSIN", Some("assassin")),
    ("Terrorist", "TERRORIST", Some("terrorist")),
    ("Hunter", "HUNTER", None),
    ("Slaughter", "SLAUGHTER", None),
    ("Trickster", "TRICKSTER", None),
];
/// Each class's weapons: a blade and a gun, the first of the two selected. Public sources name no
/// class loadout (none of the retail data, the Steam / wiki texts or the forum threads does; see
/// `docs/formats.md`), and `WEAPON` scales every weapon type, so retail probably let players
/// bring their own. These are therefore all *inferred*, from the stats only: the Gladiator's
/// melee bonus (blade first), the Duelist's shotgun bonuses (shotgun, **observed** by the
/// attribute names), the Incinerator's fire (the data's "Incinerator" machine gun, item
/// 2110008), the Terrorist's building bonus (rocket), the Slaughter's fire and magazines
/// (SMG) and the Trickster's support (pistol); the rest by their names.
const KITS: [(WeaponKind, WeaponKind); CLASSES.len()] = [
    (WeaponKind::Katana, WeaponKind::Revolver),
    (WeaponKind::Dagger, WeaponKind::Shotgun),
    (WeaponKind::Katana, WeaponKind::MachineGun),
    (WeaponKind::Katana, WeaponKind::Rifle),
    (WeaponKind::Dagger, WeaponKind::Smg),
    (WeaponKind::Katana, WeaponKind::Rocket),
    (WeaponKind::Dagger, WeaponKind::Rifle),
    (WeaponKind::Katana, WeaponKind::Smg),
    (WeaponKind::Dagger, WeaponKind::Pistol),
];
const INCINERATOR: u32 = 2110008;
/// Honor gains below / above these get the less / more effect and sound (*inferred*: the effects
/// are named Less / Legular / More, `HONOR_LIST` pays 5..150).
const GAIN_LESS: f32 = 30.0;
const GAIN_MORE: f32 = 100.0;
/// Index of the Combat Officer in [`CLASSES`].
const OFFICER: usize = 3;
/// Soldiers of `npc2.xml` (`type`): what the radars send and the terminator.
const SOLDIERS: [&str; 6] = [
    "knifeman",
    "throwman",
    "zealot",
    "cleric",
    "knight",
    "terminator",
];
/// Buff effects of the radar, barricade and combat-officer zones (`effect_list.xml`: looped
/// models) and the honor crate's; the loops are re-spawned every [`AURA_SECS`] while the player
/// stands inside (*inferred*: the effect list has no duration).
const AURAS: [&str; 3] = [
    "ef_Blitz_RadarBuff",
    "ef_Blitz_BarricadeBuff",
    "ef_Blitz_CombatOfficerBuff",
];
const HONOR_ITEM_FX: &str = "ef_Blitz_HonorItem";
const AURA_SECS: f32 = 1.5;

pub struct BlitzPlugin;

impl Plugin for BlitzPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup.run_if(on)).add_systems(
            Update,
            (
                classes,
                tag,
                income,
                scoring,
                input,
                bots_buy,
                buffs,
                faster_respawn,
                rearm,
                zones,
                reinforce,
                events,
                helps,
                crates,
                finish,
                feedback,
                hud,
                ui::select_ui,
                ui::minimap,
                ui::reward_ui,
            )
                .chain()
                .run_if(live),
        );
    }
}
fn on(rules: Option<Res<Rules>>) -> bool {
    rules.is_some_and(|r| r.mode == Mode::Blitzkrieg)
}

fn live(rules: Option<Res<Rules>>, blitz: Option<Res<Blitz>>) -> bool {
    on(rules) && blitz.is_some()
}

// ---------------------------------------------------------------------------------------------
// system/blitzkrieg.xml

/// A building's zone (`BUILDING/BARRICADE` and `/RADAR`): lengths in metres.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Zone {
    pub dist: f32,
    /// Damage a player takes inside, as a fraction removed (barricade).
    pub reduce: f32,
    /// Health and armour restored per tick as a fraction of the maximum (radar).
    pub ap_hp: f32,
    /// Ammunition restored per tick as a fraction of the maximum.
    pub mag: f32,
    pub delay: f32,
}

/// `REINFORCE`: when a side is down to `barricades`, its radar (`alliance`) or the enemy's
/// switches to machine state `state`.
#[derive(Clone, Debug, PartialEq)]
pub struct Reinforce {
    pub barricades: u32,
    pub state: String,
    pub alliance: bool,
}

/// `UPGRADE`: four steps per attribute.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Upgrades {
    pub cost: [f32; 4],
    pub dps: [f32; 4],
    pub delay: [f32; 4],
    pub hp_ap: [f32; 4],
    pub fire: [f32; 4],
    pub magazine: [f32; 4],
    pub revive: [f32; 4],
}

/// `REWARD`: what a finished match pays (see [`payout`]).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RewardCfg {
    pub bounty: f32,
    pub exp: f32,
    /// Match seconds and honor a player needs to be paid at all.
    pub min_time: f32,
    pub min_honor: f32,
    pub win_medal: f32,
    pub lose_medal: f32,
    pub minute_medal: f32,
    pub minute_medal_max: f32,
    /// The MVP's extra share of (XP, bounty, medals) on the winning / losing side.
    pub mvp_win: [f32; 3],
    pub mvp_lose: [f32; 3],
}

/// `EVENT_MESSAGE` and `HELP_MESSAGE`: seconds a message stays, the pause when another waits,
/// the radar-attack cooldown, the help distance (m) and honor, and the sounds.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Messages {
    pub view: f32,
    pub delay: f32,
    pub radar_cooldown: f32,
    pub benefit: String,
    pub loss: String,
    pub help_view: f32,
    pub help_dist: f32,
    pub help_honor: f32,
    pub help: String,
}

/// The parts of `system/blitzkrieg.xml` this mode plays by (only the quit penalty is not
/// modelled).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Cfg {
    pub start_honor: f32,
    pub income_secs: f32,
    pub income: f32,
    pub first_kill: f32,
    pub respawn: f32,
    pub invincible: f32,
    pub finish_delay: f32,
    pub enhance_hp_ap: f32,
    pub enhance_dps: f32,
    pub npc_next: f32,
    pub npc_max: f32,
    pub npc_ratio: f32,
    /// Share of a player's damage a building takes (1 - `reduceDamageRatioFromPlayer`).
    pub building_takes: f32,
    pub barricade: Zone,
    pub radar: Zone,
    pub crate_respawn: f32,
    pub crates: Vec<String>,
    pub reinforce: Vec<Reinforce>,
    pub up: Upgrades,
    /// `WEAPON`: kind -> (DPS factor, shot delay in ms).
    pub weapon: HashMap<String, (f32, f32)>,
    /// `PLYAER_HONOR` (sic): kill, assist, their total-honor divisors, credit windows.
    pub kill: [f32; 6],
    /// `HONOR actorType`: kind -> (single, all).
    pub by_type: HashMap<String, (f32, f32)>,
    /// `SPAWN`: actor, count, team (2 red, 3 blue).
    pub spawns: Vec<(String, usize, u32)>,
    /// `ROUTE`: id -> the dummy names, in order.
    pub routes: Vec<(u32, Vec<String>)>,
    /// `CLASS_SELECT_TIME`: seconds the class screen lasts.
    pub class_select: f32,
    /// `LEAVE_AUTO_INC_HONOR`: the income per interval with 3, 2, 1 players left on a side.
    pub leave: Vec<f32>,
    /// `CLASS_TABLE`: element -> attributes.
    pub class: HashMap<String, HashMap<String, f32>>,
    /// `CLASS_BOOK`: class -> the book item id.
    pub book: Vec<(String, u32)>,
    pub reward: RewardCfg,
    pub msg: Messages,
}

impl Cfg {
    /// `CLASS_TABLE` value `attr` of class `class` (index into [`CLASSES`]); 0 without a class.
    fn class_val(&self, class: Option<usize>, attr: &str) -> f32 {
        class
            .and_then(|c| self.class.get(CLASSES[c].1)?.get(attr))
            .copied()
            .unwrap_or(0.0)
    }

    /// Honor a side gains per `income_secs` with `players` players: `LEAVE_AUTO_INC_HONOR` lists
    /// 3, 2 and 1 remaining.
    fn income_for(&self, players: usize) -> f32 {
        match players {
            1..=3 => self.leave.get(3 - players).copied().unwrap_or(self.income),
            _ => self.income,
        }
    }
}

fn attr(n: Xml, a: &str) -> f32 {
    n.attribute(a)
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0.0)
}

/// `"a:b:c:d"` of four numbers.
fn steps(n: Xml, a: &str) -> [f32; 4] {
    let mut out = [0.0; 4];
    for (o, v) in out.iter_mut().zip(n.attribute(a).unwrap_or("").split(':')) {
        *o = v.trim().parse().unwrap_or(0.0);
    }
    out
}

pub fn parse(xml: &str) -> Result<Cfg, String> {
    let doc = Document::parse(xml).map_err(|e| format!("blitzkrieg.xml: {e}"))?;
    let tag = |t: &str| {
        doc.descendants()
            .find(|n| n.has_tag_name(t))
            .ok_or(format!("blitzkrieg.xml: no <{t}>"))
    };
    let kids = |t: &str| -> Result<Vec<Xml>, String> {
        Ok(tag(t)?.children().filter(|c| c.is_element()).collect())
    };
    // The first <HONOR> is the event's start / income block; HONOR_LIST has its own.
    let honor = tag("HONOR")?;
    let (barricade, radar) = (tag("BARRICADE")?, tag("RADAR")?);
    let (enhance, npc) = (tag("ENHANCE_PLAYER")?, tag("ENHANCE_NPC")?);
    let (up, kill) = (tag("UPGRADE")?, tag("PLYAER_HONOR")?);
    let list = tag("HONOR_ITEM_LIST")?;
    let weapon = |t: &str| -> Result<Vec<(String, f32)>, String> {
        Ok(tag(t)?
            .attributes()
            .map(|a| (a.name().to_string(), a.value().parse().unwrap_or(0.0)))
            .collect())
    };
    let (dps, delay) = (weapon("DPS")?, weapon("DELAY")?);
    let by_type = kids("HONOR_LIST")?
        .into_iter()
        .filter_map(|n| {
            Some((
                n.attribute("actorType")?.to_string(),
                (attr(n, "single"), attr(n, "all")),
            ))
        })
        .collect();
    let routes = kids("ROUTE_LIST")?
        .into_iter()
        .map(|r| {
            let mut names: Vec<(u32, String)> = r
                .attributes()
                .filter_map(|a| {
                    Some((
                        a.name().strip_prefix("route")?.parse().ok()?,
                        a.value().to_string(),
                    ))
                })
                .collect();
            names.sort();
            (
                attr(r, "id") as u32,
                names.into_iter().map(|n| n.1).collect(),
            )
        })
        .collect();
    let (reward, event, help) = (tag("REWARD")?, tag("EVENT_MESSAGE")?, tag("HELP_MESSAGE")?);
    Ok(Cfg {
        start_honor: attr(honor, "startHonor"),
        income_secs: attr(honor, "autoIncHonorSec"),
        income: attr(honor, "autoIncHonor"),
        first_kill: attr(honor, "firstKillHonor"),
        respawn: attr(tag("RESPAWN")?, "baseTime"),
        invincible: attr(tag("RESPAWN")?, "invincibleTime"),
        finish_delay: attr(tag("FINISH_DELAY_TIME")?, "FinishDelayTime"),
        enhance_hp_ap: attr(enhance, "apHp"),
        enhance_dps: attr(enhance, "dps"),
        npc_next: attr(npc, "nextTime"),
        npc_max: attr(npc, "maxCount"),
        npc_ratio: attr(npc, "enhancedRatio"),
        building_takes: 1.0 - attr(tag("BUILDING")?, "reduceDamageRatioFromPlayer"),
        barricade: Zone {
            dist: attr(barricade, "dist") * 0.01,
            reduce: attr(barricade, "reduceDamageRatio"),
            mag: attr(barricade, "recoveryMagazineRatio"),
            delay: attr(barricade, "recoveryDelay"),
            ..default()
        },
        radar: Zone {
            dist: attr(radar, "dist") * 0.01,
            ap_hp: attr(radar, "recoveryApHpRatio"),
            mag: attr(radar, "recoveryMagazineRatio"),
            delay: attr(radar, "recoveryDelay"),
            ..default()
        },
        crate_respawn: attr(list, "respawnTime"),
        crates: list
            .children()
            .filter_map(|c| c.attribute("actor").map(str::to_string))
            .collect(),
        reinforce: kids("REINFORCE_LIST")?
            .into_iter()
            .map(|r| Reinforce {
                barricades: attr(r, "barricadeCount") as u32,
                state: r.attribute("state").unwrap_or_default().to_string(),
                alliance: r.attribute("alliance") == Some("true"),
            })
            .collect(),
        up: Upgrades {
            cost: steps(up, "requireHonor"),
            dps: steps(up, "dps"),
            delay: steps(up, "reduceShotDelayRatio"),
            hp_ap: steps(up, "hpAp"),
            fire: steps(up, "enchantFire"),
            magazine: steps(up, "magazineRatio"),
            revive: steps(up, "reviveRatio"),
        },
        weapon: dps
            .into_iter()
            .filter_map(|(k, f)| Some((k.clone(), (f, delay.iter().find(|d| d.0 == k)?.1))))
            .collect(),
        kill: [
            attr(kill, "single"),
            attr(kill, "assist"),
            attr(kill, "singleTotalDiv"),
            attr(kill, "assistTotalDiv"),
            attr(kill, "killDelay"),
            attr(kill, "assistDelay"),
        ],
        by_type,
        spawns: kids("SPAWN_LIST")?
            .into_iter()
            .map(|s| {
                (
                    s.attribute("actor").unwrap_or_default().to_string(),
                    attr(s, "num") as usize,
                    attr(s, "team") as u32,
                )
            })
            .collect(),
        routes,
        class_select: attr(tag("CLASS_SELECT_TIME")?, "ClassSelectTime"),
        leave: honor
            .children()
            .filter(|c| c.has_tag_name("LEAVE_AUTO_INC_HONOR"))
            .map(|c| attr(c, "autoIncHonor"))
            .collect(),
        class: kids("CLASS_TABLE")?
            .into_iter()
            .map(|c| {
                let vals = c
                    .attributes()
                    .map(|a| (a.name().to_string(), a.value().parse().unwrap_or(0.0)))
                    .collect();
                (c.tag_name().name().to_string(), vals)
            })
            .collect(),
        book: tag("CLASS_BOOK")?
            .attributes()
            .map(|a| (a.name().to_string(), a.value().parse().unwrap_or(0)))
            .collect(),
        reward: RewardCfg {
            bounty: attr(reward, "baseBounty"),
            exp: attr(reward, "baseExp"),
            min_time: attr(reward, "minTime"),
            min_honor: attr(reward, "minHonor"),
            win_medal: attr(reward, "winnerMedal"),
            lose_medal: attr(reward, "loserMedal"),
            minute_medal: attr(reward, "minuteBonusMedal"),
            minute_medal_max: attr(reward, "minuteBonusMedalMax"),
            mvp_win: ["XP", "BP", "Medal"].map(|k| attr(reward, &format!("Win{k}BonusMVP"))),
            mvp_lose: ["XP", "BP", "Medal"].map(|k| attr(reward, &format!("Lose{k}BonusMVP"))),
        },
        msg: Messages {
            view: attr(event, "viewTime"),
            delay: attr(event, "delayTime"),
            radar_cooldown: attr(event, "damagedRadarCoolDown"),
            benefit: event.attribute("sound_Benefit").unwrap_or_default().into(),
            loss: event.attribute("sound_Loss").unwrap_or_default().into(),
            help_view: attr(help, "viewTime"),
            help_dist: attr(help, "dist") * 0.01,
            help_honor: attr(help, "honor"),
            help: help.attribute("sound").unwrap_or_default().into(),
        },
    })
}

// ---------------------------------------------------------------------------------------------
// State

/// Honor an actor holds, what it earned in all (the kill formula uses it), its upgrade steps, its
/// class (index into [`CLASSES`]) and which buffs it stands in (bit 0 radar, 1 barricade, 2
/// combat officer).
#[derive(Component, Debug)]
struct Honor {
    points: f32,
    total: f32,
    level: [u8; 6],
    class: Option<usize>,
    aura: u8,
}

/// The six upgrades of the panel, in `UPGRADE`'s order: name and the sentence of messages
/// 2104-2109 (`{}` is the value of the next step).
const UPGRADES: [(&str, &str); 6] = [
    ("Weapon power", "+{} attack power per second"),
    ("Rapid fire", "+{}% shooting speed"),
    ("Body armour", "+{} AP and HP"),
    ("Fire rounds", "{} fire damage per second for 4 s"),
    ("Big magazines", "+{}% bullets"),
    ("Field medics", "-{}% respawn time"),
];

/// The class screen: seconds left, the highlighted class, confirmed.
struct Select {
    left: f32,
    sel: usize,
    done: bool,
}

#[derive(Resource)]
struct Blitz {
    cfg: Cfg,
    income_t: f32,
    /// Seconds into the radar and the barricade tick.
    zone_t: [f32; 2],
    /// Reinforcements called per side (index = [`Team`]) and entry of `REINFORCE_LIST`.
    fired: Vec<Vec<bool>>,
    /// The most barricades each side was ever seen with (0: not spawned yet).
    seen: [usize; 2],
    first_kill: bool,
    crate_at: Vec<Vec3>,
    /// Seconds until a destroyed crate comes back.
    crate_wait: Vec<Option<f32>>,
    /// The side that lost its radar or guardian, and seconds since.
    finish: Option<(Team, f32)>,
    time_limit: Option<f32>,
    /// `GUNZ_BLITZ_BUY`: (match second, upgrade) still to do.
    script: Vec<(f32, usize)>,
    open: bool,
    sel: usize,
    banner: (f32, String),
    /// (victim, attacker, match second) of recent hits between players, for the assists.
    recent: Vec<(Entity, Entity, f32)>,
    /// The class screen while it is up; `assign`: the classes are still to be handed out.
    select: Option<Select>,
    assign: bool,
    /// `GUNZ_BLITZ_CLASS`: the player's class when there is no screen.
    pick: Option<usize>,
    /// Blade and gun item of each class.
    kit: [[u32; 2]; CLASSES.len()],
    /// Item icon of each class's blade and gun (the class screen's cards).
    art: Vec<[ImageNode; 2]>,
    /// Frames until the class weapons are in hand (the ammunition bonus follows).
    settle: u8,
    /// `GUNZ_BLITZ_SKIP`: the match starts this many seconds in.
    skip: f32,
    /// Announcements waiting (sound, text, seconds on screen) and the one showing.
    msgs: VecDeque<(String, String, f32)>,
    event: (f32, String),
    /// Match second each side's radar was last reported under attack.
    radar_hit: [f32; 2],
    /// Help messages already shown (bit = message id - 2121).
    helped: u16,
    /// Honor the player just gained, for the effect and sound.
    gains: Vec<f32>,
    /// Seconds until each buff effect (radar, barricade, officer) is spawned again.
    aura_t: [f32; 3],
    /// What the match paid the player once it ended.
    payout: Option<Payout>,
    /// The minimap picture and the map area it shows (Bevy x, z).
    plan: Handle<Image>,
    bounds: (Vec2, Vec2),
}

impl Blitz {
    /// Queues an event message: the sound plays and the text shows when its turn comes.
    fn say(&mut self, sound: &str, text: impl Into<String>, view: f32) {
        self.msgs.push_back((sound.into(), text.into(), view));
    }
}

fn side(t: Team) -> usize {
    t as usize
}

fn other(t: Team) -> Team {
    match t {
        Team::Red => Team::Blue,
        Team::Blue => Team::Red,
    }
}

fn word(t: Team) -> &'static str {
    match t {
        Team::Red => "RED",
        Team::Blue => "BLUE",
    }
}

#[allow(clippy::too_many_arguments)]
fn setup(
    mut commands: Commands,
    level: Res<Level>,
    col: Res<MapCollision>,
    data: Res<ActorData>,
    shot: Option<Res<Shot>>,
    mut rules: ResMut<Rules>,
    mut routes: ResMut<Routes>,
    mut clock: ResMut<Clock>,
    mut images: ResMut<Assets<Image>>,
    mut vtime: ResMut<Time<Virtual>>,
    mut spawn: MessageWriter<SpawnNpc>,
) {
    let xml = level
        .vfs
        .read("system/blitzkrieg.xml")
        .unwrap_or_else(|e| panic!("system/blitzkrieg.xml: {e}"));
    let cfg = parse(String::from_utf8_lossy(&xml).trim_start_matches('\u{feff}'))
        .unwrap_or_else(|e| panic!("{e}"));
    let dummies = &level.map.dummies;
    let floor = |d: &crate::map::Dummy| {
        let p = Vec3::from(to_bevy(d.pos)) * SCALE;
        col.raycast(p + Vec3::Y, Vec3::NEG_Y, 3.0)
            .map_or(p, |h| h.point)
    };
    let named = |name: &str| dummies.iter().find(|d| d.name == name);
    for (id, names) in &cfg.routes {
        let path: Vec<Vec3> = names.iter().filter_map(|n| named(n)).map(floor).collect();
        if path.len() != names.len() {
            warn!("blitz: route {id} is missing a dummy of {names:?}");
        }
        routes.paths.insert(*id, path);
    }
    let hp = std::env::var("GUNZ_BLITZ_HP")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    for (actor, num, team) in &cfg.spawns {
        let team = if *team == 2 { Team::Red } else { Team::Blue };
        let base = actor.split('_').next().unwrap_or(actor);
        let prefix = format!("spawn_blitz_{base}_{}", word(team).to_lowercase());
        let mut spots: Vec<_> = dummies
            .iter()
            .filter(|d| d.name.starts_with(&prefix))
            .collect();
        spots.sort_by(|a, b| a.name.cmp(&b.name));
        if spots.len() != *num {
            warn!(
                "blitz: {actor} wants {num} spawn dummies, the map has {}",
                spots.len()
            );
        }
        for d in spots {
            spawn.write(SpawnNpc {
                id: actor.clone(),
                pos: floor(d) + Vec3::Y * 0.05,
                yaw: yaw_of(Vec3::from(to_bevy(d.dir))),
                hp_scale: hp,
                team: Some(team),
                ..default()
            });
        }
    }
    // The honor crates: `tresureN` stands at `spawn_blitz_honoritem_{N-1}` (*inferred* pairing).
    let crate_at: Vec<Vec3> = (0..cfg.crates.len())
        .filter_map(|i| named(&format!("spawn_blitz_honoritem_{i}")))
        .map(|d| floor(d) + Vec3::Y * 0.05)
        .collect();
    for (id, at) in cfg.crates.iter().zip(&crate_at) {
        spawn.write(SpawnNpc {
            id: id.clone(),
            pos: *at,
            ..default()
        });
    }
    // Rules the file gives (unless the command line did).
    if rules.respawn == RESPAWN_SECS {
        rules.respawn = cfg.respawn;
    }
    if rules.protect == PROTECT_SECS {
        rules.protect = cfg.invincible;
    }
    let time_limit = rules.time_limit.take().map(|s| s as f32);
    rules.kill_limit = None;
    let script: Vec<(f32, usize)> = std::env::var("GUNZ_BLITZ_BUY")
        .map(|v| {
            v.split(',')
                .filter_map(|p| {
                    let (t, n) = p.split_once(':')?;
                    Some((t.parse().ok()?, n.parse::<usize>().ok()?.checked_sub(1)?))
                })
                .collect()
        })
        .unwrap_or_default();
    let env = |k: &str| std::env::var(k).ok().and_then(|v| v.parse::<f32>().ok());
    // The class screen holds the match (`Hold`); a `--shot` run skips it unless asked, and
    // `GUNZ_BLITZ_CLASS` picks the player's class without it.
    let pick = env("GUNZ_BLITZ_CLASS")
        .map(|n| n as usize)
        .filter(|n| (1..=CLASSES.len()).contains(n))
        .map(|n| n - 1);
    let screen =
        pick.is_none() && (shot.is_none() || std::env::var_os("GUNZ_BLITZ_SELECT").is_some());
    if screen {
        commands.insert_resource(Hold);
        freeze(&mut commands, &mut vtime, true);
    }
    let skip = env("GUNZ_BLITZ_SKIP").unwrap_or(0.0);
    clock.elapsed = skip;
    let (plan, bounds) = ui::floor_plan(&level.map);
    // The weapons' item icons (`itemicon.xml`) for the class cards; there is no class art.
    let kit = kits(&data.items);
    let shop = crate::shop::ShopData::load(&level.vfs, &data.items)
        .unwrap_or_else(|e| panic!("blitz: shop data: {e}"));
    let icons = crate::shop::Icons::load(&level.vfs, &shop, &mut images);
    let art = kit.map(|k| k.map(|id| icons.node(&shop, id))).to_vec();
    info!(
        "blitz: {} routes, {} crates, respawn {} s, protection {} s, honor {} +{}/{} s, class screen {} s",
        routes.paths.len(),
        crate_at.len(),
        rules.respawn,
        rules.protect,
        cfg.start_honor,
        cfg.income,
        cfg.income_secs,
        cfg.class_select
    );
    commands.insert_resource(Blitz {
        income_t: 0.0,
        zone_t: [0.0; 2],
        fired: vec![vec![false; cfg.reinforce.len()]; 2],
        seen: [0; 2],
        first_kill: true,
        crate_wait: vec![None; crate_at.len()],
        crate_at,
        finish: None,
        time_limit,
        // A scripted (headless) run shows the panel.
        open: !script.is_empty(),
        script,
        sel: 0,
        banner: (0.0, String::new()),
        recent: Vec::new(),
        select: screen.then_some(Select {
            left: cfg.class_select,
            sel: 0,
            done: false,
        }),
        assign: true,
        pick,
        kit,
        art,
        settle: 0,
        skip,
        msgs: VecDeque::new(),
        event: (0.0, String::new()),
        radar_hit: [f32::MIN; 2],
        helped: 0,
        gains: Vec::new(),
        aura_t: [0.0; 3],
        payout: None,
        plan: images.add(plan),
        bounds,
        cfg,
    });
}

// ---------------------------------------------------------------------------------------------
// Actors

/// Every player and bot gets honor, the base enhancement (`ENHANCE_PLAYER`) and a respawn at its
/// side's spawn points (unless the run placed it by hand); a barricade or radar takes only
/// `1 - reduceDamageRatioFromPlayer` of what players and bots deal.
fn tag(
    mut commands: Commands,
    blitz: Res<Blitz>,
    setup: Option<Res<PlayerSetup>>,
    ahead: Option<Res<BotAhead>>,
    mut q: Query<(Entity, &mut Vitals, Has<Player>), (With<Score>, With<Team>, Without<Honor>)>,
    buildings: Query<(Entity, &Npc), (Added<Npc>, Without<Mods>)>,
) {
    let cfg = &blitz.cfg;
    // `GUNZ_BLITZ_SKIP`: the income of the seconds skipped.
    let skipped = blitz.skip / cfg.income_secs.max(0.1) * cfg.income;
    for (e, n) in &buildings {
        if matches!(n.kind.as_str(), "barricade" | "radar") {
            commands.entity(e).insert(Mods {
                vs_actors: cfg.building_takes,
                building: true,
                ..default()
            });
        }
    }
    for (e, mut v, player) in &mut q {
        v.max_hp += cfg.enhance_hp_ap;
        v.max_ap += cfg.enhance_hp_ap;
        v.hp += cfg.enhance_hp_ap;
        v.ap += cfg.enhance_hp_ap;
        let mut me = commands.entity(e);
        me.insert((
            Honor {
                points: cfg.start_honor + skipped,
                total: cfg.start_honor + skipped,
                level: [0; 6],
                class: None,
                aura: 0,
            },
            Mods::default(),
        ));
        let placed = if player {
            setup.as_ref().is_some_and(|s| s.at.is_some())
        } else {
            ahead.is_some()
        };
        if !placed {
            me.insert(Dead { respawn: 0.0 });
        }
    }
}

/// Honor income, and `ENHANCE_NPC`: every `nextTime` seconds (at most `maxCount` times) the
/// waves start `enhancedRatio` stronger.
fn income(
    time: Res<Time>,
    clock: Res<Clock>,
    mut blitz: ResMut<Blitz>,
    mut routes: ResMut<Routes>,
    mut q: Query<(&mut Honor, &Team, Has<Dead>)>,
) {
    blitz.income_t += time.delta_secs();
    let every = blitz.cfg.income_secs.max(0.1);
    // `LEAVE_AUTO_INC_HONOR`: a side with 3, 2 or 1 players left earns faster.
    let mut players = [0usize; 2];
    for (_, t, _) in &q {
        players[side(*t)] += 1;
    }
    while blitz.income_t >= every {
        blitz.income_t -= every;
        for (mut h, t, dead) in &mut q {
            if !dead {
                // The Hunter's `aquirHonorRatio` (**inferred**: a share more of every honor gain).
                let g = blitz.cfg.income_for(players[side(*t)])
                    * (1.0 + blitz.cfg.class_val(h.class, "aquirHonorRatio"));
                h.points += g;
                h.total += g;
            }
        }
    }
    let c = &blitz.cfg;
    routes.boost = (clock.elapsed / c.npc_next.max(1.0)).floor().min(c.npc_max) * c.npc_ratio;
}

/// Burning rounds, the assist window and the honor of every kill (`HONOR_LIST`).
#[allow(clippy::too_many_arguments)]
fn scoring(
    clock: Res<Clock>,
    mut blitz: ResMut<Blitz>,
    mut hits: MessageReader<Damage>,
    mut kills: MessageReader<Killed>,
    mut burn: MessageWriter<Afflict>,
    mut honors: Query<(Entity, &mut Honor, &Name, Option<&Team>, Has<Player>)>,
    npcs: Query<(&Npc, Option<&Team>)>,
) {
    let now = clock.elapsed;
    let sides: HashMap<Entity, Option<Team>> = honors.iter().map(|h| (h.0, h.3.copied())).collect();
    // The side of a player, bot or monster.
    let team_of = |e: Entity| {
        sides
            .get(&e)
            .copied()
            .flatten()
            .or_else(|| npcs.get(e).ok().and_then(|n| n.1.copied()))
    };
    for d in hits.read() {
        let (Some(by), Some(foe)) = (team_of(d.attacker), team_of(d.target)) else {
            continue;
        };
        if !sides.contains_key(&d.attacker) || d.amount <= 0.0 || by == foe {
            continue;
        }
        let fire = honors.get(d.attacker).map_or(0.0, |h| {
            sum(&blitz.cfg.up.fire, h.1.level[3])
                + blitz.cfg.class_val(h.1.class, "enchantFireDamage")
        });
        if fire > 0.0 {
            burn.write(Afflict {
                target: d.target,
                by: d.attacker,
                secs: FIRE_SECS,
                slow: 1.0,
                stun: false,
                root: false,
                dot: fire * FIRE_SECS,
            });
        }
        if sides.contains_key(&d.target) {
            blitz.recent.push((d.target, d.attacker, now));
        }
    }
    let [single, assist, single_div, assist_div, _, assist_delay] = blitz.cfg.kill;
    blitz.recent.retain(|r| now - r.2 <= assist_delay);
    for k in kills.read() {
        let Some(by) = team_of(k.killer) else {
            continue;
        };
        if team_of(k.victim) == Some(by) {
            continue;
        }
        // What the killer gets itself, and what each player of its side gets.
        let mut own: Vec<(Entity, f32)> = Vec::new();
        let mut all = 0.0;
        let what;
        if let Ok((n, _)) = npcs.get(k.victim) {
            let Some(&(one, every)) = blitz.cfg.by_type.get(&n.kind) else {
                continue;
            };
            what = n.kind.clone();
            if sides.contains_key(&k.killer) {
                own.push((k.killer, one));
            }
            all = every;
        } else if sides.contains_key(&k.victim) && sides.contains_key(&k.killer) {
            what = "player".into();
            let total = honors.get(k.victim).map_or(0.0, |h| h.1.total);
            let first = std::mem::take(&mut blitz.first_kill);
            let bonus = if first { blitz.cfg.first_kill } else { 0.0 };
            own.push((k.killer, single + total / single_div + bonus));
            for r in blitz
                .recent
                .iter()
                .filter(|r| r.0 == k.victim && r.1 != k.killer)
            {
                if team_of(r.1) == Some(by) && !own.iter().any(|o| o.0 == r.1) {
                    own.push((r.1, assist + total / assist_div));
                }
            }
        } else {
            continue;
        }
        let mut gains = String::new();
        for (e, mut h, name, t, player) in &mut honors {
            let g = (own.iter().find(|o| o.0 == e).map_or(0.0, |o| o.1)
                + if t == Some(&by) { all } else { 0.0 })
                * (1.0 + blitz.cfg.class_val(h.class, "aquirHonorRatio"));
            if g > 0.0 {
                h.points += g;
                h.total += g;
                gains += &format!(" {name} +{g:.0}");
                if player {
                    blitz.banner = (now + BANNER_SECS / 2.0, format!("+{g:.0} HONOR"));
                    blitz.gains.push(g);
                }
            }
        }
        if !gains.is_empty() {
            info!(
                "t={now:.1} blitz: {} killed a {what}: honor{gains}",
                word(by)
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Upgrades

fn sum(a: &[f32; 4], level: u8) -> f32 {
    a[..level as usize].iter().sum()
}

/// Buys step `level + 1` of upgrade `which`: `Ok(text)` or why not.
fn buy(
    cfg: &Cfg,
    h: &mut Honor,
    which: usize,
    v: &mut Vitals,
    load: &mut Loadout,
) -> Result<String, String> {
    let level = h.level[which] as usize;
    if level >= 4 {
        return Err(format!("{} is at its last step", UPGRADES[which].0));
    }
    let cost = cfg.up.cost[level];
    if h.points < cost {
        return Err(format!("{} needs {cost:.0} honor", UPGRADES[which].0));
    }
    h.points -= cost;
    h.level[which] += 1;
    match which {
        2 => {
            let d = cfg.up.hp_ap[level];
            v.max_hp += d;
            v.max_ap += d;
            v.hp += d;
            v.ap += d;
        }
        4 => {
            let (old, new) = (
                sum(&cfg.up.magazine, level as u8),
                sum(&cfg.up.magazine, level as u8 + 1),
            );
            for s in &mut load.slots {
                s.reserve = (s.reserve as f32 * (1.0 + new) / (1.0 + old)).round() as u32;
            }
        }
        _ => {}
    }
    Ok(format!(
        "{} step {} (-{cost:.0} honor)",
        UPGRADES[which].0,
        level + 1
    ))
}

fn input(
    keys: Res<ButtonInput<KeyCode>>,
    clock: Res<Clock>,
    frozen: Option<Res<Frozen>>,
    mut blitz: ResMut<Blitz>,
    mut player: Query<(&mut Honor, &mut Vitals, &mut Loadout), (With<Player>, Without<Dead>)>,
) {
    let Ok((mut h, mut v, mut load)) = player.single_mut() else {
        return;
    };
    let mut wanted = Vec::new();
    while blitz.script.first().is_some_and(|s| s.0 <= clock.elapsed) {
        wanted.push(blitz.script.remove(0).1);
    }
    if frozen.is_none() {
        if keys.just_pressed(KeyCode::KeyF) {
            blitz.open = !blitz.open;
        }
        if blitz.open {
            let n = UPGRADES.len();
            if keys.just_pressed(KeyCode::ArrowDown) {
                blitz.sel = (blitz.sel + 1) % n;
            }
            if keys.just_pressed(KeyCode::ArrowUp) {
                blitz.sel = (blitz.sel + n - 1) % n;
            }
            if keys.just_pressed(KeyCode::Enter) {
                wanted.push(blitz.sel);
            }
        }
    }
    for which in wanted.into_iter().filter(|w| *w < UPGRADES.len()) {
        let text = match buy(&blitz.cfg, &mut h, which, &mut v, &mut load) {
            Ok(t) => {
                info!("t={:.1} blitz: Player buys {t}", clock.elapsed);
                t
            }
            Err(e) => e,
        };
        blitz.banner = (clock.elapsed + BANNER_SECS, text);
    }
}

fn bots_buy(
    clock: Res<Clock>,
    blitz: Res<Blitz>,
    mut bots: Query<(&mut Honor, &mut Vitals, &mut Loadout, &Name), (With<Bot>, Without<Dead>)>,
) {
    for (mut h, mut v, mut load, name) in &mut bots {
        for which in BOT_ORDER {
            if let Ok(t) = buy(&blitz.cfg, &mut h, which, &mut v, &mut load) {
                info!("t={:.1} blitz: {name} buys {t}", clock.elapsed);
                break;
            }
        }
    }
}

/// The Blitzkrieg key of a weapon in `WEAPON` (`None`: no table entry).
fn table_key(k: WeaponKind) -> Option<&'static str> {
    use WeaponKind::*;
    Some(match k {
        Smg | SmgX2 => "smg",
        Rifle => "rifle",
        MachineGun => "machineGun",
        Pistol | PistolX2 => "pistol",
        Revolver | RevolverX2 => "revolver",
        Shotgun => "shotGun",
        Rocket => "roket",
        Dagger => "dagger",
        Katana => "katana",
        DoubleKatana => "doubleKatana",
        _ => return None,
    })
}

/// What upgrades, classes and buildings do to an actor's blows, shots and wounds: the extra DPS
/// (the base `ENHANCE_PLAYER dps` plus the steps and the class's) becomes `dps * factor * delay`
/// more per hit of the current weapon (*inferred* reading of `WEAPON`), a rapid-fire step
/// shortens the gun delay, inside a friendly barricade's zone damage is cut (message 2124: "only
/// half"), and a combat officer's allies in reach take `reduceDamageRatioForMyTeam` less
/// (*inferred*: the officer counts too). Class numbers are read as `CLASS_TABLE` gives them:
/// `enhanceMeleeDPS` / `reduceDPS` are DPS like `ENHANCE_PLAYER dps`, the `*Ratio` ones and
/// `enhanceShotgunDamage="1"` (the file says shares are 0..1) are shares of the damage.
fn buffs(
    blitz: Res<Blitz>,
    data: Res<ActorData>,
    walls: Query<(&Npc, &Team, &GlobalTransform), Without<Dead>>,
    mut q: Query<(&mut Honor, &mut Mods, &Loadout, &Transform, Option<&Team>), Without<Dead>>,
) {
    let cfg = &blitz.cfg;
    let officer = (
        cfg.class_val(Some(OFFICER), "reduceDamageRatioForMyTeam"),
        { cfg.class_val(Some(OFFICER), "distance") * 0.01 },
    );
    let officers: Vec<(Team, Vec3)> = q
        .iter()
        .filter(|(h, ..)| h.class == Some(OFFICER))
        .filter_map(|(_, _, _, tf, t)| Some((*t?, tf.translation)))
        .collect();
    for (mut h, mut m, load, tf, team) in &mut q {
        let c = h.class;
        let w = load
            .slots
            .get(load.current)
            .and_then(|s| data.items.get(s.item))
            .and_then(|i| i.weapon.as_ref());
        let melee = w.is_some_and(|w| is_melee(w.kind));
        let shotgun = w.is_some_and(|w| w.kind == WeaponKind::Shotgun);
        let extra = cfg.enhance_dps
            + sum(&cfg.up.dps, h.level[0])
            + if melee {
                cfg.class_val(c, "enhanceMeleeDPS")
            } else {
                0.0
            }
            - cfg.class_val(c, "reduceDPS");
        m.dealt = w
            .and_then(|w| {
                let (factor, delay) = cfg.weapon.get(table_key(w.kind)?)?;
                Some(1.0 + extra * factor * delay * 0.001 / (w.damage as f32).max(1.0))
            })
            .unwrap_or(1.0)
            * (1.0 + cfg.class_val(c, "enhanceDamageRatio"))
            * if shotgun {
                1.0 + cfg.class_val(c, "enhanceShotgunDamage")
            } else {
                1.0
            };
        m.vs_buildings = 1.0 + cfg.class_val(c, "enhanceDamageRatioAtBuilding");
        m.shot_delay = 1.0 / (1.0 + sum(&cfg.up.delay, h.level[1]));
        let near = |kind: &str, dist: f32| {
            team.is_some_and(|t| {
                walls.iter().any(|(n, wt, g)| {
                    n.kind == kind && wt == t && within(g.translation(), tf.translation, dist)
                })
            })
        };
        let (radar, barricade) = (
            near("radar", cfg.radar.dist),
            near("barricade", cfg.barricade.dist),
        );
        let guarded = team.is_some_and(|t| {
            officers
                .iter()
                .any(|(ot, p)| ot == t && within(*p, tf.translation, officer.1))
        });
        // The Trickster's `reduceDamageRatio` takes the barricade's place where it is higher
        // (**inferred**: the attribute is the barricade's own).
        m.taken = if barricade {
            1.0 - cfg
                .barricade
                .reduce
                .max(cfg.class_val(c, "reduceDamageRatio"))
        } else {
            1.0
        } * if guarded { 1.0 - officer.0 } else { 1.0 };
        h.aura = radar as u8 | (barricade as u8) << 1 | (guarded as u8) << 2;
    }
}

fn within(a: Vec3, b: Vec3, dist: f32) -> bool {
    Vec2::new(a.x - b.x, a.z - b.z).length() <= dist && (a.y - b.y).abs() <= ZONE_HEIGHT
}

/// The field-medic upgrade: the respawn timer runs faster by the share it saves.
fn faster_respawn(time: Res<Time>, blitz: Res<Blitz>, mut q: Query<(&Honor, &mut Dead)>) {
    for (h, mut d) in &mut q {
        let k = match h.level[5] {
            0 => 0.0,
            n => blitz.cfg.up.revive[n as usize - 1],
        };
        if d.respawn > 0.0 && d.respawn < 1.0e5 {
            d.respawn -= time.delta_secs() * k / (1.0 - k).max(0.05);
        }
    }
}

/// Spare rounds of a fresh set of guns: the magazine steps stretch each gun's reserve and the
/// Duelist's `addShotgunMagazine` adds magazines to the shotgun. Sets rather than adds, so a
/// second call changes nothing.
fn restock(cfg: &Cfg, data: &ActorData, h: &Honor, load: &mut Loadout) {
    let k = 1.0 + sum(&cfg.up.magazine, h.level[4]) + cfg.class_val(h.class, "addMagazineRatio");
    let mags = cfg.class_val(h.class, "addShotgunMagazine");
    for s in &mut load.slots {
        let Some(w) = data.items.get(s.item).and_then(|i| i.weapon.as_ref()) else {
            continue;
        };
        let base = w
            .max_bullet
            .unwrap_or(w.magazine * 4)
            .saturating_sub(w.magazine);
        let extra = if w.kind == WeaponKind::Shotgun {
            mags * w.magazine as f32
        } else {
            0.0
        };
        s.reserve = (base as f32 * k + extra).round() as u32;
    }
}

/// A respawn refills the guns from scratch ([`restock`]); so do the classes' new weapons a few
/// frames after they were handed out ([`Equip`] has to land first).
fn rearm(
    mut back: RemovedComponents<Dead>,
    mut blitz: ResMut<Blitz>,
    data: Res<ActorData>,
    mut q: Query<(&Honor, &mut Loadout)>,
) {
    if blitz.settle > 0 {
        blitz.settle -= 1;
        if blitz.settle == 0 {
            for (h, mut load) in &mut q {
                restock(&blitz.cfg, &data, h, &mut load);
            }
        }
    }
    for e in back.read() {
        if let Ok((h, mut load)) = q.get_mut(e) {
            restock(&blitz.cfg, &data, h, &mut load);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Buildings

/// Restores `ratio` of each gun's ammunition cap (`max_bullet`, raised by the magazine steps and
/// the shotgun's class magazines).
fn refill(data: &ActorData, load: &mut Loadout, ratio: f32, bonus: f32, mags: f32) {
    for s in &mut load.slots {
        if let Some((cap, w)) = data
            .items
            .get(s.item)
            .and_then(|i| i.weapon.as_ref())
            .and_then(|w| Some((w.max_bullet?, w)))
        {
            let extra = if w.kind == WeaponKind::Shotgun {
                mags * w.magazine as f32
            } else {
                0.0
            };
            let cap = cap as f32 * (1.0 + bonus) + extra;
            s.reserve = (s.reserve as f32 + (cap * ratio).ceil()).min(cap) as u32;
        }
    }
}

/// The radar heals health, armour and ammunition in reach; a barricade restocks ammunition.
fn zones(
    time: Res<Time>,
    data: Res<ActorData>,
    mut blitz: ResMut<Blitz>,
    walls: Query<(&Npc, &Team, &GlobalTransform), Without<Dead>>,
    mut q: Query<(&Honor, &Team, &Transform, &mut Vitals, &mut Loadout), Without<Dead>>,
) {
    let dt = time.delta_secs();
    blitz.zone_t[0] += dt;
    blitz.zone_t[1] += dt;
    let (radar, barricade) = (blitz.cfg.radar.clone(), blitz.cfg.barricade.clone());
    let heal = blitz.zone_t[0] >= radar.delay;
    let stock = blitz.zone_t[1] >= barricade.delay;
    if heal {
        blitz.zone_t[0] = 0.0;
    }
    if stock {
        blitz.zone_t[1] = 0.0;
    }
    if !heal && !stock {
        return;
    }
    for (h, team, tf, mut v, mut load) in &mut q {
        let k = |a: &str| blitz.cfg.class_val(h.class, a);
        let bonus = sum(&blitz.cfg.up.magazine, h.level[4]) + k("addMagazineRatio");
        let mags = k("addShotgunMagazine");
        // The Trickster's `recovery*Ratio` replace the zone's own where higher (**inferred**: the
        // attributes are those of `BUILDING/RADAR` and `/BARRICADE`).
        let ap_hp = radar.ap_hp.max(k("recoveryApHpRatio"));
        let (radar_mag, barricade_mag) = (
            radar.mag.max(k("recoveryMagazineRatio")),
            barricade.mag.max(k("recoveryMagazineRatio")),
        );
        for (n, wt, g) in &walls {
            if wt != team {
                continue;
            }
            match n.kind.as_str() {
                "radar" if heal && within(g.translation(), tf.translation, radar.dist) => {
                    v.hp = (v.hp + ap_hp * v.max_hp).min(v.max_hp);
                    v.ap = (v.ap + ap_hp * v.max_ap).min(v.max_ap);
                    refill(&data, &mut load, radar_mag, bonus, mags);
                }
                "barricade" if stock && within(g.translation(), tf.translation, barricade.dist) => {
                    refill(&data, &mut load, barricade_mag, bonus, mags);
                }
                _ => {}
            }
        }
    }
}

/// `REINFORCE_LIST`: a side down to 9 / 6 / 3 barricades gets its radar switched to the next
/// reinforcement state; with none left the enemy radar summons the terminator.
fn reinforce(
    clock: Res<Clock>,
    mut blitz: ResMut<Blitz>,
    npcs: Query<(Entity, &Npc, &Team), Without<Dead>>,
    player: Query<&Team, With<Player>>,
    mut state: MessageWriter<NpcState>,
) {
    let mut count = [0usize; 2];
    let mut radar: [Option<Entity>; 2] = [None; 2];
    for (e, n, t) in &npcs {
        match n.kind.as_str() {
            "barricade" => count[side(*t)] += 1,
            "radar" => radar[side(*t)] = Some(e),
            _ => {}
        }
    }
    let b = &mut *blitz;
    for i in 0..2 {
        b.seen[i] = b.seen[i].max(count[i]);
    }
    let mine = player.single().ok().copied();
    for t in [Team::Red, Team::Blue] {
        let s = side(t);
        for i in 0..b.cfg.reinforce.len() {
            let r = b.cfg.reinforce[i].clone();
            if b.seen[s] == 0 || b.fired[s][i] || count[s] > r.barricades as usize {
                continue;
            }
            b.fired[s][i] = true;
            let to = if r.alliance { t } else { other(t) };
            let Some(npc) = radar[side(to)] else { continue };
            state.write(NpcState {
                npc,
                state: r.state.clone(),
            });
            let what = r.state.trim_start_matches("summon_");
            info!(
                "t={:.1} blitz: {} has {} barricades left: {} radar calls {what}",
                clock.elapsed,
                word(t),
                count[s],
                word(to)
            );
            // `EventBenefit` for the player's side, `EventLoss` for the other; the enemy's wave
            // carries message 2126 (the wording of the allied one and of the terminator is mine).
            let (sound, text) = if Some(to) == mine {
                (
                    b.cfg.msg.benefit.clone(),
                    format!("ALLIED REINFORCEMENTS: {what}"),
                )
            } else if what == "terminator" {
                (b.cfg.msg.loss.clone(), "THE TERMINATOR HAS ARRIVED".into())
            } else {
                b.helped |= 1 << 5;
                (
                    b.cfg.msg.loss.clone(),
                    "The enemy's reinforcements have arrived. Eliminate them to win Honor Points."
                        .into(),
                )
            };
            let view = b.cfg.msg.view;
            b.say(&sound, text, view);
        }
    }
}

/// A destroyed honor crate comes back after `HONOR_ITEM_LIST respawnTime`.
fn crates(
    time: Res<Time>,
    mut blitz: ResMut<Blitz>,
    mut kills: MessageReader<Killed>,
    npcs: Query<&Npc>,
    mut spawn: MessageWriter<SpawnNpc>,
    mut vfx: MessageWriter<Vfx>,
) {
    for k in kills.read() {
        let Ok(n) = npcs.get(k.victim) else { continue };
        let Some(i) = blitz.cfg.crates.iter().position(|c| *c == n.id) else {
            continue;
        };
        blitz.crate_wait[i] = Some(blitz.cfg.crate_respawn);
    }
    for i in 0..blitz.crate_wait.len() {
        let Some(t) = blitz.crate_wait[i].as_mut() else {
            continue;
        };
        *t -= time.delta_secs();
        if *t <= 0.0 {
            blitz.crate_wait[i] = None;
            spawn.write(SpawnNpc {
                id: blitz.cfg.crates[i].clone(),
                pos: blitz.crate_at[i],
                ..default()
            });
            vfx.write(Vfx::Named {
                name: HONOR_ITEM_FX.into(),
                at: Transform::from_translation(blitz.crate_at[i]),
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// The end

/// The match ends when a side loses its radar or guardian (the end screen follows
/// `FINISH_DELAY_TIME` later), or, at the time limit, to the side with more barricades left
/// (*inferred*).
#[allow(clippy::too_many_arguments)]
fn finish(
    time: Res<Time>,
    npcs: Query<(&Npc, &Team), Without<Dead>>,
    mut kills: MessageReader<Killed>,
    victims: Query<(&Npc, &Team)>,
    player: Query<&Team, With<Player>>,
    mut blitz: ResMut<Blitz>,
    mut clock: ResMut<Clock>,
    mut commands: Commands,
    mut vtime: ResMut<Time<Virtual>>,
    honors: Query<(&Honor, &Team, Has<Player>)>,
    mut reward: MessageWriter<Reward>,
) {
    let mut count = [0usize; 2];
    let mut soldiers = [0usize; 2];
    for (n, t) in &npcs {
        match n.kind.as_str() {
            "barricade" => count[side(*t)] += 1,
            "knifeman" | "throwman" | "zealot" | "cleric" | "knight" | "terminator" => {
                soldiers[side(*t)] += 1
            }
            _ => {}
        }
    }
    clock.note = format!("BARRICADES RED {} : {} BLUE", count[0], count[1]);
    if (clock.elapsed / 20.0).floor() != ((clock.elapsed - time.delta_secs()) / 20.0).floor() {
        info!(
            "t={:.0} blitz: barricades RED {} : {} BLUE, soldiers RED {} : {} BLUE",
            clock.elapsed, count[0], count[1], soldiers[0], soldiers[1]
        );
    }
    for k in kills.read() {
        if let Ok((n, t)) = victims.get(k.victim)
            && matches!(n.kind.as_str(), "radar" | "guardian")
            && blitz.finish.is_none()
        {
            info!(
                "t={:.1} blitz: {} {} destroyed",
                clock.elapsed,
                word(*t),
                n.kind
            );
            blitz.finish = Some((*t, 0.0));
        }
    }
    let mine = player.single().ok().copied();
    let verdict = |loser: Option<Team>| match (loser, mine) {
        (None, _) | (_, None) => "DRAW",
        (Some(l), Some(m)) if l == m => "DEFEAT",
        _ => "VICTORY",
    };
    let delay = blitz.cfg.finish_delay;
    if let Some((loser, t)) = &mut blitz.finish {
        *t += time.delta_secs();
        clock.note = format!("{} DESTROYED - {} WINS", word(*loser), word(other(*loser)));
        if *t >= delay && clock.over.is_none() {
            clock.over = Some(verdict(Some(*loser)).into());
            freeze(&mut commands, &mut vtime, true);
        }
    } else if blitz.time_limit.is_some_and(|l| clock.elapsed >= l) && clock.over.is_none() {
        let loser = match count[0].cmp(&count[1]) {
            std::cmp::Ordering::Less => Some(Team::Red),
            std::cmp::Ordering::Greater => Some(Team::Blue),
            std::cmp::Ordering::Equal => None,
        };
        info!(
            "t={:.1} blitz: time is up, barricades RED {} : {} BLUE",
            clock.elapsed, count[0], count[1]
        );
        clock.over = Some(verdict(loser).into());
        freeze(&mut commands, &mut vtime, true);
    }
    // The reward, once, as soon as the match is over.
    if let Some(headline) = clock.over.clone().filter(|_| blitz.payout.is_none())
        && let Some((h, t, _)) = honors.iter().find(|h| h.2)
    {
        let best = honors
            .iter()
            .filter(|o| o.1 == t)
            .map(|o| o.0.total)
            .fold(0.0, f32::max);
        let p = payout(
            &blitz.cfg.reward,
            headline == "VICTORY",
            h.total >= best,
            clock.elapsed,
            h.total,
        );
        info!(
            "t={:.0} blitz: reward {headline}{}: +{} XP, +{} bounty, +{} medals{}",
            clock.elapsed,
            if p.mvp { " (MVP)" } else { "" },
            p.xp,
            p.bounty,
            p.medals,
            p.none
                .as_deref()
                .map_or(String::new(), |w| format!(" ({w})"))
        );
        if p.xp + p.bounty + p.medals > 0 {
            reward.write(Reward {
                xp: p.xp,
                bounty: p.bounty,
                medals: p.medals,
            });
        }
        blitz.payout = Some(p);
    }
}

/// What a finished match paid the player.
#[derive(Clone, Debug, PartialEq)]
struct Payout {
    won: bool,
    mvp: bool,
    minutes: u32,
    xp: u32,
    bounty: u32,
    medals: u32,
    /// Why nothing was paid (`REWARD minTime` / `minHonor` not reached).
    none: Option<String>,
}

/// The `REWARD` rules. **Observed**: nothing is paid below `minTime` seconds or `minHonor` honor;
/// the medals are `winnerMedal` / `loserMedal` plus `minuteBonusMedal` per minute up to
/// `minuteBonusMedalMax`; the MVP of a side gets `WinXPBonusMVP` .. `LoseMedalBonusMVP` more of
/// each. **Inferred**: XP and bounty are `baseExp` / `baseBounty` per minute played (the file
/// only says "base amount"), the same for either side; a draw pays like a loss; the MVP is the
/// side's player with the most honor earned. `minPlayCount` (a newcomer bonus) and the waiting
/// medals have no offline counterpart.
fn payout(r: &RewardCfg, won: bool, mvp: bool, secs: f32, honor: f32) -> Payout {
    let minutes = (secs / 60.0) as u32;
    let mut p = Payout {
        won,
        mvp,
        minutes,
        xp: 0,
        bounty: 0,
        medals: 0,
        none: None,
    };
    if secs < r.min_time {
        p.none = Some(format!("needs {:.0} s of play", r.min_time));
    } else if honor < r.min_honor {
        p.none = Some(format!("needs {:.0} honor", r.min_honor));
    }
    if p.none.is_some() {
        return p;
    }
    let bonus = match (mvp, won) {
        (false, _) => [0.0; 3],
        (true, true) => r.mvp_win,
        (true, false) => r.mvp_lose,
    };
    let m = minutes as f32;
    let medals =
        if won { r.win_medal } else { r.lose_medal } + (m * r.minute_medal).min(r.minute_medal_max);
    p.xp = (r.exp * m * (1.0 + bonus[0])).round() as u32;
    p.bounty = (r.bounty * m * (1.0 + bonus[1])).round() as u32;
    p.medals = (medals * (1.0 + bonus[2])).round() as u32;
    p
}

// ---------------------------------------------------------------------------------------------
// Classes

/// Each class's blade and gun item (0: the data has none): the default loadout's katana,
/// revolver or rifle where the kind matches, else the lowest id of the kind that has a name, a
/// model and a real damage (zitem lists debug items with `damage="1"`): the same weapons every
/// run, unlike a map's iteration order.
fn kits(items: &Items) -> [[u32; 2]; CLASSES.len()] {
    let is = |i: &crate::item::Item, kind| {
        i.weapon
            .as_ref()
            .is_some_and(|w| w.kind == kind && w.damage > 1)
    };
    let first = |kind: WeaponKind| {
        DEFAULT_LOADOUT
            .into_iter()
            .find(|id| items.get(*id).is_some_and(|i| is(i, kind)))
            .or_else(|| {
                items
                    .weapons()
                    .filter(|i| i.name.is_some() && items.model(i).is_some() && is(i, kind))
                    .map(|i| i.id)
                    .min()
            })
            .unwrap_or(0)
    };
    let mut kits = KITS.map(|(blade, gun)| [first(blade), first(gun)]);
    if items
        .get(INCINERATOR)
        .is_some_and(|i| items.model(i).is_some())
    {
        kits[2][1] = INCINERATOR;
    }
    kits
}

/// What a Blitzkrieg actor spawns with: the default loadout and every class weapon (an
/// [`Equip`] then picks the ones its class uses).
pub fn arsenal(items: &Items) -> Vec<u32> {
    let mut all = DEFAULT_LOADOUT.to_vec();
    for id in kits(items).into_iter().flatten() {
        if id != 0 && !all.contains(&id) {
            all.push(id);
        }
    }
    all
}

/// The class screen (`CLASS_SELECT_TIME`, `blitz/ui.rs`) and then the classes: the player's
/// pick (or none), a random one for each bot with at most [`SAME_CLASS`] per side, each
/// equipped with its weapons. Enter / Space confirm, 1-9 and the arrow keys choose; the
/// highlighted class is taken when the time is up.
#[allow(clippy::too_many_arguments)]
fn classes(
    real: Res<Time<Real>>,
    keys: Res<ButtonInput<KeyCode>>,
    clock: Res<Clock>,
    mut blitz: ResMut<Blitz>,
    mut commands: Commands,
    mut vtime: ResMut<Time<Virtual>>,
    mut equip: MessageWriter<Equip>,
    mut seed: Local<u32>,
    mut q: Query<(Entity, &mut Honor, &mut Vitals, &Team, &Name, Has<Player>)>,
) {
    if !blitz.assign || !q.iter().any(|a| a.5) {
        return;
    }
    let mut chosen = blitz.pick;
    let held = blitz.select.is_some();
    if let Some(s) = &mut blitz.select {
        s.left -= real.delta_secs();
        let n = CLASSES.len();
        let digits = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        if let Some(i) = digits.iter().position(|k| keys.just_pressed(*k)) {
            s.sel = i;
        }
        if keys.any_just_pressed([KeyCode::ArrowRight, KeyCode::ArrowDown]) {
            s.sel = (s.sel + 1) % n;
        }
        if keys.any_just_pressed([KeyCode::ArrowLeft, KeyCode::ArrowUp]) {
            s.sel = (s.sel + n - 1) % n;
        }
        s.done |= s.left <= 0.0 || keys.any_just_pressed([KeyCode::Enter, KeyCode::Space]);
        if !s.done {
            return;
        }
        chosen = Some(s.sel);
    }
    if *seed == 0 {
        *seed = 0x2545_f491;
    }
    let mut order: Vec<(Entity, bool)> = q.iter().map(|a| (a.0, a.5)).collect();
    order.sort_by_key(|o| !o.1);
    let mut count = [[0u8; CLASSES.len()]; 2];
    let mut log = String::new();
    for (e, player) in order {
        let Ok((_, mut h, mut v, team, name, _)) = q.get_mut(e) else {
            continue;
        };
        let c = if player {
            chosen
        } else {
            let start = (rnd(&mut seed) * CLASSES.len() as f32) as usize;
            (0..CLASSES.len())
                .map(|i| (start + i) % CLASSES.len())
                .find(|c| count[side(*team)][*c] < SAME_CLASS)
        };
        h.class = c;
        // The Gladiator's `addMaxApHp`.
        let add = blitz.cfg.class_val(c, "addMaxApHp");
        v.max_hp += add;
        v.max_ap += add;
        v.hp += add;
        v.ap += add;
        let items = match c {
            Some(c) => blitz.kit[c].map(|id| (id, None)).to_vec(),
            None => DEFAULT_LOADOUT.map(|id| (id, None)).to_vec(),
        };
        equip.write(Equip {
            actor: e,
            items,
            current: usize::from(c.is_some_and(|c| c != 0)),
        });
        if let Some(c) = c {
            count[side(*team)][c] += 1;
            log += &format!(" {name} ({}) {};", word(*team), CLASSES[c].0);
        }
        if player {
            blitz.banner = (
                clock.elapsed + BANNER_SECS,
                c.map_or("NO CLASS".into(), |c| format!("CLASS: {}", CLASSES[c].0)),
            );
        }
    }
    info!("blitz: classes:{log}");
    blitz.assign = false;
    blitz.settle = 3;
    if held {
        blitz.select = None;
        commands.remove_resource::<Hold>();
        freeze(&mut commands, &mut vtime, false);
    }
}

// ---------------------------------------------------------------------------------------------
// Announcer

/// `EVENT_MESSAGE`: a radar under attack (at most once per `damagedRadarCoolDown` per side) and
/// a barricade destroyed; the sound is `EventBenefit` when it favours the player's side,
/// `EventLoss` when not (*inferred* pairing; the texts are mine, no string gives them).
fn events(
    clock: Res<Clock>,
    mut blitz: ResMut<Blitz>,
    mut hits: MessageReader<Damage>,
    mut kills: MessageReader<Killed>,
    npcs: Query<(&Npc, &Team)>,
    player: Query<&Team, With<Player>>,
) {
    let Ok(&mine) = player.single() else { return };
    let now = clock.elapsed;
    let view = blitz.cfg.msg.view;
    let say = |blitz: &mut Blitz, team: Team, benefit: &str, loss: &str| {
        let (sound, text) = if team == mine {
            (blitz.cfg.msg.loss.clone(), loss)
        } else {
            (blitz.cfg.msg.benefit.clone(), benefit)
        };
        blitz.say(&sound, text, view);
    };
    for d in hits.read() {
        if let Ok((n, t)) = npcs.get(d.target)
            && n.kind == "radar"
            && d.amount > 0.0
            && now - blitz.radar_hit[side(*t)] >= blitz.cfg.msg.radar_cooldown
        {
            blitz.radar_hit[side(*t)] = now;
            say(
                &mut blitz,
                *t,
                "ENEMY RADAR UNDER ATTACK",
                "YOUR RADAR IS UNDER ATTACK",
            );
        }
    }
    for k in kills.read() {
        if let Ok((n, t)) = npcs.get(k.victim)
            && n.kind == "barricade"
        {
            say(
                &mut blitz,
                *t,
                "ENEMY BARRICADE DESTROYED",
                "YOUR BARRICADE WAS DESTROYED",
            );
        }
    }
}

/// `HELP_MESSAGE`: the retail help sentences (messages 2121-2128), each once, when their
/// situation arises (the triggers are *inferred* from the wording; `honor` and `dist` are the
/// file's) and nothing else is on screen.
fn helps(
    clock: Res<Clock>,
    mut blitz: ResMut<Blitz>,
    data: Res<ActorData>,
    player: Query<(&Honor, &Vitals, &Transform, &Team, &Loadout), (With<Player>, Without<Dead>)>,
    npcs: Query<(&Npc, &Team, &GlobalTransform), Without<Dead>>,
) {
    let now = clock.elapsed;
    if blitz.select.is_some() || !blitz.msgs.is_empty() || now < blitz.event.0 {
        return;
    }
    let Ok((h, v, tf, team, load)) = player.single() else {
        return;
    };
    let dist = blitz.cfg.msg.help_dist;
    let (mut building, mut soldiers) = (0, 0);
    for (n, t, g) in &npcs {
        if t != team && within(g.translation(), tf.translation, dist) {
            match n.kind.as_str() {
                "radar" | "barricade" => building += 1,
                k if SOLDIERS.contains(&k) => soldiers += 1,
                _ => {}
            }
        }
    }
    let melee = load
        .slots
        .get(load.current)
        .and_then(|s| data.items.get(s.item))
        .and_then(|i| i.weapon.as_ref())
        .is_some_and(|w| is_melee(w.kind));
    let upgraded = h.level.iter().any(|l| *l > 0);
    let help = [
        (
            0,
            building > 0,
            "Make your soldiers annihilate the buildings. The building has 94% resistance against a player's attack.",
        ),
        (
            1,
            soldiers >= 3 && !melee,
            "If there are too many enemy soldiers, use your sword. Soldiers are no match for melee attacks.",
        ),
        (
            2,
            h.points >= blitz.cfg.msg.help_honor && !upgraded,
            "Upgrade using the F key. Your character will grow stronger.",
        ),
        (
            3,
            h.aura & 2 != 0,
            "You are currently near the barricade. Only half the damage will be inflicted upon you.",
        ),
        (
            4,
            h.aura & 1 != 0,
            "You are currently near the radar. Your HP/AP/Bullet will restore.",
        ),
        (
            6,
            now - blitz.skip > 5.0,
            "When there are 9/6/3 barricades left, ally reinforcements will be spawned. When everything is destroyed, the Terminator will appear.",
        ),
        (
            7,
            v.hp < v.max_hp * 0.5 && h.aura & 1 == 0,
            "You are currently injured. Stay near your radar to restore your health and regain bullets.",
        ),
    ];
    if let Some((bit, _, text)) = help
        .into_iter()
        .find(|(bit, on, _)| *on && blitz.helped >> bit & 1 == 0)
    {
        blitz.helped |= 1 << bit;
        let (sound, view) = (blitz.cfg.msg.help.clone(), blitz.cfg.msg.help_view);
        blitz.say(&sound, text, view);
    }
}

/// Plays the queued announcements one at a time, the honor-gain effects and sounds
/// (`ef_Blitz_*Honor_Gain`, `*gainhonor.wav`) and the buff effects around the player.
fn feedback(
    clock: Res<Clock>,
    time: Res<Time>,
    mut blitz: ResMut<Blitz>,
    player: Query<(&GlobalTransform, &Honor), With<Player>>,
    mut vfx: MessageWriter<Vfx>,
    mut sound: MessageWriter<PlaySound>,
) {
    let Ok((tf, h)) = player.single() else { return };
    let (at, now) = (tf.translation(), clock.elapsed);
    if now >= blitz.event.0
        && let Some((stem, text, view)) = blitz.msgs.pop_front()
    {
        let secs = if blitz.msgs.is_empty() {
            view
        } else {
            blitz.cfg.msg.delay
        };
        info!("t={now:.1} blitz: announce [{stem}] {text}");
        blitz.event = (now + secs, text);
        sound.write(PlaySound { stem, at });
    }
    for g in std::mem::take(&mut blitz.gains) {
        let (fx, stem) = match g {
            g if g < GAIN_LESS => ("ef_Blitz_LessHonor_Gain", "Blitzkrieg/lessgainhonor"),
            g if g < GAIN_MORE => ("ef_Blitz_LegularHonor_Gain", "Blitzkrieg/regulargainhonor"),
            _ => ("ef_Blitz_MoreHonor_Gain", "Blitzkrieg/moregainhonor"),
        };
        vfx.write(Vfx::Named {
            name: fx.into(),
            at: Transform::from_translation(at),
        });
        sound.write(PlaySound {
            stem: stem.into(),
            at,
        });
    }
    for (i, fx) in AURAS.into_iter().enumerate() {
        if h.aura >> i & 1 == 0 {
            blitz.aura_t[i] = 0.0;
            continue;
        }
        blitz.aura_t[i] -= time.delta_secs();
        if blitz.aura_t[i] <= 0.0 {
            blitz.aura_t[i] = AURA_SECS;
            vfx.write(Vfx::Named {
                name: fx.into(),
                at: Transform::from_translation(at),
            });
        }
    }
}

// ---------------------------------------------------------------------------------------------
// HUD

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Line {
    Honor,
    Panel,
    Banner,
    Event,
}

/// The honor counter (top right), the upgrade panel (left) and the event banner (top centre).
fn hud(
    mut commands: Commands,
    clock: Res<Clock>,
    blitz: Res<Blitz>,
    camera: Query<Entity, With<Camera3d>>,
    player: Query<&Honor, With<Player>>,
    mut lines: Query<(&Line, &mut Text)>,
) {
    if lines.is_empty() {
        let Ok(camera) = camera.single() else { return };
        commands
            .spawn((
                UiTargetCamera(camera),
                GlobalZIndex(4),
                Node {
                    position_type: PositionType::Absolute,
                    width: percent(100),
                    height: percent(100),
                    ..default()
                },
            ))
            .with_children(|r| {
                let text = |line, size, left: Val, top: Val, right: Val| {
                    (
                        line,
                        Text::new(""),
                        TextFont::from_font_size(size),
                        TextColor(Color::srgb(1.0, 0.9, 0.5)),
                        TextShadow::default(),
                        Node {
                            position_type: PositionType::Absolute,
                            left,
                            top,
                            right,
                            ..default()
                        },
                    )
                };
                r.spawn(text(Line::Honor, 22.0, px(16), px(56), auto()));
                r.spawn(text(Line::Panel, 20.0, px(24), px(200), auto()));
                r.spawn(text(Line::Banner, 30.0, percent(30), px(110), auto()));
                r.spawn(text(Line::Event, 24.0, percent(20), px(150), auto()));
            });
        return;
    }
    let Ok(h) = player.single() else { return };
    let cfg = &blitz.cfg;
    let panel = if blitz.open && clock.over.is_none() {
        let mut s = String::from("UPGRADES  (Up/Down, Enter buys, F closes)\n");
        for (i, (name, what)) in UPGRADES.iter().enumerate() {
            let l = h.level[i] as usize;
            let next = |a: &[f32; 4]| a[l.min(3)];
            let value = match i {
                0 => next(&cfg.up.dps),
                1 => next(&cfg.up.delay) * 100.0,
                2 => next(&cfg.up.hp_ap),
                3 => next(&cfg.up.fire),
                4 => next(&cfg.up.magazine) * 100.0,
                _ => next(&cfg.up.revive) * 100.0,
            };
            s += &format!(
                "{} {}  {name}: {}   step {l}/4   {}\n",
                if i == blitz.sel { ">" } else { " " },
                i + 1,
                what.replace("{}", &format!("{value:.0}")),
                if l < 4 {
                    format!("{:.0} honor", cfg.up.cost[l])
                } else {
                    "done".into()
                }
            );
        }
        s
    } else {
        String::new()
    };
    let banner = if clock.elapsed < blitz.banner.0 && clock.over.is_none() {
        blitz.banner.1.clone()
    } else {
        String::new()
    };
    let event = if clock.elapsed < blitz.event.0 && clock.over.is_none() {
        blitz.event.1.clone()
    } else {
        String::new()
    };
    for (line, mut t) in &mut lines {
        let want = match line {
            Line::Honor => format!(
                "HONOR {:.0}   [F] upgrades{}",
                h.points,
                h.class.map_or(String::new(), |c| format!(
                    "   {}",
                    CLASSES[c].0.to_uppercase()
                ))
            ),
            Line::Panel => panel.clone(),
            Line::Banner => banner.clone(),
            Line::Event => event.clone(),
        };
        if t.0 != want {
            t.0 = want;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The retail rule book, when the extract is present (`.local/extract`).
    #[test]
    fn rule_book_parses() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(".local/extract/system/blitzkrieg.xml");
        let Ok(xml) = std::fs::read_to_string(path) else {
            return;
        };
        let c = parse(xml.trim_start_matches('\u{feff}')).unwrap();
        assert_eq!(
            (c.start_honor, c.income_secs, c.income, c.first_kill),
            (470.0, 1.0, 2.0, 50.0)
        );
        assert_eq!((c.respawn, c.invincible, c.finish_delay), (8.0, 5.0, 8.0));
        assert_eq!((c.enhance_hp_ap, c.enhance_dps), (75.0, 60.0));
        assert!((c.building_takes - 0.07).abs() < 1e-6);
        assert_eq!(
            (c.barricade.dist, c.barricade.reduce, c.radar.delay),
            (8.0, 0.5, 0.7)
        );
        assert_eq!(c.up.cost, [250.0, 325.0, 400.0, 1200.0]);
        assert_eq!(c.up.hp_ap, [65.0, 65.0, 65.0, 195.0]);
        assert_eq!(c.weapon["smg"], (1.1, 85.0));
        assert_eq!(c.by_type["terminator"], (50.0, 150.0));
        assert_eq!(c.kill, [50.0, 25.0, 50.0, 100.0, 5.0, 5.0]);
        assert_eq!((c.crate_respawn, c.crates.len()), (120.0, 4));
        assert_eq!(
            c.reinforce
                .iter()
                .map(|r| (r.barricades, r.alliance))
                .collect::<Vec<_>>(),
            [(9, true), (6, true), (3, true), (0, false)]
        );
        assert_eq!(c.reinforce[3].state, "summon_terminator");
        assert_eq!(c.spawns.len(), 6);
        assert_eq!(c.spawns[2], ("barricade_red".into(), 12, 2));
        assert_eq!(c.routes.len(), 8);
        assert_eq!(c.routes[0].0, 100);
        assert_eq!(
            c.routes[0].1.first().map(String::as_str),
            Some("route_top_2")
        );
        assert_eq!(c.routes[0].1.len(), 7);
        // Classes, the class screen, the leave income, the reward and the messages.
        assert_eq!(c.class_select, 30.0);
        assert_eq!(c.class.len(), 9);
        assert_eq!(c.class_val(Some(3), "distance"), 800.0);
        assert_eq!(c.class_val(Some(0), "enhanceMeleeDPS"), 60.0);
        assert_eq!(c.class_val(None, "addMaxApHp"), 0.0);
        // Six classes have a book, the other three (Hunter, Slaughter, Trickster) only a row.
        assert_eq!(c.book.len(), 6);
        for (i, (_, element, book)) in CLASSES.iter().enumerate() {
            assert!(c.class.contains_key(*element), "{element}");
            assert_eq!(
                book.map(|b| c.book.iter().find(|x| x.0 == b).unwrap().1),
                (i < 6).then_some(900000 + i as u32)
            );
        }
        assert_eq!(c.class_val(Some(6), "aquirHonorRatio"), 0.2);
        assert_eq!(c.class_val(Some(7), "addMagazineRatio"), 0.4);
        assert_eq!(c.class_val(Some(8), "recoveryMagazineRatio"), 1.0);
        assert_eq!(c.leave, [3.0, 4.0, 8.0]);
        assert_eq!([5, 3, 2, 1].map(|n| c.income_for(n)), [2.0, 3.0, 4.0, 8.0]);
        assert_eq!((c.reward.min_time, c.reward.min_honor), (420.0, 2000.0));
        assert_eq!(c.reward.mvp_lose, [0.45; 3]);
        assert_eq!(c.msg.benefit, "Blitzkrieg/EventBenefit");
        assert_eq!(
            (c.msg.radar_cooldown, c.msg.help_dist, c.msg.help_honor),
            (2.0, 5.0, 300.0)
        );
        // The reward: the MVP of the winners after 10 minutes, a loser's, and the minimums.
        let r = &c.reward;
        let p = payout(r, true, true, 600.0, 2500.0);
        assert_eq!((p.xp, p.bounty, p.medals), (575, 575, 29));
        let p = payout(r, false, false, 600.0, 2500.0);
        assert_eq!((p.xp, p.bounty, p.medals), (500, 500, 15));
        assert_eq!(payout(r, true, false, 2400.0, 9000.0).medals, 35);
        assert!(payout(r, true, false, 419.0, 9000.0).none.is_some());
        assert!(payout(r, true, false, 900.0, 1999.0).none.is_some());
    }
}
