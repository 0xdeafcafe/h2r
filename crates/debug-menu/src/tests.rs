//! The menu as pure logic: input sequences in, state and the action log
//! out, over a fake agent whose registries change as the real ones do.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};

use super::*;
use crate::layout::{COLS, ROWS, Tone};

/// A fake game: registries behind the agent methods the menu calls.
struct Game {
    toggles: Vec<Value>,
    gain: f64,
    fov: i64,
    cheats_refused: Option<String>,
    god: bool,
    speed: f64,
    changes: Vec<u64>,
    calls: Vec<Call>,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            toggles: vec![
                json!({"name": "overrides", "on": true, "default": true, "about": "all overrides"}),
                json!({"name": "fog", "on": false, "default": false, "about": "", "restart": "read at map load"}),
            ],
            gain: 1.0,
            fov: 70,
            cheats_refused: None,
            god: false,
            speed: 1.0,
            changes: vec![3, 4],
            calls: Vec::new(),
        }
    }
}

impl Game {
    fn handle(&mut self, c: &Call) -> Result<Value, String> {
        self.calls.push(c.clone());
        let p = &c.params;
        match c.method.as_str() {
            "render.toggle" => match (p["name"].as_str(), p.get("on")) {
                (None, _) => Ok(json!({ "toggles": self.toggles })),
                (Some(n), Some(on)) => {
                    let t = self
                        .toggles
                        .iter_mut()
                        .find(|t| t["name"] == n)
                        .ok_or(format!("no toggle {n:?}"))?;
                    t["on"] = on.clone();
                    Ok(t.clone())
                }
                (Some(_), None) => Err("read one".into()),
            },
            "render.param" => match p.get("value") {
                None => Ok(
                    json!({"params": [{"name": "gain", "value": self.gain, "default": 1.0, "about": "x.wgsl"}]}),
                ),
                Some(v) => {
                    self.gain = v.as_f64().ok_or("value: a number")?;
                    Ok(json!({"name": "gain", "value": self.gain}))
                }
            },
            "settings.list" => Ok(json!({"settings": [
                {"key": "graphics.fov", "label": "Field of View", "kind": "int", "value": self.fov,
                 "steps": [60, 70, 80], "apply": "live", "help": "degrees"},
                {"key": "graphics.lighting", "label": "Lighting", "kind": "choice", "value": 0,
                 "choices": [{"value": 0, "label": "Retail"}, {"value": 1, "label": "E3"}],
                 "apply": "live", "locked": "H2_LIGHTING", "help": ""},
                {"key": "graphics.old", "label": "Old", "kind": "bool", "value": true, "apply": "restart", "help": ""},
            ]})),
            "settings.set" => {
                self.fov = p["value"].as_i64().ok_or("value")?;
                Ok(json!({"changed": true}))
            }
            "render.stats" => {
                Ok(json!({"at": {"tick": 1}, "pipeline": {"frames": 12, "on": true}, "paths": {}}))
            }
            "render.overrides" => Ok(json!({
                "dir": "/tmp/o", "enabled": self.toggles[0]["on"], "overrides": [
                    {"program": "00000000000000aa", "file": "aa.wgsl", "params": [], "state": "applied"},
                    {"program": "00000000000000bb", "file": "bb.wgsl", "params": [], "state": "rejected", "error": "binding 3"},
                ], "problems": []})),
            "render.reload" => Ok(json!({"loaded": []})),
            "edit.changes" => Ok(json!({"changes": self.changes.iter().map(|id| json!({
                "id": id, "group": "weap", "name": "objects\\weapons\\rifle\\battle_rifle", "tag": "0xe1230045",
                "path": "magazines[0].rounds_loaded_maximum", "old": 36, "new": 99, "stale": false, "live": true,
            })).collect::<Vec<_>>()})),
            "edit.revert" => {
                match p["id"].as_u64() {
                    Some(id) => self.changes.retain(|c| *c != id),
                    None => self.changes.clear(),
                }
                Ok(json!({"reverted": []}))
            }
            "cheat.list" => Ok(json!({"refused": self.cheats_refused, "cheats": [
                {"name": "god", "label": "God mode", "kind": "bool", "value": self.god, "about": "no damage"},
                {"name": "speed", "label": "Game speed", "kind": "choice", "value": self.speed,
                 "choices": [{"value": 0.5, "label": "0.5x"}, {"value": 1.0, "label": "1x"}, {"value": 2.0, "label": "2x"}]},
            ]})),
            "cheat.set" => {
                if let Some(why) = &self.cheats_refused {
                    return Err(why.clone());
                }
                match p["name"].as_str() {
                    Some("god") => self.god = p["value"].as_bool().ok_or("value")?,
                    Some("speed") => self.speed = p["value"].as_f64().ok_or("value")?,
                    _ => return Err("no cheat".into()),
                }
                Ok(json!({}))
            }
            "perf.overlay" => Ok(json!({"on": true})),
            "profile.counters" => {
                Ok(json!({"fps": 30.0, "p50_ms": 33.3, "phase_ms": {"cpu": 20.0}}))
            }
            "features.list" => {
                Ok(json!({"plan": ["agent   on   answers requests", "", "lockstep off"]}))
            }
            m => Err(format!("no method {m}")),
        }
    }
}

