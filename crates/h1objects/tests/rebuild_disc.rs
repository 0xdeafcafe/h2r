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
//!   words, and Havok sees the scenery through its `coll`'s BSP physics
//!   slots (no `phmo`), as retail's.
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
                for f in ["attachments", "widgets", "functions"] {
                    assert_eq!(
                        r.at(&format!("object.{f}")),
                        Some(&Val::Block(Vec::new())),
                        "{name}: {f}"
                    );
                }
                // Change colours: Halo 1's in Halo 2's four slots
                // (`h1::ChangeColour::halo2_slots`), each with Halo 1's
                // permutations; none without (unless the model reads C).
                let Some(Val::Block(cc)) = r.at("object.change_colors") else {
                    panic!("{name}: change colours")
                };
                if obj.change_colours.is_empty() {
                    assert!(
                        cc.is_empty() || cc.len() == 4,
                        "{name}: {} change colours",
                        cc.len()
                    );
                } else {
                    assert_eq!(cc.len(), 4, "{name}: change colours");
                    let perms: usize = obj
                        .change_colours
                        .iter()
                        .map(|c| c.permutations.len())
                        .sum();
                    let got: usize = (0..4)
                        .map(
                            |k| match r.at(&format!("object.change_colors[{k}].block_0")) {
                                Some(Val::Block(b)) => b.len(),
                                _ => 0,
                            },
                        )
                        .sum();
                    assert!(
                        got <= perms,
                        "{name}: {got} permutations from Halo 1's {perms}"
                    );
                    // Scenery with only A (the boulders, trees, crates):
                    // all of it in slot 0.
                    if obj.change_colours.len() == 1 {
                        assert_eq!(got, perms, "{name}: A's permutations in slot 0");
                    }
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
                assert_eq!(reference(&hr, "physics_model"), None, "{name}: phmo");
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

                // Havok's view: the coll's BSP physics slots, one per BSP.
                if let Some(cl) = coll {
                    assert_eq!(local(tree, cl).map(|x| x.group), Some(FourCC(*b"coll")));
                    assert_eq!(c.coll_reports.len(), 1, "{name}: one coll");
                    let cr = &c.coll_reports[0].1;
                    assert_eq!(cr.bsp_physics, cr.bsps, "{name}: a BSP physics slot per BSP");
                    assert!(cr.bsps > 0, "{name}: BSPs");
                    *tally.entry("collision, BSP physics".into()).or_default() += 1;
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
        let modes = kinds.get(&(0xffff_0002, "mode".into())).copied().unwrap_or(0);
        let bitmaps = kinds.get(&(0, "bitm".into())).copied().unwrap_or(0);
        assert!(modes > 0 && bitmaps > 0, "{base_name}: {kinds:?}");
        assert_eq!(first.keys().collect::<Vec<_>>(), ["mode"], "{base_name}: the model first");
    }
    assert!(statics > 0, "retail scenery with collision and no phmo");
}
