//! Barrel assist. Remote shots can already carry a point; local shots
//! interpolate the obstruction ray and the selected target before clamping
//! the direction to the weapon's bend cone.
use super::*;
use crate::light_constants::X87;
use crate::sim_physics::{arithmetic, spatial::{finish_fpu, fp_error, load_fpu}};
use runtime::TransferAbi;
use x87::abi::{arith_op, unary_op};

pub const SHOT_ENTRY: u32 = 0x001a_4c00;
const UNIT: u32 = 0x1a4c31;
const REMOTE_DIRECTION: u32 = 0x1a4c8a;
const CONE: u32 = 0x1a4cc4;
const BASIS: u32 = 0x1a4cde;
const TARGET: u32 = 0x1a4d2b;
const TARGET_DIRECTION: u32 = 0x1a4d7d;
const EYE: u32 = 0x1a4ddd;
const EYE_DIRECTION: u32 = 0x1a4e32;
const IGNORE_OBJECT: u32 = 0x1a4ec2;
const OBSTRUCTION: u32 = 0x1a4ed7;
const OBSTRUCTION_DIRECTION: u32 = 0x1a4f12;
const CLAMP: u32 = 0x1a5003;
const CLAMPED_DIRECTION: u32 = 0x1a502e;
pub const SHOT_SITES: &[u32] = &[
    UNIT, REMOTE_DIRECTION, CONE, BASIS, TARGET, TARGET_DIRECTION, EYE,
    EYE_DIRECTION, IGNORE_OBJECT, OBSTRUCTION, OBSTRUCTION_DIRECTION, CLAMP, CLAMPED_DIRECTION,
];

const NORMALIZE: u32 = 0x30bf0;
const SIM_GLOBALS: u32 = 0x4e6948;
const PLAYER_TABLE: u32 = 0x4e8c24;
const TICK_GLOBALS: u32 = 0x510c54;
const RAY_SCALE: u32 = 0x45dd60;
// Immediate operand of the collision query. The loaded image owns the mask.
const RAY_MASK: u32 = 0x1a4ece;

fn finish(c: &mut CpuAbi, g: &impl Memory, hit: bool) -> Result {
    c.edi = pop(c, g)?;
    c.esi = pop(c, g)?;
    c.eax = byte(c.eax, u8::from(hit));
    c.ebp = pop(c, g)?;
    c.esp = arithmetic(c, c.esp, 0xd0, false);
    g.ret(c, 12)
}

fn delta(c: &mut CpuAbi, g: &mut impl Memory, point: u32, dest: u32) -> std::result::Result<(), GuestFault> {
    for offset in [0,4,8] {
        sse::load(c, 0, g.u32(at(point, offset))?);
        let word = sse::ss(c, Ss::Sub, 0, g.u32(at(c.ebx, offset))?);
        g.set_u32(at(dest, offset), word)?;
    }
    c.eax = dest;
    Ok(())
}

fn normalize_delta(c: &mut CpuAbi, g: &mut impl Memory, point: u32, dest: u32, resume: u32) -> Result {
    delta(c, g, point, dest)?;
    g.call(c, NORMALIZE, resume)
}

fn nonzero(c: &mut CpuAbi, left: u32, right: u32) -> bool {
    sse::ucomiss(c, left, right);
    sse::lahf(c);
    let flags = (c.eax >> 8) & 0x44;
    logic(c, flags, 0x80);
    c.eflags & 4 != 0
}

fn eye(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    c.eax = g.u32(at(c.esp, 0x48))?;
    g.set_u16(at(c.esp, 0x7c), c.esi as u16)?;
    c.esi = at(c.esp, 0x0c);
    g.call(c, 0xb9dd0, EYE)
}

fn eye_length(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let sp = c.esp;
    let mut x = load_fpu(c);
    for offset in [0,4,8] {
        x.fld(g.u32(at(sp, 12+offset))?).map_err(fp_error)?;
        x.arith_m(arith_op::SUB, g.u32(at(sp, 0x34+offset))?).map_err(fp_error)?;
    }
    c.eax = g.u32(at(sp, 0x20))?;
    c.ecx = g.u32(at(sp, 0x18))?;
    c.edx = g.u32(at(sp, 0x1c))?;
    g.set_u32(at(sp, 0x14), c.eax)?;
    c.eax = at(sp, 0x0c);
    g.set_u32(at(sp, 0xc), c.ecx)?;
    g.set_u32(at(sp, 0x10), c.edx)?;
    x.fld_st(0).map_err(fp_error)?;
    x.arith_st(arith_op::MUL, 1).map_err(fp_error)?;
    x.fld_st(2).map_err(fp_error)?;
    x.arith_st(arith_op::MUL, 3).map_err(fp_error)?;
    x.arith_reg(arith_op::ADD, 1, 0, true).map_err(fp_error)?;
    x.fld_st(3).map_err(fp_error)?;
    x.arith_st(arith_op::MUL, 4).map_err(fp_error)?;
    x.arith_reg(arith_op::ADD, 1, 0, true).map_err(fp_error)?;
    x.unary(unary_op::SQRT).map_err(fp_error)?;
    for slot in [3,0,0] { x.fstp_st(slot).map_err(fp_error)?; }
    let out = at(sp, 0x40);
    g.set_u32(out, x.fstp_m32().map_err(fp_error)?)?;
    finish_fpu(c, g, x, 0x1a4e29, out)?;
    g.call(c, NORMALIZE, EYE_DIRECTION)
}

