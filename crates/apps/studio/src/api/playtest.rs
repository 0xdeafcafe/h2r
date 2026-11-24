//! `playtest.static`: the open map's static checks
//! ([`crate::app::playtest_static`]; docs/editor/playtest.md, "Static
//! checks"). Read only. It starts the bots' mesh load when nothing has
//! (`nav.bots`'s load), and `wait` blocks until it is done.
//!
//! `playtest.run`, `.status`, `.result` and `.cancel` are the runner's
//! ([`crate::app::playtest`]; playtest.md, "What a run is"). The runner
//! plays through a [`Source`] (the build, the bots' mesh and `h2 sim`);
//! with none connected `playtest.run` is refused. Compare, the overlays
//! and the pool come with PT5 to PT7.

use std::time::Instant;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::params::block_name;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::playtest::{self as run, Preset, Run, Runner, Source, Spec};
use crate::app::playtest_static::{Report, Target};

const RESULT: &str = r#"{"ok":"boolean","nav":"no_map|idle|loading|ready|failed","problems":"[{kind,text,target}]","ready":"[{game_type,engine,ok,missing}]"}"#;
const RUN_PARAMS: &str = r#"{"preset?":"probe|standard|soak","game_types?":"[string]","seeds?":"number","minutes?":"number","bots?":"number"}"#;
const RUN_RESULT: &str = r#"{"run":"string","build":"string","matches":"number","parallel":"number","cached":"boolean"}"#;
const STATUS_RESULT: &str = r#"{"run":"string|null","state":"none|running|done|failed|cancelled","preset":"string","build":"string","done":"number","total":"number","speedup":"number|null","stale":"boolean","coarse":"boolean","failed":"[{game_type,seed,swapped,why}]"}"#;
const RESULT_PARAMS: &str =
    r#"{"run?":"string","game_type?":"string","measure?":"heat|spawns|flow|sight|items|stuck"}"#;
const RESULT_RESULT: &str = r#"{"run":"string|null","state":"string","stale":"boolean","coarse":"boolean","partial":"boolean","build":"string","map":"string","match_file":"string","seeds":"[number]","game_types":"[string]","game_type":"string","traces":"number","seconds":"number","measures":"{name: {ok, refused?}}","measure":"string?","ok":"boolean?","value":"any?","refused":"{measure,what,have,need}?"}"#;

pub struct Playtest {
    runner: Runner,
    source: Option<Box<dyn Source>>,
    t0: Instant,
}

impl Playtest {
    pub fn new() -> Self {
        Playtest {
            runner: Runner::new(),
            source: None,
            t0: Instant::now(),
        }
    }

    /// Plays through `source`.
    pub fn with_source(source: Box<dyn Source>) -> Self {
        Playtest {
            source: Some(source),
            ..Playtest::new()
        }
    }

    fn now(&self) -> f64 {
        self.t0.elapsed().as_secs_f64()
    }

    /// Folds in the matches that ended.
    fn pump(&mut self) {
        let now = self.now();
        if let Some(source) = self.source.as_deref_mut() {
            self.runner.pump(source, now);
        }
    }
}

impl Default for Playtest {
    fn default() -> Self {
        Self::new()
    }
}

impl Provider for Playtest {
    fn methods(&self) -> Vec<MethodDef> {
        vec![
            def(
                m::PLAYTEST_STATIC,
                "The open map's static checks, as edited: placements off the bots' mesh, starting locations that cannot reach each other, islands no start reaches, one-way links with no way back, opposing starts in sight, and each built-in game type's readiness with what it lacks. A problem's target is {block, index} for nav.go or {kind: poly, index} for nav.pick. Starts the bots' mesh load; wait blocks until it is done",
                r#"{"wait?":"boolean"}"#,
                RESULT,
                r#"{"wait":true}"#,
            ),
            def(
                m::PLAYTEST_RUN,
                "Plays the open map in headless bot matches: builds it, makes the bots' mesh, starts a batch of sim matches over fixed seeds (1 to N) of the game types the static checks call ready, and answers at once. A run with the same build, match, seeds and versions as a finished one answers from it (cached). A newer run cancels one still going. Refused with no sim runner connected",
                RUN_PARAMS,
                RUN_RESULT,
                r#"{"preset":"probe"}"#,
            ),
            def(
                m::PLAYTEST_STATUS,
                "A run's progress: matches done of total, the speed-up (game seconds per wall second), whether a newer build made it stale, and each failed match with why. With no run yet the state is none",
                r#"{"run?":"string"}"#,
                STATUS_RESULT,
                "{}",
            ),
            def(
                m::PLAYTEST_RESULT,
                "A run's measures over the matches that ended so far, for one game type (the first by default): heat, spawns, flow, sight, items and stuck, each ok or refused for too few events. With measure, that one in full with its bins. The result names the build, match file and seeds it came from",
                RESULT_PARAMS,
                RESULT_RESULT,
                "{}",
            ),
            def(
                m::PLAYTEST_CANCEL,
                "Stops the running run (or the one named); what came back stays",
                r#"{"run?":"string"}"#,
                r#"{"cancelled":"boolean"}"#,
                "{}",
            ),
        ]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        let p = P(params);
        Some(match method {
            m::PLAYTEST_STATIC => run_static(app, &p),
            m::PLAYTEST_RUN => self.start(app, &p),
            m::PLAYTEST_STATUS => self.status(app, &p),
            m::PLAYTEST_RESULT => self.result(app, &p),
            m::PLAYTEST_CANCEL => self.cancel(app, &p),
            _ => return None,
        })
    }
}

