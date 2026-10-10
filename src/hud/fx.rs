//! HUD extras: the death damage report, teammate bars over their heads and screen-edge blood
//! (`docs/formats.md`, "HUD extras"). A child of `hud.rs`; the UI nodes are spawned once and only
//! rewritten afterwards.

use super::{HP_TIERS, LOW_HP, Root, rand, sfx_image, shake_camera, tier};
use crate::{
    actor::ActorData,
    col::MapCollision,
    combat::{HIT_HEIGHT, Wounded},
    game::{Dead, Killed, Loadout, NewRound, Player, Settings, Team, Vitals},
    gore::{paint, smooth, splat_image},
    level::Level,
};
use bevy::{prelude::*, transform::TransformSystems, ui::UiSystems};

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Taken>()
        .add_systems(
            Update,
            (
                spawn_pools
                    .run_if(any_with_component::<Root>.and_then(not(any_with_component::<Splat>))),
                spawn_mates.run_if(any_with_component::<Root>),
                report,
                splat,
                low_blood,
                spawn_lens
                    .run_if(any_with_component::<Root>.and_then(not(any_with_component::<Lens>))),
                lens.run_if(resource_exists::<LensArt>),
            ),
        )
        // After propagation and the camera shake, so the bars follow the frame as rendered.
        .add_systems(
            PostUpdate,
            mate_bars
                .after(TransformSystems::Propagate)
                .after(shake_camera)
                .before(UiSystems::Layout),
        );
}

// ---------------------------------------------------------------------------------------------
// Death damage report

/// Damage the player took this life: attacker, HP lost, AP lost. Cleared on death and new round.
#[derive(Resource, Default)]
struct Taken(Vec<(Entity, f32, f32)>);

/// The report under the death text.
#[derive(Component)]
pub(super) struct ReportText;

/// Attackers listed in the report (the rest are summed in "others").
const REPORT_ROWS: usize = 6;

fn report(
    mut wounded: MessageReader<Wounded>,
    mut killed: MessageReader<Killed>,
    mut rounds: MessageReader<NewRound>,
    mut taken: ResMut<Taken>,
    player: Query<Entity, With<Player>>,
    names: Query<&Name>,
    time: Res<Time>,
    mut text: Single<&mut Text, With<ReportText>>,
) {
    let Ok(me) = player.single() else {
        return;
    };
    if rounds.read().count() > 0 {
        taken.0.clear();
    }
    for w in wounded.read().filter(|w| w.target == me) {
        match taken.0.iter_mut().find(|t| t.0 == w.attacker) {
            Some(t) => (t.1, t.2) = (t.1 + w.hp, t.2 + w.ap),
            None => taken.0.push((w.attacker, w.hp, w.ap)),
        }
    }
    for _ in killed.read().filter(|k| k.victim == me) {
        taken.0.sort_by(|a, b| (b.1 + b.2).total_cmp(&(a.1 + a.2)));
        let mut s = String::new();
        let (mut hp, mut ap) = (0.0, 0.0);
        for (i, &(by, h, a)) in taken.0.iter().enumerate() {
            if i < REPORT_ROWS {
                let who = if by == me {
                    "yourself"
                } else {
                    names.get(by).map_or("?", |n| n.as_str())
                };
                s += &format!("\n{who}: {h:.0} HP  {a:.0} AP");
            }
            (hp, ap) = (hp + h, ap + a);
        }
        if taken.0.len() > REPORT_ROWS {
            s += &format!("\n+{} more", taken.0.len() - REPORT_ROWS);
        }
        if !s.is_empty() {
            s = format!("Damage taken: {hp:.0} HP  {ap:.0} AP{s}");
            info!(
                "t={:.2} damage report: {}",
                time.elapsed_secs(),
                s.replace('\n', " | ")
            );
        }
        text.0 = s;
        taken.0.clear();
    }
}

// ---------------------------------------------------------------------------------------------
// Teammate bars

/// Bar width and track heights (px); the stack sits above the head.
const BAR_W: f32 = 64.0;
const HP_H: f32 = 7.0;
const AP_H: f32 = 4.0;
const AMMO_H: f32 = 14.0;
/// Teammates farther away than this (m) get no bars.
const MATE_RANGE: f32 = 120.0;

/// An actor that already has bar UI.
#[derive(Component)]
struct Barred;

/// UI node of one teammate's bars (`Mate` is on the stack root and on its fills and ammo text).
#[derive(Component)]
struct Mate(Entity);

