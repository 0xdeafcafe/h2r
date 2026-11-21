//! Invented meshes for tests and examples: a cube, an open box (an open
//! shell), a plane grid, a cylinder, a fin (three faces on one edge) and a
//! cube as a source triangle list. All are wound counter-clockwise seen
//! from outside, with texture and lightmap coordinates and normals on
//! every corner.
//!
//! Vertex ids follow the order the fixture adds them, given in each
//! function's docs.

use std::f32::consts::TAU;

use crate::attrs::Corner;
use crate::ids::VertId;
use crate::io::Source;
use crate::math::Vec3;
use crate::mesh::{Mesh, Poly};

/// How `cylinder` closes its ends.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Caps {
    /// A tube with two boundary loops.
    Open,
    /// A fan of triangles round a centre vertex (a pole) at each end.
    Fan,
    /// One n-gon at each end.
    Ngon,
}

/// A corner with texture coordinate `(u, v)`, lightmap coordinate half
/// that, and `normal`.
pub fn corner(u: f32, v: f32, normal: Vec3) -> Corner {
    Corner { uv: [u, v], lightmap: [u * 0.5, v * 0.5], normal, ..Corner::default() }
}

const CUBE_FACES: [([usize; 4], Vec3); 6] = [
    ([0, 2, 3, 1], [0.0, 0.0, -1.0]),
    ([4, 5, 7, 6], [0.0, 0.0, 1.0]),
    ([0, 1, 5, 4], [0.0, -1.0, 0.0]),
    ([2, 6, 7, 3], [0.0, 1.0, 0.0]),
    ([0, 4, 6, 2], [-1.0, 0.0, 0.0]),
    ([1, 3, 7, 5], [1.0, 0.0, 0.0]),
];
const QUAD_UVS: [[f32; 2]; 4] = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];

fn cube_corner_position(i: usize, size: f32) -> Vec3 {
    let h = size * 0.5;
    let at = |bit: bool| if bit { h } else { -h };
    [at(i & 1 != 0), at(i & 2 != 0), at(i & 4 != 0)]
}

fn cube_faces(size: f32, faces: &[usize]) -> Mesh {
    let mut m = Mesh::new();
    let v: Vec<VertId> = (0..8).map(|i| m.add_vert(cube_corner_position(i, size), None)).collect();
    for &k in faces {
        let (idx, normal) = CUBE_FACES[k];
        let poly = Poly {
            verts: idx.iter().map(|&i| v[i]).collect(),
            corners: QUAD_UVS.iter().map(|uv| corner(uv[0], uv[1], normal)).collect(),
            material: k as u16,
            group: 0,
            flags: 0,
            src: None,
        };
        m.add_face(&poly).expect("cube face");
    }
    m
}

/// A cube of side `size` centred on the origin: 8 verts (vert `i` at
/// `x = i & 1`, `y = i & 2`, `z = i & 4`, each `-size/2` or `+size/2`), 12
/// edges, 6 quads with flat normals and materials 0 to 5 in the order -z,
/// +z, -y, +y, -x, +x.
pub fn cube(size: f32) -> Mesh {
    cube_faces(size, &[0, 1, 2, 3, 4, 5])
}

/// `cube` without its top (+z) face: an open shell, 8 verts, 12 edges, 5
/// faces, a boundary loop of 4 edges.
pub fn open_box(size: f32) -> Mesh {
    cube_faces(size, &[0, 2, 3, 4, 5])
}

/// A flat `nx` by `ny` grid of quads in z = 0, facing +z, cells of side
/// `size`. Vert `(i, j)` is id `j * (nx + 1) + i`, at
/// `(i * size, j * size, 0)`; coordinates run 0 to 1 across it.
pub fn grid(nx: usize, ny: usize, size: f32) -> Mesh {
    let (nx, ny) = (nx.max(1), ny.max(1));
    let mut m = Mesh::new();
    let mut v = Vec::with_capacity((nx + 1) * (ny + 1));
    for j in 0..=ny {
        for i in 0..=nx {
            v.push(m.add_vert([i as f32 * size, j as f32 * size, 0.0], None));
        }
    }
    let at = |i: usize, j: usize| v[j * (nx + 1) + i];
    let uv = |i: usize, j: usize| corner(i as f32 / nx as f32, j as f32 / ny as f32, [0.0, 0.0, 1.0]);
    for j in 0..ny {
        for i in 0..nx {
            let cells = [(i, j), (i + 1, j), (i + 1, j + 1), (i, j + 1)];
            let mut poly = Poly::new(cells.iter().map(|&(a, b)| at(a, b)).collect());
            poly.corners = cells.iter().map(|&(a, b)| uv(a, b)).collect();
            m.add_face(&poly).expect("grid face");
        }
    }
    m
}

