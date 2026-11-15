//! Per-frame wall-time budget from a timeline trace (`h2 profile budget`,
//! docs/perf-budget.md).
//!
//! The game thread's spans are laid out on one time line and each instant
//! of it is charged to the innermost span open then (its *self* time), or
//! to `(no span)` when none is. The time between two `frame` instants is
//! one frame, so the self times of a frame add up to its wall time. The
//! game thread is the host thread that records the frames. Its spans are
//! the host events on that thread plus the guest-track `slice` and DPC
//! spans, which the scheduler records on the same thread. Guest-track
//! kernel waits are left out: they span the time a guest thread is
//! blocked, while other threads run.
//!
//! Alongside: the logical clock's rise per frame (the kernel's
//! `logical us` counter, `kernel::threads::LOGICAL_COUNTER`), idle clock
//! advances, slices and the guest accesses they charged, vblank DPCs, and
//! contended waits on the guest memory lock on every thread.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::Value;

/// Time on the game thread no span covers.
pub const NO_SPAN: &str = "(no span)";

/// Guest-track events that are not game-thread time (kernel waits).
fn is_wait(cat: &str, pid: u64) -> bool {
    pid == GUEST_PID && cat == "kernel"
}

const HOST_PID: u64 = 1;
const GUEST_PID: u64 = 2;

#[derive(Debug, Clone)]
struct SpanEv {
    start: u64,
    end: u64,
    name: String,
}

/// One frame: from one `frame` instant to the next.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Frame {
    /// The `n` of the frame instant that ends it.
    pub n: u64,
    pub wall_ns: u64,
    /// Self time per span name; sums to `wall_ns`.
    pub self_ns: BTreeMap<String, u64>,
    /// Logical time the frame took (`None`: no counter in the trace).
    pub logical_us: Option<u64>,
    /// Idle clock advances and the logical time they skipped.
    pub idle_advances: u64,
    pub idle_us: u64,
    pub slices: u64,
    pub accesses: u64,
    /// vblank DPCs run (periodic source `vblank`).
    pub vblanks: u64,
}

/// A trace's frames and its totals.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Budget {
    pub frames: Vec<Frame>,
    /// Contended guest-memory-lock waits per host thread name: count, ns.
    pub lock_waits: BTreeMap<String, (u64, u64)>,
}

fn us_to_ns(v: &Value) -> Option<u64> {
    // `ts` and `dur` are microseconds with three decimals.
    let f = v.as_f64()?;
    Some((f * 1000.0).round().max(0.0) as u64)
}

