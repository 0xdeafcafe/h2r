//! An I3DL2-style environmental reverb for the effects send.
//!
//! Clean-room: built only from the published I3DL2 / DirectSound
//! `DSI3DL2LISTENER` parameter semantics (decay time, HF decay ratio,
//! reflections and reverb level and delay, diffusion, density, HF
//! reference) and textbook reverberator structure (Schroeder allpass
//! diffusers, a Jot-style feedback delay network with per-line absorption).
//! The Xbox DSP image and its coefficient converter are not used.
//!
//! The model, for a mono send `x`:
//!
//! - **Early reflections**: a fixed pattern of seven taps starting at
//!   `reflections_delay`, alternating left and right, at
//!   `room + reflections` millibels.
//! - **Late reverb**: `x`, delayed by `reflections_delay + reverb_delay`,
//!   through two allpass diffusers (coefficient from `diffusion`), into a
//!   four-line feedback delay network with a Hadamard mixing matrix. Each
//!   line's loop gain gives the requested decay time (`-60 dB` after
//!   `decay_time` seconds) and a one-pole low-pass in the loop makes the
//!   decay at `hf_reference` last `decay_hf_ratio` times as long. Line
//!   lengths scale with `density`. Level `room + reverb` millibels.
//! - **Room HF**: the wet signal passes a one-pole low-pass that is `room_hf`
//!   millibels down at `hf_reference`.
//!
//! `room_rolloff` (distance dependence of the send) is not modelled: sends
//! reach the reverb at the level the voice's mix bin gives them.
//!
//! The reverb is plain arithmetic on its own state: given the same sequence
//! of inputs it produces the same sequence of outputs, whatever the block
//! sizes.

use std::f64::consts::PI;

use crate::fx::mb_gain;

/// `DSI3DL2LISTENER`. Levels in millibels, times in seconds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct I3dl2Listener {
    pub room: i32,
    pub room_hf: i32,
    pub room_rolloff: f32,
    pub decay_time: f32,
    pub decay_hf_ratio: f32,
    pub reflections: i32,
    pub reflections_delay: f32,
    pub reverb: i32,
    pub reverb_delay: f32,
    /// Percent, 0..=100.
    pub diffusion: f32,
    /// Percent, 0..=100.
    pub density: f32,
    pub hf_reference: f32,
}

impl I3dl2Listener {
    /// From the twelve dwords of a `DSI3DL2LISTENER` in declaration order
    /// (floats as bit patterns).
    pub fn from_words(w: [u32; 12]) -> Self {
        let f = f32::from_bits;
        Self {
            room: w[0] as i32,
            room_hf: w[1] as i32,
            room_rolloff: f(w[2]),
            decay_time: f(w[3]),
            decay_hf_ratio: f(w[4]),
            reflections: w[5] as i32,
            reflections_delay: f(w[6]),
            reverb: w[7] as i32,
            reverb_delay: f(w[8]),
            diffusion: f(w[9]),
            density: f(w[10]),
            hf_reference: f(w[11]),
        }
    }

    /// Clamped to the ranges the I3DL2 specification allows, with NaN taken
    /// as the I3DL2 default for that field.
    fn sane(mut self) -> Self {
        let clamp = |v: f32, lo: f32, hi: f32, d: f32| if v.is_nan() { d } else { v.clamp(lo, hi) };
        self.room = self.room.clamp(-10_000, 0);
        self.room_hf = self.room_hf.clamp(-10_000, 0);
        self.reflections = self.reflections.clamp(-10_000, 1000);
        self.reverb = self.reverb.clamp(-10_000, 2000);
        self.decay_time = clamp(self.decay_time, 0.1, 20.0, 1.49);
        self.decay_hf_ratio = clamp(self.decay_hf_ratio, 0.1, 2.0, 0.83);
        self.reflections_delay = clamp(self.reflections_delay, 0.0, 0.3, 0.007);
        self.reverb_delay = clamp(self.reverb_delay, 0.0, 0.1, 0.011);
        self.diffusion = clamp(self.diffusion, 0.0, 100.0, 100.0);
        self.density = clamp(self.density, 0.0, 100.0, 100.0);
        self.hf_reference = clamp(self.hf_reference, 20.0, 20_000.0, 5000.0);
        self
    }

    /// True when both the reflections and the late reverb are inaudible.
    pub fn is_silent(&self) -> bool {
        let p = self.sane();
        p.room + p.reflections.max(p.reverb) <= -9000
    }
}

