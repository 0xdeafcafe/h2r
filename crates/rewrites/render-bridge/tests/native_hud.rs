//! The `native-hud` feature (docs/graphics/native-post-hud.md, stage 5):
//! its plan (context-only hooks).
#![cfg(feature = "native-render")]

use render_bridge::native_hud::{self, NativeHud};
use hooks::plan::Base;
use hooks::{Registry, Switches};

fn plan(switches: &str) -> hooks::plan::Plan {
    let mut registry = Registry::new(Switches::parse(switches).unwrap());
    registry.add(&NativeHud).unwrap();
    registry.build(&Base).unwrap()
}

#[test]
fn the_native_ui_defaults_on() {
    for (spec, expected) in [("", true), ("impl.render=off", true)] {
        assert_eq!(native_hud::on(&Switches::parse(spec).unwrap()), expected);
    }
}

/// The Rust HUD material is on with the native UI; its own switch turns
/// only its setup hook off.
#[test]
fn the_rust_hud_defaults_on_and_its_switch_only_turns_it_off() {
    for (spec, expected) in [
        ("", true),
        ("impl.render.native-hud=off", false),
        ("impl.render=off", true),
    ] {
        assert_eq!(native_hud::rust_hud_on(&Switches::parse(spec).unwrap()), expected, "{spec}");
    }
    let placed = |spec| plan(spec).chains.values().any(|c| c.point.name == "HUD_QUAD_SETUP");
    assert!(placed(""));
    assert!(!placed("impl.render.native-hud=off"));
    assert!(plan("impl.render.native-hud=off").chains.values().any(|c| c.point.name == "HUD_QUAD_READY"));
}

/// The string draw in Rust is the walk's: placed with it, off with it.
#[test]
fn the_string_draw_is_placed_with_the_hud_walk() {
    let placed = |spec| plan(spec).chains.values().any(|c| c.point.name == "STRING_DRAW");
    assert!(placed(""));
    assert!(!placed("impl.render.native-hud-walk=off"));
    assert!(!placed("impl.render.native-hud=off"));
}

#[test]
fn the_ui_hooks_can_run_on_the_render_thread() {
    let plan = plan("");
    let points = [
        "UI_RENDER_LAYERS", "UI_ITEM_CALL", "UI_ITEM_RETURN", "UI_SCREEN_QUAD_READY",
        "UI_LOADING_SPINNER_READY", "UI_LOADING_BITMAP_READY",
        "UI_LOADING_RETURN", "UI_LOADING_EMPTY_RETURN", "UI_FADE_FAN_READY", "HUD_QUAD_READY", "HUD_QUAD_SETUP",
        "UI_LIST_READY", "UI_LIST_DONE",
    ];
    for point in points {
        let chain = plan.chains.values().find(|c| c.point.name == point)
            .unwrap_or_else(|| panic!("placed at {point}"));
        assert!(!chain.needs_shadow(), "{point}: nothing after the body");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
}

#[test]
fn ui_shares_the_loading_and_effect_points_with_other_features() {
    use engine::points::ui_render::LOADING_DRAW;
    use render_bridge::native_particles::{DYNAMIC_QUAD_READY, NativeParticles};
    use render_bridge::render_timing::RenderTiming;

    let mut registry = Registry::new(Switches::parse("render-timing").unwrap());
    registry.add(&NativeHud).unwrap();
    registry.add(&NativeParticles).unwrap();
    registry.add(&RenderTiming::new()).unwrap();
    let plan = registry.build(&Base).unwrap();
    assert!(std::ptr::eq(plan.chains[&LOADING_DRAW.base].point, &LOADING_DRAW));
    assert!(std::ptr::eq(
        plan.chains[&DYNAMIC_QUAD_READY.base].point,
        &DYNAMIC_QUAD_READY,
    ));
    let text = plan.text();
    assert!(text.contains("native_ui_inline_rect"));
    assert!(text.contains("native_effects_walk_quad"));
}
