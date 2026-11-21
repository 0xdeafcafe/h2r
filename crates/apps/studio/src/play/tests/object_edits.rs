//! Object writes through the same document commands, clock and sockets as the window.

use super::*;
use agent_proto::sim::method::{OBJECT_DELETE, OBJECT_RESPAWN, OBJECT_SET};

mod failures;

const WEAPONS: BlockKind = BlockKind::Weapons;

pub(in crate::play) fn object_pair(tag: &str, kind: SessionKind, with_set: bool) -> (Temp, Game, App, Play) {
    let t = Temp::new(tag);
    let mut bytes = scenario_edit::fixture::fixture_at_end();
    // Plain unnamed weapons with default creation fields, allowed by the real respawn method.
    for i in 0..2 {
        let at = scenario_edit::fixture::sfile(scenario_edit::fixture::WEAPONS_AT + i * 0x54);
        bytes[at + 2..at + 4].copy_from_slice(&(-1i16).to_le_bytes());
        bytes[at + 4..at + 8].fill(0);
        bytes[at + 0x24..at + 0x54].fill(0);
    }
    let file = t.0.join("example.map");
    std::fs::write(&file, bytes).unwrap();
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(file));
    app.session.set_allow_growth(true);
    let game = Game::start(tag, "example", kind);
    game.edits.lock().unwrap().checksum = app.session.source_checksum().unwrap();
    game.install_object_edits(with_set);
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "weapon placements and map identity", |p, a| {
        p.edit_refusal(a).is_none()
            && !p.finder.running().is_empty()
            && a.pairing.shown.as_ref().is_some_and(|sample| {
                a.pairing.matches_sample(a, sample)
                    && sample.objects.iter().any(|o| o.kind == 2 && o.placement == Some(0))
            })
    });
    app.session.select(Some((WEAPONS, 0)));
    (t, game, app, play)
}

fn enable(play: &mut Play, app: &mut App) {
    play.apply_edits(app, Apply::Send).unwrap();
    play.update(app);
}

fn move_weapon(app: &mut App, pos: [f32; 3]) {
    app.session
        .apply(Command::set(WEAPONS, 0, "position", FieldValue::Vec3(pos)))
        .unwrap();
}

fn push_fields(play: &mut Play, app: &mut App) {
    play.push_edits(app);
    barrier(play, app);
}

fn weapon_handle(app: &App, slot: u16) -> String {
    let body = app.pairing.shown.as_ref().unwrap().objects.iter()
        .find(|o| o.kind == 2 && o.placement == Some(slot))
        .unwrap();
    agent_proto::ObjectId { kind: agent_proto::Kind::Weapon, key: body.id }.to_string()
}

// Receiving a later answer on the same control socket proves that the editor consumed
// all earlier replies. Count STATUS reports so an earlier barrier cannot satisfy this one.
fn barrier(play: &mut Play, app: &mut App) {
    let reports = |app: &App| app.log().iter()
        .filter(|line| line.status.text().starts_with("session.status:"))
        .count();
    let before = reports(app);
    play.call(agent_proto::method::STATUS, Value::Null, Purpose::Other);
    until(play, app, "object reply barrier", |_, a| reports(a) > before);
}

#[test]
fn appended_preview_is_removed_by_its_returned_handle_on_undo() {
    let (_t, game, mut app, mut play) = object_pair("object-add", SessionKind::Local, true);
    game.allow_appended_preview();
    enable(&mut play, &mut app);
    let slot = app.session.add_entry_at(WEAPONS, None, [4., 5., 6.]).unwrap();
    assert_eq!(slot, 2);
    until(&mut play, &mut app, "unlinked preview", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|sample| {
            sample.objects.iter().any(|o| {
                o.kind == 2 && o.placement.is_none() && o.position == [4., 5., 6.]
            })
        })
    });
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
    assert_eq!(game.calls(OBJECT_RESPAWN)[0]["placement"], json!({"block":"weapons","index":2}));
    let object = app.pairing.shown.as_ref().unwrap().objects.iter()
        .find(|o| o.kind == 2 && o.placement.is_none()).unwrap();
    let preview = agent_proto::ObjectId { kind: agent_proto::Kind::Weapon, key: object.id };
    assert!(game.calls(OBJECT_SET).iter().any(|call| {
        call["id"] == preview.to_string() && call["pos"] == json!([4., 5., 6.])
    }));
    barrier(&mut play, &mut app);
    app.dispatch(Action::Undo);
    until(&mut play, &mut app, "preview deleted", |_, _| !game.calls(OBJECT_DELETE).is_empty());
    assert_eq!(game.calls(OBJECT_DELETE)[0]["id"], preview.to_string());
    app.dispatch(Action::Redo);
    until(&mut play, &mut app, "preview remade", |_, _| game.calls(OBJECT_RESPAWN).len() == 2);
    assert!(game.calls("edit.batch").is_empty());
}

