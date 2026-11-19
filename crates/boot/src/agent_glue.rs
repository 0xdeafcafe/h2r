//! `H2_AGENT=<socket>` or `H2_AGENT=tcp://HOST:PORT`: the agent API
//! (crates/agent, docs/runtime/agent-api.md).
//!
//! Listens, adds the `agent` feature (a tick observer that answers
//! requests at tick boundaries), and answers between slices when the game
//! isn't ticking. Frame stats are collected for a live window when nothing
//! else started them, so `profile.counters` has frames to report.
//!
//! With XAPI input, the agent's input tap goes on `xapi`, wrapping the one
//! `H2_INPUT_RECORD` or `H2_INPUT_REPLAY` set: `input.pad`,
//! `input.release`, `input.state`, `input.record`, `input.replay` and
//! `input.stop` (crates/agent's `tap`). That is the same on `h2 serve`,
//! `h2 play` and the Xbox app.
//!
//! It also answers the host methods `profile.sample.*` (the in-process
//! sampler, [`crate::sampling`], which `profile.capture {"sample": true}`
//! runs too) and `host.perf` (the newest host perf line), and
//! `render.frame` and `render.stats` (crates/agent's `render`, over
//! [`crate::render_probe`] and the draw wrapper in [`crate::d3d_diag`]).
//! `sim.*` ([`install_sim`]) gets its session kind from `coop_probe`'s
//! readers ([`session_view`]).
//!
//! `state.*` (machine snapshots) is the boot loop's: it installs the store
//! on the agent and runs due saves and loads between slices
//! ([`crate::snapshot_glue`]).

use std::path::PathBuf;
use std::sync::Mutex;

use agent::Access;
use agent_proto::{RpcError, code, method};
use anyhow::{Context, Result};
use serde_json::{Value, json};

/// The agent attached to one run.
pub struct AgentGlue {
    pub agent: agent::Agent,
    /// `None` for [`AgentGlue::local`]: no socket, only in-process clients.
    server: Option<agent::Server>,
}

/// The socket path from `H2_AGENT`, when set and not empty.
pub fn socket_from_env() -> Option<PathBuf> {
    std::env::var_os(agent_proto::ENV)
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

impl AgentGlue {
    /// Listens at `path`, adds the feature to `registry`, and puts the input
    /// tap on `xapi`. `replaying`: the tap already there is a replay, which
    /// stands in for the live pad (agent steps go on top of it); otherwise
    /// it is a recording, which sees agent steps too.
    pub fn install<S: xapi::Storage>(
        registry: &mut feature::Registry,
        path: PathBuf,
        xapi: Option<&Mutex<xapi::Xapi<S>>>,
        replaying: bool,
    ) -> Result<AgentGlue> {
        let endpoint = agent_proto::Endpoint::from(path);
        if let agent_proto::Endpoint::Unix(p) = &endpoint
            && let Some(dir) = p.parent()
        {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let agent = agent::Agent::new();
        add_host_methods(&agent);
        let _sim = install_sim(&agent);
        agent::havok::install(&agent);
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
        match xapi {
            Some(x) => {
                let mut x = x.lock().unwrap_or_else(|e| e.into_inner());
                let chain = if replaying {
                    agent::Chain::Before
                } else {
                    agent::Chain::After
                };
                let inner = x.take_input_tap().map(|t| (t, chain));
                x.set_input_tap(Some(Box::new(agent.input_tap(inner))));
            }
            None => eprintln!("agent: no XAPI input; the input methods are off"),
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
        Ok(AgentGlue {
            agent,
            server: Some(server),
        })
    }

    /// An agent with no socket, for in-process clients (the debug menu,
    /// docs/runtime/debug-menu.md) when `H2_AGENT` is unset: the host
    /// methods, answered between slices. No tick observer (the tick plan
    /// is what it would be without it), no input tap, nothing listening.
    pub fn local() -> AgentGlue {
        let agent = agent::Agent::new();
        add_host_methods(&agent);
        agent.set_no_tick_hook();
        AgentGlue {
            agent,
            server: None,
        }
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

/// The sampler `profile.sample.*` drives, run over a `profile.capture`
/// window: one sampler, one dump format (`profile.h2prof`, which
/// `h2 profile remote` resolves).
struct BootSampler;

impl agent::Sampler for BootSampler {
    fn start(&mut self) -> std::result::Result<(), String> {
        crate::sampling::start(Value::Null).map(drop)
    }

    fn finish(&mut self) -> std::result::Result<Value, String> {
        crate::sampling::stop(Value::Null)
    }
}

/// The host methods (module docs).
pub fn add_host_methods(agent: &agent::Agent) {
    if crate::sampling::available() {
        agent.add_method(method::SAMPLE_START, Access::Act, |v| {
            crate::sampling::start(v).map_err(rpc(code::UNAVAILABLE))
        });
        agent.add_method(method::SAMPLE_STOP, Access::Act, |v| {
            crate::sampling::stop(v).map_err(rpc(code::UNAVAILABLE))
        });
        agent.set_sampler(Box::new(BootSampler));
    }
    agent::render::add_methods(agent, &agent::render::PROBE, crate::render_probe::sources());
    agent.add_method(method::PROFILE_FALLBACKS, Access::Read, |v| {
        let top = match v.get("top") {
            None => 20,
            Some(n) => n
                .as_u64()
                .filter(|n| (1..=500).contains(n))
                .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "top: 1 to 500"))?
                as usize,
        };
        if let Some(o) = v.as_object()
            && let Some(k) = o.keys().find(|k| *k != "top")
        {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                format!("unknown field {k}"),
            ));
        }
        let stats = crate::fallback::STATS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
            .unwrap_or_default();
        Ok(stats.sites_json(top))
    });
    agent.add_method(method::SAMPLE_STATUS, Access::Read, |_| {
        Ok(crate::sampling::status())
    });
    agent.add_method(
        method::HOST_PERF,
        Access::Read,
        |_| match crate::host_perf::last_line() {
            Some((age, line)) => Ok(json!({ "line": line, "age_ms": age.as_millis() as u64 })),
            None => Err(RpcError::new(
                code::UNAVAILABLE,
                "no host perf line yet (H2_HOST_PERF=1; one every 100 frames or 5 s)",
            )),
        },
    );
    agent.add_method(
        method::UI_SCREEN,
        Access::Read,
        |_| match crate::ui_watch::last() {
            Some((age, top)) => {
                let mut v = top.map_or_else(|| json!({ "screen": null }), |t| t.json());
                v["age_ms"] = json!(age.as_millis() as u64);
                Ok(v)
            }
            None => Err(RpcError::new(
                code::UNAVAILABLE,
                "no screen read yet (H2_UI_WATCH=1; read on each game tick)",
            )),
        },
    );
}

/// `sim.*` (crates/agent's `sim`): its players are the process-wide local
/// bots ([`crate::sim_bots`]), and the session kind is read as `coop_probe`
/// reads it.
pub fn install_sim(agent: &agent::Agent) -> agent::sim::SimHandle {
    agent::sim::install(
        agent,
        Box::new(crate::sim_bots::LocalBots::new(
            bots::local::global().clone(),
        )),
        Some(Box::new(session_view)),
    )
}

/// [`crate::layout_glue::Gm`] over the agent's guest memory.
struct Mem<'a>(&'a dyn engine::GuestMemory);

