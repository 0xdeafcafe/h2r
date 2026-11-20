//! Per-map stores of only the bytes a sim reads (docs/runtime/sim.md,
//! "Sim stores"; docs/live/dedicated-host.md, "Light host").
//!
//! The retail map is never streamed in a sim or light host that uses
//! stores. A recording pass ([`RECORD_ENV`] on `h2 sim run`) notes every
//! range the guest reads through the VFS; `h2 sim store build` packs those
//! ranges of each map into `<dir>/<map>.h2store`; a boot with
//! [`STORE_ENV`] naming that directory serves `maps\<map>.map` from the
//! store, byte for byte at the same offsets. A read of a byte no recording
//! covered is a named error ([`Error::NotInStore`]) — never a silent fall
//! back to the disc — so a missed range is found, recorded, and the store
//! rebuilt.
//!
//! The store file is a small fixed header, a sorted range index, and the
//! packed bytes of those ranges, nothing else. The serving side keeps only
//! the index in memory and serves the packed data from a read-only mapping.

use std::{
    collections::{BTreeMap, HashMap},
    fs::File,
    io::{BufRead, BufReader, Write},
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use crate::{
    Backend, DirectoryEntry, Error, HostFile, Metadata, Result, VolumeInfo,
    map_override::{is_map_path, resize},
};

/// The environment variable naming the file read ranges are recorded to.
pub const RECORD_ENV: &str = "H2_SIM_STORE_RECORD";
/// The environment variable naming the directory of stores to serve from.
pub const STORE_ENV: &str = "H2_SIM_STORE";
/// A store file's extension, as [`build`] names it: `<map>.h2store`.
pub const EXTENSION: &str = "h2store";

/// The first line of a recording; [`parse_reads`] refuses anything else.
pub const READS_FORMAT: &str = "h2-sim-reads 1";

// ---------------------------------------------------------------------------
// Recording
// ---------------------------------------------------------------------------

/// Notes the ranges the guest reads, when [`RECORD_ENV`] names a file.
/// One load of the switch, then a return, when it does not.
pub mod record {
    use super::*;
    use std::io::BufWriter;

    type Spans = BTreeMap<String, BTreeMap<u64, u64>>;

    fn state() -> Option<&'static Mutex<Spans>> {
        static STATE: OnceLock<Option<Mutex<Spans>>> = OnceLock::new();
        STATE
            .get_or_init(|| target().map(|_| Mutex::new(BTreeMap::new())))
            .as_ref()
    }

    /// The file selected for this process's recordings.
    fn target() -> Option<PathBuf> {
        static TARGET: OnceLock<Option<PathBuf>> = OnceLock::new();
        TARGET
            .get_or_init(|| {
                std::env::var_os(super::RECORD_ENV)
                    .filter(|v| !v.is_empty())
                    .map(PathBuf::from)
            })
            .clone()
    }

    /// Whether recording is on: [`RECORD_ENV`] names a file. Decided once.
    pub fn enabled() -> bool {
        state().is_some()
    }

    /// Record that `[at, at + len)` of `device\path` was read. A zero
    /// length is nothing (an end-of-file probe).
    pub fn note(device: &str, path: &str, at: u64, len: u64) {
        if len == 0 {
            return;
        }
        let Some(all) = state() else {
            return;
        };
        let mut all = all.lock().unwrap_or_else(|e| e.into_inner());
        insert(&mut all, device, path, at, len);
    }

    fn insert(all: &mut Spans, device: &str, path: &str, at: u64, len: u64) {
        let key = if path.is_empty() {
            device.to_owned()
        } else {
            format!("{device}\\{path}")
        };
        let end = at + len;
        let spans = all.entry(key).or_default();
        // Merge with every span this one touches, so the map stays the size
        // of the covered bytes' boundaries, not of the reads.
        let start = spans
            .range(..at)
            .next_back()
            .filter(|&(_, &e)| e >= at)
            .map_or(at, |(&s, _)| s);
        let mut last = end;
        while let Some((&s, &e)) = spans.range(..=last).next_back() {
            if e < start {
                break;
            }
            last = last.max(e);
            spans.remove(&s);
        }
        spans.insert(start, last);
    }

    /// Write the recording to [`target`], merged into what the file already
    /// holds, so runs under different seeds accumulate coverage. One line
    /// per span: `<path>\t<start>\t<end>`.
    pub fn flush() {
        let Some(file) = target() else {
            return;
        };
        if let Some(all) = state() {
            flush_to(&file, &all.lock().unwrap_or_else(|e| e.into_inner()));
        }
    }

    fn flush_to(file: &Path, all: &Spans) {
        let mut merged: BTreeMap<String, Vec<(u64, u64)>> = all.iter()
            .map(|(path, spans)| (path.clone(), spans.iter().map(|(&s, &e)| (s, e)).collect()))
            .collect();
        match File::open(&file) {
            Ok(existing) => match parse_reads(BufReader::new(existing)) {
                Ok(previous) => {
                    for (path, spans) in previous {
                        merged.entry(path).or_default().extend(spans);
                    }
                }
                Err(e) => {
                    eprintln!(
                        "sim store: {}: recording not merged in: {e}",
                        file.display()
                    )
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => {
                eprintln!(
                    "sim store: {}: recording not merged in: {e}",
                    file.display()
                )
            }
        }
        let write = File::create(&file).and_then(|f| {
            let mut out = BufWriter::new(f);
            writeln!(out, "{}", READS_FORMAT)?;
            for (path, spans) in &merged {
                for &(start, end) in &merge(spans) {
                    writeln!(out, "{path}\t{start}\t{end}")?;
                }
            }
            out.flush()
        });
        match write {
            Ok(()) => eprintln!("sim store: recording written to {}", file.display()),
            Err(e) => eprintln!("sim store: {}: recording not written: {e}", file.display()),
        }
    }

    #[cfg(test)]
    pub(in crate::sim_store) mod tests {
        use super::*;

        /// Flush a fresh recording, as a separate run would.
        pub fn note_and_flush(file: &Path, path: &str, at: u64, len: u64) {
            let mut spans = Spans::new();
            insert(&mut spans, "\\Device\\CdRom0", path, at, len);
            flush_to(file, &spans);
        }
    }
}

/// The spans of a recording, sorted and merged, by the device path read.
pub fn parse_reads<R: BufRead>(
    input: R,
) -> std::result::Result<BTreeMap<String, Vec<(u64, u64)>>, String> {
    let mut out: BTreeMap<String, Vec<(u64, u64)>> = BTreeMap::new();
    let mut lines = input.lines();
    let first = lines.next().ok_or("the recording is empty")?
        .map_err(|e| format!("line 1: {e}"))?;
    if first != READS_FORMAT {
        return Err(format!(
            "the recording's first line is not {READS_FORMAT:?}"
        ));
    }
    for (n, line) in lines.enumerate() {
        let line = line.map_err(|e| format!("line {}: {e}", n + 2))?;
        if line.is_empty() {
            continue;
        }
        let number = |text: &str| -> std::result::Result<u64, String> {
            text.trim()
                .parse()
                .map_err(|_| format!("line {}: {text:?} is not a number", n + 2))
        };
        let span = line
            .rsplit_once('\t')
            .and_then(|(head, end)| {
                head.rsplit_once('\t')
                    .map(|(path, start)| (path, start, end))
            })
            .ok_or_else(|| format!("line {}: not <path>\\t<start>\\t<end>", n + 2))?;
        let start = number(span.1)?;
        let end = number(span.2)?;
        if end < start {
            return Err(format!("line {}: the span ends before it starts", n + 2));
        }
        if span.0.contains('\t') {
            return Err(format!("line {}: more than three fields", n + 2));
        }
        out.entry(span.0.to_owned()).or_default().push((start, end));
    }
    Ok(out
        .into_iter()
        .map(|(path, spans)| (path, merge(&spans)))
        .collect())
}

/// Sorted, non-overlapping spans: overlapping and touching ones merged.
pub fn merge(spans: &[(u64, u64)]) -> Vec<(u64, u64)> {
    let mut sorted = spans.to_vec();
    sorted.sort_unstable();
    let mut out: Vec<(u64, u64)> = Vec::with_capacity(sorted.len());
    for &(start, end) in &sorted {
        match out.last_mut() {
            Some((_, last_end)) if start <= *last_end => *last_end = (*last_end).max(end),
            _ => out.push((start, end)),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// The store file
// ---------------------------------------------------------------------------

/// `b"H2SIMST1"`: the format, its version folded in.
pub const MAGIC: [u8; 8] = *b"H2SIMST1";
/// Header: magic, the map's logical length, the range count, reserved.
pub const HEADER: usize = 24;
/// One index row: where a range sits in the map, how long it is, and where
/// its bytes start in the store.
pub const ROW: usize = 24;

/// The index of one store: the map's logical length and the sorted,
/// non-overlapping byte ranges the store holds.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct StoreIndex {
    pub logical_len: u64,
    pub ranges: Vec<Range>,
}

/// One covered range of a map: `len` bytes at `logical`, stored at `data`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Range {
    pub logical: u64,
    pub len: u64,
    pub data: u64,
}

impl StoreIndex {
    /// The index of a store file's first `HEADER + ranges * ROW` bytes.
    pub fn parse(bytes: &[u8]) -> std::result::Result<Self, String> {
        let field = |at: usize| -> std::result::Result<u64, String> {
            bytes
                .get(at..at + 8)
                .and_then(|b| b.try_into().ok())
                .map(u64::from_le_bytes)
                .ok_or_else(|| "the store file is truncated".into())
        };
        if bytes.get(..8) != Some(&MAGIC[..]) {
            return Err("not a sim store (the magic differs)".into());
        }
        let logical_len = field(8)?;
        let count = field(16)?;
        if count >> 32 != 0 {
            return Err("the store file's format is newer than this build".into());
        }
        if count > 1 << 20 {
            return Err("the store file claims more ranges than any map has".into());
        }
        let count = count as usize;
        let index_len = HEADER + count * ROW;
        if bytes.len() < index_len {
            return Err("the store index is truncated".into());
        }
        let mut ranges = Vec::with_capacity(count);
        let mut logical_end = 0;
        let mut data_end = index_len as u64;
        for i in 0..count {
            let at = HEADER + i * ROW;
            let range = Range {
                logical: field(at)?,
                len: field(at + 8)?,
                data: field(at + 16)?,
            };
            if range.len == 0 {
                return Err(format!("range {i} is empty"));
            }
            if range.logical < logical_end {
                return Err(format!("range {i} overlaps or precedes its neighbour"));
            }
            if range.data != data_end {
                return Err(format!("range {i}'s data is not packed in order"));
            }
            data_end = data_end.checked_add(range.len)
                .ok_or_else(|| format!("range {i} overflows the store"))?;
            logical_end = range.logical.checked_add(range.len)
                .ok_or_else(|| format!("range {i} overflows the map"))?;
            if logical_end > logical_len {
                return Err(format!("range {i} runs past the map's length"));
            }
            ranges.push(range);
        }
        Ok(Self {
            logical_len,
            ranges,
        })
    }

    fn write(&self, out: &mut impl Write) -> std::io::Result<()> {
        out.write_all(&MAGIC)?;
        out.write_all(&self.logical_len.to_le_bytes())?;
        out.write_all(&(self.ranges.len() as u64).to_le_bytes())?;
        let mut data = (HEADER + self.ranges.len() * ROW) as u64;
        for range in &self.ranges {
            out.write_all(&range.logical.to_le_bytes())?;
            out.write_all(&range.len.to_le_bytes())?;
            out.write_all(&data.to_le_bytes())?;
            data += range.len;
        }
        Ok(())
    }

    /// The range holding `at`: the last that starts at or before it, when
    /// it reaches past it.
    fn containing(&self, at: u64) -> Option<&Range> {
        let i = self.ranges.partition_point(|r| r.logical <= at);
        self.ranges
            .get(i.wrapping_sub(1))
            .filter(|r| at < r.logical + r.len)
    }

    /// Whether `[at, at + len)` lies within the ranges. A read may span
    /// ranges; every byte between them must be covered.
    fn covers(&self, at: u64, len: u64) -> bool {
        let end = at.saturating_add(len).min(self.logical_len);
        let mut at = at;
        while at < end {
            match self.containing(at) {
                Some(range) => at = range.logical + range.len,
                None => return false,
            }
        }
        true
    }
}

/// Why a store did not open: nothing there, or a file that is not one.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenError {
    /// No store file of that name: the disc serves the map as usual.
    Absent,
    /// A store file exists but is not a readable store of this format.
    Broken(String),
}

/// An open store: the index in memory, the whole file mapped read-only so
/// every process on one host shares the same page-cache pages, and reads
/// copy straight out of the mapping into the caller's buffer (the guest's
/// own chunk), never into private heap of the store's own.
pub struct Store {
    path: PathBuf,
    pub(crate) file: File,
    map: memmap2::Mmap,
    pub index: StoreIndex,
}

impl Store {
    pub fn open(path: impl Into<PathBuf>) -> std::result::Result<Self, OpenError> {
        let path = path.into();
        let file = File::open(&path).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound => OpenError::Absent,
            _ => OpenError::Broken(e.to_string()),
        })?;
        let broken = |e: String| OpenError::Broken(format!("{}: {e}", path.display()));
        let map = crate::disc::map_immutable(&file).map_err(|e| broken(e.to_string()))?;
        let index = StoreIndex::parse(&map).map_err(broken)?;
        let wanted = index.ranges.last().map_or(HEADER as u64, |r| r.data + r.len);
        if map.len() as u64 != wanted {
            return Err(broken(format!(
                "the store file is {} bytes, its index says {wanted}", map.len()
            )));
        }
        Ok(Self { path, file, map, index })
    }

    /// The map's logical length, as the guest sees it in `stat`.
    pub fn logical_len(&self) -> u64 {
        self.index.logical_len
    }

    /// The bytes the store holds: the sum of its ranges.
    pub fn stored_len(&self) -> u64 {
        self.index.ranges.iter().map(|r| r.len).sum()
    }

    /// Read `[at, at + buffer.len())` as the map's bytes. A read past the
    /// end is short; a read of bytes no range covers is [`Error::NotInStore`].
    pub fn read_at(&self, at: u64, buffer: &mut [u8]) -> Result<usize> {
        let len = buffer
            .len()
            .min(self.index.logical_len.saturating_sub(at) as usize);
        if len == 0 {
            return Ok(0);
        }
        if !self.index.covers(at, len as u64) {
            eprintln!(
                "sim store: {}: bytes [{}, {}) were never recorded; record them and rebuild",
                self.path.display(),
                at,
                at + len as u64
            );
            return Err(Error::NotInStore);
        }
        let mut done = 0usize;
        while done < len {
            let range = self
                .index
                .containing(at + done as u64)
                .expect("covers() checked every byte");
            let logical = at + done as u64;
            let piece = (range.logical + range.len - logical).min((len - done) as u64) as usize;
            let data = range.data + (logical - range.logical);
            let bytes = usize::try_from(data)
                .ok()
                .and_then(|at| self.map.get(at..)?.get(..piece))
                .ok_or(Error::FileCorrupt)?;
            buffer[done..done + piece].copy_from_slice(bytes);
            done += piece;
        }
        Ok(len)
    }
}

