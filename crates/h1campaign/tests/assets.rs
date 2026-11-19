//! Both variants of every campaign level (docs/halo1/campaign.md §5): Halo
//! 1's assets and Halo 2's, built as the import writes them. From the
//! owner's discs (`$H1_ISO`, `$H2_DISC`); nothing is written to disk.
//! `H1_LEVELS=a30,b40` picks levels.
//!
//! ```text
//! H1_ISO=disc/halo1.iso H2_DISC=disc/halo2.iso cargo test --release \
//!   -p h1campaign --test assets -- --ignored --nocapture
//! ```

use std::fs::File;
use std::io::{BufReader, Cursor};

use h1campaign::h2assets::{Assets, Swap, swap_for};
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

fn h1_level(name: &str) -> h1map::H1Map {
    let mut iso = xiso::Xiso::open(BufReader::new(
        File::open(disc("H1_ISO", "halo1.iso")).unwrap(),
    ))
    .unwrap();
    let d = h1map::disc::Disc::open(&mut iso).unwrap();
    let e = d.map(name).unwrap().clone();
    h1map::H1Map::from_reader(Cursor::new(iso.read(&e).unwrap())).unwrap()
}

fn word(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
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
/// map with data, of the atom type's group. Returns (atoms, resolved
/// object definitions, problems).
fn script_tags(m: &mut M, catalog: &hs::Catalog) -> (usize, usize, Vec<String>) {
    let s = hs::Scenario::read(m).expect("the scripts read, references checked");
    let (mut n, mut defs, mut bad) = (0, 0, Vec::new());
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
        defs += usize::from(ty == "object_definition");
        match m.tag(map::TagId(d)) {
            Ok(t) if t.location.is_some() && groups.contains(&t.group) => {}
            Ok(t) => bad.push(format!("{ty} atom names {} {:?}", t.group, t.name)),
            Err(_) => bad.push(format!("{ty} atom {d:#x} names no tag")),
        }
    }
    (n, defs, bad)
}

/// The tags of the scenario's weapon, equipment and vehicle palettes and
/// the starting profiles' weapons, by name.
fn placed_names(m: &mut M) -> Vec<(FourCC, String)> {
    let t = m.scenario().unwrap().clone();
    let addr = t.location.unwrap().address;
    let bytes = m.read_tag_bytes(t.id).unwrap();
    let mut out = Vec::new();
    let mut name = |d: u32| {
        if let Ok(t) = m.tag(map::TagId(d)) {
            out.push((t.group, t.name.clone().unwrap_or_default()));
        }
    };
    // Palettes (`tags::object::OBJECT_TYPES`): vehicles, equipment, weapons.
    for ty in [1usize, 2, 3] {
        let (_, pal, _) = tags::object::OBJECT_TYPES[ty].placement.unwrap();
        let n = word(&bytes, pal as usize) as usize;
        let p = word(&bytes, pal as usize + 4).wrapping_sub(addr) as usize;
        for k in 0..n {
            name(word(&bytes, p + k * h1campaign::scenario::PALETTE + 4));
        }
    }
    let (at, stride) = h1campaign::profiles::H2;
    let n = word(&bytes, at as usize) as usize;
    let p = word(&bytes, at as usize + 4).wrapping_sub(addr) as usize;
    for k in 0..n {
        for i in 0..4 {
            name(word(&bytes, p + k * stride + 0x2c + 0xc * i));
        }
    }
    out
}

