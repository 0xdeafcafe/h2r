//! `firefight.*`: the Firefight runtime of the loaded map
//! (docs/runtime/agent-api.md, "firefight"; docs/game/firefight.md,
//! "Interfaces"). Wire types only. Game methods: `session.methods` lists
//! them where the game registered them (`game::agent_glue`).
//!
//! [`Command`] has the shape of the director's own command
//! (`firefight::Command`, externally tagged, snake_case), so a call names a
//! command the way the host log writes it.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// Method names.
pub mod method {
    /// The runtime: whether it runs and why not, the role, the director,
    /// the log head and the entries the engine refused ([`super::State`];
    /// no params).
    pub const STATE: &str = "firefight.state";
    /// Append one command to the host log, or hold or run the director
    /// ([`super::CommandParams`], [`super::CommandReply`]).
    pub const COMMAND: &str = "firefight.command";
    /// The loaded map's kit, read from its scenario ([`super::Kit`]; no
    /// params).
    pub const KIT: &str = "firefight.kit";

    pub const ALL: &[&str] = &[STATE, COMMAND, KIT];

    /// **Test waves** in a local Forge session: start a wave test, building
    /// the AI ground first when the running map does not hold it, or stop
    /// one ([`super::WavesTestParams`], [`super::WavesTestReply`];
    /// docs/game/forge-firefight.md, "Test waves"). A `forge.*` method the
    /// Forge session registers, beside the Firefight runtime's.
    pub const WAVES_TEST: &str = "forge.waves.test";
}

// ---- firefight.state ----

/// What this machine does with the host log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    /// Runs the director, writes the log and applies it.
    Host,
    /// A synchronous peer or a film's playback: applies what it receives.
    Replica,
    /// A distributed client: keeps the log and applies nothing.
    Client,
}

/// `firefight.state`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct State {
    /// The mode runs on this map.
    pub running: bool,
    /// What runs is a wave test in a Forge session, not a match
    /// (`forge.waves.test`): its `Lives` and `Respawn` are not applied, and
    /// its `Finish` ends the test.
    #[serde(default)]
    pub test: bool,
    /// Why it does not: the feature is off, no game, not a multiplayer
    /// game, or the start's refusal (a map without the kit names it).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refused: Option<String>,
    /// The loaded map, as the runtime saw it at the game's start.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<Role>,
    /// The team the actors are placed on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    /// The director, on the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub director: Option<Director>,
    /// The host log.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log: Option<Log>,
    /// The latest entries (and AI switch) the engine refused, oldest first.
    #[serde(default)]
    pub errors: Vec<Refusal>,
}

/// The director's mode blob, summarised.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Director {
    /// `warmup`, `spawning`, `fighting`, `cleanup`, `intermission` or
    /// `done`.
    pub phase: String,
    /// The tick a warmup or intermission lasts until, or a cleanup began.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase_tick: Option<u32>,
    /// Zero-based.
    pub round: u8,
    pub wave: u8,
    /// Actors of the wave not placed yet, at the current player count.
    pub reserve: u16,
    /// Player teams: 1, or 2 that also fight each other.
    pub teams: u8,
    /// Each team's shared lives pool, by Firefight team (one entry with
    /// one team; unused when lives are not shared).
    pub pools: Vec<u16>,
    /// Each team's points, by Firefight team.
    pub scores: Vec<u32>,
    /// Actors the director counts as alive, and the cap at the current
    /// player count.
    pub live_actors: u16,
    pub cap: u16,
    /// Players the runtime saw after the last tick.
    pub players: u8,
    /// Active skulls, by their definition names.
    pub skulls: Vec<String>,
    /// `firefight.command`'s `director: "hold"` is in force: the director
    /// is not stepped.
    pub held: bool,
}

/// A position in the host log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Log {
    /// The head: the last entry's sequence number and tick (0 at genesis),
    /// and its hash in hex.
    pub seq: u32,
    pub tick: u32,
    pub hash: String,
    /// The sequence number of the last entry applied on this machine.
    pub applied: u32,
}

/// An entry, or the AI switch, the engine did not take.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Refusal {
    /// The entry's sequence number; none for the AI switch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seq: Option<u32>,
    pub tick: u32,
    pub error: String,
}

// ---- firefight.command ----

/// `firefight.command`'s params: a command to append, a change to the
/// director, or both (the change first).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<Command>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub director: Option<DirectorRun>,
}

/// Whether the host steps its director after each tick.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectorRun {
    /// Stop stepping it: only appended commands reach the game.
    Hold,
    /// Step it again.
    Run,
}

/// One director command, as the host log carries it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    PlaceSquad {
        slot: u8,
        class: String,
        count: u16,
        zone: u8,
        weapon_tier: u8,
    },
    /// Two teams, after a `place_squad`: the Firefight team the squad is
    /// sent at; its `hunt` goes to that team's base.
    Target {
        slot: u8,
        team: u8,
    },
    Hunt {
        slot: u8,
    },
    EraseSquad {
        slot: u8,
    },
    Round {
        index: u8,
    },
    Wave {
        index: u8,
    },
    Skull {
        id: Skull,
        on: bool,
    },
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

