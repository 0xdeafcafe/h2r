//! Derive the observer's vertical FOV, rotate its forward/up vectors about
//! two axes and compute the displacement from the original view direction.
use super::*;
use super::spatial::{finish_fpu, fp_error, load_fpu, scalar};
use crate::light_constants::{Error, Sse, X87};
use x87::abi::{arith_op, unary_op};

type Vector = [u32; 3];
type Matrix = [Vector; 3];
type Math<T> = std::result::Result<T, Error>;
const HALF: u32 = 0x0045_dbbc;
const ONE: u32 = 0x0045_dbc0;
const ATAN_DENOMINATOR: u32 = 0x0045_dcc0;
const DOT_FLOOR: u32 = 0x0045_dbdc;
const VIEW_SCALE: u32 = 0x0054_e854;
const ROTATION_AXIS: u32 = 0x0046_87b0;

fn read3(g: &Guest, at: u32) -> Result<Vector> {
    Ok([g.u32(at)?, g.u32(at.wrapping_add(4))?, g.u32(at.wrapping_add(8))?])
}
fn write3(g: &mut Guest, at: u32, v: Vector) -> Result<()> {
    for (i, value) in v.into_iter().enumerate() {
        g.set_u32(at.wrapping_add(i as u32 * 4), value)?;
    }
    Ok(())
}

// Sum three products left to right. Operand order matters for NaNs.
// Each unrolled matrix application schedules its three terms differently.
fn sum(s: &mut Sse, a: (u32,u32), b: (u32,u32), c: (u32,u32)) -> Math<u32> {
    let a = s.mul(a.0,a.1)?;
    let b = s.mul(b.0,b.1)?;
    let ab = s.add(a,b)?;
    let c = s.mul(c.0,c.1)?;
    s.add(ab,c)
}

fn rotation(s: &mut Sse, axis: Vector, sine_axis: Vector, cos: u32, one: u32, second: bool) -> Math<Matrix> {
    let [x,y,z] = axis;
    let [sx,sy,sz] = sine_axis;
    let mut diag = [0;3];
    for (dest, a) in diag.iter_mut().zip(axis) {
        let aa = s.mul(a,a)?;
        let rest = s.sub(one,aa)?;
        let t = s.mul(rest,cos)?;
        *dest = s.add(t,aa)?;
    }
    let t = s.sub(one,cos)?;
    let xy = if second {
        let ty = s.mul(t,y)?;
        s.mul(ty,x)?
    } else {
        let tx = s.mul(t,x)?;
        s.mul(tx,y)?
    };
    let tz = s.mul(t,z)?;
    let xz = s.mul(tz,x)?;
    let yz = s.mul(tz,y)?;
    Ok([
        [diag[0], s.sub(xy,sz)?, s.add(xz,sy)?],
        [s.add(xy,sz)?, diag[1], s.sub(yz,sx)?],
        [s.sub(xz,sy)?, s.add(yz,sx)?, diag[2]],
    ])
}

fn rotate_pair(s: &mut Sse, m: Matrix, f: Vector, u: Vector, second: bool) -> Math<(Vector,Vector)> {
    let [[a,b,c],[d,e,h],[i,j,k]] = m;
    let [x,y,z] = f;
    let forward = if second {
        [sum(s,(c,z),(b,y),(a,x))?, sum(s,(h,z),(e,y),(d,x))?, sum(s,(j,y),(k,z),(i,x))?]
    } else {
        [sum(s,(x,a),(c,z),(b,y))?, sum(s,(h,z),(e,y),(x,d))?, sum(s,(j,y),(k,z),(x,i))?]
    };
    let [x,y,z] = u;
    let up = if second {
        [sum(s,(a,x),(z,c),(y,b))?, sum(s,(d,x),(z,h),(y,e))?, sum(s,(i,x),(y,j),(z,k))?]
    } else {
        [sum(s,(z,c),(y,b),(x,a))?, sum(s,(x,d),(z,h),(y,e))?, sum(s,(x,i),(y,j),(z,k))?]
    };
    Ok((forward,up))
}

