//! Evidence surveys behind the converter's choices, from the owner's
//! discs at run time. They print, and assert nothing about retail data:
//!
//! - `survey_bases`: per Halo 2 campaign level, which `h1objects`
//!   templates and a30 characters it lacks, and its tag memory (the base
//!   choice, docs/halo1/campaign.md §9);
//! - `find_named_blocks`: Halo 1 scenario blocks of printable names (the
//!   trigger volume and object name offsets);
//! - `ai_and_name_literal_encoding`: how retail Halo 2 scripts encode
//!   object, trigger volume, AI and enum literals (the hook values);
//! - `base_memory_and_vehicles`, `base_neighbours`, `base_scenery`,
//!   `bipeds`: per base, its tag memory split, scenario relocation,
//!   vehicles, scenery and bipeds (base choice and templates);
//! - `h1_ai_sizes`, `squad_flag_bits`, `ai_place_counts`: Halo 1's firing
//!   positions, which squads spawn at the start in either game, and the
//!   count retail `ai_place` calls pass (docs/halo1/ai.md, Spawning);
//! - `per_bsp_scenario_blocks`, `per_bsp_blocks_on_05a`: the scenario
//!   blocks retail keeps one element per BSP of, and their contents;
//! - `donor_survey`: which retail map holds each character and object 05a
//!   lacks, and whether `tags::schema` copies its tree;
//! - `h1_bsp_switch_volumes`, `h2_bsp_switch_volumes`: where each game
//!   keeps its BSP switch trigger volumes;
//! - `lifepod_use`: how Halo 1's scripts and placements use the lifepods
//!   (their template by role, docs/halo1/objects.md);
//! - `h1_placement_flags`, `h1_initial_encounters`: which placements
//!   Halo 1 leaves for the scripts to create, and which encounters it
//!   creates at load (docs/halo1/campaign.md §9, Cutscenes).
use std::fs::File;
use std::io::{BufReader, Cursor};

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
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
    let plan = h1ai::convert::convert(&ai, &Default::default()).unwrap();
    let chars: Vec<&str> = plan
        .characters
        .iter()
        .map(|c| c.h2_character.as_str())
        .collect();
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
        // Squads with no starting locations: spawned into by vehicles,
        // migrations and command scripts, never placed themselves.
        let empty = ai
            .squads
            .iter()
            .filter(|q| q.starting_locations.is_empty())
            .count();
        println!(
            "{f}: {} squads ({empty} with no starting locations); {}",
            ai.squads.len(),
            show(&v)
        );
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

/// Scenario root blocks with one element per structure BSP on every retail
/// campaign map (the per-BSP data an added BSP needs an element of), and
/// their element size.
#[test]
#[ignore = "reads the discs"]
fn per_bsp_scenario_blocks() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let schema = tags::schema::Schema::builtin();
    let root = &schema.groups[&map::FourCC(*b"scnr")];
    let mut always: Option<std::collections::BTreeSet<u32>> = None;
    for e in iso.entries().unwrap() {
        let f = e.path.rsplit('/').next().unwrap().to_string();
        if !f.ends_with(".map") || !f.as_bytes()[0].is_ascii_digit() {
            continue;
        }
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let s = m.scenario().unwrap().clone();
        let b = m.read_tag_bytes(s.id).unwrap();
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let n = w(0x210);
        let hits: std::collections::BTreeSet<u32> = root
            .fields
            .iter()
            .filter(|(_, f)| matches!(f, tags::schema::Field::Block(c) if c.size > 1))
            .filter(|(o, _)| **o != 0x210 && w(**o as usize) == n)
            .map(|(o, _)| *o)
            .collect();
        println!("{f}: {n} bsps; fields with {n} elements: {:x?}", hits);
        always = Some(match always {
            None => hits,
            Some(a) => a.intersection(&hits).copied().collect(),
        });
    }
    let always = always.unwrap();
    for o in &always {
        if let Some(tags::schema::Field::Block(c)) = root.fields.get(o) {
            let kids: Vec<String> = c
                .fields
                .iter()
                .filter_map(|(k, f)| match f {
                    tags::schema::Field::Block(g) => Some(format!("{k:#x}:{:#x}", g.size)),
                    _ => None,
                })
                .collect();
            println!(
                "always per-BSP: {o:#x} element {:#x}, children {kids:?}",
                c.size
            );
        }
    }
}

