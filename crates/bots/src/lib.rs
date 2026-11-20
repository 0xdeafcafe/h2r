//! Multiplayer bots, "Clankers" (docs/game/bots.md).
//!
//! One brain, two ways in. A bot is driven by a [`local::PadSource`]: the
//! brain ([`Brain`]), a script or a recording, each giving controller input
//! and nothing else. The first way in is local slots ([`local`]): a pad on
//! a free controller port, signed in with a profile of its own on the
//! players screen ([`guest::sign_in`]), with its view and listener hidden
//! ([`guest::hide`]). The second, phantom peers ([`phantom`]), is stage 6.
//! The boot glue (`boot::bot_glue`) places the hooks.

pub mod aim_net;
pub mod arms;
pub mod autonav;
pub mod brain;
pub mod cache;
pub mod decisions;
pub mod emote;
pub mod experience;
pub mod glitches;
pub mod grenades;
pub mod guest;
mod human_input;
mod incoming;
pub mod lobby;
pub mod local;
pub mod los;
pub mod mind;
pub mod moves;
pub mod nav;
pub mod objective;
mod perceive;
pub mod phantom;
pub mod places;
pub mod roam;
mod sound;
pub mod stuck;
pub mod tactics;
pub mod tuning;
pub mod vehicle;
mod visits;
pub mod weapons;
mod work;

use std::collections::VecDeque;

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
    /// Over the skill's aim table, awareness and the seeded personality
    /// (sim mode's tuning files; [`tuning`]).
    tuning: Option<tuning::Tuning>,
    controller: human_input::Controller,
    observations: VecDeque<Observation>,
    spare_observations: Vec<Observation>,
    sampled: Option<(u32, bool)>,
    pending_kills: Vec<emote::Kill>,
    visibility: perceive::Visibility,
    player_unit: Option<(u32, u32)>,
    vehicle_headers: Vec<vehicle::read::VehicleHeader>,
    vehicle_seats: vehicle::read::SeatBuffers,
}

#[derive(Default)]
struct Observation {
    perception: brain::Perception,
    visible_players: Option<u16>,
    vehicles: vehicle::Sight,
    kills: Vec<emote::Kill>,
    sounds: Vec<sound::ShotSound>,
    grenades: Vec<incoming::Grenade>,
    inbox: Vec<radio::Call>,
    supply_calls: Vec<radio::SupplyCall>,
}

/// Read-only state shared by local, phantom and simulated bot views.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrainView {
    pub skill: mind::Skill,
    pub personality: decisions::Personality,
    pub mood: decisions::MoodView,
}

impl Brain {
    pub fn view(&self) -> BrainView {
        BrainView {
            skill: self.mind.skill,
            personality: self.mind.decisions.personality,
            mood: self.mind.decisions.mood(),
        }
    }

    pub fn set_skill(&mut self, skill: mind::Skill) {
        if self.mind.skill != skill {
            self.mind.skill = skill;
            self.mind.decisions.personality = decisions::Personality::for_skill(self.seed, skill);
            self.retune();
        }
    }

    pub fn set_identity_seed(&mut self, seed: u64) {
        if self.seed != seed {
            self.seed = seed;
            self.kills_seen = emote::kills().since(u64::MAX).1;
            self.reset_mind();
        }
    }

    fn reset_mind(&mut self) {
        let taunt = self.mind.emote.taunt;
        let glitches = self.mind.glitches;
        let bounces = std::mem::take(&mut self.mind.bounces);
        self.mind = mind::Mind::new(self.mind.skill, self.seed);
        self.mind.experience.enter(&self.map);
        self.mind.emote.taunt = taunt;
        self.mind.glitches = glitches;
        self.mind.bounces = bounces;
        self.retune();
        self.controller = human_input::Controller::new(self.seed);
        self.discard_observations(false);
        self.sampled = None;
        self.pending_kills.clear();
        self.visibility.clear_life();
        self.player_unit = None;
    }

