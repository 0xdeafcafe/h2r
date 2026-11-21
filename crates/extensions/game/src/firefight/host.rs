//! The Firefight host runtime (docs/game/firefight.md, "The host runtime").
//!
//! [`Host::start`] runs at map load: it refuses a map without the kit,
//! admits the definition against the kit's classes, and picks a team no
//! player holds for the actors. Each tick the caller runs
//! [`Host::pre_tick`] before the game tick, which applies the host log's
//! due entries in sequence order, and [`Host::post_tick`] after it, which
//! builds [`Observed`], steps the director and appends its commands for the
//! next tick. Only machines that run the simulation of record apply
//! entries: a distributed client keeps the log for its digest and applies
//! none. The director never touches the game; every change goes through a
//! [`Game`] entry.

use std::collections::{BTreeMap, BTreeSet};

use ::firefight::director::{Kill, PlayerSeen, SQUAD_SLOTS, State, ZoneSeen};
use ::firefight::{Command, Def, Director, Limits, Observed, Outcome, Settings, Skull};
use host_log::{Entry, Log, LogHead, Sha256};

use crate::actor_net::squads::{Placement, SquadEntry, SquadOp};

/// Teams a game can hold.
pub const TEAMS: u8 = 8;
/// Zone distances are reported in hundredths of a world unit.
pub const DISTANCE_SCALE: f32 = 100.0;

/// What this machine does with the host log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The distributed server, or the host of a synchronous session: runs
    /// the director, writes the log and applies it.
    Host,
    /// A synchronous peer, or a film's playback: applies the entries it
    /// receives and never runs the director.
    Replica,
    /// A distributed client: keeps the log for the digest and applies
    /// nothing. Its bodies come from replication.
    Client,
}

impl Role {
    /// The machine runs the actors, so it applies entries and needs AI on.
    pub fn applies(self) -> bool {
        !matches!(self, Role::Client)
    }
}

/// The two AI globals bytes `ai_update` gates on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AiGlobals {
    /// `+0`, the script's `ai` switch.
    pub on: bool,
    /// `+1`, set once AI is set up for the map.
    pub initialised: bool,
}

/// What the loaded map's kit provides (docs/game/firefight.md, "The kit").
#[derive(Clone, Debug, PartialEq)]
pub struct Kit {
    /// The drop zones in kit order.
    pub zones: Vec<[f32; 3]>,
    /// Each class's map-local character tag slot.
    pub classes: BTreeMap<String, u16>,
    /// The team the kit's squads are authored on.
    pub team: u8,
}

/// One player in the match, read after the game tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PlayerView {
    /// The roster slot.
    pub slot: u8,
    pub alive: bool,
    pub team: u8,
    /// The player's unit position; none between lives.
    pub position: Option<[f32; 3]>,
}

/// One `SimEvent::ActorKilled` of the tick, with the killer resolved to a
/// roster slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KillSeen {
    /// The killed actor's map-local character tag slot.
    pub character: u16,
    pub killer: Option<u8>,
    /// A second player who damaged the actor in its last five seconds.
    pub assist: Option<u8>,
}

/// The running game, as the host runtime reads and changes it. Each write is
/// one engine entry; docs/game/firefight.md, "Guest boundary", lists the
/// entries and which are traced.
pub trait Game: SquadEntry {
    /// The loaded map's kit, or none on a map without it.
    fn kit(&self) -> Option<Kit>;
    /// None before the AI globals exist.
    fn ai(&self) -> Option<AiGlobals>;
    /// Set the AI globals' on byte.
    fn switch_ai_on(&mut self) -> anyhow::Result<()>;
    fn players(&self) -> Vec<PlayerView>;
    /// Live actors in each kit squad slot.
    fn actors_by_squad(&self) -> [u16; SQUAD_SLOTS];
    /// Some player has line of sight to `point` (the AI's visibility test).
    fn sees(&self, point: [f32; 3]) -> bool;
    /// This tick's actor kills, in event order.
    fn kills(&self) -> Vec<KillSeen>;
    /// Add `points` to the player's score through the engine's score entry.
    fn credit_score(&mut self, player: u8, points: u32) -> anyhow::Result<()>;
    /// The shared pool (`player` none) or one player's lives.
    fn set_lives(&mut self, player: Option<u8>, pool: u16) -> anyhow::Result<()>;
    fn respawn(&mut self, player: u8) -> anyhow::Result<()>;
    fn skull(&mut self, skull: Skull, on: bool) -> anyhow::Result<()>;
    /// End the game through the engine's own game end.
    fn end_game(&mut self, outcome: Outcome) -> anyhow::Result<()>;
}

