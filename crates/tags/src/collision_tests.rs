//! Synthetic payloads only: nothing here is derived from the disc.
use super::*;

const BASE: u32 = 0x8000_0000;

/// Appends child data after a fixed root and hands out virtual addresses.
struct Payload {
    buf: Vec<u8>,
}
impl Payload {
    fn new(root: usize) -> Self {
        Self { buf: vec![0; root] }
    }
    fn addr(&self, off: usize) -> u32 {
        BASE + off as u32
    }
    fn set_u32(&mut self, at: usize, v: u32) {
        self.buf[at..at + 4].copy_from_slice(&v.to_le_bytes());
    }
    /// Append `data`, 4-aligned; returns its offset.
    fn push(&mut self, data: &[u8]) -> usize {
        while !self.buf.len().is_multiple_of(4) {
            self.buf.push(0);
        }
        let at = self.buf.len();
        self.buf.extend_from_slice(data);
        at
    }
    /// Append a block and write its `{count, address}` descriptor at `desc`.
    fn block(&mut self, desc: usize, count: usize, data: &[u8]) -> usize {
        let at = self.push(data);
        let a = self.addr(at);
        self.set_u32(desc, count as u32);
        self.set_u32(desc + 4, a);
        at
    }
}
fn f(v: f32) -> [u8; 4] {
    v.to_le_bytes()
}
fn h(v: u16) -> [u8; 2] {
    v.to_le_bytes()
}
fn rec(parts: &[&[u8]]) -> Vec<u8> {
    parts.iter().flat_map(|p| p.iter().copied()).collect()
}

// ----- coll -----

/// One region, one permutation, one BSP: a unit quad in the z=0 plane.
fn quad_coll() -> Vec<u8> {
    let mut p = Payload::new(COLL_ROOT_SIZE);
    p.block(0x14, 1, &0x0500_0912u32.to_le_bytes()); // one material
    let region_at = p.push(&[0; 12]);
    let perm_at = p.push(&[0; 0x14]);
    let bsp_at = p.push(&[0; BSP_SIZE]);
    p.set_u32(0x1c, 1);
    let a = p.addr(region_at);
    p.set_u32(0x20, a);
    p.set_u32(region_at, 0x0300_0001);
    p.set_u32(region_at + 4, 1);
    let a = p.addr(perm_at);
    p.set_u32(region_at + 8, a);
    p.set_u32(perm_at, 0x0300_0002);
    p.set_u32(perm_at + 4, 1);
    let a = p.addr(bsp_at);
    p.set_u32(perm_at + 8, a);
    p.set_u32(bsp_at, 3); // unknown_00

    // nodes: plane 0, back = leaf 0, front = none
    let node = rec(&[&h(0), &[0x00, 0x00, 0x80], &[0xff, 0xff, 0xff]]);
    p.block(bsp_at + 4, 1, &node);
    p.block(bsp_at + 0xc, 1, &rec(&[&f(0.0), &f(0.0), &f(1.0), &f(0.0)]));
    p.block(bsp_at + 0x14, 1, &rec(&[&[0, 1], &h(0)]));
    p.block(bsp_at + 0x1c, 1, &rec(&[&h(0), &h(0x8000)]));
    // no 2d nodes
    p.block(bsp_at + 0x2c, 1, &rec(&[&h(0), &h(0), &[0, 0xff], &h(0)]));
    let edges: Vec<u8> = (0..4u16)
        .flat_map(|i| {
            rec(&[
                &h(i),
                &h((i + 1) % 4),
                &h((i + 1) % 4),
                &h((i + 3) % 4),
                &h(0),
                &h(0xffff),
            ])
        })
        .collect();
    p.block(bsp_at + 0x34, 4, &edges);
    let pts = [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]];
    let verts: Vec<u8> = pts
        .iter()
        .enumerate()
        .flat_map(|(i, q)| rec(&[&f(q[0]), &f(q[1]), &f(0.0), &(i as u32).to_le_bytes()]))
        .collect();
    p.block(bsp_at + 0x3c, 4, &verts);
    // one node, one pathfinding sphere
    p.block(
        0x2c,
        1,
        &rec(&[
            &0x0400_0010u32.to_le_bytes(),
            &h(0),
            &h(0xffff),
            &h(0xffff),
            &h(0xffff),
        ]),
    );
    p.block(
        0x24,
        1,
        &rec(&[
            &(-1i16).to_le_bytes(),
            &h(0),
            &f(0.0),
            &f(0.0),
            &f(0.5),
            &f(0.5),
        ]),
    );
    p.buf
}
fn decode(b: &[u8]) -> Collision {
    Collision::decode(b, BASE).unwrap()
}

