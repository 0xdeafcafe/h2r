//! `render.frame` and `render.stats` through the socket, with the backend
//! wrapper's side played by the test.
#![cfg(unix)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agent::render::{DrawInfo, FrameProbe, RenderSources};
use agent::{Agent, GameStatus, Reader, Server};
use agent_proto::{Client, Object, code, method};
use serde_json::{Value, json};

struct Menu;

impl Reader for Menu {
    fn status(&mut self) -> GameStatus {
        GameStatus::default()
    }
    fn objects(&mut self) -> Result<Vec<Object>, String> {
        Ok(vec![])
    }
}

static PROBE: FrameProbe = FrameProbe::new();

#[test]
fn render_frame_and_stats_through_the_socket() {
    let path = std::env::temp_dir().join(format!("h2a-{}-render.sock", std::process::id()));
    let a = Agent::new();
    agent::render::add_methods(
        &a,
        &PROBE,
        RenderSources {
            interp: Box::new(|| Some(json!({"frames": 7, "unsure_camera": 1}))),
            pipeline: Box::new(|| None),
            counters: Box::new(|| None),
        },
    );
    let server = Server::listen(&path, a.clone()).unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let h = {
        let (s, a) = (Arc::clone(&stop), a.clone());
        thread::spawn(move || {
            while !s.load(Ordering::Relaxed) {
                a.between_slices(&mut Menu, 0);
                thread::sleep(Duration::from_millis(1));
            }
        })
    };
    let mut c = Client::connect(&path).unwrap();
    c.set_timeout(Some(Duration::from_secs(10))).unwrap();

    let s = c.call(method::RENDER_STATS, Value::Null).unwrap();
    assert_eq!(s["interp"]["unsure_camera"], 1);
    assert_eq!(s["pipeline"], Value::Null, "not running");

    let r = c.call(method::RENDER_FRAME, Value::Null).unwrap();
    assert_eq!(r["armed"], false);
    let r = c
        .call(method::RENDER_FRAME, json!({"arm": true, "draws": true}))
        .unwrap();
    assert_eq!((&r["armed"], &r["seq"]), (&json!(true), &json!(1)));
    PROBE.present(1);
    PROBE.draw(DrawInfo {
        color: "Surface(3)".into(),
        target: "640x480 X8R8G8B8".into(),
        outcome: "drawn".into(),
        vertices: 4,
        ..Default::default()
    });
    PROBE.present(2);
    let r = c.call(method::RENDER_FRAME, Value::Null).unwrap();
    assert_eq!(r["armed"], false);
    assert_eq!(r["frame"]["seq"], 1);
    assert_eq!(r["frame"]["passes"][0]["color"], "Surface(3)");
    assert_eq!(r["frame"]["draws"][0]["vertices"], 4);
    match c.call(method::RENDER_FRAME, json!({"draws": true})) {
        Err(agent_proto::CallError::Rpc(e)) => assert_eq!(e.code, code::INVALID_PARAMS),
        other => panic!("{other:?}"),
    }
    let methods = c.call(method::METHODS, Value::Null).unwrap();
    assert!(methods.to_string().contains("render.frame"), "{methods}");

    drop(c);
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    drop(server);
}
