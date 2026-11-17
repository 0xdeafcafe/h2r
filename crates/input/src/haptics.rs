//! The Steam Controller's trackpad actuators as a speaker and as rumble.
//!
//! The controller has no speaker, but each trackpad has a haptic actuator
//! that buzzes at the rate it is pulsed: a train of pulses at a steady
//! period is a tone (the controller plays its own startup tune this way,
//! and people play music through it). So the routed sounds (the set the
//! DualSense speaker gets, `dsound::padroute`) are rendered as pitch and
//! loudness: [`ToneTracker`] cuts the mix into [`FRAME`]-long frames and
//! finds each one's fundamental and level ([`analyse`]); [`HapticMixer`]
//! turns that, and the game's rumble, into at most two reports a frame.
//!
//! Rates: one frame every 40 ms is 25 a second. A frame sends one pulse
//! report for both pads when they play the same thing, two when they
//! differ, and on the 2026 controller one rumble report while its motors
//! run (SDL resends that every 40 ms, as the firmware stops after about
//! 50 ms). Linux throttles the 2015 controller's interface to about 20
//! reports a second per kind; see `docs/input/steam-controller.md`.

use std::time::Duration;

use crate::gamepad::Rumble;
use crate::steam::{self, HidWrite, Model, PulseTrain, Side};

/// One haptic frame.
pub const FRAME: Duration = Duration::from_millis(40);
/// A pulse train lasts this much past its frame, so the next frame's
/// replaces it before it stops (no gap between frames of one note).
pub const OVERLAP: Duration = Duration::from_millis(15);
/// The pitch range searched and played.
pub const MIN_FREQ: f32 = 60.0;
pub const MAX_FREQ: f32 = 1000.0;
/// Below this RMS level a frame is silence.
pub const GATE: f32 = 0.01;
/// The rate the pitch search runs at (the mix is averaged down to it).
const ANALYSIS_RATE: u32 = 12_000;
/// YIN's threshold on the cumulative mean normalised difference.
const YIN_THRESHOLD: f32 = 0.15;
/// Beyond this, a frame has no clear pitch (noise, a clunk).
const YIN_ACCEPT: f32 = 0.45;

/// One frame of the routed sound.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tone {
    pub freq: f32,
    /// 0..=1.
    pub amp: f32,
}

/// The pitch and level of one frame of mono samples at `rate`, or `None`
/// for silence. Pitch is YIN's (de Cheveigné and Kawahara, 2002) over the
/// frame averaged down to about 12 kHz; a frame with no clear period (noise)
/// takes its zero-crossing rate instead, at a lower level. The level is the
/// RMS scaled so a full-scale sine is 1.
pub fn analyse(mono: &[f32], rate: u32) -> Option<Tone> {
    if mono.is_empty() || rate == 0 {
        return None;
    }
    let rms = (mono.iter().map(|v| v * v).sum::<f32>() / mono.len() as f32).sqrt();
    if !rms.is_finite() || rms < GATE {
        return None;
    }
    let amp = (rms * std::f32::consts::SQRT_2).min(1.0);
    let step = (rate / ANALYSIS_RATE).max(1) as usize;
    let x: Vec<f32> = mono
        .chunks(step)
        .map(|c| c.iter().sum::<f32>() / c.len() as f32)
        .collect();
    let r = rate as f32 / step as f32;
    if let Some(freq) = yin(&x, r) {
        return Some(Tone { freq, amp });
    }
    let crossings = mono
        .windows(2)
        .filter(|w| (w[0] < 0.0) != (w[1] < 0.0))
        .count();
    let freq = crossings as f32 / 2.0 / (mono.len() as f32 / rate as f32);
    Some(Tone {
        freq: freq.clamp(MIN_FREQ, MAX_FREQ),
        amp: amp * 0.7,
    })
}

