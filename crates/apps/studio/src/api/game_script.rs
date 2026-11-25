//! `rules.*`: the game type script editor, for agents
//! ([`super::Provider`], registered in [`super::seams`]).
//!
//! The same [`crate::app::rules_edit::RulesEditor`] the window draws:
//! open a file, a built-in or text; read it with its problems and check
//! report; set, format and save it; and reload it into the paired game.
//! `rules.reload` answers once the reload is queued for the game link;
//! `rules.get`'s `reload` reads the game's answer. `rules.external` opens
//! the script in VS Code or Zed, and `rules.run` runs it over a scenario
//! on the fake game, as `h2 rules run` does; the editor extensions'
//! commands call both (`h2 rules lsp`). docs/editor/agent-api.md,
//! docs/editor/editor-guide.md ("Game type scripts", "Scripts in VS Code
//! and Zed").

use std::path::{Path, PathBuf};

use agent_proto::rules::{ReloadMode, Reloaded};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::rules_edit::{self, DiskPoll, Launch, ReloadState, RulesEditor, Watch};

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

const STATE: &str = r#"{"title":"string","path":"string|null","dirty":"boolean","text?":"string","problems":"[{line,col,severity,message,code}]","check":"{ok,hash,name,version,errors:[{line,col,message}],report:{worst_tick_ops,budget,fits,costliest,handlers,slots,ir_bytes,ir_limit,capabilities,unhostable,settings:[{name,label,kind,value,text}],quick:[{kind,name,label,value}]}|null}","mode":"auto|in_place|restart","reload_on_save":"boolean","reload":"{state:idle|sending|done|refused,mode?:in_place|restarted,hash?,tick?,reasons?,why?}"}"#;

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
        r#"{"source":"rules \"Mine\" 1\ntype slayer\nuses slayer\n"}"#,
    ),
    def(
        m::RULES_SAVE,
        "Save the script as a rules item of the edit target; with none, answers NO_TARGET. A path explicitly exports a copy. Package saves reload into the paired game when reload is true, or when the editor's reload on save is on",
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
        "Compile and verify the script, then send it to the paired game's rules.reload: in place when the game's state keeps its shape, else restart (mode auto); in_place refuses rather than restart. With path (an outside editor's save), the file as saved first: read into the open script when it is that file (UNSAVED when it has other unsaved edits here), else opened (UNSAVED unless discard while the open script has unsaved edits) and watched. Answers once queued; rules.get's reload has the game's answer",
        r#"{"mode?":"auto|in_place|restart","path?":"string","discard?":"boolean"}"#,
        r#"{"queued":"string (the blob's SHA-256)"}"#,
        r#"{"mode":"auto"}"#,
    ),
    def(
        m::RULES_EXTERNAL,
        "Open the script in VS Code (editor code), Zed (zed) or any editor (auto, the default), at line (default: the cursor's): saved to its file, or a new one under the data root, which is watched both ways. launch false only answers the file and the commands that would open it. REFUSED when launching and no such editor is found",
        r#"{"editor?":"code|zed|auto","line?":"integer","launch?":"boolean"}"#,
        r#"{"path":"string","line":"integer","col":"integer","editor":"code|zed|auto","commands":"[{program,args,line}]","launched":"boolean"}"#,
        r#"{"editor":"auto","launch":false}"#,
    ),
    def(
        m::RULES_RUN,
        "Run a script over a scenario on the fake game, as h2 rules run does: scenario is a scenario file's path or the scenario itself; the script is source, else the file at path, else the open script. label names it in the printout; trace adds every call",
        r#"{"scenario":"string|object","source?":"string","path?":"string","label?":"string","trace?":"boolean"}"#,
        r#"{"ok":"boolean","result":"string","text":"string","ticks":"integer","calls":"integer","label":"string"}"#,
        r#"{"scenario":{"ticks":[{}]}}"#,
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

pub(super) fn save_item(app: &mut App, p: &P) -> Reply {
    let reload = match p.get("reload") {
        Some(_) => p.bool("reload")?,
        None => app.rules.reload_on_save,
    };
    let target = super::packages::save_target(app, None)?;
    let saved = app.save_rules_item(&target).map_err(super::packages::refuse)?;
    let reloaded = reload.then(|| match app.rules.queue_reload() {
        Ok(hash) => json!({ "queued": hash }),
        Err(why) => json!({ "refused": why }),
    });
    Ok(json!({ "saved": true, "path": saved, "edit_target": target, "reload": reloaded }))
}

fn save(app: &mut App, p: &P) -> Reply {
    if let Some(path) = p.opt_str("path")? {
        std::fs::write(path, &app.rules.text)
            .map_err(|e| err(code::FAILED, format!("{path}: {e}")))?;
        Ok(json!({ "path": path, "reload": null }))
    } else {
        save_item(app, p)
    }
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::RULES_EXTERNAL => return external(app, p),
        m::RULES_SAVE => return save(app, p),
        m::RULES_RUN => return run(app, p),
        _ => {}
    }
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
                r.set_text(src.to_string());
            } else {
                r.new_blank();
            }
            Ok(state(r, false))
        }
        m::RULES_GET => Ok(state(r, p.bool("text")?)),
        m::RULES_SET => {
            r.set_text(p.str("source")?.to_string());
            Ok(state(r, false))
        }
        m::RULES_FORMAT => {
            r.format().map_err(refused)?;
            Ok(state(r, false))
        }
        m::RULES_RELOAD => {
            // The mode is checked before the file is taken.
            let wanted = p.opt_str("mode")?.map(mode).transpose()?;
            if let Some(path) = p.opt_str("path")? {
                take_file(r, Path::new(path), p.bool("discard")?)?;
            }
            if let Some(want) = wanted {
                r.mode = want;
            }
            let hash = r.queue_reload().map_err(refused)?;
            Ok(json!({ "queued": hash }))
        }
        _ => unreachable!("checked against METHODS"),
    }
}

