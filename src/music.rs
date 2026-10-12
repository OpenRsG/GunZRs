//! Background music. The retail `sound/bgm/` files are listed in `system/filelist.xml` and
//! nothing else in the data (system/interface/map XMLs) names them, so there is no observed
//! track-to-map mapping, and no public source gives one either (`docs/formats.md`, "Music"); the
//! rule here is **inferred**: the main menu plays `gunzmatching`; a match plays one track from
//! the in-game pool (**external**: the ijji client's seven in-game tracks plus the later "duel
//! theme" ones; lobby and character-select tracks excluded) picked by a hash of the map
//! directory (the same map always gets the same track); Duel plays `leagueloop`; the
//! match-end screen crossfades into the `fin` stinger. A change of wanted track crossfades.
//! Headless `--shot` runs only log the choice. Loudness is the profile's `Knob::Music`; every
//! other sound plays at `Knob::Effects` ([`Narration`]: `Knob::Announcer`), see [`levels`].

use crate::{
    gfx::Knob,
    level::Level,
    menu::Mode,
    mrs::Vfs,
    phase::{MatchSystems, Persist},
    profile::Profile,
    session::{Clock, Rules},
    view::Shot,
};
use bevy::{
    audio::{Decodable, Source, Volume},
    prelude::*,
};
use std::{panic::AssertUnwindSafe, sync::Arc};

/// Seconds a crossfade takes (**inferred**).
const FADE: f32 = 2.0;
/// Menu/lobby track (**inferred** from the name "matching").
const MENU: &str = "gunzmatching";
/// Match-end stinger (**inferred**: the 130 kB `fin.ogg` is the only short track).
const STINGER: &str = "fin";
/// Duel track (**inferred** from the name; `league` is its non-looping twin).
const DUEL: &str = "leagueloop";
/// Quest/challenge-quest track (**inferred** from the name "mission").
const QUEST: &str = "trance mission_tmix";
/// Kept out of the per-map pool: they have a role above, or (`theme rock(d)` = lobby,
/// `intro retake2(d-r)` = character select) are not in-game tracks (**external**: the ijji
/// client's track roles in the Ragezone BGM tutorial and the YouTube soundtrack rip, see
/// `docs/formats.md`, "Music").
const RESERVED: [&str; 7] = [
    MENU,
    STINGER,
    "league",
    DUEL,
    QUEST,
    "theme rock(d)",
    "intro retake2(d-r)",
];
/// Samples decoded to accept a track before playing it: one second of stereo 48 kHz.
const PROBE: usize = 96_000;

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Want>()
            .add_observer(level_new)
            .add_systems(
                Update,
                (
                    choose.run_if(resource_exists::<Level>).in_set(MatchSystems),
                    play.run_if(resource_changed::<Want>),
                    fade,
                    levels.run_if(resource_exists_and_changed::<Profile>),
                )
                    .chain(),
            );
    }
}

/// The track that should be playing: `bytes` is `None` when it is missing or undecodable.
#[derive(Resource, Default)]
struct Want {
    stem: String,
    bytes: Option<Arc<[u8]>>,
    once: bool,
}

/// A playing (or fading) track: `level` follows `target` (0 or 1) over [`FADE`] seconds.
#[derive(Component)]
struct Voice {
    level: f32,
    target: f32,
}

/// The menu's track: there is no `Level` to choose from, so the menu hands over its archive
/// whenever it opens (`play` crossfades from whatever the last match played).
pub fn menu(world: &mut World, vfs: &Vfs) {
    world.insert_resource(Want::new(vfs, MENU, false));
}

impl Want {
    fn new(vfs: &Vfs, stem: &str, once: bool) -> Self {
        let bytes = bgm(vfs)
            .into_iter()
            .find(|(s, _)| s == stem)
            .and_then(|(_, p)| vfs.read(&p).ok())
            .map(Arc::<[u8]>::from)
            .filter(|b| match decode(b, Some(PROBE)) {
                Some((ch, hz, _)) => {
                    info!(
                        "music: {stem}: {} kB, {ch} ch, {hz} Hz, decodes",
                        b.len() / 1000
                    );
                    true
                }
                None => false,
            });
        if bytes.is_none() {
            warn!("music: no playable track {stem}");
        }
        Self {
            stem: stem.to_owned(),
            bytes,
            once,
        }
    }
}

