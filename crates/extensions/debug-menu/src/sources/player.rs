//! Player: the cheats that act on the local player, a weapon to give, a
//! player to teleport to, and a spot to mark and recall.
//!
//! Give weapon lists the loaded map's `weap` tags (`edit.tags`) and gives
//! the pick to the local player (`sim.give`, which replaces the weapons it
//! holds). Teleport lists the other players (`sim.view`) and moves the
//! local player beside the pick (`sim.state.set`). Both are host writes,
//! refused in a co-op session; their rows carry the session's reason.
//! Mark position keeps the local player's position and facing in a slot
//! (`player.mark`, default slot `quick`); Recall position teleports back
//! to it (`player.recall`, a host write like Teleport).

use serde_json::{Value, json};

use super::cheats;
use super::method::{
    CHEAT_LIST, EDIT_TAGS, PLAYER_MARK, PLAYER_RECALL, SIM_GIVE, SIM_SESSION, SIM_STATE_SET,
    SIM_VIEW, STATUS,
};
use super::{
    PlayerInfo, at_menu, get, local_player, nothing, players, problem, tag_entries, tag_toast,
    writes_blocked,
};
use crate::model::{Answer, Call, Row, Section, text};

/// The cheats on this page: these, then Give weapon and Teleport, then
/// Hide HUD.
const BEFORE: [&str; 5] = ["god", "infinite_ammo", "bottomless_clip", "fly", "noclip"];
const AFTER: [&str; 1] = ["hide_hud"];

pub fn section() -> Section {
    Section {
        id: "player".into(),
        title: "Player".into(),
        about: "god mode, ammo, fly, noclip, weapons, teleport, hide the HUD".into(),
        calls: vec![
            Call::bare(CHEAT_LIST),
            Call::bare(SIM_SESSION),
            Call::bare(STATUS),
        ],
        rows: player_rows,
    }
}

pub fn pages() -> Vec<Section> {
    vec![
        Section {
            id: "player/give".into(),
            title: "Give weapon".into(),
            about: "put a weapon in your hands".into(),
            calls: vec![
                Call::new(EDIT_TAGS, json!({"group": "weap", "limit": 1000})),
                Call::new(SIM_VIEW, json!({"roots": ["player"]})),
                Call::bare(SIM_SESSION),
            ],
            rows: give_rows,
        },
        Section {
            id: "player/teleport".into(),
            title: "Teleport".into(),
            about: "move to another player".into(),
            calls: vec![
                Call::new(SIM_VIEW, json!({"roots": ["player"]})),
                Call::bare(SIM_SESSION),
            ],
            rows: teleport_rows,
        },
    ]
}

fn player_rows(a: &[Answer]) -> Vec<Row> {
    let v = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Cheats", e)],
    };
    let menu = at_menu(a, 2);
    let blocked = writes_blocked(a, 1).or_else(|| menu.clone());
    let mut out = cheats::rows(&v, &BEFORE, menu.as_deref());
    let mut give = Row::open("Give weapon", "player/give");
    let mut tele = Row::open("Teleport", "player/teleport");
    if let Some(why) = &blocked {
        give = give.disabled(why.clone());
        tele = tele.disabled(why.clone());
    }
    out.push(give);
    out.push(tele);
    out.push(mark_row(menu.as_deref()));
    out.push(recall_row(blocked.as_deref()));
    out.extend(
        cheats::rows(&v, &AFTER, menu.as_deref())
            .into_iter()
            .filter(|r| r.label != "Cheats are off"),
    );
    let listed: Vec<&str> = BEFORE
        .iter()
        .chain(&AFTER)
        .chain(&["game_speed"])
        .copied()
        .collect();
    out.extend(cheats::others(&v, &listed, menu.as_deref()));
    out
}

/// The local player, or why there is none.
fn me(a: &[Answer], view: usize) -> Result<PlayerInfo, String> {
    let v = get(a, view)?;
    local_player(&v).ok_or_else(|| "no local player in the game".to_owned())
}

