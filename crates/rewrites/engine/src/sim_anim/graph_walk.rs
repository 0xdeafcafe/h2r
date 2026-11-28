//! The guest side of `sim_anim::graph_matrices`: the animation graph's node
//! walk `1dd1c0` as one gather, one kernel call and one scatter
//! (docs/engine/skeleton-walk.md, "The graph walk `1dd1c0`"; phase 1b of
//! docs/engine/sim-rewrite.md).
//!
//! `1dd1c0(graph, matrices, records, root)` (stdcall, `ret 0x10`) poses
//! every node it reaches from node 0 through the graph's child and sibling
//! links, in queue order, into a scratch matrix in its frame (`1421f0`),
//! and composes that with the parent's matrix, or `root` for node 0, into
//! the node's slot (`142a60`).
//!
//! [`plan`] reads everything once and lets the kernel compute the call's
//! whole result without writing: the matrices, the cutoff the first pose
//! initialises, every byte of the dead frame (saved registers, the queue
//! and its length, the scratch matrix, the leaves' arguments and return
//! addresses) and the registers at the return. The **body** ([`run`])
//! writes the plan and the call returns without the guest; it answers
//! every call it can plan. Tests and the differential hold it equal to
//! the guest, and [`compare`] names the first difference between a plan
//! and a guest result.
//!
//! [`reference`] is the same function as a register-level port through the
//! exact `sim_native` leaves, one node at a time: the oracle of the engine
//! tests, and the third arm of the lifted differential.
//!
//! The plan declines (nothing is written; [`run`] returns the
//! [`Decline`] and the hook panics with its name) when MXCSR is not
//! masked, nearest and PE-sticky, when an operand is off the fast path,
//! when the node count is above [`MAX_NODES`], when a queued link or a
//! visited node's parent is outside the nodes, when the queue would pass
//! 255 entries, when a range it reads or writes is not inside one mapping
//! or a written range overlaps another range, or when a read faults.
use crate::function_eval::constant::{ONE, SMALL};
use crate::sim_anim::skeleton::SIGNS;
use crate::sim_native::pose::{POSE_INIT_FLAGS, POSE_NORM_THRESHOLD};
use crate::sim_native::{self, Memory};
use crate::sim_physics::{arithmetic, pop, push};
use ::sim_anim::{Constants, GraphNode, Record, graph_matrices};
use runtime::{CpuAbi, GuestFault, TransferAbi};
use sim_core::det;
use sim_core::lanes::Vec4;
use sim_core::math::NodeMatrix;
use crate::sim_native::spans::Spans;
use std::sync::atomic::{AtomicU64, Ordering};

/// The walk's entry.
pub const ENTRY: u32 = 0x001d_d1c0;
/// The return addresses its two calls push.
pub const POSE_RESUME: u32 = 0x001d_d230;
pub const COMPOSE_RESUME: u32 = 0x001d_d241;
/// Bytes the arguments take: graph, matrices, records, root.
pub const ARGS: u32 = 0x10;
/// The graph's node count (`i32`) and node array.
pub const NODE_COUNT: u32 = 0x0c;
pub const NODES: u32 = 0x10;
/// A graph node's links: first child, next sibling, parent (`i16` each).
pub const FIRST_CHILD: u32 = 4;
pub const NEXT_SIBLING: u32 = 6;
pub const PARENT: u32 = 8;
const NODE_BYTES: u32 = 0x20;
const RECORD_BYTES: u32 = 0x20;
const MATRIX_BYTES: u32 = 0x34;
/// The frame below the entry `esp`, as offsets subtracted from it: the
/// locals (`sub esp, 0x434`), then `ebx`, `ebp`, `esi` and `edi` saved.
pub const FRAME: u32 = 0x434;
/// The queue length, the lowest local.
pub const LENGTH: u32 = 0x434;
/// The scratch matrix the pose writes.
pub const SCRATCH: u32 = 0x430;
/// The queue, `u32` node indices upwards from here.
pub const QUEUE: u32 = 0x3fc;
const SAVED_EBX: u32 = 0x438;
const SAVED_EBP: u32 = 0x43c;
const SAVED_ESI: u32 = 0x440;
const SAVED_EDI: u32 = 0x444;
/// The leaves' arguments, return addresses and the pose's saved `ebp`,
/// below the saved registers: the lowest byte the call writes.
const DEAD: u32 = 0x454;
/// The largest node count the plan takes; larger ones decline.
pub const MAX_NODES: u32 = 1024;

