//! `gunz-play` HUD (Bevy UI over the 3D view) and visual feedback: bullet-hole/blood decals,
//! damage-direction arcs, low-health vignette, explosion screen shake. HUD textures come from
//! `interface/default/*.png`, decals from `sfx/`. Sound lives in `audio.rs`. Format notes:
//! `docs/formats.md` (HUD and sound).

use crate::{
    actor::ActorData,
    col::MapCollision,
    game::{
        Blast, CameraShake, Damage, Dead, Impact, Killed, Loadout, Player, Score, Status, Team,
        Vitals,
    },
    level::Level,
    menu::Art,
    mrs::Vfs,
    profile::{MatchGain, Profile, Ranks},
    session::{Clock, HOLD},
    view::decode,
};
use bevy::{
    image::ImageSampler, prelude::*, text::Justify, transform::TransformSystems, ui::UiTargetCamera,
};
use std::collections::VecDeque;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Marks>()
            .init_resource::<Feed>()
            .init_resource::<Hurt>()
            .init_resource::<Shake>()
            .add_systems(
                Update,
                (
                    load.run_if(resource_exists::<Level>.and_then(resource_exists::<ActorData>))
                        .run_if(not(resource_exists::<Hud>)),
                    (
                        spawn_ui.run_if(not(any_with_component::<Root>)),
                        track,
                        update,
                        earned,
                        indicators,
                    )
                        .chain()
                        .run_if(resource_exists::<Hud>),
                    decals
                        .run_if(resource_exists::<Decals>)
                        .run_if(resource_exists::<MapCollision>),
                ),
            )
            // The shake edits the camera's global transform after propagation, so the camera
            // controller keeps writing its plain transform and nothing accumulates.
            .add_systems(PostUpdate, shake_camera.after(TransformSystems::Propagate));
    }
}

/// Immutable HUD data, loaded once the `Level` and `ActorData` (items) exist.
#[derive(Resource)]
struct Hud {
    cross: Handle<Image>,
    hit: Handle<Image>,
    kill: Handle<Image>,
    bars: Handle<Image>,
    clock: Handle<Image>,
    reload: Handle<Image>,
    empty: Handle<Image>,
    board: Handle<Image>,
}

/// Seconds left of the transient overlays.
#[derive(Resource, Default)]
struct Marks {
    hit: f32,
    kill: f32,
    flash: f32,
    /// Centre-screen notice ("You killed X") and its seconds left.
    notice: (f32, String),
}

/// Kill feed lines with their expiry (`Time::elapsed_secs`).
#[derive(Resource, Default)]
struct Feed(Vec<(f32, String)>);

/// Recent hits on the player: seconds left, the attacker and where the hit came from.
#[derive(Resource, Default)]
struct Hurt(Vec<(f32, Entity, Vec3)>);

/// Camera shake "trauma" in 0..=1 (offset and tilt grow with its square, decaying linearly).
#[derive(Resource, Default)]
struct Shake(f32);

/// Bullet-hole and blood-mark decals: unlit textured quads laid on the hit surface, the
/// oldest recycled past [`MAX_DECALS`].
#[derive(Resource)]
struct Decals {
    quad: Handle<Mesh>,
    bullet: Vec<Handle<StandardMaterial>>,
    blood: Vec<Handle<StandardMaterial>>,
    spawned: VecDeque<Entity>,
    seed: u32,
}

const FEED_LIFE: f32 = 6.0;
const FEED_LINES: usize = 6;
const BOARD_ROWS: usize = 12;
const MAX_DECALS: usize = 128;
/// Bullet hole size (m) and blood-mark size range (m) (**inferred**).
const HOLE_SIZE: f32 = 0.22;
const BLOOD_SIZE: (f32, f32) = (0.5, 0.9);
/// How far behind a hit actor a wall still gets a blood mark (m).
const BLOOD_REACH: f32 = 2.5;
/// Distance from the surface a decal floats at (m), against z-fighting.
const DECAL_LIFT: f32 = 0.006;
/// Seconds a damage-direction arc stays up, and how many can show at once.
const HURT_LIFE: f32 = 1.5;
const HURT_SLOTS: usize = 3;
/// Below this HP fraction the screen edges turn red (**inferred**).
const LOW_HP: f32 = 0.3;
/// Shake trauma lost per second; trauma of a blast at the camera's feet; blast range (m).
const SHAKE_DECAY: f32 = 1.6;
const SHAKE_BLAST: f32 = 0.7;
const SHAKE_RANGE: f32 = 18.0;
/// Largest shake offset (m) and tilt (rad) at trauma 1: kept gentle.
const SHAKE_OFFSET: f32 = 0.08;
const SHAKE_TILT: f32 = 0.012;

