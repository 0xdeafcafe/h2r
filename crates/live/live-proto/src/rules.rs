//! Saved game-type scripts (docs/live/live-api.md "Rules scripts",
//! docs/game/rules-engine.md "Workbench"): a script that verifies is kept
//! by its IR's SHA-256, as a server-signed [`RULES_SAVED`] event on the
//! script's [`stream`]. The server compiles the source again on read, so
//! the event holds the source alone.
use serde::{Deserialize, Serialize};

use crate::Xuid;

/// `RulesSaved`: an account saved a script that verifies (server-signed).
/// Data: [`Saved`].
pub const RULES_SAVED: &str = "RulesSaved";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 1] = [RULES_SAVED];

/// The prefix of every script's stream.
pub const STREAM_PREFIX: &str = "rules-";

/// The stream a script is saved on: `rules-<hash>`.
pub fn stream(hash: &str) -> String {
    format!("{STREAM_PREFIX}{hash}")
}

/// The most source bytes a script holds (the compiler's own cap).
pub const MAX_SOURCE: usize = 256 * 1024;

/// The most scripts one account saves.
pub const MAX_PER_AUTHOR: usize = 200;

/// Whether `s` is 64 lowercase hex digits.
pub fn valid_hash(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `RulesSaved` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Saved {
    /// The IR's SHA-256, lowercase hex.
    pub hash: String,
    /// The script's `rules "<name>"`.
    pub name: String,
    pub version: u16,
    /// Who saved it first.
    pub author: Xuid,
    pub source: String,
}

/// One script in `GET /v1/rules?author=`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptSummary {
    pub hash: String,
    pub name: String,
    pub version: u16,
    pub author: Xuid,
    #[serde(default)]
    pub gamertag: String,
    /// When it was saved (RFC 3339).
    pub saved_at: String,
    pub position: u64,
}

/// `GET /v1/rules/{hash}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Script {
    #[serde(flatten)]
    pub summary: ScriptSummary,
    pub source: String,
}

/// `GET /v1/rules?author=`: newest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScriptList {
    pub items: Vec<ScriptSummary>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_and_streams() {
        let h = "ab".repeat(32);
        assert!(valid_hash(&h));
        assert!(!valid_hash(&"AB".repeat(32)));
        assert!(!valid_hash("ab"));
        assert_eq!(stream(&h), format!("rules-{h}"));
    }

    #[test]
    fn wire_shapes() {
        let s = Script {
            summary: ScriptSummary {
                hash: "00".repeat(32),
                name: "Slayer".into(),
                version: 1,
                author: Xuid(0x0009_0000_0000_0001),
                gamertag: "Pete".into(),
                saved_at: "2026-11-21T00:00:00Z".into(),
                position: 4,
            },
            source: "rules \"Slayer\" 1".into(),
        };
        let v = serde_json::to_value(&s).unwrap();
        assert_eq!(v["name"], "Slayer");
        assert_eq!(v["source"], "rules \"Slayer\" 1");
        assert_eq!(v["author"], "0009000000000001");
        let back: Script = serde_json::from_value(v).unwrap();
        assert_eq!(back, s);
    }
}
