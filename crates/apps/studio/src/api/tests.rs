//! The agent API over the scenario crate's invented map, the playlist
//! crate's sample file and the fallback variant schema (no game data).
//! Outputs go to the system temp directory, never the checkout.

use std::path::{Path, PathBuf};

use ::variants::save::{CONTENT_TYPES, SaveStore, XbeNames};
use scenario_edit::{BlockKind, fixture};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::*;
use crate::app::{Action, PlaylistEdit, Selection};

struct Temp(PathBuf);
impl Temp {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("h2s-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Temp(dir)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// An app with the invented map open, a variant store, and the sample
/// playlist file at hand (not open).
fn app(t: &Temp) -> App {
    let map = t.path("example.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    std::fs::write(
        t.path("multiplayer_hopper.dat"),
        hopper_edit::fixture::sample().build(),
    )
    .unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let names = XbeNames {
        file_names: (0..CONTENT_TYPES).map(|i| format!("t{i}")).collect(),
        labels: (0..CONTENT_TYPES).map(|i| format!("L{i}")).collect(),
    };
    // The fallback schema (no disc) has no built-in variants: give it one.
    let mut schema = ::variants::Schema::fallback();
    schema.builtins.push(::variants::schema::Builtin {
        name: "Slayer".into(),
        game_type: ::variants::GameType::Slayer,
        description: 0,
        pairs: Vec::new(),
    });
    app.dispatch(Action::OpenSource(map));
    assert!(app.session.is_open());
    // After the source: opening one replaces a variant editor made for
    // another (App::ensure_variants).
    app.set_variant_editor(::variants::VariantEditor::new(
        schema,
        Some(SaveStore::new(t.path("saves"), Some(names))),
    ));
    app
}

fn call(api: &mut Api, app: &mut App, method: &str, params: Value) -> Value {
    match api.call(app, method, params.clone()) {
        Ok(v) => v,
        Err(e) => panic!("{method} {params}: {e}"),
    }
}

fn code_of(r: Reply) -> i64 {
    match r {
        Ok(v) => panic!("expected an error, got {v}"),
        Err(e) => e.code,
    }
}

fn flag_position(app: &App) -> [f32; 3] {
    app.session
        .scenario()
        .unwrap()
        .get(BlockKind::NetgameFlags, 0, "position")
        .unwrap()
        .as_vec3()
        .unwrap()
}

/// The example params of `d` with the test's paths and values in.
fn example(d: &MethodDef, t: &Temp, app: &mut App) -> Value {
    let mut text = d.example.to_owned();
    let out = t.path("out");
    std::fs::create_dir_all(&out).unwrap();
    text = text.replace("$MAP", &t.path("example.map").display().to_string());
    text = text.replace("$OUT", &out.display().to_string());
    if text.contains("$SCREEN") {
        let first = crate::app::UiScreens::tree()[0].0;
        text = text.replace("$SCREEN", first);
    }
    if text.contains("$SETTING") {
        // A setting of the open variant and another value for it.
        app.ensure_variants();
        let e = app.variants.as_ref().unwrap();
        let v = e.current().unwrap();
        // The fallback schema (no disc) knows no choices: any value goes,
        // so flip one between 0 and 1.
        let known: Vec<_> = v
            .known_settings()
            .into_iter()
            .filter(|(id, _)| ::variants::schema::applies(*id, v.game_type()))
            .collect();
        let (id, other) = known
            .iter()
            .find_map(|&(id, now)| {
                let d = e.schema().settings.get(&id)?;
                let other = d.choices.iter().find(|c| c.value != now)?.value;
                Some((id, other))
            })
            .or_else(|| {
                known
                    .iter()
                    .find(|(id, _)| {
                        e.schema()
                            .settings
                            .get(id)
                            .is_some_and(|d| d.choices.is_empty())
                    })
                    .map(|&(id, now)| (id, i32::from(now == 0)))
            })
            .expect("a setting the variant can change");
        text = text
            .replace("\"$SETTING\"", &id.0.to_string())
            .replace("\"$VALUE\"", &other.to_string());
    }
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: example {text}: {e}", d.name))
}

/// What a method's example needs before it runs.
fn setup(api: &mut Api, app: &mut App, t: &Temp, name: &str) {
    let mut c = |m: &str, p: Value| {
        call(api, app, m, p);
    };
    let new_variant = json!({"variant": {"new": "slayer"}, "discard": true});
    match name {
        m::DOC_SAVE => c(
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": t.path("out/first.map").display().to_string()}),
        ),
        m::SCENARIO_ADD | m::SCENARIO_DUPLICATE | m::TAG_BLOCK_ADD | m::TAG_BLOCK_DUPLICATE => {
            // Every block of the invented map is full (the editor does
            // not grow blocks): make room in the starting locations, which
            // each example adds to.
            c(
                m::SCENARIO_DELETE,
                json!({"block": "starting_locations", "index": 1}),
            );
        }
        m::VOLUME_LIST | m::VOLUME_SELECT | m::VOLUME_SET | m::VOLUME_REMOVE | m::VOLUME_ADD => {
            let map = t.path("volumes.map");
            std::fs::write(&map, fixture::fixture_with_volumes()).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
            if name == m::VOLUME_ADD {
                // Both blocks are full: make room for a kill volume.
                c(m::VOLUME_REMOVE, json!({"kind": "kill", "index": 0}));
                c(m::VOLUME_REMOVE, json!({"index": 1}));
            }
        }
        m::SCRIPT_LIST
        | m::SCRIPT_SOURCE
        | m::SCRIPT_FOLLOW
        | m::SCRIPT_USES
        | m::SCRIPT_BUILTINS
        | m::SCRIPT_DOC => {
            // The invented map with scripts and the invented executable.
            let dir = t.path("scripts");
            std::fs::create_dir_all(&dir).unwrap();
            let map = crate::app::scripts::tests::scripted_source(&dir, true);
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
        }
        m::COLLISION_SURFACES | m::COLLISION_AT | m::COLLISION_SELECT | m::COLLISION_OBJECTS => {
            let map = t.path("collision.map");
            let bytes = if name == m::COLLISION_OBJECTS {
                fixture::fixture_with_object_collision()
            } else {
                fixture::fixture_with_instance_collision()
            };
            std::fs::write(&map, bytes).unwrap();
            c(m::SOURCE_OPEN, json!({"path": map.display().to_string()}));
        }
        m::DOC_CLOSE | m::PLAYLIST_LIST | m::PLAYLIST_GET | m::PLAYLIST_SET | m::PLAYLIST_ENTRY => {
            c(
                m::DOC_OPEN,
                json!({"playlists": t.path("multiplayer_hopper.dat").display().to_string()}),
            )
        }
        m::PLAYLIST_ENTRY_VARIANT => {
            c(
                m::DOC_OPEN,
                json!({"playlists": t.path("multiplayer_hopper.dat").display().to_string()}),
            );
            c(m::DOC_OPEN, new_variant);
        }
        m::VARIANT_GET | m::VARIANT_SET | m::VARIANT_RENAME | m::VARIANT_EXPORT => {
            c(m::DOC_OPEN, new_variant)
        }
        m::VARIANT_COPY => c(m::DOC_OPEN, json!({"variant": {"builtin": 0}})),
        m::VARIANT_DELETE => {
            c(m::DOC_OPEN, new_variant);
            c(m::DOC_SAVE, json!({"doc": "variant"}));
        }
        m::VARIANT_IMPORT => {
            c(m::DOC_OPEN, new_variant);
            c(
                m::VARIANT_EXPORT,
                json!({"path": t.path("out/v.h2variant").display().to_string()}),
            );
        }
        m::UI_PRESS | m::UI_WIDGETS | m::UI_RENDER => {
            let first = crate::app::UiScreens::tree()[0].0;
            c(m::UI_PICK, json!({"screen": first}));
        }
        m::DOC_UNDO => c(
            m::SCENARIO_MOVE,
            json!({"block": "netgame_flags", "index": 0, "position": [1, 2, 3]}),
        ),
        m::DOC_REDO => {
            c(
                m::SCENARIO_MOVE,
                json!({"block": "netgame_flags", "index": 0, "position": [1, 2, 3]}),
            );
            c(m::DOC_UNDO, json!({"doc": "map"}));
        }
        // A sibling of the loose map stands in for the disc's shared.map.
        m::TAG_BROWSE => std::fs::write(t.path("shared.map"), fixture::fixture()).unwrap(),
        m::TAG_IMPORT => c(
            m::TAG_EXPORT,
            json!({"tag": "itmc", "out": t.path("out/kit.h2tag").display().to_string()}),
        ),
        // The window's pairing, as its first frame publishes it.
        n if n.starts_with("pair.") => {
            crate::play::Play::new().update(app);
        }
        n if n.starts_with("hud.") => {
            // The HUD fixture's map as the source (it has no scenario).
            let map = t.path("hudtest.map");
            std::fs::write(&map, hud_edit::fixture::build().0).unwrap();
            app.dispatch(Action::OpenSource(map));
            if n != m::HUD_LIST {
                call(api, app, m::HUD_LIST, json!({"map": "hudtest"}));
            }
        }
        _ => {}
    }
}

#[test]
fn ui_palette_lists_and_runs_fixture_maps_tags_and_placements() {
    let t = Temp::new("palette");
    let mut app = app(&t);
    let map = t.path("blaster.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    app.dispatch(Action::OpenSource(map));
    let mut api = Api::new();

    let list = call(&mut api, &mut app, m::UI_PALETTE, json!({"query":"blast"}));
    let entries = list["entries"].as_array().unwrap();
    let kinds: Vec<_> = entries.iter().map(|e| e["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["map", "tag", "placement", "placement"]);
    assert_eq!(list["totals"], json!({"map":1, "tag":1, "placement":2, "command":0}));
    assert_eq!(entries[0]["label"], "blaster");
    assert_eq!(entries[1]["label"], "objects\\test\\blaster");
    assert_eq!(entries[2]["label"], "0: blaster");

    let filtered = call(&mut api, &mut app, m::UI_PALETTE, json!({"query":"WEAP bla"}));
    assert_eq!(filtered["totals"], json!({"map":0, "tag":1, "placement":2, "command":0}));
    let result = call(
        &mut api, &mut app, m::UI_PALETTE, json!({"query":"blast", "run":1}),
    );
    assert_eq!(result["ran"], 1);
    assert_eq!(app.selection(), Some(Selection::Tag(fixture::id(2))));
    call(&mut api, &mut app, m::UI_PALETTE, json!({"query":"blast", "run":3}));
    assert_eq!(
        app.selection(),
        Some(Selection::Placement(BlockKind::Weapons, 1))
    );
    let bad_index = api.call(&mut app, m::UI_PALETTE, json!({"query":"blast", "run":4}));
    assert_eq!(code_of(bad_index), code::INVALID_PARAMS);
    assert_eq!(
        app.selection(),
        Some(Selection::Placement(BlockKind::Weapons, 1))
    );

    let result = call(
        &mut api,
        &mut app,
        m::UI_PALETTE,
        json!({"query":"growing full blocks", "run":0}),
    );
    assert_eq!(result["entries"][0]["kind"], "command");
    assert_eq!(result["ran"], 0);
    assert!(app.session.allow_growth());
}

#[test]
fn doc_save_rewrites_the_open_package() {
    let t = Temp::new("doc-save-package");
    let mut app = app(&t);
    let mut api = Api::new();
    let form = app.session.form_defaults();
    app.dispatch(Action::SavePackage(form));
    call(
        &mut api, &mut app, m::SCENARIO_MOVE,
        json!({"block":"netgame_flags", "index":0, "position":[7, 3, 3]}),
    );
    let path = app.session.maps_dir().join("example-edit");
    let result = call(&mut api, &mut app, m::DOC_SAVE, json!({"doc":"map"}));
    assert_eq!(result["saved"], true);
    assert_eq!(result["path"], path.display().to_string());
    assert!(app.package_form.is_none());
    assert!(!app.is_dirty());
    app.dispatch(Action::SelectMap("example".into()));
    app.dispatch(Action::OpenPackageEntry("example-edit".into()));
    assert_eq!(flag_position(&app), [7.0, 3.0, 3.0]);
}

#[test]
fn tag_fields_describe_root_and_nested_registry_fields() {
    let t = Temp::new("field-rows");
    let mut bytes = fixture::fixture();
    // One collection entry in the fixture's ordinary metadata.
    let root = fixture::META_FILE + 0x1200;
    fixture::put(&mut bytes, root, 1);
    fixture::put(&mut bytes, root + 4, fixture::META + 0x1240);
    fixture::putf(&mut bytes, fixture::META_FILE + 0x1240, 1.5);
    let source = t.path("rows.map");
    std::fs::write(&source, bytes).unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    app.session.build_scenes = false;
    app.dispatch(Action::OpenSource(source));
    let mut api = Api::new();

    let root = call(&mut api, &mut app, m::TAG_FIELDS, json!({"tag":"itmc"}));
    let rows = root["fields"].as_array().unwrap();
    let respawn = rows.iter().find(|f| f["name"] == "respawn_time").unwrap();
    assert_eq!(respawn["offset"], 8);
    assert_eq!(respawn["size"], 2);
    assert_eq!(respawn["kind"], "i16");
    assert_eq!(
        respawn["description"],
        "Seconds before the item respawns when its placement has no override."
    );
    let entries = rows.iter().find(|f| f["name"] == "entries").unwrap();
    assert_eq!(entries["value"], 1);
    assert!(entries.get("description").unwrap().is_null());

    let nested = call(
        &mut api,
        &mut app,
        m::TAG_FIELDS,
        json!({"tag":"itmc", "path":"entries[0]"}),
    );
    let weight = &nested["fields"][0];
    assert_eq!(weight["name"], "weight");
    assert_eq!(weight["value"], 1.5);
    let via_offset = call(
        &mut api,
        &mut app,
        m::TAG_FIELDS,
        json!({"tag":"itmc", "path":"+0x0[0]"}),
    );
    assert_eq!(nested, via_offset);
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::TAG_FIELDS,
            json!({"tag":"itmc", "path":"entries[1]"}),
        )),
        code::REFUSED
    );
}

#[test]
fn the_camera_api_reads_and_sets_fly_speed() {
    let temp = Temp::new("fly-speed");
    let mut app = app(&temp);
    let mut api = Api::new();
    let initial = api.call(&mut app, m::VIEW_CAMERA, json!({})).unwrap();
    assert_eq!(initial["fly_speed"], json!(crate::camera::DEFAULT_SPEED));
    let response = api.call(&mut app, m::VIEW_CAMERA, json!({"fly_speed": 32})).unwrap();
    assert_eq!(response["fly_speed"], json!(32.0));
    let error = api.call(&mut app, m::VIEW_CAMERA, json!({"fly_speed": 0})).unwrap_err();
    assert_eq!(error.code, code::INVALID_PARAMS);
    assert_eq!(app.fly_speed(), 32.0);
}

#[test]
fn every_method_in_methods_has_a_test() {
    // Each method runs its own example from the table on a fresh app:
    // built ones answer, planned ones answer NOT_YET with their lane.
    let mut ran = Vec::new();
    for d in Api::new().methods() {
        let t = Temp::new("each");
        let mut app = app(&t);
        let mut api = Api::new();
        setup(&mut api, &mut app, &t, d.name);
        let params = example(&d, &t, &mut app);
        let r = api.call(&mut app, d.name, params.clone());
        match (d.planned, r) {
            (Some(lane), Err(e)) => {
                assert_eq!(e.code, code::NOT_YET, "{}: {e}", d.name);
                assert_eq!(e.data.unwrap()["lane"], lane);
            }
            (Some(_), Ok(v)) => panic!("{} is planned but answered {v}", d.name),
            // No renderer on this machine: the snapshot says so.
            (None, Err(e)) if d.name == m::VIEW_SNAPSHOT && e.code == code::NO_GPU => {}
            // These reads need an active watchdog, recorded samples or sampling.
            (None, Err(e))
                if matches!(d.name, mem::HISTORY | mem::HEAP | mem::DIFF | mem::POSTMORTEM)
                    && e.code == agent_proto::code::UNAVAILABLE => {}
            // The first frame has no paired or editor-launched game.
            (None, Err(e))
                if matches!(
                    d.name,
                    m::PAIR_STOP
                        | m::PAIR_EDIT
                        | m::PAIR_CONTROL
                        | m::MIRROR_APPLY
                        | m::MIRROR_PLAYER
                        | m::MIRROR_CONTROL
                ) && e.code == code::REFUSED => {}
            (None, Err(e)) => panic!("{} {params}: {e}", d.name),
            (None, Ok(v)) => assert!(!v.is_null() || matches!(d.name, m::METHODS | m::MIRROR_PICK), "{}", d.name),
        }
        ran.push(d.name);
    }
    // Every method name the protocol crate has is in the table.
    let names: Vec<&str> = METHODS.iter().map(|d| d.name).collect();
    for n in &names {
        assert_eq!(
            names.iter().filter(|x| *x == n).count(),
            1,
            "{n} listed twice"
        );
    }
    assert_eq!(ran.len(), Api::new().methods().len());
}

#[test]
fn pair_playtest_and_settings_share_the_window_controls() {
    let t = Temp::new("playtest-api");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(code_of(api.call(&mut app, m::PAIR_PLAYTEST, Value::Null)), code::REFUSED);
    crate::play::Play::new().update(&mut app);
    assert_eq!(call(&mut api, &mut app, m::PAIR_PLAYTEST, Value::Null), json!({"step":"launch"}));
    assert_eq!(app.pairing.requests, [crate::play::pair::Request::Play]);
    assert_eq!(
        code_of(api.call(&mut app, m::PAIR_PLAYTEST, json!({"confirm":"send"}))),
        code::REFUSED,
    );
    let settings = call(&mut api, &mut app, m::PAIR_SETTINGS, json!({
        "pause_while_editing": true, "spawn": "resume"
    }));
    assert_eq!(settings, json!({"pause_while_editing":true, "spawn":"resume"}));
    assert!(app.pairing.pause_while_editing);
    assert_eq!(app.pairing.spawn, Some(crate::play::Spawn::Resume));
}

#[cfg(unix)]
#[test]
fn pair_playtest_describes_manual_reload_and_prompt_answers() {
    use crate::play::{Play, playtest, testing::fake};
    use agent_proto::sim::SessionKind;

    let t = Temp::new("playtest-result");
    let mut app = app(&t);
    let mut api = Api::new();
    let definition = api.methods().into_iter().find(|d| d.name == m::PAIR_PLAYTEST).unwrap();
    let result: Value = serde_json::from_str(definition.result).unwrap();
    assert_eq!(result["why?"], "string");
    assert_eq!(result["answer?"], "cancel|open");

    let game = fake::Game::start("playtest-result", "other", SessionKind::Local);
    let mut play = Play::new();
    play.pair(&mut app, agent_proto::Endpoint::Unix(game.path.clone()));
    fake::until(&mut play, &mut app, "other game map", |_, a| {
        a.pairing.playtest.as_ref().is_some_and(|offer| offer.step
            == (playtest::Step::Ask { ask: playtest::Ask::Load }))
    });
    let mut previous = None;
    for choice in ["cancel", "open"] {
        let ask = call(&mut api, &mut app, m::PAIR_PLAYTEST, Value::Null);
        assert_eq!(ask["ask"], "load");
        let prompt = ask["prompt"].as_u64().unwrap();
        if let Some(old) = previous.replace(prompt) {
            assert_ne!(old, prompt);
            let stale = api.call(&mut app, m::PAIR_PLAYTEST, json!({"confirm":choice,"prompt":old}));
            assert_eq!(code_of(stale), code::REFUSED);
        }
        let reply = call(&mut api, &mut app, m::PAIR_PLAYTEST, json!({"confirm": choice}));
        assert_eq!(reply, json!({"step":"ask", "answer":choice}));
        play.update(&mut app);
    }
    let manual = playtest::Step::Reload(playtest::Reload::Manual).report(None).unwrap();
    assert_eq!(manual["step"], "reload");
    assert!(manual["why"].as_str().unwrap().contains("load it in the game"));
}

#[cfg(unix)]
#[test]
fn pair_status_carries_link_view() {
    use crate::play::Play;
    use crate::play::testing::fake;
    use agent_proto::sim::SessionKind;

    let t = Temp::new("pair-link");
    let mut app = app(&t);
    let mut play = Play::new();
    let mut api = Api::new();
    play.update(&mut app);
    let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
    assert_eq!(status["state"], "off");
    assert_eq!(
        status["link"],
        json!({"state":"no_game", "text":"No game", "why":null, "issues":[]})
    );
    let game = fake::Game::start("api-link", "example", SessionKind::Local);
    fake::pair_for_view(&mut play, &mut app, &game);
    let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
    assert_eq!(status["state"], "connected");
    assert_eq!(status["link"]["state"], "viewing");
    assert_eq!(status["link"]["text"], play.link_view(&app).text);
}

#[cfg(unix)]
mod paired_socket {
    use super::*;
    use crate::app::tag_tests::{self, WEAP};
    use crate::play::testing::fake::{self, Game, batch_pending, until};
    use crate::play::{Conn, Play, sync};
    use agent_proto::sim::SessionKind;
    use std::sync::mpsc::Receiver;
    use std::time::Duration;

    // The editor socket is pumped by the same thread that advances Play,
    // just as it is between frames of the window.
    pub(super) struct Editor {
        client: agent_proto::Client,
        _listener: server::Listener,
        rx: Receiver<server::Incoming>,
        app: App,
        api: Api,
        play: Play,
    }

    impl Editor {
        pub(super) fn paired(t: &Temp, tag: &str) -> (Game, Self) {
            let game = Game::start(tag, "example", SessionKind::Local);
            let (mut app, ..) = tag_tests::app(&t.0);
            let mut play = Play::new();
            play.request(crate::play::pair::Request::PairSocket(agent_proto::Endpoint::Unix(
                game.path.clone(),
            )));
            until(&mut play, &mut app, "the named game and its journal", |p, a| {
                p.edits_blocked(a).as_deref() == Some("View mode")
            });
            let sock = t.path("w.sock");
            let (listener, rx) = server::listen(&sock).unwrap();
            let client = studio_proto::cli::connect(&sock).unwrap();
            client.set_timeout(Some(Duration::from_secs(5))).unwrap();
            let mut api = Api::new();
            api.window = true;
            (
                game,
                Self {
                    client,
                    _listener: listener,
                    rx,
                    app,
                    api,
                    play,
                },
            )
        }

        pub(super) fn call(&mut self, method: &str, params: Value) -> Value {
            let client = &mut self.client;
            std::thread::scope(|scope| {
                let request = scope.spawn(move || client.call(method, params));
                while !request.is_finished() {
                    self.api.pump(&mut self.app, &self.rx);
                    self.play.update(&mut self.app);
                    std::thread::sleep(Duration::from_millis(2));
                }
                request.join().unwrap().unwrap()
            })
        }

        pub(super) fn wait(&mut self, what: &str, done: impl FnMut(&Play, &App) -> bool) {
            until(&mut self.play, &mut self.app, what, done);
        }

        fn send_speed(&mut self) {
            self.call(
                m::TAG_SET,
                json!({"tag":"weap", "path":"speed", "value":4.5}),
            );
            self.wait("the pending field", |_, a| !a.pairing.pending.is_empty());
            let pending = self.call(m::MIRROR_PENDING, Value::Null);
            let rows = pending["pending"].as_array().unwrap();
            assert_eq!(rows.len(), 1);
            assert_eq!(rows[0]["doc"], "tags");
            assert_eq!(rows[0]["at"], WEAP);
            assert!(rows[0]["path"].as_str().unwrap().contains("speed"));
            assert!(rows[0]["class"].as_str().unwrap().starts_with("Poke"));
            assert_eq!(
                self.call(m::MIRROR_APPLY, json!({"what":"send"})),
                json!({"queued":"send"})
            );
            self.wait("the outgoing batch", |p, _| batch_pending(p));
        }
    }

    #[test]
    fn mirror_apply_clears_the_pending_field_only_after_ack() {
        let t = Temp::new("mir-ack");
        let (game, mut editor) = Editor::paired(&t, "mir-ack");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        editor.call(m::MIRROR_APPLY, json!({"what":"send"}));
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"][0]["at"],
            WEAP
        );

        // A diff while the first write waits still compares to the file.
        editor.call(m::DOC_UNDO, json!({"doc":"tags"}));
        editor.wait("undo compared to the file", |_, a| {
            a.pairing.pending.is_empty()
        });
        editor.call(m::DOC_REDO, json!({"doc":"tags"}));
        editor.wait("redo compared to the file", |_, a| {
            !a.pairing.pending.is_empty()
        });
        assert!(batch_pending(&editor.play));
        drop(boundary);

        editor.wait("the acknowledged field", |p, a| {
            !batch_pending(p) && a.pairing.pending.is_empty()
        });
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"],
            json!([])
        );

        // The inverse is a fence on the ordered game connection. Any
        // duplicate batch queued before the ack would arrive first.
        editor.call(m::DOC_UNDO, json!({"doc":"tags"}));
        editor.wait("the inverse write", |p, a| {
            !batch_pending(p)
                && a.pairing.pending.is_empty()
                && game.calls("edit.batch").len() >= 2
        });
        let calls = game.calls("edit.batch");
        assert_eq!(calls.len(), 2);
        let (first, undo) = (&calls[0]["writes"][0], &calls[1]["writes"][0]);
        assert_eq!(first["at"], WEAP);
        assert_eq!(first["value"], sync::hex(&4.5f32.to_le_bytes()));
        assert_eq!(first["was"], sync::hex(&2.5f32.to_le_bytes()));
        assert_eq!(undo["was"], first["value"]);
        assert_eq!(undo["value"], first["was"]);
    }

