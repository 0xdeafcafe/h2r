use feature::native_particles::{compare_budget, parse_compare, NativeParticles, DYNAMIC_QUAD_READY, EMIT_READY};
use feature::plan::Base;
use feature::{Registry, Switches};

#[test]
fn the_switch_off_and_no_parent_update_skip() {
    let mut r = Registry::new(Switches::parse("impl.render.native_particles=off").unwrap());
    r.add(&NativeParticles).unwrap();
    assert!(!r.build(&Base).unwrap().text().contains("native_particles"));
    let mut r = Registry::new(Switches::parse("impl.render.native_particles=on").unwrap());
    r.add(&NativeParticles).unwrap();
    let plan = r.build(&Base).unwrap();
    assert_eq!(plan.chains.len(), 21 + usize::from(compare_budget().is_none()));
    let emit = if compare_budget().is_some() {
        native_render::particles::EMIT_QUAD
    } else {
        EMIT_READY.base
    };
    assert!(plan.chains.contains_key(&emit));
    assert_eq!(plan.chains.contains_key(&DYNAMIC_QUAD_READY.base), compare_budget().is_none());
    assert!(plan
        .chains
        .contains_key(&native_render::effect_geometry::DYNAMIC_DRAW));
    for site in native_render::effect_immediate::Site::ALL {
        let (begin, end) = site.boundaries();
        assert!(plan.chains.contains_key(&begin));
        assert!(plan.chains.contains_key(&end));
    }
    for chain in plan.chains.values() {
        assert_ne!(chain.point.base, native_render::particles::RENDER_UPDATE);
        assert!(chain.render_thread().is_ok());
        assert!(!chain.needs_shadow());
    }
}
#[test]
fn compare_budget_is_explicit_and_bounded() {
    assert_eq!(parse_compare(""), Some(8));
    assert_eq!(parse_compare("16"), Some(16));
    assert_eq!(parse_compare("0"), None);
    assert_eq!(parse_compare("-1"), None);
    assert_eq!(parse_compare("on"), None);
}
