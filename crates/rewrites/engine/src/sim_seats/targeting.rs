//! Local-user targeting. The search keeps the candidate array in the guest
//! frame while the BSP walk, sort and line-of-sight callback can see it.
use crate::light_constants::Sse;
use crate::sim_native::{Memory, plan::segment};
use crate::sim_physics::{ARITH, arithmetic, byte, logic, pop, push, spatial::fp_error};
use runtime::{CpuAbi, Guest, GuestFault, TransferAbi};

mod follow;
mod rank;
pub use rank::compare;
pub use follow::{UPDATE_SITES, update};

type Boundary = Result<Option<TransferAbi>, GuestFault>;
type Transfer = Result<TransferAbi, GuestFault>;
const QUERY_PARAMS: u32 = 0x0051_e99c;
const COMPARE: u32 = 0x001a_66f0;
const STRIDE: u32 = 0x60;
const SEARCH_FRAME: u32 = 0x1808;
const FIRST_CANDIDATE: u32 = 0x18;
pub const SEARCH_SITES: &[u32] = &[0x1a537a, 0x1a539b, 0x1a53e1, 0x1a5403, 0x1a5432];

#[inline]
fn at(p: u32, n: u32) -> u32 {
    p.wrapping_add(n)
}

fn scalar(c: &mut CpuAbi, slot: usize, bits: u32) {
    c.xmm[slot] = u128::from(bits).to_le_bytes();
}

fn leave_search(c: &mut CpuAbi, g: &impl Memory, saved_esi: bool, result: u8) -> Transfer {
    if saved_esi {
        c.esi = pop(c, g)?;
    }
    c.edi = pop(c, g)?;
    c.ebp = pop(c, g)?;
    c.ebx = pop(c, g)?;
    c.eax = byte(c.eax, result);
    c.esp = arithmetic(c, c.esp, SEARCH_FRAME, false);
    g.ret(c, 20)
}

fn test_visibility(c: &mut CpuAbi, g: &mut impl Memory, cursor: u32) -> Transfer {
    c.ecx = g.u32(at(c.esp, 0x1824))?;
    c.ebx = cursor.wrapping_sub(12);
    c.eax = g.u32(cursor.wrapping_sub(20))?;
    g.call(c, 0x1a67f0, 0x1a5432)
}

fn select(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Transfer {
    let sp = c.esp;
    match pc {
        0x1a5370 => {
            c.eax = SEARCH_FRAME;
            g.call(c, 0x320560, 0x1a537a) // page-probe and allocate the candidate frame
        }
        0x1a537a => {
            push(c, g, c.ebx)?;
            push(c, g, c.ebp)?;
            c.ebp = g.u32(at(c.esp, 0x1814))?;
            push(c, g, c.edi)?;
            c.edi = c.ecx;
            c.ecx = g.u32(0x4e033c)?;
            c.ebx = byte(c.ebx, 0);
            c.edx = c.edi;
            c.eax = 0;
            logic(c, 0, 1 << 31);
            g.set_u8(at(c.esp, 15), 0)?;
            g.call(c, 0x14a280, 0x1a539b)
        }
        0x1a539b => {
            if c.eax != u32::MAX {
                c.ecx = g.u32(0x4e0348)?;
                c.edx = g.u32(at(c.ecx, 0x30))?;
                let cluster = g.u16(at(c.edx, c.eax.wrapping_mul(8)))?;
                c.eax = (c.eax & 0xffff_0000) | u32::from(cluster);
                if cluster != u16::MAX {
                    crate::sim_native::sub_flags(c, u32::from(cluster), 0xffff, 16);
                    c.edx = g.u32(at(sp, 0x1824))?;
                    push(c, g, c.esi)?;
                    c.ecx = at(c.esp, FIRST_CANDIDATE);
                    push(c, g, c.ecx)?;
                    c.ecx = g.u32(at(c.esp, 0x1828))?;
                    push(c, g, c.edx)?;
                    c.edx = g.u32(at(c.esp, 0x1828))?;
                    for arg in [c.ecx, c.edx, c.edi, c.eax, c.ebp] {
                        push(c, g, arg)?;
                    }
                    return g.call(c, 0x1a54a0, 0x1a53e1);
                }
            }
            leave_search(c, g, false, c.ebx as u8)
        }
        0x1a53e1 => {
            c.esi = c.eax;
            logic(c, c.esi, 1 << 31);
            if c.esi as i32 <= 0 {
                return leave_search(c, g, true, c.ebx as u8);
            }
            push(c, g, COMPARE)?;
            push(c, g, STRIDE)?;
            c.eax = at(c.esp, 0x20);
            push(c, g, c.esi)?;
            push(c, g, c.eax)?;
            g.set_u32(QUERY_PARAMS, c.ebp)?;
            g.call(c, 0x321460, 0x1a5403)
        }
        0x1a5403 => {
            c.esp = at(sp, 16); // cdecl qsort
            c.ebp = 0;
            logic(c, c.esi, 1 << 31);
            if c.esi as i32 <= 0 {
                return leave_search(c, g, true, c.ebx as u8);
            }
            let cursor = at(c.esp, FIRST_CANDIDATE + 20);
            g.set_u32(at(c.esp, 20), cursor)?;
            test_visibility(c, g, cursor)
        }
        0x1a5432 => {
            if c.eax as u8 != 0 {
                // Ordered moves preserve overlapping and reverse-direction copies.
                c.edi = g.u32(at(sp, 0x182c))?;
                c.esi = at(at(sp, FIRST_CANDIDATE), c.ebp.wrapping_mul(STRIDE));
                c.ecx = STRIDE / 4;
                let step = if c.eflags & (1 << 10) == 0 { 4 } else { u32::MAX - 3 };
                while c.ecx != 0 {
                    g.set_u32(c.edi, g.u32(c.esi)?)?;
                    c.edi = at(c.edi, step);
                    c.esi = at(c.esi, step);
                    c.ecx -= 1;
                }
                return leave_search(c, g, true, 1);
            }

            c.ecx = at(g.u32(at(sp, 20))?, STRIDE);
            c.ebp = c.ebp.wrapping_add(1);
            arithmetic(c, c.ebp, c.esi, true);
            g.set_u32(at(sp, 20), c.ecx)?;
            if (c.ebp as i32) < c.esi as i32 {
                test_visibility(c, g, c.ecx)
            } else {
                leave_search(c, g, true, g.u8(at(sp, 19))?)
            }
        }
        _ => unreachable!("undeclared target search continuation"),
    }
}

/// One search segment. Children may suspend and edit the live frame.
pub fn search(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Boundary {
    segment::<32, _>(c, g, pc, 0x1a5370..0x1a549c, SEARCH_SITES, select)
}