    #[test]
    fn pair_disconnect_restores_after_the_outstanding_batch() {
        let t = Temp::new("mir-leave");
        let (game, mut editor) = Editor::paired(&t, "mir-leave");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        editor.call(m::PAIR_DISCONNECT, json!({"leave":"restore"}));
        assert_eq!(editor.call(m::MIRROR_PENDING, Value::Null)["leaving"], true);
        assert_eq!(
            editor.call(m::PAIR_STATUS, Value::Null)["state"],
            "connected"
        );
        drop(boundary);

        editor.wait("restore and disconnect", |p, _| p.conn == Conn::Off);
        assert_eq!(game.calls("edit.batch").len(), 1);
        assert_eq!(game.calls("edit.revert").len(), 1);
        let state = game.edits.lock().unwrap();
        assert!(state.journal.is_empty());
        assert_eq!(state.bytes[&WEAP.to_string()], sync::hex(&2.5f32.to_le_bytes()));
    }

    #[test]
    fn losing_the_socket_mid_batch_keeps_the_field_pending() {
        let t = Temp::new("mir-lost");
        let (game, mut editor) = Editor::paired(&t, "mir-lost");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        game.agent.shutdown();
        drop(boundary);
        drop(game);
        editor.wait("the interrupted connection", |p, a| {
            matches!(p.conn, Conn::Reconnecting { .. }) && !a.pairing.pending.is_empty()
        });
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"][0]["at"],
            WEAP
        );

        let replacement = Game::start("mir-new", "example", SessionKind::Local);
        fake::pair_for_view(&mut editor.play, &mut editor.app, &replacement);
        editor.call(m::MIRROR_APPLY, json!({"what":"send"}));
        editor.wait("the fresh game to acknowledge the edit", |p, a| {
            !batch_pending(p)
                && a.pairing.pending.is_empty()
                && !replacement.calls("edit.batch").is_empty()
        });
        let calls = replacement.calls("edit.batch");
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0]["writes"][0]["was"], sync::hex(&2.5f32.to_le_bytes()));
    }

    #[test]
    fn an_epoch_change_mid_batch_requires_a_fresh_send() {
        let t = Temp::new("mir-cut");
        let (game, mut editor) = Editor::paired(&t, "mir-cut");
        let boundary = game.boundary.lock().unwrap();
        editor.send_speed();
        game.bsp.store(1, std::sync::atomic::Ordering::Relaxed);
        drop(boundary);

        editor.wait("the new epoch and its journal", |p, a| {
            a.pairing.shown.as_ref().is_some_and(|s| s.epoch == 2)
                && p.link_view(a).state.name() == "viewing"
                && !a.pairing.pending.is_empty()
        });
        assert_eq!(game.calls("edit.batch").len(), 1);
        assert!(game.edits.lock().unwrap().bytes.is_empty());
        assert_eq!(
            editor.call(m::MIRROR_PENDING, Value::Null)["pending"][0]["at"],
            WEAP
        );

        editor.call(m::MIRROR_APPLY, json!({"what":"send"}));
        editor.wait("the new epoch to acknowledge", |p, a| {
            !batch_pending(p) && a.pairing.pending.is_empty()
        });
        let calls = game.calls("edit.batch");
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0]["epoch"], 1);
        assert_eq!(calls[1]["epoch"], 2);
        assert_eq!(calls[1]["writes"], calls[0]["writes"]);
    }
}

/// P14: `view.lighting` reads and sets the Lighting mode, refuses an
/// unknown one, and `view.stats` reports it under `native`, beside the
/// level's BSPs on the native renderer (none here: the test app builds
/// no art).
#[test]
fn view_lighting_sets_the_mode_and_stats_report_it() {
    let t = Temp::new("view-lighting");
    let mut app = app(&t);
    let mut api = Api::new();
    let read = call(&mut api, &mut app, m::VIEW_LIGHTING, Value::Null);
    assert_eq!(read["mode"], "game");
    assert_eq!(read["modes"], json!(["game", "flat", "lightmap", "collision"]));
    let set = call(&mut api, &mut app, m::VIEW_LIGHTING, json!({"mode": "lightmap"}));
    assert_eq!(set["mode"], "lightmap");
    assert_eq!(app.lighting, scenario_edit::native_view::Lighting::LightmapOnly);
    for invalid in [json!({"mode": "bright"}), json!({"mode": 3})] {
        let reply = api.call(&mut app, m::VIEW_LIGHTING, invalid);
        assert_eq!(code_of(reply), code::INVALID_PARAMS);
    }
    assert_eq!(app.lighting, scenario_edit::native_view::Lighting::LightmapOnly);
    let stats = call(&mut api, &mut app, m::VIEW_STATS, Value::Null);
    assert_eq!(stats["native"]["lighting"], "lightmap");
    assert!(stats["native"]["bsps"].as_array().is_some());
    assert!(stats["native"]["errors"].as_array().is_some());
    // Each BSP's base look: translated, or the named fallback with its
    // layers off.
    for b in stats["native"]["bsps"].as_array().unwrap() {
        let look = &b["base_look"];
        let translated = look["translated"].as_bool().expect("base_look.translated");
        assert_eq!(look["layers_off"], json!(!translated), "{look}");
        assert_eq!(look["fallback"].is_string(), !translated, "{look}");
    }
    // The placed objects: null with no art built, else the counts.
    let objects = &stats["native"]["objects"];
    assert!(stats["native"].get("objects").is_some());
    assert!(
        objects.is_null()
            || (objects["models"].is_u64()
                && objects["placed"].is_u64()
                && objects["errors"].is_array()),
        "{objects}"
    );
}

