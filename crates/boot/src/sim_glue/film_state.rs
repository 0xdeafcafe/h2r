//! A film keyframe can resume a sim after lobby creation, spawning, the
//! seed and team writes. Those one-time guest mutations must never repeat.
//! The match report itself is only produced by the recording process.
use super::{Inner, Phase, State, hosted};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, atomic::Ordering};
use std::time::Instant;

#[derive(Serialize, Deserialize)]
struct Playing {
    version: u8,
    clock: sim::metrics::TickCursor,
    bindings: BTreeMap<u8, u8>,
    machines: BTreeMap<u8, [u8; 6]>,
}

impl Playing {
    fn capture(state: &State) -> Self {
        Self {
            version: 1,
            clock: state.tally.cursor(),
            bindings: state.bindings.clone(),
            machines: state.machines.clone(),
        }
    }

    fn restore(self, sim: &Inner) -> Result<(), String> {
        let valid_seat = |seat: &u8| sim.matched.bots.iter().any(|bot| bot.seat == *seat);
        if self.version != 1
            || self
                .bindings
                .keys()
                .chain(self.machines.keys())
                .any(|seat| !valid_seat(seat))
            || self.bindings.values().any(|&player| player >= 16)
            || (sim.matched.distributed() && self.bindings.len() != sim.matched.bots.len())
        {
            return Err("invalid film sim seat bindings".into());
        }
        let mut state = sim.lock();
        state.tally.resume_playback(self.clock)?;
        state.phase = Phase::Playing;
        state.set_up = true;
        state.bindings = self.bindings;
        state.machines = self.machines;
        state.fired.clear();
        state.end = None;
        state.first_tick = Some(Instant::now());
        state.last_tick = state.first_tick;
        sim.stop.store(false, Ordering::Release);
        Ok(())
    }
}

pub(super) fn policy(
    sim: &Arc<Inner>,
    lobby: Option<Arc<Mutex<hosted::LobbyState>>>,
    playback: bool,
) -> feature::SnapshotPolicy {
    let check = sim.clone();
    let save = sim.clone();
    let load = sim.clone();
    let check_lobby = lobby.clone();
    feature::SnapshotPolicy::section_forbid_while(
        move || {
            if !crate::theater::in_keyframe() || playback {
                return None;
            }
            let state = check.lock();
            let ready = state.phase == Phase::Playing && state.set_up && state.tally.ticks() > 0;
            drop(state);
            let lobby_ready = check_lobby.as_ref().is_none_or(|lobby| {
                lobby
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .setup_complete()
            });
            (!ready || !lobby_ready)
                .then(|| "sim lobby, seat binding or initial seed/team setup is in progress".into())
        },
        move || {
            if !crate::theater::in_keyframe() {
                return vec![]; // A normal state.load starts a fresh metrics tally.
            }
            serde_json::to_vec(&Playing::capture(&save.lock())).expect("integer sim film state")
        },
        move |bytes, _| {
            if !crate::theater::in_keyframe() && bytes.is_empty() {
                return Ok(());
            }
            if !playback || !crate::theater::in_keyframe() {
                return Err("sim playback state requires a film player".into());
            }
            let state: Playing = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            state.restore(&load)?;
            if let Some(lobby) = &lobby {
                lobby
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .resume_match();
            }
            Ok(())
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_glue::Sim;
    use sim::metrics::TickSample;
    use std::path::PathBuf;

    #[test]
    fn restoring_an_active_sim_keeps_setup_latched_and_its_tick_limit_clock() {
        let config = sim::config::parse_match(
            "map = \"lockout\"\nmax_ticks = 3\n[[bot]]\nkind = \"phantom\"\n[[bot]]\n",
            false,
        )
        .unwrap();
        let matched = sim::config::resolve(config, &|_| Err("no tuning files".into())).unwrap();
        let source = Sim::new(PathBuf::from("match"), matched.clone(), None);
        let bytes = {
            let mut s = source.0.lock();
            s.phase = Phase::Playing;
            s.set_up = true;
            s.tally.start(40);
            for tick in [41, 42] {
                s.tally.tick(&TickSample {
                    tick,
                    ..Default::default()
                });
            }
            s.bindings = BTreeMap::from([(0, 7), (1, 8)]);
            serde_json::to_vec(&Playing::capture(&s)).unwrap()
        };
        let fresh = Sim::new(PathBuf::from("film"), matched, None);
        serde_json::from_slice::<Playing>(&bytes)
            .unwrap()
            .restore(&fresh.0)
            .unwrap();
        let mut s = fresh.0.lock();
        assert_eq!(s.phase, Phase::Playing);
        assert!(s.set_up);
        assert_eq!(s.bindings, BTreeMap::from([(0, 7), (1, 8)]));
        s.tally.tick(&TickSample {
            tick: 42,
            ..Default::default()
        });
        assert_eq!(s.tally.ticks(), 2);
        s.tally.tick(&TickSample {
            tick: 43,
            ..Default::default()
        });
        assert_eq!(s.tally.ticks(), 3);
        assert_eq!(s.tally.resets(), 0);
    }
}
