//! The bottom-left player panel: a portrait box (the player's initial on hatching: a live
//! face-cam would need a second `Camera3d`, which every `Query<_, With<Camera3d>>`'s `single()`
//! in the game would trip over), level diamond, name, a blinking CRITICAL tag, the armour
//! segments, the health bar with its lagging damage trail, the status effects, and the red
//! low-health vignette. The skewed bars are drawn as small supersampled images (Bevy UI has no
//! skew), repainted when their values move. A child of `hud.rs`.

use super::{Hud, LOW_HP, Root, set};
use crate::{
    game::{Dead, Player, Status, Vitals},
    gore::{paint, smooth},
    phase::{MatchApp, MatchSystems},
    profile::Profile,
    ui::{self, ACCENT, Cut, INK, ON_ACCENT, PLATE},
};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.init_match_resource::<Shown>().add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<Panel>))),
            update.run_if(resource_exists::<Hud>),
        )
            .in_set(MatchSystems),
    );
}

/// The mock's critical red (`#ff3b30`), the bars' fills and the white of a healthy one.
const CRITICAL: Color = Color::srgb(1.0, 0.231, 0.188);
const FILL: Color = Color::srgb(0.957, 0.961, 0.969);
/// Bar sizes (px), the slack the skew and glow need around them, and `tan(24°)` of the skew.
const HP: Vec2 = Vec2::new(280.0, 16.0);
const AP: Vec2 = Vec2::new(281.0, 8.0);
const HP_PAD: Vec2 = Vec2::new(8.0, 4.0);
const AP_PAD: Vec2 = Vec2::new(8.0, 6.0);
const TAN: f32 = 0.445;
/// Image texels per px.
const SS: f32 = 2.0;
/// Seconds the damage trail waits and then takes to drain (the mock's `.6s ease .35s`) and the
/// fill's own catch-up.
const WAIT: f32 = 0.35;
const DRAIN: f32 = 0.6;
const CATCH: f32 = 0.12;

#[derive(Component)]
struct Panel;

#[derive(Component, Clone, Copy)]
enum Txt {
    Hp,
    Ap,
    Name,
    Level,
    Initial,
    Status,
}

#[derive(Component, Clone, Copy, PartialEq)]
enum Bar {
    Hp,
    Ap,
}

/// The CRITICAL tag, and the red screen-edge vignette.
#[derive(Component)]
struct Tag;

#[derive(Component)]
struct Vignette;

/// What the bars show: fill, the trail's tween (from, to, seconds since the change), the target
/// it follows, and the last painted state of each bar.
#[derive(Resource, Default)]
struct Shown {
    fill: f32,
    trail: (f32, f32, f32),
    target: f32,
    init: bool,
    drawn: [(i32, i32, bool); 2],
}

type Rgba = [f32; 4];
const CLEAR: Rgba = [1.0, 1.0, 1.0, 0.0];
/// The empty part of a bar: a dark plate, so the white fill reads over bright scenes.
const TRACK: Rgba = [0.02, 0.022, 0.028, 0.62];
/// The dark margin of [`MARGIN_W`] px around a bar.
const MARGIN: Rgba = [0.02, 0.022, 0.028, 0.5];
const MARGIN_W: f32 = 3.0;

/// The plate around a `size` bar at its own pixel (`x`, `y`): TRACK inside, MARGIN outside.
fn plate(x: f32, y: f32, size: Vec2) -> Rgba {
    let d = (-x).max(x - size.x).max(-y).max(y - size.y);
    match d {
        d if d <= 0.0 => TRACK,
        d if d < MARGIN_W => MARGIN,
        _ => CLEAR,
    }
}
const GAP: Rgba = [0.04, 0.043, 0.05, 0.7];
const ORANGE: Rgba = [1.0, 0.416, 0.122, 1.0];
const BLUE_RGB: [f32; 3] = [0.561, 0.753, 1.0];
const AP_TEXT: Color = Color::srgb(0.561, 0.753, 1.0);

/// `s` over `d` (straight alpha).
fn over(d: Rgba, s: Rgba) -> Rgba {
    let a = s[3] + d[3] * (1.0 - s[3]);
    if a <= 0.0 {
        return CLEAR;
    }
    let m = |i: usize| (s[i] * s[3] + d[i] * d[3] * (1.0 - s[3])) / a;
    [m(0), m(1), m(2), a]
}

