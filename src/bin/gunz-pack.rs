//! `gunz-pack [GAME_DIR] OUT_DIR [MAP..]`: the game data of the browser build (`web/`), cut to
//! what a match needs. It plays a short headless match on every map (or the ones named) with
//! `GUNZ_TRACE` set (`mrs.rs`), plus one match of every other mode the web menu offers, then
//! writes what they read:
//! - `core.pack.gz`: every file not tied to a map (characters, weapons, effects, sounds, ...),
//! - `maps/<map>.pack.gz`: the files under `maps/` and the music that map's match read,
//! - `index.json`: the maps and the packs' sizes and CRCs (the page caches packs by CRC).
//!
//! A pack is `mrs::pack` compressed with gzip. Packs hold retail bytes: keep them on your
//! machine (`web/data/` is ignored by git).

use gunz::mrs::{Vfs, pack};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// The traced player fires every default weapon (melee, revolver, rifle) and dies at 3 s (the
/// killcam and the damage report load then).
const SCRIPT: &str = "attack:0.3;2:0.1;attack:0.3;3:0.1;attack:0.3;1:0.1;attack:0.3";
/// The modes of the web menu; the first is traced on every map, the rest once.
const MODES: [&str; 8] = [
    "dm",
    "tdm",
    "gladiator",
    "team-gladiator",
    "elimination",
    "assassinate",
    "duel",
    "berserker",
];

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let game = match args.first() {
        Some(a) if args.len() >= 2 && Path::new(a).join("system.mrs").exists() => args.remove(0),
        _ => match gunz::steam::find_game() {
            Some(d) => d.to_string_lossy().into_owned(),
            None => {
                eprintln!("usage: gunz-pack [GAME_DIR] OUT_DIR [MAP..] (no Steam install found)");
                return ExitCode::from(2);
            }
        },
    };
    if args.is_empty() {
        eprintln!("usage: gunz-pack [GAME_DIR] OUT_DIR [MAP..]");
        return ExitCode::from(2);
    }
    let out = PathBuf::from(args.remove(0));
    match run(&game, &out, &args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(game: &str, out: &Path, wanted: &[String]) -> io::Result<()> {
    let vfs = Vfs::mount(game)?;
    let all: BTreeSet<String> = vfs
        .paths()
        .filter_map(|p| {
            let (dir, file) = p.strip_prefix("maps/")?.split_once('/')?;
            (file.ends_with(".rs") && !file.contains('/')).then(|| dir.to_owned())
        })
        .collect();
    let maps: Vec<String> = if wanted.is_empty() {
        all.into_iter().collect()
    } else {
        wanted.iter().map(|m| m.to_ascii_lowercase()).collect()
    };
    let Some(first) = maps.first().cloned() else {
        return Err(io::Error::other("no maps"));
    };
    let tmp = out.join("trace");
    fs::create_dir_all(&tmp)?;
    let mut runs: Vec<(String, &str)> = maps.iter().map(|m| (m.clone(), MODES[0])).collect();
    runs.extend(MODES[1..].iter().map(|&mode| (first.clone(), mode)));
    let exe = std::env::current_exe()?
        .with_file_name(format!("gunz-play{}", std::env::consts::EXE_SUFFIX));
    let traces: Vec<Option<BTreeSet<String>>> = {
        let threads = std::thread::available_parallelism().map_or(4, |n| n.get().min(6));
        let chunk = runs.len().div_ceil(threads);
        std::thread::scope(|s| {
            let jobs: Vec<_> = runs
                .chunks(chunk)
                .map(|c| {
                    s.spawn(|| {
                        c.iter()
                            .map(|r| trace(&exe, game, &tmp, r))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            jobs.into_iter().flat_map(|j| j.join().unwrap()).collect()
        })
    };
    let map_file = |p: &str| p.starts_with("maps/") || p.starts_with("sound/bgm/");
    let mut core = BTreeSet::new();
    let mut packs = Vec::new();
    for ((map, mode), t) in runs.iter().zip(traces) {
        let Some(t) = t else {
            eprintln!("{map} ({mode}): the traced match failed, skipped");
            continue;
        };
        core.extend(t.iter().filter(|p| !map_file(p)).cloned());
        if *mode == MODES[0] {
            packs.push((map.clone(), t.into_iter().filter(|p| map_file(p)).collect()));
        }
    }
    fs::create_dir_all(out.join("maps"))?;
    // Every other file of the install goes in as a name only: listings (maps, music) and
    // existence checks see the whole install.
    let names: BTreeSet<String> = vfs
        .paths()
        .filter(|p| !core.contains(*p))
        .map(str::to_owned)
        .collect();
    let (core_bytes, core_crc) = write(&vfs, &core, &names, &out.join("core.pack.gz"))?;
    println!(
        "core: {} files ({} names), {} kB",
        core.len(),
        names.len(),
        core_bytes / 1024
    );
    let mut index = format!(
        "{{\n  \"core\": {{\"file\": \"core.pack.gz\", \"bytes\": {core_bytes}, \"crc\": {core_crc}}},\n  \"maps\": ["
    );
    for (i, (map, files)) in packs.iter().enumerate() {
        let file = format!("maps/{map}.pack.gz");
        let (bytes, crc) = write(&vfs, files, &BTreeSet::new(), &out.join(&file))?;
        println!("{map}: {} files, {} kB", files.len(), bytes / 1024);
        index += &format!(
            "{}\n    {{\"name\": {map:?}, \"file\": {file:?}, \"bytes\": {bytes}, \"crc\": {crc}}}",
            if i == 0 { "" } else { "," }
        );
    }
    index += "\n  ]\n}\n";
    fs::write(out.join("index.json"), index)?;
    fs::remove_dir_all(&tmp)
}

/// Plays one traced headless match; `None` if it failed.
fn trace(
    exe: &Path,
    game: &str,
    tmp: &Path,
    (map, mode): &(String, &str),
) -> Option<BTreeSet<String>> {
    let stem = tmp.join(format!("{map}-{mode}"));
    let list = stem.with_extension("txt");
    let _ = fs::remove_file(&list);
    let ok = Command::new(exe)
        .args([game, map, "--mode", mode, "--bots", "3", "--time", "6"])
        .args(["--die-at", "3", "--script", SCRIPT, "--shot"])
        .arg(stem.with_extension("png"))
        .env("GUNZ_TRACE", &list)
        .env("GUNZ_NOVSYNC", "1")
        .env("GUNZ_PROFILE", stem.with_extension("profile"))
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success());
    println!("traced {map} ({mode}){}", if ok { "" } else { ": FAILED" });
    ok.then(|| fs::read_to_string(&list).ok())
        .flatten()
        .map(|t| t.lines().map(str::to_owned).collect())
}

/// Packs the readable files of `paths` and the bare `names`, gzips the pack to `to`; returns its
/// size and CRC-32.
fn write(
    vfs: &Vfs,
    paths: &BTreeSet<String>,
    names: &BTreeSet<String>,
    to: &Path,
) -> io::Result<(usize, u32)> {
    let files: Vec<(String, Option<Vec<u8>>)> = paths
        .iter()
        .filter_map(|p| Some((p.clone(), Some(vfs.read(p).ok()?))))
        .chain(names.iter().map(|n| (n.clone(), None)))
        .collect();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&pack(&files))?;
    let bytes = gz.finish()?;
    let mut crc = flate2::Crc::new();
    crc.update(&bytes);
    fs::write(to, &bytes)?;
    Ok((bytes.len(), crc.sum()))
}
