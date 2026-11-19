//! The `native-world` feature (docs/graphics/native-renderer.md): its plan
//! next to every other hook on the layer draw, its one observer on
//! synthetic guest memory (the BSP items the host draws lose layer 3's bit
//! for the layer and get it back at the next layer draw; nothing changes
//! without a host, in another layer, or for a part the host leaves to the
//! D3D path, even one of a shader it draws), and its chain on the
//! pipelined render thread.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_world::{self, Host, NativeWorld, PartSet};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS, STENCIL_MARK, STENCIL_OFF_FLAGS, StencilPlan};
use native_render::world::Owner;

#[test]
fn the_plan_orders_it_inside_render_timing_with_the_layer_ports() {
    let switches = "impl.render.native_world=on,render-timing=on,render-timing.deep=on,impl.render.layer_draw=on";
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(&feature::render_timing::RenderTiming::new()).unwrap());
    assert!(r.add(&feature::render_loop::RenderLoop).unwrap());
    assert!(r.add(&NativeWorld).unwrap());
    let plan = match r.build(&Base) {
        Ok(p) => p,
        Err(c) => panic!("refused: {c:?}"),
    };
    let text = plan.text();
    assert!(text.contains("native_world_skip"), "{text}");
    assert!(!text.contains("native_world_restore"), "nothing after the body: {text}");
}

/// Alone at the layer draw (switched on: it is off by default), the chain
/// runs on the pipelined render thread: one context-only observer before,
/// nothing after.
#[test]
fn its_layer_draw_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_world=on").unwrap());
    assert!(r.add(&NativeWorld).unwrap());
    let plan = r.build(&Base).unwrap();
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "LAYER_DRAW")
        .expect("placed at the layer draw");
    assert!(!chain.needs_shadow());
    assert!(chain.render_thread().is_ok());
}

/// Off, it holds nothing and never blocks a snapshot; on, its section
/// (no bytes) drops the pending restore at a load.
#[test]
fn its_snapshot_policy_is_stateless_off_and_a_section_on() {
    for (switches, policy) in [
        ("", "stateless"),
        ("impl.render.native_world=on", "section"),
    ] {
        let mut r = Registry::new(Switches::parse(switches).unwrap());
        assert!(r.add(&NativeWorld).unwrap());
        let plan = r.build(&Base).unwrap();
        let policies: Vec<_> = plan
            .snapshot
            .iter()
            .map(|f| (f.id, f.policy.name()))
            .collect();
        assert_eq!(policies, vec![(native_world::ID, policy)], "{switches:?}");
        assert!(plan.snapshot_blockers().is_empty(), "{switches:?}");
    }
}

#[test]
fn material_programs_are_on_with_the_world_and_need_it() {
    let on = |s: &str| native_world::programs_on(&Switches::parse(s).unwrap());
    assert!(native_world::PROGRAMS_DEFAULT);
    assert!(!on(""), "the world is off by default");
    assert!(on("impl.render.native_world=on"), "on with the world");
    assert!(!on("impl.render.native_world=on,impl.render.native_world_programs=off"), "stage 1 for the A/B");
    assert!(!on("impl.render.native_world=off,impl.render.native_world_programs=on"));
    assert!(on("impl.render.native_world=on,impl.render=off"), "looked up exactly, not through impl.render");
}

/// The plan text with the native world and a pipeline that is on.
fn plan_with_pipeline(switches: &str) -> String {
    use feature::pipeline_render::{Addrs, PipelineRender};
    let pipeline = PipelineRender::with(true, Addrs::HALO2).print(false);
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    r.add(&pipeline).unwrap();
    r.add(&NativeWorld).unwrap();
    r.build(&Base).unwrap().text()
}

#[test]
fn it_is_off_by_default_and_on_keeps_pipelining() {
    assert!(!native_world::DEFAULT);
    let text = plan_with_pipeline("");
    assert!(!text.contains("native_world"), "{text}");
    let text = plan_with_pipeline("impl.render.native_world=on");
    assert!(text.contains("native_world_skip"), "{text}");
    assert!(text.contains("pipelined frame"), "{text}");
}