fn obstruction(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let sp = c.esp;
    let distance = g.u32(at(sp, 0x40))?;
    sse::load(c, 3, distance);
    // The ray's starting point lies along the normalised eye direction.
    let xyz = [g.u32(at(sp, 12))?, g.u32(at(sp, 16))?, g.u32(at(sp, 20))?];
    for (axis, r) in [(1,1), (0,0), (2,2)] {
        sse::load(c, r, xyz[axis]);
        sse::ss(c, Ss::Mul, r, distance);
        let p = at(sp, 0x34 + axis as u32 * 4);
        let pos = sse::ss(c, Ss::Add, r, g.u32(p)?);
        g.set_u32(p, pos)?;
    }
    let scale = g.u32(RAY_SCALE)?;
    sse::load(c, 0, scale);
    for axis in 0..3 {
        sse::load(c, 1, g.u32(at(sp, 0x18 + axis*4))?);
        let d = sse::ss(c, Ss::Mul, 1, scale);
        g.set_u32(at(sp, 0x4c + axis*4), d)?;
    }
    c.ecx = at(sp, 0x58);
    push(c, g, c.ecx)?;
    c.ecx = g.u32(at(sp, 0x48))?;
    push(c, g, u32::MAX)?;
    g.call(c, 0xbaf80, IGNORE_OBJECT)
}

// x87 normalises each component by the same extended-precision reciprocal.
fn normalize_blend(c: &mut CpuAbi, g: &mut impl Memory, length2: u32) -> std::result::Result<(), GuestFault> {
    let mut x: X87 = load_fpu(c);
    x.fld(length2).map_err(fp_error)?;
    x.unary(unary_op::SQRT).map_err(fp_error)?;
    x.arith_m(arith_op::DIVR, g.u32(ONE)?).map_err(fp_error)?;
    for offset in [12,16,20] {
        let dest = at(c.esp, offset);
        x.fld(g.u32(dest)?).map_err(fp_error)?;
        x.arith_st(arith_op::MUL, 1).map_err(fp_error)?;
        g.set_u32(dest, x.fstp_m32().map_err(fp_error)?)?;
    }
    x.fstp_st(0).map_err(fp_error)?;
    finish_fpu(c, g, x, 0x1a4ff5, at(c.esp, 20))
}

fn blend(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let sp = c.esp;
    sse::zero(c, 5);
    let length = sse::lane0(c, 0);
    if !nonzero(c, length, 0) {
        c.edx = g.u32(c.edi)?;
        c.eax = g.u32(at(c.edi, 4))?;
        c.ecx = g.u32(at(c.edi, 8))?;
        for (offset, word) in [(0x18,c.edx),(0x1c,c.eax),(0x20,c.ecx)] { g.set_u32(at(sp, offset), word)?; }
    }
    let target_weight = g.u32(at(sp, 0x24))?;
    sse::load(c, 4, target_weight);
    sse::load(c, 1, g.u32(ONE)?);
    let ray_weight = sse::ss(c, Ss::Sub, 1, target_weight);
    let mut direction = [0; 3];
    for axis in 0..3 {
        let ray = g.u32(at(sp, 0x18 + 4*axis))?;
        let target = g.u32(at(sp, 0x28 + 4*axis))?;
        // The guest multiplies ray_weight*ray.x and ray.yz*ray_weight.
        let (a,b) = if axis == 0 { (ray_weight,ray) } else { (ray,ray_weight) };
        sse::load(c, 0, a);
        sse::ss(c, Ss::Mul, 0, b);
        sse::load(c, 1, target);
        let pull = sse::ss(c, Ss::Mul, 1, target_weight);
        direction[axis as usize] = sse::ss(c, Ss::Add, 0, pull);
    }
    let [x,y,z] = direction;
    sse::load(c, 0, x);
    sse::load(c, 1, z);
    sse::ss(c, Ss::Mul, 1, z);
    g.set_u32(at(sp, 20), z)?;
    sse::load(c, 3, y);
    let yy = sse::ss(c, Ss::Mul, 3, y);
    g.set_u32(at(sp, 16), y)?;
    sse::load(c, 2, x);
    sse::ss(c, Ss::Add, 1, yy);
    let xx = sse::ss(c, Ss::Mul, 2, x);
    let length2 = sse::ss(c, Ss::Add, 1, xx);
    let needs_normalize = nonzero(c, length2, 0);
    g.set_u32(at(sp, 12), x)?;
    g.set_u32(at(sp, 0x40), length2)?;
    if needs_normalize { normalize_blend(c, g, length2)?; }
    c.ecx = g.u32(at(sp, 0xe0))?;
    g.call(c, 0x1a6d30, CLAMP)
}

