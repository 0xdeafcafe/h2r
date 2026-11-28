//! What the agent's `render.stats` reads (crates/extensions/agent's `render`): the
//! frame owner's display-rate loop and the frame-stats window. Each reads as
//! `null` (or is absent) when it isn't running.

use serde_json::{Value, json};

#[cfg(feature = "gpu")]
/// The frame owner's display-rate loop
/// (`native_render::frame_graph::display_rate`): `render.stats`'
/// `display_rate`.
fn display_rate() -> Option<agent_proto::DisplayRateStats> {
    native_render::frame_graph::display_rate::stats().map(display_rate_of)
}

#[cfg(feature = "gpu")]
fn display_rate_of(s: native_render::frame_graph::display_rate::DisplayRateStats) -> agent_proto::DisplayRateStats {
    let f = s.frames;
    agent_proto::DisplayRateStats {
        rate_hz: f.rate_hz,
        shown_hz: f.shown_hz,
        capped: f.capped,
        swaps: f.swaps,
        blended: f.blended,
        swap_blends: f.swap_blends,
        between_newest: f.between_newest,
        failed: f.failed,
        resets: f.resets,
        avg_cost_us: f.avg_cost_us,
        marks_moved: s.marks_moved,
        marks_held: s.marks_held,
    }
}

/// The frame-stats window's per-frame means of the render counters
/// (`profile.counters` has the rest).
fn counters() -> Option<Value> {
    timeline::phase::window_summary().as_ref().map(render_counters)
}

fn render_counters(summary: &timeline::phase::Summary) -> Value {
    let mut counters = serde_json::Map::new();
    counters.insert("line".into(), json!(summary.short_line()));
    for (index, label) in timeline::phase::counter_columns() {
        let mean = summary.count_mean[index];
        if label == "shadow_walk_layers"
            || label.starts_with("native_ui_overlay_")
            || (label.starts_with("native_scene_lists_")
                && (mean != 0.0 || label.ends_with("_native") || label.ends_with("_fallback"))) {
            counters.insert(label.into(), json!(mean));
        }
    }
    Value::Object(counters)
}

/// The UI's layout now (`render.stats`'s `ui`): the window, the menu's
/// fit, how the HUD's corners anchor and the glyph pages' density.
fn ui_stats() -> Option<agent_proto::UiStats> {
    let report = crate::render_aspect::layout_report()?;
    let (atlas_scale, atlas_page) = crate::hud_overlay::atlas_state();
    Some(ui_stats_of(report, atlas_scale, atlas_page))
}

/// The report's wire shape ([`agent_proto::UiStats`]).
fn ui_stats_of(
    ((w, h), [sx, sy, ox, oy], pinned): ((u32, u32), [f32; 4], bool),
    atlas_scale: u32,
    atlas_page: u32,
) -> agent_proto::UiStats {
    agent_proto::UiStats {
        output: [w, h],
        scale: [sx, sy],
        offset: [ox, oy],
        anchor: if pinned { "pinned".into() } else { "centered".into() },
        atlas_scale,
        atlas_page,
    }
}

pub fn sources() -> agent::render::RenderSources {
    agent::render::RenderSources {
        #[cfg(feature = "gpu")]
        display_rate: Box::new(display_rate),
        #[cfg(not(feature = "gpu"))]
        display_rate: Box::new(|| None),
        ui: Box::new(ui_stats),
        counters: Box::new(counters),
    }
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;

    #[test]
    fn the_ui_report_is_the_wire_shape() {
        // 16:9: the safe area's fit with the HUD's corners pinned, the
        // pages rasterised at the display's density.
        let v = ui_stats_of(((1920, 1080), [0.75, 1.0, 80.0, 0.0], true), 3, 2048);
        assert_eq!(v.output, [1920, 1080]);
        assert_eq!((v.scale, v.offset), ([0.75, 1.0], [80.0, 0.0]));
        assert_eq!(v.anchor, "pinned");
        assert_eq!((v.atlas_scale, v.atlas_page), (3, 2048));
        // A 4:3 window: identity, centered corners, no atlas yet.
        let v = ui_stats_of(((640, 480), [1.0, 1.0, 0.0, 0.0], false), 0, 0);
        assert_eq!(v.scale, [1.0, 1.0]);
        assert_eq!(v.anchor, "centered");
        assert_eq!((v.atlas_scale, v.atlas_page), (0, 0));
    }

    #[test]
    fn render_stats_exposes_shadow_stage_means_and_refusal_reasons() {
        use timeline::phase::{FrameSample, Summary, counter_columns};
        let mut first = FrameSample::default();
        for (index, name) in counter_columns() {
            first.counts[index] = match name {
                "native_scene_lists_object_shadow_caster_native" => 4,
                "native_scene_lists_object_shadow_caster_fallback" |
                "native_scene_lists_object_shadow_caster_residency" => 1,
                "native_ui_overlay_items" => 6,
                "native_ui_overlay_batches" => 2,
                _ => 0,
            };
        }
        let frames = [first, FrameSample::default()];
        let counters = render_counters(&Summary::of(frames.iter()).unwrap());
        assert_eq!(counters["native_scene_lists_object_shadow_caster_native"], 2.0);
        assert_eq!(counters["native_scene_lists_object_shadow_caster_fallback"], 0.5);
        assert_eq!(counters["native_scene_lists_object_shadow_caster_residency"], 0.5);
        assert_eq!(counters["native_scene_lists_object_shadow_list_fallback"], 0.0);
        assert_eq!(counters["shadow_walk_layers"], 0.0);
        assert_eq!(counters["native_ui_overlay_items"], 3.0);
        assert_eq!(counters["native_ui_overlay_batches"], 1.0);
        assert_eq!(counters["native_ui_overlay_refused_texture"], 0.0, "every overlay counter, zero or not");
        assert!(counters.get("native_ui_items").is_none(), "the guest-draw UI counters stay in profile.counters");
    }

    #[test]
    fn the_display_rate_reaches_the_wire_field_for_field() {
        use frame_scene::display::DisplayStats;
        use native_render::frame_graph::display_rate::DisplayRateStats;
        let v = display_rate_of(DisplayRateStats {
            frames: DisplayStats { rate_hz: 120, shown_hz: 60, capped: true, swaps: 9, blended: 7, swap_blends: 3, between_newest: 2, failed: 1, resets: 4, avg_cost_us: 850 },
            marks_moved: 11,
            marks_held: 2,
        });
        let v = serde_json::to_value(v).unwrap();
        assert_eq!(v["rate_hz"], 120);
        assert_eq!(v["shown_hz"], 60);
        assert_eq!(v["capped"], true);
        assert_eq!((&v["swaps"], &v["blended"], &v["swap_blends"]), (&json!(9), &json!(7), &json!(3)));
        assert_eq!((&v["between_newest"], &v["failed"], &v["resets"]), (&json!(2), &json!(1), &json!(4)));
        assert_eq!(v["avg_cost_us"], 850);
        assert_eq!((&v["marks_moved"], &v["marks_held"]), (&json!(11), &json!(2)));
    }
}
