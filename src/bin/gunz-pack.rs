//! `gunz-pack [GAME_DIR] OUT_DIR [MAP..]`: the game data of the browser build (`web/`), cut to
//! what a match needs. It plays a short headless match on every map (or the ones named) with
//! `GUNZ_TRACE` set (`mrs.rs`), plus one match of every other mode the web menu offers and the
//! game's own menu (its PLAYER page), then writes what they read:
//! - `core.pack.gz`: every file not tied to a map (characters, weapons, effects, sounds, ...),
//! - `maps/<map>.pack.gz`: the files under `maps/` and the music that map's match read,
//! - `files/<path>.gz`: every other file under `model/` on its own (the clothes and their
//!   textures, gzip), downloaded by the page when the game reads one (`mrs::Packed::Fetch`),
//! - `index.json`: the maps and the packs' sizes and CRCs (the page caches packs by CRC).
//!
//! A pack is `mrs::pack` compressed with gzip. Packs hold retail bytes: keep them on your
//! machine (`web/dist/` is ignored by git).

use gunz::mrs::{Packed, Vfs, pack};
use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::{Path, PathBuf},
    process::{Command, ExitCode},
};

/// The pseudo mode of the traced menu run.
const MENU: &str = "menu";

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
    let maps: Vec<String> = if wanted.is_empty() {
        vfs.maps()
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
    runs.push((first.clone(), MENU));
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
        // the menu plays its own music: everything it reads is shared
        core.extend(t.iter().filter(|p| *mode == MENU || !map_file(p)).cloned());
        if *mode == MODES[0] {
            packs.push((map.clone(), t.into_iter().filter(|p| map_file(p)).collect()));
        }
    }
    fs::create_dir_all(out.join("maps"))?;
    // Every other file of the install goes in as a name only, so existence checks see the
    // whole install; the other model files are served on their own.
    let fetch: BTreeSet<String> = vfs
        .paths()
        .filter(|p| p.starts_with("model/") && !core.contains(*p))
        .map(str::to_owned)
        .collect();
    let names: BTreeSet<String> = vfs
        .paths()
        .filter(|p| !core.contains(*p) && !fetch.contains(*p))
        .map(str::to_owned)
        .collect();
    let (core_bytes, core_crc) = write(&vfs, &core, &names, &fetch, &out.join("core.pack.gz"))?;
    println!(
        "core: {} files ({} names, {} served on their own), {} kB",
        core.len(),
        names.len(),
        fetch.len(),
        core_bytes / 1024
    );
    let written = export(&vfs, &fetch, &out.join("files"))?;
    println!("files/: {written} new");
    let mut index = format!(
        "{{\n  \"core\": {{\"file\": \"core.pack.gz\", \"bytes\": {core_bytes}, \"crc\": {core_crc}}},\n  \"maps\": ["
    );
    for (i, (map, files)) in packs.iter().enumerate() {
        let file = format!("maps/{map}.pack.gz");
        let (bytes, crc) = write(
            &vfs,
            files,
            &BTreeSet::new(),
            &BTreeSet::new(),
            &out.join(&file),
        )?;
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
    let mut cmd = Command::new(exe);
    if *mode == MENU {
        cmd.args([game, "--menu-page", "player", "--shot"]);
    } else {
        cmd.args([game, map, "--mode", mode, "--bots", "3", "--time", "6"])
            .args(["--die-at", "3", "--script", SCRIPT, "--shot"]);
    }
    let ok = cmd
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

/// Packs the readable files of `paths`, the bare `names` and the `fetch` names (served on their
/// own), gzips the pack to `to`; returns its size and CRC-32.
fn write(
    vfs: &Vfs,
    paths: &BTreeSet<String>,
    names: &BTreeSet<String>,
    fetch: &BTreeSet<String>,
    to: &Path,
) -> io::Result<(usize, u32)> {
    let files: Vec<(String, Packed)> = paths
        .iter()
        .filter_map(|p| Some((p.clone(), Packed::Bytes(vfs.read(p).ok()?))))
        .chain(names.iter().map(|n| (n.clone(), Packed::NameOnly)))
        .chain(fetch.iter().map(|n| (n.clone(), Packed::Fetch)))
        .collect();
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::best());
    gz.write_all(&pack(&files))?;
    let bytes = gz.finish()?;
    let mut crc = flate2::Crc::new();
    crc.update(&bytes);
    fs::write(to, &bytes)?;
    Ok((bytes.len(), crc.sum()))
}

/// Writes each of `paths` gzipped to `dir/<path>.gz` (on all cores); files already there are
/// kept (the install's files do not change). Returns how many were written.
fn export(vfs: &Vfs, paths: &BTreeSet<String>, dir: &Path) -> io::Result<usize> {
    let todo: Vec<&String> = paths
        .iter()
        .filter(|p| !dir.join(format!("{p}.gz")).exists())
        .collect();
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        let jobs: Vec<_> = todo
            .chunks(todo.len().div_ceil(threads).max(1))
            .map(|chunk| {
                s.spawn(move || -> io::Result<()> {
                    for p in chunk {
                        let to = dir.join(format!("{p}.gz"));
                        fs::create_dir_all(to.parent().unwrap())?;
                        let mut gz = flate2::write::GzEncoder::new(
                            Vec::new(),
                            flate2::Compression::default(),
                        );
                        gz.write_all(&vfs.read(p)?)?;
                        // a half-written file would be kept by the next run: rename into place
                        let tmp = to.with_extension("tmp");
                        fs::write(&tmp, gz.finish()?)?;
                        fs::rename(&tmp, &to)?;
                    }
                    Ok(())
                })
            })
            .collect();
        jobs.into_iter()
            .try_for_each(|j| j.join().expect("export thread panicked"))
    })?;
    Ok(todo.len())
}
