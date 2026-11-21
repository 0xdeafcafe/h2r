use super::*;
use crate::app::Action;
use crate::play::{
    Play,
    testing::{Temp, fake},
    tests::editor,
};
use agent_proto::{editor::method as presentation, sim::SessionKind};
use scenario_edit::BlockKind;
use serde_json::json;

#[test]
fn close_after_dequeue_discards_the_queued_move() {
    let (calls, queued) = mpsc::channel();
    let (stop, stopping) = mpsc::channel();
    let (ack, done) = mpsc::channel();
    calls
        .send(QueuedCall::Control {
            call: Call {
                method: agent_proto::sim::method::STATE_SET.into(),
                params: json!({"player":"player.0","pos":[1.0, 2.0, 3.0]}),
                purpose: Purpose::Other,
            },
            player: None,
        })
        .unwrap();

    let work = next_work(&stopping, || {
        let move_call = queued.recv_timeout(Duration::ZERO)?;
        // Unpair after the worker dequeues the move, before it dispatches it.
        stop.send(Close {
            clears: vec![],
            done: Some(ack),
        })
        .unwrap();
        Ok(move_call)
    });
    let Work::Close(close) = work else {
        panic!("worker dispatched after unpair")
    };
    close.done.unwrap().send(()).unwrap();
    done.try_recv().unwrap();
}

fn check_mark_handoff(tag: &str, other_game: bool) {
    let t = Temp::new(tag);
    let game = fake::Game::start(tag, "example", SessionKind::Local);
    game.presentation(true);
    let mut app = editor(&t);
    let mut play = Play::new();
    let has_mark = |game: &fake::Game| {
        game.calls(presentation::MARKS)
            .last()
            .is_some_and(|p| p["marks"].as_array().is_some_and(|marks| !marks.is_empty()))
    };
    fake::pair_for_view(&mut play, &mut app, &game);
    app.dispatch(Action::Select(Some((BlockKind::NetgameFlags, 0))));
    fake::until(&mut play, &mut app, "first selection lease", |_, _| {
        has_mark(&game)
    });

    // Hold delivery of the first worker's close request. Keep its ordinary
    // queue connected until the real worker can consume that request.
    let old = play.link.as_mut().unwrap();
    let keep_open = old.calls.clone();
    let (hold_close, held_close) = mpsc::channel();
    let old_stop = std::mem::replace(&mut old.stop, hold_close);
    let other = other_game.then(|| {
        let other = fake::Game::start(&format!("{tag}-b"), "example", SessionKind::Local);
        other.presentation(true);
        other
    });
    if let Some(other) = &other {
        fake::pair_for_view(&mut play, &mut app, other);
        fake::until(
            &mut play,
            &mut app,
            "other game's selection lease",
            |_, _| has_mark(other),
        );
    } else {
        play.unpair(&mut app);
    }
    let close = held_close.try_recv().expect("old worker receives unpair");
    assert!(
        close
            .clears
            .iter()
            .any(|c| c.method == presentation::MARKS && c.params["marks"] == json!([]))
    );
    let marks = game.calls(presentation::MARKS).len();
    let links = game.calls(presentation::LINK).len();

    // A second connection to the first game can pair and serve controls
    // while its new indicator and selection wait for the old clear.
    fake::pair_for_view(&mut play, &mut app, &game);
    fake::until(
        &mut play,
        &mut app,
        "new connection mirrors the map",
        |_, a| crate::play::select::matching(a),
    );
    assert!(play.has_method(presentation::FOCUS));
    let focus = game.calls(presentation::FOCUS).len();
    play.call(presentation::FOCUS, json!({}), Purpose::Focus);
    fake::until(&mut play, &mut app, "control after pairing", |_, _| {
        game.calls(presentation::FOCUS).len() > focus
    });
    assert_eq!(game.calls(presentation::MARKS).len(), marks);
    assert_eq!(game.calls(presentation::LINK).len(), links);

    old_stop.send(close).unwrap();
    drop(keep_open);
    fake::until(
        &mut play,
        &mut app,
        "clear followed by the new lease",
        |_, _| game.calls(presentation::MARKS).len() >= marks + 2 && has_mark(&game),
    );
    let sent = game.calls(presentation::MARKS);
    assert_eq!(sent[marks]["marks"], json!([]));
    assert!(!sent[marks + 1]["marks"].as_array().unwrap().is_empty());
}

#[test]
fn unpair_and_pair_waits_for_the_old_mark_clear() {
    check_mark_handoff("unpair-lease", false);
}

#[test]
fn returning_to_a_game_waits_only_for_that_games_old_mark_clear() {
    check_mark_handoff("return-lease", true);
}

#[test]
fn sparse_control_methods_refuse_before_claiming_or_queueing_a_call() {
    use agent_proto::method::{CLAIM, PAUSE, RELEASE, RESUME, STEP};
    use crate::api::Api;
    use crate::play::Conn;

    let all = [CLAIM, RELEASE, PAUSE, RESUME, STEP];
    for (missing, action) in [
        (PAUSE, "pause"),
        (RESUME, "resume"),
        (STEP, "step"),
        (CLAIM, "pause"),
        (RELEASE, "pause"),
    ] {
        let mut play = Play::new();
        let mut app = crate::app::App::new();
        let (calls, queued) = mpsc::channel();
        let (_, events) = mpsc::channel();
        let (stop, _closing) = mpsc::channel();
        play.link = Some(Link {
            endpoint: Endpoint::Unix("fixture-control.sock".into()),
            calls,
            events,
            stop,
        });
        play.conn = Conn::Connected;
        play.paused = action != "pause";
        let methods: Vec<_> = all.into_iter().filter(|&m| m != missing).collect();
        play.received_methods(&mut app, Ok(json!(methods)));
        play.publish(&mut app);
        let view = app.pairing.link.as_ref().unwrap();
        assert!(view.paired);
        assert!(view.unavailable.iter().any(|why| why.contains(missing)));
        let claim_available = missing != CLAIM && missing != RELEASE;
        assert_eq!(view.controls.pause.is_none(), claim_available && missing != PAUSE);
        assert_eq!(view.controls.resume.is_none(), claim_available && missing != RESUME);
        assert_eq!(view.controls.step.is_none(), claim_available && missing != STEP);
        let failure = Api::new()
            .call(&mut app, studio_proto::method::PAIR_CONTROL, json!({"action":action}))
            .unwrap_err();
        assert_eq!(failure.code, studio_proto::code::REFUSED);
        assert!(failure.message.contains(missing));

        match action {
            "pause" => {
                app.pairing.pause_while_editing = true;
                play.window_focus(&app, true);
            }
            "resume" => play.finish_playtest(&mut app),
            _ => play.step(1),
        }
        if missing == RELEASE {
            play.controller = true;
            play.release_control();
            assert!(play.controller);
        } else {
            assert!(!play.controller);
        }
        assert!(queued.try_recv().is_err(), "queued game control without {missing}");
    }
}

mod object_dispatch;
