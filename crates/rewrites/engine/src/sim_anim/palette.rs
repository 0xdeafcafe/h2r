//! The guest side of `sim_anim::palette`: the first-person bone palette
//! step of `16760c` as one gather, one kernel call and one scatter
//! (docs/engine/palette.md; phase 1c of docs/engine/sim-rewrite.md).
//!
//! The register-level port (`sim_physics::first_person`) stays the
//! vehicle; at its palette step — the node count in `ax` of `eax`, the
//! seat's arguments at `[ebp + 8]` and `[ebp + 0xc]`, the two models at
//! `[ebp - 0xc]` and `[ebp - 0x1c]`, the palette indices at `ebx + 0xfc`
//! and `ebx + 0x1fc` — [`run`] gathers the identity record, the model
//! nodes and the indices, lets the kernel build the palette span in
//! Rust, and writes what the step writes: the palette bytes, the node
//! count at `[ebx + 0x300]`, the palette base at `[ebp - 4]`, the final
//! counter at `[ebp + 8]`, the pushed 8 below `esp` the identity fill's
//! `rep movsd` setup leaves, and the registers and flags the loops
//! leave. The call to `0xb6780` that ends the step is made either way.
//!
//! The step is pure copying, so there is no float decline. It declines
//! before writing anything (the port's loops run) when a read faults,
//! when a count passes the gather bound, when a write overlaps a
//! range it reads, or when the direction flag is set (the guest's
//! copies then run downward from each record's first word and write
//! below it, a window the kernel does not model).
use crate::function_eval::constant::ONE;
use crate::sim_native::Memory;
use ::sim_anim::palette::{self, Overlay};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The palettes table `[0x4e9bc4]` and the identity record pointer
/// `[0x4687d8]`.
const PALETTES: u32 = 0x004e_9bc4;
const IDENTITY: u32 = 0x0046_87d8;
/// The call that ends the step, and where it returns.
const BUILD_MATRICES: u32 = 0x000b_6780;
const AFTER_MATRICES: u32 = 0x0016_7804;
/// The step's frame: the seat arguments above `ebp`, and the two models,
/// the saved `edi` and the graph pointer below it.
const ARG_A: u32 = 0xc;
const ARG_B: u32 = 8;
const MODEL_ONE: u32 = 0x0c;
const MODEL_TWO: u32 = 0x1c;
const SAVED_EDI: u32 = 0x10;
const GRAPH: u32 = 8;
const BASE_SLOT: u32 = 4;
const COUNTER_SLOT: u32 = 8;
/// The palette index tables in the weapon state `ebx` points at, and the
/// node count it takes.
const INDICES: [u32; 2] = [0xfc, 0x1fc];
const NODE_COUNT: u32 = 0x300;
/// A model's node count `+0x48` and node array `+0x4c`.
const MODEL_COUNT: u32 = 0x48;
const MODEL_NODES: u32 = 0x4c;
/// The largest node count the kernel gathers (the fill's and each
/// model's).
pub const NODES_MAX: usize = 4096;

static ENABLED: AtomicBool = AtomicBool::new(true);

/// On by default: the kernel runs wherever the first-person pose port runs. Off
/// leaves the register-level path, the tests' reference.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Why the kernel declined the step; the port's loops run it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// A guest read faulted.
    Fault,
    /// A count passed the gather bound.
    Count,
    /// A write overlaps a range the step reads.
    Alias,
    /// The direction flag is set: the guest's `movsd` copies then run
    /// downward from each record's first word, writing below it.
    Direction,
}

/// Call counts since start, for timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Steps the kernel filled.
    pub native: u64,
    /// Palette records it wrote.
    pub records: u64,
}

static NATIVE: AtomicU64 = AtomicU64::new(0);
static RECORDS: AtomicU64 = AtomicU64::new(0);

pub fn stats() -> Stats {
    Stats {
        native: NATIVE.load(Ordering::Relaxed),
        records: RECORDS.load(Ordering::Relaxed),
    }
}

type Step<T> = Result<T, Decline>;

fn read<T>(r: Result<T, GuestFault>) -> Step<T> {
    r.map_err(|_| Decline::Fault)
}

fn overlaps(a: (u32, u32), b: (u32, u32)) -> bool {
    a.0 < b.1 && b.0 < a.1
}

/// The address of a palette slot: the base plus 32 bytes a slot, with a
/// negative slot (a signed index) reaching below the base.
fn slot_at(base: u32, slot: i32) -> u32 {
    base.wrapping_add((slot as u32).wrapping_mul(palette::NODE_BYTES as u32))
}

