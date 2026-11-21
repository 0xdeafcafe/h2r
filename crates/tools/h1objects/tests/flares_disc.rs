//! Halo 1 lens flares, debris and placed lights against the discs. Reads
//! `$H1_ISO` and `$H2_DISC`; nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test it flares_disc:: -- --ignored --nocapture
//! ```
//!
//! Surveys (print only): `halo1_layout_survey` (`H1_GROUP`, the root words
//! and the blocks of a Halo 1 group), `halo1_reference_survey` (which Halo 1
//! tags name a `lens`, `garb` or `lifi`), `halo2_reference_survey` (the
//! same on Halo 2, with the classes effect parts create),
//! `halo2_light_placement_survey` (the scenario's light placements).

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Group, H1Map, TagRef};
use map::{FourCC, Map, TagId};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

/// Every Halo 1 map on the disc, by stem.
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

/// Every Halo 2 map on the disc, by stem.
fn h2_maps() -> Vec<(String, Map<Cursor<Vec<u8>>>)> {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap();
    let mut out = Vec::new();
    for e in iso.entries().unwrap() {
        let file = e.path.rsplit(['/', '\\']).next().unwrap_or("").to_string();
        let Some(stem) = file.strip_suffix(".map") else {
            continue;
        };
        if let Ok(only) = std::env::var("H2_LEVELS")
            && !only.split(',').any(|l| l == stem)
        {
            continue;
        }
        let bytes = iso.read(&e).unwrap();
        if let Ok(m) = Map::from_reader(Cursor::new(bytes)) {
            out.push((stem.to_string(), m));
        }
    }
    out
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

#[derive(Default)]
struct Words {
    nonzero: usize,
    min: f32,
    max: f32,
    few: BTreeMap<u32, usize>,
    distinct_capped: bool,
}

fn note<K: Ord>(words: &mut BTreeMap<K, Words>, at: K, w: u32) {
    let e = words.entry(at).or_insert(Words {
        min: f32::MAX,
        max: f32::MIN,
        ..Words::default()
    });
    if w == 0 {
        return;
    }
    e.nonzero += 1;
    let f = f32::from_bits(w);
    if f.is_finite() {
        e.min = e.min.min(f);
        e.max = e.max.max(f);
    }
    if e.few.len() < 8 || e.few.contains_key(&w) {
        *e.few.entry(w).or_default() += 1;
    } else {
        e.distinct_capped = true;
    }
}

fn print_words(words: &BTreeMap<usize, Words>) {
    for (at, w) in words {
        if w.nonzero == 0 {
            continue;
        }
        let few: Vec<String> = w
            .few
            .iter()
            .map(|(k, n)| format!("{k:#x}x{n}"))
            .collect();
        println!(
            "  {at:#05x}: nonzero {}, float {} ..= {}, words {few:?}{}",
            w.nonzero,
            w.min,
            w.max,
            if w.distinct_capped { " ..." } else { "" }
        );
    }
}

/// Survey: a Halo 1 group's tag lengths, the words of its root and its
/// blocks (a count then an address inside the tag's own bytes), with the
/// strides that tile them.
#[test]
#[ignore]
fn halo1_layout_survey() {
    let g = std::env::var("H1_GROUP").unwrap_or_else(|_| "lens".into());
    let root = usize::from_str_radix(
        std::env::var("H1_ROOT").unwrap_or_else(|_| "100".into()).trim_start_matches("0x"),
        16,
    )
    .unwrap();
    let group = Group::new(g.as_bytes().try_into().unwrap());
    let mut lens: BTreeMap<usize, usize> = BTreeMap::new();
    let mut words: BTreeMap<usize, Words> = BTreeMap::new();
    let mut blocks: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut refs: BTreeMap<String, usize> = BTreeMap::new();
    let mut element_words: BTreeMap<(usize, usize), Words> = BTreeMap::new();
    let stride_of: BTreeMap<usize, usize> = std::env::var("H1_STRIDES")
        .unwrap_or_default()
        .split(',')
        .filter_map(|p| {
            let (a, s) = p.split_once(':')?;
            Some((
                usize::from_str_radix(a.trim_start_matches("0x"), 16).ok()?,
                usize::from_str_radix(s.trim_start_matches("0x"), 16).ok()?,
            ))
        })
        .collect();
    let mut n = 0;
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == group) {
            let Ok(b) = m.tag_data(t) else { continue };
            n += 1;
            *lens.entry(b.len()).or_default() += 1;
            let len = b.len().min(root);
            for at in (0..len).step_by(4).filter(|a| a + 4 <= len) {
                note(&mut words, at, u32_at(b, at));
            }
            for at in (0..len.saturating_sub(16)).step_by(4) {
                if let Some(r) = TagRef::read(b, at)
                    && !r.id.is_none()
                    && let Ok(x) = m.tag(r.id)
                    && x.group == r.group
                {
                    *refs.entry(format!("{at:#05x} {}", r.group)).or_default() += 1;
                }
            }
            let start = t.data_address;
            let end = start + b.len() as u32;
            for at in (0..len.saturating_sub(12)).step_by(4) {
                let count = u32_at(b, at) as usize;
                let addr = u32_at(b, at + 4);
                if count == 0 || count > 4096 || addr <= start || addr >= end {
                    continue;
                }
                let off = (addr - start) as usize;
                blocks
                    .entry(at)
                    .or_default()
                    .entry(format!("first at {off:#x}"))
                    .and_modify(|c| *c += 1)
                    .or_insert(1);
                *blocks
                    .entry(at)
                    .or_default()
                    .entry(format!("count {count}"))
                    .or_default() += 1;
                if let Some(&stride) = stride_of.get(&at) {
                    for k in 0..count {
                        let e = off + k * stride;
                        if e + stride > b.len() {
                            break;
                        }
                        for w in (0..stride).step_by(4) {
                            note(&mut element_words, (at, w), u32_at(b, e + w));
                        }
                    }
                }
            }
        }
    }
    println!("{n} {g} tags; lengths {lens:?}");
    for (k, v) in &refs {
        println!("ref {k}: {v}");
    }
    for (at, v) in &blocks {
        let mut v: Vec<_> = v.iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        v.truncate(8);
        println!("block? {at:#05x}: {v:?}");
    }
    println!("root words:");
    print_words(&words);
    let mut per: BTreeMap<usize, BTreeMap<usize, Words>> = BTreeMap::new();
    for ((b, w), v) in element_words {
        per.entry(b).or_default().insert(w, v);
    }
    for (b, w) in &per {
        println!("elements of block {b:#x}:");
        print_words(w);
    }
}

