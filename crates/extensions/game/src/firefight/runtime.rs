//! Firefight in the running game: the [`Game`] over the guest bodies
//! (`super::guest`), one match's state, and what each tick boundary does
//! with it (docs/game/firefight.md, "The host runtime").
//!
//! The feature (`super::feature`) calls in at three moments:
//!
//! - [`Runtime::start`] when a game starts: off unless it is a
//!   multiplayer game; [`Host::start`] then refuses a map without the kit,
//!   and the refusal is kept with its reason.
//! - [`Runtime::pre_tick`] before each game tick: [`Host::pre_tick`] over a
//!   [`GuestGame`] that makes memory reads and writes at once and keeps
//!   every engine call for later, in entry order. The feature makes those
//!   calls, and the sight pass, in an async body ([`run_work`]) and then
//!   runs the tick itself.
//! - [`Runtime::post_tick`] after each game tick: [`Host::post_tick`] over
//!   what the guest shows, the tick's actor and player kills and the last
//!   sight pass.
//!
//! Ticks are numbered as the tick a boundary belongs to: before a tick it
//! is the game time's tick count plus one, after it the count, so the
//! director's entries for "the next tick" are due at the next boundary.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ::firefight::director::SQUAD_SLOTS;
use ::firefight::{Command, MAX_TEAMS, Outcome, PlayerKill, Settings, Skull};
use agent_proto::firefight as wire;
use engine::entry::{Cx, EntryError, GuestCall, Ret};
use engine::{GuestMemory, MemFault, points};
use sim_state::SimEvent;

use super::guest::{self, KitLayout};
use super::host::{AiGlobals, Game, Host, KillSeen, Kit, PlayerView, Role};
use super::hud;
use super::options::{self, Options};
use crate::actor_net::squads::{SquadEntry, SquadOp};

/// The match seed until the session data carries one (FF9).
pub const SEED: u64 = 0;

/// How often the sight pass runs, in ticks.
pub const SIGHT_EVERY: u32 = 15;
/// An assist is a second player's damage in the actor's last five seconds.
pub const ASSIST_TICKS: u32 = 5 * 30;
/// The engine refusals `firefight.state` keeps.
pub const ERRORS_KEPT: usize = 16;
/// The options mode of a multiplayer game (`[GAME_GLOBALS] + 8`).
pub const MODE_MULTIPLAYER: u32 = 2;
/// Simulation world modes (docs/live/live-coop.md): the synchronous
/// client and the distributed client.
pub const WORLD_SYNC_CLIENT: u32 = 3;
pub const WORLD_DIST_CLIENT: u32 = 5;
/// The player body's depleted bit in its damage flags.
const BODY_DEPLETED: u16 = 1 << 2;

/// Writes the director's skulls to the engine: in the game,
/// `skulls_mp`'s Firefight set and skull array.
pub type SkullSink =
    dyn Fn(&BTreeSet<Skull>, &mut dyn GuestMemory) -> anyhow::Result<()> + Send + Sync;

/// What the host reports of a match as it plays: each credited score and
/// the result (docs/game/firefight.md, "Scoring and stats"). In the game it
/// goes to Live telemetry, which records it in the game stream and the film.
pub enum Report {
    Score(live_proto::firefight::FirefightScore),
    Ended(live_proto::firefight::FirefightEnded),
}

pub type ReportSink = dyn Fn(Report) + Send + Sync;

/// A credit as the wire names it.
fn score_report(credit: super::host::Credit) -> live_proto::firefight::FirefightScore {
    live_proto::firefight::FirefightScore {
        tick: u64::from(credit.tick),
        player: u32::from(credit.player),
        points: credit.points,
        reason: match credit.reason {
            ::firefight::ScoreReason::Kill => "kill",
            ::firefight::ScoreReason::Assist => "assist",
            ::firefight::ScoreReason::PlayerKill => "player_kill",
        }
        .into(),
        class: credit.class,
    }
}

/// The wire's name for an outcome: `won`, `lost`, `team_won` (the team is
/// the wire's `team`) or `draw`.
pub fn outcome_name(outcome: Outcome) -> &'static str {
    match outcome {
        Outcome::Won => "won",
        Outcome::Lost => "lost",
        Outcome::TeamWon { .. } => "team_won",
        Outcome::Draw => "draw",
    }
}

/// A result as the wire names it: the team count, the outcome with a
/// winning team, and one score a team. The set's name is cut to the wire's
/// 32.
pub fn ended_report(ended: super::host::Ended) -> live_proto::firefight::FirefightEnded {
    let teams = usize::from(ended.teams).clamp(1, MAX_TEAMS);
    live_proto::firefight::FirefightEnded {
        tick: u64::from(ended.tick),
        set: ended.set.chars().take(32).collect(),
        teams: teams as u8,
        outcome: outcome_name(ended.outcome).into(),
        team: match ended.outcome {
            Outcome::TeamWon { team } => Some(team),
            _ => None,
        },
        scores: ended.scores[..teams].to_vec(),
        rounds_cleared: ended.rounds_cleared,
        lives_used: ended.lives_used,
    }
}

