//! `game.status` and `game.snapshot` over an invented picture ring
//! (docs/editor/agent-api.md, "Game picture").
#![cfg(unix)]

use std::time::Instant;

use agent_proto::picture::{Opened, Ring, SlotMeta, Transport};

use super::*;
use crate::play::picture::Picture;

/// A game's ring with one 4x2 frame in it, and the editor's picture of it.
fn fake(t: &Temp, frames: u64) -> (Ring, Picture) {
    let path = t.path("picture.ring");
    let game = Ring::create(&path, (8, 8)).unwrap();
    for n in 1..=frames {
        game.publish(
            SlotMeta {
                frame: n,
                tick: Some(100 + n),
                present: n,
                time_us: n * 16_000,
                width: 4,
                height: 2,
            },
            &[n as u8 * 40; 32],
        );
    }
    let opened = Opened {
        id: 1,
        transport: Transport::Ring,
        path: Some(path.clone()),
        size: [8, 8],
        format: "rgba8".into(),
        fps: 60,
    };
    let picture = Picture::with_ring(Ring::open(&path).unwrap(), opened, Instant::now());
    (game, picture)
}

#[test]
fn status_with_no_game_has_no_picture() {
    let t = Temp::new("game-status-none");
    let mut app = app(&t);
    let mut api = Api::new();
    let s = call(&mut api, &mut app, m::GAME_STATUS, Value::Null);
    assert_eq!(s["mode"], "attached");
    assert_eq!(s["own"]["state"], "none");
    assert!(s["picture"].is_null());
    assert!(s["attached"].is_null());
}

#[test]
fn status_reads_the_newest_frame_and_its_lag() {
    let t = Temp::new("game-status-picture");
    let mut app = app(&t);
    let mut api = Api::new();
    let (_game, picture) = fake(&t, 2);
    app.pairing.picture = Some(picture);
    app.pairing.published = Some(json!({"state": "connected", "map": "example", "phase": "gameplay", "tick": 110}));
    let s = call(&mut api, &mut app, m::GAME_STATUS, Value::Null);
    let p = &s["picture"];
    assert_eq!(p["state"], "open");
    assert_eq!(p["size"], json!([4, 2]));
    assert_eq!(p["frames"], 1, "the status took the newest frame once");
    assert_eq!(p["tick"], 102);
    assert_eq!(p["tick_lag"], 8);
    assert_eq!(p["asked_fps"], 60);
    assert_eq!(s["attached"]["map"], "example");
}

#[test]
fn snapshot_writes_the_frame_as_a_png() {
    let t = Temp::new("game-snapshot");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(
        code_of(api.call(&mut app, m::GAME_SNAPSHOT, json!({"out": t.path("a.png")}))),
        code::REFUSED,
        "no picture"
    );
    let (game, picture) = fake(&t, 0);
    app.pairing.picture = Some(picture);
    assert_eq!(
        code_of(api.call(&mut app, m::GAME_SNAPSHOT, json!({"out": t.path("a.png")}))),
        code::REFUSED,
        "no frame yet"
    );
    game.publish(
        SlotMeta {
            frame: 1,
            tick: Some(7),
            present: 1,
            time_us: 0,
            width: 4,
            height: 2,
        },
        &[200; 32],
    );
    let out = t.path("game.png");
    let r = call(&mut api, &mut app, m::GAME_SNAPSHOT, json!({"out": out}));
    assert_eq!((r["width"].as_u64(), r["height"].as_u64()), (Some(4), Some(2)));
    assert_eq!(r["tick"], 7);
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&out).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut pixels = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut pixels).unwrap();
    assert_eq!((info.width, info.height), (4, 2));
    assert_eq!(pixels[0], 200);
}

#[test]
fn snapshot_stays_out_of_a_git_checkout() {
    let t = Temp::new("game-snapshot-git");
    let mut app = app(&t);
    let mut api = Api::new();
    let (_game, picture) = fake(&t, 1);
    app.pairing.picture = Some(picture);
    let repo = t.path("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    assert_eq!(
        code_of(api.call(&mut app, m::GAME_SNAPSHOT, json!({"out": repo.join("g.png")}))),
        code::REFUSED
    );
}

fn own(running: bool) -> crate::play::OwnGame {
    crate::play::OwnGame {
        level: "example".into(),
        socket: "/cache/editor-1-1.sock".into(),
        editor: true,
        running,
    }
}

#[test]
fn status_names_the_own_game_and_the_mode_follows_it() {
    let t = Temp::new("game-own-status");
    let mut app = app(&t);
    let mut api = Api::new();
    // No own game: the panel cannot be switched to it.
    assert_eq!(
        code_of(api.call(&mut app, m::GAME_MODE, json!({"mode": "own"}))),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::GAME_MODE, json!({"mode": "both"}))),
        code::INVALID_PARAMS
    );
    let view = {
        let mut v = crate::play::Play::new().link_view(&app);
        v.own = Some(own(false));
        v
    };
    app.pairing.link = Some(std::sync::Arc::new(view.clone()));
    let s = call(&mut api, &mut app, m::GAME_STATUS, Value::Null);
    assert_eq!(s["own"]["state"], "starting");
    assert_eq!(s["own"]["map"], "example");
    assert_eq!(s["own"]["editor"], true);
    assert_eq!(s["mode"], "attached");

    app.pairing.link = Some(std::sync::Arc::new(crate::play::LinkView { own: Some(own(true)), ..view }));
    assert_eq!(call(&mut api, &mut app, m::GAME_STATUS, Value::Null)["own"]["state"], "running");
    assert_eq!(call(&mut api, &mut app, m::GAME_MODE, json!({"mode": "own"}))["mode"], "own");
    assert_eq!(call(&mut api, &mut app, m::GAME_STATUS, Value::Null)["mode"], "own");
    call(&mut api, &mut app, m::GAME_MODE, json!({"mode": "attached"}));
    assert_eq!(app.pairing.panel, crate::play::pair::PanelMode::Attached);
}

#[test]
fn start_and_stop_leave_requests_for_the_window() {
    use crate::play::pair::Request;
    let t = Temp::new("game-start-stop");
    let mut app = app(&t);
    let mut api = Api::new();
    // No window has published a link yet.
    assert_eq!(code_of(api.call(&mut app, m::GAME_START, Value::Null)), code::REFUSED);

    app.pairing.published = Some(json!({"state": "none"}));
    let mut view = crate::play::Play::new().link_view(&app);
    view.can_play = false;
    view.can_stop = false;
    app.pairing.link = Some(std::sync::Arc::new(view.clone()));
    // No map to play, and no game of its own to stop.
    assert_eq!(code_of(api.call(&mut app, m::GAME_START, Value::Null)), code::REFUSED);
    assert_eq!(code_of(api.call(&mut app, m::GAME_STOP, Value::Null)), code::REFUSED);
    assert!(app.pairing.requests.is_empty());

    view.can_play = true;
    view.can_stop = true;
    app.pairing.link = Some(std::sync::Arc::new(view));
    assert_eq!(call(&mut api, &mut app, m::GAME_START, Value::Null)["requested"], true);
    assert_eq!(call(&mut api, &mut app, m::GAME_STOP, Value::Null)["requested"], true);
    assert_eq!(app.pairing.requests, [Request::StartOwn, Request::Stop]);
}
