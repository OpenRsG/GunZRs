//! `gunz-play` main menu (Bevy UI over a 3D character preview) and the run configuration it
//! edits. The menu is the [`Screen::Menu`] of the one app: START hands the chosen [`Config`] to
//! the loading screen (`phase.rs`); in the browser the page starts the match instead. Art comes
//! from `interface/default/` (lobby background, logo); the type and widgets are `ui.rs`'s.
//! Notes: `docs/formats.md` (menu).

use crate::{
    actor::DEFAULT_LOADOUT,
    ani::Ani,
    anim::{Animator, Loop},
    character::{self, Character, Look, Slot, TINTS, Wardrobe},
    clan::{self, ClanArt},
    dashfx::{self, HUES, Hue, STYLES, Style, Trail},
    elu,
    hud::try_image,
    item::{Items, WeaponKind},
    model::{self, Textures},
    mrs::Vfs,
    phase::{Hooks, Launch, MenuSystems, Mount, Screen},
    profile::{Profile, Ranks, progress},
    shop::{self, ShopData},
    slashfx::{self, Slash},
    ui::{
        ACCENT, Chosen, DIM, INK, LINE, RAISED, SHADOW, body, button, compact, digits, display,
        flat, heading, hover, panel, primary, shadow, strong,
    },
    view::{self, Shot},
};
use bevy::{
    mesh::skinning::SkinnedMeshInverseBindposes,
    prelude::*,
    text::{Justify, LineBreak},
    ui::{RelativeCursorPosition, UiTargetCamera},
    window::PrimaryWindow,
};
use std::{collections::HashMap, f32::consts::TAU, str::FromStr, sync::Arc};

/// Removes `flag VALUE` from `args` and parses the value; `Err` if the value is missing/bad.
pub fn take_arg<T: FromStr>(args: &mut Vec<String>, flag: &str) -> Result<Option<T>, ()> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    let v = args.get(i + 1).and_then(|v| v.parse().ok()).ok_or(())?;
    args.drain(i..=i + 1);
    Ok(Some(v))
}

/// The game modes (`system/gametypecfg.xml` ids in the comments); bots fill the other seats.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// id 0 (`GAMETYPE_DEATHMATCH_SOLO`).
    Deathmatch,
    /// id 9 (`DEATHMATCH_TEAM2`, "Death match team" in `strings.xml`): Red against Blue with
    /// respawns; team kills count.
    Team,
    /// id 2: deathmatch with melee weapons only.
    Gladiator,
    /// id 3.
    TeamGladiator,
    /// id 1 (`DEATHMATCH_TEAM`, "Elimination" in `strings.xml`): team rounds, nobody respawns
    /// until the round ends.
    Elimination,
    /// id 4: team rounds, each side hides a VIP; a round ends when one dies.
    Assassinate,
    /// id 10: one-on-one rounds, the winner stays and the loser queues.
    Duel,
    /// id 5: no bots, dummy targets.
    Training,
    /// id 7 (`GAMETYPE_QUEST`; also plays the challenge quest, id 12, and survival, id 6):
    /// the player and bot allies against scripted NPC sectors (`quest.rs`).
    Quest,
    /// id 8 (`GAMETYPE_BERSERKER`; its `gametypecfg.xml` block is commented out but the
    /// `champion` channel rule lists it): one berserker against everyone; kill it to become it.
    Berserker,
    /// id 11 (`GAMETYPE_DUELTOURNAMENT`, the `dueltournament` channel): a knockout bracket of
    /// duels, the loser is out.
    DuelTournament,
    /// id 17 (`MMATCH_GAMETYPE_RANDOM_WEAPON`, "Gunman" in `strings.xml`): deathmatch where every
    /// life brings a random melee weapon and gun.
    Gunman,
    /// id 14 (`MMATCH_GAMETYPE_SPY`, `system/spymode.xml`): hidden spies against trackers, in
    /// rounds.
    Spy,
    /// id 13 (`GAMETYPE_BLITZKRIEG`, `system/blitzkrieg.xml`; map `blitzkrieg` only): two sides,
    /// radars that send waves of soldiers down the lanes, barricades to destroy; honor buys
    /// upgrades (`blitz.rs`).
    Blitzkrieg,
    /// id 22 (`GAMETYPE_CLAN_SCRIM`, "Clan War" in `strings.xml`): the player's clan against a
    /// rival clan, 4 against 4, elimination rounds (`clan.rs`).
    ClanWar,
    /// No retail id (port design, `modes/gungame.rs`): free for all up a fixed weapon ladder,
    /// every kill swaps your weapon for the next; a kill from the last step wins.
    GunGame,
    /// No retail id (port design, `modes/infected.rs`): rounds in which a random actor turns
    /// zombie; a zombie's kill turns the survivor, who respawns as a zombie.
    Infected,
    /// No retail id (port design, `modes/dynduel.rs`): one room, several one-on-one duels at
    /// once in separate arenas; the winner stays, the loser queues and the next challenges.
    DynDuel,
}

/// The choices of one mode's limit steppers: `gametypecfg.xml`'s `ROUNDS` and `LIMITTIME`
/// lists (`0` minutes = the file's `-1`, unlimited) with the `default="true"` entries.
pub struct Limits {
    pub kills: &'static [u32],
    pub kills_default: u32,
    pub minutes: &'static [u32],
    pub minutes_default: u32,
}

impl Mode {
    pub const ALL: [Mode; 18] = [
        Mode::Deathmatch,
        Mode::Team,
        Mode::Gladiator,
        Mode::TeamGladiator,
        Mode::Elimination,
        Mode::Assassinate,
        Mode::Duel,
        Mode::Training,
        Mode::Quest,
        Mode::Berserker,
        Mode::DuelTournament,
        Mode::Gunman,
        Mode::Spy,
        Mode::Blitzkrieg,
        Mode::ClanWar,
        Mode::GunGame,
        Mode::Infected,
        Mode::DynDuel,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Mode::Deathmatch => "Deathmatch",
            Mode::Team => "Team DM",
            Mode::Gladiator => "Gladiator",
            Mode::TeamGladiator => "Team Gladiator",
            Mode::Elimination => "Elimination",
            Mode::Assassinate => "Assassinate",
            Mode::Duel => "Duel",
            Mode::Training => "Training",
            Mode::Quest => "Quest",
            Mode::Berserker => "Berserker",
            Mode::DuelTournament => "Tournament",
            Mode::Gunman => "Gunman",
            Mode::Spy => "Spy",
            Mode::Blitzkrieg => "Blitzkrieg",
            Mode::ClanWar => "Clan War",
            Mode::GunGame => "Gun Game",
            Mode::Infected => "Infected",
            Mode::DynDuel => "Dynamic Duels",
        }
    }

    /// The `--mode` value.
    pub fn arg(self) -> &'static str {
        match self {
            Mode::Deathmatch => "dm",
            Mode::Team => "tdm",
            Mode::Gladiator => "gladiator",
            Mode::TeamGladiator => "team-gladiator",
            Mode::Elimination => "elimination",
            Mode::Assassinate => "assassinate",
            Mode::Duel => "duel",
            Mode::Training => "training",
            Mode::Quest => "quest",
            Mode::Berserker => "berserker",
            Mode::DuelTournament => "tournament",
            Mode::Gunman => "gunman",
            Mode::Spy => "spy",
            Mode::Blitzkrieg => "blitzkrieg",
            Mode::ClanWar => "clanwar",
            Mode::GunGame => "gungame",
            Mode::Infected => "infected",
            Mode::DynDuel => "dynduel",
        }
    }

    /// Red against Blue (actors get a [`crate::game::Team`]).
    pub fn teams(self) -> bool {
        matches!(
            self,
            Mode::Team
                | Mode::TeamGladiator
                | Mode::Elimination
                | Mode::Assassinate
                | Mode::Blitzkrieg
                | Mode::ClanWar
        )
    }

    /// Loadouts are cut to the melee slot.
    pub fn melee_only(self) -> bool {
        matches!(self, Mode::Gladiator | Mode::TeamGladiator)
    }

    /// Played in rounds: nobody respawns until the round is decided.
    pub fn rounds(self) -> bool {
        matches!(
            self,
            Mode::Elimination
                | Mode::Assassinate
                | Mode::Duel
                | Mode::DuelTournament
                | Mode::Spy
                | Mode::Infected
                | Mode::ClanWar
        )
    }

    /// Pickups use the map's `spawn_item_team_*` list instead of `spawn_item_solo_*`.
    pub fn team_items(self) -> bool {
        self.teams()
    }

    /// One-on-one rounds: the duel and its tournament.
    pub fn duel(self) -> bool {
        matches!(self, Mode::Duel | Mode::DuelTournament)
    }

    /// Whether the map's item pickups exist (*inferred*: not in the one-on-one duel, nor in
    /// the training range).
    pub fn items(self) -> bool {
        !matches!(
            self,
            Mode::Duel | Mode::DuelTournament | Mode::DynDuel | Mode::Training
        )
    }

    /// What the kill limit counts: kills, team kills, or round wins.
    pub fn kills_name(self) -> &'static str {
        if self.rounds() {
            "Wins to end"
        } else {
            "Kill limit"
        }
    }

    /// What the time limit is: the match, or (duel) one round.
    pub fn time_name(self) -> &'static str {
        if self.duel() {
            "Round time"
        } else {
            "Time limit"
        }
    }

    pub fn blurb(self) -> &'static str {
        match self {
            Mode::Deathmatch => "Free for all. Respawn after a few seconds.",
            Mode::Team => "Red against Blue. Team kills count. Respawn after a few seconds.",
            Mode::Gladiator => "Free for all with melee weapons only.",
            Mode::TeamGladiator => "Red against Blue with melee weapons only.",
            Mode::Elimination => "Red against Blue rounds. No respawn until the round ends.",
            Mode::Assassinate => "Rounds: kill the enemy VIP, keep your own alive.",
            Mode::Duel => "One on one. The winner stays, the loser queues, the rest watch.",
            Mode::Training => "No bots: slash and shoot the dummy targets.",
            Mode::Quest => "Clear NPC sectors with bot allies, then take the portal.",
            Mode::Berserker => "One berserker, everyone else hunts it. Kill it to become it.",
            Mode::DuelTournament => {
                "Knockout bracket of one-on-one duels. More damage wins on time."
            }
            Mode::Gunman => "Deathmatch with a random melee weapon and gun every life.",
            Mode::Spy => "Hidden spies with grenades only, trackers hunt them. Rounds.",
            Mode::Blitzkrieg => {
                "Red against Blue on the Blitzkrieg map: soldiers march, destroy the enemy buildings. F: honor upgrades."
            }
            Mode::ClanWar => {
                "Your clan and bot members against a rival clan, 4 against 4. Rounds, no respawn. Clan points change."
            }
            Mode::GunGame => {
                "Free for all up a weapon ladder: each kill upgrades you, a melee kill demotes the victim. Clear the last step to win."
            }
            Mode::Infected => {
                "Rounds: one random player turns zombie. Survivors outlast the timer, a zombie's kill turns you."
            }
            Mode::DynDuel => {
                "Several one-on-one duels at once. The winner stays, the loser queues and watches, the next in line challenges."
            }
        }
    }

    pub fn limits(self) -> Limits {
        const LONG: &[u32] = &[0, 10, 20, 30, 40, 50, 60];
        const SHORT: &[u32] = &[0, 5, 10, 15, 20, 25, 30];
        fn l(
            kills: &'static [u32],
            kills_default: u32,
            minutes: &'static [u32],
            minutes_default: u32,
        ) -> Limits {
            Limits {
                kills,
                kills_default,
                minutes,
                minutes_default,
            }
        }
        match self {
            Mode::Deathmatch => l(&[5, 7, 10, 20, 30, 50, 70, 100], 50, LONG, 30),
            Mode::Team => l(&[5, 10, 20, 30, 50, 70, 100], 70, LONG, 40),
            Mode::Gladiator => l(&[10, 20, 30, 50, 70, 100], 50, LONG, 30),
            Mode::TeamGladiator | Mode::Assassinate => l(&[10, 20, 30, 50, 70, 100], 30, SHORT, 10),
            Mode::Elimination => l(&[3, 5, 10, 20, 30, 50, 70, 100], 30, SHORT, 10),
            Mode::Duel => l(&[10, 15, 20, 25, 30], 20, &[1, 2, 3, 4, 5], 3),
            // The file lists 10..100 (default 50, 30 min); a training range that ends on its
            // own is no use, so it starts unlimited (0 = off, inferred).
            Mode::Training => l(&[0, 10, 20, 30, 50, 70, 100], 0, LONG, 0),
            // No limits: a quest ends when its sectors are cleared or the player falls.
            Mode::Quest => l(&[0], 0, &[0], 0),
            Mode::Berserker => l(&[10, 20, 30, 50, 70, 100], 50, LONG, 30),
            // The bracket decides itself; the time limit is per duel (as the duel's).
            Mode::DuelTournament => l(&[0], 0, &[1, 2, 3, 4, 5], 3),
            Mode::Gunman => l(&[50], 50, &[20], 20),
            // `spymode.xml`/`gametypecfg.xml`: 3-5 rounds, the round time comes from the map.
            Mode::Spy => l(&[3, 4, 5], 3, &[0], 0),
            // No score limit: the match ends when a radar falls (or at the time limit).
            Mode::Blitzkrieg => l(&[0], 0, LONG, 0),
            // `gametypecfg.xml` game type 22: `ROUNDS` 3 (the only choice), `LIMITTIME` -1.
            Mode::ClanWar => l(&[3], 3, &[0], 0),
            // No limits (*inferred*): a match ends when someone clears the ladder.
            Mode::GunGame => l(&[0], 0, &[0], 0),
            // Rounds to win and match minutes (*inferred*; a round is 3 minutes at most).
            Mode::Infected => l(&[3, 5, 10, 20], 5, SHORT, 20),
            // Duel wins to end and match minutes (*inferred*).
            Mode::DynDuel => l(&[5, 10, 15, 20, 30], 10, LONG, 10),
        }
    }
}

