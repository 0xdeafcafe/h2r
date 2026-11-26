//! Host transport state shared by the scheduler and the agent. Only the
//! film player advances the recorded simulation.
use super::{Mode, Presentation, SeekProgress, State};
use agent_proto::{RpcError, code, theater as api};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// How a film player process opens its film: the tick, the speed and the
/// view (`h2 theater play` hands it to the process it starts).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Launch {
    pub target: u64,
    pub speed: f64,
    pub paused: bool,
    pub presentation: Presentation,
}

impl State {
    pub(super) fn holding(&self) -> bool {
        self.paused && self.seeking.is_none() && self.step_until.is_none()
    }

    pub(super) fn jump_in_progress(&self) -> bool {
        self.seeking.is_some() || self.restart.is_some()
    }

    pub(super) fn length(&self) -> u64 {
        self.header.meta.length_ticks
    }

    pub(super) fn set_paused(&mut self, paused: bool) {
        if self.paused != paused || self.step_until.is_some() {
            self.rate = PlaybackRate::default();
        }
        self.step_until = None;
        self.paused = paused;
    }

    pub(super) fn set_speed(&mut self, speed: f64) {
        if self.speed != speed {
            self.rate = PlaybackRate::default();
            if let Some(clock) = &mut self.pacing {
                clock.debt = 0.;
            }
            self.speed = speed;
        }
    }

    pub(super) fn step(&mut self, request: api::Step) -> Result<(), RpcError> {
        if request.ticks == 0 {
            return Err(RpcError::new(
                code::INVALID_PARAMS,
                "step needs a positive tick count",
            ));
        }
        if !self.holding() || self.jump_in_progress() {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "step requires paused playback",
            ));
        }
        let target = self
            .tick
            .checked_add(request.ticks)
            .filter(|&tick| tick <= self.length())
            .ok_or_else(|| RpcError::new(code::INVALID_PARAMS, "step beyond film"))?;
        self.step_until = Some(target);
        self.rate = PlaybackRate::default();
        Ok(())
    }

    pub(super) fn seek(&mut self, request: api::Seek) -> Result<(), RpcError> {
        let invalid = |why| RpcError::new(code::INVALID_PARAMS, why);
        let continuing = self.jump_in_progress();
        let from = self
            .seek
            .as_ref()
            .filter(|_| continuing)
            .map_or(self.tick, |s| s.from_tick);
        if request.cancel == Some(true) {
            if request.source.is_some() || request.clip_marks.is_some() {
                return Err(invalid("cancel only accepts an optional tick"));
            }
            if continuing && self.seek.as_ref().is_some_and(|seek| seek.target != from) {
                if self.seeking.is_none() && self.tick == from {
                    // The jump has been queued but the film still holds the
                    // departure tick.
                    self.restart = None;
                    self.seek = None;
                } else if self.ring_pending() && self.tick == from {
                    // A ring jump whose entry has not loaded: the film
                    // still holds the departure tick.
                    if let Some(ring) = &mut self.ring {
                        ring.cancel();
                    }
                    self.seeking = None;
                    self.seek = None;
                } else {
                    self.seek_in_game(from, from, None);
                }
                self.set_paused(true);
            }
            return Ok(());
        }

        let target = request.tick.ok_or_else(|| invalid("seek needs a tick"))?;
        if target > self.length() {
            return Err(invalid("seek beyond film"));
        }
        if let Some([start, end]) = request.clip_marks {
            if start.is_some_and(|tick| tick > self.length())
                || end.is_some_and(|tick| {
                    tick > self.length().saturating_add(1)
                        || start.is_none_or(|start| tick <= start)
                })
            {
                return Err(invalid("invalid clip marks"));
            }
            self.presentation.clip_marks = [start, end];
        }
        let paused = self.paused;
        self.set_paused(paused);
        // Forward from the tick shown, or the start route, which the
        // session turns into a queued game (`Session::requeue`).
        self.seek_in_game(target, from, request.source);
        Ok(())
    }

    /// A start-route jump: the film's game is queued again
    /// ([`super::Session::requeue`]) and runs forward to `target`.
    pub(super) fn queue_jump(
        &mut self,
        target: u64,
        from_tick: u64,
        requested_source: Option<api::SeekRoute>,
    ) {
        self.seek = Some(SeekProgress {
            target,
            from_tick,
            source: api::SeekSource::Boot,
            requested_source,
        });
        if let Some(ring) = &mut self.ring {
            ring.cancel();
        }
        self.restart = Some(target);
        self.step_until = None;
        self.rate = PlaybackRate::default();
    }

    pub(super) fn seek_info(&self) -> Option<api::SeekInfo> {
        self.seek.as_ref().map(|seek| api::SeekInfo {
            target: seek.target,
            from_tick: seek.from_tick,
            source: seek.source,
            requested: seek.requested_source,
            remaining_ticks: if self.restart.is_some() {
                seek.target
            } else {
                seek.target.saturating_sub(self.tick)
            },
            restore_ms: self.ring_restore_ms(seek.source),
        })
    }

    pub(super) fn completed_transport_tick(&mut self, reached_target: bool) {
        if self.step_until == Some(self.tick) {
            self.step_until = None;
        }
        if !matches!(self.mode, Mode::Replay(..))
            || reached_target
            || self.jump_in_progress()
            || self.paused
        {
            return;
        }
        self.rate.sample(self.tick, Instant::now(), self.tick_hz());
    }

    pub(super) fn tick_hz(&self) -> f64 {
        let hz = self
            .play
            .as_ref()
            .and_then(|play| play.start().game.descriptor.ticks_per_second)
            .unwrap_or(30)
            .max(1);
        f64::from(hz)
    }

    /// What the player asks of the next frame: the answer to the engine's
    /// question in `GAME_UPDATE_TICKS` ([`FrameClock::frame_ticks`]).
    pub(super) fn demand(&self) -> Demand {
        // A film played from its game start loads its map at the engine's
        // own pace: a pause, a speed or a target waits for its first update.
        if self.play.as_ref().is_some_and(|play| !play.begun()) {
            return Demand {
                paused: false,
                speed: 1.0,
                tick: self.tick,
                run_to: None,
                jumping: self.restart.is_some(),
                tick_hz: self.tick_hz(),
            };
        }
        Demand {
            paused: self.paused,
            speed: self.speed,
            tick: self.tick,
            run_to: self.seeking.or(self.step_until),
            jumping: self.restart.is_some(),
            tick_hz: self.tick_hz(),
        }
    }

    pub(super) fn speed_reached(&self) -> Option<f64> {
        if self.jump_in_progress() || self.step_until.is_some() {
            None
        } else if self.paused {
            Some(0.0)
        } else {
            self.rate.reached.map(|speed| speed.min(self.speed))
        }
    }
}

