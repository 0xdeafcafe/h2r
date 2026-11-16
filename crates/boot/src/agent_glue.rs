//! `H2_AGENT=<socket>`: the agent API (crates/agent, docs/runtime/agent-api.md).
//!
//! Listens on the socket, adds the `agent` feature (a tick observer that
//! answers requests at tick boundaries), and answers between slices when
//! the game isn't ticking. Frame stats are collected for a live window when
//! nothing else started them, so `profile.counters` has frames to report.

use std::path::PathBuf;

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
    /// Listens at `path` and adds the feature to `registry`.
    pub fn install(registry: &mut feature::Registry, path: PathBuf) -> Result<AgentGlue> {
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

impl Drop for AgentGlue {
    fn drop(&mut self) {
        self.agent.shutdown();
        let _ = &self.server;
    }
}
