//! The wake leaves and the early-mover wake (docs/engine/object-update.md,
//! "Slice 1: walks and dispatchers").
//!
//! `b7360`, `baf80`, `bba20` and `bc7b0` call no guest code: their bodies
//! run `sim_objects::wake` to the end and return with the registers and
//! flags retail returns with, leaving the stack below `esp` as retail
//! leaves it (the pushes and return addresses of the leaves and their
//! calls to each other). `109580` calls the stack probe `320560`, a
//! guest call, and the spatial visit `bb050` (a body, `callouts`), so it
//! is a segment body.

use super::{Cx, Next, Result, unowned};
use guest_layout::points;
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use guest_layout::{Fault, Mem, MemMut};
use runtime::guest::{Access, FaultReason};
use runtime::{CpuAbi, Guest, GuestFault};
use ::sim_objects::wake::{self as kernels, Exit, Flags, Stack};
use std::cell::RefCell;

const NONE: u32 = u32::MAX;

/// The runtime's guest memory as a `guest_layout` memory, keeping the
/// first fault with its reason so the body reports what the guest access
/// would have.
struct Exact<'a> {
    g: &'a mut Guest,
    fault: RefCell<Option<GuestFault>>,
}

impl<'a> Exact<'a> {
    fn new(g: &'a mut Guest) -> Self {
        Exact { g, fault: RefCell::new(None) }
    }

    fn keep(&self, f: GuestFault) {
        self.fault.borrow_mut().get_or_insert(f);
    }

    /// The fault a kernel stopped at, as the guest would see it.
    fn fault(self, f: Fault) -> GuestFault {
        self.fault.into_inner().unwrap_or(GuestFault {
            addr: f.addr,
            len: f.len as usize,
            access: if f.write { Access::Write } else { Access::Read },
            reason: FaultReason::Unmapped,
        })
    }
}

impl Mem for Exact<'_> {
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> std::result::Result<(), Fault> {
        let len = buf.len() as u32;
        self.g.read_bytes(addr, buf, 1).map_err(|f| {
            self.keep(f);
            Fault { addr, len, write: false }
        })
    }
}

impl MemMut for Exact<'_> {
    fn write_bytes(&mut self, addr: u32, data: &[u8]) -> std::result::Result<(), Fault> {
        let len = data.len() as u32;
        match self.g.write_bytes(addr, data, 1) {
            Ok(()) => Ok(()),
            Err(f) => {
                self.keep(f);
                Err(Fault { addr, len, write: true })
            }
        }
    }
}

/// The leaf's frame: entered with `esp` at its return address.
fn stack(c: &CpuAbi) -> Stack {
    Stack { esp: c.esp, ebx: c.ebx, esi: c.esi, edi: c.edi }
}

