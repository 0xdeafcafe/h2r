use super::*;
use tags::collision::{Edge, Surface, Vertex};

/// A BSP of separate polygons, each with its own edges, and its surface
/// flags and breakable index.
fn bsp_of(polys: &[(&[V3], u8, u8)]) -> Bsp {
    let mut bsp = Bsp::default();
    for (s, (verts, flags, index)) in polys.iter().enumerate() {
        let first = bsp.edges.len();
        let base = bsp.vertices.len();
        for (k, &p) in verts.iter().enumerate() {
            bsp.vertices.push(Vertex { point: p, edge: 0 });
            bsp.edges.push(Edge {
                start_vertex: (base + k) as u16,
                end_vertex: (base + (k + 1) % verts.len()) as u16,
                forward_edge: (first + (k + 1) % verts.len()) as u16,
                reverse_edge: 0,
                left_surface: s as u16,
                right_surface: u16::MAX,
            });
        }
        bsp.surfaces.push(Surface {
            plane: 0,
            first_edge: first as u16,
            flags: *flags,
            breakable_surface: *index,
            material: 0,
        });
    }
    bsp
}

/// A pane in the plane x = 5.30 from `y0` to `y1` and `z0` to `z1`.
fn pane(y0: f32, y1: f32, z0: f32, z1: f32) -> [V3; 4] {
    [[5.3, y0, z0], [5.3, y1, z0], [5.3, y1, z1], [5.3, y0, z1]]
}

#[test]
fn only_flagged_surfaces_with_an_index_are_panes() {
    let low = pane(-26.5, -26.1, 5.1, 5.6);
    let lip = pane(-26.5, -26.1, 5.7, 6.3);
    let wall = pane(-25.5, -24.5, 5.1, 6.3);
    let bsp = bsp_of(&[
        (&low[..], BREAKABLE_FLAG, 3),
        (&lip[..], BREAKABLE_FLAG, 4),
        (&wall[..], 0, 0xff),
        // Flagged with no index, or indexed with no flag: not breakable.
        (&wall[..], BREAKABLE_FLAG, NO_INDEX),
        (&wall[..], 0, 7),
    ]);
    let got = panes(&bsp);
    assert_eq!(got.len(), 2);
    assert_eq!((got[0].index, got[1].index), (3, 4));
    assert_eq!(got[0].verts, low.to_vec());
    let c = got[0].center();
    assert!((c[1] + 26.3).abs() < 1e-4 && (c[2] - 5.35).abs() < 1e-4, "{c:?}");
}

#[test]
fn the_walk_in_goes_through_the_low_pieces_and_under_the_lip() {
    // Zanzibar's window, simplified: the sill at z 5.10, the pane at x
    // 5.30, a low piece in the gap, the lip's piece above the crouched
    // body (5.10 + 0.5), and a piece of the next window along.
    let panes = vec![
        Pane { index: 4, verts: pane(-26.6, -26.0, 5.7, 6.3).to_vec() },
        Pane { index: 3, verts: pane(-26.6, -26.0, 5.1, 5.6).to_vec() },
        Pane { index: 9, verts: pane(-25.5, -24.5, 5.1, 6.3).to_vec() },
    ];
    let start = [5.57, -26.285, 5.11];
    let stand = [5.22, -26.285, 5.11];
    assert_eq!(in_the_way(&panes, start, stand), vec![1]);
    // Walking along the pane, not through it: nothing in the way.
    assert!(in_the_way(&panes, [5.5, -26.9, 5.11], [5.5, -26.0, 5.11]).is_empty());
    // A stand short of the pane by more than the body's front: clear.
    assert!(in_the_way(&panes, start, [5.5, -26.285, 5.11]).is_empty());
    assert!(in_the_way(&panes, start, start).is_empty());
}

#[test]
fn a_piece_is_whole_while_its_bit_is_set() {
    let mut mask: Mask = [0; 8];
    mask[0] = 1 << 3;
    mask[7] = 1 << 31;
    assert!(intact(&mask, 3));
    assert!(!intact(&mask, 4));
    assert!(intact(&mask, 255));
    assert!(!intact(&mask, 32));
}
