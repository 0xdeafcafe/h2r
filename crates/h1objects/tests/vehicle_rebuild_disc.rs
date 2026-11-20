//! Every Halo 1 vehicle built from the `vehi` definition
//! (`h1objects::rebuild::vehicle`, the only vehicle path) against the
//! discs. Reads `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it vehicle_rebuild_disc:: -- --ignored --nocapture
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
//!   (type, flags, camera, bounds, seats and weapons).
//! - `powered_mass_points_become_friction_and_hover_points`: the
//!   warthog's four tires become friction points and the ghost's three
//!   powered points hover points, each naming a marker of the converted
//!   model (on a node in range) and a region of Halo 1's collision; Halo
//!   1's mass point positions are in object space; the warthog has gears.
//! - `control_follows_vehicle_type`: on the three bases
//!   (05a_deltaapproach, coagulation, beavercreek) `vehi +0x1f2`
//!   ([`VEHI_CONTROL`]) is 2 on every type 0 vehicle (the scorpion, the
//!   one tank) and 0 on every vehicle of types 1 to 5; of the turrets
//!   (type 6) only the scorpion's two cannons hold 2 (8 to 11 hold 0 on
//!   each base). Converted tanks select mode 2, checked by
//!   `every_vehicle_rebuilds_from_its_definition`.
//! - `seat_flags_match_halo_2s_counterparts`: on 05a the boarding bit is
//!   set on exactly the retail `_b_` seats; Halo 1's seat bits stay below
//!   bit 11 and equal the counterpart seat's on most of the eight pairs'
//!   seats, the driver bit on every one.
//! - `every_seat_passes_the_engines_boarding_rule`: every seat of every
//!   vehicle (campaign and both sandboxes) passes the engine's enter-seat
//!   search for the converted Halo 1 player and, on the sandboxes, Halo
//!   2's `masterchief_mp`: flags, markers, entry distances and angles, and
//!   the label's enter animation.
//! - `wheels_and_treads_are_solid_above_the_hull`: the retail scorpion's
//!   `phmo` reaches past 1.0 either side (its treads) and stays 0.2 above
//!   the origin; the base's scorpion has wheel shapes and its warthog
//!   none (`member_wheel_shapes`). The converted Blood Gulch scorpion's body reaches as far
//!   as its Halo 1 tread collision does, the warthog's no further than
//!   its hull, and nothing of either is under the hull's lowest point;
//!   the ghost has no wheel nodes.
//! - `every_powered_point_rides_the_root_node`: every friction and hover
//!   marker of every vehicle on the three bases is on node 0, and the
//!   converted Blood Gulch ghost's three hover markers are too, each at
//!   its Halo 1 mass point's position in the object's space.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree, template_candidates};
use h1objects::rebuild::{
    H1_SEAT_FLAGS, H1_VEHICLE_FLAGS, H1_VEHICLE_TYPE, SEAT_BOARDING, SEAT_DRIVER, SEAT_FLAGS, SEAT_GUNNER,
    FRICTION_REGION_INDEX, PHANTOM_MAX_SPHERES, PHANTOM_SPHERE_COUNT, PHANTOM_SPHERES, SEAT_NEEDS_DRIVER, SEAT_NOT_NON_PLAYER, SEAT_NOT_PLAYER, VEHI_CONTROL, VEHI_FRICTION, VEHI_GEARS, VEHI_HOVER, VEHI_PHYSICS, VEHI_SEATS,
};
use h1objects::{Converted, h1};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, StructDef, Target, Val};
use tags::object::direct_drive::GRAVITY_RATIO;

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

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn control_follows_vehicle_type() {
    let d = def(b"vehi");
    let (ty, ctl) = (offset(d, "vehicle_type", 0) as usize, offset(d, VEHI_CONTROL, 0) as usize);
    for base_name in ["05a_deltaapproach", "coagulation", "beavercreek"] {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        let mut types = BTreeMap::<u16, Vec<(String, u16)>>::new();
        for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"vehi")) {
            let r = base.root(t.0).unwrap();
            let (k, c) = (
                u16::from_le_bytes([r[ty], r[ty + 1]]),
                u16::from_le_bytes([r[ctl], r[ctl + 1]]),
            );
            println!("{base_name} type {k} control {c}: {}", t.2);
            types.entry(k).or_default().push((t.2.clone(), c));
        }
        assert!(types.contains_key(&0), "{base_name}: no tank");
        for (k, members) in &types {
            for (name, c) in members {
                let want = match *k {
                    0 => 2,
                    6 if name.starts_with("objects\\vehicles\\scorpion\\") => 2,
                    _ => 0,
                };
                assert_eq!(*c, want, "{base_name} {name} (type {k})");
            }
        }
    }
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
    let mut reclassified = 0;
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
                if let Some(group) = h1objects::scripted_props::replacement_group(&t.name) {
                    assert_eq!(got.group, FourCC(group), "{name}");
                    read_back(got, &name);
                    reclassified += 1;
                    continue;
                }
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
                // The sphere is the model's (`rebuild::model_sphere`;
                // `tests/vehicle_ground_disc.rs`), Halo 1's without one.
                let model = u.object.model.map(|m| {
                    h1models::model_from_map(&h1, h1.tag(m).unwrap(), &Default::default()).unwrap().0
                });
                let (radius, centre) = model
                    .as_ref()
                    .and_then(h1objects::rebuild::model_sphere)
                    .unwrap_or((u.object.bounding_radius, u.object.bounding_offset));
                assert_eq!(r.at("unit.obje.bounding_radius"), Some(&Val::Float(radius)), "{name}");
                assert_eq!(r.at("unit.obje.bounding_offset"), Some(&Val::floats(&centre)), "{name}");
                assert_eq!(
                    r.at("unit.camera_field_of_view"),
                    Some(&Val::Float(u.camera_field_of_view)),
                    "{name}"
                );
                let Some(Val::Block(seats)) = r.at(VEHI_SEATS) else { panic!() };
                assert_eq!(seats.len(), u.seats.len(), "{name}: seats");
                for (k, s) in u.seats.iter().enumerate() {
                    let p = format!("{VEHI_SEATS}[{k}]");
                    // The rider's whole mode, as the biped graphs name
                    // it: a10's lifepod seat is `driver lifepod`, a unit
                    // of the cyborg's graph of its own.
                    assert_eq!(
                        text(&r, &format!("{p}.label")),
                        h1models::names::mode_name(&s.label),
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

                assert_eq!(flags & 0xffff_0000, 0, "{name}");
                assert_eq!(
                    r.at(VEHI_CONTROL),
                    Some(&Val::Bytes(vec![if h1_type == 0 { 2 } else { 0 }, 0]))
                );
                assert_eq!(word(&r, "unknown_25c"), 30f32.to_bits());
                for field in [
                    "unknown_244",
                    "unknown_274",
                    "unknown_278",
                    "unknown_2b0",
                    "unknown_2bc",
                ] {
                    assert_eq!(word(&r, field), 0, "{name}: {field}");
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
                    "{base_name}: {name}: type {h1_type}, {} seats {:?}, {}/{h1_weapons} weapons; {path}",
                    seats.len(),
                    seats
                        .iter()
                        .enumerate()
                        .map(|(k, _)| text(&r, &format!("{VEHI_SEATS}[{k}].label")))
                        .collect::<Vec<_>>(),
                    weapons.len(),
                ));
            }
        }
    }
    for l in &lines {
        println!("{l}");
    }
    println!(
        "{count} vehicles rebuilt, {reclassified} rebuilt as other kinds, {templated} with a template row compared"
    );
    if only.is_none() {
        // 23 distinct campaign vehicles, 3 on each multiplayer base.
        assert!(
            count + reclassified >= 29,
            "every Halo 1 vehicle tag on the discs"
        );
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
            if *block == VEHI_FRICTION {
                // +0x48 is the region's own index among the hlmt's
                // regions, never a word borrowed from the role.
                let Some(Val::Block(hr)) = h.at("regions") else { panic!() };
                let names: Vec<String> = hr.iter().map(|r| text(r, "name")).collect();
                let want = names
                    .iter()
                    .position(|n| h1objects::coll::sid_name(n) == region)
                    .map_or(u32::MAX, |i| i as u32);
                assert_eq!(word(p, FRICTION_REGION_INDEX), want, "{name}: {marker} region index of {names:?}");
            }
            println!("{name}: {block}[{k}] marker {marker}, region {region}, {} instances", ms.len());
        }
        if *block == VEHI_FRICTION {
            // The phantom and friction markers use the same source order.
            let Some(Val::Block(ph)) = r.at(VEHI_PHYSICS) else { panic!() };
            let n = word(&ph[0], &format!("unknown_{PHANTOM_SPHERE_COUNT:x}")).min(PHANTOM_MAX_SPHERES) as usize;
            let spheres: Vec<[f32; 3]> = (0..n)
                .map(|i| {
                    std::array::from_fn(|a| {
                        let at = PHANTOM_SPHERES as usize + i * 0x10 + 4 * a;
                        f32::from_bits(word(&ph[0], &format!("unknown_{at:x}")))
                    })
                })
                .collect();
            assert_eq!(spheres.len(), points.len(), "{name}: a sphere per point");
            for (k, p) in points.iter().enumerate() {
                let Some(Val::String(Str::Text(marker))) = p.get("string_0") else { panic!() };
                let at = powered
                    .iter()
                    .find(|m| h1objects::coll::sid_name(&m.name) == *marker)
                    .unwrap_or_else(|| panic!("{name}: no mass point for {marker}"))
                    .position;
                let d = |s: &[f32; 3]| (0..3).map(|a| (at[a] - s[a]).powi(2)).sum::<f32>();
                let nearest = (0..n).min_by(|x, y| d(&spheres[*x]).total_cmp(&d(&spheres[*y]))).unwrap();
                assert_eq!(at, spheres[k], "{name}: source mass point");
                assert_eq!(nearest, k, "{name}: {marker} at {at:?} rides sphere {k} of {spheres:?}");
            }
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
    // Every retail seat on 05a: a boarding seat (`_b_` in its label, halo1/scripts.md
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
            // The seatless props Halo 2 places as another kind (a10's
            // ring as scenery, b30's falling boxes as crates) have no seats.
            for t in h1.tags().iter().filter(|t| {
                t.group.0.to_be_bytes() == *b"vehi"
                    && h1objects::scripted_props::replacement_group(&t.name).is_none()
            }) {
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

/// Halo 1's powered mass points as the converter counts them
/// (`Converter::powered_points`): one per distinct name; none without a
/// model or physics.
fn h1_powered(h1: &H1Map, u: &h1::Unit) -> usize {
    let (Some(_), Some(p)) = (u.object.model, u.object.physics) else {
        return 0;
    };
    let phys = h1::Physics::read(h1, h1.tag(p).unwrap()).unwrap();
    phys.mass_points
        .iter()
        .filter(|m| m.powered_mass_point >= 0)
        .map(|m| h1objects::coll::sid_name(&m.name))
        .collect::<HashSet<_>>()
        .len()
}

/// Every Ghost lift coefficient comes from its own powered group.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn ghost_keeps_its_source_hover_points() {
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let h1 = h1_level("bloodgulch").unwrap();
    let t = h1
        .tags()
        .iter()
        .find(|t| t.name == "vehicles\\ghost\\ghost_mp" && t.group.0.to_be_bytes() == *b"vehi")
        .unwrap();
    let u = h1::Unit::read(&h1, t).unwrap();
    assert_eq!(h1_powered(&h1, &u), 3, "Halo 1 ghost's powered points");
    let mut c = Converter::new(&h1, &base);
    let id = c.object(t).unwrap();
    let r = read_back(local(&c.tree, id).unwrap(), "ghost_mp");
    let src = h1.tag_data(t).unwrap();
    let speed = f32::from_le_bytes(src[0x2f8..0x2fc].try_into().unwrap()) * 30.0;
    assert_eq!(r.at("unknown_1f4"), Some(&Val::Bytes(speed.to_le_bytes().to_vec())));

    let Some(Val::Block(points)) = r.at(VEHI_HOVER) else {
        panic!()
    };
    assert_eq!(points.len(), 3, "hover points");
    let phys = h1::Physics::read(&h1, h1.tag(u.object.physics.unwrap()).unwrap()).unwrap();
    for point in points {
        let Some(Val::String(Str::Text(marker))) = point.get("string_0") else { panic!("no marker") };
        let source = phys.mass_points.iter().find(|p| h1objects::coll::sid_name(&p.name) == *marker).unwrap();
        let [strength, offset, height, damping, full, cutoff] = phys.antigravity[source.powered_mass_point as usize];
        for (field, value) in [
            ("unknown_8", strength * GRAVITY_RATIO),
            ("unknown_c", offset),
            ("unknown_10", height),
            ("unknown_14", damping * 30.0 / GRAVITY_RATIO),
            ("unknown_18", full),
            ("unknown_1c", cutoff),
            ("unknown_20", source.radius),
        ] {
            assert_eq!(point.at(field), Some(&Val::Bytes(value.to_le_bytes().to_vec())), "{marker} {field}");
        }
    }
}

/// Lays a built tag out at `base` (blocks after their parents, aligned) so
/// the decoder can read it.
fn lay_out(b: &map::build::Block, base: u32) -> Vec<u8> {
    use map::build::{Block, FixupKind};
    fn place(b: &Block, out: &mut Vec<u8>, base: u32) -> u32 {
        let align = b.align.max(4) as usize;
        while out.len() % align != 0 {
            out.push(0);
        }
        let at = out.len();
        out.extend_from_slice(&b.bytes);
        for f in &b.fixups {
            let o = at + f.offset as usize;
            let (n, addr) = match &f.kind {
                FixupKind::Block { count, child } => {
                    if *count == 0 && child.bytes.is_empty() {
                        (0, 0)
                    } else {
                        (*count, place(child, out, base))
                    }
                }
                FixupKind::Data(child) => (child.bytes.len() as u32, place(child, out, base)),
                _ => continue,
            };
            out[o..o + 4].copy_from_slice(&n.to_le_bytes());
            out[o + 4..o + 8].copy_from_slice(&addr.to_le_bytes());
        }
        base + at as u32
    }
    let mut out = Vec::new();
    place(b, &mut out, base);
    out
}

/// (widest |y|, lowest z) of points.
fn reach(points: impl Iterator<Item = [f32; 3]>) -> (f32, f32) {
    points.fold((0.0f32, f32::INFINITY), |(y, z), p| {
        (y.max(p[1].abs()), z.min(p[2]))
    })
}

/// The scorpion's treads are in its body, above its hull's lowest point,
/// as retail's are; the warthog's tires stay out, as retail's do; the
/// ghost has no wheel nodes. Which by the base: the retail scorpion's
/// physics model has tread shapes, the warthog's no wheel shape
/// ([`h1objects::rebuild::member_wheel_shapes`]).
///
/// The user walked through the converted scorpion's treads on Blood Gulch
/// (2026-11-19): its `coll` carried them, its `phmo` (versions 77 to 85)
/// left every wheel and tread node out. V, the retail scorpion's `phmo`
/// (coagulation): its widest point 1.13 either side, its lowest 0.22
/// above the origin (printed).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn wheels_and_treads_are_solid_above_the_hull() {
    use h1objects::physics::Transform;
    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();

    // Retail: the tank's tread shapes reach well past its hull's sides,
    // and nothing of it comes within 0.2 of the ground.
    let id = base
        .tags
        .iter()
        .find(|t| t.1 == FourCC(*b"phmo") && t.2 == "objects\\vehicles\\scorpion\\scorpion")
        .unwrap()
        .0;
    let retail = tags::collision::PhysicsModel::read(&mut base_map, id).unwrap();
    let (y, z) = reach(
        retail
            .polyhedra
            .iter()
            .flat_map(|p| p.vertices.iter().copied()),
    );
    println!("retail scorpion phmo: widest {y}, lowest {z}");
    assert!(y > 1.0 && z > 0.2, "retail scorpion: {y} {z}");
    // The base's vehicles with wheel shapes: the scorpion, not the warthog.
    let d = def(b"vehi");
    let fat = offset(d, VEHI_FRICTION, 0) as usize;
    let fdef = d.child(VEHI_FRICTION).unwrap();
    for (retail_name, want) in [
        ("objects\\vehicles\\scorpion\\scorpion", true),
        ("objects\\vehicles\\warthog\\warthog", false),
    ] {
        let t = base
            .tags
            .iter()
            .find(|t| t.1 == FourCC(*b"vehi") && t.2 == retail_name)
            .unwrap();
        let root = base.root(t.0).unwrap();
        let got = h1objects::rebuild::member_wheel_shapes(&base, &root, d, fat, fdef).unwrap();
        println!("retail {retail_name}: wheel shapes {got:?}");
        assert_eq!(got, Some(want), "{retail_name}");
    }

    let h1 = h1_level("bloodgulch").unwrap();
    // (name, has wheel nodes, holds them)
    for (name, wheels_expected, held) in [
        ("vehicles\\scorpion\\scorpion_mp", true, true),
        ("vehicles\\warthog\\warthog", true, false),
        ("vehicles\\ghost\\ghost_mp", false, false),
    ] {
        let t = h1
            .tags()
            .iter()
            .find(|t| t.name == name && t.group.0.to_be_bytes() == *b"vehi")
            .unwrap();
        let u = h1::Unit::read(&h1, t).unwrap();
        let phys = h1::Physics::read(&h1, h1.tag(u.object.physics.unwrap()).unwrap()).unwrap();
        let model = h1models::model_from_map(
            &h1,
            h1.tag(u.object.model.unwrap()).unwrap(),
            &Default::default(),
        )
        .unwrap()
        .0;
        let world = h1models::mode::world_transforms(
            &model
                .nodes
                .iter()
                .map(|n| (n.parent, n.rotation, n.translation))
                .collect::<Vec<_>>(),
        );
        let wheels = h1objects::convert::wheel_nodes(&phys, &model.nodes);
        assert_eq!(
            !wheels.is_empty(),
            wheels_expected,
            "{name}: wheel nodes {wheels:?}"
        );

        // Halo 1's collision in object space: the wheels' reach and the
        // hull's lowest point.
        let coll = h1::Collision::read(&h1, h1.tag(u.object.collision.unwrap()).unwrap()).unwrap();
        let (mut wheel_pts, mut hull_pts) = (Vec::new(), Vec::new());
        for (i, n) in coll.nodes.iter().enumerate() {
            let Some(src) = n.bsps.first() else { continue };
            let node = model
                .nodes
                .iter()
                .position(|m| m.name == n.name)
                .or((i < model.nodes.len()).then_some(i))
                .unwrap_or(0);
            let (r, t) = world[node];
            let m = Transform { r, t };
            let bsp = h1convert::collision::convert(src).unwrap();
            let pts = bsp.vertices.iter().map(|v| m.point(v.point));
            if wheels.contains(&(node as i16)) {
                wheel_pts.extend(pts);
            } else {
                hull_pts.extend(pts);
            }
        }
        let (hull_y, floor) = reach(hull_pts.into_iter());
        let (wheel_y, _) = reach(wheel_pts.into_iter());

        let mut c = Converter::new(&h1, &base);
        let vid = c.object(t).unwrap();
        let r = read_back(local(&c.tree, vid).unwrap(), name);
        let h = defs::from_built(
            def(b"hlmt"),
            &reference(&r, "unit.obje.model")
                .and_then(|x| local(&c.tree, x))
                .unwrap()
                .root,
        )
        .unwrap();
        let built = reference(&h, "physics_model")
            .and_then(|x| local(&c.tree, x))
            .unwrap();
        const BASE: u32 = 0x8000_0000;
        let p = tags::collision::PhysicsModel::decode(&lay_out(&built.root, BASE), BASE).unwrap();
        assert!(p.validate().is_empty(), "{name}: {:?}", p.validate());
        let (y, z) = reach(p.polyhedra.iter().flat_map(|q| q.vertices.iter().copied()));
        let rep = c.tree.reports.iter().find(|x| x.source == name).unwrap();
        println!(
            "{name}: Halo 1 hull widest {hull_y} lowest {floor}, wheels widest {wheel_y}; built widest {y} lowest {z}; {:?}",
            rep.physics
        );
        // Nothing of the body below the hull's lowest point...
        assert!(
            z >= floor - 0.01,
            "{name}: built lowest {z} under the hull's {floor}"
        );
        // ...and the wheels' sides in it where held, out of it where not.
        if held && wheel_y > hull_y {
            assert!(
                y >= wheel_y - 0.01,
                "{name}: built widest {y}, the wheels reach {wheel_y}"
            );
        }
        if wheels_expected && !held {
            assert!(
                y <= hull_y + 0.01,
                "{name}: built widest {y}, past the hull's {hull_y}"
            );
        }
    }
}

