//! Finds the retail install without a GAME_DIR argument: Steam's `libraryfolders.vdf` lists the
//! libraries, `appmanifest_3139440.acf` names the install directory (*observed*: the local Flatpak
//! Steam has that manifest with `installdir` `GUNZ THE DUEL`).

use std::path::{Path, PathBuf};

const APP_ID: u32 = 3139440;

/// Steam installation roots (Linux native and Flatpak, Windows, macOS); missing ones are skipped.
fn steam_roots() -> Vec<PathBuf> {
    let mut roots = vec![
        PathBuf::from(r"C:\Program Files (x86)\Steam"),
        PathBuf::from(r"C:\Program Files\Steam"),
    ];
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        let home = PathBuf::from(home);
        roots.extend(
            [
                ".steam/steam",
                ".local/share/Steam",
                ".var/app/com.valvesoftware.Steam/.local/share/Steam",
                "Library/Application Support/Steam",
            ]
            .map(|p| home.join(p)),
        );
    }
    roots
}

/// Values of every `"key"  "value"` line of a Steam VDF/ACF text whose key is `key`,
/// with `\\` unescaped. Not a VDF parser: the two files read here need nothing more.
fn values<'a>(text: &'a str, key: &'a str) -> impl Iterator<Item = String> + 'a {
    text.lines().filter_map(move |l| {
        let mut q = l.split('"').skip(1).step_by(2);
        let (k, v) = (q.next()?, q.next()?);
        k.eq_ignore_ascii_case(key).then(|| v.replace("\\\\", "\\"))
    })
}

/// The GunZ install directory in the first Steam library that has the app manifest.
pub fn find_game() -> Option<PathBuf> {
    for root in steam_roots() {
        let steamapps = root.join("steamapps");
        let mut libraries = vec![root.clone()];
        if let Ok(text) = std::fs::read_to_string(steamapps.join("libraryfolders.vdf")) {
            libraries.extend(values(&text, "path").map(PathBuf::from));
        }
        if let Some(dir) = libraries.iter().find_map(|l| install_dir(l)) {
            return Some(dir);
        }
    }
    None
}

fn install_dir(library: &Path) -> Option<PathBuf> {
    let apps = library.join("steamapps");
    let manifest = std::fs::read_to_string(apps.join(format!("appmanifest_{APP_ID}.acf"))).ok()?;
    let dir = apps
        .join("common")
        .join(values(&manifest, "installdir").next()?);
    dir.is_dir().then_some(dir)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_library_paths_and_install_dir() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"C:\\\\Games\\\\Steam\"\n\t\t\"label\"\t\t\"\"\n\t}\n}\n";
        assert_eq!(values(vdf, "path").collect::<Vec<_>>(), [r"C:\Games\Steam"]);
        let acf = "\"AppState\"\n{\n\t\"appid\"\t\t\"3139440\"\n\t\"installdir\"\t\t\"GUNZ THE DUEL\"\n}\n";
        assert_eq!(values(acf, "installdir").next().unwrap(), "GUNZ THE DUEL");
    }
}
