//! `state.save`, `state.load` and the rest in the boot
//! (docs/runtime/snapshots.md, phase 5).
//!
//! The agent queues a save or load and, while the game ticks, ends the
//! slice right after the next tick (crates/agent's `snap`). The boot loop
//! then builds a [`BootMachine`] over everything that holds state (the
//! [`Host`] subsystems, the kernel machine, the feature plan) and hands it
//! to `Agent::run_snapshots`, between slices, with no guest code on the
//! host stack.
//!
//! A save runs only when nothing anywhere is live: the machine is
//! quiescent (`Machine::quiescent`), every feature's policy says so, and
//! every host subsystem's blockers are empty (d3d, dsound and its output,
//! XAPI, a Bink movie, Xbox Live). It writes the parts in [`SAVE_ORDER`]:
//! the device first (it writes render targets back into guest RAM), then
//! the audio, then guest RAM and the TSC, the kernel and its threads,
//! XAPI with the input tap's cursor, and the feature sections; last a
//! `ramhash` section, `lockstep::hash_guest` of the RAM saved. Feature
//! policies are asked again after the save: one that went live meanwhile
//! (a co-op thread changing the role) throws the file away. A load puts
//! the parts back in [`LOAD_ORDER`] (guest RAM before the kernel, the
//! kernel before dsound, whose pins adopt the loaded kernel's references)
//! and compares the RAM hash.
//!
//! Snapshots live in the store ([`Shelf`], `snapshot::Store`): the user's
//! cache directory, one directory per build key. The key is the image's
//! build stamp, the switches, the installed features and the host parts,
//! so a snapshot only loads into a run made the same way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use agent_proto::state::{HashCheck, List, Listed, Loaded, Saved};
use agent_proto::{At, RpcError, code};
use snapshot::codec::{Reader, Writer};
use snapshot::{BuildKey, Codec, Header, Snapshot, SnapshotWriter, Store, Tag};

