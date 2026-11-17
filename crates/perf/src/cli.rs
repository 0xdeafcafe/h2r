//! `h2 profile` and `h2 bench`. Each returns the process exit code.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};

use crate::bench::{self, BenchCtx, Plan, Prepared, Verdict};
use crate::profile::{Render, Session, SessionConfig};

pub const PROFILE_USAGE: &str = "usage: h2 profile <scenario> [options]
       h2 profile budget <trace.json> [--json]   (see h2 profile budget --help)
       h2 profile remote <dump.h2prof> [options] (see h2 profile remote --help)

Run a benchmark scenario repeatedly under the sampling profiler (and,
optionally, the timeline) and write profile.folded, profile.txt and
profile.json. `h2 bench list` names the scenarios.

  --out DIR         output directory (default $TMPDIR/h2profile)
  --seconds S       how long to keep running the scenario (default 3)
  --hz N            samples per second (default 1000)
  --top N           rows per table (default 25)
  --cpu             leave out samples where the thread was not running
  --by-category     root each folded stack at its category
  --no-insns        no guest-instruction leaf frames in the folded stacks
  --full-stacks     keep the start-up frames (main, lang_start) in them
  --timeline FILE   also write a Chrome/Perfetto trace JSON
  --cats SPEC       timeline categories (default: all but hle)
  --work DIR        build scratch (default $TMPDIR/h2perf-work)
  --disc ISO        for the boot scenario (default $H2_DISC)
  --quick           the scenarios' small workloads

Any run can be profiled without this command: set H2_PROFILE=<dir>
and/or H2_TIMELINE=<file.json> for a binary that opens a perf::Session
(docs/perf/perf.md).";

pub const BENCH_USAGE: &str = "usage:
  h2 bench [run] [--out DIR] [--label NAME] [--only PATTERN]... [--samples N]
           [--warmup N] [--frames N] [--work DIR] [--disc ISO] [--quick]
  h2 bench list
  h2 bench compare <base.json|DIR> [<new.json>] [--threshold PCT] [--json]

run      runs every scenario (or those whose name contains a PATTERN) and
         writes <DIR>/<unix time>-<label>.json (default DIR $TMPDIR/h2bench)
compare  compares two runs; with a directory and no second file, its two
         newest. Exits 1 when any scenario regressed by more than the
         threshold (default 5%) and twice its noise (MAD).";

fn tmp(name: &str) -> PathBuf {
    std::env::temp_dir().join(name)
}

struct Args {
    it: std::vec::IntoIter<String>,
}

impl Args {
    fn value(&mut self, flag: &str) -> Result<String> {
        self.it
            .next()
            .with_context(|| format!("{flag} needs a value"))
    }
    fn num<T: std::str::FromStr>(&mut self, flag: &str) -> Result<T>
    where
        T::Err: std::fmt::Display,
    {
        let v = self.value(flag)?;
        v.parse().map_err(|e| anyhow::anyhow!("{flag} {v}: {e}"))
    }
}

pub const BUDGET_USAGE: &str = "usage: h2 profile budget <trace.json> [--json]

Split each frame of a timeline trace (H2_TIMELINE, or --timeline) into
the self time of the game thread's spans, with logical time, idle
advances, slices, accesses and vblanks per frame and the contended guest
memory lock waits (docs/perf/perf-budget.md). Record it with the default
categories (all but hle) or `all` to split HLE calls out of guest slices.";

/// `h2 profile budget`.
pub fn budget(args: Vec<String>) -> Result<i32> {
    budget_to(args, &mut std::io::stdout().lock())
}

/// `h2 profile budget`, printing to `out`.
pub fn budget_to(args: Vec<String>, out: &mut impl std::io::Write) -> Result<i32> {
    let mut path = None;
    let mut json = false;
    for a in args {
        match a.as_str() {
            "--json" => json = true,
            "-h" | "--help" => bail!("{BUDGET_USAGE}"),
            _ if path.is_none() => path = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{BUDGET_USAGE}"),
        }
    }
    let path = path.context(BUDGET_USAGE)?;
    let text =
        std::fs::read_to_string(&path).with_context(|| format!("reading {}", path.display()))?;
    let b = crate::budget::Budget::from_chrome_json(&text)?;
    if json {
        writeln!(out, "{}", serde_json::to_string_pretty(&b)?)?;
    } else {
        write!(out, "{}", b.table())?;
    }
    Ok(0)
}

