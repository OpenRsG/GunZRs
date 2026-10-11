//! Sword slash trail (**not retail**): a glowing ribbon swept by a swinging blade, plus sparks
//! where a blow lands. Retail has no swing trail: `sfx/ef_sword_*` are blade auras of the
//! `effect_melee` customise items (which one an item enables is not in the data) and
//! `sword_slash_effect` is a burst, so every style here is procedural and the GRAPHICS preset
//! ORIGINAL draws none of them ([`dashfx::original`]). Notes: `docs/formats.md`.
//!
//! Anything that swings a blade puts [`Swing`] on the weapon model's root: [`mark`] does it for
//! every [`Melee`] actor while its blow clip plays, the PLAYER page's demo (`menu::demo`) for its
//! sword. [`sweep`] measures the blade once ([`Blade`]: the model's longest axis, grip end to
//! tip), samples its world position every frame into a [`Ribbon`] and rebuilds the ribbon's
//! mesh: additive, fading over time ([`Slash::life`]) and along the blade, brighter where the
//! tip moves fast. The settings are the `slash_*` profile keys, the colours are the dash
//! trail's ([`Hue`]); the player's own blade takes the chosen colour, everyone else's the team
//! colour. A landed blow ([`Hit`], from `melee::resolve`) also sprays a few sparks.

use crate::{
    actor::Actor,
    dashfx::{self, HUES, Hue, Spray},
    game::{Player, Team},
    melee::Melee,
    profile::Profile,
};
use bevy::{
    asset::RenderAssetUsages,
    camera::{primitives::Aabb, visibility::NoFrustumCulling},
    mesh::{Indices, PrimitiveTopology},
    prelude::*,
};
use std::collections::VecDeque;

pub struct SlashFxPlugin;

impl Plugin for SlashFxPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Hit>()
            .add_systems(Startup, setup)
            .add_systems(Update, (mark, sweep, sparks).chain());
    }
}

/// How the ribbon looks.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Style {
    Off,
    /// A smooth glowing ribbon.
    #[default]
    Arc,
    /// A white-hot core in a soft halo that flickers.
    Energy,
    /// Ragged and flickering, rising as it fades.
    Fire,
}

/// Value, profile text and label of each style.
pub const STYLES: [(Style, &str, &str); 4] = [
    (Style::Off, "off", "OFF"),
    (Style::Arc, "arc", "ARC"),
    (Style::Energy, "energy", "ENERGY"),
    (Style::Fire, "fire", "FIRE"),
];

/// The slash settings (`slash_style`, `slash_color`, `slash_amount` in the profile).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Slash {
    pub style: Style,
    pub hue: Hue,
    /// 0..=1: how bright, how long-lived and how many sparks (0 draws nothing).
    pub amount: f32,
}

impl Default for Slash {
    fn default() -> Self {
        Self {
            style: Style::Arc,
            hue: Hue::Team,
            amount: 0.5,
        }
    }
}

impl Slash {
    pub fn to_text(&self) -> String {
        let style = STYLES.iter().find(|o| o.0 == self.style).unwrap().1;
        let hue = HUES.iter().find(|o| o.0 == self.hue).unwrap().1;
        format!(
            "slash_style={style}\nslash_color={hue}\nslash_amount={}\n",
            self.amount
        )
    }

