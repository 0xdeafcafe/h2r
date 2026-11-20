//! `h2 tags extract` and `h2 tags build`: the loose tag repository
//! (docs/formats/tag-repo.md).
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::{Map, ResourceFile};
use tags::repo::{MapSource, SharedFiles, TagRepo, compare, store_map};
use tags::schema::Schema;

use crate::safe_out::{self, OutDir};

const USAGE: &str = "usage:
  h2 tags extract [disc.iso|file.map ...] --out REPO [--maps a,b] [--verify]
      every tag of every map into REPO (one file per tag, raw by hash);
      identical tags are stored once, others as variants (see the report)
  h2 tags build REPO SCENARIO --out FILE.map [--map NAME] [--compare disc.iso] [--onto disc.iso] [--force]
      the map of SCENARIO (e.g. scenarios\\multi\\lockout\\lockout) from REPO
      alone, by closure from the scenario and the globals; header values,
      variants and the low-detail region come from map NAME's manifest
      (default: the map whose scenario it is); --compare reads the result
      back and compares every tag with the disc's map; --onto keeps the
      disc map's file, BSP regions and the tags the repository cannot give
      exactly, and builds every other tag from REPO (a safer load test)";

type DiscMap = Map<BufReader<File>>;

/// Whether `args` (after `h2 tags`) is one of these subcommands.
pub(crate) fn handles(args: &[String]) -> bool {
    matches!(args.first().map(String::as_str), Some("extract" | "build"))
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("extract") => extract(&args[1..]),
        Some("build") => build(&args[1..]),
        _ => bail!("{USAGE}"),
    }
}

/// A map file inside an ISO or on its own: (stem, path, base, length).
#[derive(Clone)]
struct Source {
    stem: String,
    path: PathBuf,
    base: u64,
    len: u64,
}

impl Source {
    fn open(&self) -> Result<DiscMap> {
        Map::from_region(BufReader::new(File::open(&self.path)?), self.base, self.len)
            .with_context(|| format!("parsing {}", self.stem))
    }
}

fn sources(inputs: &[PathBuf]) -> Result<Vec<Source>> {
    let mut out = Vec::new();
    for p in inputs {
        safe_out::protect(p);
        let is_map = p.extension().is_some_and(|e| e.eq_ignore_ascii_case("map"));
        if is_map {
            let stem = p
                .file_stem()
                .context("map file name")?
                .to_string_lossy()
                .into_owned();
            let len = std::fs::metadata(p)?.len();
            out.push(Source {
                stem,
                path: p.clone(),
                base: 0,
                len,
            });
            // Shared caches next to a lone map.
            for s in ["mainmenu", "shared", "single_player_shared"] {
                let q = p.with_file_name(format!("{s}.map"));
                if q != *p && q.exists() && !out.iter().any(|o: &Source| o.stem == s) {
                    let len = std::fs::metadata(&q)?.len();
                    out.push(Source {
                        stem: s.into(),
                        path: q,
                        base: 0,
                        len,
                    });
                }
            }
            continue;
        }
        let mut iso = xiso::Xiso::open(BufReader::new(
            File::open(p).with_context(|| format!("opening {}", p.display()))?,
        ))?;
        for e in iso.entries()? {
            let file = e.path.rsplit(['/', '\\']).next().unwrap_or("");
            let Some(stem) = file.strip_suffix(".map") else {
                continue;
            };
            if e.is_dir || out.iter().any(|o: &Source| o.stem == stem) {
                continue;
            }
            out.push(Source {
                stem: stem.to_owned(),
                path: p.clone(),
                base: iso.partition_offset() + u64::from(e.sector) * xiso::SECTOR,
                len: e.size.into(),
            });
        }
    }
    Ok(out)
}

fn shared_file(stem: &str) -> Option<ResourceFile> {
    match stem {
        "mainmenu" => Some(ResourceFile::MainMenu),
        "shared" => Some(ResourceFile::Shared),
        "single_player_shared" => Some(ResourceFile::SinglePlayerShared),
        _ => None,
    }
}

fn shared(schema: &Schema, all: &[Source]) -> Result<SharedFiles<BufReader<File>>> {
    let mut s = SharedFiles::default();
    for src in all {
        if let Some(f) = shared_file(&src.stem) {
            s.set(schema, f, src.open()?)?;
        }
    }
    Ok(s)
}

