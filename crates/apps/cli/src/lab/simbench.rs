//! `h2 lab sim`: sim benchmarks on the lab host (docs/runtime/lab.md, "Sim
//! benchmarks").
//!
//! The client half parses the command line and gathers the match or batch
//! file with the tuning files it names. The host half is the pure part of a
//! job: the order the arms run in, the step directories, the report files a
//! step left, and the summary. `host.rs` runs the steps (`run_sim`).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::Serialize;
use serde_json::Value;
use sim_bench::diff::{self, Verdict};
use sim_bench::metrics::Report;

use super::proto::{self, SimArm, SimReq};

/// The summary files a job writes into its run directory.
pub const SUMMARY_TEXT: &str = "sim-summary.txt";
pub const SUMMARY_JSON: &str = "sim-summary.json";

// ---- the command line ----

/// What `h2 lab sim` was given, before its build is resolved.
#[derive(Debug)]
pub struct Parsed {
    /// Everything but `files`, `file`, `batch` and `build.rev`.
    pub req: SimReq,
    pub path: PathBuf,
    pub wait: bool,
    pub to: Option<PathBuf>,
}

fn split_kv(s: &str, flag: &str) -> Result<(String, String)> {
    let (k, v) = s
        .split_once('=')
        .with_context(|| format!("{flag} needs NAME=VALUE, not {s:?}"))?;
    Ok((k.to_owned(), v.to_owned()))
}

/// The flags of `h2 lab sim` that are not about which commit to build
/// (`--branch`, `--rev`, `--current` and `--no-sync` are the client's
/// `Source`). The one positional argument is the match or batch file.
pub fn parse_args(args: Vec<String>) -> Result<Parsed> {
    let mut req = SimReq {
        repeat: 1,
        ..SimReq::default()
    };
    let mut path = None;
    let mut wait = false;
    let mut to = None;
    let mut name = None;
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--name" => name = Some(proto::job_name(&val()?)?.to_owned()),
            "--repeat" => req.repeat = val()?.parse().context("--repeat needs a count")?,
            "--arm" => {
                let (name, features) = split_kv(&val()?, "--arm")?;
                req.arms.push(SimArm {
                    name,
                    // An empty value leaves `H2_FEATURES` as it is.
                    features: Some(features).filter(|f| !f.is_empty()),
                });
            }
            "--parallel" => {
                req.parallel = Some(val()?.parse().context("--parallel needs a count")?)
            }
            "--env" => {
                let (k, v) = split_kv(&val()?, "--env")?;
                req.env.insert(k, v);
            }
            "--feature" => req.build.features.push(val()?),
            "--build-env" => {
                let (k, v) = split_kv(&val()?, "--build-env")?;
                req.build.env.insert(k, v);
            }
            "--mem-limit" => {
                req.mem_limit = Some(mem_watch::parse_size(&val()?).map_err(anyhow::Error::msg)?)
            }
            "--note" => req.note = Some(val()?),
            "--wait" => wait = true,
            "--to" => to = Some(PathBuf::from(val()?)),
            _ if path.is_none() && !a.starts_with('-') => path = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{}", super::USAGE),
        }
    }
    let path = path.context("usage: h2 lab sim <match.toml|batch.toml> [options]")?;
    if req.arms.is_empty() {
        req.arms.push(SimArm {
            name: "default".into(),
            features: None,
        });
    }
    req.name = match name {
        Some(n) => n,
        None => {
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("sim");
            format!("sim {stem}").chars().take(60).collect()
        }
    };
    Ok(Parsed { req, path, wait, to })
}

// ---- the files a match or batch names ----

/// The entry file and what it names, ready for [`SimReq`].
#[derive(Debug, PartialEq)]
pub struct Loaded {
    pub file: String,
    pub batch: bool,
    pub files: BTreeMap<String, String>,
}

fn is_json(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"))
}

/// A match or batch file as plain JSON values, whichever syntax it uses.
fn parse_value(text: &str, json: bool) -> Result<Value> {
    if json {
        Ok(serde_json::from_str(text)?)
    } else {
        Ok(toml::from_str(text)?)
    }
}

/// Whether a parsed file is a batch: only a batch file has a `match` key.
fn is_batch(v: &Value) -> bool {
    v.get("match").is_some_and(Value::is_string)
}

