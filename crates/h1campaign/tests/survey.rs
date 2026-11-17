//! Evidence surveys behind the converter's choices, from the owner's
//! discs at run time. They print, and assert nothing about retail data:
//!
//! - `survey_bases`: per Halo 2 campaign level, which `h1objects`
//!   templates and a30 characters it lacks, and its tag memory (the base
//!   choice, docs/game/campaigns.md §9);
//! - `find_named_blocks`: Halo 1 scenario blocks of printable names (the
//!   trigger volume and object name offsets);
//! - `ai_and_name_literal_encoding`: how retail Halo 2 scripts encode
//!   object, trigger volume, AI and enum literals (the hook values);
//! - `base_memory_and_vehicles`, `base_neighbours`, `base_scenery`,
//!   `bipeds`: per base, its tag memory split, scenario relocation,
//!   vehicles, scenery and bipeds (base choice and templates);
//! - `h1_ai_sizes`, `squad_flag_bits`, `ai_place_counts`: Halo 1's firing
//!   positions, which squads spawn at the start in either game, and the
//!   count retail `ai_place` calls pass (docs/formats/h1-ai.md, Spawning).
use std::fs::File;
use std::io::{BufReader, Cursor};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

#[test]
#[ignore = "reads the discs"]
fn survey_bases() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let e = d.map("a30").unwrap().clone();
    let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
    println!("a30 bsps: {}", h1.structure_bsps().unwrap().len());
    let ai = h1ai::h1::H1Ai::read(&h1).unwrap();
    let plan = h1ai::convert::convert(&ai, &Default::default());
    let chars: Vec<&str> = plan.characters.iter().map(|c| c.h2_character).collect();
    let mut groups = std::collections::BTreeMap::<String, usize>::new();
    for t in h1.tags() {
        *groups.entry(t.group.to_string()).or_default() += 1;
    }
    println!("a30 groups: {groups:?}");
    for t in h1
        .tags()
        .iter()
        .filter(|t| ["weap", "eqip", "vehi", "bipd"].contains(&t.group.to_string().as_str()))
    {
        println!(
            "  a30 {} {} -> {:?}",
            t.group,
            t.name,
            h1objects::convert::template_for(
                &t.group.to_string().as_bytes().try_into().unwrap(),
                &t.name
            )
        );
    }
    for name in [
        "03a_oldmombasa",
        "03b_newmombasa",
        "05a_deltaapproach",
        "05b_deltatowers",
        "08a_deltacliffs",
        "08b_deltacontrol",
        "01b_spacestation",
    ] {
        let e = iso
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.path.ends_with(&format!("/{name}.map")))
            .unwrap();
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let names: Vec<(String, String)> = m
            .tags()
            .iter()
            .map(|t| (t.group.to_string(), t.name.clone().unwrap_or_default()))
            .collect();
        let has = |g: &str, n: &str| names.iter().any(|(gg, nn)| gg == g && nn == n);
        let missing_t: Vec<&str> = h1objects::convert::TEMPLATES
            .iter()
            .filter(|(g, _, n)| !has(std::str::from_utf8(*g).unwrap(), n))
            .map(|t| t.2)
            .collect();
        let missing_c: Vec<&&str> = chars.iter().filter(|c| !has("char", c)).collect();
        let h = m.header().clone();
        let scnr = m.scenario().unwrap().clone();
        let b = m.read_tag_bytes(scnr.id).unwrap();
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        println!(
            "{name}: tags {} meta {:#x} bsps {} skies {} weapons {} starts {} tv {} squads? scnr {:#x}",
            names.len(),
            h.meta_size,
            w(0x210),
            w(8),
            w(0x90),
            w(0x100),
            w(0x108),
            b.len()
        );
        println!("   missing templates: {missing_t:?}");
        println!("   missing chars: {missing_c:?}");
        if w(0x90) > 1 {
            let at = (w(0x94) - scnr.location.unwrap().address) as usize;
            for k in 0..3usize.min(w(0x90) as usize) {
                let e = &b[at + k * 0x54..at + (k + 1) * 0x54];
                let words: Vec<String> = e[0x24..0x54]
                    .chunks(4)
                    .map(|c| format!("{:08x}", u32::from_le_bytes(c.try_into().unwrap())))
                    .collect();
                println!("   weapon {k} tail: {}", words.join(" "));
            }
        }
    }
}

