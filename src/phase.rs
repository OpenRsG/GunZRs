//! One window from launch to quit: the main menu, the loading screen and the match are three
//! [`Screen`]s of the same Bevy `App`.
//!
//! * Game plugins put their per-frame systems in [`MatchSystems`] (run only in [`Screen::Match`])
//!   and their start-up systems in [`MatchStart`] / [`MatchPostStart`] (run once when a match
//!   begins, and again by a quest's sector swap); the menu's systems are [`MenuSystems`].
//!   Plugins both screens use (graphics, controls, profile, animation, ...) add theirs plain.
//! * Leaving a screen despawns every entity that was not there after start-up and is not
//!   [`Persist`]; leaving a match also removes the resources it created, and the plugins' own
//!   match state is reset by [`MatchApp::init_match_resource`] resources. A `Local` of a match
//!   system holds on to its value across matches: keep match state in resources or components.
//! * START (or the command line) enters [`Screen::Loading`]: the map, collision, characters and
//!   items (and a quest plan or the LAN handshake) are read on a task pool thread behind an
//!   animated loading screen, then [`Screen::Match`] builds the level and starts the match.
//!   MAIN MENU and PLAY AGAIN of a match send [`Leave`]; in the browser the page reloads for a
//!   new match instead (`web.rs`), so there they end the app with an exit code.
//!
//! Headless `--shot` runs can drive the transitions: `GUNZ_MENU_START=SECS[,SECS..]` presses the
//! menu's START that many seconds after the menu opens (one entry per opening) and
//! `GUNZ_LEAVE=SECS:menu|again[,..]` presses MAIN MENU / PLAY AGAIN when the match clock reaches
//! SECS (one entry per match); every screen of the run sets the capture frame of the shot when
//! it is left to the next ([`Shot`]), and with `GUNZ_SEQ` the loading screen frames are saved.

use crate::{
    actor::{ActorData, PlayerSetup, Script},
    bot::{BotAhead, BotCount, BotSkill},
    col::MapCollision,
    game::Settings,
    gfx::Preset,
    level::Level,
    map,
    menu::{Config, Mode, Net, Page},
    modes::{self, DieAt},
    mrs::Vfs,
    net::{self, Client, Host, Lan, Welcome},
    npc::NpcDemo,
    profile::{MatchGain, Profile, opt_mut},
    quest::{self, Quest},
    session::{Clock, PauseAt, Rules, StartVitals, band},
    ui::{
        self, ACCENT, BACKDROP, Cut, DIM, INK, SURFACE, Segs, caption, display, hatch, mono, segs,
        shadow, shape, strong, tracking,
    },
    view::{self, Shot},
};
use bevy::{
    ecs::{
        component::ComponentId, observer::Observer, resource::IsResource, schedule::ScheduleLabel,
        system::SystemIdMarker,
    },
    input::gamepad::Gamepad,
    picking::pointer::PointerId,
    prelude::*,
    render::view::screenshot::Screenshot,
    tasks::{AsyncComputeTaskPool, Task, futures::check_ready},
    ui::{GlobalZIndex, UiTargetCamera},
    window::{CursorGrabMode, CursorOptions, Monitor, PrimaryWindow},
};
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

/// Simulated seconds before a headless script starts, so pipelines compile and actors land.
pub const LEAD: f32 = 1.5;

/// What the window shows.
#[derive(States, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Screen {
    /// The first frame: `StateTransition` runs before any `Startup` system, so the screen the
    /// app opens on is entered only after them.
    #[default]
    Boot,
    Menu,
    /// The match's data is read (off the main thread) behind a loading screen.
    Loading,
    Match,
}

/// Systems that only run while a match is on (`run_if(in_state(Screen::Match))`). Without the
/// [`PhasePlugin`] (the viewer binaries) the set has no condition and they always run.
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MatchSystems;

/// Systems of the main menu (`run_if(in_state(Screen::Menu))`).
#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MenuSystems;

/// Plays the role of `Startup` for a match: the systems that build its world from the loaded
/// [`Level`] and friends. Run when [`Screen::Match`] begins.
#[derive(ScheduleLabel, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MatchStart;

/// `PostStartup` of a match; a quest's sector swap runs it again for the new map.
#[derive(ScheduleLabel, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MatchPostStart;

