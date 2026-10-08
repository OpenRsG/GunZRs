//! `gunz-play GAME_DIR [MAP] [OPTIONS]`: play a retail map as a GunZ character. Without MAP the
//! main menu opens (map, mode and limits, bots, character and outfit, loadout, sensitivity);
//! Start re-executes this binary with the chosen options, and "Main menu" in the pause or
//! match-end screen re-executes it back into the menu.
//!
//! Controls: WASD move, Space jump (in the air next to a wall: wall kick), double-tap a
//! direction to tumble, mouse look, left mouse attack, right mouse guard (melee), R reload,
//! 1..5 or the wheel switch
//! weapon, Tab scoreboard, Esc pause menu (resume, mouse sensitivity, main menu, quit).
//!
//! OPTIONS (the menu writes the same ones): `--char man|woman`, `--outfit N` (1-based
//! `AddParts` set, 0 = default), `--loadout ID,ID,..` (zitem ids), `--bots N`,
//! `--bots-ahead M` (spawn them M metres in front of the player), `--skill 0..1` (bot
//! difficulty), `--sens X` (mouse sensitivity, 1 = default), `--mode MODE`, `--time-limit
//! SECONDS` and `--kill-limit N` (0 = none; the defaults are `gametypecfg.xml`'s, none in
//! headless runs), `--map NAME` (with no MAP: preselect it in the menu).
//! MODE is `dm` (deathmatch), `tdm` (team deathmatch), `gladiator` / `team-gladiator` (melee
//! weapons only), `elimination` (team rounds, no respawn until the round ends), `assassinate`
//! (rounds, one VIP per team), `duel` (one-on-one rounds, the winner stays, the rest queue and
//! watch) or `training` (no bots, dummy targets). In the round modes the kill limit counts
//! rounds won and the time limit is for the match (duel: for one round).
//! Rules (not in the menu): `--respawn S` (seconds dead before the respawn, default 5),
//! `--protect S` (spawn protection, default 3), `--round-time S` (round limit, default 180),
//! `--ready S` (countdown before a round, default 3).
//!
//! Headless, reproducible runs (never opens a window):
//! `gunz-play GAME_DIR [MAP] --shot OUT.png [--script SCRIPT] [--time S] [--at X,Y,Z]
//! [--yaw DEG] [--hp N] [--ap N] [--pause-at S] [--die-at S]`. The simulation advances in fixed
//! 1/60 s steps: 1.5 s of settling (the actor lands, pipelines compile), then the script
//! starts; the frame `--time` seconds after the script start (default: script length + 0.3, or
//! 0.5) is saved. `--at` is the player's feet in map coordinates (cm), `--yaw` the facing in
//! degrees (0 = -Z, positive turns left), `--hp`/`--ap` the starting health/armour,
//! `--pause-at` opens the pause menu when the match clock reaches S seconds, `--die-at` kills
//! the player then (the match clock starts with the first frame, 1.5 s before the script).
//! Without MAP, `--shot` saves the main menu instead: `--menu-page match|player` picks the
//! screen and the options above set what it shows.
//!
//! SCRIPT is `;`-separated steps run one after another: `KEYS:SECONDS` holds the `+`-joined
//! keys for that long (`w a s d` move, `jump`, `attack`, `guard`, `reload`, `tab` scoreboard,
//! `1`..`9`
//! select weapon, `wait` nothing), `yaw=DEG` / `pitch=DEG` set the aim instantly (pitch
//! positive looks up).
//! Example: `"w:1.0;jump:0.3;2:0.1;attack:0.5;w:0.05;wait:0.05;w:0.4"` runs, jumps, draws the
//! revolver, fires, then double-taps W (tumble forward).

use bevy::{prelude::*, time::TimeUpdateStrategy};
use gunz::{
    actor::{ActorData, PlayerSetup, Script},
    bot::{BotAhead, BotCount, BotSkill},
    col::MapCollision,
    effect::{self, FxPlugin},
    game::{GamePlugin, Settings},
    level::{Level, LevelPlugin},
    map,
    menu::{self, Config, Mode, Page, take_arg},
    modes::DieAt,
    mrs::Vfs,
    session::{EXIT_AGAIN, EXIT_MENU, PauseAt, Rules, StartVitals},
    view::{self, SCALE, Shot, to_bevy},
};
use std::{os::unix::process::CommandExt, process::Command, time::Duration};

/// Simulated seconds before a headless script starts, so pipelines compile and actors land.
const LEAD: f32 = 1.5;

/// Replaces this process with a fresh `gunz-play` (a new window, so no state carries over):
/// the match described by `config` (`menu == false`) or the main menu with its choices.
fn relaunch(game: &str, config: &Config, menu: bool) -> AppExit {
    let exe = std::env::current_exe().expect("own executable path");
    let mut cmd = Command::new(exe);
    cmd.arg(game);
    match (&config.map, menu) {
        (Some(map), true) => cmd.args(["--map", map]),
        (Some(map), false) => cmd.arg(map),
        (None, _) => &mut cmd,
    };
    let err = cmd.args(config.flags()).exec();
    eprintln!("cannot restart gunz-play: {err}");
    AppExit::from_code(1)
}

