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
    // Nor does scenery (converter version 50).
    assert_eq!(t(b"scen", "scenery\\rocks\\boulder\\boulder"), "-");
    assert!(t(b"bipd", "characters\\anything").contains("masterchief"));
    assert_eq!(t(b"proj", "weapons\\assault rifle\\bullet"), "-");
    assert_eq!(sid_name(" Front Left-Tire "), "front_left_tire");
}

#[test]
fn weapon_zoom_rebuilds_from_the_halo1_tag() {
    use crate::{arsenal, h1};
    use h1map::testing::{self, BASE, Opts, put};

    for (levels, range) in [(0i16, [1.5f32, 3.0]), (1, [2.5, 2.5]), (2, [2.25, 7.5])] {
        let mut fixture = testing::build(&Opts::default());
        // Use the last tag slot for a synthetic weapon root.
        let slot = fixture.data_at.len() - 1;
        let index = h1map::HEADER_LEN;
        let entry = index + 0x24 + slot * h1map::TAG_ENTRY_LEN;
        let root = fixture.file.len();
        put(&mut fixture.file, entry, testing::group(b"weap"));
        put(
            &mut fixture.file,
            entry + 0x14,
            BASE + (root - index) as u32,
        );
        fixture.file.resize(root + h1::WEAP_SIZE, 0);
        put(&mut fixture.file, root + 0x48c, h1map::TagId::NONE.0);
        let length = fixture.file.len();
        put(&mut fixture.file, 8, length as u32);
        put(&mut fixture.file, 0x14, (length - index) as u32);
        fixture.file[root + 0x3da..root + 0x3dc].copy_from_slice(&levels.to_le_bytes());
        put(&mut fixture.file, root + 0x3dc, range[0].to_bits());
        put(&mut fixture.file, root + 0x3e0, range[1].to_bits());

        let map = h1map::H1Map::from_decompressed(fixture.file).unwrap();
        let tag = &map.tags()[slot];
        let weapon = h1::Weapon::read(&map, tag).unwrap();
        let built = arsenal::weapon(
            &weapon,
            &arsenal::weapon_bytes(&map, tag).unwrap(),
            &arsenal::WeaponParts::default(),
        )
        .unwrap();
        let bytes = &built.bytes;
        let word = |at: usize| u32::from_le_bytes(bytes[at..at + 4].try_into().unwrap());
        assert_eq!(
            i16::from_le_bytes(bytes[0x1fe..0x200].try_into().unwrap()),
            levels,
        );
        let expected_range = if levels == 0 { [0.0, 0.0] } else { range };
        assert_eq!(
            [f32::from_bits(word(0x200)), f32::from_bits(word(0x204))],
            expected_range,
        );
        assert_eq!(word(0x12c) & (1 << 16) != 0, levels == 0);
    }
}

#[test]
fn weapon_hud_references_distinguish_absent_stale_and_wrong_group() {
    use crate::h1::{WEAP_SIZE, Weapon};
    use h1map::testing::{SCNR_ID, Spec, build_with_extra_tags, group, put};

    let fixture = build_with_extra_tags(
        &Default::default(),
        vec![
            Spec {
                group: b"weap",
                parent: None,
                name: "fixture rifle",
                data: Some(vec![0; WEAP_SIZE]),
            },
            Spec {
                group: b"wphi",
                parent: None,
                name: "fixture hud",
                data: Some(vec![0; crate::h1hud::wphi::SIZE]),
            },
        ],
    );
    let hud_index = fixture.data_at.len() - 1;
    let root = fixture.data_at[hud_index - 1].unwrap();
    let hud = h1map::TagId(SCNR_ID + hud_index as u32);
    let read = |id: h1map::TagId| {
        let mut file = fixture.file.clone();
        put(&mut file, root + 0x480, group(b"wphi"));
        put(&mut file, root + 0x48c, id.0);
        let map = h1map::H1Map::from_decompressed(file).unwrap();
        Weapon::read(&map, &map.tags()[hud_index - 1]).map(|w| w.hud)
    };
    assert_eq!(read(h1map::TagId::NONE).unwrap(), None);
    assert_eq!(read(hud).unwrap(), Some(hud));
    for id in [h1map::TagId(hud.0 ^ (1 << 16)), h1map::TagId(SCNR_ID)] {
        let msg = read(id).unwrap_err().to_string();
        assert!(
            msg.contains("fixture rifle") && msg.to_ascii_lowercase().contains("hud"),
            "{msg}"
        );
    }
}

#[test]
fn scaled_weapon_bitmap_keeps_its_anchor_and_each_authored_axis() {
    use crate::h1hud::{self, Anchor, WeaponEntry, WeaponEntryKind, WeaponState};
    use tags::defs::{self, Defs, Val};
    use tags::effects::function::{Function, FunctionType};

    let e = WeaponEntry {
        wphi: h1map::TagId(1),
        kind: WeaponEntryKind::Static(WeaponState::TotalAmmo),
        thresholds: [0; 4],
        flash: [0; 24],
        split_flags: 0,
        frame_period: 0,
        screen: 0,
        hidden: false,
        anchor: Anchor::TopRight,
        offset: (30, 12),
        scale: [2.0, 0.25],
        bitmap: Some(h1map::TagId(9)),
        sequence: 0,
        colour: 0xffff_ffff,
    };
    let mut hud = h1hud::weapon::Interface::new();
    let widget = h1hud::weapon_widget(&e, Some(TagId(9)), TagId(10), TagId(11)).unwrap();
    hud.bitmap(&e, widget).unwrap();
    let def = Defs::builtin().group(FourCC(*b"nhdt")).unwrap();
    let r = defs::from_built(def, &hud.build().unwrap()).unwrap();
    for mode in ["full", "half", "quarter"] {
        assert_eq!(
            r.at(&format!("bitmap_widgets[0].offset_{mode}")),
            Some(&Val::List(vec![Val::Int(-30), Val::Int(12)]))
        );
    }
    let effect = "bitmap_widgets[0].effects[0]";
    assert_eq!(r.at(&format!("{effect}.scale_flags")), Some(&Val::Flags(1)));
    for (field, scale) in [("block_10", 2.0), ("block_24", 0.25)] {
        let Some(Val::Data(bytes)) = r.at(&format!("{effect}.{field}")) else {
            panic!("missing scale function {field}");
        };
        let function = Function::parse(bytes).unwrap();
        assert_eq!(function.kind, FunctionType::Constant);
        assert_eq!(function.range, [scale, scale]);
    }
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
        material_types: vec![24, 0],
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
        ..Tree::default()
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
        material_types: vec![4],
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
        texture_override: 0,
        object_type: 6,
        acceleration_scale: 1.75,
        flags: 4,
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
        change_colours: Vec::new(),
    };
    let hlmt = map::TagId(0xe001_0002);
    let (mode_id, bitm) = (map::TagId(0xe001_0003), map::TagId(0xe001_0004));
    let predicted = [
        crate::rebuild::Predicted::RenderModel(mode_id),
        crate::rebuild::Predicted::Bitmap(bitm),
    ];
    let root = crate::rebuild::scenery(&obj, Some(hlmt), &predicted).unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"scen")).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    let mut want = defs::default_record(def);
    want.set("object.object_type", Val::Enum(6)).unwrap();
    want.set("object.flags", Val::Flags(4)).unwrap();
    want.set("object.acceleration_scale", Val::Bytes(1.75f32.to_le_bytes().to_vec())).unwrap();
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
    // Everything but the predicted resources is the definition default
    // with the fields above set.
    let left: Vec<_> = defs::diff(def, &want, &got)
        .into_iter()
        .filter(|c| !c.path.starts_with("object.predicted_resources"))
        .collect();
    assert_eq!(left, Vec::new());
    // The model first (kind 2, index -1), then its bitmaps (kind 0).
    assert_eq!(
        got.bytes("object.predicted_resources[0].type", 4),
        Some(0xffff_0002u32.to_le_bytes().to_vec())
    );
    assert_eq!(
        got.at("object.predicted_resources[0].tag"),
        Some(&Val::Datum(Target::Id(mode_id)))
    );
    assert_eq!(
        got.bytes("object.predicted_resources[1].type", 4),
        Some(vec![0; 4])
    );
    assert_eq!(
        got.at("object.predicted_resources[1].tag"),
        Some(&Val::Datum(Target::Id(bitm)))
    );
    assert!(got.at("object.predicted_resources[2].tag").is_none());

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

/// Halo 1's change colours in Halo 2's slots: A and C share slot 0, the
/// one the model reads starting there (C on the cyborg, whose Halo 1
/// player colour is C; A elsewhere); B goes to 1, D to 2; always four, so
/// a player's unit takes the player's four colours.
#[test]
fn change_colours_go_to_halo2_slots_with_c_first_when_read() {
    use crate::h1::{ChangeColour, ColourPermutation};
    let one = |v: f32| ChangeColour {
        function: None,
        permutations: vec![ColourPermutation {
            weight: 1.0,
            lower: [v; 3],
            upper: [v; 3],
        }],
    };
    let h1 = [one(0.1), one(0.2), one(0.3), one(0.4)];
    let first = |c: &ChangeColour| c.permutations.first().map(|p| p.lower[0]);
    let a = ChangeColour::halo2_slots(&h1, false);
    assert_eq!(
        a.iter().map(first).collect::<Vec<_>>(),
        [Some(0.1), Some(0.2), Some(0.4), None]
    );
    let c = ChangeColour::halo2_slots(&h1, true);
    assert_eq!(
        c.iter().map(first).collect::<Vec<_>>(),
        [Some(0.3), Some(0.2), Some(0.4), None]
    );
    // The multiplayer cyborg: A and B (functions, no permutation), its
    // armour reading C: four empty slots, the player's colours go in.
    let mp = [ChangeColour::default(), ChangeColour::default()];
    assert_eq!(
        ChangeColour::halo2_slots(&mp, true),
        vec![ChangeColour::default(); 4]
    );
    assert_eq!(ChangeColour::halo2_slots(&[], false).len(), 4);
}

/// Rebuilt scenery with change colours: `obje +0xac` holds one element
/// per slot, each with Halo 1's permutations (weight, lower, upper bound)
/// as its initial permutations, any variant, and no functions.
#[test]
fn rebuilt_scenery_carries_its_change_colours() {
    use crate::h1::{ChangeColour, ColourPermutation};
    use tags::defs::{self, Defs, Val};
    let tree = ChangeColour {
        function: None,
        permutations: vec![
            ColourPermutation {
                weight: 0.333,
                lower: [1.0, 1.0, 1.0],
                upper: [1.0, 1.0, 1.0],
            },
            ColourPermutation {
                weight: 0.667,
                lower: [0.769, 0.659, 0.4],
                upper: [0.737, 0.675, 0.427],
            },
        ],
    };
    let obj = crate::h1::Object {
        texture_override: 0,
        object_type: 6,
        acceleration_scale: 0.0,
        flags: 0,
        bounding_radius: 2.5,
        bounding_offset: [0.0; 3],
        model: None,
        animation: None,
        collision: None,
        physics: None,
        modifier_shader: None,
        creation_effect: None,
        attachments: Vec::new(),
        widgets: Vec::new(),
        change_colours: ChangeColour::halo2_slots(&[tree], false),
    };
    let root = crate::rebuild::scenery(&obj, None, &[]).unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"scen")).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    let n = |p: &str| match got.at(p) {
        Some(Val::Block(b)) => b.len(),
        other => panic!("{p}: {other:?}"),
    };
    assert_eq!(n("object.change_colors"), 4);
    assert_eq!(n("object.change_colors[0].initial_permutations"), 2);
    for k in 1..4 {
        assert_eq!(n(&format!("object.change_colors[{k}].initial_permutations")), 0);
    }
    assert_eq!(n("object.change_colors[0].functions"), 0);
    let f = |p: &str| match got.bytes(p, 4) {
        Some(b) => f32::from_le_bytes(b[..4].try_into().unwrap()),
        None => panic!("{p}"),
    };
    let p = "object.change_colors[0].initial_permutations[1]";
    assert_eq!(f(&format!("{p}.weight")), 0.667);
    assert_eq!(f(&format!("{p}.color_lower_bound")), 0.769);
    assert_eq!(f(&format!("{p}.color_lower_bound+0x8")), 0.4);
    assert_eq!(f(&format!("{p}.color_upper_bound")), 0.737);
    assert_eq!(f(&format!("{p}.color_upper_bound+0x8")), 0.427);
    // Without change colours the block stays empty, as before.
    let plain = crate::h1::Object {
        change_colours: Vec::new(),
        ..obj
    };
    let root = crate::rebuild::scenery(&plain, None, &[]).unwrap();
    let got = defs::from_built(def, &root).unwrap();
    assert!(matches!(got.at("object.change_colors"), Some(Val::Block(b)) if b.is_empty()));
}

