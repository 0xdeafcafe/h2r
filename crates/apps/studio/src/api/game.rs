//! `game.*`: the Game panel's picture of the paired game, for agents
//! ([`super::Provider`], registered in [`super::seams`]).
//!
//! The picture is `render.picture` read through the ring (`play::picture`).
//! `game.status` and `game.snapshot` look at it. `game.start` and
//! `game.stop` start and end the editor's own game (`h2 play --editor`) as
//! a request the window or `h2-editor --serve` takes once a frame, and
//! `game.mode` picks which game the panel follows. `game.camera` is not
//! here yet. docs/editor/game-view.md, docs/editor/agent-api.md.

use std::path::PathBuf;
use std::time::Instant;

use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::{MethodDef, P, Provider, Reply, def, err};
use crate::App;
use crate::play::pair::{PanelMode, Request};
use crate::play::picture::State;

/// The game picture provider.
#[derive(Default)]
pub struct Game;

const METHODS_DEFS: [(&str, &str, &str, &str, &str); 5] = [
    (
        m::GAME_STATUS,
        "The Game panel: its mode (own or attached), the own game (none, starting or running, its level and socket), the paired game's map, phase and link, and the picture's state, size, rate, age, dropped frames and tick lag. Reading it takes the newest frame",
        "{}",
        r#"{"mode":"own|attached","camera":"player","own":"{state,map?,socket?,editor?}","attached":"{state,socket,map,phase,tick,link}|null","picture":"{state,why?,transport,size,asked_fps,frames,dropped,fps,age_ms,tick,tick_lag,events}|null"}"#,
        "null",
    ),
    (
        m::GAME_SNAPSHOT,
        "The panel's newest frame as a PNG, at the size the game published. Refused with no picture or no frame yet, and inside a Git checkout",
        r#"{"out?":"string"}"#,
        r#"{"path":"string","width":"integer","height":"integer","frame":"integer","tick":"integer|null"}"#,
        r#"{"out":"$OUT/game.png"}"#,
    ),
    (
        m::GAME_START,
        "Start the editor's own game on the open document: h2 play --editor, no window, no menu, paired and shown in the Game panel. Replaces the game this editor started before. Refused with no map open, with no h2 or disc to start, and in a window-less editor with no game link. The game is up once game.status says own.state running",
        "{}",
        r#"{"requested":"boolean"}"#,
        "null",
    ),
    (
        m::GAME_STOP,
        "Stop the editor's own game. Refused when this editor started none",
        "{}",
        r#"{"requested":"boolean"}"#,
        "null",
    ),
    (
        m::GAME_MODE,
        "Which game the Game panel follows: the editor's own, or the attached one. Own is refused while there is no own game",
        r#"{"mode":"own|attached"}"#,
        r#"{"mode":"own|attached"}"#,
        r#"{"mode":"attached"}"#,
    ),
];

impl Provider for Game {
    fn methods(&self) -> Vec<MethodDef> {
        METHODS_DEFS
            .iter()
            .map(|&(name, summary, params, result, example)| def(name, summary, params, result, example))
            .collect()
    }

    fn call(&mut self, app: &mut App, method: &str, params: &Value) -> Option<Reply> {
        match method {
            m::GAME_STATUS => Some(Ok(status(app))),
            m::GAME_SNAPSHOT => Some(snapshot(app, &P(params))),
            m::GAME_START => Some(super::pair::enqueue(app, Request::StartOwn)),
            m::GAME_STOP => Some(super::pair::enqueue(app, Request::Stop)),
            m::GAME_MODE => Some(mode(app, &P(params))),
            _ => None,
        }
    }
}

/// `game.status`.
fn status(app: &mut App) -> Value {
    let now = Instant::now();
    let published = app.pairing.published.clone();
    let link = app.pairing.link.clone();
    let attached = published.as_ref().map(|p| {
        json!({
            "state": p["state"],
            "socket": p["socket"],
            "map": p["map"],
            "phase": p["phase"],
            "paused": p["paused"],
            "tick": p["tick"],
            "link": link.as_deref().map(|l| json!({"state": l.state.name(), "text": l.text})),
        })
    });
    let game_tick = published.as_ref().and_then(|p| p["tick"].as_u64());
    let picture = app.pairing.picture.as_mut().map(|picture| {
        picture.poll(now);
        let s = picture.stats(now);
        let why = match &s.state {
            State::Failed(why) => Some(why.clone()),
            _ => None,
        };
        json!({
            "state": s.state.name(),
            "why": why,
            "transport": "ring",
            "size": s.size,
            "asked_fps": s.asked_fps,
            "frames": s.frames,
            "dropped": s.dropped,
            "fps": s.fps,
            "age_ms": s.age.map(|a| a.as_millis() as u64),
            "tick": s.tick,
            // Ticks the game had run past the newest frame at its last poll.
            "tick_lag": game_tick.zip(s.tick).map(|(game, frame)| game.saturating_sub(frame)),
            "events": s.events,
        })
    });
    json!({
        "mode": app.pairing.panel.name(),
        "camera": "player",
        "own": own(app),
        "attached": attached,
        "picture": picture,
    })
}

/// `status`'s `own`: the game this editor started.
fn own(app: &App) -> Value {
    match app.pairing.link.as_deref().and_then(|l| l.own.as_ref()) {
        None => json!({"state": "none"}),
        Some(o) => json!({
            "state": if o.running { "running" } else { "starting" },
            "map": o.level,
            "socket": o.socket,
            "editor": o.editor,
        }),
    }
}

/// `game.mode`.
fn mode(app: &mut App, p: &P) -> Reply {
    let want = match p.str("mode")? {
        "own" => PanelMode::Own,
        "attached" => PanelMode::Attached,
        _ => return Err(err(code::INVALID_PARAMS, "mode is own or attached")),
    };
    if want == PanelMode::Own && app.pairing.link.as_deref().is_none_or(|l| l.own.is_none()) {
        return Err(err(code::REFUSED, "this editor has no game of its own: game.start starts one"));
    }
    // The window re-pairs the mirror and the picture at its next frame;
    // the panel's mode reads at once.
    app.pairing.panel = want;
    app.pairing.requests.push(Request::Follow(want));
    Ok(json!({"mode": want.name()}))
}

/// `game.snapshot`.
fn snapshot(app: &mut App, p: &P) -> Reply {
    let out = match p.opt_str("out")? {
        Some(o) => PathBuf::from(o),
        None => studio_proto::output_dir()
            .join("game")
            .join(format!("{}.png", app.pairing.published.as_ref().and_then(|s| s["map"].as_str()).unwrap_or("game"))),
    };
    super::docs::outside_git(&out)?;
    let picture = app.pairing.picture.as_mut().ok_or_else(|| {
        err(
            code::REFUSED,
            "no picture: pair with a game (pair.connect) that offers render.picture",
        )
    })?;
    let now = Instant::now();
    let state = picture.state(now);
    let frame = picture.snapshot(now).ok_or_else(|| {
        err(
            code::REFUSED,
            match &state {
                State::Failed(why) => format!("the game's picture failed: {why}"),
                State::Closed => "the game closed its picture".to_owned(),
                _ => "the game has published no frame yet; ask again".to_owned(),
            },
        )
    })?;
    crate::headless::write_png(&out, &frame.rgba, frame.width, frame.height)
        .map_err(|e| err(code::FAILED, format!("{e:#}")))?;
    Ok(json!({
        "path": out.display().to_string(),
        "width": frame.width,
        "height": frame.height,
        "frame": frame.number,
        "tick": frame.tick,
    }))
}
