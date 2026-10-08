//! Offline clans (`docs/formats.md`, "Clans"). The retail server owns clans; offline the profile
//! holds one: a name, an emblem and background from `system/claniconinfo.xml`, clan points, a
//! win/loss tally and named bot members. Rival clans are generated from the retail emblem set.
//! The menu's CLAN page creates, renames, edits and leaves the clan; the "Clan war" mode (retail
//! game type 22, a clan scrim) plays it against a rival: elimination rounds, emblems and clan
//! names on the HUD header, kill feed and scoreboard, clan points settled when the match ends.

use crate::{
    actor::ActorData,
    game::{Killed, Player, Reward, Team},
    level::Level,
    menu::{Art, Mode, Page, State, button, heading, panel},
    mrs::Vfs,
    profile::{MatchGain, Profile},
    session::{Clock, Rules},
    view::decode,
};
use bevy::{
    image::ImageSampler,
    input::{ButtonState, keyboard::KeyboardInput},
    prelude::*,
    ui::{GlobalZIndex, UiTargetCamera},
};
use std::{collections::HashMap, io};

/// Longest clan name: `interface/default/clan.xml`, the create dialog's `ClanCreate_ClanName`
/// edit box (**observed** `MAXLENGTH` 12).
pub const NAME_MAX: usize = 12;
/// Shortest name (**inferred**; the data only says "the clan name is incorrect", cserror 30036).
pub const NAME_MIN: usize = 2;
/// Level and bounty to create a clan: `strings.xml` `UI_CLAN_12` (**observed**: "level 10 or
/// higher and 20,000 BT"; the older `cserror.xml` 30050 and a comment in `clan.xml` say 1000).
pub const CREATE_LEVEL: u32 = 10;
pub const CREATE_BOUNTY: u32 = 20_000;
/// Founding members besides the leader: the `clan.xml` comment on the create dialog (**observed**:
/// "an additional 4 founding members"). Offline the bots are the founders.
pub const FOUNDERS: usize = 4;
/// Most bot members a clan keeps (**inferred**).
pub const MAX_BOTS: usize = 11;
/// Members on a side of a clan war: `gametypecfg.xml` game type 22 has `MAXPLAYERS` 8 only
/// (**observed**), so 4 against 4; `clanwar.xml` has four user panels per side.
pub const WAR_SIZE: usize = 4;
/// Clan points of a new clan (**inferred**: tier 11 of `leaguetier.xml`, the middle of 0..2400).
pub const START_POINTS: u32 = 1000;
/// Rating difference a rival may have (**observed**: `league.xml` `rating_gap` 300, applied to
/// clans **inferred**).
pub const RATING_GAP: u32 = 300;
/// Elo scale (**inferred**: the chess standard; `league.xml` only names an `elo_define`).
const ELO_SCALE: f32 = 400.0;
/// Atlas cell size in pixels and cells per row (**inferred** from the images: `clanicon_00.png`
/// and `clanbg_00.png` are 1024 px wide with their pictures on a 100 px grid, `OFFSET` = cell).
const CELL: f32 = 100.0;
const COLUMNS: u32 = 10;
/// Retail `claniconinfo.xml` `SOURCE`s, loaded from `interface/loadable/` (the `interface/default`
/// copy of the emblem atlas holds one logo only).
const ICON_SOURCE: &str = "clanicon_00.png";
const BACK_SOURCE: &str = "clanbg_00.png";

/// Handles of the rivals' members: four each for the 13 rivals (**inferred**: the data has no
/// names for players).
const RIVAL_HANDLES: [&str; 52] = [
    "Ash", "Bolt", "Cinder", "Dagger", "Ember", "Falcon", "Gale", "Havoc", "Iron", "Jinx", "Kite",
    "Lynx", "Moth", "Nova", "Onyx", "Pike", "Quill", "Raven", "Slate", "Thorn", "Umber", "Vex",
    "Wisp", "Xeno", "Yarrow", "Zephyr", "Rook", "Sable", "Talon", "Viper", "Wren", "Zinc", "Amber",
    "Blaze", "Cobalt", "Drift", "Echo", "Flint", "Granite", "Hawk", "Ivory", "Jade", "Knox",
    "Lark", "Mist", "Nyx", "Opal", "Pyre", "Quartz", "Ridge", "Shard", "Tundra",
];
/// Handles for the player's bot members, none shared with a rival (**inferred**); one more than
/// [`MAX_BOTS`] so the player's own name can be one of them.
const CLAN_HANDLES: [&str; 12] = [
    "Aegis", "Basalt", "Corvus", "Dune", "Flare", "Glint", "Harrier", "Indigo", "Jasper",
    "Kestrel", "Lotus", "Mirage",
];

/// One `<CLANICONINFO>`: `id` is the number of its `ICONID` (`C1000005` -> 1000005), `cell` its
/// `OFFSET` in the atlas.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub id: u32,
    pub cell: u32,
    pub name: String,
}

/// The `VISIBLE` emblems and backgrounds of `claniconinfo.xml` (the file lists 100 + 20, 52 + 9 of
/// them visible), or their strings missing: `names` maps `CLAN_ICON_*` / `CLAN_BG_*` to text.
pub fn parse_icons(
    xml: &str,
    names: &HashMap<String, String>,
) -> Result<(Vec<Entry>, Vec<Entry>), String> {
    let doc = roxmltree::Document::parse(xml.trim_start_matches('\u{feff}'))
        .map_err(|e| format!("claniconinfo.xml: {e}"))?;
    let (mut emblems, mut backs) = (Vec::new(), Vec::new());
    for n in doc
        .root_element()
        .children()
        .filter(|n| n.has_tag_name("CLANICONINFO"))
    {
        let get = |tag: &str| {
            n.children()
                .find(|c| c.has_tag_name(tag))
                .and_then(|c| c.text())
                .map(str::trim)
                .ok_or_else(|| format!("claniconinfo.xml: <{tag}> missing"))
        };
        if get("VISIBLE")? != "TRUE" {
            continue;
        }
        let emblem = get("EMBLEM")? == "TRUE";
        let want = if emblem { ICON_SOURCE } else { BACK_SOURCE };
        if !get("SOURCE")?.eq_ignore_ascii_case(want) {
            return Err(format!(
                "claniconinfo.xml: unexpected SOURCE {}",
                get("SOURCE")?
            ));
        }
        let num = |s: &str| {
            s.parse()
                .map_err(|_| format!("claniconinfo.xml: bad number {s:?}"))
        };
        let key = get("NAME")?.trim_start_matches("STR:");
        let entry = Entry {
            id: num(get("ICONID")?.trim_start_matches('C'))?,
            cell: num(get("OFFSET")?)?,
            name: names
                .get(key)
                .ok_or_else(|| format!("strings.xml: {key} missing"))?
                .clone(),
        };
        (if emblem { &mut emblems } else { &mut backs }).push(entry);
    }
    Ok((emblems, backs))
}

