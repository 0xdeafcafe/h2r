//! Per-client scheduling in simulation ticks. Byte counts cover the actor
//! packet, including its header. The tunnel supplies its own framing.

use std::collections::VecDeque;
use std::fmt;

use crate::wire::{
    HEADER_LEN, MAX_KEY_RECORD_BYTES, MAX_PACKET_BYTES, MAX_RECORDS, Packet, Record, WireError,
    record_len,
};

pub const TICKS_PER_SECOND: u32 = 30;
pub const BYTES_PER_SECOND: usize = 1_000;
/// Room for a key, state, digest and several updates per actor in one host tick.
pub const MAX_CANDIDATES: usize = 256;
/// Copies a line or a cue is repeated in: this packet and the next two.
const REPEAT_COPIES: usize = 3;
const CONTROL_PERIOD: u32 = 3;
const MAX_KEY_PACKET: usize = HEADER_LEN + MAX_KEY_RECORD_BYTES;

#[derive(Debug)]
pub enum BudgetError {
    TooManyCandidates,
    MultipleKeys,
    InvalidRecord(WireError),
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooManyCandidates => write!(f, "more than {MAX_CANDIDATES} actor candidates"),
            Self::MultipleKeys => f.write_str("more than one actor key offered in a tick"),
            Self::InvalidRecord(error) => write!(f, "invalid actor candidate: {error}"),
        }
    }
}

impl std::error::Error for BudgetError {}

/// The first slot is this packet. The other slots hold the next two
/// packets' promised line and cue copies. Each nonempty slot pays its
/// header once.
#[derive(Clone, Copy, Default)]
struct Reservation {
    bytes: [usize; REPEAT_COPIES],
    records: [usize; REPEAT_COPIES],
}

impl Reservation {
    fn add(&mut self, len: usize, copies: usize) {
        for slot in 0..copies {
            if self.records[slot] == 0 {
                self.bytes[slot] += HEADER_LEN;
            }
            self.bytes[slot] += len;
            self.records[slot] += 1;
        }
    }

    fn try_add(&mut self, len: usize, copies: usize, available: usize) -> bool {
        let mut with_record = *self;
        with_record.add(len, copies);
        if with_record.bytes.iter().sum::<usize>() > available
            || with_record.bytes.iter().any(|&n| n > MAX_PACKET_BYTES)
            || with_record.records.iter().any(|&n| n > MAX_RECORDS)
            // Even if the next call skips straight to a key deadline, its
            // committed lines must leave a packet slot and room for that key.
            || with_record.bytes[1..].iter().sum::<usize>() > BYTES_PER_SECOND - MAX_KEY_PACKET
            || with_record.records[1..].iter().any(|&n| n >= MAX_RECORDS)
        {
            return false;
        }
        *self = with_record;
        true
    }
}

#[derive(Debug)]
struct Repeat {
    record: Record,
    len: usize,
    copies_left: usize,
}

#[derive(Debug, Default)]
pub struct Budget {
    last_tick: Option<u32>,
    // Countdown as of last_tick. Once due, it stays due across further wraps.
    ticks_to_key: Option<u32>,
    // At most one send per tick in the rolling window.
    sent: VecDeque<(u32, usize)>,
    repeats: Vec<Repeat>,
    // At most CONTROL_PERIOD packets of control IDs, even under entity churn.
    controls: VecDeque<(u32, u32)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Priority {
    Key,
    Actor,
    Line,
    Cue,
    Posture,
    State,
    Digest,
    Control,
}

fn priority(record: &Record) -> Priority {
    match record {
        Record::Key { .. } => Priority::Key,
        Record::Actor(_) => Priority::Actor,
        Record::Line { .. } => Priority::Line,
        Record::Cue { .. } => Priority::Cue,
        Record::Posture { .. } => Priority::Posture,
        Record::State(_) => Priority::State,
        Record::Digest { .. } => Priority::Digest,
        Record::Control { .. } => Priority::Control,
    }
}

/// Whether the record repeats in three consecutive packets, as a `Line` is.
fn repeats(record: &Record) -> bool {
    matches!(priority(record), Priority::Line | Priority::Cue)
}

impl Budget {
    /// A new peer needs its first key. Each admitted key, including a resync,
    /// starts a fresh period. Call with the tick about to be scheduled.
    pub fn key_due(&self, tick: u32) -> bool {
        self.remaining_key_ticks(tick).unwrap_or(0) == 0
    }

    fn remaining_key_ticks(&self, tick: u32) -> Option<u32> {
        let last = self.last_tick?;
        let remaining = self.ticks_to_key?;
        if crate::newer_tick(tick, last) {
            Some(remaining.saturating_sub(tick.wrapping_sub(last)))
        } else {
            Some(remaining)
        }
    }

