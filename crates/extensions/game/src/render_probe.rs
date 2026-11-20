//! What the agent's `render.stats` reads (crates/extensions/agent's `render`): the
//! in-between frames' counters, the pipelined render's and the frame-stats
//! window. The pieces register here when
//! they are built; each reads as `null` when it isn't running.
//!
//! `render.frame` needs nothing here: the draw diagnostics wrapper
//! (`d3d_diag`) feeds `agent::render::PROBE` itself.

#[cfg(feature = "gpu")]
mod private_frame;
#[cfg(feature = "gpu")]
pub use private_frame::PrivateFrame;

use std::sync::Mutex;
#[cfg(feature = "gpu")]
use std::sync::Arc;

use serde_json::{Value, json};

#[cfg(feature = "gpu")]
static INTERP: Mutex<Option<Arc<Mutex<d3d::interp::InterpStats>>>> = Mutex::new(None);
#[cfg(feature = "gpu")]
static HOLDS: Mutex<Option<Arc<Mutex<d3d::interp::HoldLog>>>> = Mutex::new(None);
static PIPELINE: Mutex<Option<render_bridge::pipeline_render::PipelineRender>> = Mutex::new(None);

#[cfg(feature = "gpu")]
/// The interpolated replay's counters (`d3d::InterpBackend::stats`).
pub fn set_interp(stats: Arc<Mutex<d3d::interp::InterpStats>>) {
    *INTERP.lock().unwrap_or_else(|e| e.into_inner()) = Some(stats);
}

#[cfg(feature = "gpu")]
/// E1's hold log (`d3d::InterpBackend::holds`): `interp.holds`.
pub fn set_interp_holds(holds: Arc<Mutex<d3d::interp::HoldLog>>) {
    *HOLDS.lock().unwrap_or_else(|e| e.into_inner()) = Some(holds);
}

/// The pipelined render feature (a handle onto the same state).
pub fn set_pipeline(p: render_bridge::pipeline_render::PipelineRender) {
    *PIPELINE.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

#[cfg(feature = "gpu")]
fn interp() -> Option<Value> {
    let s = INTERP.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let s = *s.lock().unwrap_or_else(|e| e.into_inner());
    let mut v = agent::render::debug_fields(&format!("{s:?}"));
    let holds = HOLDS.lock().unwrap_or_else(|e| e.into_inner()).clone();
    if let (Some(h), Value::Object(m)) = (holds, &mut v) {
        let log = h.lock().unwrap_or_else(|e| e.into_inner()).clone();
        m.insert(
            "holds".into(),
            serde_json::to_value(&log).unwrap_or(Value::Null),
        );
    }
    Some(v)
}

fn pipeline() -> Option<Value> {
    let p = PIPELINE.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    Some(agent::render::pipeline_json(&p.stats()))
}

/// The frame-stats window's per-frame means of the render counters
/// (`profile.counters` has the rest).
fn counters() -> Option<Value> {
    let line = timeline::phase::window_summary()?.short_line();
    Some(json!({ "line": line }))
}

pub fn sources() -> agent::render::RenderSources {
    agent::render::RenderSources {
        #[cfg(feature = "gpu")]
        interp: Box::new(interp),
        #[cfg(not(feature = "gpu"))]
        interp: Box::new(|| None),
        pipeline: Box::new(pipeline),
        counters: Box::new(counters),
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    #[test]
    fn registered_pieces_read_as_json() {
        let stats = Arc::new(Mutex::new(d3d::interp::InterpStats {
            frames: 12,
            unsure_camera: 3,
            ..Default::default()
        }));
        set_interp(Arc::clone(&stats));
        let v = interp().unwrap();
        assert_eq!(v["frames"], 12);
        assert_eq!(v["unsure_camera"], 3);
        assert_eq!(v["dropped"], 0, "every field, zero or not");
        stats.lock().unwrap().dropped = 2;
        assert_eq!(interp().unwrap()["dropped"], 2, "read live");
        assert!(
            interp().unwrap().get("holds").is_none(),
            "no hold log registered"
        );
        let holds = Arc::new(Mutex::new(d3d::interp::HoldLog::default()));
        set_interp_holds(Arc::clone(&holds));
        holds.lock().unwrap().frames = 5;
        let v = interp().unwrap();
        assert_eq!(v["holds"]["frames"], 5, "the hold log beside the counters");
        assert_eq!(v["holds"]["kinds"], serde_json::json!([]));
        assert_eq!(v["frames"], 12);
        set_pipeline(render_bridge::pipeline_render::PipelineRender::new());
        assert_eq!(pipeline().unwrap()["pipelined"], 0);
    }
}