/// Mark position: keep the local player's spot and facing in the default
/// slot. Greyed where there is no game (a mark needs a player).
fn mark_row(at_menu: Option<&str>) -> Row {
    let row = Row::action("Mark position", Call::bare(PLAYER_MARK))
        .on_ok(|v| format!("marked {}", text(&v["slot"])))
        .about("saves where you stand and the way you face as slot \"quick\"");
    match at_menu {
        Some(why) => row.disabled(why),
        None => row,
    }
}

/// Recall position: teleport back to the marked spot. A host write, so a
/// co-op session refuses it as Teleport's.
fn recall_row(blocked: Option<&str>) -> Row {
    let row = Row::action("Recall position", Call::bare(PLAYER_RECALL))
        .on_ok(|v| format!("back at {}", text(&v["slot"])))
        .about("teleports back to the marked spot; an empty slot is an error");
    match blocked {
        Some(why) => row.disabled(why),
        None => row,
    }
}

fn give_rows(a: &[Answer]) -> Vec<Row> {
    let tags = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Weapons", e)],
    };
    let entries = tag_entries(&tags);
    if entries.is_empty() {
        return nothing("No weapons", "the loaded map has no weapon tags");
    }
    let player = me(a, 1);
    let off = writes_blocked(a, 2).or_else(|| player.clone().err());
    let who = player.map(|p| p.id).unwrap_or_default();
    let mut out: Vec<Row> = entries
        .into_iter()
        .map(|e| {
            let row = Row::action(
                e.label,
                Call::new(SIM_GIVE, json!({"player": who, "weapon": e.tag})),
            )
            .on_ok(give_toast)
            .about("replaces the weapons you hold");
            match &off {
                Some(why) => row.disabled(why.clone()),
                None => row,
            }
        })
        .collect();
    let shown = out.len() as u64;
    if let Some(total) = tags["total"].as_u64().filter(|t| *t > shown) {
        out.push(Row::info("More not shown", total - shown));
    }
    out
}

fn give_toast(v: &Value) -> String {
    tag_toast("gave", v, "weapon")
}

