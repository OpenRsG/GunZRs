//! Crosshair and hit feedback at the screen centre: the gap opens with running, jumping and every
//! shot of a gun, a hit X (red and larger on a kill) pops on every hit the player deals, damage
//! numbers drift up from it, and arcs on a ring point at whoever hurt the player. All of it hides
//! while dead. A child of `hud.rs`; the nodes are spawned once and only rewritten afterwards.
//! Sizes and timings are the HUD mock's; the mock's spread rule (+5 per shot up to 14, back to 0
//! after 0.18 s) is driven by the player's own [`Fire`] messages.

use super::{HURT_SLOTS, Hud, Hurt, Root, rand};
use crate::{
    actor::ActorData,
    game::{Damage, Dead, Fire, Killed, Loadout, Player, Settings},
    phase::{MatchApp, MatchSystems},
    ui::{self, DANGER, SHADOW},
};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.init_match_resource::<Reticle>().add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<Cross>))),
            (update, indicators).run_if(resource_exists::<Hud>),
        )
            .in_set(MatchSystems),
    );
}

/// Arms start this far (px) from the centre at rest; every shot adds up to [`SPREAD_MAX`].
const GAP: f32 = 7.0;
const ARM: f32 = 9.0;
const SPREAD_STEP: f32 = 5.0;
const SPREAD_MAX: f32 = 14.0;
const SPREAD_RELAX: f32 = 0.18;
/// Seconds the hit X and the kill X stay up, and how many damage numbers can be alive.
const HIT_LIFE: f32 = 0.3;
const KILL_LIFE: f32 = 0.5;
const NUMS: usize = 6;
const NUM_LIFE: f32 = 0.75;
/// A gun hit this high above a human's feet counts as a headshot (`combat.rs`'s rule).
const HEAD_FROM: f32 = 1.5;
/// The damage-direction arc: segments, the angle between them (degrees) and the ring radius (px).
const ARC: usize = 9;
const ARC_STEP: f32 = 9.0;
const RING: f32 = 140.0;

/// The zero-size node at the screen centre holding the crosshair, hit X's and numbers.
#[derive(Component)]
struct Cross;

/// Crosshair arm `0..4` (up, right, down, left).
#[derive(Component)]
struct Arm(usize);

/// A hit X (`false`) or kill X (`true`); its parent scales.
#[derive(Component)]
struct Burst(bool);

/// One of the four bars of a [`Burst`].
#[derive(Component)]
struct Spoke(bool);

/// Damage number node `i` (index into [`Reticle::pops`]).
#[derive(Component)]
struct Num(usize);

/// Damage-direction arc `i` (index into [`Hurt`]).
#[derive(Component)]
struct Indicator(usize);

struct Pop {
    /// 0 at the hit, 1 when gone.
    age: f32,
    /// Horizontal drift (px) at the end.
    dx: f32,
    head: bool,
    amount: i32,
    /// Text and font not written to the node yet.
    fresh: bool,
}

#[derive(Resource, Default)]
struct Reticle {
    spread: f32,
    /// Seconds since the last shot.
    idle: f32,
    /// The player's last position and smoothed running fraction (0..1): the gap opens while
    /// running (+7 px at 10 m/s) and jumping or falling (+4 px at 3 m/s vertical).
    at: Vec3,
    run: f32,
    /// Seconds left of the hit and kill X.
    hit: f32,
    kill: f32,
    pops: [Option<Pop>; NUMS],
    seed: u32,
}

/// A zero-size node at the centre of its parent (the HUD root); children are placed around it.
fn origin() -> impl Bundle {
    Node {
        position_type: PositionType::Absolute,
        left: percent(50),
        top: percent(50),
        ..default()
    }
}

/// A white bar at (`left`, `top`) from the centre with the mock's 1 px dark outline.
fn bar(left: f32, top: f32, w: f32, h: f32) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            left: px(left),
            top: px(top),
            width: px(w),
            height: px(h),
            ..default()
        },
        BackgroundColor(Color::WHITE),
        BoxShadow::new(SHADOW.with_alpha(0.5), px(0), px(0), px(1), px(0)),
    )
}