#[derive(Clone, Debug)]
struct Delay {
    buf: Vec<f32>,
    pos: usize,
}

impl Delay {
    fn new(len: usize) -> Self {
        Self {
            buf: vec![0.0; len.max(1)],
            pos: 0,
        }
    }
    /// The sample written `len` samples ago (the oldest one).
    fn read(&self) -> f32 {
        self.buf[self.pos]
    }
    fn write(&mut self, v: f32) {
        self.buf[self.pos] = v;
        self.pos = (self.pos + 1) % self.buf.len();
    }
    /// The sample written `n` writes ago (`n >= 1`; 1 is the latest).
    fn tap(&self, n: usize) -> f32 {
        let len = self.buf.len();
        self.buf[(self.pos + len - n % len) % len]
    }
}

/// A Schroeder allpass: `y = -g x + d; d' = x + g y`.
#[derive(Clone, Debug)]
struct Allpass {
    line: Delay,
    g: f32,
}

impl Allpass {
    fn tick(&mut self, x: f32) -> f32 {
        let d = self.line.read();
        let w = x + self.g * d;
        let y = d - self.g * w;
        self.line.write(w);
        y
    }
}

const ER_OFFSETS_MS: [f64; 7] = [0.0, 6.7, 11.3, 17.9, 23.3, 31.1, 41.9];
const ER_GAINS: [f32; 7] = [1.0, -0.8, 0.7, -0.6, 0.5, -0.4, 0.3];
const FDN_MS: [f64; 4] = [29.7, 37.1, 41.1, 43.7];
const ALLPASS_MS: [f64; 2] = [5.0, 1.7];
/// Scale from a unit send to the late field's initial level.
const LATE_INPUT: f32 = 0.35;
const ER_OUTPUT: f32 = 0.5;

/// One-pole low-pass `y = (1 - a) x + a y1`, DC gain 1, with `db` of
/// attenuation at `ref_hz`.
fn pole_for(db: f64, ref_hz: f64, rate: f64) -> f32 {
    let r = 10f64.powf(db / 10.0);
    if r >= 1.0 {
        return 0.0;
    }
    let c = (2.0 * PI * ref_hz.min(rate * 0.45) / rate).cos();
    let b = 1.0 - r * c;
    let disc = (b * b - (1.0 - r) * (1.0 - r)).max(0.0);
    (((b - disc.sqrt()) / (1.0 - r)).clamp(0.0, 0.999_999)) as f32
}

/// The reverberator.
#[derive(Clone, Debug)]
pub struct Reverb {
    rate: u32,
    params: I3dl2Listener,
    er_line: Delay,
    er_taps: [usize; 7],
    pre: Delay,
    pre_len: usize,
    allpass: [Allpass; 2],
    fdn: [Delay; 4],
    /// Per-line loop gain at low frequencies and damping pole.
    line_gain: [f32; 4],
    line_pole: [f32; 4],
    line_z: [f32; 4],
    room_pole: f32,
    room_z: [f32; 2],
    er_level: f32,
    late_level: f32,
    /// Inputs the delay lines were sized for, so a level change in place
    /// keeps the tail.
    sized_for: (u32, i32, i32, i32),
}

impl Reverb {
    pub fn new(rate: u32, params: I3dl2Listener) -> Self {
        let mut r = Self {
            rate,
            params,
            er_line: Delay::new(1),
            er_taps: [1; 7],
            pre: Delay::new(1),
            pre_len: 1,
            allpass: [
                Allpass {
                    line: Delay::new(1),
                    g: 0.0,
                },
                Allpass {
                    line: Delay::new(1),
                    g: 0.0,
                },
            ],
            fdn: [Delay::new(1), Delay::new(1), Delay::new(1), Delay::new(1)],
            line_gain: [0.0; 4],
            line_pole: [0.0; 4],
            line_z: [0.0; 4],
            room_pole: 0.0,
            room_z: [0.0; 2],
            er_level: 0.0,
            late_level: 0.0,
            sized_for: (0, 0, 0, 0),
        };
        r.configure(params);
        r
    }

    pub fn params(&self) -> &I3dl2Listener {
        &self.params
    }

