//! What every menu page shares: the hazard band, the vignette and the page's watermark word, the
//! header (logo, F1-F7 tabs, bounty, medals, XP bar, level badge), the footer (ESC QUIT, the match
//! summary, JOIN LAN, HOST LAN, START) and the toasts. Layout: `DESIGN.md`.

use super::*;
use crate::ui::{
    self, ACCENT, Chosen, Cut, DIM, GOLD, INK, MUTE, ON_ACCENT, OnFill, RAISED, SURFACE, TEXT,
    caption, digits, ease_out, flat_cut, hatch, hazard, key, metal, mono, segs, shape, tracking,
};

/// The menu's root node: toasts are spawned under it.
#[derive(Component)]
pub(super) struct Root;

/// A top-bar page tab; [`tab_look`] paints it from [`Interaction`] and [`Chosen`].
#[derive(Component)]
pub(super) struct NavTab;

/// The QUIT button: [`quiet_look`] lights its label on hover.
#[derive(Component)]
pub(super) struct Quiet;

/// The giant faint word behind a page ([`mark`] shows the current page's).
#[derive(Component)]
pub(super) struct Watermark(Page);

/// A toast on screen and its age in seconds.
#[derive(Component)]
pub(super) struct ToastView(f32);

const TOAST_SECS: f32 = 2.2;
pub(super) const TABS: [(&str, Page); 7] = [
    ("MATCH", Page::Match),
    ("PLAYER", Page::Player),
    ("SHOP", Page::Shop),
    ("INVENTORY", Page::Inventory),
    ("CLAN", Page::Clan),
    ("CONTROLS", Page::Controls),
    ("GRAPHICS", Page::Graphics),
];

/// 20240 -> `20,240`.
fn grouped(n: impl ToString) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}

fn abs() -> Node {
    Node {
        position_type: PositionType::Absolute,
        ..default()
    }
}

/// Everything behind the pages, then the header. Spawn it first so the pages draw over it.
pub(super) fn header(
    root: &mut ChildSpawnerCommands,
    logo: Handle<Image>,
    profile: &Profile,
    page: Page,
) {
    let (level, into, need) = progress(profile.xp);
    let frac = if need == 0 {
        1.0
    } else {
        (into as f32 / need as f32).clamp(0.0, 1.0)
    };
    // the vignette, and the footer's shade
    root.spawn((
        Node {
            width: percent(100),
            height: percent(100),
            ..abs()
        },
        BackgroundGradient::from(RadialGradient::new(
            UiPosition::CENTER,
            RadialGradientShape::ClosestCorner,
            vec![
                ColorStop::new(Color::NONE, percent(50)),
                ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.6), percent(100)),
            ],
        )),
    ));
    root.spawn((
        Node {
            width: percent(100),
            height: percent(100),
            ..abs()
        },
        hatch(Color::srgba(1.0, 1.0, 1.0, 0.0025)),
    ));
    root.spawn((
        Node {
            bottom: px(0),
            width: percent(100),
            height: px(128),
            ..abs()
        },
        BackgroundGradient::from(LinearGradient::to_bottom(vec![
            ColorStop::new(Color::NONE, percent(0)),
            ColorStop::new(Color::srgba(0.0, 0.0, 0.0, 0.55), percent(100)),
        ])),
    ));
    for (p, word) in [
        (Page::Match, "DUEL"),
        (Page::Shop, "ARMORY"),
        (Page::Player, "PLAYER"),
        (Page::Inventory, "INVENTORY"),
        (Page::Clan, "CLAN"),
        (Page::Controls, "CONTROLS"),
        (Page::Graphics, "GRAPHICS"),
    ] {
        root.spawn((
            Watermark(p),
            Node {
                right: px(-60),
                bottom: px(90),
                display: if p == page {
                    Display::Flex
                } else {
                    Display::None
                },
                ..abs()
            },
            Text::new(word),
            ui::display(420.0),
            tracking(-0.02, 420.0),
            bevy::text::LineHeight::RelativeToFont(0.8),
            TextColor(Color::srgba(1.0, 1.0, 1.0, 0.003)),
        ));
    }
    root.spawn((
        Node {
            top: px(0),
            width: percent(100),
            height: px(8),
            ..abs()
        },
        hazard(),
    ));
    root.spawn((
        ImageNode::new(logo),
        Node {
            left: px(36),
            top: px(14),
            width: px(180),
            height: px(75),
            ..abs()
        },
    ));
    root.spawn(Node {
        left: px(244),
        top: px(27),
        column_gap: px(6),
        ..abs()
    })
    .with_children(|t| {
        for (i, (name, page)) in TABS.into_iter().enumerate() {
            t.spawn((
                Act::Page(page),
                NavTab,
                Button,
                Node {
                    height: px(64),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::axes(px(22), px(8)),
                    ..default()
                },
                shape(Cut::Slant(10), Color::NONE),
                children![
                    (
                        Text::new(format!("F{}", i + 1)),
                        mono(11.0),
                        TextColor(MUTE)
                    ),
                    (
                        Text::new(name),
                        ui::display(28.0),
                        tracking(0.06, 28.0),
                        TextColor(DIM)
                    ),
                    (
                        Node {
                            height: px(3),
                            margin: UiRect::top(px(2)),
                            ..default()
                        },
                        BackgroundColor(Color::NONE)
                    ),
                ],
            ));
        }
    });
    // bounty, medals and the XP bar, the name, the level badge
    root.spawn(Node {
        top: px(25),
        right: px(38),
        height: px(68),
        align_items: AlignItems::Center,
        column_gap: px(24),
        ..abs()
    })
    .with_children(|s| {
        s.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            ..default()
        })
        .with_children(|c| {
            c.spawn(Node {
                align_items: AlignItems::Center,
                column_gap: px(12),
                ..default()
            })
            .with_children(|r| {
                r.spawn(caption("BOUNTY", 12.0));
                r.spawn((
                    Text::new(grouped(profile.bounty)),
                    digits(22.0),
                    TextColor(GOLD),
                ));
                r.spawn(Node {
                    width: px(20),
                    ..default()
                });
                r.spawn(caption("MEDALS", 12.0));
                r.spawn((
                    Text::new(grouped(profile.medals)),
                    digits(22.0),
                    TextColor(INK),
                ));
            });
            c.spawn(Node {
                align_items: AlignItems::Center,
                column_gap: px(14),
                ..default()
            })
            .with_children(|r| {
                r.spawn(Node {
                    width: px(240),
                    ..default()
                })
                .with_child(segs(10, (frac * 10.0).round() as usize, 5.0, 2.0));
                r.spawn((
                    Text::new(format!("{into}/{need} XP")),
                    mono(13.0),
                    TextColor(TEXT),
                ));
            });
        });
        s.spawn((
            Text::new(profile.name.to_uppercase()),
            ui::display(38.0),
            tracking(0.04, 38.0),
            TextColor(INK),
        ));
        s.spawn(Node {
            width: px(68),
            height: px(68),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            flex_direction: FlexDirection::Column,
            ..default()
        })
        .with_children(|b| {
            b.spawn((
                Node {
                    left: px(11),
                    top: px(11),
                    width: px(46),
                    height: px(46),
                    border: UiRect::all(px(3)),
                    ..abs()
                },
                UiTransform {
                    rotation: Rot2::degrees(45.0),
                    ..default()
                },
                BackgroundColor(RAISED),
                BorderColor::all(ACCENT),
            ));
            b.spawn((Text::new("LV"), mono(9.0), TextColor(ACCENT)));
            b.spawn((
                Text::new(level.to_string()),
                digits(24.0),
                TextColor(ACCENT),
            ));
        });
    });
}

