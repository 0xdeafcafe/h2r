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
    // Vehicles have no template: every one is built from its definition.
    assert_eq!(t(b"vehi", "vehicles\\ghost\\ghost_mp"), "-");
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
        hud_swaps: Vec::new(),
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

/// Rebuilt scenery: written from the definitions alone, every field the
/// conversion does not set at its default, nothing pinned.
#[test]
fn rebuilt_scenery_has_only_definition_bytes() {
    use tags::defs::{self, Defs, Target, Val};
    let obj = crate::h1::Object {
        object_type: 6,
        flags: 0,
        bounding_radius: 2.5,
        bounding_offset: [1.0, 2.0, 3.0],
        model: None,
        animation: None,
        collision: None,
        physics: None,
        modifier_shader: None,
        creation_effect: None,
        attachments: Vec::new(),
        widgets: Vec::new(),
    };
    let hlmt = map::TagId(0xe001_0002);
    let root = crate::rebuild::scenery(&obj, Some(hlmt)).unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"scen")).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    let mut want = defs::default_record(def);
    want.set("object.object_type", Val::Enum(6)).unwrap();
    want.set("object.bounding_radius", Val::Float(2.5)).unwrap();
    want.set("object.bounding_offset", Val::floats(&[1.0, 2.0, 3.0]))
        .unwrap();
    want.set(
        "object.model",
        Val::Ref {
            group: 0,
            target: Target::Id(hlmt),
        },
    )
    .unwrap();
    assert_eq!(defs::diff(def, &want, &got), Vec::new());

    // The material list: one per shader, the shader set, null elsewhere.
    let mut mode = map::build::Block::zeroed(0x84);
    crate::rebuild::mode_materials(&mut mode, &[map::TagId(0xe001_0005), map::TagId::NULL])
        .unwrap();
    let m = Defs::builtin().group(map::FourCC(*b"mode")).unwrap();
    let r = defs::from_built(m, &mode).unwrap();
    assert_eq!(
        r.at("materials[0].shader"),
        Some(&Val::Ref {
            group: 0,
            target: Target::Id(map::TagId(0xe001_0005)),
        })
    );
    assert_eq!(
        r.at("materials[1].shader"),
        Some(&Val::Ref {
            group: u32::MAX,
            target: Target::Null,
        })
    );
}

