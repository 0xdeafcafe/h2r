//! The tag repository against the owner's disc. Opt-in: `H2_DISC` names the
//! ISO; `H2_REPO_MAPS` picks maps (default `lockout,01a_tutorial`; `all` for
//! every map); `H2_REPO_DIR` keeps the repository (default: a temporary
//! directory, deleted afterwards). Nothing is written in the source tree.
//!
//! ```sh
//! H2_DISC=disc/halo2.iso cargo test --release -p tags --test tag_repo -- --ignored --nocapture
//! ```

use map::Map;
use std::fs::File;
use std::io::BufReader;
use tags::repo::extract::{SharedFiles, compare, store_map};
use tags::repo::{MapSource, TagRepo, TagSource};
use tags::schema::Schema;

type Res<T> = Result<T, Box<dyn std::error::Error>>;
type DiscMap = Map<BufReader<File>>;

/// Every map on the disc, (stem, opener).
fn disc_maps() -> Res<Vec<(String, u64, u64)>> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC")?;
    let mut iso = xiso::Xiso::open(BufReader::new(File::open(&disc)?))?;
    let mut out = Vec::new();
    for e in iso.entries()? {
        let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
        if let Some(stem) = file.strip_suffix(".map")
            && !e.is_dir
        {
            let base = iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR;
            out.push((stem.to_owned(), base, u64::from(e.size)));
        }
    }
    out.sort();
    Ok(out)
}

fn open(base: u64, size: u64) -> Res<DiscMap> {
    let disc = std::env::var_os("H2_DISC").ok_or("set H2_DISC")?;
    Ok(Map::from_region(
        BufReader::new(File::open(disc)?),
        base,
        size,
    )?)
}

fn shared(schema: &Schema, maps: &[(String, u64, u64)]) -> Res<SharedFiles<BufReader<File>>> {
    let mut s = SharedFiles::default();
    for (stem, base, size) in maps {
        let file = match stem.as_str() {
            "mainmenu" => map::ResourceFile::MainMenu,
            "shared" => map::ResourceFile::Shared,
            "single_player_shared" => map::ResourceFile::SinglePlayerShared,
            _ => continue,
        };
        s.set(schema, file, open(*base, *size)?)?;
    }
    Ok(s)
}

fn dir_size(p: &std::path::Path) -> u64 {
    let mut n = 0;
    if let Ok(rd) = std::fs::read_dir(p) {
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                n += dir_size(&e.path());
            } else {
                n += e.metadata().map(|m| m.len()).unwrap_or(0);
            }
        }
    }
    n
}

/// Extract maps into a repository (each file read back and decoded the
/// same), then rebuild each non-shared map from the repository alone by
/// closure from its scenario, and compare every tag with the disc's.
#[test]
#[ignore = "needs the owner's disc"]
fn extract_and_rebuild_from_the_repository() -> Res<()> {
    let _ = Schema::builtin();
    let all = disc_maps()?;
    let wanted = std::env::var("H2_REPO_MAPS").unwrap_or_else(|_| "lockout,01a_tutorial".into());
    let order = |s: &str| match s {
        "mainmenu" | "shared" | "single_player_shared" => 0,
        _ => 1,
    };
    let mut maps: Vec<&(String, u64, u64)> = all
        .iter()
        .filter(|(s, _, _)| wanted == "all" || wanted.split(',').any(|w| w == s))
        .collect();
    maps.sort_by_key(|(s, _, _)| (order(s), s.clone()));
    let keep = std::env::var_os("H2_REPO_DIR");
    let dir = keep.clone().map_or_else(
        || std::env::temp_dir().join(format!("h2-tagrepo-{}", std::process::id())),
        Into::into,
    );
    let mut repo = TagRepo::create(&dir)?;
    // H2_REPO_COUNT_RAW=1: size raw resources without writing them (and
    // skip the rebuild, which needs them).
    let count_only = std::env::var_os("H2_REPO_COUNT_RAW").is_some();
    if count_only {
        repo.count_raw = Some(Default::default());
    }
    let result = run(&mut repo, &dir, &maps, &all, count_only);
    if keep.is_none() {
        std::fs::remove_dir_all(&dir)?;
    }
    result
}

