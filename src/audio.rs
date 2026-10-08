//! `gunz-play` sound. Everything is a stem under `sound/` (`sound/effect/we_rifle_fire.wav` =
//! `we_rifle_fire`), played from the shared game messages; hud.rs only draws. Sources:
//! zitem `snd_*` (guns), the `sound` attribute of character animations (`man_jump`, `fx_dash`),
//! `effect.xml` (distances, 2D/3D type) and the map's `AMBIENTSOUNDLIST`. Which retail file is
//! which cue beyond that is **inferred** from names and lengths; notes: `docs/formats.md`.
//! Other modules play a named stem with `Cue::Anim(stem)` (at an actor) or `PlaySound` (at a
//! point); background music is `music.rs`.

use crate::{
    actor::{Actor, ActorData},
    game::{
        ActorSound, Blast, Blocked, Cue, Damage, Dead, Fire, Impact, Killed, Loadout, PlaySound,
        Player,
    },
    item::Item,
    level::Level,
    map::Map,
    mrs::Vfs,
    pickup::{Picked, SOUND as PICKUP_SOUND},
    view::{SCALE, Shot, to_bevy},
};
use bevy::prelude::*;
use std::collections::{BTreeSet, HashMap};

/// Spatial one-shots are dropped beyond this many metres unless `effect.xml` says otherwise
/// (its default `MAXDISTANCE` is 4000 cm).
const DEFAULT_REACH: f32 = 40.0;
/// One-shot entities removed after this long even if no audio device ever consumed them.
const SOUND_LIFE: f32 = 10.0;
/// Simultaneous one-shots; further 3D sounds are dropped (machine guns x bots).
const MAX_LIVE: usize = 48;
/// Looping ambient sources playing at once (the nearest win).
const MAX_AMBIENT: usize = 8;
/// A shell casing hits the floor this long after the shot (**inferred**).
const SLUG_DELAY: f32 = 0.45;
/// Impact sounds per frame (a shotgun blast is eight pellets).
const MAX_IMPACTS: usize = 3;
/// Walkable-surface lookup tolerance (m) between a point and a polygon plane.
const SURFACE_TOL: f32 = 0.12;
const CELL: f32 = 2.0;

pub struct AudioPlugin;

impl Plugin for AudioPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Later>().add_systems(
            Update,
            (
                load.run_if(resource_exists::<Level>.and_then(resource_exists::<ActorData>))
                    .run_if(not(resource_exists::<Sounds>)),
                (combat_sounds, actor_sounds, requested, later, ambient, reap)
                    .run_if(resource_exists::<Sounds>),
            ),
        );
    }
}

/// Floor/wall material of a polygon, from the `_mt_<suffix>` of its material name (**inferred**
/// meaning: concrete, dirt, metal, wood, paint, sand, snow, water, glass, fish-tank water; the
/// `man_fs_*`/`fx_bullethit_*` files carry the same suffixes). No suffix = concrete.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Surf {
    Con,
    Drt,
    Met,
    Wod,
    Pnt,
    Snd,
    Snw,
    Wat,
    Gls,
    Fsh,
}

impl Surf {
    fn of(material: &str) -> Self {
        let m = material.to_ascii_lowercase();
        let Some(i) = m.rfind("_mt_") else {
            return Surf::Con;
        };
        match m[i + 4..].trim_end_matches(|c: char| !c.is_ascii_alphabetic()) {
            "drt" | "clo" => Surf::Drt,
            "met" => Surf::Met,
            "wod" => Surf::Wod,
            "pnt" => Surf::Pnt,
            "snd" => Surf::Snd,
            "snw" => Surf::Snw,
            "wat" => Surf::Wat,
            "gls" => Surf::Gls,
            "fsh" => Surf::Fsh,
            _ => Surf::Con, // con, fcon
        }
    }

