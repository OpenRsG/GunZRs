//! The PLAYER page: the CHARACTER panel (sex, one row of piece and dye steppers per slot), the
//! 3D viewport frame the preview character stands in (view switch, floor ring) and the EFFECTS
//! panel (dash and slash trails, with the demo). Laid out on the 1920 x 1080 canvas after the
//! mock: 760 + 600 + 440 px columns, 804 px tall.

use super::{
    Act, Catalog, Config, FOCUS, Field, Fx, Orbit, State, Tab, Value, amount, piece, value,
};
use crate::{
    character::{Slot, TINTS},
    dashfx::{HUES, Hue, STYLES, Trail},
    profile::Profile,
    slashfx::{self, Slash},
    ui::{
        ACCENT, BACKDROP, BLUE, Chosen, Cut, DIM, EDGE, INK, MUTE, OnFill, RAISED, RED, SURFACE,
        Segs, TEXT, body, caption, compact, digits, display, flat_cut, hatch, mono, primary,
        section, segs, shape, strong, tracking,
    },
};
use bevy::{
    prelude::*,
    text::{Justify, LineBreak},
    ui::RelativeCursorPosition,
};

/// The preview viewport: [`super::orbit`] frames the character at its centre.
#[derive(Component)]
pub(super) struct Frame;

/// The accent ring on the stage floor under the character: flat annuli, so the camera's
/// perspective draws the ellipse (the second, wider one is the glow). [`super::preview`] shows
/// them on the player page only.
#[derive(Component)]
pub(super) struct Floor;

pub(super) fn floor(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    for (inner, outer, alpha) in [(0.58, 0.62, 0.9), (0.5, 0.8, 0.03)] {
        commands.spawn((
            Floor,
            Mesh3d(meshes.add(Annulus::new(inner, outer).mesh().resolution(96))),
            MeshMaterial3d(materials.add(StandardMaterial {
                base_color: ACCENT.with_alpha(alpha),
                unlit: true,
                alpha_mode: AlphaMode::Blend,
                cull_mode: None,
                ..default()
            })),
            Transform::from_xyz(0.0, 0.004, 0.0)
                .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            Visibility::Hidden,
        ));
    }
}

/// The colour square next to a slot's dye name.
#[derive(Component)]
pub(super) struct Swatch(usize);

/// The accent line under a piece name: how far along the slot's pieces the choice is.
#[derive(Component)]
pub(super) struct Prog(usize);

/// A text [`look`] writes from the preview framing and the shown [`Tab`].
#[derive(Component)]
pub(super) enum Live {
    View,
    Preview,
}

/// The amount slider of an EFFECTS tab (its [`Segs`] child shows the amount).
#[derive(Component)]
pub(super) struct Slider(Tab);

/// Container shown only while its [`Tab`] is the EFFECTS panel's.
#[derive(Component)]
pub(super) struct TabShow(Tab);

/// A colour tile: [`look`] rings it in the accent when [`Chosen`].
#[derive(Component)]
pub(super) struct HueTile;

/// An outlined button: transparent, filled faintly on hover.
#[derive(Component)]
pub(super) struct Ghost;

/// Steps of an amount slider (the amount moves in twentieths).
const STEPS: usize = 20;

fn text(s: impl Into<String>, font: TextFont, color: Color) -> impl Bundle {
    (Text::new(s.into()), font, TextColor(color))
}

/// A column of the page: a chamfered panel 804 px tall.
fn column(width: f32, gap: f32) -> impl Bundle {
    (
        Node {
            width: px(width),
            height: px(804),
            flex_direction: FlexDirection::Column,
            row_gap: px(gap),
            padding: UiRect::axes(px(28), px(24)),
            ..default()
        },
        shape(Cut::Chamfer(18), SURFACE),
    )
}

/// A letter-spaced choice or action button.
fn tile(w: f32, h: f32, cut: Cut, label: &str, font: TextFont, size: f32, act: Act) -> impl Bundle {
    (
        act,
        flat_cut(w, h, cut),
        children![(
            Text::new(label),
            font,
            TextColor(TEXT),
            tracking(0.14, size),
            OnFill
        )],
    )
}

/// A step button inside a well.
fn step(w: f32, h: f32, label: &str, size: f32, act: Act) -> impl Bundle {
    (
        act,
        flat_cut(w, h, Cut::Notch(4)),
        children![(Text::new(label), strong(size), TextColor(TEXT), OnFill)],
    )
}

