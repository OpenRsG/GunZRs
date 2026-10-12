//! The MATCH page: the map tiles (`01 MAP`), the hero with the chosen map, and the rules (`02
//! RULES`: mode, its description and the four steppers).

use super::*;
use crate::ui::{
    self, ACCENT, BACKDROP, Cut, DIM, INK, MUTE, OnFill, SURFACE, Segs, TEXT, button, caption,
    digits, flat_cut, hatch, hazard, mono, section, segs, shadow, shape, strong, tracking,
};

/// The segment bar of a stepper; [`bars`] lights it from the config.
#[derive(Component)]
pub(super) struct SegOf(Field);

/// A big panel: `w` x `h`, chamfered, a little see-through so the watermark shows.
fn frame(w: f32, h: f32) -> impl Bundle {
    (
        Node {
            width: px(w),
            height: px(h),
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(px(28)),
            row_gap: px(16),
            ..default()
        },
        shape(Cut::Chamfer(18), SURFACE.with_alpha(0.62)),
    )
}

/// The pictures of the maps ([`Profile::preview`]), in `Catalog::maps` order; `None` where one
/// was not made (the tile then shows the hatch).
#[derive(Resource, Clone)]
pub(super) struct Previews(Vec<Option<Handle<Image>>>);

impl Previews {
    pub(super) fn load(profile: &Profile, cat: &Catalog, images: &mut Assets<Image>) -> Self {
        Self(
            cat.maps
                .iter()
                .map(|m| profile.preview(m).map(|i| images.add(i)))
                .collect(),
        )
    }
}

/// A map tile's picture, dimmed until the tile is chosen or hovered ([`art_look`]).
#[derive(Component)]
pub(super) struct MapArt;

/// The picture behind the hero's map name ([`hero`]).
#[derive(Component)]
pub(super) struct HeroArt;

/// A tile's picture when it is neither chosen nor hovered.
const DIMMED: Color = Color::srgb(0.65, 0.65, 0.65);

/// The chosen map's picture; none in Quest mode (it has no map of its own) or if it was not made.
fn shown(cfg: &Config, cat: &Catalog, art: &Previews) -> Option<Handle<Image>> {
    let at = cat.maps.iter().position(|m| Some(m) == cfg.map.as_ref())?;
    art.0[at].clone().filter(|_| cfg.mode != Mode::Quest)
}

/// A full-size gradient from the backdrop at alpha `a[0]` through `a[1]` to `a[2]`, drawn over
/// the hero's picture.
fn fade(gradient: fn(Vec<ColorStop>) -> LinearGradient, a: [f32; 3]) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            left: px(0),
            top: px(0),
            width: percent(100),
            height: percent(100),
            ..default()
        },
        BackgroundGradient::from(gradient(vec![
            ColorStop::new(BACKDROP.with_alpha(a[0]), percent(0)),
            ColorStop::new(BACKDROP.with_alpha(a[1]), percent(50)),
            ColorStop::new(BACKDROP.with_alpha(a[2]), percent(100)),
        ])),
    )
}

/// A stepper well: `<`, the caption (a [`Value`] when `name` is set) with the big value (and
/// the skill's label), a segment bar of `n` segments, `>`.
fn well(
    g: &mut ChildSpawnerCommands,
    (cfg, cat, profile): (&Config, &Catalog, &Profile),
    cap: &str,
    name: Option<Field>,
    f: Field,
    n: usize,
) {
    g.spawn((
        Node {
            width: px(470),
            height: px(61),
            padding: UiRect::all(px(10)),
            column_gap: px(16),
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.35)),
    ))
    .with_children(|w| {
        w.spawn(button(44.0, 41.0, "<", 20.0, Act::Step(f, -1)));
        w.spawn(Node {
            flex_grow: 1.0,
            flex_direction: FlexDirection::Column,
            row_gap: px(8),
            ..default()
        })
        .with_children(|c| {
            c.spawn(Node {
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|r| {
                let mut l = r.spawn(caption(cap, 13.0));
                if let Some(name) = name {
                    l.insert(Value(name));
                }
                r.spawn(Node {
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    ..default()
                })
                .with_children(|v| {
                    v.spawn((
                        Value(f),
                        Text::new(value(cfg, cat, profile, f)),
                        digits(22.0),
                        TextColor(INK),
                    ));
                    if f == Field::Skill {
                        v.spawn((
                            Value(Field::SkillName),
                            Text::new(value(cfg, cat, profile, Field::SkillName)),
                            strong(13.0),
                            tracking(0.1, 13.0),
                            TextColor(ACCENT),
                        ));
                    }
                });
            });
            c.spawn((SegOf(f), segs(n, 0, 4.0, 4.0)));
        });
        w.spawn(button(44.0, 41.0, ">", 20.0, Act::Step(f, 1)));
    });
}

