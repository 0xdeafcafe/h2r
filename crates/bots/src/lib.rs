//! Multiplayer bots, "Clankers" (docs/game/bots.md).
//!
//! One brain, one way in. A bot is driven by a [`pad_source::PadSource`]: the
//! brain ([`Brain`]), a script or a recording, each giving controller input
//! and nothing else. Every bot plays as a phantom peer ([`phantom`]): a
//! remote console's player in a session this console hosts, never a local
//! controller seat, which belongs to a real person. The boot glue
//! (`boot::bot_glue`) places the hooks.

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
pub mod los;
pub mod mind;
pub mod moves;
pub mod nav;
pub mod objective;
pub mod pad_source;
mod perceive;
pub mod phantom;
pub mod places;
pub mod roam;
mod sound;
pub mod stuck;
pub mod tactics;
pub mod think;
pub mod tuning;
pub mod vehicle;
pub mod vis;
mod visits;
pub mod weapons;
mod work;

use std::{
    collections::VecDeque,
    sync::{Arc, Weak},
};

use decisions::radio;
use input::host::HostPadState;

pub use sound::{clear_shots, observe_shot};

/// The brain as a pad source: in a multiplayer game its [`mind::Mind`] with
/// the loaded map's data ([`cache::current`]).
pub struct Brain {
    pub mind: mind::Mind,
    /// The last death heard from [`emote::kills`].
    kills_seen: u64,
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
    map_data: Option<Weak<cache::MapData>>,
    player_unit: Option<(u32, u32)>,
    vehicle_headers: Vec<vehicle::read::VehicleHeader>,
    vehicle_seats: vehicle::read::SeatBuffers,
    /// The radio it listens and talks on ([`radio::host`]).
    radio: &'static std::sync::Mutex<radio::Radio>,
    /// What the read phase left for the think
    /// ([`pad_source::PadSource::observe`]).
    staged: Option<Staged>,
    /// Calls the last think made, published by `Brain::speak`.
    outbox: Outbox,
    /// Its place in the radio's open batch, from its last listen.
    seat: Option<radio::Seat>,
}

/// One sample between the read phase and the think: the observation, with
/// the radio already heard, and the map it was filtered against.
struct Staged {
    observation: Observation,
    map: Option<Arc<cache::MapData>>,
}

