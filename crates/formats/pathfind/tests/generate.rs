//! Generator, comparison and block-tree tests on invented collision
//! geometry. The disc test at the end is #[ignore] and reads the user's own
//! disc (H2_DISC or disc/halo2.iso); nothing from it is stored.
use map::build::{Block, BspField, FixupKind, TagPayload, TagSet};
use map::{CacheType, FourCC, Map};
use pathfind::compare::compare;
use pathfind::generate::{Options, generate, generate_with_surfaces, surface_edges};
use pathfind::geom::{V3, is_convex_xy, newell};
use std::io::Cursor;

#[path = "../examples/common/disc.rs"]
#[allow(dead_code)]
mod disc;
use tags::collision::{Bsp, Edge, Plane, Surface, Vertex};
use tags::geometry::TagView;
use tags::pathfinding::{PathfindingData, Ref, RefTarget};

/// A collision BSP from polygons, with edges shared the way the engine's
/// ring rule expects: an edge's left surface walks it start -> end.
fn bsp_from(points: &[V3], polys: &[&[u16]]) -> Bsp {
    let mut bsp = Bsp {
        vertices: points
            .iter()
            .map(|&p| Vertex { point: p, edge: 0 })
            .collect(),
        ..Bsp::default()
    };
    let mut ring_edges: Vec<Vec<usize>> = Vec::new();
    for (s, poly) in polys.iter().enumerate() {
        let pts: Vec<V3> = poly.iter().map(|&i| points[usize::from(i)]).collect();
        let n = newell(&pts);
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        let n = [n[0] / len, n[1] / len, n[2] / len];
        let d = n[0] * pts[0][0] + n[1] * pts[0][1] + n[2] * pts[0][2];
        bsp.planes.push(Plane([n[0], n[1], n[2], d]));
        let mut mine = Vec::new();
        for k in 0..poly.len() {
            let (u, v) = (poly[k], poly[(k + 1) % poly.len()]);
            let e = match bsp
                .edges
                .iter()
                .position(|e| e.start_vertex == v && e.end_vertex == u)
            {
                Some(e) => {
                    bsp.edges[e].right_surface = s as u16;
                    e
                }
                None => {
                    bsp.edges.push(Edge {
                        start_vertex: u,
                        end_vertex: v,
                        forward_edge: 0,
                        reverse_edge: 0,
                        left_surface: s as u16,
                        right_surface: u16::MAX,
                    });
                    bsp.edges.len() - 1
                }
            };
            mine.push(e);
        }
        bsp.surfaces.push(Surface {
            plane: s as u16,
            first_edge: mine[0] as u16,
            flags: 0,
            breakable_surface: 0xff,
            material: 0,
        });
        ring_edges.push(mine);
    }
    for (s, ring) in ring_edges.iter().enumerate() {
        for k in 0..ring.len() {
            let (e, next) = (ring[k], ring[(k + 1) % ring.len()] as u16);
            if usize::from(bsp.edges[e].left_surface) == s {
                bsp.edges[e].forward_edge = next;
            } else {
                bsp.edges[e].reverse_edge = next;
            }
        }
    }
    for e in 0..bsp.edges.len() {
        if bsp.edges[e].right_surface == u16::MAX {
            bsp.edges[e].reverse_edge = e as u16;
        }
    }
    bsp
}

/// Two floor squares sharing an edge, a wall, a 63° slope, and a separate
/// raised floor square.
fn level() -> Bsp {
    let p = [
        [0.0, 0.0, 0.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 0.0],
        [0.0, 1.0, 0.0],
        [2.0, 0.0, 0.0],
        [2.0, 1.0, 0.0],
        [2.0, 0.0, 1.0],
        [2.0, 1.0, 1.0],
        [-1.0, 0.0, -2.0],
        [-1.0, 1.0, -2.0],
        [5.0, 5.0, 3.0],
        [6.0, 5.0, 3.0],
        [6.0, 6.0, 3.0],
        [5.0, 6.0, 3.0],
    ];
    bsp_from(
        &p,
        &[
            &[0, 1, 2, 3],
            &[1, 4, 5, 2],
            &[5, 4, 6, 7],
            &[0, 3, 9, 8],
            &[10, 11, 12, 13],
        ],
    )
}

