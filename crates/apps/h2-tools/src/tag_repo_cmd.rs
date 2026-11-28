//! `h2 tags extract`, `h2 tags build`, `h2 tags reassemble` and `h2 tags
//! catalogue`: the loose tag repository (docs/formats/tag-repo.md), the
//! maps it gives back and the assets it holds (docs/engine/asset-store.md).
use std::collections::BTreeSet;
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use map::{FourCC, Map, ResourceFile};
use tags::repo::catalogue::{self, Variants};
use tags::repo::extract::MapWork;
use tags::repo::store::{EXTRACT_FILE, PutReport};
use tags::repo::{EXTRACT_VERSION, Layouts, MapSource, SharedFiles, TagRepo, compare, update};
use tags::schema::Schema;

use crate::safe_out::{self, OutDir};

const USAGE: &str = "usage:
  h2 tags extract [disc.iso|file.map ...] --out REPO [--maps a,b] [--verify] [--update]
      every tag of every map into REPO (one file per tag, raw by hash);
      identical tags are stored once, others as variants (see the report).
      REPO/extract.toml records the map files, the extractor version and
      each group's layout hash; --update extracts only what moved (a new
      or changed map, a map an interrupted run did not finish, the groups
      whose layout changed) and does nothing when nothing did
  h2 tags build REPO SCENARIO --out FILE.map [--map NAME] [--compare disc.iso] [--onto disc.iso] [--force]
      the map of SCENARIO (e.g. scenarios\\multi\\lockout\\lockout) from REPO
      alone, by closure from the scenario and the globals; header values,
      variants and the low-detail region come from map NAME's manifest
      (default: the map whose scenario it is); --compare reads the result
      back and compares every tag with the disc's map; --onto keeps the
      disc map's file, BSP regions and the tags the repository cannot give
      exactly, and builds every other tag from REPO (a safer load test)
  h2 tags reassemble [--repo REPO] MAP --out FILE.map [--force]
      the map file from REPO's skeleton and extent table, byte for byte
  h2 tags reassemble [--repo REPO] --all [--compare disc.iso]
      every map REPO records: reassembled and its SHA-256 checked against
      REPO/extract.toml and, with --compare, against the disc's own file;
      then the store's size by part. Fails when a map differs, has no
      skeleton, or (with --compare) a disc map is not in REPO.
  h2 tags catalogue [--repo REPO] [--force]
      REPO's asset catalogue (assets.idx): built from the manifests and
      written when missing or stale (--force: always), then one line per
      group that has variants: the keys, the files and the variant
      readings as the store keeps them, and the assets by asset hash
      after the normal forms (docs/engine/asset-store.md, 'Normal forms')
  REPO defaults to <data>/tags";

type DiscMap = Map<BufReader<File>>;

/// Whether `args` (after `h2 tags`) is one of these subcommands.
pub(crate) fn handles(args: &[String]) -> bool {
    matches!(
        args.first().map(String::as_str),
        Some("extract" | "build" | "reassemble" | "catalogue")
    )
}