/// Answers not yet received.
type Inbox = Vec<(u64, Result<Value, String>)>;

/// [`Rpc`] over a [`Game`], answering on the next look.
#[derive(Clone, Default)]
struct Fake(Arc<Mutex<(Game, Inbox)>>);

impl Rpc for Fake {
    fn send(&mut self, id: u64, call: &Call) {
        let mut g = self.0.lock().unwrap();
        let r = g.0.handle(call);
        g.1.push((id, r));
    }
    fn recv(&mut self) -> Vec<(u64, Result<Value, String>)> {
        std::mem::take(&mut self.0.lock().unwrap().1)
    }
}

impl Fake {
    fn game<T>(&self, f: impl FnOnce(&mut Game) -> T) -> T {
        f(&mut self.0.lock().unwrap().0)
    }
    /// The writes the menu made (lists left out).
    fn writes(&self) -> Vec<Call> {
        let writes = ["render.reload", "edit.revert", "cheat.set", "settings.set"];
        self.game(|g| {
            g.calls
                .iter()
                .filter(|c| {
                    writes.contains(&c.method.as_str())
                        || c.params.get("on").is_some()
                        || c.params.get("value").is_some()
                })
                .cloned()
                .collect()
        })
    }
}

const T: Duration = Duration::from_millis(16);

/// A menu over a fresh fake game.
fn menu() -> (Shared, Fake) {
    let fake = Fake::default();
    let s = Shared::default();
    s.connect(Box::new(fake.clone()));
    (s, fake)
}

/// Steps the menu: navs, then a pump so answers land.
fn go(s: &Shared, navs: &[Nav]) {
    for n in navs {
        s.key(Key::Nav(*n));
    }
    s.lines();
}

fn rows(s: &Shared) -> Vec<Row> {
    s.lines();
    s.lock().menu.rows()
}

fn row(s: &Shared, label: &str) -> Row {
    rows(s)
        .into_iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| panic!("no row {label}"))
}

/// Moves the cursor to the row labelled `label`.
fn to(s: &Shared, label: &str) {
    let i = rows(s).iter().position(|r| r.label == label).unwrap();
    let at = s.lock().menu.cursor();
    let n = if i > at { Nav::Down } else { Nav::Up };
    for _ in 0..i.abs_diff(at) {
        s.key(Key::Nav(n));
    }
}

fn enter(s: &Shared, section: &str) {
    while s.lock().menu.current().is_some() {
        s.key(Key::Nav(Nav::Back));
    }
    if !s.is_open() {
        s.key(Key::Toggle);
    }
    let title = s
        .lock()
        .menu
        .sections()
        .iter()
        .find(|x| x.id == section)
        .unwrap()
        .title
        .clone();
    to(s, &title);
    go(s, &[Nav::Select]);
}

#[test]
fn the_first_page_lists_the_sections_and_back_closes() {
    let (s, _) = menu();
    assert!(s.lines().is_none(), "closed: nothing to draw");
    s.key(Key::Toggle);
    let titles: Vec<String> = rows(&s).iter().map(|r| r.label.clone()).collect();
    assert_eq!(
        titles,
        [
            "Render",
            "Shader overrides",
            "Tag edits",
            "Cheats",
            "Perf",
            "Features"
        ]
    );
    go(&s, &[Nav::Down, Nav::Down, Nav::Select]);
    assert_eq!(
        s.lock().menu.current().map(|x| x.id.clone()),
        Some("tags".into())
    );
    go(&s, &[Nav::Back, Nav::Up, Nav::Back]);
    assert!(!s.is_open());
    let log: Vec<Action> = s
        .lock()
        .menu
        .take_log()
        .into_iter()
        .filter(|a| !matches!(a, Action::Call(_)))
        .collect();
    assert_eq!(
        log,
        vec![
            Action::Open,
            Action::Move(1),
            Action::Move(2),
            Action::Enter("tags".into()),
            Action::Back,
            Action::Move(1),
            Action::Close,
        ]
    );
    // Reopens where it was.
    s.key(Key::Toggle);
    assert_eq!(s.lock().menu.cursor(), 1);
}