fn cross(s: &mut Sse, f: Vector, u: Vector) -> Math<Vector> {
    let mut diff = |a,b,c,d| {
        let pos = s.mul(a,b)?;
        let neg = s.mul(c,d)?;
        s.sub(pos,neg)
    };
    Ok([diff(f[1],u[2],u[1],f[2])?, diff(f[2],u[0],f[0],u[2])?, diff(u[1],f[0],f[1],u[0])?])
}

// atan2 on the x87 stack (Y below the loaded X).
fn atan_one(x: &mut X87) -> Math<()> {
    X87::ok(x87::abi::ld_const(&mut x.0, x87::Const::One as u32))?;
    x.unary(unary_op::PATAN)
}

pub(super) fn camera(c: &mut CpuAbi, g: &mut Guest, _: u32) -> Result<TransferAbi> {
    let v = c.esi;
    let sp = c.esp.wrapping_sub(80);
    let mut x = load_fpu(c);
    let mut s = Sse { mxcsr: c.mxcsr };
    let fov = g.u32(v.wrapping_add(56))?;
    let half = g.u32(HALF)?;
    let one = g.u32(ONE)?;
    let original = read3(g, v.wrapping_add(32))?;
    let up = read3(g, v.wrapping_add(44))?;
    let axis = read3(g, g.u32(ROTATION_AXIS)?)?;
    let scaled = s.div(fov, g.u32(VIEW_SCALE)?).map_err(fp_error)?;
    g.set_u32(v.wrapping_add(80), scaled)?;
    x.fld(fov).map_err(fp_error)?;
    x.fmul_m(half).map_err(fp_error)?;
    x.unary(unary_op::TAN).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    x.arith_m(arith_op::DIV, g.u32(v.wrapping_add(60))?).map_err(fp_error)?;
    X87::ok(x87::abi::ld_f64(&mut x.0, g.u64(ATAN_DENOMINATOR)?)).map_err(fp_error)?;
    x.unary(unary_op::PATAN).map_err(fp_error)?;
    x.arith_st(arith_op::ADD,0).map_err(fp_error)?;
    g.set_u32(v.wrapping_add(76), x.fst_m32().map_err(fp_error)?)?;
    x.fmul_m(half).map_err(fp_error)?;
    x.unary(unary_op::TAN).map_err(fp_error)?;
    x.fstp_st(0).map_err(fp_error)?;
    x.fld_st(0).map_err(fp_error)?;
    x.fmul_m(g.u32(v.wrapping_add(64))?).map_err(fp_error)?;
    atan_one(&mut x).map_err(fp_error)?;
    x.fxch(1).map_err(fp_error)?;
    x.fmul_m(g.u32(v.wrapping_add(68))?).map_err(fp_error)?;
    atan_one(&mut x).map_err(fp_error)?;
    let angle = x.fstp_m32().map_err(fp_error)?;
    x.fld_st(0).map_err(fp_error)?;
    x.fsin().map_err(fp_error)?;
    let sin = x.fstp_m32().map_err(fp_error)?;
    x.fcos().map_err(fp_error)?;
    let cos = x.fstp_m32().map_err(fp_error)?;
    x.fld(sin).map_err(fp_error)?;
    x.fmul_m(axis[0]).map_err(fp_error)?;
    let sx = x.fstp_m32().map_err(fp_error)?;
    let sy = s.mul(sin,axis[1]).map_err(fp_error)?;
    let sz = s.mul(sin,axis[2]).map_err(fp_error)?;
    let first = rotation(&mut s,axis,[sx,sy,sz],cos,one,false).map_err(fp_error)?;
    let (f,u) = rotate_pair(&mut s,first,original,up,false).map_err(fp_error)?;
    let axis = cross(&mut s,f,u).map_err(fp_error)?;

    x.fld(angle).map_err(fp_error)?;
    x.fsin().map_err(fp_error)?;
    let sin = x.fstp_m32().map_err(fp_error)?;
    x.fld(angle).map_err(fp_error)?;
    x.fcos().map_err(fp_error)?;
    let cos = x.fstp_m32().map_err(fp_error)?;
    let mut scaled_axis = [0;3];
    for (dst, a) in scaled_axis.iter_mut().zip(axis) { *dst = s.mul(a,sin).map_err(fp_error)?; }
    let second = rotation(&mut s,axis,scaled_axis,cos,one,true).map_err(fp_error)?;
    let (forward,new_up) = rotate_pair(&mut s,second,f,u,true).map_err(fp_error)?;
    let dot = sum(&mut s,(forward[1],original[1]),(forward[2],original[2]),(forward[0],original[0])).map_err(fp_error)?;
    let floor = g.u32(DOT_FLOOR)?;
    let divisor = if s.above(dot,floor).map_err(fp_error)? { dot } else { floor };
    let factor = s.div(one,divisor).map_err(fp_error)?;
    let mut delta = [0;3];
    for i in [2,1,0] {
        let a = if i == 0 { s.mul(factor,original[i]) } else { s.mul(original[i],factor) }.map_err(fp_error)?;
        delta[i] = s.sub(a,forward[i]).map_err(fp_error)?;
        x.fld(delta[i]).map_err(fp_error)?;
        x.fmul_m(delta[i]).map_err(fp_error)?;
        if i != 2 { x.arith_reg(arith_op::ADD,1,0,true).map_err(fp_error)?; }
    }
    x.unary(unary_op::SQRT).map_err(fp_error)?;
    let displacement = x.fstp_m32().map_err(fp_error)?;
    // Surviving frame spills, including the saved EDI below the frame.
    g.set_u32(sp.wrapping_sub(4), c.edi)?;
    g.set_u32(sp,cos)?;
    g.set_u32(sp.wrapping_add(4),s.sub(one,cos).map_err(fp_error)?)?;
    write3(g,sp.wrapping_add(8),delta)?;
    write3(g,sp.wrapping_add(20),original)?;
    write3(g,sp.wrapping_add(32),u)?;
    for (off, value) in [(48,second[1][0]),(52,second[2][0]),(56,second[0][1]),
                         (64,second[2][1]),(72,first[1][2])] {
        g.set_u32(sp.wrapping_add(off),value)?;
    }
    c.eax = v.wrapping_add(32);
    c.ecx = v.wrapping_add(44);
    c.edx = sp.wrapping_add(32);
    write3(g,c.eax,forward)?;
    write3(g,c.ecx,new_up)?;
    g.set_u32(v.wrapping_add(72),displacement)?;
    finish_fpu(c,g,x,0x172341,v.wrapping_add(72))?;
    let lanes = [delta[0],delta[1],delta[2],original[0],floor,factor,second[1][1],second[0][2]];
    for (i,value) in lanes.into_iter().enumerate() { scalar(c,i,value); }
    c.mxcsr = s.mxcsr;
    c.esp = arithmetic(c,sp,80,false);
    g.ret(c,0)
}

