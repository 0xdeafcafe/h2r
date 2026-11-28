//! Each port on synthetic memory: retail's globals at their addresses, the
//! objects in a low arena.
use super::*;
use runtime::guest::Permissions;
use runtime::Guest;
use sim_core::Epoch;
use sim_havok::structure::{POLYGON_RADIUS, POLYGON_VTABLE};

const ARENA: u32 = 0x0001_0000;
const GLOBALS: u32 = 0x0040_0000;

const BSP: u32 = ARENA + 0x100;
const SURFACES: u32 = ARENA + 0x200;
const EDGES: u32 = ARENA + 0x300;
const VERTICES: u32 = ARENA + 0x400;
const BREAKABLES: u32 = ARENA + 0x500;
const COLLECTION: u32 = ARENA + 0x600;
const COLLECTION_VT: u32 = ARENA + 0x700;
const FILTER: u32 = ARENA + 0x800;
const SHAPE: u32 = ARENA + 0x1000;
const INPUT: u32 = ARENA + 0x2000;
const BODY: u32 = ARENA + 0x2100;
const MOTION: u32 = ARENA + 0x2200;
const COLLECTOR: u32 = ARENA + 0x2400;
const COLLECTOR_VT: u32 = ARENA + 0x2480;
const RESULT: u32 = ARENA + 0x2500;
const OUT: u32 = ARENA + 0x2600;
const STRUCTURE: u32 = ARENA + 0x3000;
const INSTANCES: u32 = ARENA + 0x3100;
const DEFINITIONS: u32 = ARENA + 0x3200;
const DEF_COLLECTION: u32 = ARENA + 0x3400;
const TRANSFORM: u32 = ARENA + 0x4000;
const WRAPPER: u32 = ARENA + 0x4100;
const WRAPPER_VT: u32 = ARENA + 0x4180;
const TRANSFORM_VT: u32 = ARENA + 0x41c0;
/// I's entry ESP for the nesting rays; their frames go below it.
const STACK: u32 = ARENA + 0xf000;

fn world() -> &'static CollisionWorld {
    static EPOCH: Epoch = Epoch::new();
    static WORLD: std::sync::LazyLock<CollisionWorld> = std::sync::LazyLock::new(|| CollisionWorld::new(&EPOCH));
    &WORLD
}

fn f(v: f32) -> u32 {
    v.to_bits()
}

/// Retail's constants, projection rows and an empty level.
fn guest() -> Guest {
    let mut g = Guest::new();
    g.map(ARENA, 0x1_0000, 0, Permissions::RW).unwrap();
    g.map(GLOBALS, 0x12_0000, 0x1_0000, Permissions::RW).unwrap();
    for (at, v) in [
        (data::ONE, f(1.0)),
        (data::NEG_ONE, f(-1.0)),
        (data::MIN_LENGTH, 0x38d1_b717),
        (POLYGON_RADIUS, 0x3c23_d70a),
        (data::STRUCTURE_BSP, BSP),
        (data::BREAKABLE_BITS, BREAKABLES),
    ] {
        g.set_u32(at, v).unwrap();
    }
    let rows = [0x10002, 0x10000, 0x2, 0x20000, 0x20001, 0x10000, 0x1, 0x2, 0x20001, 0x10002];
    for (k, w) in rows.into_iter().enumerate() {
        g.set_u32(data::PROJECTION + 4 * k as u32, w).unwrap();
    }
    g.set_u16(data::BREAKABLE_INDEX, 0).unwrap();
    g
}

/// Surface 0: the triangle (0,0,0), (1,0,0), (0,1,0) wound about +z,
/// breakable index 3 when `breakable`.
fn triangle(g: &mut Guest, breakable: bool) {
    g.set_u32(BSP + 0x2c, SURFACES).unwrap();
    g.set_u32(BSP + 0x34, EDGES).unwrap();
    g.set_u32(BSP + 0x3c, VERTICES).unwrap();
    // First edge 0; flags, breakable index, material.
    g.set_u16(SURFACES + 2, 0).unwrap();
    g.set_u8(SURFACES + 4, if breakable { 8 } else { 0 }).unwrap();
    g.set_u8(SURFACES + 5, 3).unwrap();
    g.set_u16(SURFACES + 6, 7).unwrap();
    for (e, (start, end, next)) in [(0u16, 1u16, 1u16), (1, 2, 2), (2, 0, 0)].into_iter().enumerate() {
        let at = EDGES + 12 * e as u32;
        for (off, v) in [(0, start), (2, end), (4, next), (6, next), (8, 0), (0xa, 0xffff)] {
            g.set_u16(at + off, v).unwrap();
        }
    }
    for (i, p) in [[0.0f32, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]].into_iter().enumerate() {
        for (k, c) in p.into_iter().enumerate() {
            g.set_u32(VERTICES + 16 * i as u32 + 4 * k as u32, f(c)).unwrap();
        }
    }
}

