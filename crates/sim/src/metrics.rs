//! The metrics tally and the match report (docs/runtime/sim.md §6).
//!
//! Pure: the boot glue reads the game (a snapshot each tick, the shot, hit
//! and kill hooks, the bots' brains) and feeds plain values in here. Player
//! numbers are the game's player indices (the datum's low 16 bits).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::clock::SpeedMeter;

/// The report's format tag.
pub const FORMAT: &str = "h2-sim-report/1";

/// One player as a tick's snapshot shows them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PlayerSample {
    pub index: u8,
    pub name: String,
    pub team: Option<i32>,
    /// The controller port of a local player.
    pub controller: Option<u8>,
    /// The unit they are in, if any (the datum).
    pub unit: Option<u32>,
    /// Their unit is alive.
    pub alive: bool,
    pub in_vehicle: bool,
    /// Grenades carried, when known.
    pub grenades: Option<u32>,
    pub score: i32,
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
    pub suicides: i32,
}

/// The game after one tick.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TickSample {
    /// The game's tick count.
    pub tick: u32,
    pub players: Vec<PlayerSample>,
    /// `(team, score)`.
    pub teams: Vec<(i32, i64)>,
}

/// What a bot's brain did in one game tick (the `Metered` wrapper).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BrainTick {
    /// The unstick escalation is past `Moving`.
    pub stuck: bool,
    /// Its repaths so far this life (a running count; the tally takes the
    /// rises).
    pub repaths: u32,
    /// What it is doing (`bots::mind::Doing`, lower case).
    pub doing: &'static str,
    /// What its hand saw of the target it is tracking, if any.
    pub aim: Option<AimSample>,
}

/// A bot's hand on one view of a tracked target (`bots::mind::AimTrace`),
/// with the right stick its pad reported on the same game tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AimSample {
    /// The tick of the view. It trails the game's by the brain's
    /// perception delay, and repeats while the brain has no new view.
    pub tick: u32,
    /// Who it tracks.
    pub target: u32,
    /// From the aim to the aim point, yaw then pitch, radians, before the
    /// hand's own error.
    pub error: [f32; 2],
    pub stick: (f32, f32),
}

/// A first error this large (radians, 20 degrees) is a flick.
pub const AIM_FLICK: f32 = 0.35;
/// Settled: within this (radians) for [`AIM_SETTLE_HOLD`] views running.
pub const AIM_SETTLED: f32 = 0.05;
pub const AIM_SETTLE_HOLD: u32 = 6;
/// Overshoot is looked for this many ticks into a flick.
pub const AIM_OVERSHOOT_WINDOW: u32 = 60;
/// Views of one target this many ticks apart or less are one engagement.
pub const AIM_GAP: u32 = 3;
/// A stick this far over (of 1) has a direction, for the reversals.
pub const AIM_STICK_MOVING: f32 = 0.05;
/// Hits on one victim further apart than this (ticks) are separate
/// fights: the time to kill starts again.
pub const TTK_GAP: u32 = 150;

/// How a bot's aim moved over a match (docs/runtime/sim.md §6).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AimReport {
    /// Views of a tracked target.
    pub tracked_ticks: u64,
    /// The mean angle off the aim point while tracking, radians.
    pub mean_error: f64,
    /// Engagements that began at least [`AIM_FLICK`] off.
    pub flicks: u64,
    /// Over the flicks, the mean of how far past the target the aim went
    /// in the first two seconds, as a fraction of the flick.
    pub overshoot_ratio: f64,
    /// Flicks that settled, and the mean ticks they took to.
    pub settled: u64,
    pub settle_ticks: f64,
    /// The mean right-stick travel a tick while tracking, in stick radii.
    pub stick_jitter: f64,
    /// Changes of the stick's yaw direction a second while tracking.
    pub reversals_per_s: f64,
}

#[derive(Clone, Copy, Debug)]
struct Engagement {
    start: u32,
    first: [f32; 2],
    size: f32,
    flick: bool,
    /// The furthest past the target so far, radians.
    past: f32,
    /// Views in a row inside [`AIM_SETTLED`].
    calm: u32,
    settled: bool,
}

/// The aim-motion tally of one player.
#[derive(Clone, Debug, Default)]
struct AimMotion {
    last: Option<AimSample>,
    run: Option<Engagement>,
    tracked: u64,
    error: f64,
    pairs: u64,
    travel: f64,
    reversals: u64,
    /// The stick's last yaw direction in the engagement, 0 before any.
    turning: i8,
    flicks: u64,
    overshoot: f64,
    settled: u64,
    settle_ticks: u64,
}

impl AimMotion {
    fn close(&mut self) {
        if let Some(e) = self.run.take()
            && e.flick
        {
            self.overshoot += f64::from(e.past / e.size);
        }
    }