// The camera path keeps its private frame in Rust until the final stores.
// With aliased inputs or an unwriteable frame, run the original in order.
pub(super) fn disjoint(c: &CpuAbi, g: &Guest) -> bool {
    use runtime::guest::Access;
    let Some(sp) = c.esp.checked_sub(84) else { return false };
    let Some(end) = c.esp.checked_add(4) else { return false };
    let Some(view_end) = c.esi.checked_add(84) else { return false };
    let Ok(axis) = g.u32(ROTATION_AXIS) else { return false };
    let Some(axis_end) = axis.checked_add(12) else { return false };
    let (Some(stack), Some(view)) =
        (physical_span(g,sp,end-sp), physical_span(g,c.esi,view_end-c.esi))
    else {return false};
    if overlap(&stack,&view) {return false;}
    for input in [axis..axis_end, HALF..ONE+4, ATAN_DENOMINATOR..ATAN_DENOMINATOR+8,
                  DOT_FLOOR..DOT_FLOOR+4, VIEW_SCALE..VIEW_SCALE+4, ROTATION_AXIS..ROTATION_AXIS+4] {
        let Some(span) = physical_span(g,input.start,input.end-input.start) else {return false};
        if overlap(&span,&view) || overlap(&span,&stack)
            || g.check(input.start,input.len(),Access::Read,1).is_err() { return false; }
    }
    g.check(sp,84,Access::Write,1).is_ok() && g.check(c.esp,4,Access::Read,1).is_ok()
        && g.check(c.esi,84,Access::Read,1).is_ok() && g.check(c.esi+32,52,Access::Write,1).is_ok()
}
