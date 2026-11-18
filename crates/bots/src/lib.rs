//! Multiplayer bots, "Clankers" (docs/game/bots.md).
//!
//! One brain, two ways in. A bot is driven by a [`local::PadSource`]: the
//! brain ([`Brain`]), a script or a recording, each giving controller input
//! and nothing else. The first way in is local slots ([`local`]): a pad on
//! a free controller port, signed in with a profile of its own on the
//! players screen ([`guest::sign_in`]), with its view and listener hidden
//! ([`guest::hide`]). The second, phantom peers ([`phantom`]), is stage 6.
//! The boot glue (`boot::bot_glue`) places the hooks.

pub mod brain;
pub mod cache;
pub mod guest;
pub mod local;
pub mod los;
pub mod mind;
pub mod nav;
pub mod phantom;

use input::host::HostPadState;

/// `H2_BOTS`: how many local bots to add at boot, 0 to
/// [`local::MAX_LOCAL`], on the ports after port 0.
pub const ENV: &str = "H2_BOTS";

/// How many bots `H2_BOTS` asks for. Unset or empty is none.
pub fn from_env_value(v: Option<&str>) -> Result<usize, String> {
    let Some(v) = v.map(str::trim).filter(|v| !v.is_empty()) else {
        return Ok(0);
    };
    match v.parse::<usize>() {
        Ok(n) if n <= local::MAX_LOCAL => Ok(n),
        _ => Err(format!("{ENV}={v:?}: expected 0 to {}", local::MAX_LOCAL)),
    }
}

/// The brain as a pad source: A on the players screen until signed in,
/// then in a multiplayer game its [`mind::Mind`] with the loaded map's data
/// ([`cache::current`]), idle anywhere else.
pub struct Brain {
    pub mind: mind::Mind,
}

impl Brain {
    pub fn new(skill: mind::Skill, seed: u64) -> Self {
        Brain {
            mind: mind::Mind::new(skill, seed),
        }
    }
}

impl Default for Brain {
    fn default() -> Self {
        Brain::new(mind::Skill::default(), 0x5eed)
    }
}

impl local::PadSource for Brain {
    fn pad(&mut self, f: &local::Frame<'_>) -> HostPadState {
        if f.joining {
            return local::join_pad(f.frame);
        }
        let who = match f.machine {
            Some(id) => guest::Who::Machine(id),
            None => guest::Who::Port(u32::from(f.port)),
        };
        match guest::perceive_as(f.guest, who) {
            Ok(p) => self.mind.step(&p, cache::current().as_deref()),
            Err(_) => HostPadState::default(),
        }
    }
}

/// `H2_BOTS_SKILL`: the `H2_BOTS` brains' skill (easy, normal, heroic,
/// legendary; default normal).
pub const SKILL_ENV: &str = "H2_BOTS_SKILL";

#[cfg(test)]
mod tests {
    use super::*;
    use input::host::HostButton;
    use local::{Frame, PadSource};

    #[test]
    fn h2_bots_is_a_count() {
        assert_eq!(from_env_value(None), Ok(0));
        assert_eq!(from_env_value(Some("0")), Ok(0));
        assert_eq!(from_env_value(Some(" 2 ")), Ok(2));
        assert!(from_env_value(Some("4")).is_err());
        assert!(from_env_value(Some("lots")).is_err());
    }

    #[test]
    fn the_brain_taps_a_while_joining_and_is_idle_outside_a_game() {
        let g = engine::testing::FakeGuest::new();
        let mut b = Brain::default();
        let f = |frame, joining| Frame {
            frame,
            tick: None,
            port: 1,
            machine: None,
            guest: &g,
            joining,
        };
        assert!(b.pad(&f(0, true)).buttons.contains(HostButton::South));
        // Unmapped memory: no game, no pad.
        assert_eq!(b.pad(&f(0, false)), HostPadState::default());
    }
}
