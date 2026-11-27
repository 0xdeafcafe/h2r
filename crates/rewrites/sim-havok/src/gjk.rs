//! The leaves of the convex agent's closest-feature query
//! (docs/engine/havok-convex.md, "The query path"), each one guest
//! function on a [`Journal`], every float operation one guest
//! instruction: packed and scalar SSE under the MXCSR, x87 under the
//! control word.
//!
//! - [`relative`] `0x002da4d0`: B's transform in A's frame.
//! - [`rotate`] `0x002da6e0`: a vector by a transform's rotation.
//! - [`triangle`] `0x00303360`: a point against a triangle's three edge
//!   planes, with the triangle's normal.
//! - [`segments`] `0x00308590`: the closest points of two segments.
//! - [`contact`] `0x003032c0`: the contact from the query's final state.
//! - [`tetrahedron`] `0x00303540` with `0x00303110`: which face of a
//!   tetrahedron the point lies beyond.
//! - [`edge`] `0x003037a0`: a segment against a triangle, which of the
//!   two simplices loses a vertex.
//!
//! The query itself (`0x003048a0` and its case `0x00303dd0`) and the
//! shapes' support virtuals are not ported. On a [`Fallback`] the journal
//! is discarded and nothing has been written.
use crate::convex::Fallback;
use crate::fp::{Arith, Exact};
use crate::jacobian::{lane, put_vector, splat, vector, Fpu, Fx, Sse};
use crate::mem::Journal;
use crate::solver::load;
use x87::{Cmp, F80, Mode};

/// `0.0`, `1.0`, the segments' parallel threshold (`2^-20`) and the
/// sign mask `xorps` flips a normal with.
pub const ZERO: u32 = 0x0045_dbd8;
pub const ONE: u32 = 0x0045_dbc0;
pub const PARALLEL: u32 = 0x0045_dd4c;
pub const SIGNS: u32 = 0x0041_24a0;
/// The `andps` mask that clears the sign bits, and [`tetrahedron`]'s two
/// thresholds (`1e-6`, `1.1`).
pub const ABS: u32 = 0x0045_5940;
pub const TINY: u32 = 0x0045_df1c;
pub const MARGIN: u32 = 0x0045_dc80;
/// `1.0` in every lane: [`edge`]'s `1 - t`.
pub const ONES: u32 = 0x0041_2420;

/// The query state's words [`triangle`] writes: the normal and the three
/// edge tests; and the two support points it compares the normal with.
pub mod state {
    pub const SUPPORT_A: u32 = 0x20;
    pub const SUPPORT_B: u32 = 0xa0;
    pub const NORMAL: u32 = 0x120;
    pub const SIDES: u32 = 0x160;
    /// The two simplex sizes, and the point [`super::contact`] takes when
    /// neither is one.
    pub const COUNT_A: u32 = 0x00;
    pub const COUNT_B: u32 = 0x04;
    pub const MIDPOINT: u32 = 0x130;
    /// The word [`super::edge`] sets when the segment drops to its
    /// second vertex.
    pub const FEATURE: u32 = 0x10;
}

/// [`contact`]'s fields in its output.
pub mod contact_field {
    pub const NORMAL: u32 = 0x00;
    pub const POINT: u32 = 0x10;
    pub const DISTANCE: u32 = 0x20;
    pub const WEIGHT: u32 = 0x24;
}

/// `shufps d, s, imm`: two lanes of `d`, then two of `s`.
fn shuf2(d: u128, s: u128, imm: u8) -> u128 {
    let pick = |v: u128, sel: u8| u128::from(lane(v, usize::from(sel & 3)));
    pick(d, imm) | pick(d, imm >> 2) << 32 | pick(s, imm >> 4) << 64 | pick(s, imm >> 6) << 96
}

/// `shufps v, v, imm`.
fn shuf1(v: u128, imm: u8) -> u128 {
    shuf2(v, v, imm)
}

/// `movmskps`: the four sign bits.
fn signs(v: u128) -> u32 {
    (0..4).fold(0, |m, i| m | (lane(v, i) >> 31) << i)
}

fn vec(j: &Journal, at: u32) -> Result<u128, Fallback> {
    vector(j, at).ok_or(Fallback::Memory)
}

/// `movaps` store: 16-byte aligned, or the guest would fault.
fn put(j: &mut Journal, at: u32, v: u128) -> Result<(), Fallback> {
    if at & 15 != 0 {
        return Err(Fallback::Memory);
    }
    put_vector(j, at, v).ok_or(Fallback::Memory)
}

fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

fn units(fpu: Fpu) -> Result<(Sse, Fx), Fallback> {
    if !fpu.masked() {
        return Err(Fallback::Float);
    }
    Ok((Sse { e: Exact::new(fpu.mxcsr) }, Fx { mode: Mode::from_cw(fpu.control) }))
}

fn done(sse: &Sse) -> Result<(), Fallback> {
    if sse.e.trapped { Err(Fallback::Float) } else { Ok(()) }
}

/// The transposed rotation of the transform at `m`: three rows, each
/// with its `w` lane the row's `z` (`0x002da4d0`'s shuffles).
fn rows(j: &Journal, m: u32) -> Result<[u128; 3], Fallback> {
    let c2 = vec(j, m.wrapping_add(0x20))?;
    let c0 = vec(j, m)?;
    let c1 = vec(j, m.wrapping_add(0x10))?;
    let lo = shuf2(c0, c1, 0x44);
    let hi = shuf2(c0, c1, 0xee);
    let (c2lo, c2hi) = (shuf1(c2, 0x44), shuf1(c2, 0xee));
    Ok([shuf2(lo, c2lo, 0x88), shuf2(lo, c2lo, 0xdd), shuf2(hi, c2hi, 0x88)])
}