/// A generated rival clan. Its name and emblem are one retail emblem design (`REX(Gold)`: the clan
/// REX with the gold variant).
#[derive(Clone, Debug)]
pub struct Rival {
    pub name: String,
    pub emblem: u32,
    pub bg: u32,
    pub points: u32,
    pub members: Vec<String>,
}

/// Emblem atlases, the visible entries and the rivals built from them.
#[derive(Resource, Clone)]
pub struct ClanArt {
    icons: Handle<Image>,
    backs: Handle<Image>,
    pub emblems: Vec<Entry>,
    pub backgrounds: Vec<Entry>,
    pub rivals: Vec<Rival>,
}

fn bad(msg: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.into())
}

impl ClanArt {
    pub fn load(vfs: &Vfs, images: &mut Assets<Image>) -> io::Result<Self> {
        let text = |path: &str| {
            String::from_utf8(vfs.read(path)?).map_err(|e| bad(format!("{path}: {e}")))
        };
        let strings = text("system/strings.xml")?;
        let strings = roxmltree::Document::parse(strings.trim_start_matches('\u{feff}'))
            .map_err(|e| bad(format!("strings.xml: {e}")))?;
        let names: HashMap<String, String> = strings
            .descendants()
            .filter(|n| n.has_tag_name("STR"))
            .filter_map(|n| Some((n.attribute("id")?.to_owned(), n.text()?.to_owned())))
            .filter(|(k, _)| k.starts_with("CLAN_ICON_") || k.starts_with("CLAN_BG_"))
            .collect();
        let (emblems, backgrounds) =
            parse_icons(&text("system/claniconinfo.xml")?, &names).map_err(bad)?;
        let mut atlas = |file: &str| {
            let path = format!("interface/loadable/{file}");
            decode(&vfs.read(&path)?, "png", true, ImageSampler::linear())
                .map(|i| images.add(i))
                .ok_or_else(|| bad(format!("{path}: cannot decode")))
        };
        let rivals = rivals(&emblems, &backgrounds);
        Ok(Self {
            icons: atlas(ICON_SOURCE)?,
            backs: atlas(BACK_SOURCE)?,
            emblems,
            backgrounds,
            rivals,
        })
    }

    fn rect(cell: u32) -> Rect {
        let (x, y) = (
            (cell % COLUMNS) as f32 * CELL,
            (cell / COLUMNS) as f32 * CELL,
        );
        Rect::new(x, y, x + CELL, y + CELL)
    }

    fn node(image: &Handle<Image>, list: &[Entry], id: u32) -> ImageNode {
        let cell = list.iter().find(|e| e.id == id).map_or(0, |e| e.cell);
        ImageNode {
            rect: Some(Self::rect(cell)),
            ..ImageNode::new(image.clone())
        }
    }

    pub fn icon(&self, id: u32) -> ImageNode {
        Self::node(&self.icons, &self.emblems, id)
    }

    pub fn back(&self, id: u32) -> ImageNode {
        Self::node(&self.backs, &self.backgrounds, id)
    }
}

/// The retail emblem designs as rival clans: the design name before the colour, the colour
/// variant and background by position, points 700..=1300 in steps of 50 and four handles each
/// (all **inferred**; the data has no clans).
pub fn rivals(emblems: &[Entry], backs: &[Entry]) -> Vec<Rival> {
    let split = |e: &Entry| {
        let (design, colour) = e.name.split_once('(').unwrap_or((&e.name, ""));
        (
            design.trim().to_owned(),
            colour.trim_end_matches(')').to_owned(),
        )
    };
    let mut designs: Vec<String> = Vec::new();
    for e in emblems {
        let d = split(e).0;
        if !designs.contains(&d) {
            designs.push(d);
        }
    }
    designs
        .into_iter()
        .enumerate()
        .map(|(i, name)| {
            let of: Vec<&Entry> = emblems.iter().filter(|e| split(e).0 == name).collect();
            Rival {
                emblem: of[i % of.len()].id,
                bg: backs.get(i % backs.len().max(1)).map_or(0, |b| b.id),
                points: 700 + 50 * i as u32,
                members: (0..WAR_SIZE)
                    .map(|j| RIVAL_HANDLES[(i * WAR_SIZE + j) % RIVAL_HANDLES.len()].to_owned())
                    .collect(),
                name,
            }
        })
        .collect()
}

/// The rival for the next war: among those within [`RATING_GAP`] of `points` the one `games`
/// selects in turn (so wars rotate), else the closest.
pub fn pick_rival(rivals: &[Rival], points: u32, games: u32) -> &Rival {
    let near: Vec<&Rival> = rivals
        .iter()
        .filter(|r| r.points.abs_diff(points) <= RATING_GAP)
        .collect();
    match near.len() {
        0 => rivals
            .iter()
            .min_by_key(|r| r.points.abs_diff(points))
            .expect("rivals"),
        n => near[games as usize % n],
    }
}

/// Elo K factor by games played: `leaguekfactorsetting.xml` (**observed**: 50 from 0 games, 30
/// from 11, 20 from 51; use for clans **inferred**).
fn k_factor(games: u32) -> f32 {
    match games {
        0..=10 => 50.0,
        11..=50 => 30.0,
        _ => 20.0,
    }
}

/// A character of a clan name: letters, digits, space, `-` and `_` (**inferred**).
fn name_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || matches!(c, ' ' | '-' | '_')
}

/// A name the create dialog takes: [`NAME_MIN`]..=[`NAME_MAX`] name characters, not starting or
/// ending with a space.
pub fn name_ok(name: &str) -> bool {
    (NAME_MIN..=NAME_MAX).contains(&name.chars().count())
        && name.trim() == name
        && name.chars().all(name_char)
}

/// The player's clan, saved in the profile as `clan=NAME|EMBLEM|BG|POINTS|WINS|LOSSES|a,b,c`.
#[derive(Clone, Debug, PartialEq)]
pub struct Clan {
    pub name: String,
    /// `ICONID` numbers of the emblem and the background.
    pub emblem: u32,
    pub bg: u32,
    pub points: u32,
    pub wins: u32,
    pub losses: u32,
    /// Bot members; the first is the officer, the player is the leader.
    pub members: Vec<String>,
}

