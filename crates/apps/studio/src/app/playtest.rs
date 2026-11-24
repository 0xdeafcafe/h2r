//! Playtest's runner (docs/editor/playtest.md, "What a run is"): the
//! steps of `playtest.run` behind a [`Source`], the gather of a batch's
//! traces into a run, its partial results, its cache and its stale mark.
//!
//! Pure over the [`Source`]: it builds the open project, makes the bots'
//! mesh, starts a batch of `h2 sim` children and hands back each child's
//! [`Trace`] as it ends. The real source (the build, the nav mesh and the
//! `h2 sim batch` process) is wired where the workspace's build is; the
//! tests drive a fake. The measures are `sim_bench::spatial::measure`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use sim_bench::config::{Arm, BatchConfig, BotSpec, BotThreads, GameType, MatchConfig};
use sim_bench::spatial::compare::{self, Comparison};
use sim_bench::spatial::{self, Measures, TICK_RATE, Trace};

/// Seats in a match when the call names no number.
pub const DEFAULT_BOTS: u32 = 4;
/// Most seeds a call may ask for.
pub const MAX_SEEDS: u32 = 256;
/// Most game minutes a call may ask for.
pub const MAX_MINUTES: u32 = 120;
/// The least and most seats.
pub const BOTS: std::ops::RangeInclusive<u32> = 2..=16;
/// Fewer seeds than this give a spread not worth trusting; the result is
/// marked coarse.
pub const COARSE_BELOW: u32 = 8;

/// What a run plays.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Preset {
    /// 4 seeds of the map's first ready game type, 2 game minutes.
    Probe,
    /// 8 seeds per ready game type, 5 game minutes.
    Standard,
    /// 32 seeds per ready game type, sides swapped, to the score limit.
    Soak,
}

impl Preset {
    pub fn parse(s: &str) -> Result<Preset, String> {
        match s {
            "probe" => Ok(Preset::Probe),
            "standard" => Ok(Preset::Standard),
            "soak" => Ok(Preset::Soak),
            _ => Err(format!("unknown preset {s:?} (probe, standard, soak)")),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Preset::Probe => "probe",
            Preset::Standard => "standard",
            Preset::Soak => "soak",
        }
    }

    pub fn seeds(self) -> u32 {
        match self {
            Preset::Probe => 4,
            Preset::Standard => 8,
            Preset::Soak => 32,
        }
    }

    /// Game minutes; `None` plays to the game's own end.
    pub fn minutes(self) -> Option<u32> {
        match self {
            Preset::Probe => Some(2),
            Preset::Standard => Some(5),
            Preset::Soak => None,
        }
    }

    /// Every seed is played a second time with the bots' sides swapped.
    pub fn swapped(self) -> bool {
        self == Preset::Soak
    }

    /// Every ready game type is played, not the first.
    pub fn every_game_type(self) -> bool {
        self != Preset::Probe
    }
}

/// What a `playtest.run` asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Spec {
    pub preset: Preset,
    /// Game types by name; none: the preset's choice among the ready ones.
    pub game_types: Vec<String>,
    /// Seeds 1 to this; none: the preset's count.
    pub seeds: Option<u32>,
    pub minutes: Option<u32>,
    pub bots: Option<u32>,
    /// The run the result is compared with: a run's id or `baseline`.
    /// Not part of what a run plays, so not of its cache key.
    pub compare_to: Option<String>,
}

impl Default for Spec {
    fn default() -> Self {
        Spec {
            preset: Preset::Probe,
            game_types: Vec::new(),
            seeds: None,
            minutes: None,
            bots: None,
            compare_to: None,
        }
    }
}

impl Spec {
    /// The call's numbers in range.
    pub fn check(&self) -> Result<(), String> {
        if let Some(n) = self.seeds
            && !(1..=MAX_SEEDS).contains(&n)
        {
            return Err(format!("seeds {n}: from 1 to {MAX_SEEDS}"));
        }
        if let Some(n) = self.minutes
            && !(1..=MAX_MINUTES).contains(&n)
        {
            return Err(format!("minutes {n}: from 1 to {MAX_MINUTES}"));
        }
        if let Some(n) = self.bots
            && !BOTS.contains(&n)
        {
            return Err(format!("bots {n}: from {} to {}", BOTS.start(), BOTS.end()));
        }
        Ok(())
    }