/// Why the plan declined a call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decline {
    /// MXCSR not masked, nearest and PE-sticky, or an operand off the
    /// fast path.
    Float = 0,
    /// A guest read faulted.
    Fault,
    /// The node count is above [`MAX_NODES`].
    Count,
    /// A queued link is outside the nodes, or the queue would pass 255
    /// entries.
    Queue,
    /// A range has an unreadable page, or a written range overlaps
    /// another range the call reads or writes.
    Alias,
    /// A visited node's parent is outside the nodes.
    Parent,
}

impl Decline {
    pub fn name(self) -> &'static str {
        match self {
            Decline::Float => "float",
            Decline::Fault => "fault",
            Decline::Count => "count",
            Decline::Queue => "queue",
            Decline::Alias => "alias",
            Decline::Parent => "parent",
        }
    }
}

type Planned<T> = Result<T, Decline>;

fn read<T>(r: Result<T, GuestFault>) -> Planned<T> {
    r.map_err(|_| Decline::Fault)
}

fn at(a: u32, b: u32) -> u32 {
    a.wrapping_add(b)
}

fn below(sp: u32, off: u32) -> u32 {
    sp.wrapping_sub(off)
}

/// `len` bytes from `start` as physical pieces, one lookup per page
/// (`sim_native::spans`).
fn span(g: &impl Memory, start: u32, len: u32) -> Planned<Spans> {
    Spans::of(g, start, len as usize).ok_or(Decline::Alias)
}

fn overlaps(a: &Spans, b: &Spans) -> bool {
    a.overlaps(b)
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

fn bytes_of(words: &[u32]) -> Vec<u8> {
    words.iter().flat_map(|w| w.to_le_bytes()).collect()
}

/// What one call leaves, decided before anything is written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    /// The entry `esp` (the return address's slot).
    pub entry: u32,
    pub matrices: u32,
    pub count: u32,
    /// Every byte the call writes, as `(address, bytes)` runs in the order
    /// [`run`] writes them. Later runs never overlap earlier ones.
    pub writes: Vec<(u32, Vec<u8>)>,
    /// The registers at the return: `esp` past the arguments, `eip` the
    /// return address.
    pub cpu: CpuAbi,
    /// Nodes posed (the queue length).
    pub posed: u32,
}

impl Outcome {
    /// What the byte at `addr` is, for a mismatch report.
    pub fn describe(&self, addr: u32) -> String {
        let matrices = self.matrices..self.matrices.wrapping_add(MATRIX_BYTES * self.count);
        if matrices.contains(&addr) {
            let off = addr - self.matrices;
            return format!("matrix {} +{:#x}", off / MATRIX_BYTES, off % MATRIX_BYTES);
        }
        match addr {
            POSE_INIT_FLAGS..=0x0055_ebb7 => return "pose cutoff flag".into(),
            POSE_NORM_THRESHOLD..=0x0055_ebb3 => return "pose cutoff".into(),
            _ => {}
        }
        let depth = self.entry.wrapping_sub(addr);
        let part = match depth {
            1..=QUEUE => format!("queue entry {}", (QUEUE - depth) / 4),
            0x3fd..=SCRATCH => format!("scratch matrix +{:#x}", SCRATCH - depth),
            0x431..=LENGTH => "queue length".into(),
            0x435..=SAVED_EBX => "saved ebx".into(),
            0x439..=SAVED_EBP => "saved ebp".into(),
            0x43d..=SAVED_ESI => "saved esi".into(),
            0x441..=SAVED_EDI => "saved edi".into(),
            0x445..=0x448 => "last output argument".into(),
            0x449..=0x44c => "composition return address".into(),
            0x44d..=0x450 => "pose return address".into(),
            0x451..=DEAD => "pose saved ebp".into(),
            _ => return format!("{addr:#010x}"),
        };
        format!("stack entry-{depth:#x} ({part})")
    }
}

