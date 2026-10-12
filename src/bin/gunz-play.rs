//! `gunz-play [GAME_DIR] [MAP] [OPTIONS]`: play a retail map as a GunZ character. Without
//! GAME_DIR the Steam install is found through `libraryfolders.vdf` (`src/steam.rs`). Without MAP the
//! main menu opens (map, mode and limits, bots, character and clothes, loadout, controls);
//! Start shows a loading screen (the map is read off the main thread) and then plays the match,
//! and "Main menu" / "Play again" in the pause or match-end screen come back through the same
//! window: the whole run is one process and one window (`src/phase.rs`).
//!
//! Default controls: WASD move, Space jump (in the air next to a wall: wall kick), double-tap
//! a direction or C to tumble, mouse look, left mouse (or left Ctrl) attack, right mouse (or
//! left Shift) guard (melee), R reload, 1..5, Q/E or the wheel switch
//! weapon, Tab scoreboard, Esc pause menu (resume, controls, main menu, quit). Every key and
//! mouse button can be rebound (CONTROLS in the menus, saved in the profile: `src/controls.rs`).
//!
//! OPTIONS (the menu writes the same ones): `--char man|woman`, `--look LOOK` (clothes and
//! dyes, the profile's `look` text: six 1-based parts `;` six tints), `--loadout ID,ID,..` (zitem ids), `--bots N`,
//! `--bots-ahead M` (spawn them M metres in front of the player), `--skill 0..1` (bot
//! difficulty), `--mode MODE`, `--time-limit
//! SECONDS` and `--kill-limit N` (0 = none; the defaults are `gametypecfg.xml`'s, none in
//! headless runs), `--map NAME` (with no MAP: preselect it in the menu).
//! TOGGLES (also in the pause menu, saved in the profile as `opt_NAME=0|1`; a flag overrides the
//! profile for this run): `--kill-sounds`/`--no-kill-sounds` (default on: the combo layer's
//! synthesized sounds), `--combo` (on: combo meter and multikill/spree banners), `--hit-sound` (plays
//! `<profile dir>/custom/hitsound.wav`, default off), `--static-spread` (off), `--team-bars` (on),
//! `--screen-blood` (on), `--killcam` (on), `--realistic-blood` (on: simulated gore, off: retail
//! blood sprites and marks); each has a `--no-` form.
//! MODE is `dm` (deathmatch), `tdm` (team deathmatch), `gladiator` / `team-gladiator` (melee
//! weapons only), `elimination` (team rounds, no respawn until the round ends), `assassinate`
//! (rounds, one VIP per team), `duel` (one-on-one rounds, the winner stays, the rest queue and
//! watch), `training` (no bots, dummy targets), `berserker` (deathmatch: the first killer turns
//! berserker, 150 % damage and draining health; kill it to become it), `tournament` (knockout
//! bracket of duels), `gunman` (a random melee weapon and gun every life) or `spy` (rounds:
//! hidden spies with grenades against trackers). In the
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
//! `--pause-at` opens the pause menu when the match clock reaches S seconds (`--pause-page
//! controls|graphics` also opens that overlay), `--die-at` kills
//! the player then (the match clock starts with the first frame, 1.5 s before the script).
//! Without MAP, `--shot` saves the main menu instead: `--menu-page match|player|shop|inventory|clan|controls|graphics` picks the
//! screen and the options above set what it shows.
//! `GUNZ_MENU_START=SECS[,SECS..]` (headless, with no MAP) presses the menu's START that many
//! seconds after it opens, one value per opening, and `GUNZ_LEAVE=SECS:menu|again[,..]` presses
//! MAIN MENU / PLAY AGAIN when the match clock reaches SECS, one value per match, so one run can
//! go menu -> loading -> match -> menu -> ... in a single process: the options above (script,
//! `--at`, `--hp`, ...) apply to every match of the run and `--time` counts from each match's
//! start; `GUNZ_SEQ=SECS` with a `%` in the `--shot` path saves the frames of the loading
//! screens too.
//! `--gfx original|enhanced|ultra` takes that graphics preset for this run (it also lands in the profile
//! file the next time the profile is saved; the in-game GRAPHICS panel edits them for good).
//! `--npc NAME[,NAME..]` spawns quest monsters (`system/npc.xml` ids such as `11`, `16`, or
//! `npc2.xml` names such as `knifeman`, `tower`) in a row in front of the player, `--bots-ahead M`
//! metres away (default 8) and hostile to it; `GUNZ_NPC_HOLD=S` keeps them idle for S seconds.
//!
//! SCRIPT is `;`-separated steps run one after another: `KEYS:SECONDS` holds the `+`-joined
//! keys for that long (`w a s d` move, `jump`, `attack`, `guard`, `reload`, `tab` scoreboard,
//! `dash` the Dash action (a double tap of the held direction, forward if none), `1`..`9`
//! select weapon, `wait` nothing), `yaw=DEG` / `pitch=DEG` set the aim instantly (pitch
//! positive looks up).
//! Example: `"w:1.0;jump:0.3;2:0.1;attack:0.5;w:0.05;wait:0.05;w:0.4"` runs, jumps, draws the
//! revolver, fires, then double-taps W (tumble forward).

