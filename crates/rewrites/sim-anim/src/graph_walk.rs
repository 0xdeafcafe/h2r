//! `graph_matrices`: the animation graph's node walk `1dd1c0`, which turns
//! an object's node records into node matrices for the graph's own callers
//! (docs/engine/skeleton-walk.md, "The graph walk `1dd1c0`"; phase 1b of
//! docs/engine/sim-rewrite.md).
//!
//! Unlike `bd090`'s walk ([`crate::skeleton`]), the graph walk visits the
//! nodes in the order of a first-in, first-out queue that starts with node
//! 0: each visited node queues its first child and then its next sibling,
//! when they are not `-1`. Every visited node is posed (`1421f0`) into a
//! scratch matrix, which is then composed (`142a60`) with its parent's
//! matrix into the node's slot. Node 0's parent is the caller's root
//! matrix. A parent is read from the array as it stands at that moment, so
//! a node visited twice is posed and composed twice.
//!
//! The arithmetic is [`crate::skeleton`]'s two leaves, lane for lane. The
//! walk also returns the queue, the last scratch matrix and the XMM
//! registers the last composition leaves, because the guest's caller can
//! observe all three (the queue and scratch matrix in its dead stack).
use crate::kernel::{Record, Unsupported};
use crate::skeleton::{Constants, compose, pose};
use sim_core::det::Fp;
use sim_core::lanes::Vec4;
use sim_core::math::NodeMatrix;

/// The link value that ends a chain.
pub const NONE: i16 = -1;

/// The guest's queue holds 255 node indices; a 256th would overwrite the
/// walk's return address. The walk declines before that.
pub const QUEUE: usize = 255;

/// The three links of a graph node (`+4`, `+6` and `+8` of its 0x20 bytes).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GraphNode {
    pub first_child: i16,
    pub next_sibling: i16,
    pub parent: i16,
}

/// What the walk leaves besides the matrices.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphWalk {
    /// Every node index queued, in queue order; the walk visits them in
    /// this order. Starts with 0.
    pub queue: Vec<usize>,
    /// The scratch matrix: the last visited node's pose.
    pub posed: NodeMatrix,
    /// XMM0 to XMM7 after the last composition.
    pub xmm: [Vec4; 8],
}

/// The queue the walk builds over `nodes`, from node 0. Declines when a
/// queued index is not `-1` and not below the node count, or when the
/// queue would pass [`QUEUE`] entries (a cycle among the links does).
pub fn graph_order(nodes: &[GraphNode]) -> Result<Vec<usize>, Unsupported> {
    let count = nodes.len();
    if count == 0 {
        return Err(Unsupported);
    }
    let mut queue = vec![0usize];
    let mut next = 0;
    while next < queue.len() {
        let node = nodes[queue[next]];
        next += 1;
        for link in [node.first_child, node.next_sibling] {
            if link == NONE {
                continue;
            }
            let i = usize::try_from(link)
                .ok()
                .filter(|&i| i < count)
                .ok_or(Unsupported)?;
            if queue.len() == QUEUE {
                return Err(Unsupported);
            }
            queue.push(i);
        }
    }
    Ok(queue)
}

/// The graph walk over `out`, whose length is the node count (at least 1).
/// `locals` and `nodes` cover at least `out.len()` nodes; `root` is node
/// 0's parent; `xmm` is the register file on entry.
///
/// Declines when an input is short, when [`graph_order`] does, when a
/// visited node other than 0 names a parent outside `out`, or when any
/// operand of any operation is off the fast path. Nothing in `out` is
/// meaningful after a decline.
pub fn graph_matrices(
    k: &Constants,
    locals: &[Record],
    nodes: &[GraphNode],
    root: &NodeMatrix,
    xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> Result<GraphWalk, Unsupported> {
    graph_with(Fp::new(), k, locals, nodes, root, xmm, out).map(|(walk, _)| walk)
}

/// [`graph_matrices`] under the guest's `mxcsr`, as `1dd1c0`'s SSE code
/// runs whatever the MXCSR holds ([`Fp::guest`]): the walk and the MXCSR
/// after it, with every flag an operation raised. Declines on the same
/// input faults, and on an operation whose exception the MXCSR unmasks.
pub fn graph_matrices_guest(
    mxcsr: u32,
    k: &Constants,
    locals: &[Record],
    nodes: &[GraphNode],
    root: &NodeMatrix,
    xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> Result<(GraphWalk, u32), Unsupported> {
    let (walk, fp) = graph_with(Fp::guest(mxcsr), k, locals, nodes, root, xmm, out)?;
    Ok((walk, fp.mxcsr().unwrap_or(mxcsr)))
}

fn graph_with(
    mut fp: Fp,
    k: &Constants,
    locals: &[Record],
    nodes: &[GraphNode],
    root: &NodeMatrix,
    xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> Result<(GraphWalk, Fp), Unsupported> {
    let count = out.len();
    if count == 0 || locals.len() < count || nodes.len() < count {
        return Err(Unsupported);
    }
    let queue = graph_order(&nodes[..count])?;
    let mut regs = xmm;
    let mut posed = NodeMatrix::default();
    for &n in &queue {
        // The guest reads the parent before the pose; both precede the
        // composition, so the order does not matter here.
        let parent = if n == 0 {
            *root
        } else {
            let p = usize::try_from(nodes[n].parent)
                .ok()
                .filter(|&p| p < count)
                .ok_or(Unsupported)?;
            out[p]
        };
        posed = pose(&mut fp, k, &locals[n], regs[1]);
        let (m, r) = compose(&mut fp, &parent, &posed);
        out[n] = m;
        regs = r;
    }
    if fp.exact() {
        let walk = GraphWalk {
            queue,
            posed,
            xmm: regs,
        };
        Ok((walk, fp))
    } else {
        Err(Unsupported)
    }
}