/// One engine call an entry needs, made in the tick's async body.
#[derive(Clone, Debug, PartialEq)]
pub enum Call {
    Squad(SquadOp),
    Score { player: u8, points: u32 },
    Respawn(u8),
    End,
}

/// What the async body before a tick does.
#[derive(Clone, Debug, PartialEq)]
pub struct Work {
    /// The tick about to run.
    pub tick: u32,
    pub kit: KitLayout,
    /// Each call with the sequence number of its entry.
    pub calls: Vec<(u32, Call)>,
    pub sight: Option<Sight>,
}

/// The line-of-sight tests of one pass: every player's unit to every kit
/// zone. The eye is the unit's origin until the engine's eye marker is
/// traced (docs/game/firefight.md, "Guest boundary", line of sight).
#[derive(Clone, Debug, PartialEq)]
pub struct Sight {
    /// Each player's unit position and datum (the object the test ignores).
    pub from: Vec<([f32; 3], u32)>,
    pub zones: Vec<[f32; 3]>,
}

/// The tick's actor kills read from the simulation events, and the damage
/// that makes a kill's assist.
#[derive(Default)]
pub struct Assists {
    /// Victim object to (tick, player slot) of each damage, oldest first.
    damage: BTreeMap<u32, Vec<(u32, u8)>>,
}

impl Assists {
    /// Takes the tick's events: remembers the damage players dealt, and
    /// returns each actor kill with its killer's and assistant's slots and
    /// the killed actor's character tag datum.
    pub fn take(&mut self, tick: u32, events: &[SimEvent]) -> Vec<(u32, Option<u8>, Option<u8>)> {
        let horizon = tick.saturating_sub(ASSIST_TICKS);
        self.damage.retain(|_, hits| {
            hits.retain(|&(at, _)| at >= horizon);
            !hits.is_empty()
        });
        let mut kills = Vec::new();
        for event in events {
            match event {
                SimEvent::Damage(d) => {
                    if let Some(player) = d.owner_player.get() {
                        self.damage
                            .entry(d.victim.raw())
                            .or_default()
                            .push((d.tick.unwrap_or(tick), (player & 0xffff) as u8));
                    }
                }
                SimEvent::ActorKilled {
                    unit,
                    character,
                    killer,
                    ..
                } => {
                    let killer = killer.get().map(|k| (k & 0xffff) as u8);
                    let assist = self.damage.remove(&unit.raw()).and_then(|hits| {
                        hits.iter()
                            .rev()
                            .map(|&(_, p)| p)
                            .find(|&p| Some(p) != killer)
                    });
                    kills.push((character.raw(), killer, assist));
                }
                _ => {}
            }
        }
        kills
    }
}

/// The tick's players killed (`SimEvent::Kill`, which the game credits
/// only for players), each with its killer's and victim's roster slots as
/// an actor kill's killer is resolved. A kill with no victim player is left
/// out; one with no killer (a fall, the world) keeps none.
pub fn player_kills(events: &[SimEvent]) -> Vec<PlayerKill> {
    events
        .iter()
        .filter_map(|event| match event {
            SimEvent::Kill(k) => Some(PlayerKill {
                killer: k.attacker.get().map(|a| (a & 0xffff) as u8),
                victim: (k.victim.get()? & 0xffff) as u8,
            }),
            _ => None,
        })
        .collect()
}

/// Whether the loaded multiplayer game's variant plays teams (its team play
/// flag); none when the options cannot be read.
pub fn team_play(m: &dyn GuestMemory) -> Option<bool> {
    use bots::guest::{OPTIONS, VARIANT_FLAGS};
    let globals = m.u32(points::GAME_GLOBALS.base).ok().filter(|&g| g != 0)?;
    let flags = m.u32(globals + OPTIONS + VARIANT_FLAGS).ok()?;
    Some(flags & live_proto::hopper::variant::flag::TEAM_PLAY != 0)
}

/// Why two player teams cannot play this game, if they cannot: the variant
/// must be a team game, so the engine keeps each team's players together.
pub fn teams_refusal(teams: u8, team_play: Option<bool>) -> Option<&'static str> {
    match (teams > 1, team_play) {
        (false, _) | (true, Some(true)) => None,
        (true, Some(false)) => Some(
            "two Firefight teams need a team game, and Team Play did not turn on for this game",
        ),
        (true, None) => Some("two Firefight teams need a team game, and the variant's team play could not be read"),
    }
}

/// The options block's variant flags (`options + 0x134 + 0x48`), which
/// the game copies into the globals at activation, team play in bit 0.
pub const OPTIONS_VARIANT_FLAGS: u32 = bots::guest::VARIANT_FLAGS;

/// Whether a multiplayer game about to start on a map that carries the kit
/// (`kit`) must be a team game for Game Setup's Firefight choices: two
/// player teams with Firefight on. One team plays whatever the variant
/// says.
pub fn wants_team_play(options: &Options, kit: bool) -> bool {
    kit && options.enabled && options.player_teams() > 1
}

/// The variant flags `flags` with team play on, when `wanted` and it is
/// off; none when nothing changes.
pub fn with_team_play(flags: u32, wanted: bool) -> Option<u32> {
    use live_proto::hopper::variant::flag::TEAM_PLAY;
    (wanted && flags & TEAM_PLAY == 0).then_some(flags | TEAM_PLAY)
}

