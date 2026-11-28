//! The emitter's particle advance, from PARTICLE_SYSTEM_UPDATE through its
//! six-argument return. Particle motion is gathered into Rust values for one
//! step. Guest callbacks can inspect the result, free the datum, and even
//! relocate the pool, so only the original frame survives those calls.
use super::Port;
use guest_layout::points;
use crate::sim_objects::frame::{pop, push, set_slot, slot};
use crate::sim_objects::sse::{self, Ss};
use crate::sim_physics::{arithmetic, byte, logic};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

const PARTICLE_POOL: u32 = 0x0051_ec84;
const LIFE_END: u32 = 0x0045_dbc0;
const EMISSION_THRESHOLD: u32 = 0x0045_dbd8;
const REST_SPEED_SQUARED: u32 = 0x0045_df78;

const AFTER_LOOKUP: u32 = 0x0024_717d;
const AFTER_REST_QUERY: u32 = 0x0024_7323;
const AFTER_DEATH: u32 = 0x0024_73e8;
const AFTER_FREE: u32 = 0x0024_73f5;
const AFTER_MOVEMENT: u32 = 0x0024_744c;

const EXPIRED: u16 = 1;
const INACTIVE: u16 = 8;
const CHECK_SPEED: u16 = 16;

type Result<T> = std::result::Result<T, GuestFault>;

pub(super) const PORTS: &[Port] = &[Port {
    point: &points::PARTICLE_SYSTEM_UPDATE,
    sites: &[AFTER_LOOKUP, AFTER_REST_QUERY, AFTER_DEATH, AFTER_FREE, AFTER_MOVEMENT],
    supported: |cpu, _, _| crate::sim_objects::supported(cpu),
    run: update,
}];

/// Coordinates and angle use the same binary32 multiply then add, with a
/// rounding after each operation. Keep their original order for MXCSR.
struct Motion {
    position: [u32; 3],
    velocity: [u32; 3],
    angle: u32,
    spin: u32,
}

impl Motion {
    fn read(g: &Guest, p: u32) -> Result<Self> {
        let vec3 = |a| -> Result<_> { Ok([g.u32(a)?, g.u32(a + 4)?, g.u32(a + 8)?]) };
        Ok(Self {
            position: vec3(p + 0x1c)?,
            velocity: vec3(p + 0x28)?,
            angle: g.u32(p + 0x38)?,
            spin: g.u32(p + 0x34)?,
        })
    }

    fn integrate(&mut self, c: &mut CpuAbi) {
        let mut advance = |value: &mut u32, rate| {
            sse::load(c, 0, rate);
            sse::ss_reg(c, Ss::Mul, 0, 3);
            *value = sse::ss(c, Ss::Add, 0, *value);
        };
        for (position, velocity) in self.position.iter_mut().zip(self.velocity) {
            advance(position, velocity);
        }
        advance(&mut self.angle, self.spin);
    }

    fn store(&self, g: &mut Guest, p: u32) -> Result<()> {
        for (i, value) in self.position.iter().enumerate() {
            g.set_u32(p + 0x1c + 4 * i as u32, *value)?;
        }
        g.set_u32(p + 0x38, self.angle)
    }
}

/// XMM state at an external callback includes the partial velocity sums.
fn at_rest(c: &mut CpuAbi, g: &Guest, p: u32) -> Result<bool> {
    sse::load(c, 2, g.u32(p + 0x28)?);
    sse::load(c, 1, g.u32(p + 0x2c)?);
    sse::load(c, 0, g.u32(p + 0x30)?);
    sse::movaps(c, 4, 2);
    sse::ss_reg(c, Ss::Mul, 4, 2);
    sse::movaps(c, 2, 1);
    sse::ss_reg(c, Ss::Mul, 2, 1);
    sse::movaps(c, 1, 0);
    sse::ss_reg(c, Ss::Add, 4, 2);
    sse::ss_reg(c, Ss::Mul, 1, 0);
    let squared = sse::ss_reg(c, Ss::Add, 4, 1);
    Ok(sse::comiss(c, squared, g.u32(REST_SPEED_SQUARED)?) & 0x41 != 0)
}