/// Entities that outlive a screen (the background music's voices).
#[derive(Component)]
pub struct Persist;

/// Plugin helpers for match state.
pub trait MatchApp {
    /// [`App::init_resource`] for state of one match: it is reset to its initial value when
    /// the match ends.
    fn init_match_resource<R: Resource + FromWorld>(&mut self) -> &mut Self;
}

impl MatchApp for App {
    fn init_match_resource<R: Resource + FromWorld>(&mut self) -> &mut Self {
        self.init_resource::<R>()
            .add_systems(OnExit(Screen::Match), |world: &mut World| {
                world.remove_resource::<R>();
                world.init_resource::<R>();
            })
    }
}

/// The mounted game archives, shared by the menu and every match.
#[derive(Resource)]
pub struct Mount(pub Vfs);

/// MAIN MENU or PLAY AGAIN of the match.
#[derive(Message, Clone, Copy, Debug)]
pub enum Leave {
    Menu,
    Again,
}

/// What to play next: the menu's choices and the command line's extras.
#[derive(Resource, Clone)]
pub struct Launch {
    pub config: Config,
    /// The menu page the first menu opens on (`--menu-page`).
    pub page: Option<Page>,
    pub opts: Opts,
}

/// What `gunz-play`'s command line adds to a match; the menu leaves it alone, so a run keeps
/// its headless script, spawn point and rules over PLAY AGAIN and the menu's next START.
#[derive(Clone, Default)]
pub struct Opts {
    /// `--script` text (`Script::parse`).
    pub script: Option<String>,
    /// `--at`/`--yaw`: the player's feet in Bevy metres and facing in radians.
    pub at: Option<Vec3>,
    pub yaw: Option<f32>,
    /// `--bots-ahead`.
    pub ahead: Option<f32>,
    /// `--npc NAME,..`.
    pub npc: Option<String>,
    /// `--pause-at`, `--pause-page`.
    pub pause_at: Option<(f32, Option<Page>)>,
    pub die_at: Option<f32>,
    /// `--hp`, `--ap`.
    pub hp: Option<f32>,
    pub ap: Option<f32>,
    /// `--respawn`, `--protect`, `--round-time`, `--ready`.
    pub respawn: Option<f32>,
    pub protect: Option<f32>,
    pub round_time: Option<f32>,
    pub ready: Option<f32>,
    /// `--NAME` / `--no-NAME`: index into `profile::OPTS` and its value.
    pub toggles: Vec<(usize, bool)>,
    pub gfx: Option<Preset>,
    /// Headless: seconds from the script's start to the captured frame.
    pub secs: f32,
}

/// Headless transition drivers (`GUNZ_MENU_START`, `GUNZ_LEAVE`), present with a [`Shot`].
#[derive(Resource, Default)]
pub struct Hooks {
    /// Seconds into each menu opening at which START is pressed.
    pub start: VecDeque<f32>,
    /// The [`Shot`] frame the menu opened at.
    pub opened: u32,
    leave: VecDeque<(f32, Leave)>,
}

impl Hooks {
    fn from_env() -> Self {
        let list = |key: &str| {
            std::env::var(key)
                .map(|s| s.split(',').map(str::to_owned).collect::<Vec<_>>())
                .unwrap_or_default()
        };
        Self {
            start: list("GUNZ_MENU_START")
                .iter()
                .filter_map(|s| s.parse().ok())
                .collect(),
            leave: list("GUNZ_LEAVE")
                .iter()
                .filter_map(|s| {
                    let (secs, what) = s.split_once(':')?;
                    let go = match what {
                        "menu" => Leave::Menu,
                        "again" => Leave::Again,
                        _ => return None,
                    };
                    Some((secs.parse().ok()?, go))
                })
                .collect(),
            opened: 0,
        }
    }
}

/// Seed of a match's random choices (quest plan, bots' clothes): the clock, but 1 in headless
/// runs so they stay reproducible; `GUNZ_SEED` overrides both.
fn seed(headless: bool) -> u32 {
    std::env::var("GUNZ_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(if headless {
            1
        } else {
            crate::profile::wall().subsec_nanos().max(1)
        })
}

/// The screen the app opens on: the menu, or the loading screen of the match the command line
/// named.
pub struct PhasePlugin(pub Screen);

