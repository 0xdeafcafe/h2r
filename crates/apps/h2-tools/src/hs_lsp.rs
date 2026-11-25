//! `h2 hs lsp`: a language server for a scenario's HaloScript working
//! copy (`.hsc`) on stdin and stdout (docs/editor/rules-editors.md,
//! "HaloScript"). Every answer comes from `hs::ide`, over the context
//! Studio writes beside the file (`h2hs.json`, read again when it
//! changes). Incremental sync in UTF-16 positions, as `h2 rules lsp`.
//! `workspace/executeCommand` runs the commands that ask Studio
//! (`studio_link`). [`Server::handle`] is the whole protocol after
//! `initialize`, so tests drive it without a process.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::SystemTime;

use anyhow::{Result, anyhow};
use hs::ide::{self, Context, Position};
use lsp_server::{Connection, Message, Notification, Request, Response};
use serde_json::{Value, json};

use crate::studio_link::{self, Studio, command};

const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const REQUEST_FAILED: i32 = -32803;

/// Serves until the client shuts the server down.
pub fn run() -> Result<()> {
    let (connection, io) = Connection::stdio();
    let (id, _params) = connection
        .initialize_start()
        .map_err(|e| anyhow!("initialize: {e:?}"))?;
    connection
        .initialize_finish(id, initialize_result())
        .map_err(|e| anyhow!("initialize: {e:?}"))?;
    let mut server = Server::new(Box::new(studio_link::Socket));
    for msg in &connection.receiver {
        if let Message::Request(req) = &msg
            && connection
                .handle_shutdown(req)
                .map_err(|e| anyhow!("shutdown: {e:?}"))?
        {
            break;
        }
        for out in server.handle(msg) {
            connection.sender.send(out).map_err(|e| anyhow!("send: {e}"))?;
        }
    }
    drop(connection);
    io.join().map_err(|e| anyhow!("stdio: {e}"))?;
    Ok(())
}

fn initialize_result() -> Value {
    json!({
        "capabilities": {
            "positionEncoding": "utf-16",
            "textDocumentSync": {"openClose": true, "change": 2},
            "completionProvider": {"triggerCharacters": ["(", "#"], "resolveProvider": false},
            "hoverProvider": true,
            "signatureHelpProvider": {"triggerCharacters": ["(", " "]},
            "definitionProvider": true,
            "referencesProvider": true,
            "documentHighlightProvider": true,
            "documentSymbolProvider": true,
            "documentFormattingProvider": true,
            "codeActionProvider": {"codeActionKinds": ["quickfix", "source"]},
            "renameProvider": {"prepareProvider": true},
            "foldingRangeProvider": true,
            "semanticTokensProvider": {
                "legend": {"tokenTypes": ide::LEGEND, "tokenModifiers": []},
                "full": true,
                "range": false,
            },
            "executeCommandProvider": {"commands": [command::USES, command::SHOW]},
        },
        "serverInfo": {"name": "h2 hs lsp", "version": env!("CARGO_PKG_VERSION")},
    })
}

struct Doc {
    text: String,
    version: Option<i64>,
}

/// A folder's context as last read: the file's modification time and the
/// context, or why it does not read.
struct Loaded {
    seen: Option<SystemTime>,
    context: Result<Context, String>,
}

pub(crate) struct Server {
    docs: HashMap<String, Doc>,
    contexts: HashMap<PathBuf, Loaded>,
    studio: Box<dyn Studio>,
}

/// The empty context, for a document with none.
fn empty() -> &'static Context {
    static EMPTY: std::sync::OnceLock<Context> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Context::default)
}

impl Server {
    pub(crate) fn new(studio: Box<dyn Studio>) -> Self {
        Server {
            docs: HashMap::new(),
            contexts: HashMap::new(),
            studio,
        }
    }

    pub(crate) fn handle(&mut self, msg: Message) -> Vec<Message> {
        match msg {
            Message::Request(req) => vec![Message::Response(self.request(req))],
            Message::Notification(n) => self.notification(n),
            Message::Response(_) => Vec::new(),
        }
    }

    /// Reads the context beside `uri`'s file again when its file changed.
    fn refresh_context(&mut self, uri: &str) -> Option<PathBuf> {
        let dir = studio_link::path_of(uri)?.parent()?.to_path_buf();
        let file = dir.join(ide::CONTEXT_FILE);
        let seen = std::fs::metadata(&file).and_then(|m| m.modified()).ok();
        let stale = self.contexts.get(&dir).is_none_or(|l| l.seen != seen);
        if stale {
            let context = match std::fs::read_to_string(&file) {
                Ok(text) => Context::parse(&text),
                // No file: the empty context (its diagnostics say so).
                Err(_) => Ok(Context::default()),
            };
            self.contexts.insert(dir.clone(), Loaded { seen, context });
        }
        Some(dir)
    }

