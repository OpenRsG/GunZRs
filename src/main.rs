//! `gunz GAME_DIR [MAP] [--shot OUT.png]`: renders a retail GunZ map with a free-fly camera.
//! Controls: WASD move, Space/C up/down, Shift fast, hold right mouse to look, Esc quit.
//! `--shot` renders one 1280x720 frame from the spawn point without opening a window;
//! `--vantage` takes it from the most open view at a spawn point instead (`Level::vantage`).
//! `gunz [GAME_DIR] --previews` makes the picture of every map that has none (960x540, in
//! `previews/` beside the profile file, `GUNZ_PROFILE`) with `--vantage --shot` runs of itself;
//! the menu and the loading screen show them (`docs/formats.md`, "Map previews").

use bevy::{ecs::schedule::ScheduleLabel, prelude::*};
use gunz::{
    effect::{self, FxPlugin},
    level::{Level, LevelPlugin},
    map,
    mrs::Vfs,
    profile::Profile,
    view::{self, PREVIEW, SCALE, Shot, to_bevy},
};
use std::{fs, process::Command};

fn main() -> AppExit {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz GAME_DIR [MAP] [--eye X,Y,Z --look X,Y,Z | --vantage] [--time S] [--shot OUT.png]\n       \
             gunz [GAME_DIR] --previews\n\
             eye/look are map coordinates (cm); --time fixes the texture animation clock"
        );
        AppExit::from_code(2)
    };
    let mut flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .map(|i| args.remove(i))
            .is_some()
    };
    let (vantage, previews) = (flag("--vantage"), flag("--previews"));
    let Ok(shot) = view::take_shot_arg(&mut args) else {
        return usage();
    };
    let Ok(time) = effect::take_f32_arg(&mut args, "--time") else {
        return usage();
    };
    let (Ok(eye), Ok(look)) = (
        effect::take_vec3_arg(&mut args, "--eye"),
        effect::take_vec3_arg(&mut args, "--look"),
    ) else {
        return usage();
    };
    let view = match (eye, look) {
        (Some(e), Some(l)) => {
            let e = Vec3::from(to_bevy(e)) * SCALE;
            let dir = Vec3::from(to_bevy(l)) * SCALE - e;
            Some((e, dir.normalize_or(Vec3::NEG_Z)))
        }
        (None, None) => None,
        _ => return usage(),
    };
    let game = args.first().cloned();
    if previews {
        let Some(game) = game.or_else(|| gunz::steam::find_game()?.to_str().map(str::to_owned))
        else {
            eprintln!("GUNZ THE DUEL not found in any Steam library; pass GAME_DIR");
            return usage();
        };
        return match make_previews(&game) {
            Ok(()) => AppExit::Success,
            Err(e) => {
                eprintln!("previews: {e}");
                AppExit::from_code(1)
            }
        };
    }
    let Some(game) = game else {
        return usage();
    };
    let map_name = args.get(1).cloned().unwrap_or_else(|| "Mansion".into());
    let vfs = Vfs::mount(&game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let rs = map::find_rs(&vfs, &map_name).unwrap_or_else(|| panic!("no map named {map_name}"));
    let level = Level {
        map: map::load(&vfs, &rs).unwrap_or_else(|e| panic!("{rs}: {e}")),
        vfs,
    };
    println!(
        "{rs}: {} polygons, {} vertices, {} materials, {} lightmaps",
        level.map.polygons.len(),
        level.map.vertices.len(),
        level.map.materials.len(),
        level.map.lightmaps.len()
    );
    let view = view.or_else(|| vantage.then(|| level.vantage()));

    let mut app = view::app(&map_name, shot);
    app.add_plugins((LevelPlugin(Startup.intern()), FxPlugin(time)))
        .insert_resource(level)
        .insert_resource(ViewOverride(view))
        .add_systems(Startup, spawn_view)
        .run()
}

/// Renders the picture of every map that has none: one `--vantage --shot` run of this program
/// per map, a few at a time. A picture is renamed into place when done, so a killed run leaves
/// no half file.
fn make_previews(game: &str) -> Result<(), String> {
    let dir = Profile::open(false)
        .previews()
        .ok_or("no profile directory")?;
    fs::create_dir_all(&dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let todo: Vec<String> = Vfs::mount(game)
        .map_err(|e| format!("mount {game}: {e}"))?
        .maps()
        .into_iter()
        .filter(|m| !dir.join(format!("{m}.png")).exists())
        .collect();
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let threads = std::thread::available_parallelism().map_or(2, |n| n.get().min(3));
    let size = format!("{}x{}", PREVIEW.0, PREVIEW.1);
    let failed = std::thread::scope(|s| {
        let jobs: Vec<_> = todo
            .chunks(todo.len().div_ceil(threads).max(1))
            .map(|maps| {
                s.spawn(|| {
                    maps.iter()
                        .filter(|m| {
                            let part = dir.join(format!("{m}.part.png"));
                            // a frame captured before the pipelines compiled is black: retry
                            let ok = (0..3).any(|_| {
                                Command::new(&exe)
                                    .args([game, m, "--vantage", "--shot"])
                                    .arg(&part)
                                    .env("GUNZ_SHOT_SIZE", &size)
                                    .stdout(std::process::Stdio::null())
                                    .stderr(std::process::Stdio::null())
                                    .status()
                                    .is_ok_and(|s| s.success())
                                    && !black(&part)
                            }) && fs::rename(&part, dir.join(format!("{m}.png"))).is_ok();
                            if !ok {
                                fs::remove_file(&part).ok();
                            }
                            println!("preview {m}{}", if ok { "" } else { ": FAILED" });
                            !ok
                        })
                        .count()
                })
            })
            .collect();
        jobs.into_iter()
            .map(|j| j.join().unwrap_or(1))
            .sum::<usize>()
    });
    match failed {
        0 => Ok(()),
        n => Err(format!("{n} maps failed")),
    }
}

/// Whether a picture is (nearly) all black: over 95% of its pixels darker than 8/255.
fn black(png: &std::path::Path) -> bool {
    let Some(data) = fs::read(png)
        .ok()
        .and_then(|b| view::decode(&b, "png", true, bevy::image::ImageSampler::linear())?.data)
    else {
        return true;
    };
    let px = data.len() / 4;
    let dark = data
        .chunks_exact(4)
        .filter(|p| p[..3].iter().all(|&c| c < 8))
        .count();
    px == 0 || dark * 20 > px * 19
}

/// Camera override (position, direction) in Bevy coordinates.
#[derive(Resource)]
struct ViewOverride(Option<(Vec3, Vec3)>);

/// Starts at `--eye/--look` or `--vantage`, else at eye height above the first spawn point.
fn spawn_view(
    mut commands: Commands,
    level: Res<Level>,
    view: Res<ViewOverride>,
    mut images: ResMut<Assets<Image>>,
    shot: Option<Res<Shot>>,
) {
    let (eye, dir) = view
        .0
        .or_else(|| {
            level
                .spawn_points()
                .first()
                .map(|&(pos, dir)| (pos + Vec3::Y * 1.6, dir))
        })
        .unwrap_or((Vec3::ZERO, Vec3::NEG_Z));
    view::spawn_camera(&mut commands, &mut images, shot.is_some(), eye, dir);
}
