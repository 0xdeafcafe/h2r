//! Game: save and load the machine, restart the map, load a test map and
//! leave for the main menu.
//!
//! Quick save and load use one snapshot named `quick` (`state.save`,
//! `state.load`); Snapshots lists every one of this build (`state.list`).
//! A save or load waits up to three seconds of game time for a moment
//! nothing the snapshot can't carry is live; when none comes the agent
//! answers BUSY with the blockers in its message, which the menu shows as
//! the failure. A load, a restart and a quit are destructive, so each
//! asks twice.
//!
//! Game code (`game.code`) says which game code this start runs: clang's
//! full-speed module, or the slower fast tier while clang builds the
//! full-speed one in the background, how that build is going, and offers
//! the restart into it once it is ready (`game.code.restart`).

use serde_json::{Value, json};

use super::method::{
    GAME_CODE, GAME_CODE_RESTART, GAME_MENU, MAP_RELOAD, MAPS_TEST_LIST, MAPS_TEST_LOAD, SIM_SESSION, STATE_LIST, STATE_LOAD,
    STATE_SAVE, STATUS,
};
use super::{at_menu, get, list, nothing, problem, str_of, writes_blocked};
use crate::model::{Answer, Call, Row, Section, text};

/// The snapshot Quick save and Quick load use.
pub const QUICK: &str = "quick";

pub fn section() -> Section {
    Section {
        id: "game".into(),
        title: "Game".into(),
        about: "quick save and load, restart, test maps, quit to the main menu".into(),
        calls: vec![
            Call::bare(STATE_LIST),
            Call::bare(SIM_SESSION),
            Call::bare(STATUS),
        ],
        rows: game_rows,
    }
}

pub fn pages() -> Vec<Section> {
    vec![
        Section {
            id: "game/snapshots".into(),
            title: "Snapshots".into(),
            about: "every saved snapshot of this build; select one to load it".into(),
            calls: vec![Call::bare(STATE_LIST), Call::bare(SIM_SESSION)],
            rows: snapshot_rows,
        },
        Section {
            id: "game/code".into(),
            title: "Game code".into(),
            about: "full speed or the slower start, and the full-speed build".into(),
            calls: vec![Call::bare(GAME_CODE)],
            rows: code_rows,
        },
        Section {
            id: "game/test-maps".into(),
            title: "Test maps".into(),
            about: "maps built from recipes on your disc; select one to play it".into(),
            calls: vec![Call::bare(MAPS_TEST_LIST)],
            rows: test_map_rows,
        },
    ]
}

fn game_rows(a: &[Answer]) -> Vec<Row> {
    let snaps = get(a, 0);
    // Snapshots are the machine, not the game state: a session that
    // refuses host writes (co-op) would desync on a load or a reload.
    let off = writes_blocked(a, 1);
    let menu = at_menu(a, 2);
    let grey = |row: Row, why: Option<&String>| match why {
        Some(w) => row.disabled(w.clone()),
        None => row,
    };
    let no_game = menu.clone().map(|m| format!("{m}; nothing to save"));
    let have_quick = snaps
        .as_ref()
        .map(|v| snapshots(v).iter().any(|s| str_of(s, "name") == QUICK));

    let mut save = Row::action(
        "Quick save",
        Call::new(STATE_SAVE, json!({"name": QUICK, "replace": true})),
    )
    .on_ok(|_| "saved quick snapshot".into())
    .about("saves the whole machine as \"quick\"; if it can't, it says what is live");
    save = match (&snaps, &no_game) {
        (Err(e), _) => save.disabled(e.clone()),
        (_, Some(w)) => save.disabled(w.clone()),
        _ => save,
    };

    let mut load = Row::action("Quick load", Call::new(STATE_LOAD, json!({"name": QUICK})))
        .confirm()
        .on_ok(|_| "loaded quick snapshot".into())
        .about("goes back to the quick save; press again to confirm");
    load = match (&have_quick, &off) {
        (Err(e), _) => load.disabled(e.clone()),
        (_, Some(w)) => load.disabled(w.clone()),
        (Ok(false), _) => load.disabled("no quick save yet"),
        _ => load,
    };

    let mut restart = Row::action("Restart map", Call::bare(MAP_RELOAD))
        .confirm()
        .on_ok(restart_toast)
        .about("loads the current map again; press again to confirm");
    restart = match (&off, &menu) {
        (Some(w), _) => restart.disabled(w.clone()),
        (_, Some(m)) => restart.disabled(m.clone()),
        _ => restart,
    };

    let quit = grey(
        Row::action("Quit to main menu", Call::bare(GAME_MENU))
            .confirm()
            .on_ok(|_| "leaving for the main menu".into())
            .about("leaves the game; press again to confirm"),
        menu.as_ref(),
    );

    vec![
        save,
        load,
        grey(Row::open("Snapshots", "game/snapshots"), snaps.as_ref().err()),
        restart,
        Row::open("Test maps", "game/test-maps"),
        quit,
        Row::open("Game code", "game/code")
            .about("full speed or the slower start; restart into full speed when it's built"),
    ]
}

