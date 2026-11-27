//! The convex shapes' support virtuals, `vt[+0x30]`, that the
//! closest-feature query calls for new support points
//! (docs/engine/havok-convex.md, "Support virtuals"). Each is thiscall
//! on the shape with the direction and the output vertex, `ret 8`, and
//! each is one guest function on a [`Journal`], every float operation one
//! guest instruction: packed and scalar SSE under the MXCSR, x87 through
//! the `x87::abi` helpers the lifted code runs.
//!
//! - [`capsule`] `0x002ddd30`, type 7: the end point the direction favours.
//! - [`biped`] `0x002dbd40`, type `0x17`: the child pill's support, moved
//!   by the shape's offset when the direction faces it.
//! - [`polygon`] `0x001ee350`, type `0x18`: the vertex with the largest
//!   dot product.
//!
//! On a [`Fallback`] the journal is discarded and the MXCSR and the x87
//! state are left as they came.
use crate::convex::Fallback;
use crate::fp::Exact;
use crate::jacobian::{Sse, lane, put_vector, vector};
use crate::layout::{CAPSULE_A, CAPSULE_B};
use crate::mem::Journal;
use crate::structure;
use x87::abi::{self, X87Abi, src_kind, status};

pub const CAPSULE_SUPPORT: u32 = 0x002d_dd30;
pub const BIPED_SUPPORT: u32 = 0x002d_bd40;
pub const POLYGON_SUPPORT: u32 = 0x001e_e350;
/// The `0.0` the x87 compares the dot product with, and `-FLT_MAX`, the
/// polygon's starting best.
pub const ZERO: u32 = 0x0045_dbd8;
pub const LOWEST: u32 = 0x0044_09d4;
/// The high bits of a capsule's vertex ID; end B adds `0x10`.
pub const FEATURE_BITS: u32 = 0x3f00_0000;

/// The type-`0x17` wrapper's fields.
pub mod biped_field {
    /// The offset added to the child's support point.
    pub const OFFSET: u32 = 0x10;
    /// The child shape, the pill.
    pub const CHILD: u32 = 0x30;
}

/// The registers a support virtual leaves that its caller can see; ebx,
/// ebp, esi and edi are restored and esp is past the two arguments.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

/// x87 status bits a compare sets: C3, C2, C0.
const CC: u32 = 0x4500;
const LESS: u32 = 0x0100;

fn sse(mxcsr: u32) -> Result<Sse, Fallback> {
    if mxcsr & 0x1f80 != 0x1f80 {
        return Err(Fallback::Float);
    }
    Ok(Sse { e: Exact::new(mxcsr) })
}

fn masked(x: &X87Abi) -> Result<(), Fallback> {
    if x.control & 0x3f == 0x3f { Ok(()) } else { Err(Fallback::Float) }
}

