//! The host's local and phantom pads share one radio. Publish into this
//! tick, listen to completed ticks only, so pad polling order is irrelevant.
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::supplies::Supply;
use crate::brain::{Perception, Vec3};

const CALL_TICKS: u32 = 75;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Call {
    pub sender: u32,
    pub team: i16,
    pub target: u32,
    pub at: Vec3,
    pub tick: u32,
    pub help: bool,
}

#[derive(Clone, Copy)]
pub(crate) struct SupplyCall {
    pub sender: u32,
    pub team: i16,
    pub item: Supply,
    pub present: bool,
    pub tick: u32,
}

#[derive(Default)]
pub(crate) struct Radio {
    pub epoch: u64,
    tick: Option<u32>,
    ready: Vec<Call>,
    pending: Vec<Call>,
    supplies: Vec<SupplyCall>,
    pending_supplies: Vec<SupplyCall>,
}

impl Radio {
    pub fn reset(&mut self) {
        self.epoch = self.epoch.wrapping_add(1);
        self.tick = None;
        self.ready.clear();
        self.pending.clear();
        self.supplies.clear();
        self.pending_supplies.clear();
    }

    pub fn advance(&mut self, tick: u32) {
        if self.tick == Some(tick) {
            return;
        }
        if self.tick.is_some_and(|last| tick < last) {
            self.reset();
        }
        self.tick = Some(tick);
        self.supplies.retain(|c| tick - c.tick <= CALL_TICKS);
        for c in self.pending_supplies.drain(..) {
            self.supplies.retain(|old| {
                (old.team, old.item.site, old.sender) != (c.team, c.item.site, c.sender)
            });
            if tick - c.tick <= CALL_TICKS {
                self.supplies.push(c);
            }
        }
        self.ready.retain(|c| tick - c.tick <= CALL_TICKS);
        for call in self.pending.drain(..) {
            self.ready.retain(|c| c.sender != call.sender);
            if tick - call.tick <= CALL_TICKS {
                self.ready.push(call);
            }
        }
    }

    pub fn forget_deaths(&mut self, kills: &[crate::emote::Kill]) {
        if kills.is_empty() {
            return;
        }
        let surviving = |c: &Call| {
            !kills
                .iter()
                .any(|k| k.tick >= c.tick && (k.victim == c.sender || k.victim == c.target))
        };
        self.ready.retain(surviving);
        self.pending.retain(surviving);
    }

    pub fn listen(&self, p: &Perception) -> Vec<Call> {
        let Some((id, team)) = p.observations.player.filter(|_| p.teams) else {
            return Vec::new();
        };
        self.ready
            .iter()
            .copied()
            .filter(|c| {
                c.sender != id
                    && c.team == team
                    && p.others.iter().any(|o| o.id == c.sender && o.team == team)
            })
            .collect()
    }

    pub fn supply_calls(&self, p: &Perception) -> Vec<SupplyCall> {
        let Some((id, team)) = p.observations.player.filter(|_| p.teams) else {
            return vec![];
        };
        let mut calls: Vec<_> = self
            .supplies
            .iter()
            .copied()
            .filter(|c| c.sender != id && c.team == team)
            .collect();
        calls.sort_by_key(|c| (c.tick, c.sender));
        calls
    }

    pub fn publish_supply(&mut self, c: SupplyCall) {
        if self.tick == Some(c.tick) {
            self.pending_supplies.push(c);
        }
    }

    pub fn publish(&mut self, call: Call) {
        if self.tick == Some(call.tick) {
            self.pending.retain(|c| c.sender != call.sender);
            self.pending.push(call);
        }
    }
}

pub(crate) fn host() -> &'static Mutex<Radio> {
    static RADIO: OnceLock<Mutex<Radio>> = OnceLock::new();
    RADIO.get_or_init(Mutex::default)
}

pub(crate) fn lock(board: &Mutex<Radio>) -> MutexGuard<'_, Radio> {
    board.lock().unwrap_or_else(|e| e.into_inner())
}

pub(crate) fn map_loaded() {
    lock(host()).reset();
}
