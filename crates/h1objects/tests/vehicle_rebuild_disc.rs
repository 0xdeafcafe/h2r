//! Every Halo 1 vehicle built from the `vehi` definition
//! (`h1objects::rebuild::vehicle`, the only vehicle path) against the
//! discs. Reads `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test vehicle_rebuild_disc -- --ignored --nocapture
//! ```
//!
//! - `vehicle_type_and_flags_match_halo_2s_counterparts`: on eight pairs of
//!   a Halo 1 vehicle and its Halo 2 counterpart, Halo 1's `vehi +0x2f4`
//!   is Halo 2's `vehicle_type` on every pair, Halo 1's `+0x2f0` is the
//!   low half of Halo 2's `vehicle_flags` on at least five, and the camera
//!   field of view agrees on every pair.
//! - `every_vehicle_rebuilds_from_its_definition`: every `vehi` of the ten
//!   campaign levels (on 05a_deltaapproach) and the multiplayer maps (on
//!   their bases, coagulation and beavercreek) converts; the `vehi`, its `hlmt`
//!   and `phmo` read back through the definitions, write the same again,
//!   pin nothing and hold no template bytes; the known fields are Halo 1's
//!   (type, flags, camera, bounds, seats, weapons) and the rest the type's
//!   role.
//! - `powered_mass_points_become_friction_and_hover_points`: the
//!   warthog's four tires become friction points and the ghost's three
//!   powered points hover points, each naming a marker of the converted
//!   model (on a node in range) and a region of Halo 1's collision; Halo
//!   1's mass point positions are in object space; the warthog has gears.
//! - `seat_flags_match_halo_2s_counterparts`: on 05a the boarding bit is
//!   set on exactly the retail `_b_` seats; Halo 1's seat bits stay below
//!   bit 11 and equal the counterpart seat's on most of the eight pairs'
//!   seats, the driver bit on every one.
//! - `every_seat_passes_the_engines_boarding_rule`: every seat of every
//!   vehicle (campaign and both sandboxes) passes the engine's enter-seat
//!   search for the converted Halo 1 player and, on the sandboxes, Halo
//!   2's `masterchief_mp`: flags, markers, entry distances and angles, and
//!   the label's enter animation.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree, template_candidates};
use h1objects::rebuild::{
    H1_SEAT_FLAGS, H1_VEHICLE_FLAGS, H1_VEHICLE_TYPE, SEAT_BOARDING, SEAT_DRIVER, SEAT_FLAGS, SEAT_GUNNER,
    SEAT_NEEDS_DRIVER, SEAT_NOT_NON_PLAYER, SEAT_NOT_PLAYER, VEHI_FRICTION, VEHI_GEARS, VEHI_HOVER, VEHI_PHYSICS, VEHI_SEATS,
};
use h1objects::{Converted, h1};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_map(want: &str) -> Map<Cursor<Vec<u8>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{want}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h1_level(name: &str) -> Option<H1Map> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name)?.clone();
    Some(H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap())
}

fn def(g: &[u8; 4]) -> &'static StructDef {
    Defs::builtin().group(FourCC(*g)).unwrap()
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// A field's offset in `d`, through embedded structs.
fn offset(d: &StructDef, path: &str, at: u32) -> u32 {
    let (head, rest) = path
        .split_once('.')
        .map_or((path, None), |(h, r)| (h, Some(r)));
    let f = d.fields.iter().find(|f| f.name == head).unwrap();
    match (rest, &f.kind) {
        (None, _) => at + f.offset,
        (Some(r), defs::Kind::Struct(s)) => offset(s, r, at + f.offset),
        _ => panic!("{path}"),
    }
}