/// The page's panels, children of the MATCH [`PageRoot`]. The caller adds the quest panel
/// (`ModeShow(true)`), which takes the map panel's place in Quest mode.
pub(super) fn fill(
    p: &mut ChildSpawnerCommands,
    cfg: &Config,
    cat: &Catalog,
    profile: &Profile,
    art: &Previews,
) {
    let ctx = (cfg, cat, profile);
    let shown = shown(cfg, cat, art);
    p.spawn((ModeShow(false), frame(800.0, 804.0)))
        .with_children(|m| {
            m.spawn(section(1, "MAP")).with_children(|s| {
                s.spawn((
                    Text::new(format!("{} ARENAS", cat.maps.len())),
                    mono(13.0),
                    TextColor(DIM),
                ));
                s.spawn(button(98.0, 34.0, "RANDOM", 13.0, Act::RandomMap));
            });
            m.spawn(Node {
                width: px(745),
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(10),
                row_gap: px(10),
                ..default()
            })
            .with_children(|g| {
                for (i, name) in cat.maps.iter().enumerate() {
                    let pic = &art.0[i];
                    g.spawn((Act::Map(i), flat_cut(141.0, 104.0, Cut::Notch(12))))
                        .with_children(|t| {
                            let top = Node {
                                position_type: PositionType::Absolute,
                                left: px(0),
                                top: px(0),
                                width: percent(100),
                                height: px(62),
                                ..default()
                            };
                            match pic {
                                Some(img) => t.spawn((
                                    MapArt,
                                    top,
                                    ui::art(img.clone(), 141.0, 62.0, DIMMED),
                                )),
                                None => t.spawn((top, hatch(Color::srgba(1.0, 1.0, 1.0, 0.006)))),
                            };
                            t.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    top: px(8),
                                    right: px(12),
                                    ..default()
                                },
                                Text::new(format!("{:02}", i + 1)),
                                mono(11.0),
                                TextColor(if pic.is_some() { TEXT } else { MUTE }),
                                shadow(),
                            ));
                            t.spawn((
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(12),
                                    bottom: px(12),
                                    ..default()
                                },
                                Text::new(title(name)),
                                strong(15.0),
                                TextColor(TEXT),
                                OnFill,
                            ));
                        });
                }
            });
        });
    p.spawn(Node {
        flex_direction: FlexDirection::Column,
        row_gap: px(16),
        ..default()
    })
    .with_children(|c| {
        c.spawn((
            Node {
                width: px(1016),
                height: px(330),
                flex_direction: FlexDirection::Column,
                justify_content: JustifyContent::FlexEnd,
                padding: UiRect {
                    left: px(44),
                    bottom: px(34),
                    ..default()
                },
                ..default()
            },
            shape(Cut::Notch(18), SURFACE.with_alpha(0.62)),
            children![
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        top: px(0),
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                    BackgroundGradient::from(RadialGradient::new(
                        UiPosition::CENTER,
                        RadialGradientShape::ClosestCorner,
                        vec![
                            ColorStop::new(Color::srgb(0.137, 0.145, 0.169), percent(0)),
                            ColorStop::new(Color::srgb(0.075, 0.078, 0.09), percent(62)),
                            ColorStop::new(Color::srgb(0.039, 0.043, 0.051), percent(100)),
                        ],
                    ))
                ),
                (
                    HeroArt,
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        top: px(0),
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                    ui::art(
                        shown.clone().unwrap_or_default(),
                        1016.0,
                        330.0,
                        Color::WHITE
                    ),
                    if shown.is_some() {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    },
                    // the name stays readable: the left and the bottom fade to the backdrop
                    children![
                        fade(LinearGradient::to_right, [0.9, 0.55, 0.05]),
                        fade(LinearGradient::to_bottom, [0.0, 0.1, 1.0])
                    ]
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        top: px(0),
                        width: percent(100),
                        height: percent(100),
                        ..default()
                    },
                    hatch(Color::srgba(1.0, 1.0, 1.0, 0.006))
                ),
                (
                    Node {
                        position_type: PositionType::Absolute,
                        left: px(0),
                        top: px(0),
                        width: px(180),
                        height: px(30),
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                    hazard(),
                    children![(
                        Node {
                            padding: UiRect::horizontal(px(14)),
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        BackgroundColor(BACKDROP),
                        children![(
                            Text::new("SELECTED"),
                            strong(12.0),
                            tracking(0.2, 12.0),
                            TextColor(ACCENT)
                        )]
                    )]
                ),
            ],
        ))
        .with_children(|h| {
            h.spawn((
                Value(Field::MapNo),
                Text::new(value(cfg, cat, profile, Field::MapNo)),
                mono(13.0),
                tracking(0.14, 13.0),
                TextColor(ACCENT),
            ));
            h.spawn((
                Value(Field::MapName),
                Text::new(value(cfg, cat, profile, Field::MapName).to_uppercase()),
                ui::display(132.0),
                tracking(-0.01, 132.0),
                TextColor(INK),
            ));
            h.spawn((
                Value(Field::Hero),
                Text::new(value(cfg, cat, profile, Field::Hero)),
                ui::body(18.0),
                tracking(0.08, 18.0),
                TextColor(TEXT),
            ));
        });
        c.spawn(frame(1016.0, 458.0)).with_children(|m| {
            m.spawn(section(2, "RULES"));
            m.spawn(Node {
                width: px(960),
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(6),
                row_gap: px(6),
                ..default()
            })
            .with_children(|g| {
                for mode in Mode::ALL {
                    g.spawn(button(155.0, 40.0, mode.name(), 14.0, Act::Mode(mode)));
                }
            });
            m.spawn((
                Value(Field::Blurb),
                Text::new(value(cfg, cat, profile, Field::Blurb)),
                ui::body(16.0),
                TextColor(TEXT),
                Node {
                    min_height: px(44),
                    ..default()
                },
            ));
            m.spawn(Node {
                width: px(960),
                flex_wrap: FlexWrap::Wrap,
                column_gap: px(20),
                row_gap: px(12),
                ..default()
            })
            .with_children(|g| {
                well(g, ctx, "TIME LIMIT", Some(Field::TimeName), Field::Time, 6);
                well(
                    g,
                    ctx,
                    "KILL LIMIT",
                    Some(Field::KillsName),
                    Field::Kills,
                    6,
                );
                well(g, ctx, "BOTS", None, Field::Bots, super::MAX_BOTS as usize);
                well(g, ctx, "BOT SKILL", None, Field::Skill, 10);
            });
        });
    });
}

