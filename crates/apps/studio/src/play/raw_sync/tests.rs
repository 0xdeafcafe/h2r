//! Geometry and pixels against the fake game (docs/editor/realtime-edits.md,
//! slice R5): the invented textured map with its metadata last (so a
//! grown section can be appended), its floor cluster `cluster:e1000004:0`
//! and its 4 by 4 `A8R8G8B8` floor bitmap. The fake game's `edit.batch`
//! and `edit.raw.append` record what they are sent, in order.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent::Access;
use agent_proto::{Endpoint, RpcError, code, sim::SessionKind};
use scenario_edit::fixture;
use serde_json::{Value, json};
use studio_proto::method as m;

use super::super::testing::Temp;
use super::super::testing::fake::{Game, until};
use super::*;
use crate::api::Api;
use crate::app::textures::BitmapAction;
use crate::app::{Action, App};

const CLUSTER: &str = "cluster:e1000004:0";
/// The pointer the fake game gives an appended block.
const POINTER: u32 = 0x0000_9000;

/// The fake game's raw methods: every call in order, and a switch that
/// makes the next batches answer `MISMATCH`.
#[derive(Clone, Default)]
struct RawGame {
    log: Arc<Mutex<Vec<(String, Value)>>>,
    mismatch: Arc<AtomicBool>,
}

impl RawGame {
    fn install(game: &Game) -> RawGame {
        let raw = RawGame::default();
        let (log, mismatch) = (raw.log.clone(), raw.mismatch.clone());
        game.agent
            .add_game_method("edit.batch", Access::Act, move |params, _| {
                log.lock().unwrap().push(("edit.batch".into(), params.clone()));
                if mismatch.load(Ordering::Relaxed) {
                    let first = &params["raw"][0];
                    return Err(RpcError {
                        code: code::MISMATCH,
                        message: "the game holds other bytes".into(),
                        data: Some(json!({"fields": [], "raw": [{
                            "owner": first["owner"], "offset": first["offset"],
                            "holds_hash": "0", "why": "the file view differs"
                        }]})),
                    });
                }
                let n = params["writes"].as_array().map_or(0, Vec::len);
                Ok(json!({"id": 1, "applied": n, "changes": [], "raw": [],
                    "evicted": params.get("evict").cloned().unwrap_or(json!([]))}))
            });
        let log = raw.log.clone();
        game.agent
            .add_game_method("edit.raw.append", Access::Act, move |params, _| {
                log.lock().unwrap().push(("edit.raw.append".into(), params.clone()));
                let len = agent_proto::hot::decode_base64(params["bytes"].as_str().unwrap())
                    .unwrap()
                    .len();
                Ok(json!({"id": 2, "pointer": POINTER, "len": len}))
            });
        raw
    }

    fn calls(&self, name: &str) -> Vec<Value> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter(|(n, _)| n == name)
            .map(|(_, v)| v.clone())
            .collect()
    }

    fn names(&self) -> Vec<String> {
        self.log.lock().unwrap().iter().map(|(n, _)| n.clone()).collect()
    }
}

/// The editor with the textured map open as `example`, paired in View
/// mode with a local game that has loaded it.
fn paired(tag: &str) -> (Temp, Game, RawGame, App, Play, Api) {
    let t = Temp::new(tag);
    let game = Game::start(tag, "example", SessionKind::Local);
    let raw = RawGame::install(&game);
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    let map = t.0.join("example.map");
    std::fs::write(&map, fixture::fixture_with_textured_bsp_last()).unwrap();
    app.dispatch(Action::OpenSource(map));
    assert_eq!(app.loaded_map(), Some("example"));
    game.edits.lock().unwrap().checksum = app.session.source_checksum().unwrap();
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "the loaded map and its journal", |p, a| {
        p.edits.journal_ready && p.edit_refusal(a).is_none()
    });
    (t, game, raw, app, play, Api::new())
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    api.call(app, method, params)
        .unwrap_or_else(|e| panic!("{method}: {e}"))
}

