use super::http::{Parse, parse_request, parse_response, request, response};
use super::*;

fn measured(ms: f64, mib: u64) -> MatchLoad {
    MatchLoad {
        ms_per_tick: Some(ms),
        rss_mib: Some(mib),
        ..MatchLoad::default()
    }
}

fn small() -> Limits {
    Limits::new(3, 1.0, 3072)
}

// -- Admission ----------------------------------------------------------------

#[test]
fn budget_is_seventy_percent_of_a_tick_per_vcpu() {
    assert!((small().tick_budget_ms() - 23.333).abs() < 0.01);
    assert!((Limits::new(3, 2.0, 6144).tick_budget_ms() - 46.667).abs() < 0.01);
    assert_eq!(small().memory_budget_mib(), 3072 - 256);
}

#[test]
fn empty_pack_admits_its_first_match() {
    assert_eq!(admit(&small(), &[], None), Ok(()));
    assert_eq!(admit(&small(), &[], Some(40)), Ok(()));
}

#[test]
fn measured_light_matches_pack_three_to_a_vcpu() {
    // 6 + 6 + 10 (assumed for the new match) = 22 ms <= 23.3 ms.
    let running = [measured(6.0, 340), measured(6.0, 340)];
    assert_eq!(admit(&small(), &running, Some(800)), Ok(()));
}