fn record(c: &mut CpuAbi, g: &mut impl Memory) -> Result {
    let weight = sse::lane0(c, 4);
    let full = sse::comiss(c, weight, g.u32(ONE)?) & 1 == 0;
    if full && c.eax as u8 != 0 {
        c.edx = g.u32(at(c.esp, 0x44))?;
        c.ecx = g.u32(TICK_GLOBALS)?;
        g.set_u32(at(c.ebp, 0x174), c.edx)?;
        c.edx = g.u32(at(c.ecx, 8))?;
        // EDI and ESI are popped before the final tick store.
        c.edi = pop(c, g)?;
        c.esi = pop(c, g)?;
        g.set_u32(at(c.ebp, 0x178), c.edx)?;
        c.eax = byte(c.eax, 1);
        c.ebp = pop(c, g)?;
        c.esp = arithmetic(c, c.esp, 0xd0, false);
        return g.ret(c, 12);
    }
    finish(c, g, false)
}

pub fn shot(c: &mut CpuAbi, g: &mut impl Memory, pc: u32) -> Result {
    let sp = c.esp;
    match pc {
        SHOT_ENTRY => {
            c.esp = sp.wrapping_sub(0xd0);
            push(c, g, c.ebp)?;
            c.ebp = (g.u32(at(c.esp, 0xd8))? & 0xffff)*0x21c;
            push(c, g, c.esi)?;
            push(c, g, c.edi)?;
            c.edi = c.eax;
            let players = g.u32(PLAYER_TABLE)?;
            c.esi = g.u32(at(players, 0x44))?;
            c.eax = g.u32(at(at(c.ebp, c.esi), 0x2c))?;
            c.ebp = arithmetic(c, c.ebp, c.esi, false);
            g.call(c, 0xcb6d0, UNIT)
        }
        UNIT => {
            c.edx = g.u32(SIM_GLOBALS)?;
            c.ecx = c.eax;
            c.eax = g.u8(at(c.edx, 12))? as i8 as i32 as u32;
            g.set_u32(at(sp, 0x48), c.ecx)?;
            if matches!(c.eax, 4 | 5) {
                c.eax = g.u32(at(sp, 0xe4))?;
                let flag = g.u8(at(c.eax, 24))? & 2;
                if flag != 0 {
                    logic(c, u32::from(flag), 0x80);
                    return normalize_delta(c, g, at(c.eax, 12), c.edi, REMOTE_DIRECTION);
                }
            }
            c.edx = g.u32(at(g.u32(points::OBJECTS.base)?, 0x44))?;
            let index = c.ecx & 0xffff;
            logic(c, index, 0x8000_0000);
            c.eax = g.u32(at(c.edx, index*12+8))?;
            c.eax = low_word(c.eax, signed_slot(g, at(c.eax, 0x240))?);
            c.esi = at(sp, 0xb8);
            push(c, g, c.eax)?;
            g.call(c, points::AIM_ASSIST_PARAMS.base, CONE)
        }
        REMOTE_DIRECTION => finish(c, g, true),
        CONE => {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 == 0 { return finish(c, g, false); }
            c.eax = g.u32(at(c.ebp, 0x2c))?;
            c.ecx = at(sp, 0x18);
            push(c, g, c.ecx)?;
            c.edx = at(sp, 0x34);
            push(c, g, c.edx)?;
            g.call(c, 0x1557c0, BASIS)
        }
        BASIS => {
            sse::zero(c, 0);
            c.ecx = g.u32(c.edi)?;
            c.edx = g.u32(at(c.edi, 4))?;
            c.eax = g.u32(at(c.edi, 8))?;
            g.set_u32(at(sp, 0x28), c.ecx)?;
            c.ecx = low_word(c.ecx, signed_slot(g, at(c.ebp, 0xc0))?);
            g.set_u32(at(sp, 0x2c), c.edx)?;
            c.edx = g.u32(at(c.ebp, 0x2c))?;
            g.set_u32(at(sp, 0x30), c.eax)?;
            c.eax = at(sp, 0x18);
            c.esi = u32::MAX;
            logic(c, c.esi, 0x8000_0000);
            g.set_u32(at(sp, 0x24), 0)?;
            g.set_u32(at(sp, 0x44), c.esi)?;
            for word in [c.ecx, c.edx, c.eax, at(sp, 0x34)] { push(c,g,word)?; }
            c.ecx = g.u32(at(sp, 0xe4))?;
            c.eax = at(sp, 0x58);
            g.call(c, 0x1a6a80, TARGET)
        }
        TARGET => {
            logic(c, c.eax & 0xff, 0x80);
            if c.eax as u8 != 0 {
                c.eax = byte(c.eax, g.u8(at(sp, 0x80))?);
                logic(c, c.eax & 0xff, 0x80);
                if c.eax as u8 == 0 {
                    return normalize_delta(c,g,at(sp,0x60),at(sp,0x28),TARGET_DIRECTION);
                }
            }
            eye(c,g)
        }
        TARGET_DIRECTION => {
            let length = sse::lane0(c,0);
            if !nonzero(c,length,g.u32(0x45dbd8)?) {
                c.eax = g.u32(c.edi)?;
                c.ecx = g.u32(at(c.edi,4))?;
                c.edx = g.u32(at(c.edi,8))?;
                for (offset,word) in [(0x28,c.eax),(0x2c,c.ecx),(0x30,c.edx)] {g.set_u32(at(sp,offset),word)?;}
            }
            c.eax = byte(c.eax,g.u8(at(sp,0xe8))?);
            logic(c,c.eax&0xff,0x80);
            let weight = g.u32(at(sp, if c.eax as u8 != 0 {0xa4} else {0xa8}))?;
            sse::load(c,0,weight);
            c.eax = g.u32(at(sp,0x58))?;
            g.set_u32(at(sp,0x24),weight)?;
            g.set_u32(at(sp,0x44),c.eax)?;
            eye(c,g)
        }
        EYE => eye_length(c,g),
        EYE_DIRECTION => obstruction(c,g),
        IGNORE_OBJECT => {
            push(c,g,c.eax)?;
            c.edx = at(sp,0x54);
            push(c,g,c.edx)?;
            c.eax = at(sp,0x3c);
            push(c,g,c.eax)?;
            let mask = g.u32(RAY_MASK)?;
            push(c,g,mask)?;
            g.call(c,0x1697c0,OBSTRUCTION)
        }
        OBSTRUCTION => normalize_delta(c,g,at(sp,0x60),at(sp,0x18),OBSTRUCTION_DIRECTION),
        OBSTRUCTION_DIRECTION => blend(c,g),
        CLAMP => {
            logic(c,c.eax&0xff,0x80);
            if c.eax as u8 == 0 {
                c.edx = g.u32(at(sp,12))?;
                c.eax = g.u32(at(sp,16))?;
                c.ecx = g.u32(at(sp,20))?;
                for (offset,word) in [(0,c.edx),(4,c.eax),(8,c.ecx)] {g.set_u32(at(c.edi,offset),word)?;}
                c.eax = byte(c.eax,1);
                return record(c,g);
            }
            let mut x = load_fpu(c);
            x.fld(g.u32(at(sp,0xc8))?).map_err(fp_error)?;
            c.esp = arithmetic(c,sp,8,true);
            x.fcos().map_err(fp_error)?;
            c.esi = c.edi;
            c.eax = at(sp,12);
            g.set_u32(at(c.esp,4),x.fstp_m32().map_err(fp_error)?)?;
            x.fld(g.u32(at(c.esp,0xd0))?).map_err(fp_error)?;
            x.fsin().map_err(fp_error)?;
            g.set_u32(c.esp,x.fstp_m32().map_err(fp_error)?)?;
            finish_fpu(c,g,x,0x1a5026,c.esp)?;
            g.call(c,0x11d3b0,CLAMPED_DIRECTION)
        }
        CLAMPED_DIRECTION => {
            sse::load(c,4,g.u32(at(sp,0x24))?);
            c.eax = byte(c.eax,u8::from(c.eax as u8 == 0));
            record(c,g)
        }
        _ => unreachable!("unregistered shot return {pc:#x}"),
    }
}
