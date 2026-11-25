//! Frame captures and `render.stats` through the socket, with the backend
//! wrapper's side played by the test.
#![cfg(unix)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use agent::render::{DrawInfo, FrameProbe, RenderSources};
use agent::{Agent, GameStatus, Reader, Server};
use agent_proto::{Client, DrawLayout, Object, code, method};
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
            display_rate: Box::new(|| {
                Some(agent_proto::DisplayRateStats { rate_hz: 60, swaps: 7, blended: 6, ..Default::default() })
            }),
            ui: Box::new(|| {
                Some(agent_proto::UiStats {
                    output: [1920, 1080],
                    scale: [0.75, 1.0],
                    offset: [80.0, 0.0],
                    anchor: "pinned".into(),
                    atlas_scale: 3,
                    atlas_page: 2048,
                })
            }),
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
    assert_eq!(s["display_rate"]["blended"], 6);
    assert_eq!(s["display_rate"]["rate_hz"], 60);
    assert_eq!(s["ui"]["output"], serde_json::json!([1920, 1080]));
    assert_eq!(s["ui"]["scale"], serde_json::json!([0.75, 1.0]));
    assert_eq!(s["ui"]["anchor"], "pinned");
    assert_eq!(s["ui"]["atlas_scale"], 3);

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
    match c.call(method::RENDER_FRAME, json!({"source": "private", "arm": true})) {
        Err(agent_proto::CallError::Rpc(e)) => assert_eq!(e.code, code::UNAVAILABLE),
        other => panic!("{other:?}"),
    }
    assert_eq!(
        c.call(method::RENDER_FRAME, json!({"source": "display"})).unwrap(),
        r,
        "requesting an unavailable private recorder preserves the displayed frame"
    );
    let methods = c.call(method::METHODS, Value::Null).unwrap();
    assert!(
        methods.to_string().contains(method::RENDER_FRAME_LAYOUT),
        "{methods}"
    );
    let r = c
        .call(method::RENDER_FRAME_LAYOUT, json!({"arm": true}))
        .unwrap();
    assert_eq!(r["seq"], 2);
    PROBE.present(3);
    assert!(PROBE.layout_requested());
    PROBE.draw(DrawInfo {
        color: "Surface(3)".into(),
        target: "640x480 X8R8G8B8".into(),
        outcome: "drawn".into(),
        vertices: 4,
        layout: Some(DrawLayout {
            guest_size: [640, 480],
            display_size: Some([2800, 1700]),
            ..Default::default()
        }),
        ..Default::default()
    });
    PROBE.present(4);
    let layout = c.call(method::RENDER_FRAME_LAYOUT, Value::Null).unwrap();
    assert_eq!(
        layout["frame"]["draws"][0]["layout"]["display_size"],
        json!([2800, 1700])
    );
    assert_eq!(c.call(method::RENDER_FRAME, Value::Null).unwrap(), layout);
    assert!(!PROBE.layout_requested());

    drop(c);
    stop.store(true, Ordering::Relaxed);
    h.join().unwrap();
    drop(server);
}
