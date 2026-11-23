//! The raw overlay (docs/editor/realtime-edits.md, "The edit overlay"):
//! byte replacements and appended extents over the map files, held in
//! memory beside whatever file serves a map.
//!
//! A map is named by its lowercase stem (`ascension`, `shared`). Both
//! readers of its raw data serve through the overlay: the guest, which
//! reads the cache slot file the map was copied into, through
//! [`crate::Vfs::read`] and the aliases the hot-edit glue sets
//! ([`RawOverlay::alias`]); and the native loaders, which read the map
//! itself through `assets::ResourceFiles`
//! (`crates/extensions/game/src/native_map.rs`). Nothing here touches a
//! file on disk: a slot file, a cache template and a play copy's private
//! generation never hold an overlay byte.

use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, OnceLock, RwLock};

/// Non-overlapping, non-adjacent replacement runs by first offset.
#[derive(Clone, Debug, Default)]
struct Runs(BTreeMap<u64, Vec<u8>>);

impl Runs {
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    fn len(&self) -> usize {
        self.0.len()
    }

    fn bytes(&self) -> u64 {
        self.0.values().map(|r| r.len() as u64).sum()
    }

    /// Removes every byte in `[at, at + len)`, splitting runs at the edges.
    fn cut(&mut self, at: u64, len: u64) {
        let end = at + len;
        let first = self
            .0
            .range(..at)
            .next_back()
            .filter(|(start, run)| **start + run.len() as u64 > at)
            .map(|(start, _)| *start)
            .unwrap_or(at);
        let hit: Vec<u64> = self.0.range(first..end).map(|(k, _)| *k).collect();
        for start in hit {
            let run = self.0.remove(&start).expect("listed");
            let run_end = start + run.len() as u64;
            if start < at {
                self.0.insert(start, run[..(at - start) as usize].to_vec());
            }
            if run_end > end {
                self.0.insert(end, run[(end - start) as usize..].to_vec());
            }
        }
    }

    fn put(&mut self, at: u64, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        let len = bytes.len() as u64;
        self.cut(at, len);
        // Join a run that ends at `at` and one that starts at the end.
        let mut start = at;
        let mut joined = bytes.to_vec();
        if let Some((&prev, run)) = self.0.range(..at).next_back()
            && prev + run.len() as u64 == at
        {
            let mut head = self.0.remove(&prev).expect("found");
            head.extend_from_slice(&joined);
            joined = head;
            start = prev;
        }
        if let Some(tail) = self.0.remove(&(at + len)) {
            joined.extend_from_slice(&tail);
        }
        self.0.insert(start, joined);
    }

    /// The runs' bytes inside `[at, at + len)`, clipped, in order.
    fn held(&self, at: u64, len: u64) -> Vec<(u64, Vec<u8>)> {
        let end = at + len;
        let first = self
            .0
            .range(..at)
            .next_back()
            .filter(|(start, run)| **start + run.len() as u64 > at)
            .map(|(start, _)| *start)
            .unwrap_or(at);
        self.0
            .range(first..end)
            .map(|(&start, run)| {
                let from = at.max(start);
                let to = end.min(start + run.len() as u64);
                (from, run[(from - start) as usize..(to - start) as usize].to_vec())
            })
            .collect()
    }

    /// Writes the runs over `buf`, which holds the file's bytes from `at`.
    fn apply(&self, at: u64, buf: &mut [u8]) {
        for (from, bytes) in self.held(at, buf.len() as u64) {
            let o = (from - at) as usize;
            buf[o..o + bytes.len()].copy_from_slice(&bytes);
        }
    }
}

/// One file's replacements and appended extents.
#[derive(Clone, Debug, Default)]
struct FileOverlay {
    /// Raw replacements: bytes no guest memory holds.
    runs: Runs,
    /// Tag metadata replacements ([`RawOverlay::put_metadata`]): copies of
    /// bytes guest memory holds too.
    metadata: Runs,
    /// Extents reserved past the file's end, `(offset, len)`, in order.
    appended: Vec<(u64, u64)>,
}

impl FileOverlay {
    /// The end of the last appended extent; 0 with none.
    fn end(&self) -> u64 {
        self.appended.iter().map(|(at, len)| at + len).max().unwrap_or(0)
    }