/// One piece of the machine with sections of its own.
pub trait Part {
    /// Its place in [`SAVE_ORDER`] and [`LOAD_ORDER`].
    fn name(&self) -> &'static str;
    /// Why it cannot be saved or loaded now; empty when it can.
    fn blockers(&self) -> Vec<String> {
        Vec::new()
    }
    fn film_load_blockers(&self) -> Vec<String> {
        self.blockers()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String>;
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String>;
    /// Film anchors supply physical RAM only. Device and feature sections
    /// are always complete for the selected keyframe.
    fn save_with_ram_base(&mut self, out: &mut SnapshotWriter, _base: &snapshot::Base<'_>) -> Result<(), String> {
        self.save(out)
    }
    fn load_with_ram_base(&mut self, snap: &Snapshot<'_>, _base: &snapshot::Base<'_>) -> Result<(), String> {
        self.load(snap)
    }

    /// [`Part::load`] with guest RAM mapped copy-on-write over `ram`, an
    /// uncompressed RAM image the save wrote beside the snapshot. The
    /// default loads normally; the guest part is the one that can do this.
    fn load_cow(&mut self, snapshot: &Snapshot<'_>, ram: &Path) -> Result<(), String> {
        let _ = ram;
        self.load(snapshot)
    }
}

/// The device first: its save writes guest memory. Guest RAM after every
/// part that may write it.
pub const SAVE_ORDER: [&str; 9] = [
    "d3d", "dsound", "audioout", "wma", "livefilm", "guest", "kernel", "xapi", "features",
];
/// Recreate taped Live HLE claims before guest RAM. The kernel precedes
/// dsound (its pins adopt the loaded
/// kernel's references); features last, over a loaded machine.
pub const LOAD_ORDER: [&str; 9] = [
    "livefilm", "guest", "kernel", "d3d", "dsound", "audioout", "wma", "xapi", "features",
];

/// `lockstep::hash_guest` of the RAM saved.
pub const RAMHASH: Tag = Tag::new("ramhash");
const RAMHASH_VERSION: u32 = 1;

/// Writes every part in [`SAVE_ORDER`], then the RAM hash. A part with no
/// place in the order is refused before anything is written.
pub fn save_parts(
    parts: &mut [Box<dyn Part + '_>],
    header: Header,
    hash: &dyn Fn() -> lockstep::RamHash,
) -> Result<Vec<u8>, String> {
    save_parts_observing(parts, header, hash, None, |_, _| {})
}

/// Reports each attempted section immediately, including a section which
/// stopped the save. D3D's capture has guest-visible effects even if a
/// later part prevents the recorder from publishing a complete keyframe.
pub(crate) fn save_parts_observing(
    parts: &mut [Box<dyn Part + '_>],
    header: Header,
    hash: &dyn Fn() -> lockstep::RamHash,
    ram_base: Option<&snapshot::Base<'_>>,
    mut attempted: impl FnMut(&str, bool),
) -> Result<Vec<u8>, String> {
    check_names(parts)?;
    let mut out = SnapshotWriter::new(header);
    for name in SAVE_ORDER {
        if let Some(p) = parts.iter_mut().find(|p| p.name() == name) {
            let result = match ram_base {
                Some(base) if name == "guest" => p.save_with_ram_base(&mut out, base),
                _ => p.save(&mut out),
            };
            attempted(name, result.is_ok());
            result.map_err(|e| format!("{name}: {e}"))?;
        }
    }
    let h = hash();
    let mut w = Writer::new();
    w.u64(h.total).list(&h.chunks, |w, c| {
        w.u64(*c);
    });
    out.section(RAMHASH, RAMHASH_VERSION, Codec::Stored, w.into_bytes())
        .map_err(|e| e.to_string())?;
    out.finish().map_err(|e| e.to_string())
}

/// Loads every part in [`LOAD_ORDER`]; the names of the parts loaded. A
/// failure part way leaves the machine part loaded, and says so. `cow`:
/// the guest part loads its RAM copy-on-write over that image instead of
/// decoding the snapshot's RAM in.
pub fn load_parts(
    parts: &mut [Box<dyn Part + '_>],
    snapshot: &Snapshot<'_>,
    cow: Option<&Path>,
) -> Result<Vec<String>, String> {
    load_parts_with_base(parts, snapshot, cow, None)
}

pub(crate) fn load_parts_with_base(
    parts: &mut [Box<dyn Part + '_>],
    snapshot: &Snapshot<'_>,
    cow: Option<&Path>,
    ram_base: Option<&snapshot::Base<'_>>,
) -> Result<Vec<String>, String> {
    check_names(parts)?;
    let mut applied = Vec::new();
    for name in LOAD_ORDER {
        if let Some(p) = parts.iter_mut().find(|p| p.name() == name) {
            let loaded = if name == "guest" {
                match (ram_base, cow) {
                    (Some(base), _) => p.load_with_ram_base(snapshot, base),
                    (None, Some(ram)) => p.load_cow(snapshot, ram),
                    (None, None) => p.load(snapshot),
                }
            } else {
                p.load(snapshot)
            };
            loaded.map_err(|e| {
                let done = if applied.is_empty() {
                    "nothing was loaded".to_owned()
                } else {
                    format!(
                        "{} loaded before it: load again or restart",
                        applied.join(", ")
                    )
                };
                format!("{name}: {e} ({done})")
            })?;
            applied.push(name.to_owned());
        }
    }
    Ok(applied)
}

fn check_names(parts: &[Box<dyn Part + '_>]) -> Result<(), String> {
    match parts.iter().find(|p| !SAVE_ORDER.contains(&p.name())) {
        Some(p) => Err(format!("{} has no place in the save order", p.name())),
        None => Ok(()),
    }
}

/// The RAM hash after a load against the saved one.
pub fn check_hash(snapshot: &Snapshot<'_>, now: &lockstep::RamHash) -> HashCheck {
    let saved = snapshot
        .require(RAMHASH, RAMHASH_VERSION..=RAMHASH_VERSION)
        .ok()
        .and_then(|bytes| {
            let mut r = Reader::new(&bytes, "ramhash");
            let total = r.u64().ok()?;
            let chunks = r.list(8, |r| r.u64()).ok()?;
            r.finish().ok()?;
            Some(lockstep::RamHash { total, chunks })
        });
    let Some(saved) = saved else {
        return HashCheck::default();
    };
    let differing_mib: Vec<u32> = saved
        .chunks
        .iter()
        .zip(&now.chunks)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, _)| i as u32)
        .take(16)
        .collect();
    HashCheck {
        checked: true,
        equal: saved == *now,
        differing_mib,
    }
}

/// The store for this run's build key.
#[derive(Clone, Debug)]
pub struct Shelf {
    root: PathBuf,
    build: BuildKey,
}

impl Shelf {
    pub fn new(root: &Path, build: BuildKey) -> Shelf {
        Shelf {
            root: root.to_owned(),
            build,
        }
    }

    /// Under `snapshot::store::default_root` (`$H2_SNAPSHOTS`, else the
    /// cache directory); `None` without a home directory.
    pub fn from_env(build: BuildKey) -> Option<Shelf> {
        Some(Shelf::new(&snapshot::store::default_root()?, build))
    }

    pub fn build(&self) -> &BuildKey {
        &self.build
    }

    fn store(&self) -> Store {
        Store::new(&self.root, &self.build)
    }
}

fn unavailable(e: impl std::fmt::Display) -> RpcError {
    RpcError::new(code::UNAVAILABLE, e.to_string())
}

impl agent::Snapshots for Shelf {
    fn list(&mut self) -> Result<List, RpcError> {
        let store = self.store();
        let snapshots = store
            .list()
            .map_err(unavailable)?
            .into_iter()
            .map(|l| Listed {
                name: l.name,
                bytes: l.bytes,
                tick: l.header.tick,
                presents: l.header.presents,
                notes: l.header.notes.into_iter().collect(),
            })
            .collect();
        Ok(List {
            dir: store.dir().display().to_string(),
            snapshots,
        })
    }

    fn remove(&mut self, name: &str) -> Result<(), RpcError> {
        self.store().remove(name).map_err(|e| match e {
            snapshot::Error::NoSuchSnapshot(_) | snapshot::Error::BadName(_) => {
                RpcError::new(code::INVALID_PARAMS, e.to_string())
            }
            e => unavailable(e),
        })
    }

    fn exists(&mut self, name: &str) -> bool {
        snapshot::store::check_name(name).is_ok()
            && self
                .store()
                .dir()
                .join(format!("{name}.{}", snapshot::store::EXTENSION))
                .is_file()
    }
}

/// The build key: the image's build stamp, and the switches, features and
/// host parts that decide what is hooked and what holds state.
pub fn build_key(stamp: String, switches: &str, plan: &feature::Plan, host: &[&str]) -> BuildKey {
    let features: Vec<&str> = plan.snapshot.iter().map(|f| f.id).collect();
    BuildKey {
        stamp,
        switches: format!(
            "switches {switches}; features {}; host {}",
            features.join(","),
            host.join(",")
        ),
    }
}

/// The host subsystems a snapshot covers, whichever this run has.
pub struct Host<'a> {
    pub rt: &'a runtime::Runtime,
    pub d3d: Option<&'a dyn crate::d3d_glue::DeviceSnapshot>,
    pub audio: Option<&'a crate::dsound_glue::Audio>,
    pub xapi: Option<&'a crate::xapi_glue::Shared>,
    pub bink: Option<&'a bink_hle::BinkHle>,
    pub live: Option<&'a live_hle::Hle>,
}

impl Host<'_> {
    /// The parts this run has, for the build key.
    pub fn names(&self) -> Vec<&'static str> {
        let mut names = vec!["guest", "kernel"];
        if self.d3d.is_some() {
            names.push("d3d");
        }
        if let Some(a) = self.audio {
            names.extend(["dsound", "audioout"]);
            if a.wma.is_some() {
                names.push("wma");
            }
        }
        if self.xapi.is_some() {
            names.push("xapi");
        }
        names.push("features");
        if self.bink.is_some() {
            names.push("bink");
        }
        if self.live.is_some() {
            names.push(if self.live.is_some_and(|l| l.film_service_taped()) {
                "livefilm"
            } else {
                "live"
            });
        }
        names
    }

    /// Host blockers, `(owner, why)`: the device and audio parts', and
    /// what has no section (a movie playing, Xbox Live signed in). The
    /// probe reports these; [`BootMachine`] adds the kernel's and the
    /// features'.
    pub fn blockers(&self) -> Vec<(String, String)> {
        let mut out = Vec::new();
        for p in self.device_parts() {
            out.extend(p.blockers().into_iter().map(|w| (p.name().to_owned(), w)));
        }
        out.extend(
            self.checks()
                .into_iter()
                .map(|(owner, why)| (owner.to_owned(), why)),
        );
        out
    }

    fn checks(&self) -> Vec<(&'static str, String)> {
        let mut out = Vec::new();
        if let Some(b) = self.bink {
            let open = b.open_streams();
            if open > 0 {
                out.push(("bink", format!("{open} movie streams open")));
            }
        }
        if let Some(l) = self.live {
            if crate::theater::in_keyframe() && !l.film_service_taped() {
                out.push(("live", "local film has untaped online service state".into()));
            }
            for c in 0..4 {
                if l.user(c) != 0 && !(crate::theater::in_keyframe() && l.film_service_taped()) {
                    out.push(("live", format!("signed in on controller {c}")));
                }
            }
        }
        if crate::coop_live::lobby().is_some_and(|l| l.in_coop())
            && !(crate::theater::in_keyframe() && self.live.is_some_and(|l| l.film_service_taped()))
        {
            out.push(("coop", "hosting or joined to a co-op session".to_owned()));
        }
        out
    }

    /// Every part but the kernel and the features.
    fn device_parts(&self) -> Vec<Box<dyn Part + '_>> {
        let mut parts: Vec<Box<dyn Part + '_>> = Vec::new();
        if let Some(live) = self.live.filter(|l| l.film_service_taped()) {
            parts.push(Box::new(FilmNetwork(live, self.rt)));
        }
        parts.push(Box::new(GuestPart(self.rt)));
        if let Some(d) = self.d3d {
            parts.push(Box::new(D3dPart(d, self.rt)));
        }
        if let Some(a) = self.audio {
            parts.push(Box::new(DsoundPart(a)));
            parts.push(Box::new(AudioOutPart(a)));
            if let Some(w) = &a.wma {
                parts.push(Box::new(WmaPart(w)));
            }
        }
        if let Some(x) = self.xapi {
            parts.push(Box::new(XapiPart(x)));
        }
        parts
    }

