//! The menu's sections, each over agent methods (docs/runtime/debug-menu.md
//! "Sections"). A section lists its page with read calls and turns the
//! answers into rows; each row that changes something carries the write
//! call. The first page is organised by what a tester does, not by agent
//! namespace: Player, World, Game, Time, View, Perf, Settings, Dev.
//!
//! A page that is only reached from another (a weapon picker, the Video
//! settings) is a sub-page: its id has a `/` (`player/give`) and a
//! [`Row::open`] on its parent opens it. [`all`] lists every section;
//! [`is_page`] says which belong on the first page.
//!
//! The agent API's own names (`render.*`, `edit.*`, `features.list`,
//! `profile.counters`) are the agent's; the ones the menu added beside
//! them are in [`method`].

use serde_json::Value;

use crate::model::{Answer, Row, Section, text};

mod cheats;
mod dev;
mod game;
mod header;
mod perf;
mod player;
mod settings;
mod time;
mod view;
mod world;

#[cfg(test)]
mod fake;

pub use cheats::active_cheats;
pub use header::header;
pub use settings::setting_row;

/// Agent methods the game's host answers for the menu (and for
/// `h2 agent`, like every other method), and the agent API's own methods
/// the menu calls.
pub mod method {
    /// `{}`: the cheats, their values, and why they are refused if they
    /// are. `{"name", "value"}` is [`CHEAT_SET`].
    pub const CHEAT_LIST: &str = "cheat.list";
    /// `{"name": "god", "value": true}`.
    pub const CHEAT_SET: &str = "cheat.set";
    /// `{"prefix"?}`: settings (`settings.toml`), how each applies.
    pub const SETTINGS_LIST: &str = "settings.list";
    /// `{"key", "value"}`: saved, and applied as the setting applies.
    pub const SETTINGS_SET: &str = "settings.set";
    /// `{}` reads, `{"on": bool}` sets: the perf overlay (the watermark).
    pub const PERF_OVERLAY: &str = "perf.overlay";
    /// The agent API's methods the menu reads.
    pub const RENDER_TOGGLE: &str = "render.toggle";
    pub const RENDER_PARAM: &str = "render.param";
    pub const RENDER_STATS: &str = "render.stats";
    pub const RENDER_OVERRIDES: &str = "render.overrides";
    pub const RENDER_RELOAD: &str = "render.reload";
    pub const EDIT_CHANGES: &str = "edit.changes";
    pub const EDIT_REVERT: &str = "edit.revert";
    pub const EDIT_TAGS: &str = "edit.tags";
    pub const FEATURES: &str = "features.list";
    pub const COUNTERS: &str = "profile.counters";
    pub const CAPTURE: &str = "profile.capture";
    pub const STATUS: &str = "session.status";
    pub const SIM_SESSION: &str = "sim.session";
    pub const SIM_VIEW: &str = "sim.view";
    pub const SIM_GIVE: &str = "sim.give";
    pub const SIM_SPAWN: &str = "sim.spawn";
    pub const SIM_STATE_SET: &str = "sim.state.set";
    pub const STATE_SAVE: &str = "state.save";
    pub const STATE_LOAD: &str = "state.load";
    pub const STATE_LIST: &str = "state.list";
    pub const MAP_RELOAD: &str = "map.reload";
    pub const MAPS_TEST_LIST: &str = "maps.test.list";
    pub const MAPS_TEST_LOAD: &str = "maps.test.load";
    pub const GAME_MENU: &str = "game.menu";
    /// `{}`: the campaign and multiplayer maps [`GAME_LOAD`] takes.
    pub const GAME_LEVELS: &str = "game.levels";
    /// `{"scenario", "difficulty"?, "variant"?}`: queue a level as the
    /// next game, from the main menu or a running one.
    pub const GAME_LOAD: &str = "game.load";
    /// `{}` reads, `{"on": bool}` sets: the window's pause (Cmd+P's).
    pub const HOST_PAUSE: &str = "host.pause";
    /// `{"ticks"?}`: N game ticks under the window's pause, default 1.
    pub const HOST_STEP: &str = "host.step";
    /// `{"slot"?, "player"?}`: keep a player's position and facing.
    pub const PLAYER_MARK: &str = "player.mark";
    /// `{"slot"?, "player"?}`: put a player back at the kept spot.
    pub const PLAYER_RECALL: &str = "player.recall";
    /// `{"path"?}`: the window's newest frame as a PNG.
    pub const HOST_SCREENSHOT: &str = "host.screenshot";
    /// `{}`: which game code runs (full-speed clang module or the slower
    /// fast tier), its module, and clang's background build.
    pub const GAME_CODE: &str = "game.code";
    /// `{}`: start the game again so it runs the full-speed module.
    pub const GAME_CODE_RESTART: &str = "game.code.restart";
}

