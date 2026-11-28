//! The `native-fog` feature (docs/graphics/native-water-fog.md, "Stage
//! 4b"): its plan next to the world's hook on the layer draw, and its one
//! observer on synthetic guest memory — the BSP items the host draws lose
//! layer 16's bit for the layer (only at the late passes' call, with the
//! layer draw's own gate holding) and get them back at the next layer
//! draw; nothing changes without a host, in another layer, from another
//! caller, with the gate off, or for a part the host leaves to the D3D
//! path.
#![cfg(feature = "native-render")]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use engine::memory::{FlatMemory, GuestMemory};
use render_bridge::native_fog::{self, Host, NativeFog};
use render_bridge::native_layers::Left;
use hooks::plan::Base;
use hooks::{Registry, Switches};
use native_render::fog::{self, FOG_BYTES, FOG_LAYER, FOG_MODE, SCENE_FOG_RETURN};
use native_render::skip::{Cleared, CURRENT_LAYER, CURRENT_LIST, GEOMETRY_BUILD, ITEM_SIZE, RENDER_RECORDS, STENCIL_OFF_FLAGS};
use native_render::world::Owner;

/// `HOST` and `CLEARED` are process-wide: the tests that touch them run
/// one at a time.
static LOCK: Mutex<()> = Mutex::new(());

/// A host that draws cluster 37's part 1 with shader 0x10 when `0`,
/// clearing its bits itself as the real one does.
struct OnePart(bool);

impl Host for OnePart {
    fn ask(&self, mem: &mut dyn GuestMemory, _display_clusters: &[u16], left: &mut Left) -> Option<Cleared> {
        if !self.0 {
            left.all("device");
            return None;
        }
        let parts: HashMap<(Owner, u8), u16> = [((Owner::Cluster(37), 1), 0x10)].into_iter().collect();
        let (_, cleared, refused) = fog::clear_fog(&mut render_bridge::native_fog::Mem(mem), FOG_LAYER as u32, &|item| {
            parts.get(&(item.owner, item.part)) == Some(&(item.shader))
        })?;
        for (slot, why) in refused.kept {
            left.item(slot, why);
        }
        Some(cleared)
    }

    fn ask_run(&self, _: &mut dyn GuestMemory, _: &render_bridge::late_sorted::RunAt<'_>, left: &mut Left) -> render_bridge::late_sorted::Run {
        left.all("device");
        render_bridge::late_sorted::Run::Refused
    }
}

fn install_host(draws: bool) {
    native_fog::set_host(Arc::new(OnePart(draws)));
}

/// A cluster and an entry-less object, with one spare slot outside the list.
fn scene() -> FlatMemory {
    let mut m = FlatMemory::new(0x0040_0000, 0x0021_0000);
    m.set_u32(CURRENT_LAYER, FOG_LAYER as u32).unwrap();
    m.set_u32(CURRENT_LIST, 0x004c_1a48).unwrap();
    m.set_u32(0x004c_1a48, 100).unwrap();
    m.set_u32(0x004c_1a48 + 4, 2).unwrap();
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
    // The record has a layer-16 entry; the planar gate (the ready byte
    // set, both fog bytes clear) makes it implementation 2.
    m.set_u32(RENDER_RECORDS + 0x20 * 2, fog::RECORD_FOG).unwrap();
    m.set_u32(fog::PLANAR_READY, 1).unwrap();
    m.set_u32(fog::FOG_BYTES, 0).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 2 + 0xc, 37 << 9).unwrap();
    m.set_u32(RENDER_RECORDS + 0x20 * 5, 0).unwrap();
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
    let switches = "render-timing=on";
    let mut r = Registry::new(Switches::parse(switches).unwrap());
    assert!(r.add(&render_bridge::render_timing::RenderTiming::new()).unwrap());
    assert!(r.add(&render_bridge::native_world::NativeWorld).unwrap());
    assert!(r.add(&NativeFog).unwrap());
    let plan = match r.build(&Base) {
        Ok(p) => p,
        Err(c) => panic!("refused: {c:?}"),
    };
    let text = plan.text();
    assert!(text.contains("native_fog_skip"), "{text}");
    assert!(text.contains("native_fog_sorted") && text.contains("native_fog_entry"), "per item over the sorted entries: {text}");
    assert!(!text.contains("native_fog_restore"), "nothing after the body: {text}");
}