    /// Hold a full key in the rolling window that ends at the deadline.
    /// Sends that expire before then can still use their whole allowance.
    fn available_for_deltas(&self, tick: u32, available: usize) -> usize {
        let Some(until) = self.ticks_to_key else {
            return available;
        };
        if until >= TICKS_PER_SECOND {
            return available;
        }
        let deadline = tick.wrapping_add(until);
        let charged_at_deadline: usize = self
            .sent
            .iter()
            .filter(|&&(at, _)| deadline.wrapping_sub(at) < TICKS_PER_SECOND)
            .map(|&(_, len)| len)
            .sum();
        available.min((BYTES_PER_SECOND - MAX_KEY_PACKET).saturating_sub(charged_at_deadline))
    }

    /// Call once per host tick for this client, including ticks with no new
    /// candidates. Consume the returned packet exactly once. Offer new lines
    /// and cues only once: admission reserves all three consecutive packet
    /// copies. After a skipped tick, outstanding copies resume in the next
    /// packet.
    ///
    /// Equal priorities retain input order. Offer controls nearest first,
    /// and omit them entirely when retail supplies the walk cycle. Reoffer
    /// a requested resync key built from the current state until the output
    /// contains it. If it cannot fit, new work waits while old bytes expire.
    /// Offer a key when [`Self::key_due`] is true. The preceding rolling
    /// window leaves enough bytes for a full key on its scheduled tick.
    /// Other candidates that miss admission are dropped.
    ///
    /// Repeated and older ticks return `Ok(None)`. Tick ordering uses the
    /// wrapping half range; start a fresh budget on a new session. Input
    /// errors leave the tick open for a corrected call.
    pub fn schedule(
        &mut self,
        tick: u32,
        candidates: &[Record],
    ) -> Result<Option<Packet>, BudgetError> {
        if self
            .last_tick
            .is_some_and(|last| !crate::newer_tick(tick, last))
        {
            return Ok(None);
        }
        if candidates.len() > MAX_CANDIDATES {
            return Err(BudgetError::TooManyCandidates);
        }
        let mut offers = Vec::with_capacity(candidates.len());
        let mut has_key = false;
        for record in candidates {
            let rank = priority(record);
            if rank == Priority::Key {
                if has_key {
                    return Err(BudgetError::MultipleKeys);
                }
                has_key = true;
            }
            let len = record_len(record).map_err(BudgetError::InvalidRecord)?;
            offers.push((rank, record, len));
        }

        self.ticks_to_key = self.remaining_key_ticks(tick);
        self.last_tick = Some(tick);
        while self
            .sent
            .front()
            .is_some_and(|&(at, _)| tick.wrapping_sub(at) >= TICKS_PER_SECOND)
        {
            self.sent.pop_front();
        }
        while self
            .controls
            .front()
            .is_some_and(|&(at, _)| tick.wrapping_sub(at) >= CONTROL_PERIOD)
        {
            self.controls.pop_front();
        }
        let available = BYTES_PER_SECOND - self.sent.iter().map(|&(_, len)| len).sum::<usize>();

        let mut delta_bytes = self.available_for_deltas(tick, available);
        let mut reserved = Reservation::default();
        for repeat in &self.repeats {
            reserved.add(repeat.len, repeat.copies_left);
        }
        let mut records = Vec::new();
        let mut waiting_for_key = false;
        for rank in [
            Priority::Key,
            Priority::Actor,
            Priority::Line,
            Priority::Cue,
            Priority::Posture,
            Priority::State,
            Priority::Digest,
            Priority::Control,
        ] {
            if matches!(rank, Priority::Line | Priority::Cue) {
                let mut owed = Vec::new();
                for mut repeat in std::mem::take(&mut self.repeats) {
                    if priority(&repeat.record) != rank {
                        owed.push(repeat);
                        continue;
                    }
                    repeat.copies_left -= 1;
                    if repeat.copies_left == 0 {
                        records.push(repeat.record);
                    } else {
                        records.push(repeat.record.clone());
                        owed.push(repeat);
                    }
                }
                self.repeats = owed;
            }
            if waiting_for_key {
                continue;
            }
            for &(offered_rank, record, len) in &offers {
                if offered_rank != rank {
                    continue;
                }
                if let Record::Control { entity, .. } = record
                    && self.controls.iter().any(|&(_, id)| id == *entity)
                {
                    continue;
                }
                let copies = if repeats(record) { REPEAT_COPIES } else { 1 };
                let limit = if rank == Priority::Key {
                    available
                } else {
                    delta_bytes
                };
                if !reserved.try_add(len, copies, limit) {
                    if rank == Priority::Key {
                        waiting_for_key = true;
                    }
                    continue;
                }
                if rank == Priority::Key {
                    self.ticks_to_key = Some(crate::KEY_PERIOD_TICKS);
                    delta_bytes = available;
                }
                if repeats(record) {
                    self.repeats.push(Repeat {
                        record: record.clone(),
                        len,
                        copies_left: REPEAT_COPIES - 1,
                    });
                }
                if let Record::Control { entity, .. } = record {
                    self.controls.push_back((tick, *entity));
                }
                records.push(record.clone());
            }
        }

        if records.is_empty() {
            return Ok(None);
        }
        self.sent.push_back((tick, reserved.bytes[0]));
        Ok(Some(Packet { tick, records }))
    }
}

#[cfg(test)]
mod tests;
