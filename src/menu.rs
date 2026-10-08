//! `gunz-play` main menu (Bevy UI over a 3D character preview) and the run configuration it
//! edits. The menu is its own `App`: [`run`] returns the chosen [`Config`], and the binary
//! re-executes itself with the matching flags (see `src/bin/gunz-play.rs`). Art comes from
//! `interface/default/` (lobby background, logo, button textures); retail fonts are not in the
//! archives, so text uses Bevy's built-in font. Notes: `docs/formats.md` (menu).

use crate::{
    actor::DEFAULT_LOADOUT,
    character::{self, Character, Outfit},
    hud::try_image,
    item::Items,
    model::Textures,
    mrs::Vfs,
    view::{self, Shot},
};
use bevy::{
    mesh::skinning::SkinnedMeshInverseBindposes,
    prelude::*,
    text::{Justify, LineBreak},
    ui::UiTargetCamera,
};
use std::{
    str::FromStr,
    sync::{Arc, Mutex},
};

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
    /// id 1.
    Team,
    /// id 2: deathmatch with melee weapons only.
    Gladiator,
    /// id 3.
    TeamGladiator,
    /// id 9 (`DEATHMATCH_TEAM2`): team rounds, nobody respawns until the round ends. The
    /// retail strings call these rounds "Elimination" (`MISSION_DES_WEEKLY_00251`).
    Elimination,
    /// id 4: team rounds, each side hides a VIP; a round ends when one dies.
    Assassinate,
    /// id 10: one-on-one rounds, the winner stays and the loser queues.
    Duel,
    /// id 5: no bots, dummy targets.
    Training,
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
    pub const ALL: [Mode; 8] = [
        Mode::Deathmatch,
        Mode::Team,
        Mode::Gladiator,
        Mode::TeamGladiator,
        Mode::Elimination,
        Mode::Assassinate,
        Mode::Duel,
        Mode::Training,
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
        }
    }

    /// Red against Blue (actors get a [`crate::game::Team`]).
    pub fn teams(self) -> bool {
        matches!(
            self,
            Mode::Team | Mode::TeamGladiator | Mode::Elimination | Mode::Assassinate
        )
    }

    /// Loadouts are cut to the melee slot.
    pub fn melee_only(self) -> bool {
        matches!(self, Mode::Gladiator | Mode::TeamGladiator)
    }

    /// Played in rounds: nobody respawns until the round is decided.
    pub fn rounds(self) -> bool {
        matches!(self, Mode::Elimination | Mode::Assassinate | Mode::Duel)
    }

    /// Pickups use the map's `spawn_item_team_*` list instead of `spawn_item_solo_*`.
    pub fn team_items(self) -> bool {
        self.teams()
    }

    /// Whether the map's item pickups exist (*inferred*: not in the one-on-one duel, nor in
    /// the training range).
    pub fn items(self) -> bool {
        !matches!(self, Mode::Duel | Mode::Training)
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
        if self == Mode::Duel {
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
            Mode::Team => l(&[3, 5, 10, 20, 30, 50, 70, 100], 30, SHORT, 10),
            Mode::Gladiator => l(&[10, 20, 30, 50, 70, 100], 50, LONG, 30),
            Mode::TeamGladiator | Mode::Assassinate => l(&[10, 20, 30, 50, 70, 100], 30, SHORT, 10),
            Mode::Elimination => l(&[5, 10, 20, 30, 50, 70, 100], 70, LONG, 40),
            Mode::Duel => l(&[10, 15, 20, 25, 30], 20, &[1, 2, 3, 4, 5], 3),
            // The file lists 10..100 (default 50, 30 min); a training range that ends on its
            // own is no use, so it starts unlimited (0 = off, inferred).
            Mode::Training => l(&[0, 10, 20, 30, 50, 70, 100], 0, LONG, 0),
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
    /// Index into the character's `AddParts` sets; `None` keeps the default outfit.
    pub outfit: Option<usize>,
    /// zitem ids for the weapon slots; empty = [`DEFAULT_LOADOUT`].
    pub loadout: Vec<u32>,
    pub bots: usize,
    /// Bot difficulty 0..=1.
    pub skill: f32,
    /// Mouse sensitivity relative to the default (1.0).
    pub sens: f32,
    pub mode: Mode,
    pub time_limit: Option<u32>,
    pub kill_limit: Option<u32>,
}

impl Config {
    /// Takes the config flags out of `args`. Without flags a match is limited to 10 minutes
    /// and 20 kills, except in headless `--shot` runs (no limits, so test runs never end).
    pub fn parse(args: &mut Vec<String>, headless: bool) -> Result<Self, String> {
        fn get<T: FromStr>(args: &mut Vec<String>, flag: &str) -> Result<Option<T>, String> {
            take_arg(args, flag).map_err(|()| format!("{flag}: missing or bad value"))
        }
        let woman = match get::<String>(args, "--char")?.as_deref() {
            None | Some("man") => false,
            Some("woman") => true,
            Some(_) => return Err("--char: man or woman".into()),
        };
        let loadout = match get::<String>(args, "--loadout")? {
            None => Vec::new(),
            Some(s) => s
                .split(',')
                .map(|n| n.parse().map_err(|_| format!("--loadout: bad id {n:?}")))
                .collect::<Result<_, _>>()?,
        };
        let mode = get::<Mode>(args, "--mode")?.unwrap_or(Mode::Deathmatch);
        let lim = mode.limits();
        let limit = |v: Option<u32>, default: u32| match v {
            None => (!headless && default > 0).then_some(default),
            Some(n) => (n > 0).then_some(n),
        };
        let (time, kills) = (get(args, "--time-limit")?, get(args, "--kill-limit")?);
        Ok(Self {
            map: get(args, "--map")?,
            woman,
            // 1-based on the command line (like `gunz-char --set`), 0 = default outfit.
            outfit: get::<usize>(args, "--outfit")?.and_then(|n| n.checked_sub(1)),
            loadout,
            bots: get(args, "--bots")?.unwrap_or(3),
            skill: get::<f32>(args, "--skill")?.unwrap_or(0.5).clamp(0.0, 1.0),
            sens: get::<f32>(args, "--sens")?.unwrap_or(1.0).max(0.05),
            mode,
            time_limit: limit(time, lim.minutes_default * 60),
            kill_limit: limit(kills, lim.kills_default),
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
            "--sens",
            &self.sens.to_string(),
            "--mode",
            self.mode.arg(),
            "--time-limit",
            &self.time_limit.unwrap_or(0).to_string(),
            "--kill-limit",
            &self.kill_limit.unwrap_or(0).to_string(),
            "--outfit",
            &self.outfit.map_or(0, |p| p + 1).to_string(),
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
        a
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Page {
    Match,
    Player,
}

impl FromStr for Page {
    type Err = ();
    fn from_str(s: &str) -> Result<Self, ()> {
        match s {
            "match" => Ok(Page::Match),
            "player" => Ok(Page::Player),
            _ => Err(()),
        }
    }
}

/// Button textures shared by the main menu and the in-game pause menu.
#[derive(Resource, Clone)]
pub struct Art {
    up: Handle<Image>,
    over: Handle<Image>,
}

impl Art {
    pub fn load(vfs: &Vfs, images: &mut Assets<Image>) -> Self {
        let mut get = |n: &str| {
            try_image(vfs, images, n).unwrap_or_else(|| panic!("interface/default/{n} missing"))
        };
        Self {
            up: get("defaultbutton_up.png"),
            over: get("defaultbutton_over.png"),
        }
    }
}

/// Marks the selected button of a group (drawn like a hovered one).
#[derive(Component)]
pub struct Chosen;

/// A clickable retail-textured button; `act` is the system-specific action component.
pub fn button<A: Component>(
    art: &Art,
    w: f32,
    h: f32,
    label: &str,
    size: f32,
    act: A,
) -> impl Bundle {
    (
        Button,
        act,
        Node {
            width: px(w),
            height: px(h),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            ..default()
        },
        BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.3)),
        ImageNode {
            image_mode: NodeImageMode::Stretch,
            ..ImageNode::new(art.up.clone())
        },
        children![(
            Text::new(label),
            TextFont::from_font_size(size),
            TextColor(Color::WHITE)
        )],
    )
}

/// Hover/selection look of every [`button`].
pub fn hover(
    art: Res<Art>,
    mut buttons: Query<(&Interaction, Has<Chosen>, &mut ImageNode, &mut BorderColor), With<Button>>,
) {
    for (i, chosen, mut img, mut border) in &mut buttons {
        let want = if chosen || *i != Interaction::None {
            &art.over
        } else {
            &art.up
        };
        if img.image != *want {
            img.image = want.clone();
        }
        // The selected button of a group gets a gold frame.
        let frame = BorderColor::all(if chosen {
            Color::srgb(1.0, 0.8, 0.3)
        } else {
            Color::srgba(1.0, 1.0, 1.0, 0.3)
        });
        if *border != frame {
            *border = frame;
        }
    }
}

pub fn panel(width: f32, align: AlignItems) -> impl Bundle {
    (
        Node {
            width: px(width),
            align_items: align,
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            padding: UiRect::all(px(14)),
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.62)),
        BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.25)),
    )
}

