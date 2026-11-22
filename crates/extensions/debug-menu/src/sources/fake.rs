//! A fake game for the sections' tests: it answers each agent method the
//! way the host does (the formats the methods really return, read from
//! their implementations, not from a guess), and keeps the calls it got.

use serde_json::{Value, json};

use crate::model::{Answer, Call, Row, Section};

pub struct Game {
    pub calls: Vec<Call>,
    /// `sim.session`'s kind: `local`, `coop_host`, ...
    pub kind: &'static str,
    /// `session.status`' `at.mode`.
    pub mode: &'static str,
    pub god: bool,
    pub speed: f64,
    pub cheats_refused: Option<String>,
    pub snapshots: Vec<&'static str>,
    pub with_players: bool,
    pub frame_stats: bool,
    /// `game.code`: clang's background build has finished.
    pub code_ready: bool,
}

impl Default for Game {
    fn default() -> Self {
        Game {
            calls: Vec::new(),
            kind: "local",
            mode: "campaign",
            god: false,
            speed: 1.0,
            cheats_refused: None,
            snapshots: vec!["quick", "before_the_bridge"],
            with_players: true,
            frame_stats: true,
            code_ready: false,
        }
    }
}

const WRITE_REFUSED: &str = "a co-op (synchronous) session: a host write desyncs the guests and \
                             trips the sim hash; drive the player with pads instead";