/// The settings tables the Video page lists.
pub const RENDER_SETTINGS: &str = "graphics.";

/// Every section: the first page's, in order, then the sub-pages.
pub fn all() -> Vec<Section> {
    let mut out = vec![
        player::section(),
        world::section(),
        game::section(),
        time::section(),
        view::section(),
        perf::section(),
        settings::section(),
        dev::section(),
    ];
    out.extend(player::pages());
    out.extend(world::pages());
    out.extend(game::pages());
    out.extend(view::pages());
    out.extend(settings::pages());
    out.extend(dev::pages());
    out
}

/// Whether section `id` is on the first page (not a sub-page).
pub fn is_page(id: &str) -> bool {
    !id.contains('/')
}

// ---- helpers ----

/// Answer `i`: waiting reads as an error saying so.
pub(crate) fn get(a: &[Answer], i: usize) -> Result<Value, String> {
    match a.get(i) {
        Some(Some(r)) => r.clone(),
        _ => Err("waiting for the game".into()),
    }
}

/// A row saying a listing failed.
pub(crate) fn problem(what: &str, e: String) -> Row {
    Row::info(what, "unavailable").disabled(e)
}

pub(crate) fn list(v: &Value, k: &str) -> Vec<Value> {
    v[k].as_array().cloned().unwrap_or_default()
}

pub(crate) fn str_of(v: &Value, k: &str) -> String {
    match &v[k] {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        other => text(other),
    }
}

/// `[x, y, z]` as three numbers.
pub(crate) fn vec3(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    if a.len() != 3 {
        return None;
    }
    Some([a[0].as_f64()?, a[1].as_f64()?, a[2].as_f64()?])
}

/// Why host writes are refused, from a `sim.session` answer at `i`: its
/// own `why`, or the error when the call failed. `None` when they are
/// allowed.
pub(crate) fn writes_blocked(a: &[Answer], i: usize) -> Option<String> {
    match get(a, i) {
        Ok(v) if v["writes"].as_bool() == Some(false) => Some(
            v["why"]
                .as_str()
                .unwrap_or("this session refuses writes")
                .to_owned(),
        ),
        Ok(_) => None,
        Err(e) => Some(e),
    }
}

/// Why nothing is running, from a `session.status` answer at `i`: `Some`
/// at the main menu. A failed call says nothing (the other calls
/// complain).
pub(crate) fn at_menu(a: &[Answer], i: usize) -> Option<String> {
    let v = get(a, i).ok()?;
    match v["at"]["mode"].as_str() {
        Some("main_menu") => Some("no game is running (this is the main menu)".into()),
        _ => None,
    }
}

/// `infinite_ammo` or `fly-through` as `Infinite ammo`.
pub(crate) fn humanize(s: &str) -> String {
    let t = s.replace(['_', '-', '.'], " ");
    let t = t.trim();
    let mut c = t.chars();
    match c.next() {
        Some(f) => f.to_uppercase().chain(c).collect(),
        None => String::new(),
    }
}

/// A tag's name as a reader would say it: the last path component, in
/// words (`objects\weapons\rifle\battle_rifle\battle_rifle` is `Battle
/// Rifle`).
pub(crate) fn tag_label(name: &str) -> String {
    leaf(name)
        .split(['_', ' '])
        .filter(|w| !w.is_empty())
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map(|f| f.to_uppercase().chain(c).collect::<String>())
                .unwrap_or_default()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// The last path component of a tag name.
pub(crate) fn leaf(name: &str) -> &str {
    name.rsplit(['\\', '/']).next().unwrap_or(name)
}

/// The component before the last.
fn parent(name: &str) -> Option<&str> {
    name.rsplit(['\\', '/']).nth(1)
}

/// A tag from `edit.tags`: how to say it and how to ask for it.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TagEntry {
    pub label: String,
    /// The datum (`0xe1230045`): what a call names it by.
    pub tag: String,
}

