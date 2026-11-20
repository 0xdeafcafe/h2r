//! A private graph and the displayed frame have independent armings.
//! Stage a graph report on the renderer until the whole draw succeeds.

use std::collections::BTreeSet;
use std::sync::Mutex;
use std::sync::atomic::Ordering;
use std::time::Instant;

use agent_proto::frame_graph::{PassSlot, PrivateFrameStats};
use agent_proto::{FrameRecord, OpRecord, PassRecord, SnapRecord};

use super::{DrawInfo, FrameProbe, Recording, Stage};
use crate::compare::{Rgba, write_png};

pub(super) static PROBE: FrameProbe = FrameProbe::new();

struct State {
    epoch: u64,
    last: Option<PrivateFrameStats>,
}

static STATE: Mutex<State> = Mutex::new(State { epoch: 0, last: None });

pub fn last_stats() -> Option<PrivateFrameStats> {
    STATE.lock().unwrap_or_else(|e| e.into_inner()).last.clone()
}

/// Forget pending requests, reports and in-flight results from the old renderer.
pub(super) fn reset() {
    let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
    state.epoch = state.epoch.wrapping_add(1);
    state.last = None;
    let mut p = PROBE.lock();
    p.stage = Stage::Idle;
    p.last = None;
    p.presents = 0;
    PROBE.armed.store(false, Ordering::Relaxed);
}

/// An observer for one draw into private targets. Dropping it before
/// completion leaves the request pending for the next successful draw.
pub struct PrivateGraph {
    epoch: u64,
    stats: PrivateFrameStats,
    recording: Option<Recording>,
    snaps: Vec<(usize, Result<Rgba, String>)>,
}

impl PrivateGraph {
    pub fn begin(stats: PrivateFrameStats) -> Self {
        let state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        let mut p = PROBE.lock();
        let recording = if let Stage::Waiting(arming) = &p.stage {
            let arming = arming.clone();
            p.stage = Stage::Graph(arming.clone());
            Some(Recording {
                rec: FrameRecord {
                    seq: arming.seq,
                    from_present: stats.present.saturating_sub(1),
                    present: stats.present,
                    ..Default::default()
                },
                arming,
                began: Instant::now(),
                targets: BTreeSet::new(),
                key: None,
            })
        } else {
            None
        };
        Self { epoch: state.epoch, stats, recording, snaps: Vec::new() }
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
            if slots.last().is_none_or(|last| last.view != view || last.slot != name) {
                slots.push(PassSlot { view, slot: name.to_owned() });
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
        if self.recording.as_ref().is_some_and(|r| r.arming.snap.contains(&index)) {
            self.snaps.push((index, read()));
        }
    }

    /// Commit counts and the armed report only after every op and final
    /// target readback succeeded. A reset or rearming ignores late results.
    pub fn complete(mut self, scene_hash: u64, render_passes: u64, backend_draws: u64, submits: u64) {
        self.stats.scene_hash = format!("{scene_hash:016x}");
        self.stats.render_passes = render_passes;
        self.stats.backend_draws = backend_draws;
        self.stats.submits = submits;
        let finished = self.recording.take().map(|mut r| {
            r.rec.ms = r.began.elapsed().as_secs_f64() * 1000.0;
            r.rec.totals.passes = r.rec.passes.len();
            r.rec.totals.targets = r.targets.len();
            r.rec.private_frame = Some(self.stats.clone());
            for (pass, result) in self.snaps.drain(..) {
                let result = result.and_then(|rgba| {
                    let name = format!("private-{}-{}-pass-{pass}.png", std::process::id(), r.arming.seq);
                    let path = agent_proto::cache_dir().join("frames").join(name);
                    write_png(&path, &rgba).map_err(|e| e.to_string())?;
                    Ok(path.display().to_string())
                });
                r.rec.snaps.push(match result {
                    Ok(file) => SnapRecord { pass, file: Some(file), error: None },
                    Err(error) => SnapRecord { pass, file: None, error: Some(error) },
                });
            }
            for &pass in r.arming.snap.range(r.rec.passes.len()..) {
                r.rec.snaps.push(SnapRecord { pass, file: None, error: Some("no such graph pass".into()) });
            }
            r.rec
        });
        let mut state = STATE.lock().unwrap_or_else(|e| e.into_inner());
        if state.epoch != self.epoch {
            return;
        }
        state.last = Some(self.stats.clone());
        let mut p = PROBE.lock();
        p.presents = self.stats.present;
        if let Some(frame) = finished
            && matches!(&p.stage, Stage::Graph(active) if active.seq == frame.seq)
        {
            p.last = Some(frame);
            p.stage = Stage::Idle;
            PROBE.armed.store(false, Ordering::Relaxed);
        }
    }
}

impl Drop for PrivateGraph {
    fn drop(&mut self) {
        if let Some(r) = self.recording.take() {
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

    fn begin() -> PrivateGraph {
        PrivateGraph::begin(PrivateFrameStats {
            map_epoch: 4, frame: 8, tick: 14, present: 9,
            lighting: CaptureLighting::E3, scene_hash: String::new(),
            passes: 0, draws: 0, clears: 0, copies: 0,
            render_passes: 0, backend_draws: 0, submits: 0,
        })
    }

    #[test]
    fn an_armed_private_graph_reports_only_the_completed_draw_and_requested_snapshots() {
        reset();
        let p = FrameParams {
            arm: true, draws: true, max_draws: Some(1), snap: vec![1, 4],
            ..FrameParams::default()
        };
        let old = PROBE.arm(&p).unwrap().seq.unwrap();
        let mut interrupted = begin();
        interrupted.pass(|| "SceneColour".into(), || None);
        let current = PROBE.arm(&p).unwrap().seq.unwrap();
        assert_ne!(old, current);
        interrupted.complete(10, 1, 0, 1);
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
                color: target(), vertices: 3, entry: "example".into(), outcome: "drawn".into(), ..Default::default()
            });
        }
        graph.stage("fog");
        graph.snapshot(1, || Err("test readback failed".into()));
        graph.complete(11, 3, 2, 1);

        let frame = PROBE.reply().frame.unwrap();
        assert_eq!((frame.seq, frame.from_present, frame.present), (current, 8, 9));
        assert_eq!(frame.totals.passes, 2, "a copy separates runs on identical attachments");
        assert_eq!((frame.totals.draws, frame.totals.clears, frame.totals.copies), (2, 1, 1));
        assert_eq!(frame.passes[1].slots.iter().map(|s| s.slot.as_str()).collect::<Vec<_>>(), ["opaque", "after_opaque"]);
        assert_eq!(frame.passes[1].stages, ["fog"]);
        assert_eq!(frame.draws.len(), 1);
        assert!(frame.truncated);
        assert_eq!(frame.draws[0].pass, 1);
        assert_eq!(frame.snaps[0].error.as_deref(), Some("test readback failed"));
        assert_eq!(frame.snaps[1].error.as_deref(), Some("no such graph pass"));
        assert_eq!(frame.ops[0].after_draws, 0);
        assert_eq!(frame.private_frame, last_stats());
        let stats = last_stats().unwrap();
        assert_eq!(stats.scene_hash, "000000000000000b");
        assert_eq!((stats.render_passes, stats.backend_draws, stats.submits), (3, 2, 1));

        let before_reset = begin();
        reset();
        before_reset.complete(12, 1, 1, 1);
        assert!(last_stats().is_none(), "a retired renderer cannot publish");
    }

}
