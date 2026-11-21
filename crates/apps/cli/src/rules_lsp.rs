//! `h2 rules lsp`: a language server for `.rules` files on stdin and
//! stdout (docs/game/rules-engine.md, "Tooling"). Full document sync;
//! every answer comes from `rules::ide`, as Live's `POST /v1/rules/ide`
//! does. [`Server::handle`] is the whole protocol, so tests drive it
//! without a process.

use std::collections::HashMap;

use anyhow::{Result, anyhow};
use lsp_server::{Connection, Message, Notification, Request, Response};
use lsp_types::notification::{
    DidChangeTextDocument, DidCloseTextDocument, DidOpenTextDocument, Notification as _, PublishDiagnostics,
};
use lsp_types::request::{
    Completion, DocumentSymbolRequest, Formatting, GotoDefinition, HoverRequest, References, Request as _,
    SemanticTokensFullRequest, SignatureHelpRequest,
};
use rules::ide::{self, Position};
use serde_json::{Value, json};

/// JSON-RPC's error codes.
const METHOD_NOT_FOUND: i32 = -32601;
const INVALID_PARAMS: i32 = -32602;

/// Serves until the client shuts the server down.
pub fn run() -> Result<()> {
    let (connection, io) = Connection::stdio();
    connection
        .initialize(capabilities())
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

/// What the server offers.
fn capabilities() -> Value {
    json!({
        // Full sync: each change carries the whole text.
        "textDocumentSync": 1,
        "completionProvider": {"triggerCharacters": [".", ":", "\""]},
        "hoverProvider": true,
        "signatureHelpProvider": {"triggerCharacters": ["(", ","]},
        "definitionProvider": true,
        "referencesProvider": true,
        "documentSymbolProvider": true,
        "documentFormattingProvider": true,
        "semanticTokensProvider": {
            "legend": {"tokenTypes": ide::LEGEND, "tokenModifiers": []},
            "full": true,
            "range": false,
        },
    })
}

/// The open documents, by URI.
#[derive(Default)]
pub(crate) struct Server {
    docs: HashMap<String, String>,
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
        let method = n.method.as_str();
        if method == DidOpenTextDocument::METHOD {
            let text = n
                .params
                .pointer("/textDocument/text")
                .and_then(Value::as_str)
                .unwrap_or("");
            self.docs.insert(uri.clone(), text.to_string());
            vec![self.publish(&uri)]
        } else if method == DidChangeTextDocument::METHOD {
            let text = n
                .params
                .get("contentChanges")
                .and_then(Value::as_array)
                .and_then(|changes| changes.last())
                .and_then(|c| c.get("text"))
                .and_then(Value::as_str);
            if let Some(text) = text {
                self.docs.insert(uri.clone(), text.to_string());
            }
            vec![self.publish(&uri)]
        } else if method == DidCloseTextDocument::METHOD {
            self.docs.remove(&uri);
            vec![diagnostics_note(&uri, json!([]))]
        } else {
            Vec::new()
        }
    }

    fn publish(&self, uri: &str) -> Message {
        let src = self.docs.get(uri).map_or("", String::as_str);
        diagnostics_note(uri, json!(ide::diagnostics(src)))
    }

    fn request(&self, req: Request) -> Response {
        match self.answer(&req) {
            Ok(v) => Response::new_ok(req.id, v),
            Err((code, message)) => Response::new_err(req.id, code, message),
        }
    }

    fn answer(&self, req: &Request) -> Result<Value, (i32, String)> {
        let method = req.method.as_str();
        let known = [
            Completion::METHOD,
            HoverRequest::METHOD,
            SignatureHelpRequest::METHOD,
            GotoDefinition::METHOD,
            References::METHOD,
            SemanticTokensFullRequest::METHOD,
            DocumentSymbolRequest::METHOD,
            Formatting::METHOD,
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
            .map(String::as_str)
            .ok_or_else(|| (INVALID_PARAMS, format!("{uri} is not open")))?;
        let pos = || -> Result<Position, (i32, String)> {
            let p = req.params.get("position").cloned().unwrap_or(Value::Null);
            serde_json::from_value(p).map_err(|e| (INVALID_PARAMS, format!("position: {e}")))
        };
        let locations = |list: Vec<ide::Location>| -> Value {
            Value::Array(
                list.into_iter()
                    .map(|l| json!({"uri": uri, "range": l.range}))
                    .collect(),
            )
        };
        Ok(if method == Completion::METHOD {
            json!(ide::complete(src, pos()?))
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
        } else if method == SemanticTokensFullRequest::METHOD {
            json!({ "data": encode_tokens(&ide::semantic_tokens(src)) })
        } else if method == DocumentSymbolRequest::METHOD {
            json!(ide::symbols(src))
        } else {
            match ide::format(src) {
                Ok(text) if text == src => json!([]),
                Ok(text) => json!([{
                    "range": {"start": Position::new(0, 0), "end": end_of(src)},
                    "newText": text,
                }]),
                // Source that does not parse is left alone; its errors are
                // in the diagnostics.
                Err(_) => Value::Null,
            }
        })
    }
}

fn diagnostics_note(uri: &str, diagnostics: Value) -> Message {
    Message::Notification(Notification::new(
        PublishDiagnostics::METHOD.to_string(),
        json!({"uri": uri, "diagnostics": diagnostics}),
    ))
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
    const SRC: &str = "rules \"T\" 1\ncarrier slayer\non kill(v, k) {\n    score(k, 1)\n}\n";

    fn note(method: &str, params: Value) -> Message {
        Message::Notification(Notification::new(method.to_string(), params))
    }

    fn req(id: i32, method: &str, params: Value) -> Message {
        Message::Request(Request::new(RequestId::from(id), method.to_string(), params))
    }

    fn at(line: u32, character: u32) -> Value {
        json!({"textDocument": {"uri": URI}, "position": {"line": line, "character": character}})
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
        let open = json!({"textDocument": {"uri": URI, "languageId": "rules", "version": 1, "text": SRC}});
        assert_eq!(published(s.handle(note("textDocument/didOpen", open))), json!([]));

        let v = result(s.handle(req(1, "textDocument/hover", at(3, 5))));
        assert!(v["contents"]["value"].as_str().unwrap().contains("score(player, int)"));
        let v = result(s.handle(req(2, "textDocument/definition", at(3, 10))));
        assert_eq!(v[0]["uri"], URI);
        assert_eq!(v[0]["range"]["start"], json!({"line": 2, "character": 11}));
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
        let v = result(s.handle(req(6, "textDocument/documentSymbol", json!({"textDocument": {"uri": URI}}))));
        assert_eq!(v[1]["name"], "on kill");
        let v = result(s.handle(req(7, "textDocument/semanticTokens/full", json!({"textDocument": {"uri": URI}}))));
        let data: Vec<u64> = v["data"].as_array().unwrap().iter().map(|x| x.as_u64().unwrap()).collect();
        assert_eq!(data.len() % 5, 0);
        // `rules`, then `"T"` four columns on, as a keyword and a string.
        assert_eq!(&data[..10], &[0, 0, 5, 0, 0, 0, 6, 3, 10, 0]);
        let v = result(s.handle(req(8, "textDocument/formatting", json!({"textDocument": {"uri": URI}}))));
        assert_eq!(v, json!([]));

        // A change replaces the text and publishes again.
        let messy = "rules \"T\" 1\ncarrier slayer\non kill(v,k){score(k, )}\n";
        let change = json!({"textDocument": {"uri": URI, "version": 2}, "contentChanges": [{"text": messy}]});
        let d = published(s.handle(note("textDocument/didChange", change)));
        assert_eq!(d[0]["severity"], 1);
        let v = result(s.handle(req(9, "textDocument/formatting", json!({"textDocument": {"uri": URI}}))));
        assert_eq!(v[0]["newText"], "rules \"T\" 1\ncarrier slayer\non kill(v, k) {\n    score(k,)\n}\n");
        assert_eq!(v[0]["range"]["end"], json!({"line": 3, "character": 0}));

        // An unknown method; a closed document clears its diagnostics and
        // is no longer answered.
        let r = response(s.handle(req(10, "textDocument/rename", at(0, 0))));
        assert_eq!(r.error.unwrap().code, METHOD_NOT_FOUND);
        let close = json!({"textDocument": {"uri": URI}});
        assert_eq!(published(s.handle(note("textDocument/didClose", close))), json!([]));
        let r = response(s.handle(req(11, "textDocument/hover", at(3, 5))));
        assert_eq!(r.error.unwrap().code, INVALID_PARAMS);
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
