//! `pair.*`: the editor's pairing with a running game, for agents
//! ([`super::Provider`], registered in [`super::seams`]).
//!
//! The pairing lives with the window ([`crate::play::Play`]); these methods
//! leave requests on the app's [`crate::play::pair::Bridge`] and read what
//! the window published there. So `pair.status`, `pair.connect` and
//! `pair.disconnect` need the window (a headless editor has no pairing)
//! and `pair.connect` answers before the window has acted: poll
//! `pair.status`. `pair.list` looks at the games itself, and works
//! headless. docs/editor/agent-api.md, docs/editor/editor-guide.md.

use agent_proto::discover::{self, MapMatch};
use agent_proto::{Endpoint, RpcError};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, err};
use crate::App;
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
        "The pairing: off, connecting, connected or lost, the game's socket, map, phase and tick, whether its map is the open one, why edits are not sent, and the auto-pair setting. Needs the window",
        "{}",
        r#"{"state":"off|connecting|connected|lost","why":"string|null","socket":"string|null","map":"string|null","phase":"string|null","paused":"boolean|null","tick":"integer|null","matches":"same|different|unknown","open_map":"string|null","live_edits":"boolean","edits_blocked":"string|null","auto_pair":"boolean","games":"integer"}"#,
        "null",
    ),
    def(
        m::PAIR_CONNECT,
        "Pair with a game, as Pair in the Game panel does: the socket given (a path or tcp://HOST:PORT), else the one game running. Answers at once; poll pair.status for connected. Needs the window",
        r#"{"socket?":"string"}"#,
        r#"{"requested":"string"}"#,
        r#"{"socket":"/tmp/h2-pair-example.sock"}"#,
    ),
    def(
        m::PAIR_DISCONNECT,
        "Unpair (the game keeps running; control is released). Needs the window",
        "{}",
        r#"{"requested":"boolean"}"#,
        "null",
    ),
    def(
        m::PAIR_AUTO,
        "Read, or with `enabled` set, the auto-pair setting: pair by itself with the one game running, once per game. On by default",
        r#"{"enabled?":"boolean"}"#,
        r#"{"auto_pair":"boolean"}"#,
        r#"{"enabled":false}"#,
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

fn need_window(app: &App) -> Result<&Value, RpcError> {
    app.pairing.published.as_ref().ok_or_else(|| {
        err(
            code::REFUSED,
            "pairing runs in the editor window: this editor has none (or has not drawn a frame yet)",
        )
    })
}

fn call(app: &mut App, method: &str, p: &P) -> Reply {
    match method {
        m::PAIR_LIST => Ok(list(app)),
        m::PAIR_STATUS => Ok(need_window(app)?.clone()),
        m::PAIR_CONNECT => {
            need_window(app)?;
            let endpoint = match p.opt_str("socket")? {
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
            app.pairing.requests.push(Request::Pair(endpoint));
            Ok(json!({ "requested": requested }))
        }
        m::PAIR_DISCONNECT => {
            need_window(app)?;
            app.pairing.requests.push(Request::Unpair);
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
        _ => Err(err(code::METHOD_NOT_FOUND, format!("no method {method}"))),
    }
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
            let m = match MapMatch::of(app.loaded_map(), g.map()) {
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
