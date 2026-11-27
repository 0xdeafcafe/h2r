//! The watermark's `full` level (docs/runtime/app.md, "Watermark"): the
//! local player, the level and the render picture, as lines for the
//! readout in the window's corner.
//!
//! Every value comes from a method the agent API already answers, over an
//! in-process connection like the cheat line's (`debug_menu_glue`), so no
//! guest memory is read here: `sim.view` (`at`, `player`, `count`),
//! `world.pools`, `collision.trace` (the cluster the player stands in) and
//! `render.scene` (the native picture and the `scene_kept_*` causes). The
//! thread asks only while the level is on, and hands the finished lines to
//! the window through [`Link`].

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use debug_menu::{Call, Rpc};
use serde_json::{Value, json};

/// How often the readout asks: about a present at 30 fps.
const POLL: Duration = Duration::from_millis(100);

/// What the window and the game share: the level switch (on for `full`)
/// and the lines the game leaves for the window.
#[derive(Clone, Default)]
pub struct Link {
    pub full: Arc<AtomicBool>,
    pub lines: Arc<Mutex<Vec<String>>>,
}

/// The values the lines are made of; each is absent until its method has
/// answered.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Readout {
    map: Option<String>,
    bsp: Option<u32>,
    pos: Option<[f64; 3]>,
    vel: Option<[f64; 3]>,
    yaw: Option<f64>,
    pitch: Option<f64>,
    cluster: Option<i64>,
    players: Option<u64>,
    actors: Option<u64>,
    vehicles: Option<u64>,
    objects: Option<u64>,
    pool_actors: Option<u64>,
    decals: Option<u64>,
    particles: Option<u64>,
    native_draws: Option<u64>,
    kept_draws: Option<u64>,
    top_kept: Option<(String, u64)>,
}

fn vec3(v: &Value) -> Option<[f64; 3]> {
    let a = v.as_array()?;
    Some([a.first()?.as_f64()?, a.get(1)?.as_f64()?, a.get(2)?.as_f64()?])
}

impl Readout {
    /// `sim.view` with roots `at`, `player` and `count`.
    pub fn apply_view(&mut self, v: &Value) {
        self.map = v["at"]["map"].as_str().map(str::to_owned);
        self.bsp = v["at"]["bsp"].as_u64().map(|b| b as u32);
        // The local player: the one with a controller slot, else the first.
        let players = v["player"].as_object();
        let me = players.and_then(|p| {
            p.values()
                .find(|o| o.get("local_user").is_some())
                .or_else(|| p.values().next())
        });
        self.pos = me.and_then(|o| vec3(&o["pos"]));
        self.vel = me.and_then(|o| vec3(&o["velocity"]));
        self.yaw = me.and_then(|o| o["yaw"].as_f64());
        self.pitch = me.and_then(|o| o["pitch"].as_f64());
        let c = &v["count"];
        self.players = c["player"].as_u64();
        self.actors = c["actor"].as_u64();
        self.vehicles = c["vehicle"].as_u64();
        self.objects = c["total"].as_u64();
    }

    /// `world.pools`.
    pub fn apply_pools(&mut self, v: &Value) {
        self.pool_actors = v["actors"].as_u64();
        self.decals = v["decals"].as_u64();
        self.particles = v["particles"].as_u64();
    }

    /// `collision.trace` at the player's position.
    pub fn apply_trace(&mut self, v: &Value) {
        self.cluster = v["locations"][0]["cluster"].as_i64().filter(|c| *c >= 0);
    }

    /// `render.scene`: draws the native graph made, draws the guest still
    /// made on D3D (the sink's last frame, all slots), and the `scene_kept_*`
    /// counter with the most draws.
    pub fn apply_scene(&mut self, v: &Value) {
        self.native_draws = v["display_frame"]["backend_draws"].as_u64();
        self.kept_draws = v["carried"]["slots"]
            .as_object()
            .map(|s| s.values().filter_map(|c| c["draws"].as_u64()).sum());
        self.top_kept = v["counters"].as_object().and_then(|c| {
            c.iter()
                .filter_map(|(k, n)| Some((k.strip_prefix("scene_kept_")?, n.as_u64()?)))
                .filter(|(_, n)| *n > 0)
                .max_by_key(|(_, n)| *n)
                .map(|(k, n)| (k.to_owned(), n))
        });
    }