fn extract(args: &[String]) -> Result<()> {
    let mut inputs = Vec::new();
    let mut out = None;
    let mut wanted: Option<Vec<String>> = None;
    let mut verify = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().context("--out needs a directory")?)),
            "--maps" => {
                wanted = Some(
                    it.next()
                        .context("--maps needs names")?
                        .split(',')
                        .map(|s| s.trim_end_matches(".map").to_owned())
                        .collect(),
                )
            }
            "--verify" => verify = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            a if a.starts_with('-') => bail!("unknown option {a}\n{USAGE}"),
            a => inputs.push(PathBuf::from(a)),
        }
    }
    if inputs.is_empty() {
        inputs.push(super::default_disc()?);
    }
    let out = out.context(format!("--out is required\n{USAGE}"))?;
    for i in &inputs {
        safe_out::guard_target(&out, Some(i))?;
    }
    let all = sources(&inputs)?;
    let schema = Schema::builtin();
    let mut maps: Vec<&Source> = all
        .iter()
        .filter(|s| wanted.as_ref().is_none_or(|w| w.contains(&s.stem)))
        .collect();
    // Shared caches first, so their tags are the main contents.
    maps.sort_by_key(|s| (shared_file(&s.stem).is_none(), s.stem.clone()));
    if maps.is_empty() {
        bail!("no maps to extract");
    }
    let mut repo = TagRepo::create(&out)?;
    let mut sh = shared(schema, &all)?;
    let mut totals = (0usize, 0usize, 0usize, 0usize, 0usize, 0u64);
    let mut conflicts: std::collections::BTreeMap<String, Vec<String>> = Default::default();
    let started = std::time::Instant::now();
    for s in maps {
        let t = std::time::Instant::now();
        let mut src = MapSource::new(schema, s.open()?, &mut sh)?;
        let r = store_map(&mut repo, &mut src, &s.stem, verify)?;
        println!(
            "{:<24} {:>5} new {:>5} same {:>4} variants {:>2} lossy {:>2} refused  raw {:>5} new ({} MB)  {:.1?}",
            s.stem,
            r.new,
            r.same,
            r.variants.len(),
            r.lossy.len(),
            r.refused.len(),
            r.raw_new.0,
            r.raw_new.1 >> 20,
            t.elapsed()
        );
        for (k, why) in &r.refused {
            println!("    refused {k}: {why}");
        }
        for k in r.variants.keys() {
            conflicts
                .entry(k.to_string())
                .or_default()
                .push(s.stem.clone());
        }
        totals.0 += r.new;
        totals.1 += r.same;
        totals.2 += r.variants.len();
        totals.3 += r.lossy.len();
        totals.4 += r.refused.len();
        totals.5 += r.raw_new.1;
    }
    println!(
        "{} tags new, {} identical to a stored one, {} variants ({} keys), {} lossy, {} refused; raw {} MB; {:.1?}",
        totals.0,
        totals.1,
        totals.2,
        conflicts.len(),
        totals.3,
        totals.4,
        totals.5 >> 20,
        started.elapsed()
    );
    let mut lines = String::new();
    for (k, maps) in &conflicts {
        lines.push_str(&format!("{k}\t{}\n", maps.join(",")));
    }
    let report = out.join("conflicts.txt");
    std::fs::write(&report, lines)?;
    println!(
        "conflicts (same path, other contents; per map) listed in {}",
        report.display()
    );
    Ok(())
}