fn restart_toast(v: &Value) -> String {
    match v["map"].as_str() {
        Some(m) => format!("restarting {m}"),
        None => "restarting the map".into(),
    }
}

fn snapshots(v: &Value) -> Vec<Value> {
    list(v, "snapshots")
}

fn snapshot_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Snapshots", e)],
    };
    let off = writes_blocked(a, 1);
    let snaps = snapshots(&v);
    if snaps.is_empty() {
        return nothing("No snapshots", "Quick save makes one");
    }
    snaps
        .iter()
        .map(|s| {
            let name = str_of(s, "name");
            let size = s["bytes"].as_u64().map_or(String::new(), |b| {
                format!(", {} MB", (b as f64 / 1_048_576.0).round())
            });
            let map = s["notes"]["map"]
                .as_str()
                .map_or(String::new(), |m| format!("{m}, "));
            let row = Row::action(
                name.clone(),
                Call::new(STATE_LOAD, json!({ "name": name })),
            )
            .confirm()
            .on_ok(|v| format!("loaded {}", text(&v["name"])))
            .about(format!(
                "{map}tick {}{size}; select twice to load",
                text(&s["tick"])
            ));
            match &off {
                Some(w) => row.disabled(w.clone()),
                None => row,
            }
        })
        .collect()
}

fn test_map_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Test maps", e)],
    };
    let maps = list(&v, "maps");
    if maps.is_empty() {
        return nothing("No test maps", "build one with maps.test.create");
    }
    maps.iter()
        .map(|m| {
            let name = str_of(m, "name");
            let base = str_of(m, "base");
            match m["state"].as_str() {
                Some("ready") => Row::action(
                    name.clone(),
                    Call::new(MAPS_TEST_LOAD, json!({ "name": name })),
                )
                .confirm()
                .on_ok(|v| format!("loading {}", text(&v["name"])))
                .about(format!("on {base}; starts a game there, press again to confirm")),
                Some("failed") => Row::info(name, "failed").disabled(
                    m["error"]
                        .as_str()
                        .unwrap_or("the build failed")
                        .to_owned(),
                ),
                _ => Row::info(name, "building").disabled("still being built"),
            }
        })
        .collect()
}

/// Minutes, rounded up, for a build's elapsed and remaining time.
fn minutes(secs: u64) -> u64 {
    secs.div_ceil(60).max(1)
}

