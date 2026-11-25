//! Added and removed placements against the agent crate's server, with an
//! invented game behind it whose `world.placement.edit` keeps the weapons
//! block as the real host does: a count, the slots the loaded map gave it,
//! and the bytes a removal leaves past the count.

use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use agent::Access;
use agent_proto::sim::{PlacementEditParams, REFUSED, SessionKind, Was, method::PLACEMENT_EDIT};
use agent_proto::{RpcError, code};
use scenario_edit::{BlockKind, Command, Value as FieldValue};
use serde_json::{Value, json};

use crate::app::{Action, App};
use crate::play::link::Purpose;
use crate::play::testing::Temp;
use crate::play::testing::fake::{Game, until};
use crate::play::tests::paired_with;
use crate::play::{Apply, Play, sync, tag_sync};

const WEAPONS: BlockKind = BlockKind::Weapons;

enum Answer {
    /// Write the records and make no object for the created entries.
    NotCreated(&'static str),
    /// Apply the edit and never answer: the call times out.
    Lose,
}

/// The game's weapons block.
#[derive(Default)]
struct Block {
    count: usize,
    room: usize,
    /// Every slot the block has room for, in hex; empty for never written.
    slots: Vec<String>,
    calls: Vec<Value>,
    answer: Option<Answer>,
}

fn refusal(code: i64, why: String, data: Value) -> RpcError {
    let mut data = data;
    data["why"] = json!(why);
    RpcError { code, message: why, data: Some(data) }
}

fn edit(b: &mut Block, p: PlacementEditParams) -> Result<Vec<Value>, RpcError> {
    assert_eq!(p.block, "weapons");
    if p.was_count as usize != b.count {
        let why = format!("weapons holds {} records, not {}", b.count, p.was_count);
        return Err(refusal(code::MISMATCH, why, json!({"count": b.count})));
    }
    let count = p.count as usize;
    if count > b.room {
        let why = format!("weapons has room for {} records; the edit needs {count}", b.room);
        return Err(refusal(REFUSED, why, json!({"need": count, "capacity": b.room})));
    }
    for r in &p.records {
        if let Some(Was::Bytes(was)) = &r.was
            && b.slots.get(usize::from(r.index)) != Some(was)
        {
            let why = format!("record {} changed", r.index);
            return Err(refusal(code::MISMATCH, why, json!({"records": [{"index": r.index}]})));
        }
    }
    for r in p.records {
        let at = usize::from(r.index);
        if b.slots.len() <= at {
            b.slots.resize(at + 1, String::new());
        }
        b.slots[at] = r.bytes;
    }
    assert!(
        b.slots.iter().take(count).all(|s| !s.is_empty()),
        "every slot under the new count holds a record"
    );
    b.count = count;
    Ok(p.create.iter().map(|&index| json!({
        "index": index, "object": format!("weapon.{:08x}", 0x0040_0000 | u32::from(index)),
        "linked": true, "created": true,
    })).collect())
}

/// A paired editor in Edit mode, on a game whose weapons block has `room` slots.
fn pair(tag: &str, room: usize) -> (Temp, Game, App, Play, Arc<Mutex<Block>>) {
    let state = Arc::new(Mutex::new(Block::default()));
    let game_state = Arc::clone(&state);
    let (t, game, mut app, mut play) = paired_with(tag, "example", SessionKind::Local, move |game| {
        game.agent.add_game_method(PLACEMENT_EDIT, Access::Act, move |params, ctx| {
            let mut b = game_state.lock().unwrap();
            b.calls.push(params.clone());
            let answer = b.answer.take();
            let p: PlacementEditParams = serde_json::from_value(params)
                .map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
            let mut created = edit(&mut b, p)?;
            match answer {
                Some(Answer::NotCreated(why)) => {
                    for c in &mut created {
                        c["created"] = json!(false);
                        c["linked"] = json!(false);
                        c["why"] = json!(why);
                        c.as_object_mut().unwrap().remove("object");
                    }
                }
                Some(Answer::Lose) => {
                    ctx.defer(3, Box::new(|_| None));
                    return Ok(Value::Null);
                }
                None => {}
            }
            Ok(json!({"at": ctx.at, "count": b.count, "capacity": b.room,
                "deleted": [], "relinked": [], "created": created}))
        });
    });
    {
        let source = app.session.scenario().unwrap().block(WEAPONS).source_entries();
        let mut b = state.lock().unwrap();
        b.slots = source.iter().map(|e| sync::hex(e)).collect();
        b.count = b.slots.len();
        b.room = room;
    }
    app.session.set_allow_growth(true);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    play.update(&mut app);
    (t, game, app, play, state)
}

fn calls(state: &Arc<Mutex<Block>>) -> Vec<Value> {
    state.lock().unwrap().calls.clone()
}

/// The `n`th transaction has its answer and Pending is empty.
fn settled(play: &mut Play, app: &mut App, state: &Arc<Mutex<Block>>, n: usize, what: &str) {
    until(play, app, what, |p, a| {
        calls(state).len() == n && !p.objects.placements.in_flight() && a.pairing.pending.is_empty()
    });
    let mut api = crate::api::Api::new();
    let pending = api.call(app, studio_proto::method::MIRROR_PENDING, Value::Null).unwrap();
    assert_eq!(pending["pending"], json!([]), "{what}");
}

/// Frames, pushes and an answered control call: whatever would be sent again is.
fn idle(play: &mut Play, app: &mut App) {
    for _ in 0..20 {
        play.update(app);
        play.push_edits(app);
        thread::sleep(Duration::from_millis(5));
    }
    let reports = |app: &App| app.log().iter()
        .filter(|line| line.status.text().starts_with("session.status:"))
        .count();
    let before = reports(app);
    play.call(agent_proto::method::STATUS, Value::Null, Purpose::Other);
    until(play, app, "a control barrier", |_, a| reports(a) > before);
}

fn entry(app: &App, index: usize) -> String {
    sync::hex(app.session.scenario().unwrap().block(WEAPONS).entry(index).unwrap())
}

#[test]
fn add_remove_undo_and_redo_are_each_one_transaction() {
    let (_t, game, mut app, mut play, state) = pair("placements-live", 4);
    let first = entry(&app, 0);

    let index = app.session.add_entry_at(WEAPONS, None, [4., 5., 6.]).unwrap();
    assert_eq!(index, 2);
    let added = entry(&app, 2);
    settled(&mut play, &mut app, &state, 1, "the add");
    let add = &calls(&state)[0];
    assert_eq!((add["was_count"].as_u64(), add["count"].as_u64()), (Some(2), Some(3)));
    assert_eq!(add["records"], json!([{"index": 2, "bytes": added}]));
    assert_eq!(add["create"], json!([2]));
    assert!(add.get("delete").is_none() && add.get("relink").is_none());
    assert_eq!(add["epoch"], play.edits.epoch.unwrap());

    app.dispatch(Action::Undo);
    settled(&mut play, &mut app, &state, 2, "the undo of the add");
    let undo = &calls(&state)[1];
    assert_eq!((undo["was_count"].as_u64(), undo["count"].as_u64()), (Some(3), Some(2)));
    assert_eq!(undo["delete"], json!([2]));
    assert!(undo.get("records").is_none() && undo.get("create").is_none());

    app.dispatch(Action::Redo);
    settled(&mut play, &mut app, &state, 3, "the redo of the add");
    let redo = &calls(&state)[2];
    assert_eq!(redo["create"], json!([2]));
    assert!(redo.get("records").is_none(), "the slot still holds the record: {redo}");

    // A swap-removal: the last record moves into the slot and keeps its object.
    app.session.remove_entry(WEAPONS, 0).unwrap();
    settled(&mut play, &mut app, &state, 4, "the remove");
    let remove = &calls(&state)[3];
    assert_eq!((remove["was_count"].as_u64(), remove["count"].as_u64()), (Some(3), Some(2)));
    assert_eq!(remove["records"], json!([{"index": 0, "bytes": added, "was": first}]));
    assert_eq!(remove["delete"], json!([0]));
    assert_eq!(remove["relink"], json!([{"from": 2, "to": 0}]));

    idle(&mut play, &mut app);
    assert_eq!(calls(&state).len(), 4);
    assert!(play.reload.is_empty());
    assert!(game.calls("edit.batch").is_empty());

    // The block's layout is the game's again: a field edit of the moved
    // entry is an ordinary write to its new slot.
    app.session
        .apply(Command::set(WEAPONS, 0, "position", FieldValue::Vec3([7., 8., 9.])))
        .unwrap();
    until(&mut play, &mut app, "the field write", |p, _| {
        game.calls("edit.batch").len() == 1 && p.edits.sent.is_none()
    });
    let position = scenario_edit::layout::block(WEAPONS).field("position").unwrap();
    assert_eq!(game.calls("edit.batch")[0]["writes"][0]["path"], sync::path(WEAPONS, 0, position));
    assert_eq!(calls(&state).len(), 4);
}

#[test]
fn refused_shapes_stay_under_reload_with_the_games_reason() {
    let (_t, _game, mut app, mut play, state) = pair("placements-refused", 2);
    app.session.add_entry(WEAPONS, None).unwrap();
    let reason = |a: &App, text: &str| a.pairing.pending.iter().find(|row| {
        row["why"].as_str().is_some_and(|why| why.contains(text))
    }).cloned();
    until(&mut play, &mut app, "the capacity refusal", |p, a| {
        !p.objects.placements.in_flight() && reason(a, "room for 2 records").is_some()
    });
    let row = reason(&app, "room for 2 records").unwrap();
    assert!(tag_sync::Effect::is_reload(&row));
    assert_eq!(row["path"], "Weapons");
    assert_eq!(row["refused"], true);
    assert_eq!(app.pairing.pending.len(), 1, "{:?}", app.pairing.pending);
    idle(&mut play, &mut app);
    assert_eq!(calls(&state).len(), 1, "a refused transaction waits for a change");

    // Back at the game's layout, nothing is left to send or explain.
    app.dispatch(Action::Undo);
    idle(&mut play, &mut app);
    assert!(app.pairing.pending.is_empty(), "{:?}", app.pairing.pending);
    assert_eq!(calls(&state).len(), 1);

    // The game writes a record it cannot make an object for: that entry
    // shows after a reload, and Pending says why.
    {
        let mut b = state.lock().unwrap();
        b.room = 4;
        b.answer = Some(Answer::NotCreated("a named record shows after a reload"));
    }
    app.dispatch(Action::Redo);
    until(&mut play, &mut app, "the record without its object", |p, a| {
        calls(&state).len() == 2 && !p.objects.placements.in_flight()
            && reason(a, "named record").is_some()
    });
    let row = reason(&app, "named record").unwrap();
    assert!(tag_sync::Effect::is_reload(&row));
    assert_eq!(row["path"], "weapons[2]");
    assert_eq!(row["target"], json!({"block": "weapons", "index": 2}));
    assert_eq!(app.pairing.pending.len(), 1, "{:?}", app.pairing.pending);

    // Undoing the add removes the record, and its row with it.
    app.dispatch(Action::Undo);
    settled(&mut play, &mut app, &state, 3, "the undo of the uncreated add");
    assert_eq!(calls(&state)[2]["delete"], json!([2]));
}

#[test]
fn a_lost_reply_is_never_sent_twice() {
    let (_t, _game, mut app, mut play, state) = pair("placements-lost", 4);
    state.lock().unwrap().answer = Some(Answer::Lose);
    app.session.add_entry(WEAPONS, None).unwrap();
    let unknown = |a: &App| a.pairing.pending.iter().find(|row| row["unknown"] == true).cloned();
    until(&mut play, &mut app, "the timed out transaction", |p, a| {
        !p.objects.placements.in_flight() && unknown(a).is_some()
    });
    let row = unknown(&app).unwrap();
    assert!(tag_sync::Effect::is_reload(&row));
    assert!(row["why"].as_str().unwrap().contains("reload the map to verify"), "{row}");
    assert_eq!(state.lock().unwrap().count, 3, "the game applied it");
    idle(&mut play, &mut app);
    assert_eq!(calls(&state).len(), 1);

    // Undo neither resends the add nor guesses at its inverse.
    app.dispatch(Action::Undo);
    idle(&mut play, &mut app);
    assert_eq!(calls(&state).len(), 1);
    assert!(unknown(&app).is_some());
}
