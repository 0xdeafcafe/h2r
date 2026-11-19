//! Context-only admission inside the original light layers.
use feature::native_lights::{self, NativeLights};
use feature::plan::Base;
use feature::{Registry, Switches};

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativeLights).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_lights"), "{text}");
}

#[test]
fn its_chains_run_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on").unwrap());
    assert!(r.add(&NativeLights).unwrap());
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(text.contains("native_lights_geometry"), "{text}");
    assert!(text.contains("native_lights_view"), "{text}");
    for point in ["RENDER_LIGHT_PASSES", "RENDER_DYNAMIC_LIGHTS", "LAYER_DRAW", "LIGHT_GEOMETRY_DRAW", "LAYER_END"] {
        let chain = plan
            .chains
            .values()
            .find(|c| c.point.name == point)
            .unwrap_or_else(|| panic!("placed at {point}"));
        assert!(!chain.needs_shadow(), "{point}: nothing after the body");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
}

#[test]
fn splitting_requires_both_switches() {
    assert!(!native_lights::split_on(&Switches::parse("impl.render.native_lights=on").unwrap()));
    assert!(native_lights::split_on(&Switches::parse("impl.render.native_lights=on,impl.render.native_lights_split=on").unwrap()));
    // Without the lights, nothing splits.
    let mut r = Registry::new(Switches::parse("impl.render.native_lights_split=on").unwrap());
    r.add(&NativeLights).unwrap();
    assert!(!r.build(&Base).unwrap().text().contains("native_lights"));
}

#[test]
fn render_timing_on_places_nothing() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on,render-timing").unwrap());
    r.add(&NativeLights).unwrap();
    let text = r.build(&Base).unwrap().text();
    assert!(!text.contains("native_lights"), "{text}");
}