    fn sample(&mut self, s: AimSample) {
        if !(s.error[0].is_finite() && s.error[1].is_finite()) {
            return;
        }
        // The same view again: the brain did not think this tick.
        if self
            .last
            .is_some_and(|l| l.tick == s.tick && l.target == s.target)
        {
            return;
        }
        let same = self
            .last
            .is_some_and(|l| l.target == s.target && s.tick.wrapping_sub(l.tick) <= AIM_GAP);
        let size = s.error[0].hypot(s.error[1]);
        if !same || self.run.is_none() {
            self.close();
            let flick = size >= AIM_FLICK;
            if flick {
                self.flicks += 1;
            }
            self.run = Some(Engagement {
                start: s.tick,
                first: s.error,
                size,
                flick,
                past: 0.0,
                calm: 0,
                settled: false,
            });
            self.turning = 0;
        } else if let Some(l) = self.last {
            self.pairs += 1;
            self.travel += f64::from((s.stick.0 - l.stick.0).hypot(s.stick.1 - l.stick.1));
        }
        let turning = if s.stick.0 > AIM_STICK_MOVING {
            1
        } else if s.stick.0 < -AIM_STICK_MOVING {
            -1
        } else {
            0
        };
        if turning != 0 {
            if self.turning == -turning {
                self.reversals += 1;
            }
            self.turning = turning;
        }
        self.tracked += 1;
        self.error += f64::from(size);
        if let Some(e) = &mut self.run
            && e.flick
        {
            let age = s.tick.wrapping_sub(e.start);
            if age <= AIM_OVERSHOOT_WINDOW {
                let along = (s.error[0] * e.first[0] + s.error[1] * e.first[1]) / e.size;
                e.past = e.past.max(-along);
            }
            if !e.settled {
                if size < AIM_SETTLED {
                    e.calm += 1;
                    if e.calm == AIM_SETTLE_HOLD {
                        e.settled = true;
                        self.settled += 1;
                        // From the flick's first view to the first of the
                        // calm ones.
                        self.settle_ticks += u64::from(age.saturating_sub(AIM_SETTLE_HOLD - 1));
                    }
                } else {
                    e.calm = 0;
                }
            }
        }
        self.last = Some(s);
    }

    fn report(&self) -> Option<AimReport> {
        if self.tracked == 0 {
            return None;
        }
        let mut done = self.clone();
        done.close();
        let over = |sum: f64, n: u64| if n > 0 { sum / n as f64 } else { 0.0 };
        Some(AimReport {
            tracked_ticks: done.tracked,
            mean_error: over(done.error, done.tracked),
            flicks: done.flicks,
            overshoot_ratio: over(done.overshoot, done.flicks),
            settled: done.settled,
            settle_ticks: over(done.settle_ticks as f64, done.settled),
            stick_jitter: over(done.travel, done.pairs),
            reversals_per_s: over(done.reversals as f64 * 30.0, done.pairs),
        })
    }
}

/// How a death is classified.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KillKind {
    /// Someone else killed them.
    Kill,
    /// They killed themselves.
    Suicide,
    /// A teammate killed them.
    Betrayal,
    /// Nobody did (a fall, the map).
    World,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct KillEvent {
    pub tick: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killer: Option<u8>,
    pub victim: u8,
    pub kind: KillKind,
}

/// Why a match ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// The game's own end (score or time limit).
    GameOver,
    /// The match file's `max_ticks`.
    TickLimit,
    /// The match file's `max_wall_s`.
    WallLimit,
    /// `match.stop`.
    Stopped,
    /// The process stopped before any of the above.
    Aborted,
}