impl FromStr for Mode {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        Mode::ALL.into_iter().find(|m| m.arg() == s).ok_or(())
    }
}

/// Everything the menu chooses; the same flags configure a run started from the command line.
#[derive(Clone, Debug)]
pub struct Config {
    /// Map directory name (`mansion`, `battle arena`).
    pub map: Option<String>,
    pub woman: bool,
    /// Clothes and dyes ([`Look`]): the profile's unless `--look` overrides them.
    pub look: Look,
    /// zitem ids for the weapon slots (melee, primary, secondary, item; 0 = empty): the
    /// profile's equipped weapons unless `--loadout` overrides them.
    pub loadout: Vec<u32>,
    pub bots: usize,
    /// Bot difficulty 0..=1.
    pub skill: f32,
    pub mode: Mode,
    pub time_limit: Option<u32>,
    pub kill_limit: Option<u32>,
    /// Quest mode: the scenario to play (`--scenario NAME`); `None` = the first one.
    pub scenario: Option<String>,
    /// Quest mode: the `<MAP dice>` to play (`--dice N`); `None` rolls one on every start.
    pub dice: Option<u32>,
    /// Quest mode: quest items in the two sacrifice slots (`--sacrifice A,B`; 0 = empty).
    pub sacrifice: [u32; 2],
    /// LAN play: host this match (`--host`) or join one (`--join ADDR`, `lan` finds the host).
    pub net: Option<Net>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Net {
    Host,
    Join(String),
}

impl Config {
    /// Takes the config flags out of `args`. Without flags a match is limited to 10 minutes
    /// and 20 kills, except in headless `--shot` runs (no limits, so test runs never end).
    pub fn parse(args: &mut Vec<String>, headless: bool) -> Result<Self, String> {
        fn get<T: FromStr>(args: &mut Vec<String>, flag: &str) -> Result<Option<T>, String> {
            take_arg(args, flag).map_err(|()| format!("{flag}: missing or bad value"))
        }
        let profile = Profile::open(headless);
        let woman = match get::<String>(args, "--char")?.as_deref() {
            None => profile.woman,
            Some("man") => false,
            Some("woman") => true,
            Some(_) => return Err("--char: man or woman".into()),
        };
        let loadout = match get::<String>(args, "--loadout")? {
            // Empty (0) slots are only ever the trailing ones.
            None => profile.equipped[..4]
                .iter()
                .copied()
                .take_while(|&i| i != 0)
                .collect(),
            Some(s) => s
                .split(',')
                .map(|n| n.parse().map_err(|_| format!("--loadout: bad id {n:?}")))
                .collect::<Result<_, _>>()?,
        };
        let mode = get::<Mode>(args, "--mode")?.unwrap_or(Mode::Deathmatch);
        if mode == Mode::ClanWar && profile.clan.is_none() {
            return Err("--mode clanwar needs a clan: create one in the menu's CLAN tab".into());
        }
        let lim = mode.limits();
        let limit = |v: Option<u32>, default: u32| match v {
            None => (!headless && default > 0).then_some(default),
            Some(n) => (n > 0).then_some(n),
        };
        let sacrifice = match get::<String>(args, "--sacrifice")? {
            None => [0; 2],
            Some(s) => {
                let id = |n: &str| n.parse().map_err(|_| format!("--sacrifice: bad id {n:?}"));
                match *s.split(',').collect::<Vec<_>>() {
                    [a] => [id(a)?, 0],
                    [a, b] => [id(a)?, id(b)?],
                    _ => return Err("--sacrifice: one or two item ids".into()),
                }
            }
        };
        let (time, kills) = (get(args, "--time-limit")?, get(args, "--kill-limit")?);
        let net = match args.iter().position(|a| a == "--host") {
            Some(at) => {
                args.remove(at);
                Some(Net::Host)
            }
            None => get::<String>(args, "--join")?.map(Net::Join),
        };
        if net == Some(Net::Host) && !crate::net::supported(mode) {
            return Err(format!("--host: {}", crate::net::MODES));
        }
        Ok(Self {
            map: get(args, "--map")?
                .or_else(|| (mode == Mode::Blitzkrieg).then(|| "blitzkrieg".into())),
            woman,
            look: match get::<String>(args, "--look")? {
                None => profile.look,
                Some(s) => s.parse().map_err(|e| format!("--look: {e}"))?,
            },
            loadout,
            bots: get(args, "--bots")?.unwrap_or(3),
            skill: get::<f32>(args, "--skill")?.unwrap_or(0.5).clamp(0.0, 1.0),
            mode,
            time_limit: limit(time, lim.minutes_default * 60),
            kill_limit: limit(kills, lim.kills_default),
            scenario: get(args, "--scenario")?,
            dice: get(args, "--dice")?,
            sacrifice,
            net,
        })
    }

    /// The flags [`Config::parse`] reads back (without the map).
    pub fn flags(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "--char",
            if self.woman { "woman" } else { "man" },
            "--bots",
            &self.bots.to_string(),
            "--skill",
            &self.skill.to_string(),
            "--mode",
            self.mode.arg(),
            "--time-limit",
            &self.time_limit.unwrap_or(0).to_string(),
            "--kill-limit",
            &self.kill_limit.unwrap_or(0).to_string(),
            "--look",
            &self.look.to_string(),
        ]
        .map(String::from)
        .into();
        // Id 0 marks an empty slot; only trailing slots can be empty.
        let ids: Vec<_> = self.loadout.iter().take_while(|&&i| i != 0).collect();
        if !ids.is_empty() {
            a.push("--loadout".into());
            a.push(
                ids.iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(","),
            );
        }
        if let Some(s) = &self.scenario {
            a.extend(["--scenario".into(), s.clone()]);
        }
        if let Some(d) = self.dice {
            a.extend(["--dice".into(), d.to_string()]);
        }
        if self.sacrifice != [0; 2] {
            a.extend([
                "--sacrifice".into(),
                format!("{},{}", self.sacrifice[0], self.sacrifice[1]),
            ]);
        }
        match &self.net {
            Some(Net::Host) => a.push("--host".into()),
            Some(Net::Join(addr)) => a.extend(["--join".into(), addr.clone()]),
            None => {}
        }
        a
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Match,
    Player,
    Shop,
    Inventory,
    Clan,
    Controls,
    Graphics,
}

impl FromStr for Page {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "match" => Ok(Page::Match),
            "player" => Ok(Page::Player),
            "shop" => Ok(Page::Shop),
            "inventory" => Ok(Page::Inventory),
            "clan" => Ok(Page::Clan),
            "controls" => Ok(Page::Controls),
            "graphics" => Ok(Page::Graphics),
            _ => Err(()),
        }
    }
}

fn label(s: &str, w: f32) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(17.0),
        TextColor(DIM),
        Node {
            width: px(w),
            height: px(24),
            ..default()
        },
    )
}

/// Marks a top-bar page tab; [`tab_look`] paints it from [`Interaction`] and [`Chosen`].
#[derive(Component)]
struct NavTab;

/// A top-bar tab: caps text over a 3 px bar that lights up on hover and shows the accent when
/// selected (no filled tile, unlike [`button`]).
fn tab(text: &str, page: Page) -> impl Bundle {
    (
        Act::Page(page),
        NavTab,
        Button,
        Node {
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: px(7),
            padding: UiRect::horizontal(px(2)),
            ..default()
        },
        children![
            (
                Text::new(text.to_uppercase()),
                strong(19.0),
                TextColor(DIM),
                shadow()
            ),
            (
                Node {
                    width: percent(100),
                    height: px(3),
                    ..default()
                },
                BackgroundColor(Color::NONE),
            ),
        ],
    )
}

