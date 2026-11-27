//! Training the bounce sites (docs/game/bounce-search.md, "Scheduling a
//! batch" and "The CLI and the sim"; docs/runtime/sim.md, "Training the
//! bounce sites"): `h2 sim train` plays one run a map and shard, each on
//! its own copy of the map's `.learned`, and reports what the sites
//! learned.
//!
//! Pure: a match's `[train]` table ([`TrainConfig`]) and the shard rule
//! ([`owns`]), the train file and its planned runs ([`load`], [`plan`]),
//! the `.learned` lines as report rows ([`parse_rows`]), the `--reopen`
//! edit ([`reopen`]), and the report's JSON and markdown ([`report`],
//! [`markdown`]). The CLI runs the processes and reads the files.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::{
    Arm, Batch, BatchConfig, BotParams, MatchConfig, Run, plan as plan_batch, safe,
};

/// The report's schema tag.
pub const FORMAT: &str = "h2-sim-train/1";
/// Scored tries a site, when the train file does not say.
pub const DEFAULT_BUDGET: u32 = 120;
/// The most runs a map is split into.
pub const MAX_SHARDS: u32 = 64;

fn one() -> u32 {
    1
}

fn default_budget() -> u32 {
    DEFAULT_BUDGET
}

/// A match's `[train]` table: this run is one shard of a training batch
/// (`[rules] script = "superbounce"` is required, as a `[drill]` requires it).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainConfig {
    /// Which shard this run owns, `0..shards`.
    #[serde(default)]
    pub shard: u32,
    /// How many runs the map is split into.
    #[serde(default = "one")]
    pub shards: u32,
    /// Scored tries a site, at most.
    #[serde(default = "default_budget")]
    pub budget: u32,
    /// The batch seed the search's draws hash (`learn::draw`).
    #[serde(default)]
    pub seed: u64,
}

impl Default for TrainConfig {
    fn default() -> TrainConfig {
        TrainConfig {
            shard: 0,
            shards: 1,
            budget: DEFAULT_BUDGET,
            seed: 0,
        }
    }
}

impl TrainConfig {
    /// What is wrong with a `[train]` table, in a match with or without
    /// `superbounce` and a `[drill]`.
    pub fn problems(&self, superbounce: bool, drill: bool) -> Vec<String> {
        let mut p = Vec::new();
        if !superbounce {
            p.push("train: needs [rules] script = \"superbounce\"".to_string());
        }
        if drill {
            p.push("train: a drill and a training run are two ways to fly a site; use one".into());
        }
        if self.shards == 0 || self.shards > MAX_SHARDS {
            p.push(format!("train: shards is 1 to {MAX_SHARDS}"));
        } else if self.shard >= self.shards {
            p.push(format!(
                "train: shard {} is not under shards {}",
                self.shard, self.shards
            ));
        }
        if self.budget == 0 {
            p.push("train: budget is at least 1 try".into());
        }
        p
    }
}

/// Whether the run of `shard` (of `shards`) owns the site with this spot
/// id (`learn::spot_id`): the one rule that splits a map between runs, so
/// no two runs fly a site.
pub fn owns(spot_id: u64, shard: u32, shards: u32) -> bool {
    shards != 0 && spot_id % u64::from(shards) == u64::from(shard)
}

/// A train file (`h2 sim train <file>`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TrainFile {
    /// The results directory, relative to the file.
    pub out: PathBuf,
    /// The match every run plays, relative to the file: `superbounce =
    /// true` and the bots.
    #[serde(rename = "match")]
    pub match_file: PathBuf,
    /// One set of runs per map (default: the match's).
    #[serde(default)]
    pub maps: Vec<String>,
    /// The batch seed: the match's seed and the search's draws.
    #[serde(default)]
    pub seed: u64,
    /// Runs a map (default 1). Fixed here, not by `--parallel`, so the
    /// result does not depend on how many run at once.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shards: Option<u32>,
    /// Scored tries a site (default [`DEFAULT_BUDGET`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub budget: Option<u32>,
}

/// What the command line puts over the file.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Overrides {
    /// `--map`: these maps instead of the file's.
    pub maps: Vec<String>,
    /// `--budget`.
    pub budget: Option<u32>,
}

/// One planned run: a map's shard.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TrainRun {
    /// The run's directory name under `<out>/runs/`.
    pub id: String,
    pub map: String,
    pub shard: u32,
}