/// [`PhasePlugin`]'s screen, entered once the app's start-up systems ran.
#[derive(Resource)]
struct Opening(Screen);

impl Plugin for PhasePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<Screen>()
            .insert_resource(Opening(self.0))
            .add_message::<Leave>();
        for schedule in [
            First.intern(),
            PreUpdate.intern(),
            Update.intern(),
            PostUpdate.intern(),
            Last.intern(),
        ] {
            app.configure_sets(
                schedule,
                (
                    MatchSystems.run_if(in_state(Screen::Match)),
                    MenuSystems.run_if(in_state(Screen::Menu)),
                ),
            );
        }
        if app.world().contains_resource::<Shot>() {
            app.insert_resource(Hooks::from_env());
        }
        app.init_schedule(MatchStart)
            .init_schedule(MatchPostStart)
            .add_systems(PostStartup, boot)
            .add_systems(OnExit(Screen::Menu), clear)
            .add_systems(
                OnEnter(Screen::Loading),
                (begin_load, spawn_loading).chain(),
            )
            .add_systems(
                Update,
                (poll, crate::menu::fit, loading_ui)
                    .chain()
                    .run_if(in_state(Screen::Loading)),
            )
            .add_systems(
                OnExit(Screen::Loading),
                (clear, |mut commands: Commands| {
                    commands.remove_resource::<Loading>()
                }),
            )
            .add_systems(OnEnter(Screen::Match), enter_match)
            .add_systems(
                Update,
                // the cover is the loading screen: the menu's scale, not the HUD's
                (uncover, crate::menu::fit.after(crate::menu::fit_hud))
                    .run_if(in_state(Screen::Match))
                    .run_if(any_with_component::<Cover>),
            )
            .add_systems(
                Update,
                (leave_hook.run_if(resource_exists::<Hooks>), leave)
                    .chain()
                    .run_if(in_state(Screen::Match)),
            )
            .add_systems(OnExit(Screen::Match), end_match);
    }
}

/// The entities of the app's start-up (the window, pointers, ...) stay through every screen.
#[derive(Resource)]
struct Keep(HashSet<Entity>);

fn boot(world: &mut World) {
    let all = world.query::<Entity>().iter(world).collect();
    world.insert_resource(Keep(all));
    let opening = world.resource::<Opening>().0;
    world.resource_mut::<NextState<Screen>>().set(opening);
}

/// Despawns what the screen being left spawned.
#[allow(clippy::type_complexity)]
fn clear(world: &mut World) {
    let keep = world.remove_resource::<Keep>();
    let doomed: Vec<Entity> = world
        .query_filtered::<Entity, (
            Without<ChildOf>,
            Without<IsResource>,
            Without<Persist>,
            Without<Window>,
            Without<Monitor>,
            Without<Gamepad>,
            Without<Observer>,
            Without<SystemIdMarker>,
            Without<Screenshot>,
            Without<PointerId>,
        )>()
        .iter(world)
        .filter(|e| keep.as_ref().is_none_or(|k| !k.0.contains(e)))
        .collect();
    for e in doomed {
        if let Ok(e) = world.get_entity_mut(e) {
            e.despawn();
        }
    }
    if let Some(keep) = keep {
        world.insert_resource(keep);
    }
}

/// Starts the clocks over, so a match begins at time 0 whatever came before it.
fn reset_time(world: &mut World) {
    world.insert_resource(Time::<Virtual>::default());
    world.insert_resource(Time::<Real>::default());
    world.insert_resource(Time::<()>::default());
}

// ---- loading ----

/// The resources present when loading began: a match removes the others when it ends.
#[derive(Resource)]
struct Baseline(HashSet<ComponentId>);

/// The data read for a match.
struct Loaded {
    level: Level,
    col: MapCollision,
    data: ActorData,
    quest: Option<Quest>,
    /// Quest items the start spends from the profile.
    spend: Vec<u32>,
    /// The config as played: a LAN client's mode and limits are the host's, a quest's scenario
    /// is the one admitted.
    config: Config,
    link: Link,
}

enum Link {
    Offline,
    Host(Host),
    Client(Welcome, Client),
}

