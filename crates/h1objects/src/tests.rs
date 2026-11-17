//! Synthetic tests: invented data only.

use std::io::Cursor;

use h1map::bsp::{Child, CollisionBsp, Edge, Leaf, Node3d, Surface, Vertex};
use map::build::{Block, TagPayload, TagSet};
use map::{CacheType, FourCC, Map, TagId};

use crate::Converted;
use crate::coll::{convert_collision, sid_name};
use crate::convert::{Tree, local, template_for};
use crate::h1::{CollNode, CollRegion, Collision, Sphere};

fn one_bsp() -> CollisionBsp {
    CollisionBsp {
        nodes: vec![Node3d {
            plane: 0,
            back: Child::Solid,
            front: Child::Leaf(0),
        }],
        planes: vec![[0.0, 0.0, 1.0, 0.5]],
        leaves: vec![Leaf {
            flags: 0,
            ref_count: 0,
            first_ref: u32::MAX,
        }],
        ..Default::default()
    }
}

fn node(name: &str, region: i16, parent: i16, bsps: usize) -> CollNode {
    CollNode {
        name: name.into(),
        region,
        parent,
        next_sibling: -1,
        first_child: -1,
        bsps: vec![one_bsp(); bsps],
    }
}

#[test]
fn templates_pick_the_role_and_the_more_specific_name_first() {
    let t = |g, p| template_for(g, p).unwrap_or("-");
    assert!(t(b"weap", "weapons\\assault rifle\\assault rifle").ends_with("\\smg"));
    assert!(t(b"weap", "weapons\\pistol\\pistol").ends_with("\\magnum"));
    assert!(t(b"weap", "weapons\\plasma pistol\\plasma pistol").ends_with("\\plasma_pistol"));
    assert!(t(b"weap", "weapons\\plasma rifle\\plasma rifle").ends_with("\\plasma_rifle"));
    assert!(t(b"eqip", "powerups\\health pack").ends_with("\\over_shield"));
    assert!(t(b"eqip", "weapons\\frag grenade\\frag grenade").contains("grenade\\frag_grenade"));
    assert!(t(b"proj", "weapons\\plasma grenade\\plasma grenade").contains("plasma_grenade"));
    assert!(t(b"vehi", "vehicles\\ghost\\ghost_mp").ends_with("\\ghost"));
    assert!(t(b"bipd", "characters\\anything").contains("masterchief"));
    assert_eq!(t(b"proj", "weapons\\assault rifle\\bullet"), "-");
    assert_eq!(sid_name(" Front Left-Tire "), "front_left_tire");
}

#[test]
fn collision_moves_node_bsps_under_region_permutations() {
    // Region "body" has two permutations; node 0 holds a BSP for each,
    // node 1 (no region) none, node 2 one for the first only.
    let c = Collision {
        body_vitality: 75.0,
        shield_vitality: 75.0,
        references: Vec::new(),
        materials: vec!["Skin".into(), "".into()],
        regions: vec![CollRegion {
            name: "Body".into(),
            destroyed_effect: None,
            permutations: vec!["base".into(), "damaged".into()],
        }],
        modifiers: 0,
        spheres: vec![Sphere {
            node: -1,
            center: [0.0, 0.0, 0.2],
            radius: 0.2,
        }],
        nodes: vec![
            node("frame", 0, -1, 2),
            node("spare", -1, 0, 0),
            node("arm", 0, 0, 1),
        ],
    };
    let (conv, report) = convert_collision(&c, false).unwrap();
    assert_eq!(
        (
            report.regions,
            report.permutations,
            report.bsps,
            report.bsp_physics
        ),
        (1, 2, 3, 0)
    );
    assert_eq!(report.body_vitality, 75.0);

    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    let id = conv.install(&mut set, "invented\\coll").unwrap();
    let (bytes, _) = set.build().unwrap();
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let out = tags::collision::Collision::read(&mut map, id).unwrap();
    assert!(out.validate().is_empty(), "{:?}", out.validate());
    let name = |s: tags::StringId| {
        map.strings()
            .get(s.decode().index as usize)
            .cloned()
            .flatten()
            .unwrap_or_default()
    };
    assert_eq!(out.materials.len(), 2);
    assert_eq!(name(out.materials[0]), "skin");
    assert_eq!(out.regions.len(), 1);
    assert_eq!(name(out.regions[0].name), "body");
    let perms = &out.regions[0].permutations;
    assert_eq!(perms.len(), 2);
    assert_eq!(name(perms[1].name), "damaged");
    assert_eq!(perms[0].bsps.len(), 2, "nodes 0 and 2");
    assert_eq!(perms[1].bsps.len(), 1, "node 0 only");
    assert_eq!(perms[0].bsps[1].unknown_00, 2, "the node index");
    assert_eq!(perms[0].bsp_physics_count, 0, "a phmo object has no slots");
    let b = &perms[0].bsps[0];
    assert_eq!((b.nodes.len(), b.planes.len(), b.leaves.len()), (1, 1, 1));
    assert_eq!(b.planes[0].0, [0.0, 0.0, 1.0, 0.5]);
    assert_eq!(out.nodes.len(), 3);
    assert_eq!(name(out.nodes[2].name), "arm");
    assert_eq!((out.nodes[1].region, out.nodes[1].parent), (-1, 0));
    assert_eq!(out.pathfinding_spheres[0].node, -1);
    assert_eq!(out.pathfinding_spheres[0].radius, 0.2);
}

