//! A fake [`Game`] for tests, `h2 rules run` and Live's workbench: plain
//! tables in, a log of the calls made out. [`FakeGame::apply`] is what the
//! engine does when it reports an event, so every scenario runner shares
//! one model of the game.

use std::collections::{BTreeMap, BTreeSet};

use crate::caps::id;
use crate::vm::{Arg, Call, Game, Input, Placed, ScriptError};

/// One player of the fake game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakePlayer {
    pub team: u8,
    pub alive: bool,
    pub score: i32,
    pub lives: i32,
    /// What `kills(p)` and `deaths(p)` read.
    pub kills: i32,
    pub deaths: i32,
}

/// A game held in tables. Actions it can model (score, team, lives,
/// respawn, objective registration) change the tables as the engine
/// would; every action and the script-error end are logged in call order.
#[derive(Clone, Debug, Default)]
pub struct FakeGame {
    pub players: BTreeMap<u8, FakePlayer>,
    pub teams: Vec<u8>,
    pub labelled: BTreeMap<String, Vec<Placed>>,
    /// Retail record fields a scenario sets; the rest read
    /// [`stock_variant`] of [`FakeGame::carrier`].
    pub variant: BTreeMap<String, i32>,
    /// The script's carrier engine index, for the stock record's fields.
    pub carrier: u8,
    pub round_time_left: i32,
    pub squad_alive: [i32; 16],
    /// The objective registry: object to its carrying player.
    pub objectives: BTreeMap<u32, Option<u8>>,
    /// Objects' teams: an objective's, or a flag placement's designator.
    pub object_teams: BTreeMap<u32, u8>,
    /// Labelled objects' label numbers.
    pub numbers: BTreeMap<u32, i32>,
    /// Players' unit objects: what `player_of` reads, and how `enter` and
    /// `leave` move a player in and out of a zone.
    pub units: BTreeMap<u32, u8>,
    /// `(player, zone object)` pairs: a player whose unit is inside the
    /// zone. `inside` and the zone test read the living ones.
    pub inside: BTreeSet<(u8, u32)>,
    /// Actions made, as `(entry name, args)`.
    pub calls: Vec<(&'static str, Vec<Arg>)>,
    /// Reads made, by entry name.
    pub reads: Vec<&'static str>,
    pub script_error: Option<ScriptError>,
}

/// The fields of the stock record of engine `carrier` (the game's
/// defaults writer, `hopper::variant::VariantRecord::stock` in
/// `live-proto`) that a script reads by name. A field it leaves out reads
/// 0, and an index that is no carrier has none.
pub fn stock_variant(carrier: u8) -> BTreeMap<String, i32> {
    if !crate::caps::CARRIERS.iter().any(|&(_, i)| i == carrier) {
        return BTreeMap::new();
    }
    // Team play by type: CTF, Assault and Territories.
    let team = matches!(carrier, 1 | 8 | 9);
    let score = match carrier {
        2 => 25,
        3 | 4 => 120,
        7 => 15,
        1 | 9 => 3,
        8 => 300,
        _ => 0,
    };
    let mut out: BTreeMap<String, i32> = [
        ("team_play", i32::from(team)),
        ("score_limit", score),
        ("time_limit", 480),
        ("lives", 0),
        ("respawn_time", if team { 10 } else { 5 }),
        ("rounds", 0),
        ("engine", i32::from(carrier)),
    ]
    .into_iter()
    .map(|(k, v)| (k.to_string(), v))
    .collect();
    let area: &[(&str, i32)] = match carrier {
        3 => &[("oddball_ball_count", 1)],
        4 => &[("king_moving_hill", 60)],
        1 => &[("ctf_reset_time", 30)],
        9 => &[("ctf_reset_time", 30), ("assault_arm_time", 5)],
        8 => &[("territories_count", 3), ("territories_contest_time", 5)],
        _ => &[],
    };
    out.extend(area.iter().map(|&(k, v)| (k.to_string(), v)));
    out
}

impl FakeGame {
    /// `n` living players in slots 0.., all on team 0.
    pub fn with_players(n: u8) -> FakeGame {
        let mut g = FakeGame {
            teams: vec![0, 1],
            ..FakeGame::default()
        };
        for slot in 0..n {
            g.players.insert(
                slot,
                FakePlayer {
                    alive: true,
                    ..FakePlayer::default()
                },
            );
        }
        g
    }

