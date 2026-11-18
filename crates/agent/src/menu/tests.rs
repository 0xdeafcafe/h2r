//! The menu against the real executor: its calls are answered by the
//! same host methods `h2 agent` reaches (`render.*` over a hub, settings
//! over a store), between slices as in a game that isn't ticking.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use debug_menu::{Key, Menu, Nav, Shared};
use serde_json::json;
use settings::{Apply, Choice, Def, Kind, Scope, Store, Tier, Unit};

use super::*;
use crate::{GameStatus, Reader};

struct Nothing;
impl Reader for Nothing {
    fn status(&mut self) -> GameStatus {
        GameStatus::default()
    }
    fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
        Ok(vec![])
    }
}

static FOV: Def = Def {
    key: "graphics.fov",
    label: "Field of View",
    help: "degrees",
    kind: Kind::Int {
        valid: 60..=110,
        steps: &[70, 90, 110],
        unit: Unit::Degrees,
    },
    default: settings::Value::Int(70),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::View,
    tier: Tier::User,
    menu: None,
};

static LIGHTING: Def = Def {
    key: "graphics.lighting",
    label: "Lighting",
    help: "",
    kind: Kind::Choice(&[
        Choice {
            value: 0,
            name: "retail",
            label: "Retail",
        },
        Choice {
            value: 1,
            name: "e3",
            label: "E3",
        },
    ]),
    default: settings::Value::Int(0),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Restart,
    scope: Scope::Host,
    tier: Tier::User,
    menu: None,
};

static SPEAKER: Def = Def {
    key: "pad_speaker.enabled",
    label: "Controller Speaker",
    help: "",
    kind: Kind::Bool,
    default: settings::Value::Bool(false),
    env: None,
    flag: None,
    aliases: &[],
    parse_env: None,
    apply: Apply::Live,
    scope: Scope::Host,
    tier: Tier::User,
    menu: None,
};

struct Rig {
    agent: Agent,
    menu: Shared,
    hub: &'static hot_render::Hub,
    store: Arc<Store>,
    overlay: Arc<AtomicBool>,
}

impl Rig {
    fn new() -> Rig {
        let agent = Agent::new();
        let hub: &'static hot_render::Hub = Box::leak(Box::new(hot_render::Hub::new()));
        hub.declare_toggle("fog", true, "distance fog");
        hub.declare_param("bloom", 0.5, "bloom strength");
        crate::hot::add_render_methods(&agent, hub);
        let store = Arc::new(Store::defaults(&[&FOV, &LIGHTING, &SPEAKER]));
        add_settings_methods(&agent, Arc::clone(&store));
        let overlay = Arc::new(AtomicBool::new(true));
        add_perf_overlay(&agent, Arc::clone(&overlay));
        let menu = Shared::new(Menu::default());
        connect(&menu, &agent);
        Rig {
            agent,
            menu,
            hub,
            store,
            overlay,
        }
    }

    /// The game's side of a few frames: the menu sends, the executor
    /// answers between slices, the menu reads.
    fn frames(&self) {
        for _ in 0..4 {
            self.menu.lines();
            self.agent.between_slices(&mut Nothing, 0);
        }
    }

    fn keys(&self, navs: &[Nav]) {
        for n in navs {
            self.menu.key(Key::Nav(*n));
            self.frames();
        }
    }

    fn row(&self, label: &str) -> debug_menu::Row {
        self.frames();
        self.menu
            .lock()
            .menu
            .rows()
            .into_iter()
            .find(|r| r.label == label)
            .unwrap_or_else(|| panic!("no row {label}: {:?}", self.menu.lock().menu.rows()))
    }

    fn cursor_to(&self, label: &str) {
        self.frames();
        let (rows, at) = {
            let g = self.menu.lock();
            (g.menu.rows(), g.menu.cursor())
        };
        let i = rows.iter().position(|r| r.label == label).unwrap();
        let n = if i > at { Nav::Down } else { Nav::Up };
        for _ in 0..i.abs_diff(at) {
            self.menu.key(Key::Nav(n));
        }
    }
}

