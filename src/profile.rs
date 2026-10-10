//! The local offline profile that replaces the retail server account: name, character,
//! level/XP, bounty, inventory and equipped items, saved as a small `key=value` text file under
//! the platform data dir (`$XDG_DATA_HOME/gunzrs/profile.txt`, `%APPDATA%\gunzrs\profile.txt`;
//! `GUNZ_PROFILE=PATH` overrides it). Headless `--shot` runs use a throwaway default profile
//! unless `GUNZ_PROFILE` is set, so shots stay reproducible. Formats and constants:
//! `docs/formats.md` (profile, grank.xml). Shop/inventory rules live in `shop.rs`.

use crate::{
    actor::{ActorData, DEFAULT_LOADOUT},
    character::Look,
    clan::Clan,
    game::{Killed, Player, QuestLoot, Reward, Settings, Vitals},
    level::Level,
    menu::Mode,
    mrs::Vfs,
    session::{Clock, Rules, StartVitals},
    shop::ShopData,
    view::Shot,
};
use bevy::prelude::*;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs, io,
    path::PathBuf,
    str::FromStr,
    time::Duration,
};

/// Time since the Unix epoch (the browser's clock in the web build, where `SystemTime` panics).
pub fn wall() -> Duration {
    #[cfg(target_arch = "wasm32")]
    return Duration::from_secs_f64(js_sys::Date::now() / 1e3);
    #[cfg(not(target_arch = "wasm32"))]
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
}

/// Wall-clock Unix seconds (rental expiries).
pub fn now() -> u64 {
    wall().as_secs()
}

/// Highest level (**inferred**; `grank.xml` lists 35 ranks but no level numbers).
pub const MAX_LEVEL: u32 = 99;
/// Bounty of a new profile (**inferred**: enough for one mid-priced weapon, `gshop.xml` weapons
/// cost 8100..81000).
pub const START_BOUNTY: u32 = 20_000;
/// XP and bounty per kill (**inferred**, scaled from `mission.xml`: a daily "kill 5-20" mission
/// pays EXP 100 / BOUNTY 100, so about 10 each per kill).
pub const KILL_XP: u32 = 10;
pub const KILL_BOUNTY: u32 = 10;
/// Equipment slots: melee, primary, secondary, item, then head, chest, hands, legs, feet.
pub const SLOTS: usize = 9;

/// XP needed to go from `level` to the next one (**inferred**: linear, 100 per level).
pub fn level_cost(level: u32) -> u64 {
    100 * level as u64
}

/// `(level, xp into it, xp it needs)` of a total XP; the last level needs 0 (no next one).
pub fn progress(xp: u64) -> (u32, u64, u64) {
    let (mut level, mut left) = (1, xp);
    while level < MAX_LEVEL && left >= level_cost(level) {
        left -= level_cost(level);
        level += 1;
    }
    (
        level,
        left,
        if level < MAX_LEVEL {
            level_cost(level)
        } else {
            0
        },
    )
}

/// XP and bounty for finishing a match, by the headline of `Clock::over` (**inferred**).
pub fn result_reward(headline: &str) -> (u32, u32) {
    match headline {
        "VICTORY" => (50, 50),
        "DRAW" => (25, 25),
        _ => (10, 10),
    }
}

/// The `Settings` bools the pause menu toggles: profile key (also the `--KEY`/`--no-KEY` flag
/// with `_` as `-`) and label. Order = [`opt_mut`]. `hit_sound` plays
/// `<profile dir>/custom/hitsound.wav` ([`Profile::hitsound_path`]).
pub const OPTS: [(&str, &str); 6] = [
    ("kill_sounds", "Kill sounds"),
    ("hit_sound", "Hit sound"),
    ("static_spread", "Fixed spread"),
    ("team_bars", "Team bars"),
    ("screen_blood", "Screen blood"),
    ("killcam", "Killcam"),
];

