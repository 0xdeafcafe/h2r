//! `pair.preview`, `pair.audio` and `pair.result` against an invented game:
//! the agent's real server and executor with invented host methods (no game
//! data), paired as the window publishes a pairing.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use agent::{Access, Agent, GameStatus, Reader, Server};
use agent_proto::{Kind, Object, RpcError, code as game_code};
use serde_json::{Value, json};
use studio_proto::{code, method as m};

use super::run::brightness;
use crate::App;
use crate::api::Api;
use crate::play::pair::Request;

const STICK: u32 = 0xe123_0045;
const STICK_OBJECT: &str = "e0010002";
const SPAWNED: &str = "e0020003";

/// What the invented game was asked, and what it holds.
#[derive(Default)]
struct World {
    calls: Vec<(String, Value)>,
    /// The player's held weapon's tag (8 hex digits).
    held: Option<String>,
    spawned: Option<[f64; 3]>,
    hide_hud: bool,
    /// The frame `render.capture` writes is black.
    black: bool,
}

struct Level(Arc<Mutex<World>>, u64);

impl Reader for Level {
    fn status(&mut self) -> GameStatus {
        GameStatus {
            tick: Some(self.1),
            map: Some("example".into()),
            mode: Some("multiplayer".into()),
            bsp: Some(0),
        }
    }

    fn objects(&mut self) -> Result<Vec<Object>, String> {
        let w = self.0.lock().unwrap();
        let mut out = vec![Object {
            id: "player.0".into(),
            kind: Kind::Player,
            pos: Some([1.0, 2.0, 3.0]),
            forward: Some([0.0, 1.0, 0.0]),
            weapon: w.held.clone(),
            ..Default::default()
        }];
        if let Some(pos) = w.spawned {
            out.push(Object {
                id: format!("scenery.{SPAWNED}"),
                kind: Kind::Scenery,
                pos: Some(pos),
                tag: Some("e4560007".into()),
                ..Default::default()
            });
        }
        Ok(out)
    }
}

