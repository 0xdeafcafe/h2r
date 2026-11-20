//! A private graph and the displayed frame have independent armings.
//! Stage a graph report on the renderer until the whole draw succeeds.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Instant;

use agent_proto::frame_graph::{DisplayStats, E3Report, FrameGraphStats, PassSlot, Source};
use agent_proto::{FrameRecord, OpRecord, PassRecord, SnapRecord};

use super::{DrawInfo, FrameProbe, Recording, Stage};
use crate::compare::{Rgba, write_png};

pub(super) static PROBE: FrameProbe = FrameProbe::new();

struct State {
    epoch: u64,
    last: Option<FrameGraphStats>,
    display: Option<DisplayStats>,
}

static STATE: Mutex<State> = Mutex::new(State {
    epoch: 0,
    last: None,
    display: None,
});

pub fn last_stats() -> Option<FrameGraphStats> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).last.clone()
}

pub fn display_stats() -> Option<DisplayStats> {
    STATE
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .display
        .clone()
}

pub fn display_presented(stats: DisplayStats) {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).display = Some(stats);
}

/// Release the original report when these were the commands the worker presented.
pub fn display_declined(swap: u64) {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).display = None;
    if !super::PROBE.armed() {
        return;
    }
    let mut p = super::PROBE.lock();
    if matches!(&p.stage, Stage::Routing { swap: pending, .. } if *pending == swap)
        && let Stage::Routing { original, .. } = std::mem::replace(&mut p.stage, Stage::Idle)
    {
        p.last = Some(*original);
        super::PROBE.armed.store(false, Ordering::Relaxed);
    }
}

/// A failed Swap produced no displayed frame. Keep the request until the
/// recorder sees the next complete one.
pub fn discard_display(swap: u64) {
    let mut p = super::PROBE.lock();
    if matches!(&p.stage, Stage::Routing { swap: pending, .. } if *pending == swap)
        && let Stage::Routing { arming, .. } = std::mem::replace(&mut p.stage, Stage::Idle)
    {
        p.stage = Stage::Waiting(arming);
    }
}

pub fn display_stopped() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.display = None;
    let mut p = super::PROBE.lock();
    if matches!(p.stage, Stage::Routing { .. })
        && let Stage::Routing { arming, .. } = std::mem::replace(&mut p.stage, Stage::Idle)
    {
        p.stage = Stage::Waiting(arming);
    }
}

/// Forget pending requests, reports and in-flight results from the old renderer.
pub(super) fn reset() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.epoch = state.epoch.wrapping_add(1);
    state.last = None;
    state.display = None;
    let mut p = PROBE.lock();
    p.stage = Stage::Idle;
    p.last = None;
    p.presents = 0;
    PROBE.armed.store(false, Ordering::Relaxed);
}

/// A single graph execution. Its report is published after successful
/// completion. A rejected private draw releases its arming for a retry.
pub struct GraphRecording {
    epoch: u64,
    source: Source,
    stats: FrameGraphStats,
    recording: Option<Recording>,
    snaps: Vec<(usize, Result<Rgba, String>)>,
}

impl GraphRecording {
    pub fn begin(stats: FrameGraphStats) -> Self {
        let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = PROBE.lock();
        let arming = if let Stage::Waiting(arming) = &p.stage {
            let arming = arming.clone();
            p.stage = Stage::Graph(arming.clone());
            Some(arming)
        } else {
            None
        };
        let from_present = stats.present.saturating_sub(1);
        Self::new(state.epoch, Source::Private, stats, arming, from_present)
    }

    pub fn begin_display(swap: u64, stats: impl FnOnce() -> FrameGraphStats) -> Option<Self> {
        if !super::PROBE.armed() {
            return None;
        }
        let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let p = super::PROBE.lock();
        let Stage::Routing {
            swap: pending,
            arming,
            original,
        } = &p.stage
        else {
            return None;
        };
        if *pending != swap {
            return None;
        }
        let mut stats = stats();
        stats.present = original.present;
        Some(Self::new(
            state.epoch,
            Source::Display,
            stats,
            Some(arming.clone()),
            original.from_present,
        ))
    }

