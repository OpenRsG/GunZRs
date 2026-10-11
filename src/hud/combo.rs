//! Arcade combo and killing-spree layer for the player's own play (**not retail**; every number
//! is **inferred**, `docs/formats.md`, "Combo and killing spree"). Each hit the player lands
//! (`Wounded`: after friendly-fire, protection and overkill) builds a combo that breaks
//! [`WINDOW`] seconds after the last hit or when the player dies; its multiplier ([`mult`])
//! scales the points of hits and kills. Kills in quick succession are a multikill and kills
//! without dying a spree ([`multikill`], [`spree`]), announced by a banner with a screen-edge
//! flash. Sounds are synthesized (`synth.rs`) and gated by `Settings::kill_sounds`, the meter and
//! banners by `Settings::combo`. A child of `hud.rs`.

mod synth;

use super::{Root, set};
use crate::{
    actor::ActorData,
    audio::{Expires, SOUND_LIFE},
    combat::{Wounded, is_melee},
    game::{Killed, Player, Settings},
    ui::{self, ACCENT, DANGER, DIM, INK, LINE, ON_ACCENT},
    view::Shot,
};
use bevy::{ecs::component::Mutable, prelude::*};

pub(super) fn plugin(app: &mut App) {
    app.init_resource::<Combo>().add_systems(
        Update,
        (
            load.run_if(
                resource_exists::<Assets<AudioSource>>.and_then(not(resource_exists::<Chimes>)),
            ),
            spawn.run_if(any_with_component::<Root>.and_then(not(any_with_component::<Meter>))),
            (
                track.run_if(resource_exists::<ActorData>),
                animate.run_if(any_with_component::<Meter>.and_then(resource_exists::<Parts>)),
            )
                .chain(),
        ),
    );
}

/// Seconds after the last hit the combo breaks.
const WINDOW: f32 = 2.5;
/// Seconds between kills that still chain into a multikill.
const CHAIN: f32 = 4.0;
/// Hits at which the multiplier steps up to x2, x3, ... x8.
const MULT_AT: [u32; 7] = [5, 10, 20, 35, 50, 70, 100];
/// Points of a kill, extra for a headshot and for a melee kill, and per banner tier (x tier + 1).
const KILL: u32 = 100;
const HEAD: u32 = 50;
const MELEE: u32 = 50;
const BONUS: u32 = 250;
/// Semitones the hit tick climbs by hit number (then holds): a major pentatonic run.
const TICK_RUN: [f32; 8] = [0.0, 2.0, 4.0, 7.0, 9.0, 12.0, 14.0, 16.0];

/// Multiplier of a combo of `hits` hits.
fn mult(hits: u32) -> u32 {
    1 + MULT_AT.iter().filter(|&&at| hits >= at).count() as u32
}

/// The multikill of `chain` kills in a row: headline and tier (0..=4).
fn multikill(chain: u32) -> Option<(&'static str, usize)> {
    const NAMES: [&str; 5] = [
        "DOUBLE KILL",
        "TRIPLE KILL",
        "QUAD KILL",
        "MULTI KILL",
        "MONSTER KILL",
    ];
    (chain >= 2).then(|| {
        let tier = (chain as usize - 2).min(4);
        (NAMES[tier], tier)
    })
}

/// The killing spree announced at `kills` kills without dying: exactly at each step, and from
/// 15 on every fifth kill.
fn spree(kills: u32) -> Option<(&'static str, usize)> {
    const STEPS: [(u32, &str); 5] = [
        (3, "KILLING SPREE"),
        (5, "RAMPAGE"),
        (7, "DOMINATING"),
        (10, "UNSTOPPABLE"),
        (15, "GODLIKE"),
    ];
    let tier = STEPS.iter().rposition(|&(at, _)| kills >= at)?;
    (kills == STEPS[tier].0 || tier == 4 && kills.is_multiple_of(5)).then(|| (STEPS[tier].1, tier))
}

/// A multikill or spree announcement: headline, tier (size and colour), the line under it.
struct Banner {
    at: f32,
    title: &'static str,
    tier: usize,
    sub: String,
}