/// A rebuilt vehicle (a10's cryotube and the other Halo 1 vehicles with no
/// template): the `vehi` definition's defaults, the turret role, the
/// header and one seat per Halo 1 seat; it reads back through the
/// definition and writes the same builder tree again.
#[test]
fn rebuilt_vehicle_is_the_definition_with_seats_and_the_turret_role() {
    use crate::rebuild::{
        ROLE_VEHICLE_TYPE, TURRET_PHYSICS_ROLE, TURRET_ROLE, TURRET_SEAT_ROLE, VEHI_PHYSICS,
        VEHI_SEATS,
    };
    use tags::defs::{self, Defs, Str, Target, Val};
    let obj = crate::h1::Object {
        object_type: 1,
        flags: 0,
        bounding_radius: 0.9,
        bounding_offset: [0.0, 0.0, 0.45],
        model: None,
        animation: None,
        collision: None,
        physics: None,
        modifier_shader: None,
        creation_effect: None,
        attachments: Vec::new(),
        widgets: Vec::new(),
    };
    let seat = crate::h1::Seat {
        flags: 0,
        label: "CT-driver".into(),
        marker: "driver".into(),
        camera_marker: "camera".into(),
    };
    let hlmt = map::TagId(0xe001_0002);
    let mode = map::TagId(0xe001_0003);
    let bitmap = map::TagId(0xe001_0004);
    let predicted = [
        crate::rebuild::Predicted::RenderModel(mode),
        crate::rebuild::Predicted::Bitmap(bitmap),
    ];
    let weapon = map::TagId(0xe001_0005);
    let markers = vec!["driver".to_string(), "driver enter".to_string()];
    let seats = [crate::rebuild::seat_names(&seat, &markers)];
    let root = crate::rebuild::vehicle(&crate::rebuild::VehicleSpec {
        object: &obj,
        hlmt: Some(hlmt),
        seats: &seats,
        predicted: &predicted,
        camera_field_of_view: 1.2,
        vehicle_type: ROLE_VEHICLE_TYPE as u16,
        flags: 0xdead_00c0,
        weapons: &[weapon],
        role: None,
        points: &[],
    })
    .unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"vehi")).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    // Written again from what was read, it reads back the same and has
    // the same fixups (a reference's group word under a fixup is the
    // builder's to fill, so `from_built` reads it as 0).
    let again = defs::to_built(def, &got, &mut |_| None).unwrap();
    assert_eq!(again.fixups, root.fixups);
    assert_eq!(defs::from_built(def, &again).unwrap(), got);

    let word = |w: u32| Val::Bytes(w.to_le_bytes().to_vec());
    let mut want = defs::default_record(def);
    for (p, w) in TURRET_ROLE {
        want.set(p, word(*w)).unwrap();
    }
    want.set("unit.obje.object_type", Val::Enum(1)).unwrap();
    want.set("unit.obje.bounding_radius", Val::Float(0.9))
        .unwrap();
    want.set("unit.obje.bounding_offset", Val::floats(&[0.0, 0.0, 0.45]))
        .unwrap();
    want.set(
        "unit.obje.model",
        Val::Ref {
            group: 0,
            target: Target::Id(hlmt),
        },
    )
    .unwrap();
    want.set("vehicle_type", Val::Enum(ROLE_VEHICLE_TYPE))
        .unwrap();
    let mut s = defs::default_record(def.child(VEHI_SEATS).unwrap());
    for (p, w) in TURRET_SEAT_ROLE {
        s.set(p, word(*w)).unwrap();
    }
    // The label is the rider's mode as the converted graphs name it.
    s.set("label", Val::text("c_turret_ap_d")).unwrap();
    s.set("string_8", Val::text("driver")).unwrap();
    // Marker names as the converted model keeps them (Halo 1's).
    s.set("string_c", Val::text("driver enter")).unwrap();
    s.set("string_60", Val::text("camera")).unwrap();
    want.set(VEHI_SEATS, Val::Block(vec![s])).unwrap();
    let pr = def.child("unit.obje.predicted_resources").unwrap();
    let res = [(0xffff_0002u32, mode), (0, bitmap)]
        .iter()
        .map(|(w, id)| {
            let mut x = defs::default_record(pr);
            x.set("unknown_0", word(*w)).unwrap();
            x.set("tag", Val::Datum(Target::Id(*id))).unwrap();
            x
        })
        .collect();
    want.set("unit.obje.predicted_resources", Val::Block(res))
        .unwrap();
    let mut p = defs::default_record(def.child(VEHI_PHYSICS).unwrap());
    for (q, w) in TURRET_PHYSICS_ROLE {
        p.set(q, word(*w)).unwrap();
    }
    want.set(VEHI_PHYSICS, Val::Block(vec![p])).unwrap();
    want.set("unit.camera_field_of_view", Val::Float(1.2))
        .unwrap();
    // Halo 1's flags: the low half only (the turret tables have no high
    // half).
    want.set("vehicle_flags", Val::Int(0xc0)).unwrap();
    let mut w = defs::default_record(def.child("unit.weapons").unwrap());
    w.set(
        "weapon",
        Val::Ref {
            group: 0,
            target: Target::Id(weapon),
        },
    )
    .unwrap();
    want.set("unit.weapons", Val::Block(vec![w])).unwrap();
    assert_eq!(defs::diff(def, &want, &got), Vec::new());
    assert_eq!(
        got.at("block_1c8[0].label"),
        Some(&Val::String(Str::Text("c_turret_ap_d".into())))
    );
    // References a fresh vehicle must not point at tag 0: the two the
    // schema could not see are null references, not zero words.
    for r in ["snd_ref_284", "foot_ref_294"] {
        assert_eq!(
            got.at(r),
            Some(&Val::Ref {
                group: u32::MAX,
                target: Target::Null
            }),
            "{r}"
        );
    }
}