fn yin(x: &[f32], rate: f32) -> Option<f32> {
    let tau_min = (rate / MAX_FREQ).floor().max(2.0) as usize;
    let tau_max = (rate / MIN_FREQ).ceil() as usize;
    if x.len() < tau_max + tau_min * 2 {
        return None;
    }
    let w = x.len() - tau_max;
    let mut d = vec![0f32; tau_max + 2];
    for (tau, slot) in d.iter_mut().enumerate().skip(1) {
        if tau + w > x.len() {
            break;
        }
        *slot = (0..w).map(|j| (x[j] - x[j + tau]).powi(2)).sum();
    }
    let mut cmnd = vec![1f32; d.len()];
    let mut sum = 0f32;
    for tau in 1..d.len() {
        sum += d[tau];
        cmnd[tau] = if sum > 0.0 {
            d[tau] * tau as f32 / sum
        } else {
            1.0
        };
    }
    let last = tau_max.min(d.len() - 2);
    let mut pick = None;
    let mut tau = tau_min;
    while tau <= last {
        if cmnd[tau] < YIN_THRESHOLD {
            while tau < last && cmnd[tau + 1] < cmnd[tau] {
                tau += 1;
            }
            pick = Some(tau);
            break;
        }
        tau += 1;
    }
    let tau = match pick {
        Some(t) => t,
        None => {
            let t = (tau_min..=last).min_by(|&a, &b| cmnd[a].total_cmp(&cmnd[b]))?;
            if cmnd[t] > YIN_ACCEPT {
                return None;
            }
            t
        }
    };
    // Parabolic interpolation around the minimum.
    let (a, b, c) = (cmnd[tau - 1], cmnd[tau], cmnd[tau + 1]);
    let den = a - 2.0 * b + c;
    let shift = if den.abs() > 1e-9 {
        (0.5 * (a - c) / den).clamp(-0.5, 0.5)
    } else {
        0.0
    };
    let freq = rate / (tau as f32 + shift);
    (MIN_FREQ..=MAX_FREQ).contains(&freq).then_some(freq)
}

/// Cuts interleaved stereo into [`FRAME`]s and analyses each.
#[derive(Clone, Debug)]
pub struct ToneTracker {
    rate: u32,
    frame_len: usize,
    mono: Vec<f32>,
}

impl ToneTracker {
    pub fn new(rate: u32) -> Self {
        let frame_len = ((u64::from(rate) * FRAME.as_millis() as u64) / 1000).max(1) as usize;
        Self {
            rate,
            frame_len,
            mono: Vec::with_capacity(frame_len),
        }
    }

    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Takes interleaved stereo; returns one entry per frame completed.
    pub fn push(&mut self, stereo: &[f32]) -> Vec<Option<Tone>> {
        let mut out = Vec::new();
        for f in stereo.as_chunks::<2>().0 {
            self.mono.push((f[0] + f[1]) * 0.5);
            if self.mono.len() == self.frame_len {
                out.push(analyse(&self.mono, self.rate));
                self.mono.clear();
            }
        }
        out
    }
}

fn train(freq: f32, duty: f32, dur: Duration) -> PulseTrain {
    let period = (1e6 / freq.clamp(MIN_FREQ / 2.0, MAX_FREQ))
        .round()
        .clamp(2.0, 65_535.0);
    let on = (period * duty.clamp(0.02, 0.5)).round().max(1.0);
    let count = (dur.as_micros() as f32 / period)
        .ceil()
        .clamp(1.0, 65_535.0);
    PulseTrain {
        on_us: on as u16,
        off_us: (period - on) as u16,
        count: count as u16,
        gain_db: 0,
    }
}

/// A tone as a pulse train lasting `dur`: one pulse per period of the
/// pitch, the pulse longer (up to half the period) the louder the frame.
pub fn tone_train(t: Tone, dur: Duration) -> Option<PulseTrain> {
    if !(t.amp > 0.0 && t.freq.is_finite()) {
        return None;
    }
    Some(train(t.freq, 0.1 + 0.4 * t.amp.min(1.0), dur))
}

/// The heavy (left) motor buzzes low and the light one higher, as on an
/// Xbox pad.
pub const HEAVY_FREQ: f32 = 35.0;
pub const LIGHT_FREQ: f32 = 90.0;
/// Motor levels below this (of 1) are off.
pub const RUMBLE_FLOOR: f32 = 0.02;

/// A rumble motor level (0..=1) as a pulse train lasting `dur`.
pub fn rumble_train(level: f32, heavy: bool, dur: Duration) -> Option<PulseTrain> {
    if level.is_nan() || level < RUMBLE_FLOOR {
        return None;
    }
    let freq = if heavy { HEAVY_FREQ } else { LIGHT_FREQ };
    Some(train(freq, 0.15 + 0.35 * level.min(1.0), dur))
}

/// What one frame of haptics sends to one controller.
#[derive(Clone, Debug)]
pub struct HapticMixer {
    model: Model,
    /// The 2026 controller's motors were running at the last frame.
    triton_rumbling: bool,
}

impl HapticMixer {
    pub fn new(model: Model) -> Self {
        Self {
            model,
            triton_rumbling: false,
        }
    }

