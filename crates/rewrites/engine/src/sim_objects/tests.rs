//! The dispatcher on synthetic bodies: fusion across calls, returns and
//! tail jumps, true callouts, the float-environment fallback, and the
//! table's consistency rules. No image needed.

use super::*;
use crate::points::Sig;
use runtime::guest::Permissions;

const DOC: &str = "docs/engine/object-update.md";
static OUTER: Point = Point::new("TEST_OUTER", 0x1000, Sig::Opaque, DOC);
static INNER: Point = Point::new("TEST_INNER", 0x2000, Sig::Opaque, DOC);
static TAIL: Point = Point::new("TEST_TAIL", 0x3000, Sig::Opaque, DOC);

/// Not a body: a call here is a true callout.
const GUEST: u32 = 0x9000;
const STACK: u32 = 0x8000;
const CALLER: u32 = 0xdead;

/// `outer(arg)`: `eax = 1`; `call inner`; `eax += 10`; `call GUEST`;
/// `ret 4`.
fn outer(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    Ok(match pc {
        0x1000 => {
            cx.cpu.eax = 1;
            Next::Call { target: INNER.base, site: 0x1010 }
        }
        0x1010 => {
            cx.cpu.eax = cx.cpu.eax.wrapping_add(10);
            Next::Call { target: GUEST, site: 0x1020 }
        }
        0x1020 => Next::Ret { pop: 4 },
        _ => unreachable!("{pc:#x}"),
    })
}

/// `inner()`: `eax *= 2`; also stores `eax` at `[esp + 8]` (outer's
/// argument slot) so the test sees the frame; `ret`.
fn inner(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    assert_eq!(pc, INNER.base);
    cx.cpu.eax = cx.cpu.eax.wrapping_mul(2);
    frame::set_slot(cx.cpu, cx.g, 8, cx.cpu.eax)?;
    Ok(Next::Ret { pop: 0 })
}

/// `tail()`: `jmp inner` when `ecx` is zero, else `jmp GUEST`.
fn tail(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    assert_eq!(pc, TAIL.base);
    let target = if cx.cpu.ecx == 0 { INNER.base } else { GUEST };
    Ok(Next::Tail { target })
}

static BODIES_T: [Body; 3] = [
    Body { point: &OUTER, sites: &[0x1010, 0x1020], run: outer },
    Body { point: &INNER, sites: &[], run: inner },
    Body { point: &TAIL, sites: &[], run: tail },
];

fn table() -> Table {
    Table::new(BODIES_T.iter()).unwrap()
}

/// A stack with `CALLER` as the return address and one argument.
fn guest() -> (CpuAbi, Guest) {
    let mut g = Guest::new();
    g.map_zeroed(STACK - 0x1000, 0x2000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    g.set_u32(STACK + 4, 0x55).unwrap();
    (CpuAbi { esp: STACK, ..Default::default() }, g)
}

#[test]
fn zero_bodies_build_an_empty_table_that_owns_nothing() {
    assert!(BODIES.is_empty());
    assert!(TABLE.is_empty());
    for point in [
        &crate::points::OBJECTS_PRE_PHYSICS,
        &crate::points::OBJECTS_POST_PHYSICS,
        &crate::points::OBJECTS_FINALIZE,
        &crate::points::OBJECT_PRE_UPDATE,
        &crate::points::OBJECT_POST_UPDATE,
    ] {
        assert!(!TABLE.owns(point.base), "{}", point.name);
        assert!(TABLE.at(point.base).is_none());
    }
    // Driving an address no body owns is an execute fault, not a guess.
    let (mut cpu, mut g) = guest();
    let fault = drive(&mut cpu, &mut g, &TABLE, OUTER.base).unwrap_err();
    assert_eq!(fault.addr, OUTER.base);
    assert_eq!(cpu.esp, STACK);
}

#[test]
fn the_table_finds_entries_and_sites() {
    let t = table();
    assert_eq!(t.bodies().len(), 3);
    assert!(t.owns(OUTER.base) && t.owns(INNER.base) && t.owns(TAIL.base));
    assert!(!t.owns(0x1010));
    let (body, role) = t.at(0x1020).unwrap();
    assert_eq!((body.point.base, role), (OUTER.base, Role::Site));
    assert!(t.at(0x1011).is_none());
    assert!(t.at(GUEST).is_none());
}

#[test]
fn an_address_claimed_twice_is_refused() {
    static TWICE: [Body; 2] = [
        Body { point: &OUTER, sites: &[0x1010], run: outer },
        Body { point: &INNER, sites: &[0x1010], run: inner },
    ];
    assert_eq!(Table::new(TWICE.iter()).err(), Some(TableError::Duplicate(0x1010)));
    static SAME_ENTRY: [Body; 2] = [
        Body { point: &OUTER, sites: &[], run: outer },
        Body { point: &OUTER, sites: &[], run: outer },
    ];
    assert_eq!(Table::new(SAME_ENTRY.iter()).err(), Some(TableError::Duplicate(OUTER.base)));
}

#[test]
fn calls_between_bodies_stay_in_rust_until_a_true_callout() {
    let t = table();
    let (mut cpu, mut g) = guest();
    let out = drive(&mut cpu, &mut g, &t, OUTER.base).unwrap();
    // inner ran fused (eax 1 * 2), outer resumed at 0x1010 (+ 10), and the
    // call to GUEST leaves with outer's return site on the stack.
    assert_eq!(out, TransferAbi::call(GUEST, 0x1020));
    assert_eq!(cpu.eax, 12);
    assert_eq!(cpu.esp, STACK - 4);
    assert_eq!(g.u32(STACK - 4).unwrap(), 0x1020);
    // inner wrote its [esp + 8] while its own return address was pushed:
    // outer's argument slot.
    assert_eq!(g.u32(STACK + 4).unwrap(), 2);
    assert_eq!(g.u32(STACK).unwrap(), CALLER);

    // The runtime runs GUEST, which returns to the registered site.
    let site = frame::pop(&mut cpu, &g).unwrap();
    cpu.eax = 0x77;
    let out = drive(&mut cpu, &mut g, &t, site).unwrap();
    // `ret 4` to a caller that is not a body: back to the runtime.
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.esp, STACK + 8);
    assert_eq!(cpu.eax, 0x77);
}