impl Budget {
    /// Read the Chrome trace-event JSON that `timeline::Trace::write_chrome`
    /// writes. Frames start at the first `frame` instant; the time before
    /// it is not counted.
    pub fn from_chrome_json(json: &str) -> Result<Budget> {
        let v: Value = serde_json::from_str(json).context("parsing the trace JSON")?;
        let events = v
            .get("traceEvents")
            .and_then(Value::as_array)
            .context("no traceEvents array")?;
        let mut names: BTreeMap<u64, String> = BTreeMap::new();
        let mut frames: Vec<(u64, u64)> = Vec::new();
        let mut game: Option<u64> = None;
        for e in events {
            let ph = e.get("ph").and_then(Value::as_str).unwrap_or("");
            let pid = e.get("pid").and_then(Value::as_u64).unwrap_or(0);
            let tid = e.get("tid").and_then(Value::as_u64).unwrap_or(0);
            let name = e.get("name").and_then(Value::as_str).unwrap_or("");
            if ph == "M" && name == "thread_name" && pid == HOST_PID {
                if let Some(n) = e.pointer("/args/name").and_then(Value::as_str) {
                    names.insert(tid, n.to_owned());
                }
            } else if ph == "i" && name == "frame" && pid == HOST_PID {
                let ts = e.get("ts").and_then(us_to_ns).unwrap_or(0);
                let n = e.pointer("/args/n").and_then(Value::as_u64).unwrap_or(0);
                frames.push((ts, n));
                game.get_or_insert(tid);
            }
        }
        let Some(game) = game else {
            bail!("the trace has no frame instants (record the frame category)");
        };
        frames.sort_unstable();
        let mut spans = Vec::new();
        let mut logical: Vec<(u64, u64)> = Vec::new();
        let mut idle: Vec<(u64, u64)> = Vec::new();
        let mut slices: Vec<(u64, u64)> = Vec::new();
        let mut vblanks: Vec<u64> = Vec::new();
        let mut lock_waits: BTreeMap<String, (u64, u64)> = BTreeMap::new();
        for e in events {
            let ph = e.get("ph").and_then(Value::as_str).unwrap_or("");
            let pid = e.get("pid").and_then(Value::as_u64).unwrap_or(0);
            let tid = e.get("tid").and_then(Value::as_u64).unwrap_or(0);
            let name = e.get("name").and_then(Value::as_str).unwrap_or("");
            let cat = e.get("cat").and_then(Value::as_str).unwrap_or("");
            let ts = e.get("ts").and_then(us_to_ns).unwrap_or(0);
            match ph {
                "X" => {
                    let dur = e.get("dur").and_then(us_to_ns).unwrap_or(0);
                    if cat == "lock" && pid == HOST_PID {
                        let who = names
                            .get(&tid)
                            .cloned()
                            .unwrap_or_else(|| format!("host thread {tid}"));
                        let w = lock_waits.entry(who).or_default();
                        w.0 += 1;
                        w.1 += dur;
                    }
                    let on_game = (pid == HOST_PID && tid == game)
                        || (pid == GUEST_PID && !is_wait(cat, pid));
                    if !on_game {
                        continue;
                    }
                    if pid == GUEST_PID && name == "slice" {
                        let a = e.pointer("/args/accesses").and_then(Value::as_u64);
                        slices.push((ts, a.unwrap_or(0)));
                    }
                    if pid == GUEST_PID && name == "vblank" {
                        vblanks.push(ts);
                    }
                    spans.push(SpanEv {
                        start: ts,
                        end: ts + dur,
                        name: name.to_owned(),
                    });
                }
                "C" if cat == "sched" => {
                    if let Some(v) = e.pointer("/args").and_then(|a| a.get(name)) {
                        logical.push((ts, v.as_u64().unwrap_or(0)));
                    }
                }
                "i" if name == "idle: clock to deadline" => {
                    let t = e.pointer("/args/ticks").and_then(Value::as_u64);
                    idle.push((ts, t.unwrap_or(0)));
                }
                _ => {}
            }
        }
        let segments = self_segments(spans);
        let mut out = Vec::new();
        for w in frames.windows(2) {
            let (a, b) = (w[0].0, w[1].0);
            let mut f = Frame {
                n: w[1].1,
                wall_ns: b - a,
                ..Frame::default()
            };
            // Segments are in time order and do not overlap.
            let first = segments.partition_point(|s| s.1 <= a);
            for (s, e, name) in &segments[first..] {
                if *s >= b {
                    break;
                }
                let d = (*e).min(b) - (*s).max(a);
                if d > 0 {
                    *f.self_ns.entry(name.clone()).or_default() += d;
                }
            }
            let covered: u64 = f.self_ns.values().sum();
            if covered < f.wall_ns {
                *f.self_ns.entry(NO_SPAN.to_owned()).or_default() += f.wall_ns - covered;
            }
            let at = |t: u64| {
                let i = logical.partition_point(|c| c.0 <= t);
                (i > 0).then(|| logical[i - 1].1)
            };
            f.logical_us = match (at(a), at(b)) {
                (Some(x), Some(y)) => Some(y.saturating_sub(x)),
                _ => None,
            };
            let within = |t: u64| t >= a && t < b;
            for &(t, ticks) in &idle {
                if within(t) {
                    f.idle_advances += 1;
                    f.idle_us += ticks / 10;
                }
            }
            for &(t, acc) in &slices {
                if within(t) {
                    f.slices += 1;
                    f.accesses += acc;
                }
            }
            f.vblanks = vblanks.iter().filter(|&&t| within(t)).count() as u64;
            out.push(f);
        }
        Ok(Budget {
            frames: out,
            lock_waits,
        })
    }