fn str_at<'a>(v: &'a Value, path: &[&str]) -> Option<&'a str> {
    let mut cur = v;
    for key in path {
        cur = cur.get(key)?;
    }
    cur.as_str()
}

/// The files `v` (a match or a batch file) names, relative to its own
/// directory: a batch's match file and its arms' tuning files, and every
/// bot's tuning file.
fn references(v: &Value) -> Vec<&str> {
    let mut out = Vec::new();
    if is_batch(v) {
        out.extend(str_at(v, &["match"]));
        for arm in ["a", "b"] {
            out.extend(str_at(v, &[arm, "params"]));
        }
    }
    for bot in v.get("bot").and_then(Value::as_array).into_iter().flatten() {
        out.extend(bot.get("params").and_then(Value::as_str));
    }
    out
}

/// A batch's results directory must stay inside the job's step directory.
pub fn check_batch_out(out: &str) -> Result<()> {
    let p = Path::new(out);
    ensure!(
        !out.is_empty()
            && p.components()
                .all(|c| matches!(c, Component::Normal(_) | Component::CurDir)),
        "a lab batch's out = {out:?} must be a relative path with no `..` (results land in the job's directory)"
    );
    Ok(())
}

/// The host's own check of an uploaded batch file's `out`: the client
/// checked it too, but the host acts on what it receives.
pub fn check_entry(req: &SimReq) -> Result<()> {
    if !req.batch {
        return Ok(());
    }
    let text = req.files.get(&req.file).context("no entry file")?;
    let v = parse_value(text, is_json(Path::new(&req.file)))?;
    check_batch_out(str_at(&v, &["out"]).context("a batch file needs `out`")?)
}