/// One model: its node count, and the node array and palette index table
/// the step reads, gathered. A negative count gathers nothing (the
/// guest's loop then runs zero iterations); a count past the bound
/// declines the whole step.
struct Model {
    count: i32,
    nodes_at: u32,
    nodes: Vec<u8>,
    indices_at: u32,
    indices: Vec<u16>,
}

impl Model {
    fn gather(g: &impl Memory, at: u32, indices_at: u32) -> Step<Self> {
        let count = read(g.u32(at.wrapping_add(MODEL_COUNT)))? as i32;
        let mut model = Self {
            count,
            nodes_at: 0,
            nodes: Vec::new(),
            indices_at,
            indices: Vec::new(),
        };
        if count > NODES_MAX as i32 {
            return Err(Decline::Count);
        }
        if count <= 0 {
            return Ok(model);
        }
        model.nodes_at = read(g.u32(at.wrapping_add(MODEL_NODES)))?;
        model.nodes = vec![0u8; count as usize * palette::MODEL_NODE_BYTES];
        read(g.read_bytes(model.nodes_at, &mut model.nodes, 1))?;
        model.indices = vec![0u16; count as usize];
        for (i, slot) in model.indices.iter_mut().enumerate() {
            *slot = read(g.u16(indices_at.wrapping_add(4 * i as u32)))?;
        }
        Ok(model)
    }

    fn overlay(&self) -> Overlay<'_> {
        Overlay {
            nodes: &self.nodes,
            indices: &self.indices,
        }
    }

    /// Whether the step's loop over this model lands a node in the
    /// palette (and so loads the scale constant into `xmm0` and restores
    /// `edi`).
    fn writes_any(&self) -> bool {
        self.count > 0 && self.indices.iter().any(|&i| i != u16::MAX)
    }

    /// The node pointer the last iteration leaves in `ecx`.
    fn last_node(&self) -> u32 {
        self.nodes_at
            .wrapping_add(palette::MODEL_NODE_BYTES as u32 * (self.count - 1) as u32)
    }
}

/// What the scatter writes, decided before anything is written.
struct Plan {
    nodes: i32,
    base: u32,
    out: palette::Palette,
    models: [Model; 2],
    graph: u32,
    saved_edi: u32,
    model_two: u32,
    one: u32,
}

fn plan(c: &CpuAbi, g: &impl Memory) -> Step<Plan> {
    if c.eflags & 0x400 != 0 {
        return Err(Decline::Direction);
    }
    let frame = c.ebp;
    let above = |off: u32| read(g.u32(frame.wrapping_add(off)));
    let below = |off: u32| read(g.u32(frame.wrapping_sub(off)));
    let nodes = c.eax as i16 as i32;
    if nodes.unsigned_abs() as usize > NODES_MAX {
        return Err(Decline::Count);
    }
    // The palette base: `(a + 2 * b) << 12` over the seat's arguments,
    // over the table at `0x4e9bc4`.
    let (a, b) = (above(ARG_A)?, above(ARG_B)?);
    let base = read(g.u32(PALETTES))?.wrapping_add(a.wrapping_add(b.wrapping_mul(2)) << 12);
    let identity_at = read(g.u32(IDENTITY))?;
    let mut identity = [0u8; palette::NODE_BYTES];
    read(g.read_bytes(identity_at, &mut identity, 1))?;
    let one = read(g.u32(ONE))?;

    let model_one_at = below(MODEL_ONE)?;
    let model_two_at = below(MODEL_TWO)?;
    let models = [
        Model::gather(g, model_one_at, c.ebx.wrapping_add(INDICES[0]))?,
        Model::gather(g, model_two_at, c.ebx.wrapping_add(INDICES[1]))?,
    ];
    let overlays = [models[0].overlay(), models[1].overlay()];
    let out = palette::build(&identity, nodes, one, &overlays);

    // The writes: the palette span (which may reach below the base — an
    // index is a signed word), the node count, the base and counter
    // slots, and the pushed 8 below `esp`. None may overlap a read, and
    // no span may wrap.
    let first = slot_at(base, out.first);
    let writes = [
        (first, first.wrapping_add(out.bytes.len() as u32)),
        (
            c.ebx.wrapping_add(NODE_COUNT),
            c.ebx.wrapping_add(NODE_COUNT + 4),
        ),
        (
            frame.wrapping_sub(BASE_SLOT),
            frame.wrapping_sub(BASE_SLOT - 4),
        ),
        (
            frame.wrapping_add(COUNTER_SLOT),
            frame.wrapping_add(COUNTER_SLOT + 4),
        ),
        (c.esp.wrapping_sub(4), c.esp),
    ];
    let mut reads = vec![(
        identity_at,
        identity_at.wrapping_add(palette::NODE_BYTES as u32),
    )];
    for model in &models {
        if model.count > 0 {
            reads.push((
                model.nodes_at,
                model
                    .nodes_at
                    .wrapping_add(palette::MODEL_NODE_BYTES as u32 * model.count as u32),
            ));
            reads.push((
                model.indices_at,
                model.indices_at.wrapping_add(4 * model.count as u32),
            ));
        }
    }
    for (n, w) in writes.iter().enumerate() {
        if w.1 < w.0
            || writes[n + 1..].iter().any(|other| overlaps(*w, *other))
            || reads.iter().any(|r| overlaps(*w, *r))
        {
            return Err(Decline::Alias);
        }
    }
    if reads.iter().any(|r| r.1 < r.0) {
        return Err(Decline::Alias);
    }
    Ok(Plan {
        nodes,
        base,
        out,
        models,
        graph: below(GRAPH)?,
        saved_edi: below(SAVED_EDI)?,
        model_two: model_two_at,
        one,
    })
}