#[test]
#[ignore = "reads the discs"]
fn find_named_blocks() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let e = d.map("a30").unwrap().clone();
    let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
    let scnr = h1.tag_data(h1.scenario()).unwrap();
    let space = h1.tag_space();
    let printable = |b: &[u8]| {
        let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
        end > 2 && b[..end].iter().all(|c| c.is_ascii_graphic() || *c == b' ')
    };
    for at in (0..0x5c0).step_by(4) {
        for stride in [0x24usize, 0x60, 0x40, 0x5c, 0x20] {
            let Ok(els) = space.block_at(scnr, at, stride) else {
                continue;
            };
            if els.is_empty() || els.len() > 5000 {
                continue;
            }
            for off in [0usize, 4] {
                if off + 32 > stride {
                    continue;
                }
                if els.iter().all(|e| printable(&e[off..off + 32])) {
                    let names: Vec<String> = els
                        .iter()
                        .take(3)
                        .map(|e| {
                            let end = e[off..off + 32].iter().position(|&c| c == 0).unwrap_or(32);
                            String::from_utf8_lossy(&e[off..off + end]).into_owned()
                        })
                        .collect();
                    println!(
                        "{at:#x} stride {stride:#x} name at +{off}: {} {:?}",
                        els.len(),
                        names
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "reads the discs"]
fn ai_and_name_literal_encoding() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso.find("default.xbe").unwrap();
    let bytes = iso.read(&e).unwrap();
    let c = hs::Catalog::from_xbe(&xbe::Xbe::parse(&bytes).unwrap()).unwrap();
    let ty = |n: &str| c.types().iter().position(|t| t == n).unwrap() as u16;
    for name in ["05a_deltaapproach", "03a_oldmombasa"] {
        let e = iso
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.path.ends_with(&format!("/{name}.map")))
            .unwrap();
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let s = hs::Scenario::read(&mut m).unwrap();
        let scnr = m.scenario().unwrap().clone();
        let b = m.read_tag_bytes(scnr.id).unwrap();
        let view = tags::geometry::TagView::new(&b, scnr.location.unwrap().address);
        let ai = h1ai::h2::ScenarioAi::read(&view).unwrap();
        let mut kinds = std::collections::BTreeMap::<String, Vec<String>>::new();
        for n in s.nodes() {
            if n.flags() & 1 == 0 {
                continue;
            }
            let t = c
                .types()
                .get(n.value_type() as usize)
                .cloned()
                .unwrap_or_default();
            if ![
                "ai",
                "object_name",
                "trigger_volume",
                "cutscene_flag",
                "object",
                "unit",
                "vehicle",
                "short",
                "team",
                "game_difficulty",
                "navpoint",
                "cutscene_camera_point",
                "device_group",
                "sound",
                "looping_sound",
            ]
            .contains(&t.as_str())
            {
                continue;
            }
            let tok = s.string(n.string_offset()).unwrap_or("?").to_owned();
            let v = kinds.entry(t.clone()).or_default();
            if v.len() < 12 {
                let note = if t == "ai" {
                    let sq = ai.squads.iter().position(|q| q.name == tok);
                    let gr = ai.squad_groups.iter().position(|q| q.name == tok);
                    format!(" squad {sq:?} group {gr:?}")
                } else {
                    String::new()
                };
                v.push(format!("{tok}={:#x}{note}", n.data()));
            }
        }
        let _ = ty;
        println!("{name}:");
        for (k, v) in kinds {
            println!("  {k}: {v:?}");
        }
    }
}

#[test]
#[ignore = "reads the discs"]
fn map_file_sizes() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let mut sizes: Vec<(u32, String)> = iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| e.path.ends_with(".map"))
        .map(|e| (e.size, e.path))
        .collect();
    sizes.sort();
    for (s, p) in sizes.iter().rev().take(6) {
        println!("{s:>10} {p}");
    }
    println!("{:?}", sizes.iter().find(|(_, p)| p.contains("05a_")));
}

