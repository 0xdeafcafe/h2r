//! Contrail point emission and reclamation: the emission clock and the
//! release of a trail's four point chains. The chains share one datum pool.
//! Guest calls can suspend the walk, so its cursor and scratch handles are
//! materialised in the guest frame at each callout. The point update
//! (`17c540`) stays guest code.
use super::Port;
use guest_layout::points::{Point, Sig};
use crate::sim_native::Result;
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, logic, pop, push};
use runtime::{CpuAbi, Guest, GuestFault};

const DOC: &str = "docs/engine/sim-rewrite.md";
const SPAWN_CLOCK: Point = Point::new("CONTRAIL_POINT_SPAWN_CLOCK", 0x17c040, Sig::Opaque, DOC);
const TEARDOWN: Point = Point::new("CONTRAIL_RELEASE_CHAINS", 0x17c880, Sig::Opaque, DOC);

// Datum arrays installed by the contrail initialiser at 17b7b0.
const TRAILS: u32 = 0x004e_a944;
const POINTS: u32 = 0x004e_a940;
const ONE: u32 = crate::function_eval::constant::ONE;
const NONE: u32 = u32::MAX;
const FREE_DATUM: u32 = 0x16ba40;
const FREED: u32 = 0x17c8d7;

pub(super) const PORTS: &[Port] = &[
    Port { point: &SPAWN_CLOCK, sites: &[], supported: clock_supported, run: spawn_clock },
    Port { point: &TEARDOWN, sites: &[FREED], supported: integer_supported, run: teardown },
];

fn integer_supported(_: &CpuAbi, g: &Guest, _: u32) -> bool {
    !g.has_trap()
}

fn at(base: u32, offset: u32) -> u32 {
    base.wrapping_add(offset)
}

fn datum(g: &Guest, array: u32, handle: u32, size: u32) -> std::result::Result<u32, GuestFault> {
    Ok(at(g.u32(at(array, 0x44))?, (handle & 0xffff) * size))
}

fn definition(g: &Guest, handle: u32) -> std::result::Result<u32, GuestFault> {
    let tags = g.u32(guest_layout::points::TAG_ENTRIES.base)?;
    g.u32(at(tags, (handle & 0xffff) * 16 + 8))
}

fn lahf_test(c: &mut CpuAbi) -> bool {
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    sse::parity(c)
}

fn nonzero(c: &mut CpuAbi, value: u32) -> bool {
    sse::ucomiss(c, value, 0);
    lahf_test(c)
}

fn decrement(c: &mut CpuAbi, value: u32) -> u32 {
    let carry = c.eflags & 1;
    let result = arithmetic(c, value, 1, true);
    c.eflags = (c.eflags & !1) | carry;
    result
}

/// The countdown is measured in seconds. Multiple expirations in a single
/// frame each subtract one rounded binary32 interval from the residual.
struct EmissionClock {
    remaining: u32,
    interval: u32,
    pulses: u32,
}

impl EmissionClock {
    fn tick(&mut self, c: &mut CpuAbi, dt: u32) {
        sse::load(c, 0, dt);
        sse::zero(c, 2);
        let mut remaining_dt = dt;
        while nonzero(c, remaining_dt) {
            if sse::comiss(c, remaining_dt, self.remaining) & 1 != 0 {
                sse::load(c, 1, self.remaining);
                self.remaining = sse::ss(c, Ss::Sub, 1, remaining_dt);
                break;
            }
            remaining_dt = sse::ss(c, Ss::Sub, 0, self.remaining);
            self.pulses += 1;
            self.remaining = self.interval;
        }
    }
}