#[test]
fn render_rows_come_from_the_registries_and_change_through_them() {
    let (s, fake) = menu();
    enter(&s, "render");
    let labels: Vec<String> = rows(&s).iter().map(|r| r.label.clone()).collect();
    assert_eq!(
        labels,
        [
            "overrides",
            "fog",
            "gain",
            "Field of View",
            "Lighting",
            "Old",
            "stats.pipeline.frames",
            "stats.pipeline.on"
        ]
    );
    // A toggle: right flips it through render.toggle; the listing after shows it.
    to(&s, "overrides");
    go(&s, &[Nav::Right]);
    assert_eq!(fake.game(|g| g.toggles[0]["on"].clone()), json!(false));
    assert_eq!(row(&s, "overrides").value, json!(false));
    // A toggle that needs a restart is greyed and refused.
    to(&s, "fog");
    go(&s, &[Nav::Right]);
    let fog = row(&s, "fog");
    assert_eq!(
        fog.disabled.as_deref(),
        Some("needs a restart: read at map load")
    );
    assert!(s.lock().menu.log().contains(&Action::Refused {
        label: "fog".into(),
        why: "needs a restart: read at map load".into()
    }));
    // A parameter steps by a tenth of its default.
    to(&s, "gain");
    go(&s, &[Nav::Right, Nav::Right]);
    assert!((fake.game(|g| g.gain) - 1.2).abs() < 1e-9);
    go(&s, &[Nav::Left]);
    assert!((fake.game(|g| g.gain) - 1.1).abs() < 1e-9);
    // A setting with steps cycles them; locked and restart ones are greyed.
    to(&s, "Field of View");
    go(&s, &[Nav::Right]);
    assert_eq!(fake.game(|g| g.fov), 80);
    go(&s, &[Nav::Right]);
    assert_eq!(fake.game(|g| g.fov), 60, "wraps");
    assert_eq!(
        row(&s, "Lighting").disabled.as_deref(),
        Some("set by H2_LIGHTING for this run")
    );
    assert!(
        row(&s, "Old")
            .disabled
            .as_deref()
            .unwrap()
            .starts_with("applies after a restart")
    );
    // render.stats is read-only.
    to(&s, "stats.pipeline.frames");
    let before = fake.writes().len();
    go(&s, &[Nav::Right, Nav::Select]);
    assert_eq!(fake.writes().len(), before, "read-only");
    // Every write went through the agent methods, in order.
    let writes: Vec<String> = fake.writes().iter().map(Call::to_string).collect();
    assert_eq!(
        writes,
        [
            r#"render.toggle {"name":"overrides","on":false}"#,
            r#"render.param {"name":"gain","value":1.1}"#,
            r#"render.param {"name":"gain","value":1.2}"#,
            r#"render.param {"name":"gain","value":1.1}"#,
            r#"settings.set {"key":"graphics.fov","value":80}"#,
            r#"settings.set {"key":"graphics.fov","value":60}"#,
        ]
    );
}

#[test]
fn a_change_made_elsewhere_shows_at_the_next_listing() {
    let (s, fake) = menu();
    enter(&s, "cheats");
    assert_eq!(row(&s, "God mode").value, json!(false));
    // `h2 agent cheat.set` from outside.
    fake.game(|g| g.god = true);
    let now = s.now() + crate::model::REFRESH;
    s.lock().pump(now);
    s.lock().pump(now);
    assert_eq!(row(&s, "God mode").value, json!(true));
}

#[test]
fn shader_overrides_switch_reload_and_show_their_state() {
    let (s, fake) = menu();
    enter(&s, "shaders");
    let r = rows(&s);
    assert_eq!(r[0].label, "all overrides");
    assert_eq!(r[0].value, json!(true));
    assert_eq!(r[1].label, "reload");
    let bad = row(&s, "00000000000000bb");
    assert_eq!(bad.disabled.as_deref(), Some("rejected: binding 3"));
    assert_eq!(row(&s, "00000000000000aa").shown(), "applied");
    go(&s, &[Nav::Select]);
    assert_eq!(fake.game(|g| g.toggles[0]["on"].clone()), json!(false));
    assert_eq!(row(&s, "all overrides").value, json!(false), "relisted");
    go(&s, &[Nav::Down, Nav::Select]);
    assert!(fake.game(|g| g.calls.iter().any(|c| c.method == "render.reload")));
}

