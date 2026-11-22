//! Published game variants (docs/live/live-api.md "Game variants"): a
//! game variant is a whole `.h2variant` file (`variants::portable`,
//! version 2), kept by the SHA-256 of its canonical bytes as a
//! server-signed [`PUBLISHED`] event on the variant's [`stream`], and taken
//! down by its author with a [`DELETED`] tombstone on the same stream.
use serde::{Deserialize, Serialize};

use crate::Xuid;

/// `VariantPublished`: an account published a game variant
/// (server-signed). Data: [`Published`].
pub const PUBLISHED: &str = "VariantPublished";
/// `VariantDeleted`: its author (or an admin) took it down. Data:
/// [`Deleted`].
pub const DELETED: &str = "VariantDeleted";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 2] = [PUBLISHED, DELETED];

/// The prefix of every variant's stream.
pub const STREAM_PREFIX: &str = "variant-";

/// The stream a variant is kept on: `variant-<id>`.
pub fn stream(id: &str) -> String {
    format!("{STREAM_PREFIX}{id}")
}

/// The most published variants one account keeps (deleted ones do not
/// count).
pub const MAX_PER_AUTHOR: usize = 200;

/// Who may see a variant in lists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Visibility {
    /// Listed on the author's profile.
    #[default]
    Public,
    /// Readable by id; listed only to its author.
    Unlisted,
}

/// `VariantPublished` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    /// The SHA-256 of `bundle`, lowercase hex.
    pub id: String,
    pub author: Xuid,
    pub visibility: Visibility,
    /// The `.h2variant` file, canonical JSON
    /// (`variants::portable::Bundle::canonical`).
    pub bundle: String,
}

/// `VariantDeleted` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Deleted {
    pub id: String,
    pub by: Xuid,
}

/// A variant's author.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Author {
    pub xuid: Xuid,
    #[serde(default)]
    pub gamertag: String,
}

/// One row of a variant's settings summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Setting {
    pub label: String,
    pub value: String,
}

/// A published variant, as `GET /v1/variants/{id}` and the lists give it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Variant {
    pub id: String,
    pub name: String,
    pub description: String,
    pub author: Author,
    /// The record's retail game-engine index.
    pub engine: i32,
    /// Our label of that game type.
    pub game_type: String,
    /// The 0x130-byte record, lowercase hex.
    pub record_hex: String,
    pub script_hash: Option<String>,
    pub script_name: Option<String>,
    pub visibility: Visibility,
    /// When it was published (RFC 3339).
    pub created: String,
    /// `GET` this for the `.h2variant` file.
    pub bundle_url: String,
    /// The record's settings, in words.
    #[serde(default)]
    pub settings: Vec<Setting>,
    /// The kinds of game the variant may play in, by the script policy
    /// (docs/game/rules-engine.md, "Roles, powers and capabilities"):
    /// `custom`, `hopper` and `ranked`, in that order, those its script's
    /// capabilities allow. Every kind with no script.
    #[serde(default)]
    pub eligible: Vec<String>,
}

/// `GET /v1/variants?author=`: newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VariantList {
    pub items: Vec<Variant>,
}

/// Whether `s` is a variant id: 64 lowercase hex digits.
pub fn valid_id(s: &str) -> bool {
    crate::rules::valid_hash(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn streams_and_visibility() {
        let id = "ab".repeat(32);
        assert!(valid_id(&id));
        assert_eq!(stream(&id), format!("variant-{id}"));
        assert_eq!(serde_json::to_value(Visibility::Unlisted).unwrap(), "unlisted");
        let v: Visibility = serde_json::from_value("public".into()).unwrap();
        assert_eq!(v, Visibility::Public);
    }
}