pub fn heading(s: &str) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(22.0),
        TextColor(Color::srgb(0.95, 0.8, 0.45)),
    )
}

fn label(s: &str, w: f32) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(18.0),
        TextColor(Color::srgb(0.85, 0.85, 0.85)),
        Node {
            width: px(w),
            height: px(24),
            ..default()
        },
    )
}

/// What the main menu offers: maps, characters and weapon candidates per slot.
#[derive(Resource)]
struct Catalog {
    vfs: Vfs,
    items: Items,
    maps: Vec<String>,
    men: Character,
    women: Character,
    /// Candidate zitem ids per slot (melee, two ranged, one item; 0 = empty, last slot only).
    slots: [Vec<u32>; 4],
}

const SLOT_NAMES: [&str; 4] = ["Melee", "Primary", "Secondary", "Item"];

impl Catalog {
    fn load(vfs: Vfs) -> std::io::Result<Self> {
        let items = Items::load(&vfs)?;
        let mut maps: Vec<String> = vfs
            .paths()
            .filter_map(|p| {
                let (dir, file) = p.strip_prefix("maps/")?.split_once('/')?;
                (file.ends_with(".rs") && !file.contains('/')).then(|| dir.to_owned())
            })
            .collect();
        maps.sort_unstable();
        maps.dedup();
        let pick = |kind: &str, empty: bool| {
            let mut v: Vec<u32> = empty.then_some(0).into_iter().collect();
            v.extend(
                items
                    .weapons()
                    .filter(|i| i.kind == kind && i.name.is_some() && items.model(i).is_some())
                    .map(|i| i.id),
            );
            v
        };
        let slots = [
            pick("melee", false),
            pick("range", false),
            pick("range", false),
            pick("custom", true),
        ];
        Ok(Self {
            men: character::load(&vfs, "heroman1")?,
            women: character::load(&vfs, "herowoman1")?,
            vfs,
            items,
            maps,
            slots,
        })
    }

