//! The game's cheats as `cheat.list` gives them, shared by the Player page
//! (god, ammo, fly, noclip, HUD) and the Time page (game speed).
//!
//! When every cheat is refused (a lockstep run, Xbox Live) the answer says
//! why and every row is greyed with it; a cheat refused on its own (in
//! co-op, those that change the simulation) carries its own reason and
//! only its row greys. At the main menu nothing acts, so the rows say so
//! instead of looking accepted.

use serde_json::{Value, json};

use super::method::CHEAT_SET;
use super::{humanize, list, str_of};
use crate::model::{Row, Setter};

/// The cheat rows named in `names`, in that order. `at_menu` is the reason
/// cheats do nothing now, when there is one.
pub(super) fn rows(v: &Value, names: &[&str], at_menu: Option<&str>) -> Vec<Row> {
    let refused = v["refused"].as_str().map(str::to_owned);
    let cheats = list(v, "cheats");
    let mut out = Vec::new();
    if let Some(why) = &refused {
        out.push(Row::info("Cheats are off", "").disabled(why.clone()));
    }
    for name in names {
        if let Some(c) = cheats.iter().find(|c| c["name"] == *name) {
            out.push(row(c, refused.as_deref(), at_menu));
        }
    }
    out
}

/// The cheats `names` does not list, for a cheat added later.
pub(super) fn others(v: &Value, listed: &[&str], at_menu: Option<&str>) -> Vec<Row> {
    let refused = v["refused"].as_str().map(str::to_owned);
    list(v, "cheats")
        .iter()
        .filter(|c| !listed.contains(&c["name"].as_str().unwrap_or("")))
        .map(|c| row(c, refused.as_deref(), at_menu))
        .collect()
}

fn row(c: &Value, all_refused: Option<&str>, at_menu: Option<&str>) -> Row {
    let name = str_of(c, "name");
    let label = if c["label"].is_string() {
        str_of(c, "label")
    } else {
        humanize(&name)
    };
    let set = Setter::new(CHEAT_SET, json!({ "name": name }), "value");
    let row = match str_of(c, "kind").as_str() {
        "choice" => {
            let choices = list(c, "choices")
                .iter()
                .map(|x| (x["value"].clone(), str_of(x, "label")))
                .collect();
            Row::choice(label, c["value"].clone(), choices, set)
        }
        _ => Row::toggle(label, c["value"].as_bool().unwrap_or(false), set),
    }
    .about(str_of(c, "about"));
    let own = c["refused"].as_str();
    match all_refused.or(own).or(at_menu) {
        Some(why) => row.disabled(why),
        None => row,
    }
}

/// What to list for the cheats that are on, from a `cheat.list` answer
/// (the watermark's cheat line): each switch that is on by its label, and
/// a choice that is not at its normal value (game speed other than 1x) by
/// the choice's label (`0.5x`).
pub fn active_cheats(v: &Value) -> Vec<String> {
    list(v, "cheats")
        .iter()
        .filter_map(|c| {
            let label = if c["label"].is_string() {
                str_of(c, "label")
            } else {
                humanize(&str_of(c, "name"))
            };
            match str_of(c, "kind").as_str() {
                "choice" => {
                    let now = c["value"].as_f64()?;
                    if now == 1.0 {
                        return None;
                    }
                    let name = list(c, "choices")
                        .iter()
                        .find(|x| x["value"].as_f64() == Some(now))
                        .map(|x| str_of(x, "label"))
                        .unwrap_or_else(|| format!("{now}x"));
                    Some(name)
                }
                _ => (c["value"].as_bool() == Some(true)).then_some(label),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::model::Call;
    use crate::sources::fake::Game;

    #[test]
    fn active_cheats_are_the_switches_on_and_a_speed_that_is_not_1x() {
        let mut g = Game {
            god: true,
            speed: 0.5,
            ..Game::default()
        };
        let v = g.call(&Call::bare("cheat.list")).unwrap();
        // God mode (set here), the speed, and noclip (on in the fake).
        assert_eq!(active_cheats(&v), ["God mode", "0.5x", "Noclip"]);
        g.speed = 1.0;
        g.god = false;
        let v = g.call(&Call::bare("cheat.list")).unwrap();
        assert_eq!(active_cheats(&v), ["Noclip"]);
        assert!(active_cheats(&json!({"cheats": []})).is_empty());
    }

    #[test]
    fn a_cheat_refused_on_its_own_greys_only_its_row() {
        // A networked game's host: game speed refused, the rest on offer.
        let why = "a networked game: game speed would not reach the guests";
        let v = json!({"refused": null, "cheats": [
            {"name": "god", "label": "God mode", "kind": "bool", "value": false, "about": ""},
            {"name": "game_speed", "label": "Game speed", "kind": "choice", "value": 1.0,
             "refused": why, "choices": [{"value": 1.0, "label": "1x"}, {"value": 2.0, "label": "2x"}]},
        ]});
        let out = rows(&v, &["god", "game_speed"], None);
        let labels: Vec<_> = out.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["God mode", "Game speed"], "no Cheats are off row");
        assert_eq!(out[0].disabled, None);
        assert_eq!(out[1].disabled.as_deref(), Some(why));
    }
}