/// `(r0 x + r1 y) + r2 z`, each product row times the splat.
fn combine(sse: &mut Sse, r: &[u128; 3], v: u128) -> u128 {
    let z = sse.mul(r[2], splat(lane(v, 2)));
    let y = sse.mul(r[1], splat(lane(v, 1)));
    let x = sse.mul(r[0], splat(lane(v, 0)));
    let xy = sse.add(x, y);
    sse.add(xy, z)
}

/// `0x002da4d0`, thiscall on `out`, `ret 8`: each of B's four columns
/// (the translation included) by A's transposed rotation, then A's
/// translation, rotated the same way, taken from the fourth. Memory is
/// read and written in the guest's order, so aliasing behaves as it does.
pub fn relative(j: &mut Journal, out: u32, a: u32, b: u32, fpu: Fpu) -> Result<(), Fallback> {
    let (mut sse, _) = units(fpu)?;
    let r = rows(j, a)?;
    for i in 0..4u32 {
        let v = vec(j, b.wrapping_add(16 * i))?;
        let s = combine(&mut sse, &r, v);
        put(j, out.wrapping_add(16 * i), s)?;
    }
    let r = rows(j, a)?;
    let t = vec(j, a.wrapping_add(0x30))?;
    let y = sse.mul(r[1], splat(lane(t, 1)));
    let x = sse.mul(r[0], splat(lane(t, 0)));
    let z = sse.mul(r[2], splat(lane(t, 2)));
    let xy = sse.add(x, y);
    let moved = sse.add(xy, z);
    let fourth = vec(j, out.wrapping_add(0x30))?;
    let t = sse.sub(fourth, moved);
    put(j, out.wrapping_add(0x30), t)?;
    done(&sse)
}

/// `0x002da6e0`, thiscall on `out`, `ret 8`: `c0 x + c1 y + c2 z` over
/// the transform's three rotation columns, no translation.
pub fn rotate(j: &mut Journal, out: u32, m: u32, v: u32, fpu: Fpu) -> Result<(), Fallback> {
    let (mut sse, _) = units(fpu)?;
    let v = vec(j, v)?;
    let z = sse.mul(vec(j, m.wrapping_add(0x20))?, splat(lane(v, 2)));
    let y = sse.mul(vec(j, m.wrapping_add(0x10))?, splat(lane(v, 1)));
    let x = sse.mul(vec(j, m)?, splat(lane(v, 0)));
    let xy = sse.add(x, y);
    let s = sse.add(xy, z);
    put(j, out, s)?;
    done(&sse)
}

/// `u.yzx * v.zxy - u.zxy * v.yzx`, `w` included: the query's cross product.
fn cross(sse: &mut Sse, u: u128, v: u128) -> u128 {
    let left = sse.mul(shuf1(u, 0xc9), shuf1(v, 0xd2));
    let right = sse.mul(shuf1(u, 0xd2), shuf1(v, 0xc9));
    sse.sub(left, right)
}

/// [`triangle`]'s answer: the edge mask (eax) and edx at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Sides {
    pub mask: u32,
    pub edx: u32,
}

/// `0x00303360`, thiscall on the query state, `(point, triangle, orient)`,
/// `ret 0xc`. The triangle is three vertices at `+0x00`, `+0x10`, `+0x20`
/// with a parallel array at `+0x40`. The normal is `(c - b) x (a - c)`;
/// each edge's test is the normal dotted with `(p - v) x edge`, and the
/// mask holds their sign bits. With `orient` set the normal is stored,
/// and when all three signs are set and the normal points away from the
/// state's support difference, it is negated and the first two vertices
/// (both arrays) and the first two tests swap.
pub fn triangle(j: &mut Journal, st: u32, p: u32, tri: u32, orient: u32, fpu: Fpu) -> Result<Sides, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let at = |off: u32| st.wrapping_add(off);
    let a = vec(j, tri)?;
    let b = vec(j, tri.wrapping_add(0x10))?;
    let c = vec(j, tri.wrapping_add(0x20))?;
    let ac = sse.sub(a, c);
    let cb = sse.sub(c, b);
    let ba = sse.sub(b, a);
    let n = cross(&mut sse, cb, ac);
    let p = vec(j, p)?;
    let pb = sse.sub(p, b);
    let e1 = cross(&mut sse, pb, cb);
    let pc = sse.sub(p, c);
    let e2 = cross(&mut sse, pc, ac);
    let pa = sse.sub(p, a);
    let e3 = cross(&mut sse, pa, ba);
    let (e12lo, e3lo) = (shuf2(e1, e2, 0x44), shuf1(e3, 0x44));
    let zrow = shuf2(shuf2(e1, e2, 0xee), shuf1(e3, 0xee), 0x88);
    let zs = sse.mul(zrow, splat(lane(n, 2)));
    let ys = sse.mul(shuf2(e12lo, e3lo, 0xdd), splat(lane(n, 1)));
    let xs = sse.mul(shuf2(e12lo, e3lo, 0x88), splat(lane(n, 0)));
    let xy = sse.add(xs, ys);
    let d = sse.add(xy, zs);
    let mask = signs(d) & 7;
    put(j, at(state::SIDES), d)?;
    if orient == 0 {
        done(&sse)?;
        return Ok(Sides { mask, edx: tri });
    }
    put(j, at(state::NORMAL), n)?;
    if mask != 7 {
        done(&sse)?;
        return Ok(Sides { mask, edx: tri });
    }
    let q = sse.sub(vec(j, at(state::SUPPORT_B))?, vec(j, at(state::SUPPORT_A))?);
    let along = sse.mul(n, q);
    let s = sse.hsum(along);
    done(&sse)?;
    if s >> 31 == 0 {
        return Ok(Sides { mask, edx: tri });
    }
    put(j, at(state::NORMAL), n ^ vec(j, SIGNS)?)?;
    for off in [0x00, 0x40] {
        let first = vec(j, tri.wrapping_add(off))?;
        let second = vec(j, tri.wrapping_add(off + 0x10))?;
        put(j, tri.wrapping_add(off), second)?;
        put(j, tri.wrapping_add(off + 0x10), first)?;
    }
    // 0x0030350d: the first test goes through the x87 (`fld`, `fstp`).
    let first = word(j, at(state::SIDES))?;
    let second = word(j, at(state::SIDES + 4))?;
    j.put(at(state::SIDES), second).ok_or(Fallback::Memory)?;
    j.put(at(state::SIDES + 4), x.st(load(first))).ok_or(Fallback::Memory)?;
    Ok(Sides { mask, edx: second })
}

