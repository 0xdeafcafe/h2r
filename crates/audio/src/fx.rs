//! Per-voice effects: filter, LFOs, amplitude envelope, I3DL2 source shelf.
//!
//! The Xbox MCPX gave every voice a filter, two LFOs and two envelope
//! generators. This module is a clean-room model of the parts a title can
//! reach through `SetFilter`, `SetLFO`, `SetEG` and `SetI3DL2Source`. The
//! descriptor encodings come from reading how Halo 2 builds them (see
//! `docs/audio/dsound.md`, "Voice effects"); the DSP itself is not reproduced.
//!
//! Everything runs in a voice's own *clock*: the number of host frames the
//! voice has produced since it (re)started. An effect therefore depends only
//! on the voice's own sample count, never on how the host chops the render
//! into callbacks or on wall time, and a stream that starves does not advance
//! its envelope. Nothing here changes how many frames a voice consumes, so
//! packet completion is untouched (pitch modulation is the one exception, by
//! design: it changes the playback rate, like `SetPitch`).
//!
//! The stages, in the order [`FxState::process`] applies them to a voice
//! rendered at unity gain: amplitude envelope and LFO tremolo, then the
//! filter cascade (cutoff modulated by the filter LFO), then the I3DL2
//! direct-path high-frequency shelf.

use std::f64::consts::PI;

/// The envelope generator's time unit: 512 samples at 48 kHz. Halo scales its
/// seconds by 93.75 before `SetEG` (`0x2ae7a3`).
pub const EG_UNITS_PER_SECOND: f64 = 93.75;
/// The LFO's tick: `dwDelay` is in 1/1500 s and `dwDelta` advances a 16-bit
/// phase per tick (`rate_hz = delta * 1500 / 65536`); Halo converts with the
/// constants 1500 and 43.69 (= 65536 / 1500) at `0x220452`.
pub const LFO_TICK_HZ: f64 = 1500.0;
/// The host rate the DSP ran at. Filter coefficient words are frequencies at
/// this rate.
pub const DSP_RATE: f64 = 48_000.0;
/// Largest voice headroom, millibels.
pub const MAX_VOICE_HEADROOM_MB: i32 = 6000;
/// Frequency the I3DL2 "HF" attenuations refer to when the listener has not
/// set one (the I3DL2 default).
pub const DEFAULT_HF_REFERENCE_HZ: f32 = 5000.0;

/// A filter response.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterKind {
    LowPass,
    HighPass,
    BandPass,
    /// Low and high pass together: everything but a band around the cutoff.
    Notch,
    /// A parametric-EQ band: boosts or cuts around the centre frequency.
    Peaking,
}

/// One second-order section.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Stage {
    pub kind: FilterKind,
    /// Cutoff or centre frequency, Hz.
    pub hz: f32,
    /// Resonance (Q); 0.7071 is maximally flat for a low or high pass.
    pub q: f32,
    /// Boost in dB; only [`FilterKind::Peaking`] uses it.
    pub gain_db: f32,
}

/// A voice's `SetFilter`: up to two cascaded sections. Empty means bypass.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FilterSpec {
    pub stages: [Option<Stage>; 2],
}

/// `DSFILTER_MODE_*`.
pub const FILTER_BYPASS: u32 = 0;
pub const FILTER_DLS2: u32 = 1;
pub const FILTER_PARAMEQ: u32 = 2;
pub const FILTER_MULTI: u32 = 3;

/// A cutoff coefficient word to Hz. The word is `4096 * log2(F)` as a signed
/// 16-bit value with `F = 2 sin(pi f / 48000)`: that is what Halo's converter
/// computes (`0x21e270`; frequencies under 30 Hz give `0x8000`, over 8 kHz
/// give 0, which decodes to 8 kHz).
pub fn coeff_to_hz(word: u32) -> f32 {
    let c = f64::from(word as u16 as i16);
    let f = (c / 4096.0).exp2().clamp(1e-4, 2.0);
    (((f / 2.0).asin()) * DSP_RATE / PI) as f32
}

/// The inverse of [`coeff_to_hz`], for building descriptors in tests and
/// tools: the coefficient word of a cutoff in Hz.
pub fn hz_to_coeff(hz: f32) -> u32 {
    if hz < 30.0 {
        return 0x8000;
    }
    if hz > 8000.0 {
        return 0;
    }
    let f = 2.0 * (PI * f64::from(hz) / DSP_RATE).sin();
    ((f.ln() * 4096.0 / std::f64::consts::LN_2) as i32 & 0xffff) as u32
}

/// A damping coefficient word to Q. The word is `32768 * 10^(-dB / 20)`
/// (`0x21e2d0`), the reciprocal of Q in Q15, so a resonance of `dB` is a Q of
/// `10^(dB / 20)`.
pub fn damping_to_q(word: u32) -> f32 {
    let d = f64::from(word.min(0xffff)) / 32768.0;
    (1.0 / d.max(1.0 / 64.0)) as f32
}

/// A parametric-EQ gain word to dB. Halo's converter (`0x21e330`) computes
/// `v = (10^(dB/20) - 1) * 8192` and encodes it in 16 bits: a boost
/// (`v >= 0`) is `v` limited to `0x7fff`; a cut (`v < 0`) is biased,
/// `0xc000 - v`, limited to `0xc000..=0xdfff`. Words in `0x8000..0xc000`
/// are not produced by it and decode as the largest boost.
pub fn gain_word_to_db(word: u32) -> f32 {
    let c = word & 0xffff;
    let v = if c >= 0xc000 {
        -f64::from((c.min(0xdfff)) - 0xc000)
    } else {
        f64::from(c.min(0x7fff))
    };
    let ratio = 1.0 + v / 8192.0;
    // The encoded range keeps `ratio` above 1/8192 (-78.3 dB); the floor only
    // guards the logarithm.
    (20.0 * ratio.max(1e-9).log10()) as f32
}

/// The inverse of [`gain_word_to_db`], mirroring the converter, for tests
/// and tools.
pub fn db_to_gain_word(db: f32) -> u32 {
    let v = ((10f64.powf(f64::from(db) / 20.0) - 1.0) * 8192.0) as i32;
    if v >= 0 {
        v.min(0x7fff) as u32
    } else {
        (0xc000 - v).clamp(0xc000, 0xdfff) as u32
    }
}