/// `interface/default/<name>` (file name with extension) as sRGB UI art; `None` if absent.
pub fn try_image(vfs: &Vfs, images: &mut Assets<Image>, name: &str) -> Option<Handle<Image>> {
    let bytes = vfs.read(&format!("interface/default/{name}")).ok()?;
    let img = decode(
        &bytes,
        name.rsplit('.').next()?,
        true,
        ImageSampler::linear(),
    )?;
    Some(images.add(img))
}

fn image(vfs: &Vfs, images: &mut Assets<Image>, name: &str) -> Handle<Image> {
    try_image(vfs, images, &format!("{name}.png"))
        .unwrap_or_else(|| panic!("interface/default/{name}.png missing or undecodable"))
}

/// `sfx/<name>` (tga/bmp) as sRGB art; the bullet-hole and blood-mark decal textures.
fn sfx_image(vfs: &Vfs, images: &mut Assets<Image>, name: &str) -> Option<Handle<Image>> {
    let bytes = vfs.read(&format!("sfx/{name}")).ok()?;
    let img = decode(
        &bytes,
        name.rsplit('.').next()?,
        true,
        ImageSampler::linear(),
    )?;
    Some(images.add(img))
}

fn load(
    mut commands: Commands,
    level: Res<Level>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let vfs = &level.vfs;
    let mut decal = |names: Vec<String>| -> Vec<Handle<StandardMaterial>> {
        names
            .iter()
            .map(|n| {
                let tex = sfx_image(vfs, &mut images, n)
                    .unwrap_or_else(|| panic!("sfx/{n} missing or undecodable"));
                materials.add(StandardMaterial {
                    base_color_texture: Some(tex),
                    unlit: true,
                    alpha_mode: AlphaMode::Blend,
                    cull_mode: None,
                    depth_bias: 2.0,
                    ..default()
                })
            })
            .collect()
    };
    commands.insert_resource(Decals {
        quad: meshes.add(Rectangle::new(1.0, 1.0)),
        bullet: decal(
            (1..=2)
                .map(|i| format!("gz_sfx_shotgun_bulletmark0{i}.tga"))
                .collect(),
        ),
        blood: decal((1..=5).map(|i| format!("blood-mark0{i}.tga")).collect()),
        spawned: VecDeque::new(),
        seed: 0x2545_F491,
    });
    commands.insert_resource(Hud {
        cross: image(vfs, &mut images, "crosshair02"),
        hit: image(vfs, &mut images, "hit_marker"),
        kill: image(vfs, &mut images, "kill_marker"),
        bars: image(vfs, &mut images, "ingame_hpbar"),
        clock: image(vfs, &mut images, "ingame_timebackground"),
        reload: image(vfs, &mut images, "ingame_reload"),
        empty: image(vfs, &mut images, "ingame_empty"),
        board: image(vfs, &mut images, "scoreboard_background_solo"),
    });
    commands.insert_resource(Art::load(vfs, &mut images));
}

#[derive(Component)]
struct Root;

#[derive(Component)]
struct Flash;

/// Red screen-edge vignette shown at low health.
#[derive(Component)]
struct LowHp;

/// Damage-direction arc `i` (index into [`Hurt`]).
#[derive(Component)]
struct Indicator(usize);

/// Text slots rewritten every frame.
#[derive(Component, Clone, Copy)]
enum Label {
    Hp,
    Ap,
    Ammo,
    Weapon,
    Feed,
    Death,
    Score,
    Names,
    Kills,
    Deaths,
    Clock,
    /// Centre-screen kill notice.
    Notice,
    /// Slow / stun / root / burn timers of the player (`Status`).
    Status,
}

