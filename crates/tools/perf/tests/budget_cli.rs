//! `h2 profile budget` (perf::cli::budget) on synthetic traces, and the
//! timeline crate's own output read back by `perf::budget`. Its own test
//! binary: the round trip records on the process-wide timeline.
use std::path::PathBuf;

/// A scratch directory, removed when dropped.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Scratch {
        let d = std::env::temp_dir().join(format!("perf-budget-cli-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        Scratch(d)
    }
    fn file(&self, text: &str) -> String {
        let p = self.0.join("trace.json");
        std::fs::write(&p, text).unwrap();
        p.to_str().unwrap().to_owned()
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Two 10 ms frames: a 6 ms slice holding a 2 ms submit, then idle.
const TRACE: &str = r#"{"traceEvents":[
{"ph":"M","name":"thread_name","pid":1,"tid":1,"args":{"name":"main"}},
{"name":"frame","cat":"frame","pid":1,"tid":1,"ts":0,"ph":"i","s":"t","args":{"n":1}},
{"name":"logical us","cat":"sched","pid":1,"tid":1,"ts":0,"ph":"C","args":{"logical us":0}},
{"name":"slice","cat":"guest","pid":2,"tid":2,"ts":0,"ph":"X","dur":6000,"args":{"accesses":1000}},
{"name":"queue submit","cat":"gfx","pid":1,"tid":1,"ts":1000,"ph":"X","dur":2000},
{"name":"logical us","cat":"sched","pid":1,"tid":1,"ts":9000,"ph":"C","args":{"logical us":33333}},
{"name":"frame","cat":"frame","pid":1,"tid":1,"ts":10000,"ph":"i","s":"t","args":{"n":2}},
{"name":"frame","cat":"frame","pid":1,"tid":1,"ts":20000,"ph":"i","s":"t","args":{"n":3}}
]}"#;

fn run(args: &[&str]) -> anyhow::Result<(i32, String)> {
    let mut out = Vec::new();
    let code = perf::cli::budget_to(args.iter().map(|s| s.to_string()).collect(), &mut out)?;
    Ok((code, String::from_utf8(out).unwrap()))
}

#[test]
fn the_table_reads_a_trace_file() {
    let d = Scratch::new("table");
    let (code, out) = run(&[&d.file(TRACE)]).unwrap();
    assert_eq!(code, 0);
    assert!(
        out.starts_with("2 frames: wall ms per frame median 10.0"),
        "{out}"
    );
    assert!(out.contains("logical ms per frame"), "{out}");
    let row = |name: &str| {
        out.lines()
            .find(|l| l.starts_with(name))
            .unwrap_or_else(|| panic!("{name}: {out}"))
            .to_owned()
    };
    // Frame 1: slice 4 ms self, submit 2 ms, 4 ms no span; frame 2 idle.
    assert!(row("(no span)").contains("7.00"), "{out}");
    assert!(row("slice").contains("2.00"), "{out}");
    assert!(row("queue submit").contains("1.00"), "{out}");
}

#[test]
fn json_has_every_frame() {
    let d = Scratch::new("json");
    let (code, out) = run(&["--json", &d.file(TRACE)]).unwrap();
    assert_eq!(code, 0);
    let v: serde_json::Value = serde_json::from_str(&out).unwrap();
    let frames = v["frames"].as_array().unwrap();
    assert_eq!(frames.len(), 2);
    assert_eq!(frames[0]["wall_ns"], 10_000_000);
    assert_eq!(frames[0]["self_ns"]["queue submit"], 2_000_000);
    assert_eq!(frames[0]["logical_us"], 33333);
    assert_eq!(frames[0]["accesses"], 1000);
}

#[test]
fn profile_dispatches_budget() {
    let d = Scratch::new("dispatch");
    let code = perf::cli::profile(vec!["budget".into(), d.file(TRACE)]).unwrap();
    assert_eq!(code, 0);
    let e = perf::cli::profile(vec!["budget".into()]).unwrap_err();
    assert!(e.to_string().contains("usage: h2 profile budget"), "{e}");
}

#[test]
fn errors_name_what_is_wrong() {
    let err = |args: &[&str]| run(args).unwrap_err().to_string();
    assert!(err(&[]).contains("usage: h2 profile budget"));
    assert!(err(&["--help"]).contains("usage: h2 profile budget"));
    let d = Scratch::new("errors");
    let p = d.file(TRACE);
    assert!(err(&[&p, "extra"]).contains("unexpected extra"));
    let missing = d.0.join("nope.json");
    assert!(err(&[missing.to_str().unwrap()]).contains("reading"));
    let bad = d.file("{not json");
    assert!(format!("{:#}", run(&[&bad]).unwrap_err()).contains("parsing the trace JSON"));
    let empty = d.file(r#"{"traceEvents":[]}"#);
    assert!(err(&[&empty]).contains("no frame instants"));
}

#[test]
fn the_timeline_crate_s_own_output_reads_back() {
    timeline::start(timeline::Config::all());
    timeline::frame();
    {
        let _s = timeline::span(timeline::Cat::Sched, timeline::Track::Host, "schedule");
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    timeline::frame();
    let t = timeline::stop();
    let mut json = Vec::new();
    t.write_chrome(&mut json).unwrap();
    let b = perf::budget::Budget::from_chrome_json(std::str::from_utf8(&json).unwrap()).unwrap();
    assert_eq!(b.frames.len(), 1);
    assert!(b.frames[0].self_ns.get("schedule").copied().unwrap_or(0) >= 2_000_000);
}
