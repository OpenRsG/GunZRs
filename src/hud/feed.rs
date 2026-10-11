//! Kill feed: one row per kill at the top right, newest at the bottom: killer, the retail
//! kill-log weapon icon (`Ingame_KillLogArrow_N` of `bitmapalias.xml`, cut from
//! `ingame_01.png`), a HEADSHOT tag, victim; clan wars add each side's emblem. Rows slide/fade
//! in, then fade out and collapse. The player's own kill also shows a kill confirmation below
//! the crosshair: "ELIMINATED" over the victim's name, with a HEADSHOT or MELEE tag when it
//! applies. A child of `hud.rs`.

use super::{Root, try_image};
use crate::phase::MatchSystems;
use crate::{
    actor::ActorData,
    clan::{ClanArt, ClanWar, Member, mark},
    combat::is_melee,
    game::{Killed, Player, Team},
    item::WeaponKind,
    level::Level,
    ui::{self, ACCENT, BLUE, DANGER, DIM, INK, PLATE, RED},
};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<FeedBox>))),
            (add, animate).chain().run_if(resource_exists::<FeedArt>),
        )
            .in_set(MatchSystems),
    );
}

/// Seconds a row stays, its slide/fade in and fade out, and how many rows show at once.
const LIFE: f32 = 6.0;
const ENTER: f32 = 0.15;
const LEAVE: f32 = 0.3;
const LINES: usize = 6;
const ROW_H: f32 = 30.0;
/// Pixels a row slides in from.
const ENTER_X: f32 = 56.0;

/// Kill confirmation timing: scale-in, hold at full size, then fade.
const CONFIRM_IN: f32 = 0.12;
const CONFIRM_HOLD: f32 = 1.4;
const CONFIRM_OUT: f32 = 0.25;

#[derive(Resource)]
struct FeedArt {
    log: Handle<Image>,
}

#[derive(Component)]
struct FeedBox;

/// A child text/icon/tag that fades with its row or the kill confirmation: `0` is its full
/// opacity colour, restored each frame and multiplied by the current fade fraction.
#[derive(Component)]
struct Fade(Color);

/// A feed row: when it came, when it started to leave, and its left accent bar colour.
#[derive(Component)]
struct Row {
    born: f32,
    leave: Option<f32>,
    edge: Color,
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

/// Retail kill-log icon index (the comments in `bitmapalias.xml` name each one).
fn log_icon(kind: WeaponKind) -> usize {
    use WeaponKind::*;
    match kind {
        Dagger => 0,
        Katana | SpyCase => 1,
        DoubleKatana => 2,
        Pistol => 3,
        PistolX2 => 4,
        Revolver => 5,
        RevolverX2 => 6,
        Smg => 7,
        SmgX2 => 8,
        Shotgun => 9,
        Rifle => 10,
        MachineGun => 11,
        Rocket => 12,
        Frag | Flashbang | Smoke | Stun | Mine => 13,
        Medikit | Potion | RepairKit => FALL,
    }
}

/// The falling man: suicides and falls.
const FALL: usize = 14;

/// Icon `i` of the 4-wide grid of 108 x 29 cells from (0, 799) of `ingame_01.png`.
fn log_rect(i: usize) -> Rect {
    let (x, y) = ((i % 4) as f32 * 109.0, 799.0 + (i / 4) as f32 * 30.0);
    Rect::new(x, y, x + 108.0, y + 29.0)
}

/// A killer/victim name, or the fallback "killed" label.
fn text(s: &str, color: Color) -> impl Bundle {
    (
        Fade(color),
        Text::new(s),
        ui::strong(16.0),
        TextColor(color),
        ui::shadow(),
    )
}

/// A small caps tag (HEADSHOT, MELEE): a hairline box in `color`.
fn tag(label: impl Into<String>, color: Color) -> impl Bundle {
    (
        Fade(color),
        Node {
            padding: UiRect::axes(px(6), px(1)),
            border: UiRect::all(px(1)),
            align_items: AlignItems::Center,
            ..default()
        },
        BorderColor::all(color),
        Text::new(label.into()),
        ui::strong(12.0),
        TextColor(color),
        ui::shadow(),
    )
}

fn spawn(
    mut commands: Commands,
    level: Res<Level>,
    mut images: ResMut<Assets<Image>>,
    root: Single<Entity, With<Root>>,
) {
    let Some(log) = try_image(&level.vfs, &mut images, "ingame_01.png") else {
        error!("kill feed: interface/default/ingame_01.png missing");
        commands.spawn(FeedBox);
        return;
    };
    commands.spawn((
        FeedBox,
        ChildOf(*root),
        Node {
            position_type: PositionType::Absolute,
            right: px(12),
            // below the clock, and on a phone below the touch layer's top buttons
            top: px(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::FlexEnd,
            row_gap: px(4),
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
                        ui::strong(14.0),
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
                    (ConfirmTag, Visibility::Hidden, tag("", DANGER)),
                ],
            ));
        });
    commands.insert_resource(FeedArt { log });
}

