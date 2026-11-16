//! Cloud saves: an account's private, versioned item store
//! (docs/live/live-cloud-saves.md, docs/live/live-api.md "Cloud saves").
//!
//! An item is `(kind, name)`: a file of the account's save storage. Each
//! write gives it the next version, and its bytes are named by their
//! SHA-256. Writes say which version they replace (`If-Match`), so two
//! machines cannot overwrite each other's changes unseen. Only the owner
//! reads or writes them, even on a public server.

use serde::{Deserialize, Serialize};

use crate::maps::Sha256;

/// The U: drive's files: XAPI save containers (profiles, saved game
/// variants, campaign saves) and the title's own files at the root of U:.
pub const KIND_SAVE: &str = "save";
/// Our per-profile settings (`[profiles."<name>"]` in settings.toml),
/// one item per profile.
pub const KIND_SETTINGS: &str = "settings";
/// Every kind the server stores.
pub const KINDS: [&str; 2] = [KIND_SAVE, KIND_SETTINGS];

/// The header carrying the version a write replaces: the item's current
/// version (a deleted one's too), or `0` when it has never existed.
pub const IF_MATCH: &str = "if-match";
/// The SHA-256 the writer computed over the body; the server refuses a
/// body that hashes to anything else.
pub const SHA256_HEADER: &str = "x-h2-sha256";
/// When the writer last changed the item: Unix milliseconds, its own clock.
/// Conflict resolution compares these (docs/live/live-cloud-saves.md).
pub const MODIFIED_HEADER: &str = "x-h2-modified";
/// The version of the item a download or write answers with.
pub const VERSION_HEADER: &str = "x-h2-version";

/// The most path segments a name has (`<save directory>/<file>` needs two).
pub const MAX_SEGMENTS: usize = 4;
/// The longest segment: FATX's 42-byte name limit, with room for settings
/// items, whose names are hex.
pub const MAX_SEGMENT: usize = 64;
/// The longest name, as the VFS limits a volume-relative path.
pub const MAX_NAME: usize = 250;

/// Whether `kind` is one the server stores.
pub fn valid_kind(kind: &str) -> bool {
    KINDS.contains(&kind)
}

/// Item names: `/`-separated segments of printable ASCII without the
/// characters FATX refuses (`" * + , / : ; < = > ? \ |`), no `.` or `..`,
/// at most [`MAX_SEGMENTS`] of at most [`MAX_SEGMENT`] bytes each.
pub fn valid_name(name: &str) -> bool {
    if name.is_empty() || name.len() > MAX_NAME {
        return false;
    }
    let segments: Vec<&str> = name.split('/').collect();
    segments.len() <= MAX_SEGMENTS
        && segments.iter().all(|s| {
            !s.is_empty()
                && s.len() <= MAX_SEGMENT
                && *s != "."
                && *s != ".."
                && s.bytes().all(|b| {
                    (0x20..0x7f).contains(&b) && !b"\"*+,/:;<=>?\\|%".contains(&b)
                })
        })
}

/// Percent-encodes one name for a URL path: segments stay separated by
/// `/`, and anything but unreserved characters is escaped.
pub fn encode_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~' | b'/') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// One item as the server lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageItem {
    pub kind: String,
    pub name: String,
    /// 1 for the first write, one more for each write or delete after it.
    pub version: u64,
    /// The bytes' SHA-256; absent once deleted.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<Sha256>,
    pub size: u64,
    /// The writer's own time of the change, Unix milliseconds.
    pub modified: i64,
    /// The server's time of the write, RFC 3339.
    pub stored_at: String,
    /// A delete leaves a tombstone, so other machines learn of it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deleted: bool,
}

/// `GET /v1/accounts/{xuid}/storage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StorageList {
    pub items: Vec<StorageItem>,
    /// Bytes counted against the quota: every live item's size.
    pub used_bytes: u64,
    pub quota_bytes: u64,
    pub item_bytes: u64,
}

/// A 409 on a write: the version it named is not the current one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VersionConflict {
    pub error: crate::ErrorCode,
    pub message: String,
    /// The item as it is now; `None` if it never existed.
    #[serde(default)]
    pub current: Option<StorageItem>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names() {
        for ok in [
            "4D7B3C2A11F0/SaveMeta.xbx",
            "TitleMeta.xbx",
            "6368696566",
            "a b/c (1)",
            "a/b/c/d",
        ] {
            assert!(valid_name(ok), "{ok}");
        }
        for bad in [
            "",
            "/a",
            "a/",
            "a//b",
            "a/../b",
            ".",
            "a\\b",
            "a:b",
            "a/b/c/d/e",
            "100%",
            "caf\u{e9}",
            &"x".repeat(65),
        ] {
            assert!(!valid_name(bad), "{bad:?}");
        }
        assert!(valid_kind("save") && valid_kind("settings") && !valid_kind("maps"));
    }

    #[test]
    fn encoding_keeps_segments() {
        assert_eq!(encode_name("ABC/Save Meta.xbx"), "ABC/Save%20Meta.xbx");
        assert_eq!(encode_name("a(1)&b"), "a%281%29%26b");
    }
}