    /// The context for `uri`, and why it did not read if it did not.
    fn context(&mut self, uri: &str) -> (&Context, Option<String>) {
        match self.refresh_context(uri).and_then(|d| self.contexts.get(&d)) {
            Some(Loaded { context: Ok(c), .. }) => (c, None),
            Some(Loaded { context: Err(e), .. }) => (empty(), Some(e.clone())),
            None => (empty(), None),
        }
    }

    fn notification(&mut self, n: Notification) -> Vec<Message> {
        let method = n.method.as_str();
        if method == "workspace/didChangeWatchedFiles" {
            // A context or working copy changed on disk: check every open
            // document again.
            let uris: Vec<String> = self.docs.keys().cloned().collect();
            return uris.iter().map(|u| self.publish(u)).collect();
        }
        let Some(uri) = n.params.pointer("/textDocument/uri").and_then(Value::as_str).map(str::to_string) else {
            return Vec::new();
        };
        let version = n.params.pointer("/textDocument/version").and_then(Value::as_i64);
        match method {
            "textDocument/didOpen" => {
                let text = n.params.pointer("/textDocument/text").and_then(Value::as_str).unwrap_or("");
                self.docs.insert(uri.clone(), Doc { text: text.to_string(), version });
                vec![self.publish(&uri)]
            }
            "textDocument/didChange" => {
                let doc = self.docs.entry(uri.clone()).or_insert(Doc { text: String::new(), version: None });
                for change in n.params.get("contentChanges").and_then(Value::as_array).into_iter().flatten() {
                    apply_change(&mut doc.text, change);
                }
                doc.version = version.or(doc.version);
                vec![self.publish(&uri)]
            }
            "textDocument/didClose" => {
                self.docs.remove(&uri);
                vec![diagnostics_note(&uri, None, json!([]))]
            }
            _ => Vec::new(),
        }
    }

    fn publish(&mut self, uri: &str) -> Message {
        let (text, version) = self
            .docs
            .get(uri)
            .map_or((String::new(), None), |d| (d.text.clone(), d.version));
        let (ctx, unreadable) = self.context(uri);
        let readable = unreadable.is_none();
        let mut list: Vec<Value> = Vec::new();
        if let Some(why) = unreadable {
            list.push(json!({
                "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
                "severity": ide::severity::ERROR,
                "code": "context.unreadable",
                "source": "h2 hs",
                "message": why,
            }));
        }
        // With the context unreadable, its error stands in for the "no
        // context" note.
        list.extend(
            ide::diagnostics(&text, ctx)
                .iter()
                .filter(|d| readable || d.code != "context.missing")
                .map(|d| json!(d)),
        );
        diagnostics_note(uri, version, Value::Array(list))
    }

    fn request(&mut self, req: Request) -> Response {
        match self.answer(&req) {
            Ok(v) => Response::new_ok(req.id, v),
            Err((code, message)) => Response::new_err(req.id, code, message),
        }
    }