/// The most ticks one frame runs while the player runs forward to a step's
/// or a jump's target, so the frame still draws and the transport answers.
pub(super) const RUN_TICKS_PER_FRAME: u32 = 30;
/// A frame longer than this counts as this long, so a stall is not repaid
/// with a burst of ticks.
const LONGEST_FRAME: f64 = 0.25;

/// The transport's state as it bears on one frame's tick count.
#[derive(Clone, Copy, Debug)]
pub(super) struct Demand {
    pub(super) paused: bool,
    pub(super) speed: f64,
    /// The tick shown.
    pub(super) tick: u64,
    /// The tick a step or a forward run to a jump's target stops at.
    pub(super) run_to: Option<u64>,
    /// A start-route jump is queued and waits for the film's game.
    pub(super) jumping: bool,
    pub(super) tick_hz: f64,
}

/// Answers the engine's per-frame tick count (`GAME_UPDATE_TICKS`). It
/// holds the time of the last frame and the fraction of a tick not yet
/// owed, and writes nothing into game state.
#[derive(Default)]
pub(super) struct FrameClock {
    last: Option<Instant>,
    pub(super) debt: f64,
}

impl FrameClock {
    /// `None` leaves the engine's own count (`engine_ticks`) alone: normal
    /// speed. `Some(n)` replaces it. A pause answers zero, a step or a
    /// forward run answers the ticks left to its target (at most
    /// [`RUN_TICKS_PER_FRAME`]), and another speed answers the ticks the
    /// frame's time is worth at that speed, so 4x at the tick rate's frame
    /// time is four.
    pub(super) fn frame_ticks(
        &mut self,
        demand: Demand,
        engine_ticks: u32,
        now: Instant,
    ) -> Option<u32> {
        let seconds = match self.last.replace(now) {
            Some(last) => now.duration_since(last).as_secs_f64().min(LONGEST_FRAME),
            None => f64::from(engine_ticks) / demand.tick_hz,
        };
        if demand.jumping {
            return Some(0);
        }
        if let Some(target) = demand.run_to {
            self.debt = 0.;
            let left = target.saturating_sub(demand.tick);
            return Some(left.min(u64::from(RUN_TICKS_PER_FRAME)) as u32);
        }
        if demand.paused {
            self.debt = 0.;
            return Some(0);
        }
        if demand.speed == 1.0 {
            self.debt = 0.;
            return None;
        }
        self.debt += demand.speed * seconds * demand.tick_hz;
        // The small bias keeps a frame of exactly n ticks from rounding to n - 1.
        let whole = (self.debt + 1e-6).floor().max(0.);
        self.debt -= whole;
        Some((whole as u32).min(RUN_TICKS_PER_FRAME))
    }
}

#[derive(Default)]
pub(super) struct PlaybackRate {
    window: Option<(u64, Instant)>,
    reached: Option<f64>,
}

impl PlaybackRate {
    fn sample(&mut self, tick: u64, now: Instant, tick_hz: f64) {
        const WINDOW: Duration = Duration::from_millis(500);
        let (first_tick, first_at) = *self.window.get_or_insert((tick, now));
        let elapsed = now.duration_since(first_at);
        if elapsed >= WINDOW {
            self.reached =
                Some(tick.saturating_sub(first_tick) as f64 / elapsed.as_secs_f64() / tick_hz);
            self.window = Some((tick, now));
        }
    }
}