use bevy::{ecs::schedule::ScheduleLabel, prelude::*, time::TimeUpdateStrategy};
use gunz::{
    actor::Script,
    effect::{self, FxPlugin},
    game::GamePlugin,
    level::LevelPlugin,
    menu::{Config, MenuPlugin, Mode, Net, Page, take_arg},
    phase::{Launch, MatchStart, Mount, Opts, PhasePlugin, Screen},
    profile::OPTS,
    view::{self, SCALE, Shot, to_bevy},
};
use std::time::Duration;

fn main() -> AppExit {
    // In the browser the page chose the match; the packs it downloaded are the game files.
    #[cfg(target_arch = "wasm32")]
    let mut args = gunz::web::args();
    #[cfg(not(target_arch = "wasm32"))]
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz-play [GAME_DIR] [MAP] [--char man|woman] [--look LOOK] [--loadout ID,..] [--bots N]\n       \
             [--bots-ahead M] [--skill 0..1] [--[no-]kill-sounds|combo|hit-sound|static-spread|team-bars|screen-blood|killcam|realistic-blood] [--mode dm|tdm|gladiator|team-gladiator|elimination|assassinate|duel|training|berserker|tournament|gunman|spy|blitzkrieg|clanwar|gungame|infected|dynduel]\n       \
             [--time-limit S] [--kill-limit N] [--respawn S] [--protect S] [--round-time S] [--ready S] [--host | --join ADDR|lan]\n       \
             [--mode quest --scenario NAME [--dice N] [--sacrifice A,B]]\n       \
             gunz-play [GAME_DIR] [MAP] --shot OUT.png [--script SCRIPT] [--time S] [--at X,Y,Z] [--yaw DEG]\n       \
             [--hp N] [--ap N] [--pause-at S [--pause-page controls|graphics]] [--die-at S] [--menu-page match|player|shop|inventory|clan|controls|graphics] [--npc NAME[,NAME..]]\n       \
             [--gfx original|enhanced|ultra]\n\
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
    let (Ok(gfx), Ok(pause_page)) = (
        take_arg::<gunz::gfx::Preset>(&mut args, "--gfx"),
        take_arg::<Page>(&mut args, "--pause-page"),
    ) else {
        return usage();
    };
    // `--npc NAME[,NAME..]`: quest monsters (npc.xml ids or npc2.xml names) ahead of the player.
    let Ok(npc) = take_arg::<String>(&mut args, "--npc") else {
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
    let config = match Config::parse(&mut args, shot.is_some()) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("{e}");
            return usage();
        }
    };
    let secs = match script.as_deref().map(Script::parse).transpose() {
        Ok(s) => time.unwrap_or_else(|| s.as_ref().map_or(0.5, |s| s.secs() + 0.3)),
        Err(e) => {
            eprintln!("--script: {e}");
            return usage();
        }
    };
    #[cfg(target_arch = "wasm32")]
    let (_game, vfs) = (String::new(), gunz::web::vfs().expect("game packs"));
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
    let vfs = match gunz::mrs::Vfs::mount(&game) {
        Ok(vfs) => vfs,
        Err(e) => {
            eprintln!("{game}: {e} (not a GUNZ THE DUEL install? pass GAME_DIR or set GUNZ_GAME)");
            return AppExit::from_code(1);
        }
    };
    // A MAP, a quest scenario or a LAN game to join starts a match at once (a quest with
    // `--menu-page` shows the menu); without one the main menu opens.
    let quest = config.mode == Mode::Quest
        && page.is_none()
        && (config.scenario.is_some() || config.sacrifice != [0; 2] || !args.is_empty());
    let direct = quest || matches!(config.net, Some(Net::Join(_))) || !args.is_empty();
    let mut config = config;
    config.map = args.first().map(|m| m.to_ascii_lowercase()).or(config.map);
    let launch = Launch {
        config,
        page,
        opts: Opts {
            script,
            at: at.map(|p| Vec3::from(to_bevy(p)) * SCALE),
            yaw: yaw.map(f32::to_radians),
            ahead,
            npc,
            pause_at: pause_at.map(|s| (s, pause_page)),
            die_at,
            hp,
            ap,
            respawn,
            protect,
            round_time,
            ready,
            toggles,
            gfx,
            secs,
        },
    };
    let mut app = view::app_plain("gunz-play", shot);
    if app.world().contains_resource::<Shot>() {
        app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f64(
            1.0 / 60.0,
        )));
    }
    #[cfg(target_arch = "wasm32")]
    app.add_plugins(gunz::web::WebPlugin);
    app.insert_resource(Mount(vfs))
        .insert_resource(launch)
        .add_plugins((
            PhasePlugin(if direct {
                Screen::Loading
            } else {
                Screen::Menu
            }),
            LevelPlugin(MatchStart.intern()),
            FxPlugin(None),
            GamePlugin,
            effect::WarmFxPlugin,
            MenuPlugin,
        ))
        .run()
}