/// Hover/selected look of the top-bar tabs [`tab`] spawns: a lighter style than [`hover`]'s
/// filled [`Flat`] buttons (tabs never get a background tile).
#[allow(clippy::type_complexity)]
fn tab_look(
    tabs: Query<(&Interaction, Has<Chosen>, &Children), With<NavTab>>,
    mut labels: Query<&mut TextColor>,
    mut bars: Query<&mut BackgroundColor>,
) {
    for (i, chosen, kids) in &tabs {
        let hot = *i != Interaction::None;
        let ink = if chosen || hot { INK } else { DIM };
        let bar = if chosen {
            ACCENT
        } else if hot {
            DIM
        } else {
            Color::NONE
        };
        if let Some(&label) = kids.first()
            && let Ok(mut t) = labels.get_mut(label)
        {
            t.set_if_neq(TextColor(ink));
        }
        if let Some(&under) = kids.get(1)
            && let Ok(mut b) = bars.get_mut(under)
        {
            b.set_if_neq(BackgroundColor(bar));
        }
    }
}

/// Marks a [`quiet`] button; [`quiet_look`] lights its label on hover.
#[derive(Component)]
struct Quiet;

/// A low-emphasis text button (QUIT): no tile, just caps text.
fn quiet<A: Component>(text: &str, size: f32, act: A) -> impl Bundle {
    (
        act,
        Quiet,
        Button,
        Node {
            padding: UiRect::axes(px(12), px(10)),
            align_items: AlignItems::Center,
            ..default()
        },
        children![(Text::new(text.to_uppercase()), strong(size), TextColor(DIM))],
    )
}

/// DIM label, INK while hovered or pressed.
fn quiet_look(
    buttons: Query<(&Interaction, &Children), With<Quiet>>,
    mut labels: Query<&mut TextColor>,
) {
    for (i, kids) in &buttons {
        if let Some(&label) = kids.first()
            && let Ok(mut t) = labels.get_mut(label)
        {
            t.set_if_neq(TextColor(if *i == Interaction::None { DIM } else { INK }));
        }
    }
}

/// What the main menu offers: maps, the characters and their clothes.
#[derive(Resource)]
pub(crate) struct Catalog {
    pub(crate) vfs: Vfs,
    pub(crate) items: Items,
    maps: Vec<String>,
    /// Quest mode: the retail quest data (`None` if the files are missing).
    quest: Option<crate::quest::Catalog>,
    /// Its scenario names; the first is the default.
    scenarios: Vec<String>,
    men: Character,
    women: Character,
    /// `[man, woman]`.
    wardrobes: [Wardrobe; 2],
}

impl Catalog {
    fn load(vfs: Vfs) -> std::io::Result<Self> {
        let items = Items::load(&vfs)?;
        let maps = vfs.maps();
        let quest = crate::quest::Catalog::load(&vfs).ok();
        let scenarios = quest.as_ref().map(|q| q.names()).unwrap_or_default();
        let (men, women) = (
            character::load(&vfs, "heroman1")?,
            character::load(&vfs, "herowoman1")?,
        );
        Ok(Self {
            wardrobes: [
                Wardrobe::new(&men, &items, false),
                Wardrobe::new(&women, &items, true),
            ],
            men,
            women,
            vfs,
            items,
            maps,
            quest,
            scenarios,
        })
    }

    fn character(&self, woman: bool) -> &Character {
        if woman { &self.women } else { &self.men }
    }

    fn wardrobe(&self, woman: bool) -> &Wardrobe {
        &self.wardrobes[woman as usize]
    }
}

/// `battle arena` -> `Battle Arena`, `snow_town` -> `Snow Town`.
fn title(dir: &str) -> String {
    dir.replace('_', " ")
        .split(' ')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Resource)]
pub(crate) struct State {
    pub(crate) cfg: Config,
    pub(crate) page: Page,
}

/// What the character preview was built for (sex, clothes, sword): it is rebuilt when that changes.
#[derive(Resource, Default)]
struct Shown(Option<(bool, Look, u32)>);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Field {
    MapName,
    Time,
    Kills,
    TimeName,
    KillsName,
    Blurb,
    Bots,
    Skill,
    /// The piece / the dye of a body slot ([`Slot::ALL`] index).
    Piece(usize),
    Tint(usize),
    /// Whether the character on screen is the saved one.
    Saved,
    Scenario,
    Sac1,
    Sac2,
    Needs,
    /// The effects panel: a tab's amount (a percentage) and colour name, and the ORIGINAL note.
    Amount(Tab),
    Colour(Tab),
    Note,
}

#[derive(Component, Clone, Copy)]
enum Act {
    Page(Page),
    Map(usize),
    Mode(Mode),
    Sex(bool),
    Step(Field, i32),
    Start,
    /// Start as the LAN host (`true`) or join the LAN host (`false`).
    Lan(bool),
    /// Clothes: everything random (`false`) or new random dyes only (`true`).
    Random(bool),
    /// Back to the base body without dyes.
    Reset,
    /// Keep the character in the profile.
    Save,
    /// Point the preview camera at a [`FOCUS`] entry.
    Focus(usize),
    /// Effects panel: the dash style, the slash style, a tab's colour and amount, the tab.
    Style(Style),
    Cut(slashfx::Style),
    Hue(Tab, Hue),
    Amount(Tab, i32),
    Tab(Tab),
    Demo,
    Quit,
}

/// Text showing the current value of a field.
#[derive(Component)]
struct Value(Field);

/// Container shown only on its page.
#[derive(Component)]
pub(crate) struct PageRoot(pub(crate) Page);

/// Container shown only while the chosen mode is Quest (`true`) or is not Quest (`false`).
#[derive(Component)]
struct ModeShow(bool);

/// The character model's parent (turned by dragging, slowly on its own), shown on the player
/// page only.
#[derive(Component)]
struct Preview;

/// The colour square next to a slot's dye name.
#[derive(Component)]
struct Swatch(usize);

/// Preview camera framings: name, eye height (m), distance (m).
const FOCUS: [(&str, f32, f32); 5] = [
    ("FULL", 1.05, 3.6),
    ("HEAD", 1.62, 1.0),
    ("BODY", 1.25, 1.8),
    ("LEGS", 0.6, 1.9),
    ("FEET", 0.22, 1.3),
];

/// Where the preview camera is heading: [`FOCUS`] index and wheel zoom factor.
#[derive(Resource)]
struct Orbit {
    focus: usize,
    zoom: f32,
}

/// The effects demo (**not retail**) on the player page: the shown [`Tab`]'s effect plays on the
/// character, when a setting of the tab changes, on DEMO and every so often: dashes across the
/// stage and back, or the katana combo.
#[derive(Resource)]
struct Demo {
    /// Seconds until the next run starts (counts down while none runs and the page is open).
    wait: f32,
    /// Seconds into the running demo.
    run: Option<f32>,
    /// `wait` was set during a run and survives its end.
    again: bool,
    /// Where the character stands (m along world X at FULL framing) and how far it has turned to
    /// face the camera (0..=1); [`orbit`] applies both.
    x: f32,
    face: f32,
    /// The run's length in seconds (a slash run ends early, half a second after its last swing)
    /// and how many swings it has started.
    end: f32,
    step: usize,
}

impl Default for Demo {
    fn default() -> Self {
        Self {
            wait: DEMO_FIRST,
            run: None,
            again: false,
            x: 0.0,
            face: 0.0,
            end: DEMO_END,
            step: 0,
        }
    }
}

/// What the EFFECTS panel shows and the demo plays.
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    Dash,
    Slash,
}

#[derive(Resource)]
struct Fx(Tab);

/// Seconds from opening the page to the first run, from a run's end to the next, and from a
/// settings change to its run.
const DEMO_FIRST: f32 = 1.0;
const DEMO_EVERY: f32 = 4.0;
const DEMO_SETTLE: f32 = 0.4;
/// Each dash: start second, goal as a share of [`DEMO_SPAN`] (to the screen's right), its clip.
/// The model faces the camera, so its left is the screen's right. The run ends at `DEMO_END`.
const DASHES: [(f32, f32, &str); 2] = [(0.35, 1.0, "tumbleL"), (1.7, 0.0, "tumbleR")];
const DASH_TIME: f32 = 0.45;
const DEMO_END: f32 = 2.7;
/// Metres of the dash at FULL framing (the preview frame is about 0.8 m wide).
const DEMO_SPAN: f32 = 0.2;
/// The slash demo plays the katana's combo clips (motion type 1) and ends after `SLASH_CAP`
/// seconds at the latest.
const SLASHES: [&str; 4] = ["attack1", "attack2", "attack3", "attack4"];
const SWORD_SET: u32 = 1;
const SLASH_CAP: f32 = 8.0;
/// The clips: the empty-handed lobby pose (`gm="0"` in the character XML; motion type 8 is the
/// bare-handed item set) and the plain dash clips of that set.
const IDLE: (u32, &str) = (8, "login_idle");
const DASH_SET: u32 = 8;

