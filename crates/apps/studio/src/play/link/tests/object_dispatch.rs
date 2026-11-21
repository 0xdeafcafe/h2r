use super::*;
use crate::app::App;
use crate::play::{Apply, PUSH_EVERY, pair::Request, tests::object_edits::object_pair};
use agent_proto::sim::method::OBJECT_DELETE;
use std::sync::atomic::Ordering;

/// Hold the worker wake-up for one real document deletion. Play still owns
/// the request data. The actual worker and stream keep their sockets open.
fn held_delete(tag: &str) -> (Temp, fake::Game, App, Play, QueuedCall) {
    let (temp, game, mut app, mut play) = object_pair(tag, SessionKind::Local, true);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    play.update(&mut app);
    let (hold, queued) = mpsc::channel();
    let worker = std::mem::replace(&mut play.link.as_mut().unwrap().calls, hold);
    app.session.remove_entry(BlockKind::Weapons, 0).unwrap();
    play.update_objects(&mut app, Instant::now() + PUSH_EVERY);
    let edit = queued.try_recv().expect("document deletion offered");
    assert!(matches!(edit, QueuedCall::Object { .. }));
    play.link.as_mut().unwrap().calls = worker;
    (temp, game, app, play, edit)
}

fn skipped(play: &mut Play, app: &mut App, edit: QueuedCall) {
    let QueuedCall::Object { ticket, .. } = &edit else { panic!("object wake-up") };
    let ticket = *ticket;
    let link = play.link.as_ref().unwrap();
    link.calls.send(edit).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let event = link.next(wait).expect("worker skips the withdrawn object edit");
        if matches!(&event, Event::ObjectSkipped(id) if *id == ticket) {
            play.on_event(app, event);
            return;
        }
        assert!(!matches!(&event, Event::Answer { purpose: Purpose::Object(id), .. } if *id == ticket));
    }
}

#[test]
fn switching_to_view_withdraws_an_object_waiting_for_the_worker() {
    let (_temp, game, mut app, mut play, edit) = held_delete("withdraw-object");
    play.request(Request::Edit(false));
    play.update(&mut app);
    skipped(&mut play, &mut app, edit);
    assert!(!play.live_edits);
    assert!(game.calls(OBJECT_DELETE).is_empty());
}

#[test]
fn the_worker_rechecks_the_stream_epoch_before_a_delayed_object_edit() {
    let (_temp, game, mut app, mut play, edit) = held_delete("object-epoch-wire");
    let epoch = play.edits.epoch.unwrap();
    game.bsp.fetch_add(1, Ordering::Relaxed);
    // Hold the editor frame until the independent stream sees the new epoch.
    let stream = app.pairing.mirror.as_ref().unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !stream.snapshot().and_then(|s| s.latest_header()).is_some_and(|(_, h)| h.epoch != epoch) {
        assert!(Instant::now() < deadline, "new streamed epoch");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(play.edits.epoch, Some(epoch));
    skipped(&mut play, &mut app, edit);
    assert!(game.calls(OBJECT_DELETE).is_empty());
}

#[test]
fn losing_the_link_before_object_dispatch_keeps_the_delete_retryable() {
    let (_temp, game, mut app, mut play, _wake) = held_delete("object-unsent-loss");
    play.on_event(&mut app, Event::Lost("lost during an earlier control request".into()));
    play.update_objects(&mut app, Instant::now());
    assert!(game.calls(OBJECT_DELETE).is_empty());
    assert!(app.pairing.pending.iter().all(|row| row["unknown"] != true));
    play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
    fake::until(&mut play, &mut app, "untransmitted delete", |_, _| {
        !game.calls(OBJECT_DELETE).is_empty()
    });
    assert_eq!(game.calls(OBJECT_DELETE).len(), 1);
}
