//! The voices of the sound sim's per-tick update (docs/engine/sim-rewrite.md,
//! "Sound sim (phase 4)"): which voices start on a tick and which play out
//! on it, in the guest's order, from data this module owns. It reads no
//! guest memory; the adapter (`sim_bridge::sim_sound::voice_arm`) feeds it
//! the guest's starts, stops and finishes and compares its reports with the
//! guest's.
//!
//! - A start enters a voice with its length. A one-shot plays out on
//!   `end_tick(start, ms)` ([`crate::schedule::end_tick`]).
//! - A loop, or a voice whose flags leave its end to another path, never
//!   plays out: only a stop or the guest ends it.
//! - A stop, or a guest finish for any reason but a play-out, interrupts the
//!   voice: an input, never a prediction.
//! - Play-outs come in pool slot order, the order of the per-instance walk
//!   (`0x128020`); starts in admission order.

use std::collections::BTreeMap;

use crate::schedule::end_tick;

/// The guest's salted instance handle: salt in the high half, slot low.
pub type Instance = u32;

/// Ticks a played-out voice stays known, so a late guest play-out of it is
/// still a difference and not an unknown voice.
pub const RETIRE_TICKS: u32 = 30 * 60;

/// How a voice ends without a stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Length {
    /// A one-shot of this many milliseconds.
    Ms(u32),
    /// No play-out: a loop, or flags that leave the end to another path.
    Loop,
    /// The length could not be read: the update predicts nothing for it.
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Playing,
    /// Stopped or cut short by the guest.
    Interrupted,
    /// The update finished it itself ([`Voices::due`]).
    Answered,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Voice {
    start: u32,
    length: Length,
    state: State,
}

impl Voice {
    fn end(&self) -> Option<u32> {
        match self.length {
            Length::Ms(ms) => Some(end_tick(self.start, ms)),
            Length::Loop | Length::Unknown => None,
        }
    }

    fn plays_out_on(&self, tick: u32) -> bool {
        self.state != State::Interrupted && self.end() == Some(tick)
    }
}

/// One tick's voice events in the guest's order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// In admission order.
    pub started: Vec<Instance>,
    /// In pool slot order.
    pub completed: Vec<Instance>,
}

impl Report {
    pub fn is_empty(&self) -> bool {
        self.started.is_empty() && self.completed.is_empty()
    }
}

/// What a guest finish was to the update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Finish {
    /// A tracked voice played out (on its predicted tick or not).
    PlayedOut,
    /// A stopped voice, or a finish for another reason.
    Interrupted,
    /// A play-out the update cannot predict: a voice it never saw start, or
    /// one of unknown length.
    Untracked,
}

fn slot(i: Instance) -> u32 {
    i & 0xffff
}

/// The running voices.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Voices {
    voices: BTreeMap<Instance, Voice>,
    /// Played-out voices and the tick they played out on.
    retired: BTreeMap<Instance, u32>,
    /// Starts not yet reported, with their tick, in admission order.
    pending: Vec<(u32, Instance)>,
}

impl Voices {
    pub const fn new() -> Self {
        Voices { voices: BTreeMap::new(), retired: BTreeMap::new(), pending: Vec::new() }
    }

    /// The guest admitted `instance` on `tick`.
    pub fn start(&mut self, tick: u32, instance: Instance, length: Length) {
        self.retired.remove(&instance);
        self.voices.insert(instance, Voice { start: tick, length, state: State::Playing });
        self.pending.push((tick, instance));
    }

    /// The guest stopped `instance`: it will not play out. False for a voice
    /// the update does not hold.
    pub fn stop(&mut self, instance: Instance) -> bool {
        match self.voices.get_mut(&instance) {
            Some(v) if v.state == State::Playing => {
                v.state = State::Interrupted;
                true
            }
            _ => false,
        }
    }

    /// The guest finished `instance`; `played_out` is the play-out reason.
    /// A play-out leaves the voice to [`Voices::step`], so one on another
    /// tick than predicted shows as a difference there.
    pub fn guest_finish(&mut self, instance: Instance, played_out: bool) -> Finish {
        if !played_out {
            self.voices.remove(&instance);
            self.retired.remove(&instance);
            return Finish::Interrupted;
        }
        match self.voices.get(&instance) {
            Some(v) if v.state == State::Interrupted => {
                self.voices.remove(&instance);
                Finish::Interrupted
            }
            Some(v) if v.length == Length::Unknown => {
                self.voices.remove(&instance);
                Finish::Untracked
            }
            Some(_) => Finish::PlayedOut,
            None if self.retired.contains_key(&instance) => Finish::PlayedOut,
            None => Finish::Untracked,
        }
    }