    /// Takes one profile line; `Ok(false)`: not a slash key.
    pub fn set_text(&mut self, k: &str, v: &str) -> Result<bool, String> {
        let bad = || format!("{k}: bad value {v:?}");
        let v = v.trim();
        match k {
            "slash_style" => self.style = STYLES.iter().find(|o| o.1 == v).ok_or_else(bad)?.0,
            "slash_color" => self.hue = HUES.iter().find(|o| o.1 == v).ok_or_else(bad)?.0,
            "slash_amount" => {
                let n = v.parse::<f32>().ok().filter(|x| x.is_finite());
                self.amount = n.ok_or_else(bad)?.clamp(0.0, 1.0);
            }
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Seconds a ribbon piece lives: 0.1 s at `amount` 0 up to 0.3 s.
    fn life(&self) -> f32 {
        0.1 + 0.2 * self.amount
    }

    fn intensity(&self) -> f32 {
        0.4 + 0.6 * self.amount
    }

    /// The settings to draw with, `None` when nothing is drawn: the style is off, the amount is
    /// 0 or the graphics preset is ORIGINAL.
    fn active(profile: Option<&Profile>) -> Option<Self> {
        let s = profile.map_or_else(Self::default, |p| p.slash);
        (s.style != Style::Off && s.amount > 0.0 && !profile.is_some_and(dashfx::original))
            .then_some(s)
    }
}

/// On a weapon model's root while its blade swings: it gets a trail. `own`: the player's blade
/// (it takes the chosen colour); everyone else's is in team colour. `cap`: longest life (s) of a
/// trail piece, so a small stage (the menu preview) keeps the trail short.
#[derive(Component)]
pub struct Swing {
    pub team: Option<Team>,
    pub own: bool,
    pub cap: f32,
}

/// A blow landed on someone at `point`, `dir` from the attacker to the victim.
#[derive(Message)]
pub struct Hit {
    pub attacker: Entity,
    pub point: Vec3,
    pub dir: Vec3,
}

/// A weapon model's blade in the model's own frame: the end of its longest axis near the grip
/// (the model's origin is its hand node) and the far end.
#[derive(Component)]
struct Blade {
    base: Vec3,
    tip: Vec3,
}

/// One sampled blade position.
#[derive(Clone, Copy)]
struct Pt {
    base: Vec3,
    tip: Vec3,
    /// Seconds on the game clock when it was sampled.
    born: f32,
    /// Spectrum position (turns) for the rainbow.
    hue: f32,
    /// 0..=1 from the tip's speed: a slow blade (wind-up, recovery) leaves nothing.
    speed: f32,
}

/// The trail of one weapon: points while the swing goes on (`open`), then they fade out.
#[derive(Component)]
struct Ribbon {
    root: Entity,
    team: Option<Team>,
    own: bool,
    cap: f32,
    open: bool,
    pts: VecDeque<Pt>,
    /// Made when the ribbon first has two points.
    mesh: Option<Handle<Mesh>>,
}

#[derive(Resource)]
struct Mat(Handle<StandardMaterial>);

fn setup(mut commands: Commands, mut materials: ResMut<Assets<StandardMaterial>>) {
    // the vertex colours carry the colour and the fade
    commands.insert_resource(Mat(materials.add(StandardMaterial {
        unlit: true,
        alpha_mode: AlphaMode::Add,
        cull_mode: None,
        double_sided: true,
        ..default()
    })));
}

/// Every melee actor's weapon models swing while its blow clip plays.
fn mark(
    actors: Query<(&Actor, &Melee, Option<&Team>, Has<Player>)>,
    swings: Query<(), With<Swing>>,
    mut commands: Commands,
) {
    for (a, m, team, own) in &actors {
        let on = m.swinging();
        for r in a.weapon_roots() {
            if on && !swings.contains(r) {
                commands.entity(r).insert(Swing {
                    team: team.copied(),
                    own,
                    cap: f32::MAX,
                });
            } else if !on && swings.contains(r) {
                commands.entity(r).remove::<Swing>();
            }
        }
    }
}

/// The blade of the model rooted at `root`: the longest axis of its meshes' bounds, in the
/// root's frame. `None` until the bounds exist.
fn measure(
    root: Entity,
    at: &GlobalTransform,
    children: &Query<&Children>,
    bounds: &Query<(&Aabb, &GlobalTransform)>,
) -> Option<Blade> {
    let into_root = at.affine().inverse();
    let (mut lo, mut hi) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
    for e in children.iter_descendants(root) {
        let Ok((b, g)) = bounds.get(e) else { continue };
        for c in 0..8 {
            let corner = Vec3A::new(
                if c & 1 == 0 { -1.0 } else { 1.0 },
                if c & 2 == 0 { -1.0 } else { 1.0 },
                if c & 4 == 0 { -1.0 } else { 1.0 },
            );
            let p = into_root.transform_point3a(
                g.affine()
                    .transform_point3a(b.center + b.half_extents * corner),
            );
            (lo, hi) = (lo.min(Vec3::from(p)), hi.max(Vec3::from(p)));
        }
    }
    if lo.x > hi.x {
        return None;
    }
    let ext = hi - lo;
    let axis = if ext.x >= ext.y && ext.x >= ext.z {
        0
    } else if ext.y >= ext.z {
        1
    } else {
        2
    };
    let (mut a, mut b) = ((lo + hi) * 0.5, (lo + hi) * 0.5);
    (a[axis], b[axis]) = (lo[axis], hi[axis]);
    let (base, tip) = if a.length() <= b.length() {
        (a, b)
    } else {
        (b, a)
    };
    debug!("slashfx: blade {base:?} -> {tip:?} (cm, model frame)");
    Some(Blade { base, tip })
}

/// Samples the swinging blades into ribbons and rebuilds the ribbons' meshes.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn sweep(
    time: Res<Time>,
    profile: Option<Res<Profile>>,
    mat: Res<Mat>,
    roots: Query<(
        Entity,
        &Swing,
        &GlobalTransform,
        &InheritedVisibility,
        Option<&Blade>,
    )>,
    children: Query<&Children>,
    bounds: Query<(&Aabb, &GlobalTransform)>,
    mut ribbons: Query<(Entity, &mut Ribbon, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut commands: Commands,
) {
    let Some(s) = Slash::active(profile.as_deref()) else {
        for (e, ..) in &ribbons {
            commands.entity(e).despawn();
        }
        return;
    };
    let (now, dt) = (time.elapsed_secs(), time.delta_secs().max(1e-3));
    let mut sampled = Vec::new();
    for (r, sw, g, vis, blade) in &roots {
        if !vis.get() {
            continue;
        }
        let Some(blade) = blade else {
            if let Some(b) = measure(r, g, &children, &bounds) {
                commands.entity(r).insert(b);
            }
            continue;
        };
        sampled.push(r);
        let (base, tip) = (g.transform_point(blade.base), g.transform_point(blade.tip));
        let mut pt = Pt {
            base,
            tip,
            born: now,
            hue: now * 3.0,
            speed: 0.0,
        };
        match ribbons
            .iter_mut()
            .find(|(_, rb, _)| rb.root == r && rb.open)
        {
            Some((_, mut rb, _)) => {
                let v = rb.pts.back().map_or(0.0, |p| p.tip.distance(tip) / dt);
                pt.speed = ((v - 2.0) / 5.0).clamp(0.0, 1.0);
                rb.pts.push_back(pt);
            }
            None => {
                commands.spawn((
                    Ribbon {
                        root: r,
                        team: sw.team,
                        own: sw.own,
                        cap: sw.cap,
                        open: true,
                        pts: VecDeque::from([pt]),
                        mesh: None,
                    },
                    MeshMaterial3d(mat.0.clone()),
                    Transform::IDENTITY,
                    Visibility::Hidden,
                    NoFrustumCulling,
                ));
            }
        }
    }
    for (e, mut rb, mut vis) in &mut ribbons {
        if !sampled.contains(&rb.root) {
            rb.open = false;
        }
        while rb
            .pts
            .front()
            .is_some_and(|p| now - p.born > s.life().min(rb.cap))
        {
            rb.pts.pop_front();
        }
        if !rb.open && rb.pts.is_empty() {
            commands.entity(e).despawn();
            continue;
        }
        let shown = rb.pts.len() >= 2;
        *vis = if shown {
            Visibility::Inherited
        } else {
            Visibility::Hidden
        };
        if shown {
            let (pos, col, idx) = shape(&rb, s, now);
            let mut mesh = Mesh::new(
                PrimitiveTopology::TriangleList,
                RenderAssetUsages::default(),
            )
            .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, pos)
            .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, col);
            mesh.insert_indices(Indices::U32(idx));
            // a mesh is only added once it has vertices (an empty one breaks the GPU allocator)
            match rb.mesh.clone() {
                Some(h) => {
                    if let Some(mut m) = meshes.get_mut(&h) {
                        *m = mesh;
                    }
                }
                None => {
                    let h = meshes.add(mesh);
                    rb.mesh = Some(h.clone());
                    commands.entity(e).insert(Mesh3d(h));
                }
            }
        }
    }
}