/// A `size` bar skewed by -24° (top edge shifted right, like CSS `skewX(-24deg)`), `pad` of
/// room around it; `f(x, y)` paints the bar's own unskewed pixel.
fn skewed(size: Vec2, pad: Vec2, f: impl Fn(f32, f32) -> Rgba) -> Image {
    let full = size + 2.0 * pad;
    paint((full.x * SS) as u32, (full.y * SS) as u32, |u, v| {
        let (x, y) = (u * full.x - pad.x, v * full.y - pad.y);
        f(x - (size.y / 2.0 - y) * TAN, y)
    })
}

/// The health bar: dark track, orange trail, fill (red when `low`), a divider after every 10%.
fn hp_image(fill: f32, trail: f32, low: bool) -> Image {
    let col = if low { CRITICAL } else { FILL }.to_srgba().to_f32_array();
    let cell = HP.x / 10.0;
    skewed(HP, HP_PAD, |x, y| {
        let mut c = plate(x, y, HP);
        if c != TRACK {
            return c;
        }
        if x < trail * HP.x {
            c = over(c, ORANGE);
        }
        if x < fill * HP.x {
            c = over(c, col);
        }
        if x % cell >= cell - 2.0 {
            c = over(c, GAP);
        }
        c
    })
}

/// Five armour segments with their glow, filled left to right by `frac` of the whole.
fn ap_image(frac: f32) -> Image {
    let sw = (AP.x - 12.0) / 5.0;
    let pitch = sw + 3.0;
    let fills: [f32; 5] = std::array::from_fn(|i| (frac * 5.0 - i as f32).clamp(0.0, 1.0));
    let [r, g, b] = BLUE_RGB;
    skewed(AP, AP_PAD, |x, y| {
        let seg = (x >= 0.0).then(|| ((x / pitch) as usize, x % pitch));
        let inside = (0.0..AP.y).contains(&y);
        let mut c = match seg {
            Some((i, at)) if i < 5 && at < sw && inside => TRACK,
            _ => match plate(x, y, AP) {
                TRACK => MARGIN,
                m => m,
            },
        };
        for (i, f) in fills.iter().enumerate().filter(|(_, f)| **f > 0.0) {
            let x0 = i as f32 * pitch;
            let d = (x0 - x)
                .max(x - x0 - f * sw)
                .max(0.0)
                .hypot((-y).max(y - AP.y).max(0.0));
            if d < 6.0 {
                c = over(c, [r, g, b, 0.3 * (1.0 - d / 6.0).powi(2)]);
            }
        }
        if let Some((i, at)) = seg.filter(|_| inside)
            && i < 5
            && at < fills[i] * sw
        {
            c = over(c, [r, g, b, 1.0]);
        }
        c
    })
}

fn txt(t: Txt, font: TextFont, color: Color) -> impl Bundle {
    (t, Text::new(""), font, TextColor(color), ui::shadow())
}

/// A skewed bar's wrapper (the bar's layout box) holding its image, which overhangs it by `pad`.
fn bar(kind: Bar, image: Handle<Image>, size: Vec2, pad: Vec2, left: f32) -> impl Bundle {
    (
        Node {
            width: px(size.x),
            height: px(size.y),
            margin: UiRect::left(px(left)),
            flex_shrink: 0.0,
            ..default()
        },
        children![(
            kind,
            Node {
                position_type: PositionType::Absolute,
                left: px(-pad.x),
                top: px(-pad.y),
                width: px(size.x + 2.0 * pad.x),
                height: px(size.y + 2.0 * pad.y),
                ..default()
            },
            ImageNode::new(image),
        )],
    )
}

/// The number at the right of a bar row.
fn number(t: Txt, font: TextFont, color: Color) -> impl Bundle {
    (
        Node {
            width: px(40),
            justify_content: JustifyContent::End,
            ..default()
        },
        children![txt(t, font, color)],
    )
}

fn row() -> Node {
    Node {
        width: percent(100),
        align_items: AlignItems::Center,
        column_gap: px(10),
        ..default()
    }
}

/// The mock's edge vignette (`ellipse 80% 75%`, clear to 55%, red from there) at `k` of its
/// alpha, scaled to 0.65 (Bevy blends in linear space, which reads brighter than the CSS).
fn vignette_gradient(k: f32) -> BackgroundGradient {
    let stop = |c: [u8; 3], a: f32, at: f32| {
        ColorStop::new(
            Color::srgba_u8(c[0], c[1], c[2], (a * k * 255.0) as u8),
            percent(at),
        )
    };
    RadialGradient::new(
        UiPosition::CENTER,
        RadialGradientShape::Ellipse(percent(80), percent(75)),
        vec![
            stop([200, 20, 12], 0.0, 55.0),
            stop([200, 20, 12], 0.22, 85.0),
            stop([140, 0, 0], 0.45, 100.0),
        ],
    )
    .into()
}