/// A cylinder round the z axis from z = 0 to `height`: `segments` quads
/// round, `rows` high, radial (smooth) side normals. Side coordinates run
/// u = 0 to 1 round (so the seam at vert 0 has u = 0 on one side and 1 on
/// the other) and v = 0 to 1 up. Materials: side 0, bottom 1, top 2.
///
/// Vert `(i, j)` (ring `j` from the bottom, `i` round from +x) is id
/// `j * segments + i`; cap centres, if any, come next (bottom, then top).
pub fn cylinder(segments: usize, rows: usize, radius: f32, height: f32, caps: Caps) -> Mesh {
    let s = segments.max(3);
    let r = rows.max(1);
    let mut m = Mesh::new();
    let angle = |i: usize| TAU * i as f32 / s as f32;
    let mut ring: Vec<Vec<VertId>> = Vec::with_capacity(r + 1);
    for j in 0..=r {
        let z = height * j as f32 / r as f32;
        let row = (0..s).map(|i| m.add_vert([radius * angle(i).cos(), radius * angle(i).sin(), z], None)).collect();
        ring.push(row);
    }
    let radial = |i: usize| [angle(i).cos(), angle(i).sin(), 0.0];
    for j in 0..r {
        for i in 0..s {
            let i1 = (i + 1) % s;
            let (u0, u1) = (i as f32 / s as f32, (i + 1) as f32 / s as f32);
            let (v0, v1) = (j as f32 / r as f32, (j + 1) as f32 / r as f32);
            let mut poly = Poly::new(vec![ring[j][i], ring[j][i1], ring[j + 1][i1], ring[j + 1][i]]);
            poly.corners = vec![
                corner(u0, v0, radial(i)),
                corner(u1, v0, radial(i1)),
                corner(u1, v1, radial(i1)),
                corner(u0, v1, radial(i)),
            ];
            m.add_face(&poly).expect("cylinder side");
        }
    }
    let cap = |i: usize, normal: Vec3| corner(0.5 + 0.5 * angle(i).cos(), 0.5 + 0.5 * angle(i).sin(), normal);
    let (down, up) = ([0.0, 0.0, -1.0], [0.0, 0.0, 1.0]);
    match caps {
        Caps::Open => {}
        Caps::Fan => {
            let bottom = m.add_vert([0.0, 0.0, 0.0], None);
            let top = m.add_vert([0.0, 0.0, height], None);
            for i in 0..s {
                let i1 = (i + 1) % s;
                let mut poly = Poly::new(vec![bottom, ring[0][i1], ring[0][i]]);
                poly.corners = vec![corner(0.5, 0.5, down), cap(i1, down), cap(i, down)];
                poly.material = 1;
                m.add_face(&poly).expect("bottom fan");
                let mut poly = Poly::new(vec![top, ring[r][i], ring[r][i1]]);
                poly.corners = vec![corner(0.5, 0.5, up), cap(i, up), cap(i1, up)];
                poly.material = 2;
                m.add_face(&poly).expect("top fan");
            }
        }
        Caps::Ngon => {
            let order: Vec<usize> = (0..s).rev().collect();
            let mut poly = Poly::new(order.iter().map(|&i| ring[0][i]).collect());
            poly.corners = order.iter().map(|&i| cap(i, down)).collect();
            poly.material = 1;
            m.add_face(&poly).expect("bottom cap");
            let mut poly = Poly::new((0..s).map(|i| ring[r][i]).collect());
            poly.corners = (0..s).map(|i| cap(i, up)).collect();
            poly.material = 2;
            m.add_face(&poly).expect("top cap");
        }
    }
    m
}

/// Three quads on one edge, as a wall stands on a floor: the floor's two
/// halves either side of the edge from vert 0 `(0,0,0)` to vert 1
/// `(0,1,0)`, and a wall up from it. Verts 2, 3 are the left floor's far
/// side (x = -1), 4, 5 the right's (x = 1), 6, 7 the wall's top (z = 1).
/// 8 verts, 10 edges, 3 faces; edge 0-1 has three faces.
pub fn fin() -> Mesh {
    let positions = [
        [0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [-1.0, 0.0, 0.0],
        [-1.0, 1.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 1.0, 1.0],
    ];
    let mut m = Mesh::new();
    let v: Vec<VertId> = positions.iter().map(|&p| m.add_vert(p, None)).collect();
    let quads: [([usize; 4], Vec3, u16); 3] = [
        ([2, 0, 1, 3], [0.0, 0.0, 1.0], 0),
        ([0, 4, 5, 1], [0.0, 0.0, 1.0], 0),
        ([0, 1, 7, 6], [1.0, 0.0, 0.0], 1),
    ];
    for (idx, normal, material) in quads {
        let mut poly = Poly::new(idx.iter().map(|&i| v[i]).collect());
        poly.corners = QUAD_UVS.iter().map(|uv| corner(uv[0], uv[1], normal)).collect();
        poly.material = material;
        m.add_face(&poly).expect("fin face");
    }
    m
}

/// A cube as a section stores it: 24 vertices (four per face, flat
/// normals, so each corner position appears three times) and 12
/// triangles, two per face in face order; face `k`'s triangles are in
/// group `k / 2` with material `k`.
pub fn cube_source(size: f32) -> Source {
    let mut s = Source::default();
    for (k, (idx, normal)) in CUBE_FACES.iter().enumerate() {
        let base = s.positions.len() as u32;
        for (c, &i) in idx.iter().enumerate() {
            s.positions.push(cube_corner_position(i, size));
            s.corners.push(corner(QUAD_UVS[c][0], QUAD_UVS[c][1], *normal));
        }
        for tri in [[0, 1, 2], [0, 2, 3]] {
            s.triangles.push(tri.map(|t| base + t));
            s.groups.push((k / 2) as u16);
            s.materials.push(k as u16);
        }
    }
    s
}