/// A train file read and planned.
#[derive(Clone, Debug, PartialEq)]
pub struct Plan {
    pub file: TrainFile,
    /// The runs as a batch plays them: each one's match has its `[train]`.
    pub batch: Batch,
    /// Each run's map and shard, in the batch's order.
    pub runs: Vec<TrainRun>,
    pub shards: u32,
    pub budget: u32,
    pub seed: u64,
}

impl Plan {
    /// The distinct maps, in the order they run.
    pub fn maps(&self) -> Vec<String> {
        let mut m: Vec<String> = Vec::new();
        for r in &self.runs {
            if !m.contains(&r.map) {
                m.push(r.map.clone());
            }
        }
        m
    }
}

/// Reads a train file and its match file and plans the runs.
pub fn load(path: &Path, over: &Overrides) -> Result<Plan, String> {
    let file: TrainFile = crate::config::parse_file(path)?;
    let base = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let match_path = base.join(&file.match_file);
    let base_match: MatchConfig = crate::config::parse_file(&match_path)?;
    let match_dir = match_path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let read_bot = |p: &Path| BotParams::load(&match_dir.join(p));
    plan(file, base_match, &read_bot, &base, over)
}

/// Plans the runs: a run for each shard of each map, each with the match
/// file's bots (tunings inline) and its own `[train]` table. Every
/// problem is listed.
pub fn plan(
    file: TrainFile,
    base_match: MatchConfig,
    read_bot: &dyn Fn(&Path) -> Result<BotParams, String>,
    base: &Path,
    over: &Overrides,
) -> Result<Plan, String> {
    let mut problems = Vec::new();
    let shards = file.shards.unwrap_or(1);
    let budget = over.budget.or(file.budget).unwrap_or(DEFAULT_BUDGET);
    let template = TrainConfig {
        shard: 0,
        shards,
        budget,
        seed: file.seed,
    };
    let drill = base_match.drill.is_some();
    let superbounce = base_match.plays_superbounce();
    problems.extend(template.problems(superbounce, drill));
    if !over.maps.is_empty() && over.maps.iter().any(|m| m.trim().is_empty()) {
        problems.push("--map: empty name".into());
    }
    let maps = if over.maps.is_empty() {
        file.maps.clone()
    } else {
        over.maps.clone()
    };
    let config = BatchConfig {
        out: file.out.clone(),
        parallel: None,
        match_file: file.match_file.clone(),
        maps,
        seeds: vec![file.seed],
        a: Arm::default(),
        ..BatchConfig::default()
    };
    let no_arm = |_: &Path| Ok(BotParams::default());
    let batch = match plan_batch(config, base_match, &no_arm, read_bot, base) {
        Ok(b) => Some(b),
        Err(e) => {
            problems.push(e);
            None
        }
    };
    if !problems.is_empty() {
        problems.dedup();
        return Err(problems.join("\n"));
    }
    let Some(mut batch) = batch else {
        return Err("no runs".into());
    };
    let mut runs = Vec::new();
    let mut planned: Vec<Run> = Vec::new();
    for r in std::mem::take(&mut batch.runs) {
        for shard in 0..shards {
            let id = format!("{}-shard{shard}", safe(&r.map));
            let mut config = r.config.clone();
            config.train = Some(TrainConfig { shard, ..template });
            planned.push(Run {
                id: id.clone(),
                map: r.map.clone(),
                seed: r.seed,
                config,
            });
            runs.push(TrainRun {
                id,
                map: r.map.clone(),
                shard,
            });
        }
    }
    batch.runs = planned;
    Ok(Plan {
        file,
        batch,
        runs,
        shards,
        budget,
        seed: template.seed,
    })
}

// ---- the file's lines as rows ----

/// The states a `.learned` line has, in the order the report lists them.
pub const STATES: [&str; 5] = ["learned", "partial", "open", "dead", "blocked"];

/// The knob keys (docs/game/bounce-search.md, "The knobs"), `how` first.
pub const KNOBS: [&str; 14] = [
    "how", "fx", "fy", "fz", "ax", "ay", "yaw", "back", "run", "early", "gain", "letgo", "drift",
    "still",
];