fn spawn(mut commands: Commands, root: Query<Entity, With<Root>>) {
    let Ok(root) = root.single() else {
        return;
    };
    commands
        .spawn((Cross, origin(), ChildOf(root)))
        .with_children(|c| {
            c.spawn(bar(-1.0, -1.0, 2.0, 2.0));
            for i in 0..4 {
                let (w, h) = if i % 2 == 0 { (2.0, ARM) } else { (ARM, 2.0) };
                c.spawn((Arm(i), bar(0.0, 0.0, w, h)));
            }
            // The mock's X: two diagonal bars with the middle 6 px masked out, so four spokes
            // 2 x 10.5 px reaching from radius 6.5 to the corner of its 24 px box.
            for kill in [false, true] {
                c.spawn((
                    Burst(kill),
                    UiTransform::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        ..default()
                    },
                ))
                .with_children(|b| {
                    for (sx, sy) in [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0f32)] {
                        let m = 11.75 * std::f32::consts::FRAC_1_SQRT_2;
                        b.spawn((
                            Spoke(kill),
                            UiTransform::from_rotation(Rot2::degrees(if sx * sy < 0.0 {
                                45.0
                            } else {
                                -45.0
                            })),
                            Node {
                                position_type: PositionType::Absolute,
                                left: px(sx * m - 1.0),
                                top: px(sy * m - 5.25),
                                width: px(2),
                                height: px(10.5),
                                ..default()
                            },
                            BackgroundColor(Color::NONE),
                        ));
                    }
                });
            }
            for i in 0..NUMS {
                c.spawn((
                    Num(i),
                    Text::new(""),
                    TextLayout::no_wrap(),
                    ui::display(22.0),
                    TextColor(Color::NONE),
                    TextShadow {
                        offset: Vec2::new(0.0, 2.0),
                        color: Color::NONE,
                    },
                    UiTransform::default(),
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(18),
                        top: px(-34),
                        ..default()
                    },
                ));
            }
        });
    for i in 0..HURT_SLOTS {
        // A thin arc of ARC segments on a ring around the crosshair; the parent turns to point
        // at the attacker, the middle segments are the brightest.
        commands
            .spawn((
                Indicator(i),
                Visibility::Hidden,
                origin(),
                UiTransform::default(),
                ChildOf(root),
            ))
            .with_children(|a| {
                for k in 0..ARC {
                    let ang = (k as f32 - (ARC - 1) as f32 / 2.0) * ARC_STEP;
                    let (s, c) = ang.to_radians().sin_cos();
                    a.spawn((
                        UiTransform::from_rotation(Rot2::degrees(ang)),
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(RING * s - 12.0),
                            top: px(-RING * c - 2.0),
                            width: px(24),
                            height: px(4),
                            border_radius: BorderRadius::all(px(2)),
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                        BoxShadow::new(Color::NONE, px(0), px(0), px(1), px(6)),
                    ));
                }
            });
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update(
    real: Res<Time<Real>>,
    data: Res<ActorData>,
    settings: Res<Settings>,
    mut r: ResMut<Reticle>,
    mut fired: MessageReader<Fire>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    player: Query<(Entity, &GlobalTransform, &Loadout, Has<Dead>), With<Player>>,
    transforms: Query<&GlobalTransform>,
    mut cross: Query<&mut Visibility, With<Cross>>,
    mut arms: Query<(&Arm, &mut Node)>,
    mut bursts: Query<(&Burst, &mut UiTransform), Without<Num>>,
    mut spokes: Query<(&Spoke, &mut BackgroundColor)>,
    mut nums: Query<(
        &Num,
        &mut UiTransform,
        &mut Text,
        &mut TextFont,
        &mut TextColor,
        &mut TextShadow,
    )>,
) {
    let Ok((me, at, loadout, dead)) = player.single() else {
        return;
    };
    let dt = real.delta_secs().max(1e-4);
    let r = &mut *r;
    if r.seed == 0 {
        r.seed = 0x2545_F491;
    }
    let melee = data
        .items
        .get(loadout.slots[loadout.current].item)
        .is_none_or(|i| i.kind == "melee");
    for f in fired.read() {
        if f.shooter == me && !melee && !settings.static_spread {
            r.spread = (r.spread + SPREAD_STEP).min(SPREAD_MAX);
            r.idle = 0.0;
        }
    }
    r.idle += dt;
    if r.idle > SPREAD_RELAX {
        r.spread = 0.0;
    }
    r.hit = (r.hit - dt).max(0.0);
    r.kill = (r.kill - dt).max(0.0);
    for p in &mut r.pops {
        if let Some(q) = p {
            q.age += dt / NUM_LIFE;
            if q.age >= 1.0 {
                *p = None;
            }
        }
    }
    for d in damage.read().filter(|d| d.attacker == me && d.target != me) {
        r.hit = HIT_LIFE;
        let head = data.items.get(d.item).is_some_and(|i| i.kind != "melee")
            && transforms
                .get(d.target)
                .is_ok_and(|g| d.point.y - g.translation().y >= HEAD_FROM);
        let slot = r.pops.iter().position(Option::is_none).unwrap_or_else(|| {
            let age = |i: usize| r.pops[i].as_ref().map_or(0.0, |p| p.age);
            (0..NUMS)
                .max_by(|&a, &b| age(a).total_cmp(&age(b)))
                .unwrap_or(0)
        });
        let dx = -10.0 + 60.0 * rand(&mut r.seed);
        r.pops[slot] = Some(Pop {
            age: 0.0,
            dx,
            head,
            amount: d.amount.round() as i32,
            fresh: true,
        });
    }
    if killed.read().any(|k| k.killer == me && k.victim != me) {
        r.kill = KILL_LIFE;
    }

    let want = if dead {
        Visibility::Hidden
    } else {
        Visibility::Inherited
    };
    for mut v in &mut cross {
        if *v != want {
            *v = want;
        }
    }
    let step = at.translation() - r.at;
    r.at = at.translation();
    let run = (step.xz().length() / dt / 10.0).min(1.0);
    let air = (step.y.abs() / dt / 3.0).min(1.0);
    r.run += (run - r.run) * (10.0 * dt).min(1.0);
    let gap = if melee || settings.static_spread {
        GAP
    } else {
        GAP + 7.0 * r.run + 4.0 * air + r.spread
    };
    for (Arm(i), mut n) in &mut arms {
        let (l, t) = match i {
            0 => (-1.0, -gap - ARM),
            1 => (gap, -1.0),
            2 => (-1.0, gap),
            _ => (-gap - ARM, -1.0),
        };
        if n.left != px(l) || n.top != px(t) {
            (n.left, n.top) = (px(l), px(t));
        }
    }
    let left = |kill: bool| {
        if kill {
            (r.kill, KILL_LIFE)
        } else {
            (r.hit, HIT_LIFE)
        }
    };
    for (Burst(kill), mut tf) in &mut bursts {
        let (t, life) = left(*kill);
        let scale = 1.5 - 0.5 * ui::ease_out(1.0 - t / life);
        tf.scale = Vec2::splat(if *kill { scale * 1.3 } else { scale });
    }
    for (Spoke(kill), mut bg) in &mut spokes {
        let (t, life) = left(*kill);
        let alpha = if t > 0.0 {
            1.0 - ui::ease_out(1.0 - t / life)
        } else {
            0.0
        };
        bg.0 = if *kill { DANGER } else { Color::WHITE }.with_alpha(alpha);
    }
    for (Num(i), mut tf, mut text, mut font, mut color, mut shadow) in &mut nums {
        let Some(p) = &mut r.pops[*i] else {
            if color.0 != Color::NONE {
                color.0 = Color::NONE;
                shadow.color = Color::NONE;
            }
            continue;
        };
        if p.fresh {
            p.fresh = false;
            text.0 = p.amount.to_string();
            *font = ui::display(if p.head { 30.0 } else { 22.0 });
        }
        let e = ui::ease_out(p.age);
        tf.translation = Val2::px(p.dx * e, -60.0 * e);
        tf.scale = Vec2::splat(1.3 - 0.4 * e);
        let base = if p.head {
            Color::srgb_u8(255, 122, 46)
        } else {
            Color::WHITE
        };
        color.0 = base.with_alpha(1.0 - e);
        shadow.color = Color::srgba(0.0, 0.0, 0.0, 0.6 * (1.0 - e));
    }
}

/// The red arcs around the crosshair pointing at whoever hurt the player, relative to the
/// camera's heading (0 = ahead, clockwise).
fn indicators(
    hurt: Res<Hurt>,
    transforms: Query<&GlobalTransform>,
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut arcs: Query<(&Indicator, &mut UiTransform, &mut Visibility, &Children)>,
    mut segs: Query<(&mut BackgroundColor, &mut BoxShadow)>,
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
        // fully lit, then fading over the last 0.75 s
        let fade = (left / 0.75).min(1.0);
        let mid = (kids.len() as f32 - 1.0) / 2.0;
        for (n, k) in kids.iter().enumerate() {
            if let Ok((mut c, mut glow)) = segs.get_mut(k) {
                let taper = 1.0 - ((n as f32 - mid).abs() / (mid + 1.0));
                c.0 = DANGER.with_alpha(fade * (0.35 + 0.6 * taper));
                glow.0[0].color = DANGER.with_alpha(fade * 0.5 * taper);
            }
        }
    }
}
