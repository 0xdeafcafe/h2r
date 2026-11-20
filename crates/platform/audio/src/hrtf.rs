//! Parametric binaural rendering for the dry stereo fold of a 3D mix-in.

use std::f32::consts::PI;

use crate::dsound::SPEED_OF_SOUND;

const HEAD_RADIUS_METRES: f32 = 0.0875;
const MAX_ITD_SECONDS: f32 = HEAD_RADIUS_METRES / SPEED_OF_SOUND * (1.0 + PI / 2.0);
const PINNA_HZ: f32 = 6500.0;
const PINNA_SPAN_HZ: f32 = 2500.0;
const RAMP_MS: u32 = 10;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Parameters {
    pub(crate) delay: [f32; 2],
    pub(crate) shadow: [f32; 2],
    pub(crate) rear: f32,
    pub(crate) front_weight: f32,
    pub(crate) pinna_delay: f32,
    pub(crate) pinna_mix: f32,
}

impl Parameters {
    /// Brown & Duda (1998), eq. 5, with incidence measured from the
    /// ear's outward axis and the minimum shadow at 150 degrees.
    fn alpha(incidence: f32) -> f32 {
        1.05 + 0.95 * (1.2 * incidence).cos()
    }

    fn for_direction(rate: f32, [right, up, front]: [f32; 3]) -> Self {
        let broadside = Self::alpha(PI / 2.0);
        // Woodworth's difference between the near and far ear arrival times,
        // using interaural azimuth. The near ear reads without added delay.
        let itd = HEAD_RADIUS_METRES / SPEED_OF_SOUND * (right.asin() + right) * rate;
        let notch_hz = (PINNA_HZ + PINNA_SPAN_HZ * up).min(rate * 0.4);
        Self {
            delay: [itd.max(0.0), (-itd).max(0.0)],
            shadow: [-right, right].map(|ear_cos| Self::alpha(ear_cos.acos()) / broadside - 1.0),
            rear: 0.35 * (-front).max(0.0),
            front_weight: (1.0 + front) * 0.5,
            pinna_delay: 0.5 * rate / notch_hz,
            pinna_mix: 0.3 * up.abs(),
        }
    }

    fn approach(&mut self, target: Self, step: f32) {
        let ramp = |value: &mut f32, target: f32| *value += (target - *value) * step;
        for ear in 0..2 {
            ramp(&mut self.delay[ear], target.delay[ear]);
            ramp(&mut self.shadow[ear], target.shadow[ear]);
        }
        ramp(&mut self.rear, target.rear);
        ramp(&mut self.front_weight, target.front_weight);
        ramp(&mut self.pinna_delay, target.pinna_delay);
        ramp(&mut self.pinna_mix, target.pinna_mix);
    }
}

/// One mono delay line serves both ears' Woodworth and pinna taps.
/// The rear shelf has common state, and each ear has its own head-shadow pole.
#[derive(Debug)]
pub(crate) struct Hrtf {
    pub(crate) history: Vec<f32>,
    pub(crate) write: usize,
    pub(crate) rear_low: f32,
    pub(crate) ear_low: [f32; 2],
    pub(crate) current: Option<Parameters>,
    pub(crate) target: Option<Parameters>,
    pub(crate) remaining: u32,
    pub(crate) rate: f32,
    pub(crate) shadow_coefficient: f32,
    pub(crate) rear_coefficient: f32,
    pub(crate) ramp_frames: u32,
}

impl Hrtf {
    pub(crate) fn history_len(rate: u32) -> usize {
        let rate = rate as f32;
        let pinna_max = (0.5 * rate / (PINNA_HZ - PINNA_SPAN_HZ)).max(1.25);
        (MAX_ITD_SECONDS * rate + pinna_max).ceil() as usize + 2
    }

    pub(crate) fn new(rate: u32) -> Self {
        Self::with_history(rate, vec![0.0; Self::history_len(rate)])
    }

