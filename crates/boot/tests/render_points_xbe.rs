//! The native renderer's hook points are function entries the recompiler
//! discovers in the retail XBE, so a hook can be placed on each. The
//! transparents' geometry handler (`points::TRANSPARENT_GEOMETRY`) is only
//! reached through the entry's function pointer (stored by `mov [esi+0x20],
//! imm32` at `0x000413ff`), so it rests on discovery following code
//! immediates (docs/graphics/native-renderer.md, "Stage 3: transparents").
//! Addresses only; no bytes are kept.
//!
//! `H2_XBE=/path/default.xbe cargo test -p boot --test render_points_xbe -- --ignored`

use engine::points;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_native_renderers_points_are_discovered_functions() {
    let data = std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe")).unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&xbe).unwrap();
    let entries = analyze::dispatch::entries(&xbe, &analysis);
    for p in [
        &points::LAYER_DRAW,
        &points::SCENE_TRANSPARENTS,
        &points::TRANSPARENT_GEOMETRY,
        &points::OBJECT_RENDER_SETUP,
        &points::RENDER_DYNAMIC_LIGHTS,
        &points::RENDER_LIGHT_PASSES,
        &points::SCREEN_EFFECT,
        &points::TEMPLATE_PASS,
    ] {
        assert!(
            analysis.functions.contains_key(&p.base),
            "{} ({:#010x}) is not a discovered function",
            p.name,
            p.base
        );
        assert!(entries.contains(&p.base), "{} ({:#010x}) is not an entry", p.name, p.base);
    }
}