/// Per Halo 2 campaign map: tag memory split (index, BSP space,
/// metadata), the scenario's size and whether it relocates, and which
/// vehicles it carries (templates for Halo 1's pelican and dropship).
#[test]
#[ignore = "reads the discs"]
fn base_memory_and_vehicles() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let entries: Vec<_> = iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| {
            e.path.ends_with(".map")
                && e.path.chars().rev().nth(4).is_some()
                && e.path
                    .rsplit('/')
                    .next()
                    .is_some_and(|f| f.as_bytes()[0].is_ascii_digit())
        })
        .collect();
    for e in entries {
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let set = map::build::TagSet::from_map(&mut m).unwrap();
        let p = set.pinned.as_ref().unwrap();
        let scnr = m.scenario().unwrap().clone();
        let r = tags::schema::Relocator::new(tags::schema::Schema::builtin(), &mut m).unwrap();
        let check = r.check(scnr.id).map(|_| ()).map_err(|d| format!("{d:?}"));
        let movable = r.movable().contains(&scnr.id);
        let vehicles: Vec<String> = m
            .tags()
            .iter()
            .filter(|t| t.group == map::FourCC(*b"vehi"))
            .filter_map(|t| t.name.clone())
            .collect();
        println!(
            "{}: memory {:#x} = index {:#x} + bsp {:#x} + meta {:#x}; scnr {:#x} check {check:?} movable {movable}",
            e.path,
            m.header().meta_size,
            p.index_size,
            p.bsp_size,
            p.metadata_size,
            scnr.location.unwrap().size,
        );
        println!("   vehicles: {vehicles:?}");
    }
}

/// The base's metadata around its scenario and sound gestalt: which tags
/// sit next to the two biggest holes a conversion leaves.
#[test]
#[ignore = "reads the discs"]
fn base_neighbours() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with("/05a_deltaapproach.map"))
        .unwrap();
    let m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let space = m.metadata_space().unwrap();
    let mut tags: Vec<_> = m
        .tags()
        .iter()
        .filter_map(|t| {
            let l = t.location?;
            (l.space == space && l.size > 0).then(|| (l.address, l.size, t.group, t.name.clone()))
        })
        .collect();
    tags.sort_by_key(|t| t.0);
    println!(
        "metadata {:#x}..{:#x}",
        space.virtual_base,
        space.virtual_base + space.size
    );
    for (i, t) in tags.iter().enumerate() {
        if [b"scnr", b"ugh!", b"matg"].contains(&&t.2.0) {
            for u in &tags[i.saturating_sub(3)..(i + 4).min(tags.len())] {
                println!("  {:#x} {:#8x} {} {:?}", u.0, u.1, u.2, u.3);
            }
            println!();
        }
    }
    let mut by: std::collections::BTreeMap<String, (usize, u32)> = Default::default();
    for t in &tags {
        let e = by.entry(t.2.to_string()).or_default();
        e.0 += 1;
        e.1 += t.1;
    }
    let mut v: Vec<_> = by.into_iter().collect();
    v.sort_by_key(|x| std::cmp::Reverse(x.1.1));
    println!("{:x?}", &v[..20]);
}

/// Per Halo 1 level: encounters, squads and firing positions (the zones
/// block's size).
#[test]
#[ignore = "reads the discs"]
fn h1_ai_sizes() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let e = d.map(level).unwrap().clone();
        let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
        let ai = h1ai::h1::H1Ai::read(&h1).unwrap();
        let fps: usize = ai.encounters.iter().map(|e| e.firing_positions.len()).sum();
        let max = ai
            .encounters
            .iter()
            .map(|e| e.firing_positions.len())
            .max()
            .unwrap_or(0);
        let mut uniq = std::collections::HashSet::new();
        for e in &ai.encounters {
            for f in &e.firing_positions {
                uniq.insert(f.position.map(f32::to_bits));
            }
        }
        println!(
            "{level}: {} encounters, {fps} firing positions ({} distinct, most in one {max})",
            ai.encounters.len(),
            uniq.len()
        );
    }
}