    fn new(
        epoch: u64,
        source: Source,
        stats: FrameGraphStats,
        arming: Option<super::Arming>,
        from_present: u64,
    ) -> Self {
        let recording = arming.map(|arming| Recording {
            rec: FrameRecord {
                seq: arming.seq,
                from_present,
                present: stats.present,
                ..Default::default()
            },
            arming,
            began: Instant::now(),
            targets: BTreeSet::new(),
            key: None,
        });
        Self {
            epoch,
            source,
            stats,
            recording,
            snaps: Vec::new(),
        }
    }

    /// The graph explicitly ended one run of work on these attachments.
    /// It may have split two runs on identical attachments with a copy.
    pub fn pass(&mut self, color: impl FnOnce() -> String, depth: impl FnOnce() -> Option<String>) {
        self.stats.passes += 1;
        if let Some(r) = &mut self.recording {
            let color = color();
            r.targets.insert(color.clone());
            r.rec.passes.push(PassRecord {
                index: r.rec.passes.len(),
                color,
                depth: depth(),
                ..Default::default()
            });
        }
    }

    pub fn slot(&mut self, view: u8, name: &str) {
        if let Some(r) = &mut self.recording {
            let slots = &mut r.pass().slots;
            if slots
                .last()
                .is_none_or(|last| last.view != view || last.slot != name)
            {
                slots.push(PassSlot {
                    view,
                    slot: name.to_owned(),
                });
            }
        }
    }

    pub fn draw(&mut self, info: impl FnOnce() -> DrawInfo) {
        self.stats.draws += 1;
        if let Some(r) = &mut self.recording {
            r.draw(info());
        }
    }

    pub fn stage(&mut self, name: &str) {
        if let Some(r) = &mut self.recording {
            r.pass().stages.push(name.to_owned());
        }
    }

    pub fn clear(&mut self) {
        self.stats.clears += 1;
        if let Some(r) = &mut self.recording {
            r.pass().clears += 1;
            r.rec.totals.clears += 1;
        }
    }

    pub fn copy(&mut self, targets: impl FnOnce() -> (String, String)) {
        self.stats.copies += 1;
        if let Some(r) = &mut self.recording {
            let (from, to) = targets();
            r.rec.ops.push(OpRecord {
                op: "copy".into(),
                surface: from,
                detail: to,
                after_draws: r.rec.totals.draws,
            });
            r.rec.totals.copies += 1;
        }
    }

    /// The callback reads this pass's actual colour before any later op
    /// can change it. Only requested passes incur a GPU readback.
    pub fn snapshot(&mut self, index: usize, read: impl FnOnce() -> Result<Rgba, String>) {
        if self
            .recording
            .as_ref()
            .is_some_and(|r| r.arming.snap.contains(&index))
        {
            self.snaps.push((index, read()));
        }
    }

