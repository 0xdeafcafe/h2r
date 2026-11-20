//! Film observers identify the bytes that the mounted backend actually served.
//! Keep the opened backing alive while the recorder's worker hashes it. This
//! matters when an editor replaces a map or the selected assets change.
use crate::{Error, Result, Vfs, sim_store::Store};
use content::{Hasher, MapId, Sha256};
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
};

enum Bytes {
    Region(File, u64),
    Store(Arc<Store>),
    Overlay {
        base: Box<FilmMapSource>,
        len: u64,
        patch: Arc<content::patch::Overlay>,
        id: MapId,
    },
    Memory(Arc<[u8]>),
    Backend {
        vfs: Arc<Mutex<Vfs>>,
        mount: usize,
        path: String,
    },
}

/// An opened file region, sparse store, or composed logical map stream.
pub struct FilmMapSource {
    bytes: Bytes,
    pub(crate) override_name: Option<String>,
    /// Header bytes rewritten by a registered package alias.
    pub(crate) alias: Option<[u8; 32]>,
}

impl FilmMapSource {
    fn new(bytes: Bytes) -> Self {
        Self {
            bytes,
            override_name: None,
            alias: None,
        }
    }
    pub(crate) fn region(file: File, offset: u64) -> Self {
        Self::new(Bytes::Region(file, offset))
    }
    pub(crate) fn store(store: Arc<Store>) -> Self {
        Self::new(Bytes::Store(store))
    }
    pub(crate) fn overlay(
        base: Self,
        len: u64,
        patch: Arc<content::patch::Overlay>,
        id: MapId,
    ) -> Self {
        Self::new(Bytes::Overlay {
            base: Box::new(base),
            len,
            patch,
            id,
        })
    }
    pub(crate) fn memory(bytes: Arc<[u8]>) -> Self {
        Self::new(Bytes::Memory(bytes))
    }
    fn read(&self, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let n = match &self.bytes {
            Bytes::Region(file, start) => {
                let at = start.checked_add(offset).ok_or(Error::FileCorrupt)?;
                #[cfg(unix)]
                {
                    std::os::unix::fs::FileExt::read_at(file, buffer, at)?
                }
                #[cfg(windows)]
                {
                    std::os::windows::fs::FileExt::seek_read(file, buffer, at)?
                }
                #[cfg(not(any(unix, windows)))]
                {
                    return Err(Error::NotSupported);
                }
            }
            Bytes::Store(store) => store.read_at(offset, buffer)?,
            Bytes::Overlay {
                base, len, patch, ..
            } => patch.read_at(&mut OverlayBase(base, *len), offset, buffer)?,
            Bytes::Memory(bytes) => {
                let i = usize::try_from(offset).map_err(|_| Error::FileCorrupt)?;
                bytes.get(i..).ok_or(Error::FileCorrupt)?.read(buffer)?
            }
            Bytes::Backend { vfs, mount, path } => {
                let mut vfs = vfs.lock().unwrap_or_else(|e| e.into_inner());
                vfs.mounts[*mount].backend.read_at(path, offset, buffer)?
            }
        };
        if let Some(alias) = &self.alias {
            let first = offset.max(0x198);
            let last = (offset + n as u64).min(0x198 + 32);
            if first < last {
                buffer[(first - offset) as usize..(last - offset) as usize]
                    .copy_from_slice(&alias[(first - 0x198) as usize..(last - 0x198) as usize]);
            }
        }
        Ok(n)
    }

    // fstat the open file, never the pathname (which might already name its
    // replacement). Inode change time catches edits with a preserved mtime.
    #[cfg(unix)]
    fn stamp(&self) -> Result<Option<Sha256>> {
        use std::os::unix::fs::MetadataExt;
        let (kind, file, offset) = match &self.bytes {
            Bytes::Region(f, offset) => (0, f, *offset),
            Bytes::Store(s) => (1, &s.file, 0),
            Bytes::Overlay { base, len, id, .. } => {
                let Some(base) = base.stamp()? else {
                    return Ok(None);
                };
                let mut h = Hasher::default();
                h.update(b"H2FILM map overlay v1\0");
                h.update(&base.0);
                h.update(&len.to_le_bytes());
                h.update(&id.0);
                if let Some(alias) = &self.alias {
                    h.update(alias)
                }
                return Ok(Some(h.finish()));
            }
            _ => return Ok(None),
        };
        let m = file.metadata()?;
        let mut h = Hasher::default();
        h.update(b"H2FILM map source v1\0");
        h.update(&[kind]);
        for v in [
            m.dev(),
            m.ino(),
            m.len(),
            offset,
            m.mtime() as u64,
            m.mtime_nsec() as u64,
            m.ctime() as u64,
            m.ctime_nsec() as u64,
        ] {
            h.update(&v.to_le_bytes());
        }
        if let Some(alias) = &self.alias {
            h.update(alias)
        }
        Ok(Some(h.finish()))
    }
    #[cfg(not(unix))]
    fn stamp(&self) -> Result<Option<Sha256>> {
        Ok(None)
    }
}

