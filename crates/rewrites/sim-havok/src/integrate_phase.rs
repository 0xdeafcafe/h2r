//! The integrate phase `0x0030fef0` (docs/engine/havok-integrate.md,
//! "The integrate phase"): thiscall on the world, one stack argument (the
//! step `{dt, 1/dt}`), `ret 4`.
//!
//! [`phase`] runs the entry on a [`Journal`] when every call it would make
//! is one the model covers: the step terms, then each active island last
//! to first, the deactivation test ([`deactivation::test`], when `W+0x23d`
//! is set) and the island integrate ([`island_integrate::integrate`]),
//! then the split pass over islands with nothing to split. Pending
//! operations, a deactivating island, listeners and a split decline before
//! anything is stored ([`Fallback`]).
use crate::deactivation;
use crate::island_integrate;
use crate::jacobian::{Fpu, Fx};
use crate::mem::Journal;
use crate::solver::{finite, load};
use std::sync::atomic::{AtomicU64, Ordering};
use x87::{F80, Mode};

/// The world's words the entry reads.
pub const WORLD_ISLANDS: u32 = 0x08;
pub const WORLD_ISLAND_COUNT: u32 = 0x0c;
/// Pending operations (`0x0030f6b0` while positive).
pub const WORLD_PENDING: u32 = 0x24;
/// Per-island listeners (`0x00318830` while non-zero).
pub const WORLD_ISLAND_LISTENERS: u32 = 0xa0;
/// Step listeners (`0x00318700`, `vt[+0x04]` each) and the world's
/// post-step listener (`0x0030c180`).
pub const WORLD_STEP_LISTENERS: u32 = 0x88;
pub const WORLD_STEP_LISTENER: u32 = 0xc0;
/// Islands to merge (`0x0030fc30`), the queue `0x0030cda0` empties,
/// islands to deactivate first (`0x0030ce20`), the entities to release
/// (`0x0030d400` each).
pub const WORLD_MERGES: u32 = 0xf4;
pub const WORLD_QUEUE: u32 = 0x100;
pub const WORLD_DEACTIVATE: u32 = 0x10c;
pub const WORLD_RELEASES: u32 = 0x118;
/// The lock byte the phase holds over its island loop.
pub const WORLD_LOCK: u32 = 0x12c;
/// The step input the integrate takes.
pub const WORLD_INPUT: u32 = 0x140;
/// The step copy and its derived terms.
pub const WORLD_STEP: u32 = 0x150;
pub const WORLD_SCALED_DT: u32 = 0x170;
pub const WORLD_SCALED_RATE: u32 = 0x174;
pub const WORLD_ITERATIONS: u32 = 0x17c;
pub const WORLD_SCALE: u32 = 0x180;
pub const WORLD_SCALED_DT_COPY: u32 = 0x198;
pub const WORLD_SCALED_RATE_COPY: u32 = 0x19c;
pub const WORLD_ITERATION_DT: u32 = 0x1a0;
pub const WORLD_RATE_TERM: u32 = 0x1a4;
/// The split pass's flag and the deactivation test's.
pub const WORLD_SPLIT: u32 = 0x23c;
pub const WORLD_DEACTIVATION: u32 = 0x23d;
/// The island's words the split pass reads, and the byte the phase clears.
pub const ISLAND_SPLIT: u32 = 0x28;
pub const ISLAND_INTEGRATED: u32 = 0x30;

/// More islands than this decline before the loop.
pub const MAX_ISLANDS: u32 = 1 << 16;

/// Why the phase runs the guest. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the model needs is unmapped or unaligned, or a store the
    /// answer needs is not plain writable memory.
    Memory,
    /// Pending operations, islands to deactivate first, merges, the queue
    /// or entities to release: guest calls the model does not cover.
    Pending,
    /// An island's deactivation test says it deactivates (`0x0030cc60`).
    Deactivate,
    /// A listener array or the post-step listener is not empty.
    Listener,
    /// The split pass would remove an empty island or split one.
    Split,
    /// The deactivation test or the island integrate declined an island;
    /// the guest's own call to it counts the reason in its arm.
    Island,
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
    pub const ALL: [Fallback; 9] = [
        Self::Memory,
        Self::Pending,
        Self::Deactivate,
        Self::Listener,
        Self::Split,
        Self::Island,
        Self::Float,
        Self::Monitor,
        Self::Unobserved,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Pending => "pending",
            Self::Deactivate => "deactivate",
            Self::Listener => "listener",
            Self::Split => "split",
            Self::Island => "island",
            Self::Float => "float",
            Self::Monitor => "monitor",
            Self::Unobserved => "unobserved",
        }
    }
}

static FALLBACKS: [AtomicU64; 9] = [const { AtomicU64::new(0) }; 9];

