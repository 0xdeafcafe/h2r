//! `h2 rules lsp`: a language server for `.rules` files on stdin and
//! stdout (docs/game/rules-engine.md, "Security and usability"; the editor extensions in
//! docs/editor/rules-editors.md). Incremental document sync in UTF-16
//! positions; every answer comes from `rules::ide`, as Live's
//! `POST /v1/rules/ide` does. `workspace/executeCommand` runs the
//! commands that ask Studio: reload the file into the paired game, and
//! run it over a scenario (`studio_link`). [`Server::handle`] is the
//! whole protocol after `initialize`, so tests drive it without a process.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    CodeActionRequest, Completion, DocumentHighlightRequest, DocumentSymbolRequest, FoldingRangeRequest, Formatting,
    GotoDefinition, HoverRequest, PrepareRenameRequest, RangeFormatting, References, Rename, Request as _,
    SemanticTokensFullRequest, SignatureHelpRequest,
};
use rules::ide::{self, Position};
use serde_json::{Value, json};

use crate::studio_link::{self, Studio, command};

/// JSON-RPC's and LSP's error codes.
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;
const REQUEST_FAILED: i32 = -32803;

/// Where a diagnostic's code is explained: each diagnostic's
/// `codeDescription`.
const CODES_DOC: &str =
    "https://github.com/0xdeafcafe/h2recomp/blob/master/docs/game/rules-language-types.md#diagnostics-and-the-editor";

/// The words after which a typed space asks for completions: what comes
/// next is a label, an event, a game type, a capability, a set, a palette,
/// a built-in game or a Quick Options mark.
const SPACE_TRIGGER_WORDS: &[&str] = &["is", "on", "type", "requires", "each", "palette", "uses", "quick"];