pub fn opt_mut(s: &mut Settings, i: usize) -> &mut bool {
    [
        &mut s.kill_sounds,
        &mut s.hit_sound,
        &mut s.static_spread,
        &mut s.team_bars,
        &mut s.screen_blood,
        &mut s.killcam,
    ]
    .into_iter()
    .nth(i)
    .unwrap()
}

#[derive(Resource, Clone, Debug, PartialEq)]
pub struct Profile {
    pub name: String,
    pub woman: bool,
    /// What the character wears.
    pub look: Look,
    /// Total XP; the level follows from it ([`progress`]).
    pub xp: u64,
    pub bounty: u32,
    /// zitem ids owned.
    pub owned: BTreeSet<u32>,
    /// zitem id per slot ([`SLOTS`]); 0 = empty (only item and armour slots may be empty).
    pub equipped: [u32; SLOTS],
    /// `zquestitem.xml` id -> how many are kept (quest drops: [`QuestLoot`]; sacrifices spend them).
    pub quest_items: BTreeMap<u32, u32>,
    /// Blitzkrieg medals earned ([`Reward::medals`]).
    pub medals: u32,
    /// Rented items (also in `owned`): zitem id -> expiry in Unix seconds ([`Profile::rent`]).
    pub rented: BTreeMap<u32, u64>,
    /// The offline clan (`clan.rs`); `None` = not in one.
    pub clan: Option<Clan>,
    /// The pause-menu toggles ([`OPTS`] order), saved as `opt_NAME=0|1`.
    pub opts: [bool; OPTS.len()],
    /// Where [`Profile::save`] writes; `None` = throwaway.
    path: Option<PathBuf>,
}

impl Profile {
    pub fn new() -> Self {
        let name = ["USER", "USERNAME"]
            .iter()
            .find_map(|k| std::env::var(k).ok().filter(|v| !v.trim().is_empty()))
            .unwrap_or_else(|| "Player".into());
        let mut equipped = [0; SLOTS];
        equipped[..3].copy_from_slice(&DEFAULT_LOADOUT);
        Self {
            name,
            woman: false,
            look: Look::default(),
            xp: 0,
            bounty: START_BOUNTY,
            owned: DEFAULT_LOADOUT.into_iter().collect(),
            equipped,
            quest_items: BTreeMap::new(),
            medals: 0,
            rented: BTreeMap::new(),
            opts: std::array::from_fn(|i| *opt_mut(&mut Settings::default(), i)),
            clan: None,
            path: None,
        }
    }

    fn default_path() -> Option<PathBuf> {
        let base = if cfg!(windows) {
            PathBuf::from(std::env::var_os("APPDATA")?)
        } else {
            match std::env::var_os("XDG_DATA_HOME").map(PathBuf::from) {
                Some(p) if p.is_absolute() => p,
                _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
            }
        };
        Some(base.join("gunzrs").join("profile.txt"))
    }