/// Both variants of every level, as the import builds them: each built
/// from scratch with every reference resolving (0 dangling) and every
/// script tag atom naming a tag of its group. The Halo 2-assets variant
/// places only retail Halo 2 tags where the table swaps (no Halo 1 weapon,
/// vehicle or grenade the table names is placed or given), makes Halo 2's
/// Chief the player, keeps the AI's characters and resolves the scripts'
/// object definitions through the swaps.
#[test]
#[ignore = "reads disc/halo1.iso and disc/halo2.iso"]
fn every_level_builds_in_both_assets() {
    let (xbe, scenarios) = h2_scripts();
    let s = scripting(&xbe, &scenarios);
    let mut catalog = hs::Catalog::from_xbe(&xbe::Xbe::parse(&xbe).unwrap()).unwrap();
    for sc in &scenarios {
        catalog.observe(sc).unwrap();
    }
    drop(scenarios);
    let donors = donors();
    let only = std::env::var("H1_LEVELS").ok();
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
        let mut chars = Vec::new();
        for assets in [Assets::Halo1, Assets::Halo2] {
            let mut base = h2_map(h1campaign::base_for(level));
            let opts = h1campaign::Options {
                scripts: Some(&s),
                sounds: usize::MAX,
                donors: Some(&donors),
                assets,
                ..Default::default()
            };
            let what = format!("{level} {assets:?}");
            let done = match h1campaign::convert(&h1, &mut base, &opts) {
                Ok(d) => d,
                Err(e) => {
                    println!("{what}: FAILED: {e}");
                    failed.push(format!("{what}: {e}"));
                    continue;
                }
            };
            let r = &done.report;
            println!("== {what}");
            print!("{r}");
            let mut m = Map::from_reader(Cursor::new(done.bytes.clone())).unwrap();
            m.validate_checksum().unwrap();
            let mem = tags::schema::Memory::of(&mut m).unwrap();
            if level == "a30" {
                let pod = m.tag(map::TagQuery::NameInGroup(
                    r"vehicles\lifepod_entry\lifepod_entry", FourCC(*b"vehi"),
                )).unwrap();
                let field = pod.location.unwrap().address + h1objects::convert::h2::ATTACHMENTS as u32;
                let block = mem.slice(field, 8).unwrap();
                let count = word(block, 0);
                let stride = h1objects::convert::h2::ATTACHMENT as u32;
                let entries = mem.slice(word(block, 4), count * stride).unwrap();
                let loops: Vec<_> = entries.chunks_exact(stride as usize)
                    .filter(|row| word(row, 0) == FourCC(*b"lsnd").as_u32())
                    .map(|row| m.tag(word(row, 4)).unwrap().name.as_deref().unwrap())
                    .collect();
                assert_eq!(loops, [r"sound\sfx\vehicles\lifeboat"], "{what}");
            }
            let (refs, bad) = reference_sweep(&m, &mem);
            let (atoms, defs, script_bad) = script_tags(&mut m, &catalog);
            println!(
                "  {}/{refs} references resolve; {atoms} script tag atoms ({defs} object definitions), {} unresolved",
                refs - bad.len(),
                script_bad.len()
            );
            if !bad.is_empty() {
                failed.push(format!(
                    "{what}: {} dangling: {:#?}",
                    bad.len(),
                    &bad[..bad.len().min(12)]
                ));
            }
            if !script_bad.is_empty() {
                failed.push(format!(
                    "{what}: script atoms {:#?}",
                    &script_bad[..script_bad.len().min(12)]
                ));
            }
            if r.scratch.is_none() || !r.fresh {
                failed.push(format!("{what}: not built from scratch: {:?}", r.dropped));
            }
            let ai: Vec<(String, String)> =
                r.scenario.ai.as_ref().map(|a| a.characters.clone()).unwrap_or_default();
            if assets == Assets::Halo2 {
                // Every swapped Halo 1 item is gone from what is placed.
                let placed = placed_names(&mut m);
                let halo1: Vec<&(FourCC, String)> = placed
                    .iter()
                    .filter(|(g, n)| {
                        matches!(swap_for(&g.0, n), Swap::Retail(_)) && !n.starts_with("objects\\")
                    })
                    .collect();
                if !halo1.is_empty() {
                    failed.push(format!("{what}: Halo 1 items still placed: {halo1:?}"));
                }
                let player = r.scenario.player_biped.clone().unwrap_or_default();
                if !player.ends_with("\\masterchief") {
                    failed.push(format!("{what}: player {player:?}"));
                }
                // The AI's characters are the same retail ones in both.
                if ai != chars {
                    failed.push(format!("{what}: AI characters differ: {ai:?} vs {chars:?}"));
                }
                println!(
                    "  swapped {}: {:?}",
                    r.scenario.swapped.len(),
                    r.scenario
                        .swapped
                        .iter()
                        .map(|s| s.0.as_str())
                        .collect::<Vec<_>>()
                );
            } else {
                chars = ai;
            }
            rows.push((what, r.memory, done.bytes.len(), m.tags().len()));
        }
    }
    println!("level | tag memory | file | tags");
    for (what, mem, size, tags) in &rows {
        println!("{what} | {mem:#x} | {size} | {tags}");
    }
    assert!(failed.is_empty(), "{failed:#?}");
}