fn cpu(control: u16) -> CpuAbi {
    let mut c = CpuAbi { mxcsr: 0x1f80, ..CpuAbi::default() };
    c.x87.control = control;
    c
}

#[test]
fn the_group_filter_reads_the_rays_row_at_the_keys_info_bit() {
    let mut g = guest();
    g.set_u32(COLLECTION, COLLECTION_VT).unwrap();
    g.set_u32(COLLECTION_VT + 0x38, target::NO_FILTER_INFO).unwrap();
    // Ray filter info with a group in the high half: row 5.
    g.set_u32(INPUT + 0x20, 0x0003_0005).unwrap();
    for (row, pass) in [(1, 1), (0, 0), (2, 0)] {
        g.set_u32(FILTER + 0x0c + 4 * 5, row).unwrap();
        g.set_u8(OUT, 0xaa).unwrap();
        let call = GuestCall::new(target::GROUP_FILTER).ecx(FILTER).args([OUT, INPUT, COLLECTION, 9]);
        assert_eq!(super::call(&mut g, world(), &mut cpu(0x027f), &call).unwrap(), OUT);
        assert_eq!(g.u8(OUT).unwrap(), pass, "row {row:#x}");
    }
}

#[test]
#[should_panic(expected = "collision: mopp_ray: filter info 0x00123450")]
fn a_collection_with_its_own_filter_info_is_named() {
    let mut g = guest();
    g.set_u32(COLLECTION, COLLECTION_VT).unwrap();
    g.set_u32(COLLECTION_VT + 0x38, 0x0012_3450).unwrap();
    let _ = group_filter(&mut g, FILTER, OUT, INPUT, COLLECTION, 0);
}

#[test]
fn a_level_surface_key_builds_its_polygon_and_a_broken_one_the_empty_shape() {
    let key = 1 << 29;
    let mut g = guest();
    triangle(&mut g, true);
    // Breakable 3 of the structure (index -1): the word at bits + 1.
    g.set_u32(BREAKABLES + 1, 1 << 3).unwrap();
    assert!(level_key(&g, key).unwrap());
    assert_eq!(level_child(&mut g, key, SHAPE, 0x1f80).unwrap(), SHAPE);
    assert_eq!(g.u32(SHAPE).unwrap(), POLYGON_VTABLE);
    assert_eq!(g.u32(SHAPE + 0x14).unwrap(), 3);
    assert_eq!(g.u32(SHAPE + 0x08).unwrap(), key, "the tail is the key");
    assert_eq!(g.u32(SHAPE + 0x1c).unwrap(), u32::MAX, "no owner");
    assert_eq!(g.u32(SHAPE + 0x30).unwrap(), f(1.0), "vertex 1 x");

    g.set_u32(BREAKABLES + 1, 0).unwrap();
    assert!(!level_key(&g, key).unwrap());
    assert_eq!(level_child(&mut g, key, SHAPE, 0x1f80).unwrap(), SHAPE);
    assert_eq!(g.u32(SHAPE).unwrap(), data::EMPTY_SHAPE);
    assert_eq!(g.u16(SHAPE + 6).unwrap(), 1);
    assert_eq!(g.u32(SHAPE + 8).unwrap(), 0);
    // A null buffer builds nothing.
    assert_eq!(level_child(&mut g, key, 0, 0x1f80).unwrap(), 0);
    // Types past retail's jump table are data it does not model.
    assert_eq!(level_key(&g, 0), Err(Bail::Data));
}