/// Eight Halo 1 vehicles and their Halo 2 counterparts on 05a.
const PAIRS: &[(&str, &str, &str)] = &[
    ("b30", "vehicles\\warthog\\warthog", "objects\\vehicles\\warthog\\warthog"),
    ("b30", "vehicles\\ghost\\ghost", "objects\\vehicles\\ghost\\ghost"),
    ("a10", "vehicles\\scorpion\\scorpion", "objects\\vehicles\\scorpion\\scorpion"),
    ("b30", "vehicles\\banshee\\banshee", "objects\\vehicles\\banshee\\banshee"),
    ("a50", "vehicles\\wraith\\wraith", "objects\\vehicles\\wraith\\wraith"),
    ("a30", "vehicles\\pelican\\pelican", "objects\\vehicles\\pelican\\pelican"),
    ("a30", "vehicles\\c_dropship\\c_dropship", "objects\\vehicles\\phantom\\phantom"),
    (
        "a50",
        "vehicles\\c gun turret\\c gun turret",
        "objects\\vehicles\\c_turret_ap\\c_turret_ap",
    ),
];

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn vehicle_type_and_flags_match_halo_2s_counterparts() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let d = def(b"vehi");
    let (ty, fl, fov) = (
        offset(d, "vehicle_type", 0) as usize,
        offset(d, "vehicle_flags", 0) as usize,
        offset(d, "unit.camera_field_of_view", 0) as usize,
    );
    let mut levels = BTreeMap::new();
    let mut flags_agree = 0;
    for (level, h1_name, h2_name) in PAIRS {
        let h1 = levels.entry(*level).or_insert_with(|| h1_level(level).unwrap());
        let t = h1
            .tags()
            .iter()
            .find(|t| t.name == *h1_name && t.group.0.to_be_bytes() == *b"vehi")
            .unwrap_or_else(|| panic!("{h1_name}"));
        let b = h1.tag_data(t).unwrap();
        let u = h1::Unit::read(h1, t).unwrap();
        let r = base.root(base.find(b"vehi", h2_name).unwrap()).unwrap();
        let h1_type = u16::from_le_bytes([b[H1_VEHICLE_TYPE], b[H1_VEHICLE_TYPE + 1]]);
        let h2_type = u16::from_le_bytes([r[ty], r[ty + 1]]);
        assert_eq!(h1_type, h2_type, "{h1_name}: vehicle type");
        let (a, z) = (u32_at(b, H1_VEHICLE_FLAGS), u32_at(&r, fl));
        if a & 0xffff == z & 0xffff {
            flags_agree += 1;
        }
        assert_eq!(u.camera_field_of_view, f32_at(&r, fov), "{h1_name}: camera");
        println!(
            "{h1_name}: type {h1_type}; flags low half {:#06x} vs {:#06x}",
            a & 0xffff,
            z & 0xffff
        );
    }
    println!("flags low halves agree on {flags_agree} of {}", PAIRS.len());
    assert!(flags_agree >= 5, "flags: {flags_agree} pairs");
}

/// The tree's tag a local datum names.
fn local(tree: &Tree, id: TagId) -> Option<&Converted> {
    if id.0 & 0xffff_0000 != h1objects::convert::local(0).0 {
        return None;
    }
    tree.tags.get(id.0 as usize & 0xffff).map(|t| &t.1)
}

fn reference(r: &Record, path: &str) -> Option<TagId> {
    match r.at(path)? {
        Val::Ref {
            target: Target::Id(id),
            ..
        } => Some(*id),
        _ => None,
    }
}

fn pinned_blocks(d: &StructDef, r: &Record, path: &str, out: &mut Vec<String>) {
    for (f, (_, v)) in d.fields.iter().zip(&r.fields) {
        let p = format!("{path}.{}", f.name);
        match (&f.kind, v) {
            (_, Val::Unresolved { .. }) => out.push(p),
            (defs::Kind::Block(s), Val::Block(e)) => {
                for x in e {
                    pinned_blocks(s, x, &p, out);
                }
            }
            (defs::Kind::Struct(s), Val::Struct(x)) => pinned_blocks(s, x, &p, out),
            _ => {}
        }
    }
}

fn text(r: &Record, path: &str) -> String {
    match r.at(path) {
        Some(Val::String(Str::Text(t))) => t.clone(),
        _ => String::new(),
    }
}

/// Reads `c`'s root through its group's definition, checks it writes the
/// same again, pins nothing and holds no template bytes.
fn read_back(c: &Converted, name: &str) -> Record {
    let g = c.group.0;
    let d = def(&g);
    assert!(c.raw.is_empty(), "{name}: {} raw", c.group);
    let r = defs::from_built(d, &c.root).unwrap_or_else(|e| panic!("{name}: {e}"));
    let again = defs::to_built(d, &r, &mut |_| None).unwrap();
    assert_eq!(defs::from_built(d, &again).unwrap(), r, "{name}: {}", c.group);
    let mut pinned = Vec::new();
    pinned_blocks(d, &r, &c.group.to_string(), &mut pinned);
    assert!(pinned.is_empty(), "{name}: pinned {pinned:?}");
    r
}