/// Q for a parametric-EQ `dwQCoefficient` index `0..=7`. The hardware's
/// table is not known; this one spans 0.5 (broad) to 8 (narrow) in equal
/// ratio steps.
pub fn parameq_q(index: u32) -> f32 {
    0.5 * 16f32.powf(index.min(7) as f32 / 7.0)
}

impl FilterSpec {
    /// Builds a spec from a `DSFILTERDESC` (`dwMode`, `dwQCoefficient`,
    /// four coefficient words), as Halo fills it:
    ///
    /// - `DSFILTER_MODE_DLS2` (1): a two-pole resonant low pass. `c[0]` is
    ///   the cutoff word and `c[1]` the damping word; `c[2]` and `c[3]`
    ///   (a second cutoff/damping pair) are not modelled.
    /// - `DSFILTER_MODE_PARAMEQ` (2): two peaking bands, `(c[0], c[1])` and
    ///   `(c[2], c[3])` as (frequency word, gain word), sharing the Q of
    ///   `dwQCoefficient` (0..=7, [`parameq_q`]).
    /// - `DSFILTER_MODE_MULTI` (3): a state-variable filter with cutoff
    ///   `c[0]` and damping `c[1]`. `dwQCoefficient` (0..=7, Halo clamps it)
    ///   is taken as the output selector: bit 0 low pass, bit 1 band pass,
    ///   bit 2 high pass. Combinations other than low+high (a notch) are
    ///   approximated by their dominant member; 0 and 7 bypass. This reading
    ///   of the selector is an assumption (see the docs).
    ///
    /// Mode 0 and unknown modes bypass.
    pub fn from_dsfilter(mode: u32, q_coefficient: u32, c: [u32; 4]) -> Self {
        let mut s = Self::default();
        match mode {
            FILTER_DLS2 => {
                s.stages[0] = Some(Stage {
                    kind: FilterKind::LowPass,
                    hz: coeff_to_hz(c[0]),
                    q: damping_to_q(c[1]),
                    gain_db: 0.0,
                });
            }
            FILTER_PARAMEQ => {
                let q = parameq_q(q_coefficient);
                for (i, pair) in c.chunks(2).enumerate() {
                    let db = gain_word_to_db(pair[1]);
                    if db.abs() > 0.01 {
                        s.stages[i] = Some(Stage {
                            kind: FilterKind::Peaking,
                            hz: coeff_to_hz(pair[0]),
                            q,
                            gain_db: db,
                        });
                    }
                }
            }
            FILTER_MULTI => {
                let kind = match q_coefficient & 7 {
                    1 | 3 => Some(FilterKind::LowPass),
                    2 => Some(FilterKind::BandPass),
                    4 | 6 => Some(FilterKind::HighPass),
                    5 => Some(FilterKind::Notch),
                    _ => None,
                };
                if let Some(kind) = kind {
                    s.stages[0] = Some(Stage {
                        kind,
                        hz: coeff_to_hz(c[0]),
                        q: damping_to_q(c[1]),
                        gain_db: 0.0,
                    });
                }
            }
            _ => {}
        }
        s
    }

    pub fn is_bypass(&self) -> bool {
        self.stages.iter().all(Option::is_none)
    }
}

/// One LFO (`DSLFODESC`). A voice has two: index 1 is the pitch LFO and
/// index 0 the "multi" LFO, as Halo uses them (it fills pitch for index 1 and
/// filter cutoff and amplitude for index 0); each applies whichever of its
/// three depths are non-zero.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LfoSpec {
    /// Seconds before the oscillation starts.
    pub delay_s: f32,
    pub rate_hz: f32,
    /// Peak pitch deviation, octaves.
    pub pitch_oct: f32,
    /// Peak cutoff deviation, octaves.
    pub filter_oct: f32,
    /// Peak amplitude deviation, dB.
    pub amp_db: f32,
}

/// Depth units. The hardware's are not documented in anything read; these
/// are the scales that make Halo's own conversions (pitch `x128`, filter and
/// amplitude `x16`, clamped to a signed byte) span a musically sensible
/// range: pitch 1/128 octave, cutoff 1/16 octave, amplitude 1/16 dB.
pub const LFO_PITCH_UNITS_PER_OCTAVE: f32 = 128.0;
pub const LFO_FILTER_UNITS_PER_OCTAVE: f32 = 16.0;
pub const LFO_AMP_UNITS_PER_DB: f32 = 16.0;

impl LfoSpec {
    /// From the five `DSLFODESC` fields after `dwLFO`.
    pub fn from_dslfo(delay: u32, delta: u32, pitch: i32, filter: i32, amp: i32) -> Self {
        Self {
            delay_s: (delay & 0xffff) as f32 / LFO_TICK_HZ as f32,
            rate_hz: ((delta & 0x3ff) as f64 * LFO_TICK_HZ / 65536.0) as f32,
            pitch_oct: pitch.clamp(-128, 127) as f32 / LFO_PITCH_UNITS_PER_OCTAVE,
            filter_oct: filter.clamp(-128, 127) as f32 / LFO_FILTER_UNITS_PER_OCTAVE,
            amp_db: amp.clamp(-128, 128) as f32 / LFO_AMP_UNITS_PER_DB,
        }
    }

    /// The triangle wave at `t` seconds of voice time: 0 until the delay has
    /// passed, then rising from 0 to +1, down to -1 and back.
    pub fn value(&self, t: f64) -> f64 {
        let t = t - f64::from(self.delay_s);
        if t < 0.0 || self.rate_hz <= 0.0 {
            return 0.0;
        }
        let p = (t * f64::from(self.rate_hz)).fract();
        if p < 0.25 {
            4.0 * p
        } else if p < 0.75 {
            2.0 - 4.0 * p
        } else {
            4.0 * p - 4.0
        }
    }
}

/// The amplitude envelope (`DSENVELOPEDESC`, generator index 1): delay,
/// attack, hold, decay then sustain until the voice stops. Times are in
/// [`EG_UNITS_PER_SECOND`] units. The release stage is not entered: stops
/// and flushes are immediate here (see the docs), so the sound is gone
/// before a release could be heard.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvSpec {
    /// `DSEG_MODE_*`: the stage the envelope starts in (1 delay, 2 attack, 3
    /// hold, 4 decay, 5 sustain). 0 is bypass and never reaches here.
    pub start_stage: u32,
    pub delay: u32,
    pub attack: u32,
    pub hold: u32,
    pub decay: u32,
    pub release: u32,
    /// 0..=255; 255 is full level.
    pub sustain: u32,
}