/// Survey: every Halo 1 tag that names a `lens`, `garb` or `lifi` (a
/// 16-byte reference whose id resolves to a tag of its group), by the
/// naming tag's group; and the light fixtures' object attachments.
#[test]
#[ignore]
fn halo1_reference_survey() {
    let wanted = [b"lens", b"garb", b"lifi", b"ligh"];
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut fixtures = 0;
    let mut fixture_attachments: BTreeMap<String, usize> = BTreeMap::new();
    for (stem, m) in h1_maps() {
        for (g, c) in m.group_counts() {
            *counts.entry(format!("{g}")).or_default() += c;
        }
        for t in m.tags() {
            let Ok(b) = m.tag_data(t) else { continue };
            for at in (0..b.len().saturating_sub(16)).step_by(4) {
                if let Some(r) = TagRef::read(b, at)
                    && wanted.iter().any(|w| r.group == Group::new(w))
                    && !r.id.is_none()
                    && let Ok(x) = m.tag(r.id)
                    && x.group == r.group
                {
                    let place = if at < 0x400 {
                        format!("{at:#x}")
                    } else {
                        "block".into()
                    };
                    *by.entry(format!("{} -> {} at {place}", t.group, r.group))
                        .or_default() += 1;
                }
            }
            if t.group == Group::new(b"lifi") {
                fixtures += 1;
                // obje +0x140: attachments, 0x48 each, the tag first.
                let n = u32_at(b, 0x140) as usize;
                let addr = u32_at(b, 0x144);
                if n > 0
                    && let Ok(e) = m.block(h1map::Block { count: n as u32, address: addr }, 0x48)
                {
                    for a in e.chunks_exact(0x48) {
                        let r = TagRef::read(a, 0).unwrap();
                        *fixture_attachments.entry(format!("{}", r.group)).or_default() += 1;
                    }
                }
            }
        }
        // Scenario light fixtures (0x2c4, 0x58) and their palette.
        let s = m.scenario();
        if let Ok(b) = m.tag_data(s) {
            let n = u32_at(b, 0x2c4);
            let p = u32_at(b, 0x2d0);
            if n > 0 || p > 0 {
                println!("{stem}: {n} light fixture placements, palette {p}");
            }
        }
    }
    println!("group counts: {counts:?}");
    println!("{fixtures} lifi tags; attachments {fixture_attachments:?}");
    for (k, v) in &by {
        println!("{k}: {v}");
    }
}

