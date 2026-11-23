//! Level collision against the fake game (docs/editor/realtime-edits.md,
//! slice R7). The wants are primed straight into [`CollisionSync`], since
//! nothing in the Modeling tool makes a level collision edit yet; the fake
//! game's `edit.stage.put`, `edit.collision.stage` and
//! `edit.collision.commit` record what they are sent, in order.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use agent::Access;
use agent_proto::{Endpoint, RpcError, code, sim::SessionKind};
use scenario_edit::fixture;
use serde_json::{Value, json};

use super::super::testing::Temp;
use super::super::testing::fake::{Game, until};
use super::*;
use crate::app::{Action, App};

/// The structure the wants name.
const TAG: u32 = 0xe100_0004;
/// Where the fake game's arena puts a committed block: `ARENA + 0x1000 * i`
/// for the `i`th named block.
const ARENA: u32 = 0xa040_4000;

/// The fake game's collision methods: every call in order, and switches
/// that make the next commit answer `MISMATCH` and the next stage refuse.
#[derive(Clone, Default)]
struct CollisionGame {
    log: Arc<Mutex<Vec<(String, Value)>>>,
    mismatch: Arc<AtomicBool>,
    refuse: Arc<AtomicBool>,
    /// Whether the game has a BSP activation.
    activation: Arc<AtomicBool>,
}

fn names() -> Vec<&'static str> {
    COLLISION_BLOCKS
        .iter()
        .copied()
        .chain([COLLISION_LEAF_CLUSTERS, COLLISION_MOPP])
        .collect()
}