    pub fn seed_count(&self) -> u32 {
        self.seeds.unwrap_or_else(|| self.preset.seeds())
    }

    pub fn game_minutes(&self) -> Option<u32> {
        self.minutes.or_else(|| self.preset.minutes())
    }

    pub fn seats(&self) -> u32 {
        self.bots.unwrap_or(DEFAULT_BOTS)
    }
}

/// One match of a batch.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct JobId {
    /// The sim's name for it (`slayer`, `ctf`).
    pub game_type: String,
    pub seed: u64,
    /// The second leg of a swapped pair.
    pub swapped: bool,
}

/// The build of the open project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Built {
    /// Names the map: the same key is the same map.
    pub key: String,
    /// The map's name in the match file.
    pub map: String,
}

/// What else a result depends on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Versions {
    pub bots: String,
    pub image: String,
}

/// The files of a batch and its matches, written for the source to run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Batch {
    pub dir: PathBuf,
    pub batch_file: PathBuf,
    pub match_file: PathBuf,
    pub jobs: Vec<JobId>,
    pub parallel: usize,
}

/// A match that ended.
#[derive(Clone, Debug)]
pub struct Finished {
    pub job: JobId,
    /// Wall seconds the match took.
    pub wall_s: f32,
    pub trace: Result<Trace, String>,
}

/// What a run needs of the world: the build, the mesh and the sim.
pub trait Source: Send {
    /// Where runs write their files (`<cache>/playtest`).
    fn dir(&self) -> PathBuf;
    fn versions(&self) -> Versions;
    /// Matches at once.
    fn parallel(&self) -> usize;
    /// Builds the open project.
    fn build(&mut self) -> Result<Built, String>;
    /// Makes the bots' mesh for the build when none is current.
    fn nav(&mut self, build: &str) -> Result<(), String>;
    /// Starts the batch; its matches end through [`Source::poll`].
    fn start(&mut self, batch: &Batch) -> Result<(), String>;
    /// The matches that ended since the last call.
    fn poll(&mut self) -> Vec<Finished>;
    /// Stops the batch.
    fn cancel(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Running,
    Done,
    /// Every match failed.
    Failed,
    Cancelled,
}

impl State {
    pub fn name(self) -> &'static str {
        match self {
            State::Running => "running",
            State::Done => "done",
            State::Failed => "failed",
            State::Cancelled => "cancelled",
        }
    }
}

/// One run: what it played and what came back.
#[derive(Clone, Debug)]
pub struct Run {
    pub id: String,
    pub build: String,
    pub map: String,
    /// What the result depends on, whole: two runs with one key give one
    /// result.
    pub key: String,
    pub preset: Preset,
    pub dir: PathBuf,
    pub game_types: Vec<String>,
    pub seeds: Vec<u64>,
    pub swapped: bool,
    pub minutes: Option<u32>,
    pub bots: u32,
    pub parallel: usize,
    pub jobs: Vec<JobId>,
    pub state: State,
    /// A newer build exists than the one this run played.
    pub stale: bool,
    /// What it is compared with by default: a run's id or `baseline`.
    pub compare_to: Option<String>,
    /// The static checks' problems when it started, as `kind: text`.
    pub statics: Vec<String>,
    traces: Arc<Vec<(JobId, Trace)>>,
    failed: Vec<(JobId, String)>,
    started_s: f64,
    last_s: f64,
}

impl Run {
    /// Matches that ended, well or not.
    pub fn done(&self) -> usize {
        self.traces.len() + self.failed.len()
    }

    pub fn total(&self) -> usize {
        self.jobs.len()
    }

    pub fn failed(&self) -> &[(JobId, String)] {
        &self.failed
    }

