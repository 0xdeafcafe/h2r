//! The hot methods through the executor, as a client sends them: a
//! synthetic tag index in synthetic guest memory, and a hub with a
//! scratch override directory. No game, no GPU.

use std::sync::mpsc::{Receiver, sync_channel};
use std::time::Duration;

use runtime::guest::Permissions;
use serde_json::{Value, json};

use super::*;
use crate::{ConnId, GameStatus, Reader};

#[derive(Default)]
struct LevelStatus(GameStatus);

impl Reader for LevelStatus {
    fn status(&mut self) -> GameStatus {
        self.0.clone()
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
        self.call_at(m, p, &mut LevelStatus::default())
    }

    fn call_at(&self, m: &str, p: Value, level: &mut LevelStatus) -> Value {
        self.agent.submit(
            self.conn,
            &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
        );
        self.agent.between_slices(level, 0);
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
    g.set_u32(guest_layout::points::TAG_INDEX.base, HEADER).unwrap();
    g.set_u32(guest_layout::points::TAG_ENTRIES.base, ENTRIES)
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
        |_| SessionView {
            kind: agent_proto::sim::SessionKind::Local,
            ..Default::default()
        },
        |_| Some("fixture.map".into()),
        false,
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
        .set_u32(guest_layout::points::TAG_INDEX.base, 0)
        .unwrap();
    let e = c.call(method::EDIT_TAGS, Value::Null);
    assert_eq!(e["error"]["code"], code::UNAVAILABLE, "{e}");
}

#[test]
fn conditional_batch_validates_all_fields_and_reverts_one_id() {
    let guest = Arc::new(Mutex::new(level()));
    let g = guest.clone();
    let agent = Agent::new();
    let source = Arc::new(Mutex::new(None));
    let current = Arc::clone(&source);
    add_edit_methods(
        &agent,
        Editor::new(),
        Arc::new(move |f| f(&mut g.lock().unwrap())),
        |_| SessionView {
            kind: agent_proto::sim::SessionKind::Local,
            ..Default::default()
        },
        move |_| current.lock().unwrap().clone(),
        false,
    );
    let c = Client::new(&agent);
    let map = c.ok(method::EDIT_MAP, json!({"known_epoch": null}));
    assert_eq!(map["scenario"], "0xe0000000");
    assert!(
        map["loaded"]
            .as_array()
            .unwrap()
            .contains(&json!(0xe1110001u32))
    );
    assert!(map["file"].is_null());
    *source.lock().unwrap() = Some("served-fixture.map".into());
    let same = c.ok(method::EDIT_MAP, json!({"known_epoch": map["epoch"]}));
    assert_eq!(same["file"], "served-fixture.map");
    assert!(same.get("loaded").is_none());
    let typed = json!({"tag":"0xe1110001","path":"postprocess[0].colors[1].color","was":0xff804020u32,"value":7});
    let raw = json!({"at":SHAD_AT,"was":"0000","value":"1122"});
    let bad = json!({"at":SHAD_AT,"was":"ffff","value":"1122"});
    let err = c.call(method::EDIT_BATCH, json!({"writes":[typed,bad]}));
    assert_eq!(err["error"]["code"], code::MISMATCH);
    assert_eq!(err["error"]["data"]["fields"][0]["holds_bytes"], "0000");
    let outside = json!({"at":ENTRIES,"was":"ff","value":"01"});
    assert_eq!(
        c.call(method::EDIT_BATCH, json!({"writes":[typed,outside]}))["error"]["code"],
        code::INVALID_PARAMS
    );
    assert_eq!(
        guest.lock().unwrap().u32(COLORS_AT + 4).unwrap(),
        0xff804020
    );
    assert_eq!(
        c.ok(method::EDIT_CHANGES, Value::Null)["changes"],
        json!([])
    );
    let reply = c.ok(
        method::EDIT_BATCH,
        json!({"writes":[typed,raw], "label":"Two fields"}),
    );
    assert_eq!(reply["applied"], 2);
    let journal = c.ok(method::EDIT_CHANGES, Value::Null);
    assert_eq!(journal["changes"].as_array().unwrap().len(), 1);
    assert_eq!(journal["changes"][0]["label"], "Two fields");
    assert_eq!(journal["changes"][0]["fields"].as_array().unwrap().len(), 2);
    assert_eq!(reply["changes"][1]["id"], reply["id"]);
    assert_eq!(guest.lock().unwrap().u32(SHAD_AT).unwrap(), 0x2211);
    c.ok(method::EDIT_REVERT, json!({"id":reply["id"]}));
    assert_eq!(guest.lock().unwrap().u32(SHAD_AT).unwrap(), 0);
    assert_eq!(
        guest.lock().unwrap().u32(COLORS_AT + 4).unwrap(),
        0xff804020
    );
}

#[test]
fn boundary_reads_return_current_bytes_and_reject_stale_epochs() {
    let guest = Arc::new(Mutex::new(level()));
    let g = guest.clone();
    let agent = Agent::new();
    add_edit_methods(
        &agent,
        Editor::new(),
        Arc::new(move |f| f(&mut g.lock().unwrap())),
        |_| SessionView::default(),
        |_| None,
        false,
    );
    let c = Client::new(&agent);
    let map = c.ok(method::EDIT_MAP, Value::Null);
    let epoch = map["epoch"].as_u64().unwrap();
    let p = json!({"at": SHAD_AT, "len": 2, "epoch": epoch});
    assert_eq!(c.ok(method::EDIT_READ, p.clone())["holds_bytes"], "0000");
    guest.lock().unwrap().set_u16(SHAD_AT, 0x4321).unwrap();
    assert_eq!(
        c.ok(method::EDIT_READ, p),
        json!({"at": SHAD_AT, "holds_bytes": "2143"})
    );
    for bad in [
        json!({"at": SHAD_AT, "len": 0, "epoch": epoch}),
        json!({"at": SHAD_AT - 1, "len": 2, "epoch": epoch}),
        json!({"at": SHAD_AT, "len": 2}),
    ] {
        assert_eq!(
            c.call(method::EDIT_READ, bad)["error"]["code"],
            code::INVALID_PARAMS
        );
    }
    let pp = tags::defs::Defs::builtin()
        .group(map::FourCC(*b"shad"))
        .unwrap()
        .field("postprocess")
        .unwrap()
        .offset;
    guest.lock().unwrap().set_u32(POST_AT, 0x7654).unwrap();
    let path = format!("+{pp:#x}/0x10[0].+0:bytes[2]");
    let field = json!({"tag": "0xe1110001", "path": path, "epoch": epoch});
    let reply = c.ok(method::EDIT_GET, field.clone());
    assert_eq!(reply["addr"], format!("{POST_AT:#010x}"));
    assert_eq!(reply["value"], "5476");
    agent.live_cut();
    for (method, params) in [
        (
            method::EDIT_READ,
            json!({"at": SHAD_AT, "len": 2, "epoch": epoch}),
        ),
        (method::EDIT_GET, field),
    ] {
        assert_eq!(
            c.call(method, params)["error"]["code"],
            code::INVALID_REQUEST
        );
    }
}

#[test]
fn bsp_switch_keeps_the_journal_until_a_full_map_cut() {
    let guest = Arc::new(Mutex::new(level()));
    let agent = Agent::new();
    add_edit_methods(
        &agent,
        Editor::new(),
        Arc::new(move |f| f(&mut guest.lock().unwrap())),
        |_| SessionView {
            kind: agent_proto::sim::SessionKind::Local,
            ..Default::default()
        },
        |_| None,
        false,
    );
    let c = Client::new(&agent);
    let mut level = LevelStatus(GameStatus {
        map: Some("fixture".into()),
        bsp: Some(0),
        ..Default::default()
    });
    let map = c.call_at(method::EDIT_MAP, Value::Null, &mut level);
    let write = json!({"at": SHAD_AT, "value": "41"});
    let result = c.call_at(method::EDIT_BATCH, json!({"writes": [write]}), &mut level);
    assert_eq!(result["result"]["applied"], 1);
    level.0.bsp = Some(1);
    let switched = c.call_at(method::EDIT_MAP, Value::Null, &mut level);
    assert_ne!(map["result"]["epoch"], switched["result"]["epoch"]);
    let changes = c.call_at(method::EDIT_CHANGES, Value::Null, &mut level);
    assert_eq!(changes["result"]["changes"][0]["fields"][0]["live"], true);
    agent.live_cut();
    let changes = c.call_at(method::EDIT_CHANGES, Value::Null, &mut level);
    assert_eq!(changes["result"]["changes"], json!([]));
}

#[test]
fn multiplayer_host_with_unknown_peer_count_explains_and_refuses_writes() {
    let guest = Arc::new(Mutex::new(level()));
    let agent = Agent::new();
    add_edit_methods(
        &agent,
        Editor::new(),
        Arc::new(move |f| f(&mut guest.lock().unwrap())),
        |_| SessionView {
            kind: agent_proto::sim::SessionKind::MpHost,
            peers: None,
            ..Default::default()
        },
        |_| None,
        false,
    );
    let c = Client::new(&agent);
    let map = c.ok(method::EDIT_MAP, Value::Null);
    assert_eq!(map["session"]["writes"], false);
    assert_eq!(
        map["session"]["why"],
        "multiplayer host peer count is unknown"
    );
    for (m, params) in [
        (
            method::EDIT_SET,
            json!({"tag":"0xe1110001","path":"+0:u8","value":7}),
        ),
        (
            method::EDIT_BATCH,
            json!({"writes": [{"at": SHAD_AT, "value": "07"}], "confirm_peers": 1}),
        ),
        (method::EDIT_REVERT, Value::Null),
    ] {
        let err = c.call(m, params);
        assert_eq!(err["error"]["code"], code::INVALID_REQUEST);
        assert_eq!(err["error"]["message"], map["session"]["why"]);
    }
}

#[test]
fn an_unknown_multiplayer_peer_count_refuses_even_a_confirmed_edit() {
    let host = SessionView {
        kind: agent_proto::sim::SessionKind::MpHost,
        peers: None,
        ..Default::default()
    }
    .settle();
    for p in [json!({}), json!({"confirm_peers": 1})] {
        let error = permit(host.clone(), &p, 0).unwrap_err();
        assert_eq!(error.code, code::INVALID_REQUEST);
        assert!(error.message.contains("peer count"));
    }
}

#[test]
fn every_edit_write_checks_the_current_session() {
    use agent_proto::sim::SessionKind::{CoopGuest, CoopHost, Local, MpClient, MpHost, Unknown};
    let guest = Arc::new(Mutex::new(level()));
    let g = guest.clone();
    let kind = Arc::new(Mutex::new(Local));
    let current = kind.clone();
    let agent = Agent::new();
    add_edit_methods(
        &agent,
        Editor::new(),
        Arc::new(move |f| f(&mut g.lock().unwrap())),
        move |_| SessionView {
            kind: *current.lock().unwrap(),
            peers: Some(3),
            ..Default::default()
        },
        |_| None,
        false,
    );
    let c = Client::new(&agent);
    let p = json!({"tag":"0xe1110001","path":"postprocess[0].colors[1].color","value":7});
    c.ok(method::EDIT_SET, p.clone());
    *kind.lock().unwrap() = MpHost;
    let mut confirmed = p.clone();
    confirmed["confirm_peers"] = json!(2);
    assert_eq!(
        c.call(method::EDIT_SET, confirmed.clone())["error"]["code"],
        code::INVALID_REQUEST
    );
    confirmed["confirm_peers"] = json!(3);
    c.ok(method::EDIT_SET, confirmed);
    for session in [CoopHost, CoopGuest, MpClient, Unknown, Local] {
        *kind.lock().unwrap() = session;
        if session == Local {
            agent.set_live_session_probe(|_| true);
        }
        for (m, params) in [
            (method::EDIT_SET, p.clone()),
            (method::EDIT_BATCH, json!({"writes":[p]})),
            (method::EDIT_BATCH, json!({"raw":[{"owner":{"section":
                {"tag":"0xe1110001","kind":"cluster","index":0}},"offset":0,"bytes":"AA=="}]})),
            (method::EDIT_RAW_APPEND, json!({"bytes":"AA=="})),
            (method::EDIT_REVERT, Value::Null),
        ] {
            assert_eq!(
                c.call(m, params)["error"]["code"],
                code::INVALID_REQUEST,
                "{m} {session:?}"
            );
        }
        assert_eq!(guest.lock().unwrap().u32(COLORS_AT + 4).unwrap(), 7);
    }
}

/// Staged bytes belong to the connection that staged them and are named
/// by `{stage, at, len}`; a game without raw files refuses raw writes as
/// unavailable, and `edit.overlay` reports an empty overlay.
#[test]
fn stages_belong_to_their_connection_and_raw_needs_the_glue() {
    let guest = Arc::new(Mutex::new(level()));
    let g = guest.clone();
    let agent = Agent::new();
    add_edit_methods(
        &agent,
        Editor::new(),
        Arc::new(move |f| f(&mut g.lock().unwrap())),
        |_| SessionView {
            kind: agent_proto::sim::SessionKind::Local,
            ..Default::default()
        },
        |_| None,
        false,
    );
    let (a, b) = (Client::new(&agent), Client::new(&agent));
    let s = a.ok(method::EDIT_STAGE_PUT, json!({"bytes": "AQID"}));
    assert_eq!(s["len"], 3, "{s}");
    let s = a.ok(method::EDIT_STAGE_PUT, json!({"stage": s["stage"], "bytes": "BA=="}));
    assert_eq!(s["len"], 4, "{s}");
    let staged = json!({"stage": s["stage"], "at": 1, "len": 2});
    let raw = |bytes: Value| json!({"raw": [{"owner": {"section":
        {"tag": "0xe1110001", "kind": "cluster", "index": 0}}, "offset": 0, "bytes": bytes}]});
    assert_eq!(
        b.call(method::EDIT_BATCH, raw(staged.clone()))["error"]["code"],
        code::INVALID_PARAMS,
        "another connection's stage"
    );
    let past = json!({"stage": s["stage"], "at": 3, "len": 2});
    assert_eq!(a.call(method::EDIT_BATCH, raw(past))["error"]["code"], code::INVALID_PARAMS);
    assert_eq!(a.call(method::EDIT_BATCH, raw(staged))["error"]["code"], code::UNAVAILABLE);
    assert_eq!(a.call(method::EDIT_BATCH, raw(json!("not base64!")))["error"]["code"], code::INVALID_PARAMS);
    let o = a.ok(method::EDIT_OVERLAY, Value::Null);
    assert_eq!(o["entries"], 0, "{o}");
    assert_eq!(o["owners"], json!([]));
}

/// `moves` in `edit.batch` grows the shader's colours block into the edit
/// arena; `edit.arena` lists it, a path read follows it, `edit.revert`
/// moves it back. A game without the arena answers `UNAVAILABLE`.
#[test]
fn a_block_move_goes_through_edit_batch_and_shows_in_edit_arena() {
    let guest = Arc::new(Mutex::new(level()));
    let g = guest.clone();
    let agent = Agent::new();
    add_edit_methods(
        &agent,
        Editor::new().with_arena(hot_tags::arena::shared()),
        Arc::new(move |f| f(&mut g.lock().unwrap())),
        |_| SessionView {
            kind: agent_proto::sim::SessionKind::Local,
            ..Default::default()
        },
        |_| None,
        false,
    );
    let c = Client::new(&agent);
    let post = tags::defs::Defs::builtin()
        .group(map::FourCC(*b"shad"))
        .unwrap()
        .child("postprocess")
        .unwrap();
    let stride = post.child("colors").unwrap().size;
    let root = POST_AT + post.field("colors").unwrap().offset;
    let mut block = vec![0u8; 3 * stride as usize];
    guest
        .lock()
        .unwrap()
        .read_bytes(COLORS_AT, &mut block[..2 * stride as usize], 1)
        .unwrap();
    let mv = |was_address: u32| {
        json!({"moves": [{"root": {"tag": "0xe1110001", "path": "postprocess[0].colors"},
            "stride": stride, "count": 3, "capacity": 4,
            "bytes": agent_proto::hot::encode_base64(&block),
            "was": {"count": 2, "address": format!("{was_address:#x}")}}]})
    };
    // A stale `was` is a mismatch naming the root's pair.
    let r = c.call(method::EDIT_BATCH, mv(COLORS_AT + 0x10));
    assert_eq!(r["error"]["code"], code::MISMATCH, "{r}");
    assert_eq!(r["error"]["data"]["moves"][0]["holds"]["address"], COLORS_AT);

    let r = c.ok(method::EDIT_BATCH, mv(COLORS_AT));
    let base = hot_tags::arena::WINDOW.start;
    assert_eq!(r["moves"][0]["address"], base, "{r}");
    assert_eq!(guest.lock().unwrap().u32(root + 4).unwrap(), base);
    let a = c.ok(method::EDIT_ARENA, Value::Null);
    assert_eq!(a["moves"][0]["address"], base, "{a}");
    assert_eq!(a["moves"][0]["capacity"], 4);
    assert_eq!(a["moves"][0]["tag"], "0xe1110001");
    let v = c.ok(method::EDIT_GET, json!({"tag": "0xe1110001", "path": "postprocess[0].colors[2]"}));
    assert_eq!(v["addr"], json!(format!("{:#010x}", base + 2 * stride)), "{v}");
    let r = c.ok(method::EDIT_REVERT, Value::Null);
    assert_eq!(r["reverted"][0]["move"]["address"], base, "{r}");
    assert_eq!(guest.lock().unwrap().u32(root + 4).unwrap(), COLORS_AT);
    assert_eq!(c.ok(method::EDIT_ARENA, Value::Null)["used"], 0);

    // Without the arena.
    let bare = Agent::new();
    let g = guest.clone();
    add_edit_methods(
        &bare,
        Editor::new(),
        Arc::new(move |f| f(&mut g.lock().unwrap())),
        |_| SessionView {
            kind: agent_proto::sim::SessionKind::Local,
            ..Default::default()
        },
        |_| None,
        false,
    );
    let b = Client::new(&bare);
    assert_eq!(b.call(method::EDIT_ARENA, Value::Null)["error"]["code"], code::UNAVAILABLE);
    assert_eq!(b.call(method::EDIT_BATCH, mv(COLORS_AT))["error"]["code"], code::UNAVAILABLE);
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
