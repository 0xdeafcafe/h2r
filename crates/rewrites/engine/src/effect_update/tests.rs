//! The per-effect bodies on a synthetic guest: the early ways out, the
//! calls they make with retail's registers and frame, and the resumes at
//! their sites. No image needed.

use super::*;
use runtime::guest::Permissions;
use runtime::{CpuAbi, Guest, TransferAbi};

const STACK: u32 = 0x0070_0800;
const CALLER: u32 = 0x0001_2345;
const POOL: u32 = 0x0060_0000;
const DATA: u32 = 0x0060_1000;
const TAG_ENTRIES: u32 = 0x0061_0000;
const DEFINITION: u32 = 0x0062_0000;
/// Effect index 1, salt 0x5151.
const HANDLE: u32 = 0x5151_0001;
const DATUM: u32 = DATA + 0x190;

fn guest() -> Guest {
    let mut g = Guest::new();
    g.map_zeroed(0x0045_0000, 0x000d_0000, Permissions::RW).unwrap();
    g.map_zeroed(0x0060_0000, 0x0003_0000, Permissions::RW).unwrap();
    g.map_zeroed(0x0070_0000, 0x0000_1000, Permissions::RW).unwrap();
    g.set_u32(0x004e_a93c, POOL).unwrap();
    g.set_u32(POOL + 0x44, DATA).unwrap();
    g.set_u32(POOL + 0x38, 8).unwrap();
    g.set_u32(POOL + 0x24, 0x190).unwrap();
    g.set_u32(0x004e_3b44, TAG_ENTRIES).unwrap();
    // The datum's definition tag index 2.
    g.set_u32(DATUM + 4, 2).unwrap();
    g.set_u32(TAG_ENTRIES + 2 * 16 + 8, DEFINITION).unwrap();
    g.set_u16(DATUM, 0x5151).unwrap();
    g.set_u32(DATUM + 0x48, u32::MAX).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g
}

fn cpu() -> CpuAbi {
    CpuAbi {
        esp: STACK,
        ebx: 0xb0b0_b0b0,
        ecx: 0xc0c0_c0c0,
        esi: 0x5e5e_5e5e,
        edi: 0xd1d1_d1d1,
        ebp: 0xb9b9_b9b9,
        mxcsr: 0x1fa0,
        ..Default::default()
    }
}

fn run(cpu: &mut CpuAbi, g: &mut Guest, pc: u32) -> TransferAbi {
    let (body, _) = TABLE.at(pc).expect("a body owns pc");
    body.segment(cpu, g, pc).unwrap()
}

fn saved_kept(c: &CpuAbi) {
    assert_eq!((c.esi, c.edi), (0x5e5e_5e5e, 0xd1d1_d1d1));
}

#[test]
fn the_table_is_the_four_points() {
    assert_eq!(TABLE.bodies().len(), 4);
    assert_eq!(POINTS.map(|p| p.base), ENTRIES);
    for p in POINTS {
        assert!(TABLE.owns(p.base), "{}", p.name);
    }
    assert!(TABLE.supports(&cpu()));
}

#[test]
fn delete_of_no_handle_returns_at_once() {
    let mut g = guest();
    let mut c = CpuAbi { eax: u32::MAX, ..cpu() };
    let t = run(&mut c, &mut g, points::EFFECT_DELETE.base);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!(c.esp, STACK + 4);
    saved_kept(&c);
}

