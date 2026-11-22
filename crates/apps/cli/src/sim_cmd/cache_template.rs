//! Sim batch runs start with Z:'s map cache already filled
//! (docs/runtime/cache-slots.md, "Sim batches: a warm Z:").
//!
//! Each batch run has a data directory of its own, so the title copies
//! `shared.map` and `single_player_shared.map` from the disc into Z:'s cache
//! slots 0 and 1 (`cache000.map`, `cache001.map`) on every boot. Z: keeps its
//! slots across boots (XapiInitProcess mounts it without a format), and a
//! boot that finds a valid slot header skips that map's copy. N: (slots 2 to
//! 5: the main menu, the multiplayer map, campaign levels) is formatted by
//! `XMountCacheDrive` on every boot, so it cannot be kept warm here.
//!
//! The first run of an image that fills Z: saves its slot files as a
//! **template**, `<out>/sim-cache/utility/<key>/`, keyed by the image's build
//! stamp, the disc and this `h2` binary. Later runs get the template's files
//! as APFS clones (copy-on-write, so N runs cost about one copy); elsewhere
//! they are copied. `H2_SIM_CACHE_TEMPLATE=0` turns it off.

use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// `0`, `off`, `false` or `no` turns the template off.
pub const ENV: &str = "H2_SIM_CACHE_TEMPLATE";
/// Z:'s cache slots: slot 0 (shared) and slot 1 (single-player shared).
pub const SLOTS: [&str; 2] = ["cache000.map", "cache001.map"];
/// Bumped when the template's layout or the rules for a valid slot change.
const LAYOUT: u32 = 1;
/// A map's header: `head` at 0, `foot` at 0x7fc, both little-endian.
const HEADER: usize = 0x800;
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
/// template.
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

/// The host directories of the data root's utility partition (Z:): its
/// files and its FATX metadata records (`vfs::Vfs::with_storage`).
fn utility(data: &Path) -> (PathBuf, PathBuf) {
    let name = format!("Partition{}", vfs::Config::new(data, 0).utility_partition);
    (
        data.join("disk").join(&name),
        data.join("metadata").join(name),
    )
}

/// The directory of a root file's metadata record: the lowercase name's
/// bytes in hex (`record_path` in crates/runtime/vfs/src/host.rs).
fn record_dir(name: &str) -> String {
    name.bytes()
        .map(|b| format!("{:02x}", b.to_ascii_lowercase()))
        .collect()
}

/// Whether `path` holds a published map header. The copier writes a zero
/// header before the payload and the map's header only after the payload
/// is flushed, so a valid header means a finished copy.
pub fn complete(path: &Path) -> bool {
    let mut header = [0u8; HEADER];
    fs::File::open(path)
        .and_then(|mut f| f.read_exact(&mut header))
        .is_ok()
        && &header[..4] == b"daeh"
        && &header[HEADER - 4..] == b"toof"
}

/// Clones `src` to the new file `dst` (APFS `fclonefileat`), else copies it.
/// True if it was cloned.
#[cfg_attr(target_vendor = "apple", allow(unsafe_code))]
pub fn clone_or_copy(src: &Path, dst: &Path) -> io::Result<bool> {
    #[cfg(target_vendor = "apple")]
    {
        use std::{ffi::CString, os::fd::AsRawFd, os::unix::ffi::OsStrExt};
        let source = fs::File::open(src)?;
        let target = CString::new(dst.as_os_str().as_bytes())?;
        // SAFETY: the source descriptor stays open for the call and the
        // destination is a NUL-terminated path that outlives it.
        if unsafe { libc::fclonefileat(source.as_raw_fd(), libc::AT_FDCWD, target.as_ptr(), 0) }
            == 0
        {
            return Ok(true);
        }
        match fs::remove_file(dst) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    // Linux's copy_file_range shares extents where the filesystem can.
    fs::copy(src, dst)?;
    Ok(false)
}

/// What [`Template::populate`] did to a run's data directory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Populated {
    /// The template's slots are in place.
    Warm { slots: usize, cloned: bool },
    /// The data already has a Z: slot file; nothing was touched.
    Occupied,
    /// There is no template yet.
    Miss,
}

/// What [`Template::save`] did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Saved {
    Saved {
        slots: usize,
        cloned: bool,
    },
    /// A template with this key is already there (another run's).
    Exists,
    /// The run left no finished Z: slot.
    NoSlots,
}