/// Survey: on every Halo 2 map, which groups name a `lens` or `garb` (an
/// 8-byte reference whose datum is a tag of that group), the classes
/// effect `obje` parts create, and the `garb` and `lens` counts.
#[test]
#[ignore]
fn halo2_reference_survey() {
    let mut by: BTreeMap<String, usize> = BTreeMap::new();
    let mut part_classes: BTreeMap<String, usize> = BTreeMap::new();
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut names: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (stem, mut m) in h2_maps() {
        let tags: Vec<(TagId, FourCC, Option<String>, bool)> = m
            .tags()
            .iter()
            .map(|t| (t.id, t.group, t.name.clone(), t.location.is_some()))
            .collect();
        let group_of: BTreeMap<u32, FourCC> = tags.iter().map(|t| (t.0.0, t.1)).collect();
        for (id, g, name, local) in &tags {
            if !local {
                continue;
            }
            if [*b"lens", *b"garb", *b"PRTM", *b"lifi"].contains(&g.0) {
                *counts.entry(format!("{g}")).or_default() += 1;
                if let Some(n) = name {
                    names.entry(format!("{g}")).or_default().push(format!("{stem}:{n}"));
                }
            }
            if g.0 == *b"effe"
                && let Ok(e) = tags::effects::read::<tags::effects::effect::Effect, _>(&mut m, *id)
            {
                for ev in &e.events {
                    for p in &ev.parts {
                        if p.reference.is_null() {
                            continue;
                        }
                        let class = group_of.get(&p.reference.id.0).copied();
                        *part_classes
                            .entry(format!("{} -> {:?}", p.type_group, class))
                            .or_default() += 1;
                    }
                }
            }
            let Ok(b) = m.read_tag_bytes(*id) else { continue };
            for at in (0..b.len().saturating_sub(8)).step_by(4) {
                let gw = FourCC(u32_at(&b, at).to_be_bytes());
                if gw.0 != *b"lens" && gw.0 != *b"garb" {
                    continue;
                }
                let d = u32_at(&b, at + 4);
                if group_of.get(&d) == Some(&gw) {
                    let place = if at < 0x400 {
                        format!("{at:#x}")
                    } else {
                        "deep".into()
                    };
                    *by.entry(format!("{g} -> {gw} at {place}")).or_default() += 1;
                }
            }
        }
    }
    println!("counts {counts:?}");
    for (k, v) in &by {
        println!("{k}: {v}");
    }
    for (k, v) in &part_classes {
        println!("part {k}: {v}");
    }
    for (g, v) in &names {
        let mut v = v.clone();
        v.sort();
        v.dedup();
        println!("{g} names ({}):", v.len());
        for n in v.iter().take(60) {
            println!("  {n}");
        }
    }
}

/// Survey: the Halo 2 scenario's light placements (`scnr +0xe8`, `0x6c`)
/// and palette (`+0xf0`, `0x28`): every word's values over the disc.
#[test]
#[ignore]
fn halo2_light_placement_survey() {
    use tags::effects::Memory;
    let mut words: BTreeMap<usize, Words> = BTreeMap::new();
    let mut palette: BTreeMap<usize, Words> = BTreeMap::new();
    let mut n = 0;
    for (stem, mut m) in h2_maps() {
        let Some(s) = m.scenario().cloned() else { continue };
        let Some(loc) = s.location else { continue };
        let Ok(root) = Memory::read(&mut m, loc.address, 0x400) else { continue };
        let (count, at) = (u32_at(&root, 0xe8) as usize, u32_at(&root, 0xec));
        let (pc, pat) = (u32_at(&root, 0xf0) as usize, u32_at(&root, 0xf4));
        println!("{stem}: {count} light placements, palette {pc}");
        if count > 0
            && let Ok(e) = Memory::read(&mut m, at, count * 0x6c)
        {
            for x in e.chunks_exact(0x6c) {
                n += 1;
                for w in (0..0x6c).step_by(4) {
                    note(&mut words, w, u32_at(x, w));
                }
            }
        }
        if pc > 0
            && let Ok(e) = Memory::read(&mut m, pat, pc * 0x28)
        {
            for x in e.chunks_exact(0x28) {
                for w in (0..0x28).step_by(4) {
                    note(&mut palette, w, u32_at(x, w));
                }
            }
        }
    }
    println!("{n} light placements:");
    print_words(&words);
    println!("palette:");
    print_words(&palette);
}

