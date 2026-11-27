//! The island solve `0x0031c100` (docs/engine/havok-step.md, "The island
//! solve"): cdecl, nine stack arguments, the caller pops `0x24`.
//!
//! [`solve`] runs the entry on a [`Journal`]. It returns the guest's `eax`,
//! or the [`Fallback`] that sends the island to the lifted entry; on a
//! fallback the journal is discarded, so nothing has been written. Only
//! persistent words are journalled: `motion+0x08`, the motions' fields the
//! virtuals write ([`motion_solve`]), the pair words, the
//! contact builder's point properties and friction blocks, the write-back
//! pass's outputs, the solver buffer's busy byte, and the LIFO
//! descriptor's words and the allocator's when an island spills into a
//! block ([`place`]). The buffer's and the block's contents (accumulators,
//! rows, impulses, schemas) are scratch and stay in Rust ([`Scratch`]).
use crate::jacobian::{self, Cursor, Fpu, Scratch};
use crate::layout as l;
use crate::mem::{self, Journal, System};
use crate::solver::{self, Areas, Params};
use crate::motion::{Accumulator, SolverClass, ACCUMULATOR_BYTES, ACCUMULATOR_END, ACCUMULATOR_KIND};
use crate::motion_solve::{self, Solved};
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
    /// The island's LIFO block comes from a route the allocator model
    /// does not cover (the overflow object, an unexpected vtable), or its
    /// rows run past the buffer or the block, where retail writes on.
    Overflow,
    /// A motion whose solver virtuals are not ported.
    Motion,
    /// A constraint whose `vt[+0x24]` is not ported, or a contact the
    /// builder does not cover (a point shape retail would corrupt, a
    /// friction axis retail would spin on, too many points).
    Constraint,
    /// The solver tables are not built (`[0x00480c60] == 0`).
    Tables,
    /// The schema solve declined the stream: a non-finite operand, or a
    /// stream shape its decode does not cover.
    Solver,
    /// An unmasked float exception, or a non-finite float the builder's
    /// x87 code would load from memory.
    Float,
}

impl Fallback {
    pub const ALL: [Fallback; 7] = [
        Self::Memory,
        Self::Overflow,
        Self::Motion,
        Self::Constraint,
        Self::Tables,
        Self::Solver,
        Self::Float,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Memory => "memory",
            Self::Overflow => "overflow",
            Self::Motion => "motion",
            Self::Constraint => "constraint",
            Self::Tables => "tables",
            Self::Solver => "solver",
            Self::Float => "float",
        }
    }
}