    /// The reports for one frame: the game's `rumble` and the routed
    /// sound's `tone` (`None`: silence, or the speaker is off).
    ///
    /// 2015 controller: each pad plays whichever is stronger, the rumble of
    /// its motor (left heavy, right light) or the tone. 2026 controller: its
    /// rumble report while the motors run (and one to stop them), else the
    /// tone as pulses.
    pub fn frame(&mut self, rumble: Rumble, tone: Option<Tone>) -> Vec<HidWrite> {
        let dur = FRAME + OVERLAP;
        let level = |v: u16| f32::from(v) / 65_535.0;
        let tone = tone.filter(|t| t.amp > 0.0);
        match self.model {
            Model::Original => {
                let pick = |motor: f32, heavy: bool| match tone {
                    Some(t) if t.amp >= motor || motor < RUMBLE_FLOOR => tone_train(t, dur),
                    _ => rumble_train(motor, heavy, dur),
                };
                let left = pick(level(rumble.left_motor), true);
                let right = pick(level(rumble.right_motor), false);
                match (left, right) {
                    (Some(l), Some(r)) if l == r => {
                        vec![steam::haptic_pulse(self.model, Side::Both, l)]
                    }
                    (l, r) => [(Side::Left, l), (Side::Right, r)]
                        .into_iter()
                        .filter_map(|(side, t)| Some(steam::haptic_pulse(self.model, side, t?)))
                        .collect(),
                }
            }
            Model::Triton => {
                let running = level(rumble.left_motor) >= RUMBLE_FLOOR
                    || level(rumble.right_motor) >= RUMBLE_FLOOR;
                if running || self.triton_rumbling {
                    self.triton_rumbling = running;
                    let (l, r) = if running {
                        (rumble.left_motor, rumble.right_motor)
                    } else {
                        (0, 0)
                    };
                    return vec![steam::triton_rumble(l, r)];
                }
                tone.and_then(|t| tone_train(t, dur))
                    .map(|t| vec![steam::haptic_pulse(self.model, Side::Both, t)])
                    .unwrap_or_default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn sine(freq: f32, amp: f32, n: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * (std::f32::consts::TAU * freq * i as f32 / RATE as f32).sin())
            .collect()
    }

    fn frame_len() -> usize {
        RATE as usize * FRAME.as_millis() as usize / 1000
    }

    #[test]
    fn sines_are_found_within_two_percent() {
        for freq in [65.0, 110.0, 220.0, 330.0, 440.0, 660.0, 880.0] {
            let t = analyse(&sine(freq, 0.5, frame_len()), RATE).unwrap();
            assert!(
                (t.freq - freq).abs() / freq < 0.02,
                "{freq} Hz read as {}",
                t.freq
            );
            assert!((t.amp - 0.5).abs() < 0.02, "{}", t.amp);
        }
    }

    #[test]
    fn a_square_wave_reads_as_its_fundamental() {
        let sq: Vec<f32> = sine(150.0, 1.0, frame_len())
            .into_iter()
            .map(|v| 0.3 * v.signum())
            .collect();
        let t = analyse(&sq, RATE).unwrap();
        assert!((t.freq - 150.0).abs() < 3.0, "{}", t.freq);
    }

    #[test]
    fn silence_and_quiet_are_nothing_and_level_follows_loudness() {
        assert_eq!(analyse(&vec![0.0; frame_len()], RATE), None);
        assert_eq!(analyse(&sine(220.0, 0.005, frame_len()), RATE), None);
        assert_eq!(analyse(&[], RATE), None);
        let quiet = analyse(&sine(220.0, 0.1, frame_len()), RATE).unwrap();
        let loud = analyse(&sine(220.0, 0.9, frame_len()), RATE).unwrap();
        assert!(loud.amp > quiet.amp * 5.0);
        assert!(loud.amp <= 1.0);
        let mut bad = sine(220.0, 0.5, frame_len());
        bad[3] = f32::NAN;
        assert_eq!(analyse(&bad, RATE), None);
    }

    #[test]
    fn noise_still_buzzes_in_range() {
        // A deterministic LCG, not captured audio.
        let mut s = 0x1234_5678u32;
        let noise: Vec<f32> = (0..frame_len())
            .map(|_| {
                s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                (s >> 8) as f32 / (1u32 << 24) as f32 - 0.5
            })
            .collect();
        let t = analyse(&noise, RATE).unwrap();
        assert!((MIN_FREQ..=MAX_FREQ).contains(&t.freq), "{}", t.freq);
        assert!(t.amp > 0.0 && t.amp < 1.0);
    }

    #[test]
    fn the_tracker_cuts_stereo_into_frames() {
        let mut tr = ToneTracker::new(RATE);
        let mono = sine(440.0, 0.5, frame_len() * 5 / 2);
        let stereo: Vec<f32> = mono.iter().flat_map(|&v| [v, v]).collect();
        let (a, b) = stereo.split_at(1000);
        let mut frames = tr.push(a);
        frames.extend(tr.push(b));
        assert_eq!(frames.len(), 2, "two whole frames, half of one waiting");
        for f in &frames {
            assert!((f.unwrap().freq - 440.0).abs() < 9.0);
        }
        // Left and right are mixed: opposite channels cancel.
        let cancel: Vec<f32> = mono[..frame_len()].iter().flat_map(|&v| [v, -v]).collect();
        let mut tr = ToneTracker::new(RATE);
        assert_eq!(tr.push(&cancel), vec![None]);
    }

    #[test]
    fn a_tone_becomes_pulses_at_its_period() {
        let t = tone_train(
            Tone {
                freq: 250.0,
                amp: 1.0,
            },
            FRAME + OVERLAP,
        )
        .unwrap();
        assert_eq!(u32::from(t.on_us) + u32::from(t.off_us), 4000);
        assert_eq!(t.on_us, 2000, "full level: half the period");
        assert_eq!(t.count, 14, "55 ms of 4 ms periods, rounded up");
        let soft = tone_train(
            Tone {
                freq: 250.0,
                amp: 0.0,
            },
            FRAME,
        );
        assert_eq!(soft, None);
        let soft = tone_train(
            Tone {
                freq: 250.0,
                amp: 0.25,
            },
            FRAME,
        )
        .unwrap();
        assert_eq!(soft.on_us, 800);
        // Out-of-range pitches are clamped, never a zero period.
        let hi = tone_train(
            Tone {
                freq: 1e9,
                amp: 1.0,
            },
            FRAME,
        )
        .unwrap();
        assert_eq!(u32::from(hi.on_us) + u32::from(hi.off_us), 1000);
        assert!(rumble_train(0.01, true, FRAME).is_none());
        let heavy = rumble_train(1.0, true, FRAME).unwrap();
        let light = rumble_train(1.0, false, FRAME).unwrap();
        assert!(heavy.on_us + heavy.off_us > light.on_us + light.off_us);
    }

    #[test]
    fn the_2015_mixer_plays_the_louder_of_rumble_and_tone_per_pad() {
        let mut m = HapticMixer::new(Model::Original);
        assert!(
            m.frame(Rumble::default(), None).is_empty(),
            "silence sends nothing"
        );
        let tone = Some(Tone {
            freq: 200.0,
            amp: 0.5,
        });
        // A tone and no rumble: one report for both pads.
        let w = m.frame(Rumble::default(), tone);
        assert_eq!(w.len(), 1);
        assert_eq!((w[0].data[1], w[0].data[3]), (0x8f, 2));
        // Heavy rumble louder than the tone on the left: the left pad
        // rumbles, the right plays the tone.
        let w = m.frame(
            Rumble {
                left_motor: 60_000,
                right_motor: 0,
            },
            tone,
        );
        assert_eq!(w.len(), 2);
        assert_eq!(w[0].data[3], 1, "left");
        assert_eq!(w[1].data[3], 0, "right");
        let period = |w: &HidWrite| {
            u16::from_le_bytes([w.data[4], w.data[5]]) + u16::from_le_bytes([w.data[6], w.data[7]])
        };
        assert_eq!(period(&w[1]), 5000, "200 Hz");
        assert!(period(&w[0]) > 20_000, "a low buzz");
    }

    #[test]
    fn the_2026_mixer_sends_rumble_reports_then_stops_them() {
        let mut m = HapticMixer::new(Model::Triton);
        let tone = Some(Tone {
            freq: 300.0,
            amp: 1.0,
        });
        let w = m.frame(Rumble::default(), tone);
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].data[..2], [0x81, 2]);
        let r = Rumble {
            left_motor: 0x1000,
            right_motor: 0x2000,
        };
        for _ in 0..3 {
            let w = m.frame(r, tone);
            assert_eq!(
                w,
                vec![steam::triton_rumble(0x1000, 0x2000)],
                "resent every frame"
            );
        }
        assert_eq!(
            m.frame(Rumble::default(), None),
            vec![steam::triton_rumble(0, 0)]
        );
        assert!(m.frame(Rumble::default(), None).is_empty());
    }

    #[test]
    fn reports_stay_within_two_a_frame() {
        let mut o = HapticMixer::new(Model::Original);
        let mut t = HapticMixer::new(Model::Triton);
        let mut s = 7u32;
        for _ in 0..1000 {
            s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            let r = Rumble {
                left_motor: (s >> 16) as u16,
                right_motor: s as u16,
            };
            let tone = (s & 1 == 0).then_some(Tone {
                freq: 100.0 + (s % 800) as f32,
                amp: (s % 100) as f32 / 100.0,
            });
            assert!(o.frame(r, tone).len() <= 2);
            assert!(t.frame(r, tone).len() <= 1);
        }
        // 25 frames a second.
        assert_eq!(Duration::from_secs(1).as_millis() / FRAME.as_millis(), 25);
    }
}