fn run(
    repo: &mut TagRepo,
    dir: &std::path::Path,
    maps: &[&(String, u64, u64)],
    all: &[(String, u64, u64)],
    count_only: bool,
) -> Res<()> {
    let schema = Schema::builtin();
    let order = |s: &str| match s {
        "mainmenu" | "shared" | "single_player_shared" => 0,
        _ => 1,
    };
    let dir = dir.to_path_buf();
    let mut sh = shared(schema, all)?;
    let started = std::time::Instant::now();
    for (stem, base, size) in maps.iter().copied() {
        let t = std::time::Instant::now();
        let mut src = MapSource::new(schema, open(*base, *size)?, &mut sh)?;
        let r = store_map(repo, &mut src, stem, true)?;
        println!(
            "{stem}: {} new, {} same, {} variants, {} lossy, {} refused, raw {} new ({} MB), {:?}",
            r.new,
            r.same,
            r.variants.len(),
            r.lossy.len(),
            r.refused.len(),
            r.raw_new.0,
            r.raw_new.1 >> 20,
            t.elapsed()
        );
        for (k, why) in r.refused.iter().take(5) {
            println!("  refused {k}: {why}");
        }
        for k in r.lossy.iter().take(3) {
            let tag = TagRepo::for_map(&dir, stem)?.read(k)?.ok_or("lossy tag")?;
            println!("  lossy {k}: {:?}", tag.tree().map(|t| &t.losses));
        }
    }
    let mut groups = std::collections::BTreeMap::<String, (usize, usize)>::new();
    for m in maps {
        let man = repo.manifest(&m.0)?;
        for (k, v) in &man.tags {
            let e = groups.entry(k.group.to_string()).or_default();
            e.0 += 1;
            e.1 += usize::from(v.is_some());
        }
    }
    let varied: Vec<String> = groups
        .iter()
        .filter(|(_, v)| v.1 > 0)
        .map(|(g, v)| format!("{g} {}/{}", v.1, v.0))
        .collect();
    println!(
        "variants by group (variant/instances): {}",
        varied.join(", ")
    );
    println!(
        "extracted {} maps in {:?}; repository {} MB on disk{}",
        maps.len(),
        started.elapsed(),
        dir_size(&dir) >> 20,
        repo.count_raw
            .as_ref()
            .map_or(String::new(), |(n, b)| format!(
                " + {} raw resources ({} MB) counted, not written",
                n.len(),
                b >> 20
            ))
    );
    if count_only {
        return Ok(());
    }
    let mut failures = 0;
    for (stem, base, size) in maps.iter().copied() {
        if order(stem) == 0 {
            continue;
        }
        let m = repo.manifest(stem)?;
        let Some(scenario) = m.scenario.clone() else {
            continue;
        };
        let t = std::time::Instant::now();
        let (set, report, closure) =
            tags::repo::build::from_repo(&dir, &scenario.path, Some(stem))?;
        let (bytes, layout) = set.build()?;
        let built = Map::from_reader(std::io::Cursor::new(bytes))?;
        let mut none = SharedFiles::default();
        none.unread_ok = true;
        let mut b = MapSource::new(schema, built, &mut none)?;
        let mut s = MapSource::new(schema, open(*base, *size)?, &mut sh)?;
        let c = compare(&mut b, &mut s)?;
        let retail = s.tags().len();
        let in_closure: std::collections::HashSet<_> =
            closure.tags.iter().map(|t| t.key.clone()).collect();
        let unreached: Vec<_> = s
            .keys()
            .into_iter()
            .filter(|k| !in_closure.contains(k))
            .collect();
        println!(
            "{stem}: rebuilt {} tags ({} retail, {} not reached), {} bsp regions, {} raw ({} MB) copied, {} external; file {} MB (retail {} MB), metadata {:#x}; same {}, differ {}, missing {}, refused {}, raw compared {}; dangling {}, {:?}",
            report.tags,
            retail,
            unreached.len(),
            report.bsps,
            report.raw.0,
            report.raw.1 >> 20,
            report.external,
            layout.file_size >> 20,
            size >> 20,
            layout.memory_size,
            c.same,
            c.differ.len(),
            c.missing.len(),
            c.refused.len(),
            c.raw_compared,
            report.dangling.len(),
            t.elapsed()
        );
        let mut by_group = std::collections::BTreeMap::<String, usize>::new();
        for k in &unreached {
            *by_group.entry(k.group.to_string()).or_default() += 1;
        }
        println!("  not reached by group: {by_group:?}");
        for (k, d) in c.differ.iter().take(8) {
            println!("  differs {k}: {d}");
        }
        for (k, d) in c.refused.iter().take(5) {
            println!("  refused {k}: {d}");
        }
        for k in &c.lossy {
            println!("  lossy {k}");
        }
        failures += c.differ.len() + c.missing.len();
        // The same tags onto the disc map's own file (BSP regions and
        // lossy tags pinned).
        let mut r = TagRepo::for_map(&dir, stem)?;
        let (set, onto) = tags::repo::build::onto_retail(schema, open(*base, *size)?, &mut r)?;
        let (bytes, layout) = set.build()?;
        let built = Map::from_reader(std::io::Cursor::new(bytes))?;
        let mut none = SharedFiles::default();
        none.unread_ok = true;
        let mut b = MapSource::new(schema, built, &mut none)?;
        let mut s = MapSource::new(schema, open(*base, *size)?, &mut sh)?;
        let c = compare(&mut b, &mut s)?;
        println!(
            "{stem} onto the disc map: {} tags from the repository ({} aliases), {} pinned ({} lossy, {} missing), {} raw copied, {} raw pointers into the file kept; file {} MB; same {}, differ {}, missing {}, refused {}",
            onto.replaced,
            onto.aliases,
            onto.pinned,
            onto.lossy.len(),
            onto.missing.len(),
            onto.build.raw.0,
            onto.build.external,
            layout.file_size >> 20,
            c.same,
            c.differ.len(),
            c.missing.len(),
            c.refused.len()
        );
        for (k, d) in c.differ.iter().take(8) {
            println!("  differs {k}: {d}");
        }
        failures += c.differ.len() + c.missing.len();
    }
    assert_eq!(failures, 0);
    Ok(())
}