#[test]
fn quad_decodes_every_table() {
    let c = decode(&quad_coll());
    assert_eq!(c.materials, vec![StringId(0x0500_0912)]);
    assert_eq!((c.regions.len(), c.regions[0].permutations.len()), (1, 1));
    let b = &c.regions[0].permutations[0].bsps[0];
    assert_eq!(b.unknown_00, 3);
    assert_eq!(
        b.nodes,
        vec![Node3d {
            plane: 0,
            back: Child::Leaf(0),
            front: Child::None
        }]
    );
    assert_eq!(b.planes, vec![Plane([0.0, 0.0, 1.0, 0.0])]);
    assert_eq!(
        b.leaves,
        vec![Leaf {
            flags: 0,
            ref_count: 1,
            first_ref: 0
        }]
    );
    assert_eq!(b.refs[0].target(), Child2d::Surface(0));
    assert!(b.nodes_2d.is_empty());
    assert_eq!(b.surfaces[0].breakable_surface, 0xff);
    assert_eq!((b.edges.len(), b.vertices.len()), (4, 4));
    assert_eq!(b.surface_ring(0), vec![0, 1, 2, 3]);
    assert_eq!(c.pathfinding_spheres[0].radius, 0.5);
    assert_eq!(c.nodes[0].parent, -1);
    assert_eq!(c.validate(), Vec::<String>::new());
}
#[test]
fn quad_exports_obj_triangles() {
    let obj = decode(&quad_coll()).to_obj().unwrap();
    assert_eq!(obj.lines().filter(|l| l.starts_with("v ")).count(), 4);
    assert_eq!(obj.lines().filter(|l| l.starts_with("f ")).count(), 2);
    assert!(obj.contains("f 1 2 3") && obj.contains("f 1 3 4"));
}
#[test]
fn child_encoding_matches_the_engine() {
    // child = (i32 read at +1) >> 8: bit 23 is the leaf flag, all ones is none.
    assert_eq!(Child::from_raw24(0xff_ffff), Child::None);
    assert_eq!(Child::from_raw24(0x80_0005), Child::Leaf(5));
    assert_eq!(Child::from_raw24(0x00_0007), Child::Node(7));
    for c in [Child::None, Child::Node(0x1234), Child::Leaf(0x7f_fffe)] {
        assert_eq!(Child::from_raw24(c.raw24()), c);
    }
}
#[test]
fn zero_count_blocks_ignore_their_address() {
    let mut b = quad_coll();
    b[0x24..0x2c].copy_from_slice(&[0, 0, 0, 0, 0xde, 0xad, 0xbe, 0xef]);
    assert!(decode(&b).pathfinding_spheres.is_empty());
}
#[test]
fn out_of_payload_blocks_are_errors_not_panics() {
    let mut b = quad_coll();
    b[0x2c..0x30].copy_from_slice(&0x7fff_ffffu32.to_le_bytes()); // huge node count
    assert!(Collision::decode(&b, BASE).is_err());
    let mut b = quad_coll();
    b[0x30..0x34].copy_from_slice(&0x1000u32.to_le_bytes()); // address below base
    assert!(Collision::decode(&b, BASE).is_err());
    assert!(matches!(
        Collision::decode(&b[..0x20], BASE),
        Err(Error::RecordSize { .. })
    ));
}
#[test]
fn aliased_regions_hit_the_decode_budget() {
    // 1024 regions all claiming the same (large) permutation run: the work done
    // must be bounded by the payload size, not by count products.
    let mut p = Payload::new(COLL_ROOT_SIZE);
    let big = p.push(&vec![0u8; 0x14 * 1000]);
    let zeros = p.push(&vec![0u8; BSP_SIZE * 100]);
    let mut regions = Vec::new();
    for _ in 0..3000 {
        regions.extend(rec(&[
            &0u32.to_le_bytes(),
            &1000u32.to_le_bytes(),
            &p.addr(big).to_le_bytes(),
        ]));
    }
    p.block(0x1c, 3000, &regions);
    for i in 0..1000 {
        // every permutation claims the same 100 (empty) BSPs
        let at = big + i * 0x14;
        p.set_u32(at + 4, 100);
        let a = p.addr(zeros);
        p.set_u32(at + 8, a);
    }
    assert!(matches!(
        Collision::decode(&p.buf, BASE),
        Err(Error::Invalid("aliased blocks exceed decode budget"))
    ));
}
fn mutate(f: impl FnOnce(&mut Bsp)) -> Vec<String> {
    let mut c = decode(&quad_coll());
    f(&mut c.regions[0].permutations[0].bsps[0]);
    c.validate()
}
#[test]
fn validation_reports_each_defect() {
    let has = |v: Vec<String>, s: &str| assert!(v.iter().any(|m| m.contains(s)), "{s}: {v:?}");
    has(mutate(|b| b.planes[0].0 = [0.0, 0.0, 2.0, 0.0]), "not unit");
    has(mutate(|b| b.planes[0].0[0] = f32::NAN), "not unit");
    has(mutate(|b| b.nodes[0].plane = 9), "plane 9 out of range");
    has(
        mutate(|b| b.nodes[0].front = Child::Leaf(4)),
        "leaf 4 out of range",
    );
    has(
        mutate(|b| b.nodes[0].front = Child::Node(7)),
        "child node 7 out of range",
    );
    has(mutate(|b| b.nodes[0].front = Child::Node(0)), "cycle");
    has(mutate(|b| b.leaves[0].ref_count = 5), "reference run");
    has(
        mutate(|b| b.refs[0].node = 0x8001),
        "surface 1 out of range",
    );
    has(mutate(|b| b.refs[0].node = 0), "2d node 0 out of range");
    has(mutate(|b| b.refs[0].plane = 3), "plane 3 out of range");
    has(
        mutate(|b| b.surfaces[0].material = 4),
        "material 4 out of range",
    );
    has(
        mutate(|b| b.surfaces[0].first_edge = 9),
        "first edge 9 out of range",
    );
    has(
        mutate(|b| b.edges[2].start_vertex = 8),
        "vertex 8 out of range",
    );
    has(
        mutate(|b| b.edges[2].forward_edge = 8),
        "edge 8 out of range",
    );
    has(
        mutate(|b| b.edges[2].left_surface = 3),
        "surface 3 out of range",
    );
    has(mutate(|b| b.vertices[0].edge = 9), "edge 9 out of range");
    has(mutate(|b| b.vertices[1].point[2] = 5.0), "off its plane");
    has(mutate(|b| b.edges[1].forward_edge = 1), "does not close");
    has(
        mutate(|b| b.edges[3].left_surface = 0xffff),
        "does not close",
    );
}
#[test]
fn two_dimensional_trees_are_checked_for_cycles() {
    let has = |v: Vec<String>, s: &str| assert!(v.iter().any(|m| m.contains(s)), "{s}: {v:?}");
    let node = |back, front| Node2d {
        plane: [1.0, 0.0, 0.0],
        back,
        front,
    };
    has(
        mutate(|b| {
            b.nodes_2d = vec![
                node(Child2d::Node(1), Child2d::Surface(0)),
                node(Child2d::Node(0), Child2d::Surface(0)),
            ]
        }),
        "2d tree has a cycle",
    );
    let ok = mutate(|b| {
        b.nodes_2d = vec![
            node(Child2d::Surface(0), Child2d::Node(1)),
            node(Child2d::Surface(0), Child2d::Surface(0)),
        ]
    });
    assert!(ok.is_empty(), "{ok:?}");
    // A shared child (DAG) is not a cycle.
    let ok = mutate(|b| {
        b.nodes = vec![
            Node3d {
                plane: 0,
                back: Child::Node(1),
                front: Child::Node(2),
            },
            Node3d {
                plane: 0,
                back: Child::Leaf(0),
                front: Child::Node(2),
            },
            Node3d {
                plane: 0,
                back: Child::Leaf(0),
                front: Child::None,
            },
        ]
    });
    assert!(ok.is_empty(), "{ok:?}");
}
#[test]
fn coll_node_and_sphere_indices_are_checked() {
    let mut c = decode(&quad_coll());
    c.nodes[0].parent = 4;
    c.pathfinding_spheres[0].node = 3;
    c.pathfinding_spheres[0].radius = f32::INFINITY;
    let v = c.validate();
    assert!(
        v.iter().any(|m| m.contains("node 0"))
            && v.iter().any(|m| m.contains("pathfinding sphere 0")),
        "{v:?}"
    );
}

