//! Evidence surveys behind the converter's choices, from the owner's
//! discs at run time. They print, and assert nothing about retail data:
//!
//! - `survey_bases`: per Halo 2 campaign level, which `h1objects`
//!   templates and a30 characters it lacks, and its tag memory (the base
//!   choice, docs/game/campaigns.md §9);
//! - `find_named_blocks`: Halo 1 scenario blocks of printable names (the
//!   trigger volume and object name offsets);
//! - `ai_and_name_literal_encoding`: how retail Halo 2 scripts encode
//!   object, trigger volume, AI and enum literals (the hook values).
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
