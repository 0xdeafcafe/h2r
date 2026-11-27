//! The convex agent's process call `0x002eeb40` (docs/engine/havok-convex.md):
//! thiscall on the agent, four stack arguments (collidable A, collidable
//! B, the collision input, the result collector), `ret 0x10`. The convex
//! agent `0x00412c48` (its `vt[+0x10]`) and the biped wrapper's agent
//! `0x00412290` (`0x002db5e0`) call it.
//!
//! [`process`] runs its separated-pair path on a [`Journal`]: with a
//! positive separation bound, the bound is projected along the cached
//! axis by both bodies' motion and, when it stays positive, stored, and
//! the agent drops its manifold. Every float operation is one guest
//! instruction: x87 under the control word, packed and scalar SSE under
//! the MXCSR. The closest-feature query (`0x003048a0`, `0x003032c0`), the
//! manifold update (`0x00302c70`, `0x00302eb0`) and a manifold with points
//! to remove through the contact manager stay the guest's
//! ([`Fallback::Closest`], [`Fallback::Points`]). On a [`Fallback`] the
//! journal is discarded and nothing has been written.
use crate::fp::Exact;
use crate::jacobian::{above, vector, Fpu, Fx, Sse};
use crate::mem::Journal;
use crate::solver::{finite, load};
use std::sync::atomic::{AtomicU64, Ordering};
use x87::Mode;

/// The agent's fields, by byte offset.
pub mod field {
    /// The contact manager the manifold's points belong to.
    pub const MANAGER: u32 = 0x08;
    /// The four support vertex IDs and the two counts (bytes `+0x14`, `+0x15`).
    pub const FEATURES: u32 = 0x0c;
    /// The manifold's point count; the slots are at [`SLOTS`] + `0x30 i`.
    pub const POINTS: u32 = 0x18;
    /// The cached separating axis (`xyzw`, 16-byte aligned).
    pub const AXIS: u32 = 0x20;
    /// The cached separation bound; not positive runs the closest-feature query.
    pub const BOUND: u32 = 0x2c;
    pub const SLOTS: u32 = 0x30;
}

/// The collidable's shape and motion words. The motion holds two
/// positions at `+0x00` and `+0x10`, and the word at `+0x0c` (the first
/// position's `w`) is the motion's allowance added to the bound's
/// projection (**S**: the step's start and end; a rotation allowance).
pub const COLLIDABLE_SHAPE: u32 = 0x00;
pub const COLLIDABLE_MOTION: u32 = 0x08;
pub const MOTION_FROM: u32 = 0x00;
pub const MOTION_ALLOWANCE: u32 = 0x0c;
pub const MOTION_TO: u32 = 0x10;
/// The `0.0` both bound tests compare with.
pub const ZERO: u32 = 0x0045_dbd8;

/// The call's arguments: the agent in ecx, then the two collidables. The
/// input and the collector are not read on the separated path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    pub agent: u32,
    pub a: u32,
    pub b: u32,
}

/// The separated path's result: the bound stored, and the manager word
/// the removal call reads (edx at the return).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Separated {
    pub bound: u32,
    pub manager: u32,
}

/// Why a process call runs the guest. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the path reads is unmapped or unaligned, a vector is not
    /// 16-byte aligned, or a store is not plain writable memory.
    Memory,
    /// The bound is not positive before or after the projection: the
    /// closest-feature query and the manifold update run.
    Closest,
    /// The pair is separated but its manifold holds points, which the
    /// contact manager removes (`0x003029d0`, the manager's `vt[+0x08]`).
    Points,
    /// An unmasked float exception, an x87 stack not empty at the entry,
    /// or a non-finite float the x87 code would load.
    Float,
    /// The monitor stream is on.
    Monitor,
    /// The guest answered an audited call whose return could not be
    /// observed, so it went uncompared.
    Unobserved,
}

impl Fallback {
    pub const ALL: [Fallback; 6] = [Self::Memory, Self::Closest, Self::Points, Self::Float, Self::Monitor, Self::Unobserved];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Closest => "closest",
            Self::Points => "points",
            Self::Float => "float",
            Self::Monitor => "monitor",
            Self::Unobserved => "unobserved",
        }
    }
}