/// Bars whose width is a percentage.
#[derive(Component, Clone, Copy)]
enum Fill {
    Hp,
    /// Segment 0..3 of the armour bar.
    Ap(usize),
}

/// Elements shown only in some states.
#[derive(Component, Clone, Copy)]
enum Show {
    Cross,
    Hit,
    Kill,
    Reload,
    Empty,
    Death,
    Board,
}

const WHITE: Color = Color::WHITE;

fn label(slot: Label, size: f32, color: Color) -> impl Bundle {
    (
        slot,
        Text::new(""),
        TextFont::from_font_size(size),
        TextColor(color),
    )
}

fn centered(w: f32, h: f32) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: percent(50),
        top: percent(50),
        margin: UiRect {
            left: px(-w / 2.0),
            top: px(-h / 2.0),
            ..default()
        },
        width: px(w),
        height: px(h),
        ..default()
    }
}

fn overlay(color: Color) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(color),
    )
}

/// Retail HP/AP bars (`combatinterface.xml`: `CombatHPBG`/`CombatHPProgressBar`): the
/// 366x26 `ingame_hpbar.png` frame with the fill inset 3 px (360x20); armour is three 118 px
/// segments. Fill colours are the retail gradients.
fn seg(fill: Fill, x: f32, w: f32, g: ([u8; 3], [u8; 3])) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            left: px(x),
            top: px(3),
            width: px(w),
            height: px(20),
            ..default()
        },
        children![(
            fill,
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            gradient(g),
        )],
    )
}

fn gradient((a, b): ([u8; 3], [u8; 3])) -> BackgroundGradient {
    let c = |[r, g, b]: [u8; 3]| Color::srgb_u8(r, g, b);
    LinearGradient::to_right(vec![
        ColorStop::new(c(a), percent(0)),
        ColorStop::new(c(b), percent(100)),
    ])
    .into()
}

/// Transparent centre fading to red at the screen edge (`alpha` there).
fn vignette(alpha: f32) -> BackgroundGradient {
    RadialGradient::new(
        UiPosition::CENTER,
        RadialGradientShape::default(),
        vec![
            ColorStop::new(Color::srgba(0.75, 0.0, 0.0, 0.0), percent(45)),
            ColorStop::new(Color::srgba(0.75, 0.0, 0.0, alpha), percent(100)),
        ],
    )
    .into()
}

/// HP fill colours by remaining fraction; the tier thresholds are inferred (retail lists four
/// colours `INDEX 0..3` white, yellow, orange, red without the rule that picks one).
const HP_TIERS: [([u8; 3], [u8; 3]); 4] = [
    ([212, 212, 212], [230, 230, 230]),
    ([232, 190, 58], [255, 235, 60]),
    ([232, 128, 58], [255, 179, 60]),
    ([207, 60, 56], [216, 81, 29]),
];
const AP_SEGS: [([u8; 3], [u8; 3]); 3] = [
    ([23, 87, 125], [29, 95, 139]),
    ([30, 97, 141], [39, 109, 159]),
    ([40, 111, 162], [47, 119, 175]),
];

fn tier(frac: f32) -> usize {
    match frac {
        f if f >= 0.75 => 0,
        f if f >= 0.5 => 1,
        f if f >= 0.25 => 2,
        _ => 3,
    }
}

/// UI art drawn stretched over its node (the default mode keeps the image's own size).
fn stretch(image: &Handle<Image>) -> ImageNode {
    ImageNode {
        image_mode: NodeImageMode::Stretch,
        ..ImageNode::new(image.clone())
    }
}

fn bar(name: &str, frame: &Handle<Image>, value: Label, fills: impl Bundle) -> impl Bundle {
    (
        Node {
            align_items: AlignItems::Center,
            column_gap: px(8),
            ..default()
        },
        children![
            (
                Text::new(name),
                TextFont::from_font_size(18.0),
                TextColor(WHITE),
                Node {
                    width: px(30),
                    ..default()
                }
            ),
            (
                Node {
                    width: px(366),
                    height: px(26),
                    ..default()
                },
                stretch(frame),
                fills,
            ),
            (
                label(value, 20.0, WHITE),
                Node {
                    width: px(44),
                    ..default()
                }
            ),
        ],
    )
}