    fn character(&self, woman: bool) -> &Character {
        if woman { &self.women } else { &self.men }
    }

    fn item_name(&self, id: u32) -> String {
        match self.items.get(id) {
            None => "None".into(),
            Some(i) => {
                let name = i.name.as_deref().unwrap_or("?");
                match &i.weapon {
                    Some(w) if i.kind != "custom" => format!("{name} ({})", w.damage),
                    _ => name.to_owned(),
                }
            }
        }
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
struct State {
    cfg: Config,
    page: Page,
}

/// Where [`run`] leaves the choice when Start is pressed.
#[derive(Resource)]
struct Pick(Arc<Mutex<Option<Config>>>);

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
    Sens,
    Outfit,
    Slot(usize),
}

#[derive(Component, Clone, Copy)]
enum Act {
    Page(Page),
    Map(usize),
    Mode(Mode),
    Sex(bool),
    Step(Field, i32),
    Start,
    Quit,
}

/// Text showing the current value of a field.
#[derive(Component)]
struct Value(Field);

/// Container shown only on its page.
#[derive(Component)]
struct PageRoot(Page);

/// The character model's parent (turned slowly), shown on the player page only.
#[derive(Component)]
struct Preview;

const MAX_BOTS: i32 = 15;

/// Moves `cur` by `d` positions within `list`, clamped.
fn walk(list: &[u32], cur: u32, d: i32) -> u32 {
    let at = list.iter().position(|&v| v == cur).unwrap_or(0) as i32;
    list[(at + d).clamp(0, list.len() as i32 - 1) as usize]
}

fn step(cfg: &mut Config, cat: &Catalog, field: Field, d: i32) {
    match field {
        Field::MapName | Field::TimeName | Field::KillsName | Field::Blurb => {}
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
        Field::Sens => cfg.sens = ((cfg.sens * 4.0).round() + d as f32).clamp(1.0, 16.0) / 4.0,
        Field::Outfit => {
            let n = cat.character(cfg.woman).parts.len() as i32 + 1;
            let at = (cfg.outfit.map_or(0, |p| p + 1) as i32 + d).rem_euclid(n) as usize;
            cfg.outfit = at.checked_sub(1);
        }
        Field::Slot(s) => {
            let list = &cat.slots[s];
            let at = list.iter().position(|&v| v == cfg.loadout[s]).unwrap_or(0) as i32;
            cfg.loadout[s] = list[(at + d).rem_euclid(list.len() as i32) as usize];
        }
    }
}

fn value(cfg: &Config, cat: &Catalog, field: Field) -> String {
    let clock = |s: u32| format!("{}:{:02}", s / 60, s % 60);
    match field {
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
        Field::Sens => format!("x{:.2}", cfg.sens),
        Field::Outfit => match cfg.outfit {
            None => "Default".into(),
            Some(p) => format!("{} / {}", p + 1, cat.character(cfg.woman).parts.len()),
        },
        Field::Slot(s) => cat.item_name(cfg.loadout[s]),
    }
}

/// Opens the menu window and returns the config chosen with Start (`None`: closed/Esc/Quit).
/// Headless (`shot`), the menu shows `page` and the run ends with the screenshot.
pub fn run(vfs: Vfs, mut cfg: Config, page: Page, shot: Option<String>) -> Option<Config> {
    let cat = Catalog::load(vfs).unwrap_or_else(|e| panic!("menu data: {e}"));
    println!(
        "menu: {} maps, {} weapon candidates",
        cat.maps.len(),
        cat.slots[1].len()
    );
    // Fill every loadout slot (0 = empty item slot) so the pickers always have a value.
    let mut ids: Vec<u32> = if cfg.loadout.is_empty() {
        DEFAULT_LOADOUT.to_vec()
    } else {
        cfg.loadout.clone()
    };
    ids.resize(4, 0);
    cfg.loadout = ids;
    let default_map = cat
        .maps
        .iter()
        .find(|m| *m == "mansion")
        .or(cat.maps.first())
        .cloned();
    cfg.map = cfg.map.filter(|m| cat.maps.contains(m)).or(default_map);
    let pick = Arc::new(Mutex::new(None));
    let mut app = view::app_plain("gunz-play", shot);
    app.insert_resource(ClearColor(Color::srgb(0.05, 0.05, 0.07)))
        .insert_resource(cat)
        .insert_resource(State { cfg, page })
        .insert_resource(Pick(pick.clone()))
        .add_systems(Startup, build)
        .add_systems(
            Update,
            (
                act,
                refresh,
                hover.run_if(resource_exists::<Art>),
                preview,
                turn,
            )
                .chain(),
        )
        .run();
    pick.lock().unwrap().take()
}

fn build(
    mut commands: Commands,
    cat: Res<Catalog>,
    state: Res<State>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    shot: Option<Res<Shot>>,
) {
    let art = Art::load(&cat.vfs, &mut images);
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
    // Lobby background as a quad behind the character (UI sits on top of the 3D view).
    commands.spawn((
        Mesh3d(meshes.add(Rectangle::new(19.0, 10.7))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color_texture: Some(bg),
            base_color: Color::srgb(0.85, 0.85, 0.9),
            unlit: true,
            ..default()
        })),
        Transform::from_xyz(0.0, 1.0, -3.4),
    ));
    commands.spawn((Preview, Transform::default(), Visibility::Hidden));

    let cfg = &state.cfg;
    let row = || Node {
        align_items: AlignItems::Center,
        column_gap: px(6),
        ..default()
    };
    // `◀ value ▶` with the field's value text between two step buttons.
    let stepper = |p: &mut ChildSpawnerCommands, name: &str, f: Field, w: f32, big: bool| {
        p.spawn(row()).with_children(|r| {
            // The limit labels name what the limit means in the chosen mode.
            let mut l = r.spawn(label(name, 130.0));
            match f {
                Field::Time => drop(l.insert(Value(Field::TimeName))),
                Field::Kills => drop(l.insert(Value(Field::KillsName))),
                _ => {}
            }
            if big {
                r.spawn(button(&art, 40.0, 30.0, "<<", 16.0, Act::Step(f, -10)));
            }
            r.spawn(button(&art, 34.0, 30.0, "<", 16.0, Act::Step(f, -1)));
            r.spawn((
                Value(f),
                Text::new(value(cfg, &cat, f)),
                TextFont::from_font_size(18.0),
                TextColor(Color::WHITE),
                TextLayout {
                    justify: Justify::Center,
                    linebreak: LineBreak::NoWrap,
                    ..default()
                },
                Node {
                    width: px(w),
                    height: px(24),
                    ..default()
                },
            ));
            r.spawn(button(&art, 34.0, 30.0, ">", 16.0, Act::Step(f, 1)));
            if big {
                r.spawn(button(&art, 40.0, 30.0, ">>", 16.0, Act::Step(f, 10)));
            }
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
            // Tabs.
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: px(300),
                top: px(40),
                column_gap: px(8),
                ..default()
            })
            .with_children(|t| {
                t.spawn(button(
                    &art,
                    160.0,
                    40.0,
                    "MATCH",
                    20.0,
                    Act::Page(Page::Match),
                ));
                t.spawn(button(
                    &art,
                    160.0,
                    40.0,
                    "PLAYER",
                    20.0,
                    Act::Page(Page::Player),
                ));
            });
            let page = |page| {
                (
                    PageRoot(page),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(24),
                        top: px(120),
                        width: px(1232),
                        column_gap: px(16),
                        align_items: AlignItems::FlexStart,
                        ..default()
                    },
                )
            };
            root.spawn(page(Page::Match)).with_children(|p| {
                p.spawn(panel(676.0, AlignItems::FlexStart))
                    .with_children(|m| {
                        m.spawn(heading("MAP"));
                        m.spawn(Node {
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: px(6),
                            row_gap: px(6),
                            ..default()
                        })
                        .with_children(|g| {
                            for (i, name) in cat.maps.iter().enumerate() {
                                g.spawn(button(&art, 206.0, 34.0, &title(name), 16.0, Act::Map(i)));
                            }
                        });
                    });
                p.spawn(panel(540.0, AlignItems::FlexStart))
                    .with_children(|m| {
                        m.spawn(heading("RULES"));
                        m.spawn((
                            Value(Field::MapName),
                            Text::new(value(cfg, &cat, Field::MapName)),
                            TextFont::from_font_size(30.0),
                            TextColor(Color::WHITE),
                        ));
                        m.spawn(Node {
                            flex_wrap: FlexWrap::Wrap,
                            column_gap: px(6),
                            row_gap: px(6),
                            ..default()
                        })
                        .with_children(|g| {
                            for mode in Mode::ALL {
                                g.spawn(button(
                                    &art,
                                    122.0,
                                    30.0,
                                    mode.name(),
                                    13.0,
                                    Act::Mode(mode),
                                ));
                            }
                        });
                        m.spawn((
                            Value(Field::Blurb),
                            Text::new(value(cfg, &cat, Field::Blurb)),
                            TextFont::from_font_size(15.0),
                            TextColor(Color::srgb(0.75, 0.75, 0.75)),
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
            });
            root.spawn(page(Page::Player)).with_children(|p| {
                p.spawn(panel(420.0, AlignItems::FlexStart))
                    .with_children(|m| {
                        m.spawn(heading("CHARACTER"));
                        m.spawn(row()).with_children(|r| {
                            r.spawn(label("Sex", 130.0));
                            r.spawn(button(&art, 110.0, 30.0, "Man", 16.0, Act::Sex(false)));
                            r.spawn(button(&art, 110.0, 30.0, "Woman", 16.0, Act::Sex(true)));
                        });
                        stepper(m, "Outfit", Field::Outfit, 110.0, true);
                        stepper(m, "Mouse sens.", Field::Sens, 110.0, false);
                    });
                p.spawn(Node {
                    flex_grow: 1.0,
                    ..default()
                });
                p.spawn(panel(520.0, AlignItems::FlexStart))
                    .with_children(|m| {
                        m.spawn(heading("LOADOUT"));
                        for s in 0..4 {
                            m.spawn(row()).with_children(|r| {
                                r.spawn(label(SLOT_NAMES[s], 100.0));
                                r.spawn(button(
                                    &art,
                                    34.0,
                                    30.0,
                                    "<",
                                    16.0,
                                    Act::Step(Field::Slot(s), -1),
                                ));
                                r.spawn((
                                    Value(Field::Slot(s)),
                                    Text::new(value(cfg, &cat, Field::Slot(s))),
                                    TextFont::from_font_size(18.0),
                                    TextColor(Color::WHITE),
                                    TextLayout {
                                        justify: Justify::Center,
                                        linebreak: LineBreak::NoWrap,
                                        ..default()
                                    },
                                    Node {
                                        width: px(300),
                                        height: px(24),
                                        ..default()
                                    },
                                ));
                                r.spawn(button(
                                    &art,
                                    34.0,
                                    30.0,
                                    ">",
                                    16.0,
                                    Act::Step(Field::Slot(s), 1),
                                ));
                            });
                        }
                    });
            });
            root.spawn(Node {
                position_type: PositionType::Absolute,
                left: px(24),
                right: px(24),
                bottom: px(24),
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            })
            .with_children(|f| {
                f.spawn(button(&art, 160.0, 48.0, "QUIT", 22.0, Act::Quit));
                f.spawn(button(&art, 260.0, 56.0, "START", 28.0, Act::Start));
            });
        });
    commands.insert_resource(art);
}

fn act(
    clicks: Query<(&Interaction, &Act), Changed<Interaction>>,
    keys: Res<ButtonInput<KeyCode>>,
    cat: Res<Catalog>,
    mut state: ResMut<State>,
    pick: Res<Pick>,
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
                state.cfg.time_limit = Some(lim.minutes_default * 60).filter(|&t| t > 0);
                state.cfg.kill_limit = Some(lim.kills_default).filter(|&k| k > 0);
            }
            Act::Sex(w) => {
                state.cfg.woman = w;
                state.cfg.outfit = None;
            }
            Act::Step(f, d) => step(&mut state.cfg, &cat, f, d),
            Act::Start => {
                *pick.0.lock().unwrap() = Some(state.cfg.clone());
                exit.write(AppExit::Success);
            }
            Act::Quit => {
                exit.write(AppExit::Success);
            }
        }
    }
}