fn main() -> AppExit {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz-play GAME_DIR [MAP] [--char man|woman] [--outfit N] [--loadout ID,..] [--bots N]\n       \
             [--bots-ahead M] [--skill 0..1] [--sens X] [--mode dm|tdm|gladiator|team-gladiator|elimination|assassinate|duel|training]\n       \
             [--time-limit S] [--kill-limit N] [--respawn S] [--protect S] [--round-time S] [--ready S]\n       \
             gunz-play GAME_DIR [MAP] --shot OUT.png [--script SCRIPT] [--time S] [--at X,Y,Z] [--yaw DEG]\n       \
             [--hp N] [--ap N] [--pause-at S] [--die-at S] [--menu-page match|player]\n\
             (see the doc comment of src/bin/gunz-play.rs)"
        );
        AppExit::from_code(2)
    };
    let (Ok(shot), Ok(time), Ok(at), Ok(yaw), Ok(script), Ok(ahead)) = (
        view::take_shot_arg(&mut args),
        effect::take_f32_arg(&mut args, "--time"),
        effect::take_vec3_arg(&mut args, "--at"),
        effect::take_f32_arg(&mut args, "--yaw"),
        take_arg::<String>(&mut args, "--script"),
        take_arg::<f32>(&mut args, "--bots-ahead"),
    ) else {
        return usage();
    };
    let (Ok(hp), Ok(ap), Ok(pause_at), Ok(page), Ok(die_at)) = (
        take_arg::<f32>(&mut args, "--hp"),
        take_arg::<f32>(&mut args, "--ap"),
        take_arg::<f32>(&mut args, "--pause-at"),
        take_arg::<Page>(&mut args, "--menu-page"),
        take_arg::<f32>(&mut args, "--die-at"),
    ) else {
        return usage();
    };
    let (Ok(respawn), Ok(protect), Ok(round_time), Ok(ready)) = (
        take_arg::<f32>(&mut args, "--respawn"),
        take_arg::<f32>(&mut args, "--protect"),
        take_arg::<f32>(&mut args, "--round-time"),
        take_arg::<f32>(&mut args, "--ready"),
    ) else {
        return usage();
    };
    let mut config = match Config::parse(&mut args, shot.is_some()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return usage();
        }
    };
    let script = match script.as_deref().map(Script::parse).transpose() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("--script: {e}");
            return usage();
        }
    };
    let Some(game) = args.first().cloned() else {
        return usage();
    };
    let vfs = Vfs::mount(&game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let Some(map_name) = args.get(1).cloned() else {
        // No MAP: the main menu; Start leaves the choice behind, then the game starts.
        return match menu::run(vfs, config, page.unwrap_or(Page::Match), shot) {
            Some(chosen) => relaunch(&game, &chosen, false),
            None => AppExit::Success,
        };
    };
    config.map = Some(map_name.to_ascii_lowercase());
    let rs = map::find_rs(&vfs, &map_name).unwrap_or_else(|| panic!("no map named {map_name}"));
    let col = MapCollision::load(&vfs, &rs).unwrap_or_else(|e| panic!("{rs} collision: {e}"));
    let map = map::load(&vfs, &rs).unwrap_or_else(|e| panic!("{rs}: {e}"));
    let level = Level { vfs, map };
    let data = ActorData::new(&level).unwrap_or_else(|e| panic!("items/characters: {e}"));

    let secs = time.unwrap_or_else(|| script.as_ref().map_or(0.5, |s| s.secs() + 0.3));
    let mut app = view::app_plain(&format!("gunz-play {map_name}"), shot);
    let headless = app.world().contains_resource::<Shot>();
    if headless {
        app.world_mut().resource_mut::<Shot>().capture = ((LEAD + secs) * 60.0).round() as u32;
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )));
    }
    if let Some(mut script) = script {
        if headless {
            script.lead = LEAD;
        }
        app.insert_resource(script);
    }
    if let Some(ahead) = ahead {
        app.insert_resource(BotAhead(ahead));
    }
    if let Some(s) = pause_at {
        app.insert_resource(PauseAt(s));
    }
    if let Some(s) = die_at {
        app.insert_resource(DieAt(s));
    }
    let mut rules = Rules::new(config.mode, config.time_limit, config.kill_limit);
    rules.respawn = respawn.unwrap_or(rules.respawn);
    rules.protect = protect.unwrap_or(rules.protect);
    rules.round_secs = round_time.unwrap_or(rules.round_secs);
    rules.ready = ready.unwrap_or(rules.ready);
    // The training range has no bots, only dummies.
    let bots = if config.mode == Mode::Training {
        0
    } else {
        config.bots
    };
    if hp.is_some() || ap.is_some() {
        app.insert_resource(StartVitals { hp, ap });
    }
    let sensitivity = Settings::default().sensitivity * config.sens;
    let exit = app
        .add_plugins((LevelPlugin, FxPlugin(None), GamePlugin))
        .insert_resource(PlayerSetup {
            at: at.map(|p| Vec3::from(to_bevy(p)) * SCALE),
            yaw: yaw.map(f32::to_radians),
            woman: config.woman,
            loadout: config.loadout.clone(),
            outfit: config.outfit,
        })
        .insert_resource(Settings { sensitivity })
        .insert_resource(rules)
        .insert_resource(BotCount(bots))
        .insert_resource(BotSkill(config.skill))
        .insert_resource(col)
        .insert_resource(data)
        .insert_resource(level)
        .run();
    match exit {
        AppExit::Error(c) if c.get() == EXIT_MENU => relaunch(&game, &config, true),
        AppExit::Error(c) if c.get() == EXIT_AGAIN => relaunch(&game, &config, false),
        other => other,
    }
}
