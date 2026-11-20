use super::measure::{
    Usage, parse_cpu_max, parse_memory_max, parse_stat, used_mib,
};
use super::*;

fn add(lease: &str) -> AddMatch {
    AddMatch {
        lease: lease.into(),
        lease_json: json!({ "id": lease, "map": "fixture-a" }).to_string(),
        live: "http://127.0.0.1:8074".into(),
        token: "lease-token".into(),
        data_url: "https://data.example/bundle?sig=a".into(),
        data_sha: "ab".repeat(32),
        archive: live_proto::hosts::HostArchive::Dual,
        region: None,
    }
}

/// A pack whose matches are `sleep`s in their own process groups.
fn pack(limits: Limits) -> (Pack, Arc<Mutex<Vec<(String, u16, SlotPorts)>>>) {
    let launched = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&launched);
    let launch: Launch = Box::new(move |add: &AddMatch, slot: u16, ports: SlotPorts| {
        record.lock().unwrap().push((add.lease.clone(), slot, ports));
        Command::new("sh")
            .args(["-c", "exec sleep 30"])
            .stdin(Stdio::null())
            .process_group(0)
            .spawn()
    });
    let pack = Pack::new(
        limits,
        (host_pack::BASE_PORT, host_pack::TUNNEL_PORT, host_pack::AGENT_PORT),
        Duration::from_secs(60),
        launch,
        Instant::now(),
    );
    (pack, launched)
}

fn roomy() -> Limits {
    Limits::new(3, 8.0, 64 * 1024)
}

fn usage(cpu_ms: f64) -> Usage {
    Usage {
        cpu_ms,
        memory: Some(live_proto::host_pack::memory::ResidentMemory {
            pss: 300 << 20,
            private: 100 << 20,
            shared: 500 << 20,
            private_dirty: 95 << 20,
            shared_clean: 450 << 20,
        }),
    }
}

// -- Options --------------------------------------------------------------------

#[test]
fn options_default_to_five_matches_on_the_container_ports() {
    let opts = Options::parse_with(std::iter::empty::<String>(), |_| None).unwrap();
    assert_eq!(opts.max_matches, 5);
    assert_eq!(opts.control.port(), host_pack::CONTROL_PORT);
    assert_eq!(opts.tunnel.port(), host_pack::TUNNEL_PORT);
    assert_eq!(opts.base_port, host_pack::BASE_PORT);
    assert_eq!(opts.linger, Duration::from_secs(60));
    assert!(opts.vcpus.is_none() && opts.memory_mib.is_none());
}

#[test]
fn options_read_the_environment_and_prefer_flags() {
    let env = |name: &str| {
        Some(
            match name {
                "H2_PACK_MAX_MATCHES" => "2",
                "H2_PACK_VCPUS" => "1",
                "H2_PACK_MEMORY_MIB" => "3072",
                "H2_PACK_CONTROL" => "127.0.0.1:41099",
                _ => return None,
            }
            .to_owned(),
        )
    };
    let args = ["--max-matches", "4", "--linger-s", "5"].map(str::to_owned);
    let opts = Options::parse_with(args.into_iter(), env).unwrap();
    assert_eq!(opts.max_matches, 4);
    assert_eq!(opts.vcpus, Some(1.0));
    assert_eq!(opts.memory_mib, Some(3072));
    assert_eq!(opts.control.to_string(), "127.0.0.1:41099");
    assert_eq!(opts.linger, Duration::from_secs(5));
    let limits = opts.limits();
    assert_eq!((limits.max_matches, limits.vcpus, limits.memory_mib), (4, 1.0, 3072));
}

#[test]
fn options_reject_bad_values() {
    for args in [
        &["--max-matches", "0"][..],
        &["--max-matches", "11"],
        &["--vcpus", "-1"],
        &["--bogus", "1"],
        &["--max-matches", "3", "--max-matches", "3"],
        // The last slot's UDP range would pass 65535.
        &["--base-port", "65520"],
    ] {
        let owned = args.iter().map(|s| s.to_string());
        assert!(Options::parse_with(owned, |_| None).is_err(), "{args:?}");
    }
}