#[test]
fn view_layers_applies_overrides_atomically_and_shares_the_halo1_menu_switch() {
    let t = Temp::new("view-layers");
    let mut app = app(&t);
    let mut api = Api::new();
    let result = call(&mut api, &mut app, m::VIEW_LAYERS, json!({
        "preset": "everything", "set": {"fog": false}, "xray": true
    }));
    assert_eq!(result["xray"], true);
    let rows = result["layers"].as_array().unwrap();
    assert_eq!(rows.iter().find(|r| r["name"] == "fog").unwrap()["on"], false);
    let scenery = rows.iter().find(|r| r["label"] == "Scenery").unwrap();
    assert_eq!(scenery["count"], 1);
    assert_eq!(scenery["on"], true);
    assert_eq!(scenery["color"].as_array().unwrap().len(), 3);

    for invalid in [
        json!({"preset": "missing"}),
        json!({"preset": "art", "set": []}),
        json!({"preset": "art", "set": {"fog": true, "unknown": false}}),
        json!({"set": {"fog": "on"}}),
        json!({"set": {"fog": true}, "xray": 1}),
    ] {
        let reply = api.call(&mut app, m::VIEW_LAYERS, invalid);
        assert_eq!(code_of(reply), code::INVALID_PARAMS);
        assert_eq!(call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null), result);
    }

    assert!(app.overlay_shown());
    app.dispatch(Action::SetOverlay(false));
    let hidden = call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null);
    let halo = hidden["layers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["name"] == "halo1")
        .unwrap();
    assert_eq!(halo["on"], false);
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {"halo1": true}}));
    assert!(app.overlay_shown());
}

/// P8: the invented map's two volumes, kill trigger and BSP switch
/// through `volume.list`; `volume.select` replaces the placements, frames
/// the box and shows in `nav.status`; the layers count and draw them.
#[test]
fn volume_list_and_select_read_the_invented_volumes() {
    let t = Temp::new("volumes");
    let mut app = app(&t);
    let mut api = Api::new();
    // The plain invented map has none.
    let none = call(&mut api, &mut app, m::VOLUME_LIST, Value::Null);
    assert_eq!(none["volumes"], json!([]));
    assert_eq!(none["error"], Value::Null);
    let map = t.path("volumes.map");
    std::fs::write(&map, fixture::fixture_with_volumes()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );

    let all = call(&mut api, &mut app, m::VOLUME_LIST, Value::Null);
    let rows = all["volumes"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (all["kill_triggers"].clone(), all["bsp_switches"].clone()),
        (json!(1), json!(1))
    );
    for (row, (origin, forward, size)) in rows.iter().zip(fixture::VOLUMES) {
        assert_eq!(row["origin"], json!(origin));
        assert_eq!(row["forward"], json!(forward));
        assert_eq!(row["up"], json!([0.0, 0.0, 1.0]));
        assert_eq!(row["size"], json!(size));
        assert_eq!(row["world"], true);
    }
    assert_eq!(rows[0]["kinds"], json!(["bsp_switch", "trigger"]));
    assert_eq!(
        rows[0]["named_by"],
        json!([{"kind": "bsp_switch", "index": 0, "from": 0, "to": 1}])
    );
    assert_eq!(rows[1]["kinds"], json!(["kill", "trigger"]));
    assert_eq!(rows[1]["label"], "trigger volume 1");
    // The turned box: forward +y, so it runs 6 along y and 2 along -x.
    assert_eq!(rows[1]["center"], json!([-41.0, -37.0, -8.5]));
    let kills = call(&mut api, &mut app, m::VOLUME_LIST, json!({"kind": "kill"}));
    let kill_rows = kills["volumes"].as_array().unwrap();
    assert_eq!(kill_rows.len(), 1);
    assert_eq!(kill_rows[0]["index"], 1);
    assert_eq!(
        code_of(api.call(&mut app, m::VOLUME_LIST, json!({"kind": "soft"}))),
        code::INVALID_PARAMS
    );

    let layers = call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null);
    let row = |name: &str| {
        layers["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone()
    };
    for (name, count) in [
        ("trigger_volumes", 2),
        ("kill_volumes", 1),
        ("bsp_switches", 1),
    ] {
        assert_eq!(row(name)["group"], "Volumes", "{name}");
        assert_eq!(row(name)["count"], count, "{name}");
    }

    call(
        &mut api,
        &mut app,
        m::SCENARIO_SELECT,
        json!({"block": "weapons", "index": 0}),
    );
    let selected = call(
        &mut api,
        &mut app,
        m::VOLUME_SELECT,
        json!({"index": 1, "focus": true}),
    );
    assert_eq!(selected, json!({"selected": 1}));
    assert_eq!(app.session.selected(), None);
    let nav = call(&mut api, &mut app, m::NAV_STATUS, Value::Null);
    assert_eq!(
        (nav["volume"].clone(), nav["selected"].clone()),
        (json!(1), Value::Null)
    );
    assert_eq!(app.session.camera().target, [-41.0, -37.0, -8.5]);
    assert_eq!(
        call(&mut api, &mut app, m::VOLUME_LIST, Value::Null)["selected"],
        1
    );
    assert_eq!(
        code_of(api.call(&mut app, m::VOLUME_SELECT, json!({"index": 2}))),
        code::INVALID_PARAMS
    );
    assert_eq!(app.session.selected_volume(), Some(1));
    call(
        &mut api,
        &mut app,
        m::SCENARIO_SELECT,
        json!({"block": "weapons", "index": 0}),
    );
    assert_eq!(app.session.selected_volume(), None);
    call(&mut api, &mut app, m::VOLUME_SELECT, json!({"index": 0}));
    assert_eq!(
        call(&mut api, &mut app, m::VOLUME_SELECT, Value::Null),
        json!({"selected": null})
    );
    assert_eq!(app.session.selected_volume(), None);

    // Each volume layer draws its boxes in the snapshot.
    let out = t.path("volumes.png");
    let params = json!({
        "out": out, "size": [128, 96],
        "camera": {"target": [-41, -37, -8.5], "distance": 20, "yaw": 30, "pitch": 30}
    });
    call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"preset": "everything"}),
    );
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Ok(_) => {}
        Err(e) => panic!("view.snapshot: {e}"),
    }
    let shown = png_pixels(&out);
    call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"set": {"trigger_volumes": false, "kill_volumes": false}}),
    );
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
    let hidden = png_pixels(&out);
    assert!(
        hidden != shown,
        "hiding the volume layers must remove the box"
    );
    call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"set": {"kill_volumes": true}}),
    );
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    assert!(
        png_pixels(&out) != hidden,
        "the Kill layer alone draws the kill box"
    );
}

/// P10: the invented structure BSP's room through `collision.*`. It has
/// no render geometry, so all three surfaces are collision only;
/// `collision.at` finds the far wall by ray and by point whatever layers
/// are shown, and selecting it replaces the placements. The collision
/// layers start hidden and count the surfaces.
#[test]
fn collision_surfaces_and_at_report_the_invented_rooms_walls() {
    let t = Temp::new("collision");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = t.path("collision.map");
    std::fs::write(&map, fixture::fixture_with_collision()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    let all = call(&mut api, &mut app, m::COLLISION_SURFACES, Value::Null);
    assert_eq!(all["bsps"].as_array().unwrap().len(), 1);
    assert_eq!(all["counts"], json!({"drawn": 0, "collision_only": 3}));
    assert_eq!(all["errors"], json!([]));
    let rows = all["surfaces"].as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2]["class"], "collision_only");
    assert_eq!(rows[2]["breakable"], true);
    assert_eq!(rows[2]["ring"].as_array().unwrap().len(), 4);
    assert_eq!(rows[0]["breakable"], false);
    let drawn = call(
        &mut api,
        &mut app,
        m::COLLISION_SURFACES,
        json!({"class": "drawn"}),
    );
    assert_eq!(drawn["matching"], 0);
    let paged = call(
        &mut api,
        &mut app,
        m::COLLISION_SURFACES,
        json!({"offset": 1, "limit": 1}),
    );
    assert_eq!(paged["matching"], 3);
    assert_eq!(paged["surfaces"][0]["index"], 1);
    for bad in [
        (m::COLLISION_SURFACES, json!({"class": "invisible"})),
        (
            m::COLLISION_AT,
            json!({"origin": [0, 0, 0], "dir": [1, 0, 0], "point": [0, 0, 0]}),
        ),
        (m::COLLISION_AT, json!({"origin": [0, 0, 0]})),
        (
            m::COLLISION_AT,
            json!({"origin": [0, 0, 0], "dir": [0, 0, 0]}),
        ),
        (m::COLLISION_SELECT, json!({"index": 3})),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, bad.0, bad.1.clone())),
            code::INVALID_PARAMS,
            "{}",
            bad.1
        );
    }

    // The far wall stands at x 210; the layers are hidden by default.
    let ray = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({
            "origin": [205, 205, 1], "dir": [2, 0, 0]
        }),
    );
    assert_eq!(ray["surface"]["index"], 2);
    assert_eq!(ray["surface"]["class"], "collision_only");
    assert!((ray["distance"].as_f64().unwrap() - 5.0).abs() < 1e-4);
    let point = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({"point": [209.98, 205, 1]}),
    );
    assert_eq!(point["surface"]["index"], 2);
    let miss = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({"point": [205, 205, 2]}),
    );
    assert_eq!(miss["surface"], Value::Null);

    call(
        &mut api,
        &mut app,
        m::SCENARIO_SELECT,
        json!({"block": "netgame_flags", "index": 0}),
    );
    let picked = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({
            "origin": [205, 205, 1], "dir": [1, 0, 0], "select": true
        }),
    );
    assert_eq!(picked["selected"], 2);
    assert_eq!(app.session.selected(), None);
    let cleared = call(&mut api, &mut app, m::COLLISION_SELECT, Value::Null);
    assert_eq!(cleared["selected"], Value::Null);
    let framed = call(
        &mut api,
        &mut app,
        m::COLLISION_SELECT,
        json!({"index": 0, "focus": true}),
    );
    assert_eq!(framed["selected"], 0);
    let target = app.session.camera().target;
    assert!(
        (target[0] - 205.0).abs() < 1e-3 && (target[1] - 205.0).abs() < 1e-3,
        "{target:?}"
    );

    let layers = call(&mut api, &mut app, m::VIEW_LAYERS, Value::Null);
    let row = |name: &str| {
        layers["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()
            .clone()
    };
    for (name, count) in [("level_collision", 3), ("collision_only", 3)] {
        assert_eq!(row(name)["group"], "Collision", "{name}");
        assert_eq!(row(name)["count"], count, "{name}");
        assert_eq!(row(name)["on"], false, "{name}");
    }
    let shown = call(
        &mut api,
        &mut app,
        m::VIEW_LAYERS,
        json!({"preset": "collision"}),
    );
    assert!(
        shown["layers"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|r| r["group"] == "Collision")
            .all(|r| r["on"] == true)
    );
}