    /// The machine `Agent::run_snapshots` works on, between slices.
    pub fn machine<'a>(
        &'a self,
        m: &'a mut kernel::Machine,
        plan: &'a feature::Plan,
        rebuilders: &'a kernel::snapshot::Rebuilders,
        shelf: &'a Shelf,
        presents: u64,
    ) -> BootMachine<'a> {
        let mut parts = self.device_parts();
        parts.push(Box::new(KernelPart { m, rebuilders }));
        parts.push(Box::new(FeaturesPart { plan, rt: self.rt }));
        let rt = self.rt;
        BootMachine {
            parts,
            checks: Box::new(move || self.checks()),
            shelf: shelf.clone(),
            hash: Box::new(move || lockstep::hash_guest(&rt.memory())),
            presents,
            rt: Some(rt),
        }
    }
}

/// Recreate dynamic Live HLE claims before Guest::restore verifies the
/// fixed layout. Loading this section requires an isolated playback HLE.
struct FilmNetwork<'a>(&'a live_hle::Hle, &'a runtime::Runtime);
impl Part for FilmNetwork<'_> {
    fn name(&self) -> &'static str {
        "livefilm"
    }
    fn blockers(&self) -> Vec<String> {
        if crate::theater::in_keyframe() {
            Vec::new()
        } else {
            vec!["the live service cursor belongs to a film keyframe".into()]
        }
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        let bytes = self.0.save_film_network(&self.1.memory())?;
        out.section(Tag::new("livefilm"), 1, Codec::Zstd, bytes)
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        let bytes = snapshot
            .require(Tag::new("livefilm"), 1..=1)
            .map_err(|e| e.to_string())?;
        self.0.load_film_network(&bytes, &mut self.1.memory())
    }
}

