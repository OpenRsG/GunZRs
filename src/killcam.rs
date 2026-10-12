//! Kill camera (`Settings::killcam`): when another actor kills the player, the camera flies to
//! the killer and orbits it (mouse turns it, the wheel zooms, the map keeps it out of walls)
//! while a blood burst sprays from the corpse. It ends at the respawn, or once spectating (round
//! modes) has been up for [`KILLCAM_SECS`]. Everything is *inferred* port design
//! (`docs/formats.md`, "Kill camera"); the blood is `Vfx::Blood` (`gore.rs`, or the retail
//! `sfx/blood0N.tga` sprites with realistic blood off). The killer's name is on the death screen
//! (`hud/death.rs`).

use crate::{
    actor::{ActorSet, CAM_RADIUS},
    col::MapCollision,
    combat::{Vfx, rnd},
    game::{Dead, HitShape, Intent, Killed, Player, Settings, Spectate},
    phase::{MatchApp, MatchSystems},
};
use bevy::{input::mouse::AccumulatedMouseScroll, prelude::*};
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
/// Blood puffs of the burst.
const BURST: usize = 10;
/// Damage each puff of the burst stands for in the simulated blood (`gore.rs`).
const BURST_BLOOD: f32 = 30.0;

struct Cam {
    killer: Entity,
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

/// The kill camera in flight, if any.
#[derive(Resource, Default)]
struct Active(Option<Cam>);

pub struct KillcamPlugin;

impl Plugin for KillcamPlugin {
    fn build(&self, app: &mut App) {
        app.init_match_resource::<Active>()
            .add_systems(Update, killcam.after(ActorSet::Camera).in_set(MatchSystems));
    }
}

/// Human actors are 1.8 m; quest monsters carry their own `HitShape`.
fn height(shape: Option<&HitShape>) -> f32 {
    shape.map_or(1.8, |s| s.height)
}

#[allow(clippy::too_many_arguments)]
fn killcam(
    time: Res<Time>,
    settings: Res<Settings>,
    spectate: Option<Res<Spectate>>,
    scroll: Res<AccumulatedMouseScroll>,
    col: Res<MapCollision>,
    mut killed: MessageReader<Killed>,
    mut vfx: MessageWriter<Vfx>,
    player: Query<(Entity, &Intent, Has<Dead>), With<Player>>,
    actors: Query<(&GlobalTransform, Option<&HitShape>, Option<&Name>), Without<Camera3d>>,
    mut camera: Single<&mut Transform, With<Camera3d>>,
    mut cam: ResMut<Active>,
    mut seed: Local<u32>,
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
                amount: BURST_BLOOD,
            });
        }
        let away = (v.translation() - g.translation()).with_y(0.0);
        let name = name.map_or("?", |n| n.as_str());
        info!("killcam: {name} killed the player");
        cam.0 = Some(Cam {
            killer: k.killer,
            t: 0.0,
            from: **camera,
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
    let Some(c) = cam.0.as_mut() else { return };
    c.seen |= dead;
    let over = (!dead && (c.seen || c.t > 0.5))
        || !settings.killcam
        || (c.t >= KILLCAM_SECS && spectate.is_some_and(|s| s.0.is_some()));
    let Ok((g, shape, _)) = actors.get(c.killer) else {
        cam.0 = None;
        return;
    };
    if over {
        cam.0 = None;
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
        ..**camera
    };
    camera.translation = c.from.translation.lerp(to.translation, s);
    camera.rotation = c.from.rotation.slerp(to.rotation, s);
}