/// Reads `path` and every file it names. Keys are paths relative to the
/// files' deepest common directory, so `../tuning/a.toml` references keep
/// resolving on the host.
pub fn load_files(path: &Path) -> Result<Loaded> {
    let mut texts: BTreeMap<PathBuf, String> = BTreeMap::new();
    let entry = fs::canonicalize(path).with_context(|| format!("{}", path.display()))?;
    let mut todo = vec![entry.clone()];
    let mut batch = false;
    while let Some(file) = todo.pop() {
        if texts.contains_key(&file) {
            continue;
        }
        let text =
            fs::read_to_string(&file).with_context(|| format!("reading {}", file.display()))?;
        let v = parse_value(&text, is_json(&file))
            .map_err(|e| anyhow::anyhow!("{}: {e}", file.display()))?;
        let base = file.parent().context("a file with no directory")?;
        if file == entry {
            batch = is_batch(&v);
            if batch {
                let out = str_at(&v, &["out"]).context("a batch file needs `out`")?;
                check_batch_out(out)?;
            }
        }
        // Tuning files name nothing further; match files name tunings.
        for r in references(&v) {
            let target = base.join(r);
            let target = fs::canonicalize(&target).with_context(|| {
                format!("{} names {r}, which isn't there ({})", file.display(), target.display())
            })?;
            todo.push(target);
        }
        texts.insert(file, text);
    }
    // The deepest directory above every file.
    let mut root: Option<PathBuf> = None;
    for file in texts.keys() {
        let dir = file.parent().context("a file with no directory")?;
        root = Some(match root {
            None => dir.to_path_buf(),
            Some(r) => r
                .components()
                .zip(dir.components())
                .take_while(|(a, b)| a == b)
                .map(|(a, _)| a)
                .collect(),
        });
    }
    let root = root.context("no files")?;
    let key = |p: &Path| -> Result<String> {
        let rel = p.strip_prefix(&root)?;
        Ok(rel
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/"))
    };
    let mut files = BTreeMap::new();
    for (p, text) in &texts {
        files.insert(key(p)?, text.clone());
    }
    Ok(Loaded {
        file: key(&entry)?,
        batch,
        files,
    })
}

// ---- the order of runs ----

/// The runs of a benchmark, as `(arm, repeat)` pairs in the order they
/// run: repeat by repeat, every arm once per repeat, so slow drift in the
/// machine (thermals, background work) lands on every arm. The arm order
/// reverses on odd repeats (A B, B A, A B), which also cancels a drift that
/// is linear within a repeat.
pub fn schedule(arms: usize, repeat: u32) -> Vec<(usize, u32)> {
    let mut out = Vec::with_capacity(arms * repeat as usize);
    for r in 0..repeat {
        let order: Vec<usize> = if r % 2 == 0 {
            (0..arms).collect()
        } else {
            (0..arms).rev().collect()
        };
        out.extend(order.into_iter().map(|a| (a, r)));
    }
    out
}

/// The directory name of arm `arm`'s run number `repeat` (0-based) under
/// `runs/<id>/sim/`.
pub fn step_name(arm: &str, repeat: u32) -> String {
    format!("{arm}-r{}", repeat + 1)
}

/// Writes the uploaded files under `dir`.
pub fn write_inputs(dir: &Path, files: &BTreeMap<String, String>) -> Result<()> {
    for (key, text) in files {
        proto::check_sim_path(key)?;
        let path = dir.join(key);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// Removes every per-run data directory (`<out>/runs/<id>/data`) under
/// `dir`: a batch removes its own after each match, so this catches the one
/// a killed or crashed step left. A symlink inside (the data's `bots/`) goes
/// without its target.
pub fn remove_run_data(dir: &Path) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    for e in entries.flatten() {
        let Ok(t) = e.file_type() else { continue };
        if !t.is_dir() {
            continue;
        }
        let path = e.path();
        let is_run_data = e.file_name() == "data"
            && path
                .parent()
                .and_then(Path::parent)
                .and_then(Path::file_name)
                .is_some_and(|n| n == "runs");
        if is_run_data {
            let _ = fs::remove_dir_all(&path);
        } else {
            remove_run_data(&path);
        }
    }
}

/// The reports a step left, by path relative to the step directory
/// (`report.json` for a match, `<out>/runs/<id>/report.json` for each match
/// of a batch). Files that don't parse are named in the second list.
pub fn collect_reports(step: &Path) -> (BTreeMap<String, Report>, Vec<String>) {
    fn walk(root: &Path, dir: &Path, out: &mut BTreeMap<String, Report>, bad: &mut Vec<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let Ok(t) = e.file_type() else { continue };
            let path = e.path();
            if t.is_dir() {
                walk(root, &path, out, bad);
            } else if t.is_file() && e.file_name() == "report.json" {
                let key = path
                    .strip_prefix(root)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                match fs::read(&path)
                    .map_err(anyhow::Error::from)
                    .and_then(|b| Ok(serde_json::from_slice::<Report>(&b)?))
                {
                    Ok(r) => {
                        out.insert(key, r);
                    }
                    Err(err) => bad.push(format!("{key}: {err:#}")),
                }
            }
        }
    }
    let (mut out, mut bad) = (BTreeMap::new(), Vec::new());
    walk(step, step, &mut out, &mut bad);
    (out, bad)
}

// ---- the summary ----

/// One finished step of a job.
#[derive(Debug)]
pub struct StepResult {
    pub arm: usize,
    /// 0-based.
    pub repeat: u32,
    /// The sim exited 0 and left parseable reports.
    pub ok: bool,
    pub error: Option<String>,
    pub reports: BTreeMap<String, Report>,
}

/// Median, minimum and maximum of some measurements.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Stat {
    pub n: usize,
    pub median: f64,
    pub min: f64,
    pub max: f64,
}

/// The median of an even count is the mean of the middle two. `None` for
/// no finite values.
pub fn stat(values: impl IntoIterator<Item = f64>) -> Option<Stat> {
    let mut v: Vec<f64> = values.into_iter().filter(|x| x.is_finite()).collect();
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    let median = if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    };
    Some(Stat {
        n,
        median,
        min: v[0],
        max: v[n - 1],
    })
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ArmSummary {
    pub name: String,
    pub features: Option<String>,
    /// Steps run, and steps that failed or left no usable report.
    pub steps: usize,
    pub failed: usize,
    /// `wall.speedup` and `wall.ms_per_tick`, one value per report.
    pub speedup: Option<Stat>,
    pub ms_per_tick: Option<Stat>,
}

