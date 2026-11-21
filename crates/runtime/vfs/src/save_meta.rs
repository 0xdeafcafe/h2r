//! Save metadata files (`SaveMeta.xbx`, `TitleMeta.xbx`) and the repair of
//! damaged ones on U: when storage is mounted.
//!
//! The game's `XCreateSaveGame` opens `SaveMeta.xbx` with FILE_OVERWRITE_IF
//! and then writes it, so a process killed in between leaves it empty (or
//! torn). The save scanner then skips the directory, so its profile is not
//! listed, and creating the profile again collides with the directory
//! (the game shows "too many profiles"). The game never writes an empty
//! one: a good file is UTF-16LE, the byte-order mark `FF FE`, then
//! `key=value` lines with a `Name=` (`TitleName=` in `TitleMeta.xbx`).
//! docs/runtime/xapi.md "Damaged save metadata".

use std::io;
use std::path::{Path, PathBuf};

pub const SAVE_META: &str = "SaveMeta.xbx";
pub const TITLE_META: &str = "TitleMeta.xbx";
/// Under the data root: save directories set aside by [`repair_saves`].
pub const DAMAGED_SAVES: &str = "damaged-saves";

/// Largest prime below 2^48, the modulus of the save directory hash
/// (`0x330a3a`; xapi's `saves::directory_name`).
const HASH_MODULUS: u64 = 0x0000_ffff_ffff_ffc5;
/// Longest save name the game reads, in UTF-16 units.
const MAX_NAME_UNITS: usize = 127;
/// How much of each payload file is searched for the save's name.
const MAX_SEARCH_BYTES: u64 = 1 << 20;

/// Whether the last segment of `path` (`/` or `\` separated) is a meta
/// file, in any case.
pub fn is_meta_file(path: &str) -> bool {
    let last = path.rsplit(['/', '\\']).next().unwrap_or(path);
    last.eq_ignore_ascii_case(SAVE_META) || last.eq_ignore_ascii_case(TITLE_META)
}

/// Whether a meta file's bytes are damaged: not UTF-16LE with the `FF FE`
/// mark and a line whose key is `Name` or ends in `Name` (`TitleName`).
/// An empty file is damaged.
pub fn meta_damaged(bytes: &[u8]) -> bool {
    let Some(body) = bytes.strip_prefix(&[0xff, 0xfe]) else {
        return true;
    };
    let units: Vec<u16> = body
        .as_chunks::<2>().0.iter()
        .map(|p| u16::from_le_bytes([p[0], p[1]]))
        .collect();
    let name: Vec<u16> = "Name".encode_utf16().collect();
    !units.split(|&u| u == u16::from(b'\n')).any(|line| {
        line.iter()
            .position(|&u| u == u16::from(b'='))
            .is_some_and(|eq| line[..eq].ends_with(&name))
    })
}

/// `SaveMeta.xbx` naming `name`, as `XCreateSaveGame` writes it without
/// flags (`0x330f8f`).
pub fn encode_save_meta(name: &[u16]) -> Vec<u8> {
    std::iter::once(0xfeff)
        .chain("Name=".encode_utf16())
        .chain(name.iter().copied())
        .chain("\r\n".encode_utf16())
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// The save directory `name` gets (`0x3309b8`): the 48-bit hash of its
/// UTF-16 units.
pub fn save_directory_hash(name: &[u16]) -> u64 {
    name.iter()
        .fold(0, |h, &u| ((h << 16) + u64::from(u)) % HASH_MODULUS)
}

/// What [`repair_saves`] did to one save directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repair {
    /// `SaveMeta.xbx` rewritten naming the save, found in its payload.
    Rewritten { dir: String, name: String },
    /// No name found: the directory moved to `to`.
    SetAside { dir: String, to: PathBuf },
}

impl std::fmt::Display for Repair {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Repair::Rewritten { dir, name } => write!(
                f,
                "U:\\{dir}\\{SAVE_META} was damaged; rewritten naming {name:?} (found in the save)"
            ),
            Repair::SetAside { dir, to } => write!(
                f,
                "U:\\{dir}\\{SAVE_META} was damaged and names nothing; the save was moved to {} so it can be made again",
                to.display()
            ),
        }
    }
}

