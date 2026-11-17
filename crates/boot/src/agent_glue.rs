//! `H2_AGENT=<socket>`: the agent API (crates/agent, docs/runtime/agent-api.md).
//!
//! Listens on the socket, adds the `agent` feature (a tick observer that
//! answers requests at tick boundaries), and answers between slices when
//! the game isn't ticking. With XAPI input, the agent's input tap goes on
//! `xapi` (`input.pad`), wrapping the one `H2_INPUT_RECORD` or
//! `H2_INPUT_REPLAY` set. Frame stats are collected for a live window when
//! nothing else started them, so `profile.counters` has frames to report.

use std::path::PathBuf;
use std::sync::Mutex;

use anyhow::{Context, Result};

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
    /// Listens at `path`, adds the feature to `registry`, and puts the input
    /// tap on `xapi`. `replaying`: the tap already there is a replay, which
    /// stands in for the live pad (agent packets go on top of it);
    /// otherwise it is a recording, which sees agent packets too.
    pub fn install<S: xapi::Storage>(
        registry: &mut feature::Registry,
        path: PathBuf,
        xapi: Option<&Mutex<xapi::Xapi<S>>>,
        replaying: bool,
    ) -> Result<AgentGlue> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        }
        let agent = agent::Agent::new();
        let server = agent::Server::listen(&path, agent.clone())
            .with_context(|| format!("agent: listening on {}", path.display()))?;
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
        // profile.capture {"sample": true} runs perf's sampler.
        #[cfg(feature = "perf")]
        agent.set_sampler(Box::new(PerfSampler(None)));
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
            None => eprintln!("agent: no XAPI input; input.pad is unavailable"),
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

/// `perf::Session` as the agent's sampler, started on the game thread
/// (where `profile.capture` is answered).
#[cfg(feature = "perf")]
struct PerfSampler(Option<(perf::Session, PathBuf)>);

#[cfg(feature = "perf")]
impl agent::Sampler for PerfSampler {
    fn start(&mut self, dir: &std::path::Path) -> std::result::Result<(), String> {
        if self.0.is_some() {
            return Err("a sampling session is already running".into());
        }
        let config = perf::SessionConfig {
            profile: Some(dir.to_owned()),
            ..Default::default()
        };
        let s = perf::Session::start(config).map_err(|e| format!("starting the sampler: {e}"))?;
        self.0 = Some((s, dir.to_owned()));
        Ok(())
    }

    fn finish(&mut self) -> std::result::Result<PathBuf, String> {
        let (s, dir) = self.0.take().ok_or("no sampling session is running")?;
        s.finish()
            .map_err(|e| format!("writing the profile: {e}"))?;
        Ok(dir.join("profile.folded"))
    }
}

impl Drop for AgentGlue {
    fn drop(&mut self) {
        self.agent.shutdown();
        let _ = &self.server;
    }
}
