//! Tag layout schemas against the owner's retail maps. Opt-in: needs the
//! disc in `H2_DISC` (and optionally the map pack in `H2_MAPPACK`). Nothing
//! is written unless `H2_SCHEMA_OUT` names a file for the inferred schema.
//! `H2_SCHEMA_MAPS` picks maps by stem (comma-separated; default all).
//!
//! ```sh
//! H2_DISC=disc/halo2.iso H2_MAPPACK=disc/mappack.iso \
//!   cargo test --release -p tags --test tag_schema -- --ignored --nocapture
//! ```

use map::{FourCC, Map};
use std::collections::BTreeMap;
use std::fs::File;
use std::io::{BufReader, Cursor};
use tags::schema::infer::{MapSample, infer};
use tags::schema::{Memory, Refusal, Relocator, Schema, read_tree, relocated_variant};

type Res<T> = Result<T, Box<dyn std::error::Error>>;
type DiscMap = Map<BufReader<File>>;

/// Every `.map` on the disc and the map pack, as (stem, map).
fn maps() -> Res<Vec<(String, DiscMap)>> {
    let wanted: Option<Vec<String>> = std::env::var("H2_SCHEMA_MAPS")
        .ok()
        .map(|v| v.split(',').map(str::to_owned).collect());
    let mut out = Vec::new();
    for var in ["H2_DISC", "H2_MAPPACK"] {
        let Some(disc) = std::env::var_os(var) else {
            continue;
        };
        let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc)?))?;
        let mut entries = iso.entries()?;
        entries.sort_by(|a, b| a.path.cmp(&b.path));
        for e in entries {
            let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
            let Some(stem) = file.strip_suffix(".map") else {
                continue;
            };
            if e.is_dir
                || wanted
                    .as_ref()
                    .is_some_and(|w| !w.iter().any(|x| x == stem))
            {
                continue;
            }
            let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
            let map = Map::from_region(BufReader::new(File::open(&disc)?), base, e.size.into())?;
            out.push((stem.to_owned(), map));
        }
    }
    assert!(!out.is_empty(), "set H2_DISC (and H2_MAPPACK) to the ISOs");
    Ok(out)
}

/// Infer the schema from every map; write it to `H2_SCHEMA_OUT` if set.
#[test]
#[ignore = "needs the owner's ISOs"]
fn infer_from_retail_maps() -> Res<()> {
    let started = std::time::Instant::now();
    let mut samples = Vec::new();
    for (stem, mut map) in maps()? {
        samples.push(MapSample::read(&stem, &mut map)?);
    }
    let schema = infer(&mut samples);
    println!(
        "inferred {} groups from {} maps in {:?}",
        schema.groups.len(),
        samples.len(),
        started.elapsed()
    );
    let text = schema.to_text();
    assert_eq!(Schema::parse(&text)?, schema);
    if let Some(out) = std::env::var_os("H2_SCHEMA_OUT") {
        std::fs::write(out, text)?;
    }
    Ok(())
}

/// Per group: tags, relocatable, refusals by reason.
#[derive(Default)]
struct Coverage {
    tags: u32,
    ok: u32,
    refused: BTreeMap<Refusal, u32>,
    /// (reason, path, offset, value in the tag?) -> count
    places: BTreeMap<(Refusal, Vec<u32>, u32, bool), u32>,
}

fn coverage_of(map: &mut DiscMap, cov: &mut BTreeMap<FourCC, Coverage>) -> Res<Relocator<'static>> {
    let r = Relocator::new(Schema::builtin(), map)?;
    for t in map.tags().to_vec() {
        let c = cov.entry(t.group).or_default();
        c.tags += 1;
        match r.check(t.id) {
            Ok(_) => c.ok += 1,
            Err(d) => {
                let inside = t
                    .location
                    .is_some_and(|l| (l.address..l.address + l.size).contains(&d.value));
                *c.places
                    .entry((d.refusal.clone(), d.path, d.offset, inside))
                    .or_default() += 1;
                *c.refused.entry(d.refusal).or_default() += 1;
            }
        }
    }
    Ok(r)
}

