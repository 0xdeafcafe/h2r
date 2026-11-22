//! The slice 3 helpers and the import dispatcher `108d90` on synthetic
//! memory: SSE lanes as the instructions leave them, the `lahf` compare,
//! and the dispatcher's zeroing, slot call and return. No image needed;
//! the bodies against the lifted original are in
//! `tests/sim_objects_differential.rs`.

use super::*;
use crate::sim_objects::{Cx, Next, TABLE};
use runtime::guest::Permissions;

#[test]
fn sse_helpers_keep_or_clear_the_upper_lanes_as_the_instructions_do() {
    let mut c = CpuAbi {
        mxcsr: 0x1f80,
        ..Default::default()
    };
    c.xmm[1] = (u128::MAX).to_le_bytes();
    // `movss xmm1, m32` clears lanes 1 to 3.
    load(&mut c, 1, 2.0f32.to_bits());
    assert_eq!(u128::from_le_bytes(c.xmm[1]), u128::from(2.0f32.to_bits()));
    // `mulss` keeps the destination's upper lanes.
    c.xmm[2] = ((7u128 << 96) | u128::from(3.0f32.to_bits())).to_le_bytes();
    let r = ss(&mut c, Ss::Mul, 2, 2.0f32.to_bits()).unwrap();
    assert_eq!(r, 6.0f32.to_bits());
    assert_eq!(u128::from_le_bytes(c.xmm[2]) >> 96, 7);
    // `movss xmm2, xmm1` takes the low lane only; `movaps` all four.
    movss(&mut c, 2, 1);
    assert_eq!(lo(&c, 2), 2.0f32.to_bits());
    assert_eq!(u128::from_le_bytes(c.xmm[2]) >> 96, 7);
    movaps(&mut c, 2, 1);
    assert_eq!(c.xmm[2], c.xmm[1]);
    zero(&mut c, 2);
    assert_eq!(u128::from_le_bytes(c.xmm[2]), 0);
}

#[test]
fn comparisons_follow_the_unordered_rules() {
    let mut c = CpuAbi { mxcsr: 0x1f80, ..Default::default() };
    let (one, two, nan) = (1.0f32.to_bits(), 2.0f32.to_bits(), f32::NAN.to_bits());
    assert!(above(&mut c, two, one).unwrap());
    assert!(!above(&mut c, one, two).unwrap());
    assert!(!above(&mut c, one, one).unwrap());
    // Unordered: `ja` is not taken, `jbe` is.
    assert!(!above(&mut c, nan, one).unwrap());
    assert!(below_or_equal(&mut c, nan, one).unwrap());
    // `ucomiss; lahf; test ah, 44h; jp`: taken unless ordered and equal.
    assert!(!ucomiss_lahf_jp(&mut c, one, one).unwrap());
    assert_eq!((c.eax >> 8) as u8 & 0x44, 0x40);
    assert!(ucomiss_lahf_jp(&mut c, one, two).unwrap());
    assert!(ucomiss_lahf_jp(&mut c, nan, one).unwrap());
}

/// The object array, one object of type `kind` in slot 3, and the type
/// table entry for `kind` pointing at a descriptor whose ancestry is
/// `levels` (each a level descriptor with its slot `+4c`).
const ARRAY: u32 = 0x0050_0000;
const DATA: u32 = 0x0050_1000;
const BODY: u32 = 0x0050_2000;
const DESCRIPTOR: u32 = 0x0050_3000;
const LEVELS: u32 = 0x0050_4000;
const OUT: u32 = 0x0050_5000;
const STACK: u32 = 0x0050_8000;
const CALLER: u32 = 0xdead;
const HANDLE: u32 = 0x8001_0003;
const NAME: u32 = 0x0abc_0001;