impl EnvSpec {
    /// The linear gain `t` seconds after the envelope started.
    pub fn gain(&self, t: f64) -> f64 {
        let u = EG_UNITS_PER_SECOND;
        let (delay, attack, hold, decay) = (
            f64::from(self.delay) / u,
            f64::from(self.attack) / u,
            f64::from(self.hold) / u,
            f64::from(self.decay) / u,
        );
        let sustain = f64::from(self.sustain.min(255)) / 255.0;
        // Starting in a later stage skips the earlier ones.
        let mut t = t;
        t += match self.start_stage {
            0 | 1 => 0.0,
            2 => delay,
            3 => delay + attack,
            4 => delay + attack + hold,
            _ => delay + attack + hold + decay,
        };
        if t < delay {
            return 0.0;
        }
        t -= delay;
        if t < attack {
            return t / attack;
        }
        t -= attack;
        if t < hold {
            return 1.0;
        }
        t -= hold;
        if t < decay {
            return 1.0 + (sustain - 1.0) * (t / decay);
        }
        sustain
    }
}

/// `DSI3DL2BUFFER`: how a source is occluded and how much it sends to the
/// room. Levels are millibels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct I3dl2Source {
    pub direct: i32,
    pub direct_hf: i32,
    pub room: i32,
    pub room_hf: i32,
    pub room_rolloff: f32,
    pub obstruction_hf: i32,
    pub obstruction_lf_ratio: f32,
    pub occlusion_hf: i32,
    pub occlusion_lf_ratio: f32,
}

impl Default for I3dl2Source {
    fn default() -> Self {
        Self {
            direct: 0,
            direct_hf: 0,
            room: 0,
            room_hf: 0,
            room_rolloff: 0.0,
            obstruction_hf: 0,
            obstruction_lf_ratio: 0.0,
            occlusion_hf: 0,
            occlusion_lf_ratio: 0.25,
        }
    }
}

impl I3dl2Source {
    /// From the nine dwords of a `DSI3DL2BUFFER` (floats as bit patterns),
    /// sanitised: levels are limited to the I3DL2 ranges and ratios and
    /// the rolloff to finite values, so nothing downstream sees NaN or
    /// infinity whatever the guest wrote.
    pub fn from_words(w: [u32; 9]) -> Self {
        let f = f32::from_bits;
        Self {
            direct: w[0] as i32,
            direct_hf: w[1] as i32,
            room: w[2] as i32,
            room_hf: w[3] as i32,
            room_rolloff: f(w[4]),
            obstruction_hf: w[5] as i32,
            obstruction_lf_ratio: f(w[6]),
            occlusion_hf: w[7] as i32,
            occlusion_lf_ratio: f(w[8]),
        }
        .sane()
    }

    /// Clamps to the I3DL2 ranges. NaN ratios and rolloff take their
    /// defaults.
    pub fn sane(mut self) -> Self {
        let ratio = |v: f32, d: f32| if v.is_nan() { d } else { v.clamp(0.0, 1.0) };
        self.direct = self.direct.clamp(-10_000, 1000);
        self.direct_hf = self.direct_hf.clamp(-10_000, 0);
        self.room = self.room.clamp(-10_000, 1000);
        self.room_hf = self.room_hf.clamp(-10_000, 0);
        self.obstruction_hf = self.obstruction_hf.clamp(-10_000, 0);
        self.occlusion_hf = self.occlusion_hf.clamp(-10_000, 0);
        self.obstruction_lf_ratio = ratio(self.obstruction_lf_ratio, 0.0);
        self.occlusion_lf_ratio = ratio(self.occlusion_lf_ratio, 0.25);
        self.room_rolloff = if self.room_rolloff.is_nan() {
            0.0
        } else {
            self.room_rolloff.clamp(0.0, 10.0)
        };
        self
    }

    /// Direct-path level at low frequencies, millibels: the direct level plus
    /// the low-frequency share of obstruction and occlusion, which the I3DL2
    /// model gives as `lFLevel = lHFLevel * flLFRatio`.
    pub fn direct_lf_mb(&self) -> f32 {
        self.direct as f32
            + self.obstruction_hf as f32 * self.obstruction_lf_ratio
            + self.occlusion_hf as f32 * self.occlusion_lf_ratio
    }

    /// Direct-path level at the HF reference, millibels.
    pub fn direct_hf_mb(&self) -> f32 {
        self.direct as f32
            + self.direct_hf as f32
            + self.obstruction_hf as f32
            + self.occlusion_hf as f32
    }

    /// Linear gain of the direct path at low frequencies.
    pub fn direct_gain(&self) -> f32 {
        mb_gain(self.direct_lf_mb())
    }

    /// Linear gain of the room (reverb) send: the room level plus the
    /// low-frequency share of occlusion (obstruction leaves the room alone).
    pub fn room_gain(&self) -> f32 {
        mb_gain(self.room as f32 + self.occlusion_hf as f32 * self.occlusion_lf_ratio)
    }

    /// The direct path's high-frequency attenuation relative to its low
    /// frequencies, in dB (<= 0), as a one-pole shelf at the HF reference.
    pub fn shelf_db(&self) -> f32 {
        ((self.direct_hf_mb() - self.direct_lf_mb()) / 100.0).min(0.0)
    }

    /// True when the direct path is the plain open path.
    pub fn is_open(&self) -> bool {
        self.direct_lf_mb() == 0.0 && self.shelf_db() == 0.0 && self.room_gain() == 1.0
    }
}

/// `10^(mb / 2000)` for millibels of either sign; exactly 1 at 0.
pub fn mb_gain(mb: f32) -> f32 {
    if mb == 0.0 {
        1.0
    } else {
        10f32.powf(mb / 2000.0)
    }
}

/// The gain of a voice's `SetHeadroom` (millibels, `0..=6000`): the voice
/// is attenuated by its headroom, exactly 1 at 0. Halo sets 0 on every voice
/// and mix bin it creates, so it is unaffected.
///
/// Mix-bin headroom (`SetMixBinHeadroom`) has no level effect here: on the
/// MCPX a bin's headroom bits are taken off on the way in and given back on
/// the way out, so the only audible effect is where the bin clips, and the
/// float mixer does not clip until the output stage.
pub fn headroom_gain(voice_mb: i32) -> f32 {
    let mb = voice_mb.clamp(0, MAX_VOICE_HEADROOM_MB);
    if mb == 0 {
        1.0
    } else {
        10f32.powf(-(mb as f32) / 2000.0)
    }
}