    fn answer(&mut self, req: &Request) -> Result<Value, (i32, String)> {
        let method = req.method.as_str();
        if method == "workspace/executeCommand" {
            return self.execute(&req.params).map_err(|e| (REQUEST_FAILED, e));
        }
        let known = [
            "textDocument/completion",
            "textDocument/hover",
            "textDocument/signatureHelp",
            "textDocument/definition",
            "textDocument/references",
            "textDocument/documentHighlight",
            "textDocument/semanticTokens/full",
            "textDocument/documentSymbol",
            "textDocument/formatting",
            "textDocument/codeAction",
            "textDocument/prepareRename",
            "textDocument/rename",
            "textDocument/foldingRange",
        ];
        if !known.contains(&method) {
            return Err((METHOD_NOT_FOUND, format!("unsupported method {method}")));
        }
        let uri = req
            .params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "no textDocument.uri".to_string()))?
            .to_owned();
        let src = self
            .docs
            .get(&uri)
            .map(|d| d.text.clone())
            .ok_or_else(|| (INVALID_PARAMS, format!("{uri} is not open")))?;
        let src = src.as_str();
        let (ctx, _) = self.context(&uri);
        let pos = || -> Result<Position, (i32, String)> {
            let p = req.params.get("position").cloned().unwrap_or(Value::Null);
            serde_json::from_value(p).map_err(|e| (INVALID_PARAMS, format!("position: {e}")))
        };
        let locations = |list: Vec<ide::Range>| -> Value {
            Value::Array(list.into_iter().map(|r| json!({"uri": uri, "range": r})).collect())
        };
        Ok(match method {
            "textDocument/completion" => json!(ide::complete(src, pos()?, ctx)),
            "textDocument/hover" => json!(ide::hover(src, pos()?, ctx)),
            "textDocument/signatureHelp" => ide::signature_help(src, pos()?, ctx).map_or(Value::Null, |s| {
                json!({
                    "signatures": [{
                        "label": s.signature.label,
                        "documentation": s.signature.documentation.map(|d| json!({"kind": "markdown", "value": d})),
                        "parameters": s.signature.parameters.iter().map(|p| json!({"label": p})).collect::<Vec<_>>(),
                    }],
                    "activeSignature": 0,
                    "activeParameter": s.active_parameter,
                })
            }),
            "textDocument/definition" => locations(ide::definition(src, pos()?, ctx)),
            "textDocument/references" => {
                let declaration = req
                    .params
                    .pointer("/context/includeDeclaration")
                    .and_then(Value::as_bool)
                    .unwrap_or(true);
                locations(ide::references(src, pos()?, ctx, declaration))
            }
            "textDocument/documentHighlight" => json!(ide::highlights(src, pos()?, ctx)),
            "textDocument/semanticTokens/full" => json!({"data": encode_tokens(&ide::semantic_tokens(src, ctx))}),
            "textDocument/documentSymbol" => json!(ide::symbols(src, ctx)),
            "textDocument/foldingRange" => json!(ide::folding_ranges(src)),
            "textDocument/formatting" => match ide::format(src) {
                Ok(text) if text == src => json!([]),
                Ok(text) => json!([{
                    "range": {"start": Position::new(0, 0), "end": ide::position(src, src.len())},
                    "newText": text,
                }]),
                Err(_) => Value::Null,
            },
            "textDocument/codeAction" => {
                let range: ide::Range = serde_json::from_value(req.params.get("range").cloned().unwrap_or(Value::Null))
                    .map_err(|e| (INVALID_PARAMS, format!("range: {e}")))?;
                let mut out: Vec<Value> = ide::actions(src, range, ctx)
                    .into_iter()
                    .map(|a| {
                        json!({
                            "title": a.title,
                            "kind": a.kind,
                            "diagnostics": [a.diagnostic],
                            "edit": {"changes": {uri.clone(): a.edits}},
                        })
                    })
                    .collect();
                // The commands, for an editor that runs them from its code
                // action menu (Zed).
                if let Some((kind, name)) = ide::name_at(src, range.start, ctx) {
                    let arg = json!({"uri": uri, "position": range.start});
                    out.push(json!({
                        "title": format!("Show where the map names {kind} {name} (Studio)"),
                        "kind": "source",
                        "command": {"title": "Show uses in the map", "command": command::USES, "arguments": [arg]},
                    }));
                    if matches!(kind, "script" | "global") {
                        out.push(json!({
                            "title": format!("Show {kind} {name} in Studio"),
                            "kind": "source",
                            "command": {"title": "Show in Studio", "command": command::SHOW, "arguments": [arg]},
                        }));
                    }
                }
                Value::Array(out)
            }
            "textDocument/prepareRename" => json!(ide::prepare_rename(src, pos()?, ctx).map_err(|e| (REQUEST_FAILED, e))?),
            "textDocument/rename" => {
                let to = req
                    .params
                    .get("newName")
                    .and_then(Value::as_str)
                    .ok_or((INVALID_PARAMS, "no newName".to_string()))?;
                let edits = ide::rename(src, pos()?, ctx, to).map_err(|e| (INVALID_PARAMS, e))?;
                json!({"changes": {uri.clone(): edits}})
            }
            _ => unreachable!("checked against known"),
        })
    }

    /// `workspace/executeCommand`.
    fn execute(&mut self, params: &Value) -> Result<Value, String> {
        let name = params.get("command").and_then(Value::as_str).unwrap_or("");
        if name != command::USES && name != command::SHOW {
            return Err(format!("no command {name}"));
        }
        let arg = studio_link::argument(params)?;
        let uri = arg.get("uri").and_then(Value::as_str).ok_or("no uri")?.to_owned();
        let pos: Position = serde_json::from_value(arg.get("position").cloned().unwrap_or(Value::Null))
            .map_err(|e| format!("position: {e}"))?;
        let src = self.docs.get(&uri).map(|d| d.text.clone()).ok_or_else(|| format!("{uri} is not open"))?;
        let (ctx, _) = self.context(&uri);
        let (kind, found) = ide::name_at(&src, pos, ctx).ok_or("no script, global, object or trigger volume here")?;
        let defs: Vec<(String, Value)> = ide::symbols(&src, ctx)
            .into_iter()
            .map(|s| (s.name, json!(s.selection_range)))
            .collect();
        if name == command::USES {
            studio_link::uses(self.studio.as_mut(), kind, &found, &defs)
        } else if matches!(kind, "script" | "global") {
            studio_link::show(self.studio.as_mut(), &found)
        } else {
            Err(format!("Studio shows scripts and globals; {found} is a{} {kind}", if kind == "object" { "n" } else { "" }))
        }
    }
}