/// Serves until the client shuts the server down.
pub fn run() -> Result<()> {
    let (connection, io) = Connection::stdio();
    let (id, _params) = connection
        .initialize_start()
        .map_err(|e| anyhow!("initialize: {e:?}"))?;
    connection
        .initialize_finish(id, initialize_result())
        .map_err(|e| anyhow!("initialize: {e:?}"))?;
    let mut server = Server::default();
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

/// The answer to `initialize`: the capabilities and the server's name.
fn initialize_result() -> Value {
    json!({
        "capabilities": capabilities(),
        "serverInfo": {"name": "h2 rules lsp", "version": env!("CARGO_PKG_VERSION")},
    })
}

/// What the server offers.
fn capabilities() -> Value {
    json!({
        "positionEncoding": "utf-16",
        "textDocumentSync": {
            "openClose": true,
            // Incremental: each change carries a range and its new text.
            "change": 2,
        },
        "completionProvider": {
            "triggerCharacters": [".", ":", "\"", "(", ",", " "],
            "resolveProvider": false,
        },
        "hoverProvider": true,
        "signatureHelpProvider": {"triggerCharacters": ["(", ","], "retriggerCharacters": [","]},
        "definitionProvider": true,
        "referencesProvider": true,
        "documentHighlightProvider": true,
        "documentSymbolProvider": true,
        "documentFormattingProvider": true,
        "documentRangeFormattingProvider": true,
        "codeActionProvider": {"codeActionKinds": ["quickfix", "source"]},
        "renameProvider": {"prepareProvider": true},
        "foldingRangeProvider": true,
        "semanticTokensProvider": {
            "legend": {"tokenTypes": ide::LEGEND, "tokenModifiers": []},
            "full": true,
            "range": false,
        },
        "executeCommandProvider": {"commands": [command::RELOAD, command::RUN_SCENARIO]},
    })
}

/// An open document.
struct Doc {
    text: String,
    version: Option<i64>,
}

/// The open documents, by URI, and the line to Studio.
pub(crate) struct Server {
    docs: HashMap<String, Doc>,
    studio: Box<dyn Studio>,
}

impl Default for Server {
    fn default() -> Self {
        Server {
            docs: HashMap::new(),
            studio: Box::new(studio_link::Socket),
        }
    }
}

impl Server {
    /// One message in, the messages to send back out.
    pub(crate) fn handle(&mut self, msg: Message) -> Vec<Message> {
        match msg {
            Message::Request(req) => vec![Message::Response(self.request(req))],
            Message::Notification(n) => self.notification(n),
            Message::Response(_) => Vec::new(),
        }
    }

    fn notification(&mut self, n: Notification) -> Vec<Message> {
        let Some(uri) = n
            .params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            return Vec::new();
        };
        let version = n.params.pointer("/textDocument/version").and_then(Value::as_i64);
        let method = n.method.as_str();
        if method == DidOpenTextDocument::METHOD {
            let text = n
                .params
                .pointer("/textDocument/text")
                .and_then(Value::as_str)
                .unwrap_or("");
            self.docs.insert(
                uri.clone(),
                Doc {
                    text: text.to_string(),
                    version,
                },
            );
            vec![self.publish(&uri)]
        } else if method == DidChangeTextDocument::METHOD {
            let doc = self.docs.entry(uri.clone()).or_insert(Doc {
                text: String::new(),
                version: None,
            });
            // In order: each change's range is in the text the one before
            // left.
            for change in n.params.get("contentChanges").and_then(Value::as_array).into_iter().flatten() {
                apply_change(&mut doc.text, change);
            }
            doc.version = version.or(doc.version);
            vec![self.publish(&uri)]
        } else if method == DidCloseTextDocument::METHOD {
            self.docs.remove(&uri);
            vec![diagnostics_note(&uri, None, json!([]))]
        } else {
            Vec::new()
        }
    }

    fn publish(&self, uri: &str) -> Message {
        let (src, version) = self
            .docs
            .get(uri)
            .map_or(("", None), |d| (d.text.as_str(), d.version));
        // A script's `use` files are the ones beside it on disk, as `h2
        // rules check` reads them; a module's unsaved edits are not seen.
        let found = match studio_link::path_of(uri) {
            Some(path) => {
                let label = path.display().to_string();
                let root = path.file_name().map_or_else(|| label.clone(), |n| n.to_string_lossy().into_owned());
                ide::diagnostics_with_files(src, &root, &crate::rules_cmd::module_reader(&label))
            }
            None => ide::diagnostics(src),
        };
        let list: Vec<Value> = found.iter().map(diagnostic_json).collect();
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
            Completion::METHOD,
            HoverRequest::METHOD,
            SignatureHelpRequest::METHOD,
            GotoDefinition::METHOD,
            References::METHOD,
            DocumentHighlightRequest::METHOD,
            SemanticTokensFullRequest::METHOD,
            DocumentSymbolRequest::METHOD,
            Formatting::METHOD,
            RangeFormatting::METHOD,
            CodeActionRequest::METHOD,
            PrepareRenameRequest::METHOD,
            Rename::METHOD,
            FoldingRangeRequest::METHOD,
        ];
        if !known.contains(&method) {
            return Err((METHOD_NOT_FOUND, format!("unsupported method {method}")));
        }
        let uri = req
            .params
            .pointer("/textDocument/uri")
            .and_then(Value::as_str)
            .ok_or((INVALID_PARAMS, "no textDocument.uri".to_string()))?;
        let src = self
            .docs
            .get(uri)
            .map(|d| d.text.as_str())
            .ok_or_else(|| (INVALID_PARAMS, format!("{uri} is not open")))?;
        let pos = || -> Result<Position, (i32, String)> {
            let p = req.params.get("position").cloned().unwrap_or(Value::Null);
            serde_json::from_value(p).map_err(|e| (INVALID_PARAMS, format!("position: {e}")))
        };
        let range = || -> Result<ide::Range, (i32, String)> {
            let r = req.params.get("range").cloned().unwrap_or(Value::Null);
            serde_json::from_value(r).map_err(|e| (INVALID_PARAMS, format!("range: {e}")))
        };
        let locations = |list: Vec<ide::Location>| -> Value {
            Value::Array(
                list.into_iter()
                    .map(|l| json!({"uri": uri, "range": l.range}))
                    .collect(),
            )
        };
        Ok(if method == Completion::METHOD {
            let pos = pos()?;
            if space_typed(&req.params) && !space_wants_completion(src, pos) {
                json!([])
            } else {
                json!(ide::complete(src, pos))
            }
        } else if method == HoverRequest::METHOD {
            json!(ide::hover(src, pos()?))
        } else if method == SignatureHelpRequest::METHOD {
            ide::signature_help(src, pos()?).map_or(Value::Null, |s| signature_json(&s))
        } else if method == GotoDefinition::METHOD {
            locations(ide::definition(src, pos()?))
        } else if method == References::METHOD {
            let declaration = req
                .params
                .pointer("/context/includeDeclaration")
                .and_then(Value::as_bool)
                .unwrap_or(true);
            let mut list = ide::references(src, pos()?);
            // The declaration comes first.
            if !declaration && !list.is_empty() {
                list.remove(0);
            }
            locations(list)
        } else if method == DocumentHighlightRequest::METHOD {
            json!(ide::highlights(src, pos()?))
        } else if method == SemanticTokensFullRequest::METHOD {
            json!({ "data": encode_tokens(&ide::semantic_tokens(src)) })
        } else if method == DocumentSymbolRequest::METHOD {
            json!(ide::symbols(src))
        } else if method == FoldingRangeRequest::METHOD {
            json!(ide::folding_ranges(src))
        } else if method == CodeActionRequest::METHOD {
            let mut out: Vec<Value> = ide::actions(src, range()?)
                .into_iter()
                .map(|a| {
                    json!({
                        "title": a.title,
                        "kind": a.kind,
                        "diagnostics": [diagnostic_json(&a.diagnostic)],
                        "edit": {"changes": {uri: a.edits}},
                    })
                })
                .collect();
            // The commands, for an editor that runs them from its code
            // action menu (Zed).
            out.extend(studio_actions(uri));
            Value::Array(out)
        } else if method == PrepareRenameRequest::METHOD {
            let p = ide::prepare_rename(src, pos()?).map_err(|e| (REQUEST_FAILED, e))?;
            json!(p)
        } else if method == Rename::METHOD {
            let name = req
                .params
                .get("newName")
                .and_then(Value::as_str)
                .ok_or((INVALID_PARAMS, "no newName".to_string()))?;
            let edits = ide::rename(src, pos()?, name).map_err(|e| (INVALID_PARAMS, e))?;
            json!({"changes": {uri: edits}})
        } else if method == RangeFormatting::METHOD {
            match ide::format_range(src, range()?) {
                Ok(edits) => json!(edits),
                // Source that does not parse is left alone; its errors are
                // in the diagnostics.
                Err(_) => Value::Null,
            }
        } else {
            match ide::format(src) {
                Ok(text) if text == src => json!([]),
                Ok(text) => json!([{
                    "range": {"start": Position::new(0, 0), "end": end_of(src)},
                    "newText": text,
                }]),
                Err(_) => Value::Null,
            }
        })
    }
}