/// The whole machine for a save or load.
pub struct BootMachine<'a> {
    pub parts: Vec<Box<dyn Part + 'a>>,
    /// Blockers with no part: `(owner, why)`.
    pub checks: Box<dyn Fn() -> Vec<(&'static str, String)> + 'a>,
    pub shelf: Shelf,
    pub hash: Box<dyn Fn() -> lockstep::RamHash + 'a>,
    pub presents: u64,
    /// The runtime, for the raw RAM image a cow save writes and a cow
    /// load maps. `None` in tests, which exercise no cow path.
    pub rt: Option<&'a runtime::Runtime>,
}

impl BootMachine<'_> {
    fn notes(at: &At) -> Vec<(String, String)> {
        let mut notes = vec![("agent_ticks".to_owned(), at.ticks.to_string())];
        let phase = serde_json::to_value(at.phase)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned));
        for (k, v) in [
            ("map", at.map.clone()),
            ("mode", at.mode.clone()),
            ("bsp", at.bsp.map(|b| b.to_string())),
            ("map_ticks", Some(at.map_ticks.to_string())),
            ("phase", phase),
        ] {
            if let Some(v) = v {
                notes.push((k.to_owned(), v));
            }
        }
        notes
    }

    pub(crate) fn film_load_blockers(&self) -> Vec<String> {
        self.collect_blockers(true)
    }

    fn collect_blockers(&self, film_load: bool) -> Vec<String> {
        let mut out: Vec<String> = self
            .parts
            .iter()
            .flat_map(|p| {
                let name = p.name();
                let reasons = if film_load {
                    p.film_load_blockers()
                } else {
                    p.blockers()
                };
                reasons.into_iter().map(move |w| format!("{name}: {w}"))
            })
            .collect();
        out.extend(
            (self.checks)()
                .into_iter()
                .map(|(owner, why)| format!("{owner}: {why}")),
        );
        out
    }
}

