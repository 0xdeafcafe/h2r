//! The subject on the invented game (docs/editor/live-editors.md, LE2):
//! where it stands, what a write queues after it, and that an attached
//! game is left alone.
use std::sync::{Arc, Mutex};

use agent::Access;
use agent_proto::Endpoint;
use agent_proto::sim::SessionKind;
use agent_proto::sim::method as sim;
use serde_json::{Value, json};
use tags::registry::Registry;

use super::*;
use crate::app::live::{Change, Class};
use crate::app::{Action, App, TagAction, tag_tests};
use crate::play::Play;
use crate::play::testing::Temp;
use crate::play::testing::fake::{Game, until};

/// The invented weapon layout with the two live-apply words: `speed` is
/// read when the object is made, `mode` when it is used, `flags` has no
/// word.
fn registry() -> &'static Registry {
    static R: std::sync::OnceLock<Registry> = std::sync::OnceLock::new();
    R.get_or_init(|| {
        let schema = tags::schema::Schema::parse(
            "group scnr 0x10 align 4\ngroup weap 0x10 align 4\n  0x8 block 0xc align 4\n    0x0 tagref\n",
        )
        .unwrap();
        let mut defs = tags::defs::Defs::default();
        defs.parse_into(
            "test",
            "\
group weap 0x10
  0x0 f32 speed applies=creation -- invented reader
  0x4 flags16 flags bits=loud|quiet
  0x6 enum16 mode values=off|on|auto applies=use -- invented reader
  0x8 block parts 0xc align 4
    0x0 tagref item groups=itmc
    0x8 u32 value
",
        )
        .unwrap();
        Registry::build(&schema, &defs)
    })
}

type Calls = Arc<Mutex<Vec<(String, Value)>>>;

fn calls(log: &Calls, method: &str) -> Vec<Value> {
    log.lock()
        .unwrap()
        .iter()
        .filter(|(m, _)| m == method)
        .map(|(_, v)| v.clone())
        .collect()
}

/// The invented game answering the subject's methods, and the editor
/// paired with it over the weapon fixture; `own` makes it Studio's own.
fn paired(name: &str, own: bool) -> (Temp, Game, App, Play, Calls) {
    let t = Temp::new(name);
    let game = Game::start(name, "example", SessionKind::Local);
    let log: Calls = Calls::default();
    for (method, reply) in [
        (sim::SPAWN, json!({"object": "0000abcd", "pos": [0.0, 0.0, 0.0]})),
        (
            sim::OBJECT_RESPAWN,
            json!({"old": "weapon.0000abcd", "object": "weapon.0000abce", "linked": false}),
        ),
        (sim::OBJECT_DELETE, json!({"object": "weapon.0000abce"})),
        (sim::AUDIO_PLAY, json!({})),
    ] {
        let log = Arc::clone(&log);
        game.agent.add_method(method, Access::Act, move |params| {
            log.lock().unwrap().push((method.to_owned(), params));
            Ok(reply.clone())
        });
    }
    let (mut app, _, _) = tag_tests::app_with_registry(&t.0, tag_tests::map(), registry());
    app.pairing.auto_pair = false;
    let mut play = Play::new();
    play.pair(&mut app, Endpoint::Unix(game.path.clone()));
    until(&mut play, &mut app, "game identity and journal", |p, a| {
        p.edits.journal_ready && p.edit_refusal(a).is_none()
    });
    if own {
        play.child_endpoint = play.endpoint().cloned();
    }
    (t, game, app, play, log)
}

fn select(app: &mut App, tag: map::TagId) {
    app.dispatch(Action::Tag(TagAction::Select(tag)));
}

fn set(app: &mut App, path: &str, value: Value) {
    let action = crate::app::tag_api::set_field(&app.tags, tag_tests::weap_id(), path, &value).unwrap();
    app.dispatch(Action::Tag(action));
}