#[allow(clippy::too_many_arguments)]
fn add(
    mut commands: Commands,
    time: Res<Time>,
    art: Res<FeedArt>,
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
        let color = |e| {
            if players.contains(e) {
                ACCENT
            } else {
                match teams.get(e) {
                    Ok(Team::Red) => RED,
                    Ok(Team::Blue) => BLUE,
                    _ => INK,
                }
            }
        };
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
        let edge = match (you, died) {
            (true, _) => ACCENT,
            (_, true) => DANGER,
            _ => Color::NONE,
        };
        let row = commands
            .spawn((
                Row {
                    born: now,
                    leave: None,
                    edge,
                },
                ChildOf(*feed),
                UiTransform::from_translation(Val2::px(ENTER_X, 0.0)),
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    height: px(ROW_H),
                    padding: UiRect::axes(px(12), px(0)),
                    border: UiRect::left(px(3)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(PLATE),
                BorderColor::all(edge),
            ))
            .id();
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
            (name, emblem, color(e))
        };
        commands.entity(row).with_children(|r| {
            let name = |r: &mut ChildSpawnerCommands, e| {
                let (n, emblem, c) = who(e);
                if let Some(m) = emblem {
                    r.spawn(m);
                }
                r.spawn(text(&n, c));
            };
            if !suicide {
                name(r, k.killer);
            }
            let icon = match kind {
                _ if suicide => Some(FALL),
                Some(kind) => Some(log_icon(kind)),
                None => None,
            };
            match icon {
                Some(i) => drop(r.spawn((
                    Fade(Color::WHITE),
                    Node {
                        width: px(82),
                        height: px(22),
                        ..default()
                    },
                    ImageNode {
                        rect: Some(log_rect(i)),
                        image_mode: NodeImageMode::Stretch,
                        ..ImageNode::new(art.log.clone())
                    },
                ))),
                None => drop(r.spawn(text("killed", DIM))),
            }
            if k.head {
                r.spawn(tag("HEADSHOT", DANGER));
            }
            name(r, k.victim);
        });
    }
}

/// Applies `frac` (1 = full opacity) to every direct [`Fade`] child of a row or the confirmation.
fn fade_children(
    children: &Children,
    frac: f32,
    parts: &mut Query<
        (
            &Fade,
            Option<&mut TextColor>,
            Option<&mut BorderColor>,
            Option<&mut ImageNode>,
        ),
        Without<Row>,
    >,
) {
    for &e in children {
        let Ok((fade, text, border, img)) = parts.get_mut(e) else {
            continue;
        };
        let c = fade.0.with_alpha(fade.0.alpha() * frac);
        if let Some(mut t) = text {
            t.0 = c;
        }
        if let Some(mut b) = border {
            b.set_all(c);
        }
        if let Some(mut i) = img {
            i.color = c;
        }
    }
}

#[allow(clippy::type_complexity)]
fn animate(
    mut commands: Commands,
    time: Res<Time>,
    mut rows: Query<
        (
            Entity,
            &mut Row,
            &mut UiTransform,
            &mut Node,
            &mut BackgroundColor,
            &mut BorderColor,
            &Children,
        ),
        (Without<Confirm>, Without<Fade>),
    >,
    mut parts: Query<
        (
            &Fade,
            Option<&mut TextColor>,
            Option<&mut BorderColor>,
            Option<&mut ImageNode>,
        ),
        Without<Row>,
    >,
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
    for (e, row, mut tf, mut node, mut bg, mut border, children) in &mut rows {
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
            None => {
                let f = ui::ease_out((now - row.born) / ENTER);
                (ENTER_X * (1.0 - f), f)
            }
        };
        tf.translation = Val2::px(x, 0.0);
        bg.0 = PLATE.with_alpha(PLATE.alpha() * frac);
        border.set_all(row.edge.with_alpha(row.edge.alpha() * frac));
        fade_children(children, frac, &mut parts);
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
        fade_children(children, f, &mut parts);
    } else if age < CONFIRM_IN + CONFIRM_HOLD {
        // steady hold: apply once, not every frame
        if tf.scale != Vec2::ONE {
            tf.scale = Vec2::ONE;
            fade_children(children, 1.0, &mut parts);
        }
    } else {
        let f = 1.0 - ui::ease_out((age - CONFIRM_IN - CONFIRM_HOLD) / CONFIRM_OUT);
        fade_children(children, f, &mut parts);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_icons_match_the_atlas() {
        // bitmapalias.xml: icon 4 at (0, 829), 11 at (327, 859), 14 at (218, 889)
        assert_eq!(log_rect(4).min, Vec2::new(0.0, 829.0));
        assert_eq!(log_rect(11).min, Vec2::new(327.0, 859.0));
        assert_eq!(log_rect(FALL).min, Vec2::new(218.0, 889.0));
        assert_eq!(log_icon(WeaponKind::Rocket), 12);
    }
}
