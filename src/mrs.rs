//! MRS archives: ZIP files whose local headers and file names are XOR-obscured.
//!
//! Every local file header (30 bytes) and the file name that follows it are
//! XORed with the same keystream, restarting at index 0 for each buffer. File
//! payloads are plain stored/deflate ZIP data. The central directory uses the
//! same keystream as one long buffer; we never need it because the local
//! headers chain from offset 0 to the start of the central directory.
//! Format notes: `docs/formats.md`.

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock, Mutex};

/// Keystream prefix recovered from the retail archives (see `docs/formats.md`).
/// Indices 84 and 87 rest on a single observation; no retail name reaches them.
const KEY: [u8; 256] = [
    0x67, 0x1a, 0x36, 0x14, 0x9a, 0xaa, 0x28, 0x38, 0xc7, 0x78, 0x04, 0x82, 0x69, 0x57, 0x8f, 0xae,
    0x58, 0xfe, 0x28, 0xc8, 0x89, 0x20, 0x91, 0x2b, 0x69, 0xee, 0x80, 0xff, 0xaf, 0x56, 0xfa, 0xab,
    0x38, 0x01, 0xd7, 0xc4, 0x40, 0x7b, 0xf2, 0xba, 0x2d, 0x30, 0xda, 0x46, 0x02, 0x98, 0x2d, 0x1b,
    0x94, 0x0e, 0x9c, 0xad, 0xd3, 0x8e, 0x9d, 0xa5, 0xf0, 0x7a, 0xbb, 0x9c, 0x42, 0x63, 0x45, 0x8f,
    0x54, 0x68, 0x8b, 0x46, 0x14, 0x4f, 0xbe, 0x5b, 0x7a, 0x41, 0xcc, 0xd9, 0xeb, 0x18, 0x86, 0x6d,
    0x66, 0xdb, 0xfe, 0x7d, 0xcf, 0x4d, 0xdb, 0x74, 0xbe, 0xcc, 0x36, 0xb1, 0x2e, 0x25, 0x86, 0x7c,
    0xdf, 0x9e, 0x54, 0xbc, 0x18, 0x4a, 0x62, 0xde, 0x88, 0x30, 0x01, 0x73, 0x00, 0x5a, 0x13, 0xf7,
    0x09, 0x91, 0x31, 0x16, 0x21, 0xc3, 0x04, 0xa6, 0x31, 0x7e, 0x7f, 0xf2, 0x4d, 0x4d, 0x93, 0xaa,
    0xd0, 0x16, 0x6b, 0xa4, 0x88, 0xc4, 0x9f, 0x10, 0x6f, 0xa1, 0xd8, 0xe9, 0x35, 0xcd, 0x52, 0x43,
    0xaf, 0x69, 0x78, 0x44, 0x29, 0x0c, 0xf8, 0x60, 0x77, 0x8e, 0xde, 0x4e, 0x1f, 0x1e, 0x14, 0x7d,
    0xf4, 0xaf, 0x27, 0x24, 0x18, 0xa4, 0xf1, 0xea, 0x01, 0xaf, 0x2b, 0x10, 0x64, 0x01, 0xbc, 0xdc,
    0xac, 0x4a, 0x85, 0xce, 0xc4, 0xd1, 0xcf, 0x9d, 0xdf, 0x25, 0x1b, 0x99, 0xef, 0xda, 0xb7, 0x49,
    0xda, 0x10, 0xa6, 0xef, 0x84, 0x86, 0x15, 0x9a, 0x26, 0x6f, 0x67, 0x72, 0x07, 0xfc, 0xff, 0x6f,
    0x38, 0xd3, 0x95, 0x9f, 0xe1, 0xf9, 0xaa, 0x9b, 0xea, 0xce, 0x41, 0x56, 0xa4, 0x9d, 0x0b, 0x9c,
    0x1b, 0x25, 0x52, 0x18, 0x03, 0x07, 0xdc, 0xc2, 0xbd, 0x85, 0xb4, 0x05, 0xbc, 0x88, 0x6e, 0x35,
    0x80, 0x2a, 0x56, 0x6b, 0xe3, 0xcd, 0xc9, 0x6e, 0x79, 0x4d, 0xc5, 0xb9, 0xcc, 0xcc, 0x26, 0x1d,
];

const LOCAL_SIG: u32 = 0x0403_4b50;

fn unxor(buf: &mut [u8]) {
    for (b, k) in buf.iter_mut().zip(KEY) {
        *b ^= k;
    }
}

fn invalid(msg: String) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg)
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub name: String,
    pub method: u16,
    pub crc: u32,
    pub compressed: u32,
    pub size: u32,
    /// Absolute offset of the payload.
    pub data: u64,
}