/// The calls a think made, held for the game thread to publish in seat
/// order ([`pad_source::PadSource::conclude`]).
#[derive(Default)]
struct Outbox {
    tick: u32,
    report: Option<radio::Call>,
    supplies: Vec<radio::SupplyCall>,
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

/// Read-only brain state for phantom bot views.
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
        self.player_unit = None;
        self.outbox = Outbox::default();
    }

    pub fn new(skill: mind::Skill, seed: u64) -> Self {
        Brain {
            mind: mind::Mind::new(skill, seed),
            seed,
            epoch: radio::lock(radio::host()).epoch,
            kills_seen: emote::kills().since(u64::MAX).1,
            map: String::new(),
            tuning: None,
            controller: human_input::Controller::new(seed),
            observations: VecDeque::new(),
            spare_observations: Vec::new(),
            sampled: None,
            pending_kills: Vec::new(),
            map_data: None,
            player_unit: None,
            vehicle_headers: Vec::new(),
            vehicle_seats: Default::default(),
            radio: radio::host(),
            staged: None,
            outbox: Outbox::default(),
            seat: None,
        }
    }

    /// The brain on its own radio, for tests that run beside each other.
    #[cfg(test)]
    pub(crate) fn on_radio(mut self, board: &'static std::sync::Mutex<radio::Radio>) -> Self {
        self.radio = board;
        self.epoch = radio::lock(board).epoch;
        self
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

    /// One whole sample in the serial order: listen, think, publish.
    #[cfg(test)]
    fn sample(
        &mut self,
        observation: Observation,
        map: Option<&cache::MapData>,
        channel: &std::sync::Mutex<radio::Radio>,
    ) -> HostPadState {
        let observation = self.listen(observation, channel);
        let pad = self.decide(observation, map);
        self.speak(channel);
        pad
    }

    /// The radio half of the read phase (game thread, seat order): the
    /// board moves on to this tick, this bot's deaths prune it, and the
    /// bot hears its team. A new match gives a fresh mind.
    fn listen(
        &mut self,
        mut observation: Observation,
        channel: &std::sync::Mutex<radio::Radio>,
    ) -> Observation {
        let p = &observation.perception;
        let tick = p.tick;
        let epoch = {
            let mut board = radio::lock(channel);
            let seq = board.take_seat();
            if p.observations.player.is_some() {
                board.advance(tick);
                if board.epoch == self.epoch {
                    board.forget_deaths(&observation.kills);
                    if let Some(seq) = seq {
                        board.note_forget(seq, &observation.kills);
                    }
                }
            }
            board.listen_into(p, &mut observation.inbox);
            board.supply_calls_into(p, &mut observation.supply_calls);
            self.seat = seq.map(|seq| board.seat(seq));
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
        observation
    }

    /// The think (any thread): everything after the radio is heard, up to
    /// the pad. Touches this brain, the map data and thread-local scratch
    /// only; the calls it makes wait in the outbox.
    fn decide(&mut self, mut observation: Observation, map: Option<&cache::MapData>) -> HostPadState {
        let p = &observation.perception;
        let tick = p.tick;
        let alive = p.me.is_some();
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
                ready.map_or_else(HostPadState::default, |o| self.step_mind(o, map, tick))
            } else {
                self.step_mind(observation, map, tick)
            };
        self.controller.step(tick, intent, limits)
    }

    fn step_mind(
        &mut self,
        mut observation: Observation,
        map: Option<&cache::MapData>,
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
        self.outbox.tick = tick;
        self.outbox.report = self.mind.decisions.report.map(|mut call| {
            call.tick = tick;
            call
        });
        self.outbox.supplies = reports;
        self.spare_observations.push(observation);
        pad
    }

    /// Publishes what the last think said (game thread, seat order). From
    /// a batch seat the call goes in as the serial order would have put it.
    fn speak(&mut self, channel: &std::sync::Mutex<radio::Radio>) {
        let seat = self.seat.take();
        let out = std::mem::take(&mut self.outbox);
        if out.report.is_none() && out.supplies.is_empty() {
            return;
        }
        let mut board = radio::lock(channel);
        if board.epoch != self.epoch {
            return;
        }
        match seat {
            None => {
                if let Some(call) = out.report {
                    board.publish(call);
                }
            }
            Some(seat) => {
                if !seat.accepts(self.epoch, out.tick) {
                    return;
                }
                if let Some(call) = out.report {
                    board.publish_seated(seat, call);
                }
            }
        }
        for call in out.supplies {
            board.publish_supply(call, out.tick);
        }
    }
}

impl Default for Brain {
    fn default() -> Self {
        Brain::new(mind::Skill::default(), 0x5eed)
    }
}

impl pad_source::PadSource for Brain {
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
        // A think staged or made for the interrupted stream says nothing.
        if let Some(staged) = self.staged.take() {
            self.spare_observations.push(staged.observation);
        }
        self.outbox = Outbox::default();
        self.seat = None;
    }

    fn phantom_latency(&self) -> tuning::PhantomLatency {
        self.tuning.unwrap_or_default().phantom_latency()
    }

    fn pad(&mut self, f: &pad_source::Frame<'_>) -> HostPadState {
        self.pad_in_map(f, cache::current().as_ref())
    }

    fn observe(&mut self, f: &pad_source::Frame<'_>) -> pad_source::Sampled {
        self.observe_in_map(f, cache::current().as_ref())
    }

    fn think(&mut self) -> HostPadState {
        match self.staged.take() {
            Some(Staged { observation, map }) => self.decide(observation, map.as_deref()),
            None => self.controller.report(),
        }
    }

    fn conclude(&mut self) {
        self.speak(self.radio);
    }
}

impl Brain {
    fn pad_in_map(
        &mut self,
        f: &pad_source::Frame<'_>,
        map: Option<&Arc<cache::MapData>>,
    ) -> HostPadState {
        use pad_source::PadSource;
        match self.observe_in_map(f, map) {
            pad_source::Sampled::Pad(pad) => pad,
            pad_source::Sampled::Think => {
                let pad = self.think();
                self.conclude();
                pad
            }
        }
    }