/// P11: the invented instance's quad through `collision.surfaces`, and
/// the invented scenery's `coll` cube and `phmo` sphere through
/// `collision.objects`, each counted by its layer.
#[test]
fn collision_objects_and_instances_report_the_invented_shapes() {
    let t = Temp::new("collision-objects");
    let mut app = app(&t);
    let mut api = Api::new();
    let open = |api: &mut Api, app: &mut App, name: &str, bytes: Vec<u8>| {
        let map = t.path(name);
        std::fs::write(&map, bytes).unwrap();
        call(
            api,
            app,
            m::SOURCE_OPEN,
            json!({"path": map.display().to_string()}),
        );
    };
    let layer = |api: &mut Api, app: &mut App, name: &str| {
        let layers = call(api, app, m::VIEW_LAYERS, Value::Null);
        layers["layers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|r| r["name"] == name)
            .unwrap()["count"]
            .clone()
    };

    open(
        &mut api,
        &mut app,
        "instance.map",
        fixture::fixture_with_instance_collision(),
    );
    let all = call(&mut api, &mut app, m::COLLISION_SURFACES, Value::Null);
    let rows = all["surfaces"].as_array().unwrap();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[0]["instance"], Value::Null);
    assert_eq!(rows[3]["instance"], 0);
    assert_eq!(rows[3]["plane"], json!([0.0, 0.0, 1.0, 3.0]));
    assert_eq!(layer(&mut api, &mut app, "level_collision"), 3);
    assert_eq!(layer(&mut api, &mut app, "instance_collision"), 1);
    let down = call(
        &mut api,
        &mut app,
        m::COLLISION_AT,
        json!({"origin": [202.25, 202.5, 10], "dir": [0, 0, -1]}),
    );
    assert_eq!(down["surface"]["index"], 3);

    open(
        &mut api,
        &mut app,
        "object.map",
        fixture::fixture_with_object_collision(),
    );
    let objects = call(&mut api, &mut app, m::COLLISION_OBJECTS, Value::Null);
    assert_eq!(objects["error"], Value::Null);
    let models = objects["models"].as_array().unwrap();
    let crate_model = models.iter().find(|m| m["shapes"] == 2).unwrap();
    assert_eq!(crate_model["nodes"], 2);
    assert_eq!(crate_model["errors"], json!([]));
    let shapes = objects["shapes"].as_array().unwrap();
    assert_eq!(shapes.len(), 2);
    let kinds: Vec<&str> = shapes.iter().map(|s| s["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["bsp", "sphere"]);
    for s in shapes {
        assert_eq!(s["block"], "scenery");
        assert_eq!(s["index"], 0);
    }
    assert_eq!(shapes[0]["node"], 1);
    assert_eq!(shapes[0]["center"], json!([5.0, 5.0, 6.0]));
    assert_eq!(shapes[1]["center"], json!([5.0, 5.0, 5.5]));
    assert_eq!(shapes[1]["radius"], 0.5);
    let none = call(
        &mut api,
        &mut app,
        m::COLLISION_OBJECTS,
        json!({"block": "weapons"}),
    );
    assert_eq!(none["shapes"], json!([]));
    assert_eq!(layer(&mut api, &mut app, "object_collision"), 2);
}

/// P9: `volume.set` moves, turns and resizes a box as one undo step each;
/// `volume.remove` refuses a volume a kill trigger names, naming it, and
/// takes the kill trigger first; `volume.add` with `kill` adds both
/// entries, refused past the source size while growth is off.
#[test]
fn volume_set_add_and_remove_edit_through_the_command_layer() {
    let t = Temp::new("volume-edits");
    let mut app = app(&t);
    let mut api = Api::new();
    let map = t.path("volumes.map");
    std::fs::write(&map, fixture::fixture_with_volumes()).unwrap();
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": map.display().to_string()}),
    );
    let undo = |api: &mut Api, app: &mut App| {
        call(api, app, m::DOC_UNDO, json!({"doc": "map"}));
    };

    // Move by the centre, then resize keeping it, then turn.
    let moved = call(
        &mut api,
        &mut app,
        m::VOLUME_SET,
        json!({"index": 0, "center": [110, 100, 2]}),
    );
    assert_eq!(moved["volume"]["center"], json!([110.0, 100.0, 2.0]));
    assert_eq!(moved["volume"]["size"], json!([4.0, 4.0, 4.0]));
    let sized = call(
        &mut api,
        &mut app,
        m::VOLUME_SET,
        json!({"index": 0, "size": [8, 2, 4]}),
    );
    assert_eq!(sized["volume"]["center"], json!([110.0, 100.0, 2.0]));
    assert_eq!(sized["volume"]["origin"], json!([106.0, 99.0, 0.0]));
    let turned = call(
        &mut api,
        &mut app,
        m::VOLUME_SET,
        json!({"index": 0, "forward": [0, 2, 0]}),
    );
    assert_eq!(turned["volume"]["forward"], json!([0.0, 1.0, 0.0]));
    assert_eq!(turned["volume"]["center"], json!([110.0, 100.0, 2.0]));
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::VOLUME_SET,
            json!({"index": 0, "size": [0, 1, 1]})
        )),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::VOLUME_SET, json!({"index": 5}))),
        code::INVALID_PARAMS
    );
    for _ in 0..3 {
        undo(&mut api, &mut app);
    }
    let row = &call(&mut api, &mut app, m::VOLUME_LIST, Value::Null)["volumes"][0];
    assert_eq!(row["origin"], json!(fixture::VOLUMES[0].0));
    assert_eq!(row["size"], json!(fixture::VOLUMES[0].2));
    assert!(!app.session.is_dirty());

    // Volume 1 is the kill box: the refusal names its kill trigger.
    let e = api
        .call(&mut app, m::VOLUME_REMOVE, json!({"index": 1}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("kill trigger 0"), "{}", e.message);
    assert_eq!(
        e.data.unwrap()["named_by"],
        json!([{"kind": "kill", "index": 0}])
    );
    let gone = call(
        &mut api,
        &mut app,
        m::VOLUME_REMOVE,
        json!({"kind": "kill", "index": 0}),
    );
    assert_eq!(gone["kill_triggers"], 0);
    let gone = call(&mut api, &mut app, m::VOLUME_REMOVE, json!({"index": 1}));
    assert_eq!(gone["triggers"], 1);

    // Room for one more of each: Make a kill volume fills it.
    let made = call(
        &mut api,
        &mut app,
        m::VOLUME_ADD,
        json!({"kill": true, "center": [0, 0, 2], "size": [10, 10, 1]}),
    );
    assert_eq!(
        (made["index"].clone(), made["kill_trigger"].clone()),
        (json!(1), json!(0))
    );
    assert_eq!(made["volume"]["kinds"], json!(["kill", "trigger"]));
    assert_eq!(made["volume"]["center"], json!([0.0, 0.0, 2.0]));
    assert_eq!(app.session.selected_volume(), Some(1));
    let e = api
        .call(&mut app, m::VOLUME_ADD, json!({"kill": true}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert_eq!(e.data.unwrap()["capacity"], 2);
    // One undo takes both entries away.
    undo(&mut api, &mut app);
    let list = call(&mut api, &mut app, m::VOLUME_LIST, Value::Null);
    assert_eq!(list["volumes"].as_array().unwrap().len(), 1);
    assert_eq!(list["kill_triggers"], 0);
    // BSP switches are read only.
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::VOLUME_REMOVE,
            json!({"kind": "bsp_switch", "index": 0})
        )),
        code::INVALID_PARAMS
    );
}

fn png_pixels(path: &Path) -> Vec<u8> {
    let file = std::io::BufReader::new(std::fs::File::open(path).unwrap());
    let mut png = png::Decoder::new(file).read_info().unwrap();
    let mut rgba = vec![0; png.output_buffer_size().unwrap()];
    png.next_frame(&mut rgba).unwrap();
    rgba
}

#[test]
fn view_layers_changes_snapshot_pixels_on_the_fixture() {
    let t = Temp::new("view-layer-pixels");
    let mut app = app(&t);
    let mut api = Api::new();
    let out = t.path("view.png");
    let params = json!({
        "out": out, "size": [128, 96],
        "camera": {"target": [5, 5, 5], "distance": 2, "yaw": 15, "pitch": 15}
    });
    // The fixture has one scenery glyph at [5, 5, 5].
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"preset": "everything"}));
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Ok(_) => {}
        Err(e) => panic!("view.snapshot: {e}"),
    }
    let shown = png_pixels(&out);
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {"scenery": false}}));
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
    assert!(png_pixels(&out) != shown, "hiding scenery must remove its glyph from the snapshot");
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {"scenery": true}}));
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    assert!(png_pixels(&out) == shown, "showing scenery must restore the original pixels");
}

#[test]
fn view_layers_gameplay_legend_counts_survive_changes_of_preset() {
    let t = Temp::new("view-gameplay-legend");
    let mut app = app(&t);
    let mut api = Api::new();
    for (preset, enabled) in [
        ("gameplay", [true, true, true]),
        ("art", [false, false, true]),
        ("collision", [false, false, false]),
    ] {
        let state = call(&mut api, &mut app, m::VIEW_LAYERS, json!({"preset": preset}));
        let rows = state["layers"].as_array().unwrap();
        for ((name, count), on) in [("spawn_points", 2), ("flags", 1), ("netgame_items", 1)]
            .into_iter()
            .zip(enabled)
        {
            let row = rows.iter().find(|r| r["name"] == name).unwrap();
            assert_eq!(row["group"], "Gameplay", "{name}");
            assert_eq!(row["count"], count, "{name}");
            assert_eq!(row["on"], on, "{preset} / {name}");
        }
    }
}

#[test]
fn view_layers_gameplay_switches_and_respawn_labels_change_snapshot_pixels() {
    let t = Temp::new("view-gameplay-pixels");
    let mut app = app(&t);
    let mut api = Api::new();
    let path = t.path("gameplay.png");
    // All three kinds of placement are in frame. Keep the camera fixed so
    // only the changed layer (or the edited respawn caption) affects pixels.
    let params = json!({
        "out": path, "size": [320, 240],
        "camera": {"target": [1, 1, 1], "distance": 14, "yaw": 80, "pitch": 15}
    });
    call(&mut api, &mut app, m::VIEW_LAYERS, json!({"preset": "gameplay"}));
    match api.call(&mut app, m::VIEW_SNAPSHOT, params.clone()) {
        Err(e) if e.code == code::NO_GPU => return,
        Err(e) => panic!("view.snapshot: {e}"),
        Ok(_) => {}
    }
    let shown = png_pixels(&path);
    for name in ["spawn_points", "flags", "netgame_items"] {
        call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {(name): false}}));
        call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
        assert!(png_pixels(&path) != shown, "hiding {name} must change the picture");
        call(&mut api, &mut app, m::VIEW_LAYERS, json!({"set": {(name): true}}));
    }
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params.clone());
    assert!(png_pixels(&path) == shown, "showing all three layers must restore the picture");

    call(&mut api, &mut app, m::SCENARIO_SET, json!({
        "block": "netgame_equipment", "index": 0, "field": "respawn time", "value": 77
    }));
    call(&mut api, &mut app, m::VIEW_SNAPSHOT, params);
    assert!(png_pixels(&path) != shown, "the equipment's new respawn time must appear in its caption");
}

#[test]
fn methods_describe_their_params_results_and_examples() {
    let t = Temp::new("methods");
    let mut app = app(&t);
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::METHODS, Value::Null);
    let list = list.as_array().unwrap();
    // The table with the providers' methods over and after it.
    let defs = api.methods();
    assert_eq!(list.len(), defs.len());
    for (d, v) in defs.iter().zip(list) {
        assert_eq!(v["name"], d.name);
        assert!(
            v["params"].is_object(),
            "{}: params shape {}",
            d.name,
            d.params
        );
        assert!(
            !v["result"].is_string(),
            "{}: result shape {}",
            d.name,
            d.result
        );
        assert!(!d.summary.is_empty());
        assert_eq!(
            v["status"],
            if d.planned.is_some() {
                "planned"
            } else {
                "ready"
            }
        );
        // The example names only params the shape has.
        if let Value::Object(o) = &v["example"] {
            let keys = shape_keys(d.params);
            for k in o.keys() {
                assert!(
                    keys.iter().any(|(n, _)| n == k),
                    "{}: example param {k}",
                    d.name
                );
            }
        }
    }
    let planned: Vec<&str> = list
        .iter()
        .filter(|v| v["status"] == "planned")
        .map(|v| v["name"].as_str().unwrap().split('.').next().unwrap())
        .collect();
    for ns in ["model", "custom"] {
        assert!(planned.contains(&ns), "{ns}.* is a planned namespace");
    }
    // The HUD tool's provider answers all of hud.*.
    assert!(!planned.contains(&"hud"));
    for name in [m::HUD_LIST, m::HUD_READ, m::HUD_SET, m::HUD_MOVE, m::HUD_RENDER, m::HUD_CHECK, m::HUD_SAVE_AS] {
        assert!(list.iter().any(|v| v["name"] == name && v["status"] == "ready"), "{name}");
    }
}

#[test]
fn params_are_checked_before_anything_runs() {
    let t = Temp::new("params");
    let mut app = app(&t);
    let mut api = Api::new();
    let before = flag_position(&app);
    let set = |p: Value| (m::SCENARIO_SET, p);
    for (method, p) in [
        set(
            json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [1, 2, 3], "typo": 1}),
        ),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position"})),
        set(json!({"block": "nope", "index": 0, "field": "position", "value": [1, 2, 3]})),
        set(json!({"block": "netgame_flags", "index": 9, "field": "position", "value": [1, 2, 3]})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "nope", "value": 1})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [1, 2]})),
        set(json!({"block": "netgame_flags", "index": 0, "field": "position", "value": "up"})),
        (m::SCENARIO_GET, json!([1, 2])),
    ] {
        assert_eq!(
            code_of(api.call(&mut app, method, p.clone())),
            code::INVALID_PARAMS,
            "{p}"
        );
    }
    assert_eq!(
        code_of(api.call(&mut app, "scenario.nope", Value::Null)),
        code::METHOD_NOT_FOUND
    );
    assert_eq!(flag_position(&app), before, "nothing was applied");
    assert!(!app.is_dirty());
}

#[test]
fn growth_where_the_map_editor_cannot_grow_safely_is_refused_with_a_reason() {
    let t = Temp::new("growth");
    let mut app = app(&t);
    let mut api = Api::new();
    let e = api
        .call(&mut app, m::SCENARIO_ADD, json!({"block": "netgame_flags"}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(e.message.contains("full"), "{}", e.message);
    assert_eq!(e.data.unwrap(), json!({"count": 1, "capacity": 1}));
    let e = api
        .call(
            &mut app,
            m::TAG_BLOCK_ADD,
            json!({"tag": "scnr", "path": "netgame_flags"}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
    assert!(!app.is_dirty());
    // With room made, the same add goes in.
    call(
        &mut api,
        &mut app,
        m::SCENARIO_DELETE,
        json!({"block": "netgame_flags", "index": 0}),
    );
    let v = call(
        &mut api,
        &mut app,
        m::SCENARIO_ADD,
        json!({"block": "netgame_flags", "position": [9, 9, 9]}),
    );
    assert_eq!(v["index"], 0);
    assert_eq!(flag_position(&app), [9.0, 9.0, 9.0]);
}

#[test]
fn source_maps_reports_the_picker_groups_in_order() {
    use crate::app::map_cards_tests::{maps_folder, package_with_picture};

    let t = Temp::new("source-maps-groups");
    let maps = maps_folder(&t.0);
    package_with_picture(&t.path("data"));
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    let mut api = Api::new();
    call(&mut api, &mut app, m::SOURCE_OPEN, json!({ "path": maps }));
    app.wait_cards();

    let result = call(&mut api, &mut app, m::SOURCE_MAPS, Value::Null);
    assert_eq!(result["maps"], json!(["02_pier", "mainmenu", "shared", "yard"]));
    assert_eq!(
        result["map_entries"],
        json!([
            {"map": "yard", "label": "Yard", "group": "multiplayer"},
            {"map": "02_pier", "label": "The Pier", "group": "campaign"},
            {"map": "mainmenu", "label": "mainmenu", "group": "shared_and_menu"},
            {"map": "shared", "label": "shared", "group": "shared_and_menu"},
        ]),
    );
    assert_eq!(result["packages"][0]["group"], "packages");
}

#[test]
fn unsaved_edits_are_never_dropped_without_discard() {
    let t = Temp::new("unsaved");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [1, 1, 1]}),
    );
    let path = t.path("example.map").display().to_string();
    assert_eq!(
        code_of(api.call(&mut app, m::SOURCE_OPEN, json!({"path": path}))),
        code::UNSAVED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_CLOSE, json!({"doc": "map"}))),
        code::UNSAVED
    );
    assert!(
        app.is_dirty() && app.pending().is_none(),
        "refused, not held for a prompt"
    );
    call(
        &mut api,
        &mut app,
        m::SOURCE_OPEN,
        json!({"path": path, "discard": true}),
    );
    assert!(!app.is_dirty());
}

#[test]
fn outputs_stay_out_of_git_and_off_the_source() {
    let t = Temp::new("git");
    let mut app = app(&t);
    let mut api = Api::new();
    call(
        &mut api,
        &mut app,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [1, 1, 1]}),
    );
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("never.map");
    let e = api
        .call(
            &mut app,
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": repo.display().to_string()}),
        )
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    assert!(!repo.exists());
    let src = t.path("example.map").display().to_string();
    assert_eq!(
        code_of(api.call(&mut app, m::DOC_SAVE_AS, json!({"doc": "map", "path": src}))),
        code::REFUSED
    );
    assert_eq!(
        std::fs::read(t.path("example.map")).unwrap(),
        fixture::fixture()
    );
    let save = call(&mut api, &mut app, m::DOC_SAVE, json!({"doc": "map"}));
    assert_eq!(save["saved"], false);
    assert_eq!(save["package_form"], json!({"name":"example edit", "dir":"example-edit"}));
    assert_eq!(app.package_form.as_ref().unwrap().dir, "example-edit");
    assert_eq!(std::fs::read(t.path("example.map")).unwrap(), fixture::fixture());
    let first = crate::app::UiScreens::tree()[0].0;
    call(&mut api, &mut app, m::UI_PICK, json!({"screen": first}));
    let png = Path::new(env!("CARGO_MANIFEST_DIR")).join("never.png");
    assert_eq!(
        code_of(api.call(
            &mut app,
            m::UI_RENDER,
            json!({"out": png.display().to_string()})
        )),
        code::REFUSED
    );
    assert!(!png.exists());
}

