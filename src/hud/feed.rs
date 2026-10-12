//! Kill feed: one slanted row per kill at the top right, newest at the bottom: killer, a light
//! chip with the weapon's short type, a red HEADSHOT tag, victim; clan wars add each side's
//! emblem. The player's kills are orange, the player's deaths dark red, the rest dark. Rows slide
//! and fade in, then fade out and collapse. The player's own kill also shows a kill confirmation
//! below the crosshair: "ELIMINATED" over the victim's name, with a HEADSHOT or MELEE tag when it
//! applies. A child of `hud.rs`.

use super::{
    Root,
    top::{Edge, back},
    weapon::short,
};
use crate::phase::MatchSystems;
use crate::{
    actor::ActorData,
    clan::{ClanArt, ClanWar, Member, mark},
    combat::is_melee,
    game::{Killed, Player, Team},
    ui::{self, ACCENT, BLUE, INK, ON_ACCENT, PLATE, RED},
};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<FeedBox>))),
            (add, animate).chain(),
        )
            .in_set(MatchSystems),
    );
}

/// Seconds a row stays, its slide/fade in and fade out, and how many rows show at once.
const LIFE: f32 = 6.0;
const ENTER: f32 = 0.25;
const LEAVE: f32 = 0.3;
const LINES: usize = 4;
const ROW_H: f32 = 28.0;
/// Pixels a row slides in from.
const ENTER_X: f32 = 30.0;

/// Kill confirmation timing: scale-in, hold at full size, then fade.
const CONFIRM_IN: f32 = 0.12;
const CONFIRM_HOLD: f32 = 1.4;
const CONFIRM_OUT: f32 = 0.25;

/// The mock's row fills, name colours and the HEADSHOT red.
const DEATH_FILL: Color = Color::srgba(0.588, 0.07, 0.047, 0.75);
const LIGHT: Color = Color::srgb(0.941, 0.945, 0.953);
const SOFT: Color = Color::srgb(0.769, 0.78, 0.8);
const PINK: Color = Color::srgb(1.0, 0.816, 0.784);
const HEAD: Color = Color::srgb(1.0, 0.353, 0.29);
/// The weapon chip's text (`#0b0c0e`).
const CHIP_INK: Color = Color::srgb(0.043, 0.047, 0.055);

#[derive(Component)]
struct FeedBox;

/// A child text/chip that fades with its row or the kill confirmation: `0` is its full opacity
/// colour, restored each frame and multiplied by the current fade fraction.
#[derive(Component)]
struct Fade(Color);

/// A feed row: when it came and when it started to leave.
#[derive(Component)]
struct Row {
    born: f32,
    leave: Option<f32>,
}

/// The kill confirmation below the crosshair; `start` is when the latest kill set it.
#[derive(Component)]
struct Confirm {
    start: f32,
}

/// The victim name line of the confirmation.
#[derive(Component)]
struct ConfirmName;

/// The HEADSHOT / MELEE tag of the confirmation (hidden when neither applies).
#[derive(Component)]
struct ConfirmTag;

/// A killer/victim name.
fn name(s: &str, color: Color) -> impl Bundle {
    (
        Fade(color),
        Text::new(s),
        ui::strong(14.0),
        TextColor(color),
    )
}

/// The light chip with the weapon's short type.
fn chip(label: &str) -> impl Bundle {
    (
        Node {
            padding: UiRect::axes(px(7), px(2)),
            ..default()
        },
        children![
            (Fade(INK), back(INK, Edge::Both, 4)),
            (
                Fade(CHIP_INK),
                Text::new(label),
                ui::mono(9.0),
                ui::tracking(0.08, 9.0),
                TextColor(CHIP_INK),
            ),
        ],
    )
}

/// A small red caps tag (HEADSHOT, MELEE).
fn tag(label: &str, size: f32) -> impl Bundle {
    (
        Fade(HEAD),
        Text::new(label),
        ui::strong(size),
        ui::tracking(0.14, size),
        TextColor(HEAD),
    )
}

