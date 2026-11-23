//! The Firefight director's interface: the [`Settings`] a match starts
//! with, the [`Observed`] the host feeds the script each tick, the
//! [`Command`]s it answers with, and the [`State`] the mode blob is made
//! of. The director itself is [`crate::script::ScriptDirector`]: the
//! definition compiled to a rules script and run on the rules VM. It reads
//! no clock, touches no game memory and draws only from its own generator,
//! so the same definition, settings and observed sequence always give the
//! same commands.
//! See `docs/game/firefight.md`, "The director as a rules script".

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::scoring::ScoreReason;
use crate::{Difficulty, MAX_TEAMS, Skull};

/// Squad slots the kit provides on every map.
pub const SQUAD_SLOTS: usize = 16;
/// Squads placed in one step at most.
pub const PLACE_PER_TICK: usize = 2;
/// Live actors at or under which a wave enters cleanup.
pub const CLEANUP_ACTORS: u16 = 3;
/// Ticks of cleanup before the director erases what is left (60 seconds).
pub const CLEANUP_TICKS: u32 = 60 * 30;
/// Ticks a placed squad may go unobserved, or an erased one may stay
/// observed, before the director treats it as gone (3 seconds).
pub const SLOT_TIMEOUT_TICKS: u32 = 3 * 30;

/// Match options chosen in Game Setup or by the hopper.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    /// The match seed from the session data.
    pub seed: u64,
    pub difficulty: Difficulty,
    /// Skulls on from the start, beside the definition's round schedule.
    pub skulls: BTreeSet<Skull>,
}

/// What the host saw this tick, after the game tick.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Observed {
    /// The players in the match by roster slot. Order does not matter.
    pub players: Vec<PlayerSeen>,
    /// Live actors in each kit squad slot.
    pub actors_alive_by_squad: [u16; SQUAD_SLOTS],
    /// The kit's drop zones in kit order.
    pub zones: Vec<ZoneSeen>,
    /// Actors killed this tick (`SimEvent::ActorKilled`), in event order.
    pub kills: Vec<Kill>,
    /// Players killed by players this tick, in event order. Only a kill
    /// across the two teams scores ([`crate::Scoring::player_kill`]).
    pub player_kills: Vec<PlayerKill>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerSeen {
    pub slot: u8,
    pub alive: bool,
    /// The player's game team. With two Firefight teams, team 0 is the
    /// first and every other team the second; with one, it is ignored.
    pub team: u8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ZoneSeen {
    /// Some player has line of sight to the zone (the AI's visibility test).
    pub seen: bool,
    /// Distance from the players' centre in any fixed integer unit.
    pub distance: u32,
    /// Distance from each Firefight team's players' centre, in the same
    /// unit; `u32::MAX` for a team with no unit. Read with two teams only.
    pub team_distance: [u32; MAX_TEAMS],
}

/// A player killed by a player (`SimEvent` kill of a player's unit).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlayerKill {
    /// The credited player's roster slot; none for a fall or the AI.
    pub killer: Option<u8>,
    pub victim: u8,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Kill {
    /// The kit class of the killed actor's character.
    pub class: String,
    /// The credited player's roster slot.
    pub killer: Option<u8>,
    /// A second player who damaged the actor in its last five seconds.
    pub assist: Option<u8>,
}

/// A director command; the host log carries each as `Entry<Command>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Command {
    PlaceSquad {
        slot: u8,
        class: String,
        count: u16,
        zone: u8,
        weapon_tier: u8,
    },
    /// With two teams, right after a `PlaceSquad`: the Firefight team the
    /// squad is sent at. Its cleanup `Hunt` goes to that team's base. With
    /// one team there is no `Target` and every squad's target is team 0.
    Target {
        slot: u8,
        team: u8,
    },
    /// Send the squad at the players (cleanup): at its target's base.
    Hunt {
        slot: u8,
    },
    EraseSquad {
        slot: u8,
    },
    /// The zero-based round starts.
    Round {
        index: u8,
    },
    /// The zero-based wave of the current round starts.
    Wave {
        index: u8,
    },
    Skull {
        id: Skull,
        on: bool,
    },
    /// The shared pool (one team), or one player's count when lives are
    /// not shared.
    Lives {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        player: Option<u8>,
        pool: u16,
    },
    /// A team's shared pool, with two teams.
    TeamLives {
        team: u8,
        pool: u16,
    },
    Score {
        player: u8,
        points: u32,
        reason: ScoreReason,
    },
    Respawn {
        player: u8,
    },
    Finish {
        outcome: Outcome,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The last round's boss wave was cleared (one team).
    Won,
    /// Every player was dead with no lives left (one team, or the only
    /// team present of two).
    Lost,
    /// Two teams: the other team ran out of lives, or this team led when
    /// the last round was cleared.
    TeamWon { team: u8 },
    /// Two teams: level on score and on lives left at the end.
    Draw,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    /// Before the first wave, until the tick given.
    Warmup {
        until: u32,
    },
    /// Squads of the wave are still in the reserve.
    Spawning,
    /// Every squad is placed and enough actors are alive.
    Fighting,
    /// The last few actors hunt the players; the rest is erased at the deadline.
    Cleanup {
        since: u32,
    },
    /// Between waves or rounds, until the tick given.
    Intermission {
        until: u32,
    },
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Slot {
    Free,
    /// A placed squad. `alive` is the count until the host first reports
    /// actors, then the lowest count reported.
    Placed {
        count: u16,
        alive: u16,
        seen: bool,
        placed_at: u32,
        /// The Firefight team the squad is sent at (0 with one team).
        target: u8,
    },
    /// Erased; free once the host reports it empty or the timeout passes.
    Draining {
        since: u32,
    },
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlayerState {
    /// The player's own lives when lives are not shared.
    pub lives: u16,
    /// A respawn was issued and the player has not been seen alive since.
    pub respawning: bool,
    /// The player's Firefight team, fixed when the director first sees
    /// the player (0 with one team).
    pub team: u8,
}

/// The director's whole state: the mode blob.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct State {
    pub phase: Phase,
    pub round: u8,
    pub wave: u8,
    /// The next squad of the wave to place; the rest is the reserve.
    pub next_squad: u8,
    /// Actors placed so far in this wave.
    pub wave_actors: u16,
    pub slots: [Slot; SQUAD_SLOTS],
    /// Each team's shared pool (unused when lives are not shared). With
    /// one team the second stays 0.
    pub pools: [u16; MAX_TEAMS],
    /// Each team's points: the sum of its players' `Score` commands.
    pub scores: [u32; MAX_TEAMS],
    pub players: BTreeMap<u8, PlayerState>,
    pub skulls: BTreeSet<Skull>,
    pub rng: u64,
    pub started: bool,
}

/// The Firefight team of a game team: 0 with one team; with two, game
/// team 0 is the first and every other team the second.
pub fn firefight_team(teams: u8, game_team: u8) -> u8 {
    u8::from(teams > 1 && game_team != 0)
}
