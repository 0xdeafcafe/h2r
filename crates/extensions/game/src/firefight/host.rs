//! The Firefight host runtime (docs/game/firefight.md, "The host runtime").
//!
//! [`Host::start`] runs at map load: it refuses a map without the kit,
//! admits the definition against the kit's classes, and picks a team for
//! the actors that no player holds and that is not a player team (0, and 1
//! with two teams). Each tick the caller runs
//! [`Host::pre_tick`] before the game tick, which applies the host log's
//! due entries in sequence order, and [`Host::post_tick`] after it, which
//! builds [`Observed`], steps the director and appends its commands for the
//! next tick. Only machines that run the simulation of record apply
//! entries: a distributed client applies none and checks the actor
//! channel's digest instead (`crate::actor_net::client`). The director never touches the game; every change goes through a
//! [`Game`] entry.
//!
//! [`Host::start_test`] starts a wave test in a Forge session
//! (docs/game/forge-firefight.md, "Test waves"): the host of a match over
//! the same game, whose `apply` has a test arm. `Lives`, `TeamLives` and
//! `Respawn` are not applied (the session's variant respawns players as it
//! does), and `Finish` ends the test, not the game ([`Host::take_test_end`]).

use std::collections::{BTreeMap, BTreeSet};

use ::firefight::director::{
    Kill, PlayerSeen, Phase, SQUAD_SLOTS, State, ZoneSeen, firefight_team,
};
use ::firefight::{
    Command, Def, Limits, MAX_TEAMS, Observed, Outcome, PlayerKill, ScoreReason, ScriptDirector,
    Settings, Skull,
};
use host_log::{Entry, Log, LogHead, Sha256};

use crate::actor_net::squads::{Placement, SquadEntry, SquadOp};

/// Teams a game can hold.
pub const TEAMS: u8 = 8;
/// Zone distances are reported in hundredths of a world unit.
pub const DISTANCE_SCALE: f32 = 100.0;
/// The author of an entry the operator appended ([`Host::append`]).
pub const OPERATOR: &str = "agent";

/// What this machine does with the host log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    /// The distributed server, or the host of a synchronous session: runs
    /// the director, writes the log and applies it.
    Host,
    /// A synchronous peer, or a film's playback: applies the entries it
    /// receives and never runs the director.
    Replica,
    /// A distributed client: runs no director and applies nothing. Its
    /// bodies come from replication, the mode state and the host's log
    /// head from the actor channel.
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
    /// Each player team's base, by Firefight team: base 0 is the home
    /// base, base 1 the second team's. A squad's cleanup hunt goes to its
    /// target's base.
    pub bases: Vec<[f32; 3]>,
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
    /// This tick's players killed (`SimEvent::Kill`), in event order, with
    /// the killer resolved to a roster slot.
    fn player_kills(&self) -> Vec<PlayerKill>;
    /// Add `points` to the player's score through the engine's score entry.
    fn credit_score(&mut self, player: u8, points: u32) -> anyhow::Result<()>;
    /// The shared pool (`player` none) or one player's lives.
    fn set_lives(&mut self, player: Option<u8>, pool: u16) -> anyhow::Result<()>;
    /// A team's shared pool, with two teams.
    fn set_team_lives(&mut self, team: u8, pool: u16) -> anyhow::Result<()>;
    fn respawn(&mut self, player: u8) -> anyhow::Result<()>;
    fn skull(&mut self, skull: Skull, on: bool) -> anyhow::Result<()>;
    /// End the game through the engine's own game end.
    fn end_game(&mut self, outcome: Outcome) -> anyhow::Result<()>;
    /// The entry whose engine calls follow, before [`Host::pre_tick`]
    /// applies it: a game that makes its calls later reports their errors
    /// by sequence number.
    fn begin_entry(&mut self, _seq: u32) {}
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
    /// The definition's rules script did not build or verify.
    Script(String),
    /// Every team is held by a player or is a player team.
    NoFreeTeam,
    /// Two player teams, and the kit marks fewer bases than that.
    Bases { have: usize, need: u8 },
    Log(String),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::NoKit => f.write_str("the map does not carry the Firefight kit"),
            Refusal::NoZones => f.write_str("the map's Firefight kit has no drop zones"),
            Refusal::Def(refusal) => write!(f, "the Firefight definition: {refusal}"),
            Refusal::Script(refusal) => write!(f, "the director script: {refusal}"),
            Refusal::NoFreeTeam => f.write_str("every team is held by a player"),
            Refusal::Bases { have, need } => write!(
                f,
                "the map's Firefight kit marks {have} bases, and {need} player teams need one each"
            ),
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

