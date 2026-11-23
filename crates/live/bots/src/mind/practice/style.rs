//! How a bot practises (docs/game/bots.md, "How each bot practises"):
//! its tastes, how often it fumbles, how long it waits for someone and
//! how keen it is on chains and Monsters. Each is a continuous draw from
//! the bot's name ([`crate::name_profile`]'s hash, the seed without one),
//! placed inside its skill's band, so a name practises the same way every
//! match and two names practise differently. Match randomness decides the
//! moments (which try fumbles, which chain it goes for); the name decides
//! how often.

use crate::mind::Skill;
use crate::name_profile::draw;

/// The name hash's lanes the practice draws from (the name profile's
/// combat traits use 1 to 13).
const LANE: u64 = 40;

/// One band a skill: Easy, Normal, Heroic, Legendary.
type Bands = [(f32, f32); 4];

/// Fumbles a try, by skill: the share of tries the bot sets up a little
/// wrong (off the line, early or late, lazy in the air).
pub const MISS_BANDS: Bands = [(0.15, 0.30), (0.10, 0.20), (0.06, 0.14), (0.03, 0.10)];
/// Goes for a chain when one is in reach of the launch, by skill.
pub const CHAIN_BANDS: Bands = [(0.15, 0.45), (0.25, 0.60), (0.40, 0.80), (0.55, 0.95)];
/// The pull of a Monster site, by skill: times [`super::W_MONSTER`].
pub const MONSTER_BANDS: Bands = [(0.10, 0.60), (0.15, 0.75), (0.20, 0.85), (0.25, 0.95)];
/// How long it waits for a player on its site before going elsewhere
/// (ticks): 3 to 12 s.
pub const PATIENCE: (u32, u32) = (90, 360);
/// The chance it goes again at a site whose landing it just missed.
pub const RETRY: (f32, f32) = (0.20, 0.70);
/// How much recent tries weigh a site down: times [`super::W_RECENT`].
pub const NOVELTY: (f32, f32) = (0.6, 1.4);
/// The distance weight, times [`super::W_DISTANCE`]: a homebody's 1.6, a
/// wanderer's 0.4.
pub const ROAM: (f32, f32) = (1.6, 0.4);

/// A bot's way of practising: every field a continuous value inside its
/// band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Style {
    /// Taste for slots (the two-joint strips), 0 to 1: 0.5 is neutral.
    pub slots: f32,
    /// Taste for single creases, Vs and busy points, 0 to 1.
    pub creases: f32,
    /// The distance weight's factor ([`ROAM`]): under 1 a bot that ranges
    /// far, over 1 one that stays near.
    pub roam: f32,
    /// How much recent tries weigh a site down ([`NOVELTY`]).
    pub novelty: f32,
    /// The share of tries it fumbles ([`MISS_BANDS`]).
    pub miss: f32,
    /// The chance it goes again at a site it just missed ([`RETRY`]).
    pub retry: f32,
    /// How long it waits for a player before it goes elsewhere, ticks
    /// ([`PATIENCE`]).
    pub patience: u32,
    /// The chance it chains, a link at a time ([`CHAIN_BANDS`]).
    pub chain: f32,
    /// The pull of a Monster site ([`MONSTER_BANDS`]).
    pub monster: f32,
}

fn lerp((a, b): (f32, f32), x: f32) -> f32 {
    a + (b - a) * x
}

impl Style {
    /// The style of the bot whose name hashes to `seed`, at `skill`. The
    /// name's try-hard draw (the name profile's own, lane 1) pulls the
    /// fumbles down and the chains up, as it does its combat traits.
    pub fn for_name(seed: u64, skill: Skill) -> Style {
        let d = |lane: u64| draw(seed, LANE + lane);
        let try_hard = draw(seed, 1);
        // Half the name's own draw, half its try-hard.
        let keen = |lane: u64| 0.5 * d(lane) + 0.5 * try_hard;
        let band = |b: Bands, x: f32| lerp(b[skill as usize], x);
        Style {
            slots: d(0),
            creases: d(1),
            roam: lerp(ROAM, d(2)),
            novelty: lerp(NOVELTY, d(3)),
            miss: band(MISS_BANDS, 1.0 - keen(4)),
            retry: lerp(RETRY, d(5)),
            patience: lerp((PATIENCE.0 as f32, PATIENCE.1 as f32), d(6)).round() as u32,
            chain: band(CHAIN_BANDS, keen(7)),
            monster: band(MONSTER_BANDS, d(8)),
        }
    }

    /// Its taste for a site of this shape, -1 to 1 (0 neutral).
    pub fn taste(&self, shape: superbounce::Shape) -> f32 {
        let t = match shape {
            superbounce::Shape::Slot => self.slots,
            _ => self.creases,
        };
        2.0 * t - 1.0
    }
}

impl Default for Style {
    /// The middle of every band at Normal: what a test without a name
    /// plays.
    fn default() -> Style {
        let mid = |b: Bands| lerp(b[Skill::Normal as usize], 0.5);
        Style {
            slots: 0.5,
            creases: 0.5,
            roam: 1.0,
            novelty: 1.0,
            miss: mid(MISS_BANDS),
            retry: lerp(RETRY, 0.5),
            patience: (PATIENCE.0 + PATIENCE.1) / 2,
            chain: mid(CHAIN_BANDS),
            monster: mid(MONSTER_BANDS),
        }
    }
}
