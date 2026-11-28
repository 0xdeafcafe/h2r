//! The native renderer's hook points are function entries the recompiler
//! discovers in the retail XBE, so a hook can be placed on each. The
//! transparents' geometry handler (`points::TRANSPARENT_GEOMETRY`) is only
//! reached through the entry's function pointer (stored by `mov [esi+0x20],
//! imm32` at `0x000413ff`), so it rests on discovery following code
//! immediates (docs/graphics/native-transparents.md, "Stage 3: transparents").
//! Addresses only; no bytes are kept.
//!
//! `H2_XBE=/path/default.xbe cargo test -p boot --test render_points_xbe -- --ignored`

use guest_layout::points;

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_native_renderers_points_are_discovered_functions() {
    let data =
        std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe"))
            .unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    let analysis = analyze::analyze(&xbe).unwrap();
    let entries = analyze::dispatch::entries(&xbe, &analysis);
    for p in [
        &points::LAYER_DRAW,
        &points::DECORATOR_GRID,
        &points::SCENE_TRANSPARENTS,
        &points::VIEW_WINDOW_BUILD,
        &points::TRANSPARENT_GEOMETRY,
        &points::PORTAL_WINDOW_MERGE,
        &points::PORTAL_WINDOW_REGIONS,
        &points::SCENE_CLUSTER_GATHER,
        &points::PORTAL_WINDOW_WALK,
        &points::SCENE_GEOMETRY_GATHER,
        &points::VIEW_PVS_REGIONS,
        &points::SCENE_OBJECT_ORDER,
        &points::VIEW_SPHERE_REGIONS,
        &points::VISIBILITY_SETUP,
        &points::VIEW_FRUSTA_REGIONS,
        &points::VIEW_PROJECTION_BUILD,
        &points::VISIBLE_LIGHT_LIST,
        &points::LIGHT_GEOMETRY_FLAGS,
        &points::LIGHT_EXTRA_CASTER,
        &points::RESET_SCENE_RECORDS,
        &points::OBJECT_RENDER_RECORDS,
        &points::MODEL_LOD,
        &points::SCENE_RECORD_WRITER,
        &points::TRANSPARENT_EFFECT_ENTRY,
        &points::TRANSPARENT_POLYGON_ENTRY,
        &points::OBJECT_RENDER_SETUP,
        &points::RENDER_DYNAMIC_LIGHTS,
        &points::RENDER_DECAL_MODE,
        &points::RENDER_LIGHT_PASSES,
        &points::SCENE_ITEMS,
        &points::FIRST_PERSON_LIGHTS,
        &points::FIRST_PERSON_LIGHT_RECORDS,
        &points::LIGHT_GEOMETRY_DRAW,
        &points::LIGHT_RESTORE_TARGET,
        &points::LAYER_BUILD,
        &points::SCREEN_EFFECT,
        &points::TEMPLATE_PASS,
        &points::TEMPLATE_QUAD,
        &points::HUD_BLACK_OVERLAY,
    ] {
        assert!(
            analysis.functions.contains_key(&p.base),
            "{} ({:#010x}) is not a discovered function",
            p.name,
            p.base
        );
        assert!(
            entries.contains(&p.base),
            "{} ({:#010x}) is not an entry",
            p.name,
            p.base
        );
    }
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn the_native_scene_list_sites_are_instruction_boundaries() {
    let data =
        std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE to a retail default.xbe"))
            .unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    let mut analysis = analyze::analyze(&xbe).unwrap();
    let sites = [
        points::scene_effects::LAYER_BUILT.base,
        points::scene_effects::LAYER_ENTRY_DONE.base,
        points::OBJECT_RECORD_APPEND.base,
        points::TRANSPARENT_GEOMETRY_ENTRY.base,
        points::TRANSPARENT_GEOMETRY_QUEUED.base,
        points::TRANSPARENT_ORDER_BEGIN.base,
        points::TRANSPARENT_ORDER_READY.base,
        points::scene_effects::TRANSPARENT_DISPATCH_NEXT.base,
    ];
    for site in sites.into_iter().chain([points::OBJECT_RECORD_APPENDED]) {
        assert!(
            matches!(analysis.claim(site), Some(('I', Some(_)))),
            "{site:#010x}"
        );
    }
    analysis.expose_instructions(points::scene_effects::instruction_entries());
    for site in sites {
        assert!(analysis.functions.contains_key(&site), "{site:#010x}");
    }
}

#[test]
#[ignore = "needs the retail default.xbe in $H2_XBE"]
fn render_aspect_sites_are_dispatch_blocks_in_the_build_tiers_analysis() {
    let data = std::fs::read(std::env::var_os("H2_XBE").expect("set H2_XBE")).unwrap();
    let xbe = xbe::Xbe::parse(&data).unwrap();
    assert!(recomp::has_retail_render_sites(&xbe));
    boot::register_image_entries();
    let (analysis, _) =
        recomp::analyze_cached(&xbe, recomp::analysis_cache(&data).as_deref()).unwrap();
    let builder = analyze::cfg::CfgBuilder::new(&analysis, &xbe);
    for pc in boot::image_entries() {
        assert!(
            analysis.functions.contains_key(&pc),
            "missing render hook entry {pc:#010x}"
        );
        let cfg = builder.build(pc).unwrap();
        assert!(
            cfg.blocks.contains_key(&pc),
            "no block for render hook {pc:#010x}"
        );
    }
}
