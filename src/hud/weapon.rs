//! Bottom right of the HUD: the loadout slots, the current weapon's name, mode and type, the
//! magazine and reserve counters, the round pips and the reload prompt (the "GunZ HUD" mock).

use super::top::{Edge, back};
use super::{Root, set};
use crate::actor::{Actor, ActorData};
use crate::controls::{Action, DEFAULTS, label};
use crate::game::{Loadout, Player};
use crate::item::{Item, WeaponKind};
use crate::phase::MatchSystems;
use crate::profile::Profile;
use crate::ui::{self, ACCENT, Cut, DANGER, GOLD, INK, ON_ACCENT, TEXT};
use bevy::{prelude::*, text::LineHeight};

/// Slot keys, in loadout order (`actor.rs` picks the slot the same way).
const KEYS: [Action; 5] = [
    Action::Melee,
    Action::Primary,
    Action::Secondary,
    Action::Item1,
    Action::Item2,
];
/// Most pips drawn; a larger magazine still shows its first rounds.
const PIPS: usize = 120;
const WIDTH: f32 = 440.0;
const PLATE: Color = Color::srgba(0.04, 0.043, 0.05, 0.55);

/// The whole panel; on a touch screen it moves to the bottom centre.
#[derive(Component)]
struct WeaponBox;

#[derive(Component, Clone, Copy, PartialEq)]
enum Txt {
    Key(usize),
    Kind(usize),
    Name,
    Mode,
    Type,
    Mag,
    Reserve,
    PromptKey,
    PromptText,
}

#[derive(Component, Clone, Copy, PartialEq)]
enum Part {
    Chip(usize),
    Pip(usize),
    PipRow,
    Ammo,
    Inf,
    Prompt,
}

pub(super) fn plugin(app: &mut App) {
    app.add_systems(
        Update,
        (
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<WeaponBox>))),
            (touch_layout, update.run_if(resource_exists::<ActorData>)),
        )
            .in_set(MatchSystems),
    );
}

/// On a touch screen the fire button covers the bottom right corner: the weapon panel moves to
/// the bottom centre, and back when the player switches to mouse and keyboard.
fn touch_layout(
    touch: Option<Res<crate::game::TouchScreen>>,
    mut panel: Query<(&mut Node, &mut UiTransform), With<WeaponBox>>,
) {
    for (mut node, mut tf) in &mut panel {
        let centre = touch.is_some();
        if (node.right == Val::Auto) != centre {
            (node.right, node.left, node.bottom) = if centre {
                (Val::Auto, percent(50), px(8))
            } else {
                (px(28), Val::Auto, px(28))
            };
            tf.translation = Val2::new(if centre { percent(-50) } else { px(0) }, px(0));
        }
    }
}

fn txt(t: Txt, s: &str, font: TextFont, color: Color) -> impl Bundle {
    (t, Text::new(s), font, TextColor(color))
}