fn spawn_ui(mut commands: Commands, hud: Res<Hud>, cameras: Query<Entity, With<Camera3d>>) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    // Sounds of other actors are positioned relative to the view.
    commands.entity(camera).insert(SpatialListener::new(0.2));
    let panel = Color::srgba(0.0, 0.0, 0.0, 0.45);
    let root = commands
        .spawn((
            Root,
            UiTargetCamera(camera),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            children![
                (
                    Show::Cross,
                    centered(32.0, 32.0),
                    ImageNode::new(hud.cross.clone())
                ),
                (
                    Show::Hit,
                    Visibility::Hidden,
                    centered(75.0, 71.0),
                    ImageNode::new(hud.hit.clone())
                ),
                (
                    Show::Kill,
                    Visibility::Hidden,
                    centered(29.0, 29.0),
                    ImageNode::new(hud.kill.clone())
                ),
                (Flash, overlay(Color::srgba(0.8, 0.0, 0.0, 0.0))),
                (
                    LowHp,
                    Visibility::Hidden,
                    overlay(Color::NONE),
                    vignette(0.0)
                ),
                (
                    Node {
                        top: percent(62),
                        justify_content: JustifyContent::Center,
                        ..centered(400.0, 30.0)
                    },
                    children![label(Label::Notice, 24.0, Color::srgb(1.0, 0.85, 0.3))],
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(24),
                        bottom: px(110),
                        ..default()
                    },
                    children![label(Label::Status, 22.0, Color::srgb(0.45, 0.8, 1.0))],
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(24),
                        bottom: px(24),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(6),
                        padding: UiRect::all(px(8)),
                        ..default()
                    },
                    BackgroundColor(panel),
                    children![
                        bar(
                            "HP",
                            &hud.bars,
                            Label::Hp,
                            children![seg(Fill::Hp, 3.0, 360.0, HP_TIERS[0])]
                        ),
                        bar(
                            "AP",
                            &hud.bars,
                            Label::Ap,
                            children![
                                seg(Fill::Ap(0), 3.0, 118.0, AP_SEGS[0]),
                                seg(Fill::Ap(1), 124.0, 118.0, AP_SEGS[1]),
                                seg(Fill::Ap(2), 245.0, 118.0, AP_SEGS[2]),
                            ]
                        ),
                    ],
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: percent(50),
                        margin: UiRect::left(px(-230)),
                        top: px(8),
                        width: px(460),
                        height: px(40),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    stretch(&hud.clock),
                    children![label(Label::Clock, 22.0, WHITE)],
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        right: px(24),
                        bottom: px(24),
                        flex_direction: FlexDirection::Column,
                        align_items: AlignItems::End,
                        padding: UiRect::all(px(8)),
                        ..default()
                    },
                    BackgroundColor(panel),
                    children![
                        label(Label::Weapon, 22.0, WHITE),
                        (
                            Node {
                                align_items: AlignItems::Center,
                                column_gap: px(12),
                                ..default()
                            },
                            children![
                                (
                                    Show::Reload,
                                    Visibility::Hidden,
                                    ImageNode::new(hud.reload.clone())
                                ),
                                (
                                    Show::Empty,
                                    Visibility::Hidden,
                                    ImageNode::new(hud.empty.clone())
                                ),
                                label(Label::Ammo, 44.0, WHITE),
                            ],
                        ),
                    ],
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        right: px(16),
                        top: px(16),
                        ..default()
                    },
                    children![(
                        label(Label::Feed, 18.0, WHITE),
                        TextLayout {
                            justify: Justify::Right,
                            ..default()
                        },
                    )],
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(16),
                        top: px(16),
                        ..default()
                    },
                    children![label(Label::Score, 20.0, WHITE)],
                ),
                (
                    Show::Death,
                    Visibility::Hidden,
                    overlay(Color::srgba(0.3, 0.0, 0.0, 0.55)),
                    children![label(Label::Death, 40.0, WHITE)]
                ),
                (
                    Show::Board,
                    Visibility::Hidden,
                    Node {
                        padding: UiRect::all(px(24)),
                        flex_direction: FlexDirection::Column,
                        row_gap: px(8),
                        ..centered(720.0, 440.0)
                    },
                    children![
                        (
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(0),
                                top: px(0),
                                width: percent(100),
                                height: percent(100),
                                ..default()
                            },
                            stretch(&hud.board),
                        ),
                        (
                            Text::new("SCOREBOARD"),
                            TextFont::from_font_size(26.0),
                            TextColor(WHITE)
                        ),
                        (
                            Node {
                                column_gap: px(8),
                                ..default()
                            },
                            children![
                                (
                                    label(Label::Names, 20.0, WHITE),
                                    Node {
                                        width: px(360),
                                        ..default()
                                    }
                                ),
                                (
                                    label(Label::Kills, 20.0, WHITE),
                                    Node {
                                        width: px(100),
                                        ..default()
                                    }
                                ),
                                (
                                    label(Label::Deaths, 20.0, WHITE),
                                    Node {
                                        width: px(100),
                                        ..default()
                                    }
                                ),
                            ],
                        ),
                        (
                            Earned,
                            Text::new(""),
                            TextFont::from_font_size(20.0),
                            TextColor(Color::srgb(1.0, 0.85, 0.3)),
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(24),
                                bottom: px(16),
                                ..default()
                            }
                        ),
                    ],
                ),
            ],
        ))
        .id();
    for i in 0..HURT_SLOTS {
        commands.spawn((
            Indicator(i),
            Visibility::Hidden,
            UiTransform::default(),
            ChildOf(root),
            centered(300.0, 300.0),
            children![(
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(50),
                    margin: UiRect::left(px(-8)),
                    top: px(0),
                    width: px(16),
                    height: px(56),
                    border_radius: BorderRadius::all(px(8)),
                    ..default()
                },
                BackgroundColor(Color::NONE),
            )],
        ));
    }
}

