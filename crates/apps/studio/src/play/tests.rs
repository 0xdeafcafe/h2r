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
use super::testing::fake::{Game, SCNR, UNIT, VEHICLE, until};
use super::*;
use crate::app::{Action, App};

/// The editor with the invented map open as `example`.
fn editor(t: &Temp) -> App {
    let mut app = App::new();
    app.set_data_root(t.0.join("data"));
    app.dispatch(Action::OpenSource(t.map("example")));
    assert_eq!(app.loaded_map(), Some("example"));
    app
}

fn paired(tag: &str, map: &str, kind: SessionKind) -> (Temp, Game, App, Play) {
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

fn move_flag(app: &mut App, to: [f32; 3]) {
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
fn teleport_and_spawn_move_the_player() {
    let (_t, game, mut app, mut play) = paired("tp", "example", SessionKind::MpHost);
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
    play.pending_spawn = Some([1.0, 2.0, 3.0]);
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
    play.pending_spawn = Some([1.0, 2.0, 3.0]);
    let end = Instant::now() + Duration::from_millis(600);
    while Instant::now() < end {
        play.update(&mut app);
        thread::sleep(Duration::from_millis(5));
    }
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
    for _ in 0..50 {
        play.update(&mut app);
        thread::sleep(Duration::from_millis(5));
    }
    assert!(game.calls("edit.batch").is_empty());
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
fn a_game_that_goes_away_is_lost() {
    let (_t, game, mut app, mut play) = paired("lost", "example", SessionKind::Local);
    drop(game);
    until(&mut play, &mut app, "the loss", |p, _| {
        matches!(p.conn, Conn::Lost(_))
    });
    assert_eq!(play.link_view(&app).text, "Game lost");
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
    let mut app = editor(&t);
    move_flag(&mut app, [5.0, 6.0, 7.0]);
    let mut play = Play::new();
    play.h2_path = "/opt/h2".into();
    // The source is a loose map: the disc comes from H2_DISC or the
    // repository's disc/; without either, Play says so.
    match play.prepare(&mut app) {
        Ok(l) => {
            assert_eq!(l.level, "example");
            assert_eq!(l.h2, std::path::PathBuf::from("/opt/h2"));
            let dir = l.map_dir.unwrap();
            let written = std::fs::read(dir.join("example.map")).unwrap();
            let mut s = scenario_edit::Session::new();
            s.open(scenario_edit::MapSource::file(dir.join("example.map")))
                .unwrap();
            let p = s
                .scenario()
                .unwrap()
                .get(BlockKind::NetgameFlags, 0, "position")
                .unwrap()
                .as_vec3()
                .unwrap();
            assert_eq!(p, [5.0, 6.0, 7.0]);
            assert!(!written.is_empty());
            assert!(app.is_dirty(), "writing the play copy is not a save");
            let _ = std::fs::remove_dir_all(launch::scratch());
        }
        Err(e) => assert!(e.contains("disc"), "{e}"),
    }
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

fn tag_pair(name: &str) -> (Temp, Game, App, Play) {
    let t = Temp::new(name);
    let game = Game::start(name, "example", SessionKind::Local);
    let (mut app, _, _) = crate::app::tag_tests::app(&t.0);
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "game identity and journal", |p, _| {
        p.edits.journal_ready
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
        assert_eq!(sync::hex(&app.tags.read(WEAP, 4).unwrap()), expected);
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
    assert_eq!(app.tags.read(WEAP, 4).unwrap(), 4.5f32.to_le_bytes());

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
fn finder_for(paths: &[&Game]) -> pair::Finder {
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
    assert_eq!(play.auto_target(false), None);
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
    let t = Temp::new("other");
    let game = Game::start("other", "zanzibar", SessionKind::Local);
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
    assert_eq!(play.auto_target(true), None);
    assert!(play.notice(&app).is_some(), "still offered by hand");
}

#[test]
fn several_games_are_not_paired_for_the_user() {
    let t = Temp::new("many");
    let a = Game::start("many-a", "example", SessionKind::Local);
    let b = Game::start("many-b", "zanzibar", SessionKind::Local);
    let mut app = editor(&t);
    let mut play = Play::new();
    play.finder = finder_for(&[&a, &b]);
    until(&mut play, &mut app, "both to be found", |p, _| p.finder.running().len() == 2);
    assert_eq!(play.auto_target(true), None);
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
    assert_eq!(
        s["link"],
        json!({"state":"editing", "text":"Editing example live", "why":null})
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
    assert!(idle.can_play && !idle.can_stop && !idle.can_pause && !idle.paused);
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
    assert!(check(&play, &app, "viewing", LinkTone::View).can_pause);
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
    drop(coop);
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
