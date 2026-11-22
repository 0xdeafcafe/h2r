//! The bounce tuning sweep (docs/runtime/sim.md, "Sweeping the bounce
//! tuning"): `h2 sim sweep` plays a batch once per candidate bounce
//! tuning and ranks the candidates by their bounce rate, then by their
//! mean launch height.
//!
//! Pure: the sweep file and its axes ([`load_sweep`]), the candidates of a
//! grid, a random search or one coordinate step ([`Sweep::grid`],
//! [`Sweep::random`], [`Sweep::step`]), each candidate's batch file
//! ([`candidate_batch`]), and the ranking and its CSV, JSON and markdown
//! ([`rank`], [`csv`], [`markdown`]). The CLI runs the batches.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::bounce::Stats;
use crate::config::{Arm, BatchConfig, BounceParams, bounce_field};

/// The sweep result's schema tag.
pub const FORMAT: &str = "h2-sim-sweep/1";
/// A grid has at most this many candidates.
pub const MAX_CANDIDATES: usize = 512;
/// A candidate with fewer tries than this is listed last, unranked,
/// unless the sweep file says otherwise.
pub const MIN_ATTEMPTS: u64 = 10;

/// How the candidates are chosen.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Method {
    /// Every combination of the axes' values.
    #[default]
    Grid,
    /// `samples` draws, each axis uniform between its least and most value.
    Random,
    /// From the base, one axis at a time: each of its values with the
    /// others at the best so far; `rounds` passes over the axes.
    Coordinate,
}

impl Method {
    pub fn parse(s: &str) -> Result<Method, String> {
        match s.trim().to_ascii_lowercase().as_str() {
            "grid" => Ok(Method::Grid),
            "random" => Ok(Method::Random),
            "coordinate" => Ok(Method::Coordinate),
            _ => Err(format!("unknown method {s:?} (grid, random, coordinate)")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Method::Grid => "grid",
            Method::Random => "random",
            Method::Coordinate => "coordinate",
        }
    }
}

/// A sweep file.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SweepConfig {
    /// The results directory, relative to the sweep file.
    pub out: PathBuf,
    /// The batch each candidate plays, relative to the sweep file: its
    /// match (with `superbounce = true`), maps, seeds and parallelism.
    /// Its arm A is the candidate (its tuning kept, the candidate's bounce
    /// over it); arm B is left out.
    pub batch: PathBuf,
    /// `grid` (the default), `random` or `coordinate`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Random: how many draws.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub samples: Option<usize>,
    /// Coordinate: passes over the axes (default 2).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounds: Option<usize>,
    /// Random: the draws' seed.
    #[serde(default)]
    pub seed: u64,
    /// Fewer tries than this and a candidate is not ranked (default
    /// [`MIN_ATTEMPTS`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_attempts: Option<u64>,
    /// What every candidate starts from (default: the built-in route).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub base: Option<BounceParams>,
    /// A bounce field (`bots::tuning::BounceTuning`'s names) and the values
    /// to try.
    pub axes: BTreeMap<String, Vec<f64>>,
}

/// One tuning to try.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Candidate {
    /// `c000` (the base), `c001`, ...: its directory and arm name.
    pub id: String,
    /// The fields it sets, as `name=value` in schema order.
    pub key: String,
    pub bounce: BounceParams,
}

/// A sweep, read and checked.
#[derive(Clone, Debug, PartialEq)]
pub struct Sweep {
    pub config: SweepConfig,
    pub out: PathBuf,
    pub batch: PathBuf,
    pub method: Method,
    /// In schema order.
    pub axes: Vec<(String, Vec<f64>)>,
    pub base: BounceParams,
    pub min_attempts: u64,
}

/// `name=value` for each field `b` sets, in schema order.
pub fn key(b: &BounceParams) -> String {
    crate::config::BOUNCE_FIELDS
        .iter()
        .filter_map(|f| b.get(f.name).map(|v| format!("{}={}", f.name, trim(v))))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Whether `a` and `b` play the same: every field equal once those left
/// out are at their built-in value. A candidate that sets a field to its
/// default is the base again, and is not played twice.
pub fn same_play(a: &BounceParams, b: &BounceParams) -> bool {
    crate::config::BOUNCE_FIELDS.iter().all(|f| {
        let at = |p: &BounceParams| p.get(f.name).unwrap_or(f.default) as f32;
        at(a) == at(b)
    })
}

fn trim(v: f64) -> String {
    let s = format!("{v:.4}");
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s.is_empty() || s == "-" {
        "0".into()
    } else {
        s.into()
    }
}

/// Reads a sweep file (TOML, or JSON by its name) and checks it.
pub fn load_sweep(path: &Path) -> Result<Sweep, String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let json = path
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("json"));
    let config: SweepConfig = if json {
        serde_json::from_str(&text).map_err(|e| e.to_string())?
    } else {
        toml::from_str(&text).map_err(|e| e.to_string())?
    };
    plan(config, path.parent().unwrap_or(Path::new(".")))
}