/// A vehicle with a role: the role's root, block and seat words, its
/// flags' high half under Halo 1's low half, and Halo 1's type over the
/// role's.
#[test]
fn rebuilt_vehicle_takes_its_types_role_and_halo_1s_type() {
    use crate::rebuild::{RoleWords, VEHI_PHYSICS, VehicleRole, VehicleSpec};
    use tags::defs::{self, Defs, Val};
    let obj = crate::h1::Object {
        object_type: 1,
        flags: 0,
        bounding_radius: 2.0,
        bounding_offset: [0.0; 3],
        model: None,
        animation: None,
        collision: None,
        physics: None,
        modifier_shader: None,
        creation_effect: None,
        attachments: Vec::new(),
        widgets: Vec::new(),
    };
    let role = VehicleRole {
        words: RoleWords {
            members: vec!["jeep".into()],
            root: vec![("unknown_210".into(), 0x43e1_0000)],
            blocks: vec![(
                VEHI_PHYSICS.into(),
                vec![vec![("unknown_48".into(), 0xbf80_0000)]],
            )],
        },
        seat: vec![("unknown_20".into(), 0x3f00_0000)],
        flags_high: 0x0012_0000,
        friction: Some(vec![("unknown_8".into(), 0x4160_0000)]),
        hover: None,
        gears: vec![vec![
            ("unknown_0".into(), 0x4795_6a00),
            ("raw_14".into(), 0x4000_1234),
        ]],
    };
    let points = [
        crate::rebuild::Point {
            marker: "tire_front_left".into(),
            region: "left_front_tire".into(),
        },
        crate::rebuild::Point {
            marker: "tire_back_left".into(),
            region: String::new(),
        },
    ];
    let seat = crate::h1::Seat {
        flags: 0,
        label: "W-driver".into(),
        marker: "driver".into(),
        camera_marker: String::new(),
    };
    let seats = [crate::rebuild::seat_names(&seat, &[])];
    let root = crate::rebuild::vehicle(&VehicleSpec {
        object: &obj,
        hlmt: None,
        seats: &seats,
        predicted: &[],
        camera_field_of_view: 1.0,
        vehicle_type: 1,
        flags: 0x0000_0cd1,
        weapons: &[],
        role: Some(&role),
        points: &points,
    })
    .unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"vehi")).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    let word = |w: u32| Some(Val::Bytes(w.to_le_bytes().to_vec()));
    assert_eq!(got.at("vehicle_type"), Some(&Val::Enum(1)));
    assert_eq!(got.at("vehicle_flags"), Some(&Val::Int(0x0012_0cd1)));
    assert_eq!(got.at("unknown_210").cloned(), word(0x43e1_0000));
    assert_eq!(
        got.at("block_2f8[0].unknown_48").cloned(),
        word(0xbf80_0000)
    );
    assert_eq!(got.at("block_1c8[0].unknown_20").cloned(), word(0x3f00_0000));
    assert_eq!(
        got.at("block_1c8[0].label"),
        Some(&Val::text("warthog_d"))
    );
    // Halo 1's powered points as friction points with the role's words;
    // the role's gear, its curve pointer kept as a word.
    assert_eq!(
        got.at("block_2f0[0].string_0"),
        Some(&Val::text("tire_front_left"))
    );
    assert_eq!(
        got.at("block_2f0[0].string_44"),
        Some(&Val::text("left_front_tire"))
    );
    assert_eq!(got.at("block_2f0[1].unknown_8").cloned(), word(0x4160_0000));
    assert!(matches!(got.at("block_2e8"), Some(Val::Block(b)) if b.is_empty()));
    assert_eq!(got.at("block_268[0].unknown_0").cloned(), word(0x4795_6a00));
    assert_eq!(
        got.at("block_268[0].raw_14"),
        Some(&Val::Raw(defs::RawVal::Word(0x4000_1234)))
    );
    let again = defs::to_built(def, &got, &mut |_| None).unwrap();
    assert_eq!(defs::from_built(def, &again).unwrap(), got);
    // Turret words stay off a vehicle with its own role.
    assert_eq!(got.at("unknown_23c").cloned(), word(0));
}

#[test]
fn device_times_become_rates_per_second() {
    use crate::rebuild::{INSTANT_RATE, device_rates};
    // a10's small door: power 2 s, accel 2 s; position 0.8 s, accel 0.2 s;
    // nothing depowered.
    let r = device_rates(&[2.0, 2.0, 0.8, 0.2, 0.0, 0.0]);
    assert_eq!(r, [0.5, 0.5, 1.25, 5.0, 0.0, INSTANT_RATE]);
}