pub(crate) fn run(args: Vec<String>) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("extract") => extract(&args[1..]),
        Some("build") => build(&args[1..]),
        Some("reassemble") => reassemble(&args[1..]),
        Some("catalogue") => catalogue_cmd(&args[1..]),
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

    /// SHA-256 of the map file's bytes (hex): the disc's identity.
    fn file_hash(&self) -> Result<String> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.base))?;
        tags::repo::extract::file_hash(f.take(self.len))
            .with_context(|| format!("hashing {}", self.stem))
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
    let mut only_moved = false;
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
            "--update" => only_moved = true,
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
    let hashing = std::time::Instant::now();
    let mut disc = Vec::with_capacity(maps.len());
    for s in &maps {
        disc.push((s.stem.clone(), s.file_hash()?));
    }
    let layouts = Layouts::new(schema);
    let plan = tags::repo::plan(&repo, &layouts, &disc, !only_moved)?;
    if plan.is_empty() {
        println!(
            "{}: up to date ({} maps, extractor {EXTRACT_VERSION}, file hashes read in {:.1?}); nothing to extract",
            out.display(),
            disc.len(),
            hashing.elapsed()
        );
        return Ok(());
    }
    if plan.reset {
        println!(
            "{}: no record under extractor {EXTRACT_VERSION} and this tag file format; emptying the tree (raw resources kept)",
            out.display()
        );
    }
    for w in &plan.maps {
        match &w.groups {
            None => println!("plan {:<24} whole map ({:?})", w.map, w.why),
            Some(g) => println!(
                "plan {:<24} {} groups whose layout moved: {}",
                w.map,
                g.len(),
                g.iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        }
    }
    let mut sh = shared(schema, &all)?;
    let mut totals = (0usize, 0usize, 0usize, 0usize, 0usize, 0u64);
    let started = std::time::Instant::now();
    let mut t = std::time::Instant::now();
    let mut open = |stem: &str| -> tags::repo::Result<DiscMap> {
        let src = all
            .iter()
            .find(|s| s.stem == stem)
            .ok_or_else(|| tags::repo::Error::Invalid(format!("{stem}.map is not an input")))?;
        src.open()
            .map_err(|e| tags::repo::Error::Invalid(format!("{e:#}")))
    };
    let mut on_map = |w: &MapWork, r: &PutReport| {
        if let Some(k) = &r.skeleton {
            println!(
                "{:<24} skeleton {} KB ({} KB on disk), {} named extents ({} MB), {} unnamed ({} KB); reassembles to its SHA-256",
                w.map,
                k.skeleton.0 >> 10,
                k.skeleton.1 >> 10,
                k.named.0,
                k.named.1 >> 20,
                k.unnamed.0,
                k.unnamed.1 >> 10
            );
        }
        println!(
            "{:<24} {:>5} new {:>5} same {:>4} variants {:>2} lossy {:>2} refused  raw {:>5} new ({} MB)  {:.1?}",
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
        for (k, why) in &r.refused {
            println!("    refused {k}: {why}");
        }
        totals.0 += r.new;
        totals.1 += r.same;
        totals.2 += r.variants.len();
        totals.3 += r.lossy.len();
        totals.4 += r.refused.len();
        totals.5 += r.raw_new.1;
        t = std::time::Instant::now();
    };
    let done = update(
        &mut repo,
        &plan,
        &layouts,
        schema,
        &mut sh,
        &mut open,
        verify,
        &mut on_map,
    )?;
    println!(
        "{} maps: {} tags new, {} identical to a stored one, {} variants, {} lossy, {} refused; raw {} MB; {} main files settled; {:.1?}",
        done.maps,
        totals.0,
        totals.1,
        totals.2,
        totals.3,
        totals.4,
        totals.5 >> 20,
        done.settled.promoted,
        started.elapsed()
    );
    println!(
        "{} keys with variants (same path, other contents; per map) listed in {}; versions in {}",
        done.settled.conflicts,
        out.join("conflicts.txt").display(),
        out.join(EXTRACT_FILE).display()
    );
    Ok(())
}

fn build(args: &[String]) -> Result<()> {
    if args.iter().any(|a| a == "--project") {
        return build_project(args);
    }
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

const PROJECT_USAGE: &str = "usage:
  h2 tags build --project FILE [--disc ISO | --repo DIR] [--data DIR] [--out FILE.map] [--report FILE.json] [--force]
      the map a map.toml names (content/stage/map.toml: the content
      stage, docs/editor/content-stage.md), built fresh over its base map,
      read from the disc (default: the default disc) or from a tag tree
      `h2 tags extract` wrote (--repo); written as the map package
      <data>/maps/<name>/ (default <data>: the user's data directory),
      with --out also as a lone map and --report the build report";

/// `--project`: the content stage over its base (`content_kit::stage::map`).
fn build_project(args: &[String]) -> Result<()> {
    let mut project = None;
    let mut disc = None;
    let mut repo_dir = None;
    let mut data = None;
    let mut out = None;
    let mut report = None;
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut path = |what: &str| -> Result<PathBuf> {
            Ok(PathBuf::from(
                it.next().with_context(|| format!("{a} needs {what}"))?,
            ))
        };
        match a.as_str() {
            "--project" => project = Some(path("a project file")?),
            "--disc" => disc = Some(path("a disc")?),
            "--repo" => repo_dir = Some(path("a tag tree")?),
            "--data" => data = Some(path("a data directory")?),
            "--out" => out = Some(path("a file")?),
            "--report" => report = Some(path("a file")?),
            "--force" => force = true,
            "-h" | "--help" => {
                println!("{PROJECT_USAGE}");
                return Ok(());
            }
            a => bail!("unknown argument {a}\n{PROJECT_USAGE}"),
        }
    }
    let project_path = project.context(format!("--project needs a file\n{PROJECT_USAGE}"))?;
    if disc.is_some() && repo_dir.is_some() {
        bail!("give --disc or --repo, not both\n{PROJECT_USAGE}");
    }
    let text = std::fs::read_to_string(&project_path)
        .with_context(|| format!("reading {}", project_path.display()))?;
    let project = workspace::stage::project(&text)?;
    let started = std::time::Instant::now();
    let (built, base) = match repo_dir {
        Some(dir) => {
            let (base, mut repo) = content_kit::stage::map::Base::from_repo(&dir, &project.base)?;
            (content_kit::stage::map::build(&mut repo, &base, &project)?, base)
        }
        None => {
            let disc = disc.map(Ok).unwrap_or_else(super::default_disc)?;
            let all = sources(std::slice::from_ref(&disc))?;
            let src = all
                .iter()
                .find(|s| s.stem == project.base)
                .with_context(|| format!("{}.map is not on {}", project.base, disc.display()))?;
            let schema = Schema::builtin();
            let mut sh = shared(schema, &all)?;
            let mut ms = MapSource::new(schema, src.open()?, &mut sh)?;
            let base = content_kit::stage::map::Base::from_map(&mut ms, &project.base)?;
            (content_kit::stage::map::build(&mut ms, &base, &project)?, base)
        }
    };
    let data = data.unwrap_or_else(content::store::default_data_root);
    let package = content_kit::stage::map::package(&built, &project, &base, &data)?;
    let write = |path: &Path, bytes: &[u8]| -> Result<PathBuf> {
        let name = path
            .file_name()
            .context("an output names a file")?
            .to_string_lossy()
            .into_owned();
        let dir = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        Ok(OutDir::new(dir, force)?.write(&[name], bytes)?)
    };
    if let Some(out) = &out {
        println!("{}", write(out, &built.bytes)?.display());
    }
    if let Some(path) = &report {
        let json = serde_json::to_vec_pretty(&built.report)?;
        println!("{}", write(path, &json)?.display());
    }
    let r = &built.report;
    println!(
        "{}: {} over {} ({} bytes, {} tags, {} BSP region, tag memory {:#x} of {:#x}, key {}), {:.1?}",
        package.display(),
        project.scenario,
        base.map,
        r.file_size,
        r.tags,
        r.bsps,
        r.memory_size,
        r.memory_max,
        &r.key[..16],
        started.elapsed()
    );
    if let Some(why) = &r.gestalt_whole {
        println!("    sound gestalt kept whole: {why}");
    }
    for gap in &r.scenario_edit.gaps {
        println!("    base scenario: {gap}");
    }
    Ok(())
}

/// Files and bytes under `dir` (0 when it is not there).
fn dir_size(dir: &Path) -> (u64, u64) {
    let mut out = (0, 0);
    let mut todo = vec![dir.to_path_buf()];
    while let Some(d) = todo.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in rd.flatten() {
            let Ok(t) = e.file_type() else { continue };
            if t.is_dir() {
                todo.push(e.path());
            } else if let Ok(m) = e.metadata() {
                out.0 += 1;
                out.1 += m.len();
            }
        }
    }
    out
}

fn mb(b: u64) -> String {
    format!("{:.1} MB", b as f64 / (1u64 << 20) as f64)
}

/// `h2 tags reassemble`: one map out of the store, or every map checked
/// (the coordinator's disc check for docs/engine/asset-store.md, AS1).
fn reassemble(args: &[String]) -> Result<()> {
    let mut repo_dir = None;
    let mut out = None;
    let mut all = false;
    let mut against = None;
    let mut force = false;
    let mut maps = Vec::new();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repo" => {
                repo_dir = Some(PathBuf::from(
                    it.next().context("--repo needs a directory")?,
                ))
            }
            "--out" | "-o" => out = Some(PathBuf::from(it.next().context("--out needs a file")?)),
            "--compare" => {
                against = Some(PathBuf::from(it.next().context("--compare needs a disc")?))
            }
            "--all" => all = true,
            "--force" => force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            a if a.starts_with('-') => bail!("unknown option {a}\n{USAGE}"),
            a => maps.push(a.trim_end_matches(".map").to_owned()),
        }
    }
    let repo_dir = repo_dir.unwrap_or_else(|| content::store::default_data_root().join("tags"));
    let repo = TagRepo::open(&repo_dir)?;
    let record = repo.extract_record()?;
    if !all {
        let [map] = maps.as_slice() else {
            bail!("name one map, or --all\n{USAGE}");
        };
        let out = out.context(format!("--out is required for one map\n{USAGE}"))?;
        let mut bytes = Vec::new();
        let hash = repo.reassemble(map, &mut bytes)?;
        let name = out
            .file_name()
            .context("--out names a file")?
            .to_string_lossy()
            .into_owned();
        let dir = out
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let path = OutDir::new(dir, force)?.write(&[name], &bytes)?;
        let recorded = record.as_ref().and_then(|r| r.maps.get(map.as_str()));
        println!(
            "{}: {} bytes, SHA-256 {hash}{}",
            path.display(),
            bytes.len(),
            match recorded {
                Some(m) if m.file == hash => " (the extracted file's)",
                Some(_) => " (NOT the extracted file's)",
                None => "",
            }
        );
        if recorded.is_some_and(|m| m.file != hash) {
            bail!("{map} does not reassemble to the file it was extracted from");
        }
        return Ok(());
    }
    if out.is_some() || !maps.is_empty() {
        bail!("--all takes no map and no --out\n{USAGE}");
    }
    let record = record.context(format!(
        "{} has no {EXTRACT_FILE}: extract first",
        repo_dir.display()
    ))?;
    let started = std::time::Instant::now();
    let mut disc = std::collections::BTreeMap::new();
    if let Some(d) = &against {
        for s in sources(std::slice::from_ref(d))? {
            let h = s.file_hash()?;
            disc.insert(s.stem.clone(), h);
        }
    }
    let mut names: std::collections::BTreeSet<String> = record.maps.keys().cloned().collect();
    names.extend(disc.keys().cloned());
    let mut failed = Vec::new();
    let (mut logical, mut skel, mut skel_file, mut ext_n, mut ext_b) =
        (0u64, 0u64, 0u64, 0usize, 0u64);
    for map in &names {
        let Some(rec) = record.maps.get(map) else {
            println!("{map:<24} on the disc, not in the store");
            failed.push(map.clone());
            continue;
        };
        if !repo.has_skeleton(map) {
            println!("{map:<24} no skeleton or extent table (extract --update takes it again)");
            failed.push(map.clone());
            continue;
        }
        let mut f = repo.store_file(map)?;
        let hash = match tags::repo::extract::file_hash(&mut f) {
            Ok(h) => h,
            Err(e) => {
                println!("{map:<24} does not read back: {e}");
                failed.push(map.clone());
                continue;
            }
        };
        let skel_size = std::fs::metadata(repo_dir.join("maps").join(format!("{map}.skel")))
            .map(|m| m.len())
            .unwrap_or(0);
        let x = tags::repo::StoreFile::extents(&repo_dir, map)?;
        logical += f.len();
        skel += f.skeleton_bytes();
        skel_file += skel_size;
        ext_n += x.extents.len();
        ext_b += x.raw_bytes();
        let vs_record = hash == rec.file;
        let vs_disc = disc.get(map).map(|d| *d == hash);
        let ok = vs_record && vs_disc != Some(false);
        println!(
            "{map:<24} {:>10} skeleton {:>10} ({:>10} on disk) {:>6} extents  {}{}",
            mb(f.len()),
            mb(f.skeleton_bytes()),
            mb(skel_size),
            x.extents.len(),
            if vs_record {
                "= extract.toml"
            } else {
                "!= extract.toml"
            },
            match vs_disc {
                Some(true) => ", = disc",
                Some(false) => ", != disc",
                None => "",
            }
        );
        if !ok {
            failed.push(map.clone());
        }
    }
    let tags_size = dir_size(&repo_dir.join("tags"));
    let raw_size = dir_size(&repo_dir.join("raw"));
    println!(
        "{} maps, {} as map files; store: tag files {} ({} files), raw {} ({} files), skeletons {} ({} before compression), extent tables {} extents over {}; {:.1?}",
        names.len(),
        mb(logical),
        mb(tags_size.1),
        tags_size.0,
        mb(raw_size.1),
        raw_size.0,
        mb(skel_file),
        mb(skel),
        ext_n,
        mb(ext_b),
        started.elapsed()
    );
    if !failed.is_empty() {
        bail!(
            "{} maps do not reassemble: {}",
            failed.len(),
            failed.join(", ")
        );
    }
    println!("every map reassembles to its SHA-256");
    Ok(())
}

