//! Contract 9 of docs/live/dedicated-host-build.md.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenParams {
    pub map: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub max_players: u8,
    pub live: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EmptyParams {}

pub type StateParams = EmptyParams;
pub type StartParams = EmptyParams;
pub type CloseParams = EmptyParams;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyResult {}

pub type StartResult = EmptyResult;
pub type CloseResult = EmptyResult;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Menu,
    Lobby,
    Loading,
    Playing,
    Postgame,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Slot in the session player mask. Two players on one console share a peer.
    pub slot: u32,
    pub peer: u32,
    pub name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StateResult {
    pub phase: Phase,
    pub players: Vec<Player>,
    pub ready: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn contract_open_requires_a_limit_and_explicit_network_choice() {
        let min = json!({"map": "lockout", "max_players": 1, "live": false});
        let p: OpenParams = from_value(min.clone()).unwrap();
        assert_eq!(p.variant, None);
        assert_eq!(to_value(p).unwrap(), min);
        for invalid in [
            json!({"map": "lockout", "max_players": 16}),
            json!({"map": "lockout", "live": true}),
            json!({"map": "lockout", "max_players": -1, "live": true}),
            json!({"map": "lockout", "max_players": 2, "live": true, "owner": 0}),
        ] {
            assert!(from_value::<OpenParams>(invalid).is_err());
        }
        assert_eq!(to_value(OpenResult::default()).unwrap(), json!({}));
        let live = OpenResult {
            session_id: Some("0123456789abcdef".into()),
        };
        assert_eq!(
            to_value(live).unwrap(),
            json!({"session_id": "0123456789abcdef"})
        );
    }

    #[test]
    fn contract_state_lists_players_individually_and_spells_all_five_phases() {
        let players = (0..2)
            .map(|slot| Player {
                slot,
                peer: 1,
                name: format!("player {slot}"),
            })
            .collect::<Vec<_>>();
        for (phase, name) in [
            (Phase::Menu, "menu"),
            (Phase::Lobby, "lobby"),
            (Phase::Loading, "loading"),
            (Phase::Playing, "playing"),
            (Phase::Postgame, "postgame"),
        ] {
            let state = StateResult {
                phase,
                players: players.clone(),
                ready: false,
            };
            let wire = json!({
                "phase": name,
                "players": [
                    {"slot": 0, "peer": 1, "name": "player 0"},
                    {"slot": 1, "peer": 1, "name": "player 1"}
                ],
                "ready": false
            });
            assert_eq!(to_value(&state).unwrap(), wire);
            assert_eq!(from_value::<StateResult>(wire).unwrap(), state);
        }
    }

    #[test]
    fn contract_empty_calls_have_object_payloads() {
        assert_eq!(to_value(StartParams::default()).unwrap(), json!({}));
        assert_eq!(to_value(CloseParams::default()).unwrap(), json!({}));
        assert_eq!(to_value(StateParams::default()).unwrap(), json!({}));
        assert_eq!(to_value(StartResult::default()).unwrap(), json!({}));
        assert_eq!(to_value(CloseResult::default()).unwrap(), json!({}));
        assert!(from_value::<EmptyParams>(json!({"force": true})).is_err());
    }
}
