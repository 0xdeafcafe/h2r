use feature::native_particles::{NativeParticles, parse_compare};
use feature::plan::Base;
use feature::{Registry, Switches};

#[test]
fn default_off_and_no_parent_update_skip() {
    let mut r = Registry::new(Switches::parse("").unwrap());
    r.add(&NativeParticles).unwrap();
    assert!(!r.build(&Base).unwrap().text().contains("native_particles"));
    let mut r = Registry::new(Switches::parse("impl.render.native_particles=on").unwrap());
    r.add(&NativeParticles).unwrap();
    let plan = r.build(&Base).unwrap();
    assert_eq!(plan.chains.len(), 6);
    assert!(
        plan.chains
            .contains_key(&native_render::particles::EMIT_QUAD)
    );
    assert!(
        plan.chains
            .contains_key(&native_render::effect_geometry::DYNAMIC_DRAW)
    );
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
