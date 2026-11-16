//! Map packages through the whole VFS: a synthetic XISO as the DVD, packages
//! under `<data>/maps`, and the game's view through `D:` (docs/maps.md).
use std::sync::{Arc, Mutex};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use vfs::*;

use content::{Manifest, Sha256, manifest::Layer};

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "h2-packages-it-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("data").join("maps")).unwrap();
        fs::create_dir_all(path.join("override")).unwrap();
        Self(path)
    }
    fn overrides(&self) -> PathBuf {
        self.0.join("override")
    }
    fn maps(&self) -> PathBuf {
        self.0.join("data").join("maps")
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The retail copy-time checksum (docs/cache-slots.md): XOR of each whole
/// little-endian qword after the header, folded to 32 bits with the parity
/// seed; a final partial qword is ignored. Written out here as an
/// independent oracle.
fn checksum(payload: &[u8]) -> u32 {
    payload.as_chunks::<8>().0.iter().fold(0, |c, q| {
        c ^ u32::from_le_bytes(q[..4].try_into().unwrap())
            ^ u32::from_le_bytes(q[4..].try_into().unwrap())
            ^ 0x8844_2211
    })
}
/// An invented cache file: a 0x800 header with its checksum at +0x2d0 (and
/// the length at +8), then `payload`.
fn cache_file(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 0x800];
    bytes[..4].copy_from_slice(b"daeh");
    bytes[8..12].copy_from_slice(&(0x800 + payload.len() as u32).to_le_bytes());
    bytes[0x2d0..0x2d4].copy_from_slice(&checksum(payload).to_le_bytes());
    bytes[0x7fc..].copy_from_slice(b"toof");
    bytes.extend_from_slice(payload);
    bytes
}

fn dirent(right: u16, sector: u32, size: u32, directory: bool, name: &str) -> Vec<u8> {
    let mut bytes = vec![0; 14];
    bytes[2..4].copy_from_slice(&right.to_le_bytes());
    bytes[4..8].copy_from_slice(&sector.to_le_bytes());
    bytes[8..12].copy_from_slice(&size.to_le_bytes());
    bytes[12] = if directory { 0x10 } else { 0 };
    bytes[13] = name.len() as u8;
    bytes.extend_from_slice(name.as_bytes());
    bytes.resize(bytes.len().next_multiple_of(4), 0);
    bytes
}
/// Records chained through their right links, as one directory.
fn directory(records: &[(u32, u32, bool, &str)]) -> Vec<u8> {
    let mut out = Vec::new();
    for (i, &(sector, size, dir, name)) in records.iter().enumerate() {
        let len = dirent(0, sector, size, dir, name).len();
        let right = if i + 1 < records.len() {
            ((out.len() + len) / 4) as u16
        } else {
            0
        };
        out.extend(dirent(right, sector, size, dir, name));
    }
    out
}

const DISC_MENU: &[u8] = &[0x11; 0x1000];
const DISC_LEVEL: &[u8] = &[0x22; 0x900];

