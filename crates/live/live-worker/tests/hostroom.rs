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

#[path = "../src/hostroom/pack_state.rs"]
mod pack_state;

#[test]
fn search_warms_an_empty_pack_and_silence_retires_it_even_after_eviction() {
    use live_proto::host_pack::LINGER_MS;
    use pack_state::PackLeases;
    let mut pack = PackLeases::default();
    assert_eq!(pack.deadline(), None);
    pack.hold_until(10 + LINGER_MS);
    pack.hold_until(1_000 + LINGER_MS);
    let mut restored: PackLeases = serde_json::from_value(serde_json::to_value(pack).unwrap()).unwrap();
    let end = 1_000 + LINGER_MS;
    restored.expire(end - 1);
    assert_eq!(restored.deadline(), Some(end));
    restored.expire(end);
    assert_eq!(restored.deadline(), None);
}

#[test]
fn releasing_the_last_match_lingers_once_and_a_new_match_supersedes_the_timer() {
    use live_proto::host_pack::LINGER_MS;
    use pack_state::{PackLease, PackLeases};
    // The persisted layout predates warm packs.
    let mut pack: PackLeases = serde_json::from_value(json!({
        "leases":{"first":{"deadline_ms":1_000_000,"tunnel_port":31100}}
    })).unwrap();
    pack.hold_until(LINGER_MS);
    assert_eq!(pack.deadline(), Some(1_000_000));
    pack.release("first", 3);
    pack.release("first", 9);
    assert_eq!(pack.deadline(), Some(3 + LINGER_MS));
    pack.admitted("next".into(), PackLease { deadline_ms: 2_000_000, tunnel_port: 31100 });
    pack.expire(3 + LINGER_MS);
    assert_eq!(pack.deadline(), Some(2_000_000));
    pack.expire(2_000_000);
    assert_eq!(pack.deadline(), None);
}

#[test]
fn the_pool_and_container_agree_when_a_search_starts_and_its_hold_expires() {
    use live_proto::host_pack::{LINGER_MS, Placement, Pool, WarmDemand};
    use pack_state::PackLeases;
    let mut pool = Pool::default();
    let mut container = PackLeases::default();
    let quiet = WarmDemand { available: 4, ..WarmDemand::default() };
    assert!(pool.warm_plan(quiet, 3, 0).start.is_none());
    let searching = WarmDemand { searchers: 1, ..quiet };
    let name = pool.warm_plan(searching, 3, 0).start.unwrap();
    container.hold_until(20 + LINGER_MS);
    pool.warmed(&name, 20, Some(20));
    assert_eq!(pool.place("synthetic-lease", 3), Placement::Existing(name.clone()));
    let hold = pool.warm_plan(quiet, 3, 21);
    assert!(hold.start.is_none() && hold.refresh.is_empty());
    let end = container.deadline().unwrap();
    assert_eq!(end, 20 + LINGER_MS);
    assert_eq!(pool.next_deadline(), Some(end));
    assert!(pool.expire(end - 1).is_empty());
    container.expire(end);
    assert_eq!(pool.expire(end), vec![name]);
    assert!(container.deadline().is_none() && pool.is_empty());
}

#[test]
fn a_stale_quote_cannot_start_a_replacement_or_extend_the_paid_interval() {
    use live_proto::host_pack::{LINGER_MS, WarmPlan};
    use pack_state::{funded, grant_deadline};

    let grant = WarmPlan { refresh: vec!["pack-2".into()], start: Some("pack-3".into()) };
    let changed = WarmPlan {
        refresh: vec!["pack-2".into(), "pack-3".into()],
        start: Some("pack-4".into()),
    };
    let plan = funded(changed, &grant);
    assert_eq!(plan.refresh, ["pack-2"]);
    assert_eq!(plan.start, None);
    let until = 200 + LINGER_MS;
    assert_eq!(grant_deadline(200, until + 1), Some(until));
    assert_eq!(grant_deadline(until - 1, until), Some(until));
    assert_eq!(grant_deadline(until, until), None);
}