/// `(stem, path)` of every `sound/bgm/` ogg/mp3, sorted by stem.
fn bgm(vfs: &Vfs) -> Vec<(String, String)> {
    let mut v: Vec<_> = vfs
        .paths()
        .filter_map(|p| {
            let (stem, ext) = p.strip_prefix("sound/bgm/")?.rsplit_once('.')?;
            matches!(ext, "ogg" | "mp3").then(|| (stem.to_owned(), p.to_owned()))
        })
        .collect();
    v.sort();
    v
}

/// The pool track for `map` (FNV-1a of its name, so it is stable across runs).
fn pick<'a>(pool: &'a [String], map: &str) -> Option<&'a str> {
    let h = map.bytes().fold(0x811c_9dc5u32, |h, b| {
        (h ^ u32::from(b)).wrapping_mul(0x0100_0193)
    });
    pool.get(h as usize % pool.len().max(1)).map(String::as_str)
}

/// Channels, sample rate and seconds decoded from the first `limit` samples (all when `None`);
/// `None` if bevy's decoder refuses the bytes (it panics, so the call is caught).
pub(crate) fn decode(bytes: &Arc<[u8]>, limit: Option<usize>) -> Option<(u16, u32, f32)> {
    let src = AudioSource {
        bytes: bytes.clone(),
    };
    std::panic::catch_unwind(AssertUnwindSafe(|| {
        let d = src.decoder();
        let (ch, hz) = (d.channels().get(), d.sample_rate().get());
        let n = match limit {
            Some(l) => d.take(l).count(),
            None => d.count(),
        };
        (ch, hz, n as f32 / (ch as f32 * hz as f32))
    }))
    .ok()
    .filter(|s| s.2 > 0.0)
}

/// In a match: the wanted track follows the map, the mode and the match end.
fn choose(
    level: Res<Level>,
    rules: Option<Res<Rules>>,
    clock: Option<Res<Clock>>,
    mut pool: Local<Vec<String>>,
    mut want: ResMut<Want>,
) {
    let over = clock.is_some_and(|c| c.over.is_some());
    if pool.is_empty() {
        *pool = bgm(&level.vfs)
            .into_iter()
            .map(|t| t.0)
            .filter(|s| !RESERVED.contains(&s.as_str()))
            .collect();
    }
    let stem = if over {
        STINGER
    } else if rules
        .as_ref()
        .is_some_and(|r| matches!(r.mode, Mode::Duel | Mode::DuelTournament | Mode::DynDuel))
    {
        DUEL
    } else if rules.is_some_and(|r| r.mode == Mode::Quest) {
        QUEST
    } else if let Some(s) = pick(&pool, &level.map.dir) {
        s
    } else {
        return;
    };
    if want.stem != stem {
        *want = Want::new(&level.vfs, stem, over);
    }
}

/// Starts the wanted track silent and sends everything already playing towards silence.
fn play(
    want: Res<Want>,
    mut commands: Commands,
    mut assets: ResMut<Assets<AudioSource>>,
    mut voices: Query<&mut Voice>,
    shot: Option<Res<Shot>>,
) {
    if want.stem.is_empty() {
        return;
    }
    info!(
        "music: {}{}",
        want.stem,
        if shot.is_some() {
            " (silent in --shot runs)"
        } else {
            ""
        }
    );
    for mut v in &mut voices {
        v.target = 0.0;
    }
    let (Some(bytes), None) = (&want.bytes, &shot) else {
        return;
    };
    let mode = if want.once {
        PlaybackSettings::DESPAWN
    } else {
        PlaybackSettings::LOOP
    };
    commands.spawn((
        Persist,
        Voice {
            level: 0.0,
            target: 1.0,
        },
        AudioPlayer::new(assets.add(AudioSource {
            bytes: bytes.clone(),
        })),
        mode.with_volume(Volume::SILENT),
    ));
}

/// Moves every voice towards its target and applies the music volume; silent ones go away.
/// Real time, so the end-of-match freeze (paused virtual time) does not stop the crossfade.
fn fade(
    time: Res<Time<Real>>,
    profile: Option<Res<Profile>>,
    mut commands: Commands,
    mut voices: Query<(Entity, &mut Voice, Option<&mut AudioSink>)>,
) {
    let loud = loudness(profile.as_deref(), Knob::Music);
    let step = time.delta_secs() / FADE;
    for (e, mut v, sink) in &mut voices {
        let to = v.target;
        v.level = if v.level < to {
            (v.level + step).min(to)
        } else {
            (v.level - step).max(to)
        };
        if let Some(mut s) = sink {
            s.set_volume(Volume::Linear(v.level * loud));
        }
        if to == 0.0 && v.level == 0.0 {
            commands.entity(e).despawn();
        }
    }
}