    /// The saved profile (a new one if the file does not exist). Panics on an unreadable or
    /// corrupt file rather than overwrite it later. `headless`: no file unless `GUNZ_PROFILE`.
    pub fn open(headless: bool) -> Self {
        // the browser keeps the profile in the page's local storage
        #[cfg(target_arch = "wasm32")]
        if !headless {
            return crate::web::load_profile()
                .and_then(|t| Self::parse(&t).map_err(|e| eprintln!("profile: {e}")).ok())
                .unwrap_or_else(Self::new);
        }
        let path = std::env::var_os("GUNZ_PROFILE")
            .map(PathBuf::from)
            .or_else(|| if headless { None } else { Self::default_path() });
        let Some(path) = path else {
            return Self::new();
        };
        let mut p = match fs::read_to_string(&path) {
            Ok(text) => Self::parse(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Self::new(),
            Err(e) => panic!("{}: {e}", path.display()),
        };
        for id in p.expire(now()) {
            println!("profile: rental of item {id} expired and was removed");
        }
        p.path = Some(path);
        p
    }

    pub fn level(&self) -> u32 {
        progress(self.xp).0
    }

    pub fn add(&mut self, xp: u32, bounty: u32) {
        self.xp += xp as u64;
        self.bounty = self.bounty.saturating_add(bounty);
    }

    pub fn to_text(&self) -> String {
        let ids =
            |v: &mut dyn Iterator<Item = &u32>| v.map(u32::to_string).collect::<Vec<_>>().join(",");
        let mut text = format!(
            "name={}\nwoman={}\nlook={}\nxp={}\nbounty={}\nmedals={}\nowned={}\nequipped={}\nquest_items={}\nrented={}\n",
            self.name.replace('\n', " "),
            self.woman,
            self.look,
            self.xp,
            self.bounty,
            self.medals,
            ids(&mut self.owned.iter()),
            ids(&mut self.equipped.iter()),
            self.quest_items
                .iter()
                .map(|(id, n)| format!("{id}:{n}"))
                .collect::<Vec<_>>()
                .join(","),
            self.rented
                .iter()
                .map(|(id, t)| format!("{id}:{t}"))
                .collect::<Vec<_>>()
                .join(","),
        );
        for (o, on) in OPTS.iter().zip(self.opts) {
            text += &format!("opt_{}={}\n", o.0, on as u8);
        }
        if let Some(c) = &self.clan {
            text += &format!("clan={}\n", c.to_text());
        }
        text
    }

    /// Parses [`Profile::to_text`]; missing keys keep the new-profile values, unknown keys and
    /// bad values are errors.
    pub fn parse(text: &str) -> Result<Self, String> {
        fn num<T: FromStr>(k: &str, v: &str) -> Result<T, String> {
            v.trim()
                .parse()
                .map_err(|_| format!("{k}: bad value {v:?}"))
        }
        fn list(k: &str, v: &str) -> Result<Vec<u32>, String> {
            v.split(',')
                .filter(|s| !s.trim().is_empty())
                .map(|s| num(k, s))
                .collect()
        }
        let mut p = Self::new();
        for line in text
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
        {
            let (k, v) = line
                .split_once('=')
                .ok_or_else(|| format!("bad line {line:?}"))?;
            match k.trim() {
                "name" => p.name = v.trim().to_owned(),
                "woman" => p.woman = num(k, v)?,
                "look" => p.look = v.trim().parse()?,
                // older profiles: one part set for every slot (slots it lacks keep the base)
                "outfit" => {
                    p.look = match v.trim() {
                        "none" => Look::default(),
                        n => Look {
                            parts: [Some(num(k, n)?); 6],
                            ..default()
                        },
                    }
                }
                "xp" => p.xp = num(k, v)?,
                "bounty" => p.bounty = num(k, v)?,
                "medals" => p.medals = num(k, v)?,
                "owned" => p.owned = list(k, v)?.into_iter().collect(),
                "equipped" => {
                    p.equipped = list(k, v)?
                        .try_into()
                        .map_err(|_| format!("equipped: need {SLOTS} ids"))?
                }
                "quest_items" | "rented" => {
                    let pairs = v
                        .split(',')
                        .filter(|s| !s.trim().is_empty())
                        .map(|s| {
                            let (id, n) =
                                s.split_once(':').ok_or(format!("{k}: bad item {s:?}"))?;
                            Ok((num(k, id)?, num(k, n)?))
                        })
                        .collect::<Result<Vec<(u32, u64)>, String>>()?;
                    if k.trim() == "rented" {
                        p.rented = pairs.into_iter().collect();
                    } else {
                        p.quest_items = pairs
                            .into_iter()
                            .map(|(i, n)| {
                                let n =
                                    u32::try_from(n).map_err(|_| format!("{k}: bad count {n}"))?;
                                Ok((i, n))
                            })
                            .collect::<Result<_, String>>()?;
                    }
                }
                "clan" => p.clan = Some(Clan::parse(v)?),
                k if k.starts_with("opt_") => {
                    let i = OPTS.iter().position(|o| o.0 == &k[4..]);
                    p.opts[i.ok_or(format!("unknown key {k:?}"))?] = num::<u8>(k, v)? != 0;
                }
                _ => return Err(format!("unknown key {k:?}")),
            }
        }
        Ok(p)
    }

    /// Writes the file (via a temp file, so a crash never leaves half a profile); a no-op for
    /// throwaway profiles. Errors are reported, not fatal.
    pub fn save(&self) {
        #[cfg(target_arch = "wasm32")]
        if self.path.is_none() {
            crate::web::store_profile(&self.to_text());
            return;
        }
        let Some(path) = &self.path else { return };
        let tmp = path.with_extension("tmp");
        let done = path
            .parent()
            .map_or(Ok(()), fs::create_dir_all)
            .and_then(|()| fs::write(&tmp, self.to_text()))
            .and_then(|()| fs::rename(&tmp, path));
        if let Err(e) = done {
            eprintln!("profile: cannot save {}: {e}", path.display());
        }
    }

    /// `<profile dir>/custom/hitsound.wav`, the player's own hit sound (`None`: throwaway profile).
    pub fn hitsound_path(&self) -> Option<PathBuf> {
        Some(self.path.as_ref()?.parent()?.join("custom/hitsound.wav"))
    }

    /// Copies the saved toggles into `s`.
    pub fn apply(&self, s: &mut Settings) {
        for (i, &on) in self.opts.iter().enumerate() {
            *opt_mut(s, i) = on;
        }
    }

    /// Keeps quest items (a finished quest's [`QuestLoot`]).
    pub fn add_quest_items(&mut self, items: &[(u32, u32)]) {
        for &(id, n) in items {
            *self.quest_items.entry(id).or_default() += n;
        }
    }

    /// Spends one of each of `items`; the caller checked that they are there.
    pub fn spend_quest_items(&mut self, items: &[u32]) {
        for id in items {
            if let Some(n) = self.quest_items.get_mut(id) {
                *n -= 1;
                if *n == 0 {
                    self.quest_items.remove(id);
                }
            }
        }
    }

    /// Rents shop item `id` for `hours` (`rent_period`) from `now`. An item owned for good stays
    /// as it is (`false`); renting one again keeps the later expiry.
    pub fn rent(&mut self, id: u32, hours: u32, now: u64) -> bool {
        if self.owned.contains(&id) && !self.rented.contains_key(&id) {
            return false;
        }
        let until = now + hours as u64 * 3600;
        let e = self.rented.entry(id).or_default();
        *e = (*e).max(until);
        self.owned.insert(id);
        true
    }

    /// Removes the rentals that ran out by `now` (also from the equipment: a melee or ranged slot
    /// falls back to its starter weapon). Returns their ids.
    pub fn expire(&mut self, now: u64) -> Vec<u32> {
        let gone: Vec<u32> = self
            .rented
            .iter()
            .filter(|&(_, &t)| t <= now)
            .map(|(&id, _)| id)
            .collect();
        for id in &gone {
            self.rented.remove(id);
            self.owned.remove(id);
            for (slot, e) in self.equipped.iter_mut().enumerate() {
                if *e == *id {
                    *e = DEFAULT_LOADOUT.get(slot).copied().unwrap_or(0);
                    self.owned.extend((*e != 0).then_some(*e));
                }
            }
        }
        gone
    }
}

impl Default for Profile {
    fn default() -> Self {
        Self::new()
    }
}

/// `ADR_NAME` of every `system/grank.xml` rank, in ID order.
#[derive(Resource, Default)]
pub struct Ranks(pub Vec<String>);

impl Ranks {
    pub fn load(vfs: &Vfs) -> io::Result<Self> {
        let bytes = vfs.read("system/grank.xml")?;
        let bad = |e: String| io::Error::new(io::ErrorKind::InvalidData, format!("grank.xml: {e}"));
        let text = String::from_utf8(bytes).map_err(|e| bad(e.to_string()))?;
        let doc = roxmltree::Document::parse(text.trim_start_matches('\u{feff}'))
            .map_err(|e| bad(e.to_string()))?;
        let mut ranks: Vec<(u32, String)> = Vec::new();
        for n in doc
            .root_element()
            .children()
            .filter(|n| n.has_tag_name("RANK"))
        {
            let id = n.attribute("ID").and_then(|v| v.parse().ok());
            let (Some(id), Some(code)) = (id, n.attribute("ADR_NAME")) else {
                return Err(bad("RANK without ID/ADR_NAME".into()));
            };
            ranks.push((id, code.to_owned()));
        }
        ranks.sort();
        Ok(Self(ranks.into_iter().map(|(_, c)| c).collect()))
    }

