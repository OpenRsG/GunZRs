//! The in-match HUD panels (Bevy UI over the 3D view): dynamic crosshair with hit and kill
//! markers, health and armour bars with a damage trail, the weapon strip and ammo counter, the
//! match header, kill/death counters, notices, the death screen and the scoreboard. A child of
//! `hud.rs`; the nodes are spawned once and only rewritten afterwards. Everything is drawn from
//! rounded dark cards and the item icons of `interface/loadable/`; the retail frame art is gone.

use super::{HP_TIERS, HURT_SLOTS, Hud, Indicator, LOW_HP, LowHp, Marks, Root, fx, set, tier};
use crate::{
    actor::ActorData,
    controls::{Action, Input},
    game::{Dead, Fire, Loadout, Player, Score, Settings, Status, Team, Vitals},
    gfx::Knob,
    menu::{ACCENT, DIM},
    profile::{MatchGain, Profile, Ranks},
    session::{Clock, HOLD},
};
use bevy::{prelude::*, text::Justify, ui::UiTargetCamera};

/// Rows of the scoreboard and the weapon tiles of the strip.
const BOARD_ROWS: usize = 12;
const TILES: usize = 6;

const PANEL: Color = Color::srgba(0.03, 0.04, 0.07, 0.62);
const EDGE: Color = Color::srgba(1.0, 1.0, 1.0, 0.09);
const RED: Color = Color::srgb(1.0, 0.32, 0.28);
const TRACK: Color = Color::srgba(1.0, 1.0, 1.0, 0.12);
const AP_FILL: ([u8; 3], [u8; 3]) = ([40, 111, 162], [64, 150, 220]);

/// The damage-direction arc: segments, the angle between them (degrees) and the ring radius (px).
const ARC: usize = 9;
const ARC_STEP: f32 = 9.0;
const RING: f32 = 140.0;

/// The header's mode line wrapper (hidden when the mode has none).
#[derive(Component)]
pub(super) struct HeaderLine;

/// Text slots rewritten every frame.
#[derive(Component, Clone, Copy)]
pub(super) enum Label {
    Hp,
    Ap,
    /// Magazine, reserve and the RELOAD / EMPTY hint.
    Mag,
    Reserve,
    Hint,
    Weapon,
    /// The respawn line of the death screen.
    Death,
    /// The player's own kills and deaths at the top left.
    Kills,
    Deaths,
    /// The header's mode line, and its timer above it.
    Clock,
    Timer,
    /// Slow / stun / root / burn timers of the player (`Status`).
    Status,
}

/// Bars whose width is a percentage; a `Trail` lags behind its bar (the damage just taken).
#[derive(Component, Clone, Copy, PartialEq)]
pub(super) enum Fill {
    Hp,
    HpTrail,
    Ap,
    ApTrail,
}

/// Elements shown only in some states.
#[derive(Component, Clone, Copy, PartialEq)]
pub(super) enum Show {
    Cross,
    Hit,
    Kill,
    Death,
    Board,
}

/// One arm of a crosshair or marker; `.1` is its direction (0 up, then clockwise).
#[derive(Component)]
pub(super) struct Tick(Show, usize);

/// The health number: pops when the player is hurt.
#[derive(Component)]
pub(super) struct Pop;

/// Weapon tile `i` of the strip, and the icon inside it with the item it shows.
#[derive(Component)]
pub(super) struct Tile(usize);

#[derive(Component)]
pub(super) struct Icon(usize, u32);

/// The centre-screen notice.
#[derive(Component)]
pub(super) struct Banner;

/// Scoreboard row `i`, its team stripe and its text cells (rank, name, kills, deaths, ratio).
#[derive(Component)]
pub(super) struct Row(usize);

#[derive(Component)]
pub(super) struct Stripe(usize);

#[derive(Component)]
pub(super) struct Cell(usize, usize);

/// The weapon strip and ammo panel (bottom right; bottom centre on a touch screen).
#[derive(Component)]
pub(super) struct WeaponBox;

/// The scoreboard's last line once the match is over: level, XP and bounty earned (`profile.rs`).
#[derive(Component)]
pub(super) struct Earned;

