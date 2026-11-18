//! Vehicles built from the `vehi` definition (`h1objects::rebuild::vehicle`)
//! against the discs. Reads `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test unit_rebuild_disc -- --ignored --nocapture
//! ```
//!
//! - `turret_role_is_the_retail_turrets_majority`: the role table
//!   (`TURRET_ROLE`, `TURRET_SEAT_ROLE`, `TURRET_PHYSICS_ROLE`) is exactly
//!   what Halo 2's turret-type vehicles hold, recomputed from every distinct
//!   retail `vehi` on the disc.
//! - `a10_vehicles_without_templates_are_rebuilt`: every Halo 1 vehicle on
//!   `$H1_LEVELS` (default a10) that no template row names converts, its
//!   chain is in the tree, nothing is pinned, and the fields the engine
//!   reads at unit creation and seat entry are sane.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree, template_candidates};
use h1objects::rebuild::{
    ROLE_VEHICLE_TYPE, TURRET_PHYSICS_ROLE, TURRET_ROLE, TURRET_SEAT_ROLE, VEHI_PHYSICS, VEHI_SEATS,
};
use h1objects::{Converted, h1};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, StructDef, Target, Val};
use tags::repo::extract::SharedFiles;
use tags::repo::{Body, MapSource};
use tags::schema::Schema;

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_entries() -> (String, Vec<(String, u64, u64)>) {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let mut out = Vec::new();
    for e in iso.entries().unwrap() {
        let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
        if let Some(stem) = file.strip_suffix(".map")
            && !e.is_dir
        {
            let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
            out.push((stem.to_owned(), base, u64::from(e.size)));
        }
    }
    // Shared caches first: their tags are the ones levels repeat.
    out.sort_by_key(|(s, ..)| {
        (
            !matches!(s.as_str(), "mainmenu" | "shared" | "single_player_shared"),
            s.clone(),
        )
    });
    (path, out)
}