#[test]
fn undoing_a_delete_respawns_the_original_loaded_slot() {
    let (_t, game, mut app, mut play) = object_pair("object-delete", SessionKind::Local, true);
    let first = weapon_handle(&app, 0);
    let last = weapon_handle(&app, 1);
    enable(&mut play, &mut app);
    // Removing entry zero swaps the final record into its slot in the document.
    app.session.remove_entry(WEAPONS, 0).unwrap();
    until(&mut play, &mut app, "deleted object", |_, _| !game.calls(OBJECT_DELETE).is_empty());
    barrier(&mut play, &mut app);
    assert_eq!(game.calls(OBJECT_DELETE).len(), 1);
    assert_eq!(game.calls(OBJECT_DELETE)[0]["id"], first);
    assert!(game.calls(OBJECT_SET).is_empty());
    assert!(game.calls("edit.batch").is_empty());
    app.dispatch(Action::Undo);
    until(&mut play, &mut app, "respawned old slot", |_, _| !game.calls(OBJECT_RESPAWN).is_empty());
    assert_eq!(game.calls(OBJECT_RESPAWN)[0]["placement"], json!({"block":"weapons","index":0}));
    barrier(&mut play, &mut app);
    app.dispatch(Action::Redo);
    until(&mut play, &mut app, "deleted replacement", |_, _| game.calls(OBJECT_DELETE).len() == 2);
    assert_ne!(game.calls(OBJECT_DELETE)[1]["id"], first);
    assert_ne!(game.calls(OBJECT_DELETE)[1]["id"], last);
}

#[test]
fn appended_slot_refused_by_the_loaded_block_stays_pending_for_reload() {
    let (_t, game, mut app, mut play) = object_pair("object-add-refused", SessionKind::Local, true);
    enable(&mut play, &mut app);
    app.session.add_entry(WEAPONS, None).unwrap();
    until(&mut play, &mut app, "strict loaded block", |_, a| {
        a.pairing.pending.iter().any(|row| {
            row["why"].as_str().is_some_and(|why| why.contains("outside a block of 2 records"))
        })
    });
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
    assert!(app.pairing.pending.iter().any(|row| row["class"] == tag_sync::RELOAD));
    assert!(game.calls(OBJECT_DELETE).is_empty());
    app.dispatch(Action::Undo);
    barrier(&mut play, &mut app);
    assert!(game.calls(OBJECT_DELETE).is_empty());
}

#[test]
fn a_game_without_object_set_keeps_the_field_poke_and_explains_the_object() {
    let (_t, game, mut app, mut play) = object_pair("object-no-set", SessionKind::Local, false);
    assert!(!play.has_method(OBJECT_SET));
    // A trap installed after session.methods catches even an unadvertised call.
    let called = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let trap = called.clone();
    game.agent.add_game_method(OBJECT_SET, agent::Access::Act, move |_, _| {
        trap.store(true, Ordering::Relaxed);
        Ok(json!({}))
    });
    enable(&mut play, &mut app);
    move_weapon(&mut app, [14., 15., 16.]);
    until(&mut play, &mut app, "poke with unavailable live move", |p, a| {
        game.calls("edit.batch").len() == 1 && p.edits.sent.is_none()
            && a.pairing.pending.iter().any(|row| {
                row["why"].as_str().is_some_and(|why| why.contains(OBJECT_SET))
            })
    });
    barrier(&mut play, &mut app);
    assert!(!called.load(Ordering::Relaxed));
}

