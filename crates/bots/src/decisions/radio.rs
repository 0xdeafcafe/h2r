//! The host's local and phantom pads share one radio. Publish into this
//! tick, listen to completed ticks only, so pad polling order is irrelevant.
//!
//! Almost: a bot's own deaths prune pending calls ([`Radio::forget_deaths`]),
//! and a pad sampled later in the same tick prunes the calls of the bots
//! sampled before it. A batch ([`Radio::begin_batch`], `crate::think`)
//! listens for every bot first and publishes after they have all thought;
//! it keeps each listener's place ([`Seat`]) and the deaths it pruned with,
//! so a call published late is pruned exactly as the serial order would
//! have pruned it.
use std::sync::{Mutex, MutexGuard, OnceLock};

use super::supplies::Supply;
use crate::brain::{Perception, Vec3};

const CALL_TICKS: u32 = 75;
pub(super) const MAX_REPORT_INTERVAL: u32 = 24;

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

/// Rate-limit repeat sightings. New targets and urgent help go out immediately.
#[derive(Clone, Debug, Default)]
pub(super) struct Voice(Option<Call>);

impl Voice {
    pub fn report(&mut self, call: Option<Call>, interval: u32) -> Option<Call> {
        let Some(call) = call else {
            self.0 = None;
            return None;
        };
        if self.0.is_some_and(|old| {
            old.target == call.target
                && (!call.help || old.help)
                && call.tick.wrapping_sub(old.tick) < interval
        }) {
            return None;
        }
        self.0 = Some(call);
        Some(call)
    }
}

#[derive(Default)]
pub(crate) struct Radio {
    pub epoch: u64,
    tick: Option<u32>,
    ready: Vec<Call>,
    pending: Vec<Call>,
    supplies: Vec<SupplyCall>,
    pending_supplies: Vec<SupplyCall>,
    batch: Option<Batch>,
}

/// One batch of listeners: each one's place in seat order and the deaths
/// it pruned pending calls with.
#[derive(Default)]
struct Batch {
    next: u32,
    forgets: Vec<(u32, Vec<crate::emote::Kill>)>,
}

/// A listener's place in a batch and the board as the serial order would
/// have left it for that listener's publish: right after its own listen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Seat {
    seq: u32,
    tick: Option<u32>,
    epoch: u64,
}

impl Seat {
    /// Whether a serial publish right after this seat's listen would have
    /// reached the board: same match, and the board on the call's tick.
    pub fn accepts(self, epoch: u64, tick: u32) -> bool {
        self.epoch == epoch && self.tick == Some(tick)
    }
}

fn survives(kills: &[crate::emote::Kill], c: &Call) -> bool {
    !kills
        .iter()
        .any(|k| k.tick >= c.tick && (k.victim == c.sender || k.victim == c.target))
}

impl Radio {
    pub fn reset(&mut self) {
        self.batch = None;
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
        self.ready.retain(|c| survives(kills, c));
        self.pending.retain(|c| survives(kills, c));
    }

    /// Opens a batch: the listeners that follow take places in order.
    /// Replaces any batch still open.
    pub fn begin_batch(&mut self) {
        self.batch = Some(Batch::default());
    }

    pub fn end_batch(&mut self) {
        self.batch = None;
    }

    /// The next place in the open batch, if one is open.
    pub fn take_seat(&mut self) -> Option<u32> {
        let b = self.batch.as_mut()?;
        let seq = b.next;
        b.next += 1;
        Some(seq)
    }

    /// The seat `seq` with the board as it is now (after its listen).
    pub fn seat(&self, seq: u32) -> Seat {
        Seat {
            seq,
            tick: self.tick,
            epoch: self.epoch,
        }
    }

    /// Records the deaths seat `seq` pruned with, for the calls published
    /// by earlier seats after it ([`Radio::publish_seated`]).
    pub fn note_forget(&mut self, seq: u32, kills: &[crate::emote::Kill]) {
        if kills.is_empty() {
            return;
        }
        if let Some(b) = self.batch.as_mut() {
            b.forgets.push((seq, kills.to_vec()));
        }
    }

