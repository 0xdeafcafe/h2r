//! The bots' nav meshes, built while the game runs (docs/game/bots.md §10,
//! stage 2, "Built for you"): [`bots::autonav::Worker`] on a background
//! thread of its own ([`background::spawn`]), with nav probes and
//! visibility grid rows split across [`background::workers`] lowered
//! threads, so the boot and a map
//! load never wait for it, nor for the cores it uses.
//!
//! It scans retail maps as the VFS serves them, including explicit
//! `H2_MAP_DIR` editor overrides. Generated offers open on demand so the
//! background scan does not apply every installed patch at boot.
//!
//! - the disc's `maps\*.map`, opened as `D:\maps\<name>.map`;
//! - the installed Map Pack's maps, `T:\$c\<offer>\*.map`.
//!
//! The level being booted (`H2_LEVEL`) goes first. When the game loads a
//! map, `bot_glue` asks for it ([`bots::autonav::Handle::want`]): it is
//! scanned again (a map installed since boot is found) and looked at next.
//! If it was stale the bots walk straight until the build lands, then take
//! the new mesh without a reload ([`bots::autonav::offer`]).
//!
//! The scans never resolve a map package: a packaged map nothing has
//! opened yet is skipped ([`unresolved`]) until the game loads it, so the
//! worker never holds the VFS for a read of a whole base map.
//!
//! The main menu and the shared resource maps are never scanned or built
//! ([`bots::autonav::map_use`]).
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
    disc: &Path,
    vfs: SharedVfs,
    first: Option<String>,
    light: bool,
) -> Option<Handle> {
    let v = std::env::var(autonav::ENV).ok();
    start_with(
        autonav::enabled(v.as_deref()),
        data_root,
        disc,
        vfs,
        first,
        light,
    )
}

/// [`start`] with the switch given. `light`: the light-host profile,
/// which scans only the booted map — reading every map on the disc at
/// boot (its BSPs) would bring the whole image resident.
pub fn start_with(
    on: bool,
    data_root: &Path,
    disc: &Path,
    vfs: SharedVfs,
    first: Option<String>,
    light: bool,
) -> Option<Handle> {
    if !on {
        eprintln!(
            "bots: nav meshes are not built automatically ({}=0); `h2 bots nav all` builds them",
            autonav::ENV
        );
        return None;
    }
    let (disc, content) = (disc.to_path_buf(), content_root(data_root));
    let scan_vfs = Arc::clone(&vfs);
    let wanted = first.clone();
    let scan = if light {
        Box::new(move || scan_light(&scan_vfs, wanted.as_deref()))
            as Box<dyn FnMut() -> Vec<Box<dyn Source>> + Send>
    } else {
        Box::new(move || scan(&disc, &content, &scan_vfs))
            as Box<dyn FnMut() -> Vec<Box<dyn Source>> + Send>
    };
    let (worker, handle) = Worker::new(bots::cache::dir(data_root), scan);
    let worker = worker
        .shared_cache(autonav::shared_cache_dir())
        .on_demand(move |name| {
            if !played(name) {
                return None;
            }
            // Content packages first, then the disc's own map of the name:
            // a light host's scan holds only the booted map, so a later
            // `want` finds the rest here.
            let mut path = vfs
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .content_map_path(name);
            if path.is_none() {
                path = Some(format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE));
            }
            let path = path?;
            vfs_source(&vfs, name, &path)
        })
        .first(first)
        .stay(true)
        .background(true)
        .on_built(|name, d| {
            if autonav::offer(name, d.clone(), true) {
                eprintln!("bots: {name}: the new nav mesh is in use");
            }
        });
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

/// Whether a bot ever plays the map `name`: the scans never open the main
/// menu or the shared resource maps ([`autonav::map_use`]).
fn played(name: &str) -> bool {
    autonav::map_use(name, None) != autonav::MapUse::Never
}

/// Where content packages live on the host: `T:\$c`.
fn content_root(data_root: &Path) -> PathBuf {
    vfs::Config::new(data_root, crate::new_maps::HALO2_TITLE)
        .title_data_dir()
        .join(vfs::CONTENT_DIR)
}

/// The light host's scan: only the booted map, through the VFS's own path
/// — no walk of the image, no content enumeration, no other map's bytes.
/// Maps loaded later arrive through [`Handle::want`], which the worker
/// answers with the on-demand lookup.
pub fn scan_light(vfs: &SharedVfs, first: Option<&str>) -> Vec<Box<dyn Source>> {
    let Some(first) = first.filter(|f| played(f)) else {
        return Vec::new();
    };
    let name = first.to_ascii_lowercase();
    let path = format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE);
    if unresolved(vfs, &path) {
        return Vec::new();
    }
    vfs_source(vfs, &name, &path).into_iter().collect()
}

