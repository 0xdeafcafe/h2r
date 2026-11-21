use super::*;
use crate::api::Api;
use crate::app::{Action, ToolKind, tag_tests};
use crate::play::testing::{Temp, fake};
use crate::play::{Apply, Play};
use agent_proto::sim::SessionKind;
use studio_proto::method as m;

#[test]
fn a_refused_batch_marks_the_placement_and_navigates_to_its_row() {
    let (_dir, game, mut app, mut play) =
        crate::play::tests::paired("field-refusal", "example", SessionKind::Local);
    game.agent.add_game_method(agent_proto::hot::method::EDIT_BATCH, agent::Access::Act, |_, _| {
        let mut err = agent_proto::RpcError::new(agent_proto::sim::REFUSED, "write refused");
        err.data = Some(json!({"why":"fixture is read only"}));
        Err(err)
    });
    crate::play::tests::move_flag(&mut app, [7., 8., 9.]);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    play.playtest(&mut app, None).unwrap();
    fake::until(&mut play, &mut app, "refused flag field", |p, a| {
        p.link_view(a).issues.iter().any(|i| i.kind == Kind::Refused)
    });
    assert!(play.live_edits);
    play.push_edits(&mut app);
    assert!(play.edits.sent.is_none(), "an unchanged refusal is not an automatic retry");
    let link = play.link_view(&app);
    let issue = link.issues.iter().find(|i| i.kind == Kind::Refused).unwrap();
    assert!(issue.matches_placement(BlockKind::NetgameFlags, 0, Some("position")));
    assert!(issue.matches_placement(BlockKind::NetgameFlags, 0, None));
    assert!(!issue.matches_placement(BlockKind::NetgameFlags, 1, None));
    assert_eq!(issue.text, "fixture is read only");
    let mark = crate::ui::inspector::row_issue(&link.issues, |i| {
        i.matches_placement(BlockKind::NetgameFlags, 0, None)
    });
    assert_eq!(mark, Some(issue));

    let mut api = Api::new();
    let status = api.call(&mut app, m::PAIR_STATUS, Value::Null).unwrap();
    let reported = status["link"]["issues"].as_array().unwrap().iter()
        .find(|r| r["kind"] == "refused").unwrap();
    assert_eq!(reported, &issue.wire());
    app.dispatch(Action::SetTool(ToolKind::Hud));
    let nav = api.call(&mut app, m::NAV_GO, reported["target"].clone()).unwrap();
    assert_eq!(app.tool, ToolKind::Scenario);
    assert_eq!(nav["selected"], json!({"block":"netgame_flags", "index":0}));

    game.agent.add_game_method(agent_proto::hot::method::EDIT_BATCH, agent::Access::Act, |_, _| {
        Err(agent_proto::RpcError::new(agent_proto::code::INVALID_PARAMS, "invalid field"))
    });
    play.apply_edits(&mut app, Apply::Send).unwrap();
    fake::until(&mut play, &mut app, "field validation error", |p, a| {
        p.link_view(a).issues.iter().any(|i| i.kind == Kind::Refused && i.text == "invalid field")
    });

    app.dispatch(Action::Undo);
    fake::until(&mut play, &mut app, "refused field undone", |p, a| {
        p.link_view(a).issues.iter().all(|i| i.kind != Kind::Refused)
    });
}

#[test]
fn a_mismatch_marks_the_nested_tag_field_and_navigates_to_it() {
    let t = Temp::new("field-nav");
    let (mut app, _, _) = tag_tests::app(&t.0);
    let game = fake::Game::start("field-nav", "example", SessionKind::Local);
    let mut play = Play::new();
    fake::pair_for_view(&mut play, &mut app, &game);
    let tag = tag_tests::weap_id();
    let path = "parts[1].value";
    let at = tag_api::field_focus(&app.tags, tag, path).unwrap().address;
    let edit = tag_api::set_field(&app.tags, tag, path, &json!(42)).unwrap();
    app.dispatch(Action::Tag(edit));
    game.edits.lock().unwrap().bytes.insert(at.to_string(), "03000000".into());
    play.apply_edits(&mut app, Apply::Send).unwrap();
    fake::until(&mut play, &mut app, "mismatching tag field", |p, a| {
        p.link_view(a).issues.iter().any(|i| i.kind == Kind::Mismatch)
    });
    let link = play.link_view(&app);
    let issue = link.issues.iter().find(|i| i.kind == Kind::Mismatch).unwrap();
    assert_eq!(issue.at, Some(at));
    assert!(issue.matches_tag(tag, Some(at)));
    let mark = crate::ui::inspector::row_issue(&link.issues, |i| i.matches_tag(tag, None));
    assert_eq!(mark, Some(issue));

    let mut api = Api::new();
    let status = api.call(&mut app, m::PAIR_STATUS, Value::Null).unwrap();
    let reported = status["link"]["issues"].as_array().unwrap().iter()
        .find(|r| r["kind"] == "mismatch").unwrap();
    assert_eq!(reported, &issue.wire());
    app.dispatch(Action::SetTool(ToolKind::Hud));
    let nav = api.call(&mut app, m::NAV_GO, reported["target"].clone()).unwrap();
    assert_eq!(app.tool, ToolKind::Scenario);
    let focus = tag_api::field_focus(&app.tags, tag, nav["path"].as_str().unwrap()).unwrap();
    assert_eq!(focus.address, at);
}

#[test]
fn the_link_summarizes_the_largest_count_and_pending_is_navigable() {
    let t = Temp::new("issue-counts");
    let (mut app, _, _) = tag_tests::app(&t.0);
    let play = Play::new();
    app.pairing.pending = vec![
        tag_sync::row("scenario", json!(tag_tests::weap_id().0), "Weapons", tag_sync::RELOAD, "layout changed"),
        tag_sync::row("tags", Value::Null, "second", tag_sync::RELOAD, "new map string"),
        tag_sync::row("tags", Value::Null, "third", tag_sync::CREATION, "next creation"),
    ];
    play.publish(&mut app);
    let link = app.pairing.link.as_ref().unwrap();
    assert!(link.text.ends_with(" · 2 need a reload"));
    assert_eq!(link.issue_summary().as_deref(), Some("2 need a reload"));
    let target = link.issues[0].target.clone();
    assert_eq!(target, json!({"panel":"pending"}));
    Api::new().call(&mut app, m::NAV_GO, target).unwrap();
    assert!(app.pairing.open_pending);
}