impl agent::SnapshotMachine for BootMachine<'_> {
    fn blockers(&mut self) -> Vec<String> {
        self.collect_blockers(false)
    }

    fn save(&mut self, name: &str, replace: bool, at: &At, cow: bool) -> Result<Saved, RpcError> {
        let mut shelf = self.shelf.clone();
        if !replace && agent::Snapshots::exists(&mut shelf, name) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("a snapshot named {name:?} exists"),
            ));
        }
        let header = Header {
            build: self.shelf.build.clone(),
            name: name.to_owned(),
            tick: at.tick.unwrap_or(0),
            presents: self.presents,
            notes: Self::notes(at),
        };
        let bytes = save_parts(&mut self.parts, header, &*self.hash).map_err(unavailable)?;
        // A background thread (co-op) may have gone live while saving.
        let late = agent::SnapshotMachine::blockers(&mut *self);
        if !late.is_empty() {
            return Err(RpcError::new(
                code::BUSY,
                format!(
                    "went live while saving, nothing written: {}",
                    late.join("; ")
                ),
            ));
        }
        let path = self.shelf.store().save(name, &bytes).map_err(unavailable)?;
        if cow {
            let rt = self.rt.ok_or_else(|| {
                unavailable("this machine has no runtime for a RAM image")
            })?;
            let sidecar = ram_image_path(&path);
            write_ram_image(&sidecar, rt.memory().physical_ram())
                .map_err(|e| unavailable(format!("{}: {e}", sidecar.display())))?;
            eprintln!("snapshots: RAM image {}", sidecar.display());
        }
        let sections: Vec<String> = Snapshot::open(&bytes, &self.shelf.build)
            .map(|s| s.tags().iter().map(ToString::to_string).collect())
            .unwrap_or_default();
        Ok(Saved {
            name: name.to_owned(),
            path: path.display().to_string(),
            bytes: bytes.len() as u64,
            sections,
            ..Default::default()
        })
    }

    fn load(&mut self, name: &str, cow: bool) -> Result<Loaded, RpcError> {
        let bytes = self.shelf.store().load(name).map_err(|e| match e {
            snapshot::Error::NoSuchSnapshot(_) | snapshot::Error::BadName(_) => {
                RpcError::new(code::INVALID_PARAMS, e.to_string())
            }
            e => unavailable(e),
        })?;
        let snap = Snapshot::open(&bytes, &self.shelf.build).map_err(unavailable)?;
        let cow_path = cow
            .then(|| {
                let image = ram_image_path(
                    &self
                        .shelf
                        .store()
                        .dir()
                        .join(format!("{name}.{}", snapshot::store::EXTENSION)),
                );
                if !image.is_file() {
                    return Err(RpcError::new(
                        code::INVALID_PARAMS,
                        format!(
                            "{} has no RAM image; save it again with \"cow\": true",
                            image.display()
                        ),
                    ));
                }
                Ok(image)
            })
            .transpose()?;
        let applied =
            load_parts(&mut self.parts, &snap, cow_path.as_deref()).map_err(unavailable)?;
        let hash = check_hash(&snap, &(self.hash)());
        let notes: BTreeMap<String, String> = snap.header.notes.iter().cloned().collect();
        Ok(Loaded {
            name: name.to_owned(),
            tick: snap.header.tick,
            presents: snap.header.presents,
            notes,
            applied,
            hash,
            ..Default::default()
        })
    }
}

