//! The player biped's physics against the Chief's: the converted Halo 1
//! cyborg (a10) beside Halo 2's `masterchief` on the campaign base
//! (`05a_deltaapproach`). Reads `$H1_ISO` and `$H2_DISC`; nothing is
//! written. The first test asserts every converted rigid body carries the
//! Chief's body words (flags, motion, `+0x1e`) and prints the remaining
//! differences of the two `bipd` and `phmo` records; the second tallies
//! the body words of every retail physics model.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it biped_physics_disc:: -- --ignored --nocapture
//! ```

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::H1Map;
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree};
use h1objects::Converted;
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, StructDef, Target, Val};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
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

/// Every scalar of a record as text, by path (blocks: their count and,
/// for the first `elems`, their elements).
fn flat(d: &StructDef, r: &Record, prefix: &str, elems: usize, out: &mut BTreeMap<String, String>) {
    for (f, (_, v)) in d.fields.iter().zip(&r.fields) {
        let p = format!("{prefix}{}", f.name);
        match (&f.kind, v) {
            (defs::Kind::Struct(s), Val::Struct(x)) => flat(s, x, &format!("{p}."), elems, out),
            (defs::Kind::Block(s), Val::Block(e)) => {
                out.insert(format!("{p}#"), e.len().to_string());
                for (i, x) in e.iter().take(elems).enumerate() {
                    flat(s, x, &format!("{p}[{i}]."), elems, out);
                }
            }
            (_, Val::Bytes(b)) if b.len() == 4 => {
                let w = u32::from_le_bytes(b[..4].try_into().unwrap());
                let fl = f32::from_bits(w);
                let as_f = if fl.is_finite() && fl.abs() > 1e-6 && fl.abs() < 1e6 {
                    format!(" ({fl})")
                } else {
                    String::new()
                };
                out.insert(p, format!("{w:08x}{as_f}"));
            }
            (_, Val::Bytes(b)) => {
                out.insert(p, format!("{b:02x?}"));
            }
            (_, Val::Ref { .. }) => {}
            (_, x) => {
                out.insert(p, format!("{x:?}"));
            }
        }
    }
}