/// 05a's two per-BSP scenario blocks (`0x290`, `0x388`): each element's
/// words, its children's counts and first elements, against the BSP's
/// cluster count.
#[test]
#[ignore = "reads the discs"]
fn per_bsp_blocks_on_05a() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    for name in ["05a_deltaapproach", "03a_oldmombasa"] {
        let e = iso
            .entries()
            .unwrap()
            .into_iter()
            .find(|e| e.path.ends_with(&format!("/{name}.map")))
            .unwrap();
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let mem = tags::schema::Memory::of(&mut m).unwrap();
        let s = m.scenario().unwrap().clone();
        let a = s.location.unwrap().address;
        let w = |at: u32| u32::from_le_bytes(mem.slice(at, 4).unwrap().try_into().unwrap());
        let n = w(a + 0x210);
        let desc = w(a + 0x214);
        for k in 0..n {
            let sbsp = map::TagId(w(desc + k * 0x44 + 0x14));
            let t = m.tag(sbsp).unwrap().clone();
            let bytes = m.read_tag_bytes(sbsp).unwrap();
            let clusters = u32::from_le_bytes(bytes[0x9c..0xa0].try_into().unwrap());
            let check = u32::from_le_bytes(
                mem.slice(w(a + 0x294) + k * 0x34 + 0x18, 4)
                    .unwrap()
                    .try_into()
                    .unwrap(),
            );
            let hits: Vec<usize> = (0..bytes.len() / 4)
                .filter(|i| {
                    u32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap()) == check
                })
                .map(|i| i * 4)
                .take(4)
                .collect();
            println!(
                "{name} bsp {k}: {:?}, clusters word +0x9c {clusters}, cluster data +0x18 {check:08x} found in the sbsp at {hits:x?}",
                t.name
            );
        }
        for (field, size) in [(0x290u32, 0x34u32), (0x388, 0x10)] {
            let at = w(a + field + 4);
            for k in 0..w(a + field) {
                let e = mem.slice(at + k * size, size).unwrap();
                let words: Vec<String> = e
                    .chunks(4)
                    .map(|c| format!("{:08x}", u32::from_le_bytes(c.try_into().unwrap())))
                    .collect();
                println!("{name} {field:#x}[{k}]: {}", words.join(" "));
                let mut o = 0;
                while o + 8 <= size {
                    let c = u32::from_le_bytes(e[o as usize..o as usize + 4].try_into().unwrap());
                    let p =
                        u32::from_le_bytes(e[o as usize + 4..o as usize + 8].try_into().unwrap());
                    if c > 0 && c < 0x10000 && mem.slice(p, 4).is_some() {
                        let first: Vec<String> = mem
                            .slice(p, 12.min(c * 4))
                            .unwrap()
                            .chunks(4)
                            .map(|x| format!("{:08x}", u32::from_le_bytes(x.try_into().unwrap())))
                            .collect();
                        println!("    +{o:#x}: {c} elements, first {first:?}");
                    }
                    o += 4;
                }
            }
        }
    }
}

