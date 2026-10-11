//! Dash trail (**not retail**): what a dashing character sheds. The match's tumble loop
//! (`actor::dash_trail`) and the main menu's demo (`menu::demo`) both write one [`Dash`] per
//! [`due`] step of a burst; [`DashFxPlugin`] turns it into the player's chosen [`Trail`]:
//! afterimages (frozen glowing copies of the model, fading where they were left), particles
//! (a few hundred additive streaks sharing one mesh, flying back and shrinking), or both.
//! The settings are cosmetic and live in the profile (`dash_*` keys), apart from the graphics
//! presets; the GRAPHICS panel's speed effects only cover the screen. Notes: `docs/formats.md`.

use crate::{combat::rnd, game::Team, profile::Profile};
use bevy::{camera::visibility::NoFrustumCulling, mesh::skinning::SkinnedMesh, prelude::*};
use std::collections::HashMap;

pub struct DashFxPlugin;

impl Plugin for DashFxPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Dash>()
            .add_systems(Startup, setup)
            .add_systems(Update, (emit, fade));
    }
}

/// What a dash leaves behind.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Style {
    Off,
    #[default]
    Afterimage,
    Particles,
    Both,
}

/// Value, profile text and label of each option of a setting.
pub const STYLES: [(Style, &str, &str); 4] = [
    (Style::Off, "off", "OFF"),
    (Style::Afterimage, "afterimage", "AFTERIMAGE"),
    (Style::Particles, "particles", "PARTICLES"),
    (Style::Both, "both", "BOTH"),
];

/// The trail's colour. `Team` is the old look (red for the red team, cyan otherwise); the
/// others apply to the player's own trail, other actors keep their team colours.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Hue {
    #[default]
    Team,
    Cyan,
    Gold,
    Violet,
    Ember,
    Toxic,
    /// Every piece takes the next hue of the spectrum: the trail runs through all of it.
    Rainbow,
}

pub const HUES: [(Hue, &str, &str); 7] = [
    (Hue::Team, "team", "TEAM"),
    (Hue::Cyan, "cyan", "CYAN"),
    (Hue::Gold, "gold", "GOLD"),
    (Hue::Violet, "violet", "VIOLET"),
    (Hue::Ember, "ember", "EMBER"),
    (Hue::Toxic, "toxic", "TOXIC"),
    (Hue::Rainbow, "rainbow", "RAINBOW"),
];

impl Hue {
    /// Linear colour, brighter than 1 so the additive trail blooms. `phase` (turns) picks the
    /// spectrum position of `Rainbow`.
    pub fn color(self, team: Option<Team>, phase: f32) -> LinearRgba {
        let (r, g, b) = match self {
            Hue::Team if team == Some(Team::Red) => (1.6, 0.35, 0.2),
            Hue::Team => (0.3, 0.75, 1.6),
            Hue::Cyan => (0.1, 1.3, 1.6),
            Hue::Gold => (1.7, 1.2, 0.2),
            Hue::Violet => (1.0, 0.3, 1.8),
            Hue::Ember => (1.8, 0.55, 0.1),
            Hue::Toxic => (0.5, 1.7, 0.15),
            Hue::Rainbow => {
                let c = Color::hsl(phase.fract().abs() * 360.0, 1.0, 0.5).to_linear();
                (c.red * 1.6, c.green * 1.6, c.blue * 1.6)
            }
        };
        LinearRgba::rgb(r, g, b)
    }
}

/// The dash trail settings (`dash_style`, `dash_color`, `dash_amount` in the profile).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Trail {
    pub style: Style,
    pub hue: Hue,
    /// 0..=1: how many particles and how opaque the afterimages (0 sheds nothing).
    pub amount: f32,
}

impl Default for Trail {
    /// Today's look: afterimages in team colour at the opacity the speed effects had in ENHANCED.
    fn default() -> Self {
        Self {
            style: Style::Afterimage,
            hue: Hue::Team,
            amount: 0.5,
        }
    }
}

impl Trail {
    pub fn to_text(&self) -> String {
        let style = STYLES.iter().find(|o| o.0 == self.style).unwrap().1;
        let hue = HUES.iter().find(|o| o.0 == self.hue).unwrap().1;
        format!(
            "dash_style={style}\ndash_color={hue}\ndash_amount={}\n",
            self.amount
        )
    }