#[test]
fn the_render_page_reads_and_changes_the_hub_and_the_store() {
    let r = Rig::new();
    r.menu.key(Key::Toggle);
    r.keys(&[Nav::Select]);
    assert_eq!(r.row("fog").value, json!(true));
    assert_eq!(r.row("bloom").shown(), "0.50");
    assert_eq!(r.row("Field of View").shown(), "70");
    // Flip fog from the menu: the hub, which the renderer reads, has it.
    r.cursor_to("fog");
    r.keys(&[Nav::Right]);
    assert_eq!(r.hub.toggle("fog"), Some(false));
    assert_eq!(r.row("fog").value, json!(false));
    // A parameter steps by a tenth of its default.
    r.cursor_to("bloom");
    r.keys(&[Nav::Right]);
    assert_eq!(r.hub.param("bloom"), Some(0.55));
    // The FOV steps through the store, as the Video screen does.
    r.cursor_to("Field of View");
    r.keys(&[Nav::Right]);
    assert_eq!(r.store.get("graphics.fov"), Some(settings::Value::Int(90)));
    // Lighting applies after a restart: greyed, and refused.
    assert!(
        r.row("Lighting")
            .disabled
            .unwrap()
            .starts_with("applies after a restart")
    );
    r.cursor_to("Lighting");
    r.keys(&[Nav::Right]);
    assert_eq!(
        r.store.get("graphics.lighting"),
        Some(settings::Value::Int(0))
    );
    // pad_speaker is not under graphics.
    assert!(
        !r.menu
            .lock()
            .menu
            .rows()
            .iter()
            .any(|x| x.label == "Controller Speaker")
    );
    // And the other way: an `h2 agent render.toggle` shows in the menu.
    r.hub.set_toggle("fog", Some(true)).unwrap();
    let later = r.menu.now() + debug_menu::model::REFRESH;
    r.menu.lock().pump(later);
    r.agent.between_slices(&mut Nothing, 0);
    assert_eq!(r.row("fog").value, json!(true));
}

#[test]
fn the_perf_overlay_switch_is_shared() {
    let r = Rig::new();
    r.menu.key(Key::Toggle);
    r.cursor_to("Perf");
    r.keys(&[Nav::Select]);
    assert_eq!(r.row("perf overlay").value, json!(true));
    r.keys(&[Nav::Select]);
    assert!(!r.overlay.load(Ordering::Relaxed));
    assert_eq!(r.row("perf overlay").value, json!(false));
    // Frame times: no frame stats in a test, so the row says why.
    assert!(r.row("profile.counters").disabled.is_some());
}

#[test]
fn settings_methods_check_what_they_are_given() {
    let agent = Agent::new();
    let store = Arc::new(Store::defaults(&[&FOV, &LIGHTING, &SPEAKER]));
    add_settings_methods(&agent, Arc::clone(&store));
    let (tx, rx) = sync_channel(16);
    let conn = agent.connect(tx);
    let call = |m: &str, p: Value| {
        agent.submit(
            conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
        );
        agent.between_slices(&mut Nothing, 0);
        answer(&rx.try_recv().unwrap()).unwrap().1
    };
    let l = call("settings.list", json!({"prefix": "graphics."})).unwrap();
    let keys: Vec<&str> = l["settings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["key"].as_str().unwrap())
        .collect();
    assert_eq!(keys, ["graphics.fov", "graphics.lighting"]);
    assert_eq!(
        l["settings"][0]["steps"][1],
        json!({"value": 90, "label": "90"})
    );
    assert_eq!(l["settings"][1]["apply"], json!("restart"));
    assert_eq!(l["settings"][1]["choices"][1]["label"], json!("E3"));
    let set = call("settings.set", json!({"key": "graphics.fov", "value": 110})).unwrap();
    assert_eq!(
        (set["value"].clone(), set["changed"].clone()),
        (json!(110), json!(true))
    );
    assert!(call("settings.set", json!({"key": "graphics.fov", "value": 200})).is_err());
    assert!(
        call(
            "settings.set",
            json!({"key": "graphics.fov", "value": true})
        )
        .is_err()
    );
    assert!(call("settings.set", json!({"key": "nope", "value": 1})).is_err());
    let e3 = call(
        "settings.set",
        json!({"key": "graphics.lighting", "value": "e3"}),
    )
    .unwrap();
    assert_eq!(
        (e3["value"].clone(), e3["pending"].clone()),
        (json!(1), json!(true))
    );
    assert_eq!(answer(r#"{"jsonrpc":"2.0","method":"event"}"#), None);
}