/// Per retail map, which of the characters and objects 05a lacks it holds,
/// whether `tags::schema::transplant` takes its tree onto 05a, and how
/// many tags, `snd!` tags and bytes the tree adds (the donor choice for
/// `h1campaign::donor`).
#[test]
#[ignore = "reads the Halo 2 disc"]
fn donor_survey() {
    let wanted: &[(&[u8; 4], &str)] = &[
        (b"char", "objects\\characters\\hunter\\ai\\hunter"),
        (
            b"char",
            "objects\\characters\\floodcombat_human\\ai\\flood_combat_human",
        ),
        (
            b"char",
            "objects\\characters\\floodcombat_elite\\ai\\floodcombat_elite",
        ),
        (
            b"char",
            "objects\\characters\\flood_infection\\ai\\flood_infection",
        ),
        (
            b"char",
            "objects\\characters\\floodcarrier\\ai\\flood_carrier",
        ),
        (b"char", "objects\\characters\\monitor\\ai\\monitor"),
        (
            b"char",
            "objects\\characters\\sentinel_aggressor\\ai\\sentinel_aggressor_halo1",
        ),
        (b"vehi", "objects\\vehicles\\insertion_pod\\insertion_pod"),
    ];
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let entries: Vec<_> = iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
        .collect();
    let base_e = entries
        .iter()
        .find(|e| e.path.ends_with("/05a_deltaapproach.map"))
        .unwrap()
        .clone();
    let base_bytes = iso.read(&base_e).unwrap();
    let schema = tags::schema::Schema::builtin();
    for e in &entries {
        let f = e.path.rsplit('/').next().unwrap().to_string();
        if !f.as_bytes()[0].is_ascii_digit() {
            continue;
        }
        let bytes = iso.read(e).unwrap();
        let mut m = map::Map::from_reader(Cursor::new(bytes.as_slice())).unwrap();
        // Any sentinel character too: the Halo 1 one does not copy.
        let sentinels: Vec<String> = m
            .tags()
            .iter()
            .filter(|t| t.group == map::FourCC(*b"char"))
            .filter_map(|t| t.name.clone())
            .filter(|n| n.contains("sentinel") && !wanted.iter().any(|w| w.1 == n))
            .collect();
        let sentinels: Vec<(&[u8; 4], &str)> =
            sentinels.iter().map(|n| (b"char", n.as_str())).collect();
        for (g, name) in wanted.iter().chain(&sentinels) {
            let Some(t) = m
                .tags()
                .iter()
                .find(|t| t.group == map::FourCC(**g) && t.name.as_deref() == Some(name))
                .cloned()
            else {
                continue;
            };
            let mut base = map::Map::from_reader(Cursor::new(base_bytes.as_slice())).unwrap();
            let mut set = map::build::TagSet::from_map(&mut base).unwrap();
            let before = set.slots.len();
            match tags::schema::transplant::transplant(schema, &mut m, t.id, &mut set).unwrap() {
                Ok(done) => {
                    let sounds = set.slots[before..]
                        .iter()
                        .filter(|s| {
                            s.tag
                                .as_ref()
                                .is_some_and(|t| t.group == map::FourCC(*b"snd!"))
                        })
                        .count();
                    let bytes: u64 = set.slots[before..]
                        .iter()
                        .filter_map(|s| match &s.tag.as_ref()?.payload {
                            map::build::TagPayload::Built(b) => Some(b.tree_len()),
                            _ => None,
                        })
                        .sum();
                    println!(
                        "{f}: {name}: {} tags added ({sounds} snd!), {bytes:#x} metadata bytes, raw {:?}, {} dropped",
                        done.added.len(),
                        done.raw,
                        done.dropped.len()
                    );
                }
                Err(r) => println!(
                    "{f}: {name}: refused: {:?}",
                    r.iter()
                        .take(3)
                        .map(|r| format!("{} {} {:?}", r.group, r.name, r.detail))
                        .collect::<Vec<_>>()
                ),
            }
        }
    }
}