    pub(crate) fn with_history(rate: u32, history: Vec<f32>) -> Self {
        let rate = rate as f32;
        // Divide Brown & Duda's transfer function by its broadside response.
        // The resulting fixed pole and directional zero give unity at DC and
        // make a horizontal, straight-ahead source exactly neutral.
        let shadow_k = SPEED_OF_SOUND / (HEAD_RADIUS_METRES * Parameters::alpha(PI / 2.0) * rate);
        let rear_k = (PI * (2500.0 / rate).min(0.2)).tan();
        Self {
            history,
            write: 0,
            rear_low: 0.0,
            ear_low: [0.0; 2],
            current: None,
            target: None,
            remaining: 0,
            rate,
            shadow_coefficient: shadow_k / (1.0 + shadow_k),
            rear_coefficient: rear_k / (1.0 + rear_k),
            ramp_frames: (rate * RAMP_MS as f32 / 1000.0).round().max(1.0) as u32,
        }
    }

    pub(crate) fn reset(&mut self) {
        if self.current.take().is_some() {
            self.history.fill(0.0);
            self.write = 0;
            self.rear_low = 0.0;
            self.ear_low = [0.0; 2];
            self.target = None;
            self.remaining = 0;
        }
    }

    /// Filters a mono mix-in (duplicated into stereo by the mixer) and adds
    /// it to the main output using the original front/back gains for each
    /// ear. The directional front/back split ramps along with the filters.
    pub(crate) fn mix(
        &mut self,
        src: &[f32],
        out: &mut [f32],
        direction: [f32; 3],
        [front_gains, back_gains]: [[f32; 2]; 2],
    ) {
        let target = Parameters::for_direction(self.rate, direction);
        let mut current = self.current.unwrap_or(target);
        if self.target != Some(target) {
            self.target = Some(target);
            self.remaining = if self.current.is_some() {
                self.ramp_frames
            } else {
                0
            };
        }
        for (src, out) in src
            .as_chunks::<2>()
            .0
            .iter()
            .zip(out.as_chunks_mut::<2>().0)
        {
            if self.remaining > 1 {
                current.approach(target, 1.0 / self.remaining as f32);
                self.remaining -= 1;
            } else {
                current = target;
                self.remaining = 0;
            }
            let x = src[0];
            let low = low_pass(&mut self.rear_low, x, self.rear_coefficient);
            self.history[self.write] = x + current.rear * (low - x);
            for (ear, dst) in out.iter_mut().enumerate() {
                let delay = current.delay[ear];
                let direct = self.read(delay);
                let reflection = self.read(delay + current.pinna_delay);
                // Blend with one positive reflection for a unity-DC comb.
                let x = direct + current.pinna_mix * (reflection - direct);
                let low = low_pass(&mut self.ear_low[ear], x, self.shadow_coefficient);
                let gain =
                    back_gains[ear] + current.front_weight * (front_gains[ear] - back_gains[ear]);
                if gain != 0.0 {
                    *dst += (x + current.shadow[ear] * (x - low)) * gain;
                }
            }
            self.write += 1;
            if self.write == self.history.len() {
                self.write = 0;
            }
        }
        self.current = Some(current);
    }

    fn read(&self, delay: f32) -> f32 {
        let frames = delay as usize;
        let fraction = delay - frames as f32;
        let i = if self.write >= frames {
            self.write - frames
        } else {
            self.write + self.history.len() - frames
        };
        let older = if i == 0 {
            self.history.len() - 1
        } else {
            i - 1
        };
        let x = self.history[i];
        x + fraction * (self.history[older] - x)
    }
}

/// Bilinear one-pole low-pass, used with a parallel dry path to make a shelf.
fn low_pass(state: &mut f32, x: f32, coefficient: f32) -> f32 {
    let v = coefficient * (x - *state);
    let low = *state + v;
    let next = low + v;
    *state = if next.abs() < 1e-30 { 0.0 } else { next };
    low
}