fn apply_change(text: &mut String, change: &Value) {
    let Some(new) = change.get("text").and_then(Value::as_str) else {
        return;
    };
    let range = change
        .get("range")
        .and_then(|r| serde_json::from_value::<ide::Range>(r.clone()).ok());
    match range {
        Some(r) => {
            let s = ide::offset(text, r.start);
            let e = ide::offset(text, r.end).max(s);
            text.replace_range(s..e, new);
        }
        None => *text = new.to_string(),
    }
}

fn diagnostics_note(uri: &str, version: Option<i64>, diagnostics: Value) -> Message {
    let mut params = json!({"uri": uri, "diagnostics": diagnostics});
    if let Some(v) = version {
        params["version"] = json!(v);
    }
    Message::Notification(Notification::new("textDocument/publishDiagnostics".to_string(), params))
}

/// LSP's relative encoding: line delta, start delta (on the same line),
/// length, type, modifiers.
fn encode_tokens(tokens: &[ide::Token]) -> Vec<u32> {
    let mut out = Vec::with_capacity(tokens.len() * 5);
    let (mut line, mut start) = (0u32, 0u32);
    for t in tokens {
        let dl = t.line.saturating_sub(line);
        let ds = if dl == 0 { t.start.saturating_sub(start) } else { t.start };
        out.extend([dl, ds, t.len, t.kind.index(), 0]);
        line = t.line;
        start = t.start;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::studio_link::tests::Fake;
    use lsp_server::RequestId;

    /// A folder with a working copy's context: invented builtins and names.
    struct Dir(PathBuf);

    impl Dir {
        fn new(tag: &str) -> Dir {
            let d = std::env::temp_dir().join(format!("h2-hs-lsp-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Dir(d)
        }
        fn uri(&self) -> String {
            format!("file://{}/example.hsc", self.0.display())
        }
        fn write_context(&self, ctx: &Context) {
            std::fs::write(self.0.join(ide::CONTEXT_FILE), ctx.to_json()).unwrap();
        }
    }

    impl Drop for Dir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn ctx() -> Context {
        let mut names = std::collections::BTreeMap::new();
        names.insert("object_name".to_owned(), vec!["door".to_owned()]);
        Context {
            version: ide::CONTEXT_VERSION,
            map: "example".into(),
            types: ["void", "short", "string", "object_name"].map(str::to_owned).to_vec(),
            script_kinds: ["startup", "static"].map(str::to_owned).to_vec(),
            typed_kinds: vec!["static".into()],
            builtins: vec![
                ide::BuiltinInfo {
                    index: 0,
                    name: Some("invented_print".into()),
                    returns: "void".into(),
                    parameters: vec!["string".into()],
                    from: Some("map".into()),
                    ..Default::default()
                },
                ide::BuiltinInfo {
                    index: 1,
                    name: Some("invented_open".into()),
                    returns: "void".into(),
                    parameters: vec!["object_name".into()],
                    from: Some("map".into()),
                    ..Default::default()
                },
            ],
            object_types: vec!["object_name".into()],
            names,
            ..Default::default()
        }
    }

    const SRC: &str = "(script static void helper\n  (invented_open door))\n\n(script startup main\n  (helper))\n";

    fn note(method: &str, params: Value) -> Message {
        Message::Notification(Notification::new(method.to_string(), params))
    }

    fn req(id: i32, method: &str, params: Value) -> Message {
        Message::Request(Request::new(RequestId::from(id), method.to_string(), params))
    }

    fn result(out: Vec<Message>) -> Value {
        match out.into_iter().next() {
            Some(Message::Response(r)) => {
                assert!(r.error.is_none(), "{:?}", r.error);
                r.result.unwrap_or(Value::Null)
            }
            other => panic!("not a response: {other:?}"),
        }
    }

    fn error(out: Vec<Message>) -> lsp_server::ResponseError {
        match out.into_iter().next() {
            Some(Message::Response(r)) => r.error.expect("an error"),
            other => panic!("not a response: {other:?}"),
        }
    }

    fn published(out: Vec<Message>) -> Value {
        match &out[..] {
            [Message::Notification(n)] => {
                assert_eq!(n.method, "textDocument/publishDiagnostics");
                n.params["diagnostics"].clone()
            }
            other => panic!("not one notification: {other:?}"),
        }
    }

    fn open(s: &mut Server, uri: &str, text: &str) -> Value {
        let p = json!({"textDocument": {"uri": uri, "languageId": "haloscript", "version": 1, "text": text}});
        published(s.handle(note("textDocument/didOpen", p)))
    }

    fn at(uri: &str, line: u32, character: u32) -> Value {
        json!({"textDocument": {"uri": uri}, "position": {"line": line, "character": character}})
    }

    #[test]
    fn the_server_checks_against_the_context_beside_the_file_and_rereads_it() {
        let d = Dir::new("context");
        let uri = d.uri();
        let mut s = Server::new(Box::new(Fake::default()));
        // No context yet: the syntax only, and a note saying so.
        let diags = open(&mut s, &uri, SRC);
        assert_eq!(diags.as_array().unwrap().len(), 1, "{diags}");
        assert_eq!(diags[0]["code"], "context.missing");

        d.write_context(&ctx());
        let diags = published(s.handle(note("workspace/didChangeWatchedFiles", json!({"changes": []}))));
        assert_eq!(diags, json!([]), "clean against the context");

        let change = json!({
            "textDocument": {"uri": uri, "version": 2},
            "contentChanges": [{"range": {"start": {"line": 1, "character": 17}, "end": {"line": 1, "character": 21}}, "text": "window"}],
        });
        let diags = published(s.handle(note("textDocument/didChange", change)));
        assert_eq!(diags[0]["code"], "name.unknown", "{diags}");
        assert_eq!(diags[0]["range"]["start"], json!({"line": 1, "character": 17}));

        // A context of another format is named.
        std::fs::write(d.0.join(ide::CONTEXT_FILE), "{\"version\": 7}").unwrap();
        // The modification time may not move within the clock's step.
        s.contexts.clear();
        let diags = published(s.handle(note("workspace/didChangeWatchedFiles", json!({"changes": []}))));
        assert_eq!(diags[0]["code"], "context.unreadable", "{diags}");
        assert!(!diags.as_array().unwrap().iter().any(|x| x["code"] == "context.missing"));
    }

    #[test]
    fn requests_answer_from_the_ide_module() {
        let d = Dir::new("requests");
        d.write_context(&ctx());
        let uri = d.uri();
        let mut s = Server::new(Box::new(Fake::default()));
        assert_eq!(open(&mut s, &uri, SRC), json!([]));

        let v = result(s.handle(req(1, "textDocument/hover", at(&uri, 1, 5))));
        assert!(v["contents"]["value"].as_str().unwrap().contains("(invented_open <object_name>)"));
        let v = result(s.handle(req(2, "textDocument/definition", at(&uri, 4, 4))));
        assert_eq!(v[0]["range"]["start"], json!({"line": 0, "character": 20}));
        assert_eq!(v[0]["uri"], uri.as_str());
        let mut refs = at(&uri, 4, 4);
        refs["context"] = json!({"includeDeclaration": false});
        assert_eq!(result(s.handle(req(3, "textDocument/references", refs))).as_array().unwrap().len(), 1);
        let v = result(s.handle(req(4, "textDocument/completion", at(&uri, 4, 3))));
        assert!(v.as_array().unwrap().iter().any(|i| i["label"] == "helper"), "{v}");
        let v = result(s.handle(req(5, "textDocument/documentSymbol", json!({"textDocument": {"uri": uri}}))));
        assert_eq!(v[1]["name"], "main");
        let v = result(s.handle(req(6, "textDocument/semanticTokens/full", json!({"textDocument": {"uri": uri}}))));
        let data: Vec<u64> = v["data"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect();
        // `script` at 0:1, a keyword; `static` five on, a keyword too.
        assert_eq!(&data[..10], &[0, 1, 6, 0, 0, 0, 7, 6, 0, 0]);
        let v = result(s.handle(req(7, "textDocument/formatting", json!({"textDocument": {"uri": uri}}))));
        assert_eq!(v, json!([]), "already formatted");
        let mut sig = at(&uri, 1, 17);
        sig["context"] = json!({});
        let v = result(s.handle(req(8, "textDocument/signatureHelp", sig)));
        assert_eq!(v["signatures"][0]["label"], "(invented_open <object_name>)");
        let mut ren = at(&uri, 4, 4);
        ren["newName"] = json!("helper_two");
        let v = result(s.handle(req(9, "textDocument/rename", ren)));
        assert_eq!(v["changes"][uri.as_str()].as_array().unwrap().len(), 2);
        let e = error(s.handle(req(10, "textDocument/prepareRename", at(&uri, 1, 5))));
        assert_eq!(e.code, REQUEST_FAILED);
        let e = error(s.handle(req(11, "textDocument/inlayHint", at(&uri, 0, 0))));
        assert_eq!(e.code, METHOD_NOT_FOUND);
    }

    #[test]
    fn code_actions_offer_the_fix_and_the_studio_commands() {
        let d = Dir::new("actions");
        d.write_context(&ctx());
        let uri = d.uri();
        let mut s = Server::new(Box::new(Fake::default()));
        open(&mut s, &uri, "(script startup main\n  (invented_prnt \"hi\"))\n");
        let p = json!({
            "textDocument": {"uri": uri},
            "range": {"start": {"line": 1, "character": 4}, "end": {"line": 1, "character": 4}},
            "context": {"diagnostics": []},
        });
        let v = result(s.handle(req(1, "textDocument/codeAction", p)));
        assert_eq!(v[0]["title"], "Change to `invented_print`");
        assert_eq!(v[0]["edit"]["changes"][uri.as_str()][0]["newText"], "invented_print");
        // Off a name: the definition around it, as a script.
        assert_eq!(v[1]["command"]["command"], command::USES);
        assert_eq!(v[2]["command"]["command"], command::SHOW);
        assert!(v[2]["title"].as_str().unwrap().contains("script main"));
    }

    #[test]
    fn commands_ask_studio_for_uses_and_to_show_a_script() {
        let d = Dir::new("commands");
        d.write_context(&ctx());
        let uri = d.uri();
        let mut fake = Fake::default();
        fake.answers = vec![
            ("script.uses", Ok(json!({"uses": [{"name": "main", "item": "script", "count": 1}]}))),
            ("script.source", Ok(json!({"item": "script"}))),
        ];
        let mut s = Server::new(Box::new(fake));
        open(&mut s, &uri, SRC);
        let run = |s: &mut Server, id: i32, command: &str, line: u32, character: u32| {
            s.handle(req(
                id,
                "workspace/executeCommand",
                json!({"command": command, "arguments": [{"uri": uri, "position": {"line": line, "character": character}}]}),
            ))
        };
        let v = result(run(&mut s, 1, command::USES, 4, 4));
        assert_eq!(v["kind"], "script");
        assert_eq!(v["name"], "helper");
        assert_eq!(v["uses"][0]["range"]["start"], json!({"line": 3, "character": 16}));
        let v = result(run(&mut s, 2, command::SHOW, 4, 4));
        assert_eq!(v["message"], "Studio shows script helper");
        // An object is shown by uses, not in the Scripts tab.
        let e = error(run(&mut s, 3, command::SHOW, 1, 19));
        assert!(e.message.contains("an object"), "{}", e.message);
        let e = error(s.handle(req(4, "workspace/executeCommand", json!({"command": "h2.nothing"}))));
        assert_eq!(e.code, REQUEST_FAILED);
    }

    #[test]
    fn initialize_declares_the_legend_and_the_commands() {
        let r = initialize_result();
        let c = &r["capabilities"];
        assert_eq!(c["semanticTokensProvider"]["legend"]["tokenTypes"].as_array().unwrap().len(), ide::LEGEND.len());
        assert_eq!(c["executeCommandProvider"]["commands"], json!([command::USES, command::SHOW]));
        assert_eq!(c["textDocumentSync"]["change"], 2);
        assert_eq!(r["serverInfo"]["name"], "h2 hs lsp");
    }
}
