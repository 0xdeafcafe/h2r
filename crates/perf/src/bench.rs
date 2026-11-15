//! The benchmark harness: repeatable scenarios, results as JSON, and a
//! comparison that flags regressions beyond a threshold and the noise.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};

/// Which way is better for a metric.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Better {
    Lower,
    Higher,
}

/// Settings every scenario sees.
#[derive(Debug, Clone)]
pub struct BenchCtx {
    /// Scratch for builds (generated C, objects, dylibs). Under `$TMPDIR`.
    pub work: PathBuf,
    /// Smaller workloads, for tests and smoke runs.
    pub quick: bool,
    /// Frames the boot scenario holds the menu for.
    pub frames: u32,
}

/// One measured sample.
#[derive(Debug, Clone, Default)]
pub struct Measurement {
    /// In the scenario's unit.
    pub value: f64,
    /// Secondary numbers, averaged over the samples (counts, rates).
    pub extra: BTreeMap<String, f64>,
}

impl Measurement {
    pub fn new(value: f64) -> Self {
        Measurement {
            value,
            extra: BTreeMap::new(),
        }
    }
    pub fn with(mut self, k: &str, v: f64) -> Self {
        self.extra.insert(k.into(), v);
        self
    }
}

/// A scenario's answer to [`Scenario::prepare`].
pub enum Prepared {
    Ready,
    /// Not runnable here (no clang, no boot driver, no disc): recorded as
    /// skipped with the reason.
    Skip(String),
}

/// A repeatable benchmark.
pub trait Scenario {
    /// Stable id (`hot-loop/alu`): results are matched on it.
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn unit(&self) -> &str;
    fn better(&self) -> Better;
    /// Build and load whatever it needs. Not timed.
    fn prepare(&mut self, ctx: &BenchCtx) -> Result<Prepared>;
    /// One sample.
    fn measure(&mut self) -> Result<Measurement>;
}

/// Summary statistics of the samples.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct Stats {
    pub median: f64,
    pub mean: f64,
    pub min: f64,
    pub max: f64,
    pub stddev: f64,
    /// Median absolute deviation.
    pub mad: f64,
}

impl Stats {
    pub fn of(samples: &[f64]) -> Option<Stats> {
        if samples.is_empty() {
            return None;
        }
        let mut v = samples.to_vec();
        v.sort_by(f64::total_cmp);
        let median = median_sorted(&v);
        let n = v.len() as f64;
        let mean = v.iter().sum::<f64>() / n;
        let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n;
        let mut dev: Vec<f64> = v.iter().map(|x| (x - median).abs()).collect();
        dev.sort_by(f64::total_cmp);
        Some(Stats {
            median,
            mean,
            min: v[0],
            max: v[v.len() - 1],
            stddev: var.sqrt(),
            mad: median_sorted(&dev),
        })
    }
}

fn median_sorted(v: &[f64]) -> f64 {
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScenarioResult {
    pub scenario: String,
    pub description: String,
    pub unit: String,
    pub better: Better,
    pub samples: Vec<f64>,
    pub stats: Option<Stats>,
    #[serde(default)]
    pub extra: BTreeMap<String, f64>,
    #[serde(default)]
    pub skipped: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub os: String,
    pub arch: String,
    pub cpus: usize,
}

/// One `h2 bench` run: what is written to `<out>/<stamp>-<label>.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BenchRun {
    pub schema: u32,
    pub label: String,
    pub git: Option<String>,
    pub git_dirty: Option<bool>,
    pub host: Host,
    pub started_unix: u64,
    pub samples: usize,
    pub warmup: usize,
    pub results: Vec<ScenarioResult>,
}

pub const SCHEMA: u32 = 1;

/// How many times to run each scenario.
#[derive(Debug, Clone, Copy)]
pub struct Plan {
    pub warmup: usize,
    pub samples: usize,
}

impl Default for Plan {
    fn default() -> Self {
        Plan {
            warmup: 2,
            samples: 10,
        }
    }
}

/// `git <args>` in the current directory, else in the source tree this
/// binary was built from.
fn git(args: &[&str]) -> Option<String> {
    let run = |dir: Option<&str>| {
        let mut c = std::process::Command::new("git");
        if let Some(d) = dir {
            c.arg("-C").arg(d);
        }
        let o = c.args(args).output().ok()?;
        o.status
            .success()
            .then(|| String::from_utf8_lossy(&o.stdout).trim().to_owned())
    };
    run(None).or_else(|| run(Some(env!("CARGO_MANIFEST_DIR"))))
}