pub fn profile(args: Vec<String>) -> Result<i32> {
    if args.first().is_some_and(|a| a == "budget") {
        return budget(args[1..].to_vec());
    }
    if args.first().is_some_and(|a| a == "remote") {
        return remote(args[1..].to_vec());
    }
    let mut a = Args {
        it: args.into_iter(),
    };
    let mut cfg = SessionConfig {
        profile: Some(tmp("h2profile")),
        ..SessionConfig::default()
    };
    let mut seconds = 3.0f64;
    let mut work = tmp("h2perf-work");
    let mut disc = None;
    let mut quick = false;
    let mut scenario = None;
    let mut cats = None;
    while let Some(f) = a.it.next() {
        match f.as_str() {
            "--out" => cfg.profile = Some(a.value(&f)?.into()),
            "--seconds" => seconds = a.num(&f)?,
            "--hz" => cfg.hz = a.num(&f)?,
            "--top" => cfg.render.top = a.num(&f)?,
            "--cpu" => cfg.render.on_cpu_only = true,
            "--by-category" => cfg.render.by_category = true,
            "--no-insns" => cfg.render.guest_insns = false,
            "--full-stacks" => cfg.render.full_stacks = true,
            "--timeline" => cfg.timeline = Some(a.value(&f)?.into()),
            "--cats" => cats = Some(a.value(&f)?),
            "--work" => work = a.value(&f)?.into(),
            "--disc" => disc = Some(PathBuf::from(a.value(&f)?)),
            "--quick" => quick = true,
            "-h" | "--help" => bail!("{PROFILE_USAGE}"),
            _ if scenario.is_none() && !f.starts_with('-') => scenario = Some(f),
            _ => bail!("unexpected {f}\n\n{PROFILE_USAGE}"),
        }
    }
    if let Some(c) = cats {
        cfg.timeline_config = timeline::Config::parse(&c).map_err(anyhow::Error::msg)?;
    }
    let name = scenario.context(PROFILE_USAGE)?;
    let mut found = crate::scenarios(disc)
        .into_iter()
        .filter(|s| s.name() == name)
        .collect::<Vec<_>>();
    let Some(mut s) = found.pop() else {
        bail!("no scenario {name:?}; `h2 bench list` names them");
    };
    let ctx = BenchCtx {
        work,
        quick,
        frames: 60,
    };
    eprintln!("preparing {name}");
    if let Prepared::Skip(why) = s.prepare(&ctx)? {
        bail!("{name} cannot run here: {why}");
    }
    let render: Render = cfg.render;
    let session = Session::start(cfg)?;
    let t = Instant::now();
    let mut runs = 0u64;
    while t.elapsed() < Duration::from_secs_f64(seconds) || runs == 0 {
        s.measure()?;
        runs += 1;
    }
    let report = session.finish()?;
    eprintln!("{runs} runs of {name} in {:.2}s", t.elapsed().as_secs_f64());
    if let Some(p) = &report.profile {
        print!("{}", p.table(&render));
    }
    if report.timeline_events > 0 || report.timeline_dropped > 0 {
        eprintln!(
            "timeline: {} events ({} dropped)",
            report.timeline_events, report.timeline_dropped
        );
    }
    for f in &report.files {
        eprintln!("wrote {}", f.display());
    }
    Ok(0)
}