/// Folds this frame's messages into the transient overlays and the kill feed.
#[allow(clippy::too_many_arguments)]
fn track(
    time: Res<Time>,
    real: Res<Time<Real>>,
    data: Res<ActorData>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    mut blasts: MessageReader<Blast>,
    mut shakes: MessageReader<CameraShake>,
    players: Query<&GlobalTransform, With<Player>>,
    transforms: Query<&GlobalTransform>,
    names: Query<&Name>,
    mut marks: ResMut<Marks>,
    mut feed: ResMut<Feed>,
    mut hurt: ResMut<Hurt>,
    mut shake: ResMut<Shake>,
) {
    // Overlays fade on real time so a pause does not freeze a red flash on screen.
    let dt = real.delta_secs();
    marks.hit = (marks.hit - dt).max(0.0);
    marks.kill = (marks.kill - dt).max(0.0);
    marks.flash = (marks.flash - dt).max(0.0);
    marks.notice.0 = (marks.notice.0 - dt).max(0.0);
    shake.0 = (shake.0 - SHAKE_DECAY * dt).max(0.0);
    for h in &mut hurt.0 {
        h.0 -= dt;
    }
    hurt.0.retain(|h| h.0 > 0.0);
    for d in damage.read() {
        if players.contains(d.attacker) && d.attacker != d.target {
            marks.hit = 0.2;
        }
        if players.contains(d.target) {
            marks.flash = 0.4;
            if d.attacker != d.target {
                let from = transforms
                    .get(d.attacker)
                    .map_or(d.point - d.dir * 3.0, |t| t.translation());
                if let Some(h) = hurt.0.iter_mut().find(|h| h.1 == d.attacker) {
                    (h.0, h.2) = (HURT_LIFE, from);
                } else {
                    if hurt.0.len() >= HURT_SLOTS {
                        hurt.0.remove(0);
                    }
                    hurt.0.push((HURT_LIFE, d.attacker, from));
                }
            }
        }
    }
    for b in blasts.read() {
        if let Ok(p) = players.single() {
            let near = (1.0 - p.translation().distance(b.at) / SHAKE_RANGE).clamp(0.0, 1.0);
            shake.0 = (shake.0 + SHAKE_BLAST * near * near).min(1.0);
        }
    }
    for s in shakes.read() {
        if let Ok(p) = players.single() {
            let near = (1.0 - p.translation().distance(s.at) / s.range.max(0.1)).clamp(0.0, 1.0);
            shake.0 = shake.0.max(s.trauma * near);
        }
    }
    let now = time.elapsed_secs();
    for k in killed.read() {
        let name = |e| names.get(e).map_or("?", |n| n.as_str());
        let line = if k.killer == k.victim {
            format!("{} suicide", name(k.victim))
        } else {
            let weapon = data
                .items
                .get(k.item)
                .and_then(|i| i.name.as_deref())
                .unwrap_or("?");
            format!("{} [{weapon}] {}", name(k.killer), name(k.victim))
        };
        feed.0.push((now + FEED_LIFE, line));
        if players.contains(k.killer) && k.killer != k.victim {
            marks.kill = 0.8;
            marks.notice = (2.0, format!("You killed {}", name(k.victim)));
        }
    }
    feed.0.retain(|(until, _)| *until > now);
    let extra = feed.0.len().saturating_sub(FEED_LINES);
    feed.0.drain(..extra);
}