/// Squad flag bits on retail Halo 2 campaign maps, and encounter flag bits
/// on Halo 1's (which squads spawn at the start).
#[test]
#[ignore = "reads the discs"]
fn squad_flag_bits() {
    let bits = |v: &mut [usize; 32], f: u32| {
        for (b, n) in v.iter_mut().enumerate() {
            *n += usize::from(f >> b & 1 == 1);
        }
    };
    let show = |v: &[usize; 32]| -> String {
        v.iter()
            .enumerate()
            .filter(|(_, n)| **n > 0)
            .map(|(b, n)| format!("bit{b}:{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    for e in iso.entries().unwrap() {
        let f = e.path.rsplit('/').next().unwrap().to_string();
        if !f.ends_with(".map") || !f.as_bytes()[0].is_ascii_digit() {
            continue;
        }
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let s = m.scenario().unwrap().clone();
        let b = m.read_tag_bytes(s.id).unwrap();
        let view = tags::geometry::TagView::new(&b, s.location.unwrap().address);
        let ai = h1ai::h2::ScenarioAi::read(&view).unwrap();
        let mut v = [0usize; 32];
        for q in &ai.squads {
            bits(&mut v, q.flags);
        }
        println!("{f}: {} squads; {}", ai.squads.len(), show(&v));
    }
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let e = d.map(level).unwrap().clone();
        let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
        let ai = h1ai::h1::H1Ai::read(&h1).unwrap();
        let mut v = [0usize; 32];
        let mut sq = [0usize; 32];
        for e in &ai.encounters {
            bits(&mut v, e.flags);
            for s in &e.squads {
                bits(&mut sq, s.flags);
            }
        }
        println!(
            "{level}: {} encounters; {} | squads {}",
            ai.encounters.len(),
            show(&v),
            show(&sq)
        );
    }
}

/// What retail Halo 2 scripts pass as `ai_place`'s count (the adapter's
/// default for Halo 1's one-argument calls).
#[test]
#[ignore = "reads the discs"]
fn ai_place_counts() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let x = iso.find("default.xbe").unwrap();
    let xbe = iso.read(&x).unwrap();
    let mut catalog = hs::Catalog::from_xbe(&xbe::Xbe::parse(&xbe).unwrap()).unwrap();
    let mut scenarios = Vec::new();
    for e in iso.entries().unwrap() {
        let f = e.path.rsplit('/').next().unwrap().to_string();
        if !f.ends_with(".map") || !f.as_bytes()[0].is_ascii_digit() {
            continue;
        }
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let s = hs::Scenario::read(&mut m).unwrap();
        catalog.observe(&s).unwrap();
        scenarios.push((f, s));
    }
    let mut counts = std::collections::BTreeMap::<String, usize>::new();
    let mut one_arg = 0;
    for (f, s) in &scenarios {
        let src = s.decompile(&catalog, None).unwrap().source;
        for (i, _) in src.match_indices("(ai_place ") {
            let rest = &src[i + 10..];
            let end = rest.find(')').unwrap_or(rest.len());
            let args: Vec<&str> = rest[..end].split_whitespace().collect();
            match args.get(1) {
                Some(c) => *counts.entry(c.to_string()).or_default() += 1,
                None => one_arg += 1,
            }
        }
        let _ = f;
    }
    println!("ai_place second argument: {counts:?}; with one argument shown: {one_arg}");
}

/// Scenery on the base: candidates for Halo 1 scenery's template (one
/// whose model has no physics model, so a converted model never inherits
/// physics built for other nodes).
#[test]
#[ignore = "reads the discs"]
fn base_scenery() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with("/05a_deltaapproach.map"))
        .unwrap();
    let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let base = h1objects::base::Base::read(&mut m).unwrap();
    let mut n = 0;
    for t in m.tags().iter().filter(|t| t.group == map::FourCC(*b"scen")) {
        let name = t.name.clone().unwrap_or_default();
        let root = base.root(t.id).unwrap();
        let hlmt = base.reference(&root, 0x34);
        let (phmo, coll) = match hlmt {
            Some(h) => {
                let r = base.root(h).unwrap();
                (
                    base.reference(&r, 0x20).is_some(),
                    base.reference(&r, 0x8).is_some(),
                )
            }
            None => (false, false),
        };
        n += 1;
        if n <= 40 {
            println!("{name}: root {:#x} phmo {phmo} coll {coll}", root.len());
        }
    }
    println!("{n} scenery tags");
}

/// Bipeds the base carries (templates for Halo 1's placed bipeds), and the
/// biped palettes of Halo 1's levels.
#[test]
#[ignore = "reads the discs"]
fn bipeds() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with("/05a_deltaapproach.map"))
        .unwrap();
    let m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
    let b: Vec<_> = m
        .tags()
        .iter()
        .filter(|t| t.group == map::FourCC(*b"bipd"))
        .filter_map(|t| t.name.clone())
        .collect();
    println!("05a bipeds: {b:?}");
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let mut all = std::collections::BTreeSet::new();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let e = d.map(level).unwrap().clone();
        let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
        for t in h1
            .tags()
            .iter()
            .filter(|t| t.group == h1map::Group::new(b"bipd"))
        {
            all.insert(t.name.clone());
        }
    }
    println!("Halo 1 bipeds: {all:?}");
}