fn h1_device(group: &[u8; 4]) -> crate::h1::Device {
    crate::h1::Device {
        group: h1map::Group::new(group),
        object: crate::h1::Object {
            object_type: 7,
            flags: 0,
            bounding_radius: 1.5,
            bounding_offset: [0.0, 0.0, 1.0],
            model: None,
            animation: None,
            collision: None,
            physics: None,
            modifier_shader: None,
            creation_effect: None,
            attachments: Vec::new(),
            widgets: Vec::new(),
        },
        flags: 1,
        times: [0.0, 0.0, 4.0, 0.0, 0.0, 0.0],
        delay_time: 0.5,
        activation_radius: 2.6,
        machine: (group == b"mach").then_some(crate::h1::Machine {
            machine_type: 2,
            flags: 3,
            door_open_time: 1.5,
            collision_response: 1,
            elevator_node: 1,
        }),
        control: (group == b"ctrl").then_some(crate::h1::Control {
            control_type: 1,
            trigger: 0,
            value: 0.0,
        }),
    }
}

#[test]
fn rebuilt_devices_are_their_definitions() {
    use crate::rebuild::{CONTROL_TYPE, FIXTURE_MACHINE_TYPE, MACHINE_TYPE, device};
    use tags::defs::{self, Defs, Val};
    let hlmt = map::TagId(0xe001_0002);
    for (h1, h2, ty, machine_type) in [
        (b"mach", b"mach", MACHINE_TYPE, Some(2)),
        (b"lifi", b"mach", MACHINE_TYPE, Some(FIXTURE_MACHINE_TYPE)),
        (b"ctrl", b"ctrl", CONTROL_TYPE, None),
    ] {
        let root = device(&h1_device(h1), Some(hlmt), &[]).unwrap();
        let def = Defs::builtin().group(map::FourCC(*h2)).unwrap();
        let got = defs::from_built(def, &root).unwrap();
        let again = defs::to_built(def, &got, &mut |_| None).unwrap();
        assert_eq!(again.fixups, root.fixups, "{}", map::FourCC(*h1));
        assert_eq!(got.at("devi.obje.object_type"), Some(&Val::Enum(ty)));
        assert_eq!(got.at("devi.position_rate"), Some(&Val::Float(0.25)));
        assert_eq!(
            got.at("devi.position_acceleration_rate"),
            Some(&Val::Float(crate::rebuild::INSTANT_RATE))
        );
        assert_eq!(got.at("devi.activation_radius"), Some(&Val::Float(2.6)));
        assert_eq!(got.at("devi.device_flags"), Some(&Val::Int(1)));
        match machine_type {
            Some(t) => assert_eq!(got.at("machine_type"), Some(&Val::Enum(t))),
            None => assert_eq!(got.at("control_type"), Some(&Val::Int(1))),
        }
        if h1 == b"mach" {
            assert_eq!(got.at("door_open_time"), Some(&Val::Float(1.5)));
            assert_eq!(got.at("elevator_node"), Some(&Val::Int(1)));
        }
        assert!(
            root.fixups
                .iter()
                .any(|f| f.offset == 0x34 && f.kind == map::build::FixupKind::TagRef(hlmt)),
            "the model"
        );
    }
}

#[test]
fn stand_in_physics_model_is_one_sphere_on_one_body() {
    use tags::defs::{self, Defs, Val};
    let root =
        crate::rebuild::sphere_physics(0.05, [0.0, 0.0, 0.05], 1000.0, "hard_metal_thin_hum")
            .unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"phmo")).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    let again = defs::to_built(def, &got, &mut |_| None).unwrap();
    assert_eq!(again.fixups.len(), root.fixups.len());
    let Some(Val::Block(bodies)) = got.at("rigid_bodies") else {
        panic!()
    };
    assert_eq!(bodies.len(), 1);
    assert_eq!(bodies[0].get("shape_type"), Some(&Val::Int(0)), "a sphere");
    assert_eq!(bodies[0].get("shape_index"), Some(&Val::Int(0)));
    assert_eq!(bodies[0].get("node"), Some(&Val::Int(-1)));
    let Some(Val::List(i)) = bodies[0].get("inertia_tensor_x") else {
        panic!()
    };
    assert_eq!(i[0], Val::Float(0.4 * 1000.0 * 0.05 * 0.05));
    let Some(Val::Block(spheres)) = got.at("spheres") else {
        panic!()
    };
    assert_eq!(spheres.len(), 1);
    assert_eq!(spheres[0].get("radius"), Some(&Val::Float(0.05)));
    let Some(Val::Block(regions)) = got.at("regions") else {
        panic!()
    };
    assert_eq!(regions.len(), 1, "the hlmt's physics region 0");
    let Some(Val::Block(materials)) = got.at("materials") else {
        panic!()
    };
    assert_eq!(materials.len(), 1, "the sphere's material 0");
}