/// One `Score` command as the host's director credited it, with the class
/// of the actor a kill was for (docs/game/firefight.md, "Scoring and
/// stats"). The host reports each to Live and the film.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Credit {
    /// The tick the director stepped on.
    pub tick: u32,
    pub player: u8,
    pub points: u32,
    pub reason: ScoreReason,
    /// The class killed, on a kill the director scored by class.
    pub class: Option<String>,
}

/// How a match ended, as the host's director said so.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ended {
    pub tick: u32,
    /// The definition's name.
    pub set: String,
    /// With two teams the winner is in it (`TeamWon { team }`).
    pub outcome: Outcome,
    /// Rounds whose last wave was cleared (an endless game counts every
    /// set's).
    pub rounds_cleared: u32,
    /// Sets played through, bonus round included (an endless game); a
    /// finite game is one set, cleared when it ends `Won`.
    pub sets_cleared: u32,
    /// Respawns the director has issued.
    pub lives_used: u32,
    /// The definition's player teams: 1, or 2.
    pub teams: u8,
    /// Each Firefight team's points; the second is 0 with one team.
    pub scores: [u32; MAX_TEAMS],
}

/// Where the match was when the director finished it, for [`Ended`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Progress {
    /// The rounds cleared before it ended, when it did not clear them all
    /// (an endless game counts every set's), and the definition's rounds in
    /// one set.
    round: u32,
    rounds: u32,
    /// The sets played through when it ended.
    sets: u32,
    /// The last round's last wave is cleared: no actor of it is alive.
    last_wave_cleared: bool,
    respawns: u32,
    teams: u8,
    scores: [u32; MAX_TEAMS],
}

/// One match on one machine.
pub struct Host {
    role: Role,
    /// The host's director (the definition's rules script on the VM); none
    /// on a replica or a client.
    director: Option<ScriptDirector>,
    log: Log<Command>,
    /// The sequence number of the last applied entry.
    applied: u32,
    /// The bonus round is on (`Bonus { on }` applied): no respawn is made
    /// in it.
    bonus: bool,
    kit: Kit,
    by_character: BTreeMap<u16, String>,
    team: u8,
    /// The definition's player teams, on every role.
    teams: u8,
    ai_checked: bool,
    /// The director is not stepped (`firefight.command`'s hold).
    held: bool,
    /// Scores credited since the last [`Host::take_credits`].
    credits: Vec<Credit>,
    /// The result, once the director has issued `Finish`.
    ended: Option<Ended>,
    /// The director has finished the match (once).
    finished: bool,
    /// Respawn commands the director has issued.
    respawns: u32,
    /// The Firefight team each squad slot was last sent at (`Target`; 0
    /// with one team): its cleanup hunt goes to that team's base.
    targets: [u8; SQUAD_SLOTS],
    /// The mode blob of the last `ModeState` entry applied here.
    mode: Option<Vec<u8>>,
    /// The mode blob of the last `ModeState` entry the host appended.
    mode_sent: Option<Vec<u8>>,
    /// A wave test, not a match ([`Host::start_test`]).
    test: bool,
    /// The outcome of the test's `Finish`, until the runtime takes it.
    test_end: Option<Outcome>,
    /// Placements applied here whose slot has shown no actor yet: the slot
    /// to the tick it was applied and its class.
    unseen: BTreeMap<u8, (u32, String)>,
    /// Placements that showed no actor within [`UNSEEN_AFTER`] ticks.
    placements_unseen: u32,
    /// Their error rows, until the runtime takes them.
    unseen_errors: Vec<ApplyError>,
}