    /// Fewer seeds than [`COARSE_BELOW`]: the spread is not worth trusting.
    pub fn coarse(&self) -> bool {
        (self.seeds.len() as u32) < COARSE_BELOW
    }

    /// Game seconds played by the matches that ended.
    pub fn game_seconds(&self) -> f32 {
        self.traces.iter().map(|(_, t)| t.header.seconds()).sum()
    }

    /// Game seconds per wall second since the run started.
    pub fn speedup(&self) -> Option<f64> {
        let wall = self.last_s - self.started_s;
        (wall > 0.0 && !self.traces.is_empty()).then(|| f64::from(self.game_seconds()) / wall)
    }

    /// The game types with a trace, in the run's order.
    pub fn played(&self) -> Vec<&str> {
        self.game_types
            .iter()
            .map(String::as_str)
            .filter(|g| self.traces.iter().any(|(j, _)| j.game_type == *g))
            .collect()
    }

    /// The measures of one game type's traces so far.
    pub fn measures(&self, game_type: &str) -> Option<Measures> {
        let traces = self.traces_of(game_type);
        (!traces.is_empty()).then(|| spatial::measure(&traces))
    }

    /// One game type's traces so far, every leg of every seed.
    pub fn traces_of(&self, game_type: &str) -> Vec<Trace> {
        self.traces
            .iter()
            .filter(|(j, _)| j.game_type == game_type)
            .map(|(_, t)| t.clone())
            .collect()
    }
}

/// What `playtest.run` answers at once.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Started {
    pub run: String,
    pub build: String,
    pub matches: usize,
    pub parallel: usize,
    /// An earlier run with the same key answered; nothing was started.
    pub cached: bool,
}

/// The runs of a session.
#[derive(Default)]
pub struct Runner {
    runs: Vec<Run>,
    /// Key to the index of a finished run with no failed match.
    cache: BTreeMap<String, usize>,
}