/// A rebuilt vehicle (a10's cryotube and the other Halo 1 vehicles with no
/// template): the `vehi` definition's defaults, the turret role, the
/// header and one seat per Halo 1 seat; it reads back through the
/// definition and writes the same builder tree again.
#[test]
fn rebuilt_vehicle_is_the_definition_with_seats_and_the_turret_role() {
    use crate::rebuild::{
        ROLE_VEHICLE_TYPE, TURRET_PHYSICS_ROLE, TURRET_SEAT_ROLE, VEHI_PHYSICS,
        VEHI_SEATS,
    };
    use tags::defs::{self, Defs, Str, Target, Val};
    let obj = crate::h1::Object {
        texture_override: 0,
        object_type: 1,
        acceleration_scale: 0.0,
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
        change_colours: Vec::new(),
    };
    let seat = crate::h1::Seat {
        camera_angles: [0; 5],
        rates: [0; 2],
        flags: 0x1d,
        label: "CT-driver".into(),
        marker: "driver".into(),
        camera_marker: "camera".into(),
        camera_tracks: Vec::new(),
    };
    let hlmt = map::TagId(0xe001_0002);
    let mode = map::TagId(0xe001_0003);
    let bitmap = map::TagId(0xe001_0004);
    let predicted = [
        crate::rebuild::Predicted::RenderModel(mode),
        crate::rebuild::Predicted::Bitmap(bitmap),
    ];
    let weapon = map::TagId(0xe001_0005);
    let markers = vec!["driver".to_string(), "driver enter".to_string(), "camera".to_string()];
    let seats = [crate::rebuild::seat_names(&seat, &markers)];
    let root = crate::rebuild::vehicle(&crate::rebuild::VehicleSpec {
        source: &[],
        references: &[],
        object: &obj,
        bounds: None,
        hlmt: Some(hlmt),
        seats: &seats,
        predicted: &predicted,
        camera_field_of_view: 1.2,
        vehicle_type: ROLE_VEHICLE_TYPE as u16,
        flags: 0xdead_00c0,
        weapons: &[weapon],
        points: &[],
        attachments: &[],
        functions: &[],
        creation_effect: None,
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
    s.set("seat_marker_name", Val::text("driver")).unwrap();
    // Marker names as the converted model keeps them (Halo 1's).
    s.set("entry_marker_name", Val::text("driver enter")).unwrap();
    s.set("camera_marker_name", Val::text("camera")).unwrap();
    // Halo 1's seat flags: the driver's, so the rider drives.
    s.set("flags", word(0x1d)).unwrap();
    want.set(VEHI_SEATS, Val::Block(vec![s])).unwrap();
    let pr = def.child("unit.obje.predicted_resources").unwrap();
    let res = [(0xffff_0002u32, mode), (0, bitmap)]
        .iter()
        .map(|(w, id)| {
            let mut x = defs::default_record(pr);
            x.set("type", word(*w)).unwrap();
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
        got.at("seats[0].label"),
        Some(&Val::String(Str::Text("c_turret_ap_d".into())))
    );
    // References a fresh vehicle must not point at tag 0: the two the
    // schema could not see are null references, not zero words.
    for r in ["suspension_sound", "foot_ref_294"] {
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

/// A Halo 1 camera track becomes a Halo 2 `trak` with the same points, in
/// order, each orientation the conjugate, and reads back through the
/// definition. Halo 1's orientations (here its warthog's top, middle and
/// bottom points) look up at the top of the track; converted, the
/// elevation the engine searches by rises along it, as on retail tracks.
#[test]
fn camera_track_keeps_halo_1s_control_points() {
    use std::f32::consts::FRAC_1_SQRT_2;
    use tags::defs::{self, Defs, Val};
    let h1 = crate::h1::CameraTrack {
        flags: 0,
        points: vec![
            ([0.5, 0.0, 3.5], [0.0, -FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2]),
            ([-2.75, 0.0, 0.68], [0.0, 0.0, 0.0, 1.0]),
            ([-1.0, 0.0, -0.18], [0.0, FRAC_1_SQRT_2, 0.0, FRAC_1_SQRT_2]),
        ],
    };
    let root = crate::rebuild::camera_track(&h1).unwrap();
    let def = Defs::builtin().group(map::FourCC(*b"trak")).unwrap();
    // Root 0xc, then 0x1c per point (V: every retail track's size).
    assert_eq!(def.size, 0xc);
    assert_eq!(def.child("control_points").unwrap().size, 0x1c);
    let got = defs::from_built(def, &root).unwrap();
    assert_eq!(got.at("flags"), Some(&Val::Int(0)));
    for (i, (p, q)) in h1.points.iter().enumerate() {
        assert_eq!(
            got.at(&format!("control_points[{i}].position")),
            Some(&Val::floats(p))
        );
        assert_eq!(
            got.at(&format!("control_points[{i}].orientation")),
            Some(&Val::floats(&[-q[0], -q[1], -q[2], q[3]]))
        );
    }
    assert!(got.at("control_points[3].position").is_none());
    let elevation = |q: [f32; 4]| crate::rebuild::camera_elevation(q).to_degrees();
    // As stored, Halo 1's first point looks straight up.
    assert!((elevation(h1.points[0].1) - 90.0).abs() < 0.1);
    let conv: Vec<f32> = (0..3)
        .map(|i| match got.at(&format!("control_points[{i}].orientation")) {
            Some(Val::List(q)) => {
                let q: Vec<f32> = q
                    .iter()
                    .map(|v| match v {
                        Val::Float(f) => *f,
                        other => panic!("component {other:?}"),
                    })
                    .collect();
                elevation([q[0], q[1], q[2], q[3]])
            }
            other => panic!("orientation {other:?}"),
        })
        .collect();
    assert!((conv[0] + 90.0).abs() < 0.1, "{conv:?}");
    assert!(conv[1].abs() < 0.1, "{conv:?}");
    assert!((conv[2] - 90.0).abs() < 0.1, "{conv:?}");
    let again = defs::to_built(def, &got, &mut |_| None).unwrap();
    assert_eq!(defs::from_built(def, &again).unwrap(), got);
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
            texture_override: 0,
            object_type: 7,
            acceleration_scale: 0.0,
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
            change_colours: Vec::new(),
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
    let root = physics_model(&bodies, &nodes, crate::rebuild::DEVICE_BODY, &[]).unwrap();
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
    // The body words: flags in `+0x18`'s low half, the motion in its high
    // half, `+0x1e` in `+0x1c`'s high half.
    let words = |r: &tags::defs::Record| match (r.get("unknown_18"), r.get("unknown_1c")) {
        (Some(Val::Bytes(a)), Some(Val::Bytes(b))) => (a.clone(), b.clone()),
        other => panic!("{other:?}"),
    };
    assert!(rbs.iter().all(|b| words(b) == (vec![0, 0, 2, 0], vec![0, 0, 5, 0])));
    let biped = physics_model(&bodies, &nodes, crate::rebuild::BIPED_BODY, &[]).unwrap();
    let biped = defs::from_built(def, &biped).unwrap();
    let Some(Val::Block(brbs)) = biped.at("rigid_bodies") else {
        panic!()
    };
    assert!(brbs.iter().all(|b| words(b) == (vec![3, 0, 2, 0], vec![0, 0, 1, 0])));
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
        ["flags", "parent", "next_sibling", "first_child"].map(|f| ns[1].get(f).cloned()),
        [0, 0, 2, -1].map(|v| Some(Val::Int(v)))
    );
    // No joints asked for: no edges or constraints.
    assert!(block("node_edges").is_empty() && block("ragdoll_constraints").is_empty());
}

#[test]
fn biped_carries_each_source_word_and_leaves_unmapped_fields_at_default() {
    use crate::rebuild::{BIPED_TYPE, BIPED_WORDS};
    use tags::defs::{self, Defs, Target, Val};
    let obj = crate::h1::Object {
        texture_override: 0,
        object_type: 0,
        acceleration_scale: 0.0,
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
        change_colours: Vec::new(),
    };
    let unit = crate::h1::Unit {
        hud_interfaces: Vec::new(),
        group: h1map::Group::new(b"bipd"),
        object: obj,
        camera_field_of_view: 1.25,
        biped_words: BIPED_WORDS
            .iter()
            .map(|w| (w.1, (w.1 as f32).to_bits()))
            .collect(),
        seats: Vec::new(),
        camera_tracks: Vec::new(),
        size: None,
        references: Vec::new(),
    };
    let hlmt = map::TagId(0xe001_0002);
    let mode = map::TagId(0xe001_0003);
    let predicted = [crate::rebuild::Predicted::RenderModel(mode)];
    let root = crate::rebuild::biped(
        &unit,
        Some(hlmt),
        &predicted,
        &crate::rebuild::BipedParts::default(),
    )
    .unwrap();
    let d = Defs::builtin().group(map::FourCC(*b"bipd")).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    let again = defs::to_built(d, &r, &mut |_| None).unwrap();
    assert_eq!(defs::from_built(d, &again).unwrap(), r);
    let word = |p: &str| match r.bytes(p, 4) {
        Some(b) => u32::from_le_bytes(b[..4].try_into().unwrap()),
        None => panic!("{p}"),
    };
    assert_eq!(r.at("unit.obje.object_type"), Some(&Val::Enum(BIPED_TYPE)));
    assert_eq!(r.at("unit.obje.bounding_radius"), Some(&Val::Float(0.7)));
    assert_eq!(r.at("unit.camera_field_of_view"), Some(&Val::Float(1.25)));
    assert!(matches!(
        r.at("unit.obje.model"),
        Some(Val::Ref { target: Target::Id(id), .. }) if *id == hlmt
    ));
    assert_eq!(word("boost_peak_power"), 0);
    for (path, at) in BIPED_WORDS {
        assert_eq!(word(path), (*at as f32).to_bits(), "{path}");
    }
    assert!(
        BIPED_WORDS
            .iter()
            .all(|w| !crate::rebuild::BIPED_LANDING.iter().any(|l| l.0 == w.0)),
        "landing velocities take the gravity factor, not the plain copy"
    );
    assert!(matches!(r.at("unit.powered_seats"), Some(Val::Block(entries)) if entries.is_empty()));
    let Some(Val::Block(pr)) = r.at("unit.obje.predicted_resources") else {
        panic!()
    };
    assert_eq!(pr.len(), 1);
}

/// A Halo 1 biped unit holding `words` (Halo 1 offset, word) and no
/// collision size.
fn biped_unit(words: Vec<(usize, u32)>) -> crate::h1::Unit {
    crate::h1::Unit {
        hud_interfaces: Vec::new(),
        group: h1map::Group::new(b"bipd"),
        object: crate::h1::Object {
            texture_override: 0,
            object_type: 0,
            acceleration_scale: 0.0,
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
            change_colours: Vec::new(),
        },
        camera_field_of_view: 1.2,
        biped_words: words,
        seats: Vec::new(),
        camera_tracks: Vec::new(),
        size: None,
        references: Vec::new(),
    }
}

/// A biped with `jump_velocity` and the landing thresholds set from the
/// given Halo 1 values (per tick for the jump).
fn biped_with_fall_words(jump_per_tick: f32, landing: [f32; 3]) -> tags::defs::Record {
    use crate::rebuild::{BIPED_LANDING, BIPED_RATES};
    use tags::defs::{self, Defs};
    let unit = crate::h1::Unit {
        hud_interfaces: Vec::new(),
        group: h1map::Group::new(b"bipd"),
        object: crate::h1::Object {
            texture_override: 0,
            object_type: 0,
            acceleration_scale: 0.0,
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
            change_colours: Vec::new(),
        },
        camera_field_of_view: 1.25,
        biped_words: BIPED_RATES
            .iter()
            .filter(|w| w.0 == "jump_velocity")
            .map(|w| (w.1, jump_per_tick.to_bits()))
            .chain(BIPED_LANDING.iter().zip(landing).map(|(w, v)| (w.1, v.to_bits())))
            .collect(),
        seats: Vec::new(),
        camera_tracks: Vec::new(),
        size: None,
        references: Vec::new(),
    };
    let root = crate::rebuild::biped(
        &unit,
        None,
        &[],
        &crate::rebuild::BipedParts::default(),
    )
    .unwrap();
    defs::from_built(Defs::builtin().group(map::FourCC(*b"bipd")).unwrap(), &root).unwrap()
}

fn float_word(r: &tags::defs::Record, path: &str) -> f32 {
    f32::from_le_bytes(r.bytes(path, 4).unwrap()[..4].try_into().unwrap())
}

/// Halo 2's gravity is 1 / `GRAVITY_RATIO` times Halo 1's. The cyborg's
/// 0.07 per tick is 2.1 world units per second under Halo 1's gravity;
/// the rebuilt jump is 2.394 and rises to the same 0.687 under Halo 2's.
#[test]
fn jump_reaches_halo1s_apex_under_halo2_gravity() {
    use crate::rebuild::{GRAVITY_RATIO, SOURCE_GRAVITY};
    let r = biped_with_fall_words(0.07, [0.0; 3]);
    let jump = float_word(&r, "jump_velocity");
    assert!((jump - 2.394).abs() < 0.002, "{jump}");
    let h2_gravity = SOURCE_GRAVITY / GRAVITY_RATIO;
    let (h1_apex, h2_apex) = (
        2.1f32 * 2.1 / (2.0 * SOURCE_GRAVITY),
        jump * jump / (2.0 * h2_gravity),
    );
    assert!((h1_apex - h2_apex).abs() < 1e-4, "{h1_apex} against {h2_apex}");
    assert!((h2_apex - 0.687).abs() < 0.003, "{h2_apex}");
}

/// A landing speed is sqrt(2 g h): the same fall height lands at Halo 1's
/// speed times sqrt(1 / `GRAVITY_RATIO`) under Halo 2's gravity, and
/// the words are not scaled by the tick rate.
#[test]
fn landing_thresholds_keep_halo1s_fall_heights() {
    use crate::rebuild::{GRAVITY_RATIO, SOURCE_GRAVITY};
    let from = [0.1f32, 0.18, 0.4];
    let r = biped_with_fall_words(0.07, from);
    let h2_gravity = SOURCE_GRAVITY / GRAVITY_RATIO;
    for ((path, _), v) in crate::rebuild::BIPED_LANDING.iter().zip(from) {
        let got = float_word(&r, path);
        let (h1_height, h2_height) = (
            v * v / (2.0 * SOURCE_GRAVITY),
            got * got / (2.0 * h2_gravity),
        );
        assert!((h1_height - h2_height).abs() < 1e-6, "{path}: {h1_height} against {h2_height}");
        assert!(got > v, "{path}");
    }
}

/// A rebuilt biped's character shapes follow Halo 1's collision size: the
/// standing and crouching pills run from one radius up to the height less
/// a radius, the three spheres rest on the ground at 1, 1.35 and 1.7
/// radii; the HUD interfaces given replace the counterpart's; and the
/// counterpart's references come with it, the model staying Halo 1's.
#[test]
fn rebuilt_biped_uses_source_shapes_tracks_weapons_and_hud() {
    use crate::h1::BipedSize;
    use crate::rebuild::{BIPD_HUD, BIPD_PILLS, BIPD_SPHERES, BipedParts};
    use tags::defs::{self, Defs, Target, Val};
    let d = Defs::builtin().group(map::FourCC(*b"bipd")).unwrap();
    let size = BipedSize {
        standing_height: 0.7,
        crouching_height: 0.5,
        radius: 0.2,
    };
    let unit = crate::h1::Unit {
        hud_interfaces: Vec::new(),
        group: h1map::Group::new(b"bipd"),
        object: crate::h1::Object {
            texture_override: 0,
            object_type: 0,
            acceleration_scale: 0.0,
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
            change_colours: Vec::new(),
        },
        camera_field_of_view: 1.2,
        biped_words: Vec::new(),
        seats: Vec::new(),
        camera_tracks: Vec::new(),
        size: Some(size),
        references: Vec::new(),
    };
    let weapon = map::TagId(0xfffd_0010);
    let track = map::TagId(0xfffd_0011);
    let hud = map::TagId(0xfffd_0007);
    let hlmt = map::TagId(0xfffd_0001);
    // The model has Halo 1's head and right hand markers, but no left.
    let parts = BipedParts {
        tracks: vec![track],
        weapons: vec![weapon],
        hud: Some(vec![hud, hud]),
        markers: vec!["head".into(), "right hand".into(), "body".into()],
        ..Default::default()
    };
    let root =
        crate::rebuild::biped(&unit, Some(hlmt), &[], &parts).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    let id = |p: &str| match r.at(p) {
        Some(Val::Ref {
            target: Target::Id(id),
            ..
        }) => *id,
        x => panic!("{p}: {x:?}"),
    };
    assert_eq!(id("unit.obje.model"), hlmt, "Halo 1's model");
    assert_eq!(id("unit.weapons[0].weapon"), weapon);
    assert_eq!(id("unit.camera_tracks[0].track"), track);
    assert_eq!(id(&format!("{BIPD_HUD}[1].hud_interface")), hud);
    // The camera at the head, a weapon in the right hand; no left hand
    // marker, so none named.
    let text = |p: &str| match r.at(p) {
        Some(Val::String(defs::Str::Text(t))) => Some(t.clone()),
        Some(Val::String(defs::Str::Null)) | None => None,
        x => panic!("{p}: {x:?}"),
    };
    assert_eq!(text("unit.camera_marker_name").as_deref(), Some("head"));
    assert_eq!(text("unit.right_hand_node").as_deref(), Some("right hand"));
    assert_eq!(text("unit.left_hand_node"), None);
    let f = |p: &str| match r.bytes(p, 4) {
        Some(b) => f32::from_le_bytes(b[..4].try_into().unwrap()),
        None => panic!("{p}"),
    };
    assert_eq!(
        [f("height_standing"), f("height_crouching"), f("radius")],
        [0.7, 0.5, 0.2]
    );
    let close = |a: f32, b: f32| (a - b).abs() < 1e-6;
    for (i, top) in [0.5, 0.3].into_iter().enumerate() {
        let p = format!("{BIPD_PILLS}[{i}]");
        assert!(close(f(&format!("{p}.radius")), 0.2));
        assert!(close(f(&format!("{p}.bottom+0x8")), top), "{p}");
        assert!(close(f(&format!("{p}.top+0x8")), 0.2));
        // The Havok header the engine writes stays zero.
        assert_eq!(f(&format!("{p}.unknown_20")), 0.0);
    }
    for (i, k) in [1.0, 1.35, 1.7].into_iter().enumerate() {
        let p = format!("{BIPD_SPHERES}[{i}]");
        assert!(close(f(&format!("{p}.radius")), 0.2 * k));
        assert!(close(f(&format!("{p}.translation+0x8")), 0.2 * k));
        assert_eq!(f(&format!("{p}.unknown_3c")), 0.0, "the child pointer");
    }
}

#[test]
fn rebuilt_effect_keeps_halo_1s_events_and_the_parts_that_convert() {
    use crate::h1::{Effect, EffectEvent, EffectPart};
    use crate::rebuild::{PartTarget, effect, part_type_group};
    use tags::defs::{self, Defs, Str, Target, Val};
    let part = |group: &[u8; 4], location| EffectPart {
        create_in: 1,
        violence_mode: 0,
        location,
        flags: 1,
        group: h1map::Group::new(group),
        reference: Some(h1map::TagId(0xe000_0001)),
        velocity_bounds: [1.0, 2.0],
        velocity_cone_angle: 0.5,
        angular_velocity_bounds: [-1.0, 1.0],
        radius_modifier_bounds: [0.5, 1.5],
        a_scales: 0x20,
        b_scales: 0,
    };
    let e = Effect {
        flags: 0,
        loop_start_event: -1,
        loop_stop_event: -1,
        locations: vec!["invented marker".into(), String::new()],
        events: vec![EffectEvent {
            skip_fraction: 0.25,
            delay_bounds: [0.0, 0.5],
            duration_bounds: [1.0 / 30.0, 1.0 / 30.0],
            parts: vec![part(b"snd!", 0), part(b"deca", 1), part(b"proj", 1)],
            particles: 3,
        }],
    };
    let root = effect(&e, &mut |p| {
        let g = p.group.0.to_be_bytes();
        (&g != b"deca").then(|| PartTarget {
            type_group: part_type_group(g),
            group: g,
            tag: crate::convert::local(7),
        })
    })
    .unwrap();
    let d = Defs::builtin().group(FourCC(*b"effe")).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    let Some(Val::Block(locs)) = r.at("locations") else { panic!() };
    assert_eq!(locs.len(), 2);
    assert_eq!(
        locs[0].at("marker"),
        Some(&Val::String(Str::Text("invented marker".into())))
    );
    assert_eq!(locs[1].at("marker"), Some(&Val::String(Str::Null)));
    let Some(Val::Block(events)) = r.at("events") else { panic!() };
    assert_eq!(events[0].at("skip_fraction"), Some(&Val::Float(0.25)));
    let Some(Val::Block(parts)) = events[0].at("parts") else { panic!() };
    // The decal has no converter: two parts, the sound and the projectile.
    assert_eq!(parts.len(), 2);
    let word = |g: &[u8; 4]| Val::Int(u32::from_be_bytes(*g).into());
    assert_eq!(parts[0].at("type_group"), Some(&word(b"snd!")));
    assert_eq!(parts[1].at("type_group"), Some(&word(b"obje")));
    assert_eq!(parts[1].at("location"), Some(&Val::Int(1)));
    assert_eq!(parts[0].at("create_in"), Some(&Val::Int(1)));
    assert_eq!(parts[0].at("a_scales"), Some(&Val::Int(0x20)));
    assert_eq!(parts[0].at("velocity_bounds"), Some(&Val::floats(&[1.0, 2.0])));
    assert!(matches!(
        parts[0].at("reference"),
        Some(Val::Ref { target: Target::Id(id), .. }) if *id == crate::convert::local(7)
    ));
    let Some(Val::Block(systems)) = events[0].at("particle_systems") else { panic!() };
    assert!(systems.is_empty());
}

#[test]
fn audio_placeholders_without_their_sounds_are_a_named_error_over_donor_slots() {
    // A tree whose tag names a sound and a looping sound it holds none of:
    // installing over donor slots adds no sound, so neither placeholder has
    // a datum. The install refuses, naming the map and the tag, and leaves
    // the donor as it was (no null reference, no stray placeholder datum).
    let t = tag(b"effe", &[(0, crate::convert::sound(0)), (8, crate::convert::looping(0))]);
    let mut set = TagSet::new(CacheType::Multiplayer, "h1test", "invented\\h1test");
    set.add_group(FourCC(*b"effe"), None, None);
    let donor = set
        .add_tag(FourCC(*b"effe"), "invented\\donor", TagPayload::Built(Block::zeroed(4)))
        .unwrap();
    let tree = Tree {
        tags: vec![("invented\\effect".into(), t)],
        ..Tree::default()
    };
    let Err(crate::Error::Bsp(h1convert::Error::NoRebuild { map, tag, reason })) =
        tree.install_over(&mut set, &[donor])
    else {
        panic!("a tag naming sounds the tree lacks installed over a donor");
    };
    assert_eq!((map.as_str(), tag.as_str()), ("h1test", "invented\\effect"));
    assert!(reason.contains("unresolved tree datum"), "{reason}");
    let Some(entry) = set.slots[donor.index()].tag.as_ref() else { panic!() };
    assert_eq!(entry.group, FourCC(*b"effe"));
    let TagPayload::Built(root) = &entry.payload else { panic!() };
    assert_eq!(root.bytes, vec![0; 4]);
    assert!(root.fixups.is_empty());
}

#[test]
fn rebuilt_light_is_halo_1s_sphere_or_cone_with_its_colours() {
    use crate::h1::Light;
    use tags::defs::{self, Defs, Val};
    let mut l = Light {
        flags: 0,
        radius: 2.5,
        falloff_angle: std::f32::consts::PI,
        cutoff_angle: std::f32::consts::PI,
        colour_lower: [1.0, 0.1, 0.2, 0.3],
        colour_upper: [1.0, 0.4, 0.5, 0.6],
        duration_ticks: 15.0,
        lens_flare: None,
    };
    let d = Defs::builtin().group(FourCC(*b"ligh")).unwrap();
    let r = defs::from_built(d, &crate::rebuild::light(&l).unwrap()).unwrap();
    assert_eq!(r.at("kind"), Some(&Val::Enum(0)));
    assert_eq!(r.at("radius"), Some(&Val::Float(2.5)));
    assert_eq!(r.at("flags"), Some(&Val::Int(8)));
    assert_eq!(r.at("duration"), Some(&Val::Float(0.5)));
    assert_eq!(r.at("diffuse_colour_lower"), Some(&Val::floats(&[0.1, 0.2, 0.3])));
    assert_eq!(r.at("specular_colour_upper"), Some(&Val::floats(&[0.4, 0.5, 0.6])));
    l.cutoff_angle = 1.0;
    let r = defs::from_built(d, &crate::rebuild::light(&l).unwrap()).unwrap();
    assert_eq!(r.at("kind"), Some(&Val::Enum(2)));
    assert_eq!(r.at("field_of_view"), Some(&Val::Float(1.0)));
    assert_eq!(r.at("cutoff_distance"), Some(&Val::Float(2.5)));
    assert_eq!(r.at("radius"), Some(&Val::Float(0.0)));
}

#[test]
fn rebuilt_decal_keeps_halo_1s_fields_and_maps_its_layer() {
    use crate::h1::Decal;
    use crate::rebuild::decal;
    use tags::defs::{self, Defs, Target, Val};
    let x = Decal {
        flags: 8,
        kind: 2,
        layer: 0,
        next: None,
        radius: [0.1, 0.2],
        colour_lower: [0.1, 0.2, 0.3],
        colour_upper: [0.4, 0.5, 0.6],
        lifetime: [10.0, 12.0],
        decay_time: [1.0, 2.0],
        bitmap: None,
        maximum_sprite_extent: 16.0,
    };
    let d = Defs::builtin().group(FourCC(*b"deca")).unwrap();
    let root = decal(&x, Some(local(1)), Some(local(2))).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    assert_eq!(r.at("flags"), Some(&Val::Int(8)));
    assert_eq!(r.at("kind"), Some(&Val::Int(2)));
    assert_eq!(r.at("layer"), Some(&Val::Int(2)));
    assert_eq!(r.at("max_overlapping"), Some(&Val::Int(0)));
    assert_eq!(r.at("radius"), Some(&Val::floats(&[0.1, 0.2])));
    assert_eq!(r.at("decay_time_bounds"), Some(&Val::floats(&[1.0, 2.0])));
    assert_eq!(r.at("maximum_sprite_extent"), Some(&Val::Float(16.0)));
    assert!(matches!(r.at("bitmap"), Some(Val::Ref { target: Target::Id(id), .. }) if *id == local(2)));
    assert!(matches!(r.at("next_decal"), Some(Val::Ref { target: Target::Id(id), .. }) if *id == local(1)));
}

#[test]
fn dialogue_takes_the_first_slot_holding_a_sound_per_vocalization() {
    use crate::convert::sound;
    use crate::h1::Dialogue;
    use crate::rebuild::{dialogue, dialogue_lines};
    use tags::defs::{self, Defs, Str, Target, Val};
    let s = |k: u32| h1map::TagId(0xe000_0000 | k);
    // deathviolent (0x100) and deathinstant (0x130), no deathquiet.
    let d = Dialogue {
        lines: vec![(0x100, s(1)), (0x130, s(2)), (0x020, s(3)), (0x0b0, s(4))],
    };
    let got: Vec<_> = dialogue_lines(&d).iter().map(|l| (l.0, l.1)).collect();
    // dth_hdsht from deathinstant before deathviolent; 0x0b0 has no line.
    assert_eq!(
        got,
        vec![
            ("look_cmbt", 0x020),
            ("dth_mjr", 0x100),
            ("dth", 0x100),
            ("dth_hdsht", 0x130),
        ]
    );
    let vocabulary = ["dth".into(), "pain".into(), "look".into()];
    let root = dialogue(
        &vocabulary,
        &[("pain", 0x70, sound(1)), ("dth", 0x100, sound(0))],
        &[(0x70, sound(1)), (0xb0, sound(2))],
    )
    .unwrap();
    let def = Defs::builtin().group(FourCC(*b"udlg")).unwrap();
    let r = defs::from_built(def, &root).unwrap();
    assert_eq!(r.at("unknown_0"), Some(&Val::Bytes(b"glda".to_vec())));
    assert_eq!(r.at("unknown_4"), Some(&Val::Bytes(vec![0xff; 4])));
    let Some(Val::Block(els)) = r.at("vocalizations") else { panic!() };
    assert_eq!(els.len(), 5);
    assert_eq!(els[1].at("vocalization"), Some(&Val::String(Str::Text("pain".into()))));
    assert_eq!(
        els[1].at("unknown_0"),
        Some(&Val::Bytes((map::H1_NATIVE_DIALOGUE_SLOT | 6).to_le_bytes().to_vec())),
    );
    assert_eq!(els[1].at("unknown_8"), Some(&Val::Bytes(b"!dns".to_vec())));
    assert_eq!(els[1].at("sound"), Some(&Val::Datum(Target::Id(sound(1)))));
    assert_eq!(els[2].at("sound"), Some(&Val::Datum(Target::Null)));
    assert_eq!(els[4].at("sound"), Some(&Val::Datum(Target::Id(sound(2)))));
    assert_eq!(
        els[4].at("unknown_0"),
        Some(&Val::Bytes((map::H1_DIALOGUE_SLOT | 0x0a).to_le_bytes().to_vec())),
    );
}

#[test]
fn a_multi_voice_model_keeps_selectors_and_duplicate_generic_alternatives() {
    use crate::rebuild::model_dialogue;
    use tags::defs::{self, Defs, Val};

    let hlmt = Defs::builtin().group(FourCC(*b"hlmt")).unwrap();
    let mut model = defs::default_record(hlmt);
    let voices = [(0, local(1)), (100, local(2)), (0, local(3))];
    model_dialogue(&mut model, &voices).unwrap();
    let block = defs::to_built(hlmt, &model, &mut |_| None).unwrap();
    let model = defs::from_built(hlmt, &block).unwrap();
    let Some(Val::Block(rows)) = model.at("variants") else { panic!("missing variants") };
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0].at("h1_voice_selector"), Some(&Val::Int(0)));
    for (row, (selector, voice)) in rows[1..].iter().zip(voices) {
        let marker = i64::from(map::H1_VOICE_VARIANT | u32::from(selector as u16));
        assert_eq!(row.at("h1_voice_selector"), Some(&Val::Int(marker)));
        assert!(matches!(
            row.at("udlg_ref_30"),
            Some(Val::Ref { target: defs::Target::Id(id), .. }) if *id == voice
        ));
    }
}

#[test]
fn foot_kinds_keep_their_materials() {
    use crate::rebuild::{FootKind, FootLine, foot};
    use tags::defs::{self, Defs, Str, Val};
    let k = FootKind {
        sounds: vec![FootLine { material: "tough_terrain_dirt", index: 7, tag: local(1) }],
        effects: vec![FootLine { material: "liquid_thin_water", index: 9, tag: local(2) }],
    };
    let root = foot(&[FootKind::default(), k]).unwrap();
    let d = Defs::builtin().group(FourCC(*b"foot")).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    let Some(Val::Block(kinds)) = r.at("effects") else { panic!() };
    assert_eq!(kinds.len(), 2);
    let Some(Val::Block(s)) = kinds[1].at("sounds") else { panic!() };
    assert_eq!(s[0].at("material"), Some(&Val::String(Str::Text("tough_terrain_dirt".into()))));
    assert_eq!(s[0].at("material_index"), Some(&Val::Int(7)));
    assert_eq!(s[0].at("unknown_16"), Some(&Val::Bytes(vec![2, 0])));
    let Some(Val::Block(e)) = kinds[1].at("effects") else { panic!() };
    assert_eq!(e[0].at("material_index"), Some(&Val::Int(9)));
    assert_eq!(e[0].at("unknown_16"), Some(&Val::Bytes(vec![1, 0])));
}

/// An `hlmt`'s materials follow its `coll`'s: one per collision material
/// in order, named as it, mapped through its Halo 1 type to the global
/// material and that material's index in the globals; a type with no
/// global material is an error, not a guess.
#[test]
fn hlmt_materials_follow_the_collision_materials() {
    use crate::rebuild::{
        HLMT_MATERIALS, HLMT_MATERIAL_SIZE, model_material_count, model_materials,
        model_materials_of, set_model_materials,
    };
    let c = Collision {
        body_vitality: 0.0,
        shield_vitality: 0.0,
        references: Vec::new(),
        materials: vec!["Rubber".into(), "metal".into(), "glass".into()],
        material_types: vec![8, 7, 9],
        regions: Vec::new(),
        modifiers: 0,
        spheres: Vec::new(),
        nodes: Vec::new(),
    };
    let globals: Vec<String> = [
        "default_material",
        "brittle_glass",
        "hard_metal_thick",
        "tough_inorganic_rubber",
    ]
    .map(String::from)
    .to_vec();
    let m = model_materials(&c, &globals).unwrap();
    let got: Vec<_> = m
        .iter()
        .map(|m| (m.name.as_str(), m.global.as_str(), m.global_index))
        .collect();
    assert_eq!(
        got,
        [
            ("rubber", "tough_inorganic_rubber", 3),
            ("metal", "hard_metal_thick", 2),
            ("glass", "brittle_glass", 1),
        ]
    );
    let mut hlmt = Block::zeroed(0x100);
    set_model_materials(&mut hlmt, &m);
    assert_eq!(model_material_count(&hlmt), 3);
    assert_eq!(model_materials_of(&hlmt), m);
    let child = hlmt
        .fixups
        .iter()
        .find_map(|f| match &f.kind {
            map::build::FixupKind::Block { child, .. } if f.offset == HLMT_MATERIALS => Some(child),
            _ => None,
        })
        .unwrap();
    let e = &child.bytes[HLMT_MATERIAL_SIZE..2 * HLMT_MATERIAL_SIZE];
    assert_eq!(&e[4..8], &0xffff_0000u32.to_le_bytes());
    assert_eq!(&e[8..12], &0xffff_0001u32.to_le_bytes(), "+8: k, high -1");
    assert_eq!(&e[0x10..0x14], &2u32.to_le_bytes());

    // Writing again replaces, never appends.
    set_model_materials(&mut hlmt, &m[..1]);
    assert_eq!(model_material_count(&hlmt), 1);
    assert_eq!(
        hlmt.fixups.iter().filter(|f| f.offset == HLMT_MATERIALS).count(),
        1
    );

    let no_glass = &globals[..1];
    assert!(model_materials(&c, no_glass).is_err());
}

/// A weapon type's class is the one most of the map's animations name it
/// under; a tie goes to the first class by name.
#[test]
fn weapon_class_is_the_one_most_animations_name() {
    use std::collections::HashMap;
    let votes: HashMap<(String, String), usize> = [
        (("ar", "rifle"), 40),
        (("ar", "pistol"), 2),
        (("hp", "pistol"), 30),
        (("x", "rifle"), 3),
        (("x", "missile"), 3),
    ]
    .into_iter()
    .map(|((k, c), n)| ((k.to_owned(), c.to_owned()), n))
    .collect();
    let m = crate::convert::weapon_classes(votes);
    assert_eq!(m["ar"], "rifle");
    assert_eq!(m["hp"], "pistol");
    assert_eq!(m["x"], "missile");
    assert_eq!(m.len(), 3);
}

/// The rebuilt vehicle's sphere is the middle of its model's box and
/// reaches its farthest vertex, wherever Halo 1's origin is.
#[test]
fn model_sphere_centres_the_model_box() {
    use h1_assets::models::h1::{Geometry, Model, Part, Vertex};
    let vertex = |position: [f32; 3]| Vertex {
        position,
        normal: [0.0, 0.0, 1.0],
        binormal: [0.0, 1.0, 0.0],
        tangent: [1.0, 0.0, 0.0],
        texcoord: [0.0; 2],
        nodes: [0, -1],
        weights: [1.0, 0.0],
    };
    let part = |vs: Vec<Vertex>| Part {
        flags: 0,
        shader: 0,
        centroid_nodes: [0, -1],
        centroid_weights: [1.0, 0.0],
        centroid: [0.0; 3],
        vertices: vs,
        strip: Vec::new(),
    };
    // A warthog-like body on wheels its origin touches: z 0 to 1.
    let model = Model {
        flags: 0,
        node_list_checksum: 0,
        lod_cutoffs: [0.0; 5],
        base_map_scale: [1.0; 2],
        markers: Vec::new(),
        nodes: Vec::new(),
        regions: Vec::new(),
        geometries: vec![
            Geometry {
                flags: 0,
                parts: vec![part(vec![vertex([-1.0, -0.5, 0.0]), vertex([1.0, 0.5, 0.6])])],
            },
            Geometry {
                flags: 0,
                parts: vec![part(vec![vertex([-0.6, 0.0, 1.0])])],
            },
        ],
        shaders: Vec::new(),
    };
    let (r, c) = crate::rebuild::model_sphere(&model).unwrap();
    assert_eq!(c, [0.0, 0.0, 0.5]);
    let far = (1.0f32 + 0.25 + 0.25).sqrt();
    assert!((r - far).abs() < 1e-6, "{r}");
    let empty = Model { geometries: Vec::new(), ..model };
    assert_eq!(crate::rebuild::model_sphere(&empty), None);
}

/// A Halo 1 function of the shape every Halo 1 vehicle's has: one,
/// scaled by input `k`, bounded.
fn h1_function(k: u16, bounds_mode: u16, bounds: [f32; 2]) -> crate::h1::Function {
    crate::h1::Function {
        period: 1.0,
        wobble: false,
        flags: 0,
        function: 0,
        scale_function_by: k,
        square_threshold: 0.0,
        result_scale: 0.0,
        scale_period_by: 0,
        map_to: 0,
        add: 0,
        scale_result_by: 0,
        step_count: 0,
        sawtooth_count: 0,
        bounds_mode,
        bounds,
        turn_off_with: -1,
    }
}

/// The warthog's functions: its inputs from the unit (A, driver's seat
/// power) and the vehicle (B jump, C engine hack); each output is its
/// input through Halo 1's bounds, and the engine turns off with A.
#[test]
fn halo1_functions_become_halo2_functions_by_their_inputs() {
    use crate::functions::*;
    use tags::effects::function::Function;
    let inputs = crate::h1::FunctionInputs {
        object: [0; 4],
        unit: [1, 0, 0, 0],
        vehicle: [0, 12, 35, 0],
        ..Default::default()
    };
    let mut engine = h1_function(3, 0, [0.0, 1.0]);
    engine.flags = 4;
    engine.turn_off_with = 0;
    let fs = [
        h1_function(1, 0, [0.0, 1.0]),
        h1_function(2, 2, [0.2, 0.6]),
        engine,
        h1_function(4, 1, [0.25, 0.75]),
        h1_function(4, 0, [0.25, 0.75]),
    ];
    let mut notes = Vec::new();
    let got = convert(&fs, &inputs, &mut notes);
    assert_eq!(got.len(), 4, "A to D only");
    // D's input is unset: not converted, and said so.
    assert!(got[3].is_err());
    assert_eq!(notes.len(), 1, "{notes:?}");
    let s: Vec<_> = got.iter().flatten().collect();
    assert_eq!(
        s.iter().map(|s| (s.import.as_str(), s.export.as_str(), s.turn_off_with.as_str(), s.flags)).collect::<Vec<_>>(),
        [
            ("driver_seat_power", "a_out", "", 0),
            ("jump", "b_out", "", 0),
            ("engine_hack", "c_out", "a_out", 4),
        ]
    );
    // Each blob, mapped onto its range, is Halo 1's bounding.
    let eval = |data: &[u8], x: f32| {
        let f = Function::parse(data).unwrap();
        let y = f.evaluate(x, 0.0);
        f.range[0] + (f.range[1] - f.range[0]) * y
    };
    for x in [0.0, 0.3, 1.0] {
        assert_eq!(eval(&s[0].data, x), x);
        // Scale to fit [0.2, 0.6].
        assert!((eval(&s[1].data, x) - (0.2 + 0.4 * x)).abs() < 1e-6);
    }
    // Clip to [0.25, 0.75], and clip and normalise.
    let clip = bounded_for_test(0, [0.25, 0.75]);
    let norm = bounded_for_test(1, [0.25, 0.75]);
    for (x, c, n) in [(0.0, 0.25, 0.0), (0.5, 0.5, 0.5), (0.6, 0.6, 0.7), (1.0, 0.75, 1.0)] {
        assert!((eval(&clip, x) - c).abs() < 1e-6, "clip {x}");
        assert!((eval(&norm, x) - n).abs() < 1e-6, "normalise {x}");
    }
    // Attachments name the outputs that converted; one naming an output
    // whose function did not convert, or that has none, is an error.
    assert_eq!(primary_scale(3, &got).unwrap(), "c_out");
    assert!(primary_scale(4, &got).is_err());
    assert_eq!(primary_scale(0, &got).unwrap(), "");
    assert!(primary_scale(4, &got[..3]).is_err());
}

#[test]
fn device_power_gain_follows_threshold_and_precedes_bounds() {
    let inputs = crate::h1::FunctionInputs { device: [1, 0, 0, 0], ..Default::default() };
    let mut ramp = h1_function(1, 0, [0.0, 1.0]);
    ramp.result_scale = 10.0;
    let mut switch = ramp.clone();
    switch.square_threshold = 0.5;
    let mut notes = Vec::new();
    let specs = crate::functions::convert(&[ramp, switch], &inputs, &mut notes);
    assert!(notes.is_empty(), "{notes:?}");
    for (i, spec) in specs.iter().enumerate() {
        let spec = spec.as_ref().unwrap();
        assert_eq!(spec.import, "power");
        let f = tags::effects::function::Function::parse(&spec.data).unwrap();
        for (power, expected) in [(0.05, [0.5, 0.0]), (0.5, [1.0, 0.0]), (0.5f32.next_up(), [1.0, 1.0])] {
            let actual = f.range[0] + (f.range[1] - f.range[0]) * f.evaluate(power, 0.0);
            assert_eq!(actual, expected[i]);
        }
    }
}

/// Halo 1's additive accumulator has no Halo 2 blob, and a function that
/// turns off with an unconverted one cannot carry the switch: both stay
/// unconverted with their reason, so a consumer of either fails.
#[test]
fn additive_and_orphaned_switch_functions_do_not_convert() {
    let inputs = crate::h1::FunctionInputs { unit: [1, 1, 0, 0], ..Default::default() };
    let mut additive = h1_function(1, 0, [0.0, 1.0]);
    additive.flags = 2;
    let mut switched = h1_function(2, 0, [0.0, 1.0]);
    switched.turn_off_with = 0;
    let mut notes = Vec::new();
    let got = crate::functions::convert(&[additive, switched], &inputs, &mut notes);
    assert!(got.iter().all(Result::is_err), "{got:?}");
    assert_eq!(notes.len(), 2, "{notes:?}");
    assert_eq!(
        crate::functions::primary_scale(1, &got).unwrap_err(),
        "output 1's function did not convert: additive accumulation, H1 0x8c06d"
    );
    assert_eq!(
        crate::functions::primary_scale(2, &got).unwrap_err(),
        "output 2's function did not convert: turns off with function 0, which has no converted output"
    );
}

#[test]
fn sentinel_shield_glow_exports_the_fourth_root_of_recent_damage() {
    let inputs = crate::h1::FunctionInputs { object: [4, 0, 0, 0], ..Default::default() };
    let mut glow = h1_function(1, 0, [0.0, 1.0]);
    glow.map_to = 2;
    let specs = crate::functions::convert(&[glow], &inputs, &mut Vec::new());
    let spec = specs[0].as_ref().unwrap();
    assert_eq!((spec.import.as_str(), spec.export.as_str()), ("current_shield_damage", "a_out"));
    let function = tags::effects::function::Function::parse(&spec.data).unwrap();
    assert!((function.evaluate(0.0625, 0.0) - 0.5).abs() < 0.01);
}

/// A wave slab for both sides of the periodic tests: pseudo-random bytes
/// in every slot, the shape the engine's own tables have (this repository
/// never holds them), so each kind reads a table of its own.
fn wave_slab() -> tags::effects::function::Tables {
    use tags::effects::function::{TABLE_LEN, Tables};
    let mut x = 0x1234_5678u32;
    let slab: Vec<u8> = (0..18 * TABLE_LEN)
        .map(|_| {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (x >> 24) as u8
        })
        .collect();
    Tables::procedural_owned().with_engine_slab(&slab).unwrap()
}

/// Halo 1's value of periodic `f` at `t` seconds with input `input`: the
/// wave at `t / period` read from `table` (slide waves 6 and 7 wrap
/// across the seam), times the scale and the input, then bounded.
fn h1_periodic_value(f: &crate::h1::Function, t: f32, input: f32, table: &[u8; 1024]) -> f32 {
    let v = t / f.period * 36.571_43;
    let frac = v - v.floor();
    let i = (v.floor() as i64 & 0x3ff) as usize;
    let a = f32::from(table[i]) / 255.0;
    let mut b = f32::from(table[(i + 1) & 0x3ff]) / 255.0;
    let slide = matches!(f.function, 6 | 7);
    if slide && a > 0.75 && b < 0.25 {
        b += 1.0;
    }
    let mut w = (1.0 - frac) * a + b * frac;
    if slide && w > 1.0 {
        w -= 1.0;
    }
    let gain = if f.result_scale > 0.0 { f.result_scale } else { 1.0 };
    let x = w * gain * input;
    let [lo, hi] = f.bounds;
    let y = match f.bounds_mode {
        0 => x.clamp(lo, hi),
        1 => ((x - lo) / (hi - lo)).clamp(0.0, 1.0),
        _ => lo + (hi - lo) * x,
    };
    y.clamp(0.0, 1.0)
}

/// Halo 2's value of a converted function at `t` seconds with its import
/// at `import`: `0x000ba8c0` evaluates the blob on time with range input
/// 1, maps it onto the header's range, multiplies by the import
/// (`0x000baa27`) and clamps.
fn h2_periodic_value(spec: &crate::functions::FunctionSpec, t: f32, import: f32, tables: &tags::effects::function::Tables) -> f32 {
    let f = tags::effects::function::Function::parse(&spec.data).unwrap();
    let y = f.evaluate_with(t, 1.0, tables);
    ((f.range[0] + (f.range[1] - f.range[0]) * y) * import).clamp(0.0, 1.0)
}

fn h1_periodic(kind: u16, scaled_by: u16, mode: u16, bounds: [f32; 2], scale: f32) -> crate::h1::Function {
    crate::h1::Function {
        function: kind,
        period: 2.5,
        result_scale: scale,
        ..h1_function(scaled_by, mode, bounds)
    }
}

/// Every periodic kind (2 to 11) converts to a Halo 2 periodic blob of
/// the same wave slot whose value, through Halo 2's export evaluator, is
/// Halo 1's over a run of times: unscaled with clip, normalise and
/// scale-to-fit bounds and a result scale, and scaled by an input where
/// the bounds start at 0.
#[test]
fn halo1_periodic_functions_match_halo1_through_halo2s_evaluator() {
    use tags::effects::function::FunctionType;
    let tables = wave_slab();
    let inputs = crate::h1::FunctionInputs { unit: [1, 0, 0, 0], ..Default::default() };
    for kind in 2..=11u16 {
        let table = tables.wave_table(kind as u8).unwrap();
        let cases = [
            (h1_periodic(kind, 0, 0, [0.0, 1.0], 0.0), [1.0]),
            (h1_periodic(kind, 0, 0, [0.2, 0.9], 0.8), [1.0]),
            (h1_periodic(kind, 0, 1, [0.1, 0.6], 0.0), [1.0]),
            (h1_periodic(kind, 0, 2, [0.3, 0.8], 0.5), [1.0]),
            (h1_periodic(kind, 1, 0, [0.0, 1.0], 0.0), [0.35]),
            (h1_periodic(kind, 1, 1, [0.0, 0.9], 0.7), [0.6]),
            (h1_periodic(kind, 1, 2, [0.0, 0.7], 1.2), [0.8]),
        ];
        for (f, [input]) in cases {
            let mut notes = Vec::new();
            let got = crate::functions::convert(std::slice::from_ref(&f), &inputs, &mut notes);
            let spec = got[0].as_ref().unwrap_or_else(|e| panic!("kind {kind}: {e}"));
            assert!(notes.is_empty(), "{notes:?}");
            let blob = tags::effects::function::Function::parse(&spec.data).unwrap();
            assert_eq!((blob.kind, blob.sub_types, blob.is_ranged()), (FunctionType::Periodic, [kind as u8; 2], false));
            let scaled = f.scale_function_by != 0;
            assert_eq!(spec.import, if scaled { "driver_seat_power" } else { "one" });
            let import = if scaled { input } else { 1.0 };
            for step in 0..400 {
                let t = step as f32 * 0.037;
                let (h1, h2) = (h1_periodic_value(&f, t, input, table), h2_periodic_value(spec, t, import, &tables));
                assert!((h1 - h2).abs() < 2e-3, "kind {kind} {f:?} at {t}: Halo 1 {h1}, Halo 2 {h2}");
            }
        }
    }
}

/// What a periodic blob can't hold is refused by name, never stood in
/// for: a scaled wave whose bounds don't start at 0 or that clips, a
/// period scale, a wobble, an invert, a map-to; and no reason says "is
/// not one" for a periodic kind.
#[test]
fn halo1_periodic_shapes_past_a_blob_are_named_refusals() {
    let inputs = crate::h1::FunctionInputs { unit: [1, 0, 0, 0], ..Default::default() };
    let refuse = |f: crate::h1::Function, why: &str| {
        let got = crate::functions::convert(&[f], &inputs, &mut Vec::new());
        let e = got[0].as_ref().unwrap_err();
        assert!(e.contains(why) && !e.contains("is not one"), "{e}");
    };
    refuse(h1_periodic(9, 1, 0, [0.2, 1.0], 0.0), "0xbaa27");
    refuse(h1_periodic(9, 1, 0, [0.0, 0.5], 0.0), "0xbaa27");
    refuse(h1_periodic(9, 1, 1, [0.0, 0.5], 0.8), "0xbaa27");
    refuse(crate::h1::Function { scale_period_by: 2, ..h1_periodic(3, 0, 0, [0.0, 1.0], 0.0) }, "period scaled");
    refuse(crate::h1::Function { wobble: true, ..h1_periodic(8, 0, 0, [0.0, 1.0], 0.0) }, "wobble");
    refuse(crate::h1::Function { flags: 1, ..h1_periodic(2, 0, 0, [0.0, 1.0], 0.0) }, "invert");
    refuse(crate::h1::Function { map_to: 2, ..h1_periodic(2, 0, 0, [0.0, 1.0], 0.0) }, "map-to 2");
    refuse(crate::h1::Function { period: 0.0, ..h1_periodic(2, 0, 0, [0.0, 1.0], 0.0) }, "period 0");
    refuse(crate::h1::Function { function: 12, ..h1_function(0, 0, [0.0, 1.0]) }, "function 12");
}

/// An input with no Halo 2 import names its object type and Halo 1
/// source; a vehicle's source comes before its unit's, an unset type's
/// before the object's base. An output that didn't convert carries its
/// reason to the attachment that names it.
#[test]
fn unconverted_inputs_name_their_type_and_source() {
    use crate::functions::{input_source, primary_scale};
    let inputs = crate::h1::FunctionInputs {
        object: [0, 3, 9, 0],
        unit: [7, 2, 0, 0],
        vehicle: [0, 12, 0, 0],
        weapon: [0; 4],
        device: [0; 4],
    };
    assert_eq!(input_source(&inputs, 0), ("unit", 7));
    assert_eq!(input_source(&inputs, 1), ("vehicle", 12));
    assert_eq!(input_source(&inputs, 2), ("object", 9));
    let fs = [h1_function(1, 0, [0.0, 1.0]), h1_function(2, 0, [0.0, 1.0]), h1_function(3, 0, [0.0, 1.0])];
    let got = crate::functions::convert(&fs, &inputs, &mut Vec::new());
    assert_eq!(got[0].as_ref().unwrap_err(), "input 1 (unit source 7) has no Halo 2 import");
    assert_eq!(got[1].as_ref().unwrap().import, "jump");
    assert_eq!(got[2].as_ref().unwrap_err(), "input 3 (object source 9) has no Halo 2 import");
    let weapon = crate::h1::FunctionInputs { weapon: [6, 0, 0, 0], ..Default::default() };
    let got = crate::functions::convert(&fs[..1], &weapon, &mut Vec::new());
    assert_eq!(
        primary_scale(1, &got).unwrap_err(),
        "output 1's function did not convert: input 1 (weapon source 6) has no Halo 2 import"
    );
}

fn bounded_for_test(mode: u16, bounds: [f32; 2]) -> Vec<u8> {
    let inputs = crate::h1::FunctionInputs { unit: [1, 0, 0, 0], ..Default::default() };
    let got = crate::functions::convert(&[h1_function(1, mode, bounds)], &inputs, &mut Vec::new());
    got[0].as_ref().unwrap().data.clone()
}

/// A rebuilt vehicle holds the functions given and each attachment's
/// primary scale.
#[test]
fn rebuilt_vehicle_holds_its_functions_and_primary_scales() {
    use crate::functions::FunctionSpec;
    use tags::defs::{self, Val};
    let def = |g: &[u8; 4]| tags::defs::Defs::builtin().group(FourCC(*g));
    let f = [FunctionSpec {
        flags: 4,
        import: "engine_hack".into(),
        export: "c_out".into(),
        turn_off_with: "a_out".into(),
        data: vec![0; 0x14],
    }];
    let a = [crate::rebuild::AttachmentSpec {
        group: *b"lsnd",
        tag: TagId(0xe100_0007),
        marker: String::new(),
        primary_scale: "c_out".into(),
        secondary_scale: "b_out".into(),
        change_colour: 0,
    }];
    let d = def(b"vehi").unwrap();
    let mut r = defs::default_record(d);
    crate::rebuild::object_effects(b"vehi", &mut r, "unit.obje", &a, None).unwrap();
    crate::rebuild::object_functions(b"vehi", &mut r, "unit.obje", &f).unwrap();
    let r = defs::from_built(d, &defs::to_built(d, &r, &mut |_| None).unwrap()).unwrap();
    assert_eq!(r.at("unit.obje.functions[0].import_name"), Some(&Val::text("engine_hack")));
    assert_eq!(r.at("unit.obje.functions[0].output"), Some(&Val::text("c_out")));
    assert_eq!(r.at("unit.obje.functions[0].turn_off_with"), Some(&Val::text("a_out")));
    assert_eq!(r.at("unit.obje.functions[0].default_function"), Some(&Val::Data(vec![0; 0x14])));
    assert_eq!(r.at("unit.obje.attachments[0].primary_scale"), Some(&Val::text("c_out")));
    assert_eq!(r.at("unit.obje.attachments[0].secondary_scale"), Some(&Val::text("b_out")));
    assert_eq!(r.at("unit.obje.attachments[0].change_colour"), Some(&Val::Int(0)));
}

#[test]
fn a_converted_particles_frame_always_has_a_function() {
    // The sprite draw path evaluates `frame` (prt3 +0x5c) and reads its
    // function's header byte with no null check (0x00246cfc): an empty
    // frame faults reading address 1. The base's static particles leave
    // it empty because they draw the static quad instead.
    use crate::particles::{first_sprite_frame, sprite_frame};
    use tags::defs::{self, Defs, Val};
    use tags::effects::function::{Function, FunctionType};
    let d = Defs::builtin().group(FourCC(*b"prt3")).unwrap();
    let mut r = defs::default_record(d);
    r.set("frame.function", Val::Data(Vec::new())).unwrap();
    sprite_frame(&mut r).unwrap();
    let Some(Val::Data(b)) = r.at("frame.function") else { panic!("frame function") };
    assert_eq!(b, &first_sprite_frame());
    assert_eq!(b.len(), 0x1c);
    let f = Function::parse(b).unwrap();
    assert_eq!(f.kind, FunctionType::Constant);
    assert!(!f.is_ranged() && !f.is_raw_output());
    assert_eq!(f.range, [0.0, 0.0]);
    // The first sprite whatever the inputs.
    for x in [0.0, 0.5, 1.0] {
        assert_eq!(f.evaluate(x, x), 0.0);
    }
    // It survives the write: the built tag holds the 0x1c bytes.
    let built = defs::to_built(d, &r, &mut |_| None).unwrap();
    let back = defs::from_built(d, &built).unwrap();
    assert_eq!(back.at("frame.function"), Some(&Val::Data(first_sprite_frame())));

    // Bytes that aren't a function are replaced too.
    r.set("frame.function", Val::Data(vec![1, 0, 0])).unwrap();
    sprite_frame(&mut r).unwrap();
    assert_eq!(r.at("frame.function"), Some(&Val::Data(first_sprite_frame())));

    // A function the base gave is kept.
    let mut kept = Function::parse(&first_sprite_frame()).unwrap();
    kept.range = [0.25, 0.75];
    r.set("frame.function", Val::Data(kept.to_bytes())).unwrap();
    sprite_frame(&mut r).unwrap();
    assert_eq!(r.at("frame.function"), Some(&Val::Data(kept.to_bytes())));
}

/// An invented Halo 1 trigger: flags at `+0`, charging time at `+0x48`.
fn h1_trigger(flags: u32, charging: f32) -> Vec<u8> {
    let mut t = vec![0u8; crate::h1::TRIGGER_SIZE];
    t[0..4].copy_from_slice(&flags.to_le_bytes());
    let at = crate::arsenal::H1_TRIGGER_CHARGING_TIME;
    t[at..at + 4].copy_from_slice(&charging.to_le_bytes());
    t
}

#[test]
fn every_halo1_trigger_gets_a_halo2_trigger_naming_its_barrel() {
    use crate::arsenal::{TriggerWords, h2_triggers};
    assert!(h2_triggers(&[]).is_empty());
    // An automatic rifle: one trigger, input 0, behaviour 0, barrel 0.
    let auto = h2_triggers(&[h1_trigger(0, 0.0)]);
    assert_eq!(auto, [TriggerWords::new(0, 0, 0, -1)]);
    assert_eq!(auto[0].0[1..4], [0, 0xffff_0000, 1]);
    // Once per press (flag 0x8): behaviour 1, `+0xc` 0.
    let semi = h2_triggers(&[h1_trigger(0x408, 0.0)]);
    assert_eq!(
        (semi[0].behaviour(), semi[0].primary(), semi[0].secondary(), semi[0].0[3]),
        (1, 0, -1, 0)
    );
    // Two triggers, neither charging: two Halo 2 triggers on inputs 0 and
    // 1, each firing its own barrel (a cannon and its machine gun).
    let two = h2_triggers(&[h1_trigger(0x28, 0.0), h1_trigger(0x20, 0.0)]);
    assert_eq!(
        two,
        [TriggerWords::new(0, 1, 0, -1), TriggerWords::new(1, 0, 1, -1)]
    );
    assert_eq!(two[1].input(), 1);
    // A charging first trigger with a second: one trigger, barrel 1 the
    // charged shot, behaviour and `+0xc` 2, the charge time carried.
    let charge = h2_triggers(&[h1_trigger(0, 0.6), h1_trigger(4, 0.0)]);
    assert_eq!(charge.len(), 1);
    let c = charge[0];
    assert_eq!(
        (c.input(), c.behaviour(), c.primary(), c.secondary(), c.0[3]),
        (0, 2, 0, 1, 2)
    );
    assert_eq!(c.charging_time(), 0.6);
    // A single-barrel fuel rod discharges the same barrel it charged.
    let fuel = h2_triggers(&[h1_trigger(8, 1.25)])[0];
    assert_eq!((fuel.behaviour(), fuel.primary(), fuel.secondary()), (2, 0, 0));
    assert_eq!((fuel.0[6], fuel.charging_time()), (1, 1.25));
}

/// Identical unflagged barrels must obey their own weapon's trigger
/// semantics. Pellet count and ammunition cost are independent of both.
#[test]
fn shotgun_and_automatic_have_distinct_inputs_on_the_same_barrel() {
    use crate::arsenal::{WeaponParts, weapon};
    use crate::h1::{TRIGGER_SIZE, Trigger, WEAP_SIZE, Weapon};
    use tags::defs::{self, Defs, Val};

    let mut source = vec![0; WEAP_SIZE + TRIGGER_SIZE];
    let t = &mut source[WEAP_SIZE..];
    t[4..8].copy_from_slice(&2.0_f32.to_le_bytes());
    t[0x22..0x24].copy_from_slice(&1_i16.to_le_bytes());
    t[0x6e..0x70].copy_from_slice(&15_i16.to_le_bytes());
    t[0x38..0x3c].copy_from_slice(&0.5_f32.to_le_bytes());
    let d = Defs::builtin().group(FourCC(*b"weap")).unwrap();
    for (kind, single, prediction) in [(1, 1_i16, 2), (0, 0, 1)] {
        let w = Weapon {
            weapon_type: kind,
            triggers: vec![Trigger::default()],
            ..Default::default()
        };
        let built = weapon(&w, &source, &WeaponParts::default()).unwrap();
        let r = defs::from_built(d, &built).unwrap();
        let Some(Val::Block(triggers)) = r.at("triggers") else {
            panic!("triggers")
        };
        let Some(Val::Block(barrels)) = r.at("barrels") else {
            panic!("barrels")
        };
        let bytes = |path, record| {
            defs::to_built(d.child(path).unwrap(), record, &mut |_| None)
                .unwrap()
                .bytes
        };
        let tr = bytes("triggers", &triggers[0]);
        let b = bytes("barrels", &barrels[0]);
        let short = |buf: &[u8], offset: usize| i16::from_le_bytes([buf[offset], buf[offset + 1]]);
        assert_eq!(short(&tr, 6), single);
        assert_eq!((short(&b, 0x1c), short(&b, 0x1e)), (single, single));
        assert_eq!(short(&b, 0x34), prediction);
        assert_eq!((short(&b, 0x2a), short(&b, 0x6a)), (1, 15));
        assert_eq!(&b[0x50..0x54], &2.0_f32.to_le_bytes());
        assert_eq!(&b[0x98..0x9c], &u32::MAX.to_le_bytes());
    }
}

/// A charging trigger carries Halo 1's charge after its charging time:
/// charged time, overcharged action, charged illumination and spew time
/// (Halo 1 `+0x4c..+0x5c`, Halo 2 `+0x20..+0x30`), as Blood Gulch's
/// plasma pistol holds them (45.0, 0, 1.0, 0); the overcharged action's
/// pad stays 0.
#[test]
fn a_charging_trigger_carries_its_charge() {
    use crate::arsenal::{H1_TRIGGER_CHARGED, h2_triggers};
    let mut first = h1_trigger(0, 0.6);
    let at = H1_TRIGGER_CHARGED;
    first[at..at + 4].copy_from_slice(&45.0f32.to_le_bytes());
    first[at + 4..at + 8].copy_from_slice(&0xabcd_0002u32.to_le_bytes());
    first[at + 8..at + 12].copy_from_slice(&1.0f32.to_le_bytes());
    first[at + 12..at + 16].copy_from_slice(&0.25f32.to_le_bytes());
    let c = h2_triggers(&[first, h1_trigger(4, 0.0)])[0];
    assert_eq!(c.charging_time(), 0.6);
    assert_eq!(c.charged_time(), 45.0);
    assert_eq!(c.overcharged_action(), 2);
    assert_eq!(c.0[9], 2, "the pad after the action stays 0");
    assert_eq!(c.charged_illumination(), 1.0);
    assert_eq!(c.spew_time(), 0.25);
    // A trigger that does not charge carries none of it.
    let mut plain = h1_trigger(0, 0.0);
    plain[at..at + 4].copy_from_slice(&45.0f32.to_le_bytes());
    assert_eq!(h2_triggers(&[plain])[0].charged_time(), 0.0);
}

#[test]
fn the_reserve_maximum_is_the_total_less_the_loaded() {
    use crate::arsenal::reserve_maximum;
    assert_eq!(reserve_maximum(600, 60), 540);
    assert_eq!(reserve_maximum(24, 4), 20);
    assert_eq!(reserve_maximum(0, 0), 0);
    assert_eq!(reserve_maximum(4, 8), 0);
}

/// A vehicle's wheels are the nodes its gripping powered mass points ride
/// and the nodes under them; hover points (the ghost's) and points on the
/// root are not wheels.
#[test]
fn wheel_nodes_are_the_gripping_points_nodes_and_below() {
    use crate::h1::{MassPoint, POWERED_GROUND_FRICTION, Physics};
    let node = |name: &str, parent: i16| h1_assets::models::h1::Node {
        name: name.into(),
        next_sibling: -1,
        first_child: -1,
        parent,
        translation: [0.0; 3],
        rotation: [0.0, 0.0, 0.0, 1.0],
        distance_from_parent: 0.0,
    };
    // hull, suspension, engine (under the suspension), tire (under the
    // engine), a gun on the hull.
    let nodes = [
        node("hull", -1),
        node("suspension", 0),
        node("engine", 1),
        node("tire", 2),
        node("gun", 0),
    ];
    let point = |powered: i16, model_node: i16| MassPoint {
        name: String::new(),
        powered_mass_point: powered,
        model_node,
        mass: 1.0,
        density: 1.0,
        position: [0.0; 3],
        radius: 0.25,
    };
    let phys = |flags: u32, points: Vec<MassPoint>| Physics {
        mass: 1.0,
        center_of_mass: [0.0; 3],
        density: 1.0,
        gravity_scale: 1.0,
        moments: [1.0; 3],
        powered_mass_points: 2,
        powered_flags: vec![flags, 0],
        antigravity: Vec::new(),
        mass_points: points,
    };
    let wheel_nodes = crate::convert::wheel_nodes;
    // The warthog's way: the gripping point rides the tire.
    let warthog = phys(POWERED_GROUND_FRICTION, vec![point(-1, 0), point(0, 3), point(1, 4)]);
    assert_eq!(wheel_nodes(&warthog, &nodes), vec![3]);
    // The scorpion's: on the suspension, so everything under it too.
    let tank = phys(POWERED_GROUND_FRICTION, vec![point(0, 1)]);
    assert_eq!(wheel_nodes(&tank, &nodes), vec![1, 2, 3]);
    // The ghost's antigravity points do not grip.
    let hover = phys(1 << 6, vec![point(0, 3)]);
    assert!(wheel_nodes(&hover, &nodes).is_empty());
    // A gripping point on the root leaves the body whole.
    let root = phys(POWERED_GROUND_FRICTION, vec![point(0, 0)]);
    assert!(wheel_nodes(&root, &nodes).is_empty());
}

/// A weapon fires Halo 1's effects: each barrel takes its own trigger's
/// firing effects (firing, misfire, empty) in order, with the damage the
/// parts give, and a barrel whose trigger has none gets none (before
/// converter version 97 every barrel took the Halo 2 counterpart's, the
/// last one's when it had fewer barrels). A magazine takes Halo 1's
/// reloading and chambering effects, and the root's Halo 1 references
/// win over the counterpart's.
#[test]
fn a_weapon_fires_and_reloads_with_halo1_s_effects() {
    use crate::arsenal::{
        FIRING_DAMAGE_REFS, FIRING_EFFECT_REFS, FiringParts, MAGAZINE_CHAMBERING_EFFECT,
        MAGAZINE_RELOADING_EFFECT, WeaponParts, weapon,
    };
    use crate::h1::{Magazine, TRIGGER_SIZE, Trigger, WEAP_SIZE, Weapon};
    use tags::defs::{self, Defs, Record, Target, Val};

    let w = Weapon {
        magazines: vec![Magazine { flags: 5, chamber_time: 1.25, ..Default::default() }],
        triggers: vec![Trigger::default(), Trigger::default()],
        ..Default::default()
    };
    let mut b = vec![0u8; WEAP_SIZE + 2 * TRIGGER_SIZE];
    b[WEAP_SIZE+0xc..WEAP_SIZE+0x10].copy_from_slice(&0.25f32.to_le_bytes());
    let shake = TagId(0xe123_0045);
    let (effe, snd) = (FourCC(*b"effe"), FourCC(*b"snd!"));
    let parts = WeaponParts {
        firing_effects: vec![vec![
            FiringParts {
                // Halo 1's empty effect may be a sound (the plasma rifle's).
                effects: [Some((effe, local(3))), None, Some((snd, local(4)))],
                damage: [Some(shake), None, None],
                shot_count: [2, 7],
            },
            FiringParts {
                effects: [Some((effe, local(5))), None, None],
                damage: [None; 3],
                shot_count: [8, 12],
            },
        ]],
        magazine_objects: vec![vec![(9, local(12))]],
        magazine_effects: vec![[
            Some((FourCC(*b"effe"), local(6))),
            Some((FourCC(*b"effe"), local(7))),
        ]],
        halo1: vec![("ready_effect".into(), FourCC(*b"snd!"), local(8))],
        ..Default::default()
    };
    let root = weapon(&w, &b, &parts).unwrap();
    let d = Defs::builtin().group(FourCC(*b"weap")).unwrap();
    let r = defs::from_built(d, &root).unwrap();
    let block = |r: &Record, p: &str| match r.at(p) {
        Some(Val::Block(b)) => b.clone(),
        x => panic!("{p}: {x:?}"),
    };
    let target = |r: &Record, p: &str| match r.at(p) {
        Some(Val::Ref {
            target: Target::Id(id),
            ..
        }) if *id != TagId::NULL => Some(*id),
        Some(Val::Ref { .. }) => None,
        x => panic!("{p}: {x:?}"),
    };
    let barrels = block(&r, "barrels");
    assert_eq!(barrels.len(), 2);
    assert_eq!(barrels[0].bytes("rate_of_fire_acceleration_rate", 4), Some(4.0f32.to_le_bytes().to_vec()));
    assert_eq!(barrels[1].bytes("rate_of_fire_acceleration_rate", 4), Some(1000.0f32.to_le_bytes().to_vec()));
    let fx = block(&barrels[0], "firing_effects");
    assert_eq!(fx.len(), 2);
    assert_eq!(fx[0].bytes("shot_count_lower_bound", 4), Some(vec![2, 0, 7, 0]));
    assert_eq!(fx[1].bytes("shot_count_lower_bound", 4), Some(vec![8, 0, 12, 0]));
    let effects = |e: &Record| FIRING_EFFECT_REFS.map(|p| target(e, p));
    let damage = |e: &Record| FIRING_DAMAGE_REFS.map(|p| target(e, p));
    assert_eq!(effects(&fx[0]), [Some(local(3)), None, Some(local(4))]);
    // The group word is the target's, written by the builder (a `snd!`
    // placeholder's is `snd!`): `from_built` reads it back as 0, so the
    // groups are checked on the built map (`weapon_sounds_disc`).
    assert_eq!(damage(&fx[0]), [Some(shake), None, None]);
    assert_eq!(effects(&fx[1]), [Some(local(5)), None, None]);
    assert!(
        block(&barrels[1], "firing_effects").is_empty(),
        "the second trigger has no firing effects of its own"
    );
    let mags = block(&r, "magazines");
    assert_eq!(mags[0].bytes("flags", 4), Some(5u32.to_le_bytes().to_vec()));
    assert_eq!(mags[0].bytes("chamber_time", 4), Some(1.25f32.to_le_bytes().to_vec()));
    let objects = block(&mags[0], "magazine_equipment");
    assert_eq!(objects[0].bytes("rounds", 4), Some(9u32.to_le_bytes().to_vec()));
    assert_eq!(target(&objects[0], "equipment"), Some(local(12)));
    assert_eq!(target(&mags[0], MAGAZINE_RELOADING_EFFECT), Some(local(6)));
    assert_eq!(target(&mags[0], MAGAZINE_CHAMBERING_EFFECT), Some(local(7)));
    assert_eq!(target(&r, "ready_effect"), Some(local(8)));
    // Slots no part names are empty.
    assert_eq!(target(&r, "pickup_sound"), None);
}

/// A wheel's pieces are cut at the hull's lowest point and what is left
/// is one piece; a wheel wholly under it gives none; a hull of no pieces
/// (an infinite floor) holds no wheel.
#[test]
fn held_wheels_keep_what_is_above_the_floor() {
    use crate::convert::held_wheels;
    // An axis box, lo..hi.
    let cube = |lo: [f64; 3], hi: [f64; 3]| {
        let mut planes = Vec::new();
        for a in 0..3 {
            let mut n = [0.0; 3];
            n[a] = 1.0;
            planes.push((n, -hi[a]));
            n[a] = -1.0;
            planes.push((n, lo[a]));
        }
        crate::physics::hull(&planes).unwrap()
    };
    let tread = vec![
        cube([-1.0, 0.5, -0.5], [0.0, 1.0, 0.5]),
        cube([0.0, 0.5, -0.5], [1.0, 1.0, 0.5]),
    ];
    let sunk = vec![cube([-1.0, -1.0, -0.5], [1.0, -0.5, -0.1])];
    let held = held_wheels(&[tread, sunk], 0.0);
    assert_eq!(held.len(), 1, "the sunk wheel gives nothing");
    let (lo, hi) = held[0].vertices.iter().fold(
        ([f32::INFINITY; 3], [f32::NEG_INFINITY; 3]),
        |(mut lo, mut hi), v| {
            for a in 0..3 {
                lo[a] = lo[a].min(v[a]);
                hi[a] = hi[a].max(v[a]);
            }
            (lo, hi)
        },
    );
    for (got, want) in lo
        .iter()
        .zip([-1.0, 0.5, 0.0])
        .chain(hi.iter().zip([1.0, 1.0, 0.5]))
    {
        assert!((got - want).abs() < 1e-3, "{lo:?}..{hi:?}");
    }
    assert!((held[0].volume - 0.5).abs() < 1e-3, "{}", held[0].volume);
    let one = cube([0.0, 0.0, -1.0], [1.0, 1.0, 1.0]).above(0.25).unwrap();
    assert!((one.volume - 0.75).abs() < 1e-4);
    assert!(cube([0.0; 3], [1.0; 3]).above(2.0).is_none());
    assert!(held_wheels(&[vec![cube([0.0; 3], [1.0; 3])]], f32::INFINITY).is_empty());
}

/// A rebuilt seat's gunner role is Halo 1's alone: the role word's player
/// gunner bit (`0x000cc810` makes a seat with it, ridden by a player, the
/// vehicle's gunner) is dropped, so a driver seat whose role word carries
/// it (the retail warthog driver's `0x48014`) does not fire the gun.
#[test]
fn a_rebuilt_seat_s_gunner_is_halo_1_s_alone() {
    use crate::rebuild::{SEAT_DRIVER, SEAT_GUNNER, SEAT_PLAYER_GUNNER, seat_flags};
    let driver = seat_flags(0x14, 0x48014);
    assert_eq!(driver & SEAT_PLAYER_GUNNER, 0, "{driver:#x}");
    assert_eq!(driver & SEAT_GUNNER, 0, "{driver:#x}");
    assert_ne!(driver & SEAT_DRIVER, 0, "{driver:#x}");
    // Halo 1's gunner keeps its bit whatever the role word.
    assert_ne!(seat_flags(0x198, 0) & SEAT_GUNNER, 0);
    assert_ne!(seat_flags(0x198, SEAT_PLAYER_GUNNER) & SEAT_GUNNER, 0);
    // Other role bits above Halo 1's pass through.
    assert_eq!(seat_flags(0, 1 << 20), 1 << 20);
}

/// A slot Halo 1 fills (`arsenal::Halo1Refs`) holds Halo 1's converted
/// tag, or none where Halo 1 names none, over the counterpart's; the
/// counterpart's other slots stay.
#[test]
fn only_the_source_projectile_slots_get_references() {
    use crate::arsenal::{self, PROJECTILE_DETONATION, PROJECTILE_FLYBY_SOUND};
    use tags::defs::{self, Defs, Target, Val};
    let d = Defs::builtin().group(FourCC(*b"proj")).unwrap();
    let mut r = defs::default_record(d);
    let halo1 = vec![
        arsenal::halo1_ref(PROJECTILE_FLYBY_SOUND, b"snd!", None),
        arsenal::halo1_ref(PROJECTILE_DETONATION, b"effe", Some(local(3))),
    ];
    arsenal::refs(&mut r, &halo1).unwrap();
    let target = |p: &str| match r.at(p) {
        Some(Val::Ref { target, .. }) => target.clone(),
        v => panic!("{p}: {v:?}"),
    };
    assert_eq!(target(PROJECTILE_FLYBY_SOUND), Target::Null);
    assert_eq!(target(PROJECTILE_DETONATION), Target::Id(local(3)));
    assert_eq!(target("impact_effect"), Target::Null);
}

#[test]
fn a_fixture_changes_the_light_emission_and_keeps_the_shape() {
    let source = crate::h1::Light {
        flags: 0,
        radius: 3.5,
        falloff_angle: 0.0,
        cutoff_angle: std::f32::consts::PI,
        colour_lower: [1.0; 4],
        colour_upper: [1.0; 4],
        duration_ticks: 0.0,
        lens_flare: None,
    };
    let original = crate::rebuild::light(&source).unwrap();
    let rebuilt = crate::rebuild::fixture_light(
        &original,
        h1convert::fixture::Lighting {
            rgb: [0.1, 0.3, 0.8],
            intensity: 0.25,
        },
    )
    .unwrap();
    let channel = |root: &map::build::Block, at: usize| {
        f32::from_le_bytes(root.bytes[at..at + 4].try_into().unwrap())
    };
    assert_eq!(channel(&rebuilt, 0x18), 3.5);
    assert_eq!(channel(&original, 0x70), 1.0);
    for field in [0x40, 0x4c, 0x58, 0x64] {
        assert_eq!(
            [
                channel(&rebuilt, field),
                channel(&rebuilt, field + 4),
                channel(&rebuilt, field + 8)
            ],
            [0.1, 0.3, 0.8],
        );
    }
    assert_eq!(
        [channel(&rebuilt, 0x70), channel(&rebuilt, 0x74)],
        [0.25; 2]
    );
}

/// Every projectile's gravity scales are Halo 1's times `GRAVITY_RATIO`, so
/// its fall under Halo 2's gravity is Halo 1's: a bullet (scale 1) keeps
/// Halo 1's drop and a grenade (scale 1.5, water 0.5) its arc.
#[test]
fn projectile_gravity_keeps_halo1s_arc() {
    use tags::defs::{self, Defs};
    use crate::rebuild::{GRAVITY_RATIO, SOURCE_GRAVITY};
    let d = Defs::builtin().group(map::FourCC(*b"proj")).unwrap();
    let h2_gravity = SOURCE_GRAVITY / GRAVITY_RATIO;
    for (air, water) in [(1.0f32, 1.0f32), (1.5, 0.5), (0.0, 0.0)] {
        let mut b = [0u8; crate::h1::PROJ_SIZE];
        b[0x1cc..0x1d0].copy_from_slice(&air.to_le_bytes());
        b[0x1d8..0x1dc].copy_from_slice(&water.to_le_bytes());
        let p = crate::h1::Projectile {
            object: crate::h1::Object {
                texture_override: 0,
                object_type: 0,
                acceleration_scale: 0.0,
                flags: 0,
                bounding_radius: 0.1,
                bounding_offset: [0.0; 3],
                model: None,
                animation: None,
                collision: None,
                physics: None,
                modifier_shader: None,
                creation_effect: None,
                attachments: Vec::new(),
                widgets: Vec::new(),
                change_colours: Vec::new(),
            },
            flags: 0,
            detonation_timer_starts: 0,
            impact_noise: 0,
            super_detonation: None,
            effect: None,
            ai_perception_radius: 0.0,
            collision_radius: 0.0,
            arming_time: 0.0,
            danger_radius: 0.0,
            timer: [0.0; 2],
            minimum_velocity: 0.0,
            maximum_range: 0.0,
            air_gravity_scale: air,
            air_damage_range: [0.0; 2],
            water_gravity_scale: water,
            water_damage_range: [0.0; 2],
            initial_velocity: 0.0,
            final_velocity: 0.0,
            guided_angular_velocity: 0.0,
            detonation_noise: 0,
            detonation_started: None,
            flyby_sound: None,
            attached_detonation_damage: None,
            impact_damage: None,
            material_responses: Vec::new(),
        };
        let root = crate::arsenal::projectile(&p, &b, &Default::default()).unwrap();
        let r = defs::from_built(d, &root).unwrap();
        for (path, h1_scale) in [("air_gravity_scale", air), ("water_gravity_scale", water)] {
            let got = f32::from_le_bytes(r.bytes(path, 4).unwrap()[..4].try_into().unwrap());
            assert_eq!(got, h1_scale * GRAVITY_RATIO, "{path}");
            assert!(
                (got * h2_gravity - h1_scale * SOURCE_GRAVITY).abs() < 1e-4,
                "{path}: the pull is Halo 1's"
            );
        }
    }
}

/// A Halo 1 vehicle built on a counterpart's drive: the drive spans and
/// gears are the counterpart's word for word; the model, bounding sphere,
/// seats and the points' markers, regions and radii stay Halo 1's.
/// `friction` picks the point kind; the counterpart holds `theirs` points.
fn drive_vehicle_keeps_halo1_and_takes_the_counterpart_drive(
    path: &str,
    kind: u16,
    friction: bool,
    ours: usize,
    theirs: usize,
) {
    use crate::rebuild::{
        DRIVE_SPANS, Point, VEHI_FRICTION, VEHI_GEARS, VEHI_HOVER, VEHI_SEATS, VehicleSpec,
    };
    use tags::defs::{self, Defs, Val};
    let def = Defs::builtin().group(map::FourCC(*b"vehi")).unwrap();
    // The counterpart: every named drive field holds its own number.
    let mut drive = defs::default_record(def);
    drive.set("vehicle_type", Val::Enum(kind.into())).unwrap();
    for (i, name) in [
        "maximum_forward_speed",
        "speed_acceleration",
        "steering_scale",
        "maximum_left_slide",
        "engine_moment",
        "seat_entrance_acceleration_scale",
        "thrust_multiplier",
        "ground_depth",
        "gravity_scale",
        "vehicle_physics_radius",
    ]
    .into_iter()
    .enumerate()
    {
        drive.set(name, Val::Float(1.5 + i as f32)).unwrap();
    }
    let gear = def.child(VEHI_GEARS).unwrap();
    let gears = (0..3)
        .map(|i| {
            let mut g = defs::default_record(gear);
            g.set("speed_ratio", Val::Float(i as f32 - 0.5)).unwrap();
            g
        })
        .collect();
    drive.set(VEHI_GEARS, Val::Block(gears)).unwrap();
    let block = if friction { VEHI_FRICTION } else { VEHI_HOVER };
    let point = def.child(block).unwrap();
    let their_points = (0..theirs)
        .map(|i| {
            let mut p = defs::default_record(point);
            p.set("radius", Val::Float(9.0)).unwrap();
            if friction {
                p.set("fraction_of_total_mass", Val::Float(1.0 / theirs as f32)).unwrap();
                p.set("flags", Val::Flags(0x26 + i as u32)).unwrap();
            } else {
                p.set("antigrav_strength", Val::Float(2.0 + i as f32)).unwrap();
            }
            p
        })
        .collect();
    drive.set(block, Val::Block(their_points)).unwrap();
    let seat_def = def.child(VEHI_SEATS).unwrap();
    let mut their_seat = defs::default_record(seat_def);
    their_seat.set("flags", Val::Flags(crate::rebuild::SEAT_DRIVER)).unwrap();
    drive.set(VEHI_SEATS, Val::Block(vec![their_seat])).unwrap();

    let obj = crate::h1::Object {
        texture_override: 0,
        object_type: 1,
        acceleration_scale: 0.0,
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
        change_colours: Vec::new(),
    };
    let seat = crate::h1::Seat {
        camera_angles: [0; 5],
        rates: [0; 2],
        flags: 0x1d,
        label: "driver".into(),
        marker: "driver".into(),
        camera_marker: "camera".into(),
        camera_tracks: Vec::new(),
    };
    let markers = vec!["driver".to_string(), "camera".to_string()];
    let seats = [crate::rebuild::seat_names(&seat, &markers)];
    let points: Vec<_> = (0..ours)
        .map(|i| Point {
            sphere: [i as f32, 1.0, -2.0, 0.25 + i as f32],
            hover: !friction,
            marker: format!("h1 {path} {i}"),
            region: format!("h1 region {i}"),
            region_index: i as i16,
        })
        .collect();
    let hlmt = map::TagId(0xe001_0002);
    let root = crate::rebuild::vehicle_with_tuning(
        &VehicleSpec {
            source: &[],
            references: &[],
            object: &obj,
            bounds: None,
            hlmt: Some(hlmt),
            seats: &seats,
            predicted: &[],
            camera_field_of_view: 1.0,
            vehicle_type: kind,
            flags: 0,
            weapons: &[],
            points: &points,
            attachments: &[],
            functions: &[],
            creation_effect: None,
        },
        None,
        Some(&drive),
    )
    .unwrap();
    let got = defs::from_built(def, &root).unwrap();

    // The drive is the counterpart's, field for field.
    let at = |r: &defs::Record, name: &str| r.fields.iter().position(|f| f.0 == name).unwrap();
    for (first, last) in DRIVE_SPANS {
        for i in at(&got, first)..=at(&got, last) {
            assert_eq!(got.fields[i], drive.fields[i], "{path}: {}", got.fields[i].0);
        }
    }
    assert_eq!(got.at(VEHI_GEARS), drive.at(VEHI_GEARS), "{path}: gears");
    assert_eq!(got.at("vehicle_type"), Some(&Val::Enum(kind.into())));

    // The model, bounds, seats and points are Halo 1's.
    assert_eq!(got.at("unit.obje.bounding_radius"), Some(&Val::Float(0.9)));
    assert_eq!(
        got.at("unit.obje.model"),
        Some(&Val::Ref { group: 0, target: tags::defs::Target::Id(hlmt) }),
        "{path}: model"
    );
    assert_eq!(got.at(&format!("{VEHI_SEATS}[0].seat_marker_name")), Some(&Val::text("driver")));
    assert_eq!(got.at(&format!("{VEHI_SEATS}[0].camera_marker_name")), Some(&Val::text("camera")));
    // A counterpart with no points of the kind (a flier) holds itself up
    // with its controller, and Halo 1's points are left out.
    if theirs == 0 {
        assert!(
            matches!(got.at(block), Some(Val::Block(b)) if b.is_empty())
                || matches!(got.at(block), Some(Val::Unresolved { count: 0, .. })),
            "{path}"
        );
    }
    for (i, p) in points.iter().enumerate().filter(|_| theirs != 0) {
        assert_eq!(got.at(&format!("{block}[{i}].marker_name")), Some(&Val::text(&p.marker)), "{path}");
        assert_eq!(got.bytes(&format!("{block}[{i}].radius"), 4), Some(p.sphere[3].to_le_bytes().to_vec()), "{path}");
        if friction {
            assert_eq!(got.at(&format!("{block}[{i}].region_name")), Some(&Val::text(&p.region)));
            assert_eq!(
                got.bytes(&format!("{block}[{i}].region_index"), 4),
                Some((i as u32).to_le_bytes().to_vec())
            );
            // One for one, a point's tuning is its counterpart's; else
            // the mass is shared equally.
            let mass = if ours == theirs { 1.0 / theirs as f32 } else { 1.0 / ours as f32 };
            assert_eq!(got.at(&format!("{block}[{i}].fraction_of_total_mass")), Some(&Val::Float(mass)));
            let source = if ours == theirs { i } else { i * theirs / ours };
            assert_eq!(got.at(&format!("{block}[{i}].flags")), Some(&Val::Flags(0x26 + source as u32)));
        } else {
            let source = if ours == theirs { i } else { i * theirs / ours };
            let strength = 2.0 + source as f32;
            assert_eq!(got.at(&format!("{block}[{i}].antigrav_strength")), Some(&Val::Float(strength)));
        }
    }
    // Nothing of the Halo 1 drive is left: no source gravity or contact
    // data in the phantom, no direct-drive engine word.
    assert_ne!(got.bytes("engine_moment", 4), Some(u32::from_le_bytes(*b"H1dd").to_le_bytes().to_vec()));
    if friction {
        // Where the removed Halo 1 contact and power data sat.
        for at in [0x180u32, 0x288] {
            assert_eq!(got.bytes(&format!("phantom_shapes[0].@{at:#x}"), 4), Some(vec![0; 4]), "{path}");
        }
    }
}

/// Halo 1's biped flags (`bipd +0x2f4`) set the Halo 2 flags and physics
/// word they name: bits 0, 5, 7 the `flags` of those names, bits 3, 4, 6
/// the physics word's bits 0, 1, 3, with the player and flying bits as
/// before; a bit with no Halo 2 counterpart sets nothing.
#[test]
fn biped_flags_carry_halo1s_named_bits() {
    use tags::defs::{self, Defs, Val};
    let d = Defs::builtin().group(map::FourCC(*b"bipd")).unwrap();
    let convert = |h1_flags: u32| {
        let unit = biped_unit(vec![(0x2f4, h1_flags)]);
        let root = crate::rebuild::biped(&unit, None, &[], &crate::rebuild::BipedParts::default())
            .unwrap();
        let r = defs::from_built(d, &root).unwrap();
        let physics = u32::from_le_bytes(r.bytes("unknown_264", 4).unwrap()[..4].try_into().unwrap());
        (r.at("flags").cloned(), physics)
    };
    let named = |bits: &[&str]| {
        let Some(defs::Kind::Flags { bits: names, .. }) = d.field("flags").map(|f| &f.kind) else {
            panic!("bipd flags");
        };
        Val::Flags(bits.iter().map(|b| 1u32 << names.iter().position(|n| n == b).unwrap()).sum())
    };
    assert_eq!(
        convert(1 << 0),
        (Some(named(&["turns_without_animating"])), 0)
    );
    assert_eq!(
        convert(1 << 5),
        (Some(named(&["passes_through_other_bipeds"])), 0)
    );
    assert_eq!(
        convert(1 << 7),
        (Some(named(&["immune_to_falling_damage"])), 0)
    );
    assert_eq!(convert(1 << 3), (Some(Val::Flags(0)), 0x1));
    assert_eq!(convert(1 << 4), (Some(Val::Flags(0)), 0x2));
    assert_eq!(convert(1 << 6), (Some(Val::Flags(0)), 0x8));
    // The player bit and the flying bit keep their physics words.
    assert_eq!(convert(1 << 1), (Some(Val::Flags(0)), 0x4));
    assert_eq!(convert(1 << 2), (Some(Val::Flags(0)), 0x13));
    // Halo 1's rotate-while-airborne (8) and limp body (9) are not mapped.
    assert_eq!(convert(1 << 8 | 1 << 9), (Some(Val::Flags(0)), 0));
    assert_eq!(
        convert(0xb1),
        (
            Some(named(&[
                "turns_without_animating",
                "passes_through_other_bipeds",
                "immune_to_falling_damage"
            ])),
            0x2
        )
    );
}

/// Every biped field Halo 1 has no mapped word for is given a Halo 1 word
/// (the death landing velocity), a declared value with its reason, or its
/// node in the converted model; none stays at the definition's zero.
#[test]
fn no_converted_biped_field_is_left_at_definition_zero() {
    use crate::rebuild::{BIPED_DECLARED, BIPED_NODES, BIPED_WORDS, BipedParts, Declared};
    use tags::defs::{self, Defs, Val};
    let d = Defs::builtin().group(map::FourCC(*b"bipd")).unwrap();
    // Every field the audit named is in one of the three tables.
    for path in [
        "death_hard_landing_velocity",
        "living_global_material_name",
        "dead_global_material_name",
        "living_global_material_index",
        "dead_global_material_index",
        "headshot_acceleration_scale",
        "physics_control_node_index",
        "pelvis_node_index",
        "head_node_index",
        "contact_points",
        "stun_duration",
        "unit.acceleration_range",
        "unit.look_speed_limit",
        "unit.look_acceleration_limit",
    ] {
        let held = BIPED_WORDS.iter().any(|w| w.0 == path)
            || BIPED_DECLARED.iter().any(|w| w.0 == path)
            || BIPED_NODES.iter().any(|w| w.0 == path);
        assert!(held, "{path} has no Halo 1 word, declared value or node");
    }
    assert!(BIPED_DECLARED.iter().all(|w| w.2.len() > 20), "each declared value says why");
    assert!(BIPED_DECLARED.iter().all(|w| d_has(d, w.0)));
    // The death landing velocity is Halo 1's (+0x3e8), as its neighbours are.
    let unit = biped_unit(BIPED_WORDS.iter().map(|w| (w.1, 0.25f32.to_bits())).collect());
    let build = |nodes: &[&str]| {
        let parts = BipedParts {
            nodes: nodes.iter().map(|n| n.to_string()).collect(),
            ..Default::default()
        };
        let root = crate::rebuild::biped(&unit, None, &[], &parts).unwrap();
        defs::from_built(d, &root).unwrap()
    };
    let r = build(&["bip01", "bip01 pelvis", "bip01 spine", "Bip01 Head"]);
    assert_eq!(r.at("death_hard_landing_velocity"), Some(&Val::Float(0.25)));
    for (path, declared, _) in BIPED_DECLARED {
        assert_eq!(r.at(path), Some(&declared.value()), "{path}");
    }
    assert_eq!(r.at("living_global_material_index"), Some(&Val::Int(-1)));
    assert_eq!(r.at("dead_global_material_index"), Some(&Val::Int(-1)));
    assert_eq!(BIPED_DECLARED.iter().find(|w| w.0 == "contact_points").unwrap().1, Declared::NoElements);
    assert_eq!(
        ["physics_control_node_index", "pelvis_node_index", "head_node_index"].map(|p| r.at(p).cloned()),
        [1, 1, 3].map(|v| Some(Val::Int(v)))
    );
    // A model without a pelvis or head (the monitor) has no such node: -1,
    // not node 0.
    let r = build(&["monitor body"]);
    assert_eq!(
        ["physics_control_node_index", "pelvis_node_index", "head_node_index"].map(|p| r.at(p).cloned()),
        [-1, -1, -1].map(|v| Some(Val::Int(v)))
    );
}

/// Whether the definition has a field at `path`, through embedded structs.
fn d_has(d: &tags::defs::StructDef, path: &str) -> bool {
    match path.split_once('.') {
        None => d.field(path).is_some(),
        Some((head, rest)) => match d.field(head).map(|f| &f.kind) {
            Some(tags::defs::Kind::Struct(s)) => d_has(s, rest),
            _ => false,
        },
    }
}

/// Each Halo 1 vehicle's path picks its counterpart.
#[test]
fn drive_counterparts_are_named_per_vehicle() {
    use crate::rebuild::drive_counterpart;
    let warthog: &[&str] = &["objects\\vehicles\\warthog\\warthog"];
    for path in [
        "vehicles\\warthog\\warthog",
        "vehicles\\rwarthog\\rwarthog",
        "vehicles\\rocket_warthog\\rocket_warthog",
    ] {
        assert_eq!(drive_counterpart(path), Some(warthog), "{path}");
    }
    for path in ["vehicles\\ghost\\ghost_mp", "vehicles\\ghost\\ghost"] {
        assert_eq!(
            drive_counterpart(path),
            Some(&["objects\\vehicles\\ghost\\ghost_mp", "objects\\vehicles\\ghost\\ghost"][..]),
            "{path}"
        );
    }
    assert_eq!(
        drive_counterpart("vehicles\\scorpion\\scorpion_mp"),
        Some(&["objects\\vehicles\\scorpion\\scorpion_mp", "objects\\vehicles\\scorpion\\scorpion"][..])
    );
    assert_eq!(
        drive_counterpart("vehicles\\banshee\\banshee"),
        Some(&["objects\\vehicles\\banshee\\banshee"][..])
    );
    // Props a recorded or cutscene animation moves take none.
    for path in ["vehicles\\banshee\\banshee_oldplayback", "vehicles\\scorpion\\scorpion_cinematic"] {
        assert_eq!(drive_counterpart(path), None, "{path}");
    }
    assert_eq!(drive_counterpart("vehicles\\pelican\\pelican"), None);
}

/// The warthog and rocket warthog: four wheels, the H2 warthog's four
/// friction points.
#[test]
fn warthog_drive_is_the_counterparts_and_the_rest_halo_1s() {
    drive_vehicle_keeps_halo1_and_takes_the_counterpart_drive(
        "vehicles\\warthog\\warthog",
        1,
        true,
        4,
        4,
    );
}

/// The ghost: three hover points, the H2 ghost's three.
#[test]
fn ghost_drive_is_the_counterparts_and_the_rest_halo_1s() {
    drive_vehicle_keeps_halo1_and_takes_the_counterpart_drive(
        "vehicles\\ghost\\ghost_mp",
        4,
        false,
        3,
        3,
    );
}

/// The scorpion: eight tread points of Halo 1 on the twelve of Halo 2's,
/// each taking the tuning at the same fraction of the list.
#[test]
fn scorpion_drive_is_the_counterparts_and_the_rest_halo_1s() {
    drive_vehicle_keeps_halo1_and_takes_the_counterpart_drive(
        "vehicles\\scorpion\\scorpion_mp",
        0,
        true,
        8,
        12,
    );
}

/// The banshee: the H2 banshee is a flier with no hover points, so none of
/// Halo 1's three are written and its controller flies it.
#[test]
fn banshee_drive_is_the_counterparts_and_the_rest_halo_1s() {
    drive_vehicle_keeps_halo1_and_takes_the_counterpart_drive(
        "vehicles\\banshee\\banshee",
        5,
        false,
        3,
        0,
    );
}

/// The rocket warthog is the warthog's drive with Halo 1's own model and
/// points: its path matches its own row before the warthog's.
#[test]
fn rocket_warthog_drive_is_the_counterparts_and_the_rest_halo_1s() {
    use crate::rebuild::DRIVE_COUNTERPARTS;
    let row = |path: &str| {
        let path = path.to_ascii_lowercase().replace('_', " ");
        DRIVE_COUNTERPARTS
            .iter()
            .position(|(words, _)| words.iter().all(|w| path.contains(w)))
            .unwrap()
    };
    assert_eq!(DRIVE_COUNTERPARTS[row("vehicles\\rwarthog\\rwarthog")].0, ["rwarthog"]);
    assert_eq!(DRIVE_COUNTERPARTS[row("vehicles\\rocket_warthog")].0, ["rocket warthog"]);
    drive_vehicle_keeps_halo1_and_takes_the_counterpart_drive(
        "vehicles\\rwarthog\\rwarthog",
        1,
        true,
        4,
        4,
    );
}

/// Halo 1's vehicle drive code is gone: the feature file, its
/// registration and the tag formats only it read.
#[test]
fn no_halo1_drive_code_remains() {
    let game = include_str!("../../../extensions/game/src/lib.rs");
    let boot = include_str!("../../../apps/boot/src/registration.rs");
    let tags = include_str!("../../../formats/tags/src/object/mod.rs");
    assert!(!game.contains("h1_drive"));
    assert!(!boot.contains("h1_drive") && !boot.contains("DirectDrive"));
    assert!(!tags.contains("direct_drive") && !tags.contains("h1_flight"));
    let file = concat!(env!("CARGO_MANIFEST_DIR"), "/../../extensions/game/src/h1_drive.rs");
    assert!(!std::path::Path::new(file).exists());
}

/// Builds a vehicle with Halo 1 seats `(flags, [yaw, pitch rate])` on a
/// counterpart whose seats are `(flags, entry radius, prompt)`, or on none
/// (a turret).
#[cfg(test)]
fn seat_vehicle(
    h1_seats: &[(u32, [f32; 2])],
    counterpart: Option<&[(u32, f32, &str)]>,
) -> tags::defs::Record {
    use crate::rebuild::{VEHI_SEATS, VehicleSpec};
    use tags::defs::{self, Defs, Val};
    let def = Defs::builtin().group(map::FourCC(*b"vehi")).unwrap();
    let drive = counterpart.map(|seats| {
        let mut d = defs::default_record(def);
        d.set("vehicle_type", Val::Enum(1)).unwrap();
        let seat_def = def.child(VEHI_SEATS).unwrap();
        let block = seats
            .iter()
            .map(|(flags, radius, prompt)| {
                let mut e = defs::default_record(seat_def);
                e.set("flags", Val::Flags(*flags)).unwrap();
                e.set("entry_radius", Val::Float(*radius)).unwrap();
                e.set("entry_marker_facing_angle", Val::Float(radius * 2.0)).unwrap();
                e.set("maximum_relative_velocity", Val::Float(radius * 3.0)).unwrap();
                e.set("enter_seat_string", Val::text(prompt)).unwrap();
                e.set("ai_seat_role", Val::Enum(*radius as u32)).unwrap();
                e
            })
            .collect();
        d.set(VEHI_SEATS, Val::Block(block)).unwrap();
        d
    });
    let obj = crate::h1::Object {
        texture_override: 0,
        object_type: 1,
        acceleration_scale: 0.0,
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
        change_colours: Vec::new(),
    };
    let seats: Vec<_> = h1_seats
        .iter()
        .map(|(flags, rates)| {
            let seat = crate::h1::Seat {
                camera_angles: [0; 5],
                rates: rates.map(f32::to_bits),
                flags: *flags,
                label: "seat".into(),
                marker: "seat".into(),
                camera_marker: String::new(),
                camera_tracks: Vec::new(),
            };
            crate::rebuild::seat_names(&seat, &[])
        })
        .collect();
    let root = crate::rebuild::vehicle_with_tuning(
        &VehicleSpec {
            source: &[],
            references: &[],
            object: &obj,
            bounds: None,
            hlmt: None,
            seats: &seats,
            predicted: &[],
            camera_field_of_view: 1.0,
            vehicle_type: 1,
            flags: 0,
            weapons: &[],
            points: &[],
            attachments: &[],
            functions: &[],
            creation_effect: None,
        },
        None,
        drive.as_ref(),
    )
    .unwrap();
    defs::from_built(def, &root).unwrap()
}

/// A seat takes its entry prompt and AI role from the paired counterpart
/// seat, and its turn rates from Halo 1.
#[test]
fn seats_have_prompts_and_turn_rates() {
    use crate::rebuild::{SEAT_DRIVER, SEAT_GUNNER};
    use tags::defs::Val;
    let got = seat_vehicle(
        &[(SEAT_DRIVER, [0.0, 0.0]), (SEAT_GUNNER, [1.5, 0.75])],
        Some(&[(SEAT_GUNNER, 4.0, "gunner prompt"), (SEAT_DRIVER, 2.0, "driver prompt")]),
    );
    assert_eq!(got.at("seats[0].enter_seat_string"), Some(&Val::text("driver prompt")));
    assert_eq!(got.at("seats[1].enter_seat_string"), Some(&Val::text("gunner prompt")));
    assert_eq!(got.at("seats[1].ai_seat_role"), Some(&Val::Enum(4)));
    assert_eq!(got.at("seats[1].yaw_rate_bounds"), Some(&Val::floats(&[0.0, 1.5])));
    assert_eq!(got.at("seats[1].pitch_rate_bounds"), Some(&Val::floats(&[0.0, 0.75])));
}

/// Seats of one kind pair in order, the extra ones with the last; a
/// counterpart-driven seat keeps the no-region word and its paired seat's
/// entry numbers, and a turret holds the turret role whole.
#[test]
fn each_seat_gets_its_own_entry_role() {
    use crate::rebuild::{SEAT_DRIVER, SEAT_GUNNER, SeatKind, TURRET_SEAT_ROLE, seat_kind};
    assert_eq!(seat_kind(SEAT_DRIVER | SEAT_GUNNER), SeatKind::Driver);
    assert_eq!(seat_kind(0), SeatKind::Passenger);
    let ours = [(SEAT_DRIVER, [0.0; 2]), (0, [0.0; 2]), (0, [0.0; 2]), (0, [0.0; 2])];
    let got = seat_vehicle(
        &ours,
        Some(&[(SEAT_DRIVER, 2.0, "d"), (0, 5.0, "p0"), (0, 6.0, "p1")]),
    );
    for (i, want) in [2.0f32, 5.0, 6.0, 6.0].into_iter().enumerate() {
        let word = |name: &str| got.bytes(&format!("seats[{i}].{name}"), 4).unwrap();
        assert_eq!(word("entry_radius"), want.to_le_bytes().to_vec(), "seat {i}");
        assert_eq!(word("entry_marker_facing_angle"), (want * 2.0).to_le_bytes().to_vec());
        assert_eq!(word("invisible_seat_region_index"), u32::MAX.to_le_bytes().to_vec());
        // The facing test stays open: Halo 1's entry markers face forward.
        assert_eq!(word("entry_marker_cone_angle"), std::f32::consts::PI.to_le_bytes().to_vec());
    }
    // A gunner with no counterpart gunner takes the counterpart's first seat.
    let got = seat_vehicle(&[(SEAT_GUNNER, [0.0; 2])], Some(&[(SEAT_DRIVER, 2.0, "d")]));
    assert_eq!(got.bytes("seats[0].entry_radius", 4).unwrap(), 2.0f32.to_le_bytes().to_vec());
    // A turret has no counterpart and holds the turret role.
    let got = seat_vehicle(&[(SEAT_GUNNER, [0.0; 2])], None);
    for (path, word) in TURRET_SEAT_ROLE {
        assert_eq!(got.bytes(&format!("seats[0].{path}"), 4).unwrap(), word.to_le_bytes().to_vec(), "{path}");
    }
}