/// Ticks after a placement is applied at which a slot with no actor counts
/// as a placement nobody sees: the director's slot timeout (3 s).
pub const UNSEEN_AFTER: u32 = 90;

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
        if def.teams > 1 && kit.bases.len() < usize::from(def.teams) {
            return Err(Refusal::Bases {
                have: kit.bases.len(),
                need: def.teams,
            });
        }
        let held: BTreeSet<u8> = game.players().iter().map(|p| p.team).collect();
        let team = actor_team(def.teams, kit.team, &held)?;
        let teams = def.teams;
        let genesis = genesis(&def, settings.seed)?;
        let log = Log::new(LogHead::genesis(genesis)).map_err(|e| Refusal::Log(e.to_string()))?;
        let director = match role {
            Role::Host => Some(
                ScriptDirector::new(def, settings)
                    .map_err(|refusal| Refusal::Script(refusal.to_string()))?,
            ),
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
            bonus: false,
            kit,
            by_character,
            team,
            teams,
            ai_checked: false,
            held: false,
            credits: Vec::new(),
            ended: None,
            finished: false,
            respawns: 0,
            targets: [0; SQUAD_SLOTS],
            mode: None,
            mode_sent: None,
            test: false,
            test_end: None,
            unseen: BTreeMap::new(),
            placements_unseen: 0,
            unseen_errors: Vec::new(),
        })
    }

    /// Start a wave test in a Forge session: the host of a match, on this
    /// machine, whose `Lives`, `TeamLives` and `Respawn` are not applied
    /// and whose `Finish` ends the test ([`Host::take_test_end`]).
    pub fn start_test(def: Def, settings: Settings, game: &impl Game) -> Result<Self, Refusal> {
        let mut host = Host::start(Role::Host, def, settings, game)?;
        host.test = true;
        Ok(host)
    }

    /// Whether this is a wave test, not a match.
    pub fn test(&self) -> bool {
        self.test
    }

    /// The outcome of the test's `Finish`, once: the runtime ends the test.
    pub fn take_test_end(&mut self) -> Option<Outcome> {
        self.test_end.take()
    }

    pub fn role(&self) -> Role {
        self.role
    }

    /// The team the actors are placed on.
    pub fn team(&self) -> u8 {
        self.team
    }

    /// The definition's player teams: 1, or 2.
    pub fn teams(&self) -> u8 {
        self.teams
    }

    /// The Firefight team each squad slot is sent at.
    pub fn targets(&self) -> &[u8; SQUAD_SLOTS] {
        &self.targets
    }

    pub fn log(&self) -> &Log<Command> {
        &self.log
    }

    pub fn head(&self) -> LogHead {
        self.log.head()
    }

    /// The director's mode blob, on the host.
    pub fn state(&self) -> Option<State> {
        self.director.as_ref().map(ScriptDirector::state)
    }

    /// The director, on the host.
    pub fn director(&self) -> Option<&ScriptDirector> {
        self.director.as_ref()
    }

    /// The sequence number of the last entry applied here.
    pub fn applied(&self) -> u32 {
        self.applied
    }

    /// The mode blob the last applied `ModeState` entry carried: what a
    /// replica or a film's playback draws (docs/game/firefight.md, "The
    /// HUD").
    pub fn mode_state(&self) -> Option<&[u8]> {
        self.mode.as_deref()
    }

    /// On the host, after [`Host::post_tick`]: append the step's mode blob
    /// as a `ModeState` entry for the next tick, when it differs from the
    /// last one appended. Returns whether it appended. Elsewhere nothing is
    /// appended: the host writes the log.
    pub fn note_mode(&mut self, tick: u32, blob: &[u8]) -> Result<bool, host_log::Error> {
        if self.role != Role::Host || self.mode_sent.as_deref() == Some(blob) {
            return Ok(false);
        }
        let head = self.log.head();
        self.log.push(Entry {
            seq: head.next_seq()?,
            tick: tick.saturating_add(1).max(head.tick),
            author: None,
            op: Command::ModeState {
                blob: blob.to_vec(),
            },
            prev_hash: head.hash,
        })?;
        self.mode_sent = Some(blob.to_vec());
        Ok(true)
    }

    pub fn kit(&self) -> &Kit {
        &self.kit
    }

    /// Whether [`Host::post_tick`] leaves the director alone.
    pub fn held(&self) -> bool {
        self.held
    }

    /// Hold the director (only appended commands reach the game) or run it
    /// again. Refused where no director runs.
    pub fn set_held(&mut self, held: bool) -> anyhow::Result<()> {
        anyhow::ensure!(self.director.is_some(), "only the host runs the director");
        self.held = held;
        Ok(())
    }

    /// Append one command of the operator's (the agent API's
    /// `firefight.command`) on the host, to apply at `tick` or at the log
    /// head's tick if that is later. Its entry carries [`OPERATOR`] as
    /// the author, so the log tells it from the director's. Clients and
    /// replicas refuse: the host writes the log.
    pub fn append(&mut self, tick: u32, op: Command) -> anyhow::Result<Entry<Command>> {
        anyhow::ensure!(self.role == Role::Host, "the host writes its own log");
        let head = self.log.head();
        let entry = Entry {
            seq: head.next_seq()?,
            tick: tick.max(head.tick),
            author: Some(OPERATOR.to_string()),
            op,
            prev_hash: head.hash,
        };
        self.log.push(entry.clone())?;
        Ok(entry)
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
            game.begin_entry(entry.seq);
            match self.apply(&entry.op, game) {
                Ok(()) => match &entry.op {
                    Command::PlaceSquad { slot, class, .. } => {
                        self.unseen.insert(*slot, (entry.tick, class.clone()));
                    }
                    Command::EraseSquad { slot } => {
                        self.unseen.remove(slot);
                    }
                    _ => {}
                },
                Err(error) => errors.push(ApplyError {
                    seq: Some(entry.seq),
                    tick: entry.tick,
                    error,
                }),
            }
        }
        errors
    }

    /// After the game tick, on the host: step the director on what the game
    /// shows and append its commands for the next tick. Returns how many
    /// entries were appended.
    pub fn post_tick(&mut self, tick: u32, game: &impl Game) -> Result<usize, host_log::Error> {
        if self.director.is_none() || self.held {
            return Ok(0);
        }
        let teams = self.director.as_ref().map_or(1, |d| d.def().teams);
        let seen = observe(&self.kit, &self.by_character, teams, game);
        self.check_unseen(tick, &seen.actors_alive_by_squad);
        let Some(director) = self.director.as_mut() else {
            return Ok(0);
        };
        let commands = director.step(tick, &seen);
        let set = director.def().name.clone();
        let progress = progress(director);
        let by_class = &director.def().scoring.by_class;
        self.credits.extend(credits(tick, &seen, &commands, |class| {
            by_class.contains_key(class)
        }));
        self.respawns += commands
            .iter()
            .filter(|op| matches!(op, Command::Respawn { .. }))
            .count() as u32;
        let progress = Progress {
            respawns: self.respawns,
            ..progress
        };
        if !self.finished
            && let Some(ended) = result(tick, set, progress, &commands)
        {
            self.finished = true;
            self.ended = Some(ended);
        }
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

    /// A placement whose slot shows no actor [`UNSEEN_AFTER`] ticks after it
    /// was applied is reported once: a counter and one error row. A wave
    /// whose actors nobody counts would otherwise clear at the director's
    /// slot timeout with no error.
    fn check_unseen(&mut self, tick: u32, alive: &[u16]) {
        self.unseen
            .retain(|slot, _| alive.get(usize::from(*slot)).is_none_or(|&n| n == 0));
        let due: Vec<u8> = self
            .unseen
            .iter()
            .filter(|(_, (at, _))| tick >= at.saturating_add(UNSEEN_AFTER))
            .map(|(slot, _)| *slot)
            .collect();
        for slot in due {
            let Some((at, class)) = self.unseen.remove(&slot) else {
                continue;
            };
            self.placements_unseen = self.placements_unseen.saturating_add(1);
            self.unseen_errors.push(ApplyError {
                seq: None,
                tick,
                error: anyhow::anyhow!("slot {slot}: {class} placed at tick {at}, none seen"),
            });
        }
    }

    /// Placements that showed no actor, since the game started.
    pub fn placements_unseen(&self) -> u32 {
        self.placements_unseen
    }

    /// The error rows of placements newly found unseen.
    pub fn take_unseen_errors(&mut self) -> Vec<ApplyError> {
        std::mem::take(&mut self.unseen_errors)
    }

    /// The scores credited since the last call, in the order the director
    /// issued them.
    pub fn take_credits(&mut self) -> Vec<Credit> {
        std::mem::take(&mut self.credits)
    }

    /// The match's result, once, when the director has finished it.
    pub fn take_ended(&mut self) -> Option<Ended> {
        self.ended.take()
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

    fn apply(&mut self, op: &Command, game: &mut impl Game) -> anyhow::Result<()> {
        if self.test {
            match op {
                // A test charges no lives, and the session's variant
                // respawns its players as it does.
                Command::Lives { .. } | Command::TeamLives { .. } | Command::Respawn { .. } => {
                    return Ok(());
                }
                // The end of the waves ends the test: the runtime erases
                // the squads and the session goes on.
                Command::Finish { outcome } => {
                    self.test_end = Some(*outcome);
                    return Ok(());
                }
                _ => {}
            }
        }
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
            Command::Target { slot, team } => {
                anyhow::ensure!(
                    usize::from(*team) < MAX_TEAMS && usize::from(*team) < self.kit.bases.len(),
                    "the kit has no base for team {team}"
                );
                let target = self.targets.get_mut(usize::from(*slot)).ok_or_else(|| {
                    anyhow::anyhow!("squad slot {slot} is outside the map's {SQUAD_SLOTS} slots")
                })?;
                *target = *team;
                Ok(())
            }
            Command::Hunt { slot } => {
                let base = self.targets.get(usize::from(*slot)).copied().unwrap_or(0);
                squad(game, SquadOp::Hunt { slot: *slot, base })
            }
            Command::EraseSquad { slot } => squad(game, SquadOp::Erase { slot: *slot }),
            // The HUD reads sets, rounds and waves from the mode blob.
            Command::Set { .. } | Command::Round { .. } | Command::Wave { .. } => Ok(()),
            // The bonus round is a director rule (docs/game/firefight.md,
            // "The bonus round"): the host keeps whether it is on, and
            // [`Command::Respawn`] is refused while it is. Its enemies are
            // ordinary `PlaceSquad`s and its skulls ordinary `Skull`s.
            Command::Bonus { on, .. } => {
                self.bonus = *on;
                Ok(())
            }
            // Iron is a director rule, never an engine skull.
            Command::Skull {
                id: ::firefight::Skull::Iron,
                ..
            } => Ok(()),
            Command::Skull { id, on } => game.skull(*id, *on),
            // The engine keeps no pool: the director's is the pool its
            // respawns spend, so a `Lives` or `TeamLives` sets it there (the
            // director's own set the pool it holds; an operator's changes
            // it). Where no director runs the entry is the game's alone.
            Command::Lives { player, pool } => {
                if let Some(director) = self.director.as_mut() {
                    director
                        .set_lives(*player, *pool)
                        .map_err(anyhow::Error::msg)?;
                }
                game.set_lives(*player, *pool)
            }
            Command::TeamLives { team, pool } => {
                anyhow::ensure!(usize::from(*team) < MAX_TEAMS, "no player team {team}");
                if let Some(director) = self.director.as_mut() {
                    director
                        .set_team_pool(*team, *pool)
                        .map_err(anyhow::Error::msg)?;
                }
                game.set_team_lives(*team, *pool)
            }
            Command::Score { player, points, .. } => game.credit_score(*player, *points),
            Command::Respawn { .. } if self.bonus => {
                anyhow::bail!("no respawn is made in the bonus round (Iron)")
            }
            Command::Respawn { player } => game.respawn(*player),
            Command::Finish { outcome } => game.end_game(*outcome),
            // The game takes no call: the HUD draws the blob.
            Command::ModeState { blob } => {
                self.mode = Some(blob.clone());
                Ok(())
            }
        }
    }
}