#[test]
fn undo_and_redo_through_the_api_for_each_document() {
    let t = Temp::new("undo");
    let mut app = app(&t);
    let mut api = Api::new();
    let before = flag_position(&app);
    call(
        &mut api,
        &mut app,
        m::SCENARIO_SET,
        json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4, 5, 6]}),
    );
    // The variant tool is shown now: undo still names the map.
    call(
        &mut api,
        &mut app,
        m::DOC_OPEN,
        json!({"variant": {"new": "slayer"}}),
    );
    let v = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(v["done"], true);
    assert_eq!(flag_position(&app), before);
    let v = call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(v["done"], false, "nothing left");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&app), [4.0, 5.0, 6.0]);

    call(
        &mut api,
        &mut app,
        m::VARIANT_RENAME,
        json!({"name": "Renamed"}),
    );
    let name = |app: &App| app.variants.as_ref().unwrap().current().unwrap().name();
    assert_eq!(name(&app), "Renamed");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "variant"}));
    assert_ne!(name(&app), "Renamed");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "variant"}));
    assert_eq!(name(&app), "Renamed");

    let hop = t.path("multiplayer_hopper.dat").display().to_string();
    call(&mut api, &mut app, m::DOC_OPEN, json!({"playlists": hop}));
    call(
        &mut api,
        &mut app,
        m::PLAYLIST_SET,
        json!({"playlist": 0, "title": "Agent"}),
    );
    let title = |app: &App| app.hoppers.hoppers().unwrap()[0].title.clone();
    assert_eq!(title(&app), "Agent");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "playlists"}));
    assert_ne!(title(&app), "Agent");
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "playlists"}));
    assert_eq!(title(&app), "Agent");
    let docs = call(&mut api, &mut app, m::DOC_LIST, Value::Null);
    let dirty: Vec<&str> = docs
        .as_array()
        .unwrap()
        .iter()
        .filter(|d| d["dirty"] == true)
        .map(|d| d["doc"].as_str().unwrap())
        .collect();
    assert_eq!(dirty, ["map", "variant", "playlists"]);
}

#[test]
fn the_window_and_the_api_make_identical_documents() {
    // The inspector's path: a dragged field (merged under one key), then
    // the end of the drag, as dispatched actions.
    let t = Temp::new("same");
    let mut ui = app(&t);
    ui.dispatch(Action::SetField {
        block: BlockKind::NetgameFlags,
        index: 0,
        field: "position".into(),
        value: scenario_edit::Value::Vec3([2.0, 2.0, 2.0]),
        merge: Some(7),
    });
    ui.dispatch(Action::SetField {
        block: BlockKind::NetgameFlags,
        index: 0,
        field: "position".into(),
        value: scenario_edit::Value::Vec3([4.0, 5.0, 6.0]),
        merge: Some(7),
    });
    ui.dispatch(Action::SealEdit);
    // The API's path.
    let t2 = Temp::new("same-api");
    let mut agent = app(&t2);
    let mut api = Api::new();
    call(
        &mut api,
        &mut agent,
        m::SCENARIO_SET,
        json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4, 5, 6]}),
    );
    let (a, b) = (
        ui.session.scenario().unwrap(),
        agent.session.scenario().unwrap(),
    );
    assert_eq!(a.changes(), b.changes());
    assert!(!a.changes().is_empty());
    // One undo step each.
    ui.dispatch(Action::Undo);
    call(&mut api, &mut agent, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&ui), flag_position(&agent));
    ui.dispatch(Action::Redo);
    call(&mut api, &mut agent, m::DOC_REDO, json!({"doc": "map"}));
    let (pa, pb) = (t.path("ui.map"), t2.path("api.map"));
    ui.dispatch(Action::SaveMapAs(pa.clone()));
    call(
        &mut api,
        &mut agent,
        m::DOC_SAVE_AS,
        json!({"doc": "map", "path": pb.display().to_string()}),
    );
    assert_eq!(std::fs::read(pa).unwrap(), std::fs::read(pb).unwrap());

    // Playlists: the table's row buttons apply the same edits.
    let hop = |t: &Temp| t.path("multiplayer_hopper.dat");
    ui.dispatch(Action::OpenHopper(hop(&t)));
    let edits = [
        PlaylistEdit::Weight {
            playlist: 0,
            entry: 0,
            weight: 9,
        },
        PlaylistEdit::Duplicate {
            playlist: 0,
            entry: 0,
        },
        PlaylistEdit::Move {
            playlist: 0,
            entry: 1,
            to: 0,
        },
    ];
    for e in &edits {
        // As entries_table does inside its frame's edit.
        ui.hoppers.edit(|hs| e.apply(hs).unwrap());
    }
    call(
        &mut api,
        &mut agent,
        m::DOC_OPEN,
        json!({"playlists": hop(&t2).display().to_string()}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "weight", "entry": 0, "weight": 9}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "duplicate", "entry": 0}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_ENTRY,
        json!({"playlist": 0, "op": "move", "entry": 1, "to": 0}),
    );
    assert_eq!(ui.hoppers.hoppers(), agent.hoppers.hoppers());
}

#[test]
fn tags_list_read_and_follow_references() {
    let t = Temp::new("tags");
    let mut app = app(&t);
    let mut api = Api::new();
    let all = call(&mut api, &mut app, m::TAG_LIST, Value::Null);
    assert_eq!(all["total"], 4);
    let scnr = call(&mut api, &mut app, m::TAG_LIST, json!({"group": "scnr"}));
    assert_eq!(scnr["tags"][0]["name"], "levels\\test\\fixture");
    let raw = |t: Value| json!({"tag": t, "raw": true});
    let byname = call(&mut api, &mut app, m::TAG_READ, raw(json!("levels\\test\\fixture")));
    let bygroup = call(&mut api, &mut app, m::TAG_READ, raw(json!("scnr")));
    let byid = call(&mut api, &mut app, m::TAG_READ, raw(scnr["tags"][0]["id"].clone()));
    assert_eq!(byname, bygroup);
    assert_eq!(byname, byid);
    assert_eq!(byname["layout"], "schema");
    assert!(byname["root"]["fields"].as_array().unwrap().len() > 3);
    // The weapon palette names the blaster.
    let refs = call(&mut api, &mut app, m::TAG_REFS, json!({"tag": "scnr"}));
    let blaster = refs
        .as_array()
        .unwrap()
        .iter()
        .position(|r| r["name"] == "objects\\test\\blaster")
        .expect("the palette's reference");
    let to = call(&mut api, &mut app, m::TAG_FOLLOW, json!({"tag": "scnr", "ref": blaster}));
    assert_eq!(to["group"], "weap");
    let path = refs[blaster]["path"].clone();
    assert_eq!(call(&mut api, &mut app, m::TAG_FOLLOW, json!({"tag": "scnr", "ref": path})), to);

    // Any tag's fields through the Tags tool's registry and commands.
    let kit = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc:objects\\test\\kit"}));
    assert_eq!(kit["access"], "edit", "{kit}");
    assert_eq!(kit, call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"})));
    let v = call(&mut api, &mut app, m::TAG_SET, json!({"tag": "itmc", "path": "+0x8", "value": 7}));
    assert_eq!(v["value"], 7);
    assert!(app.is_dirty(), "a tag edit is the map's to save");
    let diff = call(&mut api, &mut app, m::DOC_DIFF, json!({"doc": "tags"}));
    assert_eq!(diff["patches"][0]["tag"], "itmc:objects\\test\\kit", "{diff}");
    assert_eq!(diff["patches"][0]["offset"], "0x8");
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "tags"}));
    assert!(!app.is_dirty());
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "tags"}));
    assert_eq!(call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}))["edited"], true);
    // A tag too short for its group's layout is refused with the reason.
    let e = api
        .call(&mut app, m::TAG_SET, json!({"tag": "weap", "path": "+0x4", "value": 1}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    assert!(e.message.contains("shorter than"), "{}", e.message);
    // The scenario's placements still edit as scenario.set.
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": "scnr", "path": "netgame_flags[0].position", "value": [7, 7, 7]}));
    assert_eq!(flag_position(&app), [7.0, 7.0, 7.0]);
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_READ, json!({"tag": "nope"}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn tags_browse_a_shared_cache_and_round_trip_through_a_bundle() {
    let t = Temp::new("tag-io");
    let mut app = app(&t);
    let mut api = Api::new();
    std::fs::write(t.path("shared.map"), fixture::fixture()).unwrap();
    // The open map's tags, by group and by folder, with where they live.
    let all = call(&mut api, &mut app, m::TAG_LIST, json!({"homes": true}));
    assert_eq!(all["map"], "example");
    assert_eq!(all["browsing"], false);
    assert_eq!(all["groups"]["weap"], 1);
    assert!(all["tags"].as_array().unwrap().iter().any(|t| t["raw"]["local"].is_number()));
    let objects = call(&mut api, &mut app, m::TAG_LIST, json!({"path": "objects\\test"}));
    assert_eq!(objects["total"], 2, "{objects}");
    let q = call(&mut api, &mut app, m::TAG_LIST, json!({"query": "itmc kit"}));
    assert_eq!(q["total"], 1);

    // A shared cache, read-only.
    let b = call(&mut api, &mut app, m::TAG_BROWSE, json!({"map": "shared"}));
    assert_eq!((b["map"].clone(), b["browsing"].clone(), b["total"].clone()), (json!("shared"), json!(true), json!(4)));
    let kit = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    assert!(kit["access"]["read_only"].is_string(), "{kit}");
    let e = api
        .call(&mut app, m::TAG_SET, json!({"tag": "itmc", "path": "+0x8", "value": 7}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED, "{}", e.message);
    let res = call(&mut api, &mut app, m::TAG_RESOURCES, json!({"tag": "itmc"}));
    assert_eq!(res["raw"], json!([]));
    assert_eq!(res["media"], Value::Null);
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_IMPORT, json!({"path": t.path("x.h2tag").display().to_string()}))),
        code::REFUSED
    );
    let back = call(&mut api, &mut app, m::TAG_BROWSE, json!({}));
    assert_eq!(back["map"], "example");
    assert_eq!(back["browsing"], false);

    // Export, edit, import: the edit is gone, as one undo step.
    let out = t.path("out/kit.h2tag");
    let x = call(&mut api, &mut app, m::TAG_EXPORT, json!({"tag": "itmc", "out": out.display().to_string()}));
    assert_eq!(x["tags"], json!(["itmc:objects\\test\\kit"]));
    let before = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    call(&mut api, &mut app, m::TAG_SET, json!({"tag": "itmc", "path": "+0x8", "value": 7}));
    let r = call(&mut api, &mut app, m::TAG_IMPORT, json!({"path": out.display().to_string()}));
    assert_eq!(r["replaced"], json!(["itmc:objects\\test\\kit"]));
    let after = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    assert_eq!(after["fields"], before["fields"]);
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "tags"}));
    let undone = call(&mut api, &mut app, m::TAG_READ, json!({"tag": "itmc"}));
    assert_ne!(undone["fields"], before["fields"], "undo takes the import back");
    // Never into a Git checkout.
    let repo = t.path("repo");
    std::fs::create_dir_all(repo.join(".git")).unwrap();
    let e = api
        .call(&mut app, m::TAG_EXPORT, json!({"tag": "itmc", "out": repo.join("k.h2tag").display().to_string()}))
        .unwrap_err();
    assert_eq!(e.code, code::REFUSED);
}

struct Models;
impl Provider for Models {
    fn methods(&self) -> Vec<MethodDef> {
        vec![MethodDef {
            name: m::MODEL_LIST,
            summary: "test models",
            params: "{}",
            result: "[string]",
            example: "null",
            planned: None,
        }]
    }
    fn call(&mut self, _: &mut App, method: &str, _: &Value) -> Option<Reply> {
        (method == m::MODEL_LIST).then(|| Ok(json!(["masterchief"])))
    }
}

#[test]
fn a_registered_provider_answers_a_planned_method() {
    let t = Temp::new("provider");
    let mut app = app(&t);
    let mut api = Api::new();
    assert_eq!(
        code_of(api.call(&mut app, m::MODEL_LIST, Value::Null)),
        code::NOT_YET
    );
    api.register(Box::new(Models));
    assert_eq!(
        call(&mut api, &mut app, m::MODEL_LIST, Value::Null),
        json!(["masterchief"])
    );
    let listed = call(&mut api, &mut app, m::METHODS, Value::Null);
    let model = listed
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == m::MODEL_LIST)
        .unwrap();
    assert_eq!(model["status"], "ready");
    // The rest of the namespace stays planned.
    assert_eq!(
        code_of(api.call(&mut app, m::MODEL_READ, json!({"tag": "x"}))),
        code::NOT_YET
    );
}

/// A headless editor on a socket, as `h2-editor --serve` runs: the app
/// lives on its own thread, which answers the queue.
fn serve(
    sock: PathBuf,
    map: PathBuf,
    data: PathBuf,
) -> (server::Listener, std::thread::JoinHandle<()>) {
    let (listener, rx) = server::listen(&sock).unwrap();
    let h = std::thread::spawn(move || {
        let mut app = App::new();
        app.set_data_root(data);
        let _ = map;
        let mut api = Api::new();
        for inc in rx.iter() {
            let r = api.handle(&mut app, inc.request.clone());
            inc.answer(r);
        }
    });
    (listener, h)
}

#[test]
fn a_headless_editor_opens_sets_saves_and_patches_only_those_bytes() {
    let t = Temp::new("e2e");
    let map = t.path("example.map");
    std::fs::write(&map, fixture::fixture()).unwrap();
    let sock = t.path("s.sock");
    let (listener, _h) = serve(sock.clone(), map.clone(), t.path("data"));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&sock).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the socket is the user's alone");
    }
    let mut c = studio_proto::cli::connect(&sock).unwrap();
    c.set_timeout(Some(std::time::Duration::from_secs(30)))
        .unwrap();
    let status = c.call(m::STATUS, Value::Null).unwrap();
    assert_eq!(status["window"], false);
    c.call(m::SOURCE_OPEN, json!({"path": map.display().to_string()}))
        .unwrap();
    let got = c
        .call(m::SCENARIO_SET, json!({"block": "netgame_flags", "index": 0, "field": "position", "value": [4.5, -2, 8]}))
        .unwrap();
    assert_eq!(got["value"], json!([4.5, -2.0, 8.0]));
    let diff = c.call(m::DOC_DIFF, json!({"doc": "map"})).unwrap();
    let patches = diff["patches"].as_array().unwrap();
    assert!(!patches.is_empty(), "{diff}");
    for p in patches {
        assert_eq!(p["block"], "netgame_flags", "{diff}");
        assert_eq!(p["fields"], json!(["position"]), "{diff}");
    }
    let out = t.path("patched.map");
    let saved = c
        .call(
            m::DOC_SAVE_AS,
            json!({"doc": "map", "path": out.display().to_string()}),
        )
        .unwrap();
    assert_eq!(saved["bytes"], fixture::fixture().len());
    // Errors come back as errors, on the same connection.
    let e = c
        .call(m::SCENARIO_GET, json!({"block": "nope", "index": 0}))
        .unwrap_err();
    assert!(
        matches!(e, agent_proto::CallError::Rpc(ref e) if e.code == code::INVALID_PARAMS),
        "{e}"
    );
    drop(c);

    let (a, b) = (fixture::fixture(), std::fs::read(&out).unwrap());
    assert_eq!(a.len(), b.len());
    let flag = fixture::sfile(fixture::FLAGS_AT);
    let changed: Vec<usize> = (0..a.len()).filter(|&i| a[i] != b[i]).collect();
    let body: Vec<usize> = changed.iter().copied().filter(|&i| i >= 0x800).collect();
    let mut want = Vec::new();
    for (k, v) in [4.5f32, -2.0, 8.0].iter().enumerate() {
        let old = 3.0f32.to_le_bytes();
        for (j, (x, y)) in v.to_le_bytes().iter().zip(old).enumerate() {
            if *x != y {
                want.push(flag + 4 * k + j);
            }
        }
    }
    assert_eq!(body, want, "only the position's bytes past the header");
    // In the header, at most the checksum word.
    let header: Vec<usize> = changed.iter().copied().filter(|&i| i < 0x800).collect();
    assert!(
        header.len() <= 4 && header.windows(2).all(|w| w[1] - w[0] < 4),
        "{header:?}"
    );

    // A notification gets no answer; a bad line gets a parse error.
    #[cfg(unix)]
    {
        use std::io::{BufRead, Write};
        let mut s = std::os::unix::net::UnixStream::connect(&sock).unwrap();
        s.write_all(b"{\"method\":\"session.status\"}\nnot json\n")
            .unwrap();
        let mut line = String::new();
        std::io::BufReader::new(s).read_line(&mut line).unwrap();
        let r: agent_proto::Response = serde_json::from_str(&line).unwrap();
        assert_eq!(r.error.unwrap().code, code::PARSE);
    }
    drop(listener);
    assert!(!sock.exists(), "the socket file goes with the listener");
}

