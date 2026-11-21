//! The bots' nav meshes, built while the game runs (docs/game/bots.md §10,
//! stage 2, "Built for you"): [`bots::autonav::Worker`] on a background
//! thread of its own ([`background::spawn`]), with nav probes and
//! visibility grid rows split across [`background::workers`] lowered
//! threads, so the boot and a map
//! load never wait for it, nor for the cores it uses.
//!
//! Work is requested when a map is booted, picked with Clankers on, or
//! loaded. The worker sleeps between requests. It opens the named map as
//! the VFS serves it, including Map Pack files and editor overrides.
//! An unresolved package waits for the game's first open. Until a mesh
//! arrives the bots walk toward known goals; the mesh is offered without
//! reloading the map ([`bots::autonav::offer`]).
//!
//! `H2_BOTS_NAV_AUTO=0` turns it off; `h2 bots nav` still builds by hand.

use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use bots::autonav::{self, Handle, Region, Source, Worker};

type SharedVfs = Arc<Mutex<vfs::Vfs>>;

/// Starts the worker unless [`autonav::ENV`] says off. `first`: the map
/// the boot loads, if it knows.
pub fn start(
    data_root: &Path,
    vfs: SharedVfs,
    first: Option<String>,
) -> Option<Handle> {
    let v = std::env::var(autonav::ENV).ok();
    start_with(
        autonav::enabled(v.as_deref()),
        data_root,
        vfs,
        first,
    )
}

/// [`start`] with the switch given.
pub fn start_with(
    on: bool,
    data_root: &Path,
    vfs: SharedVfs,
    first: Option<String>,
) -> Option<Handle> {
    if !on {
        eprintln!(
            "bots: nav meshes are not built automatically ({}=0); `h2 bots nav all` builds them",
            autonav::ENV
        );
        return None;
    }
    let content = content_root(data_root);
    let (worker, handle) = Worker::new(bots::cache::dir(data_root), Box::new(Vec::new));
    let worker = worker
        .shared_cache(autonav::shared_cache_dir())
        .on_demand(move |name, source| lookup(&vfs, &content, name, source))
        .stay(true)
        .background(true);
    if let Some(first) = first {
        handle.want(&first);
    }
    let spawned = background::spawn("bots-nav", move || {
        worker.run();
    });
    if let Err(e) = spawned {
        eprintln!("bots: nav meshes not built: {e}");
        return None;
    }
    autonav::set_handle(handle.clone());
    Some(handle)
}

/// Whether any bot plays this map (excluding the menu and shared maps).
fn played(name: &str) -> bool {
    autonav::map_use(name, None) != autonav::MapUse::Never
}

/// Where content packages live on the host: `T:\$c`.
fn content_root(data_root: &Path) -> PathBuf {
    vfs::Config::new(data_root, crate::new_maps::HALO2_TITLE)
        .title_data_dir()
        .join(vfs::CONTENT_DIR)
}

/// Find the one requested map. Installed physical Map Pack offers need
/// no generated package registration. Look for their filename outside
/// the VFS lock when the disc has no such map.
fn lookup(vfs: &SharedVfs, content: &Path, name: &str, source: Option<&str>) -> Option<Box<dyn Source>> {
    let _stage = timeline::setup::Stage::Nav.worker();
    if !played(name) || name.contains(['/', '\\']) {
        return None;
    }
    if let Some(path) = source {
        return vfs_source(vfs, name, path);
    }
    let package = vfs.lock().unwrap_or_else(|e| e.into_inner()).content_map_path(name);
    if let Some(path) = package {
        return vfs_source(vfs, name, &path);
    }
    let disc = format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE);
    if let Some(source) = vfs_source(vfs, name, &disc) {
        return Some(source);
    }
    let filename = format!("{name}.map");
    let mut offers: Vec<_> = std::fs::read_dir(content).ok()?.flatten().collect();
    offers.sort_by_key(|offer| offer.file_name());
    for offer in offers {
        let Some(file) = std::fs::read_dir(offer.path()).ok().and_then(|files| {
            files.flatten().find(|entry| entry.file_name().to_string_lossy().eq_ignore_ascii_case(&filename))
        }) else { continue };
        let path = format!(
            "{}\\TDATA\\{:08X}\\{}\\{}\\{}",
            vfs::DATA_DEVICE, crate::new_maps::HALO2_TITLE, vfs::CONTENT_DIR,
            offer.file_name().to_string_lossy(), file.file_name().to_string_lossy(),
        );
        if let Some(source) = vfs_source(vfs, name, &path) {
            return Some(source);
        }
    }
    None
}