/// The `Score` commands of one step, each with the class it was for. The
/// director scores each seen kill that has a killer and a scored class, in
/// order, then that kill's assist; a score it cannot pair with a kill is
/// kept without a class, so no points are lost.
fn credits(
    tick: u32,
    seen: &Observed,
    commands: &[Command],
    scored: impl Fn(&str) -> bool,
) -> Vec<Credit> {
    let mut scores: std::collections::VecDeque<(u8, u32, ScoreReason)> = commands
        .iter()
        .filter_map(|op| match op {
            Command::Score {
                player,
                points,
                reason,
            } => Some((*player, *points, *reason)),
            _ => None,
        })
        .collect();
    let mut out = Vec::new();
    let mut credit = |(player, points, reason): (u8, u32, ScoreReason), class: Option<String>| {
        out.push(Credit {
            tick,
            player,
            points,
            reason,
            class,
        });
    };
    for kill in &seen.kills {
        let Some(killer) = kill.killer.filter(|_| scored(&kill.class)) else {
            continue;
        };
        if let Some(s) = scores.pop_front_if(|s| s.2 == ScoreReason::Kill && s.0 == killer) {
            credit(s, Some(kill.class.clone()));
        }
        if let Some(assist) = kill.assist.filter(|&assist| assist != killer)
            && scores
                .front()
                .is_some_and(|s| s.2 == ScoreReason::Assist && s.0 == assist)
        {
            credit(scores.pop_front().expect("fronted"), None);
        }
    }
    for rest in scores {
        credit(rest, None);
    }
    out
}

