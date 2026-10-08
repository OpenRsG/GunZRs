//! `gunz-play` match flow: rules (mode, time and kill/round limit, respawn and protection
//! times), the match clock and its top-line text, the end-of-match scoreboard, the Esc pause
//! menu and the mouse grab. Round modes, spectating, spawns and training are `modes.rs`. The
//! binary maps the exit codes below to "back to the main menu" / "play again" (re-executing
//! itself). Pausing pauses `Time<Virtual>` and inserts [`Frozen`]; actors ignore input while
//! frozen.

use crate::{
    game::{Frozen, Hold, Player, Score, Settings, Team, Vitals},
    menu::{Art, Mode, button, heading, hover, panel},
    modes::{Berserker, ModesPlugin, Phase, Round},
    view::Shot,
};
use bevy::{
    prelude::*,
    text::Justify,
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
    /// The HUD's top line: score, round and time for the current mode.
    pub header: String,
    /// Extra HUD text of the mode (the quest's sector and NPC count), written by its plugin.
    pub note: String,
}

/// Headless runs: open the pause menu when the match clock reaches this many seconds.
#[derive(Resource)]
pub struct PauseAt(pub f32);

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
                    hover.run_if(resource_exists::<Art>),
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
    if rules.mode == Mode::Spy {
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

/// The HUD's top line.
fn header(
    rules: &Rules,
    round: &Round,
    clock: &Clock,
    (mine, theirs): (u32, u32),
    names: &Query<&Name>,
    boss: Option<&str>,
) -> String {
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
        Mode::Team | Mode::TeamGladiator => format!("RED {mine} : {theirs} BLUE   {total}"),
        Mode::Elimination | Mode::Assassinate => {
            format!(
                "RED {mine} : {theirs} BLUE   ROUND {}   {round_t}",
                round.n.max(1)
            )
        }
        Mode::Duel | Mode::DuelTournament => {
            let name = |e| names.get(e).map_or("?", |n| n.as_str());
            match round.duelists() {
                Some((a, b)) if in_round && rules.mode == Mode::DuelTournament => {
                    format!("{}   {} vs {}   {round_t}", round.stage(), name(a), name(b))
                }
                Some((a, b)) if in_round => {
                    format!("ROUND {}   {} vs {}   {round_t}", round.n, name(a), name(b))
                }
                _ => total,
            }
        }
        Mode::Spy => format!(
            "YOU {mine} : {theirs} THEM   ROUND {}   {round_t}",
            round.n.max(1)
        ),
        Mode::ClanWar => format!("{mine} : {theirs}   ROUND {}   {round_t}", round.n.max(1)),
        Mode::Berserker => match rules.kill_limit {
            Some(k) => format!("BERSERKER {}   {total}   first to {k}", boss.unwrap_or("-")),
            None => format!("BERSERKER {}   {total}", boss.unwrap_or("-")),
        },
        Mode::Deathmatch | Mode::Gladiator | Mode::Gunman => match rules.kill_limit {
            Some(k) => format!("{total}   first to {k}"),
            None => total,
        },
        Mode::Quest | Mode::Blitzkrieg => format!("{}   {total}", clock.note),
        Mode::Training => total,
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
    let text = header(&rules, &round, &clock, (mine, theirs), &names, boss);
    if clock.header != text {
        clock.header = text;
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
    mut commands: Commands,
    mut vtime: ResMut<Time<Virtual>>,
) {
    if clock.over.is_some() || hold.is_some() {
        return;
    }
    let scripted = pause_at.is_some_and(|p| clock.elapsed >= p.0);
    if scripted {
        commands.remove_resource::<PauseAt>();
    }
    if scripted || keys.just_pressed(KeyCode::Escape) {
        freeze(&mut commands, &mut vtime, frozen.is_none());
    }
}

#[derive(Component)]
struct PauseUi;

#[derive(Component)]
struct EndUi;

#[derive(Component)]
struct SensText;

#[derive(Component, Clone, Copy)]
enum Act {
    Resume,
    Sens(i32),
    Menu,
    Again,
    Quit,
}

/// Sensitivity steps: quarters of the default.
fn sens_text(s: &Settings) -> String {
    format!("x{:.2}", s.sensitivity / Settings::default().sensitivity)
}

/// Spawns/despawns the pause and end-of-match overlays to follow [`Frozen`] and [`Clock`].
fn panels(
    mut commands: Commands,
    frozen: Option<Res<Frozen>>,
    hold: Option<Res<Hold>>,
    clock: Res<Clock>,
    art: Option<Res<Art>>,
    settings: Res<Settings>,
    camera: Query<Entity, With<Camera3d>>,
    pause: Query<Entity, With<PauseUi>>,
    end: Query<Entity, With<EndUi>>,
    mut sens: Query<&mut Text, With<SensText>>,
) {
    let (Some(art), Ok(camera)) = (art, camera.single()) else {
        return;
    };
    let want_pause = frozen.is_some() && clock.over.is_none() && hold.is_none();
    match (want_pause, pause.single()) {
        (false, Ok(e)) => commands.entity(e).despawn(),
        (true, Err(_)) => spawn_pause(&mut commands, &art, camera, &settings),
        _ => {}
    }
    match (&clock.over, end.single()) {
        (None, Ok(e)) => commands.entity(e).despawn(),
        (Some(headline), Err(_)) => spawn_end(&mut commands, &art, camera, headline),
        _ => {}
    }
    if settings.is_changed() {
        for mut t in &mut sens {
            t.0 = sens_text(&settings);
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

fn spawn_pause(commands: &mut Commands, art: &Art, camera: Entity, settings: &Settings) {
    commands
        .spawn((
            PauseUi,
            root(camera),
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.55)),
        ))
        .with_children(|r| {
            r.spawn(panel(380.0, AlignItems::Center))
                .with_children(|p| {
                    p.spawn(heading("PAUSED"));
                    p.spawn(button(art, 300.0, 44.0, "RESUME", 22.0, Act::Resume));
                    p.spawn(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(8),
                        ..default()
                    })
                    .with_children(|s| {
                        s.spawn((
                            Text::new("Mouse sens."),
                            TextFont::from_font_size(18.0),
                            TextColor(Color::WHITE),
                        ));
                        s.spawn(button(art, 34.0, 30.0, "<", 16.0, Act::Sens(-1)));
                        s.spawn((
                            SensText,
                            Text::new(sens_text(settings)),
                            TextFont::from_font_size(18.0),
                            TextColor(Color::WHITE),
                            TextLayout {
                                justify: Justify::Center,
                                ..default()
                            },
                            Node {
                                width: px(70),
                                height: px(24),
                                ..default()
                            },
                        ));
                        s.spawn(button(art, 34.0, 30.0, ">", 16.0, Act::Sens(1)));
                    });
                    p.spawn(button(art, 300.0, 44.0, "RETURN TO MENU", 20.0, Act::Menu));
                    p.spawn(button(art, 300.0, 44.0, "QUIT", 20.0, Act::Quit));
                });
        });
}

/// The scoreboard itself is the HUD's (shown while the match is over); this adds the headline
/// above it and the buttons below it (the board is 420 px tall and centred).
fn spawn_end(commands: &mut Commands, art: &Art, camera: Entity, headline: &str) {
    let color = match headline {
        "VICTORY" => Color::srgb(1.0, 0.85, 0.3),
        "DEFEAT" => Color::srgb(0.9, 0.25, 0.2),
        _ => Color::WHITE,
    };
    commands.spawn((EndUi, root(camera))).with_children(|r| {
        r.spawn((
            Text::new(headline),
            TextFont::from_font_size(56.0),
            TextColor(color),
            TextShadow::default(),
            Node {
                height: px(70),
                ..default()
            },
        ));
        r.spawn(Node {
            height: px(430),
            ..default()
        });
        r.spawn(Node {
            column_gap: px(12),
            ..default()
        })
        .with_children(|b| {
            b.spawn(button(art, 220.0, 44.0, "PLAY AGAIN", 20.0, Act::Again));
            b.spawn(button(art, 220.0, 44.0, "MAIN MENU", 20.0, Act::Menu));
            b.spawn(button(art, 160.0, 44.0, "QUIT", 20.0, Act::Quit));
        });
    });
}

fn buttons(
    clicks: Query<(&Interaction, &Act), Changed<Interaction>>,
    mut settings: ResMut<Settings>,
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
            Act::Sens(d) => {
                let unit = Settings::default().sensitivity;
                let quarters = (settings.sensitivity / unit * 4.0).round() + d as f32;
                settings.sensitivity = quarters.clamp(1.0, 16.0) / 4.0 * unit;
            }
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
