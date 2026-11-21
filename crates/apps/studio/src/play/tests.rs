//! Play and pair against the agent crate's real server and executor, with
//! an invented game behind it: a status, one player and a vehicle, and the
//! host methods the editor uses (`edit.map`, `edit.batch`,
//! `sim.state.set`) recording what they are sent. No game, no game data.
#![cfg(unix)]

use std::sync::atomic::Ordering;
use std::thread;
use std::time::{Duration, Instant};

use agent_proto::Endpoint;
use agent_proto::sim::SessionKind;
use scenario_edit::{BlockKind, Command, Value as FieldValue};
use serde_json::{Value, json};

use super::testing::Temp;
use super::testing::fake::{Game, PLAYER_AT, SCNR, UNIT, VEHICLE, until};
use super::*;
use crate::app::{Action, App};

/// The editor with the invented map open as `example`.
pub(super) fn editor(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(t.map("example")));
    assert_eq!(app.loaded_map(), Some("example"));
    app
}

pub(super) fn paired(tag: &str, map: &str, kind: SessionKind) -> (Temp, Game, App, Play) {
    let t = Temp::new(tag);
    let game = Game::start(tag, map, kind);
    let mut app = editor(&t);
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "the paired stream", |p, a| {
        p.is_paired()
            && p.edits.journal_ready
            && p.session.is_some()
            && p.scenario_tag.is_some()
            && a.pairing.player().is_some()
    });
    (t, game, app, play)
}

pub(super) fn move_flag(app: &mut App, to: [f32; 3]) {
    app.session
        .apply(Command::set(
            BlockKind::NetgameFlags,
            0,
            "position",
            FieldValue::Vec3(to),
        ))
        .unwrap();
}

#[test]
fn pairing_shows_the_game_and_its_objects() {
    let (_t, _game, mut app, mut play) = paired("show", "example", SessionKind::MpHost);
    assert_eq!(play.map(), Some("example"));
    assert!(matches!(
        play.link_view(&app).state,
        LinkState::Viewing { .. }
    ));
    let sample = app.pairing.shown.as_ref().unwrap();
    let ids: Vec<u32> = sample.objects.iter().map(|o| o.id).collect();
    assert_eq!(ids, [UNIT, VEHICLE]);
    assert_eq!(app.pairing.player().unwrap().0.name, "Tester");
    assert_eq!(play.edits_blocked(&app).as_deref(), Some("View mode"));
    // Go to player: the camera's focus is on them, looking their way (+y).
    play.go_to_player(&mut app);
    let cam = *app.session.camera();
    assert_eq!(cam.target[..2], [10.0, -4.0]);
    let f = cam.forward();
    assert!(f[1] > 0.9 && f[0].abs() < 1e-3, "{f:?}");
    // Unpairing ends it.
    play.unpair(&mut app);
    assert_eq!(play.link_view(&app).state, LinkState::NoGame);
    assert!(app.pairing.mirror.is_none() && app.pairing.shown.is_none());
}

#[test]
fn map_identity_tracks_the_frame_epoch_and_the_loaded_file() {
    let t = Temp::new("map-identity");
    let mut app = editor(&t);
    let sample = live_mirror::Sample {
        tick: 100, epoch: 2, stale: false, objects: Vec::new(), players: Vec::new()
    };
    let checksum = app.session.source_checksum().unwrap();
    app.pairing.stream_map = json!({"name": "example", "epoch": 1, "checksum": checksum});
    assert!(!app.pairing.matches_sample(&app, &sample));
    app.pairing.stream_map["epoch"] = json!(2);
    assert!(app.pairing.matches_sample(&app, &sample));
    move_flag(&mut app, [2.0, 4.0, 8.0]);
    assert!(app.pairing.matches_sample(&app, &sample));
    let other = !checksum;
    app.pairing.stream_map["checksum"] = json!(other);
    assert!(!app.pairing.matches_sample(&app, &sample));
    app.pairing.play_copy = Some((app.session.world_rev(), other));
    assert!(app.pairing.matches_sample(&app, &sample));
    app.pairing.stream_map["name"] = json!("another");
    assert!(!app.pairing.matches_sample(&app, &sample));
}

#[test]
fn a_moved_game_object_reaches_mirror_objects_within_two_frames() {
    let (_t, game, mut app, mut play) = paired("motion", "example", SessionKind::Local);
    let mut api = crate::api::Api::new();
    let mut read = |app: &mut App| {
        let answer = api.call(app, studio_proto::method::MIRROR_OBJECTS, json!({"kind":"vehi"})).unwrap();
        assert_eq!(answer["objects"].as_array().unwrap().len(), 1);
        assert_eq!(answer["objects"][0]["id"], agent_proto::sim::objects::handle(VEHICLE));
        (answer["tick"].as_u64().unwrap(), answer["objects"][0]["pos"].clone())
    };
    // Hold the fake game, so the test controls how many frames it can send.
    play.pause();
    until(&mut play, &mut app, "the paused tick", |p, a| {
        p.status.as_ref().is_some_and(|s| s.at.paused &&
            s.at.tick == a.pairing.shown.as_ref().map(|shown| shown.tick))
    });
    let (before, old) = read(&mut app);
    assert_eq!(old, json!([1.0, 2.0, 3.0]));
    game.moved.store(true, Ordering::Relaxed);
    play.step(2);
    until(&mut play, &mut app, "two game ticks", |_, a| {
        a.pairing.shown.as_ref().is_some_and(|s| s.tick == before + 2)
    });
    let (after, moved) = read(&mut app);
    assert_eq!(after, before + 2);
    assert_eq!(moved, json!([5.0, 2.0, 3.0]));
    play.resume();
    until(&mut play, &mut app, "the resume", |p, _| !p.paused);
}

#[test]
fn a_moved_placement_is_written_to_the_games_scenario() {
    let (_t, game, mut app, mut play) = paired("edit", "example", SessionKind::Local);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    until(&mut play, &mut app, "the edit", |_, _| {
        !game.calls("edit.batch").is_empty()
    });
    let sets = game.calls("edit.batch");
    assert_eq!(sets.len(), 1, "{sets:?}");
    let s = &sets[0]["writes"][0];
    assert_eq!(s["tag"], SCNR);
    assert_eq!(s["group"], "scnr");
    let b = scenario_edit::layout::block(BlockKind::NetgameFlags);
    let f = b.field("position").unwrap();
    assert_eq!(
        s["path"],
        format!(
            "+{:#x}/{:#x}[0].+{:#x}:bytes[12]",
            b.offset, b.stride, f.offset
        )
    );
    let want: Vec<u8> = [5.0f32, 6.0, 7.0]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    assert_eq!(s["value"], sync::hex(&want));
    until(&mut play, &mut app, "the answer", |p, _| {
        p.edits.sent.is_none()
    });
    // Nothing more goes until something changes; undo sends the old bytes.
    play.update(&mut app);
    app.dispatch(Action::Undo);
    until(&mut play, &mut app, "the undo", |_, _| {
        game.calls("edit.batch").len() == 2
    });
    assert_ne!(
        game.calls("edit.batch")[1]["writes"][0]["value"],
        s["value"]
    );
    // An added entry is a reload, not a write.
    app.session.set_allow_growth(true);
    let kind = scenario_edit::layout::BLOCKS
        .iter()
        .map(|b| b.kind)
        .find(|k| app.session.add_refusal(*k).is_none())
        .expect("the invented map has a block that grows");
    app.session.add_entry(kind, None).unwrap();
    until(&mut play, &mut app, "the reload note", |p, _| {
        p.reload == [kind]
    });
    assert_eq!(game.calls("edit.batch").len(), 2);
}

#[test]
fn replacing_a_placement_only_pokes_other_blocks() {
    let (_t, game, mut app, mut play) = paired("replace-slot", "example", SessionKind::Local);
    let kind = BlockKind::Weapons;
    app.session.remove_entry(kind, 0).unwrap();
    app.session.add_entry(kind, None).unwrap();
    move_flag(&mut app, [17.0, 18.0, 19.0]);

    play.apply_edits(&mut app, Apply::Send).unwrap();
    until(&mut play, &mut app, "one flag write", |p, a| {
        p.edits.sent.is_none() && !game.calls("edit.batch").is_empty()
            && a.pairing.pending.iter().all(|row| row["class"] == "Reload")
    });
    let calls = game.calls("edit.batch");
    assert_eq!(calls.len(), 1);
    let writes = calls[0]["writes"].as_array().unwrap();
    assert_eq!(writes.len(), 1);
    let flags = scenario_edit::layout::block(BlockKind::NetgameFlags);
    assert_eq!(
        writes[0]["path"],
        sync::path(flags.kind, 0, flags.field("position").unwrap())
    );
    assert_eq!(app.pairing.pending.len(), 1);
    assert_eq!(app.pairing.pending[0]["path"], "Weapons");
}