/// The call's result without writing anything. `c` is the CPU at the
/// entry (`esp` at the return address).
pub fn plan(c: &CpuAbi, g: &impl Memory) -> Result<Outcome, Decline> {
    if !det::fast_env(c.mxcsr) {
        return Err(Decline::Float);
    }
    let sp = c.esp;
    let arg = |k: u32| read(g.u32(at(sp, 4 * k)));
    let (target, graph, matrices, records, root) = (arg(0)?, arg(1)?, arg(2)?, arg(3)?, arg(4)?);
    let count = read(g.u32(at(graph, NODE_COUNT)))?;
    let mut out = CpuAbi { eip: target, ..*c };
    // `add esp, 0x434` after the pops: the flags the caller receives.
    let low = below(sp, FRAME);
    let after = arithmetic(&mut out, low, FRAME, false);
    debug_assert_eq!(after, sp);
    out.esp = at(sp, 4 + ARGS);
    let mut writes = vec![(below(sp, SAVED_EBP), bytes_of(&[c.ebp, c.ebx]))];
    if count as i32 <= 0 {
        // `jle` past the loop: only `ebx` and `ebp` were saved.
        span(g, below(sp, SAVED_EBP), 8)?;
        out.eax = graph;
        out.ecx = count;
        return Ok(Outcome {
            entry: sp,
            matrices,
            count: 0,
            writes,
            cpu: out,
            posed: 0,
        });
    }
    if count > MAX_NODES {
        return Err(Decline::Count);
    }
    let nodes_at = read(g.u32(at(graph, NODES)))?;

    let writes_spans = [
        span(g, matrices, MATRIX_BYTES * count)?,
        span(g, below(sp, DEAD), DEAD)?,
        span(g, POSE_INIT_FLAGS, 4)?,
        span(g, POSE_NORM_THRESHOLD, 4)?,
    ];
    let reads = [
        span(g, sp, 4 + ARGS)?,
        span(g, at(graph, NODE_COUNT), 8)?,
        span(g, nodes_at, NODE_BYTES * count)?,
        span(g, records, RECORD_BYTES * count)?,
        span(g, root, MATRIX_BYTES)?,
        span(g, SMALL, 4)?,
        span(g, ONE, 4)?,
        span(g, SIGNS, 16)?,
    ];
    for (i, w) in writes_spans.iter().enumerate() {
        if writes_spans[i + 1..]
            .iter()
            .chain(&reads)
            .any(|r| overlaps(w, r))
        {
            return Err(Decline::Alias);
        }
    }

    let n = count as usize;
    let mut raw = vec![0u8; (NODE_BYTES * count) as usize];
    read(g.read_bytes(nodes_at, &mut raw, 1))?;
    let half = |b: &[u8], off: u32| i16::from_le_bytes([b[off as usize], b[off as usize + 1]]);
    let nodes: Vec<GraphNode> = raw
        .chunks_exact(NODE_BYTES as usize)
        .map(|b| GraphNode {
            first_child: half(b, FIRST_CHILD),
            next_sibling: half(b, NEXT_SIBLING),
            parent: half(b, PARENT),
        })
        .collect();
    let order = ::sim_anim::graph_order(&nodes).map_err(|_| Decline::Queue)?;
    let mut raw = vec![0u8; (RECORD_BYTES * count) as usize];
    read(g.read_bytes(records, &mut raw, 1))?;
    let locals: Vec<Record> = raw
        .chunks_exact(RECORD_BYTES as usize)
        .map(|r| {
            std::array::from_fn(|i| u32::from_le_bytes(r[4 * i..4 * i + 4].try_into().unwrap()))
        })
        .collect();
    let mut raw = vec![0u8; (MATRIX_BYTES * count) as usize];
    read(g.read_bytes(matrices, &mut raw, 1))?;
    let mut mats: Vec<NodeMatrix> = raw
        .chunks_exact(MATRIX_BYTES as usize)
        .map(|m| NodeMatrix::from_bytes(m.try_into().unwrap()))
        .collect();
    let root_matrix = NodeMatrix::from_bytes(&read(g.load::<0x34>(root))?);

    let initialised = read(g.u8(POSE_INIT_FLAGS))? & 1 != 0;
    let threshold = read(g.u32(if initialised {
        POSE_NORM_THRESHOLD
    } else {
        SMALL
    }))?;
    let k = Constants {
        one: read(g.u32(ONE))?,
        threshold,
        signs: lanes(&read(g.u128(SIGNS))?.to_le_bytes()),
    };
    let xmm: [Vec4; 8] = std::array::from_fn(|i| lanes(&c.xmm[i]));
    let walk = graph_matrices(&k, &locals, &nodes, &root_matrix, xmm, &mut mats).map_err(|_| {
        let bad_parent = order
            .iter()
            .any(|&v| v != 0 && !usize::try_from(nodes[v].parent).is_ok_and(|p| p < n));
        if bad_parent {
            Decline::Parent
        } else {
            Decline::Float
        }
    })?;

    // The scatter, in the order `run` writes it.
    if !initialised {
        let flags = read(g.u32(POSE_INIT_FLAGS))?;
        writes.push((POSE_INIT_FLAGS, (flags | 1).to_le_bytes().to_vec()));
        writes.push((POSE_NORM_THRESHOLD, threshold.to_le_bytes().to_vec()));
    }
    let mut visited = vec![false; n];
    for &v in &walk.queue {
        visited[v] = true;
    }
    for (i, m) in mats.iter().enumerate().filter(|&(i, _)| visited[i]) {
        writes.push((at(matrices, MATRIX_BYTES * i as u32), m.to_bytes().to_vec()));
    }
    let last = *walk.queue.last().expect("the queue starts with node 0");
    let length = walk.queue.len() as u32;
    let queue: Vec<u32> = walk.queue.iter().map(|&v| v as u32).collect();
    writes.push((below(sp, QUEUE), bytes_of(&queue)));
    writes.push((below(sp, SCRATCH), walk.posed.to_bytes().to_vec()));
    writes.push((below(sp, LENGTH), length.to_le_bytes().to_vec()));
    writes.push((below(sp, SAVED_EDI), bytes_of(&[c.edi, c.esi])));
    let output = at(matrices, MATRIX_BYTES * last as u32);
    writes.push((
        below(sp, DEAD),
        bytes_of(&[matrices, POSE_RESUME, COMPOSE_RESUME, output]),
    ));

    let parent = if last == 0 {
        root
    } else {
        at(
            matrices,
            MATRIX_BYTES.wrapping_mul(nodes[last].parent as i32 as u32),
        )
    };
    let node = nodes[last];
    out.eax = length;
    out.ecx = if node.first_child == ::sim_anim::graph_walk::NONE {
        parent
    } else {
        node.first_child as i32 as u32
    };
    out.edx = if node.next_sibling == ::sim_anim::graph_walk::NONE {
        below(sp, SCRATCH)
    } else {
        node.next_sibling as i32 as u32
    };
    for (reg, v) in out.xmm.iter_mut().zip(walk.xmm) {
        *reg = unlanes(v);
    }
    Ok(Outcome {
        entry: sp,
        matrices,
        count,
        writes,
        cpu: out,
        posed: length,
    })
}

