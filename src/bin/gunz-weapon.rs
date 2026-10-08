//! `gunz-weapon GAME_DIR ITEM_ID_OR_NAME [--on man|woman [--idle]] [--shot OUT.png]`: a weapon
//! item's model alone, or held by the assembled character at the item's attach dummies, in bind
//! pose or (`--idle`) frozen at frame 0 of the character's idle animation for the weapon's
//! motion type. `gunz-weapon GAME_DIR --report` prints item / weapon.xml parse counts and model
//! resolution.

use bevy::{mesh::skinning::SkinnedMeshInverseBindposes, prelude::*};
use gunz::{
    ani,
    anim::{AnimPlugin, Animator, Loop},
    character::{self, Character, Outfit},
    elu::{self, Elu},
    item::{Items, WeaponKind},
    model::{self, Textures},
    mrs::Vfs,
    view::{self, SCALE, Shot},
};
use std::{collections::BTreeMap, sync::Arc};

#[derive(Resource)]
struct Scene {
    vfs: Vfs,
    elu_path: String,
    elu: Elu,
    kind: WeaponKind,
    on: Option<Character>,
    idle: Option<Arc<ani::Ani>>,
}

fn main() -> AppExit {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let usage = || {
        eprintln!(
            "usage: gunz-weapon GAME_DIR ITEM_ID_OR_NAME [--on man|woman] [--shot OUT.png]\n       gunz-weapon GAME_DIR --report"
        );
        AppExit::from_code(2)
    };
    let Ok(shot) = view::take_shot_arg(&mut args) else {
        return usage();
    };
    let on = match args.iter().position(|a| a == "--on") {
        Some(i) if i + 1 < args.len() => {
            let v = args.remove(i + 1);
            args.remove(i);
            Some(v)
        }
        Some(_) => return usage(),
        None => None,
    };
    let report = args
        .iter()
        .position(|a| a == "--report")
        .map(|i| args.remove(i))
        .is_some();
    let idle = args
        .iter()
        .position(|a| a == "--idle")
        .map(|i| args.remove(i))
        .is_some();
    let Some(game) = args.first().cloned() else {
        return usage();
    };
    let vfs = Vfs::mount(&game).unwrap_or_else(|e| panic!("mount {game}: {e}"));
    let items = Items::load(&vfs).unwrap_or_else(|e| panic!("items: {e}"));
    if report {
        print_report(&items);
        return AppExit::Success;
    }
    let Some(query) = args.get(1) else {
        return usage();
    };
    let item = query
        .parse()
        .ok()
        .and_then(|id| items.get(id))
        .or_else(|| items.find(query))
        .unwrap_or_else(|| panic!("no item with id or name {query:?}"));
    let weapon = item
        .weapon
        .as_ref()
        .unwrap_or_else(|| panic!("item {} is not a weapon", item.id));
    let model = items
        .model(item)
        .unwrap_or_else(|| panic!("item {} ({:?}) has no mesh_name", item.id, item.name));
    let elu =
        elu::load(&vfs.read(&model.elu).unwrap()).unwrap_or_else(|e| panic!("{}: {e}", model.elu));
    println!(
        "item {} {:?} {:?}: mesh {} -> {} (motion {}, type {}) damage {} delay {} magazine {}",
        item.id,
        item.name,
        weapon.kind,
        model.name,
        model.elu,
        model.motion_type,
        model.weapon_type,
        weapon.damage,
        weapon.delay,
        weapon.magazine
    );
    let on = on.map(|sex| {
        let name = match sex.as_str() {
            "man" => "heroman1",
            "woman" => "herowoman1",
            _ => panic!("--on takes man or woman"),
        };
        character::load(&vfs, name).unwrap_or_else(|e| panic!("{name}: {e}"))
    });
    let idle = on.as_ref().filter(|_| idle).map(|ch| {
        let a = ch
            .animations
            .iter()
            .find(|a| a.name == "idle" && a.motion_type == model.motion_type)
            .unwrap_or_else(|| panic!("no idle animation for motion type {}", model.motion_type));
        let bytes = vfs
            .read(&a.file)
            .unwrap_or_else(|e| panic!("{}: {e}", a.file));
        Arc::new(ani::load(&bytes).unwrap_or_else(|e| panic!("{}: {e}", a.file)))
    });
    let scene = Scene {
        vfs,
        elu_path: model.elu.clone(),
        elu,
        kind: weapon.kind,
        on,
        idle,
    };
    let mut app = view::app(&format!("gunz-weapon {}", model.name), shot);
    app.add_plugins(AnimPlugin);
    app.insert_resource(ClearColor(Color::srgb(0.18, 0.2, 0.24)))
        .insert_resource(scene)
        .add_systems(Startup, spawn)
        .run()
}