#[test]
fn reloading_a_play_copy_uses_the_layout_at_save_time() {
    let (_t, game, mut app, mut play) = paired("copy-layout", "example", SessionKind::Local);
    let kind = BlockKind::Weapons;
    app.session.remove_entry(kind, 0).unwrap();
    app.session.add_entry(kind, None).unwrap();
    play.apply_edits(&mut app, Apply::Send).unwrap();
    play.push_edits(&mut app);
    assert_eq!(play.reload, [kind]);
    {
        let _boundary = game.boundary.lock().unwrap();
        play.apply_edits(&mut app, Apply::Reload).unwrap();
        // Undo in the document after saving, before the game loads the copy.
        assert!(app.session.undo());
        assert!(app.session.undo());
        play.push_edits(&mut app);
        assert!(play.reload.is_empty());
    }
    until(&mut play, &mut app, "the saved layout at the new epoch", |p, a| {
        p.edits.epoch == Some(2)
            && a.pairing.play_copy.is_some()
            && p.reload == [kind]
            && p.waiting == Waiting::Idle
            && p.edit_refusal(a).is_none()
    });
    let loads = game.calls("map.reload");
    assert_eq!(loads.len(), 1);
    std::fs::remove_file(loads[0]["file"].as_str().unwrap()).unwrap();

    // Redo returns to the copy's slot order, so a field edit can reach it.
    assert!(app.session.redo());
    assert!(app.session.redo());
    play.push_edits(&mut app);
    assert!(app.pairing.pending.is_empty());
    assert!(game.calls("edit.batch").is_empty());
    play.apply_edits(&mut app, Apply::Send).unwrap();
    app.session
        .apply(Command::set(
            kind,
            0,
            "position",
            FieldValue::Vec3([3., 4., 5.]),
        ))
        .unwrap();
    until(&mut play, &mut app, "the field in the reloaded block", |p, a| {
        !game.calls("edit.batch").is_empty()
            && p.edits.sent.is_none()
            && a.pairing.pending.is_empty()
    });
    let calls = game.calls("edit.batch");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["writes"].as_array().unwrap().len(), 1);
    let block = scenario_edit::layout::block(kind);
    assert_eq!(
        calls[0]["writes"][0]["path"],
        sync::path(kind, 0, block.field("position").unwrap())
    );
}

#[test]
fn teleport_and_spawn_move_the_player() {
    let (_t, game, mut app, mut play) = paired("tp", "example", SessionKind::MpHost);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    play.update(&mut app);
    app.session.camera_mut().target = [3.0, 4.0, 5.0];
    play.teleport_to_camera(&mut app);
    until(&mut play, &mut app, "the teleport", |_, _| {
        !game.calls("sim.state.set").is_empty()
    });
    let s = &game.calls("sim.state.set")[0];
    assert_eq!(s["player"], "player.0");
    // No structure in the invented map: the point itself, lifted.
    assert_eq!(s["pos"], json!([3.0, 4.0, 5.0 + f64::from(LIFT)]));
    // A launched game's spawn waits for gameplay, then moves the player.
    play.pending_spawn = Some(Destination::Position([1.0, 2.0, 3.0]));
    until(&mut play, &mut app, "the spawn", |_, _| {
        game.calls("sim.state.set").len() == 2
    });
    assert_eq!(
        game.calls("sim.state.set")[1]["pos"],
        json!([1.0, 2.0, 3.0])
    );
}

#[test]
fn a_coop_session_gets_no_writes() {
    let (_t, game, mut app, mut play) = paired("coop", "example", SessionKind::CoopHost);
    let why = play.edits_blocked(&app).unwrap();
    assert!(why.contains("co-op"), "{why}");
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    play.teleport_to_camera(&mut app);
    play.pending_spawn = Some(Destination::Position([1.0, 2.0, 3.0]));
    until(&mut play, &mut app, "co-op spawn refusal", |p, _| {
        p.pending_spawn.is_none()
    });
    assert!(game.calls("edit.batch").is_empty());
    assert!(game.calls("sim.state.set").is_empty());
    assert!(
        play.pending_spawn.is_none(),
        "the spawn is dropped, not kept"
    );
}

#[test]
fn another_map_in_the_game_blocks_edits() {
    let (_t, game, mut app, mut play) = paired("other", "zanzibar", SessionKind::Local);
    let why = play.edits_blocked(&app).unwrap();
    assert!(why.contains("zanzibar") && why.contains("example"), "{why}");
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    play.push_edits(&mut app);
    assert!(!app.pairing.pending.is_empty());
    assert!(play.edits.sent.is_none());
    assert!(game.calls("edit.batch").is_empty());
}

#[test]
fn play_releases_the_claim_after_the_pause_expires() {
    let (_t, game, mut app, mut play) = paired("expired-claim", "example", SessionKind::Local);
    app.pairing.pause_while_editing = true;
    play.window_focus(&app, true);
    until(&mut play, &mut app, "focus pause", |p, _| p.paused && p.controller);
    play.call(method::PAUSE, json!({"max_ms": 1}), Purpose::Control("pause"));
    until(&mut play, &mut app, "expired pause with claim", |p, _| {
        !p.paused && p.controller && !game.agent.is_paused()
            && p.status.as_ref().is_some_and(|s| !s.at.paused && s.controller)
    });
    play.request(Request::Play);
    until(&mut play, &mut app, "released claim", |p, _| {
        !game.calls(agent_proto::editor::method::FOCUS).is_empty()
            && p.status.as_ref().is_some_and(|s| !s.at.paused && !s.controller)
    });
}

#[test]
fn spawn_waits_for_the_move_reply_and_defers_focus_pause() {
    let (_t, game, mut app, mut play) = paired("spawn-reply", "example", SessionKind::Local);
    until(&mut play, &mut app, "spawn tick and local player", |p, a| {
        p.status.as_ref().is_some_and(|s| s.at.map_ticks >= SPAWN_AFTER_TICKS)
            && p.spawn_choice(a) == Spawn::Resume
            && p.map_refusal(a).is_none()
    });
    app.pairing.pause_while_editing = true;
    play.waiting = Waiting::Launch;
    play.pending_spawn = Some(Destination::Position([1.0, 2.0, 3.0]));
    play.window_focus(&app, true);
    assert!(play.pause_on_return);
    assert!(!play.controller);
    let boundary = game.boundary.lock().unwrap();
    game.agent.add_game_method(agent_proto::sim::method::STATE_SET, agent::Access::Act, |_, _| {
        Err(agent_proto::RpcError::new(agent_proto::sim::REFUSED, "fixture move refused"))
    });
    play.maybe_spawn(&mut app);
    play.continue_playtest(&mut app);
    assert!(matches!(play.waiting, Waiting::Spawn(_)));
    assert!(play.pending_spawn.is_some());
    assert!(game.calls(agent_proto::editor::method::FOCUS).is_empty());
    drop(boundary);
    until(&mut play, &mut app, "move refusal and deferred pause", |p, a| {
        p.paused && a.log().iter().any(|l| l.status.text().contains("fixture move refused"))
    });
    assert_eq!(play.waiting, Waiting::Idle);
    assert!(play.pending_spawn.is_none());
    assert!(game.calls(agent_proto::editor::method::FOCUS).is_empty());
}