impl Clan {
    pub fn to_text(&self) -> String {
        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.name,
            self.emblem,
            self.bg,
            self.points,
            self.wins,
            self.losses,
            self.members.join(",")
        )
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let f: Vec<&str> = text.trim().split('|').collect();
        let [name, emblem, bg, points, wins, losses, members] = f[..] else {
            return Err(format!("clan: need 7 fields, got {}", f.len()));
        };
        let num = |s: &str| {
            s.trim()
                .parse()
                .map_err(|_| format!("clan: bad number {s:?}"))
        };
        if !name_ok(name) {
            return Err(format!("clan: bad name {name:?}"));
        }
        Ok(Self {
            name: name.to_owned(),
            emblem: num(emblem)?,
            bg: num(bg)?,
            points: num(points)?,
            wins: num(wins)?,
            losses: num(losses)?,
            members: members
                .split(',')
                .filter(|m| !m.trim().is_empty())
                .map(|m| m.trim().to_owned())
                .collect(),
        })
    }

    /// Records a clan war (`score` 1 win, 0.5 draw, 0 loss) against a clan of `rival` points and
    /// returns the points gained: Elo with the K factor of the games played so far.
    pub fn settle(&mut self, rival: u32, score: f32) -> i32 {
        let expect = 1.0 / (1.0 + 10f32.powf((rival as f32 - self.points as f32) / ELO_SCALE));
        let delta = (k_factor(self.wins + self.losses) * (score - expect)).round() as i32;
        self.points = (self.points as i32 + delta).max(0) as u32;
        if score > 0.5 {
            self.wins += 1;
        } else if score < 0.5 {
            self.losses += 1;
        }
        delta
    }

    /// `1` = best, among this clan and the rivals, and how many clans there are.
    pub fn ranking(&self, rivals: &[Rival]) -> (usize, usize) {
        let ahead = rivals.iter().filter(|r| r.points > self.points).count();
        (ahead + 1, rivals.len() + 1)
    }
}

/// Handles not worn by `members` (and the player), for a recruit.
fn free_handles<'a>(
    members: &'a [String],
    player: &'a str,
) -> impl Iterator<Item = &'static str> + 'a {
    CLAN_HANDLES
        .into_iter()
        .filter(move |h| *h != player && !members.iter().any(|m| m == h))
}

impl Profile {
    /// Founds the clan (`messages.xml` 1107 "Clan created"); the error texts are the retail ones
    /// (`cserror.xml` 30032..30051, `messages.xml` 1111).
    pub fn create_clan(
        &mut self,
        name: &str,
        (emblem, bg): (u32, u32),
        rivals: &[Rival],
    ) -> Result<(), String> {
        if self.clan.is_some() {
            return Err("You have already joined a clan.".into());
        }
        if !name_ok(name) {
            return Err("The clan name is incorrect.".into());
        }
        if rivals.iter().any(|r| r.name.eq_ignore_ascii_case(name)) {
            return Err("The clan name is already in use.".into());
        }
        if self.level() < CREATE_LEVEL {
            return Err("Only level 10 or higher can create a clan.".into());
        }
        if self.bounty < CREATE_BOUNTY {
            return Err("Insufficient bounty (20,000 BT) for clan creation.".into());
        }
        self.bounty -= CREATE_BOUNTY;
        let members: Vec<String> = free_handles(&[], &self.name)
            .take(FOUNDERS)
            .map(String::from)
            .collect();
        self.clan = Some(Clan {
            name: name.into(),
            emblem,
            bg,
            points: START_POINTS,
            wins: 0,
            losses: 0,
            members,
        });
        Ok(())
    }

    pub fn rename_clan(&mut self, name: &str, rivals: &[Rival]) -> Result<(), String> {
        let clan = self
            .clan
            .as_mut()
            .ok_or("You are not a member of any clan.")?;
        if !name_ok(name) {
            return Err("The clan name is incorrect.".into());
        }
        if rivals.iter().any(|r| r.name.eq_ignore_ascii_case(name)) {
            return Err("The clan name is already in use.".into());
        }
        clan.name = name.into();
        Ok(())
    }

    /// Adds a bot member ("A new member has joined the clan.", `messages.xml` 1115).
    pub fn recruit(&mut self) -> Result<(), String> {
        let player = self.name.clone();
        let clan = self
            .clan
            .as_mut()
            .ok_or("You are not a member of any clan.")?;
        if clan.members.len() >= MAX_BOTS {
            return Err("Cannot add an additional clan member.".into());
        }
        let h = free_handles(&clan.members, &player)
            .next()
            .ok_or("No handle left.")?;
        clan.members.push(h.into());
        Ok(())
    }

    /// Removes the newest bot member; a war needs [`WAR_SIZE`] members with the leader.
    pub fn kick(&mut self) -> Result<(), String> {
        let clan = self
            .clan
            .as_mut()
            .ok_or("You are not a member of any clan.")?;
        if clan.members.len() < WAR_SIZE {
            return Err(format!("A clan war needs {WAR_SIZE} members."));
        }
        clan.members.pop();
        Ok(())
    }
}

// ---------------------------------------------------------------------------------------------
// Emblem widget shared by the menu and the HUD.

/// A square emblem on its background.
pub fn mark(art: &ClanArt, emblem: u32, bg: u32, size: f32) -> impl Bundle + use<> {
    (
        Node {
            width: px(size),
            height: px(size),
            flex_shrink: 0.0,
            ..default()
        },
        art.back(bg),
        children![(
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            art.icon(emblem),
        )],
    )
}

fn line(s: impl Into<String>, size: f32, color: Color) -> (Text, TextFont, TextColor) {
    (
        Text::new(s),
        TextFont::from_font_size(size),
        TextColor(color),
    )
}

const GOLD: Color = Color::srgb(1.0, 0.85, 0.3);
const GREY: Color = Color::srgb(0.85, 0.85, 0.85);

// ---------------------------------------------------------------------------------------------
// The menu's CLAN page.

/// The menu page's editing state: the name being typed, the emblem picked while there is no clan.
#[derive(Resource, Default, Clone, PartialEq)]
pub(crate) struct Ui {
    draft: String,
    typing: bool,
    emblem: usize,
    back: usize,
    msg: String,
    /// "Leave" was pressed once and waits for its confirmation.
    sure: bool,
}