/// Repairs every save directory in `udata` (the host directory U: is)
/// whose `SaveMeta.xbx` is present but damaged. Its name is looked for in
/// the directory's other files: a UTF-16LE or 8-bit run whose
/// [`save_directory_hash`] is the directory's own name is the save's name,
/// with odds of a false match of about 2^-48. Found, the meta file is
/// rewritten naming it; not found, the directory moves to
/// `<data_root>/damaged-saves/<unix ms>-<dir>`. Directories without a meta
/// file, or not named by 12 hex digits, are left alone. Run before the
/// storage is mounted: nothing else has the files open.
pub fn repair_saves(udata: &Path, data_root: &Path) -> io::Result<Vec<Repair>> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(udata) {
        Ok(e) => e,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(out),
        Err(e) => return Err(e),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let Some(dir) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        let Some(hash) = (dir.len() == 12)
            .then(|| u64::from_str_radix(&dir, 16).ok())
            .flatten()
        else {
            continue;
        };
        let Some(meta) = find_file(&entry.path(), SAVE_META)? else {
            continue;
        };
        if !meta_damaged(&std::fs::read(&meta)?) {
            continue;
        }
        match find_name(&entry.path(), hash)? {
            Some(name) => {
                write_replacing(&meta, &encode_save_meta(&name))?;
                out.push(Repair::Rewritten {
                    dir,
                    name: String::from_utf16_lossy(&name),
                });
            }
            None => {
                let aside = data_root.join(DAMAGED_SAVES);
                std::fs::create_dir_all(&aside)?;
                let to = aside.join(format!("{}-{dir}", unix_ms()));
                std::fs::rename(entry.path(), &to)?;
                out.push(Repair::SetAside { dir, to });
            }
        }
    }
    Ok(out)
}

fn unix_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis())
}

/// The file in `dir` named `name` in any case.
fn find_file(dir: &Path, name: &str) -> io::Result<Option<PathBuf>> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        if entry
            .file_name()
            .to_str()
            .is_some_and(|n| n.eq_ignore_ascii_case(name))
            && entry.file_type()?.is_file()
        {
            return Ok(Some(entry.path()));
        }
    }
    Ok(None)
}

/// A name hashing to `hash` in one of `dir`'s files other than the meta
/// file and host litter (names starting with a dot).
fn find_name(dir: &Path, hash: u64) -> io::Result<Option<Vec<u16>>> {
    let mut files: Vec<PathBuf> = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let Some(n) = entry.file_name().to_str().map(str::to_string) else {
            continue;
        };
        if n.starts_with('.') || n.eq_ignore_ascii_case(SAVE_META) || !entry.file_type()?.is_file()
        {
            continue;
        }
        files.push(entry.path());
    }
    files.sort();
    for path in files {
        let mut bytes = Vec::new();
        use std::io::Read;
        std::fs::File::open(&path)?
            .take(MAX_SEARCH_BYTES)
            .read_to_end(&mut bytes)?;
        let utf16 = |offset: usize| {
            bytes[offset.min(bytes.len())..]
                .as_chunks::<2>().0.iter()
                .map(|p| u16::from_le_bytes([p[0], p[1]]))
                .collect::<Vec<u16>>()
        };
        let narrow: Vec<u16> = bytes.iter().map(|&b| u16::from(b)).collect();
        for units in [utf16(0), utf16(1), narrow] {
            if let Some(name) = name_in(&units, hash) {
                return Ok(Some(name));
            }
        }
    }
    Ok(None)
}

/// The first run of printable units in `units` that hashes to `hash`.
fn name_in(units: &[u16], hash: u64) -> Option<Vec<u16>> {
    let printable =
        |u: u16| (0x20..0x7f).contains(&u) || (0xa0..0xd800).contains(&u) || u >= 0xe000;
    for run in units.split(|&u| !printable(u)) {
        for start in 0..run.len() {
            let mut h = 0u64;
            for (len, &u) in run[start..].iter().take(MAX_NAME_UNITS).enumerate() {
                h = ((h << 16) + u64::from(u)) % HASH_MODULUS;
                if h == hash {
                    return Some(run[start..start + len + 1].to_vec());
                }
            }
        }
    }
    None
}

