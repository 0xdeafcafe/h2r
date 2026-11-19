//! `render.frame` and `render.stats` (docs/runtime/agent-api.md, phase 5).
//!
//! [`FrameProbe`] records one whole guest frame when armed: the backend
//! wrapper in front of the D3D backend (boot's `d3d_diag`) tells it each
//! clear, draw, copy, readback and present, as plain text and numbers, so
//! this crate needs no `d3d` types. Unarmed, each call site costs one
//! relaxed load ([`FrameProbe::armed`]).
//!
//! A frame is everything between two presents: arming waits for the next
//! present, records until the one after, then disarms. Work on one colour
//! target (and depth) in a row is a pass; going back to a target makes a
//! new pass.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use agent_proto::{
    DepthRecord, DrawRecord, FrameParams, FrameRecord, FrameReply, OpRecord, PassRecord,
    RenderStats, RpcError, SnapRecord, code, method,
};
use serde_json::{Map, Value, json};

use crate::exec::{Access, Agent};

/// The calls into each native render path so far (name, calls).
pub type PathCalls = fn() -> Vec<(&'static str, u64)>;

/// One draw, as the backend wrapper describes it.
#[derive(Debug, Clone, Default)]
pub struct DrawInfo {
    pub color: String,
    pub depth: Option<String>,
    /// `640x480 X8R8G8B8`.
    pub target: String,
    pub entry: String,
    pub primitive: String,
    pub vertices: u32,
    pub indices: u32,
    /// `drawn`, or why not.
    pub outcome: String,
    /// Each bound texture's format.
    pub textures: Vec<String>,
    pub program: bool,
    pub blend: bool,
    pub depth_write: bool,
    /// The vertex side, as [`DrawRecord::vs`].
    pub vs: String,
    /// How the draw tests and stores depth, as [`DrawRecord::depth`]
    /// (`depth` here is the depth surface).
    pub depth_state: Option<DepthRecord>,
    /// The draw's constant registers, hardware order (D3D register + 96),
    /// shared with the backend: the probe copies out the ones asked for.
    pub constants: Option<Arc<[[f32; 4]]>>,
}

/// What the wrapper should do after telling the probe about a draw or a
/// clear: read back the pass that just ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapAsk {
    /// The arming's number, for the file name.
    pub seq: u64,
    pub pass: usize,
}

/// The recorder. One per process ([`PROBE`]); tests make their own.
pub struct FrameProbe {
    armed: AtomicBool,
    state: Mutex<ProbeState>,
}

struct ProbeState {
    next_seq: u64,
    stage: Stage,
    last: Option<FrameRecord>,
    presents: u64,
    paths: Option<PathCalls>,
}

enum Stage {
    Idle,
    Waiting(Arming),
    Recording(Box<Recording>),
}

#[derive(Clone)]
struct Arming {
    seq: u64,
    at_present: u64,
    draws: bool,
    max_draws: usize,
    snap: BTreeSet<usize>,
    constants: Vec<i32>,
}

