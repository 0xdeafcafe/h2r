//! Outcomes which have different meanings for the next edit on a placed object.

use super::*;

fn creation_edit(app: &mut App, play: &mut Play) {
    app.session.apply(Command::set(WEAPONS, 0, "scale", FieldValue::Float(0.))).unwrap();
    until(play, app, "creation field acknowledged", |p, _| {
        p.can_respawn() && p.edits.sent.is_none()
    });
}

#[test]
fn respawn_now_leaves_a_replacement_that_reused_the_requested_slot_alive() {
    let (_dir, game, mut app, mut play) = object_pair("raced-respawn", SessionKind::Local, true);
    let old = weapon_handle(&app, 0);
    enable(&mut play, &mut app);
    creation_edit(&mut app, &mut play);
    game.replace_weapon_before_respawn(0);
    play.apply_edits(&mut app, Apply::Respawn).unwrap();
    until(&mut play, &mut app, "the other occupant still has its placement link", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| {
            s.objects.iter().any(|o| {
                o.kind == 2 && o.placement == Some(0)
                    && Some(o.id) != agent_proto::ObjectId::parse(&old).map(|id| id.key)
            })
        })
    });
    barrier(&mut play, &mut app);
    let attempts = game.calls(OBJECT_RESPAWN);
    assert_eq!(attempts.len(), 1);
    assert_eq!(attempts[0]["id"], old);
    assert!(attempts[0].get("placement").is_none());
}

#[test]
fn partial_respawn_failure_allows_an_explicit_retry_of_the_empty_slot() {
    let (_dir, game, mut app, mut play) = object_pair("partial-respawn", SessionKind::Local, true);
    enable(&mut play, &mut app);
    creation_edit(&mut app, &mut play);
    let old = weapon_handle(&app, 0);
    game.fail_next_creation();
    play.apply_edits(&mut app, Apply::Respawn).unwrap();
    until(&mut play, &mut app, "empty slot and retained retry", |p, a| {
        p.can_respawn()
            && a.pairing.shown.as_ref().is_some_and(|s| {
                s.objects.iter().all(|o| o.kind != 2 || o.placement != Some(0))
            })
            && a.pairing.pending.iter().any(|r| {
                r["why"].as_str().is_some_and(|why| why.contains("object_new refused"))
            })
    });
    assert_eq!(game.calls(OBJECT_RESPAWN).len(), 1);
    assert_eq!(game.calls(OBJECT_RESPAWN)[0]["id"], old);
    play.apply_edits(&mut app, Apply::Respawn).unwrap();
    until(&mut play, &mut app, "explicit by-slot replacement", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| {
            s.objects.iter().any(|o| o.kind == 2 && o.placement.is_none())
        })
    });
    let attempts = game.calls(OBJECT_RESPAWN);
    assert_eq!(attempts.len(), 2);
    assert!(attempts[1].get("id").is_none());
    assert_eq!(attempts[1]["placement"], json!({"block":"weapons","index":0}));
}

#[test]
fn respawning_an_unlinked_preview_rechecks_its_returned_salt() {
    let (_dir, game, mut app, mut play) = object_pair("preview-respawn", SessionKind::Local, true);
    let old = weapon_handle(&app, 0);
    enable(&mut play, &mut app);
    creation_edit(&mut app, &mut play);
    play.apply_edits(&mut app, Apply::Respawn).unwrap();
    until(&mut play, &mut app, "first unlinked replacement", |p, a| {
        !p.can_respawn() && a.pairing.shown.as_ref().is_some_and(|s| {
            s.objects.iter().any(|o| o.kind == 2 && o.placement.is_none())
        })
    });
    let preview = app.pairing.shown.as_ref().unwrap().objects.iter()
        .find(|o| o.kind == 2 && o.placement.is_none()).unwrap().id;
    let preview = agent_proto::ObjectId { kind: agent_proto::Kind::Weapon, key: preview }.to_string();
    assert_ne!(old, preview);
    app.session.apply(Command::set(WEAPONS, 0, "scale", FieldValue::Float(1.))).unwrap();
    until(&mut play, &mut app, "preview's new creation field", |p, _| {
        p.can_respawn() && p.edits.sent.is_none()
    });
    play.apply_edits(&mut app, Apply::Respawn).unwrap();
    until(&mut play, &mut app, "respawn by preview handle", |_, _| {
        game.calls(OBJECT_RESPAWN).len() == 2
    });
    let attempts = game.calls(OBJECT_RESPAWN);
    assert_eq!(attempts[0]["id"], old);
    assert_eq!(attempts[1]["id"], preview);
    assert!(attempts.iter().all(|call| call.get("placement").is_none()));
}