#[derive(Component, Clone, Copy)]
enum Act {
    Emblem(i32),
    Back(i32),
    Name,
    Create,
    Rename,
    Recruit,
    Kick,
    Leave,
}

#[derive(Component, Clone, Copy)]
enum Txt {
    Name,
    Msg,
    Emblem,
    Back,
    Info,
    Members,
}

/// Shown only while the profile has (`true`) or lacks (`false`) a clan.
#[derive(Component)]
struct Show(bool);

/// The ranking table's rows live here.
#[derive(Component)]
struct Table;

/// The big emblem on the page (its first child is the emblem on top of the background).
#[derive(Component)]
struct Preview;

/// The `Start` of a clan war with no clan sends the player here (`messages.xml` 1112).
const NO_CLAN: &str = "You are not a member of any clan.";

pub(crate) fn fill(p: &mut ChildSpawnerCommands, art: &Art, clan: &ClanArt) {
    let row = || Node {
        align_items: AlignItems::Center,
        column_gap: px(6),
        ..default()
    };
    let cap = |s: &str, w: f32| {
        (
            line(s, 18.0, GREY),
            Node {
                width: px(w),
                ..default()
            },
        )
    };
    let value = |t: Txt| {
        (
            t,
            line("", 18.0, Color::WHITE),
            Node {
                width: px(220),
                ..default()
            },
        )
    };
    p.spawn(panel(560.0, AlignItems::FlexStart))
        .with_children(|m| {
            m.spawn(heading("CLAN"));
            m.spawn(row()).with_children(|r| {
                r.spawn((
                    Preview,
                    mark(clan, clan.emblems[0].id, clan.backgrounds[0].id, 110.0),
                ));
                r.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(8),
                    ..default()
                })
                .with_children(|c| {
                    for (name, step, t) in [
                        ("Emblem", Act::Emblem as fn(i32) -> Act, Txt::Emblem),
                        ("Background", Act::Back, Txt::Back),
                    ] {
                        c.spawn(row()).with_children(|r| {
                            r.spawn(cap(name, 110.0));
                            r.spawn(button(art, 30.0, 28.0, "<", 16.0, step(-1)));
                            r.spawn(value(t));
                            r.spawn(button(art, 30.0, 28.0, ">", 16.0, step(1)));
                        });
                    }
                });
            });
            m.spawn(row()).with_children(|r| {
                r.spawn(cap("Name", 110.0));
                r.spawn((
                    Button,
                    Act::Name,
                    Node {
                        width: px(250),
                        height: px(30),
                        align_items: AlignItems::Center,
                        padding: UiRect::horizontal(px(8)),
                        border: UiRect::all(px(1)),
                        ..default()
                    },
                    BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
                    BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.3)),
                    children![(Txt::Name, line("", 18.0, Color::WHITE))],
                ));
            });
            m.spawn((
                Show(false),
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(8),
                    ..default()
                },
            ))
            .with_children(|c| {
                c.spawn((
                    line(
                        format!(
                            "Clan creation requires level {CREATE_LEVEL} or higher and 20,000 BT, a unique name of up to {NAME_MAX} characters and {FOUNDERS} founding members (bots join for you). Click the name box and type."
                        ),
                        16.0,
                        GREY,
                    ),
                    Node {
                        width: px(500),
                        ..default()
                    },
                ));
                c.spawn(button(art, 160.0, 40.0, "CREATE", 20.0, Act::Create));
            });
            m.spawn((Show(true), row())).with_children(|r| {
                for (label, act) in [
                    ("RENAME", Act::Rename),
                    ("RECRUIT", Act::Recruit),
                    ("KICK", Act::Kick),
                    ("LEAVE", Act::Leave),
                ] {
                    r.spawn(button(art, 98.0, 36.0, label, 16.0, act));
                }
            });
            m.spawn((
                Txt::Msg,
                line("", 16.0, GOLD),
                Node {
                    width: px(500),
                    ..default()
                },
            ));
        });
    p.spawn((Show(true), panel(290.0, AlignItems::FlexStart)))
        .with_children(|m| {
            m.spawn(heading("CLAN INFO"));
            m.spawn((Txt::Info, line("", 18.0, Color::WHITE)));
            m.spawn(heading("MEMBERS"));
            m.spawn((Txt::Members, line("", 18.0, Color::WHITE)));
        });
    p.spawn(panel(340.0, AlignItems::FlexStart))
        .with_children(|m| {
            m.spawn(heading("RANKING"));
            m.spawn((
                Table,
                Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: px(3),
                    ..default()
                },
            ));
        });
}

pub struct ClanMenuPlugin;

impl Plugin for ClanMenuPlugin {
    fn build(&self, app: &mut App) {
        let draft = app.world().resource::<Profile>().clan.as_ref();
        let draft = draft.map(|c| c.name.clone()).unwrap_or_default();
        app.insert_resource(Ui { draft, ..default() })
            // `shop::ShopPlugin`'s system saves the profile after any change
            .add_systems(Update, (act, typing, refresh).chain());
    }
}

/// The emblem and background shown: the clan's, or the ones being picked for a new clan.
fn current(art: &ClanArt, ui: &Ui, profile: &Profile) -> (u32, u32) {
    match &profile.clan {
        Some(c) => (c.emblem, c.bg),
        None => (art.emblems[ui.emblem].id, art.backgrounds[ui.back].id),
    }
}

/// Moves the emblem (or background) by `d`: of the clan, or of the one being created.
fn cycle(art: &ClanArt, ui: &mut Ui, profile: &mut Profile, back: bool, d: i32) {
    let list = if back { &art.backgrounds } else { &art.emblems };
    let n = list.len() as i32;
    match profile.clan.as_mut() {
        Some(c) => {
            let id = if back { &mut c.bg } else { &mut c.emblem };
            let at = list.iter().position(|e| e.id == *id).unwrap_or(0) as i32;
            *id = list[(at + d).rem_euclid(n) as usize].id;
        }
        None => {
            let ix = if back { &mut ui.back } else { &mut ui.emblem };
            *ix = (*ix as i32 + d).rem_euclid(n) as usize;
        }
    }
}

