//! A read-only backend that adds campaign maps the disc doesn't have
//! (docs/halo1/campaign-picker.md §3).
//!
//! 1.0 opens a campaign level's map as `d:\maps\<basename of its scenario
//! path>.map` (`0x214db0`). A second campaign's converted maps live in
//! `<data>/campaigns/<key>/<map>.map`; [`CampaignMapsBackend`] serves each
//! named one as `maps\<map>.map` when the inner backend has no such file.
//! A name the disc has is never shadowed: the disc (and the packages over
//! it) win. Metadata is the disc's `maps\mainmenu.map`'s, resized, so times
//! and attributes look like the disc's. Files are opened on first use and
//! kept while they are the ones to serve; nothing is written.
//!
//! **Assets.** A map may have a second file, its level with Halo 2's
//! assets (docs/halo1/campaign.md §5), served instead while the
//! [`SandboxChoice`] given to [`CampaignMapsBackend::with_assets`] says
//! Halo 2. Without that file the Halo 1 one is served, with one line on
//! stderr. The choice applies at a map's next open: an open file keeps
//! what it was opened with, and [`Backend::map_changed`] says when the map
//! would now come from the other file, so the game's cached copy is
//! dropped (docs/runtime/cache-slots.md).
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::PathBuf,
};

use crate::map_override::{is_map_path, override_id, resize};
use crate::{Backend, DirectoryEntry, Error, HostFile, Metadata, Result, SandboxChoice, VolumeInfo};

/// The disc map whose metadata ours borrow.
const TEMPLATE: &str = "maps\\mainmenu.map";

struct Open {
    file: File,
    len: u64,
    id: Option<content::MapId>,
    /// The host file this is.
    host: PathBuf,
}

pub struct CampaignMapsBackend<B> {
    inner: B,
    /// By lower-case map name: its host file.
    maps: BTreeMap<String, PathBuf>,
    open: BTreeMap<String, Option<Open>>,
    /// By lower-case map name: its Halo 2-assets file, served while
    /// `assets` says Halo 2 and the file is there.
    alternates: BTreeMap<String, PathBuf>,
    assets: SandboxChoice,
    /// Handles open per lower-case map name.
    handles: BTreeMap<String, usize>,
    /// The host file each map was last opened from.
    last_opened: BTreeMap<String, PathBuf>,
    /// Maps whose missing Halo 2-assets file was already reported.
    warned: BTreeSet<String>,
}

impl<B: Backend> CampaignMapsBackend<B> {
    /// Serve `maps` (map name, host file) over `inner`.
    pub fn new(inner: B, maps: impl IntoIterator<Item = (String, PathBuf)>) -> Self {
        Self {
            inner,
            maps: maps
                .into_iter()
                .map(|(n, p)| (n.to_ascii_lowercase(), p))
                .collect(),
            open: BTreeMap::new(),
            alternates: BTreeMap::new(),
            assets: SandboxChoice::default(),
            handles: BTreeMap::new(),
            last_opened: BTreeMap::new(),
            warned: BTreeSet::new(),
        }
    }

    /// Serve `alternates` (map name, host file of its Halo 2-assets level)
    /// in place of the maps' own files while `assets` says Halo 2.
    pub fn with_assets(
        mut self,
        assets: SandboxChoice,
        alternates: impl IntoIterator<Item = (String, PathBuf)>,
    ) -> Self {
        self.assets = assets;
        self.alternates = alternates
            .into_iter()
            .map(|(n, p)| (n.to_ascii_lowercase(), p))
            .collect();
        self
    }

    /// The host file `stem` should be served from at its next open.
    fn wanted(&mut self, stem: &str) -> Option<PathBuf> {
        let own = self.maps.get(stem)?.clone();
        if self.assets.halo1() {
            return Some(own);
        }
        match self.alternates.get(stem) {
            Some(alt) if alt.is_file() => Some(alt.clone()),
            alt => {
                if self.warned.insert(stem.to_owned()) {
                    let at = alt.map_or_else(|| "none".to_owned(), |p| p.display().to_string());
                    eprintln!(
                        "campaigns: {stem}: no Halo 2 assets level ({at}); serving its Halo 1 map"
                    );
                }
                Some(own)
            }
        }
    }

    pub fn into_inner(self) -> B {
        self.inner
    }

