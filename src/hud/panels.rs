//! The in-match HUD panels (Bevy UI over the 3D view): the notice banner and the scoreboard. A
//! child of `hud.rs`; the nodes are spawned once and only rewritten afterwards.

use super::{Marks, Root, set};
use crate::{
    controls::{Action, Input},
    game::{Frozen, Player, Score, Team},
    profile::{MatchGain, Profile, Ranks},
    session::Clock,
    ui::{self, ACCENT, BLUE, Cut, INK, ON_ACCENT, RAISED, RED, SURFACE},
};
use bevy::{
    prelude::*,
    ui::{GlobalZIndex, UiTargetCamera},
};

/// Rows of the scoreboard.
const BOARD_ROWS: usize = 12;

/// Elements shown only in some states.
#[derive(Component, Clone, Copy, PartialEq)]
pub(super) enum Show {
    Board,
}

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

/// The scoreboard panel (`clan.rs` hangs its strip on it).
#[derive(Component)]
pub struct BoardRoot;

/// The scoreboard's last line once the match is over: level, XP and bounty earned (`profile.rs`).
#[derive(Component)]
pub(super) struct Earned;

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
        // Notice banner: upper-centre, display caps, fast pop.
        r.spawn((
            Node {
                position_type: PositionType::Absolute,
                top: percent(14),
                left: px(0),
                width: percent(100),
                justify_content: JustifyContent::Center,
                ..default()
            },
            children![(
                Banner,
                UiTransform::default(),
                Text::new(""),
                ui::display(26.0),
                TextColor(ACCENT),
                ui::shadow(),
            )],
        ));
        // Scoreboard: 900 wide on the 1920 canvas; `board_fit` places and scales it (the clan
        // strip sits on its top right).
        r.spawn((
            Show::Board,
            BoardRoot,
            // above the end screen's scrim (`session.rs`, 10) and the reward panel
            GlobalZIndex(11),
            Visibility::Hidden,
            UiTransform::default(),
            Node {
                position_type: PositionType::Absolute,
                left: percent(50),
                width: px(900),
                padding: UiRect::all(px(22)),
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                ..default()
            },
            ui::shape(Cut::Chamfer(18), SURFACE),
        ))
        .with_children(|b| {
            b.spawn(ui::section(1, "Scoreboard"));
            let cols = [
                ("#", 50.0),
                ("PLAYER", 400.0),
                ("KILLS", 120.0),
                ("DEATHS", 120.0),
                ("K/D", 110.0),
            ];
            b.spawn(Node {
                height: px(28),
                padding: UiRect::horizontal(px(16)),
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
                        children![ui::caption(name, 14.0)],
                    ));
                }
            });
            for i in 0..BOARD_ROWS {
                b.spawn((
                    Row(i),
                    Node {
                        height: px(40),
                        padding: UiRect::horizontal(px(16)),
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    ui::shape(Cut::Notch(12), RAISED),
                ))
                .with_children(|row| {
                    row.spawn((
                        Stripe(i),
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(0),
                            top: px(8),
                            width: px(4),
                            height: px(24),
                            ..default()
                        },
                        BackgroundColor(Color::NONE),
                    ));
                    for (c, (_, w)) in cols.iter().enumerate() {
                        let font = if c == 1 {
                            ui::strong(22.0)
                        } else {
                            ui::digits(22.0)
                        };
                        row.spawn((
                            Node {
                                width: px(*w),
                                overflow: Overflow::clip(),
                                ..default()
                            },
                            children![(
                                Cell(i, c),
                                Text::new(""),
                                font,
                                TextColor(INK),
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
                ui::compact(22.0),
                TextColor(ACCENT),
                Node {
                    margin: UiRect::top(px(8)),
                    ..default()
                },
            ));
        });
    });
}

/// Frame-to-frame state of the panels of one match: the notice pop age and the time left last
/// frame ([`banner`]).
#[derive(Resource, Default)]
pub(super) struct Smooth {
    banner: (f32, f32),
}

/// The scoreboard: its rows and the keys that show it.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub(super) fn scores(
    input: Input,
    clock: Option<Res<Clock>>,
    actors: Query<(&Name, &Score, Has<Player>, Option<&Team>)>,
    mut shows: Query<(&Show, &mut Visibility), Without<Row>>,
    mut rows: Query<(&Row, &mut Node, &mut ImageNode), Without<Stripe>>,
    mut stripes: Query<(&Stripe, &mut BackgroundColor), Without<Row>>,
    mut cells: Query<(&Cell, &mut Text, &mut TextColor)>,
) {
    let over = clock.as_ref().is_some_and(|c| c.over.is_some());
    for (_, mut v) in &mut shows {
        let on = input.pressed(Action::Score) || over;
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
    for (Row(i), mut n, mut img) in &mut rows {
        let (display, fill) = match list.get(*i) {
            None => (Display::None, RAISED),
            Some((_, _, true, _)) => (Display::Flex, ACCENT),
            Some(_) => (Display::Flex, RAISED),
        };
        if n.display != display {
            n.display = display;
        }
        if img.color != fill {
            img.color = fill;
        }
    }
    // Team colour, except the player's own row (filled with the accent): its stripe is dark.
    for (Stripe(i), mut bg) in &mut stripes {
        let c = match list.get(*i) {
            Some((_, _, true, _)) => ON_ACCENT,
            Some((_, _, _, Some(Team::Red))) => RED,
            Some((_, _, _, Some(Team::Blue))) => BLUE,
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
        let want = if *you { ON_ACCENT } else { INK };
        if color.0 != want {
            color.0 = want;
        }
    }
}

/// The board is a 1920-canvas panel: while the match is over it hangs below the headline at full
/// size, under the HUD scale (Tab) it sits in the middle of the screen at 0.8 so it keeps the
/// size it had at 1280.
pub(super) fn board_fit(
    frozen: Option<Res<Frozen>>,
    mut board: Query<(&Show, &mut Node, &mut UiTransform)>,
) {
    let (top, y, scale) = if frozen.is_some() {
        (px(230), px(0), 1.0)
    } else {
        (percent(50), percent(-50), 0.8)
    };
    for (show, mut node, mut tf) in &mut board {
        if *show != Show::Board {
            continue;
        }
        let translation = Val2::new(percent(-50), y);
        if node.top != top {
            node.top = top;
        }
        if tf.translation != translation {
            tf.translation = translation;
        }
        if tf.scale.x != scale {
            tf.scale = Vec2::splat(scale);
        }
    }
}

/// The notice pops in, holds and fades out, in caps.
pub(super) fn banner(
    marks: Res<Marks>,
    real: Res<Time<Real>>,
    banner: Single<(&mut Text, &mut TextColor, &mut UiTransform), With<Banner>>,
    mut st: ResMut<Smooth>,
) {
    let age = &mut st.banner;
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
            line.to_uppercase()
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
