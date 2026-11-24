//! The Scripts tool as the agent API's `script.*` ([`super::Provider`],
//! registered in [`super::seams`]): the open map's globals and scripts,
//! a definition's source with its links, following one, who uses a name,
//! the builtins and where their names come from, and the documentation
//! listing to name builtins from; `script.external` writes the working
//! copy VS Code and Zed edit and opens it, and `script.working` reads
//! back what a save there changed (`app::external`). The map is read
//! only; selection and following go through `Action::Script`, the
//! window's path. docs/editor/rich-editors.md, "Scripts";
//! docs/editor/editor-guide.md, "Scripts in VS Code and Zed";
//! docs/editor/agent-api.md.

use std::path::PathBuf;
use std::time::Instant;

use agent_proto::RpcError;
use hs::{Item, NameFrom};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::game_script::{commands, editor_params};
use super::params::block_name;
use super::scenario::refused;
use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::app::scripts::{Entry, Link, LinkKind, Loaded, Target};
use crate::app::{Action, ScriptAction};

/// The Scripts tool's provider.
#[derive(Default)]
pub struct Scripts;

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

const METHODS: &[MethodDef] = &[
    def(
        m::SCRIPT_LIST,
        "The open map's scripts and globals, decompiled with the source's default.xbe: name, kind, type and the node slots each shows; `filter` keeps those whose name or source contains it (any case)",
        r#"{"filter?":"string"}"#,
        r#"{"map":"string","executable":"string","scripts":"[{name,kind,type,nodes}]","globals":"[{name,type,nodes}]","free_nodes":"integer","node_limit":"integer"}"#,
        "null",
    ),
    def(
        m::SCRIPT_SOURCE,
        "A script's (else a global's) source, shown from now on (default: the one shown), and every link in it: its byte span, kind (script, global, object, volume, tag), name, and where it goes now, or why it goes nowhere",
        r#"{"name?":"string"}"#,
        r#"{"name":"string","item":"script|global","kind":"string","type":"string","source":"string","links":"[{span:[start,end],kind,name,target,why?}]"}"#,
        r#"{"name":"main"}"#,
    ),
    def(
        m::SCRIPT_FOLLOW,
        "Follow link `link` (script.source's index) of a definition (default: the one shown), as a click does: a script or global shows it, an object name selects its placement in Scene, a volume selects it, a tag opens it in Tags",
        r#"{"name?":"string","link":"integer"}"#,
        r#"{"kind":"string","name":"string","target":"object","tool":"string","log":"[string]"}"#,
        r#"{"name":"helper","link":0}"#,
    ),
    def(
        m::SCRIPT_USES,
        "The definitions that name a script, global, object name, trigger volume or tag (by the name the scripts spell, or a tag's id), with how many times",
        r#"{"kind":"script|global|object|volume|tag|recording","name":"string"}"#,
        r#"{"kind":"string","name":"string","uses":"[{name,item,count}]"}"#,
        r#"{"kind":"object","name":"door"}"#,
    ),
    def(
        m::SCRIPT_BUILTINS,
        "Every builtin of the executable: its name and where it comes from (map: a call in this map's scripts; doc: the script documentation listing, by matching return and parameter types), or #builtin:<index>. `filter` matches the call form; `unnamed` keeps the unnamed",
        r#"{"filter?":"string","unnamed?":"boolean"}"#,
        r#"{"builtins":"[{index,name,label,from,returns,parameters,own_arguments,call,usage}]","named_by_map":"integer","named_by_doc":"integer","unnamed":"integer","doc":"object|null"}"#,
        r#"{"filter":"select"}"#,
    ),
    def(
        m::SCRIPT_DOC,
        "Name builtins from a script documentation listing (the user's own hs_doc text, read now and never stored), or (null) from scenarios only; what it named",
        r#"{"path":"string|null"}"#,
        r#"{"path":"string|null","doc":"object|null","log":"[string]"}"#,
        r#"{"path":null}"#,
    ),
    def(
        m::SCRIPT_EXTERNAL,
        "Write every script and global of the open map to the working copy (<data root>/scripts/<map>/<map>.hsc, with the h2hs.json context h2 hs lsp checks it against; a copy holding edits is kept) and open it in VS Code (editor code), Zed (zed) or any editor (auto, the default) at definition name (default: the one shown). launch false only answers the file and the commands that would open it. REFUSED when launching and no such editor is found",
        r#"{"name?":"string","editor?":"code|zed|auto","launch?":"boolean"}"#,
        r#"{"path":"string","context":"string","line":"integer","editor":"code|zed|auto","commands":"[{program,args,line}]","launched":"boolean","working":"object (script.working's answer)"}"#,
        r#"{"name":"main","launch":false}"#,
    ),
    def(
        m::SCRIPT_WORKING,
        "The working copy, read and checked now: its file, whether it holds edits, which definitions differ from the map's (changed, added, removed), its problems against the context, and stale when the map's scripts changed under edits. The map is not written: that needs the script compiler (slices H2, H3). open false when no working copy is open",
        "{}",
        r#"{"open":"boolean","map?":"string","path?":"string","edited?":"boolean","changed?":"[string]","added?":"[string]","removed?":"[string]","problems?":"[{line,col,severity,message,code}]","stale?":"boolean","summary?":"string"}"#,
        "null",
    ),
];