/// Where the director is after a step: the rounds and sets cleared, whether
/// the last round's last wave is cleared, and the teams' points. The
/// respawns are the host's to fill in.
///
/// An endless game counts across sets: a set played through is
/// `rounds` rounds and a set. The bonus round follows a set, so in it the
/// set is already played through.
fn progress(director: &ScriptDirector) -> Progress {
    let (state, def) = (director.state(), director.def());
    let rounds = def.rounds.len() as u32;
    let round = u32::from(state.round);
    let waves = def
        .rounds
        .get(usize::from(state.round))
        .map_or(0, |r| r.waves.len());
    let (sets, cleared) = if def.set.endless {
        let set = u32::from(state.set);
        let bonus = matches!(state.phase, Phase::Bonus { .. });
        (
            set + u32::from(bonus),
            set * rounds + if bonus { rounds } else { round },
        )
    } else {
        (0, round)
    };
    Progress {
        round: cleared,
        rounds,
        sets,
        last_wave_cleared: round + 1 == rounds
            && usize::from(state.wave) + 1 == waves
            && director.live_actors() == 0,
        respawns: 0,
        teams: def.teams,
        scores: state.scores,
    }
}

/// The result a step's `Finish` makes: a won match cleared every round, a
/// lost one the rounds before the one it was in. Two teams' result (a team
/// won, or a draw) cleared every round when the last round's last wave was
/// cleared, else the rounds before the one it was in.
fn result(tick: u32, set: String, progress: Progress, commands: &[Command]) -> Option<Ended> {
    let outcome = commands.iter().find_map(|op| match op {
        Command::Finish { outcome } => Some(*outcome),
        _ => None,
    })?;
    let all = match outcome {
        Outcome::Won => true,
        Outcome::Lost => false,
        Outcome::TeamWon { .. } | Outcome::Draw => progress.last_wave_cleared,
    };
    Some(Ended {
        tick,
        set,
        outcome,
        rounds_cleared: if all { progress.rounds } else { progress.round },
        sets_cleared: if progress.sets > 0 { progress.sets } else { u32::from(all) },
        lives_used: progress.respawns,
        teams: progress.teams,
        scores: progress.scores,
    })
}

