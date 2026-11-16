//! The agent API server (docs/runtime/agent-api.md): a Unix socket in the
//! game process that answers JSON-RPC requests about the running game.
//!
//! * [`Server`] listens and queues requests from connections.
//! * [`Agent`] is the executor. The game thread calls
//!   [`Agent::on_tick`] after each `game_tick` (through [`AgentFeature`],
//!   a read-only observer on the `frame` bridge) and
//!   [`Agent::between_slices`] from the boot loop. Every answer is read
//!   on the game's thread, at a tick boundary when the game is ticking.
//! * [`Reader`] is what it reads: [`GuestReader`] over `live-telemetry`'s
//!   readers in the game, a fake in tests.
//!
//! The observer adds dispatch transfers to every tick, as `lockstep`'s
//! does: compare runs that all have the agent on, or all off.

mod exec;
mod reader;
#[cfg(unix)]
mod server;

pub use exec::{Agent, ConnId, Page, TICK_STALE, query};
pub use reader::{
    GameStatus, GuestReader, Reader, actor_object, machine_object, player_object, vehicle_object,
};
#[cfg(unix)]
pub use server::Server;

/// The socket is a Unix socket; elsewhere listening fails with a clear error.
#[cfg(not(unix))]
pub struct Server(std::path::PathBuf);

#[cfg(not(unix))]
impl Server {
    pub fn listen(path: &std::path::Path, _agent: Agent) -> std::io::Result<Server> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            format!("{}: the agent API needs Unix sockets", path.display()),
        ))
    }
    pub fn path(&self) -> &std::path::Path {
        &self.0
    }
}

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