/// Count a call sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The phase's declines since start, by reason, in [`Fallback::ALL`] order.
pub fn fallbacks() -> [(&'static str, u64); 9] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

/// What the phase computed besides its journal.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Outcome {
    /// Islands integrated.
    pub islands: u32,
    /// Of those, islands that went through the solve.
    pub solved: u32,
}

fn word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    j.get(at).ok_or(Fallback::Memory)
}

fn byte(j: &Journal, at: u32) -> Result<u8, Fallback> {
    j.byte(at).ok_or(Fallback::Memory)
}

fn put(j: &mut Journal, at: u32, v: u32) -> Result<(), Fallback> {
    j.put(at, v).ok_or(Fallback::Memory)
}

fn put_byte(j: &mut Journal, at: u32, v: u8) -> Result<(), Fallback> {
    deactivation::put_byte(j, at, v).ok_or(Fallback::Memory)
}

fn x87_word(j: &Journal, at: u32) -> Result<u32, Fallback> {
    let v = word(j, at)?;
    if finite(v) { Ok(v) } else { Err(Fallback::Float) }
}

fn fild(j: &Journal, at: u32) -> Result<F80, Fallback> {
    Ok(F80::from_i64(i64::from(word(j, at)? as i32)))
}

/// The whole entry for `world` and the step at `step`.
pub fn phase(j: &mut Journal, world: u32, step: u32, fpu: Fpu) -> Result<Outcome, Fallback> {
    use Fallback::*;
    if !fpu.masked() {
        return Err(Float);
    }
    let x = Fx { mode: Mode::from_cw(fpu.control) };
    let w = |off: u32| world.wrapping_add(off);
    // 0x0030ff22: the step copy and its terms, in retail's order.
    let dt = word(j, step)?;
    put(j, w(WORLD_STEP), dt)?;
    let rate = word(j, step.wrapping_add(4))?;
    put(j, w(WORLD_STEP + 4), rate)?;
    let scale = x87_word(j, w(WORLD_SCALE))?;
    let dt = x87_word(j, step)?;
    put(j, w(WORLD_SCALED_DT), x.st(x.mul(load(scale), load(dt))))?;
    let scaled_dt = word(j, w(WORLD_SCALED_DT))?;
    let iterations = fild(j, w(WORLD_ITERATIONS))?;
    let rate = x87_word(j, step.wrapping_add(4))?;
    put(j, w(WORLD_SCALED_RATE), x.st(x.mul(iterations, load(rate))))?;
    put(j, w(WORLD_SCALED_DT_COPY), scaled_dt)?;
    let scaled_rate = word(j, w(WORLD_SCALED_RATE))?;
    put(j, w(WORLD_SCALED_RATE_COPY), scaled_rate)?;
    let iterations = fild(j, w(WORLD_ITERATIONS))?;
    let scaled_dt = x87_word(j, w(WORLD_SCALED_DT))?;
    put(j, w(WORLD_ITERATION_DT), x.st(x.mul(iterations, load(scaled_dt))))?;
    let scale = x87_word(j, w(WORLD_SCALE))?;
    let scaled_rate = x87_word(j, w(WORLD_SCALED_RATE))?;
    put(j, w(WORLD_RATE_TERM), x.st(x.mul(load(scale), load(scaled_rate))))?;
    // 0x0030ff90: `0x00175f40` is a bare `ret`.
    if word(j, w(WORLD_PENDING))? as i32 > 0 || word(j, w(WORLD_DEACTIVATE))? != 0 {
        return Err(Pending);
    }
    put(j, w(WORLD_DEACTIVATE), 0)?;
    put_byte(j, w(WORLD_LOCK), 1)?;
    // 0x0030ffb5: the count is read once; the array and the flag again
    // for each island, last to first.
    let count = word(j, w(WORLD_ISLAND_COUNT))? as i32;
    if count > MAX_ISLANDS as i32 {
        return Err(Memory);
    }
    let mut out = Outcome::default();
    for i in (0..count.max(0) as u32).rev() {
        let test = byte(j, w(WORLD_DEACTIVATION))? != 0;
        let island = word(j, word(j, w(WORLD_ISLANDS))?.wrapping_add(4 * i))?;
        // The out byte lands in the phase's own argument slot, dead at the
        // return: not stored.
        if test && deactivation::test(j, island, None, step, fpu).map_err(|_| Island)? {
            return Err(Deactivate);
        }
        let integrated = island_integrate::integrate(j, island, w(WORLD_INPUT), fpu).map_err(|_| Island)?;
        out.islands += 1;
        out.solved += u32::from(integrated.solved);
        put_byte(j, island.wrapping_add(ISLAND_INTEGRATED), 0)?;
        if word(j, w(WORLD_ISLAND_LISTENERS))? != 0 {
            return Err(Listener);
        }
    }
    // 0x00310080: `0x0030e720` leaves an island alone when it has entities
    // and its split byte is clear; anything else declines.
    if byte(j, w(WORLD_SPLIT))? != 0 {
        let count = word(j, w(WORLD_ISLAND_COUNT))? as i32;
        if count > MAX_ISLANDS as i32 {
            return Err(Memory);
        }
        for i in (0..count.max(0) as u32).rev() {
            let island = word(j, word(j, w(WORLD_ISLANDS))?.wrapping_add(4 * i))?;
            if word(j, island.wrapping_add(deactivation::ISLAND_ENTITY_COUNT))? == 0 || byte(j, island.wrapping_add(ISLAND_SPLIT))? != 0 {
                return Err(Split);
            }
        }
    }
    put_byte(j, w(WORLD_LOCK), 0)?;
    if word(j, w(WORLD_MERGES))? != 0 || word(j, w(WORLD_QUEUE))? != 0 || word(j, w(WORLD_RELEASES))? as i32 > 0 {
        return Err(Pending);
    }
    put(j, w(WORLD_RELEASES), 0)?;
    // 0x00318700: no step listener and no post-step listener.
    if word(j, w(WORLD_STEP_LISTENERS))? != 0 || word(j, w(WORLD_STEP_LISTENER))? != 0 {
        return Err(Listener);
    }
    Ok(out)
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

    const WORLD: u32 = 0x1_0000;
    const STEP: u32 = 0x8000;
    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    fn world() -> Heap {
        let mut h = Heap::default();
        h.0.insert(STEP, (1.0f32 / 30.0).to_bits());
        h.0.insert(STEP + 4, 30.0f32.to_bits());
        h.0.insert(WORLD + WORLD_SCALE, 1.0f32.to_bits());
        h.0.insert(WORLD + WORLD_ITERATIONS, 4);
        h
    }

    #[test]
    fn a_world_with_no_island_stores_the_step_terms_and_clears_its_flags() {
        let h = world();
        let mut j = Journal::new(&h);
        assert_eq!(phase(&mut j, WORLD, STEP, FPU), Ok(Outcome::default()));
        assert_eq!(j.get(WORLD + WORLD_STEP), Some((1.0f32 / 30.0).to_bits()));
        assert_eq!(j.get(WORLD + WORLD_SCALED_RATE), Some(120.0f32.to_bits()));
        assert_eq!(j.get(WORLD + WORLD_SCALED_RATE_COPY), Some(120.0f32.to_bits()));
        assert_eq!(j.get(WORLD + WORLD_RATE_TERM), Some(120.0f32.to_bits()));
        assert_eq!(j.byte(WORLD + WORLD_LOCK), Some(0));
    }

    #[test]
    fn pending_work_listeners_and_splits_decline() {
        for (at, v, reason) in [
            (WORLD_PENDING, 1, Fallback::Pending),
            (WORLD_DEACTIVATE, 1, Fallback::Pending),
            (WORLD_QUEUE, 1, Fallback::Pending),
            (WORLD_RELEASES, 1, Fallback::Pending),
            (WORLD_STEP_LISTENERS, 1, Fallback::Listener),
            (WORLD_STEP_LISTENER, 0x40, Fallback::Listener),
            (WORLD_ISLAND_COUNT, MAX_ISLANDS + 1, Fallback::Memory),
        ] {
            let mut h = world();
            h.0.insert(WORLD + at, v);
            assert_eq!(phase(&mut Journal::new(&h), WORLD, STEP, FPU), Err(reason), "{at:#x}");
        }
        // One island with no entity, under the split flag.
        let mut h = world();
        h.0.insert(WORLD + WORLD_ISLAND_COUNT, 1);
        h.0.insert(WORLD + WORLD_ISLANDS, 0x2000);
        h.0.insert(0x2000, 0x3000);
        h.0.insert(WORLD + (WORLD_SPLIT & !3), 1 << (8 * (WORLD_SPLIT & 3)));
        assert_eq!(phase(&mut Journal::new(&h), WORLD, STEP, FPU), Err(Fallback::Split));
        h.0.insert(WORLD + (WORLD_SPLIT & !3), 0);
        assert_eq!(phase(&mut Journal::new(&h), WORLD, STEP, FPU), Ok(Outcome { islands: 1, solved: 0 }));
    }

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        let names = fallbacks().map(|(n, _)| n);
        assert_eq!(names, ["memory", "pending", "deactivate", "listener", "split", "island", "float", "monitor", "unobserved"]);
        let before = fallbacks()[4].1;
        count(Fallback::Split);
        assert!(fallbacks()[4].1 > before);
    }
}
