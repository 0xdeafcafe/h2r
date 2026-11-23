//! `playtest.static`: the open map's static checks
//! ([`crate::app::playtest_static`]; docs/editor/playtest.md, "Static
//! checks"). Read only. It starts the bots' mesh load when nothing has
//! (`nav.bots`'s load), and `wait` blocks until it is done.
//!
//! The rest of the `playtest.*` namespace (runs, results, compares) comes
//! with the runner (playtest.md, "Build slices", PT4 on).

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::params::block_name;
use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::app::playtest_static::{Report, Target};

const RESULT: &str = r#"{"ok":"boolean","nav":"no_map|idle|loading|ready|failed","problems":"[{kind,text,target}]","ready":"[{game_type,engine,ok,missing}]"}"#;

pub struct Playtest;

impl Provider for Playtest {
    fn methods(&self) -> Vec<MethodDef> {
        vec![def(
            m::PLAYTEST_STATIC,
            "The open map's static checks, as edited: placements off the bots' mesh, starting locations that cannot reach each other, islands no start reaches, one-way links with no way back, opposing starts in sight, and each built-in game type's readiness with what it lacks. A problem's target is {block, index} for nav.go or {kind: poly, index} for nav.pick. Starts the bots' mesh load; wait blocks until it is done",
            r#"{"wait?":"boolean"}"#,
            RESULT,
            r#"{"wait":true}"#,
        )]
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if method != m::PLAYTEST_STATIC {
            return None;
        }
        let p = P(params);
        Some(run(app, &p))
    }
}

fn run(app: &mut App, p: &P) -> Reply {
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
