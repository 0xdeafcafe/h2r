//! The hot methods through the executor, as a client sends them: a
//! synthetic tag index in synthetic guest memory, and a hub with a
//! scratch override directory. No game, no GPU.

use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use runtime::guest::Permissions;
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

    fn call(&self, m: &str, p: Value) -> Value {
        self.agent.submit(
            self.conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
        );
        self.agent.between_slices(&mut Nothing, 0);
        serde_json::from_str(&self.rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
    }

    fn ok(&self, m: &str, p: Value) -> Value {
        let r = self.call(m, p);
        assert!(r.get("error").is_none(), "{m}: {r}");
        r["result"].clone()
    }
}

const HEADER: u32 = 0x0050_0000;
const ENTRIES: u32 = 0x0050_1000;
const SHAD_AT: u32 = 0x1000_1000;
const POST_AT: u32 = 0x1000_3000;
const COLORS_AT: u32 = 0x1000_4000;

/// One shader with two postprocess colours, laid out by its definition.
fn level() -> Guest {
    let defs = tags::defs::Defs::builtin();
    let shad = defs.group(map::FourCC(*b"shad")).unwrap();
    let post = shad.child("postprocess").unwrap();
    let mut g = Guest::new();
    g.map_zeroed(0x0040_0000, 0x0020_0000, Permissions::RW)
        .unwrap();
    g.map_zeroed(0x1000_0000, 0x0001_0000, Permissions::RW)
        .unwrap();
    g.set_u32(engine::points::TAG_INDEX.base, HEADER).unwrap();
    g.set_u32(engine::points::TAG_ENTRIES.base, ENTRIES)
        .unwrap();
    g.set_u32(HEADER + 0x0c, 0xe000_0000).unwrap();
    g.set_u32(HEADER + 0x18, 2).unwrap();
    g.write_bytes(ENTRIES, &[0xff; 16], 1).unwrap();
    g.set_u32(ENTRIES + 16, map::FourCC(*b"shad").as_u32())
        .unwrap();
    g.set_u32(ENTRIES + 20, 0xe111_0001).unwrap();
    g.set_u32(ENTRIES + 24, SHAD_AT).unwrap();
    g.set_u32(ENTRIES + 28, shad.size).unwrap();
    let pp = shad.field("postprocess").unwrap().offset;
    g.set_u32(SHAD_AT + pp, 1).unwrap();
    g.set_u32(SHAD_AT + pp + 4, POST_AT).unwrap();
    let colors = post.field("colors").unwrap().offset;
    g.set_u32(POST_AT + colors, 2).unwrap();
    g.set_u32(POST_AT + colors + 4, COLORS_AT).unwrap();
    g.set_u32(COLORS_AT + 4, 0xff80_4020).unwrap();
    g
}

#[test]
fn edit_methods_read_write_list_and_revert() {
    let guest = Arc::new(Mutex::new(level()));
    let agent = Agent::new();
    let g = Arc::clone(&guest);
    add_edit_methods(
        &agent,
        Editor::new(),
        Arc::new(move |f: &mut dyn FnMut(&mut Guest)| f(&mut g.lock().unwrap())),
    );
    let c = Client::new(&agent);

    let methods = c.ok(agent_proto::method::METHODS, Value::Null);
    let listed = methods.to_string();
    for m in method::ALL.iter().filter(|m| m.starts_with("edit.")) {
        assert!(listed.contains(m), "{m} in {listed}");
    }

    let tags = c.ok(method::EDIT_TAGS, json!({"group": "shad"}));
    assert_eq!(tags["total"], 1, "{tags}");
    assert_eq!(tags["tags"][0]["tag"], "0xe1110001");

    let path = "postprocess[0].colors[1].color";
    let v = c.ok(method::EDIT_GET, json!({"tag": "0xe1110001", "path": path}));
    assert_eq!(v["value"], 0xff80_4020u32, "{v}");

    let ch = c.ok(
        method::EDIT_SET,
        json!({"tag": "0xe1110001", "path": path, "value": "0xff0000ff"}),
    );
    assert_eq!(ch["id"], 1);
    assert_eq!(
        guest.lock().unwrap().u32(COLORS_AT + 4).unwrap(),
        0xff00_00ff
    );

    let list = c.ok(method::EDIT_CHANGES, Value::Null);
    assert_eq!(list["changes"][0]["live"], true, "{list}");

    let r = c.ok(method::EDIT_REVERT, Value::Null);
    assert_eq!(r["reverted"][0]["id"], 1, "{r}");
    assert_eq!(
        guest.lock().unwrap().u32(COLORS_AT + 4).unwrap(),
        0xff80_4020
    );

    // Errors carry the right codes.
    let e = c.call(
        method::EDIT_GET,
        json!({"tag": "0xe1110001", "path": "nope"}),
    );
    assert_eq!(e["error"]["code"], code::INVALID_PARAMS, "{e}");
    let e = c.call(method::EDIT_SET, json!({"tag": "0xe1110001", "path": path}));
    assert_eq!(e["error"]["code"], code::INVALID_PARAMS, "{e}");
    let e = c.call(method::EDIT_GET, json!({"path": path}));
    assert_eq!(e["error"]["code"], code::INVALID_PARAMS, "{e}");
    guest
        .lock()
        .unwrap()
        .set_u32(engine::points::TAG_INDEX.base, 0)
        .unwrap();
    let e = c.call(method::EDIT_TAGS, Value::Null);
    assert_eq!(e["error"]["code"], code::UNAVAILABLE, "{e}");
}