fn act(
    clicks: Query<(&Interaction, &Act), Changed<Interaction>>,
    art: Res<ClanArt>,
    mut ui: ResMut<Ui>,
    mut profile: ResMut<Profile>,
) {
    for (i, a) in &clicks {
        if *i != Interaction::Pressed {
            continue;
        }
        ui.typing = matches!(a, Act::Name);
        let confirm = matches!(a, Act::Leave) && ui.sure;
        ui.sure = matches!(a, Act::Leave) && !confirm;
        let result = match *a {
            Act::Emblem(d) | Act::Back(d) => {
                let back = matches!(a, Act::Back(_));
                cycle(&art, &mut ui, &mut profile, back, d);
                Ok(String::new())
            }
            Act::Name => Ok(String::new()),
            Act::Create => {
                let pick = current(&art, &ui, &profile);
                let name = ui.draft.clone();
                profile
                    .create_clan(&name, pick, &art.rivals)
                    .map(|()| "Clan created".to_owned())
            }
            Act::Rename => {
                let name = ui.draft.clone();
                profile
                    .rename_clan(&name, &art.rivals)
                    .map(|()| "Clan renamed.".to_owned())
            }
            Act::Recruit => profile
                .recruit()
                .map(|()| "A new member has joined the clan.".to_owned()),
            Act::Kick => profile
                .kick()
                .map(|()| "The selected member has been removed from the clan.".to_owned()),
            Act::Leave if confirm => {
                profile.clan = None;
                ui.draft.clear();
                Ok("You have left the clan.".to_owned())
            }
            Act::Leave if profile.clan.is_some() => {
                Ok("Are you sure that you want to leave the clan? Press LEAVE again.".to_owned())
            }
            Act::Leave => Err(NO_CLAN.to_owned()),
        };
        ui.msg = result.unwrap_or_else(|e| e);
    }
}