/// A voice's effect configuration (everything `Set*` stores).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceFx {
    pub filter: FilterSpec,
    /// Index 0: filter and amplitude LFO; index 1: pitch LFO.
    pub lfo: [Option<LfoSpec>; 2],
    pub env: Option<EnvSpec>,
    /// `SetHeadroom`, millibels `0..=6000` (see [`headroom_gain`]).
    pub headroom_mb: i32,
    pub i3dl2: Option<I3dl2Source>,
}

impl VoiceFx {
    /// Whether [`FxState::process`] would change a voice's samples.
    pub fn processes(&self) -> bool {
        !self.filter.is_bypass()
            || self
                .lfo
                .iter()
                .flatten()
                .any(|l| l.filter_oct != 0.0 || l.amp_db != 0.0)
            || self.env.is_some()
    }

    fn modulates_pitch(&self) -> bool {
        self.lfo.iter().flatten().any(|l| l.pitch_oct != 0.0)
    }
}

/// Second-order section coefficients, normalised so `a0 = 1`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    /// RBJ audio-EQ-cookbook biquads (published, textbook formulas).
    fn new(s: &Stage, rate: f64) -> Self {
        // max/min rather than clamp: a very low host rate must not panic.
        let f = f64::from(s.hz).max(10.0).min(rate * 0.45);
        let q = f64::from(s.q).clamp(0.1, 40.0);
        let w = 2.0 * PI * f / rate;
        let (sin, cos) = w.sin_cos();
        let alpha = sin / (2.0 * q);
        let (b0, b1, b2, a0, a1, a2) = match s.kind {
            FilterKind::LowPass => (
                (1.0 - cos) / 2.0,
                1.0 - cos,
                (1.0 - cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::HighPass => (
                (1.0 + cos) / 2.0,
                -(1.0 + cos),
                (1.0 + cos) / 2.0,
                1.0 + alpha,
                -2.0 * cos,
                1.0 - alpha,
            ),
            FilterKind::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::Notch => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            FilterKind::Peaking => {
                let a = 10f64.powf(f64::from(s.gain_db) / 40.0);
                (
                    1.0 + alpha * a,
                    -2.0 * cos,
                    1.0 - alpha * a,
                    1.0 + alpha / a,
                    -2.0 * cos,
                    1.0 - alpha / a,
                )
            }
        };
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
        }
    }
}

/// Transposed direct form II state, one pair per channel.
#[derive(Clone, Copy, Debug, Default)]
struct BiquadState {
    z1: [f64; 2],
    z2: [f64; 2],
}

impl BiquadState {
    fn tick(&mut self, c: &Coeffs, ch: usize, x: f64) -> f64 {
        let y = c.b0 * x + self.z1[ch];
        self.z1[ch] = flush64(c.b1 * x - c.a1 * y + self.z2[ch]);
        self.z2[ch] = flush64(c.b2 * x - c.a2 * y);
        y
    }
}

/// Flushes values too small to matter to zero, so recursive state decaying in
/// silence never reaches the denormal range (slow on some hosts), without
/// depending on a floating-point mode.
pub(crate) fn flush64(v: f64) -> f64 {
    if v.abs() < 1e-30 { 0.0 } else { v }
}

/// A one-pole low-pass shelf with unity gain at DC and the requested gain at
/// the reference frequency (`y += (1 - a)(x - y)`-style, solved exactly).
fn shelf_pole(db_at_ref: f32, ref_hz: f64, rate: f64) -> f64 {
    let r = 10f64.powf(f64::from(db_at_ref) / 10.0); // squared magnitude ratio
    if r >= 1.0 {
        return 0.0;
    }
    let c = (2.0 * PI * ref_hz.min(rate * 0.45) / rate).cos();
    // |(1-a) / (1 - a e^-jw)|^2 = r  =>  (1-r) a^2 - 2 (1 - r c) a + (1 - r) = 0
    let b = 1.0 - r * c;
    let disc = (b * b - (1.0 - r) * (1.0 - r)).max(0.0);
    ((b - disc.sqrt()) / (1.0 - r)).clamp(0.0, 0.999_999)
}

/// The mutable side of a voice's effects: its clock and filter memory.
#[derive(Clone, Debug)]
pub struct FxState {
    /// Frames produced since the voice (re)started.
    pub(crate) clock: u64,
    /// Clock value at which the envelope started.
    env_start: u64,
    /// Playback-rate multiplier from 3D Doppler, set by the mixer before a
    /// render.
    pub(crate) doppler: f64,
    bq: [BiquadState; 2],
    coeffs: [Option<Coeffs>; 2],
    /// Inputs the cached `coeffs` were built from.
    cached: Option<(FilterSpec, i32, u32)>,
    shelf_z: [f64; 2],
    /// Stream 3D/LFO pitch effects move the *read* position by this many
    /// frames, bounded by [`MAX_PHASE_OFFSET`], instead of changing the
    /// consumption rate (see `pitch_mult`).
    pub(crate) phase_off: f64,
}

/// Bound on the read-position offset pitch modulation may build up on a
/// stream, in frames.
pub const MAX_PHASE_OFFSET: f64 = 512.0;

impl Default for FxState {
    fn default() -> Self {
        Self {
            clock: 0,
            env_start: 0,
            doppler: 1.0,
            bq: [BiquadState::default(); 2],
            coeffs: [None; 2],
            cached: None,
            shelf_z: [0.0; 2],
            phase_off: 0.0,
        }
    }
}

impl FxState {
    /// The voice starts over: clock, envelope and filter memory.
    pub fn reset(&mut self) {
        let doppler = self.doppler;
        *self = Self::default();
        self.doppler = doppler;
    }

    /// `SetEG` while running restarts the envelope from now.
    pub fn restart_envelope(&mut self) {
        self.env_start = self.clock;
    }