/// Run a kernel over the guest and put its exit in the registers.
fn leaf(
    c: &mut CpuAbi,
    g: &mut Guest,
    kernel: impl FnOnce(&mut Exact<'_>) -> std::result::Result<Exit, Fault>,
) -> Result<()> {
    let mut m = Exact::new(g);
    let exit = match kernel(&mut m) {
        Ok(exit) => exit,
        Err(f) => return Err(m.fault(f)),
    };
    c.eax = exit.eax;
    c.ecx = exit.ecx;
    c.edx = exit.edx;
    match exit.flags {
        Flags::NoneCompared => {
            arithmetic(c, NONE, NONE, true);
        }
        Flags::AwakeTested => logic(c, 2, 0x80),
    }
    Ok(())
}

/// `b7360 (object: eax)`.
pub(super) fn wake(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != points::OBJECT_WAKE.base {
        return Err(unowned(pc));
    }
    let (object, s) = (cx.cpu.eax, stack(cx.cpu));
    leaf(cx.cpu, cx.g, |m| kernels::wake(m, object, s))?;
    Ok(Next::Ret { pop: 0 })
}

/// `baf80 (object: ecx) -> eax`.
pub(super) fn root(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != points::OBJECT_ROOT.base {
        return Err(unowned(pc));
    }
    let (object, edx) = (cx.cpu.ecx, cx.cpu.edx);
    leaf(cx.cpu, cx.g, |m| kernels::root(m, object, edx))?;
    Ok(Next::Ret { pop: 0 })
}

/// `bba20 (object: eax)`.
pub(super) fn wake_for_post(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != points::OBJECT_WAKE_FOR_POST.base {
        return Err(unowned(pc));
    }
    let (object, s) = (cx.cpu.eax, stack(cx.cpu));
    leaf(cx.cpu, cx.g, |m| kernels::wake_for_post(m, object, s))?;
    Ok(Next::Ret { pop: 0 })
}

/// `bc7b0 (object: stack 0)`; `ret 4`.
pub(super) fn wake_children(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != points::OBJECT_WAKE_CHILDREN.base {
        return Err(unowned(pc));
    }
    let object = cx.g.u32(cx.cpu.esp.wrapping_add(4))?;
    let s = stack(cx.cpu);
    leaf(cx.cpu, cx.g, |m| kernels::wake_children(m, object, s))?;
    Ok(Next::Ret { pop: 4 })
}

/// The stack probe the early-mover wake makes its `1004`-byte frame with.
const STACK_PROBE: u32 = 0x0032_0560;
const FRAME: u32 = 0x1004;
/// The most results the spatial visit may return.
const RESULTS: u32 = 0x400;
/// All six partition lists; no type filter.
const LISTS: u32 = 0x3f;

/// After the probe, the visit, the wake and the wake for post.
pub(super) const EARLY_MOVER_SITES: [u32; 4] = [0x10958a, 0x1095c7, 0x109622, 0x109638];

/// `109580 (mover: stack 0)`; `ret 4`. Asks the spatial index for the
/// objects in the mover's sphere (`bb050`, body `+28` location, `+30`
/// centre, `+3c` radius) into a `400`-entry buffer in its frame, and for
/// each result that is ACTIVE, neither NEW nor DELETE, and not yet woken
/// this tick (body `+c0` bit 2): sets that bit, stores the mover at body
/// `+b8`, wakes it (`b7360`), and for weapons, equipment, garbage and
/// projectiles also `bba20` and NEW.
///
/// Frame, with `esp` after its pushes (`ebx`, `ebp`, `esi`, `edi`): the
/// count at `[esp+10]`, the buffer at `[esp+14]`, the mover at
/// `[esp+1018]`. `ebx` holds the objects array across the calls, `ebp`
/// the result index, `esi` the result's header and `edi` its handle.
pub(super) fn early_mover_wake(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        _ if pc == points::OBJECT_EARLY_MOVER_WAKE.base => {
            c.eax = FRAME;
            Ok(Next::Call { target: STACK_PROBE, site: EARLY_MOVER_SITES[0] })
        }
        0x10958a => {
            c.eax = g.u32(c.esp.wrapping_add(FRAME + 4))?;
            push(c, g, c.ebx)?;
            c.ebx = g.u32(points::OBJECTS.base)?;
            c.ecx = g.u32(c.ebx.wrapping_add(guest_layout::datum::layout::DATA))?;
            push(c, g, c.ebp)?;
            c.eax &= 0xffff;
            push(c, g, RESULTS)?;
            c.eax *= 3;
            c.eax = g.u32(c.ecx.wrapping_add(c.eax * 4 + 8))?;
            c.ecx = g.u32(c.eax.wrapping_add(0x3c))?;
            c.edx = c.esp.wrapping_add(0x10);
            push(c, g, c.edx)?;
            push(c, g, c.ecx)?;
            c.edx = c.eax.wrapping_add(0x30);
            push(c, g, c.edx)?;
            c.eax = arithmetic(c, c.eax, 0x28, false);
            push(c, g, c.eax)?;
            push(c, g, LISTS)?;
            push(c, g, 0)?;
            Ok(Next::Call { target: points::OBJECT_SPATIAL_VISIT.base, site: EARLY_MOVER_SITES[1] })
        }
        0x1095c7 => {
            c.eax = c.eax as u16 as i16 as i32 as u32;
            c.ebp = 0;
            logic(c, c.eax, 1 << 31);
            g.set_u32(c.esp.wrapping_add(8), c.eax)?;
            if c.eax as i32 <= 0 {
                return early_mover_ret(c, g, false);
            }
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            early_mover_results(c, g)
        }
        0x109622 => {
            let kind = g.u8(c.esi.wrapping_add(3))?;
            c.ecx = byte(c.ecx, kind);
            c.edx = 1u32 << (kind & 31);
            logic(c, c.edx & 0x3c, 0x80);
            if c.edx & 0x3c != 0 {
                c.eax = c.edi;
                return Ok(Next::Call { target: points::OBJECT_WAKE_FOR_POST.base, site: EARLY_MOVER_SITES[3] });
            }
            early_mover_next(c, g)
        }
        0x109638 => {
            let at = c.esi.wrapping_add(2);
            let f = g.u8(at)? | 0x08;
            g.set_u8(at, f)?;
            logic(c, u32::from(f), 0x80);
            early_mover_next(c, g)
        }
        _ => Err(unowned(pc)),
    }
}

