//! Sim batch runs start with the map cache already filled
//! (docs/runtime/cache-slots.md, "Sim batches: a warm Z:" and "Sim batches:
//! a warm N:").
//!
//! Each batch run has a data directory of its own, so the title would copy
//! every map it uses from the disc into its cache slots on every boot:
//! `shared` and `single_player_shared` into Z:'s slots 0 and 1, the main menu
//! and the match's map into N:'s slots 2 and 3.
//!
//! The first run of an image that fills them saves them as a **template**,
//! `<out>/sim-cache/utility/<key>/`, keyed by the image's build stamp, the
//! disc and this `h2` binary:
//!
//! - `utility/`: Z:'s finished slots and `preferences.dat`. Z: keeps its
//!   files across boots, so the batch clones them into the run's Z: before
//!   it boots. The preferences file is part of it: without one, the title's
//!   cache services (`0x2136c0`) see the default language and build and
//!   delete every slot file on Z: and N: at boot.
//! - `cache/mainmenu/` and `cache/<map>/`: N:'s slot 2 and slot 3. The boot
//!   formats N:, so these are not placed before the run: the run's VFS clones
//!   them in straight after the format (`vfs::Config::cache_seed`, named by
//!   [`vfs::CACHE_SEED_ENV`]).
//!
//! Each holds `disk/` (the files) and `metadata/` (their FATX records).
//! Runs get the files as APFS clones (copy-on-write, so N runs cost about
//! one copy); elsewhere they are copied. `H2_SIM_CACHE_TEMPLATE=0` turns all
//! of it off.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use vfs::clone_or_copy;

/// `0`, `off`, `false` or `no` turns the template off.
pub const ENV: &str = "H2_SIM_CACHE_TEMPLATE";
/// Z:'s cache slots: slot 0 (shared) and slot 1 (single-player shared).
pub const SLOTS: [&str; 2] = ["cache000.map", "cache001.map"];
/// The title's global preferences on Z: (`z:\preferences.dat`), read at
/// boot (`0x121350`) before the cache services check the language and build
/// they hold (`0x2136c0`).
pub const PREFERENCES: &str = "preferences.dat";
/// The preferences file's size: the title reads 0x1fc bytes and takes
/// nothing else (`0x1214e0`).
const PREFERENCES_LEN: u64 = 0x1fc;
/// N:'s main menu slot (slot 2).
pub const MENU_SLOT: &str = "cache002.map";
/// N:'s multiplayer map slot (slot 3).
pub const MAP_SLOT: &str = "cache003.map";
/// The main menu's map name, as the slot header's path names it.
const MENU: &str = "mainmenu";
/// Bumped when the template's layout or the rules for a valid file change.
const LAYOUT: u32 = 2;
/// A map's header: `head` at 0, `foot` at 0x7fc, both little-endian.
const HEADER: usize = 0x800;
/// The slot's source path in its header (copy state 4, `0x214343`).
const HEADER_PATH: usize = 0x20;
/// The longest path the header holds.
const HEADER_PATH_MAX: usize = 0x100;
/// A staging directory older than this was left by a run that died.
const STALE_STAGING: Duration = Duration::from_secs(3600);

