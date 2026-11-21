//! Nav meshes built without asking (docs/game/bots.md §10, stage 2, "Built
//! for you").
//!
//! Every `.bots` file carries a [`Stamp`]: the generator version
//! ([`cache::GENERATOR`]) and the source map's [`fingerprint`] (its 2 KiB
//! cache header, which holds the map's size, name and XOR checksum). A
//! file is [`Status::Fresh`] only when both match the map the game would
//! load now; a missing, damaged, older-format, older-generator or
//! other-map file is stale and is built again.
//!
//! A [`Worker`] goes through the maps a `scan` finds ([`Source`]s: a host
//! file, a region of the disc image, a file the VFS serves), builds each
//! stale multiplayer map and writes it atomically ([`write_atomic`]): the
//! other structure BSPs first, `<map>.0.bots` (the one checked) last. Then
//! each BSP's visibility grid (`crate::vis`) is built and written as it
//! finishes, saving its progress every [`crate::vis::SAVE_EVERY`] so a
//! build cut short carries on next time; a map whose `.bots` is fresh but
//! whose `.vis` is not gets only the grid built, and one whose `.vis` is
//! current gets nothing. [`Worker::shared_cache`] can restore completed
//! nav and visibility builds to a new data directory. A map asked for
//! with [`Handle::want`] (the map being loaded) goes to the front
//! of the queue, scanned again so a map installed since boot is found.
//! `h2 bots nav` runs the same worker once, in the foreground; the game
//! runs it on a background thread (`boot::bots_nav_auto`), switched off by
//! [`ENV`]` = 0`, with [`Worker::background`]: parallel build phases use
//! [`background::workers`] threads below the game's priority.
//!
//! Maps no bot plays ([`map_use`]: the main menu and the shared resource
//! maps) are never asked for, scanned or built.

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use content::{Hasher, Sha256};

use crate::cache::{self, MapData, Stamp};

mod pads;
pub(crate) mod shared_cache;
mod traversals;

/// The user-level cache of completed builds. The worker still writes the
/// game's copies to its data directory.
pub use shared_cache::default_dir as shared_cache_dir;

/// `H2_BOTS_NAV_AUTO`: `0` (or `off`, `false`, `no`) stops the game
/// building nav meshes by itself. Unset or anything else: on.
pub const ENV: &str = "H2_BOTS_NAV_AUTO";

/// Whether an [`ENV`] value leaves the automatic build on.
pub fn enabled(v: Option<&str>) -> bool {
    !matches!(
        v.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("0" | "off" | "false" | "no")
    )
}

/// The maps no bot ever plays, by name: the main menu and the shared
/// resource maps.
pub const NOT_PLAYED: [&str; 3] = ["mainmenu", "shared", "single_player_shared"];

/// What the bots do with a map.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapUse {
    /// Nothing: no bot plays there (the main menu, the shared resource
    /// maps). No data is read, asked for or built.
    Never,
    /// A `.bots` file built by hand (`h2 bots nav <map>`) is read; the
    /// automatic build leaves it alone (campaign and unknown kinds).
    LoadOnly,
    /// Multiplayer: built automatically when stale.
    Build,
}

/// What the bots do with the map `name` of cache `kind` (the header's
/// `+0x140`; `None` when only the name is known, as in [`Handle::want`]:
/// the build looks at the header before it builds).
pub fn map_use(name: &str, kind: Option<map::CacheType>) -> MapUse {
    use map::CacheType as K;
    let name = name.trim();
    if name.is_empty() || NOT_PLAYED.iter().any(|n| n.eq_ignore_ascii_case(name)) {
        return MapUse::Never;
    }
    match kind {
        Some(K::MainMenu | K::Shared | K::SinglePlayerShared) => MapUse::Never,
        Some(K::Multiplayer) | None => MapUse::Build,
        Some(K::Campaign | K::Unknown(_)) => MapUse::LoadOnly,
    }
}

/// Whether the worker takes `s` at all: not a map no bot plays.
fn played(s: &dyn Source) -> bool {
    map_use(s.key(), None) != MapUse::Never
}

/// A map file's cache header: the bytes [`fingerprint`] reads.
pub const MAP_HEADER_LEN: usize = 0x800;