/// Keeps value texts, selected buttons and the visible page in line with [`State`].
fn refresh(
    mut commands: Commands,
    state: Res<State>,
    cat: Res<Catalog>,
    mut values: Query<(&Value, &mut Text)>,
    buttons: Query<(Entity, &Act, Has<Chosen>)>,
    mut pages: Query<(&PageRoot, &mut Node)>,
) {
    if !state.is_changed() {
        return;
    }
    let cfg = &state.cfg;
    for (v, mut t) in &mut values {
        let s = value(cfg, &cat, v.0);
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
}

/// Respawns the character model when sex or outfit change.
fn preview(
    mut commands: Commands,
    state: Res<State>,
    cat: Res<Catalog>,
    root: Single<(Entity, &mut Visibility), With<Preview>>,
    mut shown: Local<Option<(bool, Option<usize>)>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
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
    if *shown == Some((cfg.woman, cfg.outfit)) {
        return;
    }
    *shown = Some((cfg.woman, cfg.outfit));
    commands.entity(parent).despawn_children();
    let ch = cat.character(cfg.woman);
    let outfit = match cfg.outfit {
        None => Outfit::base(),
        Some(p) => Outfit::from_part(&cat.vfs, ch, p)
            .unwrap_or_else(|e| panic!("{}: outfit set {}: {e}", ch.name, p + 1)),
    };
    let mut textures = Textures::new(&cat.vfs, "model/");
    let model = character::spawn(
        &mut commands,
        &mut meshes,
        &mut bindposes,
        &mut images,
        &mut materials,
        &mut textures,
        &cat.vfs,
        ch,
        &outfit,
        Transform::IDENTITY,
    )
    .unwrap_or_else(|e| panic!("{}: {e}", ch.name));
    commands.entity(parent).add_child(model.root);
}

/// Slow turntable (windowed only; headless shots stay reproducible).
fn turn(time: Res<Time>, shot: Option<Res<Shot>>, mut q: Query<&mut Transform, With<Preview>>) {
    if shot.is_none() {
        for mut t in &mut q {
            t.rotate_y(time.delta_secs() * 0.5);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The game is started by re-executing with the menu's flags, so they must parse back.
    #[test]
    fn flags_round_trip() {
        let mut args: Vec<String> = [
            "--char",
            "woman",
            "--outfit",
            "12",
            "--loadout",
            "1,2,3",
            "--bots",
            "5",
            "--skill",
            "0.7",
            "--sens",
            "1.5",
            "--mode",
            "tdm",
            "--time-limit",
            "90",
            "--kill-limit",
            "0",
        ]
        .map(String::from)
        .into();
        let c = Config::parse(&mut args, false).unwrap();
        assert!(args.is_empty());
        assert_eq!(
            (c.outfit, c.time_limit, c.kill_limit),
            (Some(11), Some(90), None)
        );
        let mut again = c.flags();
        let d = Config::parse(&mut again, false).unwrap();
        assert!(again.is_empty());
        assert_eq!(c.flags(), d.flags());
    }
}
