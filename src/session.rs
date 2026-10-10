//! `gunz-play` match flow: rules (mode, time and kill/round limit, respawn and protection
//! times), the match clock and its top-line text, the end-of-match scoreboard, the Esc pause
//! menu and the mouse grab. Round modes, spectating, spawns and training are `modes.rs`. The
//! binary maps the exit codes below to "back to the main menu" / "play again" (re-executing
//! itself). Pausing pauses `Time<Virtual>` and inserts [`Frozen`]; actors ignore input while
//! frozen.

use crate::{
    game::{Frozen, Hold, Player, Score, Settings, Team, Vitals},
    menu::{ACCENT, Chosen, DIM, Mode, Page, button, heading, hover, panel, primary},
    modes::{Berserker, ModesPlugin, Phase, Round},
    net::Lan,
    profile::{OPTS, Profile, opt_mut},
    view::Shot,
};
use bevy::{
    prelude::*,
    text::LineBreak,
    ui::{GlobalZIndex, UiTargetCamera},
    window::{CursorGrabMode, CursorOptions, PrimaryWindow},
};

/// Exit code of `gunz-play` asking its launcher to show the main menu again.
pub const EXIT_MENU: u8 = 3;
/// Exit code asking for the same match once more.
pub const EXIT_AGAIN: u8 = 4;

/// Seconds a dead actor waits before it respawns in the deathmatch modes (*inferred*; the
/// retail Blitzkrieg mode lists `RESPAWN baseTime="8"` in `system/blitzkrieg.xml`).
pub const RESPAWN_SECS: f32 = 5.0;
/// Spawn protection after a respawn (*inferred*; Blitzkrieg's `invincibleTime="5"`).
pub const PROTECT_SECS: f32 = 3.0;
/// Round time limit (*inferred*, = the duel's default `LIMITTIME` of 3 minutes).
pub const ROUND_SECS: f32 = 180.0;
/// Countdown before a round starts (*inferred*).
pub const READY_SECS: f32 = 3.0;
/// `Dead::respawn` of an actor waiting for the next round: never counts down to zero.
pub const HOLD: f32 = 1.0e6;

/// The match rules; without this resource the game never ends.
#[derive(Resource, Clone)]
pub struct Rules {
    pub mode: Mode,
    /// Match time limit in seconds.
    pub time_limit: Option<u32>,
    /// Kills to win; in the round modes rounds (duel: duels, Spy: rounds the player's side
    /// won or lost) to end the match.
    pub kill_limit: Option<u32>,
    /// Seconds a dead actor waits to respawn (deathmatch modes).
    pub respawn: f32,
    /// Seconds of spawn protection after a respawn.
    pub protect: f32,
    /// Round modes: seconds before a round is decided on points.
    pub round_secs: f32,
    /// Round modes: countdown before each round.
    pub ready: f32,
}

impl Rules {
    /// The retail limits of `mode`; the duel's and the tournament's time limit is per round
    /// (`LIMITTIME` 1-5 minutes, *inferred*), not for the match.
    pub fn new(mode: Mode, time_limit: Option<u32>, kill_limit: Option<u32>) -> Self {
        let (time_limit, round_secs) = match (mode, time_limit) {
            (m, t) if m.duel() => (None, t.map_or(ROUND_SECS, |s| s as f32)),
            (_, t) => (t, ROUND_SECS),
        };
        Self {
            mode,
            time_limit,
            kill_limit,
            respawn: RESPAWN_SECS,
            protect: PROTECT_SECS,
            round_secs,
            ready: READY_SECS,
        }
    }
}

/// Match time (virtual seconds, so pausing stops it) and, once over, the headline.
#[derive(Resource, Default)]
pub struct Clock {
    pub elapsed: f32,
    pub over: Option<String>,
    /// The HUD's mode line under the timer: score, round and leader for the current mode (may
    /// be empty), and the timer above it.
    pub header: String,
    pub timer: String,
    /// Extra HUD text of the mode (the quest's sector and NPC count), written by its plugin.
    pub note: String,
}

/// Headless runs: open the pause menu when the match clock reaches this many seconds, with the
/// CONTROLS or GRAPHICS overlay on top for `Some(Page::Controls | Page::Graphics)`.
#[derive(Resource)]
pub struct PauseAt(pub f32, pub Option<Page>);