/// On a touch screen the fire button covers the bottom right corner: the weapon panel moves to
/// the bottom centre, and back when the player switches to mouse and keyboard.
pub(super) fn touch_layout(
    touch: Option<Res<crate::game::TouchScreen>>,
    mut panel: Query<(&mut Node, &mut UiTransform), With<WeaponBox>>,
) {
    for (mut node, mut tf) in &mut panel {
        let centre = touch.is_some();
        if (node.right == Val::Auto) != centre {
            (node.right, node.left, node.bottom) = if centre {
                (Val::Auto, percent(50), px(8))
            } else {
                (px(24), Val::Auto, px(24))
            };
            tf.translation = Val2::new(if centre { percent(-50) } else { px(0) }, px(0));
        }
    }
}

fn label(slot: Label, size: f32, color: Color) -> impl Bundle {
    (
        slot,
        Text::new(""),
        TextFont::from_font_size(size),
        TextColor(color),
    )
}

fn text(s: &str, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(size),
        TextColor(color),
    )
}

/// A rounded dark card (12 px corners unless the node sets its own).
fn card(node: Node) -> impl Bundle {
    card_on(node, PANEL)
}

fn card_on(mut node: Node, bg: Color) -> impl Bundle {
    node.border = UiRect::all(px(1));
    if node.border_radius == BorderRadius::ZERO {
        node.border_radius = BorderRadius::all(px(12));
    }
    (
        node,
        BackgroundColor(bg),
        BorderColor::all(EDGE),
        BoxShadow::new(
            Color::srgba(0.0, 0.0, 0.0, 0.35),
            px(0),
            px(4),
            px(0),
            px(14),
        ),
    )
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

/// A zero-size node at the screen centre; its children are placed around that point.
fn origin(turn: f32) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            top: percent(50),
            ..default()
        },
        UiTransform::from_rotation(Rot2::degrees(turn)),
    )
}

fn tick(kind: Show, dir: usize, len: f32, thick: f32, color: Color) -> impl Bundle {
    let (w, h) = if dir % 2 == 0 {
        (thick, len)
    } else {
        (len, thick)
    };
    let (left, top) = match dir {
        0 => (-thick / 2.0, -len),
        1 => (0.0, -thick / 2.0),
        2 => (-thick / 2.0, 0.0),
        _ => (-len, -thick / 2.0),
    };
    (
        Tick(kind, dir),
        UiTransform::default(),
        Node {
            position_type: PositionType::Absolute,
            left: px(left),
            top: px(top),
            width: px(w),
            height: px(h),
            ..default()
        },
        BackgroundColor(color),
        BoxShadow::new(
            Color::srgba(0.0, 0.0, 0.0, 0.55),
            px(0),
            px(0),
            px(1),
            px(2),
        ),
    )
}

fn marker(p: &mut ChildSpawnerCommands, kind: Show, len: f32, thick: f32, color: Color, turn: f32) {
    p.spawn((kind, Visibility::Hidden, origin(turn)))
        .with_children(|c| {
            for dir in 0..4 {
                c.spawn(tick(kind, dir, len, thick, color));
            }
        });
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
pub(super) fn vignette(alpha: f32) -> BackgroundGradient {
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

fn stretch() -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(0),
        top: px(0),
        width: percent(100),
        height: percent(100),
        ..default()
    }
}

/// A rounded bar with its damage trail behind the fill.
fn bar(fill: Fill, trail: Fill, (w, h): (f32, f32), g: ([u8; 3], [u8; 3])) -> impl Bundle {
    (
        Node {
            width: px(w),
            height: px(h),
            border_radius: BorderRadius::all(px(h / 2.0)),
            overflow: Overflow::clip(),
            ..default()
        },
        BackgroundColor(TRACK),
        children![
            (
                trail,
                stretch(),
                BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.7))
            ),
            (fill, stretch(), gradient(g)),
        ],
    )
}

