//! Scenery rebuilt from typed definitions against the template path, field
//! by field. Every Halo 1 scenery object of the levels in `$H1_LEVELS`
//! (default `a10`) is converted twice on its campaign base
//! (`05a_deltaapproach`): on the retail template (the default) and with
//! `Options::rebuild`. The `scen`, its `hlmt` and its `mode` are read back
//! through `tags::defs` and compared. Prints every field that differs, with
//! how often and why; asserts that every field the conversion writes
//! agrees. Reads `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test rebuild_disc -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree};
use h1objects::{Converted, Options};
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

/// The tree's tag a local datum names.
fn local<'t>(tree: &'t Tree, id: TagId) -> Option<&'t Converted> {
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

/// The `scen`, `hlmt` and `mode` records of the scenery at `id`.
fn chain(tree: &Tree, id: TagId) -> Vec<(&'static str, Record)> {
    let mut out = Vec::new();
    let scen = local(tree, id).expect("scen in the tree");
    let r = defs::from_built(def(b"scen"), &scen.root).expect("scen reads");
    let hlmt = reference(&r, "object.model").and_then(|h| local(tree, h));
    out.push(("scen", r));
    if let Some(h) = hlmt {
        let hr = defs::from_built(def(b"hlmt"), &h.root).expect("hlmt reads");
        let mode = reference(&hr, "render_model").and_then(|m| local(tree, m));
        out.push(("hlmt", hr));
        if let Some(m) = mode {
            out.push((
                "mode",
                defs::from_built(def(b"mode"), &m.root).expect("mode reads"),
            ));
        }
    }
    out
}

/// A path with element indices dropped (`regions[2].name` -> `regions[].name`).
fn general(path: &str) -> String {
    let mut out = String::new();
    let mut skip = false;
    for c in path.chars() {
        match c {
            '[' => {
                skip = true;
                out.push('[');
            }
            ']' => {
                skip = false;
                out.push(']');
            }
            _ if skip => {}
            _ => out.push(c),
        }
    }
    out
}

/// Fields the conversion writes from Halo 1 or the converted chain: they
/// must agree between the two paths.
const WRITTEN: &[&str] = &[
    "scen.object.object_type",
    "scen.object.bounding_radius",
    "scen.object.bounding_offset",
    "scen.object.default_variant",
    "scen.object.model",
    "hlmt.render_model",
    "hlmt.collision_model",
    "hlmt.animation",
    "hlmt.nodes",
    "hlmt.regions",
    "hlmt.variants",
    "mode.",
];

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn rebuilt_scenery_matches_the_template_path_on_every_written_field() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let levels = std::env::var("H1_LEVELS").unwrap_or("a10".into());
    // path -> (instances, first example (template, rebuilt), unknown span)
    let mut diffs: BTreeMap<String, (usize, String, String, bool)> = BTreeMap::new();
    let mut objects = 0;
    for level in levels.split(',') {
        let h1 = h1_level(level);
        for t in h1.tags() {
            if &t.group.0.to_be_bytes() != b"scen" {
                continue;
            }
            let mut a = Converter::new(&h1, &base);
            let mut b = Converter::new(&h1, &base);
            b.options = Options {
                rebuild: true,
                ..Default::default()
            };
            let (ia, ib) = match (a.object(t), b.object(t)) {
                (Ok(x), Ok(y)) => (x, y),
                (x, y) => {
                    eprintln!(
                        "{level} {}: template {:?}, rebuilt {:?}",
                        t.name,
                        x.err(),
                        y.err()
                    );
                    continue;
                }
            };
            objects += 1;
            let (ca, cb) = (chain(&a.tree, ia), chain(&b.tree, ib));
            assert_eq!(
                ca.iter().map(|c| c.0).collect::<Vec<_>>(),
                cb.iter().map(|c| c.0).collect::<Vec<_>>(),
                "{}: the same chain",
                t.name
            );
            // The rebuilt tags carry no template bytes: every block they
            // have is built, none pinned at a retail address.
            for (g, r) in &cb {
                let mut pinned = Vec::new();
                pinned_blocks(def(g.as_bytes().try_into().unwrap()), r, g, &mut pinned);
                assert!(
                    pinned.is_empty(),
                    "{}: rebuilt {g} has pinned {pinned:?}",
                    t.name
                );
            }
            for ((g, ra), (_, rb)) in ca.iter().zip(&cb) {
                let d = def(g.as_bytes().try_into().unwrap());
                for c in defs::diff(d, ra, rb) {
                    let e = diffs.entry(format!("{g}.{}", general(&c.path))).or_insert((
                        0,
                        c.a.clone(),
                        c.b.clone(),
                        c.unknown,
                    ));
                    e.0 += 1;
                }
            }
        }
    }
    println!("{objects} scenery objects compared ({levels})");
    println!("fields that differ (template -> rebuilt), instances, first example:");
    let mut bad = Vec::new();
    for (p, (n, a, b, unknown)) in &diffs {
        let why = why(p, *unknown, a);
        println!("  {p}: {n}x  {a} -> {b}  [{why}]");
        if WRITTEN.iter().any(|w| p.starts_with(w)) {
            bad.push(p.clone());
        }
    }
    assert!(objects > 0, "no scenery converted");
    assert!(bad.is_empty(), "written fields differ: {bad:?}");
}

/// Why a field differs between the paths.
fn why(path: &str, unknown: bool, template: &str) -> &'static str {
    if template.ends_with(" null") {
        "both null: the template keeps its retail group word, the rebuilt reference 0xffffffff"
    } else if unknown {
        "unknown span: the template's retail bytes; rebuilt: zero"
    } else if template.contains("(pinned)") {
        "the template's retail block, kept at its address; rebuilt: empty"
    } else if path.ends_with(".count") {
        "a block the template fills; rebuilt: empty"
    } else if template.contains(' ') && template.len() > 5 && !template.starts_with('"') {
        "the template's retail reference; rebuilt: null"
    } else {
        "the template's retail value; rebuilt: the definition default"
    }
}

fn pinned_blocks(def: &StructDef, r: &Record, path: &str, out: &mut Vec<String>) {
    for (f, (_, v)) in def.fields.iter().zip(&r.fields) {
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