/// Starting health/armour of the player (`gunz-play --hp/--ap`), for HUD checks.
#[derive(Resource)]
pub struct StartVitals {
    pub hp: Option<f32>,
    pub ap: Option<f32>,
}

pub struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Settings>()
            .init_resource::<Clock>()
            .add_plugins(ModesPlugin)
            .add_systems(
                Update,
                (
                    start_vitals.run_if(resource_exists::<StartVitals>),
                    (teams, clock, pause_key, panels, buttons, cursor)
                        .chain()
                        .run_if(resource_exists::<Rules>),
                    hover,
                ),
            );
    }
}

fn start_vitals(start: Res<StartVitals>, mut q: Query<&mut Vitals, Added<Player>>) {
    for mut v in &mut q {
        v.hp = start.hp.unwrap_or(v.hp);
        v.ap = start.ap.unwrap_or(v.ap);
    }
}

/// Team modes: the player is Red; bots fill the smaller side (Blue first).
fn teams(
    rules: Res<Rules>,
    mut commands: Commands,
    new: Query<(Entity, Has<Player>), (With<Score>, Without<Team>)>,
    teams: Query<&Team>,
) {
    if !rules.mode.teams() {
        return;
    }
    let mut red = teams.iter().filter(|t| **t == Team::Red).count();
    let mut blue = teams.iter().count() - red;
    for (e, player) in &new {
        let t = if player || red < blue {
            Team::Red
        } else {
            Team::Blue
        };
        if t == Team::Red {
            red += 1;
        } else {
            blue += 1;
        }
        commands.entity(e).insert(t);
    }
}

pub fn freeze(commands: &mut Commands, time: &mut Time<Virtual>, on: bool) {
    if on {
        commands.insert_resource(Frozen);
        time.pause();
    } else {
        commands.remove_resource::<Frozen>();
        time.unpause();
    }
}

/// The player's score and the best opposing one: team kills (the player is Red) or round
/// wins in the team modes, kills otherwise.
fn standing(
    rules: &Rules,
    round: &Round,
    actors: &Query<(&Score, Has<Player>, Option<&Team>)>,
) -> (u32, u32) {
    if matches!(rules.mode, Mode::Spy | Mode::Infected) {
        return (round.mine[0], round.mine[1]);
    }
    if rules.mode.rounds() && rules.mode.teams() {
        return (round.wins[0], round.wins[1]);
    }
    if rules.mode.teams() {
        let team = |t: Team| {
            actors
                .iter()
                .filter(|a| a.2 == Some(&t))
                .map(|a| a.0.kills)
                .sum::<u32>()
        };
        return (team(Team::Red), team(Team::Blue));
    }
    let me = actors.iter().find(|a| a.1).map_or(0, |a| a.0.kills);
    let other = actors
        .iter()
        .filter(|a| !a.1)
        .map(|a| a.0.kills)
        .max()
        .unwrap_or(0);
    (me, other)
}