#[derive(Resource)]
struct Loading {
    task: Task<Result<Loaded, String>>,
    /// Steps of `steps` the task finished.
    done: Arc<AtomicUsize>,
    steps: Vec<&'static str>,
    /// Progress drawn, easing towards the real one.
    shown: f32,
    /// The data arrived: the screen says so for a frame before the level is built.
    built: bool,
}

#[derive(Resource)]
struct Ready(Loaded);

/// Admits and plans the scenario `config` names and returns the quest and the quest items its
/// start spends; `config.scenario` becomes the scenario actually played.
fn start_quest(
    vfs: &Vfs,
    config: &mut Config,
    profile: &Profile,
    headless: bool,
) -> Result<(Quest, Vec<u32>), String> {
    let cat = quest::Catalog::load(vfs).map_err(|e| format!("quest data: {e}"))?;
    let ok = cat.admit(
        config.scenario.as_deref(),
        config.sacrifice,
        &profile.quest_items,
        profile.level(),
    )?;
    if !ok.spend.is_empty() {
        let names: Vec<_> = ok.spend.iter().map(|&i| cat.items.name(i)).collect();
        println!("quest: sacrificed {}", names.join(" + "));
    }
    let seed = seed(headless);
    let plan = cat.plan(&ok.scenario, config.dice, seed)?;
    config.scenario = Some(ok.scenario);
    Ok((Quest::new(vfs, &cat, plan, config.bots, seed)?, ok.spend))
}

/// The slow part of starting a match, on a task pool thread.
fn load(
    vfs: Vfs,
    mut config: Config,
    profile: Profile,
    headless: bool,
    done: &AtomicUsize,
) -> Result<Loaded, String> {
    let step = || done.fetch_add(1, Ordering::Relaxed);
    let mut link = Link::Offline;
    let mut map_name = None;
    // A LAN client plays the host's map, mode and limits.
    if let Some(Net::Join(addr)) = config.net.clone() {
        let (w, c) = net::join(
            &addr,
            &profile.name,
            config.woman,
            &config.look,
            &config.loadout,
        )
        .map_err(|e| format!("--join: {e}"))?;
        println!("lan: joined {} ({})", w.map, w.mode.arg());
        (config.mode, config.time_limit, config.kill_limit) = (w.mode, w.time_limit, w.kill_limit);
        map_name = Some(w.map.clone());
        link = Link::Client(w, c);
        step();
    }
    // Quest mode plays a scenario's sectors in turn: its first sector is the map.
    let (mut quest, mut spend) = (None, vec![]);
    if config.mode == Mode::Quest && matches!(link, Link::Offline) {
        let (q, s) = start_quest(&vfs, &mut config, &profile, headless)?;
        map_name = Some(q.first_map().to_owned());
        (quest, spend) = (Some(q), s);
        step();
    }
    let map_name = map_name
        .or_else(|| config.map.clone())
        .ok_or("no map to play")?;
    // A quest restarts from its scenario, not from a map.
    config.map = quest.is_none().then(|| map_name.to_ascii_lowercase());
    if config.mode == Mode::Blitzkrieg && !map_name.eq_ignore_ascii_case("blitzkrieg") {
        return Err("--mode blitzkrieg is played on the map blitzkrieg".into());
    }
    let rs = map::find_rs(&vfs, &map_name).ok_or_else(|| format!("no map named {map_name}"))?;
    let col = MapCollision::load(&vfs, &rs).map_err(|e| format!("{rs} collision: {e}"))?;
    step();
    let map = map::load(&vfs, &rs).map_err(|e| format!("{rs}: {e}"))?;
    step();
    let level = Level { vfs, map };
    let mut data = ActorData::new(&level).map_err(|e| format!("items/characters: {e}"))?;
    data.seed = seed(headless) as u64;
    step();
    if let (Link::Offline, Some(Net::Host)) = (&link, &config.net) {
        let host = Host::bind(&map_name).map_err(|e| format!("--host: port {}: {e}", net::PORT))?;
        println!("lan: hosting {map_name} on port {}", net::PORT);
        link = Link::Host(host);
    }
    Ok(Loaded {
        level,
        col,
        data,
        quest,
        spend,
        config,
        link,
    })
}

