//! The motion tracker as a player reads it (docs/game/bots.md, "The
//! motion tracker"): the dots a glance showed, each remembered for
//! [`KEEP_TICKS`] after the last glance that showed it, so a missed glance
//! does not wipe them. A dot has no identity and no height: it can turn
//! the head, put the bot on its guard and send it to look, never pick a
//! target or aim a shot.

use super::feel::reached;
use super::{Vec3, angle_between, dist_xy, sub};
use crate::brain::Seen;
use crate::cache::MapData;

/// A dot is remembered this long after the last glance that showed it.
pub(crate) const KEEP_TICKS: u32 = 45;
/// At most this many dots, the HUD's nearest.
pub(crate) const DOTS: usize = 4;
/// A dot this near (XY) a remembered one is that dot, moved.
pub(crate) const SAME: f32 = 1.5;
/// The disc's radius when the host does not give it (about 25 m).
pub(crate) const RANGE: f32 = 8.0;
/// A dot inside this fraction of the disc puts the bot on its guard: the
/// next sighting is an alert one, and it is not calm.
pub(crate) const NEAR: f32 = 0.5;
/// Travelling, the look at a new dot lasts this long (ticks, rolled).
pub(crate) const GLANCE_TICKS: (u32, u32) = (12, 20);
/// In a fight, a dot behind it this close (XY) is worth a turn...
pub(crate) const BEHIND: f32 = 3.0;
/// ...that lasts this long (ticks, rolled).
pub(crate) const CHECK_TICKS: (u32, u32) = (8, 12);
/// Standing this near (XY) a dot is at it.
pub(crate) const ARRIVED: f32 = 1.0;
/// At a dot with nothing in view, a floor at least this far over or
/// under it, and at most the second, is where its player may be.
pub(crate) const OTHER_FLOOR: (f32, f32) = (1.0, 8.0);

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Dot {
    /// Where it is: the bot's own height when read, another floor's once
    /// that was tried ([`Tracker::arrive`]).
    pub at: Vec3,
    /// The last glance that showed it.
    pub tick: u32,
    floor_tried: bool,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct Tracker {
    dots: Vec<Dot>,
    range: Option<f32>,
    /// Travelling, the look at a new dot ends at this tick.
    glance_until: Option<u32>,
    /// A turn in a fight to check a dot behind, and its end.
    check: Option<(Vec3, u32)>,
}

impl Tracker {
    pub(crate) fn reset(&mut self) {
        *self = Self::default();
    }

    fn forget(&mut self, tick: u32) {
        self.dots
            .retain(|d| tick.wrapping_sub(d.tick) <= KEEP_TICKS);
        if self.check.is_some_and(|(_, until)| reached(tick, until)) {
            self.check = None;
        }
    }

    fn fresh(&self, tick: u32) -> impl Iterator<Item = &Dot> {
        self.dots
            .iter()
            .filter(move |d| tick.wrapping_sub(d.tick) <= KEEP_TICKS)
    }

    /// A glance that read `dots` (nearest first): each moves the
    /// remembered dot within [`SAME`] or is a new one. A new dot starts a
    /// look of `look_ticks` while travelling ([`Tracker::glancing`]). A
    /// glance that shows nothing forgets nothing.
    pub(crate) fn saw(&mut self, dots: &[Vec3], range: Option<f32>, tick: u32, look_ticks: u32) {
        self.forget(tick);
        if range.is_some() {
            self.range = range;
        }
        let mut new = false;
        for &at in dots.iter().take(DOTS) {
            if let Some(d) = self.dots.iter_mut().find(|d| dist_xy(d.at, at) < SAME) {
                let z = if d.floor_tried { d.at[2] } else { at[2] };
                d.at = [at[0], at[1], z];
                d.tick = tick;
                continue;
            }
            if self.dots.len() >= DOTS
                && let Some(oldest) = (0..self.dots.len()).min_by_key(|&i| self.dots[i].tick)
            {
                self.dots.remove(oldest);
            }
            self.dots.push(Dot {
                at,
                tick,
                floor_tried: false,
            });
            new = true;
        }
        if new {
            self.glance_until = Some(tick.wrapping_add(look_ticks));
        }
    }

    /// The remembered dots.
    #[cfg(test)]
    pub(crate) fn dots(&self, tick: u32) -> Vec<Vec3> {
        self.fresh(tick).map(|d| d.at).collect()
    }

    /// The nearest remembered dot (XY).
    pub(crate) fn nearest(&self, from: Vec3, tick: u32) -> Option<Vec3> {
        self.fresh(tick)
            .min_by(|a, b| dist_xy(a.at, from).total_cmp(&dist_xy(b.at, from)))
            .map(|d| d.at)
    }

    /// A remembered dot inside [`NEAR`] of the disc: on its guard.
    pub(crate) fn near(&self, from: Vec3, tick: u32) -> bool {
        let reach = self.range.unwrap_or(RANGE) * NEAR;
        self.fresh(tick).any(|d| dist_xy(d.at, from) < reach)
    }

    /// Where to look for the player behind a dot between fights: the
    /// nearest remembered dot it is not standing at.
    pub(crate) fn goal(&self, from: Vec3, tick: u32) -> Option<Vec3> {
        self.fresh(tick)
            .filter(|d| dist_xy(d.at, from) > ARRIVED || (d.at[2] - from[2]).abs() >= OTHER_FLOOR.0)
            .min_by(|a, b| dist_xy(a.at, from).total_cmp(&dist_xy(b.at, from)))
            .map(|d| d.at)
    }

    /// Standing at a dot with nothing in view: its player is on another
    /// floor, or gone. The dot moves to the nav mesh's nearest floor over
    /// or under it ([`OTHER_FLOOR`]); with none, or tried already, it is
    /// dropped.
    pub(crate) fn arrive(&mut self, at: Vec3, map: Option<&MapData>, tick: u32) {
        self.forget(tick);
        let Some(i) = self
            .dots
            .iter()
            .position(|d| dist_xy(d.at, at) <= ARRIVED && (d.at[2] - at[2]).abs() < OTHER_FLOOR.0)
        else {
            return;
        };
        let d = &mut self.dots[i];
        match map
            .filter(|_| !d.floor_tried)
            .and_then(|m| other_floor(m, d.at))
        {
            Some(floor) => {
                d.at = floor;
                d.floor_tried = true;
            }
            None => {
                self.dots.remove(i);
            }
        }
    }

    /// Travelling, a look at a new dot is still due.
    pub(crate) fn glancing(&self, tick: u32) -> bool {
        self.glance_until.is_some_and(|until| !reached(tick, until))
    }

    /// In a fight, after a glance: a dot read this tick behind the bot
    /// (more than a right angle off its aim), within [`BEHIND`], with no
    /// enemy in view near it, starts a turn of `ticks` to check it.
    pub(crate) fn check_behind(
        &mut self,
        at: Vec3,
        aim: Vec3,
        seen: &[Seen],
        tick: u32,
        ticks: u32,
    ) {
        if self.check.is_some() {
            return;
        }
        let flat = [aim[0], aim[1], 0.0];
        let behind = self.dots.iter().find(|d| {
            let to = sub(d.at, at);
            d.tick == tick
                && dist_xy(d.at, at) < BEHIND
                && angle_between([to[0], to[1], 0.0], flat) > std::f32::consts::FRAC_PI_2
                && !seen.iter().any(|o| dist_xy(o.position, d.at) < SAME)
        });
        if let Some(d) = behind {
            self.check = Some((d.at, tick.wrapping_add(ticks)));
        }
    }

    /// The dot a fight's turn is checking, while it lasts.
    pub(crate) fn checking(&mut self, tick: u32) -> Option<Vec3> {
        if self.check.is_some_and(|(_, until)| reached(tick, until)) {
            self.check = None;
        }
        self.check.map(|(at, _)| at)
    }
}

/// The nav mesh's floor nearest `at` (XY within [`ARRIVED`]) at least
/// [`OTHER_FLOOR`]`.0` over or under it, the nearer in height first.
fn other_floor(m: &MapData, at: Vec3) -> Option<Vec3> {
    let (min, max) = OTHER_FLOOR;
    let over = m
        .nav
        .nearest([at[0], at[1], at[2] + min], ARRIVED, max - min, 0.0)
        .map(|(_, p)| p);
    let under = m
        .nav
        .nearest([at[0], at[1], at[2] - min], ARRIVED, 0.0, max - min)
        .map(|(_, p)| p);
    match (over, under) {
        (Some(a), Some(b)) if (b[2] - at[2]).abs() < (a[2] - at[2]).abs() => Some(b),
        (a, b) => a.or(b),
    }
}

#[cfg(test)]
mod tests;