impl crate::layout_glue::Gm for Mem<'_> {
    fn rd(&self, addr: u32, out: &mut [u8]) -> bool {
        self.0.read(addr, out).is_ok()
    }
    fn wr(&mut self, _: u32, _: &[u8]) -> bool {
        false
    }
}

/// The session kind from the simulation mode, and the squad session
/// (`coop_probe::read_sim`, `read_session`).
pub fn session_view(m: &dyn engine::GuestMemory) -> agent_proto::sim::SessionView {
    use crate::coop_probe::{protocol_name, read_session, read_sim};
    let m = Mem(m);
    let sim = read_sim(&m);
    let s = read_session(&m);
    agent_proto::sim::SessionView {
        kind: sim.map_or(agent_proto::sim::SessionKind::Unknown, |s| {
            agent_proto::sim::SessionKind::from_sim_mode(s.mode)
        }),
        sim_mode: sim.map(|s| s.mode),
        host: s.map(|s| s.is_host()),
        peers: s.map(|s| s.peers),
        players: s.map(|s| s.players),
        protocol: s.map(|s| protocol_name(s.protocol).into()),
        campaign: s.map(|s| s.is_campaign()),
        ..Default::default()
    }
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
    use std::time::Duration;

    #[test]
    fn the_sim_session_kind_comes_from_the_simulation_mode() {
        use agent_proto::sim::SessionKind;
        let mut g = engine::testing::FakeGuest::new();
        assert_eq!(session_view(&g).kind, SessionKind::Unknown);
        let globals = 0x9000_0000;
        g.map(engine::points::SIM_GLOBALS.base, 4);
        g.put_u32(engine::points::SIM_GLOBALS.base, globals);
        g.map(globals, 0x40);
        for (mode, kind) in [
            (2, SessionKind::CoopHost),
            (3, SessionKind::CoopGuest),
            (4, SessionKind::MpHost),
            (5, SessionKind::MpClient),
            (1, SessionKind::Local),
        ] {
            g.put_u32(globals + 8, mode);
            let v = session_view(&g);
            assert_eq!((v.kind, v.sim_mode), (kind, Some(mode)));
            assert_eq!(v.host, None, "no squad session");
        }
    }

    #[test]
    fn the_glue_answers_pad_and_profiler_methods() {
        let a = agent::Agent::new();
        add_host_methods(&a);
        let _tap = a.input_tap(None);
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
            a.submit(
                c,
                &json!({"jsonrpc": "2.0", "id": 1, "method": m, "params": p}).to_string(),
            );
            a.between_slices(&mut Nothing, 0);
            serde_json::from_str(&rx.recv_timeout(Duration::from_secs(5)).unwrap()).unwrap()
        };
        let r = call(method::RENDER_STATS, Value::Null);
        assert!(r["result"]["paths"].is_object(), "{r}");
        let r = call(method::PROFILE_FALLBACKS, json!({"top": 3}));
        assert!(r["result"]["top"].is_array(), "{r}");
        let r = call(method::PROFILE_FALLBACKS, json!({"top": 0}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(method::RENDER_FRAME, Value::Null);
        assert_eq!(r["result"]["armed"], false, "{r}");
        let r = call(method::INPUT_STATE, Value::Null);
        assert_eq!(r["result"]["pad"]["active"], false, "{r}");
        let r = call(method::SAMPLE_STATUS, Value::Null);
        assert!(r["result"]["available"].is_boolean(), "{r}");
        let r = call(method::INPUT_PAD, json!({"steps": [{"buttons": ["nope"]}]}));
        assert_eq!(r["error"]["code"], code::INVALID_PARAMS, "{r}");
        let r = call(
            method::INPUT_PAD,
            json!({"steps": [{"buttons": ["a"], "ms": 50}]}),
        );
        assert_eq!(r["result"]["queued"], 1, "{r}");
    }
}
