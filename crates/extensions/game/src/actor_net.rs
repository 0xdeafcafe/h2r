//! Host-side actor presentation capture at the simulation tick boundary.

mod dialogue;
pub mod host;

use std::sync::{Arc, Mutex};

use engine::GuestMemory;
use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};

/// The capture and the enqueue observer use the simulation world's mode.
/// The game options use a different numbering for the client and server.
fn host_tick(m: &dyn GuestMemory) -> Option<u32> {
    if crate::coop_look::view::mode(m)? != bots::phantom::session::WORLD_DIST_SERVER {
        return None;
    }
    bots::guest::game_tick(m)
}

type AfterCapture = dyn Fn(&dyn GuestMemory, u32, &host::Host) + Send + Sync;

/// One feature per runtime. The transport can schedule for all connected
/// peers from the same capture before the next tick replaces its deltas.
pub struct Capture {
    after_capture: Arc<AfterCapture>,
    reset_peers: Arc<dyn Fn() + Send + Sync>,
}

impl Capture {
    pub fn new(
        after_capture: impl Fn(&dyn GuestMemory, u32, &host::Host) + Send + Sync + 'static,
        reset_peers: impl Fn() + Send + Sync + 'static,
    ) -> Self {
        Self {
            after_capture: Arc::new(after_capture),
            reset_peers: Arc::new(reset_peers),
        }
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
        Manifest::new("actor-net", "Capture distributed host actor presentation")
            .off_by_default()
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
            let mut state = ticks.lock().unwrap();
            let Some(tick) = host_tick(&*memory) else {
                if state.last_tick.is_some() {
                    *state = State::default();
                    in_flight.clear();
                    reset_peers();
                }
                return;
            };
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
                Ok(()) => after_capture(&*memory, tick, &state.host),
                Err(error) => eprintln!("actor-net: host capture at tick {tick}: {error}"),
            }
        });

        let reset_peers = Arc::clone(&self.reset_peers);
        let reset = move || {
            *state.lock().unwrap() = State::default();
            pending.clear();
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
    use engine::{points, testing::FakeGuest};

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