#[test]
fn the_socket_is_local_only_and_one_editor_per_path() {
    let t = Temp::new("sock");
    assert!(server::listen(Path::new("tcp://127.0.0.1:4721")).is_err());
    let sock = t.path("one.sock");
    let (_l, _rx) = server::listen(&sock).unwrap();
    let e = server::listen(&sock)
        .err()
        .expect("a second listener is refused");
    assert_eq!(e.kind(), std::io::ErrorKind::AddrInUse);
    let file = t.path("plain");
    std::fs::write(&file, "x").unwrap();
    assert!(
        server::listen(&file).is_err(),
        "a file that isn't a socket stays"
    );
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
}

#[test]
fn a_window_answers_between_frames() {
    // The window's loop: pump drains the queue on the app it draws.
    let t = Temp::new("pump");
    let mut app = app(&t);
    let mut api = Api::new();
    api.window = true;
    let sock = t.path("w.sock");
    let (_l, rx) = server::listen(&sock).unwrap();
    let client = std::thread::spawn(move || {
        let mut c = studio_proto::cli::connect(&sock).unwrap();
        c.call(
            m::SCENARIO_MOVE,
            json!({"block": "netgame_flags", "index": 0, "position": [3, 2, 1]}),
        )
        .unwrap();
        c.call(m::STATUS, Value::Null).unwrap()
    });
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !client.is_finished() {
        api.pump(&mut app, &rx);
        assert!(std::time::Instant::now() < deadline);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let status = client.join().unwrap();
    assert_eq!(status["window"], true);
    assert_eq!(
        flag_position(&app),
        [3.0, 2.0, 1.0],
        "the window's own document changed"
    );
    assert_eq!(
        app.log().last().unwrap().status.text(),
        "moved NetgameFlags 0"
    );
}

/// An app with `hud_edit`'s invented map as the source, open in the HUD
/// tool through the API (no game data).
fn hud_app(t: &Temp) -> (Api, App) {
    let map = t.path("hudtest.map");
    std::fs::write(&map, hud_edit::fixture::build().0).unwrap();
    let mut app = App::new();
    app.set_data_root(t.path("data"));
    app.dispatch(Action::OpenSource(map));
    let mut api = Api::new();
    let list = call(&mut api, &mut app, m::HUD_LIST, json!({"map": "hudtest"}));
    assert_eq!(list["huds"].as_array().unwrap().len(), 2, "{list}");
    assert_eq!(list["current"], "invented\\hud\\player");
    (api, app)
}

/// The fixture's stray bitmap: full-screen offset (40, -48), flagged by
/// the checks.
const STRAY: hud_edit::WidgetKey = hud_edit::WidgetKey {
    kind: hud_edit::WidgetKind::Bitmap,
    index: 2,
};

/// The stray's entry in a hud.read.
fn stray_of(read: &Value) -> Value {
    read["widgets"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["widget"] == "bitmap:2")
        .expect("the stray")
        .clone()
}

#[test]
fn hud_read_check_move_set_render_and_save_through_the_hud_tool() {
    let t = Temp::new("hud");
    let (mut api, mut app) = hud_app(&t);
    let read = call(&mut api, &mut app, m::HUD_READ, Value::Null);
    let stray = stray_of(&read);
    assert_eq!((stray["fields"]["x0"].clone(), stray["fields"]["y0"].clone()), (json!(40), json!(-48)));
    assert!(stray["placed"].is_object(), "{stray}");
    assert!(read["fields"]["bitmap"].as_array().unwrap().iter().any(|f| f["key"] == "anchor"));
    // The checks name the stray.
    let found = call(&mut api, &mut app, m::HUD_CHECK, Value::Null);
    assert!(
        found.as_array().unwrap().iter().any(|f| f["widgets"]
            .as_array()
            .unwrap()
            .contains(&json!({"hud": 0, "widget": "bitmap:2"}))),
        "{found}"
    );
    // A move is the HUD tool's undo step and patch.
    let moved = call(&mut api, &mut app, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 60, "y": -48}));
    assert_eq!(moved["offset"], json!([60, -48]));
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    assert!(app.tool_dirty(crate::ToolKind::Hud));
    let diff = call(&mut api, &mut app, m::DOC_DIFF, json!({"doc": "hud"}));
    assert_eq!(diff["patches"][0]["widget"], "bitmap:2", "{diff}");
    assert_eq!(diff["patches"][0]["fields"], json!(["x0"]));
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc": "hud"}));
    assert_eq!(app.hud.offset(STRAY), Some((40, -48)));
    assert!(!app.tool_dirty(crate::ToolKind::Hud));
    call(&mut api, &mut app, m::DOC_REDO, json!({"doc": "hud"}));
    assert_eq!(app.hud.offset(STRAY), Some((60, -48)));
    // Another mode's offset: the state's mode follows.
    call(&mut api, &mut app, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 21, "y": -24, "mode": "half"}));
    assert_eq!(app.hud.state.mode, hud_edit::Mode::Half);
    let half = call(&mut api, &mut app, m::HUD_READ, Value::Null);
    assert_eq!(half["state"]["mode"], "half");
    assert_eq!(stray_of(&half)["fields"]["x1"], 21, "{half}");
    // Fields by key; string IDs are the Tags tool's; unknown keys say so.
    let set = call(&mut api, &mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "anchor", "value": 3}));
    assert_eq!(set["value"], 3);
    assert_eq!(
        app.hud.session.as_ref().unwrap().hud().unwrap().widget(STRAY).unwrap().anchor(),
        3
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "name", "value": 1}))),
        code::REFUSED
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SET, json!({"widget": "bitmap:2", "field": "nope", "value": 1}))),
        code::INVALID_PARAMS
    );
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_MOVE, json!({"widget": "bitmap:99", "x": 0, "y": 0}))),
        code::INVALID_PARAMS
    );
    // Opening another map would drop the edits: refused without discard.
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_LIST, json!({"map": "hudtest"}))),
        code::UNSAVED
    );
    // The picture, never into the checkout.
    let png = t.path("out/hud.png");
    let r = call(&mut api, &mut app, m::HUD_RENDER, json!({"out": png.display().to_string(), "state": {"mode": "full", "shield": 0.5}}));
    assert_eq!(r["width"], 640);
    assert!(png.exists());
    assert_eq!(app.hud.state.shield, 0.5);
    let inside = Path::new(env!("CARGO_MANIFEST_DIR")).join("hud.png");
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_RENDER, json!({"out": inside.display().to_string()}))),
        code::REFUSED
    );
    assert!(!inside.exists());
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_RENDER, json!({"state": {"shield": 2}}))),
        code::INVALID_PARAMS
    );
    // The save: never over the source; the saved map has the edits.
    assert_eq!(
        code_of(api.call(&mut app, m::HUD_SAVE_AS, json!({"path": t.path("hudtest.map").display().to_string()}))),
        code::REFUSED
    );
    let out = t.path("out/hud.map");
    call(&mut api, &mut app, m::HUD_SAVE_AS, json!({"path": out.display().to_string()}));
    assert!(!app.tool_dirty(crate::ToolKind::Hud));
    // (No scenario in it, so the scenario tool's open logs an error.)
    app.dispatch(Action::OpenSource(out));
    call(&mut api, &mut app, m::HUD_LIST, json!({"map": "hud"}));
    let back = call(&mut api, &mut app, m::HUD_READ, json!({"state": {"mode": "full"}}));
    assert_eq!(stray_of(&back)["fields"]["x0"], 60);
    assert_eq!(stray_of(&back)["fields"]["anchor"], 3);
}

#[test]
fn the_hud_window_and_the_api_make_identical_documents() {
    let t = Temp::new("hud-ui");
    let (_, mut ui) = hud_app(&t);
    // The window's drag: three moves merged, then a field.
    for (i, x) in [41, 45, 60].into_iter().enumerate() {
        ui.dispatch(Action::HudMove(STRAY, (x, -48), i > 0));
    }
    ui.dispatch(Action::HudSet(STRAY, "anchor".into(), hud_edit::Value::Int(3), false));
    let t2 = Temp::new("hud-api");
    let (mut api, mut agent) = hud_app(&t2);
    call(&mut api, &mut agent, m::HUD_MOVE, json!({"widget": "bitmap:2", "x": 60, "y": -48}));
    call(&mut api, &mut agent, m::HUD_SET, json!({"widget": "bitmap:2", "field": "anchor", "value": 3}));
    let patches = |a: &App| a.hud.session.as_ref().unwrap().patches();
    assert_eq!(patches(&ui), patches(&agent));
    assert_eq!(
        ui.hud.session.as_ref().unwrap().history().count(),
        agent.hud.session.as_ref().unwrap().history().count()
    );
}

/// The viewport gizmo's drag (its actions, as `viewport.rs` dispatches
/// them) and `scenario.move` make the same map, one undo step each; the
/// playlist text fields' edits (`PlaylistEdit`, as `ui/playlists.rs`
/// applies them) and `playlist.set` the same playlists.
#[test]
fn the_gizmo_and_the_text_fields_go_through_the_api_commands() {
    let t = Temp::new("gizmo");
    let mut ui = app(&t);
    ui.dispatch(Action::Select(Some((BlockKind::NetgameFlags, 0))));
    let ypr = ui.session.selected_transform().unwrap().ypr();
    for position in [[2.0, 2.0, 2.0], [3.0, 4.0, 5.0], [4.0, 5.0, 6.0]] {
        ui.dispatch(Action::GestureSet { position, ypr });
    }
    assert!(ui.session.in_gesture());
    ui.dispatch(Action::EndGesture);
    assert!(!ui.session.in_gesture());
    let t2 = Temp::new("gizmo-api");
    let mut agent = app(&t2);
    let mut api = Api::new();
    call(
        &mut api,
        &mut agent,
        m::SCENARIO_MOVE,
        json!({"block": "netgame_flags", "index": 0, "position": [4, 5, 6]}),
    );
    assert_eq!(flag_position(&ui), [4.0, 5.0, 6.0]);
    assert_eq!(
        ui.session.scenario().unwrap().changes(),
        agent.session.scenario().unwrap().changes()
    );
    // The whole drag is one undo step.
    ui.dispatch(Action::Undo);
    call(&mut api, &mut agent, m::DOC_UNDO, json!({"doc": "map"}));
    assert_eq!(flag_position(&ui), flag_position(&agent));
    assert!(!ui.session.is_dirty());

    // The title field: each keystroke's text as a PlaylistEdit, merged
    // under the field's key, then sealed.
    let hop = |t: &Temp| t.path("multiplayer_hopper.dat");
    ui.dispatch(Action::OpenHopper(hop(&t)));
    for text in ["T", "Te", "Team"] {
        let e = PlaylistEdit::Title {
            playlist: 2,
            text: text.into(),
        };
        ui.hoppers.edit_with(|hs, _| (e.apply(hs).unwrap(), Some(7)));
    }
    ui.hoppers.seal();
    let long = "x".repeat(200);
    ui.hoppers.edit(|hs| {
        PlaylistEdit::Description {
            playlist: 2,
            text: long.clone(),
        }
        .apply(hs)
        .unwrap()
    });
    call(
        &mut api,
        &mut agent,
        m::DOC_OPEN,
        json!({"playlists": hop(&t2).display().to_string()}),
    );
    call(
        &mut api,
        &mut agent,
        m::PLAYLIST_SET,
        json!({"playlist": 2, "title": "Team", "description": long}),
    );
    assert_eq!(ui.hoppers.hoppers(), agent.hoppers.hoppers());
    // Clipped to the format's room by the edit, not the widget.
    let d = &ui.hoppers.hoppers().unwrap()[2].description;
    assert!(d.len() < 200, "{}", d.len());
}

#[test]
fn placement_chain_uses_the_current_tag_document_and_stops_at_cycles() {
    let t = Temp::new("placement-chain");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    let placement = json!({"block": "weapons", "index": 0});
    let chain = call(&mut api, &mut app, m::TAG_CHAIN, placement.clone());
    let object = &chain["root"];
    assert_eq!(object["group"], "weap");
    let model = &object["children"][0];
    assert_eq!(model["group"], "hlmt");
    assert_eq!(model["via"], "model");
    let render = &model["children"][0];
    assert_eq!(render["group"], "mode");
    assert_eq!(render["via"], "render_model");
    assert_eq!(render["children"][0]["tag"], object["tag"]);
    assert_eq!(render["children"][0]["cycle"], true);
    assert_eq!(render["children"][0]["children"], json!([]));
    assert_eq!(object["children"][1]["via"], "parts[0].item");
    assert_eq!(object["children"][1]["label"], Value::Null);

    let shallow = call(&mut api, &mut app, m::TAG_CHAIN, json!({"tag":"weap", "depth":0}));
    assert_eq!(shallow["root"]["tag"], object["tag"]);
    assert_eq!(shallow["root"]["children"], json!([]));
    let spawn = call(&mut api, &mut app, m::TAG_CHAIN, json!({"block":"starting_locations", "index":0}));
    assert!(spawn["root"].is_null());
    call(&mut api, &mut app, m::TAG_SET, json!({"tag":"hlmt", "path":"render_model", "value":null}));
    let edited = call(&mut api, &mut app, m::TAG_CHAIN, placement);
    assert_eq!(edited["root"]["children"][0]["children"], json!([]));
    assert_eq!(
        code_of(api.call(&mut app, m::TAG_CHAIN, json!({"block":"weapons", "tag":"weap"}))),
        code::INVALID_PARAMS
    );
}