/// ESC QUIT, the match summary, JOIN LAN, HOST LAN and START.
pub(super) fn footer(
    root: &mut ChildSpawnerCommands,
    cfg: &Config,
    cat: &Catalog,
    profile: &Profile,
) {
    root.spawn(Node {
        left: px(60),
        right: px(40),
        bottom: px(28),
        height: px(88),
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        ..abs()
    })
    .with_children(|f| {
        // the browser page is this menu: nothing to quit to (the node keeps START right)
        if cfg!(target_arch = "wasm32") {
            f.spawn(Node::default());
        } else {
            f.spawn((
                Act::Quit,
                Quiet,
                Button,
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(12),
                    padding: UiRect::axes(px(4), px(10)),
                    ..default()
                },
                children![
                    key("ESC"),
                    (
                        Text::new("QUIT"),
                        ui::display(24.0),
                        tracking(0.14, 24.0),
                        TextColor(DIM)
                    )
                ],
            ));
        }
        f.spawn(Node {
            align_items: AlignItems::Center,
            column_gap: px(16),
            ..default()
        })
        .with_children(|r| {
            r.spawn(Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                row_gap: px(6),
                margin: UiRect::right(px(20)),
                ..default()
            })
            .with_children(|s| {
                s.spawn(caption("READY TO DEPLOY", 11.0));
                s.spawn((
                    Value(Field::Summary),
                    Text::new(value(cfg, cat, profile, Field::Summary)),
                    mono(15.0),
                    TextColor(TEXT),
                ));
            });
            // no LAN in the browser
            if !cfg!(target_arch = "wasm32") {
                for (label, host) in [("JOIN LAN", false), ("HOST LAN", true)] {
                    r.spawn((
                        Act::Lan(host),
                        flat_cut(200.0, 72.0, Cut::Notch(12)),
                        children![(
                            Text::new(label),
                            ui::display(26.0),
                            tracking(0.12, 26.0),
                            TextColor(TEXT),
                            OnFill
                        )],
                    ));
                }
            }
            r.spawn((
                Act::Start,
                metal(380.0, 88.0, 14.0),
                children![(
                    Node {
                        width: percent(100),
                        height: percent(100),
                        padding: UiRect::horizontal(px(32)),
                        justify_content: JustifyContent::SpaceBetween,
                        align_items: AlignItems::Center,
                        ..default()
                    },
                    children![
                        (
                            Node {
                                flex_direction: FlexDirection::Column,
                                ..default()
                            },
                            children![
                                (
                                    Text::new("START"),
                                    ui::display(48.0),
                                    tracking(0.1, 48.0),
                                    TextColor(ON_ACCENT)
                                ),
                                (Text::new("ENTER"), mono(11.0), TextColor(ON_ACCENT))
                            ]
                        ),
                        (
                            Node {
                                column_gap: px(2),
                                ..default()
                            },
                            Children::spawn(SpawnIter([0.35, 0.65, 1.0].into_iter().map(|a| {
                                (
                                    Text::new(">"),
                                    ui::display(44.0),
                                    TextColor(ON_ACCENT.with_alpha(a)),
                                )
                            })))
                        )
                    ]
                )],
            ));
        });
    });
}

