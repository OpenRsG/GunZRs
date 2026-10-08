//! `gunz-anim GAME_DIR [man|woman] [ANIM_NAME] [--type N] [--time SECONDS] [--shot OUT.png]`:
//! the player character playing the character-XML animation `ANIM_NAME` (default `idle`; with
//! `--type N` the one for weapon motion type N, else the first).
//! `gunz-anim GAME_DIR --elu VFS/PATH.elu [--time SECONDS] [--shot OUT.png]`: any ELU model
//! playing its sibling `PATH.elu.ani`.
//! Character options: `--upper NAME [--upper-time SECONDS]` plays that clip of the same motion
//! type over the upper body (`Animator::set_upper`), `--pitch DEGREES` aims up (+) or down.
//! `--time` freezes the pose at that time; otherwise the clip plays (windowed: fly camera,
//! Esc quits).

use bevy::{mesh::skinning::SkinnedMeshInverseBindposes, prelude::*};
use gunz::{
    ani,
    anim::{AnimPlugin, Animator, Loop},
    character::{self, Character, Outfit},
    elu::{self, Elu},
    model::{self, Textures},
    mrs::Vfs,
    view::{self, SCALE, Shot},
};
use std::sync::Arc;

enum Subject {
    Character(Character),
    /// VFS path and parsed model.
    Elu(String, Arc<Elu>),
}

#[derive(Resource)]
struct Scene {
    vfs: Vfs,
    subject: Subject,
    /// VFS path of the `.elu.ani` and how it ends.
    ani: (String, Loop),
    /// Upper-body layer: VFS path, how it ends, seek time.
    upper: Option<(String, Loop, f32)>,
    /// Aim pitch in degrees, positive up.
    pitch: f32,
    time: Option<f32>,
}

/// Removes `flag VALUE` from `args` and parses the value; `Err` if the value is missing/bad.
fn take<T: std::str::FromStr>(args: &mut Vec<String>, flag: &str) -> Result<Option<T>, ()> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    let v = args.get(i + 1).and_then(|v| v.parse().ok()).ok_or(())?;
    args.drain(i..=i + 1);
    Ok(Some(v))
}

