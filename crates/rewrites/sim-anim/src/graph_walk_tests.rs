//! The graph walk on synthetic links, records and matrices only: the queue
//! order, the declines, and the walk against the two leaves called one
//! node at a time in that order. The guest-memory equivalence against the
//! register-level reference is in `engine::sim_anim::graph_walk`'s tests.
use crate::graph_walk::{
    GraphNode, GraphWalk, NONE, QUEUE, graph_matrices, graph_matrices_guest, graph_order,
};
use crate::kernel::{Record, Unsupported};
use crate::skeleton::{Constants, compose, pose};
use sim_core::det::Fp;
use sim_core::lanes::Vec4;
use sim_core::math::{NodeMatrix, Vec3};

const ONE: u32 = 0x3f80_0000;
const SIGN: u32 = 0x8000_0000;

fn k() -> Constants {
    Constants {
        one: ONE,
        threshold: 1e-6f32.to_bits(),
        signs: [SIGN, 0, 0, SIGN],
    }
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    /// A magnitude in `[0.25, 4)`, either sign.
    fn value(&mut self) -> u32 {
        let r = self.next();
        let exponent = 125 + (r % 4) as u32;
        let mantissa = (r >> 8) as u32 & 0x7f_ffff;
        let sign = ((r >> 40) as u32 & 1) << 31;
        sign | exponent << 23 | mantissa
    }
    fn record(&mut self) -> Record {
        let mut r: Record = std::array::from_fn(|_| self.value());
        r[7] = (r[7] & 0x807f_ffff) | (126 + (self.next() % 2) as u32) << 23;
        r
    }
    fn matrix(&mut self) -> NodeMatrix {
        let mut v = || Vec3([self.value(), self.value(), self.value()]);
        let (forward, left, up, position) = (v(), v(), v(), v());
        NodeMatrix {
            scale: (126 + (self.next() % 2) as u32) << 23,
            forward,
            left,
            up,
            position,
        }
    }
}

fn node(first_child: i16, next_sibling: i16, parent: i16) -> GraphNode {
    GraphNode {
        first_child,
        next_sibling,
        parent,
    }
}

/// A tree of `count` nodes as child and sibling links, parents first.
fn tree(rng: &mut Rng, count: usize) -> Vec<GraphNode> {
    let mut nodes = vec![node(NONE, NONE, NONE); count];
    let mut last: Vec<Option<usize>> = vec![None; count];
    for i in 1..count {
        let p = (rng.next() % i as u64) as usize;
        match last[p] {
            None => nodes[p].first_child = i as i16,
            Some(prev) => nodes[prev].next_sibling = i as i16,
        }
        last[p] = Some(i);
        nodes[i].parent = p as i16;
    }
    nodes
}

/// The walk one node at a time through the leaves, in `order`.
fn by_hand(
    k: &Constants,
    locals: &[Record],
    nodes: &[GraphNode],
    order: &[usize],
    root: &NodeMatrix,
    mut xmm: [Vec4; 8],
    out: &mut [NodeMatrix],
) -> GraphWalk {
    let mut posed = NodeMatrix::default();
    for &n in order {
        let mut fp = Fp::new();
        let parent = if n == 0 {
            *root
        } else {
            out[nodes[n].parent as usize]
        };
        posed = pose(&mut fp, k, &locals[n], xmm[1]);
        let (m, regs) = compose(&mut fp, &parent, &posed);
        assert!(fp.exact());
        out[n] = m;
        xmm = regs;
    }
    GraphWalk {
        queue: order.to_vec(),
        posed,
        xmm,
    }
}

#[test]
fn the_queue_takes_children_before_siblings_first_in_first_out() {
    // 0 -> 1 -> (3, 2), 2 -> 4; 5 is unlinked.
    let nodes = [
        node(1, NONE, NONE),
        node(3, NONE, 0),
        node(4, NONE, 1),
        node(NONE, 2, 1),
        node(NONE, NONE, 2),
        node(NONE, NONE, 0),
    ];
    assert_eq!(graph_order(&nodes), Ok(vec![0, 1, 3, 2, 4]));
    // Node 0's own sibling is queued, and a node linked twice is queued
    // twice.
    let nodes = [node(1, 2, NONE), node(NONE, 2, 0), node(NONE, NONE, 0)];
    assert_eq!(graph_order(&nodes), Ok(vec![0, 1, 2, 2]));
    assert_eq!(graph_order(&[node(NONE, NONE, NONE)]), Ok(vec![0]));
}