// ----- phmo -----

/// A shape header with unit friction, material 0 and a mass.
fn header(mass: f32) -> Vec<u8> {
    rec(&[
        &0x0400_0095u32.to_le_bytes(),
        &h(0),
        &h(0),
        &f(1.0),
        &f(0.8),
        &f(0.4),
        &f(0.001),
        &f(mass),
        &h(0),
        &[0xff, 0],
    ])
}
fn element(size: usize, parts: &[(usize, &[u8])]) -> Vec<u8> {
    let mut v = vec![0u8; size];
    for (at, d) in parts {
        v[*at..*at + d.len()].copy_from_slice(d);
    }
    v
}
/// The eight corners of the cube [-1,1]^3 as two four-vector groups.
fn cube_groups() -> Vec<u8> {
    let corners: Vec<[f32; 3]> = (0..8)
        .map(|k| {
            [
                if k & 1 == 0 { -1.0 } else { 1.0 },
                if k & 2 == 0 { -1.0 } else { 1.0 },
                if k & 4 == 0 { -1.0 } else { 1.0 },
            ]
        })
        .collect();
    let mut out = Vec::new();
    for g in corners.chunks(4) {
        for axis in 0..3 {
            for c in g {
                out.extend(f(c[axis]));
            }
        }
    }
    out
}
fn cube_planes() -> Vec<u8> {
    let mut out = Vec::new();
    for (axis, sign) in [
        (0, 1.0),
        (0, -1.0),
        (1, 1.0),
        (1, -1.0),
        (2, 1.0),
        (2, -1.0),
    ] {
        let mut n = [0.0f32; 3];
        n[axis] = sign;
        out.extend(rec(&[&f(n[0]), &f(n[1]), &f(n[2]), &f(-1.0)]));
    }
    out
}