impl Server {
    /// `workspace/executeCommand`: the commands that ask Studio.
    fn execute(&mut self, params: &Value) -> Result<Value, String> {
        let name = params.get("command").and_then(Value::as_str).unwrap_or("");
        let arg = studio_link::argument(params)?;
        match name {
            command::RELOAD => studio_link::reload(self.studio.as_mut(), arg),
            command::RUN_SCENARIO => {
                // The text as it is in the editor, saved or not.
                let mut arg = arg.clone();
                if arg.get("source").is_none()
                    && let Some(doc) = arg.get("uri").and_then(Value::as_str).and_then(|u| self.docs.get(u))
                {
                    arg["source"] = json!(doc.text);
                }
                studio_link::run_scenario(self.studio.as_mut(), &arg)
            }
            other => Err(format!("no command {other}")),
        }
    }
}

/// The Studio commands as code actions: reload the file into the game,
/// and run it over each scenario beside it whose name starts with the
/// script's (`ctf.rules` and `ctf_capture.json`).
fn studio_actions(uri: &str) -> Vec<Value> {
    let Some(path) = studio_link::path_of(uri) else {
        return Vec::new();
    };
    let mut out = vec![json!({
        "title": "Reload in the game (Studio)",
        "kind": "source",
        "command": {"title": "Reload in the game", "command": command::RELOAD, "arguments": [{"uri": uri}]},
    })];
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut scenarios: Vec<std::path::PathBuf> = path
        .parent()
        .and_then(|d| std::fs::read_dir(d).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| {
            p.extension().is_some_and(|x| x == "json")
                && p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(&stem))
        })
        .collect();
    scenarios.sort();
    for s in scenarios {
        let file = s.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        out.push(json!({
            "title": format!("Run scenario {file} (Studio)"),
            "kind": "source",
            "command": {
                "title": "Run scenario",
                "command": command::RUN_SCENARIO,
                "arguments": [{"uri": uri, "scenario": s.display().to_string()}],
            },
        }));
    }
    out
}