impl Runner {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every run of the session, oldest first.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }

    /// The run named `id`, or the newest.
    pub fn run(&self, id: Option<&str>) -> Result<Option<&Run>, String> {
        match id {
            None => Ok(self.runs.last()),
            Some(id) => self
                .runs
                .iter()
                .find(|r| r.id == id)
                .map(Some)
                .ok_or_else(|| format!("no run {id}")),
        }
    }

    fn active(&self) -> Option<usize> {
        self.runs.iter().position(|r| r.state == State::Running)
    }

    /// Every run of another build than `build` is stale.
    pub fn mark_stale(&mut self, build: &str) {
        for r in &mut self.runs {
            r.stale = r.build != build;
        }
    }

    /// Plays `spec` over the `ready` game types (by name, as the static
    /// checks give them): builds, makes the mesh, writes the match and
    /// batch files and starts the batch. An earlier run with the same
    /// build, match, seeds and versions answers instead; a run still
    /// going is cancelled by a new one.
    pub fn start(
        &mut self,
        source: &mut dyn Source,
        spec: &Spec,
        ready: &[String],
        now_s: f64,
    ) -> Result<Started, String> {
        spec.check()?;
        if let Some(r) = spec.compare_to.as_deref().filter(|r| *r != "baseline") {
            self.reference(r)?;
        }
        let game_types = choose(spec, ready)?;
        let built = source.build().map_err(|e| format!("build: {e}"))?;
        self.mark_stale(&built.key);
        let versions = source.versions();
        let seeds: Vec<u64> = (1..=u64::from(spec.seed_count())).collect();
        let swapped = spec.preset.swapped();
        let key = format!(
            "build {}; map {}; image {}; bots {}; {}; seeds {}; swapped {}; minutes {:?}; seats {}",
            built.key,
            built.map,
            versions.image,
            versions.bots,
            game_types.join(","),
            seeds.len(),
            swapped,
            spec.game_minutes(),
            spec.seats(),
        );
        if let Some(&i) = self.cache.get(&key) {
            let r = &self.runs[i];
            return Ok(Started {
                run: r.id.clone(),
                build: r.build.clone(),
                matches: r.total(),
                parallel: r.parallel,
                cached: true,
            });
        }
        if self.active().is_some() {
            self.cancel(source, None)?;
        }
        source
            .nav(&built.key)
            .map_err(|e| format!("the bots' mesh: {e}"))?;
        let mut jobs = Vec::new();
        for g in &game_types {
            for &seed in &seeds {
                jobs.push(JobId {
                    game_type: g.clone(),
                    seed,
                    swapped: false,
                });
                if swapped {
                    jobs.push(JobId {
                        game_type: g.clone(),
                        seed,
                        swapped: true,
                    });
                }
            }
        }
        let id = format!("r{}", self.runs.len() + 1);
        let dir = source.dir().join(&id);
        let parallel = source.parallel().clamp(1, jobs.len());
        let batch = write_files(
            &dir,
            &built.map,
            spec,
            &game_types,
            &seeds,
            parallel,
            jobs.clone(),
        )?;
        source.start(&batch).map_err(|e| format!("the sim: {e}"))?;
        let started = Started {
            run: id.clone(),
            build: built.key.clone(),
            matches: jobs.len(),
            parallel,
            cached: false,
        };
        self.runs.push(Run {
            id,
            build: built.key,
            map: built.map,
            key,
            preset: spec.preset,
            dir,
            game_types,
            seeds,
            swapped,
            minutes: spec.game_minutes(),
            bots: spec.seats(),
            parallel,
            jobs,
            state: State::Running,
            stale: false,
            compare_to: spec.compare_to.clone(),
            statics: Vec::new(),
            traces: Arc::new(Vec::new()),
            failed: Vec::new(),
            started_s: now_s,
            last_s: now_s,
        });
        Ok(started)
    }

    /// Folds the matches that ended into the running run.
    pub fn pump(&mut self, source: &mut dyn Source, now_s: f64) {
        let finished = source.poll();
        let Some(i) = self.active() else {
            return;
        };
        let run = &mut self.runs[i];
        run.last_s = now_s;
        for f in finished {
            let known = run.jobs.contains(&f.job);
            let seen = run.traces.iter().any(|(j, _)| *j == f.job)
                || run.failed.iter().any(|(j, _)| *j == f.job);
            if !known || seen {
                continue;
            }
            match f.trace {
                Ok(t) => Arc::make_mut(&mut run.traces).push((f.job, t)),
                Err(why) => run.failed.push((f.job, why)),
            }
        }
        if run.done() >= run.total() {
            run.state = if run.traces.is_empty() {
                State::Failed
            } else {
                State::Done
            };
            if run.state == State::Done && run.failed.is_empty() {
                self.cache.insert(run.key.clone(), i);
            }
        }
    }

    /// Stops the run named `id`, or the running one. True when one was
    /// running. What came back stays.
    pub fn cancel(&mut self, source: &mut dyn Source, id: Option<&str>) -> Result<bool, String> {
        let i = match id {
            Some(id) => self
                .runs
                .iter()
                .position(|r| r.id == id)
                .ok_or_else(|| format!("no run {id}"))?,
            None => match self.active() {
                Some(i) => i,
                None => return Ok(false),
            },
        };
        if self.runs[i].state != State::Running {
            return Ok(false);
        }
        source.cancel();
        self.runs[i].state = State::Cancelled;
        Ok(true)
    }
}

impl Runner {
    /// The run kept as the baseline: the first of the session with a match
    /// that ended.
    pub fn baseline(&self) -> Option<&Run> {
        self.runs.iter().find(|r| !r.traces.is_empty())
    }

    /// A run named `id`, or `baseline`.
    pub fn reference(&self, id: &str) -> Result<&Run, String> {
        if id == "baseline" {
            return self
                .baseline()
                .ok_or_else(|| "no baseline yet: no run has a match that ended".to_owned());
        }
        self.runs
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| format!("no run {id}"))
    }

    /// Records the static checks' problems the run started under, for
    /// [`Runner::compare`].
    pub fn note_static(&mut self, id: &str, problems: Vec<String>) {
        if let Some(r) = self.runs.iter_mut().find(|r| r.id == id) {
            r.statics = problems;
        }
    }

    /// Compares run `a` with run `b` (ids or `baseline`) over `game_type`
    /// (the first both played by default).
    pub fn compare(&self, a: &str, b: &str, game_type: Option<&str>) -> Result<Compared, String> {
        compare_runs(self.reference(a)?, self.reference(b)?, game_type)
    }
}

