//! A fake [`Game`] for tests and `h2 rules run`: plain tables in, a log of
//! the calls made out.

use std::collections::BTreeMap;

use crate::caps::id;
use crate::vm::{Arg, Call, Game, Placed, ScriptError};

/// One player of the fake game.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FakePlayer {
    pub team: u8,
    pub alive: bool,
    pub score: i32,
    pub lives: i32,
}

/// A game held in tables. Actions it can model (score, team, lives,
/// respawn) change the tables as the engine would; every action and the
/// script-error end are logged in call order.
#[derive(Clone, Debug, Default)]
pub struct FakeGame {
    pub players: BTreeMap<u8, FakePlayer>,
    pub teams: Vec<u8>,
    pub labelled: BTreeMap<String, Vec<Placed>>,
    pub variant: BTreeMap<String, i32>,
    pub round_time_left: i32,
    pub squad_alive: [i32; 16],
    /// Actions made, as `(entry name, args)`.
    pub calls: Vec<(&'static str, Vec<Arg>)>,
    /// Reads made, by entry name.
    pub reads: Vec<&'static str>,
    pub script_error: Option<ScriptError>,
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
                    team: 0,
                    alive: true,
                    score: 0,
                    lives: 0,
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

    fn player(&self, arg: Option<&Arg>) -> Option<&FakePlayer> {
        match arg {
            Some(Arg::Player(Some(p))) => self.players.get(p),
            _ => None,
        }
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

    fn variant(&self, field: &str) -> i32 {
        self.variant.get(field).copied().unwrap_or(0)
    }

    fn read(&mut self, call: &Call) -> i32 {
        self.reads.push(call.entry.name);
        let p = self.player(call.args.first());
        match call.entry.id {
            id::PLAYER_SCORE => p.map_or(0, |p| p.score),
            id::ALIVE => p.is_some_and(|p| p.alive) as i32,
            id::TEAM_OF => p.map_or(-1, |p| p.team as i32),
            id::LIVES => p.map_or(0, |p| p.lives),
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