    /// Our map for `path` (its lower-case name and open file), when it is
    /// one and the inner backend lacks it.
    fn ours(&mut self, path: &str) -> Option<(String, &mut Open)> {
        let name = is_map_path(path)?;
        let stem = name[..name.len() - 4].to_ascii_lowercase();
        self.maps.get(&stem)?;
        // An open map keeps its file; otherwise a changed choice reopens.
        if self.handles.get(&stem).copied().unwrap_or(0) == 0
            && let Some(Some(o)) = self.open.get(&stem)
        {
            let had = o.host.clone();
            if self.wanted(&stem)? != had {
                self.open.remove(&stem);
            }
        }
        if !self.open.contains_key(&stem) {
            let host = self.wanted(&stem)?;
            let chosen = if self.inner.stat(path).is_ok() {
                None
            } else {
                File::open(&host).ok().and_then(|file| {
                    let len = file.metadata().ok().filter(|m| m.is_file())?.len();
                    (len <= u64::from(u32::MAX)).then_some(Open {
                        file,
                        len,
                        id: None,
                        host,
                    })
                })
            };
            self.open.insert(stem.clone(), chosen);
        }
        let o = self.open.get_mut(&stem)?.as_mut()?;
        Some((stem, o))
    }

    fn meta(&mut self, len: u64) -> Result<Metadata> {
        let mut m = self.inner.stat(TEMPLATE)?;
        resize(&mut m, len);
        Ok(m)
    }
}

