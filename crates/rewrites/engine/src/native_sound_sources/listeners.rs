//! Selection of a listener for an impulse or looping sound (127d00)
//! and its source distance (127e20). The mixer receives a position
//! relative to the listener chosen here.
use super::*;
use crate::sim_physics::spatial::{finish_fpu, fp_error, load_fpu};
use guest_layout::sound::SoundListener;
use x87::abi::{arith_op, unary_op};

pub(super) static SELECT: Point = Point::new(
    "SOUND_SELECT_LISTENER",
    0x127d00,
    Sig::Fn {
        args: &[
            ("source", Loc::Stack(0)),
            ("range", Loc::Stack(1)),
            ("distance", Loc::Stack(2)),
        ],
        ret: Some(Loc::Eax),
        pop: 12,
    },
    DOC,
);
pub(super) static DISTANCE: Point = Point::new(
    "SOUND_SOURCE_DISTANCE",
    0x127e20,
    Sig::Fn {
        args: &[("source", Loc::Ecx), ("user", Loc::Edx)],
        ret: Some(Loc::Xmm0),
        pop: 0,
    },
    DOC,
);
const FAR_DISTANCE: u32 = 0x44aed4;
const LISTENERS: u32 = 0x88;
const ACTIVE: u32 = 6;
const USER_COUNT: u32 = 4;

fn first_mapped(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    c.ecx = g.u32(points::PLAYER_GLOBALS.base)?.wrapping_add(12);
    c.eax = 0;
    while c.eax < USER_COUNT {
        if g.u32(c.ecx)? != u32::MAX {
            c.ebx = c.eax;
            break;
        }
        c.eax += 1;
        c.ecx = c.ecx.wrapping_add(4);
    }
    Ok(())
}

pub(super) fn select(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    match pc {
        0x127d00 => {
            frame::push(c, g, c.ebx)?;
            frame::push(c, g, c.ebp)?;
            c.ebp = frame::slot(c, g, 12)?;
            c.eax = byte(c.eax, g.u8(c.ebp + 3)? & 0xf);
            c.ebx = u32::MAX;
            if c.eax as u8 != 1 {
                first_mapped(c, g)?;
                sse::zero(c, 0);
                return finish_selection(c, g);
            }
            sse::load(c, 5, g.u32(FAR_DISTANCE)?);
            frame::push(c, g, c.esi)?;
            frame::push(c, g, c.edi)?;
            c.edi = g.u32(points::SOUND_GLOBALS.base)?;
            sse::movaps(c, 4, 5);
            c.esi = 0;
            frame::set_slot(c, g, 0x14, sse::lane0(c, 4))?;
            c.edi = arithmetic(c, c.edi, LISTENERS + ACTIVE, false);
        }
        0x127d6f => {
            let flags = sse::comiss(c, sse::lane0(c, 4), sse::lane0(c, 0));
            if flags & 0x41 == 0 {
                c.ebx = c.esi;
                sse::movaps(c, 4, 0);
            }
            advance_listener(c);
        }
        0x127da3 => {
            if c.eax as u8 != 0 {
                c.ebx = u32::MAX;
            }
            c.edi = frame::pop(c, g)?;
            c.esi = frame::pop(c, g)?;
            c.eax = u32::from(g.u16(c.ebp)?);
            let bypass_range = c.eax & 0x100 != 0 || {
                c.eax >>= 11;
                c.eax & 1 != 0
            };
            if bypass_range {
                if c.ebx == u32::MAX {
                    first_mapped(c, g)?;
                }
            } else {
                sse::load(c, 0, frame::slot(c, g, 0x10)?);
                sse::movaps(c, 1, 0);
                let range_sq = sse::ss_reg(c, sse::Ss::Mul, 1, 0);
                let flags = sse::comiss(c, sse::lane0(c, 4), range_sq);
                if flags & 0x41 == 0 {
                    c.ebx = u32::MAX;
                    sse::movaps(c, 0, 5);
                    return finish_selection(c, g);
                }
            }
            sse::load(c, 0, frame::slot(c, g, 12)?);
            return finish_selection(c, g);
        }
        _ => return Err(invalid_site(pc)),
    }
    while c.esi < USER_COUNT {
        let active = g.u8(c.edi)?;
        if active != 0 {
            // cmp byte ptr [edi],0
            cmp_byte_zero(c, active);
            c.ecx = c.ebp;
            c.edx = c.esi;
            return call(DISTANCE.base, 0x127d6f);
        }
        advance_listener(c);
    }
    frame::set_slot(c, g, 0x14, sse::lane0(c, 4))?;
    let mut x = load_fpu(c);
    x.fld(frame::slot(c, g, 0x14)?).map_err(fp_error)?;
    frame::push(c, g, c.ecx)?;
    x.unary(unary_op::SQRT).map_err(fp_error)?;
    c.edx = c.ebp;
    c.ecx = c.ebx;
    frame::set_slot(c, g, 0x18, x.fst_m32().map_err(fp_error)?)?;
    x.arith_m(arith_op::DIV, frame::slot(c, g, 0x1c)?)
        .map_err(fp_error)?;
    let ratio = x.fstp_m32().map_err(fp_error)?;
    g.set_u32(c.esp, ratio)?;
    finish_fpu(c, g, x, 0x127d9b, c.esp)?;
    call(0x221da0, 0x127da3)
}

fn advance_listener(c: &mut CpuAbi) {
    c.esi = c.esi.wrapping_add(1);
    c.edi = c.edi.wrapping_add(SoundListener::SIZE);
    arithmetic(c, c.esi, USER_COUNT, true);
}