fn teleport_rows(a: &[Answer]) -> Vec<Row> {
    let view = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem("Players", e)],
    };
    let Some(me) = local_player(&view) else {
        return nothing("No local player", "start a game first");
    };
    let off = writes_blocked(a, 1);
    let others: Vec<_> = players(&view)
        .into_iter()
        .filter(|p| p.id != me.id)
        .collect();
    if others.is_empty() {
        return nothing(
            "No other players",
            "teleport goes to another player in the game",
        );
    }
    others
        .into_iter()
        .map(|p| {
            let row = match p.ahead(1.5, 0.0) {
                Some((pos, yaw)) => Row::action(
                    p.label(),
                    Call::new(
                        SIM_STATE_SET,
                        json!({"player": me.id, "pos": pos, "yaw": yaw, "pitch": 0}),
                    ),
                )
                .on_ok(|_| "teleported".into())
                .about("puts you next to them, facing the way they do"),
                None => Row::info(p.label(), "").disabled("no position (between lives)"),
            };
            match &off {
                Some(why) => row.disabled(why.clone()),
                None => row,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    #[test]
    fn the_page_is_cheats_with_give_and_teleport_between() {
        let mut g = Game::default();
        let rows = page(&mut g, "player");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "God mode",
                "Infinite ammo",
                "Bottomless clip",
                "Fly",
                "Noclip",
                "Give weapon",
                "Teleport",
                "Mark position",
                "Recall position",
                "Hide HUD"
            ]
        );
        assert!(rows.iter().all(|r| r.disabled.is_none()));
        assert_eq!(row(&rows, "Noclip").value, json!(true));
        assert_eq!(
            row(&rows, "Give weapon").edit,
            Edit::Open("player/give".into())
        );
    }

    #[test]
    fn mark_keeps_the_spot_and_recall_is_a_host_write() {
        fn call_of(r: &Row) -> Call {
            match &r.edit {
                Edit::Action(c) => c.clone(),
                other => panic!("not an action: {other:?}"),
            }
        }
        let mut g = Game::default();
        let rows = page(&mut g, "player");
        let mark = row(&rows, "Mark position");
        assert_eq!(
            call_of(&mark),
            Call::new("player.mark", json!({})),
            "the default slot and player"
        );
        let result = g.call(&call_of(&mark)).unwrap();
        let toast = mark.on_ok.map(|f| (f.0)(&result));
        assert_eq!(toast.as_deref(), Some("marked quick"));
        let recall = row(&rows, "Recall position");
        assert_eq!(call_of(&recall).method, "player.recall");
        let result = g.call(&call_of(&recall)).unwrap();
        let toast = recall.on_ok.map(|f| (f.0)(&result));
        assert_eq!(toast.as_deref(), Some("back at quick"));
        // At the main menu there is no player to mark or put back.
        let mut g = Game {
            mode: "main_menu",
            ..Game::default()
        };
        let rows = page(&mut g, "player");
        for l in ["Mark position", "Recall position"] {
            assert!(
                row(&rows, l).disabled.as_deref().unwrap().contains("main menu"),
                "{l}"
            );
        }
    }

    #[test]
    fn cheats_say_so_at_the_main_menu_and_in_a_refusing_session() {
        let mut g = Game {
            mode: "main_menu",
            ..Game::default()
        };
        let rows = page(&mut g, "player");
        assert!(rows.iter().all(|r| {
            r.disabled
                .as_deref()
                .is_some_and(|d| d.contains("main menu"))
        }));

        let mut g = Game {
            kind: "coop_host",
            cheats_refused: Some("a co-op session".into()),
            ..Game::default()
        };
        let rows = page(&mut g, "player");
        assert_eq!(rows[0].label, "Cheats are off");
        assert!(
            rows
                .iter()
                .all(|r| r.disabled.is_some() || r.label == "Mark position"),
            "mark only reads and keeps the spot, so co-op allows it: {rows:?}"
        );
        assert!(
            row(&rows, "Give weapon")
                .disabled
                .as_deref()
                .unwrap()
                .contains("co-op")
        );
        assert!(
            row(&rows, "Recall position")
                .disabled
                .as_deref()
                .unwrap()
                .contains("co-op"),
            "recall's teleport is a host write"
        );
    }

    #[test]
    fn give_weapon_lists_readable_names_and_gives_to_the_local_player() {
        let mut g = Game::default();
        let rows = page(&mut g, "player/give");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Battle Rifle", "Magnum"]);
        let r = row(&rows, "Battle Rifle");
        assert!(r.disabled.is_none());
        let Edit::Action(call) = &r.edit else {
            panic!("an action")
        };
        assert_eq!(
            *call,
            Call::new(
                "sim.give",
                json!({"player": "player.1", "weapon": "0xe1230045"})
            )
        );
        // The tag list was asked for weapons only.
        assert_eq!(
            g.made("edit.tags")[0].params,
            json!({"group": "weap", "limit": 1000})
        );
        // The toast reads the real result's weapon name.
        let result = g.call(call).unwrap();
        assert_eq!(give_toast(&result), "gave Battle Rifle");
    }

    #[test]
    fn give_weapon_is_greyed_with_the_sessions_reason() {
        let mut g = Game {
            kind: "coop_host",
            ..Game::default()
        };
        let rows = page(&mut g, "player/give");
        assert!(rows.iter().all(|r| {
            r.disabled
                .as_deref()
                .is_some_and(|d| d.contains("co-op"))
        }));
    }

    #[test]
    fn teleport_goes_beside_another_player() {
        let mut g = Game::default();
        let rows = page(&mut g, "player/teleport");
        assert_eq!(rows.len(), 1);
        let r = row(&rows, "clanker 1");
        let Edit::Action(call) = &r.edit else {
            panic!("an action")
        };
        assert_eq!(call.method, "sim.state.set");
        assert_eq!(call.params["player"], "player.1");
        // 1.5 units ahead of (40, -70) facing -x.
        assert_eq!(call.params["pos"], json!([38.5, -70.0, 1.7]));
        assert_eq!(call.params["yaw"], json!(180.0));
        let mut alone = Game {
            with_players: false,
            ..Game::default()
        };
        assert_eq!(
            page(&mut alone, "player/teleport")[0].label,
            "No local player"
        );
    }
}
