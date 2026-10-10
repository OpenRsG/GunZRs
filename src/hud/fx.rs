//! HUD extras: the death damage report, teammate bars over their heads and screen-edge blood
//! (`docs/formats.md`, "HUD extras"). A child of `hud.rs`; the UI nodes are spawned once and only
//! rewritten afterwards.

use super::{HP_TIERS, LOW_HP, Root, rand, sfx_image, shake_camera, tier};
use crate::{
    actor::ActorData,
    col::MapCollision,
    combat::{HIT_HEIGHT, Wounded},
    game::{Dead, Killed, Loadout, NewRound, Player, Settings, Team, Vitals},
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
        if !settings.screen_blood {
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
        if dead || !settings.screen_blood || frac >= LOW_HP {
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