#[test]
fn a_picked_up_weapon_survives_a_gone_delete_until_it_is_put_down() {
    let (_dir, game, mut app, mut play) = object_pair("pickup-delete", SessionKind::Local, true);
    let handle = weapon_handle(&app, 0);
    enable(&mut play, &mut app);
    game.pick_up_weapon_on_next_delete();
    app.session.remove_entry(WEAPONS, 0).unwrap();
    until(&mut play, &mut app, "the held weapon after GONE", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| {
            s.objects.iter().any(|o| o.kind == 2 && o.placement == Some(0) && o.parent.is_some())
        })
    });
    barrier(&mut play, &mut app);
    assert_eq!(game.calls(OBJECT_DELETE).len(), 1);
    let gone = format!("{handle} is gone");
    assert!(!play.toasts(Instant::now()).any(|(message, _)| message.contains(&gone)));
    assert!(!app.log().iter().any(|line| line.status.text().contains(&gone)));
    assert!(!app.pairing.pending.iter().any(|r| r["why"] == "still in the game's scenario until the map reloads"));
    app.dispatch(Action::Undo);
    play.update_objects(&mut app, Instant::now() + PUSH_EVERY);
    barrier(&mut play, &mut app);
    assert!(game.calls(OBJECT_RESPAWN).is_empty());
    app.dispatch(Action::Redo);
    game.put_down_weapon(0);
    until(&mut play, &mut app, "the free weapon can be deleted", |_, app| {
        app.pairing.shown.as_ref().is_some_and(|s| {
            s.objects.iter().all(|o| o.kind != 2 || o.placement != Some(0))
        })
    });
    let deletes = game.calls(OBJECT_DELETE);
    assert_eq!(deletes.len(), 2);
    assert!(deletes.iter().all(|v| v["id"] == handle));
    assert!(game.calls(OBJECT_RESPAWN).is_empty());
}

#[test]
fn a_changed_peer_count_retries_a_refused_object_edit_only_after_confirmation() {
    for (label, at_execution, reason) in [
        ("prequeue-peers", false, "confirm the current peer count for host-only edits"),
        ("execution-peers", true, "the session or its peer count changed before the edit"),
    ] {
        let (_dir, game, mut app, mut play) = object_pair(label, SessionKind::MpHost, true);
        enable(&mut play, &mut app);
        game.change_peers_on_next_object_edit(3, at_execution);
        app.session.remove_entry(WEAPONS, 0).unwrap();
        until(&mut play, &mut app, "new peer count and refused edit", |p, a| {
            p.peer_confirmation().is_some_and(|scope| scope.peers == 3)
                && a.pairing.pending.iter().any(|r| {
                    r["why"].as_str().is_some_and(|why| why.contains(reason))
                })
        });
        assert!(!app.pairing.pending.iter().any(|r| r["unknown"] == true));
        assert_eq!(game.calls(OBJECT_DELETE).len(), 1);
        let scope = play.peer_confirmation().unwrap();
        play.apply_edits(&mut app, Apply::Confirm(scope)).unwrap();
        until(&mut play, &mut app, "confirmed delete", |_, a| {
            a.pairing.shown.as_ref().is_some_and(|s| {
                s.objects.iter().all(|o| o.kind != 2 || o.placement != Some(0))
            })
        });
        let deletes = game.calls(OBJECT_DELETE);
        assert_eq!(deletes.len(), 2);
        assert_eq!(deletes[0]["id"], deletes[1]["id"]);
        assert_eq!(deletes[1]["confirm_peers"], 3);
    }
}

#[test]
fn a_timed_out_move_can_target_a_fresh_replacement_without_another_drag() {
    let (_dir, game, mut app, mut play) = object_pair("move-replacement", SessionKind::Local, true);
    let old = weapon_handle(&app, 0);
    enable(&mut play, &mut app);
    game.fail_next_object_edit(agent_proto::code::TIMEOUT, "fixture move timed out");
    move_weapon(&mut app, [15., 20., 25.]);
    until(&mut play, &mut app, "missing move reply", |_, a| {
        a.pairing.pending.iter().any(|r| r["unknown"] == true)
    });
    let replacement = game.replace_weapon(0);
    assert_ne!(replacement, old);
    until(&mut play, &mut app, "new salt at the requested pose", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| {
            s.objects.iter().any(|o| o.kind == 2 && o.placement == Some(0)
                && o.position == [15., 20., 25.])
        })
    });
    let moves = game.calls(OBJECT_SET);
    assert_eq!(moves.len(), 2);
    assert_eq!(moves[0]["id"], old);
    assert_eq!(moves[1]["id"], replacement);
    assert_eq!(moves[1]["pos"], json!([15., 20., 25.]));
}

#[test]
fn a_verified_single_machine_can_finish_a_peer_refused_delete() {
    let (_dir, game, mut app, mut play) = object_pair("host-alone", SessionKind::MpHost, true);
    enable(&mut play, &mut app);
    game.edits.lock().unwrap().peers = Some(3);
    until(&mut play, &mut app, "three machines", |p, _| {
        p.peer_confirmation().is_some_and(|scope| scope.peers == 3)
    });
    let scope = play.peer_confirmation().unwrap();
    play.apply_edits(&mut app, Apply::Confirm(scope)).unwrap();
    game.change_peers_on_next_object_edit(1, true);
    app.session.remove_entry(WEAPONS, 0).unwrap();
    until(&mut play, &mut app, "deletion after verified local permission", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| {
            s.objects.iter().all(|o| o.kind != 2 || o.placement != Some(0))
        })
    });
    let deletes = game.calls(OBJECT_DELETE);
    assert_eq!(deletes.len(), 2);
    assert_eq!(deletes[0]["id"], deletes[1]["id"]);
    assert_eq!(deletes[0]["confirm_peers"], 3);
    assert!(deletes[1]["confirm_peers"].is_null());
}