/// Why the mode does not start on this map.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The loaded map does not carry the Firefight kit.
    NoKit,
    /// The kit has no drop zones.
    NoZones,
    /// The definition is invalid or names a class the kit lacks.
    Def(::firefight::Refusal),
    /// Every team is held by a player.
    NoFreeTeam,
    Log(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoKit => f.write_str("the map does not carry the Firefight kit"),
            Refusal::NoZones => f.write_str("the map's Firefight kit has no drop zones"),
            Refusal::Def(refusal) => write!(f, "the Firefight definition: {refusal}"),
            Refusal::NoFreeTeam => f.write_str("every team is held by a player"),
            Refusal::Log(error) => write!(f, "the host log: {error}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// An entry, or the AI switch, the engine did not take.
#[derive(Debug)]
pub struct ApplyError {
    /// The entry's sequence number; none for the AI switch.
    pub seq: Option<u32>,
    pub tick: u32,
    pub error: anyhow::Error,
}

impl std::fmt::Display for ApplyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.seq {
            Some(seq) => write!(
                f,
                "host log entry {seq} at tick {}: {}",
                self.tick, self.error
            ),
            None => write!(f, "AI switch at tick {}: {}", self.tick, self.error),
        }
    }
}

/// One match on one machine.
pub struct Host {
    role: Role,
    /// The host's director; none on a replica or a client.
    director: Option<Director>,
    log: Log<Command>,
    /// The sequence number of the last applied entry.
    applied: u32,
    kit: Kit,
    by_character: BTreeMap<u16, String>,
    team: u8,
    ai_checked: bool,
}

