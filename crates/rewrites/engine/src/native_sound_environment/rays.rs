//! Per-source obstruction, using either two collision rays or a
//! cluster-distance estimate. The portals' weights enter the filter
//! calculation after the geometric contribution.
use super::{BSP, CLUSTER_DISTANCE, low8, low16, signed16};
use crate::sim_objects::{
    Cx, Next, Result, frame,
    sse::{self, Ss},
};
use crate::sim_physics::{arithmetic, logic, spatial};
use runtime::{CpuAbi, Guest};
use sim_sound::environment::ObstructionSpan;

const FORWARD: u32 = 0x221ede;
const REVERSE: u32 = 0x221f42;
const DISTANCE: u32 = 0x222048;
const LENGTH: u32 = 0x222059;
const PORTAL: u32 = 0x2220ea;
pub(super) const SITES: &[u32] = &[FORWARD, REVERSE, DISTANCE, LENGTH, PORTAL];
const RAY: u32 = 0x1de630;
const ONE: u32 = 0x0045_dbc0;
const COLLISION: u32 = 0x004e_0340;
const FRAME: u32 = 0x858;

fn ret(c: &mut CpuAbi, g: &Guest) -> Result<Next> {
    c.ebp = frame::pop(c, g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    Ok(Next::Ret { pop: 8 })
}

fn accumulate(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    sse::load(c, 0, g.u32(c.edi.wrapping_add(0x30))?);
    let total = sse::ss(c, Ss::Add, 0, frame::slot(c, g, 0xc)?);
    c.esi = frame::pop(c, g)?;
    g.set_u32(c.edi.wrapping_add(0x30), total)?;
    c.ebx = frame::pop(c, g)?;
    ret(c, g)
}

fn environment(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    let listener = g.u16(c.ebp.wrapping_add(4))?;
    c.ecx = low16(c.ecx, listener);
    if listener != u16::MAX {
        c.eax = g.u32(c.edi.wrapping_add(0x3c))?;
        if c.eax != u32::MAX {
            c.ecx = signed16(c.ecx);
            if c.eax != c.ecx {
                let direct = g.u8(c.edi.wrapping_add(0x44))?;
                c.edx = low8(c.edx, direct);
                logic(c, u32::from(direct), 0x80);
                let weights = frame::slot(c, g, 0x868)?;
                if direct == 0 {
                    c.edx = weights;
                    frame::push(c, g, c.eax)?;
                    c.eax = c.edi.wrapping_add(0x24);
                    frame::push(c, g, c.eax)?;
                    c.esi = c.ebp.wrapping_add(0x30);
                    frame::push(c, g, c.edx)?;
                    c.ebx = c.esp.wrapping_add(0x1c);
                    return Ok(Next::Call {
                        target: 0x222150,
                        site: PORTAL,
                    });
                }
                sse::zero(c, 0);
                c.ecx = weights;
                g.set_u32(c.edi.wrapping_add(0x34), 0)?;
                c.edx = g.u32(weights.wrapping_add(c.eax.wrapping_mul(4)))?;
                g.set_u32(c.edi.wrapping_add(0x38), c.edx)?;
            }
        }
    }
    accumulate(c, g)
}

fn cast(c: &mut CpuAbi, g: &mut Guest, reverse: bool) -> Result<Next> {
    let base = c.esp;
    let (direction, origin, output, site) = if reverse {
        c.eax = frame::slot(c, g, 0x1c)?;
        c.edx = g.u32(COLLISION)?;
        (0x20, c.ebx, 0x448, REVERSE)
    } else {
        let bsp_index = signed16(u32::from(g.u16(0x0046_86c4)?));
        c.ecx = bsp_index << 5;
        logic(c, c.ecx, 1 << 31);
        c.eflags |= (bsp_index >> 27) & 1;
        c.eflags_defined &= !(1 << 11); // SHL by more than one
        c.eax = g.u32(0x004e_d280)?.wrapping_add(c.ecx).wrapping_add(1);
        c.edx = g.u32(COLLISION)?;
        c.ebx = c.edi.wrapping_add(0x24);
        c.esi = c.ebp.wrapping_add(0x30);
        (0x10, c.esi, 0x2c, FORWARD)
    };
    for axis in 0..3 {
        let offset = 4 * axis;
        let reg = usize::from(reverse && axis == 1);
        let component = if reverse {
            sse::zero(c, reg);
            sse::ss(c, Ss::Sub, reg, frame::slot(c, g, 0x10 + offset)?)
        } else {
            sse::load(c, 0, g.u32(c.ebx.wrapping_add(offset))?);
            sse::ss(c, Ss::Sub, 0, g.u32(c.esi.wrapping_add(offset))?)
        };
        frame::set_slot(c, g, direction + offset, component)?;
    }
    frame::push(c, g, base.wrapping_add(direction))?;
    frame::push(c, g, origin)?;
    frame::push(c, g, c.eax)?;
    if !reverse {
        g.set_u32(base.wrapping_add(0x1c), c.eax)?;
    }
    sse::load(c, 0, g.u32(ONE)?);
    frame::push(c, g, 0x100)?;
    c.eax = base.wrapping_add(output);
    c.ecx = 1;
    Ok(Next::Call { target: RAY, site })
}

fn contacts(c: &mut CpuAbi, g: &mut Guest) -> Result<()> {
    let unit = g.u32(ONE)?;
    let span = ObstructionSpan {
        forward: frame::slot(c, g, 0x2c)?,
        reverse: frame::slot(c, g, 0x448)?,
    };
    let (raw, depth, hits) = span.depth(&mut c.mxcsr, unit);
    sse::load(c, 4, unit);
    sse::load(c, 1, span.reverse);
    sse::load(c, 2, span.forward);
    sse::zero(c, 3);
    sse::load(c, 0, raw);
    frame::set_slot(c, g, 0xc, depth)?;
    if hits {
        g.set_u8(c.edi.wrapping_add(0x45), 1)?;
        for (direction, pos, fraction, dest) in [
            (0x10, c.esi, span.forward, 4),
            (0x20, c.ebx, span.reverse, 0x10),
        ] {
            for axis in 0..3 {
                let offset = 4 * axis;
                sse::load(c, 0, frame::slot(c, g, direction + offset)?);
                sse::ss(c, Ss::Mul, 0, fraction);
                if dest == 0x10 && axis == 0 {
                    g.set_u32(c.edi.wrapping_add(0x1c), span.forward)?;
                }
                let v = sse::ss(c, Ss::Add, 0, g.u32(pos.wrapping_add(offset))?);
                g.set_u32(c.edi.wrapping_add(dest + offset), v)?;
            }
        }
        g.set_u32(c.edi.wrapping_add(0x20), span.reverse)?;
    }
    Ok(())
}

pub(super) fn update(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        FORWARD => {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 != 0 {
                return cast(c, g, true);
            }
        }
        REVERSE => {
            if c.eax as u8 == 0 {
                sse::load(c, 0, g.u32(ONE)?);
                let depth = sse::ss(c, Ss::Sub, 0, frame::slot(c, g, 0x2c)?);
                frame::set_slot(c, g, 0xc, depth)?;
            } else {
                contacts(c, g)?;
            }
        }
        DISTANCE => {
            c.eax = c.edi.wrapping_add(0x24);
            c.ecx = c.ebp.wrapping_add(0x30);
            frame::set_slot(c, g, 0x10, sse::lane0(c, 0))?;
            return Ok(Next::Call {
                target: 0x3ea30,
                site: LENGTH,
            });
        }
        LENGTH => {
            let mut fpu = spatial::load_fpu(c);
            let len = fpu.fstp_m32().map_err(spatial::fp_error)?;
            frame::set_slot(c, g, 0x1c, len)?;
            spatial::finish_fpu(c, g, fpu, LENGTH, c.esp.wrapping_add(0x1c))?;
            sse::load(c, 1, len);
            let minimum = g.u32(0x0045_dc70)?;
            sse::load(c, 2, minimum);
            sse::movaps(c, 0, 1);
            let sum = sse::ss(c, Ss::Add, 0, frame::slot(c, g, 0x10)?);
            sse::comiss(c, sum, minimum);
            if sse::below_or_equal(c) {
                sse::movaps(c, 0, 2);
            }
            sse::ss_reg(c, Ss::Div, 1, 0);
            sse::load(c, 0, g.u32(ONE)?);
            let depth = sse::ss_reg(c, Ss::Sub, 0, 1);
            frame::set_slot(c, g, 0xc, depth)?;
        }
        PORTAL => {
            c.ecx = signed16(u32::from(g.u16(c.ebp.wrapping_add(4))?)).wrapping_mul(0xb0);
            c.edx = g.u32(c.edi.wrapping_add(0x3c))?.wrapping_mul(0xb0);
            let bsp = g.u32(BSP)?;
            c.eax = g.u32(bsp.wrapping_add(0xa0))?;
            let tag = |offset: u32| g.u16(c.eax.wrapping_add(offset).wrapping_add(0x7a));
            let listener_material = tag(c.ecx)?;
            let source_material = tag(c.edx)?;
            c.ecx = low16(c.ecx, listener_material);
            let obstruction = if listener_material == source_material {
                0
            } else {
                frame::slot(c, g, 0x10)?
            };
            sse::load(c, 0, obstruction);
            g.set_u32(c.edi.wrapping_add(0x34), obstruction)?;
            let room = frame::slot(c, g, 0x14)?;
            sse::load(c, 0, room);
            g.set_u32(c.edi.wrapping_add(0x38), room)?;
            return accumulate(c, g);
        }
        _ => {
            c.ecx = g.u32(c.edi.wrapping_add(0x40))?;
            c.esp = c.esp.wrapping_sub(FRAME);
            frame::push(c, g, c.ebp)?;
            c.ebp = frame::slot(c, g, 0x864)?;
            c.eax = g.u32(c.ebp)?;
            if c.eax == c.ecx {
                return ret(c, g);
            }
            let by_portal = g.u8(c.edi.wrapping_add(0x47))?;
            c.eax = low8(c.eax, by_portal);
            sse::zero(c, 0);
            c.edx = g.u32(BSP)?;
            frame::push(c, g, c.ebx)?;
            frame::push(c, g, c.esi)?;
            frame::set_slot(c, g, 0xc, 0)?;
            if by_portal == 0 {
                return cast(c, g, false);
            }
            let direct = g.u8(c.edi.wrapping_add(0x44))?;
            c.eax = low8(c.eax, direct);
            if direct == 0 {
                c.eax = signed16(u32::from(g.u16(c.ebp.wrapping_add(4))?));
                c.ecx = g.u32(c.edi.wrapping_add(0x3c))?;
                if c.eax != c.ecx {
                    arithmetic(c, c.eax, c.ecx, true);
                    return Ok(Next::Call {
                        target: CLUSTER_DISTANCE.base,
                        site: DISTANCE,
                    });
                }
            }
        }
    }
    environment(c, g)
}
