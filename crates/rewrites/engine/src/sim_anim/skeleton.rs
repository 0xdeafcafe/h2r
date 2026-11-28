//! The guest side of `sim_anim::build_matrices`: the skeleton walk of
//! `bd090` as one gather, one kernel call and one scatter
//! (docs/engine/skeleton-walk.md; phase 1b of docs/engine/sim-rewrite.md).
//!
//! `bd090` builds the root matrix, calls the after-root dispatcher, and
//! then, for nodes 1 to `count - 1`, calls the pose matrix `1421f0` and
//! the composition `142a60` for every node its stack mask does not keep.
//! The register-level port (`sim_physics::node_matrices`) runs those two
//! leaves inline, one node at a time, through guest memory. [`run`] takes
//! the whole loop instead, at the point the port enters it: it reads the
//! skeleton, the node records, the mask and the matrices once, lets the
//! kernel compute every matrix in Rust, and writes back what the loop
//! writes: the matrices of the posed nodes, the cutoff the first pose
//! initialises, the dead stack the last node's two calls leave, the XMM
//! registers the last composition leaves, and the loop's exit registers.
//!
//! The kernel runs under the guest's MXCSR, whatever it holds
//! ([`sim_anim::build_matrices_guest`]), and the walk leaves the MXCSR
//! with every flag the loop's SSE code raises.
//!
//! It declines before writing anything (the port's loop runs instead) when
//! an operation raises an exception the MXCSR unmasks, when the node
//! count is outside `2..=MAX_NODES`, when a range it reads or writes has
//! an unreadable page or a written range overlaps a read one (physically,
//! so aliased mappings count), when a read faults, or when a parent index
//! is outside the matrices.
use crate::function_eval::constant::{ONE, SMALL};
use crate::sim_native::pose::{POSE_INIT_FLAGS, POSE_NORM_THRESHOLD};
use ::sim_anim::{Constants, Record, Walk, build_matrices_guest};
use runtime::{CpuAbi, Guest, GuestFault};
use sim_core::lanes::Vec4;
use sim_core::math::NodeMatrix;
use crate::sim_native::spans::Spans;
use std::sync::atomic::{AtomicU64, Ordering};

/// The pose matrix's sign mask (16 bytes).
pub const SIGNS: u32 = 0x0045_3750;
/// The return addresses the loop's two calls push.
pub const POSE_RESUME: u32 = 0x000b_d55e;
pub const COMPOSE_RESUME: u32 = 0x000b_d571;
/// `bd090`'s frame at the loop, from `esp`: the skeleton definition, the
/// orientation block, the node matrices, and the keep mask.
pub const DEFINITION_SLOT: u32 = 0x14;
pub const ORIENTATIONS_SLOT: u32 = 0x18;
pub const MATRICES_SLOT: u32 = 0x1c;
pub const KEEP_SLOT: u32 = 0x70;
/// The skeleton definition's node count and node array.
pub const NODE_COUNT: u32 = 0x48;
pub const NODES: u32 = 0x4c;
/// The parent index (`i16`) in a 0x60-byte node.
pub const PARENT: u32 = 4;
const MATRIX_BYTES: u32 = 0x34;
const NODE_BYTES: u32 = 0x60;
const RECORD_BYTES: u32 = 0x20;
/// The dead stack below the loop's `esp`: the pose call's two arguments,
/// its return address and its saved `ebp`; the composition's argument and
/// return address land on the first two.
const DEAD: u32 = 16;
/// The largest node count the walk takes; larger ones run the loop.
pub const MAX_NODES: u32 = 1024;

/// Why a walk was left to the loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// An operation raised an SSE exception the MXCSR unmasks.
    Float,
    /// A guest read faulted.
    Fault,
    /// The node count is below 2 or above [`MAX_NODES`].
    Count,
    /// A range has an unreadable page, or a written range overlaps
    /// another range the walk reads or writes.
    Alias,
    /// A posed node's parent is outside the matrices.
    Parent,
}

/// Counts since start, for timing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Walks the kernel ran.
    pub native: u64,
    /// Nodes it posed.
    pub nodes: u64,
}

static NATIVE: AtomicU64 = AtomicU64::new(0);
static POSED: AtomicU64 = AtomicU64::new(0);

pub fn stats() -> Stats {
    Stats {
        native: NATIVE.load(Ordering::Relaxed),
        nodes: POSED.load(Ordering::Relaxed),
    }
}

type Step<T> = Result<T, Decline>;

fn read<T>(r: Result<T, GuestFault>) -> Step<T> {
    r.map_err(|_| Decline::Fault)
}

