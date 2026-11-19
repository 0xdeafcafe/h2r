//! Every Halo 1 scenery object built from the `scen` definition
//! (`h1objects::rebuild::scenery`, the only scenery path) against the
//! discs. Reads `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test rebuild_disc -- --ignored --nocapture
//! ```
//!
//! - `every_scenery_rebuilds_from_its_definition`: every `scen` of the ten
//!   campaign levels (on 05a_deltaapproach) and the multiplayer maps (on
//!   their bases, coagulation and beavercreek) converts; the `scen`, its
//!   `hlmt` and `mode` read back through the definitions, write the same
//!   again, pin nothing, hold no raw data and reference only converted
//!   tags; the known fields are Halo 1's (type, bounds, the model chain,
//!   the predicted resources), the unknown spans the base's scenery role
//!   words, and Havok sees the scenery through a `phmo` built from Halo
//!   1's collision where that holds solid space (its rigid bodies holding
//!   the base's scenery body words, its `coll` no BSP physics slots), else
//!   through its `coll`'s BSP physics slots.
//! - `scenery_bodies_take_the_bases_scenery_words`: the retail scenery
//!   rigid body words on each base; on Coagulation every scenery with
//!   collision has a `phmo` and all their bodies hold one pattern.
//! - `retail_scenery_has_bsp_physics_and_predicts_its_model`: on the bases,
//!   the retail rule the rebuilt scenery follows: scenery with collision
//!   and no `phmo` is the norm, and scenery predicts its render model and
//!   bitmaps with the words the rebuilt scenery writes.

use std::collections::{BTreeMap, HashSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree, h2};
use h1objects::{Converted, h1};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, StructDef, Target, Val};

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

/// The ten campaign levels on 05a, the multiplayer maps on their bases.
fn sets() -> Vec<(&'static str, Vec<String>)> {
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
    sets
}