#[derive(Resource, Default)]
struct Combo {
    hits: u32,
    score: u32,
    /// When the latest hit landed.
    last: f32,
    /// Kills in the current multikill chain, and when the latest kill happened.
    chain: u32,
    last_kill: f32,
    /// Kills this life.
    spree: u32,
    /// When the latest hit landed and the multiplier last stepped up (animation clocks).
    hit_at: f32,
    step_at: f32,
    /// The score the meter shows, rolling up to `score`.
    shown: f32,
    /// Points waiting for a popup: (points, from a kill).
    pending: Vec<(u32, bool)>,
    banner: Option<Banner>,
}

impl Combo {
    /// One landed hit of `damage` points (HP and AP taken); true if the multiplier stepped up.
    fn hit(&mut self, now: f32, damage: f32) -> bool {
        let before = mult(self.hits);
        self.hits += 1;
        self.last = now;
        self.hit_at = now;
        self.score += damage.round().max(1.0) as u32 * mult(self.hits);
        mult(self.hits) > before
    }

    /// The player's kill: its points (with a multikill and spree bonus) and the announcement.
    fn kill(&mut self, now: f32, head: bool, melee: bool) -> (u32, Option<Banner>) {
        self.chain = if self.chain > 0 && now - self.last_kill <= CHAIN {
            self.chain + 1
        } else {
            1
        };
        self.last_kill = now;
        self.spree += 1;
        let (multi, run) = (multikill(self.chain), spree(self.spree));
        let m = mult(self.hits);
        let bonus: u32 = [multi, run]
            .iter()
            .flatten()
            .map(|e| BONUS * (e.1 as u32 + 1))
            .sum();
        let points =
            (KILL + if head { HEAD } else { 0 } + if melee { MELEE } else { 0 } + bonus) * m;
        self.score += points;
        let banner = match (multi, run) {
            (Some(a), Some(b)) => Some(if b.1 >= a.1 {
                (b, Some(a.0))
            } else {
                (a, Some(b.0))
            }),
            (Some(a), None) | (None, Some(a)) => Some((a, None)),
            _ => None,
        }
        .map(|((title, tier), other)| Banner {
            at: now,
            title,
            tier,
            sub: match other {
                Some(o) => format!("{o}  +{}", bonus * m),
                None if run.is_some() => format!("{} KILLS  +{}", self.spree, bonus * m),
                None => format!("+{}", bonus * m),
            },
        });
        (points, banner)
    }

    /// `(hits, score)` of the combo if it ran out by `now`.
    fn expire(&mut self, now: f32) -> Option<(u32, u32)> {
        let over = self.hits > 0 && now - self.last > WINDOW;
        over.then(|| self.end())
    }

    /// Ends the combo; its `(hits, score)`.
    fn end(&mut self) -> (u32, u32) {
        let out = (self.hits, self.score);
        (self.hits, self.score) = (0, 0);
        out
    }

    /// The player died: the combo, chain and spree end with it.
    fn die(&mut self) {
        self.end();
        self.chain = 0;
        self.spree = 0;
        self.banner = None;
    }
}

// ---------------------------------------------------------------------------------------------
// Sound

/// The synthesized clips, in `synth::NAMES` order.
#[derive(Resource)]
struct Chimes(Vec<Handle<AudioSource>>);

impl Chimes {
    /// Plays clip `i` 2D at `speed` (pitch and tempo); headless `--shot` runs only log it.
    fn play(&self, commands: &mut Commands, now: f32, shot: bool, i: usize, speed: f32) {
        if shot {
            info!("sfx combo {} x{speed:.2}", synth::NAMES[i]);
            return;
        }
        commands.spawn((
            AudioPlayer::new(self.0[i].clone()),
            PlaybackSettings::DESPAWN.with_speed(speed),
            Expires(now + SOUND_LIFE),
        ));
    }
}

fn load(mut commands: Commands, mut assets: ResMut<Assets<AudioSource>>) {
    let clips = synth::clips();
    commands.insert_resource(Chimes(
        clips
            .into_iter()
            .map(|bytes| {
                assets.add(AudioSource {
                    bytes: bytes.into(),
                })
            })
            .collect(),
    ));
}

// ---------------------------------------------------------------------------------------------
// Rules