fn spawn(mut commands: Commands, root: Single<Entity, With<Root>>) {
    commands.spawn((
        FeedBox,
        ChildOf(*root),
        Node {
            position_type: PositionType::Absolute,
            right: px(28),
            top: px(24),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            row_gap: px(3),
            ..default()
        },
    ));
    // Kill confirmation: a centred column just below the crosshair.
    commands
        .spawn((
            ChildOf(*root),
            Node {
                position_type: PositionType::Absolute,
                left: px(0),
                top: percent(56),
                width: percent(100),
                justify_content: JustifyContent::Center,
                ..default()
            },
        ))
        .with_children(|r| {
            r.spawn((
                Confirm { start: f32::MIN },
                Visibility::Hidden,
                UiTransform::default(),
                Node {
                    flex_direction: FlexDirection::Column,
                    align_items: AlignItems::Center,
                    row_gap: px(2),
                    ..default()
                },
                children![
                    (
                        Fade(ACCENT),
                        Text::new("ELIMINATED"),
                        ui::strong(12.0),
                        ui::tracking(0.3, 12.0),
                        TextColor(ACCENT),
                        ui::shadow(),
                    ),
                    (
                        ConfirmName,
                        Fade(INK),
                        Text::new(""),
                        ui::display(30.0),
                        TextColor(INK),
                        ui::shadow(),
                    ),
                    (ConfirmTag, Visibility::Hidden, tag("", 12.0)),
                ],
            ));
        });
}

#[allow(clippy::too_many_arguments)]
fn add(
    mut commands: Commands,
    time: Res<Time>,
    data: Res<ActorData>,
    mut killed: MessageReader<Killed>,
    feed: Single<Entity, With<FeedBox>>,
    names: Query<&Name>,
    teams: Query<&Team>,
    players: Query<(), With<Player>>,
    war: Option<Res<ClanWar>>,
    clan_art: Option<Res<ClanArt>>,
    members: Query<&Member>,
    mut confirm: Single<&mut Confirm>,
    mut confirm_name: Single<&mut Text, (With<ConfirmName>, Without<ConfirmTag>)>,
    mut confirm_tag: Single<(&mut Text, &mut Visibility), (With<ConfirmTag>, Without<ConfirmName>)>,
) {
    let now = time.elapsed_secs();
    for k in killed.read() {
        let suicide = k.killer == k.victim;
        let you = players.contains(k.killer) && !suicide;
        let died = players.contains(k.victim);
        let kind = data
            .items
            .get(k.item)
            .and_then(|i| i.weapon.as_ref())
            .map(|w| w.kind);
        if you {
            confirm.start = now;
            confirm_name.0 = names
                .get(k.victim)
                .map_or("?", |n| n.as_str())
                .to_uppercase();
            let melee = kind.is_some_and(is_melee);
            let (text, vis) = &mut *confirm_tag;
            match (k.head, melee) {
                (true, _) => {
                    text.0 = "HEADSHOT".into();
                    **vis = Visibility::Inherited;
                }
                (false, true) => {
                    text.0 = "MELEE".into();
                    **vis = Visibility::Inherited;
                }
                (false, false) => **vis = Visibility::Hidden,
            }
        }
        let tint = |e: Entity, base: Color| match teams.get(e) {
            Ok(Team::Red) => RED,
            Ok(Team::Blue) => BLUE,
            _ => base,
        };
        let (fill, killer_ink, victim_ink) = match (you, died) {
            (true, _) => (ACCENT.with_alpha(0.8), ON_ACCENT, ON_ACCENT),
            (_, true) => (DEATH_FILL, tint(k.killer, LIGHT), PINK),
            _ => (PLATE, tint(k.killer, LIGHT), tint(k.victim, SOFT)),
        };
        // in a clan war: the side's emblem and the name without the clan tag
        let who = |e: Entity| {
            let member = war.as_ref().zip(clan_art.as_ref()).zip(members.get(e).ok());
            let name = member.map_or_else(
                || names.get(e).map_or("?", |n| n.as_str()).to_owned(),
                |(_, m)| m.name.clone(),
            );
            let emblem = member.map(|((w, a), m)| {
                let s = &w.sides[m.side];
                mark(a, s.emblem, s.bg, 20.0)
            });
            (name, emblem)
        };
        let label = match kind {
            Some(kind) => short(kind),
            None if suicide => "FALL",
            None => "KILLED",
        };
        commands
            .spawn((
                Row {
                    born: now,
                    leave: None,
                },
                ChildOf(*feed),
                UiTransform::from_translation(Val2::px(ENTER_X, 0.0)),
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(10),
                    height: px(ROW_H),
                    padding: UiRect::new(px(16), px(12), px(0), px(0)),
                    overflow: Overflow::clip(),
                    ..default()
                },
            ))
            .with_children(|r| {
                r.spawn((Fade(fill), back(fill, Edge::Left, 8)));
                let person = |r: &mut ChildSpawnerCommands, e, ink| {
                    let (n, emblem) = who(e);
                    if let Some(m) = emblem {
                        r.spawn(m);
                    }
                    r.spawn(name(&n, ink));
                };
                if !suicide {
                    person(r, k.killer, killer_ink);
                }
                r.spawn(chip(label));
                if k.head {
                    r.spawn(tag("HEADSHOT", 9.0));
                }
                person(r, k.victim, victim_ink);
            });
    }
}