    /// File suffixes to try for this surface, best first; `con` always closes the list.
    fn suffixes(self) -> [&'static str; 3] {
        match self {
            Surf::Con => ["con", "con", "con"],
            Surf::Drt => ["drt", "con", "con"],
            Surf::Met => ["met", "con", "con"],
            Surf::Wod => ["wod", "con", "con"],
            Surf::Pnt => ["pnt", "con", "con"],
            Surf::Snd => ["snd", "con", "con"],
            Surf::Snw => ["snw", "snd", "con"],
            Surf::Wat => ["wat", "con", "con"],
            Surf::Gls => ["gls", "met", "con"],
            Surf::Fsh => ["fsh", "wat", "con"],
        }
    }
}

/// Non-concrete map polygons, bucketed in a 3D grid, to find the material under a point.
/// Concrete is the default, so only the other materials are indexed.
struct Surfaces {
    cells: HashMap<[i32; 3], Vec<u32>>,
    polys: Vec<(Surf, Vec<Vec3>)>,
}

fn cell(p: Vec3) -> [i32; 3] {
    (p / CELL).floor().as_ivec3().to_array()
}

impl Surfaces {
    fn new(map: &Map) -> Self {
        let mut s = Surfaces {
            cells: HashMap::new(),
            polys: Vec::new(),
        };
        for p in &map.polygons {
            let surf = Surf::of(&map.materials[p.material as usize].name);
            if surf == Surf::Con {
                continue;
            }
            let verts: Vec<Vec3> = map.vertices[p.first as usize..(p.first + p.count) as usize]
                .iter()
                .map(|v| Vec3::from(to_bevy(v.pos)) * SCALE)
                .collect();
            if verts.len() < 3 {
                continue;
            }
            let (lo, hi) = verts.iter().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), v| {
                (lo.min(*v), hi.max(*v))
            });
            let (lo, hi) = (
                cell(lo - Vec3::splat(SURFACE_TOL)),
                cell(hi + Vec3::splat(SURFACE_TOL)),
            );
            let i = s.polys.len() as u32;
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        s.cells.entry([x, y, z]).or_default().push(i);
                    }
                }
            }
            s.polys.push((surf, verts));
        }
        s
    }

    /// Material of the polygon through `p` (within [`SURFACE_TOL`]); with `facing`, only
    /// polygons whose normal is near it count (a bullet hole on a wall, not the floor beside).
    fn at(&self, p: Vec3, facing: Option<Vec3>) -> Surf {
        let (lo, hi) = (
            cell(p - Vec3::splat(SURFACE_TOL)),
            cell(p + Vec3::splat(SURFACE_TOL)),
        );
        let mut best = (SURFACE_TOL, Surf::Con);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    for &i in self.cells.get(&[x, y, z]).into_iter().flatten() {
                        let (surf, v) = &self.polys[i as usize];
                        let n = (v[1] - v[0]).cross(v[2] - v[0]).normalize_or_zero();
                        let d = (p - v[0]).dot(n);
                        if d.abs() >= best.0 || facing.is_some_and(|f| n.dot(f).abs() < 0.7) {
                            continue;
                        }
                        let q = p - n * d;
                        if (1..v.len() - 1).any(|k| in_triangle(q, v[0], v[k], v[k + 1])) {
                            best = (d.abs(), *surf);
                        }
                    }
                }
            }
        }
        best.1
    }
}

/// Whether `p`, on the plane of triangle `abc`, lies inside it (winding independent).
fn in_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> bool {
    let (v0, v1, v2) = (b - a, c - a, p - a);
    let (d00, d01, d11) = (v0.dot(v0), v0.dot(v1), v1.dot(v1));
    let (d20, d21) = (v2.dot(v0), v2.dot(v1));
    let den = d00 * d11 - d01 * d01;
    if den.abs() < 1e-12 {
        return false;
    }
    let u = (d11 * d20 - d01 * d21) / den;
    let v = (d00 * d21 - d01 * d20) / den;
    const EPS: f32 = 1e-3;
    u >= -EPS && v >= -EPS && u + v <= 1.0 + EPS
}

/// `effect.xml` entry of a sound: audible range and whether it is plain 2D (types 1, 3, 4, 5).
#[derive(Clone, Copy)]
struct Reach {
    max: f32,
    flat: bool,
}

const DEFAULT: Reach = Reach {
    max: DEFAULT_REACH,
    flat: false,
};