/// A rigid body (mass 8) over a list holding one cube polyhedron and one box.
/// `external` puts the cube's four-vector groups in the model-wide block.
fn cube_phmo(external: bool) -> Vec<u8> {
    let mut p = Payload::new(PHMO_ROOT_SIZE);
    p.set_u32(4, 8000.0f32.to_bits());
    p.block(0x40, 1, &element(12, &[(0, &0x0600_0001u32.to_le_bytes())])); // one material
    let capacity: u32 = if external { 4 } else { 3 } | 0x8000_0000;
    let mut poly = element(
        0x100,
        &[
            (0, &header(8.0)),
            (0x30, &rec(&[&f(1.0), &f(1.0), &f(1.0), &f(0.0)])),
            (0x54, &2u32.to_le_bytes()),
            (0x58, &capacity.to_le_bytes()),
            (0xf0, &8u32.to_le_bytes()),
            (0xf8, &6u32.to_le_bytes()),
        ],
    );
    if external {
        p.block(0x78, 2, &cube_groups());
    } else {
        poly[0x60..0x60 + 0x60].copy_from_slice(&cube_groups());
    }
    p.block(0x70, 1, &poly);
    p.block(0x80, 6, &cube_planes());
    let bx = element(
        0x90,
        &[
            (0, &header(2.0)),
            (0x30, &rec(&[&f(0.5), &f(0.5), &f(0.5), &f(0.1)])),
            (
                0x50,
                &rec(&[
                    &f(1.0),
                    &f(0.0),
                    &f(0.0),
                    &f(0.0),
                    &f(0.0),
                    &f(1.0),
                    &f(0.0),
                    &f(0.0),
                    &f(0.0),
                    &f(0.0),
                    &f(1.0),
                    &f(0.0),
                    &f(0.0),
                    &f(0.0),
                    &f(3.0),
                    &f(0.0),
                ]),
            ),
        ],
    );
    p.block(0x60, 1, &bx);
    // list of {polyhedron 0, box 0}, inline
    let list = element(
        0x38,
        &[
            (0x10, &2u32.to_le_bytes()),
            (0x14, &(4u32 | 0x8000_0000).to_le_bytes()),
            (
                0x18,
                &rec(&[
                    &h(4),
                    &h(0),
                    &0u32.to_le_bytes(),
                    &h(2),
                    &h(0),
                    &0u32.to_le_bytes(),
                ]),
            ),
        ],
    );
    p.block(0x90, 1, &list);
    // mopp over the list: 20 bytes of code rounds up to 32
    let mut code_block = vec![0u8; 0x30];
    code_block[0x20..0x24].copy_from_slice(&0x24u32.to_le_bytes());
    p.block(0xa8, 0x30, &code_block);
    p.block(0xa0, 1, &element(0x14, &[(0xc, &rec(&[&h(0xe), &h(0)]))]));
    // rigid body: node -1, shape list 0, mass 8, symmetric inertia
    let rb = element(
        0x90,
        &[
            (0, &(-1i16).to_le_bytes()),
            (0x38, &rec(&[&h(0xe), &h(0)])),
            (0x3c, &f(8.0)),
            (0x50, &rec(&[&f(2.0), &f(0.1), &f(0.0), &f(0.0)])),
            (0x60, &rec(&[&f(0.1), &f(3.0), &f(0.0), &f(0.0)])),
            (0x70, &rec(&[&f(0.0), &f(0.0), &f(4.0), &f(0.0)])),
        ],
    );
    p.block(0x38, 1, &rb);
    p.buf
}
fn pdecode(b: &[u8]) -> PhysicsModel {
    PhysicsModel::decode(b, BASE).unwrap()
}