    /// The voices that play out on `tick` and that the update has not
    /// finished yet, in slot order. They are marked answered: the caller
    /// finishes them.
    pub fn due(&mut self, tick: u32) -> Vec<Instance> {
        let mut due: Vec<Instance> = self
            .voices
            .iter()
            .filter(|(_, v)| v.state == State::Playing && v.end() == Some(tick))
            .map(|(&i, _)| i)
            .collect();
        due.sort_by_key(|&i| (slot(i), i));
        for i in &due {
            if let Some(v) = self.voices.get_mut(i) {
                v.state = State::Answered;
            }
        }
        due
    }

    /// The update for `tick`: its starts and its play-outs, which leave the
    /// running set. Ticks are stepped in order; a voice whose end passed
    /// unstepped leaves silently.
    pub fn step(&mut self, tick: u32) -> Report {
        let (started, later): (Vec<_>, Vec<_>) = self.pending.drain(..).partition(|&(t, _)| t <= tick);
        self.pending = later;
        let started = started.into_iter().filter(|&(t, _)| t == tick).map(|(_, i)| i).collect();
        let mut completed: Vec<Instance> =
            self.voices.iter().filter(|(_, v)| v.plays_out_on(tick)).map(|(&i, _)| i).collect();
        completed.sort_by_key(|&i| (slot(i), i));
        for &i in &completed {
            self.voices.remove(&i);
            self.retired.insert(i, tick);
        }
        self.voices.retain(|_, v| v.end().is_none_or(|end| end > tick));
        self.retired.retain(|_, t| tick.saturating_sub(*t) <= RETIRE_TICKS);
        Report { started, completed }
    }

    /// Forget every voice (a new map, or the tick going back).
    pub fn clear(&mut self) {
        *self = Voices::new();
    }

    pub fn running(&self) -> usize {
        self.voices.len()
    }

    /// Canonical bytes: the voices, the retired voices, the pending starts.
    pub fn save(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let mut put = |w: u32| out.extend_from_slice(&w.to_le_bytes());
        put(self.voices.len() as u32);
        for (&i, v) in &self.voices {
            let (kind, ms) = match v.length {
                Length::Ms(ms) => (0, ms),
                Length::Loop => (1, 0),
                Length::Unknown => (2, 0),
            };
            let state = match v.state {
                State::Playing => 0,
                State::Interrupted => 1,
                State::Answered => 2,
            };
            for w in [i, v.start, kind, ms, state] {
                put(w);
            }
        }
        put(self.retired.len() as u32);
        for (&i, &t) in &self.retired {
            put(i);
            put(t);
        }
        put(self.pending.len() as u32);
        for &(t, i) in &self.pending {
            put(t);
            put(i);
        }
        out
    }