    /// The actions made with entry `name`, in order.
    pub fn calls_to(&self, name: &str) -> Vec<&Vec<Arg>> {
        self.calls
            .iter()
            .filter(|(n, _)| *n == name)
            .map(|(_, a)| a)
            .collect()
    }

    /// Declares a labelled object: its label, number and placement. Its
    /// team, when it has one, goes in [`FakeGame::object_teams`].
    pub fn label(&mut self, label: &str, object: u32, number: i32, placement: u32) {
        self.labelled
            .entry(label.to_string())
            .or_default()
            .push(Placed { placement, object });
        self.numbers.insert(object, number);
    }

    /// A new tick begins: the round clock counts down one, to 0.
    pub fn begin_tick(&mut self, tick: u32) {
        if tick > 0 {
            self.round_time_left = (self.round_time_left - 1).max(0);
        }
    }

    /// A dead or departed player stands in no zone and carries nothing.
    fn down(&mut self, p: u8) {
        self.inside.retain(|&(q, _)| q != p);
        for carrier in self.objectives.values_mut() {
            if *carrier == Some(p) {
                *carrier = None;
            }
        }
    }

    /// What the engine does when it reports `input`, before the handler
    /// runs: a kill marks its victim dead and counts the victim's death and
    /// the killer's kill (a suicide counts no kill), a death marks the
    /// player dead (either takes the player out of every zone and drops
    /// what they carried), a spawn alive, a join seats a living player on
    /// team 0, an `enter` or `leave` of a player's unit moves the player
    /// in or out of the zone, and a `pickup` or `drop` of a registered
    /// objective sets or clears its carrier. [`FakeGame::after`] removes a
    /// player who quit.
    pub fn apply(&mut self, input: &Input) {
        match *input {
            Input::Kill { victim, killer } => {
                if let Some(v) = self.players.get_mut(&victim) {
                    v.alive = false;
                    v.deaths = v.deaths.saturating_add(1);
                }
                if let Some(k) = killer
                    && k != victim
                    && let Some(k) = self.players.get_mut(&k)
                {
                    k.kills = k.kills.saturating_add(1);
                }
                self.down(victim);
            }
            Input::Death(p) => {
                if let Some(v) = self.players.get_mut(&p) {
                    v.alive = false;
                }
                self.down(p);
            }
            Input::Pickup(p, o) => {
                if let Some(carrier) = self.objectives.get_mut(&o) {
                    *carrier = Some(p);
                }
            }
            Input::Drop(p, o) => {
                if let Some(carrier) = self.objectives.get_mut(&o)
                    && *carrier == Some(p)
                {
                    *carrier = None;
                }
            }
            Input::Spawn(p) => {
                if let Some(v) = self.players.get_mut(&p) {
                    v.alive = true;
                }
            }
            Input::Join(p) => {
                self.players.entry(p).or_insert(FakePlayer {
                    alive: true,
                    ..FakePlayer::default()
                });
            }
            Input::Enter { zone, object } => {
                if let Some(&p) = self.units.get(&object) {
                    self.inside.insert((p, zone));
                }
            }
            Input::Leave { zone, object } => {
                if let Some(&p) = self.units.get(&object) {
                    self.inside.remove(&(p, zone));
                }
            }
            _ => {}
        }
    }

    /// What the engine does after the handler of `input`: a quit removes
    /// the player and its unit.
    pub fn after(&mut self, input: &Input) {
        if let Input::Quit(p) = *input {
            self.players.remove(&p);
            self.units.retain(|_, q| *q != p);
            self.down(p);
        }
    }

    fn player(&self, arg: Option<&Arg>) -> Option<&FakePlayer> {
        match arg {
            Some(Arg::Player(Some(p))) => self.players.get(p),
            _ => None,
        }
    }

    /// Whether player `p` is alive and inside `zone`.
    fn living_inside(&self, p: u8, zone: u32) -> bool {
        self.inside.contains(&(p, zone)) && self.players.get(&p).is_some_and(|q| q.alive)
    }
}

impl Game for FakeGame {
    fn players(&self) -> Vec<u8> {
        // Reverse order, so the VM's own ordering is what tests see.
        self.players.keys().rev().copied().collect()
    }

