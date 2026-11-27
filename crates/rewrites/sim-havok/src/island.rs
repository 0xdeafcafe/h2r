//! The island solve `0x0031c100` (docs/engine/havok-step.md, "The island
//! solve"): cdecl, nine stack arguments, the caller pops `0x24`.
//!
//! [`solve`] runs the entry on a [`Journal`]. It returns the guest's `eax`,
//! or the [`Fallback`] that sends the island to the lifted entry; on a
//! fallback the journal is discarded, so nothing has been written. Only
//! persistent words are journalled: `motion+0x08`, the pair words, the
//! solver buffer's busy byte. The buffer's contents (accumulators, rows,
//! impulses, schemas) are scratch and stay in Rust.
use crate::jacobian::{self, Cursor};
use crate::mem::Journal;
use crate::motion::{Accumulator, SolverClass, ACCUMULATOR_BYTES};
use std::sync::atomic::{AtomicU64, Ordering};

/// The solver buffer object: base `+0x08`, size `+0x0c`, busy byte `+0x10`.
pub const SOLVER_BUFFER: u32 = 0x0048_4b38;
pub const BUFFER_BASE: u32 = 0x08;
pub const BUFFER_SIZE: u32 = 0x0c;
pub const BUFFER_BUSY: u32 = 0x10;
/// The schema solver's one-time table flag (`0x0031c7d0` sets it).
pub const TABLES_READY: u32 = 0x0048_0c60;
/// The island's size words: the largest constraint's rows `+0x08`, all
/// rows `+0x0c`, schemas `+0x10`, accumulators `+0x14` (less the leading
/// one and the end byte), impulse words `+0x18`.
pub const ISLAND_MAX_ROWS: u32 = 0x08;
pub const ISLAND_ROWS: u32 = 0x0c;
pub const ISLAND_SCHEMAS: u32 = 0x10;
pub const ISLAND_ACCUMULATORS: u32 = 0x14;
pub const ISLAND_IMPULSES: u32 = 0x18;
/// An entity's motion, a motion's accumulator offset from the buffer base.
pub const ENTITY_MOTION: u32 = 0x3c;
pub const MOTION_ACCUMULATOR: u32 = 0x08;
/// The motion virtuals and the constraint's builder.
pub const MOTION_ACCUMULATE_SLOT: u32 = 0x0c;
pub const MOTION_EXPORT_SLOT: u32 = 0x10;
pub const CONSTRAINT_BUILD_SLOT: u32 = 0x24;
pub const CONSTRAINT_ENTITY_A: u32 = 0x0c;
pub const CONSTRAINT_ENTITY_B: u32 = 0x10;

/// The nine arguments, in stack order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Args {
    /// `W+0x150`: `{dt, 1/dt}`.
    pub step: u32,
    /// `W+0x160`: the solver's parameters; `+0x20` is the export scale.
    pub info: u32,
    /// `W+0x190`: the two accumulator addresses of the current constraint.
    pub pairs: u32,
    /// `W+0xe0`.
    pub gravity: u32,
    pub island: u32,
    pub entities: u32,
    pub entity_count: u32,
    pub constraints: u32,
    pub constraint_count: u32,
}

impl Args {
    /// From the words above the return address at `esp`.
    pub fn read(j: &Journal, esp: u32) -> Option<Self> {
        let at = |n: u32| j.get(esp.wrapping_add(4 + 4 * n));
        Some(Self {
            step: at(0)?,
            info: at(1)?,
            pairs: at(2)?,
            gravity: at(3)?,
            island: at(4)?,
            entities: at(5)?,
            entity_count: at(6)?,
            constraints: at(7)?,
            constraint_count: at(8)?,
        })
    }
}