    /// New environment. Levels, decay and damping take effect at once and
    /// keep the tail; a change of delays or density clears it.
    pub fn configure(&mut self, params: I3dl2Listener) {
        let p = params.sane();
        self.params = params;
        let rate = f64::from(self.rate);
        let samples = |s: f64| ((s * rate).round() as usize).max(1);
        let scale = 0.5 + 0.5 * f64::from(p.density) / 100.0;
        let key = (
            self.rate,
            (p.reflections_delay * 1e6) as i32,
            (p.reverb_delay * 1e6) as i32,
            (p.density * 100.0) as i32,
        );
        if self.sized_for != key {
            self.sized_for = key;
            let first = f64::from(p.reflections_delay);
            self.er_line = Delay::new(samples(first + 0.045) + 2);
            for (t, off) in self.er_taps.iter_mut().zip(ER_OFFSETS_MS) {
                *t = samples(first + off / 1000.0);
            }
            for (a, ms) in self.allpass.iter_mut().zip(ALLPASS_MS) {
                a.line = Delay::new(samples(ms / 1000.0));
            }
            for (l, ms) in self.fdn.iter_mut().zip(FDN_MS) {
                *l = Delay::new(samples(ms / 1000.0 * scale));
            }
            // The late field is heard after the shortest FDN line has run
            // once, so the pre-delay is what is left of the requested onset.
            let onset = samples(first + f64::from(p.reverb_delay));
            self.pre_len = onset.saturating_sub(self.fdn[0].buf.len());
            self.pre = Delay::new(self.pre_len + 2);
            self.line_z = [0.0; 4];
            self.room_z = [0.0; 2];
        }
        for a in &mut self.allpass {
            a.g = 0.7 * p.diffusion / 100.0;
        }
        let ratio = f64::from(p.decay_hf_ratio);
        for i in 0..4 {
            let secs = self.fdn[i].buf.len() as f64 / rate;
            let g_lf = 10f64.powf(-3.0 * secs / f64::from(p.decay_time));
            let g_hf = 10f64.powf(-3.0 * secs / (f64::from(p.decay_time) * ratio));
            self.line_gain[i] = g_lf as f32;
            self.line_pole[i] = pole_for(
                20.0 * (g_hf / g_lf).log10(),
                f64::from(p.hf_reference),
                rate,
            );
        }
        self.room_pole = pole_for(
            f64::from(p.room_hf) / 100.0,
            f64::from(p.hf_reference),
            rate,
        );
        self.er_level = mb_gain((p.room + p.reflections) as f32) * ER_OUTPUT;
        self.late_level = mb_gain((p.room + p.reverb) as f32);
    }

    /// Reverberates `input` (mono) and adds the stereo result to `out`
    /// (interleaved).
    pub fn process(&mut self, input: &[f32], out: &mut [f32]) {
        for (x, o) in input.iter().zip(out.as_chunks_mut::<2>().0) {
            let x = *x;
            // Early reflections.
            self.er_line.write(x);
            let (mut er_l, mut er_r) = (0.0f32, 0.0f32);
            for (k, (&t, &g)) in self.er_taps.iter().zip(&ER_GAINS).enumerate() {
                let v = self.er_line.tap(t + 1) * g;
                if k % 2 == 0 { er_l += v } else { er_r += v }
            }
            // Late field.
            self.pre.write(x);
            let delayed = self.pre.tap(self.pre_len + 1);
            let mut d = delayed * LATE_INPUT;
            for a in &mut self.allpass {
                d = a.tick(d);
            }
            let y: [f32; 4] = std::array::from_fn(|i| {
                let raw = self.fdn[i].read();
                let z = (1.0 - self.line_pole[i]) * raw + self.line_pole[i] * self.line_z[i];
                self.line_z[i] = flush(z);
                z * self.line_gain[i]
            });
            let raw: [f32; 4] = std::array::from_fn(|i| self.fdn[i].read());
            // Hadamard mix, scaled by 1/2 (orthonormal).
            let m = [
                0.5 * (y[0] + y[1] + y[2] + y[3]),
                0.5 * (y[0] - y[1] + y[2] - y[3]),
                0.5 * (y[0] + y[1] - y[2] - y[3]),
                0.5 * (y[0] - y[1] - y[2] + y[3]),
            ];
            for (line, mi) in self.fdn.iter_mut().zip(m) {
                line.write(flush(mi + d));
            }
            let late_l = 0.5 * (raw[0] - raw[1] + raw[2] - raw[3]);
            let late_r = 0.5 * (raw[0] + raw[1] - raw[2] - raw[3]);
            let wet = [
                er_l * self.er_level + late_l * self.late_level,
                er_r * self.er_level + late_r * self.late_level,
            ];
            for ch in 0..2 {
                let z = (1.0 - self.room_pole) * wet[ch] + self.room_pole * self.room_z[ch];
                self.room_z[ch] = flush(z);
                o[ch] += z;
            }
        }
    }
}

