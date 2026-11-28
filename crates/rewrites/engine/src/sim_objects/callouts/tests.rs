//! The walks' callouts on a synthetic pool: each body's first transfers
//! and its return, with retail's registers. No image needed. A call is
//! returned from by hand: the body is entered at the site with `esp`
//! past the arguments the callee pops.

use super::*;
use runtime::guest::Permissions;

const STACK: u32 = 0x0057_0800;
const ARRAY: u32 = 0x0057_1000;
const HEADERS: u32 = 0x0057_2000;
const BODIES: u32 = 0x0057_4000;
const DESCRIPTOR: u32 = 0x0057_6000;
const CALLER: u32 = 0xdead_0000;

/// Slot 3 (salt `8003`) live, of type `kind`, its body at `BODIES`.
const OBJECT: u32 = 0x8003_0003;

fn guest(kind: u8) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0x0044_0000, 0x14_0000, Permissions::RW).unwrap();
    g.set_u32(points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + DATA, HEADERS).unwrap();
    let header = HEADERS + 12 * 3;
    g.set_u16(header, 0x8003).unwrap();
    g.set_u8(header + 2, 0x01).unwrap();
    g.set_u8(header + 3, kind).unwrap();
    g.set_u32(header + 8, BODIES).unwrap();
    for link in [0x0c, 0x10, 0x14] {
        g.set_u32(BODIES + link, NONE).unwrap();
    }
    g.set_u32(STACK, CALLER).unwrap();
    let cpu = CpuAbi { esp: STACK, ebx: 0xb0b0, ebp: 0xb9b9, esi: 0x5151, edi: 0xd1d1, ..Default::default() };
    (cpu, g)
}

