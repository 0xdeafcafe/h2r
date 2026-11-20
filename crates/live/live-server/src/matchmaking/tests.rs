use super::*;
use live_proto::matchmaking::Sample;

const HOPPER: u32 = 2;
const FORMED: i64 = 100;
const DEADLINE: i64 = FORMED + BOOT_SECONDS;
const FIRST: Xuid = Xuid(1);
const SECOND: Xuid = Xuid(2);
const SESSION: &str = "8abc1234def56789";

fn config(global: bool, hopper: u32) -> DedicatedConfig {
    DedicatedConfig::from_lookup(|key| match key {
        "LIVE_DEDICATED" if global => Some("1".into()),
        "LIVE_DEDICATED_HOPPERS" => Some(serde_json::json!({
            (hopper.to_string()): {"map": "lockout", "variant": "Team Slayer"}
        }).to_string()),
        "LIVE_HOST_REGIONS" => Some(r#"{"lhr":"192.0.2.30:31000","iad":null}"#.into()),
        "LIVE_HOST_DEFAULT_REGION" => Some("iad".into()),
        _ => None,
    })
    .unwrap()
}

fn ticket(port: u16) -> Ticket {
    Ticket {
        hopper: HOPPER,
        port,
        dedicated: false,
        connectivity: Connectivity::default(),
        samples: Vec::new(),
    }
}

fn sample(target: &Candidate, rtt_ms: u16) -> Sample {
    Sample {
        target: target.xuid,
        generation: target.generation.clone(),
        rtt_ms: Some(rtt_ms),
    }
}

fn make_cohort(
    config: &DedicatedConfig,
    provider: Option<ProviderKind>,
    with_probes: bool,
) -> (Lease, Option<HostLease>) {
    let probes = if with_probes { [Some(65), Some(70)] } else { [None, None] };
    make_cohort_with_probes(config, provider, &probes, true, 45)
}

fn make_cohort_with_probes(
    config: &DedicatedConfig,
    provider: Option<ProviderKind>,
    probes: &[Option<u16>],
    ranked: bool,
    peer_rtt: u16,
) -> (Lease, Option<HostLease>) {
    let rules = Rules { size: probes.len(), team_count: 2, ranked };
    let mut bucket = Bucket::default();
    let mut tickets: Vec<_> = (0..probes.len()).map(|n| {
        let t = ticket(31000 + 10 * n as u16);
        let endpoint = SocketAddrV4::new([192, 0, 2, 1 + n as u8].into(), t.port);
        (t, Xuid(1 + n as u64), endpoint)
    }).collect();
    for (t, id, endpoint) in &tickets {
        bucket.submit(t, *id, *endpoint, 4 + id.0 as u8, FORMED, Some(&config.regions)).unwrap();
    }
    assert!(bucket.assign(HOPPER, rules, FORMED, config, provider.clone()).unwrap().is_none());
    let peers: Vec<_> = bucket.waiting.values().map(|w| w.candidate.clone()).collect();
    // Give the candidates fully measured routes. The first player is the
    // reserved peer; choosing it still requires a route from everyone else.
    for ((t, id, endpoint), region_ms) in tickets.iter_mut().zip(probes) {
        for peer in &peers {
            if peer.xuid != *id {
                let ms = if peer.xuid == FIRST { peer_rtt } else { peer_rtt + 10 };
                t.samples.push(sample(peer, ms));
            }
        }
        if let Some(ms) = region_ms {
            let probe = config.regions.candidates().next().unwrap();
            t.samples.push(sample(probe, *ms));
        }
        bucket.submit(t, *id, *endpoint, 4 + id.0 as u8, FORMED, Some(&config.regions)).unwrap();
    }
    let clock = if peer_rtt > 90 { FORMED + 30 } else { FORMED };
    let cohort = bucket.assign(HOPPER, rules, clock, config, provider).unwrap().unwrap();
    assert!(bucket.waiting.is_empty());
    cohort
}

fn reserve() -> (Matchmaker, HostLease) {
    let config = config(true, HOPPER);
    let (lease, request) = make_cohort(&config, Some(ProviderKind::Local), false);
    let mut request = request.unwrap();
    request.id = HostLease::new_id();
    request.created_ms = FORMED * 1000 + 100;
    let matcher = Matchmaker {
        leases: BTreeMap::from([(lease.assignment.id.clone(), lease)]),
        config,
        ..Default::default()
    };
    (matcher, request)
}

fn waiting(lease: &Lease, xuid: Xuid) -> Assignment {
    let Queue::Waiting { assignment } = lease.queue(xuid, &Model::default()) else {
        panic!("expected waiting for {xuid}");
    };
    assignment
}

fn check_fallback(matcher: &Matchmaker, host: &HostLease, clock: i64) {
    assert!(matcher.dedicated_lease(&host.assignment_id, clock).is_none());
    let lease = matcher.lease(&host.assignment_id, clock).unwrap();
    let Queue::Host { assignment } = lease.queue(FIRST, &Model::default()) else {
        panic!("the elected peer should get Host");
    };
    assert_eq!(assignment.id, host.assignment_id);
    assert_eq!(assignment.members, host.roster);
    assert_eq!(assignment.teams, BTreeMap::from([(FIRST, 1), (SECOND, 0)]));
    assert_eq!(assignment.rtt_ms, BTreeMap::from([(FIRST, 0), (SECOND, 45)]));
    assert_eq!(assignment.host_lease, None);
    assert!(!assignment.dedicated);
    assert_eq!(lease.joiners, 1);
    assert_eq!(waiting(lease, SECOND), assignment);
}

#[test]
fn ad_hoc_cohort_requests_configured_lobby_in_measured_region() {
    let config = config(true, HOPPER);
    let (lease, request) = make_cohort(&config, Some(ProviderKind::Cloudflare), true);
    let request = request.unwrap();
    assert_eq!(request.assignment_id, lease.assignment.id);
    assert_eq!(request.roster, [FIRST, SECOND]);
    assert_eq!(request.hopper, Some(HOPPER));
    assert_eq!(request.map, "lockout");
    assert_eq!(request.variant.as_deref(), Some("Team Slayer"));
    assert_eq!(request.region.as_deref(), Some("lhr"));
    assert_eq!(request.transport, Transport::Tunnel);
    assert_eq!(request.max_players, 2);
    assert_eq!(lease.until, DEADLINE);
    for member in [FIRST, SECOND] {
        let assignment = waiting(&lease, member);
        assert!(assignment.dedicated);
        assert_eq!(assignment.rtt_ms, BTreeMap::from([(FIRST, 65), (SECOND, 70)]));
    }
}

#[test]
fn unranked_dedicated_rtt_veto_requires_a_majority_of_the_whole_cohort() {
    let config = config(true, HOPPER);
    let cases: &[(&[Option<u16>], u16, bool)] = &[
        (&[Some(80), Some(121)], 45, false),     // One vote of two.
        (&[Some(120), Some(121)], 45, false),    // Exactly 120 does not vote.
        (&[Some(121), Some(130)], 45, true),
        (&[Some(121), Some(130)], 130, false),   // Second route ties the peer.
        (&[Some(160), Some(129)], 130, false),   // The second member prefers dedicated.
    ];
    for &(probes, peer_rtt, veto) in cases {
        let (lease, request) = make_cohort_with_probes(
            &config, Some(ProviderKind::Local), probes, false, peer_rtt,
        );
        assert_eq!(request.is_none(), veto, "{probes:?} vs {peer_rtt}");
        assert_eq!(lease.assignment.dedicated, !veto);
        if veto { assert_eq!(lease.assignment.host, FIRST); }
    }
}

#[test]
fn missing_regional_rtts_abstain_with_the_whole_cohort_as_denominator() {
    let config = DedicatedConfig::from_lookup(|key| match key {
        "LIVE_DEDICATED" => Some("1".into()),
        "LIVE_DEDICATED_HOPPERS" => Some(format!(r#"{{"{HOPPER}":{{"map":"lockout"}}}}"#)),
        "LIVE_HOST_REGIONS" => Some(r#"{"lhr":"192.0.2.30:31000"}"#.into()),
        "LIVE_HOST_DEFAULT_REGION" => Some("lhr".into()),
        _ => None,
    }).unwrap();
    for (probes, veto) in [
        ([Some(121), None, None], false),
        ([Some(121), None, Some(122)], true),
    ] {
        let (_, request) = make_cohort_with_probes(
            &config, Some(ProviderKind::Cloudflare), &probes, false, 45,
        );
        assert_eq!(request.is_none(), veto);
    }
}

#[test]
fn ranked_hopper_keeps_the_dedicated_host_even_with_unanimous_slow_routes() {
    let config = config(true, HOPPER);
    let (lease, request) = make_cohort_with_probes(
        &config, Some(ProviderKind::Local), &[Some(121), Some(240)], true, 45,
    );
    assert!(request.is_some());
    assert!(lease.assignment.dedicated);
    assert!(lease.ranked);
}

#[test]
fn ad_hoc_disabled_global_hopper_or_provider_keeps_peer_assignment() {
    for (config, provider) in [
        (config(false, HOPPER), Some(ProviderKind::Local)),
        (config(true, HOPPER + 1), Some(ProviderKind::Local)),
        (config(true, HOPPER), None),
    ] {
        let (lease, request) = make_cohort(&config, provider, false);
        assert!(request.is_none());
        let Queue::Host { assignment } = lease.queue(FIRST, &Model::default()) else {
            panic!("the elected peer should get Host immediately");
        };
        assert!(!assignment.dedicated);
        assert_eq!(assignment.host_lease, None);
    }
}

#[test]
fn ad_hoc_binds_before_publication_and_accepts_timely_ready_on_a_late_poll() {
    let (mut matcher, host) = reserve();
    let id = &host.assignment_id;
    assert_eq!(host.region.as_deref(), Some("iad"));
    assert!(matcher.lease(id, FORMED).is_none());
    assert!(matcher.dedicated_lease(id, FORMED).is_none());
    let mut wrong_roster = host.clone();
    wrong_roster.roster.pop();
    assert!(!matcher.bind_host_lease(&wrong_roster, FORMED + 1));
    assert!(matcher.bind_host_lease(&host, FORMED + 1));

    let lease = matcher.dedicated_lease(id, FORMED + 1).unwrap();
    assert_eq!(lease.assignment.host, Xuid(0));
    assert_eq!(lease.joiners, 2);
    assert!(lease.ranked);
    for member in [FIRST, SECOND] {
        assert_eq!(
            waiting(lease, member).host_lease.as_deref(),
            Some(host.id.as_str())
        );
    }
    let mut competing = host.clone();
    competing.id = HostLease::new_id();
    assert!(!matcher.bind_host_lease(&competing, FORMED + 2));

    // A poll elsewhere in the queue can prune old tickets before the
    // members see the registration. Preserve the bound lease meanwhile.
    matcher.expire(DEADLINE);
    let lease = matcher.leases.get_mut(id).unwrap();
    lease.poll_host(&host.id, DEADLINE + 1, BootProgress::Ready {
        session_id: SESSION.into(),
        ready_ms: DEADLINE * 1000 - 1,
    });
    for member in [FIRST, SECOND] {
        let Queue::Join { session_id, frames_from_host } = lease.queue(member, &Model::default()) else {
            panic!("expected the registered session for {member}");
        };
        assert_eq!(session_id, SESSION);
        assert_ne!(session_id, *id);
        assert!(frames_from_host);
    }
}

#[test]
fn ad_hoc_allocation_rejection_restores_the_reserved_peer() {
    let (mut matcher, host) = reserve();
    matcher.host_request_failed(&host.assignment_id, FORMED + 3);
    check_fallback(&matcher, &host, FORMED + 3);
    assert!(!matcher.bind_host_lease(&host, FORMED + 4));
}

#[test]
fn ad_hoc_terminal_provider_view_restores_the_reserved_peer() {
    let (mut matcher, host) = reserve();
    assert!(matcher.bind_host_lease(&host, FORMED + 1));
    matcher.leases.get_mut(&host.assignment_id).unwrap()
        .poll_host(&host.id, FORMED + 3, BootProgress::Failed);
    check_fallback(&matcher, &host, FORMED + 3);
}

#[test]
fn ad_hoc_bound_or_unbound_boot_cannot_outlast_deadline() {
    for progress in [
        None,
        Some(BootProgress::Pending),
        Some(BootProgress::Ready { session_id: SESSION.into(), ready_ms: DEADLINE * 1000 }),
    ] {
        let (mut matcher, host) = reserve();
        let id = &host.assignment_id;
        if progress.is_some() {
            assert!(matcher.bind_host_lease(&host, FORMED + 1));
        }
        matcher.expire(DEADLINE - 1);
        assert!(waiting(&matcher.leases[id], FIRST).dedicated);
        matcher.expire(DEADLINE);
        if let Some(progress) = progress {
            matcher.leases.get_mut(id).unwrap().poll_host(&host.id, DEADLINE, progress);
        }
        check_fallback(&matcher, &host, DEADLINE);
        assert!(!matcher.bind_host_lease(&host, DEADLINE));
        assert!(matcher.lease(id, DEADLINE + ASSIGNMENT_SECONDS - 1).is_some());
        matcher.expire(DEADLINE + ASSIGNMENT_SECONDS);
        assert!(!matcher.leases.contains_key(id));
    }
}

#[tokio::test]
async fn leased_publication_authorizes_only_the_assigned_session() {
    use crate::{Policy, ServerIdentity, app::Timers, store::MemoryStore};
    use live_proto::{
        ErrorCode, hosts, session,
        sign::{EventBody, Jws, KeyPair, TYP_TOKEN, sign_batch},
    };
    use serde_json::json;

    let live = Live::start_with(
        Arc::new(MemoryStore::new()),
        ServerIdentity {
            id: "lease.test".into(),
            key: KeyPair::generate(),
        },
        Policy::default(),
        Timers::Driven,
    )
    .await
    .unwrap();
    let (mut matcher, mut host) = reserve();
    let clock = now();
    host.created_ms = clock * 1000;
    matcher.leases.get_mut(&host.assignment_id).unwrap().until = clock + BOOT_SECONDS;
    assert!(matcher.host_requested(&host));
    *live.matchmaking.lock().unwrap() = matcher;

    let mut other = host.clone();
    other.id = HostLease::new_id();
    let events = [&host, &other].map(|h| {
        live.server_event(
            hosts::STREAM.into(),
            hosts::HOST_REQUESTED,
            json!({"lease": h}),
        )
    });
    let rows = live.store.append(events.into(), &[]).await.unwrap();
    live.wait_applied(rows.last().unwrap().position)
        .await
        .unwrap();
    let token = live.issue_host_token(&host);
    let wrong = live.issue_host_token(&other);
    let ad = serde_json::from_value::<session::NewSession>(json!({
        "port":31000, "host_address":vec![0;36], "attributes":{}, "map":host.map,
        "slots":{"public_open":2,"public_filled":0,"private_open":0,"private_filled":0}
    }))
    .unwrap();
    let addr = Some(std::net::Ipv4Addr::LOCALHOST.into());
    assert_eq!(
        live.create_session(Some(&wrong), ad.clone(), addr)
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    let row = live.create_session(Some(&token), ad, addr).await.unwrap();
    assert_ne!(row.session_id, host.assignment_id);
    let assignment = row.matchmaking.as_ref().unwrap();
    assert_eq!(assignment.host_lease.as_ref(), Some(&host.id));
    assert_eq!(assignment.members, host.roster);

    let batch = |bearer: &str, kind: &str, data| {
        let claims = Jws::parse(bearer).unwrap().json_payload(TYP_TOKEN).unwrap();
        let key = KeyPair::from_secret_b64(claims["event_key"].as_str().unwrap()).unwrap();
        let event = EventBody::at_unix(
            &key,
            claims["sub"].as_str().unwrap(),
            &session::stream(&row.session_id),
            kind,
            data,
            now(),
        );
        sign_batch(&key, &[event])
    };
    let update = |bearer: &str| batch(bearer, session::UPDATED, json!({"name":"ready"}));
    assert_eq!(
        live.append_session_events(Some(&wrong), &row.session_id, &update(&wrong))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    live.append_session_events(Some(&token), &row.session_id, &update(&token))
        .await
        .unwrap();
    assert_eq!(
        live.read_caller(Some(&token)).unwrap_err().code,
        ErrorCode::Unauthorized
    );
    assert_eq!(
        live.append_signed(Some(&token), &update(&token))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Unauthorized
    );

    let ended = live.server_event(
        hosts::STREAM.into(),
        hosts::HOST_ENDED,
        json!({
            "lease_id":host.id, "reason":"stopped", "ran_ms":2, "players_peak":0
        }),
    );
    let events = live.store.append(vec![ended], &[]).await.unwrap();
    live.wait_applied(events[0].position).await.unwrap();
    assert_eq!(
        live.append_session_events(Some(&token), &row.session_id, &update(&token))
            .await
            .unwrap_err()
            .code,
        ErrorCode::Forbidden
    );
    let delete = batch(&token, session::DELETED, json!({"reason":"closed"}));
    let pos = live
        .append_session_events(Some(&token), &row.session_id, &delete)
        .await
        .unwrap();
    assert_eq!(
        live.append_session_events(Some(&token), &row.session_id, &delete)
            .await
            .unwrap(),
        pos
    );
}

#[test]
fn warm_demand_counts_only_fresh_players_in_opted_in_hoppers() {
    let mut matcher = Matchmaker { config: config(true, HOPPER), ..Default::default() };
    let rules = Rules { size: 2, team_count: 2, ranked: false };
    matcher.rules = Some(("synthetic".into(), [Some(rules); HOPPERS]));
    let endpoint = "192.0.2.1:31000".parse().unwrap();
    let bucket = matcher.buckets.entry(HOPPER).or_default();
    let mut first = ticket(31000);
    let lhr = matcher.config.regions.candidates().next().unwrap().clone();
    first.samples.push(sample(&lhr, 60));
    bucket.submit(&first, FIRST, endpoint, 1, FORMED, Some(&matcher.config.regions)).unwrap();
    let mut registered = ticket(31001);
    registered.dedicated = true;
    bucket.submit(&registered, SECOND, endpoint, 1, FORMED, None).unwrap();
    matcher.buckets.entry(HOPPER + 1).or_default()
        .submit(&ticket(31002), Xuid(3), endpoint, 1, FORMED, None).unwrap();
    assert_eq!(matcher.search_demand(FORMED), BTreeMap::from([(Some("lhr".into()), 1)]));
    assert!(matcher.search_demand(FORMED + TICKET_SECONDS).is_empty());
}