/// OnEnter(Loading): remembers the resources, starts reading.
fn begin_load(world: &mut World) {
    let base = world
        .query::<&IsResource>()
        .iter(world)
        .map(IsResource::resource_component_id)
        .collect();
    world.insert_resource(Baseline(base));
    let launch = world.resource::<Launch>().clone();
    let vfs = world.resource::<Mount>().0.clone();
    let profile = world.resource::<Profile>().clone();
    let headless = world.contains_resource::<Shot>();
    if let Some(mut shot) = world.get_resource_mut::<Shot>() {
        shot.pending();
    }
    let config = &launch.config;
    let join = matches!(config.net, Some(Net::Join(_)));
    let mut steps = vec![];
    if join {
        steps.push("Joining the host");
    } else if config.mode == Mode::Quest {
        steps.push("Reading the quest data");
    }
    steps.extend([
        "Reading collision",
        "Reading the map",
        "Loading characters and items",
    ]);
    let done = Arc::new(AtomicUsize::new(0));
    let task = {
        let (config, done) = (config.clone(), done.clone());
        AsyncComputeTaskPool::get()
            .spawn(async move { load(vfs, config, profile, headless, &done) })
    };
    world.insert_resource(Loading {
        task,
        done,
        steps,
        shown: 0.0,
        built: false,
    });
}

/// Waits for the task, then lets one frame show "building" before the level is built.
fn poll(
    mut loading: ResMut<Loading>,
    mut commands: Commands,
    mut next: ResMut<NextState<Screen>>,
    mut launch: ResMut<Launch>,
    shot: Option<Res<Shot>>,
    mut exit: MessageWriter<AppExit>,
) {
    if loading.built {
        next.set(Screen::Match);
        return;
    }
    let Some(result) = check_ready(&mut loading.task) else {
        return;
    };
    match result {
        Ok(loaded) => {
            commands.insert_resource(Ready(loaded));
            loading.built = true;
        }
        Err(e) => {
            eprintln!("{e}");
            // headless runs and the browser end; the desktop goes back to the menu
            if shot.is_some() || cfg!(target_arch = "wasm32") {
                exit.write(AppExit::from_code(2));
            } else {
                launch.config.net = None;
                next.set(Screen::Menu);
            }
        }
    }
}

#[derive(Component)]
struct LoadBar;

#[derive(Component)]
struct StepText;

/// Segments of the loading bar.
const LOAD_SEGS: usize = 32;

/// OnEnter(Loading): the loading screen.
fn spawn_loading(
    mut commands: Commands,
    mut images: ResMut<Assets<Image>>,
    shot: Option<Res<Shot>>,
    launch: Res<Launch>,
    profile: Res<Profile>,
) {
    commands.insert_resource(ClearColor(BACKDROP));
    let camera = view::spawn_camera(
        &mut commands,
        &mut images,
        shot.is_some(),
        Vec3::new(0.0, 1.05, 3.6),
        Vec3::NEG_Z,
    )
    .id();
    let art = map_art(&profile, &launch.config).map(|i| images.add(i));
    loading_screen(&mut commands, camera, &launch.config, true, art);
}

/// The picture of the map the loading screen is for ([`Profile::preview`]); a quest or a LAN
/// join has no map of its own.
fn map_art(profile: &Profile, c: &Config) -> Option<Image> {
    match (&c.net, &c.scenario, &c.map) {
        (Some(Net::Join(_)), ..) => None,
        (_, Some(_), _) if c.mode == Mode::Quest => None,
        (_, _, Some(map)) => profile.preview(map),
        _ => None,
    }
}

