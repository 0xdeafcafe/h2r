//! Tickets and host assignments for the OptiMatch QoS queue.

use crate::Xuid;
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, net::SocketAddrV4};

pub const MATCH_HEADER: &str = "x-h2-match";
pub const MAX_CANDIDATES: usize = 32;
pub const ASSIGNMENT_SECONDS: i64 = 60;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Nat {
    Open,
    Moderate,
    Strict,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Connectivity {
    pub nat: Option<Nat>,
    pub upload_kbps: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ticket {
    /// OptiMatch hopper index.
    pub hopper: u32,
    pub port: u16,
    #[serde(default)]
    pub dedicated: bool,
    #[serde(default)]
    pub connectivity: Connectivity,
    #[serde(default)]
    pub samples: Vec<Sample>,
}

/// An observed game endpoint, with an opaque generation for this ticket.
/// The client probes base+2 using the existing QoS echo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Candidate {
    pub xuid: Xuid,
    pub endpoint: SocketAddrV4,
    pub generation: String,
}

/// An authenticated WebSocket echo on the signed-in Live origin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionProbe {
    pub region: String,
    pub xuid: Xuid,
    pub generation: String,
}

pub fn valid_probe_region(region: &str) -> bool {
    (1..=24).contains(&region.len())
        && region.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
}

// Bounds parallel upgrades per poll, separately from the ticket's total targets.
pub const MAX_REGION_ECHOES: usize = 9;
pub const REGION_ECHO: &str = "h2-region-qos";
/// Internal tunnel route. The matching subprotocol proves the image serves
/// only the echo here before Live passes its socket to a searcher.
pub const PACK_ECHO_PATH: &str = "/internal/region-echo";

pub fn echo_region(path: &str) -> Option<&str> {
    let region = path.strip_prefix("/v1/hosts/regions/")?.strip_suffix("/echo")?;
    valid_probe_region(region).then_some(region)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub target: Xuid,
    pub generation: String,
    /// Null means a probe was sent and timed out.
    pub rtt_ms: Option<u16>,
}

// dh-lease: a lease ID and telemetry source extend the queue wire types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Assignment {
    pub id: String,
    pub hopper: u32,
    pub host: Xuid,
    pub dedicated: bool,
    pub members: Vec<Xuid>,
    /// Omitted for free-for-all. Team IDs are zero-based.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub teams: BTreeMap<Xuid, u8>,
    /// Member -> selected host, in milliseconds. A peer host's own entry is zero.
    pub rtt_ms: BTreeMap<Xuid, u16>,
    /// Omitted unless an ad hoc dedicated host serves this assignment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_lease: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Queue {
    Probing {
        candidates: Vec<Candidate>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        region_echoes: Vec<RegionProbe>,
    },
    Host { assignment: Assignment },
    Waiting { assignment: Assignment },
    Join {
        session_id: String,
        /// The dedicated host supplies the match's telemetry frames.
        #[serde(default)]
        frames_from_host: bool,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn assignment_host_lease_round_trips_and_is_optional() {
        let mut wire = json!({
            "id": "0123456789abcdef",
            "hopper": 1,
            "host": "0009000000000001",
            "dedicated": true,
            "members": ["0009000000000002"],
            "rtt_ms": {"0009000000000002": 35},
        });
        let assignment: Assignment = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(assignment.host_lease, None);
        assert_eq!(serde_json::to_value(&assignment).unwrap(), wire);

        wire["host_lease"] = json!("hl-AAAAAAAAAAAAAAAAAAAAAA");
        let with_lease = Assignment {
            host_lease: Some("hl-AAAAAAAAAAAAAAAAAAAAAA".into()),
            ..assignment
        };
        assert_eq!(serde_json::to_value(&with_lease).unwrap(), wire);
        assert_eq!(
            serde_json::from_value::<Assignment>(wire).unwrap(),
            with_lease
        );
    }

    #[test]
    fn regional_echo_path_contains_one_validated_label() {
        assert_eq!(echo_region("/v1/hosts/regions/weur/echo"), Some("weur"));
        for code in ["", "../lhr", "x%2fy", "?q", "a:b", "x/y", "a_1"] {
            assert!(echo_region(&format!("/v1/hosts/regions/{code}/echo")).is_none());
        }
        let legacy: Queue = serde_json::from_value(json!({"state":"probing","candidates":[]})).unwrap();
        assert!(matches!(legacy, Queue::Probing { region_echoes, .. } if region_echoes.is_empty()));
    }

    #[test]
    fn queue_join_frame_source_round_trips_and_defaults_to_false() {
        let id = "0123456789abcdef";
        for (wire_value, expected) in [(None, false), (Some(false), false), (Some(true), true)] {
            let mut wire = json!({ "state": "join", "session_id": id });
            if let Some(frames_from_host) = wire_value {
                wire["frames_from_host"] = json!(frames_from_host);
            }
            let join: Queue = serde_json::from_value(wire.clone()).unwrap();
            let Queue::Join {
                ref session_id,
                frames_from_host,
            } = join
            else {
                panic!("expected a join reply");
            };
            assert_eq!(session_id.as_str(), id);
            assert_eq!(frames_from_host, expected);
            wire["frames_from_host"] = json!(expected);
            assert_eq!(serde_json::to_value(join).unwrap(), wire);
        }
    }
}
