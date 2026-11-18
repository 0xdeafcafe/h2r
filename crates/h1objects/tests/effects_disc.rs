//! Halo 1 effects (`effe`) against the disc. Reads `$H1_ISO` (and
//! `$H2_DISC` where a base map is needed); nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test --release -p h1objects --test effects_disc -- --ignored --nocapture
//! ```
//!
//! - `halo1_effect_layout_survey`: the Halo 1 `effe` layout
//!   (`h1objects::h1::Effect`) over every map: root, events, parts (each
//!   reference of its own group word) and particles, with the value range
//!   of every part word.
//! - `every_effect_rebuilds_from_its_definition`: every `effe` and `vehi`
//!   of the ten campaign levels (on 05a_deltaapproach) and the multiplayer
//!   maps (on coagulation and beavercreek) converts; every built `effe`
//!   reads back through the definition, writes the same again, pins
//!   nothing (no template bytes: nothing but the definition is read); its
//!   locations and events are Halo 1's; every part's reference resolves
//!   to a converted sound or a tree tag of its group; every vehicle
//!   attachment resolves to a converted looping sound or effect at Halo
//!   1's marker.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1map::{Block, Group, H1Map, TagRef};
use h1objects::base::Base;
use h1objects::convert::{Converter, Tree};
use h1objects::{Converted, h1};
use map::{FourCC, Map, TagId};
use tags::defs::{self, Defs, Record, Str, StructDef, Target, Val};

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

const PLACEHOLDER: u32 = 0xffff_0000;

/// What a placeholder datum names: a tree tag's group, a sound or a
/// looping sound of the tree.
fn resolve(tree: &Tree, id: TagId) -> Option<FourCC> {
    let k = id.0 as usize & 0xffff;
    let kind = id.0 & PLACEHOLDER;
    if kind == h1objects::convert::local(0).0 {
        tree.tags.get(k).map(|t| t.1.group)
    } else if kind == h1objects::convert::sound(0).0 {
        (k < tree.sounds.len()).then_some(FourCC(*b"snd!"))
    } else if kind == h1objects::convert::looping(0).0 {
        (k < tree.loops.len()).then_some(FourCC(*b"lsnd"))
    } else {
        None
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

/// Reads `c`'s root through its group's definition, checks it writes the
/// same again and pins nothing.
fn read_back(c: &Converted, name: &str) -> Record {
    let d = def(&c.group.0);
    assert!(c.raw.is_empty(), "{name}: {} raw", c.group);
    let r = defs::from_built(d, &c.root).unwrap_or_else(|e| panic!("{name}: {e}"));
    let again = defs::to_built(d, &r, &mut |_| None).unwrap();
    assert_eq!(defs::from_built(d, &again).unwrap(), r, "{name}: {}", c.group);
    let mut pinned = Vec::new();
    pinned_blocks(d, &r, &c.group.to_string(), &mut pinned);
    assert!(pinned.is_empty(), "{name}: pinned {pinned:?}");
    r
}

fn block<'r>(r: &'r Record, path: &str) -> &'r [Record] {
    match r.at(path) {
        Some(Val::Block(b)) => b,
        x => panic!("{path}: {x:?}"),
    }
}

fn text(r: &Record, path: &str) -> String {
    match r.at(path) {
        Some(Val::String(Str::Text(t))) => t.clone(),
        _ => String::new(),
    }
}