// ---------------------------------------------------------------------------
// Building
// ---------------------------------------------------------------------------

/// What [`build`] wrote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    pub map: String,
    pub ranges: usize,
    pub logical_len: u64,
    pub store_len: u64,
}

/// Why a store could not be built. Each names one operator-fixable cause;
/// the build never silently serves less than was asked.
#[derive(Debug, thiserror::Error)]
pub enum BuildError {
    #[error("{0} is not on the disc (expected maps\\{0}.map)")]
    MapNotOnDisc(String),
    #[error("no reads were recorded for {0}")]
    NoReads(String),
    #[error("a read [{1}, {2}) runs past {0}'s end ({3} bytes)")]
    RangeBeyondEnd(String, u64, u64, u64),
    #[error("the built store does not match the disc: {0}")]
    Verify(String),
    #[error("{0}")]
    Io(String),
}

impl From<std::io::Error> for BuildError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// Write the store of `map` — its `ranges` (merged) of the disc's
/// `maps\<map>.map` — to `out`, verifying every byte on the way back.
/// Nothing outside the ranges is stored.
pub fn build(
    disc: &Path,
    map: &str,
    ranges: &[(u64, u64)],
    out: &Path,
) -> std::result::Result<Built, BuildError> {
    let ranges = merge(ranges);
    if ranges.is_empty() {
        return Err(BuildError::NoReads(map.to_owned()));
    }
    let path = format!("maps\\{map}.map");
    let mut disc = crate::DiscBackend::open(disc).map_err(|e| BuildError::Io(format!("{e:?}")))?;
    let logical_len = disc
        .stat(&path)
        .map_err(|_| BuildError::MapNotOnDisc(map.to_owned()))?
        .end_of_file;
    if let Some(&(start, end)) = ranges.iter().find(|&&(_, end)| end > logical_len) {
        return Err(BuildError::RangeBeyondEnd(
            map.to_owned(),
            start,
            end,
            logical_len,
        ));
    }
    let mut index = StoreIndex {
        logical_len,
        ranges: Vec::with_capacity(ranges.len()),
    };
    let mut data = (HEADER + ranges.len() * ROW) as u64;
    for &(start, end) in &ranges {
        index.ranges.push(Range {
            logical: start,
            len: end - start,
            data,
        });
        data += end - start;
    }
    std::fs::create_dir_all(out.parent().unwrap_or(Path::new(".")))?;
    let tmp = out.with_extension(format!("tmp-{}", std::process::id()));
    let buffer_len = 1 << 20;
    let written = (|| -> std::result::Result<(), BuildError> {
        {
            let mut writer = File::create(&tmp)?;
            index.write(&mut writer)?;
            let mut buffer = vec![0u8; buffer_len];
            for range in &index.ranges {
                let mut at = range.logical;
                while at < range.logical + range.len {
                    let n = (range.logical + range.len - at).min(buffer_len as u64) as usize;
                    disc.read_at(&path, at, &mut buffer[..n])
                        .map_err(|e| BuildError::Io(format!("reading the disc: {e:?}")))?;
                    writer.write_all(&buffer[..n])?;
                    at += n as u64;
                }
            }
            writer.flush()?;
        }
        // Verify: every stored byte, back against the disc.
        {
            let store = Store::open(&tmp).map_err(|e| BuildError::Verify(format!("{e:?}")))?;
            let mut have = vec![0u8; buffer_len];
            let mut want = vec![0u8; buffer_len];
            for range in &index.ranges {
                let mut at = range.logical;
                while at < range.logical + range.len {
                    let n = (range.logical + range.len - at).min(buffer_len as u64) as usize;
                    store.read_at(at, &mut have[..n]).map_err(|e| {
                        BuildError::Verify(format!("reading the store back: {e:?}"))
                    })?;
                    disc.read_at(&path, at, &mut want[..n])
                        .map_err(|e| BuildError::Verify(format!("reading the disc: {e:?}")))?;
                    if have[..n] != want[..n] {
                        return Err(BuildError::Verify(format!(
                            "bytes at [{}, {}) differ",
                            at,
                            at + n as u64
                        )));
                    }
                    at += n as u64;
                }
            }
        }
        Ok(())
    })();
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    std::fs::rename(&tmp, out)?;
    Ok(Built {
        map: map.to_owned(),
        ranges: ranges.len(),
        logical_len,
        store_len: data,
    })
}

