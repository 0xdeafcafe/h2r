#![cfg(feature = "native-render")]
use std::sync::{Arc, Mutex};

use engine::{memory::GuestMemory, points};
use frame_scene::{GroupSite, LightGroup, ShadowPass, ViewSlot};
use hooks::{Registry, Switches, plan::Base};
use native_render::shadows::Site;
use render_bridge::{
    cut::{self, CUTS},
    native_lights::{NativeLights, SceneGroup},
    native_shadows::{self, Entry, Host, Layer, NativeShadows},
    producers::{self, Status},
};

/// The host is one global: tests that set it take turns.
static HOST_TURN: Mutex<()> = Mutex::new(());

/// A host that draws every group at the size it is given, and remembers it.
struct Scaled(Mutex<Vec<(u32, [u32; 2])>>);
impl Host for Scaled {
    fn parent(&self, _: Option<Site>, _: Option<[u32; 2]>) {}
    fn entry(&self, _: &mut dyn GuestMemory, _: Option<Entry>) {}
    fn begin(&self, _: &mut dyn GuestMemory, _: Layer) {}
    fn end(&self, _: &mut dyn GuestMemory, _: Site, _: u32) {}
    fn draw_alone(
        &self,
        _: &mut dyn GuestMemory,
        view: u32,
        group: &SceneGroup,
    ) -> Result<(), &'static str> {
        let size = group.group.pass.map_or([0, 0], |p| p.size);
        self.0.lock().unwrap().push((view, size));
        Ok(())
    }
}

fn group(layer: u8, size: [u32; 2]) -> SceneGroup {
    SceneGroup {
        group: LightGroup {
            light: 0,
            layer,
            site: GroupSite::ObjectShadow,
            projection: None,
            pass: Some(ShadowPass {
                target: 1,
                size,
                flags: 0,
                clip_control: 0,
                raster: 0,
                offset_enable: 0,
                offset_constant: 0,
                offset_slope: 0,
                stencil_mode: 0,
                shadow_function: 0,
            }),
            entries: 0..0,
            remainder: 0,
        },
        entries: Vec::new(),
        rows: Vec::new(),
    }
}

#[test]
fn a_scaled_view_draws_its_shadow_layers() {
    let _turn = HOST_TURN.lock().unwrap_or_else(|e| e.into_inner());
    let mut mem = engine::FlatMemory::new(0, 0x510000);
    let host = Arc::new(Scaled(Mutex::default()));
    native_shadows::set_host(host.clone());
    // Render scale 1 and 2: the size is the host's business, not a refusal.
    for (layer, size) in [(10u8, [256, 256]), (11, [2560, 1440]), (14, [2560, 1440])] {
        let before = native_shadows::drawn_alone(u32::from(layer));
        assert!(native_shadows::draw_alone(&mut mem, u32::from(layer), &group(layer, size)));
        assert_eq!(native_shadows::drawn_alone(u32::from(layer)), before + 1, "layer {layer}");
    }
    assert_eq!(host.0.lock().unwrap().iter().filter(|(_, s)| s[0] == 2560).count(), 2);
    // A host that cannot draw a group drops it under its cause: the guest's
    // handlers never run, and no refusal counter exists.
    struct Cannot;
    impl Host for Cannot {
        fn parent(&self, _: Option<Site>, _: Option<[u32; 2]>) {}
        fn entry(&self, _: &mut dyn GuestMemory, _: Option<Entry>) {}
        fn begin(&self, _: &mut dyn GuestMemory, _: Layer) {}
        fn end(&self, _: &mut dyn GuestMemory, _: Site, _: u32) {}
        fn draw_alone(&self, _: &mut dyn GuestMemory, _: u32, _: &SceneGroup) -> Result<(), &'static str> {
            Err("target")
        }
    }
    native_shadows::set_host(Arc::new(Cannot));
    assert!(native_shadows::draw_alone(&mut mem, 11, &group(11, [2560, 1440])));
    let rows = native_shadows::alone_counters();
    assert!(rows.iter().any(|(n, c)| *n == "scene_dropped_shadow_layers_target" && *c > 0));
    assert!(rows.iter().all(|(n, _)| !n.contains("_refused_")));
    assert!(native_shadows::filter_counters().iter().all(|(n, _)| !n.contains("_refused_")));
    native_shadows::clear_host();
}

#[test]
fn object_shadows_draw_before_layer_7() {
    // The list is cut whole, and the census says so.
    let cut = CUTS.iter().find(|c| c.name == "object_shadows").unwrap();
    assert_eq!((cut.entry, cut.pop), (0x0003_35e0, 4));
    assert!(cut::OWN_PRODUCER.contains(&"object_shadows"));
    cut::check(CUTS).unwrap();
    let row = &producers::REGISTRY[producers::row("object_shadows").unwrap()];
    assert_eq!(row.status, Status::Native);
    // Slot 3 holds the shadows, then layer 7; the lights follow in slot 4.
    assert_eq!(native_shadows::SLOT, ViewSlot::ShadowsAndLayer7);
    assert_eq!(native_shadows::SLOT.number(), Some(3));
    assert!(ViewSlot::DecalsMode0 < native_shadows::SLOT);
    assert!(native_shadows::SLOT < ViewSlot::LightAccumulation);
}

#[test]
fn on_shares_a_typed_sorted_hook_with_lights() {
    let switches = Switches::parse("").unwrap();
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