    /// Mean self time per frame for each span name, largest first.
    pub fn rows(&self) -> Vec<(String, f64, f64)> {
        let n = self.frames.len().max(1) as f64;
        let mut total: BTreeMap<&str, u64> = BTreeMap::new();
        for f in &self.frames {
            for (k, v) in &f.self_ns {
                *total.entry(k).or_default() += v;
            }
        }
        let wall: u64 = self.frames.iter().map(|f| f.wall_ns).sum();
        let mut rows: Vec<_> = total
            .into_iter()
            .map(|(k, v)| {
                let pct = if wall == 0 {
                    0.0
                } else {
                    v as f64 * 100.0 / wall as f64
                };
                (k.to_owned(), v as f64 / n / 1e6, pct)
            })
            .collect();
        rows.sort_by(|a, b| b.1.total_cmp(&a.1));
        rows
    }

    /// The report `h2 profile budget` prints.
    pub fn table(&self) -> String {
        let mut s = String::new();
        let n = self.frames.len();
        if n == 0 {
            return "no complete frames in the trace\n".into();
        }
        let median = |mut v: Vec<f64>| {
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let wall = median(self.frames.iter().map(|f| f.wall_ns as f64 / 1e6).collect());
        let mean = self.frames.iter().map(|f| f.wall_ns).sum::<u64>() as f64 / n as f64 / 1e6;
        let _ = writeln!(
            s,
            "{n} frames: wall ms per frame median {wall:.1}, mean {mean:.1} ({:.2} fps)",
            1e3 / mean
        );
        let logical: Vec<f64> = self
            .frames
            .iter()
            .filter_map(|f| f.logical_us.map(|l| l as f64 / 1e3))
            .collect();
        if !logical.is_empty() {
            let lm = logical.iter().sum::<f64>() / logical.len() as f64;
            let _ = writeln!(
                s,
                "logical ms per frame: median {:.1}, mean {lm:.1}; logical/wall {:.3}",
                median(logical.clone()),
                lm / mean
            );
        }
        let per = |f: fn(&Frame) -> u64| self.frames.iter().map(f).sum::<u64>() as f64 / n as f64;
        let _ = writeln!(
            s,
            "per frame: {:.1} vblanks, {:.0} slices, {:.2}M accesses, {:.1} idle advances skipping {:.1} logical ms",
            per(|f| f.vblanks),
            per(|f| f.slices),
            per(|f| f.accesses) / 1e6,
            per(|f| f.idle_advances),
            per(|f| f.idle_us) / 1e3,
        );
        let _ = writeln!(s, "\n{:<32} {:>10} {:>7}", "self time", "ms/frame", "%");
        for (k, ms, pct) in self.rows() {
            let _ = writeln!(s, "{k:<32} {ms:>10.2} {pct:>6.1}%");
        }
        if !self.lock_waits.is_empty() {
            let _ = writeln!(s, "\ncontended guest memory lock waits (whole trace)");
            for (who, (c, ns)) in &self.lock_waits {
                let _ = writeln!(s, "{who:<32} {c:>8} waits {:>10.2} ms", *ns as f64 / 1e6);
            }
        }
        s
    }
}

/// Lay `spans` out on one time line: `(start, end, name)` segments, in
/// order and disjoint, each charged to the innermost span open over it.
/// A span that outlives its parent is cut at the parent's end.
fn self_segments(mut spans: Vec<SpanEv>) -> Vec<(u64, u64, String)> {
    spans.sort_by(|a, b| a.start.cmp(&b.start).then(b.end.cmp(&a.end)));
    let mut out: Vec<(u64, u64, String)> = Vec::new();
    let mut stack: Vec<SpanEv> = Vec::new();
    let mut t = 0u64;
    let emit = |out: &mut Vec<(u64, u64, String)>, s: u64, e: u64, name: &str| {
        if e > s {
            match out.last_mut() {
                Some(last) if last.1 == s && last.2 == name => last.1 = e,
                _ => out.push((s, e, name.to_owned())),
            }
        }
    };
    for mut sp in spans {
        while let Some(top) = stack.last() {
            if top.end > sp.start {
                break;
            }
            let top = stack.pop().unwrap();
            emit(&mut out, t.max(top.start), top.end, &top.name);
            t = t.max(top.end);
        }
        if let Some(top) = stack.last() {
            emit(&mut out, t.max(top.start), sp.start, &top.name);
            sp.end = sp.end.min(top.end);
        }
        t = t.max(sp.start);
        stack.push(sp);
    }
    while let Some(top) = stack.pop() {
        emit(&mut out, t.max(top.start), top.end, &top.name);
        t = t.max(top.end);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn x(pid: u64, tid: u64, cat: &str, name: &str, ts: f64, dur: f64, args: &str) -> String {
        let args = if args.is_empty() {
            String::new()
        } else {
            format!(",\"args\":{{{args}}}")
        };
        format!(
            "{{\"name\":\"{name}\",\"cat\":\"{cat}\",\"pid\":{pid},\"tid\":{tid},\"ts\":{ts},\"ph\":\"X\",\"dur\":{dur}{args}}}"
        )
    }
    fn frame(ts: f64, n: u64) -> String {
        format!(
            "{{\"name\":\"frame\",\"cat\":\"frame\",\"pid\":1,\"tid\":1,\"ts\":{ts},\"ph\":\"i\",\"s\":\"t\",\"args\":{{\"n\":{n}}}}}"
        )
    }
    fn counter(ts: f64, v: u64) -> String {
        format!(
            "{{\"name\":\"logical us\",\"cat\":\"sched\",\"pid\":1,\"tid\":1,\"ts\":{ts},\"ph\":\"C\",\"args\":{{\"logical us\":{v}}}}}"
        )
    }
    fn trace(events: &[String]) -> String {
        let meta = [
            "{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":1,\"tid\":1,\"args\":{\"name\":\"main\"}}".to_owned(),
            "{\"ph\":\"M\",\"name\":\"thread_name\",\"pid\":1,\"tid\":2,\"args\":{\"name\":\"dsound-output\"}}".to_owned(),
        ];
        let all: Vec<String> = meta.iter().cloned().chain(events.iter().cloned()).collect();
        format!("{{\"traceEvents\":[{}]}}", all.join(",\n"))
    }

    /// Two frames of 100 us each; times in microseconds.
    fn sample() -> Budget {
        let ev = vec![
            frame(0.0, 1),
            counter(0.0, 1000),
            // Frame 1: schedule 0-100 holds a slice 10-60 holding an HLE
            // call 20-30 and a pushbuf drain 30-50 holding a submit 40-45.
            x(1, 1, "sched", "schedule", 0.0, 100.0, ""),
            x(2, 5, "guest", "slice", 10.0, 50.0, "\"accesses\":70000"),
            x(1, 1, "hle", "hle call", 20.0, 10.0, "\"pc\":1"),
            x(1, 1, "d3d", "pushbuf drain", 30.0, 20.0, ""),
            x(1, 1, "gfx", "queue submit", 40.0, 5.0, ""),
            // A guest thread's wait: not game-thread time.
            x(2, 6, "kernel", "wait objects", 0.0, 150.0, ""),
            // The vblank DPC, 70-80.
            x(2, 0xffff, "interrupt", "vblank", 70.0, 10.0, "\"routine\":5"),
            counter(60.0, 1400),
            "{\"name\":\"idle: clock to deadline\",\"cat\":\"kernel\",\"pid\":1,\"tid\":1,\"ts\":65,\"ph\":\"i\",\"s\":\"t\",\"args\":{\"ticks\":166000}}".to_owned(),
            counter(65.0, 18000),
            frame(100.0, 2),
            // Frame 2: one slice 100-150, then nothing.
            x(2, 5, "guest", "slice", 100.0, 50.0, "\"accesses\":5"),
            counter(150.0, 18100),
            frame(200.0, 3),
            // Another thread waited on the memory lock.
            x(1, 2, "lock", "memory lock wait", 120.0, 3.0, ""),
        ];
        Budget::from_chrome_json(&trace(&ev)).unwrap()
    }

    #[test]
    fn self_times_nest_and_add_up_to_the_frame() {
        let b = sample();
        assert_eq!(b.frames.len(), 2);
        let f = &b.frames[0];
        assert_eq!(f.n, 2);
        assert_eq!(f.wall_ns, 100_000);
        let get = |k: &str| f.self_ns.get(k).copied().unwrap_or(0);
        assert_eq!(get("schedule"), 40_000, "0-10, 60-70, 80-100");
        assert_eq!(get("slice"), 20_000, "10-20, 50-60");
        assert_eq!(get("hle call"), 10_000);
        assert_eq!(get("pushbuf drain"), 15_000);
        assert_eq!(get("queue submit"), 5_000);
        assert_eq!(get("vblank"), 10_000);
        assert_eq!(get("wait objects"), 0, "waits are not game-thread time");
        assert_eq!(f.self_ns.values().sum::<u64>(), f.wall_ns);
        let g = &b.frames[1];
        assert_eq!(g.self_ns.get("slice"), Some(&50_000));
        assert_eq!(g.self_ns.get(NO_SPAN), Some(&50_000));
    }

    #[test]
    fn logical_time_idle_slices_and_vblanks_per_frame() {
        let b = sample();
        let f = &b.frames[0];
        assert_eq!(f.logical_us, Some(17_000));
        assert_eq!((f.idle_advances, f.idle_us), (1, 16_600));
        assert_eq!((f.slices, f.accesses), (1, 70_000));
        assert_eq!(f.vblanks, 1);
        assert_eq!(b.frames[1].logical_us, Some(100));
        assert_eq!(b.lock_waits.get("dsound-output"), Some(&(1, 3_000)));
        let t = b.table();
        assert!(t.contains("2 frames"), "{t}");
        assert!(t.contains("dsound-output"), "{t}");
        let rows = b.rows();
        assert_eq!(rows[0].0, "slice", "{rows:?}");
        let pct: f64 = rows.iter().map(|r| r.2).sum();
        assert!((pct - 100.0).abs() < 1e-6, "{pct}");
    }

    #[test]
    fn a_child_outliving_its_parent_is_cut_at_the_parents_end() {
        let s = self_segments(vec![
            SpanEv {
                start: 0,
                end: 10,
                name: "a".into(),
            },
            SpanEv {
                start: 5,
                end: 20,
                name: "b".into(),
            },
            SpanEv {
                start: 30,
                end: 40,
                name: "c".into(),
            },
        ]);
        assert_eq!(
            s,
            vec![
                (0, 5, "a".into()),
                (5, 10, "b".into()),
                (30, 40, "c".into())
            ]
        );
    }

    #[test]
    fn a_trace_without_frames_is_an_error() {
        assert!(Budget::from_chrome_json(&trace(&[])).is_err());
    }

    #[test]
    fn the_timeline_crate_s_own_output_reads_back() {
        static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
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
        let b = Budget::from_chrome_json(std::str::from_utf8(&json).unwrap()).unwrap();
        assert_eq!(b.frames.len(), 1);
        assert!(b.frames[0].self_ns.get("schedule").copied().unwrap_or(0) >= 2_000_000);
    }
}