pub fn bench(args: Vec<String>) -> Result<i32> {
    let mut args = args;
    let sub = match args.first().map(String::as_str) {
        Some("list") | Some("compare") | Some("run") => args.remove(0),
        _ => "run".into(),
    };
    let mut a = Args {
        it: args.into_iter(),
    };
    match sub.as_str() {
        "list" => {
            for s in crate::scenarios(None) {
                println!("{:<24} {:<9} {}", s.name(), s.unit(), s.description());
            }
            Ok(0)
        }
        "compare" => {
            let mut files = Vec::new();
            let mut threshold = 5.0;
            let mut json = false;
            while let Some(f) = a.it.next() {
                match f.as_str() {
                    "--threshold" => threshold = a.num(&f)?,
                    "--json" => json = true,
                    "-h" | "--help" => bail!("{BENCH_USAGE}"),
                    _ => files.push(PathBuf::from(f)),
                }
            }
            let (base, new) = match files.as_slice() {
                [dir] if dir.is_dir() => {
                    let runs = bench::runs_in(dir)?;
                    let [.., b, n] = runs.as_slice() else {
                        bail!("{} has fewer than two runs", dir.display());
                    };
                    (b.clone(), n.clone())
                }
                [b, n] => (b.clone(), n.clone()),
                _ => bail!("{BENCH_USAGE}"),
            };
            let (b, n) = (bench::load(&base)?, bench::load(&new)?);
            let rows = bench::compare(&b, &n, threshold);
            if json {
                println!("{}", serde_json::to_string_pretty(&rows)?);
            } else {
                print!("{}", bench::comparison_table(&b, &n, &rows));
            }
            Ok(i32::from(
                rows.iter().any(|r| r.verdict == Verdict::Regressed),
            ))
        }
        _ => {
            let mut out = tmp("h2bench");
            let mut label = "run".to_string();
            let mut only = Vec::new();
            let mut plan = Plan::default();
            let mut ctx = BenchCtx {
                work: tmp("h2perf-work"),
                quick: false,
                frames: 120,
            };
            let mut disc = None;
            while let Some(f) = a.it.next() {
                match f.as_str() {
                    "--out" => out = a.value(&f)?.into(),
                    "--label" => label = a.value(&f)?,
                    "--only" => only.push(a.value(&f)?),
                    "--samples" => plan.samples = a.num(&f)?,
                    "--warmup" => plan.warmup = a.num(&f)?,
                    "--frames" => ctx.frames = a.num(&f)?,
                    "--work" => ctx.work = a.value(&f)?.into(),
                    "--disc" => disc = Some(PathBuf::from(a.value(&f)?)),
                    "--quick" => ctx.quick = true,
                    "-h" | "--help" => bail!("{BENCH_USAGE}"),
                    _ => bail!("unexpected {f}\n\n{BENCH_USAGE}"),
                }
            }
            let mut scenarios = crate::select(disc, &only);
            if scenarios.is_empty() {
                bail!("no scenario matches {only:?}; `h2 bench list` names them");
            }
            let run = bench::run(&mut scenarios, &ctx, plan, &label, |l| eprintln!("{l}"));
            let path = bench::save(&run, &out)?;
            println!("{}", path.display());
            Ok(0)
        }
    }
}

pub const REMOTE_USAGE: &str = "usage: h2 profile remote <dump.h2prof> [options]