fn print_report(items: &Items) {
    let mut kinds: BTreeMap<&str, usize> = BTreeMap::new();
    for i in items.items.values() {
        *kinds.entry(&i.kind).or_default() += 1;
    }
    println!("items {} by type {kinds:?}", items.items.len());
    println!(
        "weapon.xml models {} (all .elu exist in the VFS)",
        items.models().count()
    );
    let (mut ok, mut none) = (0, vec![]);
    let mut by_kind: BTreeMap<String, usize> = BTreeMap::new();
    for i in items.weapons() {
        let w = i.weapon.as_ref().unwrap();
        match items.model(i) {
            Some(m) => {
                assert_eq!(m.motion_type, w.kind.motion_type(), "item {}", i.id);
                ok += 1;
                *by_kind.entry(format!("{:?}", w.kind)).or_default() += 1;
            }
            None => none.push(i.id),
        }
    }
    println!(
        "weapon items {}: {ok} resolve to an existing .elu, {} without mesh_name: {none:?}",
        items.weapons().count(),
        none.len()
    );
    println!("resolved by kind {by_kind:?}");
}

/// The weapon's model spawned unattached; the shell-casing helper node is an ejected-shell
/// effect, not part of the weapon.
fn spawn_weapon(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    inverse_bindposes: &mut Assets<SkinnedMeshInverseBindposes>,
    images: &mut Assets<Image>,
    standard: &mut Assets<StandardMaterial>,
    textures: &mut Textures,
    scene: &Scene,
    placement: Transform,
) -> model::Model {
    let dir = &scene.elu_path[..scene.elu_path.rfind('/').map_or(0, |i| i + 1)];
    let mut material = |m: &elu::Material| textures.standard(images, standard, dir, m);
    model::spawn_elu(
        commands,
        meshes,
        inverse_bindposes,
        &scene.elu,
        &mut material,
        placement,
        |n| !n.name.starts_with("empty_cartridge"),
    )
}

fn spawn(
    mut commands: Commands,
    scene: Res<Scene>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut images: ResMut<Assets<Image>>,
    mut standard: ResMut<Assets<StandardMaterial>>,
    mut inverse_bindposes: ResMut<Assets<SkinnedMeshInverseBindposes>>,
    shot: Option<Res<Shot>>,
) {
    let mut textures = Textures::new(&scene.vfs, "model/");
    let Some(ch) = &scene.on else {
        spawn_weapon(
            &mut commands,
            &mut meshes,
            &mut inverse_bindposes,
            &mut images,
            &mut standard,
            &mut textures,
            &scene,
            Transform::IDENTITY,
        );
        let (min, max) = bounds(&scene.elu);
        let (center, size) = ((min + max) / 2.0 * SCALE, (max - min) * SCALE);
        // Look along the thinnest horizontal axis so the long profile faces the camera.
        let dir = if size.x < size.z {
            Vec3::NEG_X
        } else {
            Vec3::NEG_Z
        };
        let dist = size.max_element() + size.dot(dir.abs()) / 2.0;
        view::spawn_camera(
            &mut commands,
            &mut images,
            shot.is_some(),
            center - dir * dist,
            dir,
        );
        return;
    };
    let character = character::spawn(
        &mut commands,
        &mut meshes,
        &mut inverse_bindposes,
        &mut images,
        &mut standard,
        &mut textures,
        &scene.vfs,
        ch,
        &Outfit::base(),
        Transform::IDENTITY,
    )
    .unwrap_or_else(|e| panic!("character: {e}"));
    let mut target = Vec3::ZERO;
    for dummy in scene.kind.dummies() {
        let node = *character
            .nodes
            .get(*dummy)
            .unwrap_or_else(|| panic!("character has no node {dummy}"));
        let weapon = spawn_weapon(
            &mut commands,
            &mut meshes,
            &mut inverse_bindposes,
            &mut images,
            &mut standard,
            &mut textures,
            &scene,
            Transform::IDENTITY,
        );
        weapon.attach(&mut commands, node);
        target += character.bind[*dummy].w_axis.truncate() * SCALE;
    }
    target /= scene.kind.dummies().len() as f32;
    let mut offset = Vec3::new(-0.8, 0.2, 1.1);
    if let Some(idle) = &scene.idle {
        let mut animator = Animator::new(idle.clone(), Loop::Wrap);
        animator.speed = 0.0;
        commands.entity(character.root).insert(animator);
        // Hands are no longer at their bind positions: frame the upper body, the pose's
        // weapon arm reaches forward (+Z).
        (target, offset) = (Vec3::new(0.0, 1.2, 0.4), Vec3::new(-1.0, 0.1, 1.7));
    }
    view::spawn_camera(
        &mut commands,
        &mut images,
        shot.is_some(),
        target + offset,
        -offset,
    );
}

/// Bevy-space (cm) bounding box of the weapon's mesh nodes.
fn bounds(elu: &Elu) -> (Vec3, Vec3) {
    let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for n in elu
        .nodes
        .iter()
        .filter(|n| !n.name.starts_with("empty_cartridge"))
    {
        let m = model::bevy_matrix(&n.world);
        for p in &n.positions {
            let w = m.transform_point3(Vec3::new(p[0], p[1], -p[2]));
            (min, max) = (min.min(w), max.max(w));
        }
    }
    (min, max)
}