/// Catmull-Rom point between `p[1]` and `p[2]` at `t`.
fn spline(p: [Vec3; 4], t: f32) -> Vec3 {
    0.5 * (2.0 * p[1]
        + (p[2] - p[0]) * t
        + (2.0 * p[0] - 5.0 * p[1] + 4.0 * p[2] - p[3]) * t * t
        + (3.0 * (p[1] - p[2]) + p[3] - p[0]) * t * t * t)
}

/// Pseudo-random 0..1 from `x`.
fn noise(x: f32) -> f32 {
    (x.sin() * 43758.547).fract().abs()
}

/// Rows across the blade of each style: blade fraction from the grip, opacity, whiteness.
fn rows(style: Style) -> &'static [(f32, f32, f32)] {
    match style {
        Style::Energy => &[
            (0.6, 0.0, 0.0),
            (0.85, 0.35, 0.0),
            (0.96, 1.0, 0.9),
            (1.04, 0.25, 0.0),
        ],
        Style::Fire => &[
            (0.45, 0.0, 0.0),
            (0.7, 0.5, 0.0),
            (0.9, 0.9, 0.15),
            (1.04, 0.6, 0.7),
        ],
        _ => &[(0.55, 0.0, 0.0), (1.0, 1.0, 0.2)],
    }
}