#[test]
fn waiting_for_a_missing_local_player_expires() {
    let (_t, game, mut app, mut play) = paired("spawn-expiry", "example", SessionKind::Local);
    until(&mut play, &mut app, "spawn tick", |p, _| {
        p.status.as_ref().is_some_and(|s| s.at.map_ticks >= SPAWN_AFTER_TICKS)
    });
    app.pairing.shown = None;
    play.waiting = Waiting::Launch;
    play.pending_spawn = Some(Destination::Position([4.0, 5.0, 6.0]));
    play.maybe_spawn(&mut app);
    assert!(play.spawn_deadline.is_some());
    assert_eq!(play.waiting, Waiting::Launch);
    play.spawn_deadline = Some(Instant::now());
    play.maybe_spawn(&mut app);
    play.continue_playtest(&mut app);
    assert_eq!(play.waiting, Waiting::Idle);
    assert!(play.pending_spawn.is_none());
    assert!(app.status().unwrap().text().contains("timed out waiting for the local player"));
    assert!(game.calls(agent_proto::sim::method::STATE_SET).is_empty());
    assert!(game.calls(agent_proto::editor::method::FOCUS).is_empty());
}

#[test]
fn pause_step_and_resume_hold_control_only_while_needed() {
    let (_t, game, mut app, mut play) = paired("pause", "example", SessionKind::Local);
    play.pause();
    until(&mut play, &mut app, "the pause", |p, _| {
        p.status.as_ref().is_some_and(|s| s.at.paused)
    });
    assert!(game.agent.is_paused());
    assert!(play.link_view(&app).paused);
    assert!(play.controller);
    play.step(2);
    until(&mut play, &mut app, "the step", |_, a| {
        a.log()
            .iter()
            .any(|l| matches!(&l.status, editor_core::Status::Info(m) if m == "game: step"))
    });
    play.resume();
    until(&mut play, &mut app, "the resume", |p, _| {
        p.status
            .as_ref()
            .is_some_and(|s| !s.at.paused && !s.controller)
    });
    assert!(!game.agent.is_paused());
}

#[test]
fn a_game_that_goes_away_returns_to_no_game() {
    let (_t, game, mut app, mut play) = paired("lost", "example", SessionKind::Local);
    drop(game);
    until(&mut play, &mut app, "the loss", |p, _| {
        p.conn == Conn::Off
    });
    assert_eq!(play.link_view(&app).state, LinkState::NoGame);
}

#[test]
fn nothing_listening_is_reported() {
    let t = Temp::new("none");
    let mut app = editor(&t);
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(t.0.join("nobody.sock")));
    until(&mut play, &mut app, "the failure", |p, _| {
        matches!(p.conn, Conn::Lost(_))
    });
}

#[test]
fn play_writes_the_edited_map_for_the_override() {
    let t = Temp::new("prepare");
    let disc = t.0.join("disc");
    std::fs::create_dir_all(disc.join("maps")).unwrap();
    std::fs::write(disc.join("default.xbe"), b"invented").unwrap();
    let source = disc.join("maps/example.map");
    std::fs::copy(t.map("example"), &source).unwrap();
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(source));
    move_flag(&mut app, [5.0, 6.0, 7.0]);

    let mut play = Play::new();
    play.h2_path = "/opt/h2".into();
    let launch = play.prepare(&mut app).unwrap();
    assert_eq!(launch.level, "example");
    assert_eq!(launch.disc, disc);
    assert_eq!(launch.h2, std::path::PathBuf::from("/opt/h2"));

    let mut session = scenario_edit::Session::new();
    let copy = launch.map_dir.unwrap().join("example.map");
    session
        .open(scenario_edit::MapSource::file(&copy))
        .unwrap();
    let position = session
        .scenario()
        .unwrap()
        .get(BlockKind::NetgameFlags, 0, "position")
        .unwrap()
        .as_vec3()
        .unwrap();
    assert_eq!(position, [5.0, 6.0, 7.0]);
    assert!(app.is_dirty(), "writing the play copy is not a save");
    let _ = std::fs::remove_file(copy);
}

#[test]
fn pairing_accepts_the_cache_identity_of_a_renamed_document() {
    let (_t, game, mut app, mut play) = paired("cache-name", "fixture", SessionKind::Local);
    assert_eq!(app.loaded_map(), Some("example"));
    assert_eq!(play.map_match(&app), MapMatch::Same);
    until(&mut play, &mut app, "sample for the cache name", |p, a| {
        p.edit_refusal(a).is_none()
            && a.pairing.shown.as_ref().is_some_and(|s| a.pairing.matches_sample(a, s))
    });
    app.pairing.stream_map["checksum"] = Value::Null;
    assert!(!app.pairing.matches_sample(&app, app.pairing.shown.as_ref().unwrap()));

    game.edits.lock().unwrap().checksum ^= 1;
    until(&mut play, &mut app, "different cache checksum", |p, a| {
        p.edit_refusal(a).is_some_and(|why| why.contains("checksum"))
    });
}

#[test]
fn client_live_and_different_checksums_stay_in_view() {
    for (label, kind) in [
        ("client", SessionKind::MpClient),
        ("live", SessionKind::Local),
        ("checksum", SessionKind::Local),
    ] {
        let (_t, game, mut app, mut play) = paired(label, "example", kind);
        {
            let mut state = game.edits.lock().unwrap();
            state.external_live = label == "live";
            state.checksum = u32::from(label == "checksum");
        }
        until(
            &mut play,
            &mut app,
            "session or checksum refusal",
            |p, a| matches!(p.link_view(a).state,LinkState::Blocked {ref why} if why.to_lowercase().contains(label)),
        );
        move_flag(&mut app, [1., 2., 3.]);
        assert!(play.apply_edits(&mut app, Apply::Send).is_err());
        play.push_edits(&mut app);
        assert!(play.edits.sent.is_none());
        assert!(game.calls("edit.batch").is_empty());
    }
}

#[test]
fn host_peer_confirmation_allows_loading_another_map_but_keeps_field_writes_guarded() {
    let (_t, game, mut app, mut play) = paired("peer-map", "other", SessionKind::MpHost);
    game.edits.lock().unwrap().peers = Some(3);
    until(&mut play, &mut app, "host peer count", |_, a| {
        a.pairing.peer_confirmation.is_some_and(|scope| scope.peers == 3)
    });

    let mut api = crate::api::Api::new();
    let confirm = |peers| json!({"what":"confirm", "peers":peers});
    let method = studio_proto::method::MIRROR_APPLY;
    assert_eq!(
        api.call(&mut app, method, confirm(2)).unwrap_err().code,
        studio_proto::code::REFUSED
    );
    api.call(&mut app, method, confirm(3)).unwrap();
    play.update(&mut app);
    assert_eq!(play.session_refusal(&app), None);
    move_flag(&mut app, [2., 3., 4.]);
    assert!(play.apply_edits(&mut app, Apply::Send).is_err());
    assert!(game.calls("edit.batch").is_empty());

    play.playtest(&mut app, None).unwrap();
    let load = app.pairing.prompt.clone().unwrap();
    assert_eq!(load.ask, playtest::Ask::Load);
    play.playtest(&mut app, Some((load, "load".into()))).unwrap();
    until(&mut play, &mut app, "confirmed host reload", |_, _| {
        !game.calls(agent_proto::sim::method::MAP_RELOAD).is_empty()
    });
    assert_eq!(
        game.calls(agent_proto::sim::method::MAP_RELOAD)[0]["confirm_peers"],
        3
    );
}

#[test]
fn host_peer_confirmation_expires_with_the_count_epoch_or_pairing() {
    let (_t, game, mut app, mut play) = paired("peer-scope", "example", SessionKind::MpHost);
    game.edits.lock().unwrap().peers = Some(2);
    until(&mut play, &mut app, "two peers", |_, a| {
        a.pairing.peer_confirmation.is_some_and(|scope| scope.peers == 2)
    });
    let first = app.pairing.peer_confirmation.unwrap();
    play.apply_edits(&mut app, Apply::Confirm(first)).unwrap();
    game.edits.lock().unwrap().peers = Some(3);
    until(&mut play, &mut app, "three peers", |_, a| {
        a.pairing.peer_confirmation.is_some_and(|scope| scope.peers == 3)
    });
    assert!(play.apply_edits(&mut app, Apply::Confirm(first)).is_err());
    let second = app.pairing.peer_confirmation.unwrap();
    assert!(play.session_refusal(&app).unwrap().contains("confirm"));
    play.apply_edits(&mut app, Apply::Confirm(second)).unwrap();

    game.bsp.fetch_add(1, Ordering::Relaxed);
    until(&mut play, &mut app, "new map epoch", |p, a| {
        p.edits.journal_ready
            && a.pairing.peer_confirmation.is_some_and(|s| s.epoch != second.epoch)
    });
    assert!(play.apply_edits(&mut app, Apply::Confirm(second)).is_err());
    let third = app.pairing.peer_confirmation.unwrap();
    assert!(play.session_refusal(&app).unwrap().contains("confirm"));
    play.apply_edits(&mut app, Apply::Confirm(third)).unwrap();

    play.unpair(&mut app);
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "new pairing on the same epoch", |p, a| {
        p.edits.journal_ready && a.pairing.peer_confirmation.is_some()
    });
    let current = app.pairing.peer_confirmation.unwrap();
    assert_eq!((current.peers, current.epoch), (third.peers, third.epoch));
    assert!(play.apply_edits(&mut app, Apply::Confirm(third)).is_err());
    assert!(play.session_refusal(&app).unwrap().contains("confirm"));
    play.apply_edits(&mut app, Apply::Confirm(current)).unwrap();
    assert_eq!(play.session_refusal(&app), None);
}