/// One site's `.learned` line, as the report shows it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SiteRow {
    /// The site's `stand`, as the scan gave it.
    pub stand: [f32; 3],
    pub state: String,
    /// The `how` knob: `runoff`, `jump`, `stand`.
    pub how: Option<String>,
    /// Scored tries, and of them the ones that bounced.
    pub tries: u32,
    pub hits: u32,
    /// Tries at the centre (the best knobs), and of them the bounces.
    pub n: u32,
    pub ok: u32,
    /// The mean score at the centre.
    pub score: f64,
    /// The best launch, units/s up; 0 when none.
    pub best: f64,
    pub deaths: u32,
    pub why: Option<String>,
    /// The numeric knobs the line sets, by key.
    pub knobs: BTreeMap<String, f64>,
    /// The search's cursor: `stage`, `axis`, `shrink`, `moved`,
    /// `scattered`.
    pub cursor: BTreeMap<String, String>,
}

impl SiteRow {
    /// The bounce rate at the best knobs: `ok / n`.
    pub fn rate(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            f64::from(self.ok) / f64::from(self.n)
        }
    }
}

/// The header a `.learned` file starts with.
pub const LEARNED_HEADER: &str = "# superbounce learned v1";

/// Reads the report rows from a `.learned` file's text. Lenient where the
/// game's reader is strict (an unknown key is kept in no row, not
/// refused): this is a report, not the file's checker. A line it cannot
/// read is an error naming it.
pub fn parse_rows(text: &str) -> Result<Vec<SiteRow>, String> {
    let mut rows = Vec::new();
    for (i, line) in text.lines().enumerate() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') {
            continue;
        }
        let at = |why: &str| format!("line {}: {why}", i + 1);
        let mut it = t.split_whitespace();
        if it.next() != Some("bounce") {
            return Err(at("not a bounce line"));
        }
        let state = it.next().ok_or_else(|| at("no state"))?;
        if !STATES.contains(&state) {
            return Err(at(&format!("unknown state {state:?}")));
        }
        let mut stand = [0f32; 3];
        for c in &mut stand {
            *c = it
                .next()
                .and_then(|s| s.parse().ok())
                .ok_or_else(|| at("no stand x y z"))?;
        }
        let mut row = SiteRow {
            stand,
            state: state.to_string(),
            ..SiteRow::default()
        };
        for kv in it {
            let (k, v) = kv
                .split_once('=')
                .ok_or_else(|| at(&format!("{kv:?}: not key=value")))?;
            let num = |v: &str| {
                v.parse::<f64>()
                    .map_err(|_| at(&format!("{k}={v}: not a number")))
            };
            let count = |v: &str| {
                v.parse::<u32>()
                    .map_err(|_| at(&format!("{k}={v}: not a count")))
            };
            match k {
                "how" => row.how = Some(v.to_string()),
                "tries" => row.tries = count(v)?,
                "hits" => row.hits = count(v)?,
                "n" => row.n = count(v)?,
                "ok" => row.ok = count(v)?,
                "score" => row.score = num(v)?,
                "best" => row.best = num(v)?,
                "deaths" => row.deaths = count(v)?,
                "why" => row.why = Some(v.to_string()),
                "stage" | "axis" | "shrink" | "moved" | "scattered" => {
                    row.cursor.insert(k.to_string(), v.to_string());
                }
                _ if KNOBS.contains(&k) => {
                    row.knobs.insert(k.to_string(), num(v)?);
                }
                _ => {}
            }
        }
        rows.push(row);
    }
    rows.sort_by(|a, b| {
        a.stand
            .partial_cmp(&b.stand)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(rows)
}

/// The rows of one shard: those `id` (a site's spot id from its map and
/// stand) gives to `shard` of `shards`.
pub fn shard_rows(
    rows: &[SiteRow],
    shard: u32,
    shards: u32,
    id: &dyn Fn([f32; 3]) -> u64,
) -> Vec<SiteRow> {
    rows.iter()
        .filter(|r| owns(id(r.stand), shard, shards))
        .cloned()
        .collect()
}

/// `--reopen`: every `dead` and `blocked` line back to `open`, its `why`
/// dropped and its cursor back at the first stage (a `dead` site's cursor
/// says its search ended). The text and how many lines changed.
pub fn reopen(text: &str) -> (String, usize) {
    let mut out = String::with_capacity(text.len());
    let mut changed = 0;
    for line in text.lines() {
        let mut it = line.split_whitespace();
        let closed =
            it.next() == Some("bounce") && matches!(it.next(), Some("dead") | Some("blocked"));
        if !closed {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        changed += 1;
        let mut fields = line.split_whitespace();
        let kind = fields.next().unwrap_or("bounce");
        fields.next();
        out.push_str(kind);
        out.push_str(" open");
        for f in fields {
            let key = f.split_once('=').map(|(k, _)| k);
            match key {
                Some("why") => {}
                Some("stage") => out.push_str(" stage=arms"),
                Some("axis") => out.push_str(" axis=0"),
                Some("shrink") => out.push_str(" shrink=0"),
                Some("moved") => out.push_str(" moved=0"),
                Some("scattered") => out.push_str(" scattered=0"),
                _ => {
                    out.push(' ');
                    out.push_str(f);
                }
            }
        }
        out.push('\n');
    }
    (out, changed)
}

// ---- the report ----

/// One map's sites and totals.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct MapReport {
    pub map: String,
    pub sites: usize,
    /// Sites by state (only the states that have one).
    pub states: BTreeMap<String, usize>,
    /// Scored tries, summed over the sites.
    pub tries: u64,
    pub rows: Vec<SiteRow>,
}

/// The batch's report: `<out>/train.json`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct TrainReport {
    pub format: String,
    pub maps: Vec<MapReport>,
}