/// An L of two boxes, `[0,2]x[0,1]x[0,1]` and `[0,1]x[1,2]x[0,1]`, as a
/// Halo 2 form collision BSP: leaves outside, missing children inside.
fn l_shape() -> tags::collision::Bsp {
    use tags::collision::{Bsp, Child as C, Node3d as N, Plane, Vertex as V};
    let planes = vec![
        Plane([0.0, 0.0, 1.0, 0.0]),
        Plane([0.0, 0.0, 1.0, 1.0]),
        Plane([1.0, 0.0, 0.0, 0.0]),
        Plane([0.0, 1.0, 0.0, 0.0]),
        Plane([0.0, 1.0, 0.0, 1.0]),
        Plane([1.0, 0.0, 0.0, 2.0]),
        Plane([1.0, 0.0, 0.0, 1.0]),
        Plane([0.0, 1.0, 0.0, 2.0]),
    ];
    let n = |plane, back, front| N { plane, back, front };
    let nodes = vec![
        n(0, C::Leaf(0), C::Node(1)),
        n(1, C::Node(2), C::Leaf(0)),
        n(2, C::Leaf(0), C::Node(3)),
        n(3, C::Leaf(0), C::Node(4)),
        n(4, C::Node(5), C::Node(6)),
        n(5, C::None, C::Leaf(0)),
        n(6, C::Node(7), C::Leaf(0)),
        n(7, C::None, C::Leaf(0)),
    ];
    let vertices = [[0.0, 0.0, 0.0], [2.0, 0.0, 0.0], [0.0, 2.0, 1.0]]
        .into_iter()
        .map(|point| V { point, edge: 0 })
        .collect();
    Bsp {
        nodes,
        planes,
        vertices,
        ..Default::default()
    }
}

fn inside(h: &crate::physics::Hull, p: [f32; 3]) -> bool {
    h.planes
        .iter()
        .all(|pl| pl[0] * p[0] + pl[1] * p[1] + pl[2] * p[2] + pl[3] <= 1e-4)
}

#[test]
fn solid_cells_are_the_bsp_s_solid_space() {
    let cells = crate::physics::solid_cells(&l_shape());
    assert_eq!(cells.len(), 2);
    let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
    assert!(near(cells[0].volume, 2.0) && near(cells[1].volume, 1.0));
    assert_eq!((cells[0].vertices.len(), cells[0].planes.len()), (8, 6));
    assert!(near(cells[0].centroid[0], 1.0) && near(cells[1].centroid[1], 1.5));
    // A 2x1x1 box of unit density: Ixx = m (1 + 1) / 12.
    assert!(near(cells[0].inertia[0][0], 2.0 * 2.0 / 12.0));
    assert!(near(cells[0].inertia[1][1], 2.0 * 5.0 / 12.0));
    for h in &cells {
        for v in &h.vertices {
            assert!(inside(h, *v), "every corner inside its own planes");
        }
    }
    let solid = |p: [f32; 3]| cells.iter().any(|h| inside(h, p));
    assert!(solid([1.5, 0.5, 0.5]) && solid([0.5, 1.5, 0.5]));
    assert!(!solid([1.5, 1.5, 0.5]), "the L's notch is open");
    // The model of the two: mass, centre and parallel-axis inertia.
    let (m, c, i) = crate::physics::mass_properties(&cells, 10.0);
    assert!(near(m, 30.0));
    assert!(near(c[0], (2.0 * 1.0 + 0.5) / 3.0) && near(c[2], 0.5));
    assert!((i[0][1] - i[1][0]).abs() < 1e-4, "symmetric");
    // One enclosing piece holds every corner and cuts the notch's corner.
    let e = crate::physics::enclosing(&cells).unwrap();
    for h in &cells {
        for v in &h.vertices {
            assert!(inside(&e, *v));
        }
    }
    assert!(!inside(&e, [1.9, 1.9, 0.5]));
}

