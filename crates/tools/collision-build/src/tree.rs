//! The 3D tree, its open leaves and their 2D references.
//!
//! Each node partitions the fragments it is given on one fragment's plane
//! (fewest splits, then the smaller difference between the sides, then
//! the lowest plane index). Fragments on that plane stay at the node; the
//! rest go to the side they lie on, split when they straddle. An empty
//! side is solid when a one-sided fragment at the node faces away from it
//! and none faces it; otherwise it is an open leaf.
//!
//! An open leaf borders, on each plane of its path, the fragments kept at
//! that node and seen from its side (one-sided ones facing it, and
//! two-sided ones) that still have area once clipped to the leaf's side of
//! every plane below the node. It takes one reference per such plane, its
//! flip bit set when the leaf is on the plane's back (the sphere query
//! walks a reference only from that side, collision.md), and a 2D tree
//! over the surfaces of those fragments.
//!
//! The build runs on an explicit stack, so a deep tree cannot overflow the
//! thread's.

use tags::collision::{Bsp2dRef, Child, Leaf, Node2d, Node3d};

use crate::geom::{self, V3};
use crate::prep::Prepared;
use crate::tree2d::Trees2d;
use crate::{Limits, Result, TWO_SIDED};

/// Candidate planes scored per node; more are sampled evenly.
const CANDIDATES: usize = 48;

