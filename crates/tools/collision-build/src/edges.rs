//! The winged-edge table.
//!
//! The engine walks a surface's ring from its first edge: on an edge whose
//! right surface it is, it takes the end vertex and goes on at the reverse
//! edge, otherwise the start vertex and the forward edge
//! (docs/formats/collision.md, "Ring rule"). So the surface that runs an
//! edge from start to end is its left surface and names the next edge of
//! its ring as the forward edge; a surface that runs it from end to start
//! is its right surface and names its next edge as the reverse edge. An
//! edge holds at most one of each; a third surface on two vertices, or a
//! second running the same way, gets an edge of its own. An edge with no
//! right surface names itself as its reverse edge.

use std::collections::HashMap;

use tags::collision::Edge;

use crate::prep::Prepared;
use crate::{Error, Result};

const NONE: u16 = u16::MAX;

pub(crate) struct Edges {
    pub edges: Vec<Edge>,
    pub first_edge: Vec<u16>,
    pub vertex_edge: Vec<u32>,
}

pub(crate) fn build(p: &Prepared) -> Result<Edges> {
    let mut edges: Vec<Edge> = Vec::new();
    let mut by_pair: HashMap<(u32, u32), Vec<usize>> = HashMap::new();
    // Per surface, the edge of each ring step.
    let mut steps: Vec<Vec<usize>> = Vec::with_capacity(p.surfaces.len());
    for (s, surf) in p.surfaces.iter().enumerate() {
        let n = surf.ring.len();
        let mut mine = Vec::with_capacity(n);
        for i in 0..n {
            let (a, b) = (surf.ring[i], surf.ring[(i + 1) % n]);
            let key = (a.min(b), a.max(b));
            let list = by_pair.entry(key).or_default();
            let partner = list.iter().copied().find(|&e| {
                let r = &edges[e];
                u32::from(r.start_vertex) == b
                    && u32::from(r.end_vertex) == a
                    && r.right_surface == NONE
                    && usize::from(r.left_surface) != s
            });
            let e = match partner {
                Some(e) => {
                    edges[e].right_surface = s as u16;
                    e
                }
                None => {
                    let e = edges.len();
                    if e >= usize::from(NONE) {
                        return Err(Error::TooBig {
                            what: "edge",
                            value: e + 1,
                            max: usize::from(NONE),
                        });
                    }
                    edges.push(Edge {
                        start_vertex: a as u16,
                        end_vertex: b as u16,
                        forward_edge: e as u16,
                        reverse_edge: e as u16,
                        left_surface: s as u16,
                        right_surface: NONE,
                    });
                    list.push(e);
                    e
                }
            };
            mine.push(e);
        }
        steps.push(mine);
    }
    for (s, mine) in steps.iter().enumerate() {
        let n = mine.len();
        for i in 0..n {
            let (e, next) = (mine[i], mine[(i + 1) % n] as u16);
            if usize::from(edges[e].left_surface) == s
                && u32::from(edges[e].start_vertex) == p.surfaces[s].ring[i]
            {
                edges[e].forward_edge = next;
            } else {
                edges[e].reverse_edge = next;
            }
        }
    }
    let mut vertex_edge = vec![u32::MAX; p.points.len()];
    for (i, e) in edges.iter().enumerate() {
        for v in [e.start_vertex, e.end_vertex] {
            let slot = &mut vertex_edge[usize::from(v)];
            if *slot == u32::MAX {
                *slot = i as u32;
            }
        }
    }
    // Every vertex is on a ring (prep keeps only used ones).
    for slot in &mut vertex_edge {
        if *slot == u32::MAX {
            *slot = 0;
        }
    }
    Ok(Edges {
        first_edge: steps.iter().map(|m| m[0] as u16).collect(),
        edges,
        vertex_edge,
    })
}