    /// The read phase (game thread): everything the bot takes from the
    /// guest and from host-wide state (kill feed, shots, the loaded map,
    /// the radio), staged for [`pad_source::PadSource::think`].
    fn observe_in_map(
        &mut self,
        f: &pad_source::Frame<'_>,
        map: Option<&Arc<cache::MapData>>,
    ) -> pad_source::Sampled {
        use pad_source::Sampled;
        let mut observation = self.spare_observations.pop().unwrap_or_default();
        let cursor = emote::kills().since_into(self.kills_seen, &mut observation.kills);
        let name_change = autonav::loaded_change(&self.map);
        let same_map = match (&self.map_data, map) {
            (Some(old), Some(new)) => std::ptr::eq(old.as_ptr(), Arc::as_ptr(new)),
            (None, None) => true,
            _ => false,
        };
        // A same-tick sample is reusable while the unit, map and kill feed agree.
        if observation.kills.is_empty()
            && name_change.is_none()
            && self.sampled == Some((f.tick, true))
            && same_map
            && self
                .player_unit
                .is_some_and(|(slot, unit)| guest::same_live_unit(f.guest, f.machine, slot, unit))
        {
            self.kills_seen = cursor;
            self.spare_observations.push(observation);
            return Sampled::Pad(self.controller.report());
        }
        self.vehicle_headers.clear();
        if guest::perception_into(
            f.guest,
            f.machine,
            true,
            &mut observation.perception,
            &mut observation.grenades,
            Some(&mut self.vehicle_headers),
        )
        .is_err()
        {
            self.spare_observations.push(observation);
            pad_source::PadSource::reset_input(self);
            return Sampled::Pad(HostPadState::default());
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
        if !same_map {
            self.map_data = map.map(Arc::downgrade);
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
                f.machine,
                &mut observation.vehicles,
                headers,
                &mut self.vehicle_seats,
            );
            sound::recent_into(p.tick, &mut observation.sounds);
        } else {
            self.vehicle_seats.clear_sight(&mut observation.vehicles);
            observation.sounds.clear();
        }
        if let Some(loaded) = map
            && observation
                .vehicles
                .riding
                .is_some_and(|r| r.role == vehicle::SeatRole::Driver)
        {
            self.mind.vehicles.paths.bind_map(loaded);
        }
        let map_data = map.map(|m| m.as_ref());
        observation.visible_players = Some(perceive::observe_for_brain(
            p,
            &mut observation.vehicles,
            &mut observation.kills,
            map_data,
            &mut self.vehicle_seats,
        ));
        if map.is_none() {
            observation.sounds.clear();
            observation.grenades.clear();
        }
        let observation = self.listen(observation, self.radio);
        self.staged = Some(Staged {
            observation,
            map: map.cloned(),
        });
        Sampled::Think
    }

    /// The read phase from a fixture instead of the guest: `p` and the
    /// kills heard since the last sample, filtered as the guest read is.
    #[cfg(test)]
    pub(crate) fn observe_fixture(
        &mut self,
        p: brain::Perception,
        kills: &[emote::Kill],
        map: Option<&Arc<cache::MapData>>,
    ) -> pad_source::Sampled {
        let mut observation = self.spare_observations.pop().unwrap_or_default();
        observation.perception = p;
        observation.kills.clear();
        observation.kills.extend_from_slice(kills);
        observation.sounds.clear();
        observation.grenades.clear();
        self.vehicle_seats.clear_sight(&mut observation.vehicles);
        observation.visible_players = Some(perceive::observe_for_brain(
            &mut observation.perception,
            &mut observation.vehicles,
            &mut observation.kills,
            map.map(|m| m.as_ref()),
            &mut self.vehicle_seats,
        ));
        let observation = self.listen(observation, self.radio);
        self.staged = Some(Staged {
            observation,
            map: map.cloned(),
        });
        pad_source::Sampled::Think
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use input::host::HostButton;
    use pad_source::{Frame, PadSource};

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
    fn the_brain_presses_nothing_outside_a_game() {
        let g = engine::testing::FakeGuest::new();
        let mut brain = Brain::default();
        assert_eq!(
            brain.pad(&Frame {
                frame: 0,
                tick: 0,
                machine: [7; 6],
                guest: &g,
            }),
            HostPadState::default()
        );
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