/// Sound stem -> VFS path, per-stem `effect.xml` data, and the decode-checked asset cache.
#[derive(Resource)]
struct Sounds {
    index: HashMap<String, String>,
    reach: HashMap<String, Reach>,
    cache: HashMap<String, Option<Handle<AudioSource>>>,
    surfaces: Surfaces,
    seed: u32,
}

impl Sounds {
    fn exists(&self, stem: &str) -> bool {
        self.index.contains_key(stem)
    }

    /// `<prefix>_mt_<surface>` with fallbacks to a surface the files cover.
    fn surfaced(&self, prefix: &str, surf: Surf) -> String {
        let name = |s: &str| format!("{prefix}_mt_{s}");
        surf.suffixes()
            .iter()
            .map(|s| name(s))
            .find(|n| self.exists(n))
            .unwrap_or_else(|| name("con"))
    }

    fn rand(&mut self, n: usize) -> usize {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 17;
        self.seed ^= self.seed << 5;
        self.seed as usize % n
    }

    /// The decoded-on-first-use asset of `stem`; `None` (warned once) if the file is missing or
    /// not a format bevy decodes.
    fn handle(
        &mut self,
        vfs: &Vfs,
        assets: &mut Assets<AudioSource>,
        stem: &str,
    ) -> Option<Handle<AudioSource>> {
        if let Some(h) = self.cache.get(stem) {
            return h.clone();
        }
        let h = self
            .index
            .get(stem)
            .and_then(|p| vfs.read(p).ok())
            .filter(|b| playable(b))
            .map(|b| assets.add(AudioSource { bytes: b.into() }));
        if h.is_none() {
            warn!("audio: no playable sound {stem}");
        }
        self.cache.insert(stem.to_owned(), h.clone());
        h
    }
}

/// bevy's decoder panics on formats it cannot read, so only PCM WAV and Ogg get through.
fn playable(bytes: &[u8]) -> bool {
    if bytes.starts_with(b"OggS") {
        return true;
    }
    if !(bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WAVE")) {
        return false;
    }
    let mut at = 12;
    while let Some(h) = bytes.get(at..at + 8) {
        let size = u32::from_le_bytes(h[4..].try_into().unwrap()) as usize;
        if &h[..4] == b"fmt " {
            return bytes.get(at + 8..at + 10) == Some(&[1, 0]);
        }
        at += 8 + size + (size & 1);
    }
    false
}

/// A looping map ambience (`AMBIENTSOUNDLIST`): a sphere (`type` b*, 3D at the centre) or a box
/// (`type` a*, 2D while the listener is inside).
struct Ambient {
    stem: String,
    centre: Vec3,
    /// Sphere radius, or the box half extent.
    extent: Vec3,
    sphere: bool,
    live: Option<Entity>,
}

#[derive(Resource)]
struct Ambients(Vec<Ambient>);

#[derive(Component)]
struct Expires(f32);

/// Shell casings and other sounds due later: (due `Time::elapsed_secs`, stem, at).
#[derive(Resource, Default)]
struct Later(Vec<(f32, String, Vec3)>);

fn vec3(text: Option<&str>) -> Option<Vec3> {
    let mut it = text?.split_whitespace().map(|v| v.parse::<f32>());
    let v = [it.next()?.ok()?, it.next()?.ok()?, it.next()?.ok()?];
    Some(Vec3::from(to_bevy(v)) * SCALE)
}

