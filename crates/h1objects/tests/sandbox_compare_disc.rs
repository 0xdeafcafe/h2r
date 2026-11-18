//! Survey: Blood Gulch's two sandboxes side by side, and retail Coagulation.
//! Prints names and counts only. Nothing is written.
//!
//! ```sh
//! H1_ISO=... H2_DISC=... cargo test -p h1objects --test sandbox_compare_disc -- --ignored --nocapture
//! ```

use std::collections::{BTreeMap, BTreeSet};
use std::fs::File;
use std::io::{BufReader, Cursor};

use h1convert::convert::halo1::prepare_halo1;
use h1convert::convert::{Sandbox, structure_mopp};
use h1map::H1Map;
use h1objects::base::Base;
use h1objects::sandbox::Objects;
use map::{FourCC, Map};

fn bloodgulch() -> H1Map {
    let path = std::env::var("H1_ISO").expect("set H1_ISO");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let disc = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = disc.map("bloodgulch").unwrap().clone();
    H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn h2_map(name: &str) -> Map<Cursor<Vec<u8>>> {
    let path = std::env::var("H2_DISC").expect("set H2_DISC");
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(path).unwrap())).unwrap();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.rsplit(['/', '\\']).next() == Some(&format!("{name}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn fresh(sandbox: Sandbox) -> Vec<u8> {
    let h1 = bloodgulch();
    let mut base = h2_map("coagulation");
    let sha = content::Sha256::of(b"bloodgulch");
    let p = if sandbox == Sandbox::Halo1 {
        let templates = Base::read(&mut base).unwrap();
        let mut objects = Objects::new(&h1, &templates);
        prepare_halo1(&h1, sha, &mut base, &mut objects).unwrap()
    } else {
        {
            let templates = Base::read(&mut base).unwrap();
            let mut objects = Objects::new(&h1, &templates);
            h1convert::convert::halo1::prepare_halo2(&h1, sha, &mut base, &mut objects).unwrap()
        }
    };
    let mopp = structure_mopp(&p.collision).unwrap();
    p.build_fresh(&mut base, &mopp).unwrap().bytes
}

fn names(m: &Map<Cursor<Vec<u8>>>) -> BTreeSet<(String, String)> {
    m.tags()
        .iter()
        .filter(|t| t.location.is_some())
        .map(|t| (t.group.to_string(), t.name.clone().unwrap_or_default()))
        .collect()
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn bloodgulch_sandboxes_side_by_side() {
    let retail = h2_map("coagulation");
    let h2 = Map::from_reader(Cursor::new(fresh(Sandbox::Halo2))).unwrap();
    let h1 = Map::from_reader(Cursor::new(fresh(Sandbox::Halo1))).unwrap();
    let (r, a, b) = (names(&retail), names(&h2), names(&h1));
    let groups = |s: &BTreeSet<(String, String)>| {
        let mut g = BTreeMap::<String, usize>::new();
        for (k, _) in s {
            *g.entry(k.clone()).or_default() += 1;
        }
        g
    };
    let (gr, ga, gb) = (groups(&r), groups(&a), groups(&b));
    let all: BTreeSet<&String> = gr.keys().chain(ga.keys()).chain(gb.keys()).collect();
    for g in all {
        let n = |m: &BTreeMap<String, usize>| m.get(g).copied().unwrap_or(0);
        println!("group {g}: retail {} h2 {} h1 {}", n(&gr), n(&ga), n(&gb));
    }
    for (g, n) in a.difference(&b) {
        println!("only in h2 sandbox: {g} {n}");
    }
    for (g, n) in b.difference(&a) {
        if ["weap", "eqip", "vehi", "bipd", "itmc", "vehc", "scen"].contains(&g.as_str()) {
            println!("only in h1 sandbox: {g} {n}");
        }
    }
    for (label, mut m) in [("retail", retail), ("h2", h2), ("h1", h1)] {
        units(label, &mut m);
    }
}

fn memory(m: &mut Map<Cursor<Vec<u8>>>) -> tags::schema::Memory {
    let space = m.metadata_space().unwrap();
    let meta = m
        .read_file_range(space.file_offset, u64::from(space.size))
        .unwrap();
    tags::schema::Memory::from_regions(
        vec![(space.virtual_base, meta)],
        vec![(space.virtual_base, space.virtual_base + space.size)],
    )
}

/// Each biped's unit string IDs and its render model's nodes and markers.
fn units(label: &str, m: &mut Map<Cursor<Vec<u8>>>) {
    let mem = memory(m);
    let w = |a: u32| {
        mem.slice(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0)
    };
    let strings = m.strings().to_vec();
    let s = |v: u32| {
        strings
            .get((v & 0xff_ffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("{v:#x}"))
    };
    let tags = m.tags().to_vec();
    let at = |id: u32| {
        tags.iter()
            .find(|t| t.id.0 == id)
            .and_then(|t| t.location.map(|l| (l.address, t.name.clone().unwrap_or_default())))
    };
    for t in tags.iter().filter(|t| t.group == FourCC(*b"bipd")) {
        let Some(loc) = t.location else { continue };
        let a = loc.address;
        let name = t.name.clone().unwrap_or_default();
        let strs: Vec<String> = [0xd4u32, 0x158, 0x15c, 0x160, 0x30]
            .iter()
            .map(|o| format!("+{o:#x} {:?}", s(w(a + o))))
            .collect();
        println!("{label} bipd {name}: {}", strs.join(", "));
        let n = w(a + 0x1c8);
        let e = w(a + 0x1cc);
        for k in 0..n.min(4) {
            let b = e + k * 0xb4;
            println!(
                "{label}   bipd +0x1c8[{k}]: {:?} {:?} {:?}",
                s(w(b + 4)),
                s(w(b + 8)),
                s(w(b + 0xc))
            );
        }
        let Some((h, _)) = at(w(a + 0x38)) else { continue };
        let Some((mo, mname)) = at(w(h + 4)) else { continue };
        let nodes: Vec<String> = (0..w(mo + 0x48))
            .map(|k| s(w(w(mo + 0x4c) + k * 0x60)))
            .collect();
        let markers: Vec<String> = (0..w(mo + 0x58))
            .map(|k| s(w(w(mo + 0x5c) + k * 0xc)))
            .collect();
        println!("{label}   mode {mname}: nodes {nodes:?}");
        println!("{label}   markers {markers:?}");
    }
}

/// The printable names in Halo 1's cyborg_mp `bipd` root (4-aligned runs).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn cyborg_unit_names() {
    let h1 = bloodgulch();
    let tag = h1
        .tags()
        .iter()
        .find(|t| t.name.ends_with("cyborg_mp") && t.group.to_string() == "bipd")
        .unwrap()
        .clone();
    let b = h1.tag_data(&tag).unwrap();
    let mut at = 0;
    while at + 4 <= b.len() {
        let run: Vec<u8> = b[at..].iter().take_while(|c| c.is_ascii_graphic() || **c == b' ').copied().collect();
        if run.len() >= 3 && b.get(at + run.len()) == Some(&0) {
            println!("cyborg_mp +{at:#x}: {:?}", String::from_utf8_lossy(&run));
            at += (run.len() + 4) & !3;
        } else {
            at += 4;
        }
    }
    println!("cyborg_mp root {:#x} bytes", b.len());
}