/// The raw RAM image a cow save writes beside the snapshot file.
fn ram_image_path(snapshot: &Path) -> PathBuf {
    snapshot.with_extension("ram")
}

/// Write `ram` to `path` as a sparse image: zero pages are holes, so every
/// process mapping the file shares one zero page for them, and the file's
/// length is the whole of RAM.
fn write_ram_image(path: &Path, ram: &[u8]) -> std::io::Result<()> {
    use std::io::{Seek, SeekFrom, Write};
    const PAGE: usize = 4096;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut out = std::fs::File::create(&tmp)?;
    let mut at = 0usize;
    while at < ram.len() {
        let page = &ram[at..(at + PAGE).min(ram.len())];
        if page.iter().any(|&b| b != 0) {
            out.seek(SeekFrom::Start(at as u64))?;
            out.write_all(page)?;
        }
        at += page.len();
    }
    out.set_len(ram.len() as u64)?;
    out.flush()?;
    drop(out);
    std::fs::rename(&tmp, path)
}

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// Guest RAM, its layout and the TSC (`snapshot::guest`).
struct GuestPart<'a>(&'a runtime::Runtime);

impl Part for GuestPart<'_> {
    fn name(&self) -> &'static str {
        "guest"
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        snapshot::guest::save_guest(out, &self.0.memory(), None).map_err(err)?;
        snapshot::guest::save_tsc(out, self.0).map_err(err)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        snapshot::guest::load_guest(snapshot, &mut self.0.memory(), None).map_err(err)?;
        snapshot::guest::load_tsc(snapshot, self.0).map_err(err)
    }
    fn save_with_ram_base(&mut self, out: &mut SnapshotWriter, base: &snapshot::Base<'_>) -> Result<(), String> {
        snapshot::guest::save_guest(out, &self.0.memory(), Some(base)).map_err(err)?;
        snapshot::guest::save_tsc(out, self.0).map_err(err)
    }
    fn load_with_ram_base(&mut self, snap: &Snapshot<'_>, base: &snapshot::Base<'_>) -> Result<(), String> {
        snapshot::guest::load_guest_delta(snap, &mut self.0.memory(), base).map_err(err)?;
        snapshot::guest::load_tsc(snap, self.0).map_err(err)
    }
    fn load_cow(&mut self, snapshot: &Snapshot<'_>, ram: &Path) -> Result<(), String> {
        let layout = snapshot::guest::load_layout(snapshot).map_err(err)?;
        self.0.memory().restore_cow(&layout, ram).map_err(err)?;
        snapshot::guest::load_tsc(snapshot, self.0).map_err(err)
    }
}

/// `kcore` and `machine`.
struct KernelPart<'a> {
    m: &'a mut kernel::Machine,
    rebuilders: &'a kernel::snapshot::Rebuilders,
}

