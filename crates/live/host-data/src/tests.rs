/// Test fixtures shared by the crate's own suites: a synthetic DVD
/// directory holding a default.xbe and a few maps with real headers.
#[cfg(test)]
pub(crate) mod testkit {
    use std::io::Read;
    use std::path::{Path, PathBuf};

    use sha2::{Digest, Sha256};

    pub(crate) const CACHE_TYPE_CAMPAIGN: u32 = 0;
    pub(crate) const CACHE_TYPE_MULTIPLAYER: u32 = 1;
    pub(crate) const CACHE_TYPE_MAINMENU: u32 = 2;
    pub(crate) const CACHE_TYPE_SHARED: u32 = 3;
    pub(crate) const CACHE_TYPE_SINGLE_PLAYER_SHARED: u32 = 4;

    /// A map file with a real 0x800-byte cache header: signatures,
    /// version 8, retail build string, the given cache type and internal
    /// name, plus a small body so sizes differ per map.
    pub(crate) fn map_bytes(cache_type: u32, name: &str) -> Vec<u8> {
        let mut b = vec![0u8; 0x800];
        b[..4].copy_from_slice(b"daeh");
        b[4..8].copy_from_slice(&8u32.to_le_bytes());
        b[0x120..0x120 + b"02.09.27.09809".len()].copy_from_slice(b"02.09.27.09809");
        b[0x140..0x144].copy_from_slice(&cache_type.to_le_bytes());
        b[0x198..0x198 + name.len()].copy_from_slice(name.as_bytes());
        b[0x7fc..0x800].copy_from_slice(b"toof");
        b.extend_from_slice(format!("payload for {name}\n").repeat(64).as_bytes());
        b
    }

    pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
        let mut h = Sha256::new();
        h.update(bytes);
        crate::tar::hex(&h.finalize())
    }

    /// A DVD directory: default.xbe, the three shared maps, two
    /// multiplayer maps and one campaign map (which must be excluded).
    pub(crate) fn write_disc_dir(dir: &Path) {
        let maps = dir.join("maps");
        std::fs::create_dir_all(maps.join("fonts")).unwrap();
        std::fs::write(maps.join("fonts/font_table.txt"), b"conduit-12").unwrap();
        std::fs::write(maps.join("fonts/conduit-12"), b"synthetic packed font").unwrap();
        std::fs::create_dir_all(dir.join("bink")).unwrap();
        std::fs::write(dir.join("bink/intro_60.bik"), b"synthetic movie").unwrap();
        std::fs::write(dir.join("default.xbe"), b"XBEH synthetic executable").unwrap();
        std::fs::write(maps.join("mainmenu.map"), map_bytes(CACHE_TYPE_MAINMENU, "mainmenu")).unwrap();
        std::fs::write(maps.join("shared.map"), map_bytes(CACHE_TYPE_SHARED, "shared")).unwrap();
        std::fs::write(
            maps.join("single_player_shared.map"),
            map_bytes(CACHE_TYPE_SINGLE_PLAYER_SHARED, "single_player_shared"),
        )
        .unwrap();
        std::fs::write(maps.join("lockout.map"), map_bytes(CACHE_TYPE_MULTIPLAYER, "lockout")).unwrap();
        std::fs::write(maps.join("zanzibar.map"), map_bytes(CACHE_TYPE_MULTIPLAYER, "zanzibar")).unwrap();
        std::fs::write(maps.join("a30.map"), map_bytes(CACHE_TYPE_CAMPAIGN, "a30_deltahalo")).unwrap();
    }

    /// A sector-zero XDVDFS image of the synthetic disc. Each directory
    /// uses a right-only chain, with one 64-byte slot per entry.
    pub(crate) fn write_disc_image(image: &Path, disc: &Path) {
        const SECTOR: usize = xiso::SECTOR as usize;
        const ROOT: u32 = 33;

        fn write_directory(bytes: &mut Vec<u8>, dir: &Path, sector: u32) {
            let mut entries: Vec<_> = std::fs::read_dir(dir)
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap();
            entries.sort_by_key(|entry| entry.file_name());
            assert!(entries.len() * 64 <= SECTOR);
            let count = entries.len();
            for (i, entry) in entries.into_iter().enumerate() {
                let name = entry.file_name().into_string().unwrap();
                assert!(name.len() <= 50);
                let child_sector = (bytes.len() / SECTOR) as u32;
                let child_start = bytes.len();
                let is_dir = entry.file_type().unwrap().is_dir();
                let size = if is_dir {
                    bytes.resize(child_start + SECTOR, 0);
                    write_directory(bytes, &entry.path(), child_sector);
                    SECTOR
                } else {
                    let file = std::fs::read(entry.path()).unwrap();
                    bytes.extend_from_slice(&file);
                    bytes.resize(bytes.len().next_multiple_of(SECTOR), 0);
                    file.len()
                };
                let start = sector as usize * SECTOR + i * 64;
                let node = &mut bytes[start..start + 64];
                let right = if i + 1 == count { 0 } else { (i as u16 + 1) * 16 };
                node[2..4].copy_from_slice(&right.to_le_bytes());
                node[4..8].copy_from_slice(&child_sector.to_le_bytes());
                node[8..12].copy_from_slice(&(size as u32).to_le_bytes());
                node[12] = if is_dir { 0x10 } else { 0 };
                node[13] = name.len() as u8;
                node[14..14 + name.len()].copy_from_slice(name.as_bytes());
            }
        }

        let mut bytes = vec![0u8; (ROOT as usize + 1) * SECTOR];
        let desc = &mut bytes[32 * SECTOR..33 * SECTOR];
        desc[..20].copy_from_slice(b"MICROSOFT*XBOX*MEDIA");
        desc[20..24].copy_from_slice(&ROOT.to_le_bytes());
        desc[24..28].copy_from_slice(&(SECTOR as u32).to_le_bytes());
        write_directory(&mut bytes, disc, ROOT);
        std::fs::write(image, bytes).unwrap();
    }

    /// A unique directory under the system temp dir, removed on drop.
    pub(crate) struct TempDir(pub(crate) PathBuf);
    impl TempDir {
        pub(crate) fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "host-data-{tag}-{}-{}",
                std::process::id(),
                crate::unique_suffix()
            ));
            std::fs::create_dir_all(&dir).unwrap();
            Self(dir)
        }
    }
    impl Drop for TempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    impl std::ops::Deref for TempDir {
        type Target = Path;
        fn deref(&self) -> &Path {
            &self.0
        }
    }

    /// A hand-built archive entry our writer would refuse to make, for
    /// reader-side rejection tests.
    pub(crate) fn raw_entry(name: &str, typeflag: u8, data: &[u8]) -> Vec<u8> {
        let mut h = [0u8; 512];
        let n = name.len().min(99);
        h[..n].copy_from_slice(&name.as_bytes()[..n]);
        h[124..136].copy_from_slice(format!("{:011o}\0", data.len()).as_bytes());
        h[156] = typeflag;
        h[257..263].copy_from_slice(b"ustar\0");
        h[263..265].copy_from_slice(b"00");
        let sum: u32 = h.iter().map(|&b| u32::from(b)).sum::<u32>() + 8 * u32::from(b' ');
        h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
        let mut out = h.to_vec();
        out.extend_from_slice(data);
        let pad = (512 - data.len() % 512) % 512;
        out.extend(std::iter::repeat_n(0u8, pad));
        out
    }

    /// zstd-compress a hand-built archive.
    pub(crate) fn compress(entries: &[Vec<u8>]) -> Vec<u8> {
        let mut tar = entries.concat();
        tar.extend_from_slice(&[0u8; 1024]);
        zstd::bulk::compress(&tar, 3).unwrap()
    }

    pub(crate) fn read_all(p: &Path) -> Vec<u8> {
        let mut v = Vec::new();
        std::fs::File::open(p).unwrap().read_to_end(&mut v).unwrap();
        v
    }

    pub(crate) fn file_url(p: &Path) -> String {
        format!("file://{}", p.display())
    }
}

use self::testkit::TempDir;
use super::*;
use std::fs;