/// A burst of transport taps becomes one absolute target after the last tap.
#[derive(Default)]
pub(crate) struct SeekTaps(Option<(u64, Instant)>);

impl SeekTaps {
    const WINDOW: Duration = Duration::from_millis(250);

    pub(crate) fn push(&mut self, origin: u64, delta: i64, bounds: (u64, u64), now: Instant) {
        let target = self.0.map_or(origin, |(target, _)| target);
        self.0 = Some((
            target
                .saturating_add_signed(delta)
                .clamp(bounds.0, bounds.1),
            now + Self::WINDOW,
        ));
    }

    pub(crate) fn take_due(&mut self, now: Instant) -> Option<u64> {
        self.0.filter(|&(_, due)| now >= due)?;
        self.take()
    }

    pub(crate) fn take(&mut self) -> Option<u64> {
        self.0.take().map(|(target, _)| target)
    }

    pub(crate) fn is_pending(&self) -> bool {
        self.0.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn demand() -> Demand {
        Demand {
            paused: false,
            speed: 1.0,
            tick: 100,
            run_to: None,
            jumping: false,
            tick_hz: 30.0,
        }
    }

    fn frames(demand: Demand, engine: u32, count: usize, frame: Duration) -> Vec<Option<u32>> {
        let mut clock = FrameClock::default();
        let start = Instant::now();
        (0..count)
            .map(|i| clock.frame_ticks(demand, engine, start + frame * i as u32))
            .collect()
    }

    #[test]
    fn a_pause_answers_zero_every_frame_and_normal_speed_leaves_the_engines_count() {
        let tick = Duration::from_secs_f64(1.0 / 30.0);
        let paused = Demand { paused: true, ..demand() };
        assert_eq!(frames(paused, 1, 5, tick), vec![Some(0); 5]);
        assert_eq!(frames(demand(), 1, 5, tick), vec![None; 5]);
    }

    #[test]
    fn a_step_runs_the_ticks_asked_and_no_more() {
        let tick = Duration::from_secs_f64(1.0 / 30.0);
        let one = Demand { paused: true, run_to: Some(101), ..demand() };
        assert_eq!(frames(one, 1, 1, tick), vec![Some(1)]);
        let done = Demand { paused: true, tick: 101, run_to: None, ..demand() };
        assert_eq!(frames(done, 1, 1, tick), vec![Some(0)]);
    }

    #[test]
    fn four_times_speed_asks_for_four_ticks_a_frame_and_a_quarter_for_one_in_four() {
        let tick = Duration::from_secs_f64(1.0 / 30.0);
        let fast = Demand { speed: 4.0, ..demand() };
        let got = frames(fast, 1, 4, tick);
        // The first frame has no earlier frame; it is worth the engine's count.
        assert_eq!(got[1..], [Some(4), Some(4), Some(4)]);
        let slow = Demand { speed: 0.25, ..demand() };
        let total: u32 = frames(slow, 1, 17, tick)[1..].iter().map(|n| n.unwrap()).sum();
        assert_eq!(total, 4);
    }

    #[test]
    fn a_forward_run_stops_exactly_at_its_target_within_the_frame_cap() {
        let tick = Duration::from_secs_f64(1.0 / 30.0);
        let mut at = 100;
        let mut clock = FrameClock::default();
        let start = Instant::now();
        let mut frames = 0;
        while at < 170 {
            let run = Demand { paused: true, tick: at, run_to: Some(170), ..demand() };
            let n = clock.frame_ticks(run, 0, start + tick * frames).unwrap();
            assert!(n <= RUN_TICKS_PER_FRAME);
            at += u64::from(n);
            frames += 1;
        }
        assert_eq!((at, frames), (170, 3));
    }

    #[test]
    fn a_queued_jump_runs_no_ticks_and_a_stall_is_not_repaid() {
        let tick = Duration::from_secs_f64(1.0 / 30.0);
        let jump = Demand { jumping: true, run_to: Some(500), ..demand() };
        assert_eq!(frames(jump, 1, 2, tick), vec![Some(0); 2]);
        let mut clock = FrameClock::default();
        let start = Instant::now();
        let fast = Demand { speed: 2.0, ..demand() };
        clock.frame_ticks(fast, 1, start);
        let n = clock.frame_ticks(fast, 1, start + Duration::from_secs(10)).unwrap();
        assert_eq!(n, 15); // 0.25 s at 2x and 30 Hz
    }

    #[test]
    fn rate_reports_tick_throughput_relative_to_the_films_tick_rate() {
        let mut rate = PlaybackRate::default();
        let start = Instant::now();
        rate.sample(10, start, 60.0);
        rate.sample(20, start + Duration::from_millis(300), 60.0);
        assert_eq!(rate.reached, None);
        rate.sample(40, start + Duration::from_secs(1), 60.0);
        assert_eq!(rate.reached, Some(0.5));
        rate.sample(100, start + Duration::from_millis(1500), 60.0);
        assert_eq!(rate.reached, Some(2.0));
    }
}
