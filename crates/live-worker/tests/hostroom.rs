//! Run the production path and launch helpers without a Worker runtime.
#[path = "../src/hostroom/lease.rs"]
mod lease;

use lease::{HostEnv, tunnel_lease};
use live_server::hosts::HostBoot;
use serde_json::json;

#[test]
fn tunnel_matches_one_lease_segment_and_no_control_routes() {
    let id = "hl-aZ0_-1234567890123456Q";
    assert_eq!(tunnel_lease(&format!("/v1/hosts/{id}/tunnel")), Some(id));
    for path in [
        "/v1/hosts//tunnel".to_owned(),
        "/v1/hosts/hl-short/tunnel".to_owned(),
        format!("/v1/hosts/{id}/start"),
        format!("/v1/hosts/{id}/stop/tunnel"),
        format!("/v1/hosts/{id}/tunnel/"),
        format!("/v1/hosts/{id}%2ftunnel"),
        "/v1/hosts/hl-1234567890123456789%2F/tunnel".to_owned(),
    ] {
        assert_eq!(tunnel_lease(&path), None, "{path}");
    }
}

#[test]
fn boot_env_preserves_the_complete_lease_and_signed_url() {
    let boot = HostBoot {
        live_url: "https://live.example".into(),
        token: "lease-scoped-token".into(),
        data_url: "https://data.example/bundle?sig=a%2Bb&expires=123".into(),
        data_sha256: "ab".repeat(32),
        archive: live_proto::hosts::HostArchive::Film,
    };
    // Include the round trip through the provider's internal start request.
    let mut lease = cloud_lease();
    assert!(
        serde_json::to_value(HostEnv::new(&lease, &boot).unwrap())
            .unwrap()
            .get("H2_REGION")
            .is_none()
    );
    lease.region = Some("enam".into());
    lease.variant = Some(r#"Team "Slayer""#.into());
    lease.bots = vec!["Clanker".into()];
    let env: HostEnv =
        serde_json::from_value(serde_json::to_value(HostEnv::new(&lease, &boot).unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        serde_json::to_value(env).unwrap(),
        json!({
            "H2_LEASE": lease.id,
            "H2_HOST_LEASE_JSON": serde_json::to_string(&lease).unwrap(),
            "H2_LIVE": boot.live_url,
            "H2_TOKEN": boot.token,
            "H2_DATA_URL": boot.data_url,
            "H2_DATA_SHA": boot.data_sha256,
            "H2_HOST_ARCHIVE": "film",
            "H2_REGION": "enam",
            "H2_AGENT": "tcp://127.0.0.1:14720",
            "H2_TUNNEL_LISTEN": "0.0.0.0:31100",
        })
    );
}

fn cloud_lease() -> live_proto::hosts::HostLease {
    use live_proto::{Xuid, hosts::*};
    HostLease {
        id: "hl-aZ0_-1234567890123456Q".into(),
        assignment_id: "custom-lockout".into(),
        class: HostClass::Small,
        provider: ProviderKind::Cloudflare,
        region: None,
        map: "lockout".into(),
        map_id: None,
        variant: None,
        hopper: None,
        max_players: 2,
        roster: vec![Xuid(42)],
        bots: vec![],
        transport: Transport::Tunnel,
        idle_timeout_s: 120,
        max_match_s: 1800,
        created_ms: 25_000,
    }
}

#[test]
fn provider_references_address_the_same_lease_in_each_class() {
    use live_proto::hosts::HostClass;
    let mut lease = cloud_lease();
    for (class, binding) in [
        (HostClass::Small, "HOST_ROOM"),
        (HostClass::Large, "HOST_ROOM_LARGE"),
    ] {
        lease.class = class;
        let reference = lease::provider_ref(&lease);
        assert_eq!(
            lease::parse_ref(&reference),
            Some((binding, lease.id.as_str()))
        );
    }
    assert!(lease::parse_ref(&format!("LIVE_HUB/{}", lease.id)).is_none());
    assert!(lease::parse_ref(&format!("HOST_ROOM/{}/start", lease.id)).is_none());
}

#[test]
fn a_tunnel_needs_a_rostered_account_on_a_joinable_cloudflare_lease() {
    use live_proto::{
        ErrorCode, Xuid,
        hosts::{HostState, ProviderKind, Transport},
    };
    let mut host = cloud_lease();
    for state in [HostState::Joinable, HostState::Playing] {
        assert!(lease::admit(&host, &state, Xuid(42)).is_ok());
        assert_eq!(
            lease::admit(&host, &state, Xuid(99)).unwrap_err().code,
            ErrorCode::Forbidden
        );
    }
    for state in [
        HostState::Requested,
        HostState::Booting,
        HostState::Ended,
        HostState::Failed,
    ] {
        assert_eq!(
            lease::admit(&host, &state, Xuid(42)).unwrap_err().code,
            ErrorCode::Conflict
        );
    }
    host.transport = Transport::Udp;
    assert!(lease::admit(&host, &HostState::Joinable, Xuid(42)).is_err());
    host.transport = Transport::Tunnel;
    host.provider = ProviderKind::Fly;
    assert!(lease::admit(&host, &HostState::Playing, Xuid(42)).is_err());
    host.provider = ProviderKind::Cloudflare;
    host.roster.clear();
    assert_eq!(
        lease::admit(&host, &HostState::Joinable, Xuid(42))
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
}

// Wrangler's cap is a deploy-time number; LIVE_MAX_HOSTS is read at runtime.
// Catch an edit that changes capacity in just one place or environment.
#[test]
fn wrangler_room_caps_match_live_admission_in_each_environment() {
    let config = include_str!("../wrangler.toml");
    for prefix in ["", "env.staging.", "env.cutover.", "env.production."] {
        let vars = format!("[{prefix}vars]\n");
        let vars = config
            .split_once(&vars)
            .unwrap()
            .1
            .split("\n[")
            .next()
            .unwrap();
        let cap = vars
            .lines()
            .find_map(|line| line.strip_prefix("LIVE_MAX_HOSTS = "))
            .unwrap();
        let cap = cap.trim_matches('"').parse::<usize>().unwrap();

        let header = format!("[[{prefix}containers]]\n");
        let mut classes = vec![];
        for section in config.split(&header).skip(1) {
            let section = section.split("\n[").next().unwrap();
            let field = |name: &str| {
                section
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once('=')?;
                        (key.trim() == name).then(|| value.trim().trim_matches('"'))
                    })
                    .unwrap()
            };
            assert_eq!(
                field("max_instances").parse::<usize>().unwrap(),
                cap,
                "{prefix}"
            );
            classes.push(field("class_name"));
        }
        classes.sort_unstable();
        assert_eq!(classes, ["HostRoom", "LargeHostRoom"], "{prefix}");
    }
}