#[test]
fn polyhedron_list_and_rigid_body_decode() {
    for external in [false, true] {
        let m = pdecode(&cube_phmo(external));
        assert_eq!(m.mass, 8000.0);
        assert_eq!(
            (
                m.rigid_bodies.len(),
                m.polyhedra.len(),
                m.boxes.len(),
                m.lists.len(),
                m.mopps.len()
            ),
            (1, 1, 1, 1, 1)
        );
        let r = &m.rigid_bodies[0];
        assert_eq!((r.node, r.shape.kind, r.mass), (-1, ShapeType::List, 8.0));
        assert_eq!(r.inertia_tensor[1], [0.1, 3.0, 0.0]);
        let c = &m.polyhedra[0];
        assert_eq!((c.vertices.len(), c.planes.len(), c.group_count), (8, 6, 2));
        assert_eq!(c.vertices[0], [-1.0, -1.0, -1.0]);
        assert_eq!(c.vertices[7], [1.0, 1.0, 1.0]);
        assert_eq!(
            m.lists[0]
                .children
                .iter()
                .map(|c| c.shape.kind)
                .collect::<Vec<_>>(),
            [ShapeType::Polyhedron, ShapeType::Box]
        );
        assert_eq!(m.boxes[0].transform.translation, [0.0, 0.0, 3.0]);
        assert_eq!((m.mopps[0].code_size, m.mopp_code.len()), (0x24, 0x30));
        assert_eq!(m.validate(), Vec::<String>::new(), "external={external}");
    }
}
#[test]
fn hull_obj_has_two_triangles_per_cube_face() {
    let obj = pdecode(&cube_phmo(false)).to_obj().unwrap();
    let faces = obj.lines().filter(|l| l.starts_with("f ")).count();
    // 6 faces x 2 (hull) + 12 (box)
    assert_eq!(faces, 12 + 12);
    assert!(obj.contains("o polyhedron0") && obj.contains("o box0"));
}
fn pmutate(f: impl FnOnce(&mut PhysicsModel)) -> Vec<String> {
    let mut m = pdecode(&cube_phmo(true));
    f(&mut m);
    m.validate()
}
#[test]
fn physics_validation_reports_each_defect() {
    let has = |v: Vec<String>, s: &str| assert!(v.iter().any(|m| m.contains(s)), "{s}: {v:?}");
    has(
        pmutate(|m| m.rigid_bodies[0].shape.index = 3),
        "index 3 out of range",
    );
    has(
        pmutate(|m| m.rigid_bodies[0].shape.kind = ShapeType::Unknown(9)),
        "unknown shape type 14",
    );
    has(pmutate(|m| m.rigid_bodies[0].mass = f32::NAN), "bad mass");
    has(pmutate(|m| m.rigid_bodies[0].mass = -1.0), "bad mass");
    has(
        pmutate(|m| m.rigid_bodies[0].inertia_tensor[0][1] = 9.0),
        "not symmetric",
    );
    has(
        pmutate(|m| m.rigid_bodies[0].node = 5),
        "node 5 out of range",
    );
    has(
        pmutate(|m| m.polyhedra[0].header.material = 3),
        "material 3 out of range",
    );
    has(pmutate(|m| m.polyhedra[0].planes[0][0] = 2.0), "not unit");
    has(
        pmutate(|m| m.polyhedra[0].vertices[0] = [9.0, 0.0, 0.0]),
        "outside plane",
    );
    has(
        pmutate(|m| m.polyhedra[0].planes[0][3] = -5.0),
        "touches no vertex",
    );
    has(pmutate(|m| m.polyhedra[0].vertex_count = 3), "vertex count");
    has(pmutate(|m| m.plane_equations = 2), "plane block");
    has(pmutate(|m| m.four_vector_groups = 1), "four-vector block");
    has(pmutate(|m| m.list_children = 3), "child block"); // inline children are not counted
    has(
        pmutate(|m| {
            m.spheres.push(Sphere {
                header: m.polyhedra[0].header,
                radius: -1.0,
                transform: m.boxes[0].transform,
            })
        }),
        "bad radius",
    );
    has(
        pmutate(|m| m.boxes[0].half_extents[1] = -1.0),
        "half extents",
    );
    has(pmutate(|m| m.mopp_code.truncate(16)), "mopp code block");
    has(pmutate(|m| m.mopps[0].child.index = 4), "mopp 0");
}
#[test]
fn nested_lists_must_not_cycle() {
    let list = |idx| List {
        capacity: 4,
        children: vec![ListChild {
            shape: ShapeRef {
                kind: ShapeType::List,
                raw_type: 0xe,
                index: idx,
            },
            runtime: 0,
        }],
    };
    let v = pmutate(|m| m.lists = vec![list(1), list(0)]);
    assert!(
        v.iter()
            .any(|s| s.contains("compound shape graph has a cycle")),
        "{v:?}"
    );
    let v = pmutate(|m| m.lists = vec![list(0)]);
    assert!(
        v.iter()
            .any(|s| s.contains("compound shape graph has a cycle")),
        "{v:?}"
    );
}
#[test]
fn physics_block_errors_are_reported() {
    let mut b = cube_phmo(false);
    b[0x74..0x78].copy_from_slice(&0x7000_0000u32.to_le_bytes()); // polyhedra address far outside
    assert!(PhysicsModel::decode(&b, BASE).is_err());
    let mut b = cube_phmo(false);
    b[0x70..0x74].copy_from_slice(&0x7fffu32.to_le_bytes()); // absurd polyhedron count
    assert!(PhysicsModel::decode(&b, BASE).is_err());
    assert!(matches!(
        PhysicsModel::decode(&b[..0x100], BASE),
        Err(Error::RecordSize { .. })
    ));
    // an external polyhedron whose groups run past the block
    let mut b = cube_phmo(true);
    let poly = u32::from_le_bytes(b[0x74..0x78].try_into().unwrap()) - BASE;
    b[poly as usize + 0x54..poly as usize + 0x58].copy_from_slice(&9u32.to_le_bytes());
    assert!(PhysicsModel::decode(&b, BASE).is_err());
}
#[test]
fn shape_type_numbers_follow_the_engine_switch() {
    let kinds = [
        (0, ShapeType::Sphere),
        (1, ShapeType::Pill),
        (2, ShapeType::Box),
        (3, ShapeType::Triangle),
        (4, ShapeType::Polyhedron),
        (5, ShapeType::Type5),
        (6, ShapeType::Type6),
        (0xe, ShapeType::List),
        (0xf, ShapeType::Mopp),
    ];
    for (n, k) in kinds {
        assert_eq!(ShapeType::from_u16(n), k);
    }
    assert_eq!(ShapeType::from_u16(7), ShapeType::Unknown(7));
}