impl Playtest {
    fn start(&mut self, app: &mut App, p: &P) -> Reply {
        let spec = spec_of(p)?;
        if self.source.is_none() {
            return Err(err(
                code::REFUSED,
                "no sim runner is connected: the build, the bots' mesh and the sim are not wired to Studio yet",
            ));
        }
        let report = app
            .playtest_static(true)
            .ok_or_else(|| err(code::NO_DOCUMENT, "no map is open"))?
            .map_err(|e| err(code::FAILED, e))?;
        let ready: Vec<String> = report
            .ready
            .iter()
            .filter(|g| g.ok)
            .map(|g| g.game_type.to_owned())
            .collect();
        if ready.is_empty() {
            let lacks: Vec<String> = report
                .ready
                .iter()
                .map(|g| format!("{} lacks {}", g.game_type, g.missing.join(", ")))
                .collect();
            return Err(err(
                code::FAILED,
                format!("no game type is ready: {}", lacks.join("; ")),
            ));
        }
        let now = self.now();
        let source = self.source.as_deref_mut().expect("checked above");
        let s = self
            .runner
            .start(source, &spec, &ready, now)
            .map_err(|e| err(code::FAILED, e))?;
        self.publish(app, None);
        Ok(json!({
            "run": s.run,
            "build": s.build,
            "matches": s.matches,
            "parallel": s.parallel,
            "cached": s.cached,
        }))
    }

    fn the_run(&self, p: &P) -> Result<Option<&Run>, super::RpcError> {
        self.runner
            .run(p.opt_str("run")?)
            .map_err(|e| err(code::INVALID_PARAMS, e))
    }

    /// Hands the newest run's result to the viewport's playtest layers and
    /// the panel (`app::playtest_shown`), for `game_type` or the first
    /// played.
    fn publish(&self, app: &mut App, game_type: Option<&str>) {
        match self.runner.run(None).ok().flatten() {
            Some(r) => app.playtest.show(r, game_type),
            None => app.playtest.clear(),
        }
    }

    fn status(&mut self, app: &mut App, p: &P) -> Reply {
        self.pump();
        self.publish(app, None);
        Ok(match self.the_run(p)? {
            None => json!({
                "run": null, "state": "none", "preset": null, "build": null, "done": 0, "total": 0,
                "speedup": null, "stale": false, "coarse": false, "failed": [],
            }),
            Some(r) => status_json(r),
        })
    }

    fn result(&mut self, app: &mut App, p: &P) -> Reply {
        self.pump();
        let measure = p.opt_str("measure")?;
        if let Some(n) = measure
            && !run::MEASURES.contains(&n)
        {
            return Err(err(
                code::INVALID_PARAMS,
                format!("unknown measure {n:?} ({})", run::MEASURES.join(", ")),
            ));
        }
        let game_type = p.opt_str("game_type")?;
        let Some(r) = self.the_run(p)? else {
            return Ok(json!({"run": null, "state": "none", "measures": {}}));
        };
        if let Some(g) = game_type
            && !r.game_types.iter().any(|x| x == g)
        {
            return Err(err(
                code::INVALID_PARAMS,
                format!(
                    "{g:?} is not one of the run's game types ({})",
                    r.game_types.join(", ")
                ),
            ));
        }
        let reply = result_json(r, game_type, measure);
        self.publish(app, game_type);
        Ok(reply)
    }

    fn cancel(&mut self, app: &mut App, p: &P) -> Reply {
        let id = p.opt_str("run")?;
        let Some(source) = self.source.as_deref_mut() else {
            return Ok(json!({"cancelled": false}));
        };
        let cancelled = self
            .runner
            .cancel(source, id)
            .map_err(|e| err(code::INVALID_PARAMS, e))?;
        self.publish(app, None);
        Ok(json!({"cancelled": cancelled}))
    }
}