#[derive(Clone)]
pub struct Archive {
    pub path: PathBuf,
    pub entries: Vec<Entry>,
}

impl Archive {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let path = path.as_ref().to_path_buf();
        let mut f = File::open(&path)?;
        let len = f.metadata()?.len();
        let mut entries = Vec::new();
        let mut pos = 0u64;
        while pos + 30 <= len {
            let mut h = [0u8; 30];
            f.seek(SeekFrom::Start(pos))?;
            f.read_exact(&mut h)?;
            unxor(&mut h);
            let u16_at = |o: usize| u16::from_le_bytes([h[o], h[o + 1]]);
            let u32_at = |o: usize| u32::from_le_bytes(h[o..o + 4].try_into().unwrap());
            if u32_at(0) != LOCAL_SIG {
                break; // central directory reached
            }
            let (flags, method, crc) = (u16_at(6), u16_at(8), u32_at(14));
            let (compressed, size) = (u32_at(18), u32_at(22));
            let (name_len, extra_len) = (u16_at(26) as usize, u16_at(28) as u64);
            if flags & 0x8 != 0 || !matches!(method, 0 | 8) {
                return Err(invalid(format!(
                    "{}: unsupported entry at {pos}: flags {flags:#x} method {method}",
                    path.display()
                )));
            }
            if name_len > KEY.len() {
                return Err(invalid(format!(
                    "{}: name length {name_len} at {pos} exceeds known key",
                    path.display()
                )));
            }
            let mut name = vec![0u8; name_len];
            f.read_exact(&mut name)?;
            unxor(&mut name);
            let data = pos + 30 + name_len as u64 + extra_len;
            pos = data + compressed as u64;
            if pos > len {
                return Err(invalid(format!(
                    "{}: entry at {data} overruns file",
                    path.display()
                )));
            }
            let name = String::from_utf8(name)
                .map_err(|e| invalid(format!("{}: name at {data}: {e}", path.display())))?;
            entries.push(Entry {
                name,
                method,
                crc,
                compressed,
                size,
                data,
            });
        }
        if entries.is_empty() {
            return Err(invalid(format!("{}: no MRS entries", path.display())));
        }
        Ok(Self { path, entries })
    }

    /// Reads, inflates and CRC-checks one entry.
    pub fn read(&self, e: &Entry) -> io::Result<Vec<u8>> {
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(e.data))?;
        let raw = f.take(e.compressed as u64);
        let mut out = Vec::with_capacity(e.size as usize);
        match e.method {
            0 => {
                raw.take(e.size as u64).read_to_end(&mut out)?;
            }
            _ => {
                flate2::read::DeflateDecoder::new(raw)
                    .take(e.size as u64 + 1)
                    .read_to_end(&mut out)?;
            }
        }
        let mut crc = flate2::Crc::new();
        crc.update(&out);
        if out.len() != e.size as usize || crc.sum() != e.crc {
            return Err(invalid(format!(
                "{}: {} failed size/CRC check",
                self.path.display(),
                e.name
            )));
        }
        Ok(out)
    }
}

/// Game virtual file system: `<dir>/<archive stem>/<entry name>` keyed case-insensitively,
/// e.g. `Maps.mrs` + `Mansion/Mansion.rs` -> `maps/mansion/mansion.rs`. Either the archives of
/// an install ([`Vfs::mount`]) or files held in memory ([`Vfs::from_packs`], the browser build).
#[derive(Clone)]
pub struct Vfs {
    pub archives: Vec<Archive>,
    index: HashMap<String, (usize, usize)>,
    files: Arc<HashMap<String, Packed>>,
}

/// A file of a pack.
#[derive(Clone, Debug, PartialEq)]
pub enum Packed {
    Bytes(Vec<u8>),
    /// In the install, but no loaded pack carries it.
    NameOnly,
    /// Served on its own next to the packs and downloaded when read (the browser build's
    /// clothes: `web::fetch`).
    Fetch,
}

pub fn normalize(path: &str) -> String {
    path.replace('\\', "/").to_ascii_lowercase()
}

/// `GUNZ_TRACE=FILE`: every path read is appended to FILE, one per line, so `gunz-pack` knows
/// what a match needs.
fn trace(path: &str) {
    static OUT: LazyLock<Option<Mutex<File>>> = LazyLock::new(|| {
        let p = std::env::var_os("GUNZ_TRACE")?;
        let f = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(p);
        Some(Mutex::new(f.ok()?))
    });
    if let Some(Ok(mut f)) = OUT.as_ref().map(Mutex::lock) {
        let _ = writeln!(f, "{path}");
    }
}

