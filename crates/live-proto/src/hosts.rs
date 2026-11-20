//! Dedicated host leases and server-signed lifecycle events on the hosts stream.

use crate::{Xuid, sign::random_id};
use serde::{Deserialize, Serialize};

pub const STREAM: &str = "hosts";
pub const HOST_REQUESTED: &str = "HostRequested";
pub const HOST_BOOTING: &str = "HostBooting";
pub const HOST_READY: &str = "HostReady";
pub const HOST_PLAYING: &str = "HostPlaying";
pub const HOST_ENDED: &str = "HostEnded";
pub const HOST_STOPPED: &str = "HostStopped";
pub const KINDS: [&str; 6] = [
    HOST_REQUESTED,
    HOST_BOOTING,
    HOST_READY,
    HOST_PLAYING,
    HOST_ENDED,
    HOST_STOPPED,
];

pub const DEFAULT_IDLE_TIMEOUT_S: u32 = 120;
pub const DEFAULT_MAX_MATCH_S: u32 = 1800;

/// The lease metadata shown on public game and session pages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DedicatedHostFields {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_lease: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dedicated_region: Option<String>,
}

/// A reservation for one match on a dedicated process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostLease {
    /// An opaque ID starting with `hl-`.
    pub id: String,
    /// A matchmaking assignment ID, or `sim-<id>` or `custom-<id>`.
    pub assignment_id: String,
    pub provider: ProviderKind,
    /// A provider region code, e.g. `lhr` or `enam`.
    pub region: Option<String>,
    pub class: HostClass,
    /// The scenario name.
    pub map: String,
    /// The mod package hash. Retail maps have none.
    pub map_id: Option<String>,
    pub variant: Option<String>,
    pub hopper: Option<u32>,
    /// Total seats, from 1 to 16.
    pub max_players: u8,
    /// The reserved accounts. Open custom games have an empty roster.
    pub roster: Vec<Xuid>,
    /// Bot seat names for sim matches and bot fill.
    pub bots: Vec<String>,
    pub transport: Transport,
    #[serde(default = "default_idle_timeout_s")]
    pub idle_timeout_s: u32,
    #[serde(default = "default_max_match_s")]
    pub max_match_s: u32,
    pub created_ms: i64,
}

impl HostLease {
    /// The hard runtime deadline, in Unix milliseconds.
    pub fn deadline_ms(&self) -> i64 {
        self.created_ms
            .saturating_add(i64::from(self.max_match_s) * 1000)
    }

    /// Generates a lease ID from 16 random bytes encoded as unpadded base64url.
    pub fn new_id() -> String {
        format!("hl-{}", random_id())
    }
}

fn default_idle_timeout_s() -> u32 {
    DEFAULT_IDLE_TIMEOUT_S
}