/// Where Halo 1 keeps BSP switch trigger volumes: per level, every root
/// block of `stride`-byte elements whose first three shorts read as (a
/// trigger volume index, a BSP, another BSP). Halo 2 keeps them at
/// `scnr+0x130` (`0xe` each), switched by the engine.
#[test]
#[ignore = "reads the Halo 1 disc"]
fn h1_bsp_switch_volumes() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let mut hits = std::collections::BTreeMap::<(usize, usize), Vec<String>>::new();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let e = d.map(level).unwrap().clone();
        let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
        let bsps = h1.structure_bsps().unwrap().len() as i16;
        let volumes = h1campaign::scenario::trigger_volumes(&h1).unwrap().len() as i16;
        let scnr = h1.tag_data(h1.scenario()).unwrap();
        for at in (0..0x5a0).step_by(4) {
            for stride in [6usize, 8, 0xc, 0xe, 0x10] {
                let Ok(es) = h1.tag_space().block_at(scnr, at, stride) else {
                    continue;
                };
                let s = |e: &[u8], o: usize| i16::from_le_bytes([e[o], e[o + 1]]);
                if !es.is_empty()
                    && es.len() < 200
                    && es.iter().all(|e| {
                        (0..volumes).contains(&s(e, 0))
                            && (0..bsps).contains(&s(e, 2))
                            && (0..bsps).contains(&s(e, 4))
                            && s(e, 2) != s(e, 4)
                    })
                {
                    let pairs: Vec<String> = es
                        .iter()
                        .take(6)
                        .map(|e| format!("{}:{}->{}", s(e, 0), s(e, 2), s(e, 4)))
                        .collect();
                    hits.entry((at, stride)).or_default().push(format!(
                        "{level} ({bsps} bsps, {volumes} volumes) {} {}",
                        es.len(),
                        pairs.join(" ")
                    ));
                }
            }
        }
    }
    for ((at, stride), levels) in &hits {
        println!("+{at:#x} stride {stride:#x}: {} levels", levels.len());
        for l in levels {
            println!("    {l}");
        }
    }
}

/// Halo 2's BSP switch trigger volumes (`scnr+0x130`, `0xe` each) on a few
/// retail levels, as shorts: what follows (volume, source, destination).
#[test]
#[ignore = "reads the Halo 2 disc"]
fn h2_bsp_switch_volumes() {
    let path = disc("H2_DISC", "halo2.iso");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&path).unwrap())).unwrap();
    let mut seen = std::collections::BTreeMap::<Vec<i16>, usize>::new();
    for e in iso.entries().unwrap() {
        let f = e.path.rsplit('/').next().unwrap().to_string();
        if !f.ends_with(".map") || !f.as_bytes()[0].is_ascii_digit() {
            continue;
        }
        let mut m = map::Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        let t = m.scenario().unwrap().clone();
        let addr = t.location.unwrap().address;
        let b = m.read_tag_bytes(t.id).unwrap();
        let w = |at: usize| u32::from_le_bytes(b[at..at + 4].try_into().unwrap());
        let n = w(0x130) as usize;
        let first = w(0x134).wrapping_sub(addr) as usize;
        let mut rows = Vec::new();
        for k in 0..n {
            let e = &b[first + k * 0xe..][..0xe];
            let s: Vec<i16> = (0..7)
                .map(|i| i16::from_le_bytes([e[2 * i], e[2 * i + 1]]))
                .collect();
            *seen.entry(s[3..].to_vec()).or_default() += 1;
            rows.push(format!("{:?}", s));
        }
        println!(
            "{f}: {n}: {}",
            rows.iter().take(4).cloned().collect::<Vec<_>>().join(" ")
        );
    }
    println!("tails (+6..+0xe) by count: {seen:?}");
}