    /// Rank code of a level: the ranks are spread evenly over 1..=[`MAX_LEVEL`] (**inferred**).
    pub fn code(&self, level: u32) -> &str {
        let n = self.0.len();
        let i = (level.saturating_sub(1) as usize * n.saturating_sub(1)) / (MAX_LEVEL - 1) as usize;
        self.0
            .get(i.min(n.saturating_sub(1)))
            .map_or("", String::as_str)
    }
}

/// What this match has earned so far (the scoreboard shows it once the match is over).
#[derive(Resource, Default, Clone, Debug)]
pub struct MatchGain {
    pub xp: u32,
    pub bounty: u32,
    /// The player's level when the match began.
    pub from_level: u32,
    /// The result bonus was paid.
    pub done: bool,
}

pub struct ProfilePlugin;

impl Plugin for ProfilePlugin {
    fn build(&self, app: &mut App) {
        let profile = Profile::open(app.world().contains_resource::<Shot>());
        app.insert_resource(MatchGain {
            from_level: profile.level(),
            ..default()
        })
        .insert_resource(profile)
        .add_systems(Startup, load_ranks)
        .add_systems(Update, (gear_bonus, earn, keep_loot, finish, save).chain());
    }
}

fn load_ranks(mut commands: Commands, level: Res<Level>) {
    commands.insert_resource(Ranks::load(&level.vfs).unwrap_or_else(|e| panic!("profile: {e}")));
}

/// A finished quest's drops stay in the profile: `zquestitem.xml` ids (six digits) as quest
/// items, shop items (zitem ids; all 21 permanent `droptable.xml` drops exist there, **observed**)
/// owned for good, rented shop items with their expiry ([`Profile::rent`]).
fn keep_loot(mut loot: MessageReader<QuestLoot>, mut profile: ResMut<Profile>) {
    for l in loot.read() {
        let (kept, shop): (Vec<_>, Vec<_>) = l.items.iter().partition(|i| i.0 < 1_000_000);
        if !kept.is_empty() {
            profile.add_quest_items(&kept);
            println!("profile: kept quest items {kept:?}");
        }
        for &(id, _) in &shop {
            profile.rented.remove(&id);
            profile.owned.insert(id);
            println!("profile: item {id} owned for good");
        }
        for &(id, hours) in &l.rented {
            let new = profile.rent(id, hours, now());
            println!("profile: rented item {id} for {hours} h (new: {new}, owned for good if not)");
        }
    }
}

/// Equipped armour adds its `hp`/`ap` to the player's maxima (zitem.xml `hp`/`ap`).
fn gear_bonus(
    profile: Res<Profile>,
    level: Res<Level>,
    data: Res<ActorData>,
    start: Option<Res<StartVitals>>,
    mut q: Query<&mut Vitals, Added<Player>>,
) {
    if q.is_empty() || profile.equipped[4..].iter().all(|&i| i == 0) {
        return;
    }
    let shop = ShopData::load(&level.vfs, &data.items)
        .unwrap_or_else(|e| panic!("profile: shop data: {e}"));
    let (hp, ap) = profile.equipped[4..]
        .iter()
        .filter_map(|id| shop.entries.get(id))
        .fold((0, 0), |(h, a), e| (h + e.hp, a + e.ap));
    for mut v in &mut q {
        v.max_hp += hp as f32;
        v.max_ap += ap as f32;
        if start.is_none() {
            (v.hp, v.ap) = (v.max_hp, v.max_ap);
        }
    }
}

/// Kills by the player and [`Reward`]s (quest payouts) pay out as they happen.
fn earn(
    mut killed: MessageReader<Killed>,
    mut rewards: MessageReader<Reward>,
    player: Query<Entity, With<Player>>,
    rules: Option<Res<Rules>>,
    clock: Res<Clock>,
    mut profile: ResMut<Profile>,
    mut gain: ResMut<MatchGain>,
) {
    let (mut xp, mut bounty, mut medals) = (0, 0, 0);
    for r in rewards.read() {
        xp += r.xp;
        bounty += r.bounty;
        medals += r.medals;
    }
    let training = rules.is_some_and(|r| r.mode == Mode::Training);
    let over = clock.over.is_some();
    for k in killed.read() {
        if !training
            && !over
            && k.killer != k.victim
            && player.single().is_ok_and(|p| p == k.killer)
        {
            xp += KILL_XP;
            bounty += KILL_BOUNTY;
        }
    }
    if medals > 0 {
        profile.medals = profile.medals.saturating_add(medals);
        println!("profile: +{medals} medals -> {}", profile.medals);
    }
    if xp + bounty > 0 {
        profile.add(xp, bounty);
        gain.xp += xp;
        gain.bounty += bounty;
    }
}

/// Pays the result bonus once when the match ends and logs what the match earned.
fn finish(clock: Res<Clock>, mut profile: ResMut<Profile>, mut gain: ResMut<MatchGain>) {
    let Some(headline) = &clock.over else { return };
    if gain.done {
        return;
    }
    let (xp, bounty) = result_reward(headline);
    profile.add(xp, bounty);
    gain.xp += xp;
    gain.bounty += bounty;
    gain.done = true;
    info!(
        "match {headline}: +{} XP, +{} bounty -> level {} (was {}), bounty {}",
        gain.xp,
        gain.bounty,
        profile.level(),
        gain.from_level,
        profile.bounty
    );
}

/// Saves the profile whenever it changed (a kill, a purchase): the file is a few hundred bytes.
pub fn save(profile: Res<Profile>) {
    if profile.is_changed() {
        profile.save();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_and_file_round_trip() {
        let mut p = Profile::new();
        p.name = "Tester = 1".into();
        p.woman = true;
        p.look = "12,0,3,0,0,7;1,0,0,19,0,0".parse().unwrap();
        p.add(1234, 77);
        p.owned.insert(2000000);
        p.equipped[4] = 3010013;
        p.quest_items.insert(200008, 2);
        p.medals = 42;
        assert!(p.rent(3000042, 72, 1_000));
        assert!(!p.rent(DEFAULT_LOADOUT[0], 72, 1_000), "owned for good");
        p.clan = Some(Clan {
            name: "Phoenix 1".into(),
            emblem: 1000005,
            bg: 2000003,
            points: 1042,
            wins: 3,
            losses: 1,
            members: vec!["Ash".into(), "Bolt".into(), "Cinder".into()],
        });
        assert_eq!(Profile::parse(&p.to_text()).unwrap().to_text(), p.to_text());
        let path = std::env::temp_dir().join(format!("gunzrs-profile-{}.txt", std::process::id()));
        p.path = Some(path.clone());
        p.save();
        let text = fs::read_to_string(&path).unwrap();
        fs::remove_file(&path).unwrap();
        let mut back = Profile::parse(&text).unwrap();
        back.path = p.path.clone();
        assert_eq!(back, p);
        let mut p = Profile::new();
        p.opts = [false, true, true, false, false, true];
        let q = Profile::parse(&p.to_text()).unwrap();
        assert!(p.to_text().contains("opt_hit_sound=1\n") && q.opts == p.opts);
        let mut s = Settings::default();
        q.apply(&mut s);
        assert!(!s.kill_sounds && s.hit_sound && s.static_spread && !s.team_bars && s.killcam);
        assert!(Profile::parse("bogus=1").is_err());
        assert!(Profile::parse("equipped=1,2").is_err());
        assert!(Profile::parse("clan=Phoenix|1|2|3").is_err());
        assert!(Profile::parse("rented=1").is_err());
    }

    /// Rentals expire against the wall clock: the item leaves `owned` and the equipment.
    #[test]
    fn rentals_expire() {
        let mut p = Profile::new();
        assert!(p.rent(2120016, 168, 1_000));
        p.rent(2120016, 72, 1_000); // a shorter rental never shortens it
        assert_eq!(p.rented[&2120016], 1_000 + 168 * 3600);
        p.equipped[1] = 2120016;
        assert!(p.expire(1_000 + 168 * 3600 - 1).is_empty());
        assert_eq!(p.expire(1_000 + 168 * 3600), [2120016]);
        assert!(!p.owned.contains(&2120016) && p.rented.is_empty());
        assert_eq!(p.equipped[1], DEFAULT_LOADOUT[1]);
        assert!(p.owned.contains(&DEFAULT_LOADOUT[1]));
    }

    #[test]
    fn level_table() {
        assert_eq!(progress(0), (1, 0, 100));
        assert_eq!(progress(100), (2, 0, 200));
        assert_eq!(progress(299), (2, 199, 200));
        assert_eq!(progress(u32::MAX as u64).0, MAX_LEVEL);
        let ranks = Ranks((1..=35).map(|i| i.to_string()).collect());
        assert_eq!((ranks.code(1), ranks.code(MAX_LEVEL)), ("1", "35"));
    }

    /// A finished quest's quest items are kept (shop items are not) and a sacrifice spends them.
    #[test]
    fn quest_loot_is_kept_and_sacrifices_are_spent() {
        let mut app = App::new();
        app.add_message::<QuestLoot>()
            .insert_resource(Profile::new())
            .add_systems(Update, keep_loot);
        for items in [vec![(200011, 2), (3000042, 1)], vec![(200011, 1)]] {
            app.world_mut()
                .resource_mut::<Messages<QuestLoot>>()
                .write(QuestLoot {
                    items,
                    rented: vec![],
                });
            app.update();
        }
        let mut p = app.world().resource::<Profile>().clone();
        assert_eq!(p.quest_items, BTreeMap::from([(200011, 3)]));
        assert!(p.owned.contains(&3000042), "a permanent shop drop is owned");
        assert_eq!(
            Profile::parse(&p.to_text()).unwrap().quest_items,
            p.quest_items
        );
        p.spend_quest_items(&[200011, 200011, 200011]);
        assert!(p.quest_items.is_empty());
    }

    /// Kills by the player pay, other kills do not; the result bonus is paid once at the end.
    #[test]
    fn kills_and_result_pay_out() {
        let mut app = App::new();
        app.add_message::<Killed>()
            .add_message::<Reward>()
            .init_resource::<Clock>()
            .insert_resource(Profile::new())
            .insert_resource(MatchGain::default())
            .add_systems(Update, (earn, finish).chain());
        let player = app.world_mut().spawn(Player).id();
        let (bot, other) = (
            app.world_mut().spawn_empty().id(),
            app.world_mut().spawn_empty().id(),
        );
        let start = app.world().resource::<Profile>().clone();
        let mut kill = |killer, victim| {
            app.world_mut()
                .resource_mut::<Messages<Killed>>()
                .write(Killed {
                    victim,
                    killer,
                    item: 0,
                    head: false,
                });
        };
        kill(player, bot);
        kill(player, player);
        kill(other, bot);
        app.world_mut()
            .resource_mut::<Messages<Reward>>()
            .write(Reward {
                xp: 5,
                bounty: 7,
                medals: 3,
            });
        app.update();
        let p = app.world().resource::<Profile>();
        assert_eq!((p.xp, p.bounty), (start.xp + 15, start.bounty + 17));
        assert_eq!(p.medals, start.medals + 3);
        app.world_mut().resource_mut::<Clock>().over = Some("VICTORY".into());
        app.update();
        app.update();
        let p = app.world().resource::<Profile>();
        let (xp, bounty) = result_reward("VICTORY");
        assert_eq!(
            (p.xp, p.bounty),
            (start.xp + 15 + xp as u64, start.bounty + 17 + bounty)
        );
        assert!(app.world().resource::<MatchGain>().done);
    }
}