    fn teams(&self) -> Vec<u8> {
        self.teams.iter().rev().copied().collect()
    }

    fn labelled(&self, label: &str) -> Vec<Placed> {
        let mut list = self.labelled.get(label).cloned().unwrap_or_default();
        list.reverse();
        list
    }

    fn objectives(&self) -> Vec<u32> {
        self.objectives.keys().rev().copied().collect()
    }

    fn variant(&self, field: &str) -> i32 {
        self.variant
            .get(field)
            .copied()
            .or_else(|| stock_variant(self.carrier).get(field).copied())
            .unwrap_or(0)
    }

    fn zone_holder(&self, zone: u32) -> Option<u8> {
        let mut holder = None;
        for (&p, player) in &self.players {
            if !self.living_inside(p, zone) {
                continue;
            }
            match holder {
                None => holder = Some(player.team),
                Some(t) if t == player.team => {}
                Some(_) => return None,
            }
        }
        holder
    }

    fn read(&mut self, call: &Call) -> i32 {
        self.reads.push(call.entry.name);
        let p = self.player(call.args.first());
        let object = |i: usize| match call.args.get(i) {
            Some(Arg::Object(Some(o))) => Some(*o),
            _ => None,
        };
        match call.entry.id {
            id::IS_OBJECTIVE => object(0).is_some_and(|o| self.objectives.contains_key(&o)) as i32,
            id::CARRIER_OF => object(0)
                .and_then(|o| self.objectives.get(&o).copied().flatten())
                .map_or(-1, i32::from),
            id::OBJECT_TEAM => object(0)
                .and_then(|o| self.object_teams.get(&o))
                .map_or(-1, |&t| i32::from(t)),
            id::PLAYER_OF => object(0)
                .and_then(|o| self.units.get(&o))
                .map_or(-1, |&p| i32::from(p)),
            id::LABEL_NUMBER => object(0).and_then(|o| self.numbers.get(&o).copied()).unwrap_or(0),
            id::INSIDE => match (call.args.first(), object(1)) {
                (Some(Arg::Player(Some(p))), Some(z)) => self.living_inside(*p, z) as i32,
                _ => 0,
            },
            id::PLAYER_SCORE => p.map_or(0, |p| p.score),
            id::ALIVE => p.is_some_and(|p| p.alive) as i32,
            id::TEAM_OF => p.map_or(-1, |p| p.team as i32),
            id::LIVES => p.map_or(0, |p| p.lives),
            id::KILLS => p.map_or(0, |p| p.kills),
            id::DEATHS => p.map_or(0, |p| p.deaths),
            id::ROUND_TIME_LEFT => self.round_time_left,
            id::SQUAD_ALIVE => match call.args.first() {
                Some(Arg::Squad(s)) => self.squad_alive.get(*s as usize).copied().unwrap_or(0),
                _ => 0,
            },
            id::TEAM_SCORE => match call.args.first() {
                Some(Arg::Team(Some(t))) => self
                    .players
                    .values()
                    .filter(|p| p.team == *t)
                    .map(|p| p.score)
                    .sum(),
                _ => 0,
            },
            _ => 0,
        }
    }

    fn act(&mut self, call: &Call) {
        self.calls.push((call.entry.name, call.args.clone()));
        if call.entry.id == id::REGISTER_OBJECTIVE
            && let Some(Arg::Object(Some(o))) = call.args.first()
        {
            self.objectives.insert(*o, None);
            if let Some(Arg::Team(Some(t))) = call.args.get(1) {
                self.object_teams.insert(*o, *t);
            }
            return;
        }
        let slot = match call.args.first() {
            Some(Arg::Player(Some(p))) => Some(*p),
            _ => None,
        };
        let Some(player) = slot.and_then(|s| self.players.get_mut(&s)) else {
            return;
        };
        match (call.entry.id, call.args.get(1)) {
            (id::SCORE, Some(Arg::Int(n))) => player.score = player.score.saturating_add(*n),
            (id::SET_TEAM, Some(Arg::Team(Some(t)))) => player.team = *t,
            (id::SET_LIVES, Some(Arg::Int(n))) => player.lives = *n,
            (id::RESPAWN, _) => player.alive = true,
            _ => {}
        }
    }

