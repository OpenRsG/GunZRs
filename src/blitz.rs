//! Blitzkrieg (`--mode blitzkrieg`, retail id 13, map `blitzkrieg`): two sides, each with a radar
//! that sends waves of soldiers down the `route_*` lanes, twelve barricades, a guardian and the
//! players and bots in between. `system/blitzkrieg.xml` is the rule book ([`Cfg`]); the soldiers,
//! radars and barricades are `npc2.xml` actors driven by `aifsm.xml` through `npc.rs` (they march
//! along [`Routes`], fight whatever is hostile and a radar's machine summons the waves). This
//! module places the objectives, pays and spends honor, applies the building rules, calls the
//! reinforcements and ends the match. Rules *inferred* rather than read are marked; the long form
//! is `docs/formats.md` "Blitzkrieg".
//!
//! Controls: `F` opens the upgrade panel, Up/Down choose, Enter buys (the honor is spent per step
//! of [`Cfg::up`]); bots buy by themselves. `GUNZ_BLITZ_BUY="SECS:N,.."` buys upgrade N (1-6) for
//! the player at match second SECS and `GUNZ_BLITZ_HP=K` scales the objectives' health (headless
//! checks).

use crate::{
    actor::{ActorData, PlayerSetup},
    bot::BotAhead,
    col::MapCollision,
    combat::yaw_of,
    game::{
        Afflict, Bot, Damage, Dead, Frozen, Killed, Loadout, Mods, Npc, NpcState, Player, Routes,
        Score, SpawnNpc, Team, Vitals,
    },
    item::WeaponKind,
    level::Level,
    menu::Mode,
    session::{Clock, PROTECT_SECS, RESPAWN_SECS, Rules, freeze},
    view::{SCALE, to_bevy},
};
use bevy::{
    prelude::*,
    ui::{GlobalZIndex, UiTargetCamera},
};
use roxmltree::{Document, Node as Xml};
use std::collections::HashMap;

/// Seconds a banner (reinforcements, a purchase, an honor gain) stays up. *Inferred* from the
/// `EVENT_MESSAGE viewTime="4"`.
const BANNER_SECS: f32 = 4.0;
/// Seconds a fire enchant burns (`UPGRADE fireDamageDuration`, observed 4.0).
const FIRE_SECS: f32 = 4.0;
/// Vertical reach (m) of a radar's or barricade's zone. *Inferred* (the file gives a distance).
const ZONE_HEIGHT: f32 = 6.0;
/// What a bot buys first, as indices into [`UPGRADES`] (*inferred*: toughness, then damage).
const BOT_ORDER: [usize; 6] = [2, 0, 1, 4, 3, 5];

pub struct BlitzPlugin;

impl Plugin for BlitzPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, setup.run_if(on)).add_systems(
            Update,
            (
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
                crates,
                finish,
                hud,
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

/// The parts of `system/blitzkrieg.xml` this mode plays by (the class table, the medal rewards,
/// the quit penalty and the event messages are not modelled).
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
    })
}

// ---------------------------------------------------------------------------------------------
// State