/// Where the first state difference between two runs is.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Divergence {
    /// The step it differs in and the step it was compared with.
    pub step: String,
    pub against: String,
    /// The report within the step (`report.json` for a match).
    pub report: String,
    /// `None` when the reports can't be compared by tick.
    pub tick: Option<u32>,
    /// `h2 sim diff`'s verdict text.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Hashes {
    /// `identical`, `differ`, `unverifiable` or `nothing to compare`.
    pub status: &'static str,
    /// Every repeat of every arm has the same state hashes: `status` is
    /// `identical` and every step ran.
    pub identical: bool,
    /// Report pairs compared.
    pub compared: usize,
    pub first_difference: Option<Divergence>,
    /// Why some pairs couldn't be compared.
    pub notes: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    pub name: String,
    pub repeat: u32,
    pub arms: Vec<ArmSummary>,
    pub hashes: Hashes,
}

fn verdict_tick(v: &Verdict) -> Option<u32> {
    match v {
        Verdict::Components(d) => Some(d.tick),
        Verdict::Aggregate(d) => Some(d.tick),
        _ => None,
    }
}

/// Medians per arm and whether every run played the same match. Every
/// report is compared with the first usable one by `sim_bench::diff` (the
/// component series when both have it, else the digests), and the earliest
/// differing tick is the one named.
pub fn summarize(req: &SimReq, steps: &[StepResult]) -> Summary {
    let label = |s: &StepResult| step_name(&req.arms[s.arm].name, s.repeat);
    let arms = req
        .arms
        .iter()
        .enumerate()
        .map(|(i, arm)| {
            let mine: Vec<&StepResult> = steps.iter().filter(|s| s.arm == i).collect();
            let reports = || mine.iter().copied().filter(|s| s.ok).flat_map(|s| s.reports.values());
            ArmSummary {
                name: arm.name.clone(),
                features: arm.features.clone(),
                steps: mine.len(),
                failed: mine.iter().filter(|s| !s.ok).count(),
                speedup: stat(reports().map(|r| r.wall.speedup)),
                ms_per_tick: stat(reports().map(|r| r.wall.ms_per_tick)),
            }
        })
        .collect();

    let usable: Vec<&StepResult> = steps.iter().filter(|s| s.ok).collect();
    let mut hashes = Hashes {
        status: "nothing to compare",
        identical: false,
        compared: 0,
        first_difference: None,
        notes: Vec::new(),
    };
    let failed = steps.len() - usable.len();
    if failed > 0 {
        hashes
            .notes
            .push(format!("{failed} of {} runs failed or left no report", steps.len()));
    }
    for s in steps.iter().filter(|s| !s.ok) {
        hashes.notes.push(format!(
            "{} failed: {}",
            label(s),
            s.error.as_deref().unwrap_or("no report")
        ));
    }
    if let Some((base, rest)) = usable.split_first() {
        let mut unverifiable = false;
        for s in rest {
            let keys: std::collections::BTreeSet<&String> =
                base.reports.keys().chain(s.reports.keys()).collect();
            for key in keys {
                let (a, b) = (base.reports.get(key), s.reports.get(key));
                let (Some(a), Some(b)) = (a, b) else {
                    hashes.notes.push(format!(
                        "{}: {key} is missing from {}",
                        label(s),
                        label(if a.is_some() { s } else { base })
                    ));
                    unverifiable = true;
                    continue;
                };
                hashes.compared += 1;
                let v = diff::diff(a, b);
                if matches!(v, Verdict::Unverifiable(_)) {
                    unverifiable = true;
                    hashes.notes.push(format!("{}: {key}: {}", label(s), v.render().trim()));
                } else if v.differs() {
                    let tick = verdict_tick(&v);
                    let earlier = match (&hashes.first_difference, tick) {
                        (None, _) => true,
                        (Some(d), Some(t)) => d.tick.is_none_or(|p| t < p),
                        _ => false,
                    };
                    if earlier {
                        hashes.first_difference = Some(Divergence {
                            step: label(s),
                            against: label(base),
                            report: key.clone(),
                            tick,
                            detail: v.render(),
                        });
                    }
                }
            }
        }
        hashes.status = if hashes.first_difference.is_some() {
            "differ"
        } else if unverifiable {
            "unverifiable"
        } else if hashes.compared == 0 {
            "nothing to compare"
        } else {
            "identical"
        };
        hashes.identical = hashes.status == "identical" && failed == 0;
    }
    Summary {
        name: req.name.clone(),
        repeat: req.repeat,
        arms,
        hashes,
    }
}