pub(super) fn spawn_ui(mut commands: Commands, cameras: Query<Entity, With<Camera3d>>) {
    let Ok(camera) = cameras.single() else {
        return;
    };
    // Sounds of other actors are positioned relative to the view.
    commands.entity(camera).insert(SpatialListener::new(0.2));
    let root = commands
        .spawn((
            Root,
            UiTargetCamera(camera),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
        ))
        .id();
    commands.entity(root).with_children(|r| {
        // Crosshair, hit marker, kill marker.
        marker(
            r,
            Show::Cross,
            8.0,
            2.0,
            Color::srgba(1.0, 1.0, 1.0, 0.92),
            0.0,
        );
        marker(r, Show::Hit, 8.0, 2.0, Color::WHITE, 45.0);
        marker(r, Show::Kill, 11.0, 3.0, RED, 45.0);
        r.spawn((
            LowHp,
            Visibility::Hidden,
            overlay(Color::NONE),
            vignette(0.0),
        ));
        // Notice.
        r.spawn((
            Node {
                position_type: PositionType::Absolute,
                top: percent(62),
                left: px(0),
                width: percent(100),
                justify_content: JustifyContent::Center,
                ..default()
            },
            children![(
                Banner,
                UiTransform::default(),
                Text::new(""),
                TextFont::from_font_size(26.0),
                TextColor(ACCENT),
                TextShadow::default(),
            )],
        ));
        // Status effects, above the vitals.
        r.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: px(24),
                bottom: px(118),
                ..default()
            },
            children![(
                label(Label::Status, 20.0, Color::srgb(0.45, 0.8, 1.0)),
                TextShadow::default()
            )],
        ));
        // Health and armour.
        r.spawn(card(Node {
            position_type: PositionType::Absolute,
            left: px(24),
            bottom: px(24),
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            padding: UiRect::axes(px(14), px(12)),
            ..default()
        }))
        .with_children(|c| {
            c.spawn((
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    ..default()
                },
                children![
                    (
                        Node {
                            width: px(26),
                            ..default()
                        },
                        children![text("HP", 13.0, DIM)]
                    ),
                    bar(Fill::Hp, Fill::HpTrail, (230.0, 14.0), HP_TIERS[0]),
                    (
                        Pop,
                        Node {
                            width: px(52),
                            justify_content: JustifyContent::FlexEnd,
                            ..default()
                        },
                        UiTransform::default(),
                        children![(label(Label::Hp, 28.0, Color::WHITE), TextShadow::default())],
                    ),
                ],
            ));
            c.spawn((
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    ..default()
                },
                children![
                    (
                        Node {
                            width: px(26),
                            ..default()
                        },
                        children![text("AP", 13.0, DIM)]
                    ),
                    bar(Fill::Ap, Fill::ApTrail, (230.0, 8.0), AP_FILL),
                    (
                        Node {
                            width: px(52),
                            justify_content: JustifyContent::FlexEnd,
                            ..default()
                        },
                        children![label(Label::Ap, 20.0, Color::srgb(0.7, 0.85, 1.0))],
                    ),
                ],
            ));
        });
        // Match header: the timer over the mode line, on a bar fading out at both ends.
        r.spawn((
            Node {
                position_type: PositionType::Absolute,
                left: percent(50),
                margin: UiRect::left(px(-230)),
                top: px(8),
                width: px(460),
                min_height: px(40),
                padding: UiRect::vertical(px(3)),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundGradient::from(LinearGradient::to_right(vec![
                ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.0), percent(0)),
                ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.6), percent(22)),
                ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.6), percent(78)),
                ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.0), percent(100)),
            ])),
            children![
                (
                    label(Label::Timer, 24.0, Color::WHITE),
                    TextShadow::default()
                ),
                (
                    HeaderLine,
                    Node {
                        display: Display::None,
                        ..default()
                    },
                    children![(
                        label(Label::Clock, 15.0, Color::srgb(0.85, 0.88, 0.95)),
                        TextShadow::default(),
                        TextLayout {
                            linebreak: bevy::text::LineBreak::NoWrap,
                            ..default()
                        },
                    )],
                ),
            ],
        ));
        // Own kills and deaths.
        r.spawn(card(Node {
            position_type: PositionType::Absolute,
            left: px(16),
            top: px(12),
            align_items: AlignItems::Center,
            column_gap: px(8),
            padding: UiRect::axes(px(12), px(6)),
            border_radius: BorderRadius::all(px(10)),
            ..default()
        }))
        .with_children(|c| {
            c.spawn(text("K", 13.0, DIM));
            c.spawn(label(Label::Kills, 22.0, Color::WHITE));
            c.spawn(Node {
                width: px(8),
                ..default()
            });
            c.spawn(text("D", 13.0, DIM));
            c.spawn(label(Label::Deaths, 22.0, Color::WHITE));
        });
        // Weapon strip and ammo.
        r.spawn((
            WeaponBox,
            UiTransform::default(),
            Node {
                position_type: PositionType::Absolute,
                right: px(24),
                bottom: px(24),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::End,
                row_gap: px(10),
                ..default()
            },
        ))
        .with_children(|w| {
            w.spawn(Node {
                column_gap: px(6),
                align_items: AlignItems::FlexEnd,
                ..default()
            })
            .with_children(|s| {
                for i in 0..TILES {
                    s.spawn((
                        Tile(i),
                        UiTransform::default(),
                        Node {
                            width: px(50),
                            height: px(50),
                            border: UiRect::all(px(1)),
                            border_radius: BorderRadius::all(px(8)),
                            ..default()
                        },
                        BackgroundColor(PANEL),
                        BorderColor::all(EDGE),
                        children![
                            (
                                Icon(i, u32::MAX),
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(4),
                                    top: px(4),
                                    width: px(40),
                                    height: px(40),
                                    ..default()
                                },
                                ImageNode::default(),
                            ),
                            (
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(4),
                                    top: px(1),
                                    ..default()
                                },
                                children![text(&(i + 1).to_string(), 11.0, DIM)],
                            ),
                        ],
                    ));
                }
            });
            w.spawn(card(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::End,
                min_width: px(220),
                padding: UiRect::axes(px(16), px(10)),
                ..default()
            }))
            .with_children(|a| {
                a.spawn(label(Label::Weapon, 15.0, DIM));
                a.spawn(Node {
                    align_items: AlignItems::Baseline,
                    column_gap: px(8),
                    ..default()
                })
                .with_children(|n| {
                    n.spawn((
                        label(Label::Hint, 16.0, RED),
                        Node {
                            margin: UiRect::right(px(10)),
                            ..default()
                        },
                    ));
                    n.spawn((label(Label::Mag, 46.0, Color::WHITE), TextShadow::default()));
                    n.spawn(label(Label::Reserve, 22.0, DIM));
                });
            });
        });
        // Death screen.
        r.spawn((
            Show::Death,
            Visibility::Hidden,
            overlay(Color::srgba(0.0, 0.0, 0.0, 0.18)),
        ))
        .with_children(|d| {
            d.spawn(card(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(10),
                padding: UiRect::axes(px(40), px(22)),
                border_radius: BorderRadius::all(px(16)),
                ..default()
            }))
            .with_children(|c| {
                c.spawn((text("YOU DIED", 44.0, RED), TextShadow::default()));
                c.spawn(label(Label::Death, 22.0, Color::WHITE));
                c.spawn((
                    fx::ReportText,
                    Text::new(""),
                    TextFont::from_font_size(18.0),
                    TextColor(Color::srgb(1.0, 0.85, 0.85)),
                    TextLayout {
                        justify: Justify::Center,
                        ..default()
                    },
                ));
            });
        });
        // Scoreboard: 720 x 440, centred (the clan strip sits on its top right).
        r.spawn((
            Show::Board,
            Visibility::Hidden,
            card_on(
                Node {
                    position_type: PositionType::Absolute,
                    left: percent(50),
                    top: percent(50),
                    margin: UiRect {
                        left: px(-360),
                        top: px(-220),
                        ..default()
                    },
                    width: px(720),
                    padding: UiRect::axes(px(24), px(18)),
                    flex_direction: FlexDirection::Column,
                    row_gap: px(3),
                    border_radius: BorderRadius::all(px(16)),
                    ..default()
                },
                Color::srgba(0.03, 0.04, 0.07, 0.88),
            ),
        ))
        .with_children(|b| {
            b.spawn((
                Node {
                    height: px(34),
                    ..default()
                },
                children![text("SCOREBOARD", 22.0, ACCENT)],
            ));
            let cols = [
                ("#", 34.0),
                ("PLAYER", 350.0),
                ("KILLS", 90.0),
                ("DEATHS", 90.0),
                ("K/D", 70.0),
            ];
            b.spawn(Node {
                height: px(22),
                padding: UiRect::horizontal(px(12)),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|h| {
                for (name, w) in cols {
                    h.spawn((
                        Node {
                            width: px(w),
                            ..default()
                        },
                        children![text(name, 12.0, DIM)],
                    ));
                }
            });
            for i in 0..BOARD_ROWS {
                b.spawn((
                    Row(i),
                    Node {
                        height: px(25),
                        padding: UiRect::horizontal(px(12)),
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(px(6)),
                        ..default()
                    },
                    BackgroundColor(Color::NONE),
                ))
                .with_children(|row| {
                    row.spawn((
                        Stripe(i),
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(0),
                            top: px(4),
                            width: px(3),
                            height: px(17),
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                    ));
                    for (c, (_, w)) in cols.iter().enumerate() {
                        row.spawn((
                            Node {
                                width: px(*w),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            children![(
                                Cell(i, c),
                                Text::new(""),
                                TextFont::from_font_size(17.0),
                                TextColor(Color::WHITE),
                                TextLayout {
                                    linebreak: bevy::text::LineBreak::NoWrap,
                                    ..default()
                                },
                            )],
                        ));
                    }
                });
            }
            b.spawn((
                Earned,
                Text::new(""),
                TextFont::from_font_size(18.0),
                TextColor(ACCENT),
                Node {
                    margin: UiRect::top(px(8)),
                    ..default()
                },
            ));
        });
    });
    for i in 0..HURT_SLOTS {
        // A thin arc of ARC segments on a ring around the crosshair; the parent turns to point
        // at the attacker, the middle segments are the brightest.
        commands
            .spawn((Indicator(i), Visibility::Hidden, origin(0.0), ChildOf(root)))
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

/// Colour at `frac` of the way from red to the bar's own colour: the number's low-health tint.
fn lerp(a: Color, b: Color, t: f32) -> Color {
    Color::from(LinearRgba::from(a).mix(&LinearRgba::from(b), t.clamp(0.0, 1.0)))
}

/// Health and armour: bar widths with their trails, numbers, the damage pop, status effects, the
/// hit flash and the low-health vignette.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn vitals(
    marks: Res<Marks>,
    real: Res<Time<Real>>,
    player: Query<(&Vitals, Has<Dead>, Option<&Status>), With<Player>>,
    mut texts: Query<(&Label, &mut Text, &mut TextColor)>,
    mut fills: Query<(&Fill, &mut Node, &mut BackgroundGradient)>,
    mut pop: Query<&mut UiTransform, With<Pop>>,
    settings: Res<Settings>,
    profile: Option<Res<Profile>>,
    mut low: Query<(&mut BackgroundGradient, &mut Visibility), (With<LowHp>, Without<Fill>)>,
    // shown fractions [hp, ap], their trails, seconds before the trails drain, last values
    mut shown: Local<[f32; 4]>,
    mut hold: Local<f32>,
    mut seen: Local<[f32; 2]>,
    mut hp_tier: Local<usize>,
) {
    let Ok((vitals, dead, status)) = player.single() else {
        return;
    };
    let dt = real.delta_secs();
    let now = [
        vitals.hp / vitals.max_hp.max(1.0),
        vitals.ap / vitals.max_ap.max(1.0),
    ];
    let hp_frac = now[0];
    for k in 0..2 {
        if now[k] < seen[k] - 0.001 {
            *hold = 0.5;
        }
        seen[k] = now[k];
    }
    *hold = (*hold - dt).max(0.0);
    for k in 0..2 {
        // the bar jumps down at once, eases up; its trail waits, then drains
        shown[k] = if now[k] < shown[k] {
            now[k]
        } else {
            shown[k] + (now[k] - shown[k]) * (14.0 * dt).min(1.0)
        };
        let trail = &mut shown[2 + k];
        if *trail < now[k] {
            *trail = now[k];
        } else if *hold == 0.0 {
            *trail = (*trail - 0.5 * dt).max(now[k]);
        }
    }
    let low_hp = !dead && hp_frac < LOW_HP;
    let wash = !settings.screen_blood
        && profile
            .as_ref()
            .is_none_or(|p| p.graphics.get(Knob::LowHealth) == 0.0);
    let pulse = 0.5 + 0.5 * (real.elapsed_secs() * 8.0).sin();
    for (l, mut t, mut c) in &mut texts {
        let (s, color) = match l {
            Label::Hp => (
                format!("{}", vitals.hp.ceil()),
                if low_hp {
                    lerp(Color::WHITE, RED, 0.5 + 0.5 * pulse)
                } else {
                    Color::WHITE
                },
            ),
            Label::Ap => (format!("{}", vitals.ap.ceil()), c.0),
            Label::Status => (
                status.map_or_else(String::new, |s| {
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
                c.0,
            ),
            _ => continue,
        };
        set(&mut t, s);
        if c.0 != color {
            c.0 = color;
        }
    }
    for (f, mut n, mut g) in &mut fills {
        let frac = match f {
            Fill::Hp => shown[0],
            Fill::Ap => shown[1],
            Fill::HpTrail => shown[2],
            Fill::ApTrail => shown[3],
        };
        let w = percent(frac.clamp(0.0, 1.0) * 100.0);
        if n.width != w {
            n.width = w;
        }
        if *f == Fill::Hp {
            let t = tier(hp_frac);
            if t != *hp_tier {
                *hp_tier = t;
                *g = gradient(HP_TIERS[t]);
            }
        }
    }
    for mut tf in &mut pop {
        let s = 1.0 + 0.3 * (marks.flash / 0.4);
        tf.scale = Vec2::splat(s);
    }
    // Low health: a thin red edge vignette, only when neither the graphics page's low-health
    // effect nor the screen blood already tints the frame.
    let level = if low_hp && wash {
        1.0 - hp_frac / LOW_HP
    } else {
        0.0
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
            *g = vignette(0.4 * level * pulse);
        }
    }
}

/// The crosshair opens with running, jumping and every shot of a gun; hit and kill markers fade.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn reticle(
    marks: Res<Marks>,
    real: Res<Time<Real>>,
    data: Res<ActorData>,
    settings: Res<Settings>,
    mut fired: MessageReader<Fire>,
    player: Query<(Entity, &GlobalTransform, &Loadout, Has<Dead>), With<Player>>,
    mut ticks: Query<(&Tick, &mut UiTransform, &mut BackgroundColor)>,
    mut shows: Query<(&Show, &mut Visibility, &mut UiTransform), Without<Tick>>,
    // last position, smoothed speed (m/s), shot kick (px)
    mut state: Local<(Vec3, f32, f32)>,
) {
    let Ok((me, at, loadout, dead)) = player.single() else {
        return;
    };
    let dt = real.delta_secs().max(1e-4);
    let melee = data
        .items
        .get(loadout.slots[loadout.current].item)
        .is_none_or(|i| i.kind == "melee");
    for f in fired.read() {
        if f.shooter == me && !melee {
            state.2 = (state.2 + 5.0).min(14.0);
        }
    }
    let p = at.translation();
    let step = p - state.0;
    state.0 = p;
    let run = (step.xz().length() / dt / 10.0).min(1.0);
    let air = (step.y.abs() / dt / 3.0).min(1.0);
    state.1 += (run - state.1) * (10.0 * dt).min(1.0);
    state.2 = (state.2 - 40.0 * dt).max(0.0);
    let gap = if melee {
        10.0
    } else if settings.static_spread {
        6.0
    } else {
        6.0 + 7.0 * state.1 + 4.0 * air + state.2
    };
    let (hit, kill) = (marks.hit / 0.2, (marks.kill / 0.8).min(1.0));
    for (Tick(kind, dir), mut tf, mut bg) in &mut ticks {
        let (out, alpha) = match kind {
            Show::Cross => (gap, bg.0.alpha()),
            Show::Hit => (7.0, hit),
            _ => (10.0, kill),
        };
        let v = match dir {
            0 => Vec2::new(0.0, -out),
            1 => Vec2::new(out, 0.0),
            2 => Vec2::new(0.0, out),
            _ => Vec2::new(-out, 0.0),
        };
        tf.translation = Val2::px(v.x, v.y);
        if *kind != Show::Cross {
            bg.0 = bg.0.with_alpha(alpha);
        }
    }
    for (s, mut v, mut tf) in &mut shows {
        let (on, scale) = match s {
            Show::Cross => (!dead, 1.0),
            Show::Hit => (marks.hit > 0.0, 1.0 + 0.5 * hit),
            Show::Kill => (marks.kill > 0.0, 1.0 + 0.6 * marks.kill / 0.8),
            _ => continue,
        };
        let want = if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if *v != want {
            *v = want;
        }
        tf.scale = Vec2::splat(scale);
    }
}

