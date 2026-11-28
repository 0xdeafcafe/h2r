//! One effect's event clock. A tick can cross an event's delay, run its
//! parts, cross its duration, then start its successor, all in the same
//! call. Each callout gets the current clock in the effect datum. In
//! particular damage, object creation and random draws can see every
//! preceding event's writes. Only the current time slice lives in Rust.
use super::Port;
use guest_layout::points::{Loc, Point, Sig};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::spatial::{finish_fpu, fp_error, load_fpu};
use crate::sim_physics::{arithmetic, byte, logic, pop, push};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

const EFFECT_POOL: u32 = 0x004e_a93c;
/// Mapped scalar used for an exhausted time budget and a new event's +68.
const END_OF_STEP: u32 = 0x0045_dbcc;
const STEP: Point = Point::new(
    "effect_step_events",
    0x0017_92b0,
    Sig::Fn {
        args: &[("effect", Loc::Stack(0)), ("dt", Loc::Stack(1))],
        ret: Some(Loc::Al),
        pop: 8,
    },
    "docs/engine/sim-rewrite.md",
);

// Return addresses, also used by the lifted function when a child yields.
const EMITTERS: u32 = 0x1793ae;
const LOCATIONS: u32 = 0x1793be;
const SELECT: u32 = 0x1793cc;
const START: u32 = 0x1793e0;
const RANDOM_STREAM: u32 = 0x179430;
const DURATION: u32 = 0x179437;
const PARTS: u32 = 0x17944b;
const BEAMS: u32 = 0x179452;
const END: u32 = 0x179481;
const SITES: &[u32] = &[EMITTERS, LOCATIONS, SELECT, START, RANDOM_STREAM, DURATION, PARTS, BEAMS, END];

pub(super) const PORTS: &[Port] = &[Port {
    point: &STEP,
    sites: SITES,
    supported: |cpu, _, _| crate::sim_native::supported(cpu),
    run: step,
}];

type Result<T> = std::result::Result<T, GuestFault>;

#[derive(Clone, Copy)]
struct EventClock {
    elapsed: u32,
    end: u32,
}

struct TimeSlice {
    elapsed: u32,
    consumed: u32,
    remaining: u32,
    reached_end: bool,
}

impl EventClock {
    /// Preserve the guest's two ordered comparisons. For unordered
    /// operands selecting the consumed time and reaching the end take
    /// different branches. Even a zero length event consumes an iteration.
    fn advance(self, c: &mut CpuAbi, dt: u32, exhausted: u32) -> TimeSlice {
        sse::load(c, 1, self.end);
        let until_end = sse::ss(c, Ss::Sub, 1, self.elapsed);
        let consumed = if sse::comiss(c, until_end, dt) & 0x41 == 0 { dt } else { until_end };
        sse::load(c, 3, self.elapsed);
        let reached_end = sse::comiss(c, dt, until_end) & 1 == 0;
        let (elapsed, remaining) = if reached_end {
            (self.end, sse::ss(c, Ss::Sub, 0, until_end))
        } else {
            sse::load(c, 1, self.elapsed);
            let elapsed = sse::ss(c, Ss::Add, 1, dt);
            sse::load(c, 0, exhausted);
            (elapsed, exhausted)
        };
        TimeSlice { elapsed, consumed, remaining, reached_end }
    }
}

fn addr(base: u32, offset: u32) -> u32 { base.wrapping_add(offset) }

fn finish(c: &mut CpuAbi, g: &mut Guest, saved_ebx: bool) -> Result<TransferAbi> {
    if saved_ebx { c.ebx = pop(c, g)?; }
    c.eax = u32::from(c.esi == 0);
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 12, false);
    g.ret(c, 8)
}

fn clear_and_finish(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    let flags = addr(c.esi, 3);
    g.set_u8(flags, g.u8(flags)? & !2)?;
    finish(c, g, true)
}

