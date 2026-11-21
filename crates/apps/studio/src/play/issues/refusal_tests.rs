//! Conditional multi-field writes through the fake agent's atomic batch handler.
use super::*;
use crate::app::{Action, tag_tests};
use crate::play::testing::{Temp, fake};
use crate::play::tests::move_flag;
use crate::play::{Apply, Play};
use agent_proto::hot::method::EDIT_BATCH;
use agent_proto::sim::SessionKind;
use agent_proto::{RpcError, code};

fn batch_pair(label: &str, refusal: RpcError) -> (Temp, fake::Game, Play, App) {
    let temp = Temp::new(label);
    let (mut app, _, _) = tag_tests::app(&temp.0);
    let game = fake::Game::start(label, "example", SessionKind::Local);
    let mut play = Play::new();
    fake::pair_for_view(&mut play, &mut app, &game);
    game.edits.lock().unwrap().batch_error = Some(refusal);
    let speed = tag_api::set_field(&app.tags, tag_tests::weap_id(), "speed", &json!(4.5)).unwrap();
    app.dispatch(Action::Tag(speed));
    move_flag(&mut app, [4., 5., 6.]);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    (temp, game, play, app)
}

#[test]
fn an_address_refusal_holds_only_that_field_and_sends_its_sibling() {
    let at = tag_tests::WEAP;
    let why = format!("{at:#010x}..{:#x} outside loaded tags", at + 4);
    let error = RpcError::new(code::INVALID_PARAMS, why.clone());
    let (_temp, game, mut play, mut app) = batch_pair("named-refusal", error);
    fake::until(&mut play, &mut app, "unrefused placement acknowledged", |p, a| {
        p.edits.sent.is_none() && a.pairing.pending.len() == 1 && a.pairing.pending[0]["refused"] == true
    });
    let calls = game.calls(EDIT_BATCH);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0]["writes"].as_array().unwrap().len(), 2);
    assert_eq!(calls[1]["writes"].as_array().unwrap().len(), 1);
    assert_eq!(calls[1]["writes"][0]["group"], "scnr");
    assert_eq!(app.pairing.pending[0]["at"], at);
    assert_eq!(app.pairing.pending[0]["why"], why);
    assert!(play.live_edits);
}

#[test]
fn changing_one_member_retries_the_whole_unattributed_batch() {
    let why = "overlapping fields in edit.batch";
    let error = RpcError::new(code::INVALID_PARAMS, why);
    let (_temp, game, mut play, mut app) = batch_pair("group-refusal", error);
    fake::until(&mut play, &mut app, "refused batch", |_, a| {
        a.pairing.pending.iter().any(|r| r["doc"] == "batch" && r["why"] == why)
    });
    let problems: Vec<_> = app.pairing.pending.iter().filter(|r| r["refused"] == true).collect();
    assert_eq!(problems.len(), 1);
    assert_eq!(problems[0]["target"], json!({"panel":"pending"}));
    play.push_edits(&mut app);
    assert!(play.edits.sent.is_none());
    assert_eq!(game.calls(EDIT_BATCH).len(), 1);

    move_flag(&mut app, [10., 20., 30.]);
    fake::until(&mut play, &mut app, "both fields reconsidered", |p, a| {
        p.edits.sent.is_none() && a.pairing.pending.is_empty()
    });
    let calls = game.calls(EDIT_BATCH);
    assert_eq!(calls.len(), 2);
    let retry = calls[1]["writes"].as_array().unwrap();
    assert_eq!(retry.len(), 2);
    assert_eq!(retry[0], calls[0]["writes"][0], "the unchanged sibling was retried");
    assert_ne!(retry[1]["value"], calls[0]["writes"][1]["value"]);
}

#[test]
fn a_missing_game_slot_refuses_its_placement_and_allows_the_tag_write() {
    let offset = scenario_edit::layout::block(BlockKind::NetgameFlags).offset;
    let why = format!("+{offset:#x}[0]: the block has 0 elements");
    let (_temp, game, mut play, mut app) = batch_pair(
        "slot-refusal",
        RpcError::new(code::INVALID_PARAMS, why.clone()),
    );
    fake::until(&mut play, &mut app, "unrefused tag acknowledged", |p, a| {
        p.edits.sent.is_none() && a.pairing.pending.len() == 1 && a.pairing.pending[0]["refused"] == true
    });
    assert_eq!(app.pairing.pending[0]["target"], json!({"block":"netgame_flags","index":0}));
    assert_eq!(app.pairing.pending[0]["why"], why);
    let calls = game.calls(EDIT_BATCH);
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[1]["writes"].as_array().unwrap().len(), 1);
    assert_eq!(calls[1]["writes"][0]["at"], tag_tests::WEAP);
}