    /// Commit counts and the armed report only after every op and final
    /// target readback succeeded. A reset or rearming ignores late results.
    pub fn complete(
        mut self,
        scene_hash: u64,
        render_passes: u64,
        backend_draws: u64,
        submits: u64,
        e3: Option<E3Report>,
    ) {
        self.stats.e3 = e3;
        self.stats.scene_hash = format!("{scene_hash:016x}");
        self.stats.render_passes = render_passes;
        self.stats.backend_draws = backend_draws;
        self.stats.submits = submits;
        let finished = self.recording.take().map(|mut r| {
            r.rec.ms = r.began.elapsed().as_secs_f64() * 1000.0;
            r.rec.totals.passes = r.rec.passes.len();
            r.rec.totals.targets = r.targets.len();
            match self.source {
                Source::Private => r.rec.private_frame = Some(self.stats.clone()),
                Source::Display => r.rec.display_frame = Some(self.stats.clone()),
            }
            for (pass, result) in self.snaps.drain(..) {
                let result = result.and_then(|rgba| {
                    let prefix = match self.source {
                        Source::Private => "private",
                        Source::Display => "display",
                    };
                    let name = format!(
                        "{prefix}-{}-{}-pass-{pass}.png",
                        std::process::id(),
                        r.arming.seq
                    );
                    let path = agent_proto::cache_dir().join("frames").join(name);
                    write_png(&path, &rgba).map_err(|e| e.to_string())?;
                    Ok(path.display().to_string())
                });
                r.rec.snaps.push(match result {
                    Ok(file) => SnapRecord {
                        pass,
                        file: Some(file),
                        error: None,
                    },
                    Err(error) => SnapRecord {
                        pass,
                        file: None,
                        error: Some(error),
                    },
                });
            }
            for &pass in r.arming.snap.range(r.rec.passes.len()..) {
                r.rec.snaps.push(SnapRecord {
                    pass,
                    file: None,
                    error: Some("no such graph pass".into()),
                });
            }
            r.rec
        });
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if self.source == Source::Private && state.epoch != self.epoch {
            return;
        }
        let probe = match self.source {
            Source::Private => {
                state.last = Some(self.stats.clone());
                &PROBE
            }
            Source::Display => &super::PROBE,
        };
        let mut p = probe.lock();
        if self.source == Source::Private {
            p.presents = self.stats.present;
        }
        if let Some(frame) = finished {
            let expected = match &p.stage {
                Stage::Graph(active) => self.source == Source::Private && active.seq == frame.seq,
                Stage::Routing {
                    arming, original, ..
                } => {
                    self.source == Source::Display
                        && arming.seq == frame.seq
                        && original.present == frame.present
                }
                _ => false,
            };
            if !expected {
                return;
            }
            p.last = Some(frame);
            p.stage = Stage::Idle;
            probe.armed.store(false, Ordering::Relaxed);
        }
    }
}