#[test]
fn respawn_now_follows_the_batch_and_confirms_the_peer_count() {
    let (_t, game, mut app, mut play) = object_pair("object-respawn", SessionKind::MpHost, true);
    assert!(play.apply_edits(&mut app, Apply::Respawn).is_err());
    game.edits.lock().unwrap().peers = Some(3);
    until(&mut play, &mut app, "host confirmation", |p, _| {
        p.peer_confirmation().is_some_and(|scope| scope.peers == 3)
    });
    assert!(play.apply_edits(&mut app, Apply::Send).is_err());
    let scope = play.peer_confirmation().unwrap();
    play.apply_edits(&mut app, Apply::Confirm(scope)).unwrap();
    enable(&mut play, &mut app);
    // Both zero and one are accepted default scales on the real host.
    app.session.apply(Command::set(WEAPONS, 0, "scale", FieldValue::Float(0.))).unwrap();
    until(&mut play, &mut app, "acknowledged creation field", |p, _| {
        p.can_respawn() && !game.calls("edit.batch").is_empty() && p.edits.sent.is_none()
    });
    assert!(game.calls(OBJECT_RESPAWN).is_empty());
    assert_eq!(game.calls("edit.batch")[0]["confirm_peers"], 3);
    let old = weapon_handle(&app, 0);
    play.apply_edits(&mut app, Apply::Respawn).unwrap();
    until(&mut play, &mut app, "host preview", |_, _| !game.calls(OBJECT_RESPAWN).is_empty());
    let calls = game.calls(OBJECT_RESPAWN);
    assert_eq!(calls[0]["confirm_peers"], 3);
    assert_eq!(calls[0]["id"], old);
    assert!(calls[0].get("placement").is_none());
    barrier(&mut play, &mut app);

    game.edits.lock().unwrap().peers = None;
    until(&mut play, &mut app, "unknown peer count", |p, a| {
        p.edit_refusal(a).is_some_and(|why| why.contains("peer count"))
    });
    move_weapon(&mut app, [6., 7., 8.]);
    play.update_objects(&mut app, Instant::now() + PUSH_EVERY);
    barrier(&mut play, &mut app);
    assert!(game.calls(OBJECT_SET).iter().all(|call| call["pos"] != json!([6., 7., 8.])));
    assert!(play.apply_edits(&mut app, Apply::Respawn).is_err());
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
}

#[test]
fn refused_and_timed_out_moves_do_not_retry_until_the_document_changes() {
    for (label, code, why) in [
        ("object-denied", agent_proto::sim::REFUSED, "fixture move refused"),
        ("object-timeout", agent_proto::code::TIMEOUT, "fixture move timed out"),
    ] {
        let (_t, game, mut app, mut play) = object_pair(label, SessionKind::Local, true);
        enable(&mut play, &mut app);
        game.fail_next_object_edit(code, why);
        move_weapon(&mut app, [2., 4., 8.]);
        until(&mut play, &mut app, "object failure", |_, a| {
            a.pairing.pending.iter().any(|r| r["why"].as_str().is_some_and(|w| w.contains(why)))
        });
        let next = Instant::now() + Duration::from_secs(2);
        play.update_objects(&mut app, next);
        barrier(&mut play, &mut app);
        assert_eq!(game.calls(OBJECT_SET).len(), 1);
        move_weapon(&mut app, [3., 6., 9.]);
        push_fields(&mut play, &mut app);
        play.update_objects(&mut app, next + PUSH_EVERY);
        until(&mut play, &mut app, "subsequent move", |_, _| game.calls(OBJECT_SET).len() == 2);
        assert_eq!(game.calls(OBJECT_SET)[1]["pos"], json!([3.,6.,9.]));
    }
}