#[test]
fn its_layer_draw_chain_runs_on_the_render_thread() {
    let mut r = Registry::new(Switches::parse("").unwrap());
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
fn it_is_on_by_default() {
    assert!(native_fog::on(&Switches::parse("").unwrap()));
}

#[test]
fn the_hosts_parts_lose_their_bit_and_get_it_back() {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_host(true);
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
fn the_hook_skips_both_clusters_around_an_object_without_a_fog_entry() {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_host(true);
    let mut m = scene();
    // Include a duplicate cluster in the spare slot after the entry-less object.
    m.set_u32(0x004c_1a48 + 4, 3).unwrap();
    let late = 0x0060_0000 + ITEM_SIZE * 2;
    m.set_u32(late, GEOMETRY_BUILD).unwrap();
    m.set_u32(late + 8, 0xe000_0010).unwrap();
    m.set_u32(late + 0xc, (2 << 8) | 1).unwrap();
    draw(&mut m);
    assert_eq!(mask(&m, 0), 1 << 3);
    assert_eq!(mask(&m, 1), (1 << 16) | (1 << 3));
    assert_eq!(mask(&m, 2), 1 << 3);
    native_fog::restore_pending(&mut m);
    assert_eq!(mask(&m, 2), (1 << 16) | (1 << 3));
    assert_eq!(mask(&m, 0), (1 << 16) | (1 << 3));
}

#[test]
fn nothing_changes_without_a_host_for_another_layer_or_gate_off() {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_host(false);
    let mut m = scene();
    draw(&mut m);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3);
    // Another layer.
    install_host(true);
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
    m.set_u32(STENCIL_OFF_FLAGS, 0x0001_0000).unwrap();
    native_fog::before_layer(&mut m, SCENE_FOG_RETURN);
    assert_eq!(mask(&m, 0), 1 << 3);
    native_fog::restore_pending(&mut m);
}

#[test]
fn another_caller_or_a_part_left_to_d3d_keeps_the_layer() {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_host(true);
    let mut m = scene();
    native_fog::before_layer(&mut m, 0x0004_0136);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3, "not the late passes' call");
    // A shader the host does not draw with: kept, even on the part.
    m.set_u32(0x0060_0000 + 8, 0xe000_0099).unwrap();
    native_fog::before_layer(&mut m, SCENE_FOG_RETURN);
    assert_eq!(mask(&m, 0), 1 << 16 | 1 << 3, "the shader is not the item's");
}

fn kept(name: &str) -> u64 {
    render_bridge::native_layers::counters().into_iter().find(|(k, _)| k == name).map_or(0, |(_, n)| n)
}

fn kept_16() -> u64 {
    render_bridge::native_layers::counters().into_iter().filter(|(k, _)| k.starts_with("scene_kept_16_")).map(|(_, n)| n).sum()
}

/// What the guest's layer draw still draws is counted
/// `scene_kept_16_<kind>_<cause>`: another caller's items under `caller`,
/// an item the walk refuses under its own cause, the host's refusal under
/// the host's. A record with no layer-16 entry draws nothing and is not
/// counted, and a layer the host takes keeps nothing.
#[test]
fn what_the_guest_still_draws_is_counted_scene_kept_by_cause() {
    let _held = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    install_host(true);
    let mut m = scene();
    let (caller, object) = (kept("scene_kept_16_world_caller"), kept("scene_kept_16_object_caller"));
    native_fog::before_layer(&mut m, 0x0004_0136);
    assert_eq!(kept("scene_kept_16_world_caller"), caller + 1, "not the late passes' call");
    assert_eq!(kept("scene_kept_16_object_caller"), object, "the object's record has no layer-16 entry");
    m.set_u32(0x0060_0000 + 8, 0xe000_0099).unwrap();
    let material = kept("scene_kept_16_world_material");
    draw(&mut m);
    assert_eq!(kept("scene_kept_16_world_material"), material + 1, "the walk's cause for the slot");
    install_host(false);
    let device = kept("scene_kept_16_world_device");
    draw(&mut m);
    assert_eq!(kept("scene_kept_16_world_device"), device + 1, "the host's cause for the view");
    install_host(true);
    m.set_u32(0x0060_0000 + 8, 0xe000_0010).unwrap();
    let before = kept_16();
    draw(&mut m);
    assert_eq!(mask(&m, 0), 1 << 3, "taken");
    assert_eq!(kept_16(), before, "a taken layer keeps nothing");
    native_fog::restore_pending(&mut m);
}
