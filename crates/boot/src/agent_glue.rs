//! `H2_AGENT=<socket>` or `H2_AGENT=tcp://HOST:PORT`: the agent API
//! (crates/agent, docs/runtime/agent-api.md).
//!
//! Listens, adds the `agent` feature (a tick observer that answers
//! requests at tick boundaries), and answers between slices when the game
//! isn't ticking. Frame stats are collected for a live window when nothing
//! else started them, so `profile.counters` has frames to report.
//!
//! It also answers the host methods: `input.pad`, `input.release` and
//! `input.state` drive player one's pad (`input::backend::remote`, merged
//! by the app shell's input; `h2 serve`'s headless pad doesn't merge it),
//! `profile.sample.*` the in-process sampler ([`crate::sampling`]), and
//! `host.perf` gives the newest host perf line.

use std::path::PathBuf;
use std::time::Duration;

use agent::Access;
use agent_proto::{PadParams, PadStep, RpcError, code, method};
use anyhow::{Context, Result};
use input::backend::remote::{Remote, Step};
use input::gamepad::{Buttons, Gamepad};
use serde_json::{Value, json};

/// The agent attached to one run.
pub struct AgentGlue {
    pub agent: agent::Agent,
    server: agent::Server,
}

/// The socket path from `H2_AGENT`, when set and not empty.
pub fn socket_from_env() -> Option<PathBuf> {
    std::env::var_os(agent_proto::ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl AgentGlue {
    /// Listens at `path` and adds the feature to `registry`.
    pub fn install(registry: &mut feature::Registry, path: PathBuf) -> Result<AgentGlue> {
        let endpoint = agent_proto::Endpoint::from(path);
        if let agent_proto::Endpoint::Unix(p) = &endpoint
            && let Some(dir) = p.parent()
        {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let agent = agent::Agent::new();
        add_host_methods(&agent);
        // On the game thread: the sampler's main target.
        crate::sampling::set_game_thread();
        let server = agent::Server::listen_on(&endpoint, agent.clone())
            .with_context(|| format!("agent: listening on {endpoint}"))?;
        match registry.add(&agent::AgentFeature(agent.clone())) {
            Ok(true) => {}
            Ok(false) => {
                agent.set_no_tick_hook();
                eprintln!(
                    "agent: switched off by H2_FEATURES; answering between slices only, no pausing"
                )
            }
            Err(e) => anyhow::bail!("agent: {e}"),
        }
        // H2_FRAME_STATS and `h2 play` start their own collection; only
        // start a quiet one when neither will.
        if !timeline::phase::enabled() && timeline::phase::Config::from_env().is_none() {
            let window = timeline::phase::Config::DEFAULT_WINDOW_NS;
            if let Err(e) = timeline::phase::start(timeline::phase::Config::live(window)) {
                eprintln!("agent: frame stats: {e}");
            }
        }
        eprintln!(
            "agent: listening on {} (docs/runtime/agent-api.md)",
            server.path().display()
        );
        Ok(AgentGlue { agent, server })
    }

    /// The installed plan, for `features.list`.
    pub fn set_plan(&self, plan: &feature::Plan, installed: &feature::Installed) {
        let mut lines: Vec<String> = plan.text().lines().map(str::to_owned).collect();
        lines.extend(installed.notes.iter().cloned());
        self.agent.set_plan(lines);
    }

    /// Between slices: answer what's queued.
    pub fn after_slice(&self, rt: &runtime::Runtime, presents: u64) {
        let mut reader = agent::GuestReader::new(|| rt.memory());
        self.agent.between_slices(&mut reader, presents);
    }
}

fn rpc(code: i64) -> impl Fn(String) -> RpcError {
    move |m| RpcError::new(code, m)
}

/// The host methods (module docs).
pub fn add_host_methods(agent: &agent::Agent) {
    agent.add_method(method::INPUT_PAD, Access::Act, |v| {
        let p: PadParams = serde_json::from_value(v).map_err(|e| RpcError::new(code::INVALID_PARAMS, e.to_string()))?;
        let steps = pad_steps(&p.steps).map_err(rpc(code::INVALID_PARAMS))?;
        let n = steps.len();
        let remote = Remote::global();
        if p.append {
            remote.append(steps);
        } else {
            remote.replace(steps);
        }
        Ok(json!({ "queued": n, "pad": pad_state(remote) }))
    });
    agent.add_method(method::INPUT_RELEASE, Access::Act, |_| {
        Remote::global().clear();
        Ok(json!({ "pad": pad_state(Remote::global()) }))
    });
    agent.add_method(method::INPUT_STATE, Access::Read, |_| Ok(json!({ "pad": pad_state(Remote::global()) })));
    if crate::sampling::available() {
        agent.add_method(method::SAMPLE_START, Access::Act, |v| {
            crate::sampling::start(v).map_err(rpc(code::UNAVAILABLE))
        });
        agent.add_method(method::SAMPLE_STOP, Access::Act, |v| crate::sampling::stop(v).map_err(rpc(code::UNAVAILABLE)));
    }
    agent.add_method(method::SAMPLE_STATUS, Access::Read, |_| Ok(crate::sampling::status()));
    agent.add_method(method::HOST_PERF, Access::Read, |_| match crate::host_perf::last_line() {
        Some((age, line)) => Ok(json!({ "line": line, "age_ms": age.as_millis() as u64 })),
        None => Err(RpcError::new(
            code::UNAVAILABLE,
            "no host perf line yet (H2_HOST_PERF=1; one every 100 frames or 5 s)",
        )),
    });
}

/// `input.state`'s pad: what is held, and what is queued.
fn pad_state(r: &Remote) -> Value {
    let s = r.status();
    let g = s.pad;
    let mut buttons: Vec<&str> = Vec::new();
    for (name, bit) in DIGITAL {
        if g.buttons.contains(*bit) {
            buttons.push(name);
        }
    }
    for (name, i) in ANALOG {
        if g.analog[*i] > 0 {
            buttons.push(name);
        }
    }
    let axis = |v: i16| (f64::from(v) / 32767.0 * 1000.0).round() / 1000.0;
    json!({
        "active": s.active,
        "queued": s.queued,
        "done": s.done,
        "buttons": buttons,
        "left": [axis(g.thumb_lx), axis(g.thumb_ly)],
        "right": [axis(g.thumb_rx), axis(g.thumb_ry)],
    })
}

const DIGITAL: &[(&str, Buttons)] = &[
    ("up", Buttons::DPAD_UP),
    ("down", Buttons::DPAD_DOWN),
    ("left", Buttons::DPAD_LEFT),
    ("right", Buttons::DPAD_RIGHT),
    ("start", Buttons::START),
    ("back", Buttons::BACK),
    ("ls", Buttons::LEFT_THUMB),
    ("rs", Buttons::RIGHT_THUMB),
];

/// Pressure buttons and their `bAnalogButtons` index.
const ANALOG: &[(&str, usize)] = &[
    ("a", 0),
    ("b", 1),
    ("x", 2),
    ("y", 3),
    ("black", 4),
    ("white", 5),
    ("lt", 6),
    ("rt", 7),
];

/// `input.pad` steps as pad reports for [`Remote`].
pub fn pad_steps(steps: &[PadStep]) -> Result<Vec<Step>, String> {
    steps
        .iter()
        .map(|s| {
            s.check()?;
            let mut g = Gamepad::default();
            for b in &s.buttons {
                let b = b.to_ascii_lowercase();
                if let Some((_, bit)) = DIGITAL.iter().find(|(n, _)| *n == b) {
                    g.buttons.set(*bit, true);
                } else if let Some((_, i)) = ANALOG.iter().find(|(n, _)| *n == b) {
                    g.analog[*i] = 255;
                }
            }
            let pull = |v: f32| (v * 255.0).round() as u8;
            if let Some(v) = s.lt {
                g.analog[6] = pull(v);
            }
            if let Some(v) = s.rt {
                g.analog[7] = pull(v);
            }
            let axis = |v: f32| (v * 32767.0).round() as i16;
            (g.thumb_lx, g.thumb_ly) = (axis(s.left[0]), axis(s.left[1]));
            (g.thumb_rx, g.thumb_ry) = (axis(s.right[0]), axis(s.right[1]));
            Ok(Step {
                pad: g,
                hold: s.ms.map(Duration::from_millis),
                reads: s.reads.unwrap_or(1),
            })
        })
        .collect()
}

impl Drop for AgentGlue {
    fn drop(&mut self) {
        self.agent.shutdown();
        let _ = &self.server;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_steps_become_reports() {
        let steps: Vec<PadStep> = serde_json::from_value(json!([
            {"buttons": ["A", "start", "rt"], "left": [0.5, -1], "ms": 120},
            {"right": [1, 0], "lt": 0.5, "reads": 0}
        ]))
        .unwrap();
        let s = pad_steps(&steps).unwrap();
        assert_eq!(s[0].pad.analog[0], 255);
        assert_eq!(s[0].pad.analog[7], 255);
        assert!(s[0].pad.buttons.contains(Buttons::START));
        assert_eq!((s[0].pad.thumb_lx, s[0].pad.thumb_ly), (16384, -32767));
        assert_eq!(s[0].hold, Some(Duration::from_millis(120)));
        assert_eq!(s[0].reads, 1);
        assert_eq!(s[1].pad.thumb_rx, 32767);
        assert_eq!(s[1].pad.analog[6], 128);
        assert_eq!((s[1].hold, s[1].reads), (None, 0));
        let bad: Vec<PadStep> = serde_json::from_value(json!([{"buttons": ["jump"]}])).unwrap();
        assert!(pad_steps(&bad).is_err());
        // Every name the protocol offers maps to something.
        for b in agent_proto::PAD_BUTTONS {
            assert!(DIGITAL.iter().any(|(n, _)| n == b) || ANALOG.iter().any(|(n, _)| n == b), "{b}");
        }
    }

    #[test]
    fn the_glue_answers_pad_and_profiler_methods() {
        let a = agent::Agent::new();
        add_host_methods(&a);
        let (tx, rx) = std::sync::mpsc::sync_channel(16);
        let c = a.connect(tx);
        struct Nothing;
        impl agent::Reader for Nothing {
            fn status(&mut self) -> agent::GameStatus {
                agent::GameStatus::default()
            }
            fn objects(&mut self) -> Result<Vec<agent_proto::Object>, String> {
                Ok(vec![])
            }
        }
        let call = |m: &str, p: Value| -> Value {
            a.submit(c, &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string());
            a.between_slices(&mut Nothing, 0);
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
        };
        let r = call(method::INPUT_STATE, Value::Null);
        assert_eq!(r["result"]["pad"]["active"], false, "{r}");
        let r = call(method::SAMPLE_STATUS, Value::Null);
        assert!(r["result"]["available"].is_boolean(), "{r}");
        let r = call(method::INPUT_PAD, json!({"steps": [{"buttons": ["nope"]}]}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
    }
}
