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
//!   `render.stats` from what the host registers; `render.capture`
//!   ([`render::CAPTURE`]), `render.scene` and `render.parity` over what
//!   the host registers, and `render.compare` ([`compare`]).
//! * [`hostpads`] answers `input.host`: the host controllers as the input
//!   layer and the debug menu's gate see them.
//! * [`menu`] connects the debug menu as an in-process client, and adds
//!   `settings.*` and `perf.overlay` for it (docs/runtime/debug-menu.md).
//! * [`Reader`] is what it reads: [`GuestReader`] over `live-telemetry`'s
//!   readers in the game, a fake in tests.
//! * [`collision`] answers `collision.stats`: the native collision
//!   world's call, bail and decode counters.
//! * [`anim`] reports the graph walk and manager parity counters and their
//!   first mismatches through `anim.graph_walk` and `anim.manager`.
//! * [`havok`] answers `havok.bodies` and `havok.contacts`: the Havok
//!   world's rigid bodies and contact points ([`Agent::add_game_method`]).
//! * [`firefight`] answers `firefight.*` over the game's Firefight
//!   runtime, which the game registers as a [`firefight::Source`].
//! * [`actors`] answers `actors.net`: the actor channel's per-peer budgets
//!   on a distributed host and the puppets and digest check on a client,
//!   from the report the game publishes each tick.
//! * [`rules`] answers `rules.state` over the game's rules runtime, which
//!   the game registers as a [`rules::Source`].
//! * [`variants`] answers `variants.quick`: the lobby variant's Quick
//!   Options rows as its rules script marks them, which the game
//!   registers as a [`variants::Source`].
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
//! * [`shadow`] answers `shadow.*`: shadow checks of the native ports
//!   against the lifted original (docs/engine/shadow-exec.md).
//! * `state.*` saves and loads machine snapshots: the tick observer ends
//!   the slice when one is due, and the boot loop runs it between slices
//!   through a [`SnapshotMachine`] ([`Agent::run_snapshots`]).
//!
//! The observer adds dispatch transfers to every tick, as `lockstep`'s
//! does: compare runs that all have the agent on, or all off.

pub mod actors;
pub mod ai;
pub mod anim;
mod capture;
pub mod collision;
pub mod collision_sources;
pub mod collision_trace_query;
mod collision_traces;
pub use collision_traces::{CollisionTrace, CollisionTraces};
pub mod compare;
pub mod debug_views;
pub mod editor;
mod exec;
pub mod firefight;
pub mod forge;
pub mod rules;
pub mod havok;
pub mod hostpads;
pub mod hot;
pub mod lightbar;
mod live;
mod loads;
pub mod forensics;
mod memory;
pub mod menu;
pub mod net;
pub mod pools;
mod outputs;
pub mod probe;
pub mod raw;
mod reader;
pub mod render;
mod server;
pub mod hs_eval;
pub mod hs_state;
pub mod shadow;
pub mod sim_arms;
pub mod sim;
mod tap;
mod trace;
pub mod variants;

pub use capture::{FrameSource, Sampler, TickMark, summarise, write_csv};
pub use exec::{
    Access, Agent, ConnId, DEFER_TICKS_MAX, GameCtx, GameMethod, HostMethod, Later,
    Page, SnapshotMachine, Snapshots, TICK_STALE, TICKER_CONN, Ticker, query,
};
pub use live::CameraSource;
pub use loads::{LoadTracker, TimelineTotals, TotalsSource};
pub use reader::{
    GameStatus, GuestReader, Reader, StandingFits, TagNames, actor_object, fill_standing_fits,
    machine_object, name_tags, player_object, thing_object, vehicle_object,
};
pub use server::Server;
pub use tap::{
    AgentTap, Chain, Input, MenuPad, MenuReceipt, RoutePad, RoutePorts, live_pad, note_live_pad,
    pad_steps,
};

/// The agent as a feature (`agent`): the tick observer, and the cutscene
/// start and stop observers that set the `cinematic` phase.
pub struct AgentFeature(pub Agent);

impl hooks::Feature for AgentFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new("agent", "Agent API: answers requests at tick boundaries")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
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
            (&guest_layout::points::CINEMATIC_START, true, "cinematic-start"),
            (&guest_layout::points::CINEMATIC_STOP, false, "cinematic-stop"),
        ] {
            let a = self.0.clone();
            b.raw()
                .observe_after(point, label, move |_| a.set_cinematic(on))
                .optional();
        }
        let revert = self.0.clone();
        b.raw().observe_after(&guest_layout::points::GAME_STATE_REVERT, "mirror-cut", move |_| {
            revert.live_cut();
        }).optional();
        let (save, load) = (self.0.clone(), self.0.clone());
        b.snapshot(hooks::SnapshotPolicy::section(
            move || save.save_state(),
            move |bytes, _| load.load_state(bytes),
        ));
        Ok(())
    }
}

/// Lifecycle hooks that must be present before a queued world edit can run.
pub const EDIT_LIFECYCLE: &str = "agent-edit-lifecycle";

pub type EditorActivation = std::sync::Arc<dyn Fn(&dyn guest_layout::GuestMemory) + Send + Sync>;

pub struct EditLifecycleFeature(pub sim::calls::Shared, pub Agent, pub Option<EditorActivation>);

impl hooks::Feature for EditLifecycleFeature {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(EDIT_LIFECYCLE, "Track the active game for queued editor writes")
            .doc("docs/runtime/agent-api.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        use sim::calls;

        let start = self.0.clone();
        let agent = self.1.clone();
        b.game().on_start("map activation", move |o| {
            let current = calls::preview_scenario(&*o.guest());
            calls::lock(&start).map_activated(current);
            agent.live_cut();
        });
        let revert = self.0.clone();
        b.raw().observe_after(&guest_layout::points::GAME_STATE_REVERT, "checkpoint previews", move |_| {
            calls::lock(&revert).checkpoint_reverted();
        });
        calls::install_reload_recovery(b, self.0.clone(), self.1.clone(), self.2.clone());
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}