// ----- review regressions -----

fn obj_zs(obj: &str) -> Vec<f32> {
    obj.lines()
        .filter_map(|l| l.strip_prefix("v "))
        .map(|l| l.split(' ').nth(2).unwrap().parse().unwrap())
        .collect()
}

#[test]
fn looping_rings_are_budgeted_in_validation_and_export() {
    // Every surface's ring loops over two edges that never return to its first
    // edge: surfaces x edges steps unless the walk is budgeted.
    let hostile = |b: &mut Bsp| {
        let n = 1000u16;
        b.surfaces = (0..n)
            .map(|s| Surface {
                plane: 0,
                first_edge: s * 3,
                flags: 0,
                breakable_surface: 0,
                material: 0,
            })
            .collect();
        b.edges = (0..n)
            .flat_map(|s| {
                let e = |next: u16, rev: u16| Edge {
                    start_vertex: 0,
                    end_vertex: 0,
                    forward_edge: next,
                    reverse_edge: rev,
                    left_surface: s,
                    right_surface: s,
                };
                [
                    e(s * 3 + 1, s * 3 + 1),
                    e(s * 3 + 2, s * 3 + 2),
                    e(s * 3 + 1, s * 3 + 1),
                ]
            })
            .collect();
    };
    let v = mutate(hostile);
    assert!(v.iter().any(|m| m.contains("walk budget")), "{v:?}");
    let mut c = decode(&quad_coll());
    hostile(&mut c.regions[0].permutations[0].bsps[0]);
    assert!(c.to_obj().is_err());
}