fn done(sse: &Sse) -> Result<u32, Fallback> {
    if sse.e.trapped { Err(Fallback::Float) } else { Ok(sse.e.mxcsr) }
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

fn ok(r: u32) -> Result<u32, Fallback> {
    if r & status::MASK == status::OK { Ok(r) } else { Err(Fallback::Float) }
}

/// `fld m32` of the dot product's spill, then `fcomp` with [`ZERO`]: the
/// condition bits C3, C2, C0 of the compare.
fn against_zero(j: &Journal, x: &mut X87Abi, dot: u32) -> Result<u32, Fallback> {
    let zero = word(j, ZERO)?;
    ok(abi::ld_f32(x, dot))?;
    Ok(ok(abi::cmp(x, 0, src_kind::F32, u64::from(zero), 1, false))? & CC)
}

/// `v . d`: `y + x` then `z + that`, the `w` product dropped.
fn dot(sse: &mut Sse, v: u128, d: u128) -> u32 {
    let p = sse.mul(v, d);
    sse.hsum(p)
}

/// Type 7, `0x002ddd30`: end A (`+0x10`) when `(B - A) . d` is below zero,
/// else end B (`+0x20`), equal and unordered included; the vertex goes to
/// `out` with its `w` the ID, `0x3f000000` for A and `0x3f000010` for B.
/// Returns eax, the ID; ecx is `out` and edx is kept.
pub fn capsule(
    j: &mut Journal,
    this: u32,
    dir: u32,
    out: u32,
    mxcsr: &mut u32,
    x: &mut X87Abi,
) -> Result<u32, Fallback> {
    masked(x)?;
    let mut sse = sse(*mxcsr)?;
    let mut fx = *x;
    let b = vec(j, this.wrapping_add(CAPSULE_B))?;
    let a = vec(j, this.wrapping_add(CAPSULE_A))?;
    let d = vec(j, dir)?;
    let axis = sse.sub(b, a);
    let along = dot(&mut sse, axis, d);
    let m = done(&sse)?;
    // 0x002ddd7d: `test ah, 5; jp` takes B on 000 (above), 100 (equal)
    // and 111 (unordered).
    let (v, id) = if against_zero(j, &mut fx, along)? == LESS {
        (a, FEATURE_BITS)
    } else {
        (b, FEATURE_BITS | 0x10)
    };
    put(j, out, v)?;
    j.put(out.wrapping_add(0xc), id).ok_or(Fallback::Memory)?;
    (*mxcsr, *x) = (m, fx);
    Ok(id)
}

/// Type `0x17`, `0x002dbd40`: the child at `+0x30` answers through its
/// own `vt[+0x30]` (`child`: the journal, the child, its entry esp, the
/// MXCSR and the x87 state); then, when `offset . d` is above zero, the
/// offset at `+0x10` is added to the child's point, all four lanes,
/// and the point's `w` is stored back through the x87 as the child left
/// it (`fld`, `fstp`: an SNaN comes back quiet).
///
/// `esp` is the entry stack pointer (at the return address). Returns ax
/// the x87 status word after the compare over the child's eax, ecx the
/// frame word `((esp - 4) & !15) - 4`, and the child's edx.
#[allow(clippy::too_many_arguments)]
pub fn biped(
    j: &mut Journal,
    this: u32,
    dir: u32,
    out: u32,
    esp: u32,
    mxcsr: &mut u32,
    x: &mut X87Abi,
    child: impl FnOnce(&mut Journal, u32, u32, &mut u32, &mut X87Abi) -> Result<Exit, Fallback>,
) -> Result<Exit, Fallback> {
    masked(x)?;
    sse(*mxcsr)?;
    let (mut m, mut fx) = (*mxcsr, *x);
    let frame = esp.wrapping_sub(4) & !15;
    let shape = word(j, this.wrapping_add(biped_field::CHILD))?;
    // ecx, ebx, esi, edi saved, then edi, ebx and the return address.
    let inner = child(j, shape, frame.wrapping_sub(28), &mut m, &mut fx)?;
    masked(&fx)?;
    let mut sse = sse(m)?;
    let offset = vec(j, this.wrapping_add(biped_field::OFFSET))?;
    let d = vec(j, dir)?;
    let along = dot(&mut sse, offset, d);
    let cc = against_zero(j, &mut fx, along)?;
    let eax = inner.eax & 0xffff_0000 | u32::from(fx.status);
    // 0x002dbd93: `test ah, 0x41; jne` moves the point only when above.
    if cc == 0 {
        let w = word(j, out.wrapping_add(0xc))?;
        ok(abi::ld_f32(&mut fx, w))?;
        let point = vec(j, out)?;
        let moved = sse.add(point, offset);
        put(j, out, moved)?;
        let (r, stored) = abi::st_f32(&fx, 0);
        ok(r)?;
        j.put(out.wrapping_add(0xc), stored.value).ok_or(Fallback::Memory)?;
        abi::store_commit(&mut fx, stored.status, true);
    }
    let m = done(&sse)?;
    (*mxcsr, *x) = (m, fx);
    Ok(Exit { eax, ecx: frame.wrapping_sub(4), edx: inner.edx })
}

/// Type `0x18`, `0x001ee350`: over the vertex count at `+0x14` (signed;
/// none reads vertex 0), the vertex at `+0x20` whose `v . d` is the
/// largest, the first on ties, `comiss` against a best that starts at
/// [`LOWEST`] ([`structure::support`]); the whole vertex goes to `out`.
/// Returns eax `out`, ecx the shape and edx the count.
pub fn polygon(j: &mut Journal, this: u32, dir: u32, out: u32, mxcsr: &mut u32) -> Result<Exit, Fallback> {
    sse(*mxcsr)?;
    let count = word(j, this.wrapping_add(structure::field::COUNT))?;
    let n = (count as i32).max(0) as u32;
    if n > structure::MAX_RING {
        return Err(Fallback::Memory);
    }
    let best = word(j, LOWEST)?;
    let d = vec(j, dir)?;
    let at = |i: u32| this.wrapping_add(structure::field::VERTICES).wrapping_add(16 * i);
    let vertices = (0..n)
        .map(|i| vec(j, at(i)).map(|v| std::array::from_fn(|k| lane(v, k))))
        .collect::<Result<Vec<_>, _>>()?;
    let mut m = *mxcsr;
    let d = std::array::from_fn(|k| lane(d, k));
    let selected = structure::support(&vertices, d, best, &mut m).map_err(|_| Fallback::Float)?;
    let v = vec(j, at(selected as u32))?;
    put(j, out, v)?;
    *mxcsr = m;
    Ok(Exit { eax: out, ecx: this, edx: count })
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

    const SHAPE: u32 = 0x1000;
    const CHILD: u32 = 0x2000;
    const DIR: u32 = 0x3000;
    const OUT: u32 = 0x4000;
    const MXCSR: u32 = 0x1f80;

    fn put4(h: &mut Heap, at: u32, v: [f32; 4]) {
        for (i, x) in v.into_iter().enumerate() {
            h.0.insert(at + 4 * i as u32, x.to_bits());
        }
    }

    fn get4(j: &Journal, at: u32) -> [u32; 4] {
        std::array::from_fn(|i| j.get(at + 4 * i as u32).unwrap())
    }

    fn heap() -> Heap {
        let mut h = Heap::default();
        h.0.insert(LOWEST, f32::MIN.to_bits());
        h
    }

    fn fpu() -> X87Abi {
        let mut x = X87Abi::default();
        abi::init(&mut x);
        x.control = 0x027f;
        x
    }

    #[test]
    fn a_capsule_gives_the_end_the_direction_favours() {
        let mut h = heap();
        put4(&mut h, SHAPE + CAPSULE_A, [0.0, 0.0, -1.0, 0.25]);
        put4(&mut h, SHAPE + CAPSULE_B, [0.0, 0.0, 1.0, 0.25]);
        let mut cases = vec![([0.0, 0.0, -1.0, 0.0], CAPSULE_A, 0x3f00_0000), ([0.0, 0.0, 1.0, 0.0], CAPSULE_B, 0x3f00_0010)];
        // Perpendicular: exactly zero takes B; so does an unordered dot.
        cases.push(([1.0, 0.0, 0.0, 0.0], CAPSULE_B, 0x3f00_0010));
        cases.push(([0.0, 0.0, f32::NAN, 0.0], CAPSULE_B, 0x3f00_0010));
        for (d, end, id) in cases {
            put4(&mut h, DIR, d);
            let (mut m, mut x) = (MXCSR, fpu());
            let mut j = Journal::new(&h);
            assert_eq!(capsule(&mut j, SHAPE, DIR, OUT, &mut m, &mut x), Ok(id), "{d:?}");
            let mut want = get4(&Journal::new(&h), SHAPE + end);
            want[3] = id;
            assert_eq!(get4(&j, OUT), want);
            assert_eq!(x.tag, 0xffff, "the compare pops what it loaded");
        }
        // The NaN raised IE through the x87 compare.
        let (mut m, mut x) = (MXCSR, fpu());
        capsule(&mut Journal::new(&h), SHAPE, DIR, OUT, &mut m, &mut x).unwrap();
        assert_eq!(x.status & 1, 1);
        assert_eq!(capsule(&mut Journal::new(&h), SHAPE, DIR, OUT + 8, &mut m, &mut x), Err(Fallback::Memory));
        let mut unmasked = fpu();
        unmasked.control = 0x027e;
        assert_eq!(capsule(&mut Journal::new(&h), SHAPE, DIR, OUT, &mut m, &mut unmasked), Err(Fallback::Float));
    }

    #[test]
    fn a_biped_moves_the_childs_point_only_toward_its_offset() {
        let mut h = heap();
        put4(&mut h, SHAPE + biped_field::OFFSET, [0.0, 0.0, 0.5, 0.0]);
        h.0.insert(SHAPE + biped_field::CHILD, CHILD);
        let snan = 0x7f80_0001;
        for (dz, cc) in [(1.0f32, 0), (-1.0, LESS), (0.0, EQUAL)] {
            let moved = cc == 0;
            put4(&mut h, DIR, [0.0, 0.0, dz, 0.0]);
            let (mut m, mut x) = (MXCSR, fpu());
            let mut j = Journal::new(&h);
            let exit = biped(&mut j, SHAPE, DIR, OUT, 0x7f00_0000, &mut m, &mut x, |j, shape, esp, _, _| {
                assert_eq!((shape, esp), (CHILD, 0x7eff_fff0 - 28));
                put_vector(j, OUT, u128::from(snan) << 96 | u128::from(1.0f32.to_bits())).unwrap();
                Ok(Exit { eax: 0x3f00_0010, ecx: OUT, edx: 7 })
            })
            .unwrap();
            assert_eq!((exit.ecx, exit.edx, exit.eax >> 16), (0x7eff_ffec, 7, 0x3f00), "{dz}");
            let got = get4(&j, OUT);
            assert_eq!(got[2], if moved { 0.5f32.to_bits() } else { 0 }, "{dz}");
            assert_eq!(got[3], if moved { snan | 0x0040_0000 } else { snan }, "{dz}: w through the x87");
            assert_eq!(exit.eax & CC, cc, "{dz}");
        }
    }

    #[test]
    fn a_polygon_gives_its_first_farthest_vertex() {
        let mut h = heap();
        h.0.insert(SHAPE + structure::field::COUNT, 3);
        let v = SHAPE + structure::field::VERTICES;
        put4(&mut h, v, [1.0, 0.0, 0.0, 1.0]);
        put4(&mut h, v + 0x10, [0.0, 2.0, 0.0, 2.0]);
        put4(&mut h, v + 0x20, [0.0, 2.0, 0.0, 3.0]);
        put4(&mut h, DIR, [0.0, 1.0, 0.0, 0.0]);
        let mut m = MXCSR;
        let mut j = Journal::new(&h);
        let exit = polygon(&mut j, SHAPE, DIR, OUT, &mut m).unwrap();
        assert_eq!(exit, Exit { eax: OUT, ecx: SHAPE, edx: 3 });
        assert_eq!(get4(&j, OUT), get4(&Journal::new(&h), v + 0x10), "the first of two ties");
        // No vertices: vertex 0 all the same.
        h.0.insert(SHAPE + structure::field::COUNT, (-2i32) as u32);
        let mut j = Journal::new(&h);
        assert_eq!(polygon(&mut j, SHAPE, DIR, OUT, &mut m).map(|e| e.edx), Ok((-2i32) as u32));
        assert_eq!(get4(&j, OUT), get4(&Journal::new(&h), v));
        assert_eq!(polygon(&mut Journal::new(&h), SHAPE, DIR + 4, OUT, &mut m), Err(Fallback::Memory));
    }
}