    pub fn new(skill: mind::Skill, seed: u64) -> Self {
        Brain {
            mind: mind::Mind::new(skill, seed),
            seed,
            epoch: radio::lock(radio::host()).epoch,
            kills_seen: emote::kills().since(u64::MAX).1,
            fidget: mind::pad::Fidget::new(seed),
            map: String::new(),
            tuning: None,
            controller: human_input::Controller::new(seed),
            observations: VecDeque::new(),
            spare_observations: Vec::new(),
            sampled: None,
            pending_kills: Vec::new(),
            visibility: Default::default(),
            player_unit: None,
            vehicle_headers: Vec::new(),
            vehicle_seats: Default::default(),
        }
    }

    /// The brain with `t` over its skill: the aim table and awareness while
    /// it thinks, and the personality now and in every new match's mind.
    pub fn with_tuning(mut self, t: tuning::Tuning) -> Self {
        if t.is_empty() {
            self.tuning = None;
            return self;
        }
        self.tuning = Some(t);
        self.retune();
        self
    }

    pub fn tuning(&self) -> Option<tuning::Tuning> {
        self.tuning
    }

    fn retune(&mut self) {
        if let Some(t) = self.tuning {
            let p = &mut self.mind.decisions.personality;
            *p = t.personality(*p);
        }
    }

    #[cfg(test)]
    fn play(
        &mut self,
        p: &brain::Perception,
        map: Option<&cache::MapData>,
        kills: &[emote::Kill],
        channel: &std::sync::Mutex<radio::Radio>,
    ) -> HostPadState {
        self.sample(
            Observation {
                perception: p.clone(),
                kills: kills.to_vec(),
                ..Observation::default()
            },
            map,
            channel,
        )
    }

    fn discard_observations(&mut self, keep_kills: bool) {
        let mut newer = Vec::new();
        if keep_kills {
            std::mem::swap(&mut newer, &mut self.pending_kills);
        }
        while let Some(mut observation) = self.observations.pop_front() {
            if keep_kills {
                self.pending_kills.append(&mut observation.kills);
            }
            self.spare_observations.push(observation);
        }
        if keep_kills {
            self.pending_kills.append(&mut newer);
        }
    }

    fn sample(
        &mut self,
        mut observation: Observation,
        map: Option<&cache::MapData>,
        channel: &std::sync::Mutex<radio::Radio>,
    ) -> HostPadState {
        let p = &observation.perception;
        let tick = p.tick;
        let alive = p.me.is_some();
        let epoch = {
            let mut board = radio::lock(channel);
            if p.observations.player.is_some() {
                board.advance(tick);
                if board.epoch == self.epoch {
                    board.forget_deaths(&observation.kills);
                }
            }
            board.listen_into(p, &mut observation.inbox);
            board.supply_calls_into(p, &mut observation.supply_calls);
            board.epoch
        };
        if self.epoch != epoch {
            let mut experience = std::mem::take(&mut self.mind.experience);
            experience.new_life();
            self.reset_mind();
            self.mind.experience = experience;
            self.epoch = epoch;
            observation.kills.clear();
        }

        if self.sampled == Some((tick, alive)) {
            self.pending_kills.append(&mut observation.kills);
            self.spare_observations.push(observation);
            return self.controller.report();
        }
        if self
            .sampled
            .is_some_and(|(_, was_alive)| was_alive != alive)
        {
            // Visual samples are for the previous life. Preserve their
            // kill feed in order so a quick death cannot swallow a kill.
            self.discard_observations(true);
            self.controller.discard_pending();
        }
        self.sampled = Some((tick, alive));
        if !self.pending_kills.is_empty() {
            self.pending_kills.append(&mut observation.kills);
            std::mem::swap(&mut self.pending_kills, &mut observation.kills);
        }
        let limits = self
            .tuning
            .unwrap_or_default()
            .input_limits(self.mind.skill);
        let intent =
            if alive {
                self.observations.push_back(observation);
                let mut ready = None::<Observation>;
                while self.observations.front().is_some_and(|o| {
                    tick.wrapping_sub(o.perception.tick) >= limits.perception_ticks
                }) {
                    let mut next = self.observations.pop_front().unwrap();
                    if let Some(mut earlier) = ready.take() {
                        earlier.kills.append(&mut next.kills);
                        std::mem::swap(&mut earlier.kills, &mut next.kills);
                        self.spare_observations.push(earlier);
                    }
                    ready = Some(next);
                }
                ready.map_or_else(HostPadState::default, |o| self.think(o, map, channel, tick))
            } else {
                self.think(observation, map, channel, tick)
            };
        self.controller.step(tick, intent, limits)
    }

