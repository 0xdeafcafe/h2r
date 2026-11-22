//! `rules.*`: the game type script editor, for agents
//! ([`super::Provider`], registered in [`super::seams`]).
//!
//! The same [`crate::app::rules_edit::RulesEditor`] the window draws:
//! open a file, a built-in or text; read it with its problems and check
//! report; set, format and save it; and reload it into the paired game.
//! `rules.reload` answers once the reload is queued for the game link;
//! `rules.get`'s `reload` reads the game's answer. docs/editor/agent-api.md,
//! docs/editor/editor-guide.md ("Game type scripts").

use std::path::Path;

use agent_proto::rules::{ReloadMode, Reloaded};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::rules_edit::{self, ReloadState, RulesEditor};

/// The script editor provider.
#[derive(Default)]
pub struct GameScript;

const fn def(
    name: &'static str,
    summary: &'static str,
    params: &'static str,
    result: &'static str,
    example: &'static str,
) -> MethodDef {
    MethodDef {
        name,
        summary,
        params,
        result,
        example,
        planned: None,
    }
}

const STATE: &str = r#"{"title":"string","path":"string|null","dirty":"boolean","text?":"string","problems":"[{line,col,severity,message,code}]","check":"{ok,hash,name,version,errors:[{line,col,message}],report:{worst_tick_ops,budget,fits,costliest,handlers,slots,ir_bytes,ir_limit,capabilities,unhostable}|null}","mode":"auto|in_place|restart","reload_on_save":"boolean","reload":"{state:idle|sending|done|refused,mode?:in_place|restarted,hash?,tick?,reasons?,why?}"}"#;

const METHODS: &[MethodDef] = &[
    def(
        m::RULES_OPEN,
        "Open a .rules file, a built-in or example by name (rules.get's starters), or text, in the script editor; none of them opens a blank script. UNSAVED unless discard when the open script has unsaved edits",
        r#"{"path?":"string","starter?":"string","source?":"string","discard?":"boolean"}"#,
        STATE,
        r#"{"starter":"slayer"}"#,
    ),
    def(
        m::RULES_GET,
        "The open script: its text (with text: true), problems, check report (budget, slots, capabilities), the reload mode and the last reload's state, and the starters",
        r#"{"text?":"boolean"}"#,
        STATE,
        r#"{"text":true}"#,
    ),
    def(
        m::RULES_SET,
        "Replace the open script's text (unsaved) and check it",
        r#"{"source":"string"}"#,
        STATE,
        r#"{"source":"rules \"Mine\" 1\ncarrier slayer\n"}"#,
    ),
    def(
        m::RULES_SAVE,
        "Save the script to its file, or to path (which becomes its file). Reloads it into the paired game when reload is true, or by default when the editor's reload on save is on",
        r#"{"path?":"string","reload?":"boolean"}"#,
        r#"{"path":"string","reload":"{queued:hash}|{refused:why}|null"}"#,
        r#"{"path":"$OUT/mine.rules"}"#,
    ),
    def(
        m::RULES_FORMAT,
        "Format the script (refused while it does not parse)",
        "{}",
        STATE,
        "null",
    ),
    def(
        m::RULES_RELOAD,
        "Compile and verify the script, then send it to the paired game's rules.reload: in place when the game's state keeps its shape, else restart (mode auto); in_place refuses rather than restart. Answers once queued; rules.get's reload has the game's answer",
        r#"{"mode?":"auto|in_place|restart"}"#,
        r#"{"queued":"string (the blob's SHA-256)"}"#,
        r#"{"mode":"auto"}"#,
    ),
];

impl Provider for GameScript {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS.to_vec()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        if !METHODS.iter().any(|d| d.name == method) {
            return None;
        }
        Some(call(app, method, &P(params)))
    }
}

fn refused(why: impl Into<String>) -> agent_proto::RpcError {
    err(code::REFUSED, why)
}

fn mode(s: &str) -> Result<ReloadMode, agent_proto::RpcError> {
    Ok(match s {
        "auto" => ReloadMode::Auto,
        "in_place" => ReloadMode::InPlace,
        "restart" => ReloadMode::Restart,
        other => {
            return Err(err(
                code::INVALID_PARAMS,
                format!("mode: expected auto, in_place or restart, not {other}"),
            ));
        }
    })
}

fn mode_name(m: ReloadMode) -> &'static str {
    match m {
        ReloadMode::Auto => "auto",
        ReloadMode::InPlace => "in_place",
        ReloadMode::Restart => "restart",
    }
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    let r = &mut app.rules;
    match method {
        m::RULES_OPEN => {
            let given = ["path", "starter", "source"].iter().filter(|k| p.has(k)).count();
            if given > 1 {
                return Err(err(
                    code::INVALID_PARAMS,
                    "give one of path, starter and source",
                ));
            }
            if r.dirty() && !p.bool("discard")? {
                return Err(err(
                    code::UNSAVED,
                    "the open script has unsaved edits: save it, or pass discard: true",
                ));
            }
            if let Some(path) = p.opt_str("path")? {
                r.open_path(Path::new(path)).map_err(|e| err(code::FAILED, e))?;
            } else if let Some(name) = p.opt_str("starter")? {
                r.open_starter(name).map_err(refused)?;
            } else if let Some(src) = p.opt_str("source")? {
                r.new_blank();
                r.text = src.to_string();
            } else {
                r.new_blank();
            }
            Ok(state(r, false))
        }
        m::RULES_GET => Ok(state(r, p.bool("text")?)),
        m::RULES_SET => {
            r.text = p.str("source")?.to_string();
            r.cursor = r.cursor.min(r.text.len());
            while !r.text.is_char_boundary(r.cursor) {
                r.cursor -= 1;
            }
            Ok(state(r, false))
        }
        m::RULES_SAVE => {
            let path = p.opt_str("path")?;
            let saved = r
                .save(path.map(Path::new))
                .map_err(|e| err(if r.path.is_none() { code::REFUSED } else { code::FAILED }, e))?;
            let reload = match p.get("reload") {
                Some(_) => p.bool("reload")?,
                None => r.reload_on_save,
            };
            let reloaded = reload.then(|| match r.queue_reload() {
                Ok(hash) => json!({ "queued": hash }),
                Err(why) => json!({ "refused": why }),
            });
            Ok(json!({ "path": saved.display().to_string(), "reload": reloaded }))
        }
        m::RULES_FORMAT => {
            r.format().map_err(refused)?;
            Ok(state(r, false))
        }
        m::RULES_RELOAD => {
            if let Some(s) = p.opt_str("mode")? {
                r.mode = mode(s)?;
            }
            let hash = r.queue_reload().map_err(refused)?;
            Ok(json!({ "queued": hash }))
        }
        _ => unreachable!("checked against METHODS"),
    }
}