fn build(args: &[String]) -> Result<()> {
    let mut positional = Vec::new();
    let mut out = None;
    let mut map = None;
    let mut against = None;
    let mut onto = None;
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--out" => out = Some(PathBuf::from(it.next().context("--out needs a file")?)),
            "--map" => map = Some(it.next().context("--map needs a name")?.clone()),
            "--compare" => {
                against = Some(PathBuf::from(it.next().context("--compare needs a disc")?))
            }
            "--onto" => onto = Some(PathBuf::from(it.next().context("--onto needs a disc")?)),
            "--force" => force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            a if a.starts_with('-') => bail!("unknown option {a}\n{USAGE}"),
            a => positional.push(a.to_owned()),
        }
    }
    let [repo_dir, scenario] = positional.as_slice() else {
        bail!("{USAGE}");
    };
    let out = out.context(format!("--out is required\n{USAGE}"))?;
    let repo_dir = Path::new(repo_dir);
    let started = std::time::Instant::now();
    if let Some(disc) = onto {
        return build_onto(repo_dir, scenario, map, &disc, &out, force);
    }
    let (set, report, closure) = tags::repo::build::from_repo(repo_dir, scenario, map.as_deref())?;
    let (bytes, layout) = set.build().context("laying out the map")?;
    let name = out
        .file_name()
        .context("--out names a file")?
        .to_string_lossy()
        .into_owned();
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut od = OutDir::new(dir, force)?;
    if let Some(d) = &against {
        od = od.protecting(d);
    }
    let path = od.write(&[name], &bytes)?;
    println!(
        "{}: {} bytes, {} tags ({} aliases) by closure, {} BSP regions, {} raw copied ({} MB), {} shared-cache pointers kept, metadata {:#x}, {:.1?}",
        path.display(),
        layout.file_size,
        report.tags,
        report.aliases,
        report.bsps,
        report.raw.0,
        report.raw.1 >> 20,
        report.external,
        layout.memory_size,
        started.elapsed()
    );
    for (from, to) in report.dangling.iter().take(10) {
        println!("    nulled reference {from} -> {to}");
    }
    for k in &report.orphan_bsps {
        println!("    BSP tag no descriptor names, left without data: {k}");
    }
    let lossy: Vec<String> = closure
        .tags
        .iter()
        .filter(|t| t.tree().is_some_and(|tr| !tr.losses.is_empty()))
        .map(|t| t.key.to_string())
        .collect();
    if !lossy.is_empty() {
        println!(
            "    {} tags carry extraction losses: {}",
            lossy.len(),
            lossy.join(", ")
        );
    }
    let Some(disc) = against else {
        return Ok(());
    };
    let m = TagRepo::open(repo_dir)?;
    let stem = match map {
        Some(m) => m,
        None => m
            .map_of_scenario(scenario)?
            .context("no manifest names the scenario; pass --map")?,
    };
    let all = sources(&[disc])?;
    let src = all
        .iter()
        .find(|s| s.stem == stem)
        .with_context(|| format!("{stem}.map is not on the disc"))?;
    let schema = Schema::builtin();
    let mut sh = shared(schema, &all)?;
    let mut none = SharedFiles::default();
    none.unread_ok = true;
    let built = Map::from_reader(std::io::Cursor::new(bytes))?;
    let mut b = MapSource::new(schema, built, &mut none)?;
    let mut s = MapSource::new(schema, src.open()?, &mut sh)?;
    let c = compare(&mut b, &mut s)?;
    let in_closure: std::collections::HashSet<_> =
        closure.tags.iter().map(|t| t.key.clone()).collect();
    let mut unreached = std::collections::BTreeMap::<String, usize>::new();
    for t in s.tags() {
        let k = tags::repo::TagKey::new(t.group, t.name.clone().unwrap_or_default());
        if !in_closure.contains(&k) {
            *unreached.entry(t.group.to_string()).or_default() += 1;
        }
    }
    println!(
        "compared with {stem}.map: {} of {} tags identical, {} differ, {} not in the disc map, {} refused; {} local raw resources compared byte for byte; disc map has {} tags, {} not reached {:?}",
        c.same,
        report.tags,
        c.differ.len(),
        c.missing.len(),
        c.refused.len(),
        c.raw_compared,
        s.tags().len(),
        unreached.values().sum::<usize>(),
        unreached
    );
    for (k, d) in c.differ.iter().take(10) {
        println!("    differs {k}: {d}");
    }
    if !c.lossy.is_empty() {
        println!(
            "    identical but lossy in both (fields the schema could not keep): {}",
            c.lossy
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    if !c.differ.is_empty() || !c.missing.is_empty() {
        bail!("the rebuilt map differs from the disc's");
    }
    Ok(())
}

/// `--onto`: the disc map's file with every tag the repository has exactly
/// rebuilt from it (`tags::repo::build::onto_retail`).
fn build_onto(
    repo_dir: &Path,
    scenario: &str,
    map: Option<String>,
    disc: &Path,
    out: &Path,
    force: bool,
) -> Result<()> {
    let base = TagRepo::open(repo_dir)?;
    let stem = match map {
        Some(m) => m,
        None => base
            .map_of_scenario(scenario)?
            .context("no manifest names the scenario; pass --map")?,
    };
    let all = sources(&[disc.to_path_buf()])?;
    let src = all
        .iter()
        .find(|s| s.stem == stem)
        .with_context(|| format!("{stem}.map is not on the disc"))?;
    let mut repo = TagRepo::for_map(repo_dir, &stem)?;
    let schema = Schema::builtin();
    let (set, report) = tags::repo::build::onto_retail(schema, src.open()?, &mut repo)?;
    let (bytes, layout) = set.build().context("laying out the map")?;
    let name = out
        .file_name()
        .context("--out names a file")?
        .to_string_lossy()
        .into_owned();
    let dir = out
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let path = OutDir::new(dir, force)?
        .protecting(disc)
        .write(&[name], &bytes)?;
    println!(
        "{}: {} bytes; {} tags built from the repository ({} aliases), {} left as the disc's ({} lossy in the repository, {} missing); {} raw copied, {} raw pointers kept",
        path.display(),
        layout.file_size,
        report.replaced,
        report.aliases,
        report.pinned,
        report.lossy.len(),
        report.missing.len(),
        report.build.raw.0,
        report.build.external,
    );
    let mut sh = shared(schema, &all)?;
    let mut none = SharedFiles::default();
    none.unread_ok = true;
    let built = Map::from_reader(std::io::Cursor::new(bytes))?;
    let mut b = MapSource::new(schema, built, &mut none)?;
    let mut s = MapSource::new(schema, src.open()?, &mut sh)?;
    let c = compare(&mut b, &mut s)?;
    println!(
        "compared with {stem}.map: {} identical, {} differ, {} not in the disc map, {} refused",
        c.same,
        c.differ.len(),
        c.missing.len(),
        c.refused.len()
    );
    for (k, d) in c.differ.iter().take(10) {
        println!("    differs {k}: {d}");
    }
    if !c.differ.is_empty() || !c.missing.is_empty() {
        bail!("the rebuilt map differs from the disc's");
    }
    Ok(())
}