/// The Firefight match a game about to start plays, as the session data
/// names it (`NewSession.firefight`, docs/game/firefight.md, "Live"): the
/// definition Game Setup's choices give, its id, teams and difficulty, and
/// the kit version. None when Firefight is off, the map lacks the kit, or
/// the choices give no definition.
pub fn session_firefight(options: &Options, kit: bool) -> Option<live_proto::session::Firefight> {
    if !kit || !options.enabled {
        return None;
    }
    let def = options.definition().ok()?;
    let id = def.id().ok()?;
    use ::firefight::Difficulty as D;
    Some(live_proto::session::Firefight {
        set: def.name.clone(),
        id: id.to_string(),
        teams: def.teams,
        difficulty: match options.difficulty {
            D::Easy => 0,
            D::Normal => 1,
            D::Heroic => 2,
            D::Legendary => 3,
        },
        kit: ::firefight::KIT_VERSION,
    })
}

/// Where the match a game about to start plays goes: on a Live host, its
/// session's advertisement (`live_hle::Hle::set_session_firefight`).
pub type SessionSink = dyn Fn(Option<live_proto::session::Firefight>) + Send + Sync;

/// The character palette row whose tag is `character`.
fn palette_row(m: &dyn GuestMemory, character: u32) -> Option<u16> {
    let scnr = m.u32(points::SCENARIO.base).ok().filter(|&s| s != 0)?;
    let (field, size) = guest::scenario::CHARACTER_PALETTE;
    let count = m.u32(scnr + field).ok()?.min(0x7fff);
    let at = m.u32(scnr + field + 4).ok().filter(|&a| a != 0)?;
    (0..count).find_map(|i| {
        let datum = m.u32(at.wrapping_add(i * size + 4)).ok()?;
        (datum == character).then_some(i as u16)
    })
}

/// The players in the game: each listed player's roster slot, team and
/// unit, alive while its unit's body is not depleted.
pub fn read_players(m: &dyn GuestMemory) -> Result<Vec<(PlayerView, Option<u32>)>, MemFault> {
    use engine::view::{objects, players};
    let Some(array) = players::array(m)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for (handle, p) in array.live(m, 16)? {
        if !p.listed(m)? {
            continue;
        }
        let unit = p.unit(m)?;
        let body = match unit {
            Some(h) => objects::get(m, h)?.map(|(_, body)| body),
            None => None,
        };
        let (alive, position) = match body {
            Some(b) => (
                b.damage_flags(m)? & BODY_DEPLETED == 0,
                Some(b.position(m)?),
            ),
            None => (false, None),
        };
        out.push((
            PlayerView {
                slot: handle.index() as u8,
                alive,
                team: p.team(m)? as u8,
                position,
            },
            unit.map(|h| h.raw()),
        ));
    }
    Ok(out)
}

/// Live actors by kit slot.
pub fn read_actors(m: &dyn GuestMemory, kit: &KitLayout) -> [u16; SQUAD_SLOTS] {
    let mut by_squad = [0u16; SQUAD_SLOTS];
    let _ = sim_state::visit_actors(m, false, |a| {
        if let Some(slot) = kit.squads.iter().position(|s| *s == Some(a.squad)) {
            by_squad[slot] = by_squad[slot].saturating_add(1);
        }
    });
    by_squad
}

/// The game, as the host runtime reads and changes it at one boundary.
/// Reads and memory writes happen at once; the engine calls are kept in
/// `calls`, tagged with their entry, for the tick's async body.
pub struct GuestGame<'a> {
    pub m: &'a mut dyn GuestMemory,
    pub kit: Option<&'a KitLayout>,
    /// The last sight pass, by kit zone.
    pub sight: &'a [bool],
    pub kills: Vec<KillSeen>,
    pub player_kills: Vec<PlayerKill>,
    pub calls: Vec<(u32, Call)>,
    entry: u32,
    pub skulls: &'a mut BTreeSet<Skull>,
    pub sink: &'a SkullSink,
}

impl<'a> GuestGame<'a> {
    pub fn new(
        m: &'a mut dyn GuestMemory,
        kit: Option<&'a KitLayout>,
        sight: &'a [bool],
        skulls: &'a mut BTreeSet<Skull>,
        sink: &'a SkullSink,
    ) -> Self {
        Self {
            m,
            kit,
            sight,
            kills: Vec::new(),
            player_kills: Vec::new(),
            calls: Vec::new(),
            entry: 0,
            skulls,
            sink,
        }
    }

    fn defer(&mut self, call: Call) -> anyhow::Result<()> {
        self.calls.push((self.entry, call));
        Ok(())
    }
}

fn fault(e: MemFault) -> anyhow::Error {
    anyhow::anyhow!("guest memory at {:#x} faulted", e.addr)
}

impl SquadEntry for GuestGame<'_> {
    fn squad(&mut self, op: SquadOp) -> anyhow::Result<()> {
        self.defer(Call::Squad(op))
    }
}