fn build_fixture_disc(out: &TempDir) -> (PathBuf, crate::BundleOut) {
    let disc = out.join("disc");
    fs::create_dir_all(&disc).unwrap();
    testkit::write_disc_dir(&disc);
    let bundle_out = out.join("bundle");
    let built = crate::bundle::build(&crate::BundleSpec {
        disc: disc.clone(),
        out: bundle_out,
        stripped: None,
        sim_stores: None,
        force: false,
        zstd_level: 3,
    })
    .unwrap();
    (disc, built)
}

#[test]
fn provision_round_trips_a_built_bundle() {
    let tmp = TempDir::new("roundtrip");
    let (disc, built) = build_fixture_disc(&tmp);
    let bundle = tmp.join("bundle");
    let manifest: Manifest =
        serde_json::from_slice(&testkit::read_all(&bundle.join(MANIFEST_NAME))).unwrap();
    assert_eq!(manifest.sha256, built.manifest.sha256);

    let cache = tmp.join("cache");
    let url = testkit::file_url(&bundle.join(ARCHIVE_NAME));
    let dir = provision(&url, &manifest.sha256, &cache).unwrap();

    // The payload matches the disc file for file, and the manifest
    // travels inside the archive.
    for f in &manifest.files {
        let want = testkit::read_all(&disc.join(&f.path));
        assert_eq!(testkit::read_all(&dir.join(&f.path)), want, "{}", f.path);
        assert_eq!(f.sha256, testkit::sha256_hex(&want));
    }
    assert_eq!(read_embedded_manifest(&dir).unwrap().files, manifest.files);
    assert_eq!(bundled_store_dir(&dir).unwrap(), None);
    assert!(!dir.join("bink").exists());
    assert!(!dir.join("maps/a30.map").exists());
    // No partials or staging left behind.
    assert!(fs::read_dir(&cache).unwrap().flatten().all(|e| {
        let n = e.file_name().into_string().unwrap();
        n == manifest.sha256 || n == CACHE_LOCK
    }));
}

#[test]
fn a_cached_bundle_is_reused_without_downloading() {
    let tmp = TempDir::new("reuse");
    let (_, built) = build_fixture_disc(&tmp);
    let bundle = tmp.join("bundle");
    let sha = built.manifest.sha256.clone();
    let cache = tmp.join("cache");
    let url = testkit::file_url(&bundle.join(ARCHIVE_NAME));

    let first = provision_with(&url, &sha, &cache, &Options { max_cache_bytes: u64::MAX }).unwrap();
    // The source disappears: a second download could not succeed.
    fs::remove_file(bundle.join(ARCHIVE_NAME)).unwrap();
    let second = provision_with(&url, &sha, &cache, &Options { max_cache_bytes: u64::MAX }).unwrap();
    assert_eq!(first, second);
    assert!(first.join("default.xbe").is_file());
}

#[test]
fn a_wrong_hash_is_rejected_and_cleans_up_after_itself() {
    let tmp = TempDir::new("badhash");
    let (_, _) = build_fixture_disc(&tmp);
    let bundle = tmp.join("bundle");
    let cache = tmp.join("cache");
    let url = testkit::file_url(&bundle.join(ARCHIVE_NAME));
    let wrong = "0".repeat(64);

    let err = provision_with(&url, &wrong, &cache, &Options { max_cache_bytes: u64::MAX }).unwrap_err();
    assert!(matches!(err, Error::Hash { .. }), "{err}");
    // Neither an entry nor a partial survived.
    let entries = fs::read_dir(&cache).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_name(), CACHE_LOCK);
}

#[test]
fn a_traversal_entry_is_rejected_before_anything_is_written() {
    let tmp = TempDir::new("traversal");
    let archive = tmp.join("evil.tar.zst");
    let bytes = testkit::compress(&[
        testkit::raw_entry("../escape", b'0', b"evil"),
        testkit::raw_entry("host-data.json", b'0', br#"{"sha256":"","files":[]}"#),
    ]);
    fs::write(&archive, bytes).unwrap();
    let cache = tmp.join("cache");
    let sha = testkit::sha256_hex(&testkit::read_all(&archive));
    let err = provision_with(
        &testkit::file_url(&archive),
        &sha,
        &cache,
        &Options { max_cache_bytes: u64::MAX },
    )
    .unwrap_err();
    assert!(matches!(err, Error::UnsafePath(_)), "{err}");
    assert!(!tmp.join("escape").exists());
    let entries = fs::read_dir(&cache).unwrap().collect::<Result<Vec<_>, _>>().unwrap();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_name(), CACHE_LOCK);
}