impl End {
    pub fn name(self) -> &'static str {
        match self {
            End::GameOver => "game_over",
            End::TickLimit => "tick_limit",
            End::WallLimit => "wall_limit",
            End::Stopped => "stopped",
            End::Aborted => "aborted",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Winner {
    Player(u8),
    Team(i32),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TeamResult {
    pub team: i32,
    pub score: i64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MatchResult {
    pub end: End,
    /// `None`: a tie, or nobody scored.
    pub winner: Option<Winner>,
    #[serde(default)]
    pub teams: Vec<TeamResult>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Wall {
    pub setup_s: f64,
    pub match_s: f64,
    pub process_s: f64,
    pub speedup: f64,
    pub process_speedup: f64,
    pub ms_per_tick: f64,
}

/// One player's line in the report.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PlayerReport {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<crate::config::SeatKind>,
    #[serde(default)]
    pub opposing_hits: u64,
    pub index: u8,
    pub name: String,
    /// A bot of ours.
    pub bot: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u8>,
    /// The batch arm the bot played for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub arm: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    pub score: i32,
    pub kills: i32,
    pub deaths: i32,
    pub assists: i32,
    pub suicides: i32,
    pub betrayals: u32,
    pub shots: u64,
    /// Shots' damage reports on another player's unit.
    pub hits: u64,
    /// `hits / shots` (0 with no shots). A shotgun's pellets, a
    /// grenade's splash and melee count as hits too, so it can pass 1.
    pub accuracy: f64,
    pub damage_dealt: f64,
    pub damage_taken: f64,
    pub alive_ticks: u64,
    pub vehicle_ticks: u64,
    /// Grenades thrown: the carried count falling within a life.
    pub grenades: u64,
    pub stuck_ticks: u64,
    pub repaths: u64,
    /// Ticks in an objective activity (carry, keep, play).
    pub objective_ticks: u64,
    /// Ticks per activity.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub doing: BTreeMap<String, u64>,
    /// Kills with a time to kill: the killer had hurt the victim first.
    #[serde(default)]
    pub ttk_kills: u64,
    /// Over those, the mean ticks from the killer's first hit of the
    /// fight on the victim to the kill.
    #[serde(default)]
    pub ttk_ticks: f64,
    /// How the bot's aim moved while it tracked a target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<AimReport>,
}

/// The whole report: one JSON file per match.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub format: String,
    /// The match as played (`config::Match::resolved_json`).
    #[serde(rename = "match")]
    pub match_: serde_json::Value,
    pub result: MatchResult,
    pub ticks: u64,
    pub game_seconds: f64,
    pub wall: Wall,
    pub players: Vec<PlayerReport>,
    pub kills: Vec<KillEvent>,
    /// The bots' nav mesh: `loaded` or `missing`.
    pub nav: String,
    /// Runner dispatch and preemption census when enabled for this match.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dispatch_per_tick: Option<BTreeMap<String, f64>>,
    #[serde(default)]
    pub notes: Vec<String>,
}

impl Report {
    pub fn player(&self, index: u8) -> Option<&PlayerReport> {
        self.players.iter().find(|p| p.index == index)
    }
}

/// A bot as the tally knows it.
#[derive(Clone, Debug, PartialEq)]
pub struct BotInfo {
    pub seat: u8,
    pub kind: crate::config::SeatKind,
    pub player: Option<u8>,
    pub port: Option<u8>,
    pub name: String,
    pub arm: Option<String>,
}

#[derive(Clone, Debug, Default)]
struct Acc {
    opposing_hits: u64,
    shots: u64,
    hits: u64,
    damage_dealt: f64,
    damage_taken: f64,
    alive_ticks: u64,
    vehicle_ticks: u64,
    grenades: u64,
    betrayals: u32,
    stuck_ticks: u64,
    repaths: u64,
    objective_ticks: u64,
    doing: BTreeMap<&'static str, u64>,
    /// `(unit, grenades)` last tick, for the throws.
    held: Option<(u32, u32)>,
    /// The brain's repath count last tick.
    last_repaths: u32,
    ttk_kills: u64,
    ttk_ticks: u64,
    aim: AimMotion,
}

/// A tick this far past the last one is a snapshot load, not play.
pub const JUMP_TICKS: u32 = 30;

/// The objective activities (`bots::mind::Doing`).
pub const OBJECTIVE_DOINGS: [&str; 3] = ["carry", "keep", "play"];

/// What the report needs beyond the tally.
#[derive(Clone, Debug)]
pub struct Finish<'a> {
    /// The match as played (`config::Match::resolved_json`).
    pub match_: serde_json::Value,
    pub team_play: bool,
    pub bots: &'a [BotInfo],
    pub end: End,
    /// Its `ticks` are ignored: the tally's count is used.
    pub speed: SpeedMeter,
    /// `loaded` or `missing`.
    pub nav: &'a str,
    pub notes: Vec<String>,
}

impl Finish<'_> {
    /// A finish with nothing but the end (tests, the agent's early report).
    pub fn bare(end: End) -> Finish<'static> {
        Finish {
            match_: serde_json::Value::Null,
            team_play: false,
            bots: &[],
            end,
            speed: SpeedMeter::default(),
            nav: "loaded",
            notes: Vec::new(),
        }
    }
}

/// Aggregates a match.
#[derive(Clone, Debug, Default)]
pub struct Tally {
    started: Option<u32>,
    last_tick: Option<u32>,
    ticks: u64,
    acc: BTreeMap<u8, Acc>,
    kills: Vec<KillEvent>,
    last: Option<TickSample>,
    /// Bot port → player index, from the snapshots.
    ports: BTreeMap<u8, u8>,
    resets: u32,
    /// `(attacker, victim)` → the first and the latest tick of the fight's
    /// hits, for the time to kill.
    fights: BTreeMap<(u8, u8), (u32, u32)>,
}

impl Tally {
    pub fn new() -> Tally {
        Tally::default()
    }

    /// The game started (or restarted): everything so far is dropped.
    pub fn start(&mut self, tick: u32) {
        let resets = self.resets;
        *self = Tally::default();
        self.resets = resets;
        self.started = Some(tick);
    }

    pub fn started(&self) -> bool {
        self.started.is_some()
    }

    /// Game ticks counted since the start.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }

    /// How many times the tick went backwards (a snapshot load) and the
    /// tally started again.
    pub fn resets(&self) -> u32 {
        self.resets
    }

    pub fn last(&self) -> Option<&TickSample> {
        self.last.as_ref()
    }