/// The loading screen, in the menu's look (a 1920 x 1080 canvas): the map's name huge in display,
/// the mode, a segment bar and the step being read in mono, and the map's picture in a frame
/// (`art`; a hatch when it was not made). `live` lets [`loading_ui`] drive the bar; without it
/// the bar is full: that is the [`Cover`].
fn loading_screen<'a>(
    commands: &'a mut Commands,
    camera: Entity,
    c: &Config,
    live: bool,
    art: Option<Handle<Image>>,
) -> EntityCommands<'a> {
    let (title, line) = match (&c.net, &c.scenario, &c.map) {
        (Some(Net::Join(_)), ..) => ("LAN match".to_owned(), "Joining the host".to_owned()),
        (_, Some(scenario), _) if c.mode == Mode::Quest => (scenario.clone(), "Quest".to_owned()),
        (_, _, Some(map)) => (
            map.clone(),
            match c.net {
                Some(Net::Host) => format!("{} - hosting a LAN match", c.mode.name()),
                _ => c.mode.name().to_owned(),
            },
        ),
        _ => ("Quest".to_owned(), c.mode.name().to_owned()),
    };
    let mut root = commands.spawn((
        UiTargetCamera(camera),
        Node {
            width: percent(100),
            height: percent(100),
            align_items: AlignItems::Center,
            column_gap: px(100),
            padding: UiRect::horizontal(px(140)),
            ..default()
        },
    ));
    root.with_children(|root| {
        root.spawn(band());
        root.spawn(Node {
            flex_grow: 1.0,
            flex_basis: px(0),
            flex_direction: FlexDirection::Column,
            row_gap: px(12),
            ..default()
        })
        .with_children(|left| {
            left.spawn(caption("Loading", 20.0));
            left.spawn((
                Text::new(title.to_uppercase()),
                display(180.0),
                TextColor(INK),
                tracking(0.01, 180.0),
                shadow(),
            ));
            left.spawn((
                Text::new(line.to_uppercase()),
                strong(30.0),
                TextColor(ACCENT),
                tracking(0.14, 30.0),
            ));
            left.spawn(Node {
                width: percent(100),
                margin: UiRect::top(px(48)),
                ..default()
            })
            .with_children(|w| {
                let mut bar = w.spawn(segs(LOAD_SEGS, if live { 0 } else { LOAD_SEGS }, 14.0, 5.0));
                if live {
                    bar.insert(LoadBar);
                }
            });
            let mut step = left.spawn((
                Text::new("STARTING"),
                mono(20.0),
                TextColor(DIM),
                tracking(0.14, 20.0),
            ));
            if live {
                step.insert(StepText);
            }
        });
        root.spawn((
            Node {
                width: px(620),
                height: px(420),
                padding: UiRect::all(px(12)),
                ..default()
            },
            shape(Cut::Chamfer(18), SURFACE),
        ))
        .with_children(|frame| {
            let inner = Node {
                width: percent(100),
                height: percent(100),
                ..default()
            };
            match art {
                Some(pic) => frame.spawn((inner, ui::art(pic, 596.0, 396.0, Color::WHITE))),
                None => frame.spawn((inner, hatch(Color::srgba(1.0, 1.0, 1.0, 0.005)))),
            };
        });
    });
    root
}

/// The loading screen kept over the first frames of a match: the camera shows the level before
/// its pipelines compile, which would be a black window.
#[derive(Component)]
struct Cover(u32);

/// Frames the [`Cover`] stays.
const COVER_FRAMES: u32 = 45;

fn uncover(mut commands: Commands, mut cover: Query<(Entity, &mut Cover)>) {
    for (e, mut c) in &mut cover {
        c.0 += 1;
        if c.0 >= COVER_FRAMES {
            commands.entity(e).despawn();
        }
    }
}

/// Lights the bar towards the finished steps and writes the step being read.
fn loading_ui(
    time: Res<Time>,
    mut loading: ResMut<Loading>,
    mut bar: Single<&mut Segs, With<LoadBar>>,
    mut text: Single<&mut Text, With<StepText>>,
) {
    let (done, total) = (loading.done.load(Ordering::Relaxed), loading.steps.len());
    // the level's build is the last step
    let target = if loading.built {
        1.0
    } else {
        (done as f32 + 0.5) / (total + 1) as f32
    };
    let dt = time.delta_secs();
    loading.shown += (target - loading.shown) * (1.0 - (-5.0 * dt).exp());
    let lit = (loading.shown * LOAD_SEGS as f32).round() as usize;
    if bar.lit != lit {
        bar.lit = lit;
    }
    // trailing dots, so the screen never looks frozen
    let dots = ".".repeat((time.elapsed_secs() * 2.5) as usize % 4);
    let label = if loading.built {
        format!("BUILDING THE LEVEL   {0}/{0}{dots}", total + 1)
    } else {
        let at = done.min(total - 1);
        format!(
            "{}   {}/{}{dots}",
            loading.steps[at].to_uppercase(),
            at + 1,
            total + 1
        )
    };
    if text.0 != label {
        text.0 = label;
    }
}