#[test]
fn placement_inspector_and_tag_set_make_identical_documents_and_saves() {
    let window = Temp::new("inspector-tag");
    let socket = Temp::new("inspector-api");
    let mut ui = crate::app::tag_tests::placement_app(&window.0);
    let mut agent = crate::app::tag_tests::placement_app(&socket.0);
    let mut api = Api::new();
    ui.dispatch(Action::Select(Some((BlockKind::Weapons, 0))));
    let scenario = ui.session.scenario().unwrap();
    let root = crate::app::tag_api::placement_root(scenario, BlockKind::Weapons, 0).unwrap().0;
    // The same byte write the speed widget emits at the end of a drag.
    let t = ui.tags.tag(root).unwrap();
    let f = &ui.tags.layout(root).unwrap().fields[0];
    let action = crate::fields::write_action(root, t.address + f.offset, 4.25f32.to_le_bytes().to_vec(), &f.name);
    ui.dispatch(crate::ui::inspector::tag_action(action));
    ui.dispatch(crate::ui::inspector::tag_action(Action::Tag(crate::app::TagAction::Seal)));
    call(&mut api, &mut agent, m::TAG_SET, json!({"tag":"weap", "path":"speed", "value":4.25}));

    assert_eq!(ui.tags.patches(), agent.tags.patches());
    assert!(!ui.tags.patches().is_empty());
    assert_eq!(ui.session.scenario().unwrap().changes(), agent.session.scenario().unwrap().changes());
    let window_save = window.path("edited.map");
    let socket_save = socket.path("edited.map");
    ui.perform(Action::SaveMapAs(window_save.clone()), false).unwrap();
    call(&mut api, &mut agent, m::DOC_SAVE_AS, json!({"doc":"map", "path":socket_save}));
    assert_eq!(std::fs::read(window_save).unwrap(), std::fs::read(socket_save).unwrap());
}

#[test]
fn back_and_forward_restore_the_tags_document_selection() {
    use crate::app::NavAction;

    let t = Temp::new("tag-navigation");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    let weapon = fixture::id(2);
    let model = fixture::id(4);
    for tag in [weapon, model] {
        app.perform(Action::Navigate(NavAction::SelectTag(tag)), false)
            .unwrap();
        assert_eq!(app.tags.selected, Some(tag));
    }

    call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(app.tags.selected, Some(weapon));
    call(&mut api, &mut app, m::NAV_FORWARD, json!({}));
    assert_eq!(app.tags.selected, Some(model));
}

#[test]
fn reference_jump_and_history_restore_the_tag_and_field() {
    let t = Temp::new("inspector-nav");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    let weapon = fixture::id(2);
    let model = fixture::id(4);
    let render = fixture::id(5);
    let pick = json!({"block":"weapons", "index":0});
    let before = call(&mut api, &mut app, m::SCENARIO_SELECT, pick.clone());
    assert_eq!(before["selected"], pick);

    let jump = crate::fields::reference_action(Some(&[weapon]), "", "model", model);
    app.perform(crate::ui::inspector::tag_action(jump), false).unwrap();
    let arrived = call(&mut api, &mut app, m::NAV_STATUS, json!({}));
    assert_eq!(arrived["selected"], pick);
    assert_eq!(arrived["tag"], scenario::tag_id(model));
    assert_eq!(arrived["breadcrumbs"][1]["tag"], scenario::tag_id(weapon));
    assert_eq!(arrived["breadcrumbs"][2]["tag"], scenario::tag_id(model));

    let back = call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(back["tag"], scenario::tag_id(weapon));
    assert_eq!(back["path"], "model");
    assert!(back["can_forward"].as_bool().unwrap());
    assert_eq!(call(&mut api, &mut app, m::NAV_FORWARD, json!({})), arrived);
    assert_eq!(app.session.selected(), Some((BlockKind::Weapons, 0)));

    let to_render = call(&mut api, &mut app, m::NAV_GO, json!({
        "tag": "mode", "path": "example_back_reference"
    }));
    assert_eq!(to_render["tag"], scenario::tag_id(render));
    assert_eq!(to_render["breadcrumbs"].as_array().unwrap().len(), 4);
    let block_field = call(&mut api, &mut app, m::NAV_GO, json!({
        "tag": "weap", "path": "parts[1].value"
    }));
    assert_eq!(block_field["path"], "parts[1].value");
    call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(call(&mut api, &mut app, m::NAV_FORWARD, json!({})), block_field);
    let focus = crate::app::tag_api::field_focus(&app.tags, weapon, "parts[1].value").unwrap();
    assert_eq!(focus.blocks.len(), 1);
    assert_eq!(focus.blocks[0].1, 1);

    call(&mut api, &mut app, m::TAG_SET, json!({"tag":"weap", "path":"speed", "value":8}));
    call(&mut api, &mut app, m::DOC_UNDO, json!({"doc":"tags"}));
    assert_eq!(call(&mut api, &mut app, m::NAV_STATUS, json!({})), block_field);

    assert_eq!(
        code_of(api.call(&mut app, m::NAV_GO, json!({"tag":"weap", "path":"parts[2].value"}))),
        code::INVALID_PARAMS
    );
    assert_eq!(call(&mut api, &mut app, m::NAV_STATUS, json!({})), block_field);
}

#[test]
fn viewport_pick_pushes_one_history_entry() {
    let t = Temp::new("pick-history");
    let mut app = crate::app::tag_tests::placement_app(&t.0);
    let mut api = Api::new();
    call(&mut api, &mut app, m::SCENARIO_SELECT, json!({"tag":"mode"}));
    let before = call(&mut api, &mut app, m::NAV_STATUS, json!({}));
    let ray = scenario_edit::geometry::Ray {
        origin: [0.0, 1.0, 12.0],
        dir: [0.0, 0.0, -1.0],
    };
    let pick = crate::viewport::pick_action(
        &app.session, &ray, false, &app.layers, &Default::default(),
    ).unwrap();
    assert_eq!(pick, Action::Select(Some((BlockKind::Weapons, 0))));
    app.dispatch(pick.clone());
    let after = call(&mut api, &mut app, m::NAV_STATUS, json!({}));
    assert_eq!(after["selected"], json!({"block":"weapons", "index":0}));
    app.dispatch(pick);
    let mut back = call(&mut api, &mut app, m::NAV_BACK, json!({}));
    assert_eq!(back["can_forward"], true);
    back["can_forward"] = json!(false);
    assert_eq!(back, before);
    assert_eq!(call(&mut api, &mut app, m::NAV_FORWARD, json!({})), after);
}

#[cfg(unix)]
mod mirror_presence {
    use super::*;
    use crate::play::{Conn, Play, select::EYE_HEIGHT, testing::fake};
    use agent_proto::{editor, live, sim};
    use std::sync::atomic::Ordering;
    use std::time::{Duration, Instant};

    const WEAPON: u32 = 0x0010_0003;

    fn game(tag: &str, weapon: u32, window: Option<bool>) -> fake::Game {
        let game = fake::Game::start(tag, "example", sim::SessionKind::Local);
        game.edits.lock().unwrap().extra_object = Some(live::Object {
            id: weapon,
            tag: Some(fixture::id(2).0),
            kind: 2,
            placement: Some(0),
            position: [10.0, -1.0, 2.0 + EYE_HEIGHT],
            forward: [1.0, 0.0, 0.0],
            up: [0.0, 0.0, 1.0],
            scale: 1.0,
            ..Default::default()
        });
        if let Some(window) = window {
            game.presentation(window);
        }
        game
    }

    fn connect(api: &mut Api, app: &mut App, play: &mut Play, game: &fake::Game) {
        play.update(app);
        call(api, app, m::PAIR_CONNECT, json!({"socket": game.path}));
        fake::until(
            play,
            app,
            "the local player and the matching map",
            |p, a| {
                p.edits_blocked(a).as_deref() == Some("View mode")
                    && a.pairing.shown.as_ref().is_some_and(|s| {
                        a.pairing.matches_sample(a, s)
                            && !s.stale
                            && !s.players.is_empty()
                            && s.objects.iter().any(|o| o.placement == Some(0))
                    })
            },
        );
    }

    fn marked(params: &Value, style: &str, object: u32) -> bool {
        let handle = format!("{object:08x}");
        params["marks"].as_array().is_some_and(|marks| {
            marks
                .iter()
                .any(|m| m["style"] == style && m["object"].as_str() == Some(handle.as_str()))
        })
    }