#[test]
fn tag_edits_are_read_only_and_revert_one_or_all() {
    let (s, fake) = menu();
    enter(&s, "tags");
    let r = rows(&s);
    assert_eq!(r[0].label, "revert all");
    assert_eq!(
        r[1].label,
        "#3 magazines[0].rounds_loaded_maximum battle_rifle"
    );
    assert!(r[1].about.contains("36 -> 99 (live)"), "{}", r[1].about);
    go(&s, &[Nav::Down, Nav::Right]);
    assert_eq!(
        fake.game(|g| g.changes.clone()),
        vec![3, 4],
        "right does not revert"
    );
    go(&s, &[Nav::Select]);
    assert_eq!(fake.game(|g| g.changes.clone()), vec![4]);
    assert_eq!(rows(&s).len(), 2, "relisted: revert all and #4");
    go(&s, &[Nav::Up, Nav::Select]);
    assert_eq!(rows(&s)[0].label, "no edits");
}

#[test]
fn cheats_change_through_cheat_set_and_are_refused_in_co_op() {
    let (s, fake) = menu();
    enter(&s, "cheats");
    go(&s, &[Nav::Select]);
    assert!(fake.game(|g| g.god));
    to(&s, "Game speed");
    assert_eq!(row(&s, "Game speed").shown(), "1x");
    go(&s, &[Nav::Right]);
    assert_eq!(fake.game(|g| g.speed), 2.0);
    go(&s, &[Nav::Left, Nav::Left]);
    assert_eq!(fake.game(|g| g.speed), 0.5);
    // A co-op session starts: every row greyed with the reason, nothing sent.
    let why = "a co-op session: cheats would desync the players";
    fake.game(|g| g.cheats_refused = Some(why.into()));
    enter(&s, "cheats");
    let r = rows(&s);
    assert_eq!(r[0].label, "cheats are off");
    assert!(r.iter().all(|r| r.disabled.as_deref() == Some(why)));
    let sets = fake.game(|g| g.calls.iter().filter(|c| c.method == "cheat.set").count());
    go(&s, &[Nav::Down, Nav::Right, Nav::Select]);
    assert_eq!(
        fake.game(|g| g.calls.iter().filter(|c| c.method == "cheat.set").count()),
        sets
    );
    let lines = s.lines().unwrap();
    assert!(
        lines
            .iter()
            .any(|l| l.tone == Tone::Greyed && l.text.contains("co-op")),
        "{lines:?}"
    );
}

