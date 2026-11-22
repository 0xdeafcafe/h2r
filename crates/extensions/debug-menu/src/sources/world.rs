//! World: make a vehicle or an object a few steps in front of the local
//! player.
//!
//! Each picker lists the loaded map's tags of a group (`edit.tags`) and
//! spawns the pick (`sim.spawn`) ahead of the player, found from
//! `sim.view` (its position and the way it aims), a little above the
//! ground so it settles instead of starting inside the floor. Host writes,
//! refused in a co-op session.
//!
//! Kill all enemies is not here: the only delete is `world.object.delete`,
//! an editor edit that frees the object without telling the AI that owned
//! it, so it can't be offered as a safe cheat on a campaign actor.

use serde_json::json;

use super::method::{EDIT_TAGS, SIM_SESSION, SIM_SPAWN, SIM_VIEW};
use super::{get, local_player, nothing, problem, tag_entries, tag_toast, writes_blocked};
use crate::model::{Answer, Call, Row, Section};

/// World units ahead of the player: a vehicle needs room.
const VEHICLE_AHEAD: f64 = 5.0;
const OBJECT_AHEAD: f64 = 3.0;
/// And above the player's position.
const UP: f64 = 0.5;

/// The object kinds a picker makes: (page id, title, tag group, units
/// ahead).
const KINDS: [(&str, &str, &str, f64); 7] = [
    ("world/vehicles", "Spawn vehicle", "vehi", VEHICLE_AHEAD),
    ("world/weapons", "Weapons", "weap", OBJECT_AHEAD),
    ("world/equipment", "Equipment", "eqip", OBJECT_AHEAD),
    ("world/scenery", "Scenery", "scen", OBJECT_AHEAD),
    ("world/crates", "Crates", "bloc", OBJECT_AHEAD),
    ("world/machines", "Machines", "mach", OBJECT_AHEAD),
    ("world/creatures", "Creatures", "crea", OBJECT_AHEAD),
];

pub fn section() -> Section {
    Section {
        id: "world".into(),
        title: "World".into(),
        about: "spawn vehicles and objects in front of you".into(),
        calls: vec![Call::bare(SIM_SESSION)],
        rows: world_rows,
    }
}

pub fn pages() -> Vec<Section> {
    let mut out = vec![Section {
        id: "world/objects".into(),
        title: "Spawn object".into(),
        about: "weapons, equipment, scenery, crates, machines, creatures".into(),
        calls: vec![Call::bare(SIM_SESSION)],
        rows: object_rows,
    }];
    out.push(picker::<0>());
    out.push(picker::<1>());
    out.push(picker::<2>());
    out.push(picker::<3>());
    out.push(picker::<4>());
    out.push(picker::<5>());
    out.push(picker::<6>());
    out
}

fn picker<const K: usize>() -> Section {
    let (id, title, group, _) = KINDS[K];
    Section {
        id: id.into(),
        title: title.into(),
        about: format!("make one of the map's {group} tags in front of you"),
        calls: vec![
            Call::new(EDIT_TAGS, json!({"group": group, "limit": 1000})),
            Call::new(SIM_VIEW, json!({"roots": ["player"]})),
            Call::bare(SIM_SESSION),
        ],
        rows: spawn_rows::<K>,
    }
}

fn world_rows(a: &[Answer]) -> Vec<Row> {
    let off = writes_blocked(a, 0);
    let mut vehicles = Row::open("Spawn vehicle", KINDS[0].0);
    let mut objects = Row::open("Spawn object", "world/objects");
    if let Some(why) = off {
        vehicles = vehicles.disabled(why.clone());
        objects = objects.disabled(why);
    }
    vec![vehicles, objects]
}

fn object_rows(a: &[Answer]) -> Vec<Row> {
    let off = writes_blocked(a, 0);
    KINDS[1..]
        .iter()
        .map(|(id, title, _, _)| {
            let row = Row::open(*title, id);
            match &off {
                Some(why) => row.disabled(why.clone()),
                None => row,
            }
        })
        .collect()
}

fn spawn_rows<const K: usize>(a: &[Answer]) -> Vec<Row> {
    let (_, title, group, ahead) = KINDS[K];
    let tags = match get(a, 0) {
        Ok(v) => v,
        Err(e) => return vec![problem(title, e)],
    };
    let entries = tag_entries(&tags);
    if entries.is_empty() {
        return nothing("None in this map", &format!("the loaded map has no {group} tags"));
    }
    // Where to put it: in front of the player as of this listing (the menu
    // lists again every second, so it follows).
    let spot = match get(a, 1) {
        Ok(v) => local_player(&v)
            .ok_or_else(|| "no local player in the game".to_owned())
            .and_then(|p| {
                p.ahead(ahead, UP)
                    .ok_or_else(|| "the player has no position (between lives)".to_owned())
            }),
        Err(e) => Err(e),
    };
    let off = writes_blocked(a, 2).or_else(|| spot.clone().err());
    let mut out: Vec<Row> = entries
        .into_iter()
        .map(|e| {
            let (pos, yaw) = spot.clone().unwrap_or(([0.0; 3], 0.0));
            let row = Row::action(
                e.label,
                Call::new(SIM_SPAWN, json!({"tag": e.tag, "pos": pos, "yaw": yaw})),
            )
            .on_ok(|v| tag_toast("spawned", v, "tag"))
            .about("makes it in front of you");
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

#[cfg(test)]
mod tests {
    use super::super::fake::{Game, page, row};
    use super::*;
    use crate::model::Edit;

    #[test]
    fn the_page_opens_the_pickers() {
        let mut g = Game::default();
        let rows = page(&mut g, "world");
        assert_eq!(rows[0].edit, Edit::Open("world/vehicles".into()));
        assert_eq!(rows[1].edit, Edit::Open("world/objects".into()));
        let objs = page(&mut g, "world/objects");
        let labels: Vec<_> = objs.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(
            labels,
            ["Weapons", "Equipment", "Scenery", "Crates", "Machines", "Creatures"]
        );
    }

    #[test]
    fn a_vehicle_is_made_ahead_of_the_player_facing_the_same_way() {
        let mut g = Game::default();
        let rows = page(&mut g, "world/vehicles");
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Scorpion", "Warthog"]);
        let Edit::Action(call) = &row(&rows, "Warthog").edit else {
            panic!("an action")
        };
        assert_eq!(call.method, "sim.spawn");
        // Player at (36.7, -80.8, 1.7) aiming +y: 5 ahead, half a unit up.
        assert_eq!(call.params["tag"], "0xe2000001");
        let pos: Vec<f64> = call.params["pos"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c.as_f64().unwrap())
            .collect();
        for (got, want) in pos.iter().zip([36.7, -75.8, 2.2]) {
            assert!((got - want).abs() < 1e-9, "{pos:?}");
        }
        assert_eq!(call.params["yaw"], json!(90.0));
        let result = g.call(call).unwrap();
        let toast = tag_toast("spawned", &result, "tag");
        assert_eq!(toast, "spawned Warthog");
    }

    #[test]
    fn objects_are_greyed_in_co_op_and_with_no_player() {
        let mut g = Game {
            kind: "coop_host",
            ..Game::default()
        };
        assert!(page(&mut g, "world/scenery").iter().all(|r| r.disabled.is_some()));
        assert!(page(&mut g, "world")[0].disabled.is_some());
        let mut g = Game {
            with_players: false,
            ..Game::default()
        };
        let rows = page(&mut g, "world/scenery");
        assert_eq!(
            row(&rows, "Barrel").disabled.as_deref(),
            Some("no local player in the game")
        );
    }
}
