use super::*;
use crate::api::Api;
use crate::play::{
    Apply,
    playtest::Reload,
    testing::fake::{UNIT, VEHICLE, until},
    tests::paired,
};
use studio_proto::method as m;

#[test]
fn stale_and_replaced_player_units_refuse_the_move() {
    let (_t, game, mut app, mut play) =
        paired("stale-player-move", "example", sim::SessionKind::Local);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    // A paused exact sample stays fresh while the test changes only the
    // displayed projection, independent of the clock and frame rate.
    play.pause();
    until(
        &mut play,
        &mut app,
        "paused game with a player scope",
        |p, a| p.paused && a.pairing.player_scope.is_ok() && a.pairing.shown.is_some(),
    );
    let pos = [1., 2., 3.];
    let request = PlayerMove::capture(&app, pos, false).unwrap();
    app.pairing.shown.as_mut().unwrap().stale = true;
    assert!(
        PlayerMove::capture(&app, pos, false)
            .unwrap_err()
            .contains("stale")
    );
    assert!(
        play.send_player_move(&app, request)
            .unwrap_err()
            .contains("stale")
    );
    let shown = app.pairing.shown.as_mut().unwrap();
    shown.stale = false;
    // The viewport still depicts a different salted unit for player.0.
    // Only a check of the exact sample can catch this at dispatch.
    shown.players[0].unit = Some(VEHICLE);
    let request = PlayerMove::capture(&app, pos, false).unwrap();
    let latest = app.pairing.mirror.as_ref().unwrap().sample(true).unwrap();
    assert_eq!(latest.players[0].unit, Some(UNIT));
    assert!(
        play.send_player_move(&app, request)
            .unwrap_err()
            .contains("unit changed")
    );
    play.player_here(&mut app, pos, false);
    assert!(app.status().unwrap().text().contains("unit changed"));
    assert!(game.calls(sim::method::STATE_SET).is_empty());
}

#[test]
fn playtest_transition_and_reload_copy_block_capture_and_dispatch() {
    let (_t, game, mut app, mut play) =
        paired("move-reload-guard", "example", sim::SessionKind::Local);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    play.publish(&mut app);
    let request = PlayerMove::capture(&app, [1., 2., 3.], false).unwrap();
    play.waiting = Waiting::Batch;
    play.publish(&mut app);
    let refusal = play.send_player_move(&app, request.clone()).unwrap_err();
    assert!(refusal.contains("playtest"));
    assert!(app.pairing.player_scope.is_err());
    assert_eq!(
        PlayerMove::capture(&app, [1., 2., 3.], false).unwrap_err(),
        refusal
    );

    let _boundary = game.boundary.lock().unwrap();
    play.reload_for_play(&mut app, Reload::Method).unwrap();
    assert!(play.reload_copy.is_some());
    play.waiting = Waiting::Idle; // Independently exercise the outstanding receipt.
    play.publish(&mut app);
    assert!(app.pairing.player_scope.is_err());
    assert_eq!(
        PlayerMove::capture(&app, [1., 2., 3.], false).unwrap_err(),
        refusal
    );
    assert_eq!(play.send_player_move(&app, request).unwrap_err(), refusal);
    assert!(game.calls(sim::method::STATE_SET).is_empty());
}

#[test]
fn split_screen_camera_holds_its_target_across_a_missing_unit() {
    let (_t, _game, mut app, _play) =
        paired("camera-split-target", "example", sim::SessionKind::Local);
    let sample = app.pairing.shown.as_mut().unwrap();
    let mut second = sample.players[0].clone();
    second.index = 3;
    second.local_user = Some(1);
    second.unit = Some(VEHICLE);
    second.aim = [0., -1., 0.];
    sample.players.push(second);
    let mut api = Api::new();
    let mode = api
        .call(
            &mut app,
            m::MIRROR_CAMERA,
            json!({"mode":"follow","target":"player.3"}),
        )
        .unwrap();
    assert_eq!(mode, json!({"mode":"follow","target":"player.3"}));
    assert_eq!(app.session.camera().target, [1., 2., 3.]);

    app.pairing.shown.as_mut().unwrap().players[1].unit = None;
    let before = *app.session.camera();
    Play::update_shared_camera(&mut app);
    assert_eq!(
        api.call(&mut app, m::MIRROR_CAMERA, json!({})).unwrap(),
        mode
    );
    assert_eq!(*app.session.camera(), before);
    // Switching camera modes during the respawn must keep the same index.
    let eyes = api
        .call(&mut app, m::MIRROR_CAMERA, json!({"mode":"eyes"}))
        .unwrap();
    assert_eq!(eyes, json!({"mode":"eyes","target":"player.3"}));
    assert_eq!(*app.session.camera(), before);
    app.pairing.shown.as_mut().unwrap().players[1].unit = Some(VEHICLE);
    Play::update_shared_camera(&mut app);
    let cam = app.session.camera();
    assert!(cam.forward()[1] < -0.999);
    assert!((cam.eye()[2] - (3. + select::EYE_HEIGHT)).abs() < 1e-4);
    app.pairing.shown.as_mut().unwrap().epoch += 1;
    Play::update_shared_camera(&mut app);
    assert_eq!(
        api.call(&mut app, m::MIRROR_CAMERA, json!({})).unwrap(),
        json!({"mode":"free","target":null})
    );
}

#[test]
fn control_worker_refuses_a_move_after_a_queued_respawn() {
    use crate::play::link::Purpose;
    use agent_proto::hot::method::EDIT_GET;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let (_t, game, mut app, mut play) =
        paired("worker-respawn", "example", sim::SessionKind::Local);
    play.apply_edits(&mut app, Apply::Send).unwrap();

    // Hold the first control reply across a stream update. The fake keeps
    // capturing game ticks on the binary connection throughout.
    let entered = Arc::new(AtomicBool::new(false));
    let release = Arc::new(AtomicBool::new(false));
    let holding = Arc::clone(&entered);
    let finish = Arc::clone(&release);
    game.agent
        .add_game_method(EDIT_GET, agent::Access::Read, move |_, ctx| {
            holding.store(true, Ordering::Release);
            let finish = Arc::clone(&finish);
            ctx.defer(
                300,
                Box::new(move |_| finish.load(Ordering::Acquire).then(|| Ok(json!({})))),
            );
            Ok(json!({}))
        });
    play.call(EDIT_GET, json!({}), Purpose::Other);
    until(&mut play, &mut app, "control reply held", |_, a| {
        entered.load(Ordering::Acquire) && a.pairing.player_scope.is_ok()
    });
    let request = PlayerMove::capture(&app, [1., 2., 3.], false).unwrap();
    play.send_player_move(&app, request).unwrap();
    let new_unit = 0x000f_0001;
    game.edits.lock().unwrap().replacement_unit = Some(new_unit);
    until(
        &mut play,
        &mut app,
        "respawn on the binary stream",
        |_, a| {
            a.pairing
                .mirror
                .as_ref()
                .and_then(|s| s.sample(true))
                .is_some_and(|s| {
                    s.players
                        .iter()
                        .any(|p| p.index == 0 && p.unit == Some(new_unit))
                })
        },
    );
    release.store(true, Ordering::Release);
    until(
        &mut play,
        &mut app,
        "refusal from the control worker",
        |_, a| {
            a.log()
                .iter()
                .any(|l| l.status.text() == "the local player's unit changed before the move")
        },
    );
    assert!(game.calls(sim::method::STATE_SET).is_empty());
}