/// Spline pieces between two samples.
const SUB: usize = 3;

/// The ribbon's vertices (positions, linear colours with the opacity in alpha) and triangles.
fn shape(rb: &Ribbon, s: Slash, now: f32) -> (Vec<[f32; 3]>, Vec<[f32; 4]>, Vec<u32>) {
    let n = rb.pts.len();
    let at = |i: isize| rb.pts[i.clamp(0, n as isize - 1) as usize];
    let mut smooth = Vec::with_capacity((n - 1) * SUB + 1);
    for i in 0..n as isize - 1 {
        let (a, b, c, d) = (at(i - 1), at(i), at(i + 1), at(i + 2));
        for j in 0..SUB {
            let t = j as f32 / SUB as f32;
            let lerp = |f: fn(&Pt) -> f32| f(&b) + (f(&c) - f(&b)) * t;
            smooth.push(Pt {
                base: spline([a.base, b.base, c.base, d.base], t),
                tip: spline([a.tip, b.tip, c.tip, d.tip], t),
                born: lerp(|p| p.born),
                hue: lerp(|p| p.hue),
                speed: lerp(|p| p.speed),
            });
        }
    }
    smooth.push(at(n as isize - 1));
    let hue = if rb.own { s.hue } else { Hue::Team };
    let rows = rows(s.style);
    let (mut pos, mut col) = (Vec::new(), Vec::new());
    for (m, p) in smooth.iter().enumerate() {
        let age = (now - p.born).max(0.0);
        let fade = (1.0 - age / s.life().min(rb.cap)).clamp(0.0, 1.0).powf(1.5);
        let base = hue.color(rb.team, p.hue);
        let flick = match s.style {
            Style::Energy | Style::Fire => 0.7 + 0.3 * noise(m as f32 * 7.1 + (now * 30.0).floor()),
            _ => 1.0,
        };
        // fire rises as it fades and its edge frays
        let lift = if s.style == Style::Fire {
            Vec3::Y * (age * 0.8 + 0.06 * noise(m as f32 * 3.7))
        } else {
            Vec3::ZERO
        };
        for (k, &(f, a, white)) in rows.iter().enumerate() {
            let fray = if s.style == Style::Fire {
                let x = m as f32 * 5.3 + k as f32 * 2.9 + (now * 24.0).floor();
                Vec3::new(noise(x) - 0.5, noise(x + 1.7) - 0.5, noise(x + 3.1) - 0.5) * 0.16 * f
            } else {
                Vec3::ZERO
            };
            pos.push((p.base.lerp(p.tip, f) + lift + fray).to_array());
            let c = LinearRgba::rgb(base.red, base.green, base.blue);
            let c = c * (1.0 - white) + LinearRgba::rgb(2.0, 2.0, 2.0) * white;
            col.push([
                c.red,
                c.green,
                c.blue,
                a * fade * p.speed * s.intensity() * flick,
            ]);
        }
    }
    let r = rows.len() as u32;
    let mut idx = Vec::new();
    for m in 0..smooth.len() as u32 - 1 {
        for k in 0..r - 1 {
            let (a, b) = (m * r + k, m * r + k + 1);
            let (c, d) = (a + r, b + r);
            idx.extend([a, b, c, b, d, c]);
        }
    }
    (pos, col, idx)
}