/// [`segments`]' answer: the case flags (eax and ecx) and the squared
/// distance's bits (edx).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Closest {
    pub flags: u32,
    pub distance: u32,
}

fn cmp(a: F80, b: F80) -> Cmp {
    x87::compare(a, b, false).0
}

/// `0x00308590`, cdecl `(p1, d1, p2, d2, out)`: the closest points of the
/// segments `p1 + s d1` and `p2 + t d2`, `s` and `t` clamped to `[0, 1]`.
/// `out` gets the point on the first, the difference to the point on the
/// second and its squared length. The dot products are SSE; the solve is
/// x87, its branches as the guest's flag tests take them. The flags say
/// which ends clamped: 1 `s = 1`, 2 `s = 0` (first pass or second),
/// 4 `t = 1`, 8 `t = 0`.
pub fn segments(j: &mut Journal, p1: u32, d1: u32, p2: u32, d2: u32, out: u32, fpu: Fpu) -> Result<Closest, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let zero = load(word(j, ZERO)?);
    let one = load(word(j, ONE)?);
    let k = load(word(j, PARALLEL)?);
    let dir1 = vec(j, d1)?;
    let base1 = vec(j, p1)?;
    let w = sse.sub(vec(j, p2)?, base1);
    let dir2 = vec(j, d2)?;
    let m = sse.mul(dir1, dir2);
    let b = sse.hsum(m);
    let m = sse.mul(dir1, w);
    let d = sse.hsum(m);
    let m = sse.mul(dir2, w);
    let e = sse.hsum(m);
    let m = sse.mul(dir1, dir1);
    let a = sse.hsum(m);
    let m = sse.mul(dir2, dir2);
    let c = sse.hsum(m);
    done(&sse)?;
    let ca = x.st(x.mul(load(c), load(a)));
    let bb = x.st(x.mul(load(b), load(b)));
    let den = x.st(x.sub(load(ca), load(bb)).abs());
    // `fsubp`: c d - e b, kept in the register.
    let num_x = x.sub(x.mul(load(c), load(d)), x.mul(load(e), load(b)));
    let num = x.st(num_x);
    let numden = x.mul(num_x, load(den));
    let den2 = x.mul(load(den), load(den));
    let (mut flags, t) = match cmp(den2, numden) {
        // `test ah,0x41; jnp`: at or past the end.
        Cmp::Less | Cmp::Equal => (1, one),
        _ => match cmp(load(num), zero) {
            Cmp::Less | Cmp::Equal => (2, zero),
            _ => {
                let parallel = x.mul(x.add(load(bb), load(ca)), k);
                // `test ah,5; jp`: only below divides.
                if cmp(parallel, load(den)) == Cmp::Less { (0, x.div(load(num), load(den))) } else { (1, one) }
            }
        },
    };
    let s_x = x.sub(x.mul(load(b), t), load(e));
    let s = x.st(s_x);
    let (u, t) = match cmp(s_x, load(c)) {
        Cmp::Less | Cmp::Unordered => match cmp(load(s), zero) {
            // 0x0030879d: t stands.
            Cmp::Greater | Cmp::Unordered => (x.st(x.div(load(s), load(c))), t),
            _ => {
                flags = 8;
                (0, again(&x, b, 0, d, a, zero, one, &mut flags))
            }
        },
        _ => {
            flags = 4;
            (ONE_BITS, again(&x, b, ONE_BITS, d, a, zero, one, &mut flags))
        }
    };
    let t = x.st(t);
    let scaled = sse_mul_splat(&mut sse, t, dir1);
    let near1 = sse.add(base1, scaled);
    put(j, out, near1)?;
    let dir2 = vec(j, d2)?;
    let base2 = vec(j, p2)?;
    let scaled = sse_mul_splat(&mut sse, u, dir2);
    let near2 = sse.add(base2, scaled);
    let diff = sse.sub(near1, near2);
    put(j, out.wrapping_add(0x10), diff)?;
    let r = vec(j, out.wrapping_add(0x10))?;
    let m = sse.mul(r, r);
    let distance = sse.hsum(m);
    done(&sse)?;
    j.put(out.wrapping_add(0x20), distance).ok_or(Fallback::Memory)?;
    Ok(Closest { flags, distance })
}

