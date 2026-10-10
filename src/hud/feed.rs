//! Kill feed: one row per kill at the top right, newest at the bottom: killer, the retail
//! kill-log weapon icon (`Ingame_KillLogArrow_N` of `bitmapalias.xml`, cut from
//! `ingame_01.png`), a HEADSHOT tag, victim; clan wars add each side's emblem. Rows slide in,
//! then slide out and collapse. The player's own headshot kill also flashes the retail
//! `Ingame_KillDirection_HeadShot` banner (`ingame_00.png`). A child of `hud.rs`.

use super::{Root, try_image};
use crate::{
    actor::ActorData,
    clan::{ClanArt, ClanWar, Member, mark},
    game::{Killed, Player, Team},
    item::WeaponKind,
    level::Level,
};
use bevy::prelude::*;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<FeedBox>))),
            (add, animate).chain().run_if(resource_exists::<FeedArt>),
        ),
    );
}

/// Seconds a row stays, its slide in and out, and how many rows show at once.
const LIFE: f32 = 6.0;
const ENTER: f32 = 0.22;
const LEAVE: f32 = 0.3;
const LINES: usize = 6;
const ROW_H: f32 = 30.0;
/// Seconds the headshot banner shows.
const BANNER: f32 = 1.4;

const GOLD: Color = Color::srgb(0.95, 0.78, 0.36);
const RED: Color = Color::srgb(1.0, 0.45, 0.4);
const BLUE: Color = Color::srgb(0.48, 0.68, 1.0);

#[derive(Resource)]
struct FeedArt {
    log: Handle<Image>,
    banner: Handle<Image>,
}

#[derive(Component)]
struct FeedBox;

#[derive(Component)]
struct Banner(f32);

/// A feed row: when it came and when it started to leave.
#[derive(Component)]
struct Row {
    born: f32,
    leave: Option<f32>,
}

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

fn spawn(
    mut commands: Commands,
    level: Res<Level>,
    mut images: ResMut<Assets<Image>>,
    root: Single<Entity, With<Root>>,
) {
    let (Some(log), Some(banner)) = (
        try_image(&level.vfs, &mut images, "ingame_01.png"),
        try_image(&level.vfs, &mut images, "ingame_00.png"),
    ) else {
        error!("kill feed: interface/default/ingame_00.png or ingame_01.png missing");
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
    commands.spawn((
        Banner(0.0),
        ChildOf(*root),
        Visibility::Hidden,
        UiTransform::default(),
        Node {
            position_type: PositionType::Absolute,
            left: percent(50),
            top: percent(22),
            width: px(411),
            height: px(62),
            margin: UiRect::left(px(-205)),
            ..default()
        },
        ImageNode {
            rect: Some(Rect::new(0.0, 596.0, 411.0, 658.0)),
            ..ImageNode::new(banner.clone())
        },
    ));
    commands.insert_resource(FeedArt { log, banner });
}

fn text(s: &str, color: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(17.0),
        TextColor(color),
        TextShadow::default(),
    )
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
    mut banner: Single<(&mut Banner, &mut Visibility)>,
) {
    let now = time.elapsed_secs();
    for k in killed.read() {
        let suicide = k.killer == k.victim;
        let you = players.contains(k.killer) && !suicide;
        let died = players.contains(k.victim);
        let color = |e| {
            if players.contains(e) {
                GOLD
            } else {
                match teams.get(e) {
                    Ok(Team::Red) => RED,
                    Ok(Team::Blue) => BLUE,
                    _ => Color::WHITE,
                }
            }
        };
        let kind = data
            .items
            .get(k.item)
            .and_then(|i| i.weapon.as_ref())
            .map(|w| w.kind);
        if you && k.head {
            *banner.0 = Banner(now);
            *banner.1 = Visibility::Inherited;
        }
        let edge = match (you, died) {
            (true, _) => GOLD,
            (_, true) => Color::srgb(0.85, 0.15, 0.12),
            _ => Color::NONE,
        };
        let row = commands
            .spawn((
                Row {
                    born: now,
                    leave: None,
                },
                ChildOf(*feed),
                UiTransform::from_translation(Val2::px(120.0, 0.0)),
                Node {
                    align_items: AlignItems::Center,
                    column_gap: px(8),
                    height: px(ROW_H),
                    padding: UiRect::axes(px(12), px(0)),
                    border: UiRect::left(px(3)),
                    border_radius: BorderRadius::all(px(8)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                BackgroundColor(Color::srgba(
                    0.03,
                    0.04,
                    0.07,
                    if you { 0.82 } else { 0.62 },
                )),
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
                None => drop(r.spawn(text("killed", Color::srgb(0.7, 0.7, 0.7)))),
            }
            if k.head {
                r.spawn((
                    Node {
                        width: px(78),
                        height: px(18),
                        ..default()
                    },
                    ImageNode {
                        rect: Some(Rect::new(150.0, 604.0, 380.0, 656.0)),
                        image_mode: NodeImageMode::Stretch,
                        ..ImageNode::new(art.banner.clone())
                    },
                ));
            }
            name(r, k.victim);
        });
    }
}

fn ease(t: f32) -> f32 {
    1.0 - (1.0 - t.clamp(0.0, 1.0)).powi(3)
}

fn animate(
    mut commands: Commands,
    time: Res<Time>,
    mut rows: Query<(Entity, &mut Row, &mut UiTransform, &mut Node)>,
    mut banner: Single<(&Banner, &mut Visibility, &mut UiTransform), Without<Row>>,
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
    for (e, row, mut tf, mut node) in &mut rows {
        let x = match row.leave {
            Some(t) => {
                let t = (now - t) / LEAVE;
                if t >= 1.0 {
                    commands.entity(e).despawn();
                    continue;
                }
                node.height = px(ROW_H * (1.0 - ease(t)));
                320.0 * t * t
            }
            None => 120.0 * (1.0 - ease((now - row.born) / ENTER)),
        };
        tf.translation = Val2::px(x, 0.0);
    }
    let (b, vis, tf) = &mut *banner;
    let age = now - b.0;
    if age > BANNER {
        if **vis != Visibility::Hidden {
            **vis = Visibility::Hidden;
        }
    } else {
        tf.scale = Vec2::splat(1.0 + 0.35 * (1.0 - ease(age / 0.18)));
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
