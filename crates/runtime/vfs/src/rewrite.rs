//! A read-only backend that serves some files with different contents.
//!
//! [`RewriteBackend`] wraps another backend. For a path the [`Rewrite`] wants,
//! it reads the whole inner file, hands the bytes to the rewrite, and
//! serves the result: its size from `stat` and directory listings, its
//! bytes from `read_at`. A level's rewritten bytes can be evicted once
//! its file objects close. The shared resource maps stay available across
//! level loads. Everything else passes through. The inner backend is never
//! written.
//!
//! The boot runner uses it to swap button-icon glyphs in the disc's font
//! files (`docs/input/button-icons.md`).
use std::collections::HashMap;
use std::sync::Arc;

use crate::{Backend, DirectoryEntry, Error, FileSource, HostFile, Metadata, Result, VolumeInfo};

/// Files larger than this are served as they are, without asking the rewrite.
pub const MAX_REWRITE_BYTES: u64 = 64 * 1024 * 1024;
/// A directory enumeration can visit every map before the game opens one.
/// Retain only a bounded set of their rewritten payloads.
const MAP_CACHE_BYTES: u64 = 64 * 1024 * 1024;
static MAP_BYTES: mem_watch::Counter = mem_watch::Counter::new("assets.rewritten_map_cache_bytes");
static SHARED_MAP_BYTES: mem_watch::Counter =
    mem_watch::Counter::new("assets.rewritten_shared_map_cache_bytes");

fn shared_map(name: &str) -> bool {
    // Match the native renderer's fixed raw-resource selectors. Native map
    // opens request these alongside their level, including in multiplayer.
    ["mainmenu.map", "shared.map", "single_player_shared.map"]
        .iter()
        .any(|shared| name.eq_ignore_ascii_case(shared))
}

fn level_map(path: &str) -> bool {
    crate::map_override::is_map_path(path).is_some_and(|name| !shared_map(name))
}

/// Decides which files change and produces their new contents.
pub trait Rewrite: Send {
    /// Whether `path` (root-relative, backslash-separated, any case) may be
    /// rewritten. Must be cheap: it runs on every `stat` and `read_at`.
    fn wants(&self, path: &str) -> bool;
    /// New contents for `path`, or `None` to serve `original` unchanged.
    /// A rewritten map can be requested again after its cached bytes are
    /// evicted, so rewriting the same input must produce the same bytes.
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>>;
    /// A wanted file exceeded the size limit and will be served unchanged.
    /// Called once so dependent substitutions can stop.
    fn skipped(&mut self, _path: &str) {}
}

/// No rewrite: wants nothing.
impl<R: Rewrite> Rewrite for Option<R> {
    fn wants(&self, path: &str) -> bool {
        self.as_ref().is_some_and(|r| r.wants(path))
    }
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        self.as_mut()?.rewrite(path, original)
    }
    fn skipped(&mut self, path: &str) {
        if let Some(r) = self {
            r.skipped(path)
        }
    }
}

/// Two rewrites, the first's output the second's input. A file either
/// wants is rewritten; one neither changes is served unchanged.
pub struct Chain<A, B>(pub A, pub B);

impl<A: Rewrite, B: Rewrite> Rewrite for Chain<A, B> {
    fn wants(&self, path: &str) -> bool {
        self.0.wants(path) || self.1.wants(path)
    }
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        let first = if self.0.wants(path) {
            self.0.rewrite(path, original)
        } else {
            None
        };
        let input = first.as_deref().unwrap_or(original);
        let second = if self.1.wants(path) {
            self.1.rewrite(path, input)
        } else {
            None
        };
        second.or(first)
    }
    fn skipped(&mut self, path: &str) {
        if self.0.wants(path) {
            self.0.skipped(path);
        }
        if self.1.wants(path) {
            self.1.skipped(path);
        }
    }
}

enum Cached {
    Unchanged,
    Bytes(Arc<[u8]>),
    /// Retain the directory listing size without rebuilding an idle map.
    Evicted(usize),
}

struct Entry {
    contents: Cached,
    last_use: u64,
}