fn spawn(mut commands: Commands, root: Single<Entity, With<Root>>) {
    let tag = |t: Txt, font, bg, color| {
        (
            Node {
                padding: UiRect::axes(px(6), px(2)),
                border: UiRect::all(px(1)),
                ..default()
            },
            BackgroundColor(bg),
            BorderColor::all(if bg == Color::NONE {
                Color::srgba(1.0, 1.0, 1.0, 0.1)
            } else {
                Color::NONE
            }),
            children![(txt(t, "", font, color), ui::tracking(0.16, 9.0))],
        )
    };
    commands.entity(*root).with_children(|r| {
        r.spawn((
            WeaponBox,
            UiTransform::default(),
            Node {
                position_type: PositionType::Absolute,
                right: px(28),
                bottom: px(28),
                width: px(WIDTH),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::FlexEnd,
                row_gap: px(8),
                ..default()
            },
        ))
        .with_children(|w| {
            w.spawn(Node {
                column_gap: px(3),
                ..default()
            })
            .with_children(|row| {
                for i in 0..KEYS.len() {
                    row.spawn((
                        Part::Chip(i),
                        Node {
                            height: px(26),
                            padding: UiRect::new(px(12), px(14), px(0), px(0)),
                            column_gap: px(7),
                            align_items: AlignItems::Center,
                            ..default()
                        },
                        ui::shape(Cut::Slant(6), PLATE),
                    ))
                    .with_children(|c| {
                        c.spawn(txt(Txt::Key(i), "", ui::mono(9.0), TEXT));
                        c.spawn((
                            txt(Txt::Kind(i), "", ui::strong(10.0), TEXT),
                            ui::tracking(0.14, 10.0),
                        ));
                    });
                }
            });
            w.spawn((
                Node {
                    align_items: AlignItems::Stretch,
                    padding: UiRect::new(px(30), px(16), px(10), px(10)),
                    overflow: Overflow::clip(),
                    ..default()
                },
                children![back(PLATE, Edge::Left, 14)],
            ))
            .with_children(|p| {
                p.spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::FlexEnd,
                        row_gap: px(6),
                        padding: UiRect::right(px(18)),
                        margin: UiRect::right(px(16)),
                        border: UiRect::right(px(1)),
                        ..default()
                    },
                    BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.03)),
                ))
                .with_children(|l| {
                    l.spawn((
                        txt(Txt::Name, "", ui::display(22.0), INK),
                        ui::tracking(0.08, 22.0),
                    ));
                    l.spawn(Node {
                        column_gap: px(4),
                        ..default()
                    })
                    .with_children(|t| {
                        t.spawn(tag(Txt::Mode, ui::strong(9.0), ACCENT, ON_ACCENT));
                        t.spawn(tag(Txt::Type, ui::strong(9.0), Color::NONE, TEXT));
                    });
                });
                p.spawn((
                    Part::Ammo,
                    Node {
                        align_items: AlignItems::FlexEnd,
                        column_gap: px(6),
                        ..default()
                    },
                ))
                .with_children(|a| {
                    a.spawn((
                        txt(Txt::Mag, "", ui::display(58.0), Color::WHITE),
                        LineHeight::RelativeToFont(0.8),
                        ui::shadow(),
                    ));
                    a.spawn(txt(Txt::Reserve, "", ui::digits(16.0), ui::DIM));
                });
                p.spawn((
                    Part::Inf,
                    Node {
                        align_self: AlignSelf::Center,
                        ..default()
                    },
                    children![(Text::new("∞"), ui::strong(44.0), TextColor(INK))],
                ));
            });
            w.spawn((
                Part::PipRow,
                Node {
                    height: px(4),
                    column_gap: px(2),
                    ..default()
                },
            ))
            .with_children(|row| {
                for i in 0..PIPS {
                    row.spawn((
                        Part::Pip(i),
                        Node {
                            width: px(5),
                            height: percent(100),
                            ..default()
                        },
                        BackgroundColor(INK),
                    ));
                }
            });
            w.spawn((
                Part::Prompt,
                Node {
                    position_type: PositionType::Absolute,
                    right: px(0),
                    bottom: px(118),
                    padding: UiRect::axes(px(12), px(4)),
                    column_gap: px(8),
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(PLATE),
            ))
            .with_children(|p| {
                p.spawn((
                    Node {
                        padding: UiRect::axes(px(6), px(1)),
                        border: UiRect::all(px(1)),
                        ..default()
                    },
                    BorderColor::all(DANGER),
                    children![txt(Txt::PromptKey, "", ui::mono(11.0), DANGER)],
                ));
                p.spawn((
                    txt(Txt::PromptText, "", ui::strong(11.0), DANGER),
                    ui::tracking(0.24, 11.0),
                ));
            });
        });
    });
}

/// Slot / type name of a weapon kind.
pub(super) fn short(k: WeaponKind) -> &'static str {
    use WeaponKind::*;
    match k {
        Katana | DoubleKatana => "SWORD",
        Dagger => "DAGGER",
        Pistol | PistolX2 | Revolver | RevolverX2 => "PISTOL",
        Smg | SmgX2 => "SMG",
        Shotgun => "SHOTGUN",
        Rifle => "RIFLE",
        MachineGun => "MG",
        Rocket => "ROCKET",
        Frag => "FRAG",
        Flashbang => "FLASH",
        Smoke => "SMOKE",
        Stun => "STUN",
        Mine => "MINE",
        Medikit => "MEDKIT",
        Potion => "POTION",
        RepairKit => "REPAIR",
        SpyCase => "CASE",
    }
}

fn mode(k: WeaponKind) -> &'static str {
    use WeaponKind::*;
    match k {
        Katana | DoubleKatana | Dagger => "BLADE",
        Frag | Flashbang | Smoke | Stun | Mine => "THROW",
        Medikit | Potion | RepairKit | SpyCase => "USE",
        k if k.automatic() => "AUTO",
        _ => "SEMI",
    }
}