/// Typing into the name box.
fn typing(mut keys: MessageReader<KeyboardInput>, mut ui: ResMut<Ui>) {
    for k in keys.read() {
        if k.state != ButtonState::Pressed || !ui.typing {
            continue;
        }
        match k.key_code {
            KeyCode::Backspace => drop(ui.draft.pop()),
            KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Tab => ui.typing = false,
            _ => {
                for c in k.text.iter().flat_map(|t| t.chars()) {
                    if ui.draft.chars().count() < NAME_MAX && name_char(c) {
                        ui.draft.push(c);
                    }
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn refresh(
    mut commands: Commands,
    art: Res<ClanArt>,
    ui: Res<Ui>,
    profile: Res<Profile>,
    mut texts: Query<(&Txt, &mut Text)>,
    mut shows: Query<(&Show, &mut Node)>,
    mut fields: Query<(&Act, &mut BorderColor)>,
    mut preview: Query<(&mut ImageNode, &Children), With<Preview>>,
    mut icons: Query<&mut ImageNode, Without<Preview>>,
    table: Query<Entity, With<Table>>,
    mut last: Local<Option<(Option<Clan>, Ui)>>,
) {
    let key = (profile.clan.clone(), ui.clone());
    if last.as_ref() == Some(&key) {
        return;
    }
    *last = Some(key);
    let (emblem, bg) = current(&art, &ui, &profile);
    let name_of = |list: &[Entry], id: u32| {
        list.iter()
            .find(|e| e.id == id)
            .map_or_else(String::new, |e| e.name.clone())
    };
    let clan = profile.clan.as_ref();
    let info = clan.map_or_else(String::new, |c| {
        let (rank, of) = c.ranking(&art.rivals);
        format!(
            "Clan Leader : {}\nWin/Lose : {} / {}\nPoint : {}\nRanking : {rank} / {of}",
            profile.name, c.wins, c.losses, c.points
        )
    });
    let members = clan.map_or_else(String::new, |c| {
        let mut s = format!("Clan Leader   {}", profile.name);
        for (i, m) in c.members.iter().enumerate() {
            let rank = if i == 0 {
                "Clan Officer"
            } else {
                "Clan Member"
            };
            s += &format!("\n{rank:<13} {m}");
        }
        s
    });
    for (t, mut text) in &mut texts {
        let s = match t {
            Txt::Name if ui.typing => format!("{}_", ui.draft),
            Txt::Name if ui.draft.is_empty() => "(type a name)".into(),
            Txt::Name => ui.draft.clone(),
            Txt::Msg => ui.msg.clone(),
            Txt::Emblem => name_of(&art.emblems, emblem),
            Txt::Back => name_of(&art.backgrounds, bg),
            Txt::Info => info.clone(),
            Txt::Members => members.clone(),
        };
        if text.0 != s {
            text.0 = s;
        }
    }
    for (h, mut n) in &mut shows {
        n.display = if h.0 == clan.is_some() {
            Display::Flex
        } else {
            Display::None
        };
    }
    for (a, mut b) in &mut fields {
        if matches!(a, Act::Name) {
            *b = BorderColor::all(if ui.typing {
                GOLD
            } else {
                Color::srgba(1.0, 1.0, 1.0, 0.3)
            });
        }
    }
    for (mut back, kids) in &mut preview {
        *back = art.back(bg);
        if let Some(mut icon) = kids.first().and_then(|k| icons.get_mut(*k).ok()) {
            *icon = art.icon(emblem);
        }
    }
    let Ok(table) = table.single() else { return };
    let mut rows: Vec<(String, u32, u32, u32, bool)> = art
        .rivals
        .iter()
        .map(|r| (r.name.clone(), r.points, r.emblem, r.bg, false))
        .collect();
    if let Some(c) = clan {
        rows.push((c.name.clone(), c.points, c.emblem, c.bg, true));
    }
    rows.sort_by_key(|r| (std::cmp::Reverse(r.1), !r.4));
    commands
        .entity(table)
        .despawn_children()
        .with_children(|t| {
            for (i, (name, points, emblem, bg, you)) in rows.iter().enumerate() {
                t.spawn(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    ..default()
                })
                .with_children(|r| {
                    r.spawn(mark(&art, *emblem, *bg, 26.0));
                    let color = if *you { GOLD } else { GREY };
                    r.spawn(line(
                        format!("{:>2}. {name:<12} {points:>5}", i + 1),
                        17.0,
                        color,
                    ));
                });
            }
        });
}

/// Called by the menu when Start is pressed for a clan war without a clan: opens the CLAN page.
pub(crate) fn need_clan(state: &mut State, ui: &mut Ui) {
    state.page = Page::Clan;
    ui.msg = NO_CLAN.into();
}

// ---------------------------------------------------------------------------------------------
// The clan war in game.

/// One side of the war: the clan and its four fighters.
#[derive(Clone, Debug)]
pub struct Side {
    pub name: String,
    pub emblem: u32,
    pub bg: u32,
    pub points: u32,
    /// Who fights, the player first on the player's side.
    pub members: Vec<String>,
}

/// The running clan war (inserted at startup in `Mode::ClanWar`): `sides[0]` is the player's
/// clan (Red), `sides[1]` the rival (Blue).
#[derive(Resource)]
pub struct ClanWar {
    pub sides: [Side; 2],
    /// Points gained once the match is settled.
    pub delta: Option<i32>,
    next: [usize; 2],
}

/// An actor of the war: its side and its name without the clan.
#[derive(Component)]
struct Member {
    side: usize,
    name: String,
}

#[derive(Component)]
struct Overlay;

#[derive(Component)]
struct Strip;

#[derive(Component)]
struct Points(usize);

#[derive(Component)]
struct FeedBox;

/// Recent kills: seconds they expire at, the killer's side and name, weapon (`None` = suicide),
/// the victim's side and name.
#[derive(Resource, Default)]
struct FeedLog(Vec<(f32, (usize, String), Option<String>, (usize, String))>);

const FEED_LIFE: f32 = 6.0;
const FEED_LINES: usize = 6;

pub struct ClanPlugin;

impl Plugin for ClanPlugin {
    fn build(&self, app: &mut App) {
        let war = |rules: Option<Res<Rules>>| rules.is_some_and(|r| r.mode == Mode::ClanWar);
        app.init_resource::<FeedLog>()
            .add_systems(Startup, setup.run_if(war))
            .add_systems(
                Update,
                (
                    tag,
                    overlay.run_if(not(any_with_component::<Overlay>)),
                    strip,
                    feed,
                    settle,
                )
                    .run_if(resource_exists::<ClanWar>),
            );
    }
}

fn setup(
    mut commands: Commands,
    level: Res<Level>,
    profile: Res<Profile>,
    mut images: ResMut<Assets<Image>>,
) {
    let art = ClanArt::load(&level.vfs, &mut images).unwrap_or_else(|e| panic!("clan data: {e}"));
    let Some(me) = &profile.clan else {
        error!("clan war without a clan: create one in the menu's CLAN page");
        return;
    };
    let rival = pick_rival(&art.rivals, me.points, me.wins + me.losses);
    let fighters = std::iter::once(profile.name.clone()).chain(me.members.iter().cloned());
    let war = ClanWar {
        sides: [
            Side {
                name: me.name.clone(),
                emblem: me.emblem,
                bg: me.bg,
                points: me.points,
                members: fighters.take(WAR_SIZE).collect(),
            },
            Side {
                name: rival.name.clone(),
                emblem: rival.emblem,
                bg: rival.bg,
                points: rival.points,
                members: rival.members.clone(),
            },
        ],
        delta: None,
        next: [1, 0],
    };
    info!(
        "clan war: {} ({} points) against {} ({} points)",
        war.sides[0].name, war.sides[0].points, war.sides[1].name, war.sides[1].points
    );
    commands.insert_resource(war);
    commands.insert_resource(art);
}

/// Names the actors after the clans: Red is the player's clan (the player first, then its bot
/// members), Blue the rival; `Phoenix.Ash` in the scoreboard.
fn tag(
    mut commands: Commands,
    mut war: ResMut<ClanWar>,
    mut actors: Query<(Entity, &Team, Has<Player>, &mut Name), Added<Team>>,
) {
    for (e, team, player, mut name) in &mut actors {
        let side = (*team == Team::Blue) as usize;
        let at = if player { 0 } else { war.next[side] };
        if !player {
            war.next[side] += 1;
        }
        let who = war.sides[side]
            .members
            .get(at)
            .cloned()
            .unwrap_or_else(|| name.as_str().to_owned());
        *name = Name::new(format!("{}.{who}", war.sides[side].name));
        commands.entity(e).insert(Member { side, name: who });
    }
}

/// The HUD additions: the header's emblems and names beside the clock, the scoreboard's clan
/// strip, and the emblem kill feed (the stock feed is silent while a war runs).
fn overlay(
    mut commands: Commands,
    war: Res<ClanWar>,
    art: Res<ClanArt>,
    cameras: Query<Entity, With<Camera3d>>,
) {
    let Ok(camera) = cameras.single() else { return };
    let white = Color::WHITE;
    // Beside the 460 px clock bar of the HUD (centred, 8 px from the top, 40 px high); the name
    // sits under the emblem so a long one stays clear of the "Kills / Deaths" text at the left.
    let side = |s: &Side, i: usize| {
        let at = if i == 0 { -250.0 - 120.0 } else { 250.0 };
        (
            Node {
                position_type: PositionType::Absolute,
                left: percent(50),
                margin: UiRect::left(px(at)),
                top: px(4),
                width: px(120),
                align_items: if i == 0 {
                    AlignItems::FlexEnd
                } else {
                    AlignItems::FlexStart
                },
                flex_direction: FlexDirection::Column,
                ..default()
            },
            children![
                mark(&art, s.emblem, s.bg, 48.0),
                (line(&s.name, 18.0, white), TextShadow::default()),
            ],
        )
    };
    let points = |s: &Side, i: usize| {
        (
            Points(i),
            line(format!("{}  {}", s.name, s.points), 18.0, white),
        )
    };
    let strip = |s: &Side, i: usize| {
        (
            Node {
                align_items: AlignItems::Center,
                column_gap: px(6),
                ..default()
            },
            children![mark(&art, s.emblem, s.bg, 36.0), points(s, i)],
        )
    };
    commands
        .spawn((
            Overlay,
            UiTargetCamera(camera),
            GlobalZIndex(5),
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .with_children(|r| {
            r.spawn(side(&war.sides[0], 0));
            r.spawn(side(&war.sides[1], 1));
            // Over the scoreboard's top right (the board is 720 x 440 and centred).
            r.spawn((
                Strip,
                Visibility::Hidden,
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(50),
                    top: percent(50),
                    margin: UiRect {
                        left: px(-170),
                        top: px(-212),
                        ..default()
                    },
                    column_gap: px(24),
                    ..default()
                },
            ))
            .with_children(|s| {
                s.spawn(strip(&war.sides[0], 0));
                s.spawn(strip(&war.sides[1], 1));
            });
            r.spawn((
                FeedBox,
                Node {
                    position_type: PositionType::Absolute,
                    right: px(16),
                    top: px(84),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::FlexEnd,
                    row_gap: px(3),
                    ..default()
                },
            ));
        });
}

/// The strip shows with the scoreboard (Tab or match over) and carries each clan's points; once
/// the war is settled the player's shows the gain.
fn strip(
    war: Res<ClanWar>,
    profile: Res<Profile>,
    keys: Res<ButtonInput<KeyCode>>,
    clock: Res<Clock>,
    mut vis: Query<&mut Visibility, With<Strip>>,
    mut texts: Query<(&Points, &mut Text)>,
) {
    let show = keys.pressed(KeyCode::Tab) || clock.over.is_some();
    for mut v in &mut vis {
        let want = if show {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
    for (Points(i), mut t) in &mut texts {
        let s = &war.sides[*i];
        let text = match (*i, war.delta, &profile.clan) {
            (0, Some(d), Some(c)) => format!("{}  {} ({d:+})", s.name, c.points),
            _ => format!("{}  {}", s.name, s.points),
        };
        if t.0 != text {
            t.0 = text;
        }
    }
}

/// The emblem kill feed: `[emblem] killer  [weapon]  [emblem] victim`.
#[allow(clippy::too_many_arguments)]
fn feed(
    mut commands: Commands,
    time: Res<Time>,
    data: Res<ActorData>,
    art: Res<ClanArt>,
    war: Res<ClanWar>,
    mut killed: MessageReader<Killed>,
    members: Query<&Member>,
    boxes: Query<Entity, With<FeedBox>>,
    mut log: ResMut<FeedLog>,
) {
    let now = time.elapsed_secs();
    let who = |e| members.get(e).map(|m| (m.side, m.name.clone())).ok();
    let mut dirty = false;
    for k in killed.read() {
        let (Some(killer), Some(victim)) = (who(k.killer), who(k.victim)) else {
            continue;
        };
        let weapon = (k.killer != k.victim).then(|| {
            data.items
                .get(k.item)
                .and_then(|i| i.name.clone())
                .unwrap_or_else(|| "?".into())
        });
        log.0.push((now + FEED_LIFE, killer, weapon, victim));
        dirty = true;
    }
    let before = log.0.len();
    log.0.retain(|l| l.0 > now);
    let extra = log.0.len().saturating_sub(FEED_LINES);
    log.0.drain(..extra);
    if !dirty && log.0.len() == before {
        return;
    }
    let Ok(feed) = boxes.single() else { return };
    commands.entity(feed).despawn_children().with_children(|f| {
        for (_, killer, weapon, victim) in &log.0 {
            f.spawn(Node {
                align_items: AlignItems::Center,
                column_gap: px(6),
                ..default()
            })
            .with_children(|r| {
                let s = &war.sides[killer.0];
                r.spawn(mark(&art, s.emblem, s.bg, 22.0));
                match weapon {
                    Some(w) => {
                        r.spawn(line(format!("{} [{w}]", killer.1), 18.0, Color::WHITE));
                        let s = &war.sides[victim.0];
                        r.spawn(mark(&art, s.emblem, s.bg, 22.0));
                        r.spawn(line(&victim.1, 18.0, Color::WHITE));
                    }
                    None => {
                        drop(r.spawn(line(format!("{} suicide", killer.1), 18.0, Color::WHITE)))
                    }
                }
            });
        }
    });
}

/// The match is over and its result paid: records the war in the clan (Elo points, win/loss) and
/// pays the clan-war XP bonus through [`Reward`]. **Observed** (a commented-out tip in
/// `tips.xml`): "in a clan war the EXP gain is 1.5 times, with no loss for level differences",
/// so the war's earnings so far get half again as XP; bounty is not mentioned and stays as is.
fn settle(
    clock: Res<Clock>,
    gain: Res<MatchGain>,
    mut war: ResMut<ClanWar>,
    mut profile: ResMut<Profile>,
    mut rewards: MessageWriter<Reward>,
) {
    let Some(headline) = &clock.over else { return };
    if war.delta.is_some() || !gain.done {
        return;
    }
    let score = match headline.as_str() {
        "VICTORY" => 1.0,
        "DEFEAT" => 0.0,
        _ => 0.5,
    };
    let Some(clan) = profile.clan.as_mut() else {
        return;
    };
    let before = clan.points;
    let delta = clan.settle(war.sides[1].points, score);
    info!(
        "clan war {headline}: {} {before} -> {} points ({delta:+}) against {} ({}), won {} lost {}, clan EXP bonus +{}",
        clan.name,
        clan.points,
        war.sides[1].name,
        war.sides[1].points,
        clan.wins,
        clan.losses,
        gain.xp / 2
    );
    war.delta = Some(delta);
    rewards.write(Reward {
        xp: gain.xp / 2,
        bounty: 0,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = r#"<XML id="ClanIconInfo">
  <CLANICONINFO><ICONID>C1000001</ICONID><SOURCE>ClanIcon_00.png</SOURCE><OFFSET>21</OFFSET><EMBLEM>TRUE</EMBLEM><NAME>STR:CLAN_ICON_1000001</NAME><VISIBLE>TRUE</VISIBLE></CLANICONINFO>
  <CLANICONINFO><ICONID>C1000050</ICONID><SOURCE>ClanIcon_00.png</SOURCE><OFFSET>50</OFFSET><EMBLEM>TRUE</EMBLEM><NAME>STR:CLAN_ICON_1000050</NAME><VISIBLE>FALSE</VISIBLE></CLANICONINFO>
  <CLANICONINFO><ICONID>C2000003</ICONID><SOURCE>ClanBG_00.png</SOURCE><OFFSET>3</OFFSET><EMBLEM>FALSE</EMBLEM><NAME>STR:CLAN_BG_2000003</NAME><VISIBLE>TRUE</VISIBLE></CLANICONINFO>
</XML>"#;

    fn art() -> (Vec<Entry>, Vec<Entry>) {
        let names = [
            ("CLAN_ICON_1000001", "FLEX(Gold)"),
            ("CLAN_ICON_1000050", "hidden"),
            ("CLAN_BG_2000003", "Brushed Steel(Silver)"),
        ]
        .map(|(k, v)| (k.to_owned(), v.to_owned()))
        .into();
        parse_icons(XML, &names).unwrap()
    }

    #[test]
    fn icons_and_rivals() {
        let (emblems, backs) = art();
        assert_eq!(emblems.len(), 1);
        assert_eq!((emblems[0].id, emblems[0].cell), (1000001, 21));
        assert_eq!(backs[0].id, 2000003);
        assert_eq!(ClanArt::rect(21), Rect::new(100.0, 200.0, 200.0, 300.0));
        let r = rivals(&emblems, &backs);
        assert_eq!(
            (r[0].name.as_str(), r[0].emblem, r[0].points),
            ("FLEX", 1000001, 700)
        );
        assert_eq!(pick_rival(&r, 5000, 0).name, "FLEX");
    }

    #[test]
    fn names_points_and_text() {
        assert!(name_ok("Phoenix_1") && name_ok("a b"));
        assert!(!name_ok("x") && !name_ok("ThirteenChars") && !name_ok("a|b") && !name_ok(" ab"));
        let mut c = Clan {
            name: "Phoenix".into(),
            emblem: 1000001,
            bg: 2000003,
            points: 1000,
            wins: 0,
            losses: 0,
            members: vec!["Ash".into(), "Bolt".into()],
        };
        assert_eq!(Clan::parse(&c.to_text()).unwrap(), c);
        assert!(Clan::parse("Phoenix|1|2|3").is_err() && Clan::parse("a|1|2|3|4|5|").is_err());
        // Equal rating: a win is worth K/2 = 25 in the first ten games, a loss costs as much.
        assert_eq!(c.settle(1000, 1.0), 25);
        assert_eq!((c.points, c.wins), (1025, 1));
        assert!(c.settle(1025, 0.0) < 0 && c.losses == 1);
        assert_eq!(k_factor(10), 50.0);
        assert_eq!(
            (k_factor(11), k_factor(50), k_factor(51)),
            (30.0, 30.0, 20.0)
        );
    }

    #[test]
    fn creating_needs_level_and_bounty() {
        let r = rivals(&art().0, &art().1);
        let mut p = Profile::new();
        let pick = (1000001, 2000003);
        assert!(
            p.create_clan("Phoenix", pick, &r)
                .unwrap_err()
                .contains("level 10")
        );
        p.xp = 10_000;
        assert!(
            p.create_clan("FLEX", pick, &r)
                .unwrap_err()
                .contains("in use")
        );
        p.create_clan("Phoenix", pick, &r).unwrap();
        assert_eq!(p.bounty, 0);
        let c = p.clan.as_ref().unwrap();
        assert_eq!((c.points, c.members.len()), (START_POINTS, FOUNDERS));
        assert!(
            p.create_clan("Other", pick, &r)
                .unwrap_err()
                .contains("already")
        );
        p.recruit().unwrap();
        assert_eq!(p.clan.as_ref().unwrap().members.len(), FOUNDERS + 1);
        while p.kick().is_ok() {}
        assert_eq!(p.clan.as_ref().unwrap().members.len(), WAR_SIZE - 1);
    }

    fn menu_app(p: Profile) -> App {
        let (emblems, backgrounds) = art();
        let rivals = rivals(&emblems, &backgrounds);
        let mut app = App::new();
        app.add_message::<KeyboardInput>()
            .insert_resource(ClanArt {
                icons: default(),
                backs: default(),
                emblems,
                backgrounds,
                rivals,
            })
            .insert_resource(Ui {
                draft: "Phoenix".into(),
                ..default()
            })
            .insert_resource(p)
            .add_systems(Update, (act, typing).chain());
        app
    }

    /// Create, rename and leave through the buttons' `Interaction`s, and typing into the name box.
    #[test]
    fn menu_creates_renames_and_leaves() {
        let mut p = Profile::new();
        p.xp = 10_000;
        let mut app = menu_app(p);
        let press = |app: &mut App, a: Act| {
            let e = app.world_mut().spawn((Interaction::Pressed, a)).id();
            app.update();
            app.world_mut().despawn(e);
        };
        press(&mut app, Act::Create);
        let clan = |app: &App| app.world().resource::<Profile>().clan.clone();
        assert_eq!(clan(&app).unwrap().name, "Phoenix");
        assert_eq!(app.world().resource::<Ui>().msg, "Clan created");
        press(&mut app, Act::Name);
        let window = app.world_mut().spawn_empty().id();
        let key = |app: &mut App, code: KeyCode, text: Option<&str>| {
            app.world_mut().write_message(KeyboardInput {
                key_code: code,
                logical_key: bevy::input::keyboard::Key::Character(text.unwrap_or("").into()),
                state: ButtonState::Pressed,
                text: text.map(Into::into),
                repeat: false,
                window,
            });
            app.update();
        };
        key(&mut app, KeyCode::Backspace, None);
        key(&mut app, KeyCode::KeyX, Some("x"));
        key(&mut app, KeyCode::Backslash, Some("|"));
        assert_eq!(app.world().resource::<Ui>().draft, "Phoenix");
        key(&mut app, KeyCode::Backspace, None);
        key(&mut app, KeyCode::KeyX, Some("x"));
        assert_eq!(
            app.world().resource::<Ui>().draft,
            "Phoeni".to_owned() + "x"
        );
        key(&mut app, KeyCode::Enter, None);
        press(&mut app, Act::Rename);
        assert_eq!(clan(&app).unwrap().name, "Phoenix");
        press(&mut app, Act::Leave);
        assert!(clan(&app).is_some());
        press(&mut app, Act::Leave);
        assert!(clan(&app).is_none());
    }

    /// The war's end: points move once, the XP bonus is a `Reward` of half the match's XP.
    #[test]
    fn war_settles_once() {
        let mut p = Profile::new();
        p.clan = Some(Clan::parse("Phoenix|1000001|2000003|1000|0|0|Aegis,Basalt,Corvus").unwrap());
        let side = |name: &str, points| Side {
            name: name.into(),
            emblem: 0,
            bg: 0,
            points,
            members: vec![],
        };
        let mut app = App::new();
        app.add_message::<Reward>()
            .insert_resource(p)
            .insert_resource(Clock {
                over: Some("VICTORY".into()),
                ..default()
            })
            .insert_resource(MatchGain {
                xp: 60,
                done: true,
                ..default()
            })
            .insert_resource(ClanWar {
                sides: [side("Phoenix", 1000), side("REX", 700)],
                delta: None,
                next: [1, 0],
            })
            .add_systems(Update, settle);
        app.update();
        app.update();
        let clan = app.world().resource::<Profile>().clan.clone().unwrap();
        // Expected score vs 300 points lower is 0.849: K 50 x 0.151 rounds to 8.
        assert_eq!((clan.points, clan.wins, clan.losses), (1008, 1, 0));
        assert_eq!(app.world().resource::<ClanWar>().delta, Some(8));
        let rewards: Vec<u32> = app
            .world_mut()
            .resource_mut::<Messages<Reward>>()
            .drain()
            .map(|r| r.xp)
            .collect();
        assert_eq!(rewards, [30]);
    }
}