/// A disc with `default.xbe`, `maps\mainmenu.map` and `maps\Ascension.map`.
fn image() -> Vec<u8> {
    let menu = cache_file(DISC_MENU);
    let level = cache_file(DISC_LEVEL);
    let mut image = vec![0; 44 * 2048];
    image[32 * 2048..32 * 2048 + 20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
    image[32 * 2048 + 20..32 * 2048 + 24].copy_from_slice(&33u32.to_le_bytes());
    let root = directory(&[(34, 6, false, "default.xbe"), (35, 2048, true, "maps")]);
    image[32 * 2048 + 24..32 * 2048 + 28].copy_from_slice(&(root.len() as u32).to_le_bytes());
    image[33 * 2048..][..root.len()].copy_from_slice(&root);
    image[34 * 2048..][..6].copy_from_slice(b"header");
    let maps = directory(&[
        (36, menu.len() as u32, false, "mainmenu.map"),
        (40, level.len() as u32, false, "Ascension.map"),
    ]);
    image[35 * 2048..][..maps.len()].copy_from_slice(&maps);
    image[36 * 2048..][..menu.len()].copy_from_slice(&menu);
    image[40 * 2048..][..level.len()].copy_from_slice(&level);
    image
}

fn config(t: &TempDir) -> Config {
    Config::new(t.0.join("data"), 0x4D530064)
}
fn linked(mut vfs: Vfs, config: &Config) -> Vfs {
    vfs.create_standard_links(config).unwrap();
    vfs
}
fn disc(t: &TempDir) -> PathBuf {
    let iso = t.0.join("disc.iso");
    fs::write(&iso, image()).unwrap();
    iso
}
type Notices = Arc<Mutex<Vec<String>>>;
/// The disc wrapped in packages from `<data>/maps`, and the notices.
fn with_packages(t: &TempDir, iso: &Path) -> (Vfs, Notices) {
    let cfg = config(t);
    let notices = Notices::default();
    let sink = Arc::clone(&notices);
    let backend = PackageBackend::with_notice(
        DiscBackend::open(iso).unwrap(),
        content::Library::open(t.maps()),
        Box::new(move |l: &str| sink.lock().unwrap().push(l.to_owned())),
    );
    let vfs = linked(Vfs::with_disc_backend(cfg.clone(), backend).unwrap(), &cfg);
    (vfs, notices)
}
fn read_file(vfs: &mut Vfs, name: &str) -> Vec<u8> {
    let h = vfs.open(name, OpenOptions::default()).unwrap().handle;
    let len = vfs.query_information(h).unwrap().metadata.end_of_file;
    let mut out = vec![0; len as usize];
    let mut at = 0;
    while at < out.len() {
        let end = (at + 777).min(out.len());
        let n = vfs.read(h, Offset::Current, &mut out[at..end]).unwrap();
        assert_ne!(n, 0);
        at += n;
    }
    assert_eq!(
        vfs.read(h, Offset::Current, &mut [0; 4]),
        Err(Error::EndOfFile)
    );
    vfs.close(h).unwrap();
    out
}
fn listing(vfs: &mut Vfs, dir: &str) -> Vec<DirectoryEntry> {
    let h = vfs
        .open(
            dir,
            OpenOptions {
                directory: Some(true),
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    let mut out = Vec::new();
    loop {
        match vfs.query_directory(h, None, false) {
            Ok(e) => out.push(e),
            Err(Error::NoMoreFiles) => break,
            Err(e) => panic!("{e:?}"),
        }
    }
    vfs.close(h).unwrap();
    out
}

/// Everything the guest and host tools can see of the disc.
fn observe(vfs: &mut Vfs) -> Vec<String> {
    let mut seen = Vec::new();
    for dir in ["D:\\", "D:\\maps", "D:\\MAPS"] {
        seen.push(format!("{dir}: {:?}", listing(vfs, dir)));
    }
    for name in [
        "D:\\default.xbe",
        "D:\\maps\\mainmenu.map",
        "D:\\MAPS\\ASCENSION.MAP",
        "D:\\maps\\ascension.map",
    ] {
        seen.push(format!("{name}: {:?}", vfs.query_path(name)));
        seen.push(format!("{name}: {:?}", read_file(vfs, name)));
        seen.push(format!("{name}: {:?}", vfs.host_file(name)));
    }
    for name in [
        "D:\\maps\\missing.map",
        "D:\\maps\\new.map",
        "D:\\nope\\a.map",
    ] {
        seen.push(format!("{name}: {:?}", vfs.query_path(name)));
        seen.push(format!(
            "{name}: {:?}",
            vfs.open(name, OpenOptions::default()).err()
        ));
    }
    let h = vfs.open("D:\\", OpenOptions::default()).unwrap().handle;
    seen.push(format!("{:?}", vfs.query_volume_information(h)));
    vfs.close(h).unwrap();
    seen
}

/// The disc's Ascension.map, as the package's base.
fn ascension() -> Vec<u8> {
    cache_file(DISC_LEVEL)
}

/// Write a package for `map` over `base` with `files` as its layers.
fn package(t: &TempDir, dir: &str, map: &str, base: &[u8], files: &[(&str, &[u8])]) -> Manifest {
    let d = t.maps().join(dir);
    fs::create_dir_all(&d).unwrap();
    let mut m = Manifest::retail(map, Sha256::of(base));
    m.package.name = dir.into();
    for (name, bytes) in files {
        fs::write(d.join(name), bytes).unwrap();
        m.patches.push(Layer {
            file: (*name).into(),
            sha256: Sha256::of(bytes),
        });
    }
    fs::write(d.join("manifest.toml"), m.to_toml()).unwrap();
    m
}

/// An IPS patch of plain records.
fn ips(records: &[(u32, &[u8])]) -> Vec<u8> {
    let mut p = b"PATCH".to_vec();
    for (o, d) in records {
        p.extend(&o.to_be_bytes()[1..]);
        p.extend((d.len() as u16).to_be_bytes());
        p.extend(*d);
    }
    p.extend(b"EOF");
    p
}

/// A PPF1 patch of `records`.
fn ppf1(records: &[(u32, &[u8])]) -> Vec<u8> {
    let mut p = b"PPF10\0".to_vec();
    p.extend([b' '; 50]);
    for (o, d) in records {
        p.extend(o.to_le_bytes());
        p.push(d.len() as u8);
        p.extend(*d);
    }
    p
}

fn bps_varint(out: &mut Vec<u8>, mut data: u64) {
    loop {
        let x = (data & 0x7f) as u8;
        data >>= 7;
        if data == 0 {
            out.push(0x80 | x);
            return;
        }
        out.push(x);
        data -= 1;
    }
}
/// A BPS patch: the whole source, then `extra` appended.
fn bps_append(source: &[u8], extra: &[u8]) -> (Vec<u8>, Vec<u8>) {
    let mut target = source.to_vec();
    target.extend(extra);
    let mut p = b"BPS1".to_vec();
    bps_varint(&mut p, source.len() as u64);
    bps_varint(&mut p, target.len() as u64);
    bps_varint(&mut p, 0);
    bps_varint(&mut p, (source.len() as u64 - 1) << 2);
    bps_varint(&mut p, ((extra.len() as u64 - 1) << 2) | 1);
    p.extend(extra);
    p.extend(content::patch::crc32(source).to_le_bytes());
    p.extend(content::patch::crc32(&target).to_le_bytes());
    p.extend(content::patch::crc32(&p).to_le_bytes());
    (p, target)
}

/// `bytes` with the header checksum recomputed over the payload.
fn resigned(mut bytes: Vec<u8>) -> Vec<u8> {
    let sum = checksum(&bytes[0x800..]);
    bytes[0x2d0..0x2d4].copy_from_slice(&sum.to_le_bytes());
    bytes
}

/// With no packages (an empty maps directory, a cache directory, a broken
/// package, a package for a map the disc lacks), the wrapped disc is the
/// disc.
#[test]
fn without_a_usable_package_the_disc_is_unchanged() {
    let t = TempDir::new();
    let iso = disc(&t);
    let cfg = config(&t);
    let mut plain = linked(Vfs::with_disc(cfg.clone(), &iso).unwrap(), &cfg);
    let expected = observe(&mut plain);
    drop(plain);
    let (mut wrapped, _) = with_packages(&t, &iso);
    assert_eq!(observe(&mut wrapped), expected);
    drop(wrapped);
    fs::create_dir_all(t.maps().join("cache")).unwrap();
    fs::create_dir_all(t.maps().join("broken")).unwrap();
    fs::write(t.maps().join("broken").join("manifest.toml"), "nope").unwrap();
    package(&t, "new", "new", b"x", &[("a.ips", &ips(&[(0, b"y")]))]);
    // A base that is not the disc's: refused, the disc's map served.
    package(
        &t,
        "wrong",
        "ascension",
        b"another base",
        &[("a.ips", &ips(&[(0x900, b"y")]))],
    );
    let (mut wrapped, notices) = with_packages(&t, &iso);
    assert_eq!(observe(&mut wrapped), expected);
    let notices = notices.lock().unwrap();
    assert!(notices.iter().any(|n| n.contains("broken")), "{notices:?}");
    assert!(
        notices
            .iter()
            .any(|n| n.contains("does not match") && n.contains("using the disc's copy")),
        "{notices:?}"
    );
}

#[test]
fn an_offset_patched_map_streams_through_d_with_its_checksum_repaired() {
    let t = TempDir::new();
    let iso = disc(&t);
    let base = ascension();
    let m = package(
        &t,
        "ascension-mod",
        "Ascension",
        &base,
        &[
            ("a.ppf", &ppf1(&[(0x800, b"first"), (0x10ff, b"!")])),
            ("b.ips", &ips(&[(0x803, b"IPS"), (0x1100, b"grown")])),
        ],
    );
    let mut want = base.clone();
    want[0x800..0x805].copy_from_slice(b"first");
    want[0x803..0x806].copy_from_slice(b"IPS");
    want[0x10ff] = b'!';
    want.extend(b"grown");
    let want = resigned(want);
    let (mut vfs, notices) = with_packages(&t, &iso);
    assert_eq!(read_file(&mut vfs, "D:\\maps\\ascension.map"), want);
    assert_eq!(read_file(&mut vfs, "D:\\MAPS\\ASCENSION.MAP"), want);
    assert_eq!(
        read_file(&mut vfs, "D:\\maps\\mainmenu.map"),
        cache_file(DISC_MENU)
    );
    // Reads at arbitrary offsets and lengths.
    let h = vfs
        .open("D:\\maps\\Ascension.map", OpenOptions::default())
        .unwrap()
        .handle;
    let mut seed = 7u64;
    for _ in 0..200 {
        seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        let at = (seed >> 33) % want.len() as u64;
        let n = ((seed >> 13) % 0x400) as usize + 1;
        let mut buf = vec![0; n];
        let got = vfs.read(h, Offset::Absolute(at), &mut buf).unwrap();
        let end = (at as usize + n).min(want.len());
        assert_eq!(&buf[..got], &want[at as usize..end], "at {at:#x}");
    }
    vfs.close(h).unwrap();
    let info = vfs.query_path("D:\\maps\\ascension.map").unwrap();
    assert_eq!(info.end_of_file, want.len() as u64);
    assert_eq!(info.basic.attributes, ATTR_READONLY);
    let sizes: Vec<_> = listing(&mut vfs, "D:\\maps")
        .iter()
        .map(|e| (e.name.clone(), e.metadata.end_of_file))
        .collect();
    assert_eq!(
        sizes,
        [
            ("Ascension.map".to_owned(), want.len() as u64),
            ("mainmenu.map".to_owned(), 0x1800)
        ]
    );
    // Not one region of a host file: no host location.
    assert_eq!(vfs.host_file("D:\\maps\\ascension.map"), None);
    assert!(vfs.host_file("D:\\maps\\mainmenu.map").is_some());
    // Nothing copied.
    assert!(!t.maps().join("cache").exists());
    let notices = notices.lock().unwrap();
    assert_eq!(notices.len(), 1, "{notices:?}");
    assert!(notices[0].contains(&m.map_id().to_string()), "{notices:?}");
    assert_eq!(
        vfs.open(
            "D:\\maps\\ascension.map",
            OpenOptions {
                access: Access::ALL,
                ..Default::default()
            }
        )
        .err(),
        Some(Error::MediaWriteProtected)
    );
}

#[test]
fn a_bps_map_is_served_from_the_cache() {
    let t = TempDir::new();
    let iso = disc(&t);
    let base = ascension();
    let (patch, target) = bps_append(&base, b"appended by bps");
    let m = package(&t, "p", "ascension", &base, &[("p.bps", &patch)]);
    let want = resigned(target);
    let (mut vfs, _) = with_packages(&t, &iso);
    assert_eq!(read_file(&mut vfs, "D:\\maps\\ascension.map"), want);
    let host = vfs.host_file("D:\\maps\\ascension.map").unwrap();
    assert_eq!(
        host.path.file_name().unwrap().to_str().unwrap(),
        format!("{}.map", m.map_id())
    );
    assert_eq!((host.offset, host.len), (0, want.len() as u64));
    assert_eq!(fs::read(&host.path).unwrap(), want);
}

/// `H2_MAP_DIR` stays a whole-file override over everything, packages too.
#[test]
fn a_map_dir_override_wins_over_a_package() {
    let t = TempDir::new();
    let iso = disc(&t);
    let base = ascension();
    package(
        &t,
        "p",
        "ascension",
        &base,
        &[("a.ips", &ips(&[(0x900, b"pkg")]))],
    );
    let edited = cache_file(&[0x5a; 0x2345]);
    fs::write(t.overrides().join("ascension.map"), &edited).unwrap();
    let cfg = config(&t);
    let packages = PackageBackend::with_notice(
        DiscBackend::open(&iso).unwrap(),
        content::Library::open(t.maps()),
        Box::new(|_: &str| {}),
    );
    let backend = MapOverrideBackend::with_notice(packages, t.overrides(), Box::new(|_: &str| {}));
    let mut vfs = linked(Vfs::with_disc_backend(cfg.clone(), backend).unwrap(), &cfg);
    assert_eq!(read_file(&mut vfs, "D:\\maps\\ascension.map"), edited);
}

/// Make `dir` the package `active.toml` picks for `map`.
fn activate(t: &TempDir, map: &str, dir: &str) {
    fs::write(t.maps().join("active.toml"), format!("{map} = \"{dir}\"\n")).unwrap();
}
fn open_map(vfs: &mut Vfs, name: &str) -> Handle {
    vfs.open(name, OpenOptions::default()).unwrap().handle
}
fn read_at(vfs: &mut Vfs, h: Handle, at: u64, n: usize) -> Vec<u8> {
    let mut out = vec![0; n];
    let got = vfs.read(h, Offset::Absolute(at), &mut out).unwrap();
    out.truncate(got);
    out
}
const ASCENSION: &str = "D:\\maps\\ascension.map";
fn ascension_id(vfs: &mut Vfs) -> Option<content::MapId> {
    vfs.map_id(&format!("{CDROM_DEVICE}\\maps\\ascension.map"))
}

/// A package written into `<data>/maps` while the game runs (a Live join,
/// the editor's save) is served at the map's next open, with no restart and
/// no explicit call: the open notices the folder changed.
#[test]
fn a_package_installed_while_running_is_served_at_the_next_open() {
    let t = TempDir::new();
    let iso = disc(&t);
    let (mut vfs, notices) = with_packages(&t, &iso);
    assert_eq!(read_file(&mut vfs, ASCENSION), ascension());
    assert_eq!(ascension_id(&mut vfs), None);
    let m = package(
        &t,
        "live-0123456789abcdef",
        "ascension",
        &ascension(),
        &[("a.ips", &ips(&[(0x900, b"NEW")]))],
    );
    let mut want = ascension();
    want[0x900..0x903].copy_from_slice(b"NEW");
    let want = resigned(want);
    assert_eq!(read_file(&mut vfs, ASCENSION), want);
    assert_eq!(ascension_id(&mut vfs), Some(m.map_id()));
    // The listing of maps shows it too.
    let entry = listing(&mut vfs, "D:\\maps")
        .into_iter()
        .find(|e| e.name.eq_ignore_ascii_case("ascension.map"))
        .unwrap();
    assert_eq!(entry.metadata.end_of_file, want.len() as u64);
    let notices = notices.lock().unwrap();
    assert!(
        notices.iter().any(|n| n.contains("changed: 1 package")),
        "{notices:?}"
    );
    // Other maps are untouched.
    drop(notices);
    assert_eq!(
        read_file(&mut vfs, "D:\\maps\\mainmenu.map"),
        cache_file(DISC_MENU)
    );
}

/// A map whose package changes while a file object has it open keeps
/// serving that object (and any other open meanwhile) what it opened; the
/// new package takes effect at the first open after the last close.
#[test]
fn a_changed_package_never_swaps_under_an_open_handle() {
    let t = TempDir::new();
    let iso = disc(&t);
    let a = package(
        &t,
        "a",
        "ascension",
        &ascension(),
        &[("a.ips", &ips(&[(0x900, b"AAA")]))],
    );
    let (mut vfs, _) = with_packages(&t, &iso);
    let h = open_map(&mut vfs, ASCENSION);
    assert_eq!(read_at(&mut vfs, h, 0x900, 3), b"AAA");
    let b = package(
        &t,
        "b",
        "ascension",
        &ascension(),
        &[("b.ips", &ips(&[(0x900, b"BBB")]))],
    );
    activate(&t, "ascension", "b");
    // Still open: the old map, for this handle and for a second open.
    assert_eq!(read_at(&mut vfs, h, 0x900, 3), b"AAA");
    let h2 = open_map(&mut vfs, ASCENSION);
    assert_eq!(read_at(&mut vfs, h2, 0x900, 3), b"AAA");
    assert_eq!(ascension_id(&mut vfs), Some(a.map_id()));
    // An explicit rescan does not swap it either.
    vfs.rescan_maps();
    assert_eq!(read_at(&mut vfs, h, 0x900, 3), b"AAA");
    vfs.close(h).unwrap();
    assert_eq!(read_at(&mut vfs, h2, 0x900, 3), b"AAA");
    vfs.close(h2).unwrap();
    // Closed: the next open gets the new package.
    let h = open_map(&mut vfs, ASCENSION);
    assert_eq!(read_at(&mut vfs, h, 0x900, 3), b"BBB");
    assert_eq!(ascension_id(&mut vfs), Some(b.map_id()));
    vfs.close(h).unwrap();
    // Choosing the disc again (active.toml: false) serves the disc's map.
    fs::write(t.maps().join("active.toml"), "ascension = false\n").unwrap();
    assert_eq!(read_file(&mut vfs, ASCENSION), ascension());
    assert_eq!(ascension_id(&mut vfs), None);
}

/// A rescan resolves again only maps whose chosen package changed: a map
/// whose package did not change is not read again (its base not hashed a
/// second time), and its choice is not logged again.
#[test]
fn a_rescan_keeps_maps_whose_package_did_not_change() {
    let t = TempDir::new();
    let iso = disc(&t);
    package(
        &t,
        "asc",
        "ascension",
        &ascension(),
        &[("a.ips", &ips(&[(0x900, b"one")]))],
    );
    let (mut vfs, notices) = with_packages(&t, &iso);
    read_file(&mut vfs, ASCENSION);
    let resolved = |n: &Notices| {
        n.lock()
            .unwrap()
            .iter()
            .filter(|l| l.starts_with("maps\\ascension.map from package"))
            .count()
    };
    assert_eq!(resolved(&notices), 1);
    let menu = cache_file(DISC_MENU);
    package(
        &t,
        "menu",
        "mainmenu",
        &menu,
        &[("m.ips", &ips(&[(0x800, b"M")]))],
    );
    vfs.rescan_maps();
    read_file(&mut vfs, ASCENSION);
    assert_eq!(resolved(&notices), 1, "unchanged choice kept");
    let mut want = menu.clone();
    want[0x800] = b'M';
    assert_eq!(
        read_file(&mut vfs, "D:\\maps\\mainmenu.map"),
        resigned(want)
    );
    // Removing the package: the disc's map at the next open.
    fs::remove_dir_all(t.maps().join("asc")).unwrap();
    assert_eq!(read_file(&mut vfs, ASCENSION), ascension());
}

/// With an empty (or missing) maps directory at mount, a package that
/// appears later is still served: the folder need not exist at boot.
#[test]
fn a_maps_directory_created_after_mount_is_seen() {
    let t = TempDir::new();
    let iso = disc(&t);
    fs::remove_dir_all(t.maps()).unwrap();
    let (mut vfs, _) = with_packages(&t, &iso);
    assert_eq!(read_file(&mut vfs, ASCENSION), ascension());
    let m = package(
        &t,
        "asc",
        "ascension",
        &ascension(),
        &[("a.ips", &ips(&[(0x900, b"new")]))],
    );
    let h = open_map(&mut vfs, ASCENSION);
    assert_eq!(read_at(&mut vfs, h, 0x900, 3), b"new");
    vfs.close(h).unwrap();
    assert_eq!(ascension_id(&mut vfs), Some(m.map_id()));
}

/// `map_changed` says when a map last opened from one package (or the
/// disc) would now come from another: what the boot needs to drop the
/// game's cached copy (docs/cache-slots.md, "Changed maps").
#[test]
fn map_changed_compares_the_last_open_with_the_folder_now() {
    let t = TempDir::new();
    let iso = disc(&t);
    let (mut vfs, _) = with_packages(&t, &iso);
    let name = format!("{CDROM_DEVICE}\\maps\\ascension.map");
    // Never opened: nothing to say.
    assert!(!vfs.map_changed(&name));
    read_file(&mut vfs, ASCENSION);
    assert!(!vfs.map_changed(&name));
    assert!(!vfs.map_changed(&format!("{CDROM_DEVICE}\\maps\\mainmenu.map")));
    package(
        &t,
        "a",
        "ascension",
        &ascension(),
        &[("a.ips", &ips(&[(0x900, b"AAA")]))],
    );
    // Installed after the last open: changed, with no open in between,
    // and still so until the next open.
    assert!(vfs.map_changed(&name));
    assert!(vfs.map_changed(&name));
    assert!(!vfs.map_changed(&format!("{CDROM_DEVICE}\\maps\\mainmenu.map")));
    // Open now: not changed (the copy in flight has what it has).
    let h = open_map(&mut vfs, ASCENSION);
    assert!(!vfs.map_changed(&name));
    vfs.close(h).unwrap();
    assert!(!vfs.map_changed(&name));
    // A package that fails to resolve is still the same choice: no churn.
    package(
        &t,
        "b",
        "ascension",
        b"not the disc's",
        &[("b.ips", &ips(&[]))],
    );
    activate(&t, "ascension", "b");
    assert!(vfs.map_changed(&name));
    assert_eq!(read_file(&mut vfs, ASCENSION), ascension());
    assert!(!vfs.map_changed(&name));
    // Through an H2_MAP_DIR override, a path it serves never changes.
    drop(vfs);
    fs::write(t.overrides().join("ascension.map"), b"override").unwrap();
    let cfg = config(&t);
    let backend = MapOverrideBackend::new(
        PackageBackend::new(
            DiscBackend::open(&iso).unwrap(),
            content::Library::open(t.maps()),
        ),
        t.overrides(),
    );
    let mut vfs = linked(Vfs::with_disc_backend(cfg.clone(), backend).unwrap(), &cfg);
    read_file(&mut vfs, ASCENSION);
    activate(&t, "ascension", "a");
    assert!(!vfs.map_changed(&name));
}
