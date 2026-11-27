//! Translate the completed graph's lighting work into agent wire rows.

use agent_proto::frame_graph::{E3Pass, E3PassName, E3Report};
use native_render::frame_graph::{FrameReport, PassWork};

pub(super) fn e3_report(frame: &FrameReport) -> Option<E3Report> {
    let work = frame.e3?;
    let pass = |name, work: PassWork| E3Pass {
        name,
        draws: work.draws,
        gpu_ns: work.gpu_ns,
    };
    Some(E3Report {
        passes: [
            pass(E3PassName::Tiles, work.tiles),
            pass(E3PassName::SunTiles, work.sun_tile_draws),
            pass(E3PassName::Sun, work.sun),
            pass(E3PassName::Marks, work.marks),
            pass(E3PassName::Lights, work.lights),
        ],
        lights_lit: work.lights_lit,
        lights_shadowed: work.lights_shadowed,
        lights_over_cap: work.lights_over_cap,
        lights_beyond_guest: work.lights_beyond_guest,
        sun_tiles: work.sun_tiles,
        sun_tiles_offscreen: work.sun_tiles_offscreen,
        sun_tiles_redrawn: work.sun_tiles_redrawn,
        atlas_halved: work.atlas_halved,
        tiles_refused: work.tiles_refused,
        cones_shadowed: work.cones_shadowed,
        cones_flagged_off: work.cones_flagged_off,
        flashlight_shadowed: work.flashlight_shadowed,
        shadows_over_cap: work.shadows_over_cap,
        cones_refused: work.cones_refused,
        cones_drawn: work.cones_drawn,
        cones_kept: work.cones_kept,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::SceneId;
    use native_render::frame_graph;
    use serde_json::json;

    #[test]
    fn e3_rows_keep_stage_counts_and_missing_gpu_samples_distinct() {
        let mut frame = FrameReport {
            id: SceneId::default(),
            lighting: d3d::Lighting::Retail,
            e3: None,
            draws: 28,
            render_passes: 11,
            submits: 2,
            targets: 3,
        };
        assert!(e3_report(&frame).is_none());
        frame.lighting = d3d::Lighting::E3;
        frame.e3 = Some(frame_graph::E3Report {
            tiles: PassWork {
                draws: 6,
                gpu_ns: Some(34),
            },
            sun_tile_draws: PassWork {
                draws: 2,
                gpu_ns: None,
            },
            sun: PassWork::default(),
            marks: PassWork {
                draws: 3,
                gpu_ns: None,
            },
            lights: PassWork {
                draws: 4,
                gpu_ns: Some(17),
            },
            lights_lit: 5,
            lights_shadowed: 3,
            lights_over_cap: 2,
            lights_beyond_guest: 1,
            sun_tiles: 4,
            sun_tiles_offscreen: 2,
            sun_tiles_redrawn: 1,
            atlas_halved: 2,
            tiles_refused: 1,
            cones_shadowed: 2,
            cones_flagged_off: 2,
            flashlight_shadowed: 1,
            shadows_over_cap: 3,
            cones_refused: 0,
            cones_drawn: 1,
            cones_kept: 0,
        });
        assert_eq!(
            json!(e3_report(&frame)),
            json!({
                "passes": [
                    {"name":"e3.tiles", "draws":6, "gpu_ns":34},
                    {"name":"e3.sun_tiles", "draws":2, "gpu_ns":null},
                    {"name":"e3.sun", "draws":0, "gpu_ns":null},
                    {"name":"e3.marks", "draws":3, "gpu_ns":null},
                    {"name":"e3.lights", "draws":4, "gpu_ns":17}
                ],
                "lights_lit":5, "lights_shadowed":3, "lights_over_cap":2, "lights_beyond_guest":1,
                "sun_tiles":4, "sun_tiles_offscreen":2, "sun_tiles_redrawn":1, "atlas_halved":2,
                "tiles_refused":1,
                "cones_shadowed":2, "cones_flagged_off":2, "flashlight_shadowed":1,
                "shadows_over_cap":3, "cones_refused":0, "cones_drawn":1, "cones_kept":0
            })
        );
    }
}
