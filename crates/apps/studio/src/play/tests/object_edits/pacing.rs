//! Pump the whole editor frame at a fixed input time while socket replies arrive.
//! A worker scheduling delay cannot turn one drag interval into two.
use super::*;

fn frames_at(
    play: &mut Play,
    app: &mut App,
    now: Instant,
    what: &str,
    done: impl Fn(&Play, &App) -> bool,
) {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        play.update_at(app, now);
        if done(play, app) {
            return;
        }
        assert!(Instant::now() < deadline, "waiting for {what}");
        thread::sleep(Duration::from_millis(2));
    }
}

// A test can advance the editor clock by 200 ms in one instruction. Give the
// independent stream a fresh capture before presenting that time to the editor.
fn fresh_capture(app: &App, now: Instant) {
    let stream = app.pairing.mirror.as_ref().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !stream.snapshot().is_some_and(|s| s.age_ticks(now) < 1.0) {
        assert!(Instant::now() < deadline, "waiting for a fresh game capture");
        thread::sleep(Duration::from_millis(2));
    }
}

fn barrier_at(play: &mut Play, app: &mut App, now: Instant) {
    let reports = |a: &App| {
        a.log().iter().filter(|line| line.status.text().starts_with("session.status:")).count()
    };
    let before = reports(app);
    play.call(agent_proto::method::STATUS, Value::Null, Purpose::Other);
    frames_at(play, app, now, "control reply", |_, a| reports(a) > before);
}

fn start_drag(play: &mut Play, app: &mut App, position: [f32; 3]) -> Instant {
    play.apply_edits(app, Apply::Send).unwrap();
    app.session.begin_gesture();
    app.session.gesture_set(position, [0.; 3]).unwrap();
    // The first field batch is due on the next editor frame.
    let now = Instant::now();
    play.next_push = now;
    fresh_capture(app, now);
    now
}

#[test]
fn gizmo_coalesces_object_frames_flushes_release_and_follows_document_undo() {
    let (_t, game, mut app, mut play) = object_pair("object-drag", SessionKind::Local, true);
    let id = weapon_handle(&app, 0);
    let first = start_drag(&mut play, &mut app, [2., 3., 4.]);
    frames_at(&mut play, &mut app, first, "first acknowledged move", |p, _| {
        p.edits.sent.is_none() && game.calls(OBJECT_SET).len() == 1
    });
    barrier_at(&mut play, &mut app, first);
    let sent = game.calls(OBJECT_SET);
    assert_eq!(sent[0]["id"], id);
    assert_eq!(sent[0]["pos"], json!([2.,3.,4.]));
    let fields = &game.calls("edit.batch")[0]["writes"];
    let position = scenario_edit::layout::block(WEAPONS).field("position").unwrap();
    assert_eq!(fields[0]["group"], "scnr");
    assert_eq!(fields[0]["path"], sync::path(WEAPONS, 0, position));

    app.session.gesture_set([5., 6., 7.], [0.; 3]).unwrap();
    play.update_at(&mut app, first);
    barrier_at(&mut play, &mut app, first);
    assert_eq!(game.calls(OBJECT_SET).len(), 1);

    let next = Instant::now().max(first) + PUSH_EVERY;
    fresh_capture(&app, next);
    app.session.gesture_set([8., 9., 10.], [0.; 3]).unwrap();
    play.request(Request::Apply(Apply::Send));
    frames_at(&mut play, &mut app, next, "second interval's move", |_, _| {
        game.calls(OBJECT_SET).len() == 2
    });
    barrier_at(&mut play, &mut app, next);
    assert_eq!(game.calls(OBJECT_SET)[1]["pos"], json!([8.,9.,10.]));

    app.session.gesture_set([11., 12., 13.], [0.5, 0., 0.]).unwrap();
    app.session.end_gesture();
    frames_at(&mut play, &mut app, next, "release inside the interval", |_, _| {
        game.calls(OBJECT_SET).len() == 3
    });
    barrier_at(&mut play, &mut app, next);
    let final_pose = game.calls(OBJECT_SET)[2].clone();
    assert_eq!(final_pose["pos"], json!([11.,12.,13.]));
    assert!(final_pose["forward"][1].as_f64().unwrap().abs() > 0.1);
    assert!(final_pose["up"].is_array());

    app.dispatch(Action::Undo);
    let undo = Instant::now().max(next) + PUSH_EVERY;
    play.request(Request::Apply(Apply::Send));
    fresh_capture(&app, undo);
    frames_at(&mut play, &mut app, undo, "original document frame", |_, _| {
        game.calls(OBJECT_SET).len() == 4
    });
    barrier_at(&mut play, &mut app, undo);
    let restored = &game.calls(OBJECT_SET)[3];
    assert_eq!(restored["pos"], json!([0.,1.,2.]));
    assert_eq!(restored["forward"], json!([1.,0.,0.]));

    app.dispatch(Action::Redo);
    let redo = Instant::now().max(undo) + PUSH_EVERY;
    play.request(Request::Apply(Apply::Send));
    fresh_capture(&app, redo);
    frames_at(&mut play, &mut app, redo, "redone document frame", |_, _| {
        game.calls(OBJECT_SET).len() == 5
    });
    assert_eq!(game.calls(OBJECT_SET)[4]["pos"], final_pose["pos"]);
}