impl Host {
    /// Start the mode at map load, or refuse it.
    pub fn start(
        role: Role,
        def: Def,
        settings: Settings,
        game: &impl Game,
    ) -> Result<Self, Refusal> {
        let kit = game.kit().ok_or(Refusal::NoKit)?;
        if kit.zones.is_empty() {
            return Err(Refusal::NoZones);
        }
        ::firefight::validate(&def, &Limits::new(kit.classes.keys().cloned()))
            .map_err(Refusal::Def)?;
        let held: BTreeSet<u8> = game.players().iter().map(|p| p.team).collect();
        let team = if held.contains(&kit.team) {
            (0..TEAMS)
                .find(|team| !held.contains(team))
                .ok_or(Refusal::NoFreeTeam)?
        } else {
            kit.team
        };
        let genesis = genesis(&def, settings.seed)?;
        let log = Log::new(LogHead::genesis(genesis)).map_err(|e| Refusal::Log(e.to_string()))?;
        let director = match role {
            Role::Host => Some(Director::new(def, settings).map_err(Refusal::Def)?),
            Role::Replica | Role::Client => None,
        };
        let by_character = kit
            .classes
            .iter()
            .map(|(class, &character)| (character, class.clone()))
            .collect();
        Ok(Self {
            role,
            director,
            log,
            applied: 0,
            kit,
            by_character,
            team,
            ai_checked: false,
        })
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// The team the actors are placed on.
    pub fn team(&self) -> u8 {
        self.team
    }

    pub fn log(&self) -> &Log<Command> {
        &self.log
    }

    pub fn head(&self) -> LogHead {
        self.log.head()
    }

    /// The director's mode blob, on the host.
    pub fn state(&self) -> Option<&State> {
        self.director.as_ref().map(Director::state)
    }

    /// Before the game tick: switch AI on if it is off, then apply every
    /// entry due by `tick` once, in sequence order. An entry the engine
    /// refuses is reported and not retried.
    pub fn pre_tick(&mut self, tick: u32, game: &mut impl Game) -> Vec<ApplyError> {
        let mut errors = Vec::new();
        if !self.role.applies() {
            return errors;
        }
        if let Err(error) = self.check_ai(game) {
            errors.push(ApplyError {
                seq: None,
                tick,
                error,
            });
        }
        let due: Vec<Entry<Command>> = self
            .log
            .since(self.applied)
            .iter()
            .take_while(|entry| entry.tick <= tick)
            .cloned()
            .collect();
        for entry in due {
            self.applied = entry.seq;
            if let Err(error) = self.apply(&entry.op, game) {
                errors.push(ApplyError {
                    seq: Some(entry.seq),
                    tick: entry.tick,
                    error,
                });
            }
        }
        errors
    }

    /// After the game tick, on the host: step the director on what the game
    /// shows and append its commands for the next tick. Returns how many
    /// entries were appended.
    pub fn post_tick(&mut self, tick: u32, game: &impl Game) -> Result<usize, host_log::Error> {
        if self.director.is_none() {
            return Ok(0);
        }
        let seen = observe(&self.kit, &self.by_character, game);
        let Some(director) = self.director.as_mut() else {
            return Ok(0);
        };
        let commands = director.step(tick, &seen);
        let at = tick.saturating_add(1);
        let appended = commands.len();
        for op in commands {
            let head = self.log.head();
            self.log.push(Entry {
                seq: head.next_seq()?,
                tick: at,
                author: None,
                op,
                prev_hash: head.hash,
            })?;
        }
        Ok(appended)
    }

    /// Take an entry from the host (a synchronous peer, a client's digest)
    /// or from a film. The log checks the chain; the host writes its own.
    pub fn receive(&mut self, entry: Entry<Command>) -> anyhow::Result<LogHead> {
        anyhow::ensure!(self.role != Role::Host, "the host writes its own log");
        Ok(self.log.push(entry)?)
    }

    /// Switch AI on once its globals are initialised, only if it is off
    /// (probe P2, docs/game/ai-networking.md).
    fn check_ai(&mut self, game: &mut impl Game) -> anyhow::Result<()> {
        if self.ai_checked {
            return Ok(());
        }
        let Some(ai) = game.ai().filter(|ai| ai.initialised) else {
            return Ok(());
        };
        self.ai_checked = true;
        if !ai.on {
            game.switch_ai_on()?;
        }
        Ok(())
    }

    fn apply(&self, op: &Command, game: &mut impl Game) -> anyhow::Result<()> {
        match op {
            Command::PlaceSquad {
                slot,
                class,
                count,
                zone,
                weapon_tier,
            } => {
                let character = *self
                    .kit
                    .classes
                    .get(class)
                    .ok_or_else(|| anyhow::anyhow!("the kit has no class {class}"))?;
                anyhow::ensure!(
                    usize::from(*zone) < self.kit.zones.len(),
                    "the kit has no zone {zone}"
                );
                squad(
                    game,
                    SquadOp::Place(Placement {
                        slot: *slot,
                        character,
                        count: *count,
                        zone: *zone,
                        team: self.team,
                        weapon_tier: *weapon_tier,
                    }),
                )
            }
            Command::Hunt { slot } => squad(game, SquadOp::Hunt { slot: *slot }),
            Command::EraseSquad { slot } => squad(game, SquadOp::Erase { slot: *slot }),
            // The HUD reads rounds and waves from the mode blob.
            Command::Round { .. } | Command::Wave { .. } => Ok(()),
            Command::Skull { id, on } => game.skull(*id, *on),
            Command::Lives { player, pool } => game.set_lives(*player, *pool),
            Command::Score { player, points, .. } => game.credit_score(*player, *points),
            Command::Respawn { player } => game.respawn(*player),
            Command::Finish { outcome } => game.end_game(*outcome),
        }
    }
}

fn squad(game: &mut impl Game, op: SquadOp) -> anyhow::Result<()> {
    op.check(SQUAD_SLOTS)?;
    game.squad(op)
}

/// The log's starting hash: the definition and the match seed.
fn genesis(def: &Def, seed: u64) -> Result<Sha256, Refusal> {
    let id = def.id().map_err(Refusal::Def)?;
    let mut bytes = id.0.to_vec();
    bytes.extend_from_slice(&seed.to_le_bytes());
    Ok(Sha256::of(&bytes))
}

/// What the director sees this tick. A kill of a character the kit does not
/// name scores nothing, so it is left out.
fn observe(kit: &Kit, by_character: &BTreeMap<u16, String>, game: &impl Game) -> Observed {
    let players = game.players();
    let positions: Vec<[f32; 3]> = players.iter().filter_map(|p| p.position).collect();
    let centre = (!positions.is_empty()).then(|| {
        let mut sum = [0.0f32; 3];
        for position in &positions {
            for (total, value) in sum.iter_mut().zip(position) {
                *total += value;
            }
        }
        sum.map(|total| total / positions.len() as f32)
    });
    let zones = kit
        .zones
        .iter()
        .map(|&zone| ZoneSeen {
            seen: game.sees(zone),
            distance: centre.map_or(0, |centre| distance(centre, zone)),
        })
        .collect();
    let kills = game
        .kills()
        .into_iter()
        .filter_map(|kill| {
            Some(Kill {
                class: by_character.get(&kill.character)?.clone(),
                killer: kill.killer,
                assist: kill.assist,
            })
        })
        .collect();
    Observed {
        players: players
            .iter()
            .map(|p| PlayerSeen {
                slot: p.slot,
                alive: p.alive,
            })
            .collect(),
        actors_alive_by_squad: game.actors_by_squad(),
        zones,
        kills,
    }
}

/// Distance in hundredths of a world unit; the cast saturates.
fn distance(a: [f32; 3], b: [f32; 3]) -> u32 {
    let squared: f32 = a.iter().zip(&b).map(|(a, b)| (a - b) * (a - b)).sum();
    (squared.sqrt() * DISTANCE_SCALE) as u32
}

#[cfg(test)]
mod tests;