fn target(r: &Record, path: &str) -> (u32, Option<TagId>) {
    match r.at(path) {
        Some(Val::Ref { group, target: Target::Id(id) }) => (*group, Some(*id)),
        Some(Val::Ref { group, .. }) => (*group, None),
        x => panic!("{path}: {x:?}"),
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_effect_rebuilds_from_its_definition() {
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
    let effe = Group::new(b"effe");
    let (mut effects, mut parts_kept, mut attached) = (0usize, 0usize, 0usize);
    let (mut lit, mut cones) = (0usize, 0usize);
    let (mut decaled, mut unbitmapped) = (0usize, 0usize);
    let mut left: BTreeMap<String, usize> = BTreeMap::new();
    for (base_name, levels) in sets {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        for level in levels {
            if only
                .as_deref()
                .is_some_and(|o| !o.split(',').any(|x| x == level))
            {
                continue;
            }
            let Some(m) = h1_level(&level) else { continue };
            let mut c = Converter::new(&m, &base);
            let mut wanted = Vec::new();
            for t in m.tags() {
                if t.group == effe {
                    let id = c.effect(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                    wanted.push((t, id));
                }
            }
            let mut lights = Vec::new();
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"ligh")) {
                let id = c.light(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                lights.push((t, id));
            }
            let mut decals = Vec::new();
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"deca")) {
                let id = c.decal(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                decals.push((t, id));
            }
            let mut vehicles = Vec::new();
            for t in m.tags().iter().filter(|t| t.group == Group::new(b"vehi")) {
                let id = c.object(t).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                vehicles.push((t, id));
            }
            for r in &c.tree.reports {
                if r.group == "effe" {
                    for n in r.no_home.iter().filter(|n| n.contains("parts left out")) {
                        let mut w = n.split(' ');
                        let k: usize = w.next().unwrap().parse().unwrap();
                        *left.entry(w.next().unwrap().to_owned()).or_default() += k;
                    }
                }
            }
            let tree = &c.tree;
            for (t, id) in &wanted {
                let name = format!("{level} {}", t.name);
                let e = h1::Effect::read(&m, t).unwrap();
                let got = &tree.tags[id.0 as usize & 0xffff].1;
                assert_eq!(got.group, FourCC(*b"effe"), "{name}");
                let r = read_back(got, &name);
                let locs = block(&r, "locations");
                assert_eq!(locs.len(), e.locations.len(), "{name}");
                for (x, l) in locs.iter().zip(&e.locations) {
                    assert_eq!(&text(x, "marker"), l, "{name}");
                }
                let events = block(&r, "events");
                assert_eq!(events.len(), e.events.len(), "{name}");
                for (x, ev) in events.iter().zip(&e.events) {
                    assert_eq!(x.at("skip_fraction"), Some(&Val::Float(ev.skip_fraction)));
                    let parts = block(x, "parts");
                    assert!(parts.len() <= ev.parts.len(), "{name}");
                    for p in parts {
                        let (_, tag) = target(p, "reference");
                        let tag = tag.unwrap_or_else(|| panic!("{name}: null part"));
                        let g = resolve(tree, tag)
                            .unwrap_or_else(|| panic!("{name}: part names {tag:?}"));
                        let tg = match p.at("type_group") {
                            Some(Val::Int(v)) => (*v as u32).to_be_bytes(),
                            x => panic!("{x:?}"),
                        };
                        assert_eq!(tg, h1objects::rebuild::part_type_group(g.0), "{name}");
                        let loc = match p.at("location") {
                            Some(Val::Int(v)) => *v,
                            x => panic!("{x:?}"),
                        };
                        assert!(loc < locs.len() as i64, "{name}: location {loc}");
                    }
                    parts_kept += parts.len();
                    for k in ["beams", "accelerations", "particle_systems"] {
                        assert!(block(x, k).is_empty(), "{name}: {k}");
                    }
                }
                effects += 1;
            }
            for (t, id) in &lights {
                let name = format!("{level} {}", t.name);
                let l = h1::Light::read(&m, t).unwrap();
                let got = &tree.tags[id.0 as usize & 0xffff].1;
                assert_eq!(got.group, FourCC(*b"ligh"), "{name}");
                let r = read_back(got, &name);
                let kind = if l.is_cone() { 2 } else { 0 };
                assert_eq!(r.at("kind"), Some(&Val::Enum(kind)), "{name}");
                let range = if l.is_cone() { "cutoff_distance" } else { "radius" };
                assert_eq!(r.at(range), Some(&Val::Float(l.radius)), "{name}");
                assert_eq!(
                    r.at("diffuse_colour_upper"),
                    Some(&Val::floats(&l.colour_upper[1..])),
                    "{name}"
                );
                lit += 1;
                cones += usize::from(l.is_cone());
            }
            for (t, id) in &decals {
                let name = format!("{level} {}", t.name);
                let x = h1::Decal::read(&m, t).unwrap();
                let got = &tree.tags[id.0 as usize & 0xffff].1;
                assert_eq!(got.group, FourCC(*b"deca"), "{name}");
                let r = read_back(got, &name);
                assert_eq!(r.at("radius"), Some(&Val::floats(&x.radius)), "{name}");
                assert_eq!(r.at("lifetime_bounds"), Some(&Val::floats(&x.lifetime)), "{name}");
                for (field, group, h1ref) in [("bitmap", *b"bitm", x.bitmap), ("next_decal", *b"deca", x.next)] {
                    let (_, tag) = target(&r, field);
                    match tag {
                        Some(tag) => assert_eq!(resolve(tree, tag), Some(FourCC(group)), "{name}"),
                        None => {
                            assert!(h1ref.is_none() || field == "bitmap", "{name}: {field}");
                            if h1ref.is_some() {
                                unbitmapped += 1;
                            }
                        }
                    }
                }
                decaled += 1;
            }
            for (t, id) in &vehicles {
                let name = format!("{level} {}", t.name);
                let got = &tree.tags[id.0 as usize & 0xffff].1;
                let r = read_back(got, &name);
                let b = m.tag_data(t).unwrap();
                let h1a = h1::attachments(&m, b).unwrap();
                let att = block(&r, "unit.obje.attachments");
                for a in att {
                    let (_, tag) = target(a, "type");
                    let g = resolve(tree, tag.expect("attachment")).expect("resolves");
                    assert!([*b"lsnd", *b"effe", *b"ligh"].contains(&g.0), "{name}: {g}");
                    let marker = text(a, "marker");
                    assert!(
                        h1a.iter().any(|x| x.marker == marker && x.group.0.to_be_bytes() == g.0),
                        "{name}: {marker}"
                    );
                }
                let kept = h1a
                    .iter()
                    .filter(|x| {
                        x.tag.is_some()
                            && [*b"lsnd", *b"effe", *b"ligh"].contains(&x.group.0.to_be_bytes())
                    })
                    .count();
                if att.len() != kept {
                    println!("{name}: {} of {kept} looping sounds and effects attached", att.len());
                }
                attached += att.len();
                println!(
                    "{name}: {} attachments (Halo 1's: {:?})",
                    att.len(),
                    h1a.iter()
                        .map(|a| format!("{} {}", a.group, a.tag.is_some()))
                        .collect::<Vec<_>>()
                );
            }
            let bytes: usize = tree
                .sounds
                .iter()
                .flat_map(|(_, s)| &s.ranges)
                .flat_map(|r| &r.permutations)
                .map(|p| p.size())
                .sum();
            println!(
                "{level}: {} effects, {} sounds ({bytes:#x} sample bytes), {} looping sounds",
                wanted.len(),
                tree.sounds.len(),
                tree.loops.len()
            );
        }
    }
    println!(
        "{effects} effects, {parts_kept} parts kept, {attached} vehicle attachments, {lit} lights ({cones} cones), {decaled} decals ({unbitmapped} without their bitmap)"
    );
    for (g, n) in &left {
        println!("{n:8} {g} parts left out");
    }
    assert!(effects > 0);
}

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
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

fn u16_at(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}
fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}