/// Header of a pack: `GZPK`, then a little-endian `u32` file count and per file a `u16` name
/// length, the normalized name, a `u32` length and the bytes. Length [`NAME_ONLY`] has no bytes:
/// the file exists in the install but is not packed ([`Vfs::exists`] still sees it, so checks
/// such as the character's animation list behave as with the install); length [`FETCH`] has
/// none either, the file is downloaded when read ([`Packed::Fetch`]).
const PACK: &[u8; 4] = b"GZPK";
const NAME_ONLY: u32 = u32::MAX;
const FETCH: u32 = u32::MAX - 1;

/// Writes `files` (normalized path and contents) as one pack.
pub fn pack(files: &[(String, Packed)]) -> Vec<u8> {
    let mut out = PACK.to_vec();
    out.extend((files.len() as u32).to_le_bytes());
    for (name, file) in files {
        out.extend((name.len() as u16).to_le_bytes());
        out.extend(name.as_bytes());
        match file {
            Packed::Bytes(b) => {
                out.extend((b.len() as u32).to_le_bytes());
                out.extend(b);
            }
            Packed::NameOnly => out.extend(NAME_ONLY.to_le_bytes()),
            Packed::Fetch => out.extend(FETCH.to_le_bytes()),
        }
    }
    out
}

impl Vfs {
    pub fn mount(root: impl AsRef<Path>) -> io::Result<Self> {
        let root = root.as_ref();
        let mut paths = Vec::new();
        collect_mrs(root, &mut paths)?;
        paths.sort();
        let mut archives = Vec::new();
        let mut index = HashMap::new();
        for p in paths {
            let archive = Archive::open(&p)?;
            let prefix = p.strip_prefix(root).unwrap().with_extension("");
            let prefix = normalize(&prefix.to_string_lossy());
            for (i, e) in archive.entries.iter().enumerate() {
                if !e.name.ends_with('/') {
                    index.insert(
                        format!("{prefix}/{}", normalize(&e.name)),
                        (archives.len(), i),
                    );
                }
            }
            archives.push(archive);
        }
        Ok(Self {
            archives,
            index,
            files: Arc::default(),
        })
    }

    /// The files of [`pack`]s; a later pack's bytes replace an earlier one's of the same name,
    /// an entry without bytes never replaces one with bytes.
    pub fn from_packs<'a>(packs: impl IntoIterator<Item = &'a [u8]>) -> io::Result<Self> {
        let mut files = HashMap::new();
        for p in packs {
            let bad = || invalid("truncated pack".into());
            let mut r = p
                .strip_prefix(PACK)
                .ok_or_else(|| invalid("not a pack".into()))?;
            let mut take = |n: usize| -> io::Result<&[u8]> {
                let (a, b) = r.split_at_checked(n).ok_or_else(bad)?;
                r = b;
                Ok(a)
            };
            let count = u32::from_le_bytes(take(4)?.try_into().unwrap());
            for _ in 0..count {
                let n = u16::from_le_bytes(take(2)?.try_into().unwrap()) as usize;
                let name = String::from_utf8(take(n)?.to_vec()).map_err(|_| bad())?;
                let len = u32::from_le_bytes(take(4)?.try_into().unwrap());
                let file = match len {
                    NAME_ONLY => Packed::NameOnly,
                    FETCH => Packed::Fetch,
                    _ => Packed::Bytes(take(len as usize)?.to_vec()),
                };
                let slot = files.entry(name).or_insert(Packed::NameOnly);
                if matches!(file, Packed::Bytes(_)) || *slot == Packed::NameOnly {
                    *slot = file;
                }
            }
        }
        Ok(Self {
            archives: Vec::new(),
            index: HashMap::new(),
            files: Arc::new(files),
        })
    }

    pub fn read(&self, path: &str) -> io::Result<Vec<u8>> {
        let path = normalize(path);
        trace(&path);
        match self.files.get(&path) {
            Some(Packed::Bytes(b)) => return Ok(b.clone()),
            Some(Packed::NameOnly) => {
                return Err(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("{path} is not in the downloaded packs"),
                ));
            }
            #[cfg(target_arch = "wasm32")]
            Some(Packed::Fetch) => return crate::web::fetch(&path),
            #[cfg(not(target_arch = "wasm32"))]
            Some(Packed::Fetch) => {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    format!("{path} is downloaded only by the browser build"),
                ));
            }
            None => {}
        }
        let &(a, e) = self
            .index
            .get(&path)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, path.clone()))?;
        self.archives[a].read(&self.archives[a].entries[e])
    }

    pub fn exists(&self, path: &str) -> bool {
        let path = normalize(path);
        self.files.contains_key(&path) || self.index.contains_key(&path)
    }

    /// Every readable file. Name-only pack entries are left out: callers pick files from this
    /// list (music, sounds, textures by name) and must not pick one without bytes.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        let packed = self
            .files
            .iter()
            .filter(|(_, f)| **f != Packed::NameOnly)
            .map(|(n, _)| n);
        self.index.keys().chain(packed).map(String::as_str)
    }

    /// Directory names of the maps (`maps/<dir>/<file>.rs`), sorted, whether or not their files
    /// are loaded (the browser build downloads a map's pack once it is picked).
    pub fn maps(&self) -> Vec<String> {
        let mut maps: Vec<String> = self
            .index
            .keys()
            .chain(self.files.keys())
            .filter_map(|p| {
                let (dir, file) = p.strip_prefix("maps/")?.split_once('/')?;
                (file.ends_with(".rs") && !file.contains('/')).then(|| dir.to_owned())
            })
            .collect();
        maps.sort_unstable();
        maps.dedup();
        maps
    }
}

