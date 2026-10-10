//! Kill camera (`Settings::killcam`): when another actor kills the player, the camera flies to
//! the killer and orbits it (mouse turns it, the wheel zooms, the map keeps it out of walls)
//! while a world-anchored "<killer> killed YOU" tag floats over the killer's head on a
//! procedural blood splat, and a blood burst sprays from the corpse. It ends at the respawn,
//! or once spectating (round modes) has been up for [`KILLCAM_SECS`]. Everything is *inferred*
//! port design (`docs/formats.md`, "Kill camera"); the splat is drawn here, the blood is the
//! retail `sfx/blood0N.tga` sprites of `Vfx::Blood`.

use crate::{
    actor::{ActorSet, CAM_RADIUS},
    col::MapCollision,
    combat::{Vfx, rnd},
    game::{Dead, HitShape, Intent, Killed, Player, Settings, Spectate},
};
use bevy::{
    asset::RenderAssetUsages,
    input::mouse::AccumulatedMouseScroll,
    prelude::*,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
    ui::{GlobalZIndex, UiTargetCamera},
};
use std::f32::consts::TAU;

/// Seconds the camera keeps looking at the corpse (the blood burst) before it flies off.
const DWELL: f32 = 0.5;
/// Seconds the camera takes to fly from the player's view to the killer.
const FLY_SECS: f32 = 0.9;
/// Seconds of killcam before the round-mode spectator camera takes over.
const KILLCAM_SECS: f32 = 3.0;
/// Seconds for one turn of the camera around the killer on its own.
const ORBIT_SECS: f32 = 24.0;
/// Camera distance (m) from a human-sized killer: default and wheel limits; wheel metres per notch.
const DIST: f32 = 3.2;
const DIST_RANGE: (f32, f32) = (1.5, 9.0);
const ZOOM_STEP: f32 = 0.5;
/// Camera pitch over the killer when the mouse has not moved (looking slightly down).
const PITCH: f32 = -0.25;
/// Size of the tag on screen (pixels) and blood puffs of the burst.
const TAG: Vec2 = Vec2::new(260.0, 130.0);
const BURST: usize = 10;

#[derive(Resource)]
struct Splat(Handle<Image>);

#[derive(Component)]
struct Tag;

struct Cam {
    killer: Entity,
    tag: Entity,
    /// Seconds since the kill, and the camera it started from.
    t: f32,
    from: Transform,
    /// Camera yaw looking at the killer from where the victim stood.
    yaw: f32,
    /// The player's look angles at death: mouse movement since then orbits.
    look: Vec2,
    dist: f32,
    /// The player has been seen dead (the `Dead` insert lands a frame after `Killed`).
    seen: bool,
}

pub struct KillcamPlugin;

impl Plugin for KillcamPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, make_splat)
            .add_systems(Update, killcam.after(ActorSet::Camera));
    }
}