#[test]
fn the_switch_off_places_nothing_and_keeps_pipelining() {
    let text = plan_with_pipeline("impl.render.native_world=off");
    assert!(!text.contains("native_world"), "{text}");
    assert!(text.contains("pipelined frame"), "{text}");
}

const LIST: u32 = 0x004c_1a48;
const ITEMS: u32 = 0x0060_0000;
const BSP: u32 = 0x0080_0000;

struct Stub {
    asked: Mutex<Vec<(u32, RenderCamera, StencilPlan)>>,
    parts: Arc<PartSet>,
}

impl Host for Stub {
    fn ask(&self, _mem: &dyn GuestMemory, bsp: u32, camera: &RenderCamera, stencil: &StencilPlan) -> Option<Arc<PartSet>> {
        self.asked.lock().unwrap().push((bsp, *camera, stencil.clone()));
        Some(Arc::clone(&self.parts))
    }
}

/// Layer 3 current; two BSP items (shaders 0x10 and 0x11, cluster 0's
/// parts 0 and 1) and an object item, all in layers 1 and 3; a usable
/// camera; the active BSP.
fn guest() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    m.set_u32(CURRENT_LIST, LIST).unwrap();
    m.set_u32(LIST + 4, 3).unwrap();
    m.set_u32(LIST + 0xc, 0).unwrap();
    m.set_u32(LIST + 0x14, ITEMS).unwrap();
    for (i, (shader, record, handle)) in [(0x10u32, 1u32, u32::MAX), (0x11, 2, u32::MAX), (0x10, 3, 0xe123_0001)]
        .into_iter()
        .enumerate()
    {
        let a = ITEMS + 0x18 * i as u32;
        m.set_u32(a, GEOMETRY_BUILD).unwrap();
        m.set_u32(a + 8, 0xe000_0000 | shader).unwrap();
        m.set_u32(a + 0xc, record << 8 | i as u32).unwrap();
        m.set_u32(a + 0x10, 0b1010).unwrap();
        m.set_u32(RENDER_RECORDS + 0x20 * record + 4, handle).unwrap();
    }
    let c = RENDER_CAMERA;
    for (i, v) in [0.0f32, 0.0, 1.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0].into_iter().enumerate() {
        m.set_u32(c + 4 * i as u32, v.to_bits()).unwrap();
    }
    m.set_u32(c + 0x28, 1.2f32.to_bits()).unwrap();
    m.set_u32(c + 0x34, 480 | (640 << 16)).unwrap();
    m.set_u32(c + 0x40, 0.0625f32.to_bits()).unwrap();
    m.set_u32(c + 0x44, 1024f32.to_bits()).unwrap();
    m.set_u32(native_world::ACTIVE_BSP, BSP).unwrap();
    m
}

fn masks(m: &FlatMemory) -> Vec<u32> {
    (0..3).map(|i| m.u32(ITEMS + 0x18 * i + 0x10).unwrap()).collect()
}

