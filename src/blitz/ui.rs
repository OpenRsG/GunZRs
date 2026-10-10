//! Blitzkrieg's three screens: the class select at the start (`CLASS_SELECT_TIME`), the minimap
//! and the reward panel after the match. The data has no minimap or class art (`interface/` has
//! only the map banner `map_blitzkrieg.bmp`, the empty `blitzkrieginterface.xml` and the unused
//! `blitzinfo_panel.tga`, a stats panel with an empty icon frame), so all three are drawn here:
//! the minimap from the map's own floor polygons, the class cards with the item icons of their
//! weapons.

use super::*;
use crate::map::Map;
use bevy::{
    asset::RenderAssetUsages,
    render::render_resource::{Extent3d, TextureDimension, TextureFormat},
};

/// Longest side of the floor plan image (pixels).
const PLAN_PX: f32 = 256.0;
/// Width of the minimap on screen (pixels) and its dot sizes.
const MINI_W: f32 = 260.0;
/// Alpha of floor and of the empty background in the plan.
const FLOOR_ALPHA: u8 = 235;
const EMPTY_ALPHA: u8 = 150;

/// The map seen from above: upward-facing polygons (`normal.y > 0.5`), the highest one per
/// pixel, shaded by height on a dark ground. Returns the image and the map area (Bevy x, z) it
/// covers. The x axis is mirrored so the Red base (+x) is on the left.
pub(super) fn floor_plan(map: &Map) -> (Image, (Vec2, Vec2)) {
    let world = |p: [f32; 3]| Vec3::from(to_bevy(p)) * SCALE;
    let (mut lo, mut hi) = (Vec2::MAX, Vec2::MIN);
    for v in &map.vertices {
        let p = world(v.pos);
        lo = lo.min(Vec2::new(p.x, p.z));
        hi = hi.max(Vec2::new(p.x, p.z));
    }
    let size = hi - lo;
    let k = PLAN_PX / size.max_element();
    let (w, h) = (
        (size.x * k).ceil() as usize + 1,
        (size.y * k).ceil() as usize + 1,
    );
    let mut top = vec![f32::MIN; w * h];
    let px = |p: Vec3| Vec2::new((hi.x - p.x) * k, (p.z - lo.y) * k);
    for poly in &map.polygons {
        let first = poly.first as usize;
        let Some(vs) = map.vertices.get(first..first + poly.count as usize) else {
            continue;
        };
        if vs.len() < 3 || Vec3::from(to_bevy(vs[0].normal)).y <= 0.5 {
            continue;
        }
        for i in 1..vs.len() - 1 {
            let t = [vs[0], vs[i], vs[i + 1]].map(|v| world(v.pos));
            let (a, b, c) = (px(t[0]), px(t[1]), px(t[2]));
            let area = (b - a).perp_dot(c - a);
            if area.abs() < 1.0e-4 {
                continue;
            }
            let (min, max) = (a.min(b).min(c), a.max(b).max(c));
            for y in (min.y.floor().max(0.0) as usize)..=(max.y.ceil() as usize).min(h - 1) {
                for x in (min.x.floor().max(0.0) as usize)..=(max.x.ceil() as usize).min(w - 1) {
                    let p = Vec2::new(x as f32 + 0.5, y as f32 + 0.5);
                    let (u, v) = (
                        (p - a).perp_dot(c - a) / area,
                        (b - a).perp_dot(p - a) / area,
                    );
                    if u < 0.0 || v < 0.0 || u + v > 1.0 {
                        continue;
                    }
                    let height = t[0].y + u * (t[1].y - t[0].y) + v * (t[2].y - t[0].y);
                    top[y * w + x] = top[y * w + x].max(height);
                }
            }
        }
    }
    let (lowest, highest) = top
        .iter()
        .filter(|h| **h > f32::MIN)
        .fold((f32::MAX, f32::MIN), |(l, u), h| (l.min(*h), u.max(*h)));
    let data = top
        .iter()
        .flat_map(|&height| {
            if height == f32::MIN {
                return [10, 14, 18, EMPTY_ALPHA];
            }
            let t = ((height - lowest) / (highest - lowest).max(1.0)).clamp(0.0, 1.0);
            let shade = |a: f32, b: f32| (a + (b - a) * t) as u8;
            [
                shade(55.0, 190.0),
                shade(90.0, 205.0),
                shade(100.0, 190.0),
                FLOOR_ALPHA,
            ]
        })
        .collect();
    let image = Image::new(
        Extent3d {
            width: w as u32,
            height: h as u32,
            depth_or_array_layers: 1,
        },
        TextureDimension::D2,
        data,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    (image, (lo, hi))
}

fn root(camera: Entity, z: i32) -> impl Bundle {
    (
        UiTargetCamera(camera),
        GlobalZIndex(z),
        Node {
            position_type: PositionType::Absolute,
            width: percent(100),
            height: percent(100),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            row_gap: px(18),
            ..default()
        },
    )
}

fn text(s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (
        Text::new(s),
        TextFont::from_font_size(size),
        TextColor(color),
        TextShadow::default(),
    )
}

// ---------------------------------------------------------------------------------------------
// Class select

#[derive(Component)]
pub(super) struct SelectRoot;

#[derive(Component)]
pub(super) struct Card(usize);

#[derive(Component)]
pub(super) struct Countdown;

/// What a class does, from its `CLASS_TABLE` row.
fn describe(cfg: &Cfg, class: usize) -> String {
    let v = |a: &str| cfg.class_val(Some(class), a);
    match class {
        0 => format!(
            "+{} melee attack power\n+{} AP and HP",
            v("enhanceMeleeDPS"),
            v("addMaxApHp")
        ),
        1 => format!(
            "+{} shotgun magazines\n+{:.0}% shotgun damage",
            v("addShotgunMagazine"),
            v("enhanceShotgunDamage") * 100.0
        ),
        2 => format!(
            "{} fire damage per second\nfor {} s on every hit\n-{} attack power",
            v("enchantFireDamage"),
            v("fireDamageDuration"),
            v("reduceDPS")
        ),
        3 => format!(
            "Allies within {} m take\n{:.0}% less damage",
            v("distance") * 0.01,
            v("reduceDamageRatioForMyTeam") * 100.0
        ),
        4 => format!("+{:.0}% damage", v("enhanceDamageRatio") * 100.0),
        5 => format!(
            "+{:.0}% damage to buildings",
            v("enhanceDamageRatioAtBuilding") * 100.0
        ),
        6 => format!(
            "+{:.0}% honor from\nevery gain",
            v("aquirHonorRatio") * 100.0
        ),
        7 => format!(
            "{} fire damage per second\nfor {} s on every hit\n+{:.0}% bullets",
            v("enchantFireDamage"),
            v("fireDamageDuration"),
            v("addMagazineRatio") * 100.0
        ),
        _ => format!(
            "Radar heals {:.0}% AP/HP\nand {:.0}% ammo; barricade\ncuts damage {:.0}%",
            v("recoveryApHpRatio") * 100.0,
            v("recoveryMagazineRatio") * 100.0,
            v("reduceDamageRatio") * 100.0
        ),
    }
}

/// The class screen: nine cards (click or press 1-9 / the arrows, Enter confirms) under the
/// countdown of message 2100.
pub(super) fn select_ui(
    mut commands: Commands,
    mut blitz: ResMut<Blitz>,
    data: Res<ActorData>,
    camera: Query<Entity, With<Camera3d>>,
    open: Query<Entity, With<SelectRoot>>,
    mut cards: Query<(&Card, &Interaction, &mut BorderColor, &mut BackgroundColor)>,
    mut count: Query<&mut Text, With<Countdown>>,
) {
    let Some(sel) = &blitz.select else {
        for e in &open {
            commands.entity(e).despawn();
        }
        return;
    };
    if open.is_empty() {
        let Ok(camera) = camera.single() else { return };
        let name = |id: u32| {
            data.items
                .get(id)
                .and_then(|i| i.name.clone())
                .unwrap_or_else(|| "-".into())
        };
        commands
            .spawn((
                SelectRoot,
                root(camera, 6),
                BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.7)),
            ))
            .with_children(|r| {
                r.spawn(text(
                    "BLITZKRIEG: SELECT YOUR CLASS",
                    40.0,
                    Color::srgb(1.0, 0.85, 0.3),
                ));
                r.spawn((Countdown, text("", 24.0, Color::WHITE)));
                r.spawn(Node {
                    column_gap: px(6),
                    row_gap: px(6),
                    max_width: percent(98),
                    flex_wrap: FlexWrap::Wrap,
                    justify_content: JustifyContent::Center,
                    ..default()
                })
                .with_children(|row| {
                    for (i, (title, _, book)) in CLASSES.iter().enumerate() {
                        row.spawn((
                            Card(i),
                            Button,
                            Node {
                                width: px(130),
                                height: px(330),
                                padding: UiRect::all(px(8)),
                                border: UiRect::all(px(3)),
                                flex_direction: FlexDirection::Column,
                                row_gap: px(8),
                                ..default()
                            },
                            BorderColor::all(Color::srgb(0.4, 0.4, 0.4)),
                            BackgroundColor(Color::srgba(0.1, 0.12, 0.14, 0.9)),
                        ))
                        .with_children(|c| {
                            c.spawn(text(
                                format!("{} {}", i + 1, title.replace("Combat ", "Combat\n")),
                                16.0,
                                Color::srgb(1.0, 0.9, 0.5),
                            ));
                            c.spawn(Node {
                                column_gap: px(6),
                                ..default()
                            })
                            .with_children(|icons| {
                                for icon in &blitz.art[i] {
                                    icons.spawn((
                                        Node {
                                            width: px(48),
                                            height: px(48),
                                            ..default()
                                        },
                                        icon.clone(),
                                    ));
                                }
                            });
                            c.spawn(text(describe(&blitz.cfg, i), 13.0, Color::WHITE));
                            let kit = blitz.kit[i];
                            c.spawn(text(
                                format!("{}\n{}", name(kit[0]), name(kit[1])),
                                12.0,
                                Color::srgb(0.65, 0.8, 0.95),
                            ));
                            if book.is_none() {
                                c.spawn(text("no class book", 12.0, Color::srgb(0.6, 0.6, 0.6)));
                            }
                        });
                    }
                });
                r.spawn(text(
                    "1-9 / arrow keys / click to choose, Enter or Space to confirm",
                    20.0,
                    Color::srgb(0.8, 0.8, 0.8),
                ));
            });
        return;
    }
    let (chosen, left) = (sel.sel, sel.left);
    for (card, hover, mut border, mut back) in &mut cards {
        if *hover == Interaction::Pressed {
            if let Some(s) = &mut blitz.select {
                s.sel = card.0;
                s.done = true;
            }
        }
        let on = card.0 == chosen || *hover == Interaction::Hovered;
        *border = BorderColor::all(if card.0 == chosen {
            Color::srgb(1.0, 0.85, 0.3)
        } else if on {
            Color::srgb(0.8, 0.8, 0.8)
        } else {
            Color::srgb(0.4, 0.4, 0.4)
        });
        back.0 = Color::srgba(0.1, 0.12, 0.14, if on { 1.0 } else { 0.9 });
    }
    for mut t in &mut count {
        t.0 = format!(
            "Please select your class in {} second(s).",
            left.ceil().max(0.0)
        );
    }
}