impl Provider for Scripts {
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

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    app.sync_scripts();
    if method == m::SCRIPT_DOC {
        let path = p.opt_str("path")?.map(PathBuf::from);
        let log = app
            .perform(Action::Script(ScriptAction::SetDoc(path.clone())), false)
            .map_err(refused)?;
        let doc = app.scripts_loaded().ok().and_then(doc_json);
        return Ok(json!({
            "path": path.map(|p| p.display().to_string()),
            "doc": doc,
            "log": log,
        }));
    }
    if method == m::SCRIPT_WORKING {
        app.check_working_copy(Instant::now(), true);
        return Ok(working(app));
    }
    loaded(app)?;
    match method {
        m::SCRIPT_LIST => list(app, p.opt_str("filter")?.unwrap_or("")),
        m::SCRIPT_EXTERNAL => {
            let (only, editor, launch) = editor_params(p)?;
            let name = match p.opt_str("name")? {
                Some(n) => {
                    pick(app, Some(n))?;
                    Some(n)
                }
                None => None,
            };
            let (path, line, launches) = app
                .open_scripts_in_editor(name, only, false)
                .map_err(|why| err(code::FAILED, why))?;
            let cmds = commands(&launches);
            if launch {
                if launches.is_empty() {
                    return Err(err(code::REFUSED, crate::app::rules_edit::no_editor(only)));
                }
                if let Some(w) = &mut app.scripts.external {
                    w.launched = Some(crate::app::rules_edit::launch(launches));
                }
                app.info(format!("Opening {} at line {line}", path.display()));
            }
            let context = path.with_file_name(hs::ide::CONTEXT_FILE);
            Ok(json!({
                "path": path.display().to_string(),
                "context": context.display().to_string(),
                "line": line,
                "editor": editor,
                "commands": cmds,
                "launched": launch,
                "working": working(app),
            }))
        }
        m::SCRIPT_SOURCE => {
            let item = pick(app, p.opt_str("name")?)?;
            app.perform(Action::Script(ScriptAction::Select(item)), false)
                .map_err(refused)?;
            let l = loaded(app)?;
            let e = l.entry(item).expect("picked");
            Ok(json!({
                "name": e.source.name,
                "item": item_kind(item),
                "kind": e.kind,
                "type": e.type_name,
                "source": e.source.source,
                "links": e.links.iter().map(|k| link_json(app, l, k)).collect::<Vec<_>>(),
            }))
        }
        m::SCRIPT_FOLLOW => {
            let item = pick(app, p.opt_str("name")?)?;
            let index = p.usize("link")?;
            let (kind, name, target) = {
                let l = loaded(app)?;
                let e = l.entry(item).expect("picked");
                let k = e.links.get(index).ok_or_else(|| {
                    err(
                        code::INVALID_PARAMS,
                        format!("link: {} has {} links", e.source.name, e.links.len()),
                    )
                })?;
                let target = l
                    .resolve(k, app.session.scenario())
                    .map(|t| target_json(l, &t))
                    .map_err(|why| {
                        err(
                            code::REFUSED,
                            format!("{} {}: {why}", k.kind.name(), k.name),
                        )
                    })?;
                (k.kind.name(), k.name.clone(), target)
            };
            let log = app
                .perform(
                    Action::Script(ScriptAction::Follow { item, link: index }),
                    false,
                )
                .map_err(refused)?;
            Ok(json!({
                "kind": kind,
                "name": name,
                "target": target,
                "tool": super::tool_name(app.tool),
                "log": log,
            }))
        }
        m::SCRIPT_USES => {
            let kind = p.str("kind")?;
            let name = p.str("name")?;
            if kind == "recording" {
                let l = loaded(app)?;
                let rows = app.recordings_loaded().map_err(|why| err(code::REFUSED, why))?;
                let index = rows
                    .iter()
                    .position(|r| r.recorded.name.eq_ignore_ascii_case(name))
                    .ok_or_else(|| {
                        err(
                            code::INVALID_PARAMS,
                            format!("name: no recording {name:?} (recording.list lists them)"),
                        )
                    })?;
                let uses: Vec<Value> = l
                    .recording_uses(rows, index)
                    .into_iter()
                    .map(|(item, user, count)| {
                        json!({"name": user, "item": item_kind(item), "count": count})
                    })
                    .collect();
                return Ok(json!({ "kind": kind, "name": name, "uses": uses }));
            }
            let k = LinkKind::parse(kind).ok_or_else(|| {
                err(
                    code::INVALID_PARAMS,
                    format!(
                        "kind: {kind:?} is none of {}",
                        LinkKind::ALL.map(LinkKind::name).join(", ")
                    ),
                )
            })?;
            let l = loaded(app)?;
            let uses: Vec<Value> = l
                .uses(k, name)
                .into_iter()
                .map(|(item, user, count)| {
                    json!({"name": user, "item": item_kind(item), "count": count})
                })
                .collect();
            Ok(json!({ "kind": kind, "name": name, "uses": uses }))
        }
        m::SCRIPT_BUILTINS => builtins(
            loaded(app)?,
            p.opt_str("filter")?.unwrap_or(""),
            p.bool("unnamed")?,
        ),
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

/// The read tables, or the tool's refusal: no map (`NO_DOCUMENT`), else
/// why they did not read (`REFUSED`, the executable's sentence first).
fn loaded(app: &App) -> Result<&Loaded, RpcError> {
    app.scripts_loaded().map_err(|why| {
        let c = if app.session.is_open() {
            code::REFUSED
        } else {
            code::NO_DOCUMENT
        };
        err(c, why)
    })
}

fn pick(app: &App, name: Option<&str>) -> Result<Item, RpcError> {
    let l = loaded(app)?;
    match name {
        Some(n) => l.find(n).ok_or_else(|| {
            err(
                code::INVALID_PARAMS,
                format!("name: no script or global {n:?} (script.list lists them)"),
            )
        }),
        None => app
            .scripts
            .selected
            .ok_or_else(|| err(code::INVALID_PARAMS, "name: none is shown; give one")),
    }
}

/// `script.working`'s answer: the working copy as last read.
fn working(app: &App) -> Value {
    let Some(w) = &app.scripts.external else {
        return json!({ "open": false });
    };
    let problems: Vec<Value> = w
        .problems
        .iter()
        .map(|d| {
            json!({
                "line": d.range.start.line + 1,
                "col": d.range.start.character + 1,
                "severity": match d.severity {
                    hs::ide::severity::ERROR => "error",
                    hs::ide::severity::WARNING => "warning",
                    _ => "information",
                },
                "message": d.message,
                "code": d.code,
            })
        })
        .collect();
    json!({
        "open": true,
        "map": w.map,
        "path": w.path.display().to_string(),
        "edited": !w.changes.is_empty(),
        "changed": w.changes.changed,
        "added": w.changes.added,
        "removed": w.changes.removed,
        "problems": problems,
        "stale": w.stale,
        "summary": w.summary(),
    })
}

fn item_kind(item: Item) -> &'static str {
    match item {
        Item::Script(_) => "script",
        Item::Global(_) => "global",
    }
}

fn list(app: &App, filter: &str) -> Reply {
    let l = loaded(app)?;
    let row = |e: &Entry| match e.source.item {
        Item::Script(_) => {
            json!({"name": e.source.name, "kind": e.kind, "type": e.type_name, "nodes": e.nodes})
        }
        Item::Global(_) => json!({"name": e.source.name, "type": e.type_name, "nodes": e.nodes}),
    };
    let (globals, scripts): (Vec<&Entry>, Vec<&Entry>) = l
        .matching(filter)
        .partition(|e| matches!(e.source.item, Item::Global(_)));
    Ok(json!({
        "map": l.map,
        "executable": l.executable,
        "scripts": scripts.into_iter().map(row).collect::<Vec<_>>(),
        "globals": globals.into_iter().map(row).collect::<Vec<_>>(),
        "free_nodes": l.free_nodes(),
        "node_limit": hs::RETAIL_NODES,
    }))
}

fn target_json(l: &Loaded, t: &Target) -> Value {
    match *t {
        Target::Item(item) => {
            let name = l.entry(item).map(|e| e.source.name.clone());
            let mut v = serde_json::Map::new();
            v.insert(item_kind(item).to_owned(), json!(name));
            Value::Object(v)
        }
        Target::Placement(kind, index) => json!({"block": block_name(kind), "index": index}),
        Target::Volume(index) => json!({"volume": index}),
        Target::Tag(id) => json!({"tag": super::scenario::tag_id(id)}),
    }
}

fn link_json(app: &App, l: &Loaded, k: &Link) -> Value {
    let mut v = json!({
        "span": [k.start, k.end],
        "kind": k.kind.name(),
        "name": k.name,
    });
    match l.resolve(k, app.session.scenario()) {
        Ok(t) => v["target"] = target_json(l, &t),
        Err(why) => {
            v["target"] = Value::Null;
            v["why"] = json!(why);
        }
    }
    v
}

fn doc_json(l: &Loaded) -> Option<Value> {
    let (path, doc) = l.doc.as_ref()?;
    Some(match doc {
        Err(e) => json!({"path": path.display().to_string(), "error": e}),
        Ok(d) => {
            let r = l.naming.report.clone().unwrap_or_default();
            json!({
                "path": path.display().to_string(),
                "functions": r.functions,
                "globals": d.globals.len(),
                "without_signature": r.without_signature,
                "agree": r.anchors,
                "disagree": r.disagree,
                "named": r.named,
                "unreadable_lines": d.unreadable,
            })
        }
    })
}

fn builtins(l: &Loaded, filter: &str, unnamed_only: bool) -> Reply {
    let want = filter.to_lowercase();
    let (mut by_map, mut by_doc, mut unnamed) = (0, 0, 0);
    let mut rows = Vec::new();
    for i in 0..l.naming.len() {
        let from = l.naming.from(i);
        match from {
            Some(NameFrom::Map) => by_map += 1,
            Some(NameFrom::Doc(_)) => by_doc += 1,
            None => unnamed += 1,
        }
        let f = l
            .catalog
            .function(i as u16)
            .map_err(|e| err(code::FAILED, e.to_string()))?;
        let call = l
            .naming
            .call_form(&l.catalog, i)
            .map_err(|e| err(code::FAILED, e.to_string()))?;
        if (unnamed_only && from.is_some())
            || !(want.is_empty() || call.to_lowercase().contains(&want))
        {
            continue;
        }
        let ty = |t: u16| l.catalog.type_name(t).unwrap_or("?").to_owned();
        rows.push(json!({
            "index": i,
            "name": l.naming.name(i),
            "label": l.naming.label(i),
            "from": match from {
                Some(NameFrom::Map) => json!("map"),
                Some(NameFrom::Doc(_)) => json!("doc"),
                None => Value::Null,
            },
            "returns": ty(f.return_type()),
            "parameters": f.parameters().iter().map(|&p| ty(p)).collect::<Vec<_>>(),
            "own_arguments": f.flags() & 2 != 0,
            "call": call,
            "usage": f.usage(),
        }));
    }
    Ok(json!({
        "builtins": rows,
        "named_by_map": by_map,
        "named_by_doc": by_doc,
        "unnamed": unnamed,
        "doc": doc_json(l),
    }))
}
