//! `coll` BSPs as meshes, rebuilt with their slots, saved and read back;
//! From render. Invented tags only.

use tags::StringId;
use tags::collision::{
    Bsp, BspPhysics, CollNode, Collision, PathfindingSphere, Permutation, Region,
};

use crate::coll::{
    BspAt, encode_collision, group_of, mesh_from_bsp, polygons_from_mesh, rebuild, surface_of,
};
use crate::fixtures::{cube, ramp};
use crate::from_render::{FromRender, from_render};
use crate::query::locate;
use crate::{Error, Options, TWO_SIDED, build, verify};

const BASE: u32 = 0x8010_0000;
const OWNER: u32 = 0xe123_0004;

fn coll(bsp: Bsp, slot: bool, materials: usize) -> Collision {
    let bsp_physics = if slot {
        vec![BspPhysics {
            center: [0.0; 3],
            half_extents: [1.0; 3],
            owner: OWNER,
            mopp: crate::coll::bsp_mopp(&bsp),
        }]
    } else {
        Vec::new()
    };
    Collision {
        reserved: [0; 5],
        materials: (0..materials as u32).map(|i| StringId(0x0500_0010 + i)).collect(),
        regions: vec![Region {
            name: StringId(0x0300_0001),
            permutations: vec![Permutation {
                name: StringId(0x0300_0002),
                bsps: vec![Bsp {
                    unknown_00: 3,
                    ..bsp
                }],
                bsp_physics_count: bsp_physics.len() as u32,
                bsp_physics,
            }],
        }],
        pathfinding_spheres: vec![PathfindingSphere {
            node: 0,
            flags: 0,
            center: [0.0, 0.0, 1.0],
            radius: 0.5,
        }],
        nodes: vec![CollNode {
            name: StringId(0x0300_0003),
            region: 0,
            parent: -1,
            next_sibling: -1,
            first_child: -1,
        }],
    }
}

const AT: BspAt = BspAt {
    region: 0,
    permutation: 0,
    bsp: 0,
};

/// The slot's code keys exactly the BSP's surfaces, each once.
fn slot_keys_every_surface_once(slot: &BspPhysics, bsp: &Bsp) {
    let code = mopp::Code::parse(&slot.mopp).expect("slot code parses");
    let report = mopp::check::validate(&code.bytecode).expect("slot code validates");
    let want: Vec<u32> = (0..bsp.surfaces.len() as u32).collect();
    assert_eq!(report.keys, want);
}

#[test]
fn a_coll_bsp_opens_as_a_mesh_and_compiles_back() {
    let mut polys = ramp();
    polys[1].flags = 0x8;
    polys[1].breakable_surface = 2;
    let c = build(&polys).unwrap();
    let mesh = mesh_from_bsp(&c.bsp).unwrap();
    assert_eq!(mesh.face_count(), c.bsp.surfaces.len());
    for (f, s) in mesh.face_ids().zip(0..) {
        // Faces wind counter-clockwise about their open side.
        let (plane, flip) = crate::query::surface_plane(&c.bsp, s).unwrap();
        let open = if flip { -1.0 } else { 1.0 };
        let n = mesh.face_normal(f);
        assert!((n[0] * plane[0] + n[1] * plane[1] + n[2] * plane[2]) * open > 0.9);
        let surf = c.bsp.surfaces[s];
        assert_eq!(mesh.face_material(f), surf.material);
        assert_eq!(surface_of(mesh.face_group(f)), (surf.flags, surf.breakable_surface));
    }
    let back = build(&polygons_from_mesh(&mesh)).unwrap();
    assert!(verify::check(&back.bsp, 3).is_empty());
    assert_eq!(back.bsp.surfaces, c.bsp.surfaces);
    assert_eq!(back.bsp.vertices, c.bsp.vertices);
    assert_eq!(back.bsp, c.bsp);
    assert_eq!(group_of(0x8, 2), 0x0208);
}

#[test]
fn an_edited_bsp_recompiles_into_its_permutation_with_its_slot() {
    let mut tag = coll(build(&cube([-1.0; 3], [1.0; 3])).unwrap().bsp, true, 3);
    let compiled = rebuild(&mut tag, AT, &ramp(), &Options::default()).unwrap();
    let perm = &tag.regions[0].permutations[0];
    let bsp = &perm.bsps[0];
    assert_eq!(bsp.unknown_00, 3, "the leading word is kept");
    assert_eq!(bsp.surfaces.len(), compiled.bsp.surfaces.len());
    assert!(verify::check(bsp, tag.materials.len()).is_empty());
    let slot = &perm.bsp_physics[0];
    assert_eq!(slot.owner, OWNER);
    assert_eq!(slot.center, [2.0, 1.0, 1.0]);
    assert_eq!(slot.half_extents, [2.0, 1.0, 1.0]);
    slot_keys_every_surface_once(slot, bsp);
    assert_eq!(perm.bsp_physics_count, 1);
}