/// Two runs compared.
#[derive(Clone, Debug)]
pub struct Compared {
    pub a: String,
    pub b: String,
    pub game_type: String,
    pub comparison: Comparison,
    /// Problems the static checks had at `b` and not at `a`, and the
    /// reverse.
    pub gained: Vec<String>,
    pub lost: Vec<String>,
}

/// Compares `a` with `b` over the same seeds and match.
pub fn compare_runs(a: &Run, b: &Run, game_type: Option<&str>) -> Result<Compared, String> {
    if a.seeds != b.seeds || a.minutes != b.minutes || a.bots != b.bots || a.swapped != b.swapped {
        return Err(format!(
            "{} and {} did not play the same seeds and match ({} seeds, {} seeds)",
            a.id,
            b.id,
            a.seeds.len(),
            b.seeds.len()
        ));
    }
    let played = |r: &Run| {
        r.played()
            .into_iter()
            .map(str::to_owned)
            .collect::<Vec<_>>()
    };
    let (pa, pb) = (played(a), played(b));
    let game_type = match game_type {
        Some(g) => g.to_owned(),
        None => pa.iter().find(|g| pb.contains(g)).cloned().ok_or_else(|| {
            format!(
                "{} and {} played no game type with a result in common",
                a.id, b.id
            )
        })?,
    };
    let (ta, tb) = (a.traces_of(&game_type), b.traces_of(&game_type));
    if ta.is_empty() || tb.is_empty() {
        return Err(format!(
            "{game_type}: {} has {} matches that ended, {} has {}",
            a.id,
            ta.len(),
            b.id,
            tb.len()
        ));
    }
    let comparison = compare::compare(&ta, &tb)?;
    let gone = |x: &[String], y: &[String]| -> Vec<String> {
        x.iter().filter(|p| !y.contains(p)).cloned().collect()
    };
    Ok(Compared {
        a: a.id.clone(),
        b: b.id.clone(),
        game_type,
        comparison,
        gained: gone(&b.statics, &a.statics),
        lost: gone(&a.statics, &b.statics),
    })
}

/// The sim's names of the game types a run plays.
fn choose(spec: &Spec, ready: &[String]) -> Result<Vec<String>, String> {
    let ready: Vec<GameType> = ready
        .iter()
        .filter_map(|n| GameType::parse(n).ok())
        .collect();
    if ready.is_empty() {
        return Err("no game type is ready (playtest.static says what each lacks)".into());
    }
    if spec.game_types.is_empty() {
        let first = ready[0].name().to_owned();
        return Ok(if spec.preset.every_game_type() {
            ready.iter().map(|g| g.name().to_owned()).collect()
        } else {
            vec![first]
        });
    }
    let mut out: Vec<String> = Vec::new();
    for n in &spec.game_types {
        let g = GameType::parse(n)?;
        if !ready.contains(&g) {
            return Err(format!("{} is not ready for this map", g.name()));
        }
        if !out.iter().any(|o| o == g.name()) {
            out.push(g.name().to_owned());
        }
    }
    Ok(out)
}

