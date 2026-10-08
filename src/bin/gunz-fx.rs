//! `gunz-fx GAME_DIR EFFECT[,EFFECT...] [--time S] [--zoom F] [--yaw DEGREES] [--shot OUT.png]`:
//! plays sfx effects (`sfx/effect_list.xml` names, or `.elu` file names) side by side with their
//! animation. `gunz-fx GAME_DIR --list` prints the effect names. `--time S` freezes the animation
//! at second `S` (default 0 with `--shot`); `--zoom F` scales the camera distance, `--yaw` turns
//! the camera around the effects.

use bevy::prelude::*;
use gunz::{
    ani,
    anim::Loop,
    effect::{self, EffectDef, FxAssets, FxPlugin, Loader},
    elu,
    mrs::Vfs,
    view,
};
use std::sync::Arc;

struct Item {
    elu: Arc<elu::Elu>,
    ani: Option<(Arc<ani::Ani>, Loop)>,
    placement: Transform,
    dir: String,
}

#[derive(Resource)]
struct Fx {
    vfs: Vfs,
    items: Vec<Item>,
    eye: Vec3,
    look: Vec3,
}

fn main() -> AppExit {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz-fx GAME_DIR EFFECT[,EFFECT...] [--time S] [--zoom F] [--yaw DEGREES] [--shot OUT.png]\n       gunz-fx GAME_DIR --list"
        );
        AppExit::from_code(2)
    };
    let (Ok(shot), Ok(time), Ok(zoom), Ok(yaw)) = (
        view::take_shot_arg(&mut args),
        effect::take_f32_arg(&mut args, "--time"),
        effect::take_f32_arg(&mut args, "--zoom"),
        effect::take_f32_arg(&mut args, "--yaw"),
    ) else {
        return usage();
    };
    let zoom = zoom.unwrap_or(1.0);
    let [game, what] = args.as_slice() else {
        return usage();
    };
    let vfs = Vfs::mount(game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let defs = effect::load_list(&vfs).unwrap_or_else(|e| panic!("{e}"));
    if what == "--list" {
        for d in &defs {
            println!("{}", d.name);
        }
        return AppExit::Success;
    }

    let mut items = Vec::new();
    let (mut cursor, mut radius, mut height) = (0.0f32, 0.0f32, 0.0f32);
    for name in what.split(',') {
        let def = effect::find(&defs, name).cloned().or_else(|| {
            let model = format!("sfx/{}", gunz::mrs::normalize(name));
            vfs.exists(&model).then(|| EffectDef {
                name: name.into(),
                animation: Some(format!("{model}.ani"))
                    .filter(|p| vfs.exists(p))
                    .map(|p| (p, true)),
                model,
                particle: None,
            })
        });
        let Some(def) = def else {
            eprintln!("no effect named {name} (see --list)");
            return AppExit::from_code(1);
        };
        let load = |def: &EffectDef| -> std::io::Result<Item> {
            let e = Arc::new(elu::load(&vfs.read(&def.model)?)?);
            let ani = match &def.animation {
                Some((path, looping)) => Some((
                    Arc::new(ani::load(&vfs.read(path)?)?),
                    if *looping { Loop::Wrap } else { Loop::Hold },
                )),
                None => None,
            };
            Ok(Item {
                elu: e,
                ani,
                placement: Transform::IDENTITY,
                dir: "sfx/".into(),
            })
        };
        let mut item = load(&def).unwrap_or_else(|e| panic!("{}: {e}", def.name));
        let (center, r) = effect::bounds(&item.elu);
        println!(
            "{}: {} nodes, {} materials, animation {}",
            def.name,
            item.elu.nodes.len(),
            item.elu.materials.len(),
            item.ani
                .as_ref()
                .map_or("none".into(), |(a, _)| format!("{} frames", a.max_frame))
        );
        // Lay effects out left to right, centred on the origin.
        item.placement.translation = Vec3::new(cursor + r - center.x, -center.y, -center.z);
        cursor += 2.4 * r;
        radius = radius.max(r);
        height = height.max(r);
        items.push(item);
    }
    let width = cursor - 0.4 * radius;
    // Horizontal FOV is 90 degrees: half the width fits at distance `width / 2`.
    let distance = ((width * 0.5).max(height * 16.0 / 9.0) * 1.2 + 0.3) * zoom;
    let look = Vec3::new(width * 0.5, 0.0, 0.0);
    let offset = Quat::from_rotation_y(yaw.unwrap_or(0.0).to_radians()) * Vec3::new(0.0, 0.2, 1.0);
    let eye = look + offset.normalize() * distance;

    let mut app = view::app(what, shot.clone());
    app.insert_resource(Fx {
        vfs,
        items,
        eye,
        look,
    })
    .add_plugins(FxPlugin(time.or(shot.map(|_| 0.0))))
    .add_systems(Startup, (spawn_effects, spawn_camera))
    .run()
}

fn spawn_effects(mut commands: Commands, fx: Res<Fx>, mut assets: FxAssets) {
    let mut loader = Loader::new(&fx.vfs, "sfx/");
    for item in &fx.items {
        loader.spawn(
            &mut assets,
            &mut commands,
            &item.dir,
            &item.elu,
            item.ani.clone(),
            item.placement,
        );
    }
}

fn spawn_camera(
    mut commands: Commands,
    fx: Res<Fx>,
    mut images: ResMut<Assets<Image>>,
    shot: Option<Res<view::Shot>>,
) {
    view::spawn_camera(
        &mut commands,
        &mut images,
        shot.is_some(),
        fx.eye,
        (fx.look - fx.eye).normalize(),
    );
}