// -- Measurement ----------------------------------------------------------------

#[test]
fn parses_proc_and_cgroup_files() {
    let stat = "4242 (h2 (host) run) S 1 4242 4242 0 -1 4194560 100 0 0 0 250 50 0 0 20 0 9 0";
    assert_eq!(parse_stat(stat), Some((4242, 300)));
    assert_eq!(parse_stat("garbage"), None);
    assert_eq!(parse_cpu_max("200000 100000\n"), Some(2.0));
    assert_eq!(parse_cpu_max("max 100000\n"), None);
    assert_eq!(parse_memory_max("3221225472\n"), Some(3072));
    assert_eq!(parse_memory_max("max\n"), None);
    assert_eq!(
        used_mib("1073741824\n", "anon 1\ninactive_file 536870912\nactive_file 2\n"),
        Some(512)
    );
}

#[test]
fn meter_reports_cpu_per_tick_after_warmup() {
    let start = Instant::now();
    let mut meter = Meter::default();
    // 240 ms of CPU per second of wall time is 8 ms per 33.3 ms tick.
    for i in 0..=2u32 {
        meter.observe(start + Duration::from_secs(u64::from(i)), usage(240.0 * f64::from(i)));
    }
    assert_eq!(meter.load().ms_per_tick, None, "two intervals are still warmup");
    assert_eq!(meter.load().rss_mib, Some(300));
    meter.observe(start + Duration::from_secs(3), usage(720.0));
    let ms = meter.load().ms_per_tick.unwrap();
    assert!((ms - 8.0).abs() < 1e-6, "{ms}");
    // A burst shows at once; the smoothed value then decays.
    meter.observe(start + Duration::from_secs(4), usage(720.0 + 480.0));
    let burst = meter.load().ms_per_tick.unwrap();
    assert!((burst - 16.0).abs() < 1e-6, "{burst}");
    meter.observe(start + Duration::from_secs(5), usage(1200.0 + 240.0));
    let after = meter.load().ms_per_tick.unwrap();
    assert!(after > 8.0 && after < 16.0, "{after}");
}

// -- Admission and teardown -----------------------------------------------------

fn gameplay_sample(pack: &mut Pack, p99: f64) {
    for slot in pack.slots.iter_mut().flatten() {
        let reading = serde_json::from_value(json!({ "wall_ms_per_tick_p99": p99 })).unwrap();
        slot.meter.observe_agent(Instant::now(), Some(reading));
    }
}

#[test]
fn a_late_match_pauses_admission_until_its_tick_window_recovers() {
    let (mut pack, _) = pack(roomy());
    pack.add(&add("hl-a")).unwrap();
    gameplay_sample(&mut pack, 39.0);
    assert!(matches!(pack.add(&add("hl-b")), Err(AddError::Refused(Refusal::Late { .. }))));
    gameplay_sample(&mut pack, 34.0);
    assert!(pack.add(&add("hl-b")).is_ok());
    pack.shutdown();
}

#[test]
fn admits_up_to_the_cap_with_disjoint_ports_and_refuses_the_next() {
    let (mut pack, launched) = pack(roomy());
    let a = pack.add(&add("hl-a")).unwrap();
    let b = pack.add(&add("hl-b")).unwrap();
    let c = pack.add(&add("hl-c")).unwrap();
    assert_eq!((a.slot, b.slot, c.slot), (0, 1, 2));
    assert_eq!(
        (a.ports.tunnel_port, b.ports.tunnel_port, c.ports.tunnel_port),
        (31100, 31101, 31102)
    );
    assert!(matches!(
        pack.add(&add("hl-d")),
        Err(AddError::Refused(Refusal::Full { running: 3, max: 3 }))
    ));
    // Adding a running lease again is idempotent and launches nothing.
    assert_eq!(pack.add(&add("hl-b")).unwrap(), b);
    assert_eq!(launched.lock().unwrap().len(), 3);
    pack.shutdown();
}

