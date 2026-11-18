//! Halo 1 particles (`part`, `pctl`) and contrails (`cont`) as Halo 2
//! `prt3` and `cont`: surveys of the Halo 1 layouts and the proofs of the
//! conversion, from the owner's discs (`$H1_ISO`, `$H2_DISC`).
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Block, Group, H1Map, TagRef};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// Every Halo 1 map on the disc (`H1_LEVELS` for some), by stem.
fn h1_maps() -> Vec<(String, H1Map)> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let entries = iso.entries().unwrap();
    let mut out = Vec::new();
    for e in h1map::disc::maps(&entries) {
        let stem = h1map::disc::map_stem(&e).to_ascii_lowercase();
        if let Ok(only) = std::env::var("H1_LEVELS")
            && !only.split(',').any(|l| l == stem)
        {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        if let Ok(m) = H1Map::from_reader(Cursor::new(bytes)) {
            out.push((stem, m));
        }
    }
    out
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

/// Survey (`H1_GROUP`, default `part`; `H1_SPAN` bytes, default 0x180):
/// per 4-byte offset of every distinct tag of the group, the references
/// by group, the block fields (count range), and the words: nonzero
/// count, float range, distinct values when few.
#[test]
#[ignore]
fn halo1_layout_survey() {
    let g = std::env::var("H1_GROUP").unwrap_or_else(|_| "part".into());
    let group = Group::new(g.as_bytes().try_into().unwrap());
    let span = std::env::var("H1_SPAN")
        .ok()
        .and_then(|s| usize::from_str_radix(s.trim_start_matches("0x"), 16).ok())
        .unwrap_or(0x180);
    let mut seen = std::collections::BTreeSet::new();
    let mut tags = 0;
    let mut refs: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut words: BTreeMap<usize, BTreeMap<u32, usize>> = BTreeMap::new();
    let mut lens: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == group) {
            if !seen.insert(t.name.clone()) {
                continue;
            }
            let b = m.tag_data(t).unwrap();
            tags += 1;
            *lens.entry(b.len()).or_default() += 1;
            for at in (0..span.min(b.len().saturating_sub(4))).step_by(4) {
                *words.entry(at).or_default().entry(u32_at(b, at)).or_default() += 1;
                if at + 16 <= b.len()
                    && let Some(r) = TagRef::read(b, at)
                    && !r.id.is_none()
                    && let Ok(x) = m.tag(r.id)
                    && x.group == r.group
                {
                    let tail = x.name.rsplit('\\').next().unwrap_or("").to_owned();
                    *refs.entry(at).or_default().entry(format!("{} {tail}", r.group)).or_default() += 1;
                }
                if let Some(k) = Block::read(b, at)
                    && k.count > 0
                    && k.count < 0x400
                    && k.address >> 28 == 8
                {
                    *refs.entry(at).or_default().entry(format!("block? count {}", k.count)).or_default() += 1;
                }
            }
        }
    }
    println!("{g}: {tags} distinct tags, lengths {:?}", lens.iter().take(8).collect::<Vec<_>>());
    for (at, w) in &words {
        let nz: usize = w.iter().filter(|(v, _)| **v != 0).map(|(_, c)| c).sum();
        if nz == 0 {
            continue;
        }
        let floats: Vec<f32> = w
            .keys()
            .map(|v| f32::from_bits(*v))
            .filter(|f| f.is_finite() && f.abs() < 1e7 && (f.abs() > 1e-6 || *f == 0.0))
            .collect();
        let (lo, hi) = floats
            .iter()
            .fold((f32::MAX, f32::MIN), |(a, b), f| (a.min(*f), b.max(*f)));
        let few: Vec<String> = if w.len() <= 6 {
            w.iter().map(|(v, c)| format!("{v:#x}x{c}")).collect()
        } else {
            vec![format!("{} distinct", w.len())]
        };
        let r = refs.get(at).map(|r| {
            let mut v: Vec<_> = r.iter().collect();
            v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
            format!("{:?}", &v[..v.len().min(3)])
        });
        println!(
            "{at:#05x}: nonzero {nz}, floats {}/{} {lo} ..= {hi}, {few:?} {}",
            floats.len(),
            w.len(),
            r.unwrap_or_default()
        );
    }
}