fn main() -> AppExit {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz-anim GAME_DIR [man|woman] [ANIM_NAME] [--type N] [--time SECONDS] [--shot OUT.png]\n       \
             gunz-anim GAME_DIR --elu VFS/PATH.elu [--time SECONDS] [--shot OUT.png]"
        );
        AppExit::from_code(2)
    };
    let (
        Ok(shot),
        Ok(time),
        Ok(motion_type),
        Ok(elu_path),
        Ok(upper_name),
        Ok(upper_time),
        Ok(pitch),
    ) = (
        view::take_shot_arg(&mut args),
        take::<f32>(&mut args, "--time"),
        take::<u32>(&mut args, "--type"),
        take::<String>(&mut args, "--elu"),
        take::<String>(&mut args, "--upper"),
        take::<f32>(&mut args, "--upper-time"),
        take::<f32>(&mut args, "--pitch"),
    )
    else {
        return usage();
    };
    let mut upper = None;
    let Some(game) = args.first().cloned() else {
        return usage();
    };
    let vfs = Vfs::mount(&game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let (title, subject, ani) = if let Some(path) = elu_path {
        let bytes = vfs.read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let elu = elu::load(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        let ani = format!("{path}.ani");
        (
            path.clone(),
            Subject::Elu(path, Arc::new(elu)),
            (ani, Loop::Wrap),
        )
    } else {
        let name = match args.get(1).map_or("man", String::as_str) {
            "man" => "heroman1",
            "woman" => "herowoman1",
            _ => return usage(),
        };
        let anim_name = args.get(2).map_or("idle", String::as_str);
        let character = character::load(&vfs, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let find = |n: &str| {
            character
                .animations
                .iter()
                .find(|a| a.name == n && motion_type.is_none_or(|t| a.motion_type == t))
        };
        let a = find(anim_name).unwrap_or_else(|| {
            panic!("{name} has no animation `{anim_name}` (type {motion_type:?})")
        });
        println!(
            "{name}: `{}` = {} (motion type {}, {})",
            a.name, a.file, a.motion_type, a.loop_type
        );
        upper = upper_name.map(|n| {
            let u = find(&n).unwrap_or_else(|| panic!("{name} has no animation `{n}`"));
            (
                u.file.clone(),
                Loop::from_xml(&u.loop_type),
                upper_time.unwrap_or(0.0),
            )
        });
        let ani = (a.file.clone(), Loop::from_xml(&a.loop_type));
        (
            format!("{name} {anim_name}"),
            Subject::Character(character),
            ani,
        )
    };
    let mut app = view::app(&format!("gunz-anim {title}"), shot);
    app.add_plugins(AnimPlugin)
        .insert_resource(ClearColor(Color::srgb(0.18, 0.2, 0.24)))
        .insert_resource(Scene {
            vfs,
            subject,
            ani,
            upper,
            pitch: pitch.unwrap_or(0.0),
            time,
        })
        .add_systems(Startup, spawn)
        .run()
}

fn spawn(
    mut commands: Commands,
    scene: Res<Scene>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut inverse_bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    shot: Option<Res<Shot>>,
) {
    let mut textures = Textures::new(&scene.vfs, "model/");
    let (root, elu, eye, dir) = match &scene.subject {
        Subject::Character(character) => {
            let model = character::spawn(
                &mut commands,
                &mut meshes,
                &mut inverse_bindposes,
                &mut images,
                &mut materials,
                &mut textures,
                &scene.vfs,
                character,
                &Outfit::base(),
                Transform::IDENTITY,
            )
            .unwrap_or_else(|e| panic!("spawn: {e}"));
            // The character faces +Z; look at it from the front, framing its ~1.8 m height.
            (model.root, None, Vec3::new(0.0, 0.95, 2.4), Vec3::NEG_Z)
        }
        Subject::Elu(path, elu) => {
            let dir = path
                .rsplit_once('/')
                .map_or(String::new(), |(d, _)| format!("{d}/"));
            let mut material =
                |m: &elu::Material| textures.standard(&mut images, &mut materials, &dir, m);
            let model = model::spawn_elu(
                &mut commands,
                &mut meshes,
                &mut inverse_bindposes,
                elu,
                &mut material,
                Transform::IDENTITY,
                |_| true,
            );
            // Frame the bind-pose bounds.
            let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
            for n in &elu.nodes {
                let world = model::bevy_matrix(&n.world);
                for p in &n.positions {
                    let p = world.transform_point3(Vec3::new(p[0], p[1], -p[2])) * SCALE;
                    (lo, hi) = (lo.min(p), hi.max(p));
                }
            }
            let (center, radius) = ((lo + hi) / 2.0, ((hi - lo) / 2.0).length().max(0.2));
            (
                model.root,
                Some(elu.clone()),
                center + Vec3::Z * (radius * 2.0 + 0.3),
                Vec3::NEG_Z,
            )
        }
    };
    let (file, looping) = &scene.ani;
    let bytes = scene
        .vfs
        .read(file)
        .unwrap_or_else(|e| panic!("{file}: {e}"));
    let clip = ani::load(&bytes).unwrap_or_else(|e| panic!("{file}: {e}"));
    println!(
        "{:?} animation: {} frames, {} nodes",
        clip.kind,
        clip.max_frame,
        clip.nodes.len()
    );
    let mut animator = Animator::new(Arc::new(clip), *looping);
    animator.elu = elu;
    animator.aim_pitch = scene.pitch.to_radians();
    if let Some((file, looping, at)) = &scene.upper {
        let bytes = scene
            .vfs
            .read(file)
            .unwrap_or_else(|e| panic!("{file}: {e}"));
        let clip = ani::load(&bytes).unwrap_or_else(|e| panic!("{file}: {e}"));
        animator.set_upper(Arc::new(clip), *looping, 0.0);
        animator.seek_upper(*at);
    }
    if let Some(t) = scene.time {
        animator.time = t;
        animator.speed = 0.0;
    }
    commands.entity(root).insert(animator);
    view::spawn_camera(&mut commands, &mut images, shot.is_some(), eye, dir);
}