/// Run `scenarios`, reporting progress through `log`. A scenario that
/// fails to prepare or measure is recorded as skipped with the error; the
/// run carries on.
pub fn run(
    scenarios: &mut [Box<dyn Scenario>],
    ctx: &BenchCtx,
    plan: Plan,
    label: &str,
    mut log: impl FnMut(&str),
) -> BenchRun {
    let started_unix = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let mut results = Vec::new();
    for s in scenarios.iter_mut() {
        let mut r = ScenarioResult {
            scenario: s.name().into(),
            description: s.description().into(),
            unit: s.unit().into(),
            better: s.better(),
            samples: vec![],
            stats: None,
            extra: BTreeMap::new(),
            skipped: None,
        };
        match one(s.as_mut(), ctx, plan, &mut r) {
            Ok(()) => {}
            Err(e) => r.skipped = Some(format!("{e:#}")),
        }
        r.stats = Stats::of(&r.samples);
        match (&r.skipped, &r.stats) {
            (Some(why), _) => log(&format!("{:<24} skipped: {why}", r.scenario)),
            (None, Some(st)) => log(&format!(
                "{:<24} {:>12.2} {:<9} (min {:.2}, mad {:.2}, n={})",
                r.scenario,
                st.median,
                r.unit,
                st.min,
                st.mad,
                r.samples.len()
            )),
            (None, None) => {}
        }
        results.push(r);
    }
    BenchRun {
        schema: SCHEMA,
        label: label.into(),
        git: git(&["rev-parse", "--short=12", "HEAD"]),
        git_dirty: git(&["status", "--porcelain", "--untracked-files=no"]).map(|s| !s.is_empty()),
        host: Host {
            os: std::env::consts::OS.into(),
            arch: std::env::consts::ARCH.into(),
            cpus: std::thread::available_parallelism().map_or(1, |n| n.get()),
        },
        started_unix,
        samples: plan.samples,
        warmup: plan.warmup,
        results,
    }
}

fn one(s: &mut dyn Scenario, ctx: &BenchCtx, plan: Plan, r: &mut ScenarioResult) -> Result<()> {
    if let Prepared::Skip(why) = s.prepare(ctx)? {
        r.skipped = Some(why);
        return Ok(());
    }
    for _ in 0..plan.warmup {
        s.measure()?;
    }
    let mut extra: BTreeMap<String, f64> = BTreeMap::new();
    for _ in 0..plan.samples.max(1) {
        let m = s.measure()?;
        r.samples.push(m.value);
        for (k, v) in m.extra {
            *extra.entry(k).or_default() += v;
        }
    }
    let n = r.samples.len() as f64;
    r.extra = extra.into_iter().map(|(k, v)| (k, v / n)).collect();
    Ok(())
}

/// A file name for a run: `<unix seconds>-<label>.json`, sorting by time.
pub fn file_name(run: &BenchRun) -> String {
    let label: String = run
        .label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{}-{label}.json", run.started_unix)
}

/// Write `run` into `dir` (created if needed) and return the path. Never
/// overwrites: a clash gets a numeric suffix.
pub fn save(run: &BenchRun, dir: &Path) -> Result<PathBuf> {
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let base = file_name(run);
    let mut path = dir.join(&base);
    let mut n = 1;
    while path.exists() {
        path = dir.join(base.replace(".json", &format!("-{n}.json")));
        n += 1;
    }
    std::fs::write(&path, serde_json::to_vec_pretty(run)?)?;
    Ok(path)
}

pub fn load(path: &Path) -> Result<BenchRun> {
    let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    let run: BenchRun =
        serde_json::from_slice(&bytes).with_context(|| format!("parsing {}", path.display()))?;
    if run.schema != SCHEMA {
        bail!(
            "{}: schema {} (this tool reads {SCHEMA})",
            path.display(),
            run.schema
        );
    }
    Ok(run)
}

/// The run files in `dir`, oldest first.
pub fn runs_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .with_context(|| format!("reading {}", dir.display()))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|e| e == "json"))
        .collect();
    v.sort_by_key(|p| {
        let stamp = p
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.split('-').next())
            .and_then(|s| s.parse::<u64>().ok());
        (stamp, p.clone())
    });
    Ok(v)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
    Regressed,
    Improved,
    Same,
    /// Skipped or missing on one side.
    Incomparable,
}

#[derive(Debug, Clone, Serialize)]
pub struct Comparison {
    pub scenario: String,
    pub unit: String,
    pub base: Option<f64>,
    pub new: Option<f64>,
    /// Positive is worse, in percent of the base.
    pub worse_pct: Option<f64>,
    /// The change that counts as real: the larger of the threshold and the
    /// samples' spread (MAD) as a percentage, times two.
    pub bar_pct: Option<f64>,
    pub verdict: Verdict,
    pub note: String,
}