#[test]
fn a_link_entry_is_rejected() {
    let tmp = TempDir::new("linkentry");
    let archive = tmp.join("link.tar.zst");
    let entry = testkit::raw_entry("maps/evil.map", b'2', b"");
    let bytes = testkit::compress(&[entry]);
    fs::write(&archive, bytes).unwrap();
    let cache = tmp.join("cache");
    let sha = testkit::sha256_hex(&testkit::read_all(&archive));
    let err = provision_with(
        &testkit::file_url(&archive),
        &sha,
        &cache,
        &Options { max_cache_bytes: u64::MAX },
    )
    .unwrap_err();
    assert!(matches!(&err, Error::Corrupt(m) if m.contains("regular")), "{err}");
}

#[test]
fn a_manifest_that_lies_about_a_file_is_rejected() {
    let tmp = TempDir::new("lyingmanifest");
    let archive = tmp.join("lying.tar.zst");
    let payload = b"true bytes";
    let mut files = Vec::new();
    let mut entries = Vec::new();
    for path in REQUIRED_FILES {
        entries.push(testkit::raw_entry(path, b'0', payload));
        files.push(FileEntry {
            path: path.to_owned(),
            size: payload.len() as u64,
            sha256: testkit::sha256_hex(payload),
        });
    }
    files[0].sha256 = testkit::sha256_hex(b"other bytes");
    let manifest = Manifest { sha256: String::new(), files };
    let json = serde_json::to_vec(&manifest).unwrap();
    entries.push(testkit::raw_entry(MANIFEST_NAME, b'0', &json));
    let bytes = testkit::compress(&entries);
    fs::write(&archive, bytes).unwrap();
    let cache = tmp.join("cache");
    let sha = testkit::sha256_hex(&testkit::read_all(&archive));
    let err = provision_with(
        &testkit::file_url(&archive),
        &sha,
        &cache,
        &Options { max_cache_bytes: u64::MAX },
    )
    .unwrap_err();
    assert!(matches!(&err, Error::Corrupt(m) if m.contains("default.xbe")), "{err}");
}

#[test]
fn the_cache_evicts_least_recently_used_entries() {
    let tmp = TempDir::new("lru");
    let cache = tmp.join("cache");
    let big = Options { max_cache_bytes: u64::MAX };
    let mut hashes = Vec::new();
    for name in ["a", "b", "c"] {
        let disc = tmp.join(format!("disc-{name}"));
        testkit::write_disc_dir(&disc);
        fs::write(disc.join("default.xbe"), format!("XBEH synthetic build {name}")).unwrap();
        let out = tmp.join(format!("bundle-{name}"));
        let built = crate::build(&crate::BundleSpec {
            disc,
            out: out.clone(),
            stripped: None,
            sim_stores: None,
            force: false,
            zstd_level: 3,
        })
        .unwrap();
        let sha = built.manifest.sha256;
        provision_with(&testkit::file_url(&out.join(ARCHIVE_NAME)), &sha, &cache, &big)
            .unwrap();
        hashes.push(sha);
    }
    let [sha_a, sha_b, sha_c] = hashes.as_slice() else { unreachable!() };
    let old = std::time::SystemTime::now() - Duration::from_secs(3600);
    set_used(&cache.join(sha_a), old).unwrap();
    set_used(&cache.join(sha_b), old + Duration::from_secs(60)).unwrap();

    // One eviction suffices, with two eligible entries. Reuse C to
    // exercise the cap through the same path the supervisor calls.
    let total: u64 = hashes.iter().map(|sha| dir_size(&cache.join(sha)).unwrap()).sum();
    let small = Options { max_cache_bytes: total - 1 };
    provision_with("unused-url", sha_c, &cache, &small).unwrap();
    assert!(!cache.join(sha_a).exists());
    assert!(cache.join(sha_b).is_dir());
    assert!(cache.join(sha_c).is_dir());

    // The entry in use is never the one dropped, even at a zero cap.
    let zero = Options { max_cache_bytes: 0 };
    provision_with("unused-url", sha_c, &cache, &zero).unwrap();
    assert!(!cache.join(sha_b).exists());
    assert!(cache.join(sha_c).is_dir());
}