/// Open the floor cluster, select its `(4, 4, 0)` corner and lift it by 1.
fn lift_corner(api: &mut Api, app: &mut App) {
    call(api, app, m::GEOMETRY_OPEN, json!({"target": CLUSTER}));
    let read = call(api, app, m::GEOMETRY_READ, Value::Null);
    let id = read["verts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["position"] == json!([4.0, 4.0, 0.0]))
        .unwrap()["id"]
        .clone();
    call(api, app, m::MESH_SELECT, json!({"ids": [id]}));
    call(api, app, m::MESH_TRANSFORM, json!({"translate": [0.0, 0.0, 1.0]}));
}

fn section_owner() -> Value {
    json!({"section": {"tag": "0xe1000004", "kind": "cluster", "index": 0}})
}

fn bytes(v: &Value) -> Vec<u8> {
    agent_proto::hot::decode_base64(v.as_str().unwrap()).unwrap()
}

fn idle(play: &Play) -> bool {
    play.edits.sent.is_none() && !play.edits.raw.busy()
}

/// A `mesh.transform` goes as one batch: the stream bytes as `raw` with
/// their `was`, and the cluster's bounds as `writes`. Once acknowledged,
/// nothing is pending and nothing more is sent.
#[test]
fn a_mesh_transform_is_one_batch_with_raw_and_bounds() {
    let (_t, _game, raw, mut app, mut play, mut api) = paired("raw-move");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    lift_corner(&mut api, &mut app);
    until(&mut play, &mut app, "the batch", |p, _| {
        raw.calls("edit.batch").len() == 1 && idle(p)
    });
    let batch = &raw.calls("edit.batch")[0];
    let entries = batch["raw"].as_array().unwrap();
    assert!(!entries.is_empty(), "{batch}");
    for e in entries {
        assert_eq!(e["owner"], section_owner());
        assert_eq!(bytes(&e["bytes"]).len(), bytes(&e["was"]).len());
        assert_ne!(e["bytes"], e["was"]);
    }
    let writes = batch["writes"].as_array().unwrap();
    assert!(!writes.is_empty(), "the cluster bounds go in the same batch");
    assert!(writes.iter().all(|w| w["was"].is_string() && w["value"] != w["was"]));
    assert!(batch.get("evict").is_none());
    assert!(raw.calls("edit.raw.append").is_empty());
    until(&mut play, &mut app, "nothing pending", |_, a| a.pairing.pending.is_empty());
    for _ in 0..5 {
        play.push_edits(&mut app);
        play.update(&mut app);
    }
    assert_eq!(raw.calls("edit.batch").len(), 1);
}

/// Undo sends the bytes the game held before the edit: each raw entry and
/// field write of the second batch is the first one's `was`.
#[test]
fn undo_sends_the_old_bytes() {
    let (_t, _game, raw, mut app, mut play, mut api) = paired("raw-undo");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    lift_corner(&mut api, &mut app);
    until(&mut play, &mut app, "the edit", |p, _| {
        raw.calls("edit.batch").len() == 1 && idle(p)
    });
    let done = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(done["done"], true);
    until(&mut play, &mut app, "the undo", |p, _| {
        raw.calls("edit.batch").len() == 2 && idle(p)
    });
    let calls = raw.calls("edit.batch");
    let (edit, undo) = (&calls[0], &calls[1]);
    let entries = |b: &Value| -> Vec<(Value, u64, Vec<u8>, Vec<u8>)> {
        b["raw"]
            .as_array()
            .unwrap()
            .iter()
            .map(|e| (e["owner"].clone(), e["offset"].as_u64().unwrap(), bytes(&e["bytes"]), bytes(&e["was"])))
            .collect()
    };
    let back: Vec<_> = entries(edit)
        .into_iter()
        .map(|(o, at, now, was)| (o, at, was, now))
        .collect();
    assert_eq!(entries(undo), back);
    let fields = |b: &Value| -> Vec<(Value, Value, Value)> {
        b["writes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| (w["at"].clone(), w["value"].clone(), w["was"].clone()))
            .collect()
    };
    let back: Vec<_> = fields(edit).into_iter().map(|(at, v, w)| (at, w, v)).collect();
    assert_eq!(fields(undo), back);
    until(&mut play, &mut app, "nothing pending", |_, a| a.pairing.pending.is_empty());
    assert!(play.edits.raw.base.is_empty(), "the game holds the file again");
}

/// In View mode an edit is a Pending row, applies now, and nothing goes.
#[test]
fn view_mode_sends_nothing() {
    let (_t, _game, raw, mut app, mut play, mut api) = paired("raw-view");
    lift_corner(&mut api, &mut app);
    until(&mut play, &mut app, "the pending row", |_, a| {
        a.pairing.pending.iter().any(|r| r["doc"] == "geometry")
    });
    for _ in 0..5 {
        play.push_edits(&mut app);
        play.update(&mut app);
    }
    assert!(raw.calls("edit.batch").is_empty());
    assert!(raw.calls("edit.raw.append").is_empty());
    let row = app
        .pairing
        .pending
        .iter()
        .find(|r| r["doc"] == "geometry")
        .unwrap()
        .clone();
    assert_eq!(row["path"], CLUSTER);
    assert_eq!(row["effect"], "now");
    assert_eq!(row["target"]["target"], CLUSTER);
}

/// A `was` the game does not hold refuses the batch whole: the edit stays
/// in Pending as a conflict and is not sent again until the user sends.
#[test]
fn a_mismatch_leaves_the_batch_pending() {
    let (_t, _game, raw, mut app, mut play, mut api) = paired("raw-mismatch");
    raw.mismatch.store(true, Ordering::Relaxed);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    lift_corner(&mut api, &mut app);
    until(&mut play, &mut app, "the refused batch", |p, a| {
        raw.calls("edit.batch").len() == 1
            && idle(p)
            && a.pairing.pending.iter().any(|r| r["doc"] == "geometry" && r["conflict"] == true)
    });
    assert!(play.edits.raw.base.is_empty(), "nothing was taken as written");
    for _ in 0..5 {
        play.push_edits(&mut app);
        play.update(&mut app);
    }
    assert_eq!(raw.calls("edit.batch").len(), 1, "held until the user sends");
    // Sending again retries the same bytes.
    raw.mismatch.store(false, Ordering::Relaxed);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    until(&mut play, &mut app, "the retry", |p, a| {
        raw.calls("edit.batch").len() == 2 && idle(p) && a.pairing.pending.is_empty()
    });
    let calls = raw.calls("edit.batch");
    assert_eq!(calls[0]["raw"], calls[1]["raw"]);
}

/// A grown section goes as `edit.raw.append`, then one batch with the
/// pointer word it returned among the writes and `evict` naming the
/// section, in that order.
#[test]
fn a_grown_edit_sends_append_writes_evict_in_order() {
    let (_t, _game, raw, mut app, mut play, mut api) = paired("raw-grown");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    call(&mut api, &mut app, m::GEOMETRY_OPEN, json!({"target": CLUSTER}));
    call(
        &mut api,
        &mut app,
        m::MESH_APPLY,
        json!({"ops": [
            {"op": "mode", "mode": "face"},
            {"op": "select_all"},
            {"op": "extrude_faces", "offset": [0.0, 0.0, 1.0]}
        ]}),
    );
    let check = call(&mut api, &mut app, m::GEOMETRY_CHECK, Value::Null);
    assert_eq!(check["targets"][0]["class"], "grown_raw");
    until(&mut play, &mut app, "the append and the batch", |p, _| {
        raw.calls("edit.batch").len() == 1 && idle(p)
    });
    assert_eq!(raw.names(), ["edit.raw.append", "edit.batch"]);
    let append = &raw.calls("edit.raw.append")[0];
    assert!(!bytes(&append["bytes"]).is_empty());
    assert_eq!(append["align"], geometry_edit::RAW_ALIGN);
    let batch = &raw.calls("edit.batch")[0];
    let word = sync::hex(&POINTER.to_le_bytes());
    assert!(
        batch["writes"].as_array().unwrap().iter().any(|w| w["value"] == word.as_str()),
        "the block info points at the appended block: {batch}"
    );
    assert_eq!(batch["evict"], json!([section_owner()]));
    until(&mut play, &mut app, "nothing pending", |_, a| a.pairing.pending.is_empty());
    // Undo points the section back at its old block and evicts it again;
    // nothing is appended for it.
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    until(&mut play, &mut app, "the undo", |p, _| {
        raw.calls("edit.batch").len() == 2 && idle(p)
    });
    assert_eq!(raw.calls("edit.raw.append").len(), 1);
    let undo = &raw.calls("edit.batch")[1];
    assert_eq!(undo["evict"], json!([section_owner()]));
    assert!(
        undo["writes"].as_array().unwrap().iter().any(|w| w["was"] == word.as_str()),
        "the pointer word goes back from the appended block's: {undo}"
    );
}

/// A stroke sends its tiles: during the drag only mip 0's bytes of the
/// floor bitmap, as `pixels` entries with their `was`; the release sends
/// the rest, and nothing is pending after.
#[test]
fn a_stroke_sends_its_tiles() {
    let (_t, _game, raw, mut app, mut play, _api) = paired("raw-stroke");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    let floor = fixture::id(7);
    app.dispatch(Action::Bitmap(BitmapAction::Select { tag: floor, image: 0 }));
    app.textures.brush.size = 1.0;
    app.textures.fg = [1, 2, 3, 255];
    let layer = bitmap_edit::LayerId::new(0, 0, 0);
    app.dispatch(Action::Bitmap(BitmapAction::StrokeBegin { tag: floor, layer }));
    app.dispatch(Action::Bitmap(BitmapAction::StrokeTo { x: 0.5, y: 0.5 }));
    until(&mut play, &mut app, "the stroke's tiles", |p, _| {
        raw.calls("edit.batch").len() == 1 && idle(p)
    });
    let owner = json!({"pixels": {"tag": format!("{:#010x}", floor.0), "image": 0, "lod": 0}});
    let first = &raw.calls("edit.batch")[0];
    let entries = first["raw"].as_array().unwrap();
    assert!(!entries.is_empty(), "{first}");
    for e in entries {
        assert_eq!(e["owner"], owner);
        // Mip 0 of a 4 by 4 A8R8G8B8 image is its first 64 bytes.
        let end = e["offset"].as_u64().unwrap() + bytes(&e["bytes"]).len() as u64;
        assert!(end <= 64, "only mip 0 goes during the stroke: {e}");
        assert_eq!(bytes(&e["was"]).len(), bytes(&e["bytes"]).len());
    }
    assert!(first["writes"].as_array().unwrap().is_empty());
    app.dispatch(Action::Bitmap(BitmapAction::StrokeEnd));
    until(&mut play, &mut app, "the rebuilt mips, or nothing more", |p, a| {
        idle(p) && a.pairing.pending.is_empty()
    });
    for b in raw.calls("edit.batch").iter().skip(1) {
        for e in b["raw"].as_array().unwrap() {
            assert_eq!(e["owner"], owner);
            assert!(e["offset"].as_u64().unwrap() >= 64, "mip 0 went already: {e}");
        }
    }
}
