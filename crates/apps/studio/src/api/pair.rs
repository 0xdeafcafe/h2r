//! `pair.*`: the editor's pairing with a running game, for agents
//! ([`super::Provider`], registered in [`super::seams`]).
//!
//! The pairing lives with the window ([`crate::play::Play`]); these methods
//! leave requests on the app's [`crate::play::pair::Bridge`] and read what
//! the window published there. The controls answer before the window acts.
//! Read `pair.status` for the result. `pair.list` looks for games itself
//! and works headless. docs/editor/agent-api.md, docs/editor/editor-guide.md.

use agent_proto::discover::{self, MapMatch};
use agent_proto::{Endpoint, RpcError};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
use crate::play::{LinkView, Spawn};
use crate::play::pair::Request;

/// The pairing provider.
#[derive(Default)]
pub struct Pair;

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
        m::PAIR_LIST,
        "The games running now (each one's default socket in the agents directory, and the sockets Play and h2 serve use): map, phase, whether its map is the one open here, and which one the editor is paired with. Leftover sockets nothing answers on are removed",
        "{}",
        r#"{"open_map":"string|null","games":"[{socket,pid,map,phase,paused,tick,matches:same|different|unknown,paired,note}]","dir":"string"}"#,
        "null",
    ),
    def(
        m::PAIR_STATUS,
        "The socket connection, game status, edit permissions and auto-pair setting, with the game link pill's state, label and tooltip. Needs the window",
        "{}",
        r#"{"state":"off|connecting|reconnecting|connected|lost","why":"string|null","socket":"string|null","map":"string|null","phase":"string|null","paused":"boolean|null","tick":"integer|null","matches":"same|different|unknown","open_map":"string|null","live_edits":"boolean","edits_blocked":"string|null","auto_pair":"boolean","games":"integer","ask":"send|load|null","prompt":"integer|null","playtest_busy":"boolean","link":{"state":"no_game|found|connecting|reconnecting|viewing|editing|blocked|other_map|lost","text":"string","why":"string|null","elapsed_ms?":"integer","unavailable?":"[string]"}}"#,
        "null",
    ),
    def(
        m::PAIR_CONNECT,
        "Pair with a game in View mode: the socket given (a path or tcp://HOST:PORT), else the one game running. Answers at once; poll pair.status for connected. Needs the window",
        r#"{"socket?":"string"}"#,
        r#"{"requested":"string"}"#,
        r#"{"socket":"/tmp/h2-pair-example.sock"}"#,
    ),
    def(
        m::PAIR_DISCONNECT,
        "Unpair. Choose keep or restore, or open the leave prompt",
        r#"{"leave?":"keep|restore"}"#,
        r#"{"requested":"boolean"}"#,
        "null",
    ),
    def(
        m::PAIR_AUTO,
        "Read, or with `enabled` set, the auto-pair setting: prefer its own game, the only game, or the unique matching map. On by default",
        r#"{"enabled?":"boolean"}"#,
        r#"{"auto_pair":"boolean"}"#,
        r#"{"enabled":false}"#,
    ),
    def(
        m::PAIR_PLAY,
        "Play the open map from the editor. Needs the window",
        "{}",
        r#"{"requested":"boolean"}"#,
        "null",
    ),
    def(
        m::PAIR_PLAYTEST,
        "Play the open document in the game. A confirmation answers the current playtest prompt",
        r#"{"confirm?":"send|cancel|load|open","prompt?":"integer"}"#,
        r#"{"step":"launch|pair|resume|reload|ask","ask?":"send|load","prompt?":"integer","place?":"{pos,yaw,pitch}","why?":"string","answer?":"cancel|open"}"#,
        "null",
    ),
    def(
        m::PAIR_SETTINGS,
        "Read or set the playtest spawn and pause on editor focus",
        r#"{"spawn?":"auto|level|resume|camera|selection","pause_while_editing?":"boolean"}"#,
        r#"{"spawn":"auto|level|resume|camera|selection","pause_while_editing":"boolean"}"#,
        r#"{"pause_while_editing":true}"#,
    ),
    def(
        m::PAIR_STOP,
        "Stop the game the editor started. Needs the window",
        "{}",
        r#"{"requested":"boolean"}"#,
        "null",
    ),
    def(
        m::PAIR_EDIT,
        "Set the Edit switch for the paired game. REFUSED includes the current reason",
        r#"{"enabled":"boolean"}"#,
        r#"{"requested":"boolean","mode":"edit|view"}"#,
        r#"{"enabled":true}"#,
    ),
    def(
        m::PAIR_CONTROL,
        "Pause, step or resume through the editor's game link. Needs the window",
        r#"{"action":"string","ticks?":"integer"}"#,
        r#"{"requested":"boolean"}"#,
        r#"{"action":"step","ticks":1}"#,
    ),
];