/// The HUD's top lines: the mode line (score, round, who leads; may be empty) and the timer.
fn header(
    rules: &Rules,
    round: &Round,
    clock: &Clock,
    (mine, theirs): (u32, u32),
    names: &Query<&Name>,
    boss: Option<&str>,
) -> (String, String) {
    let mmss = |s: u32| format!("{}:{:02}", s / 60, s % 60);
    let left = |limit: f32, used: f32| mmss((limit - used).max(0.0).ceil() as u32);
    let total = match rules.time_limit {
        Some(l) => left(l as f32, clock.elapsed),
        None => mmss(clock.elapsed as u32),
    };
    let in_round = round.n > 0 && round.phase != Phase::Done;
    let round_t = left(
        rules.round_secs,
        if round.phase == Phase::Live {
            round.t
        } else {
            0.0
        },
    );
    match rules.mode {
        Mode::Team | Mode::TeamGladiator => (format!("RED {mine} : {theirs} BLUE"), total),
        Mode::Elimination | Mode::Assassinate => (
            format!("RED {mine} : {theirs} BLUE   ROUND {}", round.n.max(1)),
            round_t,
        ),
        Mode::Duel | Mode::DuelTournament => {
            let name = |e| names.get(e).map_or("?", |n| n.as_str());
            match round.duelists() {
                Some((a, b)) if in_round && rules.mode == Mode::DuelTournament => (
                    format!("{}   {} vs {}", round.stage(), name(a), name(b)),
                    round_t,
                ),
                Some((a, b)) if in_round => (
                    format!("ROUND {}   {} vs {}", round.n, name(a), name(b)),
                    round_t,
                ),
                _ => (String::new(), total),
            }
        }
        Mode::Spy => (
            format!("YOU {mine} : {theirs} THEM   ROUND {}", round.n.max(1)),
            round_t,
        ),
        Mode::ClanWar => (
            format!("{mine} : {theirs}   ROUND {}", round.n.max(1)),
            round_t,
        ),
        Mode::Infected => (
            format!("{}   YOU {mine} : {theirs} THEM", clock.note),
            round_t,
        ),
        Mode::Berserker => match rules.kill_limit {
            Some(k) => (
                format!("BERSERKER {}   first to {k}", boss.unwrap_or("-")),
                total,
            ),
            None => (format!("BERSERKER {}", boss.unwrap_or("-")), total),
        },
        Mode::Deathmatch | Mode::Gladiator | Mode::Gunman => match rules.kill_limit {
            Some(k) => (format!("first to {k}"), total),
            None => (String::new(), total),
        },
        Mode::Quest | Mode::Blitzkrieg | Mode::GunGame | Mode::DynDuel => {
            (clock.note.clone(), total)
        }
        Mode::Training => (String::new(), total),
    }
}

fn clock(
    time: Res<Time>,
    rules: Res<Rules>,
    round: Res<Round>,
    mut clock: ResMut<Clock>,
    mut commands: Commands,
    mut vtime: ResMut<Time<Virtual>>,
    actors: Query<(&Score, Has<Player>, Option<&Team>)>,
    names: Query<&Name>,
    boss: Query<&Name, With<Berserker>>,
) {
    if clock.over.is_some() {
        return;
    }
    clock.elapsed += time.delta_secs();
    let (mine, theirs) = standing(&rules, &round, &actors);
    let boss = boss
        .single()
        .ok()
        .and_then(|n| n.as_str().split(" [").next());
    let (line, timer) = header(&rules, &round, &clock, (mine, theirs), &names, boss);
    if clock.header != line {
        clock.header = line;
    }
    if clock.timer != timer {
        clock.timer = timer;
    }
    // The tournament bracket decides the match by itself, once its last screen has shown.
    if let (Some(v), Phase::Done) = (round.verdict, round.phase) {
        clock.over = Some(v.into());
        freeze(&mut commands, &mut vtime, true);
        return;
    }
    // A decided round shows its win screen before the match can end on it.
    let settled = !rules.mode.rounds() || round.phase != Phase::Over;
    let times_up = rules.time_limit.is_some_and(|l| clock.elapsed >= l as f32);
    if !times_up && !(settled && rules.kill_limit.is_some_and(|n| mine.max(theirs) >= n)) {
        return;
    }
    clock.over = Some(
        match mine.cmp(&theirs) {
            std::cmp::Ordering::Greater => "VICTORY",
            std::cmp::Ordering::Less => "DEFEAT",
            std::cmp::Ordering::Equal => "DRAW",
        }
        .into(),
    );
    freeze(&mut commands, &mut vtime, true);
}

fn pause_key(
    keys: Res<ButtonInput<KeyCode>>,
    clock: Res<Clock>,
    frozen: Option<Res<Frozen>>,
    hold: Option<Res<Hold>>,
    pause_at: Option<Res<PauseAt>>,
    lan: Option<Res<Lan>>,
    mut commands: Commands,
    mut vtime: ResMut<Time<Virtual>>,
) {
    if clock.over.is_some() || hold.is_some() {
        return;
    }
    let scripted = pause_at.as_ref().is_some_and(|p| clock.elapsed >= p.0);
    if scripted {
        match pause_at.and_then(|p| p.1) {
            Some(Page::Controls) => commands.insert_resource(crate::controls::Overlay),
            Some(Page::Graphics) => commands.insert_resource(crate::gfx::Overlay),
            _ => {}
        }
        commands.remove_resource::<PauseAt>();
    }
    if scripted || keys.just_pressed(KeyCode::Escape) {
        freeze(&mut commands, &mut vtime, frozen.is_none());
        // A LAN match goes on for everybody else: the menu only takes the player's input away.
        if lan.is_some() {
            vtime.unpause();
        }
    }
}