/// Why an island runs the lifted entry. Each is counted ([`fallbacks`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Fallback {
    /// A word the model needs is unmapped or unaligned.
    Memory,
    /// The island does not fit in the solver buffer: retail takes LIFO
    /// blocks from `[0x00480118]` (`0x0031c17d`), which the model does not.
    Overflow,
    /// A motion whose solver virtuals are not ported.
    Motion,
    /// A constraint whose `vt[+0x24]` is not ported.
    Constraint,
    /// The solver tables are not built (`[0x00480c60] == 0`).
    Tables,
    /// The schema solve `0x0031c940` and the export pass are not modelled.
    Solver,
}

impl Fallback {
    pub const ALL: [Fallback; 6] = [
        Self::Memory,
        Self::Overflow,
        Self::Motion,
        Self::Constraint,
        Self::Tables,
        Self::Solver,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Overflow => "overflow",
            Self::Motion => "motion",
            Self::Constraint => "constraint",
            Self::Tables => "tables",
            Self::Solver => "solver",
        }
    }
}

static FALLBACKS: [AtomicU64; 6] = [const { AtomicU64::new(0) }; 6];
static SOLVED: AtomicU64 = AtomicU64::new(0);

/// Count an island sent to the guest.
pub fn count(reason: Fallback) {
    FALLBACKS[reason as usize].fetch_add(1, Ordering::Relaxed);
}

/// Count an island the model answered.
pub fn count_solved() {
    SOLVED.fetch_add(1, Ordering::Relaxed);
}

/// The island fallback counters, by reason, and the solved count.
pub fn fallbacks() -> ([(&'static str, u64); 6], u64) {
    (
        Fallback::ALL.map(|f| (f.name(), FALLBACKS[f as usize].load(Ordering::Relaxed))),
        SOLVED.load(Ordering::Relaxed),
    )
}

/// The buffer's four areas, as `0x0031c142..0x0031c177` lays them out:
/// accumulators (`I+0x14 + 0x90`: the leading one, the entities', the end
/// byte), rows (`I+0x0c`), impulses (`I+0x18` words and 8 bytes),
/// schemas (`I+0x10` and the 4-byte end schema). Sums wrap as retail's do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub accumulators: u32,
    pub rows: u32,
    pub impulses: u32,
    pub schemas: u32,
    /// One past the end schema.
    pub end: u32,
    /// The buffer's end; the island fits when `end <= limit`, unsigned.
    pub limit: u32,
}

impl Layout {
    pub fn read(j: &Journal, island: u32, base: u32, size: u32) -> Option<Self> {
        let word = |off: u32| j.get(island.wrapping_add(off));
        let rows = base
            .wrapping_add(word(ISLAND_ACCUMULATORS)?)
            .wrapping_add(0x90);
        let impulses = rows.wrapping_add(word(ISLAND_ROWS)?);
        let schemas = impulses
            .wrapping_add(word(ISLAND_IMPULSES)?.wrapping_mul(4))
            .wrapping_add(8);
        let end = schemas.wrapping_add(word(ISLAND_SCHEMAS)?).wrapping_add(4);
        Some(Self {
            accumulators: base,
            rows,
            impulses,
            schemas,
            end,
            limit: base.wrapping_add(size),
        })
    }

    pub fn fits(&self) -> bool {
        self.end <= self.limit
    }
}

/// One entity's motion and the accumulator it builds.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    pub motion: u32,
    pub class: SolverClass,
    pub accumulator: Accumulator,
}

/// What the gather stage found: everything the later stages read.
#[derive(Debug)]
pub struct Island {
    pub layout: Layout,
    pub bodies: Vec<Body>,
    pub constraints: Vec<(u32, jacobian::Class)>,
}