Symbolise a profile the Xbox app took (agent `profile.sample.stop`,
docs/runtime/xbox.md \"Profiling on the console\") against the binaries
that were deployed, and print the top functions by self time; write
profile.folded, profile.txt and profile.json too.

  --image PE        a deployed image (h2game.dll, h2uwp.exe); repeat. The
                    generated module's entry table names guest functions
  --map MAP         an lld-link /map for the image of the same stem
                    (h2uwp.map for h2uwp.exe); repeat
  --names TSV       guest function names (disc/ghidra-functions.tsv)
  --out DIR         output directory (default: beside the dump, <stem>/)
  --top N           rows per table (default 40)
  --cpu             leave out samples where the thread was not running
  --by-category     root each folded stack at its category
  --full-stacks     keep the thread start-up frames in folded stacks";

/// `h2 profile remote`.
pub fn remote(args: Vec<String>) -> Result<i32> {
    use crate::remote::{Dump, Image, Symbols, guest_names, name_guest};
    let mut a = Args {
        it: args.into_iter(),
    };
    let mut dump = None;
    let mut images: Vec<PathBuf> = Vec::new();
    let mut maps: Vec<PathBuf> = Vec::new();
    let mut names = None;
    let mut out = None;
    let mut render = Render {
        top: 40,
        guest_insns: false,
        ..Render::default()
    };
    while let Some(f) = a.it.next() {
        match f.as_str() {
            "--image" => images.push(a.value(&f)?.into()),
            "--map" => maps.push(a.value(&f)?.into()),
            "--names" => names = Some(PathBuf::from(a.value(&f)?)),
            "--out" => out = Some(PathBuf::from(a.value(&f)?)),
            "--top" => render.top = a.num(&f)?,
            "--cpu" => render.on_cpu_only = true,
            "--by-category" => render.by_category = true,
            "--full-stacks" => render.full_stacks = true,
            "-h" | "--help" => {
                println!("{REMOTE_USAGE}");
                return Ok(0);
            }
            other if other.starts_with('-') => bail!("unknown option {other}\n\n{REMOTE_USAGE}"),
            other => dump = Some(PathBuf::from(other)),
        }
    }
    let Some(dump_path) = dump else {
        bail!("{REMOTE_USAGE}")
    };
    let text = std::fs::read_to_string(&dump_path).with_context(|| format!("reading {}", dump_path.display()))?;
    let d = Dump::parse(&text).map_err(|e| anyhow::anyhow!("{}: {e}", dump_path.display()))?;
    let file_name = |p: &PathBuf| p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let stem = |p: &PathBuf| p.file_stem().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let mut syms = Symbols::default();
    for img in &images {
        let bytes = std::fs::read(img).with_context(|| format!("reading {}", img.display()))?;
        let mut image = Image::from_pe(&bytes).map_err(|e| anyhow::anyhow!("{}: {e}", img.display()))?;
        if let Some(m) = maps.iter().find(|m| stem(m) == stem(img)) {
            let map = std::fs::read_to_string(m).with_context(|| format!("reading {}", m.display()))?;
            image = image.with_map(&map);
        }
        if image.guest_functions() > 0 {
            eprintln!("profile remote: {}: {} guest functions", file_name(img), image.guest_functions());
        }
        syms.add(&file_name(img), image);
    }
    for m in maps.iter().filter(|m| !images.iter().any(|i| stem(i) == stem(m))) {
        let map = std::fs::read_to_string(m).with_context(|| format!("reading {}", m.display()))?;
        syms.add(&format!("{}.exe", stem(m)), Image::default().with_map(&map));
    }
    for m in &d.modules {
        if syms.image(&m.name).is_none() {
            eprintln!("profile remote: no --image for {}: its frames stay {}+offset", m.name, m.name);
        }
    }
    let profile = syms.resolve(&d);
    let names = match &names {
        Some(p) => guest_names(&std::fs::read_to_string(p).with_context(|| format!("reading {}", p.display()))?),
        None => Default::default(),
    };
    let mut summary = profile.summary(&render);
    for rows in [
        &mut summary.self_top,
        &mut summary.guest_total,
        &mut summary.guest_self,
    ] {
        for r in rows.iter_mut() {
            r.name = name_guest(&r.name, &names);
        }
    }
    let table = summary.text();
    let out = out.unwrap_or_else(|| {
        let s = dump_path.file_stem().map(|s| s.to_owned()).unwrap_or_else(|| "profile".into());
        dump_path.with_file_name(s)
    });
    std::fs::create_dir_all(&out)?;
    std::fs::write(out.join("profile.folded"), profile.folded(&render))?;
    std::fs::write(out.join("profile.txt"), &table)?;
    std::fs::write(out.join("profile.json"), serde_json::to_vec_pretty(&summary)?)?;
    print!("{table}");
    eprintln!("profile remote: wrote profile.folded, profile.txt and profile.json in {}", out.display());
    Ok(0)
}
