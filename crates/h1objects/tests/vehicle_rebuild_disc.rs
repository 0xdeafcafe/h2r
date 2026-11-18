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

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree, template_candidates};
use h1objects::rebuild::{
    H1_VEHICLE_FLAGS, H1_VEHICLE_TYPE, VEHI_FRICTION, VEHI_GEARS, VEHI_HOVER, VEHI_PHYSICS, VEHI_SEATS,
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
                    assert_eq!(
                        text(&r, &format!("{p}.string_8")).to_ascii_lowercase(),
                        s.marker.to_ascii_lowercase(),
                        "{name}: seat {k} marker"
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