    fn is_empty(&self) -> bool {
        self.runs.is_empty() && self.metadata.is_empty() && self.appended.is_empty()
    }

    /// Whether it holds more than metadata copies.
    fn holds_raw(&self) -> bool {
        !self.runs.is_empty() || !self.appended.is_empty()
    }

    /// Writes both kinds of runs over `buf`, which holds the file's bytes
    /// from `at`. They cover different ranges of a map (its metadata, its
    /// raw blocks); where they meet, the raw run is written last.
    fn apply(&self, at: u64, buf: &mut [u8]) {
        self.metadata.apply(at, buf);
        self.runs.apply(at, buf);
    }
}

/// What the overlay holds, for `edit.overlay`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OverlaySummary {
    /// Raw replacement runs, over every file.
    pub entries: usize,
    /// Bytes in those runs.
    pub bytes: u64,
    /// Tag metadata runs ([`RawOverlay::put_metadata`]) and their bytes.
    pub metadata: usize,
    pub metadata_bytes: u64,
    /// Appended extents and their total length.
    pub appended: usize,
    pub appended_bytes: u64,
    /// The files with anything in the overlay, sorted.
    pub files: Vec<String>,
}

#[derive(Default)]
struct Shared {
    files: RwLock<HashMap<String, FileOverlay>>,
    /// Lowercase VFS object names (`\device\harddisk0\partition4\cache003.map`)
    /// served as the map they name.
    aliases: RwLock<HashMap<String, String>>,
    /// Set while any file holds anything: a read of an untouched overlay
    /// takes no lock.
    busy: AtomicBool,
}

/// A handle on one overlay; clones share it. [`RawOverlay::global`] is the
/// process's, which the game's readers and its hot-edit glue use.
#[derive(Clone, Default)]
pub struct RawOverlay(Arc<Shared>);

impl std::fmt::Debug for RawOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("RawOverlay").field(&self.summary()).finish()
    }
}

impl RawOverlay {
    /// A new, empty overlay of its own (tests, tools).
    pub fn new() -> Self {
        Self::default()
    }