/// The tags of an `edit.tags` answer, named for people and sorted by
/// name. Two tags that read the same get their folder added. A tag the map
/// has no name for reads `<group> <n>`.
pub(crate) fn tag_entries(v: &Value) -> Vec<TagEntry> {
    let mut rows: Vec<(String, Option<String>, String)> = list(v, "tags")
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let datum = str_of(t, "tag");
            match t["name"].as_str() {
                Some(n) => (tag_label(n), parent(n).map(tag_label), datum),
                None => (format!("{} {}", str_of(t, "group"), i + 1), None, datum),
            }
        })
        .collect();
    rows.sort_by(|a, b| a.0.to_lowercase().cmp(&b.0.to_lowercase()).then(a.2.cmp(&b.2)));
    let dup: Vec<bool> = rows
        .iter()
        .enumerate()
        .map(|(i, r)| {
            (i > 0 && rows[i - 1].0 == r.0) || rows.get(i + 1).is_some_and(|n| n.0 == r.0)
        })
        .collect();
    rows.into_iter()
        .zip(dup)
        .map(|((label, folder, tag), dup)| TagEntry {
            label: match (dup, folder) {
                (true, Some(f)) if f != label => format!("{label} ({f})"),
                _ => label,
            },
            tag,
        })
        .collect()
}

/// A player in `sim.view`'s `player` root.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct PlayerInfo {
    /// `player.<n>`, what `sim.*` calls name it by.
    pub id: String,
    pub name: Option<String>,
    pub pos: Option<[f64; 3]>,
    /// Where it aims.
    pub forward: Option<[f64; 3]>,
    pub yaw: Option<f64>,
    /// A controller on this machine drives it.
    pub local: bool,
}

/// The players of a `sim.view {roots: ["player"]}` answer, by index.
pub(crate) fn players(v: &Value) -> Vec<PlayerInfo> {
    let Some(map) = v["player"].as_object() else {
        return Vec::new();
    };
    let mut keyed: Vec<(u64, PlayerInfo)> = map
        .iter()
        .map(|(k, p)| {
            let n: u64 = k.parse().unwrap_or(u64::MAX);
            let id = p["id"]
                .as_str()
                .map_or_else(|| format!("player.{k}"), str::to_owned);
            (
                n,
                PlayerInfo {
                    id,
                    name: p["name"].as_str().filter(|n| !n.is_empty()).map(str::to_owned),
                    pos: vec3(&p["pos"]),
                    forward: vec3(&p["forward"]),
                    yaw: p["yaw"].as_f64(),
                    local: p["local_user"].is_number(),
                },
            )
        })
        .collect();
    keyed.sort_by_key(|(n, _)| *n);
    keyed.into_iter().map(|(_, p)| p).collect()
}

/// The local player (the one a controller here drives).
pub(crate) fn local_player(v: &Value) -> Option<PlayerInfo> {
    players(v).into_iter().find(|p| p.local)
}

impl PlayerInfo {
    /// How a menu says this player: its name, else `Player <n>`.
    pub fn label(&self) -> String {
        match &self.name {
            Some(n) => n.clone(),
            None => format!("Player {}", self.id.rsplit('.').next().unwrap_or("?")),
        }
    }

    /// The level direction the player faces, a unit vector (z 0).
    pub fn heading(&self) -> Option<[f64; 2]> {
        let from_forward = self.forward.and_then(|f| {
            let n = f[0].hypot(f[1]);
            (n > 1e-6).then(|| [f[0] / n, f[1] / n])
        });
        from_forward.or_else(|| {
            self.yaw
                .map(|y| [y.to_radians().cos(), y.to_radians().sin()])
        })
    }

    /// A point `dist` world units ahead of the player and `up` above it,
    /// and the yaw to face the way the player does.
    pub fn ahead(&self, dist: f64, up: f64) -> Option<([f64; 3], f64)> {
        let (p, h) = (self.pos?, self.heading()?);
        let yaw = h[1].atan2(h[0]).to_degrees().rem_euclid(360.0);
        Some(([p[0] + h[0] * dist, p[1] + h[1] * dist, p[2] + up], yaw))
    }
}

/// A one-line toast from a result's object field: `<verb> <tag name>`.
pub(crate) fn tag_toast(verb: &str, v: &Value, field: &str) -> String {
    match v[field]["name"].as_str() {
        Some(n) => format!("{verb} {}", tag_label(n)),
        None => verb.to_owned(),
    }
}

/// An empty-page row.
pub(crate) fn nothing(label: &str, why: &str) -> Vec<Row> {
    vec![Row::info(label, "").about(why)]
}

/// `v` as one decimal (`33.3`).
pub(crate) fn one(v: &Value) -> String {
    match v.as_f64() {
        Some(f) => format!("{f:.1}"),
        None => text(v),
    }
}