fn spawn_parts(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.ecx = byte(c.ecx, g.u8(addr(c.esi, 2))? >> 3);
    logic(c, c.ecx & 1, 0x80);
    if c.ecx & 1 == 0 {
        push(c, g, c.esi)?;
        g.call(c, 0x17a380, PARTS)
    } else {
        c.edx = c.esi;
        g.call(c, 0x17add0, BEAMS)
    }
}

fn start_duration(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    let event = g.u16(addr(c.esi, 0x5c))? as i16 as i32 as u32;
    c.eax = g.u32(addr(c.ebp, 0x18))?;
    let flags = addr(c.esi, 2);
    g.set_u8(flags, g.u8(flags)? | 1)?;
    c.ecx = addr(c.eax, event.wrapping_mul(0x38));
    sse::load(c, 0, g.u32(END_OF_STEP)?);
    g.set_u32(addr(c.esi, 0x60), sse::lane0(c, 2))?;
    g.set_u32(addr(c.esi, 0x68), sse::lane0(c, 0))?;
    let lo = g.u32(addr(c.ecx, 0x10))?;
    let hi = g.u32(addr(c.ecx, 0x14))?;
    sse::load(c, 0, lo);
    let equal = sse::ucomiss(c, lo, hi) & 0x44 == 0x40;
    let tested = c.eflags & 0x44;
    logic(c, tested, 0x80);
    c.eax = lo;
    if equal {
        g.set_u32(addr(c.esi, 0x64), lo)?;
        return spawn_parts(c, g);
    }
    c.edx = hi;
    push(c, g, hi)?;
    push(c, g, lo)?;
    c.eax = g.u32(addr(c.esi, 4))?;
    g.call(c, 0x177c20, RANDOM_STREAM)
}

