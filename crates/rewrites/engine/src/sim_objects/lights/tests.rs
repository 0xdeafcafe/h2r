//! The slice 5 bodies on a synthetic light array: entries, sites, the
//! frame each leaves, and the stores retail makes. No image, no game data;
//! the differential harness (`tests/sim_objects_differential.rs`) compares
//! them with the lifted original.

use super::*;
use crate::points;
use crate::sim_objects::{TABLE, drive};
use runtime::TransferAbi;
use runtime::guest::Permissions;

const STACK: u32 = 0x0060_8000;
const CALLER: u32 = 0xdead;
const ARRAY: u32 = 0x0060_0000;
const ELEMENTS: u32 = 0x0060_1000;
const TIME: u32 = 0x0060_6000;
const BLOCK: u32 = 0x0060_7000;

fn guest() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0x004e_0000, 0x4_0000, Permissions::RW).unwrap();
    g.map_zeroed(0x0060_0000, 0x1_0000, Permissions::RW).unwrap();
    // The image constants the bodies read, and the code page the x87
    // state's last-instruction opcode is read from (zero here).
    g.map_zeroed(0x0045_d000, 0x1000, Permissions::RW).unwrap();
    g.map_zeroed(0x000c_0000, 0x1000, Permissions::RW).unwrap();
    g.set_u32(0x0045_dbc0, 1.0f32.to_bits()).unwrap();
    g.set_u32(0x0045_dbb4, (1.0f32 / 255.0).to_bits()).unwrap();
    g.set_u32(LIGHTS, ARRAY).unwrap();
    g.set_u32(ARRAY + ELEMENT_SIZE, STRIDE).unwrap();
    g.set_u32(ARRAY + DATA, ELEMENTS).unwrap();
    g.set_u32(points::GAME_TIME.base, TIME).unwrap();
    g.set_u16(TIME + 2, 30).unwrap();
    g.set_u32(TIME + 4, (1.0f32 / 30.0).to_bits()).unwrap();
    g.set_u32(TIME + 8, 1234).unwrap();
    let mut c = CpuAbi { esp: STACK, mxcsr: 0x1f80, ..Default::default() };
    c.x87.control = 0x037f;
    c.ebx = 0xb0;
    c.ebp = 0xb9;
    c.esi = 0x51;
    c.edi = 0xd1;
    g.set_u32(STACK, CALLER).unwrap();
    (c, g)
}

/// The callee's `ret n`: pop the return address and `n` bytes; the
/// runtime then resumes the body at the site.
fn callee_returns(c: &mut CpuAbi, g: &Guest, n: u32) -> u32 {
    let site = g.u32(c.esp).unwrap();
    c.esp = c.esp.wrapping_add(4 + n);
    site
}

fn saved(c: &CpuAbi) {
    assert_eq!((c.ebx, c.ebp, c.esi, c.edi), (0xb0, 0xb9, 0x51, 0xd1));
}

#[test]
fn slice_five_bodies_own_their_entries() {
    for p in [
        &points::LIGHTS_UPDATE,
        &points::LIGHT_RECORD_UPDATE,
        &points::LIGHT_RECORD_PHASE,
        &points::LIGHT_LINK,
        &points::LIGHT_UNLINK,
        &points::LIGHT_EXPIRE,
    ] {
        assert!(TABLE.owns(p.base), "{}", p.name);
    }
    // The lists themselves stay guest.
    for callout in [0x1cae40, 0x1cac60, 0x16ba40, 0x16bc00, 0x13b390] {
        assert!(TABLE.at(callout).is_none(), "{callout:#x}");
    }
}

