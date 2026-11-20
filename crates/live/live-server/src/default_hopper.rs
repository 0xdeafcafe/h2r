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

use std::sync::OnceLock;

use live_proto::hopper::{HALO_2_HOPPER_FILE, HopperFile};
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

#[cfg(test)]
mod tests {
    use super::*;
    use live_proto::hopper::defaults::retail_map_ids;

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