impl Game for GuestGame<'_> {
    fn kit(&self) -> Option<Kit> {
        self.kit.map(KitLayout::kit)
    }
    fn ai(&self) -> Option<AiGlobals> {
        guest::ai_globals(&*self.m).ok().flatten()
    }
    fn switch_ai_on(&mut self) -> anyhow::Result<()> {
        guest::switch_ai_on(self.m).map(|_| ()).map_err(fault)
    }
    fn players(&self) -> Vec<PlayerView> {
        read_players(&*self.m)
            .unwrap_or_default()
            .into_iter()
            .map(|(p, _)| p)
            .collect()
    }
    fn actors_by_squad(&self) -> [u16; SQUAD_SLOTS] {
        self.kit
            .map_or([0; SQUAD_SLOTS], |kit| read_actors(&*self.m, kit))
    }
    fn sees(&self, point: [f32; 3]) -> bool {
        self.kit
            .and_then(|kit| kit.zones.iter().position(|z| z.point == point))
            .and_then(|i| self.sight.get(i).copied())
            .unwrap_or(false)
    }
    fn kills(&self) -> Vec<KillSeen> {
        self.kills.clone()
    }
    fn player_kills(&self) -> Vec<PlayerKill> {
        self.player_kills.clone()
    }
    fn credit_score(&mut self, player: u8, points: u32) -> anyhow::Result<()> {
        self.defer(Call::Score { player, points })
    }
    /// The pool is the director's ([`Host`] sets it there): the HUD reads
    /// it from the mode blob, and the engine holds every respawn by the
    /// variant's lives setting until the director's `Respawn`.
    fn set_lives(&mut self, _player: Option<u8>, _pool: u16) -> anyhow::Result<()> {
        Ok(())
    }
    /// As [`Game::set_lives`]: the engine keeps no team pool, so a team's is
    /// the director's, which spends it on that team's respawns and holds
    /// them at 0 (`Director::set_team_pool`).
    fn set_team_lives(&mut self, _team: u8, _pool: u16) -> anyhow::Result<()> {
        Ok(())
    }
    fn respawn(&mut self, player: u8) -> anyhow::Result<()> {
        self.defer(Call::Respawn(player))
    }
    fn skull(&mut self, skull: Skull, on: bool) -> anyhow::Result<()> {
        if on {
            self.skulls.insert(skull);
        } else {
            self.skulls.remove(&skull);
        }
        (self.sink)(self.skulls, self.m)
    }
    fn end_game(&mut self, _outcome: Outcome) -> anyhow::Result<()> {
        self.defer(Call::End)
    }
    fn begin_entry(&mut self, seq: u32) {
        self.entry = seq;
    }
}

/// One match on the loaded map.
pub struct Match {
    pub map: String,
    pub host: Host,
    pub kit: KitLayout,
    sight: Vec<bool>,
    sight_due: u32,
    /// The tick the last [`Runtime::pre_tick`] and [`Runtime::post_tick`]
    /// ran for.
    pre: Option<u32>,
    post: Option<u32>,
    players: u8,
    skulls: BTreeSet<Skull>,
    assists: Assists,
    /// The HUD's mode blob could not be made, and the runtime said so.
    hud_failed: bool,
    /// What the Clankers were last told of the match
    /// (`bots::firefight::set_context`), to tell them only of a change.
    bots: Option<bots::firefight::Context>,
}

impl Match {
    /// The last sight pass, by kit zone.
    pub fn sight(&self) -> &[bool] {
        &self.sight
    }
}

/// Whether the mode runs on the loaded map, and why not.
pub enum Status {
    Idle(&'static str),
    Refused { map: String, why: String },
    Running(Box<Match>),
}

/// The runtime for the whole run: one match at a time.
pub struct Runtime {
    pub status: Status,
    errors: VecDeque<wire::Refusal>,
    sink: Box<SkullSink>,
    /// What Game Setup chose, read when a game starts.
    options: Box<options::Source>,
    /// Where scores and the result go.
    reports: Box<ReportSink>,
    /// Where the match a game about to start plays is published.
    session: Box<SessionSink>,
}

/// The loaded map's name, from the cache header copy.
fn map_name(m: &dyn GuestMemory) -> String {
    let Ok(bytes) = m.bytes(guest_layout::global::CACHE_MAP_NAME.base, 32) else {
        return String::new();
    };
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    String::from_utf8_lossy(&bytes[..end]).into_owned()
}

/// This machine's role: a film's playback replays the host's entries; the
/// simulation world's mode says the rest.
pub fn role(m: &dyn GuestMemory, film: bool) -> Role {
    if film {
        return Role::Replica;
    }
    match crate::coop_look::view::mode(m) {
        Some(WORLD_SYNC_CLIENT) => Role::Replica,
        Some(WORLD_DIST_CLIENT) => Role::Client,
        _ => Role::Host,
    }
}

impl Runtime {
    pub fn new(sink: Box<SkullSink>) -> Self {
        Self {
            status: Status::Idle("no game has started"),
            errors: VecDeque::new(),
            sink,
            // Without a settings store (a headless run with none) the match
            // is the settings' defaults: Holdout, Normal, skulls on.
            options: Box::new(|| Ok(Options::default())),
            reports: Box::new(|_| {}),
            session: Box::new(|_| {}),
        }
    }

