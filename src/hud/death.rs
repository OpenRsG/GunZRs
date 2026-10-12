//! The death screen: while the player is dead the 3D view goes grey and soft (`gfx.wgsl`) under a
//! dark veil, a band with hazard-striped edges slides open across the middle of the screen with
//! ELIMINATED, the killer, the damage report (`fx::Report`) and the respawn countdown. The rest of
//! the HUD stays under the veil. A child of `hud.rs`; the nodes are spawned once and only
//! rewritten afterwards.

use super::{Root, fx::Report, set};
use crate::phase::MatchSystems;
use crate::{
    controls::{Action, Input},
    game::{Dead, Player},
    gore::paint,
    session::{Clock, HOLD},
    ui::{self, ACCENT, Cut, DIM, INK},
};
use bevy::{
    prelude::*,
    text::{LetterSpacing, LineHeight},
};

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<DeathRoot>))),
            update.run_if(any_with_component::<DeathRoot>.and_then(resource_exists::<Parts>)),
        )
            .in_set(MatchSystems),
    );
}

/// The mock's red (`#ff3b30`), its orange count, light-blue armour and the weapon chip's ink.
const RED: Color = Color::srgb(1.0, 0.231, 0.188);
const COUNT: Color = Color::srgb(1.0, 0.478, 0.18);
const ARMOR: Color = Color::srgb(0.561, 0.753, 1.0);
const CHIP: Color = Color::srgb(0.043, 0.047, 0.055);

/// Opacity of the veil over the view. The mock lays rgba(5,5,6,.55) in display space; Bevy blends
/// UI in linear light, where the same darkening (x0.45 in display space) takes 0.83; 0.87 matches
/// the mock's render of a lit scene.
const VEIL: f32 = 0.87;
/// Band top and height, stripe height (px on the 1080 canvas); respawn block top.
const BAND: (f32, f32, f32) = (380.0, 240.0, 4.0);
const COUNTDOWN_TOP: f32 = 660.0;
/// Seconds the veil fades in, the band opens and ELIMINATED slams in.
const VEIL_IN: f32 = 0.3;
const BAND_IN: f32 = 0.25;
const SLAM_IN: f32 = 0.35;
/// Stripe image width (texels); the node stretches it over the screen.
const STRIPE_W: u32 = 1920;

/// The screen's root, and its nodes by index into [`Parts`].
#[derive(Component)]
struct DeathRoot;

const ROOT: usize = 0;
const VEILED: usize = 1;
const BANDED: usize = 2;
const TITLE: usize = 3;
const BY: usize = 4;
const NAME: usize = 5;
const CHIP_BOX: usize = 6;
const WEAPON: usize = 7;
const STATS: usize = 8;
const TAKEN: usize = 9;
const TAKEN_HP: usize = 10;
const TAKEN_AP: usize = 11;
const LEFT: usize = 12;
const LEFT_CAP: usize = 13;
const LEFT_HP: usize = 14;
const LEFT_AP: usize = 15;
const HITS: usize = 16;
const HITS_N: usize = 17;
const LABEL: usize = 18;
const COUNTER: usize = 19;
const TRACK: usize = 20;
const FILL: usize = 21;

#[derive(Resource)]
struct Parts([Entity; 22]);

fn txt(s: &str, font: TextFont, color: Color) -> impl Bundle {
    (Text::new(s), font, TextColor(color))
}

fn caption(s: &str, size: f32) -> impl Bundle {
    (txt(s, ui::strong(size), DIM), ui::tracking(0.24, size))
}

/// A red hazard line (`repeating-linear-gradient(135deg, red 14px, clear 14px)`) with both ends
/// faded out, as an image stretched over the screen width.
fn stripes() -> Image {
    // Share of a one px window at `d` px of the 28 px period that is lit.
    let lit = |d: f32| {
        (d + 0.5).clamp(0.0, 1.0) - (d - 13.5).clamp(0.0, 1.0) + (d - 27.5).clamp(0.0, 1.0)
    };
    paint(STRIPE_W, BAND.2 as u32, |u, v| {
        let (x, y) = (u * STRIPE_W as f32 - 0.5, v * BAND.2 - 0.5);
        let d = ((x + y) * std::f32::consts::FRAC_1_SQRT_2).rem_euclid(28.0);
        let fade = (u / 0.25).min((1.0 - u) / 0.25).min(1.0);
        let c = RED.to_srgba();
        [c.red, c.green, c.blue, lit(d) * fade]
    })
}