struct Game {
    path: PathBuf,
    dir: PathBuf,
    world: Arc<Mutex<World>>,
    agent: Agent,
    _server: Server,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl Game {
    fn start(tag: &str) -> Game {
        let dir = std::env::temp_dir().join(format!("h2e-preview-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = std::env::temp_dir().join(format!("h2e-pv-{tag}-{}.sock", std::process::id()));
        let agent = Agent::new();
        let world: Arc<Mutex<World>> = Arc::default();
        let host = |name: &'static str, access: Access, f: fn(&mut World, &Value, &Path) -> Result<Value, RpcError>| {
            let world = Arc::clone(&world);
            let dir = dir.clone();
            agent.add_method(name, access, move |params| {
                let mut w = world.lock().unwrap();
                w.calls.push((name.to_owned(), params.clone()));
                f(&mut w, &params, &dir)
            });
        };
        host("sim.give", Access::Act, |w, p, _| {
            assert_eq!(p["player"], "player.0");
            w.held = Some(format!("{STICK:08x}"));
            Ok(json!({
                "player": "player.0",
                "weapon": {"tag": format!("{STICK:#010x}"), "group": "weap", "name": "objects\\weapons\\pogo\\pogo_unsc"},
                "object": STICK_OBJECT, "mode": 2, "held": true,
            }))
        });
        host("sim.spawn", Access::Act, |w, p, _| {
            let pos: [f64; 3] = serde_json::from_value(p["pos"].clone()).unwrap();
            w.spawned = Some(pos);
            Ok(json!({"object": SPAWNED, "pos": pos, "tag": {"tag": "0xe4560007", "group": "scen"}}))
        });
        host("render.capture", Access::Read, |w, _, dir| {
            let path = dir.join(format!("frame-{}.png", w.calls.len()));
            write_png(&path, if w.black { 0 } else { 128 });
            Ok(json!({"path": path.display().to_string(), "tick": 200, "present": 190, "scene_hash": null}))
        });
        host("render.scene", Access::Read, |_, _, _| {
            Ok(json!({"sealed": true, "tick": 199, "views": [
                {"key": "game:0:0", "instances": 40, "cut": false},
                {"key": "first_person:0:0", "instances": 3, "cut": false},
            ]}))
        });
        host("pogo.state", Access::Read, |w, _, _| {
            let riding = w.held.is_some();
            Ok(json!({
                "active": true, "option": true, "style": "unsc", "weapons": [STICK, STICK + 1, STICK + 2],
                "riders": if riding { json!([{"player": 0, "local": true, "unit": 1, "weapon": u32::from_str_radix(STICK_OBJECT, 16).unwrap(), "style": "unsc", "phase": "carried"}]) } else { json!([]) },
                "camera": {"on": riding, "drawn": riding, "room": 1.0, "share": 1.0},
                "tricks": [],
            }))
        });
        host("cheat.list", Access::Read, |w, _, _| {
            Ok(json!({"refused": null, "cheats": [{"name": "hide_hud", "kind": "bool", "value": w.hide_hud}]}))
        });
        host("cheat.set", Access::Act, |w, p, _| {
            assert_eq!(p["name"], "hide_hud");
            w.hide_hud = p["value"] == true;
            Ok(json!({}))
        });
        host("audio.voices", Access::Read, |w, _, _| {
            // Louder on every sample: the peak is the last.
            let n = w.calls.iter().filter(|(c, _)| c == "audio.voices").count() as f64;
            Ok(json!({"voices": [
                {"id": 7, "kind": "stream", "applied_gain": 0.1 * n, "volume_mb": 0, "distance_units": 2.0},
                {"id": 9, "kind": "mix_in", "applied_gain": 0.05, "volume_mb": -600, "distance_units": null},
            ]}))
        });
        host("audio.loops", Access::Read, |_, _, _| {
            Ok(json!({"loops": [{"tag_name": "sound\\pogo\\spring", "tag_datum": "0xe7770001", "segment": "loop"}]}))
        });
        host("pogo.test", Access::Act, |_, p, _| Ok(json!({"player": 0, "trick": p["trick"], "name": "Backflip"})));
        host("ui.audit", Access::Read, |_, _, _| {
            Ok(json!({"screen": {"id": 1, "name": "main_menu"}, "active": {"id": 191, "name": "settings"}, "issues": []}))
        });
        let server = Server::listen(&path, agent.clone()).unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let (s, a, w) = (Arc::clone(&stop), agent.clone(), Arc::clone(&world));
        let thread = thread::spawn(move || {
            let mut level = Level(w, 100);
            while !s.load(Ordering::Relaxed) {
                level.1 += 1;
                a.on_tick(&mut level);
                a.between_slices(&mut level, 0);
                thread::sleep(Duration::from_millis(2));
            }
        });
        Game { path, dir, world, agent, _server: server, stop, thread: Some(thread) }
    }

    fn calls(&self, name: &str) -> Vec<Value> {
        let w = self.world.lock().unwrap();
        w.calls.iter().filter(|(n, _)| n == name).map(|(_, v)| v.clone()).collect()
    }

    /// An app paired with this game, as the window publishes it.
    fn app(&self) -> App {
        let mut app = App::new();
        app.pairing.published = Some(json!({
            "state": "connected", "socket": self.path.display().to_string(),
            "live_edits": false, "paused": false, "edit_refusal": null,
        }));
        app
    }
}

impl Drop for Game {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        self.agent.shutdown();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn write_png(path: &Path, grey: u8) {
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut e = png::Encoder::new(file, 8, 4);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    let mut w = e.write_header().unwrap();
    w.write_image_data(&[grey; 8 * 4 * 3]).unwrap();
}

/// Polls `pair.result` until the job is done (or failed).
fn finish(api: &mut Api, app: &mut App, id: u64) -> Value {
    let end = Instant::now() + Duration::from_secs(25);
    loop {
        let r = api.call(app, m::PAIR_RESULT, json!({"id": id})).unwrap();
        if r["state"] == "done" || r["state"] == "failed" {
            return r;
        }
        assert!(Instant::now() < end, "preview {id} never finished: {r}");
        thread::sleep(Duration::from_millis(5));
    }
}

fn warned(report: &Value, part: &str) -> bool {
    report["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|w| w.as_str().unwrap().contains(part))
}

#[test]
fn a_weapon_preview_gives_it_waits_and_reads_what_the_player_sees() {
    let game = Game::start("weapon");
    game.world.lock().unwrap().black = true;
    let mut app = game.app();
    let mut api = Api::new();
    let out = game.dir.join("shots/weapon.png");
    let started = api
        .call(&mut app, m::PAIR_PREVIEW, json!({
            "what": "weapon", "target": "pogo_unsc", "ticks": 5, "out": out.display().to_string(),
        }))
        .unwrap();
    assert_eq!(started["state"], "running");
    let r = finish(&mut api, &mut app, started["id"].as_u64().unwrap());
    assert_eq!(r["state"], "done", "{r}");
    assert_eq!(game.calls("sim.give"), [json!({"player": "player.0", "weapon": "pogo_unsc"})]);
    let weapon = &r["reads"]["weapon"];
    assert_eq!(weapon["held"], true);
    assert_eq!(weapon["tag"], format!("{STICK:08x}"));
    assert_eq!(r["reads"]["camera"]["first_person"]["drawn"], true);
    // The stick is the kit's: its rider and the chase camera are read.
    assert_eq!(r["reads"]["pogo"]["stick"], true);
    assert_eq!(r["reads"]["pogo"]["camera"]["drawn"], true);
    assert_eq!(r["reads"]["pogo"]["rider"]["player"], 0);
    // The frame is copied out, and its blackness is a warning.
    assert_eq!(r["capture"]["path"], out.display().to_string());
    assert!(out.is_file());
    assert_eq!(r["capture"]["black"], true);
    assert!(warned(&r, "black"), "{r}");
    let steps: Vec<&str> = r["steps"].as_array().unwrap().iter().map(|s| s.as_str().unwrap()).collect();
    assert_eq!(
        steps,
        ["session.methods", "sim.give", "session.wait", "world.object", "pogo.state", "render.scene", "render.capture"]
    );
}

#[test]
fn an_object_preview_spawns_it_in_front_of_the_player_facing_them() {
    let game = Game::start("object");
    let mut app = game.app();
    let mut api = Api::new();
    let id = api
        .call(&mut app, m::PAIR_PREVIEW, json!({"what": "object", "target": "scen:crate", "ticks": 2}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let r = finish(&mut api, &mut app, id);
    assert_eq!(r["state"], "done", "{r}");
    // The player stands at [1, 2, 3] facing +y: 3 units on, turned back.
    let spawn = &game.calls("sim.spawn")[0];
    assert_eq!(spawn["pos"], json!([1.0, 5.0, 3.0]));
    assert!((spawn["yaw"].as_f64().unwrap() - 270.0).abs() < 1e-9, "{spawn}");
    assert_eq!(r["reads"]["object"]["id"], format!("scenery.{SPAWNED}"));
    assert_eq!(r["capture"]["black"], false);
    assert!(!warned(&r, "black"));
}

#[test]
fn pending_edits_are_sent_through_the_pairing_before_the_preview_runs() {
    let game = Game::start("send");
    let mut app = game.app();
    let mut api = Api::new();
    app.pairing.pending.push(json!({"at": 64, "tag": 1, "why": "edit waiting for the game"}));
    app.pairing.pending.push(json!({"at": 96, "tag": 1, "refused": true, "why": "read-only"}));
    let started = api
        .call(&mut app, m::PAIR_PREVIEW, json!({"what": "hud", "ticks": 1}))
        .unwrap();
    assert_eq!(started["state"], "sending");
    // The window's own Send, as mirror.apply asks for it.
    assert_eq!(app.pairing.requests, [Request::Apply(crate::play::Apply::Send)]);
    let id = started["id"].as_u64().unwrap();
    assert_eq!(api.call(&mut app, m::PAIR_RESULT, json!({"id": id})).unwrap()["state"], "sending");
    assert!(game.calls("cheat.list").is_empty());
    // The window takes the request and the game takes the waiting edit.
    app.pairing.requests.clear();
    app.pairing.pending.remove(0);
    let r = finish(&mut api, &mut app, id);
    assert_eq!(r["state"], "done", "{r}");
    assert_eq!(r["edits"]["sent"], true);
    assert_eq!(r["edits"]["left"].as_array().unwrap().len(), 1);
    assert!(warned(&r, "1 edits are not in the game"), "{r}");
}

#[test]
fn a_hud_preview_shows_the_hud_and_puts_the_cheat_back() {
    let game = Game::start("hud");
    game.world.lock().unwrap().hide_hud = true;
    let mut app = game.app();
    let mut api = Api::new();
    let id = api
        .call(&mut app, m::PAIR_PREVIEW, json!({"what": "hud", "target": "pogo", "ticks": 1}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let r = finish(&mut api, &mut app, id);
    assert_eq!(r["state"], "done", "{r}");
    assert_eq!(r["reads"]["hud"]["hidden_before"], true);
    assert_eq!(r["reads"]["state"]["active"], true);
    let sets: Vec<Value> = game.calls("cheat.set").iter().map(|c| c["value"].clone()).collect();
    assert_eq!(sets, [json!(false), json!(true)]);
    assert!(game.world.lock().unwrap().hide_hud);
}

#[test]
fn a_ui_preview_reads_the_screen_shown_and_says_when_it_is_not_the_one_asked() {
    let game = Game::start("ui");
    let mut app = game.app();
    let mut api = Api::new();
    for (screen, matches) in [("settings", true), ("theater", false)] {
        let id = api
            .call(&mut app, m::PAIR_PREVIEW, json!({"what": "ui", "target": screen}))
            .unwrap()["id"]
            .as_u64()
            .unwrap();
        let r = finish(&mut api, &mut app, id);
        assert_eq!(r["state"], "done", "{r}");
        assert_eq!(r["reads"]["ui"]["matches"], matches);
        assert_eq!(warned(&r, "open it with ui.press"), !matches, "{r}");
    }
}

#[test]
fn pair_audio_makes_the_call_and_reports_the_loudest_voice() {
    let game = Game::start("audio");
    let mut app = game.app();
    let mut api = Api::new();
    let id = api
        .call(&mut app, m::PAIR_AUDIO, json!({
            "seconds": 0.2, "every_ms": 20, "call": {"method": "pogo.test", "params": {"trick": "backflip"}},
        }))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    let r = finish(&mut api, &mut app, id);
    assert_eq!(r["state"], "done", "{r}");
    assert_eq!(game.calls("pogo.test"), [json!({"trick": "backflip"})]);
    let audio = &r["audio"];
    let samples = audio["samples"].as_u64().unwrap();
    assert!(samples >= 2, "{audio}");
    assert_eq!(audio["voices"], 2);
    assert_eq!(audio["loudest"][0]["id"], 7);
    let peak = audio["peak_gain"].as_f64().unwrap();
    assert!((peak - 0.1 * samples as f64).abs() < 1e-9, "{audio}");
    assert_eq!(audio["loops"][0]["tag_name"], "sound\\pogo\\spring");
    assert_eq!(warned(&r, "above unity gain"), peak > 1.0);
}

#[test]
fn previews_are_refused_without_a_pairing_a_target_or_a_running_game() {
    let game = Game::start("refused");
    let mut api = Api::new();
    let mut unpaired = App::new();
    let refused = |r: crate::api::Reply| r.unwrap_err().code;
    assert_eq!(refused(api.call(&mut unpaired, m::PAIR_PREVIEW, json!({"what": "hud"}))), code::REFUSED);
    assert_eq!(refused(api.call(&mut unpaired, m::PAIR_RESULT, json!({}))), code::REFUSED);
    let mut app = game.app();
    assert_eq!(refused(api.call(&mut app, m::PAIR_PREVIEW, json!({"what": "weapon"}))), code::INVALID_PARAMS);
    assert_eq!(refused(api.call(&mut app, m::PAIR_PREVIEW, json!({"what": "sound"}))), code::INVALID_PARAMS);
    assert_eq!(
        refused(api.call(&mut app, m::PAIR_PREVIEW, json!({"what": "hud", "view": "eyes"}))),
        code::INVALID_PARAMS
    );
    app.pairing.published.as_mut().unwrap()["paused"] = json!(true);
    assert_eq!(refused(api.call(&mut app, m::PAIR_PREVIEW, json!({"what": "hud"}))), code::REFUSED);
    assert_eq!(refused(api.call(&mut app, m::PAIR_AUDIO, json!({}))), code::REFUSED);
    // A game method the game refuses fails the job with its reason.
    app.pairing.published.as_mut().unwrap()["paused"] = json!(false);
    game.agent.add_method("sim.give", Access::Act, |_| {
        Err(RpcError::new(game_code::INVALID_PARAMS, "no weap tag named nothing"))
    });
    let id = api
        .call(&mut app, m::PAIR_PREVIEW, json!({"what": "weapon", "target": "nothing"}))
        .unwrap()["id"]
        .as_u64()
        .unwrap();
    // One job at a time.
    assert_eq!(refused(api.call(&mut app, m::PAIR_PREVIEW, json!({"what": "hud"}))), code::REFUSED);
    let r = finish(&mut api, &mut app, id);
    assert_eq!(r["state"], "failed");
    assert!(r["why"].as_str().unwrap().contains("no weap tag named nothing"), "{r}");
}

#[test]
fn brightness_tells_a_black_frame_from_a_lit_one() {
    let dir = std::env::temp_dir().join(format!("h2e-preview-png-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let (dark, lit) = (dir.join("dark.png"), dir.join("lit.png"));
    write_png(&dark, 3);
    write_png(&lit, 200);
    let d = brightness(&dark).unwrap();
    assert!(d.black());
    assert_eq!((d.width, d.height), (8, 4));
    let l = brightness(&lit).unwrap();
    assert!(!l.black());
    assert!((l.mean_luma - 200.0 / 255.0).abs() < 1e-6);
    let _ = std::fs::remove_dir_all(&dir);
}

mod switch {
    //! The preview switch and **Preview in game**: each document kind asks
    //! the invented game for the calls its `what` lists, over a map of
    //! invented tags.

    use super::*;
    use crate::app::preview_switch::plan_for_tag;
    use crate::app::shell_tests::{self as tabbed, Temp};
    use crate::app::tag_tests::weap_id;
    use crate::app::{Action, Doc};

    fn refused(r: crate::api::Reply) -> i64 {
        r.unwrap_err().code
    }

    fn weap_name(app: &App) -> String {
        app.tags.tag(weap_id()).unwrap().name.clone()
    }

    fn app_with_tags(game: &Game, t: &Temp) -> App {
        let mut app = tabbed::app(&t.0);
        app.pairing.published = game.app().pairing.published;
        app
    }

    fn logged(app: &App, part: &str) -> bool {
        app.log().iter().any(|l| l.status.text().contains(part))
    }

    /// The window's click: the ask, then the API run between frames.
    fn click(api: &mut Api, app: &mut App, doc: Option<Doc>) -> Option<u64> {
        app.preview.last_job = None;
        app.dispatch(Action::PreviewInGame(doc));
        api.run_preview_asks(app);
        app.preview.last_job
    }

    #[test]
    fn a_kind_asks_for_the_preview_its_what_lists() {
        let tag = |g: &[u8; 4], name: &str| plan_for_tag(map::FourCC(*g), name);
        let p = tag(b"weap", "objects\\w\\rifle").unwrap();
        assert_eq!((p.what, p.target.as_deref()), ("weapon", Some("objects\\w\\rifle")));
        for g in [b"vehi", b"bipd", b"scen", b"eqip"] {
            let p = tag(g, "objects\\x").unwrap();
            let want = format!("{}:objects\\x", std::str::from_utf8(g).unwrap());
            assert_eq!((p.what, p.target.as_deref()), ("object", Some(want.as_str())));
        }
        let p = tag(b"wgit", "ui\\screens\\main_menu").unwrap();
        assert_eq!((p.what, p.target.as_deref()), ("ui", Some("main_menu")));
        let p = tag(b"nhdt", "ui\\hud\\a").unwrap();
        assert_eq!((p.what, p.target), ("hud", None));
        assert_eq!(p.params(), json!({"what": "hud"}));
        for g in [b"snd!", b"proj", b"hlmt", b"mode", b"bitm", b"itmc"] {
            assert!(tag(g, "x").is_err(), "{g:?}");
        }
        assert!(tag(b"snd!", "x").unwrap_err().contains("pair.audio"));
    }

    #[test]
    fn the_target_switch_follows_the_pairing_and_names_the_game_call() {
        let game = Game::start("switch");
        let t = Temp::new("pv-switch");
        let mut app = app_with_tags(&game, &t);
        let mut api = Api::new();
        // Nothing open but the map: the map has no game preview.
        let r = api.call(&mut app, m::PREVIEW_TARGET, json!({})).unwrap();
        assert_eq!((r["doc"].as_str(), r["target"].as_str()), (Some("map"), Some("studio")));
        assert!(r["why"].as_str().unwrap().contains("Playtest"), "{r}");

        app.dispatch(Action::OpenDoc(Doc::Tag(weap_id())));
        let name = weap_name(&app);
        let r = api.call(&mut app, m::PREVIEW_TARGET, json!({})).unwrap();
        assert_eq!(r["game"], json!({"what": "weapon", "target": name}));
        let ids: Vec<(&str, bool)> = r["targets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| (t["id"].as_str().unwrap(), t["available"].as_bool().unwrap()))
            .collect();
        // A game someone else started is paired.
        assert_eq!(ids, [("studio", true), ("game", false), ("attached", true)]);
        assert_eq!(refused(api.call(&mut app, m::PREVIEW_TARGET, json!({"target": "game"}))), code::REFUSED);
        assert_eq!(
            refused(api.call(&mut app, m::PREVIEW_TARGET, json!({"target": "moon"}))),
            code::INVALID_PARAMS
        );
        let r = api.call(&mut app, m::PREVIEW_TARGET, json!({"target": "attached"})).unwrap();
        assert_eq!(r["target"], "attached");
        assert_eq!(app.preview_target(Doc::Tag(weap_id())), crate::app::preview_switch::PreviewTarget::Attached);
        // The switch is per document.
        let r = api.call(&mut app, m::PREVIEW_TARGET, json!({"doc": "map"})).unwrap();
        assert_eq!(r["target"], "studio");

        // Unpaired, a game target is refused and studio stays.
        let mut alone = tabbed::app(&Temp::new("pv-alone").0);
        assert_eq!(
            refused(api.call(&mut alone, m::PREVIEW_TARGET, json!({"target": "attached"}))),
            code::REFUSED
        );
    }

    #[test]
    fn preview_in_game_sends_each_kinds_calls() {
        let game = Game::start("click");
        let t = Temp::new("pv-click");
        let mut app = app_with_tags(&game, &t);
        let mut api = Api::new();

        // A weapon tag: the tab in front, Cmd+Enter's action.
        app.dispatch(Action::OpenDoc(Doc::Tag(weap_id())));
        let name = weap_name(&app);
        let id = click(&mut api, &mut app, None).expect("a job started");
        let r = finish(&mut api, &mut app, id);
        assert_eq!((r["what"].as_str(), r["state"].as_str()), (Some("weapon"), Some("done")), "{r}");
        assert_eq!(game.calls("sim.give"), [json!({"player": "player.0", "weapon": name})]);

        // The HUD tool's document.
        let id = click(&mut api, &mut app, Some(Doc::Hud)).expect("a job started");
        let r = finish(&mut api, &mut app, id);
        assert_eq!(r["what"], "hud");
        assert!(!game.calls("cheat.list").is_empty());

        // The screens tool's document: the menu is read, not opened.
        let id = click(&mut api, &mut app, Some(Doc::Ui)).expect("a job started");
        let r = finish(&mut api, &mut app, id);
        assert_eq!(r["what"], "ui");
        assert!(!game.calls("ui.audit").is_empty());

        // An object kind's params are the ones pair.preview takes for it.
        let p = plan_for_tag(map::FourCC(*b"scen"), "scen:crate").unwrap().params();
        let id = api.call(&mut app, m::PAIR_PREVIEW, p).unwrap()["id"].as_u64().unwrap();
        let r = finish(&mut api, &mut app, id);
        assert_eq!(r["what"], "object");
        assert_eq!(game.calls("sim.spawn").len(), 1);
    }

    #[test]
    fn a_kind_with_no_game_preview_and_a_paused_game_refuse_with_their_message() {
        let game = Game::start("click-refused");
        let t = Temp::new("pv-click-refused");
        let mut app = app_with_tags(&game, &t);
        let mut api = Api::new();

        // The map has none: the click is refused at once, nothing runs.
        assert_eq!(click(&mut api, &mut app, None), None);
        assert!(logged(&app, "Playtest"), "{:?}", app.log().last().map(|l| l.status.text()));

        // The game is paused: the built message, and no call made.
        app.dispatch(Action::OpenDoc(Doc::Tag(weap_id())));
        app.pairing.published.as_mut().unwrap()["paused"] = json!(true);
        assert_eq!(click(&mut api, &mut app, None), None);
        assert!(logged(&app, "the paired game is paused: resume it (pair.control) to preview"));
        assert!(game.calls("sim.give").is_empty());

        // Not paired at all.
        let mut alone = tabbed::app(&Temp::new("pv-click-alone").0);
        alone.dispatch(Action::OpenDoc(Doc::Tag(weap_id())));
        assert_eq!(click(&mut api, &mut alone, None), None);
        assert!(logged(&alone, "pair with a game first") || logged(&alone, "previews run in the editor window"));
    }
}
