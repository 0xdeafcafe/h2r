//! What the agent's `render.stats` reads (crates/agent's `render`): the
//! in-between frames' counters, the pipelined render's, the native render
//! paths' calls and the frame-stats window. The pieces register here when
//! they are built; each reads as `null` when it isn't running.
//!
//! `render.frame` needs nothing here: the draw diagnostics wrapper
//! (`d3d_diag`) feeds `agent::render::PROBE` itself.

use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

static INTERP: Mutex<Option<Arc<Mutex<d3d::interp::InterpStats>>>> = Mutex::new(None);
static PIPELINE: Mutex<Option<feature::pipeline_render::PipelineRender>> = Mutex::new(None);

/// The interpolated replay's counters (`d3d::InterpBackend::stats`).
pub fn set_interp(stats: Arc<Mutex<d3d::interp::InterpStats>>) {
    *INTERP.lock().unwrap_or_else(|e| e.into_inner()) = Some(stats);
}

/// The pipelined render feature (a handle onto the same state).
pub fn set_pipeline(p: feature::pipeline_render::PipelineRender) {
    *PIPELINE.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
}

fn interp() -> Option<Value> {
    let s = INTERP.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    let s = *s.lock().unwrap_or_else(|e| e.into_inner());
    Some(agent::render::debug_fields(&format!("{s:?}")))
}

fn pipeline() -> Option<Value> {
    let p = PIPELINE.lock().unwrap_or_else(|e| e.into_inner()).clone()?;
    Some(agent::render::pipeline_json(&p.stats()))
}

/// The frame-stats window's per-frame means of the render counters
/// (`profile.counters` has the rest).
fn counters() -> Option<Value> {
    let i = timeline::phase::window_summary()?;
    Some(json!({ "line": i.short_line() }))
}

pub fn sources() -> agent::render::RenderSources {
    agent::render::RenderSources {
        interp: Box::new(interp),
        pipeline: Box::new(pipeline),
        paths: Some(feature::render_loop::path_calls),
        counters: Box::new(counters),
    }
}

#[cfg(test)]
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
        set_pipeline(feature::pipeline_render::PipelineRender::new());
        assert_eq!(pipeline().unwrap()["pipelined"], 0);
        let paths = (sources().paths.unwrap())();
        assert!(paths.iter().any(|p| p.0 == "render_views"));
    }
}
