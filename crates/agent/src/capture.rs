//! `profile.capture`: frame-stats rows (`timeline::phase`, the numbers
//! `H2_FRAME_STATS` prints) over a window, summarised, with every frame
//! over a budget kept as a spike with its own breakdown and the tick and
//! map it fell in, and every row written to a CSV.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::Path;

use agent_proto::{CaptureStats, CountStats, PhaseStats, Spike};
use timeline::phase::{COUNTERS, Counter, FrameSample, PHASES, Phase};

/// Where frames come from: the frame-stats collection in the game, a list
/// in tests.
pub trait FrameSource: Send {
    /// Starts keeping every frame; false when frame stats are off.
    fn begin(&mut self) -> bool;
    /// Every frame since [`begin`](Self::begin), in order; stops keeping
    /// them.
    fn end(&mut self) -> Option<Vec<FrameSample>>;
}

/// The game's frame stats.
pub struct TimelineFrames;

impl FrameSource for TimelineFrames {
    fn begin(&mut self) -> bool {
        timeline::phase::capture_begin()
    }
    fn end(&mut self) -> Option<Vec<FrameSample>> {
        timeline::phase::capture_end()
    }
}

/// A sampling profiler run over the window: the game's own
/// (`profile.sample.*`, boot's `sampling`, with the `perf` feature).
pub trait Sampler: Send {
    /// Starts sampling the game's threads.
    fn start(&mut self) -> Result<(), String>;
    /// Stops and writes the profile; answers what `profile.sample.stop`
    /// would (where the dump went, how many samples).
    fn finish(&mut self) -> Result<serde_json::Value, String>;
}

/// One game tick seen during the capture: when it ended, and where.
#[derive(Debug, Clone, PartialEq)]
pub struct TickMark {
    pub ns: u64,
    pub tick: Option<u64>,
    pub map: Option<String>,
}

/// The tick a frame that ended at `ns` belongs to: the last tick that had
/// ended by then (the first one, for frames before it).
fn tick_of<'a>(marks: &'a [TickMark], ns: u64) -> Option<&'a TickMark> {
    let i = marks.partition_point(|m| m.ns <= ns);
    marks.get(i.saturating_sub(1))
}

