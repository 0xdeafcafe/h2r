//! Map packages over the Multiplayer Map Pack's installed maps, through the
//! whole VFS: a synthetic map installed where `h2 import-mappack` puts it
//! (`<data>/disk/Partition1/TDATA/4D530064/$c/<offer>/<map>.map`), packages
//! under `<data>/maps`, and the game's view through `T:` (docs/game/maps.md,
//! "Importing a mod"). Every byte is invented.
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use vfs::*;

use content::{Manifest, Sha256, manifest::Layer};

const TITLE: u32 = 0x4D53_0064;
/// A map pack offer (`…0102`), and a new map's offer (not the map pack's).
const PACK_OFFER: &str = "4d53006400000102";
const OTHER_OFFER: &str = "4d530064000a0001";

struct TempDir(PathBuf);
impl TempDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir);
        let path = base.join(format!(
            "h2-mappack-packages-it-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("data").join("maps")).unwrap();
        Self(path)
    }
    fn data(&self) -> PathBuf {
        self.0.join("data")
    }
    fn maps(&self) -> PathBuf {
        self.data().join("maps")
    }
    /// Host directory of `$c\<offer>`.
    fn offer(&self, offer: &str) -> PathBuf {
        Config::new(self.data(), TITLE)
            .title_data_dir()
            .join(CONTENT_DIR)
            .join(offer)
    }
}
impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The retail copy-time checksum, written out as an independent oracle
/// (as in tests/packages.rs).
fn checksum(payload: &[u8]) -> u32 {
    payload.as_chunks::<8>().0.iter().fold(0, |c, q| {
        c ^ u32::from_le_bytes(q[..4].try_into().unwrap())
            ^ u32::from_le_bytes(q[4..].try_into().unwrap())
            ^ 0x8844_2211
    })
}
/// An invented cache file: a 0x800 header with its checksum, then `payload`.
fn cache_file(payload: &[u8]) -> Vec<u8> {
    let mut bytes = vec![0; 0x800];
    bytes[..4].copy_from_slice(b"daeh");
    bytes[8..12].copy_from_slice(&(0x800 + payload.len() as u32).to_le_bytes());
    bytes[0x2d0..0x2d4].copy_from_slice(&checksum(payload).to_le_bytes());
    bytes[0x7fc..].copy_from_slice(b"toof");
    bytes.extend_from_slice(payload);
    bytes
}
fn resigned(mut bytes: Vec<u8>) -> Vec<u8> {
    let sum = checksum(&bytes[0x800..]);
    bytes[0x2d0..0x2d4].copy_from_slice(&sum.to_le_bytes());
    bytes
}

/// The installed "turf.map" (as the map pack disc names it, any case).
fn turf() -> Vec<u8> {
    let payload: Vec<u8> = (0..0x3000u32).map(|i| (i * 7 + 3) as u8).collect();
    cache_file(&payload)
}

/// Install the map pack's Turf as the import does, beside its other files,
/// and the same name under a non map pack offer.
fn install(t: &TempDir) {
    let dir = t.offer(PACK_OFFER);
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join("Turf.map"), turf()).unwrap();
    fs::write(dir.join("patch.lvl"), b"invented record").unwrap();
    fs::write(dir.join("ContentMeta.xbx"), b"invented meta").unwrap();
    let other = t.offer(OTHER_OFFER);
    fs::create_dir_all(&other).unwrap();
    fs::write(other.join("turf.map"), turf()).unwrap();
}

/// A package for `turf` over the installed map, with `patch` as its layer.
fn package(t: &TempDir, dir: &str, file: &str, patch: &[u8]) -> Manifest {
    let d = t.maps().join(dir);
    fs::create_dir_all(&d).unwrap();
    let mut m = Manifest::retail("turf", Sha256::of(&turf()));
    m.package.name = dir.into();
    fs::write(d.join(file), patch).unwrap();
    m.patches.push(Layer {
        file: file.into(),
        sha256: Sha256::of(patch),
    });
    fs::write(d.join("manifest.toml"), m.to_toml()).unwrap();
    m
}

