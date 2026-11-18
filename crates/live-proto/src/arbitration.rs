//! Ranked-game arbitration (docs/live/live-matchmaking.md §8.4): each
//! console registers the round it plays (`XOnlineArbitrationRegister`) and
//! reports it at the end (`XOnlineArbitrationReport`). The server writes
//! one event per account and call on the round's [`stream`]; nobody else
//! writes them. A ranked game ranks only the humans whose own account
//! registered its round.
use serde::{Deserialize, Serialize};

use crate::Xuid;

/// `ArbitrationRegistered`: an account registered a round. Data: [`Registered`].
pub const REGISTERED: &str = "ArbitrationRegistered";
/// `ArbitrationExtended`: an account extended a round. Data: [`Extended`].
pub const EXTENDED: &str = "ArbitrationExtended";
/// `ArbitrationReported`: an account reported a round. Data: [`Reported`].
pub const REPORTED: &str = "ArbitrationReported";
/// `ArbitrationUsed`: the server ranked a game by the round (moving
/// someone or not); the round ranks no other game. Data: [`Used`].
pub const USED: &str = "ArbitrationUsed";

/// Every event type of this module; only the server writes them.
pub const KINDS: [&str; 4] = [REGISTERED, EXTENDED, REPORTED, USED];

/// The most stat procedures in one report (the XDK's limit).
pub const MAX_PROCEDURES: usize = 1000;

/// The stream a round's events are on.
pub fn stream(session_id: &str, round_id: &str) -> String {
    format!("round-{session_id}-{round_id}")
}

/// A session or round id as the API writes it: the game's 8 bytes in
/// order, as 16 lowercase hex digits (`sign::hex`).
pub fn valid_id(s: &str) -> bool {
    s.len() == 16
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// `POST /v1/arbitration/{session}/{round}/register`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Register {
    /// The duration-like word the game passes (not yet decoded).
    pub duration: u32,
    pub flags: u32,
}

/// `POST /v1/arbitration/{session}/{round}/extend`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extend {
    pub duration: u32,
}

/// `POST /v1/arbitration/{session}/{round}/report`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Report {
    pub flags: u32,
    /// Each stat procedure's id (0x8001, 0x8003, 0x8007 seen); their data
    /// is not decoded yet.
    pub procedures: Vec<u16>,
}

/// `ArbitrationRegistered` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registered {
    pub session_id: String,
    pub round_id: String,
    pub xuid: Xuid,
    pub duration: u32,
    pub flags: u32,
}

/// `ArbitrationExtended` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Extended {
    pub session_id: String,
    pub round_id: String,
    pub xuid: Xuid,
    pub duration: u32,
}

/// `ArbitrationReported` data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reported {
    pub session_id: String,
    pub round_id: String,
    pub xuid: Xuid,
    pub flags: u32,
    pub procedures: Vec<u16>,
}

/// [`USED`]'s data.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Used {
    pub session_id: String,
    pub round_id: String,
    pub game_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_sixteen_lowercase_hex_digits() {
        assert!(valid_id("0123456789abcdef"));
        assert!(!valid_id("0123456789ABCDEF"));
        assert!(!valid_id("0123456789abcde"));
        assert!(!valid_id("0123456789abcdeg"));
        assert_eq!(
            stream("0123456789abcdef", "fedcba9876543210"),
            "round-0123456789abcdef-fedcba9876543210"
        );
    }
}