/// A dark-red blot: a chain of overlapping discs plus droplets around it, soft edge.
fn make_splat(mut commands: Commands, mut images: ResMut<Assets<Image>>) {
    const W: usize = 256;
    const H: usize = 128;
    let mut seed = 0x517C_C1B7_u32;
    let mut discs: Vec<(f32, f32, f32)> = (0..9)
        .map(|i| {
            let y = 64.0 + (rnd(&mut seed) - 0.5) * 24.0;
            (40.0 + i as f32 * 22.0, y, 26.0 + rnd(&mut seed) * 18.0)
        })
        .collect();
    discs.extend((0..16).map(|_| {
        let (x, y) = (rnd(&mut seed) * W as f32, rnd(&mut seed) * H as f32);
        (x, y, 3.0 + rnd(&mut seed) * 7.0)
    }));
    let data = (0..W * H)
        .flat_map(|i| {
            let (x, y) = ((i % W) as f32 + 0.5, (i / W) as f32 + 0.5);
            let a = discs
                .iter()
                .map(|&(cx, cy, r)| (r - (x - cx).hypot(y - cy)) / 2.5)
                .fold(0.0, f32::max)
                .clamp(0.0, 1.0);
            let k = 0.55 + 0.45 * a;
            [
                (150.0 * k) as u8,
                (6.0 * k) as u8,
                (8.0 * k) as u8,
                (a * 235.0) as u8,
            ]
        })
        .collect();
    commands.insert_resource(Splat(images.add(Image::new(
        Extent3d {
            width: W as u32,
            height: H as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ))));
}

/// Human actors are 1.8 m; quest monsters carry their own `HitShape`.
fn height(shape: Option<&HitShape>) -> f32 {
    shape.map_or(1.8, |s| s.height)
}

#[allow(clippy::too_many_arguments)]
fn killcam(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    spectate: Option<Res<Spectate>>,
    scroll: Res<AccumulatedMouseScroll>,
    col: Res<MapCollision>,
    splat: Res<Splat>,
    mut killed: MessageReader<Killed>,
    mut vfx: MessageWriter<Vfx>,
    player: Query<(Entity, &Intent, Has<Dead>), With<Player>>,
    actors: Query<(&GlobalTransform, Option<&HitShape>, Option<&Name>), Without<Camera3d>>,
    mut camera: Single<(Entity, &Camera, &mut Transform), With<Camera3d>>,
    mut tag: Query<(&mut Node, &mut Visibility), With<Tag>>,
    mut cam: Local<Option<Cam>>,
    mut seed: Local<u32>,
    ui: Res<UiScale>,
) {
    let Ok((me, intent, dead)) = player.single() else {
        return;
    };
    if *seed == 0 {
        *seed = 0x2F6E_2B1D;
    }
    for k in killed.read() {
        let (true, true, Ok((v, vs, _)), Ok((g, gs, name))) = (
            settings.killcam,
            k.victim == me && k.killer != me,
            actors.get(k.victim),
            actors.get(k.killer),
        ) else {
            continue;
        };
        if let Some(old) = cam.take() {
            commands.entity(old.tag).despawn();
        }
        let chest = v.translation() + Vec3::Y * height(vs) * 0.5;
        for _ in 0..BURST {
            let dir = Vec3::new(
                rnd(&mut seed) - 0.5,
                rnd(&mut seed) * 0.8 + 0.2,
                rnd(&mut seed) - 0.5,
            );
            vfx.write(Vfx::Blood {
                point: chest,
                dir: dir.normalize() * 1.5,
            });
        }
        let away = (v.translation() - g.translation()).with_y(0.0);
        let name = name.map_or("?", |n| n.as_str());
        let tag = commands
            .spawn((
                Tag,
                UiTargetCamera(camera.0),
                GlobalZIndex(3),
                Visibility::Hidden,
                Node {
                    position_type: PositionType::Absolute,
                    width: px(TAG.x),
                    height: px(TAG.y),
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    justify_content: JustifyContent::Center,
                    ..default()
                },
                ImageNode {
                    image_mode: NodeImageMode::Stretch,
                    ..ImageNode::new(splat.0.clone())
                },
                children![
                    (
                        Text::new(format!("{name} killed")),
                        TextFont::from_font_size(24.0),
                        TextColor(Color::WHITE),
                        TextShadow::default(),
                    ),
                    (
                        Text::new("YOU"),
                        TextFont::from_font_size(64.0),
                        TextColor(Color::srgb(1.0, 0.9, 0.2)),
                        TextShadow::default(),
                    ),
                ],
            ))
            .id();
        info!("killcam: {name} killed the player");
        *cam = Some(Cam {
            killer: k.killer,
            tag,
            t: 0.0,
            from: *camera.2,
            yaw: if away == Vec3::ZERO {
                intent.yaw
            } else {
                away.x.atan2(away.z)
            },
            look: Vec2::new(intent.yaw, intent.pitch),
            dist: DIST * (height(gs) / 1.8).max(1.0),
            seen: false,
        });
    }
    let Some(c) = cam.as_mut() else { return };
    c.seen |= dead;
    let over = (!dead && (c.seen || c.t > 0.5))
        || !settings.killcam
        || (c.t >= KILLCAM_SECS && spectate.is_some_and(|s| s.0.is_some()));
    let Ok((g, shape, _)) = actors.get(c.killer) else {
        commands.entity(c.tag).despawn();
        *cam = None;
        return;
    };
    if over {
        commands.entity(c.tag).despawn();
        *cam = None;
        return;
    }
    c.t += time.delta_secs();
    c.dist = (c.dist - scroll.delta.y * ZOOM_STEP).clamp(DIST_RANGE.0, DIST_RANGE.1);

    let h = height(shape);
    let focus = g.translation() + Vec3::Y * h * 0.5;
    let rot = Quat::from_euler(
        EulerRot::YXZ,
        c.yaw + intent.yaw - c.look.x + c.t * TAU / ORBIT_SECS,
        (PITCH + intent.pitch - c.look.y).clamp(-1.2, 0.6),
        0.0,
    );
    let back = rot * Vec3::Z;
    let dist = col
        .sweep_sphere(focus, focus + back * c.dist, CAM_RADIUS)
        .map_or(c.dist, |h| h.distance.max(0.3));
    // ponytail: the flight is a straight smoothstep blend (it may clip a wall); the orbit
    // proper is swept. Path-find the flight if it ever shows.
    let s = ((c.t - DWELL) / FLY_SECS).clamp(0.0, 1.0);
    let s = s * s * (3.0 - 2.0 * s);
    let to = Transform {
        translation: focus + back * dist,
        rotation: rot,
        ..*camera.2
    };
    camera.2.translation = c.from.translation.lerp(to.translation, s);
    camera.2.rotation = c.from.rotation.slerp(to.rotation, s);

    // pin the tag over the killer's head, as seen from the camera set just now
    let Ok((mut node, mut vis)) = tag.single_mut() else {
        return;
    };
    let view = GlobalTransform::from(*camera.2);
    let head = g.translation() + Vec3::Y * (h + 0.1);
    let at = camera
        .1
        .world_to_viewport(&view, head)
        .ok()
        .filter(|_| view.affine().inverse().transform_point3(head).z < 0.0);
    let want = if at.is_some() {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if *vis != want {
        *vis = want;
    }
    if let Some(p) = at.map(|p| p / ui.0) {
        node.left = px(p.x - TAG.x / 2.0);
        node.top = px((p.y - TAG.y + 20.0).max(4.0));
    }
}