/// Whether `a` and `b` name the same file.
fn same_file(a: &Path, b: &Path) -> bool {
    a == b
        || a.canonicalize()
            .ok()
            .is_some_and(|x| b.canonicalize().ok() == Some(x))
}

/// `rules.reload`'s `path`: the file as an outside editor saved it, into
/// the editor. The open script's own file is read now (not at the next
/// poll), and refused while unsaved edits here differ from it; another
/// file is opened, unless the open script has unsaved edits and not
/// `discard`, and watched like one "Open in VS Code" opened.
fn take_file(r: &mut RulesEditor, path: &Path, discard: bool) -> Result<(), agent_proto::RpcError> {
    if let Some(own) = r.path.clone().filter(|p| same_file(p, path)) {
        let disk = std::fs::read_to_string(&own)
            .map_err(|e| err(code::FAILED, format!("{}: {e}", own.display())))?;
        if r.disk_text(disk) == DiskPoll::Asked {
            return Err(err(
                code::UNSAVED,
                format!(
                    "{} changed on disk and has other unsaved edits in Studio: pick which to keep in the Game type script window",
                    own.display()
                ),
            ));
        }
        // Read now: the next poll has nothing more to take.
        let seen = rules_edit::modified(&own);
        match r.watch.as_mut().filter(|w| w.path == own) {
            Some(w) => w.seen = seen,
            None => r.watch = Some(Watch::new(own, seen)),
        }
        return Ok(());
    }
    if r.dirty() && !discard {
        return Err(err(
            code::UNSAVED,
            format!(
                "{} has unsaved edits in Studio: save it, or pass discard: true to open {}",
                r.title,
                path.display()
            ),
        ));
    }
    r.open_path(path).map_err(|e| err(code::FAILED, e))?;
    r.watch = Some(Watch::new(path.to_path_buf(), rules_edit::modified(path)));
    Ok(())
}

/// The ways to open a file, on the wire.
pub fn commands(launches: &[Launch]) -> Vec<Value> {
    launches
        .iter()
        .map(|l| {
            json!({
                "program": l.program.display().to_string(),
                "args": l.args,
                "line": l.describe(),
            })
        })
        .collect()
}