fn tag_pair(name: &str) -> (Temp, Game, App, Play) {
    let t = Temp::new(name);
    let game = Game::start(name, "example", SessionKind::Local);
    let (mut app, _, _) = crate::app::tag_tests::app(&t.0);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "game identity and journal", |p, a| {
        p.edits.journal_ready && p.edit_refusal(a).is_none()
    });
    (t, game, app, play)
}

fn speed(app: &mut App) {
    let action = crate::app::tag_api::set_field(
        &app.tags,
        crate::app::tag_tests::weap_id(),
        "speed",
        &json!(4.5),
    )
    .unwrap();
    app.dispatch(Action::Tag(action));
}

#[test]
fn tag_and_placement_share_a_batch_and_undo_sends_the_old_field() {
    use crate::app::tag_tests::WEAP;
    let (_t, game, mut app, mut play) = tag_pair("tag-undo");
    speed(&mut app);
    move_flag(&mut app, [3., 4., 5.]);
    play.push_edits(&mut app);
    assert!(game.calls("edit.batch").is_empty());
    assert_eq!(app.pairing.pending.len(), 2);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    until(&mut play, &mut app, "one atomic batch", |p, a| {
        p.edits.sent.is_none() && a.pairing.pending.is_empty()
    });
    let calls = game.calls("edit.batch");
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["writes"].as_array().unwrap().len(), 2);
    assert_eq!(
        calls[0]["writes"][0],
        json!({"at":WEAP,"was":sync::hex(&2.5f32.to_le_bytes()),"value":sync::hex(&4.5f32.to_le_bytes())})
    );
    app.dispatch(Action::Undo);
    until(&mut play, &mut app, "tag undo", |_, _| {
        game.calls("edit.batch").len() == 2
    });
    let undo = &game.calls("edit.batch")[1]["writes"][0];
    assert_eq!(undo["value"], calls[0]["writes"][0]["was"]);
    assert_eq!(undo["was"], calls[0]["writes"][0]["value"]);
}

#[test]
fn taking_reads_the_current_field_and_overwrite_checks_the_mismatch_value() {
    use crate::app::tag_tests::WEAP;
    for take in [true, false] {
        let (_t, game, mut app, mut play) = tag_pair(if take {
            "take-field"
        } else {
            "overwrite-field"
        });
        let holds = sync::hex(&9f32.to_le_bytes());
        game.edits
            .lock()
            .unwrap()
            .bytes
            .insert(WEAP.to_string(), holds.clone());
        speed(&mut app);
        play.apply_edits(&mut app, Apply::Send).unwrap();
        until(&mut play, &mut app, "both conflict choices", |_, a| {
            a.pairing.pending.iter().any(|r| r["conflict"] == true)
        });
        assert_eq!(game.edits.lock().unwrap().bytes[&WEAP.to_string()], holds);
        if take {
            game.edits.lock().unwrap().bytes.insert(
                WEAP.to_string(),
                sync::hex(&11f32.to_le_bytes()),
            );
        }
        play.apply_edits(
            &mut app,
            if take {
                Apply::Take(WEAP)
            } else {
                Apply::Overwrite(WEAP)
            },
        )
        .unwrap();
        until(&mut play, &mut app, "document matches the game", |p, a| {
            a.pairing.pending.is_empty() && p.edits.sent.is_none()
        });
        let expected = sync::hex(&(if take { 11f32 } else { 4.5f32 }).to_le_bytes());
        assert_eq!(
            sync::hex(
                &app.tags
                    .read(crate::app::tag_tests::weap_id(), WEAP, 4)
                    .unwrap()
            ),
            expected
        );
        assert_eq!(
            game.edits.lock().unwrap().bytes[&WEAP.to_string()],
            expected
        );
        assert_eq!(game.calls("edit.batch").len(), if take { 1 } else { 2 });
        assert_eq!(game.calls("edit.read").len(), if take { 1 } else { 0 });
    }
}

#[test]
fn taking_a_placement_reads_its_path_in_the_paired_epoch() {
    let (_t, game, mut app, mut play) = paired("take-placement", "example", SessionKind::Local);
    let kind = BlockKind::NetgameFlags;
    let block = app.session.scenario().unwrap().block(kind);
    let pos = block.def.field("position").unwrap();
    // The Play copy can resolve a placement outside the document's original block.
    let at = block.address + pos.offset + 0x10000;
    let key = json!(sync::path(kind, 0, pos)).to_string();
    let encode = |pos: [f32; 3]| {
        let bytes: Vec<_> = pos.into_iter().flat_map(f32::to_le_bytes).collect();
        sync::hex(&bytes)
    };
    {
        let mut state = game.edits.lock().unwrap();
        state.placements.insert(key.clone(), at);
        state.bytes.insert(key.clone(), encode([9., 8., 7.]));
    }
    move_flag(&mut app, [6., 5., 4.]);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    until(&mut play, &mut app, "placement conflict", |_, a| {
        a.pairing.pending.iter().any(|r| r["at"] == at && r["conflict"] == true)
    });
    let latest = [13., 14., 15.];
    game.edits.lock().unwrap().bytes.insert(key, encode(latest));
    play.apply_edits(&mut app, Apply::Take(at)).unwrap();
    until(&mut play, &mut app, "latest placement value", |_, a| {
        a.pairing.pending.is_empty()
    });
    assert_eq!(
        app.session.scenario().unwrap().get(kind, 0, "position").unwrap(),
        FieldValue::Vec3(latest)
    );
    assert_eq!(game.calls("edit.get").len(), 1);
    assert_eq!(game.calls("edit.batch").len(), 1);
}

#[test]
fn reverting_a_changed_field_uses_its_observed_bytes_on_the_next_send() {
    use crate::app::tag_tests::WEAP;
    let (_t, game, mut app, mut play) = tag_pair("revert-baseline");
    speed(&mut app);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    until(&mut play, &mut app, "first write acknowledged", |p, _| {
        game.calls("edit.batch").len() == 1 && p.edits.sent.is_none()
    });

    let holds = sync::hex(&7f32.to_le_bytes());
    game.edits.lock().unwrap().bytes.insert(WEAP.to_string(), holds.clone());
    play.apply_edits(&mut app, Apply::Revert).unwrap();
    until(&mut play, &mut app, "revert finished", |p, a| {
        !game.calls("edit.revert").is_empty()
            && p.edits.journal_ready
            && a.log().iter().any(|l| l.status.text().contains("left those alone"))
    });
    assert_eq!(game.edits.lock().unwrap().bytes[&WEAP.to_string()], holds);
    assert_eq!(
        app.tags
            .read(crate::app::tag_tests::weap_id(), WEAP, 4)
            .unwrap(),
        4.5f32.to_le_bytes()
    );

    play.apply_edits(&mut app, Apply::Send).unwrap();
    until(&mut play, &mut app, "write against the observed bytes", |p, a| {
        game.calls("edit.batch").len() == 2
            && p.edits.sent.is_none()
            && a.pairing.pending.is_empty()
    });
    assert_eq!(game.calls("edit.batch")[1]["writes"][0]["was"], holds);
}