const ONE_BITS: u32 = 0x3f80_0000;
const HALF_BITS: u32 = 0x3f00_0000;

/// `0x003032c0`, thiscall on the query state, `(cache, separation, out)`,
/// `ret 0xc`; the cache is not read. The separation is a direction with
/// the distance in `w`. Out: the negated direction, the distance, and
/// the contact point with a weight: A's support point when A's simplex is
/// one vertex (1.0), else B's moved back along the direction by the
/// distance when B's is (0.0), else the state's midpoint (0.5). Returns
/// edx at the return: the distance's bits on B's case, else the
/// separation pointer.
pub fn contact(j: &mut Journal, st: u32, sep: u32, out: u32, fpu: Fpu) -> Result<u32, Fallback> {
    let (mut sse, _) = units(fpu)?;
    let at = |off: u32| st.wrapping_add(off);
    let field = |off: u32| out.wrapping_add(off);
    let distance = word(j, sep.wrapping_add(0xc))?;
    j.put(field(contact_field::DISTANCE), distance).ok_or(Fallback::Memory)?;
    let normal = vec(j, sep)? ^ vec(j, SIGNS)?;
    put(j, field(contact_field::NORMAL), normal)?;
    let (point, weight, edx) = if word(j, at(state::COUNT_A))? == 1 {
        (vec(j, at(state::SUPPORT_A))?, ONE_BITS, sep)
    } else if word(j, at(state::COUNT_B))? == 1 {
        let distance = word(j, sep.wrapping_add(0xc))?;
        let back = sse.mul(splat(distance), normal);
        (sse.add(vec(j, at(state::SUPPORT_B))?, back), 0, distance)
    } else {
        (vec(j, at(state::MIDPOINT))?, HALF_BITS, sep)
    };
    j.put(field(contact_field::WEIGHT), weight).ok_or(Fallback::Memory)?;
    put(j, field(contact_field::POINT), point)?;
    done(&sse)?;
    Ok(edx)
}

/// `splat(s) * v`, the operand order of `mulps xmm2, xmm1`.
fn sse_mul_splat(sse: &mut Sse, s: u32, v: u128) -> u128 {
    sse.mul(splat(s), v)
}

/// 0x0030874f: with `t` clamped to `u`, `s` again from `b u + d` over `a`.
#[allow(clippy::too_many_arguments)]
fn again(x: &Fx, b: u32, u: u32, d: u32, a: u32, zero: F80, one: F80, flags: &mut u32) -> F80 {
    let tt_x = x.add(x.mul(load(b), load(u)), load(d));
    let tt = x.st(tt_x);
    match cmp(tt_x, zero) {
        Cmp::Less | Cmp::Equal => {
            *flags |= 2;
            zero
        }
        _ => match cmp(load(tt), load(a)) {
            Cmp::Less | Cmp::Unordered => x.div(load(tt), load(a)),
            _ => {
                *flags |= 1;
                one
            }
        },
    }
}

const MAX_BITS: u32 = 0x7f7f_ffff;
const MINUS_ONE_BITS: u32 = 0xbf80_0000;

/// [`tetrahedron`]'s answer: eax (a face index, or `-1` as `u32`) and ecx.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Face {
    pub eax: u32,
    pub ecx: u32,
}

/// `0x00303540`: eax the four vertices (`+0x00` to `+0x30`, the last the
/// newest), ebx the point, one stack word `s`, plain `ret`. Each face
/// opposite vertex 0, 1, 2 has the normal `(e) x (f)` over two of its
/// edges; its test is `n . s (p - v3)`, signed-squared and divided by
/// `|n|^2` (`FLT_MAX` for a zero normal). Below `1e-6` everywhere: `-1`.
/// One face past `1.1` times the next: that face. Otherwise
/// `0x00303110` picks among the three by their plain tests. Reads only;
/// the frame and the test array it hands down are dead at the return.
pub fn tetrahedron(j: &Journal, verts: u32, p: u32, s: u32, fpu: Fpu) -> Result<Face, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let vert = |i: u32| verts.wrapping_add(16 * i);
    let zero = load(word(j, ZERO)?);
    let v1 = vec(j, vert(1))?;
    let v0 = vec(j, vert(0))?;
    let v3 = vec(j, vert(3))?;
    let v2 = vec(j, vert(2))?;
    let e10 = sse.sub(v1, v0);
    let e02 = sse.sub(v0, v2);
    let e30 = sse.sub(v3, v0);
    let e31 = sse.sub(v3, v1);
    let e32 = sse.sub(v3, v2);
    let e21 = sse.sub(v2, v1);
    let n = [cross(&mut sse, e21, e31), cross(&mut sse, e02, e32), cross(&mut sse, e10, e30)];
    let to_p = sse.sub(vec(j, p)?, vec(j, vert(3))?);
    let q = sse.mul(splat(s), to_p);
    let (lo, n2lo) = (shuf2(n[0], n[1], 0x44), shuf1(n[2], 0x44));
    let zrow = shuf2(shuf2(n[0], n[1], 0xee), shuf1(n[2], 0xee), 0x88);
    let zs = sse.mul(zrow, splat(lane(q, 2)));
    let ys = sse.mul(shuf2(lo, n2lo, 0xdd), splat(lane(q, 1)));
    let xs = sse.mul(shuf2(lo, n2lo, 0x88), splat(lane(q, 0)));
    let xy = sse.add(xs, ys);
    let d = sse.add(xy, zs);
    let magnitude = d & vec(j, ABS)?;
    let signed = sse.mul(d, magnitude);
    let mut r = [0u32; 3];
    for (i, n) in n.into_iter().enumerate() {
        let m = sse.mul(n, n);
        let sq = sse.hsum(m);
        // `fucompp; test ah,0x44; jnp`: only an equal zero gives FLT_MAX.
        r[i] = if x87::compare(load(sq), zero, true).0 == Cmp::Equal {
            MAX_BITS
        } else {
            x.st(x.div(load(lane(signed, i)), load(sq)))
        };
    }
    done(&sse)?;
    // 0x003036ec: the largest test into the third slot, the next in st0.
    let (mut index, m, low) = if cmp(load(r[0]), load(r[1])) == Cmp::Greater { (0, load(r[0]), x.st(load(r[1]))) } else { (1, load(r[1]), r[0]) };
    let (largest, second) = if cmp(m, load(r[2])) == Cmp::Greater {
        let next = if cmp(load(low), load(r[2])) == Cmp::Greater { load(low) } else { load(r[2]) };
        (x.st(m), next)
    } else {
        index = 2;
        (r[2], m)
    };
    if cmp(load(largest), load(word(j, TINY)?)) == Cmp::Less {
        return Ok(Face { eax: u32::MAX, ecx: index });
    }
    let reach = x.mul(load(word(j, MARGIN)?), second);
    if cmp(load(largest), reach) == Cmp::Greater {
        return Ok(Face { eax: index, ecx: index });
    }
    let mut w = [lane(d, 0), lane(d, 1), lane(d, 2)];
    let eax = drop_edge(j, verts, p, &mut w, fpu)?;
    Ok(Face { eax, ecx: vert(3) })
}

