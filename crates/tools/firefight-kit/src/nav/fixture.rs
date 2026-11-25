//! Invented collision for the kit's tests and its [`crate::testkit`]: a
//! 7x7 arena of 4-unit squares with shared edges, and floors around it.
use std::collections::BTreeMap;

use map::build::Block;
use tags::collision::{Bsp, Child, Edge, Leaf, Node3d, Plane, Surface, Vertex};
use tags::pathfinding::SBSP_COLLISION_FIELD;

/// The arena with one detached square off its east edge and a floor far
/// off to the north-east that no start reaches, as a decorative island or
/// a ledge past a map's boundary.
pub(crate) fn grid_with_far_floor() -> Bsp {
    floors(&[(9, 3), (40, 40)])
}

/// The 7x7 arena of 4-unit squares, then a square at each of `extra`'s
/// cells (shifted 7.8 west from column 9 on, so the first lies 0.2 off
/// the arena's east edge).
pub(crate) fn floors(extra: &[(i32, i32)]) -> Bsp {
    let mut bsp = Bsp {
        planes: vec![Plane([0., 0., 1., 0.])],
        nodes: vec![Node3d {
            plane: 0,
            back: Child::None,
            front: Child::Leaf(0),
        }],
        leaves: vec![Leaf {
            flags: 0,
            ref_count: 0,
            first_ref: u16::MAX,
        }],
        ..Default::default()
    };
    let mut points = BTreeMap::new();
    let mut edges = BTreeMap::new();
    for (x, y) in (0..7)
        .flat_map(|y| (0..7).map(move |x| (x, y)))
        .chain(extra.iter().copied())
    {
        let corners = [(x, y), (x + 1, y), (x + 1, y + 1), (x, y + 1)].map(|p| {
            *points.entry(p).or_insert_with(|| {
                let i = bsp.vertices.len() as u16;
                bsp.vertices.push(Vertex {
                    point: [
                        4. * p.0 as f32 - if p.0 >= 9 { 7.8 } else { 0. },
                        4. * p.1 as f32,
                        0.,
                    ],
                    edge: 0,
                });
                i
            })
        });
        let sector = bsp.surfaces.len() as u16;
        let ring: [u16; 4] = std::array::from_fn(|i| {
            let (a, b) = (corners[i], corners[(i + 1) % 4]);
            if let Some(&k) = edges.get(&(b, a)) {
                bsp.edges[usize::from(k)].right_surface = sector;
                return k;
            }
            let k = bsp.edges.len() as u16;
            edges.insert((a, b), k);
            bsp.vertices[a as usize].edge = k.into();
            bsp.edges.push(Edge {
                start_vertex: a,
                end_vertex: b,
                forward_edge: k,
                reverse_edge: k,
                left_surface: sector,
                right_surface: u16::MAX,
            });
            k
        });
        for (i, &k) in ring.iter().enumerate() {
            let e = &mut bsp.edges[usize::from(k)];
            let next = ring[(i + 1) % 4];
            if e.left_surface == sector {
                e.forward_edge = next;
            } else {
                e.reverse_edge = next;
            }
        }
        bsp.surfaces.push(Surface {
            plane: 0,
            first_edge: ring[0],
            flags: 0,
            breakable_surface: 0xff,
            material: 0,
        });
    }
    bsp
}

/// A structure BSP tag whose collision is [`grid_with_far_floor`].
pub(crate) fn test_floor() -> Block {
    let mut sbsp = Block::zeroed(0x23c);
    sbsp.block(
        SBSP_COLLISION_FIELD as u32,
        1,
        h1convert::structure::collision_block(&grid_with_far_floor()),
    );
    sbsp.block(0x2c, 1, Block::zeroed(8));
    sbsp
}
