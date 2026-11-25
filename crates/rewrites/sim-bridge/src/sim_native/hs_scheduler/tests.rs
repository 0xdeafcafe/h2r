use super::*;
use engine::FlatMemory;
use ir::interp::{Cpu, FlatMemory as InterpMemory};
use lifted::{code::run_until, image::Image};

const BASE: u32 = 0x4f_0000;
const HEADER: u32 = 0x52_0000;
const DATA: u32 = HEADER + 0x100;
const BITMAP: u32 = HEADER + 0xb000;
const TIME: u32 = HEADER + 0xc000;
const STACK: u32 = HEADER + 0xd000;
const TAIL: u32 = 0x001d_eed0;

fn address(index: u16) -> u32 {
    DATA + u32::from(index) * THREAD_BYTES as u32
}

fn handle(index: u16) -> DatumHandle {
    DatumHandle::new(0x8a00 + index, u32::from(index))
}

fn fixture() -> FlatMemory {
    let mut m = FlatMemory::new(BASE, 0x40000);
    m.set_u8(ENABLED, 1).unwrap();
    m.set_u32(points::GAME_TIME.base, TIME).unwrap();
    m.set_i32(TIME + 8, 7).unwrap();
    m.set_u32(hs_pool::THREADS, HEADER).unwrap();
    for (offset, value) in [
        (layout::DATA, DATA),
        (layout::BITMAP, BITMAP),
        (layout::HIGH_WATER, 34),
        (layout::MAXIMUM_COUNT, 40),
        (layout::ELEMENT_SIZE, THREAD_BYTES as u32),
    ] {
        m.set_u32(HEADER + offset, value).unwrap();
    }
    for (slot, category, wake) in [(1, 0, -2), (2, 4, 0), (5, 0, 7), (13, 0, 8), (33, 2, 7)] {
        let base = address(slot);
        m.set_u16(base, handle(slot).salt()).unwrap();
        m.set_u8(base + 2, category).unwrap();
        m.set_i32(base + 8, wake).unwrap();
        let word = BITMAP + u32::from(slot / 32) * 4;
        let bits = m.u32(word).unwrap() | (1 << (slot % 32));
        m.set_u32(word, bits).unwrap();
    }
    m
}

// Effects of the first dispatched thread: it changes the clock, wakes an
// earlier slot, and appends a thread. The earlier slot waits until next tick.
fn effects(m: &mut FlatMemory) {
    m.set_i32(TIME + 8, 8).unwrap();
    m.set_i32(address(1) + 8, 0).unwrap();
    m.set_u32(HEADER + layout::HIGH_WATER, 35).unwrap();
    let bits = m.u32(BITMAP + 4).unwrap();
    m.set_u32(BITMAP + 4, bits | 4).unwrap();
    m.set_u16(address(34), handle(34).salt()).unwrap();
}

#[test]
fn live_occupancy_and_wake_times_use_the_update_entry_tick() {
    let mut m = fixture();
    let mut pass = Pass::begin(&m, STACK).unwrap();
    assert_eq!(pass.next, Some(handle(5)));
    effects(&mut m);
    pass.read_next(&m).unwrap();
    assert_eq!(pass.next, Some(handle(33)));
    pass.read_next(&m).unwrap();
    assert_eq!(pass.next, Some(handle(34)));
    pass.read_next(&m).unwrap();
    assert_eq!(pass.next, None);

    let mut pass = Pass::begin(&m, STACK).unwrap();
    assert_eq!(pass.next, Some(handle(1)));
    m.set_u8(ENABLED, 0).unwrap();
    pass.read_next(&m).unwrap();
    assert_eq!(pass.next, None);
    m.set_u32(points::GAME_TIME.base, u32::MAX).unwrap();
    assert_eq!(Pass::begin(&m, STACK).unwrap().next, None);
}

#[test]
fn lifted_scheduler_selects_the_predicted_salted_handles() {
    let Some(image) = Image::get() else { return };
    let mut code = image.original();
    let mut native = fixture();
    let mut pass = Pass::begin(&native, STACK).unwrap();
    let mut guest = InterpMemory {
        base: BASE,
        bytes: native.bytes.clone(),
    };
    let mut cpu = Cpu {
        eip: points::SCRIPT_UPDATE.base,
        ..Cpu::default()
    };
    cpu.gpr[4] = STACK;
    let mut order = Vec::new();
    loop {
        run_until(&mut code, &mut cpu, &mut guest, 10000, |c| {
            c.eip == points::HS_THREAD_EXECUTE.base || c.eip == TAIL
        })
        .unwrap();
        if cpu.eip == TAIL {
            assert_eq!(pass.next, None);
            break;
        }
        assert_eq!(cpu.gpr[4], STACK - SAVED_BYTES - 4);
        assert_eq!(guest.read_u32(cpu.gpr[4]), Some(SCHEDULER_RESUME));
        let called = DatumHandle(cpu.gpr[7]);
        assert_eq!(pass.next, Some(called));
        order.push(called.index());
        if order.len() == 1 {
            effects(&mut native);
            // Copy only the fixture data, preserving the guest scheduler's
            // live stack and registers across the scripted call.
            let end = (TIME + 12 - BASE) as usize;
            assert!(guest.load(BASE, &native.bytes[..end]));
        }
        pass.read_next(&native).unwrap();
        cpu.gpr[4] += 4;
        cpu.eip = SCHEDULER_RESUME;
    }
    assert_eq!(order, [5, 33, 34]);
}