#[test]
fn map_identity_accepts_case_differences_for_live_edits() {
    let (_t, game, mut app, mut play) = paired("case-map", "Example", SessionKind::Local);
    assert_eq!(app.loaded_map(), Some("example"));
    play.apply_edits(&mut app, Apply::Send).unwrap();
    move_flag(&mut app, [3., 4., 5.]);
    until(&mut play, &mut app, "case-matched placement write", |p, _| {
        !game.calls("edit.batch").is_empty() && p.edits.sent.is_none()
    });
}

#[test]
fn a_grown_tag_block_stays_under_reload() {
    use crate::app::tag_tests::{WEAP, weap_id};
    let (_t, game, mut app, mut play) = tag_pair("grow-tag");
    play.apply_edits(&mut app, Apply::Send).unwrap();
    app.session.set_allow_growth(true);
    app.tags.allow_growth = true;
    let field = app.tags.layout(weap_id()).unwrap().field_at(8).unwrap();
    let tags::registry::Kind::Block(layout) = &field.kind else {
        panic!()
    };
    app.dispatch(Action::Tag(crate::app::TagAction::Block {
        tag: weap_id(),
        pair: WEAP + 8,
        layout: layout.clone(),
        op: crate::app::BlockOp::Add,
    }));
    play.push_edits(&mut app);
    assert!(app.pairing.pending.iter().any(|r| r["class"] == "Reload"));
    assert!(play.edits.sent.is_none());
    assert!(game.calls("edit.batch").is_empty());
}

#[test]
fn rejoining_lists_the_kept_journal_and_restore_waits_for_revert() {
    let (_t, game, mut app, mut play) = tag_pair("rejoin-tags");
    speed(&mut app);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    until(&mut play, &mut app, "tag reached game", |p, _| {
        !game.calls("edit.batch").is_empty() && p.edits.sent.is_none()
    });
    play.request(Request::Unpair);
    play.update(&mut app);
    assert!(app.pairing.leaving);
    play.apply_edits(&mut app, Apply::Keep).unwrap();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "journal", |_, a| {
        !a.pairing.journal.is_empty()
    });
    assert!(play.apply_edits(&mut app, Apply::Send).is_err());
    play.apply_edits(&mut app, Apply::Adopt).unwrap();
    play.push_edits(&mut app);
    assert!(app.pairing.pending.is_empty());
    play.apply_edits(&mut app, Apply::Restore).unwrap();
    until(&mut play, &mut app, "restored and unpaired", |p, _| {
        p.conn == Conn::Off
    });
    assert_eq!(game.calls("edit.revert").len(), 1);
}

// ---- finding games ----

/// A finder that looks only at these sockets (the real one reads the
/// user's agents directory).
pub(super) fn finder_for(paths: &[&Game]) -> pair::Finder {
    let paths: Vec<PathBuf> = paths.iter().map(|g| g.path.clone()).collect();
    pair::Finder::with_scan(std::sync::Arc::new(move || {
        agent_proto::discover::scan_paths(&paths, Duration::from_secs(5), false)
    }))
}

#[test]
fn a_running_game_is_offered_with_its_map_and_pairs_on_request() {
    let t = Temp::new("offer");
    let game = Game::start("offer", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the game to be found", |p, a| p.notice(a).is_some());
    let n = play.notice(&app).unwrap();
    assert_eq!(n.matches, MapMatch::Same);
    assert!(n.text.contains("example"), "{}", n.text);
    assert_eq!(n.socket, Some(game.path.clone()));
    assert_eq!(play.link_view(&app).text, "Halo 2 is running example");
    // Auto-pair is off: it waits for the button.
    assert_eq!(play.conn, Conn::Off);
    assert_eq!(play.auto_target(false, &app), None);
    play.request(Request::Pair(Endpoint::Unix(game.path.clone())));
    until(&mut play, &mut app, "the pairing", |p, _| {
        p.is_paired() && p.edits.journal_ready
    });
    assert!(play.notice(&app).is_none(), "paired: nothing to offer");
    assert_eq!(play.map_match(&app), MapMatch::Same);
    assert!(matches!(
        play.link_view(&app).state,
        LinkState::Viewing { .. }
    ));
}

#[test]
fn a_game_on_another_map_is_offered_and_says_so() {
    let t = Temp::new("offer-other");
    let game = Game::start("offer-other", "zanzibar", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the game to be found", |p, a| p.notice(a).is_some());
    let n = play.notice(&app).unwrap();
    assert_eq!(n.matches, MapMatch::Different);
    assert!(n.text.contains("zanzibar") && n.text.contains("example"), "{}", n.text);
    play.request(Request::Pair(Endpoint::Unix(game.path.clone())));
    until(&mut play, &mut app, "the pairing", |p, _| {
        p.is_paired() && p.edits.journal_ready
    });
    assert_eq!(
        play.link_view(&app).state,
        LinkState::OtherMap {
            game: "zanzibar".into(),
            open: "example".into(),
        }
    );
    play.request(Request::Edit(true));
    play.update(&mut app);
    assert!(play.edits_blocked(&app).unwrap().contains("zanzibar"));
}

#[test]
fn the_one_running_game_is_paired_on_its_own_once() {
    let t = Temp::new("auto");
    let game = Game::start("auto", "example", SessionKind::Local);
    let mut app = editor(&t);
    assert!(app.pairing.auto_pair, "on by default");
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the automatic pairing", |p, _| {
        p.is_paired() && p.edits.journal_ready
    });
    assert!(!play.live_edits, "auto-pair starts in View mode");
    // After Unpair it stays unpaired, however often it looks.
    play.unpair(&mut app);
    for _ in 0..3 {
        play.finder.look_again();
        let end = Instant::now() + Duration::from_millis(300);
        while Instant::now() < end {
            play.update(&mut app);
            thread::sleep(Duration::from_millis(5));
        }
    }
    assert_eq!(play.conn, Conn::Off);
    assert_eq!(play.auto_target(true, &app), None);
    assert!(play.notice(&app).is_some(), "still offered by hand");
}

#[test]
fn several_games_are_not_paired_for_the_user() {
    let t = Temp::new("many");
    let a = Game::start("many-a", "example", SessionKind::Local);
    let b = Game::start("many-b", "example", SessionKind::Local);
    let mut app = editor(&t);
    let mut play = Play::new();
    play.finder = finder_for(&[&a, &b]);
    until(&mut play, &mut app, "both to be found", |p, _| p.finder.running().len() == 2);
    assert_eq!(play.auto_target(true, &app), None);
    let n = play.notice(&app).unwrap();
    assert_eq!(n.socket, None);
    assert!(n.text.starts_with("2 "), "{}", n.text);
    assert_eq!(play.conn, Conn::Off);
}