/// Storage with the map pack's packages layer (`packages`) or without.
fn mounted(t: &TempDir, packages: bool) -> Vfs {
    let mut cfg = Config::new(t.data(), TITLE);
    if packages {
        cfg.map_packages = Some(t.maps());
    }
    let mut vfs = Vfs::with_storage(cfg.clone()).unwrap();
    vfs.create_standard_links(&cfg).unwrap();
    vfs
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
fn read_at(vfs: &mut Vfs, name: &str, at: u64, n: usize) -> Vec<u8> {
    let h = vfs.open(name, OpenOptions::default()).unwrap().handle;
    let mut out = vec![0; n];
    let got = vfs.read(h, Offset::Absolute(at), &mut out).unwrap();
    out.truncate(got);
    vfs.close(h).unwrap();
    out
}
fn listing(vfs: &mut Vfs, dir: &str) -> Vec<(String, u64)> {
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
            Ok(e) => out.push((e.name, e.metadata.end_of_file)),
            Err(Error::NoMoreFiles) => break,
            Err(e) => panic!("{e:?}"),
        }
    }
    vfs.close(h).unwrap();
    out.sort();
    out
}
fn writable(disposition: CreateDisposition) -> OpenOptions {
    OpenOptions {
        access: Access::ALL,
        disposition,
        ..Default::default()
    }
}

fn pack_path() -> String {
    format!("T:\\$c\\{PACK_OFFER}\\turf.map")
}

/// A same-length mod (PPF3, as `h2 live maps import` makes it) streams
/// over the installed map at exactly its path; every other file of the
/// partition, the map pack's own package files and the same name under
/// another offer included, reads as it is.
#[test]
fn a_ppf_mod_streams_over_the_installed_map_and_nothing_else_changes() {
    let t = TempDir::new();
    install(&t);
    let base = turf();
    let mut edited = base.clone();
    edited[0x900..0x910].copy_from_slice(b"ownage modz turf");
    edited[0x2000] ^= 0xff;
    let edited = resigned(edited);
    let patch = content::patch::diff(&base, &edited, "turf");
    assert_eq!(patch.format, content::patch::Format::Ppf3);

    // Without a package, or without the layer: the installed bytes.
    for packages in [false, true] {
        let mut vfs = mounted(&t, packages);
        assert_eq!(read_file(&mut vfs, &pack_path()), base);
        assert_eq!(vfs.map_id(&pack_path()), None);
    }
    let m = package(&t, "turf-ownage", "mod.ppf", &patch.bytes);
    let mut plain = mounted(&t, false);
    assert_eq!(
        read_file(&mut plain, &pack_path()),
        base,
        "no layer: untouched"
    );

    let mut vfs = mounted(&t, true);
    for name in [
        pack_path(),
        format!("T:\\$C\\{}\\TURF.MAP", PACK_OFFER.to_uppercase()),
        format!("\\Device\\Harddisk0\\Partition1\\TDATA\\4D530064\\$c\\{PACK_OFFER}\\Turf.map"),
    ] {
        assert_eq!(read_file(&mut vfs, &name), edited, "{name}");
        assert_eq!(vfs.map_id(&name), Some(m.map_id()), "{name}");
    }
    assert_eq!(
        read_at(&mut vfs, &pack_path(), 0x900, 16),
        b"ownage modz turf"
    );
    // Everything else as it is.
    let other = format!("T:\\$c\\{OTHER_OFFER}\\turf.map");
    assert_eq!(read_file(&mut vfs, &other), base);
    assert_eq!(vfs.map_id(&other), None);
    assert_eq!(
        read_file(&mut vfs, &format!("T:\\$c\\{PACK_OFFER}\\patch.lvl")),
        b"invented record"
    );
    assert_eq!(
        listing(&mut vfs, &format!("T:\\$c\\{PACK_OFFER}")),
        listing(&mut plain, &format!("T:\\$c\\{PACK_OFFER}")),
        "same length: the listing is the same"
    );
    // The installed file on the host is never touched.
    assert_eq!(
        fs::read(t.offer(PACK_OFFER).join("Turf.map")).unwrap(),
        base
    );
}