/// Checks a sweep file's config; paths are relative to `base`. Every
/// problem is listed.
pub fn plan(config: SweepConfig, base: &Path) -> Result<Sweep, String> {
    let mut problems = Vec::new();
    let method = match config.method.as_deref() {
        Some(m) => Method::parse(m).unwrap_or_else(|e| {
            problems.push(e);
            Method::Grid
        }),
        None => Method::Grid,
    };
    let mut axes = Vec::new();
    for f in crate::config::BOUNCE_FIELDS {
        if let Some(values) = config.axes.get(f.name) {
            axes.push((f.name.to_string(), values.clone()));
        }
    }
    for (name, values) in &config.axes {
        let Some(f) = bounce_field(name) else {
            problems.push(format!(
                "axes.{name}: not a bounce field ({})",
                crate::config::bounce_names()
            ));
            continue;
        };
        if values.is_empty() {
            problems.push(format!("axes.{name}: no values"));
        }
        for v in values {
            if !(v.is_finite() && (f.min..=f.max).contains(v)) {
                problems.push(format!("axes.{name}: {v} is not {} to {}", f.min, f.max));
            }
        }
    }
    if axes.is_empty() {
        problems.push("axes: name at least one bounce field to sweep".into());
    }
    let base_params = config.base.clone().unwrap_or_default();
    problems.extend(base_params.problems("base"));
    match method {
        Method::Grid => {
            let n = axes.iter().map(|(_, v)| v.len().max(1)).product::<usize>();
            if n > MAX_CANDIDATES {
                problems.push(format!(
                    "a grid of {n} candidates: at most {MAX_CANDIDATES} (fewer values, or method = \"random\")"
                ));
            }
        }
        Method::Random => match config.samples {
            Some(0) | None => problems.push("method = \"random\" needs samples (1 or more)".into()),
            Some(n) if n > MAX_CANDIDATES => {
                problems.push(format!("samples = {n}: at most {MAX_CANDIDATES}"))
            }
            Some(_) => {}
        },
        Method::Coordinate => {
            if config.rounds == Some(0) {
                problems.push("rounds = 0: at least 1".into());
            }
        }
    }
    if !problems.is_empty() {
        return Err(problems.join("\n"));
    }
    Ok(Sweep {
        out: base.join(&config.out),
        batch: base.join(&config.batch),
        method,
        axes,
        base: base_params,
        min_attempts: config.min_attempts.unwrap_or(MIN_ATTEMPTS),
        config,
    })
}