pub struct RewriteBackend<B, R> {
    inner: B,
    rewrite: R,
    /// By lowercase path, including declined files so they are asked once.
    files: HashMap<String, Entry>,
    open_maps: HashMap<String, usize>,
    access: u64,
    map_bytes: mem_watch::Gauge,
    shared_map_bytes: mem_watch::Gauge,
    map_byte_limit: u64,
}

impl<B: Backend, R: Rewrite> RewriteBackend<B, R> {
    pub fn new(inner: B, rewrite: R) -> Self {
        Self {
            inner,
            rewrite,
            files: HashMap::new(),
            open_maps: HashMap::new(),
            access: 0,
            map_bytes: MAP_BYTES.track(0),
            shared_map_bytes: SHARED_MAP_BYTES.track(0),
            map_byte_limit: MAP_CACHE_BYTES,
        }
    }

    /// The rewritten contents of `path`, loading it when there is no cached
    /// payload. `None` when the file is served from the inner backend.
    fn rewritten(&mut self, path: &str) -> Result<Option<&Arc<[u8]>>> {
        if path.is_empty() || !self.rewrite.wants(path) {
            return Ok(None);
        }
        let key = path.to_ascii_lowercase();
        let map_name = crate::map_override::is_map_path(path);
        let level = map_name.is_some_and(|name| !shared_map(name));
        self.access += 1;
        if self
            .files
            .get(&key)
            .is_none_or(|entry| matches!(&entry.contents, Cached::Evicted(_)))
        {
            let _scope = mem_watch::scope(if map_name.is_some() {
                "assets.map_rewrite"
            } else {
                "assets.file_rewrite"
            });
            let meta = self.inner.stat(path)?;
            let new = if meta.is_directory() {
                None
            } else if meta.end_of_file > MAX_REWRITE_BYTES {
                self.rewrite.skipped(path);
                None
            } else {
                if level {
                    // The source buffer is needed before we know the output size.
                    self.trim_maps(None, meta.end_of_file);
                }
                let mut original = vec![0u8; meta.end_of_file as usize];
                let mut done = 0;
                while done < original.len() {
                    match self
                        .inner
                        .read_at(path, done as u64, &mut original[done..])?
                    {
                        0 => return Err(Error::FileCorrupt),
                        n => done += n,
                    }
                }
                self.rewrite.rewrite(path, &original)
            };
            let contents = match new {
                Some(bytes) => {
                    if level {
                        self.map_bytes.set(self.map_bytes.get() + bytes.len() as u64);
                    } else if map_name.is_some() {
                        self.shared_map_bytes
                            .set(self.shared_map_bytes.get() + bytes.len() as u64);
                    }
                    Cached::Bytes(Arc::from(bytes))
                }
                None => Cached::Unchanged,
            };
            self.files.insert(
                key.clone(),
                Entry {
                    contents,
                    last_use: self.access,
                },
            );
            if level {
                self.trim_maps(Some(&key), 0);
            }
        }
        let entry = self.files.get_mut(&key).expect("rewrite is cached");
        entry.last_use = self.access;
        match &entry.contents {
            Cached::Bytes(bytes) => Ok(Some(bytes)),
            Cached::Unchanged => Ok(None),
            Cached::Evicted(_) => unreachable!("the current entry is retained"),
        }
    }

    /// Keep open files and the payload serving the current lookup. Trim
    /// idle maps before allocating an incoming source or when a file closes.
    fn trim_maps(&mut self, current: Option<&str>, incoming: u64) {
        while self.map_bytes.get() > self.map_byte_limit.saturating_sub(incoming) {
            let Some((_, entry)) = self
                .files
                .iter_mut()
                .filter(|(key, entry)| {
                    current != Some(key.as_str())
                        && level_map(key)
                        && !self.open_maps.contains_key(key.as_str())
                        && matches!(&entry.contents, Cached::Bytes(_))
                })
                .min_by_key(|(_, entry)| entry.last_use)
            else {
                break;
            };
            let Cached::Bytes(bytes) = &entry.contents else {
                unreachable!("the eviction candidate has bytes");
            };
            let len = bytes.len();
            entry.contents = Cached::Evicted(len);
            self.map_bytes.set(self.map_bytes.get() - len as u64);
        }
    }