/// `h2 tags catalogue`: the store's asset catalogue
/// (docs/engine/asset-store.md, "The catalogue"), built and written when
/// it is missing or stale, then the variants by group as the store keeps
/// the files and as the catalogue holds the assets.
fn catalogue_cmd(args: &[String]) -> Result<()> {
    let mut repo_dir = None;
    let mut force = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "--repo" => {
                repo_dir = Some(PathBuf::from(
                    it.next().context("--repo needs a directory")?,
                ))
            }
            "--force" => force = true,
            "-h" | "--help" => {
                println!("{USAGE}");
                return Ok(());
            }
            a if a.starts_with('-') => bail!("unknown option {a}\n{USAGE}"),
            a => bail!("catalogue takes no map name ({a})\n{USAGE}"),
        }
    }
    let repo_dir = repo_dir.unwrap_or_else(|| content::store::default_data_root().join("tags"));
    TagRepo::open(&repo_dir)?;
    let schema = Schema::builtin();
    let current =
        catalogue::read(&repo_dir)?.is_some_and(|c| c.is_current(&repo_dir).unwrap_or(false));
    let started = std::time::Instant::now();
    let (c, report) = catalogue::build(&repo_dir, schema)?;
    let wrote = force || !current;
    if wrote {
        catalogue::write(&repo_dir, &c)?;
    }
    let keys = c.assets.iter().map(|a| &a.key).collect::<std::collections::BTreeSet<_>>().len();
    let (before, after) = report.totals();
    println!(
        "{}: {} maps, {keys} keys, {} assets, {} keys with more than one content (as files: {} keys with variants); {} tags not hashed; assets.idx {}; {:.1?}",
        repo_dir.display(),
        c.maps.len(),
        after.contents,
        after.split,
        before.split,
        report.problems.len(),
        if wrote {
            "written"
        } else {
            "current, not rewritten"
        },
        started.elapsed()
    );
    println!(
        "{:<8} {:>7} {:>7} {:>7} {:>8}  |  {:>7} {:>7} {:>8}",
        "group", "keys", "files", "var", "var read", "assets", "split", "other"
    );
    let row = |name: &str, b: &Variants, a: &Variants| {
        println!(
            "{name:<8} {:>7} {:>7} {:>7} {:>8}  |  {:>7} {:>7} {:>8}",
            b.keys, b.contents, b.split, b.others, a.contents, a.split, a.others
        );
    };
    let mut groups: Vec<&FourCC> = report
        .before
        .keys()
        .chain(report.after.keys())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    groups.sort_by(|a, b| {
        let others = |g: &&FourCC| report.before.get(*g).map_or(0, |v| v.others);
        others(b).cmp(&others(a)).then_with(|| a.cmp(b))
    });
    let none = Variants::default();
    for g in &groups {
        let (b, a) = (
            report.before.get(g).unwrap_or(&none),
            report.after.get(g).unwrap_or(&none),
        );
        if b.split + a.split > 0 {
            row(&tags::defs::fourcc_text(**g), b, a);
        }
    }
    println!(
        "files: a key's contents as tag files; var: keys with more than one file; \
         var read: readings of another file than the first map's; assets: contents by \
         asset hash; split: keys with more than one asset; other: readings of another \
         asset than the first map's"
    );
    row("all", &before, &after);
    for p in report.problems.iter().take(20) {
        println!("  not hashed: {p}");
    }
    if report.problems.len() > 20 {
        println!("  ... and {} more not hashed", report.problems.len() - 20);
    }
    Ok(())
}
