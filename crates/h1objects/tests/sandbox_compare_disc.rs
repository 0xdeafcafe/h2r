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
    // What only one sandbox holds, by group and the path's first three
    // components.
    for (label, x, y) in [("h2", &a, &b), ("h1", &b, &a)] {
        let mut dirs = BTreeMap::<(String, String), usize>::new();
        for (g, n) in x.difference(y) {
            let d: Vec<&str> = n.split('\\').take(3).collect();
            *dirs.entry((g.clone(), d.join("\\"))).or_default() += 1;
        }
        for ((g, d), c) in dirs {
            println!("only in {label} sandbox: {g} {d}\\.. x{c}");
        }
    }
    for (g, n) in a.difference(&b) {
        if !["snd!", "bitm", "shad", "effe", "prt3"].contains(&g.as_str()) {
            println!("only in h2 sandbox: {g} {n}");
        }
    }
    for (g, n) in b.difference(&a) {
        if ["weap", "eqip", "vehi", "bipd", "itmc", "vehc", "scen"].contains(&g.as_str()) {
            println!("only in h1 sandbox: {g} {n}");
        }
    }
    let mut maps = [("retail", retail), ("h2", h2), ("h1", h1)];
    for (label, m) in &mut maps {
        units(label, m);
    }
    let refs: Vec<(String, BTreeMap<String, Vec<String>>)> = maps
        .iter_mut()
        .map(|(l, m)| (l.to_string(), ambient_refs(m)))
        .collect();
    for (k, _) in &refs[1].1 {
        let get = |i: usize| refs[i].1.get(k).cloned().unwrap_or_default();
        let (h2s, h1s) = (get(1), get(2));
        println!(
            "{k}: {}",
            if h2s == h1s { "same in both sandboxes" } else { "DIFFERS" }
        );
        if h2s != h1s {
            println!("  h2: {h2s:?}\n  h1: {h1s:?}");
        }
        println!("  retail: {:?}", get(0));
    }
    // Both sandboxes take the structure, lightmap, sky and scenario
    // ambience from the same conversion (`prepare` with the Halo 2
    // sandbox, then the Halo 1 sandbox's objects): every ambience
    // reference is the same in both.
    assert_eq!(refs[1].1, refs[2].1, "the sandboxes' ambience differs");
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

/// The printable names in Halo 1's cyborg_mp `bipd` root and each `weap` root (4-aligned runs).
#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO)"]
fn cyborg_unit_names() {
    let h1 = bloodgulch();
    let mut list: Vec<_> = h1
        .tags()
        .iter()
        .filter(|t| {
            (t.name.ends_with("cyborg_mp") && t.group.to_string() == "bipd")
                || t.group.to_string() == "weap"
        })
        .cloned()
        .collect();
    list.sort_by_key(|t| t.group.to_string());
    for tag in list {
    let b = h1.tag_data(&tag).unwrap();
    let b = &b[..b.len().min(0x800)];
    let mut at = 0;
    while at + 4 <= b.len() {
        let run: Vec<u8> = b[at..].iter().take_while(|c| c.is_ascii_graphic() || **c == b' ').copied().collect();
        if run.len() >= 2 && b.get(at + run.len()) == Some(&0) {
            println!("{} +{at:#x}: {:?}", tag.name, String::from_utf8_lossy(&run));
            at += (run.len() + 4) & !3;
        } else {
            at += 4;
        }
    }
    }
}

/// The tag names each ambience-related tag names, by `<group> <where>`:
/// the scenario's skies (`+0x08`), background sound and sound environment
/// palettes (`+0x250`, `+0x258`), the scenario and globals roots, each
/// `sky ` root, the structure's root and its lightmap's root. A word pair
/// counts when it is a group and the datum of a tag of that group.
fn ambient_refs(m: &mut Map<Cursor<Vec<u8>>>) -> BTreeMap<String, Vec<String>> {
    let mem = memory(m);
    let tags = m.tags().to_vec();
    let by_id: std::collections::HashMap<u32, (FourCC, String)> = tags
        .iter()
        .map(|t| (t.id.0, (t.group, t.name.clone().unwrap_or_default())))
        .collect();
    let scan = |a: u32, n: u32| -> Vec<String> {
        let Some(b) = mem.slice(a, n) else { return vec!["<unreadable>".into()] };
        let mut out = Vec::new();
        for k in (0..b.len().saturating_sub(7)).step_by(4) {
            let g = u32::from_le_bytes(b[k..k + 4].try_into().unwrap());
            let d = u32::from_le_bytes(b[k + 4..k + 8].try_into().unwrap());
            if let Some((tg, name)) = by_id.get(&d)
                && FourCC(g.to_be_bytes()) == *tg
            {
                out.push(format!("+{k:#x} {tg} {name}"));
            }
        }
        out
    };
    let w = |a: u32| {
        mem.slice(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0)
    };
    let mut out = BTreeMap::new();
    let at = |g: &[u8; 4]| -> Vec<(u32, String)> {
        tags.iter()
            .filter(|t| t.group == FourCC(*g))
            .filter_map(|t| t.location.map(|l| (l.address, t.name.clone().unwrap_or_default())))
            .collect()
    };
    for (a, _) in at(b"scnr") {
        out.insert("scnr root".into(), scan(a, 0x3e0));
        for (field, stride, label) in [(0x08u32, 8u32, "skies"), (0x250, 0x64, "background sounds"), (0x258, 0x48, "sound environments")] {
            let n = w(a + field).min(64);
            out.insert(format!("scnr {label} ({n})"), scan(w(a + field + 4), n * stride));
        }
    }
    for (a, _) in at(b"matg") {
        out.insert("matg root".into(), scan(a, 0x2a0));
    }
    for (a, n) in at(b"sky ") {
        out.insert(format!("sky {n}"), scan(a, 0xac));
    }
    for (a, _) in at(b"sbsp") {
        out.insert("sbsp root".into(), scan(a, 0x240));
    }
    for (a, _) in at(b"ltmp") {
        out.insert("ltmp root".into(), scan(a, 0x100));
    }
    out
}