/// `match.json` and `batch.json` under `dir`.
fn write_files(
    dir: &std::path::Path,
    map: &str,
    spec: &Spec,
    game_types: &[String],
    seeds: &[u64],
    parallel: usize,
    jobs: Vec<JobId>,
) -> Result<Batch, String> {
    let swapped = spec.preset.swapped();
    let m = MatchConfig {
        map: map.to_owned(),
        game_type: game_types[0].clone(),
        seed: 1,
        max_ticks: spec
            .game_minutes()
            .map(|m| (f64::from(m) * 60.0 * f64::from(TICK_RATE)) as u64),
        detail: Some("spatial".into()),
        bot_threads: Some(BotThreads::Word("off".into())),
        bots: (0..spec.seats()).map(|_| BotSpec::default()).collect(),
        ..MatchConfig::default()
    };
    let named = |n: &str| Arm {
        name: Some(n.into()),
        ..Arm::default()
    };
    let b = BatchConfig {
        out: "results".into(),
        parallel: Some(parallel),
        match_file: "match.json".into(),
        game_types: game_types.to_vec(),
        seeds: seeds.to_vec(),
        mode: swapped.then(|| "arm_swap".to_owned()),
        detail: Some("spatial".into()),
        a: named("a"),
        b: swapped.then(|| named("b")),
        ..BatchConfig::default()
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let match_file = dir.join("match.json");
    let batch_file = dir.join("batch.json");
    let text = |v: Result<String, serde_json::Error>| v.map_err(|e| e.to_string());
    std::fs::write(&match_file, text(serde_json::to_string_pretty(&m))?)
        .map_err(|e| format!("{}: {e}", match_file.display()))?;
    std::fs::write(&batch_file, text(serde_json::to_string_pretty(&b))?)
        .map_err(|e| format!("{}: {e}", batch_file.display()))?;
    Ok(Batch {
        dir: dir.to_owned(),
        batch_file,
        match_file,
        jobs,
        parallel,
    })
}

/// One measure's answer: its value, or why it was refused.
macro_rules! part {
    ($r:expr) => {
        match &$r {
            Ok(v) => json!({"ok": true, "value": serde_json::to_value(v).unwrap_or(Value::Null)}),
            Err(e) => {
                json!({"ok": false, "refused": serde_json::to_value(e).unwrap_or(Value::Null)})
            }
        }
    };
}

/// The measures' names, as `playtest.result` takes them.
pub const MEASURES: [&str; 6] = ["heat", "spawns", "flow", "sight", "items", "stuck"];

/// One measure in full, or `None` for a name that is none.
pub fn measure_json(m: &Measures, name: &str) -> Option<Value> {
    Some(match name {
        "heat" => part!(m.heat),
        "spawns" => part!(m.spawns),
        "flow" => part!(m.flow),
        "sight" => part!(m.sight),
        "items" => part!(m.items),
        "stuck" => part!(m.stuck),
        _ => return None,
    })
}

/// Every measure as `{ok}` or `{ok: false, refused}`, with no bins.
pub fn summary_json(m: &Measures) -> Value {
    let mut o = serde_json::Map::new();
    for n in MEASURES {
        let Some(mut v) = measure_json(m, n) else {
            continue;
        };
        if let Some(x) = v.as_object_mut() {
            x.remove("value");
        }
        o.insert(n.to_owned(), v);
    }
    Value::Object(o)
}

/// `playtest.compare`'s answer.
pub fn compared_json(c: &Compared) -> Value {
    json!({
        "a": c.a,
        "b": c.b,
        "game_type": c.game_type,
        "measures": c.comparison.scalars,
        "changed": c.comparison.changed(),
        "traces": {"a": c.comparison.diff.a_traces, "b": c.comparison.diff.b_traces},
        "layers": layers_json(&c.comparison.diff),
        "static": {"gained": c.gained, "lost": c.lost},
    })
}

/// Each difference layer: its marks' count and the biggest, or why it has
/// none.
fn layers_json(d: &compare::Diff) -> Value {
    let one = |m: &compare::Marks| match m {
        Ok(v) => json!({"marks": v.len(), "top": v.iter().take(5).collect::<Vec<_>>()}),
        Err(why) => json!({"marks": 0, "refused": why}),
    };
    json!({
        "kills": one(&d.kills),
        "deaths": one(&d.deaths),
        "traffic": one(&d.traffic),
        "flow": one(&d.flow),
        "unvisited": one(&d.unvisited),
        "spawns": one(&d.spawns),
        "weapon_use": one(&d.weapon_use),
        "stuck": one(&d.stuck),
    })
}

#[cfg(test)]
pub mod fake;
#[cfg(test)]
mod tests;