/// Every scalar under `v` as a read-only row named by its path (`stats
/// pipeline.frames`). Objects and arrays nest; at most 80 rows.
pub(crate) fn flat(prefix: &str, v: &Value) -> Vec<Row> {
    fn walk(path: String, v: &Value, out: &mut Vec<Row>) {
        if out.len() >= 80 {
            return;
        }
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    if k != "at" {
                        walk(format!("{path} {}", humanize(k).to_lowercase()), x, out);
                    }
                }
            }
            Value::Array(a) => {
                for (i, x) in a.iter().enumerate() {
                    walk(format!("{path} {}", i + 1), x, out);
                }
            }
            Value::Null => {}
            x => out.push(Row::info(path, x.clone())),
        }
    }
    let mut out = Vec::new();
    walk(prefix.to_owned(), v, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn tag_names_read_as_words() {
        assert_eq!(
            tag_label("objects\\weapons\\rifle\\battle_rifle\\battle_rifle"),
            "Battle Rifle"
        );
        assert_eq!(tag_label("vehicles/warthog/warthog"), "Warthog");
        assert_eq!(humanize("infinite_ammo"), "Infinite ammo");
        assert_eq!(humanize("a-b.c"), "A b c");
    }

    #[test]
    fn tag_entries_sort_and_tell_twins_apart() {
        let v = json!({"names": true, "total": 4, "tags": [
            {"tag": "0xe0000002", "group": "scen", "name": "scenery\\rock\\boulder"},
            {"tag": "0xe0000001", "group": "scen", "name": "scenery\\cliff\\boulder"},
            {"tag": "0xe0000003", "group": "scen", "name": "scenery\\barrel\\barrel"},
            {"tag": "0xe0000004", "group": "scen", "name": null},
        ]});
        let e = tag_entries(&v);
        let labels: Vec<_> = e.iter().map(|e| e.label.as_str()).collect();
        assert_eq!(
            labels,
            [
                "Barrel",
                "Boulder (Cliff)",
                "Boulder (Rock)",
                "scen 4"
            ]
        );
        assert_eq!(e[0].tag, "0xe0000003");
    }

    #[test]
    fn players_and_the_point_ahead() {
        let v = json!({"at": {}, "player": {
            "2": {"id": "player.2", "kind": "player", "pos": [1.0, 2.0, 3.0],
                  "forward": [0.0, 1.0, 0.5], "yaw": 90.0, "name": "bot"},
            "1": {"id": "player.1", "kind": "player", "pos": [10.0, 0.0, 1.0],
                  "forward": [0.6, 0.0, -0.8], "yaw": 0.0, "name": "afr", "local_user": 0},
        }});
        let all = players(&v);
        assert_eq!(all[0].id, "player.1");
        let me = local_player(&v).unwrap();
        assert_eq!(me.label(), "afr");
        let (pos, yaw) = me.ahead(4.0, 0.5).unwrap();
        assert!((pos[0] - 14.0).abs() < 1e-9 && (pos[2] - 1.5).abs() < 1e-9);
        assert_eq!(yaw, 0.0);
        assert_eq!(all[1].label(), "bot");
    }

    #[test]
    fn writes_and_menu_are_read_from_their_answers() {
        let a: Vec<Answer> = vec![
            Some(Ok(json!({"kind": "coop_host", "writes": false, "why": "a co-op session"}))),
            Some(Ok(json!({"kind": "local", "writes": true}))),
            Some(Err("nope".into())),
            Some(Ok(json!({"at": {"mode": "main_menu"}}))),
        ];
        assert_eq!(writes_blocked(&a, 0).as_deref(), Some("a co-op session"));
        assert_eq!(writes_blocked(&a, 1), None);
        assert_eq!(writes_blocked(&a, 2).as_deref(), Some("nope"));
        assert_eq!(writes_blocked(&[None], 0).as_deref(), Some("waiting for the game"));
        assert!(at_menu(&a, 3).is_some());
        assert!(at_menu(&a, 1).is_none());
    }

    #[test]
    fn the_first_page_is_the_ids_without_a_slash() {
        let ids: Vec<String> = all().into_iter().map(|s| s.id).collect();
        let top: Vec<&str> = ids.iter().map(String::as_str).filter(|i| is_page(i)).collect();
        assert_eq!(
            top,
            ["player", "world", "game", "time", "view", "perf", "settings", "dev"]
        );
        let mut sorted = ids.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), ids.len(), "ids are unique");
    }

    #[test]
    fn every_open_row_names_a_section() {
        let ids: Vec<String> = all().into_iter().map(|s| s.id).collect();
        let mut game = fake::Game::default();
        for s in all() {
            for r in fake::rows(&mut game, &s) {
                if let crate::model::Edit::Open(id) = &r.edit {
                    assert!(ids.contains(id), "{} opens {id}", s.id);
                }
            }
        }
    }

    #[test]
    fn flat_names_rows_by_their_path() {
        let rows = flat("stats", &json!({"at": {"tick": 1}, "pipeline": {"frames": 12, "ok": true}}));
        let labels: Vec<_> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["stats pipeline frames", "stats pipeline ok"]);
    }
}