/// The actors' team: the kit's, unless a player holds it or it is a player
/// team (team 0, and team 1 with two teams), else the lowest team that is
/// neither. With two player teams the actors are always a third.
pub(crate) fn actor_team(teams: u8, kit: u8, held: &BTreeSet<u8>) -> Result<u8, Refusal> {
    let taken = |team: u8| held.contains(&team) || team < teams.max(1);
    if !taken(kit) {
        return Ok(kit);
    }
    (0..TEAMS).find(|&team| !taken(team)).ok_or(Refusal::NoFreeTeam)
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

/// The mean of `positions`, or none without one.
fn centre<'a>(positions: impl IntoIterator<Item = &'a [f32; 3]>) -> Option<[f32; 3]> {
    let mut sum = [0.0f32; 3];
    let mut n = 0usize;
    for position in positions {
        for (total, value) in sum.iter_mut().zip(position) {
            *total += value;
        }
        n += 1;
    }
    (n > 0).then(|| sum.map(|total| total / n as f32))
}

/// What the director sees this tick, for a definition of `teams` player
/// teams. A kill of a character the kit does not name scores nothing, so
/// it is left out. Each zone's distance is from the centre of the players
/// who have a unit, and from each Firefight team's such players
/// (`u32::MAX` for a team with none).
fn observe(
    kit: &Kit,
    by_character: &BTreeMap<u16, String>,
    teams: u8,
    game: &impl Game,
) -> Observed {
    let players = game.players();
    let all = centre(players.iter().filter_map(|p| p.position.as_ref()));
    let by_team: [Option<[f32; 3]>; MAX_TEAMS] = std::array::from_fn(|team| {
        centre(
            players
                .iter()
                .filter(|p| usize::from(firefight_team(teams, p.team)) == team)
                .filter_map(|p| p.position.as_ref()),
        )
    });
    let zones = kit
        .zones
        .iter()
        .map(|&zone| ZoneSeen {
            seen: game.sees(zone),
            distance: all.map_or(0, |centre| distance(centre, zone)),
            team_distance: by_team.map(|c| c.map_or(u32::MAX, |centre| distance(centre, zone))),
            // Until the kit's own roles are read back, a zone serves every role.
            roles: ::firefight::ALL_ZONE_ROLES,
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
                team: p.team,
            })
            .collect(),
        actors_alive_by_squad: game.actors_by_squad(),
        zones,
        kills,
        player_kills: game.player_kills(),
    }
}

/// Distance in hundredths of a world unit; the cast saturates.
fn distance(a: [f32; 3], b: [f32; 3]) -> u32 {
    let squared: f32 = a.iter().zip(&b).map(|(a, b)| (a - b) * (a - b)).sum();
    (squared.sqrt() * DISTANCE_SCALE) as u32
}

#[cfg(test)]
mod tests;
