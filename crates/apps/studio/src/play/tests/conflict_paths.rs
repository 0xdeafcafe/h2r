//! A conflict keeps the tag and nested field path used by the submitted batch.
use super::*;
use crate::app::{BlockOp, tag_api, tag_tests::weap_id};

const FIELD: &str = "parts[1].value";

fn address(app: &App) -> u32 {
    tag_api::field_focus(&app.tags, weap_id(), FIELD).unwrap().address
}

fn submit_mismatch(play: &mut Play, app: &mut App, game: &Game) -> u32 {
    let at = address(app);
    let action = tag_api::set_field(&app.tags, weap_id(), FIELD, &json!(42)).unwrap();
    app.dispatch(Action::Tag(action));
    game.edits.lock().unwrap().bytes.insert(at.to_string(), "03000000".into());
    play.apply_edits(app, Apply::Send).unwrap();
    play.push_edits(app);
    assert!(play.edits.sent.is_some());
    at
}

fn grow_parts(app: &mut App) {
    app.dispatch(Action::SetAllowGrowth(true));
    let action = tag_api::block(&app.tags, weap_id(), "parts", BlockOp::Add).unwrap();
    app.dispatch(Action::Tag(action));
}

#[test]
fn a_moved_field_conflict_is_disabled_until_its_submitted_path_returns() {
    let (_t, game, mut app, mut play) = tag_pair("moved-conflict");
    let at = submit_mismatch(&mut play, &mut app, &game);
    // Change Pending before consuming the batch reply. Its address alone can
    // no longer identify the original nested field.
    grow_parts(&mut app);
    assert_ne!(address(&app), at);
    play.refresh_pending(&mut app, false);
    until(&mut play, &mut app, "moved field in Pending", |_, a| {
        a.pairing.pending.iter().any(|r| r["conflict"] == true && r["resolvable"] == false)
    });
    let row = app.pairing.pending.iter().find(|r| r["conflict"] == true).unwrap();
    assert_eq!(row["target"], json!({"panel":"pending"}));
    assert_eq!(row["class"], tag_sync::RELOAD);
    let link = play.link_view(&app);
    let issue = link.issues.iter().find(|issue| issue.kind == issues::Kind::Mismatch).unwrap();
    assert_eq!(issue.target, json!({"panel":"pending"}));
    assert!(link.issues.tag_field(weap_id(), at).next().is_none());
    let mut api = crate::api::Api::new();
    for (name, action) in [("take", Apply::Take(at)), ("overwrite", Apply::Overwrite(at))] {
        let error = api.call(&mut app, studio_proto::method::MIRROR_APPLY,
            json!({"what":name, "at":at})).unwrap_err();
        assert_eq!(error.code, studio_proto::code::REFUSED);
        assert!(play.apply_edits(&mut app, action).unwrap_err().contains("field moved"));
    }
    assert_eq!(game.calls("edit.batch").len(), 1);
    assert!(game.calls("edit.read").is_empty());

    app.dispatch(Action::Undo);
    assert_eq!(address(&app), at);
    play.push_edits(&mut app);
    let row = app.pairing.pending.iter().find(|r| r["conflict"] == true).unwrap();
    let path = row["target"]["path"].as_str().unwrap();
    assert_eq!(tag_api::field_focus(&app.tags, weap_id(), path).unwrap().address, at);
    play.apply_edits(&mut app, Apply::Overwrite(at)).unwrap();
    until(&mut play, &mut app, "overwrite the restored field", |p, _| {
        p.edits.sent.is_none() && game.calls("edit.batch").len() == 2
    });
}

#[test]
fn a_take_reply_cannot_write_through_a_relocated_tag_block() {
    let (_t, game, mut app, mut play) = tag_pair("take-after-grow");
    let at = submit_mismatch(&mut play, &mut app, &game);
    until(&mut play, &mut app, "nested field conflict", |_, a| {
        a.pairing.pending.iter().any(|r| r["conflict"] == true)
    });
    play.apply_edits(&mut app, Apply::Take(at)).unwrap();
    grow_parts(&mut app);
    let moved = address(&app);
    assert_ne!(moved, at);
    let old_bytes = app.tags.read(weap_id(), at, 4);
    let new_bytes = app.tags.read(weap_id(), moved, 4);
    // The original published Pending row still shows the old location. Check
    // the socket rejection before the following update republishes the list.
    let mut api = crate::api::Api::new();
    let error = api.call(&mut app, studio_proto::method::MIRROR_APPLY,
        json!({"what":"overwrite", "at":at})).unwrap_err();
    assert!(error.message.contains("field moved"));
    until(&mut play, &mut app, "late take refused", |_, a| {
        a.log().iter().any(|l| l.status.text().contains("take the game's value: the conflicted tag field moved"))
    });
    assert_eq!(app.tags.read(weap_id(), at, 4), old_bytes);
    assert_eq!(app.tags.read(weap_id(), moved, 4), new_bytes);
    assert_eq!(game.calls("edit.read").len(), 1);
    assert_eq!(game.calls("edit.batch").len(), 1);
}