#[test]
fn built_physics_model_round_trips_with_every_piece() {
    use crate::physics::{Hull, Transform};
    use crate::rebuild::{Body, PhysNode, physics_model};
    use tags::defs::{self, Defs, Val};
    let cells = crate::physics::solid_cells(&l_shape());
    // A 26-sided piece: more corners than the inline twelve.
    let mut planes = Vec::new();
    for x in -1i32..=1 {
        for y in -1i32..=1 {
            for z in -1i32..=1 {
                if (x, y, z) != (0, 0, 0) {
                    let n = [f64::from(x), f64::from(y), f64::from(z)];
                    let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
                    planes.push(([n[0] / l, n[1] / l, n[2] / l], -1.0));
                }
            }
        }
    }
    let round = crate::physics::hull(&planes).unwrap();
    assert!(round.vertices.len() > 12);
    let moved = |h: &Hull, x: f32| {
        h.transformed(&Transform {
            t: [x, 0.0, 0.0],
            ..Transform::IDENTITY
        })
    };
    let five: Vec<Hull> = (0..5).map(|k| moved(&cells[1], 3.0 * k as f32)).collect();
    let bodies = [
        Body {
            node: 1,
            pieces: vec![cells[0].clone()],
            mass: 100.0,
        },
        Body {
            node: 2,
            pieces: vec![round.clone(), cells[1].clone()],
            mass: 50.0,
        },
        Body {
            node: 0,
            pieces: five,
            mass: 10.0,
        },
    ];
    let node = |name: &str, parent, next_sibling, first_child| PhysNode {
        name: name.into(),
        parent,
        next_sibling,
        first_child,
    };
    let nodes = [
        node("root", -1, -1, 1),
        node("a", 0, 2, -1),
        node("b", 0, -1, -1),
    ];
    let root = physics_model(&bodies, &nodes, crate::rebuild::DEVICE_BODY).unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"phmo")).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    let again = defs::to_built(def, &got, &mut |_| None).unwrap();
    assert_eq!(again.bytes, root.bytes);
    assert_eq!(again.fixups.len(), root.fixups.len());
    let block = |path: &str| match got.at(path) {
        Some(Val::Block(b)) => b.clone(),
        other => panic!("{path}: {other:?}"),
    };
    let int = |r: &tags::defs::Record, f: &str| match r.get(f) {
        Some(Val::Int(i)) => *i,
        other => panic!("{f}: {other:?}"),
    };
    let rbs = block("rigid_bodies");
    assert_eq!(rbs.len(), 3);
    assert_eq!(
        (int(&rbs[0], "shape_type"), int(&rbs[0], "shape_index")),
        (4, 0)
    );
    assert_eq!(
        (int(&rbs[1], "shape_type"), int(&rbs[1], "shape_index")),
        (0xe, 0)
    );
    assert_eq!(
        (int(&rbs[2], "shape_type"), int(&rbs[2], "shape_index")),
        (0xe, 1)
    );
    assert_eq!(
        rbs.iter().map(|b| int(b, "node")).collect::<Vec<_>>(),
        [1, 2, 0]
    );
    assert_eq!(got.at("mass"), Some(&Val::Float(160.0)));
    let polys = block("polyhedra");
    assert_eq!(polys.len(), 1 + 2 + 5);
    let lists = block("lists");
    assert_eq!(
        int(&lists[0], "child_capacity"),
        0x8000_0004,
        "two children inline"
    );
    assert_eq!(int(&lists[0], "inline_child_index_1"), 2);
    assert_eq!(
        int(&lists[1], "child_capacity"),
        0x8000_0005,
        "five from the block"
    );
    let children = block("list_children");
    assert_eq!(
        children
            .iter()
            .map(|c| int(c, "shape_index"))
            .collect::<Vec<_>>(),
        [3, 4, 5, 6, 7]
    );
    // The big piece takes its corners from the root block, the rest inline;
    // every piece's planes come from the root block in order.
    let groups = round.vertices.len().div_ceil(4);
    assert_eq!(
        int(&polys[1], "four_vector_capacity"),
        0x8000_0000 | groups as i64
    );
    assert_eq!(block("four_vectors").len(), groups);
    assert_eq!(int(&polys[0], "four_vector_capacity"), 0x8000_0003);
    let all = [&cells[0], &round, &cells[1]];
    let plane_total: usize =
        all.iter().map(|h| h.planes.len()).sum::<usize>() + 5 * cells[1].planes.len();
    assert_eq!(block("plane_equations").len(), plane_total);
    // Corners read back lie inside the planes read back.
    let floats = |r: &tags::defs::Record, f: &str| -> Vec<f32> {
        match r.get(f) {
            Some(Val::List(v)) => v
                .iter()
                .map(|x| match x {
                    Val::Float(f) => *f,
                    _ => panic!(),
                })
                .collect(),
            other => panic!("{f}: {other:?}"),
        }
    };
    let pe = block("plane_equations");
    let x = floats(&polys[0], "inline_x_0");
    let y = floats(&polys[0], "inline_y_0");
    let z = floats(&polys[0], "inline_z_0");
    for k in 0..4 {
        for p in &pe[..6] {
            let n = floats(p, "normal");
            let Some(Val::Float(w)) = p.get("w") else {
                panic!()
            };
            assert!(n[0] * x[k] + n[1] * y[k] + n[2] * z[k] + w <= 1e-4);
        }
    }
    let regions = block("regions");
    let Some(Val::Block(perms)) = regions[0].get("permutations") else {
        panic!()
    };
    let Some(Val::Block(members)) = perms[0].get("block_4") else {
        panic!()
    };
    assert_eq!(members.len(), 3, "every body in the one permutation");
    let ns = block("nodes");
    assert_eq!(ns.len(), 3);
    assert_eq!(
        ns[1].get("unknown_8"),
        Some(&Val::Bytes(vec![2, 0, 0xff, 0xff]))
    );
}

