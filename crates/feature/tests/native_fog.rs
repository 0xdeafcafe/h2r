//! The `native-fog` feature (docs/graphics/native-renderer.md, "Stage
//! 4b"): its plan next to the world's hook on the layer draw, and its one
//! observer on synthetic guest memory — the BSP items the host draws lose
//! layer 16's bit for the layer (only at the late passes' call, with the
//! layer draw's own gate holding) and get them back at the next layer
//! draw; nothing changes without a host, in another layer, from another
//! caller, with the gate off, or for a part the host leaves to the D3D
//! path.

use std::collections::HashMap;
use std::sync::Arc;

use engine::memory::{FlatMemory, GuestMemory};
use feature::native_fog::{self, Host, NativeFog};
use feature::plan::Base;
use feature::{Registry, Switches};
use native_render::fog::{FOG_BYTES, FOG_LAYER, FOG_MODE, SCENE_FOG_RETURN};
use native_render::skip::{CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, RENDER_RECORDS, STENCIL_OFF_FLAGS};
use native_render::world::Owner;

/// A host that draws one part: cluster 37's part 1 with shader 0x10.
struct OnePart(Option<Arc<HashMap<(Owner, u8), u16>>>);

impl Host for OnePart {
    fn ask(&self, _mem: &dyn GuestMemory) -> Option<Arc<HashMap<(Owner, u8), u16>>> {
        self.0.clone()
    }
}

fn install_host(parts: Option<HashMap<(Owner, u8), u16>>) {
    native_fog::set_host(Arc::new(OnePart(parts.map(Arc::new))));
}

/// A layer-16 list of three items: a BSP cluster, an object, and another
/// build's; records 2 (BSP), 5 (object).
fn scene() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(CURRENT_LAYER, FOG_LAYER as u32).unwrap();
    m.set_u32(CURRENT_LIST, 0x004c_1a48).unwrap();
    m.set_u32(0x004c_1a48, 100).unwrap();
    m.set_u32(0x004c_1a48 + 4, 3).unwrap();
    m.set_u32(0x004c_1a48 + 0xc, 0).unwrap();
    m.set_u32(0x004c_1a48 + 0x14, 0x0060_0000).unwrap();
    let item = |m: &mut FlatMemory, i: u32, build: u32, record: u32| {
        let a = 0x0060_0000 + ITEM_SIZE * i;
        m.set_u32(a, build).unwrap();
        m.set_u32(a + 4, 0x0004_d0b0).unwrap();
        m.set_u32(a + 8, 0xe000_0010 + i).unwrap();
        m.set_u32(a + 0xc, record << 8 | (i + 1)).unwrap();
        m.set_u32(a + 0x10, 1 << 16 | 1 << 3).unwrap();
    };
    item(&mut m, 0, GEOMETRY_BUILD, 2);
    item(&mut m, 1, GEOMETRY_BUILD, 5);
    item(&mut m, 2, 0x0005_0000, 2);
    m.set_u32(RENDER_RECORDS + 0x20 * 2 + 4, u32::MAX).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 2, 37 << 9).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 5 + 4, 0xe1a3_0004).unwrap();
    // The layer draw's gate on: the fog mode 3.
    m.set_u32(FOG_MODE, 3).unwrap();
    m
}

fn mask(m: &FlatMemory, i: u32) -> u32 {
    m.u32(0x0060_0000 + ITEM_SIZE * i + 0x10).unwrap()
}

/// The observer with the gate on and the late passes' caller.
fn draw(m: &mut FlatMemory) {
    native_fog::before_layer(m, SCENE_FOG_RETURN);
}

#[test]
fn the_plan_orders_it_inside_render_timing_and_the_world() {
    let switches = "impl.render.native_fog=on,render-timing=on,impl.render.native_world=on";
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(&feature::render_timing::RenderTiming::new()).unwrap());
    assert!(r.add(&feature::native_world::NativeWorld).unwrap());
    assert!(r.add(&NativeFog).unwrap());
    let plan = match r.build(&Base) {
        Ok(p) => p,
        Err(c) => panic!("refused: {c:?}"),
    };
    let text = plan.text();
    assert!(text.contains("native_fog_skip"), "{text}");
    assert!(!text.contains("native_fog_restore"), "nothing after the body: {text}");
}

#[test]
fn its_layer_draw_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("impl.render.native_fog=on").unwrap());
    assert!(r.add(&NativeFog).unwrap());
    let plan = r.build(&Base).unwrap();
    let chain = plan
        .chains
        .values()
        .find(|c| c.point.name == "LAYER_DRAW")
        .expect("placed at the layer draw");
    assert!(!chain.needs_shadow());
    assert!(chain.render_thread().is_ok());
}

#[test]
fn it_is_off_by_default() {
    assert!(!native_fog::DEFAULT);
    let mut r = Registry::new(Switches::parse("").unwrap());
    assert!(r.add(&NativeFog).unwrap());
    assert!(!r.build(&Base).unwrap().text().contains("native_fog"));
}

/// A host that takes every part.
fn all_parts() -> HashMap<(Owner, u8), u16> {
    [((Owner::Cluster(37), 1), 0x10)].into_iter().collect()
}

#[test]
fn the_hosts_parts_lose_their_bit_and_get_it_back() {
    install_host(Some(all_parts()));
    let mut m = scene();
    draw(&mut m);
    assert_eq!(mask(&m, 0), 1 << 3, "the cluster's layer-16 bit gone, layer 3 kept");
    assert_eq!(mask(&m, 1), 1 << 16 | 1 << 3, "the object's kept (stage 2's)");
    assert_eq!(mask(&m, 2), 1 << 16 | 1 << 3, "another build's kept");
    // The next layer draw sets it back.
    m.set_u32(CURRENT_LAYER, 6).unwrap();
    native_fog::before_layer(&mut m, 0x0002_c109);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3);
}

#[test]
fn nothing_changes_without_a_host_for_another_layer_or_gate_off() {
    install_host(None);
    let mut m = scene();
    draw(&mut m);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3);
    // Another layer.
    install_host(Some(all_parts()));
    m.set_u32(CURRENT_LAYER, 6).unwrap();
    native_fog::before_layer(&mut m, SCENE_FOG_RETURN);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3);
    // The gate off (mode not 3, no fog byte): the layer draws no fog, so
    // neither does the native pass.
    m.set_u32(CURRENT_LAYER, FOG_LAYER as u32).unwrap();
    m.set_u32(FOG_MODE, 0).unwrap();
    m.set_u32(FOG_BYTES, 0).unwrap();
    native_fog::before_layer(&mut m, SCENE_FOG_RETURN);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3);
    // A fog byte draws it (the player chain's).
    m.set_u32(STENCIL_OFF_FLAGS, 0x0001_0000);
    native_fog::before_layer(&mut m, SCENE_FOG_RETURN);
    assert_eq!(mask(&m, 0), 1 << 3);
}

#[test]
fn another_caller_or_a_part_left_to_d3d_keeps_the_layer() {
    install_host(Some(all_parts()));
    let mut m = scene();
    native_fog::before_layer(&mut m, 0x0004_0136);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3, "not the late passes' call");
    // A part the host does not draw (another shader on it).
    let other: HashMap<_, _> = [((Owner::Cluster(37), 1), 0x99)].into_iter().collect();
    install_host(Some(other));
    native_fog::before_layer(&mut m, SCENE_FOG_RETURN);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3, "the shader is not the item's");
}
