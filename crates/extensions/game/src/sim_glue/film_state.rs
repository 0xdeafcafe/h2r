//! A film keyframe can resume a sim after lobby creation, spawning, the
//! initial seed write. Those one-time guest mutations must never repeat.
//! The match report itself is only produced by the recording process.
use super::{Inner, Phase, State};
use crate::host_lobby::SquadId;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::{Arc, atomic::Ordering};
use std::time::Instant;

#[derive(Serialize, Deserialize)]
struct Playing {
    version: u8,
    clock: sim_bench::metrics::TickCursor,
    bindings: BTreeMap<u8, u8>,
    machines: BTreeMap<u8, [u8; 6]>,
    #[serde(default)]
    owned_squad: Option<SquadId>,
}

impl Playing {
    fn capture(state: &State, owned_squad: Option<SquadId>) -> Self {
        Self {
            version: 1,
            clock: state.tally.cursor(),
            bindings: state.bindings.clone(),
            machines: state.machines.clone(),
            owned_squad,
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
            || self.bindings.len() != sim.matched.bots.len()
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
        drop(state);
        sim.lobby
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .resume_match(self.owned_squad);
        sim.stop.store(false, Ordering::Release);
        Ok(())
    }
}

pub(super) fn policy(sim: &Arc<Inner>, playback: bool) -> hooks::SnapshotPolicy {
    let check = sim.clone();
    let save = sim.clone();
    let load = sim.clone();
    hooks::SnapshotPolicy::section_forbid_while(
        move || {
            if !crate::theater::in_keyframe() || playback {
                return None;
            }
            let state = check.lock();
            let ready = state.phase == Phase::Playing && state.set_up && state.tally.ticks() > 0;
            drop(state);
            let lobby_ready = check
                .lobby
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .setup_complete();
            (!ready || !lobby_ready)
                .then(|| "sim lobby, seat binding or initial seed setup is in progress".into())
        },
        move || {
            if !crate::theater::in_keyframe() {
                return vec![]; // A normal state.load starts a fresh metrics tally.
            }
            let owner = save
                .lobby
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .owned_squad;
            serde_json::to_vec(&Playing::capture(&save.lock(), owner))
                .expect("integer sim film state")
        },
        move |bytes, _| {
            if !crate::theater::in_keyframe() && bytes.is_empty() {
                return Ok(());
            }
            if !playback || !crate::theater::in_keyframe() {
                return Err("sim playback state requires a film player".into());
            }
            let state: Playing = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
            state.restore(&load)
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim_glue::Sim;
    use sim_bench::metrics::TickSample;
    use std::path::PathBuf;

    #[test]
    fn restoring_an_active_sim_keeps_setup_latched_and_its_tick_limit_clock() {
        let config = sim_bench::config::parse_match(
            "map = \"lockout\"\nmax_ticks = 3\n[[bot]]\nkind = \"phantom\"\n[[bot]]\n",
            false,
        )
        .unwrap();
        let matched =
            sim_bench::config::resolve(config, &|_| Err("no tuning files".into())).unwrap();
        let source = Sim::new(PathBuf::from("match"), matched.clone(), None);
        let owner: SquadId = serde_json::from_value(serde_json::json!({
            "addr": 0x8040_0000u32, "incarnation": 7, "class": 1
        }))
        .unwrap();
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
            serde_json::to_vec(&Playing::capture(&s, Some(owner))).unwrap()
        };
        let fresh = Sim::new(PathBuf::from("film"), matched, None);
        serde_json::from_slice::<Playing>(&bytes)
            .unwrap()
            .restore(&fresh.0)
            .unwrap();
        assert_eq!(fresh.0.lobby.lock().unwrap().owned_squad, Some(owner));
        assert_eq!(fresh.status()["lobby"]["step"], "Loading");
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
        drop(s);

        // Loading a keyframe from a film without a saved owner must clear
        // the last film's identity, even if the guest addresses are reused.
        let mut legacy: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        legacy.as_object_mut().unwrap().remove("owned_squad");
        serde_json::from_value::<Playing>(legacy)
            .unwrap()
            .restore(&fresh.0)
            .unwrap();
        assert_eq!(fresh.0.lobby.lock().unwrap().owned_squad, None);
    }
}