/// Folds this frame's hits, kills and deaths of the player into the combo; plays the sounds.
#[allow(clippy::too_many_arguments)]
fn track(
    mut commands: Commands,
    time: Res<Time>,
    settings: Res<Settings>,
    data: Res<ActorData>,
    shot: Option<Res<Shot>>,
    chimes: Option<Res<Chimes>>,
    parts: Option<Res<Parts>>,
    mut combo: ResMut<Combo>,
    mut wounded: MessageReader<Wounded>,
    mut killed: MessageReader<Killed>,
    players: Query<(), With<Player>>,
) {
    let now = time.elapsed_secs();
    if let Some((hits, score)) = combo.expire(now) {
        info!("t={now:.2} combo: ended hits={hits} score={score}");
    }
    let (mut landed, mut gained, mut stepped) = (0, 0, false);
    for w in wounded.read() {
        let damage = w.hp + w.ap;
        if damage <= 0.0 || !players.contains(w.attacker) || players.contains(w.target) {
            continue;
        }
        let before = combo.score;
        stepped |= combo.hit(now, damage);
        landed += 1;
        gained += combo.score - before;
    }
    if landed > 0 {
        let (hits, m) = (combo.hits, mult(combo.hits));
        info!(
            "t={now:.2} combo: hits={hits} mult=x{m} score={} (+{gained})",
            combo.score
        );
        if stepped {
            combo.step_at = now;
        }
        if parts.is_some() {
            combo.pending.push((gained, false));
        }
    }
    // (killed by the player, headshot, banner tier)
    let (mut kills, mut head, mut tier) = (false, false, None);
    for k in killed.read() {
        if players.contains(k.victim) {
            if combo.hits > 0 || combo.spree > 0 {
                info!(
                    "t={now:.2} combo: player died hits={} spree={}",
                    combo.hits, combo.spree
                );
            }
            combo.die();
        } else if players.contains(k.killer) {
            let melee = data
                .items
                .get(k.item)
                .and_then(|i| i.weapon.as_ref())
                .is_some_and(|w| is_melee(w.kind));
            let (points, banner) = combo.kill(now, k.head, melee);
            info!(
                "t={now:.2} combo: kill +{points} chain={} spree={} score={}",
                combo.chain, combo.spree, combo.score
            );
            (kills, head) = (true, head | k.head);
            if parts.is_some() {
                combo.pending.push((points, true));
            }
            if let Some(b) = banner {
                info!("t={now:.2} spree: {} (tier {}) {}", b.title, b.tier, b.sub);
                tier = tier.max(Some(b.tier));
                combo.banner = Some(b);
            }
        }
    }
    let Some(chimes) = chimes.filter(|_| settings.kill_sounds) else {
        return;
    };
    let mut play = |i, speed| chimes.play(&mut commands, now, shot.is_some(), i, speed);
    if landed > 0 && !kills {
        play(
            synth::TICK,
            (TICK_RUN[(combo.hits as usize - 1).min(TICK_RUN.len() - 1)] / 12.0).exp2(),
        );
    }
    if stepped {
        play(synth::STEP, 1.0 + 0.05 * (mult(combo.hits) - 2) as f32);
    }
    if kills {
        play(synth::KILL, 1.0);
        if head {
            play(synth::PING, 1.0);
        }
    }
    if let Some(t) = tier {
        play(synth::STING + t, 1.0);
    }
}

// ---------------------------------------------------------------------------------------------
// HUD

/// Layout: the meter sits left of the centre line at mid height, clear of the corner counters,
/// the feed on the right and the vitals; the banner is upper centre, between the clock and the
/// kill confirmation.
const METER_W: f32 = 200.0;
/// Popups beside the meter: pool size, rise (px) and life (s).
const POPS: usize = 5;
const POP_RISE: f32 = 34.0;
const POP_LIFE: f32 = 0.8;
/// Points landing within this long of a popup's start add into it. Hit popups start 70 px and
/// kill popups 0 px down the meter, so the two kinds never meet.
const POP_MERGE: f32 = 0.4;
const POP_HIT_TOP: f32 = 70.0;
const POP_KILL_TOP: f32 = 0.0;