/// Calls taken.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Calls [`run`] answered.
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

/// The body: the call answered from [`plan`] without the guest.
/// `Ok(Err(why))`: declined, nothing written; the hook panics with
/// `why`. A fault while writing is the guest's own: every address written
/// was checked readable, so only a read-only page gets there.
pub fn run(c: &mut CpuAbi, g: &mut impl Memory) -> Result<Result<TransferAbi, Decline>, GuestFault> {
    match plan(c, g) {
        Ok(o) => apply(o, c, g).map(Ok),
        Err(why) => Ok(Err(why)),
    }
}

/// Write a plan's result into the guest and the registers, and count the
/// call as native.
fn apply(o: Outcome, c: &mut CpuAbi, g: &mut impl Memory) -> Result<TransferAbi, GuestFault> {
    for (addr, bytes) in &o.writes {
        g.write_bytes(*addr, bytes, 1)?;
    }
    // The transfer carries the return address; `eip` stays as the
    // register-level bodies leave it.
    *c = CpuAbi {
        eip: c.eip,
        ..o.cpu
    };
    NATIVE.fetch_add(1, Ordering::Relaxed);
    POSED.fetch_add(u64::from(o.posed), Ordering::Relaxed);
    Ok(TransferAbi::ret(o.cpu.eip))
}