#[test]
fn unlink_skips_an_inactive_light_and_removes_an_active_one() {
    let (mut c, mut g) = guest();
    let handle = 0x0007_0002;
    let element = ELEMENTS + 2 * STRIDE;
    g.set_u8(element + 2, 0x08).unwrap();
    c.eax = handle;
    let out = drive(&mut c, &mut g, &TABLE, points::LIGHT_UNLINK.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(c.esp, STACK + 4);
    assert_eq!(g.u8(element + 2).unwrap(), 0x08);
    assert_eq!(c.esi, 0x51);
    // `cl` is the flags byte, over the array pointer.
    assert_eq!(c.ecx, (ARRAY & !0xff) | 0x08);

    let (mut c, mut g) = guest();
    g.set_u8(element + 2, 0x0a).unwrap();
    c.eax = handle;
    let out = drive(&mut c, &mut g, &TABLE, points::LIGHT_UNLINK.base).unwrap();
    assert_eq!(out, TransferAbi::call(0x1cae40, 0xc3253));
    assert_eq!(c.edi, LIGHT_LISTS);
    assert_eq!(g.u32(c.esp + 4).unwrap(), handle);
    assert_eq!(g.u32(c.esp + 8).unwrap(), element + 0x10);
    let site = callee_returns(&mut c, &g, 8);
    let out = drive(&mut c, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(g.u8(element + 2).unwrap(), 0x02);
    assert_eq!(c.esp, STACK + 4);
    saved(&c);
}

#[test]
fn expire_without_a_parent_frees_the_light() {
    let (mut c, mut g) = guest();
    let handle = 0x0003_0001;
    let element = ELEMENTS + STRIDE;
    g.set_u8(element + 2, 0x0a).unwrap();
    g.set_u32(element + 0x58, u32::MAX).unwrap();
    g.set_u32(STACK + 4, handle).unwrap();
    g.set_u32(STACK + 8, 1).unwrap();
    // Linked and active: removed from the lists first.
    let out = drive(&mut c, &mut g, &TABLE, points::LIGHT_EXPIRE.base).unwrap();
    assert_eq!(out, TransferAbi::call(0x1cae40, 0xc329b));
    let site = callee_returns(&mut c, &g, 8);
    let out = drive(&mut c, &mut g, &TABLE, site).unwrap();
    assert_eq!(g.u8(element + 2).unwrap(), 0x02);
    assert_eq!(out, TransferAbi::call(0x16ba40, 0xc332d));
    assert_eq!((c.eax, c.edx), (handle, ARRAY));
    let site = callee_returns(&mut c, &g, 0);
    let out = drive(&mut c, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(c.esp, STACK + 12);
    saved(&c);
}

#[test]
fn phase_with_empty_blocks_stores_the_phase_and_returns() {
    let (mut c, mut g) = guest();
    // Block flags bit 0: the phase is game time × seconds per tick.
    g.set_u32(BLOCK, 1).unwrap();
    g.set_u32(STACK + 4, 0x0001_0005).unwrap();
    c.eax = ELEMENTS + 0xd4;
    c.ecx = BLOCK;
    let out = drive(&mut c, &mut g, &TABLE, points::LIGHT_RECORD_PHASE.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(c.esp, STACK + 4 + 0x10);
    let phase = 1234.0f32 * (1.0f32 / 30.0);
    assert_eq!(g.u32(STACK + 4).unwrap(), phase.to_bits());
    assert_eq!(c.xmm[0][..4], phase.to_bits().to_le_bytes());
    // `eax` is the last count read (`+14`), `ecx` the block it came in.
    assert_eq!((c.eax, c.ecx), (0, BLOCK));
    saved(&c);
}

#[test]
fn phase_per_light_offset_divides_by_a_minute_of_ticks() {
    let (mut c, mut g) = guest();
    g.set_u32(STACK + 4, 0x0001_0005).unwrap();
    c.eax = ELEMENTS + 0xd4;
    c.ecx = BLOCK;
    drive(&mut c, &mut g, &TABLE, points::LIGHT_RECORD_PHASE.base).unwrap();
    let n = (0x0001_0005u32.wrapping_mul(0x1387) & 0x7fff_ffff) as i32;
    let ticks = (n % (30 * 0x3c)) + 1234;
    let phase = ticks as f32 * (1.0f32 / 30.0);
    assert_eq!(g.u32(STACK + 4).unwrap(), phase.to_bits());
    assert_eq!(c.edx, ticks as u32);

    // No ticks per second: the guest's divide error, as a fault at the
    // `idiv`, with nothing popped.
    let (mut c, mut g) = guest();
    g.set_u16(TIME + 2, 0).unwrap();
    c.ecx = BLOCK;
    let fault = drive(&mut c, &mut g, &TABLE, points::LIGHT_RECORD_PHASE.base).unwrap_err();
    assert_eq!(fault.addr, 0xc0b3b);
}

#[test]
fn phase_colour_scales_each_channel_by_its_byte() {
    let (mut c, mut g) = guest();
    g.set_u32(BLOCK, 1).unwrap();
    g.set_u32(BLOCK + 0xc, 1).unwrap();
    g.set_u32(BLOCK + 0x10, 0x00ab_cdef).unwrap();
    let rgb = ELEMENTS + 0xd4;
    for i in 0..3 {
        g.set_u32(rgb + 4 * i, 0.5f32.to_bits()).unwrap();
    }
    c.eax = rgb;
    c.ecx = BLOCK;
    let out = drive(&mut c, &mut g, &TABLE, points::LIGHT_RECORD_PHASE.base).unwrap();
    assert_eq!(out, TransferAbi::call(0x13b390, 0xc0bcb));
    let site = callee_returns(&mut c, &g, 8);
    let out = drive(&mut c, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::call(0x13bc00, 0xc0bd8));
    assert_eq!(c.ecx, 0x00ab_cdef);
    let site = callee_returns(&mut c, &g, 4);
    c.eax = 0x0012_3456;
    let x87_before = c.x87.status;
    let out = drive(&mut c, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    // The three bytes, high first, over the input's argument slot; the
    // last one stays there. `eax` is the last count read (`+14`).
    assert_eq!((c.edx, c.ecx, c.eax), (0x12, 0x34, 0));
    assert_eq!(g.u32(STACK + 8).unwrap(), 0x56);
    // The x87 stack is balanced and its last instruction is `c0c48`.
    assert_eq!(c.x87.status & 0x3800, x87_before & 0x3800);
    assert_eq!(c.x87.instruction_pointer, 0xc0c48);
    assert_eq!(c.x87.data_pointer, rgb + 8);
}

#[test]
fn walk_with_no_lights_calls_the_iterator_once_and_returns() {
    let (mut c, mut g) = guest();
    let out = drive(&mut c, &mut g, &TABLE, points::LIGHTS_UPDATE.base).unwrap();
    assert_eq!(out, TransferAbi::call(0x16bc00, 0xc1420));
    assert_eq!((c.ecx, c.edx), (ARRAY, 0));
    // The tick, in the frame.
    assert_eq!(g.u32(c.esp + 4 + 0xc).unwrap(), 1234);
    let site = callee_returns(&mut c, &g, 0);
    c.eax = u32::MAX;
    let out = drive(&mut c, &mut g, &TABLE, site).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(c.esp, STACK + 4);
    assert_eq!(c.ebx, 0xb0);
    assert_eq!(c.esi, 0x51);
}