/// SplitMix64: the random search's draws.
fn next(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *state;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

impl Sweep {
    fn candidate(&self, n: usize, b: BounceParams) -> Candidate {
        Candidate {
            id: format!("c{n:03}"),
            key: key(&b),
            bounce: b,
        }
    }

    /// The base alone: `c000`, the control every sweep plays.
    pub fn base_candidate(&self) -> Candidate {
        self.candidate(0, self.base.clone())
    }

    /// The base, then every combination of the axes' values (the first
    /// axis slowest), without repeats ([`same_play`]).
    pub fn grid(&self) -> Vec<Candidate> {
        let mut combos: Vec<BounceParams> = vec![self.base.clone()];
        for (name, values) in &self.axes {
            let mut more = Vec::new();
            for c in &combos {
                for v in values {
                    let mut b = c.clone();
                    let _ = b.set(name, *v);
                    more.push(b);
                }
            }
            combos = more;
        }
        let mut out = vec![self.base_candidate()];
        for b in combos {
            if out.iter().all(|c| !same_play(&c.bounce, &b)) {
                let n = out.len();
                out.push(self.candidate(n, b));
            }
        }
        out
    }

    /// The base, then `samples` draws: each axis uniform between its
    /// least and most value (whole fields rounded), seeded by `seed`.
    pub fn random(&self) -> Vec<Candidate> {
        let mut state = self.config.seed ^ 0x5eed_b0b0;
        let mut out = vec![self.base_candidate()];
        for _ in 0..self.config.samples.unwrap_or(0) {
            let mut b = self.base.clone();
            for (name, values) in &self.axes {
                let lo = values.iter().copied().fold(f64::INFINITY, f64::min);
                let hi = values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
                let u = (next(&mut state) >> 11) as f64 / (1u64 << 53) as f64;
                let _ = b.set(name, lo + (hi - lo) * u);
            }
            let n = out.len();
            out.push(self.candidate(n, b));
        }
        out
    }

    /// One coordinate step: axis `axis`'s values, each with the other
    /// fields at `best`; numbered from `first`. Values `best` already
    /// plays (set, or the built-in value of a field it leaves out) are
    /// left out.
    pub fn step(&self, best: &BounceParams, axis: usize, first: usize) -> Vec<Candidate> {
        let Some((name, values)) = self.axes.get(axis) else {
            return Vec::new();
        };
        let mut out: Vec<Candidate> = Vec::new();
        for v in values {
            let mut b = best.clone();
            let _ = b.set(name, *v);
            if !same_play(&b, best) && out.iter().all(|c| !same_play(&c.bounce, &b)) {
                let n = first + out.len();
                out.push(self.candidate(n, b));
            }
        }
        out
    }

    /// Coordinate: the passes over the axes.
    pub fn rounds(&self) -> usize {
        self.config.rounds.unwrap_or(2)
    }
}

/// The batch file candidate `c` plays: `orig` (read from `orig_dir`) with
/// its paths made absolute, its results here (`out = "."`), and one arm,
/// the candidate: arm A's tuning file and inline tuning kept, the
/// candidate's bounce over them, named `c.id`. The mode is the default
/// (every bot plays for the one arm).
pub fn candidate_batch(orig: &BatchConfig, orig_dir: &Path, c: &Candidate) -> BatchConfig {
    let abs = |p: &Path| {
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            orig_dir.join(p)
        }
    };
    let mut tuning = orig.a.tuning.clone().unwrap_or_default();
    tuning.bounce = Some(match &tuning.bounce {
        Some(b) => b.overlay(&c.bounce),
        None => c.bounce.clone(),
    });
    BatchConfig {
        out: PathBuf::from("."),
        match_file: abs(&orig.match_file),
        mode: None,
        b: None,
        a: Arm {
            name: Some(c.id.clone()),
            params: orig.a.params.as_deref().map(abs),
            tuning: Some(tuning),
        },
        ..orig.clone()
    }
}

/// A candidate's result: what its batch's bots did, pooled.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Scored {
    pub id: String,
    pub key: String,
    pub bounce: BounceParams,
    /// Runs with a bounce block.
    pub runs: usize,
    pub stats: Stats,
    /// Pooled per map.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub by_map: BTreeMap<String, Stats>,
    /// 1 the best; `None`: too few tries to rank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rank: Option<usize>,
}

/// Orders `scored` best first and numbers the ranks: candidates with at
/// least `min_attempts` tries first, by bounce rate, then mean launch
/// height, then tries (more is surer), then id; the rest after, unranked.
pub fn rank(scored: &mut [Scored], min_attempts: u64) {
    let launch = |s: &Scored| s.stats.launch.map_or(0.0, |l| l.mean);
    scored.sort_by(|a, b| {
        let ea = a.stats.attempts >= min_attempts;
        let eb = b.stats.attempts >= min_attempts;
        eb.cmp(&ea)
            .then(b.stats.bounce_rate.total_cmp(&a.stats.bounce_rate))
            .then(launch(b).total_cmp(&launch(a)))
            .then(b.stats.attempts.cmp(&a.stats.attempts))
            .then(a.id.cmp(&b.id))
    });
    let mut n = 0;
    for s in scored.iter_mut() {
        s.rank = if s.stats.attempts >= min_attempts {
            n += 1;
            Some(n)
        } else {
            None
        };
    }
}

/// Reads a batch file as written, its paths unresolved: what
/// [`candidate_batch`] takes.
pub fn read_batch_config(path: &Path) -> Result<BatchConfig, String> {
    crate::config::parse_file(path)
}

/// Candidate `c`'s result from its batch's comparison: its arm's bounce
/// block, or `None` when no run gave one.
pub fn score(c: &Candidate, cmp: &crate::compare::Comparison) -> Option<Scored> {
    let arm = cmp.bounce.get(&c.id)?;
    Some(Scored {
        id: c.id.clone(),
        key: c.key.clone(),
        bounce: c.bounce.clone(),
        runs: arm.runs,
        stats: arm.totals.clone(),
        by_map: arm.by_map.clone(),
        rank: None,
    })
}

/// The best ranked of `scored` ([`rank`]'s order), if any has enough
/// tries: where a coordinate sweep goes on from.
pub fn leader(scored: &[Scored], min_attempts: u64) -> Option<Scored> {
    let mut all = scored.to_vec();
    rank(&mut all, min_attempts);
    all.into_iter().next().filter(|s| s.rank.is_some())
}

