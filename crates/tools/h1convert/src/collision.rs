//! Halo 1 structure collision BSP to Halo 2's.
//!
//! The two layouts hold the same tree (docs/halo1/map-format.md,
//! "Collision BSP"; docs/formats/collision.md): Halo 1 stores every index as
//! an `i32` with bit 31 as the flag; Halo 2 packs 3D children into 24 bits
//! (bit 23 a leaf) and everything else into `u16` (bit 15 the flag). So the
//! conversion is a narrowing copy: planes and vertices bit for bit, every
//! index re-encoded, and a refusal when one doesn't fit.

use h1map::bsp::{self as h1, CollisionBsp};
use tags::collision::{Bsp, Bsp2dRef, Child, Child2d, Edge, Leaf, Node3d, Plane, Surface, Vertex};

use crate::{Error, Result};

/// Largest 3D node or leaf index Halo 2's 24-bit children hold (bit 23 is
/// the leaf flag, `0xffffff` is none).
pub const MAX_CHILD: u32 = 0x7f_ffff;
/// Largest index a flagged `u16` holds (bit 15 is the flag).
pub const MAX_FLAGGED: u32 = 0x7fff;

fn narrow(what: &'static str, v: u32, max: u32) -> Result<u16> {
    if v > max {
        return Err(Error::TooBig {
            what,
            value: v,
            max,
        });
    }
    Ok(v as u16)
}

/// `u32::MAX` (Halo 1's `-1`) is Halo 2's `0xffff`.
fn index16(what: &'static str, v: u32) -> Result<u16> {
    if v == u32::MAX {
        return Ok(u16::MAX);
    }
    narrow(what, v, u16::MAX as u32 - 1)
}

/// Bit 31 flag and index to bit 15 flag and index.
fn flagged(what: &'static str, v: u32) -> Result<u16> {
    let flag = if v & 0x8000_0000 != 0 { 0x8000 } else { 0 };
    Ok(flag | narrow(what, v & 0x7fff_ffff, MAX_FLAGGED)?)
}

fn child(c: h1::Child) -> Result<Child> {
    let fits = |what, v: u32| {
        if v > MAX_CHILD {
            Err(Error::TooBig {
                what,
                value: v,
                max: MAX_CHILD,
            })
        } else {
            Ok(v)
        }
    };
    Ok(match c {
        h1::Child::Solid => Child::None,
        h1::Child::Node(n) => Child::Node(fits("3D node", n)?),
        h1::Child::Leaf(l) => Child::Leaf(fits("leaf", l)?),
    })
}

fn child2d(v: u32) -> Result<Child2d> {
    let i = narrow("2D child", v & 0x7fff_ffff, MAX_FLAGGED)?;
    Ok(if v & 0x8000_0000 != 0 {
        Child2d::Surface(i)
    } else {
        Child2d::Node(i)
    })
}

/// Most edges the Rust collision walk follows around one surface's ring.
pub const MAX_RING: usize = 64;
/// Deepest 3D tree (nodes on one root-to-leaf path) the Rust walk holds.
pub const MAX_DEPTH: usize = 256;

/// The longest surface ring (edges) and the deepest 3D path (nodes) of
/// `bsp`. A ring that doesn't close counts as one past `edges.len()`; a
/// tree that loops counts as unbounded (`usize::MAX`).
pub fn extent(bsp: &Bsp) -> (usize, usize) {
    let mut ring = 0;
    for (k, s) in bsp.surfaces.iter().enumerate() {
        let (mut e, mut n) = (s.first_edge, 0usize);
        loop {
            let Some(edge) = bsp.edges.get(usize::from(e)) else { break };
            n += 1;
            e = if usize::from(edge.right_surface) == k {
                edge.reverse_edge
            } else {
                edge.forward_edge
            };
            if e == s.first_edge || n > bsp.edges.len() {
                break;
            }
        }
        ring = ring.max(n);
    }
    let mut depth = 0;
    let mut stack = vec![(0usize, 1usize)];
    let mut visited = 0usize;
    while let Some((i, d)) = stack.pop() {
        let Some(node) = bsp.nodes.get(i) else { continue };
        visited += 1;
        if visited > bsp.nodes.len().saturating_mul(2).max(1) + 1 {
            return (ring, usize::MAX);
        }
        depth = depth.max(d);
        for c in [node.back, node.front] {
            if let Child::Node(k) = c {
                stack.push((k as usize, d + 1));
            }
        }
    }
    (ring, depth)
}

/// Refuses by name a surface ring over [`MAX_RING`] edges or a tree deeper
/// than [`MAX_DEPTH`]: the Rust walk's bounds. Nothing is split or trimmed.
pub fn check_bounds(bsp: &Bsp) -> Result<()> {
    let (ring, depth) = extent(bsp);
    if ring > MAX_RING {
        return Err(Error::CollisionBound { what: "surface ring edges", value: ring, max: MAX_RING });
    }
    if depth > MAX_DEPTH {
        return Err(Error::CollisionBound { what: "tree depth", value: depth, max: MAX_DEPTH });
    }
    Ok(())
}