fn kind_of(item: Option<&Item>) -> Option<WeaponKind> {
    item.and_then(|i| i.weapon.as_ref()).map(|w| w.kind)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update(
    data: Res<ActorData>,
    profile: Option<Res<Profile>>,
    player: Query<(&Loadout, &Actor), With<Player>>,
    mut texts: Query<(&Txt, &mut Text, &mut TextColor)>,
    mut parts: Query<(
        &Part,
        &mut Node,
        Option<&mut ImageNode>,
        Option<&mut BackgroundColor>,
    )>,
) {
    let Ok((load, actor)) = player.single() else {
        return;
    };
    let controls = profile.as_deref().map_or(&*DEFAULTS, |p| &p.controls);
    let key = |a: Action| {
        controls.binds[a as usize]
            .iter()
            .flatten()
            .next()
            .map_or_else(String::new, |&b| label(b))
    };
    let slot = &load.slots[load.current];
    let item = data.items.get(slot.item);
    let kind = kind_of(item);
    let cap = item
        .and_then(|i| i.weapon.as_ref())
        .map_or(0, |w| w.magazine);
    let gun = cap > 0 && item.is_some_and(|i| i.kind != "melee");
    let mag = slot.magazine;
    let low = gun && mag <= cap.div_ceil(4);
    let reloading = actor.reloading();
    let can_reload = gun && slot.reserve > 0 && mag < cap;
    let prompt = gun && (reloading || (low && (can_reload || mag == 0)));
    let (prompt_text, prompt_color) = if reloading {
        ("RELOADING", GOLD)
    } else if mag == 0 && slot.reserve == 0 {
        ("NO AMMO", DANGER)
    } else if mag == 0 {
        ("NO AMMO · RELOAD", DANGER)
    } else {
        ("LOW AMMO · RELOAD", DANGER)
    };
    for (t, mut text, mut color) in &mut texts {
        let (s, c) = match *t {
            Txt::Key(i) | Txt::Kind(i) => {
                let on = i == load.current;
                let c = if on { ON_ACCENT } else { TEXT };
                let Some(s) = load.slots.get(i) else {
                    continue;
                };
                if matches!(*t, Txt::Key(_)) {
                    (key(KEYS[i]), c.with_alpha(0.7))
                } else {
                    let k = kind_of(data.items.get(s.item));
                    (k.map_or("ITEM", short).to_owned(), c)
                }
            }
            Txt::Name => (
                item.and_then(|i| i.name.clone())
                    .map_or_else(|| format!("ITEM {}", slot.item), |n| n.to_uppercase()),
                color.0,
            ),
            Txt::Mode => (kind.map_or("BLADE", mode).to_owned(), color.0),
            Txt::Type => (kind.map_or("ITEM", short).to_owned(), color.0),
            Txt::Mag => (
                mag.to_string(),
                if mag == 0 {
                    DANGER
                } else if low {
                    GOLD
                } else {
                    Color::WHITE
                },
            ),
            Txt::Reserve => (slot.reserve.to_string(), color.0),
            Txt::PromptKey => (key(Action::Reload), prompt_color),
            Txt::PromptText => (prompt_text.to_owned(), prompt_color),
        };
        set(&mut text, s);
        if color.0 != c {
            color.0 = c;
        }
    }
    let spent = (cap.saturating_sub(mag)) as usize;
    let shown = (cap as usize).min(PIPS);
    // keep the row inside WIDTH for big magazines: pip + gap = 7 px
    let scale = (WIDTH / (7.0 * shown.max(1) as f32)).min(1.0);
    let flex = |on: bool| if on { Display::Flex } else { Display::None };
    for (p, mut n, img, bg) in &mut parts {
        let want = match *p {
            Part::Chip(i) => {
                if let (Some(mut img), true) = (img, i < load.slots.len()) {
                    let c = if i == load.current { ACCENT } else { PLATE };
                    if img.color != c {
                        img.color = c;
                    }
                }
                flex(i < load.slots.len())
            }
            Part::Pip(i) => {
                if let Some(mut bg) = bg {
                    let c = if i < spent {
                        Color::srgba(1.0, 1.0, 1.0, 0.03)
                    } else {
                        INK
                    };
                    if bg.0 != c {
                        bg.0 = c;
                    }
                }
                let w = px(5.0 * scale);
                if n.width != w {
                    n.width = w;
                }
                flex(gun && i < shown)
            }
            Part::PipRow => {
                let g = px(2.0 * scale);
                if n.column_gap != g {
                    n.column_gap = g;
                }
                flex(gun)
            }
            Part::Ammo => flex(gun),
            Part::Inf => flex(!gun),
            Part::Prompt => flex(prompt),
        };
        if n.display != want {
            n.display = want;
        }
    }
}