/// Every friction and hover point rides the root node. Retail: on the
/// three bases every marker of every vehicle's friction (`+0x2f0`) and
/// hover (`+0x2e8`) points is on node 0. Converted: each of the Blood
/// Gulch ghost's three hover points has its markers on node 0, at Halo
/// 1's mass point position in the object's space. Through converter
/// version 92 its back point ("hull back seat") rode its own node, the
/// seat (4), and gave no lift at any strength: with a driver the ghost
/// rode nose up on its tail (game run, 2026-11-19; the same point moved
/// live to node 0 at the same place held it level, as the retail ghost).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_powered_point_rides_the_root_node() {
    let d = def(b"vehi");
    let mut points = 0;
    for map in ["coagulation", "beavercreek", "05a_deltaapproach"] {
        let mut base_map = h2_map(map);
        let base = Base::read(&mut base_map).unwrap();
        for x in base.tags.iter().filter(|x| x.1 == FourCC(*b"vehi")) {
            let r = base.root(x.0).unwrap();
            for block in [VEHI_FRICTION, VEHI_HOVER] {
                let e = d.child(block).unwrap();
                let (at, marker) = (
                    offset(d, block, 0) as usize,
                    offset(e, "string_0", 0) as usize,
                );
                let mut names = Vec::new();
                while let Some(p) = base.element(&r, at, e.size as usize, names.len()).unwrap() {
                    names.push(u32_at(&p, marker));
                }
                if names.is_empty() {
                    continue;
                }
                let Some(nodes) = h1objects::rebuild::marker_nodes(&base, &r, d, &names).unwrap()
                else {
                    continue;
                };
                for (k, n) in nodes.iter().enumerate() {
                    assert!(
                        n.iter().all(|n| *n == 0),
                        "{map} {}: {block}[{k}] markers on nodes {n:?}",
                        x.2
                    );
                    points += usize::from(!n.is_empty());
                }
            }
        }
    }
    println!("{points} retail friction and hover points, every marker on node 0");
    assert!(points > 20, "{points}");

    let mut base_map = h2_map("coagulation");
    let base = Base::read(&mut base_map).unwrap();
    let h1 = h1_level("bloodgulch").unwrap();
    let t = h1
        .tags()
        .iter()
        .find(|t| t.name == "vehicles\\ghost\\ghost_mp" && t.group.0.to_be_bytes() == *b"vehi")
        .unwrap();
    let u = h1::Unit::read(&h1, t).unwrap();
    let phys = h1::Physics::read(&h1, h1.tag(u.object.physics.unwrap()).unwrap()).unwrap();
    let model = h1models::model_from_map(
        &h1,
        h1.tag(u.object.model.unwrap()).unwrap(),
        &Default::default(),
    )
    .unwrap()
    .0;
    let world = h1models::mode::world_transforms(
        &model
            .nodes
            .iter()
            .map(|n| (n.parent, n.rotation, n.translation))
            .collect::<Vec<_>>(),
    );
    let mut c = Converter::new(&h1, &base);
    let id = c.object(t).unwrap();
    let r = read_back(local(&c.tree, id).unwrap(), "ghost_mp");
    let src = h1.tag_data(t).unwrap();
    let speed = f32::from_le_bytes(src[0x2f8..0x2fc].try_into().unwrap()) * 30.0;
    assert_eq!(r.at("unknown_1f4"), Some(&Val::Bytes(speed.to_le_bytes().to_vec())));

    let h = defs::from_built(
        def(b"hlmt"),
        &reference(&r, "unit.obje.model")
            .and_then(|x| local(&c.tree, x))
            .unwrap()
            .root,
    )
    .unwrap();
    let mode = reference(&h, "render_model")
        .and_then(|x| local(&c.tree, x))
        .unwrap();
    let m = defs::from_built(def(b"mode"), &mode.root).unwrap();
    let Some(Val::Block(groups)) = m.at("marker_groups") else {
        panic!()
    };
    let Some(Val::Block(hover)) = r.at(VEHI_HOVER) else {
        panic!()
    };
    assert_eq!(hover.len(), 3, "the ghost's hover points");
    for (k, p) in hover.iter().enumerate() {
        let Some(Val::String(Str::Text(marker))) = p.get("string_0") else {
            panic!("point {k}")
        };
        let mass = phys
            .mass_points
            .iter()
            .find(|x| x.powered_mass_point >= 0 && h1objects::coll::sid_name(&x.name) == *marker)
            .unwrap_or_else(|| panic!("no mass point {marker}"));
        let g = groups
            .iter()
            .find(|g| g.get("name") == Some(&Val::String(Str::Text(marker.clone()))))
            .unwrap_or_else(|| panic!("no marker {marker}"));
        let Some(Val::Block(ms)) = g.get("markers") else {
            panic!()
        };
        assert!(!ms.is_empty(), "{marker}");
        for x in ms {
            assert_eq!(x.get("node"), Some(&Val::Int(0)), "{marker}: node");
            let Some(Val::List(tr)) = x.get("translation") else {
                panic!("{marker}")
            };
            let tr: Vec<f32> = tr
                .iter()
                .map(|v| match v {
                    Val::Float(f) => *f,
                    v => panic!("{v:?}"),
                })
                .collect();
            let (rot, pos) = world[0];
            let at: [f32; 3] =
                std::array::from_fn(|a| pos[a] + (0..3).map(|j| rot[a][j] * tr[j]).sum::<f32>());
            let off = (0..3)
                .map(|a| (at[a] - mass.position[a]).powi(2))
                .sum::<f32>()
                .sqrt();
            assert!(
                off < 1e-3,
                "{marker}: at {at:?}, Halo 1 {:?}",
                mass.position
            );
            println!("ghost_mp hover[{k}] {marker}: node 0, at {at:?}");
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn scripted_flyers_keep_their_source_physics_and_lifeboat_loop() {
    let base = Base::read(&mut h2_map("05a_deltaapproach")).unwrap();
    for (level, name, ty, spheres, sounds) in [
        (
            "a10",
            "vehicles\\fighterbomber\\fighterbomber",
            3,
            18,
            false,
        ),
        ("a10", "vehicles\\lifepod\\lifepod", 5, 0, false),
        ("a30", "vehicles\\lifepod_entry\\lifepod_entry", 5, 0, true),
    ] {
        let h1 = h1_level(level).unwrap();
        let tag = h1.find(h1map::Group::new(b"vehi"), name).unwrap();
        let unit = h1::Unit::read(&h1, tag).unwrap();
        let mut c = Converter::new(&h1, &base);
        let id = c.object(tag).unwrap();
        let object = read_back(local(&c.tree, id).unwrap(), name);
        assert_eq!(object.at("vehicle_type"), Some(&Val::Enum(ty)));
        let speed = f32_at(h1.tag_data(tag).unwrap(), 0x2f8) * 30.0;
        assert_eq!(
            object.at("unknown_1f4"),
            Some(&Val::Bytes(speed.to_le_bytes().to_vec()))
        );
        let model = reference(&object, "unit.obje.model").unwrap();
        let hlmt = read_back(local(&c.tree, model).unwrap(), name);
        let physics = reference(&hlmt, "physics_model").unwrap();
        let phmo = read_back(local(&c.tree, physics).unwrap(), name);
        let src_phys = h1.tag(unit.object.physics.unwrap()).unwrap();
        let src = h1::Physics::read(&h1, src_phys).unwrap();
        assert_eq!(phmo.at("mass"), Some(&Val::Float(src.mass)));
        assert_eq!(
            phmo.at("rigid_bodies[0].center_of_mass"),
            Some(&Val::floats(&src.center_of_mass))
        );
        let Some(Val::Block(shapes)) = phmo.at("spheres") else {
            panic!()
        };
        assert_eq!(shapes.len(), spheres);
        let Some(Val::Block(seats)) = object.at(VEHI_SEATS) else {
            panic!()
        };
        assert_eq!(seats.len(), unit.seats.len());
        if sounds {
            let path = "sound\\sfx\\vehicles\\lifeboat";
            let (k, lp) = c
                .tree
                .loops
                .iter()
                .enumerate()
                .find(|(_, l)| l.name == path)
                .unwrap();
            let Some(Val::Block(attachments)) = object.at("unit.obje.attachments") else {
                panic!()
            };
            assert!(
                attachments
                    .iter()
                    .any(|a| reference(a, "type") == Some(h1objects::convert::looping(k)))
            );
            assert!(lp.notes.is_empty(), "{:?}", lp.notes);
            assert!(
                lp.tracks
                    .iter()
                    .flat_map(|t| t.sounds.iter().flatten())
                    .any(|sound| { lp.sounds.iter().any(|(path, _)| path == sound) })
            );
        } else {
            assert!(c.tree.loops.is_empty(), "{name}: only Halo 1's attachments");
        }
    }
}

/// Four of these torque values used to be relocated as shared-map resources.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_jeep_gear_decodes_as_inline_torque_and_shift_parameters() {
    let mut map = h2_map("coagulation");
    let base = Base::read(&mut map).unwrap();
    let root = base
        .root(
            base.find(b"vehi", r"objects\vehicles\warthog\warthog")
                .unwrap(),
        )
        .unwrap();
    let bytes = base.element(&root, 0x268, 0x44, 1).unwrap().unwrap();
    let gear = def(b"vehi").child(VEHI_GEARS).unwrap();
    let record = defs::from_built(gear, &map::build::Block::new(bytes.clone())).unwrap();
    for (field, expected) in [
        ("drive_at_rest", 76500.0),
        ("drive_at_peak", 85500.0),
        ("drive_peak_fraction", 0.26),
        ("drive_exponent", 1.6),
        ("drive_at_limit", 20250.0),
        ("drive_at_double_limit", -36000.0),
        ("coast_at_rest", 0.0),
        ("coast_at_peak", -45000.0),
        ("coast_peak_fraction", 0.3),
        ("coast_exponent", 1.75),
        ("coast_at_limit", -54000.0),
        ("coast_at_double_limit", -135000.0),
        ("upshift_seconds", 0.2),
        ("upshift_fraction", 0.9),
        ("speed_ratio", 0.6),
        ("downshift_seconds", 0.1),
        ("downshift_fraction", 0.5),
    ] {
        let Some(Val::Float(value)) = record.get(field) else {
            panic!("{field}")
        };
        assert!((value - expected).abs() < 1e-5, "{field}: {value}");
    }
    let again = defs::to_built(gear, &record, &mut |_| None).unwrap();
    assert_eq!(again.bytes, bytes);
    assert!(again.fixups.is_empty());
}

/// Numeric drive envelopes independently read from Blood Gulch. Round-trip the
/// complete tag so the schema, writer and converter agree on inline gear data.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn blood_gulch_drive_commands_follow_the_source_on_both_bases() {
    use tags::object::direct_drive::{H1_DIRECT_DRIVE, H1_SECOND_TRACK, Limits};
    let level = h1_level("bloodgulch").unwrap();
    for base_name in ["coagulation", "05a_deltaapproach"] {
        let mut map = h2_map(base_name);
        let base = Base::read(&mut map).unwrap();
        let mut converter = Converter::new(&level, &base);
        for (name, kind, caps, spring, angles) in [
            (
                r"vehicles\warthog\warthog",
                1,
                [7.65, 3.0, 2.475, 9.9],
                0.15,
                [30.0, -30.0, 180.0],
            ),
            (
                r"vehicles\ghost\ghost_mp",
                4,
                [6.75, 6.75, 4.5, 4.5],
                0.15,
                [0.0; 3],
            ),
            (
                r"vehicles\scorpion\scorpion_mp",
                0,
                [4.2, 3.6, 1.8, 5.4],
                0.25,
                [0.0; 3],
            ),
        ] {
            let source = level
                .tags()
                .iter()
                .find(|t| t.name == name && t.group.0.to_be_bytes() == *b"vehi")
                .unwrap();
            let unit = h1::Unit::read(&level, source).unwrap();
            let phys = h1::Physics::read(&level, level.tag(unit.object.physics.unwrap()).unwrap())
                .unwrap();
            let id = converter.object(source).unwrap();
            let r = read_back(local(&converter.tree, id).unwrap(), name);
            let scalar = |field| f32::from_bits(word(&r, field));
            let close =
                |a: f32, b: f32| assert!((a - b).abs() < 0.00001, "{base_name} {name}: {a} vs {b}");
            for (field, expected) in ["unknown_1f4", "unknown_1f8", "unknown_1fc", "unknown_200"]
                .into_iter()
                .zip(caps)
            {
                close(scalar(field), expected);
            }
            for (field, expected) in ["unknown_204", "unknown_208", "unknown_210"]
                .into_iter()
                .zip(angles)
            {
                close(scalar(field), expected);
            }
            for (field, expected) in [
                ("unknown_2b4", spring / GRAVITY_RATIO),
                ("unknown_2b8", 1.5),
                ("unknown_2c0", 0.707107),
                ("unknown_2c4", 0.5),
                ("unknown_2e0", 1.0),
            ] {
                close(scalar(field), expected);
            }
            let Some(Val::Block(gears)) = r.get(VEHI_GEARS) else {
                panic!("missing gears")
            };
            if kind == 4 {
                assert!(gears.is_empty());
                close(scalar("unknown_2d8"), 0.0);
                close(scalar("unknown_2dc"), 0.0);
                let Some(Val::Block(lifts)) = r.get(VEHI_HOVER) else {
                    panic!("missing lift")
                };
                assert_eq!(lifts.len(), 3);
                for point in lifts {
                    // Half-height hover at 0.4 units/s, using H2's solver.
                    let strength = f32::from_bits(word(point, "unknown_8"));
                    let damping = f32::from_bits(word(point, "unknown_14"));
                    close(
                        strength * (4.1712594 * 0.25 - damping * 0.4),
                        1.5 * (0.0035651792 * 900.0 * 0.25 - 0.6 * 0.4),
                    );
                    for (field, value) in [
                        ("unknown_8", 1.5 * GRAVITY_RATIO),
                        ("unknown_10", 0.75),
                        ("unknown_14", 0.6 / GRAVITY_RATIO),
                        ("unknown_18", 0.5),
                        ("unknown_1c", 0.258819),
                    ] {
                        close(f32::from_bits(word(point, field)), value);
                    }
                }
                for (field, expected) in [
                    ("unknown_22c", 6.75),
                    ("unknown_230", 6.75),
                    ("unknown_234", 4.5),
                    ("unknown_238", 4.5),
                ] {
                    close(scalar(field), expected);
                }
                assert_eq!(word(&r, "unknown_218") & 0xffff, 0);
                continue;
            }
            assert_eq!(word(&r, "unknown_260"), H1_DIRECT_DRIVE);
            close(scalar("unknown_264"), 0.0);
            let gear_def = def(b"vehi").child(VEHI_GEARS).unwrap();
            assert_eq!(gears.len(), 2);
            for (gear, ratio) in gears.iter().zip([-1.0, 1.0]) {
                for field in &gear_def.fields {
                    let value = if field.name == "speed_ratio" {
                        ratio
                    } else {
                        0.0
                    };
                    assert_eq!(
                        gear.get(&field.name),
                        Some(&Val::Float(value)),
                        "{name} {}",
                        field.name
                    );
                }
            }
            let limits = Limits {
                forward: scalar("unknown_1f4"),
                reverse: scalar("unknown_1f8"),
                acceleration: scalar("unknown_1fc"),
                braking: scalar("unknown_200"),
            };
            close(
                limits.advance(0.0, 30.0, 1.0 / 30.0),
                if kind == 1 { 0.0825 } else { 0.06 },
            );
            close(
                limits.advance(1.0, 0.0, 1.0 / 30.0),
                if kind == 1 { 0.67 } else { 0.82 },
            );
            close(limits.advance(0.0, -30.0, 10.0), -caps[1]);
            close(limits.advance(0.0, 30.0, 10.0), caps[0]);
            if kind == 0 {
                close(scalar("unknown_22c"), caps[0]);
                close(
                    limits.tread_turn(0.0, 1.0, std::f32::consts::FRAC_PI_2, 1.0 / 30.0),
                    0.12,
                );
            }
            let Some(Val::Block(points)) = r.get(VEHI_FRICTION) else {
                panic!("missing contacts")
            };
            assert_eq!(points.len(), if kind == 1 { 4 } else { 8 });
            for point in points {
                let Some(Val::String(Str::Text(marker))) = point.get("string_0") else {
                    panic!("missing marker")
                };
                let mass = phys
                    .mass_points
                    .iter()
                    .find(|p| h1objects::coll::sid_name(&p.name) == *marker)
                    .unwrap();
                let flags = word(point, "unknown_4");
                if kind == 1 {
                    assert_eq!(flags & 12, if mass.powered_mass_point == 0 { 4 } else { 8 });
                } else {
                    assert_eq!(flags & H1_SECOND_TRACK != 0, mass.powered_mass_point == 1);
                }
            }
        }
    }
}