#[derive(Component)]
struct MateRoot;

#[derive(Component)]
enum MateBar {
    Hp,
    Ap,
}

/// Magazine count the ammo text shows (`None` = blank, melee).
#[derive(Component)]
struct MateAmmo(Option<u32>);

fn track(h: f32) -> impl Bundle {
    (
        Node {
            width: percent(100),
            height: px(h),
            border_radius: BorderRadius::all(px(h / 2.0)),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
    )
}

fn fill(actor: Entity, bar: MateBar) -> impl Bundle {
    (
        Mate(actor),
        bar,
        Node {
            height: percent(100),
            ..default()
        },
        BackgroundColor(Color::WHITE),
    )
}

/// One bar stack for every teamed actor that has none yet (shown only for the player's side).
fn spawn_mates(
    mut commands: Commands,
    root: Single<Entity, With<Root>>,
    new: Query<
        Entity,
        (
            With<Team>,
            With<Vitals>,
            With<Loadout>,
            Without<Player>,
            Without<Barred>,
        ),
    >,
) {
    for a in &new {
        commands.entity(a).insert(Barred);
        commands.spawn((
            MateRoot,
            Mate(a),
            ChildOf(*root),
            ZIndex(-1),
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                width: px(BAR_W),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(1),
                ..default()
            },
            children![
                (track(HP_H), children![fill(a, MateBar::Hp)]),
                (track(AP_H), children![fill(a, MateBar::Ap)]),
                (
                    Mate(a),
                    MateAmmo(None),
                    Text::new(""),
                    TextFont::from_font_size(AMMO_H - 1.0),
                    TextColor(Color::WHITE),
                    TextShadow::default(),
                ),
            ],
        ));
    }
}

