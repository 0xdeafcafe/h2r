//! Multiplayer bots, "Clankers" (docs/game/bots.md).
//!
//! One brain, two ways in. A bot is driven by a [`local::PadSource`]: the
//! brain ([`Brain`]), a script or a recording, each giving controller input
//! and nothing else. The first way in is local slots ([`local`]): a pad on
//! a free controller port, signed in with a profile of its own on the
//! players screen ([`guest::sign_in`]), with its view and listener hidden
//! ([`guest::hide`]). The second, phantom peers ([`phantom`]), is stage 6.
//! The boot glue (`boot::bot_glue`) places the hooks.

pub mod arms;
pub mod autonav;
pub mod brain;
pub mod cache;
pub mod decisions;
pub mod emote;
pub mod glitches;
pub mod grenades;
pub mod guest;
mod incoming;
pub mod lobby;
pub mod local;
pub mod los;
pub mod mind;
pub mod moves;
pub mod nav;
pub mod objective;
pub mod phantom;
pub mod roam;
mod sound;
pub mod stuck;
pub mod tactics;
pub mod weapons;

use decisions::radio;
use input::host::HostPadState;

pub use sound::{clear_shots, observe_shot};

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

/// The brain as a pad source: in a multiplayer game its [`mind::Mind`] with
/// the loaded map's data ([`cache::current`]). A waiting local pad may
/// fidget with the right stick. Sign-in keeps the pad neutral.
pub struct Brain {
    pub mind: mind::Mind,
    /// The last death heard from [`emote::kills`].
    kills_seen: u64,
    fidget: mind::pad::Fidget,
    seed: u64,
    epoch: u64,
    /// The map [`Brain::mind`]'s super bounce spots are for.
    map: String,
}

impl Brain {
    pub fn new(skill: mind::Skill, seed: u64) -> Self {
        Brain {
            mind: mind::Mind::new(skill, seed),
            seed,
            epoch: radio::lock(radio::host()).epoch,
            kills_seen: emote::kills().since(u64::MAX).1,
            fidget: mind::pad::Fidget::new(seed),
            map: String::new(),
        }
    }

    /// One observer and one pad. The radio lock covers the exchange only.
    fn play(
        &mut self,
        p: &brain::Perception,
        map: Option<&cache::MapData>,
        kills: &[emote::Kill],
        channel: &std::sync::Mutex<radio::Radio>,
    ) -> HostPadState {
        self.play_with_cues(p, map, kills, channel, &[], &[])
    }

    /// [`Brain::play`] with what the bot hears and the live grenades near
    /// it ([`mind::Mind::step_with_cues`]).
    fn play_with_cues(
        &mut self,
        p: &brain::Perception,
        map: Option<&cache::MapData>,
        kills: &[emote::Kill],
        channel: &std::sync::Mutex<radio::Radio>,
        sounds: &[sound::ShotSound],
        grenades: &[incoming::Grenade],
    ) -> HostPadState {
        let (epoch, inbox) = {
            let mut board = radio::lock(channel);
            if p.observations.player.is_some() {
                board.advance(p.tick);
                if board.epoch == self.epoch {
                    board.forget_deaths(kills);
                }
            }
            (board.epoch, board.listen(p))
        };
        if self.epoch != epoch {
            let taunt = self.mind.emote.taunt;
            let glitches = self.mind.glitches;
            let bounces = std::mem::take(&mut self.mind.bounces);
            self.mind = mind::Mind::new(self.mind.skill, self.seed);
            self.mind.emote.taunt = taunt;
            self.mind.glitches = glitches;
            self.mind.bounces = bounces;
            self.epoch = epoch;
        } else {
            self.mind.hear_deaths(p, kills);
            self.mind.emote.hear(kills);
        }
        self.mind.decisions.inbox = inbox;
        let pad = self.mind.step_with_cues(p, map, sounds, grenades);
        if let Some(call) = self.mind.decisions.report {
            let mut board = radio::lock(channel);
            if board.epoch == epoch {
                board.publish(call);
            }
        }
        pad
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
            self.fidget.reset();
            return HostPadState::default();
        }
        let who = match f.machine {
            Some(id) => guest::Who::Machine(id),
            None => guest::Who::Port(u32::from(f.port)),
        };
        let Ok((p, grenades)) = guest::perceive_for_brain(f.guest, who) else {
            self.fidget.reset();
            return HostPadState::default();
        };
        // This map's super bounce spots (crate::glitches::spots).
        let map = autonav::loaded();
        if map != self.map {
            self.mind.bounces = glitches::bounces(&map);
            self.map = map;
        }
        let (kills, cursor) = emote::kills().since(self.kills_seen);
        self.kills_seen = cursor;
        // The radio and the deaths every frame, alive or not
        // (crate::decisions); the shots heard only in play.
        let alive = p.me.is_some();
        let shots = if alive {
            sound::recent(p.tick)
        } else {
            Vec::new()
        };
        let pad = self.play_with_cues(
            &p,
            cache::current().as_deref(),
            &kills,
            radio::host(),
            &shots,
            &grenades,
        );
        if alive {
            self.fidget.reset();
            return pad;
        }
        // Only a signed-in local pad fidgets in a pregame lobby.
        // Outside play it sends only right-stick axes.
        if f.machine.is_none()
            && (guest::game_mode(f.guest) == Some(guest::MODE_MULTIPLAYER)
                || (guest::slot_held(f.guest, u32::from(f.port)).unwrap_or(false)
                    && lobby::in_pregame(f.guest)))
        {
            self.fidget.pad(f.frame)
        } else {
            self.fidget.reset();
            pad
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
    fn the_brain_presses_nothing_while_joining_or_outside_a_game() {
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
        // Signed in by the glue: an A here would open a profile chooser.
        for frame in 0..120 {
            assert!(
                !b.pad(&f(frame, true)).buttons.contains(HostButton::South),
                "frame {frame}"
            );
            assert_eq!(b.pad(&f(frame, true)), HostPadState::default());
        }
        // Unmapped memory: no game, no pad.
        assert_eq!(b.pad(&f(0, false)), HostPadState::default());
    }

    #[test]
    fn a_signed_in_lobby_bot_only_fidgets_its_right_stick() {
        let mut g = engine::testing::FakeGuest::new();
        let session = 0x1000;
        g.put_u32(engine::points::GAME_GLOBALS.base, 0)
            .put_u8(lobby::LOBBY_ON, 1)
            .put_u8(lobby::NETWORKING, 1)
            .put_u32(lobby::SESSION_TYPE, 2)
            .put_u32(lobby::MAP_FAILED_HERE, 0)
            .put_u32(lobby::SESSION, session)
            .map(session, 0x7500)
            .put_u32(session + lobby::STATE, 5)
            .put_u32(guest::slot(1), guest::SLOT_PROFILE);
        let mut b = Brain::default();
        let mut moved = false;
        for frame in 0..160 {
            let pad = b.pad(&Frame {
                frame,
                tick: None,
                port: 1,
                machine: None,
                guest: &g,
                joining: false,
            });
            assert_eq!((pad.buttons.bits(), pad.left_stick), (0, (0.0, 0.0)));
            assert_eq!((pad.left_trigger, pad.right_trigger), (0.0, 0.0));
            moved |= pad.right_stick != (0.0, 0.0);
        }
        assert!(moved);
    }
}