    #[test]
    fn mirror_pick_queries_without_selecting_and_aim_select_marks_the_dwell() {
        let t = Temp::new("presence-aim");
        let game = game("presence-aim", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let before = app.session.selected();
        let pick = call(&mut api, &mut app, m::MIRROR_PICK, json!({"from":"aim"}));
        assert_eq!(pick["object"], sim::objects::handle(WEAPON));
        assert_eq!(pick["placement"], json!({"block":"weapons","index":0}));
        assert_eq!(pick["tag"], sim::objects::handle(fixture::id(2).0));
        assert!(pick["distance"].as_f64().unwrap() > 0.);
        // An explicit ray includes the player's unit. Start this one beyond it.
        let ray = json!({"ray":{"origin":[10.,-2.,2. + EYE_HEIGHT], "dir":[0.,1.,0.]}});
        let hit = call(&mut api, &mut app, m::MIRROR_PICK, ray);
        assert_eq!(hit["object"], pick["object"]);
        assert_eq!(hit["placement"], pick["placement"]);
        assert!(hit["distance"].as_f64().unwrap() < pick["distance"].as_f64().unwrap());
        let miss = json!({"ray":{"origin":[99.,99.,99.],"dir":[0.,0.,1.]}});
        assert!(call(&mut api, &mut app, m::MIRROR_PICK, miss).is_null());
        assert_eq!(app.session.selected(), before);

        call(
            &mut api,
            &mut app,
            m::MIRROR_AIM_SELECT,
            json!({"enabled":true}),
        );
        fake::until(
            &mut play,
            &mut app,
            "candidate and selected marks",
            |_, a| {
                a.session.selected() == Some((BlockKind::Weapons, 0))
                    && game
                        .calls(editor::method::MARKS)
                        .iter()
                        .any(|p| marked(p, "candidate", WEAPON))
            },
        );
        let cursor = call(&mut api, &mut app, m::MIRROR_AIM_SELECT, json!({}));
        assert_eq!(cursor["enabled"], true);
        assert_eq!(cursor["candidate"]["object"], pick["object"]);
        fake::until(&mut play, &mut app, "aim indicator", |_, _| {
            game.calls(editor::method::LINK)
                .iter()
                .any(|p| p["mode"] == "view" && p["text"] == "Aim select")
        });
        call(
            &mut api,
            &mut app,
            m::MIRROR_AIM_SELECT,
            json!({"enabled":false}),
        );
        fake::until(
            &mut play,
            &mut app,
            "selection without the candidate",
            |_, _| {
                game.calls(editor::method::MARKS).last().is_some_and(|p| {
                    p["marks"].as_array().is_some_and(|m| m.len() == 1)
                        && marked(p, "selected", WEAPON)
                })
            },
        );
    }

    #[test]
    fn a_placement_issue_opens_its_row_after_picking_a_loose_live_object() {
        use crate::play::select;

        let t = Temp::new("presence-issue-nav");
        let game = game("presence-issue-nav", WEAPON, None);
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let row = json!({"block":"netgame_flags", "index":0});
        call(&mut api, &mut app, m::SCENARIO_MOVE, json!({
            "block":"netgame_flags", "index":0, "position":[1., 2., 3.]
        }));
        game.edits.lock().unwrap().extra_object.as_mut().unwrap().placement = None;
        fake::until(&mut play, &mut app, "loose weapon and pending flag edit", |_, a| {
            select::from_aim(a).is_some_and(|hit| hit.object == WEAPON && hit.placement.is_none())
                && a.pairing.pending.iter().any(|r| r["target"] == row)
        });
        let hit = select::from_aim(&app).unwrap();
        select::apply(&mut app, hit, false);
        assert_eq!(select::selected_object(&app), Some(WEAPON));
        assert_eq!(app.selection(), Some(Selection::Tag(fixture::id(2))));

        let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
        let issue = status["link"]["issues"].as_array().unwrap().iter()
            .find(|issue| issue["target"] == row).unwrap();
        let nav = call(&mut api, &mut app, m::NAV_GO, issue["target"].clone());
        assert_eq!(nav["selected"], row);
        assert_eq!(select::selected_object(&app), None);
    }

    #[test]
    fn placement_marks_renew_in_view_and_clear_on_unpair() {
        let t = Temp::new("presence-lease");
        let game = game("presence-lease", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        let start = Instant::now();
        fake::presence_at(&mut play, &mut app, start);
        let first = game.calls(editor::method::MARKS);
        assert_eq!(first.len(), 1);
        assert!(marked(&first[0], "selected", WEAPON));
        let deadline = start + Duration::from_secs(1);
        for when in [start, start + Duration::from_millis(300), deadline - Duration::from_nanos(1)] {
            fake::presence_at(&mut play, &mut app, when);
            assert_eq!(game.calls(editor::method::MARKS), first);
        }
        fake::presence_at(&mut play, &mut app, deadline);
        assert_eq!(game.calls(editor::method::MARKS), [first[0].clone(), first[0].clone()]);
        // View mode keeps presentation after the edit switch is turned off.
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":true}));
        fake::until(&mut play, &mut app, "edit indicator", |_, _| {
            game.calls(editor::method::LINK)
                .last()
                .is_some_and(|p| p["mode"] == "edit")
        });
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":false}));
        fake::until(&mut play, &mut app, "view indicator", |_, _| {
            game.calls(editor::method::LINK)
                .last()
                .is_some_and(|p| p["mode"] == "view")
        });
        assert!(marked(
            game.calls(editor::method::MARKS).last().unwrap(),
            "selected",
            WEAPON
        ));

        call(&mut api, &mut app, m::MIRROR_CAMERA, json!({"mode":"eyes"}));
        call(
            &mut api,
            &mut app,
            m::MIRROR_AIM_SELECT,
            json!({"enabled":true}),
        );
        call(
            &mut api,
            &mut app,
            m::PAIR_DISCONNECT,
            json!({"leave":"keep"}),
        );
        fake::until(&mut play, &mut app, "release of both leases", |p, _| {
            p.conn == Conn::Off
                && game
                    .calls(editor::method::LINK)
                    .last()
                    .is_some_and(|p| p["clear"] == true)
                && game
                    .calls(editor::method::MARKS)
                    .last()
                    .is_some_and(|p| p["marks"] == json!([]))
        });
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_CAMERA, json!({})),
            json!({"mode":"free","target":null})
        );
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_AIM_SELECT, json!({})),
            json!({"enabled":false,"candidate":null})
        );
    }

    #[test]
    fn a_rejected_clear_retries_at_the_deadline_and_on_close() {
        for close in [false, true] {
            let name = if close { "marks-close" } else { "marks-retry" };
            let t = Temp::new(name);
            let game = game(name, WEAPON, Some(true));
            let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
            connect(&mut api, &mut app, &mut play, &game);
            call(&mut api, &mut app, m::SCENARIO_SELECT, json!({"block":"weapons","index":0}));
            let now = Instant::now();
            fake::presence_at(&mut play, &mut app, now);
            game.edits.lock().unwrap().mark_replies.push_back(Err(
                agent_proto::RpcError::new(agent_proto::code::UNAVAILABLE, "fixture clear failed"),
            ));
            call(&mut api, &mut app, m::SCENARIO_SELECT, json!({}));
            fake::presence_at(&mut play, &mut app, now);
            let failed = game.calls(editor::method::MARKS);
            assert_eq!(failed.len(), 2);
            assert!(marked(&failed[0], "selected", WEAPON));
            assert_eq!(failed[1]["marks"], json!([]));
            assert!(play.link_view(&app).unavailable.iter().any(|s| s.contains("fixture clear failed")));
            let due = now + Duration::from_secs(1);
            fake::presence_at(&mut play, &mut app, due - Duration::from_nanos(1));
            assert_eq!(game.calls(editor::method::MARKS), failed);
            if close {
                play.unpair(&mut app);
                fake::until(&mut play, &mut app, "final clear", |_, _| {
                    game.calls(editor::method::MARKS).len() == 3
                });
            } else {
                fake::presence_at(&mut play, &mut app, due);
                assert!(!play.link_view(&app).unavailable.iter().any(|s| s.contains("fixture clear failed")));
                fake::presence_at(&mut play, &mut app, due + Duration::from_secs(3));
            }
            let sent = game.calls(editor::method::MARKS);
            assert_eq!(sent.len(), 3);
            assert_eq!(sent[2]["marks"], json!([]));
        }
    }

    #[test]
    fn a_held_set_renews_until_drawn_and_reports_refused_indices_independently() {
        let t = Temp::new("marks-camera");
        let game = game("marks-camera", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        app.session.select_many(vec![(BlockKind::Weapons, 0), (BlockKind::NetgameFlags, 0)]);
        // The weapon has both a selected and a candidate mark. The flag has a
        // fixed box which the host still holds if the weapon disappears.
        let start = Instant::now();
        call(&mut api, &mut app, m::MIRROR_AIM_SELECT, json!({"enabled":true}));
        let why = "no camera for the presented frame";
        let refused = json!([
            {"index":0,"why":"object is gone"}, {"index":2,"why":"object is gone"}
        ]);
        game.edits.lock().unwrap().mark_replies.extend([
            Ok(json!({"held":1,"drawn":false,"why":why,"refused":refused})),
            Ok(json!({"held":1,"drawn":true,"refused":refused})),
        ]);
        fake::presence_at(&mut play, &mut app, start);
        let first = game.calls(editor::method::MARKS);
        assert_eq!(first.len(), 1);
        let marks = first[0]["marks"].as_array().unwrap();
        assert_eq!(marks.len(), 3);
        assert_eq!(marks[0]["object"], format!("{WEAPON:08x}"));
        assert!(marks[1]["box"].is_object());
        assert_eq!(marks[2]["style"], "candidate");
        let waiting = format!("editor.marks: {why}");
        let refused_lines = [0, 2].map(|i| format!("editor.marks: mark {i}: object is gone"));
        let view = play.link_view(&app);
        assert!(view.unavailable.contains(&waiting));
        assert!(refused_lines.iter().all(|line| view.unavailable.contains(line)));
        assert!(!app.log().iter().any(|l| l.status.text() == waiting.as_str()));

        fake::presence_at(&mut play, &mut app, start + Duration::from_secs(1));
        let renewed = game.calls(editor::method::MARKS);
        assert_eq!(renewed, [first[0].clone(), first[0].clone()]);
        let view = play.link_view(&app);
        assert!(!view.unavailable.contains(&waiting));
        assert!(refused_lines.iter().all(|line| view.unavailable.contains(line)));
        for reason in &refused_lines {
            assert_eq!(app.log().iter().filter(|l| l.status.text() == reason.as_str()).count(), 1);
        }
    }

    #[test]
    fn mirror_player_faces_the_camera_in_edit_and_refuses_view() {
        let t = Temp::new("presence-player");
        let game = game("presence-player", WEAPON, None);
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let move_here = json!({"to":[1.,2.,3.],"facing":true});
        let refusal = api.call(&mut app, m::MIRROR_PLAYER, move_here.clone()).unwrap_err();
        assert_eq!(refusal.code, code::REFUSED);
        assert_eq!(refusal.message, "View mode");
        let reports = |app: &App| {
            app.log().iter().filter(|line| line.status.text() == "View mode").count()
        };
        play.update(&mut app);
        assert_eq!(reports(&app), 1);
        assert_eq!(
            call(&mut api, &mut app, m::PAIR_STATUS, Value::Null)["link"]["issues"],
            json!([{"kind":"refused", "text":"View mode", "target":{"panel":"pending"}}])
        );
        assert!(app.pairing.pending.is_empty());
        assert_eq!(
            play.toasts(std::time::Instant::now())
                .filter(|(why, problem)| *problem && *why == refusal.message.as_str())
                .count(),
            1
        );
        assert!(game.calls(sim::method::STATE_SET).is_empty());

        let camera = app.session.camera_mut();
        camera.yaw = 0.41;
        camera.pitch = -0.32;
        let forward = camera.forward().map(f64::from);
        let (yaw, pitch) = sim::yaw_pitch(forward).unwrap();
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":true}));
        fake::until(&mut play, &mut app, "Edit", |p, a| {
            p.edits_blocked(a).is_none()
        });
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_PLAYER, move_here),
            json!({"requested":true})
        );
        fake::until(&mut play, &mut app, "teleport with facing", |_, _| {
            !game.calls(sim::method::STATE_SET).is_empty()
        });
        let moves = game.calls(sim::method::STATE_SET);
        assert_eq!(moves.len(), 1);
        assert_eq!(moves[0]["player"], "player.0");
        assert_eq!(moves[0]["pos"], json!([1., 2., 3.]));
        assert!((moves[0]["yaw"].as_f64().unwrap() - yaw).abs() < 1e-4);
        assert!((moves[0]["pitch"].as_f64().unwrap() - pitch).abs() < 1e-4);

        // The last published status is still Edit when the two socket calls arrive.
        // The ordered window requests turn off Edit before the move reaches the game.
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":false}));
        call(
            &mut api,
            &mut app,
            m::MIRROR_PLAYER,
            json!({"to":[9.,8.,7.]}),
        );
        fake::until(
            &mut play,
            &mut app,
            "the queued move refused in View",
            |_, a| reports(a) == 2,
        );
        assert_eq!(game.calls(sim::method::STATE_SET).len(), 1);
    }

    #[test]
    fn absent_presentation_methods_are_named_and_receive_no_requests() {
        let t = Temp::new("presence-methods");
        let game = game("presence-methods", WEAPON, None);
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
        let reasons = status["link"]["unavailable"].as_array().unwrap();
        for method in [editor::method::LINK, editor::method::MARKS] {
            assert!(reasons.iter().any(|r| r.as_str().unwrap().contains(method)));
        }
        // Install recording handlers after the handshake. They are callable on
        // the transport, but the editor has never seen them in session.methods.
        game.presentation(true);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        call(&mut api, &mut app, m::PAIR_EDIT, json!({"enabled":true}));
        fake::until(&mut play, &mut app, "edit switch", |p, a| {
            p.edits_blocked(a).is_none()
        });
        call(
            &mut api,
            &mut app,
            m::MIRROR_PLAYER,
            json!({"to":[7.,8.,9.]}),
        );
        fake::until(&mut play, &mut app, "ordered game request", |_, _| {
            !game.calls(sim::method::STATE_SET).is_empty()
        });
        assert!(game.calls(editor::method::LINK).is_empty());
        assert!(game.calls(editor::method::MARKS).is_empty());
    }

    #[test]
    fn headless_presentation_explains_the_refusal_once() {
        let t = Temp::new("presence-headless");
        let game = game("presence-headless", WEAPON, Some(false));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        fake::until(
            &mut play,
            &mut app,
            "another mark lease on the windowless host",
            |_, _| {
                game.calls(editor::method::MARKS)
                    .iter()
                    .filter(|p| marked(p, "selected", WEAPON))
                    .count()
                    >= 2
            },
        );
        let status = call(&mut api, &mut app, m::PAIR_STATUS, Value::Null);
        let reasons = status["link"]["unavailable"].as_array().unwrap();
        for method in [editor::method::LINK, editor::method::MARKS] {
            let reason = format!("{method}: this host has no window");
            assert!(reasons.contains(&json!(reason)));
            assert_eq!(
                app.log()
                    .iter()
                    .filter(|line| line.status.text().contains(reason.as_str()))
                    .count(),
                1
            );
        }
        assert_eq!(game.calls(editor::method::LINK).len(), 1);
    }

    #[test]
    fn epoch_changes_release_the_shared_camera_and_replace_salted_marks() {
        let t = Temp::new("presence-epoch");
        let game = game("presence-epoch", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &game);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        fake::until(&mut play, &mut app, "old placement", |_, _| {
            game.calls(editor::method::MARKS)
                .last()
                .is_some_and(|p| marked(p, "selected", WEAPON))
        });
        for mode in ["follow", "eyes"] {
            let camera = call(&mut api, &mut app, m::MIRROR_CAMERA, json!({"mode":mode}));
            assert_eq!(camera["mode"], mode);
            assert_eq!(camera["target"], "player.0");
            play.update(&mut app);
        }
        let new_handle = WEAPON + 0x10000;
        {
            let _hold = game.boundary.lock().unwrap();
            game.edits.lock().unwrap().extra_object.as_mut().unwrap().id = new_handle;
            game.bsp.store(1, Ordering::Relaxed);
        }
        fake::until(
            &mut play,
            &mut app,
            "marks from the replacement epoch",
            |_, a| {
                a.pairing.shown.as_ref().is_some_and(|s| s.epoch == 2)
                    && game.calls(editor::method::MARKS).last().is_some_and(|p| {
                        marked(p, "selected", new_handle) && !marked(p, "selected", WEAPON)
                    })
            },
        );
        assert_eq!(
            call(&mut api, &mut app, m::MIRROR_CAMERA, json!({}))["mode"],
            "free"
        );
    }

    #[test]
    fn reconnect_sends_the_selection_from_the_new_stream() {
        let t = Temp::new("presence-retry");
        let first = game("presence-retry", WEAPON, Some(true));
        let (mut api, mut app, mut play) = (Api::new(), app(&t), Play::new());
        connect(&mut api, &mut app, &mut play, &first);
        call(
            &mut api,
            &mut app,
            m::SCENARIO_SELECT,
            json!({"block":"weapons","index":0}),
        );
        fake::until(&mut play, &mut app, "first game's selection", |_, _| {
            first
                .calls(editor::method::MARKS)
                .last()
                .is_some_and(|p| marked(p, "selected", WEAPON))
        });
        drop(first);
        fake::until(&mut play, &mut app, "lost named socket", |p, _| {
            matches!(p.conn, Conn::Reconnecting { .. })
        });
        let new_handle = WEAPON + 0x20000;
        let second = game("presence-retry", new_handle, Some(true));
        fake::until(&mut play, &mut app, "reconnected presentation", |p, _| {
            p.is_paired()
                && !second.calls(editor::method::LINK).is_empty()
                && second
                    .calls(editor::method::MARKS)
                    .iter()
                    .any(|p| marked(p, "selected", new_handle))
        });
        assert!(
            !second
                .calls(editor::method::MARKS)
                .iter()
                .any(|p| marked(p, "selected", WEAPON))
        );
    }

    #[test]
    fn mirror_controls_cross_the_editor_and_game_sockets() {
        let t = Temp::new("presence-wire");
        let (game, mut editor) = paired_socket::Editor::paired(&t, "presence-wire");
        editor.wait("the matching player sample", |_, a| {
            a.pairing
                .shown
                .as_ref()
                .is_some_and(|s| a.pairing.matches_sample(a, s) && !s.players.is_empty())
        });
        assert!(editor.call(m::MIRROR_PICK, json!({"from":"aim"})).is_null());
        assert_eq!(
            editor.call(m::MIRROR_AIM_SELECT, json!({"enabled":true}))["enabled"],
            true
        );
        let camera = editor.call(m::MIRROR_CAMERA, json!({"mode":"eyes","target":"player.0"}));
        assert_eq!(camera, json!({"mode":"eyes","target":"player.0"}));
        editor.call(m::PAIR_EDIT, json!({"enabled":true}));
        editor.wait("Edit", |p, a| p.edits_blocked(a).is_none());
        let moved = editor.call(m::MIRROR_PLAYER, json!({"to":[2.,3.,4.]}));
        assert_eq!(moved, json!({"requested":true}));
        editor.wait("the game's player move", |_, _| {
            !game.calls(sim::method::STATE_SET).is_empty()
        });
        let moves = game.calls(sim::method::STATE_SET);
        assert_eq!(moves[0]["pos"], json!([2., 3., 4.]));
        assert!(moves[0].get("yaw").is_none() && moves[0].get("pitch").is_none());
    }
}

#[cfg(unix)]
mod windowless_serve {
    use super::*;
    use crate::app::tag_tests;
    use crate::play::Play;
    use crate::play::testing::fake::Game;
    use agent_proto::sim::SessionKind;
    use std::sync::mpsc::Receiver;
    use std::time::{Duration, Instant};

    struct Served {
        client: agent_proto::Client,
        rx: Receiver<server::Incoming>,
        api: Api,
        app: App,
        play: Play,
    }

    impl Served {
        /// A socket call answered by `Api::serve_once`, as `h2-editor --serve` runs.
        fn call(&mut self, method: &str, params: Value) -> Value {
            let client = &mut self.client;
            std::thread::scope(|scope| {
                let request = scope.spawn(move || client.call(method, params));
                while !request.is_finished() {
                    assert!(self.api.serve_once(
                        &mut self.app,
                        &mut self.play,
                        &self.rx,
                        Duration::from_millis(2),
                    ));
                }
                request.join().unwrap().unwrap()
            })
        }
    }

    #[test]
    fn a_windowless_serve_pairs_and_answers_from_the_mirror() {
        let t = Temp::new("serve-pair");
        let game = Game::start("serve-pair", "example", SessionKind::Local);
        let (mut app, ..) = tag_tests::app(&t.0);
        app.pairing.auto_pair = false;
        let sock = t.path("serve.sock");
        let (_listener, rx) = server::listen(&sock).unwrap();
        let client = studio_proto::cli::connect(&sock).unwrap();
        client.set_timeout(Some(Duration::from_secs(5))).unwrap();
        let mut play = Play::new();
        // As `serve` starts: the game link has published before the first request.
        play.update(&mut app);
        let mut served = Served {
            client,
            rx,
            api: Api::new(),
            app,
            play,
        };
        assert!(!served.api.window);
        let socket = game.path.display().to_string();
        served.call(m::PAIR_CONNECT, json!({"socket": socket}));
        let end = Instant::now() + Duration::from_secs(25);
        loop {
            let objects = served.call(m::MIRROR_OBJECTS, json!({}));
            if objects["objects"].as_array().is_some_and(|o| !o.is_empty()) {
                break;
            }
            assert!(Instant::now() < end, "no streamed objects: {objects}");
        }
        assert_eq!(
            served.call(m::PAIR_STATUS, Value::Null)["state"],
            "connected"
        );
        let status = served.call(m::MIRROR_STATUS, json!({}));
        assert_eq!(status["stream"]["state"], "streaming");
        assert!(status["store"]["tick"].is_u64());
        let control = served.call(m::MIRROR_CONTROL, json!({"op":"pause"}));
        assert_eq!(control["requested"], true);
    }
}
