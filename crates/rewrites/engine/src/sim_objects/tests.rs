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
fn slice_one_bodies_form_a_table_that_owns_their_entries_and_sites() {
    assert!(!TABLE.is_empty());
    assert_eq!(TABLE.bodies().len(), BODIES.len());
    for body in BODIES {
        assert!(TABLE.owns(body.point.base), "{}", body.point.name);
        for &site in body.sites {
            let (at, role) = TABLE.at(site).unwrap();
            assert_eq!((at.point.base, role), (body.point.base, Role::Site), "{site:#x}");
            assert!(!TABLE.owns(site));
        }
    }
    for point in [
        &crate::points::OBJECTS_PRE_PHYSICS,
        &crate::points::OBJECTS_POST_PHYSICS,
        &crate::points::OBJECTS_FINALIZE,
        &crate::points::OBJECT_PRE_UPDATE,
        &crate::points::OBJECT_POST_UPDATE,
        &crate::points::OBJECT_TYPE_UPDATE,
        &crate::points::OBJECT_TYPE_POST_UPDATE,
        &crate::points::OBJECT_TYPE_POSE,
        &crate::points::OBJECT_TYPE_AFTER_ROOT,
        &crate::points::OBJECT_WAKE,
        &crate::points::OBJECT_ROOT,
        &crate::points::OBJECT_WAKE_FOR_POST,
        &crate::points::OBJECT_WAKE_CHILDREN,
        &crate::points::OBJECT_EARLY_MOVER_WAKE,
        &crate::points::OBJECT_DAMAGE_UPDATE,
    ] {
        assert!(TABLE.owns(point.base), "{}", point.name);
    }
    // Callouts that stay guest are not bodies.
    for callout in [0xfd910, 0xbf380, 0xb83b0, 0x109660, 0x320560, 0xbb050] {
        assert!(TABLE.at(callout).is_none(), "{callout:#x}");
    }
    // Driving an address no body owns is an execute fault, not a guess.
    let (mut cpu, mut g) = guest();
    let fault = drive(&mut cpu, &mut g, &TABLE, OUTER.base).unwrap_err();
    assert_eq!(fault.addr, OUTER.base);
    assert_eq!(cpu.esp, STACK);
}

// A synthetic objects pool in guest memory: the globals the walks read,
// the datum array, headers and bodies. No game data.
const POOL_MAP: u32 = 0x004d_0000;
const PHASE_STATE: u32 = 0x004d_e2f4;
const PHASE: u32 = 0x0050_0000;
const ARRAY: u32 = 0x0050_1000;
const HEADERS: u32 = 0x0050_2000;
const BITMAP: u32 = 0x0050_3000;
const BODY_AT: u32 = 0x0051_0000;
const NONE: u32 = u32::MAX;

fn body_of(slot: u16) -> u32 {
    BODY_AT + 0x200 * u32::from(slot)
}

fn handle(salt: u16, slot: u16) -> u32 {
    u32::from(salt) << 16 | u32::from(slot)
}

/// `slots`: `(slot, salt, header flags, type)`; each body's definition is
/// `e000_0000 | slot` and it has no links.
fn pool(g: &mut Guest, slots: &[(u16, u16, u8, u8)]) {
    g.map_zeroed(POOL_MAP, 0x6_0000, Permissions::RW).unwrap();
    g.set_u32(PHASE_STATE, PHASE).unwrap();
    g.set_u32(crate::points::OBJECTS.base, ARRAY).unwrap();
    g.set_u32(ARRAY + 0x24, 12).unwrap();
    g.set_u32(ARRAY + 0x44, HEADERS).unwrap();
    g.set_u32(ARRAY + 0x48, BITMAP).unwrap();
    let (mut bits, mut high) = (0u32, 0u32);
    for &(slot, salt, flags, kind) in slots {
        let h = HEADERS + 12 * u32::from(slot);
        g.set_u16(h, salt).unwrap();
        g.set_u8(h + 2, flags).unwrap();
        g.set_u8(h + 3, kind).unwrap();
        g.set_u32(h + 8, body_of(slot)).unwrap();
        g.set_u32(body_of(slot), 0xe000_0000 | u32::from(slot)).unwrap();
        for link in [0x0c, 0x10, 0x14] {
            g.set_u32(body_of(slot) + link, NONE).unwrap();
        }
        bits |= 1 << slot;
        high = high.max(u32::from(slot) + 1);
    }
    g.set_u32(BITMAP, bits).unwrap();
    g.set_u32(ARRAY + 0x38, high).unwrap();
}

/// Return from a true callout that pops `cleanup` argument bytes.
fn returned(cpu: &mut CpuAbi, g: &Guest, cleanup: u32) -> u32 {
    let site = frame::pop(cpu, g).unwrap();
    cpu.esp += cleanup;
    site
}

