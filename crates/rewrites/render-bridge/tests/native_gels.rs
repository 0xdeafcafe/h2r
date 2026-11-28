//! Synthetic admission/placement tests; no retail bytes or game process.
#![cfg(feature = "native-render")]
use engine::memory::{FlatMemory, GuestMemory};
use hooks::{Registry, Switches, plan::Base};
use render_bridge::{native_gels::{self, Call, NativeGels}};
use native_render::gels;

#[test]
fn the_switch_places_gels_and_hooks_are_render_context_only() {
    let plan = |switches: &str| {
        let mut registry = Registry::new(Switches::parse(switches).unwrap());
        registry.add(&NativeGels).unwrap();
        registry.build(&Base).unwrap()
    };
    let on = plan("");
    assert!(on.text().contains("native_gels_quad_end"));
    let find = |name: &str| {
        on.chains
            .values()
            .find(|c| c.point.name == name)
            .expect("gel hook")
    };
    // The template hook is a plain before hook: it runs on the render thread.
    let template = find("TEMPLATE_PASS");
    assert!(!template.needs_shadow());
    assert!(template.render_thread().is_ok());
    // The quad helper has an exit hook, so its chain runs after the body
    // through the shadow, and a chain like that stays on the game thread.
    let quad = find("TEMPLATE_QUAD");
    assert!(quad.needs_shadow());
    assert!(quad.render_thread().is_err());
}

#[test]
fn only_the_original_gel_call_for_the_current_definition_is_admitted() {
    let mut mem = FlatMemory::new(0x0040_0000, 0x0021_0000);
    let esp = 0x0060_0000;
    let definition = 0x0050_0000;
    let shader = 0x1234;
    mem.set_u32(esp, gels::TEMPLATE_RETURN).unwrap();
    for (n, arg) in [shader, 1, u32::MAX, 0, 13, 1].iter().enumerate() {
        mem.set_u32(esp + 4 + n as u32 * 4, *arg).unwrap();
    }
    mem.set_u32(gels::DEFINITION, definition).unwrap();
    mem.set_u32(definition + 0xe0, shader).unwrap();
    mem.set_u8(gels::BUILDING, 1).unwrap();
    assert_eq!(
        native_gels::producer_call(&mem, esp),
        Some(Call { shader, definition })
    );
    mem.set_u32(definition + 0xe0, shader + 1).unwrap();
    assert!(native_gels::producer_call(&mem, esp).is_none());
    mem.set_u32(definition + 0xe0, shader).unwrap();
    mem.set_u8(gels::BUILDING, 0).unwrap();
    assert!(native_gels::producer_call(&mem, esp).is_none());
    assert!(native_gels::producer_call(&mem, u32::MAX - 3).is_none());
}
