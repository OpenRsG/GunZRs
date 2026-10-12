//! Top of the HUD (the "GunZ HUD" mock): at the top left the mode and map with the player's kills,
//! deaths and K/D; at the top centre the compass, the clock between two score chips (YOU / LEAD in
//! free-for-all modes, RED / BLUE in team modes) and the mode's status line ("FIRST TO 50", the
//! round, ...). The blitz HUD (`blitz/ui.rs`) sits lower, so nothing here overlaps it.

use super::{Root, set};
use crate::{
    game::{Player, Score},
    menu::Mode,
    phase::{Launch, MatchSystems},
    session::{Clock, Rules},
    ui::{self, ACCENT, BLUE, Cut, DIM, INK, ON_ACCENT, PLATE, RED},
};
use bevy::{prelude::*, text::LineHeight};

/// The mock's off-white caps (`#c4c7cc`), the lit "N" (`#ff7a2e`) and the red clock (`#ff3b30`).
const SOFT: Color = Color::srgb(0.769, 0.78, 0.8);
const HOT: Color = Color::srgb(1.0, 0.478, 0.18);
const LOW: Color = Color::srgb(1.0, 0.231, 0.188);

/// Compass geometry (px): strip width, pixels per degree, ticks every `STEP` degrees, how far
/// from the centre they show, and the fade-in length at each end.
const WIDTH: f32 = 380.0;
const PER_DEG: f32 = 3.0;
const STEP: i32 = 15;
const REACH: f32 = 63.0;
const FADE: f32 = 68.0;
/// Width of a tick's box (its label is centred in it).
const SLOT: f32 = 40.0;

/// Marks the top-left block: [`spawn`] runs once.
#[derive(Component)]
struct Top;

/// A text of [`readout`].
#[derive(Component, Clone, Copy)]
enum Slot {
    Mode,
    Kills,
    Deaths,
    Kd,
    You,
    Lead,
    Red,
    Blue,
    Timer,
    Line,
}

/// A part that only shows in some modes ([`readout`] sets its `display`).
#[derive(Component, Clone, Copy)]
enum Part {
    /// YOU / LEAD chips: free-for-all kill counts.
    Ffa,
    /// RED / BLUE chips.
    Teams,
    Clock,
    Line,
}

/// A compass tick at this many degrees (a multiple of [`STEP`]); its children are the label and
/// the bar.
#[derive(Component)]
struct Tick(i32);

/// The bearing's three digits.
#[derive(Component)]
struct Heading;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<Top>))),
            (readout, compass),
        )
            .in_set(MatchSystems),
    );
}

/// Which sides of a slab slant: both (a parallelogram), or one with the other upright.
#[derive(Clone, Copy)]
pub(super) enum Edge {
    Both,
    Left,
    Right,
}

/// A box its text children sit on; its [`back`] (the first child) is cut off at the edges.
fn slab(pad: UiRect, shown: bool) -> impl Bundle {
    Node {
        display: if shown { Display::Flex } else { Display::None },
        align_items: AlignItems::FlexEnd,
        column_gap: px(6),
        padding: pad,
        overflow: Overflow::clip(),
        ..default()
    }
}

/// The fill under a clipped box's content in the `Slant(cut)` shape: the upright side's slant
/// spills `cut` px past the box, where the clip trims it.
pub(super) fn back(color: Color, edge: Edge, cut: u8) -> impl Bundle {
    let spill = -f32::from(cut);
    let (l, r) = match edge {
        Edge::Both => (0.0, 0.0),
        Edge::Left => (0.0, spill),
        Edge::Right => (spill, 0.0),
    };
    (
        Node {
            position_type: PositionType::Absolute,
            left: px(l),
            right: px(r),
            top: px(0),
            bottom: px(0),
            ..default()
        },
        ui::shape(Cut::Slant(cut), color),
    )
}

/// A letter-spaced caps caption (KILLS, YOU, ...), `lift` px above the slab's bottom so it shares
/// the baseline of the number beside it.
fn cap(s: &str, size: f32, color: Color, lift: f32) -> impl Bundle {
    (
        Text::new(s),
        ui::strong(size),
        ui::tracking(0.2, size),
        TextColor(color),
        Node {
            margin: UiRect::bottom(px(lift)),
            ..default()
        },
    )
}