fn code_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Game code", e)],
    };
    let full = v["full_speed"].as_bool().unwrap_or(false);
    let running = if full {
        format!("full speed ({})", text(&v["tier"]))
    } else {
        "slower start (fast tier)".to_owned()
    };
    let bg = &v["background"];
    let state = bg["state"].as_str().unwrap_or("none");
    let build = match state {
        "building" => {
            let done = bg["elapsed_s"].as_u64().unwrap_or(0);
            let left = bg["typical_s"].as_u64().unwrap_or(0).saturating_sub(done);
            format!("building, {} min in, about {} min left", minutes(done), minutes(left))
        }
        "ready" => "ready: restart to use it".to_owned(),
        "failed" => "failed: see the log".to_owned(),
        _ => "not needed".to_owned(),
    };
    let mut rows = vec![
        Row::info("Running", running).about(if full {
            "the optimised clang module: the game at full speed"
        } else {
            "the Cranelift fast tier, built in seconds so the game starts; it runs slower"
        }),
        Row::info("Full-speed build", build)
            .about("clang builds the full-speed module in the background; the next start uses it"),
    ];
    if state == "building"
        && let Some(step) = bg["step"].as_str()
    {
        rows.push(Row::info("Last step", step.to_owned()).about("the build log's last line"));
    }
    if let Some(log) = bg["log"].as_str() {
        rows.push(Row::info("Build log", log.to_owned()));
    }
    rows.push(Row::info("Module", text(&v["module"])).about("the game code this start loaded"));
    if let Some(stamp) = v["stamp"].as_str() {
        rows.push(Row::info("Stamp", stamp.to_owned()).about("image and generator the module was built for"));
    }
    let restart = Row::action("Restart at full speed", Call::bare(GAME_CODE_RESTART))
        .confirm()
        .on_ok(|_| "restarting at full speed".into())
        .about("starts the game again on the full-speed module; unsaved progress is lost, press again to confirm");
    rows.push(match v["restart"]["why"].as_str() {
        Some(why) if v["restart"]["ok"] != Value::Bool(true) => restart.disabled(why.to_owned()),
        _ => restart,
    });
    rows
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    fn call_of(r: &Row) -> Call {
        match &r.edit {
            Edit::Action(c) => c.clone(),
            other => panic!("not an action: {other:?}"),
        }
    }

    #[test]
    fn the_page_is_save_load_snapshots_restart_maps_quit() {
        let mut g = Game::default();
        let rows = page(&mut g, "game");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Quick save",
                "Quick load",
                "Snapshots",
                "Restart map",
                "Test maps",
                "Quit to main menu",
                "Game code"
            ]
        );
        assert_eq!(
            call_of(&row(&rows, "Quick save")),
            Call::new("state.save", json!({"name": "quick", "replace": true}))
        );
        assert_eq!(
            call_of(&row(&rows, "Quick load")),
            Call::new("state.load", json!({"name": "quick"}))
        );
        assert_eq!(call_of(&row(&rows, "Restart map")).method, "map.reload");
        assert_eq!(call_of(&row(&rows, "Quit to main menu")).method, "game.menu");
        assert!(rows.iter().all(|r| r.disabled.is_none()));
    }

    #[test]
    fn load_needs_a_quick_save_and_a_session_that_allows_it() {
        let mut g = Game {
            snapshots: vec!["other"],
            ..Game::default()
        };
        let rows = page(&mut g, "game");
        assert_eq!(
            row(&rows, "Quick load").disabled.as_deref(),
            Some("no quick save yet")
        );
        assert!(row(&rows, "Quick save").disabled.is_none());

        let mut g = Game {
            kind: "coop_host",
            ..Game::default()
        };
        let rows = page(&mut g, "game");
        assert!(row(&rows, "Quick load").disabled.as_deref().unwrap().contains("co-op"));
        assert!(row(&rows, "Restart map").disabled.is_some());
        assert!(row(&rows, "Quit to main menu").disabled.is_none(), "leaving is always allowed");
    }

    #[test]
    fn at_the_main_menu_there_is_nothing_to_save_or_leave() {
        let mut g = Game {
            mode: "main_menu",
            ..Game::default()
        };
        let rows = page(&mut g, "game");
        for l in ["Quick save", "Restart map", "Quit to main menu"] {
            assert!(row(&rows, l).disabled.as_deref().unwrap().contains("main menu"), "{l}");
        }
    }

    #[test]
    fn snapshots_load_by_name_and_say_what_is_in_them() {
        let mut g = Game::default();
        let rows = page(&mut g, "game/snapshots");
        assert_eq!(rows.len(), 2);
        let r = row(&rows, "before_the_bridge");
        assert_eq!(
            call_of(&r),
            Call::new("state.load", json!({"name": "before_the_bridge"}))
        );
        assert_eq!(r.about, "coagulation, tick 4000, 50 MB; select twice to load");
        let mut none = Game {
            snapshots: vec![],
            ..Game::default()
        };
        assert_eq!(page(&mut none, "game/snapshots")[0].label, "No snapshots");
    }

    #[test]
    fn test_maps_load_when_ready_and_explain_when_not() {
        let mut g = Game::default();
        let rows = page(&mut g, "game/test-maps");
        assert_eq!(
            call_of(&row(&rows, "bridge-rig")),
            Call::new("maps.test.load", json!({"name": "bridge-rig"}))
        );
        assert_eq!(row(&rows, "half-built").disabled.as_deref(), Some("still being built"));
        assert_eq!(row(&rows, "broken").disabled.as_deref(), Some("no BSP"));
    }

    #[test]
    fn toasts_read_the_results() {
        assert_eq!(restart_toast(&json!({"queued": true, "map": "coagulation"})), "restarting coagulation");
        assert_eq!(restart_toast(&json!({"queued": true})), "restarting the map");
    }

    #[test]
    fn game_code_says_slower_start_build_progress_and_waits_to_restart() {
        let mut g = Game::default();
        let rows = page(&mut g, "game/code");
        assert_eq!(row(&rows, "Running").shown(), "slower start (fast tier)");
        assert_eq!(row(&rows, "Full-speed build").shown(), "building, 4 min in, about 12 min left");
        assert_eq!(row(&rows, "Last step").shown(), "emit: 2.6s, 18662 owners");
        let r = row(&rows, "Restart at full speed");
        assert_eq!(r.disabled.as_deref(), Some("the full-speed version is still building"));
        assert_eq!(call_of(&r).method, "game.code.restart");
        assert!(r.confirm);

        g.code_ready = true;
        let rows = page(&mut g, "game/code");
        assert_eq!(row(&rows, "Full-speed build").shown(), "ready: restart to use it");
        assert!(row(&rows, "Restart at full speed").disabled.is_none());
        assert!(rows.iter().all(|r| r.label != "Last step"));
    }
}