impl Summary {
    /// The text printed with `--wait` and kept as `sim-summary.txt`.
    pub fn render(&self) -> String {
        use std::fmt::Write as _;
        let mut out = String::new();
        let runs: usize = self.arms.iter().map(|a| a.steps).sum();
        let failed: usize = self.arms.iter().map(|a| a.failed).sum();
        let _ = writeln!(
            out,
            "sim benchmark {}: {} arm(s) x {} repeat(s), {runs} runs, {failed} failed",
            self.name,
            self.arms.len(),
            self.repeat
        );
        let _ = writeln!(
            out,
            "{:<14} {:>5}  {:>8} {:>8} {:>8}   {:>9} {:>9} {:>9}",
            "arm", "n", "speedup", "min", "max", "ms/tick", "min", "max"
        );
        let cell = |s: &Option<Stat>, f: fn(&Stat) -> f64| {
            s.as_ref().map_or("-".to_owned(), |s| format!("{:.3}", f(s)))
        };
        for a in &self.arms {
            let _ = writeln!(
                out,
                "{:<14} {:>5}  {:>8} {:>8} {:>8}   {:>9} {:>9} {:>9}",
                a.name,
                a.speedup.as_ref().map_or(0, |s| s.n),
                cell(&a.speedup, |s| s.median),
                cell(&a.speedup, |s| s.min),
                cell(&a.speedup, |s| s.max),
                cell(&a.ms_per_tick, |s| s.median),
                cell(&a.ms_per_tick, |s| s.min),
                cell(&a.ms_per_tick, |s| s.max),
            );
        }
        if let Some(first) = self.arms.first() {
            for a in &self.arms[1..] {
                if let (Some(x), Some(y), Some(sx), Some(sy)) =
                    (&first.ms_per_tick, &a.ms_per_tick, &first.speedup, &a.speedup)
                    && x.median > 0.0
                    && sx.median > 0.0
                {
                    let _ = writeln!(
                        out,
                        "{} vs {}: median ms/tick {:+.1}%, median speedup {:+.1}%",
                        a.name,
                        first.name,
                        (y.median / x.median - 1.0) * 100.0,
                        (sy.median / sx.median - 1.0) * 100.0
                    );
                }
            }
        }
        let h = &self.hashes;
        match (&h.first_difference, h.status) {
            (Some(d), _) => {
                let _ = writeln!(
                    out,
                    "state hashes: DIFFER, first at tick {} ({} against {}, {})",
                    d.tick.map_or("?".to_owned(), |t| t.to_string()),
                    d.step,
                    d.against,
                    d.report
                );
            }
            (None, "identical") if h.identical => {
                let _ = writeln!(
                    out,
                    "state hashes: identical across all runs and arms ({} report pair(s) compared)",
                    h.compared
                );
            }
            (None, "identical") => {
                let _ = writeln!(
                    out,
                    "state hashes: identical among the runs that finished ({} pair(s)); not all runs did",
                    h.compared
                );
            }
            (None, status) => {
                let _ = writeln!(out, "state hashes: {status}");
            }
        }
        for n in &h.notes {
            let _ = writeln!(out, "  note: {n}");
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sim_bench::metrics::{End, Finish, Tally, TickHash};

    fn arms(n: &[&str]) -> Vec<SimArm> {
        n.iter()
            .map(|n| SimArm {
                name: (*n).into(),
                features: None,
            })
            .collect()
    }

    fn report(speedup: f64, ms: f64, hashes: &[(u32, &str)]) -> Report {
        let mut r = Tally::new().report(Finish::bare(End::TickLimit));
        r.wall.speedup = speedup;
        r.wall.ms_per_tick = ms;
        r.state_hashes_v1 = hashes
            .iter()
            .map(|(tick, h)| TickHash {
                tick: *tick,
                gameplay: (*h).into(),
                arena: None,
            })
            .collect();
        r
    }

    fn step(arm: usize, repeat: u32, r: Report) -> StepResult {
        StepResult {
            arm,
            repeat,
            ok: true,
            error: None,
            reports: BTreeMap::from([("report.json".to_owned(), r)]),
        }
    }

    fn req(names: &[&str], repeat: u32) -> SimReq {
        SimReq {
            name: "t".into(),
            arms: arms(names),
            repeat,
            ..SimReq::default()
        }
    }

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn arguments_become_arms_repeats_and_a_build() {
        let p = parse_args(args(&[
            "m/lockout.toml",
            "--repeat",
            "3",
            "--arm",
            "on=impl.sim.native",
            "--arm",
            "off=-impl.sim.native",
            "--arm",
            "plain=",
            "--env",
            "H2_FEATURES=x",
            "--build-env",
            "H2_OPT=2",
            "--feature",
            "perf",
            "--mem-limit",
            "4G",
            "--wait",
            "--to",
            "out",
        ]))
        .unwrap();
        assert_eq!(p.path, PathBuf::from("m/lockout.toml"));
        assert_eq!(p.req.repeat, 3);
        assert_eq!(p.req.name, "sim lockout");
        let feats: Vec<_> = p.req.arms.iter().map(|a| (&*a.name, a.features.as_deref())).collect();
        assert_eq!(
            feats,
            [
                ("on", Some("impl.sim.native")),
                ("off", Some("-impl.sim.native")),
                ("plain", None)
            ]
        );
        assert_eq!(p.req.env["H2_FEATURES"], "x");
        assert_eq!(p.req.build.env["H2_OPT"], "2");
        assert_eq!(p.req.build.features, ["perf"]);
        assert_eq!(p.req.mem_limit, Some(4 << 30));
        assert!(p.wait && p.to == Some(PathBuf::from("out")));
        assert!(p.req.parallel.is_none(), "sequential unless asked");
    }

    #[test]
    fn defaults_are_one_arm_one_repeat_and_a_name_from_the_file() {
        let p = parse_args(args(&["--parallel", "2", "docs/b.toml"])).unwrap();
        assert_eq!(p.req.arms, arms(&["default"]));
        assert_eq!(p.req.repeat, 1);
        assert_eq!(p.req.parallel, Some(2));
        assert_eq!(p.req.name, "sim b");
        assert!(!p.wait);
        let named = parse_args(args(&["--name", "My A/B", "x.toml"])).unwrap();
        assert_eq!(named.req.name, "My A/B");
    }

    #[test]
    fn bad_arguments_are_refused() {
        for bad in [
            args(&[]),
            args(&["a.toml", "b.toml"]),
            args(&["a.toml", "--repeat"]),
            args(&["a.toml", "--repeat", "x"]),
            args(&["a.toml", "--arm", "noequals"]),
            args(&["a.toml", "--bogus"]),
            args(&["a.toml", "--env", "NOEQ"]),
        ] {
            assert!(parse_args(bad.clone()).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn arms_interleave_by_repeat_and_reverse_on_odd_repeats() {
        assert_eq!(
            schedule(2, 3),
            [(0, 0), (1, 0), (1, 1), (0, 1), (0, 2), (1, 2)]
        );
        assert_eq!(schedule(1, 3), [(0, 0), (0, 1), (0, 2)]);
        assert_eq!(schedule(3, 1), [(0, 0), (1, 0), (2, 0)]);
        // Every arm runs every repeat exactly once.
        let s = schedule(4, 5);
        assert_eq!(s.len(), 20);
        for a in 0..4 {
            for r in 0..5 {
                assert_eq!(s.iter().filter(|x| **x == (a, r)).count(), 1);
            }
        }
        assert_eq!(step_name("on", 0), "on-r1");
    }

    #[test]
    fn median_min_and_max_over_odd_even_and_single_samples() {
        assert_eq!(stat([3.0, 1.0, 2.0]).unwrap(), Stat { n: 3, median: 2.0, min: 1.0, max: 3.0 });
        assert_eq!(stat([4.0, 1.0, 2.0, 3.0]).unwrap().median, 2.5);
        assert_eq!(stat([7.0]).unwrap(), Stat { n: 1, median: 7.0, min: 7.0, max: 7.0 });
        assert_eq!(stat([f64::NAN, 5.0]).unwrap().n, 1);
        assert!(stat([]).is_none());
    }

    #[test]
    fn identical_hashes_across_arms_and_repeats_are_reported_as_such() {
        let h = [(1, "aa"), (2, "bb")];
        let r = req(&["on", "off"], 2);
        let steps = vec![
            step(0, 0, report(4.0, 8.0, &h)),
            step(1, 0, report(3.0, 10.0, &h)),
            step(1, 1, report(3.2, 9.5, &h)),
            step(0, 1, report(4.2, 7.5, &h)),
        ];
        let s = summarize(&r, &steps);
        assert!(s.hashes.identical);
        assert_eq!(s.hashes.status, "identical");
        assert_eq!(s.hashes.compared, 3);
        let on = &s.arms[0];
        assert!((on.speedup.as_ref().unwrap().median - 4.1).abs() < 1e-9);
        assert_eq!(on.ms_per_tick.as_ref().unwrap().min, 7.5);
        assert_eq!(s.arms[1].speedup.as_ref().unwrap().max, 3.2);
        let text = s.render();
        assert!(text.contains("identical across all runs and arms (3 report pair(s) compared)"), "{text}");
        assert!(text.contains("off vs on: median ms/tick +"), "{text}");
    }

    #[test]
    fn the_earliest_differing_tick_is_named() {
        let a = [(1, "aa"), (2, "bb"), (3, "cc")];
        let late = [(1, "aa"), (2, "bb"), (3, "XX")];
        let early = [(1, "aa"), (2, "YY"), (3, "cc")];
        let r = req(&["on", "off"], 2);
        let steps = vec![
            step(0, 0, report(4.0, 8.0, &a)),
            step(1, 0, report(3.0, 10.0, &late)),
            step(1, 1, report(3.0, 10.0, &early)),
            step(0, 1, report(4.0, 8.0, &a)),
        ];
        let s = summarize(&r, &steps);
        assert!(!s.hashes.identical);
        assert_eq!(s.hashes.status, "differ");
        let d = s.hashes.first_difference.as_ref().unwrap();
        assert_eq!((d.tick, d.step.as_str(), d.against.as_str()), (Some(2), "off-r2", "on-r1"));
        assert!(s.render().contains("DIFFER, first at tick 2 (off-r2 against on-r1, report.json)"));
    }

    #[test]
    fn failed_runs_and_reports_without_hashes_are_not_called_identical() {
        let h = [(1, "aa")];
        let r = req(&["on", "off"], 1);
        let mut failed = step(1, 0, report(1.0, 1.0, &h));
        failed.ok = false;
        failed.reports.clear();
        let s = summarize(&r, &[step(0, 0, report(4.0, 8.0, &h)), failed]);
        assert!(!s.hashes.identical);
        assert_eq!(s.arms[1].failed, 1);
        assert!(s.arms[1].speedup.is_none());
        assert_eq!(s.hashes.status, "nothing to compare");
        assert!(s.render().contains("1 failed"));

        let s = summarize(
            &r,
            &[step(0, 0, report(4.0, 8.0, &h)), step(1, 0, report(4.0, 8.0, &[]))],
        );
        assert_eq!(s.hashes.status, "unverifiable");
        assert!(!s.hashes.identical);
    }

    #[test]
    fn a_batch_compares_each_match_by_its_path_and_flags_a_missing_one() {
        let h = [(1, "aa")];
        let two = |a: &Report, b: &Report| StepResult {
            arm: 0,
            repeat: 0,
            ok: true,
            error: None,
            reports: BTreeMap::from([
                ("out/runs/m1/report.json".to_owned(), a.clone()),
                ("out/runs/m2/report.json".to_owned(), b.clone()),
            ]),
        };
        let (x, y) = (report(2.0, 5.0, &h), report(4.0, 3.0, &h));
        let mut second = two(&x, &y);
        second.repeat = 1;
        let r = req(&["default"], 2);
        let s = summarize(&r, &[two(&x, &y), second]);
        assert_eq!(s.hashes.status, "identical");
        assert_eq!(s.hashes.compared, 2);
        // One value per report: two matches times two repeats.
        assert_eq!(s.arms[0].speedup.as_ref().unwrap().n, 4);

        let mut short = two(&x, &y);
        short.repeat = 1;
        short.reports.remove("out/runs/m2/report.json");
        let s = summarize(&r, &[two(&x, &y), short]);
        assert_eq!(s.hashes.status, "unverifiable");
        assert!(s.hashes.notes.iter().any(|n| n.contains("m2") && n.contains("missing")));
    }

    #[test]
    fn a_batchs_results_stay_inside_the_job() {
        check_batch_out("results/x").unwrap();
        check_batch_out("./r").unwrap();
        for bad in ["", "/tmp/x", "../x", "a/../../x"] {
            assert!(check_batch_out(bad).is_err(), "{bad:?}");
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("h2simbench-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p.canonicalize().unwrap()
    }

    #[test]
    fn a_batch_brings_its_match_and_every_tuning_file_with_it() {
        let d = scratch("files");
        fs::create_dir_all(d.join("tuning")).unwrap();
        fs::create_dir_all(d.join("batches")).unwrap();
        fs::write(d.join("tuning/base.toml"), "skill = 1\n").unwrap();
        fs::write(d.join("tuning/hot.toml"), "skill = 2\n").unwrap();
        fs::write(
            d.join("match.toml"),
            "map = \"lockout\"\n[[bot]]\nparams = \"tuning/base.toml\"\n[[bot]]\nname = \"x\"\n",
        )
        .unwrap();
        fs::write(
            d.join("batches/ab.toml"),
            "out = \"results\"\nmatch = \"../match.toml\"\n[a]\nparams = \"../tuning/base.toml\"\n[b]\nparams = \"../tuning/hot.toml\"\n",
        )
        .unwrap();
        let l = load_files(&d.join("batches/ab.toml")).unwrap();
        assert!(l.batch);
        assert_eq!(l.file, "batches/ab.toml");
        let keys: Vec<_> = l.files.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["batches/ab.toml", "match.toml", "tuning/base.toml", "tuning/hot.toml"]
        );
        // The layout is mirrored, so the same relative references resolve.
        let host = d.join("host");
        write_inputs(&host, &l.files).unwrap();
        assert!(host.join("batches/../tuning/hot.toml").is_file());

        // A single match: its tuning files, rooted at its own directory.
        let m = load_files(&d.join("match.toml")).unwrap();
        assert!(!m.batch);
        assert_eq!(m.file, "match.toml");
        assert_eq!(m.files.len(), 2);
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn a_missing_tuning_file_or_an_escaping_out_is_refused_before_upload() {
        let d = scratch("bad");
        fs::write(d.join("m.toml"), "[[bot]]\nparams = \"gone.toml\"\n").unwrap();
        let e = format!("{:#}", load_files(&d.join("m.toml")).unwrap_err());
        assert!(e.contains("gone.toml"), "{e}");
        fs::write(d.join("m2.toml"), "map = \"x\"\n").unwrap();
        fs::write(d.join("b.toml"), "out = \"../escape\"\nmatch = \"m2.toml\"\n[a]\n").unwrap();
        let e = format!("{:#}", load_files(&d.join("b.toml")).unwrap_err());
        assert!(e.contains("no `..`"), "{e}");
        let _ = fs::remove_dir_all(&d);
    }

    #[test]
    fn reports_are_collected_per_step_and_run_data_is_removed() {
        let d = scratch("collect");
        let good = serde_json::to_vec(&report(1.0, 2.0, &[(1, "aa")])).unwrap();
        for p in ["report.json", "in/res/runs/m1/report.json", "in/res/runs/m2/report.json"] {
            fs::create_dir_all(d.join(p).parent().unwrap()).unwrap();
            fs::write(d.join(p), &good).unwrap();
        }
        fs::create_dir_all(d.join("in/res/runs/m1/data/sub")).unwrap();
        fs::write(d.join("in/res/runs/m1/data/sub/big"), b"x").unwrap();
        fs::write(d.join("in/res/runs/m2/log.txt"), "keep").unwrap();
        // A `data` directory that isn't a run's is left alone.
        fs::create_dir_all(d.join("in/data")).unwrap();
        let (reports, bad) = collect_reports(&d);
        assert!(bad.is_empty(), "{bad:?}");
        let keys: Vec<_> = reports.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["in/res/runs/m1/report.json", "in/res/runs/m2/report.json", "report.json"]
        );
        remove_run_data(&d);
        assert!(!d.join("in/res/runs/m1/data").exists());
        assert!(d.join("in/res/runs/m1/report.json").is_file());
        assert!(d.join("in/res/runs/m2/log.txt").is_file());
        assert!(d.join("in/data").is_dir());
        fs::write(d.join("report.json"), "{not json").unwrap();
        let (_, bad) = collect_reports(&d);
        assert_eq!(bad.len(), 1);
        let _ = fs::remove_dir_all(&d);
    }
}