#[test]
fn a_game_that_is_gone_is_no_longer_offered() {
    let t = Temp::new("gone");
    let game = Game::start("gone", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the game to be found", |p, a| p.notice(a).is_some());
    drop(game);
    let end = Instant::now() + Duration::from_secs(25);
    while play.notice(&app).is_some() {
        assert!(Instant::now() < end, "the game was still offered");
        play.finder.look_again();
        play.update(&mut app);
        thread::sleep(Duration::from_millis(5));
    }
}

#[test]
fn the_agent_api_lists_pairs_and_unpairs() {
    let t = Temp::new("api");
    let game = Game::start("api", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut api = crate::api::Api::new();
    // No window yet: nothing is published.
    assert!(api.call(&mut app, "pair.status", Value::Null).is_err());
    assert!(api.call(&mut app, "pair.connect", json!({"socket": "/x"})).is_err());
    assert_eq!(
        api.call(&mut app, "pair.auto", json!({"enabled": false})).unwrap(),
        json!({"auto_pair": false})
    );
    let mut play = Play::new();
    play.update(&mut app);
    let s = api.call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!((s["state"].as_str(), s["open_map"].as_str()), (Some("off"), Some("example")));
    assert_eq!(s["link"]["state"], "no_game");
    let socket = game.path.display().to_string();
    let r = api
        .call(&mut app, "pair.connect", json!({"socket": socket}))
        .unwrap();
    assert_eq!(r["requested"], socket);
    until(&mut play, &mut app, "the pairing", |p, _| {
        p.is_paired() && p.edits.journal_ready
    });
    let s = api.call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!(s["state"], "connected");
    assert_eq!(s["map"], "example");
    assert_eq!(s["matches"], "same");
    assert_eq!(s["socket"], socket);
    assert_eq!(s["link"]["state"], "viewing");
    assert_eq!(s["live_edits"], false);
    api.call(&mut app, "pair.edit", json!({"enabled": true}))
        .unwrap();
    until(&mut play, &mut app, "live edits", |p, a| {
        p.edits_blocked(a).is_none()
    });
    let s = api.call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!(s["link"]["state"], "editing");
    assert_eq!(s["link"]["text"], "Editing example live");
    assert!(s["link"]["why"].is_null());
    assert!(
        s["link"]["unavailable"]
            .as_array()
            .unwrap()
            .iter()
            .any(|reason| reason.as_str().unwrap().contains("world.object.respawn"))
    );

    let control = studio_proto::method::PAIR_CONTROL;
    let refused = api
        .call(&mut app, control, json!({"action": "step"}))
        .unwrap_err();
    assert_eq!(refused.code, studio_proto::code::REFUSED);
    api.call(&mut app, control, json!({"action": "pause"}))
        .unwrap();
    until(&mut play, &mut app, "the API pause", |p, _| p.paused);
    api.call(&mut app, control, json!({"action": "step"}))
        .unwrap();
    until(&mut play, &mut app, "the API step", |_, a| {
        a.log().iter().any(|l| l.status.text() == "game: step")
    });
    api.call(&mut app, control, json!({"action": "resume"}))
        .unwrap();
    until(&mut play, &mut app, "the API resume", |p, _| !p.paused);

    api.call(&mut app, "pair.disconnect", json!({"leave":"keep"}))
        .unwrap();
    until(&mut play, &mut app, "the unpairing", |p, _| {
        p.conn == Conn::Off
    });
    let s = api.call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!(s["state"], "off");
}

#[test]
fn the_link_view_names_every_state() {
    let t = Temp::new("link-view");
    let game = Game::start("link-local", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    fn check(play: &Play, app: &App, state: &str, tone: LinkTone) -> LinkView {
        let view = play.link_view(app);
        assert_eq!(view.state.name(), state, "{view:?}");
        assert_eq!(view.tone, tone, "{view:?}");
        assert!(!view.text.is_empty());
        view
    }

    let idle = check(&play, &app, "no_game", LinkTone::Idle);
    assert!(idle.can_play && !idle.can_stop && !idle.paired && !idle.paused);
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "the game to be found", |p, a| {
        p.notice(a).is_some()
    });
    assert_eq!(
        check(&play, &app, "found", LinkTone::Notice).state,
        LinkState::Found {
            map: Some("example".into()),
            matches: MapMatch::Same
        }
    );
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    check(&play, &app, "connecting", LinkTone::Notice);
    until(&mut play, &mut app, "the paired game", |p, _| {
        p.is_paired() && p.edits.journal_ready
    });
    assert!(check(&play, &app, "viewing", LinkTone::View).controls.pause.is_none());
    assert!(!play.live_edits);

    play.request(Request::Edit(true));
    until(&mut play, &mut app, "the writable session", |p, a| {
        p.edits_blocked(a).is_none()
    });
    check(&play, &app, "editing", LinkTone::Live);

    let other = Game::start("link-other", "other", SessionKind::Local);
    play.pair(&mut app, Endpoint::Unix(other.path.clone()));
    until(&mut play, &mut app, "the other map", |p, _| {
        p.map() == Some("other")
    });
    check(&play, &app, "other_map", LinkTone::Warn);
    assert!(!play.live_edits, "a new pairing starts in View mode");

    let coop = Game::start("link-coop", "example", SessionKind::CoopHost);
    let endpoint = Endpoint::Unix(coop.path.clone());
    play.pair(&mut app, endpoint.clone());
    until(&mut play, &mut app, "the co-op session", |p, _| {
        p.session == Some(SessionKind::CoopHost) && p.scenario_tag.is_some()
    });
    play.request(Request::Edit(true));
    play.update(&mut app);
    check(&play, &app, "blocked", LinkTone::Warn);
    play.finder = finder_for(&[&coop]);
    until(&mut play, &mut app, "listed game", |p, _| !p.finder.running().is_empty());
    play.on_event(&mut app, Event::Lost("socket closed".into()));
    check(&play, &app, "reconnecting", LinkTone::Notice);
    play.publish(&mut app);
    let status = crate::api::Api::new().call(&mut app, "pair.status", Value::Null).unwrap();
    assert_eq!(status["state"], "reconnecting");
    assert_eq!(status["link"]["state"], "reconnecting");
    assert_eq!(status["socket"], endpoint.to_string());
    play.reconnect(&mut app, Instant::now() + Duration::from_secs(30));
    until(&mut play, &mut app, "the loss", |p, _| {
        matches!(p.conn, Conn::Lost(_))
    });
    assert!(check(&play, &app, "lost", LinkTone::Error).why.is_some());
    assert_eq!(play.retry_endpoint(), Some(&endpoint));
}

#[test]
fn blocked_names_the_reason() {
    let (_t, _game, app, play) = paired("blocked-reason", "example", SessionKind::CoopGuest);
    let why = SessionKind::CoopGuest.writes().unwrap_err();
    let view = play.link_view(&app);
    assert_eq!(view.state, LinkState::Blocked { why: why.into() });
    assert_eq!(view.why.as_deref(), Some(why));
    assert_eq!(view.text, "Edits stay here: co-op session");
}

#[test]
fn reconnect_delays_double_then_stop_at_thirty_seconds() {
    let start = Instant::now();
    let mut clock = auto::Backoff::new(start);
    let mut at = start;
    for ms in [250, 500, 1000, 2000, 2000, 2000] {
        at += Duration::from_millis(ms);
        assert!(!clock.due(at - Duration::from_millis(1)));
        assert!(clock.due(at));
    }
    let end = start + Duration::from_secs(30);
    assert!(clock.expired(end));
    assert!(!clock.due(end));
}

#[test]
fn discovery_prefers_the_child_then_the_unique_matching_map() {
    let (_t, game, mut app, mut play) = paired("choose", "example", SessionKind::Local);
    let found = agent_proto::discover::Found {
        path: game.path.clone(),
        pid: None,
        status: play.status.clone(),
        note: None,
    };
    let mut other = found.clone();
    other.path = "other.sock".into();
    other.status.as_mut().unwrap().at.map = Some("other".into());
    let same_map = |g: &agent_proto::discover::Found| g.map() == Some("example");
    assert_eq!(
        auto::choose(&[&found, &other], same_map, None, |_| false),
        Some(found.path.clone())
    );
    assert_eq!(
        auto::choose(&[&found, &other], same_map, Some(&other.endpoint()), |_| false),
        Some(other.path)
    );
    assert_eq!(
        auto::choose(&[&found], |_| false, None, |_| false),
        Some(found.path.clone())
    );
    play.unpair(&mut app);
    play.finder.games = vec![found.clone()];
    assert_eq!(play.auto_target(true, &app), None);
    play.finder.look_again();
    until(&mut play, &mut app, "scan without the socket", |p, _| {
        p.finder.games.is_empty()
    });
    play.finder.games = vec![found.clone()];
    assert_eq!(play.auto_target(true, &app), None);
    play.request(Request::Pair(found.endpoint()));
    until(&mut play, &mut app, "pair by hand", |p, _| p.is_paired());
    assert!(!play.unpaired.contains(&found.path, None));
    play.finder.look_again();
    until(&mut play, &mut app, "forget the pairing attempt", |p, _| {
        !p.auto_tried.contains(&found.path)
    });
    play.disconnect(&mut app);
    play.finder.games = vec![found.clone()];
    assert_eq!(play.auto_target(true, &app), Some(found.path));
}

#[test]
fn reconnect_keeps_acknowledged_edits_only_for_the_same_identity() {
    let (_t, game, mut app, mut play) = paired("reconnect", "example", SessionKind::Local);
    play.finder = finder_for(&[&game]);
    play.apply_edits(&mut app, Apply::Send).unwrap();
    move_flag(&mut app, [4., 3., 2.]);
    until(&mut play, &mut app, "acknowledged edit", |p, a| {
        !p.finder.running().is_empty() && !game.calls("edit.batch").is_empty()
            && p.edits.sent.is_none() && a.pairing.pending.is_empty()
    });
    let baseline = play.baseline.clone();
    let epoch = play.edits.epoch;
    for changed in [false, true] {
        play.on_event(&mut app, Event::Lost("dropped".into()));
        assert!(matches!(play.link_view(&app).state, LinkState::Reconnecting { .. }));
        if changed {
            game.bsp.fetch_add(1, Ordering::Relaxed);
        }
        play.reconnect(&mut app, Instant::now() + Duration::from_secs(1));
        until(&mut play, &mut app, "verified reconnect", |p, _| {
            p.is_paired() && p.edits.journal_ready && (p.edits.epoch != epoch) == changed
        });
        play.push_edits(&mut app);
        assert_eq!(play.live_edits, !changed);
        assert_eq!(play.baseline == baseline, !changed);
        assert_eq!(app.pairing.pending.is_empty(), !changed);
        assert_eq!(game.calls("edit.batch").len(), 1);
    }
}

#[test]
fn playtest_decides_each_link_and_reload_route() {
    use playtest::{Ask, Reload, Step, decide};
    let view = LinkState::Viewing { map: Some("fixture".into()), phase: None, tick: None };
    let found = LinkState::Found { map: None, matches: MapMatch::Unknown };
    let other = LinkState::OtherMap { game: "other".into(), open: "fixture".into() };
    let reload = [json!({"class": "Reload"})];
    let poke = [json!({"class": "Poke"})];
    assert_eq!(decide(&LinkState::NoGame, None, &poke, false, false, false), Step::Launch);
    assert_eq!(decide(&found, None, &[], false, false, false), Step::Pair);
    assert_eq!(decide(&view, None, &[], false, false, false), Step::Resume);
    assert_eq!(decide(&view, None, &poke, false, true, false), Step::Resume);
    assert_eq!(decide(&view, None, &poke, false, false, true), Step::Ask { ask: Ask::Send });
    assert_eq!(decide(&view, None, &reload, true, false, true), Step::Reload(Reload::Method));
    assert_eq!(decide(&other, None, &[], true, false, false), Step::Reload(Reload::Relaunch));
    assert_eq!(decide(&other, None, &[], false, false, true), Step::Ask { ask: Ask::Load });
    assert_eq!(decide(&other, None, &[], false, true, true), Step::Reload(Reload::Method));
    assert_eq!(decide(&view, None, &reload, false, true, false), Step::Reload(Reload::Manual));
}

#[test]
fn playtest_asks_then_sends_one_batch_resumes_and_focuses() {
    let (_t, game, mut app, mut play) = tag_pair("playtest-send");
    app.session.select(Some((BlockKind::NetgameFlags, 0)));
    app.session.camera_mut().yaw = 1.2;
    speed(&mut app);
    let selection = app.session.selected();
    let camera = *app.session.camera();
    let undo = app.tags.undo_len();
    assert!(undo > 0);
    play.window_focus(&app, true);
    assert!(!play.controller);
    app.pairing.pause_while_editing = true;
    play.window_focus(&app, false);
    assert!(!play.controller);
    play.window_focus(&app, true);
    until(&mut play, &mut app, "pause on focus and pending tag", |p, a| {
        p.paused && !a.pairing.pending.is_empty()
    });
    play.request(Request::Play);
    play.update(&mut app);
    assert_eq!(app.pairing.prompt.as_ref().unwrap().ask, playtest::Ask::Send);
    assert!(game.calls("edit.batch").is_empty());
    assert!(game.calls("editor.focus").is_empty());
    let mut api = crate::api::Api::new();
    let r = api.call(&mut app, "pair.playtest", json!({"confirm": "send"})).unwrap();
    assert_eq!(r["step"], "resume");
    until(&mut play, &mut app, "sent, resumed and focused", |p, a| {
        !p.paused && a.pairing.pending.is_empty() && !game.calls("editor.focus").is_empty()
    });
    assert_eq!(game.calls("edit.batch").len(), 1);
    assert_eq!(game.calls("editor.focus").len(), 1);
    assert!(!game.agent.is_paused());
    assert_eq!(app.session.selected(), selection);
    assert_eq!(*app.session.camera(), camera);
    assert_eq!(app.tags.undo_len(), undo);
    assert!(!play.live_edits);

    move_flag(&mut app, [6., 5., 4.]);
    play.push_edits(&mut app);
    assert!(play.edits.sent.is_none());
    assert!(!app.pairing.pending.is_empty());
    play.playtest(&mut app, None).unwrap();
    assert!(app.pairing.prompt.is_none());
    until(&mut play, &mut app, "second F5 sends later edits", |p, a| {
        p.waiting == Waiting::Idle && a.pairing.pending.is_empty()
    });
    assert_eq!(game.calls("edit.batch").len(), 2);
    assert!(!play.live_edits);
}

#[test]
fn playtest_reloads_a_fresh_copy_and_restores_the_streamed_place() {
    let t = Temp::new("playtest-reload");
    let game = Game::start("playtest-reload", "example", SessionKind::Local);
    let file = t.0.join("example.map");
    std::fs::write(&file, scenario_edit::fixture::fixture_at_end()).unwrap();
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(file));
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "local player's place", |p, a| {
        p.edits.journal_ready && p.spawn_choice(a) == Spawn::Resume
    });
    app.session.set_allow_growth(true);
    let kind = scenario_edit::layout::BLOCKS.iter().map(|b| b.kind)
        .find(|k| app.session.add_refusal(*k).is_none()).unwrap();
    app.session.add_entry(kind, None).unwrap();
    play.playtest(&mut app, None).unwrap();
    let prompt = app.pairing.prompt.clone().unwrap();
    assert_eq!(prompt.ask, playtest::Ask::Send);
    assert_eq!(prompt.confirmed, playtest::Step::Reload(playtest::Reload::Method));
    play.playtest(&mut app, Some((prompt, "send".into()))).unwrap();
    assert!(!play.live_edits);
    until(&mut play, &mut app, "restored place", |_, _| !game.calls("editor.focus").is_empty());
    assert!(game.edits.lock().unwrap().teleport_map_ticks >= SPAWN_AFTER_TICKS);
    let moves = game.calls("sim.state.set");
    assert_eq!(moves, [json!({"player":"player.0", "pos":PLAYER_AT, "yaw":90.0, "pitch":0.0})]);
    assert!(app.pairing.pending.is_empty());
    assert!(!play.live_edits);
    move_flag(&mut app, [6., 3., 2.]);
    play.push_edits(&mut app);
    assert!(play.edits.sent.is_none());
    assert!(!app.pairing.pending.is_empty());
}