/// Instance 1 of the active structure, definition 0, placed with scale
/// one at `(0, 0, 2)`: the triangle as its definition's surface 0.
fn instance_scene(g: &mut Guest) {
    triangle(g, false);
    g.set_u32(data::STRUCTURE, STRUCTURE).unwrap();
    g.set_u32(STRUCTURE + 0x144, INSTANCES).unwrap();
    g.set_u32(STRUCTURE + 0x13c, DEFINITIONS).unwrap();
    let instance = INSTANCES + 0x58;
    g.set_u32(instance, f(1.0)).unwrap();
    for c in 0..3 {
        g.set_u32(instance + 4 + 0x0c * c + 4 * c, f(1.0)).unwrap();
    }
    g.set_u32(instance + 0x30, f(2.0)).unwrap();
    g.set_u16(instance + 0x34, 0).unwrap();
    g.set_u32(DEFINITIONS + 0x9c, SURFACES).unwrap();
    g.set_u32(DEFINITIONS + 0xb4, DEF_COLLECTION).unwrap();
    g.set_u32(DEF_COLLECTION + 0x0c, BSP).unwrap();
    g.set_u32(DEF_COLLECTION + 0x30, 0x4321).unwrap();
}

#[test]
fn an_instance_surface_key_builds_its_polygon_in_the_instances_space() {
    let key = (5 << 29) | 1;
    let mut g = guest();
    instance_scene(&mut g);
    assert!(level_key(&g, key).unwrap());
    assert_eq!(level_child(&mut g, key, SHAPE, 0x1f80).unwrap(), SHAPE);
    assert_eq!(g.u32(SHAPE).unwrap(), POLYGON_VTABLE);
    assert_eq!(g.u32(SHAPE + 0x08).unwrap(), key, "the tail is the key");
    assert_eq!(g.u32(SHAPE + 0x1c).unwrap(), 0x4321, "the collection's owner");
    assert_eq!(g.u32(SHAPE + 0x30).unwrap(), f(1.0), "vertex 1 x");
    assert_eq!(g.u32(SHAPE + 0x38).unwrap(), f(2.0), "vertex 1 z, placed");
    assert_eq!(level_child(&mut g, key, 0, 0x1f80).unwrap(), 0);
}

#[test]
fn an_instance_key_builds_a_transform_shape_over_the_definitions_tree() {
    let key = (2 << 29) | 1;
    let tree = DEF_COLLECTION + data::COLLECTION_TREE;
    let mut g = guest();
    instance_scene(&mut g);
    g.set_u16(tree + 6, 3).unwrap();
    assert_eq!(level_child(&mut g, key, SHAPE, 0x1f80).unwrap(), SHAPE);
    assert_eq!(g.u32(SHAPE).unwrap(), data::TRANSFORM_SHAPE);
    assert_eq!(g.u16(SHAPE + 6).unwrap(), 1);
    assert_eq!(g.u32(SHAPE + 0x0c).unwrap(), tree);
    assert_eq!(g.u16(tree + 6).unwrap(), 3, "the reference taken and dropped");
    let words: Vec<u32> = (0..16).map(|k| g.u32(SHAPE + 0x10 + 4 * k).unwrap()).collect();
    let (one, two) = (f(1.0), f(2.0));
    assert_eq!(words, [one, 0, 0, 0, 0, one, 0, 0, 0, 0, one, 0, 0, 0, two, 0], "the placement without its scale");
}

#[test]
#[should_panic(expected = "frees shape")]
fn an_instance_tree_freed_outside_the_tags_is_named() {
    let mut g = guest();
    g.map(0x0054_7000, 0x1000, 0x13_0000, Permissions::RW).unwrap();
    instance_scene(&mut g);
    let _ = level_child(&mut g, (2 << 29) | 1, SHAPE, 0x1f80);
}

#[test]
fn an_object_bsp_child_is_its_surfaces_polygon() {
    let mut g = guest();
    triangle(&mut g, false);
    g.set_u32(COLLECTION + 0x0c, BSP).unwrap();
    g.set_u32(COLLECTION + 0x30, 0x1234).unwrap();
    assert_eq!(bsp_child(&mut g, COLLECTION, 0, SHAPE, 0x1f80).unwrap(), SHAPE);
    assert_eq!(g.u32(SHAPE).unwrap(), POLYGON_VTABLE);
    assert_eq!(g.u32(SHAPE + 0x1c).unwrap(), 0x1234);
    assert_eq!(g.u32(SHAPE + 0x08).unwrap(), 0);
}

#[test]
fn an_instance_child_names_the_collection_and_key() {
    let mut g = guest();
    g.set_u32(COLLECTION + 0x0c, 0x4321).unwrap();
    assert_eq!(instance_child(&mut g, COLLECTION, 0x77, SHAPE).unwrap(), SHAPE);
    assert_eq!(g.u32(SHAPE).unwrap(), data::INSTANCE_SHAPE);
    assert_eq!(g.u32(SHAPE + 0x0c).unwrap(), 0x4321);
    assert_eq!(g.u32(SHAPE + 0x10).unwrap(), 0x77);
    assert_eq!(g.u16(SHAPE + 6).unwrap(), 1);
}

