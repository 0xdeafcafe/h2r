//! Context-only admission inside the original light layers.
use feature::native_lights::{self, NativeLights};
use feature::plan::Base;
use feature::{Registry, Switches};

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=off").unwrap());
    r.add(&NativeLights).unwrap();
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(!text.contains("native_lights"), "{text}");
    // Nothing placed, nothing held: no snapshot blocker.
    assert!(plan.snapshot_blockers().is_empty());
}

#[test]
fn its_chains_run_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on").unwrap());
    assert!(r.add(&NativeLights).unwrap());
    let plan = r.build(&Base).unwrap();
    use engine::points::{LIGHT_GEOMETRY_DRAW, Loc, Sig};
    let Sig::Fn { args, ret, pop } = LIGHT_GEOMETRY_DRAW.sig else {
        panic!("geometry admission requires a returnable handler");
    };
    assert_eq!(args.iter().map(|&(_, loc)| loc).collect::<Vec<_>>(), (0..7).map(Loc::Stack).collect::<Vec<_>>());
    assert_eq!(u32::from(pop), engine::render_loop::layer::HANDLER_CLEANUP);
    assert_eq!(ret, None);
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
    let mut r = Registry::new(
        Switches::parse("impl.render.native_lights=off,impl.render.native_lights_split=on").unwrap(),
    );
    r.add(&NativeLights).unwrap();
    assert!(!r.build(&Base).unwrap().text().contains("native_lights"));
}

#[test]
fn render_timing_on_places_nothing() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on,render-timing").unwrap());
    r.add(&NativeLights).unwrap();
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(!text.contains("native_lights"), "{text}");
    // Nothing placed, nothing held: no snapshot blocker.
    assert!(plan.snapshot_blockers().is_empty());
}

/// Placed, its section (no bytes) forgets the open run and the split
/// light at a load.
#[test]
fn placed_its_snapshot_policy_is_a_section() {
    let mut r = Registry::new(Switches::parse("impl.render.native_lights=on").unwrap());
    assert!(r.add(&NativeLights).unwrap());
    let plan = r.build(&Base).unwrap();
    let policies: Vec<_> = plan
        .snapshot
        .iter()
        .map(|f| (f.id, f.policy.name()))
        .collect();
    assert_eq!(policies, vec![(native_lights::ID, "section")]);
    assert!(plan.snapshot_blockers().is_empty());
}