    fn rewritten_size(&mut self, path: &str) -> Result<Option<usize>> {
        if !self.rewrite.wants(path) {
            return Ok(None);
        }
        let evicted_size = self
            .files
            .get(&path.to_ascii_lowercase())
            .and_then(|entry| match &entry.contents {
                Cached::Evicted(len) => Some(*len),
                _ => None,
            });
        if evicted_size.is_some() {
            return Ok(evicted_size);
        }
        let size = self.rewritten(path)?.map(|bytes| bytes.len());
        self.trim_maps(None, 0);
        Ok(size)
    }

    fn resize(meta: &mut Metadata, len: usize) {
        let len = len as u64;
        let unit = xiso::SECTOR;
        meta.end_of_file = len;
        meta.allocation_size = len.div_ceil(unit) * unit;
    }
}

impl<B: Backend, R: Rewrite> Backend for RewriteBackend<B, R> {
    fn snapshot_map_bindings(&self) -> std::result::Result<Vec<(String, String)>, crate::StateError> {
        self.inner.snapshot_map_bindings()
    }

    fn opened(&mut self, path: &str) {
        if level_map(path) {
            *self.open_maps.entry(path.to_ascii_lowercase()).or_default() += 1;
        }
        self.inner.opened(path)
    }
    fn closed(&mut self, path: &str) {
        let key = path.to_ascii_lowercase();
        if let Some(count) = self.open_maps.get_mut(&key) {
            *count -= 1;
            if *count == 0 {
                self.open_maps.remove(&key);
                self.trim_maps(None, 0);
            }
        }
        self.inner.closed(path)
    }
    fn rescan(&mut self) {
        self.inner.rescan()
    }
    fn resolves_on_use(&mut self, path: &str) -> bool {
        self.inner.resolves_on_use(path)
    }
    fn map_changed(&mut self, path: &str) -> bool {
        self.inner.map_changed(path)
    }
    fn film_source(&mut self, path: &str) -> Result<Option<crate::FilmMapSource>> {
        if let Some(bytes) = self.rewritten(path)? {
            return Ok(Some(crate::FilmMapSource::memory(bytes.clone())));
        }
        self.inner.film_source(path)
    }
    fn map_ranges(&mut self, path: &str) -> Option<Vec<(u64, u64)>> {
        if self.rewritten_size(path).ok().flatten().is_some() {
            None
        } else {
            self.inner.map_ranges(path)
        }
    }
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        self.inner.map_id(path)
    }
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        let mut meta = self.inner.stat(path)?;
        if let Some(len) = self.rewritten_size(path)? {
            Self::resize(&mut meta, len);
        }
        Ok(meta)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        for e in &mut entries {
            if e.metadata.is_directory() {
                continue;
            }
            let child = if path.is_empty() {
                e.name.clone()
            } else {
                format!("{path}\\{}", e.name)
            };
            if let Some(len) = self.rewritten_size(&child)? {
                Self::resize(&mut e.metadata, len);
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let Some(bytes) = self.rewritten(path)? else {
            return self.inner.read_at(path, offset, buffer);
        };
        let start = usize::try_from(offset)
            .unwrap_or(usize::MAX)
            .min(bytes.len());
        let n = (bytes.len() - start).min(buffer.len());
        buffer[..n].copy_from_slice(&bytes[start..start + n]);
        Ok(n)
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    /// The inner file's host bytes, unless this file may be rewritten.
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        if self.rewrite.wants(path) {
            return None;
        }
        self.inner.host_file(path)
    }
    fn file_source(&mut self, path: &str) -> Option<FileSource> {
        if let Some(bytes) = self.rewritten(path).ok()? {
            return Some(FileSource::Memory(Arc::clone(bytes)));
        }
        self.inner.file_source(path)
    }
    fn create(&mut self, _path: &str, _directory: bool, _attributes: u32) -> Result<()> {
        Err(Error::MediaWriteProtected)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ATTR_DIRECTORY, ATTR_READONLY, BasicInfo};

    /// Files by path; `fonts` and the root are directories.
    struct Mem {
        files: Vec<(&'static str, Vec<u8>)>,
        reads: usize,
    }
    fn meta(len: u64, dir: bool) -> Metadata {
        Metadata {
            basic: BasicInfo {
                attributes: ATTR_READONLY | if dir { ATTR_DIRECTORY } else { 0 },
                ..Default::default()
            },
            end_of_file: len,
            allocation_size: len,
        }
    }
    impl Backend for Mem {
        fn read_only(&self) -> bool {
            true
        }
        fn stat(&mut self, path: &str) -> Result<Metadata> {
            if path.is_empty() || path.eq_ignore_ascii_case("fonts") || path.eq_ignore_ascii_case("maps")
            {
                return Ok(meta(0, true));
            }
            self.files
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .map(|f| meta(f.1.len() as u64, false))
                .ok_or(Error::ObjectNameNotFound)
        }
        fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
            Ok(self
                .files
                .iter()
                .filter_map(|(p, d)| {
                    let name = p.strip_prefix(path)?.strip_prefix('\\')?;
                    Some(DirectoryEntry {
                        name: name.to_owned(),
                        metadata: meta(d.len() as u64, false),
                    })
                })
                .collect())
        }
        fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
            self.reads += 1;
            let f = self
                .files
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .ok_or(Error::ObjectNameNotFound)?;
            // Short reads, to exercise the loop.
            let start = (offset as usize).min(f.1.len());
            let n = (f.1.len() - start).min(buffer.len()).min(3);
            buffer[..n].copy_from_slice(&f.1[start..start + n]);
            Ok(n)
        }
        fn volume_info(&mut self) -> Result<VolumeInfo> {
            Err(Error::InvalidParameter)
        }
    }

    /// Doubles files under `fonts\` except `.txt`; declines ones starting with `!`.
    struct Double(usize);
    impl Rewrite for Double {
        fn wants(&self, path: &str) -> bool {
            let p = path.to_ascii_lowercase();
            p.starts_with("fonts\\") && !p.ends_with(".txt")
        }
        fn rewrite(&mut self, _path: &str, original: &[u8]) -> Option<Vec<u8>> {
            self.0 += 1;
            (original.first() != Some(&b'!')).then(|| [original, original].concat())
        }
    }

    fn backend() -> RewriteBackend<Mem, Double> {
        let files = vec![
            ("fonts\\a", b"abcdefg".to_vec()),
            ("fonts\\table.txt", b"a\r\n".to_vec()),
            ("fonts\\no", b"!keep".to_vec()),
            ("other", b"xyz".to_vec()),
        ];
        RewriteBackend::new(Mem { files, reads: 0 }, Double(0))
    }
    fn read_all(b: &mut impl Backend, path: &str) -> Vec<u8> {
        let mut out = Vec::new();
        let mut buf = [0u8; 5];
        loop {
            match b.read_at(path, out.len() as u64, &mut buf).unwrap() {
                0 => return out,
                n => out.extend_from_slice(&buf[..n]),
            }
        }
    }

    #[test]
    fn wanted_file_is_served_rewritten_with_its_new_size() {
        let mut b = backend();
        assert_eq!(b.stat("FONTS\\A").unwrap().end_of_file, 14);
        assert_eq!(read_all(&mut b, "fonts\\a"), b"abcdefgabcdefg");
        // Past the end, and a read spanning the end.
        let mut buf = [0u8; 8];
        assert_eq!(b.read_at("fonts\\a", 14, &mut buf).unwrap(), 0);
        assert_eq!(b.read_at("fonts\\a", u64::MAX, &mut buf).unwrap(), 0);
        assert_eq!(b.read_at("fonts\\a", 10, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"defg");
        // Rewritten once, however many stats and reads follow.
        assert_eq!(b.rewrite.0, 1);
    }

    #[test]
    fn other_files_pass_through_untouched() {
        let mut b = backend();
        assert_eq!(read_all(&mut b, "other"), b"xyz");
        assert_eq!(read_all(&mut b, "fonts\\table.txt"), b"a\r\n");
        assert_eq!(b.stat("other").unwrap().end_of_file, 3);
        assert_eq!(b.rewrite.0, 0);
        assert!(b.files.is_empty());
    }

    #[test]
    fn declined_file_is_served_from_the_inner_backend_and_asked_once() {
        let mut b = backend();
        assert_eq!(b.stat("fonts\\no").unwrap().end_of_file, 5);
        assert_eq!(read_all(&mut b, "fonts\\no"), b"!keep");
        assert_eq!(read_all(&mut b, "fonts\\no"), b"!keep");
        assert_eq!(b.rewrite.0, 1);
    }

    #[test]
    fn directory_listing_reports_rewritten_sizes() {
        let mut b = backend();
        let sizes: Vec<(String, u64)> = b
            .read_directory("fonts")
            .unwrap()
            .into_iter()
            .map(|e| (e.name, e.metadata.end_of_file))
            .collect();
        assert_eq!(
            sizes,
            [
                ("a".to_owned(), 14),
                ("table.txt".to_owned(), 3),
                ("no".to_owned(), 5)
            ]
        );
    }

    #[derive(Default)]
    struct Maps(usize);
    impl Rewrite for Maps {
        fn wants(&self, path: &str) -> bool {
            crate::map_override::is_map_path(path).is_some()
        }
        fn rewrite(&mut self, _path: &str, original: &[u8]) -> Option<Vec<u8>> {
            self.0 += 1;
            Some([original, original].concat())
        }
    }

    const MAP_A: &str = "maps\\a.map";
    const MAP_B: &str = "maps\\b.map";

    fn map_backend() -> RewriteBackend<Mem, Maps> {
        let files = vec![(MAP_A, b"ab".to_vec()), (MAP_B, b"cd".to_vec())];
        let mut backend = RewriteBackend::new(Mem { files, reads: 0 }, Maps::default());
        backend.map_byte_limit = 4;
        backend
    }

    #[test]
    fn directory_listing_remembers_evicted_sizes_without_retaining_the_maps() {
        let mut backend = map_backend();
        for _ in 0..2 {
            let sizes: Vec<_> = backend
                .read_directory("maps")
                .unwrap()
                .into_iter()
                .map(|e| e.metadata.end_of_file)
                .collect();
            assert_eq!(sizes, [4, 4]);
            assert_eq!(backend.map_bytes.get(), 4);
        }
        assert_eq!(backend.stat(MAP_A).unwrap().end_of_file, 4);
        assert_eq!(backend.rewrite.0, 2);
        assert_eq!(read_all(&mut backend, MAP_A), b"abab");
        assert_eq!(backend.rewrite.0, 3);
        assert_eq!(backend.map_bytes.get(), 4);
    }

    #[test]
    fn an_oversized_listing_payload_is_released_after_reported() {
        let mut backend = map_backend();
        backend.map_byte_limit = 3;
        let entries = backend.read_directory("maps").unwrap();
        assert!(entries.iter().all(|e| e.metadata.end_of_file == 4));
        assert_eq!(backend.map_bytes.get(), 0);
    }

    #[test]
    fn shared_resources_do_not_displace_a_level() {
        let mut backend = map_backend();
        let shared = "maps\\mainmenu.map";
        backend.inner.files.push((shared, b"ef".to_vec()));

        assert_eq!(read_all(&mut backend, MAP_A), b"abab");
        assert_eq!(read_all(&mut backend, shared), b"efef");
        assert_eq!(read_all(&mut backend, MAP_A), b"abab");
        assert_eq!(backend.rewrite.0, 2);
        assert_eq!(backend.map_bytes.get(), 4);
        assert_eq!(backend.shared_map_bytes.get(), 4);
    }

    #[test]
    fn eviction_releases_the_backend_owner_after_the_last_file_object_closes() {
        let mut backend = map_backend();
        let Some(FileSource::Memory(first)) = backend.file_source(MAP_A) else {
            panic!("the first map has no rewritten source");
        };
        backend.opened("MAPS\\A.MAP");
        backend.opened(MAP_A);
        assert_eq!(read_all(&mut backend, MAP_B), b"cdcd");
        backend.opened(MAP_B);
        assert_eq!(backend.map_bytes.get(), 8);

        backend.closed(MAP_A);
        assert_eq!(Arc::strong_count(&first), 2);
        assert_eq!(read_all(&mut backend, MAP_A), b"abab");
        assert_eq!(backend.rewrite.0, 2);
        backend.closed(MAP_A);
        assert_eq!(backend.map_bytes.get(), 4);
        assert_eq!(Arc::strong_count(&first), 1);
        assert_eq!(&*first, b"abab");
        backend.closed(MAP_B);
    }

    #[test]
    fn missing_files_directories_and_writes_fail_as_on_the_inner_backend() {
        let mut b = backend();
        assert_eq!(b.stat("fonts\\missing"), Err(Error::ObjectNameNotFound));
        assert!(b.stat("fonts").unwrap().is_directory());
        assert!(b.read_only());
        assert_eq!(
            b.create("fonts\\new", false, 0),
            Err(Error::MediaWriteProtected)
        );
        assert_eq!(
            b.write_at("fonts\\a", 0, b"x"),
            Err(Error::MediaWriteProtected)
        );
    }
    #[test]
    fn short_source_is_not_rewritten_and_oversized_source_notifies_once() {
        struct Source(u64);
        impl Backend for Source {
            fn read_only(&self) -> bool {
                true
            }
            fn stat(&mut self, _: &str) -> Result<Metadata> {
                Ok(meta(self.0, false))
            }
            fn read_at(&mut self, _: &str, _: u64, _: &mut [u8]) -> Result<usize> {
                Ok(0)
            }
            fn read_directory(&mut self, _: &str) -> Result<Vec<DirectoryEntry>> {
                Ok(vec![])
            }
            fn volume_info(&mut self) -> Result<VolumeInfo> {
                Err(Error::NotSupported)
            }
        }
        struct Notices(usize);
        impl Rewrite for Notices {
            fn wants(&self, _: &str) -> bool {
                true
            }
            fn rewrite(&mut self, _: &str, _: &[u8]) -> Option<Vec<u8>> {
                panic!("incomplete source must not reach the rewrite")
            }
            fn skipped(&mut self, _: &str) {
                self.0 += 1;
            }
        }
        let mut short = RewriteBackend::new(Source(5), Notices(0));
        assert_eq!(short.stat("font"), Err(Error::FileCorrupt));
        let mut large = RewriteBackend::new(Source(MAX_REWRITE_BYTES + 1), Notices(0));
        for _ in 0..2 {
            assert_eq!(
                large.stat("font").unwrap().end_of_file,
                MAX_REWRITE_BYTES + 1
            );
        }
        assert_eq!(large.rewrite.0, 1);
    }

    /// Appends `!` to files under `other`.
    struct Bang;
    impl Rewrite for Bang {
        fn wants(&self, path: &str) -> bool {
            path.to_ascii_lowercase().starts_with("other")
        }
        fn rewrite(&mut self, _path: &str, original: &[u8]) -> Option<Vec<u8>> {
            Some([original, b"!"].concat())
        }
    }

    #[test]
    fn a_chain_rewrites_what_either_wants_one_after_the_other() {
        let files = vec![
            ("fonts\\a", b"ab".to_vec()),
            ("fonts\\no", b"!keep".to_vec()),
            ("other", b"xyz".to_vec()),
            ("rest", b"r".to_vec()),
        ];
        let mut b = RewriteBackend::new(
            Mem { files, reads: 0 },
            Chain(Some(Double(0)), Chain(None::<Double>, Bang)),
        );
        assert_eq!(read_all(&mut b, "fonts\\a"), b"abab");
        assert_eq!(read_all(&mut b, "other"), b"xyz!");
        assert_eq!(read_all(&mut b, "fonts\\no"), b"!keep", "declined by both");
        assert_eq!(read_all(&mut b, "rest"), b"r");
        assert!(!b.rewrite.wants("rest"));
        assert_eq!(b.rewrite.0.as_ref().unwrap().0, 2);

        // Both want it: the second sees the first's output.
        struct Both;
        impl Rewrite for Both {
            fn wants(&self, _: &str) -> bool {
                true
            }
            fn rewrite(&mut self, _: &str, original: &[u8]) -> Option<Vec<u8>> {
                Some([original, b"+"].concat())
            }
        }
        let mut c = Chain(Both, Bang);
        assert_eq!(c.rewrite("other", b"o").unwrap(), b"o+!");
        // The second declining keeps the first's.
        let mut d = Chain(Both, None::<Bang>);
        assert_eq!(d.rewrite("other", b"o").unwrap(), b"o+");
    }
}