/// What the starting weapon meets in each sandbox: the scenario's
/// starting profiles (`+0xf8`, `0x44` each, weapon references at `+0x28`
/// and `+0x34`), each `weap`'s `+0x288`/`+0x28c` names, the globals'
/// player biped's animation graph's modes, classes and types (`jmad +0x34`,
/// `0x14` each, classes `+4`, types `+4`), and the string IDs `0x101` and
/// `0xc` the weapon gate `0x0010fcd0` is called with.
fn starting_weapon(label: &str, m: &mut Map<Cursor<Vec<u8>>>) {
    let mem = memory(m);
    let strings = m.strings().to_vec();
    let s = |v: u32| {
        strings
            .get((v & 0xff_ffff) as usize)
            .cloned()
            .flatten()
            .unwrap_or_else(|| format!("{v:#x}"))
    };
    println!("{label}: sid 0x101 {:?}, sid 0xc {:?}", s(0x101), s(0xc));
    let w = |a: u32| {
        mem.slice(a, 4)
            .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            .unwrap_or(0)
    };
    let tags = m.tags().to_vec();
    let at = |id: u32| {
        tags.iter()
            .find(|t| t.id.0 == id && id != u32::MAX)
            .and_then(|t| Some((t.name.clone().unwrap_or_default(), t.location?.address)))
    };
    let scnr = tags.iter().find(|t| t.group == FourCC(*b"scnr")).unwrap();
    let a = scnr.location.unwrap().address;
    let (n, first) = (w(a + 0xf8), w(a + 0xfc));
    for i in 0..n {
        let e = first + i * 0x44;
        for k in 0..2 {
            let r = w(e + 0x28 + 0xc * k + 4);
            println!(
                "{label}: profile {i} weapon {k}: {:?}",
                at(r).map(|x| x.0)
            );
        }
    }
    for t in tags.iter().filter(|t| t.group == FourCC(*b"weap")) {
        let Some(l) = t.location else { continue };
        println!(
            "{label}: weap {}: {:?} {:?}",
            t.name.as_deref().unwrap_or("?"),
            s(w(l.address + 0x288)),
            s(w(l.address + 0x28c))
        );
    }
    let mut jmads = BTreeSet::new();
    for t in tags.iter().filter(|t| t.group == FourCC(*b"bipd")) {
        let Some(l) = t.location else { continue };
        let Some((_, h)) = at(w(l.address + 0x38)) else { continue };
        jmads.insert((t.name.clone().unwrap_or_default(), w(h + 0x14)));
    }
    // The weapon list `0x0015eb80` searches: globals `+0x168` (`mulg`),
    // its `+8` block's first element, `+0x68` (8 bytes each, datum at
    // `+4`).
    let matg = tags.iter().find(|t| t.group == FourCC(*b"matg")).unwrap();
    let mulg = w(matg.location.unwrap().address + 0x16c);
    if let Some((mn, ma)) = at(mulg) {
        let u = w(ma + 0xc);
        let (n, first) = (w(u + 0x68), w(u + 0x6c));
        for i in 0..n {
            println!(
                "{label}: {mn} weapon {i}: {:?}",
                at(w(first + i * 8 + 4)).map(|x| x.0)
            );
        }
    }
    for (bipd, j) in jmads {
        let Some((jn, ja)) = at(j) else { continue };
        let (mn, mf) = (w(ja + 0x34), w(ja + 0x38));
        let mut tree = Vec::new();
        for i in 0..mn.min(64) {
            let me = mf + i * 0x14;
            let (cn, cf) = (w(me + 4), w(me + 8));
            for c in 0..cn.min(64) {
                let ce = cf + c * 0x14;
                let (tn, tf) = (w(ce + 4), w(ce + 8));
                let types: Vec<String> = (0..tn.min(64)).map(|k| s(w(tf + k * 0x34))).collect();
                tree.push(format!("{}/{}: {types:?}", s(w(me)), s(w(ce))));
            }
        }
        println!("{label}: bipd {bipd} jmad {jn}: {tree:#?}");
    }
}

#[test]
#[ignore = "reads the Halo 1 disc ($H1_ISO) and the Halo 2 disc ($H2_DISC)"]
fn starting_weapons_side_by_side() {
    for sandbox in [Sandbox::Halo2, Sandbox::Halo1] {
        let mut m = Map::from_reader(Cursor::new(fresh(sandbox))).unwrap();
        starting_weapon(&format!("{sandbox:?}"), &mut m);
    }
    starting_weapon("retail coagulation", &mut h2_map("coagulation"));
}