/// Compare medians scenario by scenario. A change counts when it beats
/// both `threshold_pct` and twice the larger relative MAD of the sides.
pub fn compare(base: &BenchRun, new: &BenchRun, threshold_pct: f64) -> Vec<Comparison> {
    let mut names: Vec<&str> = base.results.iter().map(|r| r.scenario.as_str()).collect();
    for r in &new.results {
        if !names.contains(&r.scenario.as_str()) {
            names.push(&r.scenario);
        }
    }
    let find = |run: &'_ BenchRun, n: &str| run.results.iter().find(|r| r.scenario == n).cloned();
    names
        .into_iter()
        .map(|n| {
            let (b, w) = (find(base, n), find(new, n));
            let unit = b
                .as_ref()
                .or(w.as_ref())
                .map_or(String::new(), |r| r.unit.clone());
            let stats = |r: &Option<ScenarioResult>| {
                r.as_ref()
                    .filter(|r| r.skipped.is_none())
                    .and_then(|r| r.stats)
            };
            let mut c = Comparison {
                scenario: n.into(),
                unit,
                base: stats(&b).map(|s| s.median),
                new: stats(&w).map(|s| s.median),
                worse_pct: None,
                bar_pct: None,
                verdict: Verdict::Incomparable,
                note: String::new(),
            };
            let (Some(bs), Some(ns), Some(br)) = (stats(&b), stats(&w), b.as_ref()) else {
                c.note = match (&b, &w) {
                    (None, _) => "only in new".into(),
                    (_, None) => "only in base".into(),
                    (Some(b), _) if b.skipped.is_some() => {
                        format!("base skipped: {}", b.skipped.as_deref().unwrap())
                    }
                    (_, Some(w)) => format!(
                        "new skipped: {}",
                        w.skipped.as_deref().unwrap_or("no samples")
                    ),
                };
                return c;
            };
            if w.as_ref().is_some_and(|w| w.unit != br.unit) {
                c.note = "units differ".into();
                return c;
            }
            if bs.median == 0.0 {
                c.note = "base median is zero".into();
                return c;
            }
            let change = (ns.median - bs.median) / bs.median * 100.0;
            let worse = match br.better {
                Better::Lower => change,
                Better::Higher => -change,
            };
            let rel = |s: Stats| {
                if s.median == 0.0 {
                    0.0
                } else {
                    s.mad / s.median.abs() * 100.0
                }
            };
            let bar = threshold_pct.max(2.0 * rel(bs).max(rel(ns)));
            c.worse_pct = Some(worse);
            c.bar_pct = Some(bar);
            c.verdict = if worse > bar {
                Verdict::Regressed
            } else if -worse > bar {
                Verdict::Improved
            } else {
                Verdict::Same
            };
            c
        })
        .collect()
}