/// A number the [`readout`] fills.
fn val(slot: Slot, font: TextFont, color: Color) -> impl Bundle {
    (
        slot,
        Text::new(""),
        font,
        TextColor(color),
        LineHeight::RelativeToFont(1.0),
    )
}

/// One compass tick: its label (a direction in display, a number in mono) over a bar.
fn tick(c: &mut ChildSpawnerCommands, deg: i32) {
    let (label, font, bar) = if deg % 45 == 0 {
        let name = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"][(deg / 45) as usize];
        let size = if deg % 90 == 0 { 16.0 } else { 13.0 };
        (name.to_owned(), ui::display(size), 8.0)
    } else {
        (deg.to_string(), ui::mono(9.0), 4.0)
    };
    c.spawn((
        Tick(deg),
        Node {
            display: Display::None,
            position_type: PositionType::Absolute,
            bottom: px(0),
            width: px(SLOT),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            row_gap: px(2),
            ..default()
        },
        children![
            (
                Text::new(label),
                font,
                LineHeight::RelativeToFont(1.0),
                TextColor(tick_color(deg)),
            ),
            (
                Node {
                    width: px(2),
                    height: px(bar),
                    ..default()
                },
                BackgroundColor(tick_color(deg)),
            ),
        ],
    ));
}

fn tick_color(deg: i32) -> Color {
    match deg {
        0 => HOT,
        d if d % 45 == 0 => Color::WHITE,
        _ => Color::srgba(1.0, 1.0, 1.0, 0.6),
    }
}