/// A full-width strip `h` px high at `top` px.
fn strip(top: f32, h: Val) -> Node {
    Node {
        position_type: PositionType::Absolute,
        left: px(0),
        top: px(top),
        width: percent(100),
        height: h,
        ..default()
    }
}

/// A centred column across the screen at `top` px.
fn column(top: f32, gap: f32) -> Node {
    Node {
        flex_direction: FlexDirection::Column,
        align_items: AlignItems::Center,
        row_gap: px(gap),
        ..strip(top, Val::Auto)
    }
}

/// `100 HP · 50 AP` as a row; returns the two number nodes.
fn pair(c: &mut ChildSpawnerCommands) -> (Entity, Entity) {
    let mut ids = (Entity::PLACEHOLDER, Entity::PLACEHOLDER);
    c.spawn(Node {
        align_items: AlignItems::Baseline,
        column_gap: px(5),
        ..default()
    })
    .with_children(|r| {
        ids.0 = r.spawn(txt("", ui::digits(16.0), INK)).id();
        r.spawn(txt("HP", ui::digits(11.0), DIM));
        r.spawn(txt("·", ui::digits(16.0), INK));
        ids.1 = r.spawn(txt("", ui::digits(16.0), ARMOR)).id();
        r.spawn(txt("AP", ui::digits(11.0), DIM));
    });
    ids
}