/// Honor an actor holds, what it earned in all (the kill formula uses it) and its upgrade steps.
#[derive(Component, Debug)]
struct Honor {
    points: f32,
    total: f32,
    level: [u8; 6],
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

fn setup(
    mut commands: Commands,
    level: Res<Level>,
    col: Res<MapCollision>,
    mut rules: ResMut<Rules>,
    mut routes: ResMut<Routes>,
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
    info!(
        "blitz: {} routes, {} crates, respawn {} s, protection {} s, honor {} +{}/{} s",
        routes.paths.len(),
        crate_at.len(),
        rules.respawn,
        rules.protect,
        cfg.start_honor,
        cfg.income,
        cfg.income_secs
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
    for (e, n) in &buildings {
        if matches!(n.kind.as_str(), "barricade" | "radar") {
            commands.entity(e).insert(Mods {
                vs_actors: cfg.building_takes,
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
                points: cfg.start_honor,
                total: cfg.start_honor,
                level: [0; 6],
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
    mut q: Query<&mut Honor, Without<Dead>>,
) {
    blitz.income_t += time.delta_secs();
    let every = blitz.cfg.income_secs.max(0.1);
    while blitz.income_t >= every {
        blitz.income_t -= every;
        for mut h in &mut q {
            h.points += blitz.cfg.income;
            h.total += blitz.cfg.income;
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
        let fire = honors
            .get(d.attacker)
            .map_or(0.0, |h| sum(&blitz.cfg.up.fire, h.1.level[3]));
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
            let g = own.iter().find(|o| o.0 == e).map_or(0.0, |o| o.1)
                + if t == Some(&by) { all } else { 0.0 };
            if g > 0.0 {
                h.points += g;
                h.total += g;
                gains += &format!(" {name} +{g:.0}");
                if player {
                    blitz.banner = (now + BANNER_SECS / 2.0, format!("+{g:.0} HONOR"));
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

/// What upgrades and buildings do to an actor's blows, shots and wounds: the extra DPS (the base
/// `ENHANCE_PLAYER dps` plus the steps) becomes `dps * factor * delay` more per hit of the
/// current weapon (*inferred* reading of `WEAPON`), a rapid-fire step shortens the gun delay,
/// and inside a friendly barricade's zone damage is cut (message 2124: "only half").
fn buffs(
    blitz: Res<Blitz>,
    data: Res<ActorData>,
    walls: Query<(&Npc, &Team, &GlobalTransform), Without<Dead>>,
    mut q: Query<(&Honor, &mut Mods, &Loadout, &Transform, Option<&Team>), Without<Dead>>,
) {
    let cfg = &blitz.cfg;
    for (h, mut m, load, tf, team) in &mut q {
        let extra = cfg.enhance_dps + sum(&cfg.up.dps, h.level[0]);
        let slot = load.slots.get(load.current);
        let w = slot
            .and_then(|s| data.items.get(s.item))
            .and_then(|i| i.weapon.as_ref());
        m.dealt = w
            .and_then(|w| {
                let (factor, delay) = cfg.weapon.get(table_key(w.kind)?)?;
                Some(1.0 + extra * factor * delay * 0.001 / (w.damage as f32).max(1.0))
            })
            .unwrap_or(1.0);
        m.shot_delay = 1.0 / (1.0 + sum(&cfg.up.delay, h.level[1]));
        let near = team.is_some_and(|t| {
            walls.iter().any(|(n, wt, g)| {
                n.kind == "barricade"
                    && wt == t
                    && within(g.translation(), tf.translation, cfg.barricade.dist)
            })
        });
        m.taken = if near {
            1.0 - cfg.barricade.reduce
        } else {
            1.0
        };
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

/// A respawn refills the guns from scratch: the magazine steps stretch the spare rounds again.
fn rearm(
    mut back: RemovedComponents<Dead>,
    blitz: Res<Blitz>,
    mut q: Query<(&Honor, &mut Loadout)>,
) {
    for e in back.read() {
        if let Ok((h, mut load)) = q.get_mut(e) {
            let k = 1.0 + sum(&blitz.cfg.up.magazine, h.level[4]);
            for s in &mut load.slots {
                s.reserve = (s.reserve as f32 * k).round() as u32;
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Buildings

/// Restores `ratio` of each gun's ammunition cap (`max_bullet`, raised by the magazine steps).
fn refill(data: &ActorData, load: &mut Loadout, ratio: f32, bonus: f32) {
    for s in &mut load.slots {
        if let Some(cap) = data
            .items
            .get(s.item)
            .and_then(|i| i.weapon.as_ref())
            .and_then(|w| w.max_bullet)
        {
            let cap = cap as f32 * (1.0 + bonus);
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
        let bonus = sum(&blitz.cfg.up.magazine, h.level[4]);
        for (n, wt, g) in &walls {
            if wt != team {
                continue;
            }
            match n.kind.as_str() {
                "radar" if heal && within(g.translation(), tf.translation, radar.dist) => {
                    v.hp = (v.hp + radar.ap_hp * v.max_hp).min(v.max_hp);
                    v.ap = (v.ap + radar.ap_hp * v.max_ap).min(v.max_ap);
                    refill(&data, &mut load, radar.mag, bonus);
                }
                "barricade" if stock && within(g.translation(), tf.translation, barricade.dist) => {
                    refill(&data, &mut load, barricade.mag, bonus);
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
    let mut banner = None;
    for t in [Team::Red, Team::Blue] {
        let s = side(t);
        for (i, r) in b.cfg.reinforce.iter().enumerate() {
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
            banner = Some((to, what.to_string()));
        }
    }
    if let Some((to, what)) = banner {
        blitz.banner = (
            clock.elapsed + BANNER_SECS,
            format!("{} reinforcements: {what}", word(to)),
        );
    }
}

/// A destroyed honor crate comes back after `HONOR_ITEM_LIST respawnTime`.
fn crates(
    time: Res<Time>,
    mut blitz: ResMut<Blitz>,
    mut kills: MessageReader<Killed>,
    npcs: Query<&Npc>,
    mut spawn: MessageWriter<SpawnNpc>,
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
}

// ---------------------------------------------------------------------------------------------
// HUD

#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Line {
    Honor,
    Panel,
    Banner,
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
    let banner = if clock.elapsed < blitz.banner.0 {
        blitz.banner.1.clone()
    } else {
        String::new()
    };
    for (line, mut t) in &mut lines {
        let want = match line {
            Line::Honor => format!("HONOR {:.0}   [F] upgrades", h.points),
            Line::Panel => panel.clone(),
            Line::Banner => banner.clone(),
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
    }
}