#[test]
fn measured_cpu_refuses_a_match_that_would_pass_the_budget() {
    // A third fixture-a match is estimated from the first two: 3 × 9 > 23.3 ms.
    let (mut pack, _) = pack(Limits::new(3, 1.0, 64 * 1024));
    pack.add(&add("hl-a")).unwrap();
    pack.add(&add("hl-b")).unwrap();
    gameplay_sample(&mut pack, 33.4);
    let start = Instant::now();
    for i in 0..=4u32 {
        let at = start + Duration::from_secs(u64::from(i));
        pack.tick(at, &|_| Some(usage(270.0 * f64::from(i))), Some(1000));
    }
    let loads: Vec<_> = pack.status().matches.iter().map(|m| m.load.ms_per_tick).collect();
    assert!(loads.iter().all(|ms| ms.is_some_and(|ms| (ms - 9.0).abs() < 1e-6)), "{loads:?}");
    assert!(matches!(
        pack.add(&add("hl-c")),
        Err(AddError::Refused(Refusal::Cpu { .. }))
    ));
    // No history for fixture-b: the 10 ms cold estimate also exceeds the margin.
    let mut other = add("hl-c");
    other.lease_json = json!({ "id": other.lease, "map": "fixture-b" }).to_string();
    assert!(matches!(pack.add(&other), Err(AddError::Refused(Refusal::Cpu { .. }))));
    pack.shutdown();
}

#[test]
fn three_five_ms_matches_leave_room_for_a_fourth() {
    let (mut pack, _) = pack(host_pack::class_limits(&live_proto::hosts::HostClass::Small, None));
    pack.add(&add("hl-a")).unwrap();
    pack.add(&add("hl-b")).unwrap();
    gameplay_sample(&mut pack, 33.4);
    let start = Instant::now();
    for i in 0..=4u32 {
        let at = start + Duration::from_secs(u64::from(i));
        pack.tick(at, &|_| Some(usage(150.0 * f64::from(i))), Some(650));
    }
    // Both additional leases inherit the observed map cost.
    pack.add(&add("hl-c")).unwrap();
    assert!(pack.add(&add("hl-d")).is_ok());
    pack.shutdown();
}

#[test]
fn container_memory_refuses_a_match_that_would_not_fit() {
    let (mut pack, _) = pack(Limits::new(3, 8.0, 3072));
    pack.add(&add("hl-a")).unwrap();
    pack.tick(Instant::now(), &|_| None, Some(2700));
    assert!(matches!(
        pack.add(&add("hl-b")),
        Err(AddError::Refused(Refusal::Memory { .. }))
    ));
    pack.shutdown();
}

#[test]
fn rapid_adds_reserve_memory_until_the_children_fault_it() {
    let (mut pack, _) = pack(Limits::new(5, 2.0, 1024));
    pack.tick(Instant::now(), &|_| None, Some(60));
    pack.add(&add("hl-a")).unwrap();
    pack.add(&add("hl-b")).unwrap();
    assert!(matches!(
        pack.add(&add("hl-c")),
        Err(AddError::Refused(Refusal::Memory { projected_mib: 1080, budget_mib: 768 }))
    ));
    pack.shutdown();
}

