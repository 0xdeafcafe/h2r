//! The template, its coverage, its collision, the campaign AI and the
//! placements on content-kit's block-out invented map. No game data.

use std::io::Cursor;

use map::build::{Block, TagPayload, TagSet};
use map::{FourCC, Map};
use scenario_edit::BlockKind;
use tags::repo::{Node, Region, TagKey, Tree, Value};

use crate::build::{self, Request};
use crate::coverage::Census;
use crate::template::{Flag, FlagKind, Kind, Mode, Template, map_name};
use crate::{Error, ai};

fn template() -> Template {
    Template::builtin().expect("the shipped template parses")
}

const STAND_IN: content_kit::stage::Player = content_kit::stage::Player {
    height: 0.7,
    radius: 0.2,
};

#[test]
fn the_template_parses_with_sixteen_spawns_over_two_teams() {
    let t = template();
    let mp = &t.multiplayer;
    assert_eq!(mp.spawns.len(), 16);
    for team in [0, 1] {
        let n = mp.spawns.iter().filter(|s| s.team == Some(team)).count();
        assert_eq!(n, 8, "team {team}");
    }
    assert_eq!(mp.modes, Mode::ALL.to_vec(), "every retail mode");
    assert_eq!(mp.items.len(), 2, "one weapon and one vehicle");
    assert!(t.pieces.len() >= 8, "walls, a ramp and cover");
    assert!(!t.campaign.ai.zone.firing_positions.is_empty());
}

#[test]
fn every_mode_the_template_claims_starts() {
    let t = template();
    let c = Census::of_template(&t.multiplayer).unwrap();
    for m in Mode::ALL {
        assert_eq!(c.missing(m), None, "{}", m.name());
    }
    assert_eq!(c.items(), None);
    assert!(c.covers(&Mode::ALL).is_ok());
}

