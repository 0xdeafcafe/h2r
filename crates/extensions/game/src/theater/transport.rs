//! Host transport state shared by the scheduler, the agent and the restart
//! handoff. Only the replay child advances the recorded simulation.
use super::{Mode, Presentation, Replay, SeekProgress, State};
use agent_proto::{RpcError, code, theater as api};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Serialize, Deserialize)]
pub struct Restart {
    pub target: u64,
    pub from_tick: Option<u64>,
    pub source: Option<api::SeekRoute>,
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
        match &self.mode {
            Mode::Replay(tape, _) => tape.length,
            Mode::Record(_) => self.header.meta.length_ticks,
        }
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
                    // The jump has been queued but the old child still holds
                    // the departure tick.
                    self.restart = None;
                    self.seek = None;
                } else {
                    self.queue_jump(from, from, None, self.seek_source(from));
                }
                self.set_paused(true);
            }
            return Ok(());
        }

        let target = request.tick.ok_or_else(|| invalid("seek needs a tick"))?;
        if target > self.length() {
            return Err(invalid("seek beyond film"));
        }
        let source = match request.source {
            Some(api::SeekRoute::Boot) => api::SeekSource::Boot,
            _ => self.seek_source(target),
        };
        if request.source == Some(api::SeekRoute::Keyframe) && source != api::SeekSource::Keyframe {
            return Err(RpcError::new(
                code::UNAVAILABLE,
                "no compatible keyframe at or before target",
            ));
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
        self.queue_jump(target, from, request.source, source);
        Ok(())
    }

    fn seek_source(&self, tick: u64) -> api::SeekSource {
        self.engine
            .as_ref()
            .map_or(api::SeekSource::Boot, |engine| engine.seek_source(tick))
    }

    fn queue_jump(
        &mut self,
        target: u64,
        from_tick: u64,
        requested_source: Option<api::SeekRoute>,
        source: api::SeekSource,
    ) {
        self.seek = Some(SeekProgress {
            target,
            from_tick,
            source,
            requested_source,
            warm: false,
            restore_ms: None,
        });
        self.restart = Some(target);
        self.step_until = None;
        self.rate = PlaybackRate::default();
    }

    pub(super) fn seek_info(&self) -> Option<api::SeekInfo> {
        self.seek.as_ref().map(|seek| api::SeekInfo {
            target: seek.target,
            from_tick: seek.from_tick,
            source: seek.source,
            warm: seek.warm,
            remaining_ticks: if self.restart.is_some() {
                seek.target
            } else {
                seek.target.saturating_sub(self.tick)
            },
            restore_ms: seek.restore_ms,
        })
    }

    pub(super) fn write_restart(&self, replay: &Replay) -> anyhow::Result<bool> {
        let Some(target) = self.restart else {
            return Ok(false);
        };
        let seek = self.seek.as_ref().expect("restart has a seek request");
        let mut presentation = self.presentation.clone();
        if self.header.version >= 2 {
            replay.camera.save(&mut presentation);
        }
        let request = Restart {
            target,
            from_tick: Some(seek.from_tick),
            source: seek.requested_source,
            speed: self.speed,
            paused: self.paused,
            presentation,
        };
        std::fs::write(&replay.restart, serde_json::to_vec(&request)?)?;
        Ok(true)
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
        let tick_hz = self
            .engine
            .as_ref()
            .and_then(|engine| engine.track.descriptor.ticks_per_second)
            .unwrap_or(30)
            .max(1);
        self.rate
            .sample(self.tick, Instant::now(), f64::from(tick_hz));
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