/// Print a BSP tag's root block (`H2_PROBE=map:offset`), for undescribed
/// fields.
#[test]
#[ignore = "needs the owner's disc"]
fn probe_bsp_block() -> Res<()> {
    let spec = std::env::var("H2_PROBE").unwrap_or_else(|_| "lockout:138".into());
    let (stem, off) = spec.split_once(':').ok_or("map:offset")?;
    let off = u32::from_str_radix(off, 16)?;
    let (_, base, size) = disc_maps()?
        .into_iter()
        .find(|m| m.0 == stem)
        .ok_or("no map")?;
    let mut m = open(base, size)?;
    let meta = m.metadata_space();
    for t in m.tags().to_vec() {
        let Some(l) = t
            .location
            .filter(|l| Some(l.space) != meta && t.group.0 == *b"sbsp")
        else {
            continue;
        };
        let bytes = m.read_tag_bytes(t.id)?;
        let w = |o: u32| u32::from_le_bytes(bytes[o as usize..o as usize + 4].try_into().unwrap());
        let (c, a) = (w(off), w(off + 4));
        println!(
            "{:?} at {:#x} size {:#x} space {:#x}+{:#x}: [{off:#x}] count {c} address {a:#x}",
            t.name, l.address, l.size, l.space.virtual_base, l.space.size
        );
        let at = (a - l.address) as usize;
        for row in bytes[at..(at + 0x120).min(bytes.len())].chunks(16) {
            println!(
                "  {}",
                row.iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
    }
    Ok(())
}

/// Why variants differ: for a kept repository (`H2_REPO_DIR`) and a map
/// (`H2_PROBE_MAP`), the first differing canonical line of a few variants
/// per group.
#[test]
#[ignore = "needs a repository"]
fn probe_variants() -> Res<()> {
    let dir = std::env::var_os("H2_REPO_DIR").ok_or("set H2_REPO_DIR")?;
    let map = std::env::var("H2_PROBE_MAP")?;
    let main = TagRepo::open(&dir)?;
    let mut per_map = TagRepo::for_map(&dir, &map)?;
    let m = main.manifest(&map)?;
    let mut shown = std::collections::BTreeMap::<String, usize>::new();
    for (k, v) in &m.tags {
        if v.is_none() {
            continue;
        }
        let n = shown.entry(k.group.to_string()).or_default();
        if *n >= 3 {
            continue;
        }
        *n += 1;
        let a = tags::repo::canonical(&TagRepo::open(&dir)?.read(k)?.ok_or("main")?);
        let b = tags::repo::canonical(&per_map.read(k)?.ok_or("variant")?);
        let i = a
            .iter()
            .zip(&b)
            .position(|(x, y)| x != y)
            .unwrap_or(a.len().min(b.len()));
        let cut =
            |s: Option<&String>| s.map_or("(end)".to_owned(), |s| s.chars().take(300).collect());
        println!(
            "{k} line {i} of {}/{}:\n  main    {}\n  variant {}",
            a.len(),
            b.len(),
            cut(a.get(i)),
            cut(b.get(i))
        );
    }
    Ok(())
}