fn advance_age(c: &mut CpuAbi, g: &mut Guest, p: u32) -> Result<()> {
    let next = g.u32(p + 4)?;
    c.eax = next;
    set_slot(c, g, 0x10, next)?;
    sse::load(c, 0, g.u32(p + 0xc)?);
    sse::ss_reg(c, Ss::Mul, 0, 3);
    let age = sse::ss(c, Ss::Add, 0, g.u32(p + 8)?);
    g.set_u32(p + 8, age)?;
    if sse::comiss(c, age, sse::lane0(c, 1)) & 0x41 != 0 {
        return Ok(());
    }

    let flags = g.u8(c.edx + 0xc)?;
    c.eax = byte(c.eax, flags >> 6);
    let wrap_slot = if flags & 0x41 == 0x41 {
        Some(0x20)
    } else {
        c.edx = slot(c, g, 0x38)?;
        if g.u8(c.edx + 0x17)? & 1 != 0 {
            c.eax = slot(c, g, 0x34)?;
            let emission = g.u32(c.eax + 0x1c)?;
            sse::load(c, 2, emission);
            (sse::comiss(c, emission, g.u32(EMISSION_THRESHOLD)?) & 0x41 == 0).then_some(0x24)
        } else {
            None
        }
    };
    if let Some(scratch) = wrap_slot {
        set_slot(c, g, scratch, age)?;
        c.eax = sse::cvttss2si(c, age);
        sse::load(c, 2, age);
        sse::cvtsi2ss(c, 0, c.eax);
        let remainder = sse::ss_reg(c, Ss::Sub, 2, 0);
        g.set_u32(p + 8, remainder)?;
    } else {
        g.set_u16(p + 2, g.u16(p + 2)? | EXPIRED)?;
        g.set_u8(c.esp + 0x40, 1)?;
    }
    Ok(())
}

fn include_in_bounds(c: &mut CpuAbi, g: &mut Guest, p: u32) -> Result<()> {
    // The high end chooses the new point on ties/unordered; the low end
    // keeps the old endpoint in both cases (including signed-zero ties).
    for high in [false, true] {
        for axis in 0..3 {
            let address = c.ebp + 8 * axis + 4 * u32::from(high);
            let point = g.u32(p + 0x1c + 4 * axis)?;
            let old = g.u32(address)?;
            let larger = sse::comiss(c, old, point) & 0x41 == 0;
            let selected = if larger == high { old } else { point };
            sse::load(c, 0, selected);
            g.set_u32(address, selected)?;
        }
    }
    Ok(())
}

/// Select the active particle's next action. Rest queries and on-death
/// effects run guest code and resume with the live particle in ESI.
fn advance(c: &mut CpuAbi, g: &mut Guest) -> Result<Option<TransferAbi>> {
    let header = g.u32(PARTICLE_POOL)?;
    let p = g.u32(header + 0x44)?.wrapping_add((c.edi & 0xffff) << 6);
    c.esi = p;
    sse::load(c, 3, slot(c, g, 0x3c)?);
    let end = g.u32(LIFE_END)?;
    sse::load(c, 1, end);
    advance_age(c, g, p)?;
    let flags = g.u16(p + 2)?;
    c.eax = (c.eax & 0xffff_0000) | u32::from(flags);
    if flags & (EXPIRED | INACTIVE) == 0 && sse::comiss(c, end, g.u32(p + 8)?) & 1 == 0 {
        if flags & CHECK_SPEED != 0 && at_rest(c, g, p)? {
            c.eax |= u32::from(INACTIVE);
        } else {
            let mut motion = Motion::read(g, p)?;
            motion.integrate(c);
            motion.store(g, p)?;
            c.eax &= 0xffef;
        }
        g.set_u16(p + 2, c.eax as u16)?;
    } else {
        c.eax = byte(c.eax, (c.eax as u8) >> 3);
        let resting = c.eax & 1;
        if resting != 0 {
            logic(c, resting, 0x80);
            c.edx = g.u32(c.ecx)?;
            return Ok(Some(g.call(c, g.u32(c.edx + 0x34)?, AFTER_REST_QUERY)?));
        }
    }
    Ok(None)
}

fn epilogue(c: &mut CpuAbi, g: &Guest) -> Result<TransferAbi> {
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0x18, false);
    g.ret(c, 0x18)
}

fn finish(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.edx = slot(c, g, 0x34)?;
    let expire_emitter = g.u8(c.edx + 0x16)? & 1 != 0;
    c.edi = pop(c, g)?;
    if expire_emitter {
        c.eax = byte(c.eax, g.u8(c.esp + 0x38)?);
        if c.eax as u8 != 0 {
            c.eax = slot(c, g, 0x28)?;
            g.set_u8(c.eax + 0xc, g.u8(c.eax + 0xc)? & !8)?;
        }
    }
    epilogue(c, g)
}

