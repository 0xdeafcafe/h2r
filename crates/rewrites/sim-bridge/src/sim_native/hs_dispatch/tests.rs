use super::*;
use engine::FlatMemory;

const BASE: u32 = hs::FUNCTION_COUNT & !0xfff;
const ARENA: u32 = THREADS & !0xfff;
const THREAD_POOL: u32 = ARENA + 0x1000;
const THREAD: u32 = ARENA + 0x1100;
const NODE_POOL: u32 = ARENA + 0x2000;
const NODE_DATA: u32 = ARENA + 0x2100;
const STACK: u32 = ARENA + 0x3000;
const PARAMETERS: u32 = ARENA + 0x3200;
const FRAME: u32 = THREAD + 0x40;
const PREFIX: u16 = 2;
const ARGS: u32 = FRAME + FRAME_LOCALS + PREFIX as u32;
const RETURN: u32 = 0xf000_1234;
const CALL: u32 = 0xcafe_0000;
const NAME: u32 = 0xf00d_0001;
const ARG1: u32 = 0xa11c_0002;
const ARG2: u32 = 0xc0de_0003;

fn fixture(count: u32, first: bool, position: u16, expression: u32) -> (CpuAbi, FlatMemory) {
    let mut mem = FlatMemory::new(BASE, (ARENA - BASE + 0x4000) as usize);
    for (global, pool, data, stride, used) in [
        (THREADS, THREAD_POOL, THREAD, THREAD_BYTES, 1),
        (NODES, NODE_POOL, NODE_DATA, Node::SIZE as u32, 4),
    ] {
        mem.set_u32(global, pool).unwrap();
        mem.set_u32(pool + layout::DATA, data).unwrap();
        mem.set_u32(pool + layout::ELEMENT_SIZE, stride).unwrap();
        mem.set_u32(pool + layout::HIGH_WATER, used).unwrap();
    }
    mem.set_u16(THREAD, 0xbabe).unwrap();
    mem.set_u8(THREAD + 3, 0x40).unwrap();
    mem.set_u32(THREAD + 0x10, FRAME).unwrap();
    mem.set_u32(FRAME + 4, CALL).unwrap();
    mem.set_u16(FRAME + 0xc, PREFIX).unwrap();
    for (handle, ty, flags, next, data) in [
        (CALL, 8, 0, u32::MAX, NAME),
        (NAME, 2, 1, ARG1, 0),
        (ARG1, 8, 1, ARG2, 0xbadc0de),
        (ARG2, 8, 0, u32::MAX, NAME),
    ] {
        let at = NODE_DATA + (handle & 0xffff) * Node::SIZE as u32;
        mem.set_u16(at, (handle >> 16) as u16).unwrap();
        mem.set_u16(at + 2, ty).unwrap();
        mem.set_u16(at + 4, ty).unwrap();
        mem.set_u16(at + 6, flags).unwrap();
        mem.set_u32(at + 8, next).unwrap();
        mem.set_u32(at + 0x10, data).unwrap();
    }
    let descriptor = PARAMETERS - 14;
    mem.set_u32(hs::FUNCTION_COUNT, 9).unwrap();
    mem.set_u32(hs::FUNCTION_TABLE + 4 * 8, descriptor).unwrap();
    mem.set_u16(descriptor + 12, count as u16).unwrap();
    mem.set_u16(PARAMETERS, 8).unwrap();
    mem.set_u16(PARAMETERS + 2, 8).unwrap();
    let index = ARGS + count * 4;
    mem.set_u16(index, position).unwrap();
    mem.set_u32(index + 2, expression).unwrap();
    for (i, value) in [RETURN, 0xbabe_0000, count, PARAMETERS, u32::from(first)]
        .into_iter()
        .enumerate()
    {
        mem.set_u32(STACK + i as u32 * 4, value).unwrap();
    }
    (
        CpuAbi {
            eip: points::HS_EVALUATE_ARGUMENTS.base,
            esp: STACK,
            ..CpuAbi::default()
        },
        mem,
    )
}

