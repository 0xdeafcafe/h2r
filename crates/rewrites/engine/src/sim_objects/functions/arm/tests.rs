//! The arm's protocol on a synthetic guest: compare until proven, then
//! answer with every [`AUDIT_EVERY`]th call compared, stop on a mismatch,
//! drop passed-over audits, decline by reason, and start over at a reset.
//! The evaluator's arithmetic is tested in `sim_objects` itself.

use super::*;
use runtime::guest::Permissions;

/// The counters are process-wide: the arm's tests run one at a time.
static ARM: Mutex<()> = Mutex::new(());

const STACK: u32 = 0x007f_0000;
const OUT: u32 = 0x007f_0800;
const POOL: u32 = 0x0050_0000;
const BODY: u32 = 0x0050_2000;
const HANDLE: u32 = 0x1234_0001;
const ONE_BITS: u32 = 0x3f80_0000;

/// An image whose constants and names are zero but for 1.0, one object,
/// and a call `bab40(HANDLE, 0, OUT)` (name 0 is the built-in one).
fn call() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(0x000b_0000, 0x0010_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(0x0041_0000, 0x0020_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(STACK, 0x1000, Permissions::RW).unwrap();
    g.set_u32(ONE, ONE_BITS).unwrap();
    g.set_u32(OBJECTS, POOL).unwrap();
    g.set_u32(POOL + 0x44, POOL + 0x1000).unwrap();
    g.set_u32(POOL + 0x1000 + 12 + 8, BODY).unwrap();
    g.set_u32(STACK, 0x0001_2345).unwrap();
    g.set_u32(STACK + 4, HANDLE).unwrap();
    g.set_u32(STACK + 8, 0).unwrap();
    g.set_u32(STACK + 0xc, OUT).unwrap();
    let mut cpu = CpuAbi {
        esp: STACK,
        ecx: 0xaabb_ccdd,
        eflags_defined: ARITH,
        mxcsr: 0x1fa0,
        ..Default::default()
    };
    cpu.x87.control = 0x027f;
    (cpu, g)
}

/// The guest's return from `cpu`'s call: `value` stored, `al`, the frame
/// popped, `ecx` with a zero result byte, `xmm0` the value.
fn guest_return(cpu: &CpuAbi, g: &mut Guest, value: u32, al: u8) -> CpuAbi {
    let mut c = *cpu;
    g.set_u32(OUT, value).unwrap();
    c.esp += 4 + ARGS;
    c.eax = (c.eax & !0xff) | u32::from(al);
    c.ecx &= 0x00ff_ffff;
    c.xmm[0] = u128::from(value).to_le_bytes();
    c
}

fn audit(cpu: &CpuAbi, g: &mut Guest, value: u32, al: u8) {
    let mut entering = *cpu;
    assert_eq!(enter(&mut entering, g), Step::Audit);
    assert_eq!(
        (entering.esp, entering.eax, entering.ecx, entering.xmm[0]),
        (cpu.esp, cpu.eax, cpu.ecx, cpu.xmm[0]),
        "an audited call leaves the registers alone"
    );
    let back = guest_return(cpu, g, value, al);
    returned(&back, g);
}

#[test]
fn the_arm_compares_then_answers_audits_and_stops_on_a_mismatch() {
    let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
    reset();
    let (cpu, mut g) = call();
    while status().matched < PROVEN_AFTER {
        audit(&cpu, &mut g, ONE_BITS, 1);
    }
    assert_eq!(
        (status().checked, status().mismatched, status().answered),
        (PROVEN_AFTER, 0, 0)
    );

    // Proven: the evaluator answers all but every AUDIT_EVERY'th call.
    let (mut answered, mut audited) = (0, 0);
    for _ in 0..2 * AUDIT_EVERY {
        let mut entering = cpu;
        g.set_u32(OUT, 0xdead_beef).unwrap();
        match enter(&mut entering, &mut g) {
            Step::Answered { target } => {
                answered += 1;
                assert_eq!(target, 0x0001_2345);
                assert_eq!(entering.esp, STACK + 4 + ARGS);
                assert_eq!(g.u32(OUT).unwrap(), ONE_BITS);
                assert_eq!(entering.eax as u8, 1);
                assert_eq!(
                    entering.ecx, 0x00bb_ccdd,
                    "the frame's result byte, 0, on top"
                );
                assert_eq!(&entering.xmm[0][..4], &ONE_BITS.to_le_bytes());
                assert_eq!(entering.eflags_defined & ARITH, 0);
            }
            Step::Audit => {
                audited += 1;
                returned(&guest_return(&cpu, &mut g, ONE_BITS, 1), &g);
            }
            Step::Guest => panic!("an answerable call is never declined"),
        }
    }
    assert_eq!((answered, audited), (2 * AUDIT_EVERY - 2, 2));
    assert_eq!(status().answered, answered);

    // A mismatch at an audit ends the take-over.
    loop {
        let mut entering = cpu;
        if enter(&mut entering, &mut g) == Step::Audit {
            returned(&guest_return(&cpu, &mut g, 0, 0), &g);
            break;
        }
    }
    assert_eq!(status().mismatched, 1);
    for _ in 0..2 * AUDIT_EVERY {
        let mut entering = cpu;
        assert_eq!(enter(&mut entering, &mut g), Step::Audit);
        returned(&guest_return(&cpu, &mut g, ONE_BITS, 1), &g);
    }
}

#[test]
fn declines_are_counted_by_reason_and_passed_over_audits_dropped() {
    let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
    reset();
    let (cpu, mut g) = call();
    // The float environment.
    let mut env = cpu;
    env.mxcsr = 0x1f80;
    assert_eq!(enter(&mut env, &mut g), Step::Guest);
    // An override.
    g.set_u32(BODY + 0xc0, 0x200).unwrap();
    assert_eq!(enter(&mut { cpu }, &mut g), Step::Guest);
    g.set_u32(BODY + 0xc0, 0).unwrap();
    // A faulting read: the out pointer's slot is past the stack.
    let mut off = cpu;
    off.esp = STACK + 0x1000 - 8;
    assert_eq!(enter(&mut off, &mut g), Step::Guest);
    let s = status();
    assert_eq!(
        (
            s.declined[Decline::Float.index()],
            s.declined[Decline::Override.index()],
            s.declined[Decline::Fault.index()]
        ),
        (1, 1, 1)
    );
    assert_eq!(s.declined_total(), 3);
    assert_eq!(s.checked, 0);

    // An audited call whose return was taken elsewhere is dropped when an
    // outer frame returns past it.
    assert_eq!(enter(&mut { cpu }, &mut g), Step::Audit);
    let mut outer = cpu;
    outer.esp = STACK + 0x100;
    returned(&outer, &g);
    assert_eq!((status().dropped, status().checked), (1, 0));
    // A return below a pending audit's frame leaves it waiting.
    assert_eq!(enter(&mut { cpu }, &mut g), Step::Audit);
    let mut inner = cpu;
    inner.esp = STACK - 0x40;
    returned(&inner, &g);
    returned(&guest_return(&cpu, &mut g, ONE_BITS, 1), &g);
    assert_eq!(
        (status().checked, status().matched, status().dropped),
        (1, 1, 1)
    );
}

#[test]
fn a_reset_puts_the_arm_back_to_comparing() {
    let _one = ARM.lock().unwrap_or_else(|e| e.into_inner());
    reset();
    let (cpu, mut g) = call();
    while status().matched < PROVEN_AFTER {
        audit(&cpu, &mut g, ONE_BITS, 1);
    }
    let took_over =
        (0..AUDIT_EVERY).any(|_| matches!(enter(&mut { cpu }, &mut g), Step::Answered { .. }));
    assert!(took_over);
    reset();
    assert_eq!(status(), Status::default());
    for _ in 0..AUDIT_EVERY {
        audit(&cpu, &mut g, ONE_BITS, 1);
    }
    assert_eq!(status().answered, 0);
}