/// The `editor` param as `code`, `zed` or (`None`) any, its wire name,
/// and `launch` (default true).
pub fn editor_params(p: &P) -> Result<(Option<&'static str>, &'static str, bool), agent_proto::RpcError> {
    let only = rules_edit::editor_param(p.opt_str("editor")?.unwrap_or("auto"))
        .map_err(|e| err(code::INVALID_PARAMS, e))?;
    let launch = match p.get("launch") {
        Some(_) => p.bool("launch")?,
        None => true,
    };
    Ok((only, only.unwrap_or("auto"), launch))
}

/// `rules.external`.
fn external(app: &mut App, p: &P) -> Reply {
    let (only, editor, launch) = editor_params(p)?;
    let line = p
        .opt_u64("line")?
        .map(|l| u32::try_from(l).unwrap_or(u32::MAX));
    let root = app.data_root();
    let (path, (line, col), launches) = app
        .rules
        .external_launches(&root, only, line)
        .map_err(|e| err(code::FAILED, e))?;
    let cmds = commands(&launches);
    if launch {
        if launches.is_empty() {
            return Err(refused(rules_edit::no_editor(only)));
        }
        app.rules.launched = Some(rules_edit::launch(launches));
        app.info(format!("Opening {}", path.display()));
    }
    Ok(json!({
        "path": path.display().to_string(),
        "line": line,
        "col": col,
        "editor": editor,
        "commands": cmds,
        "launched": launch,
    }))
}

/// `rules.run`.
fn run(app: &mut App, p: &P) -> Reply {
    let scenario = match p.get("scenario") {
        Some(Value::String(path)) => std::fs::read_to_string(path)
            .map_err(|e| err(code::FAILED, format!("scenario: {path}: {e}")))?,
        Some(v @ Value::Object(_)) => v.to_string(),
        Some(v) => {
            return Err(err(
                code::INVALID_PARAMS,
                format!("scenario: expected a file's path or a scenario object, not {v}"),
            ));
        }
        None => return Err(err(code::INVALID_PARAMS, "scenario is required")),
    };
    let (src, label) = if let Some(src) = p.opt_str("source")? {
        (src.to_owned(), p.opt_str("path")?.map(str::to_owned))
    } else if let Some(path) = p.opt_str("path")? {
        let text = std::fs::read_to_string(PathBuf::from(path))
            .map_err(|e| err(code::FAILED, format!("{path}: {e}")))?;
        (text, Some(path.to_owned()))
    } else {
        (app.rules.text.clone(), Some(app.rules.title.clone()))
    };
    let label = p
        .opt_str("label")?
        .map(str::to_owned)
        .or(label)
        .unwrap_or_default();
    let r = rules_edit::run_scenario(&src, &scenario, p.bool("trace")?).map_err(refused)?;
    Ok(json!({
        "ok": r.ok,
        "result": r.outcome,
        "text": r.text,
        "ticks": r.ticks,
        "calls": r.calls,
        "label": label,
    }))
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
                "settings": rep.settings.iter().map(|s| json!({
                    "name": s.name, "label": s.label, "kind": s.kind,
                    "value": s.value, "text": s.text,
                })).collect::<Vec<_>>(),
                "quick": rep.quick.iter().map(|q| json!({
                    "kind": q.kind, "name": q.name, "label": q.label, "value": q.value,
                })).collect::<Vec<_>>(),
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
        assert!(v["text"].as_str().unwrap().contains("type slayer"));
        assert!(v["starters"].as_array().unwrap().iter().any(|s| s == "Slayer (built in)"));

        // The report lists the script's settings and its Quick Options marks.
        r.open_starter("SWAT").unwrap();
        let v = state(&mut r, false);
        let report = &v["check"]["report"];
        assert!(!report["settings"].as_array().unwrap().is_empty(), "{v}");
        assert_eq!(report["quick"].as_array().unwrap().len(), 5, "{v}");
        assert_eq!(report["quick"][0]["kind"], "setting");

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