    fn script_error(&mut self, error: &ScriptError) {
        self.script_error = Some(*error);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_kill_counts_and_a_spawn_revives() {
        let mut g = FakeGame::with_players(2);
        g.apply(&Input::Kill { victim: 1, killer: Some(0) });
        assert!(!g.players[&1].alive);
        assert_eq!((g.players[&0].kills, g.players[&1].deaths), (1, 1));
        g.apply(&Input::Kill { victim: 0, killer: Some(0) });
        assert_eq!(g.players[&0].kills, 1, "a suicide counts no kill");
        g.apply(&Input::Spawn(1));
        assert!(g.players[&1].alive);
        g.apply(&Input::Join(4));
        assert!(g.players[&4].alive);
        g.after(&Input::Quit(4));
        assert!(!g.players.contains_key(&4));
    }

    #[test]
    fn an_objective_is_carried_until_dropped_or_its_carrier_dies() {
        let mut g = FakeGame::with_players(2);
        g.objectives.insert(9, None);
        g.apply(&Input::Pickup(0, 9));
        assert_eq!(g.objectives[&9], Some(0));
        // Another player's drop is not this one's.
        g.apply(&Input::Drop(1, 9));
        assert_eq!(g.objectives[&9], Some(0));
        g.apply(&Input::Drop(0, 9));
        assert_eq!(g.objectives[&9], None);
        g.apply(&Input::Pickup(1, 9));
        g.apply(&Input::Kill { victim: 1, killer: Some(0) });
        assert_eq!(g.objectives[&9], None);
        // An object that is no objective has no carrier.
        g.apply(&Input::Pickup(0, 77));
        assert!(!g.objectives.contains_key(&77));
    }

    #[test]
    fn the_dead_leave_their_zones() {
        let mut g = FakeGame::with_players(1);
        g.units.insert(100, 0);
        g.apply(&Input::Enter { zone: 7, object: 100 });
        g.apply(&Input::Kill { victim: 0, killer: None });
        g.apply(&Input::Spawn(0));
        assert_eq!(g.zone_holder(7), None, "a respawn is not where the death was");
    }

    #[test]
    fn the_zone_test_counts_living_units_of_one_team() {
        let mut g = FakeGame::with_players(3);
        g.players.get_mut(&2).unwrap().team = 1;
        for p in 0..3u8 {
            g.units.insert(100 + u32::from(p), p);
        }
        assert_eq!(g.zone_holder(7), None);
        g.apply(&Input::Enter { zone: 7, object: 100 });
        g.apply(&Input::Enter { zone: 7, object: 101 });
        assert_eq!(g.zone_holder(7), Some(0));
        g.apply(&Input::Enter { zone: 7, object: 102 });
        assert_eq!(g.zone_holder(7), None, "contested");
        g.apply(&Input::Kill { victim: 2, killer: Some(0) });
        assert_eq!(g.zone_holder(7), Some(0), "the dead do not hold");
        g.apply(&Input::Leave { zone: 7, object: 100 });
        g.apply(&Input::Leave { zone: 7, object: 101 });
        assert_eq!(g.zone_holder(7), None);
    }

    #[test]
    fn variant_fields_read_the_scenario_then_the_stock_record() {
        let mut g = FakeGame {
            carrier: 1,
            ..FakeGame::default()
        };
        assert_eq!(g.variant("team_play"), 1);
        assert_eq!(g.variant("score_limit"), 3);
        assert_eq!(g.variant("engine"), 1);
        g.variant.insert("score_limit".into(), 5);
        assert_eq!(g.variant("score_limit"), 5);
        g.carrier = 2;
        assert_eq!(g.variant("team_play"), 0);
        assert_eq!(g.variant("friendly_fire"), 0);
    }

    #[test]
    fn the_round_clock_counts_down_to_zero() {
        let mut g = FakeGame {
            round_time_left: 2,
            ..FakeGame::default()
        };
        g.begin_tick(0);
        assert_eq!(g.round_time_left, 2);
        for t in 1..5 {
            g.begin_tick(t);
        }
        assert_eq!(g.round_time_left, 0);
    }
}
