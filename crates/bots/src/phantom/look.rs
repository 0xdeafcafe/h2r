//! On-foot right-stick response before target assist, at the remote gather.
//! The local input pass samples the profile and map tables described in
//! docs/game/bots.md §5.1. Keep the same inputs for a virtual remote pad.

use crate::guest::BOT_SENSITIVITY;
use engine::memory::{GuestMemory, MemFault};
use input::Gamepad;

#[derive(Clone, Copy, Default)]
pub(super) struct Look {
    pegged_seconds: [f32; 2],
}

// The XInput reader removes 9,000 counts from each axis independently, then
// expands each surviving half-axis to its signed 16-bit range (0x124dfe).
fn axis(count: i16) -> f32 {
    let n = i32::from(count);
    let limit = if n < 0 { 32768 } else { 32767 };
    let scaled = (n.abs() - 9000).max(0) * limit / (limit - 9000);
    (n.signum() * scaled) as f32 / 32767.0
}

fn square_axes(mut x: f32, mut y: f32, threshold: f32) -> [f32; 2] {
    let peak = x.abs().max(y.abs());
    if x.abs().min(y.abs()) > threshold {
        let scale = x.hypot(y) / peak;
        x *= scale;
        y *= scale;
    }
    [x.clamp(-1.0, 1.0), y.clamp(-1.0, 1.0)]
}

impl Look {
    pub(super) fn delta(
        &mut self,
        m: &dyn GuestMemory,
        pad: &Gamepad,
        dt: f32,
    ) -> Result<[f32; 2], MemFault> {
        // The Default layout expands a diagonal at 0x217dab and again at
        // 0x185ee3 when both gameplay axes exceed 0.1.
        let [x, y] = square_axes(axis(pad.thumb_rx), axis(pad.thumb_ry), 0.0);
        let axes = square_axes(x, y, 0.1);
        if axes == [0.; 2] {
            self.pegged_seconds = [0.; 2];
            return Ok([0.; 2]);
        }
        let control = m.u32(m.u32(0x004e_034c)? + 0xf4)?;
        let count = m.u16(control + 0x74)? as i16;
        let curve = m.u32(control + 0x78)?;
        if count <= 0 || curve == 0 {
            self.pegged_seconds = [0.; 2];
            return Ok([0.; 2]);
        }
        let last = (count - 1) as u32;
        let peg = m.f32(control + 0x48)?;
        let sensitivity = 4 * u32::from(BOT_SENSITIVITY - 1);
        let mut delta = [0.; 2];
        for (i, (stick, rates, ramp)) in
            [(axes[0], 0x0044_4c04, 0x4c), (axes[1], 0x0044_4bdc, 0x54)]
                .into_iter()
                .enumerate()
        {
            // 0x185b30 interpolates adjacent samples of the look function.
            let pos = stick.abs() * last as f32;
            let lo = pos as u32;
            let a = m.f32(curve.wrapping_add(4 * lo))?;
            let b = m.f32(curve.wrapping_add(4 * (lo + 1).min(last)))?;
            let response = a + (b - a) * pos.fract();
            let mut speed = m.f32(rates + sensitivity)?.to_radians();
            if stick.abs() >= peg {
                let duration = m.f32(control + ramp)?;
                let fraction = if duration > 0.0 {
                    (self.pegged_seconds[i] / duration).min(1.0)
                } else {
                    1.0
                };
                speed *= 1.0 + fraction * (m.f32(control + ramp + 4)? - 1.0);
                self.pegged_seconds[i] += dt;
            } else {
                self.pegged_seconds[i] = 0.0;
            }
            let signed = if stick < 0.0 { -response } else { response };
            delta[i] = signed * speed * dt;
        }
        Ok(delta)
    }
}
