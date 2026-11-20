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
//! * [`hostpads`] answers `input.host`: the host controllers as the input
//!   layer and the debug menu's gate see them.
//! * [`menu`] connects the debug menu as an in-process client, and adds
//!   `settings.*` and `perf.overlay` for it (docs/runtime/debug-menu.md).
//! * [`Reader`] is what it reads: [`GuestReader`] over `live-telemetry`'s
//!   readers in the game, a fake in tests.
//! * [`collision`] answers `collision.stats`: the native collision
//!   world's call, bail and decode counters.
//! * [`havok`] answers `havok.bodies` and `havok.contacts`: the Havok
//!   world's rigid bodies and contact points ([`Agent::add_game_method`]).
//! * [`raw`] answers `world.raw`: an object's body, physics component,
//!   Havok entity, motion and shape as raw bytes. `world.poke` writes
//!   bounded byte ranges within those parts.
//! * [`sim`] answers `sim.*`: simulated players (through the bots),
//!   host writes to player state, the game's own teleport, give and spawn
//!   (`sim::calls`, whose feature makes the calls at the next tick and
//!   whose methods answer then, [`GameCtx::defer`]), and expectations over
//!   one view of the game ([`Agent::add_game_method`]).
//! * `trace` answers `trace.*` and `flight.*`: rows of chosen objects
//!   taken at every tick, streamed to a file or kept in a ring with the
//!   tick's controller reads.
//! * `state.*` saves and loads machine snapshots: the tick observer ends
//!   the slice when one is due, and the boot loop runs it between slices
//!   through a [`SnapshotMachine`] ([`Agent::run_snapshots`]).
//!
//! The observer adds dispatch transfers to every tick, as `lockstep`'s
//! does: compare runs that all have the agent on, or all off.

pub mod ai;
mod capture;
pub mod collision;
mod exec;
pub mod havok;
pub mod hostpads;
pub mod hot;
mod loads;
pub mod menu;
pub mod raw;
mod reader;
pub mod render;
mod server;
pub mod hs_state;
pub mod sim;
mod tap;
mod trace;

pub use capture::{FrameSource, Sampler, TickMark, summarise, write_csv};
pub use exec::{
    Access, Agent, ConnId, DEFER_TICKS_MAX, GameCtx, GameMethod, HostMethod, Later, LobbyTape,
    Page, SnapshotMachine, Snapshots, TICK_STALE, query,
};
pub use loads::{LoadTracker, TimelineTotals, TotalsSource};
pub use reader::{
    GameStatus, GuestReader, Reader, TagNames, actor_object, machine_object, name_tags,
    player_object, thing_object, vehicle_object,
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
                // A save or load is due: end the slice a few transfers
                // from here, where the boot loop runs it.
                if a.on_tick(&mut r) {
                    o.runtime.request_preempt(runtime::preempt::REQUEST);
                }
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
        let (save, load) = (self.0.clone(), self.0.clone());
        b.snapshot(feature::SnapshotPolicy::section(
            move || save.save_state(),
            move |bytes, _| load.load_state(bytes),
        ));
        Ok(())
    }
}
