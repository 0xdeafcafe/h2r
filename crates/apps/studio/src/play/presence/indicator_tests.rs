use super::*;
use crate::play::{
    testing::{Temp, fake},
    tests::{editor as open_editor, move_flag},
};
use agent_proto::sim::SessionKind;

#[test]
fn the_indicator_renews_with_the_largest_issue_count_before_aim_select() {
    let temp = Temp::new("issue-indicator");
    let game = fake::Game::start("issue-indicator", "example", SessionKind::Local);
    game.presentation(true);
    let mut app = open_editor(&temp);
    let mut play = Play::new();
    fake::pair_for_view(&mut play, &mut app, &game);
    move_flag(&mut app, [7., 3., 9.]);
    app.pairing.selection.enabled = true;
    fake::until(&mut play, &mut app, "issue summary and first lease", |p, a| {
        a.pairing.link.as_ref().is_some_and(|l| l.issue_summary().is_some())
            && p.presence.link_sent.is_some() && !p.presence.link_pending
    });
    let due = play.presence.link_sent.unwrap() + RENEW;
    let calls = game.calls(editor::method::LINK).len();
    play.update_presence(&app, due - Duration::from_millis(1));
    assert!(!play.presence.link_pending);
    play.update_presence(&app, due);
    assert!(play.presence.link_pending);
    let summary = app.pairing.link.as_ref().unwrap().issue_summary().unwrap();
    fake::until(&mut play, &mut app, "renewed indicator", |p, _| !p.presence.link_pending);
    let sent = game.calls(editor::method::LINK);
    assert_eq!(sent.len(), calls + 1);
    let lease = sent.last().unwrap();
    assert_eq!(lease["text"].as_str(), Some(summary.as_str()));
    assert_eq!(lease["mode"], "view");
    assert_eq!(lease["ttl_ms"].as_u64(), Some(editor::LINK_TTL_MS));
}

#[test]
fn a_transient_indicator_error_retries_once_a_second_and_logs_once() {
    let temp = Temp::new("indicator-retry");
    let game = fake::Game::start("indicator-retry", "example", SessionKind::Local);
    game.presentation(true);
    game.edits.lock().unwrap().indicator_failure = Some("fixture window busy");
    let mut app = open_editor(&temp);
    let mut play = Play::new();
    fake::pair_for_view(&mut play, &mut app, &game);
    fake::until(&mut play, &mut app, "first indicator refusal", |p, _| {
        p.presence.link_why.is_some() && !p.presence.link_pending
    });
    let retry = Instant::now() + Duration::from_secs(60);
    let calls = game.calls(editor::method::LINK).len();
    play.update_presence(&app, retry);
    assert!(play.presence.link_pending);
    fake::until(&mut play, &mut app, "second indicator refusal", |p, _| {
        !p.presence.link_pending && game.calls(editor::method::LINK).len() == calls + 1
    });
    let errors = app.log().iter().filter(|l| l.status.text().contains("fixture window busy"));
    assert_eq!(errors.count(), 1);
    game.edits.lock().unwrap().indicator_failure = None;
    play.update_presence(&app, retry + RENEW - Duration::from_millis(1));
    assert!(!play.presence.link_pending);
    play.update_presence(&app, retry + RENEW);
    assert!(play.presence.link_pending);
    fake::until(&mut play, &mut app, "recovered indicator", |p, _| {
        p.presence.link_why.is_none() && !p.presence.link_pending
    });
}

#[test]
fn waiting_for_a_genuine_frame_is_quiet_and_an_unavailable_mark_notifies_once() {
    let mut play = Play::new();
    let mut app = App::new();
    play.presence.sent.push(Target::Object(7, MarkStyle::Selected));
    let answer = |play: &mut Play, app: &mut App, why, refused: Value| {
        play.presence_answer(app, Purpose::Marks, Ok(json!({
            "held":1, "drawn":false, "why":why, "refused":refused
        })));
    };
    for why in ["waiting", "no camera for the presented frame", "Waiting for a genuine frame"] {
        answer(&mut play, &mut app, why, json!([]));
    }
    assert!(play.presence.marks_why.is_none());
    assert_eq!(play.toasts(Instant::now()).count(), 0);
    for _ in 0..2 {
        answer(&mut play, &mut app, "this host has no window", json!([]));
    }
    assert_eq!(play.toasts(Instant::now()).count(), 1);
    let errors = app.log().iter().filter(|l| l.status.text().contains("this host has no window"));
    assert_eq!(errors.count(), 1);
    // A frame delay does not mask a rejected object in the same mark set.
    answer(
        &mut play,
        &mut app,
        "waiting for a genuine frame",
        json!([{"index":1, "why":"object is gone"}]),
    );
    assert!(play.toasts(Instant::now()).any(|(text, _)| text.contains("object is gone")));
}