/// `AMBIENTSOUNDLIST` of the map's `.rs.xml`.
fn ambients(vfs: &Vfs, dir: &str) -> Vec<Ambient> {
    let Some(path) = vfs
        .paths()
        .find(|p| p.starts_with(dir) && p.ends_with(".rs.xml"))
    else {
        return Vec::new();
    };
    let text = String::from_utf8_lossy(&vfs.read(path).unwrap_or_default()).into_owned();
    let Ok(doc) = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')) else {
        return Vec::new();
    };
    let child = |n: roxmltree::Node, tag: &str| {
        n.children()
            .find(|c| c.has_tag_name(tag))
            .and_then(|c| c.text())
            .map(str::to_owned)
    };
    doc.descendants()
        .filter(|n| n.has_tag_name("AMBIENTSOUND"))
        .filter_map(|n| {
            let stem = n.attribute("filename")?.to_ascii_lowercase();
            if let (Some(c), Some(r)) = (vec3(child(n, "CENTER").as_deref()), child(n, "RADIUS")) {
                Some(Ambient {
                    stem,
                    centre: c,
                    extent: Vec3::splat(r.trim().parse::<f32>().ok()? * SCALE),
                    sphere: true,
                    live: None,
                })
            } else {
                // GunZ-space corners may swap under the axis flip: order them per axis.
                let a = vec3(child(n, "MIN_POSITION").as_deref())?;
                let b = vec3(child(n, "MAX_POSITION").as_deref())?;
                Some(Ambient {
                    stem,
                    centre: (a + b) * 0.5,
                    extent: (a - b).abs() * 0.5,
                    sphere: false,
                    live: None,
                })
            }
        })
        .collect()
}

fn fire_sound(item: &Item) -> Option<&str> {
    let w = item.weapon.as_ref()?;
    w.snd_fire
        .as_deref()
        .or((item.kind == "melee").then_some("blade_swing"))
}

/// Stems in the surface families (`man_fs_l`, `man_jump`, `fx_bullethit`, `fx_slugdrop`).
const SURFACE_FAMILIES: [&str; 5] = [
    "man_fs_l",
    "man_fs_r",
    "man_jump",
    "fx_bullethit",
    "fx_slugdrop",
];
const SURFACE_FILES: [&str; 10] = [
    "con", "drt", "met", "wod", "pnt", "snd", "snw", "wat", "gls", "fsh",
];
const VOICES: [&str; 13] = [
    "mal_shot_01",
    "mal_shot_02",
    "mal_shot_03",
    "mal_shot_04",
    "fem_shot_01",
    "fem_shot_02",
    "fem_shot_03",
    "fem_shot_04",
    "mal06",
    "mal07",
    "fem06",
    "fem07",
    "death01_a_male",
];
const MISC: [&str; 16] = [
    "blade_swing",
    "blade_damage",
    "blade_concrete",
    "fx_bladeon_concrete",
    "fx_guard",
    "uppercut",
    "hangonwall",
    "fx_dash",
    "hitbody00",
    "fx_myhit",
    "fx_respawn",
    "we_weapon_rdy",
    "fx_explosion01",
    "we_grenade_explosion",
    "we_flashbang_explosion",
    PICKUP_SOUND,
];