/// The map's fingerprint: FNV-1a (64-bit) over its cache header. The
/// header holds the declared size, the tag index's offsets and sizes, the
/// name and the XOR checksum of everything after it, which every map
/// writer here recomputes, so any edit to the map changes it; reading it
/// costs 2 KiB.
pub fn fingerprint(header: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in header {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// What the build needs from a map's cache header.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MapHeader {
    /// The internal name (`+0x198`), what the game's loaded-map name
    /// (`0x547898`) holds and what the file is named by.
    pub name: String,
    /// The cache type (`+0x140`).
    pub kind: map::CacheType,
    /// A multiplayer map (`+0x140` is 1); the bots play nowhere else.
    pub multiplayer: bool,
    pub fingerprint: u64,
}

impl MapHeader {
    pub fn parse(h: &[u8]) -> Result<Self, String> {
        if h.len() < MAP_HEADER_LEN || &h[..4] != b"daeh" || &h[0x7fc..0x800] != b"toof" {
            return Err("not a Halo 2 map (no cache header)".into());
        }
        let raw = &h[0x198..0x1b8];
        let end = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        let name = String::from_utf8_lossy(&raw[..end]).into_owned();
        if name.is_empty() {
            return Err("the map has no name".into());
        }
        let kind = map::CacheType::from(u32::from_le_bytes(h[0x140..0x144].try_into().unwrap_or_default()));
        Ok(MapHeader {
            name,
            kind,
            multiplayer: kind == map::CacheType::Multiplayer,
            fingerprint: fingerprint(&h[..MAP_HEADER_LEN]),
        })
    }
}

/// Where a map's `<map>.0.bots` stands against the map the game would load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Missing,
    /// Not a bot data file, or cut short.
    Damaged,
    /// Another file format (version 1 had no stamp).
    OldFormat(u32),
    /// Built by another generator version.
    OldGenerator(u32),
    /// Built from another map file of the same name (edited, replaced by
    /// a package, another sandbox's variant).
    OtherMap,
    Fresh,
}

impl Status {
    pub fn stale(self) -> bool {
        self != Status::Fresh
    }

    /// Why it is built, for the log.
    pub fn why(self) -> String {
        match self {
            Status::Missing => "none yet".into(),
            Status::Damaged => "the old file was damaged".into(),
            Status::OldFormat(v) => format!("file format {v}"),
            Status::OldGenerator(g) => {
                format!("generator {g}, now {}", cache::GENERATOR)
            }
            Status::OtherMap => "the map changed".into(),
            Status::Fresh => "up to date".into(),
        }
    }
}

/// The status of `dir/<map>.0.bots` for a map with `fingerprint`. Reads
/// the file's first [`cache::HEADER_LEN`] bytes.
pub fn status(dir: &Path, map: &str, fingerprint: u64) -> Status {
    let path = dir.join(cache::file_name(map, 0));
    let mut f = match std::fs::File::open(&path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Status::Missing,
        Err(_) => return Status::Damaged,
    };
    let mut b = [0u8; cache::HEADER_LEN];
    if f.read_exact(&mut b).is_err() {
        return Status::Damaged;
    }
    match cache::read_header(&b) {
        None => Status::Damaged,
        Some((v, None)) => Status::OldFormat(v),
        Some((_, Some(s))) if s.generator != cache::GENERATOR => Status::OldGenerator(s.generator),
        Some((_, Some(s))) if s.fingerprint != fingerprint => Status::OtherMap,
        Some(_) => Status::Fresh,
    }
}