#[test]
fn perf_and_features_are_rows_too() {
    let (s, fake) = menu();
    enter(&s, "perf");
    assert_eq!(row(&s, "perf overlay").value, json!(true));
    assert_eq!(row(&s, "fps").shown(), "30");
    assert_eq!(row(&s, "cpu ms").shown(), "20");
    go(&s, &[Nav::Select]);
    assert!(fake.game(|g| {
        g.calls
            .iter()
            .any(|c| c.to_string() == r#"perf.overlay {"on":false}"#)
    }));
    enter(&s, "features");
    let r = rows(&s);
    assert_eq!(
        (r[0].label.as_str(), r[0].shown()),
        ("agent", "on   answers requests".to_string())
    );
    assert_eq!(r.len(), 2, "blank lines dropped");
}

#[test]
fn without_an_agent_every_page_says_why() {
    let s = Shared::default();
    s.key(Key::Toggle);
    go(&s, &[Nav::Select]);
    let r = rows(&s);
    assert!(
        r.iter().all(|r| r
            .disabled
            .as_deref()
            .is_some_and(|d| d.contains("no agent"))),
        "{r:?}"
    );
}

#[test]
fn a_players_section_can_be_added_later() {
    fn players(a: &[Answer]) -> Vec<Row> {
        match a.first() {
            Some(Some(Ok(v))) => vec![Row::info("players", v["players"].clone())],
            _ => vec![],
        }
    }
    let (s, fake) = menu();
    s.add_section(Section {
        id: "players".into(),
        title: "Players".into(),
        about: "simulated players (sim.*)".into(),
        calls: vec![Call::bare("sim.session")],
        rows: players,
    });
    enter(&s, "players");
    assert_eq!(s.lock().menu.path(), vec!["Players"]);
    assert!(fake.game(|g| g.calls.iter().any(|c| c.method == "sim.session")));
}

#[test]
fn values_step_and_show() {
    let set = Setter::new("x", json!({}), "v");
    let f = Row::float("f", 0.95, 0.05, (0.0, 1.0), set.clone());
    assert_eq!(f.stepped(1), Some(json!(1.0)));
    assert_eq!(
        Row::float("f", 1.0, 0.05, (0.0, 1.0), set.clone()).stepped(1),
        None,
        "at the bound"
    );
    assert_eq!(f.shown(), "0.95");
    let i = Row::int("i", 3, 2, (0, 4), set.clone());
    assert_eq!(
        (i.stepped(1), i.stepped(-1)),
        (Some(json!(4)), Some(json!(1)))
    );
    let c = Row::choice(
        "c",
        json!(9),
        vec![(json!(1), "one".into()), (json!(2), "two".into())],
        set.clone(),
    );
    assert_eq!(
        c.stepped(1),
        Some(json!(1)),
        "an unknown value starts at the first"
    );
    assert_eq!(c.shown(), "9");
    let c = Row::choice(
        "c",
        json!(1.0),
        vec![(json!(1), "one".into()), (json!(2), "two".into())],
        set,
    );
    assert_eq!((c.shown(), c.stepped(-1)), ("one".into(), Some(json!(2))));
    assert_eq!(model::text(&json!(0.25)), "0.25");
    assert_eq!(model::text(&json!(true)), "on");
}

#[test]
fn the_layout_is_a_fixed_width_block_with_a_scrolling_window() {
    fn many(a: &[Answer]) -> Vec<Row> {
        let n = a.len();
        (0..30 + n - 1)
            .map(|i| {
                let r = Row::toggle(
                    format!("row {i:02}"),
                    i % 2 == 0,
                    Setter::new("t", json!({}), "on"),
                );
                if i == 5 {
                    r.disabled("needs a restart: because")
                } else {
                    r
                }
            })
            .collect()
    }
    let fake = Fake::default();
    let s = Shared::new(Menu::with_sections(vec![Section {
        id: "many".into(),
        title: "Many".into(),
        about: String::new(),
        calls: vec![Call::bare("features.list")],
        rows: many,
    }]));
    s.connect(Box::new(fake));
    s.key(Key::Toggle);
    go(&s, &[Nav::Select]);
    let l = s.lines().unwrap();
    assert!(l.iter().all(|l| l.text.chars().count() <= COLS), "{l:?}");
    assert_eq!(l[0].text, "DEBUG MENU > Many");
    assert_eq!(
        l[2].text,
        format!("> row 00{}< on >", " ".repeat(COLS - 2 - 6 - 6))
    );
    assert_eq!(l[2].tone, Tone::Selected);
    assert_eq!(l[7].tone, Tone::Greyed, "row 05");
    let rows_shown = l.iter().filter(|l| l.text.contains("row ")).count();
    assert_eq!(rows_shown, ROWS);
    assert_eq!(l[2 + ROWS].text, "  ... 16 more");
    // Down to row 20: the window follows, with more above and below.
    for _ in 0..20 {
        s.key(Key::Nav(Nav::Down));
    }
    let l = s.lines().unwrap();
    assert_eq!(l[2].text, "  ... 13 more");
    assert!(
        l.iter()
            .any(|l| l.tone == Tone::Selected && l.text.starts_with("> row 20"))
    );
    // The greyed row's reason shows under the menu when selected.
    for _ in 0..15 {
        s.key(Key::Nav(Nav::Up));
    }
    let l = s.lines().unwrap();
    assert!(
        l.iter()
            .any(|l| l.tone == Tone::Greyed && l.text == "needs a restart: because"),
        "{l:?}"
    );
    assert_eq!(l.last().unwrap().text, layout::fit(layout::CONTROLS, COLS));
    assert_eq!(layout::window(30, 0, 14), (0, 14));
    assert_eq!(layout::window(30, 29, 14), (16, 30));
    assert_eq!(
        layout::wrap("a bb ccc dddd", 6),
        vec!["a bb", "ccc", "dddd"]
    );
}

#[path = "gate_tests.rs"]
mod gate_tests;