fn tag(group: &[u8; 4], refs: &[(u32, TagId)]) -> Converted {
    let mut root = Block::zeroed(0x10);
    for (at, id) in refs {
        root.tag_ref(*at, *id);
    }
    Converted {
        group: FourCC(*group),
        root,
        raw: Vec::new(),
    }
}

/// A tree's tags reference each other by placeholder; installing gives
/// each its datum, either in new slots or over donor slots.
#[test]
fn tree_placeholders_become_datums() {
    let tree = || Tree {
        tags: vec![
            ("invented\\a".into(), tag(b"mode", &[])),
            ("invented\\b".into(), tag(b"hlmt", &[(0, local(0))])),
            ("invented\\c".into(), tag(b"weap", &[(8, local(1))])),
        ],
        reports: Vec::new(),
    };
    let check = |set: TagSet, ids: &[TagId]| {
        let (bytes, _) = set.build().unwrap();
        let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
        let w = |map: &mut Map<Cursor<Vec<u8>>>, id: TagId, at: usize| {
            let b = map.read_tag_bytes(id).unwrap();
            u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
        };
        assert_eq!(w(&mut map, ids[1], 4), ids[0].0);
        assert_eq!(w(&mut map, ids[2], 12), ids[1].0);
        assert_eq!(map.tag(ids[2]).unwrap().group, FourCC(*b"weap"));
    };

    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    let ids = tree().install(&mut set).unwrap();
    check(set, &ids);

    // Over donors: three placeholder bitmaps give up their slots.
    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    set.add_group(FourCC(*b"bitm"), None, None);
    let donors: Vec<TagId> = (0..3)
        .map(|i| {
            set.add_tag(
                FourCC(*b"bitm"),
                &format!("invented\\donor{i}"),
                TagPayload::Built(Block::zeroed(4)),
            )
            .unwrap()
        })
        .collect();
    let ids = tree().install_over(&mut set, &donors).unwrap();
    assert_eq!(ids, donors);
    check(set, &ids);
}