/// Writes `bytes` to a temporary file beside `path`, then renames it over.
fn write_replacing(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let tmp = path.with_file_name(format!(".h2repair-{}", std::process::id()));
    std::fs::write(&tmp, bytes)?;
    std::fs::File::open(&tmp)?.sync_all()?;
    std::fs::rename(&tmp, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "h2-vfs-save-meta-{name}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn utf16(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    fn dir_for(name: &str) -> String {
        format!("{:012X}", save_directory_hash(&utf16(name)))
    }

    #[test]
    fn the_hash_matches_xapis_vectors() {
        assert_eq!(dir_for(""), "000000000000");
        assert_eq!(dir_for("A"), "000000000041");
        assert_eq!(dir_for("AB"), "000000410042");
    }

    #[test]
    fn good_meta_files_are_not_damaged_and_empty_or_torn_ones_are() {
        let good = encode_save_meta(&utf16("Chief"));
        assert!(!meta_damaged(&good));
        let mut no_copy = good.clone();
        no_copy.extend("NoCopy=1\r\n".encode_utf16().flat_map(u16::to_le_bytes));
        assert!(!meta_damaged(&no_copy));
        let title: Vec<u8> = std::iter::once(0xfeff)
            .chain("TitleName=Halo 2\r\n".encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        assert!(!meta_damaged(&title));
        assert!(meta_damaged(b""));
        assert!(meta_damaged(&[0xff, 0xfe]));
        // Torn inside the key.
        assert!(meta_damaged(&good[..8]));
        // No byte-order mark, or 8-bit text.
        assert!(meta_damaged(&good[2..]));
        assert!(meta_damaged(b"Name=Chief"));
        assert!(is_meta_file("0123456789AB/SaveMeta.xbx"));
        assert!(is_meta_file("U:\\0123456789AB\\savemeta.XBX"));
        assert!(is_meta_file("TitleMeta.xbx"));
        assert!(!is_meta_file("0123456789AB/profile"));
    }

    #[test]
    fn a_damaged_meta_is_rewritten_from_the_name_in_the_payload() {
        let data = temp("rewrite");
        let udata = data.join("UDATA");
        let dir = dir_for("Chief");
        let save = udata.join(&dir);
        std::fs::create_dir_all(&save).unwrap();
        std::fs::write(save.join(SAVE_META), b"").unwrap();
        // The payload holds the name in UTF-16LE at an odd offset, among
        // other text.
        let mut payload = vec![0u8, 1, 2];
        payload.extend(utf16("xChief").iter().flat_map(|u| u.to_le_bytes()));
        payload.extend([0, 0, 9, 9]);
        std::fs::write(save.join("profile"), &payload).unwrap();
        // A good save beside it is untouched.
        let other = udata.join(dir_for("Arbiter"));
        std::fs::create_dir_all(&other).unwrap();
        let arbiter = encode_save_meta(&utf16("Arbiter"));
        std::fs::write(other.join(SAVE_META), &arbiter).unwrap();
        let done = repair_saves(&udata, &data).unwrap();
        assert_eq!(
            done,
            [Repair::Rewritten {
                dir: dir.clone(),
                name: "Chief".into()
            }]
        );
        assert_eq!(
            std::fs::read(save.join(SAVE_META)).unwrap(),
            encode_save_meta(&utf16("Chief"))
        );
        assert_eq!(std::fs::read(other.join(SAVE_META)).unwrap(), arbiter);
        assert!(!data.join(DAMAGED_SAVES).exists());
        // Run again: nothing to do.
        assert_eq!(repair_saves(&udata, &data).unwrap(), []);
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn a_damaged_meta_with_no_name_to_find_is_set_aside() {
        let data = temp("aside");
        let udata = data.join("UDATA");
        let dir = dir_for("Chief");
        let save = udata.join(&dir);
        std::fs::create_dir_all(&save).unwrap();
        std::fs::write(save.join(SAVE_META), b"Name=Ch").unwrap();
        std::fs::write(save.join("profile"), b"no name here").unwrap();
        // Not a save directory, and a save without a meta file: left alone.
        std::fs::create_dir_all(udata.join("scratch")).unwrap();
        std::fs::write(udata.join("scratch").join(SAVE_META), b"").unwrap();
        std::fs::create_dir_all(udata.join("0123456789AB")).unwrap();
        let done = repair_saves(&udata, &data).unwrap();
        let [Repair::SetAside { dir: d, to }] = done.as_slice() else {
            panic!("{done:?}");
        };
        assert_eq!(d, &dir);
        assert!(!save.exists());
        assert!(to.starts_with(data.join(DAMAGED_SAVES)));
        assert!(
            to.file_name()
                .unwrap()
                .to_str()
                .unwrap()
                .ends_with(&format!("-{dir}"))
        );
        assert_eq!(std::fs::read(to.join("profile")).unwrap(), b"no name here");
        assert!(udata.join("scratch").join(SAVE_META).exists());
        assert!(udata.join("0123456789AB").exists());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn no_u_drive_yet_is_nothing_to_do() {
        let data = temp("none");
        assert_eq!(repair_saves(&data.join("UDATA"), &data).unwrap(), []);
        let _ = std::fs::remove_dir_all(&data);
    }
}
