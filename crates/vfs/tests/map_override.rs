//! Map overrides through the whole VFS: a synthetic XISO as the DVD, an
//! override directory on the host, and the game's view through `D:`.
use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
use vfs::*;

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "h2-map-override-it-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("override")).unwrap();
        Self(path)
    }
    fn overrides(&self) -> PathBuf {
        self.0.join("override")
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
fn with_overrides(t: &TempDir, iso: &Path) -> Vfs {
    let cfg = config(t);
    let backend = MapOverrideBackend::with_notice(
        DiscBackend::open(iso).unwrap(),
        t.overrides(),
        Box::new(|_: &str| {}),
    );
    linked(Vfs::with_disc_backend(cfg.clone(), backend).unwrap(), &cfg)
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

/// With no override for a map, the wrapped disc is the disc: every listing,
/// size, byte, host location, missing name and volume field is the same as
/// `Vfs::with_disc`, even with unrelated files in the directory.
#[test]
fn without_a_matching_override_the_disc_is_unchanged() {
    let t = TempDir::new();
    let iso = disc(&t);
    let cfg = config(&t);
    let mut plain = linked(Vfs::with_disc(cfg.clone(), &iso).unwrap(), &cfg);
    let expected = observe(&mut plain);
    drop(plain);
    fs::write(t.overrides().join("new.map"), b"not on the disc").unwrap();
    fs::write(t.overrides().join("default.xbe"), b"not a map").unwrap();
    fs::create_dir(t.overrides().join("maps")).unwrap();
    fs::write(t.overrides().join("maps").join("mainmenu.map"), b"nested").unwrap();
    let mut wrapped = with_overrides(&t, &iso);
    assert_eq!(observe(&mut wrapped), expected);
}

#[test]
fn guest_sees_the_override_through_d_with_its_size_in_listings() {
    let t = TempDir::new();
    let iso = disc(&t);
    let edited = cache_file(&[0x5a; 0x2345]);
    fs::write(t.overrides().join("ASCENSION.map"), &edited).unwrap();
    let mut vfs = with_overrides(&t, &iso);
    assert_eq!(read_file(&mut vfs, "D:\\maps\\ascension.map"), edited);
    assert_eq!(
        read_file(&mut vfs, "\\Device\\CdRom0\\Maps\\Ascension.MAP"),
        edited
    );
    assert_eq!(
        read_file(&mut vfs, "D:\\maps\\mainmenu.map"),
        cache_file(DISC_MENU)
    );
    let maps = listing(&mut vfs, "D:\\maps");
    let sizes: Vec<_> = maps
        .iter()
        .map(|e| (e.name.as_str(), e.metadata.end_of_file))
        .collect();
    assert_eq!(
        sizes,
        [
            ("Ascension.map", edited.len() as u64),
            ("mainmenu.map", 0x1800)
        ]
    );
    let info = vfs.query_path("D:\\maps\\Ascension.map").unwrap();
    assert_eq!(info.end_of_file, edited.len() as u64);
    assert_eq!(info.basic.attributes, ATTR_READONLY);
    // Host readers (telemetry, the speaker index) find the edited file.
    let host = vfs.host_file("D:\\maps\\ascension.map").unwrap();
    assert_eq!((host.offset, host.len), (0, edited.len() as u64));
    assert_eq!(fs::read(&host.path).unwrap(), edited);
    // Still the read-only DVD.
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

/// Campaign maps reach ~280 MiB, past `RewriteBackend`'s 64 MiB cap: a
/// sparse 300 MiB override is served at every offset without buffering.
#[test]
fn large_override_streams_through_the_guest_api() {
    use std::io::{Seek, SeekFrom, Write};
    let t = TempDir::new();
    let iso = disc(&t);
    let len = 300 * 1024 * 1024u64;
    let mut f = fs::File::create(t.overrides().join("mainmenu.map")).unwrap();
    f.set_len(len).unwrap();
    for (at, mark) in [
        (0x10u64, b"near"),
        (65 * 1024 * 1024, b"past"),
        (len - 4, b"last"),
    ] {
        f.seek(SeekFrom::Start(at)).unwrap();
        f.write_all(mark).unwrap();
    }
    drop(f);
    let mut vfs = with_overrides(&t, &iso);
    let h = vfs
        .open("D:\\maps\\MAINMENU.MAP", OpenOptions::default())
        .unwrap()
        .handle;
    assert_eq!(vfs.query_information(h).unwrap().metadata.end_of_file, len);
    let mut buf = [0u8; 4];
    for (at, mark) in [
        (0x10u64, b"near"),
        (65 * 1024 * 1024, b"past"),
        (len - 4, b"last"),
    ] {
        assert_eq!(vfs.read(h, Offset::Absolute(at), &mut buf).unwrap(), 4);
        assert_eq!(&buf, mark);
    }
    assert_eq!(
        vfs.read(h, Offset::Absolute(len), &mut buf),
        Err(Error::EndOfFile)
    );
    let entry = listing(&mut vfs, "D:\\maps")
        .into_iter()
        .find(|e| e.name == "mainmenu.map")
        .unwrap();
    assert_eq!(entry.metadata.end_of_file, len);
}

/// A model of the retail cache copy (docs/cache-slots.md): read the source's
/// physical size, its 0x800 header, then stream the payload from +0x800 to
/// EOF into a cache file on the hard disk, accumulating the copy-time
/// checksum and comparing it with header +0x2d0. An edited map whose
/// checksum the editor repaired passes and lands byte for byte; the retail
/// routine itself is exercised by the ignored kernel `cache_slots` test.
#[test]
fn overridden_map_survives_a_cache_copy_with_its_checksum() {
    let t = TempDir::new();
    let iso = disc(&t);
    // An odd tail: the final partial qword is copied but not summed.
    let payload: Vec<u8> = (0..0x3_0005u32).map(|i| (i * 7 + i / 251) as u8).collect();
    let edited = cache_file(&payload);
    fs::write(t.overrides().join("ascension.map"), &edited).unwrap();
    let mut vfs = with_overrides(&t, &iso);

    let src = vfs
        .open("D:\\maps\\ascension.map", OpenOptions::default())
        .unwrap()
        .handle;
    let size = vfs.query_information(src).unwrap().metadata.end_of_file;
    let mut header = vec![0; 0x800];
    assert_eq!(
        vfs.read(src, Offset::Absolute(0), &mut header).unwrap(),
        0x800
    );
    let dst = vfs
        .open(
            "Z:\\cache004.map",
            OpenOptions {
                access: Access::ALL,
                disposition: CreateDisposition::OverwriteIf,
                ..Default::default()
            },
        )
        .unwrap()
        .handle;
    let mut sum = Vec::new();
    let mut chunk = vec![0; 0x1_0000];
    let mut at = 0x800;
    while at < size {
        let n = vfs.read(src, Offset::Absolute(at), &mut chunk).unwrap();
        assert_ne!(n, 0, "short source");
        vfs.write(dst, Offset::Absolute(at), &chunk[..n]).unwrap();
        sum.extend_from_slice(&chunk[..n]);
        at += n as u64;
    }
    assert_eq!(
        checksum(&sum),
        u32::from_le_bytes(header[0x2d0..0x2d4].try_into().unwrap()),
        "copy-time checksum"
    );
    vfs.write(dst, Offset::Absolute(0), &header).unwrap();
    vfs.close(dst).unwrap();
    vfs.close(src).unwrap();
    assert_eq!(read_file(&mut vfs, "Z:\\cache004.map"), edited);
    // The disc's own copy was the other map all along.
    assert_ne!(edited, cache_file(DISC_LEVEL));
}