impl Part for KernelPart<'_> {
    fn name(&self) -> &'static str {
        "kernel"
    }
    fn blockers(&self) -> Vec<String> {
        self.m
            .quiescent()
            .blockers
            .iter()
            .map(ToString::to_string)
            .collect()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        self.m.save_snapshot(out).map_err(err)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        self.m.load_snapshot(snapshot, self.rebuilders).map_err(err)
    }
}

/// `d3d`.
struct D3dPart<'a>(
    &'a dyn crate::d3d_glue::DeviceSnapshot,
    &'a runtime::Runtime,
);

impl Part for D3dPart<'_> {
    fn name(&self) -> &'static str {
        "d3d"
    }
    fn blockers(&self) -> Vec<String> {
        self.0.blockers()
    }
    fn film_load_blockers(&self) -> Vec<String> {
        self.0.film_load_blockers()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        self.0.save(self.1, out)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        if crate::theater::in_keyframe() {
            self.0.load_film(self.1, snapshot)
        } else {
            self.0.load(snapshot)
        }
    }
}

/// `dsound` and the mixer's `audio`.
struct DsoundPart<'a>(&'a crate::dsound_glue::Audio);

impl Part for DsoundPart<'_> {
    fn name(&self) -> &'static str {
        "dsound"
    }
    fn blockers(&self) -> Vec<String> {
        self.0.hle.snapshot_blockers()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        self.0.hle.save_snapshot(out).map_err(err)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        self.0.hle.load_snapshot(snapshot).map_err(err)
    }
}

/// `audioout`: the output's logical grid. The output renders only when
/// the boot loop advances it, so it stands still while this runs.
struct AudioOutPart<'a>(&'a crate::dsound_glue::Audio);

impl Part for AudioOutPart<'_> {
    fn name(&self) -> &'static str {
        "audioout"
    }
    fn blockers(&self) -> Vec<String> {
        self.0.output.snapshot_blockers()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        self.0.output.save_snapshot(out).map_err(err)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        self.0.output.load_snapshot(snapshot).map_err(err)
    }
}

/// `wma`.
struct WmaPart<'a>(&'a dsound::xwma::WmaHle);

impl Part for WmaPart<'_> {
    fn name(&self) -> &'static str {
        "wma"
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        self.0.save_snapshot(out).map_err(err)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        self.0.load_snapshot(snapshot).map_err(err)
    }
}

/// `xapi`: pads, change words, ports and the input tap's cursor.
struct XapiPart<'a>(&'a crate::xapi_glue::Shared);

impl Part for XapiPart<'_> {
    fn name(&self) -> &'static str {
        "xapi"
    }
    fn blockers(&self) -> Vec<String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .snapshot_blockers()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        let x = self.0.lock().unwrap_or_else(|e| e.into_inner());
        x.save_snapshot(out).map_err(err)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        let mut x = self.0.lock().unwrap_or_else(|e| e.into_inner());
        // The installed tap (the agent's) is seeked by the load itself.
        x.load_snapshot(snapshot).map(drop).map_err(err)
    }
}

/// `features`: every feature's own section, and every feature's policy.
struct FeaturesPart<'a> {
    plan: &'a feature::Plan,
    rt: &'a runtime::Runtime,
}

impl Part for FeaturesPart<'_> {
    fn name(&self) -> &'static str {
        "features"
    }
    fn blockers(&self) -> Vec<String> {
        self.plan
            .snapshot_blockers()
            .into_iter()
            .map(|(id, why)| format!("{id}: {why}"))
            .collect()
    }
    fn save(&mut self, out: &mut SnapshotWriter) -> Result<(), String> {
        snapshot::features::save_features(out, &self.plan.save_sections()).map_err(err)
    }
    fn load(&mut self, snapshot: &Snapshot<'_>) -> Result<(), String> {
        let saved = snapshot::features::load_features(snapshot).map_err(err)?;
        self.plan.load_sections(&saved, self.rt)?;
        #[cfg(feature = "gpu")]
        feature::native_map_work::reset();
        Ok(())
    }
}

#[cfg(test)]
mod tests;