    fn think(
        &mut self,
        mut observation: Observation,
        map: Option<&cache::MapData>,
        channel: &std::sync::Mutex<radio::Radio>,
        tick: u32,
    ) -> HostPadState {
        let p = &observation.perception;
        self.mind.hear_deaths(p, &observation.kills, map);
        self.mind
            .decisions
            .hear_supplies(&observation.supply_calls, p.tick);
        if let Some((id, _)) = p.observations.player {
            let weight = self.mind.decisions.behavior().taunt_weight();
            self.mind
                .emote
                .hear_with_style(&observation.kills, Some(id), weight);
        }
        std::mem::swap(&mut self.mind.decisions.inbox, &mut observation.inbox);
        std::mem::swap(&mut self.mind.vehicles.sight, &mut observation.vehicles);
        let pad = tuning::with(self.tuning, || {
            self.mind.step_observed(
                p,
                map,
                &observation.sounds,
                &observation.grenades,
                observation.visible_players,
            )
        });
        let reports = self.mind.decisions.supply_reports(p);
        if self.mind.decisions.report.is_some() || !reports.is_empty() {
            let mut board = radio::lock(channel);
            if board.epoch == self.epoch {
                if let Some(mut call) = self.mind.decisions.report {
                    call.tick = tick;
                    board.publish(call);
                }
                for call in reports {
                    board.publish_supply(call, tick);
                }
            }
        }
        self.spare_observations.push(observation);
        pad
    }
}

impl Default for Brain {
    fn default() -> Self {
        Brain::new(mind::Skill::default(), 0x5eed)
    }
}

impl local::PadSource for Brain {
    fn brain_view(&self) -> Option<BrainView> {
        Some(self.view())
    }

    fn reset_input(&mut self) {
        if let Some((tick, _)) = self.sampled.take() {
            self.mind.release(tick);
            self.discard_observations(false);
            self.pending_kills.clear();
            self.controller.reset();
        }
        self.player_unit = None;
        self.visibility.clear_life();
    }

    fn phantom_latency(&self) -> tuning::PhantomLatency {
        self.tuning.unwrap_or_default().phantom_latency()
    }