/// Read and check everything the solve needs before any write.
pub fn gather(j: &Journal, a: &Args) -> Result<Island, Fallback> {
    use Fallback::*;
    let buffer = j.get(SOLVER_BUFFER).ok_or(Memory)?;
    let base = j.get(buffer.wrapping_add(BUFFER_BASE)).ok_or(Memory)?;
    let size = j.get(buffer.wrapping_add(BUFFER_SIZE)).ok_or(Memory)?;
    j.byte(buffer.wrapping_add(BUFFER_BUSY)).ok_or(Memory)?;
    let layout = Layout::read(j, a.island, base, size).ok_or(Memory)?;
    if !layout.fits() {
        return Err(Overflow);
    }
    let mut bodies = Vec::with_capacity(a.entity_count as usize);
    for i in 0..a.entity_count {
        let entity = j.get(a.entities.wrapping_add(4 * i)).ok_or(Memory)?;
        let motion = j.get(entity.wrapping_add(ENTITY_MOTION)).ok_or(Memory)?;
        let vt = j.get(motion).ok_or(Memory)?;
        j.get(motion.wrapping_add(MOTION_ACCUMULATOR)).ok_or(Memory)?;
        let accumulate = j.get(vt.wrapping_add(MOTION_ACCUMULATE_SLOT)).ok_or(Memory)?;
        let export = j.get(vt.wrapping_add(MOTION_EXPORT_SLOT)).ok_or(Memory)?;
        let class = SolverClass::of(accumulate, export).ok_or(Motion)?;
        bodies.push(Body { motion, class, accumulator: class.accumulator() });
    }
    let mut constraints = Vec::with_capacity(a.constraint_count as usize);
    for i in 0..a.constraint_count {
        let c = j.get(a.constraints.wrapping_add(4 * i)).ok_or(Memory)?;
        let vt = j.get(c).ok_or(Memory)?;
        let build = j.get(vt.wrapping_add(CONSTRAINT_BUILD_SLOT)).ok_or(Memory)?;
        let class = jacobian::Class::of(build).ok_or(Constraint)?;
        constraints.push((c, class));
    }
    if j.byte(TABLES_READY).ok_or(Memory)? == 0 {
        return Err(Tables);
    }
    Ok(Island { layout, bodies, constraints })
}

/// The accumulator stage (`0x0031c2a2..0x0031c31c`): each entity's
/// `motion+0x08` becomes its accumulator's offset from the buffer base,
/// written only when it differs, in entity order.
pub fn place_accumulators(j: &mut Journal, island: &Island) -> Option<()> {
    for (i, body) in island.bodies.iter().enumerate() {
        let at = body.motion.wrapping_add(MOTION_ACCUMULATOR);
        let offset = ACCUMULATOR_BYTES * (i as u32 + 1);
        if j.get(at)? != offset {
            j.put(at, offset)?;
        }
    }
    Some(())
}

/// The constraint stage's persistent writes (`0x0031c343..0x0031c36c`):
/// the pair words get each constraint's two accumulator addresses, entity
/// A's then B's, read through the motions' offsets as they stand. An
/// entity outside the island (the level, in the fixed island) keeps the
/// offset it had, usually 0: the leading zeroed accumulator.
pub fn pair(j: &mut Journal, a: &Args, base: u32, constraint: u32) -> Option<[u32; 2]> {
    let mut out = [0; 2];
    for (slot, side) in [CONSTRAINT_ENTITY_A, CONSTRAINT_ENTITY_B].into_iter().enumerate() {
        let entity = j.get(constraint.wrapping_add(side))?;
        let motion = j.get(entity.wrapping_add(ENTITY_MOTION))?;
        out[slot] = j.get(motion.wrapping_add(MOTION_ACCUMULATOR))?.wrapping_add(base);
        j.put(a.pairs.wrapping_add(4 * slot as u32), out[slot])?;
    }
    Some(out)
}

