//! `mrs list ARCHIVE` | `mrs extract GAME_DIR OUT_DIR`
use gunz::mrs::{Archive, Vfs};
use std::{env, fs, path::Path, process::ExitCode};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        ["list", archive] => list(archive),
        ["extract", game, out] => extract(game, out),
        _ => {
            eprintln!("usage: mrs list ARCHIVE | mrs extract GAME_DIR OUT_DIR");
            return ExitCode::from(2);
        }
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

fn list(path: &str) -> std::io::Result<()> {
    for e in Archive::open(path)?.entries {
        println!("{:>10} {:>10} {}", e.size, e.compressed, e.name);
    }
    Ok(())
}

fn extract(game: &str, out: &str) -> std::io::Result<()> {
    let vfs = Vfs::mount(game)?;
    let mut paths: Vec<&str> = vfs.paths().collect();
    paths.sort_unstable();
    let mut bytes = 0usize;
    for p in &paths {
        let data = vfs.read(p)?;
        let dest = Path::new(out).join(p);
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::write(dest, &data)?;
        bytes += data.len();
    }
    println!(
        "{} archives, {} files, {bytes} bytes, all CRCs verified",
        vfs.archives.len(),
        paths.len()
    );
    Ok(())
}