fn spawn(
    mut commands: Commands,
    root: Single<Entity, With<Root>>,
    mut images: ResMut<Assets<Image>>,
) {
    let stripes = images.add(stripes());
    let mut at = [Entity::PLACEHOLDER; 22];
    let clear = |a: f32| Color::srgba(10.0 / 255.0, 11.0 / 255.0, 13.0 / 255.0, a);
    at[ROOT] = commands
        .spawn((
            DeathRoot,
            ChildOf(*root),
            // over the rest of the HUD, which stays under the veil
            ZIndex(5),
            Visibility::Hidden,
            strip(0.0, percent(100)),
        ))
        .with_children(|r| {
            at[VEILED] = r
                .spawn((strip(0.0, percent(100)), BackgroundColor(Color::NONE)))
                .id();
            at[BANDED] = r
                .spawn((
                    strip(BAND.0, px(BAND.1)),
                    UiTransform::from_scale(Vec2::new(1.0, 0.0)),
                    BackgroundGradient::from(LinearGradient::to_right(vec![
                        ColorStop::new(clear(0.0), percent(0)),
                        ColorStop::new(clear(0.88), percent(25)),
                        ColorStop::new(clear(0.88), percent(75)),
                        ColorStop::new(clear(0.0), percent(100)),
                    ])),
                ))
                .id();
            for top in [BAND.0, BAND.0 + BAND.1 - BAND.2] {
                r.spawn((strip(top, px(BAND.2)), ImageNode::new(stripes.clone())));
            }
            r.spawn(column(BAND.0 + 20.0, 12.0)).with_children(|c| {
                at[TITLE] = c
                    .spawn((
                        txt("ELIMINATED", ui::display(110.0), RED),
                        ui::tracking(0.06, 110.0),
                        LineHeight::RelativeToFont(0.85),
                        UiTransform::default(),
                    ))
                    .id();
                at[BY] = c
                    .spawn(Node {
                        align_items: AlignItems::Center,
                        column_gap: px(14),
                        ..default()
                    })
                    .with_children(|b| {
                        b.spawn((txt("BY", ui::strong(13.0), DIM), ui::tracking(0.3, 13.0)));
                        at[NAME] = b
                            .spawn((
                                txt("", ui::display(34.0), INK),
                                ui::tracking(0.06, 34.0),
                                LineHeight::RelativeToFont(1.0),
                            ))
                            .id();
                        at[CHIP_BOX] = b
                            .spawn((
                                Node {
                                    padding: UiRect::axes(px(8), px(3)),
                                    ..default()
                                },
                                ui::shape(Cut::Slant(4), INK),
                            ))
                            .with_children(|w| {
                                at[WEAPON] = w
                                    .spawn((
                                        txt("", ui::mono(10.0), CHIP),
                                        ui::tracking(0.08, 10.0),
                                    ))
                                    .id();
                            })
                            .id();
                    })
                    .id();
                let stat = Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: px(2),
                    ..default()
                };
                at[STATS] = c
                    .spawn(Node {
                        column_gap: px(40),
                        margin: UiRect::top(px(6)),
                        ..default()
                    })
                    .with_children(|s| {
                        at[TAKEN] = s
                            .spawn(stat.clone())
                            .with_children(|c| {
                                c.spawn(caption("DAMAGE TAKEN", 9.0));
                                (at[TAKEN_HP], at[TAKEN_AP]) = pair(c);
                            })
                            .id();
                        at[LEFT] = s
                            .spawn(stat.clone())
                            .with_children(|c| {
                                at[LEFT_CAP] = c.spawn(caption("", 9.0)).id();
                                (at[LEFT_HP], at[LEFT_AP]) = pair(c);
                            })
                            .id();
                        at[HITS] = s
                            .spawn(stat)
                            .with_children(|c| {
                                c.spawn(caption("HITS", 9.0));
                                at[HITS_N] = c.spawn(txt("", ui::digits(16.0), INK)).id();
                            })
                            .id();
                    })
                    .id();
            });
            r.spawn(column(COUNTDOWN_TOP, 8.0)).with_children(|c| {
                c.spawn(Node {
                    align_items: AlignItems::Baseline,
                    column_gap: px(10),
                    ..default()
                })
                .with_children(|row| {
                    at[LABEL] = row
                        .spawn((txt("", ui::strong(11.0), INK), ui::tracking(0.3, 11.0)))
                        .id();
                    at[COUNTER] = row
                        .spawn((
                            txt("", ui::display(30.0), COUNT),
                            LineHeight::RelativeToFont(0.8),
                        ))
                        .id();
                });
                at[TRACK] = c
                    .spawn((
                        Node {
                            width: px(300),
                            height: px(3),
                            ..default()
                        },
                        BackgroundColor(Color::srgba(1.0, 1.0, 1.0, 0.03)),
                    ))
                    .with_children(|t| {
                        at[FILL] = t
                            .spawn((
                                Node {
                                    width: percent(0),
                                    height: percent(100),
                                    ..default()
                                },
                                BackgroundColor(ACCENT),
                                BoxShadow::new(ACCENT, px(0), px(0), px(0), px(8)),
                            ))
                            .id();
                    })
                    .id();
            });
        })
        .id();
    commands.insert_resource(Parts(at));
}

/// Cubic ease-out of `t` seconds into an animation of `len`.
fn ease(t: f32, len: f32) -> f32 {
    1.0 - (1.0 - (t / len).clamp(0.0, 1.0)).powi(3)
}