#[test]
fn delete_of_a_live_effect_checks_the_salt_then_frees_through_four_calls() {
    let mut g = guest();
    let mut c = CpuAbi { eax: HANDLE, ..cpu() };
    let t = run(&mut c, &mut g, points::EFFECT_DELETE.base);
    assert_eq!(t, TransferAbi::call(0x0017_7590, 0x0017_7535));
    assert_eq!((c.esi, c.edi, c.ebx, c.edx), (HANDLE, DATUM, POOL, 0x5151));
    // Datum +8 is 0: an attachment record, marked and unlinked.
    let attachments = 0x0060_2000;
    g.set_u32(0x004e_d28c, POOL + 0x100).unwrap();
    g.set_u32(POOL + 0x144, attachments).unwrap();
    c.esp += 4;
    let t = run(&mut c, &mut g, 0x0017_7535);
    assert_eq!(t, TransferAbi::call(0x0017_71a0, 0x0017_7564));
    assert_eq!(g.u8(attachments + 4).unwrap(), 2);
    assert_eq!(g.u32(DATUM + 8).unwrap(), u32::MAX);
    assert_eq!(g.u32(c.esp + 4).unwrap(), DATUM);
    c.esp += 8;
    let t = run(&mut c, &mut g, 0x0017_7564);
    assert_eq!((t, c.eax), (TransferAbi::call(0x0017_73a0, 0x0017_756b), HANDLE));
    c.esp += 4;
    let t = run(&mut c, &mut g, 0x0017_756b);
    assert_eq!(t, TransferAbi::call(0x0017_7460, 0x0017_7571));
    c.esp += 8;
    let t = run(&mut c, &mut g, 0x0017_7571);
    assert_eq!(t, TransferAbi::jump(0x0016_ba40));
    assert_eq!((c.eax, c.edx, c.ebx, c.esp), (HANDLE, POOL, 0xb0b0_b0b0, STACK));
    saved_kept(&c);
}

#[test]
fn culled_without_a_cluster_keeps_a_visible_effect() {
    let mut g = guest();
    g.set_u16(DATUM + 0x20, 0xffff).unwrap();
    g.set_u8(DATUM + 2, 8).unwrap();
    let mut c = CpuAbi { ebx: HANDLE, ..cpu() };
    let t = run(&mut c, &mut g, points::EFFECT_CULLED.base);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!((c.eax, c.esp, c.ecx & 0xff), (0, STACK + 4, 1));
    assert_eq!(c.eflags & 0x40, 0, "test esi, esi on the datum");
    saved_kept(&c);
}

#[test]
fn culled_asks_the_end_when_an_effect_turns_visible() {
    let mut g = guest();
    g.set_u16(DATUM + 0x20, 0xffff).unwrap();
    let mut c = CpuAbi { ebx: HANDLE, ..cpu() };
    let t = run(&mut c, &mut g, points::EFFECT_CULLED.base);
    assert_eq!(t, TransferAbi::call(0x0017_7610, 0x0017_97e8));
    assert_eq!(c.eax, HANDLE);
    assert_eq!(g.u8(DATUM + 2).unwrap(), 8);
    // `177610` true: the result is true.
    c.esp += 4;
    c.eax = 1;
    let t = run(&mut c, &mut g, 0x0017_97e8);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!(c.eax, 1);
    saved_kept(&c);
}

#[test]
fn update_of_a_stopped_effect_returns_false() {
    let mut g = guest();
    g.set_u8(DATUM + 2, 0x40).unwrap();
    g.set_u32(STACK + 4, HANDLE).unwrap();
    let mut c = cpu();
    let t = run(&mut c, &mut g, points::EFFECT_UPDATE.base);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!((c.eax, c.esp, c.ebp), (0, STACK + 12, 0xb9b9_b9b9));
    saved_kept(&c);
}

#[test]
fn update_with_a_negative_step_clears_bit_1() {
    let mut g = guest();
    g.set_u8(DATUM + 3, 0xff).unwrap();
    g.set_u32(STACK + 4, HANDLE).unwrap();
    g.set_u32(STACK + 8, (-1.0f32).to_bits()).unwrap();
    let mut c = cpu();
    let t = run(&mut c, &mut g, points::EFFECT_UPDATE.base);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!(g.u8(DATUM + 3).unwrap(), 0xfd);
    assert_eq!((c.eax, c.esp, c.ebx), (0, STACK + 12, 0xb0b0_b0b0));
    saved_kept(&c);
}

