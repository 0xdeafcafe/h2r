//! Sphere against a cluster portal. Reject by plane, then bounding sphere.
//! Survivors are projected onto the dominant plane and tested against the
//! portal polygon. The dominant-axis and polygon helpers are guest calls.
use super::*;
use crate::light_constants::{Error, Sse, X87};
use x87::abi::{arith_op, unary_op};

pub(super) const SITES: [u32; 2] = [0x14a431, 0x14a52e];
const DOMINANT_AXIS: u32 = 0x0012_0850;
const POLYGON_TEST: u32 = 0x0023_a160;
const STRUCTURE_GLOBAL: u32 = 0x004e_033c;
const PROJECTION_AXES: u32 = 0x0044_0b94;
const SCRATCH_SIZE: u32 = 0x420;

pub(super) fn fp_error(_: Error) -> GuestFault {
    GuestFault {
        addr: 0,
        len: 0, access: runtime::guest::Access::Execute,
        reason: runtime::guest::FaultReason::InvalidOperation,
    }
}

pub(super) fn load_fpu(c: &CpuAbi) -> X87 {
    let x = &c.x87;
    X87(x87::abi::X87Abi {
        st: x.st, control: x.control, status: x.status, tag: x.tag, opcode: x.opcode,
        instruction_pointer: x.instruction_pointer, data_pointer: x.data_pointer,
        instruction_selector: x.instruction_selector, data_selector: x.data_selector,
    })
}

// Only the final x87 instruction and the most recent memory operand
// survive at a transfer. Read FOP from the mapped instruction.
pub(super) fn finish_fpu(c: &mut CpuAbi, g: &Guest, x: X87, pc: u32, operand: u32) -> Result<()> {
    let [op, modrm] = g.read::<2>(pc)?;
    c.x87.st = x.0.st;
    c.x87.status = x.0.status;
    c.x87.tag = x.0.tag;
    c.x87.instruction_pointer = pc;
    c.x87.instruction_selector = 8;
    c.x87.opcode = (u16::from(op & 7) << 8) | u16::from(modrm);
    c.x87.data_pointer = operand;
    c.x87.data_selector = 16;
    Ok(())
}

pub(super) fn scalar(c: &mut CpuAbi, index: usize, value: u32) {
    c.xmm[index] = u128::from(value).to_le_bytes();
}

fn leave(c: &mut CpuAbi, g: &Guest, hit: bool) -> Result<TransferAbi> {
    c.edi = pop(c, g)?;
    c.eax = byte(c.eax, u8::from(hit));
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, SCRATCH_SIZE, false);
    g.ret(c, 4)
}

pub(super) fn portal(c: &mut CpuAbi, g: &mut Guest, pc: u32) -> Result<TransferAbi> {
    if pc == SITES[1] {
        let hit = c.eax as u8 != 0;
        c.esi = pop(c, g)?;
        c.ebx = pop(c, g)?;
        return leave(c, g, hit);
    }
    if pc == SITES[0] {
        return project(c, g);
    }
    c.esp = c.esp.wrapping_sub(SCRATCH_SIZE);
    push(c, g, c.ebp)?;
    push(c, g, c.edi)?;
    c.edi = c.ecx;
    c.ecx = g.u32(c.edx.wrapping_add(0x60))?;
    c.edx = g.u32(c.edx.wrapping_add(0x18))?;
    let index = c.eax as i16 as i32 as u32;
    c.ebp = c.ecx.wrapping_add(index.wrapping_mul(36));
    c.ecx = g.u32(c.ebp.wrapping_add(4))?.wrapping_shl(4);
    c.eax = g.u32(c.edx.wrapping_add(12))?.wrapping_add(c.ecx);
    let sp = c.esp;
    g.set_u32(sp.wrapping_add(12), c.ebp)?;
    let (plane, center) = (c.eax, c.edi);
    let mut x = load_fpu(c);
    // (ny*y + nz*z) + x*nx - d. Compare the unrounded distance;
    // the stack retains a separately rounded copy for projection.
    x.fld(g.u32(plane.wrapping_add(4))?).map_err(fp_error)?;
    x.fmul_m(g.u32(center.wrapping_add(4))?).map_err(fp_error)?;
    x.fld(g.u32(plane.wrapping_add(8))?).map_err(fp_error)?;
    x.fmul_m(g.u32(center.wrapping_add(8))?).map_err(fp_error)?;
    x.arith_reg(arith_op::ADD, 1, 0, true).map_err(fp_error)?;
    x.fld(g.u32(center)?).map_err(fp_error)?;
    x.fmul_m(g.u32(plane)?).map_err(fp_error)?;
    x.arith_reg(arith_op::ADD, 1, 0, true).map_err(fp_error)?;
    x.arith_m(arith_op::SUB, g.u32(plane.wrapping_add(12))?).map_err(fp_error)?;
    g.set_u32(sp.wrapping_add(8), x.fst_m32().map_err(fp_error)?)?;
    x.unary(unary_op::ABS).map_err(fp_error)?;
    let radius_at = sp.wrapping_add(SCRATCH_SIZE + 12);
    let radius = g.u32(radius_at)?;
    x.fld(radius).map_err(fp_error)?;
    let compare = x.fcomip(1).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    finish_fpu(c, g, x, 0x14a3be, radius_at)?;
    if compare & 0x41 != 0 {
        return leave(c, g, false);
    }

    let mut s = Sse { mxcsr: c.mxcsr };
    let delta = |s: &mut Sse, off| -> Result<u32> {
        s.sub(g.u32(c.ebp.wrapping_add(8 + off))?, g.u32(center.wrapping_add(off))?)
            .map_err(fp_error)
    };
    let dz = delta(&mut s, 8)?;
    let dx = delta(&mut s, 0)?;
    let dy = delta(&mut s, 4)?;
    let reach = s.add(g.u32(c.ebp.wrapping_add(20))?, radius).map_err(fp_error)?;
    let zz = s.mul(dz, dz).map_err(fp_error)?;
    let xx = s.mul(dx, dx).map_err(fp_error)?;
    let yy = s.mul(dy, dy).map_err(fp_error)?;
    let zx = s.add(zz, xx).map_err(fp_error)?;
    let dist = s.add(zx, yy).map_err(fp_error)?;
    let rr = s.mul(reach, reach).map_err(fp_error)?;
    let comparison = s.comiss(rr, dist).map_err(fp_error)?;
    c.mxcsr = s.mxcsr;
    for (i, v) in [rr, dy, xx, reach, dist].into_iter().enumerate() { scalar(c, i, v); }
    if comparison & 0x41 != 0 {
        return leave(c, g, false);
    }
    c.eflags = (c.eflags & !ARITH) | comparison;
    c.eflags_defined |= ARITH;
    c.eax = g.u32(STRUCTURE_GLOBAL)?;
    push(c, g, c.ebx)?;
    push(c, g, c.esi)?;
    let base = g.u32(c.eax.wrapping_add(12))?;
    c.esi = arithmetic(c, base, c.ecx, false);
    c.eax = c.esi;
    g.call(c, DOMINANT_AXIS, SITES[0])
}

