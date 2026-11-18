//! The agent API server (docs/runtime/agent-api.md): a Unix socket (or a
//! TCP port, for the Xbox app) in the game process that answers JSON-RPC
//! requests about the running game.
//!
//! * [`Server`] listens and queues requests from connections.
//! * [`Agent::add_method`] lets the game's host answer more methods
//!   (the sampling profiler, host perf) on the same thread.
//! * [`Agent::input_tap`] is `xapi`'s input tap: `input.pad` steps per
//!   player, `input.record` and `input.replay`, wherever the game has XAPI
//!   input (`h2 serve`, `h2 play`, the Xbox app).
//! * [`Agent`] is the executor. The game thread calls
//!   [`Agent::on_tick`] after each `game_tick` (through [`AgentFeature`],
//!   a read-only observer on the `frame` bridge) and
//!   [`Agent::between_slices`] from the boot loop. Every answer is read
//!   on the game's thread, at a tick boundary when the game is ticking.
//! * [`render`] records one frame's passes and draws for `render.frame`
//!   (boot's backend wrapper feeds [`render::PROBE`]) and answers
//!   `render.stats` from what the host registers.
//! * [`menu`] connects the debug menu as an in-process client, and adds
//!   `settings.*` and `perf.overlay` for it (docs/runtime/debug-menu.md).
//! * [`Reader`] is what it reads: [`GuestReader`] over `live-telemetry`'s
//!   readers in the game, a fake in tests.
//! * [`sim`] answers `sim.*`: simulated players (through the bots),
//!   host writes to player state, and expectations over one view of the
//!   game ([`Agent::add_game_method`]).
//!
//! The observer adds dispatch transfers to every tick, as `lockstep`'s
//! does: compare runs that all have the agent on, or all off.

mod capture;
mod exec;
pub mod hot;
mod loads;
pub mod menu;
mod reader;
pub mod render;
mod server;
pub mod sim;
mod tap;

pub use capture::{FrameSource, Sampler, TickMark, summarise, write_csv};
pub use exec::{Access, Agent, ConnId, GameCtx, GameMethod, HostMethod, Page, TICK_STALE, query};
pub use loads::{LoadTracker, TimelineTotals, TotalsSource};
pub use reader::{
    GameStatus, GuestReader, Reader, actor_object, machine_object, player_object, vehicle_object,
};
pub use server::Server;
pub use tap::{AgentTap, Chain, Input, pad_steps};

/// The agent as a feature (`agent`): the tick observer, and the cutscene
/// start and stop observers that set the `cinematic` phase.
pub struct AgentFeature(pub Agent);

impl feature::Feature for AgentFeature {
    fn manifest(&self) -> feature::Manifest {
        feature::Manifest::new("agent", "Agent API: answers requests at tick boundaries")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut feature::Builder) -> Result<(), feature::BuildError> {
        let a = self.0.clone();
        // Last, so other observers (lockstep's hash) see the tick before a
        // pause holds it.
        b.frame()
            .on_tick("serve", move |o| {
                let mut r = GuestReader::new(|| o.guest());
                a.on_tick(&mut r);
            })
            .last();
        for (point, on, label) in [
            (&engine::points::CINEMATIC_START, true, "cinematic-start"),
            (&engine::points::CINEMATIC_STOP, false, "cinematic-stop"),
        ] {
            let a = self.0.clone();
            b.raw()
                .observe_after(point, label, move |_| a.set_cinematic(on))
                .optional();
        }
        Ok(())
    }
}
