//! `input.host` through the executor, over a real input manager with a
//! synthetic DualShock 4 behind the keyboard fallback and the debug
//! menu's gate on it, as `h2 play` builds them. No controller, no game.

use std::sync::mpsc::{Receiver, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use debug_menu::{Menu, Shared};
use input::backend::fallback::KeyboardFallback;
use input::backend::keyboard::KeyboardMouse;
use input::bindings::Bindings;
use input::host::{HostPad, HostPadId};
use input::report::DeviceInfo;
use input::{HostBackend, InputManager};
use serde_json::{Value, json};

use super::*;
use crate::{ConnId, GameStatus, Reader};

struct Nothing;
impl Reader for Nothing {
    fn status(&mut self) -> GameStatus {
        GameStatus::default()
    }
    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(vec![])
    }
}

struct Client {
    agent: Agent,
    conn: ConnId,
    rx: Receiver<String>,
}

impl Client {
    fn new(agent: &Agent) -> Client {
        let (tx, rx) = sync_channel(64);
        Client {
            agent: agent.clone(),
            conn: agent.connect(tx),
            rx,
        }
    }

    fn ok(&self, m: &str) -> Value {
        self.agent.submit(
            self.conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": m}).to_string(),
        );
        self.agent.between_slices(&mut Nothing, 0);
        let r: Value =
            serde_json::from_str(&self.rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap();
        assert!(r.get("error").is_none(), "{m}: {r}");
        r["result"].clone()
    }
}

/// A DualShock 4 as the GameController backend would describe it.
struct Ds4 {
    touch: Arc<Mutex<Option<Touchpad>>>,
    start: Arc<Mutex<bool>>,
}

impl Ds4 {
    fn state(&self) -> HostPadState {
        let mut s = HostPadState {
            prefiltered: true,
            touchpad: *self.touch.lock().unwrap(),
            ..Default::default()
        };
        s.buttons
            .set(HostButton::Start, *self.start.lock().unwrap());
        s
    }
}

impl HostBackend for Ds4 {
    fn name(&self) -> &str {
        "game-controller"
    }
    fn poll(&mut self) -> Vec<HostPad> {
        vec![HostPad {
            id: HostPadId(1),
            name: "DUALSHOCK 4 Wireless Controller".into(),
            state: self.state(),
            physical: true,
            supports_rumble: true,
        }]
    }
    fn devices(&self) -> Vec<DeviceInfo> {
        vec![DeviceInfo {
            backend: "game-controller".into(),
            id: 1,
            name: "DUALSHOCK 4 Wireless Controller".into(),
            profile: Some("dualshock4".into()),
            category: Some("DualShock 4".into()),
            state: self.state(),
        }]
    }
}

struct Rig {
    client: Client,
    input: InputManager,
    menu: Shared,
    touch: Arc<Mutex<Option<Touchpad>>>,
    start: Arc<Mutex<bool>>,
}

fn rig(with_menu: bool) -> Rig {
    let touch = Arc::new(Mutex::new(Some(Touchpad::default())));
    let start = Arc::new(Mutex::new(false));
    let mut input = InputManager::new(Default::default()).unwrap();
    input.add_backend(Box::new(KeyboardFallback::new(
        KeyboardMouse::new(Bindings::default()),
        Some(Box::new(Ds4 {
            touch: Arc::clone(&touch),
            start: Arc::clone(&start),
        })),
        Default::default(),
    )));
    let menu = Shared::new(Menu::default());
    if with_menu {
        input.set_gate(Box::new(menu.gate()));
    }
    let agent = Agent::new();
    add_host_pads(&agent, input.host_report(), with_menu.then(|| menu.clone()));
    Rig {
        client: Client::new(&agent),
        input,
        menu,
        touch,
        start,
    }
}

#[test]
fn hostpads_shows_the_controller_the_merged_pad_and_the_gate() {
    let mut r = rig(true);
    let v = r.client.ok(method::INPUT_HOST);
    assert_eq!(v["syncs"], 0);
    assert!(
        v["notes"][0].as_str().unwrap().contains("not synced"),
        "{v}"
    );
    r.input.sync();
    // Options down, no click yet: Start waits for the click.
    *r.start.lock().unwrap() = true;
    r.input.sync();
    let v = r.client.ok(method::INPUT_HOST);
    let d = &v["devices"][0];
    assert_eq!(d["profile"], "dualshock4");
    assert_eq!(d["category"], "DualShock 4");
    assert_eq!(d["backend"], "game-controller");
    assert_eq!(d["touchpad"]["click"], false);
    assert_eq!(d["buttons"], json!(["start"]));
    let p = &v["pads"][0];
    assert_eq!(p["backend"], "keyboard-fallback");
    assert_eq!(p["key"], json!([0, 0]));
    assert_eq!(p["port"], 0);
    assert!(
        p["touchpad"].is_object(),
        "player one keeps the touchpad: {p}"
    );
    let g = &v["gate"];
    assert_eq!(g["open"], false);
    assert_eq!(g["pads"][0]["touchpad"], true);
    assert_eq!(g["pads"][0]["chord_pending"], true);
    assert_eq!(v["notes"], json!([]));
    // The click joins: the menu opens, and the answer says so.
    *r.touch.lock().unwrap() = Some(Touchpad {
        click: true,
        primary: Some((0.0, 0.25)),
        secondary: None,
    });
    r.input.sync();
    let v = r.client.ok(method::INPUT_HOST);
    assert_eq!(v["devices"][0]["touchpad"]["click"], true);
    assert_eq!(v["devices"][0]["touchpad"]["primary"], json!([0.0, 0.25]));
    assert_eq!(v["pads"][0]["touchpad"]["click"], true);
    assert_eq!(v["gate"]["open"], true);
    assert_eq!(v["gate"]["pads"][0]["click"], true);
    assert_eq!(v["gate"]["pads"][0]["chord"], true);
    assert!(r.menu.is_open());
}

#[test]
fn notes_name_a_pad_without_a_touchpad_and_a_missing_menu() {
    let mut r = rig(false);
    *r.touch.lock().unwrap() = None;
    r.input.sync();
    let v = r.client.ok(method::INPUT_HOST);
    assert_eq!(v["gate"], Value::Null);
    let notes: Vec<&str> = v["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|n| n.as_str().unwrap())
        .collect();
    assert!(
        notes
            .iter()
            .any(|n| n.contains("without touchpad elements")),
        "{notes:?}"
    );
    assert!(
        notes.iter().any(|n| n.contains("no debug menu")),
        "{notes:?}"
    );
}

#[test]
fn notes_say_when_the_touchpad_is_lost_before_the_gate() {
    let report = HostReport {
        syncs: 3,
        devices: vec![DeviceInfo {
            backend: "game-controller".into(),
            name: "DualSense Wireless Controller".into(),
            profile: Some("dualsense".into()),
            state: HostPadState {
                touchpad: Some(Touchpad::default()),
                ..Default::default()
            },
            ..Default::default()
        }],
        pads: vec![input::report::PadInfo {
            backend: "keyboard-fallback".into(),
            ..Default::default()
        }],
    };
    let v = host_pads_json(&report, Some(&(false, GateReport::default())));
    let notes = v["notes"].to_string();
    assert!(notes.contains("lost between"), "{notes}");
    assert!(notes.contains("gate has not seen a sync"), "{notes}");
    // A plain extended profile: no touchpad, said why.
    let report = HostReport {
        syncs: 1,
        devices: vec![DeviceInfo {
            name: "Xbox Wireless Controller".into(),
            profile: Some("extended".into()),
            ..Default::default()
        }],
        pads: vec![],
    };
    let v = host_pads_json(&report, None);
    assert!(v["notes"].to_string().contains("plain extended profile"));
}