struct OverlayBase<'a>(&'a FilmMapSource, u64);

impl content::patch::Source for OverlayBase<'_> {
    fn size(&self) -> u64 {
        self.1
    }
    fn read_at(&mut self, offset: u64, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.0.read(offset, buffer).map_err(std::io::Error::other)
    }
}

impl Drop for FilmMapSource {
    fn drop(&mut self) {
        if let Bytes::Backend { vfs, mount, path } = &self.bytes {
            vfs.lock().unwrap_or_else(|e| e.into_inner()).mounts[*mount]
                .backend
                .closed(path);
        }
    }
}

/// The mounted source and exact extent selected at the match-start tick.
pub struct FilmMap {
    source: FilmMapSource,
    stamp: Option<Sha256>,
    len: u64,
    ranges: Option<Vec<(u64, u64)>>,
    package: Option<MapId>,
}

impl FilmMap {
    /// With no recorded extent, use the mounted store's index if present.
    /// Playback supplies the recorded extent, including a whole-file extent.
    pub fn open(
        vfs: &Arc<Mutex<Vfs>>,
        name: &str,
        recorded: Option<Option<&[(u64, u64)]>>,
    ) -> Result<Self> {
        let mut guard = vfs.lock().unwrap_or_else(|e| e.into_inner());
        let (mount, path) = guard.resolve(name)?;
        let backend = &mut guard.mounts[mount].backend;
        let info = backend.stat(&path)?;
        if info.is_directory() {
            return Err(Error::FileIsADirectory);
        }
        let ranges = match recorded {
            Some(ranges) => ranges.map(Vec::from),
            None => backend.map_ranges(&path),
        };
        if let Some(spans) = &ranges {
            if spans.is_empty() || spans.len() > 1 << 20 {
                return Err(Error::FileCorrupt);
            }
            let mut previous = 0;
            for &(start, len) in spans {
                if start < previous || len == 0 {
                    return Err(Error::FileCorrupt);
                }
                previous = start
                    .checked_add(len)
                    .filter(|&end| end <= info.end_of_file)
                    .ok_or(Error::FileCorrupt)?;
            }
        }
        let detached = backend.film_source(&path)?;
        let derived = detached.as_ref().and_then(|s| s.override_name.clone());
        let package = if derived.is_some() {
            None
        } else {
            backend.map_id(&path)
        };
        // Unix pread leaves a shared file description's seek cursor untouched.
        // On other targets use the serialised backend's own read operation.
        let detached = if cfg!(unix) { detached } else { None };
        let source = detached.unwrap_or_else(|| {
            backend.opened(&path);
            let mut source = FilmMapSource::new(Bytes::Backend {
                vfs: vfs.clone(),
                mount,
                path,
            });
            source.override_name = derived;
            source
        });
        drop(guard);
        let stamp = source.stamp()?;
        Ok(Self {
            source,
            stamp,
            len: info.end_of_file,
            ranges,
            package,
        })
    }

    pub fn ranges(&self) -> Option<&[(u64, u64)]> {
        self.ranges.as_deref()
    }

    /// Hash with a machine-local cache shared across host leases. Only stable
    /// file generations are persisted. An uncacheable overlay still hashes
    /// the VFS's logical bytes, holding its lock for at most one block's read.
    pub fn digest(&self, cache: &Path) -> Result<(Sha256, Option<MapId>)> {
        let hash = self.hash_extent(cache, self.ranges())?;
        let package = if let Some(name) = &self.source.override_name {
            let whole = if self.ranges.is_some() {
                self.hash_extent(cache, None)?
            } else {
                hash
            };
            Some(crate::map_override::override_id_from_hash(name, whole))
        } else {
            self.package
        };
        if self.source.stamp()? != self.stamp {
            return Err(Error::SharingViolation);
        }
        Ok((hash, package))
    }