fn update_locations(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<TransferAbi>> {
    c.edx = byte(c.edx, g.u8(addr(c.esi, 2))? >> 3);
    logic(c, c.edx & 1, 0x80);
    if c.edx & 1 == 0 {
        Ok(Some(g.call(c, 0x179fb0, LOCATIONS)?))
    } else {
        Ok(None)
    }
}

fn select_next(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<TransferAbi>> {
    logic(c, c.ebx & 0xff, 0x80);
    if c.ebx as u8 != 0 {
        push(c, g, c.esi)?;
        Ok(Some(g.call(c, 0x179190, SELECT)?))
    } else {
        Ok(None)
    }
}

fn advance(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<TransferAbi>> {
    let flags = g.u16(addr(c.esi, 2))?;
    c.eax = (c.eax & !0xffff) | u32::from(flags);
    c.ecx = byte(c.ecx, (flags as u8) >> 2);
    if flags & 4 != 0 || g.u32(addr(c.esp, 0x10))? as i32 >= 8 {
        return Ok(Some(clear_and_finish(c, g)?));
    }

    let dt = sse::lane0(c, 0);
    let clock = EventClock {
        elapsed: g.u32(addr(c.esi, 0x60))?,
        end: g.u32(addr(c.esi, 0x64))?,
    };
    let slice = clock.advance(c, dt, g.u32(END_OF_STEP)?);
    g.set_u32(addr(c.esp, 0x18), clock.elapsed)?;
    g.set_u32(addr(c.esp, 0x14), slice.consumed)?;
    g.set_u32(addr(c.esi, 0x60), slice.elapsed)?;
    if slice.reached_end { c.edx = clock.end; }
    c.ebx = byte(c.ebx, u8::from(slice.reached_end));
    g.set_u32(addr(c.esp, 0x24), slice.remaining)?;

    if g.u8(addr(c.esi, 2))? & 1 != 0 {
        c.eax = byte(c.eax, (c.eax as u8) >> 3);
        logic(c, c.eax & 1, 0x80);
        if c.eax & 1 == 0 {
            c.eax = g.u32(addr(c.esp, 0x14))?;
            c.ecx = g.u32(addr(c.esp, 0x18))?;
            push(c, g, c.eax)?;
            push(c, g, c.ecx)?;
            push(c, g, c.edi)?;
            return Ok(Some(g.call(c, guest_layout::points::EFFECT_STEP_EMITTERS.base, EMITTERS)?));
        }
        if let Some(xfer) = update_locations(c, g)? { return Ok(Some(xfer)); }
        return select_next(c, g);
    }
    if slice.reached_end { return Ok(Some(start_duration(c, g)?)); }
    Ok(None)
}

fn step(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        p if p == STEP.base => {
            c.eax = g.u32(EFFECT_POOL)?;
            c.ecx = g.u32(addr(c.eax, 0x44))?;
            c.esp = c.esp.wrapping_sub(12);
            c.edx = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
            push(c, g, c.ebp)?;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = g.u32(addr(c.esp, 0x1c))?;
            c.esi = addr(c.ecx, (c.edi & 0xffff) * 0x190);
            c.eax = byte(c.eax, g.u8(addr(c.esi, 2))? >> 6);
            c.ecx = (g.u32(addr(c.esi, 4))? & 0xffff) << 4;
            c.ebp = g.u32(addr(addr(c.edx, c.ecx), 8))?;
            if c.eax & 1 != 0 { return finish(c, g, false); }
            let dt = g.u32(addr(c.esp, 0x20))?;
            sse::load(c, 0, dt);
            sse::zero(c, 2);
            let negative = sse::comiss(c, dt, 0) & 1 != 0;
            push(c, g, c.ebx)?;
            g.set_u32(addr(c.esp, 0x10), 0)?;
            if negative { return clear_and_finish(c, g); }
            if let Some(xfer) = advance(c, g)? { return Ok(xfer); }
        }
        EMITTERS => {
            if let Some(xfer) = update_locations(c, g)? { return Ok(xfer); }
            if let Some(xfer) = select_next(c, g)? { return Ok(xfer); }
        }
        LOCATIONS => {
            if let Some(xfer) = select_next(c, g)? { return Ok(xfer); }
        }
        SELECT => {
            let count = g.u32(addr(c.ebp, 0x14))?;
            arithmetic(c, c.eax, count, true);
            if (c.eax as i32) >= count as i32 {
                c.eax = c.edi;
                return g.call(c, 0x177610, END);
            }
            c.edx = g.u32(addr(c.esp, 0x20))?;
            c.edi = c.eax;
            return g.call(c, 0x1782a0, START);
        }
        START => c.edi = g.u32(addr(c.esp, 0x20))?,
        RANDOM_STREAM => {
            c.ecx = c.eax;
            return g.call(c, 0x259d0, DURATION);
        }
        DURATION => {
            let dest = addr(c.esi, 0x64);
            let mut fp = load_fpu(c);
            let value = fp.fstp_m32().map_err(fp_error)?;
            g.set_u32(dest, value)?;
            finish_fpu(c, g, fp, DURATION, dest)?;
            return spawn_parts(c, g);
        }
        PARTS => {
            c.edx = c.esi;
            return g.call(c, 0x17add0, BEAMS);
        }
        BEAMS => {
            let current = g.u16(addr(c.esi, 0x5c))?;
            c.edx = (c.edx & !0xffff) | u32::from(current);
            g.set_u16(addr(c.esi, 0x5e), current)?;
        }
        END if c.eax as u8 != 0 => {
            c.esi = 0;
            return finish(c, g, true);
        }
        END => return clear_and_finish(c, g),
        _ => return Err(GuestFault {
            addr: pc, len: 1, access: runtime::guest::Access::Execute,
            reason: runtime::guest::FaultReason::InvalidOperation,
        }),
    }
    loop {
        c.eax = g.u32(addr(c.esp, 0x10))?.wrapping_add(1);
        let dt = g.u32(addr(c.esp, 0x24))?;
        sse::load(c, 0, dt);
        sse::zero(c, 2);
        let negative = sse::comiss(c, dt, 0) & 1 != 0;
        g.set_u32(addr(c.esp, 0x10), c.eax)?;
        if negative { return clear_and_finish(c, g); }
        if let Some(xfer) = advance(c, g)? { return Ok(xfer); }
    }
}