/// The triangle as an object BSP child in `SHAPE`, a level body with the
/// identity basis, the phantom collector at fraction one and its result
/// with the ray's direction.
fn ray_scene(from: [f32; 3], to: [f32; 3]) -> Guest {
    let mut g = guest();
    triangle(&mut g, false);
    g.set_u32(COLLECTION + 0x0c, BSP).unwrap();
    g.set_u32(COLLECTION + 0x30, 0).unwrap();
    bsp_child(&mut g, COLLECTION, 0, SHAPE, 0x1f80).unwrap();
    for k in 0..3 {
        g.set_u32(INPUT + 4 * k, f(from[k as usize])).unwrap();
        g.set_u32(INPUT + 0x10 + 4 * k, f(to[k as usize])).unwrap();
        g.set_u32(RESULT + 0x0c + 4 * k, f(to[k as usize] - from[k as usize])).unwrap();
    }
    g.set_u32(BODY + 4, 0x55).unwrap();
    g.set_u32(BODY + 8, MOTION).unwrap();
    g.set_u32(BODY + 0x0c, 0).unwrap();
    for c in 0..3 {
        g.set_u32(MOTION + 0x20 + 0x10 * c + 4 * c, f(1.0)).unwrap();
    }
    g.set_u32(COLLECTOR, COLLECTOR_VT).unwrap();
    g.set_u32(COLLECTOR_VT, target::PHANTOM_COLLECTOR).unwrap();
    g.set_u32(COLLECTOR + 4, f(1.0)).unwrap();
    g.set_u32(COLLECTOR + 8, RESULT).unwrap();
    g.set_u32(COLLECTOR + 0x0c, u32::MAX).unwrap();
    g.set_u32(RESULT + 0x18, f(1.0)).unwrap();
    g
}

fn cast(g: &mut Guest, control: u16) -> CpuAbi {
    let mut c = cpu(control);
    let call = GuestCall::new(target::CONVEX_RAY).ecx(SHAPE).eax(SHAPE).args([INPUT, BODY, COLLECTOR]);
    super::call(g, world(), &mut c, &call).unwrap();
    c
}

#[test]
fn a_ray_through_the_polygon_reaches_the_phantom_collector_in_both_precisions() {
    for control in [0x027f, 0x007f] {
        let mut g = ray_scene([0.25, 0.25, 1.0], [0.25, 0.25, -1.0]);
        let c = cast(&mut g, control);
        assert_eq!(g.u8(COLLECTOR + 0x10).unwrap(), 1, "the hit byte");
        assert_eq!(g.u32(COLLECTOR + 4).unwrap(), f(0.5), "the early-out fraction");
        assert_eq!(g.u32(RESULT + 0x18).unwrap(), f(0.5));
        let normal = [0x1c, 0x20, 0x24].map(|o| g.u32(RESULT + o).unwrap());
        assert_eq!(normal, [0, 0, f(1.0)]);
        assert_eq!(c.mxcsr & !0x3f, 0x1f80);
    }
}

#[test]
fn a_ray_outside_the_ring_or_from_behind_is_not_kept() {
    for (from, to) in [([2.0, 2.0, 1.0], [2.0, 2.0, -1.0]), ([0.25, 0.25, -1.0], [0.25, 0.25, 1.0])] {
        let mut g = ray_scene(from, to);
        cast(&mut g, 0x027f);
        assert_eq!(g.u32(COLLECTOR + 4).unwrap(), f(1.0), "{from:?}");
        assert_eq!(g.u32(RESULT + 0x18).unwrap(), f(1.0), "{from:?}");
    }
}

#[test]
fn the_polygon_cast_finds_the_plane_fraction_and_unit_normal() {
    let mut g = ray_scene([0.25, 0.25, 3.0], [0.25, 0.25, -1.0]);
    // A scaled triangle: the normal is normalised.
    for (i, k) in [(1, 0), (2, 1)] {
        g.set_u32(SHAPE + 0x20 + 16 * i + 4 * k, f(4.0)).unwrap();
    }
    let mut fp = Fp::guest(0x1f80);
    let hit = polygon_cast(&g, &mut fp, Precision::Double, SHAPE, INPUT, f(1.0)).unwrap().unwrap();
    assert_eq!(hit.fraction, f(0.75));
    assert_eq!(hit.normal, [0, 0, f(1.0), 0]);
    // Below the collector's fraction only.
    assert_eq!(polygon_cast(&g, &mut fp, Precision::Double, SHAPE, INPUT, f(0.75)).unwrap(), None);
}

