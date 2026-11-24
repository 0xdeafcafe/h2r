//! Maps served from the tag store (docs/engine/asset-store.md, AS2): an
//! invented store written by the extractor's own writer, over an invented
//! disc whose map files hold other bytes, so a byte from the disc shows.
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use tags::repo::TagRepo;
use tags::repo::skeleton::partition;
use vfs::*;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "h2-store-map-it-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An invented map's bytes, different for each `seed`.
fn level(seed: u8, len: usize) -> Vec<u8> {
    (0..len)
        .map(|i| (i.wrapping_mul(37) as u8) ^ (i >> 9) as u8 ^ seed)
        .collect()
}

/// Store `bytes` as the map `name`: a header and two BSP regions in the
/// skeleton, three local raw resources, the rest unnamed. Returns the
/// extent borders.
fn store(root: &std::path::Path, name: &str, bytes: &[u8]) -> Vec<u64> {
    let mut repo = TagRepo::create(root).unwrap();
    let len = bytes.len() as u64;
    let structural = [
        (0, 0x800),
        (0x2000, 0x600),
        (0x3000, 0x500),
        (len - 0x400, 0x400),
    ];
    let mut named = Vec::new();
    for (o, l) in [(0x800u64, 0x700u64), (0x1000, 0x10), (0x4000, 0x1801)] {
        let h = repo.put_raw(&bytes[o as usize..(o + l) as usize]).unwrap();
        named.push((o, l, h));
    }
    let parts = partition(len, &structural, &named);
    let file = tags::repo::hex(&tags::repo::sha256(bytes));
    let mut read = |o: u64, l: u64| -> tags::repo::Result<Vec<u8>> {
        Ok(bytes[o as usize..(o + l) as usize].to_vec())
    };
    repo.put_skeleton(name, &file, len, &parts, &mut read)
        .unwrap();
    parts.iter().map(|p| p.range().0).chain([len]).collect()
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

/// A disc of `files` (path, bytes); `maps` is a folder when any file is in it.
struct Disc(Vec<(String, Vec<u8>)>);
impl Backend for Disc {
    fn read_only(&self) -> bool {
        true
    }
    fn stat(&mut self, path: &str) -> Result<Metadata> {
        if path.eq_ignore_ascii_case("maps") {
            return self
                .0
                .iter()
                .any(|f| f.0.starts_with("maps\\"))
                .then(|| meta(0, true))
                .ok_or(Error::ObjectNameNotFound);
        }
        self.0
            .iter()
            .find(|f| f.0.eq_ignore_ascii_case(path))
            .map(|f| meta(f.1.len() as u64, false))
            .ok_or(Error::ObjectNameNotFound)
    }
    fn read_directory(&mut self, path: &str) -> Result<Vec<DirectoryEntry>> {
        let out: Vec<DirectoryEntry> = self
            .0
            .iter()
            .filter_map(|(p, d)| {
                let (dir, name) = p.rsplit_once('\\')?;
                dir.eq_ignore_ascii_case(path).then(|| DirectoryEntry {
                    name: name.to_owned(),
                    metadata: meta(d.len() as u64, false),
                })
            })
            .collect();
        if out.is_empty() {
            return Err(Error::ObjectNameNotFound);
        }
        Ok(out)
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

const LEN: usize = 0x6c37;

fn disc() -> Disc {
    Disc(vec![
        ("maps\\level.map".into(), level(0xff, LEN)),
        ("maps\\other.map".into(), level(0xee, 0x1000)),
        ("default.xbe".into(), b"xbe!".to_vec()),
    ])
}

/// Reads across extent borders equal the map's bytes at every offset
/// class; the length is the map's; other files are the disc's.
#[test]
fn reads_across_extent_borders_equal_the_map() {
    let dir = TempDir::new();
    let bytes = level(1, LEN);
    let borders = store(&dir.0, "level", &bytes);
    let mut b = StoreMapBackend::new(disc(), &dir.0);
    assert_eq!(b.stat("maps\\level.map").unwrap().end_of_file, LEN as u64);
    // Case is the guest's.
    assert_eq!(b.stat("maps\\LEVEL.map").unwrap().end_of_file, LEN as u64);
    for border in borders {
        for start in [border.saturating_sub(5), border, border + 1] {
            for n in [1usize, 9, 0x800, 0x2001] {
                let mut buf = vec![0u8; n];
                let got = b.read_at("maps\\level.map", start, &mut buf).unwrap();
                let want = LEN.saturating_sub(start as usize).min(n);
                assert_eq!(got, want, "at {start:#x} +{n}");
                let s = (start as usize).min(LEN);
                assert_eq!(&buf[..got], &bytes[s..s + got], "at {start:#x} +{n}");
            }
        }
    }
    let mut whole = vec![0u8; LEN];
    assert_eq!(b.read_at("maps\\level.map", 0, &mut whole).unwrap(), LEN);
    assert_eq!(whole, bytes);
    // Not one host region; no recorded ranges; the retail identity.
    assert!(b.host_file("maps\\level.map").is_none());
    assert!(b.map_ranges("maps\\level.map").is_none());
    assert!(b.map_id("maps\\level.map").is_none());
    assert!(b.film_source("maps\\level.map").unwrap().is_none());
    // Everything else is the disc's.
    let mut x = [0u8; 4];
    assert_eq!(b.read_at("default.xbe", 0, &mut x).unwrap(), 4);
    assert_eq!(&x, b"xbe!");
}

/// A level the store lacks is a named error, never the disc's file.
#[test]
fn a_store_lacking_a_level_never_falls_to_the_disc() {
    let dir = TempDir::new();
    store(&dir.0, "level", &level(1, LEN));
    let mut b = StoreMapBackend::new(disc(), &dir.0);
    assert_eq!(b.stat("maps\\other.map"), Err(Error::ObjectNameNotFound));
    assert_eq!(
        b.read_at("maps\\other.map", 0, &mut [0u8; 4]),
        Err(Error::ObjectNameNotFound)
    );
    assert!(b.host_file("maps\\other.map").is_none());
    // The listing has the store's maps only, at their lengths.
    let names: Vec<(String, u64)> = b
        .read_directory("maps")
        .unwrap()
        .into_iter()
        .map(|e| (e.name, e.metadata.end_of_file))
        .collect();
    assert_eq!(names, [("level.map".to_owned(), LEN as u64)]);
}

/// A raw file the extent table names that is gone fails the read; the disc
/// is not read in its place.
#[test]
fn a_missing_raw_file_fails_the_read() {
    let dir = TempDir::new();
    let bytes = level(1, LEN);
    store(&dir.0, "level", &bytes);
    let h = tags::repo::hex(&tags::repo::sha256(&bytes[0x4000..0x4000 + 0x1801]));
    std::fs::remove_file(dir.0.join("raw").join(&h[..2]).join(&h)).unwrap();
    let mut b = StoreMapBackend::new(disc(), &dir.0);
    let mut buf = [0u8; 16];
    assert_eq!(b.read_at("maps\\level.map", 0, &mut buf).unwrap(), 16);
    assert_eq!(
        b.read_at("maps\\level.map", 0x4100, &mut buf),
        Err(Error::FileCorrupt)
    );
}

/// With the disc's maps hidden (no `maps` folder on the disc), the store's
/// maps are the folder.
#[test]
fn the_store_serves_maps_the_disc_does_not_have() {
    let dir = TempDir::new();
    let bytes = level(3, LEN);
    store(&dir.0, "level", &bytes);
    let bare = Disc(vec![("default.xbe".into(), b"xbe!".to_vec())]);
    let mut b = StoreMapBackend::new(bare, &dir.0);
    assert!(b.stat("maps").unwrap().is_directory());
    let list = b.read_directory("maps").unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "level.map");
    assert_eq!(b.stat("maps\\level.map").unwrap().end_of_file, LEN as u64);
    let mut whole = vec![0u8; LEN];
    assert_eq!(b.read_at("maps\\level.map", 0, &mut whole).unwrap(), LEN);
    assert_eq!(whole, bytes);
}

/// A changed extent table is a changed map: `map_changed` fires once, and
/// the new bytes are served from the next open with no handle on the old.
#[test]
fn map_changed_fires_when_a_levels_extents_change() {
    let dir = TempDir::new();
    let first = level(1, LEN);
    store(&dir.0, "level", &first);
    let mut b = StoreMapBackend::new(disc(), &dir.0);
    let path = "maps\\level.map";
    b.opened(path);
    let mut buf = [0u8; 64];
    b.read_at(path, 0x900, &mut buf).unwrap();
    assert_eq!(&buf[..], &first[0x900..0x940]);
    assert!(!b.map_changed(path));

    let second = level(2, LEN + 0x10);
    store(&dir.0, "level", &second);
    assert!(b.map_changed(path), "the extents moved");
    // An open handle keeps the bytes it opened.
    b.read_at(path, 0x900, &mut buf).unwrap();
    assert_eq!(&buf[..], &first[0x900..0x940]);
    b.closed(path);
    b.opened(path);
    b.read_at(path, 0x900, &mut buf).unwrap();
    assert_eq!(&buf[..], &second[0x900..0x940]);
    assert_eq!(b.stat(path).unwrap().end_of_file, (LEN + 0x10) as u64);
    assert!(!b.map_changed(path), "reported once");
    b.closed(path);

    // With nothing open, the change is taken at once.
    let third = level(3, LEN);
    store(&dir.0, "level", &third);
    assert!(b.map_changed(path));
    assert!(!b.map_changed(path));
    b.read_at(path, 0x900, &mut buf).unwrap();
    assert_eq!(&buf[..], &third[0x900..0x940]);
}