// ---------------------------------------------------------------------------------------------
// Minimap

#[derive(Component)]
pub(super) struct MiniRoot;

#[derive(Component)]
pub(super) struct Dot;

/// The minimap (top right, `M` hides it): the floor plan with a dot per player and bot (the
/// player's brighter and larger), soldier, barricade (square) and radar (large); allies green,
/// enemies red, buildings tinted.
#[allow(clippy::too_many_arguments)]
pub(super) fn minimap(
    mut commands: Commands,
    blitz: Res<Blitz>,
    keys: Res<ButtonInput<KeyCode>>,
    camera: Query<Entity, With<Camera3d>>,
    root: Query<(Entity, Option<&Children>), With<MiniRoot>>,
    mut visible: Query<&mut Visibility, With<MiniRoot>>,
    mut dots: Query<(&mut Node, &mut BackgroundColor), With<Dot>>,
    actors: Query<(&Transform, &Team, Has<Player>), (With<Honor>, Without<Dead>)>,
    npcs: Query<(&GlobalTransform, &Npc, &Team), Without<Dead>>,
) {
    let (lo, hi) = blitz.bounds;
    let area = hi - lo;
    let size = Vec2::new(MINI_W, MINI_W * area.y / area.x);
    let Ok((root, kids)) = root.single() else {
        let Ok(camera) = camera.single() else { return };
        commands.spawn((
            MiniRoot,
            UiTargetCamera(camera),
            GlobalZIndex(3),
            Node {
                position_type: PositionType::Absolute,
                right: px(16),
                bottom: px(120),
                width: px(size.x),
                height: px(size.y),
                border: UiRect::all(px(2)),
                ..default()
            },
            BorderColor::all(Color::srgba(1.0, 1.0, 1.0, 0.5)),
            ImageNode {
                image_mode: NodeImageMode::Stretch,
                ..ImageNode::new(blitz.plan.clone())
            },
        ));
        return;
    };
    if keys.just_pressed(KeyCode::KeyM)
        && let Ok(mut v) = visible.single_mut()
    {
        *v = if *v == Visibility::Hidden {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
    }
    let Some(mine) = actors.iter().find(|a| a.2).map(|a| *a.1) else {
        return;
    };
    let at = |p: Vec3| Vec2::new((hi.x - p.x) / area.x, (p.z - lo.y) / area.y) * size;
    let tint = |team: Team, ally: Color, foe: Color| if team == mine { ally } else { foe };
    // (position on the map image, colour, size, round)
    let mut marks: Vec<(Vec2, Color, f32, bool)> = Vec::new();
    for (g, n, t) in &npcs {
        let (colour, d, round) = match n.kind.as_str() {
            "radar" => (
                tint(*t, Color::srgb(0.3, 0.8, 1.0), Color::srgb(1.0, 0.6, 0.2)),
                14.0,
                false,
            ),
            "barricade" => (
                tint(*t, Color::srgb(0.3, 0.8, 1.0), Color::srgb(1.0, 0.6, 0.2)),
                8.0,
                false,
            ),
            k if SOLDIERS.contains(&k) => (
                tint(*t, Color::srgb(0.3, 1.0, 0.4), Color::srgb(1.0, 0.3, 0.3)),
                4.0,
                true,
            ),
            _ => continue,
        };
        marks.push((at(g.translation()), colour, d, round));
    }
    for (tf, t, player) in &actors {
        let (colour, d) = if player {
            (Color::WHITE, 9.0)
        } else {
            (
                tint(*t, Color::srgb(0.3, 1.0, 0.4), Color::srgb(1.0, 0.3, 0.3)),
                6.0,
            )
        };
        marks.push((at(tf.translation), colour, d, true));
    }
    // One pooled node per mark (every child of the root is one); spare ones are hidden.
    let mut pool = kids.map(|k| k.to_vec()).unwrap_or_default().into_iter();
    for (pos, colour, d, round) in marks {
        let Some(e) = pool.next() else {
            commands.entity(root).with_child((
                Dot,
                Node {
                    position_type: PositionType::Absolute,
                    ..default()
                },
                BackgroundColor(colour),
            ));
            continue;
        };
        if let Ok((mut node, mut back)) = dots.get_mut(e) {
            node.display = Display::Flex;
            node.width = px(d);
            node.height = px(d);
            node.left = px(pos.x - d / 2.0);
            node.top = px(pos.y - d / 2.0);
            node.border_radius = if round {
                BorderRadius::all(px(d))
            } else {
                BorderRadius::ZERO
            };
            back.0 = colour;
        }
    }
    for e in pool {
        if let Ok((mut node, _)) = dots.get_mut(e) {
            node.display = Display::None;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Reward

#[derive(Component)]
pub(super) struct RewardRoot;

/// The reward panel of the finished match (left of the scoreboard): result, MVP bonus, XP,
/// bounty and medals, or why nothing was paid.
pub(super) fn reward_ui(
    mut commands: Commands,
    blitz: Res<Blitz>,
    camera: Query<Entity, With<Camera3d>>,
    open: Query<(), With<RewardRoot>>,
) {
    let Some(p) = &blitz.payout else { return };
    if !open.is_empty() {
        return;
    }
    let Ok(camera) = camera.single() else { return };
    let r = &blitz.cfg.reward;
    let share = if p.won { r.mvp_win } else { r.mvp_lose };
    let mut lines = vec![
        ("REWARD".to_string(), 30.0, Color::srgb(1.0, 0.85, 0.3)),
        (
            format!(
                "{} after {} min",
                if p.won { "Victory" } else { "No victory" },
                p.minutes
            ),
            20.0,
            Color::WHITE,
        ),
    ];
    match &p.none {
        Some(why) => lines.push((
            format!("No reward: {why}"),
            20.0,
            Color::srgb(0.9, 0.5, 0.4),
        )),
        None => {
            if p.mvp {
                lines.push((
                    format!("MVP: +{:.0}% XP/BP/Medal", share[0] * 100.0),
                    20.0,
                    Color::srgb(0.6, 1.0, 0.6),
                ));
            }
            lines.push((format!("+{} XP", p.xp), 24.0, Color::WHITE));
            lines.push((format!("+{} bounty", p.bounty), 24.0, Color::WHITE));
            lines.push((
                format!("+{} medals", p.medals),
                24.0,
                Color::srgb(0.7, 0.85, 1.0),
            ));
        }
    }
    commands
        .spawn((
            RewardRoot,
            UiTargetCamera(camera),
            GlobalZIndex(5),
            Node {
                position_type: PositionType::Absolute,
                left: px(12),
                top: px(110),
                padding: UiRect::all(px(12)),
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.75)),
        ))
        .with_children(|c| {
            for (s, size, colour) in lines {
                c.spawn(text(s, size, colour));
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::map::{Polygon, Vertex};

    /// A flat 20 m square seen from above fills the plan and leaves nothing outside it.
    #[test]
    fn plan_fills_the_floor() {
        let vertex = |x: f32, y: f32| Vertex {
            pos: [x, y, 0.0],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
            lm_uv: [0.0; 2],
        };
        let map = Map {
            rs: String::new(),
            dir: String::new(),
            materials: Vec::new(),
            vertices: vec![
                vertex(0.0, 0.0),
                vertex(2000.0, 0.0),
                vertex(2000.0, 2000.0),
                vertex(0.0, 2000.0),
                // a wall: not drawn, but widens the bounds to 40 m along x
                Vertex {
                    normal: [1.0, 0.0, 0.0],
                    ..vertex(4000.0, 0.0)
                },
                Vertex {
                    normal: [1.0, 0.0, 0.0],
                    ..vertex(4000.0, 100.0)
                },
                Vertex {
                    normal: [1.0, 0.0, 0.0],
                    ..vertex(4000.0, 200.0)
                },
            ],
            polygons: vec![
                Polygon {
                    material: 0,
                    flags: 0,
                    first: 0,
                    count: 4,
                    lightmap: 0,
                },
                Polygon {
                    material: 0,
                    flags: 0,
                    first: 4,
                    count: 3,
                    lightmap: 0,
                },
            ],
            lightmaps: Vec::new(),
            dummies: Vec::new(),
            objects: Vec::new(),
        };
        let (image, (lo, hi)) = floor_plan(&map);
        let data = image.data.as_ref().unwrap();
        let floor = data.chunks(4).filter(|p| p[3] == FLOOR_ALPHA).count();
        let all = data.len() / 4;
        // The floor is half of the width of the plan.
        assert!((hi.x - lo.x - 40.0).abs() < 0.01 || (hi.y - lo.y - 40.0).abs() < 0.01);
        assert!(
            (floor as f32 / all as f32 - 0.5).abs() < 0.1,
            "{floor} of {all}"
        );
    }
}
