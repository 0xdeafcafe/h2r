//! Campaign levels built fresh: nothing of the Halo 2 base pinned, the
//! scenario written by the converter, and the base's tags nothing reaches
//! dropped. From the owner's discs (`$H1_ISO`, `$H2_DISC`); nothing is
//! written to disk, counts go to stdout. `H1_LEVELS=a30,b40` picks levels.

use std::fs::File;
use std::io::{BufReader, Cursor};

use map::{FourCC, Map};

type M = Map<Cursor<Vec<u8>>>;

fn disc(var: &str, name: &str) -> String {
    std::env::var(var)
        .unwrap_or_else(|_| format!("{}/../../disc/{name}", env!("CARGO_MANIFEST_DIR")))
}

fn h2_iso() -> xiso::Xiso<BufReader<File>> {
    xiso::Xiso::open(BufReader::new(
        File::open(disc("H2_DISC", "halo2.iso")).unwrap(),
    ))
    .unwrap()
}

fn h2_map(name: &str) -> M {
    let mut iso = h2_iso();
    let e = iso
        .entries()
        .unwrap()
        .into_iter()
        .find(|e| e.path.ends_with(&format!("/{name}.map")))
        .unwrap();
    Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

/// What the base's scenario root holds that the conversion does not
/// replace: every non-empty block field, tag reference and non-zero word.
#[test]
#[ignore = "reads disc/halo2.iso"]
fn survey_base_scenario_fields() {
    let mut base = h2_map(h1campaign::BASE);
    let t = base.scenario().unwrap().clone();
    let bytes = base.read_tag_bytes(t.id).unwrap();
    let schema = tags::schema::Schema::builtin();
    let root = &schema.groups[&FourCC(*b"scnr")];
    let replaced = h1campaign::scenario::replaced_fields();
    for (&o, f) in &root.fields {
        let o = o as usize;
        let mark = if replaced.contains(&(o as u32)) {
            "replaced"
        } else {
            "KEPT"
        };
        match f {
            tags::schema::Field::Block(c) => {
                let n = word(&bytes, o);
                if n > 0 {
                    println!("{o:#x} block x{n} (elem {:#x}) {mark}", c.size);
                }
            }
            tags::schema::Field::TagRef => {
                let d = word(&bytes, o + 4);
                if d != u32::MAX {
                    let name = base.tag(map::TagId(d)).ok().and_then(|t| t.name.clone());
                    println!("{o:#x} tagref {d:#x} {name:?} {mark}");
                }
            }
            _ => {}
        }
    }
    for k in (0..0x3e0).step_by(4) {
        let inside = root.fields.iter().any(|(&o, f)| {
            let n = match f {
                tags::schema::Field::Block(_) | tags::schema::Field::TagRef => 8,
                _ => 4,
            };
            (o as usize..o as usize + n).contains(&k)
        });
        let w = word(&bytes, k);
        if !inside && w != 0 {
            println!("{k:#x} word {w:#x}");
        }
    }
}

/// The base's elements the fresh scenario writes defaults for, as bytes,
/// and which groups the simulation table (`+0x3d8`) lists.
#[test]
#[ignore = "reads disc/halo2.iso"]
fn survey_base_scenario_elements() {
    for name in [h1campaign::BASE, "03a_oldmombasa", "coagulation"] {
        let mut base = h2_map(name);
        let t = base.scenario().unwrap().clone();
        let addr = t.location.unwrap().address;
        let bytes = base.read_tag_bytes(t.id).unwrap();
        println!("{name}: salts {:x?}", &bytes[0x298..0x2c8]);
        for (at, size) in [
            (0x210usize, 0x44usize),
            (0x100, 0x34),
            (0xf8, 0x44),
            (0x218, 0x18),
            (0x398, 0x18),
            (0x318, 0x60),
        ] {
            let n = word(&bytes, at) as usize;
            let p = word(&bytes, at + 4).wrapping_sub(addr) as usize;
            for k in 0..n.min(3) {
                let e = bytes.get(p + k * size..p + (k + 1) * size);
                println!("{name} {at:#x}[{k}] {e:02x?}");
            }
        }
        let n = word(&bytes, 0x3d8) as usize;
        let p = word(&bytes, 0x3dc).wrapping_sub(addr) as usize;
        let mut groups = std::collections::BTreeMap::<String, usize>::new();
        if let Some(list) = bytes.get(p..p + n * 4) {
            for w in list.chunks(4) {
                let d = u32::from_le_bytes(w.try_into().unwrap());
                let g = base
                    .tag(map::TagId(d))
                    .map(|t| t.group.to_string())
                    .unwrap_or("?".into());
                *groups.entry(g).or_default() += 1;
            }
        }
        println!("{name} simulation table x{n}: {groups:?}");
    }
}

fn h1_level(name: &str) -> h1map::H1Map {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    h1map::H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

/// The Halo 2 disc's XBE and every map's scripts.
fn h2_scripts() -> (Vec<u8>, Vec<hs::Scenario>) {
    let mut iso = h2_iso();
    let e = iso.find("default.xbe").unwrap();
    let xbe = iso.read(&e).unwrap();
    let mut scenarios = Vec::new();
    for e in iso
        .entries()
        .unwrap()
        .into_iter()
        .filter(|e| !e.is_dir && e.path.ends_with(".map"))
    {
        let mut m = Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap();
        scenarios.push(hs::Scenario::read(&mut m).unwrap());
    }
    (xbe, scenarios)
}

fn scripting(h2_xbe: &[u8], scenarios: &[hs::Scenario]) -> h1campaign::Scripting {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let e = iso.find("/default.xbe").unwrap();
    let h1_xbe = iso.read(&e).unwrap();
    h1campaign::Scripting::new(&h1_xbe, h2_xbe, scenarios).unwrap()
}

fn donors() -> h1campaign::donor::Donors {
    let mut d = h1campaign::donor::Donors::default();
    let mut iso = h2_iso();
    let entries = iso.entries().unwrap();
    for name in h1campaign::donor::MAPS {
        let e = entries
            .iter()
            .find(|e| e.path.ends_with(&format!("/{name}.map")))
            .unwrap();
        d.maps.push((name.to_owned(), iso.read(e).unwrap()));
    }
    d
}

fn memory(m: &mut M) -> tags::schema::Memory {
    tags::schema::Memory::of(m).unwrap()
}

/// Every tag reference and datum of every tag the schema walks names a
/// tag of the map, of the referenced group (or a descendant), with data
/// (BSP tags excepted). Returns (references checked, problems).
fn reference_sweep(m: &M, mem: &tags::schema::Memory) -> (usize, Vec<String>) {
    use tags::schema::{Field, Schema, visit};
    let schema = Schema::builtin();
    let descends = |g: FourCC, of: FourCC| {
        g == of
            || m.groups()
                .iter()
                .any(|x| x.class == g && (x.parent == Some(of) || x.grandparent == Some(of)))
    };
    let in_bsp = |g: FourCC| g == FourCC(*b"sbsp") || g == FourCC(*b"ltmp");
    let (mut n, mut bad) = (0, Vec::new());
    for t in m.tags() {
        let (Some(l), Some(s)) = (t.location, schema.groups.get(&t.group)) else {
            continue;
        };
        if mem.region(l.address).is_none_or(|r| r.0 != l.address) {
            continue;
        }
        let label = format!("{} {}", t.group, t.name.as_deref().unwrap_or("?"));
        visit(s, l.address, mem, &mut |s, e, _| {
            for (&o, f) in &s.fields {
                let o = o as usize;
                let (g, d) = match f {
                    Field::TagRef => (
                        Some(word(e, o))
                            .filter(|&g| g != u32::MAX)
                            .map(|g| FourCC(g.to_be_bytes())),
                        word(e, o + 4),
                    ),
                    Field::TagId => (None, word(e, o)),
                    _ => continue,
                };
                if d == u32::MAX {
                    continue;
                }
                n += 1;
                match m.tag(map::TagId(d)) {
                    Err(_) => bad.push(format!("{label}: {d:#x} names no tag")),
                    Ok(r) if g.is_some_and(|g| !descends(r.group, g)) => {
                        bad.push(format!("{label}: a {g:?} reference names a {}", r.group))
                    }
                    Ok(r) if r.location.is_none() && !in_bsp(r.group) => bad.push(format!(
                        "{label}: {} {} has no data",
                        r.group,
                        r.name.as_deref().unwrap_or("?")
                    )),
                    Ok(_) => {}
                }
            }
        })
        .unwrap_or_else(|| bad.push(format!("{label}: does not walk")));
    }
    (n, bad)
}

/// Object groups an `object_definition` atom may name.
const OBJECT_GROUPS: [&[u8; 4]; 12] = [
    b"bipd", b"vehi", b"weap", b"eqip", b"garb", b"proj", b"scen", b"mach", b"ctrl", b"ssce",
    b"bloc", b"crea",
];

/// The scripts' tag-valued atoms: each one not `none` names a tag of the
/// map with data, of the atom type's group. Returns (atoms, problems).
fn script_tags(m: &mut M, catalog: &hs::Catalog) -> (usize, Vec<String>) {
    let s = hs::Scenario::read(m).expect("the scripts read, references checked");
    let (mut n, mut bad) = (0, Vec::new());
    for node in s
        .nodes()
        .iter()
        .filter(|x| x.live() && x.atom() && !x.global())
    {
        let Ok(ty) = catalog.type_name(node.value_type()) else {
            continue;
        };
        let groups: Vec<FourCC> = match ty {
            "object_definition" => OBJECT_GROUPS.iter().map(|g| FourCC(**g)).collect(),
            t => match h1scripts::level::tag_group(t) {
                Some(g) => vec![g],
                None => continue,
            },
        };
        let d = node.data();
        if d == u32::MAX {
            continue;
        }
        n += 1;
        match m.tag(map::TagId(d)) {
            Ok(t) if t.location.is_some() && groups.contains(&t.group) => {}
            Ok(t) => bad.push(format!("{ty} atom names {} {:?}", t.group, t.name)),
            Err(_) => bad.push(format!("{ty} atom {d:#x} names no tag")),
        }
    }
    (n, bad)
}

/// Every level converted fresh (and pinned, the old way, with
/// `H1_PINNED=1`); per level the tag memory, file size and lean step of each, and
/// on the fresh map: every reference resolves, no tag is the base level's
/// own, and the scripts' tag-valued atoms resolve.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_level_converts_fresh() {
    let (xbe, scenarios) = h2_scripts();
    let s = scripting(&xbe, &scenarios);
    let mut catalog = hs::Catalog::from_xbe(&xbe::Xbe::parse(&xbe).unwrap()).unwrap();
    for sc in &scenarios {
        catalog.observe(sc).unwrap();
    }
    drop(scenarios);
    let donors = donors();
    let only = std::env::var("H1_LEVELS").ok();
    // Pinned is no longer the default build, and pinned a10 is over the
    // file size cap since the level render (version 40): compared only
    // with H1_PINNED=1.
    let pinned_too = std::env::var("H1_PINNED").is_ok_and(|v| v == "1");
    let base_folder = format!("scenarios\\solo\\{}\\", h1campaign::BASE);
    let mut failed = Vec::new();
    let mut rows = Vec::new();
    for level in h1map::disc::CAMPAIGN_LEVELS {
        if only
            .as_deref()
            .is_some_and(|o| !o.split(',').any(|l| l == level))
        {
            continue;
        }
        let h1 = h1_level(level);
        let mut base = h2_map(h1campaign::base_for(level));
        let mut opts = h1campaign::Options {
            scripts: Some(&s),
            donors: Some(&donors),
            ..Default::default()
        };
        let mut pinned_bad = std::collections::BTreeSet::new();
        let mut pinned_sounds = None;
        // The pinned build's retries: over the file size, it is converted
        // again with less music, so its trimmed sounds differ by design.
        let mut pinned_retries = Vec::new();
        let before = pinned_too.then(|| {
            opts.pinned = true;
            let p = h1campaign::convert(&h1, &mut base, &opts).unwrap();
            opts.pinned = false;
            let mut pm = Map::from_reader(Cursor::new(p.bytes.clone())).unwrap();
            let pmem = memory(&mut pm);
            pinned_bad = reference_sweep(&pm, &pmem).1.into_iter().collect();
            pinned_sounds = Some(h1convert::compact::map_entries(&mut pm, &p.bytes).unwrap());
            pinned_retries = p.report.retries.clone();
            let profiles = start_profiles(&mut pm, &h1);
            assert!(
                profiles.is_empty(),
                "{level} pinned: starting profiles {profiles:#?}"
            );
            (p.report.memory, p.bytes.len(), p.report.lean)
        });
        let done = match h1campaign::convert(&h1, &mut base, &opts) {
            Ok(d) => d,
            Err(e) => {
                println!("{level}: FAILED: {e}");
                failed.push(format!("{level}: {e}"));
                continue;
            }
        };
        let r = &done.report;
        print!("{}", r);
        let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
        m.validate_checksum().unwrap();
        // `tag_group_find` bisects the group table: unsorted, it misses
        // groups (a10 lost `bitm` and faulted at campaign start).
        let words: Vec<u32> = m.groups().iter().map(|g| g.class.as_u32()).collect();
        assert!(words.is_sorted(), "{level}: group table out of order");
        let mem = memory(&mut m);
        let (refs, all_bad) = reference_sweep(&m, &mem);
        // What the pinned conversion of the same level already has (a
        // converted tag the schema reads differently) is not the fresh
        // build's doing; it is counted, not failed.
        let (old, bad): (Vec<String>, Vec<String>) =
            all_bad.into_iter().partition(|b| pinned_bad.contains(b));
        if !old.is_empty() {
            let mut by: std::collections::BTreeMap<&str, usize> = Default::default();
            for b in &old {
                *by.entry(b.split(':').next().unwrap_or("")).or_default() += 1;
            }
            println!("  {} problems the pinned map has too: {by:?}", old.len());
        }
        let theirs: Vec<String> = m
            .tags()
            .iter()
            .filter_map(|t| t.name.clone())
            .filter(|n| n.starts_with(&base_folder) || n.starts_with("scenarios\\multi\\"))
            .collect();
        // Every vehicle's `+0x2f8` elements hold at most 4 physics entries
        // (`+0x38`; the load loop `0x205550` has room for 4): no template
        // address read as junk (h1objects/tests/sandbox_disc.rs).
        let mut vehicles = 0;
        for t in m.tags().to_vec() {
            let (Some(l), true) = (t.location, t.group == FourCC(*b"vehi")) else {
                continue;
            };
            let word = |a: u32| {
                mem.slice(a, 4)
                    .map(|b| u32::from_le_bytes(b.try_into().unwrap()))
            };
            let count = word(l.address + 0x2f8).unwrap_or(u32::MAX);
            let first = word(l.address + 0x2fc).unwrap_or(0);
            for k in 0..count.min(16) {
                vehicles += 1;
                let n = word(first + k * 0x2a0 + 0x38).unwrap_or(u32::MAX);
                if n > 4 || count > 16 {
                    failed.push(format!("{level}: {:?} [{k}] physics count {n:#x}", t.name));
                }
            }
        }
        println!("  {vehicles} vehicle physics elements, each 4 entries or fewer");
        let profile_bad = start_profiles(&mut m, &h1);
        if !profile_bad.is_empty() {
            failed.push(format!("{level}: starting profiles {profile_bad:#?}"));
        }
        let (atoms, script_bad) = script_tags(&mut m, &catalog);
        println!(
            "  {}/{refs} references resolve ({} new problems); {} tags of the base level's own; \
             {atoms} script tag atoms, {} unresolved",
            refs - bad.len() - old.len(),
            bad.len(),
            theirs.len(),
            script_bad.len()
        );
        if !bad.is_empty() {
            failed.push(format!(
                "{level}: {} dangling: {:#?}",
                bad.len(),
                &bad[..bad.len().min(12)]
            ));
        }
        if !theirs.is_empty() {
            failed.push(format!(
                "{level}: base level tags {:#?}",
                &theirs[..theirs.len().min(12)]
            ));
        }
        if !script_bad.is_empty() {
            failed.push(format!(
                "{level}: script atoms {:#?}",
                &script_bad[..script_bad.len().min(12)]
            ));
        }
        match &r.scratch {
            // From scratch: no slot of the base's index left over, and the
            // gestalt cut to the kept sounds, each naming what it did in
            // the pinned map (entries, names, chunk bytes).
            Some(sr) => {
                let holes = m.index_slots() as usize - m.tags().len();
                if holes != 0 {
                    failed.push(format!("{level}: {holes} empty index slots"));
                }
                if sr.gestalt.is_none() {
                    failed.push(format!("{level}: gestalt whole: {:?}", sr.gestalt_whole));
                }
                if let Some(before) = &pinned_sounds {
                    let after = h1convert::compact::map_entries(&mut m, &done.bytes).unwrap();
                    let differ: Vec<&String> = after
                        .iter()
                        .filter(|(n, e)| before.get(*n) != Some(*e))
                        .map(|(n, _)| n)
                        .collect();
                    println!(
                        "  {} sounds name what they did pinned, {} do not",
                        after.len() - differ.len(),
                        differ.len()
                    );
                    if !pinned_retries.is_empty() {
                        println!("  pinned build retried: {pinned_retries:?}");
                    }
                    if !differ.is_empty() && pinned_retries.is_empty() {
                        failed.push(format!(
                            "{level}: sounds differ from pinned: {:?}",
                            &differ[..differ.len().min(8)]
                        ));
                    }
                }
            }
            None => failed.push(format!("{level}: not built from scratch: {:?}", r.dropped)),
        }
        if !r.fresh {
            failed.push(format!("{level}: written pinned: {:?}", r.dropped.last()));
        }
        if r.lean != h1campaign::Lean::Everything {
            failed.push(format!("{level}: the lean ladder fired ({:?})", r.lean));
        }
        rows.push((
            level,
            before,
            r.memory,
            done.bytes.len(),
            r.lean,
            m.tags().len(),
        ));
    }
    println!(
        "level | pinned memory | pinned size | pinned lean | fresh memory | fresh size | fresh lean | tags"
    );
    for (level, b, mem, size, lean, tags) in &rows {
        let (bm, bs, bl) = b.unwrap_or((0, 0, h1campaign::Lean::Everything));
        println!("{level} | {bm:#x} | {bs} | {bl:?} | {mem:#x} | {size} | {lean:?} | {tags}");
    }
    assert!(failed.is_empty(), "{failed:#?}");
}

/// Every level's starting profiles read with Halo 1's layout: each weapon
/// reference names a `weap` or none, counts are sane.
#[test]
#[ignore = "reads disc/halo1.iso"]
fn h1_starting_profiles_read() {
    for level in h1map::disc::CAMPAIGN_LEVELS {
        let h1 = h1_level(level);
        let ps = h1campaign::profiles::read(&h1).unwrap();
        assert!(!ps.is_empty(), "{level}: no starting profile");
        for p in &ps {
            for (w, loaded, total) in &p.weapons {
                if let Some(w) = w {
                    let t = h1.tag(*w).unwrap();
                    assert_eq!(&t.group.0.to_be_bytes(), b"weap", "{level}: {}", t.name);
                }
                assert!((0..=2000).contains(loaded) && (0..=2000).contains(total));
            }
            assert!(p.frags <= 8 && p.plasmas <= 8, "{level} {p:?}");
            println!(
                "{level} {:?}: health {} shield {} {:?} frags {} plasmas {}",
                p.name,
                p.health,
                p.shield,
                p.weapons
                    .iter()
                    .map(|(w, l, t)| (w.map(|w| h1.tag(w).unwrap().name.clone()), l, t))
                    .collect::<Vec<_>>(),
                p.frags,
                p.plasmas
            );
        }
    }
}

/// The converted map's starting profiles: one per Halo 1 profile, each
/// weapon Halo 1 names a `weap` of the map with data, named as Halo 1's
/// (a converted tag keeps the Halo 1 path) or a retail stand-in.
fn start_profiles(m: &mut M, h1: &h1map::H1Map) -> Vec<String> {
    let ps = h1campaign::profiles::read(h1).unwrap();
    let t = m.scenario().unwrap().clone();
    let addr = t.location.unwrap().address;
    let bytes = m.read_tag_bytes(t.id).unwrap();
    let (at, stride) = h1campaign::profiles::H2;
    let n = word(&bytes, at as usize) as usize;
    let mut bad = Vec::new();
    if n != ps.len() {
        bad.push(format!("{n} profiles for Halo 1's {}", ps.len()));
        return bad;
    }
    let p0 = word(&bytes, at as usize + 4).wrapping_sub(addr) as usize;
    for (k, p) in ps.iter().enumerate() {
        let e = &bytes[p0 + k * stride..p0 + (k + 1) * stride];
        for (i, (w, loaded, _)) in p.weapons.iter().enumerate() {
            let d = word(e, 0x2c + 0xc * i);
            let Some(w) = w else {
                if d != u32::MAX {
                    bad.push(format!("{}: weapon {i} where Halo 1 has none", p.name));
                }
                continue;
            };
            let h1_name = h1.tag(*w).unwrap().name.clone();
            match m.tag(map::TagId(d)) {
                Ok(t) if t.group == FourCC(*b"weap") && t.location.is_some() => {
                    println!(
                        "  profile {:?} weapon {i}: {h1_name} -> {:?} ({loaded} loaded)",
                        p.name, t.name
                    );
                }
                _ => bad.push(format!("{}: weapon {i} ({h1_name}) is {d:#x}", p.name)),
            }
        }
    }
    bad
}