/// Positions and fills every bar stack: shown for living teammates of the player in front of the
/// camera with a clear line to their head.
#[allow(clippy::too_many_arguments)]
fn mate_bars(
    settings: Res<Settings>,
    data: Res<ActorData>,
    col: Option<Res<MapCollision>>,
    camera: Single<(&Camera, &GlobalTransform), With<Camera3d>>,
    me: Query<Option<&Team>, With<Player>>,
    actors: Query<(
        &GlobalTransform,
        &Vitals,
        &Loadout,
        Has<Dead>,
        Option<&Team>,
    )>,
    mut roots: Query<(Entity, &Mate, &mut Node, &mut Visibility), With<MateRoot>>,
    mut fills: Query<(&Mate, &MateBar, &mut Node, &mut BackgroundColor), Without<MateRoot>>,
    mut ammo: Query<(&Mate, &mut MateAmmo, &mut Text)>,
    mut commands: Commands,
    ui: Res<UiScale>,
) {
    let (cam, cam_at) = *camera;
    let side = me.single().ok().flatten().copied();
    let size = cam.logical_viewport_size().unwrap_or(Vec2::ZERO);
    for (root, Mate(a), mut node, mut vis) in &mut roots {
        let Ok((at, _, _, dead, team)) = actors.get(*a) else {
            commands.entity(root).despawn();
            continue;
        };
        let head = at.translation() + Vec3::Y * (HIT_HEIGHT + 0.25);
        let to = head - cam_at.translation();
        let p = (settings.team_bars
            && !dead
            && matches!(side, Some(Team::Red | Team::Blue))
            && team.copied() == side
            && to.length() < MATE_RANGE)
            .then(|| cam.world_to_viewport(cam_at, head).ok())
            .flatten()
            .filter(|p| p.x > 0.0 && p.y > 0.0 && p.x < size.x && p.y < size.y)
            .filter(|_| {
                col.as_ref().is_none_or(|c| {
                    c.raycast(cam_at.translation(), to, to.length() - 0.3)
                        .is_none()
                })
            });
        let want = if p.is_some() {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        let Some(p) = p else {
            continue;
        };
        let p = p / ui.0;
        node.left = px(p.x - BAR_W / 2.0);
        node.top = px(p.y - HP_H - AP_H - AMMO_H - 2.0);
    }
    for (Mate(a), bar, mut node, mut bg) in &mut fills {
        let Ok((_, v, ..)) = actors.get(*a) else {
            continue;
        };
        let (value, max, colour) = match bar {
            MateBar::Hp => {
                let [r, g, b] = HP_TIERS[tier(v.hp / v.max_hp.max(1.0))].0;
                (v.hp, v.max_hp, Color::srgb_u8(r, g, b))
            }
            MateBar::Ap => (v.ap, v.max_ap, Color::srgb_u8(70, 150, 235)),
        };
        node.width = percent((value / max.max(1.0)).clamp(0.0, 1.0) * 100.0);
        if bg.0 != colour {
            bg.0 = colour;
        }
    }
    for (Mate(a), mut shown, mut text) in &mut ammo {
        let Ok((_, _, ld, ..)) = actors.get(*a) else {
            continue;
        };
        let slot = &ld.slots[ld.current];
        let ranged = data.items.get(slot.item).is_some_and(|i| i.kind != "melee");
        let want = ranged.then_some(slot.magazine);
        if shown.0 != want {
            shown.0 = want;
            text.0 = want.map_or_else(String::new, |n| n.to_string());
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Screen blood

/// Splatters in the pool, and seconds one lives (*inferred*: 1-2 s fade-out) with its fade-in.
const SPLATS: usize = 12;
const SPLAT_LIFE: f32 = 1.8;
const SPLAT_IN: f32 = 0.08;
/// Splat size (px) at 0 HP lost and per HP lost, the largest allowed (*inferred*).
const SPLAT_MIN: f32 = 200.0;
const SPLAT_PER_HP: f32 = 5.0;
const SPLAT_MAX: f32 = 460.0;
/// Size (px) of the persistent low-health corner blood.
const CORNER: f32 = 340.0;

/// One pooled splatter; `age` runs to [`SPLAT_LIFE`], `peak` is its opacity at full fade-in.
#[derive(Component)]
struct Splat {
    age: f32,
    peak: f32,
}

/// Persistent blood in screen corner `i`, shown under [`LOW_HP`].
#[derive(Component)]
struct Corner;

/// The pools, from the retail `sfx/blood-mark0N.tga` decal textures.
fn spawn_pools(
    mut commands: Commands,
    root: Single<Entity, With<Root>>,
    level: Res<Level>,
    mut images: ResMut<Assets<Image>>,
) {
    let tex: Vec<_> = (1..=5)
        .map(|i| {
            let n = format!("blood-mark0{i}.tga");
            sfx_image(&level.vfs, &mut images, &n).unwrap_or_else(|| panic!("sfx/{n} missing"))
        })
        .collect();
    let node = Node {
        position_type: PositionType::Absolute,
        ..default()
    };
    for i in 0..SPLATS {
        commands.spawn((
            Splat {
                age: SPLAT_LIFE,
                peak: 0.0,
            },
            ChildOf(*root),
            ZIndex(-1),
            Visibility::Hidden,
            UiTransform::default(),
            node.clone(),
            ImageNode::new(tex[i % tex.len()].clone()),
        ));
    }
    for i in 0..4 {
        commands.spawn((
            Corner,
            ChildOf(*root),
            ZIndex(-1),
            Visibility::Hidden,
            UiTransform::from_rotation(Rot2::radians(i as f32 * 1.7)),
            Node {
                left: percent(100 * (i % 2)),
                top: percent(100 * (i / 2)),
                margin: UiRect::all(px(-CORNER / 2.0)),
                width: px(CORNER),
                height: px(CORNER),
                ..node.clone()
            },
            ImageNode::new(tex[3 + i % 2].clone()),
        ));
    }
}

/// A hit that cost the player HP drops a splatter near a random screen edge, sized by the damage,
/// fading out over [`SPLAT_LIFE`].
fn splat(
    mut wounded: MessageReader<Wounded>,
    player: Query<Entity, With<Player>>,
    settings: Res<Settings>,
    real: Res<Time<Real>>,
    mut pool: Query<
        (
            &mut Splat,
            &mut Node,
            &mut UiTransform,
            &mut ImageNode,
            &mut Visibility,
        ),
        Without<Corner>,
    >,
    mut next: Local<usize>,
    mut seed: Local<u32>,
) {
    let dt = real.delta_secs();
    for (mut s, _, _, mut img, mut vis) in &mut pool {
        if s.age >= SPLAT_LIFE {
            continue;
        }
        s.age += dt;
        if s.age >= SPLAT_LIFE {
            *vis = Visibility::Hidden;
            continue;
        }
        let a = s.peak * (s.age / SPLAT_IN).min(1.0) * (1.0 - s.age / SPLAT_LIFE);
        img.color = Color::srgba(1.0, 1.0, 1.0, a);
    }
    let Ok(me) = player.single() else {
        return;
    };
    if *seed == 0 {
        *seed = 0x9E37_79B9;
    }
    for w in wounded.read().filter(|w| w.target == me && w.hp > 0.0) {
        if !settings.screen_blood || settings.realistic_blood {
            continue;
        }
        // Big hits splash twice or three times.
        for _ in 0..1 + (w.hp / 25.0) as usize {
            let slot = *next % SPLATS;
            *next += 1;
            let Some((mut s, mut n, mut t, mut img, mut vis)) = pool.iter_mut().nth(slot) else {
                return;
            };
            let size =
                ((SPLAT_MIN + w.hp * SPLAT_PER_HP) * (0.7 + 0.6 * rand(&mut seed))).min(SPLAT_MAX);
            // Along a random edge, up to a quarter of the screen in.
            let (along, depth) = (rand(&mut seed), rand(&mut seed) * 0.25);
            let (x, y) = match (rand(&mut seed) * 4.0) as u32 {
                0 => (along, depth),
                1 => (along, 1.0 - depth),
                2 => (depth, along),
                _ => (1.0 - depth, along),
            };
            *s = Splat {
                age: 0.0,
                peak: (0.6 + w.hp / 60.0).min(1.0),
            };
            n.left = percent(x * 100.0);
            n.top = percent(y * 100.0);
            n.margin = UiRect::all(px(-size / 2.0));
            n.width = px(size);
            n.height = px(size);
            t.rotation = Rot2::radians(rand(&mut seed) * std::f32::consts::TAU);
            img.color = Color::srgba(1.0, 1.0, 1.0, 0.0);
            *vis = Visibility::Inherited;
        }
    }
}

/// Persistent blood in the screen corners below [`LOW_HP`], thicker and pulsing as health drops.
fn low_blood(
    settings: Res<Settings>,
    real: Res<Time<Real>>,
    player: Query<(&Vitals, Has<Dead>), With<Player>>,
    mut corners: Query<(&mut ImageNode, &mut Visibility), With<Corner>>,
) {
    let level = player.single().ok().map_or(0.0, |(v, dead)| {
        let frac = v.hp / v.max_hp.max(1.0);
        if dead || !settings.screen_blood || settings.realistic_blood || frac >= LOW_HP {
            0.0
        } else {
            1.0 - frac / LOW_HP
        }
    });
    let want = if level > 0.0 {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let pulse = 0.85 + 0.15 * (real.elapsed_secs() * (2.0 + 3.0 * level)).sin();
    for (mut img, mut vis) in &mut corners {
        if *vis != want {
            *vis = want;
        }
        if level > 0.0 {
            img.color = Color::srgba(1.0, 1.0, 1.0, (0.4 + 0.5 * level) * pulse);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Lens blood (`Settings::realistic_blood`: replaces the splats and corner blood above)

/// Nodes in the pool (drops, sliding drops and spatters), seconds a drop fades out over, and the
/// share of its remaining slide a drop covers per second.
const LENS: usize = 40;
const LENS_FADE: f32 = 1.5;
const LENS_SLIDE: f32 = 0.3;
/// Seconds blood takes to go from bright red to dark rusty on the lens.
const LENS_AGE: f32 = 6.0;
/// Drops stay clear of the middle of the screen: invisible within the first radius, fully shown
/// beyond the second (fractions of the screen height).
const CLEAR: (f32, f32) = (0.12, 0.3);

/// One blood drop or spatter on the "lens"; free once `age` reaches `life`. After `slide_at`
/// seconds a big drop runs `to` px down, leaving a trail ([`Trail`]) of the distance `travel`.
/// `at` is where it started (fractions of the screen).
#[derive(Component)]
struct Lens {
    age: f32,
    life: f32,
    peak: f32,
    size: f32,
    slide_at: f32,
    travel: f32,
    to: f32,
    at: Vec2,
}

/// The streak a sliding drop leaves above itself (child of a [`Lens`]).
#[derive(Component)]
struct Trail;

/// Dark red screen edges, stronger at low health and for a moment after a hit.
#[derive(Component)]
struct Vignette;

#[derive(Resource)]
struct LensArt {
    drops: Vec<Handle<Image>>,
    sliders: Vec<Handle<Image>>,
    spatter: Vec<Handle<Image>>,
}

/// Tint of lens blood `age` seconds old (textures are baked a little brown): vivid red when fresh,
/// darker and browner after [`LENS_AGE`].
fn lens_tint(age: f32, alpha: f32) -> Color {
    let t = smooth(0.0, LENS_AGE, age);
    Color::srgba(1.0 - 0.35 * t, 0.7 + 0.3 * t, 0.7 + 0.3 * t, alpha)
}

/// One texel of a blood drop on glass: `q` is 0 in the middle and 1 at the nominal edge, `rr` the
/// distance as a fraction of the (wobbly) edge. A thick dark rim, a thin see-through middle
/// (thicker toward the bottom, where gravity pulls), a small specular highlight, a faint second
/// one and an inner caustic arc low on the far side.
fn lens_texel(q: Vec2, rr: f32) -> [f32; 4] {
    let rim = smooth(0.4, 1.0, rr).powf(1.4);
    let low = q.y.max(0.0);
    let spec = (-((q.x + 0.3).powi(2) + (q.y + 0.38).powi(2)) / 0.012).exp();
    let spec2 = (-((q.x - 0.34).powi(2) + (q.y - 0.4).powi(2)) / 0.02).exp() * 0.35;
    let caustic =
        smooth(0.55, 0.72, rr) * smooth(0.92, 0.78, rr) * smooth(0.0, 0.5, q.x + q.y) * 0.35;
    let body = mix3([0.6, 0.07, 0.05], [0.18, 0.0, 0.01], rim.max(low * 0.5));
    let c = mix3(
        mix3(body, [0.9, 0.2, 0.15], caustic),
        [1.0, 0.95, 0.95],
        (spec + spec2).min(1.0),
    );
    let inside = smooth(1.0, 0.9, rr);
    let thick = 0.32 + 0.6 * rim + 0.12 * low;
    [
        c[0],
        c[1],
        c[2],
        inside * thick.max((spec + 0.5 * spec2).min(1.0) * 0.95),
    ]
}

fn mix3(a: [f32; 3], b: [f32; 3], t: f32) -> [f32; 3] {
    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * t)
}

/// A blood drop on glass with a noise-perturbed outline. Round ones get a few satellite
/// droplets; `slider` ones are a teardrop: a round head low in the image with a thin wobbly tail
/// tapering to the top, where the [`Trail`] continues.
fn lens_drop_image(seed: u32, slider: bool) -> Image {
    let mut s = seed | 1;
    let ph: [f32; 4] = std::array::from_fn(|_| rand(&mut s) * std::f32::consts::TAU);
    let sats: Vec<(Vec2, f32)> = if slider {
        vec![]
    } else {
        (0..3)
            .map(|_| {
                let a = Vec2::from_angle(rand(&mut s) * std::f32::consts::TAU);
                (a * (0.74 + 0.16 * rand(&mut s)), 0.06 + 0.07 * rand(&mut s))
            })
            .collect()
    };
    let (c, r0) = if slider {
        (Vec2::new(0.0, 0.3), 0.4)
    } else {
        (Vec2::ZERO, 0.58)
    };
    paint(96, 96, |u, v| {
        let p = Vec2::new(u - 0.5, v - 0.5) * 2.0;
        let d = p - c;
        let th = d.y.atan2(d.x);
        let edge = r0
            * (1.0
                + 0.14 * (2.0 * th + ph[0]).sin()
                + 0.08 * (3.0 * th + ph[1]).sin()
                + 0.05 * (5.0 * th + ph[2]).sin());
        let mut px = lens_texel(d / r0, d.length() / edge);
        for &(sc, sr) in &sats {
            let t = lens_texel((p - sc) / sr, (p - sc).length() / sr);
            if t[3] > px[3] {
                px = t;
            }
        }
        // The tail leaves the top of the head: a cylinder of blood, rim-dark at its sides.
        let join = c.y - r0 * 0.75;
        if slider && p.y < join && p.y > -0.95 {
            let t = ((join - p.y) / (join + 0.95)).clamp(0.0, 1.0);
            let half = 0.17 * (1.0 - t).powf(1.2);
            let x = p.x + 0.03 * (p.y * 9.0 + ph[3]).sin();
            let tail = lens_texel(Vec2::new(x / half * 0.5, 0.0), x.abs() / half);
            let a = tail[3] * (1.0 - 0.5 * t);
            if a > px[3] {
                px = [tail[0], tail[1], tail[2], a];
            }
        }
        px
    })
}

fn spawn_lens(
    mut commands: Commands,
    root: Single<Entity, With<Root>>,
    mut images: ResMut<Assets<Image>>,
) {
    // Thin at the top, widening and wobbling toward the drop below.
    let streak = images.add(paint(16, 64, |u, v| {
        let wob = 0.1 * (v * 10.0).sin() + 0.06 * (v * 4.0 + 1.0).sin();
        let cx = (u - 0.5 - wob).abs() * 2.0;
        let w = 0.12 + 0.3 * v;
        [
            0.42,
            0.06,
            0.05,
            smooth(w, w * 0.4, cx) * smooth(0.0, 0.45, v) * 0.75,
        ]
    }));
    let vignette = images.add(paint(128, 128, |u, v| {
        let r = Vec2::new(u - 0.5, v - 0.5).length() * 2.0;
        [0.25, 0.0, 0.01, smooth(0.35, 1.25, r)]
    }));
    commands.insert_resource(LensArt {
        drops: (1..=4)
            .map(|i| images.add(lens_drop_image(0x1E45_1000 + i, false)))
            .collect(),
        sliders: (1..=2)
            .map(|i| images.add(lens_drop_image(0x1E45_2000 + i, true)))
            .collect(),
        spatter: (1..=4)
            .map(|i| images.add(splat_image(0x1E45_0000 + i, false)))
            .collect(),
    });
    for _ in 0..LENS {
        commands
            .spawn((
                Lens {
                    age: 0.0,
                    life: 0.0,
                    peak: 0.0,
                    size: 0.0,
                    slide_at: f32::MAX,
                    travel: 0.0,
                    to: 0.0,
                    at: Vec2::ZERO,
                },
                ChildOf(*root),
                ZIndex(-1),
                Visibility::Hidden,
                UiTransform::default(),
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                ImageNode::default(),
            ))
            .with_children(|c| {
                c.spawn((
                    Trail,
                    Node {
                        position_type: PositionType::Absolute,
                        bottom: percent(50),
                        ..default()
                    },
                    ImageNode::new(streak.clone()),
                ));
            });
    }
    commands.spawn((
        Vignette,
        ChildOf(*root),
        ZIndex(-2),
        Visibility::Hidden,
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            ..default()
        },
        ImageNode {
            image_mode: NodeImageMode::Stretch,
            ..ImageNode::new(vignette)
        },
    ));
}

/// Hits on the player throw a cluster of drops (and spatters for the bigger ones) on the lens
/// near the screen edges; big drops slide down, everything darkens and fades out; the edges
/// darken with damage and low health.
#[allow(clippy::too_many_arguments)]
fn lens(
    mut wounded: MessageReader<Wounded>,
    player: Query<(Entity, &Vitals, Has<Dead>), With<Player>>,
    settings: Res<Settings>,
    real: Res<Time<Real>>,
    art: Res<LensArt>,
    screen: Query<&ComputedNode, With<Root>>,
    mut lenses: Query<
        (
            &mut Lens,
            &mut Node,
            &mut UiTransform,
            &mut ImageNode,
            &mut Visibility,
            &Children,
        ),
        Without<Trail>,
    >,
    mut trails: Query<(&mut Node, &mut ImageNode), (With<Trail>, Without<Lens>)>,
    mut vignette: Query<
        (&mut ImageNode, &mut Visibility),
        (With<Vignette>, Without<Lens>, Without<Trail>),
    >,
    mut next: Local<usize>,
    mut flash: Local<f32>,
    mut seed: Local<u32>,
) {
    let dt = real.delta_secs();
    let on = settings.screen_blood && settings.realistic_blood;
    if *seed == 0 {
        *seed = 0x1E45_9A7B;
    }
    *flash = (*flash - 0.8 * dt).max(0.0);
    // Logical size of the screen: its aspect and height in px.
    let (aspect, height) = screen.single().map_or((16.0 / 9.0, 720.0), |n| {
        let s = n.size() * n.inverse_scale_factor();
        (s.x / s.y.max(1.0), s.y.max(1.0))
    });
    for (mut l, mut n, _, mut img, mut vis, kids) in &mut lenses {
        if l.age >= l.life {
            continue;
        }
        l.age += dt;
        if l.age >= l.life || !on {
            l.age = l.life;
            *vis = Visibility::Hidden;
            continue;
        }
        if l.age > l.slide_at {
            l.travel += (l.to - l.travel) * LENS_SLIDE * dt;
            n.margin.top = px(l.travel - l.size / 2.0);
        }
        let here = Vec2::new((l.at.x - 0.5) * aspect, l.at.y + l.travel / height - 0.5);
        let a = l.peak
            * smooth(0.0, 0.08, l.age)
            * smooth(l.life, l.life - LENS_FADE, l.age)
            * smooth(CLEAR.0, CLEAR.1, here.length());
        img.color = lens_tint(l.age, a);
        for &k in kids {
            if let Ok((mut tn, mut timg)) = trails.get_mut(k) {
                tn.height = px(l.travel);
                timg.color = lens_tint(l.age, a);
            }
        }
    }
    let (me, frac, dead) = player.single().map_or((None, 1.0, true), |(e, v, d)| {
        (Some(e), v.hp / v.max_hp.max(1.0), d)
    });
    for w in wounded
        .read()
        .filter(|w| Some(w.target) == me && w.hp > 0.0)
    {
        if !on {
            continue;
        }
        *flash = (*flash + w.hp / 30.0).min(0.8);
        // The cluster lands near a random edge: where the blow came from is not known here.
        let (along, depth) = (rand(&mut seed), rand(&mut seed) * 0.22);
        let (cx, cy) = match (rand(&mut seed) * 4.0) as u32 {
            0 => (along, depth),
            1 => (along, 1.0 - depth),
            2 => (depth, along),
            _ => (1.0 - depth, along),
        };
        for i in 0..(3 + (w.hp / 6.0) as usize).min(12) {
            let slot = *next % LENS;
            *next += 1;
            let Some((mut l, mut n, mut t, mut img, mut vis, kids)) = lenses.iter_mut().nth(slot)
            else {
                return;
            };
            let spatter = w.hp >= 8.0 && i < 1 + (w.hp / 30.0) as usize;
            let size = if spatter {
                ((180.0 + w.hp * 4.0) * (0.7 + 0.6 * rand(&mut seed))).min(460.0)
            } else {
                let r = rand(&mut seed);
                22.0 + 70.0 * r * r * (0.5 + w.hp / 60.0).min(1.5)
            };
            let slides = !spatter && size > 30.0;
            let spread = if spatter { 0.2 } else { 0.1 };
            let at = Vec2::new(
                (cx + (rand(&mut seed) - 0.5) * spread).clamp(0.02, 0.98),
                (cy + (rand(&mut seed) - 0.5) * spread).clamp(0.02, 0.98),
            );
            *l = Lens {
                age: 0.0,
                life: if spatter { 4.0 } else { 6.0 } + 3.0 * rand(&mut seed),
                peak: if spatter { 0.8 } else { 0.95 },
                size,
                slide_at: if slides {
                    0.5 + 2.0 * rand(&mut seed)
                } else {
                    f32::MAX
                },
                travel: 0.0,
                to: ((30.0 + 110.0 * rand(&mut seed)) * size / 40.0).min(170.0),
                at,
            };
            n.left = percent(at.x * 100.0);
            n.top = percent(at.y * 100.0);
            n.margin = UiRect::all(px(-size / 2.0));
            n.width = px(size);
            n.height = px(size);
            t.rotation = if spatter {
                Rot2::radians(rand(&mut seed) * std::f32::consts::TAU)
            } else {
                Rot2::IDENTITY
            };
            let set = if spatter {
                &art.spatter
            } else if slides {
                &art.sliders
            } else {
                &art.drops
            };
            img.image = set[(rand(&mut seed) * set.len() as f32) as usize % set.len()].clone();
            img.color = lens_tint(0.0, 0.0);
            *vis = Visibility::Inherited;
            for &k in kids {
                if let Ok((mut tn, _)) = trails.get_mut(k) {
                    tn.width = px(size * 0.16);
                    tn.left = px(size * 0.42);
                    tn.height = px(0);
                }
            }
        }
    }
    let level = if dead || !on {
        0.0
    } else {
        ((1.0 - frac / 0.5).clamp(0.0, 1.0) * 0.8 + *flash).min(1.0)
    };
    let pulse = 0.9 + 0.1 * (real.elapsed_secs() * (2.0 + 3.0 * (1.0 - frac))).sin();
    if let Ok((mut img, mut vis)) = vignette.single_mut() {
        let want = if level > 0.0 {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *vis != want {
            *vis = want;
        }
        img.color = Color::srgba(1.0, 1.0, 1.0, level * pulse);
    }
}