#[test]
fn a_collector_ignores_its_objects_hits() {
    let mut g = ray_scene([0.25, 0.25, 1.0], [0.25, 0.25, -1.0]);
    // The body's root is a body owned by an object whose `0x2001` word is 9.
    let (root, owner, pairs) = (ARENA + 0x2800, ARENA + 0x2900, ARENA + 0x2a00);
    g.set_u32(BODY + 0x0c, root).unwrap();
    g.set_u32(root + 0x0c, 0).unwrap();
    g.set_u32(root + 0x18, 1).unwrap();
    g.set_u32(root + 0x20, owner).unwrap();
    g.set_u32(owner + 0x30, pairs).unwrap();
    g.set_u32(owner + 0x34, 2).unwrap();
    for (at, v) in [(pairs, 0x1000), (pairs + 4, 3), (pairs + 8, 0x2001), (pairs + 12, 9)] {
        g.set_u32(at, v).unwrap();
    }
    g.set_u32(COLLECTOR + 0x0c, 9).unwrap();
    cast(&mut g, 0x027f);
    assert_eq!(g.u32(COLLECTOR + 4).unwrap(), f(1.0));
    g.set_u32(COLLECTOR + 0x0c, 3).unwrap();
    cast(&mut g, 0x027f);
    assert_eq!(g.u32(COLLECTOR + 4).unwrap(), f(0.5));
}

#[test]
fn the_empty_shapes_ray_returns_its_shape() {
    let mut g = guest();
    let call = GuestCall::new(target::EMPTY_RAY).ecx(SHAPE).eax(SHAPE).args([INPUT, BODY, COLLECTOR]);
    assert_eq!(super::call(&mut g, world(), &mut cpu(0x027f), &call).unwrap(), SHAPE);
}

/// Entry E's collector over key `0x0001_0002`: placement 0x10002 of a
/// structure with one definition and no surfaces, which the definition
/// filter `0x001689b0` rejects.
fn instance_collector_scene() -> Guest {
    let mut g = guest();
    g.set_u32(COLLECTOR, COLLECTOR_VT).unwrap();
    g.set_u32(COLLECTOR_VT, target::INSTANCE_COLLECTOR).unwrap();
    g.set_u32(COLLECTOR + 4, f(0.75)).unwrap();
    g.set_u32(COLLECTOR + 0x10, INPUT).unwrap();
    g.set_u32(COLLECTOR + 0x14, INPUT + 0x10).unwrap();
    g.set_u32(COLLECTOR + 0x1c, RESULT).unwrap();
    g.set_u32(RESULT + 4, f(0.75)).unwrap();
    g.set_u32(BODY + 4, 0x0001_0002).unwrap();
    g.set_u32(data::INSTANCE_EPOCH, 7).unwrap();
    g.set_u32(data::STRUCTURE, STRUCTURE).unwrap();
    g.set_u32(STRUCTURE + 0x140, 0x0002_0000).unwrap();
    g.set_u32(STRUCTURE + 0x144, 0x0060_0000).unwrap();
    g.set_u32(STRUCTURE + 0x138, 1).unwrap();
    g.set_u32(STRUCTURE + 0x13c, DEFINITIONS).unwrap();
    g
}

#[test]
fn the_instance_collector_marks_each_key_once_by_its_low_half() {
    let mut g = instance_collector_scene();
    // Placement 0x10002 sits at 0x600000 + 0x10002 * 0x58.
    g.map(0x00b8_0000, 0x1000, 0x13_0000, Permissions::RW).unwrap();
    let mark = instance::MARK_BASE + 4 * 2;
    let call = GuestCall::new(target::INSTANCE_RAY).ecx(SHAPE).args([INPUT, BODY, COLLECTOR]);
    super::call(&mut g, world(), &mut cpu(0x027f), &call).unwrap();
    assert_eq!(g.u32(mark).unwrap(), 7, "marked before the placement test");
    assert_eq!(g.u8(COLLECTOR + 0x18).unwrap(), 0, "rejected: no hit");
    assert_eq!(g.u32(COLLECTOR + 4).unwrap(), f(0.75));
    // A marked key returns before the placement: an unreadable placement
    // block is not reached.
    g.set_u32(STRUCTURE + 0x144, 0xdead_0000).unwrap();
    assert_eq!(super::call(&mut g, world(), &mut cpu(0x027f), &call).unwrap(), 0x0001_0002);
    assert_eq!(g.u32(RESULT).unwrap(), 0, "the result is untouched");
}

