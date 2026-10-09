//! `gunz-play [GAME_DIR] [MAP] [OPTIONS]`: play a retail map as a GunZ character. Without
//! GAME_DIR the Steam install is found through `libraryfolders.vdf` (`src/steam.rs`). Without MAP the
//! main menu opens (map, mode and limits, bots, character and clothes, loadout, sensitivity);
//! Start re-executes this binary with the chosen options, and "Main menu" in the pause or
//! match-end screen re-executes it back into the menu.
//!
//! Controls: WASD move, Space jump (in the air next to a wall: wall kick), double-tap a
//! direction to tumble, mouse look, left mouse attack, right mouse guard (melee), R reload,
//! 1..5 or the wheel switch
//! weapon, Tab scoreboard, Esc pause menu (resume, mouse sensitivity, main menu, quit).
//!
//! OPTIONS (the menu writes the same ones): `--char man|woman`, `--look LOOK` (clothes and
//! dyes, the profile's `look` text: six 1-based parts `;` six tints), `--loadout ID,ID,..` (zitem ids), `--bots N`,
//! `--bots-ahead M` (spawn them M metres in front of the player), `--skill 0..1` (bot
//! difficulty), `--sens X` (mouse sensitivity, 1 = default), `--mode MODE`, `--time-limit
//! SECONDS` and `--kill-limit N` (0 = none; the defaults are `gametypecfg.xml`'s, none in
//! headless runs), `--map NAME` (with no MAP: preselect it in the menu).
//! TOGGLES (also in the pause menu, saved in the profile as `opt_NAME=0|1`; a flag overrides the
//! profile for this run): `--kill-sounds`/`--no-kill-sounds` (default on), `--hit-sound` (plays
//! `<profile dir>/custom/hitsound.wav`, default off), `--static-spread` (off), `--team-bars` (on),
//! `--screen-blood` (on), `--killcam` (on); each has a `--no-` form.
//! MODE is `dm` (deathmatch), `tdm` (team deathmatch), `gladiator` / `team-gladiator` (melee
//! weapons only), `elimination` (team rounds, no respawn until the round ends), `assassinate`
//! (rounds, one VIP per team), `duel` (one-on-one rounds, the winner stays, the rest queue and
//! watch), `training` (no bots, dummy targets), `berserker` (everybody hunts one berserker; kill
//! it to become it), `tournament` (knockout bracket of duels), `gunman` (a random melee weapon
//! and gun every life) or `spy` (rounds: hidden spies with grenades against trackers). In the
//! round modes the kill limit counts rounds won and the time limit is for the match (duel and
//! tournament: for one round; spy: none, the round time is the map's).
//! `gungame` is a free-for-all up a weapon ladder: every kill swaps the killer's weapon for the
//! next one, a melee kill demotes the victim, a kill from the last step wins.
//! `infected` plays rounds: after the countdown one random actor turns zombie (a blade only, faster,
//! tougher, hits knock back); a survivor a zombie kills respawns as a zombie. Survivors win the round
//! at the round timer, zombies when everyone is infected.
//! `dynduel` runs several one-on-one duels at once (players / 2 arenas, phases of the same map):
//! the winner stays, the loser queues and watches an arena (Space / click: the next one), the next
//! in line challenges the winner; the time limit is for the match, a duel lasts `--round-time`.
//! `blitzkrieg` plays the map `blitzkrieg` only: soldiers march along the lanes, destroy the
//! enemy barricades and radar; `F` opens the honor upgrade panel (Up/Down, Enter buys); the time
//! limit is optional, headless checks may set `GUNZ_BLITZ_BUY=SECS:N,..` and `GUNZ_BLITZ_HP=K`.
//! `clanwar` (retail game type 22, needs a clan made in the menu's CLAN tab): the clan and three
//! bot members against a rival clan, 4 against 4, rounds; the kill limit counts round wins (3);
//! the clan's points change when the match ends (`docs/formats.md`, "Clans").
//! Rules (not in the menu): `--respawn S` (seconds dead before the respawn, default 5),
//! `--protect S` (spawn protection, default 3), `--round-time S` (round limit, default 180),
//! `--ready S` (countdown before a round, default 3).
//! LAN (also the menu's HOST LAN / JOIN LAN): `--host` serves this match on TCP+UDP port 7790
//! (modes `dm`, `tdm`, `gladiator`, `team-gladiator`); `--join ADDR[:PORT]` plays the match of the
//! host at ADDR, `--join lan` finds it with a broadcast. A joining player brings its character and
//! weapons; map, mode, limits and bots are the host's (`src/net.rs`).
//! Quest: `--mode quest --scenario NAME [--dice N] [--sacrifice A,B] [--bots N]` plays a retail quest
//! (no MAP: the scenario's first sector is the map). NAME is a scenario title (`"Quest Mansion QL0"`,
//! `"Goblin King"`), `"Challenge 101"`, `"Survival Prison"` or a special id / challenge id;
//! `--dice` picks the scenario's `<MAP dice>` (default: rolled, 1..6; `GUNZ_SEED=N` fixes the roll
//! and the NPC picks, headless runs use seed 1); `--sacrifice A,B` puts quest items from the
//! profile into the two sacrifice slots (a special scenario's two items switch to it; standard
//! quests of level 2+ need their Torn Page; the start spends them; `GUNZ_PROFILE=PATH` holds the
//! items); the bots are allies. Clear a sector, then walk into its portal (or wait 30 s); see
//! `docs/formats.md`, "Quest".
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
//! Without MAP, `--shot` saves the main menu instead: `--menu-page match|player|shop|inventory|clan` picks the
//! screen and the options above set what it shows.
//! `--npc NAME[,NAME..]` spawns quest monsters (`system/npc.xml` ids such as `11`, `16`, or
//! `npc2.xml` names such as `knifeman`, `tower`) in a row in front of the player, `--bots-ahead M`
//! metres away (default 8) and hostile to it; `GUNZ_NPC_HOLD=S` keeps them idle for S seconds.
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
    menu::{self, Config, Mode, Net, Page, take_arg},
    modes::DieAt,
    mrs::Vfs,
    net::{self, Host, Lan},
    profile::{OPTS, Profile, opt_mut},
    quest::{Catalog, Quest},
    session::{EXIT_AGAIN, EXIT_MENU, PauseAt, Rules, StartVitals},
    view::{self, SCALE, Shot, to_bevy},
};
use std::{process::Command, time::Duration};