/// A live "+N" popup: when it started and last grew, its kind and the points it shows.
struct Pop {
    born: f32,
    punch: f32,
    kill: bool,
    points: u32,
}
/// Count punch and multiplier pop: extra scale and seconds.
const PUNCH: (f32, f32) = (0.3, 0.12);
const BADGE: (f32, f32) = (0.6, 0.3);
/// Banner: seconds on screen, the fade-out at its end, shake, and the edge flash.
const BANNER_LIFE: f32 = 2.0;
const BANNER_OUT: f32 = 0.4;
const SHAKE_SECS: f32 = 0.4;
const FLASH_SECS: f32 = 0.45;
/// Banner headline size by tier.
const TITLE: [f32; 5] = [44.0, 52.0, 60.0, 70.0, 82.0];

#[derive(Component)]
struct Meter;

/// The layer's UI nodes.
#[derive(Resource)]
struct Parts {
    meter: Entity,
    count: Entity,
    badge: Entity,
    badge_text: Entity,
    fill: Entity,
    score: Entity,
    pops: [Entity; POPS],
    banner: Entity,
    card: Entity,
    title: Entity,
    sub: Entity,
    flash: Entity,
}

/// INK at 0, amber at 0.5, danger red at 1: how hot a multiplier or tier burns.
fn heat(t: f32) -> Color {
    let mix =
        |a: Color, b: Color, t: f32| Color::from(LinearRgba::from(a).mix(&LinearRgba::from(b), t));
    let t = t.clamp(0.0, 1.0);
    if t < 0.5 {
        mix(INK, ACCENT, t * 2.0)
    } else {
        mix(ACCENT, DANGER, t * 2.0 - 1.0)
    }
}

/// Transparent screen centre fading to `color` at the edges.
fn edge(color: Color) -> BackgroundGradient {
    RadialGradient::new(
        UiPosition::CENTER,
        RadialGradientShape::default(),
        vec![
            ColorStop::new(color.with_alpha(0.0), percent(50)),
            ColorStop::new(color, percent(100)),
        ],
    )
    .into()
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn spawn(mut commands: Commands, root: Single<Entity, With<Root>>) {
    let at = |left: f32, top: Val| Node {
        position_type: PositionType::Absolute,
        left: px(left),
        top,
        ..default()
    };
    let meter = commands
        .spawn((
            Meter,
            ChildOf(*root),
            Visibility::Hidden,
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(4),
                ..at(24.0, percent(34))
            },
        ))
        .id();
    let row = commands
        .spawn((
            ChildOf(meter),
            Node {
                align_items: AlignItems::Center,
                column_gap: px(10),
                ..default()
            },
        ))
        .id();
    let count = commands
        .spawn((
            ChildOf(row),
            UiTransform::default(),
            Text::new("0"),
            ui::digits(64.0),
            TextColor(INK),
            ui::shadow(),
        ))
        .id();
    let side = commands
        .spawn((
            ChildOf(row),
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(2),
                ..default()
            },
        ))
        .id();
    commands.spawn((
        ChildOf(side),
        Text::new("COMBO"),
        ui::strong(16.0),
        TextColor(DIM),
        ui::shadow(),
    ));
    let badge = commands
        .spawn((
            ChildOf(side),
            UiTransform::default(),
            Node {
                padding: UiRect::axes(px(8), px(0)),
                align_self: AlignSelf::FlexStart,
                ..default()
            },
            BackgroundColor(INK),
            BoxShadow::new(Color::NONE, px(0), px(0), px(0), px(0)),
        ))
        .id();
    let badge_text = commands
        .spawn((
            ChildOf(badge),
            Text::new("x1"),
            ui::digits(26.0),
            TextColor(ON_ACCENT),
        ))
        .id();
    let track = commands
        .spawn((
            ChildOf(meter),
            Node {
                width: px(METER_W),
                height: px(4),
                ..default()
            },
            BackgroundColor(LINE),
        ))
        .id();
    let fill = commands
        .spawn((
            ChildOf(track),
            Node {
                width: percent(100),
                height: percent(100),
                ..default()
            },
            BackgroundColor(ACCENT),
        ))
        .id();
    let score_row = commands
        .spawn((
            ChildOf(meter),
            Node {
                align_items: AlignItems::Baseline,
                column_gap: px(8),
                ..default()
            },
        ))
        .id();
    let score = commands
        .spawn((
            ChildOf(score_row),
            Text::new("0"),
            ui::digits(28.0),
            TextColor(INK),
            ui::shadow(),
        ))
        .id();
    commands.spawn((
        ChildOf(score_row),
        Text::new("PTS"),
        ui::strong(13.0),
        TextColor(DIM),
        ui::shadow(),
    ));
    let pops = std::array::from_fn(|_| {
        commands
            .spawn((
                ChildOf(meter),
                Visibility::Hidden,
                UiTransform::default(),
                Text::new(""),
                ui::digits(20.0),
                TextColor(INK),
                ui::shadow(),
                at(METER_W + 16.0, px(70)),
            ))
            .id()
    });
    // Banner: a full-width row with the card (headline and line under it) centred in it.
    let banner = commands
        .spawn((
            ChildOf(*root),
            Visibility::Hidden,
            Node {
                width: percent(100),
                justify_content: JustifyContent::Center,
                ..at(0.0, percent(19))
            },
        ))
        .id();
    let card = commands
        .spawn((
            ChildOf(banner),
            UiTransform::default(),
            Node {
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                row_gap: px(2),
                ..default()
            },
        ))
        .id();
    let title = commands
        .spawn((
            ChildOf(card),
            Text::new(""),
            ui::display(TITLE[0]),
            TextColor(ACCENT),
            ui::shadow(),
        ))
        .id();
    let sub = commands
        .spawn((
            ChildOf(card),
            Text::new(""),
            ui::strong(22.0),
            TextColor(INK),
            ui::shadow(),
        ))
        .id();
    let flash = commands
        .spawn((
            ChildOf(*root),
            Visibility::Hidden,
            Node {
                width: percent(100),
                height: percent(100),
                ..at(0.0, px(0))
            },
            edge(ACCENT.with_alpha(0.0)),
        ))
        .id();
    commands.insert_resource(Parts {
        meter,
        count,
        badge,
        badge_text,
        fill,
        score,
        pops,
        banner,
        card,
        title,
        sub,
        flash,
    });
}