/// How far `cur` is along a limit list, as lit segments of `n` (none for no limit).
fn share(list: &[u32], cur: Option<u32>, n: usize) -> usize {
    let Some(cur) = cur else { return 0 };
    let at = list.iter().position(|&v| v == cur).unwrap_or(0);
    ((at + 1) * n).div_ceil(list.len().max(1))
}

/// Lights the steppers' segment bars from the config.
pub(super) fn bars(state: Res<State>, mut bars: Query<(&SegOf, &mut Segs)>) {
    if !state.is_changed() {
        return;
    }
    let cfg = &state.cfg;
    let lim = cfg.mode.limits();
    for (f, mut s) in &mut bars {
        let lit = match f.0 {
            Field::Time => share(lim.minutes, cfg.time_limit.map(|t| t / 60), 6),
            Field::Kills => share(lim.kills, cfg.kill_limit, 6),
            Field::Bots => cfg.bots,
            _ => (cfg.skill * 10.0).round() as usize,
        };
        s.set_if_neq(Segs { lit });
    }
}

/// Dims the pictures of the map tiles that are neither chosen nor hovered.
pub(super) fn art_look(
    tiles: Query<(&Act, &Interaction, Has<Chosen>, &Children)>,
    mut art: Query<&mut ImageNode, With<MapArt>>,
) {
    for (act, hover, chosen, kids) in &tiles {
        let tint = match (act, chosen, hover) {
            (Act::Map(_), true, _) => Color::WHITE,
            (Act::Map(_), _, Interaction::None) => DIMMED,
            (Act::Map(_), ..) => Color::srgb(0.8, 0.8, 0.8),
            _ => continue,
        };
        for &kid in kids {
            if let Ok(mut pic) = art.get_mut(kid)
                && pic.color != tint
            {
                pic.color = tint;
            }
        }
    }
}

/// Shows the chosen map's picture in the hero.
pub(super) fn hero(
    state: Res<State>,
    cat: Res<Catalog>,
    art: Res<Previews>,
    mut hero: Single<(&mut ImageNode, &mut Visibility), With<HeroArt>>,
) {
    if !state.is_changed() {
        return;
    }
    let (node, vis) = &mut *hero;
    match shown(&state.cfg, &cat, &art) {
        Some(pic) => {
            if node.image != pic {
                node.image = pic;
            }
            vis.set_if_neq(Visibility::Inherited);
        }
        None => {
            vis.set_if_neq(Visibility::Hidden);
        }
    }
}