    /// Where the host's scores and result go (`Report`).
    pub fn set_reports(&mut self, reports: Box<ReportSink>) {
        self.reports = reports;
    }

    /// Where the match a game about to start plays is published
    /// ([`SessionSink`]).
    pub fn set_session(&mut self, session: Box<SessionSink>) {
        self.session = session;
    }

    /// Where the match's choices come from (`options::source`).
    pub fn set_options(&mut self, options: Box<options::Source>) {
        self.options = options;
    }

    fn error(&mut self, seq: Option<u32>, tick: u32, error: impl std::fmt::Display) {
        let row = wire::Refusal {
            seq,
            tick,
            error: error.to_string(),
        };
        eprintln!(
            "firefight: {}at tick {tick}: {}",
            seq.map_or("the AI switch ".to_string(), |s| format!(
                "host log entry {s} "
            )),
            row.error
        );
        if self.errors.len() == ERRORS_KEPT {
            self.errors.pop_front();
        }
        self.errors.push_back(row);
    }

    pub fn running(&self) -> Option<&Match> {
        match &self.status {
            Status::Running(m) => Some(m),
            _ => None,
        }
    }

    /// A game is about to start (`GAME_ACTIVATE`, before the game copies
    /// `options`, the options block, into its globals): two Firefight
    /// teams on a kit map turn the variant's team play on there, so the
    /// game plays as teams and [`teams_refusal`] has nothing to refuse.
    /// Game Setup's Firefight choices decide, as they do the match. A
    /// film's playback, another mode, a map without the kit and one team
    /// leave the options as they are. It also publishes the match this
    /// game plays ([`session_firefight`], none for any other game) to the
    /// session sink. Returns the line to log when it wrote.
    pub fn activate(&self, m: &mut dyn GuestMemory, options: u32, film: bool) -> Option<String> {
        if film || m.u32(options).ok()? != MODE_MULTIPLAYER {
            return None;
        }
        let chosen = (self.options)().ok();
        let kit = guest::read_kit(&*m).ok().flatten().is_some();
        // Every multiplayer start: a game that is not Firefight clears the
        // last one's.
        (self.session)(chosen.as_ref().and_then(|o| session_firefight(o, kit)));
        let chosen = chosen?;
        let at = options.wrapping_add(OPTIONS_VARIANT_FLAGS);
        let flags = m.u32(at).ok()?;
        let on = with_team_play(flags, wants_team_play(&chosen, kit))?;
        Some(match m.set_u32(at, on) {
            Ok(()) => "firefight: two teams: Team Play is on for this game".to_string(),
            Err(e) => format!(
                "firefight: two teams: Team Play could not be turned on: {}",
                fault(e)
            ),
        })
    }

    /// A game started. Returns the line to log.
    pub fn start(&mut self, m: &mut dyn GuestMemory, film: bool) -> String {
        self.errors.clear();
        hud::slot().set(None);
        // No match for the Clankers until one runs.
        bots::firefight::set_context(None);
        if bots::guest::game_mode(&*m) != Some(MODE_MULTIPLAYER) {
            self.status = Status::Idle("not a multiplayer game");
            return "firefight: idle: not a multiplayer game".into();
        }
        let map = map_name(&*m);
        let chosen = match (self.options)().and_then(|o| o.definition().map(|d| (o, d))) {
            Ok(chosen) => chosen,
            Err(why) => {
                let why = format!("reading Game Setup's Firefight choices: {why}");
                let line = format!("firefight: off on {map}: {why}");
                self.status = Status::Refused { map, why };
                return line;
            }
        };
        let (options, definition) = chosen;
        if !options.enabled {
            self.status = Status::Idle("Firefight is off in Game Setup");
            return "firefight: idle: Firefight is off in Game Setup".into();
        }
        if let Some(why) = teams_refusal(definition.teams, team_play(&*m)) {
            let line = format!("firefight: off on {map}: {why}");
            self.status = Status::Refused {
                map,
                why: why.into(),
            };
            return line;
        }
        let kit = match guest::read_kit(&*m) {
            Ok(kit) => kit,
            Err(e) => {
                let why = format!("reading the kit: {}", fault(e));
                let line = format!("firefight: off on {map}: {why}");
                self.status = Status::Refused { map, why };
                return line;
            }
        };
        let role = role(&*m, film);
        let mut skulls = BTreeSet::new();
        let started = {
            let game = GuestGame::new(m, kit.as_ref(), &[], &mut skulls, &*self.sink);
            Host::start(
                role,
                definition,
                Settings {
                    seed: SEED,
                    difficulty: options.difficulty,
                    skulls: BTreeSet::new(),
                },
                &game,
            )
        };
        match (started, kit) {
            (Ok(host), Some(kit)) => {
                let line = format!(
                    "firefight: on on {map} as {role:?}: {} zones, actors on team {}",
                    kit.zones.len(),
                    host.team()
                );
                let context = bot_context(&host);
                bots::firefight::set_context(context.clone());
                self.status = Status::Running(Box::new(Match {
                    map,
                    sight: vec![false; kit.zones.len()],
                    host,
                    kit,
                    sight_due: 0,
                    pre: None,
                    post: None,
                    players: 0,
                    skulls,
                    assists: Assists::default(),
                    hud_failed: false,
                    bots: context,
                }));
                line
            }
            (Err(refusal), _) => {
                let why = refusal.to_string();
                let line = format!("firefight: off on {map}: {why}");
                self.status = Status::Refused { map, why };
                line
            }
            (Ok(_), None) => unreachable!("Host::start refuses a map without the kit"),
        }
    }