/// A rebuilt biped: the role's root and block words, then the object
/// header, Halo 1's camera field of view and `BIPED_WORDS` over them; it
/// reads back through the definition, nothing pinned.
#[test]
fn rebuilt_biped_is_the_role_with_halo_1s_header_and_words() {
    use crate::rebuild::{BIPED_TYPE, BIPED_WORDS, RoleWords};
    use tags::defs::{self, Defs, Target, Val};
    let obj = crate::h1::Object {
        object_type: 0,
        flags: 0,
        bounding_radius: 0.7,
        bounding_offset: [0.0, 0.0, 0.3],
        model: None,
        animation: None,
        collision: None,
        physics: None,
        modifier_shader: None,
        creation_effect: None,
        attachments: Vec::new(),
        widgets: Vec::new(),
    };
    let unit = crate::h1::Unit {
        group: h1map::Group::new(b"bipd"),
        object: obj,
        camera_field_of_view: 1.25,
        biped_words: BIPED_WORDS
            .iter()
            .map(|w| (w.1, (w.1 as f32).to_bits()))
            .collect(),
        seats: Vec::new(),
        size: None,
        references: Vec::new(),
    };
    let role = RoleWords {
        members: vec!["a".into(), "b".into()],
        root: vec![
            ("unknown_1d0".into(), 7),
            // Halo 1's own word replaces the role's.
            (BIPED_WORDS[0].0.into(), 9),
        ],
        blocks: vec![(
            "unit.block_1b8".into(),
            vec![vec![("unknown_0".into(), 3)], vec![("unknown_4".into(), 4)]],
        )],
    };
    let hlmt = map::TagId(0xe001_0002);
    let mode = map::TagId(0xe001_0003);
    let predicted = [crate::rebuild::Predicted::RenderModel(mode)];
    let root = crate::rebuild::biped(
        &unit,
        Some(hlmt),
        &predicted,
        &role,
        &crate::rebuild::BipedParts::default(),
    )
    .unwrap();
    let d = Defs::builtin().group(map::FourCC(*b"bipd")).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    let again = defs::to_built(d, &r, &mut |_| None).unwrap();
    assert_eq!(defs::from_built(d, &again).unwrap(), r);
    let word = |p: &str| match r.at(p) {
        Some(Val::Bytes(b)) => u32::from_le_bytes(b[..4].try_into().unwrap()),
        x => panic!("{p}: {x:?}"),
    };
    assert_eq!(r.at("unit.obje.object_type"), Some(&Val::Enum(BIPED_TYPE)));
    assert_eq!(r.at("unit.obje.bounding_radius"), Some(&Val::Float(0.7)));
    assert_eq!(r.at("unit.camera_field_of_view"), Some(&Val::Float(1.25)));
    assert!(matches!(
        r.at("unit.obje.model"),
        Some(Val::Ref { target: Target::Id(id), .. }) if *id == hlmt
    ));
    assert_eq!(word("unknown_1d0"), 7);
    for (path, at) in BIPED_WORDS {
        assert_eq!(word(path), (*at as f32).to_bits(), "{path}");
    }
    assert_eq!(word("unit.block_1b8[0].unknown_0"), 3);
    assert_eq!(word("unit.block_1b8[1].unknown_4"), 4);
    let Some(Val::Block(pr)) = r.at("unit.obje.predicted_resources") else {
        panic!()
    };
    assert_eq!(pr.len(), 1);
}