/// Flushes values too small to matter, so a dying tail does not run on
/// denormals.
fn flush(v: f32) -> f32 {
    if v.abs() < 1e-20 { 0.0 } else { v }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn params() -> I3dl2Listener {
        I3dl2Listener {
            room: 0,
            room_hf: 0,
            room_rolloff: 0.0,
            decay_time: 1.0,
            decay_hf_ratio: 1.0,
            reflections: -10_000,
            reflections_delay: 0.01,
            reverb: 0,
            reverb_delay: 0.02,
            diffusion: 100.0,
            density: 100.0,
            hf_reference: 5000.0,
        }
    }

    /// The left channel of the impulse response, `secs` long.
    fn ir(p: I3dl2Listener, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(RATE)) as usize;
        let mut input = vec![0.0f32; n];
        input[0] = 1.0;
        let mut out = vec![0.0f32; n * 2];
        Reverb::new(RATE, p).process(&input, &mut out);
        out.chunks(2).map(|f| f[0]).collect()
    }

    /// Reverberation time from the Schroeder backward integral, fitted
    /// between -5 and -25 dB and extrapolated to -60 dB.
    fn t60(ir: &[f32]) -> f64 {
        let mut e = vec![0.0f64; ir.len()];
        let mut acc = 0.0;
        for i in (0..ir.len()).rev() {
            acc += f64::from(ir[i]).powi(2);
            e[i] = acc;
        }
        let db: Vec<f64> = e.iter().map(|v| 10.0 * (v / e[0]).log10()).collect();
        let t = |level: f64| db.iter().position(|&v| v < level).unwrap() as f64 / f64::from(RATE);
        let (a, b) = (t(-5.0), t(-25.0));
        60.0 * (b - a) / 20.0
    }

    #[test]
    fn decay_time_sets_the_reverberation_time() {
        for target in [0.6f32, 1.0, 2.5] {
            let h = ir(
                I3dl2Listener {
                    decay_time: target,
                    ..params()
                },
                6.0,
            );
            let measured = t60(&h);
            assert!(
                (measured / f64::from(target) - 1.0).abs() < 0.2,
                "decay {target}: measured {measured}"
            );
        }
    }

    #[test]
    fn first_reflection_arrives_at_its_delay() {
        let p = I3dl2Listener {
            reflections: 0,
            reverb: -10_000,
            reflections_delay: 0.05,
            ..params()
        };
        let h = ir(p, 0.2);
        let first = h.iter().position(|&v| v != 0.0).unwrap();
        assert_eq!(first, (0.05 * f64::from(RATE)) as usize);
        assert!(
            h.iter().take(first).all(|&v| v == 0.0),
            "silent before the first reflection"
        );
        // Later taps follow and the 7th is ~42 ms on.
        let last = h.iter().rposition(|&v| v.abs() > 1e-3).unwrap();
        assert!(((last - first) as f64 / f64::from(RATE) - 0.0419).abs() < 0.001);
    }

    #[test]
    fn late_reverb_starts_after_both_delays_and_not_before() {
        let h = ir(
            I3dl2Listener {
                reverb_delay: 0.03,
                reflections_delay: 0.02,
                ..params()
            },
            0.3,
        );
        let first = h.iter().position(|&v| v.abs() > 1e-3).unwrap();
        // Pre-delay (50 ms) plus the first diffuser's group delay at most.
        assert!(first >= (0.05 * f64::from(RATE)) as usize, "{first}");
        assert!(first < (0.06 * f64::from(RATE)) as usize, "{first}");
    }

    #[test]
    fn levels_are_in_millibels() {
        let energy = |p| {
            ir(p, 3.0)
                .iter()
                .map(|&v| f64::from(v).powi(2))
                .sum::<f64>()
        };
        let base = energy(params());
        assert!(base > 0.0);
        let quiet = energy(I3dl2Listener {
            reverb: -600,
            ..params()
        });
        assert!(
            (10.0 * (quiet / base).log10() + 6.0).abs() < 0.1,
            "-6 dB: {}",
            10.0 * (quiet / base).log10()
        );
        // Room level scales both.
        let room = energy(I3dl2Listener {
            room: -1000,
            ..params()
        });
        assert!((10.0 * (room / base).log10() + 10.0).abs() < 0.1);
        // Very low room level is silent.
        assert!(
            I3dl2Listener {
                room: -10_000,
                ..params()
            }
            .is_silent()
        );
        assert!(!params().is_silent());
    }

    #[test]
    fn hf_ratio_darkens_the_tail() {
        // Zero crossings in the tail: a smaller HF ratio loses the highs
        // sooner, so the late tail is smoother.
        let zc = |ratio: f32| {
            let h = ir(
                I3dl2Listener {
                    decay_hf_ratio: ratio,
                    decay_time: 2.0,
                    hf_reference: 2000.0,
                    ..params()
                },
                3.0,
            );
            let tail = &h[(1.0 * f64::from(RATE)) as usize..(2.0 * f64::from(RATE)) as usize];
            tail.windows(2).filter(|w| w[0] * w[1] < 0.0).count()
        };
        let bright = zc(1.0);
        let dark = zc(0.15);
        assert!(dark * 10 < bright * 8, "dark {dark} vs bright {bright}");
        // And the loop's damping filter has the attenuation asked for at the reference.
        let p = pole_for(-6.0, 2000.0, f64::from(RATE)) as f64;
        let w = 2.0 * PI * 2000.0 / f64::from(RATE);
        let mag = (1.0 - p) / ((1.0 - p * w.cos()).powi(2) + (p * w.sin()).powi(2)).sqrt();
        assert!((20.0 * mag.log10() + 6.0).abs() < 0.05);
    }

    #[test]
    fn room_hf_filters_the_wet_signal() {
        let zc = |room_hf: i32| {
            let h = ir(
                I3dl2Listener {
                    room_hf,
                    reflections: 0,
                    ..params()
                },
                1.0,
            );
            h.windows(2).filter(|w| w[0] * w[1] < 0.0).count()
        };
        assert!(zc(-3000) < zc(0));
    }

    #[test]
    fn diffusion_zero_is_still_stable_and_decays() {
        let h = ir(
            I3dl2Listener {
                diffusion: 0.0,
                density: 0.0,
                ..params()
            },
            5.0,
        );
        assert!(h.iter().all(|v| v.is_finite() && v.abs() < 2.0));
        assert!(h[h.len() - 1000..].iter().all(|v| v.abs() < 1e-3));
    }

    #[test]
    fn output_is_independent_of_block_size_and_repeatable() {
        let input: Vec<f32> = (0..9600)
            .map(|i| ((i * 37 % 101) as f32 - 50.0) / 50.0)
            .collect();
        let run = |block: usize| {
            let mut r = Reverb::new(
                RATE,
                I3dl2Listener {
                    reflections: -500,
                    ..params()
                },
            );
            let mut out = vec![0.0f32; input.len() * 2];
            for (i, o) in input.chunks(block).zip(out.chunks_mut(block * 2)) {
                r.process(i, o);
            }
            out
        };
        let a = run(9600);
        assert_eq!(a, run(64));
        assert_eq!(a, run(1000));
        assert_eq!(a, run(9600));
    }

    #[test]
    fn reconfiguring_levels_keeps_the_tail_but_new_delays_clear_it() {
        let mut r = Reverb::new(RATE, params());
        let mut out = vec![0.0f32; 4800 * 2];
        let mut input = vec![0.0f32; 4800];
        input[0] = 1.0;
        r.process(&input, &mut out);
        r.configure(I3dl2Listener {
            reverb: -600,
            ..params()
        });
        let mut out2 = vec![0.0f32; 4800 * 2];
        r.process(&vec![0.0; 4800], &mut out2);
        assert!(
            out2.iter().any(|&v| v != 0.0),
            "tail survives a level change"
        );
        r.configure(I3dl2Listener {
            reverb_delay: 0.05,
            ..params()
        });
        let mut out3 = vec![0.0f32; 480 * 2];
        r.process(&vec![0.0; 480], &mut out3);
        assert!(out3.iter().all(|&v| v == 0.0), "new delays reset the lines");
    }

    #[test]
    fn nan_and_out_of_range_parameters_are_safe() {
        let p = I3dl2Listener {
            decay_time: f32::NAN,
            decay_hf_ratio: f32::INFINITY,
            reflections_delay: -1.0,
            reverb_delay: 9.0,
            diffusion: 500.0,
            density: f32::NAN,
            hf_reference: 0.0,
            room: 77,
            ..params()
        };
        let h = ir(p, 1.0);
        assert!(h.iter().all(|v| v.is_finite()));
    }
}