impl Game {
    pub fn call(&mut self, c: &Call) -> Result<Value, String> {
        self.calls.push(c.clone());
        let p = &c.params;
        match c.method.as_str() {
            "session.status" => Ok(json!({
                "at": {"tick": 5412, "ticks": 5400, "activation": 2, "map_ticks": 900,
                       "present": 7000, "map": "coagulation", "mode": self.mode,
                       "phase": "gameplay", "paused": false, "boundary": "tick"},
                "protocol": 1, "fps": 29.97, "controller": false, "claimed": false,
            })),
            // game::game_code::GameCode::answer, on the fast tier.
            "game.code" => {
                let ready = self.code_ready;
                Ok(json!({
                    "tier": "fast", "full_speed": false,
                    "module": "/tmp/h2boot/fast/libh2game.dylib",
                    "stamp": "xbe 1f2e generator 9a8b abi 7",
                    "background": {
                        "state": if ready { "ready" } else { "building" },
                        "log": "/tmp/h2boot/clang-build.log",
                        "elapsed_s": 230, "typical_s": 900,
                        "step": if ready { "clang (O1): 965 compiled, 42 reused" } else { "emit: 2.6s, 18662 owners" },
                    },
                    "restart": {
                        "ok": ready,
                        "why": (!ready).then_some("the full-speed version is still building"),
                    },
                }))
            }
            "sim.session" => {
                let coop = self.kind == "coop_host";
                Ok(json!({
                    "kind": self.kind, "sim_mode": 1, "writes": !coop,
                    "why": coop.then_some(WRITE_REFUSED),
                }))
            }
            "sim.view" => {
                let players = if self.with_players {
                    json!({
                        "1": {"id": "player.1", "kind": "player", "pos": [36.7, -80.8, 1.7],
                              "forward": [0.0, 1.0, 0.0], "yaw": 90.0, "pitch": 0.0,
                              "name": "afr", "local_user": 0, "team": 0,
                              "health": 1.0, "shield": 1.0},
                        "2": {"id": "player.2", "kind": "player", "pos": [40.0, -70.0, 1.7],
                              "forward": [-1.0, 0.0, 0.0], "yaw": 180.0, "name": "clanker 1"},
                    })
                } else {
                    json!({})
                };
                Ok(json!({"at": {"tick": 5412}, "player": players}))
            }
            "edit.tags" => {
                let group = p["group"].as_str().unwrap_or("");
                let all = [
                    ("0xe1230045", "weap", "objects\\weapons\\rifle\\battle_rifle\\battle_rifle"),
                    ("0xe1230046", "weap", "objects\\weapons\\pistol\\magnum\\magnum"),
                    ("0xe2000001", "vehi", "objects\\vehicles\\warthog\\warthog"),
                    ("0xe2000002", "vehi", "objects\\vehicles\\scorpion\\scorpion"),
                    ("0xe3000001", "scen", "scenery\\barrel\\barrel"),
                ];
                let tags: Vec<Value> = all
                    .iter()
                    .filter(|t| group.is_empty() || t.1 == group)
                    .map(|t| {
                        json!({"tag": t.0, "group": t.1, "name": t.2,
                               "address": "0x10000000", "size": 64, "defined": true})
                    })
                    .collect();
                Ok(json!({"scenario": "0xe0000000", "names": true,
                          "total": tags.len(), "tags": tags}))
            }
            "sim.give" | "sim.spawn" | "sim.state.set" => {
                if self.kind == "coop_host" {
                    return Err(WRITE_REFUSED.into());
                }
                let tag = p["weapon"].as_str().or(p["tag"].as_str());
                Ok(json!({"player": p["player"], "object": "e2000003",
                          "weapon": {"tag": tag, "group": "weap", "name": "objects\\weapons\\rifle\\battle_rifle\\battle_rifle"},
                          "tag": {"tag": tag, "group": "vehi", "name": "objects\\vehicles\\warthog\\warthog"},
                          "held": true, "mode": 2}))
            }
            "cheat.list" => Ok(json!({"refused": self.cheats_refused, "cheats": [
                {"name": "god", "label": "God mode", "about": "no damage", "refused": null,
                 "kind": "bool", "value": self.god},
                {"name": "infinite_ammo", "label": "Infinite ammo", "about": "", "refused": null,
                 "kind": "bool", "value": false},
                {"name": "bottomless_clip", "label": "Bottomless clip", "about": "", "refused": null,
                 "kind": "bool", "value": false},
                {"name": "game_speed", "label": "Game speed", "about": "", "refused": null,
                 "kind": "choice", "value": self.speed,
                 "choices": [{"value": 0.5, "label": "0.5x"}, {"value": 1.0, "label": "1x"},
                             {"value": 2.0, "label": "2x"}]},
                {"name": "fly", "label": "Fly", "about": "", "refused": null,
                 "kind": "bool", "value": false},
                {"name": "noclip", "label": "Noclip", "about": "", "refused": null,
                 "kind": "bool", "value": true},
                {"name": "hide_hud", "label": "Hide HUD", "about": "", "refused": null,
                 "kind": "bool", "value": false},
            ]})),
            "cheat.set" => Ok(json!({})),
            "state.list" => Ok(json!({"dir": "/cache/snapshots", "snapshots": self.snapshots.iter().map(|n| json!({
                "name": n, "bytes": 52_428_800u64, "tick": 4000, "presents": 5000,
                "notes": {"map": "coagulation", "mode": "campaign"},
            })).collect::<Vec<_>>()})),
            "state.save" | "state.load" | "map.reload" | "game.menu" => {
                Ok(json!({"name": p["name"], "queued": true, "map": "coagulation"}))
            }
            "maps.test.list" => Ok(json!({"recipes": [], "maps": [
                {"name": "bridge-rig", "base": "coagulation", "state": "ready", "dir": "/maps/bridge-rig"},
                {"name": "half-built", "base": "zanzibar", "state": "building", "dir": "/maps/half"},
                {"name": "broken", "base": "lockout", "state": "failed", "dir": "/maps/b", "error": "no BSP"},
            ]})),
            "maps.test.load" => Ok(json!({"name": p["name"], "queued": true})),
            "perf.overlay" => Ok(json!({"on": true})),
            "profile.counters" if !self.frame_stats => {
                Err("no frames in the frame-stats window yet".into())
            }
            "profile.counters" => Ok(json!({
                "at": {"tick": 5412}, "frames": 150, "fps": 29.97,
                "p50_ms": 33.3, "p99_ms": 41.26, "max_ms": 80.5,
                "phase_ms": {"cpu": 20.1, "decode": 1.2, "encode": 3.4, "submit": 0.5,
                             "gpu-wait": 6.0, "pace": 25.0, "render-wait": 2.2, "d3d": 4.4},
                "counts": {"draws": 1100.0, "passes": 12.0},
            })),
            "profile.capture" => Ok(json!({
                "at": {"tick": 5800}, "ticks": 300, "complete": true, "frames": 300,
                "wall_ms": 10000.0, "fps": 30.0,
                "frame_ms": {"mean": 33.3, "p50": 33.2, "p95": 36.0, "p99": 41.0, "max": 60.0, "total": 9990.0},
                "csv": "/cache/captures/a.csv",
            })),
            "debug.views" => Ok(json!({"views": [
                {"name": "hitboxes", "label": "Hitboxes", "on": false},
            ], "range": 48, "trace_ticks": 30})),
            "render.toggle" => Ok(json!({"toggles": [
                {"name": "overrides", "on": true, "default": true, "about": "all overrides"},
                {"name": "fog", "on": false, "default": false, "about": ""},
            ]})),
            "render.param" => Ok(json!({"params": [
                {"name": "gain", "value": 1.0, "default": 1.0, "about": "x.wgsl"},
            ]})),
            "settings.list" => {
                let prefix = p["prefix"].as_str().unwrap_or("");
                let all = vec![
                    json!({"key": "graphics.fov", "label": "Field of view", "help": "degrees",
                           "apply": "live", "scope": "view", "kind": "int", "min": 60, "max": 120,
                           "steps": [{"value": 60, "label": "60"}, {"value": 70, "label": "70"}],
                           "value": 70, "running": 70, "pending": false}),
                    json!({"key": "graphics.resolution", "label": "Resolution", "help": "",
                           "apply": "restart", "scope": "host", "kind": "resolution", "value": 1}),
                    json!({"key": "controls.aim_assist", "label": "Aim assist", "help": "",
                           "apply": "live", "scope": "sim", "kind": "bool", "value": true}),
                    json!({"key": "coop.max_players", "label": "Max players", "help": "",
                           "apply": "next_map", "scope": "sim", "kind": "int", "min": 2, "max": 4,
                           "value": 4}),
                    json!({"key": "skulls.envy", "label": "Envy", "help": "campaign skull",
                           "apply": "next_map", "scope": "sim", "kind": "bool", "value": false}),
                    json!({"key": "halo1.hud", "label": "Halo 1 HUD", "help": "",
                           "apply": "live", "scope": "view", "kind": "bool", "value": false}),
                    json!({"key": "superbounce.fall_assist", "label": "Fall assist", "help": "",
                           "apply": "live", "scope": "sim", "kind": "bool", "value": false,
                           "locked": "env"}),
                    json!({"key": "brand_new.thing", "label": "Brand new", "help": "",
                           "apply": "live", "scope": "host", "kind": "bool", "value": false}),
                ];
                Ok(json!({"settings": all.into_iter()
                    .filter(|s| s["key"].as_str().unwrap_or("").starts_with(prefix))
                    .collect::<Vec<_>>()}))
            }
            "edit.changes" => Ok(json!({"changes": [
                {"id": 3, "group": "weap", "name": "objects\\weapons\\rifle\\battle_rifle\\battle_rifle",
                 "tag": "0xe1230045", "path": "magazines[0].rounds_loaded_maximum",
                 "old": 36, "new": 99, "stale": false, "live": true},
            ]})),
            "edit.revert" => Ok(json!({"reverted": []})),
            "render.overrides" => Ok(json!({
                "dir": "/tmp/o", "enabled": true, "overrides": [
                    {"program": "00000000000000aa", "file": "aa.wgsl", "params": [], "state": "applied"},
                    {"program": "00000000000000bb", "file": "bb.wgsl", "params": [], "state": "rejected", "error": "binding 3"},
                ], "problems": []})),
            "render.reload" => Ok(json!({"loaded": []})),
            "render.stats" => Ok(json!({"at": {"tick": 1}, "pipeline": {"frames": 12, "on": true}})),
            "features.list" => Ok(json!({"at": {"tick": 1}, "plan": [
                "feature agent: on",
                "feature lockstep: off (not requested)",
                "feature cheats: on",
                "0x00123456 game_tick: observe_before [agent:tick] -> original -> after [cheats:apply]",
                "0x00123abc entry agent:server",
                "locale: strings loaded",
                "note: 3 points placed",
                "warning: feature debug-views wanted 0x0 and lost it",
            ]})),
            m => Err(format!("no method {m}")),
        }
    }

    /// The calls made to `method`.
    pub fn made(&self, method: &str) -> Vec<Call> {
        self.calls
            .iter()
            .filter(|c| c.method == method)
            .cloned()
            .collect()
    }
}

/// The rows of `s` over `g`: each call answered, then the rows function.
pub fn rows(g: &mut Game, s: &Section) -> Vec<Row> {
    let answers: Vec<Answer> = s.calls.iter().map(|c| Some(g.call(c))).collect();
    (s.rows)(&answers)
}

/// The section with `id`.
pub fn section(id: &str) -> Section {
    super::all()
        .into_iter()
        .find(|s| s.id == id)
        .unwrap_or_else(|| panic!("no section {id}"))
}

/// Rows of the section `id`.
pub fn page(g: &mut Game, id: &str) -> Vec<Row> {
    rows(g, &section(id))
}

/// The row labelled `label`.
pub fn row(rows: &[Row], label: &str) -> Row {
    rows.iter()
        .find(|r| r.label == label)
        .unwrap_or_else(|| {
            panic!(
                "no row {label:?} among {:?}",
                rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>()
            )
        })
        .clone()
}