/// The first place a plan and the guest's result differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    /// The register or byte, by name ([`Outcome::describe`]).
    pub what: String,
    /// The plan's value and the guest's, in hex.
    pub plan: String,
    pub guest: String,
}

/// The first difference between `o` and the state the guest left: `c` at
/// the return and `g`. Registers first, in a fixed order, then the
/// written bytes in write order. EFLAGS compare on the arithmetic bits
/// both sides define.
pub fn compare(o: &Outcome, c: &CpuAbi, g: &impl Memory) -> Option<Mismatch> {
    let p = &o.cpu;
    let hex = |v: u32| format!("{v:#010x}");
    let gprs = [
        ("eax", p.eax, c.eax),
        ("ecx", p.ecx, c.ecx),
        ("edx", p.edx, c.edx),
        ("ebx", p.ebx, c.ebx),
        ("esp", p.esp, c.esp),
        ("ebp", p.ebp, c.ebp),
        ("esi", p.esi, c.esi),
        ("edi", p.edi, c.edi),
        ("mxcsr", p.mxcsr, c.mxcsr),
    ];
    for (name, want, got) in gprs {
        if want != got {
            return Some(Mismatch {
                what: name.into(),
                plan: hex(want),
                guest: hex(got),
            });
        }
    }
    let defined = p.eflags_defined & c.eflags_defined & crate::sim_physics::ARITH;
    if (p.eflags ^ c.eflags) & defined != 0 {
        return Some(Mismatch {
            what: "eflags".into(),
            plan: hex(p.eflags & defined),
            guest: hex(c.eflags & defined),
        });
    }
    for (i, (want, got)) in p.xmm.iter().zip(&c.xmm).enumerate() {
        if want != got {
            return Some(Mismatch {
                what: format!("xmm{i}"),
                plan: format!("{:#034x}", u128::from_le_bytes(*want)),
                guest: format!("{:#034x}", u128::from_le_bytes(*got)),
            });
        }
    }
    for (addr, bytes) in &o.writes {
        let mut have = vec![0u8; bytes.len()];
        if g.read_bytes(*addr, &mut have, 1).is_err() {
            return Some(Mismatch {
                what: o.describe(*addr),
                plan: "written".into(),
                guest: "unreadable".into(),
            });
        }
        if let Some(k) = have.iter().zip(bytes).position(|(a, b)| a != b) {
            let a = at(*addr, k as u32);
            return Some(Mismatch {
                what: o.describe(a),
                plan: format!("{:#04x}", bytes[k]),
                guest: format!("{:#04x}", have[k]),
            });
        }
    }
    None
}

/// `call 1421f0` after the caller pushed the record and the scratch.
fn pose_leaf(c: &mut CpuAbi, g: &mut impl Memory) -> Result<(), GuestFault> {
    push(c, g, POSE_RESUME)?;
    let back = sim_native::pose_matrix(c, g)?;
    debug_assert_eq!(back.target_pc, POSE_RESUME);
    Ok(())
}

/// `call 142a60` with the parent in ECX, the child in EDX and the output
/// pushed.
fn compose_leaf(c: &mut CpuAbi, g: &mut impl Memory) -> Result<(), GuestFault> {
    push(c, g, COMPOSE_RESUME)?;
    let back = sim_native::compose(c, g)?;
    debug_assert_eq!(back.target_pc, COMPOSE_RESUME);
    Ok(())
}