/// One image's Z: template.
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
            println!("map cache: {ENV} is off; every run copies Z:'s maps");
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
                println!("map cache: no template ({e}); every run copies Z:'s maps");
                None
            }
        }
    }

    pub fn key(&self) -> &str {
        &self.key
    }

    /// `<root>/<key>`: `disk/` holds the slot files, `metadata/` their
    /// records.
    pub fn dir(&self) -> PathBuf {
        self.root.join(&self.key)
    }

    /// Puts the template's finished slots into `data`'s Z:, before the run
    /// boots. Leaves a Z: that already has a slot file alone. On an error,
    /// removes what it placed, so the run starts cold.
    pub fn populate(&self, data: &Path) -> io::Result<Populated> {
        let (disk, meta) = utility(data);
        if SLOTS.iter().any(|s| disk.join(s).exists()) {
            return Ok(Populated::Occupied);
        }
        let dir = self.dir();
        let (tdisk, tmeta) = (dir.join("disk"), dir.join("metadata"));
        let slots: Vec<&str> = SLOTS
            .into_iter()
            .filter(|s| complete(&tdisk.join(s)))
            .collect();
        if slots.is_empty() {
            return Ok(Populated::Miss);
        }
        let mut placed = Vec::new();
        let result = (|| -> io::Result<bool> {
            fs::create_dir_all(&disk)?;
            let mut cloned = true;
            for s in &slots {
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
                slots: slots.len(),
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

    /// Saves `data`'s finished Z: slots as the template, after a run that
    /// started cold has exited. Staged in a directory of its own and renamed
    /// into place, so concurrent savers leave one whole template. Removes
    /// other keys' templates once saved.
    pub fn save(&self, data: &Path) -> io::Result<Saved> {
        let dir = self.dir();
        if dir.exists() {
            return Ok(Saved::Exists);
        }
        let (disk, meta) = utility(data);
        let slots: Vec<&str> = SLOTS
            .into_iter()
            .filter(|s| complete(&disk.join(s)))
            .collect();
        if slots.is_empty() {
            return Ok(Saved::NoSlots);
        }
        fs::create_dir_all(&self.root)?;
        let staging = self.root.join(format!(
            ".staging-{}-{}-{}",
            self.key,
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos())
        ));
        let staged = (|| -> io::Result<bool> {
            let (sdisk, smeta) = (staging.join("disk"), staging.join("metadata"));
            fs::create_dir_all(&sdisk)?;
            fs::create_dir_all(&smeta)?;
            let mut cloned = true;
            for s in &slots {
                cloned &= clone_or_copy(&disk.join(s), &sdisk.join(s))?;
                let record = meta.join(record_dir(s)).join("record");
                if record.is_file() {
                    let to = smeta.join(record_dir(s));
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
                    slots: slots.len(),
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

/// Lets the first run that misses make the template while the other
/// workers of the batch wait for it, instead of every run of the first
/// wave copying Z: at once. One claim a batch: if that run saves nothing,
/// the rest run cold.
#[derive(Default)]
pub struct Gate {
    /// (claimed, producing)
    state: Mutex<(bool, bool)>,
    done: Condvar,
}

/// Held by the run making the template; dropping it lets the others go.
pub struct Producing<'a>(&'a Gate);

impl Drop for Producing<'_> {
    fn drop(&mut self) {
        self.0.state.lock().unwrap_or_else(|e| e.into_inner()).1 = false;
        self.0.done.notify_all();
    }
}

impl Gate {
    /// The first caller of the batch gets the guard.
    pub fn claim(&self) -> Option<Producing<'_>> {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        if s.0 {
            return None;
        }
        *s = (true, true);
        Some(Producing(self))
    }

    /// Waits while a claimed run is making the template.
    pub fn wait(&self) {
        let mut s = self.state.lock().unwrap_or_else(|e| e.into_inner());
        while s.1 {
            s = self.done.wait(s).unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// How one batch run starts.
pub enum Start<'a> {
    /// No template (switched off, or the image has no stamp).
    Off,
    /// From the template: its slot count, and whether they were cloned.
    Warm { slots: usize, cloned: bool },
    /// The data already had Z: slots.
    Occupied,
    /// Cold, and this run saves the template when it finishes.
    Producer(Producing<'a>),
    /// Cold.
    Cold,
}

impl Start<'_> {
    /// For the run's status line.
    pub fn label(&self) -> String {
        match self {
            Start::Off | Start::Occupied => String::new(),
            Start::Warm { slots, cloned } => format!(
                "  warm Z: ({slots} slots {})",
                if *cloned {
                    "cloned"
                } else {
                    "copied, no clone"
                }
            ),
            Start::Producer(_) => "  cold Z:, saving the template".into(),
            Start::Cold => "  cold Z:".into(),
        }
    }
}

/// Prepares `data` for one batch run: the template's slots if there is a
/// template, else the claim to make it, else (after waiting for the run
/// that claimed it) the template's slots or a cold start.
pub fn start<'a>(template: Option<&Template>, gate: &'a Gate, data: &Path) -> Start<'a> {
    let Some(t) = template else {
        return Start::Off;
    };
    for attempt in 0..2 {
        match t.populate(data) {
            Ok(Populated::Warm { slots, cloned }) => return Start::Warm { slots, cloned },
            Ok(Populated::Occupied) => return Start::Occupied,
            Ok(Populated::Miss) if attempt == 0 => {
                if let Some(p) = gate.claim() {
                    return Start::Producer(p);
                }
                gate.wait();
            }
            Ok(Populated::Miss) => {}
            Err(e) => {
                println!(
                    "map cache: template {} not used ({e}); this run copies Z:'s maps",
                    t.key()
                );
                return Start::Cold;
            }
        }
    }
    Start::Cold
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    fn scratch(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-sim-cache-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// A finished slot: a map header and a payload, synthetic bytes.
    fn slot(fill: u8) -> Vec<u8> {
        let mut b = vec![fill; HEADER + 4096];
        b[..4].copy_from_slice(b"daeh");
        b[HEADER - 4..HEADER].copy_from_slice(b"toof");
        b
    }

    /// A data root whose Z: holds `slots` (name, bytes), each with a
    /// metadata record, and a file that is not a cache slot.
    fn data_with(root: &Path, slots: &[(&str, Vec<u8>)]) {
        let (disk, meta) = utility(root);
        fs::create_dir_all(&disk).unwrap();
        for (name, bytes) in slots {
            fs::write(disk.join(name), bytes).unwrap();
            let r = meta.join(record_dir(name));
            fs::create_dir_all(&r).unwrap();
            fs::write(r.join("record"), format!("HVF1 {name}")).unwrap();
        }
        fs::write(disk.join("other.bin"), b"per run").unwrap();
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
    fn a_saved_template_populates_a_fresh_run_with_the_same_slots() {
        let d = scratch("roundtrip");
        let first = d.join("runs/a/data");
        data_with(&first, &[(SLOTS[0], slot(1)), (SLOTS[1], slot(2))]);
        let t = Template::new(d.join("image/sim-cache/utility"), "k1".into());
        assert_eq!(t.populate(&d.join("runs/b/data")).unwrap(), Populated::Miss);
        assert!(matches!(
            t.save(&first).unwrap(),
            Saved::Saved { slots: 2, .. }
        ));
        // The run's other files stay out of the template.
        assert!(!t.dir().join("disk/other.bin").exists());

        let next = d.join("runs/b/data");
        let got = t.populate(&next).unwrap();
        assert!(matches!(got, Populated::Warm { slots: 2, .. }), "{got:?}");
        let (disk, meta) = utility(&next);
        assert_eq!(fs::read(disk.join(SLOTS[0])).unwrap(), slot(1));
        assert_eq!(fs::read(disk.join(SLOTS[1])).unwrap(), slot(2));
        assert_eq!(
            fs::read_to_string(meta.join(record_dir(SLOTS[1])).join("record")).unwrap(),
            format!("HVF1 {}", SLOTS[1])
        );
        assert!(!disk.join("other.bin").exists());
        // A clone is a file of its own: writing it leaves the template.
        fs::write(disk.join(SLOTS[0]), b"changed").unwrap();
        assert_eq!(
            fs::read(t.dir().join("disk").join(SLOTS[0])).unwrap(),
            slot(1)
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn unfinished_slots_are_never_saved_or_used() {
        let d = scratch("unfinished");
        let run = d.join("run");
        // Slot 1's header is still the zero one the copier writes first.
        data_with(
            &run,
            &[(SLOTS[0], slot(1)), (SLOTS[1], vec![0; HEADER + 16])],
        );
        let t = Template::new(d.join("t"), "k".into());
        assert!(matches!(
            t.save(&run).unwrap(),
            Saved::Saved { slots: 1, .. }
        ));
        assert!(!t.dir().join("disk").join(SLOTS[1]).exists());
        let fresh = d.join("fresh");
        assert!(matches!(
            t.populate(&fresh).unwrap(),
            Populated::Warm { slots: 1, .. }
        ));
        assert!(!utility(&fresh).0.join(SLOTS[1]).exists());

        let empty = d.join("empty");
        data_with(&empty, &[(SLOTS[0], vec![0; 8])]);
        let t2 = Template::new(d.join("t"), "k2".into());
        assert_eq!(t2.save(&empty).unwrap(), Saved::NoSlots);
        assert!(!t2.dir().exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_run_that_already_has_z_slots_is_left_alone() {
        let d = scratch("occupied");
        let src = d.join("src");
        data_with(&src, &[(SLOTS[0], slot(1))]);
        let t = Template::new(d.join("t"), "k".into());
        t.save(&src).unwrap();
        let run = d.join("run");
        data_with(&run, &[(SLOTS[1], slot(9))]);
        assert_eq!(t.populate(&run).unwrap(), Populated::Occupied);
        assert!(!utility(&run).0.join(SLOTS[0]).exists());
        assert_eq!(fs::read(utility(&run).0.join(SLOTS[1])).unwrap(), slot(9));
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn concurrent_saves_leave_one_whole_template() {
        let d = scratch("race");
        let t = Template::new(d.join("t"), "k".into());
        let runs: Vec<PathBuf> = (0..6u8)
            .map(|i| {
                let r = d.join(format!("run{i}"));
                data_with(&r, &[(SLOTS[0], slot(i + 1)), (SLOTS[1], slot(i + 1))]);
                r
            })
            .collect();
        let results: Vec<Saved> = std::thread::scope(|s| {
            let hs: Vec<_> = runs
                .iter()
                .map(|r| s.spawn(|| t.save(r).unwrap()))
                .collect();
            hs.into_iter().map(|h| h.join().unwrap()).collect()
        });
        let saved = results
            .iter()
            .filter(|r| matches!(r, Saved::Saved { .. }))
            .count();
        assert_eq!(saved, 1, "{results:?}");
        assert!(
            results
                .iter()
                .all(|r| matches!(r, Saved::Saved { .. } | Saved::Exists))
        );
        // Both slots come from the same run.
        let a = fs::read(t.dir().join("disk").join(SLOTS[0])).unwrap();
        let b = fs::read(t.dir().join("disk").join(SLOTS[1])).unwrap();
        assert_eq!(a, b);
        let names: Vec<String> = fs::read_dir(d.join("t"))
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, ["k"]);
        // A later save keeps the template it found.
        assert_eq!(t.save(&runs[5]).unwrap(), Saved::Exists);
        assert_eq!(fs::read(t.dir().join("disk").join(SLOTS[0])).unwrap(), a);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_new_key_replaces_the_old_templates() {
        let d = scratch("prune");
        let run = d.join("run");
        data_with(&run, &[(SLOTS[0], slot(1))]);
        let old = Template::new(d.join("t"), "old".into());
        old.save(&run).unwrap();
        // A fresh staging directory may be another run's save in progress.
        fs::create_dir_all(d.join("t/.staging-new-1-2")).unwrap();
        let new = Template::new(d.join("t"), "new".into());
        new.save(&run).unwrap();
        assert!(!old.dir().exists());
        assert!(new.dir().exists());
        assert!(d.join("t/.staging-new-1-2").exists());
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn removing_a_run_data_leaves_the_template() {
        let d = scratch("cleanup");
        let data = d.join("out/runs/r1/data");
        data_with(&data, &[(SLOTS[0], slot(3))]);
        let t = Template::new(d.join("image/sim-cache/utility"), "k".into());
        t.save(&data).unwrap();
        let warm = d.join("out/runs/r2/data");
        t.populate(&warm).unwrap();
        // What `h2 sim batch` does after each run.
        fs::remove_dir_all(&data).unwrap();
        fs::remove_dir_all(&warm).unwrap();
        assert_eq!(
            fs::read(t.dir().join("disk").join(SLOTS[0])).unwrap(),
            slot(3)
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn one_run_makes_the_template_while_the_others_wait() {
        let d = scratch("gate");
        let src = d.join("src");
        data_with(&src, &[(SLOTS[0], slot(4)), (SLOTS[1], slot(5))]);
        let t = Template::new(d.join("t"), "k".into());
        let gate = Gate::default();
        let producer = start(Some(&t), &gate, &d.join("r0"));
        assert!(matches!(producer, Start::Producer(_)));
        let finished = Arc::new(AtomicBool::new(false));
        std::thread::scope(|s| {
            let waiter = s.spawn(|| {
                let got = start(Some(&t), &gate, &d.join("r1"));
                // It only went on once the producer was done.
                assert!(finished.load(Ordering::SeqCst));
                matches!(got, Start::Warm { .. })
            });
            std::thread::sleep(Duration::from_millis(50));
            // The producer's run finishes and saves.
            finished.store(true, Ordering::SeqCst);
            t.save(&src).unwrap();
            drop(producer);
            assert!(waiter.join().unwrap());
        });
        assert_eq!(
            fs::read(utility(&d.join("r1")).0.join(SLOTS[1])).unwrap(),
            slot(5)
        );
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_producer_that_saves_nothing_leaves_the_rest_cold() {
        let d = scratch("gate-fail");
        let t = Template::new(d.join("t"), "k".into());
        let gate = Gate::default();
        let producer = start(Some(&t), &gate, &d.join("r0"));
        assert!(matches!(producer, Start::Producer(_)));
        drop(producer);
        // One claim a batch: later runs neither wait nor produce.
        assert!(matches!(start(Some(&t), &gate, &d.join("r1")), Start::Cold));
        assert!(matches!(start(Some(&t), &gate, &d.join("r2")), Start::Cold));
        assert!(matches!(start(None, &gate, &d.join("r3")), Start::Off));
        let _ = fs::remove_dir_all(&d);
    }
}