/// The `hlmt`'s render, collision and animation tags' groups.
fn chain(tree: &Tree, r: &Record) -> Vec<Option<FourCC>> {
    let Some(h) = reference(r, "unit.obje.model").and_then(|x| local(tree, x)) else {
        return vec![None];
    };
    let hr = defs::from_built(def(b"hlmt"), &h.root).unwrap();
    let anim = match hr.at("animation") {
        Some(Val::Datum(Target::Id(id))) => local(tree, *id).map(|x| x.group),
        _ => None,
    };
    vec![
        Some(h.group),
        reference(&hr, "render_model")
            .and_then(|x| local(tree, x))
            .map(|x| x.group),
        reference(&hr, "collision_model")
            .and_then(|x| local(tree, x))
            .map(|x| x.group),
        anim,
    ]
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_vehicle_rebuilds_from_its_definition() {
    let only = std::env::var("H1_LEVELS").ok();
    let mut sets: Vec<(&str, Vec<String>)> = vec![(
        "05a_deltaapproach",
        h1map::disc::CAMPAIGN_LEVELS
            .iter()
            .map(|s| s.to_string())
            .collect(),
    )];
    for mp in ["coagulation", "beavercreek"] {
        sets.push((
            mp,
            h1convert::convert::MAPS
                .iter()
                .filter(|m| m.2 == mp)
                .map(|m| m.0.to_string())
                .collect(),
        ));
    }
    let d = def(b"vehi");
    let mut lines = Vec::new();
    let mut count = 0;
    let mut templated = 0;
    for (base_name, levels) in sets {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        let mut seen = HashSet::new();
        for level in levels {
            if only
                .as_deref()
                .is_some_and(|o| !o.split(',').any(|x| x == level))
            {
                continue;
            }
            let Some(h1) = h1_level(&level) else { continue };
            for t in h1.tags() {
                if t.group.0.to_be_bytes() != *b"vehi" || !seen.insert(t.name.clone()) {
                    continue;
                }
                let name = format!("{level} {}", t.name);
                let b = h1.tag_data(t).unwrap();
                let u = h1::Unit::read(&h1, t).unwrap();
                let mut c = Converter::new(&h1, &base);
                                let id = c.object(t).unwrap_or_else(|e| panic!("{name}: {e}"));
                let got = local(&c.tree, id).expect("vehi in the tree");
                assert_eq!(got.group, FourCC(*b"vehi"));
                let r = read_back(got, &name);

                // Halo 1's fields.
                let h1_type = u16::from_le_bytes([b[H1_VEHICLE_TYPE], b[H1_VEHICLE_TYPE + 1]]);
                assert_eq!(r.at("vehicle_type"), Some(&Val::Enum(h1_type.into())), "{name}");
                let flags = match r.at("vehicle_flags") {
                    Some(Val::Int(x)) => *x as u32,
                    x => panic!("{name}: flags {x:?}"),
                };
                assert_eq!(flags & 0xffff, u32_at(b, H1_VEHICLE_FLAGS) & 0xffff, "{name}");
                assert_eq!(r.at("unit.obje.object_type"), Some(&Val::Enum(1)));
                assert_eq!(
                    r.at("unit.obje.bounding_radius"),
                    Some(&Val::Float(u.object.bounding_radius)),
                    "{name}"
                );
                assert_eq!(
                    r.at("unit.camera_field_of_view"),
                    Some(&Val::Float(u.camera_field_of_view)),
                    "{name}"
                );
                let Some(Val::Block(seats)) = r.at(VEHI_SEATS) else { panic!() };
                assert_eq!(seats.len(), u.seats.len(), "{name}: seats");
                for (k, s) in u.seats.iter().enumerate() {
                    let p = format!("{VEHI_SEATS}[{k}]");
                    assert_eq!(
                        text(&r, &format!("{p}.label")),
                        h1models::names::map_name(&s.label).mode,
                        "{name}: seat {k} label"
                    );
                    // Halo 1's marker, or none where the model lacks it.
                    let m = text(&r, &format!("{p}.string_8"));
                    assert!(
                        m.is_empty() || m.eq_ignore_ascii_case(&s.marker),
                        "{name}: seat {k} marker {m:?}"
                    );
                }
                // Weapons: each a converted weap in the tree.
                let Some(Val::Block(weapons)) = r.at("unit.weapons") else { panic!() };
                for (k, w) in weapons.iter().enumerate() {
                    let Some(Val::Ref { target: Target::Id(id), .. }) = w.get("weapon") else {
                        panic!("{name}: weapon {k}")
                    };
                    assert_eq!(local(&c.tree, *id).map(|x| x.group), Some(FourCC(*b"weap")));
                }
                let h1_weapons = u
                    .references
                    .iter()
                    .filter(|x| x.tag.is_some() && x.group.0.to_be_bytes() == *b"weap")
                    .count();
                let Some(Val::Block(phys)) = r.at(VEHI_PHYSICS) else { panic!() };
                assert_eq!(phys.len(), 1, "{name}: every retail vehicle has one");

                // The type's role, where the base has the type.
                let role = c.vehicle_role(h1_type).unwrap();
                if let Some(role) = &role {
                    for (p, w) in &role.words.root {
                        assert_eq!(
                            r.at(p),
                            Some(&Val::Bytes(w.to_le_bytes().to_vec())),
                            "{name}: role {p}"
                        );
                    }
                    assert_eq!(flags & 0xffff_0000, role.flags_high, "{name}");
                }

                // The model chain: hlmt, mode, its coll and jmad; a phmo
                // (vehicle_new refuses a model with none), all from
                // definitions.
                let hlmt = reference(&r, "unit.obje.model").and_then(|x| local(&c.tree, x));
                let hlmt = hlmt.unwrap_or_else(|| panic!("{name}: no hlmt"));
                let h = read_back(hlmt, &name);
                let phmo = reference(&h, "physics_model")
                    .and_then(|x| local(&c.tree, x))
                    .unwrap_or_else(|| panic!("{name}: no phmo"));
                read_back(phmo, &name);
                let rc = chain(&c.tree, &r);
                assert_eq!(rc[1], Some(FourCC(*b"mode")), "{name}");
                assert_eq!(rc[2].is_some(), u.object.collision.is_some(), "{name}: coll");
                assert_eq!(rc[3].is_some(), u.object.animation.is_some(), "{name}: jmad");
                let path = if template_candidates(b"vehi", &t.name).is_empty() {
                    "no template row".to_owned()
                } else {
                    let mut tc = Converter::new(&h1, &base);
                    let tid = tc.object(t).unwrap_or_else(|e| panic!("{name}: template: {e}"));
                    let tr = defs::from_built(d, &local(&tc.tree, tid).unwrap().root).unwrap();
                    assert_eq!(chain(&tc.tree, &tr), rc, "{name}: model chain");
                    templated += 1;
                    "template path's chain".to_owned()
                };
                count += 1;
                lines.push(format!(
                    "{base_name}: {name}: type {h1_type}, {} seats {:?}, {}/{h1_weapons} weapons, role {}; {path}",
                    seats.len(),
                    seats
                        .iter()
                        .enumerate()
                        .map(|(k, _)| text(&r, &format!("{VEHI_SEATS}[{k}].label")))
                        .collect::<Vec<_>>(),
                    weapons.len(),
                    role.as_ref()
                        .map_or("turret tables".to_owned(), |x| format!(
                            "over {:?}",
                            x.words.members
                        )),
                ));
            }
        }
    }
    for l in &lines {
        println!("{l}");
    }
    println!("{count} vehicles rebuilt, {templated} with a template row compared");
    if only.is_none() {
        // 23 distinct campaign vehicles, 3 on each multiplayer base.
        assert!(count >= 29, "every vehicle of the discs: {count}");
    }
}

/// Halo 1 vehicles whose powered mass points become Halo 2 points: level,
/// name, the point block, and how many Halo 2's counterpart holds there.
const POINTED: &[(&str, &str, &str, usize)] = &[
    ("b30", "vehicles\\warthog\\warthog", VEHI_FRICTION, 4),
    ("b30", "vehicles\\ghost\\ghost", VEHI_HOVER, 3),
];

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn powered_mass_points_become_friction_and_hover_points() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let h1 = h1_level("b30").unwrap();
    let d = def(b"vehi");
    for (_, name, block, want) in POINTED {
        let t = h1
            .tags()
            .iter()
            .find(|t| t.name == *name && t.group.0.to_be_bytes() == *b"vehi")
            .unwrap();
        let u = h1::Unit::read(&h1, t).unwrap();
        let phys = h1::Physics::read(&h1, h1.tag(u.object.physics.unwrap()).unwrap()).unwrap();
        let model = h1models::model_from_map(&h1, h1.tag(u.object.model.unwrap()).unwrap(), &Default::default())
            .unwrap()
            .0;
        // Halo 1's mass point positions are in the object's space: each
        // powered point lies within its radius (plus a tenth of a unit)
        // of its node's default position.
        let world = h1models::mode::world_transforms(
            &model
                .nodes
                .iter()
                .map(|n| (n.parent, n.rotation, n.translation))
                .collect::<Vec<_>>(),
        );
        let powered: Vec<_> = phys.mass_points.iter().filter(|m| m.powered_mass_point >= 0).collect();
        assert_eq!(powered.len(), *want, "{name}: Halo 1 powered points");
        if *block == VEHI_FRICTION {
            for m in &powered {
                let t = world[m.model_node as usize].1;
                let dist = (0..3).map(|a| (m.position[a] - t[a]).powi(2)).sum::<f32>().sqrt();
                assert!(dist <= m.radius + 0.1, "{name}: {} {dist} from its node", m.name);
            }
        }

        let mut c = Converter::new(&h1, &base);
                let id = c.object(t).unwrap();
        let r = read_back(local(&c.tree, id).unwrap(), name);
        let Some(Val::Block(points)) = r.at(block) else { panic!() };
        assert_eq!(points.len(), *want, "{name}: {block}");
        let other = if *block == VEHI_FRICTION { VEHI_HOVER } else { VEHI_FRICTION };
        assert!(matches!(r.at(other), Some(Val::Block(b)) if b.is_empty()), "{name}");
        // Each point's marker is a marker of the converted model, on a
        // node in range; each region a region of Halo 1's collision.
        let h = defs::from_built(
            def(b"hlmt"),
            &reference(&r, "unit.obje.model").and_then(|x| local(&c.tree, x)).unwrap().root,
        )
        .unwrap();
        let mode = reference(&h, "render_model").and_then(|x| local(&c.tree, x)).unwrap();
        let m = defs::from_built(def(b"mode"), &mode.root).unwrap();
        let Some(Val::Block(groups)) = m.at("marker_groups") else { panic!() };
        let coll = h1::Collision::read(&h1, h1.tag(u.object.collision.unwrap()).unwrap()).unwrap();
        let regions: Vec<String> = coll.regions.iter().map(|r| h1objects::coll::sid_name(&r.name)).collect();
        let (mk, rg) = if *block == VEHI_FRICTION {
            ("string_0", "string_44")
        } else {
            ("string_0", "string_34")
        };
        for (k, p) in points.iter().enumerate() {
            let marker = match p.get(mk) {
                Some(Val::String(Str::Text(x))) => x.clone(),
                x => panic!("{name}: point {k} marker {x:?}"),
            };
            let g = groups
                .iter()
                .find(|g| g.get("name") == Some(&Val::String(Str::Text(marker.clone()))))
                .unwrap_or_else(|| panic!("{name}: no marker {marker}"));
            let Some(Val::Block(ms)) = g.get("markers") else { panic!() };
            assert!(!ms.is_empty(), "{name}: {marker} has instances");
            for x in ms {
                let Some(Val::Int(n)) = x.get("node") else { panic!() };
                assert!((*n as usize) < model.nodes.len(), "{name}: {marker} node");
            }
            let region = match p.get(rg) {
                Some(Val::String(Str::Text(x))) => x.clone(),
                x => panic!("{name}: point {k} region {x:?}"),
            };
            assert!(regions.contains(&region), "{name}: {marker} region {region} of {regions:?}");
            println!("{name}: {block}[{k}] marker {marker}, region {region}, {} instances", ms.len());
        }
        let Some(Val::Block(gears)) = r.at(VEHI_GEARS) else { panic!() };
        println!("{name}: {} gears", gears.len());
        if *block == VEHI_FRICTION {
            assert!(!gears.is_empty(), "{name}: a jeep has gears");
        }
        let _ = d;
    }
}