/// Pin the opened source, including an overlay and any alias in its
/// header. Backends without a detached Unix reader use a host region or
/// a chunked VFS handle. The worker leaves a cold package to the game.
pub fn vfs_source(vfs: &SharedVfs, key: &str, path: &str) -> Option<Box<dyn Source>> {
    let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
    if v.resolves_on_use(path) {
        return None;
    }
    #[cfg(unix)]
    if let Some((reader, len)) = v.detached_map_reader(path) {
        return Some(Box::new(Region::new(key, 0, len, Box::new(move || {
            Ok(Box::new(std::io::BufReader::new(reader.clone())))
        }))));
    }
    if let Some(h) = v.host_file(path) {
        let file = h.path.clone();
        return Some(Box::new(Region::new(
            key,
            h.offset,
            h.len,
            Box::new(move || {
                Ok(Box::new(std::io::BufReader::new(std::fs::File::open(
                    &file,
                )?)))
            }),
        )));
    }
    let r = v.open(path, vfs::OpenOptions::default()).ok()?;
    let len = v
        .query_information(r.handle)
        .map(|i| i.metadata.end_of_file);
    let _ = v.close(r.handle);
    let len = len.ok()?;
    drop(v);
    let (vfs, path) = (Arc::clone(vfs), path.to_owned());
    Some(Box::new(Region::new(
        key,
        0,
        len,
        Box::new(move || {
            let f = VfsFile::open(Arc::clone(&vfs), &path)?;
            Ok(Box::new(std::io::BufReader::with_capacity(
                VfsFile::CHUNK,
                f,
            )))
        }),
    )))
}

/// A file read through the VFS, a chunk per lock, so the game's own reads
/// wait at most one chunk.
pub struct VfsFile {
    vfs: SharedVfs,
    handle: vfs::Handle,
    pos: u64,
    len: u64,
}

impl VfsFile {
    pub const CHUNK: usize = 256 * 1024;

    pub fn open(vfs: SharedVfs, path: &str) -> std::io::Result<Self> {
        let err = |e: vfs::Error| std::io::Error::other(format!("{path}: {e:?}"));
        let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
        let r = v.open(path, vfs::OpenOptions::default()).map_err(err)?;
        let len = match v.query_information(r.handle) {
            Ok(i) => i.metadata.end_of_file,
            Err(e) => {
                let _ = v.close(r.handle);
                return Err(err(e));
            }
        };
        drop(v);
        Ok(VfsFile {
            vfs,
            handle: r.handle,
            pos: 0,
            len,
        })
    }
}

impl Read for VfsFile {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.len.saturating_sub(self.pos);
        let n = buf
            .len()
            .min(Self::CHUNK)
            .min(usize::try_from(left).unwrap_or(usize::MAX));
        if n == 0 {
            return Ok(0);
        }
        let mut v = self.vfs.lock().unwrap_or_else(|e| e.into_inner());
        let got = v
            .read(self.handle, vfs::Offset::Absolute(self.pos), &mut buf[..n])
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        self.pos += got as u64;
        Ok(got)
    }
}

