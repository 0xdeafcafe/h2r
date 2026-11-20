//! A read-only backend that serves some files with different contents.
//!
//! [`RewriteBackend`] wraps another backend. For a path the [`Rewrite`] wants,
//! it reads the whole inner file once, hands the bytes to the rewrite, and
//! from then on serves the result: its size from `stat` and directory
//! listings, its bytes from `read_at`. Everything else passes through. The
//! inner backend is never written; rewritten files live in memory only.
//!
//! The boot runner uses it to swap button-icon glyphs in the disc's font
//! files (`docs/input/button-icons.md`).
use std::collections::HashMap;
use std::sync::Arc;

use crate::{Backend, DirectoryEntry, Error, FileSource, HostFile, Metadata, Result, VolumeInfo};

/// Files larger than this are served as they are, without asking the rewrite.
pub const MAX_REWRITE_BYTES: u64 = 64 * 1024 * 1024;

/// Decides which files change and produces their new contents.
pub trait Rewrite: Send {
    /// Whether `path` (root-relative, backslash-separated, any case) may be
    /// rewritten. Must be cheap: it runs on every `stat` and `read_at`.
    fn wants(&self, path: &str) -> bool;
    /// New contents for `path`, or `None` to serve `original` unchanged.
    /// Called at most once per file.
    fn rewrite(&mut self, path: &str, original: &[u8]) -> Option<Vec<u8>>;
    /// A wanted file exceeded the size limit and will be served unchanged.
    /// Called once, just like `rewrite`, so dependent substitutions can stop.
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

pub struct RewriteBackend<B, R> {
    inner: B,
    rewrite: R,
    /// By lowercase path: the rewritten bytes, or `None` for a file the
    /// rewrite declined (so it is not read and asked again).
    files: HashMap<String, Option<Arc<[u8]>>>,
}

impl<B: Backend, R: Rewrite> RewriteBackend<B, R> {
    pub fn new(inner: B, rewrite: R) -> Self {
        Self {
            inner,
            rewrite,
            files: HashMap::new(),
        }
    }

    /// The rewritten contents of `path`, loading and rewriting it on first
    /// use. `None` when the file is served from the inner backend.
    fn rewritten(&mut self, path: &str) -> Result<Option<&Arc<[u8]>>> {
        if path.is_empty() || !self.rewrite.wants(path) {
            return Ok(None);
        }
        let key = path.to_ascii_lowercase();
        if !self.files.contains_key(&key) {
            let meta = self.inner.stat(path)?;
            let new = if meta.is_directory() {
                None
            } else if meta.end_of_file > MAX_REWRITE_BYTES {
                self.rewrite.skipped(path);
                None
            } else {
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
            self.files.insert(key.clone(), new.map(Arc::from));
        }
        Ok(self.files[&key].as_ref())
    }

    fn resize(meta: &mut Metadata, len: usize) {
        let len = len as u64;
        let unit = xiso::SECTOR;
        meta.end_of_file = len;
        meta.allocation_size = len.div_ceil(unit) * unit;
    }
}

impl<B: Backend, R: Rewrite> Backend for RewriteBackend<B, R> {
    fn opened(&mut self, path: &str) {
        self.inner.opened(path)
    }
    fn closed(&mut self, path: &str) {
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
        if self.rewrite.wants(path) && self.rewritten(path).ok().flatten().is_some() {
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
        if let Some(bytes) = self.rewritten(path)? {
            Self::resize(&mut meta, bytes.len());
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
            if let Some(bytes) = self.rewritten(&child)? {
                Self::resize(&mut e.metadata, bytes.len());
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
            if path.is_empty() || path.eq_ignore_ascii_case("fonts") {
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