/// How Halo 1 uses its lifepods, per level: the lifepod vehicle tags, how
/// many placements each has and under which object names, and which
/// script builtins take those names (counted). Decides the lifepod's
/// template by role (driven, flown by recording, or only animated).
#[test]
#[ignore = "reads the Halo 1 disc"]
fn lifepod_use() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let e = h1iso.find("/default.xbe").unwrap();
    let catalog =
        h1scripts::Catalog::from_xbe(&xbe::Xbe::parse(&h1iso.read(&e).unwrap()).unwrap()).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let e = d.map(level).unwrap().clone();
        let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
        let scnr = h1.tag_data(h1.scenario()).unwrap();
        let space = h1.tag_space();
        let pal = space.block_at(scnr, 0x24c, 0x30).unwrap();
        let pods: Vec<(usize, String)> = pal
            .iter()
            .enumerate()
            .filter_map(|(i, e)| {
                let r = h1map::TagRef::read(e, 0)?;
                let t = h1.tag(r.id).ok()?;
                t.name.contains("lifepod").then(|| (i, t.name.clone()))
            })
            .collect();
        if pods.is_empty() {
            continue;
        }
        let vehicles = space.block_at(scnr, 0x240, 0x78).unwrap();
        let names = space.block_at(scnr, 0x204, 0x24).unwrap();
        let name_of = |placement: usize| {
            names.iter().find_map(|n| {
                let ty = i16::from_le_bytes([n[0x20], n[0x21]]);
                let at = i16::from_le_bytes([n[0x22], n[0x23]]);
                (ty == 1 && at as usize == placement).then(|| {
                    let end = n[..32].iter().position(|&c| c == 0).unwrap_or(32);
                    String::from_utf8_lossy(&n[..end]).into_owned()
                })
            })
        };
        let t = h1scripts::Tables::read(&h1).unwrap();
        let src = h1scripts::decompile::decompile(&t, &catalog)
            .unwrap()
            .source;
        for (i, path) in &pods {
            let placed: Vec<usize> = vehicles
                .iter()
                .enumerate()
                .filter(|(_, v)| i16::from_le_bytes([v[0], v[1]]) as usize == *i)
                .map(|(k, _)| k)
                .collect();
            let named: Vec<String> = placed.iter().filter_map(|&k| name_of(k)).collect();
            let mut calls = std::collections::BTreeMap::<String, usize>::new();
            for n in &named {
                for (at, _) in src.match_indices(&format!(" {n}")) {
                    let before = &src[..at];
                    let open = before.rfind('(').unwrap();
                    let f = before[open + 1..].split_whitespace().next().unwrap_or("");
                    *calls.entry(f.to_owned()).or_default() += 1;
                }
            }
            println!(
                "{level}: {path}: {} placed, named {named:?}; calls {calls:?}",
                placed.len()
            );
        }
    }
}

/// Per Halo 1 level and placed kind: how many placements set each bit of
/// the `u16` at `+4`, and (`H1_LEVEL`, default a10) each biped's name,
/// bits, palette tag and position: which objects Halo 1 leaves for the
/// scripts to create (the cutscene cast).
#[test]
#[ignore = "reads the discs"]
fn h1_placement_flags() {
    use h1campaign::scenario::h1 as k;
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let detail = std::env::var("H1_LEVEL").unwrap_or("a10".into());
    let kinds = [
        ("weapons", k::WEAPONS, k::WEAPON_PALETTE),
        ("equipment", k::EQUIPMENT, k::EQUIPMENT_PALETTE),
        ("vehicles", k::VEHICLES, k::VEHICLE_PALETTE),
        ("scenery", k::SCENERY, k::SCENERY_PALETTE),
        ("bipeds", k::BIPEDS, k::BIPED_PALETTE),
    ];
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let e = d.map(level).unwrap().clone();
        let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
        let scnr = h1.tag_data(h1.scenario()).unwrap();
        let block =
            |(at, stride): (usize, usize)| h1.tag_space().block_at(scnr, at, stride).unwrap();
        let names: Vec<String> = block(k::OBJECT_NAMES)
            .iter()
            .map(|b| {
                let end = b[..32].iter().position(|&c| c == 0).unwrap_or(32);
                String::from_utf8_lossy(&b[..end]).into_owned()
            })
            .collect();
        for (label, at, pal) in kinds {
            let pal: Vec<String> = block(pal)
                .iter()
                .map(|e| {
                    h1map::TagRef::read(e, 0)
                        .and_then(|r| h1.tag(r.id).ok())
                        .map(|t| t.name.clone())
                        .unwrap_or_default()
                })
                .collect();
            let src = block(at);
            let mut bits = [0usize; 16];
            for e in &src {
                let f = u16::from_le_bytes([e[4], e[5]]);
                for (b, n) in bits.iter_mut().enumerate() {
                    *n += usize::from(f >> b & 1 == 1);
                }
                if level == detail && label == "bipeds" {
                    let p = i16::from_le_bytes([e[0], e[1]]);
                    let n = i16::from_le_bytes([e[2], e[2 + 1]]);
                    let pos: Vec<f32> = (0..3)
                        .map(|i| f32::from_le_bytes(e[8 + 4 * i..12 + 4 * i].try_into().unwrap()))
                        .collect();
                    println!(
                        "  {level} biped {:<24} flags {f:#06x} {:<40} at {pos:.1?}",
                        usize::try_from(n)
                            .ok()
                            .and_then(|n| names.get(n))
                            .map_or("-", |s| s.as_str()),
                        usize::try_from(p)
                            .ok()
                            .and_then(|p| pal.get(p))
                            .map_or("-", |s| s.as_str()),
                    );
                }
            }
            println!(
                "{level} {label}: {} placements, bits set {bits:?}",
                src.len()
            );
        }
    }
}