#[test]
fn foreign_map_confirmation_is_scoped_to_the_pairing() {
    let (_t, game, mut app, mut play) = paired("ask-load", "other", SessionKind::Local);
    play.request(Request::Play);
    play.update(&mut app);
    assert!(game.calls("map.reload").is_empty());
    let prompt = app.pairing.prompt.clone().unwrap();
    assert_eq!(prompt.ask, playtest::Ask::Load);
    let mut api = crate::api::Api::new();
    let method = studio_proto::method::PAIR_PLAYTEST;
    let first = api.call(&mut app, method, Value::Null).unwrap();
    let first_id = first["prompt"].as_u64().unwrap();
    assert_eq!(first_id, prompt.id);
    api.call(&mut app, method, json!({"confirm":"cancel", "prompt":first_id})).unwrap();
    play.update(&mut app);
    assert!(app.pairing.prompt.is_none());
    play.unpair(&mut app);
    let second = Game::start("ask-load-second", "other", SessionKind::Local);
    play.pair(&mut app, Endpoint::Unix(second.path.clone()));
    until(&mut play, &mut app, "second pairing on the same foreign map", |p, _| {
        p.edits.journal_ready && p.map() == Some("other")
    });
    let second_ask = api.call(&mut app, method, Value::Null).unwrap();
    let second_id = second_ask["prompt"].as_u64().unwrap();
    assert_ne!(first_id, second_id);
    let status = api.call(&mut app, studio_proto::method::PAIR_STATUS, Value::Null).unwrap();
    assert_eq!(status["prompt"], second_id);
    let current = app.pairing.prompt.clone().unwrap();
    assert_eq!(current.ask, playtest::Ask::Load);
    let stale = api.call(&mut app, method, json!({"confirm":"load", "prompt":first_id}));
    assert_eq!(stale.unwrap_err().code, studio_proto::code::REFUSED);
    assert!(play.playtest(&mut app, Some((prompt, "load".into()))).is_err());
    assert_eq!(app.pairing.prompt.as_ref(), Some(&current));
    assert!(second.calls("map.reload").is_empty());

    api.call(&mut app, method, json!({"confirm":"load", "prompt":second_id})).unwrap();
    until(&mut play, &mut app, "second game receives its own confirmed load", |_, _| {
        second.calls("map.reload").len() == 1
    });
    assert!(!play.live_edits);
    assert!(game.calls("map.reload").is_empty());
}