/// A seat element's 4-byte unknown word at `field`.
fn word(x: &Record, field: &str) -> u32 {
    match x.get(field) {
        Some(Val::Bytes(b)) if b.len() == 4 => u32::from_le_bytes(b[..4].try_into().unwrap()),
        v => panic!("{field}: {v:?}"),
    }
}

fn string(x: &Record, field: &str, strings: &[Option<String>]) -> String {
    match x.get(field) {
        Some(Val::String(Str::Text(t))) => t.clone(),
        Some(Val::String(Str::Index(w))) => strings
            .get((w & 0xff_ffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_default(),
        _ => String::new(),
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn seat_flags_match_halo_2s_counterparts() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let strings = base_map.strings().to_vec();
    let d = def(b"vehi");
    let mut levels = BTreeMap::new();
    let (mut matched, mut agree) = (0, 0);
    // Every retail seat on 05a: a boarding seat (`_b_` in its label, h1-scripts.md
    // "Seats") holds SEAT_BOARDING, no other does.
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"vehi")) {
        let r = base.record(d, t.0).unwrap();
        let Some(Val::Block(seats)) = r.at(VEHI_SEATS) else { panic!() };
        for x in seats {
            let label = string(x, "label", &strings);
            assert_eq!(
                word(x, SEAT_FLAGS) & SEAT_BOARDING != 0,
                label.contains("_b_"),
                "{}: {label}",
                t.2
            );
        }
    }
    for (level, h1_name, h2_name) in PAIRS {
        let h1 = levels.entry(*level).or_insert_with(|| h1_level(level).unwrap());
        let t = h1
            .tags()
            .iter()
            .find(|t| t.name == *h1_name && t.group.0.to_be_bytes() == *b"vehi")
            .unwrap();
        let u = h1::Unit::read(h1, t).unwrap();
        let r = base.record(d, base.find(b"vehi", h2_name).unwrap()).unwrap();
        let Some(Val::Block(seats)) = r.at(VEHI_SEATS) else { panic!() };
        for s in &u.seats {
            let mode = h1models::names::map_name(&s.label).mode;
            let Some(x) = seats.iter().find(|x| string(x, "label", &strings) == mode) else {
                continue;
            };
            let h2 = word(x, SEAT_FLAGS);
            matched += 1;
            assert_eq!(s.flags & !H1_SEAT_FLAGS, 0, "{h1_name} {}: Halo 1 bits", s.label);
            assert_eq!(s.flags & SEAT_DRIVER, h2 & SEAT_DRIVER, "{h1_name} {}: driver", s.label);
            if s.flags & H1_SEAT_FLAGS == h2 & H1_SEAT_FLAGS {
                agree += 1;
            }
            println!("{h1_name} {}: {:#05x} vs {mode} {h2:#07x}", s.label, s.flags);
        }
    }
    println!("Halo 1's seat bits equal the counterpart's on {agree} of {matched} seats");
    assert!(matched >= 8 && 2 * agree > matched, "{agree} of {matched}");
}

/// The animations the engine's seat search asks the rider's graph for
/// under the seat's label (`0x000c7160` calls `0x0010f9b0` with each of the
/// five string IDs at `0x00467430`): a seat whose label has none of them
/// in the rider's graph is never entered.
const ENTER: [&str; 5] = ["enter", "enter_a", "enter_b", "enter_c", "enter_d"];

/// A graph's modes and, for each, every action name under it (any weapon
/// class and type).
fn modes(r: &Record, strings: &[Option<String>]) -> BTreeMap<String, HashSet<String>> {
    let mut out = BTreeMap::<String, HashSet<String>>::new();
    let Some(Val::Block(ms)) = r.at("modes") else { return out };
    for m in ms {
        let e = out.entry(string(m, "name", strings)).or_default();
        let Some(Val::Block(cs)) = m.get("classes") else { continue };
        for c in cs {
            let Some(Val::Block(ts)) = c.get("types") else { continue };
            for t in ts {
                let Some(Val::Block(xs)) = t.get("actions") else { continue };
                e.extend(xs.iter().map(|a| string(a, "name", strings)));
            }
        }
    }
    out
}

/// Whether `graph` lets a rider enter a seat labelled `label`.
fn enters(graph: &BTreeMap<String, HashSet<String>>, label: &str) -> bool {
    graph.get(label).is_some_and(|a| ENTER.iter().any(|e| a.contains(*e)))
}

/// A marker group of the converted model `groups` by `name`, with its
/// instance count.
fn instances(groups: &[Record], name: &str) -> usize {
    groups
        .iter()
        .find(|g| g.get("name") == Some(&Val::String(Str::Text(name.to_owned()))))
        .map_or(0, |g| match g.get("markers") {
            Some(Val::Block(m)) => m.len(),
            _ => 0,
        })
}

/// The converted graph of the converted unit `id`.
fn converted_graph(tree: &Tree, group: &[u8; 4], id: TagId) -> Option<BTreeMap<String, HashSet<String>>> {
    let r = defs::from_built(def(group), &local(tree, id)?.root).ok()?;
    let h = defs::from_built(def(b"hlmt"), &local(tree, reference(&r, "unit.obje.model")?)?.root).ok()?;
    let Some(Val::Datum(Target::Id(a))) = h.at("animation") else { return None };
    let g = defs::from_built(def(b"jmad"), &local(tree, *a)?.root).ok()?;
    Some(modes(&g, &[]))
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_seat_passes_the_engines_boarding_rule() {
    // The enter-seat search (`0x000c8bb0`) over each seat of each
    // vehicle, as the engine applies it to a rider:
    // - the seat's flags let the rider in (`0x000c8c87`): not closed to
    //   players nor to non-players, not a boarding seat (`0x000c92c0`),
    //   and a seat needing a driver is not the driver's;
    // - the entry marker (`+0xc`, `0x000c6fb0`) and the rider's marker
    //   (`+8`, `0x000c7160`) resolve through `0x000b8d30`: an empty name
    //   is the object's origin, a name the model lacks fails the seat;
    // - the entry tests (`0x000c9040`) compare the rider's distance and
    //   angles with the seat's `+0x98`, `+0x9c`, `+0xa0`, `+0xa4`: zero
    //   admits no one;
    // - the rider's graph has the seat's label as a mode with one of
    //   [`ENTER`] (`0x000c7160`).
    // The riders: each level's converted Halo 1 player (`cyborg`,
    // `cyborg_mp`), and on the multiplayer bases Halo 2's sandbox player
    // (`masterchief_mp`). Each seat whose label Halo 1's own player graph
    // has an enter animation for must take the converted player, and on
    // the sandboxes, where it is a driver's or gunner's, Halo 2's.
    let only = std::env::var("H1_LEVELS").ok();
    let mut sets: Vec<(&str, Vec<String>)> = vec![(
        "05a_deltaapproach",
        h1map::disc::CAMPAIGN_LEVELS.iter().map(|s| s.to_string()).collect(),
    )];
    for mp in ["coagulation", "beavercreek"] {
        sets.push((
            mp,
            h1convert::convert::MAPS
                .iter()
                .filter(|m| m.2 == mp)
                .map(|m| m.0.to_string())
                .collect(),
        ));
    }
    let d = def(b"vehi");
    let mut seats_checked = 0;
    let mut failures = Vec::new();
    for (base_name, levels) in sets {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        let strings = base_map.strings().to_vec();
        let multiplayer = base_name != "05a_deltaapproach";
        // Halo 2's sandbox player.
        let sandbox = multiplayer.then(|| {
            let id = base.find(b"bipd", "objects\\characters\\masterchief\\masterchief_mp").unwrap();
            let b = base.record(def(b"bipd"), id).unwrap();
            let h = base.record(def(b"hlmt"), reference(&b, "unit.obje.model").unwrap()).unwrap();
            let Some(Val::Datum(Target::Id(a))) = h.at("animation") else { panic!() };
            modes(&base.record(def(b"jmad"), *a).unwrap(), &strings)
        });
        for level in levels {
            if only.as_deref().is_some_and(|o| !o.split(',').any(|x| x == level)) {
                continue;
            }
            let Some(h1) = h1_level(&level) else { continue };
            let mut c = Converter::new(&h1, &base);
            // The level's Halo 1 player and its graph, converted.
            let mut players = Vec::new();
            for t in h1.tags().iter().filter(|t| {
                t.group.0.to_be_bytes() == *b"bipd" && t.name.rsplit('\\').next().is_some_and(|n| n.starts_with("cyborg"))
            }) {
                let u = h1::Unit::read(&h1, t).unwrap();
                let h1_graph =
                    h1models::graph_from_map(&h1, h1.tag(u.object.animation.unwrap()).unwrap(), None)
                        .unwrap()
                        .0;
                let id = c.object(t).unwrap();
                let g = converted_graph(&c.tree, b"bipd", id).unwrap_or_else(|| panic!("{level} {}: graph", t.name));
                players.push((t.name.clone(), h1_graph, g));
            }
            assert!(!players.is_empty(), "{level}: a cyborg");
            for t in h1.tags().iter().filter(|t| t.group.0.to_be_bytes() == *b"vehi") {
                let name = format!("{level} {}", t.name);
                let u = h1::Unit::read(&h1, t).unwrap();
                let id = c.object(t).unwrap_or_else(|e| panic!("{name}: {e}"));
                let r = defs::from_built(d, &local(&c.tree, id).unwrap().root).unwrap();
                let h = defs::from_built(def(b"hlmt"), &local(&c.tree, reference(&r, "unit.obje.model").unwrap()).unwrap().root).unwrap();
                let mode = defs::from_built(def(b"mode"), &local(&c.tree, reference(&h, "render_model").unwrap()).unwrap().root).unwrap();
                let Some(Val::Block(groups)) = mode.at("marker_groups") else { panic!() };
                let Some(Val::Block(seats)) = r.at(VEHI_SEATS) else { panic!() };
                for (k, (x, s)) in seats.iter().zip(&u.seats).enumerate() {
                    seats_checked += 1;
                    let mut fail = |why: String| failures.push(format!("{base_name}: {name} seat {k} {}: {why}", s.label));
                    assert_eq!(s.flags & !H1_SEAT_FLAGS, 0, "{name}: Halo 1 seat bits");
                    let f = word(x, SEAT_FLAGS);
                    if f & (SEAT_NOT_PLAYER | SEAT_NOT_NON_PLAYER | SEAT_BOARDING) != 0 {
                        fail(format!("flags {f:#x} keep riders out"));
                    }
                    if f & SEAT_NEEDS_DRIVER != 0 && f & SEAT_DRIVER != 0 {
                        fail(format!("flags {f:#x}: the driver's seat needs a driver"));
                    }
                    for field in ["unknown_98", "unknown_9c", "unknown_a0", "unknown_a4"] {
                        let v = f32::from_bits(word(x, field));
                        if !(v > 0.0) {
                            fail(format!("{field} {v}"));
                        }
                    }
                    let marker = text(x, "string_8");
                    if !marker.is_empty() && instances(groups, &marker) == 0 {
                        fail(format!("rider marker {marker:?} not on the model"));
                    }
                    let entry = text(x, "string_c");
                    if !entry.is_empty() && instances(groups, &entry) == 0 {
                        fail(format!("entry marker {entry:?} not on the model"));
                    }
                    let label = text(x, "label");
                    for (who, h1_graph, g) in &players {
                        let h1_enters = h1_graph
                            .animations
                            .iter()
                            .any(|a| a.name.eq_ignore_ascii_case(&format!("{} enter", s.label)));
                        if h1_enters && !enters(g, &label) {
                            fail(format!("{who}'s graph has no {label}:enter"));
                        }
                    }
                    // Halo 2's sandbox player rides what Halo 1's player
                    // drives and guns; its graph has no Halo 1 passenger
                    // modes it lacks in Halo 2 (the scorpion's sides), a
                    // graph's matter, listed.
                    let h1_player = players.iter().any(|(_, h1_graph, _)| {
                        h1_graph
                            .animations
                            .iter()
                            .any(|a| a.name.eq_ignore_ascii_case(&format!("{} enter", s.label)))
                    });
                    if let Some(g) = &sandbox
                        && h1_player
                        && !enters(g, &label)
                    {
                        if f & (SEAT_DRIVER | SEAT_GUNNER) != 0 {
                            fail(format!("masterchief_mp's graph has no {label}:enter"));
                        } else {
                            println!("{base_name}: {name} seat {k}: masterchief_mp has no {label}:enter");
                        }
                    }
                    println!(
                        "{base_name}: {name} seat {k} {label}: flags {f:#x}, marker {marker:?} entry {entry:?}, radius {}",
                        f32::from_bits(word(x, "unknown_98"))
                    );
                }
            }
        }
    }
    for f in &failures {
        println!("FAIL {f}");
    }
    println!("{seats_checked} seats, {} failures", failures.len());
    assert!(seats_checked > 100, "{seats_checked} seats");
    assert!(failures.is_empty(), "{} seats fail the boarding rule", failures.len());
}