/// A level body over a transform shape (`TRANSFORM`, the identity moved
/// to `(0, 0, 2)`) whose child is a wrapper over the triangle polygon.
fn transform_scene(from: [f32; 3], to: [f32; 3]) -> Guest {
    let mut g = ray_scene(from, to);
    g.set_u32(TRANSFORM, TRANSFORM_VT).unwrap();
    g.set_u32(TRANSFORM_VT + 0x24, target::TRANSFORM_RAY).unwrap();
    g.set_u32(TRANSFORM + 0x0c, WRAPPER).unwrap();
    for c in 0..3 {
        g.set_u32(TRANSFORM + 0x10 + 0x10 * c + 4 * c, f(1.0)).unwrap();
    }
    g.set_u32(TRANSFORM + 0x48, f(2.0)).unwrap();
    g.set_u32(WRAPPER, WRAPPER_VT).unwrap();
    g.set_u32(WRAPPER_VT + 0x24, target::WRAPPER_RAY).unwrap();
    g.set_u32(WRAPPER + 0x0c, SHAPE).unwrap();
    g.set_u32(POLYGON_VTABLE + 0x20, target::POLYGON_CAST).unwrap();
    g.set_u32(POLYGON_VTABLE + 0x24, target::CONVEX_RAY).unwrap();
    g.set_u32(INPUT + 0x20, 0x0003_0005).unwrap();
    g
}

fn transform_cast(g: &mut Guest) -> u32 {
    let mut c = cpu(0x027f);
    c.esp = STACK;
    let call = GuestCall::new(target::TRANSFORM_RAY).ecx(TRANSFORM).eax(TRANSFORM).args([INPUT, BODY, COLLECTOR]);
    super::call(g, world(), &mut c, &call).unwrap()
}

#[test]
fn a_transform_shape_casts_its_child_in_local_space_through_a_wrapper() {
    let mut g = transform_scene([0.25, 0.25, 3.0], [0.25, 0.25, 1.0]);
    transform_cast(&mut g);
    assert_eq!(g.u8(COLLECTOR + 0x10).unwrap(), 1, "the hit byte");
    assert_eq!(g.u32(RESULT + 0x18).unwrap(), f(0.5));
    let normal = [0x1c, 0x20, 0x24].map(|o| g.u32(RESULT + o).unwrap());
    assert_eq!(normal, [0, 0, f(1.0)], "turned by the composed motion");
    // The frame: the callee ESP is below I's, the arguments and the
    // return; the local ray 0x90 below its aligned frame.
    let esp = STACK - 0x10;
    let s = ((esp - 4) & !15) - 0xac;
    let local = (0..4).map(|k| g.u32(s + 0x1c + 4 * k).unwrap()).collect::<Vec<_>>();
    assert_eq!(&local[..3], &[f(0.25), f(0.25), f(1.0)]);
    assert_eq!(g.u32(s + 0x1c + 0x20).unwrap(), 0x0003_0005, "the filter info kept");
    let body = (0..4).map(|k| g.u32(s + 0x0c + 4 * k).unwrap()).collect::<Vec<_>>();
    assert_eq!(body, [WRAPPER, 0x55, s + 0x4c, BODY]);
    assert_eq!(g.u32(s + 0x4c + 0x20 + 0x38).unwrap(), f(2.0), "the composed translation");
}

#[test]
fn a_transform_shape_misses_what_its_placement_moves_away() {
    // Through the triangle in world space, past it once moved up by two.
    let mut g = transform_scene([0.25, 0.25, 1.0], [0.25, 0.25, -1.0]);
    transform_cast(&mut g);
    assert_eq!(g.u8(COLLECTOR + 0x10).unwrap(), 0);
    assert_eq!(g.u32(RESULT + 0x18).unwrap(), f(1.0));
}

#[test]
#[should_panic(expected = "collision: mopp_ray: callback 0x0000dead")]
fn an_unknown_callback_is_named_not_run_in_the_guest() {
    let mut g = guest();
    let call = GuestCall::new(0xdead).ecx(SHAPE);
    let _ = super::call(&mut g, world(), &mut cpu(0x027f), &call);
}