fn spawn(mut commands: Commands, root: Single<Entity, With<Root>>) {
    // top left: the mode line over the player's plates
    commands
        .spawn((
            Top,
            ChildOf(*root),
            Node {
                position_type: PositionType::Absolute,
                left: px(28),
                top: px(24),
                flex_direction: FlexDirection::Column,
                row_gap: px(8),
                ..default()
            },
        ))
        .with_children(|c| {
            c.spawn(Node {
                align_items: AlignItems::Center,
                column_gap: px(10),
                ..default()
            })
            .with_children(|r| {
                r.spawn((
                    Node {
                        width: px(22),
                        height: px(8),
                        ..default()
                    },
                    ui::hatch(ACCENT),
                ));
                r.spawn((
                    Slot::Mode,
                    Text::new(""),
                    ui::strong(11.0),
                    ui::tracking(0.3, 11.0),
                    TextColor(SOFT),
                    ui::shadow(),
                ));
            });
            c.spawn(Node {
                column_gap: px(4),
                ..default()
            })
            .with_children(|r| {
                let pad = |a: f32, b: f32| UiRect::new(px(a), px(b), px(4), px(4));
                r.spawn((
                    slab(pad(12.0, 16.0), true),
                    children![
                        back(PLATE, Edge::Right, 8),
                        cap("KILLS", 10.0, DIM, 3.0),
                        val(Slot::Kills, ui::display(28.0), INK)
                    ],
                ));
                r.spawn((
                    slab(pad(14.0, 16.0), true),
                    children![
                        back(PLATE, Edge::Both, 8),
                        cap("DEATHS", 10.0, DIM, 3.0),
                        val(
                            Slot::Deaths,
                            ui::display(28.0),
                            Color::srgb(0.725, 0.737, 0.761)
                        )
                    ],
                ));
                r.spawn((
                    slab(pad(14.0, 16.0), true),
                    children![
                        back(ACCENT.with_alpha(0.9), Edge::Both, 8),
                        cap("K/D", 10.0, ON_ACCENT, 2.0),
                        val(Slot::Kd, ui::digits(18.0), ON_ACCENT)
                    ],
                ));
            });
        });

    // top centre: compass, bearing, clock row, status line
    commands
        .spawn((
            ChildOf(*root),
            Node {
                position_type: PositionType::Absolute,
                top: px(16),
                width: percent(100),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(6),
                ..default()
            },
        ))
        .with_children(|c| {
            c.spawn(Node {
                width: px(WIDTH),
                height: px(30),
                overflow: Overflow::clip(),
                ..default()
            })
            .with_children(|s| {
                // the baseline, faded out at both ends like the ticks
                let line = Color::srgba(1.0, 1.0, 1.0, 0.15);
                let end = FADE / WIDTH * 100.0;
                s.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        right: px(0),
                        bottom: px(0),
                        height: px(1),
                        ..default()
                    },
                    BackgroundGradient::from(LinearGradient::to_right(vec![
                        ColorStop::new(line.with_alpha(0.0), percent(0)),
                        ColorStop::new(line, percent(end)),
                        ColorStop::new(line, percent(100.0 - end)),
                        ColorStop::new(line.with_alpha(0.0), percent(100)),
                    ])),
                ));
                for k in 0..360 / STEP {
                    tick(s, k * STEP);
                }
            });
            // the caret: a square turned 45 degrees, clipped to its upper half
            c.spawn(Node {
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|r| {
                r.spawn((
                    Node {
                        width: px(12),
                        height: px(7),
                        margin: UiRect::right(px(8)),
                        overflow: Overflow::clip(),
                        ..default()
                    },
                    children![(
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(1.76),
                            top: px(1.76),
                            width: px(8.49),
                            height: px(8.49),
                            ..default()
                        },
                        BackgroundColor(ACCENT),
                        UiTransform::from_rotation(Rot2::degrees(45.0)),
                    )],
                ));
                r.spawn((
                    Node {
                        width: px(34),
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    children![(Heading, Text::new(""), ui::digits(13.0), TextColor(HOT))],
                ));
            });
            c.spawn((
                Part::Clock,
                Node {
                    display: Display::None,
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    margin: UiRect::top(px(2)),
                    ..default()
                },
            ))
            .with_children(|r| {
                let left = UiRect::new(px(10), px(14), px(3), px(3));
                let right = UiRect::new(px(14), px(10), px(3), px(3));
                r.spawn((
                    Part::Ffa,
                    slab(left, false),
                    children![
                        back(ACCENT.with_alpha(0.9), Edge::Right, 8),
                        cap("YOU", 9.0, ON_ACCENT, 4.0),
                        val(Slot::You, ui::display(20.0), ON_ACCENT)
                    ],
                ));
                r.spawn((
                    Part::Teams,
                    slab(left, false),
                    children![
                        back(RED.with_alpha(0.9), Edge::Right, 8),
                        cap("RED", 9.0, ON_ACCENT, 4.0),
                        val(Slot::Red, ui::display(20.0), ON_ACCENT)
                    ],
                ));
                r.spawn((
                    Slot::Timer,
                    Text::new(""),
                    ui::display(34.0),
                    ui::tracking(0.04, 34.0),
                    LineHeight::RelativeToFont(0.9),
                    TextColor(INK),
                    ui::shadow(),
                ));
                r.spawn((
                    Part::Ffa,
                    slab(right, false),
                    children![
                        back(PLATE.with_alpha(0.65), Edge::Left, 8),
                        cap("LEAD", 9.0, DIM, 4.0),
                        val(Slot::Lead, ui::display(20.0), INK)
                    ],
                ));
                r.spawn((
                    Part::Teams,
                    slab(right, false),
                    children![
                        back(BLUE.with_alpha(0.9), Edge::Left, 8),
                        cap("BLUE", 9.0, ON_ACCENT, 4.0),
                        val(Slot::Blue, ui::display(20.0), ON_ACCENT)
                    ],
                ));
            });
            c.spawn((
                Part::Line,
                Node {
                    display: Display::None,
                    ..default()
                },
                children![(
                    Slot::Line,
                    Text::new(""),
                    ui::strong(9.0),
                    ui::tracking(0.3, 9.0),
                    TextColor(DIM),
                    ui::shadow(),
                )],
            ));
        });
}

