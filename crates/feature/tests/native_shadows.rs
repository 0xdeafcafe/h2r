use engine::points;
use feature::{
    Registry, Switches, native_lights::NativeLights, native_shadows::NativeShadows, plan::Base,
};

#[test]
fn shadows_are_opt_in_and_share_a_typed_sorted_hook_with_lights() {
    let mut off = Registry::new(Switches::defaults());
    off.add(&NativeShadows).unwrap();
    assert!(off.build(&Base).unwrap().chains.is_empty());

    let switches =
        Switches::parse("impl.render.native_shadows=on,impl.render.native_lights=on").unwrap();
    let mut r = Registry::new(switches);
    r.add(&NativeLights).unwrap();
    r.add(&NativeShadows).unwrap();
    let plan = r.build(&Base).unwrap();
    for point in [
        &points::LAYER_DRAW,
        &points::LIGHT_GEOMETRY_DRAW,
        &points::LAYER_END,
    ] {
        let chain = plan
            .chains
            .values()
            .find(|c| c.point.base == point.base)
            .unwrap();
        assert!(chain.render_thread().is_ok(), "{}", point.name);
    }
    assert_eq!(
        points::LIGHT_GEOMETRY_DRAW.sig.arg("entry"),
        Some(points::Loc::Stack(6))
    );
    assert!(matches!(
        points::LIGHT_GEOMETRY_DRAW.sig,
        points::Sig::Fn { pop: 28, .. }
    ));
    assert_ne!(engine::render_loop::layer::DRAWN_LAYERS & (1 << 14), 0);
}
