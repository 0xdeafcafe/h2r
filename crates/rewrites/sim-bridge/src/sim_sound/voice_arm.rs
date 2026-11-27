//! The sound sim's per-tick voice update as a shadow arm of
//! `impl.sim.native.sound` (docs/engine/sim-rewrite.md, "Sound sim
//! (phase 4)"). [`sim_sound::voices::Voices`] owns the voices; the hooks
//! here feed it the guest's admissions, stops and finishes and compare, per
//! tick, the update's starts and play-outs with the guest's.
//!
//! The turn of a tick ([`Arm::turn`]):
//!
//! | Tick | Proven | Turn |
//! |---|---|---|
//! | `tick % SAMPLE_EVERY == 0` | any | compare the whole report |
//! | other | no | skip: the update steps, nothing is compared |
//! | other | yes | answer: the update finishes its play-outs first, and the guest's report must hold the rest |
//!
//! Proven is [`PROVEN_AFTER`] matched ticks with voices in them and no
//! difference ever. One difference ends the answers for the process.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use engine::entry::{Cx, EntryResult, GuestCall, Ret};
use engine::{GuestMemory, points};
use hooks::{Builder, Flow};
use sim_sound::voices::{Finish, Instance, Length, Report, Voices};

use super::owners;

/// One tick in this many is compared.
pub const SAMPLE_EVERY: u32 = 64;
/// Matched ticks with voices, and no difference, before the update answers.
pub const PROVEN_AFTER: u64 = 64;
/// A longer jump between ticks forgets the voices instead of stepping each.
const MAX_GAP: u32 = 1 << 16;
/// Flags word (`+4`) bits 9 and 6: the per-instance walk finishes a voice
/// whose channel stopped with the play-out reason only with both set
/// (`0x1280d4..0x12810f`).
const PLAYS_OUT: u16 = (1 << 9) | (1 << 6);

/// The arm's outcomes, process-wide (`sim.arms` reads them as
/// `sound.voices`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Sampled ticks with voices whose reports matched.
    pub matched: u64,
    /// Ticks whose reports differed (sampled or answered).
    pub mismatched: u64,
    /// Sampled ticks with no voice event on either side.
    pub quiet: u64,
    /// Sampled ticks not compared: a play-out of a voice the update never
    /// saw start, or one of unknown length.
    pub declined: u64,
    /// Ticks neither sampled nor answered.
    pub skipped: u64,
    /// Voices the update finished itself.
    pub answered: u64,
    /// Voices due to the update that the guest had freed already.
    pub stale: u64,
    /// Voices stopped or cut short by the guest.
    pub interrupted: u64,
    pub first: Option<Mismatch>,
}

impl Stats {
    const fn new() -> Self {
        Stats {
            matched: 0,
            mismatched: 0,
            quiet: 0,
            declined: 0,
            skipped: 0,
            answered: 0,
            stale: 0,
            interrupted: 0,
            first: None,
        }
    }
}

/// The first differing tick.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Mismatch {
    pub tick: u32,
    /// What the guest's report should have been.
    pub rust: Report,
    pub guest: Report,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Turn {
    Skip,
    Compare,
    Answer,
}

#[derive(Debug)]
struct Window {
    tick: u32,
    turn: Turn,
    guest: Report,
    declined: bool,
    answered: Vec<Instance>,
}

/// The update, the tick being gathered and the outcomes.
#[derive(Debug)]
pub struct Arm {
    voices: Voices,
    window: Option<Window>,
    stats: Stats,
}

impl Default for Arm {
    fn default() -> Self {
        Arm::new()
    }
}

impl Arm {
    pub const fn new() -> Self {
        Arm { voices: Voices::new(), window: None, stats: Stats::new() }
    }

    pub fn stats(&self) -> Stats {
        self.stats.clone()
    }

    pub fn proven(&self) -> bool {
        self.stats.mismatched == 0 && self.stats.matched >= PROVEN_AFTER
    }

    pub fn turn(&self, tick: u32) -> Turn {
        if tick % SAMPLE_EVERY == 0 {
            Turn::Compare
        } else if self.proven() {
            Turn::Answer
        } else {
            Turn::Skip
        }
    }