/// A longer mod (BPS, materialised): its size in `stat` and listings, its
/// bytes past the installed map's end.
#[test]
fn a_bps_mod_grows_the_map_past_the_installed_size() {
    let t = TempDir::new();
    install(&t);
    let base = turf();
    let mut edited = base.clone();
    edited.extend((0..0x2345u32).map(|i| (i * 13 + 1) as u8));
    edited[0x1234] = 0x42;
    let edited = resigned(edited);
    let patch = content::patch::diff(&base, &edited, "turf");
    assert_eq!(patch.format, content::patch::Format::Bps);
    package(&t, "turf-big", "mod.bps", &patch.bytes);

    let mut vfs = mounted(&t, true);
    let meta = vfs.query_path(&pack_path()).unwrap();
    assert_eq!(meta.end_of_file, edited.len() as u64);
    assert!(meta.allocation_size >= meta.end_of_file);
    let listed = listing(&mut vfs, &format!("T:\\$c\\{PACK_OFFER}"));
    assert!(
        listed.contains(&("Turf.map".to_string(), edited.len() as u64)),
        "{listed:?}"
    );
    let past = base.len() as u64 + 0x100;
    assert_eq!(
        read_at(&mut vfs, &pack_path(), past, 64),
        edited[past as usize..][..64]
    );
    // A read across the base's end, and one running off the grown end.
    let across = base.len() as u64 - 8;
    assert_eq!(
        read_at(&mut vfs, &pack_path(), across, 16),
        edited[across as usize..][..16]
    );
    let tail = edited.len() as u64 - 5;
    assert_eq!(
        read_at(&mut vfs, &pack_path(), tail, 64),
        edited[tail as usize..]
    );
    assert_eq!(read_file(&mut vfs, &pack_path()), edited);
}

/// A served map is read-only; the rest of the partition stays writable:
/// saves, new files, the map pack's other files.
#[test]
fn a_served_map_refuses_writes_and_the_partition_stays_writable() {
    let t = TempDir::new();
    install(&t);
    let base = turf();
    let mut edited = base.clone();
    edited[0x900] ^= 1;
    let patch = content::patch::diff(&base, &resigned(edited.clone()), "turf");
    package(&t, "turf-mod", "mod.ppf", &patch.bytes);
    let mut vfs = mounted(&t, true);

    let h = vfs
        .open(&pack_path(), writable(CreateDisposition::Open))
        .unwrap()
        .handle;
    assert_eq!(
        vfs.write(h, Offset::Absolute(0), b"x"),
        Err(Error::AccessDenied)
    );
    assert_eq!(vfs.set_end_of_file(h, 1), Err(Error::AccessDenied));
    vfs.close(h).unwrap();
    assert_eq!(
        vfs.open(&pack_path(), writable(CreateDisposition::Overwrite))
            .map(|_| ()),
        Err(Error::AccessDenied)
    );
    assert_eq!(
        fs::read(t.offer(PACK_OFFER).join("Turf.map")).unwrap(),
        base
    );

    // Elsewhere: written as without packages.
    let lvl = format!("T:\\$c\\{PACK_OFFER}\\patch.lvl");
    let h = vfs
        .open(&lvl, writable(CreateDisposition::Open))
        .unwrap()
        .handle;
    assert_eq!(vfs.write(h, Offset::Absolute(0), b"INVENTED").unwrap(), 8);
    vfs.close(h).unwrap();
    assert_eq!(
        fs::read(t.offer(PACK_OFFER).join("patch.lvl")).unwrap(),
        b"INVENTED record"
    );
    let save = "U:\\profile.dat";
    let h = vfs
        .open(save, writable(CreateDisposition::Create))
        .unwrap()
        .handle;
    assert_eq!(vfs.write(h, Offset::Absolute(0), b"saved").unwrap(), 5);
    vfs.close(h).unwrap();
    assert_eq!(read_file(&mut vfs, save), b"saved");
    let other = format!("T:\\$c\\{OTHER_OFFER}\\turf.map");
    let h = vfs
        .open(&other, writable(CreateDisposition::Open))
        .unwrap()
        .handle;
    assert_eq!(vfs.write(h, Offset::Absolute(0), b"X").unwrap(), 1);
    vfs.close(h).unwrap();
}

/// No map pack installed: nothing is invented at its path, and a package
/// for one of its maps changes nothing else.
#[test]
fn without_the_map_pack_installed_nothing_is_served() {
    let t = TempDir::new();
    package(&t, "turf-mod", "mod.ips", b"PATCH\0\0\0\0\x01xEOF");
    let mut vfs = mounted(&t, true);
    assert!(vfs.query_path(&pack_path()).is_err());
    assert!(vfs.open(&pack_path(), OpenOptions::default()).is_err());
    assert_eq!(vfs.map_id(&pack_path()), None);
}