/// Simulated seconds before a headless script starts, so pipelines compile and actors land.
const LEAD: f32 = 1.5;

/// Replaces this process with a fresh `gunz-play` (a new window, so no state carries over):
/// the match described by `config` (`menu == false`) or the main menu with its choices.
fn relaunch(game: &str, config: &Config, menu: bool) -> AppExit {
    let exe = std::env::current_exe().expect("own executable path");
    let mut cmd = Command::new(exe);
    cmd.arg(game);
    // back in the menu, hosting or joining is chosen again
    let config = &Config {
        net: config.net.clone().filter(|_| !menu),
        ..config.clone()
    };
    match (&config.map, menu) {
        (Some(map), true) => cmd.args(["--map", map]),
        (Some(map), false) => cmd.arg(map),
        (None, _) => &mut cmd,
    };
    let cmd = cmd.args(config.flags());
    // A fresh process: exec replaces this one on Unix; elsewhere spawn it and exit.
    #[cfg(unix)]
    let err = std::os::unix::process::CommandExt::exec(cmd);
    #[cfg(not(unix))]
    let err = match cmd.spawn() {
        Ok(_) => return AppExit::Success,
        Err(e) => e,
    };
    eprintln!("cannot restart gunz-play: {err}");
    AppExit::from_code(1)
}

/// Admits and plans the scenario `config` names, spends its sacrifice from the profile and
/// returns the quest; `config.scenario` becomes the scenario actually played.
fn start_quest(vfs: &Vfs, config: &mut Config, headless: bool) -> Result<Quest, String> {
    let cat = Catalog::load(vfs).map_err(|e| format!("quest data: {e}"))?;
    let mut profile = Profile::open(headless);
    let name = config.scenario.as_deref();
    let ok = cat.admit(
        name,
        config.sacrifice,
        &profile.quest_items,
        profile.level(),
    )?;
    if !ok.spend.is_empty() {
        let names: Vec<_> = ok.spend.iter().map(|&i| cat.items.name(i)).collect();
        println!("quest: sacrificed {}", names.join(" + "));
        profile.spend_quest_items(&ok.spend);
        profile.save();
    }
    let seed = seed(headless);
    let plan = cat.plan(&ok.scenario, config.dice, seed)?;
    config.scenario = Some(ok.scenario);
    Quest::new(vfs, &cat, plan, config.bots, seed)
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
            gunz::profile::wall().subsec_nanos().max(1)
        })
}

