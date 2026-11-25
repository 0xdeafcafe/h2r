//! Time: the game's speed, pausing the window and stepping a tick.
//!
//! Game speed is a cheat (`cheat.list`, `cheat.set game_speed`); the
//! speeds on offer are the ones the cheat lists. Pause is the window's
//! own pause, the one Cmd+P toggles, read and set over `host.pause`
//! (`{}` reads, `{"on": bool}` sets); Step one tick runs one game tick
//! while it holds (`host.step`). A headless game has no window, so both
//! are greyed with the method's refusal.

use serde_json::{Value, json};

use super::cheats;
use super::method::{CHEAT_LIST, HOST_PAUSE, HOST_STEP, SIM_SESSION, STATUS};
use super::{at_menu, get, problem};
use crate::model::{Answer, Call, Row, Section, Setter};

pub fn section() -> Section {
    Section {
        id: "time".into(),
        title: "Time".into(),
        about: "game speed, pause, step a tick".into(),
        calls: vec![
            Call::bare(CHEAT_LIST),
            Call::bare(SIM_SESSION),
            Call::bare(STATUS),
            Call::bare(HOST_PAUSE),
        ],
        rows: time_rows,
    }
}

fn time_rows(a: &[Answer]) -> Vec<Row> {
    let mut out = match get(a, 0) {
        Ok(v) => cheats::rows(&v, &["game_speed"], at_menu(a, 2).as_deref()),
        Err(e) => vec![problem("Game speed", e)],
    };
    out.extend(pause_rows(a));
    out
}

/// Pause and Step one tick, from the `host.pause` answer at 3.
fn pause_rows(a: &[Answer]) -> Vec<Row> {
    let state = get(a, 3);
    let paused = state
        .as_ref()
        .ok()
        .and_then(|v| v["paused"].as_bool())
        .unwrap_or(false);
    let mut pause = Row::toggle("Pause", paused, Setter::new(HOST_PAUSE, json!({}), "on"))
        .about("the window's own pause, the one Cmd+P toggles; the game stops until you resume");
    let mut step = Row::action("Step one tick", Call::new(HOST_STEP, json!({"ticks": 1})))
        .on_ok(step_toast)
        .about("runs one game tick while paused, then holds again");
    match &state {
        Err(e) => {
            pause = pause.disabled(e.clone());
            step = step.disabled(e.clone());
        }
        Ok(_) if !paused => {
            step = step.disabled("not paused: turn Pause on first");
        }
        _ => {}
    }
    vec![pause, step]
}

fn step_toast(v: &Value) -> String {
    match v["stepped"].as_u64() {
        Some(1) => "stepped 1 tick".into(),
        Some(n) => format!("stepped {n} ticks"),
        None => "stepping one tick".into(),
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    #[test]
    fn game_speed_is_a_choice_of_what_the_cheat_lists() {
        let mut g = Game::default();
        let rows = page(&mut g, "time");
        assert_eq!(rows[0].label, "Game speed");
        assert_eq!(rows[0].shown(), "1x");
        assert_eq!(rows[0].stepped(1), Some(json!(2.0)));
        let set = rows[0].set.as_ref().unwrap().call(json!(0.5));
        assert_eq!(set.to_string(), r#"cheat.set {"name":"game_speed","value":0.5}"#);
    }

    #[test]
    fn game_speed_is_refused_with_the_reason() {
        let mut g = Game {
            cheats_refused: Some("a lockstep run".into()),
            ..Game::default()
        };
        let rows = page(&mut g, "time");
        assert_eq!(rows[0].label, "Cheats are off");
        assert_eq!(rows[1].disabled.as_deref(), Some("a lockstep run"));
    }

    #[test]
    fn pause_is_the_windows_pause_and_step_waits_for_it() {
        let mut g = Game::default();
        let rows = page(&mut g, "time");
        let pause = row(&rows, "Pause");
        assert_eq!(pause.value, json!(false));
        assert_eq!(
            pause.set.as_ref().unwrap().call(json!(true)).to_string(),
            r#"host.pause {"on":true}"#
        );
        assert_eq!(
            row(&rows, "Step one tick").disabled.as_deref(),
            Some("not paused: turn Pause on first")
        );
        // Paused: the step runs one tick, and its toast counts them.
        let mut g = Game {
            paused: true,
            ..Game::default()
        };
        let rows = page(&mut g, "time");
        assert_eq!(row(&rows, "Pause").value, json!(true));
        let step = row(&rows, "Step one tick");
        assert!(step.disabled.is_none());
        let Edit::Action(call) = &step.edit else {
            panic!("an action")
        };
        assert_eq!(*call, Call::new("host.step", json!({"ticks": 1})));
        let result = g.call(call).unwrap();
        assert_eq!(step_toast(&result), "stepped 1 tick");
    }

    #[test]
    fn without_a_window_pause_and_step_say_so() {
        // A headless game: `host.pause {}` is refused, both rows carry it.
        let no_window: Answer = Some(Err(
            "no window here: host.pause is the window's pause (Cmd+P)".into(),
        ));
        let rows = time_rows(&[
            Some(Ok(json!({"refused": null, "cheats": []}))),
            Some(Ok(json!({"kind": "local", "writes": true}))),
            Some(Ok(json!({"at": {"mode": "campaign"}}))),
            no_window,
        ]);
        for label in ["Pause", "Step one tick"] {
            assert!(
                row(&rows, label)
                    .disabled
                    .as_deref()
                    .is_some_and(|d| d.contains("no window")),
                "{label}"
            );
        }
    }
}