fn run(f: fn(&mut Cx<'_>, u32) -> Result<Next>, cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> Next {
    f(&mut Cx { cpu, g }, pc).unwrap()
}

/// Back from a callee that pops `pop` argument bytes.
fn back(cpu: &mut CpuAbi, pop: u32) {
    cpu.esp += pop;
}

fn saved(cpu: &CpuAbi) {
    assert_eq!((cpu.esp, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (STACK, 0xb0b0, 0xb9b9, 0x5151, 0xd1d1));
}

#[test]
fn a_replicated_transform_does_nothing_without_a_pending_transform_and_defers_an_attached_one() {
    let (mut cpu, mut g) = guest(0);
    let entry = points::OBJECT_REPLICATED_TRANSFORM.base;
    cpu.eax = OBJECT;
    assert_eq!(run(replicated_transform, &mut cpu, &mut g, entry), Next::Ret { pop: 0 });
    saved(&cpu);
    assert_eq!(cpu.eax, 9, "the header index");
    // Pending and attached: `a9640` with `esi` the body, `edi` the handle.
    g.set_u8(BODIES + 0xd8, 1).unwrap();
    g.set_u32(BODIES + 0x14, 0x8001_0001).unwrap();
    cpu.eax = OBJECT;
    assert_eq!(run(replicated_transform, &mut cpu, &mut g, entry), Next::Call { target: 0xa9640, site: 0xa95a4 });
    assert_eq!((cpu.esi, cpu.edi, cpu.esp), (BODIES, OBJECT, STACK - 0x30));
    assert_eq!(run(replicated_transform, &mut cpu, &mut g, 0xa95a4), Next::Ret { pop: 0 });
    saved(&cpu);
    // Pending, not attached, and the record comes back empty: bit 0 of
    // `+d8` is cleared.
    g.set_u32(BODIES + 0x14, NONE).unwrap();
    cpu.eax = OBJECT;
    assert_eq!(run(replicated_transform, &mut cpu, &mut g, entry), Next::Call { target: 0xa9820, site: 0xa95e7 });
    assert_eq!(cpu.ecx, OBJECT);
    assert_eq!(g.u32(cpu.esp).unwrap(), STACK - 0x30 + 8, "the record's address");
    back(&mut cpu, 4);
    assert_eq!(run(replicated_transform, &mut cpu, &mut g, 0xa95e7), Next::Ret { pop: 0 });
    assert_eq!(g.u8(BODIES + 0xd8).unwrap(), 0);
    saved(&cpu);
}

#[test]
fn a_type_new_calls_each_ancestor_slot_until_one_answers_no() {
    let (mut cpu, mut g) = guest(2);
    g.set_u8(BODIES + 0xaa, 2).unwrap();
    g.set_u32(DESCRIPTORS + 8, DESCRIPTOR).unwrap();
    // Ancestry: two types whose slot `+2c` is set, then the end.
    for (i, slot) in [0x0011_1000u32, 0x0022_2000].into_iter().enumerate() {
        let kind = DESCRIPTOR + 0x100 * (i as u32 + 1);
        g.set_u32(DESCRIPTOR + 0x84 + 4 * i as u32, kind).unwrap();
        g.set_u32(kind + 0x2c, slot).unwrap();
    }
    g.set_u32(STACK + 4, 0xaaaa).unwrap();
    g.set_u32(STACK + 8, 0xbbbb).unwrap();
    cpu.ebx = OBJECT;
    let entry = points::OBJECT_TYPE_NEW.base;
    assert_eq!(run(type_new, &mut cpu, &mut g, entry), Next::Call { target: 0x0011_1000, site: 0x108ae6 });
    let args: Vec<u32> = (0..3).map(|i| g.u32(cpu.esp + 4 * i).unwrap()).collect();
    assert_eq!(args, [OBJECT, 0xaaaa, 0xbbbb]);
    back(&mut cpu, 12);
    cpu.eax = 1;
    assert_eq!(run(type_new, &mut cpu, &mut g, 0x108ae6), Next::Call { target: 0x0022_2000, site: 0x108ae6 });
    back(&mut cpu, 12);
    cpu.eax = 0x0100;
    assert_eq!(run(type_new, &mut cpu, &mut g, 0x108ae6), Next::Ret { pop: 8 });
    assert_eq!(cpu.eax & 0xff, 0);
    assert_eq!((cpu.esp, cpu.edi, cpu.esi, cpu.ebp), (STACK, 0xd1d1, 0x5151, 0xb9b9));
}

#[test]
fn the_deferred_fire_steps_the_weapons_and_fires_each_flagged_one() {
    let (mut cpu, mut g) = guest(2);
    let entry = points::WEAPONS_DEFERRED_FIRE.base;
    assert_eq!(run(deferred_fire, &mut cpu, &mut g, entry), Next::Call { target: 0xbaeb0, site: 0xfd941 });
    let iterator = STACK - 0x14 + 4;
    assert_eq!(g.u32(cpu.esp).unwrap(), iterator);
    assert_eq!(g.u32(iterator).unwrap(), 4);
    assert_eq!(g.u8(iterator + 4).unwrap(), 2);
    assert_eq!(g.u32(iterator + 8).unwrap(), NONE);
    assert_eq!(g.u32(iterator + 12).unwrap(), 0x8686_8686);
    // The iterator answers a weapon with a deferred shot.
    back(&mut cpu, 4);
    g.set_u32(iterator + 8, OBJECT).unwrap();
    g.set_u8(BODIES + 0x16c, 1).unwrap();
    cpu.eax = BODIES;
    assert_eq!(run(deferred_fire, &mut cpu, &mut g, 0xfd941), Next::Call { target: 0xfd980, site: 0xfd963 });
    assert_eq!(g.u32(cpu.esp).unwrap(), OBJECT);
    back(&mut cpu, 4);
    assert_eq!(run(deferred_fire, &mut cpu, &mut g, 0xfd963), Next::Call { target: 0xbaeb0, site: 0xfd96d });
    back(&mut cpu, 4);
    cpu.eax = 0;
    assert_eq!(run(deferred_fire, &mut cpu, &mut g, 0xfd96d), Next::Ret { pop: 0 });
    assert_eq!(cpu.esp, STACK);
}

#[test]
fn a_delete_marks_the_header_and_clears_the_body_links() {
    let (mut cpu, mut g) = guest(2);
    g.set_u32(STACK + 4, OBJECT).unwrap();
    g.set_u8(BODIES + 0xaf, 0xff).unwrap();
    let entry = points::OBJECT_DELETE.base;
    assert_eq!(run(delete, &mut cpu, &mut g, entry), Next::Call { target: 0xb9ce0, site: 0xb8568 });
    assert_eq!((cpu.ecx, cpu.esi, cpu.ebp, cpu.edi), (OBJECT, OBJECT, BODIES, HEADERS + 36));
    // `b9ce0` says yes: no report, straight to DELETE.
    cpu.eax = 1;
    assert_eq!(run(delete, &mut cpu, &mut g, 0xb8568), Next::Call { target: 0xb8b70, site: 0xb858e });
    assert_eq!(g.u8(HEADERS + 36 + 2).unwrap(), 0x11);
    // Type 2 has no `10a250`.
    assert_eq!(run(delete, &mut cpu, &mut g, 0xb858e), Next::Call { target: 0xbf090, site: 0xb85a9 });
    assert_eq!(cpu.edi, OBJECT);
    assert_eq!(run(delete, &mut cpu, &mut g, 0xb85a9), Next::Call { target: 0x109400, site: 0xb85de });
    assert_eq!((g.u32(BODIES + 0xa4).unwrap(), g.u16(BODIES + 0xa8).unwrap(), g.u8(BODIES + 0xab).unwrap()), (NONE, 0xffff, 0xff));
    back(&mut cpu, 4);
    assert_eq!(run(delete, &mut cpu, &mut g, 0xb85de), Next::Call { target: 0xbb950, site: 0xb85e9 });
    assert_eq!((g.u32(cpu.esp).unwrap(), g.u32(cpu.esp + 4).unwrap(), cpu.eax), (0, NONE, OBJECT));
    back(&mut cpu, 8);
    assert_eq!(run(delete, &mut cpu, &mut g, 0xb85e9), Next::Call { target: 0xa7a60, site: 0xb85f0 });
    assert_eq!(run(delete, &mut cpu, &mut g, 0xb85f0), Next::Call { target: 0x10ace0, site: 0xb85f7 });
    assert_eq!(cpu.edx, OBJECT);
    cpu.edi = HEADERS + 36;
    let top = cpu.esp;
    assert_eq!(run(delete, &mut cpu, &mut g, 0xb85f7), Next::Ret { pop: 4 });
    assert_eq!(cpu.esp, top + 12);
    assert_eq!((cpu.esp, cpu.ebp, cpu.esi, cpu.edi), (STACK, 0xb9b9, 0x5151, 0xd1d1));
}

#[test]
fn a_delete_tree_runs_the_delete_and_the_eight_callbacks_then_its_children() {
    let (mut cpu, mut g) = guest(2);
    for i in 0..8 {
        g.set_u32(DELETE_CALLBACKS + 4 * i, 0x0030_0000 + i).unwrap();
    }
    g.set_u32(STACK + 4, OBJECT).unwrap();
    g.set_u32(STACK + 8, 1).unwrap();
    let entry = points::OBJECT_DELETE_TREE.base;
    let first = run(delete_tree, &mut cpu, &mut g, entry);
    assert_eq!(first, Next::Call { target: points::OBJECT_DELETE.base, site: 0xb83d4 });
    assert_eq!((g.u32(cpu.esp).unwrap(), cpu.ebx), (OBJECT, BODIES));
    back(&mut cpu, 4);
    let mut site = 0xb83d4;
    for i in 0..8 {
        assert_eq!(run(delete_tree, &mut cpu, &mut g, site), Next::Call { target: 0x0030_0000 + i, site: 0xb83dd });
        assert_eq!(g.u32(cpu.esp).unwrap(), OBJECT);
        back(&mut cpu, 4);
        site = 0xb83dd;
    }
    // No children.
    assert_eq!(run(delete_tree, &mut cpu, &mut g, site), Next::Ret { pop: 8 });
    assert_eq!((cpu.esp, cpu.ebx, cpu.esi, cpu.edi), (STACK, 0xb0b0, 0x5151, 0xd1d1));
}

#[test]
fn a_free_tree_of_a_child_skips_the_top_steps() {
    let (mut cpu, mut g) = guest(2);
    g.set_u32(STACK + 4, OBJECT).unwrap();
    g.set_u32(STACK + 8, 0).unwrap();
    let entry = points::OBJECT_FREE_TREE.base;
    assert_eq!(run(free_tree, &mut cpu, &mut g, entry), Next::Call { target: 0xd4a60, site: 0xb84b5 });
    assert_eq!((cpu.eax, cpu.ebp, cpu.esi, cpu.edi), (OBJECT, OBJECT, BODIES, 36));
    assert_eq!(run(free_tree, &mut cpu, &mut g, 0xb84b5), Next::Call { target: 0xbee60, site: 0xb84bc });
    assert_eq!(cpu.ebx, OBJECT);
    // No children; `+c0` bit 6 clear.
    assert_eq!(run(free_tree, &mut cpu, &mut g, 0xb84bc), Next::Call { target: 0x1c37f0, site: 0xb8514 });
    assert_eq!(run(free_tree, &mut cpu, &mut g, 0xb8514), Next::Call { target: 0x108bf0, site: 0xb8529 });
    assert_eq!(run(free_tree, &mut cpu, &mut g, 0xb8529), Next::Call { target: 0xbc300, site: 0xb852f });
    assert_eq!(g.u32(cpu.esp).unwrap(), OBJECT);
    back(&mut cpu, 4);
    assert_eq!(run(free_tree, &mut cpu, &mut g, 0xb852f), Next::Ret { pop: 8 });
    assert_eq!((cpu.esp, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (STACK, 0xb0b0, 0xb9b9, 0x5151, 0xd1d1));
}

#[test]
fn the_collector_asks_each_pass_in_the_requested_mode() {
    let (mut cpu, mut g) = guest(2);
    let state = 0x0057_7000;
    g.set_u32(PHASE_STATE, state).unwrap();
    g.set_u8(state + 1, 1).unwrap();
    // The first pass: types 2, ungated, no `pre`, `main` set.
    g.set_u32(COLLECT_TABLE - 8, 2).unwrap();
    g.set_u32(COLLECT_TABLE + 4, 0x0040_1000).unwrap();
    let entry = points::OBJECTS_COLLECT.base;
    assert_eq!(run(collect, &mut cpu, &mut g, entry), Next::Call { target: STACK_PROBE, site: 0xbf38a });
    assert_eq!(cpu.eax, COLLECT_FRAME);
    cpu.esp -= COLLECT_FRAME;
    assert_eq!(run(collect, &mut cpu, &mut g, 0xbf38a), Next::Call { target: 0xbf1c0, site: 0xbf449 });
    let frame = STACK - COLLECT_FRAME - 16;
    assert_eq!((cpu.eax, cpu.ebp, cpu.esi, cpu.edi), (1, 1, frame + 0x13, COLLECT_TABLE));
    cpu.eax = 1;
    assert_eq!(run(collect, &mut cpu, &mut g, 0xbf449), Next::Call { target: 0x0040_1000, site: 0xbf4b1 });
    let args: Vec<u32> = (0..6).map(|i| g.u32(cpu.esp + 4 * i).unwrap()).collect();
    assert_eq!(args, [1, frame + 0x214, 0x2800, frame + 0x12, frame + 0x14, 0x200]);
    // `main` sets the done byte: the same pass asks `bf1c0` again, which
    // says no.
    g.set_u8(frame + 0x12, 1).unwrap();
    back(&mut cpu, 24);
    assert_eq!(run(collect, &mut cpu, &mut g, 0xbf4b1), Next::Call { target: 0xbf1c0, site: 0xbf449 });
    cpu.eax = 0;
    assert_eq!(run(collect, &mut cpu, &mut g, 0xbf449), Next::Ret { pop: 0 });
    assert_eq!((cpu.esp, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (STACK, 0xb0b0, 0xb9b9, 0x5151, 0xd1d1));
    assert_eq!((g.u8(state + 1).unwrap(), g.u8(state + 2).unwrap()), (0, 0));
}

#[test]
fn a_spatial_visit_outside_every_cluster_finds_nothing() {
    let (mut cpu, mut g) = guest(2);
    let location = 0x0057_7100;
    g.set_u16(location + 4, 0xffff).unwrap();
    for (i, arg) in [3u32, 0x3f, location, location + 0x10, 0x3f80_0000, 0x0057_7200, 16].into_iter().enumerate() {
        g.set_u32(STACK + 4 + 4 * i as u32, arg).unwrap();
    }
    cpu.mxcsr = 0x1f80;
    let entry = points::OBJECT_SPATIAL_VISIT.base;
    assert_eq!(run(spatial_visit, &mut cpu, &mut g, entry), Next::Ret { pop: 28 });
    assert_eq!(cpu.eax & 0xffff, 0);
    assert_eq!((cpu.esp, cpu.ebx, cpu.ebp, cpu.esi, cpu.edi), (STACK, 0xb0b0, 0xb9b9, 0x5151, 0xd1d1));
}