#[test]
fn removing_frees_the_slot_once_reaped_and_the_pack_lingers_then_exits() {
    let (mut pack, _) = pack(roomy());
    let start = Instant::now();
    pack.add(&add("hl-a")).unwrap();
    pack.add(&add("hl-b")).unwrap();
    assert!(!pack.should_exit(start + Duration::from_secs(3600)));
    pack.remove("hl-a", start);
    // Stopping, still in its slot.
    assert!(!pack.find("hl-a").unwrap().1.alive());
    assert!(matches!(pack.add(&add("hl-a")), Err(AddError::Invalid(_))));
    // Wait for TERM to reap it.
    let mut reaped = false;
    for _ in 0..100 {
        pack.tick(Instant::now(), &|_| None, None);
        if pack.find("hl-a").is_none() {
            reaped = true;
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    assert!(reaped);
    // Its slot is the next one used.
    assert_eq!(pack.add(&add("hl-c")).unwrap().slot, 0);
    pack.remove("hl-b", Instant::now());
    pack.remove("hl-c", Instant::now());
    let mut idle_at = None;
    for _ in 0..100 {
        let now = Instant::now();
        pack.tick(now, &|_| None, None);
        if pack.status().matches.is_empty() {
            idle_at = Some(now);
            break;
        }
        thread::sleep(Duration::from_millis(50));
    }
    let idle_at = idle_at.expect("both matches stop");
    assert!(!pack.should_exit(idle_at + Duration::from_secs(59)));
    assert!(pack.should_exit(idle_at + Duration::from_secs(60)));
    // Removing an unknown or gone lease is harmless.
    pack.remove("hl-zz", Instant::now());
}

#[test]
fn a_match_that_ends_by_itself_frees_its_slot() {
    let launch: Launch = Box::new(|_: &AddMatch, _: u16, _: SlotPorts| {
        Command::new("true").process_group(0).stdin(Stdio::null()).spawn()
    });
    let mut pack = Pack::new(roomy(), (31000, 31100, 14720), Duration::from_secs(1), launch, Instant::now());
    pack.add(&add("hl-a")).unwrap();
    for _ in 0..100 {
        pack.tick(Instant::now(), &|_| None, None);
        if pack.find("hl-a").is_none() {
            return;
        }
        thread::sleep(Duration::from_millis(20));
    }
    panic!("the finished match kept its slot");
}

#[test]
fn rejects_a_lease_body_for_another_lease() {
    let (mut pack, launched) = pack(roomy());
    let mut wrong = add("hl-a");
    wrong.lease_json = json!({ "id": "hl-b" }).to_string();
    assert!(matches!(pack.add(&wrong), Err(AddError::Invalid(_))));
    let mut bad = add("hl-a");
    bad.lease = "../x".into();
    assert!(matches!(pack.add(&bad), Err(AddError::Invalid(_))));
    assert!(launched.lock().unwrap().is_empty());
}

// -- Control requests -------------------------------------------------------------

fn req(method: &str, path: &str, body: &[u8]) -> http::Request {
    http::Request {
        method: method.into(),
        path: path.into(),
        body: body.to_vec(),
    }
}

#[test]
fn control_routes_add_status_and_remove() {
    let (mut pack, _) = pack(Limits::new(1, 8.0, 64 * 1024));
    let now = Instant::now();
    let body = serde_json::to_vec(&add("hl-a")).unwrap();
    let (status, added) = handle(&mut pack, &req("POST", host_pack::MATCHES_PATH, &body), now);
    assert_eq!(status, 200);
    let added: Added = serde_json::from_slice(&added).unwrap();
    assert_eq!(added.slot, 0);

    let body = serde_json::to_vec(&add("hl-b")).unwrap();
    let (status, refused) = handle(&mut pack, &req("POST", host_pack::MATCHES_PATH, &body), now);
    assert_eq!(status, 409);
    assert_eq!(
        serde_json::from_slice::<Refusal>(&refused).unwrap(),
        Refusal::Full { running: 1, max: 1 }
    );

    let (status, one) = handle(&mut pack, &req("GET", "/v1/pack/matches/hl-a", b""), now);
    assert_eq!(status, 200);
    let one: MatchStatus = serde_json::from_slice(&one).unwrap();
    assert!(one.alive);
    assert_eq!(handle(&mut pack, &req("GET", "/v1/pack/matches/hl-b", b""), now).0, 404);

    let (status, all) = handle(&mut pack, &req("GET", host_pack::STATUS_PATH, b""), now);
    assert_eq!(status, 200);
    let all: PackStatus = serde_json::from_slice(&all).unwrap();
    assert_eq!(all.matches.len(), 1);
    assert_eq!(all.matches[0].load.cpu_seconds, None);
    let slot = pack.slots[0].as_mut().unwrap();
    slot.meter.observe_agent(now, Some(serde_json::from_value(json!({
        "wall_ms_per_tick_p99": 37.0, "late_ticks": 12
    })).unwrap()));
    pack.tick(now, &|_| Some(usage(1250.0)), None);
    let (_, body) = handle(&mut pack, &req("GET", host_pack::STATUS_PATH, b""), now);
    let current: PackStatus = serde_json::from_slice(&body).unwrap();
    let load = current.matches[0].load;
    assert_eq!(load.wall_ms_per_tick_p99, Some(37.0));
    assert_eq!(load.late_ticks, Some(12));
    assert_eq!(load.cpu_seconds, Some(1.25));
    assert_eq!(load.pss_bytes, Some(300 << 20));
    assert_eq!(load.peak_pss_bytes, load.pss_bytes);
    assert_eq!(load.private_bytes, Some(100 << 20));
    assert_eq!(load.shared_bytes, Some(500 << 20));
    assert_eq!(all.limits.max_matches, 1);

    assert_eq!(handle(&mut pack, &req("DELETE", "/v1/pack/matches/hl-a", b""), now).0, 204);
    let (_, one) = handle(&mut pack, &req("GET", "/v1/pack/matches/hl-a", b""), now);
    assert!(!serde_json::from_slice::<MatchStatus>(&one).unwrap().alive);
    // Idempotent.
    assert_eq!(handle(&mut pack, &req("DELETE", "/v1/pack/matches/hl-a", b""), now).0, 204);

    assert_eq!(handle(&mut pack, &req("POST", host_pack::MATCHES_PATH, b"{"), now).0, 400);
    assert_eq!(handle(&mut pack, &req("GET", "/v1/pack/matches/../x", b""), now).0, 404);
    assert_eq!(handle(&mut pack, &req("PUT", "/elsewhere", b""), now).0, 404);
    pack.shutdown();
}

#[test]
fn explicit_public_ports_shift_with_the_slot() {
    assert_eq!(slot_public_addr(None, 31000, 31007).unwrap(), None);
    assert_eq!(
        slot_public_addr(Some("203.0.113.5"), 31000, 31007).unwrap().as_deref(),
        Some("203.0.113.5")
    );
    assert_eq!(
        slot_public_addr(Some("203.0.113.5:41000"), 31000, 31014).unwrap().as_deref(),
        Some("203.0.113.5:41014")
    );
    assert!(slot_public_addr(Some("203.0.113.5:65530"), 31000, 31007).is_err());
}

#[test]
fn warm_reservation_and_first_match_share_one_start() {
    let (mut pack, _) = pack(roomy());
    let warm = req("POST", host_pack::WARM_PATH, b"");
    assert_eq!(handle(&mut pack, &warm, Instant::now()).0, 204);
    pack.add(&add("hl-first")).unwrap();
    assert!(!pack.usage("hl-first", Instant::now()).unwrap().charge_start);
    pack.shutdown();
}

#[test]
fn occupancy_divides_only_time_with_matches() {
    let start = Instant::now();
    let mut clock = Occupancy { changed: start, per_slot: Duration::ZERO };
    let at = |s| start + Duration::from_secs(s);
    clock.advance(at(10), 0); // Empty warm pack.
    clock.advance(at(12), 1);
    let second_start = clock.per_slot;
    clock.advance(at(18), 2);
    let first = clock.per_slot;
    let second = clock.at(at(22), 1) - second_start;
    assert_eq!(first, Duration::from_secs(5));
    assert_eq!(second, Duration::from_secs(7));
    assert_eq!(first + second, at(22) - at(10));
}

#[test]
fn warm_pulse_renews_an_empty_pack_without_starting_a_match() {
    let (mut pack, launches) = pack(roomy());
    let start = pack.idle_since.unwrap();
    let pulse = start + Duration::from_secs(59);
    let request = http::Request {
        method: "POST".into(), path: host_pack::WARM_PATH.into(), body: Vec::new(),
    };
    assert_eq!(handle(&mut pack, &request, pulse).0, 204);
    assert!(launches.lock().unwrap().is_empty());
    assert!(!pack.should_exit(pulse + Duration::from_secs(59)));
    assert!(pack.should_exit(pulse + Duration::from_secs(60)));
}
