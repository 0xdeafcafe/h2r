//! A read-only backend that serves some files with different contents.
//!
//! [`RewriteBackend`] wraps another backend. For a path the [`Rewrite`] wants,
//! it hands the rewrite the inner file as a [`Source`] and serves what comes
//! back ([`Rewritten`]): new contents whole, or an overlay over the
//! original whose unchanged ranges are read from the inner file by offset,
//! so a map of any size is rewritten holding only what changed. Its size
//! comes from `stat` and directory listings, its bytes from `read_at`. A
//! level's rewritten bytes can be evicted once its file objects close. The
//! shared resource maps stay available across level loads. Everything else
//! passes through. The inner backend is never written.
//!
//! The boot runner uses it to swap button-icon glyphs in the disc's font
//! files (`docs/input/button-icons.md`) and to serve the package set's maps
//! (`docs/runtime/storage.md`, "Filesystem implementations").
use std::collections::HashMap;
use std::io;
use std::sync::Arc;

use content::patch::{Overlay, OverlayView, Source};

use crate::{
    Backend, DirectoryEntry, Error, FileSource, HostFile, Metadata, Result, SplicedFile,
    VolumeInfo,
};

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

/// What a rewrite serves for a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rewritten {
    /// New contents, whole.
    Whole(Vec<u8>),
    /// The original with an overlay over it: what the overlay does not
    /// hold is read from the original by offset, never copied.
    Over(Overlay),
}

