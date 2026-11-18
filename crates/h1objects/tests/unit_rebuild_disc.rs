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
//! - `biped_words_match_halo_2s_counterparts`: the camera field of view and
//!   `BIPED_WORDS` agree with Halo 2's matching biped on five pairs.
//! - `biped_shapes_follow_the_collision_size`: every retail biped of the
//!   base with character shapes holds the pills and spheres `biped_shapes`
//!   builds from its own collision size, and every rigid body of their
//!   physics models `BIPED_BODY`.
//! - `bipeds_rebuilt_against_the_template_path`: every Halo 1 biped on the
//!   ten levels, rebuilt (the default) and on its template
//!   (`rebuild_bipeds` off): the rebuilt record round-trips with nothing
//!   pinned, carries Halo 1's header, camera, words and collision size with
//!   shapes sized to it, the same model chain as the template path, its
//!   role's words, and every reference field of its counterpart (`bipd` and
//!   `hlmt`) as the counterpart holds it; every reference resolves to a tag
//!   of its group; no reference the template path held is lost where the
//!   counterpart has one; a physics model built from Halo 1's collision
//!   wherever the template path had one (none for a biped with no
//!   collision); the player's HUD is health-meter copies.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree, template_candidates};
use h1objects::rebuild::{
    BIPD_HUD, BIPD_PILLS, BIPD_SIZE, BIPED_BODY, BIPED_OWN, BIPED_TYPE, BIPED_WORDS, BipedRole,
    MODEL_OWN, ROLE_VEHICLE_TYPE, TURRET_PHYSICS_ROLE, TURRET_ROLE, TURRET_SEAT_ROLE, VEHI_PHYSICS,
    VEHI_SEATS, biped_shapes, reference_fields, usable_size,
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
                // vehicle_new refuses a model with none: built from the coll, or the stand-in.
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

/// Five Halo 1 bipeds and their Halo 2 counterparts on 05a.
const PAIRS: &[(&str, &str, &str)] = &[
    (
        "a10",
        "characters\\cyborg\\cyborg",
        "objects\\characters\\masterchief\\masterchief",
    ),
    (
        "a10",
        "characters\\marine_armored\\marine_armored",
        "objects\\characters\\marine\\marine",
    ),
    (
        "b30",
        "characters\\elite\\elite",
        "objects\\characters\\elite\\elite",
    ),
    (
        "b30",
        "characters\\grunt\\grunt",
        "objects\\characters\\grunt\\grunt",
    ),
    (
        "b30",
        "characters\\jackal\\jackal",
        "objects\\characters\\jackal\\jackal",
    ),
];

fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn biped_words_match_halo_2s_counterparts() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let d = def(b"bipd");
    let mut levels = std::collections::HashMap::new();
    for (level, h1_name, h2_name) in PAIRS {
        let h1 = levels.entry(*level).or_insert_with(|| h1_level(level));
        let t = h1
            .tags()
            .iter()
            .find(|t| t.name == *h1_name && t.group.0.to_be_bytes() == *b"bipd")
            .unwrap_or_else(|| panic!("{h1_name}"));
        let u = h1::Unit::read(h1, t).unwrap();
        let id = base
            .tags
            .iter()
            .find(|x| x.2 == *h2_name && x.1 == FourCC(*b"bipd"))
            .unwrap_or_else(|| panic!("{h2_name}"))
            .0;
        let b = base.root(id).unwrap();
        let close = |x: f32, y: f32| (x - y).abs() <= 0.01 * x.abs().max(y.abs());
        let at = |path: &str| {
            let (_, off) = h2_offset(d, path, 0).unwrap_or_else(|| panic!("{path}"));
            f32_at(&b, off as usize)
        };
        let fov = at("unit.camera_field_of_view");
        assert!(
            close(u.camera_field_of_view, fov),
            "{h1_name}: field of view {} vs {fov}",
            u.camera_field_of_view
        );
        for (path, h1_at) in BIPED_WORDS {
            let w = u.biped_words.iter().find(|x| x.0 == *h1_at).unwrap().1;
            let (x, y) = (f32::from_bits(w), at(path));
            assert!(
                x.is_finite() && x != 0.0 && close(x, y),
                "{h1_name}: {path} {x} vs {y}"
            );
        }
        println!(
            "{h1_name}: field of view {fov}, {} words agree",
            BIPED_WORDS.len()
        );
    }
}