/// `cands` without the tunings `played` already has, numbered again from
/// `first`: a coordinate sweep's later rounds play each tuning once.
pub fn fresh(cands: Vec<Candidate>, played: &[Candidate], first: usize) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    for mut c in cands {
        if played
            .iter()
            .chain(out.iter())
            .any(|p| same_play(&p.bounce, &c.bounce))
        {
            continue;
        }
        c.id = format!("c{:03}", first + out.len());
        out.push(c);
    }
    out
}

/// `sweep.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SweepResult {
    pub format: String,
    pub method: Method,
    pub min_attempts: u64,
    /// The best ranked candidate's id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub best: Option<String>,
    /// Ranked, best first.
    pub candidates: Vec<Scored>,
    /// Candidates whose batch gave no bounce block.
    #[serde(default)]
    pub missing: Vec<String>,
}

impl SweepResult {
    pub fn new(
        method: Method,
        min_attempts: u64,
        mut scored: Vec<Scored>,
        missing: Vec<String>,
    ) -> Self {
        rank(&mut scored, min_attempts);
        SweepResult {
            format: FORMAT.into(),
            method,
            min_attempts,
            best: scored
                .first()
                .filter(|s| s.rank.is_some())
                .map(|s| s.id.clone()),
            candidates: scored,
            missing,
        }
    }
}

/// `sweep.csv`: one row a candidate, ranked, with every bounce field
/// (empty where the built-in value plays).
pub fn csv(r: &SweepResult) -> String {
    let mut out = String::from(
        "rank,id,attempts,bounced_tries,bounce_rate,primes,chain_bounces,launch_mean,launch_best,bounced_deaths,runs",
    );
    for f in crate::config::BOUNCE_FIELDS {
        out.push(',');
        out.push_str(f.name);
    }
    out.push('\n');
    for s in &r.candidates {
        let (lm, lb) = s.stats.launch.map_or((String::new(), String::new()), |l| {
            (format!("{:.3}", l.mean), format!("{:.3}", l.best))
        });
        let _ = write!(
            out,
            "{},{},{},{},{:.4},{},{},{},{},{},{}",
            s.rank.map_or(String::new(), |r| r.to_string()),
            s.id,
            s.stats.attempts,
            s.stats.bounced_tries,
            s.stats.bounce_rate,
            s.stats.primes,
            s.stats.chain_bounces,
            lm,
            lb,
            s.stats.bounced_deaths,
            s.runs
        );
        for f in crate::config::BOUNCE_FIELDS {
            out.push(',');
            if let Some(v) = s.bounce.get(f.name) {
                out.push_str(&trim(v));
            }
        }
        out.push('\n');
    }
    out
}

/// `sweep.md`: the `top` best as a table, then the best's `[bounce]`
/// block to paste.
pub fn markdown(r: &SweepResult, top: usize) -> String {
    let mut out = String::new();
    let _ = writeln!(
        out,
        "# Bounce tuning sweep ({}, {} candidates)\n",
        r.method.name(),
        r.candidates.len()
    );
    out.push_str("| rank | id | tries | rate | launch mean | launch best | chain bounces | Bounced | tuning |\n");
    out.push_str("|---:|---|---:|---:|---:|---:|---:|---:|---|\n");
    for s in r.candidates.iter().take(top) {
        let (lm, lb) = s.stats.launch.map_or(("-".into(), "-".into()), |l| {
            (format!("{:.2}", l.mean), format!("{:.2}", l.best))
        });
        let _ = writeln!(
            out,
            "| {} | {} | {} | {:.3} | {} | {} | {} | {} | {} |",
            s.rank.map_or("-".to_string(), |r| r.to_string()),
            s.id,
            s.stats.attempts,
            s.stats.bounce_rate,
            lm,
            lb,
            s.stats.chain_bounces,
            s.stats.bounced_deaths,
            if s.key.is_empty() { "built-in" } else { &s.key }
        );
    }
    if !r.missing.is_empty() {
        let _ = writeln!(out, "\nNo result: {}", r.missing.join(", "));
    }
    if let Some(best) = r.candidates.first().filter(|s| s.rank.is_some()) {
        let _ = writeln!(
            out,
            "\nBest: {} ({}).\n\n```toml\n[bounce]",
            best.id, best.key
        );
        for f in crate::config::BOUNCE_FIELDS {
            if let Some(v) = best.bounce.get(f.name) {
                let _ = writeln!(out, "{} = {}", f.name, trim(v));
            }
        }
        out.push_str("```\n");
    } else {
        let _ = writeln!(
            out,
            "\nNo candidate had {} tries: nothing is ranked.",
            r.min_attempts
        );
    }
    out
}

#[cfg(test)]
mod tests;