#[test]
fn the_world_items_skip_layer_3_and_get_it_back_at_the_next_layer_draw() {
    // No host yet: nothing changes.
    let mut m = guest();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    native_world::restore_pending(&mut m);

    let stub = Arc::new(Stub {
        asked: Mutex::default(),
        parts: Arc::new([((Owner::Cluster(0), 0), 0x10u16)].into_iter().collect()),
    });
    native_world::set_host(stub.clone());

    native_world::before_layer(&mut m);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b1010, 0b1010],
        "the drawn part's BSP item only: not the D3D-path part, not the object"
    );
    {
        let asked = stub.asked.lock().unwrap();
        assert_eq!(asked.len(), 1);
        assert_eq!(asked[0].0, BSP);
        assert_eq!(asked[0].1.far, 1024.0);
    }

    // The next layer draw (layer 5) sets the bit back first, and the host
    // is not asked for it.
    m.set_u32(CURRENT_LAYER, 5).unwrap();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    assert_eq!(stub.asked.lock().unwrap().len(), 1);

    // Nothing pending: another layer draw changes nothing.
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);

    // A snapshot load between a layer 3 and the next layer draw drops the
    // pending restore: the loaded RAM is not the memory it was made in.
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m)[0], 0b0010);
    native_world::forget_pending();
    native_world::restore_pending(&mut m);
    assert_eq!(masks(&m)[0], 0b0010, "nothing set back after the load");
    m.set_u32(ITEMS + 0x10, 0b1010).unwrap();
    m.set_u32(CURRENT_LAYER, 5).unwrap();

    // An item list built again before the next layer draw (another value
    // in the cleared item's mask): its mask is left alone.
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m)[0], 0b0010);
    m.set_u32(ITEMS + 0x10, 0b0100).unwrap();
    native_world::restore_pending(&mut m);
    assert_eq!(masks(&m)[0], 0b0100, "not the mask the skip left: not touched");

    // A part left to D3D keeps layer 3 though its shader is drawn: both
    // BSP items carry shader 0x10, and only part 0 is native.
    let mut m = guest();
    m.set_u32(ITEMS + 0x18 + 8, 0xe000_0010).unwrap();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b0010, 0b1010, 0b1010]);
    native_world::restore_pending(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);

    // The host is told how the items switch the stencil: none here.
    assert_eq!(stub.asked.lock().unwrap().last().unwrap().2, StencilPlan::default());
    // A marked section (record 1, cluster 0) and a marked object (record
    // 3): both switch, the section is named.
    let mut m = guest();
    m.set_u32(RENDER_RECORDS + 0x20, STENCIL_MARK).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 3, STENCIL_MARK).unwrap();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b0010, 0b1010, 0b1010], "still drawn natively");
    let plan = stub.asked.lock().unwrap().last().unwrap().2.clone();
    assert_eq!((plan.off, plan.switches, plan.marked), (false, true, vec![Owner::Cluster(0)]));
    native_world::restore_pending(&mut m);
    // The stencil-off flag.
    let mut m = guest();
    m.set_u32(STENCIL_OFF_FLAGS, 0x0000_0100).unwrap();
    native_world::before_layer(&mut m);
    assert!(stub.asked.lock().unwrap().last().unwrap().2.off);
    native_world::restore_pending(&mut m);
    // Unreadable switches (the list runs past the memory): the D3D path
    // draws the view, the host not asked.
    let asked = stub.asked.lock().unwrap().len();
    let left = native_world::stencil_views();
    let mut m = guest();
    m.set_u32(LIST + 4, 0x1000).unwrap();
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    assert_eq!(stub.asked.lock().unwrap().len(), asked);
    assert_eq!(native_world::stencil_views(), left + 1);

    // The first-person render's layer 3 (0x0003fdf0): the scene list
    // built again with models only, under the flag byte 0x00485a76 (the
    // stencil off). Not the world's: the host is not asked, so no native
    // pass draws the world again over the finished scene, and nothing is
    // cleared.
    let without = native_world::views_without_world();
    let mut m = guest();
    m.set_u32(STENCIL_OFF_FLAGS, 0x0001_0000).unwrap();
    for record in [1u32, 2] {
        m.set_u32(RENDER_RECORDS + 0x20 * record + 4, 0xe124_0002).unwrap();
    }
    native_world::before_layer(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    assert_eq!(stub.asked.lock().unwrap().len(), asked, "not asked");
    assert_eq!(native_world::views_without_world(), without + 1);
    // One BSP item back in the list: the world's layer 3 again.
    m.set_u32(RENDER_RECORDS + 0x20 * 2 + 4, u32::MAX).unwrap();
    native_world::before_layer(&mut m);
    assert_eq!(stub.asked.lock().unwrap().len(), asked + 1);
    native_world::restore_pending(&mut m);
}