    fn hash_extent(&self, cache: &Path, ranges: Option<&[(u64, u64)]>) -> Result<Sha256> {
        let key = self.stamp.map(|stamp| {
            let mut h = Hasher::default();
            h.update(&stamp.0);
            h.update(&self.len.to_le_bytes());
            if let Some(ranges) = ranges {
                for &(start, len) in ranges {
                    h.update(&start.to_le_bytes());
                    h.update(&len.to_le_bytes());
                }
            }
            h.finish()
        });
        if let Some(key) = key
            && let Some(hit) = cached(cache, key)
        {
            return Ok(hit);
        }
        let mut hash = Hasher::default();
        if let Some(ranges) = ranges {
            hash.update(b"H2FILM sparse map v1\0");
            hash.update(&self.len.to_le_bytes());
            hash.update(&(ranges.len() as u64).to_le_bytes());
        }
        let mut buf = vec![0; 256 * 1024];
        for &(start, len) in ranges.unwrap_or(&[(0, self.len)]) {
            if ranges.is_some() {
                hash.update(&start.to_le_bytes());
                hash.update(&len.to_le_bytes());
            }
            let mut done = 0;
            while done < len {
                let want = (len - done).min(buf.len() as u64) as usize;
                let n = self.source.read(start + done, &mut buf[..want])?;
                if n == 0 || n > want {
                    return Err(Error::FileCorrupt);
                }
                hash.update(&buf[..n]);
                done += n as u64;
            }
        }
        let digest = hash.finish();
        if self.source.stamp()? != self.stamp {
            return Err(Error::SharingViolation);
        }
        if let Some(key) = key {
            store(cache, key, digest);
        }
        Ok(digest)
    }
}

fn receipt(key: Sha256, digest: Sha256) -> [u8; 64] {
    let mut data = [0; 64];
    data[..32].copy_from_slice(&key.0);
    data[32..].copy_from_slice(&digest.0);
    let check = Sha256::of(&data);
    data[..32].copy_from_slice(&check.0);
    data
}

fn cached(cache: &Path, key: Sha256) -> Option<Sha256> {
    let mut file = File::open(cache.join(key.to_string())).ok()?;
    if file.metadata().ok()?.len() != 64 {
        return None;
    }
    let mut bytes = [0; 64];
    file.read_exact(&mut bytes).ok()?;
    let digest = Sha256(bytes[32..].try_into().ok()?);
    (bytes == receipt(key, digest)).then_some(digest)
}

fn store(cache: &Path, key: Sha256, digest: Sha256) {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    if fs::create_dir_all(cache).is_err() {
        return;
    }
    let tmp = cache.join(format!(
        ".{}.{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let Ok(mut file) = File::create_new(&tmp) else {
        return;
    };
    if file.write_all(&receipt(key, digest)).is_err()
        || fs::rename(&tmp, cache.join(key.to_string())).is_err()
    {
        let _ = fs::remove_file(tmp);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    fn whole(path: &Path) -> FilmMap {
        let f = File::open(path).unwrap();
        let len = f.metadata().unwrap().len();
        let source = FilmMapSource::region(f, 0);
        FilmMap {
            stamp: source.stamp().unwrap(),
            len,
            ranges: None,
            package: None,
            source,
        }
    }

    #[test]
    fn cached_digests_follow_inode_edits_replacements_and_sparse_extents() {
        let root = std::env::temp_dir().join(format!("h2-film-map-{}", std::process::id()));
        fs::create_dir_all(&root).unwrap();
        let cache = root.join("digests");
        let path = root.join("level.map");
        fs::write(&path, b"abcd").unwrap();
        let first = whole(&path);
        assert_eq!(first.digest(&cache).unwrap().0, Sha256::of(b"abcd"));
        let cached_file = fs::read_dir(&cache)
            .unwrap()
            .next()
            .unwrap()
            .unwrap()
            .path();
        let first_receipt = fs::read(&cached_file).unwrap();
        assert_eq!(first_receipt.len(), 64);
        let fixed_time = std::time::UNIX_EPOCH + std::time::Duration::from_secs(12345);
        File::options()
            .write(true)
            .open(&cached_file)
            .unwrap()
            .set_modified(fixed_time)
            .unwrap();
        assert_eq!(whole(&path).digest(&cache).unwrap().0, Sha256::of(b"abcd"));
        assert_eq!(
            fs::metadata(&cached_file).unwrap().modified().unwrap(),
            fixed_time
        );
        fs::write(&cached_file, &first_receipt[..17]).unwrap();
        assert_eq!(whole(&path).digest(&cache).unwrap().0, Sha256::of(b"abcd"));
        assert_eq!(fs::read(&cached_file).unwrap(), first_receipt);

        fs::write(&path, b"efgh").unwrap();
        File::options()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(fixed_time)
            .unwrap();
        assert_eq!(first.digest(&cache), Err(Error::SharingViolation));
        let mut edited = whole(&path);
        assert_eq!(edited.digest(&cache).unwrap().0, Sha256::of(b"efgh"));
        edited.ranges = Some(vec![(1, 2)]);
        let sparse = edited.digest(&cache).unwrap().0;
        assert_ne!(sparse, Sha256::of(b"efgh"));
        edited.ranges = Some(vec![(0, 2)]);
        assert_ne!(edited.digest(&cache).unwrap().0, sparse);

        let replacement = root.join("replacement");
        fs::write(&replacement, b"ijkl").unwrap();
        fs::rename(replacement, &path).unwrap();
        assert_eq!(whole(&path).digest(&cache).unwrap().0, Sha256::of(b"ijkl"));
        fs::remove_dir_all(root).unwrap();
    }
}