/// `len` bytes from `start` as physical pieces, one lookup per page
/// (`sim_native::spans`).
fn physical(g: &Guest, start: u32, len: u32) -> Step<Spans> {
    Spans::of(g, start, len as usize).ok_or(Decline::Alias)
}

fn overlaps(a: &Spans, b: &Spans) -> bool {
    a.overlaps(b)
}

fn words(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes
        .chunks_exact(4)
        .map(|w| u32::from_le_bytes(w.try_into().unwrap()))
}

fn lanes(x: &[u8; 16]) -> Vec4 {
    std::array::from_fn(|i| u32::from_le_bytes(x[4 * i..4 * i + 4].try_into().unwrap()))
}

fn unlanes(v: Vec4) -> [u8; 16] {
    let mut out = [0; 16];
    for (chunk, w) in out.chunks_exact_mut(4).zip(v) {
        chunk.copy_from_slice(&w.to_le_bytes());
    }
    out
}

/// Everything the scatter needs, decided before anything is written.
struct Plan {
    definition: u32,
    nodes: u32,
    orientations: u32,
    matrices: u32,
    count: u32,
    /// `[0x55ebb4]` bit 0 was set: the cutoff is initialised.
    initialised: bool,
    threshold: u32,
    keep: Vec<u32>,
    out: Vec<NodeMatrix>,
    walk: Walk,
    /// The MXCSR after the walk.
    mxcsr: u32,
}

impl Plan {
    fn kept(&self, i: usize) -> bool {
        self.keep[i >> 5] >> (i & 31) & 1 != 0
    }
}

fn plan(c: &CpuAbi, g: &Guest) -> Step<Plan> {
    let sp = c.esp;
    let slot = |off: u32| read(g.u32(sp.wrapping_add(off)));
    let (definition, orientations, matrices) =
        (slot(DEFINITION_SLOT)?, slot(ORIENTATIONS_SLOT)?, slot(MATRICES_SLOT)?);
    let count = read(g.u32(definition.wrapping_add(NODE_COUNT)))?;
    if !(2..=MAX_NODES).contains(&count) {
        return Err(Decline::Count);
    }
    let nodes = read(g.u32(definition.wrapping_add(NODES)))?;
    let mask_words = count.div_ceil(32);

    // Every range in physical addresses. The matrices are read and written
    // (the kernel models that in place); the rest is one or the other.
    let writes = [
        physical(g, matrices, MATRIX_BYTES * count)?,
        physical(g, sp.wrapping_sub(DEAD), DEAD)?,
        physical(g, POSE_INIT_FLAGS, 4)?,
        physical(g, POSE_NORM_THRESHOLD, 4)?,
    ];
    let reads = [
        physical(g, orientations, RECORD_BYTES * count)?,
        physical(g, definition.wrapping_add(NODE_COUNT), 8)?,
        physical(g, nodes, NODE_BYTES * count)?,
        physical(g, sp.wrapping_add(DEFINITION_SLOT), 12)?,
        physical(g, sp.wrapping_add(KEEP_SLOT), 4 * mask_words)?,
        physical(g, SMALL, 4)?,
        physical(g, ONE, 4)?,
        physical(g, SIGNS, 16)?,
    ];
    for (i, w) in writes.iter().enumerate() {
        if writes[i + 1..].iter().chain(&reads).any(|r| overlaps(w, r)) {
            return Err(Decline::Alias);
        }
    }

    let n = count as usize;
    let mut bytes = vec![0u8; RECORD_BYTES as usize * n];
    read(g.read_bytes(orientations, &mut bytes, 1))?;
    let locals: Vec<Record> = bytes
        .chunks_exact(RECORD_BYTES as usize)
        .map(|r| {
            let mut out = [0; 8];
            for (o, w) in out.iter_mut().zip(words(r)) {
                *o = w;
            }
            out
        })
        .collect();
    let parents = (0..count)
        .map(|i| read(g.u16(nodes.wrapping_add(NODE_BYTES * i + PARENT))).map(|p| p as i16))
        .collect::<Step<Vec<i16>>>()?;
    let mut keep_bytes = vec![0u8; 4 * mask_words as usize];
    read(g.read_bytes(sp.wrapping_add(KEEP_SLOT), &mut keep_bytes, 1))?;
    let keep: Vec<u32> = words(&keep_bytes).collect();
    let mut matrix_bytes = vec![0u8; MATRIX_BYTES as usize * n];
    read(g.read_bytes(matrices, &mut matrix_bytes, 1))?;
    let mut out: Vec<NodeMatrix> = matrix_bytes
        .chunks_exact(MATRIX_BYTES as usize)
        .map(|m| NodeMatrix::from_bytes(m.try_into().unwrap()))
        .collect();

    let initialised = read(g.u8(POSE_INIT_FLAGS))? & 1 != 0;
    let threshold = read(g.u32(if initialised { POSE_NORM_THRESHOLD } else { SMALL }))?;
    let k = Constants {
        one: read(g.u32(ONE))?,
        threshold,
        signs: lanes(&read(g.u128(SIGNS))?.to_le_bytes()),
    };
    let xmm: [Vec4; 8] = std::array::from_fn(|i| lanes(&c.xmm[i]));
    let walk = build_matrices_guest(c.mxcsr, &k, &locals, &parents, &keep, xmm, &mut out);
    let (walk, mxcsr) = walk.map_err(|_| {
        // The kernel declines on a parent outside the matrices or on an
        // unmasked exception; tell the two apart.
        let bad_parent = (1..n).any(|i| {
            let posed = keep[i >> 5] >> (i & 31) & 1 == 0;
            posed && !usize::try_from(parents[i]).is_ok_and(|p| p < n)
        });
        if bad_parent { Decline::Parent } else { Decline::Float }
    })?;
    Ok(Plan {
        definition,
        nodes,
        orientations,
        matrices,
        count,
        initialised,
        threshold,
        keep,
        out,
        walk,
        mxcsr,
    })
}