    fn pad(&mut self, f: &local::Frame<'_>) -> HostPadState {
        if f.joining {
            self.reset_input();
            self.fidget.reset();
            return HostPadState::default();
        }
        let who = match f.machine {
            Some(id) => guest::Who::Machine(id),
            None => guest::Who::Port(u32::from(f.port)),
        };
        let mut observation = self.spare_observations.pop().unwrap_or_default();
        let cursor = emote::kills().since_into(self.kills_seen, &mut observation.kills);
        let name_change = autonav::loaded_change(&self.map);
        let map = cache::current();
        // Render can poll a local pad repeatedly within one game tick.
        // Keep processing fresh kill events and same-tick life transitions.
        if observation.kills.is_empty()
            && name_change.is_none()
            && f.tick
                .is_some_and(|tick| self.sampled == Some((tick, true)))
            && self.visibility.matches_map(map.as_ref())
            && self
                .player_unit
                .is_some_and(|(slot, unit)| guest::same_live_unit(f.guest, who, slot, unit))
        {
            self.kills_seen = cursor;
            self.spare_observations.push(observation);
            return self.controller.report();
        }
        self.vehicle_headers.clear();
        if guest::perception_into(
            f.guest,
            who,
            true,
            &mut observation.perception,
            &mut observation.grenades,
            Some(&mut self.vehicle_headers),
        )
        .is_err()
        {
            self.spare_observations.push(observation);
            self.reset_input();
            self.fidget.reset();
            return HostPadState::default();
        }
        if let Some(name) = name_change {
            self.mind.bounces = glitches::bounces(&name);
            self.mind.experience.enter(&name);
            self.map = name;
        }
        self.kills_seen = cursor;
        let p = &mut observation.perception;
        let alive = p.me.is_some();
        let unit = p.observations.player.and_then(|(id, _)| {
            p.observations
                .supply_players
                .iter()
                .find(|p| p.id == id)
                .map(|p| (id, p.unit))
        });
        if let (Some(old), Some(new)) = (self.player_unit, unit)
            && old != new
        {
            // A replacement biped can arrive without a dead sample.
            self.discard_observations(true);
            self.mind.release(p.tick);
            self.controller.discard_pending();
            self.sampled = None;
        }
        self.player_unit = unit;
        if self.visibility.bind(map.as_ref(), unit, p.tick) {
            self.mind.invalidate_routes();
            // Observations filtered against old geometry must not guide a
            // pad after the BSP has changed. Keep their kill feed in order.
            self.discard_observations(true);
        }
        // Vehicles are extra: a fault reading them leaves the bot on foot.
        if alive {
            let headers = p
                .observations
                .pickups_scanned
                .then_some(self.vehicle_headers.as_slice());
            vehicle::read::sight_in_game_into(
                f.guest,
                who,
                &mut observation.vehicles,
                headers,
                &mut self.vehicle_seats,
            );
            sound::recent_into(p.tick, &mut observation.sounds);
        } else {
            self.vehicle_seats.clear_sight(&mut observation.vehicles);
            observation.sounds.clear();
        }
        if let Some(loaded) = &map
            && observation
                .vehicles
                .riding
                .is_some_and(|r| r.role == vehicle::SeatRole::Driver)
        {
            self.mind.vehicles.paths.bind_map(loaded);
        }
        observation.visible_players = Some(perceive::observe_cached(
            p,
            &mut observation.vehicles,
            &mut observation.kills,
            map.as_deref(),
            &mut self.visibility,
            self.tuning.unwrap_or_default().work.player_los_ticks,
            &mut self.vehicle_seats,
        ));
        if map.is_none() {
            observation.sounds.clear();
            observation.grenades.clear();
        }
        let pad = self.sample(observation, map.as_deref(), radio::host());
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
    fn a_tuned_brain_keeps_its_personality_across_matches() {
        let t = tuning::Tuning {
            aggression: Some(0.95),
            teamwork: Some(0.05),
            ..tuning::Tuning::default()
        };
        let seeded = Brain::new(mind::Skill::Heroic, 11)
            .mind
            .decisions
            .personality;
        let mut b = Brain::new(mind::Skill::Heroic, 11).with_tuning(t);
        let p = b.mind.decisions.personality;
        assert_eq!((p.aggression, p.teamwork), (0.95, 0.05));
        assert_eq!(p.caution, seeded.caution);
        // A new match (the radio's epoch moves on): a fresh mind, the
        // tuning put back over its seeded personality.
        b.epoch = b.epoch.wrapping_add(7);
        let board = std::sync::Mutex::new(radio::Radio::default());
        b.play(&brain::Perception::default(), None, &[], &board);
        let p = b.mind.decisions.personality;
        assert_eq!(
            (p.aggression, p.teamwork, p.caution),
            (0.95, 0.05, seeded.caution)
        );
        // The scope ended with the think.
        assert!(tuning::current().is_none());
        // An empty tuning is none.
        assert!(
            Brain::default()
                .with_tuning(tuning::Tuning::default())
                .tuning()
                .is_none()
        );
    }

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

    #[test]
    fn temperament_survives_a_round_reset_while_mood_clears() {
        let board = std::sync::Mutex::new(radio::Radio::default());
        let mut bot = Brain::new(mind::Skill::Easy, 42);
        bot.epoch = radio::lock(&board).epoch;
        let first = bot.view().personality;
        let mut p = brain::Perception {
            observations: brain::Observations {
                player: Some((1, 0)),
                ..Default::default()
            },
            tick: 25,
            ..Default::default()
        };
        bot.play(
            &p,
            None,
            &[emote::Kill {
                seq: 1,
                tick: p.tick,
                victim: 1,
                attacker: Some(2),
                betrayal: false,
                at: None,
            }],
            &board,
        );
        assert!(bot.view().mood.tilt > 0.0);
        bot.set_skill(mind::Skill::Legendary);
        let legendary = bot.view().personality;
        assert_ne!(legendary, first);
        assert_eq!(legendary.favorite_weapon, first.favorite_weapon);
        assert!(bot.view().mood.tilt > 0.0);
        radio::lock(&board).reset();
        p.tick = 1;
        bot.play(&p, None, &[], &board);
        assert_eq!(bot.view().mood, decisions::MoodView::default());
        assert_eq!(bot.view().personality, legendary);
    }
}