    /// The process's overlay.
    pub fn global() -> &'static RawOverlay {
        static GLOBAL: OnceLock<RawOverlay> = OnceLock::new();
        GLOBAL.get_or_init(RawOverlay::new)
    }

    pub fn is_empty(&self) -> bool {
        !self.0.busy.load(Ordering::Acquire)
    }

    fn read<T>(&self, f: impl FnOnce(&HashMap<String, FileOverlay>) -> T) -> T {
        f(&self.0.files.read().unwrap_or_else(|e| e.into_inner()))
    }

    fn write<T>(&self, f: impl FnOnce(&mut HashMap<String, FileOverlay>) -> T) -> T {
        let mut files = self.0.files.write().unwrap_or_else(|e| e.into_inner());
        let out = f(&mut files);
        files.retain(|_, o| !o.is_empty());
        self.0.busy.store(!files.is_empty(), Ordering::Release);
        out
    }

    /// Whether the overlay holds anything of `file`.
    pub fn touches(&self, file: &str) -> bool {
        !self.is_empty() && self.read(|files| files.contains_key(&file.to_ascii_lowercase()))
    }

    /// Replaces `bytes.len()` bytes of `file` at `at`.
    pub fn put(&self, file: &str, at: u64, bytes: &[u8]) {
        self.write(|files| files.entry(file.to_ascii_lowercase()).or_default().runs.put(at, bytes));
    }

    /// Replaces `bytes.len()` bytes of `file`'s tag metadata at `at`: a
    /// copy of a live tag write (docs/editor/realtime-edits.md, "The
    /// edit overlay"), so a reader of the file's metadata (the native
    /// `Map`, a BSP switch) reads what guest memory holds. Served as
    /// [`Self::put`]'s runs are; kept apart from them, since guest memory
    /// holds the same bytes and a snapshot does not need them
    /// ([`Self::holds_raw`]).
    pub fn put_metadata(&self, file: &str, at: u64, bytes: &[u8]) {
        self.write(|files| files.entry(file.to_ascii_lowercase()).or_default().metadata.put(at, bytes));
    }

    /// Whether the overlay holds anything guest memory does not: a raw
    /// replacement or an appended extent. Metadata copies are not counted,
    /// so they never refuse a snapshot.
    pub fn holds_raw(&self) -> bool {
        !self.is_empty() && self.read(|files| files.values().any(FileOverlay::holds_raw))
    }

    /// The raw replacement bytes the overlay holds inside `[at, at + len)`
    /// of `file`, as `(offset, bytes)` runs: what [`Self::restore`] puts back.
    pub fn held(&self, file: &str, at: u64, len: u64) -> Vec<(u64, Vec<u8>)> {
        self.read(|files| {
            files
                .get(&file.to_ascii_lowercase())
                .map(|o| o.runs.held(at, len))
                .unwrap_or_default()
        })
    }

    /// Drops every replacement inside `[at, at + len)` of `file`, then puts
    /// `runs` back (a [`Self::held`] taken before a write): the overlay is
    /// as it was before that write.
    pub fn restore(&self, file: &str, at: u64, len: u64, runs: &[(u64, Vec<u8>)]) {
        self.write(|files| {
            let o = files.entry(file.to_ascii_lowercase()).or_default();
            o.runs.cut(at, len);
            for (from, bytes) in runs {
                o.runs.put(*from, bytes);
            }
        });
    }

    /// Reserves `bytes.len()` bytes past the end of `file`, whose own
    /// length is `file_len`, at the next multiple of `align` (at least 1),
    /// fills them and returns their offset.
    pub fn append(&self, file: &str, file_len: u64, bytes: &[u8], align: u64) -> u64 {
        let align = align.max(1);
        self.write(|files| {
            let o = files.entry(file.to_ascii_lowercase()).or_default();
            let at = o.end().max(file_len).div_ceil(align) * align;
            o.appended.push((at, bytes.len() as u64));
            o.runs.put(at, bytes);
            at
        })
    }

    /// Drops the appended extent at `at` of `file` with its bytes.
    pub fn drop_appended(&self, file: &str, at: u64) -> bool {
        self.write(|files| {
            let Some(o) = files.get_mut(&file.to_ascii_lowercase()) else {
                return false;
            };
            let Some(i) = o.appended.iter().position(|(a, _)| *a == at) else {
                return false;
            };
            let (at, len) = o.appended.remove(i);
            o.runs.cut(at, len);
            true
        })
    }

    /// `file`'s length as served: its own, or the end of its last appended
    /// extent when that is further.
    pub fn len(&self, file: &str, file_len: u64) -> u64 {
        if self.is_empty() {
            return file_len;
        }
        self.read(|files| {
            files
                .get(&file.to_ascii_lowercase())
                .map_or(file_len, |o| o.end().max(file_len))
        })
    }

    /// Serves a read of `file` at `at`: `buf[..served]` holds the file's
    /// own bytes. Returns how many bytes of `buf` the read now fills: past
    /// the file's end an appended extent is served, and the bytes between
    /// the end and the extent are zero. Replacements are written over the
    /// whole result.
    pub fn apply(&self, file: &str, at: u64, buf: &mut [u8], served: usize) -> usize {
        if self.is_empty() {
            return served;
        }
        self.read(|files| {
            let Some(o) = files.get(&file.to_ascii_lowercase()) else {
                return served;
            };
            let fill = (o.end().saturating_sub(at).min(buf.len() as u64) as usize).max(served);
            buf[served..fill].fill(0);
            o.apply(at, &mut buf[..fill]);
            fill
        })
    }

    /// Serves the VFS object `object` (a full object name, any case) as
    /// the map `file`: [`crate::Vfs::read`] applies `file`'s overlay to it.
    pub fn alias(&self, object: &str, file: &str) {
        self.0
            .aliases
            .write()
            .unwrap_or_else(|e| e.into_inner())
            .insert(object.to_ascii_lowercase(), file.to_ascii_lowercase());
    }

    /// The map the VFS object `object` is served as, if it is an alias of
    /// one the overlay holds.
    pub fn aliased(&self, object: &str) -> Option<String> {
        if self.is_empty() {
            return None;
        }
        let file = self
            .0
            .aliases
            .read()
            .unwrap_or_else(|e| e.into_inner())
            .get(&object.to_ascii_lowercase())
            .cloned()?;
        self.touches(&file).then_some(file)
    }

    /// Drops everything, aliases too: the map the overlay was made for is gone.
    pub fn clear(&self) {
        self.write(HashMap::clear);
        self.0.aliases.write().unwrap_or_else(|e| e.into_inner()).clear();
    }

    pub fn summary(&self) -> OverlaySummary {
        self.read(|files| {
            let mut s = OverlaySummary::default();
            for (name, o) in files {
                s.entries += o.runs.len();
                s.bytes += o.runs.bytes();
                s.metadata += o.metadata.len();
                s.metadata_bytes += o.metadata.bytes();
                s.appended += o.appended.len();
                s.appended_bytes += o.appended.iter().map(|(_, l)| l).sum::<u64>();
                s.files.push(name.clone());
            }
            s.files.sort();
            s
        })
    }
}