fn open(path: &str, base: u64, size: u64) -> Map<BufReader<File>> {
    Map::from_region(BufReader::new(File::open(path).unwrap()), base, size).unwrap()
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

fn h1_level(name: &str) -> H1Map {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn def(g: &[u8; 4]) -> &'static StructDef {
    Defs::builtin().group(FourCC(*g)).unwrap()
}

/// Counts of each 4-byte unknown span's value, by path: `prefix` + field
/// name, through embedded structs (not blocks).
fn tally(
    d: &StructDef,
    r: &Record,
    prefix: &str,
    out: &mut BTreeMap<String, BTreeMap<u32, usize>>,
) {
    for (f, (_, v)) in d.fields.iter().zip(&r.fields) {
        let p = format!("{prefix}{}", f.name);
        match (&f.kind, v) {
            (defs::Kind::Struct(s), Val::Struct(x)) => tally(s, x, &format!("{p}."), out),
            (defs::Kind::Unknown { .. }, Val::Bytes(b)) if b.len() == 4 => {
                let w = u32::from_le_bytes(b[..4].try_into().unwrap());
                *out.entry(p).or_default().entry(w).or_default() += 1;
            }
            _ => {}
        }
    }
}

/// The role rule: per path, the value at least half the instances hold,
/// when that value is not zero.
fn majority(t: &BTreeMap<String, BTreeMap<u32, usize>>) -> Vec<(String, u32)> {
    let mut out = Vec::new();
    for (p, vals) in t {
        let n: usize = vals.values().sum();
        let (w, c) = vals.iter().max_by_key(|(_, c)| **c).unwrap();
        if *w != 0 && 2 * c >= n {
            out.push((p.clone(), *w));
        }
    }
    out
}

fn table(t: &[(&str, u32)]) -> Vec<(String, u32)> {
    let mut v: Vec<(String, u32)> = t.iter().map(|(p, w)| (p.to_string(), *w)).collect();
    v.sort();
    v
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn turret_role_is_the_retail_turrets_majority() {
    let schema = Schema::builtin();
    let (path, maps) = h2_entries();
    let mut sh = SharedFiles::default();
    for (stem, base, size) in &maps {
        let file = match stem.as_str() {
            "mainmenu" => map::ResourceFile::MainMenu,
            "shared" => map::ResourceFile::Shared,
            "single_player_shared" => map::ResourceFile::SinglePlayerShared,
            _ => continue,
        };
        sh.set(schema, file, open(&path, *base, *size)).unwrap();
    }
    let d = def(b"vehi");
    let (seat_def, phys_def) = (d.child(VEHI_SEATS).unwrap(), d.child(VEHI_PHYSICS).unwrap());
    let mut seen = HashSet::new();
    let (mut root, mut seats, mut phys) = Default::default();
    let mut turrets = 0;
    for (stem, base, size) in &maps {
        let mut src = MapSource::new(schema, open(&path, *base, *size), &mut sh).unwrap();
        let ids: Vec<TagId> = src
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"vehi"))
            .map(|t| t.id)
            .collect();
        for id in ids {
            let Ok(tag) = src.extract(id).unwrap() else {
                panic!("{stem}: vehi {id:?} refused");
            };
            if matches!(tag.body, Body::Alias { .. })
                || !seen.insert(tags::repo::sha256(&defs::canonical(&tag)))
            {
                continue;
            }
            let r = defs::decode(d, &tag).unwrap();
            if r.at("vehicle_type") != Some(&Val::Enum(ROLE_VEHICLE_TYPE)) {
                continue;
            }
            turrets += 1;
            tally(d, &r, "", &mut root);
            for (block, sub, out) in [
                (VEHI_SEATS, seat_def, &mut seats),
                (VEHI_PHYSICS, phys_def, &mut phys),
            ] {
                if let Some(Val::Block(e)) = r.at(block) {
                    for x in e {
                        tally(sub, x, "", out);
                    }
                }
            }
        }
    }
    println!("{turrets} distinct turret-type vehicles");
    assert!(turrets >= 20, "the role rests on Halo 2's turrets");
    assert_eq!(majority(&root), table(TURRET_ROLE), "vehi root");
    assert_eq!(majority(&seats), table(TURRET_SEAT_ROLE), "seats");
    assert_eq!(majority(&phys), table(TURRET_PHYSICS_ROLE), "physics");
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

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn a10_vehicles_without_templates_are_rebuilt() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let levels = std::env::var("H1_LEVELS").unwrap_or("a10".into());
    let mut built = Vec::new();
    for level in levels.split(',') {
        let h1 = h1_level(level);
        for t in h1.tags() {
            let g = t.group.0.to_be_bytes();
            if &g != b"vehi" || !template_candidates(&g, &t.name).is_empty() {
                continue;
            }
            let u = h1::Unit::read(&h1, t).unwrap();
            let mut c = Converter::new(&h1, &base);
            let id = c.object(t).unwrap_or_else(|e| panic!("{}: {e}", t.name));
            let vehi = local(&c.tree, id).expect("vehi in the tree");
            assert_eq!(vehi.group, FourCC(*b"vehi"));
            let d = def(b"vehi");
            let r = defs::from_built(d, &vehi.root).expect("vehi reads");
            // Written again, it reads the same: the definition holds every
            // byte the converter wrote.
            let again = defs::to_built(d, &r, &mut |_| None).unwrap();
            assert_eq!(defs::from_built(d, &again).unwrap(), r, "{}", t.name);
            let mut pinned = Vec::new();
            pinned_blocks(d, &r, "vehi", &mut pinned);
            assert!(pinned.is_empty(), "{}: pinned {pinned:?}", t.name);

            // What unit creation reads: the object type, the model chain,
            // the bounds.
            assert_eq!(r.at("unit.obje.object_type"), Some(&Val::Enum(1)));
            assert_eq!(r.at("vehicle_type"), Some(&Val::Enum(ROLE_VEHICLE_TYPE)));
            let radius = match r.at("unit.obje.bounding_radius") {
                Some(Val::Float(f)) => *f,
                _ => panic!(),
            };
            assert!(radius > 0.0 && radius.is_finite(), "{}: radius", t.name);
            let hlmt_id = reference(&r, "unit.obje.model").expect("a model");
            let hlmt = local(&c.tree, hlmt_id).expect("hlmt in the tree");
            assert_eq!(hlmt.group, FourCC(*b"hlmt"));
            let h = defs::from_built(def(b"hlmt"), &hlmt.root).expect("hlmt reads");
            let mut pinned = Vec::new();
            pinned_blocks(def(b"hlmt"), &h, "hlmt", &mut pinned);
            assert!(pinned.is_empty(), "{}: hlmt pinned {pinned:?}", t.name);
            let mode = reference(&h, "render_model").and_then(|m| local(&c.tree, m));
            assert_eq!(mode.map(|m| m.group), Some(FourCC(*b"mode")), "{}", t.name);
            for (path, group, h1_has) in [
                ("collision_model", b"coll", u.object.collision.is_some()),
                // vehicle_new refuses a model with none: the stand-in.
                ("physics_model", b"phmo", true),
            ] {
                let got = reference(&h, path).and_then(|x| local(&c.tree, x));
                assert_eq!(got.is_some(), h1_has, "{}: {path}", t.name);
                if let Some(x) = got {
                    assert_eq!(x.group, FourCC(*group));
                }
            }
            let phmo = reference(&h, "physics_model")
                .and_then(|x| local(&c.tree, x))
                .unwrap();
            let pd = def(b"phmo");
            let p = defs::from_built(pd, &phmo.root).expect("phmo reads");
            let again = defs::to_built(pd, &p, &mut |_| None).unwrap();
            assert_eq!(defs::from_built(pd, &again).unwrap(), p, "{}: phmo", t.name);
            let mut pinned = Vec::new();
            pinned_blocks(pd, &p, "phmo", &mut pinned);
            assert!(pinned.is_empty(), "{}: phmo pinned {pinned:?}", t.name);
            if u.object.animation.is_some() {
                let a = match h.at("animation") {
                    Some(Val::Datum(Target::Id(id))) => local(&c.tree, *id),
                    _ => None,
                };
                assert_eq!(a.map(|x| x.group), Some(FourCC(*b"jmad")), "{}", t.name);
            }
            let nodes = match h.at("nodes") {
                Some(Val::Block(n)) => n.len(),
                _ => 0,
            };
            assert!(nodes > 0, "{}: the hlmt has the model's nodes", t.name);

            // What seat entry reads: one seat per Halo 1 seat, labelled as
            // Halo 1's scripts name it, with the rider's marker.
            let Some(Val::Block(seats)) = r.at(VEHI_SEATS) else {
                panic!()
            };
            assert_eq!(seats.len(), u.seats.len(), "{}: seats", t.name);
            for (k, s) in u.seats.iter().enumerate() {
                let p = format!("{VEHI_SEATS}[{k}]");
                assert_eq!(text(&r, &format!("{p}.label")), s.label);
                assert_eq!(text(&r, &format!("{p}.string_8")), s.marker);
                assert!(!s.label.is_empty(), "{}: seat {k} has a label", t.name);
            }
            // Predicted resources (every retail vehicle has some): the
            // render model first, then bitmaps, each a tag in the tree.
            let Some(Val::Block(pr)) = r.at("unit.obje.predicted_resources") else {
                panic!()
            };
            assert!(!pr.is_empty(), "{}: predicted resources", t.name);
            for (k, x) in pr.iter().enumerate() {
                let Some(Val::Datum(Target::Id(id))) = x.get("tag") else {
                    panic!("{}: predicted {k}", t.name)
                };
                let g = local(&c.tree, *id).map(|c| c.group);
                let want = if k == 0 { *b"mode" } else { *b"bitm" };
                assert_eq!(g, Some(FourCC(want)), "{}: predicted {k}", t.name);
            }
            let Some(Val::Block(phys)) = r.at(VEHI_PHYSICS) else {
                panic!()
            };
            assert_eq!(phys.len(), 1, "every retail vehicle has one");
            // References the schema could not see read as null, not tag 0.
            for p in ["snd_ref_284", "foot_ref_294"] {
                assert!(
                    matches!(
                        r.at(p),
                        Some(Val::Ref {
                            target: Target::Null,
                            ..
                        })
                    ),
                    "{p}"
                );
            }
            built.push(format!(
                "{level} {}: {} seats {:?}, {nodes} nodes",
                t.name,
                u.seats.len(),
                u.seats.iter().map(|s| s.label.as_str()).collect::<Vec<_>>()
            ));
        }
    }
    for b in &built {
        println!("{b}");
    }
    assert!(
        built.iter().any(|b| b.contains("cryotube")),
        "the cryotube is among them"
    );
}