/// Survey: prints, per candidate offset, how the Halo 1 `effe` fields
/// hold up over every map.
#[test]
#[ignore]
fn halo1_effect_layout_survey() {
    let effe = Group::new(b"effe");
    let mut stats: BTreeMap<String, usize> = BTreeMap::new();
    let mut ranges: BTreeMap<usize, (f32, f32)> = BTreeMap::new();
    let mut bump = |k: String| *stats.entry(k).or_default() += 1;
    for (stem, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == effe) {
            let b = m.tag_data(t).unwrap();
            bump(format!("root len {:#x}", b.len().min(0x40)));
            let locs = Block::read(b, 0x28).unwrap();
            let events = Block::read(b, 0x34).unwrap();
            let Ok(lb) = m.block(locs, 0x20) else {
                bump("locations unreadable".into());
                continue;
            };
            let Ok(eb) = m.block(events, 0x44) else {
                bump("events unreadable".into());
                continue;
            };
            bump(format!("flags {:#x}", u32_at(b, 0)));
            bump(format!(
                "loop {} {}",
                u16_at(b, 4) as i16,
                u16_at(b, 6) as i16
            ));
            for k in 8..0x28 {
                if b[k] != 0 {
                    bump(format!("root nonzero at {k:#x}"));
                }
            }
            let _ = lb;
            for e in eb.chunks_exact(0x44) {
                for k in (0..4).chain(0x18..0x2c) {
                    if e[k] != 0 {
                        bump(format!("event nonzero at {k:#x}"));
                    }
                }
                let parts = Block::read(e, 0x2c).unwrap();
                let parts_b = match m.block(parts, 0x68) {
                    Ok(p) => p,
                    Err(_) => {
                        bump("parts unreadable".into());
                        continue;
                    }
                };
                for p in parts_b.chunks_exact(0x68) {
                    let loc = u16_at(p, 4) as i16;
                    if loc < 0 || loc as u32 >= locs.count {
                        bump("part location out of range".into());
                    }
                    let fourcc = u32_at(p, 0x18);
                    let r = TagRef::read(p, 0x18).unwrap();
                    let resolved = (!r.id.is_none())
                        .then(|| m.tag(r.id).ok())
                        .flatten()
                        .map(|t| t.group.to_string());
                    bump(format!(
                        "part type {} ref {} -> {resolved:?}",
                        Group(fourcc),
                        r.group
                    ));
                    bump(format!("part create_in {} violence {}", u16_at(p, 0), u16_at(p, 2)));
                    for k in (8..0x18).step_by(4).chain((0x28..0x68).step_by(4)) {
                        let w = u32_at(p, k);
                        if w != 0 {
                            bump(format!("part word {k:#04x} nonzero"));
                            let f = f32::from_bits(w);
                            let e = ranges.entry(k).or_insert((f32::MAX, f32::MIN));
                            e.0 = e.0.min(f);
                            e.1 = e.1.max(f);
                        }
                    }
                    bump(format!("part flags {:#x}", u16_at(p, 6)));
                    bump(format!("part 0x60 {:#x} 0x64 {:#x}", u32_at(p, 0x60), u32_at(p, 0x64)));
                }
                let particles = Block::read(e, 0x38).unwrap();
                match m.block(particles, 0xe8) {
                    Ok(pb) => {
                        for p in pb.chunks_exact(0xe8) {
                            let loc = u16_at(p, 4) as i16;
                            if loc < 0 || loc as u32 >= locs.count {
                                bump(format!("particle +4 out of range {loc}"));
                            }

                            // Every reference in the element.
                            for at in (0..0xe8 - 16).step_by(4) {
                                if let Some(r) = TagRef::read(p, at)
                                    && !r.id.is_none()
                                    && let Ok(t) = m.tag(r.id)
                                    && t.group == r.group
                                {
                                    bump(format!("particle ref at {at:#x} {}", r.group));
                                }
                            }
                        }
                    }
                    Err(_) => bump("particles unreadable".into()),
                }
            }
        }
        println!("{stem} done");
    }
    for (k, n) in &stats {
        println!("{n:8} {k}");
    }
    for (k, (a, b)) in &ranges {
        println!("part word {k:#04x} as f32 {a} ..= {b}");
    }
}