/// The report of `maps`' rows (each map's sites, any order).
pub fn report(maps: Vec<(String, Vec<SiteRow>)>) -> TrainReport {
    let maps = maps
        .into_iter()
        .map(|(map, mut rows)| {
            rows.sort_by(|a, b| {
                a.stand
                    .partial_cmp(&b.stand)
                    .unwrap_or(std::cmp::Ordering::Equal)
            });
            let mut states: BTreeMap<String, usize> = BTreeMap::new();
            for r in &rows {
                *states.entry(r.state.clone()).or_default() += 1;
            }
            MapReport {
                map,
                sites: rows.len(),
                states,
                tries: rows.iter().map(|r| u64::from(r.tries)).sum(),
                rows,
            }
        })
        .collect();
    TrainReport {
        format: FORMAT.into(),
        maps,
    }
}

impl MapReport {
    /// `map ascension: 166 sites: learned 41, partial 17, ...; 9480 tries`.
    pub fn heading(&self) -> String {
        let parts: Vec<String> = STATES
            .iter()
            .filter_map(|s| self.states.get(*s).map(|n| format!("{s} {n}")))
            .collect();
        let states = if parts.is_empty() {
            String::new()
        } else {
            format!(": {}", parts.join(", "))
        };
        format!(
            "map {}: {} sites{states}; {} tries",
            self.map, self.sites, self.tries
        )
    }
}

fn point(s: [f32; 3]) -> String {
    format!("({:.2}, {:.2}, {:.2})", s[0], s[1], s[2])
}

/// The report as `train.md` and `--status` print it: a heading and a
/// table a map.
pub fn markdown(r: &TrainReport) -> String {
    let mut out = String::new();
    for (i, m) in r.maps.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        let _ = writeln!(out, "{}", m.heading());
        let head = [
            "site", "state", "how", "tries", "hits", "n", "ok", "rate", "score", "best", "deaths",
            "why",
        ];
        let mut table: Vec<[String; 12]> = Vec::new();
        for s in &m.rows {
            table.push([
                point(s.stand),
                s.state.clone(),
                s.how.clone().unwrap_or_else(|| "-".into()),
                s.tries.to_string(),
                s.hits.to_string(),
                s.n.to_string(),
                s.ok.to_string(),
                format!("{:.2}", s.rate()),
                format!("{:.3}", s.score),
                if s.best > 0.0 {
                    format!("{:.1}", s.best)
                } else {
                    "-".into()
                },
                s.deaths.to_string(),
                s.why.clone().unwrap_or_default(),
            ]);
        }
        let mut width = head.map(str::len);
        for row in &table {
            for (w, c) in width.iter_mut().zip(row) {
                *w = (*w).max(c.len());
            }
        }
        let line = |cells: Vec<&str>| {
            let mut l = String::new();
            for (k, c) in cells.iter().enumerate() {
                // Text left, numbers right; the last column is not padded.
                if k == 11 {
                    l.push_str(c);
                } else if k < 3 {
                    let _ = write!(l, "{c:<w$} ", w = width[k]);
                } else {
                    let _ = write!(l, "{c:>w$} ", w = width[k]);
                }
            }
            l.trim_end().to_string()
        };
        let _ = writeln!(out, "{}", line(head.to_vec()));
        for row in &table {
            let _ = writeln!(out, "{}", line(row.iter().map(String::as_str).collect()));
        }
    }
    if r.maps.is_empty() {
        out.push_str("no sites\n");
    }
    out
}

#[cfg(test)]
mod tests;
