//! The node-matrix arithmetic the slice 4 bodies share
//! (docs/engine/object-update.md, "Slice 4 bodies"): a point or a vector
//! through a `34`-byte node matrix (`+0` scale, `+4` forward, `+10` left,
//! `+1c` up, `+28` translation), on the guest's own `xmm` registers.
//!
//! Retail inlines the same instruction sequence in the bounds update
//! `bdef0`, the world position `b9dd0` and the world basis `b9fc0`. Each
//! component is `(m_z·z + m_y·y) + m_x·x (+ t)`, scalar SSE in that order,
//! with `x`, `y`, `z` in `xmm0`, `xmm1`, `xmm2` and the row built in
//! `xmm3` and `xmm4`. The helpers leave every `xmm` register as that
//! sequence does, so a callee or the lifted code that takes over sees
//! retail's float state.

use super::Result;
use super::sse::{self, Ss};
use crate::sim_physics::logic;
use runtime::{CpuAbi, Guest};

/// The image's 1.0 the scale test compares with (`ucomiss xmm3, [45dbc0]`).
pub(super) const ONE: u32 = 0x0045_dbc0;

/// `movss xmm<r>, [addr]`.
pub(super) fn load(c: &mut CpuAbi, g: &Guest, r: usize, addr: u32) -> Result<()> {
    let v = g.u32(addr)?;
    sse::load(c, r, v);
    Ok(())
}

/// `movss [addr], xmm<r>`.
pub(super) fn store(c: &CpuAbi, g: &mut Guest, addr: u32, r: usize) -> Result<()> {
    g.set_u32(addr, sse::lane0(c, r))
}

/// `op xmm<r>, [addr]`.
pub(super) fn ss_mem(c: &mut CpuAbi, g: &Guest, op: Ss, r: usize, addr: u32) -> Result<()> {
    let v = g.u32(addr)?;
    sse::ss(c, op, r, v);
    Ok(())
}

/// `ucomiss xmm<r>, [addr]; lahf; test ah, 44h`, then whether `jnp` is
/// taken: the operands compare equal (ZF without PF).
pub(super) fn ucomiss_equal(c: &mut CpuAbi, g: &Guest, r: usize, addr: u32) -> Result<bool> {
    let b = g.u32(addr)?;
    sse::ucomiss(c, sse::lane0(c, r), b);
    lahf_test_44(c);
    Ok(!sse::parity(c))
}

/// `ucomiss xmm<r>, b` with `b` already read, then `lahf; test ah, 44h`;
/// whether `jnp` is taken.
pub(super) fn ucomiss_equal_bits(c: &mut CpuAbi, r: usize, b: u32) -> bool {
    sse::ucomiss(c, sse::lane0(c, r), b);
    lahf_test_44(c);
    !sse::parity(c)
}

/// `lahf; test ah, 44h`: `ah` replaced, flags of the `test`.
fn lahf_test_44(c: &mut CpuAbi) {
    sse::lahf(c);
    logic(c, (c.eax >> 8) & 0x44, 0x80);
}

/// The scale step before a point transform, when the matrix scale in
/// `xmm3` is not 1: each of `xmm0` to `xmm2` multiplied by the scale at
/// `[m]` (`mulss xmm3, xmmN; movaps xmmN, xmm3; movss xmm3, [m]`).
pub(super) fn scale(c: &mut CpuAbi, g: &Guest, m: u32) -> Result<()> {
    sse::ss_reg(c, Ss::Mul, 3, 0);
    sse::movaps(c, 0, 3);
    load(c, g, 3, m)?;
    sse::ss_reg(c, Ss::Mul, 3, 1);
    sse::movaps(c, 1, 3);
    load(c, g, 3, m)?;
    sse::ss_reg(c, Ss::Mul, 3, 2);
    sse::movaps(c, 2, 3);
    Ok(())
}

/// `x`, `y`, `z` in `xmm0` to `xmm2` through the rows of the matrix at
/// `m`, stored at `out`, `out + 4`, `out + 8`; with `translate`, each row
/// adds the matrix translation (a point), without it a vector. Leaves
/// `xmm3` the last component, `xmm4` the last row's second product,
/// `xmm2` and `xmm1` the third row's products, as retail.
pub(super) fn rows(c: &mut CpuAbi, g: &mut Guest, m: u32, out: u32, translate: bool) -> Result<()> {
    for (i, (z, y, x, t)) in [(0x1c, 0x10, 0x04, 0x28), (0x20, 0x14, 0x08, 0x2c)].into_iter().enumerate() {
        load(c, g, 3, m.wrapping_add(z))?;
        load(c, g, 4, m.wrapping_add(y))?;
        sse::ss_reg(c, Ss::Mul, 3, 2);
        sse::ss_reg(c, Ss::Mul, 4, 1);
        sse::ss_reg(c, Ss::Add, 3, 4);
        load(c, g, 4, m.wrapping_add(x))?;
        sse::ss_reg(c, Ss::Mul, 4, 0);
        sse::ss_reg(c, Ss::Add, 3, 4);
        if translate {
            ss_mem(c, g, Ss::Add, 3, m.wrapping_add(t))?;
        }
        store(c, g, out.wrapping_add(4 * i as u32), 3)?;
    }
    load(c, g, 3, m.wrapping_add(0x24))?;
    sse::ss_reg(c, Ss::Mul, 3, 2);
    load(c, g, 2, m.wrapping_add(0x18))?;
    sse::ss_reg(c, Ss::Mul, 2, 1);
    load(c, g, 1, m.wrapping_add(0x0c))?;
    sse::ss_reg(c, Ss::Mul, 1, 0);
    sse::ss_reg(c, Ss::Add, 3, 2);
    sse::ss_reg(c, Ss::Add, 3, 1);
    if translate {
        ss_mem(c, g, Ss::Add, 3, m.wrapping_add(0x30))?;
    }
    store(c, g, out.wrapping_add(8), 3)
}

/// `mov` of three dwords from `from` to `to` (a vector copy through an
/// integer register, as retail's `mov r, [a]; mov [b], r` triples do).
pub(super) fn copy3(g: &mut Guest, from: u32, to: u32) -> Result<[u32; 3]> {
    let mut v = [0; 3];
    for (i, w) in v.iter_mut().enumerate() {
        let off = 4 * i as u32;
        *w = g.u32(from.wrapping_add(off))?;
        g.set_u32(to.wrapping_add(off), *w)?;
    }
    Ok(v)
}