struct Recording {
    arming: Arming,
    began: Instant,
    paths_before: Vec<(&'static str, u64)>,
    rec: FrameRecord,
    targets: BTreeSet<String>,
    /// The current pass's colour and depth.
    key: Option<(String, Option<String>)>,
}

/// The game's probe.
pub static PROBE: FrameProbe = FrameProbe::new();

impl Default for FrameProbe {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameProbe {
    pub const fn new() -> FrameProbe {
        FrameProbe {
            armed: AtomicBool::new(false),
            state: Mutex::new(ProbeState {
                next_seq: 1,
                stage: Stage::Idle,
                last: None,
                presents: 0,
                paths: None,
            }),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ProbeState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Whether a frame is wanted: the one check on the draw path.
    #[inline]
    pub fn armed(&self) -> bool {
        self.armed.load(Ordering::Relaxed)
    }

    /// Where native render path calls are counted (boot's
    /// `feature::render_loop::path_calls`).
    pub fn set_paths(&self, paths: PathCalls) {
        self.lock().paths = Some(paths);
    }

    /// Arms for the next whole frame; forgets the last one.
    pub fn arm(&self, p: &FrameParams) -> Result<FrameReply, String> {
        let max_draws = p.max_draws.unwrap_or(FrameParams::DEFAULT_DRAWS);
        if max_draws > FrameParams::MAX_DRAWS {
            return Err(format!("max_draws is at most {}", FrameParams::MAX_DRAWS));
        }
        if p.snap.len() > FrameParams::MAX_SNAPS {
            return Err(format!("snap at most {} passes", FrameParams::MAX_SNAPS));
        }
        if p.constants.len() > FrameParams::MAX_CONSTANTS {
            return Err(format!(
                "constants: at most {} registers",
                FrameParams::MAX_CONSTANTS
            ));
        }
        if let Some(r) = p
            .constants
            .iter()
            .find(|r| !FrameParams::CONSTANT_REGISTERS.contains(r))
        {
            return Err(format!("constants: c{r} is not a register (-96..=95)"));
        }
        if !p.constants.is_empty() && !p.draws {
            return Err("constants go with draws".into());
        }
        let mut s = self.lock();
        let seq = s.next_seq;
        s.next_seq += 1;
        let at_present = s.presents;
        s.stage = Stage::Waiting(Arming {
            seq,
            at_present,
            draws: p.draws,
            max_draws,
            snap: p.snap.iter().copied().collect(),
            constants: p.constants.clone(),
        });
        s.last = None;
        self.armed.store(true, Ordering::Relaxed);
        Ok(reply(&s))
    }

    /// The last frame, and whether one is still to come.
    pub fn reply(&self) -> FrameReply {
        reply(&self.lock())
    }

    fn recording<R>(&self, f: impl FnOnce(&mut Recording) -> R) -> Option<R> {
        if !self.armed() {
            return None;
        }
        match &mut self.lock().stage {
            Stage::Recording(r) => Some(f(r)),
            _ => None,
        }
    }

    /// Work is about to go to `color` (and `depth`): when that ends a pass
    /// that wants a snapshot, read the pass's target back before the work
    /// goes on. [`clear`](Self::clear) and [`draw`](Self::draw) do this too;
    /// a wrapper that snapshots calls it first.
    pub fn enter(&self, color: &str, depth: Option<&str>) -> Option<SnapAsk> {
        self.recording(|r| r.enter(color, depth)).flatten()
    }

    /// A clear, whole-target or by rectangles.
    pub fn clear(&self, color: &str, depth: Option<&str>) -> Option<SnapAsk> {
        self.recording(|r| {
            let ask = r.enter(color, depth);
            let p = r.pass();
            p.clears += 1;
            r.rec.totals.clears += 1;
            ask
        })
        .flatten()
    }

    pub fn draw(&self, d: DrawInfo) -> Option<SnapAsk> {
        self.recording(|r| {
            let ask = r.enter(&d.color, d.depth.as_deref());
            r.targets.insert(d.color.clone());
            let index = r.rec.totals.draws;
            let pass_index = r.rec.passes.len() - 1;
            let p = r.pass();
            p.target.get_or_insert_with(|| d.target.clone());
            p.draws += 1;
            p.vertices += u64::from(d.vertices);
            p.first_draw.get_or_insert(index);
            *p.outcomes.entry(d.outcome.clone()).or_default() += 1;
            *p.primitives.entry(d.primitive.clone()).or_default() += 1;
            *p.entries.entry(d.entry.clone()).or_default() += 1;
            for t in &d.textures {
                *p.textures.entry(t.clone()).or_default() += 1;
            }
            p.programs += u64::from(d.program);
            p.blended += u64::from(d.blend);
            p.depth_writes += u64::from(d.depth_write);
            let t = &mut r.rec.totals;
            t.draws += 1;
            t.vertices += u64::from(d.vertices);
            *t.outcomes.entry(d.outcome.clone()).or_default() += 1;
            if r.arming.draws {
                if r.rec.draws.len() < r.arming.max_draws {
                    let constants = picked_constants(&r.arming.constants, d.constants.as_deref());
                    r.rec.draws.push(DrawRecord {
                        index,
                        pass: pass_index,
                        entry: d.entry,
                        primitive: d.primitive,
                        vertices: d.vertices,
                        indices: d.indices,
                        outcome: d.outcome,
                        textures: d.textures,
                        program: d.program,
                        blend: d.blend,
                        depth_write: d.depth_write,
                        vs: d.vs,
                        depth: d.depth_state,
                        constants,
                    });
                } else {
                    r.rec.truncated = true;
                }
            }
            ask
        })
        .flatten()
    }

    /// A surface copy or readback (`op`: `copy`, `read`, `read_depth`).
    pub fn op(&self, op: &str, surface: &str, detail: String) {
        self.recording(|r| {
            match op {
                "copy" => r.rec.totals.copies += 1,
                _ => r.rec.totals.reads += 1,
            }
            let after_draws = r.rec.totals.draws;
            r.rec.ops.push(OpRecord {
                op: op.into(),
                surface: surface.into(),
                detail,
                after_draws,
            });
        });
    }

    /// A guest present, numbered `n` (the run's count after it). Returns
    /// the last pass when it wants a snapshot: read it back before the
    /// present goes on.
    pub fn present(&self, n: u64) -> Option<SnapAsk> {
        let mut s = self.lock();
        s.presents = n;
        if !self.armed() {
            return None;
        }
        let paths = s.paths;
        match std::mem::replace(&mut s.stage, Stage::Idle) {
            Stage::Idle => None,
            Stage::Waiting(arming) => {
                s.stage = Stage::Recording(Box::new(Recording {
                    rec: FrameRecord {
                        seq: arming.seq,
                        from_present: n,
                        ..Default::default()
                    },
                    arming,
                    began: Instant::now(),
                    paths_before: paths.map(|f| f()).unwrap_or_default(),
                    targets: BTreeSet::new(),
                    key: None,
                }));
                None
            }
            Stage::Recording(mut r) => {
                let ask = r
                    .key
                    .as_ref()
                    .and_then(|_| r.wants_snap(r.rec.passes.len() - 1));
                r.rec.present = n;
                r.rec.ms = (r.began.elapsed().as_secs_f64() * 1e5).round() / 100.0;
                r.rec.totals.passes = r.rec.passes.len();
                r.rec.totals.targets = r.targets.len();
                if let Some(f) = paths {
                    let before: BTreeMap<&str, u64> = r.paths_before.iter().copied().collect();
                    for (name, calls) in f() {
                        let d = calls.saturating_sub(before.get(name).copied().unwrap_or(0));
                        if d > 0 {
                            r.rec.paths.insert(name.to_owned(), d);
                        }
                    }
                }
                s.last = Some(r.rec);
                if ask.is_none() {
                    self.armed.store(false, Ordering::Relaxed);
                }
                ask
            }
        }
    }

    /// Where a snapshot the probe asked for went, or why it failed. The
    /// frame may be done already (a snapshot asked for at its present).
    pub fn snapped(&self, pass: usize, result: Result<String, String>) {
        let mut s = self.lock();
        let snap = match result {
            Ok(file) => SnapRecord {
                pass,
                file: Some(file),
                error: None,
            },
            Err(e) => SnapRecord {
                pass,
                file: None,
                error: Some(e),
            },
        };
        match &mut s.stage {
            Stage::Recording(r) => r.rec.snaps.push(snap),
            _ => {
                if let Some(last) = &mut s.last {
                    last.snaps.push(snap);
                }
                if matches!(s.stage, Stage::Idle) {
                    self.armed.store(false, Ordering::Relaxed);
                }
            }
        }
    }
}

impl Recording {
    /// Starts a new pass unless this work is on the current pass's
    /// targets; answers the pass that ended when it wants a snapshot.
    fn enter(&mut self, color: &str, depth: Option<&str>) -> Option<SnapAsk> {
        let key = (color.to_owned(), depth.map(str::to_owned));
        if self.key.as_ref() == Some(&key) {
            return None;
        }
        let ended = self
            .key
            .as_ref()
            .and_then(|_| self.wants_snap(self.rec.passes.len() - 1));
        self.targets.insert(color.to_owned());
        let index = self.rec.passes.len();
        self.rec.passes.push(PassRecord {
            index,
            color: key.0.clone(),
            depth: key.1.clone(),
            ..Default::default()
        });
        self.key = Some(key);
        ended
    }

    fn pass(&mut self) -> &mut PassRecord {
        self.rec.passes.last_mut().expect("entered")
    }

    fn wants_snap(&self, pass: usize) -> Option<SnapAsk> {
        let seq = self.arming.seq;
        self.arming
            .snap
            .contains(&pass)
            .then_some(SnapAsk { seq, pass })
    }
}

fn reply(s: &ProbeState) -> FrameReply {
    let arming = match &s.stage {
        Stage::Idle => None,
        Stage::Waiting(a) => Some(a),
        Stage::Recording(r) => Some(&r.arming),
    };
    FrameReply {
        armed: arming.is_some(),
        seq: arming.map(|a| a.seq),
        armed_at_present: arming.map(|a| a.at_present),
        frame: s.last.clone(),
    }
}

/// A struct's `Debug` text (`Name { a: 1, b: 2 }`) as a JSON object of its
/// numeric and boolean fields. How counters from crates without serde
/// (`d3d::interp::InterpStats`) reach the wire without listing their
/// fields here, so a field added there shows up with no change here.
pub fn debug_fields(text: &str) -> Value {
    let body = match (text.find('{'), text.rfind('}')) {
        (Some(a), Some(b)) if a < b => &text[a + 1..b],
        _ => return Value::Null,
    };
    let mut out = Map::new();
    for part in body.split(',') {
        let Some((k, v)) = part.split_once(':') else {
            continue;
        };
        let (k, v) = (k.trim(), v.trim());
        if k.is_empty() || !k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            continue;
        }
        let v = match v {
            "true" => json!(true),
            "false" => json!(false),
            _ => match v.parse::<u64>() {
                Ok(n) => json!(n),
                Err(_) => match v.parse::<f64>() {
                    Ok(f) => json!(f),
                    Err(_) => continue,
                },
            },
        };
        out.insert(k.to_owned(), v);
    }
    Value::Object(out)
}

/// The pipelined render's totals (`feature::pipeline_render::Stats`) as
/// JSON: counts as they are, times in ms, the per-reason maps as objects.
pub fn pipeline_json(s: &feature::pipeline_render::Stats) -> Value {
    let ms = |d: std::time::Duration| (d.as_secs_f64() * 1e5).round() / 100.0;
    json!({
        "pipelined": s.pipelined,
        "completed": s.completed,
        "serial": s.serial,
        "fallbacks": s.fallbacks,
        "serviced": s.serviced,
        "d3d_joins": s.d3d_joins,
        "cache_joins": s.cache_joins,
        "sim_joins": s.sim_joins,
        "d3d_other_thread": s.d3d_other_thread,
        "lead_ms": ms(s.lead),
        "join_wait_ms": ms(s.join_wait),
        "hand_back_ms": ms(s.hand_back),
        "snapshot_ms": ms(s.snapshot),
        "render_ms": ms(s.render),
        "pages": s.pages,
        "bytes": s.bytes,
        "conflicts": s.conflicts,
        "owned_conflicts": s.owned_conflicts,
        "mispredicts": s.mispredicts,
        "accesses": s.accesses,
        "io_stalls": s.io_stalls,
        "io_stall_ms": ms(s.io_stall),
        "io_stalls_render": s.io_stalls_render,
        "io_stalls_join": s.io_stalls_join,
        "io_deferred": s.io_deferred,
        "io_refused": s.io_refused,
        "io_joins": s.io_joins,
        "io_sim_first": s.io_sim_first,
        "io_mismatches": s.io_mismatches,
        "io_other_thread": s.io_other_thread,
    })
}

/// What `render.stats` reads, each `None` where that piece isn't running.
pub struct RenderSources {
    pub interp: Box<dyn Fn() -> Option<Value> + Send>,
    pub pipeline: Box<dyn Fn() -> Option<Value> + Send>,
    pub paths: Option<PathCalls>,
    pub counters: Box<dyn Fn() -> Option<Value> + Send>,
}

/// Adds `render.frame` (over `probe`) and `render.stats` (over `sources`).
pub fn add_methods(agent: &Agent, probe: &'static FrameProbe, sources: RenderSources) {
    if let Some(p) = sources.paths {
        probe.set_paths(p);
    }
    agent.add_method(method::RENDER_FRAME, Access::Read, move |v| {
        let p: FrameParams = if v.is_null() {
            FrameParams::default()
        } else {
            serde_json::from_value(v)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?
        };
        let r = if p.arm {
            probe
                .arm(&p)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e))?
        } else {
            if p.draws || p.max_draws.is_some() || !p.snap.is_empty() || !p.constants.is_empty() {
                return Err(RpcError::new(
                    code::INVALID_PARAMS,
                    "draws, max_draws, snap and constants go with arm",
                ));
            }
            probe.reply()
        };
        serde_json::to_value(r).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
    });
    agent.add_method(method::RENDER_STATS, Access::Read, move |v| {
        if !(v.is_null() || v.as_object().is_some_and(Map::is_empty)) {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "render.stats takes no params",
            ));
        }
        let s = RenderStats {
            interp: (sources.interp)().unwrap_or(Value::Null),
            pipeline: (sources.pipeline)().unwrap_or(Value::Null),
            paths: sources
                .paths
                .map(|f| f().into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
                .unwrap_or_default(),
            counters: (sources.counters)().unwrap_or(Value::Null),
        };
        serde_json::to_value(s).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))
    });
}