    /// Takes one profile line; `Ok(false)`: not a dash key.
    pub fn set_text(&mut self, k: &str, v: &str) -> Result<bool, String> {
        let bad = || format!("{k}: bad value {v:?}");
        let v = v.trim();
        match k {
            "dash_style" => self.style = STYLES.iter().find(|o| o.1 == v).ok_or_else(bad)?.0,
            "dash_color" => self.hue = HUES.iter().find(|o| o.1 == v).ok_or_else(bad)?.0,
            "dash_amount" => {
                let n = v.parse::<f32>().ok().filter(|x| x.is_finite());
                self.amount = n.ok_or_else(bad)?.clamp(0.0, 1.0);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }
}

/// Asks for one trail piece of the model `model` (the root that carries the skeleton): the
/// actor dashing along `dir` (horizontal, world space; particles fly the other way). `own`:
/// the player's own dash, which takes the chosen colour; everyone else's is in team colour.
#[derive(Message)]
pub struct Dash {
    pub model: Entity,
    pub dir: Vec3,
    pub team: Option<Team>,
    pub own: bool,
}

/// Seconds between trail pieces of a burst.
const EVERY: f32 = 0.04;

/// A dash `t` seconds in (`dt` since the last frame) owes a trail piece: `t` crossed a multiple
/// of [`EVERY`] while still inside the burst of `burst` seconds.
pub fn due(t: f32, dt: f32, burst: f32) -> bool {
    t <= burst && (t / EVERY).floor() != ((t - dt) / EVERY).floor()
}

/// A frozen, glowing copy of an actor's visible meshes that fades out where it was left.
#[derive(Component)]
struct Ghost {
    age: f32,
    alpha: f32,
    mat: Handle<StandardMaterial>,
}

const GHOST_LIFE: f32 = 0.3;
/// Afterimage opacity at `amount` 1 (0.27 at the default 0.5, as the speed effects gave).
const GHOST_ALPHA: f32 = 0.54;

/// One streak of the particle trail.
#[derive(Component)]
struct Spark {
    vel: Vec3,
    age: f32,
    life: f32,
    /// Streak length at birth (z scale of the shared mesh).
    long: f32,
}

/// At most this many streaks live at once.
const MAX_SPARKS: usize = 1200;

/// The streak mesh and a material per colour (a handful: team, the vivid choices, and the
/// quantised steps of the rainbow).
#[derive(Resource)]
struct Sparks {
    mesh: Handle<Mesh>,
    mats: HashMap<u32, Handle<StandardMaterial>>,
}

fn setup(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>) {
    commands.insert_resource(Sparks {
        mesh: meshes.add(Cuboid::new(0.035, 0.035, 0.2)),
        mats: HashMap::new(),
    });
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn emit(
    mut dashes: MessageReader<Dash>,
    time: Res<Time>,
    profile: Option<Res<Profile>>,
    mut sparks: ResMut<Sparks>,
    live: Query<(), With<Spark>>,
    children: Query<&Children>,
    parts: Query<(
        &Mesh3d,
        Option<&SkinnedMesh>,
        &InheritedVisibility,
        &GlobalTransform,
    )>,
    globals: Query<&GlobalTransform>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut seed: Local<u32>,
    mut last: Local<HashMap<Entity, f32>>,
    mut commands: Commands,
) {
    let trail = profile.map_or_else(Trail::default, |p| p.dash);
    if trail.style == Style::Off || trail.amount <= 0.0 {
        dashes.clear();
        return;
    }
    if *seed == 0 {
        *seed = 0x9e37_79b9;
    }
    for d in dashes.read() {
        let hue = if d.own { trail.hue } else { Hue::Team };
        // the rainbow runs through the spectrum about once per burst
        let phase = time.elapsed_secs() * 3.0;
        let color = hue.color(d.team, phase);
        if matches!(trail.style, Style::Afterimage | Style::Both) {
            let alpha = GHOST_ALPHA * trail.amount;
            let mat = materials.add(StandardMaterial {
                base_color: Color::from(color.with_alpha(alpha)),
                unlit: true,
                alpha_mode: AlphaMode::Add,
                ..default()
            });
            let root = commands
                .spawn((
                    Ghost {
                        age: 0.0,
                        alpha,
                        mat: mat.clone(),
                    },
                    Transform::IDENTITY,
                    Visibility::Visible,
                ))
                .id();
            // world-space copies of the joints, shared by every skinned part
            let mut joints = HashMap::new();
            for e in children.iter_descendants(d.model) {
                let Ok((mesh, skin, shown, at)) = parts.get(e) else {
                    continue;
                };
                if !shown.get() {
                    continue;
                }
                let part = (
                    Mesh3d(mesh.0.clone()),
                    MeshMaterial3d(mat.clone()),
                    ChildOf(root),
                );
                match skin {
                    Some(s) => {
                        let copied = s
                            .joints
                            .iter()
                            .map(|j| {
                                *joints.entry(*j).or_insert_with(|| {
                                    let at = globals
                                        .get(*j)
                                        .map_or(Transform::IDENTITY, |g| g.compute_transform());
                                    commands.spawn((at, ChildOf(root))).id()
                                })
                            })
                            .collect();
                        commands.spawn((
                            part,
                            SkinnedMesh {
                                inverse_bindposes: s.inverse_bindposes.clone(),
                                joints: copied,
                            },
                            NoFrustumCulling,
                        ));
                    }
                    None => _ = commands.spawn((part, at.compute_transform())),
                }
            }
        }
        if matches!(trail.style, Style::Particles | Style::Both) {
            // 12 spectrum steps keep the rainbow to a dozen materials
            let color = if hue == Hue::Rainbow {
                hue.color(d.team, (phase * 12.0).floor() / 12.0)
            } else {
                color
            };
            let key = ((color.red * 16.0) as u32) << 20
                | ((color.green * 16.0) as u32) << 10
                | (color.blue * 16.0) as u32;
            // a hot core: far above 1 so bloom catches it
            let core = LinearRgba::rgb(color.red, color.green, color.blue) * 3.5;
            let mat = sparks
                .mats
                .entry(key)
                .or_insert_with(|| {
                    materials.add(StandardMaterial {
                        base_color: Color::from(core.with_alpha(1.0)),
                        unlit: true,
                        alpha_mode: AlphaMode::Add,
                        ..default()
                    })
                })
                .clone();
            let feet = globals.get(d.model).map_or(Vec3::ZERO, |g| g.translation());
            // the first piece of a burst throws a burst of sparks
            let now = time.elapsed_secs();
            last.retain(|_, t| now - *t < 2.0);
            let first = last.insert(d.model, now).is_none_or(|t| now - t > 0.2);
            let per = (6.0 + 24.0 * trail.amount).round() as usize * if first { 3 } else { 1 };
            let n = per.min(MAX_SPARKS.saturating_sub(live.iter().count()));
            let mut r = || rnd(&mut seed);
            for _ in 0..n {
                let speed = 2.0 + r() * 5.0;
                let vel = -d.dir * speed + Vec3::new(r() - 0.5, r() - 0.3, r() - 0.5) * 1.6;
                // spread over the whole body
                let at = feet + Vec3::new((r() - 0.5) * 0.7, 0.05 + r() * 1.65, (r() - 0.5) * 0.7);
                let long = 1.0 + speed * 0.4;
                commands.spawn((
                    Spark {
                        vel,
                        age: 0.0,
                        life: 0.35 + r() * 0.5,
                        long,
                    },
                    Mesh3d(sparks.mesh.clone()),
                    MeshMaterial3d(mat.clone()),
                    Transform::from_translation(at)
                        .looking_to(vel, Vec3::Y)
                        .with_scale(Vec3::new(1.0, 1.0, long)),
                ));
            }
        }
    }
}

/// Fades the afterimages and flies and shrinks the streaks.
fn fade(
    time: Res<Time>,
    mut ghosts: Query<(Entity, &mut Ghost)>,
    mut sparks: Query<(Entity, &mut Spark, &mut Transform)>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut commands: Commands,
) {
    let dt = time.delta_secs();
    for (e, mut g) in &mut ghosts {
        g.age += dt;
        if g.age >= GHOST_LIFE {
            commands.entity(e).despawn();
        } else if let Some(mut m) = materials.get_mut(&g.mat) {
            m.base_color
                .set_alpha(g.alpha * (1.0 - g.age / GHOST_LIFE).powi(2));
        }
    }
    for (e, mut s, mut at) in &mut sparks {
        s.age += dt;
        if s.age >= s.life {
            commands.entity(e).despawn();
            continue;
        }
        let k = 1.0 - s.age / s.life;
        s.vel = s.vel * (1.0 - 3.0 * dt).max(0.0) - Vec3::Y * 2.0 * dt;
        at.translation += s.vel * dt;
        at.scale = Vec3::new(k, k, s.long * k);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trail_survives_the_profile_text() {
        let mut t = Trail::default();
        for line in "dash_style=both\ndash_color=rainbow\ndash_amount=0.8".lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert_eq!(t.set_text(k, v), Ok(true));
        }
        assert_eq!((t.style, t.hue, t.amount), (Style::Both, Hue::Rainbow, 0.8));
        let mut back = Trail::default();
        for line in t.to_text().lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert_eq!(back.set_text(k, v), Ok(true));
        }
        assert_eq!(back, t);
        assert!(back.set_text("dash_style", "loud").is_err());
        assert_eq!(back.set_text("gfx_aa", "off"), Ok(false));
    }

    #[test]
    fn a_burst_owes_a_piece_every_step() {
        let hits = (0..60)
            .filter(|i| due(*i as f32 / 60.0, 1.0 / 60.0, 0.3))
            .count();
        assert_eq!(hits, 8, "0.04 s steps over the first 0.3 s");
    }
}