/// The maps a recording holds ranges for: every path directly inside a
/// `maps` directory, whatever the device, by lowercase name.
pub fn recorded_maps(reads: &BTreeMap<String, Vec<(u64, u64)>>) -> Vec<String> {
    let mut names: Vec<String> = reads
        .keys()
        .filter_map(|path| {
            let path = path.to_ascii_lowercase();
            let (dir, name) = path.rsplit_once('\\')?;
            (dir.ends_with("\\maps") && name.ends_with(".map"))
                .then(|| name[..name.len() - 4].to_owned())
        })
        .collect();
    names.sort();
    names.dedup();
    names
}

/// Every recorded range of one map, whatever device it was read through:
/// the union of the spans of every `...\maps\<map>.map` path. Empty when
/// the recording holds nothing for it.
pub fn ranges_for(reads: &BTreeMap<String, Vec<(u64, u64)>>, map: &str) -> Vec<(u64, u64)> {
    let wanted = format!("\\maps\\{map}.map");
    let mut spans: Vec<(u64, u64)> = reads
        .iter()
        .filter(|(path, _)| path.to_ascii_lowercase().ends_with(&wanted))
        .flat_map(|(_, spans)| spans.iter().copied())
        .collect();
    if !spans.is_empty() {
        spans = merge(&spans);
    }
    spans
}

