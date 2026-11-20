//! Run the production path and launch helpers without a Worker runtime.
#[path = "../src/hostroom/lease.rs"]
mod lease;

use lease::{
    Role, TUNNEL_PORT, add_match, pack_env, pack_matches, pack_room, parse_pack_room, pool_hint,
    pool_name, role, tunnel_lease,
};
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
fn add_match_preserves_the_complete_lease_and_signed_url() {
    let boot = HostBoot {
        live_url: "https://live.example".into(),
        token: "lease-scoped-token".into(),
        data_url: "https://data.example/bundle?sig=a%2Bb&expires=123".into(),
        data_sha256: "ab".repeat(32),
        archive: live_proto::hosts::HostArchive::Film,
    };
    // Include the round trip through the rooms' internal requests.
    let mut lease = cloud_lease();
    assert!(
        serde_json::to_value(add_match(&lease, &boot).unwrap())
            .unwrap()
            .get("region")
            .is_none()
    );
    lease.region = Some("enam".into());
    lease.variant = Some(r#"Team "Slayer""#.into());
    lease.bots = vec!["Clanker".into()];
    let add: live_proto::host_pack::AddMatch =
        serde_json::from_value(serde_json::to_value(add_match(&lease, &boot).unwrap()).unwrap())
            .unwrap();
    assert_eq!(
        serde_json::to_value(add).unwrap(),
        json!({
            "lease": lease.id,
            "lease_json": serde_json::to_string(&lease).unwrap(),
            "live": boot.live_url,
            "token": boot.token,
            "data_url": boot.data_url,
            "data_sha": boot.data_sha256,
            "archive": "film",
            "region": "enam",
        })
    );
}

#[test]
fn rooms_are_told_apart_by_name() {
    let id = "hl-aZ0_-1234567890123456Q";
    assert_eq!(role(id), Some(Role::Lease));
    assert_eq!(role("pool-any"), Some(Role::Pool));
    assert_eq!(role("pool-enam"), Some(Role::Pool));
    assert_eq!(role("pool-enam/pack-3"), Some(Role::Pack));
    for name in ["", "pool-", "pool-lhr", "pool-enam/", "pool-enam/pack-x", "pool-any/pack-1/x", "hl-short"] {
        assert_eq!(role(name), None, "{name}");
    }
    // Only Cloudflare location hints name a pool; other regions share one.
    assert_eq!(pool_name(Some("weur")), "pool-weur");
    assert_eq!(pool_name(Some("lhr")), "pool-any");
    assert_eq!(pool_name(None), "pool-any");
    assert_eq!(pool_hint("pool-weur"), Some("weur"));
    assert_eq!(pool_hint("pool-any"), None);
    let room = pack_room("pool-weur", "pack-12");
    assert_eq!(parse_pack_room(&room), Some(("pool-weur", "pack-12")));
}

#[test]
fn pack_size_follows_the_class_and_stays_below_the_control_port() {
    use live_proto::hosts::HostClass::{Small, Large};
    assert_eq!(pack_matches(&Small, None), 5);
    assert_eq!(pack_matches(&Large, None), 10);
    assert_eq!(pack_matches(&Small, Some("2")), 2);
    assert_eq!(pack_matches(&Small, Some("0")), 1);
    assert_eq!(pack_matches(&Small, Some("99")), 5);
    assert_eq!(pack_matches(&Large, Some("99")), 10);
    assert_eq!(pack_matches(&Large, Some("auto")), 10);
    use live_proto::host_pack::{AGENT_PORT, BASE_PORT, CONTROL_PORT, SlotPorts, UDP_WIDTH};
    let last = SlotPorts::for_slot(BASE_PORT, TUNNEL_PORT, AGENT_PORT, 9).unwrap();
    assert!(last.base_port + UDP_WIDTH - 1 < CONTROL_PORT);
    assert!(last.tunnel_port > CONTROL_PORT);
}

#[test]
fn pack_containers_get_their_class_size_and_no_lease() {
    use live_proto::hosts::HostClass;
    let small = pack_env(&HostClass::Small, 5);
    assert_eq!(small["H2_PACK"], "1");
    assert_eq!(small["H2_PACK_MAX_MATCHES"], "5");
    assert_eq!(small["H2_PACK_VCPUS"], "1");
    assert_eq!(small["H2_PACK_MEMORY_MIB"], "1536");
    assert_eq!(small["H2_PACK_CONTROL"], "0.0.0.0:31099");
    let large = pack_env(&HostClass::Large, 10);
    assert_eq!((large["H2_PACK_VCPUS"].as_str(), large["H2_PACK_MEMORY_MIB"].as_str()), ("2", "3072"));
    assert!(small.keys().all(|key| !key.contains("TOKEN") && !key.contains("LEASE")));
    // The capped Cloudflare scheduling policy needs at least 3 GiB per vCPU.
    let config = include_str!("../wrangler.toml");
    assert!(config.contains("instance_type = { vcpu = 1, memory_mib = 3072"));
    assert!(config.contains("instance_type = { vcpu = 2, memory_mib = 6144"));
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