    /// [`Radio::publish`] from a batch seat: the call goes in as if the
    /// seat had published before the later seats listened, so their deaths
    /// prune it. The caller checks [`Seat::accepts`].
    pub fn publish_seated(&mut self, seat: Seat, call: Call) {
        if self.tick != Some(call.tick) {
            return;
        }
        self.pending.retain(|c| c.sender != call.sender);
        let pruned = self.batch.as_ref().is_some_and(|b| {
            b.forgets
                .iter()
                .any(|(seq, kills)| *seq > seat.seq && !survives(kills, &call))
        });
        if !pruned {
            self.pending.push(call);
        }
    }

    #[cfg(test)]
    pub fn digest(&self) -> (u64, Option<u32>, Vec<Call>, Vec<Call>, usize, usize) {
        (
            self.epoch,
            self.tick,
            self.ready.clone(),
            self.pending.clone(),
            self.supplies.len(),
            self.pending_supplies.len(),
        )
    }

    #[cfg(test)]
    pub fn listen(&self, p: &Perception) -> Vec<Call> {
        let mut calls = Vec::new();
        self.listen_into(p, &mut calls);
        calls
    }

    pub fn listen_into(&self, p: &Perception, calls: &mut Vec<Call>) {
        calls.clear();
        let Some((id, team)) = p.observations.player.filter(|_| p.teams) else {
            return;
        };
        calls.extend(self.ready.iter().copied().filter(|c| {
            c.sender != id
                && c.team == team
                && p.others.iter().any(|o| o.id == c.sender && o.team == team)
        }));
    }

    #[cfg(test)]
    pub fn supply_calls(&self, p: &Perception) -> Vec<SupplyCall> {
        let mut calls = Vec::new();
        self.supply_calls_into(p, &mut calls);
        calls
    }

    pub fn supply_calls_into(&self, p: &Perception, calls: &mut Vec<SupplyCall>) {
        calls.clear();
        let Some((id, team)) = p.observations.player.filter(|_| p.teams) else {
            return;
        };
        calls.extend(
            self.supplies
                .iter()
                .copied()
                .filter(|c| c.sender != id && c.team == team),
        );
        calls.sort_by_key(|c| (c.tick, c.sender));
    }

    pub fn publish_supply(&mut self, c: SupplyCall, now: u32) {
        if self.tick == Some(now) && now.wrapping_sub(c.tick) <= CALL_TICKS {
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

#[cfg(test)]
mod batch_tests {
    use super::*;
    use crate::emote::Kill;

    fn call(sender: u32, target: u32, tick: u32) -> Call {
        Call {
            sender,
            team: 0,
            target,
            at: [0.0; 3],
            tick,
            help: false,
        }
    }

    fn death(victim: u32, tick: u32) -> Kill {
        Kill {
            seq: 1,
            tick,
            victim,
            attacker: None,
            betrayal: false,
            at: None,
        }
    }

    /// Serial: listen 0, publish 0, listen 1 (which prunes 0's call).
    /// Batched: listen 0, listen 1, publish 0, publish 1. Same pending.
    #[test]
    fn a_seated_publish_is_pruned_by_later_listeners_only() {
        let kills = [death(9, 5)];
        let mut serial = Radio::default();
        serial.advance(5);
        serial.publish(call(0, 9, 5));
        serial.forget_deaths(&kills);
        serial.publish(call(1, 9, 5));

        let mut batched = Radio::default();
        batched.begin_batch();
        let s0 = batched.take_seat().unwrap();
        batched.advance(5);
        let seat0 = batched.seat(s0);
        let s1 = batched.take_seat().unwrap();
        batched.forget_deaths(&kills);
        batched.note_forget(s1, &kills);
        let seat1 = batched.seat(s1);
        assert!(seat0.accepts(batched.epoch, 5));
        batched.publish_seated(seat0, call(0, 9, 5));
        batched.publish_seated(seat1, call(1, 9, 5));
        assert_eq!(serial.digest(), batched.digest());
        assert_eq!(batched.pending, [call(1, 9, 5)]);
    }

    #[test]
    fn a_seat_before_the_board_reached_the_tick_publishes_nothing() {
        let mut r = Radio::default();
        r.advance(4);
        r.begin_batch();
        let s0 = r.take_seat().unwrap();
        // Seat 0 had no player and did not advance; seat 1 did.
        let seat0 = r.seat(s0);
        r.take_seat();
        r.advance(5);
        assert!(!seat0.accepts(r.epoch, 5));
    }
}