    /// Before the game tick: apply what is due. The work's calls and sight
    /// pass are for the async body; none means the tick runs as it is.
    pub fn pre_tick(&mut self, m: &mut dyn GuestMemory) -> Option<Work> {
        let Status::Running(game) = &mut self.status else {
            return None;
        };
        let tick = bots::guest::game_tick(&*m)?.wrapping_add(1);
        if game.pre == Some(tick) {
            return None;
        }
        game.pre = Some(tick);
        let Match {
            host,
            kit,
            sight,
            skulls,
            ..
        } = &mut **game;
        let (calls, errors) = {
            let mut g = GuestGame::new(m, Some(&*kit), sight, skulls, &*self.sink);
            let errors = host.pre_tick(tick, &mut g);
            (std::mem::take(&mut g.calls), errors)
        };
        let sight = (game.host.director().is_some() && !game.host.held() && tick >= game.sight_due)
            .then(|| {
                game.sight_due = tick.saturating_add(SIGHT_EVERY);
                Sight {
                    from: read_players(&*m)
                        .unwrap_or_default()
                        .into_iter()
                        .filter_map(|(p, unit)| Some((p.position?, unit?)))
                        .collect(),
                    zones: game.kit.zones.iter().map(|z| z.point).collect(),
                }
            })
            .filter(|s| !s.from.is_empty() && !s.zones.is_empty());
        let kit = game.kit.clone();
        for e in errors {
            self.error(e.seq, e.tick, e.error);
        }
        (!calls.is_empty() || sight.is_some()).then_some(Work {
            tick,
            kit,
            calls,
            sight,
        })
    }

    /// The async body's outcome: the calls the engine refused and the sight
    /// pass, if one ran.
    pub fn finish(&mut self, tick: u32, refused: Vec<(u32, String)>, sight: Option<Vec<bool>>) {
        for (seq, error) in refused {
            self.error(Some(seq), tick, error);
        }
        if let (Some(sight), Status::Running(game)) = (sight, &mut self.status)
            && sight.len() == game.sight.len()
        {
            game.sight = sight;
        }
    }

    /// After the game tick: step the director over what the guest shows.
    pub fn post_tick(&mut self, m: &mut dyn GuestMemory, events: &[SimEvent]) {
        let Status::Running(game) = &mut self.status else {
            return;
        };
        let Some(tick) = bots::guest::game_tick(&*m) else {
            return;
        };
        if game.post == Some(tick) {
            return;
        }
        game.post = Some(tick);
        let player_kills = player_kills(events);
        let kills: Vec<KillSeen> = game
            .assists
            .take(tick, events)
            .into_iter()
            .filter_map(|(character, killer, assist)| {
                Some(KillSeen {
                    character: palette_row(&*m, character)?,
                    killer,
                    assist,
                })
            })
            .collect();
        let Match {
            host,
            kit,
            sight,
            skulls,
            players,
            ..
        } = &mut **game;
        let result = {
            let mut g = GuestGame::new(m, Some(&*kit), sight, skulls, &*self.sink);
            g.kills = kills;
            g.player_kills = player_kills;
            *players = g.players().len().min(usize::from(u8::MAX)) as u8;
            host.post_tick(tick, &g)
        };
        for credit in host.take_credits() {
            (self.reports)(Report::Score(score_report(credit)));
        }
        if let Some(ended) = host.take_ended() {
            (self.reports)(Report::Ended(ended_report(ended)));
        }
        // The Clankers' view of the match, when it changed.
        let context = bot_context(&game.host);
        if context != game.bots {
            bots::firefight::set_context(context.clone());
            game.bots = context;
        }
        // The HUD's mode blob, for the actor channel and the local draw.
        let mut announce = None;
        match mode_blob(&game.host, game.players) {
            Some(Ok(blob)) => {
                game.hud_failed = false;
                hud::slot().set(Some(blob));
            }
            Some(Err(e)) => {
                hud::slot().set(None);
                if !std::mem::replace(&mut game.hud_failed, true) {
                    announce = Some(format!("the HUD's mode blob: {e}"));
                }
            }
            None => {}
        }
        if let Err(e) = result {
            self.error(None, tick, format!("the director's entries: {e}"));
        }
        if let Some(line) = announce {
            self.error(None, tick, line);
        }
    }