/// The registers `wanted` (D3D numbering) out of a draw's constants
/// (hardware order): those the block has.
fn picked_constants(wanted: &[i32], block: Option<&[[f32; 4]]>) -> BTreeMap<i32, [f32; 4]> {
    let Some(block) = block else {
        return BTreeMap::new();
    };
    wanted
        .iter()
        .filter_map(|&r| {
            let i = usize::try_from(r + 96).ok()?;
            block.get(i).map(|v| (r, *v))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(color: &str, outcome: &str, verts: u32) -> DrawInfo {
        DrawInfo {
            color: color.into(),
            depth: Some("z".into()),
            target: "640x480 X8R8G8B8".into(),
            entry: "DrawIndexedVertices".into(),
            primitive: "TriangleList".into(),
            vertices: verts,
            indices: verts * 2,
            outcome: outcome.into(),
            textures: vec!["Dxt1".into()],
            program: true,
            blend: false,
            depth_write: true,
            ..DrawInfo::default()
        }
    }

    #[test]
    fn unarmed_records_nothing() {
        let p = FrameProbe::new();
        assert!(!p.armed());
        assert_eq!(p.draw(draw("a", "drawn", 3)), None);
        assert_eq!(p.present(1), None);
        let r = p.reply();
        assert!(!r.armed && r.frame.is_none());
    }

    #[test]
    fn a_whole_frame_between_two_presents_in_passes() {
        static PATHS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        fn paths() -> Vec<(&'static str, u64)> {
            vec![
                ("render_views", PATHS.load(Ordering::Relaxed)),
                ("layer_sort", 0),
            ]
        }
        let p = FrameProbe::new();
        p.set_paths(paths);
        p.present(10);
        let armed = p
            .arm(&FrameParams {
                arm: true,
                draws: true,
                max_draws: Some(3),
                ..Default::default()
            })
            .unwrap();
        assert_eq!(
            (armed.armed, armed.seq, armed.armed_at_present),
            (true, Some(1), Some(10))
        );
        // The rest of the frame being drawn when it was armed: not kept.
        p.draw(draw("shadow", "drawn", 99));
        PATHS.store(5, Ordering::Relaxed);
        p.present(11);
        p.clear("shadow", Some("z"));
        p.draw(draw("shadow", "drawn", 30));
        p.draw(draw("main", "drawn", 6));
        p.op("copy", "main", "-> mirror".into());
        p.draw(draw("main", "skipped empty", 3));
        p.draw(draw("shadow", "drawn", 3));
        PATHS.store(7, Ordering::Relaxed);
        p.present(12);
        assert!(!p.armed(), "one frame, then disarmed");
        // After it: not recorded.
        p.draw(draw("main", "drawn", 1));
        let f = p.reply().frame.unwrap();
        assert_eq!((f.seq, f.from_present, f.present), (1, 11, 12));
        assert_eq!(f.totals.draws, 4);
        assert_eq!(f.totals.vertices, 42);
        assert_eq!(
            (f.totals.clears, f.totals.copies, f.totals.targets),
            (1, 1, 2)
        );
        assert_eq!(f.totals.outcomes["skipped empty"], 1);
        let colors: Vec<&str> = f.passes.iter().map(|p| p.color.as_str()).collect();
        assert_eq!(
            colors,
            ["shadow", "main", "shadow"],
            "back to a target: a new pass"
        );
        assert_eq!(f.passes[0].clears, 1);
        assert_eq!(f.passes[0].target.as_deref(), Some("640x480 X8R8G8B8"));
        assert_eq!(f.passes[1].draws, 2);
        assert_eq!(f.passes[1].first_draw, Some(1));
        assert_eq!(f.passes[1].textures["Dxt1"], 2);
        assert_eq!(f.ops[0].after_draws, 2);
        assert_eq!(f.draws.len(), 3);
        assert!(f.truncated, "four draws, three listed");
        assert_eq!(f.draws[2].pass, 1);
        assert_eq!(f.paths.get("render_views"), Some(&2));
        assert!(!f.paths.contains_key("layer_sort"), "no calls, not listed");
    }

    #[test]
    fn snapshots_are_asked_for_when_their_pass_ends() {
        let p = FrameProbe::new();
        p.arm(&FrameParams {
            arm: true,
            snap: vec![0, 1],
            ..Default::default()
        })
        .unwrap();
        p.present(1);
        assert_eq!(p.draw(draw("a", "drawn", 3)), None);
        assert_eq!(p.draw(draw("a", "drawn", 3)), None);
        assert_eq!(
            p.draw(draw("b", "drawn", 3)),
            Some(SnapAsk { seq: 1, pass: 0 })
        );
        p.snapped(0, Ok("/tmp/p0.png".into()));
        // The last pass ends at the present: the frame is done, the probe
        // stays armed until the snapshot is in.
        assert_eq!(p.present(2), Some(SnapAsk { seq: 1, pass: 1 }));
        assert!(p.armed());
        p.snapped(1, Err("readback failed".into()));
        assert!(!p.armed());
        let f = p.reply().frame.unwrap();
        assert_eq!(f.snaps.len(), 2);
        assert_eq!(f.snaps[0].file.as_deref(), Some("/tmp/p0.png"));
        assert_eq!(f.snaps[1].error.as_deref(), Some("readback failed"));
        // Nothing more is recorded while the snapshot was pending.
        assert_eq!(p.reply().frame.unwrap().totals.draws, 3);
    }

    #[test]
    fn arming_checks_its_limits_and_forgets_the_last_frame() {
        let p = FrameProbe::new();
        let bad = FrameParams {
            arm: true,
            max_draws: Some(FrameParams::MAX_DRAWS + 1),
            ..Default::default()
        };
        assert!(p.arm(&bad).is_err());
        let bad = FrameParams {
            arm: true,
            snap: vec![0, 1, 2, 3, 4],
            ..Default::default()
        };
        assert!(p.arm(&bad).is_err());
        let ok = FrameParams {
            arm: true,
            ..Default::default()
        };
        p.arm(&ok).unwrap();
        p.present(1);
        p.present(2);
        assert!(p.reply().frame.is_some());
        let r = p.arm(&ok).unwrap();
        assert_eq!((r.seq, r.frame), (Some(2), None));
    }

    #[test]
    fn draws_carry_depth_state_and_the_constants_asked_for() {
        let p = FrameProbe::new();
        p.arm(&FrameParams {
            arm: true,
            draws: true,
            constants: vec![-96, -38, 95],
            ..Default::default()
        })
        .unwrap();
        p.present(1);
        let mut block = vec![[0.0f32; 4]; 192];
        block[0] = [1.0, 2.0, 3.0, 4.0];
        block[58] = [0.0, 0.0, 16384.0, 1.0];
        block[191] = [9.0; 4];
        let depth = DepthRecord {
            mode: "w".into(),
            w: true,
            func: "LessEqual".into(),
            write: false,
            viewport: [0.0, 1.0],
            scale: [16777215.0, 0.0],
            clip: [0.0, 16777215.0],
            full: 16777215.0,
            clamp: true,
        };
        p.draw(DrawInfo {
            vs: "prog @0 n=4 #00000001".into(),
            depth_state: Some(depth.clone()),
            constants: Some(Arc::from(block)),
            ..draw("main", "drawn", 3)
        });
        // A draw with no constant block (fixed function before any
        // register was set): no constants, no error.
        p.draw(draw("main", "drawn", 3));
        p.present(2);
        let f = p.reply().frame.unwrap();
        let d = &f.draws[0];
        assert_eq!(d.vs, "prog @0 n=4 #00000001");
        assert_eq!(d.depth.as_ref(), Some(&depth));
        assert_eq!(d.constants.len(), 3);
        assert_eq!(d.constants[&-96], [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(d.constants[&-38], [0.0, 0.0, 16384.0, 1.0]);
        assert_eq!(d.constants[&95], [9.0; 4]);
        assert!(f.draws[1].constants.is_empty());
        assert!(f.draws[1].depth.is_none());
    }

    #[test]
    fn constants_are_checked_when_arming() {
        let p = FrameProbe::new();
        let arm = |constants: Vec<i32>, draws: bool| {
            p.arm(&FrameParams {
                arm: true,
                draws,
                constants,
                ..Default::default()
            })
        };
        assert!(
            arm(vec![96], true).is_err(),
            "c96 is past the last register"
        );
        assert!(arm(vec![-97], true).is_err());
        assert!(arm((0..33).collect(), true).is_err(), "at most 32");
        assert!(arm(vec![0], false).is_err(), "only on listed draws");
        assert!(arm(vec![-96, 95], true).is_ok());
    }

    #[test]
    fn debug_text_becomes_fields() {
        #[derive(Debug, Default)]
        #[allow(dead_code)]
        struct S {
            frames: u64,
            unsure_camera: u64,
            ratio: f64,
            on: bool,
        }
        let v = debug_fields(&format!(
            "{:?}",
            S {
                frames: 9,
                unsure_camera: 2,
                ratio: 0.5,
                on: true
            }
        ));
        assert_eq!(
            v,
            json!({"frames": 9, "unsure_camera": 2, "ratio": 0.5, "on": true})
        );
        assert_eq!(debug_fields("no braces"), Value::Null);
    }

    #[test]
    fn pipeline_stats_become_json() {
        let mut s = feature::pipeline_render::Stats {
            pipelined: 4,
            completed: 3,
            join_wait: std::time::Duration::from_micros(1500),
            ..Default::default()
        };
        s.serial.insert("loading", 2);
        s.fallbacks.insert("D3DDevice_Swap".into(), 1);
        let v = pipeline_json(&s);
        assert_eq!(v["pipelined"], 4);
        assert_eq!(v["serial"]["loading"], 2);
        assert_eq!(v["fallbacks"]["D3DDevice_Swap"], 1);
        assert_eq!(v["join_wait_ms"], 1.5);
    }
}