/// `0x00303110`, cdecl `(point, tests)` with edi the vertices: for each
/// face with both neighbours' tests not below zero, the neighbour whose
/// edge the point projects past less is struck (`-1.0`); then the first
/// test above zero names the face, or `-1`.
fn drop_edge(j: &Journal, verts: u32, p: u32, w: &mut [u32; 3], fpu: Fpu) -> Result<u32, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let zero = load(word(j, ZERO)?);
    let vert = |i: usize| verts.wrapping_add(16 * i as u32);
    for i in 0..3 {
        // The tables at 0x004136b2 and 0x004136b0.
        let (c, e) = ((i + 1) % 3, (i + 2) % 3);
        if matches!(cmp(load(w[c]), zero), Cmp::Less | Cmp::Unordered) || matches!(cmp(load(w[e]), zero), Cmp::Less | Cmp::Unordered) {
            continue;
        }
        let v = vec(j, vert(i))?;
        let to_c = sse.sub(vec(j, vert(c))?, v);
        let to_q = sse.sub(vec(j, vert(3))?, v);
        let to_e = sse.sub(vec(j, vert(e))?, v);
        let to_p = sse.sub(vec(j, p)?, v);
        let m = sse.mul(to_p, to_c);
        let a = sse.hsum(m);
        let m = sse.mul(to_p, to_e);
        let b = sse.hsum(m);
        let m = sse.mul(to_q, to_c);
        let c_ = sse.hsum(m);
        let m = sse.mul(to_q, to_e);
        let d = sse.hsum(m);
        let cb = x.mul(load(c_), load(b));
        let da = x.mul(load(d), load(a));
        if cmp(cb, da) == Cmp::Greater {
            w[e] = MINUS_ONE_BITS;
        } else {
            w[c] = MINUS_ONE_BITS;
        }
    }
    done(&sse)?;
    Ok((0..3).find(|&k| cmp(load(w[k]), zero) == Cmp::Greater).map_or(u32::MAX, |k| k as u32))
}

/// [`edge`]'s answer: eax (`1` the segment meets the triangle inside it,
/// `0` the segment drops to one vertex, `2` the triangle to an edge), and
/// ecx and edx at the return.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// `dec dword ptr [at]`.
fn dec(j: &mut Journal, at: u32) -> Result<(), Fallback> {
    let n = word(j, at)?;
    j.put(at, n.wrapping_sub(1)).ok_or(Fallback::Memory)
}

/// `movaps` from one slot to another.
fn moved(j: &mut Journal, to: u32, from: u32) -> Result<(), Fallback> {
    let v = vec(j, from)?;
    put(j, to, v)
}

/// [`edge`]'s x87 tie-break between two edges: `(new[b] - old[b]) old[a]`
/// below `(new[a] - old[a]) old[b]`, `old` the sides at entry and `new`
/// the state's after the second vertex's test (`fcompp; test ah,5; jnp`).
fn skew(x: &Fx, j: &Journal, st: u32, old: u128, a: u32, b: u32) -> Result<bool, Fallback> {
    let new = |i: u32| word(j, st.wrapping_add(state::SIDES + 4 * i)).map(load);
    let old = |i: u32| load(lane(old, i as usize));
    let first = x.mul(x.sub(new(a)?, old(a)), old(b));
    let second = x.mul(x.sub(new(b)?, old(b)), old(a));
    Ok(cmp(second, first) == Cmp::Less)
}

/// Where the segment from `a` (test `da`) to `b` (test `db`) crosses the
/// plane: `t = da / (da - db)`, then `(1 - t) a + t b`.
fn crossing(j: &Journal, sse: &mut Sse, a: u128, b: u128, da: u32, db: u32) -> Result<u128, Fallback> {
    let span = sse.e.sub(da, db);
    let t = splat(sse.e.div(da, span));
    let rest = sse.sub(vec(j, ONES)?, t);
    let far = sse.mul(t, b);
    let near = sse.mul(rest, a);
    Ok(sse.add(near, far))
}