/// A field's offset in `d`, through embedded structs.
fn h2_offset<'d>(d: &'d StructDef, path: &str, at: u32) -> Option<(&'d defs::FieldDef, u32)> {
    let (head, rest) = path
        .split_once('.')
        .map_or((path, None), |(h, r)| (h, Some(r)));
    let f = d.fields.iter().find(|f| f.name == head)?;
    match (rest, &f.kind) {
        (None, _) => Some((f, at + f.offset)),
        (Some(r), defs::Kind::Struct(s)) => h2_offset(s, r, at + f.offset),
        _ => None,
    }
}

/// The record's 4-byte unknown spans, by path, through embedded structs.
fn words(d: &StructDef, r: &Record, prefix: &str, out: &mut BTreeMap<String, u32>) {
    for (f, (_, v)) in d.fields.iter().zip(&r.fields) {
        let p = format!("{prefix}{}", f.name);
        match (&f.kind, v) {
            (defs::Kind::Struct(s), Val::Struct(x)) => words(s, x, &format!("{p}."), out),
            (defs::Kind::Unknown { .. }, Val::Bytes(b)) if b.len() == 4 => {
                out.insert(p, u32::from_le_bytes(b[..4].try_into().unwrap()));
            }
            _ => {}
        }
    }
}

fn float(r: &Record, path: &str) -> f32 {
    match r.at(path) {
        Some(Val::Float(f)) => *f,
        x => panic!("{path}: {x:?}"),
    }
}