/// An announcer line: plays at `Knob::Announcer` instead of `Knob::Effects`.
#[derive(Component)]
pub struct Narration;

fn loudness(profile: Option<&Profile>, k: Knob) -> f32 {
    profile.map_or_else(
        || crate::gfx::Graphics::default().get(k),
        |p| p.graphics.get(k),
    )
}

fn effect_level(profile: Option<&Profile>, narration: bool) -> Volume {
    Volume::Linear(loudness(
        profile,
        if narration {
            Knob::Announcer
        } else {
            Knob::Effects
        },
    ))
}

/// A new sound (not music) starts at its slider's level.
fn level_new(
    add: On<Add, PlaybackSettings>,
    profile: Option<Res<Profile>>,
    mut sounds: Query<(&mut PlaybackSettings, Has<Narration>), Without<Voice>>,
) {
    if let Ok((mut s, n)) = sounds.get_mut(add.entity) {
        s.volume *= effect_level(profile.as_deref(), n);
    }
}

/// A moved slider reaches the sounds already playing (the ambience loops).
// ponytail: assumes every non-music sound spawns at volume 1 (all do); keep a base volume per
// sound if one ever spawns quieter.
fn levels(
    profile: Option<Res<Profile>>,
    mut flat: Query<(&mut AudioSink, Has<Narration>), Without<Voice>>,
    mut spatial: Query<(&mut SpatialAudioSink, Has<Narration>)>,
) {
    let p = profile.as_deref();
    for (mut s, n) in &mut flat {
        s.set_volume(effect_level(p, n));
    }
    for (mut s, n) in &mut spatial {
        s.set_volume(effect_level(p, n));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The external lobby/character-select tracks never come out of the per-map pick.
    #[test]
    fn lobby_tracks_are_not_in_game() {
        for s in ["theme rock(d)", "intro retake2(d-r)", "fin", "leagueloop"] {
            assert!(RESERVED.contains(&s), "{s}");
        }
    }

    /// New sounds start at the effects or announcer slider; music keeps its own volume.
    #[test]
    fn sounds_start_at_their_slider() {
        let mut world = World::new();
        let mut p = Profile::default();
        p.graphics.set(Knob::Effects, 0.25);
        p.graphics.set(Knob::Announcer, 0.0);
        world.insert_resource(p);
        world.add_observer(level_new);
        let fx = world.spawn(PlaybackSettings::DESPAWN).id();
        let nar = world.spawn((PlaybackSettings::DESPAWN, Narration)).id();
        let bgm = world
            .spawn((
                Voice {
                    level: 0.0,
                    target: 1.0,
                },
                PlaybackSettings::LOOP.with_volume(Volume::Linear(0.8)),
            ))
            .id();
        let v = |e| world.get::<PlaybackSettings>(e).unwrap().volume.to_linear();
        assert_eq!(v(fx), 0.25);
        assert_eq!(v(nar), 0.0);
        assert_eq!(v(bgm), 0.8);
    }

    #[test]
    fn pick_is_stable_and_in_pool() {
        let pool: Vec<String> = ["a", "b", "c", "d"].map(String::from).to_vec();
        let a = pick(&pool, "maps/mansion/");
        assert_eq!(a, pick(&pool, "maps/mansion/"));
        assert!(pool.iter().any(|p| Some(p.as_str()) == a));
        assert_eq!(pick(&[], "x"), None);
        // Different maps spread over the pool.
        let seen: std::collections::HashSet<_> =
            ["mansion", "dungeon", "prison", "lobby", "hall", "factory"]
                .iter()
                .filter_map(|m| pick(&pool, m))
                .collect();
        assert!(seen.len() > 1);
    }

    /// `GUNZ_GAME=<install dir> cargo test --release every_bgm -- --nocapture`: every retail
    /// track must decode completely through bevy's decoder (skipped without the variable).
    #[test]
    fn every_bgm_decodes() {
        let Some(vfs) = crate::mrs::retail() else {
            return;
        };
        let tracks = bgm(&vfs);
        assert!(!tracks.is_empty());
        for (stem, path) in tracks {
            let bytes: Arc<[u8]> = vfs.read(&path).unwrap().into();
            let (ch, hz, secs) =
                decode(&bytes, None).unwrap_or_else(|| panic!("{path} does not decode"));
            println!("{path}: {ch} ch, {hz} Hz, {secs:.1} s decoded");
            assert!(secs > 1.0 || stem == STINGER, "{path} too short");
        }
    }
}