/// `n . (v - t0)`, the guest's horizontal sum.
fn height(sse: &mut Sse, n: u128, v: u128, t0: u128) -> u32 {
    let d = sse.sub(v, t0);
    let m = sse.mul(n, d);
    sse.hsum(m)
}

/// `0x003037a0`, thiscall on the query state, `(segment, triangle,
/// segment count, triangle count, fresh, feature)`, `ret 0x18`; `esp` is
/// the stack pointer at entry, for the frame the calls below read their
/// vectors from. The segment is two vertices at `+0x00`, `+0x10`, the
/// triangle three at `+0x00` to `+0x20`, each with a parallel array at
/// `+0x40`. Only `fresh`'s low byte is tested.
///
/// Fresh: the triangle's normal is `(t1 - t2) x (t0 - t2)`. A segment
/// whose ends lie either side of the plane meets it at a point, and a
/// point [`triangle`] finds inside answers `1`. Ends on one side: the
/// nearer goes first (both arrays swap) and is tested with `orient`;
/// inside, the segment count drops and the answer is `0`. Otherwise the
/// segment's closest approach ([`segments`]) to the edges `t2 t1` and
/// `t2 t0` chooses the nearer, `t2` (both arrays) overwrites the vertex
/// off it, the triangle count drops, and the answer is `2`.
///
/// Not fresh: the state's normal and sides stand. A crossing inside
/// answers `1`; the second vertex inside drops the segment to it (both
/// arrays) and stores `feature` at `+0x10`, answer `0`; else the second
/// vertex's edge mask, against the old sides where two edges compete,
/// names the vertex `t2` overwrites (or none), the triangle count drops,
/// and the answer is `2`.
#[allow(clippy::too_many_arguments)]
pub fn edge(j: &mut Journal, st: u32, esp: u32, seg: u32, tri: u32, seg_count: u32, tri_count: u32, fresh: u32, feature: u32, fpu: Fpu) -> Result<Edge, Fallback> {
    let (mut sse, x) = units(fpu)?;
    let at = |off: u32| st.wrapping_add(off);
    // `push ebp; and esp,-16; sub esp,0xa4`, then three pushes.
    let frame = |off: u32| (esp.wrapping_sub(4) & !15).wrapping_sub(0xb0).wrapping_add(off);
    let s_at = |i: u32| seg.wrapping_add(16 * i);
    let t_at = |i: u32| tri.wrapping_add(16 * i);
    if fresh & 0xff == 0 {
        let sides = vec(j, at(state::SIDES))?;
        let t0 = vec(j, t_at(0))?;
        let b1 = vec(j, s_at(1))?;
        let n = vec(j, at(state::NORMAL))?;
        let d1 = height(&mut sse, n, b1, t0);
        let b0 = vec(j, s_at(0))?;
        let d0 = height(&mut sse, n, b0, t0);
        let across = sse.e.mul(d0, d1) >> 31 != 0;
        let p = if across { Some(crossing(j, &mut sse, b0, b1, d0, d1)?) } else { None };
        done(&sse)?;
        if let Some(p) = p {
            put(j, frame(0x30), p)?;
            let s = triangle(j, st, frame(0x30), tri, 0, fpu)?;
            if s.mask == 7 {
                return Ok(Edge { eax: 1, ecx: st, edx: s.edx });
            }
        }
        let s = triangle(j, st, s_at(1), tri, 0, fpu)?;
        if s.mask == 7 {
            dec(j, seg_count)?;
            moved(j, s_at(0), s_at(1))?;
            moved(j, s_at(4), s_at(5))?;
            j.put(at(state::FEATURE), feature).ok_or(Fallback::Memory)?;
            return Ok(Edge { eax: 0, ecx: st, edx: feature });
        }
        // 0x00303a89: the vertex off the edge the second vertex lies past.
        let view: &Journal = j;
        let below = |a, b| skew(&x, view, st, sides, a, b);
        let off = match s.mask {
            3 => None,
            5 => Some(1),
            6 => Some(0),
            1 => (!below(1, 2)?).then_some(1),
            2 => below(2, 0)?.then_some(0),
            4 => Some(if below(0, 1)? { 1 } else { 0 }),
            _ => Some(0),
        };
        dec(j, tri_count)?;
        if let Some(k) = off {
            moved(j, t_at(k), t_at(2))?;
            moved(j, t_at(k + 4), t_at(6))?;
        }
        return Ok(Edge { eax: 2, ecx: st, edx: s.edx });
    }
    let t0 = vec(j, t_at(0))?;
    let t2 = vec(j, t_at(2))?;
    let t1 = vec(j, t_at(1))?;
    let u = sse.sub(t1, t2);
    let w = sse.sub(t0, t2);
    let n = cross(&mut sse, u, w);
    put(j, frame(0x30), w)?;
    put(j, frame(0x40), u)?;
    let s0 = vec(j, s_at(0))?;
    let d0 = height(&mut sse, n, s0, t0);
    let s1 = vec(j, s_at(1))?;
    let d1 = height(&mut sse, n, s1, t0);
    // `movmskps` both, `xor`, `test al,1`.
    let p = if (d0 ^ d1) >> 31 != 0 { Some(crossing(j, &mut sse, s0, s1, d0, d1)?) } else { None };
    done(&sse)?;
    if let Some(p) = p {
        put(j, frame(0x10), p)?;
        let s = triangle(j, st, frame(0x10), tri, 0, fpu)?;
        if s.mask == 7 {
            return Ok(Edge { eax: 1, ecx: st, edx: s.edx });
        }
    } else {
        // `fabs` both, `fcompp; test ah,5; jp`: only a nearer second end swaps.
        if cmp(load(d1).abs(), load(d0).abs()) == Cmp::Less {
            moved(j, s_at(0), s_at(1))?;
            put(j, s_at(1), s0)?;
            let a = vec(j, s_at(4))?;
            let b = vec(j, s_at(5))?;
            put(j, s_at(4), b)?;
            put(j, s_at(5), a)?;
        }
        let s = triangle(j, st, seg, tri, 1, fpu)?;
        if s.mask == 7 {
            dec(j, seg_count)?;
            return Ok(Edge { eax: 0, ecx: st, edx: s.edx });
        }
    }
    // 0x0030390f: the segment against the edges from t2 the frame holds.
    let along = sse.sub(vec(j, s_at(1))?, vec(j, s_at(0))?);
    done(&sse)?;
    put(j, frame(0x10), along)?;
    let first = segments(j, seg, frame(0x10), t_at(2), frame(0x40), frame(0x50), fpu)?;
    let second = segments(j, seg, frame(0x10), t_at(2), frame(0x30), frame(0x80), fpu)?;
    // `fcomp; test ah,0x41; jne`: only a farther first edge keeps t0.
    let k = u32::from(cmp(load(first.distance), load(second.distance)) == Cmp::Greater);
    moved(j, t_at(k), t_at(2))?;
    moved(j, t_at(k + 4), t_at(6))?;
    dec(j, tri_count)?;
    Ok(Edge { eax: 2, ecx: k << 4, edx: second.distance })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::Words;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Heap(HashMap<u32, u32>);

    impl Words for Heap {
        fn get(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied().or(Some(0))
        }
    }

    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    fn put4(h: &mut Heap, at: u32, v: [f32; 4]) {
        for (i, x) in v.into_iter().enumerate() {
            h.0.insert(at + 4 * i as u32, x.to_bits());
        }
    }

    fn get4(j: &Journal, at: u32) -> [f32; 4] {
        std::array::from_fn(|i| f32::from_bits(j.get(at + 4 * i as u32).unwrap()))
    }

    fn constants(h: &mut Heap) {
        h.0.insert(ONE, ONE_BITS);
        h.0.insert(PARALLEL, 0x3580_0000);
        for i in 0..4 {
            h.0.insert(SIGNS + 4 * i, 0x8000_0000);
        }
    }

    #[test]
    fn shuffles_pick_two_lanes_from_each_side() {
        let d = 0x4444_4444_3333_3333_2222_2222_1111_1111u128;
        let s = 0x8888_8888_7777_7777_6666_6666_5555_5555u128;
        assert_eq!(shuf2(d, s, 0x88), 0x7777_7777_5555_5555_3333_3333_1111_1111);
        assert_eq!(shuf2(d, s, 0xdd), 0x8888_8888_6666_6666_4444_4444_2222_2222);
        assert_eq!(shuf1(d, 0xc9), 0x4444_4444_1111_1111_3333_3333_2222_2222);
        assert_eq!(signs(0x8000_0000_0000_0000_8000_0000_0000_0000), 0b1010);
    }

    #[test]
    fn a_transform_in_its_own_frame_is_the_identity() {
        let mut h = Heap::default();
        let m = 0x1000;
        put4(&mut h, m, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, m + 0x10, [-1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, m + 0x20, [0.0, 0.0, 1.0, 0.0]);
        put4(&mut h, m + 0x30, [3.0, 4.0, 5.0, 1.0]);
        let mut j = Journal::new(&h);
        relative(&mut j, 0x2000, m, m, FPU).unwrap();
        assert_eq!(get4(&j, 0x2000)[..3], [1.0, 0.0, 0.0]);
        assert_eq!(get4(&j, 0x2010)[..3], [0.0, 1.0, 0.0]);
        assert_eq!(get4(&j, 0x2030)[..3], [0.0, 0.0, 0.0]);
        put4(&mut h, 0x3000, [1.0, 2.0, 3.0, 0.0]);
        let mut j = Journal::new(&h);
        rotate(&mut j, 0x2000, m, 0x3000, FPU).unwrap();
        assert_eq!(get4(&j, 0x2000)[..3], [-2.0, 1.0, 3.0]);
        assert_eq!(rotate(&mut Journal::new(&h), 0x2008, m, 0x3000, FPU), Err(Fallback::Memory));
    }

    #[test]
    fn crossing_segments_meet_inside_and_parallel_ones_clamp() {
        let mut h = Heap::default();
        constants(&mut h);
        // x from -1 to 1 at z 0; y from -1 to 1 at z 2: closest at the middles.
        put4(&mut h, 0x100, [-1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x110, [2.0, 0.0, 0.0, 0.0]);
        put4(&mut h, 0x120, [0.0, -1.0, 2.0, 0.0]);
        put4(&mut h, 0x130, [0.0, 2.0, 0.0, 0.0]);
        let mut j = Journal::new(&h);
        let r = segments(&mut j, 0x100, 0x110, 0x120, 0x130, 0x200, FPU).unwrap();
        assert_eq!(r, Closest { flags: 0, distance: 4.0f32.to_bits() });
        assert_eq!(get4(&j, 0x200)[..3], [0.0, 0.0, 0.0]);
        assert_eq!(get4(&j, 0x210)[..3], [0.0, 0.0, -2.0]);
        // The second parallel to the first and past its end.
        put4(&mut h, 0x120, [5.0, 1.0, 0.0, 0.0]);
        put4(&mut h, 0x130, [1.0, 0.0, 0.0, 0.0]);
        let mut j = Journal::new(&h);
        let r = segments(&mut j, 0x100, 0x110, 0x120, 0x130, 0x200, FPU).unwrap();
        assert_eq!(r.flags & 1, 1, "s clamps to the first segment's end");
        assert_eq!(get4(&j, 0x200)[..3], [1.0, 0.0, 0.0]);
    }

    #[test]
    fn a_point_beyond_one_face_names_it() {
        let mut h = Heap::default();
        constants(&mut h);
        h.0.insert(TINY, 1e-6f32.to_bits());
        h.0.insert(MARGIN, 1.1f32.to_bits());
        for i in 0..4 {
            h.0.insert(ABS + 4 * i, 0x7fff_ffff);
        }
        let (verts, p) = (0x1000, 0x2000);
        put4(&mut h, verts, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, verts + 0x10, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, verts + 0x20, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, verts + 0x30, [0.0, 0.0, 1.0, 0.0]);
        // At v3 every test is zero: no face.
        put4(&mut h, p, [0.0, 0.0, 1.0, 0.0]);
        let one = 1.0f32.to_bits();
        assert_eq!(tetrahedron(&Journal::new(&h), verts, p, one, FPU).map(|f| f.eax), Ok(u32::MAX));
        // Far along -x, one face's test dominates.
        put4(&mut h, p, [-5.0, 0.1, 0.1, 0.0]);
        let face = tetrahedron(&Journal::new(&h), verts, p, one, FPU).unwrap();
        assert!(face.eax < 3, "{face:?}");
        assert_eq!(face.eax, face.ecx);
        assert_eq!(tetrahedron(&Journal::new(&h), verts + 8, p, one, FPU), Err(Fallback::Memory));
    }

    #[test]
    fn a_segment_through_a_triangle_meets_it_and_one_beside_it_drops_a_vertex() {
        let mut h = Heap::default();
        constants(&mut h);
        for i in 0..4 {
            h.0.insert(ONES + 4 * i, ONE_BITS);
        }
        let (st, seg, tri, counts, esp) = (0x1000, 0x2000, 0x3000, 0x4000, 0x8000);
        put4(&mut h, tri, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x10, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x20, [0.0, 1.0, 0.0, 0.0]);
        h.0.insert(counts, 2);
        h.0.insert(counts + 4, 3);
        let run = |h: &Heap, fresh: u32| {
            let mut j = Journal::new(h);
            let e = edge(&mut j, st, esp, seg, tri, counts, counts + 4, fresh, 9, FPU);
            (e, j.get(counts).unwrap(), j.get(counts + 4).unwrap())
        };
        // Through the middle: it meets the plane inside.
        put4(&mut h, seg, [0.25, 0.25, 1.0, 0.0]);
        put4(&mut h, seg + 0x10, [0.25, 0.25, -1.0, 0.0]);
        assert_eq!(run(&h, 1), (Ok(Edge { eax: 1, ecx: st, edx: tri }), 2, 3));
        // Above the triangle, nearer end first: the segment drops to it.
        put4(&mut h, seg + 0x10, [0.25, 0.25, 2.0, 0.0]);
        assert_eq!(run(&h, 1).0.map(|e| e.eax), Ok(0));
        assert_eq!(run(&h, 1).1, 1);
        // Off to the side: the triangle loses a vertex to t2.
        put4(&mut h, seg, [3.0, 3.0, 1.0, 0.0]);
        put4(&mut h, seg + 0x10, [4.0, 4.0, 1.0, 0.0]);
        let (e, a, b) = run(&h, 1);
        let e = e.unwrap();
        assert_eq!((e.eax, e.ecx & !0x10, a, b), (2, 0, 2, 2), "{e:?}");
        // Only the low byte says fresh; a zero normal never crosses.
        assert_eq!(run(&h, 0x100).0.map(|e| e.eax), Ok(2));
        assert_eq!(run(&h, 0x100).0.map(|e| e.ecx), Ok(st));
        let mut j = Journal::new(&h);
        assert_eq!(edge(&mut j, st, esp, seg + 8, tri, counts, counts + 4, 1, 9, FPU), Err(Fallback::Memory));
    }

    #[test]
    fn a_point_inside_a_triangle_sets_every_side() {
        let mut h = Heap::default();
        constants(&mut h);
        let (st, tri, p) = (0x1000, 0x2000, 0x3000);
        put4(&mut h, tri, [0.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x10, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, tri + 0x20, [0.0, 1.0, 0.0, 0.0]);
        put4(&mut h, p, [0.25, 0.25, 1.0, 0.0]);
        let inside = triangle(&mut Journal::new(&h), st, p, tri, 0, FPU).unwrap().mask;
        put4(&mut h, p, [2.0, 2.0, 1.0, 0.0]);
        let outside = triangle(&mut Journal::new(&h), st, p, tri, 0, FPU).unwrap().mask;
        assert!(inside == 0 || inside == 7, "{inside:#b}");
        assert_ne!(inside, outside);
        assert_eq!(triangle(&mut Journal::new(&h), st + 8, p, tri, 0, FPU), Err(Fallback::Memory));
    }
}
