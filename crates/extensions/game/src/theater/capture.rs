//! Samples a verified replay on the Theater clock. The encoder owns its
//! output and can apply backpressure to this player.
use anyhow::{Context, Result, ensure};
use std::sync::{Mutex, MutexGuard};

const TICKS_PER_SECOND: u32 = 30;
const STEREO_SAMPLES_PER_TICK: usize = 2 * 48_000 / TICKS_PER_SECOND as usize;

#[derive(Clone, Copy)]
pub struct ClipSpan {
    from: u64,
    to: u64,
    frames_per_tick: usize,
}

impl ClipSpan {
    pub fn new(from: u64, to: u64, fps: u32, length: u64) -> Result<Self> {
        ensure!(
            from < to && to <= length.saturating_add(1),
            "invalid clip range"
        );
        ensure!(matches!(fps, 30 | 60), "clip fps must be 30 or 60");
        let frames_per_tick = (fps / TICKS_PER_SECOND) as usize;
        ensure!(
            (to - from)
                .checked_mul(STEREO_SAMPLES_PER_TICK as u64)
                .is_some(),
            "clip range is too long"
        );
        Ok(Self {
            from,
            to,
            frames_per_tick,
        })
    }

    pub fn frames(self) -> u64 {
        (self.to - self.from) * self.frames_per_tick as u64
    }
}

pub struct Picture {
    pub size: (u32, u32),
    pub rgba: Vec<u8>,
}

pub trait ClipSink: Send {
    fn frame(&mut self, picture: &Picture) -> Result<()>;
    fn audio(&mut self, pcm: &[f32]) -> Result<()>;
    fn finish(&mut self) -> Result<()>;
}

pub struct ClipCapture {
    span: ClipSpan,
    state: Mutex<CaptureState>,
}

struct CaptureState {
    tick: Option<u64>,
    pending: Vec<Picture>,
    last: Picture,
    samples: Option<u64>,
    sink: Box<dyn ClipSink>,
    error: Option<String>,
}

impl ClipCapture {
    pub fn new(span: ClipSpan, blank: Picture, audio: bool, sink: Box<dyn ClipSink>) -> Self {
        Self {
            span,
            state: Mutex::new(CaptureState {
                tick: None,
                pending: Vec::with_capacity(span.frames_per_tick),
                last: blank,
                samples: audio.then(|| (span.to - span.from) * STEREO_SAMPLES_PER_TICK as u64),
                sink,
                error: None,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, CaptureState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    // A single candidate during preroll seeds the first tick. During the
    // range, keep the first and latest pictures for 60 fps, or only the
    // latest for 30 fps. Skipped draws hold the preceding picture.
    pub fn present(&self, read: impl FnOnce() -> Result<Picture>) -> Result<()> {
        let mut s = self.lock();
        if s.error.is_some()
            || s.tick
                .is_none_or(|t| t >= self.span.to || t.saturating_add(1) < self.span.from)
        {
            return Ok(());
        }
        let picture = match read() {
            Ok(picture) => picture,
            Err(error) => {
                s.error = Some(format!("{error:#}"));
                return Err(error);
            }
        };
        let count = if s.tick.is_none_or(|t| t < self.span.from) {
            1
        } else {
            self.span.frames_per_tick
        };
        if s.pending.len() == count {
            s.pending[count - 1] = picture;
        } else {
            s.pending.push(picture);
        }
        Ok(())
    }

    pub fn audio(&self, pcm: &[f32]) {
        let mut s = self.lock();
        if s.error.is_some()
            || !s
                .tick
                .is_some_and(|tick| (self.span.from..self.span.to).contains(&tick))
        {
            return;
        }
        if let Some(remaining) = s.samples.filter(|&remaining| remaining != 0) {
            let count = (pcm.len() as u64).min(remaining) as usize;
            if let Err(error) = s.sink.audio(&pcm[..count]) {
                s.error = Some(format!("{error:#}"));
            }
            s.samples = Some(remaining - count as u64);
        }
    }

    /// Advances before drawing the next tick. If the film ends on an
    /// inclusive last tick, the runner advances once more after its final draw.
    pub(super) fn advance(&self, tick: u64) -> Result<bool> {
        let mut s = self.lock();
        ensure!(
            s.error.is_none(),
            "clip capture: {}",
            s.error.as_deref().unwrap_or("")
        );
        if let Some(previous) = s.tick {
            if tick <= previous {
                return Ok(previous >= self.span.to);
            }
            let CaptureState {
                pending,
                last,
                sink,
                ..
            } = &mut *s;
            let mut frames = pending.drain(..);
            if previous < self.span.from {
                if let Some(picture) = frames.next_back() {
                    *last = picture;
                }
            }
            for current in previous.max(self.span.from)..tick.min(self.span.to) {
                for _ in 0..self.span.frames_per_tick {
                    if let Some(picture) = frames.next() {
                        *last = picture;
                    }
                    if let Err(error) = sink
                        .frame(last)
                        .with_context(|| format!("exporting tick {current}"))
                    {
                        drop(frames);
                        s.error = Some(format!("{error:#}"));
                        return Err(error);
                    }
                }
            }
        } else {
            ensure!(
                tick <= self.span.from,
                "replay started past the clip in mark"
            );
        }
        s.tick = Some(tick);
        Ok(tick >= self.span.to)
    }

    pub(super) fn done(&self) -> bool {
        self.lock().tick.is_some_and(|tick| tick >= self.span.to)
    }

    pub fn finish(&self) -> Result<()> {
        let mut s = self.lock();
        ensure!(
            s.tick.is_some_and(|tick| tick >= self.span.to),
            "replay stopped before the clip out mark"
        );
        ensure!(
            s.error.is_none(),
            "clip capture: {}",
            s.error.as_deref().unwrap_or("")
        );
        // The mixer renders whole quanta and host films can have completion
        // timing alone. Fill the remainder to the exact video duration.
        if let Some(mut remaining) = s.samples.take() {
            let silence = [0.0; STEREO_SAMPLES_PER_TICK];
            while remaining > 0 {
                let count = remaining.min(silence.len() as u64) as usize;
                s.sink.audio(&silence[..count])?;
                remaining -= count as u64;
            }
        }
        s.sink.finish()
    }
}