/// The red arcs around the crosshair pointing at whoever hurt the player, relative to the
/// camera's heading (0 = ahead, clockwise).
fn indicators(
    hurt: Res<Hurt>,
    transforms: Query<&GlobalTransform>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut arcs: Query<(&Indicator, &mut UiTransform, &mut Visibility, &Children)>,
    mut bars: Query<&mut BackgroundColor>,
) {
    let Ok(cam) = camera.single() else {
        return;
    };
    let (fwd, right) = (cam.forward().xz(), cam.right().xz());
    for (Indicator(i), mut t, mut v, kids) in &mut arcs {
        let Some((left, attacker, from)) = hurt.0.get(*i) else {
            if *v != Visibility::Hidden {
                *v = Visibility::Hidden;
            }
            continue;
        };
        let from = transforms.get(*attacker).map_or(*from, |t| t.translation());
        let to = (from - cam.translation()).xz();
        t.rotation = Rot2::radians(to.dot(right).atan2(to.dot(fwd)));
        *v = Visibility::Inherited;
        for k in kids {
            if let Ok(mut c) = bars.get_mut(*k) {
                c.0 = Color::srgba(0.9, 0.05, 0.05, (left / HURT_LIFE).min(1.0) * 0.85);
            }
        }
    }
}

fn rand(seed: &mut u32) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 17;
    *seed ^= *seed << 5;
    (*seed >> 8) as f32 / (1u32 << 24) as f32
}

/// Bullet holes where `Impact`s hit the map, blood marks on the wall or floor behind a hit
/// actor (a ray along the hit direction).
fn decals(
    mut commands: Commands,
    mut decals: ResMut<Decals>,
    col: Res<MapCollision>,
    mut impacts: MessageReader<Impact>,
    mut damage: MessageReader<Damage>,
) {
    let dec = &mut *decals;
    let mut marks: Vec<(Vec3, Vec3, f32, bool)> = Vec::new();
    for i in impacts.read().filter(|i| !i.blade) {
        marks.push((i.point, i.normal, HOLE_SIZE, false));
    }
    let mut bled: Vec<Entity> = Vec::new();
    for d in damage.read() {
        if bled.contains(&d.target) {
            continue;
        }
        bled.push(d.target);
        if let Some(h) = col.raycast(d.point, d.dir, BLOOD_REACH) {
            let size = BLOOD_SIZE.0 + rand(&mut dec.seed) * (BLOOD_SIZE.1 - BLOOD_SIZE.0);
            marks.push((h.point, h.normal, size, true));
        }
    }
    for (at, normal, size, blood) in marks {
        let set = if blood { &dec.blood } else { &dec.bullet };
        let material = set[(rand(&mut dec.seed) * set.len() as f32) as usize % set.len()].clone();
        let roll = rand(&mut dec.seed) * std::f32::consts::TAU;
        let e = commands
            .spawn((
                crate::game::MapEntity,
                Mesh3d(dec.quad.clone()),
                MeshMaterial3d(material),
                Transform {
                    translation: at + normal * DECAL_LIFT,
                    rotation: Quat::from_rotation_arc(Vec3::Z, normal)
                        * Quat::from_rotation_z(roll),
                    scale: Vec3::splat(size),
                },
            ))
            .id();
        dec.spawned.push_back(e);
        if dec.spawned.len() > MAX_DECALS
            && let Some(old) = dec.spawned.pop_front()
        {
            commands.entity(old).despawn();
        }
    }
}