fn guest(slots: &[u32]) -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0x0046_0000, 0x0001_0000, Permissions::RW).unwrap();
    g.map_zeroed(0x004e_0000, 0x0001_0000, Permissions::RW).unwrap();
    g.map_zeroed(ARRAY, 0x0001_0000, Permissions::RW).unwrap();
    g.set_u32(OBJECTS, ARRAY).unwrap();
    g.set_u32(ARRAY + layout::DATA, DATA).unwrap();
    g.set_u32(DATA + 3 * 12 + 8, BODY).unwrap();
    g.set_u8(BODY + 0xaa, 1).unwrap();
    g.set_u32(TYPES + 4, DESCRIPTOR).unwrap();
    for (i, &slot) in slots.iter().enumerate() {
        let level = LEVELS + 0x100 * i as u32;
        g.set_u32(DESCRIPTOR + 0x84 + 4 * i as u32, level).unwrap();
        g.set_u32(level + 0x4c, slot).unwrap();
    }
    g.set_u32(OUT, 0xffff_ffff).unwrap();
    g.set_u32(OUT + 0x10, 0xffff_ffff).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(STACK + 4, HANDLE).unwrap();
    g.set_u32(STACK + 8, NAME).unwrap();
    let c = CpuAbi {
        esp: STACK,
        ebx: OUT,
        edi: OUT + 0x10,
        esi: 0x5151,
        ebp: 0x7b1c,
        eax: 0x1234_5678,
        mxcsr: 0x1f80,
        ..Default::default()
    };
    (c, g)
}

fn run(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Next {
    value::import(&mut Cx { cpu: c, g }, pc).unwrap()
}

#[test]
fn import_with_no_handler_zeroes_both_outputs_and_restores_its_registers() {
    // Two levels, neither with a slot.
    let (mut c, mut g) = guest(&[0, 0]);
    let next = run(&mut c, &mut g, points::OBJECT_FUNCTION_IMPORT.base);
    assert_eq!(next, Next::Ret { pop: 8 });
    assert_eq!(g.u32(OUT).unwrap(), 0);
    assert_eq!(g.u8(OUT + 0x10).unwrap(), 0);
    assert_eq!((c.esp, c.esi, c.ebp), (STACK, 0x5151, 0x7b1c));
    // `xor al, al` on the body pointer; `ecx` the terminating null.
    assert_eq!(c.eax, BODY & !0xff);
    assert_eq!(c.ecx, 0);
    assert_eq!(c.eflags_defined & crate::sim_physics::ARITH, 0);
}

#[test]
fn import_calls_each_slot_until_one_answers() {
    const FIRST: u32 = 0x0011_0000;
    const SECOND: u32 = 0x0012_0000;
    let (mut c, mut g) = guest(&[0, FIRST, SECOND]);
    let next = run(&mut c, &mut g, points::OBJECT_FUNCTION_IMPORT.base);
    assert_eq!(next, Next::Call { target: FIRST, site: value::IMPORT_SITES[0] });
    // `push edi, ebx, name, object` over the two saved registers.
    assert_eq!(c.esp, STACK - 8 - 16);
    for (i, want) in [HANDLE, NAME, OUT, OUT + 0x10].into_iter().enumerate() {
        assert_eq!(g.u32(c.esp + 4 * i as u32).unwrap(), want);
    }
    assert_eq!((c.ecx, c.edx, c.ebp), (FIRST, NAME, HANDLE));
    // The first handler answers zero (`ret 10` pops the arguments): the
    // next level's slot is called.
    c.esp += 16;
    c.eax = 0x0abc_de00;
    let next = run(&mut c, &mut g, value::IMPORT_SITES[0]);
    assert_eq!(next, Next::Call { target: SECOND, site: value::IMPORT_SITES[0] });
    c.esp += 16;
    c.eax = 1;
    let next = run(&mut c, &mut g, value::IMPORT_SITES[0]);
    assert_eq!(next, Next::Ret { pop: 8 });
    assert_eq!((c.esp, c.esi, c.ebp, c.eax), (STACK, 0x5151, 0x7b1c, 1));
}

#[test]
fn every_slice_three_body_is_in_the_table() {
    for point in [
        &points::OBJECT_FUNCTION_VALUE,
        &points::OBJECT_FUNCTION_EVALUATE,
        &points::OBJECT_FUNCTION_IMPORT,
        &points::BASE_OBJECT_IMPORT,
        &points::OBJECT_CHANGE_COLORS,
        &points::UNIT_FUNCTION_IMPORT,
        &points::UNIT_IMPORT_FRACTION,
        &points::UNIT_IMPORT_ANIMATION,
        &points::BIPED_FUNCTION_IMPORT,
        &points::VEHICLE_FUNCTION_IMPORT,
        &points::WEAPON_FUNCTION_IMPORT,
        &points::PROJECTILE_FUNCTION_IMPORT,
        &points::DEVICE_FUNCTION_IMPORT,
        &points::MACHINE_FUNCTION_IMPORT,
    ] {
        assert!(TABLE.owns(point.base), "{}", point.name);
    }
}
