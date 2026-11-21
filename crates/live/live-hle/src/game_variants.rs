//! Live game variants and rules scripts, read by the game
//! (docs/game/rules-engine.md, "Live game variants in the game"):
//!
//! - `GET /v1/variants?author=<xuid>`: the signed-in player's game
//!   variants ([`LiveVariant`]), for the custom-game variant list;
//! - `GET /v1/rules/{hash}/ir`: a rules IR blob by its full SHA-256, for a
//!   host whose variant names a script it does not have.
//!
//! Both go through the Live worker as the first account signed in on this
//! console, with a `read` token, and the caller waits a bounded time
//! ([`crate::Hle::fetch_rules_ir`], [`crate::Hle::my_game_variants`]).
//! Neither touches guest memory. The blob is returned as the server sent
//! it; the caller checks its hash and verifies it.

use serde::Deserialize;

/// The largest IR blob the server stores (docs/game/rules-engine.md,
/// "Limits"), and so the largest answer read.
pub const MAX_IR: usize = 64 * 1024;
/// The largest variant list read.
pub const MAX_LIST: usize = 1 << 20;

/// A variant's author as the server names it.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct Author {
    pub xuid: String,
    pub gamertag: String,
}

/// One Live game variant (`GET /v1/variants/{id}`): the record
/// (`record_hex`, 0x130 bytes as 520 hex digits, its `hx` block naming the
/// script by hash prefix) and the script's full hash.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct LiveVariant {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub author: Author,
    pub engine: i32,
    #[serde(default)]
    pub game_type: String,
    pub record_hex: String,
    #[serde(default)]
    pub script_hash: Option<String>,
    #[serde(default)]
    pub script_name: Option<String>,
    #[serde(default)]
    pub visibility: String,
    #[serde(default)]
    pub created: String,
}

/// `GET /v1/variants?author=`'s answer.
#[derive(Clone, Debug, Deserialize)]
pub(crate) struct VariantList {
    pub items: Vec<LiveVariant>,
}

/// Why a fetch has no answer, by name.
#[derive(Clone, Debug, PartialEq, Eq, thiserror::Error)]
pub enum FetchError {
    #[error("this machine is not connected to a Live server")]
    Offline,
    #[error("no player is signed in to Live")]
    NotSignedIn,
    #[error("the Live client is busy")]
    Busy,
    #[error("the Live server did not answer within {0} ms")]
    Timeout(u128),
    #[error("the Live server has no such script")]
    NotFound,
    #[error("the Live server refused: {0}")]
    Server(String),
    #[error("{0:?} is not a 64-digit lowercase SHA-256")]
    BadHash(String),
}

/// `/rules/{hash}/ir` under the API base.
pub fn ir_path(hash: &str) -> String {
    format!("/rules/{hash}/ir")
}

/// `/variants?author={xuid}` under the API base.
pub fn list_path(xuid: live_proto::Xuid) -> String {
    format!("/variants?author={xuid}")
}

/// Whether `hash` is 64 lowercase hex digits.
pub fn valid_hash(hash: &str) -> bool {
    hash.len() == 64
        && hash
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_and_hashes() {
        let h = "ab".repeat(32);
        assert!(valid_hash(&h));
        assert!(!valid_hash(&h.to_uppercase()));
        assert!(!valid_hash("abc"));
        assert_eq!(ir_path(&h), format!("/rules/{h}/ir"));
        assert_eq!(
            list_path(live_proto::Xuid(0x9000_0000_0000_0001)),
            "/variants?author=9000000000000001"
        );
    }

    #[test]
    fn a_variant_parses_as_the_server_sends_it() {
        let v: VariantList = serde_json::from_str(&format!(
            r#"{{"items":[{{"id":"v1","name":"Juggernaut Tag","description":"d",
            "author":{{"xuid":"9000000000000001","gamertag":"Alex"}},"engine":2,
            "game_type":"slayer","record_hex":"{}","script_hash":null,"script_name":null,
            "visibility":"public","created":"2026-11-21T00:00:00Z"}}]}}"#,
            "00".repeat(0x130)
        ))
        .unwrap();
        assert_eq!(v.items.len(), 1);
        assert_eq!(v.items[0].author.gamertag, "Alex");
        assert_eq!(v.items[0].script_hash, None);
    }
}