impl Seek for VfsFile {
    fn seek(&mut self, to: SeekFrom) -> std::io::Result<u64> {
        let at = match to {
            SeekFrom::Start(n) => Some(n),
            SeekFrom::End(d) => self.len.checked_add_signed(d),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = at.ok_or_else(|| std::io::Error::other("seek before the start"))?;
        Ok(self.pos)
    }
}

impl Drop for VfsFile {
    fn drop(&mut self) {
        let _ = self
            .vfs
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .close(self.handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("boot-bots-nav-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn storage(root: &Path) -> SharedVfs {
        let cfg = vfs::Config::new(root, crate::new_maps::HALO2_TITLE);
        Arc::new(Mutex::new(vfs::Vfs::with_storage(cfg).unwrap()))
    }

    /// A content package's map: a synthetic cache header and a tail.
    fn put_package_map(root: &Path, offer: &str, name: &str) -> Vec<u8> {
        let dir = content_root(root).join(offer);
        std::fs::create_dir_all(&dir).unwrap();
        let mut b = vec![0u8; autonav::MAP_HEADER_LEN + 4096];
        b[..4].copy_from_slice(b"daeh");
        b[0x140..0x144].copy_from_slice(&1u32.to_le_bytes());
        b[0x198..0x198 + name.len()].copy_from_slice(name.as_bytes());
        b[0x7fc..0x800].copy_from_slice(b"toof");
        for (i, x) in b[0x800..].iter_mut().enumerate() {
            *x = i as u8;
        }
        std::fs::write(dir.join(format!("{name}.map")), &b).unwrap();
        b
    }

    #[test]
    fn switched_off_nothing_starts() {
        let root = tmp("off");
        let h = start_with(
            false,
            &root,
            storage(&root),
            None,
        );
        assert!(h.is_none());
        assert!(!root.join("bots").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn only_the_requested_content_map_is_opened() {
        let root = tmp("lookup");
        let vfs = storage(&root);
        let content = content_root(&root);
        let bytes = put_package_map(&root, "4d53006400000401", "new_map");
        put_package_map(&root, "4d53006400000402", "other");
        put_package_map(&root, "4d53006400000402", "mainmenu");
        let mut src = lookup(&vfs, &content, "new_map", None).unwrap();
        assert_eq!(src.header().unwrap(), bytes[..autonav::MAP_HEADER_LEN]);
        let path = format!(r"{}\TDATA\{:08X}\$c\4d53006400000401\new_map.map",
            vfs::DATA_DEVICE, crate::new_maps::HALO2_TITLE);
        let mut alias = lookup(&vfs, &content, "different_stem", Some(&path)).unwrap();
        assert_eq!(alias.header().unwrap(), bytes[..autonav::MAP_HEADER_LEN]);
        assert!(lookup(&vfs, &content, "mainmenu", None).is_none());
        assert!(lookup(&vfs, &content, "missing", None).is_none());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_vfs_file_reads_and_seeks_in_chunks() {
        let root = tmp("read");
        let vfs = storage(&root);
        let bytes = put_package_map(&root, "4d53006400000401", "m");
        let path = format!(
            "{}\\TDATA\\{:08X}\\$c\\4d53006400000401\\m.map",
            vfs::DATA_DEVICE,
            crate::new_maps::HALO2_TITLE
        );
        let mut f = VfsFile::open(Arc::clone(&vfs), &path).unwrap();
        let mut all = Vec::new();
        f.read_to_end(&mut all).unwrap();
        assert_eq!(all, bytes);
        assert_eq!(f.seek(SeekFrom::End(-4)).unwrap(), bytes.len() as u64 - 4);
        let mut tail = [0u8; 8];
        assert_eq!(f.read(&mut tail).unwrap(), 4);
        assert_eq!(tail[..4], bytes[bytes.len() - 4..]);
        f.seek(SeekFrom::Start(0x198)).unwrap();
        let mut name = [0u8; 1];
        f.read_exact(&mut name).unwrap();
        assert_eq!(&name, b"m");
        assert!(f.seek(SeekFrom::Current(-0x1000)).is_err());
        drop(f);
        assert!(VfsFile::open(vfs, "\\Device\\Harddisk0\\Partition1\\nothing.map").is_err());
        std::fs::remove_dir_all(&root).unwrap();
    }
}