fn default_max_match_s() -> u32 {
    DEFAULT_MAX_MATCH_S
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    Local,
    Cloudflare,
    Fly,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostClass {
    Small,
    Large,
}

impl HostClass {
    pub const fn vcpu(&self) -> u32 {
        match self {
            Self::Small => 1,
            Self::Large => 2,
        }
    }

    pub const fn gib(&self) -> u32 {
        match self {
            Self::Small => 2,
            Self::Large => 4,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Transport {
    Udp,
    Tunnel,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEndpoint {
    pub transport: Transport,
    /// The public address and base port. The game uses base through base+6.
    pub udp: Option<String>,
    pub tunnel_url: Option<String>,
    /// Hex of the host's advertised XNADDR.
    pub xnaddr: String,
    /// The Live session the host published.
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
    Requested,
    Booting,
    Joinable,
    Playing,
    Ended,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EndReason {
    MatchOver,
    Empty,
    Idle,
    MaxDuration,
    Stopped,
    BootTimeout,
    Crashed,
    Budget,
}

/// Data for [`HOST_REQUESTED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRequested {
    pub lease: HostLease,
    /// A provider address known before launch, usable for cleanup after replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
}

/// Data for [`HOST_BOOTING`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostBooting {
    pub lease_id: String,
    /// The provider's container ID, machine ID or process ID.
    pub provider_ref: String,
}

/// Data for [`HOST_READY`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostReady {
    pub lease_id: String,
    pub endpoint: HostEndpoint,
    pub boot_ms: u64,
}

/// Data for [`HOST_PLAYING`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostPlaying {
    pub lease_id: String,
    pub players: u8,
}

/// Data for [`HOST_ENDED`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostEnded {
    pub lease_id: String,
    pub reason: EndReason,
    pub ran_ms: u64,
    pub players_peak: u8,
}

/// Data for [`HOST_STOPPED`], recorded after a managed host confirms shutdown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostStopped {
    pub lease_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use serde::de::DeserializeOwned;
    use serde_json::{Value, json};
    use std::fmt::Debug;

    const LEASE_ID: &str = "hl-AAAAAAAAAAAAAAAAAAAAAA";

    fn round_trip<T>(value: T, wire: Value)
    where
        T: Debug + PartialEq + Serialize + DeserializeOwned,
    {
        assert_eq!(serde_json::to_value(&value).unwrap(), wire);
        assert_eq!(serde_json::from_value::<T>(wire).unwrap(), value);
    }

    fn lease() -> HostLease {
        HostLease {
            id: LEASE_ID.into(),
            assignment_id: "custom-lockout".into(),
            provider: ProviderKind::Local,
            region: None,
            class: HostClass::Small,
            map: "lockout".into(),
            map_id: None,
            variant: Some("Slayer".into()),
            hopper: None,
            max_players: 8,
            roster: vec![Xuid(0x0009_0000_0000_0001)],
            bots: vec!["Clanker".into()],
            transport: Transport::Udp,
            idle_timeout_s: DEFAULT_IDLE_TIMEOUT_S,
            max_match_s: DEFAULT_MAX_MATCH_S,
            created_ms: 1_800_000_000_000,
        }
    }

    fn udp_endpoint() -> HostEndpoint {
        HostEndpoint {
            transport: Transport::Udp,
            udp: Some("203.0.113.4:31000".into()),
            tunnel_url: None,
            xnaddr: "00".repeat(36),
            session_id: Some("0123456789abcdef".into()),
        }
    }

    #[test]
    fn lease_ids_encode_sixteen_random_bytes() {
        let id = HostLease::new_id();
        let token = id.strip_prefix("hl-").unwrap();
        assert_eq!(token.len(), 22);
        assert_eq!(URL_SAFE_NO_PAD.decode(token).unwrap().len(), 16);
        assert_ne!(id, HostLease::new_id());
    }

    #[test]
    fn lease_round_trips_and_defaults_timeouts() {
        let mut wire = json!({
            "id": LEASE_ID,
            "assignment_id": "custom-lockout",
            "provider": "local",
            "region": null,
            "class": "small",
            "map": "lockout",
            "map_id": null,
            "variant": "Slayer",
            "hopper": null,
            "max_players": 8,
            "roster": ["0009000000000001"],
            "bots": ["Clanker"],
            "transport": "udp",
            "idle_timeout_s": 120,
            "max_match_s": 1800,
            "created_ms": 1_800_000_000_000_i64,
        });
        round_trip(lease(), wire.clone());

        let fields = wire.as_object_mut().unwrap();
        fields.remove("idle_timeout_s");
        fields.remove("max_match_s");
        assert_eq!(
            serde_json::from_value::<HostLease>(wire.clone()).unwrap(),
            lease()
        );

        wire["idle_timeout_s"] = json!(45);
        wire["max_match_s"] = json!(900);
        round_trip(
            HostLease {
                idle_timeout_s: 45,
                max_match_s: 900,
                ..lease()
            },
            wire,
        );
    }

    #[test]
    fn host_enums_round_trip() {
        for (value, name) in [
            (ProviderKind::Local, "local"),
            (ProviderKind::Cloudflare, "cloudflare"),
            (ProviderKind::Fly, "fly"),
        ] {
            round_trip(value, json!(name));
        }
        for (value, name, vcpu, gib) in [
            (HostClass::Small, "small", 1, 2),
            (HostClass::Large, "large", 2, 4),
        ] {
            assert_eq!((value.vcpu(), value.gib()), (vcpu, gib));
            round_trip(value, json!(name));
        }
        round_trip(Transport::Udp, json!("udp"));
        round_trip(Transport::Tunnel, json!("tunnel"));
        for (value, name) in [
            (HostState::Requested, "requested"),
            (HostState::Booting, "booting"),
            (HostState::Joinable, "joinable"),
            (HostState::Playing, "playing"),
            (HostState::Ended, "ended"),
            (HostState::Failed, "failed"),
        ] {
            round_trip(value, json!(name));
        }
        for (value, name) in [
            (EndReason::MatchOver, "match_over"),
            (EndReason::Empty, "empty"),
            (EndReason::Idle, "idle"),
            (EndReason::MaxDuration, "max_duration"),
            (EndReason::Stopped, "stopped"),
            (EndReason::BootTimeout, "boot_timeout"),
            (EndReason::Crashed, "crashed"),
            (EndReason::Budget, "budget"),
        ] {
            round_trip(value, json!(name));
        }
    }

    #[test]
    fn endpoints_round_trip_for_both_transports() {
        round_trip(
            udp_endpoint(),
            json!({
                "transport": "udp",
                "udp": "203.0.113.4:31000",
                "tunnel_url": null,
                "xnaddr": "00".repeat(36),
                "session_id": "0123456789abcdef",
            }),
        );
        let tunnel_url = format!("wss://live.forbes.red/v1/hosts/{LEASE_ID}/tunnel");
        round_trip(
            HostEndpoint {
                transport: Transport::Tunnel,
                udp: None,
                tunnel_url: Some(tunnel_url.clone()),
                xnaddr: "00".repeat(36),
                session_id: None,
            },
            json!({
                "transport": "tunnel",
                "udp": null,
                "tunnel_url": tunnel_url,
                "xnaddr": "00".repeat(36),
                "session_id": null,
            }),
        );
    }

    #[test]
    fn host_event_payloads_round_trip() {
        assert_eq!(STREAM, "hosts");
        assert_eq!(
            KINDS,
            [
                "HostRequested",
                "HostBooting",
                "HostReady",
                "HostPlaying",
                "HostEnded",
                "HostStopped",
            ],
        );
        round_trip(
            HostRequested {
                lease: lease(),
                provider_ref: None,
            },
            json!({ "lease": lease() }),
        );
        round_trip(
            HostRequested {
                lease: lease(),
                provider_ref: Some("room".into()),
            },
            json!({ "lease": lease(), "provider_ref": "room" }),
        );
        round_trip(
            HostBooting {
                lease_id: LEASE_ID.into(),
                provider_ref: "1234".into(),
            },
            json!({ "lease_id": LEASE_ID, "provider_ref": "1234" }),
        );
        round_trip(
            HostReady {
                lease_id: LEASE_ID.into(),
                endpoint: udp_endpoint(),
                boot_ms: 19_000,
            },
            json!({ "lease_id": LEASE_ID, "endpoint": udp_endpoint(), "boot_ms": 19_000 }),
        );
        round_trip(
            HostPlaying {
                lease_id: LEASE_ID.into(),
                players: 4,
            },
            json!({ "lease_id": LEASE_ID, "players": 4 }),
        );
        round_trip(
            HostEnded {
                lease_id: LEASE_ID.into(),
                reason: EndReason::MatchOver,
                ran_ms: 250_000,
                players_peak: 5,
            },
            json!({
                "lease_id": LEASE_ID, "reason": "match_over", "ran_ms": 250_000, "players_peak": 5
            }),
        );
        round_trip(
            HostStopped {
                lease_id: LEASE_ID.into(),
            },
            json!({ "lease_id": LEASE_ID }),
        );
    }
}