#[test]
fn a_gone_object_is_replaced_only_from_a_fresh_stream_handle() {
    let (_t, game, mut app, mut play) = object_pair("object-gone", SessionKind::Local, true);
    enable(&mut play, &mut app);
    let old = weapon_handle(&app, 0);
    // Hold the old sample's binding until the control connection has observed GONE.
    game.fail_next_object_edit(agent_proto::code::GONE, "the weapon is gone");
    move_weapon(&mut app, [12., 14., 16.]);
    until(&mut play, &mut app, "retired handle", |_, app| {
        app.pairing.pending.iter().any(|row| row["why"] == "the weapon is gone")
    });
    move_weapon(&mut app, [1., 3., 7.]);
    push_fields(&mut play, &mut app);
    play.update_objects(&mut app, Instant::now() + PUSH_EVERY);
    barrier(&mut play, &mut app);
    assert_eq!(game.calls(OBJECT_SET).len(), 1, "the retired salt cannot take a new frame");

    let new = game.replace_weapon(0);
    assert_ne!(old, new);
    until(&mut play, &mut app, "replacement object", |_, a| weapon_handle(a, 0) == new);
    move_weapon(&mut app, [4., 5., 6.]);
    until(&mut play, &mut app, "move the replacement", |_, _| {
        game.calls(OBJECT_SET).iter().any(|p| p["id"] == new && p["pos"] == json!([4.,5.,6.]))
    });
    let calls = game.calls(OBJECT_SET);
    assert_eq!(calls[0]["id"], old);
    assert_eq!(calls.iter().filter(|p| p["id"] == old).count(), 1);
}

#[test]
fn a_map_change_during_a_disconnected_drag_discards_the_old_world() {
    let (_t, game, mut app, mut play) = object_pair("object-drag-loss", SessionKind::Local, true);
    enable(&mut play, &mut app);
    let old = weapon_handle(&app, 0);
    app.session.begin_gesture();
    app.session.gesture_set([2., 3., 4.], [0.; 3]).unwrap();
    until(&mut play, &mut app, "drag on first map", |p, _| {
        !game.calls(OBJECT_SET).is_empty() && p.edits.sent.is_none()
    });
    barrier(&mut play, &mut app);
    let before = game.calls(OBJECT_SET).len();
    play.on_event(&mut app, Event::Lost("closed during drag".into()));
    app.session.gesture_set([10., 11., 12.], [0.; 3]).unwrap();
    app.session.end_gesture();
    game.bsp.fetch_add(1, Ordering::Relaxed);
    let new = game.replace_weapon(0);
    play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
    until(&mut play, &mut app, "reconnected to a different epoch", |p, a| {
        p.edits.epoch == Some(2) && p.edits.journal_ready && p.edit_refusal(a).is_none()
            && a.pairing.shown.as_ref().is_some_and(|s| s.epoch == 2)
    });
    assert!(!play.live_edits);
    assert_eq!(game.calls(OBJECT_SET).len(), before);
    enable(&mut play, &mut app);
    move_weapon(&mut app, [30., 31., 32.]);
    until(&mut play, &mut app, "edit after reconnect", |_, _| {
        game.calls(OBJECT_SET).iter().any(|p| p["id"] == new && p["pos"] == json!([30.,31.,32.]))
    });
    assert!(game.calls(OBJECT_SET)[before..].iter().all(|p| p["id"] != old));
}

#[test]
fn object_edits_stay_in_the_document_while_viewing() {
    let (_t, game, mut app, mut play) = object_pair("object-view", SessionKind::Local, true);
    move_weapon(&mut app, [2., 4., 8.]);
    app.session.remove_entry(WEAPONS, 1).unwrap();
    play.push_edits(&mut app);
    play.update_objects(&mut app, Instant::now() + PUSH_EVERY);
    barrier(&mut play, &mut app);
    assert!(!play.live_edits);
    for method in ["edit.batch", OBJECT_SET, OBJECT_DELETE, OBJECT_RESPAWN] {
        assert!(game.calls(method).is_empty(), "{method} in View");
    }
}