fn spawn(
    mut commands: Commands,
    root: Single<Entity, With<Root>>,
    mut images: ResMut<Assets<Image>>,
) {
    let (hp, ap) = (
        images.add(hp_image(1.0, 1.0, false)),
        images.add(ap_image(1.0)),
    );
    let diamond = LinearGradient::to_bottom_right(vec![
        ColorStop::new(Color::srgb_u8(0xff, 0x8a, 0x3d), percent(0)),
        ColorStop::new(Color::srgb_u8(0xb8, 0x40, 0x0c), percent(100)),
    ]);
    commands.entity(*root).with_children(|r| {
        r.spawn((
            Vignette,
            ZIndex(-1),
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                width: percent(100),
                height: percent(100),
                ..default()
            },
            vignette_gradient(1.0),
        ));
        r.spawn((
            Panel,
            Node {
                position_type: PositionType::Absolute,
                left: px(28),
                bottom: px(28),
                width: px(420),
                column_gap: px(14),
                align_items: AlignItems::End,
                ..default()
            },
        ))
        .with_children(|p| {
            p.spawn((
                Node {
                    width: px(72),
                    height: px(88),
                    flex_shrink: 0.0,
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                ui::shape(Cut::Notch(12), PLATE),
                children![
                    (
                        Node {
                            position_type: PositionType::Absolute,
                            width: percent(100),
                            height: percent(100),
                            ..default()
                        },
                        ui::hatch(Color::srgba(1.0, 1.0, 1.0, 0.008)),
                    ),
                    txt(Txt::Initial, ui::display(44.0), INK.with_alpha(0.18)),
                    (
                        Node {
                            position_type: PositionType::Absolute,
                            left: px(0),
                            bottom: px(0),
                            width: px(62),
                            height: px(3),
                            ..default()
                        },
                        BackgroundColor(ACCENT),
                    ),
                ],
            ));
            p.spawn(Node {
                flex_grow: 1.0,
                min_width: px(0),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            })
            .with_children(|c| {
                // status effects, above the name row
                c.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        bottom: percent(100),
                        margin: UiRect::bottom(px(6)),
                        ..default()
                    },
                    children![txt(Txt::Status, ui::compact(16.0), ui::DANGER)],
                ));
                c.spawn(row()).with_children(|n| {
                    n.spawn((
                        Node {
                            width: px(26),
                            height: px(26),
                            flex_shrink: 0.0,
                            justify_content: JustifyContent::Center,
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        children![
                            (
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: px(4),
                                    top: px(4),
                                    width: px(18),
                                    height: px(18),
                                    ..default()
                                },
                                UiTransform::from_rotation(Rot2::degrees(45.0)),
                                BackgroundGradient::from(diamond),
                            ),
                            txt(Txt::Level, ui::display(14.0), ON_ACCENT),
                        ],
                    ));
                    n.spawn((
                        txt(Txt::Name, ui::display(26.0), INK),
                        ui::tracking(0.06, 26.0),
                    ));
                    n.spawn((
                        Tag,
                        Visibility::Hidden,
                        Node {
                            margin: UiRect::left(Val::Auto),
                            padding: UiRect::axes(px(8), px(2)),
                            ..default()
                        },
                        BackgroundColor(CRITICAL),
                        children![(
                            Text::new("CRITICAL"),
                            ui::strong(10.0),
                            ui::tracking(0.2, 10.0),
                            TextColor(ON_ACCENT),
                        )],
                    ));
                });
                c.spawn(row()).with_children(|n| {
                    n.spawn(bar(Bar::Ap, ap, AP, AP_PAD, 3.0));
                    n.spawn(number(Txt::Ap, ui::digits(14.0), AP_TEXT));
                });
                c.spawn(row()).with_children(|n| {
                    n.spawn(bar(Bar::Hp, hp, HP, HP_PAD, 4.0));
                    n.spawn(number(Txt::Hp, ui::display(28.0), Color::WHITE));
                });
            });
        });
    });
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update(
    real: Res<Time<Real>>,
    player: Query<(&Vitals, Has<Dead>, Option<&Status>, Option<&Name>), With<Player>>,
    profile: Option<Res<Profile>>,
    mut images: ResMut<Assets<Image>>,
    mut st: ResMut<Shown>,
    bars: Query<(&Bar, &ImageNode)>,
    mut texts: Query<(&Txt, &mut Text, &mut TextColor)>,
    mut tag: Query<(&mut BackgroundColor, &mut Visibility), (With<Tag>, Without<Vignette>)>,
    mut vignette: Query<(&mut BackgroundGradient, &mut Visibility), With<Vignette>>,
) {
    let Ok((vitals, dead, status, name)) = player.single() else {
        return;
    };
    let dt = real.delta_secs();
    let hp = (vitals.hp / vitals.max_hp.max(1.0)).clamp(0.0, 1.0);
    let ap = (vitals.ap / vitals.max_ap.max(1.0)).clamp(0.0, 1.0);
    let low = !dead && hp < LOW_HP;
    if !st.init {
        *st = Shown {
            fill: hp,
            trail: (hp, hp, 0.0),
            target: hp,
            init: true,
            drawn: [(-1, 0, false); 2],
        };
    }
    // the fill follows within a blink; the trail waits, then eases down (or up) to the fill
    if (hp - st.target).abs() > 1e-4 {
        let now = st.trail.0 + (st.trail.1 - st.trail.0) * smooth(WAIT, WAIT + DRAIN, st.trail.2);
        (st.trail, st.target) = ((now, hp, 0.0), hp);
    }
    st.trail.2 += dt;
    let trail = st.trail.0 + (st.trail.1 - st.trail.0) * smooth(WAIT, WAIT + DRAIN, st.trail.2);
    st.fill += (hp - st.fill) * (dt / CATCH).min(1.0);
    let px = |f: f32, w: f32| (f * w * SS) as i32;
    let keys = [
        (px(st.fill, HP.x), px(trail, HP.x), low),
        (px(ap, AP.x), 0, false),
    ];
    for (b, img) in &bars {
        let k = (*b == Bar::Ap) as usize;
        if st.drawn[k] != keys[k] {
            st.drawn[k] = keys[k];
            let image = match b {
                Bar::Hp => hp_image(st.fill, trail, low),
                Bar::Ap => ap_image(ap),
            };
            let _ = images.insert(&img.image, image);
        }
    }
    let name = name
        .map(|n| n.as_str())
        .or(profile.as_ref().map(|p| p.name.as_str()))
        .unwrap_or("")
        .to_uppercase();
    for (t, mut text, mut color) in &mut texts {
        let (s, c) = match t {
            Txt::Hp => (
                format!("{}", vitals.hp.ceil()),
                if low { CRITICAL } else { Color::WHITE },
            ),
            Txt::Ap => (format!("{}", vitals.ap.ceil()), color.0),
            Txt::Name => (name.clone(), color.0),
            Txt::Level => (
                profile
                    .as_ref()
                    .map_or(String::new(), |p| p.level().to_string()),
                color.0,
            ),
            Txt::Initial => (
                name.chars().next().map(String::from).unwrap_or_default(),
                color.0,
            ),
            Txt::Status => (
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
                color.0,
            ),
        };
        set(&mut text, s);
        if color.0 != c {
            color.0 = c;
        }
    }
    let want = if low {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    let t = real.elapsed_secs();
    // CRITICAL blinks 1 -> .35 every .8 s; the vignette breathes .4 -> .95 every second
    for (mut bg, mut v) in &mut tag {
        if *v != want {
            *v = want;
        }
        bg.0 = CRITICAL.with_alpha(0.675 + 0.325 * (t * std::f32::consts::TAU / 0.8).cos());
    }
    for (mut g, mut v) in &mut vignette {
        if *v != want {
            *v = want;
        }
        if low {
            *g = vignette_gradient(0.675 - 0.275 * (t * std::f32::consts::TAU).cos());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alpha_at(img: &Image, x: f32, y: f32) -> u8 {
        let i = ((y * SS) as usize * img.width() as usize + (x * SS) as usize) * 4 + 3;
        img.data.as_ref().unwrap()[i]
    }

    #[test]
    fn hp_bar_fills_left_to_right() {
        let img = hp_image(0.5, 0.5, false);
        let y = HP_PAD.y + HP.y / 2.0;
        // the skew is zero at mid-height: image x = pad + bar x
        assert_eq!(alpha_at(&img, HP_PAD.x + 20.0, y), 255);
        // the empty part is the dark plate: translucent, not clear
        let track = alpha_at(&img, HP_PAD.x + 0.75 * HP.x, y);
        assert!((100..255).contains(&track), "{track}");
        assert_eq!(alpha_at(&img, 0.0, y), 0);
    }
}