/// Top left and the clock row: the player's score, the chips, the clock and the status line.
#[allow(clippy::type_complexity)]
fn readout(
    rules: Option<Res<Rules>>,
    clock: Option<Res<Clock>>,
    launch: Option<Res<Launch>>,
    me: Query<&Score, With<Player>>,
    others: Query<&Score, Without<Player>>,
    mut texts: Query<(&Slot, &mut Text, &mut TextColor)>,
    mut parts: Query<(&Part, &mut Node), (Without<Slot>, Without<Tick>)>,
) {
    let score = me.single().map_or((0, 0), |s| (s.kills, s.deaths));
    let lead = others.iter().map(|s| s.kills).max().unwrap_or(0);
    let mode = rules.as_ref().map(|r| r.mode);
    let ffa = matches!(
        mode,
        Some(Mode::Deathmatch | Mode::Gladiator | Mode::Gunman | Mode::Berserker | Mode::GunGame)
    );
    let (timer, line, teams) = clock.as_ref().map_or(("", "", None), |c| {
        (c.timer.as_str(), c.header.as_str(), c.team_score)
    });
    // the red clock of the last minute only for a clock that counts down
    let counts_down = rules
        .as_ref()
        .is_some_and(|r| r.time_limit.is_some() || r.mode.rounds());
    let place = launch.as_ref().and_then(|l| l.config.map.as_deref());
    for (slot, mut t, mut color) in &mut texts {
        let s = match slot {
            Slot::Mode => mode.map_or_else(String::new, |m| match place {
                Some(map) => format!("{} · {map}", m.name()).to_uppercase(),
                None => m.name().to_uppercase(),
            }),
            Slot::Kills | Slot::You => score.0.to_string(),
            Slot::Deaths => score.1.to_string(),
            Slot::Kd => format!("{:.2}", score.0 as f32 / score.1.max(1) as f32),
            Slot::Lead => lead.to_string(),
            Slot::Red => teams.map_or_else(String::new, |t| t.0.to_string()),
            Slot::Blue => teams.map_or_else(String::new, |t| t.1.to_string()),
            Slot::Timer => {
                let want = if counts_down && timer.starts_with("0:") {
                    LOW
                } else {
                    INK
                };
                if color.0 != want {
                    color.0 = want;
                }
                timer.to_owned()
            }
            Slot::Line => line.to_owned(),
        };
        set(&mut t, s);
    }
    for (part, mut n) in &mut parts {
        let on = match part {
            Part::Ffa => ffa,
            Part::Teams => teams.is_some(),
            Part::Clock => !timer.is_empty(),
            Part::Line => !line.is_empty(),
        };
        let want = if on { Display::Flex } else { Display::None };
        if n.display != want {
            n.display = want;
        }
    }
}

/// Compass bearing in degrees, 0..360: 0 is N, the direction yaw 0 faces (world -Z), and it grows
/// clockwise seen from above, so +X is E (90), +Z S (180) and -X W (270).
fn bearing(forward: Vec3) -> f32 {
    forward.x.atan2(-forward.z).to_degrees().rem_euclid(360.0)
}

/// Slides the ticks under the caret and fades them towards both ends; writes the bearing.
fn compass(
    camera: Query<&GlobalTransform, With<Camera3d>>,
    mut ticks: Query<(&Tick, &mut Node, &Children)>,
    mut heading: Query<&mut Text, With<Heading>>,
    mut labels: Query<&mut TextColor, Without<Heading>>,
    mut bars: Query<&mut BackgroundColor>,
) {
    let Ok(cam) = camera.single() else {
        return;
    };
    let b = bearing(*cam.forward());
    for mut t in &mut heading {
        set(&mut t, format!("{:03}", b.round() as u32 % 360));
    }
    for (Tick(deg), mut node, kids) in &mut ticks {
        let off = (*deg as f32 - b + 540.0).rem_euclid(360.0) - 180.0;
        let on = off.abs() <= REACH;
        let display = if on { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
        if !on {
            continue;
        }
        let x = WIDTH / 2.0 + off * PER_DEG;
        node.left = px(x - SLOT / 2.0);
        let base = tick_color(*deg);
        let c = base.with_alpha(base.alpha() * (x.min(WIDTH - x) / FADE).clamp(0.0, 1.0));
        if let (Some(&label), Some(&bar)) = (kids.first(), kids.get(1)) {
            if let Ok(mut t) = labels.get_mut(label) {
                t.0 = c;
            }
            if let Ok(mut bg) = bars.get_mut(bar) {
                bg.0 = c;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bearing_runs_clockwise_from_north() {
        let at = |v: Vec3| bearing(v).round() as u32 % 360;
        assert_eq!(at(Vec3::NEG_Z), 0);
        assert_eq!(at(Vec3::X), 90);
        assert_eq!(at(Vec3::Z), 180);
        assert_eq!(at(Vec3::NEG_X), 270);
    }
}