fn ms(ns: u64) -> f64 {
    round(ns as f64 / 1e6)
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// Nearest rank, as the frame-stats report.
fn percentile(sorted: &[u64], p: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((p / 100.0) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn stats(values: impl Iterator<Item = u64>) -> PhaseStats {
    let mut v: Vec<u64> = values.collect();
    v.sort_unstable();
    let total: u64 = v.iter().sum();
    PhaseStats {
        mean: ms(total / v.len().max(1) as u64),
        p50: ms(percentile(&v, 50.0)),
        p95: ms(percentile(&v, 95.0)),
        p99: ms(percentile(&v, 99.0)),
        max: ms(v.last().copied().unwrap_or(0)),
        total: ms(total),
    }
}

/// The phases a spike is blamed on: the biggest first, pacing left out
/// (it is slack, not cost).
fn top(s: &FrameSample) -> Vec<String> {
    let mut p: Vec<(Phase, u64)> = Phase::ALL
        .into_iter()
        .filter(|&p| p != Phase::Pace)
        .map(|p| (p, s.phase_ns[p as usize]))
        .filter(|&(_, ns)| ns > 0)
        .collect();
    p.sort_by(|a, b| b.1.cmp(&a.1));
    p.into_iter()
        .take(3)
        .map(|(p, _)| p.label().to_owned())
        .collect()
}

/// Each counter's median over the frames: what a frame usually counts.
fn medians(frames: &[FrameSample]) -> [u64; COUNTERS] {
    std::array::from_fn(|c| {
        let mut v: Vec<u64> = frames.iter().map(|f| f.counts[c]).collect();
        v.sort_unstable();
        percentile(&v, 50.0)
    })
}

/// Host time in file I/O, in the frame, that makes `file_io` a cause: 2 ms
/// (or a tenth of the frame's busy time, whichever is less).
pub const FILE_IO_MIN_US: u64 = 2_000;

/// Why a frame over budget was slow, from its counters (against what a
/// frame of the window usually counts, `typical`) and its phases:
///
/// - `shader_compile`, `pipeline_compile`: any shader translated or render
///   pipeline built on the critical path;
/// - `texture_uploads`, `gpu_readback`: more uploads or blocking readbacks
///   than twice the usual (and at least one);
/// - `file_io`: file I/O took time: the guest's file calls (any guest
///   thread) spent at least [`FILE_IO_MIN_US`] (2 ms) of host time in the
///   frame, or at least a tenth of its busy time (`file_io_us`). Bytes read
///   are not the test: a read costs about 38 us per 32 KB and finishes
///   inside the call, so a frame that read some bytes was no slower than
///   one that read none;
/// - `gpu`, `render_thread`, `pushbuf_decode`: that phase is at least 40%
///   of the frame's busy time;
/// - `guest_cpu`: none of those, and the guest's own time (`cpu` plus
///   `render`) is the biggest phase.
pub fn causes(s: &FrameSample, typical: &[u64; COUNTERS]) -> Vec<String> {
    let mut out = Vec::new();
    let count = |c: Counter| s.counts[c as usize];
    let above = |c: Counter| count(c) > 0 && count(c) > 2 * typical[c as usize];
    if count(Counter::Shaders) > 0 {
        out.push("shader_compile");
    }
    if count(Counter::Pipelines) > 0 {
        out.push("pipeline_compile");
    }
    if above(Counter::Uploads) {
        out.push("texture_uploads");
    }
    if above(Counter::Readbacks) {
        out.push("gpu_readback");
    }
    let busy = s.busy_ns().max(1);
    let io_us = count(Counter::FileIoUs);
    if io_us >= FILE_IO_MIN_US || (io_us > 0 && io_us * 1000 * 10 >= busy) {
        out.push("file_io");
    }
    let share = |p: Phase| s.phase_ns[p as usize] as f64 / busy as f64;
    for (p, name) in [
        (Phase::Wait, "gpu"),
        (Phase::RenderWait, "render_thread"),
        (Phase::Decode, "pushbuf_decode"),
    ] {
        if share(p) >= 0.4 {
            out.push(name);
        }
    }
    if out.is_empty() && top(s).first().is_some_and(|t| t == "cpu" || t == "render") {
        out.push("guest_cpu");
    }
    out.into_iter().map(str::to_owned).collect()
}

/// Everything the reply says about the frames, before the stamp and files.
pub fn summarise(
    frames: &[FrameSample],
    marks: &[TickMark],
    budget_ms: f64,
    max_spikes: usize,
) -> CaptureStats {
    let wall: u64 = frames.iter().map(|f| f.total_ns).sum();
    let mut phases = BTreeMap::new();
    for p in Phase::ALL {
        let s = stats(frames.iter().map(|f| f.phase_ns[p as usize]));
        // `render` is only measured with the render-timing feature on.
        if p == Phase::Render && s.max == 0.0 {
            continue;
        }
        phases.insert(p.label().to_owned(), s);
    }
    let mut counts = BTreeMap::new();
    for (i, label) in timeline::phase::counter_columns() {
        let v = frames.iter().map(|f| f.counts[i]);
        let total: u64 = v.clone().sum();
        counts.insert(
            label.to_owned(),
            CountStats {
                mean: round(total as f64 / frames.len().max(1) as f64),
                max: v.max().unwrap_or(0),
                total,
            },
        );
    }
    let budget_ns = (budget_ms * 1e6) as u64;
    let mut over: Vec<&FrameSample> = frames.iter().filter(|f| f.total_ns > budget_ns).collect();
    let over_budget = over.len();
    let typical = medians(frames);
    let mut by_cause: BTreeMap<String, usize> = BTreeMap::new();
    for f in &over {
        for c in causes(f, &typical) {
            *by_cause.entry(c).or_default() += 1;
        }
    }
    over.sort_by(|a, b| b.total_ns.cmp(&a.total_ns).then(a.index.cmp(&b.index)));
    let spikes = over
        .into_iter()
        .take(max_spikes)
        .map(|f| {
            let mark = tick_of(marks, f.end_ns);
            Spike {
                present: f.index,
                tick: mark.and_then(|m| m.tick),
                map: mark.and_then(|m| m.map.clone()),
                ms: ms(f.total_ns),
                busy_ms: ms(f.busy_ns()),
                phases: Phase::ALL
                    .into_iter()
                    .filter(|&p| f.phase_ns[p as usize] > 0)
                    .map(|p| (p.label().to_owned(), ms(f.phase_ns[p as usize])))
                    .collect(),
                counts: timeline::phase::counter_columns()
                    .filter(|&(i, _)| f.counts[i] > 0)
                    .map(|(i, label)| (label.to_owned(), f.counts[i]))
                    .collect(),
                top: top(f),
                causes: causes(f, &typical),
            }
        })
        .collect();
    CaptureStats {
        frames: frames.len(),
        wall_ms: ms(wall),
        fps: round(frames.len() as f64 * 1e9 / wall.max(1) as f64),
        frame_ms: stats(frames.iter().map(|f| f.total_ns)),
        busy_ms: stats(frames.iter().map(FrameSample::busy_ns)),
        phases,
        counts,
        budget_ms,
        over_budget,
        spikes,
        causes: by_cause,
    }
}

/// One row per frame: `present,end_ms,tick,map,total_us,busy_us`, then each
/// phase in us and each counter, as `H2_FRAME_STATS_CSV` with the tick and
/// map added.
pub fn write_csv(path: &Path, frames: &[FrameSample], marks: &[TickMark]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let phases: Vec<String> = Phase::ALL
        .iter()
        .map(|p| format!("{}_us", p.label()))
        .collect();
    let counts: Vec<&str> = timeline::phase::counter_columns().map(|(_, label)| label).collect();
    writeln!(
        f,
        "present,end_ms,tick,map,total_us,busy_us,{},{}",
        phases.join(","),
        counts.join(",")
    )?;
    for s in frames {
        let mark = tick_of(marks, s.end_ns);
        let tick = mark
            .and_then(|m| m.tick)
            .map_or(String::new(), |t| t.to_string());
        let map = mark.and_then(|m| m.map.as_deref()).unwrap_or("");
        let phases: Vec<String> = (0..PHASES)
            .map(|i| (s.phase_ns[i] / 1000).to_string())
            .collect();
        let counts: Vec<String> = (0..COUNTERS).map(|i| s.counts[i].to_string()).collect();
        writeln!(
            f,
            "{},{:.3},{tick},{map},{},{},{},{}",
            s.index,
            s.end_ns as f64 / 1e6,
            s.total_ns / 1000,
            s.busy_ns() / 1000,
            phases.join(","),
            counts.join(",")
        )?;
    }
    f.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A frame of `cpu` and `gpu-wait` ms, ending at `end` ms.
    pub(crate) fn frame(index: u64, end: u64, cpu: u64, gpu: u64, draws: u64) -> FrameSample {
        let mut s = FrameSample {
            index,
            end_ns: end * 1_000_000,
            total_ns: (cpu + gpu) * 1_000_000,
            ..Default::default()
        };
        s.phase_ns[Phase::Guest as usize] = cpu * 1_000_000;
        s.phase_ns[Phase::Wait as usize] = gpu * 1_000_000;
        s.counts[Counter::Draws as usize] = draws;
        s
    }

    fn marks() -> Vec<TickMark> {
        vec![
            TickMark {
                ns: 0,
                tick: Some(500),
                map: Some("03a_oldmombasa".into()),
            },
            TickMark {
                ns: 1_000_000_000,
                tick: Some(530),
                map: Some("03a_oldmombasa".into()),
            },
        ]
    }

    #[test]
    fn percentiles_totals_and_spikes_with_their_tick() {
        // 98 steady 16 ms frames, a 40 ms and a 60 ms spike.
        let mut frames: Vec<FrameSample> = (0..98).map(|i| frame(i, i * 16, 12, 4, 100)).collect();
        frames.push(frame(98, 900, 30, 10, 900));
        frames.push(frame(99, 1_100, 20, 40, 400));
        let s = summarise(&frames, &marks(), 33.3, 10);
        assert_eq!(s.frames, 100);
        assert_eq!(s.frame_ms.p50, 16.0);
        assert_eq!(s.frame_ms.p99, 40.0);
        assert_eq!(s.frame_ms.max, 60.0);
        assert_eq!(s.phases["cpu"].total, 98.0 * 12.0 + 50.0);
        assert_eq!(s.phases["gpu-wait"].max, 40.0);
        assert!(!s.phases.contains_key("render"), "not measured");
        assert_eq!(s.counts["draws"].max, 900);
        assert_eq!(s.counts["draws"].total, 9_800 + 1_300);
        assert_eq!(s.over_budget, 2);
        assert_eq!(s.spikes.len(), 2);
        let worst = &s.spikes[0];
        assert_eq!((worst.present, worst.ms), (99, 60.0));
        assert_eq!(worst.tick, Some(530), "ended after the second tick");
        assert_eq!(worst.top, ["gpu-wait", "cpu"]);
        assert_eq!(worst.counts["draws"], 400);
        assert_eq!(s.spikes[1].tick, Some(500));
        assert_eq!(s.spikes[1].phases["cpu"], 30.0);
        // Fewer spikes listed than there were: the count still says so.
        let s = summarise(&frames, &marks(), 33.3, 1);
        assert_eq!((s.spikes.len(), s.over_budget), (1, 2));
        let s = summarise(&frames, &marks(), 100.0, 10);
        assert!(s.spikes.is_empty());
    }

    #[test]
    fn spikes_say_why() {
        // Steady frames upload 2 textures and read no file bytes.
        let mut frames: Vec<FrameSample> = (0..20)
            .map(|i| {
                let mut f = frame(i, i * 16, 12, 4, 100);
                f.counts[Counter::Uploads as usize] = 2;
                f
            })
            .collect();
        let typical = medians(&frames);
        // A shader compile with a burst of uploads.
        let mut a = frame(20, 400, 50, 5, 100);
        a.counts[Counter::Shaders as usize] = 3;
        a.counts[Counter::Uploads as usize] = 9;
        assert_eq!(causes(&a, &typical), ["shader_compile", "texture_uploads"]);
        // Waiting on the GPU, uploads as usual.
        let mut b = frame(21, 500, 10, 40, 100);
        b.counts[Counter::Uploads as usize] = 3;
        assert_eq!(causes(&b, &typical), ["gpu"]);
        // A level streaming in: the file calls took 6 ms.
        let mut c = frame(22, 600, 40, 2, 100);
        c.counts[Counter::ReadBytes as usize] = 1 << 20;
        c.counts[Counter::FileIoUs as usize] = 6_000;
        assert_eq!(causes(&c, &typical), ["file_io"]);
        // Nothing counted: the guest's own time.
        let d = frame(23, 700, 60, 2, 100);
        assert_eq!(causes(&d, &typical), ["guest_cpu"]);
        frames.extend([a, b, c, d]);
        let s = summarise(&frames, &[], 33.3, 2);
        assert_eq!(s.spikes.len(), 2);
        assert_eq!(s.over_budget, 4);
        assert_eq!(s.causes["shader_compile"], 1);
        assert_eq!(s.causes["guest_cpu"], 1, "counted for unlisted spikes too");
        assert_eq!(s.causes.values().sum::<usize>(), 5);
    }

    #[test]
    fn bytes_read_without_time_are_not_file_io() {
        let typical = [0; COUNTERS];
        // 1 MB read, 0.4 ms in file calls of a 42 ms frame: guest time.
        let mut f = frame(1, 100, 40, 2, 100);
        f.counts[Counter::ReadBytes as usize] = 1 << 20;
        f.counts[Counter::FileIoUs as usize] = 400;
        assert_eq!(causes(&f, &typical), ["guest_cpu"]);
        // 2 ms is material on its own.
        f.counts[Counter::FileIoUs as usize] = 2_000;
        assert!(causes(&f, &typical).contains(&"file_io".to_owned()));
        // A tenth of a short frame is material too (1.2 ms of 12 ms).
        let mut g = frame(2, 200, 10, 2, 100);
        g.counts[Counter::FileIoUs as usize] = 1_200;
        assert!(causes(&g, &typical).contains(&"file_io".to_owned()));
        g.counts[Counter::FileIoUs as usize] = 1_100;
        assert!(!causes(&g, &typical).contains(&"file_io".to_owned()));
    }

    #[test]
    fn no_frames_is_all_zero() {
        let s = summarise(&[], &[], 33.3, 10);
        assert_eq!((s.frames, s.frame_ms.max, s.over_budget), (0, 0.0, 0));
    }

    #[test]
    fn the_csv_has_a_row_per_frame_with_its_tick_and_map() {
        let dir = std::env::temp_dir().join(format!("h2-agent-csv-{}", std::process::id()));
        let path = dir.join("rows.csv");
        let mut frames = [frame(7, 10, 10, 2, 3), frame(8, 1_200, 30, 20, 5)];
        let field = "native_particles_v1_display_ranges";
        let (slot, _) = timeline::phase::counter_columns().find(|(_, label)| *label == field).unwrap();
        frames[1].counts[slot] = 21;
        assert_eq!(summarise(&frames, &marks(), 33.3, 2).counts[field].total, 21);
        write_csv(&path, &frames, &marks()).unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[0].starts_with("present,end_ms,tick,map,total_us,busy_us,cpu_us,"));
        assert!(
            lines[1].starts_with("7,10.000,500,03a_oldmombasa,12000,12000,10000,"),
            "{}",
            lines[1]
        );
        assert!(lines[2].starts_with("8,1200.000,530,"), "{}", lines[2]);
        assert_eq!(lines[1].split(',').count(), 6 + PHASES + COUNTERS);
        let column = lines[0].split(',').position(|s| s == field).unwrap();
        assert_eq!(lines[2].split(',').nth(column), Some("21"));
    }
}