/// Coverage report: per group, the share of tags the checked-in schema can
/// relocate, across all maps.
#[test]
#[ignore = "needs the owner's ISOs"]
fn coverage_report() -> Res<()> {
    let mut cov = BTreeMap::new();
    for (_, mut map) in maps()? {
        coverage_of(&mut map, &mut cov)?;
    }
    let (mut all, mut ok) = (0, 0);
    println!("group      tags  relocatable  refusals");
    for (g, c) in &cov {
        all += c.tags;
        ok += c.ok;
        println!(
            "{:<8} {:>6} {:>6} {:>5.1}%  {:?}",
            g.to_string(),
            c.tags,
            c.ok,
            100.0 * f64::from(c.ok) / f64::from(c.tags),
            c.refused
        );
        if std::env::var_os("H2_SCHEMA_WHY").is_some() {
            let mut places: Vec<_> = c.places.iter().collect();
            places.sort_by_key(|(_, n)| std::cmp::Reverse(**n));
            for ((r, path, off, inside), n) in places.into_iter().take(4) {
                println!("    {n:>6} {r:?} at {path:x?} +{off:#x} (value in tag: {inside})");
            }
        }
    }
    let full = cov.values().filter(|c| c.ok == c.tags).count();
    println!(
        "total: {ok}/{all} tags ({:.1}%), {full}/{} groups fully relocatable",
        100.0 * f64::from(ok) / f64::from(all),
        cov.len()
    );
    Ok(())
}

/// Shuffle: every relocatable ordinary-metadata tag of every map becomes a
/// built tree appended after the old metadata (new addresses, children in
/// the builder's depth-first order). The rebuilt map must read back with
/// the same index, every pinned tag byte-identical, every moved tag's
/// address-free tree identical, and the existing decoders giving the same
/// results apart from addresses.
#[test]
#[ignore = "needs the owner's ISOs"]
fn shuffle_every_relocatable_tag() -> Res<()> {
    let (mut failed, mut at_bound) = (Vec::new(), Vec::new());
    for (stem, mut map) in maps()? {
        match shuffle(&stem, &mut map) {
            Ok(()) => {}
            // Maps already at the bound (the largest of their type) have no
            // room for the few KiB of alignment padding a relayout adds.
            Err(e) if e.to_string().contains("tag memory") => {
                println!("{stem}: at the tag-memory bound: {e}");
                at_bound.push(stem);
            }
            Err(e) => {
                println!("{stem}: FAILED: {e}");
                failed.push(stem);
            }
        }
    }
    println!("at the tag-memory bound: {at_bound:?}");
    assert!(failed.is_empty(), "failed: {failed:?}");
    Ok(())
}