#[test]
fn update_at_an_events_end_selects_the_next_then_retires_past_the_last() {
    let mut g = guest();
    // In an event, emitters and locations off (bit 3).
    g.set_u8(DATUM + 2, 0x09).unwrap();
    g.set_u32(DATUM + 0x60, 0.5f32.to_bits()).unwrap();
    g.set_u32(DATUM + 0x64, 1.0f32.to_bits()).unwrap();
    g.set_u32(STACK + 4, HANDLE).unwrap();
    g.set_u32(STACK + 8, 1.0f32.to_bits()).unwrap();
    g.set_u32(DEFINITION + 0x14, 2).unwrap();
    let mut c = cpu();
    let t = run(&mut c, &mut g, points::EFFECT_UPDATE.base);
    assert_eq!(t, TransferAbi::call(0x0017_9190, 0x0017_93cc));
    let frame = STACK - 0x1c;
    assert_eq!(c.esp, frame - 8);
    assert_eq!(g.u32(c.esp + 4).unwrap(), DATUM);
    assert_eq!(g.u32(DATUM + 0x60).unwrap(), 1.0f32.to_bits());
    assert_eq!(g.u32(frame + 0x14).unwrap(), 0.5f32.to_bits(), "the time used");
    assert_eq!(g.u32(frame + 0x18).unwrap(), 0.5f32.to_bits(), "the clock before");
    assert_eq!(g.u32(STACK + 8).unwrap(), 0.5f32.to_bits(), "the time left");
    assert_eq!((c.ebx & 0xff, c.ebp, c.edi), (1, DEFINITION, HANDLE));
    // `179190` (`ret 4`) answers event 5 of 2: the end.
    c.esp = frame;
    c.eax = 5;
    let t = run(&mut c, &mut g, 0x0017_93cc);
    assert_eq!(t, TransferAbi::call(0x0017_7610, 0x0017_9481));
    assert_eq!(c.eax, HANDLE);
    // `177610` true: retired.
    c.esp = frame;
    c.eax = 1;
    let t = run(&mut c, &mut g, 0x0017_9481);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!((c.eax, c.esp, c.ebx), (1, STACK + 12, 0xb0b0_b0b0));
    saved_kept(&c);
}

#[test]
fn alive_without_an_object_returns_false_with_the_definition_in_ecx() {
    let mut g = guest();
    g.set_u32(STACK + 4, HANDLE).unwrap();
    let mut c = cpu();
    let t = run(&mut c, &mut g, points::EFFECT_ALIVE.base);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!((c.eax, c.ecx, c.esp), (0, DEFINITION, STACK + 8));
    assert_eq!((c.ebx, c.ebp), (0xb0b0_b0b0, 0xb9b9_b9b9));
    saved_kept(&c);
}

#[test]
fn alive_deletes_an_effect_whose_object_is_gone() {
    let mut g = guest();
    g.set_u32(STACK + 4, HANDLE).unwrap();
    g.set_u32(DATUM + 0x48, 0x0007_0003).unwrap();
    let mut c = cpu();
    let t = run(&mut c, &mut g, points::EFFECT_ALIVE.base);
    assert_eq!(t, TransferAbi::call(0x000b_adc0, 0x0017_952c));
    assert_eq!((c.eax, g.u32(c.esp + 4).unwrap()), (0x0007_0003, u32::MAX));
    // `badc0` (`ret 4`) false: the delete body, then true.
    c.esp += 8;
    c.eax = 0;
    let t = run(&mut c, &mut g, 0x0017_952c);
    assert_eq!(t, TransferAbi::call(points::EFFECT_DELETE.base, 0x0017_9537));
    assert_eq!(c.eax, HANDLE);
    c.esp += 4;
    let t = run(&mut c, &mut g, 0x0017_9537);
    assert_eq!(t, TransferAbi::ret(CALLER));
    assert_eq!((c.eax, c.ecx, c.esp), (1, DEFINITION, STACK + 8));
    saved_kept(&c);
}
