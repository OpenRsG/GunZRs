//! `gunz GAME_DIR [MAP] [--shot OUT.png]`: renders a retail GunZ map with a free-fly camera.
//! Controls: WASD move, Space/C up/down, Shift fast, hold right mouse to look, Esc quit.
//! `--shot` renders one 1280x720 frame from the spawn point without opening a window.

use bevy::prelude::*;
use gunz::{
    effect::{self, FxPlugin},
    level::{Level, LevelPlugin},
    map,
    mrs::Vfs,
    view::{self, SCALE, Shot, to_bevy},
};

fn main() -> AppExit {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz GAME_DIR [MAP] [--eye X,Y,Z --look X,Y,Z] [--time S] [--shot OUT.png]\n\
             eye/look are map coordinates (cm); --time fixes the texture animation clock"
        );
        AppExit::from_code(2)
    };
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
    let Some(game) = args.first().cloned() else {
        return usage();
    };
    let map_name = args.get(1).cloned().unwrap_or_else(|| "Mansion".into());
    let vfs = Vfs::mount(&game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let rs = map::find_rs(&vfs, &map_name).unwrap_or_else(|| panic!("no map named {map_name}"));
    let level = map::load(&vfs, &rs).unwrap_or_else(|e| panic!("{rs}: {e}"));
    println!(
        "{rs}: {} polygons, {} vertices, {} materials, {} lightmaps",
        level.polygons.len(),
        level.vertices.len(),
        level.materials.len(),
        level.lightmaps.len()
    );

    let mut app = view::app(&map_name, shot);
    app.add_plugins((LevelPlugin, FxPlugin(time)))
        .insert_resource(Level { vfs, map: level })
        .insert_resource(ViewOverride(view))
        .add_systems(Startup, spawn_view)
        .run()
}

/// Camera override (position, direction) in Bevy coordinates.
#[derive(Resource)]
struct ViewOverride(Option<(Vec3, Vec3)>);

/// Starts at `--eye/--look`, else at eye height above the first spawn point.
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