    /// After a game tick. Ticks move one at a time; a tick before the last
    /// one, or more than [`JUMP_TICKS`] after it, means the machine was
    /// loaded from a snapshot: the tally starts again from there.
    pub fn tick(&mut self, s: &TickSample) {
        if let Some(last) = self.last_tick
            && (s.tick < last || s.tick > last.saturating_add(JUMP_TICKS))
        {
            self.resets += 1;
            self.start(s.tick);
        }
        if self.started.is_none() {
            self.started = Some(s.tick);
        }
        if self.last_tick == Some(s.tick) {
            // The same tick again (an observer called twice): the latest
            // view, counted once.
            self.last = Some(s.clone());
            return;
        }
        self.ticks += 1;
        self.last_tick = Some(s.tick);
        for p in &s.players {
            if let Some(c) = p.controller {
                self.ports.insert(c, p.index);
            }
            let a = self.acc.entry(p.index).or_default();
            if p.alive {
                a.alive_ticks += 1;
                if p.in_vehicle {
                    a.vehicle_ticks += 1;
                }
            }
            match (p.unit, p.grenades) {
                (Some(u), Some(g)) if p.alive => {
                    if let Some((lu, lg)) = a.held
                        && lu == u
                        && g < lg
                    {
                        a.grenades += u64::from(lg - g);
                    }
                    a.held = Some((u, g));
                }
                _ => a.held = None,
            }
        }
        self.last = Some(s.clone());
    }

    /// The player on this port, from the snapshots so far.
    pub fn player_of_port(&self, port: u8) -> Option<u8> {
        self.ports.get(&port).copied()
    }

    /// A shot by `player`.
    pub fn shot(&mut self, player: u8) {
        self.acc.entry(player).or_default().shots += 1;
    }

    /// A damage report: `attacker` (if a player) hurt `victim` (if a
    /// player) by `damage` (body plus shield, in the game's units).
    pub fn hit(&mut self, attacker: Option<u8>, victim: Option<u8>, damage: f32, team_play: bool) {
        let damage = if damage.is_finite() {
            f64::from(damage.max(0.0))
        } else {
            0.0
        };
        if let Some(v) = victim {
            self.acc.entry(v).or_default().damage_taken += damage;
        }
        if let Some(a) = attacker
            && victim.is_some_and(|v| v != a)
        {
            let team = |who| {
                self.last
                    .as_ref()?
                    .players
                    .iter()
                    .find(|p| Some(p.index) == who)?
                    .team
            };
            let opposing = !team_play
                || team(attacker)
                    .zip(team(victim))
                    .is_some_and(|(a, b)| a != b);
            if opposing
                && damage > 0.0
                && let Some(v) = victim
            {
                let now = self.last_tick.map_or(0, |t| t.wrapping_add(1));
                let fight = self.fights.entry((a, v)).or_insert((now, now));
                if now.wrapping_sub(fight.1) > TTK_GAP {
                    fight.0 = now;
                }
                fight.1 = now;
            }
            let acc = self.acc.entry(a).or_default();
            if opposing && damage > 0.0 {
                acc.opposing_hits += 1;
            }
            acc.hits += 1;
            acc.damage_dealt += damage;
        }
    }

    /// A death, as `PLAYER_KILLED` gives it.
    pub fn kill(&mut self, tick: u32, killer: Option<u8>, victim: u8, betrayal: bool) {
        let kind = match killer {
            None => KillKind::World,
            Some(k) if k == victim => KillKind::Suicide,
            Some(_) if betrayal => KillKind::Betrayal,
            Some(_) => KillKind::Kill,
        };
        if kind == KillKind::Betrayal
            && let Some(k) = killer
        {
            self.acc.entry(k).or_default().betrayals += 1;
        }
        if kind == KillKind::Kill
            && let Some(k) = killer
            && let Some(&(first, _)) = self.fights.get(&(k, victim))
        {
            // The hits are stamped a tick after the last snapshot; so is
            // a kill on the same tick.
            let now = self.last_tick.map_or(tick, |t| t.wrapping_add(1));
            let a = self.acc.entry(k).or_default();
            a.ttk_kills += 1;
            a.ttk_ticks += u64::from(now.wrapping_sub(first).min(1 << 20));
        }
        // Whoever was fighting the victim starts again on its next life.
        self.fights.retain(|&(_, v), _| v != victim);
        self.kills.push(KillEvent {
            tick,
            killer,
            victim,
            kind,
        });
    }

    /// One game tick of the brain on `port`. Ignored until the snapshots
    /// have shown which player that port is.
    pub fn brain(&mut self, port: u8, b: BrainTick) {
        let Some(player) = self.player_of_port(port) else {
            return;
        };
        self.brain_for_player(player, b);
    }

    pub fn brain_for_player(&mut self, player: u8, b: BrainTick) {
        let a = self.acc.entry(player).or_default();
        if b.stuck {
            a.stuck_ticks += 1;
        }
        if b.repaths > a.last_repaths {
            a.repaths += u64::from(b.repaths - a.last_repaths);
        }
        a.last_repaths = b.repaths;
        if let Some(s) = b.aim {
            a.aim.sample(s);
        }
        if !b.doing.is_empty() {
            *a.doing.entry(b.doing).or_default() += 1;
            if OBJECTIVE_DOINGS.contains(&b.doing) {
                a.objective_ticks += 1;
            }
        }
    }

    pub fn kills(&self) -> &[KillEvent] {
        &self.kills
    }

