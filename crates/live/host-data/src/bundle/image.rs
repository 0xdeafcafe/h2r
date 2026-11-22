//! The Linux game module a dedicated host runs: `libh2game.so` and its
//! `.stamp`, compiled from the owner's disc (`deploy/host/build.sh
//! --module`), as `image/` in the archive. The host checks the stamp
//! against its own h2 before it starts a game (docs/live/dedicated-host.md,
//! Game data bundle). The module is the owner's: it never goes into the
//! container image or the repository.

use std::{fs, path::Path};

use super::{Payload, Source};
use crate::{Error, IMAGE_DIR, IMAGE_FILES};

/// Add the module and its stamp in `dir` to `payload`. Both must be
/// non-empty regular files (a symlink to one is followed).
pub(super) fn stage(dir: &Path, payload: &mut Vec<Payload>) -> Result<(), Error> {
    for name in IMAGE_FILES {
        let path = dir.join(name);
        let metadata =
            fs::metadata(&path).map_err(|e| Error::Bundle(format!("{}: {e}", path.display())))?;
        if !metadata.is_file() || metadata.len() == 0 {
            return Err(Error::Bundle(format!(
                "{} must be a non-empty file; build the module with deploy/host/build.sh --module",
                path.display()
            )));
        }
        payload.push(Payload {
            path: format!("{IMAGE_DIR}/{name}"),
            size: metadata.len(),
            source: Source::File(path),
            sha256: String::new(),
        });
    }
    payload.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::testkit::{self, TempDir};
    use crate::{ARCHIVE_NAME, BundleSpec, MANIFEST_NAME, Manifest};

    const MODULE: &[u8] = b"\x7fELF synthetic host module";
    const STAMP: &[u8] = b"xbe 0000000000000000 generator test host coverage 0000000000000000\n";

    fn spec(tmp: &TempDir, image: Option<std::path::PathBuf>) -> BundleSpec {
        let disc = tmp.join("disc");
        if !disc.exists() {
            testkit::write_disc_dir(&disc);
        }
        BundleSpec {
            disc,
            out: tmp.join("bundle"),
            stripped: None,
            sim_stores: None,
            bots: None,
            image,
            force: true,
            zstd_level: 1,
        }
    }

    fn write_image(dir: &std::path::Path) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(dir.join("libh2game.so"), MODULE).unwrap();
        std::fs::write(dir.join("libh2game.so.stamp"), STAMP).unwrap();
    }

    #[test]
    fn the_module_and_its_stamp_travel_as_image_entries_in_the_manifest() {
        let tmp = TempDir::new("image-pack");
        let image = tmp.join("image-out");
        write_image(&image);
        let built = crate::build(&spec(&tmp, Some(image))).unwrap();
        assert!(built.image);

        // The owner's manifest beside the archive keeps its shape: the
        // archive digest and one entry per file, the module among them.
        let manifest: Manifest =
            serde_json::from_slice(&testkit::read_all(&tmp.join("bundle").join(MANIFEST_NAME)))
                .unwrap();
        assert_eq!(manifest, built.manifest);
        for (path, bytes) in [("image/libh2game.so", MODULE), ("image/libh2game.so.stamp", STAMP)] {
            let entry = manifest.files.iter().find(|f| f.path == path).unwrap();
            assert_eq!(entry.size, bytes.len() as u64, "{path}");
            assert_eq!(entry.sha256, testkit::sha256_hex(bytes), "{path}");
        }
        let paths: Vec<&str> = manifest.files.iter().map(|f| f.path.as_str()).collect();
        let mut sorted = paths.clone();
        sorted.sort_unstable();
        assert_eq!(paths, sorted);

        // A host verifies and installs it, and finds the module directory.
        let dir = crate::provision(
            &testkit::file_url(&tmp.join("bundle").join(ARCHIVE_NAME)),
            &manifest.sha256,
            &tmp.join("cache"),
        )
        .unwrap();
        let installed = crate::bundled_image_dir(&dir).unwrap().unwrap();
        assert_eq!(installed, dir.join("image"));
        assert_eq!(testkit::read_all(&installed.join("libh2game.so")), MODULE);
        assert_eq!(testkit::read_all(&installed.join("libh2game.so.stamp")), STAMP);
    }

    #[test]
    fn a_bundle_without_the_module_names_no_image_dir() {
        let tmp = TempDir::new("image-none");
        let built = crate::build(&spec(&tmp, None)).unwrap();
        assert!(!built.image);
        assert!(!built.manifest.files.iter().any(|f| f.path.starts_with("image/")));
        let dir = crate::provision(
            &testkit::file_url(&tmp.join("bundle").join(ARCHIVE_NAME)),
            &built.manifest.sha256,
            &tmp.join("cache"),
        )
        .unwrap();
        assert_eq!(crate::bundled_image_dir(&dir).unwrap(), None);
    }

    #[test]
    fn a_module_without_its_stamp_is_refused_at_build() {
        let tmp = TempDir::new("image-nostamp");
        let image = tmp.join("image-out");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("libh2game.so"), MODULE).unwrap();
        let error = crate::build(&spec(&tmp, Some(image.clone()))).unwrap_err();
        assert!(error.to_string().contains("libh2game.so.stamp"), "{error}");

        std::fs::write(image.join("libh2game.so.stamp"), b"").unwrap();
        let error = crate::build(&spec(&tmp, Some(image))).unwrap_err();
        assert!(error.to_string().contains("non-empty"), "{error}");
    }

    #[test]
    fn only_the_module_and_its_stamp_are_image_payload() {
        for ok in ["image/libh2game.so", "image/libh2game.so.stamp"] {
            assert!(crate::validate_payload_path(ok).is_ok(), "{ok}");
        }
        for bad in [
            "image/libh2game.dylib",
            "image/other.so",
            "image/sub/libh2game.so",
            "image/",
            "imagex/libh2game.so",
            "libh2game.so",
        ] {
            assert!(crate::validate_payload_path(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_manifest_with_half_a_module_is_corrupt() {
        let tmp = TempDir::new("image-half");
        let image = tmp.join("image-out");
        write_image(&image);
        crate::build(&spec(&tmp, Some(image))).unwrap();
        let dir = tmp.join("installed");
        std::fs::create_dir_all(&dir).unwrap();
        let mut manifest: Manifest =
            serde_json::from_slice(&testkit::read_all(&tmp.join("bundle").join(MANIFEST_NAME)))
                .unwrap();
        manifest.sha256.clear();
        manifest.files.retain(|f| f.path != "image/libh2game.so.stamp");
        std::fs::write(dir.join(MANIFEST_NAME), serde_json::to_vec(&manifest).unwrap()).unwrap();
        let error = crate::bundled_image_dir(&dir).unwrap_err();
        assert!(matches!(&error, crate::Error::Corrupt(m) if m.contains("or neither")), "{error}");
    }

    #[test]
    fn bundles_with_and_without_the_module_share_the_cache_by_digest() {
        // An old bundle (no module) and a new one (with it) sit side by
        // side under their own digests, so a packed host mid-pack keeps
        // the match it started on either.
        let tmp = TempDir::new("image-side-by-side");
        let cache = tmp.join("cache");
        let opts = crate::Options { max_cache_bytes: u64::MAX };
        let old = TempDir::new("image-old");
        let old_built = crate::build(&spec(&old, None)).unwrap();
        let new = TempDir::new("image-new");
        let image = new.join("image-out");
        write_image(&image);
        let new_built = crate::build(&spec(&new, Some(image))).unwrap();
        assert_ne!(old_built.manifest.sha256, new_built.manifest.sha256);

        let old_url = testkit::file_url(&old.join("bundle").join(ARCHIVE_NAME));
        let new_url = testkit::file_url(&new.join("bundle").join(ARCHIVE_NAME));
        let old_dir =
            crate::provision_with(&old_url, &old_built.manifest.sha256, &cache, &opts).unwrap();
        let new_dir =
            crate::provision_with(&new_url, &new_built.manifest.sha256, &cache, &opts).unwrap();
        assert_eq!(old_dir, cache.join(&old_built.manifest.sha256));
        assert_eq!(new_dir, cache.join(&new_built.manifest.sha256));
        // Provisioning the old one again reuses its entry; neither evicted.
        assert_eq!(
            crate::provision_with(&old_url, &old_built.manifest.sha256, &cache, &opts).unwrap(),
            old_dir
        );
        assert_eq!(crate::bundled_image_dir(&old_dir).unwrap(), None);
        assert_eq!(crate::bundled_image_dir(&new_dir).unwrap(), Some(new_dir.join("image")));
    }
}