/// Whether the template is on, from [`ENV`]'s value.
pub fn enabled(value: Option<&str>) -> bool {
    !matches!(
        value.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// FNV-1a, 64 bits.
fn fnv64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| {
        (h ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// The template's key: the image's build stamp, the disc's identity and
/// this `h2`'s identity (each from [`file_id`]). Any change starts a new
/// template. The map is not in it: each map's N: slot is a part of its own
/// under the key ([`Part::Map`]).
pub fn key(stamp: &str, disc: &str, exe: &str) -> String {
    let text = format!("layout {LAYOUT}\nstamp {stamp}\ndisc {disc}\nexe {exe}\n");
    format!("{:016x}", fnv64(text.as_bytes()))
}

/// A file's identity for [`key`]: its resolved path, length and mtime.
pub fn file_id(path: &Path) -> io::Result<String> {
    let meta = fs::metadata(path)?;
    let mtime = meta
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    Ok(format!(
        "{} {} {mtime}",
        fs::canonicalize(path)?.display(),
        meta.len()
    ))
}

/// The host directories of the data root's partition `n`: its files and
/// its FATX metadata records (`vfs::Vfs::with_storage`).
fn partition(data: &Path, n: u8) -> (PathBuf, PathBuf) {
    let name = format!("Partition{n}");
    (
        data.join("disk").join(&name),
        data.join("metadata").join(name),
    )
}

/// The utility partition (Z:).
fn utility_partition() -> u8 {
    vfs::Config::new("", 0).utility_partition
}

/// The data root's utility partition (Z:).
fn utility(data: &Path) -> (PathBuf, PathBuf) {
    partition(data, utility_partition())
}

/// The data root's partition holding N:'s file `name`: one of the cache
/// partitions 3 to 5 other than Z:'s (XAPI picks N:'s at boot).
fn cache_partition_with(data: &Path, name: &str) -> Option<(PathBuf, PathBuf)> {
    (3..=5)
        .filter(|&n| n != utility_partition())
        .map(|n| partition(data, n))
        .find(|(disk, _)| disk.join(name).is_file())
}

/// The directory of a root file's metadata record: the lowercase name's
/// bytes in hex (`record_path` in crates/runtime/vfs/src/host.rs).
fn record_dir(name: &str) -> String {
    name.bytes()
        .map(|b| format!("{:02x}", b.to_ascii_lowercase()))
        .collect()
}

fn read_header(path: &Path) -> Option<[u8; HEADER]> {
    let mut header = [0u8; HEADER];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .ok()?;
    (&header[..4] == b"daeh" && &header[HEADER - 4..] == b"toof").then_some(header)
}

/// Whether `path` holds a published map header. The copier writes a zero
/// header before the payload and the map's header only after the payload
/// is flushed, so a valid header means a finished copy.
pub fn complete(path: &Path) -> bool {
    read_header(path).is_some()
}

/// Whether `path` holds a finished copy of the disc's `maps\<map>.map`: a
/// published header whose path (`+0x20`) is `d:\maps\<map>.map`.
pub fn holds_map(path: &Path, map: &str) -> bool {
    read_header(path).is_some_and(|h| {
        let field = &h[HEADER_PATH..HEADER_PATH + HEADER_PATH_MAX];
        let end = field.iter().position(|&b| b == 0).unwrap_or(field.len());
        field[..end].eq_ignore_ascii_case(format!("d:\\maps\\{map}.map").as_bytes())
    })
}

/// Whether `path` is a preferences file the title would take.
fn preferences_file(path: &Path) -> bool {
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.len() == PREFERENCES_LEN)
}

/// A map name a template part can be keyed on: a disc map's file name.
fn map_name(map: &str) -> bool {
    !map.is_empty()
        && !map.eq_ignore_ascii_case(MENU)
        && map
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// One part of a template, saved whole by one run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Part {
    /// Z:: slots 0 and 1 and the preferences file.
    Utility,
    /// N:'s slot 2, the main menu.
    Menu,
    /// N:'s slot 3 holding this map.
    Map(String),
}

impl Part {
    /// The part's directory under the key.
    fn sub(&self) -> PathBuf {
        match self {
            Part::Utility => PathBuf::from("utility"),
            Part::Menu => Path::new("cache").join(MENU),
            Part::Map(map) => Path::new("cache").join(map),
        }
    }

    /// For status lines.
    pub fn label(&self) -> String {
        match self {
            Part::Utility => "Z:".into(),
            Part::Menu => "N: slot 2 (mainmenu)".into(),
            Part::Map(map) => format!("N: slot 3 ({map})"),
        }
    }

    /// The finished files of this part in a run's data: (name, file,
    /// record directory). Empty unless the part is whole: Z: needs the
    /// preferences file and at least one finished slot.
    fn sources(&self, data: &Path) -> Vec<(&'static str, PathBuf, PathBuf)> {
        match self {
            Part::Utility => {
                let (disk, meta) = utility(data);
                let slots: Vec<&'static str> = SLOTS
                    .into_iter()
                    .filter(|s| complete(&disk.join(s)))
                    .collect();
                if slots.is_empty() || !preferences_file(&disk.join(PREFERENCES)) {
                    return Vec::new();
                }
                slots
                    .into_iter()
                    .chain([PREFERENCES])
                    .map(|s| (s, disk.join(s), meta.clone()))
                    .collect()
            }
            Part::Menu | Part::Map(_) => {
                let (name, map) = match self {
                    Part::Map(map) => (MAP_SLOT, map.as_str()),
                    _ => (MENU_SLOT, MENU),
                };
                match cache_partition_with(data, name) {
                    Some((disk, meta)) if holds_map(&disk.join(name), map) => {
                        vec![(name, disk.join(name), meta)]
                    }
                    _ => Vec::new(),
                }
            }
        }
    }
}

/// What [`Template::populate`] did to a run's data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Populated {
    /// The template's Z: files are in place.
    Warm { files: usize, cloned: bool },
    /// The data already has a Z: file; nothing was touched.
    Occupied,
    /// There is no Z: template yet.
    Miss,
}

/// What [`Template::save`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(clippy::enum_variant_names)] // `Saved::Saved` reads as the outcome it is.
pub enum Saved {
    Saved {
        files: usize,
        cloned: bool,
    },
    /// This part is already there (another run's).
    Exists,
    /// The run left no whole copy of this part.
    Nothing,
}

/// One image's template.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    /// `<out>/sim-cache/utility`: every key's template.
    root: PathBuf,
    key: String,
}

impl Template {
    pub fn new(root: PathBuf, key: String) -> Template {
        Template { root, key }
    }

    /// The template for a batch on the image `dylib` (under `out`) and
    /// `disc`, or `None` when it is off or the image has no stamp. Says why
    /// on stdout.
    pub fn for_batch(out: &Path, dylib: &Path, disc: &Path) -> Option<Template> {
        if !enabled(std::env::var(ENV).ok().as_deref()) {
            println!("map cache: {ENV} is off; every run copies its maps to Z: and N:");
            return None;
        }
        let parts = || -> io::Result<String> {
            let stamp = fs::read_to_string(boot::stamp_path(dylib))?;
            Ok(key(
                &stamp,
                &file_id(disc)?,
                &file_id(&std::env::current_exe()?)?,
            ))
        };
        match parts() {
            Ok(key) => Some(Template::new(out.join("sim-cache").join("utility"), key)),
            Err(e) => {
                println!("map cache: no template ({e}); every run copies its maps to Z: and N:");
                None
            }
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// `<root>/<key>`: one directory per part.
    pub fn dir(&self) -> PathBuf {
        self.root.join(&self.key)
    }

    /// The part's directory: `disk/` holds its files, `metadata/` their
    /// records.
    pub fn part_dir(&self, part: &Part) -> PathBuf {
        self.dir().join(part.sub())
    }

    /// The parts a run of `map` would use that are not saved yet. A map
    /// whose name cannot key a part has none of its own.
    pub fn missing(&self, map: &str) -> Vec<Part> {
        let mut parts = vec![Part::Utility, Part::Menu];
        if map_name(map) {
            parts.push(Part::Map(map.to_owned()));
        }
        parts.retain(|p| !self.part_dir(p).exists());
        parts
    }

    /// The N: seed directories for a run of `map` (`vfs::Config::cache_seed`):
    /// the main menu's part, then the map's, each when it holds a finished
    /// slot. The run's VFS clones them in after it formats N:. The map's
    /// slot 3 goes in only with slot 2: at boot an empty slot file (the
    /// wrong size) makes the title delete every higher slot's file
    /// (`0x213970`).
    pub fn seed(&self, map: &str) -> Vec<PathBuf> {
        let mut seed = Vec::new();
        let menu = self.part_dir(&Part::Menu);
        if !holds_map(&menu.join("disk").join(MENU_SLOT), MENU) {
            return seed;
        }
        seed.push(menu);
        if map_name(map) {
            let dir = self.part_dir(&Part::Map(map.to_owned()));
            if holds_map(&dir.join("disk").join(MAP_SLOT), map) {
                seed.push(dir);
            }
        }
        seed
    }

    /// Puts the template's Z: files into `data`'s Z:, before the run boots.
    /// Leaves a Z: that already has one of them alone. On an error, removes
    /// what it placed, so the run starts cold.
    pub fn populate(&self, data: &Path) -> io::Result<Populated> {
        let (disk, meta) = utility(data);
        if SLOTS
            .iter()
            .chain([&PREFERENCES])
            .any(|s| disk.join(s).exists())
        {
            return Ok(Populated::Occupied);
        }
        let dir = self.part_dir(&Part::Utility);
        let (tdisk, tmeta) = (dir.join("disk"), dir.join("metadata"));
        let mut files: Vec<&str> = SLOTS
            .into_iter()
            .filter(|s| complete(&tdisk.join(s)))
            .collect();
        if files.is_empty() || !preferences_file(&tdisk.join(PREFERENCES)) {
            return Ok(Populated::Miss);
        }
        files.push(PREFERENCES);
        let mut placed = Vec::new();
        let result = (|| -> io::Result<bool> {
            fs::create_dir_all(&disk)?;
            let mut cloned = true;
            for s in &files {
                let record = tmeta.join(record_dir(s)).join("record");
                if record.is_file() {
                    let to = meta.join(record_dir(s));
                    fs::create_dir_all(&to)?;
                    let to = to.join("record");
                    match fs::remove_file(&to) {
                        Ok(()) => {}
                        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                        Err(e) => return Err(e),
                    }
                    cloned &= clone_or_copy(&record, &to)?;
                    placed.push(to);
                }
                let to = disk.join(s);
                cloned &= clone_or_copy(&tdisk.join(s), &to)?;
                placed.push(to);
            }
            Ok(cloned)
        })();
        match result {
            Ok(cloned) => Ok(Populated::Warm {
                files: files.len(),
                cloned,
            }),
            Err(e) => {
                for p in placed {
                    let _ = fs::remove_file(p);
                }
                Err(e)
            }
        }
    }

    /// Saves one part from `data`, after a run that made it has exited.
    /// Staged in a directory of its own and renamed into place, so
    /// concurrent savers leave one whole part. Removes other keys'
    /// templates once saved.
    pub fn save(&self, data: &Path, part: &Part) -> io::Result<Saved> {
        let dir = self.part_dir(part);
        if dir.exists() {
            return Ok(Saved::Exists);
        }
        let sources = part.sources(data);
        if sources.is_empty() {
            return Ok(Saved::Nothing);
        }
        fs::create_dir_all(dir.parent().unwrap_or(&self.root))?;
        // Unique across processes (the pid) and threads (the counter).
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let staging = self.root.join(format!(
            ".staging-{}-{}-{}",
            self.key,
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let staged = (|| -> io::Result<bool> {
            let (sdisk, smeta) = (staging.join("disk"), staging.join("metadata"));
            fs::create_dir_all(&sdisk)?;
            fs::create_dir_all(&smeta)?;
            let mut cloned = true;
            for (name, file, meta) in &sources {
                cloned &= clone_or_copy(file, &sdisk.join(name))?;
                let record = meta.join(record_dir(name)).join("record");
                if record.is_file() {
                    let to = smeta.join(record_dir(name));
                    fs::create_dir_all(&to)?;
                    cloned &= clone_or_copy(&record, &to.join("record"))?;
                }
            }
            Ok(cloned)
        })();
        let cloned = match staged {
            Ok(c) => c,
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                return Err(e);
            }
        };
        match fs::rename(&staging, &dir) {
            Ok(()) => {
                self.prune();
                Ok(Saved::Saved {
                    files: sources.len(),
                    cloned,
                })
            }
            Err(e) => {
                let _ = fs::remove_dir_all(&staging);
                if dir.exists() {
                    Ok(Saved::Exists)
                } else {
                    Err(e)
                }
            }
        }
    }

    /// Removes other keys' templates (an older image, disc or `h2`) and
    /// staging directories a dead run left.
    fn prune(&self) {
        let Ok(entries) = fs::read_dir(&self.root) else {
            return;
        };
        for e in entries.flatten() {
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            if name == self.key {
                continue;
            }
            let stale = !name.starts_with(".staging-")
                || e.metadata()
                    .and_then(|m| m.modified())
                    .ok()
                    .and_then(|t| t.elapsed().ok())
                    .is_some_and(|age| age > STALE_STAGING);
            if stale {
                let _ = fs::remove_dir_all(e.path());
            }
        }
    }
}

/// Lets the first run of each map that misses part of the template make it
/// while the batch's other runs of that map wait, instead of every run of
/// the first wave copying the same maps at once. One claim a map a batch:
/// if that run saves nothing, the rest of that map's runs go on without.
#[derive(Default)]
pub struct Gate {
    /// Per map: (claimed, producing).
    state: Mutex<std::collections::HashMap<String, (bool, bool)>>,
    done: Condvar,
}

/// Held by the run making the template; dropping it lets the others go.
pub struct Producing<'a> {
    gate: &'a Gate,
    map: String,
}

impl Drop for Producing<'_> {
    fn drop(&mut self) {
        let mut s = self.gate.state.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(entry) = s.get_mut(&self.map) {
            entry.1 = false;
        }
        drop(s);
        self.gate.done.notify_all();
    }
}

impl Gate {
    /// The batch's first caller for `map` gets the guard.
    pub fn claim(&self, map: &str) -> Option<Producing<'_>> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let entry = s.entry(map.to_owned()).or_default();
        if entry.0 {
            return None;
        }
        *entry = (true, true);
        Some(Producing {
            gate: self,
            map: map.to_owned(),
        })
    }

    /// Waits while a claimed run of `map` is making the template.
    pub fn wait(&self, map: &str) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while s.get(map).is_some_and(|e| e.1) {
            s = self.done.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// How one batch run starts.
pub enum Start<'a> {
    /// No template (switched off, or the image has no stamp).
    Off,
    Run {
        /// Z:: placed from the template, already there, or cold.
        z: Populated,
        /// N:'s seed directories, for [`vfs::CACHE_SEED_ENV`].
        seed: Vec<PathBuf>,
        /// Held when this run saves the parts in `saves` once it exits 0.
        producer: Option<Producing<'a>>,
        saves: Vec<Part>,
    },
}

impl Start<'_> {
    /// N:'s seed directories for the run's VFS; empty for none.
    pub fn seed(&self) -> &[PathBuf] {
        match self {
            Start::Off => &[],
            Start::Run { seed, .. } => seed,
        }
    }

    /// The parts this run saves when it exits 0, if it is the producer.
    pub fn saves(&self) -> Option<&[Part]> {
        match self {
            Start::Run {
                producer: Some(_),
                saves,
                ..
            } => Some(saves),
            _ => None,
        }
    }

    /// For the run's status line.
    pub fn label(&self) -> String {
        let Start::Run {
            z, seed, producer, ..
        } = self
        else {
            return String::new();
        };
        let z = match z {
            Populated::Warm { files, cloned } => format!(
                "warm Z: ({files} files {})",
                if *cloned {
                    "cloned"
                } else {
                    "copied, no clone"
                }
            ),
            Populated::Occupied => "Z: as found".into(),
            Populated::Miss => "cold Z:".into(),
        };
        let n = match seed.len() {
            0 => "cold N:".to_owned(),
            n => format!("warm N: ({n} of 2 slots seeded)"),
        };
        let saving = if producer.is_some() {
            ", saving the template"
        } else {
            ""
        };
        format!("  {z}, {n}{saving}")
    }
}

/// Prepares `data` for one batch run of `map`: the template's parts that
/// exist, and the claim to make the missing ones if no run of this map has
/// it yet (otherwise, after waiting for the run that has it, whatever it
/// saved).
pub fn start<'a>(template: Option<&Template>, gate: &'a Gate, data: &Path, map: &str) -> Start<'a> {
    let Some(t) = template else {
        return Start::Off;
    };
    let mut producer = None;
    let mut saves = t.missing(map);
    if !saves.is_empty() {
        match gate.claim(map) {
            Some(p) => producer = Some(p),
            None => {
                gate.wait(map);
                saves.clear();
            }
        }
    }
    let z = match t.populate(data) {
        Ok(z) => z,
        Err(e) => {
            println!(
                "map cache: template {} not used for Z: ({e}); this run copies Z:'s maps",
                t.key()
            );
            Populated::Miss
        }
    };
    if z == Populated::Occupied {
        // Z: files from elsewhere never become the template.
        saves.retain(|p| *p != Part::Utility);
    }
    if saves.is_empty() {
        producer = None;
    }
    // With a cold Z: the boot finds slot 0 empty and deletes every higher
    // slot's file, a seeded N: too (`0x213970`).
    let seed = match z {
        Populated::Miss => Vec::new(),
        Populated::Warm { .. } | Populated::Occupied => t.seed(map),
    };
    Start::Run {
        z,
        seed,
        producer,
        saves,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::AtomicBool;

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-sim-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A finished slot of `map`: a map header naming `d:\maps\<map>.map`
    /// and a payload, synthetic bytes.
    fn slot_of(map: &str, fill: u8) -> Vec<u8> {
        let mut b = vec![fill; HEADER + 4096];
        b[..4].copy_from_slice(b"daeh");
        b[HEADER - 4..HEADER].copy_from_slice(b"toof");
        let path = format!("d:\\maps\\{map}.map\0");
        b[HEADER_PATH..HEADER_PATH + path.len()].copy_from_slice(path.as_bytes());
        b
    }

    /// A finished Z: slot.
    fn slot(fill: u8) -> Vec<u8> {
        slot_of("shared", fill)
    }

    fn prefs(fill: u8) -> Vec<u8> {
        vec![fill; PREFERENCES_LEN as usize]
    }

    fn write_with_record(disk: &Path, meta: &Path, name: &str, bytes: &[u8]) {
        fs::create_dir_all(disk).unwrap();
        fs::write(disk.join(name), bytes).unwrap();
        let r = meta.join(record_dir(name));
        fs::create_dir_all(&r).unwrap();
        fs::write(r.join("record"), format!("HVF1 {name}")).unwrap();
    }

    /// A data root whose Z: holds `files` (name, bytes), each with a
    /// metadata record, and a file that is not a cache file.
    fn data_with(root: &Path, files: &[(&str, Vec<u8>)]) {
        let (disk, meta) = utility(root);
        fs::create_dir_all(&disk).unwrap();
        for (name, bytes) in files {
            write_with_record(&disk, &meta, name, bytes);
        }
        fs::write(disk.join("other.bin"), b"per run").unwrap();
    }

    /// A whole Z:: both slots and the preferences.
    fn z_with(root: &Path, fill: u8) {
        data_with(
            root,
            &[
                (SLOTS[0], slot(fill)),
                (SLOTS[1], slot(fill)),
                (PREFERENCES, prefs(fill)),
            ],
        );
    }

    /// A data root whose N: (Partition4) holds `files`, with records.
    fn n_with(root: &Path, files: &[(&str, Vec<u8>)]) {
        let (disk, meta) = partition(root, 4);
        for (name, bytes) in files {
            write_with_record(&disk, &meta, name, bytes);
        }
        fs::write(disk.join("cache004.map"), b"campaign").unwrap();
    }

    #[test]
    fn the_switch_only_turns_it_off() {
        for on in [None, Some(""), Some("1"), Some("on"), Some("yes")] {
            assert!(enabled(on), "{on:?}");
        }
        for off in ["0", "off", "OFF", "false", "no", " no "] {
            assert!(!enabled(Some(off)), "{off}");
        }
    }

    #[test]
    fn the_key_follows_the_image_the_disc_and_h2() {
        let k = key("tier clang xbe 1", "/d/halo2.iso 10 5", "/bin/h2 7 9");
        assert_eq!(
            k,
            key("tier clang xbe 1", "/d/halo2.iso 10 5", "/bin/h2 7 9")
        );
        assert_eq!(k.len(), 16);
        assert_ne!(
            k,
            key("tier clang xbe 2", "/d/halo2.iso 10 5", "/bin/h2 7 9")
        );
        assert_ne!(
            k,
            key("tier clang xbe 1", "/d/halo2.iso 10 6", "/bin/h2 7 9")
        );
        assert_ne!(
            k,
            key("tier clang xbe 1", "/d/halo2.iso 10 5", "/bin/h2 8 9")
        );
    }

    #[test]
    fn file_ids_change_with_the_contents() {
        let d = scratch("file-id");
        let f = d.join("h2");
        fs::write(&f, b"one").unwrap();
        let a = file_id(&f).unwrap();
        assert_eq!(a, file_id(&f).unwrap());
        fs::write(&f, b"three").unwrap();
        assert_ne!(a, file_id(&f).unwrap());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn records_are_named_like_the_vfs_names_them() {
        assert_eq!(record_dir("CACHE000.MAP"), "63616368653030302e6d6170");
        assert_eq!(record_dir("cache000.map"), record_dir("CACHE000.MAP"));
    }

    #[test]
    fn a_slot_holds_the_map_its_header_names() {
        let d = scratch("holds");
        let f = d.join("cache003.map");
        fs::write(&f, slot_of("Lockout", 1)).unwrap();
        assert!(holds_map(&f, "lockout"));
        assert!(holds_map(&f, "LOCKOUT"));
        assert!(!holds_map(&f, "midship"));
        assert!(!holds_map(&f, "lock"));
        // A zero header is an unfinished copy of anything.
        fs::write(&f, vec![0; HEADER + 16]).unwrap();
        assert!(!holds_map(&f, "lockout"));
        assert!(map_name("lockout") && map_name("ascension_2"));
        for bad in ["", "mainmenu", "../x", "a b", "t:\\x"] {
            assert!(!map_name(bad), "{bad}");
        }
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_saved_template_populates_a_fresh_run_with_the_same_z_files() {
        let d = scratch("roundtrip");
        let first = d.join("runs/a/data");
        data_with(
            &first,
            &[
                (SLOTS[0], slot(1)),
                (SLOTS[1], slot(2)),
                (PREFERENCES, prefs(3)),
            ],
        );
        let t = Template::new(d.join("image/sim-cache/utility"), "k1".into());
        assert_eq!(t.populate(&d.join("runs/b/data")).unwrap(), Populated::Miss);
        assert!(matches!(
            t.save(&first, &Part::Utility).unwrap(),
            Saved::Saved { files: 3, .. }
        ));
        // The run's other files stay out of the template.
        let tdisk = t.part_dir(&Part::Utility).join("disk");
        assert!(!tdisk.join("other.bin").exists());

        let next = d.join("runs/b/data");
        let got = t.populate(&next).unwrap();
        assert!(matches!(got, Populated::Warm { files: 3, .. }), "{got:?}");
        let (disk, meta) = utility(&next);
        assert_eq!(fs::read(disk.join(SLOTS[0])).unwrap(), slot(1));
        assert_eq!(fs::read(disk.join(SLOTS[1])).unwrap(), slot(2));
        assert_eq!(fs::read(disk.join(PREFERENCES)).unwrap(), prefs(3));
        assert_eq!(
            fs::read_to_string(meta.join(record_dir(SLOTS[1])).join("record")).unwrap(),
            format!("HVF1 {}", SLOTS[1])
        );
        assert!(!disk.join("other.bin").exists());
        // A clone is a file of its own: writing it leaves the template.
        fs::write(disk.join(SLOTS[0]), b"changed").unwrap();
        assert_eq!(fs::read(tdisk.join(SLOTS[0])).unwrap(), slot(1));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn z_without_its_preferences_is_never_saved() {
        // Without them the title deletes the slots at boot (0x2136c0).
        let d = scratch("no-prefs");
        let run = d.join("run");
        data_with(&run, &[(SLOTS[0], slot(1)), (SLOTS[1], slot(2))]);
        let t = Template::new(d.join("t"), "k".into());
        assert_eq!(t.save(&run, &Part::Utility).unwrap(), Saved::Nothing);
        // Nor preferences of the wrong size.
        fs::write(utility(&run).0.join(PREFERENCES), vec![0; 16]).unwrap();
        assert_eq!(t.save(&run, &Part::Utility).unwrap(), Saved::Nothing);
        assert!(!t.part_dir(&Part::Utility).exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unfinished_slots_are_never_saved_or_used() {
        let d = scratch("unfinished");
        let run = d.join("run");
        // Slot 1's header is still the zero one the copier writes first.
        data_with(
            &run,
            &[
                (SLOTS[0], slot(1)),
                (SLOTS[1], vec![0; HEADER + 16]),
                (PREFERENCES, prefs(1)),
            ],
        );
        n_with(&run, &[(MENU_SLOT, vec![0; HEADER + 16])]);
        let t = Template::new(d.join("t"), "k".into());
        assert!(matches!(
            t.save(&run, &Part::Utility).unwrap(),
            Saved::Saved { files: 2, .. }
        ));
        assert!(
            !t.part_dir(&Part::Utility)
                .join("disk")
                .join(SLOTS[1])
                .exists()
        );
        assert_eq!(t.save(&run, &Part::Menu).unwrap(), Saved::Nothing);
        let fresh = d.join("fresh");
        assert!(matches!(
            t.populate(&fresh).unwrap(),
            Populated::Warm { files: 2, .. }
        ));
        assert!(!utility(&fresh).0.join(SLOTS[1]).exists());
        assert!(t.seed("lockout").is_empty());

        let empty = d.join("empty");
        data_with(&empty, &[(SLOTS[0], vec![0; 8]), (PREFERENCES, prefs(1))]);
        let t2 = Template::new(d.join("t"), "k2".into());
        assert_eq!(t2.save(&empty, &Part::Utility).unwrap(), Saved::Nothing);
        assert!(!t2.dir().exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn n_slots_are_saved_per_map_and_seed_later_runs() {
        let d = scratch("n-parts");
        let run = d.join("runs/a/data");
        n_with(
            &run,
            &[
                (MENU_SLOT, slot_of(MENU, 7)),
                (MAP_SLOT, slot_of("lockout", 8)),
            ],
        );
        let t = Template::new(d.join("image/sim-cache/utility"), "k".into());
        assert_eq!(
            t.missing("lockout"),
            [Part::Utility, Part::Menu, Part::Map("lockout".into())]
        );
        assert!(t.seed("lockout").is_empty());
        assert!(matches!(
            t.save(&run, &Part::Menu).unwrap(),
            Saved::Saved { files: 1, .. }
        ));
        assert!(matches!(
            t.save(&run, &Part::Map("lockout".into())).unwrap(),
            Saved::Saved { files: 1, .. }
        ));
        // Slot 3 holds Lockout, so it is no other map's.
        assert_eq!(
            t.save(&run, &Part::Map("midship".into())).unwrap(),
            Saved::Nothing
        );
        assert_eq!(t.save(&run, &Part::Menu).unwrap(), Saved::Exists);

        let menu = t.part_dir(&Part::Menu);
        let lockout = t.part_dir(&Part::Map("lockout".into()));
        assert_eq!(t.seed("lockout"), [menu.clone(), lockout.clone()]);
        assert_eq!(t.seed("midship"), std::slice::from_ref(&menu));
        assert_eq!(t.missing("lockout"), [Part::Utility]);
        assert_eq!(
            t.missing("midship"),
            [Part::Utility, Part::Map("midship".into())]
        );
        // Each part holds its slot and record and nothing else of the run.
        assert_eq!(
            fs::read(lockout.join("disk").join(MAP_SLOT)).unwrap(),
            slot_of("lockout", 8)
        );
        assert_eq!(
            fs::read_to_string(
                lockout
                    .join("metadata")
                    .join(record_dir(MAP_SLOT))
                    .join("record")
            )
            .unwrap(),
            format!("HVF1 {MAP_SLOT}")
        );
        let names: Vec<String> = fs::read_dir(lockout.join("disk"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [MAP_SLOT]);
        assert!(!menu.join("disk").join("cache004.map").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn n_is_seeded_only_behind_a_warm_z_and_slot_2() {
        let d = scratch("seed-order");
        let src = d.join("src");
        z_with(&src, 1);
        n_with(
            &src,
            &[
                (MENU_SLOT, slot_of(MENU, 2)),
                (MAP_SLOT, slot_of("lockout", 3)),
            ],
        );
        let t = Template::new(d.join("t"), "k".into());
        let lockout = Part::Map("lockout".into());
        // Slot 3 alone: slot 2 would be empty and the boot deletes slot 3.
        t.save(&src, &lockout).unwrap();
        assert!(t.seed("lockout").is_empty());
        t.save(&src, &Part::Menu).unwrap();
        assert_eq!(t.seed("lockout").len(), 2);
        // Without a Z: template the run is cold and N: is not seeded.
        let gate = Gate::default();
        let cold = start(Some(&t), &gate, &d.join("r0"), "lockout");
        assert!(cold.seed().is_empty());
        assert_eq!(cold.saves(), Some(&[Part::Utility][..]));
        drop(cold);
        t.save(&src, &Part::Utility).unwrap();
        let warm = start(Some(&t), &gate, &d.join("r1"), "lockout");
        assert_eq!(warm.seed().len(), 2);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_run_that_already_has_z_files_is_left_alone() {
        let d = scratch("occupied");
        let src = d.join("src");
        z_with(&src, 1);
        let t = Template::new(d.join("t"), "k".into());
        t.save(&src, &Part::Utility).unwrap();
        let run = d.join("run");
        data_with(&run, &[(SLOTS[1], slot(9))]);
        assert_eq!(t.populate(&run).unwrap(), Populated::Occupied);
        assert!(!utility(&run).0.join(SLOTS[0]).exists());
        assert_eq!(fs::read(utility(&run).0.join(SLOTS[1])).unwrap(), slot(9));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn concurrent_saves_leave_one_whole_part() {
        let d = scratch("race");
        let t = Template::new(d.join("t"), "k".into());
        let runs: Vec<PathBuf> = (0..6u8)
            .map(|i| {
                let r = d.join(format!("run{i}"));
                z_with(&r, i + 1);
                n_with(&r, &[(MAP_SLOT, slot_of("lockout", i + 1))]);
                r
            })
            .collect();
        let lockout = Part::Map("lockout".into());
        let results: Vec<(Saved, Saved)> = std::thread::scope(|s| {
            let hs: Vec<_> = runs
                .iter()
                .map(|r| {
                    s.spawn(|| {
                        (
                            t.save(r, &Part::Utility).unwrap(),
                            t.save(r, &lockout).unwrap(),
                        )
                    })
                })
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for part in 0..2 {
            let got: Vec<Saved> = results
                .iter()
                .map(|r| if part == 0 { r.0 } else { r.1 })
                .collect();
            let saved = got
                .iter()
                .filter(|r| matches!(r, Saved::Saved { .. }))
                .count();
            assert_eq!(saved, 1, "{got:?}");
            assert!(
                got.iter()
                    .all(|r| matches!(r, Saved::Saved { .. } | Saved::Exists))
            );
        }
        // Every Z: file comes from the same run.
        let z = t.part_dir(&Part::Utility).join("disk");
        let a = fs::read(z.join(SLOTS[0])).unwrap();
        let b = fs::read(z.join(SLOTS[1])).unwrap();
        assert_eq!(a, b);
        assert_eq!(fs::read(z.join(PREFERENCES)).unwrap()[0], a[HEADER]);
        let names: Vec<String> = fs::read_dir(d.join("t"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["k"]);
        // A later save keeps the part it found.
        assert_eq!(t.save(&runs[5], &Part::Utility).unwrap(), Saved::Exists);
        assert_eq!(fs::read(z.join(SLOTS[0])).unwrap(), a);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_new_key_replaces_the_old_templates() {
        let d = scratch("prune");
        let run = d.join("run");
        z_with(&run, 1);
        n_with(
            &run,
            &[
                (MENU_SLOT, slot_of(MENU, 2)),
                (MAP_SLOT, slot_of("lockout", 3)),
            ],
        );
        let old = Template::new(d.join("t"), "old".into());
        old.save(&run, &Part::Utility).unwrap();
        old.save(&run, &Part::Map("lockout".into())).unwrap();
        // A fresh staging directory may be another run's save in progress.
        fs::create_dir_all(d.join("t/.staging-new-1-2")).unwrap();
        let new = Template::new(d.join("t"), "new".into());
        // Saving an N: part alone prunes too.
        new.save(&run, &Part::Menu).unwrap();
        assert!(!old.dir().exists());
        assert!(new.part_dir(&Part::Menu).exists());
        assert!(d.join("t/.staging-new-1-2").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn removing_a_run_data_leaves_the_template() {
        let d = scratch("cleanup");
        let data = d.join("out/runs/r1/data");
        z_with(&data, 3);
        n_with(&data, &[(MENU_SLOT, slot_of(MENU, 4))]);
        let t = Template::new(d.join("image/sim-cache/utility"), "k".into());
        t.save(&data, &Part::Utility).unwrap();
        t.save(&data, &Part::Menu).unwrap();
        let warm = d.join("out/runs/r2/data");
        t.populate(&warm).unwrap();
        // What `h2 sim batch` does after each run.
        fs::remove_dir_all(&data).unwrap();
        fs::remove_dir_all(&warm).unwrap();
        assert_eq!(
            fs::read(t.part_dir(&Part::Utility).join("disk").join(SLOTS[0])).unwrap(),
            slot(3)
        );
        assert_eq!(
            fs::read(t.part_dir(&Part::Menu).join("disk").join(MENU_SLOT)).unwrap(),
            slot_of(MENU, 4)
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn one_run_a_map_makes_the_template_while_that_map_waits() {
        let d = scratch("gate");
        let src = d.join("src");
        z_with(&src, 4);
        n_with(
            &src,
            &[
                (MENU_SLOT, slot_of(MENU, 5)),
                (MAP_SLOT, slot_of("lockout", 6)),
            ],
        );
        let t = Template::new(d.join("t"), "k".into());
        let gate = Gate::default();
        let producer = start(Some(&t), &gate, &d.join("r0"), "lockout");
        assert_eq!(
            producer.saves(),
            Some(&[Part::Utility, Part::Menu, Part::Map("lockout".into())][..])
        );
        assert!(producer.seed().is_empty());
        // Another map claims its own.
        let other = start(Some(&t), &gate, &d.join("o0"), "midship");
        assert!(other.saves().is_some());
        drop(other);
        let finished = Arc::new(AtomicBool::new(false));
        std::thread::scope(|s| {
            let waiter = s.spawn(|| {
                let got = start(Some(&t), &gate, &d.join("r1"), "lockout");
                // It only went on once the producer was done.
                assert!(finished.load(Ordering::SeqCst));
                assert!(got.saves().is_none());
                (got.seed().to_vec(), got.label())
            });
            std::thread::sleep(Duration::from_millis(50));
            // The producer's run finishes and saves.
            finished.store(true, Ordering::SeqCst);
            for part in producer.saves().unwrap() {
                t.save(&src, part).unwrap();
            }
            drop(producer);
            let (seed, label) = waiter.join().unwrap();
            assert_eq!(
                seed,
                [
                    t.part_dir(&Part::Menu),
                    t.part_dir(&Part::Map("lockout".into()))
                ]
            );
            let how = if vfs_clones(&d) {
                "cloned"
            } else {
                "copied, no clone"
            };
            assert_eq!(
                label,
                format!("  warm Z: (3 files {how}), warm N: (2 of 2 slots seeded)")
            );
        });
        assert_eq!(
            fs::read(utility(&d.join("r1")).0.join(SLOTS[1])).unwrap(),
            slot(4)
        );
        let _ = fs::remove_dir_all(&d);
    }

    /// Whether this filesystem clones (APFS) rather than copies.
    fn vfs_clones(d: &Path) -> bool {
        let (a, b) = (d.join("probe-a"), d.join("probe-b"));
        fs::write(&a, b"x").unwrap();
        let cloned = clone_or_copy(&a, &b).unwrap();
        let _ = (fs::remove_file(&a), fs::remove_file(&b));
        cloned
    }

    #[test]
    fn a_producer_that_saves_nothing_leaves_the_rest_cold() {
        let d = scratch("gate-fail");
        let t = Template::new(d.join("t"), "k".into());
        let gate = Gate::default();
        let producer = start(Some(&t), &gate, &d.join("r0"), "lockout");
        assert!(producer.saves().is_some());
        assert_eq!(producer.label(), "  cold Z:, cold N:, saving the template");
        drop(producer);
        // One claim a map: later runs neither wait nor produce.
        for r in ["r1", "r2"] {
            let s = start(Some(&t), &gate, &d.join(r), "lockout");
            assert!(s.saves().is_none());
            assert_eq!(s.label(), "  cold Z:, cold N:");
        }
        let off = start(None, &gate, &d.join("r3"), "lockout");
        assert!(matches!(off, Start::Off));
        assert!(off.seed().is_empty());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_whole_template_needs_no_producer() {
        let d = scratch("whole");
        let src = d.join("src");
        z_with(&src, 1);
        n_with(
            &src,
            &[
                (MENU_SLOT, slot_of(MENU, 2)),
                (MAP_SLOT, slot_of("lockout", 3)),
            ],
        );
        let t = Template::new(d.join("t"), "k".into());
        for part in t.missing("lockout") {
            t.save(&src, &part).unwrap();
        }
        let gate = Gate::default();
        let s = start(Some(&t), &gate, &d.join("r"), "lockout");
        assert!(s.saves().is_none());
        assert_eq!(s.seed().len(), 2);
        // An Occupied Z: never becomes the template, and N: still seeds.
        let t2 = Template::new(d.join("t2"), "k".into());
        let run = d.join("occupied");
        data_with(&run, &[(SLOTS[0], slot(9))]);
        let s = start(Some(&t2), &gate, &run, "midship");
        assert_eq!(
            s.saves(),
            Some(&[Part::Menu, Part::Map("midship".into())][..])
        );
        let _ = fs::remove_dir_all(&d);
    }
}