#[test]
fn ring_observer_sees_nonwalkable_surfaces_in_collision_order() {
    let mut bsp = level();
    let invalid = Surface { first_edge: u16::MAX, ..bsp.surfaces[0] };
    bsp.surfaces.push(invalid);
    let opts = Options::default();
    let expected: Vec<_> = (0..bsp.surfaces.len())
        .filter_map(|surface| surface_edges(&bsp, surface))
        .collect();
    let mut rings = Vec::new();
    let data = generate_with_surfaces(&bsp, 0, 0, &opts, |ring| rings.push(ring.to_vec())).unwrap();
    assert_eq!(data, generate(&bsp, 0, 0, &opts).unwrap());
    assert_eq!(expected.len(), 5);
    assert_eq!(rings, expected);
    assert_eq!(data.sectors.len(), 3);
}

#[test]
fn walkable_surfaces_become_linked_sectors() {
    let bsp = level();
    assert!(bsp.validate(1).is_empty(), "{:?}", bsp.validate(1));
    let pf = generate(&bsp, 3, 0xdead_beef, &Options::default()).unwrap();
    assert_eq!(pf.sectors.len(), 3, "two floor squares and the raised one");
    assert_eq!(pf.refs.len(), bsp.surfaces.len());
    assert_eq!(pf.refs[0].target(), RefTarget::Sector(0));
    assert_eq!(pf.refs[1].target(), RefTarget::Sector(1));
    assert_eq!(pf.refs[2], Ref::NONE, "wall");
    assert_eq!(pf.refs[3], Ref::NONE, "63 degree slope");
    assert_eq!(pf.refs[4].target(), RefTarget::Sector(2));
    // 4 + 4 + 4 edges, one shared.
    assert_eq!(pf.links.len(), 11);
    let two_sided: Vec<_> = pf
        .links
        .iter()
        .filter(|l| l.left_sector != u16::MAX && l.right_sector != u16::MAX)
        .collect();
    assert_eq!(two_sided.len(), 1);
    assert_eq!(
        (two_sided[0].left_sector, two_sided[0].right_sector),
        (0, 1)
    );
    assert!(pf.validate(Some(bsp.surfaces.len())).is_empty());
    assert_eq!(pf.instance_refs, vec![0xffff; 3]);
    assert_eq!(pf.checksum, 0xdead_beef);
    for s in 0..pf.sectors.len() {
        let poly: Vec<V3> = pf
            .sector_polygon(s)
            .unwrap()
            .iter()
            .map(|&v| pf.vertices[v])
            .collect();
        assert_eq!(poly.len(), 4);
        assert!(
            is_convex_xy(&poly, true),
            "sector {s} is convex and CCW from above"
        );
    }
    // The shared floor is one component; the raised square is apart.
    let c = pathfind::compare::components(&pf);
    assert_eq!(c[0], c[1]);
    assert_ne!(c[0], c[2]);
}

#[test]
fn slope_limit_is_an_option() {
    let bsp = level();
    let steep = Options {
        min_normal_z: 0.4,
        ..Options::default()
    };
    let pf = generate(&bsp, 0, 0, &steep).unwrap();
    assert_eq!(pf.sectors.len(), 4);
    assert_eq!(pf.refs[3].target(), RefTarget::Sector(2));
    assert!(pf.validate(Some(bsp.surfaces.len())).is_empty());
}

#[test]
fn comparing_with_itself_is_perfect_and_a_hole_is_seen() {
    let bsp = level();
    let pf = generate(&bsp, 0, 0, &Options::default()).unwrap();
    let m = compare(&bsp, &pf, &pf);
    assert_eq!(m.coverage(), 1.0);
    assert_eq!(m.excess(), 0.0);
    assert_eq!((m.misplaced_a, m.misplaced_b), (0, 0));
    assert_eq!(m.reach_agreement(), 1.0);
    assert_eq!(m.connect_both, 1);

    // Drop the second floor square's ref: its area is retail-only.
    let mut holed = pf.clone();
    holed.refs[1] = Ref::NONE;
    let m = compare(&bsp, &pf, &holed);
    assert!((m.coverage() - 2.0 / 3.0).abs() < 1e-6, "{}", m.coverage());
}