/// Disc and physically installed content. Generated package placeholders
/// are opened through the worker's on-demand lookup.
pub fn scan(disc: &Path, content: &Path, vfs: &SharedVfs) -> Vec<Box<dyn Source>> {
    let mut out = Vec::new();
    for name in disc_map_names(disc).into_iter().filter(|n| played(n)) {
        let path = format!("{}\\maps\\{name}.map", vfs::CDROM_DEVICE);
        if unresolved(vfs, &path) {
            continue;
        }
        out.extend(vfs_source(vfs, &name, &path));
    }
    let mut dirs: Vec<_> = std::fs::read_dir(content)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.path().is_dir())
        .collect();
    dirs.sort_by_key(|e| e.file_name());
    for d in dirs {
        let mut maps: Vec<_> = std::fs::read_dir(d.path())
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.metadata().is_ok_and(|m| m.is_file() && m.len() != 0))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.to_ascii_lowercase().ends_with(".map"))
            .collect();
        maps.sort();
        for file in maps {
            let key = &file[..file.len() - ".map".len()];
            if !played(key) {
                continue;
            }
            let path = format!(
                "{}\\TDATA\\{:08X}\\{}\\{}\\{file}",
                vfs::DATA_DEVICE,
                crate::new_maps::HALO2_TITLE,
                vfs::CONTENT_DIR,
                d.file_name().to_string_lossy()
            );
            if unresolved(vfs, &path) {
                continue;
            }
            out.extend(vfs_source(vfs, key, &path));
        }
    }
    out
}

/// Whether `path` is a packaged map nothing has looked at yet: its first
/// look reads the whole base map with the VFS held
/// ([`vfs::Vfs::resolves_on_use`]), which would stall the game's own reads.
/// The scans leave it to the game's first open; when the game loads it, the
/// glue's [`Handle::want`] scans again and finds it resolved.
fn unresolved(vfs: &SharedVfs, path: &str) -> bool {
    vfs.lock()
        .unwrap_or_else(|e| e.into_inner())
        .resolves_on_use(path)
}

/// The disc's `maps\*.map` names; none if the image does not read.
fn disc_map_names(disc: &Path) -> Vec<String> {
    let Ok(f) = std::fs::File::open(disc) else {
        return Vec::new();
    };
    let Ok(mut iso) = xiso::Xiso::open(std::io::BufReader::new(f)) else {
        return Vec::new();
    };
    let Ok(entries) = iso.entries() else {
        return Vec::new();
    };
    entries
        .iter()
        .filter(|e| {
            let p = e.path.to_ascii_lowercase();
            !e.is_dir
                && p.starts_with("/maps/")
                && p.ends_with(".map")
                && p.matches('/').count() == 2
        })
        .map(|e| {
            let n = e.path.rsplit('/').next().unwrap_or(&e.path);
            n[..n.len() - ".map".len()].to_owned()
        })
        .collect()
}

/// The map the VFS serves at `path`: its host region when it is one,
/// else read through the VFS. `None` if it does not open.
pub fn vfs_source(vfs: &SharedVfs, key: &str, path: &str) -> Option<Box<dyn Source>> {
    let mut v = vfs.lock().unwrap_or_else(|e| e.into_inner());
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
            &root.join("no.iso"),
            storage(&root),
            None,
            false,
        );
        assert!(h.is_none());
        assert!(!root.join("bots").exists());
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_scan_finds_content_maps_through_the_vfs() {
        let root = tmp("scan");
        let vfs = storage(&root);
        let bytes = put_package_map(&root, "4d53006400000401", "new_map");
        put_package_map(&root, "4d53006400000402", "other");
        std::fs::write(
            content_root(&root)
                .join("4d53006400000402")
                .join("patch.lvl"),
            b"x",
        )
        .unwrap();
        // No disc: the content maps only.
        let mut found = scan(&root.join("no.iso"), &content_root(&root), &vfs);
        let keys: Vec<&str> = found.iter().map(|s| s.key()).collect();
        assert_eq!(keys, ["new_map", "other"]);
        let h = found[0].header().unwrap();
        assert_eq!(h, bytes[..autonav::MAP_HEADER_LEN]);
        let parsed = autonav::MapHeader::parse(&h).unwrap();
        assert_eq!(
            (parsed.name.as_str(), parsed.multiplayer),
            ("new_map", true)
        );
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn the_scans_never_open_the_menu_or_shared_maps() {
        let root = tmp("ui");
        let vfs = storage(&root);
        put_package_map(&root, "4d53006400000401", "mainmenu");
        put_package_map(&root, "4d53006400000401", "shared");
        put_package_map(&root, "4d53006400000401", "lockout_remix");
        let found = scan(&root.join("no.iso"), &content_root(&root), &vfs);
        let keys: Vec<&str> = found.iter().map(|s| s.key()).collect();
        assert_eq!(keys, ["lockout_remix"]);
        assert!(scan_light(&vfs, Some("mainmenu")).is_empty());
        assert!(scan_light(&vfs, Some("single_player_shared")).is_empty());
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