// ---- the match ----

/// OnEnter(Match): the loaded data and the run's options become the match's resources, then the
/// level is built.
fn enter_match(world: &mut World) {
    let Some(Ready(loaded)) = world.remove_resource::<Ready>() else {
        return;
    };
    let mut launch = world.resource::<Launch>().clone();
    let opts = launch.opts.clone();
    let headless = world.contains_resource::<Shot>();
    let Loaded {
        level,
        col,
        data,
        quest,
        spend,
        config,
        link,
    } = loaded;
    if !spend.is_empty() {
        let mut profile = world.resource_mut::<Profile>();
        profile.spend_quest_items(&spend);
        profile.save();
    }
    let name = world.resource::<Profile>().name.clone();
    let mut settings = Settings::default();
    world.resource::<Profile>().apply(&mut settings);
    for &(i, on) in &opts.toggles {
        *opt_mut(&mut settings, i) = on;
    }
    if let Some(p) = opts.gfx {
        world.resource_mut::<Profile>().graphics.preset(p);
    }
    let mut rules = Rules::new(config.mode, config.time_limit, config.kill_limit);
    rules.respawn = opts.respawn.unwrap_or(rules.respawn);
    rules.protect = opts.protect.unwrap_or(rules.protect);
    rules.round_secs = opts.round_time.unwrap_or(rules.round_secs);
    rules.ready = opts.ready.unwrap_or(rules.ready);
    // The training range has no bots, only dummies; a clan war is 4 against 4 with the player.
    let mut bots = match config.mode {
        Mode::Training => 0,
        Mode::ClanWar => 2 * crate::clan::WAR_SIZE - 1,
        _ => config.bots,
    };
    let mut start = (opts.at, opts.yaw);
    match link {
        // The host's bots and rules; its spawn point unless `--at`/`--yaw` say otherwise.
        Link::Client(w, c) => {
            (rules.respawn, rules.protect, bots) = (w.respawn, w.protect, 0);
            start = (start.0.or(Some(w.at)), start.1.or(Some(w.yaw)));
            world.insert_resource(c);
            world.insert_resource(Lan);
        }
        Link::Host(h) => {
            world.insert_resource(h);
            world.insert_resource(Lan);
        }
        Link::Offline => {}
    }
    if let Some(text) = &opts.script {
        match Script::parse(text) {
            Ok(mut script) => {
                if headless {
                    script.lead = LEAD;
                }
                world.insert_resource(script);
            }
            Err(e) => eprintln!("--script: {e}"),
        }
    }
    if let Some(a) = opts.ahead {
        world.insert_resource(BotAhead(a));
    }
    if let Some(names) = &opts.npc {
        world.insert_resource(NpcDemo {
            names: names.split(',').map(str::to_string).collect(),
            ahead: opts.ahead.unwrap_or(8.0),
        });
    }
    if let Some((s, page)) = opts.pause_at {
        world.insert_resource(PauseAt(s, page));
    }
    if let Some(s) = opts.die_at {
        world.insert_resource(DieAt(s));
    }
    if opts.hp.is_some() || opts.ap.is_some() {
        world.insert_resource(StartVitals {
            hp: opts.hp,
            ap: opts.ap,
        });
    }
    if let Some(q) = quest {
        world.insert_resource(q);
    }
    if let Some(a) = modes::arsenal(&rules, &data) {
        world.insert_resource(a);
    }
    let from_level = world.resource::<Profile>().level();
    world.insert_resource(MatchGain {
        from_level,
        ..default()
    });
    world.insert_resource(PlayerSetup {
        at: start.0,
        yaw: start.1,
        woman: config.woman,
        loadout: config.loadout.clone(),
        look: config.look,
        name: if config.net.is_some() {
            name
        } else {
            String::new()
        },
    });
    world.insert_resource(settings);
    world.insert_resource(rules);
    world.insert_resource(BotCount(bots));
    world.insert_resource(BotSkill(config.skill));
    world.insert_resource(col);
    world.insert_resource(data);
    world.insert_resource(level);
    launch.config = config;
    world.insert_resource(launch);
    world.insert_resource(ClearColor(Color::BLACK));
    reset_time(world);
    if let Some(mut shot) = world.get_resource_mut::<Shot>() {
        shot.capture_in(((LEAD + opts.secs) * 60.0).round() as u32);
    }
    world.run_schedule(MatchStart);
    world.run_schedule(MatchPostStart);
    // the loading screen stays over the first frames, while the level's pipelines compile
    let config = world.resource::<Launch>().config.clone();
    if let Ok(camera) = world
        .query_filtered::<Entity, With<Camera3d>>()
        .single(world)
    {
        let art = map_art(world.resource::<Profile>(), &config)
            .map(|i| world.resource_mut::<Assets<Image>>().add(i));
        let mut commands = world.commands();
        loading_screen(&mut commands, camera, &config, false, art).insert((
            Cover(0),
            BackgroundColor(BACKDROP),
            GlobalZIndex(1000),
        ));
        world.flush();
    }
}