    /// Who is winning on the last snapshot: the top team in a team game,
    /// else the top player; `None` on a tie or with no scores.
    pub fn leader(&self, team_play: bool) -> Option<Winner> {
        let last = self.last.as_ref()?;
        if team_play && !last.teams.is_empty() {
            let best = last.teams.iter().map(|t| t.1).max()?;
            let mut top = last.teams.iter().filter(|t| t.1 == best);
            let first = top.next()?;
            return if top.next().is_some() {
                None
            } else {
                Some(Winner::Team(first.0))
            };
        }
        let best = last.players.iter().map(|p| p.score).max()?;
        let mut top = last.players.iter().filter(|p| p.score == best);
        let first = top.next()?;
        if top.next().is_some() {
            None
        } else {
            Some(Winner::Player(first.index))
        }
    }

    /// The report as it stands.
    pub fn report(&self, f: Finish<'_>) -> Report {
        let Finish {
            match_,
            team_play,
            bots,
            end,
            speed,
            nav,
            notes,
        } = f;
        let mut players = Vec::new();
        let empty = TickSample::default();
        let last = self.last.as_ref().unwrap_or(&empty);
        let mut indices: Vec<u8> = last.players.iter().map(|p| p.index).collect();
        for i in self.acc.keys() {
            if !indices.contains(i) {
                indices.push(*i);
            }
        }
        indices.sort_unstable();
        for i in indices {
            let sample = last.players.iter().find(|p| p.index == i);
            let a = self.acc.get(&i).cloned().unwrap_or_default();
            let port = sample.and_then(|p| p.controller);
            let bot = bots.iter().find(|b| {
                b.player == Some(i) || (b.player.is_none() && b.port.is_some() && b.port == port)
            });
            let shots = a.shots;
            players.push(PlayerReport {
                seat: bot.map(|b| b.seat),
                kind: bot.map(|b| b.kind),
                opposing_hits: a.opposing_hits,
                index: i,
                name: sample
                    .map(|p| p.name.clone())
                    .filter(|n| !n.is_empty())
                    .or_else(|| bot.map(|b| b.name.clone()))
                    .unwrap_or_else(|| format!("player {i}")),
                bot: bot.is_some(),
                port,
                arm: bot.and_then(|b| b.arm.clone()),
                team: sample.and_then(|p| p.team),
                score: sample.map_or(0, |p| p.score),
                kills: sample.map_or(0, |p| p.kills),
                deaths: sample.map_or(0, |p| p.deaths),
                assists: sample.map_or(0, |p| p.assists),
                suicides: sample.map_or(0, |p| p.suicides),
                betrayals: a.betrayals,
                shots,
                hits: a.hits,
                accuracy: if shots > 0 {
                    a.hits as f64 / shots as f64
                } else {
                    0.0
                },
                damage_dealt: a.damage_dealt,
                damage_taken: a.damage_taken,
                alive_ticks: a.alive_ticks,
                vehicle_ticks: a.vehicle_ticks,
                grenades: a.grenades,
                stuck_ticks: a.stuck_ticks,
                repaths: a.repaths,
                objective_ticks: a.objective_ticks,
                doing: a.doing.iter().map(|(k, v)| (k.to_string(), *v)).collect(),
                ttk_kills: a.ttk_kills,
                ttk_ticks: if a.ttk_kills > 0 {
                    a.ttk_ticks as f64 / a.ttk_kills as f64
                } else {
                    0.0
                },
                aim: a.aim.report(),
            });
        }
        let teams = last
            .teams
            .iter()
            .map(|&(team, score)| TeamResult { team, score })
            .collect();
        let mut speed = speed;
        speed.ticks = self.ticks;
        let mut notes = notes;
        if self.resets > 0 {
            notes.push(format!(
                "the tally started again {} time(s): the tick jumped (a snapshot load)",
                self.resets
            ));
        }
        Report {
            format: FORMAT.into(),
            match_,
            result: MatchResult {
                end,
                winner: self.leader(team_play),
                teams,
            },
            ticks: self.ticks,
            game_seconds: speed.game_seconds(),
            wall: Wall {
                setup_s: speed.setup_s,
                match_s: speed.match_s,
                process_s: speed.setup_s + speed.match_s,
                speedup: speed.speedup(),
                process_speedup: speed.process_speedup(),
                ms_per_tick: speed.ms_per_tick(),
            },
            players,
            kills: self.kills.clone(),
            nav: nav.into(),
            dispatch_per_tick: None,
            notes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(index: u8, controller: Option<u8>, score: i32) -> PlayerSample {
        PlayerSample {
            index,
            name: format!("P{index}"),
            team: Some(i32::from(index % 2)),
            controller,
            unit: Some(0x1000 + u32::from(index)),
            alive: true,
            grenades: Some(2),
            score,
            ..PlayerSample::default()
        }
    }

    fn sample(tick: u32, players: Vec<PlayerSample>) -> TickSample {
        TickSample {
            tick,
            players,
            teams: vec![],
        }
    }

    #[test]
    fn counts_ticks_alive_and_vehicles() {
        let mut t = Tally::new();
        t.start(10);
        for tick in 10..20 {
            let mut a = p(0, Some(0), 0);
            a.in_vehicle = tick >= 15;
            let mut b = p(1, Some(1), 0);
            b.alive = tick < 12;
            t.tick(&sample(tick, vec![a, b]));
        }
        // The same tick twice counts once.
        t.tick(&sample(19, vec![p(0, Some(0), 0)]));
        assert_eq!(t.ticks(), 10);
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::TickLimit)
        });
        let a = r.player(0).unwrap();
        assert_eq!(a.alive_ticks, 10);
        assert_eq!(a.vehicle_ticks, 5);
        assert_eq!(r.player(1).unwrap().alive_ticks, 2);
    }

    #[test]
    fn grenades_fall_within_a_life() {
        let mut t = Tally::new();
        t.start(0);
        let with = |tick, unit, g| {
            let mut a = p(0, Some(0), 0);
            a.unit = Some(unit);
            a.grenades = Some(g);
            sample(tick, vec![a])
        };
        t.tick(&with(0, 1, 4));
        t.tick(&with(1, 1, 3)); // threw one
        t.tick(&with(2, 1, 4)); // picked one up
        t.tick(&with(3, 1, 2)); // threw two
        t.tick(&with(4, 2, 2)); // new life: no throw
        t.tick(&with(5, 2, 1)); // threw one
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::Stopped)
        });
        assert_eq!(r.player(0).unwrap().grenades, 4);
    }

    #[test]
    fn shots_hits_and_damage() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, Some(0), 0), p(1, Some(1), 0)]));
        for _ in 0..4 {
            t.shot(0);
        }
        t.hit(Some(0), Some(1), 0.25, false);
        t.hit(Some(0), Some(0), 0.5, false); // own grenade: taken, not dealt
        t.hit(None, Some(1), 1.0, false); // the world
        t.hit(Some(0), None, 1.0, false); // a vehicle or a crate
        t.hit(Some(0), Some(1), f32::NAN, false);
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::Stopped)
        });
        let a = r.player(0).unwrap();
        assert_eq!((a.shots, a.hits), (4, 2));
        assert_eq!(a.accuracy, 0.5);
        assert_eq!(a.opposing_hits, 1);
        assert_eq!(a.damage_dealt, 0.25);
        assert_eq!(a.damage_taken, 0.5);
        assert_eq!(r.player(1).unwrap().damage_taken, 1.25);
    }

    #[test]
    fn opposing_hits_exclude_friendly_self_and_zero_damage() {
        let mut t = Tally::new();
        let mut players = vec![p(0, Some(0), 0), p(1, None, 0), p(2, None, 0)];
        for (p, team) in players.iter_mut().zip([0, 0, 1]) {
            p.team = Some(team);
        }
        t.tick(&sample(1, players));
        for (v, dmg) in [(0, 1.), (1, 2.), (2, 3.), (2, 0.)] {
            t.hit(Some(0), Some(v), dmg, true);
        }
        let r = t.report(Finish::bare(End::GameOver));
        assert_eq!(r.player(0).unwrap().opposing_hits, 1);
        assert_eq!(r.player(0).unwrap().damage_dealt, 5.);
        assert_eq!(r.player(2).unwrap().damage_taken, 3.);
    }

    #[test]
    fn kills_are_classified() {
        let mut t = Tally::new();
        t.start(0);
        t.kill(5, Some(0), 1, false);
        t.kill(6, Some(1), 1, false);
        t.kill(7, Some(2), 0, true);
        t.kill(8, None, 2, false);
        let kinds: Vec<_> = t.kills().iter().map(|k| k.kind).collect();
        assert_eq!(
            kinds,
            [
                KillKind::Kill,
                KillKind::Suicide,
                KillKind::Betrayal,
                KillKind::World
            ]
        );
        let r = t.report(Finish {
            team_play: true,
            ..Finish::bare(End::GameOver)
        });
        assert_eq!(r.player(2).unwrap().betrayals, 1);
    }

    #[test]
    fn brains_count_by_port_once_known() {
        let mut t = Tally::new();
        t.start(0);
        let busy = |stuck, repaths, doing| BrainTick {
            stuck,
            repaths,
            doing,
            aim: None,
        };
        // Before any snapshot the port is unknown: dropped.
        t.brain(2, busy(true, 0, "fight"));
        t.tick(&sample(0, vec![p(5, Some(2), 0)]));
        t.brain(2, busy(true, 1, "fight"));
        t.brain(2, busy(false, 1, "carry"));
        t.brain(2, busy(false, 0, "roam")); // new life: count restarts
        t.brain(2, busy(false, 2, "roam"));
        let bots = [BotInfo {
            seat: 2,
            kind: crate::config::SeatKind::Local,
            player: None,
            port: Some(2),
            name: "Clanker 2".into(),
            arm: Some("b".into()),
        }];
        let r = t.report(Finish {
            bots: &bots,
            ..Finish::bare(End::Stopped)
        });
        let b = r.player(5).unwrap();
        assert!(b.bot);
        assert_eq!(b.port, Some(2));
        assert_eq!(b.arm.as_deref(), Some("b"));
        assert_eq!(b.stuck_ticks, 1);
        assert_eq!(b.repaths, 3);
        assert_eq!(b.objective_ticks, 1);
        assert_eq!(b.doing.get("roam"), Some(&2));
        assert_eq!(b.doing.get("fight"), Some(&1));
    }

    #[test]
    fn going_back_in_time_restarts() {
        let mut t = Tally::new();
        t.start(100);
        t.tick(&sample(100, vec![p(0, Some(0), 0)]));
        t.shot(0);
        t.tick(&sample(101, vec![p(0, Some(0), 0)]));
        t.tick(&sample(50, vec![p(0, Some(0), 0)]));
        assert_eq!(t.resets(), 1);
        assert_eq!(t.ticks(), 1);
        let r = t.report(Finish {
            team_play: false,
            ..Finish::bare(End::Stopped)
        });
        assert_eq!(r.player(0).unwrap().shots, 0);
        assert_eq!(r.notes.len(), 1);
        // Forward past a second's worth of ticks: a load too.
        t.tick(&sample(51, vec![]));
        t.tick(&sample(51 + JUMP_TICKS, vec![]));
        assert_eq!((t.resets(), t.ticks()), (1, 3));
        t.tick(&sample(52 + 2 * JUMP_TICKS, vec![]));
        assert_eq!((t.resets(), t.ticks()), (2, 1));
    }

    #[test]
    fn time_to_kill_runs_from_the_fights_first_hit() {
        let mut t = Tally::new();
        t.start(0);
        let both = || vec![p(0, Some(0), 0), p(1, Some(1), 0)];
        // Player 0 first hurts 1 on tick 11, again on 31, and kills on 41.
        for tick in 0..=40 {
            t.tick(&sample(tick, both()));
            if tick == 10 || tick == 30 {
                t.hit(Some(0), Some(1), 0.3, false);
            }
        }
        t.kill(41, Some(0), 1, false);
        // A new life: one hit, a long gap, then a second fight of 21 ticks.
        for tick in 41..=300 {
            t.tick(&sample(tick, both()));
            if tick == 50 || tick == 279 {
                t.hit(Some(0), Some(1), 0.3, false);
            }
            // No damage and a hit on itself start nothing.
            if tick == 45 {
                t.hit(Some(1), Some(0), 0.0, false);
                t.hit(Some(1), Some(1), 0.5, false);
            }
        }
        t.kill(300, Some(0), 1, false);
        // Killed with nothing landed first (a fall, a stick): no time.
        t.kill(300, Some(1), 0, false);
        t.kill(300, None, 1, false);
        let r = t.report(Finish::bare(End::GameOver));
        let a = r.player(0).unwrap();
        assert_eq!(a.ttk_kills, 2);
        assert_eq!(a.ttk_ticks, (30.0 + 21.0) / 2.0);
        let b = r.player(1).unwrap();
        assert_eq!((b.ttk_kills, b.ttk_ticks), (0, 0.0));
    }

    /// A view a tick: the error, and a stick that follows its sign.
    fn aim(t: &mut Tally, tick: u32, target: u32, yaw: f32, stick: f32) {
        t.brain_for_player(
            0,
            BrainTick {
                doing: "fight",
                aim: Some(AimSample {
                    tick,
                    target,
                    error: [yaw, 0.0],
                    stick: (stick, 0.0),
                }),
                ..BrainTick::default()
            },
        );
    }

    #[test]
    fn aim_motion_measures_overshoot_settle_and_jitter() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, Some(0), 0)]));
        // A flick from 0.8: ten views closing, four past the target (0.12
        // at the worst), then on it.
        let mut tick = 100;
        let mut errors: Vec<f32> = (0..10).map(|i| 0.8 - 0.08 * i as f32).collect();
        errors.extend([-0.04, -0.12, -0.08, -0.02]);
        errors.extend([0.01; 12]);
        for e in &errors {
            let stick = if *e > 0.03 {
                -0.5
            } else if *e < -0.03 {
                0.5
            } else {
                0.0
            };
            aim(&mut t, tick, 7, *e, stick);
            // The brain's view repeated on a tick it did not think: once.
            aim(&mut t, tick, 7, *e, 0.9);
            tick += 1;
        }
        // Another target, close already: an engagement, not a flick.
        for i in 0..5 {
            aim(&mut t, tick + i, 8, 0.1, -0.2);
        }
        let r = t.report(Finish::bare(End::GameOver));
        let a = r.player(0).unwrap().aim.clone().unwrap();
        assert_eq!(a.tracked_ticks, errors.len() as u64 + 5);
        assert_eq!(a.flicks, 1);
        assert!((a.overshoot_ratio - 0.12 / 0.8).abs() < 1e-6, "{a:?}");
        // Views 13.. are inside 0.05; the sixth of them settles it.
        assert_eq!((a.settled, a.settle_ticks), (1, 13.0));
        // One reversal past the target; the stick moved on two views of
        // the first engagement's 25 pairs and none of the second's four.
        let pairs = (errors.len() - 1 + 4) as f64;
        assert!((a.reversals_per_s - 30.0 / pairs).abs() < 1e-9, "{a:?}");
        assert!((a.stick_jitter - (1.0 + 0.5) / pairs).abs() < 1e-6, "{a:?}");
        let mean = (errors.iter().map(|e| e.abs()).sum::<f32>() + 0.5) / (errors.len() + 5) as f32;
        assert!((a.mean_error - f64::from(mean)).abs() < 1e-5, "{a:?}");
        // The report as it stands does not close the tally's engagement.
        let again = t.report(Finish::bare(End::GameOver));
        assert_eq!(again.player(0).unwrap().aim, Some(a));
    }

    #[test]
    fn aim_motion_splits_engagements_and_leaves_out_what_never_settles() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, Some(0), 0)]));
        // Never inside 0.05 for six views: a flick that does not settle.
        for i in 0..40 {
            aim(&mut t, i, 3, if i % 4 == 1 { 0.02 } else { 0.5 }, -1.0);
        }
        // The same target after a gap is a new flick; it overshoots by
        // half, and only what happens in the first two seconds counts.
        for i in 0..80 {
            let e = match i {
                0 => 0.4,
                1..=9 => -0.2,
                70 => -0.4,
                _ => 0.0,
            };
            aim(&mut t, 50 + i, 3, e, 0.0);
        }
        // A view that is not a number is dropped.
        aim(&mut t, 500, 3, f32::NAN, 0.0);
        let a = t
            .report(Finish::bare(End::GameOver))
            .player(0)
            .unwrap()
            .aim
            .clone()
            .unwrap();
        assert_eq!(a.tracked_ticks, 120);
        assert_eq!(a.flicks, 2);
        assert!((a.overshoot_ratio - 0.25).abs() < 1e-6, "{a:?}");
        assert_eq!((a.settled, a.settle_ticks), (1, 10.0));
        assert_eq!(a.reversals_per_s, 0.0);
        // A player with no tracked view has no aim section at all.
        let mut none = Tally::new();
        none.tick(&sample(0, vec![p(0, Some(0), 0)]));
        none.brain_for_player(0, BrainTick::default());
        let r = none.report(Finish::bare(End::GameOver));
        assert_eq!(r.player(0).unwrap().aim, None);
        let v = serde_json::to_value(&r).unwrap();
        assert!(v["players"][0].get("aim").is_none());
        assert_eq!(v["players"][0]["ttk_kills"], 0);
    }

    #[test]
    fn earlier_reports_still_load() {
        // A player line as written before the time to kill and the aim
        // section existed.
        let old = serde_json::json!({
            "index": 0, "name": "A1", "bot": true, "score": 3, "kills": 3, "deaths": 1,
            "assists": 0, "suicides": 0, "betrayals": 0, "shots": 10, "hits": 4,
            "accuracy": 0.4, "damage_dealt": 2.0, "damage_taken": 1.0, "alive_ticks": 9,
            "vehicle_ticks": 0, "grenades": 0, "stuck_ticks": 0, "repaths": 0,
            "objective_ticks": 0
        });
        let p: PlayerReport = serde_json::from_value(old).unwrap();
        assert_eq!((p.ttk_kills, p.ttk_ticks, p.aim.clone()), (0, 0.0, None));
        // And the new fields round trip.
        let new = PlayerReport {
            ttk_kills: 2,
            ttk_ticks: 25.5,
            aim: Some(AimReport {
                tracked_ticks: 30,
                mean_error: 0.04,
                flicks: 1,
                overshoot_ratio: 0.15,
                settled: 1,
                settle_ticks: 13.0,
                stick_jitter: 0.08,
                reversals_per_s: 1.2,
            }),
            ..p
        };
        let v = serde_json::to_value(&new).unwrap();
        assert_eq!(v["aim"]["overshoot_ratio"], 0.15);
        assert_eq!(serde_json::from_value::<PlayerReport>(v).unwrap(), new);
    }

    #[test]
    fn winners() {
        let mut t = Tally::new();
        t.start(0);
        t.tick(&sample(0, vec![p(0, None, 3), p(1, None, 5)]));
        assert_eq!(t.leader(false), Some(Winner::Player(1)));
        t.tick(&sample(1, vec![p(0, None, 5), p(1, None, 5)]));
        assert_eq!(t.leader(false), None);
        let mut s = sample(2, vec![p(0, None, 5), p(1, None, 5)]);
        s.teams = vec![(0, 7), (1, 9)];
        t.tick(&s);
        assert_eq!(t.leader(true), Some(Winner::Team(1)));
        assert_eq!(Tally::new().leader(false), None);
    }

    #[test]
    fn report_round_trips_as_json() {
        let mut t = Tally::new();
        t.start(0);
        let mut s = sample(0, vec![p(0, Some(0), 1)]);
        s.teams = vec![(0, 1)];
        t.tick(&s);
        t.kill(0, Some(0), 0, false);
        let speed = SpeedMeter {
            setup_s: 1.0,
            match_s: 0.5,
            ticks: 999,
        };
        let r = t.report(Finish {
            match_: serde_json::json!({"map": "lockout"}),
            speed,
            ..Finish::bare(End::GameOver)
        });
        // The meter's ticks are the tally's.
        assert_eq!(r.ticks, 1);
        assert_eq!(r.wall.process_s, 1.5);
        let v = serde_json::to_value(&r).unwrap();
        assert_eq!(v["format"], FORMAT);
        assert_eq!(v["match"]["map"], "lockout");
        assert_eq!(v["result"]["end"], "game_over");
        assert_eq!(v["result"]["winner"], serde_json::json!({"player": 0}));
        assert_eq!(v["kills"][0]["kind"], "suicide");
        let back: Report = serde_json::from_value(v).unwrap();
        assert_eq!(back, r);
    }
}