/// Shows the current page's watermark word only.
pub(super) fn mark(state: Res<State>, mut words: Query<(&Watermark, &mut Node)>) {
    if !state.is_changed() {
        return;
    }
    for (w, mut n) in &mut words {
        let d = if w.0 == state.page {
            Display::Flex
        } else {
            Display::None
        };
        if n.display != d {
            n.display = d;
        }
    }
}

/// Hover/selected look of the header tabs: the selected one is white over a faint tile with an
/// accent underline, its F-key lit.
#[allow(clippy::type_complexity)]
pub(super) fn tab_look(
    mut tabs: Query<(&Interaction, Has<Chosen>, &Children, &mut ImageNode), With<NavTab>>,
    mut labels: Query<&mut TextColor>,
    mut bars: Query<&mut BackgroundColor>,
) {
    for (i, chosen, kids, mut tile) in &mut tabs {
        let hot = *i != Interaction::None;
        let tint = Color::srgba(
            1.0,
            1.0,
            1.0,
            if chosen {
                0.05
            } else if hot {
                0.025
            } else {
                0.0
            },
        );
        if tile.color != tint {
            tile.color = tint;
        }
        let paint = |at: usize, c: Color, labels: &mut Query<&mut TextColor>| {
            if let Some(&e) = kids.get(at)
                && let Ok(mut t) = labels.get_mut(e)
            {
                t.set_if_neq(TextColor(c));
            }
        };
        paint(0, if chosen { ACCENT } else { MUTE }, &mut labels);
        paint(
            1,
            if chosen || hot { Color::WHITE } else { DIM },
            &mut labels,
        );
        if let Some(&under) = kids.get(2)
            && let Ok(mut b) = bars.get_mut(under)
        {
            b.set_if_neq(BackgroundColor(if chosen { ACCENT } else { Color::NONE }));
        }
    }
}

/// DIM label, INK while hovered or pressed.
pub(super) fn quiet_look(
    buttons: Query<(&Interaction, &Children), With<Quiet>>,
    mut labels: Query<&mut TextColor>,
) {
    for (i, kids) in &buttons {
        if let Some(&label) = kids.last()
            && let Ok(mut t) = labels.get_mut(label)
        {
            t.set_if_neq(TextColor(if *i == Interaction::None { DIM } else { INK }));
        }
    }
}

/// Draws [`Toast`]s: an accent-bordered plate above the footer that pops in and goes after
/// [`TOAST_SECS`]; a new toast replaces the one on screen.
pub(super) fn toast(
    mut commands: Commands,
    time: Res<Time>,
    mut msgs: MessageReader<Toast>,
    root: Query<Entity, With<Root>>,
    mut live: Query<(Entity, &mut ToastView, &mut UiTransform)>,
) {
    if let (Some(msg), Ok(root)) = (msgs.read().last(), root.single()) {
        for (e, ..) in &live {
            commands.entity(e).despawn();
        }
        commands.spawn((
            ToastView(0.0),
            UiTransform::default(),
            ChildOf(root),
            Node {
                left: px(0),
                right: px(0),
                bottom: px(150),
                justify_content: JustifyContent::Center,
                ..abs()
            },
            children![(
                Node {
                    padding: UiRect::axes(px(32), px(12)),
                    border: UiRect::all(px(2)),
                    ..default()
                },
                BackgroundColor(SURFACE),
                BorderColor::all(ACCENT),
                children![(
                    Text::new(msg.0.to_uppercase()),
                    ui::display(28.0),
                    tracking(0.1, 28.0),
                    TextColor(INK)
                )]
            )],
        ));
    }
    for (e, mut v, mut tf) in &mut live {
        v.0 += time.delta_secs();
        if v.0 >= TOAST_SECS {
            commands.entity(e).despawn();
            continue;
        }
        let k = ease_out(v.0 / 0.15);
        tf.translation = Val2::px(0.0, 14.0 * (1.0 - k));
        tf.scale = Vec2::splat(0.92 + 0.08 * k);
    }
}