/// A landed blow throws sparks in the attacker's colour.
fn sparks(
    mut hits: MessageReader<Hit>,
    time: Res<Time>,
    profile: Option<Res<Profile>>,
    who: Query<(Option<&Team>, Has<Player>)>,
    mut out: MessageWriter<Spray>,
) {
    let Some(s) = Slash::active(profile.as_deref()) else {
        hits.clear();
        return;
    };
    for h in hits.read() {
        let (team, own) = who
            .get(h.attacker)
            .map_or((None, false), |(t, p)| (t.copied(), p));
        let hue = if own { s.hue } else { Hue::Team };
        out.write(Spray {
            at: h.point,
            dir: (-h.dir + Vec3::Y * 0.4).normalize_or_zero(),
            color: hue.color(team, time.elapsed_secs() * 3.0),
            n: (4.0 + 20.0 * s.amount) as usize,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slash_survives_the_profile_text() {
        let mut s = Slash::default();
        for line in "slash_style=fire\nslash_color=rainbow\nslash_amount=0.8".lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert_eq!(s.set_text(k, v), Ok(true));
        }
        assert_eq!((s.style, s.hue, s.amount), (Style::Fire, Hue::Rainbow, 0.8));
        let mut back = Slash::default();
        for line in s.to_text().lines() {
            let (k, v) = line.split_once('=').unwrap();
            assert_eq!(back.set_text(k, v), Ok(true));
        }
        assert_eq!(back, s);
        assert!(back.set_text("slash_style", "loud").is_err());
        assert_eq!(back.set_text("dash_style", "off"), Ok(false));
    }

    #[test]
    fn a_ribbon_is_a_strip_of_rows() {
        let pt = |x: f32| Pt {
            base: Vec3::new(x, 0.0, 0.0),
            tip: Vec3::new(x, 1.0, 0.0),
            born: 0.0,
            hue: 0.0,
            speed: 1.0,
        };
        for (style, rows) in [(Style::Arc, 2), (Style::Energy, 4), (Style::Fire, 4)] {
            let rb = Ribbon {
                root: Entity::PLACEHOLDER,
                team: None,
                own: true,
                cap: f32::MAX,
                open: true,
                pts: [pt(0.0), pt(1.0), pt(2.0)].into(),
                mesh: None,
            };
            let s = Slash { style, ..default() };
            let (pos, col, idx) = shape(&rb, s, 0.05);
            let samples = 2 * SUB + 1;
            assert_eq!((pos.len(), col.len()), (samples * rows, samples * rows));
            assert_eq!(idx.len(), (samples - 1) * (rows - 1) * 6);
            assert!(idx.iter().all(|i| (*i as usize) < pos.len()));
        }
    }
}