/// `H1_LEVEL` (default a10): the encounters Halo 1 creates at load
/// (encounter flag bit 0 clear, placed by the converter's startup
/// script), each squad's actor variant and first starting location.
#[test]
#[ignore = "reads the discs"]
fn h1_initial_encounters() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let level = std::env::var("H1_LEVEL").unwrap_or("a10".into());
    let e = d.map(&level).unwrap().clone();
    let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
    let ai = h1ai::h1::H1Ai::read(&h1).unwrap();
    for e in ai.encounters.iter().filter(|e| e.flags & 1 == 0) {
        for s in &e.squads {
            let variant = usize::try_from(s.actor)
                .ok()
                .and_then(|i| ai.actor_palette.get(i).cloned().flatten())
                .map_or("-".to_owned(), |v| v.name);
            let at = s.starting_locations.first().map(|l| l.position);
            println!(
                "{level} {}/{}: {variant}, {} starting locations, first at {at:.1?}",
                e.name,
                s.name,
                s.starting_locations.len()
            );
        }
    }
}

/// Per level: Halo 1's encounters and squads, the squads with starting
/// locations, the empty ones the scripts name, and the converted squad
/// groups and squads against the engine's caps.
#[test]
#[ignore = "reads the discs"]
fn ai_counts_against_the_caps() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let e = d.map(level).unwrap().clone();
        let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
        let ai = h1ai::h1::H1Ai::read(&h1).unwrap();
        let named = h1scripts::Tables::read(&h1)
            .map(|t| t.atom_tokens(h1scripts::catalog::types::AI))
            .unwrap_or_default()
            .into_iter()
            .map(|n| n.to_ascii_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        let squads: usize = ai.encounters.iter().map(|e| e.squads.len()).sum();
        let placed: usize = ai
            .encounters
            .iter()
            .flat_map(|e| &e.squads)
            .filter(|q| !q.starting_locations.is_empty())
            .count();
        let empty_named = ai
            .encounters
            .iter()
            .flat_map(|e| e.squads.iter().map(move |q| (e, q)))
            .filter(|(e, q)| {
                q.starting_locations.is_empty()
                    && named.contains(&format!("{}/{}", e.name, q.name).to_ascii_lowercase())
            })
            .count();
        let empty_encounters = ai
            .encounters
            .iter()
            .filter(|e| e.squads.iter().all(|q| q.starting_locations.is_empty()))
            .count();
        println!(
            "{level}: {} encounters ({empty_encounters} with no placed squad), {squads} squads, {placed} with starting locations, {empty_named} empty ones named; converted {} squads; named tokens {}",
            ai.encounters.len(),
            placed + empty_named,
            named.len()
        );
    }
}

