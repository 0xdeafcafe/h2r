//! The island integrate `0x00316a20` (docs/engine/havok-step.md, "The
//! island integrate"): thiscall on the island, one stack argument (the
//! world's step input `W+0x140`), `ret 4`.
//!
//! [`integrate`] runs the entry on a [`Journal`]: the island's actions
//! (none exist in the image, so any declines), then either the island
//! solve [`island_solve::solve`] with the nine arguments the entry pushes,
//! or, with no constraint, each entity's unconstrained virtuals in reverse
//! array order, `vt[+0x08]` then `vt[+0x04]` ([`motion_solve::forces`],
//! [`motion_solve::pose`]). On a [`Fallback`] the journal is discarded and
//! nothing has been written.
use crate::island_solve::{self, Args};
use crate::jacobian::Fpu;
use crate::mem::Journal;
use crate::motion_solve::{self, FreeClass};
use std::sync::atomic::{AtomicU64, Ordering};

/// The island's words the entry reads.
pub const ISLAND_WORLD: u32 = 0x20;
pub const ISLAND_ENTITIES: u32 = 0x3c;
pub const ISLAND_ENTITY_COUNT: u32 = 0x40;
pub const ISLAND_ACTIONS: u32 = 0x4c;
pub const ISLAND_ACTION_COUNT: u32 = 0x50;
pub const ISLAND_CONSTRAINTS: u32 = 0x58;
pub const ISLAND_CONSTRAINT_COUNT: u32 = 0x5c;
/// The world's gravity vector.
pub const WORLD_GRAVITY: u32 = 0xe0;
/// The step input's parts: `{dt, 1/dt}`, the solver's parameters, the
/// pair words (`W+0x150`, `W+0x160`, `W+0x190` for the input `W+0x140`).
pub const INPUT_STEP: u32 = 0x10;
pub const INPUT_INFO: u32 = 0x20;
pub const INPUT_PAIRS: u32 = 0x50;
/// The unconstrained virtuals: forces (step, gravity; `ret 8`), then the
/// pose (step; `ret 4`).
pub const MOTION_FORCES_SLOT: u32 = 0x08;
pub const MOTION_POSE_SLOT: u32 = 0x04;

/// More entities than this decline before the loop (an island holds a
/// few; retail's count is signed, so a corrupt one would loop on).
pub const MAX_ENTITIES: u32 = 1 << 16;

/// Why an island's integrate runs the guest. Each is counted
/// ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the model needs is unmapped or unaligned, or a store the
    /// answer needs is not plain writable memory.
    Memory,
    /// The island holds actions, whose `vt[+0x14]` is guest code. The
    /// image builds none (havok-step.md, "Actions").
    Action,
    /// An entity of an island with no constraint whose `vt[+0x08]` and
    /// `vt[+0x04]` are not one of the four [`FreeClass`] pairs.
    Motion,
    /// An unmasked float exception, an x87 stack not empty at the entry,
    /// or a non-finite float the virtuals' x87 code would load.
    Float,
    /// The island solve declined the island; `havok.islands` counts its
    /// reason with the solve's.
    Solve,
    /// The monitor stream is on, so the entry would append timer records
    /// (sim-rewrite-havok.md §3.3).
    Monitor,
    /// The guest answered an audited call whose return could not be
    /// observed, so it went uncompared.
    Unobserved,
}

impl Fallback {
    pub const ALL: [Fallback; 7] =
        [Self::Memory, Self::Action, Self::Motion, Self::Float, Self::Solve, Self::Monitor, Self::Unobserved];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Action => "action",
            Self::Motion => "motion",
            Self::Float => "float",
            Self::Solve => "solve",
            Self::Monitor => "monitor",
            Self::Unobserved => "unobserved",
        }
    }
}

static FALLBACKS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];

/// Count a call sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// The integrate's declines since start, by reason, in [`Fallback::ALL`]
/// order.
pub fn fallbacks() -> [(&'static str, u64); 7] {
    Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed)))
}

/// What the entry computed besides its journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The island went through the solve (`I+0x5c != 0`).
    pub solved: bool,
}