#[derive(Component)]
struct PauseUi;

#[derive(Component)]
struct EndUi;

#[derive(Component, Clone, Copy)]
enum Act {
    Resume,
    /// Opens the CONTROLS overlay (`controls.rs`).
    Controls,
    /// Opens the GRAPHICS overlay (`gfx.rs`).
    Graphics,
    /// Toggles `profile::OPTS[i]`.
    Opt(usize),
    Menu,
    Again,
    Quit,
}

fn on_off(on: bool) -> &'static str {
    if on { "ON" } else { "OFF" }
}

/// Spawns/despawns the pause and end-of-match overlays to follow [`Frozen`] and [`Clock`]; the
/// CONTROLS overlay stands in for the pause menu while it is open.
#[allow(clippy::too_many_arguments)]
fn panels(
    mut commands: Commands,
    frozen: Option<Res<Frozen>>,
    hold: Option<Res<Hold>>,
    controls: Option<Res<crate::controls::Overlay>>,
    graphics: Option<Res<crate::gfx::Overlay>>,
    clock: Res<Clock>,
    settings: Res<Settings>,
    camera: Query<Entity, With<Camera3d>>,
    pause: Query<Entity, With<PauseUi>>,
    end: Query<Entity, With<EndUi>>,
    opts: Query<(Entity, &Act, &Children, Has<Chosen>)>,
    mut texts: Query<&mut Text>,
) {
    let Ok(camera) = camera.single() else {
        return;
    };
    let want_pause = frozen.is_some()
        && clock.over.is_none()
        && hold.is_none()
        && controls.is_none()
        && graphics.is_none();
    match (want_pause, pause.single()) {
        (false, Ok(e)) => commands.entity(e).despawn(),
        (true, Err(_)) => spawn_pause(&mut commands, camera, &settings),
        _ => {}
    }
    match (&clock.over, end.single()) {
        (None, Ok(e)) => commands.entity(e).despawn(),
        (Some(headline), Err(_)) => spawn_end(&mut commands, camera, headline),
        _ => {}
    }
    if settings.is_changed() {
        let mut s = settings.clone();
        for (e, a, kids, chosen) in &opts {
            if let (Act::Opt(i), Some(&k)) = (a, kids.first()) {
                let on = *opt_mut(&mut s, *i);
                if let Ok(mut t) = texts.get_mut(k) {
                    t.0 = on_off(on).into();
                }
                match (on, chosen) {
                    (true, false) => drop(commands.entity(e).insert(Chosen)),
                    (false, true) => drop(commands.entity(e).remove::<Chosen>()),
                    _ => {}
                }
            }
        }
    }
}

fn root(camera: Entity) -> impl Bundle {
    (
        UiTargetCamera(camera),
        GlobalZIndex(10),
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            row_gap: px(10),
            ..default()
        },
    )
}

