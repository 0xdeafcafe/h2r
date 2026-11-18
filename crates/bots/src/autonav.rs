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
//! other structure BSPs first, `<map>.0.bots` (the one checked) last. A map
//! asked for with [`Handle::want`] (the map being loaded) goes to the front
//! of the queue, scanned again so a map installed since boot is found.
//! `h2 bots nav` runs the same worker once, in the foreground; the game
//! runs it on a background thread (`boot::bots_nav_auto`), switched off by
//! [`ENV`]` = 0`.

use std::collections::{HashSet, VecDeque};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use crate::cache::{self, MapData, Stamp};

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
        let kind = u32::from_le_bytes(h[0x140..0x144].try_into().unwrap_or_default());
        Ok(MapHeader {
            name,
            multiplayer: map::CacheType::from(kind) == map::CacheType::Multiplayer,
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
/// file or the new one whole, and a crash leaves at most a `.tmp` file,
/// which the next write of that name replaces.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("no file name"))?
        .to_string_lossy();
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut f = std::fs::File::create(&tmp)?;
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
pub fn write_map(dir: &Path, map: &str, fingerprint: u64, bsps: &[MapData]) -> std::io::Result<Vec<PathBuf>> {
    std::fs::create_dir_all(dir)?;
    let stamp = Stamp::now(fingerprint);
    let mut written = Vec::with_capacity(bsps.len());
    for (k, d) in bsps.iter().enumerate().skip(1).chain(bsps.iter().enumerate().take(1)) {
        let file = dir.join(cache::file_name(map, k));
        write_atomic(&file, &d.to_bytes(stamp))?;
        written.push(file);
    }
    Ok(written)
}

/// Builds every structure BSP of a map: the scenario's BSP block
/// (`+0x210`, 0x44 each, the `sbsp` at `+0x14`), each BSP's collision.
pub fn build_map<R: Read + Seek>(map: &mut map::Map<R>) -> Result<Vec<MapData>, String> {
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
        out.push(cache::build(&bsp)?);
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
    /// Every structure BSP's data.
    fn build(&mut self) -> Result<Vec<MapData>, String>;
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
}

impl Region {
    pub fn new(key: impl Into<String>, base: u64, len: u64, open: Opener) -> Self {
        Region {
            key: key.into(),
            open,
            base,
            len,
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
        let mut r = (self.open)().map_err(|e| e.to_string())?;
        r.seek(SeekFrom::Start(self.base)).map_err(|e| e.to_string())?;
        let mut h = vec![0u8; MAP_HEADER_LEN];
        r.read_exact(&mut h).map_err(|e| format!("reading the header: {e}"))?;
        Ok(h)
    }

    fn build(&mut self) -> Result<Vec<MapData>, String> {
        let r = (self.open)().map_err(|e| e.to_string())?;
        let mut map = map::Map::from_region(r, self.base, self.len).map_err(|e| e.to_string())?;
        build_map(&mut map)
    }
}

/// The maps still to look at, in order.
#[derive(Default)]
pub struct Queue {
    jobs: VecDeque<Box<dyn Source>>,
}

impl Queue {
    /// Adds `s` at the back, in place of a queued source with its key.
    pub fn push(&mut self, s: Box<dyn Source>) {
        self.jobs.retain(|j| !j.key().eq_ignore_ascii_case(s.key()));
        self.jobs.push_back(s);
    }

    /// Moves the sources keyed `name` to the front.
    pub fn prioritise(&mut self, name: &str) {
        let (mut front, back): (VecDeque<_>, VecDeque<_>) =
            self.jobs.drain(..).partition(|j| j.key().eq_ignore_ascii_case(name));
        front.extend(back);
        self.jobs = front;
    }

    pub fn pop(&mut self) -> Option<Box<dyn Source>> {
        self.jobs.pop_front()
    }

    pub fn keys(&self) -> Vec<String> {
        self.jobs.iter().map(|j| j.key().to_owned()).collect()
    }

    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }
}

#[derive(Default)]
struct Wants {
    names: VecDeque<String>,
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
    /// being loaded, or one installed since.
    pub fn want(&self, map: &str) {
        let mut w = self.0.wants.lock().unwrap_or_else(|e| e.into_inner());
        if !w.names.iter().any(|n| n.eq_ignore_ascii_case(map)) {
            w.names.push_back(map.to_owned());
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

/// Builds the stale maps a scan finds; see the module docs.
pub struct Worker {
    dir: PathBuf,
    scan: Scan,
    first: Option<String>,
    force: bool,
    all_kinds: bool,
    stay: bool,
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
                scan,
                first: None,
                force: false,
                all_kinds: false,
                stay: false,
                log: Box::new(|l| eprintln!("{l}")),
                on_built: Box::new(|_, _| {}),
                shared: Arc::clone(&shared),
            },
            Handle(shared),
        )
    }

    /// Look at `map` first.
    pub fn first(mut self, map: Option<String>) -> Self {
        self.first = map;
        self
    }

    /// Build even fresh maps (`h2 bots nav --force`).
    pub fn force(mut self, force: bool) -> Self {
        self.force = force;
        self
    }

    /// Build maps of any kind, not only multiplayer (`h2 bots nav <map>`).
    pub fn any_kind(mut self, any: bool) -> Self {
        self.all_kinds = any;
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

    fn take_wants(&self) -> (Vec<String>, bool) {
        let mut w = self.shared.wants.lock().unwrap_or_else(|e| e.into_inner());
        (w.names.drain(..).collect(), w.stop)
    }

    /// Runs: the pass over the scan, one summary line, then (with
    /// [`Worker::stay`]) the maps asked for until stopped. Returns the
    /// summary of everything done.
    pub fn run(mut self) -> Summary {
        let mut q = Queue::default();
        let mut seen: HashSet<String> = HashSet::new();
        for s in (self.scan)() {
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
                for s in (self.scan)() {
                    let key = s.key().to_ascii_lowercase();
                    if s.key().eq_ignore_ascii_case(w) || seen.insert(key) {
                        q.push(s);
                    }
                }
                q.prioritise(w);
            }
            if let Some(src) = q.pop() {
                self.one(src, &mut sum);
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
                    .wait_while(w, |w| w.names.is_empty() && !w.stop)
                    .unwrap_or_else(|e| e.into_inner()),
            );
        }
        sum
    }

    fn one(&mut self, mut src: Box<dyn Source>, sum: &mut Summary) {
        let key = src.key().to_owned();
        let header = match src.header().and_then(|h| MapHeader::parse(&h)) {
            Ok(h) => h,
            Err(e) => {
                (self.log)(&format!("bots: nav mesh for {key} not built: {e}"));
                sum.failed.push(key);
                return;
            }
        };
        if !header.multiplayer && !self.all_kinds {
            sum.skipped += 1;
            return;
        }
        let status = status(&self.dir, &header.name, header.fingerprint);
        if !status.stale() && !self.force {
            sum.fresh += 1;
            return;
        }
        let t = Instant::now();
        let built = src.build().and_then(|bsps| {
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
                (self.on_built)(&header.name, d);
                sum.built.push(header.name);
            }
            Err(e) => {
                (self.log)(&format!("bots: nav mesh for {} not built: {e}", header.name));
                sum.failed.push(header.name);
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

/// The loaded map and whether its data came from the worker, so a file
/// read for the map as it loads never replaces data the worker built for
/// it after.
#[derive(Debug, Default)]
pub struct Loaded {
    name: String,
    built: bool,
}

impl Loaded {
    /// The loaded map is now `name`.
    pub fn changed(&mut self, name: &str) {
        self.name = name.to_owned();
        self.built = false;
    }

    /// Whether data for `map` (from the worker when `built`, else read from
    /// its file) goes in now: only for the loaded map, and a file's never
    /// over the worker's.
    pub fn accept(&mut self, map: &str, built: bool) -> bool {
        if self.name.is_empty() || !self.name.eq_ignore_ascii_case(map) {
            return false;
        }
        if built {
            self.built = true;
            true
        } else {
            !self.built
        }
    }
}

static LOADED: Mutex<Loaded> = Mutex::new(Loaded {
    name: String::new(),
    built: false,
});

/// The game loaded `map` (empty: none): its data is cleared until offered.
pub fn map_loaded(map: &str) {
    let mut l = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    l.changed(map);
    cache::set_current(None);
}

/// Offers `map`'s data to the bots ([`cache::set_current`]); see
/// [`Loaded::accept`]. Returns whether it went in.
pub fn offer(map: &str, data: MapData, built: bool) -> bool {
    let mut l = LOADED.lock().unwrap_or_else(|e| e.into_inner());
    if !l.accept(map, built) {
        return false;
    }
    cache::set_current(Some(Arc::new(data)));
    true
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
