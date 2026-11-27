//! Squads that arrive by pod (docs/game/firefight.md, "Drop pods"): the
//! pool state of the kit's `ff_pod_NN` vehicles.
//!
//! A `DropSquad` with the pod arrival takes the first free pod and flies it
//! down its zone's fall in [`FALL_TICKS`]. When the fall is over the host
//! logs a `PlaceSquad`, so the squad is placed where it landed and a film
//! replays the same arrival at the same tick; the pod then stays a wreck for
//! [`WRECK_TICKS`] and is parked again. A pod that cannot be had is never a
//! stall: no free pod and no fall place the squad on foot, counted by reason.
//!
//! [`SquadPods`] is a pure machine. Every role that applies entries steps
//! it from the same entries and ticks, so the host, a replica and a film's
//! playback hold the same pool; only the host's [`SquadPods::due`] turns a
//! landing into a log entry.

use std::collections::BTreeMap;

use super::guest::PodFall;
use super::pods::{FALL_TICKS, WRECK_TICKS};
use super::ship::Launch;

/// Why a squad that was to fall was placed on foot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Fallback {
    /// The zone has no fall.
    NoFall,
    /// Every squad pod is in use or gone.
    NoFreePod,
}

impl Fallback {
    pub fn name(self) -> &'static str {
        match self {
            Fallback::NoFall => "no_fall",
            Fallback::NoFreePod => "no_free_pod",
        }
    }
}

/// A squad on its way down: what to place when it lands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Falling {
    pub slot: u8,
    pub class: String,
    pub count: u16,
    pub zone: u8,
    pub weapon_tier: u8,
    /// The tick the fall began.
    pub started: u32,
    /// The landing has been logged as a `PlaceSquad`.
    pub landed: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Slot {
    Free,
    Falling(Falling),
    /// Landed and emptied, or its squad erased: parks at `until`.
    Wreck { until: u32 },
}

/// Counters for `firefight.state`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Squads that fell in a pod.
    pub drops: u32,
    /// Squads placed on foot instead, by reason.
    pub fallbacks: BTreeMap<Fallback, u32>,
    /// Wrecks parked again.
    pub reparks: u32,
}

/// The squad pods of one match.
#[derive(Clone, Debug)]
pub struct SquadPods {
    slots: Vec<Slot>,
    counts: Counts,
}

impl SquadPods {
    /// The machine for a kit with `pods` squad pods.
    pub fn new(pods: usize) -> Self {
        Self {
            slots: vec![Slot::Free; pods],
            counts: Counts::default(),
        }
    }

    pub fn counts(&self) -> &Counts {
        &self.counts
    }

    /// Pods free and in the world: what the director may drop squads in.
    /// `pool` is each pod's live datum, in `ff_pod_NN` order.
    pub fn free(&self, pool: &[Option<u32>]) -> u8 {
        let n = self
            .slots
            .iter()
            .enumerate()
            .filter(|(i, s)| **s == Slot::Free && pool.get(*i).is_some_and(Option::is_some))
            .count();
        u8::try_from(n).unwrap_or(u8::MAX)
    }

    /// Count a squad placed on foot instead of by pod.
    pub fn fell_back(&mut self, reason: Fallback) {
        *self.counts.fallbacks.entry(reason).or_default() += 1;
    }

    /// Begin a squad's fall at `tick`: the first free pod of `pool` flies
    /// down `fall`. `None` when no pod is free, counted as a fallback.
    pub fn start(
        &mut self,
        pool: &[Option<u32>],
        fall: PodFall,
        tick: u32,
        squad: Falling,
    ) -> Option<Launch> {
        let free = (0..self.slots.len())
            .find(|&i| self.slots[i] == Slot::Free && pool.get(i).is_some_and(Option::is_some));
        let Some(i) = free else {
            self.fell_back(Fallback::NoFreePod);
            return None;
        };
        let object = pool[i].expect("chosen for it");
        self.slots[i] = Slot::Falling(Falling { started: tick, landed: false, ..squad });
        self.counts.drops += 1;
        Some(Launch {
            ship: object,
            path: vec![fall.top, fall.ground],
            ticks: FALL_TICKS,
        })
    }

    /// The squads whose fall is over by `tick` and whose landing is not yet
    /// logged, marked as logged: the host appends a `PlaceSquad` for each.
    pub fn due(&mut self, tick: u32) -> Vec<Falling> {
        let mut out = Vec::new();
        for slot in &mut self.slots {
            if let Slot::Falling(f) = slot
                && !f.landed
                && tick >= f.started.saturating_add(FALL_TICKS)
            {
                f.landed = true;
                out.push(f.clone());
            }
        }
        out
    }

    /// The squad of `squad_slot` was placed (its landing was applied): the
    /// pod is a wreck from `tick`. A placement of any other squad is none
    /// of the pods'.
    pub fn landed(&mut self, squad_slot: u8, tick: u32) {
        for slot in &mut self.slots {
            if let Slot::Falling(f) = slot
                && f.slot == squad_slot
            {
                *slot = Slot::Wreck { until: tick.saturating_add(WRECK_TICKS) };
            }
        }
    }

    /// The squad of `squad_slot` was erased before it landed: no landing is
    /// placed, and the pod parks [`WRECK_TICKS`] after its fall ends.
    pub fn cancel(&mut self, squad_slot: u8) {
        for slot in &mut self.slots {
            if let Slot::Falling(f) = slot
                && f.slot == squad_slot
            {
                let until = f.started.saturating_add(FALL_TICKS + WRECK_TICKS);
                *slot = Slot::Wreck { until };
            }
        }
    }

    /// After a tick's entries: the pods whose wreck is over go back. Each
    /// answer is the pod's datum, to put at its parking place.
    pub fn parked(&mut self, tick: u32, pool: &[Option<u32>]) -> Vec<u32> {
        let mut out = Vec::new();
        for (i, slot) in self.slots.iter_mut().enumerate() {
            if matches!(slot, Slot::Wreck { until } if tick >= *until) {
                if let Some(Some(object)) = pool.get(i) {
                    out.push(*object);
                }
                *slot = Slot::Free;
                self.counts.reparks += 1;
            }
        }
        out
    }
}