/// The comparison as a text table.
pub fn comparison_table(base: &BenchRun, new: &BenchRun, rows: &[Comparison]) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    let id = |r: &BenchRun| format!("{} @ {}", r.label, r.git.as_deref().unwrap_or("?"));
    let _ = writeln!(o, "base: {}\nnew:  {}\n", id(base), id(new));
    let _ = writeln!(
        o,
        "{:<24} {:>12} {:>12} {:>9} {:>7}  verdict",
        "scenario", "base", "new", "worse", "bar"
    );
    for c in rows {
        let num = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:.2}"));
        let p = |v: Option<f64>| v.map_or("-".into(), |v| format!("{v:+.1}%"));
        let verdict = match c.verdict {
            Verdict::Regressed => "REGRESSED",
            Verdict::Improved => "improved",
            Verdict::Same => "same",
            Verdict::Incomparable => "-",
        };
        let _ = writeln!(
            o,
            "{:<24} {:>12} {:>12} {:>9} {:>7}  {verdict} {}{}",
            c.scenario,
            num(c.base),
            num(c.new),
            p(c.worse_pct),
            c.bar_pct.map_or("-".into(), |b| format!("{b:.1}%")),
            c.unit,
            if c.note.is_empty() {
                String::new()
            } else {
                format!(" ({})", c.note)
            }
        );
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed {
        name: &'static str,
        values: Vec<f64>,
        i: usize,
        skip: bool,
    }
    impl Scenario for Fixed {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            "fixed values"
        }
        fn unit(&self) -> &str {
            "ns"
        }
        fn better(&self) -> Better {
            Better::Lower
        }
        fn prepare(&mut self, _: &BenchCtx) -> Result<Prepared> {
            Ok(if self.skip {
                Prepared::Skip("not here".into())
            } else {
                Prepared::Ready
            })
        }
        fn measure(&mut self) -> Result<Measurement> {
            let v = self.values[self.i % self.values.len()];
            self.i += 1;
            Ok(Measurement::new(v).with("count", 2.0))
        }
    }

    fn ctx() -> BenchCtx {
        BenchCtx {
            work: std::env::temp_dir(),
            quick: true,
            frames: 1,
        }
    }

    fn fixed(name: &'static str, values: Vec<f64>) -> Box<dyn Scenario> {
        Box::new(Fixed {
            name,
            values,
            i: 0,
            skip: false,
        })
    }

    #[test]
    fn stats_of_samples() {
        let s = Stats::of(&[3.0, 1.0, 2.0, 10.0]).unwrap();
        assert_eq!(s.median, 2.5);
        assert_eq!(s.min, 1.0);
        assert_eq!(s.max, 10.0);
        assert_eq!(s.mean, 4.0);
        assert_eq!(s.mad, 1.0);
        assert!(Stats::of(&[]).is_none());
    }

    #[test]
    fn runs_record_samples_extras_and_skips() {
        let mut v: Vec<Box<dyn Scenario>> = vec![
            fixed("a", vec![1.0, 2.0, 3.0]),
            Box::new(Fixed {
                name: "b",
                values: vec![1.0],
                i: 0,
                skip: true,
            }),
        ];
        let r = run(
            &mut v,
            &ctx(),
            Plan {
                warmup: 1,
                samples: 3,
            },
            "test run",
            |_| {},
        );
        assert_eq!(
            r.results[0].samples,
            vec![2.0, 3.0, 1.0],
            "after one warmup"
        );
        assert_eq!(r.results[0].extra["count"], 2.0);
        assert_eq!(r.results[1].skipped.as_deref(), Some("not here"));
        assert!(file_name(&r).ends_with("-test_run.json"));
    }

    #[test]
    fn saved_runs_load_back_and_never_overwrite() {
        let dir = std::env::temp_dir().join(format!("perf-bench-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut v = vec![fixed("a", vec![1.0])];
        let r = run(
            &mut v,
            &ctx(),
            Plan {
                warmup: 0,
                samples: 2,
            },
            "x",
            |_| {},
        );
        let p1 = save(&r, &dir).unwrap();
        let p2 = save(&r, &dir).unwrap();
        assert_ne!(p1, p2);
        assert_eq!(load(&p1).unwrap().results[0].samples, vec![1.0, 1.0]);
        assert_eq!(runs_in(&dir).unwrap().len(), 2);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn comparison_flags_changes_beyond_threshold_and_noise() {
        let mk = |name: &str, samples: Vec<f64>, better| ScenarioResult {
            scenario: name.into(),
            description: String::new(),
            unit: "ns".into(),
            better,
            stats: Stats::of(&samples),
            samples,
            extra: BTreeMap::new(),
            skipped: None,
        };
        let runs = |rs: Vec<ScenarioResult>| BenchRun {
            schema: SCHEMA,
            label: "l".into(),
            git: None,
            git_dirty: None,
            host: Host {
                os: "x".into(),
                arch: "y".into(),
                cpus: 1,
            },
            started_unix: 0,
            samples: 3,
            warmup: 0,
            results: rs,
        };
        let base = runs(vec![
            mk("slower", vec![100.0, 100.0, 100.0], Better::Lower),
            mk("faster", vec![100.0, 100.0, 100.0], Better::Lower),
            mk("noisy", vec![80.0, 100.0, 120.0], Better::Lower),
            mk("fps", vec![60.0, 60.0, 60.0], Better::Higher),
            mk("gone", vec![1.0], Better::Lower),
        ]);
        let new = runs(vec![
            mk("slower", vec![110.0, 110.0, 110.0], Better::Lower),
            mk("faster", vec![90.0, 90.0, 90.0], Better::Lower),
            mk("noisy", vec![90.0, 110.0, 130.0], Better::Lower),
            mk("fps", vec![50.0, 50.0, 50.0], Better::Higher),
        ]);
        let c = compare(&base, &new, 5.0);
        let v = |n: &str| c.iter().find(|c| c.scenario == n).unwrap().verdict;
        assert_eq!(v("slower"), Verdict::Regressed);
        assert_eq!(v("faster"), Verdict::Improved);
        assert_eq!(v("noisy"), Verdict::Same, "a 10% move inside 20% spread");
        assert_eq!(
            v("fps"),
            Verdict::Regressed,
            "fewer frames per second is worse"
        );
        assert_eq!(v("gone"), Verdict::Incomparable);
        let t = comparison_table(&base, &new, &c);
        assert!(t.contains("REGRESSED") && t.contains("only in base"), "{t}");
    }
}