/// The dark inset a stepper sits in.
fn well(w: f32) -> impl Bundle {
    (
        Node {
            width: px(w),
            height: px(58),
            padding: UiRect::all(px(4)),
            column_gap: px(4),
            border: UiRect::all(px(1)),
            ..default()
        },
        BackgroundColor(BACKDROP),
        BorderColor::all(EDGE),
    )
}

fn label(s: &str) -> impl Bundle {
    (
        Node {
            width: px(110),
            ..default()
        },
        children![caption(s, 13.0)],
    )
}

/// A value that stays on one line and gives way to its neighbour.
fn line() -> (TextLayout, Node) {
    (
        TextLayout {
            justify: Justify::Left,
            linebreak: LineBreak::NoWrap,
            ..default()
        },
        Node {
            min_width: px(0),
            flex_shrink: 1.0,
            overflow: Overflow::clip(),
            ..default()
        },
    )
}

/// The page's three columns: the CHARACTER panel, the viewport frame and the EFFECTS panel.
pub(super) fn fill(p: &mut ChildSpawnerCommands, cfg: &Config, cat: &Catalog, profile: &Profile) {
    p.spawn(column(760.0, 18.0)).with_children(|m| {
        m.spawn(section(1, "CHARACTER"));
        m.spawn(Node {
            margin: UiRect::top(px(-6)), // takes back the section row's bottom margin
            align_items: AlignItems::Center,
            column_gap: px(12),
            ..default()
        })
        .with_children(|r| {
            r.spawn(label("SEX"));
            r.spawn(Node {
                column_gap: px(6),
                ..default()
            })
            .with_children(|b| {
                let slant = Cut::Slant(8);
                b.spawn(tile(
                    288.0,
                    44.0,
                    slant,
                    "MAN",
                    strong(15.0),
                    15.0,
                    Act::Sex(false),
                ));
                b.spawn(tile(
                    288.0,
                    44.0,
                    slant,
                    "WOMAN",
                    strong(15.0),
                    15.0,
                    Act::Sex(true),
                ));
            });
        });
        // per slot: `caption  [<< < name n/total > >>]  [< dye >]`
        m.spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(10),
            ..default()
        })
        .with_children(|list| {
            for (s, slot) in Slot::ALL.into_iter().enumerate() {
                let (_, at, n) = piece(cfg, cat, s);
                list.spawn(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(12),
                    ..default()
                })
                .with_children(|r| {
                    r.spawn(label(&slot.label().replace('/', " / ")));
                    let f = Field::Piece(s);
                    r.spawn(well(370.0)).with_children(|w| {
                        w.spawn(step(36.0, 50.0, "«", 14.0, Act::Step(f, -10)));
                        w.spawn(step(36.0, 50.0, "‹", 20.0, Act::Step(f, -1)));
                        w.spawn(Node {
                            flex_grow: 1.0,
                            flex_basis: px(0),
                            flex_direction: FlexDirection::Column,
                            justify_content: JustifyContent::Center,
                            row_gap: px(6),
                            padding: UiRect::horizontal(px(8)),
                            ..default()
                        })
                        .with_children(|c| {
                            c.spawn(Node {
                                justify_content: JustifyContent::SpaceBetween,
                                align_items: AlignItems::Baseline,
                                column_gap: px(8),
                                ..default()
                            })
                            .with_children(|t| {
                                t.spawn((
                                    Value(f),
                                    text(value(cfg, cat, profile, f), strong(16.0), INK),
                                    line(),
                                ));
                                t.spawn((
                                    Value(Field::Count(s)),
                                    text(
                                        value(cfg, cat, profile, Field::Count(s)),
                                        mono(12.0),
                                        DIM,
                                    ),
                                ));
                            });
                            c.spawn((
                                Node {
                                    width: percent(100),
                                    height: px(3),
                                    ..default()
                                },
                                BackgroundColor(EDGE),
                                children![(
                                    Prog(s),
                                    Node {
                                        width: percent(100.0 * at as f32 / n as f32),
                                        height: percent(100),
                                        ..default()
                                    },
                                    BackgroundColor(ACCENT),
                                )],
                            ));
                        });
                        w.spawn(step(36.0, 50.0, "›", 20.0, Act::Step(f, 1)));
                        w.spawn(step(36.0, 50.0, "»", 14.0, Act::Step(f, 10)));
                    });
                    let f = Field::Tint(s);
                    r.spawn(well(200.0)).with_children(|w| {
                        w.spawn(step(32.0, 50.0, "‹", 18.0, Act::Step(f, -1)));
                        w.spawn(Node {
                            flex_grow: 1.0,
                            flex_basis: px(0),
                            align_items: AlignItems::Center,
                            column_gap: px(8),
                            padding: UiRect::horizontal(px(6)),
                            ..default()
                        })
                        .with_children(|c| {
                            c.spawn((
                                Swatch(s),
                                Node {
                                    width: px(18),
                                    height: px(18),
                                    flex_shrink: 0.0,
                                    border: UiRect::all(px(2)),
                                    ..default()
                                },
                                BackgroundColor(INK),
                                BorderColor::all(MUTE),
                            ));
                            c.spawn((
                                Value(f),
                                text(value(cfg, cat, profile, f), compact(13.0), INK),
                                line(),
                            ));
                        });
                        w.spawn(step(32.0, 50.0, "›", 18.0, Act::Step(f, 1)));
                    });
                });
            }
        });
        m.spawn(Node {
            margin: UiRect::top(Val::Auto),
            column_gap: px(8),
            ..default()
        })
        .with_children(|b| {
            let notch = Cut::Notch(10);
            b.spawn(tile(
                155.0,
                52.0,
                notch,
                "RANDOM",
                strong(14.0),
                14.0,
                Act::Random(false),
            ));
            b.spawn(tile(
                155.0,
                52.0,
                notch,
                "RANDOM DYES",
                strong(14.0),
                14.0,
                Act::Random(true),
            ));
            b.spawn(tile(
                155.0,
                52.0,
                notch,
                "RESET",
                strong(14.0),
                14.0,
                Act::Reset,
            ));
            b.spawn(primary(216.0, 52.0, "SAVE LOADOUT", 20.0, Act::Save));
        });
    });

    // the 3D viewport: the character shows through, the frame only hatches and labels it
    p.spawn((
        Frame,
        Node {
            width: px(600),
            height: px(804),
            ..default()
        },
        hatch(Color::srgba(1.0, 1.0, 1.0, 0.006)),
    ))
    .with_children(|f| {
        let at = |left: f32, top: f32| Node {
            position_type: PositionType::Absolute,
            left: px(left),
            top: px(top),
            ..default()
        };
        f.spawn((
            Live::View,
            text("", mono(12.0), INK.with_alpha(0.35)),
            tracking(0.1, 12.0),
            at(24.0, 22.0),
        ));
        f.spawn((
            text("Drag to rotate · Wheel to zoom", body(13.0), DIM),
            at(24.0, 44.0),
        ));
        f.spawn(Node {
            position_type: PositionType::Absolute,
            left: px(24),
            bottom: px(24),
            column_gap: px(6),
            ..default()
        })
        .with_children(|b| {
            for (i, view) in FOCUS.iter().enumerate() {
                let slant = Cut::Slant(8);
                b.spawn(tile(
                    105.6,
                    48.0,
                    slant,
                    view.0,
                    strong(14.0),
                    14.0,
                    Act::Focus(i),
                ));
            }
        });
    });

    p.spawn(column(440.0, 22.0)).with_children(|m| {
        m.spawn(section(2, "EFFECTS"));
        m.spawn((
            Node {
                margin: UiRect::top(px(-6)), // takes back the section row's bottom margin
                padding: UiRect::all(px(4)),
                column_gap: px(4),
                ..default()
            },
            BackgroundColor(BACKDROP),
        ))
        .with_children(|s| {
            let slant = Cut::Slant(8);
            s.spawn(tile(
                186.0,
                44.0,
                slant,
                "DASH",
                display(22.0),
                22.0,
                Act::Tab(Tab::Dash),
            ));
            s.spawn(tile(
                186.0,
                44.0,
                slant,
                "SLASH",
                display(22.0),
                22.0,
                Act::Tab(Tab::Slash),
            ));
        });
        let dash = STYLES.map(|(s, _, label)| (label, Act::Style(s)));
        let slash = slashfx::STYLES.map(|(s, _, label)| (label, Act::Cut(s)));
        fx_tab(m, Tab::Dash, &dash, profile);
        fx_tab(m, Tab::Slash, &slash, profile);
        m.spawn((
            Act::Demo,
            Ghost,
            Button,
            Node {
                width: percent(100),
                height: px(52),
                border: UiRect::all(px(2)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BorderColor::all(ACCENT),
            children![(
                Live::Preview,
                text("", strong(15.0), ACCENT),
                tracking(0.2, 15.0)
            )],
        ));
        m.spawn((Value(Field::Note), text("", compact(13.0), DIM)));
        m.spawn((
            Node {
                margin: UiRect::top(Val::Auto),
                ..default()
            },
            children![text(
                "Dyes are not in the original game. Weapon and armour stats are set in Inventory.",
                body(14.0),
                DIM
            )],
        ));
    });
}

/// One tab of the EFFECTS panel: the trail styles (two to a row), the colour tiles and the amount.
fn fx_tab(
    m: &mut ChildSpawnerCommands,
    tab: Tab,
    styles: &[(&'static str, Act)],
    profile: &Profile,
) {
    let block = || Node {
        width: percent(100),
        flex_direction: FlexDirection::Column,
        row_gap: px(10),
        ..default()
    };
    let head = || Node {
        width: percent(100),
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Baseline,
        ..default()
    };
    m.spawn((
        TabShow(tab),
        Node {
            width: percent(100),
            flex_direction: FlexDirection::Column,
            row_gap: px(22),
            ..default()
        },
    ))
    .with_children(|m| {
        m.spawn(block()).with_children(|b| {
            b.spawn(caption("TRAIL", 13.0));
            b.spawn(Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            })
            .with_children(|g| {
                // two rows, not a wrapping row: a wrap's height is measured at one tile wide
                for pair in styles.chunks(2) {
                    g.spawn(Node {
                        column_gap: px(6),
                        ..default()
                    })
                    .with_children(|r| {
                        for &(label, act) in pair {
                            r.spawn(tile(
                                189.0,
                                44.0,
                                Cut::Slant(8),
                                label,
                                strong(14.0),
                                14.0,
                                act,
                            ));
                        }
                    });
                }
            });
        });
        m.spawn(block()).with_children(|b| {
            b.spawn(head()).with_children(|h| {
                h.spawn((Value(Field::Caption(tab)), caption("", 13.0)));
                h.spawn((
                    Value(Field::Colour(tab)),
                    text("", strong(13.0), INK),
                    tracking(0.12, 13.0),
                ));
            });
            b.spawn(Node {
                width: percent(100),
                column_gap: px(8),
                ..default()
            })
            .with_children(|g| {
                for (h, _, _) in HUES {
                    g.spawn((
                        Act::Hue(tab, h),
                        HueTile,
                        Button,
                        Node {
                            width: px(48),
                            height: px(44),
                            border: UiRect::all(px(2)),
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(RAISED),
                        BorderColor::all(EDGE),
                        children![diamond(h)],
                    ));
                }
            });
        });
        m.spawn(block()).with_children(|b| {
            b.spawn(head()).with_children(|h| {
                h.spawn(caption("AMOUNT", 13.0));
                h.spawn((Value(Field::Amount(tab)), text("", digits(20.0), ACCENT)));
            });
            b.spawn(Node {
                width: percent(100),
                column_gap: px(8),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|r| {
                r.spawn(step(40.0, 40.0, "−", 20.0, Act::Amount(tab, -1)));
                // the whole bar takes clicks and drags (`fx_panel`)
                let lit = (amount(profile, tab) * STEPS as f32).round() as usize;
                r.spawn((
                    Slider(tab),
                    Button,
                    RelativeCursorPosition::default(),
                    Node {
                        flex_grow: 1.0,
                        height: px(40),
                        padding: UiRect::all(px(4)),
                        border: UiRect::all(px(1)),
                        ..default()
                    },
                    BackgroundColor(BACKDROP),
                    BorderColor::all(EDGE),
                    children![segs(STEPS, lit, 30.0, 3.0)],
                ));
                r.spawn(step(40.0, 40.0, "+", 20.0, Act::Amount(tab, 1)));
            });
        });
    });
}

/// A colour tile's square, turned 45 degrees: the hue, or red | blue for TEAM and the spectrum
/// for RAINBOW.
fn diamond(h: Hue) -> impl Bundle {
    let node = Node {
        width: px(22),
        height: px(22),
        ..default()
    };
    let turned = UiTransform::from_rotation(Rot2::degrees(45.0));
    let c = h.color(None, 0.0);
    let hue = Color::linear_rgb(c.red.min(1.0), c.green.min(1.0), c.blue.min(1.0));
    let stops = match h {
        Hue::Team => vec![
            ColorStop::new(RED, percent(0)),
            ColorStop::new(RED, percent(50)),
            ColorStop::new(BLUE, percent(50)),
            ColorStop::new(BLUE, percent(100)),
        ],
        Hue::Rainbow => (0..=6)
            .map(|i| {
                let at = percent(i as f32 * 100.0 / 6.0);
                ColorStop::new(Color::hsl(i as f32 * 60.0, 1.0, 0.5), at)
            })
            .collect(),
        _ => vec![
            ColorStop::new(hue, percent(0)),
            ColorStop::new(hue, percent(100)),
        ],
    };
    (
        node,
        turned,
        BackgroundGradient::from(LinearGradient::to_right(stops)),
    )
}

/// The amount sliders (drag), their segments, and saving the effect settings once they settle.
pub(super) fn fx_panel(
    mouse: Res<ButtonInput<MouseButton>>,
    sliders: Query<(&Interaction, &RelativeCursorPosition, &Slider, &Children)>,
    mut bars: Query<&mut Segs>,
    mut profile: ResMut<Profile>,
    mut dragging: Local<Option<Tab>>,
    mut saved: Local<Option<(Trail, Slash)>>,
) {
    if !mouse.pressed(MouseButton::Left) {
        *dragging = None;
    } else if let Some(s) = sliders.iter().find(|s| *s.0 == Interaction::Pressed) {
        *dragging = Some(s.2.0);
    }
    if let Some(t) = *dragging
        && let Some(s) = sliders.iter().find(|s| s.2.0 == t)
        && let Some(n) = s.1.normalized
    {
        let v = ((n.x + 0.5).clamp(0.0, 1.0) * STEPS as f32).round() / STEPS as f32;
        if amount(&profile, t) != v {
            match t {
                Tab::Dash => profile.dash.amount = v,
                Tab::Slash => profile.slash.amount = v,
            }
        }
    }
    for (.., s, kids) in &sliders {
        let lit = (amount(&profile, s.0) * STEPS as f32).round() as usize;
        for &k in kids {
            if let Ok(mut b) = bars.get_mut(k) {
                b.set_if_neq(Segs { lit });
            }
        }
    }
    let now = (profile.dash, profile.slash);
    match &*saved {
        None => *saved = Some(now),
        Some(s) if *s != now && !mouse.pressed(MouseButton::Left) => {
            profile.save();
            *saved = Some(now);
        }
        _ => {}
    }
}

/// Keeps the page's own widgets in line with the state: dye swatches, progress lines, the viewport
/// and demo labels, the shown EFFECTS tab, and the rings of the colour tiles and the PREVIEW fill.
#[allow(clippy::type_complexity)]
pub(super) fn look(
    state: Res<State>,
    cat: Res<Catalog>,
    orbit: Res<Orbit>,
    fx: Res<Fx>,
    mut swatches: Query<(&Swatch, &mut BackgroundColor), Without<Ghost>>,
    mut bars: Query<(&Prog, &mut Node), Without<TabShow>>,
    mut tabs: Query<(&TabShow, &mut Node), Without<Prog>>,
    mut labels: Query<(&Live, &mut Text)>,
    mut tiles: Query<(&Interaction, Has<Chosen>, &mut BorderColor), With<HueTile>>,
    mut ghosts: Query<(&Interaction, &mut BackgroundColor), (With<Ghost>, Without<Swatch>)>,
) {
    for (i, chosen, mut ring) in &mut tiles {
        let c = match (chosen, *i) {
            (true, _) => ACCENT,
            (_, Interaction::None) => EDGE,
            _ => MUTE,
        };
        ring.set_if_neq(BorderColor::all(c));
    }
    for (i, mut bg) in &mut ghosts {
        let a = match *i {
            Interaction::Pressed => 0.3,
            Interaction::Hovered => 0.15,
            Interaction::None => 0.0,
        };
        bg.set_if_neq(BackgroundColor(ACCENT.with_alpha(a)));
    }
    if !state.is_changed() && !orbit.is_changed() && !fx.is_changed() {
        return;
    }
    for (s, mut bg) in &mut swatches {
        let t = TINTS[state.cfg.look.tints[s.0] as usize % TINTS.len()].1;
        bg.set_if_neq(BackgroundColor(Color::srgb(t[0], t[1], t[2])));
    }
    for (p, mut n) in &mut bars {
        let (_, at, total) = piece(&state.cfg, &cat, p.0);
        let w = percent(100.0 * at as f32 / total as f32);
        if n.width != w {
            n.width = w;
        }
    }
    for (t, mut n) in &mut tabs {
        let d = if t.0 == fx.0 {
            Display::Flex
        } else {
            Display::None
        };
        if n.display != d {
            n.display = d;
        }
    }
    for (l, mut t) in &mut labels {
        let s = match l {
            Live::View => format!("3D CHARACTER VIEWPORT · {}", FOCUS[orbit.focus].0),
            Live::Preview => format!(
                "▶ PREVIEW {}",
                match fx.0 {
                    Tab::Dash => "DASH",
                    Tab::Slash => "SLASH",
                }
            ),
        };
        if t.0 != s {
            t.0 = s;
        }
    }
}
