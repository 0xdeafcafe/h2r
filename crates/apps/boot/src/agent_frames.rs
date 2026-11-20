//! The agent's tick marks for `render.capture` and the rows of
//! `sim.hashes` (crates/extensions/agent's `render::CAPTURE` and
//! `sim_bench::hashes::HASHES`; docs/graphics/framescene.md, sections 9.2 and
//! 9.3).
//!
//! A read-only tick observer, `agent-frames`, added with the agent's
//! socket ([`crate::agent_glue`]). After each game tick it:
//!
//! - tells `render.capture` which tick ended (the game's own count, as
//!   `at.tick`), one relaxed load more when nothing is asked;
//! - records the tick's `state_hashes_v1` row as sim mode computes it
//!   ([`crate::sim_glue::report_gameplay_hash`], under the same
//!   `impl.sim.metrics-projection` switch), with the game-state arena's
//!   whole hash under `impl.sim.arena-hash` (the same hash sim mode's
//!   block pass returns). The gameplay digest reads the players and their
//!   units: a few microseconds a tick. The arena hash reads the whole
//!   arena, which is why it stays behind its switch.
//!
//! A tick where the players can't be read (menus, loading) has no row.

use std::sync::Mutex;

pub struct AgentFrames;

impl hooks::Feature for AgentFrames {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(
            "agent-frames",
            "Agent API: render.capture's tick marks and sim.hashes' rows",
        )
        .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        // Diagnostics only: a load moves the tick, which starts the hash
        // series again, and a capture waiting then never binds.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        let projected = b.switches().implementation("sim.metrics-projection", true);
        let arena = b
            .switches()
            .implementation("sim.arena-hash", false)
            .then(|| Mutex::new(Vec::new()));
        b.frame().on_tick("agent: tick marks", move |o| {
            let g = o.guest();
            let Ok(tick) = crate::telemetry::game_tick(&g) else {
                return;
            };
            agent::render::CAPTURE.tick_ended(tick);
            let Some((tick, gameplay)) = crate::sim_glue::report_gameplay_hash(&g, projected)
            else {
                return;
            };
            let arena = arena.as_ref().and_then(|buffer| {
                let mut bytes = buffer.lock().unwrap_or_else(|e| e.into_inner());
                lockstep::hash_sim_buffered(&g, lockstep::SimRegion::HALO2, &mut bytes)
            });
            agent::sim::hashes::HASHES.record(tick, gameplay, arena);
        });
        Ok(())
    }
}