#[test]
fn identical_planes_do_not_multiply_hull_triangles() {
    let m = |planes: usize, verts: usize| {
        let mut m = pdecode(&cube_phmo(true));
        m.boxes.clear();
        m.polyhedra[0].vertices = (0..verts)
            .map(|i| {
                let a = i as f32 * 0.01;
                [a.cos(), a.sin(), 0.0]
            })
            .collect();
        m.polyhedra[0].planes = vec![[0.0, 0.0, 1.0, 0.0]; planes];
        m
    };
    // 256 coplanar vertices and 256 identical planes: one face, not 65,024 triangles.
    let obj = m(256, 256).to_obj().unwrap();
    assert!(obj.lines().filter(|l| l.starts_with("f ")).count() <= 2 * 256);
    // oversized hulls are refused by validation and by export
    let big = m(5000, 5000);
    assert!(
        big.validate()
            .iter()
            .any(|s| s.contains("validation budget"))
    );
    assert!(big.to_obj().is_err());
}

#[test]
fn shape_headers_carry_no_unproven_names() {
    let m = pdecode(&cube_phmo(false));
    let h = m.polyhedra[0].header;
    assert_eq!((h.material, h.unknown_08, h.unknown_0c), (0, 1.0, 0.8));
}

#[test]
fn compound_cycles_through_mopps_are_found() {
    // list 0 holds mopp 0, whose child is list 0.
    let v = pmutate(|m| {
        m.lists[0].children.push(ListChild {
            shape: ShapeRef {
                kind: ShapeType::Mopp,
                raw_type: 0xf,
                index: 0,
            },
            runtime: 0,
        })
    });
    assert!(
        v.iter()
            .any(|s| s.contains("compound shape graph has a cycle")),
        "{v:?}"
    );
}

