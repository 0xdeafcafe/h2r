//! Copy receipts and reconnects against the invented game's control socket.

use super::testing::Temp;
use super::testing::fake::{Game, until};
use super::tests::{editor, finder_for, move_flag, paired};
use super::*;
use agent::Access;
use agent_proto::sim::method::MAP_RELOAD;

#[test]
fn reconnect_rebuilds_acknowledged_edits_when_only_the_checksum_changes() {
    let (_t, game, mut app, mut play) = paired("reconnect-sum", "example", SessionKind::Local);
    play.finder = finder_for(&[&game]);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    move_flag(&mut app, [2., 4., 8.]);
    until(&mut play, &mut app, "acknowledged placement", |p, a| {
        !p.finder.running().is_empty()
            && game.calls("edit.batch").len() == 1
            && p.edits.sent.is_none()
            && a.pairing.pending.is_empty()
    });
    let epoch = play.edits.epoch;
    let before = play.baseline.clone();
    play.on_event(&mut app, Event::Lost("closed".into()));
    game.edits.lock().unwrap().checksum ^= 1;
    play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
    until(&mut play, &mut app, "changed checksum", |p, a| {
        p.is_paired() && p.edits.journal_ready && !a.pairing.pending.is_empty()
    });
    assert_eq!(play.edits.epoch, epoch);
    assert!(!play.live_edits);
    assert_ne!(play.baseline, before);
    assert_eq!(game.calls("edit.batch").len(), 1);
}

#[test]
fn accepted_reload_survives_a_lost_reply_and_transient_map_identity() {
    let t = Temp::new("reload-lost-reply");
    let game = Game::start("reload-lost-reply", "example", SessionKind::Local);
    let mut app = editor(&t);
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    play.start(
        &mut app,
        Endpoint::Unix(game.path.clone()),
        Options {
            call_timeout: Duration::from_millis(400),
            ..Options::default()
        },
    );
    until(&mut play, &mut app, "player destination", |p, a| {
        !p.finder.running().is_empty()
            && p.spawn_choice(a) == Spawn::Resume
            && p.edits.journal_ready
    });
    move_flag(&mut app, [1., 3., 5.]);
    game.edits.lock().unwrap().hold_reload_reply = true;
    play.reload_for_play(&mut app, playtest::Reload::Method).unwrap();
    // The fake accepts the file and starts loading, but withholds its reply.
    // The link's actual read timeout closes this control connection.
    until(&mut play, &mut app, "lost reload acknowledgement", |p, _| {
        matches!(p.conn, Conn::Reconnecting { .. })
    });
    assert_eq!(play.waiting, Waiting::Reload);
    assert!(play.pending_spawn.is_some());
    assert_eq!(game.calls(MAP_RELOAD).len(), 1);

    // On reconnect the same fake first fails edit.map, then reports the old
    // checksum at the new epoch, then exposes the new checksum at that epoch.
    until(&mut play, &mut app, "verified copy and restored player", |p, a| {
        p.waiting == Waiting::Idle
            && a.pairing.play_copy.is_some()
            && !game.calls(agent_proto::editor::method::FOCUS).is_empty()
    });
    assert_eq!(game.calls(MAP_RELOAD).len(), 1);
    assert_eq!(game.calls(agent_proto::sim::method::STATE_SET).len(), 1);
    assert!(game.calls("edit.batch").is_empty());
    assert!(!play.live_edits);
}

#[test]
fn reload_identity_timeout_keeps_unsent_edits_and_reports_the_outcome() {
    let (_t, game, mut app, mut play) = paired("reload-expiry", "example", SessionKind::Local);
    move_flag(&mut app, [0., 3., 1.]);
    {
        let _boundary = game.boundary.lock().unwrap();
        play.reload_for_play(&mut app, playtest::Reload::Method).unwrap();
        play.expire_reload(&mut app, Instant::now() + Duration::from_secs(31));
        assert_eq!(play.waiting, Waiting::Idle);
        assert!(play.reload_copy.is_none() && play.pending_spawn.is_none());
        assert!(app.log().iter().any(|line| line.status.text().contains("timed out")));
    }
    until(&mut play, &mut app, "late receipt and map identity", |p, a| {
        p.edits.epoch == Some(2) && !a.pairing.pending.is_empty()
    });
    assert!(app.pairing.play_copy.is_none());
    assert!(game.calls(agent_proto::sim::method::STATE_SET).is_empty());
    assert!(game.calls(agent_proto::editor::method::FOCUS).is_empty());
}

#[test]
fn a_refused_or_unqueued_foreign_reload_leaves_the_copy_pending() {
    for (tag, result, why) in [
        ("reload-refused", Err(agent_proto::RpcError {
            code: agent_proto::sim::REFUSED,
            message: "refused".into(),
            data: Some(json!({"why":"host cannot rebind the override"})),
        }), "host cannot rebind the override. Save the map and load it in the game."),
        ("reload-unqueued", Ok(json!({"queued":false})), "did not queue the reload"),
    ] {
        let (_t, game, mut app, mut play) = paired(tag, "example", SessionKind::Local);
        game.agent.add_game_method(MAP_RELOAD, Access::Act, move |_, _| result.clone());
        move_flag(&mut app, [9., 5., 1.]);
        play.reload_for_play(&mut app, playtest::Reload::Method).unwrap();
        until(&mut play, &mut app, "reload refusal", |p, a| {
            p.waiting == Waiting::Idle && a.log().iter().any(|l| l.status.text().contains(why))
        });
        assert!(play.last_command.is_none());
        assert!(!play.live_edits);
        assert!(play.pending_spawn.is_none() && play.reload_copy.is_none());
        assert!(!app.pairing.pending.is_empty());
        assert!(game.calls("edit.batch").is_empty());
        assert!(game.calls(agent_proto::editor::method::FOCUS).is_empty());
    }
}

#[test]
fn a_refused_owned_reload_prepares_a_relaunch() {
    let t = Temp::new("owned-reload-refusal");
    let maps = t.0.join("disc/maps");
    std::fs::create_dir_all(&maps).unwrap();
    std::fs::write(t.0.join("disc/default.xbe"), b"invented").unwrap();
    let file = maps.join("own-refusal.map");
    std::fs::write(&file, scenario_edit::fixture::fixture()).unwrap();
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(file));
    let mut play = Play::new();
    let game = Game::start("owned-reload-refusal", "own-refusal", SessionKind::Local);
    super::testing::fake::pair_for_view(&mut play, &mut app, &game);
    play.child_endpoint = play.endpoint().cloned();
    // A missing executable lets preparation finish without starting a game.
    play.h2_path = t.0.join("missing-h2").display().to_string();
    game.agent.add_game_method(MAP_RELOAD, Access::Act, |_, _| {
        Err(agent_proto::RpcError::new(agent_proto::sim::REFUSED, "cannot bind the copy"))
    });
    play.reload_for_play(&mut app, playtest::Reload::Method).unwrap();
    until(&mut play, &mut app, "owned relaunch", |_, a| {
        a.log().iter().any(|line| line.status.text().contains("missing-h2"))
    });
    assert!(app.log().iter().any(|l| l.status.text().contains("cannot bind the copy; relaunching")));
    assert!(play.reload_copy.is_none());
    assert_eq!(play.waiting, Waiting::Idle);
    assert!(!play.live_edits);
}
