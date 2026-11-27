//! The active sounds and their end ticks. A sound ends on
//! `start + duration_ticks(duration_ms)`, the engine's own rounding; there is no voice, priority or
//! culling state, so the schedule is the same whether or not anything is
//! listening (docs/engine/sound-sim.md).

use crate::events::{SoundDef, SoundEvent, SoundId, Source, TICKS_PER_SECOND};

/// The engine's `0.001f32` (`0x45dc70`): milliseconds to seconds.
const MS_TO_SECONDS: f32 = f32::from_bits(0x3a83_126f);

/// Ticks a sound of `duration_ms` runs, as the engine computes them
/// (`0x18a13b`, `0x18a1c0`): the `snd!` `+0x10` milliseconds as an `f32`,
/// times `0.001f32`, times the tick rate as an `f32`, then `fistp` (round to
/// nearest, ties to even). Not a ceiling: 1 ms is 0 ticks, 17 ms is 1.
pub fn duration_ticks(duration_ms: u32) -> u32 {
    let ms = duration_ms as i32 as f32;
    let ticks = (ms * MS_TO_SECONDS * TICKS_PER_SECOND as f32).round_ties_even();
    if ticks > 0.0 { ticks as u32 } else { 0 }
}

/// The tick a sound of `duration_ms` started on `start` ends on.
pub fn end_tick(start: u32, duration_ms: u32) -> u32 {
    start.saturating_add(duration_ticks(duration_ms))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Active {
    id: SoundId,
    source: Source,
    tag: u32,
    start: u32,
    /// `None` for a looping sound.
    end: Option<u32>,
}

/// Why a save did not restore.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestoreError {
    /// The bytes end inside a record.
    Short,
    /// Bytes remain after the last record.
    Trailing,
    /// A record's end-tick flag is neither 0 nor 1.
    BadFlag(u8),
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Schedule {
    next: u32,
    /// In start order.
    active: Vec<Active>,
    /// Starts and stops since the last `step`, in call order.
    pending: Vec<SoundEvent>,
}

impl Schedule {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a sound on `tick`.
    pub fn start(&mut self, def: SoundDef, source: Source, tick: u32) -> SoundId {
        self.next += 1;
        let id = SoundId(self.next);
        let end = if def.looping { None } else { Some(end_tick(tick, def.duration_ms.unwrap_or(0))) };
        self.active.push(Active { id, source, tag: def.tag, start: tick, end });
        self.pending.push(SoundEvent::Started { id, source, tag: def.tag, looping: def.looping, tick });
        id
    }

    /// Stops a sound before its end. False when it is not active.
    pub fn stop(&mut self, id: SoundId, tick: u32) -> bool {
        let Some(at) = self.active.iter().position(|a| a.id == id) else { return false };
        self.active.remove(at);
        self.pending.push(SoundEvent::Stopped { id, tick });
        true
    }

    /// Runs `tick`: returns the starts and stops since the last step in call
    /// order, then the sounds whose end tick has come, in start order.
    pub fn step(&mut self, tick: u32) -> Vec<SoundEvent> {
        let mut out = std::mem::take(&mut self.pending);
        self.active.retain(|a| match a.end {
            Some(end) if end <= tick => {
                out.push(SoundEvent::Ended { id: a.id, tick });
                false
            }
            _ => true,
        });
        out
    }

    pub fn is_active(&self, id: SoundId) -> bool {
        self.active.iter().any(|a| a.id == id)
    }

    /// The tick a sound ends on: `None` for a looping sound or one not active.
    pub fn ends_at(&self, id: SoundId) -> Option<u32> {
        self.active.iter().find(|a| a.id == id).and_then(|a| a.end)
    }

    /// What a script waiting on the sound sees at `tick`: finished once the
    /// sound is gone, or once its computed end tick is not after `tick`. A
    /// looping sound is never finished while it is active.
    pub fn finished(&self, id: SoundId, tick: u32) -> bool {
        match self.active.iter().find(|a| a.id == id) {
            None => true,
            Some(a) => a.end.is_some_and(|end| end <= tick),
        }
    }

    /// The ticks left on a sound at `tick` (0 once finished), for a script's
    /// "time remaining" query; `None` for a looping sound.
    pub fn ticks_left(&self, id: SoundId, tick: u32) -> Option<u32> {
        match self.active.iter().find(|a| a.id == id) {
            None => Some(0),
            Some(a) => a.end.map(|end| end.saturating_sub(tick)),
        }
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// The canonical bytes of the active sounds. Starts and stops not yet
    /// stepped are not saved: save after `step`.
    pub fn save(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(8 + self.active.len() * 21);
        out.extend_from_slice(&self.next.to_le_bytes());
        out.extend_from_slice(&(self.active.len() as u32).to_le_bytes());
        for a in &self.active {
            for word in [a.id.0, a.source.0, a.tag, a.start] {
                out.extend_from_slice(&word.to_le_bytes());
            }
            match a.end {
                Some(end) => {
                    out.push(1);
                    out.extend_from_slice(&end.to_le_bytes());
                }
                None => {
                    out.push(0);
                    out.extend_from_slice(&0u32.to_le_bytes());
                }
            }
        }
        out
    }

    pub fn restore(bytes: &[u8]) -> Result<Schedule, RestoreError> {
        let mut at = 0usize;
        let word = |at: &mut usize| -> Result<u32, RestoreError> {
            let b = bytes.get(*at..*at + 4).ok_or(RestoreError::Short)?;
            *at += 4;
            Ok(u32::from_le_bytes(b.try_into().expect("four bytes")))
        };
        let next = word(&mut at)?;
        let count = word(&mut at)?;
        let mut active = Vec::new();
        for _ in 0..count {
            let id = SoundId(word(&mut at)?);
            let source = Source(word(&mut at)?);
            let tag = word(&mut at)?;
            let start = word(&mut at)?;
            let flag = *bytes.get(at).ok_or(RestoreError::Short)?;
            at += 1;
            let end = word(&mut at)?;
            let end = match flag {
                0 => None,
                1 => Some(end),
                other => return Err(RestoreError::BadFlag(other)),
            };
            active.push(Active { id, source, tag, start, end });
        }
        if at != bytes.len() {
            return Err(RestoreError::Trailing);
        }
        Ok(Schedule { next, active, pending: Vec::new() })
    }

    /// FNV-1a 64 over [`Schedule::save`]: the sim's sound state in one number.
    pub fn hash(&self) -> u64 {
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for b in self.save() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        h
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn def(ms: u32) -> SoundDef {
        SoundDef { tag: 0xe1e1_0001, duration_ms: Some(ms), looping: false }
    }

    #[test]
    fn end_tick_from_duration() {
        assert_eq!(end_tick(100, 0), 100);
        assert_eq!(end_tick(100, 1), 100);
        assert_eq!(end_tick(100, 16), 100);
        assert_eq!(end_tick(100, 17), 101);
        assert_eq!(end_tick(100, 51), 102);
        assert_eq!(end_tick(100, 352), 111);
        assert_eq!(end_tick(100, 1000), 130);
        assert_eq!(end_tick(u32::MAX - 1, 1000), u32::MAX);
        let mut s = Schedule::new();
        let id = s.start(def(500), Source(7), 10);
        assert_eq!(s.ends_at(id), Some(25));
        assert!(s.step(24).iter().all(|e| !matches!(e, SoundEvent::Ended { .. })));
        assert!(s.step(25).contains(&SoundEvent::Ended { id, tick: 25 }));
        assert!(!s.is_active(id));
    }

    #[test]
    fn looping_sound_never_ends() {
        let mut s = Schedule::new();
        let id = s.start(SoundDef { tag: 1, duration_ms: Some(10), looping: true }, Source::NONE, 0);
        for tick in [1, 30, 100_000, u32::MAX] {
            assert!(s.step(tick).iter().all(|e| !matches!(e, SoundEvent::Ended { .. })));
        }
        assert!(s.is_active(id));
        assert_eq!(s.ends_at(id), None);
        assert!(!s.finished(id, u32::MAX));
        assert!(s.stop(id, 5));
        assert!(s.finished(id, 5));
    }

    #[test]
    fn stop_cancels_end() {
        let mut s = Schedule::new();
        let id = s.start(def(1000), Source(1), 0);
        s.step(0);
        assert!(s.stop(id, 3));
        assert!(!s.stop(id, 3));
        assert_eq!(s.step(3), vec![SoundEvent::Stopped { id, tick: 3 }]);
        assert!(s.step(60).is_empty());
    }

    #[test]
    fn events_in_start_order() {
        let mut s = Schedule::new();
        let a = s.start(def(100), Source(1), 0);
        let b = s.start(def(100), Source(2), 0);
        let c = s.start(def(10), Source(3), 0);
        let ev = s.step(0);
        assert_eq!(ev.iter().map(SoundEvent::id).collect::<Vec<_>>(), [a, b, c]);
        // Sounds due on one tick end in start order.
        let ev = s.step(3);
        assert_eq!(ev, vec![SoundEvent::Ended { id: a, tick: 3 }, SoundEvent::Ended { id: b, tick: 3 }, SoundEvent::Ended { id: c, tick: 3 }]);
    }

    #[test]
    fn save_round_trips_active_sounds() {
        let mut s = Schedule::new();
        s.start(def(900), Source(4), 2);
        s.start(SoundDef { tag: 9, duration_ms: None, looping: true }, Source::NONE, 3);
        let gone = s.start(def(1), Source(5), 3);
        s.step(4);
        assert!(!s.is_active(gone));
        let bytes = s.save();
        let back = Schedule::restore(&bytes).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.save(), bytes);
        assert_eq!(back.hash(), s.hash());
        // Ids go on from the saved counter.
        let mut back = back;
        assert_eq!(back.start(def(1), Source(6), 9), SoundId(4));
        assert_eq!(Schedule::restore(&bytes[..bytes.len() - 1]), Err(RestoreError::Short));
        let mut long = bytes.clone();
        long.push(0);
        assert_eq!(Schedule::restore(&long), Err(RestoreError::Trailing));
    }
}