/// Lay a `Block` tree out the way the builder documents it, by putting it in
/// a built map and reading the BSP back through `map::Map`.
fn round_trip(pf: &PathfindingData) -> PathfindingData {
    let cc = |s: &[u8; 4]| FourCC(*s);
    let mut set = TagSet::new(CacheType::Campaign, "pf", "invented\\scenarios\\pf");
    for g in [b"matg", b"scnr", b"sbsp", b"ltmp"] {
        set.add_group(cc(g), None, None);
    }
    set.strings = vec![Some(String::new())];
    let globals = set
        .add_tag(
            cc(b"matg"),
            "globals\\globals",
            TagPayload::Built(Block::zeroed(0x40)),
        )
        .unwrap();
    let scenario = set
        .add_tag(cc(b"scnr"), "invented\\scenarios\\pf", TagPayload::Empty)
        .unwrap();
    let mut sbsp = Block::zeroed(0x240);
    sbsp.block(0xc4, 1, pathfind::build::element(pf).unwrap());
    let bsp = set
        .add_tag(cc(b"sbsp"), "invented\\pf", TagPayload::Built(sbsp))
        .unwrap();
    let lightmap = set
        .add_tag(
            cc(b"ltmp"),
            "invented\\pf",
            TagPayload::Built(Block::zeroed(0x10)),
        )
        .unwrap();
    let section = set.add_bsp(bsp, Some(lightmap));
    let mut d = Block::zeroed(0x44);
    for (o, field) in [
        (0, BspField::FileOffset),
        (4, BspField::Size),
        (8, BspField::Address),
    ] {
        d.fixup(o, FixupKind::Bsp { section, field });
    }
    d.tag_id(0x14, bsp).tag_id(0x1c, lightmap);
    let mut scen = Block::zeroed(0x3e0);
    scen.block(0x210, 1, d);
    set.tag_mut(scenario).unwrap().payload = TagPayload::Built(scen);
    set.scenario = scenario;
    set.globals = globals;
    let (bytes, _) = set.build().unwrap();
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let location = map.tag(bsp).unwrap().location.unwrap();
    let payload = map.read_tag_bytes(bsp).unwrap();
    PathfindingData::from_sbsp(&TagView::new(&payload, location.address))
        .unwrap()
        .expect("pathfinding element present")
}

#[test]
fn generated_data_survives_the_map_builder() {
    let bsp = level();
    let pf = generate(&bsp, 2, 0x1234_5678, &Options::default()).unwrap();
    let back = round_trip(&pf);
    assert_eq!(back, pf);
    assert!(back.validate(Some(bsp.surfaces.len())).is_empty());
}

#[test]
fn the_builder_refuses_undescribed_children() {
    let bsp = level();
    let mut pf = generate(&bsp, 0, 0, &Options::default()).unwrap();
    pf.object_refs
        .push(vec![0; PathfindingData::OBJECT_REF_SIZE]);
    assert!(pathfind::build::element(&pf).is_err());
    pf.object_refs.clear();
    pf.user_hints[0][0x18] = 1;
    assert!(pathfind::build::element(&pf).is_err());
}

#[test]
fn malformed_rings_are_skipped_not_walked_forever() {
    let mut bsp = level();
    // Break surface 0's ring: its first edge now points at a stranger.
    let e = usize::from(bsp.surfaces[0].first_edge);
    bsp.edges[e].forward_edge = bsp.surfaces[4].first_edge;
    let pf = generate(&bsp, 0, 0, &Options::default()).unwrap();
    assert_eq!(pf.sectors.len(), 2);
    assert_eq!(pf.refs[0], Ref::NONE);
    assert!(pf.validate(Some(bsp.surfaces.len())).is_empty());
}

/// Retail levels: generated data is valid and covers the retail walkable
/// area. Run with `cargo test -p pathfind -- --ignored`.
#[test]
#[ignore = "reads the user's own disc"]
fn retail_levels_are_covered() {
    for (name, min_coverage) in [
        ("01a_tutorial", 0.99),
        ("03a_oldmombasa", 0.99),
        ("05a_deltaapproach", 0.99),
    ] {
        let mut map = disc::open_map(name).unwrap();
        let n = disc::scenario_bsps(&mut map).unwrap().len();
        for i in 0..n {
            let (bsp, retail, _, _) = disc::load_bsp(&mut map, i).unwrap();
            let Some(retail) = retail else { continue };
            assert!(
                retail
                    .sectors
                    .iter()
                    .enumerate()
                    .all(|(s, _)| retail.sector_ring(s).is_some())
            );
            let g = generate(
                &bsp,
                retail.instance_refs.len(),
                retail.checksum,
                &Options::default(),
            )
            .unwrap();
            assert!(
                g.validate(Some(bsp.surfaces.len())).is_empty(),
                "{name} {i}"
            );
            let m = compare(&bsp, &retail, &g);
            assert!(m.coverage() >= min_coverage, "{name} {i}: {}", m.coverage());
            assert_eq!(m.misplaced_b, 0, "{name} {i}");
            assert_eq!(round_trip(&g), g);
        }
    }
}
