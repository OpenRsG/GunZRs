//! `gunz-char GAME_DIR [man|woman] [--set N | --look LOOK] [--shot OUT.png]`: the assembled
//! player character in bind pose, standing at the origin. `--set N` dresses every slot that
//! `AddParts` set N (1-based position in the character XML) provides; `--look` takes the
//! profile's text form (`character::Look`: six 1-based parts `;` six tints).
//! `gunz-char GAME_DIR --elu VFS/PATH.elu [--shot OUT.png]`: any single ELU model, camera
//! framed on its bounds (every node's mesh is shown, weapon/bone helpers included).
//! Windowed: fly camera (WASD, Esc quits).

use bevy::{mesh::skinning::SkinnedMeshInverseBindposes, prelude::*};
use gunz::{
    character::{self, Character, Look},
    elu::{self, Elu},
    model::{self, Textures},
    mrs::Vfs,
    view::{self, SCALE, Shot},
};

enum Subject {
    Character(Character, Look),
    Elu(String, Elu),
}

#[derive(Resource)]
struct Scene {
    vfs: Vfs,
    subject: Subject,
}

fn main() -> AppExit {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz-char GAME_DIR [man|woman] [--set N | --look LOOK] [--shot OUT.png]\n       \
             gunz-char GAME_DIR --elu VFS/PATH.elu [--shot OUT.png]"
        );
        AppExit::from_code(2)
    };
    let Ok(shot) = view::take_shot_arg(&mut args) else {
        return usage();
    };
    let mut option = |flag: &str| {
        let i = args.iter().position(|a| a == flag)?;
        let v = args.get(i + 1).cloned();
        args.drain(i..=(i + 1).min(args.len() - 1));
        Some(v)
    };
    let (set, path, look) = (option("--set"), option("--elu"), option("--look"));
    let Some(game) = args.first().cloned() else {
        return usage();
    };
    let vfs = Vfs::mount(&game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let (title, subject) = if let Some(path) = path {
        let Some(path) = path else { return usage() };
        let bytes = vfs.read(&path).unwrap_or_else(|e| panic!("{path}: {e}"));
        let elu = elu::load(&bytes).unwrap_or_else(|e| panic!("{path}: {e}"));
        println!(
            "{path}: v{:#x}, {} nodes, {} materials",
            elu.version,
            elu.nodes.len(),
            elu.materials.len()
        );
        (path.clone(), Subject::Elu(path, elu))
    } else {
        let name = match args.get(1).map_or("man", String::as_str) {
            "man" => "heroman1",
            "woman" => "herowoman1",
            _ => return usage(),
        };
        let character = character::load(&vfs, name).unwrap_or_else(|e| panic!("{name}: {e}"));
        let look = match (set, look) {
            (None, None) => Look::default(),
            (_, Some(l)) => match l.map(|l| l.parse::<Look>()) {
                Some(Ok(l)) => l.fit(&character),
                Some(Err(e)) => panic!("--look: {e}"),
                None => return usage(),
            },
            (Some(n), None) => {
                let Some(n) = n.and_then(|n| n.parse::<usize>().ok()) else {
                    return usage();
                };
                let part = n.checked_sub(1).filter(|&p| p < character.parts.len());
                let part =
                    part.unwrap_or_else(|| panic!("--set {n}: {} sets", character.parts.len()));
                Look::set(&vfs, &character, part).unwrap_or_else(|e| panic!("set {n}: {e}"))
            }
        };
        println!(
            "{name}: base {}, {} part sets, {} animations",
            character.base,
            character.parts.len(),
            character.animations.len()
        );
        (name.to_string(), Subject::Character(character, look))
    };
    let mut app = view::app(&format!("gunz-char {title}"), shot);
    app.insert_resource(ClearColor(Color::srgb(0.18, 0.2, 0.24)))
        .insert_resource(Scene { vfs, subject })
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
    // Camera: in front of the subject (+Z side), looking toward -Z.
    let (center, radius) = match &scene.subject {
        Subject::Character(ch, look) => {
            character::spawn(
                &mut commands,
                &mut meshes,
                &mut inverse_bindposes,
                &mut images,
                &mut materials,
                &mut textures,
                &scene.vfs,
                ch,
                look,
                Transform::IDENTITY,
            )
            .unwrap_or_else(|e| panic!("spawn: {e}"));
            // The character is ~1.8 m tall and stands on y = 0.
            (Vec3::new(0.0, 0.95, 0.0), 1.0)
        }
        Subject::Elu(path, elu) => {
            let dir = path
                .rsplit_once('/')
                .map_or(String::new(), |(d, _)| format!("{d}/"));
            let mut material =
                |m: &elu::Material| textures.standard(&mut images, &mut materials, &dir, m);
            model::spawn_elu(
                &mut commands,
                &mut meshes,
                &mut inverse_bindposes,
                elu,
                &mut material,
                Transform::IDENTITY,
                |_| true,
            );
            bounds(elu)
        }
    };
    // Horizontal FOV is 90 degrees at 16:9 (vertical ~59 degrees): fit the radius vertically.
    let dist = radius * 2.0 + 0.3;
    view::spawn_camera(
        &mut commands,
        &mut images,
        shot.is_some(),
        center + Vec3::new(0.0, 0.0, dist),
        Vec3::NEG_Z,
    );
}

/// Bounding sphere (center, radius in metres) of all meshes in Bevy space.
fn bounds(elu: &Elu) -> (Vec3, f32) {
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for n in elu.nodes.iter().filter(|n| !model::is_bone(&n.name)) {
        let world = model::bevy_matrix(&n.world);
        for p in &n.positions {
            let p = world.transform_point3(Vec3::new(p[0], p[1], -p[2])) * SCALE;
            (lo, hi) = (lo.min(p), hi.max(p));
        }
    }
    if lo.x > hi.x {
        return (Vec3::ZERO, 1.0);
    }
    ((lo + hi) / 2.0, ((hi - lo).length() / 2.0).max(0.05))
}