impl Provider for Pair {
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

fn need_window(app: &App) -> Result<(&Value, &LinkView), RpcError> {
    app.pairing.published.as_ref().zip(app.pairing.link.as_ref()).ok_or_else(|| {
        err(
            code::REFUSED,
            "pairing runs in the editor window: this editor has none (or has not drawn a frame yet)",
        )
    })
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::PAIR_LIST => Ok(list(app)),
        m::PAIR_STATUS => {
            let (status, link) = need_window(app)?;
            let mut status = status.clone();
            let prompt = app.pairing.playtest.as_ref()
                .filter(|offer| offer.current_document(app))
                .and(app.pairing.prompt.as_ref());
            status["ask"] = json!(prompt.map(|p| p.ask.name()));
            status["prompt"] = json!(prompt.map(|p| p.id));
            status["link"] =
                json!({"state": link.state.name(), "text": link.text, "why": link.why});
            if !link.unavailable.is_empty() {
                status["link"]["unavailable"] = json!(link.unavailable);
            }
            if let crate::play::LinkState::Reconnecting { since } = &link.state {
                status["link"]["elapsed_ms"] = json!(since.elapsed().as_millis());
            }
            Ok(status)
        }
        m::PAIR_CONNECT => {
            need_window(app)?;
            let socket = p.opt_str("socket")?;
            let endpoint = match socket {
                Some(s) => Endpoint::parse(s).map_err(|e| err(code::INVALID_PARAMS, e))?,
                None => {
                    let games = running(app);
                    match games.as_slice() {
                        [one] => Endpoint::Unix(std::path::PathBuf::from(one["socket"].as_str().unwrap_or_default())),
                        [] => {
                            return Err(err(
                                code::REFUSED,
                                "no game is running (pair.list); start one, or name its socket",
                            ));
                        }
                        many => {
                            return Err(err(
                                code::REFUSED,
                                format!("{} games are running: name one's socket (pair.list)", many.len()),
                            ));
                        }
                    }
                }
            };
            let requested = endpoint.to_string();
            app.pairing.requests.push(if socket.is_some() {
                Request::PairSocket(endpoint)
            } else {
                Request::Pair(endpoint)
            });
            Ok(json!({ "requested": requested }))
        }
        m::PAIR_DISCONNECT => {
            need_window(app)?;
            let request = match p.opt_str("leave")? {
                None => Request::Unpair,
                Some("keep") => Request::Apply(crate::play::Apply::Keep),
                Some("restore") => Request::Apply(crate::play::Apply::Restore),
                _ => return Err(err(code::INVALID_PARAMS, "leave must be keep or restore")),
            };
            app.pairing.requests.push(request);
            Ok(json!({ "requested": true }))
        }
        m::PAIR_AUTO => {
            if let Some(on) = p.get("enabled") {
                app.pairing.auto_pair = on
                    .as_bool()
                    .ok_or_else(|| err(code::INVALID_PARAMS, "enabled is a boolean"))?;
            }
            Ok(json!({ "auto_pair": app.pairing.auto_pair }))
        }
        m::PAIR_PLAY | m::PAIR_PLAYTEST => {
            let (_, link) = need_window(app)?;
            if let Some(why) = Request::Play.refusal(link) {
                return Err(err(code::REFUSED, why));
            }
            let offer = app.pairing.playtest.as_ref()
                .ok_or_else(|| err(code::REFUSED, "waiting for the editor window"))?;
            if !offer.current_document(app) {
                return Err(err(code::REFUSED, "the document changed; wait for the next editor frame"));
            }
            let mut report = offer.step.report(offer.place)
                .map_err(|why| err(code::REFUSED, why))?;
            if method == m::PAIR_PLAY {
                app.pairing.requests.push(Request::Play);
                return Ok(json!({"requested": true}));
            }
            let prompt_id = p.opt_u64("prompt")?;
            let (request, reply) = match p.opt_str("confirm")? {
                None => {
                    if prompt_id.is_some() {
                        return Err(err(code::INVALID_PARAMS, "prompt needs a confirm action"));
                    }
                    let prompt = offer.prompt.clone();
                    if let Some(p) = &prompt {
                        report["prompt"] = json!(p.id);
                    }
                    app.pairing.show_prompt(prompt);
                    (Request::Play, report)
                }
                Some(choice) => {
                    let prompt = app.pairing.prompt.clone()
                        .ok_or_else(|| err(code::REFUSED, "there is no playtest prompt to answer"))?;
                    if prompt_id.is_some_and(|id| id != prompt.id)
                        || offer.prompt.as_ref() != Some(&prompt)
                    {
                        return Err(err(
                            code::REFUSED,
                            "the game or pending edits changed; press Play again",
                        ));
                    }
                    let reply = if choice == prompt.ask.name() {
                        prompt.confirmed.report(offer.place)
                            .map_err(|why| err(code::REFUSED, why))?
                    } else if choice == "cancel" || (choice == "open" && prompt.ask.name() == "load") {
                        json!({"step": "ask", "answer": choice})
                    } else {
                        return Err(err(code::INVALID_PARAMS, "choose an action offered by the prompt"));
                    };
                    (Request::PlayAnswer(prompt, choice.into()), reply)
                }
            };
            app.pairing.requests.push(request);
            Ok(reply)
        }
        m::PAIR_SETTINGS => {
            let spawn = match p.opt_str("spawn")? {
                None => app.pairing.spawn,
                Some("auto") => None,
                Some("level") => Some(Spawn::Level),
                Some("resume") => Some(Spawn::Resume),
                Some("camera") => Some(Spawn::Camera),
                Some("selection") => Some(Spawn::Selection),
                Some(_) => return Err(err(code::INVALID_PARAMS, "unknown spawn choice")),
            };
            if p.get("pause_while_editing").is_some() {
                app.pairing.pause_while_editing = p.bool("pause_while_editing")?;
            }
            app.pairing.spawn = spawn;
            let name = match spawn {
                None => "auto",
                Some(Spawn::Level) => "level",
                Some(Spawn::Resume) => "resume",
                Some(Spawn::Camera) => "camera",
                Some(Spawn::Selection) => "selection",
            };
            Ok(json!({"spawn": name, "pause_while_editing": app.pairing.pause_while_editing}))
        }
        m::PAIR_STOP => enqueue(app, Request::Stop),
        m::PAIR_EDIT => {
            let enabled = p.bool("enabled")?;
            if enabled && let Some(why) = need_window(app)?.0["edit_refusal"].as_str() {
                return Err(err(code::REFUSED, why));
            }
            let mut result = enqueue(app, Request::Edit(enabled))?;
            result["mode"] = json!(if enabled { "edit" } else { "view" });
            Ok(result)
        }
        m::PAIR_CONTROL => {
            let req = match (p.str("action")?, p.get("ticks")) {
                ("pause", None) => Request::Pause,
                ("resume", None) => Request::Resume,
                ("step", _) => {
                    let ticks = p.opt_u64("ticks")?.unwrap_or(1);
                    if !(1..=600).contains(&ticks) {
                        return Err(err(code::INVALID_PARAMS, "ticks must be between 1 and 600"));
                    }
                    Request::Step(ticks)
                }
                _ => {
                    return Err(err(
                        code::INVALID_PARAMS,
                        "use pause, resume, or step (with optional ticks)",
                    ));
                }
            };
            enqueue(app, req)
        }
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
}

fn enqueue(app: &mut App, req: Request) -> Reply {
    let (_, link) = need_window(app)?;
    if let Some(why) = req.refusal(link) {
        return Err(err(code::REFUSED, why));
    }
    app.pairing.requests.push(req);
    Ok(json!({ "requested": true }))
}

/// The games found now, as `pair.list` lists them.
fn games(app: &App) -> Vec<Value> {
    let paired = app
        .pairing
        .published
        .as_ref()
        .filter(|v| v["state"] == "connected" || v["state"] == "connecting")
        .and_then(|v| v["socket"].as_str().map(str::to_owned));
    crate::play::pair::default_scan()
        .into_iter()
        .map(|g| {
            let at = g.status.as_ref().map(|s| &s.at);
            let m = match crate::play::pair::map_match(app, g.map()) {
                MapMatch::Same => "same",
                MapMatch::Different => "different",
                MapMatch::Unknown => "unknown",
            };
            let socket = g.path.display().to_string();
            json!({
                "paired": paired.as_deref() == Some(socket.as_str()),
                "socket": socket,
                "pid": g.pid,
                "map": g.map(),
                "phase": at.map(|a| format!("{:?}", a.phase).to_lowercase()),
                "paused": at.map(|a| a.paused),
                "tick": at.map(|a| a.tick.unwrap_or(a.ticks)),
                "matches": m,
                "note": g.note,
            })
        })
        .collect()
}

/// Games that answered.
fn running(app: &App) -> Vec<Value> {
    games(app).into_iter().filter(|g| g["note"].is_null()).collect()
}

fn list(app: &App) -> Value {
    json!({ "open_map": app.loaded_map(), "games": games(app), "dir": discover::dir().display().to_string() })
}
