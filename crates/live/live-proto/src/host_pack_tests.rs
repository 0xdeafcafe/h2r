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
    class_limits(&crate::hosts::HostClass::Small, None)
}

// -- Admission ----------------------------------------------------------------

#[test]
fn budget_is_seventy_percent_of_a_tick_per_vcpu() {
    assert!((small().tick_budget_ms() - 23.333).abs() < 0.01);
    assert!((Limits::new(3, 2.0, 6144).tick_budget_ms() - 46.667).abs() < 0.01);
    assert_eq!(small().memory_budget_mib(), 1536 - 256);
}

#[test]
fn empty_pack_admits_its_first_match() {
    assert_eq!(admit(&small(), &[], None), Ok(()));
    assert_eq!(admit(&small(), &[], Some(40)), Ok(()));
}

#[test]
fn a_measured_five_ms_map_leaves_room_for_a_fourth_match() {
    // Three measured matches plus one new one cost 20 ms of the 23.3 ms budget.
    let running = [measured(5.0, 170); 3];
    let estimate = Limits { new_ms_per_tick: 5.0, new_rss_mib: 170, ..small() };
    assert_eq!(admit(&estimate, &running, Some(800)), Ok(()));
}

#[test]
fn refuses_when_summed_tick_cost_passes_the_budget() {
    // 10 + 10 + 10 = 30 ms > 23.3 ms.
    let running = [measured(10.0, 170); 2];
    match admit(&small(), &running, Some(800)) {
        Err(Refusal::Cpu {
            projected_ms,
            budget_ms,
        }) => {
            assert!((projected_ms - 30.0).abs() < 1e-9);
            assert!((budget_ms - 23.333).abs() < 0.01);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn wall_p99_refuses_a_late_pack_before_cpu_and_memory() {
    let mut load = measured(1.0, 170);
    load.wall_ms_per_tick_p99 = Some(36.0);
    assert_eq!(admit(&small(), &[load], None), Ok(()));
    load.ms_per_tick = Some(100.0);
    load.wall_ms_per_tick_p99 = Some(36.1);
    assert_eq!(admit(&small(), &[load], Some(1536)), Err(Refusal::Late {
        wall_ms_per_tick_p99: 36.1,
    }));
}

#[test]
fn unmeasured_matches_count_at_the_default_cost() {
    assert_eq!((small().new_ms_per_tick, small().new_rss_mib), (10.0, 340));
    // Two vCPUs can admit a fourth cold match. One vCPU cannot.
    let running = [MatchLoad::default(); 3];
    assert!(matches!(admit(&small(), &running, None), Err(Refusal::Cpu { .. })));
    let large = class_limits(&crate::hosts::HostClass::Large, None);
    assert_eq!(admit(&large, &running, None), Ok(()));
}

#[test]
fn refuses_past_the_match_cap_even_with_cpu_to_spare() {
    let running = [measured(1.0, 100); 5];
    assert_eq!(
        admit(&small(), &running, Some(300)),
        Err(Refusal::Full { running: 5, max: 5 })
    );
}

#[test]
fn refuses_when_memory_would_pass_the_container_limit() {
    let running = [measured(5.0, 340)];
    // 1200 measured + 340 assumed > 1536 - 256.
    assert_eq!(
        admit(&small(), &running, Some(1200)),
        Err(Refusal::Memory {
            projected_mib: 1540,
            budget_mib: 1280
        })
    );
    // Without a container figure, the matches' own RSS is summed.
    let tight = Limits::new(3, 4.0, 1024);
    assert!(matches!(
        admit(&tight, &[measured(1.0, 650)], None),
        Err(Refusal::Memory { projected_mib: 990, budget_mib: 768 })
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

#[test]
fn search_warms_one_pack_and_silence_retires_it() {
    let mut pool = Pool::default();
    let demand = WarmDemand { searchers: 1, leases: 0, available: 4 };
    let start = pool.warm_plan(demand, 5, 0).start.unwrap();
    pool.warmed(&start, 1000, Some(1000));
    assert_eq!(pool.free_slots(5), 5);
    assert!(pool.warm_plan(demand, 5, 2_000).start.is_none());
    let refresh = pool.warm_plan(demand, 5, 21_000);
    assert_eq!(refresh.refresh.as_slice(), std::slice::from_ref(&start));
    pool.warmed(&start, 21_000, None);
    let silence = pool.warm_plan(WarmDemand::default(), 5, 30_000);
    assert!(silence.refresh.is_empty() && silence.start.is_none());
    assert!(pool.expire(80_999).is_empty());
    assert_eq!(pool.expire(81_000), [start]);
    assert!(pool.is_empty());
}

#[test]
fn demand_uses_recent_arrivals_and_the_observed_median_start() {
    let mut pool = Pool::default();
    let demand = WarmDemand { searchers: 1, leases: 99, available: 2 };
    for t in 0..100 {
        pool.admitted_at("pack-0", &format!("lease-{t}"), 1_000_000, t);
    }
    // 100 arrivals in five minutes, median 3s: ceil(100 × 3 × 2 / 300) = 2.
    for elapsed in [500, 3000, 20_000] {
        pool.warmed("pack-0", 100, Some(elapsed));
    }
    assert!(pool.warm_plan(demand, 101, 100).start.is_some(), "one free, want two");
    assert!(pool.warm_plan(WarmDemand { available: 1, ..demand }, 101, 100).start.is_none());
    assert!(pool.warm_plan(demand, 101, 300_000).start.is_none(), "history has expired");
}

#[test]
fn occupied_capacity_is_used_before_an_idle_hold() {
    let mut pool = Pool::default();
    pool.admitted_at("pack-0", "lease", 1_000_000, 0);
    let empty = pool.place("another", 1);
    pool.warmed(empty.pack(), 0, None);
    let demand = WarmDemand { searchers: 1, leases: 1, available: 1 };
    let plan = pool.warm_plan(demand, 3, 55_000);
    assert!(plan.start.is_none() && plan.refresh.is_empty());
    pool.release_at("lease", 55_000);
    assert_eq!(pool.next_deadline(), Some(60_000));
    assert_eq!(pool.expire(60_000), [empty.pack()]);
    assert_eq!(pool.place("later", 3), Placement::Existing("pack-0".into()));
}

#[test]
fn only_the_release_that_empties_the_pack_owns_its_linger() {
    use crate::hosts::PackRelease;
    for order in [["a", "b"], ["b", "a"]] {
        let mut pool = Pool::default();
        let pack = pool.place("a", 2).pack().to_owned();
        pool.admitted_at(&pack, "a", 120_000, 0);
        pool.admitted_at(&pack, "b", 120_000, 0);
        assert_eq!(pool.release_at(order[0], 100), PackRelease::NoLinger);
        assert_eq!(pool.release_at(order[1], 101), PackRelease::Linger { released_ms: 101 });
        assert_eq!(pool.release_at(order[1], 102), PackRelease::NoLinger);
        assert_eq!(pool.next_deadline(), Some(101 + LINGER_MS));
    }
}