// ---------------------------------------------------------------------------
// Serving
// ---------------------------------------------------------------------------

/// What the directory holds for one map.
enum Held {
    /// No store of it: the disc serves the map as usual.
    None,
    /// The store, open.
    Store(std::sync::Arc<Store>),
    /// A store file exists but does not read: reads of the map fail, never
    /// silently fall back to the disc.
    Broken(String),
}

/// Serves the disc's `maps\<map>.map` from a directory of stores
/// ([`EXTENSION`]); everything else comes from `inner`. A store-served map
/// has no contiguous host file, like a streamed overlay; `stat` keeps the
/// map's logical length, and a read of bytes the store does not hold is
/// [`Error::NotInStore`], never the disc's.
pub struct SimStoreBackend<B> {
    inner: B,
    dir: PathBuf,
    /// By lowercase map name.
    stores: HashMap<String, Held>,
}

impl<B: Backend> SimStoreBackend<B> {
    pub fn new(inner: B, dir: impl Into<PathBuf>) -> Self {
        Self {
            inner,
            dir: dir.into(),
            stores: HashMap::new(),
        }
    }

    /// The store directory's key for the map `path` names, `None` for a
    /// path that is not `maps\<name>.map`. First use looks the file up and
    /// caches what the directory holds.
    fn ensure(&mut self, path: &str) -> Option<String> {
        let name = is_map_path(path)?;
        let key = name[..name.len() - 4].to_ascii_lowercase();
        if !self.stores.contains_key(&key) {
            let file = self.dir.join(format!("{key}.{EXTENSION}"));
            let held = match Store::open(&file) {
                Ok(store) => {
                    eprintln!(
                        "sim store: maps\\{name} from {} ({} of {} bytes, {} ranges)",
                        file.display(),
                        store.stored_len(),
                        store.logical_len(),
                        store.index.ranges.len()
                    );
                    Held::Store(std::sync::Arc::new(store))
                }
                Err(OpenError::Absent) => Held::None,
                Err(OpenError::Broken(why)) => {
                    eprintln!("sim store: maps\\{name}: {why}; its reads will fail");
                    Held::Broken(why)
                }
            };
            self.stores.insert(key.clone(), held);
        }
        Some(key)
    }
}