#[test]
fn a_saved_coll_reads_back() {
    let mut tag = coll(build(&cube([-1.0; 3], [1.0; 3])).unwrap().bsp, true, 3);
    rebuild(&mut tag, AT, &ramp(), &Options::default()).unwrap();
    let bytes = encode_collision(&tag, BASE);
    assert!(bytes.len().is_multiple_of(16));
    let back = Collision::decode(&bytes, BASE).unwrap();
    assert_eq!(back, tag);
    assert!(back.validate().is_empty(), "{:#?}", back.validate());
    let perm = &back.regions[0].permutations[0];
    assert!(verify::check(&perm.bsps[0], back.materials.len()).is_empty());
    slot_keys_every_surface_once(&perm.bsp_physics[0], &perm.bsps[0]);
    // The saved shape is the edit: inside the ramp is solid.
    assert_eq!(locate(&perm.bsps[0], [3.0, 1.0, 0.5]), None);
    assert!(locate(&perm.bsps[0], [0.0, 0.0, -5.0]).is_some());
}

#[test]
fn a_coll_without_slots_stays_without() {
    let mut tag = coll(build(&cube([-1.0; 3], [1.0; 3])).unwrap().bsp, false, 3);
    rebuild(&mut tag, AT, &ramp(), &Options::default()).unwrap();
    let perm = &tag.regions[0].permutations[0];
    assert!(perm.bsp_physics.is_empty());
    assert_eq!(perm.bsp_physics_count, 0);
    let back = Collision::decode(&encode_collision(&tag, BASE), BASE).unwrap();
    assert_eq!(back, tag);
}

#[test]
fn a_rebuild_that_fails_changes_nothing() {
    let tag = coll(build(&cube([-1.0; 3], [1.0; 3])).unwrap().bsp, true, 1);
    let mut edited = tag.clone();
    // The ramp's slope uses material 2; the tag has one.
    let err = rebuild(&mut edited, AT, &ramp(), &Options::default()).unwrap_err();
    assert!(matches!(err, Error::Material { material: 2, materials: 1, .. }), "{err:?}");
    assert_eq!(edited, tag);
    let err = rebuild(&mut edited, BspAt { bsp: 1, ..AT }, &cube([0.0; 3], [1.0; 3]), &Options::default());
    assert!(matches!(err, Err(Error::Invalid(_))));
    assert!(matches!(rebuild(&mut edited, AT, &[], &Options::default()), Err(Error::Empty)));
    assert_eq!(edited, tag);
}

/// A cube as a render section: four vertices per face, two triangles each.
fn render_cube() -> (Vec<[f32; 3]>, Vec<[u32; 3]>) {
    let (mut positions, mut triangles) = (Vec::new(), Vec::new());
    for face in cube([-1.0; 3], [1.0; 3]) {
        let at = positions.len() as u32;
        positions.extend(face.points);
        triangles.push([at, at + 1, at + 2]);
        triangles.push([at, at + 2, at + 3]);
    }
    (positions, triangles)
}

#[test]
fn from_render_joins_a_cubes_triangles_into_its_faces() {
    let (positions, triangles) = render_cube();
    let polys = from_render(&positions, &triangles, &[], &FromRender::default());
    assert_eq!(polys.len(), 6);
    assert!(polys.iter().all(|p| p.points.len() == 4 && p.flags == 0));
    let c = build(&polys).unwrap();
    assert!(verify::check(&c.bsp, 1).is_empty());
    assert_eq!((c.bsp.vertices.len(), c.bsp.edges.len()), (8, 12));
    assert_eq!(locate(&c.bsp, [0.0; 3]), None);
    assert!(locate(&c.bsp, [0.0, 0.0, 2.0]).is_some());
}

#[test]
fn from_render_keeps_materials_apart_and_marks_a_sheet_drawn_both_ways() {
    let (positions, triangles) = render_cube();
    // Each face's second triangle in another material: nothing joins.
    let materials: Vec<u16> = (0..triangles.len()).map(|t| (t % 2) as u16).collect();
    let polys = from_render(&positions, &triangles, &materials, &FromRender::default());
    assert_eq!(polys.len(), 12);

    let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [0.0, 1.0, 0.0]];
    let triangles = [[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2], [0, 1, 2]];
    let polys = from_render(&positions, &triangles, &[], &FromRender::default());
    assert_eq!(polys.len(), 1, "{polys:?}");
    assert_eq!(polys[0].flags & TWO_SIDED, TWO_SIDED);
    assert_eq!(polys[0].points.len(), 4);
    let c = build(&polys).unwrap();
    assert!(verify::check(&c.bsp, 1).is_empty());
}