#[test]
fn links_out_of_range_and_a_full_queue_decline() {
    assert_eq!(graph_order(&[]), Err(Unsupported));
    assert_eq!(graph_order(&[node(1, NONE, NONE)]), Err(Unsupported));
    assert_eq!(
        graph_order(&[node(-2, NONE, NONE), node(NONE, NONE, 0)]),
        Err(Unsupported)
    );
    // A cycle grows the queue until it is full.
    assert_eq!(graph_order(&[node(0, NONE, NONE)]), Err(Unsupported));
    // Exactly the queue's capacity, and one more.
    let fan = |count: usize| -> Vec<GraphNode> {
        (0..count as i16)
            .map(|i| match i {
                0 => node(1, NONE, NONE),
                _ if (i as usize) < count - 1 => node(NONE, i + 1, 0),
                _ => node(NONE, NONE, 0),
            })
            .collect()
    };
    assert_eq!(graph_order(&fan(QUEUE)).map(|q| q.len()), Ok(QUEUE));
    assert_eq!(graph_order(&fan(QUEUE + 1)), Err(Unsupported));
}

#[test]
fn the_walk_equals_the_leaves_in_queue_order() {
    let mut rng = Rng(0x1234_5678_9abc_def1);
    let k = k();
    for count in [1usize, 2, 7, 33, 90] {
        let nodes = tree(&mut rng, count);
        let locals: Vec<Record> = (0..count).map(|_| rng.record()).collect();
        let start: Vec<NodeMatrix> = (0..count).map(|_| rng.matrix()).collect();
        let root = rng.matrix();
        let xmm: [Vec4; 8] = std::array::from_fn(|_| std::array::from_fn(|_| rng.value()));

        let mut got = start.clone();
        let walk = graph_matrices(&k, &locals, &nodes, &root, xmm, &mut got).unwrap();
        let order = graph_order(&nodes).unwrap();
        let mut want = start.clone();
        let expected = by_hand(&k, &locals, &nodes, &order, &root, xmm, &mut want);
        assert_eq!(walk, expected, "count {count}");
        assert_eq!(got, want, "count {count}");
        assert_eq!(walk.queue.len(), count, "a tree reaches every node");
    }
}

#[test]
fn the_root_is_node_zeros_parent_and_unvisited_nodes_keep_their_matrices() {
    let mut rng = Rng(0xfeed_beef);
    let k = k();
    let nodes = [node(1, NONE, 2), node(NONE, NONE, 0), node(NONE, NONE, 0)];
    let locals: Vec<Record> = (0..3).map(|_| rng.record()).collect();
    let start: Vec<NodeMatrix> = (0..3).map(|_| rng.matrix()).collect();
    let root = rng.matrix();
    let mut out = start.clone();
    let walk = graph_matrices(&k, &locals, &nodes, &root, [[0; 4]; 8], &mut out).unwrap();
    let mut fp = Fp::new();
    // Node 0's parent field names node 2; the walk uses the root.
    let p0 = pose(&mut fp, &k, &locals[0], [0; 4]);
    let (m0, x0) = compose(&mut fp, &root, &p0);
    let p1 = pose(&mut fp, &k, &locals[1], x0[1]);
    let (m1, x1) = compose(&mut fp, &m0, &p1);
    assert!(fp.exact());
    assert_eq!(out, vec![m0, m1, start[2]]);
    assert_eq!(walk.posed, p1);
    assert_eq!(walk.xmm, x1);
}

#[test]
fn a_node_queued_twice_composes_with_its_parent_as_it_stands() {
    let mut rng = Rng(77);
    let k = k();
    // Node 1 is queued twice and names itself as parent.
    let nodes = [node(1, 1, NONE), node(NONE, NONE, 1)];
    let locals: Vec<Record> = (0..2).map(|_| rng.record()).collect();
    let start: Vec<NodeMatrix> = (0..2).map(|_| rng.matrix()).collect();
    let root = rng.matrix();
    let mut out = start.clone();
    graph_matrices(&k, &locals, &nodes, &root, [[0; 4]; 8], &mut out).unwrap();
    let mut fp = Fp::new();
    let p0 = pose(&mut fp, &k, &locals[0], [0; 4]);
    let (m0, x0) = compose(&mut fp, &root, &p0);
    let p1 = pose(&mut fp, &k, &locals[1], x0[1]);
    let (first, x1) = compose(&mut fp, &start[1], &p1);
    let p1 = pose(&mut fp, &k, &locals[1], x1[1]);
    let (second, _) = compose(&mut fp, &first, &p1);
    assert!(fp.exact());
    assert_eq!(out, vec![m0, second]);
}

