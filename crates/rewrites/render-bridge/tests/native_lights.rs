//! Context-only admission inside the original light layers.
#![cfg(feature = "native-render")]
use hooks::plan::Base;
use hooks::{Registry, Switches};
use render_bridge::native_lights::{self, NativeLights};

#[test]
fn its_chains_run_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    assert!(r.add(&NativeLights).unwrap());
    let plan = r.build(&Base).unwrap();
    use guest_layout::points::{LIGHT_GEOMETRY_DRAW, Loc, Sig};
    let Sig::Fn { args, ret, pop } = LIGHT_GEOMETRY_DRAW.sig else {
        panic!("geometry admission requires a returnable handler");
    };
    assert_eq!(
        args.iter().map(|&(_, loc)| loc).collect::<Vec<_>>(),
        (0..7).map(Loc::Stack).collect::<Vec<_>>()
    );
    assert_eq!(u32::from(pop), engine::render_loop::layer::HANDLER_CLEANUP);
    assert_eq!(ret, None);
    let text = plan.text();
    assert!(text.contains("native_lights_geometry"), "{text}");
    assert!(text.contains("native_lights_view"), "{text}");
    // The light frame body is the implementation at RENDER_DYNAMIC_LIGHTS.
    assert!(
        text.contains("implement native-lights:native_lights_frame"),
        "{text}"
    );
    // And the light's item lists' at SCENE_ITEMS (slice L4).
    assert!(
        text.contains("implement native-lights:native_lights_items"),
        "{text}"
    );
    // And one light's passes' at RENDER_LIGHT_PASSES (slice L5).
    assert!(
        text.contains("implement native-lights:native_lights_passes"),
        "{text}"
    );
    // And the first-person lights' and their slots' records' (slice L6).
    assert!(
        text.contains("implement native-lights:native_lights_first_person"),
        "{text}"
    );
    assert!(
        text.contains("implement native-lights:native_lights_first_person_records"),
        "{text}"
    );
    for body in ["object_shadow_list", "object_shadow_rows", "object_shadow_caster"] {
        assert!(text.contains(&format!("implement native-lights:native_lights_{body}")), "{text}");
    }
    assert!(
        text.contains("implement native-lights:native_lights_additional_casters"),
        "{text}"
    );
    for point in [
        "ADDITIONAL_SHADOW_CASTERS",
        "OBJECT_SHADOW_LIST",
        "GATHER_OBJECT_SHADOWS",
        "OBJECT_SHADOW",
        "FIRST_PERSON_LIGHTS",
        "FIRST_PERSON_LIGHT_RECORDS",
        "RENDER_LIGHT_PASSES",
        "LIGHT_CONSTANTS",
        "RENDER_DYNAMIC_LIGHTS",
        "SCENE_ITEMS",
        "LAYER_DRAW",
        "LIGHT_GEOMETRY_DRAW",
        "LAYER_END",
    ] {
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
fn render_timing_on_places_nothing() {
    let mut r = Registry::new(Switches::parse("render-timing").unwrap());
    r.add(&NativeLights).unwrap();
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(!text.contains("native_lights"), "{text}");
    // Nothing placed, nothing held: no snapshot blocker.
    assert!(plan.snapshot_blockers().is_empty());
}

/// Placed, its section (no bytes) forgets the open run and the pending
/// light at a load.
#[test]
fn placed_its_snapshot_policy_is_a_section() {
    let mut r = Registry::new(Switches::parse("").unwrap());
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

#[test]
fn light_walk_follows_the_master_switch_with_context_only_chains() {
    use native_lights::walk::{self, NativeLightWalk};
    for (spec, enabled) in [
        ("", true),
        ("impl.render=off", true),
    ] {
        let switches = Switches::parse(spec).unwrap();
        assert_eq!(walk::on(&switches), enabled);
        let mut r = Registry::new(switches);
        r.add(&NativeLightWalk).unwrap();
        let plan = r.build(&Base).unwrap();
        assert_eq!(
            plan.text().contains("walk gathered light geometry"),
            enabled
        );
        for chain in plan.chains.values() {
            assert!(chain.render_thread().is_ok());
        }
    }
}

#[test]
fn the_light_layers_draw_in_slots_4_and_10() {
    assert_eq!(native_lights::LIGHT_SLOT.number(), Some(4));
    assert_eq!(native_lights::FIRST_PERSON_SLOT.number(), Some(10));
    assert_eq!(native_lights::SCENE_PASSES, [10, 11, 14, 12, 13]);
    // The light frame is a cut producer: its guest body never runs.
    assert!(render_bridge::cut::CUTS.iter().any(|c| c.name == "dynamic_lights" && c.entry == 0x0003_1c80));
}