/// The skulls a definition names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skull {
    Assassins,
    Thunderstorm,
    Famine,
    Blind,
    Ghost,
    Catch,
    Mythic,
    Angry,
    LowGravity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreReason {
    Kill,
    Assist,
    /// A kill of a player on the other team (two teams).
    PlayerKill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Won,
    Lost,
    /// Two teams: this Firefight team won.
    TeamWon { team: u8 },
    /// Two teams: level on score and lives.
    Draw,
}

/// `firefight.command`'s answer.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CommandReply {
    /// The appended entry: its sequence number and the tick it applies at.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appended: Option<Appended>,
    /// Whether the director is held now.
    pub held: bool,
    /// The log after the call.
    pub log: Log,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appended {
    pub seq: u32,
    pub tick: u32,
}

// ---- firefight.kit ----

/// `firefight.kit`'s answer: the kit as the scenario holds it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Kit {
    /// The scenario carries the kit (an `ff_slot_NN` squad).
    pub kit: bool,
    /// Each slot's scenario squad index, `null` where the kit has none.
    #[serde(default)]
    pub slots: Vec<Option<u16>>,
    /// The team the first slot's squad is authored on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<u8>,
    #[serde(default)]
    pub zones: Vec<KitZone>,
    /// Each class the kit maps, to its character palette row.
    #[serde(default)]
    pub classes: BTreeMap<String, u16>,
    /// Each player team's base point, by Firefight team: base 0 is the
    /// home base. A squad's hunt goes to its target team's base.
    #[serde(default)]
    pub bases: Vec<[f32; 3]>,
    /// Rows in the scenario's character palette.
    pub characters: u16,
    /// The scenario orders the kit authors for hunting, by base
    /// (`ff_hunt_00`, `ff_hunt_01`), `null` for a base without one.
    #[serde(default)]
    pub hunt_orders: Vec<Option<u16>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct KitZone {
    /// The scenario zone index.
    pub index: u16,
    /// Its first area's mean.
    pub point: [f32; 3],
    /// Its firing positions: where a squad placed there starts.
    pub starts: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{from_value, json, to_value};

    #[test]
    fn commands_take_the_host_logs_shape_and_nothing_else() {
        for value in [
            json!({"place_squad": {"slot": 0, "class": "elite", "count": 2, "zone": 1, "weapon_tier": 0}}),
            json!({"target": {"slot": 0, "team": 1}}),
            json!({"hunt": {"slot": 3}}),
            json!({"erase_squad": {"slot": 0}}),
            json!({"round": {"index": 1}}),
            json!({"wave": {"index": 4}}),
            json!({"skull": {"id": "low_gravity", "on": true}}),
            json!({"lives": {"pool": 3}}),
            json!({"lives": {"player": 1, "pool": 2}}),
            json!({"team_lives": {"team": 1, "pool": 9}}),
            json!({"score": {"player": 0, "points": 50, "reason": "assist"}}),
            json!({"score": {"player": 2, "points": 50, "reason": "player_kill"}}),
            json!({"respawn": {"player": 0}}),
            json!({"finish": {"outcome": "won"}}),
            json!({"finish": {"outcome": {"team_won": {"team": 1}}}}),
            json!({"finish": {"outcome": "draw"}}),
        ] {
            let command: Command = from_value(value.clone()).unwrap();
            assert_eq!(to_value(command).unwrap(), value);
        }
        for value in [
            json!({"hunt": {"slot": 3, "zone": 1}}),
            json!({"skip_wave": {}}),
            json!({"skull": {"id": "iron", "on": true}}),
            json!({"finish": {"outcome": "tie"}}),
            json!({"finish": {"outcome": {"team_won": {}}}}),
            json!({"target": {"slot": 0}}),
            json!({"team_lives": {"pool": 3}}),
            json!({"respawn": {}}),
        ] {
            assert!(from_value::<Command>(value.clone()).is_err(), "{value}");
        }
    }

    #[test]
    fn command_params_take_a_command_a_director_change_or_both() {
        let p: CommandParams =
            from_value(json!({"command": {"respawn": {"player": 0}}, "director": "hold"})).unwrap();
        assert_eq!(p.command, Some(Command::Respawn { player: 0 }));
        assert_eq!(p.director, Some(DirectorRun::Hold));
        let p: CommandParams = from_value(json!({"director": "run"})).unwrap();
        assert_eq!(p.director, Some(DirectorRun::Run));
        assert!(from_value::<CommandParams>(json!({"director": "pause"})).is_err());
        assert!(from_value::<CommandParams>(json!({"op": "respawn"})).is_err());
    }
}

// ---- forge.waves.test ----

/// `forge.waves.test`'s params: none starts a test, `stop` stops one.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WavesTestParams {
    #[serde(default)]
    pub stop: bool,
}

/// `forge.waves.test`'s answer: once the test has started or stopped, or
/// at once when the ground must be built first.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WavesTestReply {
    /// A wave test runs now.
    pub testing: bool,
    /// The running map does not hold the current AI ground: it is being
    /// built, and the test starts once the map is reloaded onto it
    /// (`firefight.state`'s `test` says when).
    #[serde(default)]
    pub building: bool,
    /// A stop found a test to stop.
    #[serde(default)]
    pub stopped: bool,
    /// The wave set the test plays, by its name and `Def::id`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub set: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
}

/// How many game ticks `forge.waves.test` waits for its answer: the
/// runtime takes a start or a stop at its next tick boundary.
pub const WAVES_TEST_WAIT_TICKS: u64 = 30;
