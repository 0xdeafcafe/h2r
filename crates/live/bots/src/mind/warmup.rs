//! The start-of-match warm-up (docs/game/bots.md, "Warm-up"): for the
//! first seconds of its first life in a match a bot looks round and moves
//! off its spawn as usual but picks no target, so it does not shoot,
//! throw or melee. How long is the bot's own, from its name (the
//! Clanker personality's hash, `name_profile`), so it is the same every
//! match. Taking damage ends it at once: a bot that is shot fights back.
//!
//! A mind is made fresh for each match (`Brain::reset_mind`), so a
//! respawn does not start it again.

use super::feel::reached;

/// The shortest and the longest warm-up, ticks (2 and 4 s at 30 Hz).
pub const WARMUP_MIN_TICKS: u32 = 60;
pub const WARMUP_MAX_TICKS: u32 = 120;

/// The warm-up of a bot whose name hashes to `seed`, ticks: stable, and
/// spread over [`WARMUP_MIN_TICKS`]..=[`WARMUP_MAX_TICKS`].
pub fn warmup_ticks(seed: u64) -> u32 {
    // splitmix64's finaliser: neighbouring seeds land far apart.
    let mut z = seed ^ 0x77a7_2b0f_d1e5_9c03;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^= z >> 31;
    let span = u64::from(WARMUP_MAX_TICKS - WARMUP_MIN_TICKS + 1);
    WARMUP_MIN_TICKS + (z % span) as u32
}

/// Where a bot's warm-up stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Warmup {
    /// Not begun: it begins at the bot's first living tick.
    Due,
    /// Holding fire until the tick.
    Until(u32),
    /// Over, or none (a mind not made for a match: the tests').
    #[default]
    Over,
}

impl Warmup {
    /// Whether the bot holds fire at `tick`, beginning a `ticks`-long
    /// warm-up when it is due; `hurt` (it took damage this tick) ends it.
    pub fn holding(&mut self, tick: u32, ticks: u32, hurt: bool) -> bool {
        match *self {
            Warmup::Due if hurt => {
                *self = Warmup::Over;
                false
            }
            Warmup::Due => {
                *self = Warmup::Until(tick.wrapping_add(ticks));
                true
            }
            Warmup::Until(t) if hurt || reached(tick, t) => {
                *self = Warmup::Over;
                false
            }
            Warmup::Until(_) => true,
            Warmup::Over => false,
        }
    }

    /// Ticks of warm-up left at `tick`; `None` when not holding.
    pub fn left(&self, tick: u32) -> Option<u32> {
        match *self {
            Warmup::Until(t) if !reached(tick, t) => Some(t.wrapping_sub(tick)),
            _ => None,
        }
    }
}
