//! Synthetic tests: invented data only.

use std::io::Cursor;

use h1map::bsp::{Child, CollisionBsp, Leaf, Node3d};
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
    let (conv, report) = convert_collision(&c).unwrap();
    assert_eq!(
        (report.regions, report.permutations, report.bsps),
        (1, 2, 3)
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