impl CollisionGame {
    fn install(game: &Game) -> CollisionGame {
        let g = CollisionGame::default();
        g.activation.store(true, Ordering::Relaxed);
        let buffers: Arc<Mutex<Vec<Vec<u8>>>> = Arc::default();
        let (log, bufs) = (g.log.clone(), buffers.clone());
        game.agent
            .add_game_method("edit.stage.put", Access::Act, move |params, _| {
                log.lock().unwrap().push(("edit.stage.put".into(), params.clone()));
                let bytes = agent_proto::hot::decode_base64(params["bytes"].as_str().unwrap()).unwrap();
                let mut bufs = bufs.lock().unwrap();
                let id = match params["stage"].as_u64() {
                    Some(id) => id as usize,
                    None => {
                        bufs.push(Vec::new());
                        bufs.len()
                    }
                };
                bufs[id - 1].extend_from_slice(&bytes);
                Ok(json!({"stage": id, "len": bufs[id - 1].len()}))
            });
        let (log, refuse, activation) = (g.log.clone(), g.refuse.clone(), g.activation.clone());
        let next = Arc::new(AtomicU64::new(100));
        game.agent
            .add_game_method("edit.collision.stage", Access::Act, move |params, _| {
                log.lock().unwrap().push(("edit.collision.stage".into(), params.clone()));
                if refuse.load(Ordering::Relaxed) {
                    return Err(RpcError {
                        code: agent_proto::sim::REFUSED,
                        message: "refused".into(),
                        data: Some(json!({"why": "the edit arena is full"})),
                    });
                }
                Ok(json!({"stage": next.fetch_add(1, Ordering::Relaxed), "bytes": 0, "blocks": {},
                    "arena": {"used": 0, "largest_free": 0},
                    "activation": activation.load(Ordering::Relaxed)}))
            });
        let (log, mismatch) = (g.log.clone(), g.mismatch.clone());
        game.agent
            .add_game_method("edit.collision.commit", Access::Act, move |params, _| {
                log.lock().unwrap().push(("edit.collision.commit".into(), params.clone()));
                if mismatch.load(Ordering::Relaxed) {
                    return Err(RpcError {
                        code: code::MISMATCH,
                        message: "the game's collision pairs are not the ones the commit was made against".into(),
                        data: Some(json!({"pairs": [{"name": "nodes", "root": 0,
                            "was": params["was"]["nodes"],
                            "holds": {"count": 7, "address": 0x1234}}]})),
                    });
                }
                // The new pairs: the `i`th named block holds `i + 1`
                // elements at its own arena extent.
                let mut pairs = serde_json::Map::new();
                for (i, n) in names().into_iter().enumerate() {
                    pairs.insert(n.to_owned(), json!({"count": i + 1, "address": ARENA + 0x1000 * i as u32}));
                }
                Ok(json!({"at": 1, "stage": params["stage"], "mode": params["mode"], "pairs": pairs,
                    "activated": params["activate"], "freed": []}))
            });
        g
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

/// Blocks whose `i`th named block holds `count(i)` elements of `fill`.
fn blocks(count: impl Fn(usize) -> u32, fill: u8) -> Blocks {
    let strides = [8usize, 16, 4, 4, 16, 8, 12, 16];
    Blocks {
        bsp: strides
            .iter()
            .enumerate()
            .map(|(i, s)| (count(i), vec![fill; s * count(i) as usize]))
            .collect(),
        leaves: (count(2), vec![fill; 8 * count(2) as usize]),
        mopp: vec![fill; 16],
    }
}

/// The file: one element a block, at `0x5000_0000 + 0x100 * i`.
fn file() -> (Arc<Blocks>, Pairs) {
    let b = blocks(|_| 1, 0x11);
    let mut pairs: Pairs = names()
        .into_iter()
        .enumerate()
        .map(|(i, n)| (n, (b.count(n), 0x5000_0000 + 0x100 * i as u32)))
        .collect();
    pairs.insert(COLLISION_MOPP, (16, 0x5000_0900));
    (Arc::new(b), pairs)
}

/// A solid added: every block grows.
fn added(fill: u8) -> CollisionWant {
    let (file, file_pairs) = file();
    CollisionWant {
        tag: TagId(TAG),
        blocks: Arc::new(blocks(|i| 2 + i as u32, fill)),
        file,
        file_pairs,
    }
}

/// Make the documents want `wants`, as a new level collision revision.
fn want(app: &mut App, play: &mut Play, wants: Vec<CollisionWant>) {
    app.sync_modeling();
    app.modeling.level_collision_rev += 1;
    play.edits.collision.prime(app.modeling.level_collision_rev, wants);
}

/// The editor with the textured map open as `example`, paired in View
/// mode with a local game that has loaded it.
fn paired(tag: &str) -> (Temp, Game, CollisionGame, App, Play) {
    let t = Temp::new(tag);
    let game = Game::start(tag, "example", SessionKind::Local);
    let coll = CollisionGame::install(&game);
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
    (t, game, coll, app, play)
}

fn idle(play: &Play) -> bool {
    play.edits.sent.is_none() && !play.edits.raw.busy() && !play.edits.collision.busy()
}

fn bytes(v: &Value) -> Vec<u8> {
    agent_proto::hot::decode_base64(v.as_str().unwrap()).unwrap()
}

fn collision_rows(app: &App) -> Vec<Value> {
    app.pairing
        .pending
        .iter()
        .filter(|r| r["doc"] == "collision")
        .cloned()
        .collect()
}

fn settle(play: &mut Play, app: &mut App) {
    for _ in 0..5 {
        play.push_edits(app);
        play.update(app);
    }
}

/// A solid added to the level goes as one stage of its blocks, then one
/// `append` commit against the file's pairs, asking for the activation the
/// stage reply offered. Once acknowledged, nothing is pending and nothing
/// more is sent.
#[test]
fn an_added_solid_is_one_stage_then_one_commit() {
    let (_t, _game, coll, mut app, mut play) = paired("coll-add");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    let w = added(0x22);
    want(&mut app, &mut play, vec![w.clone()]);
    until(&mut play, &mut app, "the commit", |p, _| {
        coll.calls("edit.collision.commit").len() == 1 && idle(p)
    });
    assert_eq!(coll.names(), ["edit.collision.stage", "edit.collision.commit"]);
    let stage = &coll.calls("edit.collision.stage")[0];
    assert_eq!(stage["target"], json!({"structure": format!("{TAG:#010x}")}));
    for (i, name) in COLLISION_BLOCKS.iter().enumerate() {
        assert_eq!(stage["blocks"][name]["count"], json!(w.blocks.bsp[i].0), "{name}");
        assert_eq!(bytes(&stage["blocks"][name]["bytes"]), w.blocks.bsp[i].1, "{name}");
    }
    assert_eq!(bytes(&stage["leaves"]["bytes"]), w.blocks.leaves.1);
    assert_eq!(bytes(&stage["mopp"]), w.blocks.mopp);
    let commit = &coll.calls("edit.collision.commit")[0];
    assert_eq!(commit["stage"], json!(100));
    assert_eq!(commit["mode"], "append");
    assert_eq!(commit["activate"], true);
    assert_eq!(commit["was"], pairs_json(&w.file_pairs));
    until(&mut play, &mut app, "nothing pending", |_, a| collision_rows(a).is_empty());
    settle(&mut play, &mut app);
    assert_eq!(coll.log.lock().unwrap().len(), 2);
}

/// Undo stages the file's blocks again and commits them as a `rebuild`
/// (the file's blocks are smaller), against the pairs the first commit's
/// receipt named.
#[test]
fn undo_stages_the_file_blocks_as_a_rebuild() {
    let (_t, _game, coll, mut app, mut play) = paired("coll-undo");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    let w = added(0x22);
    want(&mut app, &mut play, vec![w.clone()]);
    until(&mut play, &mut app, "the edit", |p, _| {
        coll.calls("edit.collision.commit").len() == 1 && idle(p)
    });
    want(&mut app, &mut play, Vec::new());
    until(&mut play, &mut app, "the undo", |p, _| {
        coll.calls("edit.collision.commit").len() == 2 && idle(p)
    });
    let stage = &coll.calls("edit.collision.stage")[1];
    for (i, name) in COLLISION_BLOCKS.iter().enumerate() {
        assert_eq!(bytes(&stage["blocks"][name]["bytes"]), w.file.bsp[i].1, "{name}");
    }
    let commit = &coll.calls("edit.collision.commit")[1];
    assert_eq!(commit["mode"], "rebuild");
    let receipt: Pairs = names()
        .into_iter()
        .enumerate()
        .map(|(i, n)| (n, (i as u32 + 1, ARENA + 0x1000 * i as u32)))
        .collect();
    assert_eq!(commit["was"], pairs_json(&receipt));
    until(&mut play, &mut app, "nothing pending", |_, a| collision_rows(a).is_empty());
    settle(&mut play, &mut app);
    assert_eq!(coll.calls("edit.collision.commit").len(), 2);
}

/// A game with no BSP activation still gets the commit, without
/// `activate`.
#[test]
fn without_an_activation_the_commit_does_not_ask_for_one() {
    let (_t, _game, coll, mut app, mut play) = paired("coll-noact");
    coll.activation.store(false, Ordering::Relaxed);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    want(&mut app, &mut play, vec![added(0x22)]);
    until(&mut play, &mut app, "the commit", |p, _| {
        coll.calls("edit.collision.commit").len() == 1 && idle(p)
    });
    assert_eq!(coll.calls("edit.collision.commit")[0]["activate"], false);
}

/// A `MISMATCH` holds the row as a conflict and sends nothing more until
/// the user sends again; then the commit names the pairs the game said it
/// holds.
#[test]
fn a_mismatch_holds_as_a_conflict_until_sent_again() {
    let (_t, _game, coll, mut app, mut play) = paired("coll-mismatch");
    coll.mismatch.store(true, Ordering::Relaxed);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    want(&mut app, &mut play, vec![added(0x22)]);
    until(&mut play, &mut app, "the conflict", |p, a| {
        idle(p) && collision_rows(a).first().is_some_and(|r| r["conflict"] == true)
    });
    settle(&mut play, &mut app);
    assert_eq!(coll.calls("edit.collision.commit").len(), 1);
    coll.mismatch.store(false, Ordering::Relaxed);
    play.retry_refused_fields();
    until(&mut play, &mut app, "the second commit", |p, _| {
        coll.calls("edit.collision.commit").len() == 2 && idle(p)
    });
    let commit = &coll.calls("edit.collision.commit")[1];
    assert_eq!(commit["was"]["nodes"], json!({"count": 7, "address": 0x1234}));
    until(&mut play, &mut app, "nothing pending", |_, a| collision_rows(a).is_empty());
}

/// A refused stage holds the row under Reload with the game's reason.
#[test]
fn a_refused_stage_goes_under_reload() {
    let (_t, _game, coll, mut app, mut play) = paired("coll-refused");
    coll.refuse.store(true, Ordering::Relaxed);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    want(&mut app, &mut play, vec![added(0x22)]);
    until(&mut play, &mut app, "the refusal", |p, a| {
        idle(p) && collision_rows(a).first().is_some_and(|r| r["effect"] == Effect::Reload.id())
    });
    assert_eq!(collision_rows(&app)[0]["why"], "the edit arena is full");
    settle(&mut play, &mut app);
    assert_eq!(coll.calls("edit.collision.stage").len(), 1);
    assert!(coll.calls("edit.collision.commit").is_empty());
}

/// Blocks over the inline limit go ahead in `edit.stage.put` chunks, and
/// the stage names them by offset into the buffer.
#[test]
fn large_blocks_go_in_stage_put_chunks() {
    let (_t, _game, coll, mut app, mut play) = paired("coll-large");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    let (file, file_pairs) = file();
    let big = blocks(|i| if i == 1 { 40_000 } else { 3 }, 0x33);
    let total = big.len();
    assert!(total > 2 * INLINE_BYTES);
    let w = CollisionWant {
        tag: TagId(TAG),
        blocks: Arc::new(big),
        file,
        file_pairs,
    };
    want(&mut app, &mut play, vec![w.clone()]);
    until(&mut play, &mut app, "the commit", |p, _| {
        coll.calls("edit.collision.commit").len() == 1 && idle(p)
    });
    let puts = coll.calls("edit.stage.put");
    assert_eq!(puts.len(), total.div_ceil(INLINE_BYTES));
    assert!(puts[0].get("stage").is_none());
    assert!(puts[1..].iter().all(|p| p["stage"] == json!(1)));
    let sent: Vec<u8> = puts.iter().flat_map(|p| bytes(&p["bytes"])).collect();
    assert_eq!(sent, w.blocks.joined());
    let stage = &coll.calls("edit.collision.stage")[0];
    let planes = &stage["blocks"]["planes"]["bytes"];
    assert_eq!(planes["stage"], json!(1));
    assert_eq!(planes["at"], json!(w.blocks.bsp[0].1.len()));
    assert_eq!(planes["len"], json!(w.blocks.bsp[1].1.len()));
}

/// In View mode a level collision edit is a Pending row that applies now,
/// and nothing goes.
#[test]
fn view_mode_sends_nothing() {
    let (_t, _game, coll, mut app, mut play) = paired("coll-view");
    want(&mut app, &mut play, vec![added(0x22)]);
    until(&mut play, &mut app, "the pending row", |_, a| !collision_rows(a).is_empty());
    settle(&mut play, &mut app);
    assert!(coll.names().is_empty());
    let row = &collision_rows(&app)[0];
    assert_eq!(row["effect"], Effect::Now.id());
    assert_eq!(row["path"], format!("structure_collision:{TAG:08x}:0"));
    assert_eq!(row["rebuild"], false);
}