/// `H1_LEVEL` (default a50): the builtins whose arguments name an empty
/// squad (no starting locations), with the argument position.
#[test]
#[ignore = "reads the discs"]
fn empty_squad_uses() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let level = std::env::var("H1_LEVEL").unwrap_or("a50".into());
    let e = d.map(&level).unwrap().clone();
    let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
    let ai = h1ai::h1::H1Ai::read(&h1).unwrap();
    let empty: std::collections::BTreeSet<String> = ai
        .encounters
        .iter()
        .flat_map(|e| {
            e.squads
                .iter()
                .filter(|q| q.starting_locations.is_empty())
                .map(move |q| format!("{}/{}", e.name, q.name))
        })
        .collect();
    let t = h1scripts::Tables::read(&h1).unwrap();
    let mut uses = std::collections::BTreeMap::<String, usize>::new();
    for n in t.nodes.iter().filter(|n| n.live() && !n.atom()) {
        let Ok(head) = t.node(hs::Datum(n.data())) else {
            continue;
        };
        let name = t.string(head.string_offset()).unwrap_or("?").to_owned();
        let mut a = head.next();
        let mut k = 0;
        while a != hs::Datum::NULL {
            let x = *t.node(a).unwrap();
            if x.atom()
                && !x.global()
                && x.opcode() == h1scripts::catalog::types::AI
                && empty.contains(t.string(x.string_offset()).unwrap_or(""))
            {
                *uses.entry(format!("{name} arg {k}")).or_default() += 1;
            }
            a = x.next();
            k += 1;
        }
    }
    println!("{level}: {} empty squads; uses {uses:?}", empty.len());
}

/// `H1_LEVEL` (default b30) and `TOKENS` (comma-separated): each script
/// atom with that token, its Halo 1 type and data word, and whether the
/// level's cutscene flags name it.
#[test]
#[ignore = "reads the discs"]
fn h1_atoms_by_token() {
    let h1p = disc("H1_ISO", "halo1.iso");
    let mut h1iso = xiso::Xiso::open(BufReader::new(File::open(&h1p).unwrap())).unwrap();
    let d = h1map::disc::Disc::open(&mut h1iso).unwrap();
    let level = std::env::var("H1_LEVEL").unwrap_or("b30".into());
    let e = d.map(&level).unwrap().clone();
    let h1 = h1map::H1Map::from_reader(Cursor::new(h1iso.read(&e).unwrap())).unwrap();
    let t = h1scripts::Tables::read(&h1).unwrap();
    let lv = h1scripts::level::Level::read(&h1).unwrap();
    let flags: Vec<&str> = lv.flags.iter().map(|f| f.name.as_str()).collect();
    for tok in std::env::var("TOKENS").unwrap_or_default().split(',') {
        for n in t
            .nodes
            .iter()
            .filter(|n| n.live() && n.atom() && !n.global())
        {
            if t.string(n.string_offset()).ok() == Some(tok) {
                let at = (n.data() & 0xffff) as usize;
                println!(
                    "{level} {tok}: type {} data {:#x}; in the flags block: {}; flag {at}: {:?}, device group {at}: {:?}",
                    n.opcode(),
                    n.data(),
                    flags.contains(&tok),
                    flags.get(at),
                    lv.device_groups.get(at).map(|g| g.name.as_str())
                );
            }
        }
    }
    use h1scripts::level::h2;
    let bytes = lv.flags.len() * h2::FLAGS.1
        + lv.camera_points.len() * h2::CAMERA_POINTS.1
        + lv.titles.len() * h2::TITLES.1
        + lv.device_groups.len() * h2::DEVICE_GROUPS.1;
    println!(
        "{level}: {} flags, {} camera points, {} titles, {} device groups: {bytes:#x} bytes in Halo 2's blocks",
        flags.len(),
        lv.camera_points.len(),
        lv.titles.len(),
        lv.device_groups.len()
    );
}