static FALLBACKS: [AtomicU64; 7] = [const { AtomicU64::new(0) }; 7];
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
pub fn fallbacks() -> ([(&'static str, u64); 7], u64) {
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

/// The LIFO descriptor's virtuals the entry calls when the inline block
/// does not fit or is the chunk's first (`0x002d7020`, `0x002d7090`), and
/// the dispatcher's allocate and free they call (`0x001472c0`,
/// `0x00147320`); [`mem`] models those four.
pub const LIFO_PUSH_SLOT: u32 = 0x24;
pub const LIFO_POP_SLOT: u32 = 0x28;
const LIFO_PUSH: u32 = 0x002d_7020;
const LIFO_POP: u32 = 0x002d_7090;
const DISPATCH_ALLOCATE_SLOT: u32 = 0x10;
const DISPATCH_FREE_SLOT: u32 = 0x14;
const DISPATCH_ALLOCATE: u32 = 0x0014_72c0;
const DISPATCH_FREE: u32 = 0x0014_7320;

/// The block an island spilled into (`0x0031c17d..0x0031c29d`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Spill {
    /// The block, which is the entry's `eax`.
    pub block: u32,
    /// One past its last byte: the block plus the unaligned request.
    pub end: u32,
}

/// Where the areas lie once the entry has taken any block it needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Placed {
    pub layout: Layout,
    /// Where the rows in the buffer must end: the row area's end, or the
    /// buffer's end when only the rows overflow.
    pub rows_end: u32,
    /// The cursor's limit and overflow block (`+0x24`, `+0x28`).
    pub cursor_limit: u32,
    pub overflow: u32,
    pub spill: Option<Spill>,
    /// The scratch ranges: the buffer's and the block's.
    pub ranges: Vec<(u32, u32)>,
}

fn align16(n: u32) -> u32 {
    n.wrapping_add(15) & !15
}

/// The memory system `[0x00480118]` with the four virtuals the model
/// covers, or `None`.
fn lifo_system(j: &Journal) -> Option<(u32, System)> {
    let d = j.get(l::MEMORY_SYSTEM)?;
    let vt = j.get(d)?;
    let slots = [
        (LIFO_PUSH_SLOT, LIFO_PUSH),
        (LIFO_POP_SLOT, LIFO_POP),
        (DISPATCH_ALLOCATE_SLOT, DISPATCH_ALLOCATE),
        (DISPATCH_FREE_SLOT, DISPATCH_FREE),
    ];
    for (slot, f) in slots {
        if j.get(vt.wrapping_add(slot))? != f {
            return None;
        }
    }
    Some((d, System::read(j)?))
}

/// Take `bytes` (rounded up to 16) from the LIFO descriptor: inline when
/// the remaining bytes allow (signed), else through `vt[+0x24]`.
fn lifo_take(j: &mut Journal, bytes: u32) -> Result<u32, Fallback> {
    let d = j.get(l::MEMORY_SYSTEM).ok_or(Fallback::Memory)?;
    let request = align16(bytes);
    let remaining = j.get(d.wrapping_add(l::POOL_LIFO_REMAINING)).ok_or(Fallback::Memory)?;
    if request as i32 > remaining as i32 {
        let (d, sys) = lifo_system(j).ok_or(Fallback::Overflow)?;
        return mem::lifo_allocate(&sys, j, d, request).ok_or(Fallback::Overflow);
    }
    let cursor = d.wrapping_add(l::POOL_LIFO_CURSOR);
    let block = j.get(cursor).ok_or(Fallback::Memory)?;
    j.put(cursor, block.wrapping_add(request)).ok_or(Fallback::Memory)?;
    j.put(d.wrapping_add(l::POOL_LIFO_REMAINING), remaining.wrapping_sub(request))
        .ok_or(Fallback::Memory)?;
    Ok(block)
}

/// Give the block back (`0x0031c45d`): inline, unless it is the chunk's
/// payload or the descriptor has none, which `vt[+0x28]` pops. Returns the
/// entry's `eax`: the inline path leaves the descriptor's remaining bytes
/// there (`0x0031c489`); after `vt[+0x28]` it is whatever the dispatcher's
/// free left, which the model does not track and gives as 0 (the caller
/// `0x00316b16` passes it on unread).
fn lifo_release(j: &mut Journal, spill: Spill) -> Result<u32, Fallback> {
    let d = j.get(l::MEMORY_SYSTEM).ok_or(Fallback::Memory)?;
    let bytes = align16(spill.end.wrapping_sub(spill.block));
    if bytes != 0 {
        let payload = j.get(d.wrapping_add(l::POOL_LIFO_PAYLOAD)).ok_or(Fallback::Memory)?;
        if spill.block == payload || payload == 0 {
            let (d, sys) = lifo_system(j).ok_or(Fallback::Overflow)?;
            mem::lifo_free(&sys, j, d, bytes).ok_or(Fallback::Overflow)?;
            return Ok(0);
        }
    }
    let cursor = d.wrapping_add(l::POOL_LIFO_CURSOR);
    let remaining = d.wrapping_add(l::POOL_LIFO_REMAINING);
    let (c, r) = (j.get(cursor).ok_or(Fallback::Memory)?, j.get(remaining).ok_or(Fallback::Memory)?);
    j.put(cursor, c.wrapping_sub(bytes)).ok_or(Fallback::Memory)?;
    j.put(remaining, r.wrapping_add(bytes)).ok_or(Fallback::Memory)?;
    Ok(r.wrapping_add(bytes))
}

/// The fit test and the spill (`0x0031c142..0x0031c29d`). An island past
/// the buffer's end takes one LIFO block: for every area when even the
/// rows start past the end, for the impulses and schemas when the rows
/// fit, and else for the rows' tail plus twice the largest constraint's
/// rows (`I+0x08`) ahead of the impulses and schemas, the cursor's limit
/// then the buffer's end less `I+0x08`.
pub fn place(j: &mut Journal, island: u32, l: Layout) -> Result<Placed, Fallback> {
    use Fallback::*;
    let word = |j: &Journal, off: u32| j.get(island.wrapping_add(off)).ok_or(Memory);
    let impulse_bytes = word(j, ISLAND_IMPULSES)?.wrapping_mul(4).wrapping_add(8);
    let schema_bytes = word(j, ISLAND_SCHEMAS)?;
    let limit = l.limit;
    if l.fits() {
        let ranges = vec![(l.accumulators, l.end)];
        return Ok(Placed { layout: l, rows_end: l.impulses, cursor_limit: l.impulses, overflow: 0, spill: None, ranges });
    }
    let (layout, rows_end, cursor_limit, overflow, spill, ranges);
    if l.rows >= limit {
        // 0x0031c251: every area moves into the block.
        let bytes = l.end.wrapping_sub(l.accumulators);
        let block = lifo_take(j, bytes)?;
        let end = block.wrapping_add(bytes);
        layout = Layout::read(j, island, block, bytes).ok_or(Memory)?;
        (rows_end, cursor_limit, overflow) = (layout.impulses, layout.impulses, 0);
        spill = Spill { block, end };
        ranges = vec![(block, end)];
    } else if l.impulses < limit {
        // 0x0031c203: the rows fit; the impulses and schemas move.
        let bytes = schema_bytes.wrapping_add(impulse_bytes).wrapping_add(4);
        let block = lifo_take(j, bytes)?;
        let end = block.wrapping_add(bytes);
        let schemas = block.wrapping_add(impulse_bytes);
        layout = Layout { impulses: block, schemas, end: schemas.wrapping_add(schema_bytes).wrapping_add(4), limit: end, ..l };
        (rows_end, cursor_limit, overflow) = (l.impulses, l.impulses, 0);
        spill = Spill { block, end };
        ranges = vec![(l.accumulators, l.impulses), (block, end)];
    } else {
        // 0x0031c18f: the rows' tail continues in the block.
        let max_rows = word(j, ISLAND_MAX_ROWS)?;
        let lead = max_rows.wrapping_mul(2).wrapping_sub(limit).wrapping_add(l.impulses);
        let bytes = schema_bytes.wrapping_add(lead).wrapping_add(impulse_bytes).wrapping_add(4);
        let block = lifo_take(j, bytes)?;
        let end = block.wrapping_add(bytes);
        let impulses = block.wrapping_add(lead);
        let schemas = impulses.wrapping_add(impulse_bytes);
        layout = Layout { impulses, schemas, end: schemas.wrapping_add(schema_bytes).wrapping_add(4), limit: end, ..l };
        (rows_end, cursor_limit, overflow) = (limit, limit.wrapping_sub(max_rows), block);
        spill = Spill { block, end };
        ranges = vec![(l.accumulators, limit), (block, end)];
    }
    if !layout.fits() {
        return Err(Overflow);
    }
    Ok(Placed { layout, rows_end, cursor_limit, overflow, spill: Some(spill), ranges })
}

/// One entity's motion and its solver class.
#[derive(Clone, Copy, Debug)]
pub struct Body {
    pub motion: u32,
    pub class: SolverClass,
}

/// What the gather stage found: everything the later stages read.
#[derive(Debug)]
pub struct Island {
    /// The solver buffer object `[0x00484b38]`.
    pub buffer: u32,
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
    let mut bodies = Vec::with_capacity(a.entity_count as usize);
    for i in 0..a.entity_count {
        let entity = j.get(a.entities.wrapping_add(4 * i)).ok_or(Memory)?;
        let motion = j.get(entity.wrapping_add(ENTITY_MOTION)).ok_or(Memory)?;
        let vt = j.get(motion).ok_or(Memory)?;
        j.get(motion.wrapping_add(MOTION_ACCUMULATOR)).ok_or(Memory)?;
        let accumulate = j.get(vt.wrapping_add(MOTION_ACCUMULATE_SLOT)).ok_or(Memory)?;
        let export = j.get(vt.wrapping_add(MOTION_EXPORT_SLOT)).ok_or(Memory)?;
        let class = SolverClass::of(accumulate, export).ok_or(Motion)?;
        bodies.push(Body { motion, class });
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
    Ok(Island { buffer, layout, bodies, constraints })
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

/// The schema solve's image constant: its comparand zero.
pub const SOLVER_ZERO: u32 = 0x0045_dbd8;
/// More iterations than this decline (retail's parameters hold a few).
pub const MAX_ITERATIONS: u32 = 64;

/// Each entity's `vt[+0x0c]` in array order: the motion's pending forces
/// integrated (journalled) and its accumulator built.
pub fn accumulate(j: &mut Journal, a: &Args, island: &Island, fpu: Fpu) -> Result<Vec<Accumulator>, Fallback> {
    island
        .bodies
        .iter()
        .map(|b| motion_solve::accumulate(j, b.class, b.motion, a.step, a.gravity, fpu))
        .collect()
}

/// Lay the accumulators into the scratch buffer as the frame and each
/// motion's `vt[+0x0c]` leave them (`0x0031c2a2..0x0031c31c`): kind 2 at
/// byte 0, the flag at `+0x0c` as the class writes it or as the guest's
/// buffer holds it, the seven vectors, and the end byte 3 after the last.
pub fn lay_accumulators(j: &Journal, s: &mut Scratch, island: &Island, accumulators: &[Accumulator]) -> Option<()> {
    let base = island.layout.accumulators;
    let leading = crate::motion::fixed_accumulator();
    for (i, accumulator) in std::iter::once(&leading).chain(accumulators).enumerate() {
        let at = base.checked_add(ACCUMULATOR_BYTES * i as u32)?;
        let kind = j.get(at)? & !0xff | u32::from(ACCUMULATOR_KIND);
        s.put(at, kind)?;
        for off in [4, 8] {
            s.put(at + off, j.get(at + off)?)?;
        }
        let flag = j.get(at + 0x0c)?;
        let flag = match accumulator.flag {
            Some(f) => flag & !0xff | u32::from(f),
            None => flag,
        };
        s.put(at + 0x0c, flag)?;
        for (k, v) in accumulator.vectors.iter().enumerate() {
            s.put_vector(at + 0x10 + 0x10 * k as u32, *v)?;
        }
    }
    let end = base.checked_add(ACCUMULATOR_BYTES * (island.bodies.len() as u32 + 1))?;
    s.put(end, j.get(end)? & !0xff | u32::from(ACCUMULATOR_END))
}

/// The whole entry. `Ok` is the guest's `eax`: 0 when the island fits,
/// else what giving the block back leaves ([`lifo_release`]).
pub fn solve(j: &mut Journal, a: &Args, fpu: Fpu) -> Result<u32, Fallback> {
    use Fallback::*;
    let mut island = gather(j, a)?;
    let placed = place(j, a.island, island.layout)?;
    // The later stages lay the areas where the spill put them.
    island.layout = placed.layout;
    if !fpu.masked() {
        return Err(Float);
    }
    if island.constraints.is_empty() {
        // `0x00316a20` never calls the entry so; the solve would read the
        // end schema as a header.
        return Err(Constraint);
    }
    let l = placed.layout;
    let accumulators_end = l.accumulators.wrapping_add(ACCUMULATOR_BYTES * (island.bodies.len() as u32 + 1));
    if accumulators_end >= l.rows || l.rows < l.accumulators {
        return Err(Overflow);
    }
    place_accumulators(j, &island).ok_or(Memory)?;
    let accumulators = accumulate(j, a, &island, fpu)?;
    let mut s = Scratch::of(&placed.ranges).ok_or(Memory)?;
    lay_accumulators(j, &mut s, &island, &accumulators).ok_or(Memory)?;
    let base = l.accumulators;
    let mut cursor = Cursor { rows: l.rows, schemas: l.schemas, limit: placed.cursor_limit, overflow: placed.overflow };
    // Where the buffer's rows ended when the cursor moved to the block.
    let mut moved_at = None;
    for &(constraint, class) in &island.constraints {
        let pair = pair(j, a, base, constraint).ok_or(Memory)?;
        if pair.iter().any(|&p| p < base || p.wrapping_sub(base) % ACCUMULATOR_BYTES != 0 || p >= accumulators_end) {
            return Err(Memory);
        }
        let rows = cursor.rows;
        cursor.before_constraint();
        if moved_at.is_none() && cursor.limit == u32::MAX {
            if cursor.overflow == 0 {
                // No block to continue in: retail writes from address 0.
                return Err(Overflow);
            }
            moved_at = Some(rows);
        }
        class.build(j, &mut s, fpu, constraint, a.pairs, &mut cursor)?;
        let rows_end = if moved_at.is_some() { l.impulses } else { placed.rows_end };
        if cursor.rows > rows_end || cursor.schemas.wrapping_add(4) > l.end || cursor.schemas < l.schemas {
            return Err(Overflow);
        }
    }
    // 0x0031c3a3: the end schema; the impulses are zero (Rust's buffer).
    s.put(cursor.schemas, solver::SCHEMA_END).ok_or(Memory)?;
    let params = Params::read(|at| j.get(at), a.info).ok_or(Memory)?;
    let impulse_words = j.get(a.island.wrapping_add(ISLAND_IMPULSES)).ok_or(Memory)?;
    let areas = Areas {
        accumulators: base,
        count: island.bodies.len() as u32,
        rows: l.rows,
        rows_end: moved_at.unwrap_or(cursor.rows),
        overflow: moved_at.map(|_| (cursor.overflow, cursor.rows)),
        impulse_words,
    };
    let decoded = solver::decode(&s, l.schemas, areas, params).ok_or(Solver)?;
    let iterations = (params.iterations as i32).max(0) as u32;
    if iterations > MAX_ITERATIONS {
        return Err(Solver);
    }
    let zero = j.get(SOLVER_ZERO).ok_or(Memory)?;
    let solved = decoded
        .stream
        .clone()
        .solve(iterations as usize, params.bias, zero, fpu.mxcsr, fpu.control)
        .ok_or(Solver)?;
    for (addr, value) in decoded.writeback(&solved, params.export, fpu.control) {
        j.put(addr, value).ok_or(Memory)?;
    }
    for (addr, b) in decoded.accumulators.iter().zip(&solved.bodies) {
        for (off, v) in [(0x10, b.linear), (0x20, b.angular), (0x40, b.linear_sum), (0x50, b.angular_sum)] {
            s.put_vector(addr + off, v).ok_or(Memory)?;
        }
    }
    // 0x0031c414: each motion's export, in entity order, from its
    // accumulator as the solve left it.
    for (i, body) in island.bodies.iter().enumerate() {
        let at = base + ACCUMULATOR_BYTES * (i as u32 + 1);
        let solved = Solved::read(|w| s.get(w), at).ok_or(Memory)?;
        motion_solve::export(j, body.class, body.motion, a.step, params.scale, &solved, fpu)?;
    }
    // 0x0031c457: the busy byte, set to 1 on entry, is 0 again.
    let busy = island.buffer.wrapping_add(BUFFER_BUSY);
    let word = j.get(busy & !3).ok_or(Memory)?;
    j.put(busy & !3, word & !(0xff << (8 * (busy & 3)))).ok_or(Memory)?;
    // 0x0031c45d: the block goes back.
    match placed.spill {
        Some(spill) => lifo_release(j, spill),
        None => Ok(0),
    }
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

    const SYSTEM: u32 = 0x9000;
    const LIFO_CURSOR: u32 = 0x20_0000;

    /// The memory system with `remaining` inline bytes at `LIFO_CURSOR`
    /// in a chunk whose payload is `payload`, and no vtable.
    fn lifo(h: &mut Heap, remaining: u32, payload: u32) {
        h.0.insert(l::MEMORY_SYSTEM, SYSTEM);
        h.0.insert(SYSTEM + l::POOL_LIFO_CURSOR, LIFO_CURSOR);
        h.0.insert(SYSTEM + l::POOL_LIFO_REMAINING, remaining);
        h.0.insert(SYSTEM + l::POOL_LIFO_PAYLOAD, payload);
    }

    #[test]
    fn an_island_whose_rows_start_past_the_buffer_moves_into_one_block() {
        let mut h = heap(2, 0x100);
        lifo(&mut h, 0x1000, 0x1f_0000);
        let mut j = Journal::new(&h);
        let l = Layout::read(&j, ISLAND, BASE, 0x100).unwrap();
        let bytes = l.end - BASE;
        let p = place(&mut j, ISLAND, l).unwrap();
        assert_eq!(p.layout, Layout::read(&j, ISLAND, LIFO_CURSOR, bytes).unwrap());
        assert_eq!(p.spill, Some(Spill { block: LIFO_CURSOR, end: LIFO_CURSOR + bytes }));
        assert_eq!(p.ranges, vec![(LIFO_CURSOR, LIFO_CURSOR + bytes)]);
        assert_eq!((p.cursor_limit, p.overflow), (p.layout.impulses, 0));
        assert_eq!(j.get(SYSTEM + l::POOL_LIFO_CURSOR), Some(LIFO_CURSOR + (bytes + 15) / 16 * 16));
        // The inline release leaves the descriptor as it was.
        lifo_release(&mut j, p.spill.unwrap()).unwrap();
        for off in [l::POOL_LIFO_CURSOR, l::POOL_LIFO_REMAINING] {
            assert_eq!(j.get(SYSTEM + off), h.0.get(&(SYSTEM + off)).copied());
        }
    }

    #[test]
    fn an_island_whose_rows_fit_moves_its_impulses_and_schemas() {
        let mut h = heap(2, 0x200);
        lifo(&mut h, 0x1000, 0x1f_0000);
        let mut j = Journal::new(&h);
        let l = Layout::read(&j, ISLAND, BASE, 0x200).unwrap();
        assert!(l.impulses < l.limit && !l.fits());
        let p = place(&mut j, ISLAND, l).unwrap();
        let end = LIFO_CURSOR + 0x30 + 4 * 1 + 8 + 4;
        assert_eq!(p.layout.impulses, LIFO_CURSOR);
        assert_eq!(p.layout.schemas, LIFO_CURSOR + 0xc);
        assert_eq!(p.layout.end, end);
        assert_eq!(p.ranges, vec![(BASE, l.impulses), (LIFO_CURSOR, end)]);
        assert_eq!((p.rows_end, p.cursor_limit, p.overflow), (l.impulses, l.impulses, 0));
    }

    #[test]
    fn an_island_whose_rows_overflow_continues_them_in_the_block() {
        let mut h = heap(2, 0x1a0);
        lifo(&mut h, 0x1000, 0x1f_0000);
        let mut j = Journal::new(&h);
        let l = Layout::read(&j, ISLAND, BASE, 0x1a0).unwrap();
        let p = place(&mut j, ISLAND, l).unwrap();
        // The tail (impulses - end) and twice the largest constraint's rows.
        let lead = l.impulses - (BASE + 0x1a0) + 2 * 0x60;
        assert_eq!(p.layout.impulses, LIFO_CURSOR + lead);
        assert_eq!((p.rows_end, p.cursor_limit, p.overflow), (BASE + 0x1a0, BASE + 0x1a0 - 0x60, LIFO_CURSOR));
        assert_eq!(p.ranges, vec![(BASE, BASE + 0x1a0), (LIFO_CURSOR, p.layout.end)]);
        assert!(p.layout.fits());
    }

    #[test]
    fn a_block_from_a_route_the_model_does_not_cover_falls_back() {
        // Too few inline bytes and no covered vtable: `vt[+0x24]`.
        let mut h = heap(2, 0x100);
        lifo(&mut h, 0x10, 0x1f_0000);
        let mut j = Journal::new(&h);
        assert_eq!(solve(&mut j, &args(2), FPU), Err(Fallback::Overflow));
        // The chunk's first block pops through `vt[+0x28]`.
        let mut h = heap(2, 0x100);
        lifo(&mut h, 0x1000, LIFO_CURSOR);
        let mut j = Journal::new(&h);
        let spill = Spill { block: LIFO_CURSOR, end: LIFO_CURSOR + 0x40 };
        assert_eq!(lifo_release(&mut j, spill), Err(Fallback::Overflow));
    }

    const FPU: Fpu = Fpu { mxcsr: 0x1f80, control: 0x027f };

    #[test]
    fn an_unported_constraint_falls_back_with_nothing_written() {
        let mut h = heap(2, 0x1000);
        h.0.insert(CONTACT_VT + CONSTRAINT_BUILD_SLOT, 0x0031_be20);
        assert_eq!(gather(&Journal::new(&h), &args(2)).unwrap_err(), Fallback::Constraint);
        let mut j = Journal::new(&h);
        assert_eq!(solve(&mut j, &args(2), FPU), Err(Fallback::Constraint));
        assert!(j.finish().is_empty());
    }

    #[test]
    fn a_contact_island_is_gathered_and_unmasked_floats_decline() {
        let h = heap(2, 0x1000);
        let island = gather(&Journal::new(&h), &args(2)).unwrap();
        assert_eq!(island.constraints, vec![(0x7000, jacobian::Class::Contact)]);
        let mut j = Journal::new(&h);
        let unmasked = Fpu { mxcsr: 0x1f00, control: 0x027f };
        assert_eq!(solve(&mut j, &args(2), unmasked), Err(Fallback::Float));
        assert!(j.finish().is_empty());
    }

    #[test]
    fn an_unported_motion_falls_back_before_the_constraints_are_read() {
        let mut h = heap(1, 0x1000);
        h.0.insert(FIXED_VT + MOTION_EXPORT_SLOT, motion::SPHERE_EXPORT);
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
            buffer: BUFFER,
            layout: Layout::read(&j, ISLAND, BASE, 0x1000).unwrap(),
            bodies: (0..3)
                .map(|i| Body { motion: MOTIONS + 0x100 * i, class: SolverClass::Fixed })
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