/// A box from `lo` to `hi` as a Halo 1 collision BSP, solid inside: six
/// planes (outward), a node chain whose fronts are open, and the six
/// faces as surfaces with a closed edge structure (each edge has the face
/// on its left, where it runs start to end, and on its right).
fn box_bsp(lo: [f32; 3], hi: [f32; 3]) -> CollisionBsp {
    let corner = |i: usize| -> [f32; 3] {
        std::array::from_fn(|a| if i >> a & 1 == 1 { hi[a] } else { lo[a] })
    };
    let mut planes = Vec::new();
    let mut faces: Vec<Vec<usize>> = Vec::new();
    for a in 0..3 {
        for positive in [true, false] {
            let mut n = [0.0f32; 3];
            n[a] = if positive { 1.0 } else { -1.0 };
            let d = if positive { hi[a] } else { -lo[a] };
            planes.push([n[0], n[1], n[2], d]);
            // The face's corners, counter-clockwise seen from outside.
            let (u, w) = ((a + 1) % 3, (a + 2) % 3);
            let bit = |c: usize, ax: usize, on: bool| if on { c | 1 << ax } else { c & !(1 << ax) };
            let base = if positive { 1 << a } else { 0 };
            let mut ring: Vec<usize> = [(false, false), (true, false), (true, true), (false, true)]
                .iter()
                .map(|&(x, y)| bit(bit(base, u, x), w, y))
                .collect();
            if !positive {
                ring.reverse();
            }
            faces.push(ring);
        }
    }
    // Directed half-edges per face, merged into edges.
    let mut edges: Vec<Edge> = Vec::new();
    let mut slot = std::collections::HashMap::new();
    for (f, ring) in faces.iter().enumerate() {
        for k in 0..4 {
            let (u, v) = (ring[k], ring[(k + 1) % 4]);
            if let Some(&e) = slot.get(&(v, u)) {
                let e: &mut Edge = &mut edges[e];
                e.right_surface = f as u32;
            } else {
                slot.insert((u, v), edges.len());
                edges.push(Edge {
                    start_vertex: u as u32,
                    end_vertex: v as u32,
                    forward_edge: 0,
                    reverse_edge: 0,
                    left_surface: f as u32,
                    right_surface: u32::MAX,
                });
            }
        }
    }
    for (f, ring) in faces.iter().enumerate() {
        let id = |k: usize| {
            let (u, v) = (ring[k % 4], ring[(k + 1) % 4]);
            slot.get(&(u, v)).or(slot.get(&(v, u))).copied().unwrap()
        };
        for k in 0..4 {
            let (e, next) = (id(k), id(k + 1) as u32);
            if edges[e].left_surface == f as u32 {
                edges[e].forward_edge = next;
            } else {
                edges[e].reverse_edge = next;
            }
        }
    }
    let first_edge = |f: usize| {
        slot.get(&(faces[f][0], faces[f][1]))
            .or(slot.get(&(faces[f][1], faces[f][0])))
            .copied()
            .unwrap() as u32
    };
    let surfaces = (0..6)
        .map(|f| Surface {
            plane: f as u32,
            first_edge: first_edge(f),
            flags: 0,
            breakable_surface: -1,
            material: 0,
        })
        .collect();
    let vertices = (0..8)
        .map(|i| Vertex {
            point: corner(i),
            first_edge: edges
                .iter()
                .position(|e| e.start_vertex as usize == i || e.end_vertex as usize == i)
                .unwrap() as u32,
        })
        .collect();
    CollisionBsp {
        nodes: (0..6)
            .map(|i| Node3d {
                plane: i,
                front: Child::Leaf(0),
                back: if i == 5 {
                    Child::Solid
                } else {
                    Child::Node(i + 1)
                },
            })
            .collect(),
        planes,
        leaves: vec![Leaf {
            flags: 0,
            ref_count: 0,
            first_ref: u32::MAX,
        }],
        surfaces,
        edges,
        vertices,
        ..Default::default()
    }
}