fn spawn_pause(commands: &mut Commands, camera: Entity, settings: &Settings) {
    commands
        .spawn((
            PauseUi,
            root(camera),
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        ))
        .with_children(|r| {
            r.spawn(panel(470.0, AlignItems::Center))
                .with_children(|p| {
                    p.spawn(heading("PAUSED"));
                    p.spawn(primary(438.0, 48.0, "RESUME", 22.0, Act::Resume));
                    p.spawn(Node {
                        column_gap: px(8),
                        ..default()
                    })
                    .with_children(|b| {
                        b.spawn(button(215.0, 40.0, "CONTROLS", 17.0, Act::Controls));
                        b.spawn(button(215.0, 40.0, "GRAPHICS", 17.0, Act::Graphics));
                    });
                    p.spawn((
                        Text::new("OPTIONS"),
                        TextFont::from_font_size(13.0),
                        TextColor(DIM),
                        Node {
                            margin: UiRect::top(px(6)),
                            align_self: AlignSelf::FlexStart,
                            ..default()
                        },
                    ));
                    p.spawn(Node {
                        flex_wrap: FlexWrap::Wrap,
                        width: px(438),
                        column_gap: px(8),
                        row_gap: px(6),
                        ..default()
                    })
                    .with_children(|g| {
                        let mut s = settings.clone();
                        for (i, (_, label)) in OPTS.iter().enumerate() {
                            let on = *opt_mut(&mut s, i);
                            g.spawn(Node {
                                width: px(215),
                                align_items: AlignItems::Center,
                                justify_content: JustifyContent::SpaceBetween,
                                ..default()
                            })
                            .with_children(|c| {
                                c.spawn((
                                    Text::new(*label),
                                    TextFont::from_font_size(15.0),
                                    TextColor(Color::WHITE),
                                    TextLayout {
                                        linebreak: LineBreak::NoWrap,
                                        ..default()
                                    },
                                ));
                                let mut b =
                                    c.spawn(button(52.0, 26.0, on_off(on), 13.0, Act::Opt(i)));
                                if on {
                                    b.insert(Chosen);
                                }
                            });
                        }
                    });
                    p.spawn(Node {
                        column_gap: px(8),
                        margin: UiRect::top(px(8)),
                        ..default()
                    })
                    .with_children(|b| {
                        b.spawn(button(215.0, 40.0, "MAIN MENU", 17.0, Act::Menu));
                        b.spawn(button(215.0, 40.0, "QUIT", 17.0, Act::Quit));
                    });
                });
        });
}

/// The scoreboard itself is the HUD's (shown while the match is over); this adds the headline
/// above it and the buttons below it (the board starts 220 px above the centre).
fn spawn_end(commands: &mut Commands, camera: Entity, headline: &str) {
    let color = match headline {
        "VICTORY" => ACCENT,
        "DEFEAT" => Color::srgb(0.95, 0.3, 0.25),
        _ => Color::WHITE,
    };
    commands.spawn((EndUi, root(camera))).with_children(|r| {
        r.spawn((
            Text::new(headline),
            TextFont::from_font_size(60.0),
            TextColor(color),
            TextShadow::default(),
            Node {
                height: px(70),
                ..default()
            },
        ));
        r.spawn(Node {
            height: px(440),
            ..default()
        });
        r.spawn(Node {
            column_gap: px(10),
            ..default()
        })
        .with_children(|b| {
            b.spawn(primary(190.0, 46.0, "PLAY AGAIN", 18.0, Act::Again));
            b.spawn(button(170.0, 46.0, "MAIN MENU", 17.0, Act::Menu));
            b.spawn(button(110.0, 46.0, "QUIT", 17.0, Act::Quit));
        });
    });
}

fn buttons(
    clicks: Query<(&Interaction, &Act), Changed<Interaction>>,
    mut settings: ResMut<Settings>,
    mut profile: Option<ResMut<Profile>>,
    mut commands: Commands,
    mut vtime: ResMut<Time<Virtual>>,
    mut exit: MessageWriter<AppExit>,
) {
    for (i, a) in &clicks {
        if *i != Interaction::Pressed {
            continue;
        }
        match *a {
            Act::Resume => freeze(&mut commands, &mut vtime, false),
            Act::Opt(i) => {
                let on = !*opt_mut(&mut settings, i);
                *opt_mut(&mut settings, i) = on;
                if let Some(p) = profile.as_mut() {
                    p.opts[i] = on;
                }
            }
            Act::Controls => commands.insert_resource(crate::controls::Overlay),
            Act::Graphics => commands.insert_resource(crate::gfx::Overlay),
            Act::Menu => drop(exit.write(AppExit::from_code(EXIT_MENU))),
            Act::Again => drop(exit.write(AppExit::from_code(EXIT_AGAIN))),
            Act::Quit => drop(exit.write(AppExit::Success)),
        }
    }
}

/// The mouse is grabbed while playing and free in the pause and end screens.
fn cursor(
    frozen: Option<Res<Frozen>>,
    shot: Option<Res<Shot>>,
    mut windows: Query<&mut CursorOptions, With<PrimaryWindow>>,
) {
    if shot.is_some() {
        return;
    }
    let (mode, visible) = if frozen.is_some() {
        (CursorGrabMode::None, true)
    } else {
        (CursorGrabMode::Locked, false)
    };
    for mut c in &mut windows {
        if c.grab_mode != mode {
            c.grab_mode = mode;
            c.visible = visible;
        }
    }
}