/// OnExit(Match): drops everything the match made. Only the game's own resources go: Bevy
/// creates some of its own lazily (`AssetChanges<T>`, ...) while the first match runs.
fn end_match(world: &mut World) {
    clear(world);
    if let Some(Baseline(base)) = world.remove_resource::<Baseline>() {
        let made: Vec<ComponentId> = world
            .query::<&IsResource>()
            .iter(world)
            .map(IsResource::resource_component_id)
            .filter(|id| !base.contains(id))
            .filter(|&id| {
                let info = world.components().get_info(id);
                info.is_some_and(|i| i.name().to_string().starts_with("gunz::"))
            })
            .collect();
        for id in made {
            world.remove_resource_by_id(id);
        }
    }
    reset_time(world);
    // the pause and end screens free the mouse, a match grabs it
    for mut cursor in world
        .query_filtered::<&mut CursorOptions, With<PrimaryWindow>>()
        .iter_mut(world)
    {
        cursor.grab_mode = CursorGrabMode::None;
        cursor.visible = true;
    }
}

/// Headless: presses MAIN MENU / PLAY AGAIN when the match clock says so.
fn leave_hook(clock: Res<Clock>, mut hooks: ResMut<Hooks>, mut out: MessageWriter<Leave>) {
    if hooks.leave.front().is_some_and(|h| clock.elapsed >= h.0)
        && let Some((_, go)) = hooks.leave.pop_front()
    {
        out.write(go);
    }
}

fn leave(
    mut leaving: MessageReader<Leave>,
    mut launch: ResMut<Launch>,
    mut next: ResMut<NextState<Screen>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Some(&go) = leaving.read().last() else {
        return;
    };
    if cfg!(target_arch = "wasm32") {
        // the page starts the menu or the match again (`gunzExit`)
        exit.write(AppExit::from_code(match go {
            Leave::Menu => 3,
            Leave::Again => 4,
        }));
        return;
    }
    match go {
        // back in the menu, hosting or joining is chosen again
        Leave::Menu => {
            launch.config.net = None;
            launch.page = None;
            next.set(Screen::Menu);
        }
        Leave::Again => next.set(Screen::Loading),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Resource)]
    struct Settled;

    #[derive(Resource)]
    struct Made;

    /// Leaving a match despawns what it spawned (children with it), keeps the start-up entities
    /// and [`Persist`] ones, and drops the resources created since loading began.
    #[test]
    fn ending_a_match_cleans_up() {
        let mut world = World::new();
        let window = world.spawn_empty().id();
        world.insert_resource(Settled);
        world.insert_resource(Keep([window].into()));
        let base = world
            .query::<&IsResource>()
            .iter(&world)
            .map(IsResource::resource_component_id)
            .collect();
        world.insert_resource(Baseline(base));
        let actor = world.spawn_empty().id();
        let limb = world.spawn(ChildOf(actor)).id();
        let voice = world.spawn(Persist).id();
        world.insert_resource(Made);
        end_match(&mut world);
        assert!(world.get_entity(window).is_ok() && world.get_entity(voice).is_ok());
        assert!(world.get_entity(actor).is_err() && world.get_entity(limb).is_err());
        assert!(world.contains_resource::<Settled>() && !world.contains_resource::<Made>());
        assert!(world.contains_resource::<Keep>());
    }
}
