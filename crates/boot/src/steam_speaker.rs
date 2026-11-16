//! The Steam Controller as the controller speaker (docs/input/steam-controller.md):
//! an [`AudioSink`] that DirectSound's side output plays the routed sounds
//! into, the same set the DualSense speaker gets. Nothing is played as
//! audio; each [`input::haptics::FRAME`] of what is written becomes a
//! pitch and a loudness ([`ToneTracker`]), handed on to the trackpad
//! actuators, which buzz it as pulse trains.
//!
//! The sink takes everything at once and never queues, so the mixer never
//! waits on it; its rate is fixed, and DirectSound resamples to it.

use audio::{AudioSink, SinkStats};
use input::haptics::{Tone, ToneTracker};

/// The rate the side mix is rendered at for the haptics.
pub const RATE: u32 = 48_000;

/// Room the sink reports: a second, far more than one pump writes.
const ROOM: usize = RATE as usize;

/// Routed sound in, tones out.
pub struct HapticSink {
    tracker: ToneTracker,
    tone: Box<dyn FnMut(Option<Tone>) + Send>,
    live: Box<dyn Fn() -> bool + Send>,
    written: u64,
}

impl HapticSink {
    /// `tone` gets one tone (or silence) per frame; `live` says whether a
    /// controller is there to play it.
    pub fn new(
        tone: impl FnMut(Option<Tone>) + Send + 'static,
        live: impl Fn() -> bool + Send + 'static,
    ) -> Self {
        Self {
            tracker: ToneTracker::new(RATE),
            tone: Box::new(tone),
            live: Box::new(live),
            written: 0,
        }
    }

    /// Over a running Steam Controller thread.
    pub fn over(pads: input::steam_pad::SteamPads) -> Self {
        let live = pads.clone();
        Self::new(move |t| pads.push_tone(t), move || live.speaker_live())
    }
}

impl AudioSink for HapticSink {
    fn sample_rate(&self) -> u32 {
        RATE
    }

    fn queued_frames(&self) -> usize {
        0
    }

    fn free_frames(&self) -> usize {
        ROOM
    }

    fn write(&mut self, samples: &[f32]) -> usize {
        for t in self.tracker.push(samples) {
            (self.tone)(t);
        }
        let frames = samples.len() / 2;
        self.written += frames as u64;
        frames
    }

    fn stats(&self) -> SinkStats {
        SinkStats {
            played_frames: self.written,
            ..SinkStats::default()
        }
    }

    fn connected(&self) -> bool {
        (self.live)()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    fn sine(freq: f32, amp: f32, secs: f32) -> Vec<f32> {
        let n = (RATE as f32 * secs) as usize;
        (0..n)
            .flat_map(|i| {
                let v = amp * (std::f32::consts::TAU * freq * i as f32 / RATE as f32).sin();
                [v, v]
            })
            .collect()
    }

    #[test]
    fn a_routed_tone_becomes_its_pitch_and_silence_none() {
        let got = Arc::new(Mutex::new(Vec::new()));
        let g = Arc::clone(&got);
        let live = Arc::new(AtomicBool::new(true));
        let l = Arc::clone(&live);
        let mut sink = HapticSink::new(
            move |t| g.lock().unwrap().push(t),
            move || l.load(Ordering::Relaxed),
        );
        assert!(sink.connected());
        let tone = sine(440.0, 0.5, 0.4);
        // Written in pump-sized pieces, as DirectSound would.
        for chunk in tone.chunks(2 * 512) {
            assert_eq!(sink.write(chunk), chunk.len() / 2);
        }
        sink.write(&vec![0.0; 2 * RATE as usize / 5]);
        let tones = got.lock().unwrap().clone();
        // 0.6 s of 40 ms frames.
        assert!((14..=15).contains(&tones.len()), "{}", tones.len());
        let heard: Vec<_> = tones.iter().flatten().collect();
        assert!(heard.len() >= 8, "{tones:?}");
        for t in &heard {
            assert!((t.freq - 440.0).abs() < 15.0, "{t:?}");
            assert!(t.amp > 0.2, "{t:?}");
        }
        assert!(tones.last().unwrap().is_none(), "the silence after");
        assert_eq!(sink.queued_frames(), 0);
        assert!(sink.free_frames() >= 2 * 512);
        assert_eq!(sink.stats().played_frames, (RATE as u64 * 6) / 10);

        live.store(false, Ordering::Relaxed);
        assert!(!sink.connected(), "no controller, no speaker");
    }
}
