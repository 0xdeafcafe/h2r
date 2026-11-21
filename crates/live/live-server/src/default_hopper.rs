//! The playlists served when no `multiplayer_hopper.dat` was ever published
//! for Halo 2 (docs/live/live-admin.md §2): the built-in lineup from
//! [`HopperFile::default_set`], built from the format alone, so a server
//! offers matchmaking with no imported file.
//!
//! Nothing is written to the log. [`Live::file`](crate::app::Live::file)
//! answers with these bytes while the title has no file of that name, so
//! the game's title-storage download, the admin JSON view and its edits all
//! see them; the first edit from the web builder, a `h2 live hopper import`
//! or any other publish replaces them, and from then on the published file
//! is served as before.

use std::sync::{Arc, Mutex, OnceLock};

use live_proto::hopper::{HALO_2_HOPPER_FILE, HopperError, HopperFile};
use live_proto::{HALO_2_TITLE_ID, PublishedFile, TitleId, sign};
use sha2::{Digest, Sha256};

/// The default file and its metadata, built once.
pub fn file() -> &'static (PublishedFile, Vec<u8>) {
    static FILE: OnceLock<(PublishedFile, Vec<u8>)> = OnceLock::new();
    FILE.get_or_init(|| {
        let bytes = HopperFile::default_set()
            .build_checked()
            .expect("the default set fits the game's buffer");
        let meta = PublishedFile {
            title_id: HALO_2_TITLE_ID,
            name: HALO_2_HOPPER_FILE.into(),
            sha256: sign::hex(&Sha256::digest(&bytes)),
            size: bytes.len(),
        };
        (meta, bytes)
    })
}

/// The default for `(title, name)` when it is Halo 2's hopper file.
pub fn fallback(title: TitleId, name: &str) -> Option<(PublishedFile, Vec<u8>)> {
    (title == HALO_2_TITLE_ID && name == HALO_2_HOPPER_FILE).then(|| file().clone())
}

struct CompatibilityFile {
    source: String,
    projected: Option<(String, Vec<u8>)>,
}

/// A title-file client without the rotation hook gets a disc-only copy.
/// The canonical file remains available to the editor and capable clients.
pub fn compatibility_download(hash: String, bytes: Vec<u8>) -> Result<(String, Vec<u8>), HopperError> {
    static CACHE: Mutex<Option<Arc<CompatibilityFile>>> = Mutex::new(None);
    let cached = CACHE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .as_ref()
        .filter(|cached| cached.source == hash)
        .cloned();
    if let Some(chosen) = cached {
        return Ok(chosen.projected.clone().unwrap_or((hash, bytes)));
    }

    // Inflation and rebuild happen outside the lock. Cache a negative
    // result too: an unrecognised payload passes through to the game,
    // which already falls back to its defaults.
    let projected = match HopperFile::parse_legacy(&bytes) {
        Ok(mut hopper) => {
            if hopper.retain_base_maps()? {
                let bytes = hopper.build_checked()?;
                Some((sign::hex(&Sha256::digest(&bytes)), bytes))
            } else {
                None
            }
        }
        _ => None,
    };
    let reply = projected.clone().unwrap_or((hash.clone(), bytes));
    *CACHE.lock().unwrap_or_else(|e| e.into_inner()) =
        Some(Arc::new(CompatibilityFile { source: hash, projected }));
    Ok(reply)
}

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::hopper::defaults::retail_map_ids;

    #[test]
    fn legacy_hopper_download_keeps_the_base_map_and_has_its_own_etag() {
        let mut rotation = HopperFile::default_set();
        let base = rotation.entry(0, 0).unwrap();
        let mut addon = base.clone();
        addon.map_id = 0x12_345;
        rotation.set_entries(0, &[addon, base.clone()]).unwrap();
        let raw = rotation.build_checked().unwrap();
        let hash = sign::hex(&Sha256::digest(&raw));
        let (etag, bytes) = compatibility_download(hash.clone(), raw.clone()).unwrap();
        assert_ne!(etag, hash);
        assert_eq!(etag, sign::hex(&Sha256::digest(&bytes)));
        assert_eq!(HopperFile::parse(&bytes).unwrap().entries(0).unwrap(), vec![base]);
        assert_eq!(compatibility_download(hash, raw).unwrap(), (etag, bytes));
    }

    #[test]
    fn the_default_is_a_file_the_game_takes() {
        let (meta, bytes) = file();
        assert_eq!(meta.size, bytes.len());
        assert!(bytes.len() <= live_proto::hopper::MAX_FILE);
        let parsed = HopperFile::parse(bytes).unwrap();
        assert_eq!(parsed.problems(&retail_map_ids()), vec![]);
        assert!(fallback(HALO_2_TITLE_ID, HALO_2_HOPPER_FILE).is_some());
        assert!(fallback(HALO_2_TITLE_ID, "message_of_the_day.dat").is_none());
        let other = TitleId::parse("4d530065").unwrap();
        assert!(fallback(other, HALO_2_HOPPER_FILE).is_none());
    }
}