/// Survey: the Halo 1 root of `$H1_GROUP` (default `ligh`) over every
/// map: lengths, every reference by offset, and per word how often it is
/// nonzero with its range as a float.
#[test]
#[ignore]
fn halo1_group_word_survey() {
    let g = std::env::var("H1_GROUP").unwrap_or_else(|_| "ligh".into());
    let group = Group::new(g.as_bytes().try_into().unwrap());
    let mut lens: BTreeMap<usize, usize> = BTreeMap::new();
    let mut words: BTreeMap<usize, (usize, f32, f32, BTreeMap<u32, usize>)> = BTreeMap::new();
    let mut refs: BTreeMap<String, usize> = BTreeMap::new();
    let mut n = 0;
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == group) {
            let b = m.tag_data(t).unwrap();
            n += 1;
            *lens.entry(b.len()).or_default() += 1;
            let len = b.len().min(0x200);
            for at in (0..len.saturating_sub(16)).step_by(4) {
                if let Some(r) = TagRef::read(b, at)
                    && !r.id.is_none()
                    && let Ok(x) = m.tag(r.id)
                    && x.group == r.group
                {
                    *refs.entry(format!("{at:#05x} {}", r.group)).or_default() += 1;
                }
            }
            for at in (0..len).step_by(4).filter(|a| a + 4 <= len) {
                let w = u32_at(b, at);
                let e = words.entry(at).or_insert((0, f32::MAX, f32::MIN, BTreeMap::new()));
                if w != 0 {
                    e.0 += 1;
                    let f = f32::from_bits(w);
                    if f.is_finite() {
                        e.1 = e.1.min(f);
                        e.2 = e.2.max(f);
                    }
                    if e.3.len() < 6 {
                        *e.3.entry(w).or_default() += 1;
                    }
                }
            }
        }
    }
    println!("{n} {g} tags; lengths {lens:?}");
    for (k, v) in &refs {
        println!("ref {k}: {v}");
    }
    for (at, (nz, a, b, few)) in &words {
        if *nz > 0 {
            let few: Vec<String> = few.keys().map(|w| format!("{w:#x}")).collect();
            println!("{at:#05x}: nonzero {nz}, float {a} ..= {b}, first words {few:?}");
        }
    }
}