fn cmp_byte_zero(c: &mut CpuAbi, byte: u8) {
    // Subtracting zero defines AF and clears it even on a negative byte.
    logic(c, byte.into(), 0x80);
    c.eflags_defined |= 0x10;
}

fn finish_selection(c: &mut CpuAbi, g: &mut Guest) -> Result<Next> {
    c.eax = frame::slot(c, g, 0x14)?;
    logic(c, c.eax, 1 << 31);
    if c.eax != 0 {
        g.set_u32(c.eax, sse::lane0(c, 0))?;
    }
    c.ebp = frame::pop(c, g)?;
    c.eax = c.ebx;
    c.ebx = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 12 })
}

pub(super) fn distance(cx: &mut Cx<'_>, pc: u32) -> Result<Next> {
    if pc != DISTANCE.base {
        return Err(invalid_site(pc));
    }
    let (c, g) = (&mut *cx.cpu, &mut *cx.g);
    frame::push(c, g, c.ecx)?;
    let kind = ((g.u8(c.ecx + 3)? << 4) as i8 >> 4) as i32 as u32;
    c.eax = arithmetic(c, kind, 0, true);
    match kind {
        0 => sse::zero(c, 0),
        1 => {
            c.eax = g.u32(points::SOUND_GLOBALS.base)?;
            c.edx = c
                .eax
                .wrapping_add(c.edx.wrapping_mul(SoundListener::SIZE))
                .wrapping_add(LISTENERS);
            sse::load(c, 0, g.u32(c.edx + 0x38)?);
            sse::ss(c, sse::Ss::Sub, 0, g.u32(c.ecx + 0x14)?);
            c.eax = g.u8(c.ecx + 2)? as i8 as i32 as u32;
            arithmetic(c, c.eax, 0, true);
            if c.eax == 0 {
                point_distance(c, g)?;
            } else {
                column_distance(c, g)?;
            }
        }
        _ => {
            // dec eax leaves CF from sub eax,0 (clear).
            alu::dec32(c, kind);
            c.eax = kind.wrapping_sub(1);
            let mut x = load_fpu(c);
            for offset in [0x14, 0x10, 0xc] {
                x.fld(g.u32(c.ecx + offset)?).map_err(fp_error)?;
            }
            x.fld_st(0).map_err(fp_error)?;
            x.arith_st(arith_op::MUL, 1).map_err(fp_error)?;
            for component in [2, 3] {
                x.fld_st(component).map_err(fp_error)?;
                x.arith_st(arith_op::MUL, component + 1).map_err(fp_error)?;
                x.arith_reg(arith_op::ADD, 1, 0, true).map_err(fp_error)?;
            }
            x.unary(unary_op::SQRT).map_err(fp_error)?;
            x.fstp_st(3).map_err(fp_error)?;
            x.fstp_st(0).map_err(fp_error)?;
            x.fstp_st(0).map_err(fp_error)?;
            let length = x.fstp_m32().map_err(fp_error)?;
            g.set_u32(c.esp, length)?;
            finish_fpu(c, g, x, 0x127e5a, c.esp)?;
            sse::load(c, 0, g.u32(c.esp)?);
        }
    }
    c.ecx = frame::pop(c, g)?;
    Ok(Next::Ret { pop: 0 })
}

fn point_distance(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    sse::load(c, 2, g.u32(c.edx + 0x34)?);
    sse::ss(c, sse::Ss::Sub, 2, g.u32(c.ecx + 0x10)?);
    sse::load(c, 1, g.u32(c.edx + 0x30)?);
    sse::ss(c, sse::Ss::Sub, 1, g.u32(c.ecx + 0xc)?);
    sse::movaps(c, 3, 2);
    sse::ss_reg(c, sse::Ss::Mul, 3, 2);
    sse::ss_reg(c, sse::Ss::Mul, 0, 0);
    sse::movaps(c, 2, 1);
    sse::ss_reg(c, sse::Ss::Add, 0, 3);
    sse::ss_reg(c, sse::Ss::Mul, 2, 1);
    sse::ss_reg(c, sse::Ss::Add, 0, 2);
    Ok(())
}

fn column_distance(c: &mut CpuAbi, g: &Guest) -> Result<()> {
    sse::zero(c, 1);
    let height = sse::lane0(c, 0);
    if sse::comiss(c, 0, height) & 0x41 != 0 {
        let top = g.u32(c.ecx + 0x30)?;
        if sse::comiss(c, height, top) & 0x41 == 0 {
            sse::load(c, 1, top);
        } else {
            sse::movaps(c, 1, 0);
        }
    }
    sse::ucomiss(c, sse::lane0(c, 1), height);
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
    if sse::parity(c) {
        sse::load(c, 0, g.u32(FAR_DISTANCE)?);
    } else {
        sse::load(c, 1, g.u32(c.ecx + 0x10)?);
        sse::ss(c, sse::Ss::Sub, 1, g.u32(c.edx + 0x34)?);
        sse::load(c, 0, g.u32(c.ecx + 0xc)?);
        sse::ss(c, sse::Ss::Sub, 0, g.u32(c.edx + 0x30)?);
        sse::movaps(c, 2, 1);
        sse::ss_reg(c, sse::Ss::Mul, 0, 0);
        sse::ss_reg(c, sse::Ss::Mul, 2, 1);
        sse::ss_reg(c, sse::Ss::Add, 0, 2);
    }
    Ok(())
}