fn clock_supported(c: &CpuAbi, g: &Guest, _: u32) -> bool {
    if c.mxcsr & 0x1f80 != 0x1f80 || g.has_trap() {
        return false;
    }
    // A malformed effect can request an unbounded emission loop. Bound the
    // native call's work, leaving exceptional clocks to the normal scheduler.
    let inputs = || -> Option<(f32, f32, f32)> {
        let record = datum(g, g.u32(TRAILS).ok()?, c.eax, 0x48).ok()?;
        let tag = definition(g, g.u32(at(record, 4)).ok()?).ok()?;
        let mut rate = f32::from_bits(g.u32(at(tag, 4)).ok()?);
        if g.u8(at(tag, 2)).ok()? & 1 != 0 {
            rate *= f32::from_bits(g.u32(at(record, 0x14)).ok()?);
        }
        let dt = f32::from_bits(g.u32(at(c.esp, 4)).ok()?);
        let left = f32::from_bits(g.u32(at(record, 0x24)).ok()?);
        Some((dt, left, rate))
    };
    inputs().is_some_and(|(dt, left, rate)| {
        dt.is_finite() && left.is_finite() && left >= 0.0 && rate.is_normal()
            && rate > 0.0 && (f64::from(dt) - f64::from(left)) * f64::from(rate) < 2048.0
    }) && g.u32(ONE).ok() == Some(1f32.to_bits())
}

fn spawn_clock(c: &mut CpuAbi, g: &mut Guest, _: u32) -> Result {
    let record = datum(g, g.u32(TRAILS)?, c.eax, 0x48)?;
    let tag = definition(g, g.u32(at(record, 4))?)?;
    sse::load(c, 0, g.u32(at(tag, 4))?);
    push(c, g, c.ebx)?;
    let scaled = g.u8(at(tag, 2))? & 1 != 0;
    c.ebx = pop(c, g)?;
    if scaled {
        sse::load(c, 1, g.u32(at(record, 0x14))?);
        sse::ss_reg(c, Ss::Mul, 1, 0);
        sse::movaps(c, 0, 1);
    }
    sse::load(c, 1, g.u32(ONE)?);
    let interval = sse::ss_reg(c, Ss::Div, 1, 0);
    let countdown = at(record, 0x24);
    let dt = g.u32(at(c.esp, 4))?;
    let mut clock = EmissionClock { remaining: g.u32(countdown)?, interval, pulses: 0 };
    c.eax = tag;
    clock.tick(c, dt);
    if dt << 1 != 0 {
        g.set_u32(countdown, clock.remaining)?;
    }
    c.eax = (c.eax & 0xffff_0000) | (clock.pulses & 0xffff);
    c.ecx = record;
    c.edx = clock.pulses;
    g.ret(c, 4)
}

/// Release the point chains in channel then head-to-tail order, keeping
/// the point pool's allocator and its high-water mark under one owner.
fn teardown(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result {
    if pc == TEARDOWN.base {
        c.ecx = g.u32(TRAILS)?;
        let trail = datum(g, c.ecx, c.eax, 0x48)?;
        for reg in [c.ebx, c.ebp, c.esi, c.edi] {
            push(c, g, reg)?;
        }
        c.edx = g.u32(POINTS)?;
        c.edi = at(trail, 0x38);
        c.ebx = at(trail, 0x30);
        c.ebp = 4;
        c.eax = g.u32(c.edi)?;
    } else {
        c.eax = c.esi;
    }
    loop {
        if c.eax != NONE {
            c.ecx = (c.eax & 0xffff) * 0x38;
            let record = datum(g, c.edx, c.eax, 0x38)?;
            c.esi = g.u32(at(record, 0x34))?;
            // The indexed address calculation leaves the flags a multiply
            // by the record size sets: CF and OF clear, the rest undefined.
            let offset = (c.eax & 0xffff) * 0x38;
            logic(c, c.eax & 0xffff, 1 << 31);
            c.eflags &= !0x801;
            c.eflags_defined = (c.eflags_defined & !0x8d5) | 0x801;
            c.ecx = offset;
            return g.call(c, FREE_DATUM, FREED);
        }
        g.set_u16(c.ebx, 0)?;
        g.set_u32(c.edi, NONE)?;
        c.ebx = at(c.ebx, 2);
        c.edi = arithmetic(c, c.edi, 4, false);
        c.ebp = decrement(c, c.ebp);
        if c.ebp == 0 {
            c.edi = pop(c, g)?;
            c.esi = pop(c, g)?;
            c.ebp = pop(c, g)?;
            c.ebx = pop(c, g)?;
            return g.ret(c, 0);
        }
        c.eax = g.u32(c.edi)?;
    }
}