/// The whole entry. `Ok` is the guest's `eax` (0: no LIFO block was taken).
pub fn solve(j: &mut Journal, a: &Args) -> Result<u32, Fallback> {
    let island = gather(j, a)?;
    place_accumulators(j, &island).ok_or(Fallback::Memory)?;
    let base = island.layout.accumulators;
    let mut cursor = Cursor {
        rows: island.layout.rows,
        schemas: island.layout.schemas,
        limit: island.layout.impulses,
        overflow: 0,
    };
    for &(constraint, class) in &island.constraints {
        pair(j, a, base, constraint).ok_or(Fallback::Memory)?;
        cursor.before_constraint();
        match class {}
    }
    // An island always has a constraint (`0x00316a20` calls this entry
    // only when `I+0x5c` is not zero), so with no class ported the solve
    // never gets here. The solve (`0x0031c940` over the schemas), the
    // exports and the busy byte's release come with the first class.
    Err(Fallback::Solver)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mem::Words;
    use crate::motion;
    use std::collections::HashMap;

    #[derive(Default)]
    struct Heap(HashMap<u32, u32>);

    impl Words for Heap {
        fn get(&self, addr: u32) -> Option<u32> {
            self.0.get(&addr).copied()
        }
    }

    const BUFFER: u32 = 0x1000;
    const BASE: u32 = 0x10_0000;
    const ISLAND: u32 = 0x2000;
    const ENTITIES: u32 = 0x3000;
    const CONSTRAINTS: u32 = 0x3100;
    const MOTIONS: u32 = 0x4000;
    const FIXED_VT: u32 = 0x5000;
    const CONTACT_VT: u32 = 0x5100;

    fn heap(entities: u32, size: u32) -> Heap {
        let mut h = Heap::default();
        let mut put = |a: u32, v: u32| {
            h.0.insert(a, v);
        };
        put(SOLVER_BUFFER, BUFFER);
        put(BUFFER + BUFFER_BASE, BASE);
        put(BUFFER + BUFFER_SIZE, size);
        put(BUFFER + BUFFER_BUSY, 0);
        put(TABLES_READY, 1);
        put(ISLAND + ISLAND_MAX_ROWS, 0x60);
        put(ISLAND + ISLAND_ROWS, 0x60);
        put(ISLAND + ISLAND_SCHEMAS, 0x30);
        put(ISLAND + ISLAND_ACCUMULATORS, 0x80 * entities);
        put(ISLAND + ISLAND_IMPULSES, 1);
        put(FIXED_VT + MOTION_ACCUMULATE_SLOT, motion::FIXED_ACCUMULATE);
        put(FIXED_VT + MOTION_EXPORT_SLOT, motion::FIXED_EXPORT);
        put(CONTACT_VT + CONSTRAINT_BUILD_SLOT, jacobian::CONTACT_BUILD);
        for i in 0..entities {
            let entity = 0x6000 + 0x100 * i;
            let motion = MOTIONS + 0x100 * i;
            put(ENTITIES + 4 * i, entity);
            put(entity + ENTITY_MOTION, motion);
            put(motion, FIXED_VT);
            put(motion + MOTION_ACCUMULATOR, 0);
        }
        put(CONSTRAINTS, 0x7000);
        put(0x7000, CONTACT_VT);
        h
    }

    fn args(entities: u32) -> Args {
        Args {
            step: 0x8000,
            info: 0x8010,
            pairs: 0x8040,
            gravity: 0x8050,
            island: ISLAND,
            entities: ENTITIES,
            entity_count: entities,
            constraints: CONSTRAINTS,
            constraint_count: 1,
        }
    }

    #[test]
    fn the_layout_follows_the_island_sizes_and_fits_unsigned() {
        let h = heap(2, 0x1000);
        let j = Journal::new(&h);
        let l = Layout::read(&j, ISLAND, BASE, 0x1000).unwrap();
        assert_eq!(l.rows, BASE + 0x100 + 0x90);
        assert_eq!(l.impulses, l.rows + 0x60);
        assert_eq!(l.schemas, l.impulses + 4 + 8);
        assert_eq!(l.end, l.schemas + 0x30 + 4);
        assert!(l.fits());
        // One byte short.
        let short = Layout::read(&j, ISLAND, BASE, l.end - BASE - 1).unwrap();
        assert!(!short.fits());
    }

    #[test]
    fn an_island_that_overflows_the_buffer_falls_back_first() {
        let h = heap(2, 0x100);
        let mut j = Journal::new(&h);
        assert_eq!(solve(&mut j, &args(2)), Err(Fallback::Overflow));
        assert!(j.finish().is_empty());
    }

    #[test]
    fn a_contact_island_falls_back_with_nothing_written() {
        let h = heap(2, 0x1000);
        assert_eq!(gather(&Journal::new(&h), &args(2)).unwrap_err(), Fallback::Constraint);
        let mut j = Journal::new(&h);
        assert_eq!(solve(&mut j, &args(2)), Err(Fallback::Constraint));
        assert!(j.finish().is_empty());
    }

    #[test]
    fn an_unported_motion_falls_back_before_the_constraints_are_read() {
        let mut h = heap(1, 0x1000);
        h.0.insert(FIXED_VT + MOTION_EXPORT_SLOT, motion::KEYFRAMED_EXPORT);
        h.0.remove(&CONSTRAINTS);
        assert_eq!(gather(&Journal::new(&h), &args(1)).unwrap_err(), Fallback::Motion);
    }

    #[test]
    fn accumulator_offsets_follow_entity_order_and_skip_equal_words() {
        let mut h = heap(3, 0x1000);
        // The second already holds its offset.
        h.0.insert(MOTIONS + 0x100 + MOTION_ACCUMULATOR, 0x100);
        let mut j = Journal::new(&h);
        let island = Island {
            layout: Layout::read(&j, ISLAND, BASE, 0x1000).unwrap(),
            bodies: (0..3)
                .map(|i| Body {
                    motion: MOTIONS + 0x100 * i,
                    class: SolverClass::Fixed,
                    accumulator: motion::fixed_accumulator(),
                })
                .collect(),
            constraints: vec![],
        };
        place_accumulators(&mut j, &island).unwrap();
        let w = j.finish();
        assert_eq!(w.get(&(MOTIONS + MOTION_ACCUMULATOR)), Some(&0x80));
        assert_eq!(w.get(&(MOTIONS + 0x100 + MOTION_ACCUMULATOR)), None);
        assert_eq!(w.get(&(MOTIONS + 0x200 + MOTION_ACCUMULATOR)), Some(&0x180));
    }

    #[test]
    fn the_pair_words_take_both_accumulators_through_the_offsets() {
        let mut h = heap(2, 0x1000);
        // Constraint between entity 1 (A) and the level (B), whose motion
        // was never placed: offset 0, the leading accumulator.
        h.0.insert(0x7000 + CONSTRAINT_ENTITY_A, 0x6100);
        h.0.insert(0x7000 + CONSTRAINT_ENTITY_B, 0x9000);
        h.0.insert(0x9000 + ENTITY_MOTION, 0x9100);
        h.0.insert(0x9100 + MOTION_ACCUMULATOR, 0);
        h.0.insert(MOTIONS + 0x100 + MOTION_ACCUMULATOR, 0x100);
        let mut j = Journal::new(&h);
        let a = args(2);
        assert_eq!(pair(&mut j, &a, BASE, 0x7000), Some([BASE + 0x100, BASE]));
        let w = j.finish();
        assert_eq!(w.get(&a.pairs), Some(&(BASE + 0x100)));
        assert_eq!(w.get(&(a.pairs + 4)), Some(&BASE));
    }

    #[test]
    fn fallbacks_are_counted_by_reason() {
        let (before, _) = fallbacks();
        count(Fallback::Constraint);
        let (after, _) = fallbacks();
        let i = Fallback::Constraint as usize;
        assert_eq!(after[i].0, "constraint");
        assert!(after[i].1 > before[i].1);
    }
}