type Parts<'w, 's> = Query<
    'w,
    's,
    (
        &'static Fade,
        Option<&'static mut TextColor>,
        Option<&'static mut ImageNode>,
    ),
    Without<Row>,
>;

/// Applies `frac` (1 = full opacity) to every [`Fade`] below a row or the confirmation.
fn fade(children: &Children, frac: f32, kids: &Query<&Children>, parts: &mut Parts) {
    for &e in children {
        if let Ok((f, text, img)) = parts.get_mut(e) {
            let c = f.0.with_alpha(f.0.alpha() * frac);
            if let Some(mut t) = text {
                t.0 = c;
            }
            if let Some(mut i) = img {
                i.color = c;
            }
        }
        if let Ok(k) = kids.get(e) {
            fade(k, frac, kids, parts);
        }
    }
}

/// A row's slide (px from its place) and opacity `age` seconds after it came.
fn slide(age: f32) -> (f32, f32) {
    let f = ui::ease_out(age / ENTER);
    (ENTER_X * (1.0 - f), f)
}

#[allow(clippy::type_complexity)]
fn animate(
    mut commands: Commands,
    time: Res<Time>,
    mut rows: Query<
        (Entity, &mut Row, &mut UiTransform, &mut Node, &Children),
        (Without<Confirm>, Without<Fade>),
    >,
    kids: Query<&Children>,
    mut parts: Parts,
    mut confirm: Single<(&mut Confirm, &mut Visibility, &mut UiTransform, &Children), Without<Row>>,
) {
    let now = time.elapsed_secs();
    // oldest first: the rows past `LINES` and past their life start to leave
    let mut live: Vec<_> = rows
        .iter()
        .filter(|r| r.1.leave.is_none())
        .map(|r| (r.1.born, r.0))
        .collect();
    live.sort_by(|a, b| a.0.total_cmp(&b.0));
    let extra = live.len().saturating_sub(LINES);
    for (i, (born, e)) in live.into_iter().enumerate() {
        if (i < extra || now - born > LIFE)
            && let Ok((_, mut row, ..)) = rows.get_mut(e)
        {
            row.leave = Some(now);
        }
    }
    for (e, row, mut tf, mut node, children) in &mut rows {
        if row.leave.is_none() && now - row.born >= ENTER {
            // fully entered and not leaving: settled, nothing changes until it leaves
            continue;
        }
        let (x, frac) = match row.leave {
            Some(t) => {
                let t = (now - t) / LEAVE;
                if t >= 1.0 {
                    commands.entity(e).despawn();
                    continue;
                }
                node.height = px(ROW_H * (1.0 - ui::ease_out(t)));
                (0.0, 1.0 - ui::ease_out(t))
            }
            None => slide(now - row.born),
        };
        tf.translation = Val2::px(x, 0.0);
        fade(children, frac, &kids, &mut parts);
    }
    let (c, vis, tf, children) = &mut *confirm;
    let age = now - c.start;
    let total = CONFIRM_IN + CONFIRM_HOLD + CONFIRM_OUT;
    if age > total {
        if **vis != Visibility::Hidden {
            **vis = Visibility::Hidden;
        }
        return;
    }
    if **vis != Visibility::Inherited {
        **vis = Visibility::Inherited;
    }
    if age < CONFIRM_IN {
        let f = ui::ease_out(age / CONFIRM_IN);
        tf.scale = Vec2::splat(0.82 + 0.18 * f);
        fade(children, f, &kids, &mut parts);
    } else if age < CONFIRM_IN + CONFIRM_HOLD {
        // steady hold: apply once, not every frame
        if tf.scale != Vec2::ONE {
            tf.scale = Vec2::ONE;
            fade(children, 1.0, &kids, &mut parts);
        }
    } else {
        let f = 1.0 - ui::ease_out((age - CONFIRM_IN - CONFIRM_HOLD) / CONFIRM_OUT);
        fade(children, f, &kids, &mut parts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_slide_in_from_the_right_and_settle() {
        assert_eq!(slide(0.0), (ENTER_X, 0.0));
        let (x, f) = slide(ENTER);
        assert!(x.abs() < 1e-4 && (f - 1.0).abs() < 1e-4);
        let (mid_x, mid_f) = slide(ENTER / 2.0);
        assert!(mid_x > 0.0 && mid_x < ENTER_X && mid_f > 0.0 && mid_f < 1.0);
    }
}