fn load(
    mut commands: Commands,
    level: Res<Level>,
    data: Res<ActorData>,
    mut assets: ResMut<Assets<AudioSource>>,
) {
    let vfs = &level.vfs;
    let mut index = HashMap::new();
    for p in vfs.paths().filter(|p| p.starts_with("sound/")) {
        let Some((stem, ext)) = p.rsplit('/').next().and_then(|f| f.rsplit_once('.')) else {
            continue;
        };
        if matches!(ext, "wav" | "ogg")
            && (!index.contains_key(stem) || p.starts_with("sound/effect/"))
        {
            index.insert(stem.to_owned(), p.to_owned());
        }
    }
    // zitem 300012 has `snd_fire="swing"` and no `swing` file exists; every other melee item
    // (and `animationevent.xml`'s `melee_attack`) uses `blade_swing`, so it is the same
    // sound (**inferred**).
    if let Some(p) = index.get("blade_swing").cloned() {
        index.insert("swing".to_owned(), p);
    }
    let reach = vfs
        .read("sound/effect/effect.xml")
        .ok()
        .map(|b| String::from_utf8_lossy(&b).into_owned())
        .and_then(|t| {
            let doc = roxmltree::Document::parse(t.trim_start_matches('\u{feff}')).ok()?;
            Some(
                doc.descendants()
                    .filter(|n| n.has_tag_name("EFFECT"))
                    .filter_map(|n| {
                        let max = n
                            .attribute("MAXDISTANCE")
                            .and_then(|v| v.parse::<f32>().ok());
                        let flat = matches!(n.attribute("type"), Some("1" | "3" | "4" | "5"));
                        Some((
                            n.attribute("NAME")?.to_ascii_lowercase(),
                            Reach {
                                max: max.map_or(DEFAULT_REACH, |m| m * SCALE),
                                flat,
                            },
                        ))
                    })
                    .collect::<HashMap<_, _>>(),
            )
        })
        .unwrap_or_default();
    let amb = ambients(vfs, &level.map.dir);
    let mut sounds = Sounds {
        index,
        reach,
        cache: HashMap::new(),
        surfaces: Surfaces::new(&level.map),
        seed: 0x9E37_79B9,
    };

    // Decode-check and preload every cue so no first shot pays for the zip read, and count
    // how many cues resolve.
    let mut groups: Vec<(&str, Vec<String>)> = vec![
        (
            "surface sets (footstep L/R, jump, bullet hit, shell drop)",
            SURFACE_FAMILIES
                .iter()
                .flat_map(|f| SURFACE_FILES.iter().map(move |s| format!("{f}_mt_{s}")))
                .collect(),
        ),
        ("voices", VOICES.map(String::from).to_vec()),
        ("misc cues", MISC.map(String::from).to_vec()),
        (
            "zitem snd_fire/snd_reload/snd_dryfire",
            data.items
                .weapons()
                .flat_map(|i| {
                    let w = i.weapon.as_ref().unwrap();
                    [
                        fire_sound(i),
                        w.snd_reload.as_deref(),
                        w.snd_dryfire.as_deref(),
                    ]
                })
                .flatten()
                .map(str::to_owned)
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
        (
            "map ambience",
            amb.iter()
                .map(|a| a.stem.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect(),
        ),
    ];
    // The surface families only have a subset of the ten suffixes: those are not cues.
    groups[0].1.retain(|s| sounds.exists(s));
    for (name, stems) in &groups {
        let ok = stems
            .iter()
            .filter(|s| sounds.handle(vfs, &mut assets, s).is_some())
            .count();
        info!("audio: {name}: {ok}/{} resolve and decode", stems.len());
    }
    let weapons = data.items.weapons().count();
    let with_fire = data
        .items
        .weapons()
        .filter(|i| fire_sound(i).is_some_and(|n| sounds.exists(n)))
        .count();
    info!(
        "audio: {} sound files; {} map ambiences; weapon items with a resolving fire sound {with_fire}/{weapons}; {} non-concrete polygons indexed",
        sounds.index.len(),
        amb.len(),
        sounds.surfaces.polys.len()
    );
    commands.insert_resource(Ambients(amb));
    commands.insert_resource(sounds);
}

/// Plays sounds on behalf of systems: one entity per play.
#[derive(bevy::ecs::system::SystemParam)]
struct Sfx<'w, 's> {
    commands: Commands<'w, 's>,
    level: Res<'w, Level>,
    time: Res<'w, Time>,
    sounds: ResMut<'w, Sounds>,
    assets: ResMut<'w, Assets<AudioSource>>,
    listener: Query<'w, 's, &'static GlobalTransform, With<Camera3d>>,
    live: Query<'w, 's, (), With<Expires>>,
    shot: Option<Res<'w, Shot>>,
}

impl Sfx<'_, '_> {
    /// `name` is a sound stem, or the `dir/stem` form `animationevent.xml` uses (the directory
    /// is dropped: stems are unique across `sound/`). `local` sounds (the player's own) are 2D
    /// and prefer the `_2d` variant; the rest are positioned at `at`.
    fn play(&mut self, name: &str, local: bool, at: Vec3) {
        let mut stem = name.rsplit('/').next().unwrap_or(name).to_ascii_lowercase();
        if local && self.sounds.exists(&format!("{stem}_2d")) {
            stem.push_str("_2d");
        }
        let Some(h) = self.sounds.handle(&self.level.vfs, &mut self.assets, &stem) else {
            return;
        };
        let reach = self.sounds.reach.get(&stem).copied().unwrap_or(DEFAULT);
        let spatial = !local && !reach.flat;
        if spatial
            && self
                .listener
                .single()
                .is_ok_and(|l| l.translation().distance(at) > reach.max)
        {
            return;
        }
        // Headless `--shot` runs stay silent (a host audio device would play them).
        if self.shot.is_some() {
            info!("sfx {stem}{}", if spatial { "" } else { " (2D)" });
            return;
        }
        if self.live.iter().count() >= MAX_LIVE {
            return;
        }
        self.commands.spawn((
            AudioPlayer::new(h),
            PlaybackSettings::DESPAWN.with_spatial(spatial),
            Transform::from_translation(at),
            Expires(self.time.elapsed_secs() + SOUND_LIFE),
        ));
    }

    fn surface(&self, p: Vec3, facing: Option<Vec3>) -> Surf {
        self.sounds.surfaces.at(p, facing)
    }

    fn surfaced(&self, prefix: &str, surf: Surf) -> String {
        self.sounds.surfaced(prefix, surf)
    }
}

fn reap(mut commands: Commands, time: Res<Time>, q: Query<(Entity, &Expires)>) {
    for (e, x) in &q {
        if x.0 < time.elapsed_secs() {
            commands.entity(e).despawn();
        }
    }
}

/// Voice stem prefix of an actor.
fn voice(woman: bool) -> &'static str {
    if woman { "fem" } else { "mal" }
}

#[allow(clippy::too_many_arguments)]
fn combat_sounds(
    data: Res<ActorData>,
    time: Res<Time>,
    mut fire: MessageReader<Fire>,
    mut damage: MessageReader<Damage>,
    mut killed: MessageReader<Killed>,
    mut blocked: MessageReader<Blocked>,
    mut impacts: MessageReader<Impact>,
    mut blasts: MessageReader<Blast>,
    mut later: ResMut<Later>,
    transforms: Query<&GlobalTransform>,
    players: Query<(), With<Player>>,
    loadouts: Query<&Loadout>,
    actors: Query<&Actor>,
    mut sfx: Sfx,
) {
    let woman = |e| actors.get(e).is_ok_and(|a| a.woman);
    // Actor roots stand at the feet; sounds come from the chest.
    let at = |e: Entity| {
        transforms
            .get(e)
            .map_or(Vec3::ZERO, |t| t.translation() + Vec3::Y)
    };
    let feet = |e: Entity| transforms.get(e).map_or(Vec3::ZERO, |t| t.translation());
    for f in fire.read() {
        let Some(item) = data.items.get(f.item) else {
            continue;
        };
        let (local, here) = (players.contains(f.shooter), at(f.shooter));
        if let Some(name) = fire_sound(item) {
            sfx.play(name, local, here);
        }
        if item.kind == "melee" {
            // A short shout on about every other swing (**inferred**: `mal06/07`, `fem06/07`
            // are the 0.3-0.6 s voice clips).
            if sfx.sounds.rand(2) == 0 {
                let n = 6 + sfx.sounds.rand(2);
                sfx.play(&format!("{}0{n}", voice(woman(f.shooter))), local, here);
            }
        } else if item.weapon.as_ref().is_some_and(|w| w.slug_output) {
            let stem = sfx.surfaced("fx_slugdrop", sfx.surface(feet(f.shooter), None));
            later
                .0
                .push((time.elapsed_secs() + SLUG_DELAY, stem, feet(f.shooter)));
        }
    }
    // Several pellets of one shot hit the same actor in one frame: one pain/impact sound each.
    let dead: Vec<Entity> = killed.read().map(|k| k.victim).collect();
    let mut seen: Vec<Entity> = Vec::new();
    for d in damage.read() {
        if seen.contains(&d.target) {
            continue;
        }
        seen.push(d.target);
        let melee = loadouts.get(d.attacker).ok().is_some_and(|l| {
            data.items
                .get(l.slots[l.current].item)
                .is_some_and(|i| i.kind == "melee")
        });
        sfx.play(
            if melee { "blade_damage" } else { "hitbody00" },
            false,
            d.point,
        );
        if players.contains(d.attacker) && d.attacker != d.target {
            sfx.play("fx_myhit", true, d.point);
        }
        if !dead.contains(&d.target) {
            // Pain grunt, longer for harder hits (**inferred**: `*_shot_01..04` are 0.3-0.8 s).
            let tier = match d.amount {
                a if a < 10.0 => 1,
                a if a < 20.0 => 2,
                a if a < 35.0 => 3,
                _ => 4,
            };
            let stem = format!("{}_shot_0{tier}", voice(woman(d.target)));
            sfx.play(&stem, players.contains(d.target), at(d.target));
        }
    }
    for k in &dead {
        // No female death clip exists; her longest pain clip stands in (**inferred**).
        let stem = if woman(*k) {
            "fem_shot_04"
        } else {
            "death01_a_male"
        };
        sfx.play(stem, players.contains(*k), at(*k));
    }
    for b in blocked.read() {
        sfx.play("fx_guard", players.contains(b.0), at(b.0));
    }
    for (n, i) in impacts.read().enumerate() {
        if n >= MAX_IMPACTS {
            break;
        }
        if i.blade {
            let stem = ["blade_concrete", "fx_bladeon_concrete"][sfx.sounds.rand(2)];
            sfx.play(stem, false, i.point);
        } else {
            let surf = sfx.surface(i.point, Some(i.normal));
            let stem = sfx.surfaced("fx_bullethit", surf);
            sfx.play(&stem, false, i.point);
        }
    }
    for b in blasts.read() {
        sfx.play(b.sound, false, b.at);
    }
}

#[allow(clippy::too_many_arguments)]
fn actor_sounds(
    data: Res<ActorData>,
    mut cues: MessageReader<ActorSound>,
    mut picked: MessageReader<Picked>,
    mut respawned: RemovedComponents<Dead>,
    transforms: Query<&GlobalTransform>,
    players: Query<(), With<Player>>,
    player: Query<(&Loadout, Has<Dead>), With<Player>>,
    mut current: Local<Option<usize>>,
    mut sfx: Sfx,
) {
    let at = |e: Entity| {
        transforms
            .get(e)
            .map_or(Vec3::ZERO, |t| t.translation() + Vec3::Y)
    };
    for c in cues.read() {
        let local = players.contains(c.actor);
        let item = |id| data.items.get(id).and_then(|i| i.weapon.as_ref());
        let feet = at(c.actor) - Vec3::Y;
        let name = match &c.cue {
            Cue::Footstep { left } => {
                let surf = sfx.surface(feet, None);
                Some(sfx.surfaced(if *left { "man_fs_l" } else { "man_fs_r" }, surf))
            }
            Cue::Reload { item: i } => item(*i).and_then(|w| w.snd_reload.clone()),
            Cue::DryFire { item: i } => item(*i).and_then(|w| w.snd_dryfire.clone()),
            // Animation sounds with surface files (`man_jump`) use the floor under the actor.
            Cue::Anim(s)
                if sfx
                    .sounds
                    .exists(&format!("{}_mt_con", s.to_ascii_lowercase())) =>
            {
                let surf = sfx.surface(feet, None);
                Some(sfx.surfaced(&s.to_ascii_lowercase(), surf))
            }
            Cue::Anim(s) => Some(s.clone()),
        };
        if let Some(name) = name {
            sfx.play(&name, local, at(c.actor));
        }
    }
    for p in picked.read() {
        sfx.play(PICKUP_SOUND, players.contains(p.actor), p.at);
    }
    for e in respawned.read() {
        if players.contains(e) {
            sfx.play("fx_respawn", true, Vec3::ZERO);
        }
    }
    // Weapon switch: `we_weapon_rdy` (a short 2D click, **inferred** from the name).
    if let Ok((l, dead)) = player.single() {
        if current.is_some_and(|c| c != l.current) && !dead {
            sfx.play("we_weapon_rdy", true, Vec3::ZERO);
        }
        *current = Some(l.current);
    }
}

/// `PlaySound` requests from other modules (NPC/quest sounds, animation events).
fn requested(mut req: MessageReader<PlaySound>, mut sfx: Sfx) {
    for r in req.read() {
        sfx.play(&r.stem, false, r.at);
    }
}

fn later(mut later: ResMut<Later>, time: Res<Time>, mut sfx: Sfx) {
    let now = time.elapsed_secs();
    let due: Vec<_> = later.0.extract_if(.., |(t, ..)| *t <= now).collect();
    for (_, stem, at) in due {
        sfx.play(&stem, false, at);
    }
}

/// Starts and stops the looping ambience near the listener.
fn ambient(
    mut commands: Commands,
    mut amb: ResMut<Ambients>,
    mut sounds: ResMut<Sounds>,
    mut assets: ResMut<Assets<AudioSource>>,
    level: Res<Level>,
    listener: Query<&GlobalTransform, With<Camera3d>>,
    shot: Option<Res<Shot>>,
) {
    let Ok(l) = listener.single() else {
        return;
    };
    let p = l.translation();
    let near = |a: &Ambient| {
        let d = (p - a.centre).abs();
        if a.sphere {
            (p.distance(a.centre) < a.extent.x).then_some(p.distance(a.centre))
        } else {
            (d.x < a.extent.x && d.y < a.extent.y && d.z < a.extent.z).then_some(0.0)
        }
    };
    let mut want: Vec<(usize, f32)> = amb
        .0
        .iter()
        .enumerate()
        .filter_map(|(i, a)| near(a).map(|d| (i, d)))
        .collect();
    want.sort_by(|a, b| a.1.total_cmp(&b.1));
    want.truncate(MAX_AMBIENT);
    for (i, a) in amb.0.iter_mut().enumerate() {
        let on = want.iter().any(|w| w.0 == i);
        match (on, a.live) {
            (true, None) => {
                if let Some(h) = sounds.handle(&level.vfs, &mut assets, &a.stem) {
                    let flat = sounds.reach.get(&a.stem).is_some_and(|r| r.flat) || !a.sphere;
                    a.live = Some(if shot.is_some() {
                        // Headless `--shot` runs stay silent: only log what would loop.
                        info!(
                            "ambient on: {} ({})",
                            a.stem,
                            if flat { "2D" } else { "3D" }
                        );
                        Entity::PLACEHOLDER
                    } else {
                        commands
                            .spawn((
                                AudioPlayer::new(h),
                                PlaybackSettings::LOOP.with_spatial(!flat),
                                Transform::from_translation(a.centre),
                            ))
                            .id()
                    });
                }
            }
            (false, Some(e)) => {
                if shot.is_none() {
                    commands.entity(e).despawn();
                } else {
                    info!("ambient off: {}", a.stem);
                }
                a.live = None;
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn material_suffix_and_polygon_lookup() {
        assert_eq!(Surf::of("gzd_map_Mansion_carpet00_mt_drt"), Surf::Drt);
        assert_eq!(Surf::of("gzd_map_x_chain01_2s_mt_met"), Surf::Met);
        assert_eq!(Surf::of("gzd_map_Mansion_floor05_mt_fcon"), Surf::Con);
        assert_eq!(Surf::of("gzd_map_Mansion_curtain01_2s_mt_clo"), Surf::Drt);
        assert_eq!(Surf::of("no_suffix"), Surf::Con);
        // A 2x2 wooden quad on y = 1 (a fan of 4 vertices).
        let quad = [
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(2.0, 1.0, 2.0),
            Vec3::new(0.0, 1.0, 2.0),
        ];
        let mut s = Surfaces {
            cells: HashMap::new(),
            polys: vec![(Surf::Wod, quad.to_vec())],
        };
        s.cells.entry([0, 0, 0]).or_default().push(0);
        s.cells.entry([0, 1, 0]).or_default().push(0);
        assert_eq!(s.at(Vec3::new(1.5, 1.05, 1.5), None), Surf::Wod);
        assert_eq!(
            s.at(Vec3::new(1.5, 1.5, 1.5), None),
            Surf::Con,
            "above tolerance"
        );
        assert_eq!(
            s.at(Vec3::new(2.5, 1.0, 1.0), None),
            Surf::Con,
            "beside the quad"
        );
        assert_eq!(
            s.at(Vec3::new(1.0, 1.0, 1.0), Some(Vec3::X)),
            Surf::Con,
            "wrong facing"
        );
    }
}