/// Explosion shake: a short offset and tilt of the camera as rendered (see the plugin).
fn shake_camera(
    shake: Res<Shake>,
    time: Res<Time<Real>>,
    mut camera: Query<(&Transform, &mut GlobalTransform), With<Camera3d>>,
) {
    if shake.0 <= 0.0 {
        return;
    }
    let (a, t) = (shake.0 * shake.0, time.elapsed_secs());
    let wobble = |f: f32, p: f32| (t * f + p).sin();
    for (tf, mut g) in &mut camera {
        let local = Transform {
            translation: Vec3::new(wobble(37.0, 0.0), wobble(41.0, 1.3), 0.0) * SHAKE_OFFSET * a,
            rotation: Quat::from_euler(
                EulerRot::XYZ,
                wobble(29.0, 2.1) * SHAKE_TILT * a,
                wobble(31.0, 0.7) * SHAKE_TILT * a,
                wobble(23.0, 4.0) * SHAKE_TILT * a,
            ),
            ..default()
        };
        *g = GlobalTransform::from(*tf) * local;
    }
}

fn set(text: &mut Text, s: String) {
    if text.0 != s {
        text.0 = s;
    }
}

fn update(
    data: Res<ActorData>,
    marks: Res<Marks>,
    feed: Res<Feed>,
    keys: Res<ButtonInput<KeyCode>>,
    clock: Option<Res<Clock>>,
    player: Query<(&Vitals, &Loadout, &Score, Option<&Dead>, Option<&Status>), With<Player>>,
    actors: Query<(&Name, &Score, Has<Player>, Option<&Team>)>,
    mut texts: Query<(&Label, &mut Text)>,
    mut fills: Query<(&Fill, &mut Node, &mut BackgroundGradient)>,
    mut shows: Query<(&Show, &mut Visibility)>,
    mut flash: Query<&mut BackgroundColor, With<Flash>>,
    mut hp_tier: Local<usize>,
    mut low: Query<
        (&mut BackgroundGradient, &mut Visibility),
        (With<LowHp>, Without<Fill>, Without<Show>),
    >,
    real: Res<Time<Real>>,
    war: Option<Res<crate::clan::ClanWar>>,
) {
    let Ok((vitals, loadout, score, dead, status)) = player.single() else {
        return;
    };
    let slot = &loadout.slots[loadout.current];
    let item = data.items.get(slot.item);
    let melee = item.is_none_or(|i| i.kind == "melee");
    let ranged_empty = !melee && slot.magazine == 0;
    let mut rows: Vec<_> = actors.iter().collect();
    rows.sort_by_key(|(_, s, _, _)| (std::cmp::Reverse(s.kills), s.deaths));
    let column = |head: &str, f: &dyn Fn(&(&Name, &Score, bool, Option<&Team>)) -> String| {
        let lines = rows.iter().take(BOARD_ROWS).map(f);
        std::iter::once(head.to_owned())
            .chain(lines)
            .collect::<Vec<_>>()
            .join("\n")
    };
    for (l, mut t) in &mut texts {
        let s = match l {
            Label::Hp => format!("{}", vitals.hp.ceil()),
            Label::Ap => format!("{}", vitals.ap.ceil()),
            Label::Ammo if melee => String::new(),
            Label::Ammo => format!("{}/{}", slot.magazine, slot.reserve),
            Label::Weapon => item
                .and_then(|i| i.name.clone())
                .unwrap_or_else(|| format!("item {}", slot.item)),
            // a clan war draws its own feed with emblems (`clan.rs`)
            Label::Feed if war.is_some() => String::new(),
            Label::Feed => feed
                .0
                .iter()
                .map(|(_, s)| s.as_str())
                .collect::<Vec<_>>()
                .join("\n"),
            Label::Death => dead.map_or(String::new(), |d| {
                if d.respawn >= HOLD {
                    "You died - waiting to rejoin".to_owned()
                } else {
                    format!("You died - respawn in {}", d.respawn.ceil())
                }
            }),
            Label::Score => format!("Kills {}   Deaths {}", score.kills, score.deaths),
            Label::Notice if marks.notice.0 > 0.0 => marks.notice.1.clone(),
            Label::Notice => String::new(),
            Label::Status => status.map_or_else(String::new, |s| {
                let mut v = Vec::new();
                if s.stun > 0.0 {
                    v.push(format!("STUNNED {:.1}s", s.stun));
                }
                if s.root > 0.0 {
                    v.push(format!("ROOTED {:.1}s", s.root));
                }
                if s.slow_left > 0.0 {
                    v.push(format!("SLOWED {:.0}% {:.1}s", s.slow * 100.0, s.slow_left));
                }
                if s.dot_left > 0.0 {
                    v.push(format!("BURNING {:.0}/s {:.1}s", s.dot, s.dot_left));
                }
                v.join("\n")
            }),
            Label::Names => column("Name", &|(n, _, you, team)| {
                let tag = match team {
                    Some(Team::Red) => " [RED]",
                    Some(Team::Blue) => " [BLUE]",
                    None | Some(Team::Duel(_)) => "",
                };
                format!("{n}{}{tag}", if *you { " (you)" } else { "" })
            }),
            Label::Kills => column("Kills", &|(_, s, _, _)| s.kills.to_string()),
            Label::Deaths => column("Deaths", &|(_, s, _, _)| s.deaths.to_string()),
            Label::Clock => clock
                .as_ref()
                .map_or_else(String::new, |c| c.header.clone()),
        };
        set(&mut t, s);
    }
    for (f, mut n, mut g) in &mut fills {
        let (v, max) = match *f {
            Fill::Hp => (vitals.hp, vitals.max_hp),
            Fill::Ap(i) => {
                let seg = vitals.max_ap / 3.0;
                (vitals.ap - seg * i as f32, seg)
            }
        };
        n.width = percent((v / max.max(1.0)).clamp(0.0, 1.0) * 100.0);
        if matches!(f, Fill::Hp) {
            let t = tier(vitals.hp / vitals.max_hp.max(1.0));
            if t != *hp_tier {
                *hp_tier = t;
                *g = gradient(HP_TIERS[t]);
            }
        }
    }
    for (s, mut v) in &mut shows {
        let on = match s {
            Show::Cross => dead.is_none(),
            Show::Hit => marks.hit > 0.0,
            Show::Kill => marks.kill > 0.0,
            Show::Reload => ranged_empty && slot.reserve > 0,
            Show::Empty => ranged_empty && slot.reserve == 0,
            Show::Death => dead.is_some() && clock.as_ref().is_none_or(|c| c.over.is_none()),
            Show::Board => {
                keys.pressed(KeyCode::Tab) || clock.as_ref().is_some_and(|c| c.over.is_some())
            }
        };
        let want = if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
    }
    for mut c in &mut flash {
        c.0 = Color::srgba(0.8, 0.0, 0.0, marks.flash / 0.4 * 0.22);
    }
    // Low health: red screen edges that pulse faster the lower the health.
    let frac = vitals.hp / vitals.max_hp.max(1.0);
    let level = if dead.is_some() || frac >= LOW_HP {
        0.0
    } else {
        1.0 - frac / LOW_HP
    };
    for (mut g, mut v) in &mut low {
        let want = if level > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
        if level > 0.0 {
            let pulse = 0.7 + 0.3 * (real.elapsed_secs() * (3.0 + 4.0 * level)).sin();
            *g = vignette(0.65 * level * pulse);
        }
    }
}

/// The scoreboard's last line once the match is over: level, XP and bounty earned (`profile.rs`).
#[derive(Component)]
struct Earned;

fn earned(
    clock: Res<Clock>,
    gain: Res<MatchGain>,
    profile: Res<Profile>,
    ranks: Option<Res<Ranks>>,
    text: Single<&mut Text, With<Earned>>,
) {
    let level = profile.level();
    let s = match clock.over {
        None => String::new(),
        Some(_) => format!(
            "{}   +{} XP   +{} bounty   (bounty {})",
            if level > gain.from_level {
                format!("LEVEL UP {} -> {level}", gain.from_level)
            } else {
                format!(
                    "Level {level} [{}]",
                    ranks.as_ref().map_or("", |r| r.code(level))
                )
            },
            gain.xp,
            gain.bounty,
            profile.bounty
        ),
    };
    set(&mut text.into_inner(), s);
}