#[test]
fn playtest_pairs_then_resumes_without_an_unlisted_focus_call() {
    let t = Temp::new("playtest-pair");
    let game = Game::start("playtest-pair", "example", SessionKind::Local);
    let mut app = editor(&t);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.finder = finder_for(&[&game]);
    until(&mut play, &mut app, "one game offered", |p, a| p.notice(a).is_some());
    let mut api = crate::api::Api::new();
    let step = api.call(&mut app, "pair.playtest", Value::Null).unwrap();
    assert_eq!(step, json!({"step":"pair"}));
    until(&mut play, &mut app, "paired and focused", |_, _| game.calls("editor.focus").len() == 1);
    assert!(!play.live_edits);

    let mut methods = play.methods.clone().unwrap();
    assert!(methods.remove(agent_proto::editor::method::FOCUS));
    play.on_answer(&mut app, Purpose::Capabilities, method::METHODS, Ok(json!(methods)));
    play.pause();
    until(&mut play, &mut app, "pause", |p, _| p.paused);
    api.call(&mut app, "pair.play", Value::Null).unwrap();
    until(&mut play, &mut app, "resume", |p, _| !p.paused);
    assert_eq!(game.calls("editor.focus").len(), 1);
}

#[test]
fn playtest_refuses_a_clean_document_with_a_different_checksum() {
    let (_t, game, mut app, mut play) = paired("f5-checksum", "example", SessionKind::Local);
    game.edits.lock().unwrap().checksum ^= 1;
    until(&mut play, &mut app, "checksum refusal", |p, a| {
        matches!(p.link_view(a).state, LinkState::Blocked { ref why } if why.contains("checksum"))
    });
    assert!(app.pairing.pending.is_empty());
    let mut api = crate::api::Api::new();
    for method in [studio_proto::method::PAIR_PLAYTEST, studio_proto::method::PAIR_PLAY] {
        let err = api.call(&mut app, method, Value::Null).unwrap_err();
        assert_eq!(err.code, studio_proto::code::REFUSED);
        assert!(err.message.contains("checksum"));
    }
    assert!(play.playtest(&mut app, None).unwrap_err().contains("checksum"));
    app.pairing.shown = None;
    play.teleport(&mut app, [1., 2., 3.]);
    assert!(app.status().unwrap().text().contains("checksum"));
    assert!(game.calls(agent_proto::sim::method::STATE_SET).is_empty());
    assert!(game.calls("editor.focus").is_empty());
}

#[test]
fn send_confirmation_covers_the_step_and_both_edit_revisions() {
    let (_t, game, mut app, mut play) = tag_pair("ask-revision");
    move_flag(&mut app, [1., 2., 3.]);
    play.playtest(&mut app, None).unwrap();
    let before_tag = app.pairing.prompt.clone().unwrap();
    play.publish(&mut app);
    speed(&mut app);
    let stale = crate::api::Api::new().call(
        &mut app,
        studio_proto::method::PAIR_PLAYTEST,
        json!({"confirm":"send","prompt":before_tag.id}),
    );
    assert_eq!(stale.unwrap_err().code, studio_proto::code::REFUSED);
    assert!(play.playtest(&mut app, Some((before_tag, "send".into()))).is_err());

    play.playtest(&mut app, None).unwrap();
    let before_placement = app.pairing.prompt.clone().unwrap();
    move_flag(&mut app, [7., 8., 9.]);
    assert!(play.playtest(&mut app, Some((before_placement, "send".into()))).is_err());

    play.playtest(&mut app, None).unwrap();
    let before_layout = app.pairing.prompt.clone().unwrap();
    assert_eq!(before_layout.confirmed, playtest::Step::Resume);
    app.session.set_allow_growth(true);
    app.tags.allow_growth = true;
    let tag = crate::app::tag_tests::weap_id();
    let field = app.tags.layout(tag).unwrap().field_at(8).unwrap();
    let tags::registry::Kind::Block(layout) = &field.kind else {
        panic!("expected an invented block");
    };
    app.dispatch(Action::Tag(crate::app::TagAction::Block {
        tag,
        pair: crate::app::tag_tests::WEAP + 8,
        layout: layout.clone(),
        op: crate::app::BlockOp::Add,
    }));
    assert!(play.playtest(&mut app, Some((before_layout, "send".into()))).is_err());
    play.playtest(&mut app, None).unwrap();
    let reload = app.pairing.prompt.clone().unwrap();
    assert_eq!(reload.confirmed, playtest::Step::Reload(playtest::Reload::Method));

    // The same revisions with a different advertised route still need a fresh answer.
    play.methods.as_mut().unwrap().remove(agent_proto::sim::method::MAP_RELOAD);
    assert!(play.playtest(&mut app, Some((reload, "send".into()))).is_err());
    assert!(game.calls("map.reload").is_empty());
    assert!(game.calls("edit.batch").is_empty());
    assert!(!play.live_edits);
}

#[test]
fn one_playtest_batch_leaves_edits_made_during_its_acknowledgement_pending() {
    let (_t, game, mut app, mut play) = tag_pair("f5-ack");
    speed(&mut app);
    play.playtest(&mut app, None).unwrap();
    let prompt = app.pairing.prompt.clone().unwrap();
    {
        let _boundary = game.boundary.lock().unwrap();
        play.playtest(&mut app, Some((prompt, "send".into()))).unwrap();
        assert!(play.edits.sent.is_some());
        move_flag(&mut app, [7., 6., 5.]);
        play.update(&mut app);
    }
    until(&mut play, &mut app, "one acknowledged playtest batch", |p, _| {
        p.waiting == Waiting::Idle && !game.calls("editor.focus").is_empty()
    });
    play.push_edits(&mut app);
    assert_eq!(game.calls("edit.batch").len(), 1);
    assert!(play.edits.sent.is_none());
    assert!(!app.pairing.pending.is_empty());
    assert!(!play.live_edits);
}

#[test]
fn a_refused_one_shot_batch_keeps_its_edit_mode() {
    use crate::app::tag_tests::WEAP;
    for live in [false, true] {
        let tag = if live { "f5-refused-edit" } else { "f5-refused-view" };
        let (_t, game, mut app, mut play) = tag_pair(tag);
        game.edits.lock().unwrap().bytes.insert(
            WEAP.to_string(),
            sync::hex(&11f32.to_le_bytes()),
        );
        speed(&mut app);
        if live {
            play.apply_edits(&mut app, Apply::Send).unwrap();
        }
        play.playtest(&mut app, None).unwrap();
        if let Some(prompt) = app.pairing.prompt.clone() {
            play.playtest(&mut app, Some((prompt, "send".into()))).unwrap();
        }
        until(&mut play, &mut app, "batch refused", |p, a| {
            p.waiting == Waiting::Idle && a.pairing.pending.iter().any(|r| r["conflict"] == true)
        });
        assert_eq!(play.live_edits, live);
        assert!(!play.playtest_consent);
        assert!(game.calls("editor.focus").is_empty());
    }
}

pub(super) mod object_edits;

#[test]
fn playtest_reloads_for_failed_object_edits_and_resumes_for_receipts() {
    use playtest::{Ask, Reload, Step, decide};
    let view = LinkState::Viewing { map: Some("example".into()), phase: None, tick: None };
    for class in [tag_sync::RELOAD, tag_sync::CREATION] {
        let mut row = json!({"doc":"object", "class":class});
        let step = |row: &Value, consent| decide(&view, None, std::slice::from_ref(row), false, consent, true);
        assert_eq!(step(&row, false), Step::Resume);
        row["refused"] = json!(true);
        assert_eq!(step(&row, false), Step::Ask { ask: Ask::Send });
        assert_eq!(step(&row, true), Step::Reload(Reload::Method));
    }
}

mod conflict_paths;
