//! Engine integration for distributed actors and creature replication.

pub mod client;
pub mod creature;
mod dialogue;
pub mod host;
#[cfg(feature = "rules")]
pub mod pick;
pub mod report;
pub mod roles;
pub mod squads;

use std::sync::{Arc, Mutex};

use ::actor_net::Record;
use guest_layout::GuestMemory;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

/// The running rules script's show cues for this tick as `Cue` records
/// (`crate::rules::world::show`, docs/game/rules-world-api.md, "The show
/// channel"); none without the `rules` feature.
fn show_cues() -> Vec<Record> {
    #[cfg(feature = "rules")]
    {
        crate::rules::world::show::take_records()
    }
    #[cfg(not(feature = "rules"))]
    {
        Vec::new()
    }
}

/// The running rules script's show state, encoded
/// (`crate::rules::world::show::take_state`); none without the `rules`
/// feature or when no script runs.
fn show_state() -> Option<Vec<u8>> {
    #[cfg(feature = "rules")]
    {
        crate::rules::world::show::take_state()
    }
    #[cfg(not(feature = "rules"))]
    {
        None
    }
}

/// Forgets the show cues and the state waiting for the channel, and the
/// state a client holds: a new match, a lost host tick or a snapshot
/// restore starts it all empty.
fn clear_show_cues() {
    #[cfg(feature = "rules")]
    crate::rules::world::show::clear_cues();
    #[cfg(feature = "rules")]
    crate::rules::world::show::clear_held();
}

/// The host log head of the mode running on this machine, for the
/// channel's `Digest`: set by the writer of the log (the Firefight host
/// runtime after each tick), none when no mode keeps one.
static LOG_HEAD: Mutex<Option<[u8; 32]>> = Mutex::new(None);

/// Publish the running mode's host log head; none when no mode runs.
pub fn publish_log_head(head: Option<[u8; 32]>) {
    *LOG_HEAD.lock().unwrap_or_else(|e| e.into_inner()) = head;
}

fn log_head() -> Option<[u8; 32]> {
    *LOG_HEAD.lock().unwrap_or_else(|e| e.into_inner())
}

/// The capture and the enqueue observer use the simulation world's mode.
/// The game options use a different numbering for the client and server.
fn host_tick(m: &dyn GuestMemory) -> Option<u32> {
    if crate::coop_look::view::mode(m)? != bots::phantom::session::WORLD_DIST_SERVER {
        return None;
    }
    bots::guest::game_tick(m)
}

type AfterCapture = dyn Fn(&dyn GuestMemory, u32, &host::Host) + Send + Sync;
type Receive = dyn Fn(&mut client::Client, u32) + Send + Sync;

/// One feature per runtime: the distributed host's capture and, with
/// [`Capture::with_client`], the distributed client's puppets. The
/// transport can schedule for all connected peers from the same capture
/// before the next tick replaces its deltas.
pub struct Capture {
    after_capture: Arc<AfterCapture>,
    reset_peers: Arc<dyn Fn() + Send + Sync>,
    receive: Option<Arc<Receive>>,
}

impl Capture {
    pub fn new(
        after_capture: impl Fn(&dyn GuestMemory, u32, &host::Host) + Send + Sync + 'static,
        reset_peers: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            after_capture: Arc::new(after_capture),
            reset_peers: Arc::new(reset_peers),
            receive: None,
        }
    }

    /// Add the client half (`client::install`). `receive` runs on the
    /// game thread before each distributed client tick: it hands the
    /// drained packets to [`client::Client::receive`] and asks the host
    /// for a key when [`client::Client::take_key_request`] says so.
    pub fn with_client(
        mut self,
        receive: impl Fn(&mut client::Client, u32) + Send + Sync + 'static,
    ) -> Self {
        self.receive = Some(Arc::new(receive));
        self
    }
}

#[derive(Default)]
struct State {
    host: host::Host,
    lines: dialogue::Lines,
    last_tick: Option<u32>,
}

