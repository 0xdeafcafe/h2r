//! The `native-world` feature (docs/graphics/native-renderer.md): its plan
//! next to every other hook on the layer draw, its one observer on
//! synthetic guest memory (the BSP items the host draws lose layer 3's bit
//! for the layer and get it back at the next layer draw; nothing changes
//! without a host or in another layer; a structure item the host does not
//! draw loses the bit too and is counted `scene_dropped_world_*`, the
//! world drawing only from the scene), and its chain on a second executor.

use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use render_bridge::native_world::{self, Host, NativeWorld, PartSet};
use hooks::plan::Base;
use hooks::{Registry, Switches};
use native_render::camera::{RENDER_CAMERA, RenderCamera};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, RENDER_RECORDS, STENCIL_MARK, STENCIL_OFF_FLAGS, StencilPlan};
use native_render::world::Owner;

#[test]
fn the_plan_orders_it_inside_render_timing() {
    let switches = "render-timing=on,render-timing.deep=on";
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(&render_bridge::render_timing::RenderTiming::new()).unwrap());
    assert!(r.add(&NativeWorld).unwrap());
    let plan = match r.build(&Base) {
        Ok(p) => p,
        Err(c) => panic!("refused: {c:?}"),
    };
    let text = plan.text();
    assert!(text.contains("native_world_skip"), "{text}");
    assert!(!text.contains("native_world_restore"), "nothing after the body: {text}");
}

/// Alone at the layer draw (on by default), the chain runs on a second
/// executor: one context-only observer before, nothing after.
#[test]
fn its_layer_draw_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("").unwrap());
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

/// Off, it holds nothing and never blocks a snapshot; on (the default),
/// its section (no bytes) drops the pending restore at a load.
#[test]
fn its_snapshot_policy_is_stateless_off_and_a_section_on() {
    for (switches, policy) in [
        ("impl.render.native=off", "stateless"),
        ("", "section"),
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

/// The plan text with the native world alone.
fn plan_text(switches: &str) -> String {
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    r.add(&NativeWorld).unwrap();
    r.build(&Base).unwrap().text()
}

#[test]
fn it_is_on_by_default() {
    assert!(native_world::on(&Switches::parse("").unwrap()));
    let text = plan_text("");
    assert!(text.contains("native_world_skip"), "{text}");
}

#[test]
fn the_master_switch_off_places_nothing() {
    let text = plan_text("impl.render.native=off");
    assert!(!text.contains("native_world"), "{text}");
}

const LIST: u32 = 0x004c_1a48;
const ITEMS: u32 = 0x0060_0000;
const BSP: u32 = 0x0080_0000;

struct Stub {
    asked: Mutex<Vec<(u32, RenderCamera, StencilPlan)>>,
    parts: Arc<PartSet>,
    resolved: std::sync::atomic::AtomicBool,
}

impl Host for Stub {
    fn use_resolved_scene(&self, _: &dyn GuestMemory, bsp: u32) -> bool {
        assert_eq!(bsp, BSP);
        self.resolved.load(std::sync::atomic::Ordering::Relaxed)
    }

    fn ask(&self, mem: &mut dyn GuestMemory, bsp: u32, view: &frame_scene::View, stencil: &StencilPlan, claim_guest: bool) -> Option<native_render::skip::Cleared> {
        assert!(claim_guest);
        self.asked.lock().unwrap().push((bsp, view.camera, stencil.clone()));
        let mut mem = native_world::Mem(mem);
        let (masks, cleared, _) = native_world::plan_parts(&mem, &self.parts, |_, _| true)?;
        masks.apply(&mut mem).then_some(cleared)
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

/// `native_world::set_host` is process-wide: the tests that install one run
/// one at a time.
static HOST_TESTS: Mutex<()> = Mutex::new(());

#[test]
fn the_world_items_skip_layer_3_and_get_it_back_at_the_next_layer_draw() {
    let _serial = HOST_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    // No host yet: nothing changes.
    native_world::clear_host();
    let mut m = guest();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    native_world::restore_pending(&mut m);

    let stub = Arc::new(Stub {
        asked: Mutex::default(),
        resolved: Default::default(),
        parts: Arc::new([((Owner::Cluster(0), 0), 0xe000_0010u32)].into_iter().collect()),
    });
    native_world::set_host(stub.clone());

    native_world::before_layer(&mut m, 0);
    assert_eq!(
        masks(&m),
        vec![0b0010, 0b0010, 0b1010],
        "the drawn part's BSP item, and the part the host left as a drop; not the object"
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
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    assert_eq!(stub.asked.lock().unwrap().len(), 1);

    // Nothing pending: another layer draw changes nothing.
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b1010; 3]);

    // A snapshot load between a layer 3 and the next layer draw drops the
    // pending restore: the loaded RAM is not the memory it was made in.
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m)[0], 0b0010);
    native_world::forget_pending();
    native_world::restore_pending(&mut m);
    assert_eq!(masks(&m)[0], 0b0010, "nothing set back after the load");
    m.set_u32(ITEMS + 0x10, 0b1010).unwrap();
    m.set_u32(CURRENT_LAYER, 5).unwrap();

    // An item list built again before the next layer draw (another value
    // in the cleared item's mask): its mask is left alone.
    m.set_u32(CURRENT_LAYER, 3).unwrap();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m)[0], 0b0010);
    m.set_u32(ITEMS + 0x10, 0b0100).unwrap();
    native_world::restore_pending(&mut m);
    assert_eq!(masks(&m)[0], 0b0100, "not the mask the skip left: not touched");

    // A part the host does not draw loses layer 3 though its shader is
    // drawn: both BSP items carry shader 0x10, and only part 0 is native.
    let mut m = guest();
    m.set_u32(ITEMS + 0x18 + 8, 0xe000_0010).unwrap();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b0010, 0b0010, 0b1010]);
    native_world::restore_pending(&mut m);
    assert_eq!(masks(&m), vec![0b1010; 3]);

    // The host is told how the items switch the stencil: none here.
    assert_eq!(stub.asked.lock().unwrap().last().unwrap().2, StencilPlan::default());
    // A marked section (record 1, cluster 0) and a marked object (record
    // 3): both switch, the section is named.
    let mut m = guest();
    m.set_u32(RENDER_RECORDS + 0x20, STENCIL_MARK).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 3, STENCIL_MARK).unwrap();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b0010, 0b0010, 0b1010], "still drawn natively");
    let plan = stub.asked.lock().unwrap().last().unwrap().2.clone();
    assert_eq!((plan.off, plan.switches, plan.marked), (false, true, vec![Owner::Cluster(0)]));
    native_world::restore_pending(&mut m);
    // The stencil-off flag.
    let mut m = guest();
    m.set_u32(STENCIL_OFF_FLAGS, 0x0000_0100).unwrap();
    native_world::before_layer(&mut m, 0);
    assert!(stub.asked.lock().unwrap().last().unwrap().2.off);
    native_world::restore_pending(&mut m);
    // Unreadable switches (the list runs past the memory): the view is a
    // named drop that reads nothing, the host not asked.
    let asked = stub.asked.lock().unwrap().len();
    let left = native_world::stencil_views();
    let mut m = guest();
    m.set_u32(LIST + 4, 0x1000).unwrap();
    native_world::before_layer(&mut m, 0);
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
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    assert_eq!(stub.asked.lock().unwrap().len(), asked, "not asked");
    assert_eq!(native_world::views_without_world(), without + 1);
    // One BSP item back in the list: the world's layer 3 again.
    m.set_u32(RENDER_RECORDS + 0x20 * 2 + 4, u32::MAX).unwrap();
    native_world::before_layer(&mut m, 0);
    assert_eq!(stub.asked.lock().unwrap().len(), asked + 1);
    native_world::restore_pending(&mut m);

    // With resolved scene draws enabled, the opaque handler hooks consume
    // the guest's entries one by one and no bit is touched. The alternate
    // view can still use the mask reservation.
    let mut m = guest();
    stub.resolved.store(true, std::sync::atomic::Ordering::Relaxed);
    native_world::before_layer(&mut m, native_render::objects::SCENE_LAYER3_RETURN);
    assert_eq!(masks(&m), vec![0b1010; 3]);
    assert_eq!(stub.asked.lock().unwrap().len(), asked + 1);
    native_world::before_layer(&mut m, 0x0002_bd74);
    assert_eq!(masks(&m), vec![0b0010, 0b0010, 0b1010]);
    assert_eq!(stub.asked.lock().unwrap().len(), asked + 2);
    render_bridge::scene_step::with_open(|open| {
        let (_, view) = open.expect("alternate layer 3 opened a view");
        assert_eq!(view.key.kind, frame_scene::ViewKind::Alternate);
    });
    native_world::restore_pending(&mut m);
}

