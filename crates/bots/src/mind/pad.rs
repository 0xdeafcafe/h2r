//! Controller timing after a decision to move, reload, switch, or melee.

use input::host::{HostButton, HostPadState};

use super::feel::{Rng, limit, reached};
use super::{MELEE, RELOAD, RELOAD_COOLDOWN, SWITCH, SWITCH_COOLDOWN, Skill};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tap {
    Reload,
    Switch,
    Melee(u32),
}

impl Tap {
    fn input(self) -> (usize, HostButton, u32) {
        match self {
            Self::Reload => (0, RELOAD, RELOAD_COOLDOWN),
            Self::Switch => (1, SWITCH, SWITCH_COOLDOWN),
            Self::Melee(_) => (2, MELEE, 20),
        }
    }

    fn delay(self, skill: Skill, rng: &mut Rng) -> u32 {
        let base = match skill {
            Skill::Easy => 9,
            Skill::Normal => 7,
            Skill::Heroic => 6,
            Skill::Legendary => 5,
        };
        match self {
            // The enemy reaction has already passed before a melee is
            // requested. Finding B still takes a few ticks.
            Self::Melee(_) => rng.ticks(base - 3, base),
            _ => base + rng.ticks(0, 4),
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct Buttons {
    rng: Rng,
    request: Option<(Tap, u32)>,
    down: Option<(Tap, u32)>,
    free: Option<u32>,
    ready: [Option<u32>; 3],
    jump_request: bool,
    jump_until: Option<u32>,
}

impl Buttons {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0x6275_7474_6f6e),
            request: None,
            down: None,
            free: None,
            ready: [None; 3],
            jump_request: false,
            jump_until: None,
        }
    }

    pub(super) fn release(&mut self) {
        self.request = None;
        self.down = None;
        self.free = None;
        self.ready = [None; 3];
        self.jump_request = false;
        self.jump_until = None;
    }

    pub(super) fn step(
        &mut self,
        request: Option<Tap>,
        tick: u32,
        skill: Skill,
        weapon_allowed: bool,
    ) -> Option<HostButton> {
        if let Some((tap, until)) = self.down {
            if (weapon_allowed || matches!(tap, Tap::Melee(_))) && !reached(tick, until) {
                return Some(tap.input().1);
            }
            self.down = None;
        }
        let Some(tap) = request.filter(|tap| weapon_allowed || matches!(tap, Tap::Melee(_))) else {
            self.request = None;
            return None;
        };
        let (slot, button, cooldown) = tap.input();
        if self.request.is_none_or(|(prior, _)| prior != tap) {
            let due = tick.wrapping_add(tap.delay(skill, &mut self.rng));
            self.request = Some((tap, due));
        }
        let (_, due) = self.request.unwrap();
        if reached(tick, due)
            && self.free.is_none_or(|t| reached(tick, t))
            && self.ready[slot].is_none_or(|t| reached(tick, t))
        {
            let until = tick.wrapping_add(self.rng.ticks(2, 4));
            self.down = Some((tap, until));
            self.free = Some(until.wrapping_add(self.rng.ticks(2, 3)));
            self.ready[slot] = Some(tick.wrapping_add(cooldown + self.rng.ticks(0, 6)));
            return Some(button);
        }
        None
    }

    /// Navigation and dodges request one jump. Hold it for a few ticks
    /// and require release before another down edge.
    pub(super) fn jump(&mut self, request: bool, tick: u32) -> bool {
        if request
            && !self.jump_request
            && self
                .jump_until
                .is_none_or(|until| reached(tick, until.wrapping_add(2)))
        {
            self.jump_until = Some(tick.wrapping_add(self.rng.ticks(3, 4)));
        }
        self.jump_request = request;
        self.jump_until.is_some_and(|until| !reached(tick, until))
    }
}

/// Keep the left thumb's inertia in world space. Turning the camera does
/// not push the bot off its route.
#[derive(Clone, Debug)]
pub(super) struct Feet {
    walk: [f32; 2],
    pace: f32,
    slew: f32,
}

impl Feet {
    pub(super) fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed ^ 0x666f_6f74_776b);
        Self {
            walk: [0.0; 2],
            pace: 0.91 + 0.085 * rng.unit(),
            slew: 0.34 + 0.12 * rng.unit(),
        }
    }

    pub(super) fn stop(&mut self) {
        self.walk = [0.0; 2];
    }

    pub(super) fn step(
        &mut self,
        desired: Option<[f32; 2]>,
        dt: u32,
        slow: f32,
        immediate: bool,
        safe: impl FnOnce([f32; 2]) -> bool,
    ) -> [f32; 2] {
        let Some(desired) = desired else {
            self.stop();
            return self.walk;
        };
        if desired[0].hypot(desired[1]) < 1e-3 {
            self.stop();
            return self.walk;
        }
        let want = limit(desired, 1.0).map(|a| a * self.pace * slow);
        let delta = limit(
            [want[0] - self.walk[0], want[1] - self.walk[1]],
            self.slew * dt.clamp(1, 3) as f32,
        );
        let eased = [self.walk[0] + delta[0], self.walk[1] + delta[1]];
        let off_line = (eased[0] * want[1] - eased[1] * want[0]).abs() > 0.005;
        let opposing = eased[0] * want[0] + eased[1] * want[1] < 0.0;
        self.walk = if immediate || ((off_line || opposing) && !safe(eased)) {
            want
        } else {
            eased
        };
        // Enter the slow band promptly on a crouch or approach to a stop.
        self.walk = limit(self.walk, slow);
        self.walk
    }
}

/// Small right-thumb movement while waiting. No buttons or left-stick
/// motion, so waiting never confirms a menu option or requests a respawn.
#[derive(Clone, Debug)]
pub(crate) struct Fidget {
    rng: Rng,
    last: Option<u64>,
    next: Option<u64>,
    start: u64,
    end: u64,
    stick: (f32, f32),
}

impl Fidget {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed ^ 0x6669_6467_6574),
            last: None,
            next: None,
            start: 0,
            end: 0,
            stick: (0.0, 0.0),
        }
    }

    pub(crate) fn reset(&mut self) {
        self.next = None;
        self.end = 0;
    }

    pub(crate) fn pad(&mut self, time: u64) -> HostPadState {
        if self.last.is_some_and(|last| time < last) {
            self.reset();
        }
        self.last = Some(time);
        let due = *self
            .next
            .get_or_insert_with(|| time + u64::from(self.rng.ticks(20, 65)));
        if time >= due {
            self.start = time;
            self.end = time + u64::from(self.rng.ticks(4, 7));
            self.next = Some(self.end + u64::from(self.rng.ticks(30, 110)));
            self.stick = (0.2 * self.rng.signed().signum(), 0.1 * self.rng.signed());
        }
        let mut pad = HostPadState::default();
        if time < self.end {
            let amount =
                ((time.saturating_sub(self.start) + 1).min(self.end - time) as f32 * 0.4).min(1.0);
            pad.right_stick = (self.stick.0 * amount, self.stick.1 * amount);
        }
        pad
    }
}

#[cfg(test)]
mod tests;