/// Shows the screen while the player is dead (not once the match is over, nor while the
/// scoreboard key is held) and fills it from the [`Report`] and the respawn countdown.
#[allow(clippy::too_many_arguments)]
fn update(
    parts: Res<Parts>,
    real: Res<Time<Real>>,
    input: Input,
    clock: Option<Res<Clock>>,
    report: Res<Report>,
    player: Query<Option<&Dead>, With<Player>>,
    // seconds since the death and the longest wait seen this life
    mut st: Local<(f32, f32)>,
    mut vis: Query<&mut Visibility>,
    mut nodes: Query<&mut Node>,
    mut texts: Query<&mut Text>,
    mut colors: Query<&mut TextColor>,
    mut transforms: Query<&mut UiTransform>,
    mut backgrounds: Query<&mut BackgroundColor>,
    mut spacing: Query<&mut LetterSpacing>,
) {
    let at = parts.0;
    let dead = player.single().ok().flatten();
    let over = clock.is_some_and(|c| c.over.is_some());
    let on = dead.is_some() && !over && !input.pressed(Action::Score);
    if let Ok(mut v) = vis.get_mut(at[ROOT]) {
        v.set_if_neq(if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        });
    }
    let (age, max) = &mut *st;
    let Some(dead) = dead else {
        (*age, *max) = (0.0, 0.0);
        return;
    };
    if !on {
        return;
    }
    *age += real.delta_secs();
    let wait = dead.respawn >= HOLD;
    if !wait {
        *max = max.max(dead.respawn);
    }
    let mut text = |i: usize, s: String| {
        if let Ok(mut t) = texts.get_mut(at[i]) {
            set(&mut t, s);
        }
    };
    let mut show = |i: usize, on: bool| {
        if let Ok(mut n) = nodes.get_mut(at[i]) {
            n.display = if on { Display::Flex } else { Display::None };
        }
    };

    if let Ok(mut b) = backgrounds.get_mut(at[VEILED]) {
        b.0 = Color::srgba(
            5.0 / 255.0,
            5.0 / 255.0,
            6.0 / 255.0,
            VEIL * ease(*age, VEIL_IN),
        );
    }
    if let Ok(mut t) = transforms.get_mut(at[BANDED]) {
        t.scale = Vec2::new(1.0, ease(*age, BAND_IN));
    }
    // ELIMINATED slams in: 1.3x, clear and spread wide, settling to its size
    let slam = ease(*age, SLAM_IN);
    if let Ok(mut t) = transforms.get_mut(at[TITLE]) {
        t.scale = Vec2::splat(1.3 - 0.3 * slam);
    }
    if let Ok(mut c) = colors.get_mut(at[TITLE]) {
        c.0 = RED.with_alpha(slam);
    }
    if let Ok(mut s) = spacing.get_mut(at[TITLE]) {
        *s = ui::tracking(0.3 - 0.24 * slam, 110.0);
    }
    text(
        TITLE,
        if report.by.is_some() {
            "ELIMINATED"
        } else {
            "YOU DIED"
        }
        .to_owned(),
    );

    // who, with what, and what it cost; what the data lacks stays hidden
    show(BY, report.by.is_some());
    show(
        CHIP_BOX,
        report.by.as_ref().is_some_and(|b| !b.1.is_empty()),
    );
    if let Some((name, weapon)) = &report.by {
        text(NAME, name.to_uppercase());
        text(WEAPON, weapon.to_uppercase());
        text(LEFT_CAP, format!("{} LEFT ON", name.to_uppercase()));
    }
    let hurt = report.taken != (0.0, 0.0);
    show(TAKEN, hurt);
    text(TAKEN_HP, format!("{:.0}", report.taken.0));
    text(TAKEN_AP, format!("{:.0}", report.taken.1));
    show(LEFT, report.left.is_some());
    if let Some((hp, ap)) = report.left {
        text(LEFT_HP, format!("{hp:.0}"));
        text(LEFT_AP, format!("{ap:.0}"));
    }
    let hit = report.by.is_some() && report.hits > 0;
    show(HITS, hit);
    text(HITS_N, report.hits.to_string());
    show(STATS, hurt || report.left.is_some() || hit);

    // the countdown; a round mode keeps the dead waiting for the next round
    text(
        LABEL,
        if wait {
            "WAITING TO REJOIN"
        } else {
            "RESPAWN IN"
        }
        .to_owned(),
    );
    text(
        COUNTER,
        if wait {
            String::new()
        } else {
            (dead.respawn.ceil() as u32).to_string()
        },
    );
    show(TRACK, !wait);
    if let Ok(mut n) = nodes.get_mut(at[FILL]) {
        n.width = percent((1.0 - dead.respawn / max.max(0.01)).clamp(0.0, 1.0) * 100.0);
    }
}