#[test]
fn release_while_a_field_batch_is_outstanding_moves_on_its_reply() {
    let (_t, game, mut app, mut play) = object_pair("object-batch-release", SessionKind::Local, true);
    let first = start_drag(&mut play, &mut app, [3., 5., 7.]);
    let release = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    game.edits.lock().unwrap().release_batch_reply = Some(release.clone());
    frames_at(&mut play, &mut app, first, "game wrote the field and held its reply", |p, _| {
        p.edits.sent.is_some() && !game.edits.lock().unwrap().bytes.is_empty()
    });
    app.session.end_gesture();
    play.update_at(&mut app, first);
    assert!(play.edits.sent.is_some());
    assert!(game.calls(OBJECT_SET).is_empty());
    release.store(true, Ordering::Release);
    // No second push interval, and no direct calls to the batch or object driver.
    frames_at(&mut play, &mut app, first, "move on batch acknowledgement", |_, _| {
        !game.calls(OBJECT_SET).is_empty()
    });
    barrier_at(&mut play, &mut app, first);
    assert_eq!(game.calls("edit.batch").len(), 1);
    let moves = game.calls(OBJECT_SET);
    assert_eq!(moves.len(), 1);
    assert_eq!(moves[0]["pos"], json!([3.,5.,7.]));
}

#[test]
fn a_drag_throttles_each_selected_object_and_flushes_both_on_release() {
    let (_t, game, mut app, mut play) = object_pair("object-multi", SessionKind::Local, true);
    app.session.select_many(vec![(WEAPONS, 0), (WEAPONS, 1)]);
    let ids = [weapon_handle(&app, 0), weapon_handle(&app, 1)];
    let first = start_drag(&mut play, &mut app, [1., 2., 3.]);
    frames_at(&mut play, &mut app, first, "both selected bodies", |_, _| {
        game.calls(OBJECT_SET).len() == 2
    });
    barrier_at(&mut play, &mut app, first);
    for (i, id) in ids.iter().enumerate() {
        let d = 10. * i as f32;
        let moves = game.calls(OBJECT_SET);
        let sent = moves.iter().find(|v| v["id"].as_str() == Some(id.as_str())).unwrap();
        assert_eq!(sent["pos"], json!([d+1., d+2., d+3.]));
    }

    let mid = first + PUSH_EVERY / 2;
    fresh_capture(&app, mid);
    app.session.gesture_set([4., 5., 6.], [0.; 3]).unwrap();
    // Explicit Send pushes the second field batch early. The live bodies still
    // have a per-object rate limit while the mouse is held.
    play.request(Request::Apply(Apply::Send));
    frames_at(&mut play, &mut app, mid, "two acknowledged field batches", |p, _| {
        game.calls("edit.batch").len() == 2 && p.edits.sent.is_none()
    });
    barrier_at(&mut play, &mut app, mid);
    assert_eq!(game.calls(OBJECT_SET).len(), 2);
    app.session.end_gesture();
    frames_at(&mut play, &mut app, mid, "both released bodies", |_, _| {
        game.calls(OBJECT_SET).len() == 4
    });
    barrier_at(&mut play, &mut app, mid);
    for (i, id) in ids.iter().enumerate() {
        let d = 10. * i as f32;
        let moves = game.calls(OBJECT_SET);
        let sent: Vec<_> = moves.iter().filter(|v| v["id"].as_str() == Some(id.as_str())).collect();
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[1]["pos"], json!([d+4., d+5., d+6.]));
    }
}

#[test]
fn a_refused_object_row_survives_view_stream_loss_and_same_map_reconnect() {
    let (_t, game, mut app, mut play) = object_pair("object-row-gates", SessionKind::Local, true);
    enable(&mut play, &mut app);
    app.session.add_entry(WEAPONS, None).unwrap();
    let refused = |a: &App| a.pairing.pending.iter().any(|r| {
        r["doc"] == "object" && r["refused"] == true
    });
    until(&mut play, &mut app, "refused appended slot", |_, a| refused(a));
    play.request(Request::Edit(false));
    play.update(&mut app);
    assert!(!play.live_edits);
    assert!(refused(&app));

    play.request(Request::Edit(true));
    play.update(&mut app);
    assert!(play.live_edits);
    app.pairing.drop_stream();
    play.update_at(&mut app, Instant::now() + PUSH_EVERY);
    assert!(app.pairing.shown.is_none());
    assert!(refused(&app));
    play.on_event(&mut app, Event::Lost("fixture stream closed".into()));
    play.update(&mut app);
    assert!(matches!(play.conn, Conn::Reconnecting { .. }));
    assert!(refused(&app));
    play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
    until(&mut play, &mut app, "same map verified again", |p, a| {
        p.is_paired() && a.pairing.shown.as_ref().is_some_and(|s| a.pairing.matches_sample(a, s))
    });
    assert!(refused(&app));
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
}