/// Character clips parsed on first use: `(woman, motion type, name)` -> clip and loop mode.
#[derive(Resource, Default)]
struct Clips(HashMap<(bool, u32, &'static str), Option<Clip>>);

type Clip = (Arc<Ani>, Loop);

impl Clips {
    fn get(
        &mut self,
        cat: &Catalog,
        woman: bool,
        motion: u32,
        name: &'static str,
    ) -> Option<(Arc<Ani>, Loop)> {
        self.0
            .entry((woman, motion, name))
            .or_insert_with(|| {
                let a = cat
                    .character(woman)
                    .animations
                    .iter()
                    .find(|a| a.name == name && a.motion_type == motion)?;
                let ani = cat
                    .vfs
                    .read(&a.file)
                    .and_then(|b| crate::ani::load(&b))
                    .map_err(|e| warn!("{}: {e}", a.file))
                    .ok()?;
                Some((Arc::new(ani), Loop::from_xml(&a.loop_type)))
            })
            .clone()
    }
}

/// The amount slider of an EFFECTS tab and its filled part.
#[derive(Component)]
struct Slider(Tab);

#[derive(Component)]
struct Fill(Tab);

/// Container shown only while its [`Tab`] is the EFFECTS panel's.
#[derive(Component)]
struct TabShow(Tab);

/// The katana in the preview character's hand: hidden, but during the slash demo.
#[derive(Component)]
struct Sword;

const MAX_BOTS: i32 = 15;

/// Moves `cur` by `d` positions within `list`, clamped.
fn walk(list: &[u32], cur: u32, d: i32) -> u32 {
    let at = list.iter().position(|&v| v == cur).unwrap_or(0) as i32;
    list[(at + d).clamp(0, list.len() as i32 - 1) as usize]
}

fn step(cfg: &mut Config, cat: &Catalog, profile: &Profile, field: Field, d: i32) {
    match field {
        Field::MapName
        | Field::TimeName
        | Field::KillsName
        | Field::Blurb
        | Field::Amount(_)
        | Field::Colour(_)
        | Field::Note => {}
        Field::Time => {
            let secs: Vec<u32> = cfg.mode.limits().minutes.iter().map(|m| m * 60).collect();
            cfg.time_limit = Some(walk(&secs, cfg.time_limit.unwrap_or(0), d)).filter(|&t| t > 0)
        }
        Field::Kills => {
            let list = cfg.mode.limits().kills;
            cfg.kill_limit = Some(walk(list, cfg.kill_limit.unwrap_or(0), d)).filter(|&t| t > 0)
        }
        Field::Bots => cfg.bots = (cfg.bots as i32 + d).clamp(0, MAX_BOTS) as usize,
        Field::Skill => cfg.skill = ((cfg.skill * 10.0).round() + d as f32).clamp(0.0, 10.0) / 10.0,
        Field::Piece(s) => {
            // the base piece, then every piece of the wardrobe
            let list = &cat.wardrobe(cfg.woman).slots[s];
            let n = list.len() as i32 + 1;
            let at = cfg.look.parts[s]
                .and_then(|p| list.iter().position(|q| q.part == p))
                .map_or(0, |i| i as i32 + 1);
            let next = (at + d).rem_euclid(n) as usize;
            cfg.look.parts[s] = next.checked_sub(1).map(|i| list[i].part);
        }
        Field::Tint(s) => {
            let t = (cfg.look.tints[s] as i32 + d).rem_euclid(TINTS.len() as i32);
            cfg.look.tints[s] = t as u8;
        }
        Field::Saved => {}
        Field::Sac1 | Field::Sac2 => {
            let Some(q) = &cat.quest else { return };
            let slot = (field == Field::Sac2) as usize;
            // the empty slot, then the sacrificable quest items the profile keeps
            let mut list = vec![0];
            list.extend(
                profile
                    .quest_items
                    .keys()
                    .filter(|id| q.items.0.get(id).is_some_and(|i| i.sacrifice)),
            );
            let at = list
                .iter()
                .position(|&i| i == cfg.sacrifice[slot])
                .unwrap_or(0);
            cfg.sacrifice[slot] = list[(at as i32 + d).rem_euclid(list.len() as i32) as usize];
            // two items that make a special scenario's offering switch to it
            if let Some(t) = q.special_for(&cfg.sacrifice) {
                cfg.scenario = Some(t);
            }
        }
        Field::Needs => {}
        Field::Scenario => {
            let list = &cat.scenarios;
            let at = list
                .iter()
                .position(|s| Some(s) == cfg.scenario.as_ref())
                .unwrap_or(0) as i32;
            if !list.is_empty() {
                cfg.scenario = Some(list[(at + d).rem_euclid(list.len() as i32) as usize].clone());
            }
        }
    }
}

fn value(cfg: &Config, cat: &Catalog, profile: &Profile, field: Field) -> String {
    let clock = |s: u32| format!("{}:{:02}", s / 60, s % 60);
    match field {
        Field::MapName if cfg.mode == Mode::Quest => "Quest".into(),
        Field::MapName => cfg.map.as_deref().map(title).unwrap_or_default(),
        Field::Time => cfg.time_limit.map_or("Off".into(), clock),
        Field::Kills => cfg.kill_limit.map_or("Off".into(), |k| k.to_string()),
        Field::TimeName => cfg.mode.time_name().into(),
        Field::KillsName => cfg.mode.kills_name().into(),
        Field::Blurb => cfg.mode.blurb().into(),
        Field::Bots => cfg.bots.to_string(),
        Field::Skill => format!(
            "{:.1}  {}",
            cfg.skill,
            match cfg.skill {
                s if s < 0.34 => "Easy",
                s if s < 0.67 => "Normal",
                _ => "Hard",
            }
        ),
        Field::Piece(s) => {
            let list = &cat.wardrobe(cfg.woman).slots[s];
            match cfg.look.parts[s].and_then(|p| list.iter().position(|q| q.part == p)) {
                _ if list.is_empty() => "Default (no other)".into(),
                // long item names are cut so the count stays clear of the step buttons
                Some(i) => {
                    let name = &list[i].name;
                    let short: String = name.chars().take(18).collect();
                    let cut = if short.len() < name.len() { "." } else { "" };
                    format!("{short}{cut} {}/{}", i + 1, list.len())
                }
                None => format!("Default  0/{}", list.len()),
            }
        }
        Field::Tint(s) => TINTS[cfg.look.tints[s] as usize % TINTS.len()].0.into(),
        Field::Saved => {
            if (profile.woman, profile.look) == (cfg.woman, cfg.look) {
                "Saved: matches use this character.".into()
            } else {
                "Not saved yet: SAVE keeps it (START saves too).".into()
            }
        }
        Field::Sac1 | Field::Sac2 => {
            let id = cfg.sacrifice[(field == Field::Sac2) as usize];
            match (&cat.quest, id) {
                (Some(q), id) if id != 0 => format!(
                    "{} x{}",
                    q.items.name(id),
                    profile.quest_items.get(&id).copied().unwrap_or(0)
                ),
                _ => "-".into(),
            }
        }
        Field::Needs => {
            let Some(q) = &cat.quest else {
                return String::new();
            };
            let name = cfg
                .scenario
                .as_deref()
                .or(cat.scenarios.first().map(String::as_str));
            match q.admit(name, cfg.sacrifice, &profile.quest_items, profile.level()) {
                Ok(a) if a.spend.is_empty() => format!("{}: ready", a.scenario),
                Ok(a) => {
                    let spent: Vec<_> = a.spend.iter().map(|&i| q.items.name(i)).collect();
                    format!("{}: ready, spends {}", a.scenario, spent.join(" + "))
                }
                Err(e) => {
                    let draws = cfg.sacrifice.iter().find_map(|&i| Some((i, q.draws(i)?)));
                    let hint = draws.map(|(i, n)| format!("  ({} draws a {n})", q.items.name(i)));
                    e + &hint.unwrap_or_default()
                }
            }
        }
        Field::Amount(t) if amount(profile, t) <= 0.0 => "OFF".into(),
        Field::Amount(t) => format!("{:.0}%", amount(profile, t) * 100.0),
        Field::Colour(t) => format!(
            "COLOUR  {}",
            HUES.iter()
                .find(|h| h.0
                    == match t {
                        Tab::Dash => profile.dash.hue,
                        Tab::Slash => profile.slash.hue,
                    })
                .unwrap()
                .2
        ),
        Field::Note if dashfx::original(profile) => "ORIGINAL: no trails".into(),
        Field::Note => String::new(),
        Field::Scenario => cfg
            .scenario
            .clone()
            .or_else(|| cat.scenarios.first().cloned())
            .unwrap_or_default(),
    }
}

/// The main menu: its screen is built when [`Screen::Menu`] is entered, from the [`Launch`]'s config.
pub struct MenuPlugin;

impl Plugin for MenuPlugin {
    /// Needs `ProfilePlugin` (the clan tab reads the profile) built before.
    fn build(&self, app: &mut App) {
        app.init_resource::<Clips>()
            .add_plugins((shop::ShopPlugin, clan::ClanMenuPlugin))
            .add_systems(OnEnter(Screen::Menu), (prepare, build).chain())
            .add_systems(
                Update,
                (
                    auto_start, act, refresh, hover, tab_look, quiet_look, preview, demo, orbit,
                    fit,
                )
                    .chain()
                    .in_set(MenuSystems),
            )
            .add_systems(Update, fx_panel.in_set(MenuSystems));
    }
}

/// Loads the menu's data (once) and starts a visit with the page and choices of the [`Launch`].
/// Headless (a `Shot`), the picture is taken `GUNZ_MENU_AT=SECS` into the page (default 2, the
/// clock steps 1/60 s a frame, so the idle pose and the demo are the same on every run) unless
/// `GUNZ_MENU_START` presses START first; `GUNZ_MENU_TAB=slash` opens the EFFECTS panel on its
/// SLASH tab.
fn prepare(world: &mut World) {
    if !world.contains_resource::<Catalog>() {
        let vfs = world.resource::<Mount>().0.clone();
        let cat = Catalog::load(vfs).unwrap_or_else(|e| panic!("menu data: {e}"));
        let data =
            ShopData::load(&cat.vfs, &cat.items).unwrap_or_else(|e| panic!("shop data: {e}"));
        let ranks = Ranks::load(&cat.vfs).unwrap_or_else(|e| panic!("menu data: {e}"));
        println!(
            "menu: {} maps, {} items for sale",
            cat.maps.len(),
            data.sale.len()
        );
        world.insert_resource(cat);
        world.insert_resource(data);
        world.insert_resource(ranks);
    }
    let page = world
        .resource_mut::<Launch>()
        .page
        .take()
        .unwrap_or(Page::Match);
    let mut cfg = world.resource::<Launch>().config.clone();
    let cat = world.resource::<Catalog>();
    let default_map = cat
        .maps
        .iter()
        .find(|m| *m == "mansion")
        .or(cat.maps.first())
        .cloned();
    cfg.loadout.resize(4, 0);
    cfg.map = cfg.map.filter(|m| cat.maps.contains(m)).or(default_map);
    // The menu edits the profile's character and equipment; Start launches with them.
    let mut profile = world.resource_mut::<Profile>();
    if (profile.woman, profile.look) != (cfg.woman, cfg.look) {
        (profile.woman, profile.look) = (cfg.woman, cfg.look);
    }
    let tab = if std::env::var("GUNZ_MENU_TAB").is_ok_and(|t| t == "slash") {
        Tab::Slash
    } else {
        Tab::Dash
    };
    world.insert_resource(State { cfg, page });
    world.insert_resource(Orbit {
        focus: 0,
        zoom: 1.0,
    });
    world.insert_resource(Demo::default());
    world.insert_resource(Fx(tab));
    world.insert_resource(Shown::default());
    world.insert_resource(ClearColor(crate::phase::BACKDROP));
    world.resource_scope(|world, cat: Mut<Catalog>| crate::music::menu(world, &cat.vfs));
    let start_first = world
        .get_resource::<Hooks>()
        .is_some_and(|h| !h.start.is_empty());
    let secs = std::env::var("GUNZ_MENU_AT")
        .ok()
        .and_then(|s| s.parse::<f32>().ok())
        .unwrap_or(2.0);
    if let Some(mut shot) = world.get_resource_mut::<Shot>() {
        if start_first {
            shot.pending();
        } else {
            shot.capture_in((secs * 60.0).round() as u32);
        }
        let frame = shot.frame();
        if let Some(mut hooks) = world.get_resource_mut::<Hooks>() {
            hooks.opened = frame;
        }
    }
}

/// Headless `GUNZ_MENU_START`: presses START that many seconds after the menu opened.
fn auto_start(
    hooks: Option<ResMut<Hooks>>,
    shot: Option<Res<Shot>>,
    mut buttons: Query<(&Act, &mut Interaction)>,
) {
    let (Some(mut hooks), Some(shot)) = (hooks, shot) else {
        return;
    };
    let Some(&secs) = hooks.start.front() else {
        return;
    };
    if ((shot.frame() - hooks.opened) as f32) < secs * 60.0 {
        return;
    }
    hooks.start.pop_front();
    for (a, mut i) in &mut buttons {
        if matches!(a, Act::Start) {
            *i = Interaction::Pressed;
        }
    }
}

fn build(
    mut commands: Commands,
    cat: Res<Catalog>,
    data: Res<ShopData>,
    state: Res<State>,
    profile: Res<Profile>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    shot: Option<Res<Shot>>,
) {
    commands.insert_resource(shop::Icons::load(&cat.vfs, &data, &mut images));
    let clan_art =
        ClanArt::load(&cat.vfs, &mut images).unwrap_or_else(|e| panic!("clan data: {e}"));
    let img = |n: &str, images: &mut Assets<Image>| {
        try_image(&cat.vfs, images, n).unwrap_or_else(|| panic!("interface/default/{n} missing"))
    };
    let (logo, bg) = (
        img("gunz_logo_hq.png", &mut images),
        img("bg_play.png", &mut images),
    );
    let camera = view::spawn_camera(
        &mut commands,
        &mut images,
        shot.is_some(),
        Vec3::new(0.0, 1.05, 3.6),
        Vec3::NEG_Z,
    )
    .id();
    // Lobby background as a quad behind the character (UI sits on top of the 3D view). A child of
    // the camera, far back, so it fills the view however far the camera pulls back (the slash
    // demo); at the default framing it looks as it did 7 m away (scaled 60 / 7).
    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(162.9, 91.7))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(bg),
            base_color: Color::srgb(0.85, 0.85, 0.9),
            unlit: true,
            ..default()
        })),
        Transform::from_xyz(0.0, -0.43, -60.0),
        ChildOf(camera),
    ));
    commands.spawn((Preview, Transform::default(), Visibility::Hidden));

    let cfg = &state.cfg;
    let row = || Node {
        align_items: AlignItems::Center,
        column_gap: px(6),
        ..default()
    };
    // The `«`/`»` step buttons and the digits value between them, shared by every stepper.
    let group = |p: &mut ChildSpawnerCommands, f: Field, w: f32, big: bool| {
        if big {
            p.spawn(button(34.0, 30.0, "<<", 15.0, Act::Step(f, -10)));
        }
        p.spawn(button(30.0, 30.0, "<", 15.0, Act::Step(f, -1)));
        p.spawn((
            Value(f),
            Text::new(value(cfg, &cat, &profile, f)),
            digits(16.0),
            TextColor(INK),
            TextLayout {
                justify: Justify::Center,
                linebreak: LineBreak::NoWrap,
                ..default()
            },
            Node {
                width: px(w),
                height: px(24),
                overflow: Overflow::clip(),
                ..default()
            },
        ));
        p.spawn(button(30.0, 30.0, ">", 15.0, Act::Step(f, 1)));
        if big {
            p.spawn(button(34.0, 30.0, ">>", 15.0, Act::Step(f, 10)));
        }
    };
    // `name  « value »`: a labelled row around [`group`].
    let stepper = |p: &mut ChildSpawnerCommands, name: &str, f: Field, w: f32, big: bool| {
        p.spawn(row()).with_children(|r| {
            // The limit labels name what the limit means in the chosen mode.
            let mut l = r.spawn(label(name, 130.0));
            match f {
                Field::Time => drop(l.insert(Value(Field::TimeName))),
                Field::Kills => drop(l.insert(Value(Field::KillsName))),
                _ => {}
            }
            group(r, f, w, big);
        });
    };

    commands
        .spawn((
            UiTargetCamera(camera),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .with_children(|root| {
            // dark bands behind the tabs and the buttons, fading into the lobby picture
            for (top, stops) in [(true, [0.8, 0.0]), (false, [0.0, 0.85])] {
                let c = |a| SHADOW.with_alpha(a);
                root.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        top: if top { px(0) } else { Val::Auto },
                        bottom: if top { Val::Auto } else { px(0) },
                        width: percent(100),
                        height: px(110),
                        ..default()
                    },
                    BackgroundGradient::from(LinearGradient::to_bottom(vec![
                        ColorStop::new(c(stops[0]), percent(0)),
                        ColorStop::new(c(stops[1]), percent(100)),
                    ])),
                ));
            }
            root.spawn((
                ImageNode::new(logo),
                Node {
                    position_type: PositionType::Absolute,
                    left: px(24),
                    top: px(12),
                    width: px(231),
                    height: px(96),
                    ..default()
                },
            ));
            // Tabs: caps text, ACCENT underline on the selected page (tab_look paints it).
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: px(280),
                top: px(44),
                column_gap: px(28),
                ..default()
            })
            .with_children(|t| {
                for (label, page) in [
                    ("MATCH", Page::Match),
                    ("PLAYER", Page::Player),
                    ("SHOP", Page::Shop),
                    ("INVENTORY", Page::Inventory),
                    ("CLAN", Page::Clan),
                    ("CONTROLS", Page::Controls),
                    ("GRAPHICS", Page::Graphics),
                ] {
                    t.spawn(tab(label, page));
                }
            });
            // Player strip: name, level + XP bar, bounty, medals.
            let (level, into, need) = progress(profile.xp);
            let frac = if need == 0 {
                1.0
            } else {
                (into as f32 / need as f32).clamp(0.0, 1.0)
            };
            root.spawn(Node {
                position_type: PositionType::Absolute,
                top: px(14),
                right: px(24),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::End,
                row_gap: px(3),
                ..default()
            })
            .with_children(|s| {
                s.spawn((
                    Text::new(profile.name.to_uppercase()),
                    display(22.0),
                    TextColor(INK),
                    shadow(),
                ));
                s.spawn(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    ..default()
                })
                .with_children(|r| {
                    r.spawn((Text::new("LV"), compact(14.0), TextColor(DIM), shadow()));
                    r.spawn((
                        Text::new(level.to_string()),
                        digits(15.0),
                        TextColor(ACCENT),
                        shadow(),
                    ));
                    r.spawn((
                        Node {
                            width: px(110),
                            height: px(6),
                            ..default()
                        },
                        BackgroundColor(RAISED),
                    ))
                    .with_children(|bar| {
                        bar.spawn((
                            Node {
                                width: px(110.0 * frac),
                                height: percent(100),
                                ..default()
                            },
                            BackgroundColor(ACCENT),
                        ));
                    });
                    r.spawn((
                        Text::new(format!("{into}/{need}")),
                        digits(14.0),
                        TextColor(DIM),
                        shadow(),
                    ));
                });
                s.spawn(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    ..default()
                })
                .with_children(|r| {
                    r.spawn((Text::new("BOUNTY"), compact(13.0), TextColor(DIM), shadow()));
                    r.spawn((
                        Text::new(profile.bounty.to_string()),
                        digits(16.0),
                        TextColor(INK),
                        shadow(),
                    ));
                    r.spawn((Text::new("MEDALS"), compact(13.0), TextColor(DIM), shadow()));
                    r.spawn((
                        Text::new(profile.medals.to_string()),
                        digits(16.0),
                        TextColor(INK),
                        shadow(),
                    ));
                });
            });
            let page = |page| {
                (
                    PageRoot(page),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(24),
                        top: px(120),
                        right: px(24),
                        column_gap: px(16),
                        align_items: AlignItems::FlexStart,
                        ..default()
                    },
                )
            };
            root.spawn(page(Page::Match)).with_children(|p| {
                p.spawn((ModeShow(false), panel(676.0, AlignItems::FlexStart)))
                    .with_children(|m| {
                        m.spawn(heading("MAP"));
                        m.spawn(Node {
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: px(4),
                            row_gap: px(4),
                            ..default()
                        })
                        .with_children(|g| {
                            for (i, name) in cat.maps.iter().enumerate() {
                                g.spawn(button(200.0, 30.0, &title(name), 16.0, Act::Map(i)));
                            }
                        });
                    });
                p.spawn(panel(540.0, AlignItems::FlexStart))
                    .with_children(|m| {
                        m.spawn(heading("RULES"));
                        m.spawn((
                            Value(Field::MapName),
                            Text::new(value(cfg, &cat, &profile, Field::MapName)),
                            display(30.0),
                            TextColor(INK),
                        ));
                        m.spawn(Node {
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: px(6),
                            row_gap: px(6),
                            ..default()
                        })
                        .with_children(|g| {
                            for mode in Mode::ALL {
                                g.spawn(button(122.0,
                                    30.0,
                                    mode.name(),
                                    13.0,
                                    Act::Mode(mode),
                                ));
                            }
                        });
                        m.spawn((
                            Value(Field::Blurb),
                            Text::new(value(cfg, &cat, &profile, Field::Blurb)),
                            body(15.0),
                            TextColor(DIM),
                            Node {
                                width: px(510),
                                height: px(40),
                                ..default()
                            },
                        ));
                        stepper(m, "Time limit", Field::Time, 150.0, false);
                        stepper(m, "Kill limit", Field::Kills, 150.0, false);
                        stepper(m, "Bots", Field::Bots, 150.0, false);
                        stepper(m, "Bot skill", Field::Skill, 150.0, false);
                    });
                p.spawn((ModeShow(true), panel(660.0, AlignItems::FlexStart)))
                    .with_children(|q| {
                        q.spawn(heading("QUEST"));
                        stepper(q, "Scenario", Field::Scenario, 300.0, false);
                        stepper(q, "Sacrifice 1", Field::Sac1, 300.0, false);
                        stepper(q, "Sacrifice 2", Field::Sac2, 300.0, false);
                        q.spawn((
                            Value(Field::Needs),
                            Text::new(value(cfg, &cat, &profile, Field::Needs)),
                            body(16.0),
                            TextColor(ACCENT),
                            Node {
                                width: px(600),
                                min_height: px(60),
                                ..default()
                            },
                        ));
                    });
            });
            root.spawn(page(Page::Player)).with_children(|p| {
                p.spawn(panel(740.0, AlignItems::FlexStart))
                    .with_children(|m| {
                        m.spawn(heading("CHARACTER"));
                        m.spawn(row()).with_children(|r| {
                            r.spawn(label("Sex", 110.0));
                            r.spawn(button(110.0, 30.0, "Man", 16.0, Act::Sex(false)));
                            r.spawn(button(110.0, 30.0, "Woman", 16.0, Act::Sex(true)));
                        });
                        // per slot: `label  << < piece > >>   < swatch dye >`
                        for (s, slot) in Slot::ALL.into_iter().enumerate() {
                            m.spawn(row()).with_children(|r| {
                                r.spawn(label(slot.label(), 110.0));
                                group(r, Field::Piece(s), 236.0, true);
                                r.spawn(Node {
                                    width: px(10),
                                    ..default()
                                });
                                let f = Field::Tint(s);
                                r.spawn(button(28.0, 28.0, "<", 14.0, Act::Step(f, -1)));
                                r.spawn((
                                    Swatch(s),
                                    Node {
                                        width: px(16),
                                        height: px(16),
                                        border: UiRect::all(px(1)),
                                        ..default()
                                    },
                                    BackgroundColor(INK),
                                    BorderColor::all(LINE),
                                ));
                                r.spawn((
                                    Value(f),
                                    Text::new(value(cfg, &cat, &profile, f)),
                                    digits(14.0),
                                    TextColor(INK),
                                    TextLayout {
                                        justify: Justify::Center,
                                        linebreak: LineBreak::NoWrap,
                                        ..default()
                                    },
                                    Node {
                                        width: px(70),
                                        height: px(20),
                                        overflow: Overflow::clip(),
                                        ..default()
                                    },
                                ));
                                r.spawn(button(28.0, 28.0, ">", 14.0, Act::Step(f, 1)));
                            });
                        }
                        m.spawn(row()).with_children(|r| {
                            r.spawn(label("", 110.0));
                            r.spawn(button(130.0, 32.0, "RANDOM", 16.0, Act::Random(false)));
                            r.spawn(button(130.0, 32.0, "RANDOM DYES", 15.0, Act::Random(true)));
                            r.spawn(button(110.0, 32.0, "RESET", 16.0, Act::Reset));
                            r.spawn(button(130.0, 32.0, "SAVE", 18.0, Act::Save));
                        });
                        m.spawn((
                            Value(Field::Saved),
                            Text::new(value(cfg, &cat, &profile, Field::Saved)),
                            body(15.0),
                            TextColor(ACCENT),
                            Node {
                                margin: UiRect::left(px(116)),
                                ..default()
                            },
                        ));
                    });
                // Frames the character preview showing through the gap between the panels.
                p.spawn((
                    Node {
                        flex_grow: 1.0,
                        align_self: AlignSelf::Stretch,
                        margin: UiRect::vertical(px(4)),
                        border: UiRect::all(px(1)),
                        ..default()
                    },
                    BorderColor::all(LINE),
                ));
                // the right column: the view controls and the effects (dash, slash) settings
                p.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(16),
                    ..default()
                })
                .with_children(|col| {
                col.spawn(panel(300.0, AlignItems::FlexStart))
                    .with_children(|m| {
                        m.spawn(heading("VIEW"));
                        m.spawn(Node {
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: px(6),
                            row_gap: px(6),
                            ..default()
                        })
                        .with_children(|g| {
                            for (i, f) in FOCUS.iter().enumerate() {
                                g.spawn(button(84.0, 30.0, f.0, 15.0, Act::Focus(i)));
                            }
                        });
                        m.spawn((
                            Text::new("Drag the character to turn it, mouse wheel to zoom. Dyes are not in the original game; weapons and armour stats are set in INVENTORY."),
                            body(14.0),
                            TextColor(DIM),
                            Node {
                                width: px(270),
                                ..default()
                            },
                        ));
                    });
                col.spawn(panel(300.0, AlignItems::FlexStart))
                    .with_children(fx_ui);
                });
            });
            root.spawn(page(Page::Clan))
                .with_children(|p| clan::fill(p, &clan_art));
            root.spawn(page(Page::Controls))
                .with_children(|p| crate::controls::fill(p, false));
            root.spawn(page(Page::Graphics))
                .with_children(|p| crate::gfx::fill(p, false));
            for page_kind in [Page::Shop, Page::Inventory] {
                root.spawn(page(page_kind))
                    .with_children(|p| shop::fill(p, page_kind));
            }
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: px(24),
                right: px(24),
                bottom: px(24),
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            })
            .with_children(|f| {
                // the browser page is this menu: nothing to quit to (the node keeps START right)
                if cfg!(target_arch = "wasm32") {
                    f.spawn(Node::default());
                } else {
                    f.spawn(quiet("QUIT", 18.0, Act::Quit));
                }
                f.spawn(Node {
                    column_gap: px(16),
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|r| {
                    // no LAN in the browser
                    if !cfg!(target_arch = "wasm32") {
                        r.spawn(button(180.0, 48.0, "JOIN LAN", 22.0, Act::Lan(false)));
                        r.spawn(button(180.0, 48.0, "HOST LAN", 22.0, Act::Lan(true)));
                    }
                    r.spawn(primary(260.0, 56.0, "START", 26.0, Act::Start));
                });
            });
        });
    commands.insert_resource(clan_art);
}

