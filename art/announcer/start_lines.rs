//! Original spoken game-type lines for the announcer. The scores below
//! move three vowel resonances, pitch and frication over time.

use std::f64::consts::{PI, TAU};

const RATE: u32 = 44_100;

#[derive(Clone, Copy)]
struct Phone {
    formants: [f64; 3],
    voice: f64,
    // Centre frequency, bandwidth (Hz) and level of the consonant noise.
    hiss: [f64; 3],
}

const fn vowel(formants: [f64; 3]) -> Phone {
    Phone { formants, voice: 1.0, hiss: [3000.0, 1800.0, 0.0] }
}

const AH: Phone = vowel([740.0, 1260.0, 2450.0]);
const IH: Phone = vowel([380.0, 1950.0, 2600.0]);
const EE: Phone = vowel([275.0, 2190.0, 2980.0]);
const ER: Phone = vowel([425.0, 1250.0, 1610.0]);
const F: Phone = Phone { formants: AH.formants, voice: 0.0, hiss: [2400.0, 2900.0, 1.25] };
const S: Phone = Phone { formants: EE.formants, voice: 0.0, hiss: [6550.0, 2400.0, 2.3] };
const T: Phone = Phone { formants: IH.formants, voice: 0.0, hiss: [4650.0, 3200.0, 2.0] };
const ZH: Phone = Phone { formants: [310.0, 1680.0, 2480.0], voice: 0.26, hiss: [3100.0, 1600.0, 2.5] };
const CLOSED: Phone = Phone { formants: [220.0, 1000.0, 2200.0], voice: 0.09, hiss: [3000.0, 1800.0, 0.0] };
const QUIET: Phone = Phone { voice: 0.0, ..CLOSED };

// Each step gives the milliseconds to reach the named sound and pitch.
// Fire has a falling rhotic tail. Fight closes the second diphthong with
// a brief silence and a released T.
const FIREFIGHT: &[(u16, Phone, f64)] = &[
    (18, F, 110.0), (80, F, 110.0), (24, AH, 132.0), (58, AH, 136.0),
    (104, IH, 119.0), (83, ER, 105.0), (39, ER, 101.0),
    (20, F, 105.0), (82, F, 111.0), (23, AH, 128.0), (71, AH, 122.0),
    (104, IH, 92.0), (18, QUIET, 88.0), (35, QUIET, 88.0),
    (4, T, 88.0), (35, QUIET, 88.0), (35, QUIET, 88.0),
];

// Siege holds its front vowel, closes briefly on D and releases into
// voiced postalveolar frication.
const SIEGE: &[(u16, Phone, f64)] = &[
    (20, S, 110.0), (115, S, 115.0), (29, EE, 132.0), (116, EE, 136.0),
    (136, EE, 110.0), (22, CLOSED, 98.0), (42, CLOSED, 93.0),
    (7, ZH, 92.0), (74, ZH, 85.0), (34, QUIET, 83.0), (35, QUIET, 83.0),
];

/// A damped band-pass resonator with a zero at DC. The difference input
/// and the pole radius give approximately unit gain at its centre.
#[derive(Clone, Copy, Default)]
struct Resonator {
    y1: f64,
    y2: f64,
}

impl Resonator {
    fn step(&mut self, difference: f64, hz: f64, radius: f64) -> f64 {
        let y = (1.0 - radius) * difference
            + 2.0 * radius * (TAU * hz / f64::from(RATE)).cos() * self.y1
            - radius * radius * self.y2;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }
}

fn radius(width: f64) -> f64 {
    (-PI * width / f64::from(RATE)).exp()
}

fn samples(score: &[(u16, Phone, f64)]) -> Vec<f64> {
    let mut output = Vec::new();
    let mut mouth = [Resonator::default(); 3];
    let widths = [radius(82.0), radius(102.0), radius(145.0)];
    let mut hiss = Resonator::default();
    let mut seed = 0x47ac_912d_u32;
    let mut phase = 0.0;
    let mut glottis = 0.0;
    let mut pulses = [0.0; 2];
    let mut noise = [0.0; 2];
    let (mut from, mut pitch) = (QUIET, score[0].2);
    for &(ms, to, end_pitch) in score {
        let count = (u32::from(ms) * RATE + 500) / 1000;
        for i in 0..count {
            let t = f64::from(i + 1) / f64::from(count);
            let lerp = |a: f64, b: f64| a + t * (b - a);
            let step = lerp(pitch, end_pitch) / f64::from(RATE);
            phase += step;
            let pulse = if phase >= 1.0 {
                phase -= 1.0;
                1.0 / step
            } else {
                0.0
            };
            // A softened pulse train carries the vowel harmonics.
            glottis += 0.22 * (pulse - glottis);
            let drive = glottis - pulses[1];
            pulses = [glottis, pulses[0]];
            let mut voiced = 0.0;
            for (j, level) in [1.0, 0.9, 0.58].into_iter().enumerate() {
                let hz = lerp(from.formants[j], to.formants[j]);
                voiced += level * mouth[j].step(drive, hz, widths[j]);
            }

            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let white = f64::from(seed) * (2.0 / f64::from(u32::MAX)) - 1.0;
            let band = hiss.step(
                white - noise[1],
                lerp(from.hiss[0], to.hiss[0]),
                radius(lerp(from.hiss[1], to.hiss[1])),
            );
            noise = [white, noise[0]];
            output.push(lerp(from.voice, to.voice) * voiced + lerp(from.hiss[2], to.hiss[2]) * band);
        }
        (from, pitch) = (to, end_pitch);
    }
    output
}

/// Build a mono, 44.1 kHz, signed 16-bit little-endian PCM WAV.
pub fn wav(id: &str) -> Option<Vec<u8>> {
    let samples = samples(match id {
        "firefight" => FIREFIGHT,
        "siege" => SIEGE,
        _ => return None,
    });
    let peak = samples.iter().map(|s| s.abs()).fold(0.0_f64, f64::max);
    let gain = 0.88 * f64::from(i16::MAX) / peak;
    let len = 2 * samples.len() as u32;
    let mut out = Vec::with_capacity(44 + len as usize);
    out.extend(b"RIFF");
    out.extend((36 + len).to_le_bytes());
    out.extend(b"WAVEfmt ");
    out.extend(16_u32.to_le_bytes());
    out.extend(1_u16.to_le_bytes()); // PCM
    out.extend(1_u16.to_le_bytes()); // mono
    out.extend(RATE.to_le_bytes());
    out.extend((2 * RATE).to_le_bytes());
    out.extend(2_u16.to_le_bytes()); // bytes per frame
    out.extend(16_u16.to_le_bytes());
    out.extend(b"data");
    out.extend(len.to_le_bytes());
    for sample in samples {
        out.extend(((sample * gain).round() as i16).to_le_bytes());
    }
    Some(out)
}
