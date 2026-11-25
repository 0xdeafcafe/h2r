//! Chains (docs/game/bots.md, "Chains"): after a bounce, the sites the
//! bot can come down on from its launch without touching the ground, so
//! a practice route can plan the next bounce of a chain
//! ([`super::Step::Chain`]).
//!
//! The flight is a [`Trajectory`]: what a launch off one site can reach.
//! No flight after a bounce has been traced (docs/game/superbounce.md,
//! "What is known about a second bounce"), so the one built in,
//! [`Ballistic`], is the conservative model the route's own numbers give:
//! the launch the site's estimate says, the measured fall gravity
//! ([`super::FALL_GRAVITY`]), and the measured air control
//! ([`super::AIR_ACCEL`] up to [`super::AIR_TOP`]), with a margin. It
//! does not know which touchdown speeds a seam takes after a launch:
//! [`Ballistic::max_speed_in`] is where a measured window goes.

use super::{AIR_ACCEL, AIR_TOP, CLEARANCE, FALL_GRAVITY};
use crate::spots::{Site, dist_xy};

/// The most links a practice route plans after its first bounce.
pub const LINKS: usize = 3;

/// One leg of a chain: the flight off one site's launch down onto the
/// next.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    /// Seconds from the launch to the touchdown.
    pub time: f32,
    /// How far sideways (XY) the air stick must move the bot.
    pub steer: f32,
    /// The touchdown's downward speed (world units a second).
    pub speed_in: f32,
}

/// What a launch off a site can reach.
pub trait Trajectory {
    /// The leg off `from`'s launch down onto `to`, or `None` when the
    /// flight cannot come down on it.
    fn leg(&self, from: &Site, to: &Site) -> Option<Leg>;
}

/// The built-in flight: straight up at the site's launch, down under
/// gravity, the air stick moving the bot sideways at most `air_top`
/// after speeding up at `air_accel`, and only `margin` of that counted.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ballistic {
    pub gravity: f32,
    pub air_accel: f32,
    pub air_top: f32,
    /// The share of the reach counted (0 to 1).
    pub margin: f32,
    /// The fastest touchdown a seam takes after a launch, when measured;
    /// `None`: any at or over the site's `need`.
    pub max_speed_in: Option<f32>,
}

impl Default for Ballistic {
    fn default() -> Ballistic {
        Ballistic {
            gravity: FALL_GRAVITY,
            air_accel: AIR_ACCEL,
            air_top: AIR_TOP,
            margin: 0.75,
            max_speed_in: None,
        }
    }
}

impl Ballistic {
    /// How far sideways the air stick moves the bot in `t` seconds from
    /// standing still sideways, before the margin.
    pub fn reach(&self, t: f32) -> f32 {
        let ramp = self.air_top / self.air_accel;
        if t <= ramp {
            0.5 * self.air_accel * t * t
        } else {
            self.air_top * (t - ramp / 2.0)
        }
    }
}

impl Trajectory for Ballistic {
    fn leg(&self, from: &Site, to: &Site) -> Option<Leg> {
        let v = from.launch;
        if v <= 0.0 {
            return None;
        }
        let z0 = from.bounce.stand[2];
        let z1 = to.target()[2];
        let g = self.gravity;
        // The top must clear the next site's floor.
        if z0 + v * v / (2.0 * g) < z1 + CLEARANCE {
            return None;
        }
        let d = v * v + 2.0 * g * (z0 - z1);
        let speed_in = d.sqrt();
        let time = (v + speed_in) / g;
        let steer = dist_xy(from.target(), to.target());
        if steer > self.margin * self.reach(time) {
            return None;
        }
        if to.need > 0.0 && speed_in < to.need {
            return None;
        }
        if self.max_speed_in.is_some_and(|m| speed_in > m) {
            return None;
        }
        // A V's arm only from across its bisector.
        let at = from.target();
        if !to.approached_from([at[0], at[1]]) {
            return None;
        }
        Some(Leg {
            time,
            steer,
            speed_in,
        })
    }
}

/// The sites a launch off `from` can come down on, with their legs, the
/// least steering first. `from` itself is among them (straight back down
/// onto the same seam) when its own flight allows it.
pub fn reachable(from: &Site, sites: &[Site], model: &dyn Trajectory) -> Vec<(Site, Leg)> {
    let mut out: Vec<(Site, Leg)> = sites
        .iter()
        .filter_map(|s| Some((*s, model.leg(from, s)?)))
        .collect();
    out.sort_by(|a, b| a.1.steer.total_cmp(&b.1.steer));
    out
}

/// A chain of up to `links` sites after `first`: each link one of the
/// sites the last one's launch reaches ([`reachable`]), chosen by
/// `choose` (an index into them, or `None` to stop there). The chain
/// stops at the first link with nothing in reach or nothing chosen.
pub fn plan(
    first: &Site,
    sites: &[Site],
    model: &dyn Trajectory,
    links: usize,
    choose: &mut dyn FnMut(&Site, &[(Site, Leg)]) -> Option<usize>,
) -> Vec<Site> {
    let mut out = Vec::new();
    let mut from = *first;
    for _ in 0..links {
        let near = reachable(&from, sites, model);
        let Some(i) = choose(&from, &near).filter(|&i| i < near.len()) else {
            break;
        };
        from = near[i].0;
        out.push(from);
    }
    out
}

#[cfg(test)]
mod tests;
