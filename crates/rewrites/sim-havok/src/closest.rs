//! The closest-feature query `0x003048a0` as the game calls it
//! (docs/engine/havok-convex.md, "The query arm"): [`query`] runs
//! [`gjk::query`] with every call it makes out answered in Rust. Each
//! shape's `vt[+0x30]` and `vt[+0x34]` is picked by the function pointer
//! in its vtable and answered by its port in [`crate::shape_support`];
//! the triangle case runs [`gjk::triangle_case`], its penetration call
//! [`crate::penetration::Port`]. A virtual with no port declines
//! ([`Fallback::Shape`]).
//!
//! The support virtuals' x87 work runs on a fresh x87 state under the
//! entry's control word, and each one's MXCSR starts from the entry's:
//! the query reads no register, flag or x87 state a call leaves, and the
//! sticky exception flags are not modelled (every exception is masked, or
//! the call declines). On a [`Fallback`] the journal is discarded and
//! nothing has been written.
use crate::convex;
use crate::gjk::{self, Args, Query};
use crate::jacobian::Fpu;
use crate::mem::Journal;
use crate::penetration;
use crate::shape_support::{self as shape, Exit};
use std::sync::atomic::{AtomicU64, Ordering};
use x87::abi::{self, X87Abi};

/// A shape's support virtual and vertex virtual, by vtable offset.
pub const SUPPORT: u32 = 0x30;
pub const VERTICES: u32 = 0x34;

/// A biped wrapper holding a biped wrapper this deep declines (`shape`):
/// the game's wrappers hold a pill.
const DEPTH: u32 = 4;

/// Why a query runs the guest. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the query reads is unmapped or unaligned, a vector is not
    /// 16-byte aligned, a store is not plain writable memory, or an array
    /// would pass the port's bounds.
    Memory,
    /// A shape's `vt[+0x30]` or `vt[+0x34]` has no port.
    Shape,
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
    pub const ALL: [Fallback; 5] = [Self::Memory, Self::Shape, Self::Float, Self::Monitor, Self::Unobserved];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Shape => "shape",
            Self::Float => "float",
            Self::Monitor => "monitor",
            Self::Unobserved => "unobserved",
        }
    }

    /// A leaf's decline. The leaves return `closest`, `points` and
    /// `unobserved` nowhere; they map to `memory` should one appear.
    fn of(f: convex::Fallback) -> Self {
        match f {
            convex::Fallback::Float => Self::Float,
            convex::Fallback::Monitor => Self::Monitor,
            convex::Fallback::Memory | convex::Fallback::Closest | convex::Fallback::Points | convex::Fallback::Unobserved => Self::Memory,
        }
    }
}

static FALLBACKS: [AtomicU64; 5] = [const { AtomicU64::new(0) }; 5];

