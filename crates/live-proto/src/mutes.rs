//! The mute list (docs/live/live-voice.md §3.6, docs/live/live-api.md
//! "Mute list"): the players an account has muted, kept by the server so
//! the list follows the player between machines. The game builds its voice
//! mute matrix from it (`XOnlineMutelistGet`), and a mute holds both ways.
//! Both events are server-signed, on the muting account's [`stream`]; only
//! that account, admins and the operator read them.
use serde::{Deserialize, Serialize};

use crate::Xuid;

/// `PlayerMuted`: an account muted another (server-signed). Data: [`Muted`].
pub const PLAYER_MUTED: &str = "PlayerMuted";
/// `PlayerUnmuted`: an account took a mute back (server-signed). Data:
/// [`Muted`].
pub const PLAYER_UNMUTED: &str = "PlayerUnmuted";

/// Every event type of this module; only the server's own endpoints write
/// them.
pub const KINDS: [&str; 2] = [PLAYER_MUTED, PLAYER_UNMUTED];

/// The most players one account mutes: the size of the game's
/// `XOnlineMutelistGet` buffer.
pub const MAX: usize = 250;

/// The stream an account's mutes are on.
pub fn stream(by: Xuid) -> String {
    format!("mutes-{by}")
}

/// `PlayerMuted` and `PlayerUnmuted` data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Muted {
    pub by: Xuid,
    pub target: Xuid,
}

/// One entry of `GET /v1/accounts/by-xuid/{xuid}/mutes`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuteEntry {
    pub xuid: Xuid,
    #[serde(default)]
    pub gamertag: String,
}

/// `GET /v1/accounts/by-xuid/{xuid}/mutes`: oldest first.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MuteList {
    pub items: Vec<MuteEntry>,
    #[serde(default)]
    pub position: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wire_shapes() {
        let m = Muted {
            by: Xuid(0x0009_0000_0000_0001),
            target: Xuid(0x0009_0000_0000_0002),
        };
        assert_eq!(
            serde_json::to_value(m).unwrap(),
            serde_json::json!({"by": "0009000000000001", "target": "0009000000000002"})
        );
        assert_eq!(stream(m.by), "mutes-0009000000000001");
        let l: MuteList =
            serde_json::from_str(r#"{"items":[{"xuid":"0009000000000002"}]}"#).unwrap();
        assert_eq!(l.items[0].xuid, m.target);
    }
}