fn collect_mrs(dir: &Path, out: &mut Vec<PathBuf>) -> io::Result<()> {
    for e in std::fs::read_dir(dir)? {
        let p = e?.path();
        if p.is_dir() {
            collect_mrs(&p, out)?;
        } else if p.extension().is_some_and(|x| x.eq_ignore_ascii_case("mrs")) {
            out.push(p);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn entry(name: &str, method: u16, data: &[u8]) -> Vec<u8> {
        let payload = if method == 8 {
            let mut enc =
                flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
            enc.write_all(data).unwrap();
            enc.finish().unwrap()
        } else {
            data.to_vec()
        };
        let mut crc = flate2::Crc::new();
        crc.update(data);
        let mut h = Vec::new();
        h.extend(LOCAL_SIG.to_le_bytes());
        h.extend([20, 0, 0, 0]);
        h.extend(method.to_le_bytes());
        h.extend([0; 4]);
        h.extend(crc.sum().to_le_bytes());
        h.extend((payload.len() as u32).to_le_bytes());
        h.extend((data.len() as u32).to_le_bytes());
        h.extend((name.len() as u16).to_le_bytes());
        h.extend([0, 0]);
        let mut name = name.as_bytes().to_vec();
        unxor(&mut h);
        unxor(&mut name);
        [h, name, payload].concat()
    }

    #[test]
    fn reads_obscured_entries_until_central_directory() {
        let long = "Dungeon_cavern1/Dungeon_cavern1_obj_ef_txa_fire_a.elu.ani";
        let text = b"<XML>hello</XML>".repeat(50);
        let mut file = [entry("system/a.xml", 8, &text), entry(long, 0, b"raw")].concat();
        file.extend(b"\x37\x51\x37\x16 central directory");
        let path = std::env::temp_dir().join(format!("mrs-test-{}.mrs", std::process::id()));
        std::fs::write(&path, &file).unwrap();
        let archive = Archive::open(&path).unwrap();
        let names: Vec<&str> = archive.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["system/a.xml", long]);
        assert_eq!(archive.read(&archive.entries[0]).unwrap(), text);
        assert_eq!(archive.read(&archive.entries[1]).unwrap(), b"raw".to_vec());
        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn packs_round_trip_and_later_packs_win() {
        let a = pack(&[
            ("maps/x/x.rs".into(), Packed::Bytes(b"one".to_vec())),
            ("system/a.xml".into(), Packed::Bytes(vec![])),
            ("model/big.elu".into(), Packed::NameOnly),
            ("model/man/set.elu".into(), Packed::Fetch),
        ]);
        let b = pack(&[
            ("maps/x/x.rs".into(), Packed::Bytes(b"two".to_vec())),
            ("system/a.xml".into(), Packed::NameOnly),
            ("maps/x/x.rs".into(), Packed::Fetch),
        ]);
        let vfs = Vfs::from_packs([&a[..], &b[..]]).unwrap();
        assert_eq!(vfs.read("Maps\\X\\X.rs").unwrap(), b"two");
        // an entry without bytes neither hides bytes nor makes an unpacked file readable
        assert_eq!(vfs.read("system/a.xml").unwrap(), b"");
        assert!(vfs.exists("model/big.elu") && vfs.read("model/big.elu").is_err());
        assert!(!vfs.exists("system/b.xml"));
        // listings offer files with bytes and files that are downloaded when read
        let mut listed: Vec<&str> = vfs.paths().collect();
        listed.sort_unstable();
        assert_eq!(listed, ["maps/x/x.rs", "model/man/set.elu", "system/a.xml"]);
        assert!(Vfs::from_packs([&a[..a.len() - 1]]).is_err());
    }
}