/// Writes `bytes` to `path` through a temporary file in the same
/// directory, synced and then renamed over `path`: a reader sees the old
/// file or the new one whole. Writers in the same process use separate
/// temporary files, and a temporary file left after a crash is ignored.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    static NEXT_TMP: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?
        .to_string_lossy();
    let (tmp, mut f) = loop {
        let id = NEXT_TMP.fetch_add(1, Ordering::Relaxed);
        let tmp = dir.join(format!(".{name}.{}.{id}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
        {
            Ok(f) => break (tmp, f),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e),
        }
    };
    let result = (|| {
        f.write_all(bytes)?;
        f.sync_all()?;
        drop(f);
        std::fs::rename(&tmp, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// Writes a map's structure BSPs' data, stamped with `fingerprint`, to
/// `dir/<map>.<bsp>.bots`, BSP 0 last: once it is there, so are the rest.
/// Each BSP's visibility grid, when it has one, goes first, to
/// `dir/<map>.<bsp>.vis` (`crate::vis`). Returns the `.bots` paths.
pub fn write_map(dir: &Path, map: &str, fingerprint: u64, bsps: &[MapData]) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let stamp = Stamp::now(fingerprint);
    let order = || bsps.iter().enumerate().skip(1).chain(bsps.iter().enumerate().take(1));
    for (k, d) in order() {
        if let Some(v) = d.walls.vis() {
            write_atomic(&dir.join(crate::vis::file_name(map, k)), &v.to_bytes(stamp))?;
        }
    }
    write_bots(dir, map, bsps.iter().map(|d| d.to_bytes(stamp)))
}

fn write_bots(
    dir: &Path,
    map: &str,
    mut blobs: impl Iterator<Item = impl AsRef<[u8]>>,
) -> std::io::Result<Vec<PathBuf>> {
    let Some(first) = blobs.next() else {
        return Ok(Vec::new());
    };
    let mut written = Vec::new();
    let rest = blobs.enumerate().map(|(k, bytes)| (k + 1, bytes));
    for (k, bytes) in rest.chain(std::iter::once((0, first))) {
        let file = dir.join(cache::file_name(map, k));
        write_atomic(&file, bytes.as_ref())?;
        written.push(file);
    }
    Ok(written)
}

/// Builds every structure BSP of a map: the scenario's BSP block
/// (`+0x210`, 0x44 each, the `sbsp` at `+0x14`), each BSP's collision.
pub fn build_map<R: Read + Seek>(map: &mut map::Map<R>) -> Result<Vec<MapData>, String> {
    build_map_with(map, crate::vis::Threads::foreground())
}

fn build_map_with<R: Read + Seek>(
    map: &mut map::Map<R>,
    threads: crate::vis::Threads,
) -> Result<Vec<MapData>, String> {
    use tags::geometry::TagView;
    let e = |e: &dyn std::fmt::Display| e.to_string();
    let scenario = map.scenario().ok_or("no scenario")?.clone();
    let location = scenario.location.ok_or("the scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id).map_err(|x| e(&x))?;
    let view = TagView::new(&bytes, location.address);
    let bsps: Vec<map::TagId> = view
        .block(view.at(0, 0x218).map_err(|x| e(&x))?, 0x210, 0x44)
        .map_err(|x| e(&x))?
        .into_iter()
        .map(|b| map::TagId(u32::from_le_bytes([b[0x14], b[0x15], b[0x16], b[0x17]])))
        .collect();
    if bsps.is_empty() {
        return Err("the scenario has no structure BSP".into());
    }
    let placements = traversals::Placements::read(&view, map)?;
    let pads = pads::read(&view, map)?;
    let starts: Vec<_> = view
        .block(view.bytes(), 0x100, 0x34)
        .map_err(|x| e(&x))?
        .into_iter()
        .map(|r| {
            std::array::from_fn(|k| f32::from_le_bytes(r[k * 4..k * 4 + 4].try_into().unwrap()))
        })
        .filter(|p: &[f32; 3]| p.iter().all(|v| v.is_finite()))
        .collect();
    let mut out = Vec::with_capacity(bsps.len());
    for id in bsps {
        let tag = map.tag(id).map_err(|x| e(&x))?.clone();
        let loc = tag.location.ok_or("a BSP has no payload")?;
        let b = map.read_tag_bytes(id).map_err(|x| e(&x))?;
        let v = TagView::new(&b, loc.address);
        let h = v.at(0, 0x200).map_err(|x| e(&x))?;
        let coll = v
            .block(h, tags::pathfinding::SBSP_COLLISION_FIELD, tags::collision::Bsp::STRUCTURE_SIZE)
            .map_err(|x| e(&x))?;
        let at = coll.first().ok_or("a BSP has no collision")?.as_ptr() as usize - b.as_ptr() as usize;
        let bsp = tags::collision::Bsp::decode_structure(&b, loc.address, at).map_err(|x| e(&x))?;
        let mut data = cache::build_with(&bsp, threads)?;
        placements.add_links(&mut data);
        data.places = crate::places::Places::build_with(&data.nav, &data.walls, &starts, threads);
        data.places.pads = pads
            .iter()
            .copied()
            .filter(|p| data.nav.nearest(p.at, 1.5, 0.6, 2.).is_some())
            .collect();
        out.push(data);
    }
    Ok(out)
}

/// One map the worker may build.
pub trait Source: Send {
    /// The name it is asked for by ([`Handle::want`]): the file's stem,
    /// which for the game's maps is also the header's name.
    fn key(&self) -> &str;
    /// The map's first [`MAP_HEADER_LEN`] bytes.
    fn header(&mut self) -> Result<Vec<u8>, String>;
    /// The full byte identity of the source opened by [`Self::header`],
    /// if it can be reused across data directories.
    fn cache_digest(&mut self) -> Option<Sha256> {
        None
    }
    /// Every structure BSP's data.
    fn build(&mut self) -> Result<Vec<MapData>, String>;
    /// Build with the caller's thread budget and priority.
    fn build_with(&mut self, _threads: crate::vis::Threads) -> Result<Vec<MapData>, String> {
        self.build()
    }
}

/// A readable, seekable map stream.
pub trait ReadSeek: Read + Seek + Send {}
impl<T: Read + Seek + Send> ReadSeek for T {}

type Opener = Box<dyn FnMut() -> std::io::Result<Box<dyn ReadSeek>> + Send>;

/// A map at `base..base + len` of whatever `open` opens: a host file, a
/// region of the disc image, a file the VFS serves.
pub struct Region {
    key: String,
    open: Opener,
    base: u64,
    len: u64,
    reader: Option<Box<dyn ReadSeek>>,
}

impl Region {
    pub fn new(key: impl Into<String>, base: u64, len: u64, open: Opener) -> Self {
        Region {
            key: key.into(),
            open,
            base,
            len,
            reader: None,
        }
    }

    /// A whole host file; its stem is the key.
    pub fn file(path: PathBuf) -> Self {
        let key = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let len = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        Region::new(
            key,
            0,
            len,
            Box::new(move || Ok(Box::new(std::io::BufReader::new(std::fs::File::open(&path)?)))),
        )
    }
}

impl Source for Region {
    fn key(&self) -> &str {
        &self.key
    }

    fn header(&mut self) -> Result<Vec<u8>, String> {
        self.reader = None;
        if self.len < MAP_HEADER_LEN as u64 {
            return Err("the map region has no complete header".into());
        }
        let mut r = (self.open)().map_err(|e| e.to_string())?;
        r.seek(SeekFrom::Start(self.base)).map_err(|e| e.to_string())?;
        let mut h = vec![0u8; MAP_HEADER_LEN];
        r.read_exact(&mut h).map_err(|e| format!("reading the header: {e}"))?;
        // Keep the opened source for both digest and build. Replacing a
        // map's directory entry cannot change which bytes this job uses.
        self.reader = Some(r);
        Ok(h)
    }

    fn cache_digest(&mut self) -> Option<Sha256> {
        let r = self.reader.as_mut()?;
        r.seek(SeekFrom::Start(self.base)).ok()?;
        let mut region = r.take(self.len);
        let mut hash = Hasher::default();
        let mut buffer = vec![0; 256 * 1024];
        loop {
            let n = match region.read(&mut buffer) {
                Ok(0) => break,
                Ok(n) => n,
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return None,
            };
            hash.update(&buffer[..n]);
        }
        (region.limit() == 0).then(|| hash.finish())
    }

    fn build(&mut self) -> Result<Vec<MapData>, String> {
        self.build_with(crate::vis::Threads::foreground())
    }

    fn build_with(&mut self, threads: crate::vis::Threads) -> Result<Vec<MapData>, String> {
        let r = match self.reader.take() {
            Some(r) => r,
            None => (self.open)().map_err(|e| e.to_string())?,
        };
        let mut map = map::Map::from_region(r, self.base, self.len).map_err(|e| e.to_string())?;
        build_map_with(&mut map, threads)
    }
}

/// The maps still to look at, in order.
#[derive(Default)]
pub struct Queue {
    jobs: VecDeque<Job>,
}

struct Job {
    source: Box<dyn Source>,
    generation: Option<u64>,
}

impl Queue {
    /// Adds `s` at the back, in place of a queued source with its key.
    pub fn push(&mut self, s: Box<dyn Source>) {
        self.push_for(s, None);
    }

    fn push_for(&mut self, source: Box<dyn Source>, generation: Option<u64>) {
        if generation.is_none() && self.jobs.iter().any(|job| {
            job.generation.is_some() && job.source.key().eq_ignore_ascii_case(source.key())
        }) {
            return;
        }
        self.jobs.retain(|job| !job.source.key().eq_ignore_ascii_case(source.key()));
        self.jobs.push_back(Job { source, generation });
    }

    /// Moves the sources keyed `name` to the front.
    pub fn prioritise(&mut self, name: &str) {
        let (mut front, back): (VecDeque<_>, VecDeque<_>) =
            self.jobs.drain(..).partition(|job| job.source.key().eq_ignore_ascii_case(name));
        front.extend(back);
        self.jobs = front;
    }

    pub fn pop(&mut self) -> Option<Box<dyn Source>> {
        self.jobs.pop_front().map(|job| job.source)
    }

    pub fn keys(&self) -> Vec<String> {
        self.jobs.iter().map(|job| job.source.key().to_owned()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

struct Request {
    name: String,
    /// Mounted source recorded by the active slot, when known.
    path: Option<String>,
    generation: Option<u64>,
}

#[derive(Default)]
struct Wants {
    maps: VecDeque<Request>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    wants: Mutex<Wants>,
    wake: Condvar,
}

/// Asks a running [`Worker`] for a map, or to stop.
#[derive(Clone)]
pub struct Handle(Arc<Shared>);

impl Handle {
    /// Look at `map` next (built if stale), scanning again for it: the map
    /// being loaded, or one installed since. A map no bot plays
    /// ([`MapUse::Never`]) is ignored.
    pub fn want(&self, map: &str) {
        self.enqueue(map, None, None);
    }

    /// A loaded map carries its slot's actual source. A later menu hint
    /// with just a short name must not displace that path.
    pub fn want_active(&self, map: &str, path: Option<String>, generation: u64) {
        self.enqueue(map, path, Some(generation));
    }

    fn enqueue(&self, map: &str, path: Option<String>, generation: Option<u64>) {
        if map_use(map, None) == MapUse::Never {
            return;
        }
        let mut w = self.0.wants.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(queued) = w.maps.iter_mut().find(|r| r.name.eq_ignore_ascii_case(map)) {
            if generation.is_some() {
                queued.path = path;
                queued.generation = generation;
            }
        } else {
            w.maps.push_back(Request { name: map.to_owned(), path, generation });
        }
        self.0.wake.notify_all();
    }

    /// Stops the worker after the map it is on.
    pub fn stop(&self) {
        self.0.wants.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        self.0.wake.notify_all();
    }
}

/// What a pass did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Summary {
    pub built: Vec<String>,
    pub fresh: usize,
    /// Not multiplayer maps.
    pub skipped: usize,
    pub failed: Vec<String>,
}

type Scan = Box<dyn FnMut() -> Vec<Box<dyn Source>> + Send>;
type Lookup = Box<dyn FnMut(&str, Option<&str>) -> Option<Box<dyn Source>> + Send>;

/// Builds the stale maps a scan finds; see the module docs.
pub struct Worker {
    dir: PathBuf,
    shared_cache: Option<PathBuf>,
    scan: Scan,
    lookup: Option<Lookup>,
    first: Option<String>,
    force: bool,
    all_kinds: bool,
    stay: bool,
    threads: crate::vis::Threads,
    /// Grids stopped for a map asked for, to carry on with when the queue
    /// is empty; `true` when the map is not counted in the summary yet.
    later: VecDeque<(String, bool)>,
    log: Box<dyn FnMut(&str) + Send>,
    on_built: Box<dyn FnMut(&str, &MapData) + Send>,
    shared: Arc<Shared>,
}

impl Worker {
    /// A worker writing to `dir` over the maps `scan` finds. By default it
    /// builds stale multiplayer maps once and returns; see the setters.
    pub fn new(dir: PathBuf, scan: Scan) -> (Self, Handle) {
        let shared = Arc::new(Shared::default());
        (
            Worker {
                dir,
                shared_cache: None,
                scan,
                lookup: None,
                first: None,
                force: false,
                all_kinds: false,
                stay: false,
                threads: crate::vis::Threads::foreground(),
                later: VecDeque::new(),
                log: Box::new(|l| eprintln!("{l}")),
                on_built: Box::new(|_, _| {}),
                shared: Arc::clone(&shared),
            },
            Handle(shared),
        )
    }

    /// Sources that should only be opened on request, outside the scan.
    pub fn on_demand(
        mut self,
        lookup: impl FnMut(&str, Option<&str>) -> Option<Box<dyn Source>> + Send + 'static,
    ) -> Self {
        self.lookup = Some(Box::new(lookup));
        self
    }

    /// Look at `map` first.
    pub fn first(mut self, map: Option<String>) -> Self {
        self.first = map;
        self
    }

    /// Reuse finished nav and visibility builds across data directories.
    /// Missing or inaccessible entries leave the local build path in use.
    pub fn shared_cache(mut self, dir: Option<PathBuf>) -> Self {
        self.shared_cache = dir;
        self
    }

    /// Build even fresh maps and bypass the shared cache (`h2 bots nav --force`).
    pub fn force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Build maps of any kind, not only multiplayer (`h2 bots nav <map>`).
    pub fn any_kind(mut self, any: bool) -> Self {
        self.all_kinds = any;
        self
    }

    /// Builds navigation and visibility as background work
    /// ([`crate::vis::Threads::background`]): the game's worker. Off, they
    /// take half the machine's threads at the default priority
    /// (`h2 bots nav`, which the user waits for).
    pub fn background(mut self, on: bool) -> Self {
        self.threads = if on {
            crate::vis::Threads::background()
        } else {
            crate::vis::Threads::foreground()
        };
        self
    }

    /// After the pass, wait for [`Handle::want`] until [`Handle::stop`].
    pub fn stay(mut self, stay: bool) -> Self {
        self.stay = stay;
        self
    }

    pub fn log(mut self, log: impl FnMut(&str) + Send + 'static) -> Self {
        self.log = Box::new(log);
        self
    }

    /// Called with BSP 0's data after each map is written.
    pub fn on_built(mut self, f: impl FnMut(&str, &MapData) + Send + 'static) -> Self {
        self.on_built = Box::new(f);
        self
    }

    fn take_wants(&self) -> (Vec<Request>, bool) {
        let mut w = self.shared.wants.lock().unwrap_or_else(|e| e.into_inner());
        (w.maps.drain(..).collect(), w.stop)
    }

    /// Runs: the pass over the scan, one summary line, then (with
    /// [`Worker::stay`]) the maps asked for until stopped. Returns the
    /// summary of everything done.
    pub fn run(mut self) -> Summary {
        let mut q = Queue::default();
        let mut seen: HashSet<String> = HashSet::new();
        for s in (self.scan)().into_iter().filter(|s| played(s.as_ref())) {
            seen.insert(s.key().to_ascii_lowercase());
            q.push(s);
        }
        if let Some(f) = self.first.take() {
            q.prioritise(&f);
        }
        let start = Instant::now();
        let mut sum = Summary::default();
        let mut summarised = false;
        loop {
            let (wants, stop) = self.take_wants();
            if stop {
                break;
            }
            for w in wants.iter().rev() {
                // Again from the scan: the map may be new, or changed
                // since it was looked at.
                for s in (self.scan)().into_iter().filter(|s| played(s.as_ref())) {
                    let key = s.key().to_ascii_lowercase();
                    if s.key().eq_ignore_ascii_case(&w.name) || seen.insert(key) {
                        q.push(s);
                    }
                }
                if let Some(src) = self.lookup.as_mut().and_then(|lookup| lookup(&w.name, w.path.as_deref()))
                    && played(src.as_ref())
                {
                    q.push_for(src, w.generation);
                } else if w.generation.is_some() {
                    (self.log)(&format!("bots: {}: active map source unavailable; walking without a mesh", w.name));
                }
                q.prioritise(&w.name);
            }
            if let Some(job) = q.jobs.pop_front() {
                self.one(job, &mut sum);
                continue;
            }
            if let Some((map, count)) = self.later.pop_front() {
                match crate::vis::status(&self.dir, &map) {
                    crate::vis::Status::Fresh => {}
                    why => self.grids(&map, why, &mut sum, count, None),
                }
                continue;
            }
            if !summarised {
                summarised = true;
                let line = summary_line(&sum, start.elapsed().as_secs_f32());
                (self.log)(&line);
            }
            if !self.stay {
                break;
            }
            let w = self.shared.wants.lock().unwrap_or_else(|e| e.into_inner());
            drop(
                self.shared
                    .wake
                    .wait_while(w, |w| w.maps.is_empty() && !w.stop)
                    .unwrap_or_else(|e| e.into_inner()),
            );
        }
        sum
    }

    fn one(&mut self, job: Job, sum: &mut Summary) {
        let Job { source: mut src, generation } = job;
        let read_header = timeline::setup::Stage::Nav.worker();
        let key = src.key().to_owned();
        let header = match src.header().and_then(|h| MapHeader::parse(&h)) {
            Ok(h) => h,
            Err(e) => {
                (self.log)(&format!("bots: nav mesh for {key} not built: {e}"));
                sum.failed.push(key);
                return;
            }
        };
        match map_use(&header.name, Some(header.kind)) {
            MapUse::Never => {
                sum.skipped += 1;
                return;
            }
            MapUse::LoadOnly if !self.all_kinds => {
                sum.skipped += 1;
                return;
            }
            _ => {}
        }
        let stamp = Stamp::now(header.fingerprint);
        let mut status = status(&self.dir, &header.name, header.fingerprint);
        drop(read_header);
        if !status.stale() && !self.force
            && let Some(generation) = generation
            && let Err(found) = load_cached(&self.dir, &header.name, generation, Some(stamp))
        {
            status = found;
        }
        if !status.stale() && !self.force {
            drop(src);
            match crate::vis::status(&self.dir, &header.name) {
                crate::vis::Status::Fresh => sum.fresh += 1,
                vis => self.grids(&header.name, vis, sum, true, None),
            }
            return;
        }
        let t = Instant::now();
        let nav_stage = timeline::setup::Stage::Nav.worker();
        let digest = self.shared_dir().and_then(|_| src.cache_digest());
        if let Some(bsps) = self.shared_dir().zip(digest).and_then(|(dir, key)| {
            shared_cache::restore_nav(dir, key, &self.dir, &header.name, stamp)
        }) {
            drop(src);
            (self.log)(&format!(
                "bots: restored the nav mesh for {} from the shared cache in {:.1} s",
                header.name,
                t.elapsed().as_secs_f32()
            ));
            self.publish_build(&header.name, &bsps[0].data, generation);
            sum.built.push(header.name.clone());
            drop(nav_stage);
            let why = crate::vis::status(&self.dir, &header.name);
            self.grids(&header.name, why, sum, false, Some(bsps));
            return;
        }
        // The nav files first, on their own: the bots walk on the mesh
        // while its grids build, and a grid cut short keeps its progress.
        let built = src.build_with(self.threads).and_then(|bsps| {
            write_map(&self.dir, &header.name, header.fingerprint, &bsps)
                .map(|_| bsps)
                .map_err(|e| format!("writing {}: {e}", self.dir.display()))
        });
        match built {
            Ok(bsps) => {
                let d = &bsps[0];
                (self.log)(&format!(
                    "bots: built the nav mesh for {} ({}): {} polygons, {} links, {} wall triangles{} in {:.1} s",
                    header.name,
                    status.why(),
                    d.nav.polys.len(),
                    d.nav.edges.len(),
                    d.walls.tris.len(),
                    if bsps.len() > 1 { format!(", {} BSPs", bsps.len()) } else { String::new() },
                    t.elapsed().as_secs_f32()
                ));
                self.publish_build(&header.name, d, generation);
                sum.built.push(header.name.clone());
                if let Some((dir, key)) = self.shared_dir().zip(digest) {
                    shared_cache::save_nav(dir, key, stamp, &bsps);
                }
                drop(nav_stage);
                self.grids(&header.name, crate::vis::Status::Missing, sum, false, None);
            }
            Err(e) => {
                (self.log)(&format!("bots: nav mesh for {} not built: {e}", header.name));
                sum.failed.push(header.name);
            }
        }
    }
}

impl Worker {
    fn publish_build(&mut self, name: &str, data: &MapData, generation: Option<u64>) {
        (self.on_built)(name, data);
        if let Some(generation) = generation
            && current_load(name, generation)
            && offer(name, Arc::new(data.clone()), generation)
        {
            (self.log)(&format!("bots: {name}: the new nav mesh is in use"));
        }
    }

    fn shared_dir(&self) -> Option<&Path> {
        self.shared_cache.as_deref().filter(|_| !self.force)
    }

    /// Builds `map`'s visibility grids from its `.bots` files, whose grid
    /// is `why` stale, carrying on from saved progress
    /// ([`crate::vis::rebuild_resumable`]). A map asked for meanwhile
    /// ([`Handle::want`]) or a stop cuts it short with its progress saved;
    /// it goes on the worker's `later` list to carry on once the queue is
    /// empty.
    /// `count`: the map goes in the summary when its grid is done (not
    /// when its nav mesh was just built and counted).
    fn grids(
        &mut self,
        map: &str,
        why: crate::vis::Status,
        sum: &mut Summary,
        count: bool,
        prepared: Option<Vec<crate::vis::GridInput>>,
    ) {
        let _stage = timeline::setup::Stage::Visibility.worker();
        let t = Instant::now();
        let shared = Arc::clone(&self.shared);
        let stop = move || {
            let w = shared.wants.lock().unwrap_or_else(|e| e.into_inner());
            // A pick for this same map can wait. A real load also needs
            // its mesh published, even when a speculative grid is running.
            w.stop || w.maps.iter().any(|r| r.generation.is_some() || !r.name.eq_ignore_ascii_case(map))
        };
        let cache = self.shared_dir();
        let result = match prepared {
            Some(bsps) => {
                crate::vis::rebuild_prepared(&self.dir, map, self.threads, &stop, cache, bsps)
            }
            None => crate::vis::rebuild_cached(&self.dir, map, self.threads, &stop, cache),
        };
        match result {
            Ok(Some(done)) => {
                let d = done.data;
                let reuse = if done.restored == 0 {
                    String::new()
                } else {
                    format!(" ({} BSPs restored from the shared cache)", done.restored)
                };
                (self.log)(&format!(
                    "bots: built the visibility grid for {map} ({}): {} in {:.1} s{reuse}",
                    why.why(),
                    d.walls.vis().map_or_else(|| "none".into(), |v| v.summary()),
                    t.elapsed().as_secs_f32()
                ));
                // Into the running map's data in place: the bots keep
                // their routes and memory, and every answer is the same.
                if let Some(v) = d.walls.vis()
                    && offer_vis(map, Arc::clone(v))
                {
                    (self.log)(&format!("bots: {map}: the new visibility grid is in use"));
                }
                if count {
                    sum.built.push(map.to_owned());
                }
            }
            Ok(None) => {
                (self.log)(&format!(
                    "bots: visibility grid for {map}: paused after {:.1} s, progress saved",
                    t.elapsed().as_secs_f32()
                ));
                if !self.later.iter().any(|(m, _)| m == map) {
                    self.later.push_back((map.to_owned(), count));
                }
            }
            Err(e) => {
                (self.log)(&format!("bots: visibility grid for {map} not built: {e}"));
                if count {
                    sum.failed.push(map.to_owned());
                }
            }
        }
    }
}

/// The pass's one line.
pub fn summary_line(s: &Summary, secs: f32) -> String {
    let mut l = format!(
        "bots: nav meshes: {} built, {} up to date",
        s.built.len(),
        s.fresh
    );
    if !s.failed.is_empty() {
        l += &format!(", {} failed ({})", s.failed.len(), s.failed.join(", "));
    }
    l + &format!(" ({secs:.1} s)")
}

/// Every request which can publish a mesh names a particular load.
/// A speculative build can populate the disk cache only.
#[derive(Debug, Default)]
pub struct Loaded {
    name: String,
    generation: u64,
}

impl Loaded {
    pub fn changed(&mut self, name: &str) {
        self.name = name.to_owned();
        self.generation = self.generation.wrapping_add(1);
    }

    pub fn accepts(&self, map: &str, generation: u64) -> bool {
        !self.name.is_empty() && self.generation == generation && self.name.eq_ignore_ascii_case(map)
    }
}

static LOADED: Mutex<Loaded> = Mutex::new(Loaded { name: String::new(), generation: 0 });

fn current_load(map: &str, generation: u64) -> bool {
    LOADED.lock().unwrap_or_else(|e| e.into_inner()).accepts(map, generation)
}

/// Clear the active map and return the load generation and retired data.
/// The caller releases the retired allocations on its map-data worker.
pub fn map_loaded(map: &str) -> (u64, Option<Arc<MapData>>) {
    let mut l = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    l.changed(map);
    let retired = cache::replace_current(None);
    let generation = l.generation;
    drop(l);
    crate::decisions::radio::map_loaded();
    (generation, retired)
}

/// The loaded map's short name (empty: none).
pub fn loaded() -> String {
    LOADED.lock().unwrap_or_else(|e| e.into_inner()).name.clone()
}

/// Clone the loaded name only when the caller has a different name.
pub(crate) fn loaded_change(previous: &str) -> Option<String> {
    let loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    (loaded.name != previous).then(|| loaded.name.clone())
}

/// Publish a completed build, or a disk read for a particular map load.
/// A late read from the first visit to A cannot replace data on A → B → A.
pub fn offer(map: &str, data: Arc<MapData>, generation: u64) -> bool {
    let l = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if !l.accepts(map, generation) {
        return false;
    }
    let retired = cache::replace_current(Some(data));
    drop(l);
    drop(retired);
    true
}

/// Read the local BSP on a utility worker. An automatically managed map
/// must match the header the active slot's mounted source supplied. A
/// manual or sim run has only the local file. Publish the mesh first so
/// that attaching a large cached grid does not delay movement.
pub fn load_cached(
    dir: &Path,
    map: &str,
    generation: u64,
    expected: Option<Stamp>,
) -> Result<(), Status> {
    if !current_load(map, generation) {
        return Ok(());
    }
    let (data, stamp) = {
        let _stage = timeline::setup::Stage::Nav.worker();
        let bytes = std::fs::read(dir.join(cache::file_name(map, 0))).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound { Status::Missing } else { Status::Damaged }
        })?;
        let (data, stamp) = decode_cached(&bytes, expected)?;
        (Arc::new(data), stamp)
    };
    if offer(map, Arc::clone(&data), generation) {
        eprintln!(
            "bots: {map}: {} polygons, {} links, {} wall triangles",
            data.nav.polys.len(), data.nav.edges.len(), data.walls.tris.len(),
        );
        let _stage = timeline::setup::Stage::Visibility.worker();
        eprintln!("{}", crate::vis::load(dir, map, 0, stamp, &data.walls));
    }
    Ok(())
}

/// Verify the header before allocating the polygon and wall structures.
fn decode_cached(bytes: &[u8], expected: Option<Stamp>) -> Result<(MapData, Stamp), Status> {
    if let Some(expected) = expected {
        let (version, stamp) = cache::read_header(bytes).ok_or(Status::Damaged)?;
        let stamp = stamp.ok_or(Status::OldFormat(version))?;
        if stamp.generator != expected.generator {
            return Err(Status::OldGenerator(stamp.generator));
        }
        if stamp.fingerprint != expected.fingerprint {
            return Err(Status::OtherMap);
        }
    }
    MapData::from_bytes(bytes).ok_or(Status::Damaged)
}

/// Attach a grid to the walls it was built from. Verifying its fingerprint
/// walks every triangle, so take a shared reference and release the map
/// lock before checking. Replacing the map retires that reference safely.
pub fn offer_vis(map: &str, vis: Arc<crate::vis::Vis>) -> bool {
    let mut loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    loop {
        if loaded.name.is_empty() || !loaded.name.eq_ignore_ascii_case(map) {
            return false;
        }
        let Some(data) = cache::current() else { return false };
        drop(loaded);
        let fits = vis.fits(&data.walls);
        loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
        if cache::current().as_ref().is_some_and(|active| Arc::ptr_eq(active, &data)) {
            let installed = fits && data.walls.set_vis(vis);
            drop(loaded);
            return installed;
        }
        // A simultaneous file read or rebuild replaced the snapshot. Drop
        // its keepalive outside the lock, then try the replacement's walls.
        drop(loaded);
        drop(data);
        loaded = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    }
}

static HANDLE: std::sync::OnceLock<Handle> = std::sync::OnceLock::new();

/// Makes `h` the game's running worker, for [`handle`].
pub fn set_handle(h: Handle) {
    let _ = HANDLE.set(h);
}

/// The game's running worker, if the automatic build is on.
pub fn handle() -> Option<&'static Handle> {
    HANDLE.get()
}

/// Builds `map`'s nav mesh next if it is missing or stale (the map the
/// player just picked with Clankers on, or Clankers just turned on for):
/// the game's worker looks at it right after the map it is on, so the
/// mesh is usually there when the game starts. Returns at once; a no-op
/// when the automatic build is off or `map` is empty. `map` is the map's
/// short name (`lockout`), what its `.map` file and cache header are named.
pub fn build_soon(map: &str) {
    if let (Some(h), false) = (handle(), map.is_empty()) {
        h.want(map);
    }
}

#[cfg(test)]
mod tests;