fn project(c: &mut CpuAbi, g: &mut Guest) -> Result<TransferAbi> {
    let sp = c.esp;
    let axis = c.eax as i16 as i32 as u32;
    let mut s = Sse { mxcsr: c.mxcsr };
    let positive = s.above(g.u32(c.esi.wrapping_add(axis.wrapping_mul(4)))?, 0).map_err(fp_error)?;
    let offset = s.sub(0, g.u32(sp.wrapping_add(16))?).map_err(fp_error)?;
    c.ebx = g.u32(c.ebp.wrapping_add(28))?;
    let projection = PROJECTION_AXES.wrapping_add(axis.wrapping_mul(2).wrapping_add(u32::from(positive)).wrapping_mul(6));
    for i in 0..3 {
        let term = s.mul(g.u32(c.esi.wrapping_add(i * 4))?, offset).map_err(fp_error)?;
        let v = s.add(term, g.u32(c.edi.wrapping_add(i * 4))?).map_err(fp_error)?;
        g.set_u32(sp.wrapping_add(28 + i * 4), v)?;
        scalar(c, 1, v);
    }
    let a = (g.u16(projection)? as i16 as i32 as u32).wrapping_mul(4);
    let b = (g.u16(projection.wrapping_add(2))? as i16 as i32 as u32).wrapping_mul(4);
    for (i, axis) in [a, b].into_iter().enumerate() {
        let v = g.u32(sp.wrapping_add(28).wrapping_add(axis))?;
        g.set_u32(sp.wrapping_add(40 + i as u32 * 4), v)?;
    }
    c.edi = 0;
    if (c.ebx as i32) > 0 {
        c.esi = g.u32(c.ebp.wrapping_add(32))?;
        g.set_u32(sp.wrapping_add(24), c.esi)?;
        c.eax = 0;
        loop {
            let vertices = g.u32(sp.wrapping_add(24))?;
            c.esi = sp.wrapping_add(48).wrapping_add(c.eax.wrapping_mul(8));
            let vertex = vertices.wrapping_add(c.eax.wrapping_mul(12));
            let u = g.u32(vertex.wrapping_add(a))?;
            let v = g.u32(vertex.wrapping_add(b))?;
            c.edi = c.edi.wrapping_add(1);
            g.set_u32(c.esi.wrapping_add(4), v)?;
            c.eax = c.edi as i16 as i32 as u32;
            g.set_u32(c.esi, u)?;
            if c.eax as i32 >= c.ebx as i32 { break; }
        }
        c.ebp = g.u32(sp.wrapping_add(20))?;
    } else {
        c.eax = projection.wrapping_sub(PROJECTION_AXES);
    }
    c.edx = b;
    let radius = g.u32(sp.wrapping_add(SCRATCH_SIZE + 20))?;
    let dist = g.u32(sp.wrapping_add(16))?;
    let mut x = load_fpu(c);
    x.fld(radius).map_err(fp_error)?;
    x.fmul_m(radius).map_err(fp_error)?;
    x.fld(dist).map_err(fp_error)?;
    x.fmul_m(dist).map_err(fp_error)?;
    x.arith_reg(arith_op::SUB, 1, 0, true).map_err(fp_error)?;
    c.ecx = u32::from(g.u16(c.ebp.wrapping_add(28))?);
    c.edi = sp.wrapping_add(40);
    push(c, g, c.ecx)?;
    x.unary(unary_op::SQRT).map_err(fp_error)?;
    c.ecx = sp.wrapping_add(48);
    let cross_radius = x.fstp_m32().map_err(fp_error)?;
    g.set_u32(sp.wrapping_add(20), cross_radius)?;
    scalar(c, 0, cross_radius);
    logic(c, 0, 1 << 31);
    finish_fpu(c, g, x, 0x14a51f, sp.wrapping_add(20))?;
    c.mxcsr = s.mxcsr;
    g.call(c, POLYGON_TEST, SITES[1])
}