impl Rewritten {
    /// The served length.
    pub fn len(&self) -> u64 {
        match self {
            Self::Whole(b) => b.len() as u64,
            Self::Over(o) => o.len(),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    /// Bytes held in memory.
    pub fn held(&self) -> u64 {
        match self {
            Self::Whole(b) => b.len() as u64,
            Self::Over(o) => o.held(),
        }
    }
    /// `next`, made over this one's output, as one result over this one's
    /// original.
    pub fn then(self, next: Rewritten) -> Rewritten {
        match (self, next) {
            (_, Self::Whole(b)) => Self::Whole(b),
            (Self::Whole(a), Self::Over(o)) => Self::Whole(o.materialise(a)),
            (Self::Over(a), Self::Over(b)) => Self::Over(a.then(&b)),
        }
    }
    /// The served bytes, from the whole original.
    pub fn materialise(self, original: &[u8]) -> Vec<u8> {
        match self {
            Self::Whole(b) => b,
            Self::Over(o) => o.materialise(original.to_vec()),
        }
    }
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
    /// [`Rewrite::rewrite`] from the file's `source`, the original, which
    /// it reads as it needs; what the backend calls. The default reads the
    /// whole file and calls `rewrite`. A rewrite that changes part of a
    /// large file returns [`Rewritten::Over`], whose unchanged ranges stay
    /// in `source`.
    fn rewrite_streamed(
        &mut self,
        path: &str,
        source: &mut dyn Source,
    ) -> io::Result<Option<Rewritten>> {
        let original = source.read_all()?;
        Ok(self.rewrite(path, &original).map(Rewritten::Whole))
    }
}

/// No rewrite: wants nothing.
impl<R: Rewrite> Rewrite for Option<R> {
    fn wants(&self, path: &str) -> bool {
        self.as_ref().is_some_and(|r| r.wants(path))
    }
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>> {
        self.as_mut()?.rewrite(path, original)
    }
    fn rewrite_streamed(
        &mut self,
        path: &str,
        source: &mut dyn Source,
    ) -> io::Result<Option<Rewritten>> {
        match self {
            Some(r) => r.rewrite_streamed(path, source),
            None => Ok(None),
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
    fn rewrite_streamed(
        &mut self,
        path: &str,
        source: &mut dyn Source,
    ) -> io::Result<Option<Rewritten>> {
        let first = if self.0.wants(path) {
            self.0.rewrite_streamed(path, source)?
        } else {
            None
        };
        if !self.1.wants(path) {
            return Ok(first);
        }
        let second = match &first {
            None => self.1.rewrite_streamed(path, source)?,
            Some(Rewritten::Whole(b)) => self.1.rewrite_streamed(path, &mut b.as_slice())?,
            Some(Rewritten::Over(o)) => self.1.rewrite_streamed(
                path,
                &mut OverlayView {
                    overlay: o,
                    base: source,
                },
            )?,
        };
        Ok(match (first, second) {
            (Some(a), Some(b)) => Some(a.then(b)),
            (a, b) => b.or(a),
        })
    }
}

/// The inner backend's file at `path` as a [`Source`], keeping the
/// backend's own error when a read fails.
struct InnerFile<'a, B> {
    inner: &'a mut B,
    path: &'a str,
    len: u64,
    failed: Option<Error>,
}

impl<B: Backend> Source for InnerFile<'_, B> {
    fn size(&self) -> u64 {
        self.len
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        let n = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        self.inner.read_at(self.path, offset, &mut buf[..n]).map_err(|e| {
            self.failed = Some(e);
            io::Error::other(format!("{e:?}"))
        })
    }
}

fn io_error(e: io::Error) -> Error {
    match e.kind() {
        io::ErrorKind::UnexpectedEof => Error::FileCorrupt,
        _ => e.into(),
    }
}

/// A rewritten file as served.
#[derive(Clone)]
enum Served {
    Bytes(Arc<[u8]>),
    /// An overlay over the inner file, which is `base_len` bytes; `id`
    /// names the served bytes for a film's stamp.
    Over {
        overlay: Arc<Overlay>,
        base_len: u64,
        id: content::MapId,
    },
}

impl Served {
    fn len(&self) -> u64 {
        match self {
            Self::Bytes(b) => b.len() as u64,
            Self::Over { overlay, .. } => overlay.len(),
        }
    }
    fn held(&self) -> u64 {
        match self {
            Self::Bytes(b) => b.len() as u64,
            Self::Over { overlay, .. } => overlay.held(),
        }
    }
}

/// A name for an overlay's bytes over the inner file at `path`.
fn overlay_id(path: &str, base_len: u64, overlay: &Overlay) -> content::MapId {
    let mut h = content::Hasher::default();
    h.update(b"H2 rewrite overlay v1\0");
    h.update(path.to_ascii_lowercase().as_bytes());
    for v in [base_len, overlay.base_visible(), overlay.len()] {
        h.update(&v.to_le_bytes());
    }
    for r in overlay.runs() {
        h.update(&r.offset.to_le_bytes());
        h.update(&(r.bytes.len() as u64).to_le_bytes());
        h.update(&r.bytes);
    }
    content::MapId(h.finish().0)
}

enum Cached {
    Unchanged,
    Served(Served),
    /// Retain the directory listing size without rebuilding an idle map.
    Evicted(u64),
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
    fn rewritten(&mut self, path: &str) -> Result<Option<Served>> {
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
            } else {
                let mut source = InnerFile {
                    inner: &mut self.inner,
                    path,
                    len: meta.end_of_file,
                    failed: None,
                };
                let done = self.rewrite.rewrite_streamed(path, &mut source);
                if let Some(e) = source.failed {
                    return Err(e);
                }
                done.map_err(io_error)?
            };
            let contents = match new {
                Some(rewritten) => {
                    let held = rewritten.held();
                    if level {
                        self.map_bytes.set(self.map_bytes.get() + held);
                    } else if map_name.is_some() {
                        self.shared_map_bytes
                            .set(self.shared_map_bytes.get() + held);
                    }
                    Cached::Served(match rewritten {
                        Rewritten::Whole(bytes) => Served::Bytes(Arc::from(bytes)),
                        Rewritten::Over(overlay) => Served::Over {
                            id: overlay_id(path, meta.end_of_file, &overlay),
                            overlay: Arc::new(overlay),
                            base_len: meta.end_of_file,
                        },
                    })
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
                self.trim_maps(Some(&key));
            }
        }
        let entry = self.files.get_mut(&key).expect("rewrite is cached");
        entry.last_use = self.access;
        match &entry.contents {
            Cached::Served(served) => Ok(Some(served.clone())),
            Cached::Unchanged => Ok(None),
            Cached::Evicted(_) => unreachable!("the current entry is retained"),
        }
    }

    /// Keep open files and the payload serving the current lookup. Trim
    /// idle maps when a file closes or a payload is added.
    fn trim_maps(&mut self, current: Option<&str>) {
        while self.map_bytes.get() > self.map_byte_limit {
            let Some((_, entry)) = self
                .files
                .iter_mut()
                .filter(|(key, entry)| {
                    current != Some(key.as_str())
                        && level_map(key)
                        && !self.open_maps.contains_key(key.as_str())
                        && matches!(&entry.contents, Cached::Served(_))
                })
                .min_by_key(|(_, entry)| entry.last_use)
            else {
                break;
            };
            let Cached::Served(served) = &entry.contents else {
                unreachable!("the eviction candidate is served");
            };
            let (len, held) = (served.len(), served.held());
            entry.contents = Cached::Evicted(len);
            self.map_bytes.set(self.map_bytes.get() - held);
        }
    }

    fn rewritten_size(&mut self, path: &str) -> Result<Option<u64>> {
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
        let size = self.rewritten(path)?.map(|served| served.len());
        self.trim_maps(None);
        Ok(size)
    }

    fn resize(meta: &mut Metadata, len: u64) {
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
                self.trim_maps(None);
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
        match self.rewritten(path)? {
            Some(Served::Bytes(bytes)) => Ok(Some(crate::FilmMapSource::memory(bytes))),
            Some(Served::Over {
                overlay,
                base_len,
                id,
            }) => Ok(self
                .inner
                .film_source(path)?
                .map(|base| crate::FilmMapSource::overlay(base, base_len, overlay, id))),
            None => self.inner.film_source(path),
        }
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
        match self.rewritten(path)? {
            None => self.inner.read_at(path, offset, buffer),
            Some(Served::Bytes(bytes)) => {
                let start = usize::try_from(offset)
                    .unwrap_or(usize::MAX)
                    .min(bytes.len());
                let n = (bytes.len() - start).min(buffer.len());
                buffer[..n].copy_from_slice(&bytes[start..start + n]);
                Ok(n)
            }
            Some(Served::Over {
                overlay, base_len, ..
            }) => {
                let mut source = InnerFile {
                    inner: &mut self.inner,
                    path,
                    len: base_len,
                    failed: None,
                };
                let done = overlay.read_at(&mut source, offset, buffer);
                if let Some(e) = source.failed {
                    return Err(e);
                }
                done.map_err(io_error)
            }
        }
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
    /// A rewritten file's bytes in memory, or the inner file's host region
    /// with the overlay over it. An overlay over an inner file with no host
    /// region has no source outside the VFS.
    fn file_source(&mut self, path: &str) -> Option<FileSource> {
        match self.rewritten(path).ok()? {
            Some(Served::Bytes(bytes)) => Some(FileSource::Memory(bytes)),
            Some(Served::Over { overlay, .. }) => Some(FileSource::Spliced(SplicedFile {
                base: self.inner.host_file(path)?,
                overlay,
            })),
            None => self.inner.file_source(path),
        }
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
    fn short_source_is_not_rewritten() {
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
        struct Never;
        impl Rewrite for Never {
            fn wants(&self, _: &str) -> bool {
                true
            }
            fn rewrite(&mut self, _: &str, _: &[u8]) -> Option<Vec<u8>> {
                panic!("incomplete source must not reach the rewrite")
            }
        }
        let mut short = RewriteBackend::new(Source(5), Never);
        assert_eq!(short.stat("font"), Err(Error::FileCorrupt));
    }

    /// Over every map, an overlay: `head` over the first bytes, then the
    /// file's first byte, the file cut to `keep` and `tail` appended. It
    /// reads only that byte.
    struct Splice {
        head: Vec<u8>,
        keep: u64,
        tail: Vec<u8>,
        asked: usize,
    }
    impl Rewrite for Splice {
        fn wants(&self, path: &str) -> bool {
            crate::map_override::is_map_path(path).is_some()
        }
        fn rewrite(&mut self, _: &str, _: &[u8]) -> Option<Vec<u8>> {
            unreachable!("the backend asks for the streamed rewrite")
        }
        fn rewrite_streamed(
            &mut self,
            _: &str,
            source: &mut dyn content::patch::Source,
        ) -> io::Result<Option<Rewritten>> {
            self.asked += 1;
            let mut first = [0];
            source.read_exact_at(0, &mut first)?;
            let mut o = Overlay::new(source.size());
            o.set_len(self.keep);
            o.write(0, &self.head);
            o.write(self.head.len() as u64, &first);
            o.write(self.keep, &self.tail);
            Ok(Some(Rewritten::Over(o)))
        }
    }

    /// An inner map that counts the bytes read from it.
    struct Big {
        len: u64,
        read: u64,
    }
    fn big_byte(at: u64) -> u8 {
        (at.wrapping_mul(0x9e37_79b9) >> 7) as u8
    }
    impl Backend for Big {
        fn read_only(&self) -> bool {
            true
        }
        fn stat(&mut self, path: &str) -> Result<Metadata> {
            Ok(meta(if path.is_empty() { 0 } else { self.len }, path.is_empty()))
        }
        fn read_at(&mut self, _: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
            let n = self.len.saturating_sub(offset).min(buffer.len() as u64) as usize;
            for (i, b) in buffer[..n].iter_mut().enumerate() {
                *b = big_byte(offset + i as u64);
            }
            self.read += n as u64;
            Ok(n)
        }
        fn read_directory(&mut self, _: &str) -> Result<Vec<DirectoryEntry>> {
            Ok(vec![])
        }
        fn volume_info(&mut self) -> Result<VolumeInfo> {
            Err(Error::NotSupported)
        }
    }

    #[test]
    fn an_overlay_serves_the_unchanged_ranges_from_the_inner_file() {
        let len = 1 << 30;
        let splice = Splice {
            head: b"daeh".to_vec(),
            keep: len - 4096,
            tail: vec![7; 10_000],
            asked: 0,
        };
        let mut b = RewriteBackend::new(Big { len, read: 0 }, splice);
        let size = len - 4096 + 10_000;
        assert_eq!(b.stat(MAP_A).unwrap().end_of_file, size);
        // The rewrite read one byte; it holds the head, that byte and the tail.
        assert_eq!(b.inner.read, 1);
        assert_eq!(b.map_bytes.get(), 4 + 1 + 10_000);
        let want = |at: u64| match at {
            0..4 => b"daeh"[at as usize],
            4 => big_byte(0),
            at if at < len - 4096 => big_byte(at),
            _ => 7,
        };
        let mut seed = 1u64;
        for _ in 0..200 {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            let at = (seed >> 20) % (size + 16);
            let mut buf = vec![0xee; ((seed >> 8) % 9000) as usize];
            let n = b.read_at(MAP_A, at, &mut buf).unwrap();
            assert_eq!(n as u64, size.saturating_sub(at).min(buf.len() as u64));
            for (i, &got) in buf[..n].iter().enumerate() {
                assert_eq!(got, want(at + i as u64), "byte {:#x}", at + i as u64);
            }
        }
        assert_eq!(b.rewrite.asked, 1);
        // No host region under it: no source outside the VFS.
        assert!(b.file_source(MAP_A).is_none());
    }

    #[test]
    fn a_chain_of_overlays_reads_as_one() {
        struct Upper;
        impl Rewrite for Upper {
            fn wants(&self, _: &str) -> bool {
                true
            }
            fn rewrite(&mut self, _: &str, original: &[u8]) -> Option<Vec<u8>> {
                Some(original.to_ascii_uppercase())
            }
        }
        let files = vec![(MAP_A, b"abcdefghij".to_vec())];
        let splice = || Splice {
            head: b"X".to_vec(),
            keep: 6,
            tail: b"tail".to_vec(),
            asked: 0,
        };
        let mut both = RewriteBackend::new(Mem { files: files.clone(), reads: 0 }, Chain(splice(), splice()));
        // The second reads its byte 0 from the first's output (`X`).
        assert_eq!(read_all(&mut both, MAP_A), b"XXcdeftail");
        assert!(matches!(
            both.rewritten(MAP_A).unwrap(),
            Some(Served::Over { .. })
        ));
        let mut then_whole = RewriteBackend::new(Mem { files: files.clone(), reads: 0 }, Chain(splice(), Upper));
        assert_eq!(read_all(&mut then_whole, MAP_A), b"XACDEFTAIL");
        let mut whole_then = RewriteBackend::new(Mem { files, reads: 0 }, Chain(Upper, splice()));
        assert_eq!(read_all(&mut whole_then, MAP_A), b"XACDEFtail");
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