#[test]
fn finalize_clears_defer_deletes_and_records_each_deletion() {
    use ::sim_objects::events::{self, SimEvent};
    use guest_layout::Datum;
    let (mut cpu, mut g) = guest();
    // Slot 0 deferred, 1 marked for deletion, 2 free, 3 both.
    pool(&mut g, &[(0, 0x8000, 0x21, 6), (1, 0x8001, 0x11, 2), (3, 0x8003, 0x31, 5)]);
    events::clear();
    let t = &*TABLE;
    let fin = crate::points::OBJECTS_FINALIZE.base;
    // Pass A, then the deferred weapon fire.
    let out = drive(&mut cpu, &mut g, t, fin).unwrap();
    assert_eq!(out, TransferAbi::call(0xfd910, 0xb71a0));
    assert_eq!(g.u8(PHASE + 3).unwrap(), 1);
    for (slot, flags) in [(0, 0x01), (1, 0x11), (3, 0x11)] {
        assert_eq!(g.u8(HEADERS + 12 * slot + 2).unwrap(), flags, "slot {slot}");
    }
    assert!(events::with(<[SimEvent]>::is_empty));
    // Pass B: each deleted object goes through b83b0 then b8460, both
    // `(handle, 1)`, with its event recorded first.
    let mut site = returned(&mut cpu, &g, 0);
    for (slot, salt) in [(1, 0x8001), (3, 0x8003)] {
        for (callee, next) in [(0xb83b0, 0xb7282), (0xb8460, 0xb728a)] {
            let out = drive(&mut cpu, &mut g, t, site).unwrap();
            assert_eq!(out, TransferAbi::call(callee, next));
            assert_eq!(g.u32(cpu.esp + 4).unwrap(), handle(salt, slot));
            assert_eq!(g.u32(cpu.esp + 8).unwrap(), 1);
            site = returned(&mut cpu, &g, 8);
        }
    }
    let out = drive(&mut cpu, &mut g, t, site).unwrap();
    assert_eq!(out, TransferAbi::jump(0xbf380));
    assert_eq!(g.u8(PHASE + 3).unwrap(), 0);
    assert_eq!(cpu.esp, STACK, "the frame is gone before the tail jump");
    let deleted = |slot: u16, salt: u16, kind: u8| SimEvent::ObjectDeleted {
        id: Datum(handle(salt, slot)),
        kind,
        definition: Datum(0xe000_0000 | u32::from(slot)),
    };
    assert_eq!(events::with(<[SimEvent]>::to_vec), [deleted(1, 0x8001, 2), deleted(3, 0x8003, 5)]);
}

#[test]
fn the_wake_leaf_returns_with_retail_registers() {
    let (mut cpu, mut g) = guest();
    // A weapon (slot 2) held by a biped (slot 1).
    pool(&mut g, &[(1, 0x8001, 0x01, 0), (2, 0x8002, 0x81, 2)]);
    g.set_u32(body_of(2) + 0x14, handle(0x8001, 1)).unwrap();
    cpu.eax = handle(0x8002, 2);
    cpu.ebx = 0x1111;
    cpu.esi = 0x2222;
    let out = drive(&mut cpu, &mut g, &TABLE, crate::points::OBJECT_WAKE.base).unwrap();
    assert_eq!(out, TransferAbi::ret(CALLER));
    assert_eq!(cpu.esp, STACK + 4);
    for slot in [1, 2] {
        assert_ne!(g.u8(HEADERS + 12 * slot + 2).unwrap() & 2, 0, "slot {slot} awake");
    }
    assert_eq!((cpu.eax, cpu.ecx, cpu.edx), (NONE, HEADERS, 2 * 3));
    assert_eq!((cpu.ebx, cpu.esi), (0x1111, 0x2222));
    // ZF and PF from `cmp eax, -1`.
    assert_eq!(cpu.eflags & 0x44, 0x44);
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
fn a_guarded_body_is_called_through_the_runtime() {
    let t = table();
    assert!(!t.guarded(INNER.base));
    assert!(!t.guard(0x1010), "a site is not an entry");
    assert!(!t.guard(GUEST));
    assert!(t.guard(INNER.base));
    assert!(t.guarded(INNER.base) && !t.guarded(OUTER.base));
    let (mut cpu, mut g) = guest();
    let out = drive(&mut cpu, &mut g, &t, OUTER.base).unwrap();
    assert_eq!(out, TransferAbi::call(INNER.base, 0x1010));
    assert_eq!((cpu.eax, cpu.esp), (1, STACK - 4));
    // Entered by its chain, the body runs and returns to outer's site,
    // which resumes in Rust.
    let out = drive(&mut cpu, &mut g, &t, INNER.base).unwrap();
    assert_eq!(out, TransferAbi::call(GUEST, 0x1020));
    assert_eq!(cpu.eax, 12);
    // A tail jump to it leaves too.
    let (mut cpu, mut g) = guest();
    let out = drive(&mut cpu, &mut g, &t, TAIL.base).unwrap();
    assert_eq!(out, TransferAbi::jump(INNER.base));
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
