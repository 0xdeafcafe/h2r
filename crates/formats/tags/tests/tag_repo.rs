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
use std::io::{BufReader, Read, Seek, SeekFrom};
use tags::repo::extract::{Layouts, MapWork, SharedFiles, compare, file_hash, update};
use tags::repo::store::PutReport;
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

/// Extract maps into a repository with the plan and update `h2 tags
/// extract` runs (each file read back and decoded the same), check that a
/// second plan (`--update`) has nothing to do, then rebuild each non-shared map from the repository alone by
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
    // The disc's identity: each map file's SHA-256.
    let mut disc = Vec::new();
    for (stem, base, size) in maps.iter().copied() {
        let mut f = File::open(std::env::var_os("H2_DISC").ok_or("set H2_DISC")?)?;
        f.seek(SeekFrom::Start(*base))?;
        disc.push((stem.clone(), file_hash(f.take(*size))?));
    }
    println!("hashed {} map files in {:?}", disc.len(), started.elapsed());
    let layouts = Layouts::new(schema);
    let extract = tags::repo::plan(repo, &layouts, &disc, true)?;
    let mut lossy = Vec::new();
    let mut t = std::time::Instant::now();
    let mut opener = |stem: &str| -> tags::repo::Result<DiscMap> {
        let (_, base, size) = maps
            .iter()
            .find(|m| m.0 == stem)
            .ok_or_else(|| tags::repo::Error::Invalid(format!("no {stem}")))?;
        open(*base, *size).map_err(|e| tags::repo::Error::Invalid(e.to_string()))
    };
    let mut on_map = |w: &MapWork, r: &PutReport| {
        println!(
            "{}: {} new, {} same, {} variants, {} lossy, {} refused, raw {} new ({} MB), {:?}",
            w.map,
            r.new,
            r.same,
            r.variants.len(),
            r.lossy.len(),
            r.refused.len(),
            r.raw_new.0,
            r.raw_new.1 >> 20,
            t.elapsed()
        );
        if let Some(k) = &r.skeleton {
            println!(
                "  skeleton {} KB ({} KB compressed), {} named extents ({} MB), {} unnamed ({} KB)",
                k.skeleton.0 >> 10,
                k.skeleton.1 >> 10,
                k.named.0,
                k.named.1 >> 20,
                k.unnamed.0,
                k.unnamed.1 >> 10
            );
        }
        for (k, why) in r.refused.iter().take(5) {
            println!("  refused {k}: {why}");
        }
        for k in r.lossy.iter().take(3) {
            lossy.push((w.map.clone(), k.clone()));
        }
        t = std::time::Instant::now();
    };
    update(
        repo,
        &extract,
        &layouts,
        schema,
        &mut sh,
        &mut opener,
        true,
        &mut on_map,
    )?;
    for (stem, k) in &lossy {
        let tag = TagRepo::for_map(&dir, stem)?.read(k)?.ok_or("lossy tag")?;
        println!("  lossy {stem} {k}: {:?}", tag.tree().map(|t| &t.losses));
    }
    // Acceptance for `h2 tags extract --update`: the same disc under the
    // same build plans nothing.
    let again = tags::repo::plan(repo, &layouts, &disc, false)?;
    assert!(again.is_empty(), "--update after an extraction: {again:?}");
    // Every map is given back byte for byte by its skeleton and extents
    // (docs/engine/asset-store.md, AS1; `update` checked each as it went).
    if !count_only {
        for (stem, hash) in &disc {
            repo.check_reassembly(stem, hash)?;
        }
        println!("{} maps reassemble to the disc's SHA-256", disc.len());
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
        // A tag the disc map lacks, added onto it: a copy of one of its
        // damage tags under a new path.
        let r = TagRepo::for_map(&dir, stem)?;
        let copy_of = r
            .keys()
            .into_iter()
            .find(|k| k.group == map::FourCC(*b"jpt!"))
            .ok_or("no damage tag")?;
        let new = tags::repo::TagKey::new(copy_of.group, "invented\\added\\copy");
        let mut src = Adding {
            src: r,
            copy_of,
            new: new.clone(),
        };
        let (set, added) = tags::repo::build::onto_retail_adding(
            schema,
            open(*base, *size)?,
            &mut src,
            std::slice::from_ref(&new),
        )?;
        assert_eq!(added.added, vec![new.clone()]);
        assert!(set.find(new.group, &new.path).is_some());
        let (bytes, _) = set.build()?;
        Map::from_reader(std::io::Cursor::new(bytes))?;
        println!("{stem} onto the disc map with {new} added: reads back");
    }
    assert_eq!(failures, 0);
    Ok(())
}

/// A source with one tag more: `new`, a copy of `copy_of`.
struct Adding {
    src: TagRepo,
    copy_of: tags::repo::TagKey,
    new: tags::repo::TagKey,
}

impl TagSource for Adding {
    fn keys(&self) -> Vec<tags::repo::TagKey> {
        let mut k = self.src.keys();
        k.push(self.new.clone());
        k
    }

    fn read(
        &mut self,
        key: &tags::repo::TagKey,
    ) -> tags::repo::Result<Option<tags::repo::LooseTag>> {
        if *key == self.new {
            let mut t = self.src.read(&self.copy_of)?;
            if let Some(t) = &mut t {
                t.key = self.new.clone();
            }
            return Ok(t);
        }
        self.src.read(key)
    }

    fn raw(&mut self, raw: &tags::repo::RawRes) -> tags::repo::Result<Vec<u8>> {
        self.src.raw(raw)
    }

    fn groups(&self) -> Vec<map::TagGroup> {
        self.src.groups()
    }
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