/// Converts a Halo 1 collision BSP. Fails when an index or count is past
/// what Halo 2's packed fields hold; nothing is dropped silently.
pub fn convert(src: &CollisionBsp) -> Result<Bsp> {
    let nodes = src
        .nodes
        .iter()
        .map(|n| {
            Ok(Node3d {
                plane: index16("3D node plane", n.plane)?,
                back: child(n.back)?,
                front: child(n.front)?,
            })
        })
        .collect::<Result<_>>()?;
    let leaves = src
        .leaves
        .iter()
        .map(|l| {
            Ok(Leaf {
                flags: u8::try_from(l.flags).map_err(|_| Error::TooBig {
                    what: "leaf flags",
                    value: l.flags.into(),
                    max: 0xff,
                })?,
                ref_count: u8::try_from(l.ref_count).map_err(|_| Error::TooBig {
                    what: "leaf 2D reference count",
                    value: l.ref_count.into(),
                    max: 0xff,
                })?,
                first_ref: if l.ref_count == 0 && l.first_ref == u32::MAX {
                    u16::MAX
                } else {
                    index16("leaf first 2D reference", l.first_ref)?
                },
            })
        })
        .collect::<Result<_>>()?;
    let refs = src
        .refs
        .iter()
        .map(|r| {
            Ok(Bsp2dRef {
                plane: flagged("2D reference plane", r.plane)?,
                node: flagged("2D reference node", r.node)?,
            })
        })
        .collect::<Result<_>>()?;
    let nodes_2d = src
        .nodes_2d
        .iter()
        .map(|n| {
            Ok(tags::collision::Node2d {
                plane: n.plane,
                back: child2d(n.back)?,
                front: child2d(n.front)?,
            })
        })
        .collect::<Result<_>>()?;
    let surfaces = src
        .surfaces
        .iter()
        .map(|s| {
            Ok(Surface {
                plane: flagged("surface plane", s.plane)?,
                first_edge: index16("surface first edge", s.first_edge)?,
                flags: s.flags,
                breakable_surface: s.breakable_surface as u8,
                material: s.material,
            })
        })
        .collect::<Result<_>>()?;
    let edges = src
        .edges
        .iter()
        .map(|e| {
            Ok(Edge {
                start_vertex: index16("edge vertex", e.start_vertex)?,
                end_vertex: index16("edge vertex", e.end_vertex)?,
                forward_edge: index16("edge", e.forward_edge)?,
                reverse_edge: index16("edge", e.reverse_edge)?,
                left_surface: index16("edge surface", e.left_surface)?,
                right_surface: index16("edge surface", e.right_surface)?,
            })
        })
        .collect::<Result<_>>()?;
    for (what, n) in [
        ("planes", src.planes.len()),
        ("surfaces", src.surfaces.len()),
        ("2D nodes", src.nodes_2d.len()),
    ] {
        narrow(what, n.saturating_sub(1) as u32, MAX_FLAGGED)?;
    }
    for (what, n) in [
        ("edges", src.edges.len()),
        ("vertices", src.vertices.len()),
        ("2D references", src.refs.len()),
    ] {
        index16(what, n.saturating_sub(1) as u32)?;
    }
    let bsp = Bsp {
        unknown_00: 0,
        nodes,
        planes: src.planes.iter().map(|&p| Plane(p)).collect(),
        leaves,
        refs,
        nodes_2d,
        surfaces,
        edges,
        vertices: src
            .vertices
            .iter()
            .map(|v| Vertex {
                point: v.point,
                edge: v.first_edge,
            })
            .collect(),
    };
    check_bounds(&bsp)?;
    Ok(bsp)
}

#[cfg(test)]
mod bounds_tests {
    use super::*;
    use tags::collision::{Edge, Node3d, Surface};

    /// One surface whose ring has `n` edges, closed.
    fn ring(n: usize) -> Bsp {
        let edge = |k: usize| Edge {
            start_vertex: 0,
            end_vertex: 0,
            forward_edge: ((k + 1) % n) as u16,
            reverse_edge: ((k + n - 1) % n) as u16,
            left_surface: 0,
            right_surface: 0xffff,
        };
        Bsp {
            surfaces: vec![Surface {
                plane: 0,
                first_edge: 0,
                flags: 0,
                breakable_surface: 0xff,
                material: 0,
            }],
            edges: (0..n).map(edge).collect(),
            ..Bsp::default()
        }
    }

    /// A chain of `n` nodes, each the front of the one before.
    fn chain(n: usize) -> Bsp {
        Bsp {
            nodes: (0..n)
                .map(|k| Node3d {
                    plane: 0,
                    back: Child::None,
                    front: if k + 1 < n { Child::Node(k as u32 + 1) } else { Child::Leaf(0) },
                })
                .collect(),
            ..Bsp::default()
        }
    }

    #[test]
    fn a_ring_of_64_edges_passes_and_65_is_named() {
        assert_eq!(extent(&ring(64)), (64, 0));
        check_bounds(&ring(64)).unwrap();
        assert!(matches!(
            check_bounds(&ring(65)),
            Err(Error::CollisionBound { what: "surface ring edges", value: 65, max: 64 })
        ));
    }

    #[test]
    fn a_tree_256_deep_passes_and_257_is_named() {
        assert_eq!(extent(&chain(256)).1, 256);
        check_bounds(&chain(256)).unwrap();
        assert!(matches!(
            check_bounds(&chain(257)),
            Err(Error::CollisionBound { what: "tree depth", value: 257, max: 256 })
        ));
    }

    #[test]
    fn a_looping_tree_is_refused_not_walked_forever() {
        let mut b = chain(2);
        b.nodes[1].front = Child::Node(0);
        assert!(check_bounds(&b).is_err());
    }
}
