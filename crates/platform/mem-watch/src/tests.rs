use super::*;

#[test]
fn parses_binary_sizes_and_rejects_overflow_and_bad_units() {
    for (raw, expected) in [
        ("0", 0),
        ("12G", 12 << 30),
        (" 4mib ", 4 << 20),
        ("82B", 82),
    ] {
        assert_eq!(parse_size(raw).unwrap(), expected);
    }
    for raw in ["G", "-1", "4MB/s", "12.5", "18446744073709551615K"] {
        assert!(parse_size(raw).is_err());
    }
    assert_eq!(Limits::defaults(Some(10 << 30)).hard, 4 << 30);
    assert_eq!(Limits::defaults(Some(64 << 30)).hard, 16 << 30);
}

#[test]
fn soft_edges_rearm_and_hard_has_precedence() {
    let mut h = History::default();
    let limits = Limits { soft: 4, hard: 8 };
    for (ms, (bytes, expected)) in [
        (Some(4), None),
        (Some(6), Some(Trigger::Soft)),
        (None, None),
        (Some(7), None),
        (Some(3), None),
        (Some(8), Some(Trigger::Soft)),
        (Some(9), Some(Trigger::Hard)),
    ]
    .into_iter()
    .enumerate()
    {
        assert_eq!(h.poll(limits, || ms as u64 * 2000, || bytes), expected);
    }
    assert_eq!(h.peak, 9);
    assert_eq!(h.samples.len(), 6);
}

#[test]
fn growth_requires_three_minutes_and_history_remains_bounded() {
    let mut h = History::default();
    let limits = Limits {
        soft: u64::MAX,
        hard: 0,
    };
    for tick in 0..1000 {
        let expected = (tick == 90).then_some(Trigger::Growth);
        assert_eq!(
            h.poll(limits, || tick * 2000, || Some(tick * (8 << 20))),
            expected
        );
    }
    assert_eq!(h.samples.len(), SAMPLES);
    assert_eq!(h.samples[0].0, 700 * 2000);
    assert_eq!(h.growth, (240 << 20) as f64);
    let mut spike = History::default();
    for tick in 0..=100 {
        assert_eq!(
            spike.poll(limits, || tick * 2000, || Some(u64::from(tick > 0) << 30)),
            None
        );
    }
}

#[test]
fn report_contains_provenance_limits_full_series_and_probes() {
    let dir = std::env::temp_dir().join(format!(
        "h2-watch-report-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let watch = Watch {
        since: Instant::now(),
        limits: Limits {
            soft: 128,
            hard: 256,
        },
        history: Mutex::default(),
        dir: dir.clone(),
        commit: Some("fixture".into()),
        reports: Mutex::default(),
    };
    register_probe("test.fixture_bytes", || 27);
    for n in 0..80 {
        watch
            .history
            .lock()
            .unwrap()
            .poll(watch.limits, || n * 2000, || Some(n));
    }
    let report = Report {
        pid: 12,
        argv: vec!["fixture".into()],
        build_commit: watch.commit.clone(),
        uptime_ms: 160000,
        unix_ms: 1234,
        job_id: Some("0044".into()),
        job_name: Some("example".into()),
        reason: "soft limit".into(),
        memory: watch.snapshot(SAMPLES),
        diagnostics: BTreeMap::new(),
    };
    fs::create_dir(&dir).unwrap();
    let file = dir.join("fixture.json");
    save(&file, &report).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
    assert_eq!(v["pid"], 12);
    assert_eq!(v["samples"].as_array().unwrap().len(), 80);
    assert_eq!(v["argv"][0], "fixture");
    assert_eq!(v["probes"]["test.fixture_bytes"], 27);
    assert_eq!(v["job_name"], "example");
    assert_eq!(v["build_commit"], "fixture");
    assert_eq!(v["peak_bytes"], 79);
    assert_eq!(v["soft"], 128);
    assert_eq!(v["hard"], 256);
    assert_eq!(watch.snapshot(60).samples[0], (40000, 20));
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_recorder_row_carries_context_activity_tags_and_the_soft_limit() {
    let watch = Watch {
        since: Instant::now(),
        limits: Limits {
            soft: 512,
            hard: 1024,
        },
        history: Mutex::default(),
        dir: std::env::temp_dir(),
        commit: None,
        reports: Mutex::default(),
    };
    register_game_context(|| (Some("fixture_map".into()), Some(90)));
    let connection = agent_activity::connection();
    agent_activity::served();
    let row = current_record(&watch);
    assert_eq!(row.pid, std::process::id());
    assert_eq!(row.soft, 512);
    assert_eq!(row.level.as_deref(), Some("fixture_map"));
    assert_eq!(row.tick, Some(90));
    assert!(row.agent_connections >= 1);
    assert!(row.agent_calls >= 1);
    assert_eq!(
        row.probes.get(agent_activity::CONNECTIONS_PROBE),
        Some(&row.agent_connections)
    );
    // The always-on tags exist even when nothing was allocated in a scope.
    assert!(row.tags.contains_key("unscoped"));
    assert!(row.stacks.is_empty(), "sampling is off in tests");
    drop(connection);
    // Without a recorder in this process, live reads say so.
    assert!(history_page(None, 1, 1024).is_err());
    assert!(diff_since(0).is_err());
}