/// A reader of one map file served through an overlay: what it reads at
/// a map offset is what [`RawOverlay::apply`] serves there. `base` is
/// where the map starts in what `inner` reads (a map inside a larger
/// image). Appended extents are not served: a reader of the file reads
/// the file's own length.
pub struct OverlaidReader<R> {
    inner: R,
    overlay: RawOverlay,
    file: String,
    base: u64,
    /// `inner`'s position.
    at: u64,
}

impl<R: std::io::Seek> OverlaidReader<R> {
    pub fn new(mut inner: R, overlay: RawOverlay, file: &str, base: u64) -> std::io::Result<Self> {
        let at = inner.stream_position()?;
        Ok(OverlaidReader { inner, overlay, file: file.to_ascii_lowercase(), base, at })
    }
}

impl<R: std::io::Read> std::io::Read for OverlaidReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        if let Some(offset) = self.at.checked_sub(self.base) {
            self.overlay.apply(&self.file, offset, &mut buf[..n], n);
        } else if self.at + n as u64 > self.base {
            let skip = (self.base - self.at) as usize;
            self.overlay.apply(&self.file, 0, &mut buf[skip..n], n - skip);
        }
        self.at += n as u64;
        Ok(n)
    }
}

impl<R: std::io::Seek> std::io::Seek for OverlaidReader<R> {
    fn seek(&mut self, pos: std::io::SeekFrom) -> std::io::Result<u64> {
        self.at = self.inner.seek(pos)?;
        Ok(self.at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(len: usize) -> Vec<u8> {
        (0..len).map(|i| i as u8).collect()
    }

    /// Reads `[at, at + len)` of a `base` file through `o` as the backend does.
    fn read(o: &RawOverlay, base: &[u8], at: u64, len: usize) -> Vec<u8> {
        let mut buf = vec![0xee; len];
        let start = (at as usize).min(base.len());
        let n = (base.len() - start).min(len);
        buf[..n].copy_from_slice(&base[start..start + n]);
        let n = o.apply("m", at, &mut buf, n);
        buf.truncate(n);
        buf
    }

    #[test]
    fn reads_across_entry_edges_see_exactly_the_replaced_bytes() {
        let base = file(64);
        let o = RawOverlay::new();
        o.put("M", 10, &[0xa0, 0xa1, 0xa2, 0xa3]);
        o.put("m", 20, &[0xb0, 0xb1]);
        // A read that starts inside the first run and ends inside the second.
        let got = read(&o, &base, 12, 10);
        let mut want = base[12..22].to_vec();
        want[0] = 0xa2;
        want[1] = 0xa3;
        want[8] = 0xb0;
        want[9] = 0xb1;
        assert_eq!(got, want);
        // One wholly between them is the file's.
        assert_eq!(read(&o, &base, 14, 6), base[14..20]);
        // Another file is untouched.
        let mut buf = base[..16].to_vec();
        assert_eq!(o.apply("other", 0, &mut buf, 16), 16);
        assert_eq!(buf, base[..16]);
    }

    #[test]
    fn overlapping_puts_merge_and_held_restore_undo_them() {
        let o = RawOverlay::new();
        o.put("m", 4, &[1, 1, 1, 1]);
        let before = o.held("m", 2, 4);
        assert_eq!(before, vec![(4, vec![1, 1])]);
        o.put("m", 2, &[2, 2, 2, 2]);
        assert_eq!(o.held("m", 0, 16), vec![(2, vec![2, 2, 2, 2, 1, 1])]);
        assert_eq!(o.summary().entries, 1);
        o.restore("m", 2, 4, &before);
        assert_eq!(o.held("m", 0, 16), vec![(4, vec![1, 1, 1, 1])]);
        o.restore("m", 4, 4, &[]);
        assert!(o.is_empty());
        assert_eq!(o.summary(), OverlaySummary::default());
    }

    #[test]
    fn appended_extents_extend_the_file_past_its_end() {
        let base = file(30);
        let o = RawOverlay::new();
        let at = o.append("m", 30, &[7, 7, 7], 16);
        assert_eq!(at, 32);
        assert_eq!(o.len("m", 30), 35);
        assert_eq!(o.len("m", 40), 40);
        // A read over the end: the file's tail, zero padding, the extent.
        let got = read(&o, &base, 28, 16);
        assert_eq!(got, [28, 29, 0, 0, 7, 7, 7]);
        // A second append follows the first.
        assert_eq!(o.append("m", 30, &[9], 1), 35);
        assert!(o.drop_appended("m", 32));
        assert_eq!(o.len("m", 30), 36);
        assert_eq!(read(&o, &base, 32, 8), [0, 0, 0, 9]);
        assert!(o.drop_appended("m", 35));
        assert!(o.is_empty());
        assert_eq!(read(&o, &base, 28, 8), [28, 29]);
    }

    #[test]
    fn metadata_runs_are_served_and_kept_apart_from_raw_ones() {
        let base = file(64);
        let o = RawOverlay::new();
        o.put_metadata("M", 8, &[0xc0, 0xc1]);
        assert!(!o.is_empty());
        assert!(!o.holds_raw(), "a metadata copy is not raw");
        let mut want = base[6..12].to_vec();
        want[2] = 0xc0;
        want[3] = 0xc1;
        assert_eq!(read(&o, &base, 6, 6), want);
        // A raw restore over the same range leaves metadata alone.
        o.restore("m", 0, 64, &[]);
        assert_eq!(read(&o, &base, 8, 2), [0xc0, 0xc1]);
        o.put("m", 40, &[1]);
        assert!(o.holds_raw());
        let s = o.summary();
        assert_eq!((s.entries, s.bytes, s.metadata, s.metadata_bytes), (1, 1, 1, 2));
        o.restore("m", 40, 1, &[]);
        assert!(!o.holds_raw());
        o.clear();
        assert!(o.is_empty());
    }

    #[test]
    fn an_overlaid_reader_serves_map_offsets_past_its_base() {
        use std::io::{Read, Seek, SeekFrom};
        let o = RawOverlay::new();
        o.put_metadata("m", 2, &[0xaa]);
        // The map starts 4 bytes into the image.
        let image = file(16);
        let mut r = OverlaidReader::new(std::io::Cursor::new(image.clone()), o.clone(), "M", 4).unwrap();
        let mut all = Vec::new();
        r.read_to_end(&mut all).unwrap();
        let mut want = image.clone();
        want[6] = 0xaa;
        assert_eq!(all, want);
        r.seek(SeekFrom::Start(5)).unwrap();
        let mut two = [0u8; 2];
        r.read_exact(&mut two).unwrap();
        assert_eq!(two, [5, 0xaa]);
    }

    #[test]
    fn an_alias_names_its_map_only_while_the_overlay_holds_it() {
        let o = RawOverlay::new();
        let slot = "\\Device\\Harddisk0\\Partition4\\cache003.map";
        o.alias(slot, "Level");
        assert_eq!(o.aliased(slot), None);
        o.put("level", 0, &[1]);
        assert_eq!(o.aliased(&slot.to_ascii_uppercase()).as_deref(), Some("level"));
        assert_eq!(o.aliased("\\Device\\Harddisk0\\Partition4\\cache004.map"), None);
        o.clear();
        o.put("level", 0, &[1]);
        assert_eq!(o.aliased(slot), None, "clear drops the aliases");
    }
}
