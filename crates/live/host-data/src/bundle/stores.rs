//! Package recorded resource caches. The menu and level files stay contiguous
//! for native readers that open them before the guest VFS is mounted.
use std::{
    collections::BTreeMap,
    fs,
    io::ErrorKind,
    path::{Path, PathBuf},
};

use map::CacheType;
use vfs::{Backend, sim_store::Store};

use super::{HEADER, Payload, Source, classify};
use crate::{Error, STORE_DIR};

const RESOURCES: [(&str, CacheType); 2] = [
    ("shared", CacheType::Shared),
    ("single_player_shared", CacheType::SinglePlayerShared),
];

/// Copy to private staging before verification so the archive writer gets
/// exactly the store whose bytes we compared.
pub(super) fn stage(
    input: &Path,
    disc_path: &Path,
    staging: &Path,
    stripped: &BTreeMap<String, PathBuf>,
    payload: &mut Vec<Payload>,
) -> Result<usize, Error> {
    let mut disc = vfs::open_disc(disc_path)
        .map_err(|e| Error::Bundle(format!("opening the disc for store verification: {e:?}")))?;
    let mut count = 0;
    for (name, kind) in RESOURCES {
        let filename = format!("{name}.h2store");
        let src = input.join(&filename);
        match fs::symlink_metadata(&src) {
            Ok(m) if m.is_file() => {}
            Ok(_) => return Err(Error::Bundle(format!("{} must be a regular file", src.display()))),
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        }
        let map = format!("{name}.map");
        if stripped.contains_key(&map) {
            return Err(Error::Bundle(format!("choose one of the store and the stripped copy of {map}")));
        }
        let path = format!("maps/{map}");
        let original = payload.iter_mut().find(|entry| entry.path.eq_ignore_ascii_case(&path))
            .ok_or_else(|| Error::Bundle(format!("no original {path} in the bundle")))?;
        let packed = staging.join(&filename);
        let size = fs::copy(&src, &packed)?;
        let store = Store::open(&packed).map_err(|e| Error::Bundle(format!("{filename}: {e:?}")))?;
        if store.logical_len() != original.size {
            return Err(Error::Bundle(format!("{filename}: logical length differs from {path}")));
        }
        let mut header = [0; HEADER];
        if store.read_at(0, &mut header).ok() != Some(HEADER) {
            return Err(Error::Bundle(format!("{filename}: the recording must cover the cache header")));
        }
        if classify(&map, &header)? != kind {
            return Err(Error::Bundle(format!("{filename}: unexpected cache type")));
        }
        verify(&store, &mut disc, &map)?;
        original.size = HEADER as u64;
        original.source = Source::Bytes(header.to_vec());
        payload.push(Payload {
            path: format!("{STORE_DIR}/{filename}"),
            source: Source::File(packed),
            size,
            sha256: String::new(),
        });
        count += 1;
    }
    if count == 0 {
        return Err(Error::Bundle(format!(
            "{} contains neither shared.h2store nor single_player_shared.h2store", input.display()
        )));
    }
    payload.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(count)
}

fn verify(store: &Store, disc: &mut impl Backend, name: &str) -> Result<(), Error> {
    let path = format!("maps\\{name}");
    let mut original = vec![0; 256 * 1024];
    let mut stored = vec![0; original.len()];
    for range in &store.index.ranges {
        let end = range.logical + range.len;
        let mut at = range.logical;
        while at < end {
            let n = (end - at).min(original.len() as u64) as usize;
            let read = disc.read_at(&path, at, &mut original[..n])
                .map_err(|e| Error::Bundle(format!("{path}: reading original at {at}: {e:?}")))?;
            if read != n || store.read_at(at, &mut stored[..n]).ok() != Some(n) || original[..n] != stored[..n] {
                return Err(Error::Bundle(format!("store differs from {path} at range [{at}, {})", at + n as u64)));
            }
            at += n as u64;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testkit::{self, TempDir};

    // Use the normal store builder on a synthetic disc so the test also
    // exercises its index format, including any alignment padding.
    fn fixture(dir: &TempDir, ranges: &[(u64, u64)]) -> crate::BundleSpec {
        let disc = dir.join("disc");
        testkit::write_disc_dir(&disc);
        let iso = dir.join("disc.iso");
        testkit::write_disc_image(&iso, &disc);
        let stores = dir.join("recorded");
        vfs::sim_store::build(&iso, "shared", ranges, &stores.join("shared.h2store")).unwrap();
        crate::BundleSpec {
            disc,
            out: dir.join("bundle"),
            stripped: None,
            sim_stores: Some(stores),
            force: false,
            zstd_level: 1,
        }
    }

    #[test]
    fn resource_store_round_trips_with_its_guest_length_and_missing_range() {
        let dir = TempDir::new("store-bundle");
        let offset = HEADER as u64 + 64;
        let spec = fixture(&dir, &[(0, HEADER as u64), (offset, offset + 8)]);
        let bundle = crate::build(&spec).unwrap();
        assert_eq!(bundle.stores_used, 1);
        let out = crate::provision(
            &testkit::file_url(&spec.out.join(crate::ARCHIVE_NAME)),
            &bundle.manifest.sha256,
            &dir.join("cache"),
        ).unwrap();
        assert_eq!(crate::bundled_store_dir(&out).unwrap(), Some(out.join(STORE_DIR)));
        let original = fs::read(spec.disc.join("maps/shared.map")).unwrap();
        assert_eq!(fs::read(out.join("maps/shared.map")).unwrap(), &original[..HEADER]);
        for name in ["lockout", "mainmenu", "single_player_shared"] {
            let path = format!("maps/{name}.map");
            assert_eq!(fs::read(out.join(&path)).unwrap(), fs::read(spec.disc.join(path)).unwrap());
        }

        let mut dvd = vfs::SimStoreBackend::new(vfs::open_disc(&out).unwrap(), out.join(STORE_DIR));
        let name = "maps\\shared.map";
        assert_eq!(dvd.stat(name).unwrap().end_of_file, original.len() as u64);
        let mut buf = [0; 8];
        assert_eq!(dvd.read_at(name, offset, &mut buf), Ok(8));
        assert_eq!(buf[..], original[offset as usize..offset as usize + 8]);
        assert_eq!(dvd.read_at(name, HEADER as u64, &mut buf), Err(vfs::Error::NotInStore));
    }

    #[test]
    fn changed_recorded_bytes_are_rejected_before_publishing() {
        let dir = TempDir::new("store-mismatch");
        let spec = fixture(&dir, &[(0, HEADER as u64 + 8)]);
        let file = spec.disc.join("maps/shared.map");
        let mut original = fs::read(&file).unwrap();
        original[HEADER] ^= 1;
        fs::write(&file, original).unwrap();
        assert!(crate::build(&spec).unwrap_err().to_string().contains("store differs"));
        assert!(!spec.out.join(crate::ARCHIVE_NAME).exists());
    }

    #[test]
    fn header_coverage_and_at_least_one_resource_store_are_required() {
        let dir = TempDir::new("store-header");
        let spec = fixture(&dir, &[(32, HEADER as u64 + 8)]);
        assert!(crate::build(&spec).unwrap_err().to_string().contains("cover the cache header"));
        fs::remove_file(spec.sim_stores.as_ref().unwrap().join("shared.h2store")).unwrap();
        assert!(crate::build(&spec).unwrap_err().to_string().contains("contains neither"));
    }
}