/// Weapon name, magazine / reserve with the low-ammo warning, and the weapon strip.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn weapon(
    data: Res<ActorData>,
    hud: Res<Hud>,
    real: Res<Time<Real>>,
    player: Query<&Loadout, With<Player>>,
    mut texts: Query<(&Label, &mut Text, &mut TextColor)>,
    mut tiles: Query<(
        &Tile,
        &mut Node,
        &mut BorderColor,
        &mut BackgroundColor,
        &mut UiTransform,
    )>,
    mut icons: Query<(&mut Icon, &mut ImageNode)>,
) {
    let Ok(loadout) = player.single() else {
        return;
    };
    let slot = &loadout.slots[loadout.current];
    let item = data.items.get(slot.item);
    let melee = item.is_none_or(|i| i.kind == "melee");
    let cap = item
        .and_then(|i| i.weapon.as_ref())
        .map_or(0, |w| w.magazine)
        .max(1);
    let empty = !melee && slot.magazine == 0;
    let low = !melee && slot.magazine <= (cap / 4).max(1);
    let pulse = 0.5 + 0.5 * (real.elapsed_secs() * 8.0).sin();
    for (l, mut t, mut c) in &mut texts {
        let (s, color) = match l {
            Label::Weapon => (
                item.and_then(|i| i.name.clone())
                    .unwrap_or_else(|| format!("item {}", slot.item)),
                c.0,
            ),
            Label::Mag if melee => (String::new(), c.0),
            Label::Mag => (
                slot.magazine.to_string(),
                if empty {
                    RED
                } else if low {
                    lerp(Color::WHITE, RED, 0.4 + 0.6 * pulse)
                } else {
                    Color::WHITE
                },
            ),
            Label::Reserve if melee => (String::new(), c.0),
            Label::Reserve => (format!("/ {}", slot.reserve), c.0),
            Label::Hint => (
                match (empty, slot.reserve) {
                    (false, _) => "",
                    (true, 0) => "NO AMMO",
                    (true, _) => "RELOAD",
                }
                .to_owned(),
                if slot.reserve == 0 {
                    RED
                } else {
                    lerp(ACCENT, Color::WHITE, pulse)
                },
            ),
            _ => continue,
        };
        set(&mut t, s);
        if c.0 != color {
            c.0 = color;
        }
    }
    for (tile, mut n, mut border, mut bg, mut tf) in &mut tiles {
        let display = if tile.0 < loadout.slots.len() {
            Display::Flex
        } else {
            Display::None
        };
        if n.display != display {
            n.display = display;
        }
        let on = tile.0 == loadout.current;
        let (fill, edge, y, scale) = if on {
            (ACCENT.with_alpha(0.22), ACCENT, -6.0, 1.1)
        } else {
            (PANEL, EDGE, 0.0, 1.0)
        };
        if bg.0 != fill {
            bg.0 = fill;
        }
        let edge = BorderColor::all(edge);
        if *border != edge {
            *border = edge;
        }
        tf.translation = Val2::px(0.0, y);
        tf.scale = Vec2::splat(scale);
    }
    for (mut icon, mut img) in &mut icons {
        let Some(s) = loadout.slots.get(icon.0) else {
            continue;
        };
        if icon.1 != s.item {
            icon.1 = s.item;
            *img = hud.icons.node(&hud.shop, s.item);
        }
    }
}