fn print_diff(what: &str, a: &BTreeMap<String, String>, b: &BTreeMap<String, String>) {
    println!("--- {what}: converted vs retail");
    for k in a.keys().chain(b.keys().filter(|k| !a.contains_key(*k))) {
        let (x, y) = (a.get(k), b.get(k));
        if x != y {
            println!("  {k}: {} | {}", x.map_or("-", |s| s), y.map_or("-", |s| s));
        }
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn the_converted_cyborgs_physics_against_the_chiefs() {
    let mut base_map = h2_map("05a_deltaapproach");
    let base = Base::read(&mut base_map).unwrap();
    let (bd, hd, pd) = (def(b"bipd"), def(b"hlmt"), def(b"phmo"));
    let chief_id = base.find(b"bipd", "objects\\characters\\masterchief\\masterchief").unwrap();
    let chief = base.record(bd, chief_id).unwrap();
    let ch = base.record(hd, reference(&chief, "unit.obje.model").unwrap()).unwrap();
    let cp = base.record(pd, reference(&ch, "physics_model").unwrap()).unwrap();

    let h1 = h1_level(&std::env::var("H1_LEVEL").unwrap_or_else(|_| "a10".into()));
    let t = h1
        .tags()
        .iter()
        .find(|t| t.group.0.to_be_bytes() == *b"bipd" && t.name.ends_with("cyborg"))
        .unwrap()
        .clone();
    let mut c = Converter::new(&h1, &base);
    let id = c.object(&t).unwrap();
    let got = local(&c.tree, id).unwrap();
    let r = defs::from_built(bd, &got.root).unwrap();
    let hl = local(&c.tree, reference(&r, "unit.obje.model").unwrap()).unwrap();
    let h = defs::from_built(hd, &hl.root).unwrap();
    let p = reference(&h, "physics_model").and_then(|p| local(&c.tree, p));

    // Every converted body's flags, motion and `+0x1e` are the Chief's.
    let body_words = |rec: &Record| -> Vec<[u16; 3]> {
        let Some(Val::Block(rbs)) = rec.at("rigid_bodies") else {
            panic!("no rigid bodies")
        };
        rbs.iter()
            .map(|rb| {
                let b = |f: &str| match rb.at(f) {
                    Some(Val::Bytes(b)) => b.clone(),
                    x => panic!("{f}: {x:?}"),
                };
                let (x, y) = (b("unknown_18"), b("unknown_1c"));
                let w = |v: &[u8], o: usize| u16::from_le_bytes([v[o], v[o + 1]]);
                [w(&x, 0), w(&x, 2), w(&y, 2)]
            })
            .collect()
    };
    let chief_words = body_words(&cp);
    let ours = body_words(&defs::from_built(pd, &p.expect("a physics model").root).unwrap());
    println!("chief {chief_words:?}\nconverted {ours:?}");
    assert!(!chief_words.is_empty() && !ours.is_empty());
    assert!(chief_words.iter().all(|w| *w == chief_words[0]));
    assert!(ours.iter().all(|w| *w == chief_words[0]), "{ours:?} against {chief_words:?}");
    let b = h1objects::rebuild::BIPED_BODY;
    assert_eq!(chief_words[0], [b.flags, b.motion, b.word_1e]);

    let (mut a, mut b) = (BTreeMap::new(), BTreeMap::new());
    flat(bd, &r, "", 3, &mut a);
    flat(bd, &chief, "", 3, &mut b);
    print_diff("bipd", &a, &b);
    let (mut a, mut b) = (BTreeMap::new(), BTreeMap::new());
    if let Some(p) = p {
        flat(pd, &defs::from_built(pd, &p.root).unwrap(), "", 12, &mut a);
    }
    flat(pd, &cp, "", 12, &mut b);
    print_diff("phmo", &a, &b);
}

/// Every retail physics model's rigid-body words `+0x18` (low byte),
/// `+0x1a`, `+0x1c` and `+0x1e`, tallied by what the model belongs to
/// (`objects\characters\` against the rest).
#[test]
#[ignore = "reads the Halo 2 disc ($H2_DISC)"]
fn retail_rigid_body_words_by_kind() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let mut tally: BTreeMap<(bool, [u16; 4]), (usize, std::collections::BTreeSet<String>)> =
        BTreeMap::new();
    let mut seen = std::collections::HashSet::new();
    for e in iso.entries().unwrap() {
        let Some(_) = e.path.strip_suffix(".map") else {
            continue;
        };
        let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
        let Ok(mut m) = Map::from_region(BufReader::new(File::open(&path).unwrap()), base, e.size.into())
        else {
            continue;
        };
        let phmos: Vec<_> = m
            .tags()
            .iter()
            .filter(|t| t.group == FourCC(*b"phmo") && t.location.is_some())
            .map(|t| (t.id, t.name.clone().unwrap_or_default()))
            .collect();
        for (id, name) in phmos {
            if !seen.insert(name.clone()) {
                continue;
            }
            let Ok(p) = tags::collision::PhysicsModel::read(&mut m, id) else {
                continue;
            };
            let character = name.contains("characters\\");
            for rb in &p.rigid_bodies {
                let w = |o: usize| u16::from_le_bytes([rb.raw[o], rb.raw[o + 1]]);
                let k = [w(0x18), w(0x1a), w(0x1c), w(0x1e)];
                let t = tally.entry((character, k)).or_default();
                t.0 += 1;
                if t.1.len() < 6 {
                    t.1.insert(name.rsplit('\\').next().unwrap_or("").to_owned());
                }
            }
        }
    }
    for ((ch, k), (n, names)) in &tally {
        println!(
            "{} +18 {:#06x} +1a {} +1c {:#06x} +1e {}: {n} bodies, e.g. {names:?}",
            if *ch { "character" } else { "other    " },
            k[0],
            k[1],
            k[2],
            k[3]
        );
    }
}