#[test]
fn a_missing_flag_is_named_with_its_mode() {
    let t = template();
    let drop = |pred: &dyn Fn(&Flag) -> bool| {
        let mut mp = t.multiplayer.clone();
        mp.flags.retain(|f| !pred(f));
        Census::of_template(&mp).unwrap()
    };
    let c = drop(&|f: &Flag| f.kind == FlagKind::CtfFlagReturn && f.team == Some(1));
    assert!(c.missing(Mode::Ctf).is_some());
    assert_eq!(c.missing(Mode::Assault), None);
    let c = drop(&|f: &Flag| f.kind == FlagKind::HillCorner);
    assert!(c.missing(Mode::King).is_some());
    let c = drop(&|f: &Flag| f.kind == FlagKind::OddballSpawn);
    let why = c.covers(&Mode::ALL).unwrap_err();
    assert!(why.contains("oddball"), "{why}");
    // The template is refused for a mode it claims and does not start.
    let text = crate::SOURCE.replace("kind: oddball_spawn", "kind: territory");
    assert_ne!(text, crate::SOURCE);
    match Template::parse(&text) {
        Err(Error::Template(why)) => assert!(why.contains("oddball"), "{why}"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_flag_names_are_the_builtin_labels() {
    for k in FlagKind::ALL {
        let ty = k.flag_type(2);
        let label = forge_doc::builtin_label(ty).expect("a label");
        assert_eq!(label.text, k.label(), "type {ty}");
    }
    assert_eq!(FlagKind::HillCorner.flag_type(7), 18);
}

#[test]
fn every_piece_generates_and_its_collision_compiles() {
    let t = template();
    for d in t.defs() {
        let solid = d.generate().unwrap_or_else(|e| panic!("{}: {e}", d.name()));
        let compiled = content_kit::blockout::collision::compile(&solid)
            .unwrap_or_else(|e| panic!("{}: {e:#}", d.name()));
        assert!(!compiled.bsp.surfaces.is_empty(), "{}", d.name());
    }
}

#[test]
fn the_floor_collision_compiles() {
    let t = template();
    let layout = t.layout(STAND_IN).unwrap();
    let mesh = content_kit::stage::mesh::Mesh::build(&layout);
    let compiled = mesh.collision().expect("the floor compiles");
    assert!(!compiled.bsp.surfaces.is_empty());
}

/// A piece's box in world units: `[min, max]` per axis.
fn piece_box(p: &crate::template::Piece) -> [[f32; 2]; 3] {
    std::array::from_fn(|a| [p.at[a], p.at[a] + f32::from(p.size[a]) / 100.0])
}

#[test]
fn the_pieces_stand_on_the_floor_apart_and_off_the_spawns() {
    let t = template();
    let layout = t.layout(STAND_IN).unwrap();
    let h = layout.floor.size / 2.0;
    let boxes: Vec<_> = t.pieces.iter().map(piece_box).collect();
    for (p, b) in t.pieces.iter().zip(&boxes) {
        assert_eq!(p.facing, 0.0, "{}: the boxes below are unturned", p.name);
        assert_eq!(b[2][0], 0.0, "{} stands on the floor", p.name);
        assert!(b[0][0] >= -h && b[0][1] <= h && b[1][0] >= -h && b[1][1] <= h, "{}", p.name);
    }
    for i in 0..boxes.len() {
        for j in 0..i {
            let inside = (0..3).all(|a| {
                boxes[i][a][0] < boxes[j][a][1] - 1e-3 && boxes[j][a][0] < boxes[i][a][1] - 1e-3
            });
            assert!(!inside, "{} and {} overlap", t.pieces[i].name, t.pieces[j].name);
        }
    }
    let mp = &t.multiplayer;
    let mut points: Vec<[f32; 3]> = mp.spawns.iter().map(|s| s.at).collect();
    points.extend(mp.flags.iter().map(|f| f.at));
    points.extend(mp.items.iter().map(|i| i.at));
    points.push(t.campaign.start.at);
    points.push(t.campaign.ai.squad.at);
    for p in points {
        // Inside the walls, and not inside a piece below its top.
        assert!(p[0].abs() < 10.0 && p[1].abs() < 10.0, "{p:?} is outside the walls");
        for (piece, b) in t.pieces.iter().zip(&boxes) {
            let under = (0..2).all(|a| p[a] > b[a][0] && p[a] < b[a][1]);
            assert!(!(under && p[2] < b[2][1]), "{p:?} is inside {}", piece.name);
        }
    }
}

#[test]
fn map_names_come_from_titles() {
    assert_eq!(map_name("My Yard").unwrap(), "my_yard");
    assert_eq!(map_name("  court-2 ").unwrap(), "court-2");
    for bad in ["", "2fast", "a/b", "this name is far too long to be a map name"] {
        assert!(matches!(map_name(bad), Err(Error::Name(_))), "{bad:?}");
    }
    assert_eq!(Kind::parse("Campaign").unwrap(), Kind::Campaign);
    assert!(Kind::parse("firefight").is_err());
}

#[test]
fn the_recipe_validates_for_both_kinds() {
    let t = template();
    for kind in Kind::ALL {
        let req = Request::new("Yard", kind).unwrap();
        let r = build::recipe(&t, &req).unwrap();
        let crates = r.place.iter().filter(|p| p.block == "crates").count();
        assert_eq!(crates, t.pieces.len(), "{kind:?}");
    }
}

// The campaign AI, written into a scenario root.

fn root(len: usize) -> Tree {
    Tree {
        region: Region::Metadata,
        nodes: vec![Node {
            bytes: vec![0; len],
            count: 1,
            size: len as u32,
            align: 4,
            fields: Vec::new(),
        }],
        raws: Vec::new(),
        losses: Vec::new(),
    }
}

fn count_at(tree: &Tree, node: usize, at: u32) -> u32 {
    let b = &tree.nodes[node].bytes;
    u32::from_le_bytes(b[at as usize..at as usize + 4].try_into().unwrap())
}

fn child_of(tree: &Tree, node: u32, at: u32) -> u32 {
    content_kit::stage::map::child(tree, node, at).unwrap_or_else(|| panic!("no block at +{at:#x}"))
}

#[test]
fn the_campaign_ai_is_one_squad_one_zone_and_its_order() {
    let t = template();
    let spec = &t.campaign.ai;
    let mut tree = root(0x3e0);
    let w = ai::write(&mut tree, spec).unwrap();
    assert_eq!((w.squads, w.zones, w.orders), (1, 1, 1));
    assert_eq!(w.firing_positions, spec.zone.firing_positions.len());
    for (at, n) in [(ai::SQUADS.0, 1), (ai::ZONES.0, 1), (ai::CHARACTERS.0, 1), (ai::ORDERS.0, 1)] {
        assert_eq!(count_at(&tree, 0, at), n, "+{at:#x}");
    }
    let palette = child_of(&tree, 0, ai::CHARACTERS.0);
    let named = tree.nodes[palette as usize]
        .fields
        .iter()
        .any(|(o, v)| *o == 4 && matches!(v, Value::Ref(k) if *k == TagKey::new(ai::CHAR, spec.character.clone())));
    assert!(named, "the palette names the character");
    let zone = child_of(&tree, 0, ai::ZONES.0);
    let fps = child_of(&tree, zone, ai::FIRING_POSITIONS.0);
    assert_eq!(tree.nodes[fps as usize].count as usize, spec.zone.firing_positions.len());
    let area = child_of(&tree, zone, ai::AREAS.0);
    let a = &tree.nodes[area as usize].bytes;
    let count = i16::from_le_bytes([a[0x3a], a[0x3b]]);
    assert_eq!(count as usize, spec.zone.firing_positions.len(), "the area holds them all");
    let squad = child_of(&tree, 0, ai::SQUADS.0);
    let s = &tree.nodes[squad as usize].bytes;
    assert_eq!(i16::from_le_bytes([s[0x2c], s[0x2d]]), spec.squad.count);
    assert_eq!(i16::from_le_bytes([s[0x38], s[0x39]]), 0, "in zone 0");
    assert_eq!(i16::from_le_bytes([s[0x42], s[0x43]]), 0, "under order 0");
    let starts = child_of(&tree, squad, ai::SQUAD_STARTS.0);
    assert_eq!(tree.nodes[starts as usize].count, 1);
}

#[test]
fn ai_blocks_the_base_still_holds_are_refused() {
    let t = template();
    let mut tree = root(0x3e0);
    tree.nodes[0].bytes[ai::SQUADS.0 as usize] = 3;
    match ai::write(&mut tree, &t.campaign.ai) {
        Err(Error::Template(why)) => assert!(why.contains("squads"), "{why}"),
        other => panic!("{other:?}"),
    }
    assert!(ai::write(&mut root(0x100), &t.campaign.ai).is_err());
}

#[test]
fn the_campaign_clear_list_covers_the_ai_blocks() {
    let t = template();
    let offsets: Vec<u32> = t.campaign.clear.iter().map(|c| c.offset().unwrap()).collect();
    for at in [ai::SQUADS.0, ai::ZONES.0, ai::CHARACTERS.0, ai::ORDERS.0] {
        assert!(offsets.contains(&at), "+{at:#x}");
    }
}

// The placements, on content-kit's block-out invented map with a weapon and a
// vehicle added.

fn invented(t: &Template) -> Vec<u8> {
    let mut map = Map::from_reader(Cursor::new(content_kit::blockout::fixture::fixture())).unwrap();
    let mut set = TagSet::from_map(&mut map).unwrap();
    content_kit::blockout::make_index_room(&mut set);
    for it in &t.multiplayer.items {
        let (g, path) = it.key().unwrap();
        set.add_group(FourCC(g), None, None);
        set.add_tag(FourCC(g), path, TagPayload::Built(Block::zeroed(0x10)))
            .unwrap();
    }
    let base = set.build().unwrap().0;
    let defs = t.defs();
    let headroom = forge_build::blockout::headroom(&base).unwrap();
    forge_build::blockout::install(&base, &defs, false, headroom)
        .expect("the pieces install")
        .bytes
        .expect("the pieces add tags")
}

#[test]
fn the_multiplayer_placements_start_every_mode() {
    let t = template();
    let req = Request::new("Yard", Kind::Multiplayer).unwrap();
    let (s, placed) = build::place(&t, &req, invented(&t)).expect("the placements are made");
    let sc = s.scenario().unwrap();
    assert_eq!(sc.block(BlockKind::Crates).len(), t.pieces.len());
    assert_eq!(sc.block(BlockKind::StartingLocations).len(), 16);
    assert_eq!(sc.block(BlockKind::NetgameFlags).len(), t.multiplayer.flags.len());
    assert_eq!(sc.block(BlockKind::NetgameEquipment).len(), 2);
    Census::read(sc).unwrap().covers(&Mode::ALL).unwrap();
    // Each crate is its piece's definition, at its corner.
    let crates = placed.placed.iter().filter(|p| p.block == "crates");
    for (p, c) in t.pieces.iter().zip(crates) {
        assert_eq!(c.tag.as_deref(), Some(p.def().tag_path().as_str()), "{}", p.name);
        assert_eq!(c.pos, p.at, "{}", p.name);
    }
    // The teams as the template gives them.
    for (i, sp) in t.multiplayer.spawns.iter().enumerate() {
        let team = sc.get(BlockKind::StartingLocations, i, "team").unwrap().as_int();
        assert_eq!(team, sp.team, "spawn {i}");
    }
}

#[test]
fn the_campaign_placements_are_the_pieces_and_one_start() {
    let t = template();
    let req = Request::new("Yard", Kind::Campaign).unwrap();
    let (s, _) = build::place(&t, &req, invented(&t)).expect("the placements are made");
    let sc = s.scenario().unwrap();
    assert_eq!(sc.block(BlockKind::Crates).len(), t.pieces.len());
    assert_eq!(sc.block(BlockKind::StartingLocations).len(), 1);
    assert_eq!(sc.block(BlockKind::NetgameFlags).len(), 0);
}

#[test]
fn a_map_without_a_named_tag_is_refused_by_name() {
    let t = template();
    let req = Request::new("Yard", Kind::Multiplayer).unwrap();
    // The pieces without the weapon and the vehicle.
    let base = content_kit::blockout::fixture::fixture();
    let headroom = forge_build::blockout::headroom(&base).unwrap();
    let bytes = forge_build::blockout::install(&base, &t.defs(), false, headroom)
        .unwrap()
        .bytes
        .unwrap();
    match build::place(&t, &req, bytes) {
        Err(Error::Place(why)) => assert!(why.contains("battle_rifle"), "{why}"),
        Err(other) => panic!("{other}"),
        Ok(_) => panic!("built without the weapon"),
    }
}

#[test]
fn the_package_is_refused_inside_a_git_checkout() {
    let t = template();
    let req = Request::new("Yard", Kind::Campaign).unwrap();
    let (session, placed) = build::place(&t, &req, invented(&t)).unwrap();
    let built = build::Built {
        request: req,
        base: "invented".into(),
        description: t.description.clone(),
        session,
        report: report_stub(placed),
    };
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(matches!(build::write(&built, here, false), Err(Error::Package(_))));
}

/// A report for a map built here without the floor's fresh build.
fn report_stub(placed: scenario_edit::recipe::Built) -> build::Report {
    let layout = template().layout(STAND_IN).unwrap();
    build::Report {
        key: String::new(),
        version: crate::VERSION,
        name: "yard".into(),
        title: "Yard".into(),
        kind: Kind::Campaign,
        base: "invented".into(),
        scenario: String::new(),
        pieces: Vec::new(),
        pieces_tag_memory: 0,
        headroom: 0,
        placed: placed.counts,
        notes: placed.notes,
        modes: Vec::new(),
        ai: None,
        stage: content_kit::stage::map::Report {
            key: String::new(),
            version: content_kit::stage::version(),
            name: "yard".into(),
            base: "invented".into(),
            scenario: String::new(),
            layout,
            own: Vec::new(),
            tags: 0,
            from_base: 0,
            raw: (0, 0),
            external: 0,
            bsps: 0,
            triangles: 0,
            vertices: 0,
            collision_surfaces: 0,
            lightmap_side: 0,
            lightmap_density: 0.0,
            file_size: 0,
            memory_size: 0,
            memory_max: 0,
            scenario_edit: Default::default(),
            gestalt: None,
            gestalt_whole: None,
        },
    }
}