/// The player's counters, the header, the death screen and the scoreboard.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn scores(
    input: Input,
    clock: Option<Res<Clock>>,
    player: Query<(&Score, Option<&Dead>), With<Player>>,
    actors: Query<(&Name, &Score, Has<Player>, Option<&Team>)>,
    mut texts: Query<(&Label, &mut Text), Without<Cell>>,
    mut shows: Query<(&Show, &mut Visibility), (Without<Tick>, Without<Row>)>,
    mut rows: Query<(&Row, &mut Node, &mut BackgroundColor), Without<Stripe>>,
    mut stripes: Query<(&Stripe, &mut BackgroundColor), Without<Row>>,
    mut cells: Query<(&Cell, &mut Text, &mut TextColor), Without<Label>>,
    mut line: Query<&mut Node, (With<HeaderLine>, Without<Row>)>,
) {
    for mut n in &mut line {
        let want = if clock.as_ref().is_some_and(|c| !c.header.is_empty()) {
            Display::Flex
        } else {
            Display::None
        };
        if n.display != want {
            n.display = want;
        }
    }
    let Ok((score, dead)) = player.single() else {
        return;
    };
    let over = clock.as_ref().is_some_and(|c| c.over.is_some());
    for (l, mut t) in &mut texts {
        let s = match l {
            Label::Kills => score.kills.to_string(),
            Label::Deaths => score.deaths.to_string(),
            Label::Clock => clock
                .as_ref()
                .map_or_else(String::new, |c| c.header.clone()),
            Label::Timer => clock.as_ref().map_or_else(String::new, |c| c.timer.clone()),
            Label::Death => dead.map_or(String::new(), |d| {
                if d.respawn >= HOLD {
                    "Waiting to rejoin".to_owned()
                } else {
                    format!("Respawn in {}", d.respawn.ceil())
                }
            }),
            _ => continue,
        };
        set(&mut t, s);
    }
    for (s, mut v) in &mut shows {
        let on = match s {
            Show::Death => dead.is_some() && !over,
            Show::Board => input.pressed(Action::Score) || over,
            _ => continue,
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
    let mut list: Vec<_> = actors.iter().collect();
    list.sort_by_key(|(_, s, _, _)| (std::cmp::Reverse(s.kills), s.deaths));
    for (Row(i), mut n, mut bg) in &mut rows {
        let (display, fill) = match list.get(*i) {
            None => (Display::None, Color::NONE),
            Some((_, _, true, _)) => (Display::Flex, ACCENT.with_alpha(0.16)),
            Some(_) if i % 2 == 0 => (Display::Flex, Color::srgba(1.0, 1.0, 1.0, 0.04)),
            Some(_) => (Display::Flex, Color::NONE),
        };
        if n.display != display {
            n.display = display;
        }
        if bg.0 != fill {
            bg.0 = fill;
        }
    }
    for (Stripe(i), mut bg) in &mut stripes {
        let c = match list.get(*i).and_then(|r| r.3) {
            Some(Team::Red) => RED,
            Some(Team::Blue) => Color::srgb(0.48, 0.68, 1.0),
            _ => Color::NONE,
        };
        if bg.0 != c {
            bg.0 = c;
        }
    }
    for (Cell(i, c), mut t, mut color) in &mut cells {
        let Some((name, s, you, _)) = list.get(*i) else {
            continue;
        };
        let text = match c {
            0 => (i + 1).to_string(),
            1 if *you => format!("{name} (you)"),
            1 => name.to_string(),
            2 => s.kills.to_string(),
            3 => s.deaths.to_string(),
            _ => format!("{:.2}", s.kills as f32 / s.deaths.max(1) as f32),
        };
        set(&mut t, text);
        let want = if *you { ACCENT } else { Color::WHITE };
        if color.0 != want {
            color.0 = want;
        }
    }
}

/// The notice pops in, holds and fades out.
pub(super) fn banner(
    marks: Res<Marks>,
    real: Res<Time<Real>>,
    banner: Single<(&mut Text, &mut TextColor, &mut UiTransform), With<Banner>>,
    mut age: Local<(f32, f32)>,
) {
    let (mut text, mut color, mut tf) = banner.into_inner();
    let (left, line) = &marks.notice;
    // a fresh notice (more time left than last frame) restarts the pop
    if *left > age.1 {
        age.0 = 0.0;
    }
    age.0 += real.delta_secs();
    age.1 = *left;
    set(
        &mut text,
        if *left > 0.0 {
            line.clone()
        } else {
            String::new()
        },
    );
    tf.scale = Vec2::splat(1.0 + 0.3 * (1.0 - age.0 / 0.18).max(0.0));
    color.0 = ACCENT.with_alpha((left / 0.35).min(1.0));
}

pub(super) fn earned(
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