#[test]
fn reconnecting_during_a_drag_finishes_the_move_on_the_same_map() {
    let (_t, game, mut app, mut play) = object_pair("object-drag-return", SessionKind::Local, true);
    enable(&mut play, &mut app);
    let handle = weapon_handle(&app, 0);
    app.session.begin_gesture();
    app.session.gesture_set([2., 3., 4.], [0.; 3]).unwrap();
    play.push_edits(&mut app);
    play.update_objects(&mut app, Instant::now());
    // Consume the wire reply without delivering it to the editor. The game applied
    // the old frame, and the editor still has an outstanding request when it loses the link.
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let wait = deadline.saturating_duration_since(Instant::now());
        let event = play.link.as_ref().unwrap().next(wait).expect("object reply");
        if matches!(&event, Event::Answer { method, result: Ok(_), .. } if method == OBJECT_SET) {
            break;
        }
        play.on_event(&mut app, event);
        // A field acknowledgement makes the matching body move eligible immediately.
        play.update_objects(&mut app, Instant::now());
    }
    play.on_event(&mut app, Event::Lost("closed before move acknowledgement".into()));
    app.session.gesture_set([7., 8., 9.], [0.; 3]).unwrap();
    app.session.end_gesture();
    play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
    until(&mut play, &mut app, "final frame after reconnect", |p, _| {
        p.live_edits && game.calls(OBJECT_SET).iter().any(|call| {
            call["id"] == handle && call["pos"] == json!([7., 8., 9.])
        })
    });
    assert_eq!(game.calls(OBJECT_SET).len(), 2);
    assert!(game.calls(OBJECT_RESPAWN).is_empty());
    assert!(game.calls(OBJECT_DELETE).is_empty());
}

#[test]
fn releasing_an_unchanged_gizmo_after_reselection_sends_no_frame() {
    let (_t, game, mut app, mut play) = object_pair("object-click", SessionKind::Local, true);
    enable(&mut play, &mut app);
    let first = Instant::now() + PUSH_EVERY;
    app.session.begin_gesture();
    app.session.gesture_set([0., 1., 2.], [0.; 3]).unwrap();
    play.update_objects(&mut app, first);
    app.session.end_gesture();
    app.session.select(Some((WEAPONS, 1)));
    play.update_objects(&mut app, first + PUSH_EVERY / 2);
    barrier(&mut play, &mut app);
    assert!(game.calls("edit.batch").is_empty());
    assert!(game.calls(OBJECT_SET).is_empty());
}

#[test]
fn a_timed_out_preview_stays_pending_across_document_undo() {
    let (_t, game, mut app, mut play) = object_pair("object-add-unknown", SessionKind::Local, true);
    enable(&mut play, &mut app);
    game.fail_next_object_edit(agent_proto::code::TIMEOUT, "preview reply timed out");
    app.session.add_entry(WEAPONS, None).unwrap();
    until(&mut play, &mut app, "preview outcome unknown", |_, a| {
        a.pairing.pending.iter().any(|r| r["doc"] == "object" && r["class"] == tag_sync::RELOAD)
    });
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
    app.dispatch(Action::Undo);
    push_fields(&mut play, &mut app);
    play.update_objects(&mut app, Instant::now() + PUSH_EVERY);
    barrier(&mut play, &mut app);
    assert!(app.pairing.pending.iter().any(|r| {
        r["doc"] == "object" && r["class"] == tag_sync::RELOAD && r["unknown"] == true
    }));
    let decision = playtest::decide(
        &play.link_view(&app).state, None, &app.pairing.pending, false, true, true,
    );
    assert_eq!(decision, playtest::Step::Reload(playtest::Reload::Method));
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
    assert!(game.calls(OBJECT_DELETE).is_empty());
}

#[test]
fn undoing_the_creation_field_clears_respawn_and_its_refusal() {
    let (_t, game, mut app, mut play) = object_pair("object-undo-creation", SessionKind::Local, true);
    enable(&mut play, &mut app);
    app.session.apply(Command::set(WEAPONS, 0, "scale", FieldValue::Float(0.))).unwrap();
    until(&mut play, &mut app, "creation change acknowledged", |p, _| p.can_respawn());
    let why = "fixture weapon refuses replacement";
    game.fail_next_object_edit(agent_proto::sim::REFUSED, why);
    play.apply_edits(&mut app, Apply::Respawn).unwrap();
    until(&mut play, &mut app, "respawn refused", |_, a| {
        a.pairing.pending.iter().any(|row| row["why"] == why)
    });

    app.dispatch(Action::Undo);
    until(&mut play, &mut app, "undo clears creation difference and refusal", |p, a| {
        game.calls("edit.batch").len() == 2 && p.edits.sent.is_none() && !p.can_respawn()
            && a.pairing.pending.iter().all(|row| row["why"] != why)
    });
    assert!(play.apply_edits(&mut app, Apply::Respawn).is_err());
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
}

mod previews;

mod pacing;