/// `1095d6`: the result at index `ebp`.
fn early_mover_results(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    loop {
        c.edi = g.u32(c.esp.wrapping_add(c.ebp.wrapping_mul(4)).wrapping_add(0x14))?;
        c.ecx = g.u32(c.ebx.wrapping_add(guest_layout::datum::layout::DATA))?;
        c.eax = (c.edi & 0xffff) * 3;
        c.esi = c.ecx.wrapping_add(c.eax * 4);
        let flags = g.u8(c.esi.wrapping_add(2))?;
        c.eax = byte(c.eax, flags);
        if flags & 1 != 0 && flags & 0x18 == 0 {
            c.edx = g.u32(c.esi.wrapping_add(8))?;
            let latch = g.u8(c.edx.wrapping_add(0xc0))? >> 2;
            c.eax = byte(c.eax, latch);
            if latch & 1 == 0 {
                c.ecx = g.u32(c.esp.wrapping_add(0x1018))?;
                c.eax = c.edx;
                let at = c.eax.wrapping_add(0xc0);
                let f = g.u8(at)? | 0x04;
                g.set_u8(at, f)?;
                logic(c, u32::from(f), 0x80);
                g.set_u32(c.eax.wrapping_add(0xb8), c.ecx)?;
                c.eax = c.edi;
                return Ok(Next::Call { target: points::OBJECT_WAKE.base, site: EARLY_MOVER_SITES[2] });
            }
        }
        if !early_mover_advance(c, g)? {
            return early_mover_ret(c, g, true);
        }
    }
}

/// `10963c`, then the next result or the return.
fn early_mover_next(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    if early_mover_advance(c, g)? {
        return early_mover_results(c, g);
    }
    early_mover_ret(c, g, true)
}

/// `10963c`: `mov eax, [esp+10]; inc ebp; cmp ebp, eax; jl`.
fn early_mover_advance(c: &mut CpuAbi, g: &Guest) -> Result<bool> {
    c.eax = g.u32(c.esp.wrapping_add(0x10))?;
    c.ebp = c.ebp.wrapping_add(1);
    arithmetic(c, c.ebp, c.eax, true);
    Ok((c.ebp as i32) < c.eax as i32)
}

/// `109645` (with `esi`, `edi` pushed) or `109647`: restore, drop the
/// frame, `ret 4`.
fn early_mover_ret(c: &mut CpuAbi, g: &Guest, pushed: bool) -> Result<Next> {
    if pushed {
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
    }
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 4 })
}