#[test]
fn reentry_resumes_after_the_completed_argument() {
    let (cpu, mem) = fixture(2, false, 1, ARG2);
    let Prediction::Arguments(planned) = plan(&cpu, &mem).unwrap() else {
        panic!("synthetic signature should match");
    };
    assert_eq!(
        (planned.index, planned.next, planned.result),
        (2, Datum::NULL, 0)
    );
    let child = planned.child.unwrap();
    assert_eq!(child.frame, FRAME + u32::from(PREFIX) + 2 * 4 + 6 + 0x10);
    assert_eq!(
        (child.destination, child.expression),
        (ARGS + 4, Datum(ARG2))
    );
    let (cpu, mut mem) = fixture(2, true, 0x7777, u32::MAX);
    let Prediction::Arguments(first) = plan(&cpu, &mem).unwrap() else {
        panic!("synthetic signature should match");
    };
    assert!(first.child.is_none());
    assert_eq!((first.index, first.next, first.result), (1, Datum(ARG2), 0));
    mem.set_u16(PARAMETERS, 7).unwrap();
    assert!(plan(&cpu, &mem).is_none(), "mistyped argument");
}

#[test]
fn ready_returns_the_start_of_the_reserved_block() {
    let (mut cpu, mut mem) = fixture(2, false, 2, u32::MAX);
    let planned = plan(&cpu, &mem).unwrap();
    mem.set_u16(FRAME + 0xc, PREFIX + 14).unwrap();
    cpu.eax = ARGS;
    assert_eq!(compare(&planned, &cpu, &mem), None);
    cpu.eax = 0;
    assert!(
        compare(&planned, &cpu, &mem)
            .unwrap()
            .contains("argument block")
    );
    mem.set_u32(STACK + 8, u16::MAX.into()).unwrap();
    mem.set_u16(PARAMETERS - 2, u16::MAX).unwrap();
    assert!(plan(&cpu, &mem).is_none(), "oversized argument reservation");
}

#[test]
fn a_wrapper_receiving_another_builtins_signature_is_a_mismatch() {
    let (cpu, mut mem) = fixture(2, false, 2, u32::MAX);
    mem.set_u32(STACK + 8, 1).unwrap();
    let wrong_count = plan(&cpu, &mem).unwrap();
    assert!(
        compare(&wrong_count, &cpu, &mem)
            .unwrap()
            .contains("expects 2 arguments")
    );
    mem.set_u32(STACK + 8, 2).unwrap();
    mem.set_u32(STACK + 12, PARAMETERS + 2).unwrap();
    let wrong_type = plan(&cpu, &mem).unwrap();
    assert!(
        compare(&wrong_type, &cpu, &mem)
            .unwrap()
            .contains("argument 1 expects type 8")
    );
}

/// Repeat each boundary against the actual lifted helper and expression
/// evaluator. Only code comes from the owner's image. All VM data is invented.
#[test]
#[ignore = "reads the owner's H2_XBE"]
fn guest_argument_dispatch() {
    use lifted::{
        code::run_until,
        cpu::{from_interp, to_interp},
        image::Image,
    };

    let image = Image::require();
    let mut code = image.original();
    let mut ram = image.ram.clone();
    for (count, first, position, expression) in [
        (0, true, 0x7fff, 0xdead),
        (2, true, 0x7fff, 0xdead),
        (2, false, 1, ARG2),
        (2, false, 2, u32::MAX),
    ] {
        let (cpu, mut mem) = fixture(count, first, position, expression);
        let planned = plan(&cpu, &mem).unwrap();
        assert!(ram.load(BASE, &mem.bytes));
        let mut actual = to_interp(&cpu);
        run_until(&mut code, &mut actual, &mut ram, 1000, |cpu| {
            cpu.eip == RETURN
        })
        .unwrap();
        let start = (BASE - ram.base) as usize;
        let end = start + mem.bytes.len();
        mem.bytes.copy_from_slice(&ram.bytes[start..end]);
        assert_eq!(compare(&planned, &from_interp(&actual), &mem), None);
    }
}