    /// The readout's lines, grouped: the player, the level, the world, the
    /// picture. Each is short enough for a third of a small window.
    pub fn lines(&self) -> Vec<String> {
        fn n<T: std::fmt::Display>(v: Option<T>) -> String {
            v.map_or("-".into(), |v| v.to_string())
        }
        let mut out = Vec::new();
        out.push(match self.pos {
            Some([x, y, z]) => format!("pos {x:.2} {y:.2} {z:.2}"),
            None => "pos -".into(),
        });
        out.push(match self.vel {
            Some([x, y, z]) => format!(
                "vel {x:.2} {y:.2} {z:.2} ({:.2}/s)",
                (x * x + y * y + z * z).sqrt()
            ),
            None => "vel -".into(),
        });
        out.push(match (self.yaw, self.pitch) {
            (Some(y), Some(p)) => format!("yaw {y:.1} pitch {p:.1}"),
            _ => "yaw - pitch -".into(),
        });
        // The agent API names a BSP by its index; the map names the file.
        out.push(format!(
            "bsp {} of {}  cluster {}",
            n(self.bsp),
            self.map.as_deref().unwrap_or("-"),
            n(self.cluster)
        ));
        let ai = self.pool_actors.or(self.actors);
        out.push(format!(
            "bipeds {} veh {} obj {}",
            n(self.players.zip(self.actors).map(|(p, a)| p + a)),
            n(self.vehicles),
            n(self.objects)
        ));
        out.push(format!(
            "ai {} decals {} particles {}",
            n(ai),
            n(self.decals),
            n(self.particles)
        ));
        out.push(format!(
            "draws native {} d3d {}",
            n(self.native_draws),
            n(self.kept_draws)
        ));
        if let Some((cause, count)) = &self.top_kept {
            out.push(format!("kept {cause} {count}"));
        }
        out
    }
}

/// Starts the readout thread: while `link.full` is on, asks the agent for
/// each value every [`POLL`] and leaves the lines in `link.lines`; while it
/// is off, asks nothing and leaves none.
pub fn install(agent: &agent::Agent, link: Link) {
    let mut rpc = agent::menu::MenuRpc::new(agent);
    let spawned = std::thread::Builder::new()
        .name("debug-readout".into())
        .spawn(move || {
            let mut readout = Readout::default();
            let mut round = 0u64;
            loop {
                std::thread::sleep(POLL);
                if !link.full.load(Ordering::Relaxed) {
                    if let Ok(mut l) = link.lines.lock() {
                        l.clear();
                    }
                    continue;
                }
                // The answers to last round's asks; each id is `round * 4 + ask`.
                for (id, answer) in rpc.recv() {
                    let Ok(v) = answer else { continue };
                    match id % 4 {
                        0 => readout.apply_view(&v),
                        1 => readout.apply_pools(&v),
                        2 => readout.apply_scene(&v),
                        _ => readout.apply_trace(&v),
                    }
                }
                if let Ok(mut l) = link.lines.lock() {
                    *l = readout.lines();
                }
                round += 1;
                let id = |ask: u64| round * 4 + ask;
                rpc.send(id(0), &Call::new("sim.view", json!({"roots": ["at", "player", "count"]})));
                rpc.send(id(1), &Call::bare("world.pools"));
                rpc.send(id(2), &Call::bare("render.scene"));
                if let Some(p) = readout.pos {
                    rpc.send(id(3), &Call::new("collision.trace", json!({"start": p, "delta": [0, 0, 0], "flags": 0})));
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("watermark: full readout off: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn view() -> Value {
        json!({
            "at": {"map": "01a_tutorial", "bsp": 1},
            "player": {
                "1": {"pos": [9.0, 9.0, 9.0], "velocity": [0, 0, 0], "yaw": 1.0, "pitch": 2.0},
                "0": {"local_user": 0, "pos": [1.0, 2.0, 3.0], "velocity": [3.0, 0.0, 4.0], "yaw": 90.0, "pitch": -4.5}
            },
            "count": {"player": 2, "actor": 5, "vehicle": 1, "total": 212}
        })
    }

    #[test]
    fn the_local_player_and_the_counts_come_from_sim_view() {
        let mut r = Readout::default();
        r.apply_view(&view());
        let l = r.lines();
        assert_eq!(l[0], "pos 1.00 2.00 3.00");
        assert_eq!(l[1], "vel 3.00 0.00 4.00 (5.00/s)");
        assert_eq!(l[2], "yaw 90.0 pitch -4.5");
        assert_eq!(l[3], "bsp 1 of 01a_tutorial  cluster -");
        assert_eq!(l[4], "bipeds 7 veh 1 obj 212");
    }

    #[test]
    fn pools_trace_and_scene_fill_the_rest() {
        let mut r = Readout::default();
        r.apply_view(&view());
        r.apply_pools(&json!({"actors": 3, "decals": 14, "particles": null}));
        r.apply_trace(&json!({"locations": [{"leaf": 4, "cluster": 12, "bsp": 1}, {}]}));
        r.apply_scene(&json!({
            "display_frame": {"backend_draws": 812},
            "carried": {"slots": {"opaque": {"draws": 30}, "overlay": {"draws": 10}}},
            "counters": {"scene_kept_5_world_unread": 12, "scene_kept_3_world_set": 90, "scenes_sealed": 4000, "scene_kept_x": 0}
        }));
        let l = r.lines();
        assert_eq!(l[3], "bsp 1 of 01a_tutorial  cluster 12");
        assert_eq!(l[5], "ai 3 decals 14 particles -");
        assert_eq!(l[6], "draws native 812 d3d 40");
        assert_eq!(l[7], "kept 3_world_set 90");
    }

    #[test]
    fn nothing_known_reads_as_dashes_and_no_kept_line() {
        let l = Readout::default().lines();
        assert_eq!(l[0], "pos -");
        assert_eq!(l[6], "draws native - d3d -");
        assert_eq!(l.len(), 7);
    }
}
