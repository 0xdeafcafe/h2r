//! Time: the game's speed, a cheat (`cheat.list`, `cheat.set game_speed`).
//! The speeds on offer are the ones the cheat lists. Pause and frame
//! stepping have no agent method yet and join this page when they do.

use super::cheats;
use super::method::{CHEAT_LIST, SIM_SESSION, STATUS};
use super::{at_menu, get, problem};
use crate::model::{Answer, Call, Row, Section};

pub fn section() -> Section {
    Section {
        id: "time".into(),
        title: "Time".into(),
        about: "game speed".into(),
        calls: vec![
            Call::bare(CHEAT_LIST),
            Call::bare(SIM_SESSION),
            Call::bare(STATUS),
        ],
        rows: time_rows,
    }
}

fn time_rows(a: &[Answer]) -> Vec<Row> {
    match get(a, 0) {
        Ok(v) => cheats::rows(&v, &["game_speed"], at_menu(a, 2).as_deref()),
        Err(e) => vec![problem("Game speed", e)],
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::super::fake::{Game, page};

    #[test]
    fn game_speed_is_a_choice_of_what_the_cheat_lists() {
        let mut g = Game::default();
        let rows = page(&mut g, "time");
        assert_eq!(rows.len(), 1);
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
}