#[cfg(feature = "render-overrides")]
fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("agent-hot-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

const PROGRAM: &str = "00000000000000ab";

#[cfg(feature = "render-overrides")]
#[test]
fn render_methods_drive_the_hub() {
    let hub: &'static Hub = Box::leak(Box::new(Hub::new()));
    let agent = Agent::new();
    add_render_methods(&agent, hub);
    let c = Client::new(&agent);

    let dir = scratch("overrides");
    let r = c.ok(
        method::RENDER_RELOAD,
        json!({"dir": dir.display().to_string()}),
    );
    assert_eq!(r["loaded"], json!([]), "{r}");
    std::fs::write(
        dir.join(format!("{PROGRAM}_tint.wgsl")),
        "struct V { @builtin(position) p: vec4f }
@vertex fn vs_main(@location(0) pos: vec4f) -> V { var o: V; o.p = pos; return o; }
@fragment fn fs_main() -> @location(0) vec4f { return vec4f({{tint=0.5}}); }",
    )
    .unwrap();
    let r = c.ok(method::RENDER_RELOAD, Value::Null);
    assert_eq!(r["loaded"], json!([PROGRAM]), "{r}");
    let s = c.ok(method::RENDER_OVERRIDES, Value::Null);
    assert_eq!(s["overrides"][0]["state"], "pending", "{s}");
    assert_eq!(s["overrides"][0]["params"], json!(["tint"]));

    // Parameters: list, one, set, reset.
    let p = c.ok(method::RENDER_PARAM, Value::Null);
    assert_eq!(p["params"][0]["name"], "tint", "{p}");
    let p = c.ok(method::RENDER_PARAM, json!({"name": "tint", "value": 0.25}));
    assert_eq!(p["value"], 0.25);
    assert!(hub.source(0xab).unwrap().0.contains("vec4f(0.25)"));
    let p = c.ok(method::RENDER_PARAM, json!({"name": "tint", "value": null}));
    assert_eq!(p["value"], 0.5);
    let e = c.call(method::RENDER_PARAM, json!({"name": "nope", "value": 1}));
    assert_eq!(e["error"]["code"], code::INVALID_PARAMS);

    // Toggles.
    let t = c.ok(
        method::RENDER_TOGGLE,
        json!({"name": "overrides", "on": false}),
    );
    assert_eq!(t["on"], false);
    assert!(hub.source(0xab).is_none());
    c.ok(
        method::RENDER_TOGGLE,
        json!({"name": "overrides", "on": null}),
    );
    assert!(hub.source(0xab).is_some());
    let e = c.call(method::RENDER_TOGGLE, json!({"name": "overrides", "on": 1}));
    assert_eq!(e["error"]["code"], code::INVALID_PARAMS);

    // Programs and dumps: what the provider publishes.
    let e = c.call(method::RENDER_DUMP, json!({"program": PROGRAM}));
    assert_eq!(
        e["error"]["code"],
        code::INVALID_PARAMS,
        "not translated: {e}"
    );
    hub.publish(
        0xab,
        hot_render::ProgramInfo {
            shader: 7,
            vertex: "microcode".into(),
            textures: 1,
            discards: false,
            warnings: 0,
        },
    );
    hub.report(0xab, Ok(()));
    let p = c.ok(method::RENDER_PROGRAMS, json!({"vertex": "microcode"}));
    assert_eq!(p["total"], 1, "{p}");
    assert_eq!(p["programs"][0]["program"], PROGRAM);
    assert_eq!(p["programs"][0]["override"]["state"], "applied");
    assert_eq!(
        c.ok(method::RENDER_PROGRAMS, json!({"vertex": "pretransformed"}))["total"],
        0
    );
    let d = c.ok(method::RENDER_DUMP, json!({"program": PROGRAM}));
    assert_eq!(d["to"], dir.join("translated").display().to_string(), "{d}");
    let queued = hub.take_dumps();
    assert_eq!(queued[0].program, Some(0xab));
}