#[test]
fn type6_and_mopp_children_are_checked_like_the_engine_uses_them() {
    let v = pmutate(|m| {
        m.type6.push(Type6Shape {
            child: ShapeRef {
                kind: ShapeType::Sphere,
                raw_type: 0,
                index: 65535,
            },
            bytes: vec![0; 0x20],
        })
    });
    assert!(v.iter().any(|s| s.contains("type-6 shape 0")), "{v:?}");
    // the engine indexes the list block unconditionally for a mopp child
    let v = pmutate(|m| {
        m.mopps[0].child = ShapeRef {
            kind: ShapeType::Polyhedron,
            raw_type: 4,
            index: 0,
        }
    });
    assert!(v.iter().any(|s| s.contains("not a valid list")), "{v:?}");
}

#[test]
fn non_finite_numbers_fail_validation() {
    let has = |v: Vec<String>, s: &str| assert!(v.iter().any(|m| m.contains(s)), "{s}: {v:?}");
    has(
        pmutate(|m| m.boxes[0].transform.translation[0] = f32::NAN),
        "non-finite",
    );
    has(pmutate(|m| m.mass = f32::NAN), "root mass");
    has(
        pmutate(|m| m.polyhedra[0].aabb_center[0] = f32::INFINITY),
        "non-finite",
    );
    has(
        pmutate(|m| m.polyhedra[0].header.unknown_0c = f32::NAN),
        "non-finite",
    );
}

#[test]
fn mopp_runs_are_bounded_independently() {
    let code_at =
        |b: &[u8]| (u32::from_le_bytes(b[0xac..0xb0].try_into().unwrap()) - BASE) as usize;
    for size in [0x10u32, 0x40, u32::MAX] {
        let mut b = cube_phmo(false);
        let at = code_at(&b) + 0x20;
        b[at..at + 4].copy_from_slice(&size.to_le_bytes());
        assert!(PhysicsModel::decode(&b, BASE).is_err(), "size {size:#x}");
    }
}

#[test]
fn capsules_follow_their_end_points() {
    let mut m = pdecode(&cube_phmo(false));
    let header = m.polyhedra[0].header;
    m.polyhedra.clear();
    m.boxes.clear();
    m.lists.clear();
    m.mopps.clear();
    m.rigid_bodies.clear();
    m.pills = vec![Pill {
        header,
        radius: 1.0,
        bottom: [0.0, 0.0, 0.0],
        top: [0.0, 0.0, -10.0],
    }];
    let zs = obj_zs(&m.to_obj().unwrap());
    let (lo, hi) = zs
        .iter()
        .fold((f32::MAX, f32::MIN), |(l, h), z| (l.min(*z), h.max(*z)));
    assert!(
        (lo + 11.0).abs() < 1e-4 && (hi - 1.0).abs() < 1e-4,
        "{lo} {hi}"
    );
}

#[test]
fn type6_shapes_take_part_in_cycle_detection() {
    let t6 = |index| Type6Shape {
        child: ShapeRef {
            kind: ShapeType::Type6,
            raw_type: 6,
            index,
        },
        bytes: vec![0; 0x20],
    };
    // a type-6 shape naming itself
    let v = pmutate(|m| m.type6 = vec![t6(0)]);
    assert!(
        v.iter()
            .any(|s| s.contains("compound shape graph has a cycle")),
        "{v:?}"
    );
    // two type-6 shapes naming each other
    let v = pmutate(|m| m.type6 = vec![t6(1), t6(0)]);
    assert!(
        v.iter()
            .any(|s| s.contains("compound shape graph has a cycle")),
        "{v:?}"
    );
    // a chain that ends is fine
    let v = pmutate(|m| {
        m.type6 = vec![
            t6(1),
            Type6Shape {
                child: ShapeRef {
                    kind: ShapeType::Polyhedron,
                    raw_type: 4,
                    index: 0,
                },
                bytes: vec![0; 0x20],
            },
        ]
    });
    assert!(v.is_empty(), "{v:?}");
}