    pub fn restore(bytes: &[u8]) -> Result<Voices, String> {
        let mut words = bytes.chunks(4);
        if bytes.len() % 4 != 0 {
            return Err("voices: not whole words".into());
        }
        let mut next = || -> Result<u32, String> {
            words.next().map(|w| u32::from_le_bytes(w.try_into().expect("4 bytes"))).ok_or_else(|| "voices: short".into())
        };
        let mut out = Voices::new();
        for _ in 0..next()? {
            let (i, start, kind, ms, state) = (next()?, next()?, next()?, next()?, next()?);
            let length = match kind {
                0 => Length::Ms(ms),
                1 => Length::Loop,
                2 => Length::Unknown,
                k => return Err(format!("voices: length kind {k}")),
            };
            let state = match state {
                0 => State::Playing,
                1 => State::Interrupted,
                2 => State::Answered,
                s => return Err(format!("voices: state {s}")),
            };
            out.voices.insert(i, Voice { start, length, state });
        }
        for _ in 0..next()? {
            let (i, t) = (next()?, next()?);
            out.retired.insert(i, t);
        }
        for _ in 0..next()? {
            let (t, i) = (next()?, next()?);
            out.pending.push((t, i));
        }
        if next().is_ok() {
            return Err("voices: trailing bytes".into());
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Slot `slot`, salt `salt`.
    fn h(salt: u32, slot: u32) -> Instance {
        (salt << 16) | slot
    }

    #[test]
    fn a_start_is_reported_on_its_tick_in_admission_order() {
        let mut v = Voices::new();
        v.start(10, h(7, 5), Length::Ms(1000));
        v.start(10, h(7, 1), Length::Ms(1000));
        v.start(11, h(7, 2), Length::Ms(1000));
        assert_eq!(v.step(10).started, vec![h(7, 5), h(7, 1)]);
        assert_eq!(v.step(11).started, vec![h(7, 2)]);
        assert!(v.step(12).is_empty());
        assert_eq!(v.running(), 3);
    }

    #[test]
    fn a_one_shot_plays_out_on_its_end_tick() {
        let mut v = Voices::new();
        // 1000 ms is 30 ticks; 16 ms rounds to 0 and plays out on its start tick.
        v.start(5, h(1, 1), Length::Ms(1000));
        v.start(5, h(1, 2), Length::Ms(16));
        assert_eq!(v.step(5).completed, vec![h(1, 2)]);
        for t in 6..35 {
            assert!(v.step(t).completed.is_empty(), "tick {t}");
        }
        assert_eq!(v.step(35).completed, vec![h(1, 1)]);
        assert_eq!(v.running(), 0);
    }

    #[test]
    fn play_outs_on_one_tick_come_in_slot_order_not_start_order() {
        let mut v = Voices::new();
        // Started 9, 3, 6 on ticks 0, 1, 2, each ending on tick 32.
        v.start(0, h(2, 9), Length::Ms(1067));
        v.start(1, h(3, 3), Length::Ms(1033));
        v.start(2, h(1, 6), Length::Ms(1000));
        for t in 0..32 {
            v.step(t);
        }
        assert_eq!(v.step(32).completed, vec![h(3, 3), h(1, 6), h(2, 9)]);
    }

    #[test]
    fn a_loop_never_plays_out_and_a_stop_ends_it_as_an_interruption() {
        let mut v = Voices::new();
        v.start(0, h(1, 4), Length::Loop);
        for t in 0..10_000 {
            assert!(v.step(t).completed.is_empty());
        }
        assert_eq!(v.running(), 1);
        assert!(v.stop(h(1, 4)));
        assert_eq!(v.guest_finish(h(1, 4), false), Finish::Interrupted);
        assert_eq!(v.running(), 0);
        // A loop the guest says played out is a play-out the update did not
        // predict: the adapter's report differs.
        v.start(10_000, h(1, 5), Length::Loop);
        assert_eq!(v.guest_finish(h(1, 5), true), Finish::PlayedOut);
        assert!(v.step(10_000).completed.is_empty());
    }

    #[test]
    fn an_interrupted_voice_is_not_predicted() {
        let mut v = Voices::new();
        v.start(0, h(1, 1), Length::Ms(1000));
        v.start(0, h(1, 2), Length::Ms(1000));
        v.start(0, h(1, 3), Length::Ms(1000));
        // Stopped, then its fade finishes; cut short by the owner (reason 3).
        assert!(v.stop(h(1, 1)));
        assert!(!v.stop(h(1, 1)), "once");
        for t in 0..20 {
            v.step(t);
        }
        assert_eq!(v.guest_finish(h(1, 1), false), Finish::Interrupted);
        assert_eq!(v.guest_finish(h(1, 2), false), Finish::Interrupted);
        assert_eq!(v.step(30).completed, vec![h(1, 3)]);
        // A stopped voice the guest still finishes with the play-out reason
        // is an interruption too.
        v.start(40, h(2, 1), Length::Ms(1000));
        v.stop(h(2, 1));
        assert_eq!(v.guest_finish(h(2, 1), true), Finish::Interrupted);
        assert!(v.step(70).completed.is_empty());
    }

    #[test]
    fn guest_play_outs_are_known_while_running_or_lately_retired() {
        let mut v = Voices::new();
        v.start(0, h(1, 1), Length::Ms(100));
        v.start(0, h(1, 2), Length::Unknown);
        assert_eq!(v.guest_finish(h(1, 1), true), Finish::PlayedOut, "early: still a tracked voice");
        assert_eq!(v.guest_finish(h(1, 2), true), Finish::Untracked);
        assert_eq!(v.guest_finish(h(9, 9), true), Finish::Untracked);
        for t in 0..=3 {
            v.step(t);
        }
        assert_eq!(v.guest_finish(h(1, 1), true), Finish::PlayedOut, "late: retired");
        v.step(3 + RETIRE_TICKS + 1);
        assert_eq!(v.guest_finish(h(1, 1), true), Finish::Untracked, "forgotten");
    }

    #[test]
    fn due_marks_play_outs_answered_once_and_step_still_reports_them() {
        let mut v = Voices::new();
        v.start(0, h(1, 7), Length::Ms(100));
        v.start(0, h(1, 2), Length::Ms(100));
        v.start(0, h(1, 3), Length::Loop);
        assert!(v.due(2).is_empty());
        assert_eq!(v.due(3), vec![h(1, 2), h(1, 7)]);
        assert!(v.due(3).is_empty(), "answered once");
        assert_eq!(v.step(3).completed, vec![h(1, 2), h(1, 7)]);
    }

    #[test]
    fn the_voices_survive_a_save() {
        let mut v = Voices::new();
        v.start(0, h(1, 1), Length::Ms(500));
        v.start(0, h(1, 2), Length::Loop);
        v.start(1, h(1, 3), Length::Unknown);
        v.stop(h(1, 2));
        v.step(0);
        v.start(2, h(1, 4), Length::Ms(10));
        let mut w = Voices::restore(&v.save()).unwrap();
        assert_eq!(v, w);
        assert_eq!(v.step(15), w.step(15));
        assert!(Voices::restore(&[1, 2, 3]).is_err());
        assert!(Voices::restore(&[0; 16]).is_err(), "trailing");
    }
}