#[test]
fn an_unsupported_float_environment_stops_fusion() {
    let t = table();
    for mxcsr in [0x1f80 | 0x2000, 0x1f80 | 0x8000, 0x1f80 | 0x40, 0x1f00] {
        let (mut cpu, mut g) = guest();
        cpu.mxcsr = mxcsr;
        assert!(!supported(&cpu));
        // The installer checked the entry; drive does not fuse further.
        let out = drive(&mut cpu, &mut g, &t, OUTER.base).unwrap();
        assert_eq!(out, TransferAbi::call(INNER.base, 0x1010), "{mxcsr:#x}");
        assert_eq!(cpu.esp, STACK - 4);
    }
    let (mut cpu, _) = guest();
    cpu.x87.status |= 0x80;
    assert!(!supported(&cpu));
    assert!(supported(&CpuAbi::default()));
}

#[test]
fn tail_jumps_fuse_into_a_body_or_leave_as_a_jump() {
    let t = table();
    let (mut cpu, mut g) = guest();
    cpu.eax = 5;
    // jmp inner: inner returns straight to CALLER.
    let out = drive(&mut cpu, &mut g, &t, TAIL.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!((cpu.eax, cpu.esp), (10, STACK + 4));

    let (mut cpu, mut g) = guest();
    cpu.ecx = 1;
    let out = drive(&mut cpu, &mut g, &t, TAIL.base).unwrap();
    assert_eq!(out, TransferAbi::jump(GUEST));
    assert_eq!(cpu.esp, STACK);
}

#[test]
fn a_return_into_a_body_site_resumes_that_body() {
    let t = table();
    let (mut cpu, mut g) = guest();
    // A frame the lifted original built: outer's call to inner returns
    // to 0x1010, which is outer's site, so the guest frame is finished
    // in Rust.
    cpu.esp = STACK - 4;
    g.set_u32(STACK - 4, 0x1010).unwrap();
    cpu.eax = 3;
    let out = drive(&mut cpu, &mut g, &t, INNER.base).unwrap();
    assert_eq!(out, TransferAbi::call(GUEST, 0x1020));
    assert_eq!(cpu.eax, 16);
}

#[test]
fn a_fault_in_a_body_is_returned_with_the_frame_as_it_was() {
    let t = table();
    let mut g = Guest::new();
    g.map_zeroed(STACK, 0x1000, Permissions::RW).unwrap();
    g.set_u32(STACK, CALLER).unwrap();
    // The push of outer's site faults below the mapping.
    let mut cpu = CpuAbi { esp: STACK, ..Default::default() };
    let fault = drive(&mut cpu, &mut g, &t, OUTER.base).unwrap_err();
    assert_eq!(fault.addr, STACK - 4);
    assert_eq!(cpu.esp, STACK);
}