/// The call's params as a run's spec.
fn spec_of(p: &P) -> Result<Spec, super::RpcError> {
    let bad = |e: String| err(code::INVALID_PARAMS, e);
    let preset = match p.opt_str("preset")? {
        Some(n) => Preset::parse(n).map_err(bad)?,
        None => Preset::Probe,
    };
    let game_types = match p.get("game_types") {
        None => Vec::new(),
        Some(v) => v
            .as_array()
            .and_then(|a| a.iter().map(|x| x.as_str().map(str::to_owned)).collect())
            .ok_or_else(|| bad("game_types: a list of names".into()))?,
    };
    let num = |k: &str| -> Result<Option<u32>, super::RpcError> {
        p.opt_u64(k)?
            .map(|n| u32::try_from(n).map_err(|_| bad(format!("{k}: too large"))))
            .transpose()
    };
    let spec = Spec {
        preset,
        game_types,
        seeds: num("seeds")?,
        minutes: num("minutes")?,
        bots: num("bots")?,
    };
    spec.check().map_err(bad)?;
    Ok(spec)
}

/// A run's progress.
pub fn status_json(r: &Run) -> Value {
    json!({
        "run": r.id,
        "state": r.state.name(),
        "preset": r.preset.name(),
        "build": r.build,
        "done": r.done(),
        "total": r.total(),
        "speedup": r.speedup(),
        "stale": r.stale,
        "coarse": r.coarse(),
        "failed": r.failed().iter().map(|(j, why)| json!({
            "game_type": j.game_type,
            "seed": j.seed,
            "swapped": j.swapped,
            "why": why,
        })).collect::<Vec<_>>(),
    })
}

/// A run's measures for `game_type` (the first played by default), or
/// one in full.
pub fn result_json(r: &Run, game_type: Option<&str>, measure: Option<&str>) -> Value {
    let played = r.played();
    let shown = game_type.or_else(|| played.first().copied()).unwrap_or("");
    let m = r.measures(shown);
    let mut o = json!({
        "run": r.id,
        "state": r.state.name(),
        "stale": r.stale,
        "coarse": r.coarse(),
        "partial": r.state == run::State::Running || r.done() < r.total(),
        "build": r.build,
        "map": r.map,
        "match_file": r.dir.join("match.json").display().to_string(),
        "seeds": r.seeds,
        "game_types": r.game_types,
        "game_type": shown,
        "traces": m.as_ref().map_or(0, |m| m.traces),
        "seconds": m.as_ref().map_or(0.0, |m| m.seconds),
    });
    let Some(x) = o.as_object_mut() else {
        return o;
    };
    match (&m, measure) {
        (Some(m), Some(n)) => {
            x.insert("measure".into(), json!(n));
            if let Some(Value::Object(v)) = run::measure_json(m, n) {
                x.extend(v);
            }
        }
        (Some(m), None) => {
            x.insert("measures".into(), run::summary_json(m));
        }
        (None, Some(n)) => {
            x.insert("measure".into(), json!(n));
            x.insert("ok".into(), json!(false));
            x.insert("refused".into(), json!("no match has ended yet"));
        }
        (None, None) => {
            x.insert("measures".into(), json!({}));
        }
    }
    o
}

fn run_static(app: &mut App, p: &P) -> Reply {
    let wait = p.bool("wait")?;
    let report = app
        .playtest_static(wait)
        .ok_or_else(|| err(code::NO_DOCUMENT, "no map is open"))?
        .map_err(|e| err(code::FAILED, e))?;
    Ok(report_json(&report, app.bot_nav_state().name()))
}

fn target_json(target: Option<Target>) -> Value {
    match target {
        Some(Target::Placement(kind, index)) => json!({"block": block_name(kind), "index": index}),
        Some(Target::Poly(index)) => json!({"kind": "poly", "index": index}),
        None => Value::Null,
    }
}

/// The report as `playtest.static` answers it.
pub fn report_json(r: &Report, nav: &str) -> Value {
    json!({
        "ok": r.ok(),
        "nav": nav,
        "problems": r.problems.iter().map(|p| json!({
            "kind": p.kind.name(),
            "text": p.text,
            "target": target_json(p.target),
        })).collect::<Vec<_>>(),
        "ready": r.ready.iter().map(|g| json!({
            "game_type": g.game_type,
            "engine": g.engine,
            "ok": g.ok,
            "missing": g.missing,
        })).collect::<Vec<_>>(),
    })
}