fn ld(g: &impl Memory, a: u32) -> Result<u32, GuestFault> {
    g.u32(a)
}

fn st(g: &mut impl Memory, a: u32, v: u32) -> Result<(), GuestFault> {
    g.set_u32(a, v)
}

/// `1dd1c0` instruction by instruction, with the two leaves run by the
/// exact `sim_native` bodies. The flags inside the loop are not kept: the
/// epilogue's `add esp` replaces them before the return.
pub fn reference(c: &mut CpuAbi, g: &mut impl Memory) -> sim_native::Result {
    c.eax = ld(&*g, at(c.esp, 4))?;
    c.ecx = ld(&*g, at(c.eax, NODE_COUNT))?;
    c.esp = below(c.esp, FRAME);
    push(c, g, c.ebx)?;
    c.ebx = 0;
    push(c, g, c.ebp)?;
    c.ebp = ld(&*g, at(c.esp, 0x444))?;
    if c.ecx as i32 > 0 {
        push(c, g, c.esi)?;
        st(g, at(c.esp, 0xc), 1)?;
        st(g, at(c.esp, 0x44), c.ebx)?;
        push(c, g, c.edi)?;
        loop {
            c.edx = ld(&*g, at(c.esp, c.ebx.wrapping_mul(4).wrapping_add(0x48)))?;
            c.ecx = ld(&*g, at(c.esp, 0x448))?;
            c.esi = ld(&*g, at(c.ecx, NODES))?;
            c.eax = c.edx << 5;
            c.ebx = c.ebx.wrapping_add(1);
            c.esi = at(c.esi, c.eax);
            c.edi = if c.edx == 0 {
                ld(&*g, at(c.esp, 0x454))?
            } else {
                let parent = g.u16(at(c.esi, PARENT))? as i16 as i32 as u32;
                at(parent.wrapping_mul(MATRIX_BYTES), c.ebp)
            };
            c.ecx = ld(&*g, at(c.esp, 0x450))?;
            c.eax = at(c.eax, c.ecx);
            push(c, g, c.eax)?;
            c.eax = at(c.esp, 0x18);
            push(c, g, c.eax)?;
            pose_leaf(c, g)?;
            c.edx = at(c.edx.wrapping_mul(MATRIX_BYTES), c.ebp);
            push(c, g, c.edx)?;
            c.edx = at(c.esp, 0x18);
            c.ecx = c.edi;
            compose_leaf(c, g)?;
            let child = g.u16(at(c.esi, FIRST_CHILD))?;
            c.eax = (c.eax & !0xffff) | u32::from(child);
            if child != 0xffff {
                c.ecx = child as i16 as i32 as u32;
                c.eax = ld(&*g, at(c.esp, 0x10))?;
                st(
                    g,
                    at(c.esp, c.eax.wrapping_mul(4).wrapping_add(0x48)),
                    c.ecx,
                )?;
                c.eax = c.eax.wrapping_add(1);
                st(g, at(c.esp, 0x10), c.eax)?;
            } else {
                c.eax = ld(&*g, at(c.esp, 0x10))?;
            }
            let sibling = g.u16(at(c.esi, NEXT_SIBLING))?;
            c.esi = (c.esi & !0xffff) | u32::from(sibling);
            if sibling != 0xffff {
                c.edx = sibling as i16 as i32 as u32;
                st(
                    g,
                    at(c.esp, c.eax.wrapping_mul(4).wrapping_add(0x48)),
                    c.edx,
                )?;
                c.eax = c.eax.wrapping_add(1);
                st(g, at(c.esp, 0x10), c.eax)?;
            }
            if c.ebx == c.eax {
                break;
            }
        }
        c.edi = pop(c, &*g)?;
        c.esi = pop(c, &*g)?;
    }
    c.ebp = pop(c, &*g)?;
    c.ebx = pop(c, &*g)?;
    c.esp = arithmetic(c, c.esp, FRAME, false);
    g.ret(c, ARGS)
}

#[cfg(test)]
mod tests;