fn scatter(c: &mut CpuAbi, g: &mut impl Memory, p: &Plan) -> Result<TransferAbi, GuestFault> {
    let frame = c.ebp;
    let first = slot_at(p.base, p.out.first);
    g.write_bytes(first, &p.out.bytes, 1)?;
    g.set_u32(c.ebx.wrapping_add(NODE_COUNT), p.nodes as u32)?;
    g.set_u32(frame.wrapping_sub(BASE_SLOT), p.base)?;
    g.set_u32(
        frame.wrapping_add(COUNTER_SLOT),
        p.models[1].count.max(0) as u32,
    )?;
    if p.nodes > 0 {
        // The fill's `push 8` / `pop ecx` pair leaves the 8 below `esp`.
        g.set_u32(c.esp.wrapping_sub(4), 8)?;
    }

    // The registers the loops leave. `esi` is the second model, read
    // before its loop whatever it does; `edi` is the saved value when
    // the fill ran or either loop landed a node (every record branch
    // restores it).
    c.eax = p.graph;
    c.esi = p.model_two;
    if p.nodes > 0 || p.models[0].writes_any() || p.models[1].writes_any() {
        c.edi = p.saved_edi;
    }
    // `ecx`: the node count, zeroed by the fill's `rep movsd`, then the
    // last node pointer of each loop that ran.
    let mut ecx = p.nodes as u32;
    if p.nodes > 0 {
        ecx = 0;
    }
    if p.models[0].count > 0 {
        ecx = p.models[0].last_node();
    }
    if p.models[1].count > 0 {
        ecx = p.models[1].last_node();
    }
    c.ecx = ecx;
    // `edx` and the flags: the second loop's last compare, or its
    // entry compare against zero when it never ran.
    let count = p.models[1].count;
    if count > 0 {
        c.edx = count as u32;
        crate::sim_physics::arithmetic(c, count as u32, count as u32, true);
    } else {
        c.edx = 0;
        crate::sim_physics::arithmetic(c, count as u32, 0, true);
    }
    // `xmm0`: the scale constant, loaded by whichever loop landed a node.
    if p.models[0].writes_any() || p.models[1].writes_any() {
        let mut lanes = [0u8; 16];
        lanes[..4].copy_from_slice(&p.one.to_le_bytes());
        c.xmm[0] = lanes;
    }
    NATIVE.fetch_add(1, Ordering::Relaxed);
    RECORDS.fetch_add(
        (p.out.bytes.len() / palette::NODE_BYTES) as u64,
        Ordering::Relaxed,
    );
    // The call that ends the step: the return address pushed, the
    // transfer made, as `Guest::call` does.
    let sp = c.esp.wrapping_sub(4);
    g.set_u32(sp, AFTER_MATRICES)?;
    c.esp = sp;
    Ok(TransferAbi::call(BUILD_MATRICES, AFTER_MATRICES))
}

/// The palette step, entered where the port runs it. `Ok(None)`:
/// declined, nothing written; the port's loops run.
pub fn run(c: &mut CpuAbi, g: &mut impl Memory) -> Result<Option<TransferAbi>, GuestFault> {
    if !enabled() {
        return Ok(None);
    }
    let Ok(plan) = plan(c, g) else {
        return Ok(None);
    };
    scatter(c, g, &plan).map(Some)
}

#[cfg(test)]
mod tests;
