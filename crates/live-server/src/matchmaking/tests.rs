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
    let rules = Rules { size: 2, team_count: 2, ranked: true };
    let mut bucket = Bucket::default();
    let mut first = ticket(31000);
    let mut second = ticket(31010);
    let a = SocketAddrV4::new("192.0.2.1".parse().unwrap(), first.port);
    let b = SocketAddrV4::new("192.0.2.2".parse().unwrap(), second.port);
    let regions = with_probes.then_some(&config.regions);
    bucket.submit(&first, FIRST, a, 5, FORMED, regions).unwrap();
    bucket.submit(&second, SECOND, b, 6, FORMED, regions).unwrap();
    assert!(bucket.assign(HOPPER, rules, FORMED, config, provider.clone()).unwrap().is_none());

    first.samples.push(sample(&bucket.waiting[&SECOND].candidate, 55));
    second.samples.push(sample(&bucket.waiting[&FIRST].candidate, 45));
    if with_probes {
        let probe = config.regions.candidates().next().unwrap();
        first.samples.push(sample(probe, 65));
        second.samples.push(sample(probe, 70));
    }
    bucket.submit(&first, FIRST, a, 5, FORMED, regions).unwrap();
    bucket.submit(&second, SECOND, b, 6, FORMED, regions).unwrap();
    let cohort = bucket.assign(HOPPER, rules, FORMED, config, provider).unwrap().unwrap();
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