#[allow(clippy::too_many_arguments)]
#[cfg_attr(target_arch = "wasm32", allow(unused_variables, unused_mut))]
fn act(
    clicks: Query<(&Interaction, &Act), Changed<Interaction>>,
    keys: Res<ButtonInput<KeyCode>>,
    cat: Res<Catalog>,
    mut state: ResMut<State>,
    mut profile: ResMut<Profile>,
    mut clan_ui: ResMut<clan::Ui>,
    mut orbit: ResMut<Orbit>,
    mut demo: ResMut<Demo>,
    mut fx: ResMut<Fx>,
    mut launch: ResMut<Launch>,
    mut next: ResMut<NextState<Screen>>,
    mut exit: MessageWriter<AppExit>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        exit.write(AppExit::Success);
    }
    for (i, a) in &clicks {
        if *i != Interaction::Pressed {
            continue;
        }
        match *a {
            Act::Page(p) => state.page = p,
            Act::Map(m) => state.cfg.map = Some(cat.maps[m].clone()),
            Act::Mode(m) => {
                // Each mode has its own limit lists and defaults (`gametypecfg.xml`).
                let lim = m.limits();
                state.cfg.mode = m;
                if m == Mode::Blitzkrieg {
                    state.cfg.map = Some("blitzkrieg".into());
                }
                state.cfg.time_limit = Some(lim.minutes_default * 60).filter(|&t| t > 0);
                state.cfg.kill_limit = Some(lim.kills_default).filter(|&k| k > 0);
            }
            Act::Sex(w) => {
                // Armour is made for one sex.
                if w != state.cfg.woman {
                    profile.equipped[4..].fill(0);
                }
                state.cfg.woman = w;
                // the pieces are made for one sex
                state.cfg.look = Look::default();
            }
            Act::Step(f, d) => step(&mut state.cfg, &cat, &profile, f, d),
            Act::Random(dyes_only) => {
                let seed = crate::profile::wall().as_nanos() as u64;
                let fresh = cat.wardrobe(state.cfg.woman).random(seed);
                let look = &mut state.cfg.look;
                if !dyes_only {
                    *look = fresh;
                } else {
                    // a dye on every piece but the face (its dye is the skin tone)
                    for (s, t) in look.tints.iter_mut().enumerate() {
                        let r = (seed >> (s * 5)) as usize;
                        *t = if s == 1 { 0 } else { (r % TINTS.len()) as u8 };
                    }
                }
            }
            Act::Reset => state.cfg.look = Look::default(),
            Act::Save => {
                (profile.woman, profile.look) = (state.cfg.woman, state.cfg.look);
                profile.save();
            }
            Act::Focus(f) => {
                orbit.focus = f;
                orbit.zoom = 1.0;
            }
            Act::Style(s) => profile.dash.style = s,
            Act::Cut(s) => profile.slash.style = s,
            Act::Hue(Tab::Dash, h) => profile.dash.hue = h,
            Act::Hue(Tab::Slash, h) => profile.slash.hue = h,
            Act::Amount(t, d) => {
                let n = (amount(&profile, t) * 20.0).round() + d as f32;
                let n = n.clamp(0.0, 20.0) / 20.0;
                match t {
                    Tab::Dash => profile.dash.amount = n,
                    Tab::Slash => profile.slash.amount = n,
                }
            }
            Act::Tab(t) => fx.0 = t,
            Act::Demo => {
                demo.wait = 0.0;
                demo.again = true;
            }
            Act::Start | Act::Lan(_) => {
                state.cfg.net = match *a {
                    Act::Lan(true) => Some(Net::Host),
                    Act::Lan(false) => Some(Net::Join("lan".into())),
                    _ => None,
                };
                // only the respawning deathmatch modes are played over the LAN (`net::supported`)
                if state.cfg.net == Some(Net::Host) && !crate::net::supported(state.cfg.mode) {
                    continue;
                }
                if state.cfg.mode == Mode::ClanWar && profile.clan.is_none() {
                    clan::need_clan(&mut state, &mut clan_ui);
                    continue;
                }
                // a locked quest does not start: the "needs" line says why
                if state.cfg.mode == Mode::Quest
                    && let Some(q) = &cat.quest
                {
                    let name = state
                        .cfg
                        .scenario
                        .clone()
                        .or(cat.scenarios.first().cloned());
                    let cfg = &state.cfg;
                    match q.admit(
                        name.as_deref(),
                        cfg.sacrifice,
                        &profile.quest_items,
                        profile.level(),
                    ) {
                        Ok(a) => state.cfg.scenario = Some(a.scenario),
                        Err(_) => continue,
                    }
                }
                if state.cfg.mode == Mode::Blitzkrieg {
                    state.cfg.map = Some("blitzkrieg".into());
                }
                if state.cfg.mode == Mode::Quest {
                    // A quest picks its own map from the scenario.
                    state.cfg.map = None;
                    if let Some(first) = cat.scenarios.first() {
                        state.cfg.scenario.get_or_insert_with(|| first.clone());
                    }
                } else {
                    state.cfg.scenario = None;
                }
                (profile.woman, profile.look) = (state.cfg.woman, state.cfg.look);
                state.cfg.loadout = profile.equipped[..4]
                    .iter()
                    .copied()
                    .take_while(|&i| i != 0)
                    .collect();
                profile.save();
                // the browser page loads the map and starts the match (`web::play`, `gunzExit`)
                #[cfg(target_arch = "wasm32")]
                {
                    crate::web::play(&state.cfg);
                    exit.write(AppExit::Success);
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    launch.config = state.cfg.clone();
                    next.set(Screen::Loading);
                }
            }
            Act::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}

/// Keeps value texts, selected buttons and the visible page in line with [`State`].
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn refresh(
    mut commands: Commands,
    state: Res<State>,
    profile: Res<Profile>,
    cat: Res<Catalog>,
    orbit: Res<Orbit>,
    fx: Res<Fx>,
    mut values: Query<(&Value, &mut Text)>,
    mut swatches: Query<(&Swatch, &mut BackgroundColor)>,
    buttons: Query<(Entity, &Act, Has<Chosen>)>,
    mut pages: Query<(&PageRoot, &mut Node), (Without<ModeShow>, Without<TabShow>)>,
    mut modes: Query<(&ModeShow, &mut Node), Without<TabShow>>,
    mut tabs: Query<(&TabShow, &mut Node), (Without<ModeShow>, Without<PageRoot>)>,
) {
    if !state.is_changed() && !profile.is_changed() && !orbit.is_changed() && !fx.is_changed() {
        return;
    }
    for (s, mut bg) in &mut swatches {
        let t = TINTS[state.cfg.look.tints[s.0] as usize % TINTS.len()].1;
        let c = BackgroundColor(Color::srgb(t[0], t[1], t[2]));
        if *bg != c {
            *bg = c;
        }
    }
    let cfg = &state.cfg;
    for (v, mut t) in &mut values {
        let s = value(cfg, &cat, &profile, v.0);
        if t.0 != s {
            t.0 = s;
        }
    }
    for (e, a, chosen) in &buttons {
        let on = match *a {
            Act::Page(p) => p == state.page,
            Act::Map(m) => cfg.map.as_ref() == Some(&cat.maps[m]),
            Act::Mode(m) => m == cfg.mode,
            Act::Sex(w) => w == cfg.woman,
            Act::Focus(f) => f == orbit.focus,
            Act::Style(s) => s == profile.dash.style,
            Act::Cut(s) => s == profile.slash.style,
            Act::Hue(Tab::Dash, h) => h == profile.dash.hue,
            Act::Hue(Tab::Slash, h) => h == profile.slash.hue,
            Act::Tab(t) => t == fx.0,
            _ => false,
        };
        match (on, chosen) {
            (true, false) => drop(commands.entity(e).insert(Chosen)),
            (false, true) => drop(commands.entity(e).remove::<Chosen>()),
            _ => {}
        }
    }
    for (p, mut n) in &mut pages {
        n.display = if p.0 == state.page {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (m, mut n) in &mut modes {
        n.display = if m.0 == (cfg.mode == Mode::Quest) {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (t, mut n) in &mut tabs {
        n.display = if t.0 == fx.0 {
            Display::Flex
        } else {
            Display::None
        };
    }
}

/// Respawns the character model when sex or clothes change.
#[allow(clippy::too_many_arguments)]
fn preview(
    mut commands: Commands,
    state: Res<State>,
    cat: Res<Catalog>,
    mut clips: ResMut<Clips>,
    root: Single<(Entity, &mut Visibility), With<Preview>>,
    mut shown: ResMut<Shown>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    profile: Res<Profile>,
) {
    let (parent, mut vis) = root.into_inner();
    let want = if state.page == Page::Player {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if *vis != want {
        *vis = want;
    }
    let cfg = &state.cfg;
    // the slash demo's sword: the equipped melee weapon if it is a katana, else the starting one
    let katana = [profile.equipped[0], DEFAULT_LOADOUT[0]]
        .into_iter()
        .find(|&id| {
            let w = cat.items.get(id).and_then(|i| i.weapon.as_ref());
            w.is_some_and(|w| matches!(w.kind, WeaponKind::Katana))
        })
        .unwrap_or(DEFAULT_LOADOUT[0]);
    if shown.0 == Some((cfg.woman, cfg.look, katana)) {
        return;
    }
    shown.0 = Some((cfg.woman, cfg.look, katana));
    commands.entity(parent).despawn_children();
    let ch = cat.character(cfg.woman);
    let mut textures = Textures::new(&cat.vfs, "model/");
    let mut spawn = |look: &Look| {
        character::spawn(
            &mut commands,
            &mut meshes,
            &mut bindposes,
            &mut images,
            &mut materials,
            &mut textures,
            &cat.vfs,
            ch,
            look,
            Transform::IDENTITY,
        )
    };
    // a piece that cannot be read (a failed browser download) shows the base body
    let model = spawn(&cfg.look.fit(ch))
        .or_else(|e| {
            warn!("{}: {e}", ch.name);
            spawn(&Look::default())
        })
        .unwrap_or_else(|e| panic!("{}: {e}", ch.name));
    commands.entity(parent).add_child(model.root);
    // the sword in the hand, hidden until the slash demo swings it
    let hand = model.nodes.get("eq_wd_katana");
    let wm = cat.items.get(katana).and_then(|i| cat.items.model(i));
    if let (Some(&hand), Some(wm)) = (hand, wm) {
        let loaded = (cat.vfs.read(&wm.elu).map_err(|e| e.to_string()))
            .and_then(|b| elu::load(&b).map_err(|e| e.to_string()));
        match loaded {
            Ok(elu) => {
                let dir = &wm.elu[..wm.elu.rfind('/').map_or(0, |i| i + 1)];
                let m = model::spawn_elu(
                    &mut commands,
                    &mut meshes,
                    &mut bindposes,
                    &elu,
                    &mut |m| textures.standard(&mut images, &mut materials, dir, m),
                    Transform::IDENTITY,
                    |n| !n.name.starts_with("empty_cartridge"),
                );
                m.attach(&mut commands, hand);
                commands.entity(m.root).insert((Sword, Visibility::Hidden));
            }
            Err(e) => warn!("{}: {e}", wm.elu),
        }
    }
    // the lobby pose, so the character is alive on the stage
    if let Some((ani, looping)) = clips.get(&cat, cfg.woman, IDLE.0, IDLE.1) {
        let mut idle = Animator::new(ani, looping);
        idle.root_lock = true;
        commands.entity(model.root).insert(idle);
    }
}

/// Runs the effect demo on the player page, the shown [`Tab`]'s: [`Demo::run`] plays [`DASHES`]
/// (a tumble clip while the character slides across the stage, one trail piece per
/// [`dashfx::due`] step of the burst) or [`SLASHES`] (the katana combo with the sword in hand,
/// trailed by [`slashfx`]) and goes back to the idle pose. A run starts [`DEMO_FIRST`] s after the
/// page opens, every [`DEMO_EVERY`] s after a run, [`DEMO_SETTLE`] s after a setting of the
/// shown tab or the tab itself changed and on DEMO.
#[allow(clippy::too_many_arguments)]
fn demo(
    time: Res<Time>,
    state: Res<State>,
    profile: Res<Profile>,
    fx: Res<Fx>,
    cat: Res<Catalog>,
    mut clips: ResMut<Clips>,
    mut demo: ResMut<Demo>,
    mut model: Option<Single<(Entity, &mut Animator)>>,
    sword: Option<Single<Entity, With<Sword>>>,
    mut out: MessageWriter<dashfx::Dash>,
    mut seen: Local<Option<(Trail, Slash, Tab)>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs().min(0.05);
    let key = (profile.dash, profile.slash, fx.0);
    let before = seen.replace(key);
    let tab_changed = before.is_some_and(|b| b.2 != fx.0);
    let changed = tab_changed
        || before.is_some_and(|b| match fx.0 {
            Tab::Dash => b.0 != key.0,
            Tab::Slash => b.1 != key.1,
        });
    let sword = sword.as_deref().copied();
    let woman = state.cfg.woman;
    // leaving the page or the tab cuts a run short: back to the idle pose, sword away
    if (state.page != Page::Player || tab_changed) && demo.run.is_some() {
        hold(&mut commands, sword, false);
        if let Some(m) = model.as_mut()
            && let Some((ani, looping)) = clips.get(&cat, woman, IDLE.0, IDLE.1)
        {
            m.1.play(ani, looping, 0.12);
        }
        (demo.run, demo.x, demo.face) = (None, 0.0, 0.0);
    }
    if state.page != Page::Player {
        *demo = Demo::default();
        return;
    }
    if changed {
        demo.wait = DEMO_SETTLE;
        demo.again = true;
    }
    let Some(t) = demo.run else {
        demo.wait -= dt;
        if demo.wait <= 0.0 {
            demo.run = Some(0.0);
            demo.again = false;
            demo.step = 0;
            demo.end = if fx.0 == Tab::Slash {
                SLASH_CAP
            } else {
                DEMO_END
            };
        }
        return;
    };
    let Some(model) = model else {
        // no clips (a file that cannot be read): nothing to show
        demo.run = None;
        demo.wait = DEMO_EVERY;
        return;
    };
    let (root, mut anim) = model.into_inner();
    let now = t + dt;
    let mut x = 0.0;
    if fx.0 == Tab::Slash {
        // one swing after the other, each when the last has played out; the run ends half a
        // second after the last
        let first = t == 0.0;
        if first {
            hold(&mut commands, sword, true);
        }
        let played = anim.time >= anim.duration();
        if demo.step < SLASHES.len() && (first || played) {
            if let Some((ani, looping)) = clips.get(&cat, woman, SWORD_SET, SLASHES[demo.step]) {
                anim.play(ani, looping, if first { 0.1 } else { 0.0 });
            }
            demo.step += 1;
        } else if demo.step == SLASHES.len()
            && played
            && demo.end > now + 0.6
            && let Some((ani, looping)) = clips.get(&cat, woman, IDLE.0, IDLE.1)
        {
            anim.play(ani, looping, 0.12);
            demo.end = now + 0.5;
        }
    } else {
        let mut from = 0.0;
        for (at, to, clip) in DASHES {
            let u = now - at;
            if u >= 0.0 {
                if t < at
                    && let Some((ani, looping)) = clips.get(&cat, woman, DASH_SET, clip)
                {
                    anim.play(ani, looping, 0.0);
                }
                if dashfx::due(u, dt, crate::actor::DASH_BURST) {
                    out.write(dashfx::Dash {
                        model: root,
                        dir: Vec3::X * (to - from).signum(),
                        team: None,
                        own: true,
                    });
                }
                x = from + (to - from) * crate::ui::ease_out(u / DASH_TIME);
            }
            from = to;
        }
        // back to the idle pose when the tumble has played out
        if anim.finished()
            && let Some((ani, looping)) = clips.get(&cat, woman, IDLE.0, IDLE.1)
        {
            anim.play(ani, looping, 0.12);
        }
    }
    demo.x = x * DEMO_SPAN;
    let s = ((now / 0.3).min((demo.end - now) / 0.4)).clamp(0.0, 1.0);
    demo.face = s * s * (3.0 - 2.0 * s);
    if now >= demo.end {
        hold(&mut commands, sword, false);
        demo.run = None;
        demo.x = 0.0;
        demo.face = 0.0;
        if !demo.again {
            demo.wait = DEMO_EVERY;
        }
    } else {
        demo.run = Some(now);
    }
}

/// The demo's sword in hand and trailing (`on`) or away.
fn hold(commands: &mut Commands, sword: Option<Entity>, on: bool) {
    let Some(sword) = sword else { return };
    let mut e = commands.entity(sword);
    if on {
        e.insert((
            Visibility::Inherited,
            slashfx::Swing {
                team: None,
                own: true,
                cap: 0.16,
            },
        ));
    } else {
        e.insert(Visibility::Hidden).remove::<slashfx::Swing>();
    }
}

/// The amount sliders (drag), their filled parts, and saving the effect settings once they settle.
fn fx_panel(
    mouse: Res<ButtonInput<MouseButton>>,
    sliders: Query<(&Interaction, &RelativeCursorPosition, &Slider)>,
    mut fill: Query<(&mut Node, &Fill)>,
    mut profile: ResMut<Profile>,
    mut dragging: Local<Option<Tab>>,
    mut saved: Local<Option<(Trail, Slash)>>,
) {
    if !mouse.pressed(MouseButton::Left) {
        *dragging = None;
    } else if let Some((.., s)) = sliders.iter().find(|s| *s.0 == Interaction::Pressed) {
        *dragging = Some(s.0);
    }
    if let Some(t) = *dragging
        && let Some((_, rel, _)) = sliders.iter().find(|s| s.2.0 == t)
        && let Some(n) = rel.normalized
    {
        let v = ((n.x + 0.5).clamp(0.0, 1.0) * 20.0).round() / 20.0;
        if amount(&profile, t) != v {
            match t {
                Tab::Dash => profile.dash.amount = v,
                Tab::Slash => profile.slash.amount = v,
            }
        }
    }
    for (mut n, f) in &mut fill {
        let w = percent(amount(&profile, f.0) * 100.0);
        if n.width != w {
            n.width = w;
        }
    }
    let now = (profile.dash, profile.slash);
    match &*saved {
        None => *saved = Some(now),
        Some(s) if *s != now && !mouse.pressed(MouseButton::Left) => {
            profile.save();
            *saved = Some(now);
        }
        _ => {}
    }
}

/// The EFFECTS panel: a DASH | SLASH switch over the shown tab's style, colour and amount, then
/// the demo button and, when the graphics are ORIGINAL, a note that trails are off.
fn fx_ui(m: &mut ChildSpawnerCommands) {
    m.spawn((
        Node {
            width: percent(100),
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            padding: UiRect::bottom(px(4)),
            margin: UiRect::bottom(px(4)),
            border: UiRect::bottom(px(1)),
            ..default()
        },
        BorderColor::all(LINE),
    ))
    .with_children(|h| {
        h.spawn((Text::new("EFFECTS"), display(20.0), TextColor(ACCENT)));
        // a segmented control: the tabs share one hairline border with no gap between them
        h.spawn((
            Node {
                border: UiRect::all(px(1)),
                border_radius: BorderRadius::all(px(3)),
                overflow: Overflow::clip(),
                ..default()
            },
            BorderColor::all(LINE),
        ))
        .with_children(|r| {
            r.spawn(button(70.0, 28.0, "DASH", 14.0, Act::Tab(Tab::Dash)));
            r.spawn(button(70.0, 28.0, "SLASH", 14.0, Act::Tab(Tab::Slash)));
        });
    });
    fx_tab(
        m,
        Tab::Dash,
        STYLES.map(|(s, _, label)| (label, Act::Style(s))).to_vec(),
    );
    fx_tab(
        m,
        Tab::Slash,
        slashfx::STYLES
            .map(|(s, _, label)| (label, Act::Cut(s)))
            .to_vec(),
    );
    m.spawn(Node {
        column_gap: px(10),
        align_items: AlignItems::Center,
        ..default()
    })
    .with_children(|r| {
        r.spawn(button(124.0, 30.0, "DEMO", 16.0, Act::Demo));
        r.spawn((
            Value(Field::Note),
            crate::controls::text("", compact(13.0), DIM),
        ));
    });
}

/// One tab of the EFFECTS panel: the style buttons (two to a row), the colour and the amount.
fn fx_tab(m: &mut ChildSpawnerCommands, tab: Tab, styles: Vec<(&'static str, Act)>) {
    m.spawn((
        TabShow(tab),
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            ..default()
        },
    ))
    .with_children(|m| {
        m.spawn(Node {
            flex_wrap: FlexWrap::Wrap,
            column_gap: px(6),
            row_gap: px(6),
            ..default()
        })
        .with_children(|g| {
            for (label, act) in styles {
                g.spawn(button(131.0, 28.0, label, 14.0, act));
            }
        });
        m.spawn((
            Value(Field::Colour(tab)),
            crate::controls::text("", compact(14.0), DIM),
        ));
        m.spawn(Node {
            column_gap: px(6),
            ..default()
        })
        .with_children(|g| {
            for (h, _, label) in HUES {
                if h == Hue::Team {
                    g.spawn(button(52.0, 28.0, label, 13.0, Act::Hue(tab, h)));
                    continue;
                }
                let c = h.color(None, 0.0);
                let mut dot = Node {
                    width: px(16),
                    height: px(16),
                    border_radius: BorderRadius::all(px(8)),
                    ..default()
                };
                dot.border = UiRect::ZERO;
                g.spawn((Act::Hue(tab, h), flat(30.0, 28.0)))
                    .with_children(|b| {
                        let mut e = b.spawn((
                            dot,
                            BackgroundColor(Color::linear_rgb(
                                c.red.min(1.0),
                                c.green.min(1.0),
                                c.blue.min(1.0),
                            )),
                        ));
                        if h == Hue::Rainbow {
                            let stop = |i: f32| {
                                let c = Color::hsl(i * 60.0, 1.0, 0.5);
                                ColorStop::new(c, percent(i * 100.0 / 6.0))
                            };
                            e.insert(BackgroundGradient::from(LinearGradient::to_right(
                                (0..=6).map(|i| stop(i as f32)).collect(),
                            )));
                        }
                    });
            }
        });
        m.spawn(crate::controls::row()).with_children(|r| {
            r.spawn(crate::controls::boxed("Amount", compact(14.0), INK, 52.0));
            r.spawn((Slider(tab), crate::controls::slider_track(100.0)))
                .with_children(|t| {
                    t.spawn(crate::controls::slider_groove(6.0))
                        .with_children(|g| {
                            g.spawn((Fill(tab), crate::controls::slider_fill()));
                        });
                });
            r.spawn(button(22.0, 24.0, "-", 15.0, Act::Amount(tab, -1)));
            r.spawn(button(22.0, 24.0, "+", 15.0, Act::Amount(tab, 1)));
            r.spawn((
                Value(Field::Amount(tab)),
                crate::controls::text("", digits(14.0), ACCENT),
            ));
        });
    });
}

/// The amount of a tab's trail (0..=1).
fn amount(profile: &Profile, tab: Tab) -> f32 {
    match tab {
        Tab::Dash => profile.dash.amount,
        Tab::Slash => profile.slash.amount,
    }
}

/// Shrinks the menu to fit windows smaller than its 1280 x 720 layout (phones, small browser
/// windows). UI placed at a projected 3D point divides by the scale.
pub fn fit(windows: Query<&Window, With<PrimaryWindow>>, scale: ResMut<UiScale>) {
    fit_by(windows, scale, 1.0);
}

/// [`fit`] for the match HUD: it is anchored to the screen edges, so it may overflow the height
/// a little (`tall` = 1.25 shrinks it 20% less than the menu on a wide, short phone screen);
/// while paused or over it is the menu's scale.
pub fn fit_hud(
    windows: Query<&Window, With<PrimaryWindow>>,
    scale: ResMut<UiScale>,
    frozen: Option<Res<crate::game::Frozen>>,
) {
    // the pause and end screens and the settings overlays are as tall as the menus
    fit_by(windows, scale, if frozen.is_some() { 1.0 } else { 1.25 });
}

fn fit_by(windows: Query<&Window, With<PrimaryWindow>>, mut scale: ResMut<UiScale>, tall: f32) {
    // headless `--shot` runs have no window: the image size stands in
    let (w, h) = windows.single().map_or_else(
        |_| {
            let (w, h) = view::shot_size();
            (w as f32, h as f32)
        },
        |w| (w.width(), w.height()),
    );
    let s = (w / 1280.0).min(h / 720.0 * tall).min(1.0);
    if s > 0.0 && scale.0 != s {
        scale.0 = s;
    }
}

/// The player page's camera and turntable: dragging (mouse or one finger, away from the
/// buttons) turns the character, the wheel zooms, [`FOCUS`] picks the framing; until the
/// first drag it turns slowly on its own (windowed only, so headless shots stay
/// reproducible). Other pages get the full framing back.
#[allow(clippy::too_many_arguments)]
fn orbit(
    time: Res<Time>,
    demo: Res<Demo>,
    fx: Res<Fx>,
    shot: Option<Res<Shot>>,
    state: Res<State>,
    mut orbit: ResMut<Orbit>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<bevy::input::mouse::AccumulatedMouseMotion>,
    scroll: Res<bevy::input::mouse::AccumulatedMouseScroll>,
    touches: Res<Touches>,
    ui: Query<&Interaction>,
    mut model: Single<&mut Transform, With<Preview>>,
    mut camera: Single<&mut Transform, (With<Camera3d>, Without<Preview>)>,
    window: Query<&Window, With<PrimaryWindow>>,
    mut turned: Local<Option<f32>>,
) {
    let player = state.page == Page::Player;
    let on_ui = ui.iter().any(|i| *i != Interaction::None);
    let drag = if mouse.pressed(MouseButton::Left) {
        motion.delta.x
    } else {
        touches.iter().next().map_or(0.0, |t| t.delta().x)
    };
    if player && !on_ui && drag != 0.0 {
        *turned = Some(turned.unwrap_or(0.0) + drag * 0.01);
    }
    if player && scroll.delta.y != 0.0 {
        orbit.zoom = (orbit.zoom * (1.0 - scroll.delta.y.signum() * 0.12)).clamp(0.3, 1.6);
    }
    let yaw = match *turned {
        Some(y) => y,
        None if shot.is_none() => time.elapsed_secs() * 0.5,
        None => 0.0,
    };
    // the demo turns the character to face the camera and moves it along the stage
    let yaw = yaw + ((yaw / TAU).round() * TAU - yaw) * demo.face;
    model.rotation = Quat::from_rotation_y(yaw);
    let (_, h, d) = FOCUS[if player { orbit.focus } else { 0 }];
    // the slash demo's swings reach ~2 m: the camera backs off while one runs, so the trail
    // stays inside the preview frame
    let far = if fx.0 == Tab::Slash {
        1.0 + 1.5 * demo.face
    } else {
        1.0
    };
    let d = d * if player { orbit.zoom } else { 1.0 } * far;
    model.translation.x = demo.x * d / FOCUS[0].2;
    // the character stands in the preview frame between the panels (a third of the half width
    // right of the screen centre), clear of the CHARACTER panel; `aspect * d` scales the half
    // width of the view at distance d
    let aspect = window
        .single()
        .map_or(16.0 / 9.0, |w| w.width() / w.height().max(1.0));
    let goal = Vec3::new(if player { -0.18 * aspect * d } else { 0.0 }, h, d);
    let t = if shot.is_some() {
        1.0
    } else {
        1.0 - (-time.delta_secs() * 8.0).exp()
    };
    camera.translation = camera.translation.lerp(goal, t);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The browser page starts a match with the menu's flags (`web::play`), so they must parse back.
    #[test]
    fn flags_round_trip() {
        let mut args: Vec<String> = [
            "--char",
            "woman",
            "--look",
            "12,0,3,0,0,7;1,0,0,19,0,0",
            "--loadout",
            "1,2,3",
            "--bots",
            "5",
            "--skill",
            "0.7",
            "--mode",
            "tdm",
            "--time-limit",
            "90",
            "--kill-limit",
            "0",
            "--dice",
            "3",
            "--sacrifice",
            "200008,200018",
        ]
        .map(String::from)
        .into();
        let c = Config::parse(&mut args, false).unwrap();
        assert!(args.is_empty());
        assert_eq!(
            (c.look.parts[0], c.look.tints[3], c.time_limit, c.kill_limit),
            (Some(11), 19, Some(90), None)
        );
        assert_eq!((c.dice, c.sacrifice), (Some(3), [200008, 200018]));
        let mut again = c.flags();
        let d = Config::parse(&mut again, false).unwrap();
        assert!(again.is_empty());
        assert_eq!(c.flags(), d.flags());
    }
}
