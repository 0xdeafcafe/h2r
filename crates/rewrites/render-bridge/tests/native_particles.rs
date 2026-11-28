#![cfg(feature = "native-render")]
use render_bridge::native_particles::{NativeParticles, DYNAMIC_QUAD_READY, EMIT_READY};
use hooks::plan::Base;
use hooks::{Registry, Switches};

#[test]
fn no_parent_update_skip() {
    use render_bridge::native_particle_walk::WALK;
    let mut r = Registry::new(Switches::defaults());
    r.add(&NativeParticles).unwrap();
    let plan = r.build(&Base).unwrap();
    // The 31 particle and effect chains, the effect materials' three
    // (particle material, particle setup, contrail material) and the walk.
    use render_bridge::effect_materials::{CONTRAIL_MATERIAL, PARTICLE_MATERIAL, PARTICLE_SETUP};
    for point in [PARTICLE_MATERIAL, PARTICLE_SETUP, CONTRAIL_MATERIAL] {
        assert!(plan.chains.contains_key(&point.base), "missing {}", point.name);
    }
    assert_eq!(plan.chains.len(), 31 + 3 + 1);
    let walk = plan.chains.get(&WALK.base).expect("the walk's chain");
    assert_eq!(walk.point.base, engine::particle_walk::ENTRY);
    use guest_layout::points::scene_effects::{EFFECT_LAYERED_A, EFFECT_LAYERED_B, EFFECT_QUERY};
    use guest_layout::points::render_aspect::FLARE_QUERY_END;
    for point in [EFFECT_LAYERED_A, EFFECT_LAYERED_B] {
        assert!(!plan.chains.contains_key(&point.base), "layered geometry belongs to transparents");
    }
    for point in [EFFECT_QUERY, FLARE_QUERY_END] {
        assert!(plan.chains.contains_key(&point.base), "missing query boundary at {}", point.name);
    }
    assert!(plan.chains.contains_key(&EMIT_READY.base));
    assert!(!plan.chains.contains_key(&native_render::particles::EMIT_QUAD), "the walk replaces the emit observer");
    assert!(plan.chains.contains_key(&DYNAMIC_QUAD_READY.base));
    assert!(plan
        .chains
        .contains_key(&native_render::effect_geometry::DYNAMIC_DRAW));
    let scene = guest_layout::points::scene_effects::instruction_entries().collect::<Vec<_>>();
    for chain in plan.chains.values().filter(|c| c.point.name.starts_with("EFFECT_IMMEDIATE_")) {
        assert!(scene.contains(&chain.point.base));
    }
    assert_eq!(plan.chains.values().filter(|c| c.point.name.starts_with("EFFECT_IMMEDIATE_")).count(), 10);
    let flare_points: Vec<_> = plan.chains.values().filter(|c| c.point.name.starts_with("FLARE_DRAW_")).collect();
    assert_eq!(flare_points.len(), 10);
    assert!(flare_points.iter().all(|c| scene.contains(&c.point.base)));
    for chain in plan.chains.values() {
        assert_ne!(chain.point.base, native_render::particles::RENDER_UPDATE);
        assert!(chain.render_thread().is_ok());
        assert!(!chain.needs_shadow());
    }
}