    /// Close every tick before `tick` and open `tick`. The tick going back
    /// (a new map, a revert) forgets the voices.
    pub fn advance(&mut self, tick: u32) {
        let Some(open) = self.window.as_ref().map(|w| w.tick) else {
            self.open(tick);
            return;
        };
        if tick == open {
            return;
        }
        if tick < open {
            self.voices.clear();
            self.open(tick);
            return;
        }
        let w = self.window.take().expect("open window");
        self.close(w);
        if tick - open > MAX_GAP {
            self.voices.clear();
        } else {
            for t in open + 1..tick {
                let w = self.window_for(t);
                self.close(w);
            }
        }
        self.open(tick);
    }

    fn window_for(&self, tick: u32) -> Window {
        Window { tick, turn: self.turn(tick), guest: Report::default(), declined: false, answered: Vec::new() }
    }

    fn open(&mut self, tick: u32) {
        self.window = Some(self.window_for(tick));
    }

    fn window(&mut self, tick: u32) -> &mut Window {
        self.advance(tick);
        self.window.as_mut().expect("advance opens the window")
    }

    fn close(&mut self, w: Window) {
        let rust = self.voices.step(w.tick);
        match w.turn {
            Turn::Skip => self.stats.skipped += 1,
            Turn::Compare if w.declined => self.stats.declined += 1,
            Turn::Compare if rust.is_empty() && w.guest.is_empty() => self.stats.quiet += 1,
            Turn::Compare if rust == w.guest => self.stats.matched += 1,
            Turn::Compare => self.differ(w.tick, rust, w.guest),
            Turn::Answer => {
                let expected = Report {
                    started: rust.started,
                    completed: rust.completed.into_iter().filter(|i| !w.answered.contains(i)).collect(),
                };
                if !w.declined && expected != w.guest {
                    self.differ(w.tick, expected, w.guest);
                }
            }
        }
    }

    fn differ(&mut self, tick: u32, rust: Report, guest: Report) {
        self.stats.mismatched += 1;
        self.stats.first.get_or_insert(Mismatch { tick, rust, guest });
    }

    /// The guest admitted `instance` on `tick`; `None` when its length
    /// could not be read.
    pub fn started(&mut self, tick: u32, instance: Instance, length: Option<Length>) {
        let w = self.window(tick);
        w.guest.started.push(instance);
        if length.is_none() {
            w.declined = true;
        }
        self.voices.start(tick, instance, length.unwrap_or(Length::Unknown));
    }

    pub fn stopped(&mut self, tick: u32, instance: Instance) {
        self.advance(tick);
        self.voices.stop(instance);
    }

    /// The guest finished `instance` with `reason`.
    pub fn finished(&mut self, tick: u32, instance: Instance, reason: u32) {
        self.advance(tick);
        let played_out = reason == owners::ENDED_REASON;
        match self.voices.guest_finish(instance, played_out) {
            Finish::PlayedOut => self.window(tick).guest.completed.push(instance),
            Finish::Interrupted => self.stats.interrupted += 1,
            Finish::Untracked => self.window(tick).declined = true,
        }
    }

    /// The play-outs the update finishes itself on `tick`, in slot order:
    /// none unless the tick is an answer.
    pub fn due(&mut self, tick: u32) -> Vec<Instance> {
        if self.window(tick).turn != Turn::Answer {
            return Vec::new();
        }
        let due = self.voices.due(tick);
        self.window(tick).answered.extend(&due);
        due
    }

    /// The update finished one voice (`true`) or found it freed (`false`).
    pub fn answered(&mut self, finished: bool) {
        if finished {
            self.stats.answered += 1;
        } else {
            self.stats.stale += 1;
        }
    }

    pub fn save(&self) -> Vec<u8> {
        self.voices.save()
    }

    /// The voices of a save; the tick being gathered starts over and the
    /// outcomes stay.
    pub fn load(&mut self, bytes: &[u8]) -> Result<(), String> {
        self.voices = Voices::restore(bytes)?;
        self.window = None;
        Ok(())
    }
}

static ARM: Mutex<Arm> = Mutex::new(Arm::new());

fn arm() -> MutexGuard<'static, Arm> {
    ARM.lock().unwrap_or_else(|e| e.into_inner())
}

/// The installed arm's outcomes.
pub fn stats() -> Stats {
    arm().stats()
}

pub fn save() -> Vec<u8> {
    arm().save()
}

pub fn load(bytes: &[u8]) -> Result<(), String> {
    arm().load(bytes)
}