#[test]
fn refuses_when_summed_tick_cost_passes_the_budget() {
    // 9 + 9 + 10 = 28 ms > 23.3 ms.
    let running = [measured(9.0, 340), measured(9.0, 340)];
    match admit(&small(), &running, Some(800)) {
        Err(Refusal::Cpu {
            projected_ms,
            budget_ms,
        }) => {
            assert!((projected_ms - 28.0).abs() < 1e-9);
            assert!((budget_ms - 23.333).abs() < 0.01);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn unmeasured_matches_count_at_the_default_cost() {
    // Two just-started matches: 10 + 10 + 10 = 30 ms > 23.3 ms on one vCPU.
    let running = [MatchLoad::default(), MatchLoad::default()];
    assert!(matches!(admit(&small(), &running, None), Err(Refusal::Cpu { .. })));
    // Two vCPUs take three unmeasured matches.
    let large = Limits::new(3, 2.0, 6144);
    assert_eq!(admit(&large, &running, None), Ok(()));
}

#[test]
fn refuses_past_the_match_cap_even_with_cpu_to_spare() {
    let running = [measured(1.0, 100); 3];
    assert_eq!(
        admit(&small(), &running, Some(300)),
        Err(Refusal::Full { running: 3, max: 3 })
    );
}

#[test]
fn refuses_when_memory_would_pass_the_container_limit() {
    let running = [measured(5.0, 340)];
    // 2600 measured + 340 assumed > 3072 - 256.
    assert_eq!(
        admit(&small(), &running, Some(2600)),
        Err(Refusal::Memory {
            projected_mib: 2940,
            budget_mib: 2816
        })
    );
    // Without a container figure, the matches' own RSS is summed.
    let tight = Limits::new(3, 4.0, 1024);
    assert!(matches!(
        admit(&tight, &[measured(1.0, 500)], None),
        Err(Refusal::Memory { projected_mib: 840, budget_mib: 768 })
    ));
    assert_eq!(admit(&tight, &[measured(1.0, 400)], None), Ok(()));
}

#[test]
fn refusal_round_trips_as_tagged_json() {
    let refusal = Refusal::Full { running: 3, max: 3 };
    let json = serde_json::to_value(&refusal).unwrap();
    assert_eq!(json["refused"], "full");
    assert_eq!(serde_json::from_value::<Refusal>(json).unwrap(), refusal);
}

#[test]
fn slot_ports_do_not_overlap() {
    let a = SlotPorts::for_slot(BASE_PORT, TUNNEL_PORT, AGENT_PORT, 0).unwrap();
    let b = SlotPorts::for_slot(BASE_PORT, TUNNEL_PORT, AGENT_PORT, 1).unwrap();
    let c = SlotPorts::for_slot(BASE_PORT, TUNNEL_PORT, AGENT_PORT, 2).unwrap();
    assert_eq!((a.base_port, a.tunnel_port, a.agent_port), (31000, 31100, 14720));
    assert_eq!((b.base_port, b.tunnel_port, b.agent_port), (31007, 31101, 14721));
    assert_eq!((c.base_port, c.tunnel_port), (31014, 31102));
    // The last slot's UDP range stays below the tunnel ports.
    assert!(c.base_port + UDP_WIDTH - 1 < CONTROL_PORT);
    assert!(SlotPorts::for_slot(65529, 1, 1, 0).is_some());
    assert!(SlotPorts::for_slot(65529, 1, 1, 1).is_none());
    assert!(SlotPorts::for_slot(0, 1, 1, 0).is_none());
}

// -- Routing --------------------------------------------------------------------

#[test]
fn first_lease_names_a_new_pack_and_later_ones_join_it() {
    let mut pool = Pool::default();
    assert_eq!(pool.place("hl-a", 3), Placement::New("pack-0".into()));
    pool.admitted("pack-0", "hl-a", 100);
    assert_eq!(pool.place("hl-b", 3), Placement::Existing("pack-0".into()));
    pool.admitted("pack-0", "hl-b", 100);
    assert_eq!(pool.place("hl-c", 3), Placement::Existing("pack-0".into()));
    pool.admitted("pack-0", "hl-c", 100);
    // Full: the fourth starts another container.
    assert_eq!(pool.place("hl-d", 3), Placement::New("pack-1".into()));
    assert_eq!(pool.leases("pack-0"), 3);
}

#[test]
fn a_placed_lease_keeps_its_pack() {
    let mut pool = Pool::default();
    let pack = pool.place("hl-a", 3);
    pool.admitted(pack.pack(), "hl-a", 100);
    assert_eq!(pool.place("hl-a", 3), Placement::Existing("pack-0".into()));
    assert_eq!(pool.pack_of("hl-a"), Some("pack-0"));
}

#[test]
fn routing_fills_the_busiest_pack_with_room() {
    let mut pool = Pool::default();
    pool.admitted("pack-0", "hl-a", 100);
    pool.admitted("pack-1", "hl-b", 100);
    pool.admitted("pack-1", "hl-c", 100);
    assert_eq!(pool.place("hl-d", 3), Placement::Existing("pack-1".into()));
    // On a tie the older pack wins, numerically (pack-10 is newer than pack-9).
    let mut pool = Pool::default();
    pool.admitted("pack-10", "hl-a", 100);
    pool.admitted("pack-9", "hl-b", 100);
    assert_eq!(pool.place("hl-c", 3), Placement::Existing("pack-9".into()));
}

#[test]
fn new_pack_names_are_never_reused() {
    let mut pool = Pool::default();
    assert_eq!(pool.place("hl-a", 1).pack(), "pack-0");
    pool.admitted("pack-0", "hl-a", 100);
    assert_eq!(pool.release("hl-a"), Some("pack-0".into()));
    assert_eq!(pool.place("hl-b", 1), Placement::New("pack-1".into()));
}

#[test]
fn a_refusal_marks_the_pack_full_until_a_lease_ends() {
    let mut pool = Pool::default();
    pool.admitted("pack-0", "hl-a", 100);
    pool.admitted("pack-0", "hl-b", 100);
    // The supervisor's measured load refused a third.
    assert!(!pool.refused("pack-0"));
    assert_eq!(pool.place("hl-c", 3), Placement::New("pack-1".into()));
    assert_eq!(pool.release("hl-a"), None);
    assert_eq!(pool.place("hl-c", 3), Placement::Existing("pack-0".into()));
}

#[test]
fn a_new_pack_that_refuses_its_first_match_is_dropped() {
    let mut pool = Pool::default();
    let placement = pool.place("hl-a", 3);
    assert!(pool.refused(placement.pack()));
    assert!(pool.is_empty());
}

// -- Teardown -------------------------------------------------------------------

#[test]
fn a_pack_stops_only_when_its_last_lease_ends() {
    let mut pool = Pool::default();
    pool.admitted("pack-0", "hl-a", 100);
    pool.admitted("pack-0", "hl-b", 100);
    assert_eq!(pool.release("hl-a"), None);
    assert_eq!(pool.packs().collect::<Vec<_>>(), ["pack-0"]);
    assert_eq!(pool.release("hl-b"), Some("pack-0".into()));
    assert!(pool.is_empty());
    // Releasing again, or an unknown lease, stops nothing.
    assert_eq!(pool.release("hl-b"), None);
    assert_eq!(pool.release("hl-x"), None);
}

#[test]
fn an_emptied_pack_takes_no_new_leases() {
    let mut pool = Pool::default();
    pool.admitted("pack-0", "hl-a", 100);
    assert_eq!(pool.release("hl-a"), Some("pack-0".into()));
    // The caller is stopping pack-0; a new lease must not be routed to it.
    assert_eq!(pool.place("hl-b", 3), Placement::New("pack-1".into()));
}

#[test]
fn expiry_drops_overdue_leases_and_reports_emptied_packs() {
    let mut pool = Pool::default();
    pool.admitted("pack-0", "hl-a", 100);
    pool.admitted("pack-1", "hl-b", 100);
    pool.admitted("pack-1", "hl-c", 300);
    assert_eq!(pool.deadline("pack-1"), Some(300));
    assert_eq!(pool.next_deadline(), Some(100));
    assert_eq!(pool.expire(99), Vec::<String>::new());
    assert_eq!(pool.expire(100), vec!["pack-0".to_owned()]);
    assert_eq!(pool.leases("pack-1"), 1);
    assert_eq!(pool.deadline("pack-1"), Some(300));
    assert_eq!(pool.next_deadline(), Some(300));
    assert_eq!(pool.expire(300), vec!["pack-1".to_owned()]);
    assert!(pool.is_empty());
    assert_eq!(pool.next_deadline(), None);
}

#[test]
fn forgetting_a_dead_pack_returns_its_leases() {
    let mut pool = Pool::default();
    pool.admitted("pack-0", "hl-a", 100);
    pool.admitted("pack-0", "hl-b", 100);
    let mut lost = pool.forget("pack-0");
    lost.sort();
    assert_eq!(lost, ["hl-a", "hl-b"]);
    assert_eq!(pool.pack_of("hl-a"), None);
}

#[test]
fn pool_round_trips_through_storage() {
    let mut pool = Pool::default();
    let p = pool.place("hl-a", 3);
    pool.admitted(p.pack(), "hl-a", 100);
    pool.admitted(p.pack(), "hl-b", 200);
    pool.refused(p.pack());
    let json = serde_json::to_string(&pool).unwrap();
    let back: Pool = serde_json::from_str(&json).unwrap();
    assert_eq!(back, pool);
    let mut back = back;
    assert_eq!(back.place("hl-c", 3), Placement::New("pack-1".into()));
}

#[test]
fn pack_names_are_checked() {
    assert!(valid_pack("pack-0"));
    assert!(valid_pack("pack-123"));
    for bad in ["pack-", "pack-x", "pack-1/2", "hl-a", "pack-123456789012345678901"] {
        assert!(!valid_pack(bad), "{bad}");
    }
}

// -- Wire ---------------------------------------------------------------------

#[test]
fn http_request_round_trips_and_waits_for_the_whole_body() {
    let bytes = request("POST", MATCHES_PATH, br#"{"a":1}"#);
    for cut in 0..bytes.len() {
        assert_eq!(parse_request(&bytes[..cut]), Parse::Partial, "{cut}");
    }
    assert_eq!(
        parse_request(&bytes),
        Parse::Done(super::http::Request {
            method: "POST".into(),
            path: MATCHES_PATH.into(),
            body: br#"{"a":1}"#.to_vec(),
        })
    );
}

#[test]
fn http_rejects_what_the_wire_does_not_speak() {
    assert!(matches!(
        parse_request(b"GET /x HTTP/1.1\r\ntransfer-encoding: chunked\r\n\r\n"),
        Parse::Invalid(_)
    ));
    assert!(matches!(
        parse_request(b"GET x HTTP/1.1\r\n\r\n"),
        Parse::Invalid(_)
    ));
    assert!(matches!(
        parse_request(b"GET /x HTTP/1.1\r\ncontent-length: 999999\r\n\r\n"),
        Parse::Invalid(_)
    ));
    assert!(matches!(
        parse_request(&vec![b'a'; super::http::MAX_HEAD + 1]),
        Parse::Invalid(_)
    ));
}

#[test]
fn http_response_round_trips() {
    let bytes = response(409, br#"{"refused":"draining"}"#);
    assert_eq!(
        parse_response(&bytes),
        Parse::Done((409, br#"{"refused":"draining"}"#.to_vec()))
    );
    assert_eq!(parse_response(&response(204, b"")), Parse::Done((204, vec![])));
    assert_eq!(parse_response(&bytes[..10]), Parse::Partial);
}

#[test]
fn add_match_omits_an_absent_region() {
    let add = AddMatch {
        lease: "hl-a".into(),
        lease_json: "{}".into(),
        live: "https://live".into(),
        token: "t".into(),
        data_url: "https://data".into(),
        data_sha: "ab".into(),
        archive: crate::hosts::HostArchive::Film,
        region: None,
    };
    let mut json = serde_json::to_value(&add).unwrap();
    assert!(json.get("region").is_none());
    assert_eq!(json["archive"], "film");
    json.as_object_mut().unwrap().remove("archive");
    let old: AddMatch = serde_json::from_value(json).unwrap();
    assert_eq!(old.archive, crate::hosts::HostArchive::Dual);
    let status = PackStatus {
        limits: small(),
        matches: vec![MatchStatus {
            lease: "hl-a".into(),
            slot: 0,
            alive: true,
            load: measured(6.5, 300),
        }],
        container_mib: Some(700),
        draining: false,
    };
    let back: PackStatus =
        serde_json::from_str(&serde_json::to_string(&status).unwrap()).unwrap();
    assert_eq!(back, status);
}