    /// Pitch multiplier (>0) at the current clock: the pitch LFO and Doppler.
    pub fn pitch_mult(&self, fx: &VoiceFx, rate: u32) -> f64 {
        let mut m = self.doppler;
        if fx.modulates_pitch() {
            let t = self.clock as f64 / f64::from(rate);
            let oct: f64 = fx
                .lfo
                .iter()
                .flatten()
                .map(|l| f64::from(l.pitch_oct) * l.value(t))
                .sum();
            m *= oct.exp2();
        }
        // Never NaN, zero or runaway, whatever Doppler or the LFO gave.
        if m.is_finite() && m > 0.0 {
            m.clamp(0.125, 8.0)
        } else {
            1.0
        }
    }

    /// Applies the voice's effects to `frames` interleaved stereo frames of
    /// `buf` that were rendered at voice clocks `c0..c0 + frames`.
    pub fn process(&mut self, fx: &VoiceFx, buf: &mut [f32], c0: u64, rate: u32) {
        if !fx.processes() {
            return;
        }
        let rate_f = f64::from(rate);
        for (i, frame) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let c = c0 + i as u64;
            let t = c as f64 / rate_f;
            let mut amp = 1.0f64;
            if let Some(env) = &fx.env {
                amp *= env.gain(c.saturating_sub(self.env_start) as f64 / rate_f);
            }
            let mut cutoff_oct = 0.0f64;
            for l in fx.lfo.iter().flatten() {
                if l.amp_db != 0.0 || l.filter_oct != 0.0 {
                    let v = l.value(t);
                    amp *= 10f64.powf(f64::from(l.amp_db) * v / 20.0);
                    cutoff_oct += f64::from(l.filter_oct) * v;
                }
            }
            if !fx.filter.is_bypass() {
                // Cutoff modulation is quantised to 1/64 octave so the
                // coefficients are rebuilt only when it visibly moves.
                let q = (cutoff_oct * 64.0).round() as i32;
                if self.cached != Some((fx.filter, q, rate)) {
                    for (slot, stage) in self.coeffs.iter_mut().zip(&fx.filter.stages) {
                        *slot = stage.map(|s| {
                            let mut s = s;
                            if s.kind != FilterKind::Peaking {
                                s.hz *= (f64::from(q) / 64.0).exp2() as f32;
                            }
                            Coeffs::new(&s, rate_f)
                        });
                    }
                    self.cached = Some((fx.filter, q, rate));
                }
            } else {
                self.coeffs = [None; 2];
                self.cached = None;
            }
            for (ch, sample) in frame.iter_mut().enumerate() {
                let mut x = f64::from(*sample) * amp;
                for (st, co) in self.bq.iter_mut().zip(&self.coeffs) {
                    if let Some(co) = co {
                        x = st.tick(co, ch, x);
                    }
                }
                *sample = x as f32;
            }
        }
    }

    /// The I3DL2 direct path of a voice: `src` (the voice after the common
    /// effects) through the source's high-frequency shelf into `dst`.
    /// Returns false, leaving `dst` alone, when the source has no shelf (the
    /// direct path is then `src` itself). Obstruction and the direct HF level
    /// belong to the direct path only, so the room send is taken from `src`.
    pub fn direct_shelf(&mut self, fx: &VoiceFx, src: &[f32], dst: &mut [f32], rate: u32) -> bool {
        let Some(db) = fx.i3dl2.map(|i| i.shelf_db()).filter(|&d| d < 0.0) else {
            return false;
        };
        let a = shelf_pole(db, f64::from(DEFAULT_HF_REFERENCE_HZ), f64::from(rate));
        for (s, d) in src
            .as_chunks::<2>()
            .0
            .iter()
            .zip(dst.as_chunks_mut::<2>().0)
        {
            for ch in 0..2 {
                // y = (1 - a) x + a y1, DC gain 1.
                let y = flush64((1.0 - a) * f64::from(s[ch]) + a * self.shelf_z[ch]);
                self.shelf_z[ch] = y;
                d[ch] = y as f32;
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// RMS of a sine of `hz` through the effects, measured after the
    /// transient. Input amplitude 1 so the RMS reads as gain / sqrt(2).
    fn gain_at(fx: &VoiceFx, hz: f64) -> f64 {
        let n = RATE as usize;
        let mut buf = vec![0.0f32; n * 2];
        for (i, f) in buf.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let v = (2.0 * PI * hz * i as f64 / f64::from(RATE)).sin() as f32;
            *f = [v, v];
        }
        let mut st = FxState::default();
        st.process(fx, &mut buf, 0, RATE);
        let tail = &buf.as_chunks::<2>().0[n / 2..];
        let ms: f64 = tail.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>() / tail.len() as f64;
        (ms * 2.0).sqrt()
    }

    /// Gain of the I3DL2 direct path (the shelf) on a sine of `hz`.
    fn direct_at(fx: &VoiceFx, hz: f64) -> f64 {
        let n = RATE as usize;
        let mut src = vec![0.0f32; n * 2];
        for (i, f) in src.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let v = (2.0 * PI * hz * i as f64 / f64::from(RATE)).sin() as f32;
            *f = [v, v];
        }
        let mut dst = vec![0.0f32; n * 2];
        assert!(FxState::default().direct_shelf(fx, &src, &mut dst, RATE));
        let tail = &dst.as_chunks::<2>().0[n / 2..];
        let ms: f64 = tail.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>() / tail.len() as f64;
        (ms * 2.0).sqrt()
    }

    fn db(g: f64) -> f64 {
        20.0 * g.log10()
    }

    fn fx_with(filter: FilterSpec) -> VoiceFx {
        VoiceFx {
            filter,
            ..Default::default()
        }
    }

    #[test]
    fn coefficient_words_round_trip() {
        for hz in [40.0f32, 100.0, 440.0, 1000.0, 4000.0, 7999.0] {
            let back = coeff_to_hz(hz_to_coeff(hz));
            assert!((back / hz - 1.0).abs() < 0.01, "{hz} -> {back}");
        }
        // Out of range encodes as the converter does: 8 kHz and "off".
        assert_eq!(hz_to_coeff(20_000.0), 0);
        assert!((coeff_to_hz(0) - 8000.0).abs() < 1.0);
        assert_eq!(hz_to_coeff(10.0), 0x8000);
        assert!(coeff_to_hz(0x8000) < 40.0);
        // Damping: 0 dB resonance is Q 1; 20 dB is Q 10.
        assert!((damping_to_q(32768) - 1.0).abs() < 1e-6);
        assert!((damping_to_q(3277) - 10.0).abs() < 0.05);
        // Gain words: +6.02 dB is a doubling.
        assert!((gain_word_to_db(8192) - 6.02).abs() < 0.01);
        // A cut is biased, as the converter emits it: half amplitude is
        // v = -4096, word 0xc000 + 4096, and decodes to -6.02 dB (not a boost).
        assert!((gain_word_to_db(0xc000 + 4096) + 6.02).abs() < 0.01);
        assert_eq!(gain_word_to_db(0xc000), 0.0);
        // Analytical values, not a round trip: 0xd000 is v = -4096, ratio 1/2;
        // the deepest valid cut 0xdfff is v = -8191, ratio 1/8192.
        assert!((gain_word_to_db(0xd000) - 20.0 * 0.5f32.log10()).abs() < 1e-4);
        assert!(
            (gain_word_to_db(0xdfff) + 78.2678).abs() < 1e-3,
            "{}",
            gain_word_to_db(0xdfff)
        );
        assert!(gain_word_to_db(0xdfff) < gain_word_to_db(0xdffe));
        for db in [-24.0f32, -12.0, -6.0, -3.0, -0.5, 0.5, 3.0, 6.0, 12.0] {
            let back = gain_word_to_db(db_to_gain_word(db));
            assert!((back - db).abs() < 0.02, "{db} -> {back}");
        }
        // Cuts stay below unity and boosts above, and the extremes clamp.
        assert!(gain_word_to_db(db_to_gain_word(-6.0)) < 0.0);
        assert!(db_to_gain_word(-100.0) <= 0xdfff && db_to_gain_word(-100.0) >= 0xc000);
        assert_eq!(db_to_gain_word(100.0), 0x7fff);
    }

    #[test]
    fn dls2_is_a_low_pass() {
        // Cutoff 1 kHz, Q 0.7071 (damping is 1/Q: 32768 * 1.4142 = 46341).
        let spec = FilterSpec::from_dsfilter(FILTER_DLS2, 0, [hz_to_coeff(1000.0), 46341, 0, 0]);
        let fx = fx_with(spec);
        assert!(db(gain_at(&fx, 100.0)).abs() < 0.3, "pass band");
        let at_cut = db(gain_at(&fx, 1000.0));
        assert!((at_cut + 3.0).abs() < 0.6, "-3 dB at the cutoff: {at_cut}");
        let two_up = db(gain_at(&fx, 4000.0));
        assert!((two_up + 24.0).abs() < 2.5, "12 dB/oct: {two_up}");
    }

    #[test]
    fn resonance_peaks_at_the_cutoff() {
        // Q 10 (20 dB resonance): the response peaks near +20 dB.
        let spec = FilterSpec::from_dsfilter(FILTER_DLS2, 0, [hz_to_coeff(2000.0), 3277, 0, 0]);
        let peak = db(gain_at(&fx_with(spec), 2000.0));
        assert!((peak - 20.0).abs() < 1.5, "{peak}");
    }

    #[test]
    fn multi_selects_the_response() {
        let c = [hz_to_coeff(1000.0), 46341, 0, 0];
        let hp = fx_with(FilterSpec::from_dsfilter(FILTER_MULTI, 4, c));
        assert!(db(gain_at(&hp, 100.0)) < -30.0, "low blocked");
        assert!(db(gain_at(&hp, 8000.0)).abs() < 0.5, "high passed");
        let bp = fx_with(FilterSpec::from_dsfilter(FILTER_MULTI, 2, c));
        assert!(
            db(gain_at(&bp, 1000.0)).abs() < 0.5,
            "centre passes at unity"
        );
        assert!(db(gain_at(&bp, 100.0)) < -15.0 && db(gain_at(&bp, 10_000.0)) < -15.0);
        let lp = fx_with(FilterSpec::from_dsfilter(FILTER_MULTI, 1, c));
        assert!(db(gain_at(&lp, 8000.0)) < -30.0 && db(gain_at(&lp, 100.0)).abs() < 0.3);
        let notch = fx_with(FilterSpec::from_dsfilter(FILTER_MULTI, 5, c));
        assert!(db(gain_at(&notch, 1000.0)) < -20.0 && db(gain_at(&notch, 100.0)).abs() < 1.0);
        // Selector 0 and 7 bypass.
        assert!(FilterSpec::from_dsfilter(FILTER_MULTI, 0, c).is_bypass());
        assert!(FilterSpec::from_dsfilter(FILTER_MULTI, 7, c).is_bypass());
        assert!(FilterSpec::from_dsfilter(FILTER_BYPASS, 3, c).is_bypass());
    }

    #[test]
    fn parameq_boosts_and_cuts_two_bands() {
        let c = [
            hz_to_coeff(500.0),
            8192,
            hz_to_coeff(4000.0),
            db_to_gain_word(-6.02),
        ];
        let fx = fx_with(FilterSpec::from_dsfilter(FILTER_PARAMEQ, 3, c));
        assert!((db(gain_at(&fx, 500.0)) - 6.0).abs() < 0.5, "boost");
        assert!((db(gain_at(&fx, 4000.0)) + 6.0).abs() < 0.5, "cut");
        assert!(db(gain_at(&fx, 100.0)).abs() < 1.0 && db(gain_at(&fx, 15_000.0)).abs() < 1.0);
        // A zero-gain band is not a stage at all.
        assert!(FilterSpec::from_dsfilter(FILTER_PARAMEQ, 0, [0, 0, 0, 0]).is_bypass());
    }

    #[test]
    fn impulse_through_a_low_pass_decays_and_sums_to_unity() {
        let spec = FilterSpec::from_dsfilter(FILTER_DLS2, 0, [hz_to_coeff(500.0), 46341, 0, 0]);
        let fx = fx_with(spec);
        let mut buf = vec![0.0f32; 4800 * 2];
        buf[0] = 1.0;
        buf[1] = 1.0;
        FxState::default().process(&fx, &mut buf, 0, RATE);
        // DC gain 1: the impulse response sums to 1, then dies away.
        let sum: f64 = buf.iter().step_by(2).map(|&v| f64::from(v)).sum();
        assert!((sum - 1.0).abs() < 0.01, "{sum}");
        assert!(buf[2 * 4000].abs() < 1e-4);
        assert!(
            buf[0] > 0.0 && buf[0] < 0.05,
            "smoothed, not passed through"
        );
    }

    #[test]
    fn chunking_does_not_change_the_result() {
        let fx = VoiceFx {
            filter: FilterSpec::from_dsfilter(FILTER_DLS2, 0, [hz_to_coeff(800.0), 46341, 0, 0]),
            lfo: [Some(LfoSpec::from_dslfo(0, 200, 0, 32, 40)), None],
            env: Some(EnvSpec {
                start_stage: 2,
                delay: 0,
                attack: 50,
                hold: 0,
                decay: 0,
                release: 0,
                sustain: 255,
            }),
            ..Default::default()
        };
        let src: Vec<f32> = (0..2400 * 2)
            .map(|i| ((i / 2) as f32 * 0.07).sin())
            .collect();
        let mut whole = src.clone();
        FxState::default().process(&fx, &mut whole, 0, RATE);
        let mut parts = src.clone();
        let mut st = FxState::default();
        let mut c = 0u64;
        for chunk in parts.chunks_mut(2 * 97) {
            let n = chunk.len() as u64 / 2;
            st.process(&fx, chunk, c, RATE);
            c += n;
        }
        assert_eq!(whole, parts);
    }

    #[test]
    fn envelope_stages() {
        // 94 units is about one second at 93.75 units/s.
        let e = EnvSpec {
            start_stage: 1,
            delay: 94,
            attack: 94,
            hold: 94,
            decay: 94,
            release: 94,
            sustain: 51,
        };
        let one = 94.0 / EG_UNITS_PER_SECOND;
        assert_eq!(e.gain(0.0), 0.0);
        assert_eq!(e.gain(0.5 * one), 0.0, "delay");
        assert!((e.gain(1.5 * one) - 0.5).abs() < 1e-9, "mid attack");
        assert_eq!(e.gain(2.5 * one), 1.0, "hold");
        assert!((e.gain(3.5 * one) - 0.6).abs() < 1e-9, "decay to 0.2");
        assert!((e.gain(10.0 * one) - 0.2).abs() < 1e-9, "sustain");
        // Starting at attack skips the delay.
        let a = EnvSpec {
            start_stage: 2,
            ..e
        };
        assert!((a.gain(0.5 * one) - 0.5).abs() < 1e-9);
        // Zero-length stages fall through.
        let fade = EnvSpec {
            start_stage: 2,
            delay: 0,
            attack: 0,
            hold: 0,
            decay: 0,
            release: 0,
            sustain: 255,
        };
        assert_eq!(fade.gain(0.0), 1.0);
    }

    #[test]
    fn fade_in_scales_the_signal() {
        // Halo's fade shape: attack only, sustain 0xff.
        let fx = VoiceFx {
            env: Some(EnvSpec {
                start_stage: 2,
                delay: 0,
                attack: 94,
                hold: 0,
                decay: 0,
                release: 0,
                sustain: 255,
            }),
            ..Default::default()
        };
        let mut buf = vec![1.0f32; RATE as usize * 2 * 2];
        FxState::default().process(&fx, &mut buf, 0, RATE);
        let one = 94.0 / EG_UNITS_PER_SECOND;
        let at = |s: f64| buf[(s * f64::from(RATE)) as usize * 2];
        assert!(at(0.0) < 0.01);
        assert!((f64::from(at(0.5 * one)) - 0.5).abs() < 0.01);
        assert_eq!(at(one + 0.1), 1.0);
    }

    #[test]
    fn tremolo_and_vibrato_lfo() {
        // 4.0 Hz-ish rate, 8 dB tremolo.
        let l = LfoSpec::from_dslfo(0, 175, 0, 0, 128);
        assert!((l.rate_hz - 4.0).abs() < 0.01, "{}", l.rate_hz);
        assert!((l.amp_db - 8.0).abs() < 1e-6);
        let fx = VoiceFx {
            lfo: [Some(l), None],
            ..Default::default()
        };
        let mut buf = vec![1.0f32; RATE as usize * 2];
        FxState::default().process(&fx, &mut buf, 0, RATE);
        let quarter = (0.25 / f64::from(l.rate_hz) * f64::from(RATE)) as usize;
        // Quarter cycle: triangle at +1 -> +8 dB; three quarters: -8 dB.
        assert!((db(f64::from(buf[quarter * 2])) - 8.0).abs() < 0.1);
        assert!((db(f64::from(buf[3 * quarter * 2])) + 8.0).abs() < 0.2);
        // The delay holds the LFO at 0 first.
        let delayed = LfoSpec::from_dslfo(1500, 175, 0, 0, 128);
        assert_eq!(delayed.value(0.5), 0.0);
        assert!(delayed.value(1.0 + 0.25 / f64::from(delayed.rate_hz)) > 0.99);
        // Pitch LFO: +64 units of 1/128 oct at the peak is half an octave.
        let p = VoiceFx {
            lfo: [None, Some(LfoSpec::from_dslfo(0, 175, 64, 0, 0))],
            ..Default::default()
        };
        let mut st = FxState::default();
        assert_eq!(st.pitch_mult(&p, RATE), 1.0);
        st.clock = (0.25 / f64::from(p.lfo[1].unwrap().rate_hz) * f64::from(RATE)) as u64;
        assert!((st.pitch_mult(&p, RATE) - 0.5f64.exp2()).abs() < 1e-3);
        st.doppler = 2.0;
        assert!((st.pitch_mult(&p, RATE) - 2.0 * 0.5f64.exp2()).abs() < 2e-3);
    }

    #[test]
    fn filter_lfo_sweeps_the_cutoff() {
        // A wah: low pass at 1 kHz swept +-2 octaves, a 1 kHz tone passes
        // loudest when the cutoff is highest.
        let fx = VoiceFx {
            filter: FilterSpec::from_dsfilter(FILTER_DLS2, 0, [hz_to_coeff(1000.0), 46341, 0, 0]),
            lfo: [Some(LfoSpec::from_dslfo(0, 175, 0, 32, 0)), None],
            ..Default::default()
        };
        assert!((fx.lfo[0].unwrap().filter_oct - 2.0).abs() < 1e-6);
        let mut buf: Vec<f32> = (0..RATE as usize)
            .flat_map(|i| {
                let v = (2.0 * PI * 3000.0 * i as f64 / f64::from(RATE)).sin() as f32;
                [v, v]
            })
            .collect();
        FxState::default().process(&fx, &mut buf, 0, RATE);
        let rms = |from: usize| {
            let s: f64 = buf.as_chunks::<2>().0[from..from + 480]
                .iter()
                .map(|f| f64::from(f[0]).powi(2))
                .sum();
            (s / 480.0).sqrt()
        };
        let quarter = (RATE as f64 / 16.0) as usize; // cutoff 4 kHz
        let three = 3 * quarter; // cutoff 250 Hz
        assert!(
            rms(quarter - 240) > 4.0 * rms(three - 240),
            "{} vs {}",
            rms(quarter - 240),
            rms(three - 240)
        );
    }

    #[test]
    fn i3dl2_direct_path_obstruction_rolls_off_highs() {
        // Obstruction of -20 dB at HF with the low frequencies untouched.
        let src = I3dl2Source {
            obstruction_hf: -2000,
            obstruction_lf_ratio: 0.0,
            ..Default::default()
        };
        assert_eq!(src.direct_gain(), 1.0);
        assert!((src.direct_hf_mb() + 2000.0).abs() < 1e-6);
        let fx = VoiceFx {
            i3dl2: Some(src),
            ..Default::default()
        };
        assert!(db(direct_at(&fx, 50.0)).abs() < 0.5);
        let hf = db(direct_at(&fx, 5000.0));
        assert!(
            (hf + 20.0).abs() < 1.5,
            "-20 dB at the 5 kHz reference: {hf}"
        );
        assert!(db(direct_at(&fx, 15_000.0)) < hf);
        // The shelf is the direct path only: common processing leaves the room
        // send untouched.
        assert!(db(gain_at(&fx, 5000.0)).abs() < 1e-6);
        // Occlusion also lowers the low frequencies by its ratio and the room send.
        let occ = I3dl2Source {
            occlusion_hf: -2000,
            occlusion_lf_ratio: 0.25,
            ..Default::default()
        };
        assert!((occ.direct_lf_mb() + 500.0).abs() < 1e-3);
        assert!((db(f64::from(occ.room_gain())) + 5.0).abs() < 0.01);
        assert!(!occ.is_open() && I3dl2Source::default().is_open());
    }

    #[test]
    fn voice_headroom_attenuates() {
        assert_eq!(headroom_gain(0), 1.0, "Halo's setting: no change");
        assert!((db(f64::from(headroom_gain(600))) + 6.0).abs() < 0.01);
        assert_eq!(headroom_gain(9999), headroom_gain(6000), "clamped");
        assert_eq!(headroom_gain(-5), 1.0);
        assert_eq!(VoiceFx::default().headroom_mb, 0);
    }

    #[test]
    fn bypass_leaves_the_signal_alone() {
        let fx = VoiceFx::default();
        assert!(!fx.processes());
        let src: Vec<f32> = (0..64).map(|i| i as f32 * 0.01).collect();
        let mut b = src.clone();
        FxState::default().process(&fx, &mut b, 0, RATE);
        assert_eq!(b, src);
    }

    #[test]
    fn recursive_state_never_goes_subnormal() {
        let fx = VoiceFx {
            filter: FilterSpec::from_dsfilter(FILTER_DLS2, 0, [hz_to_coeff(1000.0), 46341, 0, 0]),
            i3dl2: Some(I3dl2Source {
                obstruction_hf: -2000,
                ..Default::default()
            }),
            ..Default::default()
        };
        let n = RATE as usize * 2;
        let mut buf = vec![0.0f32; n * 2];
        buf[0] = 1.0;
        buf[1] = 1.0;
        let mut st = FxState::default();
        st.process(&fx, &mut buf, 0, RATE);
        let mut direct = vec![0.0f32; n * 2];
        assert!(st.direct_shelf(&fx, &buf, &mut direct, RATE));
        for v in buf.iter().chain(&direct) {
            assert!(*v == 0.0 || v.abs() >= f32::MIN_POSITIVE, "subnormal {v:e}");
        }
        let normal = |v: f64| v == 0.0 || v.abs() >= f64::MIN_POSITIVE;
        assert!(
            st.bq
                .iter()
                .all(|b| b.z1.iter().chain(&b.z2).all(|&v| normal(v)))
        );
        assert!(st.shelf_z.iter().all(|&v| normal(v)));
    }

    #[test]
    fn i3dl2_source_descriptors_are_sanitised() {
        let nan = f32::NAN.to_bits();
        let inf = f32::INFINITY.to_bits();
        let src = I3dl2Source::from_words([
            i32::MAX as u32,
            i32::MIN as u32,
            i32::MAX as u32,
            5,
            inf,
            i32::MIN as u32,
            nan,
            77,
            nan,
        ]);
        for v in [src.direct_gain(), src.room_gain()] {
            assert!(v.is_finite() && v > 0.0, "{v}");
        }
        assert!(src.direct_lf_mb().is_finite() && src.direct_hf_mb().is_finite());
        assert_eq!((src.direct, src.room), (1000, 1000));
        assert_eq!(src.obstruction_lf_ratio, 0.0);
        assert_eq!(src.occlusion_lf_ratio, 0.25);
        assert_eq!(src.room_rolloff, 10.0);
        assert!(src.shelf_db().is_finite());
        // A NaN ratio no longer poisons the gain even with zero attenuation.
        let z = I3dl2Source::from_words([0, 0, 0, 0, 0, 0, nan, 0, nan]);
        assert_eq!(z.direct_gain(), 1.0);
    }

    #[test]
    fn pitch_multiplier_is_always_finite_and_positive() {
        let fx = VoiceFx {
            lfo: [None, Some(LfoSpec::from_dslfo(0, 175, 127, 0, 0))],
            ..Default::default()
        };
        for d in [f64::NAN, f64::INFINITY, 0.0, -1.0, 1e300, 1e-300] {
            let st = FxState {
                doppler: d,
                ..Default::default()
            };
            let m = st.pitch_mult(&fx, RATE);
            assert!(m.is_finite() && m > 0.0, "{d}: {m}");
            let m = st.pitch_mult(&VoiceFx::default(), RATE);
            assert!(m.is_finite() && m > 0.0, "{d}: {m}");
        }
    }

    #[test]
    fn a_tiny_host_rate_does_not_panic() {
        let fx = fx_with(FilterSpec::from_dsfilter(
            FILTER_DLS2,
            0,
            [hz_to_coeff(1000.0), 46341, 0, 0],
        ));
        let mut buf = vec![0.5f32; 16];
        FxState::default().process(&fx, &mut buf, 0, 8);
        assert!(buf.iter().all(|v| v.is_finite()));
    }
}