fn shuffle(stem: &str, map: &mut DiscMap) -> Res<()> {
    let started = std::time::Instant::now();
    let mut cov = BTreeMap::new();
    let relocator = coverage_of(map, &mut cov)?;
    // Freed space is reused, in reverse slot order so tags land elsewhere;
    // if fragmentation overflows the tag-memory bound, in slot order.
    let mut order = "reverse";
    let (ids, bytes) = match relocated_variant(map, true)
        .and_then(|(set, ids)| Ok((ids, set.build()?.0)))
    {
        Ok(r) => r,
        Err(e) => {
            println!("{stem}: reverse order: {e}");
            order = "slot";
            let (mut set, ids) = relocated_variant(map, false)?;
            match set.build() {
                Ok(b) => (ids, b.0),
                Err(e) => {
                    // How far over, and where the growth is.
                    set.check_tag_memory = false;
                    let over = Map::from_reader(Cursor::new(set.build()?.0))?;
                    let (mut before, mut after) = (0u64, 0u64);
                    for id in &ids {
                        before += u64::from(map.tag(*id)?.location.map_or(0, |l| l.size));
                        after += u64::from(over.tag(*id)?.location.map_or(0, |l| l.size));
                    }
                    println!(
                        "{stem}: slot order: {e}: memory {:#x} -> {:#x}, bound {:#x}; moved tags {before:#x} -> {after:#x} bytes",
                        map.header().tag_data_size,
                        over.header().tag_data_size,
                        map::writer::max_metadata_size(map.header().cache_type)?
                    );
                    return Err(e.into());
                }
            }
        }
    };
    let mut moved = Map::from_reader(Cursor::new(bytes))?;
    moved.validate_checksum()?;
    let new_mem = Memory::of(&mut moved)?;
    assert_eq!(moved.tags().len(), map.tags().len());
    assert_eq!(moved.strings(), map.strings());
    let schema = Schema::builtin();
    let moving: std::collections::HashSet<_> = ids.iter().copied().collect();
    let (mut same_decode, mut decoded, mut stayed) = (0, 0, 0);
    for t in map.tags().to_vec() {
        let u = moved.tag(t.id)?.clone();
        assert_eq!((t.group, &t.name), (u.group, &u.name));
        let (Some(a), Some(b)) = (t.location, u.location) else {
            assert_eq!(t.location.is_some(), u.location.is_some());
            continue;
        };
        if !moving.contains(&t.id) {
            let before = map.read_tag_bytes(t.id)?;
            let after = moved.read_tag_bytes(t.id)?;
            assert_eq!(a.address, b.address, "{} pinned tag moved", t.id);
            assert!(before == after, "{} pinned bytes changed", t.id);
            continue;
        }
        if a.address == b.address {
            stayed += 1;
        }
        let root = &schema.groups[&t.group];
        let x = read_tree(root, a.address, relocator.memory()).ok_or("old tree")?;
        let y = read_tree(root, b.address, &new_mem).ok_or("new tree")?;
        if x != y {
            return Err(format!("{} {} tree differs", t.group, t.id).into());
        }
        if tags::DecodedTag::supports(t.group) {
            decoded += 1;
            let p = tags::DecodedTag::read(map, t.id).map(|d| masked(&d.data));
            let q = tags::DecodedTag::read(&mut moved, t.id).map(|d| masked(&d.data));
            match (p, q) {
                (Ok(p), Ok(q)) if p == q => same_decode += 1,
                (Err(_), Err(_)) => same_decode += 1,
                _ => return Err(format!("{} {} decodes differently", t.group, t.id).into()),
            }
        }
    }
    println!(
        "{stem}: rebuilt {}/{} tags in {order} order ({stayed} at their old address), \
         trees identical, {same_decode}/{decoded} decoder results identical ({:?})",
        ids.len(),
        map.tags().len(),
        started.elapsed()
    );
    Ok(())
}

/// Debug form of decoded data with every address-looking hex value masked.
fn masked(data: &tags::TagData) -> String {
    let text = format!("{data:?}");
    let mut out = String::with_capacity(text.len());
    let mut rest = text.as_str();
    while let Some(i) = rest.find("address: ") {
        out.push_str(&rest[..i + 9]);
        rest = &rest[i + 9..];
        let end = rest
            .find(|c: char| !(c.is_ascii_alphanumeric()))
            .unwrap_or(rest.len());
        out.push('_');
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// Print root words of one group: `H2_SCHEMA_PROBE=hlmt:0x58`.
#[test]
#[ignore = "needs the owner's ISOs"]
fn probe() -> Res<()> {
    let Ok(spec) = std::env::var("H2_SCHEMA_PROBE") else {
        return Ok(());
    };
    let (g, o) = spec.split_once(':').ok_or("group:offset")?;
    let g: FourCC = g.parse()?;
    let o = usize::from_str_radix(o.trim_start_matches("0x"), 16)?;
    let mut shown = 0;
    for (stem, mut map) in maps()? {
        let space = map.metadata_space();
        for t in map.tags().to_vec() {
            if t.group != g || shown > 40 {
                continue;
            }
            let Some(loc) = t.location else { continue };
            let b = map.read_tag_bytes(t.id)?;
            let w = |k: usize| u32::from_le_bytes(b[k..k + 4].try_into().unwrap());
            if b.len() < o + 12 {
                continue;
            }
            let owner = |v: u32| {
                map.tags()
                    .iter()
                    .find(|u| {
                        u.location
                            .is_some_and(|l| (l.address..l.address + l.size).contains(&v))
                    })
                    .map(|u| format!("{} {}", u.group, u.name.clone().unwrap_or_default()))
            };
            let odd = std::env::var_os("H2_SCHEMA_ODD").is_some();
            if odd && (w(o) == 0 || owner(w(o)).is_some()) {
                continue;
            }
            println!(
                "{stem} {:?} @{:#x}+{:#x}: {:#x} {:#x} {:#x} {:#x}  -> {:?} (meta {:x?})",
                t.name,
                loc.address,
                loc.size,
                w(o - 4),
                w(o),
                w(o + 4),
                w(o + 8),
                owner(w(o)),
                space.map(|s| (s.virtual_base, s.size))
            );
            shown += 1;
        }
    }
    Ok(())
}