/// A rebuilt biped's character shapes follow Halo 1's collision size: the
/// standing and crouching pills run from one radius up to the height less
/// a radius, the three spheres rest on the ground at 1, 1.35 and 1.7
/// radii; the HUD interfaces given replace the counterpart's; and the
/// counterpart's references come with it, the model staying Halo 1's.
#[test]
fn rebuilt_biped_is_sized_to_halo_1_and_takes_its_counterparts_references() {
    use crate::h1::BipedSize;
    use crate::rebuild::{BIPD_HUD, BIPD_PILLS, BIPD_SPHERES, BipedParts, RoleWords};
    use tags::defs::{self, Defs, Target, Val};
    let d = Defs::builtin().group(map::FourCC(*b"bipd")).unwrap();
    let size = BipedSize {
        standing_height: 0.7,
        crouching_height: 0.5,
        radius: 0.2,
    };
    let unit = crate::h1::Unit {
        group: h1map::Group::new(b"bipd"),
        object: crate::h1::Object {
            object_type: 0,
            flags: 0,
            bounding_radius: 0.7,
            bounding_offset: [0.0; 3],
            model: None,
            animation: None,
            collision: None,
            physics: None,
            modifier_shader: None,
            creation_effect: None,
            attachments: Vec::new(),
            widgets: Vec::new(),
        },
        camera_field_of_view: 1.2,
        biped_words: Vec::new(),
        seats: Vec::new(),
        size: Some(size),
        references: Vec::new(),
    };
    // A counterpart with a foot reference, a camera track and a model.
    let foot = map::TagId(0xe100_0010);
    let track = map::TagId(0xe100_0011);
    let mut c = defs::default_record(d);
    c.set(
        "unit.obje.foot_ref_54",
        Val::tag(map::FourCC(*b"foot"), foot),
    )
    .unwrap();
    c.set(
        "unit.obje.model",
        Val::tag(map::FourCC(*b"hlmt"), map::TagId(0xe100_0012)),
    )
    .unwrap();
    let mut e = defs::default_record(d.child("unit.block_e8").unwrap());
    e.set("ref_0", Val::tag(map::FourCC(*b"trak"), track))
        .unwrap();
    c.set("unit.block_e8", Val::Block(vec![e])).unwrap();
    let hud = map::TagId(0xfffd_0007);
    let hlmt = map::TagId(0xfffd_0001);
    let parts = BipedParts {
        counterpart: Some(c),
        hud: Some(vec![hud, hud]),
    };
    let root =
        crate::rebuild::biped(&unit, Some(hlmt), &[], &RoleWords::default(), &parts).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    let id = |p: &str| match r.at(p) {
        Some(Val::Ref {
            target: Target::Id(id),
            ..
        }) => *id,
        x => panic!("{p}: {x:?}"),
    };
    assert_eq!(id("unit.obje.model"), hlmt, "Halo 1's model");
    assert_eq!(id("unit.obje.foot_ref_54"), foot);
    assert_eq!(id("unit.block_e8[0].ref_0"), track);
    assert_eq!(id(&format!("{BIPD_HUD}[1].ref_0")), hud);
    let f = |p: &str| match r.at(p) {
        Some(Val::Bytes(b)) => f32::from_le_bytes(b[..4].try_into().unwrap()),
        x => panic!("{p}: {x:?}"),
    };
    assert_eq!(
        [f("unknown_268"), f("unknown_26c"), f("unknown_270")],
        [0.7, 0.5, 0.2]
    );
    let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
    for (i, top) in [0.5, 0.3].into_iter().enumerate() {
        let p = format!("{BIPD_PILLS}[{i}]");
        assert!(close(f(&format!("{p}.unknown_2c")), 0.2));
        assert!(close(f(&format!("{p}.unknown_38")), top), "{p}");
        assert!(close(f(&format!("{p}.unknown_48")), 0.2));
        // The Havok header the engine writes stays zero.
        assert_eq!(f(&format!("{p}.unknown_20")), 0.0);
    }
    for (i, k) in [1.0, 1.35, 1.7].into_iter().enumerate() {
        let p = format!("{BIPD_SPHERES}[{i}]");
        assert!(close(f(&format!("{p}.unknown_2c")), 0.2 * k));
        assert!(close(f(&format!("{p}.unknown_78")), 0.2 * k));
        assert_eq!(f(&format!("{p}.unknown_3c")), 0.0, "the child pointer");
    }
}