impl<B: Backend> Backend for SimStoreBackend<B> {
    fn opened(&mut self, path: &str) {
        self.inner.opened(path)
    }
    fn closed(&mut self, path: &str) {
        self.inner.closed(path)
    }
    fn rescan(&mut self) {
        self.inner.rescan()
    }
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        let mut meta = self.inner.stat(path)?;
        if let Some(key) = self.ensure(path)
            && let Some(Held::Store(store)) = self.stores.get_mut(&key)
        {
            resize(&mut meta, store.logical_len());
        }
        Ok(meta)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        if !path.eq_ignore_ascii_case("maps") {
            return Ok(entries);
        }
        for e in &mut entries {
            if e.metadata.is_directory() {
                continue;
            }
            if let Some(key) = self.ensure(&format!("{path}\\{}", e.name))
                && let Some(Held::Store(store)) = self.stores.get_mut(&key)
            {
                resize(&mut e.metadata, store.logical_len());
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        if let Some(key) = self.ensure(path) {
            return match self.stores.get_mut(&key).expect("ensure inserted it") {
                Held::Store(store) => store.read_at(offset, buffer),
                Held::Broken(why) => {
                    eprintln!("sim store: {path}: {why}");
                    Err(Error::FileCorrupt)
                }
                Held::None => self.inner.read_at(path, offset, buffer),
            };
        }
        self.inner.read_at(path, offset, buffer)
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    /// A store-served map's bytes are not one host region: readers that
    /// need one read through the VFS, or do without, as for a streamed
    /// overlay.
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        let stored = self
            .ensure(path)
            .is_some_and(|key| matches!(self.stores.get(&key), Some(Held::Store(_) | Held::Broken(_))));
        if stored {
            return None;
        }
        self.inner.host_file(path)
    }
    fn film_source(&mut self, path: &str) -> Result<Option<crate::FilmMapSource>> {
        if let Some(key) = self.ensure(path) {
            match self.stores.get(&key) {
                Some(Held::Store(store)) => return Ok(Some(crate::FilmMapSource::store(store.clone()))),
                Some(Held::Broken(_)) => return Err(Error::FileCorrupt),
                _ => {}
            }
        }
        self.inner.film_source(path)
    }
    fn map_ranges(&mut self, path: &str) -> Option<Vec<(u64, u64)>> {
        if let Some(key) = self.ensure(path)
            && let Some(Held::Store(store)) = self.stores.get(&key)
        {
            return Some(
                store
                    .index
                    .ranges
                    .iter()
                    .map(|r| (r.logical, r.len))
                    .collect(),
            );
        }
        self.inner.map_ranges(path)
    }
    /// Stores serve the retail map, whose identity is the disc's.
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        self.inner.map_id(path)
    }
    /// A store is immutable, as a verified server map copy is.
    fn map_changed(&mut self, path: &str) -> bool {
        let stored = self
            .ensure(path)
            .is_some_and(|key| matches!(self.stores.get(&key), Some(Held::Store(_))));
        !stored && self.inner.map_changed(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("h2-sim-store-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// `maps\level.map` holds `LEVEL_LEN` patterned bytes; `fonts\font`
    /// holds `glyphs`.
    const LEVEL_LEN: usize = 0x2000;

    fn test_iso(dir: &Path) -> std::path::PathBuf {
        let path = dir.join("disc.iso");
        if path.exists() {
            return path;
        }
        let mut image = vec![0u8; 42 * 2048];
        image[32 * 2048..][..20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        image[32 * 2048 + 20..][..4].copy_from_slice(&33u32.to_le_bytes());
        let dirent = |right: u16, sector: u32, size: u32, directory: bool, name: &str| -> Vec<u8> {
            let mut b = vec![0; 14];
            b[2..4].copy_from_slice(&right.to_le_bytes());
            b[4..8].copy_from_slice(&sector.to_le_bytes());
            b[8..12].copy_from_slice(&size.to_le_bytes());
            b[12] = if directory { 0x10 } else { 0 };
            b[13] = name.len() as u8;
            b.extend_from_slice(name.as_bytes());
            b.resize(b.len().next_multiple_of(4), 0);
            b
        };
        let fonts = dirent(0, 36, 6, false, "font");
        let maps = dirent(0, 37, LEVEL_LEN as u32, false, "level.map");
        let mut root = dirent(0, 34, fonts.len() as u32, true, "fonts");
        let right = (root.len() / 4) as u16;
        root[2..4].copy_from_slice(&right.to_le_bytes());
        root.extend(dirent(0, 35, maps.len() as u32, true, "maps"));
        image[32 * 2048 + 24..][..4].copy_from_slice(&(root.len() as u32).to_le_bytes());
        image[33 * 2048..][..root.len()].copy_from_slice(&root);
        image[34 * 2048..][..fonts.len()].copy_from_slice(&fonts);
        image[35 * 2048..][..maps.len()].copy_from_slice(&maps);
        image[36 * 2048..][..6].copy_from_slice(b"glyphs");
        for (i, b) in image[37 * 2048..][..LEVEL_LEN].iter_mut().enumerate() {
            *b = (i % 251) as u8;
        }
        std::fs::write(&path, image).unwrap();
        path
    }

    fn level_bytes() -> Vec<u8> {
        (0..LEVEL_LEN).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn spans_merge_touching_and_overlapping() {
        assert_eq!(
            merge(&[(0, 10), (10, 20), (30, 40), (35, 50), (100, 110)]),
            [(0, 20), (30, 50), (100, 110)]
        );
        assert_eq!(merge(&[]), Vec::<(u64, u64)>::new());
        assert_eq!(merge(&[(5, 5)]), [(5, 5)]);
    }

    #[test]
    fn a_recording_round_trips_through_its_lines() {
        let dir = tmp("round");
        let file = dir.join("reads.txt");
        std::fs::write(
            &file,
            "h2-sim-reads 1\n\\Device\\CdRom0\\maps\\a.map\t0\t2048\n\\Device\\CdRom0\\maps\\a.map\t8192\t16384\n\\Device\\CdRom0\\default.xbe\t0\t256\n",
        )
        .unwrap();
        let reads = parse_reads(BufReader::new(File::open(&file).unwrap())).unwrap();
        assert_eq!(
            reads["\\Device\\CdRom0\\maps\\a.map"],
            vec![(0, 2048), (8192, 16384)]
        );
        assert_eq!(reads["\\Device\\CdRom0\\default.xbe"], vec![(0, 256)]);
        let bad = dir.join("bad.txt");
        std::fs::write(&bad, "something else\n").unwrap();
        assert!(parse_reads(BufReader::new(File::open(&bad).unwrap())).is_err());
        // A line with a number where a path belongs is refused, not parsed.
        std::fs::write(&bad, "h2-sim-reads 1\nmaps\\a.map\tzero\t10\n").unwrap();
        assert!(parse_reads(BufReader::new(File::open(&bad).unwrap())).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn notes_merge_into_the_file_a_flush_writes() {
        let dir = tmp("note");
        let file = dir.join("reads.txt");
        std::fs::write(
            &file,
            "h2-sim-reads 1\n\\Device\\CdRom0\\maps\\a.map\t0\t2048\n",
        )
        .unwrap();
        // An overlapping span of the same file, and one of another.
        record::tests::note_and_flush(&file, "maps\\a.map", 1024, 4096 - 1024);
        record::tests::note_and_flush(&file, "maps\\b.map", 0, 100);
        record::tests::note_and_flush(&file, "maps\\a.map", 8192, 64);
        let reads = parse_reads(BufReader::new(File::open(&file).unwrap())).unwrap();
        assert_eq!(
            reads["\\Device\\CdRom0\\maps\\a.map"],
            vec![(0, 4096), (8192, 8256)],
            "each flush merged into what the file held"
        );
        assert_eq!(reads["\\Device\\CdRom0\\maps\\b.map"], vec![(0, 100)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_store_serves_its_ranges_and_errors_on_holes() {
        let dir = tmp("serve");
        let iso = test_iso(&dir);
        // The head and the tail, with a hole between them.
        let built = build(
            &iso,
            "level",
            &[(0, 0x800), (0x1800, LEVEL_LEN as u64)],
            &dir.join("level.h2store"),
        )
        .unwrap();
        assert_eq!(built.logical_len, LEVEL_LEN as u64);
        assert_eq!(built.ranges, 2);
        assert_eq!(
            built.store_len,
            (HEADER + 2 * ROW) as u64 + 0x800 + (LEVEL_LEN as u64 - 0x1800)
        );
        let store = Store::open(dir.join("level.h2store")).unwrap();
        let mut buffer = [0u8; 8];
        assert_eq!(store.read_at(0, &mut buffer).unwrap(), 8);
        assert_eq!(&buffer, &level_bytes()[..8]);
        assert_eq!(store.read_at(0x1804, &mut buffer).unwrap(), 8);
        assert_eq!(&buffer, &level_bytes()[0x1804..0x180c]);
        // The hole between the ranges is a named error, whichever side it
        // is read from, and a read spanning it is too.
        assert_eq!(store.read_at(0x900, &mut buffer), Err(Error::NotInStore));
        let mut span = [0u8; 0x1000];
        assert_eq!(store.read_at(0x700, &mut span), Err(Error::NotInStore));
        // Past the logical end is a short read of nothing; one straddling
        // it is short but served (the tail range reaches the end).
        assert_eq!(store.read_at(LEVEL_LEN as u64, &mut buffer).unwrap(), 0);
        assert_eq!(store.read_at(LEVEL_LEN as u64 - 4, &mut buffer).unwrap(), 4);
        assert_eq!(&buffer[..4], &level_bytes()[LEVEL_LEN - 4..]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_backend_serves_stored_maps_and_passes_the_rest_through() {
        let dir = tmp("backend");
        let iso = test_iso(&dir);
        build(&iso, "level", &[(0, 0x800)], &dir.join("level.h2store")).unwrap();
        let mut backend = SimStoreBackend::new(crate::open_disc(&iso).unwrap(), &dir);
        // The stored map: the map's size, its bytes at the covered range.
        assert_eq!(
            backend.stat("maps\\level.map").unwrap().end_of_file,
            LEVEL_LEN as u64
        );
        let mut buffer = [0u8; 8];
        assert_eq!(
            backend.read_at("maps\\level.map", 0, &mut buffer).unwrap(),
            8
        );
        assert_eq!(&buffer, &level_bytes()[..8]);
        // A stored map has no host file; a map with no store has the disc's.
        assert!(backend.host_file("maps\\level.map").is_none());
        // Another file, and an absent map, pass through to the disc.
        assert_eq!(backend.read_at("fonts\\font", 0, &mut [0u8; 6]).unwrap(), 6);
        assert_eq!(
            backend.read_at("maps\\absent.map", 0, &mut buffer),
            Err(Error::ObjectNameNotFound)
        );
        // The directory listing keeps the map's logical size.
        let entries = backend.read_directory("maps").unwrap();
        assert_eq!(entries[0].metadata.end_of_file, LEVEL_LEN as u64);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_broken_store_never_falls_back_to_the_disc() {
        let dir = tmp("broken");
        let iso = test_iso(&dir);
        std::fs::write(dir.join("level.h2store"), b"not a store").unwrap();
        let mut backend = SimStoreBackend::new(crate::open_disc(&iso).unwrap(), &dir);
        assert_eq!(
            backend.read_at("maps\\level.map", 0, &mut [0u8; 4]),
            Err(Error::FileCorrupt)
        );
        assert!(backend.host_file("maps\\level.map").is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn builds_fail_loudly_for_named_causes() {
        let dir = tmp("fail");
        let iso = test_iso(&dir);
        assert!(matches!(
            build(&iso, "level", &[], &dir.join("none.h2store")),
            Err(BuildError::NoReads(_))
        ));
        assert!(matches!(
            build(&iso, "absent", &[(0, 4)], &dir.join("absent.h2store")),
            Err(BuildError::MapNotOnDisc(_))
        ));
        assert!(matches!(
            build(
                &iso,
                "level",
                &[(0, LEVEL_LEN as u64 + 1)],
                &dir.join("far.h2store")
            ),
            Err(BuildError::RangeBeyondEnd(..))
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn recorded_maps_are_the_maps_directory_names() {
        let mut reads = BTreeMap::new();
        reads.insert(
            "\\Device\\CdRom0\\maps\\Waterworks.map".to_owned(),
            vec![(0, 4)],
        );
        reads.insert("\\Device\\CdRom0\\default.xbe".to_owned(), vec![(0, 4)]);
        reads.insert(
            "\\Device\\CdRom0\\maps\\fonts\\note.txt".to_owned(),
            vec![(0, 4)],
        );
        assert_eq!(recorded_maps(&reads), ["waterworks".to_owned()]);
    }
}