/// The model chain's groups: hlmt, then its render model, collision model
/// and animation, as the tree holds them.
fn chain(tree: &Tree, r: &Record) -> Vec<Option<FourCC>> {
    let Some(h) = reference(r, "unit.obje.model").and_then(|x| local(tree, x)) else {
        return vec![None];
    };
    let hr = defs::from_built(def(b"hlmt"), &h.root).expect("hlmt reads");
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

/// Every reference in `r` (a record of `d`) at any depth: (path, group
/// word, datum).
fn refs_in(d: &StructDef, r: &Record, path: &str, out: &mut Vec<(String, u32, TagId)>) {
    for (f, (_, v)) in d.fields.iter().zip(&r.fields) {
        let p = format!("{path}{}", f.name);
        match (&f.kind, v) {
            (
                defs::Kind::TagRef { .. },
                Val::Ref {
                    group,
                    target: Target::Id(id),
                },
            ) => out.push((p, *group, *id)),
            (defs::Kind::Struct(s), Val::Struct(x)) => refs_in(s, x, &format!("{p}."), out),
            (defs::Kind::Block(s), Val::Block(e)) => {
                for (i, x) in e.iter().enumerate() {
                    refs_in(s, x, &format!("{p}[{i}]."), out);
                }
            }
            _ => {}
        }
    }
}

/// `v` with every reference's group word cleared: a builder tree reads a
/// reference fixup back with group 0, the map's bytes with the group.
fn norm(v: Option<&Val>) -> Option<Val> {
    fn go(v: &Val) -> Val {
        match v {
            Val::Ref { target, .. } => Val::Ref {
                group: 0,
                target: target.clone(),
            },
            Val::Block(e) => Val::Block(e.iter().map(rec).collect()),
            Val::Struct(r) => Val::Struct(rec(r)),
            x => x.clone(),
        }
    }
    fn rec(r: &Record) -> Record {
        let mut out = r.clone();
        for f in &mut out.fields {
            f.1 = go(&f.1);
        }
        out
    }
    v.map(go)
}

/// Where a reference resolves: a tree tag (its group) or a base tag.
fn resolves(tree: &Tree, base: &Base, id: TagId) -> Option<(FourCC, bool)> {
    match local(tree, id) {
        Some(c) => Some((c.group, true)),
        None if id.0 & 0xffff_0000 == h1objects::convert::local(0).0 => None,
        None => base.name(id).map(|n| (n.0, false)),
    }
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn biped_shapes_follow_the_collision_size() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let d = def(b"bipd");
    let ph = def(b"phmo");
    let hd = def(b"hlmt");
    let (mut shaped, mut bodies) = (0, 0);
    for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"bipd")) {
        let r = base.record(d, t.0).unwrap();
        let f = |p: &str| match r.at(p) {
            Some(Val::Bytes(b)) => f32::from_le_bytes(b[..4].try_into().unwrap()),
            x => panic!("{p}: {x:?}"),
        };
        let [s, c, rad] = BIPD_SIZE.map(f);
        let Some(Val::Block(pills)) = r.at(BIPD_PILLS) else {
            panic!()
        };
        if pills.is_empty() {
            println!("{}: no character shapes", t.2);
        } else {
            let want = biped_shapes(&h1::BipedSize {
                standing_height: s,
                crouching_height: c,
                radius: rad,
            })
            .unwrap();
            // Every word the rule writes, against retail's; the runtime
            // words (the post-process writes them) aside.
            let runtime = |path: &str| {
                [
                    "unknown_20",
                    "unknown_24",
                    "unknown_28",
                    "unknown_30",
                    "unknown_34",
                    "unknown_38",
                    "unknown_3c",
                ]
                .contains(&path)
            };
            for (path, v) in &want {
                match v {
                    Val::Block(want_e) => {
                        let Some(Val::Block(got_e)) = r.at(path) else {
                            panic!()
                        };
                        assert_eq!(got_e.len(), want_e.len(), "{}: {path}", t.2);
                        let ed = d.child(path).unwrap();
                        for (i, (g, w)) in got_e.iter().zip(want_e).enumerate() {
                            let (mut gw, mut ww) = (BTreeMap::new(), BTreeMap::new());
                            words(ed, g, "", &mut gw);
                            words(ed, w, "", &mut ww);
                            for (p, x) in &ww {
                                // A pill's end points are not runtime words.
                                if runtime(p)
                                    && !(path == BIPD_PILLS
                                        && (p == "unknown_38" || p == "unknown_3c"))
                                {
                                    continue;
                                }
                                let y = gw[p];
                                let close = (f32::from_bits(*x) - f32::from_bits(y)).abs() <= 1e-4;
                                assert!(
                                    *x == y || close,
                                    "{}: {path}[{i}].{p} {x:08x} vs {y:08x}",
                                    t.2
                                );
                            }
                        }
                    }
                    _ => assert_eq!(r.at(path), Some(v), "{}: {path}", t.2),
                }
            }
            shaped += 1;
        }
        // Rigid body words of the biped's physics model.
        let Some(h) = reference(&r, "unit.obje.model") else {
            continue;
        };
        let hr = base.record(hd, h).unwrap();
        let Some(p) = reference(&hr, "physics_model") else {
            continue;
        };
        let pr = base.record(ph, p).unwrap();
        let Some(Val::Block(rbs)) = pr.at("rigid_bodies") else {
            panic!()
        };
        for rb in rbs {
            let w = |p: &str| match rb.at(p) {
                Some(Val::Bytes(b)) => u16::from_le_bytes([b[2], b[3]]),
                x => panic!("{x:?}"),
            };
            assert_eq!((w("unknown_18"), w("unknown_1c")), BIPED_BODY, "{}", t.2);
            bodies += 1;
        }
    }
    println!("{shaped} bipeds with character shapes, {bodies} rigid bodies");
    assert!(shaped >= 8 && bodies >= 50);
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bipeds_rebuilt_against_the_template_path() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let d = def(b"bipd");
    let hd = def(b"hlmt");
    let only = std::env::var("H1_LEVELS").ok();
    let mut seen = HashSet::new();
    let mut lines = Vec::new();
    let mut roles = HashSet::new();
    let (mut players, mut no_role, mut no_collision) = (0, 0, 0);
    for level in h1map::disc::CAMPAIGN_LEVELS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|x| x == level))
        {
            continue;
        }
        let h1 = h1_level(level);
        for t in h1.tags() {
            if t.group.0.to_be_bytes() != *b"bipd" || !seen.insert(t.name.clone()) {
                continue;
            }
            let u = h1::Unit::read(&h1, t).unwrap();

            let mut tc = Converter::new(&h1, &base);
            tc.options.rebuild_bipeds = false;
            let tid = match tc.object(t) {
                Ok(id) => id,
                Err(e) => {
                    lines.push(format!("{level} {}: neither path ({e})", t.name));
                    continue;
                }
            };
            let tpl = local(&tc.tree, tid).expect("template bipd in the tree");
            let tr = defs::from_built(d, &tpl.root).expect("template bipd reads");

            let mut c = Converter::new(&h1, &base);
            assert!(c.options.rebuild_bipeds, "rebuilt bipeds are the default");
            let role = c.biped_rebuild_role(&t.name).expect("a role");
            if BipedRole::of_h1(&t.name).is_none() {
                no_role += 1;
            }
            roles.insert(role);
            let rw = c.biped_role(role).unwrap().expect("the base has the role");
            let (cid, cname) = c.biped_counterpart(&t.name).expect("a counterpart");
            let id = c
                .object(t)
                .unwrap_or_else(|e| panic!("{}: rebuilt: {e}", t.name));
            let got = local(&c.tree, id).expect("rebuilt bipd in the tree");
            assert_eq!(got.group, FourCC(*b"bipd"));
            assert!(got.raw.is_empty(), "{}: no template bytes", t.name);
            let rep = c.tree.reports.iter().find(|r| r.source == t.name).unwrap();
            assert!(rep.template.starts_with("none: rebuilt"), "{}", t.name);
            let r = defs::from_built(d, &got.root).expect("rebuilt bipd reads");
            let again = defs::to_built(d, &r, &mut |_| None).unwrap();
            assert_eq!(defs::from_built(d, &again).unwrap(), r, "{}", t.name);
            let mut pinned = Vec::new();
            pinned_blocks(d, &r, "bipd", &mut pinned);
            assert!(pinned.is_empty(), "{}: pinned {pinned:?}", t.name);

            // The header both paths write from Halo 1.
            assert_eq!(r.at("unit.obje.object_type"), Some(&Val::Enum(BIPED_TYPE)));
            assert_eq!(
                float(&r, "unit.obje.bounding_radius"),
                float(&tr, "unit.obje.bounding_radius"),
                "{}: radius as the template path writes it",
                t.name
            );
            let (rc, tchain) = (chain(&c.tree, &r), chain(&tc.tree, &tr));
            assert_eq!(rc, tchain, "{}: model chain", t.name);
            let Some(Val::Block(pr)) = r.at("unit.obje.predicted_resources") else {
                panic!()
            };
            assert!(!pr.is_empty(), "{}: predicted resources", t.name);

            // Halo 1's camera, matched words and collision size; the role's
            // words where Halo 1 has none.
            assert_eq!(
                float(&r, "unit.camera_field_of_view"),
                u.camera_field_of_view
            );
            let mut w = BTreeMap::new();
            words(d, &r, "", &mut w);
            for (path, at) in BIPED_WORDS {
                let h = u.biped_words.iter().find(|x| x.0 == *at).unwrap().1;
                assert_eq!(w.get(*path), Some(&h), "{}: {path}", t.name);
            }
            let size = u.size.expect("a biped's size");
            let sized = usable_size(&size);
            for (path, v) in [size.standing_height, size.crouching_height, size.radius]
                .iter()
                .zip(BIPD_SIZE)
                .map(|(v, p)| (p, v))
                .filter(|_| sized)
            {
                assert_eq!(w.get(path), Some(&v.to_bits()), "{}: {path}", t.name);
            }
            if sized {
                for (path, v) in biped_shapes(&size).unwrap() {
                    assert_eq!(r.at(&path), Some(&v), "{}: {path}", t.name);
                }
            }
            for (path, word) in &rw.root {
                if !BIPED_WORDS.iter().any(|x| x.0 == path)
                    && !(sized && BIPD_SIZE.contains(&path.as_str()))
                {
                    assert_eq!(w.get(path), Some(word), "{}: {path}", t.name);
                }
            }

            // References: the model chain and physics model are Halo 1's
            // (tree tags); the player's HUD is the counterpart's with the
            // health meter (tree tags); every other reference field is the
            // counterpart's as it holds it. Every reference resolves to a
            // tag of its group.
            let cr = base.record(d, cid).unwrap();
            let player = h1objects::health_hud::is_player_biped(&t.name);
            let mut from_counterpart = 0;
            for path in reference_fields(d) {
                if BIPED_OWN.contains(&path.as_str()) || (player && path == BIPD_HUD) {
                    continue;
                }
                assert_eq!(
                    norm(r.at(&path)),
                    norm(cr.at(&path)),
                    "{}: {path} from {cname}",
                    t.name
                );
                from_counterpart += 1;
            }
            let mut all = Vec::new();
            refs_in(d, &r, "", &mut all);
            let hlmt = reference(&r, "unit.obje.model").expect("a model");
            let hr = defs::from_built(hd, &local(&c.tree, hlmt).unwrap().root).unwrap();
            refs_in(hd, &hr, "hlmt.", &mut all);
            for (path, group, id) in &all {
                let (g, _) = resolves(&c.tree, &base, *id)
                    .unwrap_or_else(|| panic!("{}: {path} does not resolve", t.name));
                if *group != 0 {
                    assert_eq!(g.as_u32(), *group, "{}: {path}", t.name);
                }
            }
            let local_refs = all
                .iter()
                .filter(|x| resolves(&c.tree, &base, x.2).unwrap().1)
                .count();
            // The hlmt's damage, dialogue and camouflage come from the
            // counterpart's.
            let chr = base
                .record(hd, reference(&cr, "unit.obje.model").unwrap())
                .unwrap();
            for path in reference_fields(hd) {
                if MODEL_OWN.contains(&path.as_str()) {
                    continue;
                }
                assert_eq!(
                    norm(hr.at(&path)),
                    norm(chr.at(&path)),
                    "{}: hlmt {path}",
                    t.name
                );
            }

            // The template path's references: each it holds non-null, the
            // rebuilt holds too, unless the counterpart holds none there.
            let mut tall = Vec::new();
            refs_in(d, &tr, "", &mut tall);
            let mut call = Vec::new();
            refs_in(d, &cr, "", &mut call);
            let lost: Vec<&String> = tall
                .iter()
                .filter(|(p, ..)| !all.iter().any(|x| &x.0 == p) && call.iter().any(|x| &x.0 == p))
                .map(|x| &x.0)
                .collect();
            assert!(lost.is_empty(), "{}: lost {lost:?}", t.name);

            // A physics model wherever the template path had one, built
            // from Halo 1's collision.
            let phmo = |tree: &Tree, r: &Record| {
                reference(r, "unit.obje.model")
                    .and_then(|x| local(tree, x))
                    .map(|h| defs::from_built(hd, &h.root).unwrap())
                    .and_then(|h| reference(&h, "physics_model"))
            };
            let borrowed = phmo(&tc.tree, &tr).is_some();
            let built = phmo(&c.tree, &r);
            // A biped Halo 1 gives no collision (a cinematic one) gets
            // none, as retail's cinematic characters have none (miranda,
            // regret_holo on 05a); the template path borrowed the
            // template's.
            if borrowed && u.object.collision.is_some() {
                assert!(
                    built.is_some(),
                    "{}: a phmo where the template path had one ({:?})",
                    t.name,
                    rep.physics
                );
            }
            if u.object.collision.is_none() {
                assert!(built.is_none(), "{}", t.name);
                no_collision += 1;
            }
            if let Some(p) = built {
                assert_eq!(local(&c.tree, p).map(|x| x.group), Some(FourCC(*b"phmo")));
                assert!(
                    rep.physics
                        .as_deref()
                        .unwrap_or("")
                        .starts_with("built from Halo 1's coll")
                );
            }

            // The player: the HUD interfaces are health-meter copies.
            if player {
                players += 1;
                let Some(Val::Block(hud)) = r.at(BIPD_HUD) else {
                    panic!()
                };
                assert!(!hud.is_empty(), "{}: a HUD", t.name);
                for e in hud {
                    let Some(Val::Ref {
                        target: Target::Id(id),
                        ..
                    }) = e.get("ref_0")
                    else {
                        panic!("{}: a HUD interface", t.name)
                    };
                    let name = &c.tree.tags[id.0 as usize & 0xffff].0;
                    assert!(name.ends_with(h1objects::health_hud::NHDT_SUFFIX), "{name}");
                    assert_eq!(local(&c.tree, *id).unwrap().group, FourCC(*b"nhdt"));
                }
            }

            let mut tw = BTreeMap::new();
            words(d, &tr, "", &mut tw);
            let differ = w.iter().filter(|(p, x)| tw.get(*p) != Some(x)).count();
            lines.push(format!(
                "{level} {}: size {:?}{}; {role:?}{} over {}, references from {cname} ({from_counterpart} fields; {} references, {local_refs} Halo 1's); template path {} references; {differ} of {} words differ; phmo {} (template {})",
                t.name,
                [size.standing_height, size.crouching_height, size.radius],
                if sized { "" } else { " (unusable: the role's)" },
                if BipedRole::of_h1(&t.name).is_none() { " (counterpart's)" } else { "" },
                rw.members.len(),
                all.len(),
                tall.len(),
                w.len(),
                if built.is_some() { "built" } else { "none" },
                if borrowed { "borrowed" } else { "none" }
            ));
        }
    }
    for l in &lines {
        println!("{l}");
    }
    println!(
        "{} bipeds; {players} player, {no_role} of no role of their own, {no_collision} with no Halo 1 collision",
        seen.len()
    );
    if only.is_none() {
        assert_eq!(roles.len(), 3, "the Chief, humans and the Covenant");
        assert!(players >= 1, "the player biped");
        assert!(no_role >= 1, "bipeds of no role of their own");
    }
}