fn frames(play: &mut Play, app: &mut App, n: usize) {
    for _ in 0..n {
        play.update(app);
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
}

/// With the weapon the subject, Studio's own game gets one `sim.spawn` of
/// it: where the level's scene says, by the weapon's datum; the state is
/// published for `tag.subject`.
#[test]
fn the_subject_stands_in_the_own_game() {
    let (_t, _game, mut app, mut play, log) = paired("subject-stands", true);
    select(&mut app, tag_tests::weap_id());
    until(&mut play, &mut app, "the spawn", |_, _| calls(&log, sim::SPAWN).len() == 1);
    until(&mut play, &mut app, "its answer", |_, a| {
        a.pairing.subject["object"] == "weapon.0000abcd"
    });
    frames(&mut play, &mut app, 20);
    let spawn = calls(&log, sim::SPAWN);
    assert_eq!(spawn.len(), 1, "one spawn, not one a frame");
    assert_eq!(spawn[0]["tag"], format!("{:#010x}", tag_tests::weap_id().0));
    assert_eq!(spawn[0]["pos"].as_array().unwrap().len(), 3);
    assert_eq!(app.pairing.subject["place"]["scene"], "level");
    assert_eq!(app.pairing.subject["place"]["show"], "object");
    assert_eq!(app.pairing.subject["calls"]["spawn"], 1);
}

/// A field read when the object is made queues one respawn of the
/// subject's object after the write's batch, and nothing else is made or
/// removed; the new object is the one that stands.
#[test]
fn a_next_spawn_write_queues_one_respawn_of_the_subject() {
    let (_t, game, mut app, mut play, log) = paired("subject-respawn", true);
    select(&mut app, tag_tests::weap_id());
    until(&mut play, &mut app, "the standing object", |_, a| {
        a.pairing.subject["object"] == "weapon.0000abcd"
    });
    set(&mut app, "speed", json!(4.5));
    until(&mut play, &mut app, "the respawn", |_, _| {
        calls(&log, sim::OBJECT_RESPAWN).len() == 1
    });
    frames(&mut play, &mut app, 20);
    assert_eq!(game.calls("edit.batch").len(), 1);
    let respawn = calls(&log, sim::OBJECT_RESPAWN);
    assert_eq!(respawn.len(), 1);
    assert_eq!(respawn[0]["id"], "weapon.0000abcd");
    assert_eq!(calls(&log, sim::SPAWN).len(), 1);
    assert!(calls(&log, sim::OBJECT_DELETE).is_empty());
    assert!(calls(&log, sim::AUDIO_PLAY).is_empty());
    assert_eq!(app.pairing.subject["object"], "weapon.0000abce");
    assert_eq!(app.pairing.subject["owed"], json!({"respawn": false, "retrigger": false}));
}

/// A field read at use queues a retrigger; an object has nothing to
/// trigger again, so no call is made and the subject says so. It makes
/// no respawn.
#[test]
fn a_next_use_write_retriggers_only_what_can_be_triggered() {
    let (_t, _game, mut app, mut play, log) = paired("subject-use", true);
    select(&mut app, tag_tests::weap_id());
    until(&mut play, &mut app, "the standing object", |_, a| {
        a.pairing.subject["object"] == "weapon.0000abcd"
    });
    set(&mut app, "mode", json!("on"));
    until(&mut play, &mut app, "the subject's note", |_, a| {
        !a.pairing.subject["failure"].is_null()
    });
    frames(&mut play, &mut app, 20);
    assert_eq!(app.pairing.subject["failure"], NO_RETRIGGER);
    assert!(calls(&log, sim::OBJECT_RESPAWN).is_empty());
    assert!(calls(&log, sim::AUDIO_PLAY).is_empty());
}

/// A write to a tag that is not the subject queues nothing, whatever it
/// is read as; a write to the subject's tag marks a field no definition
/// gives a word as unread.
#[test]
fn only_the_subjects_own_writes_queue_follow_ups() {
    let (_t, _game, mut app, mut play, _log) = paired("subject-only", true);
    select(&mut app, tag_tests::weap_id());
    frames(&mut play, &mut app, 5);
    let weap = tag_tests::weap_id();
    let other = scenario_edit::fixture::id(3);
    let at = tag_tests::WEAP;
    // The same bytes, written to another tag.
    play.subject_follow(&app, &[Change::new(Class::Field, Some(other), vec![(at, 4)], "x")]);
    assert_eq!(play.subject.owed, Owed::default());
    // The subject's `speed`, `mode` and `flags`.
    let change = Change::new(Class::Field, Some(weap), vec![(at, 4), (at + 6, 2), (at + 4, 2)], "x");
    let named: Vec<(Effect, String)> = change_effects(&app.tags, &change);
    assert_eq!(
        named,
        vec![
            (Effect::NextSpawn, "speed".to_owned()),
            (Effect::NextUse, "mode".to_owned()),
            (Effect::NotKnown, "flags".to_owned()),
        ]
    );
    play.subject_follow(&app, &[change]);
    assert_eq!(play.subject.owed, Owed { respawn: true, retrigger: true });
    assert_eq!(play.subject.unread.iter().collect::<Vec<_>>(), vec!["flags"]);
}

/// Another subject replaces the standing object: the old one is deleted
/// and nothing is made for a tag the registry puts nowhere.
#[test]
fn a_new_subject_removes_the_old_object() {
    let (_t, _game, mut app, mut play, log) = paired("subject-replaced", true);
    select(&mut app, tag_tests::weap_id());
    until(&mut play, &mut app, "the standing object", |_, a| {
        a.pairing.subject["object"] == "weapon.0000abcd"
    });
    // An `itmc`: the registry has no row for it, so it has no object.
    select(&mut app, scenario_edit::fixture::id(3));
    until(&mut play, &mut app, "the delete", |_, _| {
        calls(&log, sim::OBJECT_DELETE).len() == 1
    });
    frames(&mut play, &mut app, 20);
    assert_eq!(calls(&log, sim::OBJECT_DELETE)[0]["id"], "weapon.0000abcd");
    assert_eq!(calls(&log, sim::SPAWN).len(), 1);
    assert!(app.pairing.subject["object"].is_null());
}

/// An attached game gets neither the subject nor its follow-ups: those
/// stay Pending rows.
#[test]
fn an_attached_game_is_left_alone() {
    let (_t, game, mut app, mut play, log) = paired("subject-attached", false);
    select(&mut app, tag_tests::weap_id());
    set(&mut app, "speed", json!(4.5));
    frames(&mut play, &mut app, 30);
    for method in [sim::SPAWN, sim::OBJECT_RESPAWN, sim::OBJECT_DELETE, sim::AUDIO_PLAY] {
        assert!(calls(&log, method).is_empty(), "{method}");
    }
    assert!(game.calls("edit.batch").is_empty());
    assert!(app.pairing.subject.is_null());
    assert_eq!(app.pairing.scene, Scene::Nowhere);
}