#[test]
fn the_world_build_places_only_context_only_scene_hooks() {
    let labels = ["native_world_visible_sections", "native_world_build_section",
        "native_world_build_entry", "native_world_empty_entries", "native_world_shader_states"];
    for (settings, enabled) in [("", true), ("impl.render.native=off", false)] {
        let mut r = Registry::new(Switches::parse(settings).unwrap());
        r.add(&NativeWorld).unwrap();
        let plan = r.build(&Base).unwrap();
        let text = plan.text();
        for label in labels {
            assert_eq!(text.contains(label), enabled, "{settings}: {label}");
        }
        assert!(plan.chains.values().all(|c| c.render_thread().is_ok()));
    }
}

/// A host that draws nothing.
struct Refuser;

impl Host for Refuser {
    fn ask(&self, _: &mut dyn GuestMemory, _: u32, _: &frame_scene::View, _: &StencilPlan, _: bool) -> Option<native_render::skip::Cleared> {
        None
    }
}

fn dropped(name: &str) -> u64 {
    render_bridge::native_layers::counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
}

/// Whatever the host refuses, no structure item of layer 3 is left for the
/// guest to build an entry for: the bit is gone, the drop is counted, and
/// the object (the sorted walk's) is untouched. The next layer draw sets
/// the bits back.
#[test]
fn no_opaque_item_reaches_the_device() {
    let _serial = HOST_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    native_world::set_host(Arc::new(Refuser));
    let before = dropped("scene_dropped_world_refused");
    let mut m = guest();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b0010, 0b0010, 0b1010], "both BSP items dropped, the object left");
    assert_eq!(dropped("scene_dropped_world_refused"), before + 2);
    m.set_u32(CURRENT_LAYER, 5).unwrap();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m), vec![0b1010; 3], "the bits are back for the next layer");
}

/// A part whose render record does not read is dropped by name, not left to
/// the guest.
#[test]
fn an_unreadable_part_is_a_named_drop() {
    let _serial = HOST_TESTS.lock().unwrap_or_else(|e| e.into_inner());
    native_world::set_host(Arc::new(Refuser));
    let before = dropped("scene_dropped_world_unreadable");
    let mut m = guest();
    // Item 1's render record index points far outside the memory.
    m.set_u32(ITEMS + 0x18 + 0xc, 0x7000 << 8 | 1).unwrap();
    native_world::before_layer(&mut m, 0);
    assert_eq!(masks(&m)[1], 0b0010, "no guest entry for it");
    assert_eq!(dropped("scene_dropped_world_unreadable"), before + 1);
    native_world::restore_pending(&mut m);
}