#[test]
fn unsupported_url_schemes_and_bad_hashes_are_refused_up_front() {
    let tmp = TempDir::new("args");
    let cache = tmp.join("cache");
    let err = provision_with("ftp://example.invalid/x", &"a".repeat(64), &cache, &Options {
        max_cache_bytes: 1,
    })
    .unwrap_err();
    assert!(matches!(err, Error::UnsupportedUrl), "{err}");
    let err = provision_with("http://example.invalid/x", "not-a-hash", &cache, &Options {
        max_cache_bytes: 1,
    })
    .unwrap_err();
    assert!(matches!(err, Error::InvalidHash), "{err}");
}

#[test]
fn sha256s_are_normalized_to_lowercase() {
    let tmp = TempDir::new("case");
    let (_, built) = build_fixture_disc(&tmp);
    let bundle = tmp.join("bundle");
    let sha = built.manifest.sha256.to_ascii_uppercase();
    let cache = tmp.join("cache");
    let dir = provision_with(
        &testkit::file_url(&bundle.join(ARCHIVE_NAME)),
        &sha,
        &cache,
        &Options { max_cache_bytes: u64::MAX },
    )
    .unwrap();
    assert_eq!(dir, cache.join(built.manifest.sha256));
}

#[test]
fn simultaneous_provisions_publish_one_complete_entry() {
    let tmp = TempDir::new("concurrent");
    let (_, built) = build_fixture_disc(&tmp);
    let sha = built.manifest.sha256;
    let url = testkit::file_url(&tmp.join("bundle").join(ARCHIVE_NAME));
    let cache = tmp.join("cache");
    fs::create_dir(&cache).unwrap();
    fs::write(cache.join(format!("{sha}.partial")), b"stale download").unwrap();
    fs::create_dir(cache.join(format!("{sha}.extracting"))).unwrap();

    let start = std::sync::Barrier::new(2);
    let request = || {
        start.wait();
        provision(&url, &sha, &cache).unwrap()
    };
    std::thread::scope(|scope| {
        let first = scope.spawn(request);
        let second = scope.spawn(request);
        assert_eq!(first.join().unwrap(), second.join().unwrap());
    });
    assert_eq!(fs::read_dir(&cache).unwrap().count(), 2);
    assert!(cache.join(&sha).join("default.xbe").is_file());
}

#[cfg(unix)]
#[test]
fn a_symlink_at_the_cache_key_is_rejected() {
    let tmp = TempDir::new("cachelink");
    let (disc, built) = build_fixture_disc(&tmp);
    let cache = tmp.join("cache");
    let sha = built.manifest.sha256;
    fs::create_dir(&cache).unwrap();
    std::os::unix::fs::symlink(&disc, cache.join(&sha)).unwrap();

    let url = testkit::file_url(&tmp.join("bundle").join(ARCHIVE_NAME));
    let err = provision_with(&url, &sha, &cache, &Options { max_cache_bytes: u64::MAX })
        .unwrap_err();
    assert!(matches!(err, Error::UnsafePath(_)), "{err}");
    assert!(disc.join("default.xbe").is_file());
}

#[test]
fn an_incomplete_cache_entry_is_rebuilt() {
    let tmp = TempDir::new("incomplete");
    let (_, built) = build_fixture_disc(&tmp);
    let cache = tmp.join("cache");
    let sha = built.manifest.sha256;
    let url = testkit::file_url(&tmp.join("bundle").join(ARCHIVE_NAME));
    let dir = provision(&url, &sha, &cache).unwrap();
    fs::remove_file(dir.join("maps/zanzibar.map")).unwrap();

    assert_eq!(provision(&url, &sha, &cache).unwrap(), dir);
    assert!(dir.join("maps/zanzibar.map").is_file());
}