static FALLBACKS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];

/// Count a call sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The process call's declines since start, by reason, in [`Fallback::ALL`] order.
pub fn fallbacks() -> [(&'static str, u64); 6] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

/// A float the x87 code loads: finite, or the call declines.
fn x87_word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    let v = word(j, at)?;
    if finite(v) { Ok(v) } else { Err(Fallback::Float) }
}

fn vec(j: &Journal, at: u32) -> Result<u128, Fallback> {
    vector(j, at).ok_or(Fallback::Memory)
}

/// The whole call when the pair stays separated: the bound's store, and
/// the manifold's point count found zero.
pub fn process(j: &mut Journal, a: &Args, fpu: Fpu) -> Result<Separated, Fallback> {
    if !fpu.masked() {
        return Err(Fallback::Float);
    }
    let x = Fx { mode: Mode::from_cw(fpu.control) };
    let at = |off: u32| a.agent.wrapping_add(off);
    // 0x002eeb7c: both shapes are read before the bound test.
    word(j, a.b.wrapping_add(COLLIDABLE_SHAPE))?;
    word(j, a.a.wrapping_add(COLLIDABLE_SHAPE))?;
    let zero = x87_word(j, ZERO)?;
    let bound = x87_word(j, at(field::BOUND))?;
    // `test ah,0x41; jne`: only an ordered, strictly positive bound.
    if !above(load(bound), load(zero)) {
        return Err(Fallback::Closest);
    }
    // 0x002eebc6: the motions' displacement along the axis, packed.
    let motion_a = word(j, a.a.wrapping_add(COLLIDABLE_MOTION))?;
    let motion_b = word(j, a.b.wrapping_add(COLLIDABLE_MOTION))?;
    let allowance_b = x87_word(j, motion_b.wrapping_add(MOTION_ALLOWANCE))?;
    let allowance_a = x87_word(j, motion_a.wrapping_add(MOTION_ALLOWANCE))?;
    let mut sse = Sse { e: Exact::new(fpu.mxcsr) };
    let moved_a = sse.sub(vec(j, motion_a.wrapping_add(MOTION_TO))?, vec(j, motion_a.wrapping_add(MOTION_FROM))?);
    let moved_b = sse.sub(vec(j, motion_b.wrapping_add(MOTION_TO))?, vec(j, motion_b.wrapping_add(MOTION_FROM))?);
    let relative = sse.sub(moved_b, moved_a);
    let along = sse.mul(relative, vec(j, at(field::AXIS))?);
    let t = sse.hsum(along);
    if sse.e.trapped {
        return Err(Fallback::Float);
    }
    if !finite(t) {
        return Err(Fallback::Float);
    }
    // 0x002eec14: bound - ((allowance B + allowance A) + t), rounded at the store.
    let reach = x.add(x.add(load(allowance_b), load(allowance_a)), load(t));
    let bound = x87_word(j, at(field::BOUND))?;
    let next = x.sub(load(bound), reach);
    let stored = x.st(next);
    j.put(at(field::BOUND), stored).ok_or(Fallback::Memory)?;
    // 0x002eec1e: the register value, not the stored one, is compared.
    if !above(next, load(zero)) {
        return Err(Fallback::Closest);
    }
    // 0x002eee44: the removal call finds no points and returns.
    let manager = word(j, at(field::MANAGER))?;
    if word(j, at(field::POINTS))? != 0 {
        return Err(Fallback::Points);
    }
    Ok(Separated { bound: stored, manager })
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

    const AGENT: u32 = 0x1000;
    const A: u32 = 0x2000;
    const B: u32 = 0x2100;
    const MOTION_A: u32 = 0x3000;
    const MOTION_B: u32 = 0x3100;
    const ARGS: Args = Args { agent: AGENT, a: A, b: B };
    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    fn f(v: f32) -> u32 {
        v.to_bits()
    }

    fn put4(h: &mut Heap, at: u32, v: [f32; 4]) {
        for (i, x) in v.into_iter().enumerate() {
            h.0.insert(at + 4 * i as u32, f(x));
        }
    }

    /// A moves by +1 in x, B stays; the axis is +x; allowances 0.25 and 0.5.
    fn heap(bound: f32) -> Heap {
        let mut h = Heap::default();
        h.0.insert(ZERO, 0);
        h.0.insert(A + COLLIDABLE_MOTION, MOTION_A);
        h.0.insert(B + COLLIDABLE_MOTION, MOTION_B);
        h.0.insert(AGENT + field::MANAGER, 0x9000);
        h.0.insert(AGENT + field::BOUND, f(bound));
        put4(&mut h, AGENT + field::AXIS, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, MOTION_A, [0.0, 0.0, 0.0, 0.25]);
        put4(&mut h, MOTION_A + MOTION_TO, [1.0, 0.0, 0.0, 0.0]);
        put4(&mut h, MOTION_B, [5.0, 0.0, 0.0, 0.5]);
        put4(&mut h, MOTION_B + MOTION_TO, [5.0, 0.0, 0.0, 0.0]);
        h
    }

    #[test]
    fn a_separated_pair_shrinks_its_bound_by_the_reach() {
        // Reach: (0.5 + 0.25) + (0 - 1) = -0.25, so the bound grows.
        let h = heap(2.0);
        let mut j = Journal::new(&h);
        assert_eq!(process(&mut j, &ARGS, FPU), Ok(Separated { bound: f(2.25), manager: 0x9000 }));
        assert_eq!(j.finish().into_iter().collect::<Vec<_>>(), vec![(AGENT + field::BOUND, f(2.25))]);
        // B closing on A: reach (0.5 + 0.25) + 1.5 = 2.25.
        let mut h = heap(2.5);
        put4(&mut h, MOTION_B + MOTION_TO, [6.5, 0.0, 0.0, 0.0]);
        put4(&mut h, MOTION_A + MOTION_TO, [0.0, 0.0, 0.0, 0.0]);
        let mut j = Journal::new(&h);
        assert_eq!(process(&mut j, &ARGS, FPU).map(|s| s.bound), Ok(f(0.25)));
    }

    #[test]
    fn a_bound_that_reaches_zero_points_or_unmasked_floats_decline() {
        for bound in [0.0, -1.0] {
            assert_eq!(process(&mut Journal::new(&heap(bound)), &ARGS, FPU), Err(Fallback::Closest), "{bound}");
        }
        assert_eq!(process(&mut Journal::new(&heap(f32::NAN)), &ARGS, FPU), Err(Fallback::Float));
        // B closes by 1: reach (0.5 + 0.25) + 1 = 1.75, past a bound of 0.5.
        let mut h = heap(0.5);
        put4(&mut h, MOTION_B + MOTION_TO, [6.0, 0.0, 0.0, 0.0]);
        put4(&mut h, MOTION_A + MOTION_TO, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(process(&mut Journal::new(&h), &ARGS, FPU), Err(Fallback::Closest));
        let mut h = heap(2.0);
        h.0.insert(AGENT + field::POINTS, 1);
        assert_eq!(process(&mut Journal::new(&h), &ARGS, FPU), Err(Fallback::Points));
        let unmasked = Fpu { mxcsr: 0x1f00, ..FPU };
        assert_eq!(process(&mut Journal::new(&heap(2.0)), &ARGS, unmasked), Err(Fallback::Float));
        let mut h = heap(2.0);
        h.0.insert(A + COLLIDABLE_MOTION, MOTION_A + 4);
        assert_eq!(process(&mut Journal::new(&h), &ARGS, FPU), Err(Fallback::Memory));
    }

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        let names = fallbacks().map(|(n, _)| n);
        assert_eq!(names, ["memory", "closest", "points", "float", "monitor", "unobserved"]);
        let before = fallbacks()[1].1;
        count(Fallback::Closest);
        assert!(fallbacks()[1].1 > before);
    }

    #[test]
    fn lanes_past_z_do_not_enter_the_projection() {
        let mut h = heap(2.0);
        put4(&mut h, AGENT + field::AXIS, [1.0, 0.0, 0.0, 7.0]);
        put4(&mut h, MOTION_B + MOTION_TO, [5.0, 0.0, 0.0, 1e30]);
        assert_eq!(process(&mut Journal::new(&h), &ARGS, FPU).map(|s| s.bound), Ok(f(2.25)));
    }
}
