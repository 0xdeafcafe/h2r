//! The pinned header: one line saying where you are, over every page.
//!
//! `coagulation  tick 5412  30 fps  pos 36.7 -80.8 1.7  offline`
//!
//! From `session.status` (map, tick, fps, paused), `sim.view` (the local
//! player's position) and `sim.session` (what kind of session, and whether
//! it refuses writes). The section has a single row whose label is the
//! line; the menu shows it above the pages instead of listing it.

use serde_json::{Value, json};

use super::method::{SIM_SESSION, SIM_VIEW, STATUS};
use super::{get, local_player, str_of};
use crate::model::{Answer, Call, Row, Section};

/// The header's section: pass it to `Menu::set_header`.
pub fn header() -> Section {
    Section {
        id: "header".into(),
        title: "Status".into(),
        about: "map, tick, fps, position, session".into(),
        calls: vec![
            Call::bare(STATUS),
            Call::new(SIM_VIEW, json!({"roots": ["player"]})),
            Call::bare(SIM_SESSION),
        ],
        rows: header_rows,
    }
}

/// How a session kind reads.
fn kind_name(kind: &str) -> &'static str {
    match kind {
        "none" => "menus",
        "local" => "offline",
        "coop_host" => "co-op host",
        "coop_guest" => "co-op guest",
        "mp_host" => "hosting",
        "mp_client" => "client",
        _ => "",
    }
}

fn header_rows(a: &[Answer]) -> Vec<Row> {
    let mut parts: Vec<String> = Vec::new();
    match get(a, 0) {
        Ok(v) => {
            let at = &v["at"];
            if let Some(map) = at["map"].as_str() {
                parts.push(map.to_owned());
            }
            if let Some(t) = at["tick"].as_u64() {
                parts.push(format!("tick {t}"));
            }
            if let Some(f) = v["fps"].as_f64() {
                parts.push(format!("{} fps", f.round()));
            }
            if at["paused"].as_bool() == Some(true) {
                parts.push("paused".into());
            }
        }
        Err(e) => return vec![Row::info(format!("no game: {e}"), "")],
    }
    if let Some(p) = get(a, 1).ok().as_ref().and_then(local_player)
        && let Some(pos) = p.pos
    {
        parts.push(format!("pos {:.1} {:.1} {:.1}", pos[0], pos[1], pos[2]));
    }
    if let Ok(s) = get(a, 2) {
        let name = kind_name(&str_of(&s, "kind"));
        if !name.is_empty() {
            parts.push(name.to_owned());
        }
        if s["writes"].as_bool() == Some(false) {
            parts.push("read-only".into());
        }
    }
    vec![Row::info(parts.join("  "), Value::Null)]
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, rows};
    use super::*;

    #[test]
    fn the_line_says_map_tick_fps_position_and_session() {
        let mut g = Game::default();
        let r = rows(&mut g, &header());
        assert_eq!(r.len(), 1);
        assert_eq!(
            r[0].label,
            "coagulation  tick 5412  30 fps  pos 36.7 -80.8 1.7  offline"
        );
    }

    #[test]
    fn a_session_that_refuses_writes_says_so() {
        let mut g = Game {
            kind: "coop_host",
            ..Game::default()
        };
        let r = rows(&mut g, &header());
        assert!(r[0].label.ends_with("co-op host  read-only"), "{}", r[0].label);
    }

    #[test]
    fn with_no_player_the_position_is_left_out() {
        let mut g = Game {
            with_players: false,
            ..Game::default()
        };
        let r = rows(&mut g, &header());
        assert_eq!(r[0].label, "coagulation  tick 5412  30 fps  offline");
    }

    #[test]
    fn no_answer_reads_as_no_game() {
        let r = (header().rows)(&[None, None, None]);
        assert_eq!(r[0].label, "no game: waiting for the game");
    }
}