/// Writes `v` into component `C` of `e` when it differs.
fn put<C: Component<Mutability = Mutable> + PartialEq>(q: &mut Query<&mut C>, e: Entity, v: C) {
    if let Ok(mut c) = q.get_mut(e)
        && *c != v
    {
        *c = v;
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn animate(
    time: Res<Time>,
    settings: Res<Settings>,
    parts: Res<Parts>,
    mut combo: ResMut<Combo>,
    mut vis: Query<&mut Visibility>,
    mut texts: Query<&mut Text>,
    mut fonts: Query<&mut TextFont>,
    mut colors: Query<&mut TextColor>,
    mut tfs: Query<&mut UiTransform>,
    mut nodes: Query<&mut Node>,
    mut bgs: Query<&mut BackgroundColor>,
    mut shadows: Query<&mut BoxShadow>,
    mut grads: Query<&mut BackgroundGradient>,
    mut pops: Local<[Option<Pop>; POPS]>,
    mut shown_banner: Local<f32>,
) {
    let (now, p) = (time.elapsed_secs(), &*parts);
    let show = |on| {
        if on {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        }
    };
    let live = settings.combo && combo.hits > 0;
    put(&mut vis, p.meter, show(live));
    if combo.hits == 0 {
        combo.shown = 0.0;
    }
    // New points add into the newest popup of their kind while it is young, else take a free
    // slot (the oldest if none): popups of a kind never overlap.
    for (points, kill) in std::mem::take(&mut combo.pending) {
        let young = pops
            .iter()
            .enumerate()
            .filter_map(|(i, p)| p.as_ref().filter(|p| p.kill == kill).map(|p| (i, p.born)))
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .filter(|&(_, born)| now - born < POP_MERGE)
            .map(|(i, _)| i);
        let slot = young.unwrap_or_else(|| {
            pops.iter()
                .position(Option::is_none)
                .or_else(|| {
                    (0..POPS).min_by(|&a, &b| {
                        let born = |i: usize| pops[i].as_ref().map_or(f32::MIN, |p| p.born);
                        born(a).total_cmp(&born(b))
                    })
                })
                .unwrap_or(0)
        });
        let pop = pops[slot].get_or_insert(Pop {
            born: now,
            punch: now,
            kill,
            points: 0,
        });
        if young.is_none() {
            *pop = Pop {
                born: now,
                punch: now,
                kill,
                points: 0,
            };
            if let Ok(mut f) = fonts.get_mut(p.pops[slot]) {
                *f = ui::digits(if kill { 28.0 } else { 20.0 });
            }
            if let Ok(mut n) = nodes.get_mut(p.pops[slot]) {
                n.top = px(if kill { POP_KILL_TOP } else { POP_HIT_TOP });
            }
        }
        pop.points += points;
        pop.punch = now;
        if let Ok(mut t) = texts.get_mut(p.pops[slot]) {
            t.0 = format!("+{}", pop.points);
        }
    }
    for (slot, pop) in pops.iter_mut().enumerate() {
        let Some(Pop {
            born, punch, kill, ..
        }) = *pop
        else {
            continue;
        };
        let age = (now - born) / POP_LIFE;
        if age >= 1.0 {
            *pop = None;
            put(&mut vis, p.pops[slot], Visibility::Hidden);
            continue;
        }
        let color = if kill { ACCENT } else { INK };
        put(&mut vis, p.pops[slot], Visibility::Inherited);
        put(
            &mut colors,
            p.pops[slot],
            TextColor(color.with_alpha(1.0 - age * age)),
        );
        let mut tf = UiTransform::from_translation(Val2::px(0.0, -POP_RISE * ui::ease_out(age)));
        tf.scale = Vec2::splat(1.0 + 0.25 * (1.0 - ui::ease_out((now - punch) / 0.1)));
        put(&mut tfs, p.pops[slot], tf);
    }
    if live {
        let m = mult(combo.hits);
        let t = (m - 1) as f32 / MULT_AT.len() as f32;
        let hot = heat(t);
        // the score rolls up to its value
        let gap = combo.score as f32 - combo.shown;
        combo.shown = if gap < 1.0 {
            combo.score as f32
        } else {
            combo.shown + gap * (1.0 - (-12.0 * time.delta_secs()).exp())
        };
        if let Ok(mut x) = texts.get_mut(p.count) {
            set(&mut x, combo.hits.to_string());
        }
        if let Ok(mut x) = texts.get_mut(p.badge_text) {
            set(&mut x, format!("x{m}"));
        }
        if let Ok(mut x) = texts.get_mut(p.score) {
            set(&mut x, (combo.shown.round() as u32).to_string());
        }
        // punches: the count on every hit, the badge on a multiplier step
        let punch = |since: f32, (extra, secs): (f32, f32)| {
            Vec2::splat(1.0 + extra * (1.0 - ui::ease_out((now - since) / secs)))
        };
        put(
            &mut tfs,
            p.count,
            UiTransform::from_scale(punch(combo.hit_at, PUNCH)),
        );
        put(
            &mut tfs,
            p.badge,
            UiTransform::from_scale(punch(combo.step_at, BADGE)),
        );
        put(&mut colors, p.count, TextColor(hot));
        put(&mut bgs, p.badge, BackgroundColor(hot));
        put(&mut bgs, p.fill, BackgroundColor(hot));
        // the glow of the badge grows with the multiplier
        if let Ok(mut s) = shadows.get_mut(p.badge) {
            *s = BoxShadow::new(
                hot.with_alpha(0.3 + 0.6 * t),
                px(0),
                px(0),
                px(2.0 + 4.0 * t),
                px(8.0 + 16.0 * t),
            );
        }
        if let Ok(mut n) = nodes.get_mut(p.fill) {
            let w = percent((1.0 - (now - combo.last) / WINDOW).clamp(0.0, 1.0) * 100.0);
            if n.width != w {
                n.width = w;
            }
        }
    }
    // Banner and edge flash.
    let age = combo.banner.as_ref().map(|b| now - b.at);
    if age.is_some_and(|a| a >= BANNER_LIFE) {
        combo.banner = None;
    }
    let (Some(b), true) = (&combo.banner, settings.combo) else {
        put(&mut vis, p.banner, Visibility::Hidden);
        put(&mut vis, p.flash, Visibility::Hidden);
        return;
    };
    let a = now - b.at;
    let heat_of = heat((b.tier + 2) as f32 / 6.0);
    if *shown_banner != b.at {
        *shown_banner = b.at;
        if let Ok(mut x) = texts.get_mut(p.title) {
            x.0 = b.title.into();
        }
        if let Ok(mut x) = texts.get_mut(p.sub) {
            x.0 = b.sub.clone();
        }
        if let Ok(mut f) = fonts.get_mut(p.title) {
            *f = ui::display(TITLE[b.tier]);
        }
    }
    put(&mut vis, p.banner, Visibility::Inherited);
    // slam in from large, settle with a little undershoot
    let scale = if a < 0.12 {
        lerp(2.2, 0.92, ui::ease_out(a / 0.12))
    } else {
        lerp(0.92, 1.0, ui::ease_out((a - 0.12) / 0.12))
    };
    let shake = (1.0 - a / SHAKE_SECS).max(0.0);
    let fade = ui::ease_out(a / 0.06)
        * (1.0 - ui::ease_out((a - (BANNER_LIFE - BANNER_OUT)) / BANNER_OUT));
    if let Ok(mut tf) = tfs.get_mut(p.card) {
        tf.scale = Vec2::splat(scale);
        tf.translation = Val2::px(
            10.0 * shake * (a * 120.0).sin(),
            6.0 * shake * (a * 95.0).cos(),
        );
    }
    put(&mut colors, p.title, TextColor(heat_of.with_alpha(fade)));
    put(&mut colors, p.sub, TextColor(INK.with_alpha(fade)));
    let flash = (1.0 - a / FLASH_SECS).max(0.0);
    put(&mut vis, p.flash, show(flash > 0.0));
    if flash > 0.0
        && let Ok(mut g) = grads.get_mut(p.flash)
    {
        *g = edge(heat_of.with_alpha(0.32 * flash));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multiplier_steps_with_hits() {
        let at: Vec<u32> = [0, 1, 4, 5, 9, 10, 19, 20, 34, 35, 99, 100, 500]
            .map(mult)
            .into();
        assert_eq!(at, [1, 1, 1, 2, 2, 3, 3, 4, 4, 5, 7, 8, 8]);
    }

    #[test]
    fn multikills_and_sprees_announce_at_their_steps() {
        assert_eq!(multikill(1), None);
        assert_eq!(multikill(2), Some(("DOUBLE KILL", 0)));
        assert_eq!(multikill(9), Some(("MONSTER KILL", 4)));
        let named: Vec<u32> = (1..=20).filter(|&k| spree(k).is_some()).collect();
        assert_eq!(named, [3, 5, 7, 10, 15, 20]);
        assert_eq!(spree(10), Some(("UNSTOPPABLE", 3)));
        assert_eq!(spree(20), Some(("GODLIKE", 4)));
    }

    #[test]
    fn combo_chain_spree_and_expiry() {
        let mut c = Combo::default();
        assert!(!c.hit(1.0, 20.0));
        assert_eq!(c.score, 20);
        for i in 0..3 {
            c.hit(1.1 + i as f32 * 0.1, 10.0);
        }
        assert!(c.hit(1.5, 10.0), "the fifth hit steps to x2");
        assert_eq!(c.score, 20 + 30 + 20);
        // kills 1 s apart chain; the third is both a TRIPLE KILL and a KILLING SPREE
        let (p, b) = c.kill(2.0, false, false);
        assert_eq!((p, b.is_none()), (KILL * 2, true));
        let (p, b) = c.kill(3.0, true, false);
        assert_eq!(p, (KILL + HEAD + BONUS) * 2);
        assert_eq!(b.unwrap().title, "DOUBLE KILL");
        let b = c.kill(4.0, false, true).1.unwrap();
        assert_eq!((b.title, b.tier), ("TRIPLE KILL", 1));
        assert!(b.sub.starts_with("KILLING SPREE  +"));
        // the combo runs out WINDOW seconds after the 5th hit
        assert_eq!(c.expire(1.5 + WINDOW), None);
        assert_eq!(c.expire(1.5 + WINDOW + 0.1).map(|e| e.0), Some(5));
        assert_eq!(c.hits, 0);
        // a gap past CHAIN restarts the chain, the spree goes on
        assert_eq!(
            (c.kill(9.0, false, false).1.is_none(), c.chain, c.spree),
            (true, 1, 4)
        );
        c.hit(20.0, 1.0);
        c.die();
        assert_eq!((c.hits, c.score, c.chain, c.spree), (0, 0, 0, 0));
    }
}
