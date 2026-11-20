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
//!   attachment resolves to its converted tag at Halo 1's marker.

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
                assert_eq!(
                    r.at("loop_start_event"),
                    Some(&Val::Int(e.loop_start_event.into())),
                    "{name}"
                );
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
                    for k in ["beams", "accelerations"] {
                        assert!(block(x, k).is_empty(), "{name}: {k}");
                    }
                    // Halo 1's particles whose part converts
                    // (tests/particles_disc.rs checks them).
                    // and particle systems (`pctl` parts, a system per state).
                    let systems = block(x, "particle_systems");
                    let pctl = ev.parts.iter().filter(|p| p.group == Group::new(b"pctl")).count();
                    assert!(systems.len() <= ev.particles || pctl > 0, "{name}: particle systems");
                    for s in systems {
                        let (_, tag) = target(s, "particle");
                        let tag = tag.unwrap_or_else(|| panic!("{name}: null particle"));
                        assert_eq!(resolve(tree, tag), Some(FourCC(*b"prt3")), "{name}");
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
                let path = if got.group == FourCC(*b"vehi") {
                    "unit.obje.attachments"
                } else {
                    "object.attachments"
                };
                let att = block(&r, path);
                for a in att {
                    let (_, tag) = target(a, "type");
                    let g = resolve(tree, tag.expect("attachment")).expect("resolves");
                    assert!([*b"lsnd", *b"effe", *b"ligh", *b"cont"].contains(&g.0), "{name}: {g}");
                    let marker = text(a, "marker");
                    assert!(
                        h1a.iter().any(|x| {
                            let source = x.group.0.to_be_bytes();
                            x.marker == marker
                                && (source == g.0 || (&source == b"pctl" && &g.0 == b"effe"))
                        }),
                        "{name}: {marker}"
                    );
                }
                let kept = h1a
                    .iter()
                    .filter(|x| {
                        x.tag.is_some()
                            && [*b"lsnd", *b"effe", *b"ligh", *b"cont", *b"pctl"]
                                .contains(&x.group.0.to_be_bytes())
                    })
                    .count();
                if att.len() != kept {
                    println!(
                        "{name}: {} attachments from {kept} source elements",
                        att.len()
                    );
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
    let mut pairs: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let ids: Vec<TagId> = base.tags.iter().filter(|t| t.1 == FourCC(*b"udlg")).map(|t| t.0).collect();
    for id in &ids {
        let r = base.record(d, *id).unwrap();
        *roots.entry(format!("{:?} {:?} {:?} {:?}", r.at("unknown_0"), r.at("unknown_4"), r.at("unknown_8"), r.at("string_14"))).or_default() += 1;
        let tag_name = base.tags.iter().find(|x| x.0 == *id).map(|x| x.2.clone()).unwrap_or_default();
        println!("{tag_name}: string_14 {:?}, {} lines", text(r.at("string_14")), block(&r, "vocalizations").len());
        for e in block(&r, "vocalizations") {
            *names.entry(text(e.at("vocalization"))).or_default() += 1;
            if let Some(Val::Datum(Target::Id(t))) = e.at("sound") {
                let g = base.tags.iter().find(|x| x.0 == *t).map(|x| x.1.to_string());
                if let Some(n) = base.tags.iter().find(|x| x.0 == *t).map(|x| x.2.clone()) {
                    let tail = n.rsplit('\\').next().unwrap_or("").to_owned();
                    let parent = n.rsplit('\\').nth(1).unwrap_or("").to_owned();
                    *pairs
                        .entry(text(e.at("vocalization")))
                        .or_default()
                        .entry(format!("{parent}\\{tail}"))
                        .or_default() += 1;
                }
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
    // Sounds named otherwise than the vocalization they play for.
    for (v, ns) in &pairs {
        let other: Vec<_> = ns
            .iter()
            .filter(|(n, _)| n.rsplit('\\').next() != Some(v.as_str()))
            .collect();
        if !other.is_empty() {
            println!("{v}: {other:?}");
        }
    }
}

/// Every retail map's voice tables preserve the native indices and every
/// source slot. The source rows must resolve to the original voice's
/// sound even when that slot has no Halo 2 vocalization.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_dialogue_converts() {
    let mut sets: Vec<(&str, Vec<&str>)> = vec![(
        "05a_deltaapproach",
        h1map::disc::CAMPAIGN_LEVELS.to_vec(),
    )];
    for mp in ["coagulation", "beavercreek"] {
        sets.push((
            mp,
            h1convert::convert::MAPS
                .iter()
                .filter(|m| m.2 == mp)
                .map(|m| m.0)
                .collect(),
        ));
    }

    let weapon_lines = map::H1_WEAPON_KILL_LINES;
    let udlg = Group::new(b"udlg");
    let mut maps = 0;
    let mut multi_voice = 0;
    for (base_name, levels) in sets {
        let base = Base::read(&mut h2_map(base_name)).unwrap();
        assert!(!base.dialogue_names.is_empty(), "{base_name} has no adlg");
        assert_eq!(&base.dialogue_names[244], "chr_kllfoe");
        assert_eq!(&base.dialogue_names[253], "chr_deaddrvsh");
        for level in levels {
            let Some(m) = h1_level(level) else { continue };
            maps += 1;
            let mut c = Converter::new(&m, &base);
            let mut rep = Default::default();
            let mut voices = std::collections::HashMap::new();
            for tag in m.tags().iter().filter(|t| t.group == udlg) {
                let context = format!("{level} {}", tag.name);
                let id = c
                    .dialogue(tag, &mut rep)
                    .unwrap_or_else(|e| panic!("{context}: {e}"));
                assert!(voices.insert(tag.id, id).is_none());
                let src = h1::Dialogue::read(&m, tag).unwrap();
                let routed = h1objects::rebuild::dialogue_lines(&src);
                for &(_, offset) in weapon_lines {
                    if src.line(offset).is_some() {
                        assert!(src.line(0x360).is_some(), "{context}: missing killer cheer");
                    }
                }
                assert!(
                    src.line(0x360).is_some()
                        || src.lines.iter().all(|&(offset, _)| !map::h1_species_kill_line(offset)),
                    "{context}: species variants need a native killer cheer"
                );
                let record = read_back(&c.tree.tags[id.index()].1, &context);
                let rows = block(&record, "vocalizations");
                let (native, slots) = rows.split_at(base.dialogue_names.len());
                assert_eq!(slots.len(), src.lines.len(), "{context}");
                let assert_sound = |row: &Record, sound| {
                    let Some(Val::Datum(Target::Id(id))) = row.at("sound") else {
                        panic!("{context}: no sound");
                    };
                    assert_eq!(resolve(&c.tree, *id), Some(FourCC(*b"snd!")), "{context}");
                    assert_eq!(c.tree.sounds[id.index()].0, m.tag(sound).unwrap().name, "{context}");
                };
                for (row, name) in native.iter().zip(&base.dialogue_names) {
                    assert_eq!(&text(row, "vocalization"), name, "{context}: wrong native index");
                    if let Some((_, _, sound)) = routed.iter().find(|(voc, _, _)| *voc == name.as_str()) {
                        assert_sound(row, *sound);
                    } else {
                        assert_eq!(row.at("sound"), Some(&Val::Datum(Target::Null)), "{context} {name}");
                    }
                }
                for (row, &(offset, sound)) in slots.iter().zip(&src.lines) {
                    let slot = (offset / 0x10 - 1) as u32;
                    let marker = map::H1_DIALOGUE_SLOT | slot;
                    assert_eq!(
                        row.at("unknown_0"),
                        Some(&Val::Bytes(marker.to_le_bytes().to_vec())),
                        "{context} slot {slot}"
                    );
                    assert_sound(row, sound);
                }
            }
            for unit in m.tags().iter().filter(|t| t.group == Group::new(b"bipd")) {
                let selectors = h1::dialogue_variants(&m, m.tag_data(unit).unwrap()).unwrap();
                multi_voice += usize::from(selectors.len() > 1);
                for (_, voice) in selectors {
                    assert!(voices.contains_key(&voice), "{level} {}: missing voice {voice}", unit.name);
                }
            }
        }
    }
    assert_eq!(maps, 23);
    assert!(multi_voice > 0);
}

#[test]
#[ignore = "reads the Halo 1 and Halo 2 disc executables"]
fn automatic_damage_dialogue_has_retail_trigger_evidence() {
    let default_xbe = |var, fallback| {
        let mut iso =
            xiso::Xiso::open(BufReader::new(File::open(disc(var, fallback)).unwrap())).unwrap();
        let entry = iso
            .entries()
            .unwrap()
            .into_iter()
            .find(|entry| entry.path.eq_ignore_ascii_case("/default.xbe"))
            .unwrap();
        iso.read(&entry).unwrap()
    };
    let h1_bytes = default_xbe("H1_ISO", "halo1.iso");
    let h1 = xbe::Xbe::parse(&h1_bytes).unwrap();
    let start = 0x001e_eb70;
    let patterns = h1.bytes_at(start, 29 * 0x28).unwrap();
    for (category, offset) in [(3, 0x460)]
        .into_iter()
        .chain(map::H1_WEAPON_KILL_LINES.iter().copied())
    {
        let slot = (offset / 0x10 - 1) as u16;
        assert!(
            patterns.chunks_exact(0x28).any(|row| {
                // Kill stimulus, spoken by the attacker after a hostile kill.
                u16_at(row, 0) == 0
                    && u16_at(row, 4) == slot
                    && u16_at(row, 8) == 1
                    && u16_at(row, 0x1c) == 3
                    && u16_at(row, 0x24) == category
            }),
            "no weapon kill rule for {offset:#x}"
        );
    }
    assert_eq!(h1.bytes_at(0x0004_7293, 5).unwrap(), &[0xb8, 19, 0, 0, 0]);
    assert_eq!(h1.read_u32(0x001f_0b04).unwrap(), 2.0f32.to_bits());
    for actor in 0..16u16 {
        let record = h1.read_u32(0x0025_f2c8 + u32::from(actor) * 4).unwrap();
        let actual = u16_at(h1.bytes_at(record + 4, 2).unwrap(), 0);
        let class = map::H1DialogueClass::actor(actor);
        assert_eq!(class as u16, actual, "dialogue class for actor type {actor}");
        if let Some(offset) = class.hostile_kill_line() {
            let slot = (offset / 0x10 - 1) as u16;
            assert!(
                patterns.chunks_exact(0x28).any(|row| {
                    u16_at(row, 0) == 0
                        && u16_at(row, 4) == slot
                        && u16_at(row, 8) == 1
                        && u16_at(row, 0x1c) == 3
                        && u16_at(row, 0x20) == actual
                }),
                "no victim-specific kill rule for {offset:#x}"
            );
        }
    }

    let h2_bytes = default_xbe("H2_DISC", "halo2.iso");
    let h2 = xbe::Xbe::parse(&h2_bytes).unwrap();
    for (call, target) in [
        (0x000c_a45d, 0x0011_47e0),
        (0x0011_4a06, 0x0011_4520),
        (0x000c_a4ed, 0x001c_95d0),
        (0x001c_975c, 0x0020_ba60),
        (0x0020_bb49, 0x0020_d0c0),
    ] {
        let inst = h2.bytes_at(call, 5).unwrap();
        assert_eq!(inst[0], 0xe8, "no near call at {call:#x}");
        assert_eq!((call + 5).wrapping_add(u32_at(inst, 1)), target);
    }
}

/// Survey: Halo 1 `foot` tags: the root's block fields (count at each
/// offset), and every reference in the tag's bytes by offset with its
/// group, for the layout of the material effects.
#[test]
#[ignore]
fn halo1_foot_survey() {
    let foot = Group::new(b"foot");
    let want = std::env::var("H1_LEVELS").unwrap_or_else(|_| "a10".into());
    for (stem, m) in h1_maps() {
        if !want.split(',').any(|w| w == stem) {
            continue;
        }
        for t in m.tags().iter().filter(|t| t.group == foot) {
            let b = m.tag_data(t).unwrap();
            println!("{stem} {} len {:#x}", t.name, b.len());
            for at in (0..0x40.min(b.len())).step_by(4) {
                if let Some(k) = Block::read(b, at)
                    && k.count > 0
                    && k.count < 0x100
                {
                    println!("  root {at:#x}: count {} address {:#x}", k.count, k.address);
                }
            }
            let mut last = 0;
            for at in (0..b.len().saturating_sub(16)).step_by(4) {
                if let Some(r) = TagRef::read(b, at)
                    && (r.group == Group::new(b"effe") || r.group == Group::new(b"snd!"))
                {
                    let n = if r.id.is_none() {
                        "-".to_owned()
                    } else {
                        m.tag(r.id).map(|x| x.name.rsplit('\\').next().unwrap_or("").to_owned()).unwrap_or_default()
                    };
                    println!("  {at:#06x} (+{:#x}) {} {n}", at - last, r.group);
                    last = at;
                }
            }
            for at in (0..b.len().saturating_sub(12)).step_by(4) {
                if let Some(k) = Block::read(b, at)
                    && k.count > 0
                    && k.count < 0x40
                    && k.address >> 28 == 8
                    && at >= 0x40
                {
                    println!("  block? {at:#x}: count {}", k.count);
                }
            }
        }
    }
}

/// Survey: the Halo 2 base's `foot` tags: per effect kind (index), its
/// sounds and effects elements with the names and material strings.
#[test]
#[ignore]
fn halo2_foot_survey() {
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
    let name = |v: Option<&Val>| match v {
        Some(Val::Ref { target: Target::Id(t), .. }) => base
            .tags
            .iter()
            .find(|x| x.0 == *t)
            .map(|x| format!("{} {}", x.1, x.2))
            .unwrap_or_else(|| format!("{t:?}")),
        x => format!("{x:?}"),
    };
    let d = def(b"foot");
    for (id, g, n, _) in base.tags.iter() {
        if *g != FourCC(*b"foot") {
            continue;
        }
        let r = base.record(d, *id).unwrap();
        let kinds = block(&r, "effects");
        println!("{n}: {} kinds", kinds.len());
        for (k, e) in kinds.iter().enumerate() {
            let s = block(e, "sounds");
            let f = block(e, "effects");
            println!("  kind {k}: {} sounds, {} effects, +0 {:?}", s.len(), f.len(), e.at("unknown_0"));
            for x in s.iter().take(6) {
                println!("    sound {} [{}] +8 {:?} +14 {:?}", name(x.at("tag_0")), text(x.at("material")), x.at("unknown_8"), x.at("unknown_14"));
            }
            for x in f.iter().take(6) {
                println!("    effect {} / {} [{}] +14 {:?}", name(x.at("tag_0")), name(x.at("tag_8")), text(x.at("material")), x.at("unknown_14"));
            }
        }
    }
}

/// Every Halo 1 `foot` of the ten campaign levels and the multiplayer
/// maps converts from the definition: Halo 1's kinds at their index,
/// each element's material a global material of the base at the index
/// it gives, each tag a converted sound or effect; it reads back with no
/// pinned bytes, and every rebuilt unit naming a `foot` names the
/// converted one.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn every_foot_converts() {
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
    let foot = Group::new(b"foot");
    let (mut feet, mut sounds, mut effects, mut units) = (0usize, 0usize, 0usize, 0usize);
    for (base_name, levels) in sets {
        let mut base_map = h2_map(base_name);
        let base = Base::read(&mut base_map).unwrap();
        for level in levels {
            let Some(m) = h1_level(&level) else { continue };
            let mut c = Converter::new(&m, &base);
            let mut rep = Default::default();
            let mut got = Vec::new();
            for t in m.tags().iter().filter(|t| t.group == foot) {
                let id = c.foot(t, &mut rep).unwrap_or_else(|e| panic!("{level} {}: {e}", t.name));
                got.push((t, id));
            }
            let mut rebuilt = Vec::new();
            for t in m.tags().iter().filter(|t| {
                t.group == Group::new(b"bipd") || t.group == Group::new(b"vehi")
            }) {
                let offset = if t.group == Group::new(b"vehi") { 0x3d0 } else { 0x38c };
                let named = h1::reference(&m, m.tag_data(t).unwrap(), offset);
                if let Ok(id) = c.object(t) {
                    rebuilt.push((t, id, named));
                }
            }
            let tree = &c.tree;
            for (t, id) in &got {
                let name = format!("{level} {}", t.name);
                let f = h1::Foot::read(&m, t).unwrap();
                let built = &tree.tags[id.0 as usize & 0xffff].1;
                assert_eq!(built.group, FourCC(*b"foot"), "{name}");
                let r = read_back(built, &name);
                let kinds = block(&r, "effects");
                assert_eq!(kinds.len(), f.kinds.len().min(6), "{name}");
                for k in kinds {
                    for (list, want) in [("sounds", "snd!"), ("effects", "effe")] {
                        for e in block(k, list) {
                            let mat = text(e, "material");
                            let Some(Val::Int(i)) = e.at("material_index") else { panic!() };
                            assert_eq!(base.materials.get(*i as usize), Some(&mat), "{name}");
                            let path = if list == "sounds" { "sound" } else { "effect" };
                            let (_, tag) = target(e, path);
                            let tag = tag.unwrap_or_else(|| panic!("{name}: null {path}"));
                            assert_eq!(resolve(tree, tag), Some(FourCC(*want.as_bytes().first_chunk().unwrap())), "{name}");
                            if list == "sounds" { sounds += 1 } else { effects += 1 }
                        }
                    }
                }
                feet += 1;
            }
            for (t, id, named) in &rebuilt {
                let b = &tree.tags[id.0 as usize & 0xffff].1;
                let Some(n) = named else { continue };
                let r = defs::from_built(def(&b.group.0), &b.root).unwrap();
                let field = if t.group == Group::new(b"vehi") { "foot_ref_294" } else { h1objects::convert::FOOT_FIELD };
                let (_, f) = target(&r, field);
                let want = got.iter().find(|g| g.0.id == *n).map(|g| g.1);
                assert_eq!(f, want, "{level} {}: material effects", t.name);
                units += 1;
            }
        }
    }
    println!("{feet} foot tags, {sounds} sound and {effects} effect elements; {units} rebuilt units name theirs");
}

/// The native attached effect references the source model marker and emits
/// even though the Halo 1 system starts with an empty particle pool.
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn entry_lifepod_burns_at_its_halo1_marker_and_rate() {
    use h1objects::particles::{self, TICK};
    use tags::effects::function::Function;
    use tags::effects::h1_attachment::{PointPhysics, population_word};

    let m = h1_level("a30").unwrap();
    let base = Base::read(&mut h2_map("05a_deltaapproach")).unwrap();
    let vehicle = m
        .find(
            Group::new(b"vehi"),
            "vehicles\\lifepod_entry\\lifepod_entry",
        )
        .unwrap();
    let source = h1::attachments(&m, m.tag_data(vehicle).unwrap()).unwrap();
    let burn = source
        .iter()
        .find(|a| a.group == Group::new(b"pctl"))
        .unwrap();
    assert_eq!(burn.marker, "burn");
    let pctl = m.tag(burn.tag.unwrap()).unwrap();
    let types = particles::particle_system_types(&m, pctl).unwrap();
    assert_eq!(types.len(), 1);
    let ty = &types[0];
    assert_eq!(ty.initial_count, 0);
    assert_eq!(ty.emitter_states.len(), 1);
    let emitter = &ty.emitter_states[0];
    assert_eq!(emitter.particles_per_second, 30.0);

    let mut c = Converter::new(&m, &base);
    let id = c.object(vehicle).unwrap();
    let vehi = read_back(&c.tree.tags[id.index()].1, &vehicle.name);
    let model = target(&vehi, "unit.obje.model").1.unwrap();
    let hlmt = &c.tree.tags[model.index()].1;
    let hlmt = defs::from_built(def(b"hlmt"), &hlmt.root).unwrap();
    let mode = target(&hlmt, "render_model").1.unwrap();
    let mode = &c.tree.tags[mode.index()].1;
    let mode = defs::from_built(def(b"mode"), &mode.root).unwrap();
    assert!(
        block(&mode, "marker_groups")
            .iter()
            .any(|r| text(r, "name") == burn.marker)
    );

    let attachments = block(&vehi, "unit.obje.attachments");
    let att = attachments
        .iter()
        .find(|a| text(a, "marker") == burn.marker)
        .unwrap();
    let id = target(att, "type").1.unwrap();
    assert_eq!(resolve(&c.tree, id), Some(FourCC(*b"effe")));
    let (name, effect) = &c.tree.tags[id.index()];
    assert!(name.starts_with(&pctl.name));
    let effect = read_back(effect, name);
    assert_eq!(effect.at("loop_start_event"), Some(&Val::Int(0)));
    assert_eq!(text(&block(&effect, "locations")[0], "marker"), burn.marker);
    let events = block(&effect, "events");
    assert_eq!(events.len(), 2);
    assert_eq!(emitter.minimum_population, 10.0);
    for (event, bounds) in events.iter().zip([emitter.duration, emitter.transition]) {
        assert_eq!(event.at("duration_bounds"), Some(&Val::floats(&bounds)));
        assert_eq!(
            event.at("attachment_population"),
            Some(&Val::Int(
                population_word([emitter.minimum_population; 2], false).into()
            )),
        );
    }
    let event = &events[0];
    let states: Vec<_> = ty.states.iter().filter(|s| s.lasts()).collect();
    let systems = block(event, "particle_systems");
    assert_eq!(systems.len(), states.len());
    assert_eq!(systems.len(), 2);
    for (system, state) in systems.iter().zip(states) {
        assert_eq!(system.at("location"), Some(&Val::Int(0)));
        let particle = target(system, "particle").1.unwrap();
        let (name, particle) = &c.tree.tags[particle.index()];
        assert_eq!(particle.group, FourCC(*b"prt3"));
        assert!(name.ends_with(&state.name));
        let emitter_record = &block(system, "emitters")[0];
        let movement = target(emitter_record, "movement").1.unwrap();
        assert_eq!(resolve(&c.tree, movement), Some(FourCC(*b"pmov")));
        let (name, tag) = &c.tree.tags[movement.index()];
        let movement = read_back(tag, name);
        assert_eq!(movement.at("movements[0].kind"), Some(&Val::Int(1)));
        let parameters = block(&block(&movement, "movements")[0], "parameters");
        let source = m.tag(state.physics.unwrap()).unwrap();
        let bytes = m.tag_data(source).unwrap();
        let f32_at = |i: usize| f32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
        let mut extended = [[0; 4]; 2];
        for (i, parameter) in parameters.iter().enumerate() {
            let Some(Val::Data(curve)) = parameter.at("function") else {
                panic!("missing collision curve");
            };
            assert_eq!(
                Function::parse(curve).unwrap().range,
                [f32_at(0x2c + 4 * i); 2]
            );
            for (word, bytes) in extended[i].iter_mut().zip(curve[12..].chunks_exact(4)) {
                *word = u32::from_le_bytes(bytes.try_into().unwrap());
            }
        }
        let physical = PointPhysics::from_words(extended).unwrap();
        assert_eq!(physical.mass, f32_at(4));
        assert_eq!(physical.drag, [f32_at(0x24), f32_at(0x28)]);
        assert!(physical.acceleration[0] > 0.0);
        let Some(Val::Data(rate)) = emitter_record.at("property_8.function") else {
            panic!("particle rate");
        };
        let rate = f32::from_le_bytes(rate[4..8].try_into().unwrap());
        assert_eq!(rate, emitter.particles_per_second * TICK);
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn placed_object_builders_keep_halo1_attachments() {
    let m = h1_level("b30").unwrap();
    let base = Base::read(&mut h2_map("05a_deltaapproach")).unwrap();
    let mut c = Converter::new(&m, &base);
    for (source_group, name, object_path, marker, group, scale) in [
        (
            b"bipd",
            "characters\\cyborg\\cyborg",
            "unit.obje",
            "flashlight",
            b"ligh",
            "a_out",
        ),
        (
            b"weap",
            "weapons\\assault rifle\\assault rifle",
            "item.obje",
            "primary trigger",
            b"ligh",
            "b_out",
        ),
        (
            b"proj",
            "weapons\\assault rifle\\bullet",
            "object",
            "",
            b"cont",
            "",
        ),
        (
            b"scen",
            "scenery\\emitters\\condensation\\condensation",
            "object",
            "smoker",
            b"effe",
            "",
        ),
        (
            b"mach",
            "levels\\b30\\devices\\interior tech objects\\holo beam tube\\holo beam tube",
            "devi.obje",
            "point light",
            b"ligh",
            "",
        ),
        (
            b"ctrl",
            "levels\\b30\\devices\\interior tech objects\\holo control\\holo control",
            "devi.obje",
            "holo light",
            b"ligh",
            "",
        ),
        (
            b"lifi",
            "levels\\b30\\devices\\light fixtures\\light fixture wall3\\light fixture wall3",
            "devi.obje",
            "point light01",
            b"ligh",
            "",
        ),
        (
            b"ssce",
            "sound\\sfx\\ambience\\b30\\waves",
            "object",
            "",
            b"lsnd",
            "",
        ),
    ] {
        let t = m
            .find(Group::new(source_group), name)
            .unwrap_or_else(|| panic!("{name}"));
        let id = c.object(t).unwrap_or_else(|e| panic!("{name}: {e}"));
        let object = &c.tree.tags[id.index()].1;
        let record = defs::from_built(def(&object.group.0), &object.root).unwrap();
        let att = block(&record, &format!("{object_path}.attachments"))
            .iter()
            .find(|a| {
                text(a, "marker") == marker
                    && target(a, "type").1.and_then(|id| resolve(&c.tree, id))
                        == Some(FourCC(*group))
            })
            .unwrap_or_else(|| panic!("{name}: {marker}"));
        let (_, id) = target(att, "type");
        assert_eq!(
            resolve(&c.tree, id.unwrap()),
            Some(FourCC(*group)),
            "{name}"
        );
        assert_eq!(text(att, "string_10"), scale, "{name}");
    }
}
