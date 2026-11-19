//! A brain requests controls. This is the controller it actually operates.

use std::collections::VecDeque;

use input::host::{HostButton, HostButtons, HostPadState};

use crate::mind::feel::{Rng, reached};
use crate::tuning::InputLimits;

pub(crate) const TICK_MS: f32 = 1000.0 / 30.0;
const BUTTONS: usize = HostButton::ALL.len();
const CONTROLS: usize = BUTTONS + 2;
const RIGHT_THUMB_BUTTONS: u16 = (1 << HostButton::South as u8)
    | (1 << HostButton::East as u8)
    | (1 << HostButton::West as u8)
    | (1 << HostButton::North as u8)
    | (1 << HostButton::LeftBumper as u8)
    | (1 << HostButton::RightBumper as u8);
const RIGHT_CLICK: u16 = 1 << HostButton::RightStick as u8;

/// Stochastic rounding shares a sub-tick timing error between the adjacent
/// report boundaries. A 6 ms error crosses a boundary about 18% of the time.
fn tick_slip(rng: &mut Rng, sigma_ms: f32) -> i32 {
    if sigma_ms <= 0.0 {
        return 0;
    }
    let radius = (-2.0 * (1.0 - rng.unit()).ln()).sqrt();
    let angle = std::f32::consts::TAU * rng.unit();
    let error = radius * angle.cos() * sigma_ms;
    if rng.unit() * TICK_MS < error.abs() {
        if error < 0.0 { -1 } else { 1 }
    } else {
        0
    }
}

#[derive(Clone, Copy)]
struct Edge {
    due: u32,
    control: usize,
    value: f32,
}

pub(crate) struct Controller {
    rng: Rng,
    sticks: VecDeque<(u32, HostPadState)>,
    edges: Vec<Edge>,
    requested: [f32; CONTROLS],
    applied: [f32; CONTROLS],
    last_due: [Option<u32>; CONTROLS],
    wanted: HostPadState,
    report: HostPadState,
    last_look: Option<u32>,
    last_face: Option<u32>,
}

impl Controller {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0x7061_645f_7469_6d65),
            sticks: VecDeque::new(),
            edges: Vec::new(),
            requested: [0.0; CONTROLS],
            applied: [0.0; CONTROLS],
            last_due: [None; CONTROLS],
            wanted: HostPadState::default(),
            report: HostPadState::default(),
            last_look: None,
            last_face: None,
        }
    }

    /// A release on death discards reports for the old biped.
    pub(crate) fn discard_pending(&mut self) {
        self.sticks.clear();
        self.edges.clear();
        self.requested.fill(0.0);
        self.applied.fill(0.0);
        self.last_due.fill(None);
        self.wanted = HostPadState::default();
    }

    /// Clear the pad at a stream boundary while keeping its own timing RNG.
    pub(crate) fn reset(&mut self) {
        self.discard_pending();
        self.report = HostPadState::default();
        self.last_look = None;
        self.last_face = None;
    }

    pub(crate) fn step(
        &mut self,
        tick: u32,
        intent: HostPadState,
        limits: InputLimits,
    ) -> HostPadState {
        let due = tick.wrapping_add(limits.motor_ticks);
        self.sticks.push_back((due, intent));
        let mut report = [0.0; CONTROLS];
        for button in HostButton::ALL {
            report[button as usize] = u8::from(intent.buttons.contains(button)).into();
        }
        report[BUTTONS] = axis(intent.left_trigger, 0.0);
        report[BUTTONS + 1] = axis(intent.right_trigger, 0.0);
        for (control, value) in report.into_iter().enumerate() {
            if value == self.requested[control] {
                continue;
            }
            self.requested[control] = value;
            let slip = tick_slip(&mut self.rng, limits.button_sigma_ms);
            let delay = limits.motor_ticks.saturating_add_signed(slip);
            let candidate = tick.wrapping_add(delay);
            // A release can't overtake its press. If both land on the
            // same boundary the action sampler can miss the short tap.
            let due = self.last_due[control]
                .filter(|&last| !reached(candidate, last))
                .unwrap_or(candidate);
            self.last_due[control] = Some(due);
            self.edges.push(Edge {
                due,
                control,
                value,
            });
        }

        while self
            .sticks
            .front()
            .is_some_and(|&(due, _)| reached(tick, due))
        {
            self.wanted = self.sticks.pop_front().unwrap().1;
        }
        self.edges.retain(|edge| {
            let due = reached(tick, edge.due);
            if due {
                self.applied[edge.control] = edge.value;
                if self.last_due[edge.control] == Some(edge.due) {
                    self.last_due[edge.control] = None;
                }
            }
            !due
        });
        let mut pad = self.wanted;
        for button in HostButton::ALL {
            pad.buttons.set(button, self.applied[button as usize] > 0.0);
        }
        pad.left_trigger = self.applied[BUTTONS];
        pad.right_trigger = self.applied[BUTTONS + 1];
        self.limit_pad(tick, pad, limits)
    }

    pub(crate) fn report(&self) -> HostPadState {
        self.report
    }

    fn limit_pad(&mut self, tick: u32, mut pad: HostPadState, limits: InputLimits) -> HostPadState {
        let mut buttons = pad.buttons.bits();
        // One right thumb can select one face key per report. Keep holding
        // it when another key wants the same thumb (e.g. a jump over reload).
        let face = buttons & RIGHT_THUMB_BUTTONS;
        let held = face & self.report.buttons.bits();
        let choice = if held != 0 { held } else { face };
        let face = choice & choice.wrapping_neg();
        buttons &= !RIGHT_THUMB_BUTTONS;
        let free = |last: Option<u32>| {
            last.is_none_or(|last| tick.wrapping_sub(last) >= limits.thumb_switch_ticks)
        };
        let looking = face == 0 && free(self.last_face);
        let right = if looking { pad.right_stick } else { (0.0, 0.0) };
        pad.left_stick = slew(self.report.left_stick, pad.left_stick, limits.stick_step);
        pad.right_stick = slew(self.report.right_stick, right, limits.stick_step);
        if face != 0 && pad.right_stick == (0.0, 0.0) && free(self.last_look) {
            buttons |= face;
            self.last_face = Some(tick);
        }
        if !looking {
            buttons &= !RIGHT_CLICK;
        }
        if pad.right_stick != (0.0, 0.0) || buttons & RIGHT_CLICK != 0 {
            self.last_look = Some(tick);
        }
        // The brain uses digital button pressure and no touchpad.
        pad.buttons = HostButtons::from_bits(buttons);
        pad.analog_buttons = None;
        pad.touchpad = None;
        self.report = pad;
        pad
    }
}

fn axis(v: f32, min: f32) -> f32 {
    if v.is_finite() {
        v.clamp(min, 1.0)
    } else {
        0.0
    }
}

fn slew(current: (f32, f32), desired: (f32, f32), step: f32) -> (f32, f32) {
    let mut target = (axis(desired.0, -1.0), axis(desired.1, -1.0));
    let radius = target.0.hypot(target.1);
    if radius > 1.0 {
        target = (target.0 / radius, target.1 / radius);
    }
    let dx = target.0 - current.0;
    let dy = target.1 - current.1;
    let distance = dx.hypot(dy);
    if distance <= step {
        target
    } else {
        let ratio = step / distance;
        (current.0 + ratio * dx, current.1 + ratio * dy)
    }
}

#[cfg(test)]
mod tests;