/// A crate: one region, one permutation, one node holding a box. Without
/// a physics model the permutation gets one BSP physics slot: the box, the
/// `coll`'s own datum, and MOPP code keying the six faces by index.
#[test]
fn collision_without_a_phmo_gets_bsp_physics_slots() {
    let lo = [-0.5, -0.4, 0.0];
    let hi = [0.5, 0.4, 0.9];
    let src = box_bsp(lo, hi);
    assert!(src.validate().is_empty(), "{:?}", src.validate());
    let c = Collision {
        body_vitality: 0.0,
        shield_vitality: 0.0,
        references: Vec::new(),
        materials: vec!["wood".into()],
        regions: vec![CollRegion {
            name: "default".into(),
            destroyed_effect: None,
            permutations: vec!["base".into()],
        }],
        modifiers: 0,
        spheres: Vec::new(),
        nodes: vec![CollNode {
            name: "frame".into(),
            region: 0,
            parent: -1,
            next_sibling: -1,
            first_child: -1,
            bsps: vec![src.clone()],
        }],
    };
    let (conv, report) = convert_collision(&c, true).unwrap();
    assert_eq!((report.bsps, report.bsp_physics), (1, 1));
    assert!(report.mopp_bytes > 0 && report.mopp_bytes % 16 == 0);

    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    let id = conv.install(&mut set, "invented\\crate").unwrap();
    let (bytes, _) = set.build().unwrap();
    let mut map = Map::from_reader(Cursor::new(bytes)).unwrap();
    let raw = map.read_tag_bytes(id).unwrap();
    let out = tags::collision::Collision::read(&mut map, id).unwrap();
    assert!(out.validate().is_empty(), "{:?}", out.validate());
    let perm = &out.regions[0].permutations[0];
    assert_eq!(perm.bsp_physics_count, 1);
    let slot = &perm.bsp_physics[0];
    assert_eq!(slot.center, [0.0, 0.0, 0.45]);
    assert_eq!(slot.half_extents, [0.5, 0.4, 0.45]);
    assert_eq!(slot.owner, id.0, "the coll's own datum");
    // The slot array and the code are 16-aligned, as Havok's vectors need.
    let base = map
        .tags()
        .iter()
        .find(|t| t.id == id)
        .unwrap()
        .location
        .unwrap()
        .address;
    let word = |at: usize| u32::from_le_bytes(raw[at..at + 4].try_into().unwrap());
    let perms = word(word(0x20) as usize - base as usize + 8) as usize - base as usize;
    let slots = word(perms + 0x10);
    assert_eq!(slots % 16, 0);
    assert_eq!(word(slots as usize - base as usize + 0x68) % 16, 0);

    // The code keys every face once, and finds each face near its centre.
    let code = mopp::Code::parse(&slot.mopp).unwrap();
    let mut keys = mopp::check::validate(&code.bytecode).unwrap().keys;
    keys.sort();
    keys.dedup();
    assert_eq!(keys, (0..6).collect::<Vec<u32>>());
    let bsp = &perm.bsps[0];
    for f in 0..6 {
        let ring = bsp.surface_ring(f);
        let mid: [f32; 3] = std::array::from_fn(|a| {
            ring.iter().map(|&v| bsp.vertices[v].point[a]).sum::<f32>() / ring.len() as f32
        });
        let got = mopp::query_sphere(&code, mid, 0.01).unwrap();
        assert!(got.contains(&(f as u32)), "face {f}: {got:?}");
    }
    // Far from the box the code emits nothing.
    assert!(
        mopp::query_sphere(&code, [5.0, 5.0, 5.0], 0.1)
            .unwrap()
            .is_empty()
    );

    // The read-back BSP answers points and rays as Halo 1's does.
    use crate::query::{Child as Q, locate, ray};
    for (p, want) in [
        ([0.0, 0.0, 0.5], Q::Solid),
        ([0.0, 0.0, 1.0], Q::Leaf(0)),
        ([0.6, 0.0, 0.5], Q::Leaf(0)),
    ] {
        assert_eq!(locate(bsp, p), Some(want));
        assert_eq!(locate(&src, p), Some(want));
    }
    for (a, b) in [
        ([-2.0, 0.1, 0.3], [2.0, 0.1, 0.3]),
        ([0.1, -2.0, 0.6], [0.1, 2.0, 0.2]),
        ([0.0, 0.0, 3.0], [0.0, 0.0, -1.0]),
        ([2.0, 2.0, 2.0], [3.0, 3.0, 3.0]),
    ] {
        assert_eq!(ray(bsp, a, b).unwrap(), ray(&src, a, b).unwrap());
    }
    // The player's walk into the crate stops at its face (x = -0.5).
    let hit = ray(bsp, [-2.0, 0.0, 0.5], [2.0, 0.0, 0.5])
        .unwrap()
        .unwrap();
    assert!((hit * 4.0 - 2.0 - -0.5).abs() < 1e-5, "{hit}");
}

#[test]
fn hlmt_regions_select_collision_regions_by_name_then_index() {
    use crate::convert::region_map;
    let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
    let render = vec![
        ("Hull".to_string(), s(&["base", "damaged"])),
        ("lid".to_string(), s(&["closed"])),
        ("glass".to_string(), s(&["base"])),
    ];
    let coll = vec![
        ("lid".to_string(), s(&["open", "closed"])),
        ("hull".to_string(), s(&["damaged", "base"])),
    ];
    let m = region_map(&render, &coll);
    assert_eq!(m[0], (1, vec![1, 0]), "by name, case and spaces folded");
    assert_eq!(m[1], (0, vec![1]));
    // No `glass` in the coll and no third coll region: none.
    assert_eq!(m[2], (-1, vec![-1]));
    // A renamed region falls back to its index, then the first permutation.
    let m = region_map(
        &[("__base".into(), s(&["a", "b"]))],
        &[("default".into(), s(&["x"]))],
    );
    assert_eq!(m[0], (0, vec![0, 0]));
    assert!(region_map(&render, &[]).iter().all(|r| r.0 == -1));
}