/// Makes one `contentChanges` entry: a range and its new text, or the
/// whole text when it has no range. Positions are UTF-16, as declared.
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

/// Whether a completion request comes from typing a space (LSP's
/// `CompletionTriggerKind.TriggerCharacter` with `" "`).
fn space_typed(params: &Value) -> bool {
    params.pointer("/context/triggerKind").and_then(Value::as_u64) == Some(2)
        && params.pointer("/context/triggerCharacter").and_then(Value::as_str) == Some(" ")
}

/// Whether the space before `pos` follows a word of
/// [`SPACE_TRIGGER_WORDS`], a `:` (a type comes next) or a `,` (the next
/// argument or capability). Anywhere else a space asks for nothing, so
/// typing does not open the list at every word.
fn space_wants_completion(src: &str, pos: Position) -> bool {
    let off = ide::offset(src, pos);
    let line_start = src[..off].rfind('\n').map_or(0, |i| i + 1);
    let before = src[line_start..off].trim_end();
    if before.ends_with(':') || before.ends_with(',') {
        return true;
    }
    let word_start = before
        .char_indices()
        .rev()
        .take_while(|&(_, c)| c.is_ascii_alphanumeric() || c == '_')
        .last()
        .map_or(before.len(), |(i, _)| i);
    SPACE_TRIGGER_WORDS.contains(&&before[word_start..])
}

/// A diagnostic as LSP's: `ide::Diagnostic`'s fields, with the page that
/// explains its code. The fixes stay out; `codeAction` answers them.
fn diagnostic_json(d: &ide::Diagnostic) -> Value {
    json!({
        "range": d.range,
        "severity": d.severity,
        "message": d.message,
        "code": d.code,
        "codeDescription": {"href": CODES_DOC},
        "source": d.source,
    })
}

fn diagnostics_note(uri: &str, version: Option<i64>, diagnostics: Value) -> Message {
    let mut params = json!({"uri": uri, "diagnostics": diagnostics});
    if let Some(v) = version {
        params["version"] = json!(v);
    }
    Message::Notification(Notification::new(PublishDiagnostics::METHOD.to_string(), params))
}

/// Signature help with each parameter's label as offsets into the
/// signature's, as LSP allows.
fn signature_json(s: &ide::SignatureHelp) -> Value {
    let signatures: Vec<Value> = s
        .signatures
        .iter()
        .map(|g| {
            let parameters: Vec<Value> = g.parameters.iter().map(|p| json!({"label": p.offsets})).collect();
            json!({"label": g.label, "documentation": g.documentation, "parameters": parameters})
        })
        .collect();
    json!({
        "signatures": signatures,
        "activeSignature": s.active_signature,
        "activeParameter": s.active_parameter,
    })
}

/// LSP's relative encoding: line delta, start delta (on the same line),
/// length, type, modifiers.
fn encode_tokens(tokens: &[ide::Token]) -> Vec<u32> {
    let mut out = Vec::with_capacity(tokens.len() * 5);
    let (mut line, mut start) = (0u32, 0u32);
    for t in tokens {
        let dl = t.line.saturating_sub(line);
        let ds = if dl == 0 {
            t.start.saturating_sub(start)
        } else {
            t.start
        };
        out.extend([dl, ds, t.len, t.kind.index(), 0]);
        line = t.line;
        start = t.start;
    }
    out
}