fn main() -> AppExit {
    // In the browser the page chose the match; the packs it downloaded are the game files.
    #[cfg(target_arch = "wasm32")]
    let mut args = gunz::web::args();
    #[cfg(not(target_arch = "wasm32"))]
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz-play [GAME_DIR] [MAP] [--char man|woman] [--look LOOK] [--loadout ID,..] [--bots N]\n       \
             [--bots-ahead M] [--skill 0..1] [--sens X] [--[no-]kill-sounds|hit-sound|static-spread|team-bars|screen-blood|killcam] [--mode dm|tdm|gladiator|team-gladiator|elimination|assassinate|duel|training|berserker|tournament|gunman|spy|blitzkrieg|clanwar|gungame|infected|dynduel]\n       \
             [--time-limit S] [--kill-limit N] [--respawn S] [--protect S] [--round-time S] [--ready S] [--host | --join ADDR|lan]\n       \
             [--mode quest --scenario NAME [--dice N] [--sacrifice A,B]]\n       \
             gunz-play [GAME_DIR] [MAP] --shot OUT.png [--script SCRIPT] [--time S] [--at X,Y,Z] [--yaw DEG]\n       \
             [--hp N] [--ap N] [--pause-at S] [--die-at S] [--menu-page match|player|shop|inventory|clan] [--npc NAME[,NAME..]]\n\
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
    // `--npc NAME[,NAME..]`: quest monsters (npc.xml ids or npc2.xml names) ahead of the player.
    let npc = take_arg::<String>(&mut args, "--npc");
    if npc.is_err() {
        return usage();
    }
    let (Ok(respawn), Ok(protect), Ok(round_time), Ok(ready)) = (
        take_arg::<f32>(&mut args, "--respawn"),
        take_arg::<f32>(&mut args, "--protect"),
        take_arg::<f32>(&mut args, "--round-time"),
        take_arg::<f32>(&mut args, "--ready"),
    ) else {
        return usage();
    };
    // `--NAME` / `--no-NAME` override the profile's pause-menu toggles for this run only.
    let mut toggles = vec![];
    for (i, (key, _)) in OPTS.iter().enumerate() {
        let flag = key.replace('_', "-");
        for (f, on) in [(format!("--{flag}"), true), (format!("--no-{flag}"), false)] {
            if let Some(at) = args.iter().position(|a| *a == f) {
                args.remove(at);
                toggles.push((i, on));
            }
        }
    }
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
    #[cfg(target_arch = "wasm32")]
    let (game, vfs) = (String::new(), gunz::web::vfs().expect("game packs"));
    // GAME_DIR is the first argument when it is a directory (or looks like a path, so a typo
    // is reported as a bad directory); otherwise find the Steam install.
    #[cfg(not(target_arch = "wasm32"))]
    let game = match args
        .first()
        .filter(|a| std::path::Path::new(a).is_dir() || a.contains(['/', '\\']))
    {
        Some(_) => args.remove(0),
        None => match gunz::steam::find_game() {
            Some(dir) => dir.to_string_lossy().into_owned(),
            None => {
                eprintln!("GUNZ THE DUEL not found in any Steam library; pass GAME_DIR");
                return usage();
            }
        },
    };
    #[cfg(not(target_arch = "wasm32"))]
    let vfs = Vfs::mount(&game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let name = Profile::open(shot.is_some()).name;
    // A LAN client plays the host's map, mode and limits.
    let client = match config.net.clone() {
        Some(Net::Join(addr)) => {
            match net::join(&addr, &name, config.woman, &config.look, &config.loadout) {
                Ok((w, c)) => {
                    println!("lan: joined {} ({})", w.map, w.mode.arg());
                    (config.mode, config.time_limit, config.kill_limit) =
                        (w.mode, w.time_limit, w.kill_limit);
                    Some((w, c))
                }
                Err(e) => {
                    eprintln!("--join: {e}");
                    if shot.is_some() {
                        return AppExit::from_code(2);
                    }
                    config.net = None;
                    return match menu::run(vfs, config, Page::Match, None) {
                        Some(chosen) => relaunch(&game, &chosen, false),
                        None => AppExit::Success,
                    };
                }
            }
        }
        _ => None,
    };
    // Quest mode plays a scenario's sectors in turn: its first sector is the map. Without a
    // scenario (or MAP) the menu picks one; `--menu-page` always shows the menu.
    let wanted = page.is_none()
        && (config.scenario.is_some() || config.sacrifice != [0; 2] || !args.is_empty());
    let quest = if config.mode == Mode::Quest && wanted {
        match start_quest(&vfs, &mut config, shot.is_some()) {
            Ok(q) => Some(q),
            Err(e) => {
                eprintln!("{e}");
                if shot.is_some() {
                    return AppExit::from_code(2);
                }
                // e.g. "Play again" without the offering left: the menu says what is missing
                return match menu::run(vfs, config, Page::Match, None) {
                    Some(chosen) => relaunch(&game, &chosen, false),
                    None => AppExit::Success,
                };
            }
        }
    } else {
        None
    };
    let first = quest.as_ref().map(|q| q.first_map().to_owned());
    let joined = client.as_ref().map(|(w, _)| w.map.clone());
    let Some(map_name) = (first.clone().or(joined)).or_else(|| args.first().cloned()) else {
        // No MAP: the main menu; Start leaves the choice behind, then the game starts.
        return match menu::run(vfs, config, page.unwrap_or(Page::Match), shot) {
            Some(chosen) => relaunch(&game, &chosen, false),
            None => AppExit::Success,
        };
    };
    // A quest restarts from its scenario, not from a map.
    config.map = first.is_none().then(|| map_name.to_ascii_lowercase());
    if config.mode == Mode::Blitzkrieg && !map_name.eq_ignore_ascii_case("blitzkrieg") {
        eprintln!("--mode blitzkrieg is played on the map blitzkrieg");
        return AppExit::from_code(2);
    }
    let rs = map::find_rs(&vfs, &map_name).unwrap_or_else(|| panic!("no map named {map_name}"));
    let col = MapCollision::load(&vfs, &rs).unwrap_or_else(|e| panic!("{rs} collision: {e}"));
    let map = map::load(&vfs, &rs).unwrap_or_else(|e| panic!("{rs}: {e}"));
    let level = Level { vfs, map };
    let mut data = ActorData::new(&level).unwrap_or_else(|e| panic!("items/characters: {e}"));
    data.seed = seed(shot.is_some()) as u64;

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
    if let Ok(Some(names)) = npc {
        app.insert_resource(gunz::npc::NpcDemo {
            names: names.split(',').map(str::to_string).collect(),
            ahead: ahead.unwrap_or(8.0),
        });
    }
    if let Some(s) = pause_at {
        app.insert_resource(PauseAt(s));
    }
    if let Some(s) = die_at {
        app.insert_resource(DieAt(s));
    }
    if let Some(q) = quest {
        app.insert_resource(q);
    }
    let mut rules = Rules::new(config.mode, config.time_limit, config.kill_limit);
    rules.respawn = respawn.unwrap_or(rules.respawn);
    rules.protect = protect.unwrap_or(rules.protect);
    rules.round_secs = round_time.unwrap_or(rules.round_secs);
    rules.ready = ready.unwrap_or(rules.ready);
    // The training range has no bots, only dummies; a clan war is 4 against 4 with the player.
    let mut bots = match config.mode {
        Mode::Training => 0,
        Mode::ClanWar => 2 * gunz::clan::WAR_SIZE - 1,
        _ => config.bots,
    };
    let mut start = (
        at.map(|p| Vec3::from(to_bevy(p)) * SCALE),
        yaw.map(f32::to_radians),
    );
    match (client, &config.net) {
        // The host's bots and rules; its spawn point unless `--at`/`--yaw` say otherwise.
        (Some((w, c)), _) => {
            (rules.respawn, rules.protect, bots) = (w.respawn, w.protect, 0);
            start = (start.0.or(Some(w.at)), start.1.or(Some(w.yaw)));
            app.insert_resource(c).insert_resource(Lan);
        }
        (None, Some(Net::Host)) => match Host::bind(&map_name) {
            Ok(h) => {
                println!("lan: hosting {map_name} on port {}", net::PORT);
                app.insert_resource(h).insert_resource(Lan);
            }
            Err(e) => {
                eprintln!("--host: port {}: {e}", net::PORT);
                return AppExit::from_code(2);
            }
        },
        _ => {}
    }
    if hp.is_some() || ap.is_some() {
        app.insert_resource(StartVitals { hp, ap });
    }
    #[cfg(target_arch = "wasm32")]
    app.add_plugins(gunz::web::WebPlugin);
    let sensitivity = Settings::default().sensitivity * config.sens;
    let exit = app
        .add_plugins((
            LevelPlugin,
            FxPlugin(None),
            GamePlugin,
            effect::WarmFxPlugin,
        ))
        .insert_resource(PlayerSetup {
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
        })
        .insert_resource({
            let mut s = Settings {
                sensitivity,
                ..default()
            };
            Profile::open(headless).apply(&mut s);
            for &(i, on) in &toggles {
                *opt_mut(&mut s, i) = on;
            }
            s
        })
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