impl<B: Backend> Backend for CampaignMapsBackend<B> {
    fn opened(&mut self, path: &str) {
        if let Some((stem, o)) = self.ours(path) {
            let host = o.host.clone();
            *self.handles.entry(stem.clone()).or_insert(0) += 1;
            self.last_opened.insert(stem, host);
        }
        self.inner.opened(path)
    }
    fn closed(&mut self, path: &str) {
        if let Some(stem) = is_map_path(path).map(|n| n[..n.len() - 4].to_ascii_lowercase())
            && let Some(n) = self.handles.get_mut(&stem)
        {
            *n = n.saturating_sub(1);
            if *n == 0 {
                self.handles.remove(&stem);
            }
        }
        self.inner.closed(path)
    }
    fn rescan(&mut self) {
        self.inner.rescan()
    }
    fn map_changed(&mut self, path: &str) -> bool {
        let Some(stem) = is_map_path(path).map(|n| n[..n.len() - 4].to_ascii_lowercase()) else {
            return self.inner.map_changed(path);
        };
        if !self.maps.contains_key(&stem) || self.inner.stat(path).is_ok() {
            return self.inner.map_changed(path);
        }
        if self.handles.contains_key(&stem) {
            return false;
        }
        let Some(was) = self.last_opened.get(&stem).cloned() else {
            return false;
        };
        self.wanted(&stem).is_some_and(|want| want != was)
    }
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        match self.ours(path).map(|(_, o)| o.len) {
            Some(len) => self.meta(len),
            None => self.inner.stat(path),
        }
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let mut entries = self.inner.read_directory(path)?;
        if !path.eq_ignore_ascii_case("maps") {
            return Ok(entries);
        }
        let names: Vec<String> = self.maps.keys().cloned().collect();
        for n in names {
            let file = format!("{n}.map");
            if entries.iter().any(|e| e.name.eq_ignore_ascii_case(&file)) {
                continue;
            }
            if let Some(len) = self.ours(&format!("maps\\{file}")).map(|(_, o)| o.len) {
                entries.push(DirectoryEntry {
                    name: file,
                    metadata: self.meta(len)?,
                });
            }
        }
        Ok(entries)
    }
    fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
        let Some((_, o)) = self.ours(path) else {
            return self.inner.read_at(path, offset, buffer);
        };
        let n = o.len.saturating_sub(offset).min(buffer.len() as u64) as usize;
        if n == 0 {
            return Ok(0);
        }
        o.file.seek(SeekFrom::Start(offset))?;
        match o.file.read_exact(&mut buffer[..n]) {
            Ok(()) => Ok(n),
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Err(Error::FileCorrupt),
            Err(e) => Err(e.into()),
        }
    }
    fn volume_info(&mut self) -> Result<VolumeInfo> {
        self.inner.volume_info()
    }
    fn map_ranges(&mut self, path: &str) -> Option<Vec<(u64, u64)>> {
        if self.ours(path).is_some() {
            None
        } else {
            self.inner.map_ranges(path)
        }
    }
    /// A campaign map is a user map, identified by its whole file.
    fn map_id(&mut self, path: &str) -> Option<content::MapId> {
        let host = {
            let (stem, o) = self.ours(path)?;
            if let Some(id) = o.id {
                return Some(id);
            }
            let file = o.host.clone();
            (stem, file)
        };
        let id = override_id(&host.0, &host.1).ok()?;
        if let Some((_, o)) = self.ours(path) {
            o.id = Some(id);
        }
        Some(id)
    }
    fn host_file(&mut self, path: &str) -> Option<HostFile> {
        let Some((_, o)) = self.ours(path) else {
            return self.inner.host_file(path);
        };
        let len = o.len;
        Some(HostFile {
            path: o.host.clone(),
            offset: 0,
            len,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ATTR_DIRECTORY, ATTR_READONLY, BasicInfo};

    /// A disc holding `files` (path, bytes), with a `maps` folder.
    struct Fake(Vec<(String, Vec<u8>)>);
    impl Fake {
        fn with(files: &[(&str, &[u8])]) -> Self {
            Fake(
                files
                    .iter()
                    .map(|(p, b)| (p.to_string(), b.to_vec()))
                    .collect(),
            )
        }
    }
    fn meta(len: u64, dir: bool) -> Metadata {
        Metadata {
            basic: BasicInfo {
                attributes: ATTR_READONLY | if dir { ATTR_DIRECTORY } else { 0 },
                ..Default::default()
            },
            end_of_file: len,
            allocation_size: len.div_ceil(2048) * 2048,
        }
    }
    impl Backend for Fake {
        fn read_only(&self) -> bool {
            true
        }
        fn stat(&mut self, path: &str) -> Result<Metadata> {
            if path.eq_ignore_ascii_case("maps") {
                return Ok(meta(0, true));
            }
            self.0
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .map(|f| meta(f.1.len() as u64, false))
                .ok_or(Error::ObjectNameNotFound)
        }
        fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
            Ok(self
                .0
                .iter()
                .filter_map(|(p, d)| {
                    let (dir, name) = p.rsplit_once('\\')?;
                    dir.eq_ignore_ascii_case(path).then(|| DirectoryEntry {
                        name: name.to_owned(),
                        metadata: meta(d.len() as u64, false),
                    })
                })
                .collect())
        }
        fn read_at(&mut self, path: &str, offset: u64, buffer: &mut [u8]) -> Result<usize> {
            let f = self
                .0
                .iter()
                .find(|f| f.0.eq_ignore_ascii_case(path))
                .ok_or(Error::ObjectNameNotFound)?;
            let start = (offset as usize).min(f.1.len());
            let n = (f.1.len() - start).min(buffer.len());
            buffer[..n].copy_from_slice(&f.1[start..start + n]);
            Ok(n)
        }
        fn volume_info(&mut self) -> Result<VolumeInfo> {
            Err(Error::NotSupported)
        }
    }

    fn dir(tag: &str) -> PathBuf {
        let d =
            std::env::temp_dir().join(format!("h2-vfs-campaign-maps-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn a_map_the_disc_lacks_is_served_from_its_campaign_folder() {
        let d = dir("serve");
        std::fs::write(d.join("a10.map"), b"halo one level").unwrap();
        std::fs::write(d.join("lockout.map"), b"not served").unwrap();
        let mut b = CampaignMapsBackend::new(
            Fake::with(&[
                ("maps\\mainmenu.map", b"menu"),
                ("maps\\lockout.map", b"disc"),
            ]),
            [
                ("A10".to_string(), d.join("a10.map")),
                ("lockout".to_string(), d.join("lockout.map")),
            ],
        );
        assert_eq!(b.stat("maps\\a10.map").unwrap().end_of_file, 14);
        assert_eq!(b.stat("maps\\A10.MAP").unwrap().allocation_size, 2048);
        let mut buf = [0u8; 32];
        assert_eq!(b.read_at("maps\\a10.map", 5, &mut buf).unwrap(), 9);
        assert_eq!(&buf[..9], b"one level");
        assert_eq!(b.read_at("maps\\a10.map", 14, &mut buf).unwrap(), 0);
        // The disc's own map is never shadowed.
        assert_eq!(b.read_at("maps\\lockout.map", 0, &mut buf).unwrap(), 4);
        assert_eq!(&buf[..4], b"disc");
        // Listed once each.
        let names: Vec<String> = b
            .read_directory("maps")
            .unwrap()
            .into_iter()
            .map(|e| e.name)
            .collect();
        assert_eq!(
            names
                .iter()
                .filter(|n| n.eq_ignore_ascii_case("a10.map"))
                .count(),
            1
        );
        assert_eq!(
            names
                .iter()
                .filter(|n| n.eq_ignore_ascii_case("lockout.map"))
                .count(),
            1
        );
        assert_eq!(b.host_file("maps\\a10.map").unwrap().len, 14);
        assert!(b.map_id("maps\\a10.map").is_some());
        assert!(b.map_id("maps\\a10.map") == b.map_id("maps\\A10.map"));
        // Anything else is the inner backend's.
        assert!(b.stat("maps\\a30.map").is_err());
        assert!(b.stat("maps\\sub\\a10.map").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    fn read(b: &mut CampaignMapsBackend<Fake>, path: &str) -> Vec<u8> {
        let len = b.stat(path).unwrap().end_of_file as usize;
        let mut buf = vec![0u8; len];
        assert_eq!(b.read_at(path, 0, &mut buf).unwrap(), len);
        buf
    }

    /// The assets choice picks the file at the next open; an open map keeps
    /// its file, and the change is reported once it is closed.
    #[test]
    fn the_assets_choice_picks_the_level_at_its_next_open() {
        let d = dir("assets");
        std::fs::create_dir_all(d.join("h2")).unwrap();
        std::fs::write(d.join("a30.map"), b"halo one assets").unwrap();
        std::fs::write(d.join("h2").join("a30.map"), b"halo two").unwrap();
        let choice = SandboxChoice::default();
        let mut b = CampaignMapsBackend::new(
            Fake::with(&[("maps\\mainmenu.map", b"menu")]),
            [("a30".to_string(), d.join("a30.map"))],
        )
        .with_assets(choice.clone(), [("a30".to_string(), d.join("h2").join("a30.map"))]);
        let p = "maps\\a30.map";
        assert_eq!(read(&mut b, p), b"halo one assets");
        let h1_id = b.map_id(p);
        b.opened(p);
        choice.set_halo1(false);
        assert_eq!(read(&mut b, p), b"halo one assets", "open: keeps its file");
        assert!(!b.map_changed(p), "not while open");
        b.closed(p);
        assert!(b.map_changed(p), "opened as Halo 1, Halo 2 now");
        assert_eq!(read(&mut b, p), b"halo two");
        assert_eq!(b.host_file(p).unwrap().path, d.join("h2").join("a30.map"));
        assert_ne!(b.map_id(p), h1_id, "each file its own id");
        b.opened(p);
        b.closed(p);
        assert!(!b.map_changed(p), "opened as it is served");
        choice.set_halo1(true);
        assert!(b.map_changed(p));
        assert_eq!(read(&mut b, p), b"halo one assets");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Halo 2 assets asked for a level that has no such file: its Halo 1
    /// map, and nothing to drop.
    #[test]
    fn without_a_halo2_assets_level_the_halo1_one_is_served() {
        let d = dir("fallback");
        std::fs::write(d.join("a10.map"), b"pillar").unwrap();
        let choice = SandboxChoice::default();
        choice.set_halo1(false);
        let mut b = CampaignMapsBackend::new(
            Fake::with(&[("maps\\mainmenu.map", b"menu")]),
            [("a10".to_string(), d.join("a10.map"))],
        )
        .with_assets(choice.clone(), [("a10".to_string(), d.join("h2").join("a10.map"))]);
        let p = "maps\\a10.map";
        b.opened(p);
        b.closed(p);
        assert_eq!(read(&mut b, p), b"pillar");
        assert!(!b.map_changed(p));
        choice.set_halo1(true);
        assert!(!b.map_changed(p), "the same file either way");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_host_file_stays_missing() {
        let d = dir("missing");
        let mut b = CampaignMapsBackend::new(
            Fake::with(&[("maps\\mainmenu.map", b"menu")]),
            [("a30".to_string(), d.join("a30.map"))],
        );
        assert!(b.stat("maps\\a30.map").is_err());
        assert!(
            !b.read_directory("maps")
                .unwrap()
                .iter()
                .any(|e| e.name == "a30.map")
        );
        let _ = std::fs::remove_dir_all(&d);
    }
}