fn h2_map(want: &str) -> map::Map<Cursor<Vec<u8>>> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.to_ascii_lowercase().ends_with(&format!("{want}.map")))
        .unwrap();
    map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn words(b: &[u8], from: usize, to: usize) -> String {
    (from..to.min(b.len()))
        .step_by(4)
        .map(|o| {
            let w = u32_at(b, o);
            let f = f32::from_bits(w);
            if w == 0 {
                "0".to_owned()
            } else if f.is_finite() && f.abs() > 1e-4 && f.abs() < 1e6 {
                format!("{f}")
            } else {
                format!("{w:#x}")
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Survey: contrails side by side, every distinct Halo 1 `cont` (root
/// `+0..+0x40`, `+0xa8..+0xe0`, its first point) and every Halo 2 `cont`
/// on `H2_BASE` (root `+0..+0x40`, `+0x40..+0xe8`, its first point).
#[test]
#[ignore]
fn contrail_survey() {
    let mut seen = std::collections::BTreeSet::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"cont")) {
            if !seen.insert(t.name.clone()) {
                continue;
            }
            let b = m.tag_data(t).unwrap();
            println!("h1 {}\n  {}\n  a8: {}\n  pt: {}", t.name, words(b, 0, 0x40), words(b, 0xa8, 0xe0), words(b, 0x144, 0x144 + 0x68));
        }
    }
    let mut m = h2_map(&std::env::var("H2_BASE").unwrap_or_else(|_| "05a_deltaapproach".into()));
    let ids: Vec<_> = m
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"cont"))
        .map(|t| (t.id, t.name.clone()))
        .collect();
    for (id, name) in ids {
        let Ok(b) = m.read_tag_bytes(id) else { continue };
        println!("h2 {name:?}\n  {}\n  40: {}\n  pt: {}", words(&b, 0, 0x40), words(&b, 0x40, 0xe8), words(&b, 0xf0, 0x130));
    }
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

/// The ten campaign levels on 05a, the multiplayer maps on their bases.
fn level_sets() -> Vec<(&'static str, Vec<String>)> {
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
    sets
}

/// Every Halo 1 contrail of the campaign and multiplayer maps converts
/// from the `cont` definition: it reads back with Halo 1's spans at Halo
/// 2's offsets, its bitmap a converted one, no pinned bytes; and every
/// rebuilt projectile and vehicle whose Halo 1 attachments name a
/// contrail names the converted one.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_contrail_converts() {
    use h1objects::base::Base;
    use h1objects::convert::Converter;
    use h1objects::particles::{CONT_POINT_SPANS, CONT_SPANS, H1Contrail};
    use tags::defs::{self, Defs, Target, Val};
    let d = Defs::builtin().group(map::FourCC(*b"cont")).unwrap();
    let bytes = |r: &defs::Record, p: &str| match r.at(p) {
        Some(Val::Bytes(b)) => b.clone(),
        x => panic!("{p}: {x:?}"),
    };
    let (mut conts, mut attached) = (0usize, 0usize);
    for (base_name, levels) in level_sets() {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        for level in levels {
            let Some(m) = h1_level(&level) else { continue };
            let mut c = Converter::new(&m, &base);
            let mut got = Vec::new();
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"cont")) {
                let id = c.contrail(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                got.push((t, id));
            }
            let mut owners = Vec::new();
            for t in m.tags().iter().filter(|t| {
                t.group == Group::new(b"proj") || t.group == Group::new(b"vehi")
            }) {
                let b = m.tag_data(t).unwrap();
                let names: Vec<_> = h1objects::h1::attachments(&m, b)
                    .unwrap()
                    .into_iter()
                    .filter(|a| a.group == Group::new(b"cont"))
                    .filter_map(|a| a.tag)
                    .collect();
                if names.is_empty() {
                    continue;
                }
                if let Ok(id) = c.object(t) {
                    owners.push((t, id, names));
                }
            }
            let tree = &c.tree;
            for (t, id) in &got {
                let name = format!("{level} {}", t.name);
                let h = H1Contrail::read(&m, t).unwrap();
                let built = &tree.tags[id.0 as usize & 0xffff].1;
                assert_eq!(built.group, map::FourCC(*b"cont"), "{name}");
                assert!(built.raw.is_empty());
                let r = defs::from_built(d, &built.root).unwrap_or_else(|e| panic!("{name}: {e}"));
                for (f, at, len) in CONT_SPANS {
                    assert_eq!(bytes(&r, f), h.root[*at..at + len], "{name} {f}");
                }
                let Some(Val::Block(pts)) = r.at("points") else { panic!() };
                assert_eq!(pts.len(), h.points.len(), "{name}");
                for (x, p) in pts.iter().zip(&h.points) {
                    for (f, at, len) in CONT_POINT_SPANS {
                        assert_eq!(bytes(x, f), p[*at..at + len], "{name} point {f}");
                    }
                }
                if h.bitmap.is_some() {
                    assert!(
                        matches!(r.at("bitmap"), Some(Val::Ref { target: Target::Id(_), .. })),
                        "{name}: bitmap"
                    );
                }
                conts += 1;
            }
            for (t, id, names) in &owners {
                let b = &tree.tags[id.0 as usize & 0xffff].1;
                let od = Defs::builtin().group(b.group).unwrap();
                let r = defs::from_built(od, &b.root).unwrap();
                let path = if b.group == map::FourCC(*b"proj") { "object.attachments" } else { "unit.obje.attachments" };
                let Some(Val::Block(att)) = r.at(path) else { panic!("{level} {}: {path}", t.name) };
                for n in names {
                    let want = got.iter().find(|g| g.0.id == *n).map(|g| g.1).unwrap();
                    assert!(
                        att.iter().any(|a| matches!(a.at("type"), Some(Val::Ref { target: Target::Id(x), .. }) if *x == want)),
                        "{level} {}: contrail attachment",
                        t.name
                    );
                    attached += 1;
                }
            }
        }
    }
    println!("{conts} contrails converted, {attached} contrail attachments on rebuilt projectiles and vehicles");
    assert!(conts > 0 && attached > 0);
}