fn tick_of(m: &dyn GuestMemory) -> Option<u32> {
    let time = m.u32(points::GAME_TIME.base).ok().filter(|&t| t != 0)?;
    m.u32(time.wrapping_add(8)).ok()
}

/// What the update knows of an admitted instance's length.
pub fn length(m: &dyn GuestMemory, instance: Instance) -> Option<Length> {
    let rec = owners::record(m, instance)?;
    let flags = m.u16(rec.wrapping_add(4)).ok()?;
    if flags & PLAYS_OUT != PLAYS_OUT {
        return Some(Length::Loop);
    }
    owners::read_admitted(m, instance)?.duration_ms.map(Length::Ms)
}

/// Re-entry guards: the instance the update is finishing (its own finish
/// is not the guest's), and the service the update calls after its
/// finishes.
struct Guards {
    finishing: AtomicU32,
    servicing: AtomicBool,
}

/// The update's own finishes, then the service.
async fn play_out(cx: Cx, guards: Arc<Guards>, due: Vec<Instance>) -> EntryResult {
    for instance in due {
        if !cx.read(|m| owners::record(m, instance).is_some()) {
            arm().answered(false);
            continue;
        }
        guards.finishing.store(instance, Ordering::SeqCst);
        let called = cx
            .call(GuestCall::new(owners::FINISH).eax(instance).args([owners::ENDED_REASON]))
            .await;
        guards.finishing.store(owners::NONE, Ordering::SeqCst);
        called?;
        arm().answered(true);
    }
    guards.servicing.store(true, Ordering::SeqCst);
    let called = cx.call(GuestCall::new(points::SOUND_UPDATE.base)).await;
    guards.servicing.store(false, Ordering::SeqCst);
    called?;
    Ok(Ret::new(0, 0))
}

