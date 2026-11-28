//! Sorted entries are identified on the original guest stack, including
//! when another handler runs without calling the geometry observer.
#![cfg(feature = "native-render")]

use engine::{memory::{FlatMemory, GuestMemory}, points};
use render_bridge::native_transparents::{self, NativeTransparents, Scope};
use render_bridge::native_world::ACTIVE_BSP;
use hooks::plan::Base;
use hooks::{Registry, Switches};
use native_render::camera::GuestRead;
use native_render::skip::{RENDER_RECORDS, RENDER_RECORD_SIZE};
use native_render::transparent::{DATA_OFFSET, GEOMETRY_HANDLER, Source};

#[test]
fn the_switch_off_places_nothing() {
    let mut r = Registry::new(Switches::parse("impl.render.native=off").unwrap());
    r.add(&NativeTransparents).unwrap();
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    assert!(!text.contains("native_transparents"), "{text}");
    assert!(!text.contains("native_water_composite"), "{text}");
    assert!(!text.contains("native_late_subpart_draw"), "{text}");
    // Nothing placed, nothing held: no snapshot blocker.
    assert!(plan.snapshot_blockers().is_empty());
}

/// On too: what it holds lasts one view and changes no guest memory.
#[test]
fn on_its_snapshot_policy_is_stateless() {
    let mut r = Registry::new(Switches::defaults());
    assert!(r.add(&NativeTransparents).unwrap());
    let plan = r.build(&Base).unwrap();
    let policies: Vec<_> = plan
        .snapshot
        .iter()
        .map(|f| (f.id, f.policy.name()))
        .collect();
    assert_eq!(policies, vec![(native_transparents::ID, "stateless")]);
}

#[test]
fn the_scene_and_its_cleanup_share_the_render_thread() {
    let mut r = Registry::new(Switches::defaults());
    assert!(r.add(&NativeTransparents).unwrap());
    let plan = r.build(&Base).unwrap();
    let text = plan.text();
    for (point, label) in [
        ("TRANSPARENT_GEOMETRY", "native_transparents_entry"),
        ("SUBPART_STRIP_DRAW", "native_late_subpart_draw"),
        ("SUBPART_LIST_DRAW", "native_late_subpart_draw"),
        ("SUBPART_LIST_LAST", "native_late_subpart_draw"),
        ("GAME_ACTIVATE", "native_transparents_generation"),
        ("SCENE_TRANSPARENTS", "native_transparents_view"),
        ("SCENE_TRANSPARENTS_END", "native_transparents_end"),
    ] {
        assert!(text.contains(label), "{text}");
        let chain = plan.chains.values().find(|c| c.point.name == point).unwrap();
        assert!(!chain.needs_shadow(), "{point}");
        assert!(chain.render_thread().is_ok(), "{point}");
    }
}

const GLOBALS: u32 = 0x005e_0000;
const SCENE: u32 = 0x005f_1000;
const HANDLER: u32 = SCENE - 0x100;
const DRAW: u32 = HANDLER - 0x100;
const ENTRIES: u32 = 0x0060_0000;
const SORTED_RETURN: u32 = points::SCENE_TRANSPARENTS.base + 4;

struct Guest(FlatMemory);

impl GuestRead for Guest {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

impl Guest {
    fn set(&mut self, addr: u32, value: u32) {
        self.0.set_u32(addr, value).unwrap();
    }

    fn new() -> Self {
        let mut guest = Self(FlatMemory::new(0x0040_0000, 0x0021_0000));
        guest.set(points::GAME_GLOBALS.base, GLOBALS);
        guest.set(ACTIVE_BSP, 0x005d_0000);
        guest.set(GLOBALS, 2 << 16);
        guest.set(SCENE, 0x0010_0010);
        guest.set(HANDLER, SORTED_RETURN);
        guest
    }

    fn cluster_entry(&mut self, record: u32, part: u32) -> u32 {
        let at = ENTRIES + 0x70 * record;
        let row = RENDER_RECORDS + RENDER_RECORD_SIZE * record;
        self.set(at + 0x20, GEOMETRY_HANDLER);
        self.set(at + DATA_OFFSET, 0xe000_0010);
        self.set(at + DATA_OFFSET + 4, record << 8 | part);
        self.set(row + 4, u32::MAX);
        self.set(row + 0xc, 0x7ff << 18 | 20 << 9);
        at + DATA_OFFSET
    }
}

#[test]
fn same_shader_parts_keep_their_sorted_slots_across_a_d3d_handler() {
    let mut guest = Guest::new();
    let a = guest.cluster_entry(1, 3);
    let b = guest.cluster_entry(2, 5);
    let mut scope = Scope::default();
    scope.begin_view(&guest, SCENE);
    // The after hook changes the scene return slot before its body runs.
    guest.set(SCENE, 0xb000_0010);
    guest.set(HANDLER + 4, a);
    scope.begin_entry(&guest, HANDLER);
    let (generation, first) = scope.indexed_entry(&guest, DRAW).unwrap();
    assert_eq!((first.source, first.record, first.part), (Source::Cluster(20), 1, 3));

    // An effects handler uses the same dispatch frame and calls D3D.
    guest.set(HANDLER + 4, b + 0x70);
    assert_eq!(scope.indexed_entry(&guest, DRAW), None);

    guest.set(HANDLER + 4, b);
    scope.begin_entry(&guest, HANDLER);
    let (same_generation, second) = scope.indexed_entry(&guest, DRAW).unwrap();
    assert_eq!(generation, same_generation);
    assert_eq!(first.shader, second.shader);
    assert_eq!((second.source, second.record, second.part), (first.source, 2, 5));
    assert_eq!(scope.indexed_entry(&guest, HANDLER), None);

    scope.begin_view(&guest, SCENE);
    assert_eq!(scope.indexed_entry(&guest, DRAW), None);
}

#[test]
fn changing_bsp_at_the_same_address_invalidates_the_saved_entry() {
    let mut guest = Guest::new();
    let data = guest.cluster_entry(1, 3);
    guest.set(HANDLER + 4, data);
    let mut scope = Scope::default();
    scope.begin_view(&guest, SCENE);
    scope.begin_entry(&guest, HANDLER);
    let (first, _) = scope.indexed_entry(&guest, DRAW).unwrap();

    guest.set(GLOBALS, 3 << 16);
    assert_eq!(scope.indexed_entry(&guest, DRAW), None);
    scope.begin_view(&guest, SCENE);
    scope.begin_entry(&guest, HANDLER);
    let (second, _) = scope.indexed_entry(&guest, DRAW).unwrap();
    assert_ne!(first, second);

    guest.set(HANDLER, 0x0010_0020);
    scope.begin_entry(&guest, HANDLER);
    assert_eq!(scope.indexed_entry(&guest, DRAW), None);
}