/// `rules.get`'s answer.
pub fn state(r: &mut RulesEditor, with_text: bool) -> Value {
    r.refresh();
    let problems: Vec<Value> = rules_edit::problems(&r.text, &r.analysis.diagnostics)
        .into_iter()
        .map(|p| {
            json!({
                "line": p.line, "col": p.col, "severity": p.severity,
                "message": p.message, "code": p.code,
            })
        })
        .collect();
    let check = r.analysis.check.as_ref().map(|c| {
        json!({
            "ok": c.ok,
            "hash": c.hash,
            "name": c.name,
            "version": c.version,
            "errors": c.errors.iter().map(|e| json!({
                "line": e.line, "col": e.col, "message": e.message,
            })).collect::<Vec<_>>(),
            "report": c.report.as_ref().map(|rep| json!({
                "worst_tick_ops": rep.worst_tick_ops,
                "budget": rep.budget,
                "fits": rep.fits,
                "costliest": rep.costliest,
                "handlers": rep.handlers.iter().map(|h| json!({
                    "event": h.event, "worst_ops": h.worst_ops,
                })).collect::<Vec<_>>(),
                "slots": rep.slots.iter().map(|s| json!({
                    "scope": s.scope, "used": s.used, "limit": s.limit,
                })).collect::<Vec<_>>(),
                "ir_bytes": rep.ir_bytes,
                "ir_limit": rep.ir_limit,
                "capabilities": rep.capabilities.iter().map(|c| json!({
                    "name": c.name, "words": c.words,
                    "grantable": c.grantable, "known": c.known,
                })).collect::<Vec<_>>(),
                "unhostable": rep.unhostable,
            })),
        })
    });
    let starters: Vec<String> = rules_edit::starters().into_iter().map(|(l, _)| l).collect();
    let mut out = json!({
        "title": r.title,
        "path": r.path.as_ref().map(|p| p.display().to_string()),
        "dirty": r.dirty(),
        "problems": problems,
        "check": check,
        "mode": mode_name(r.mode),
        "reload_on_save": r.reload_on_save,
        "reload": reload(&r.reload),
        "starters": starters,
    });
    if with_text {
        out["text"] = json!(r.text);
    }
    out
}

/// A reload state on the wire.
pub fn reload(s: &ReloadState) -> Value {
    match s {
        ReloadState::Idle => json!({ "state": "idle" }),
        ReloadState::Sending(hash) => json!({ "state": "sending", "hash": hash }),
        ReloadState::Done(r) => json!({
            "state": "done",
            "mode": match r.mode {
                Reloaded::InPlace => "in_place",
                Reloaded::Restarted => "restarted",
            },
            "hash": r.hash,
            "tick": r.tick,
            "reasons": r.reasons,
        }),
        ReloadState::Refused(why) => json!({ "state": "refused", "why": why }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_names_round_trip() {
        for m in [ReloadMode::Auto, ReloadMode::InPlace, ReloadMode::Restart] {
            assert_eq!(mode(mode_name(m)).unwrap(), m);
        }
        assert_eq!(mode("later").unwrap_err().code, code::INVALID_PARAMS);
    }

    #[test]
    fn the_state_reads_the_script_its_problems_and_its_report() {
        let mut r = RulesEditor::default();
        let v = state(&mut r, true);
        assert_eq!(v["dirty"], false);
        assert_eq!(v["check"]["ok"], true, "{v}");
        assert!(v["check"]["report"]["budget"].as_u64().unwrap() > 0);
        assert_eq!(v["reload"]["state"], "idle");
        assert!(v["text"].as_str().unwrap().contains("carrier slayer"));
        assert!(v["starters"].as_array().unwrap().iter().any(|s| s == "Slayer (built in)"));

        r.text.push_str("\non kill( {\n");
        let v = state(&mut r, false);
        assert_eq!(v["dirty"], true);
        assert!(v.get("text").is_none());
        assert_eq!(v["problems"][0]["severity"], "error", "{v}");
        assert_eq!(v["check"]["ok"], false);
    }

    #[test]
    fn reload_states_on_the_wire() {
        assert_eq!(reload(&ReloadState::Refused("no".into()))["why"], "no");
        let done = ReloadState::Done(agent_proto::rules::ReloadResult {
            mode: Reloaded::Restarted,
            hash: "ab".into(),
            reasons: vec!["the script has 1 player variables, not 0".into()],
            tick: 9,
        });
        let v = reload(&done);
        assert_eq!(v["mode"], "restarted");
        assert_eq!(v["reasons"][0], "the script has 1 player variables, not 0");
    }
}