/// Survey: every reference of every Halo 1 `udlg` by offset, with the
/// last component of the names it holds there.
#[test]
#[ignore]
fn halo1_dialogue_survey() {
    let udlg = Group::new(b"udlg");
    let mut slots: BTreeMap<usize, BTreeMap<String, usize>> = BTreeMap::new();
    let mut lens: BTreeMap<usize, usize> = BTreeMap::new();
    for (_, m) in h1_maps() {
        for t in m.tags().iter().filter(|t| t.group == udlg) {
            let b = m.tag_data(t).unwrap();
            *lens.entry(b.len()).or_default() += 1;
            for at in (0..b.len().saturating_sub(16)).step_by(4) {
                if let Some(r) = TagRef::read(b, at)
                    && !r.id.is_none()
                    && let Ok(x) = m.tag(r.id)
                    && x.group == r.group
                {
                    let tail = x.name.rsplit('\\').next().unwrap_or("").to_owned();
                    *slots.entry(at).or_default().entry(format!("{} {tail}", r.group)).or_default() += 1;
                }
            }
        }
    }
    println!("lengths {lens:?}");
    for (at, names) in &slots {
        let mut v: Vec<_> = names.iter().collect();
        v.sort_by_key(|(_, c)| std::cmp::Reverse(**c));
        println!("{at:#05x}: {:?}", &v[..v.len().min(3)]);
    }
}

/// Survey: the Halo 2 base's `udlg` elements: each element's string and
/// the group of the tag its datum names, and the root's words.
#[test]
#[ignore]
fn halo2_dialogue_survey() {
    let mut m = h2_map(&std::env::var("H2_BASE").unwrap_or_else(|_| "05a_deltaapproach".into()));
    let base = Base::read(&mut m).unwrap();
    let strings = m.strings().to_vec();
    let text = |v: Option<&Val>| match v {
        Some(Val::String(Str::Index(i))) => strings
            .get((*i & 0xffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_default(),
        x => format!("{x:?}"),
    };
    let d = def(b"udlg");
    let mut names: BTreeMap<String, usize> = BTreeMap::new();
    let mut groups: BTreeMap<String, usize> = BTreeMap::new();
    let mut roots: BTreeMap<String, usize> = BTreeMap::new();
    let ids: Vec<TagId> = base.tags.iter().filter(|t| t.1 == FourCC(*b"udlg")).map(|t| t.0).collect();
    for id in &ids {
        let r = base.record(d, *id).unwrap();
        *roots.entry(format!("{:?} {:?} {:?} {:?}", r.at("unknown_0"), r.at("unknown_4"), r.at("unknown_8"), r.at("string_14"))).or_default() += 1;
        for e in block(&r, "vocalizations") {
            *names.entry(text(e.at("vocalization"))).or_default() += 1;
            if let Some(Val::Datum(Target::Id(t))) = e.at("sound") {
                let g = base.tags.iter().find(|x| x.0 == *t).map(|x| x.1.to_string());
                *groups.entry(format!("{g:?} {:?}", e.at("unknown_8"))).or_default() += 1;
            }
        }
    }
    println!("{} udlg", ids.len());
    for (k, v) in roots.iter().take(10) { println!("root {v}: {k}"); }
    for (k, v) in &groups { println!("datum {v}: {k}"); }
    let all: Vec<String> = names.keys().cloned().collect();
    println!("{}", all.join(" "));
    println!("{} distinct names", names.len());
}
