use super::*;
use std::time::{SystemTime, UNIX_EPOCH};

struct Temp(PathBuf);

impl Temp {
    fn new(name: &str) -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir =
            std::env::temp_dir().join(format!("h2-recorder-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }
}

impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn row(pid: u32, t_ms: u64, footprint: u64, tag: u64) -> Record {
    Record {
        pid,
        t_ms,
        unix_ms: 1_000_000 + t_ms,
        footprint_bytes: footprint,
        soft: 1 << 30,
        tags: BTreeMap::from([("assets.bitmap".into(), tag), ("unscoped".into(), 7)]),
        gpu: BTreeMap::from([("wgpu.texture_count".into(), tag / 10)]),
        probes: BTreeMap::from([("agent.connections".into(), 1)]),
        level: Some("01b_spacestation".into()),
        tick: Some(t_ms / 33),
        agent_connections: 1,
        ..Record::default()
    }
}

fn open(dir: &Path, pid: u32) -> Recorder {
    archive::open(dir, pid, archive::ARCHIVE_BYTES).unwrap()
}

#[test]
fn rows_page_in_order_across_an_hourly_rotation() {
    let t = Temp::new("rotate");
    let mut r = open(&t.0, 41);
    for (t_ms, bytes) in [(0, 100), (10_000, 200), (HOUR, 300), (HOUR + 10_000, 400)] {
        r.append(&row(41, t_ms, bytes, bytes)).unwrap();
    }
    assert!(segment(&t.0, 41, 1).is_file(), "the first hour rotated");
    let all = r.history(None).unwrap();
    let times: Vec<_> = all.iter().map(|r| r.t_ms).collect();
    assert_eq!(times, [0, 10_000, HOUR, HOUR + 10_000]);
    // A page stops at its row limit; the caller continues after its last row.
    let page = r.history_page(Some(5_000), 2, usize::MAX).unwrap();
    assert_eq!(
        page.iter().map(|r| r.t_ms).collect::<Vec<_>>(),
        [10_000, HOUR]
    );
    // A byte limit below one row returns nothing rather than a partial row.
    assert!(r.history_page(None, 10, 10).unwrap().is_empty());
    // A record for another process is refused.
    assert!(r.append(&row(42, HOUR + 20_000, 1, 1)).is_err());
}

#[test]
fn diff_ranks_growth_per_unit_and_rejects_times_outside_history() {
    let t = Temp::new("diff");
    let mut r = open(&t.0, 43);
    r.append(&row(43, 0, 1_000, 50)).unwrap();
    r.append(&row(43, 10_000, 2_000, 500)).unwrap();
    r.append(&row(43, 20_000, 9_000, 5_000)).unwrap();
    let d = r.diff_since(10_000).unwrap();
    assert_eq!(
        (d.from_ms, d.to_ms, d.footprint_delta),
        (10_000, 20_000, 7_000)
    );
    let first = &d.growers[0];
    assert_eq!(
        (first.category.as_str(), first.name.as_str()),
        ("tag", "assets.bitmap")
    );
    assert_eq!((first.unit.as_str(), first.delta), ("bytes", 4_500));
    // Unchanged counters are not growers; counts sort after bytes.
    assert!(!d.growers.iter().any(|c| c.name == "unscoped"));
    assert_eq!(d.growers.last().unwrap().unit, "count");
    assert!(r.diff_since(30_000).is_err());
}

#[test]
fn a_killed_writer_is_recovered_and_explained_and_a_clean_one_is_removed() {
    let t = Temp::new("recover");
    {
        let mut killed = open(&t.0, 44);
        killed.append(&row(44, 0, 1_000, 10)).unwrap();
        killed.append(&row(44, 10_000, 8_000, 6_000)).unwrap();
        // A torn final line, as a kill during a write would leave it.
        killed.active.write_all(b"{\"pid\":44,\"t_ms\":2").unwrap();
        // While its owner lock is held, the writer is live.
        assert!(!archive::is_dead(&t.0, 44).unwrap());
        assert!(postmortem(&t.0, Some(44)).is_err());
    }
    // Dropped without close: an unclean exit with a released lock.
    let pm = postmortem(&t.0, Some(44)).unwrap();
    assert_eq!(pm.pid, 44);
    assert_eq!(pm.diff.footprint_delta, 7_000);
    assert_eq!(pm.diff.growers[0].name, "assets.bitmap");
    {
        let mut clean = open(&t.0, 45);
        clean.append(&row(45, 0, 10, 1)).unwrap();
        clean.close().unwrap();
        assert!(clean.append(&row(45, 10_000, 10, 1)).is_err());
    }
    recover(&t.0).unwrap();
    let archived = archive::archived_dirs(&t.0).unwrap();
    assert_eq!(archived.len(), 1, "{archived:?}");
    assert!(
        archived[0]
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("mem-44-")
    );
    assert!(archived[0].join("mem-44.ndjson").is_file());
    assert!(!segment(&t.0, 44, 0).exists());
    assert!(
        !segment(&t.0, 45, 0).exists(),
        "a clean exit leaves nothing"
    );
    // The archived copy still explains the dead process.
    assert_eq!(postmortem(&t.0, None).unwrap().pid, 44);
    // A copied stream can be read on its own, under any name.
    let copy = t.0.join("copied.ndjson");
    fs::copy(archived[0].join("mem-44.ndjson"), &copy).unwrap();
    assert_eq!(postmortem_file(&copy).unwrap().diff.footprint_delta, 7_000);
}

#[test]
fn a_watchdog_report_is_evidence_when_no_recorder_ran() {
    let t = Temp::new("report");
    let report = serde_json::json!({
        "pid": 46, "reason": "hard limit", "job_name": "Memory check",
        "uptime_ms": 4000, "unix_ms": 9000, "footprint_bytes": 30, "soft": 8, "hard": 16,
        "samples": [[0, 10], [2000, 20], [4000, 30]], "diagnostics": {"vmmap": "large"},
    });
    let file = t.0.join("mem-46-123-0.json");
    fs::write(&file, report.to_string()).unwrap();
    let pm = postmortem(&t.0, None).unwrap();
    assert_eq!((pm.pid, pm.diff.footprint_delta), (46, 20));
    let summary = pm.report.unwrap();
    assert_eq!(summary["reason"], "hard limit");
    assert!(
        summary.get("diagnostics").is_none(),
        "large fields stay in the file"
    );
    assert_eq!(postmortem_file(&file).unwrap().pid, 46);
    assert!(postmortem(&t.0, Some(47)).is_err());
}

#[test]
fn a_full_archive_refuses_a_new_recorder() {
    let t = Temp::new("budget");
    let budget = 2 * SEGMENTS as u64 * SEGMENT_BYTES;
    let first = archive::open(&t.0, 48, budget).unwrap();
    let second = archive::open(&t.0, 49, budget).unwrap();
    let full = archive::open(&t.0, 50, budget);
    assert!(full.is_err());
    drop((first, second));
}