/// The whole entry for `island` with the step input `input`.
pub fn integrate(j: &mut Journal, island: u32, input: u32, fpu: Fpu) -> Result<Outcome, Fallback> {
    use Fallback::*;
    let word = |j: &Journal, off: u32| j.get(island.wrapping_add(off)).ok_or(Memory);
    // 0x00316a27: the action loop runs while its cursor differs from the
    // end the count gives; none exist, so any action is a decline.
    if word(j, ISLAND_ACTION_COUNT)? != 0 {
        return Err(Action);
    }
    if !fpu.masked() {
        return Err(Float);
    }
    let constraint_count = word(j, ISLAND_CONSTRAINT_COUNT)?;
    let step = input.wrapping_add(INPUT_STEP);
    if constraint_count != 0 {
        // 0x00316ae9: the nine arguments, as the entry pushes them.
        let world = word(j, ISLAND_WORLD)?;
        let args = Args {
            step,
            info: input.wrapping_add(INPUT_INFO),
            pairs: input.wrapping_add(INPUT_PAIRS),
            gravity: world.wrapping_add(WORLD_GRAVITY),
            island,
            entities: word(j, ISLAND_ENTITIES)?,
            entity_count: word(j, ISLAND_ENTITY_COUNT)?,
            constraints: word(j, ISLAND_CONSTRAINTS)?,
            constraint_count,
        };
        island_solve::solve(j, &args, fpu).map_err(|_| Solve)?;
        return Ok(Outcome { solved: true });
    }
    // 0x00316a84: the count is read once; the array, the motion and the
    // world are read again for each entity, last to first.
    let count = word(j, ISLAND_ENTITY_COUNT)?;
    if count > MAX_ENTITIES {
        return Err(Memory);
    }
    for i in (0..count).rev() {
        let entities = word(j, ISLAND_ENTITIES)?;
        let entity = j.get(entities.wrapping_add(4 * i)).ok_or(Memory)?;
        let motion = j.get(entity.wrapping_add(island_solve::ENTITY_MOTION)).ok_or(Memory)?;
        let gravity = word(j, ISLAND_WORLD)?.wrapping_add(WORLD_GRAVITY);
        let vt = j.get(motion).ok_or(Memory)?;
        let forces = j.get(vt.wrapping_add(MOTION_FORCES_SLOT)).ok_or(Memory)?;
        let pose = j.get(vt.wrapping_add(MOTION_POSE_SLOT)).ok_or(Memory)?;
        let class = FreeClass::of(forces, pose).ok_or(Motion)?;
        motion_solve::forces(j, class, motion, step, gravity, fpu).map_err(float)?;
        motion_solve::pose(j, class, motion, step, fpu).map_err(float)?;
    }
    Ok(Outcome { solved: false })
}

fn float(f: island_solve::Fallback) -> Fallback {
    match f {
        island_solve::Fallback::Float => Fallback::Float,
        _ => Fallback::Memory,
    }
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
            self.0.get(&addr).copied()
        }
    }

    const ISLAND: u32 = 0x2000;
    const ENTITIES: u32 = 0x3000;
    const INPUT: u32 = 0x8000;
    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    /// An island of `n` fixed motions, no actions, no constraints.
    fn heap(n: u32) -> Heap {
        let mut h = Heap::default();
        let (forces, pose) = FreeClass::Fixed.slots();
        let vt = 0x5000;
        h.0.insert(vt + MOTION_FORCES_SLOT, forces);
        h.0.insert(vt + MOTION_POSE_SLOT, pose);
        for (off, v) in [(ISLAND_WORLD, 0x9000), (ISLAND_ENTITIES, ENTITIES), (ISLAND_ENTITY_COUNT, n), (ISLAND_ACTIONS, 0), (ISLAND_ACTION_COUNT, 0), (ISLAND_CONSTRAINTS, 0), (ISLAND_CONSTRAINT_COUNT, 0)] {
            h.0.insert(ISLAND + off, v);
        }
        for i in 0..n {
            let (entity, motion) = (0x6000 + 0x100 * i, 0x7000 + 0x100 * i);
            h.0.insert(ENTITIES + 4 * i, entity);
            h.0.insert(entity + island_solve::ENTITY_MOTION, motion);
            h.0.insert(motion, vt);
        }
        h
    }

    #[test]
    fn fixed_motions_integrate_to_nothing() {
        let h = heap(3);
        let mut j = Journal::new(&h);
        assert_eq!(integrate(&mut j, ISLAND, INPUT, FPU), Ok(Outcome { solved: false }));
        assert!(j.finish().is_empty());
    }

    #[test]
    fn actions_unknown_motions_and_unmasked_floats_decline() {
        let mut h = heap(2);
        h.0.insert(ISLAND + ISLAND_ACTION_COUNT, 1);
        assert_eq!(integrate(&mut Journal::new(&h), ISLAND, INPUT, FPU), Err(Fallback::Action));
        let mut h = heap(2);
        h.0.insert(0x5000 + MOTION_POSE_SLOT, 0x0012_3456);
        assert_eq!(integrate(&mut Journal::new(&h), ISLAND, INPUT, FPU), Err(Fallback::Motion));
        let h = heap(2);
        let unmasked = Fpu { mxcsr: 0x1f00, control: 0x027f };
        assert_eq!(integrate(&mut Journal::new(&h), ISLAND, INPUT, unmasked), Err(Fallback::Float));
        let mut h = heap(2);
        h.0.remove(&(ENTITIES + 4));
        assert_eq!(integrate(&mut Journal::new(&h), ISLAND, INPUT, FPU), Err(Fallback::Memory));
    }

    #[test]
    fn a_constrained_island_the_solve_declines_is_a_solve_decline() {
        // No solver buffer: the solve's gather declines.
        let mut h = heap(1);
        h.0.insert(ISLAND + ISLAND_CONSTRAINT_COUNT, 1);
        assert_eq!(integrate(&mut Journal::new(&h), ISLAND, INPUT, FPU), Err(Fallback::Solve));
    }

    #[test]
    fn declines_are_counted_by_reason_in_order() {
        let names = fallbacks().map(|(n, _)| n);
        assert_eq!(names, ["memory", "action", "motion", "float", "solve", "monitor", "unobserved"]);
        let before = fallbacks()[1].1;
        count(Fallback::Action);
        assert!(fallbacks()[1].1 > before);
    }
}