#[derive(Clone, Debug)]
struct Frag {
    surface: u32,
    pts: Vec<V3>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Facing {
    Front,
    Back,
    Both,
}

struct NodeRec {
    plane: usize,
    back: Child,
    front: Child,
    kept: Vec<(Frag, Facing)>,
}

struct Task {
    frags: Vec<Frag>,
    parent: Option<(usize, bool)>,
    path: Vec<(usize, bool)>,
}

pub(crate) struct Tree {
    pub nodes: Vec<Node3d>,
    pub leaves: Vec<Leaf>,
    pub refs: Vec<Bsp2dRef>,
    pub nodes_2d: Vec<Node2d>,
}

pub(crate) fn build(p: &Prepared, limits: &Limits) -> Result<Tree> {
    let frags: Vec<Frag> = (0..p.surfaces.len())
        .map(|s| Frag {
            surface: s as u32,
            pts: p.ring_points(s),
        })
        .collect();
    let mut nodes: Vec<NodeRec> = Vec::new();
    // Each open leaf's path: (node, front side).
    let mut open: Vec<Vec<(usize, bool)>> = Vec::new();
    let mut stack = vec![Task {
        frags,
        parent: None,
        path: Vec::new(),
    }];
    while let Some(task) = stack.pop() {
        let plane = choose(p, &task.frags);
        let (n, d) = p.plane64(plane);
        let (mut front, mut back, mut kept) = (Vec::new(), Vec::new(), Vec::new());
        for f in task.frags {
            let (lo, hi) = geom::extent(&f.pts, n, d);
            // A surface's own plane always keeps it, so every node keeps
            // at least the fragment it was chosen from.
            let own = p.surfaces[f.surface as usize].plane == plane;
            if own || (lo >= -geom::PLANE_EPS && hi <= geom::PLANE_EPS) {
                let facing = facing(p, f.surface as usize, n);
                kept.push((f, facing));
            } else if lo >= -geom::PLANE_EPS {
                front.push(f);
            } else if hi <= geom::PLANE_EPS {
                back.push(f);
            } else {
                let (a, b) = geom::split(&f.pts, n, d);
                if geom::area(&a) > geom::AREA_EPS {
                    front.push(Frag {
                        surface: f.surface,
                        pts: a,
                    });
                }
                if geom::area(&b) > geom::AREA_EPS {
                    back.push(Frag {
                        surface: f.surface,
                        pts: b,
                    });
                }
            }
        }
        let at = nodes.len();
        limits.check("3D node", at + 1)?;
        if let Some((parent, side)) = task.parent {
            let slot = &mut nodes[parent];
            if side {
                slot.front = Child::Node(at as u32);
            } else {
                slot.back = Child::Node(at as u32);
            }
        }
        let mut rec = NodeRec {
            plane,
            back: Child::None,
            front: Child::None,
            kept,
        };
        for (side, list) in [(false, back), (true, front)] {
            let mut path = task.path.clone();
            path.push((at, side));
            if list.is_empty() {
                let child = if solid_side(&rec.kept, side) {
                    Child::None
                } else {
                    limits.check("leaf", open.len() + 1)?;
                    open.push(path);
                    Child::Leaf((open.len() - 1) as u32)
                };
                if side {
                    rec.front = child;
                } else {
                    rec.back = child;
                }
            } else {
                stack.push(Task {
                    frags: list,
                    parent: Some((at, side)),
                    path,
                });
            }
        }
        nodes.push(rec);
    }

    let mut trees = Trees2d::new(p, limits);
    let mut leaves = Vec::with_capacity(open.len());
    let mut refs: Vec<Bsp2dRef> = Vec::new();
    for path in &open {
        let first = refs.len();
        let mut flags = 0u8;
        for (i, &(node, side)) in path.iter().enumerate() {
            let rec = &nodes[node];
            let want = if side { Facing::Front } else { Facing::Back };
            let mut set: Vec<u32> = Vec::new();
            for (f, facing) in &rec.kept {
                if *facing != want && *facing != Facing::Both {
                    continue;
                }
                if set.contains(&f.surface) {
                    continue;
                }
                let mut pts = f.pts.clone();
                for &(below, below_side) in &path[i + 1..] {
                    let (n, d) = p.plane64(nodes[below].plane);
                    pts = geom::clip(&pts, n, d, below_side);
                    if pts.is_empty() {
                        break;
                    }
                }
                if geom::area(&pts) > geom::AREA_EPS {
                    set.push(f.surface);
                }
            }
            if set.is_empty() {
                continue;
            }
            set.sort_unstable();
            if set
                .iter()
                .any(|&s| p.surfaces[s as usize].flags & TWO_SIDED != 0)
            {
                flags |= 1;
            }
            let root = trees.get(rec.plane, !side, set)?;
            limits.check("2D reference", refs.len() + 1)?;
            refs.push(Bsp2dRef {
                plane: rec.plane as u16 | if side { 0 } else { 0x8000 },
                node: root.raw(),
            });
        }
        let count = refs.len() - first;
        limits.check("leaf 2D reference", count)?;
        leaves.push(Leaf {
            flags,
            ref_count: count as u8,
            first_ref: if count == 0 { u16::MAX } else { first as u16 },
        });
    }
    Ok(Tree {
        nodes: nodes
            .iter()
            .map(|n| Node3d {
                plane: n.plane as u16,
                back: n.back,
                front: n.front,
            })
            .collect(),
        leaves,
        refs,
        nodes_2d: trees.nodes,
    })
}

/// Which side of a node's plane (normal `n`) a surface is seen from.
fn facing(p: &Prepared, s: usize, n: V3) -> Facing {
    if p.surfaces[s].flags & TWO_SIDED != 0 {
        Facing::Both
    } else if geom::dot(p.open_normal(s), n) >= 0.0 {
        Facing::Front
    } else {
        Facing::Back
    }
}

/// An empty side is solid when a one-sided fragment at the node faces away
/// from it and none faces it.
fn solid_side(kept: &[(Frag, Facing)], front: bool) -> bool {
    let (this, other) = if front {
        (Facing::Front, Facing::Back)
    } else {
        (Facing::Back, Facing::Front)
    };
    kept.iter().any(|(_, f)| *f == other) && !kept.iter().any(|(_, f)| *f == this)
}

/// The splitting plane for `frags`: fewest splits, then best balance,
/// then lowest plane index.
fn choose(p: &Prepared, frags: &[Frag]) -> usize {
    let mut planes: Vec<usize> = frags
        .iter()
        .map(|f| p.surfaces[f.surface as usize].plane)
        .collect();
    planes.sort_unstable();
    planes.dedup();
    if planes.len() > CANDIDATES {
        let step = planes.len() as f64 / CANDIDATES as f64;
        planes = (0..CANDIDATES)
            .map(|i| planes[(i as f64 * step) as usize])
            .collect();
    }
    let mut best = (usize::MAX, usize::MAX, usize::MAX);
    for &plane in &planes {
        let (n, d) = p.plane64(plane);
        let (mut splits, mut front, mut back) = (0usize, 0usize, 0usize);
        for f in frags {
            let (lo, hi) = geom::extent(&f.pts, n, d);
            let own = p.surfaces[f.surface as usize].plane == plane;
            if own || (lo >= -geom::PLANE_EPS && hi <= geom::PLANE_EPS) {
                continue;
            } else if lo >= -geom::PLANE_EPS {
                front += 1;
            } else if hi <= geom::PLANE_EPS {
                back += 1;
            } else {
                splits += 1;
                front += 1;
                back += 1;
            }
        }
        let score = (splits, front.abs_diff(back), plane);
        if score < best {
            best = score;
        }
    }
    best.2
}