/// The position after the last character.
fn end_of(src: &str) -> Position {
    let line = src.matches('\n').count() as u32;
    let last = src.rsplit('\n').next().unwrap_or("");
    Position::new(line, last.encode_utf16().count() as u32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_server::RequestId;

    const URI: &str = "file:///t.rules";
    const SRC: &str = "rules \"T\" 1\ntype slayer\non kill(k, v) {\n    score(k, 1)\n}\n";

    fn note(method: &str, params: Value) -> Message {
        Message::Notification(Notification::new(method.to_string(), params))
    }

    fn req(id: i32, method: &str, params: Value) -> Message {
        Message::Request(Request::new(RequestId::from(id), method.to_string(), params))
    }

    fn at(line: u32, character: u32) -> Value {
        json!({"textDocument": {"uri": URI}, "position": {"line": line, "character": character}})
    }

    fn doc() -> Value {
        json!({"textDocument": {"uri": URI}})
    }

    fn open(s: &mut Server, text: &str) -> Value {
        let open = json!({"textDocument": {"uri": URI, "languageId": "rules", "version": 1, "text": text}});
        published(s.handle(note("textDocument/didOpen", open)))
    }

    fn response(out: Vec<Message>) -> Response {
        match out.into_iter().next() {
            Some(Message::Response(r)) => r,
            other => panic!("not a response: {other:?}"),
        }
    }

    fn result(out: Vec<Message>) -> Value {
        let r = response(out);
        assert!(r.error.is_none(), "{:?}", r.error);
        r.result.unwrap_or(Value::Null)
    }

    fn published(out: Vec<Message>) -> Value {
        match &out[..] {
            [Message::Notification(n)] => {
                assert_eq!(n.method, "textDocument/publishDiagnostics");
                assert_eq!(n.params["uri"], URI);
                n.params["diagnostics"].clone()
            }
            other => panic!("not one notification: {other:?}"),
        }
    }

    #[test]
    fn the_handler_publishes_diagnostics_and_answers_from_the_ide_module() {
        let mut s = Server::default();
        assert_eq!(open(&mut s, SRC), json!([]));

        let v = result(s.handle(req(1, "textDocument/hover", at(3, 5))));
        assert!(v["contents"]["value"].as_str().unwrap().contains("score(player, int)"));
        let v = result(s.handle(req(2, "textDocument/definition", at(3, 10))));
        assert_eq!(v[0]["uri"], URI);
        assert_eq!(v[0]["range"]["start"], json!({"line": 2, "character": 8}));
        let mut refs = at(3, 10);
        refs["context"] = json!({"includeDeclaration": false});
        let v = result(s.handle(req(3, "textDocument/references", refs)));
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["range"]["start"], json!({"line": 3, "character": 10}));
        let v = result(s.handle(req(4, "textDocument/signatureHelp", at(3, 13))));
        assert_eq!(v["activeParameter"], 1);
        assert_eq!(v["signatures"][0]["parameters"][1]["label"], json!([14, 17]));
        let v = result(s.handle(req(5, "textDocument/completion", at(3, 4))));
        assert!(v.as_array().unwrap().iter().any(|i| i["label"] == "score"));
        let v = result(s.handle(req(6, "textDocument/documentSymbol", doc())));
        assert_eq!(v[1]["name"], "on kill");
        let v = result(s.handle(req(7, "textDocument/semanticTokens/full", doc())));
        let data: Vec<u64> = v["data"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect();
        assert_eq!(data.len() % 5, 0);
        // `rules`, then `"T"` four columns on, as a keyword and a string.
        assert_eq!(&data[..10], &[0, 0, 5, 0, 0, 0, 6, 3, 10, 0]);
        let v = result(s.handle(req(8, "textDocument/formatting", doc())));
        assert_eq!(v, json!([]));

        // A change replaces the text and publishes again.
        let messy = "rules \"T\" 1\ntype slayer\non kill(v,k){score(k, )}\n";
        let change = json!({"textDocument": {"uri": URI, "version": 2}, "contentChanges": [{"text": messy}]});
        let d = published(s.handle(note("textDocument/didChange", change)));
        assert_eq!(d[0]["severity"], 1);
        assert_eq!(d[0]["codeDescription"]["href"], CODES_DOC);
        assert!(d[0].get("fixes").is_none(), "{d}");
        let v = result(s.handle(req(9, "textDocument/formatting", doc())));
        assert_eq!(v[0]["newText"], "rules \"T\" 1\ntype slayer\non kill(v, k) {\n    score(k,)\n}\n");
        assert_eq!(v[0]["range"]["end"], json!({"line": 3, "character": 0}));

        // An unknown method; a closed document clears its diagnostics and
        // is no longer answered.
        let r = response(s.handle(req(10, "textDocument/inlayHint", at(0, 0))));
        assert_eq!(r.error.unwrap().code, METHOD_NOT_FOUND);
        let close = json!({"textDocument": {"uri": URI}});
        assert_eq!(published(s.handle(note("textDocument/didClose", close))), json!([]));
        let r = response(s.handle(req(11, "textDocument/hover", at(3, 5))));
        assert_eq!(r.error.unwrap().code, INVALID_PARAMS);
    }

    #[test]
    fn a_use_reads_the_file_beside_the_document() {
        let dir = std::env::temp_dir().join(format!("h2-rules-lsp-use-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("waves.rules"), "const base = 2\nfn bonus() -> int = nope + 1\n").unwrap();
        let uri = format!("file://{}/main.rules", dir.display());
        let main = "rules \"T\" 1\ntype slayer\nuse \"waves.rules\"\non kill(v, k) { score(k, bonus()) }\n";
        let mut s = Server::default();
        let open = json!({"textDocument": {"uri": uri, "languageId": "rules", "version": 1, "text": main}});
        let out = s.handle(note("textDocument/didOpen", open));
        let [Message::Notification(n)] = &out[..] else { panic!("{out:?}") };
        let all = &n.params["diagnostics"];
        let d = all.as_array().unwrap().iter().find(|x| x["severity"] == 1).expect(&all.to_string());
        assert_eq!(d["range"]["start"]["line"], 2);
        assert!(d["message"].as_str().unwrap().starts_with("waves.rules:2:"), "{d}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn code_actions_and_rename_answer_with_workspace_edits() {
        let mut s = Server::default();
        let src = "rules \"T\" 1\ntype slayer\nlabel barrier: objects\non tick { for each barier b { } }\n";
        let d = open(&mut s, src);
        assert!(d.as_array().unwrap().iter().any(|x| x["code"] == "label.unknown"), "{d}");

        let mut params = json!({"textDocument": {"uri": URI}, "context": {"diagnostics": []}});
        params["range"] = json!({"start": {"line": 3, "character": 21}, "end": {"line": 3, "character": 21}});
        let v = result(s.handle(req(1, "textDocument/codeAction", params)));
        assert_eq!(v[0]["title"], "Change to `barrier`");
        assert_eq!(v[0]["kind"], "quickfix");
        assert_eq!(v[0]["diagnostics"][0]["code"], "label.unknown");
        let edit = &v[0]["edit"]["changes"][URI][0];
        assert_eq!(edit["newText"], "barrier");
        assert_eq!(edit["range"]["start"], json!({"line": 3, "character": 19}));
        let r = response(s.handle(req(2, "textDocument/codeAction", doc())));
        assert_eq!(r.error.unwrap().code, INVALID_PARAMS);

        let change = json!({"textDocument": {"uri": URI, "version": 2}, "contentChanges": [{"text": SRC}]});
        published(s.handle(note("textDocument/didChange", change)));
        let mut params = at(3, 10);
        params["newName"] = json!("killer");
        let v = result(s.handle(req(3, "textDocument/rename", params)));
        let edits = v["changes"][URI].as_array().unwrap();
        assert_eq!(edits.len(), 2, "{v}");
        assert!(edits.iter().all(|e| e["newText"] == "killer"));
        let mut params = at(3, 10);
        params["newName"] = json!("on");
        let r = response(s.handle(req(4, "textDocument/rename", params)));
        assert_eq!(r.error.unwrap().code, INVALID_PARAMS);

        // prepareRename names the range and the name, or fails with why.
        let v = result(s.handle(req(5, "textDocument/prepareRename", at(3, 10))));
        assert_eq!(v["placeholder"], "k");
        assert_eq!(v["range"]["start"], json!({"line": 3, "character": 10}));
        assert_eq!(v["range"]["end"], json!({"line": 3, "character": 11}));
        let r = response(s.handle(req(6, "textDocument/prepareRename", at(0, 1))));
        let e = r.error.unwrap();
        assert_eq!(e.code, REQUEST_FAILED);
        assert!(e.message.contains("no name"), "{}", e.message);
    }

    #[test]
    fn incremental_changes_apply_utf16_ranges_in_order() {
        let mut s = Server::default();
        open(&mut s, "rules \"T\" 1\n// \u{1F600} x\n");
        // The emoji is two UTF-16 units: `x` is at character 6.
        let change = json!({
            "textDocument": {"uri": URI, "version": 2},
            "contentChanges": [
                {"range": {"start": {"line": 1, "character": 6}, "end": {"line": 1, "character": 7}}, "text": "y"},
                {"range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 0}}, "text": "type slayer\n"},
                {"range": {"start": {"line": 0, "character": 7}, "end": {"line": 0, "character": 8}}, "text": "Tag"},
            ],
        });
        let out = s.handle(note("textDocument/didChange", change));
        match &out[..] {
            [Message::Notification(n)] => assert_eq!(n.params["version"], 2),
            other => panic!("not one notification: {other:?}"),
        }
        assert_eq!(s.docs[URI].text, "rules \"Tag\" 1\n// \u{1F600} y\ntype slayer\n");
        // A change with no range is the whole text.
        let change = json!({"textDocument": {"uri": URI, "version": 3}, "contentChanges": [{"text": SRC}]});
        published(s.handle(note("textDocument/didChange", change)));
        assert_eq!(s.docs[URI].text, SRC);
        assert_eq!(s.docs[URI].version, Some(3));
    }

    #[test]
    fn highlights_folding_and_range_formatting_answer() {
        let mut s = Server::default();
        let src = "rules \"T\" 1\ntype slayer\n// one\n// two\non kill(k, v) {\n    score(k, 1)\n    score(v,  1)\n}\n";
        open(&mut s, src);

        let v = result(s.handle(req(1, "textDocument/documentHighlight", at(5, 10))));
        let marks: Vec<(u64, u64)> = v
            .as_array()
            .unwrap()
            .iter()
            .map(|h| (h["range"]["start"]["line"].as_u64().unwrap(), h["kind"].as_u64().unwrap()))
            .collect();
        assert_eq!(marks, [(4, ide::highlight_kind::WRITE as u64), (5, ide::highlight_kind::READ as u64)]);

        let v = result(s.handle(req(2, "textDocument/foldingRange", doc())));
        assert_eq!(
            v,
            json!([
                {"startLine": 2, "endLine": 3, "kind": "comment"},
                {"startLine": 4, "endLine": 6},
            ])
        );

        let mut params = doc();
        params["range"] = json!({"start": {"line": 6, "character": 0}, "end": {"line": 6, "character": 4}});
        params["options"] = json!({"tabSize": 4, "insertSpaces": true});
        let v = result(s.handle(req(3, "textDocument/rangeFormatting", params)));
        assert_eq!(v.as_array().unwrap().len(), 1, "{v}");
        assert_eq!(v[0]["newText"], "    score(v, 1)\n");
        assert_eq!(v[0]["range"]["start"], json!({"line": 6, "character": 0}));
        assert_eq!(v[0]["range"]["end"], json!({"line": 7, "character": 0}));
    }

    #[test]
    fn a_typed_space_completes_only_where_a_word_wants_one() {
        let mut s = Server::default();
        let src = "rules \"T\" 1\ntype \nlabel goal: zone\nlabel ball: objects\non enter(goal, o) {\n    if o is \n    \n}\n";
        open(&mut s, src);
        let typed = |line: u32, character: u32| {
            let mut p = at(line, character);
            p["context"] = json!({"triggerKind": 2, "triggerCharacter": " "});
            p
        };
        let v = result(s.handle(req(1, "textDocument/completion", typed(1, 5))));
        assert!(v.as_array().unwrap().iter().any(|i| i["label"] == "slayer"), "{v}");
        // `o` is an object: the object label fits, the zone label does not.
        let v = result(s.handle(req(2, "textDocument/completion", typed(5, 12))));
        assert!(v.as_array().unwrap().iter().any(|i| i["label"] == "ball"), "{v}");
        // A space at the start of a statement asks for nothing; asked for
        // (Ctrl+Space), the same place completes.
        let v = result(s.handle(req(3, "textDocument/completion", typed(6, 4))));
        assert_eq!(v, json!([]));
        let v = result(s.handle(req(4, "textDocument/completion", at(6, 4))));
        assert!(!v.as_array().unwrap().is_empty());
    }

    #[test]
    fn initialize_declares_incremental_sync_and_every_provider() {
        let r = initialize_result();
        let c = &r["capabilities"];
        assert_eq!(c["textDocumentSync"]["change"], 2);
        assert_eq!(c["positionEncoding"], "utf-16");
        assert_eq!(c["renameProvider"]["prepareProvider"], true);
        for p in [
            "hoverProvider",
            "definitionProvider",
            "referencesProvider",
            "documentHighlightProvider",
            "documentSymbolProvider",
            "documentFormattingProvider",
            "documentRangeFormattingProvider",
            "foldingRangeProvider",
        ] {
            assert_eq!(c[p], true, "{p}");
        }
        let triggers = c["completionProvider"]["triggerCharacters"].as_array().unwrap();
        assert!(triggers.contains(&json!(".")) && triggers.contains(&json!(" ")));
        assert_eq!(c["semanticTokensProvider"]["legend"]["tokenTypes"].as_array().unwrap().len(), ide::LEGEND.len());
        assert_eq!(r["serverInfo"]["name"], "h2 rules lsp");
    }

    #[test]
    fn commands_reload_the_file_and_run_the_open_text_through_studio() {
        use crate::studio_link::tests::Fake;
        let mut fake = Fake::default();
        fake.answers = vec![
            ("rules.reload", Ok(json!({"queued": "h"}))),
            ("rules.get", Ok(json!({"reload": {"state": "done", "hash": "h", "mode": "in_place", "tick": 5}}))),
            (
                "rules.run",
                Ok(json!({"ok": true, "result": "the round was still running", "text": "total: 0 calls over 1 ticks\n"})),
            ),
        ];
        let calls = fake.calls.clone();
        let mut s = Server {
            studio: Box::new(fake),
            ..Server::default()
        };
        open(&mut s, SRC);
        let exec = |command: &str, arg: Value| json!({"command": command, "arguments": [arg]});
        let v = result(s.handle(req(1, "workspace/executeCommand", exec(command::RELOAD, json!({"uri": URI})))));
        assert_eq!(v["message"], "Reloaded t.rules in place at tick 5");
        assert_eq!(calls.borrow()[0].1, json!({"path": "/t.rules"}));

        let arg = json!({"uri": URI, "scenario": "/s/case.json"});
        let v = result(s.handle(req(2, "workspace/executeCommand", exec(command::RUN_SCENARIO, arg))));
        assert_eq!(v["message"], "case.json: the round was still running");
        assert_eq!(v["text"], "total: 0 calls over 1 ticks\n");
        // The editor's text, saved or not, is what runs.
        let run = calls.borrow().iter().find(|(m, _)| m == "rules.run").cloned().unwrap();
        assert_eq!(run.1["source"], SRC);

        let r = response(s.handle(req(3, "workspace/executeCommand", exec("h2.other", json!({})))));
        assert_eq!(r.error.unwrap().code, REQUEST_FAILED);
        assert_eq!(
            initialize_result()["capabilities"]["executeCommandProvider"]["commands"],
            json!([command::RELOAD, command::RUN_SCENARIO])
        );
    }

    #[test]
    fn tokens_encode_relative_to_the_one_before() {
        let t = |line, start, len| ide::Token {
            line,
            start,
            len,
            kind: ide::TokenKind::Variable,
        };
        let v = ide::TokenKind::Variable.index();
        assert_eq!(
            encode_tokens(&[t(0, 2, 3), t(0, 8, 1), t(2, 4, 2)]),
            [0, 2, 3, v, 0, 0, 6, 1, v, 0, 2, 4, 2, v, 0]
        );
        assert_eq!(end_of("a\nbc"), Position::new(1, 2));
        assert_eq!(end_of("a\n"), Position::new(1, 0));
    }
}