fn wanted(level: &str) -> bool {
    std::env::var("H1_LEVELS")
        .ok()
        .is_none_or(|o| o.split(',').any(|x| x == level))
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

/// Every tag a record names (references and datums), with its path.
fn targets(r: &Record, path: &str, out: &mut Vec<(String, TagId)>) {
    for (name, v) in &r.fields {
        let p = format!("{path}.{name}");
        match v {
            Val::Ref {
                target: Target::Id(id),
                ..
            }
            | Val::Datum(Target::Id(id)) => out.push((p, *id)),
            Val::Block(e) => {
                for (i, x) in e.iter().enumerate() {
                    targets(x, &format!("{p}[{i}]"), out);
                }
            }
            Val::Struct(x) => targets(x, &p, out),
            _ => {}
        }
    }
}

/// Reads `c`'s root through its group's definition, checks it writes the
/// same again, pins nothing, holds no raw data (but a model's), and names
/// only tags of the tree, or `stand_in` (the base's model shader, which
/// the shader conversion puts in a material whose Halo 1 shader it cannot
/// read; counted in `stand_ins`).
fn read_back(
    tree: &Tree,
    c: &Converted,
    name: &str,
    stand_in: Option<TagId>,
    stand_ins: &mut usize,
) -> Record {
    let g = c.group.0;
    let d = def(&g);
    // A render model's vertices are raw data; the object tags have none.
    if c.group != FourCC(*b"mode") {
        assert!(c.raw.is_empty(), "{name}: {} raw", c.group);
    }
    let r = defs::from_built(d, &c.root).unwrap_or_else(|e| panic!("{name}: {e}"));
    let again = defs::to_built(d, &r, &mut |_| None).unwrap();
    assert_eq!(
        defs::from_built(d, &again).unwrap(),
        r,
        "{name}: {}",
        c.group
    );
    let mut pinned = Vec::new();
    pinned_blocks(d, &r, &c.group.to_string(), &mut pinned);
    assert!(pinned.is_empty(), "{name}: pinned {pinned:?}");
    let mut named = Vec::new();
    targets(&r, &c.group.to_string(), &mut named);
    for (p, id) in named {
        // The tag's own datum, filled in at install.
        if id == h1models::SELF_ID {
            continue;
        }
        if Some(id) == stand_in && p.contains(".materials[") {
            *stand_ins += 1;
            continue;
        }
        assert!(
            local(tree, id).is_some(),
            "{name}: {p} names {id:?}, not a converted tag"
        );
    }
    r
}

fn word(r: &Record, path: &str) -> u32 {
    match r.at(path) {
        Some(Val::Bytes(b)) if b.len() == 4 => u32::from_le_bytes(b[..4].try_into().unwrap()),
        x => panic!("{path}: {x:?}"),
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_scenery_rebuilds_from_its_definition() {
    let mut count = 0;
    let mut stand_ins = 0;
    let mut tally = BTreeMap::<String, usize>::new();
    for (base_name, levels) in sets() {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        let stand_in = base.model_shader.as_ref().map(|m| m.shader);
        let mut seen = HashSet::new();
        let mut role = None;
        let body = h1objects::rebuild::scenery_body(&base).unwrap();
        let classes = h1objects::rebuild::size_classes(&base).unwrap();
        println!("{base_name}: scenery rigid body words {body:?}");
        for level in levels {
            if !wanted(&level) {
                continue;
            }
            let Some(h1) = h1_level(&level) else { continue };
            for t in h1.tags() {
                if t.group.0.to_be_bytes() != *b"scen" || !seen.insert(t.name.clone()) {
                    continue;
                }
                let name = format!("{level} {}", t.name);
                let obj = h1::Object::read(&h1, h1.tag_data(t).unwrap()).unwrap();
                let mut c = Converter::new(&h1, &base);
                let id = c.object(t).unwrap_or_else(|e| panic!("{name}: {e}"));
                let words = c.scenery_role().unwrap().expect("the base has scenery");
                if role.as_ref() != Some(&words) {
                    println!(
                        "{base_name}: scenery role words from {} scenery ({} with a model): scen {:x?}, hlmt {:x?}",
                        words.members, words.models, words.scen, words.hlmt
                    );
                    role = Some(words.clone());
                }
                count += 1;
                let tree = &c.tree;
                let got = local(tree, id).expect("scen in the tree");
                assert_eq!(got.group, FourCC(*b"scen"), "{name}");
                let r = read_back(tree, got, &name, None, &mut stand_ins);
                let rep = tree
                    .reports
                    .iter()
                    .find(|x| x.source == t.name)
                    .expect("a report");
                assert!(rep.stopgaps.is_empty(), "{name}: {:?}", rep.stopgaps);
                assert!(rep.tuning.is_empty(), "{name}: no template to tune");

                // Halo 1's fields.
                assert_eq!(r.at("object.object_type"), Some(&Val::Enum(6)), "{name}");
                assert_eq!(
                    r.at("object.bounding_radius"),
                    Some(&Val::Float(obj.bounding_radius)),
                    "{name}"
                );
                assert_eq!(
                    r.at("object.bounding_offset"),
                    Some(&Val::floats(&obj.bounding_offset)),
                    "{name}"
                );
                for (p, w) in &words.scen {
                    assert_eq!(word(&r, p), *w, "{name}: {p}");
                }
                for f in ["attachments", "widgets", "functions", "change_colors"] {
                    assert_eq!(
                        r.at(&format!("object.{f}")),
                        Some(&Val::Block(Vec::new())),
                        "{name}: {f}"
                    );
                }

                // The model chain: Halo 1's render model, collision and
                // animation, converted; no physics model.
                let hlmt = reference(&r, "object.model");
                assert_eq!(hlmt.is_some(), obj.model.is_some(), "{name}: model");
                let Some(h) = hlmt else {
                    *tally.entry("no model".into()).or_default() += 1;
                    continue;
                };
                let hc = local(tree, h).unwrap();
                assert_eq!(hc.group, FourCC(*b"hlmt"));
                let hr = read_back(tree, hc, &name, None, &mut stand_ins);
                for (p, w) in &words.hlmt {
                    assert_eq!(word(&hr, p), *w, "{name}: hlmt {p}");
                }
                // A physics model exactly where Halo 1's collision holds
                // solid space and the base's scenery has physics models,
                // its every rigid body holding their words.
                let solid = obj.collision.is_some_and(|cl| {
                    h1::Collision::read(&h1, h1.tag(cl).unwrap())
                        .unwrap()
                        .nodes
                        .iter()
                        .filter_map(|n| n.bsps.first())
                        .any(|b| {
                            !h1objects::physics::solid_cells(
                                &h1convert::collision::convert(b).unwrap(),
                            )
                            .is_empty()
                        })
                });
                let phmo = reference(&hr, "physics_model");
                assert_eq!(phmo.is_some(), solid && body.is_some(), "{name}: phmo");
                if let (Some(p), Some((words, _))) = (phmo, body) {
                    let pc = local(tree, p).expect("a built phmo");
                    assert_eq!(pc.group, FourCC(*b"phmo"));
                    let pr = read_back(tree, pc, &name, None, &mut stand_ins);
                    let Some(Val::Block(rbs)) = pr.at("rigid_bodies") else {
                        panic!("{name}: rigid bodies")
                    };
                    assert!(!rbs.is_empty(), "{name}: rigid bodies");
                    for rb in rbs {
                        let w = word(rb, "unknown_18");
                        assert_eq!(
                            (w as u16, (w >> 16) as u16),
                            (words.flags, words.motion),
                            "{name}: rigid body words"
                        );
                        let Some(Val::Float(radius)) = rb.at("bounding_sphere_radius") else {
                            panic!("{name}: radius")
                        };
                        assert_eq!(
                            Some((word(rb, "unknown_1c") >> 16) as u16),
                            h1objects::rebuild::size_class(&classes, *radius),
                            "{name}: size class of a body of radius {radius}"
                        );
                    }
                    *tally.entry("built phmo".into()).or_default() += 1;
                }
                let mode = reference(&hr, "render_model").expect("a render model");
                let mc = local(tree, mode).unwrap();
                assert_eq!(mc.group, FourCC(*b"mode"));
                let mr = read_back(tree, mc, &name, stand_in, &mut stand_ins);
                let coll = reference(&hr, "collision_model");
                assert_eq!(coll.is_some(), obj.collision.is_some(), "{name}: coll");
                assert_eq!(
                    matches!(hr.at("animation"), Some(Val::Datum(Target::Id(_)))),
                    obj.animation.is_some(),
                    "{name}: animation"
                );
                let Some(Val::Block(regions)) = hr.at("regions") else {
                    panic!("{name}: regions")
                };
                let Some(Val::Block(mode_regions)) = mr.at("regions") else {
                    panic!("{name}: mode regions")
                };
                assert_eq!(regions.len(), mode_regions.len(), "{name}: regions");

                // Havok's view: the phmo, or else the coll's BSP physics
                // slots, one per BSP (retail: nothing with a phmo has them).
                if let Some(cl) = coll {
                    assert_eq!(local(tree, cl).map(|x| x.group), Some(FourCC(*b"coll")));
                    assert_eq!(c.coll_reports.len(), 1, "{name}: one coll");
                    let cr = &c.coll_reports[0].1;
                    assert!(cr.bsps > 0, "{name}: BSPs");
                    if phmo.is_some() {
                        assert_eq!(cr.bsp_physics, 0, "{name}: no slots beside a phmo");
                    } else {
                        assert_eq!(
                            cr.bsp_physics, cr.bsps,
                            "{name}: a BSP physics slot per BSP"
                        );
                        *tally.entry("collision, BSP physics".into()).or_default() += 1;
                    }
                } else {
                    *tally.entry("no collision".into()).or_default() += 1;
                }

                // Predicted resources: the render model, then every bitmap
                // its shaders draw, each a converted tag.
                let Some(Val::Block(pred)) = r.at("object.predicted_resources") else {
                    panic!("{name}: predicted")
                };
                assert!(!pred.is_empty(), "{name}: predicts its model");
                assert_eq!(word(&pred[0], "unknown_0"), 0xffff_0002, "{name}");
                assert_eq!(
                    pred[0].at("tag"),
                    Some(&Val::Datum(Target::Id(mode))),
                    "{name}"
                );
                for p in &pred[1..] {
                    assert_eq!(word(p, "unknown_0"), 0, "{name}");
                    let Some(Val::Datum(Target::Id(b))) = p.at("tag") else {
                        panic!("{name}: predicted bitmap")
                    };
                    assert_eq!(local(tree, *b).map(|x| x.group), Some(FourCC(*b"bitm")));
                }
                *tally.entry("predicted bitmaps".into()).or_default() += pred.len() - 1;
                for a in obj.attachments.iter().flatten() {
                    let g = h1.tag(*a).map_or("?".into(), |x| x.group.to_string());
                    *tally
                        .entry(format!("Halo 1 {g} attachments not carried"))
                        .or_default() += 1;
                }
            }
        }
    }
    println!(
        "{count} scenery objects rebuilt: {tally:?}; {stand_ins} materials on the base's model shader"
    );
    assert!(count > 0, "no scenery");
}

#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_scenery_has_bsp_physics_and_predicts_its_model() {
    let mut statics = 0;
    for base_name in ["05a_deltaapproach", "coagulation", "beavercreek"] {
        let mut m = h2_map(base_name);
        let base = Base::read(&mut m).unwrap();
        let mut rows = BTreeMap::<String, usize>::new();
        let mut kinds = BTreeMap::<(u32, String), usize>::new();
        let mut first = BTreeMap::<String, usize>::new();
        for (id, g, _, _) in base.tags.clone() {
            if g != FourCC(*b"scen") {
                continue;
            }
            let root = base.root(id).unwrap();
            let (coll, phmo) = match base.reference(&root, h2::MODEL) {
                Some(h) => {
                    let hb = base.root(h).unwrap();
                    (
                        base.reference(&hb, h2::HLMT_COLLISION).is_some(),
                        base.reference(&hb, h2::HLMT_PHYSICS_MODEL).is_some(),
                    )
                }
                None => (false, false),
            };
            *rows
                .entry(format!("collision {coll}, phmo {phmo}"))
                .or_default() += 1;
            if let Some(h) = base.reference(&root, h2::MODEL) {
                let hb = base.root(h).unwrap();
                for at in [0x58, 0x84] {
                    let n = u32::from_le_bytes(hb[at..at + 4].try_into().unwrap());
                    *rows.entry(format!("hlmt +{at:#x} count {n}")).or_default() += 1;
                }
            }
            let at = 0xb4;
            let mut i = 0;
            while let Some(e) = base.element(&root, at, 8, i).unwrap() {
                let w = u32::from_le_bytes(e[0..4].try_into().unwrap());
                let t = TagId(u32::from_le_bytes(e[4..8].try_into().unwrap()));
                let g = base.name(t).map_or("?".into(), |n| n.0.to_string());
                if i == 0 {
                    *first.entry(g.clone()).or_default() += 1;
                }
                *kinds.entry((w, g)).or_default() += 1;
                i += 1;
            }
        }
        println!("{base_name}: {rows:?}");
        println!("  predicted (word, group): {kinds:x?}");
        println!("  first predicted: {first:?}");
        // Both forms are retail: with no phmo, Havok sees the scenery
        // through its coll's BSP physics slots (collision_disc.rs,
        // `retail_static_scenery_carries_bsp_physics`).
        statics += rows.get("collision true, phmo false").copied().unwrap_or(0);
        let modes = kinds
            .get(&(0xffff_0002, "mode".into()))
            .copied()
            .unwrap_or(0);
        let bitmaps = kinds.get(&(0, "bitm".into())).copied().unwrap_or(0);
        assert!(modes > 0 && bitmaps > 0, "{base_name}: {kinds:?}");
        assert_eq!(
            first.keys().collect::<Vec<_>>(),
            ["mode"],
            "{base_name}: the model first"
        );
    }
    assert!(statics > 0, "retail scenery with collision and no phmo");
}

/// The words built scenery bodies take (`rebuild::scenery_body`): on each
/// base, every rigid body of every scenery `phmo` and how many hold each
/// flags `+0x18` and motion `+0x1a`; on Coagulation, Blood Gulch's base,
/// every scenery with collision has a `phmo` and all their bodies hold
/// one pattern, the one `scenery_body` returns. Their `+0x1e`, the size
/// class, is each body's own: the class of the base body nearest in
/// bounding radius, the body itself left out (`rebuild::size_class`),
/// on every Coagulation scenery body (V: 5 at 1.72 to 1.82, 6 at 2.53 and
/// 7.33); the other bases print how many agree.
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn scenery_bodies_take_the_bases_scenery_words() {
    use h1objects::rebuild::{
        PHMO_RIGID_BODIES, RIGID_BODY_SIZE, scenery_body, size_class, size_classes,
    };
    for base_name in ["coagulation", "beavercreek", "05a_deltaapproach"] {
        let mut m = h2_map(base_name);
        let base = Base::read(&mut m).unwrap();
        let all = size_classes(&base).unwrap();
        let mut seen = HashSet::new();
        let mut votes = BTreeMap::<(u16, u16), Vec<String>>::new();
        // (scenery, radius, its class, the class by its size)
        let mut sizes = Vec::new();
        let (mut with_coll, mut with_phmo) = (0, 0);
        for t in base.tags.iter().filter(|t| t.1 == FourCC(*b"scen")) {
            let r = base.root(t.0).unwrap();
            let Some(h) = base.reference(&r, h2::MODEL) else {
                continue;
            };
            let hr = base.root(h).unwrap();
            // `hlmt +0x8`: the collision model.
            let coll = base.reference(&hr, 0x8).is_some();
            let phmo = base.reference(&hr, h2::HLMT_PHYSICS_MODEL);
            with_coll += usize::from(coll);
            with_phmo += usize::from(coll && phmo.is_some());
            let Some(p) = phmo else { continue };
            if !seen.insert(p) {
                continue;
            }
            let pr = base.root(p).unwrap();
            let mut i = 0;
            while let Some(rb) = base
                .element(&pr, PHMO_RIGID_BODIES, RIGID_BODY_SIZE, i)
                .unwrap()
            {
                let h = |o: usize| u16::from_le_bytes([rb[o], rb[o + 1]]);
                votes
                    .entry((h(0x18), h(0x1a)))
                    .or_default()
                    .push(t.2.clone());
                let radius = f32::from_le_bytes(rb[0x14..0x18].try_into().unwrap());
                let mut others = all.clone();
                if let Some(k) = others.iter().position(|c| *c == (radius, h(0x1e))) {
                    others.remove(k);
                }
                sizes.push((t.2.clone(), radius, h(0x1e), size_class(&others, radius)));
                i += 1;
            }
        }
        let body = scenery_body(&base).unwrap();
        let agree = sizes.iter().filter(|s| Some(s.2) == s.3).count();
        println!(
            "{base_name}: {with_phmo} of {with_coll} scenery with collision have a phmo; bodies (flags, motion): {votes:#?}; scenery_body {body:?}; size classes (scenery, radius, +0x1e, by size) {sizes:#?}; {agree} of {} agree",
            sizes.len()
        );
        if base_name == "coagulation" {
            assert_eq!(
                with_phmo, with_coll,
                "every Coagulation scenery with collision"
            );
            assert_eq!(votes.len(), 1, "one pattern: {votes:?}");
            let ((flags, motion), _) = votes.iter().next().unwrap();
            let (w, _) = body.unwrap();
            assert_eq!((w.flags, w.motion), (*flags, *motion));
            assert_eq!(agree, sizes.len(), "size classes by size: {sizes:?}");
        }
    }
}