/// Count a call sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The query's declines since start, by reason, in [`Fallback::ALL`] order.
pub fn fallbacks() -> [(&'static str, u64); 5] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

/// The shapes' virtuals and the triangle case, in Rust.
pub struct Ported {
    pub fpu: Fpu,
    /// A virtual with no port was met.
    pub unported: bool,
}

impl Ported {
    pub fn new(fpu: Fpu) -> Self {
        Self { fpu, unported: false }
    }

    fn x87(&self) -> X87Abi {
        let mut x = X87Abi::default();
        abi::init(&mut x);
        x.control = self.fpu.control;
        x
    }
}

fn virtual_of(j: &Journal, shape: u32, slot: u32) -> Result<u32, convex::Fallback> {
    let vt = j.get(shape).ok_or(convex::Fallback::Memory)?;
    j.get(vt.wrapping_add(slot)).ok_or(convex::Fallback::Memory)
}

/// `shape`'s `vt[+0x30]` along `dir` into `out`, by its port.
#[allow(clippy::too_many_arguments)]
fn support(
    j: &mut Journal,
    shape: u32,
    dir: u32,
    out: u32,
    esp: u32,
    m: &mut u32,
    x: &mut X87Abi,
    unported: &mut bool,
    depth: u32,
) -> Result<Exit, convex::Fallback> {
    match virtual_of(j, shape, SUPPORT)? {
        shape::CAPSULE_SUPPORT => shape::capsule(j, shape, dir, out, m, x).map(|eax| Exit { eax, ecx: out, edx: 0 }),
        shape::POLYGON_SUPPORT => shape::polygon(j, shape, dir, out, m),
        shape::BIPED_SUPPORT if depth < DEPTH => shape::biped(j, shape, dir, out, esp, m, x, |j, child, esp, m, x| {
            support(j, child, dir, out, esp, m, x, unported, depth + 1)
        }),
        _ => {
            *unported = true;
            Err(convex::Fallback::Memory)
        }
    }
}

/// `shape`'s `vt[+0x34]`: the vertices the IDs name into `out`.
#[allow(clippy::too_many_arguments)]
fn vertices(
    j: &mut Journal,
    shape: u32,
    ids: u32,
    count: u32,
    out: u32,
    esp: u32,
    m: &mut u32,
    x: &mut X87Abi,
    unported: &mut bool,
    depth: u32,
) -> Result<(), convex::Fallback> {
    match virtual_of(j, shape, VERTICES)? {
        shape::CAPSULE_VERTICES => shape::capsule_vertices(j, shape, ids, count, out),
        shape::POLYGON_VERTICES => shape::polygon_vertices(j, shape, ids, count, out),
        shape::BIPED_VERTICES if depth < DEPTH => shape::biped_vertices(j, shape, ids, count, out, esp, m, x, |j, child, esp, m, x| {
            vertices(j, child, ids, count, out, esp, m, x, unported, depth + 1)
        }),
        _ => {
            *unported = true;
            Err(convex::Fallback::Memory)
        }
    }
}

impl gjk::Calls for Ported {
    fn vertices(&mut self, j: &mut Journal, shape: u32, ids: u32, count: u32, out: u32, esp: u32) -> Result<(), convex::Fallback> {
        let (mut m, mut x) = (self.fpu.mxcsr, self.x87());
        vertices(j, shape, ids, count, out, esp, &mut m, &mut x, &mut self.unported, 0)
    }

    fn support(&mut self, j: &mut Journal, shape: u32, dir: u32, out: u32, esp: u32) -> Result<(), convex::Fallback> {
        let (mut m, mut x) = (self.fpu.mxcsr, self.x87());
        support(j, shape, dir, out, esp, &mut m, &mut x, &mut self.unported, 0).map(|_| ())
    }

    fn triangle_case(&mut self, j: &mut Journal, st: u32, args: Args, esp: u32) -> Result<u32, convex::Fallback> {
        let fpu = self.fpu;
        gjk::triangle_case(j, st, esp, args, fpu, &mut penetration::Port { fpu, calls: self })
    }
}

impl penetration::Calls for Ported {
    fn support(&mut self, j: &mut Journal, shape: u32, dir: u32, out: u32, esp: u32) -> Result<(), convex::Fallback> {
        gjk::Calls::support(self, j, shape, dir, out, esp)
    }
}

/// `0x003048a0`, thiscall on the state `st` with `args`; `esp` is the
/// stack pointer at entry. Every call out in Rust, or a decline by name.
pub fn query(j: &mut Journal, st: u32, esp: u32, args: Args, fpu: Fpu) -> Result<Query, Fallback> {
    let mut calls = Ported::new(fpu);
    gjk::query(j, st, esp, args, fpu, &mut calls).map_err(|f| if calls.unported { Fallback::Shape } else { Fallback::of(f) })
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
    const VTABLE: u32 = 0x2000;
    const OUT: u32 = 0x3000;
    const DIR: u32 = 0x3100;
    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    #[test]
    fn virtuals_are_picked_by_their_function_and_an_unknown_one_is_named() {
        let mut h = Heap::default();
        h.0.insert(SHAPE, VTABLE);
        h.0.insert(VTABLE + SUPPORT, shape::CAPSULE_SUPPORT);
        h.0.insert(SHAPE + 0x28, 1.0f32.to_bits());
        let mut calls = Ported::new(FPU);
        let mut j = Journal::new(&h);
        gjk::Calls::support(&mut calls, &mut j, SHAPE, DIR, OUT, 0x7f00_0000).unwrap();
        assert_eq!(j.get(OUT + 0xc), Some(0x3f00_0010), "end B, ID 0x10");
        assert!(!calls.unported);
        h.0.insert(VTABLE + SUPPORT, 0x0012_3456);
        let mut j = Journal::new(&h);
        assert!(gjk::Calls::support(&mut calls, &mut j, SHAPE, DIR, OUT, 0x7f00_0000).is_err());
        assert!(calls.unported);
    }

    #[test]
    fn a_wrapper_that_holds_itself_declines() {
        let mut h = Heap::default();
        h.0.insert(SHAPE, VTABLE);
        h.0.insert(VTABLE + SUPPORT, shape::BIPED_SUPPORT);
        h.0.insert(VTABLE + VERTICES, shape::BIPED_VERTICES);
        h.0.insert(SHAPE + shape::biped_field::CHILD, SHAPE);
        let mut calls = Ported::new(FPU);
        assert!(gjk::Calls::support(&mut calls, &mut Journal::new(&h), SHAPE, DIR, OUT, 0x7f00_0000).is_err());
        assert!(calls.unported);
        let mut calls = Ported::new(FPU);
        assert!(gjk::Calls::vertices(&mut calls, &mut Journal::new(&h), SHAPE, DIR, 1, OUT, 0x7f00_0000).is_err());
        assert!(calls.unported);
    }

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        let before = fallbacks();
        count(Fallback::Shape);
        let after = fallbacks();
        assert_eq!(after.map(|f| f.0), ["memory", "shape", "float", "monitor", "unobserved"]);
        assert!(after[1].1 > before[1].1);
        assert_eq!(Fallback::of(convex::Fallback::Monitor), Fallback::Monitor);
    }
}