/// Exercise the driving definition separately from the assets attached to
/// each vehicle, including the cinematics with a null physics reference.
#[test]
#[ignore = "reads H1_ISO and H2_DISC"]
fn retail_vehicle_drive_laws_round_trip() {
    use h1objects::rebuild::{Point, VehicleSpec, vehicle};
    use tags::object::{direct_drive as ground, h1_flight as air};
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let retail = h1map::disc::Disc::open(&mut iso).unwrap();
    let materials = BTreeMap::from(
        ["05a_deltaapproach", "coagulation", "beavercreek"]
            .map(|name| (name, Base::read(&mut h2_map(name)).unwrap().materials)),
    );
    let mut aircraft = HashSet::new();
    let mut warthog = false;
    let mut scorpion = false;
    let (mut campaign, mut multiplayer, mut redirects, mut vehicles) = (0, 0, 0, 0);
    let (mut wheels, mut lifts, mut channels) = (0, 0, 0);
    let mut solid_hulls = 0;
    for entry in &retail.maps {
        let level = h1map::disc::map_stem(entry);
        if level == "ui" {
            continue;
        }
        let base_name = if h1map::disc::CAMPAIGN_LEVELS.contains(&level) {
            campaign += 1;
            "05a_deltaapproach"
        } else {
            multiplayer += 1;
            h1convert::convert::known(level)
                .unwrap_or_else(|| panic!("unknown retail map {level}"))
                .1
        };
        let base_materials = &materials[base_name];
        let ice = base_materials
            .iter()
            .position(|s| s == "hard_terrain_ice")
            .unwrap() as u16;
        let map = H1Map::from_reader(Cursor::new(iso.read(entry).unwrap())).unwrap();
        for t in map
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"vehi"))
        {
            if h1objects::scripted_props::replacement_group(&t.name).is_some() {
                redirects += 1;
                continue;
            }
            vehicles += 1;
            let name = &t.name;
            let source = map.tag_data(t).unwrap();
            let obj = h1::Object::read(&map, source).unwrap();
            let ty = u32_at(source, H1_VEHICLE_TYPE) as u16;
            assert!(
                matches!(ty, 0 | 1 | 3 | 4 | 5 | 6),
                "{level} {name}: type {ty}"
            );
            let tag = obj.physics.map(|id| map.tag(id).unwrap());
            let physics = tag.map(|t| map.tag_data(t).unwrap());
            let p = tag.map(|t| h1::Physics::read(&map, t).unwrap());
            if let Some(phys) = physics {
                assert!(
                    !h1::elements(&map, phys, 0x5c, 0x24).unwrap().is_empty(),
                    "{level} {name}: inertia"
                );
            }
            let solid_hull = obj.collision.is_some_and(|id| {
                let coll = h1::Collision::read(&map, map.tag(id).unwrap()).unwrap();
                coll.nodes
                    .iter()
                    .filter_map(|node| node.bsps.first())
                    .any(|src| {
                        let bsp = h1convert::collision::convert(src)
                            .unwrap_or_else(|e| panic!("{level} {name}: {e}"));
                        !h1objects::physics::solid_cells(&bsp).is_empty()
                    })
            });
            assert!(
                solid_hull
                    || p.as_ref().is_some_and(|p| p
                        .mass_points
                        .iter()
                        .any(|m| m.radius.is_finite() && m.radius > 0.0)),
                "{level} {name}: body"
            );
            solid_hulls += usize::from(solid_hull);
            let points: Vec<_> = p
                .iter()
                .flat_map(|p| {
                    p.mass_points.iter().filter_map(move |m| {
                        let g = usize::try_from(m.powered_mass_point).ok()?;
                        assert!(
                            matches!(p.powered_flags.get(g).copied(), Some(1 | 0x40)),
                            "{level} {name}: powered group {g}"
                        );
                        Some(Point {
                            sphere: [m.position[0], m.position[1], m.position[2], m.radius],
                            marker: h1objects::coll::sid_name(&m.name),
                            region: String::new(),
                            region_index: -1,
                            steered: g == 0,
                            antigravity: (p.powered_flags[g] & 0x40 != 0)
                                .then_some(p.antigravity[g]),
                        })
                    })
                })
                .collect();
            let names: HashSet<_> = points.iter().map(|p| &p.marker).collect();
            assert_eq!(
                names.len(),
                points.len(),
                "{level} {name}: duplicate marker"
            );
            let hover_count = points.iter().filter(|p| p.antigravity.is_some()).count();
            lifts += hover_count;
            wheels += points.len() - hover_count;
            let contacts = points.first().filter(|p| p.antigravity.is_none()).map(|_| {
                h1::GroundContacts::read(&map, physics.unwrap(), base_materials)
                    .unwrap_or_else(|e| panic!("{level} {name} on {base_name}: {e}"))
            });
            let power =
                h1::powered_seats(&map, source).unwrap_or_else(|e| panic!("{level} {name}: {e}"));
            channels += power.len();
            let root = vehicle(&VehicleSpec {
                source,
                object: &obj,
                physics,
                vehicle_type: ty,
                flags: u32_at(source, H1_VEHICLE_FLAGS),
                bounds: None,
                hlmt: None,
                seats: &[],
                predicted: &[],
                camera_field_of_view: 0.0,
                weapons: &[],
                references: &[],
                attachments: &[],
                functions: &[],
                creation_effect: None,
                points: &points,
                contacts: contacts.as_ref(),
                seat_power: &power,
            })
            .unwrap_or_else(|e| panic!("{level} {name}: {e}"));
            let r = defs::from_built(def(b"vehi"), &root).unwrap();
            let Some(Val::Block(shapes)) = r.get(VEHI_PHYSICS) else {
                panic!("{name}: phantom")
            };
            let meta = |at| word(&shapes[0], &format!("unknown_{at:x}"));
            if physics.is_some() || !power.is_empty() {
                assert_eq!(meta(ground::POWER_DATA), ground::POWER_MAGIC, "{name}");
                let flags = meta(ground::POWER_DATA + 4);
                assert_eq!(flags & 255, power.len() as u32);
                assert_eq!(flags & ground::HAS_SOURCE_GRAVITY != 0, physics.is_some());
            }
            if !power.is_empty() {
                assert_eq!(meta(ground::POWER_DATA + 8), power[0][0].to_bits());
                assert_eq!(meta(ground::POWER_DATA + 12), power[0][1].to_bits());
            }
            if let Some(c) = contacts {
                assert_eq!(meta(ground::CONTACT_DATA), ground::CONTACT_MAGIC, "{name}");
                assert_eq!(meta(ground::CONTACT_DATA + 4) & 255, points.len() as u32);
                let start = ground::CONTACT_DATA + ground::CONTACT_HEADER;
                if name == r"vehicles\warthog\warthog" {
                    warthog = true;
                    assert_eq!(
                        (meta(start), meta(start + 8), meta(start + 12)),
                        (5, 0.45f32.to_bits(), 0.65f32.to_bits())
                    );
                    let front = start + 2 * ground::WHEEL_BYTES;
                    assert_eq!(
                        (meta(front + 8), meta(front + 12)),
                        (0.75f32.to_bits(), 0.45f32.to_bits())
                    );
                    let surface = start + 4 * ground::WHEEL_BYTES;
                    assert_eq!(
                        (meta(surface), meta(surface + 4)),
                        (u32::from(ice), 0.125f32.to_bits())
                    );
                    let rate = f32::from_bits(meta(start + 4));
                    assert!((rate * 5000.0 - 306.12245 * 0.2 * 30.0).abs() < 0.01);
                    let wheel = ground::WheelDrag {
                        mode: meta(start),
                        rate: rate * f32::from_bits(meta(surface + 4)),
                        parallel: f32::from_bits(meta(start + 8)),
                        perpendicular: f32::from_bits(meta(start + 12)),
                    };
                    let cap = f32::from_bits(word(&r, "unknown_1f4"));
                    let ice_force = wheel.force(
                        [cap, 1.0, 0.0],
                        [1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0],
                        [0.0, 0.0, 1.0],
                        cap,
                        0.707107,
                        0.5,
                        5000.0,
                    );
                    // At the source speed cap only sideslip produces a force.
                    assert_eq!((ice_force[0], ice_force[2]), (0.0, 0.0));
                    assert!((ice_force[1] + 306.12245 * 0.2 * 30.0 * 0.65 * 0.125).abs() < 0.001);
                } else if name == r"vehicles\scorpion\scorpion" {
                    scorpion = true;
                    assert_eq!(meta(start), 4);
                    assert!(c.surfaces.iter().all(|(mat, _)| *mat != ice));
                }
            }
            if matches!(ty, 3 | 5) {
                assert_eq!(meta(air::DATA), air::MAGIC, "{level} {name}");
                let expected = match (ty, p.as_ref()) {
                    (_, None) => air::PASSIVE,
                    (3, _) => air::DROPSHIP,
                    (_, Some(p)) if p.powered_mass_points != 2 => air::PASSIVE,
                    _ if f32_at(physics.unwrap(), 0) > 0.0 => air::LIFEPOD,
                    _ => air::BANSHEE,
                };
                assert_eq!(meta(air::DATA + 4), expected, "{level} {name}");
                aircraft.insert(expected);
                if expected == air::BANSHEE {
                    assert_eq!(meta(air::DATA + 20), u32_at(source, 0x308));
                    assert_eq!(meta(air::DATA + 24), u32_at(source, 0x314));
                }
            }
        }
    }
    assert_eq!((campaign, multiplayer), (10, 13));
    assert_eq!((vehicles, redirects, solid_hulls), (94, 2, 90));
    assert_eq!((wheels, lifts, channels), (222, 90, 97));
    assert!(warthog && scorpion);
    assert_eq!(
        aircraft,
        HashSet::from([air::PASSIVE, air::BANSHEE, air::LIFEPOD, air::DROPSHIP])
    );
}