fn movement(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    c.edi = slot(c, g, 0x14)?;
    if c.edi != 0 {
        c.ecx = slot(c, g, 0x28)?;
        c.eax = g.u32(c.ecx + 4)?;
        arithmetic(c, c.eax, u32::MAX, true);
        if c.eax != u32::MAX {
            c.ecx = slot(c, g, 0x38)?;
            push(c, g, c.ecx)?;
            c.ecx = slot(c, g, 0x34)?;
            push(c, g, c.ecx)?;
            push(c, g, c.edx)?;
            return g.call(c, 0x002b_a3a0, AFTER_MOVEMENT);
        }
    }
    finish(c, g)
}

/// Each iteration consumes the handle saved before the previous particle
/// changed. The death callback and allocator run before unlinking. The
/// list owner or predecessor is re-read from the guest frame on return.
fn walk(c: &mut CpuAbi, g: &mut Guest, mut pc: u32) -> Result<TransferAbi> {
    loop {
        if pc == AFTER_FREE {
            c.eax = slot(c, g, 0x14)?;
            c.ebx = 1;
            if c.eax != 0 {
                c.ecx = slot(c, g, 0x10)?;
                g.set_u32(c.eax + 4, c.ecx)?;
            } else {
                c.edx = slot(c, g, 0x10)?;
                c.eax = slot(c, g, 0x2c)?;
                g.set_u32(c.eax + 4, c.edx)?;
            }
        } else {
            if pc == AFTER_REST_QUERY {
                if c.eax as u8 != 0 {
                    g.set_u16(c.esi + 2, g.u16(c.esi + 2)? | EXPIRED)?;
                }
            } else if let Some(call) = advance(c, g)? {
                return Ok(call);
            }
            if g.u8(c.esi + 2)? & 1 != 0 {
                logic(c, 1, 0x80);
                c.eax = slot(c, g, 0x2c)?;
                push(c, g, c.eax)?;
                c.eax = slot(c, g, 0x34)?;
                c.ebx = c.esi;
                return g.call(c, 0x0024_8340, AFTER_DEATH);
            }
            set_slot(c, g, 0x14, c.esi)?;
            include_in_bounds(c, g, c.esi)?;
        }
        c.edi = slot(c, g, 0x10)?;
        c.edx = slot(c, g, 0x30)?;
        if c.edi == u32::MAX {
            c.esi = pop(c, g)?;
            return movement(c, g);
        }
        c.ecx = slot(c, g, 0x1c)?;
        pc = AFTER_LOOKUP;
    }
}

fn update(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    match pc {
        AFTER_MOVEMENT => finish(c, g),
        AFTER_DEATH => {
            c.edx = g.u32(PARTICLE_POOL)?;
            c.eax = c.edi;
            g.call(c, 0x0016_ba40, AFTER_FREE)
        }
        AFTER_FREE | AFTER_REST_QUERY => walk(c, g, pc),
        AFTER_LOOKUP => {
            c.edx = slot(c, g, 0x28)?;
            c.ecx = c.eax;
            c.eax = byte(c.eax, g.u8(c.edx + 0xc)? >> 3);
            c.ebx = 1;
            set_slot(c, g, 0x14, c.ecx)?;
            if c.eax & 1 == 0 {
                return epilogue(c, g);
            }
            c.eax = slot(c, g, 0x24)?;
            push(c, g, c.edi)?;
            c.edi = g.u32(c.eax + 4)?;
            set_slot(c, g, 0x10, 0)?;
            g.set_u8(c.esp + 0x3c, 0)?;
            if c.edi == u32::MAX {
                return movement(c, g);
            }
            push(c, g, c.esi)?;
            walk(c, g, pc)
        }
        _ if pc == points::PARTICLE_SYSTEM_UPDATE.base => {
            c.eax = g.u32(c.eax + 4)?;
            c.esp = c.esp.wrapping_sub(0x18);
            arithmetic(c, c.eax, u32::MAX, true);
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = slot(c, g, 0x38)?;
            let tag = if c.eax == u32::MAX {
                0
            } else {
                c.ecx = g.u32(points::TAG_ENTRIES.base)?;
                // Shift a zero-extended slot by four. CF is clear and
                // the overflow and auxiliary carry flags are undefined.
                c.eax = (c.eax & 0xffff) << 4;
                logic(c, c.eax, 1 << 31);
                c.eflags_defined &= !0x800;
                c.edx = g.u32(c.ecx.wrapping_add(c.eax).wrapping_add(8))?;
                c.edx
            };
            set_slot(c, g, 0x10, tag)?;
            c.eax = slot(c, g, 0x30)?;
            c.edx = g.u32(c.eax + 4)?;
            g.call(c, 0x0013_7bd0, AFTER_LOOKUP)
        }
        _ => Err(GuestFault {
            addr: pc,
            len: 1,
            access: runtime::guest::Access::Execute,
            reason: runtime::guest::FaultReason::Unmapped,
        }),
    }
}