fn show(b: &[u8]) -> String {
    b.chunks_exact(4)
        .enumerate()
        .filter(|(_, w)| w.iter().any(|&x| x != 0))
        .map(|(i, w)| {
            let v = u32::from_le_bytes(w.try_into().unwrap());
            let f = f32::from_bits(v);
            if f.is_finite() && f.abs() > 1e-6 && f.abs() < 1e7 {
                format!("{:#x}={f}", i * 4)
            } else {
                format!("{:#x}=#{v:x}", i * 4)
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Survey: lens flares whose path is on both discs, Halo 1's root (to
/// `H1_ROOT`) and first reflection (`H1_REFLECTION` at `H1_REFLECTIONS`)
/// beside Halo 2's.
#[test]
#[ignore]
fn lens_pair_survey() {
    let hex = |k: &str, d: &str| {
        usize::from_str_radix(
            std::env::var(k).unwrap_or_else(|_| d.into()).trim_start_matches("0x"),
            16,
        )
        .unwrap()
    };
    let (root, refl_at, refl) = (hex("H1_ROOT", "f0"), hex("H1_REFLECTIONS", "c4"), hex("H1_REFLECTION", "80"));
    let mut h1: BTreeMap<String, (Vec<u8>, Vec<u8>, usize)> = BTreeMap::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == Group::new(b"lens")) {
            let Ok(b) = m.tag_data(t) else { continue };
            if b.len() < root || h1.contains_key(&t.name) {
                continue;
            }
            let n = u32_at(b, refl_at) as usize;
            let first = if n > 0 {
                m.read(u32_at(b, refl_at + 4), refl).map(|x| x.to_vec()).unwrap_or_default()
            } else {
                Vec::new()
            };
            h1.insert(t.name.to_lowercase(), (b[..root].to_vec(), first, n));
        }
    }
    let mut shown = 0;
    let mut seen = std::collections::BTreeSet::new();
    for (stem, mut m) in h2_maps() {
        let ids: Vec<(TagId, String)> = m
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"lens" && t.location.is_some())
            .filter_map(|t| Some((t.id, t.name.clone()?)))
            .collect();
        for (id, name) in ids {
            let key = name.to_lowercase();
            if !h1.contains_key(&key) || !seen.insert(key.clone()) {
                continue;
            }
            let Ok(l) = tags::effects::read::<tags::effects::lens::LensFlare, _>(&mut m, id) else {
                continue;
            };
            let (r1, e1, n1) = &h1[&key];
            println!("== {stem}:{name}");
            println!("  h1 root: {}", show(r1));
            println!("  h2 root: {}", show(&l.raw));
            println!("  h1 reflections {n1}, first: {}", show(e1));
            println!(
                "  h2 reflections {}, first: {}",
                l.reflections.len(),
                l.reflections.first().map(|r| show(&r.raw)).unwrap_or_default()
            );
            println!(
                "  h2 functions {} {} {}",
                l.functions_4c.len(),
                l.functions_54.len(),
                l.functions_5c.len()
            );
            shown += 1;
            if shown >= 12 {
                return;
            }
        }
    }
    println!("{shown} pairs of {} Halo 1 lens names", h1.len());
}

/// Survey: every Halo 2 lens flare's root words and reflection words.
#[test]
#[ignore]
fn halo2_lens_survey() {
    let mut root: BTreeMap<usize, Words> = BTreeMap::new();
    let mut refl: BTreeMap<usize, Words> = BTreeMap::new();
    let mut n = 0;
    let mut seen = std::collections::BTreeSet::new();
    for (_, mut m) in h2_maps() {
        let ids: Vec<(TagId, Option<String>)> = m
            .tags()
            .iter()
            .filter(|t| t.group.0 == *b"lens" && t.location.is_some())
            .map(|t| (t.id, t.name.clone()))
            .collect();
        for (id, name) in ids {
            if !seen.insert(name.unwrap_or_default()) {
                continue;
            }
            let Ok(l) = tags::effects::read::<tags::effects::lens::LensFlare, _>(&mut m, id) else {
                continue;
            };
            n += 1;
            for w in (0..l.raw.len()).step_by(4) {
                note(&mut root, w, u32_at(&l.raw, w));
            }
            for r in &l.reflections {
                for w in (0..r.raw.len()).step_by(4) {
                    note(&mut refl, w, u32_at(&r.raw, w));
                }
            }
        }
    }
    println!("{n} distinct lens flares; root:");
    print_words(&root);
    println!("reflections:");
    print_words(&refl);
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

/// Reads `c`'s root through its group's definition, checks it writes the
/// same again and pins nothing (no block or reference left unresolved).
fn read_back(c: &h1objects::Converted, name: &str) -> tags::defs::Record {
    use tags::defs::{self, Defs, Val};
    let d = Defs::builtin().group(c.group).unwrap();
    assert!(c.raw.is_empty(), "{name}: {} raw", c.group);
    let r = defs::from_built(d, &c.root).unwrap_or_else(|e| panic!("{name}: {e}"));
    let again = defs::to_built(d, &r, &mut |_| None).unwrap();
    assert_eq!(defs::from_built(d, &again).unwrap(), r, "{name}: {}", c.group);
    fn pinned(d: &tags::defs::StructDef, r: &tags::defs::Record) -> bool {
        d.fields.iter().zip(&r.fields).any(|(f, (_, v))| match (&f.kind, v) {
            (_, Val::Unresolved { .. }) => true,
            (defs::Kind::Block(s), Val::Block(e)) => e.iter().any(|x| pinned(s, x)),
            (defs::Kind::Struct(s), Val::Struct(x)) => pinned(s, x),
            _ => false,
        })
    }
    assert!(!pinned(d, &r), "{name}: pinned");
    r
}

/// The tree tag a placeholder datum names, if any.
fn tree_tag(tree: &h1objects::convert::Tree, id: TagId) -> Option<&h1objects::Converted> {
    (id.0 & 0xffff_0000 == h1objects::convert::local(0).0)
        .then(|| tree.tags.get(id.0 as usize & 0xffff).map(|t| &t.1))
        .flatten()
}

fn target(r: &tags::defs::Record, path: &str) -> Option<TagId> {
    match r.at(path) {
        Some(tags::defs::Val::Ref {
            target: tags::defs::Target::Id(id),
            ..
        }) => Some(*id),
        Some(tags::defs::Val::Ref { .. }) => None,
        x => panic!("{path}: {x:?}"),
    }
}

/// Every Halo 1 lens flare, garbage object and light fixture of the ten
/// campaign levels (on 05a_deltaapproach) and the multiplayer maps (on
/// coagulation and beavercreek) converts from the definitions: each reads
/// back and pins nothing; a lens keeps Halo 1's angles and distances and
/// its bitmap resolves in the tree; every light whose Halo 1 light names
/// a flare names a converted lens; every effect part naming Halo 1
/// garbage names a converted `garb` through `obje`; every light fixture's
/// machine carries Halo 1's lights as attachments; every sky light's
/// flare reads ([`h1convert::lens::Flare`]).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_flare_debris_and_fixture_converts() {
    use h1objects::base::Base;
    use h1objects::convert::Converter;
    use tags::defs::Val;
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
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(disc("H1_ISO", "halo1.iso")).unwrap())).unwrap();
    let d1 = h1map::disc::Disc::open(&mut iso).unwrap();
    let (mut flares, mut reflections, mut flared_lights) = (0usize, 0usize, 0usize);
    let (mut garbage, mut garbage_parts, mut fixtures, mut fixture_lights) = (0usize, 0usize, 0usize, 0usize);
    let (mut sky_flares, mut sky_flare_lights) = (0usize, 0usize);
    for (base_name, levels) in sets {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        for level in levels {
            if only.as_deref().is_some_and(|o| !o.split(',').any(|x| x == level)) {
                continue;
            }
            let Some(e) = d1.map(&level).cloned() else { continue };
            let m = H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
            let mut c = Converter::new(&m, &base);
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"lens")) {
                let l = h1convert::lens::H1Lens::read(&m, t).unwrap();
                let id = c.lens_flare(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                let x = tree_tag(&c.tree, id).expect("lens in the tree");
                let r = read_back(x, &t.name);
                assert_eq!(r.at("falloff_angle"), Some(&Val::Float(l.falloff_angle)), "{}", t.name);
                assert_eq!(r.at("far_fade_distance"), Some(&Val::Float(l.far_fade_distance)));
                match r.at("reflections") {
                    Some(Val::Block(e)) => {
                        assert_eq!(e.len(), l.reflections.len());
                        for (x, h) in e.iter().zip(&l.reflections) {
                            let f = |at: usize| f32::from_bits(u32_at(&h.bytes, at));
                            assert_eq!(x.at("position"), Some(&Val::Float(f(0x1c))), "{}", t.name);
                            assert_eq!(x.at("radius_bounds"), Some(&Val::floats(&[f(0x28), f(0x2c)])));
                        }
                        reflections += e.len();
                    }
                    x => panic!("{x:?}"),
                }
                if let Some(b) = target(&r, "bitmap") {
                    assert_eq!(tree_tag(&c.tree, b).map(|x| x.group), Some(FourCC(*b"bitm")));
                }
                flares += 1;
            }
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"ligh")) {
                let l = h1objects::h1::Light::read(&m, t).unwrap();
                let id = c.light(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                let r = read_back(tree_tag(&c.tree, id).unwrap(), &t.name);
                let flare = target(&r, "lens_flare");
                assert_eq!(flare.is_some(), l.lens_flare.is_some(), "{}", t.name);
                if let Some(f) = flare {
                    assert_eq!(tree_tag(&c.tree, f).map(|x| x.group), Some(FourCC(*b"lens")));
                    flared_lights += 1;
                }
            }
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"garb")) {
                let id = c.garbage(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                let r = read_back(tree_tag(&c.tree, id).unwrap(), &t.name);
                assert_eq!(r.at("item.obje.object_type"), Some(&Val::Enum(4)));
                garbage += 1;
            }
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"effe")) {
                let fx = h1objects::h1::Effect::read(&m, t).unwrap();
                let want = fx
                    .events
                    .iter()
                    .flat_map(|e| &e.parts)
                    .filter(|p| p.group == Group::new(b"garb") && p.reference.is_some())
                    .count();
                let id = c.effect(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                let r = read_back(tree_tag(&c.tree, id).unwrap(), &t.name);
                let mut got = 0;
                if let Some(Val::Block(events)) = r.at("events") {
                    for ev in events {
                        if let Some(Val::Block(parts)) = ev.at("parts") {
                            for p in parts {
                                if let Some(g) = target(p, "reference")
                                    .and_then(|x| tree_tag(&c.tree, x))
                                    .map(|x| x.group)
                                    && g == FourCC(*b"garb")
                                {
                                    let obje = [u32::from_be_bytes(*b"obje"), u32::from_le_bytes(*b"obje")];
                                    assert!(
                                        matches!(p.at("type_group"), Some(Val::Int(v)) if obje.contains(&(*v as u32))),
                                        "{level} {}: {:?}",
                                        t.name,
                                        p.at("type_group")
                                    );
                                    got += 1;
                                }
                            }
                        }
                    }
                }
                assert_eq!(got, want, "{level} {}", t.name);
                garbage_parts += got;
            }
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"lifi")) {
                let dev = h1objects::h1::Device::read(&m, t).unwrap();
                let lights = dev.object.attachments.iter().flatten().filter(|a| m.tag(**a).is_ok_and(|x| x.group == Group::new(b"ligh"))).count();
                let id = c.object(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                let r = read_back(tree_tag(&c.tree, id).unwrap(), &t.name);
                let got = match r.at("devi.obje.attachments") {
                    Some(Val::Block(e)) => e
                        .iter()
                        .filter(|a| target(a, "type").and_then(|x| tree_tag(&c.tree, x)).map(|x| x.group) == Some(FourCC(*b"ligh")))
                        .count(),
                    x => panic!("{x:?}"),
                };
                assert_eq!(got, lights, "{level} {}", t.name);
                fixtures += 1;
                fixture_lights += got;
            }
            let skies = h1convert::sky::read(&m).unwrap();
            for s in &skies.skies {
                for (l, f) in s.sky.lights.iter().zip(&s.flares) {
                    assert_eq!(f.is_some(), !l.lens_flare.id.is_none(), "{level} {}", s.name);
                    if f.is_some() {
                        sky_flare_lights += 1;
                    }
                }
                sky_flares += s.flares.iter().flatten().count();
            }
        }
    }
    println!(
        "{flares} lens flares ({reflections} reflections), {flared_lights} lights naming one; \
         {garbage} garbage, {garbage_parts} effect parts naming one; \
         {fixtures} light fixtures with {fixture_lights} lights; \
         {sky_flare_lights} sky lights with a flare ({sky_flares})"
    );
    assert!(flares > 0 && garbage > 0 && fixtures > 0);
}