impl Drop for GraphRecording {
    fn drop(&mut self) {
        if self.source == Source::Private
            && let Some(r) = self.recording.take()
        {
            let mut p = PROBE.lock();
            if matches!(&p.stage, Stage::Graph(active) if active.seq == r.arming.seq) {
                p.stage = Stage::Waiting(r.arming);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_proto::{CaptureLighting, FrameParams};

    fn stats() -> FrameGraphStats {
        FrameGraphStats {
            map_epoch: 4,
            frame: 8,
            tick: 14,
            present: 9,
            lighting: CaptureLighting::E3,
            scene_hash: String::new(),
            passes: 0,
            draws: 0,
            clears: 0,
            copies: 0,
            render_passes: 0,
            backend_draws: 0,
            submits: 0,
            e3: None,
        }
    }

    fn begin() -> GraphRecording {
        GraphRecording::begin(stats())
    }

    #[test]
    fn an_armed_private_graph_reports_only_the_completed_draw_and_requested_snapshots() {
        reset();
        let p = FrameParams {
            arm: true,
            draws: true,
            max_draws: Some(1),
            snap: vec![1, 4],
            ..FrameParams::default()
        };
        let old = PROBE.arm(&p).unwrap().seq.unwrap();
        let mut interrupted = begin();
        interrupted.pass(|| "SceneColour".into(), || None);
        let current = PROBE.arm(&p).unwrap().seq.unwrap();
        assert_ne!(old, current);
        interrupted.complete(10, 1, 0, 1, None);
        assert_eq!(PROBE.reply().seq, Some(current));
        assert!(PROBE.reply().frame.is_none());

        // A refusal partway through the graph also leaves the request pending.
        let aborted = begin();
        drop(aborted);
        assert_eq!(PROBE.reply().seq, Some(current));
        let mut graph = begin();
        let target = || "SceneColour".to_owned();
        graph.pass(target, || None);
        graph.slot(0, "opaque");
        graph.clear();
        graph.snapshot(0, || panic!("the first pass was not requested"));
        graph.copy(|| (target(), "Aux(0)".into()));
        graph.pass(target, || None);
        graph.slot(0, "opaque");
        graph.slot(0, "after_opaque");
        graph.slot(0, "after_opaque");
        for _ in 0..2 {
            graph.draw(|| DrawInfo {
                color: target(),
                vertices: 3,
                entry: "example".into(),
                outcome: "drawn".into(),
                ..Default::default()
            });
        }
        graph.stage("fog");
        graph.snapshot(1, || Err("test readback failed".into()));
        graph.complete(11, 3, 2, 1, None);

        let frame = PROBE.reply().frame.unwrap();
        assert_eq!(
            (frame.seq, frame.from_present, frame.present),
            (current, 8, 9)
        );
        assert_eq!(
            frame.totals.passes, 2,
            "a copy separates runs on identical attachments"
        );
        assert_eq!(
            (frame.totals.draws, frame.totals.clears, frame.totals.copies),
            (2, 1, 1)
        );
        assert_eq!(
            frame.passes[1]
                .slots
                .iter()
                .map(|s| s.slot.as_str())
                .collect::<Vec<_>>(),
            ["opaque", "after_opaque"]
        );
        assert_eq!(frame.passes[1].stages, ["fog"]);
        assert_eq!(frame.draws.len(), 1);
        assert!(frame.truncated);
        assert_eq!(frame.draws[0].pass, 1);
        assert_eq!(
            frame.snaps[0].error.as_deref(),
            Some("test readback failed")
        );
        assert_eq!(frame.snaps[1].error.as_deref(), Some("no such graph pass"));
        assert_eq!(frame.ops[0].after_draws, 0);
        assert_eq!(frame.private_frame, last_stats());
        let stats = last_stats().unwrap();
        assert_eq!(stats.scene_hash, "000000000000000b");
        assert_eq!(
            (stats.render_passes, stats.backend_draws, stats.submits),
            (3, 2, 1)
        );

        let before_reset = begin();
        reset();
        before_reset.complete(12, 1, 1, 1, None);
        assert!(last_stats().is_none(), "a retired renderer cannot publish");
    }

    #[test]
    fn the_display_recorder_waits_for_the_worker_to_choose_the_commands_actually_presented() {
        let p = &super::super::PROBE;
        let arm = FrameParams {
            arm: true,
            draws: true,
            ..Default::default()
        };
        p.arm(&arm).unwrap();
        p.present_routed(100, Some(1));
        p.clear("original", None);
        p.present_routed(101, Some(2));
        assert!(p.reply().armed);
        assert!(p.reply().frame.is_none());
        // The worker can still be completing an earlier frame.
        display_declined(1);
        assert!(p.reply().armed);
        let mut graph = GraphRecording::begin_display(2, stats).unwrap();
        graph.pass(|| "native".into(), || None);
        graph.clear();
        graph.complete(1234, 1, 0, 1, None);
        let frame = p.reply().frame.unwrap();
        assert_eq!((frame.from_present, frame.present), (100, 101));
        assert_eq!(frame.passes.len(), 1);
        assert_eq!(frame.passes[0].color, "native");
        assert_eq!(frame.display_frame.unwrap().present, 101);
        assert!(frame.private_frame.is_none());

        // On an original present, keep its own snapshot, including one
        // delivered just before the renderer made its decision.
        let snap = FrameParams {
            snap: vec![0],
            ..arm.clone()
        };
        p.arm(&snap).unwrap();
        p.present_routed(102, Some(3));
        p.clear("fallback", None);
        let wanted = p.present_routed(103, Some(4)).unwrap();
        p.snapped_for(wanted, Err("example".into()));
        display_declined(4);
        let frame = p.reply().frame.unwrap();
        assert!(frame.display_frame.is_none());
        assert_eq!(frame.passes[0].color, "fallback");
        assert_eq!(frame.snaps[0].error.as_deref(), Some("example"));

        p.arm(&arm).unwrap();
        p.present_routed(104, Some(5));
        p.present_routed(105, Some(6));
        discard_display(6);
        assert!(p.reply().armed);
        assert!(
            p.reply().frame.is_none(),
            "failed presentation has no report"
        );
        p.present(106);
        p.clear("next", None);
        p.present(107);
        assert_eq!(p.reply().frame.unwrap().present, 107);
    }
}