#[test]
fn the_order_matters_a_walk_in_index_order_differs() {
    // The deliberate mismatch: the same nodes walked in index order (as
    // `bd090` does) give other matrices, so the tests above see the order.
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    let k = k();
    // 0 -> 2 -> 1: node 1's parent is node 2, which index order reaches
    // after node 1.
    let nodes = [node(2, NONE, NONE), node(NONE, NONE, 2), node(1, NONE, 0)];
    let locals: Vec<Record> = (0..3).map(|_| rng.record()).collect();
    let start: Vec<NodeMatrix> = (0..3).map(|_| rng.matrix()).collect();
    let root = rng.matrix();
    let mut queued = start.clone();
    let walk = graph_matrices(&k, &locals, &nodes, &root, [[0; 4]; 8], &mut queued).unwrap();
    assert_eq!(walk.queue, vec![0, 2, 1]);
    let mut indexed = start.clone();
    by_hand(
        &k,
        &locals,
        &nodes,
        &[0, 1, 2],
        &root,
        [[0; 4]; 8],
        &mut indexed,
    );
    assert_eq!(queued[0], indexed[0]);
    assert_eq!(queued[2], indexed[2]);
    assert_ne!(
        queued[1], indexed[1],
        "node 1 composes with node 2 as it stands"
    );
}

#[test]
fn bad_parents_short_inputs_and_off_path_operands_decline() {
    let mut rng = Rng(5);
    let k = k();
    let chain = [node(1, NONE, NONE), node(2, NONE, 0), node(NONE, NONE, 1)];
    let locals: Vec<Record> = (0..3).map(|_| rng.record()).collect();
    let start: Vec<NodeMatrix> = (0..3).map(|_| rng.matrix()).collect();
    let root = rng.matrix();
    let run = |locals: &[Record], nodes: &[GraphNode], xmm: [Vec4; 8]| {
        let mut out = start.clone();
        graph_matrices(&k, locals, nodes, &root, xmm, &mut out)
    };
    let zero = [[0; 4]; 8];
    assert!(run(&locals, &chain, zero).is_ok());
    // A visited node's parent outside the nodes; node 0's is never read.
    let mut bad = chain;
    bad[2].parent = 3;
    assert_eq!(run(&locals, &bad, zero), Err(Unsupported));
    bad[2].parent = -1;
    assert_eq!(run(&locals, &bad, zero), Err(Unsupported));
    let mut odd_root = chain;
    odd_root[0].parent = 99;
    assert!(run(&locals, &odd_root, zero).is_ok());
    // An unvisited node's bad parent is never read.
    let mut unvisited = chain;
    unvisited[1].first_child = NONE;
    unvisited[2].parent = 99;
    assert!(run(&locals, &unvisited, zero).is_ok());
    // Short inputs.
    assert_eq!(run(&locals[..2], &chain, zero), Err(Unsupported));
    assert_eq!(run(&locals, &chain[..2], zero), Err(Unsupported));
    let mut none: Vec<NodeMatrix> = Vec::new();
    assert_eq!(
        graph_matrices(&k, &locals, &chain, &root, zero, &mut none),
        Err(Unsupported)
    );
    // A denormal in a quaternion, and an infinity in XMM1's dead lane.
    let mut odd = locals.clone();
    odd[2][1] = 1;
    assert_eq!(run(&odd, &chain, zero), Err(Unsupported));
    let mut xmm = zero;
    xmm[1][3] = 0x7f80_0000;
    assert_eq!(run(&locals, &chain, xmm), Err(Unsupported));
    // A denormal in the root is an operand of node 0's composition.
    let mut out = start.clone();
    let odd_root = NodeMatrix { scale: 1, ..root };
    assert_eq!(
        graph_matrices(&k, &locals, &chain, &odd_root, zero, &mut out),
        Err(Unsupported)
    );
}

/// Under the live MXCSR (PE clear) the walk is the fast path's, and the
/// MXCSR comes out with PE set; an unmasked PE refuses.
#[test]
fn the_guest_walk_runs_under_the_live_mxcsr() {
    let mut rng = Rng(0x0a1d_face);
    let k = k();
    let nodes = tree(&mut rng, 9);
    let locals: Vec<Record> = (0..9).map(|_| rng.record()).collect();
    let start: Vec<NodeMatrix> = (0..9).map(|_| rng.matrix()).collect();
    let root = rng.matrix();
    let mut fast = start.clone();
    let want = graph_matrices(&k, &locals, &nodes, &root, [[0; 4]; 8], &mut fast).unwrap();
    for mxcsr in [0x1f80, 0x1fa0] {
        let mut got = start.clone();
        let walk = graph_matrices_guest(mxcsr, &k, &locals, &nodes, &root, [[0; 4]; 8], &mut got);
        assert_eq!(walk, Ok((want.clone(), 0x1fa0)), "{mxcsr:#x}");
        assert_eq!(got, fast, "{mxcsr:#x}");
    }
    let mut got = start.clone();
    let refused = graph_matrices_guest(0x0f80, &k, &locals, &nodes, &root, [[0; 4]; 8], &mut got);
    assert_eq!(refused, Err(Unsupported));
}