/// The loop's own writes, from the plan.
fn scatter(c: &mut CpuAbi, g: &mut Guest, p: &Plan) -> Result<(), GuestFault> {
    if let Some(last) = p.walk.last {
        if !p.initialised {
            let flags = g.u32(POSE_INIT_FLAGS)?;
            g.set_u32(POSE_INIT_FLAGS, flags | 1)?;
            g.set_u32(POSE_NORM_THRESHOLD, p.threshold)?;
        }
        for i in 1..p.count as usize {
            if !p.kept(i) {
                let at = p.matrices.wrapping_add(MATRIX_BYTES * i as u32);
                g.write_bytes(at, &p.out[i].to_bytes(), 1)?;
            }
        }
        let k = last as u32;
        let sp = c.esp;
        let record = p.orientations.wrapping_add(RECORD_BYTES * k);
        let matrix = p.matrices.wrapping_add(MATRIX_BYTES * k);
        // Pose: `push ebp` (the record), `push edx` (the matrix), the
        // call, then its own `push ebp`. Composition: `push edx` and the
        // call, over the first two.
        g.set_u32(sp.wrapping_sub(16), record)?;
        g.set_u32(sp.wrapping_sub(12), POSE_RESUME)?;
        g.set_u32(sp.wrapping_sub(8), COMPOSE_RESUME)?;
        g.set_u32(sp.wrapping_sub(4), matrix)?;
        for (reg, v) in c.xmm.iter_mut().zip(p.walk.xmm) {
            *reg = unlanes(v);
        }
    }
    c.mxcsr = p.mxcsr;
    // The loop's registers at its exit: the counters one past the last
    // node, `edi` at the last node, `eax` and `ecx` reloaded.
    let n = p.count;
    c.esi = n;
    c.ebx = NODE_BYTES.wrapping_mul(n);
    c.ebp = p.orientations.wrapping_add(RECORD_BYTES.wrapping_mul(n));
    c.edx = p.matrices.wrapping_add(MATRIX_BYTES.wrapping_mul(n));
    c.edi = p.nodes.wrapping_add(NODE_BYTES.wrapping_mul(n - 1));
    c.eax = p.definition;
    c.ecx = n;
    Ok(())
}

/// The skeleton walk, entered where the loop starts (`esi` 1, the frame
/// slots set). `Ok(false)`: declined, nothing written; the caller runs
/// the loop. A fault in the scatter is the guest's own: every address it
/// writes was read or checked first, so only a read-only page gets there.
pub fn walk(c: &mut CpuAbi, g: &mut Guest) -> Result<bool, GuestFault> {
    debug_assert_eq!(c.esi, 1, "the walk starts at node 1");
    let Ok(p) = plan(c, g) else {
        return Ok(false);
    };
    scatter(c, g, &p)?;
    NATIVE.fetch_add(1, Ordering::Relaxed);
    let posed = (1..p.count as usize).filter(|&i| !p.kept(i)).count();
    POSED.fetch_add(posed as u64, Ordering::Relaxed);
    Ok(true)
}

#[cfg(test)]
mod tests;