    /// `firefight.state`.
    pub fn state(&self) -> wire::State {
        let errors = self.errors.iter().cloned().collect();
        match &self.status {
            Status::Idle(why) => wire::State {
                refused: Some((*why).into()),
                errors,
                ..Default::default()
            },
            Status::Refused { map, why } => wire::State {
                refused: Some(why.clone()),
                map: Some(map.clone()),
                errors,
                ..Default::default()
            },
            Status::Running(game) => wire::State {
                running: true,
                refused: None,
                map: Some(game.map.clone()),
                role: Some(wire_role(game.host.role())),
                team: Some(game.host.team()),
                director: game.host.director().map(|d| director(d, game)),
                log: Some(log(&game.host)),
                errors,
            },
        }
    }

    /// `firefight.command`: the director change, then the command at the
    /// next tick.
    pub fn command(
        &mut self,
        p: wire::CommandParams,
    ) -> Result<wire::CommandReply, agent::firefight::Refused> {
        use agent::firefight::Refused;
        let game = match &mut self.status {
            Status::Running(game) => game,
            Status::Idle(why) => return Err(Refused::NotRunning((*why).into())),
            Status::Refused { why, .. } => return Err(Refused::NotRunning(why.clone())),
        };
        match game.host.role() {
            Role::Host => {}
            Role::Client => {
                return Err(Refused::NotHost(
                    "a distributed client keeps the host's log and applies none of it".into(),
                ));
            }
            Role::Replica => {
                return Err(Refused::NotHost(
                    "a synchronous peer or a film's playback applies the host's entries; only the host writes the log"
                        .into(),
                ));
            }
        }
        let op = p.command.map(command).transpose()?;
        let kit = game.host.kit();
        match &op {
            Some(Command::PlaceSquad { class, zone, .. }) => {
                if !kit.classes.contains_key(class) {
                    return Err(Refused::Invalid(format!("the kit has no class {class}")));
                }
                if usize::from(*zone) >= kit.zones.len() {
                    return Err(Refused::Invalid(format!(
                        "the kit has no zone {zone} (it has {})",
                        kit.zones.len()
                    )));
                }
            }
            Some(Command::Target { team, .. })
                if usize::from(*team) >= kit.bases.len().min(MAX_TEAMS) =>
            {
                return Err(Refused::Invalid(format!(
                    "the kit has no base for team {team} (it has {})",
                    kit.bases.len()
                )));
            }
            Some(Command::TeamLives { team, .. }) if usize::from(*team) >= MAX_TEAMS => {
                return Err(Refused::Invalid(format!(
                    "there is no player team {team} (a match has at most {MAX_TEAMS})"
                )));
            }
            _ => {}
        }
        if let Some(run) = p.director {
            game.host
                .set_held(run == wire::DirectorRun::Hold)
                .map_err(|e| Refused::NotHost(e.to_string()))?;
        }
        let appended = match op {
            Some(op) => {
                let at = game.post.map_or(0, |t| t.wrapping_add(1));
                let e = game
                    .host
                    .append(at, op)
                    .map_err(|e| Refused::Invalid(e.to_string()))?;
                Some(wire::Appended {
                    seq: e.seq,
                    tick: e.tick,
                })
            }
            None => None,
        };
        Ok(wire::CommandReply {
            appended,
            held: game.host.held(),
            log: log(&game.host),
        })
    }
}

/// What the Clankers' brain is told of the match (`bots::firefight`,
/// docs/game/bots.md): the player teams' engine teams (Firefight team 0's
/// is game team 0, team 1's game team 1, the team the lobby seats a second
/// team's Clankers on), the actors' team, each team's base and the drop
/// zones, and on the host each team's pool (none when lives are not
/// shared: each player's own count is not a team's) and whether a wave is
/// on. A replica or client has no director: its lives are unknown and its
/// phase is `Fighting`. None once the director has finished the match.
pub fn bot_context(host: &Host) -> Option<bots::firefight::Context> {
    use ::firefight::director::Phase;
    use bots::firefight::{Context, Phase as BotPhase};
    let teams = usize::from(host.teams()).clamp(1, MAX_TEAMS);
    let (lives, phase) = match host.director() {
        Some(d) => {
            let s = d.state();
            let phase = match s.phase {
                Phase::Done => return None,
                Phase::Warmup { .. } | Phase::Intermission { .. } => BotPhase::Between,
                Phase::Spawning | Phase::Fighting | Phase::Cleanup { .. } => BotPhase::Fighting,
            };
            let shared = d.def().lives.shared;
            let lives = s.pools[..teams]
                .iter()
                .map(|&pool| shared.then_some(pool))
                .collect();
            (lives, phase)
        }
        None => (vec![None; teams], BotPhase::Fighting),
    };
    let kit = host.kit();
    Some(Context {
        player_teams: (0..teams as i16).collect(),
        ai_team: i16::from(host.team()),
        bases: kit.bases.clone(),
        lives,
        phase,
        drop_zones: kit.zones.clone(),
    })
}

/// The HUD's mode blob for the director on this machine (`hud`); none where
/// no director runs. The outcome is the last `Finish` in the host log.
fn mode_blob(host: &Host, players: u8) -> Option<Result<Vec<u8>, hud::Error>> {
    let director = host.director()?;
    let finished = host.log().since(0).iter().rev().find_map(|e| match e.op {
        Command::Finish { outcome } => Some(outcome),
        _ => None,
    });
    Some(hud::Mode::of(director, players, finished).map(|m| m.encode()))
}

/// `firefight.kit`, from the loaded scenario.
pub fn kit_reply(m: &dyn GuestMemory) -> Result<wire::Kit, MemFault> {
    let Some(kit) = guest::read_kit(m)? else {
        return Ok(wire::Kit::default());
    };
    Ok(wire::Kit {
        kit: true,
        slots: kit.squads.to_vec(),
        team: Some(kit.team),
        zones: kit
            .zones
            .iter()
            .map(|z| wire::KitZone {
                index: z.index,
                point: z.point,
                starts: z.starts.len(),
            })
            .collect(),
        classes: kit.kit().classes,
        bases: kit.kit().bases,
        characters: kit.characters,
        hunt_orders: kit.hunt_orders.clone(),
    })
}

/// The wire command as the director's: they share one shape.
pub fn command(c: wire::Command) -> Result<Command, agent::firefight::Refused> {
    let value = serde_json::to_value(c).expect("a wire command serialises");
    serde_json::from_value(value)
        .map_err(|e| agent::firefight::Refused::Invalid(format!("not a director command: {e}")))
}

fn wire_role(role: Role) -> wire::Role {
    match role {
        Role::Host => wire::Role::Host,
        Role::Replica => wire::Role::Replica,
        Role::Client => wire::Role::Client,
    }
}

fn log(host: &Host) -> wire::Log {
    let head = host.head();
    wire::Log {
        seq: head.seq,
        tick: head.tick,
        hash: head.hash.to_string(),
        applied: host.applied(),
    }
}

fn director(d: &::firefight::Director, game: &Match) -> wire::Director {
    use ::firefight::director::Phase;
    let s = d.state();
    let (phase, phase_tick) = match s.phase {
        Phase::Warmup { until } => ("warmup", Some(until)),
        Phase::Spawning => ("spawning", None),
        Phase::Fighting => ("fighting", None),
        Phase::Cleanup { since } => ("cleanup", Some(since)),
        Phase::Intermission { until } => ("intermission", Some(until)),
        Phase::Done => ("done", None),
    };
    let teams = usize::from(d.def().teams).clamp(1, MAX_TEAMS);
    wire::Director {
        phase: phase.into(),
        phase_tick,
        round: s.round,
        wave: s.wave,
        reserve: d.reserve(game.players),
        teams: teams as u8,
        pools: s.pools[..teams].to_vec(),
        scores: s.scores[..teams].to_vec(),
        live_actors: d.live_actors(),
        cap: d.cap(game.players),
        players: game.players,
        skulls: s
            .skulls
            .iter()
            .map(|k| {
                serde_json::to_value(k)
                    .ok()
                    .and_then(|v| v.as_str().map(str::to_owned))
                    .unwrap_or_default()
            })
            .collect(),
        held: game.host.held(),
    }
}

/// Before the tick: make the entries' engine calls in order, then the sight
/// pass. Returns the calls the engine refused, by entry, and the pass.
pub async fn make_calls(
    cx: &Cx,
    work: &Work,
    scratch: u32,
) -> (Vec<(u32, String)>, Option<Vec<bool>>) {
    let mut refused = Vec::new();
    for (seq, call) in &work.calls {
        let result = match *call {
            Call::Squad(SquadOp::Place(p)) => guest::place_squad(cx, &work.kit, p).await,
            Call::Squad(SquadOp::Hunt { slot, base }) => {
                guest::hunt(cx, &work.kit, slot, base).await
            }
            Call::Squad(SquadOp::Erase { slot }) => guest::erase_squad(cx, &work.kit, slot).await,
            Call::Score { player, points } => guest::credit_score(cx, player, points).await,
            Call::Respawn(player) => guest::respawn(cx, player).await,
            Call::End => guest::end_game(cx).await,
        };
        if let Err(e) = result {
            refused.push((*seq, e.to_string()));
        }
    }
    let sight = match &work.sight {
        Some(s) => {
            let mut seen = vec![false; s.zones.len()];
            for (i, &zone) in s.zones.iter().enumerate() {
                for &(from, unit) in &s.from {
                    // A failed test counts as no sight.
                    if guest::line_of_sight(cx, scratch, from, zone, unit)
                        .await
                        .unwrap_or(false)
                    {
                        seen[i] = true;
                        break;
                    }
                }
            }
            Some(seen)
        }
        None => None,
    };
    (refused, sight)
}

/// The async body before a tick with work: the calls, the sight pass, then
/// the tick itself (its chain runs again; `reentry` lets this hook pass).
pub async fn run_work(
    cx: Cx,
    shared: std::sync::Arc<super::feature::Shared>,
    work: Work,
    scratch: u32,
) -> Result<Ret, EntryError> {
    use std::sync::atomic::Ordering;
    let (refused, sight) = make_calls(&cx, &work, scratch).await;
    shared.lock().finish(work.tick, refused, sight);
    shared.reentry.store(true, Ordering::SeqCst);
    let r = cx.call(GuestCall::new(points::GAME_TICK.base)).await;
    shared.reentry.store(false, Ordering::SeqCst);
    Ok(Ret::new(r?.eax, 0))
}

#[cfg(test)]
mod tests;