/// The arm's hooks:
///
/// - `SOUND_IMPULSE_ALLOCATE`, after: a returned instance starts a voice.
/// - `SOUND_STOP_INSTANCE`, before: the voice is interrupted.
/// - `SOUND_FINISH_INSTANCE`, before: the guest's finish, with its reason.
/// - `GAME_TICK`, after: closes the tick that ran.
/// - `SOUND_UPDATE`, before: on an answer tick, the update's play-outs.
pub fn install(b: &mut Builder) {
    b.raw().observe_after(&points::SOUND_IMPULSE_ALLOCATE, "voice start", |o| {
        let instance = o.cpu.eax;
        if instance == owners::NONE {
            return;
        }
        let read = {
            let g = o.guest();
            let m: &dyn GuestMemory = &*g;
            tick_of(m).map(|tick| (tick, length(m, instance)))
        };
        if let Some((tick, length)) = read {
            arm().started(tick, instance, length);
        }
    });
    b.raw().observe_before(&points::SOUND_STOP_INSTANCE, "voice stop", |o| {
        let instance = o.cpu.ebx;
        let tick = tick_of(&*o.guest());
        if let Some(tick) = tick {
            arm().stopped(tick, instance);
        }
    });
    let guards = Arc::new(Guards { finishing: AtomicU32::new(owners::NONE), servicing: AtomicBool::new(false) });
    let g = Arc::clone(&guards);
    b.raw().observe_before(&points::SOUND_FINISH_INSTANCE, "voice finish", move |o| {
        let instance = o.cpu.eax;
        if instance == g.finishing.load(Ordering::SeqCst) {
            return;
        }
        let (Some(tick), Some(reason)) = (tick_of(&*o.guest()), o.arg_u32("reason")) else {
            return;
        };
        arm().finished(tick, instance, reason);
    });
    b.raw().observe_after(&points::GAME_TICK, "voice tick", |o| {
        let tick = tick_of(&*o.guest());
        if let Some(tick) = tick {
            arm().advance(tick);
        }
    });
    b.raw().before(&points::SOUND_UPDATE, "voice play-outs", move |call| {
        if guards.servicing.load(Ordering::SeqCst) {
            return Ok(Flow::Continue);
        }
        let rt = Arc::clone(call.ctx.runtime());
        let tick = tick_of(&*rt.memory());
        let Some(tick) = tick else {
            return Ok(Flow::Continue);
        };
        let due = arm().due(tick);
        if due.is_empty() {
            return Ok(Flow::Continue);
        }
        let guards = Arc::clone(&guards);
        call.answer_async(move |cx| Box::pin(play_out(cx, guards, due)))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(salt: u32, slot: u32) -> Instance {
        (salt << 16) | slot
    }

    /// A one-shot started on `tick` that plays out on `tick + 1`
    /// (33 ms is one tick).
    const ONE_TICK: Length = Length::Ms(33);

    /// One clean tick with a voice at the sampled tick `n * SAMPLE_EVERY`:
    /// it starts there and the guest plays it out on the next tick.
    fn clean_sample(a: &mut Arm, n: u32) {
        let t = n * SAMPLE_EVERY;
        a.started(t, h(1, n & 0xff), Some(Length::Ms(0)));
        a.finished(t, h(1, n & 0xff), owners::ENDED_REASON);
        a.advance(t + 1);
    }

    #[test]
    fn one_tick_in_sample_every_is_compared() {
        let a = Arm::new();
        assert_eq!(a.turn(0), Turn::Compare);
        assert_eq!(a.turn(SAMPLE_EVERY), Turn::Compare);
        for t in 1..SAMPLE_EVERY {
            assert_eq!(a.turn(t), Turn::Skip, "tick {t}");
        }
    }

    #[test]
    fn a_voice_start_and_its_play_out_match_on_sampled_ticks() {
        let mut a = Arm::new();
        a.started(0, h(1, 2), Some(Length::Ms(0)));
        a.started(0, h(1, 1), Some(Length::Ms(0)));
        // The guest's walk finishes them in slot order.
        a.finished(0, h(1, 1), owners::ENDED_REASON);
        a.finished(0, h(1, 2), owners::ENDED_REASON);
        a.advance(1);
        assert_eq!((a.stats().matched, a.stats().mismatched), (1, 0));
        // Ticks 1..63 are skipped, 64 is quiet.
        a.advance(SAMPLE_EVERY + 1);
        let s = a.stats();
        assert_eq!((s.skipped, s.quiet), (u64::from(SAMPLE_EVERY) - 1, 1));
    }

    #[test]
    fn completion_order_is_compared() {
        let mut a = Arm::new();
        a.started(0, h(1, 2), Some(Length::Ms(0)));
        a.started(0, h(1, 1), Some(Length::Ms(0)));
        // Start order, not slot order: a difference.
        a.finished(0, h(1, 2), owners::ENDED_REASON);
        a.finished(0, h(1, 1), owners::ENDED_REASON);
        a.advance(1);
        let s = a.stats();
        assert_eq!(s.mismatched, 1);
        let m = s.first.unwrap();
        assert_eq!(m.tick, 0);
        assert_eq!(m.rust.completed, vec![h(1, 1), h(1, 2)]);
        assert_eq!(m.guest.completed, vec![h(1, 2), h(1, 1)]);
    }

    #[test]
    fn a_play_out_on_another_tick_is_a_difference() {
        let mut a = Arm::new();
        // Predicted on tick 1; the guest plays it out on sampled tick 64.
        a.started(0, h(1, 1), Some(ONE_TICK));
        a.advance(SAMPLE_EVERY);
        a.finished(SAMPLE_EVERY, h(1, 1), owners::ENDED_REASON);
        a.advance(SAMPLE_EVERY + 1);
        let s = a.stats();
        assert_eq!((s.matched, s.mismatched), (1, 1), "tick 0 holds only the start");
        assert_eq!(s.first.unwrap().guest.completed, vec![h(1, 1)]);
    }

    #[test]
    fn a_loop_is_never_predicted_and_its_stop_is_an_interruption() {
        let mut a = Arm::new();
        a.started(0, h(1, 1), Some(Length::Loop));
        a.advance(1);
        a.stopped(SAMPLE_EVERY, h(1, 1));
        a.finished(SAMPLE_EVERY, h(1, 1), owners::FADE_REASON);
        a.advance(SAMPLE_EVERY + 1);
        let s = a.stats();
        assert_eq!((s.matched, s.quiet, s.mismatched, s.interrupted), (1, 1, 0, 1));
    }

    #[test]
    fn an_interrupted_voice_is_not_a_difference() {
        let mut a = Arm::new();
        a.started(0, h(1, 1), Some(Length::Ms(0)));
        a.started(0, h(1, 2), Some(Length::Ms(0)));
        // The owner's callback cut one short (reason 3) before the walk.
        a.finished(0, h(1, 1), 3);
        a.finished(0, h(1, 2), owners::ENDED_REASON);
        a.advance(1);
        let s = a.stats();
        assert_eq!((s.matched, s.mismatched, s.interrupted), (1, 0, 1));
    }

    #[test]
    fn an_unseen_voice_declines_its_tick() {
        let mut a = Arm::new();
        a.finished(0, h(5, 5), owners::ENDED_REASON);
        a.advance(1);
        a.started(SAMPLE_EVERY, h(1, 1), None);
        a.advance(SAMPLE_EVERY + 1);
        let s = a.stats();
        assert_eq!((s.declined, s.matched, s.mismatched), (2, 0, 0));
    }

    #[test]
    fn it_answers_after_proven_and_audits_one_tick_in_sample_every() {
        let mut a = Arm::new();
        for n in 0..PROVEN_AFTER as u32 {
            assert!(!a.proven());
            clean_sample(&mut a, n);
        }
        assert!(a.proven());
        let t = PROVEN_AFTER as u32 * SAMPLE_EVERY;
        assert_eq!(a.turn(t), Turn::Compare, "the audit tick");
        assert_eq!(a.turn(t + 1), Turn::Answer);
        // A voice due on an answer tick: the update finishes it, the guest's
        // own walk then finds nothing.
        a.started(t, h(2, 1), Some(ONE_TICK));
        a.advance(t + 1);
        assert_eq!(a.due(t + 1), vec![h(2, 1)]);
        assert!(a.due(t + 1).is_empty(), "once a tick");
        a.answered(true);
        a.advance(t + 2);
        let s = a.stats();
        assert_eq!((s.answered, s.mismatched), (1, 0));
        // No answers on a sampled tick.
        a.started(t + SAMPLE_EVERY - 1, h(2, 2), Some(ONE_TICK));
        a.advance(t + SAMPLE_EVERY);
        assert!(a.due(t + SAMPLE_EVERY).is_empty());
    }

    #[test]
    fn a_difference_ends_the_answers() {
        let mut a = Arm::new();
        for n in 0..PROVEN_AFTER as u32 {
            clean_sample(&mut a, n);
        }
        let t = PROVEN_AFTER as u32 * SAMPLE_EVERY + 1;
        assert_eq!(a.turn(t), Turn::Answer);
        // On an answer tick the guest plays out a voice the update holds for
        // later: a difference, and the arm falls back for good.
        a.started(t, h(3, 1), Some(Length::Ms(1000)));
        a.finished(t, h(3, 1), owners::ENDED_REASON);
        a.advance(t + 1);
        let s = a.stats();
        assert_eq!(s.mismatched, 1);
        assert!(!a.proven());
        assert_eq!(a.turn(t + 1), Turn::Skip);
        assert!(a.due(t + 1).is_empty());
        // More clean ticks do not bring it back.
        for n in 0..PROVEN_AFTER as u32 {
            clean_sample(&mut a, PROVEN_AFTER as u32 + 2 + n);
        }
        assert!(!a.proven());
    }

    #[test]
    fn the_tick_going_back_forgets_the_voices_and_a_load_restores_them() {
        // Started on 34, a one-second voice plays out on sampled tick 64.
        let mut a = Arm::new();
        a.started(34, h(1, 1), Some(Length::Ms(1000)));
        let saved = a.save();
        a.advance(5);
        a.finished(SAMPLE_EVERY, h(1, 1), owners::ENDED_REASON);
        a.advance(SAMPLE_EVERY + 1);
        assert_eq!(a.stats().declined, 1, "forgotten: unseen");
        let mut b = Arm::new();
        b.load(&saved).unwrap();
        b.advance(34);
        b.finished(SAMPLE_EVERY, h(1, 1), owners::ENDED_REASON);
        b.advance(SAMPLE_EVERY + 1);
        assert_eq!((b.stats().matched, b.stats().mismatched), (1, 0));
        assert!(b.load(&[1]).is_err());
    }

    #[test]
    fn the_length_comes_from_the_record_and_its_flags() {
        use crate::sim_sound::owners::fixture;
        let mut g = fixture::guest();
        g.put_u16(fixture::REC + 4, PLAYS_OUT);
        assert_eq!(length(&g, fixture::HANDLE), Some(Length::Ms(1741)));
        g.put_u16(fixture::REC + 4, 1 << 9);
        assert_eq!(length(&g, fixture::HANDLE), Some(Length::Loop));
        assert_eq!(length(&g, 0x4321_0002), None, "another salt");
    }
}