impl Feature for Capture {
    fn manifest(&self) -> Manifest {
        Manifest::new(
            "actor-net",
            "Capture distributed host actor presentation and apply it on clients",
        )
        .doc("docs/game/ai-networking.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let state = Arc::new(Mutex::new(State::default()));
        let enqueue = Arc::clone(&state);
        let pending = dialogue::install(b, move |line| enqueue.lock().unwrap().lines.push(line));

        let ticks = Arc::clone(&state);
        let after_capture = Arc::clone(&self.after_capture);
        let reset_peers = Arc::clone(&self.reset_peers);
        let in_flight = pending.clone();
        b.frame().on_tick("capture replicated actors", move |o| {
            let memory = o.guest();
            let Some(tick) = host_tick(&*memory) else {
                let mut state = ticks.lock().unwrap();
                if state.last_tick.is_some() {
                    report::clear(agent::actors::Role::Host);
                    *state = State::default();
                    in_flight.clear();
                    clear_show_cues();
                    reset_peers();
                }
                return;
            };
            #[cfg(feature = "rules")]
            let roles = crate::rules::feature::network_roles(&*memory);
            #[cfg(not(feature = "rules"))]
            let roles = None;
            let mut state = ticks.lock().unwrap();
            if let Some(previous) = state.last_tick {
                let elapsed = tick.wrapping_sub(previous);
                if elapsed == 0 {
                    return;
                }
                if elapsed >= 1 << 31 {
                    // A guest clock rewind starts a fresh budget and actor set.
                    state.host = host::Host::default();
                    reset_peers();
                }
            }
            let lines = state.lines.take();
            state.last_tick = Some(tick);
            // Probe P3 decides whether the client's walk cycle needs controls.
            match state.host.capture(&*memory, tick, false, &lines) {
                Ok(()) => {
                    let hud = crate::firefight::hud::slot().get().unwrap_or_default();
                    let mode = roles::encode(hud, roles);
                    if let Err(error) = state.host.set_mode(&mode) {
                        eprintln!("actor-net: mode state at tick {tick}: {error}");
                    }
                    // The running rules script's show cues (`message`
                    // calls); empty when no script runs.
                    state.host.set_cues(show_cues());
                    state.host.set_show(show_state());
                    state.host.set_log_head(log_head());
                    after_capture(&*memory, tick, &state.host)
                }
                Err(error) => eprintln!("actor-net: host capture at tick {tick}: {error}"),
            }
        });

        let client = self.receive.as_ref().map(|receive| {
            let receive = Arc::clone(receive);
            client::install(b, move |client, tick| receive(client, tick))
        });

        let reset_peers = Arc::clone(&self.reset_peers);
        let reset = move || {
            *state.lock().unwrap() = State::default();
            if let Some(client) = &client {
                *client.lock().unwrap() = client::Client::default();
                roles::receive(&[]);
                #[cfg(feature = "rules")]
                crate::rules::world::pick::client_reset();
            }
            pending.clear();
            clear_show_cues();
            reset_peers();
        };
        let new_match = reset.clone();
        b.game()
            .on_start("clear actor capture for the new match", move |_| {
                new_match()
            });
        b.snapshot(SnapshotPolicy::section(Vec::new, move |_, _| {
            reset();
            Ok(())
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::{points, testing::FakeGuest};

    #[test]
    fn only_a_distributed_host_has_a_capture_tick() {
        let mut m = FakeGuest::new();
        assert_eq!(host_tick(&m), None);
        m.put_u32(points::SIM_GLOBALS.base, 0x4000)
            .put_u32(points::GAME_TIME.base, 0x8000)
            .put_u32(0x8008, 17);
        for mode in 0..=5 {
            m.put_u32(0x4008, mode);
            assert_eq!(host_tick(&m), (mode == 4).then_some(17));
        }
    }
}
