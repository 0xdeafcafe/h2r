use super::*;
use runtime::guest::Permissions;

const BASE: u32 = 0x0060_0000;
const NODES: u32 = BASE + 0x400;
const THREAD: u32 = BASE + 0x700;
const PARENT: u32 = THREAD + STACK_OFFSET;
const SCENARIO: u32 = BASE + 0x1000;
const DEFS: u32 = BASE + 0x1300;
const GLOBALS: u32 = BASE + 0x1600;
const DEST: u32 = BASE + 0x4000;
const STACK: u32 = BASE + 0x4900;
const EXPR: Datum = Datum(0xabc1_0002);
const STOP: u32 = 0xee11_2200;

fn cpu() -> CpuAbi {
    CpuAbi {
        edx: 0xbeef_0000,
        ebx: DEST,
        esp: STACK,
        eip: points::HS_EVALUATE.base,
        ..CpuAbi::default()
    }
}

fn fixture() -> Guest {
    let mut g = Guest::new();
    for (base, size) in [
        (points::SCENARIO.base & !0xfff, 0x1000),
        (hs_pool::THREADS & !0xfff, 0x1000),
        (BASE, 0x5000),
    ] {
        g.map_zeroed(base, size, Permissions::RW).unwrap();
    }
    for (pointer, header, data, count) in [
        (hs_syntax::NODES, BASE, NODES, 8),
        (hs_pool::THREADS, BASE + 0x100, THREAD, 1),
        (
            hs_pool::GLOBALS,
            BASE + 0x200,
            GLOBALS,
            hs_pool::EXTERNALS + 3,
        ),
    ] {
        g.set_u32(pointer, header).unwrap();
        g.set_u32(header + layout::DATA, data).unwrap();
        g.set_u32(header + layout::MAXIMUM_COUNT, count).unwrap();
    }
    for (p, v) in [
        (STACK, STOP),
        (STACK + 4, EXPR.0),
        (THREAD + 0x10, PARENT),
        (points::SCENARIO.base, SCENARIO),
        (SCENARIO + 0x1c0, 3),
        (SCENARIO + 0x1c4, DEFS),
        (DEST, 0x1234_5678),
        (GLOBALS + (hs_pool::EXTERNALS + 1) * 8 + 4, 0xfffd),
    ] {
        g.set_u32(p, v).unwrap();
    }
    g.set_u16(DEFS + Global::SIZE as u32 + 0x20, HsType::SHORT.0)
        .unwrap();
    g.set_u8(THREAD + 3, 0xb4).unwrap();
    g.set_u16(PARENT + 0xc, 7).unwrap();
    g
}

fn node(g: &mut Guest, flags: u16, from: HsType, to: HsType, data: u32) {
    let at = NODES + EXPR.slot() as u32 * Node::SIZE as u32;
    for (off, v) in [(0, EXPR.salt()), (2, from.0), (4, to.0), (6, flags)] {
        g.set_u16(at + off, v).unwrap();
    }
    g.set_u32(at + 16, data).unwrap();
}

#[test]
fn atom_parity_reports_a_bad_slot_and_declines_engine_work() {
    let mut g = fixture();
    let c = cpu();
    node(&mut g, 1, HsType::REAL, HsType::SHORT, (-2.5f32).to_bits());
    let prediction = plan(&c, &g).unwrap();
    assert!(compare(&prediction, &c, &g).unwrap().contains("value"));
    // A real-to-short conversion preserves the source cell's high half.
    g.set_u32(DEST, 0xc020_fffe).unwrap();
    assert_eq!(compare(&prediction, &c, &g), None);

    node(&mut g, 5, HsType::REAL, HsType::LONG, 1);
    let prediction = plan(&c, &g).unwrap();
    g.set_u32(DEST, (-3i32) as u32).unwrap();
    assert_eq!(compare(&prediction, &c, &g), None);
    node(&mut g, 5, HsType::LONG, HsType::LONG, 0x8001);
    assert!(plan(&c, &g).is_none());
    node(&mut g, 1, HsType::AI, HsType::OBJECT_LIST, 1);
    assert!(plan(&c, &g).is_none());
}

#[test]
fn a_call_hands_its_destination_to_the_parent_and_starts_its_child_once() {
    let mut g = fixture();
    let c = cpu();
    node(&mut g, 2, HsType(1), HsType::VOID, 0);
    let prediction = plan(&c, &g).unwrap();
    let child = PARENT + 0x17;
    for (p, v) in [
        (PARENT + 8, DEST),
        (THREAD + 0x10, child),
        (child, PARENT),
        (child + 4, EXPR.0),
    ] {
        g.set_u32(p, v).unwrap();
    }
    g.set_u8(THREAD + 3, 0xb5).unwrap();
    g.set_u16(child + 0xc, 0).unwrap();
    assert_eq!(compare(&prediction, &c, &g), None);
    g.set_u32(child + 4, EXPR.0 & 0xffff).unwrap();
    assert!(
        compare(&prediction, &c, &g)
            .unwrap()
            .contains("child expression")
    );
    g.set_u32(THREAD + 0x10, PARENT).unwrap();
    g.set_u16(PARENT + 0xc, 0x7fff).unwrap();
    assert!(plan(&c, &g).is_none());
}

/// The lifted evaluator and cast-table initializer come from the owner's
/// image. Every expression, definition, slot and frame comes from fixture().
#[test]
#[ignore = "requires H2_XBE"]
fn lifted_evaluator_matches_synthetic_calls_literals_and_globals() {
    let image = lifted::image::Image::require();
    let mut code = image.original();
    let mut ram = image.ram.clone();
    let mut g = fixture();
    assert!(ram.load(STACK, &STOP.to_le_bytes()));
    let mut init = lifted::cpu::to_interp(&CpuAbi {
        eip: 0x0020_a1a0,
        esp: STACK,
        ..cpu()
    });
    lifted::code::run_until(&mut code, &mut init, &mut ram, 10_000, |c| c.eip == STOP).unwrap();

    for (flags, from, to, data) in [
        (0, HsType(6), HsType::REAL, 0),
        (2, HsType(0), HsType::VOID, 0),
        (1, HsType::SHORT, HsType::LONG, 0x8999_fffe),
        (1, HsType::LONG, HsType::BOOLEAN, 0x1200),
        (1, HsType::REAL, HsType::SHORT, f32::NAN.to_bits()),
        (1, HsType::STRING, HsType::BOOLEAN, BASE + 0x4100),
        (5, HsType::REAL, HsType::LONG, 1),
    ] {
        node(&mut g, flags, from, to, data);
        let prediction = plan(&cpu(), &g).unwrap();
        for m in g.mappings() {
            let mut bytes = vec![0; m.len as usize];
            g.read_bytes(m.virtual_start, &mut bytes, 1).unwrap();
            assert!(ram.load(m.virtual_start, &bytes));
        }
        let mut c = lifted::cpu::to_interp(&cpu());
        lifted::code::run_until(&mut code, &mut c, &mut ram, 1_000, |c| c.eip == STOP).unwrap();
        let end = BASE as usize + 0x5000;
        g.write_bytes(BASE, &ram.bytes[BASE as usize..end], 1)
            .unwrap();
        let after = lifted::cpu::from_interp(&c);
        assert_eq!(after.esp, STACK + 8);
        assert_eq!(
            compare(&prediction, &after, &g),
            None,
            "flags={flags}, {from:?} to {to:?}"
        );
    }
}
