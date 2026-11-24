//! Generated test matches: a script played by made-up players on the
//! [`FakeGame`](crate::fake::FakeGame) until the game ends, one match or a
//! batch of seeds with a summary. `h2 rules sim` and Live's
//! `/v1/rules/simulate` share it, so the same script, setup and seed give
//! the same match everywhere. See `docs/game/rules-engine.md`, "Workbench".
//!
//! A match is a [`workbench::Scenario`] with the trace of running it:
//! [`workbench::run`] over the scenario plays the same match again, so a
//! generated match can be saved and edited by hand.
//!
//! The director reads the script before it plays: the events it handles,
//! the state it reads, its carrier and its labels. It reports only what
//! the script can notice ([`emits`]).

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::caps::{self, id};
use crate::fake::FakeGame;
use crate::ir::{Event, Ir, LabelKind, Op};
use crate::labels::{self, Class};
use crate::scenario::{self as file, ScenarioLabel};
use crate::vm::{self, Input};
use crate::workbench::{
    self, Keep, Limit, MAX_STEPS, MAX_TICKS, RunError, Runner, Scenario, Seat, Step, Trace,
    handler_name, unit_of,
};

/// The most matches a batch plays.
pub const MAX_MATCHES: u32 = 100;
/// The ops a batch spends before it starts no further match.
pub const BATCH_OPS: u64 = 24_000_000;
/// The ticks a batch runs before it starts no further match.
pub const BATCH_TICKS: u64 = 400_000;
/// Seconds a match lasts when the setup gives no length.
pub const DEFAULT_LENGTH: u32 = 300;
/// The longest match, in seconds ([`MAX_TICKS`]).
pub const MAX_LENGTH: u32 = MAX_TICKS / TPS;

const TPS: u32 = caps::TICKS_PER_SECOND as u32;
/// A living player of weight 100 makes a kill every 8 seconds.
const KILL_ODDS: u64 = 8 * TPS as u64 * 100;
/// Seconds a dead player waits when the record gives no respawn time.
const RESPAWN_SECONDS: u32 = 5;
/// The first declared object, the first bounce spot and the first edited
/// object.
const OBJECT_BASE: u32 = file::MAP_BASE;
const SPOT_BASE: u32 = 0x200;
const EDIT_BASE: u32 = 0x300;
/// Actor classes an `on actor_kill` is reported with, when the script
/// names one of them.
const CLASSES: [&str; 8] = ["grunt", "jackal", "elite", "hunter", "brute", "drone", "flood", "sentinel"];

/// What a match is played with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Setup {
    /// Players seated, 1 to 16.
    pub players: u8,
    /// 0 or 1 is free for all; 2 to 8 is team play with that many teams.
    pub teams: u8,
    pub seed: u64,
    /// Seconds the match lasts at most (the record's time limit);
    /// [`DEFAULT_LENGTH`] when absent.
    pub length: Option<u32>,
    /// The record's score limit; the carrier's usual one when absent.
    pub score_limit: Option<i32>,
}

impl Default for Setup {
    fn default() -> Setup {
        Setup {
            players: 8,
            teams: 0,
            seed: 1,
            length: None,
            score_limit: None,
        }
    }
}

fn check(setup: &Setup) -> Result<(), RunError> {
    let bad = |m: String| Err(RunError::Scenario(m));
    if !(1..=16).contains(&setup.players) {
        return bad("a match seats 1 to 16 players".into());
    }
    if setup.teams > 8 {
        return bad("a match has at most 8 teams".into());
    }
    if setup.length.is_some_and(|s| s == 0 || s > MAX_LENGTH) {
        return bad(format!("a match lasts 1 to {MAX_LENGTH} seconds"));
    }
    if setup.score_limit.is_some_and(|s| !(1..=10_000).contains(&s)) {
        return bad("a score limit is 1 to 10000".into());
    }
    Ok(())
}

/// How a match ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum End {
    /// The script called `end_round`.
    Script,
    /// The record's score limit was reached.
    ScoreLimit,
    /// The record's time limit ran out.
    TimeLimit,
    /// The script stopped on an error.
    Error,
    /// A cap stopped it first: the game never ended.
    Cap,
}

impl End {
    pub fn words(self) -> &'static str {
        match self {
            End::Script => "the script ended it",
            End::ScoreLimit => "score limit",
            End::TimeLimit => "time limit",
            End::Error => "script error",
            End::Cap => "never ended",
        }
    }
}

/// Who won a match.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(tag = "kind", content = "index", rename_all = "snake_case")]
pub enum Winner {
    Player(u8),
    Team(u8),
    /// The top score is shared.
    Draw,
    /// The match did not end.
    Nobody,
}

impl Winner {
    pub fn words(self) -> String {
        match self {
            Winner::Player(p) => format!("player {p}"),
            Winner::Team(t) => format!("team {t}"),
            Winner::Draw => "a draw".into(),
            Winner::Nobody => "nobody".into(),
        }
    }
}

/// One generated match.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Match {
    pub seed: u64,
    /// The teams played with: 0 for free for all. A script or a carrier
    /// that needs teams has them whatever the setup asked.
    pub teams: u8,
    /// The match as a scenario: [`workbench::run`] plays it again.
    pub scenario: Scenario,
    pub trace: Trace,
    pub end: End,
    /// Why, for [`End::Error`] and [`End::Cap`].
    pub why: Option<String>,
    pub winner: Winner,
    /// The winning score: the best player's, or the best team's sum.
    pub top_score: i32,
    /// The lowest player score.
    pub low_score: i32,
}

// ---------- reading the script ----------

fn calls(ir: &Ir, entry: u16) -> bool {
    ir.handlers.iter().any(|h| {
        h.code
            .iter()
            .any(|op| matches!(op, Op::Call { entry: e, .. } if *e == entry))
    })
}

/// What the script can notice.
#[derive(Clone, Debug)]
struct Wants {
    handles: BTreeSet<Event>,
    /// Kills and respawns: it handles a kill or a death, reads who is
    /// alive or the kill counts, or has things to carry.
    combat: bool,
    /// Players walk into and out of the labelled objects: it reads
    /// `inside` or `ticks_held`, or handles `enter` or `leave`.
    zones: bool,
    /// Players pick objectives up and drop them: it handles `pickup` or
    /// `drop`, or reads an objective's carrier.
    carry: bool,
    /// The script respawns the dead itself.
    respawns: bool,
}

impl Wants {
    fn of(ir: &Ir) -> Wants {
        let handles: BTreeSet<Event> = ir.handlers.iter().map(|h| h.event).collect();
        let on = |e: Event| handles.contains(&e);
        let carry = on(Event::Pickup) || on(Event::Drop) || calls(ir, id::CARRIER_OF);
        let combat = on(Event::Kill)
            || on(Event::Death)
            || calls(ir, id::ALIVE)
            || calls(ir, id::KILLS)
            || calls(ir, id::DEATHS)
            || carry;
        let zones = on(Event::Enter)
            || on(Event::Leave)
            || calls(ir, id::INSIDE)
            || calls(ir, id::TICKS_HELD);
        Wants {
            combat,
            zones,
            carry,
            respawns: calls(ir, id::RESPAWN),
            handles,
        }
    }

    fn on(&self, e: Event) -> bool {
        self.handles.contains(&e)
    }
}

/// The events a generated match of `ir` may hold: each one the script has
/// a handler for or reads the state of. `init` and `round_start` open
/// every run and are not listed; `round_end` is never reported.
pub fn emits(ir: &Ir) -> Vec<Event> {
    let w = Wants::of(ir);
    let mut out = Vec::new();
    for e in [
        Event::Press,
        Event::SquadDead,
        Event::ActorKill,
        Event::ActorAssist,
        Event::Bounce,
        Event::Edit,
        Event::Quit,
    ] {
        if w.on(e) {
            out.push(e);
        }
    }
    if w.on(Event::Join) && w.on(Event::Quit) {
        out.push(Event::Join);
    }
    if w.combat {
        out.push(Event::Kill);
        if w.on(Event::Death) {
            out.push(Event::Death);
        }
    }
    if w.combat || w.on(Event::Spawn) {
        out.push(Event::Spawn);
    }
    if w.zones {
        out.extend([Event::Enter, Event::Leave]);
    }
    if w.carry {
        out.extend([Event::Pickup, Event::Drop]);
    }
    out.sort();
    out
}

/// The teams a match of `ir` is played with: the script's `teams n`, two
/// for a carrier that is nothing without them (CTF and Assault), else the
/// setup's. 0 is free for all.
fn teams_for(ir: &Ir, setup: &Setup) -> u8 {
    if ir.header.teams >= 2 {
        ir.header.teams.min(8)
    } else if setup.teams >= 2 {
        setup.teams
    } else if matches!(ir.header.engine(), 1 | 9) {
        2
    } else {
        0
    }
}

/// The record a match is played under: the carrier's usual limits, sized
/// so a match ends in a few minutes.
fn variant(ir: &Ir, setup: &Setup, teams: u8) -> BTreeMap<String, i32> {
    let carrier = ir.header.engine();
    let team_play = teams >= 2;
    let score = setup.score_limit.unwrap_or(match carrier {
        1 | 9 => 5,
        2 if team_play => 50,
        3 | 4 => 30,
        7 => 10,
        8 => 100,
        _ => 25,
    });
    let mut v: BTreeMap<String, i32> = BTreeMap::new();
    let mut set = |k: &str, n: i32| {
        v.insert(k.to_string(), n);
    };
    set("team_play", team_play as i32);
    set("score_limit", score);
    set("time_limit", setup.length.unwrap_or(DEFAULT_LENGTH) as i32);
    set("respawn_time", RESPAWN_SECONDS as i32);
    match carrier {
        1 | 9 => {
            set("ctf_reset_time", 10);
            set("ctf_touch_return", 1);
            set("assault_arm_time", 3);
        }
        3 => set("oddball_ball_count", 1),
        4 => set("king_moving_hill", 30),
        8 => {
            set("territories_count", 3);
            set("territories_contest_time", 3);
        }
        _ => {}
    }
    v
}

/// The map a match is played on: [`labels::minimal`] for the script's label
/// table, so each label has its kind, its count, its numbers and a member
/// for each team the label asks for, and the match is never a blocked one.
/// A label the minimal map leaves empty (`objects`, `at most n`) gets a
/// member for each team in play, numbered in turn when it is numbered, so the match plays what the label is
/// for: an Oddball map with no ball spawn, or a CTF map with no team's
/// flag spawn, scores nothing. A free-for-all match has teams 0 and 1,
/// which only a `per team` label reads.
fn objects(ir: &Ir, teams: u8) -> Vec<ScenarioLabel> {
    let in_play: Vec<u8> = if teams >= 2 { (0..teams).collect() } else { vec![0, 1] };
    let mut census = labels::minimal(ir, &in_play);
    for def in ir.labels.iter().filter(|d| d.max >= 1) {
        census.labels.entry(def.text.clone()).or_insert_with(|| {
            let class = match def.kind {
                LabelKind::Object => Class::Object,
                LabelKind::Zone => Class::Volume,
                LabelKind::Spawn => Class::Spawn,
            };
            let first = def.range().map_or(0, |r| *r.start());
            in_play
                .iter()
                .take(def.max as usize)
                .enumerate()
                .map(|(i, &t)| labels::Member {
                    class,
                    number: def.numbered().then_some(first + i as i32),
                    team: Some(t),
                })
                .collect()
        });
    }
    file::things_of(&census, OBJECT_BASE)
}

// ---------- the director ----------

struct Bot {
    slot: u8,
    /// 60 to 160: how often this player gets the kill.
    weight: u64,
    /// Seated: not after a quit.
    here: bool,
    alive: bool,
    respawn_at: Option<u32>,
    /// The object stood in, by index.
    at: Option<usize>,
    /// The last object visited, by index.
    last: Option<usize>,
    /// Has a unit to walk with: a player who quit and joined again has
    /// none (`FakeGame::after`).
    walks: bool,
}

struct Director {
    wants: Wants,
    objects: Vec<ScenarioLabel>,
    bots: Vec<Bot>,
    rng: u64,
    carrier: u8,
    team_play: bool,
    respawn_ticks: u32,
    quit_at: Option<(u32, usize)>,
    join_at: Option<(u32, usize)>,
    classes: Vec<String>,
}

impl Director {
    fn new(ir: &Ir, scenario: &Scenario) -> Director {
        let wants = Wants::of(ir);
        let field = |name: &str| scenario.variant.get(name).copied().unwrap_or(0);
        let mut d = Director {
            // A starting location is no place to stand in.
            objects: scenario
                .map
                .iter()
                .flatten()
                .filter(|o| o.class_in(ir) != Class::Spawn)
                .cloned()
                .collect(),
            bots: Vec::new(),
            // Apart from the VM's own stream, which the match seed also
            // starts.
            rng: scenario.seed ^ 0x6d61_7463_685f_6765,
            carrier: ir.header.engine(),
            team_play: field("team_play") != 0,
            respawn_ticks: match u32::try_from(field("respawn_time")) {
                Ok(s) if s > 0 => s * TPS,
                _ => RESPAWN_SECONDS * TPS,
            },
            quit_at: None,
            join_at: None,
            classes: CLASSES
                .iter()
                .filter(|c| ir.strings.iter().any(|s| s == *c))
                .map(|c| c.to_string())
                .collect(),
            wants,
        };
        for seat in &scenario.players {
            let weight = 60 + d.below(101);
            d.bots.push(Bot {
                slot: seat.slot,
                weight,
                here: true,
                alive: true,
                respawn_at: None,
                at: None,
                last: None,
                walks: true,
            });
        }
        // One player in four matches leaves, when the script answers it.
        if d.wants.on(Event::Quit) && d.bots.len() >= 4 {
            let leaves = d.below(4) == 0;
            let when = 10 * TPS + d.below(50 * u64::from(TPS)) as u32;
            let who = d.below(d.bots.len() as u64) as usize;
            if leaves {
                d.quit_at = Some((when, who));
            }
        }
        d
    }

    /// A draw in `0..n`; 0 for `n == 0`.
    fn below(&mut self, n: u64) -> u64 {
        ((u128::from(vm::splitmix64(&mut self.rng)) * u128::from(n)) >> 64) as u64
    }

    /// True once in `n` draws.
    fn once_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }

    fn living(&self) -> Vec<usize> {
        (0..self.bots.len())
            .filter(|&i| self.bots[i].here && self.bots[i].alive)
            .collect()
    }

    fn any(&mut self, of: &[usize]) -> Option<usize> {
        if of.is_empty() {
            return None;
        }
        Some(of[self.below(of.len() as u64) as usize])
    }

    /// Where a player walks next. A carrier heads for a team's object:
    /// its own, or in Assault an enemy's. Anyone else mostly visits the
    /// next number of the label last visited, so numbered checkpoints are
    /// passed in order and every hill is stood on.
    fn next_zone(&mut self, bot: usize, carrying: bool, team: Option<u8>) -> usize {
        let jump = self.below(10) >= 7;
        let anywhere = self.below(self.objects.len() as u64) as usize;
        if carrying {
            let own = self.carrier != 9;
            let goals: Vec<usize> = (0..self.objects.len())
                .filter(|&i| {
                    let t = self.objects[i].team;
                    t.is_some() && (t == team) == own
                })
                .collect();
            if let Some(i) = self.any(&goals) {
                return i;
            }
        }
        match self.bots[bot].last {
            Some(last) if !jump => {
                let label = &self.objects[last].label;
                let start = self.objects.iter().position(|o| o.label == *label).unwrap_or(last);
                let len = self.objects.iter().filter(|o| o.label == *label).count().max(1);
                start + (last - start + 1) % len
            }
            _ => anywhere,
        }
    }

    /// Whether a player of `team` picks up an objective of `owner`: in
    /// CTF an enemy's flag, in Assault its own bomb, else anything.
    fn takes(&self, team: Option<u8>, owner: Option<u8>) -> bool {
        match (owner, self.carrier) {
            (None, _) => true,
            (Some(_), 1) => owner != team,
            (Some(_), 9) => owner == team,
            _ => true,
        }
    }

    /// The events of one tick, read off the game as the last tick left it.
    fn tick(&mut self, tick: u32, game: &FakeGame) -> Vec<Input> {
        let mut out = Vec::new();
        if tick == 0 {
            if self.wants.on(Event::Spawn) {
                out.extend(self.bots.iter().map(|b| Input::Spawn(b.slot)));
            }
            return out;
        }
        let team_of = |slot: u8| game.players.get(&slot).map(|p| p.team);

        // Respawns: after the delay, or when the script made one.
        for b in &mut self.bots {
            if !b.here || b.alive {
                continue;
            }
            let script = game.players.get(&b.slot).is_some_and(|p| p.alive);
            if script || b.respawn_at.is_some_and(|t| t <= tick) {
                b.alive = true;
                b.respawn_at = None;
                out.push(Input::Spawn(b.slot));
            }
        }

        // What each player carries, by slot.
        let mut carried: BTreeMap<u8, u32> = BTreeMap::new();
        for (&o, c) in &game.objectives {
            if let Some(p) = c {
                carried.insert(*p, o);
            }
        }

        // A quit, and the same player back ten seconds on.
        if let Some((when, who)) = self.quit_at
            && when <= tick
        {
            self.quit_at = None;
            let b = &mut self.bots[who];
            if let Some(o) = carried.remove(&b.slot) {
                out.push(Input::Drop(b.slot, o));
            }
            out.push(Input::Quit(b.slot));
            b.here = false;
            b.alive = false;
            b.walks = false;
            b.at = None;
            b.respawn_at = None;
            if self.wants.on(Event::Join) {
                self.join_at = Some((tick + 10 * TPS, who));
            }
        }
        if let Some((when, who)) = self.join_at
            && when <= tick
        {
            self.join_at = None;
            let b = &mut self.bots[who];
            b.here = true;
            b.alive = true;
            out.push(Input::Join(b.slot));
        }

        // Walking: ten seconds between objects, five in one; a carrier
        // hurries and then stays.
        if self.wants.zones && !self.objects.is_empty() {
            for i in 0..self.bots.len() {
                if !self.bots[i].here || !self.bots[i].alive || !self.bots[i].walks {
                    continue;
                }
                let slot = self.bots[i].slot;
                let carrying = carried.contains_key(&slot);
                let at = self.bots[i].at;
                match at {
                    Some(z) => {
                        if self.once_in(5 * u64::from(TPS)) {
                            out.push(Input::Leave {
                                zone: self.objects[z].object,
                                object: unit_of(slot),
                            });
                            self.bots[i].at = None;
                        }
                    }
                    None => {
                        let wait = if carrying { 3 * TPS / 2 } else { 10 * TPS };
                        if self.once_in(u64::from(wait)) {
                            let z = self.next_zone(i, carrying, team_of(slot));
                            out.push(Input::Enter {
                                zone: self.objects[z].object,
                                object: unit_of(slot),
                            });
                            self.bots[i].at = Some(z);
                            self.bots[i].last = Some(z);
                        }
                    }
                }
            }
        }

        // Objectives: a free one is picked up within a few seconds, and a
        // carrier now and then lets go.
        if self.wants.carry {
            let held: Vec<(u8, u32)> = carried.iter().map(|(&p, &o)| (p, o)).collect();
            for (p, o) in held {
                if self.once_in(30 * u64::from(TPS)) {
                    out.push(Input::Drop(p, o));
                    carried.remove(&p);
                }
            }
            let free: Vec<u32> = game
                .objectives
                .iter()
                .filter(|(_, c)| c.is_none())
                .map(|(&o, _)| o)
                .collect();
            for o in free {
                if !self.once_in(3 * u64::from(TPS)) {
                    continue;
                }
                let owner = game.object_teams.get(&o).copied();
                let takers: Vec<usize> = self
                    .living()
                    .into_iter()
                    .filter(|&i| {
                        let slot = self.bots[i].slot;
                        !carried.contains_key(&slot) && self.takes(team_of(slot), owner)
                    })
                    .collect();
                if let Some(i) = self.any(&takers) {
                    let slot = self.bots[i].slot;
                    out.push(Input::Pickup(slot, o));
                    carried.insert(slot, o);
                }
            }
        }

        // A kill: the killer by weight, then what kind.
        if self.wants.combat {
            let living = self.living();
            let total: u64 = living.iter().map(|&i| self.bots[i].weight).sum();
            let roll = self.below(KILL_ODDS);
            let kind = self.below(100);
            let pick = self.below(16);
            if roll < total {
                let mut at = roll;
                let mut killer = living[0];
                for &i in &living {
                    if at < self.bots[i].weight {
                        killer = i;
                        break;
                    }
                    at -= self.bots[i].weight;
                }
                let k_slot = self.bots[killer].slot;
                let k_team = team_of(k_slot);
                let others: Vec<usize> = living.iter().copied().filter(|&i| i != killer).collect();
                let mates: Vec<usize> = others
                    .iter()
                    .copied()
                    .filter(|&i| team_of(self.bots[i].slot) == k_team)
                    .collect();
                let foes: Vec<usize> = others
                    .iter()
                    .copied()
                    .filter(|&i| team_of(self.bots[i].slot) != k_team)
                    .collect();
                let of = |list: &[usize]| (!list.is_empty()).then(|| list[pick as usize % list.len()]);
                // 3 in 100 a suicide, 2 a death by the world, and with
                // teams 4 a betrayal.
                let kill: Option<(usize, Option<u8>)> = if kind < 3 {
                    Some((killer, Some(k_slot)))
                } else if kind < 5 {
                    Some((killer, None))
                } else if !self.team_play {
                    of(&others).map(|v| (v, Some(k_slot)))
                } else if kind < 9 && !mates.is_empty() {
                    of(&mates).map(|v| (v, Some(k_slot)))
                } else {
                    of(&foes).map(|v| (v, Some(k_slot)))
                };
                if let Some((v, by)) = kill {
                    let victim = self.bots[v].slot;
                    if let Some(o) = carried.remove(&victim) {
                        out.push(Input::Drop(victim, o));
                    }
                    if let Some(z) = self.bots[v].at
                        && self.wants.on(Event::Leave)
                    {
                        out.push(Input::Leave {
                            zone: self.objects[z].object,
                            object: unit_of(victim),
                        });
                    }
                    out.push(Input::Kill { victim, killer: by });
                    if self.wants.on(Event::Death) {
                        out.push(Input::Death(victim));
                    }
                    let b = &mut self.bots[v];
                    b.alive = false;
                    b.at = None;
                    b.respawn_at = (!self.wants.respawns).then_some(tick + self.respawn_ticks);
                }
            }
        }

        // The rest, each only for a script with its handler.
        let living = self.living();
        let n = living.len() as u64;
        if self.wants.on(Event::Bounce)
            && self.below(8 * u64::from(TPS)) < n
            && let Some(i) = self.any(&living)
        {
            let spot = SPOT_BASE + self.below(6) as u32;
            // Most bounces stand alone; a few chain, and a long chain is
            // now and then a Monster Bounce.
            let chain = 1 + self.below(4) as u32;
            let monster = chain >= 3 && self.below(4) == 0;
            out.push(Input::Bounce { player: self.bots[i].slot, spot, chain, monster });
        }
        if self.wants.on(Event::Bounced)
            && self.below(40 * u64::from(TPS)) < n
            && let Some(i) = self.any(&living)
        {
            out.push(Input::Bounced(self.bots[i].slot));
        }
        if self.wants.on(Event::Edit)
            && self.below(15 * u64::from(TPS)) < n
            && let Some(i) = self.any(&living)
        {
            let object = EDIT_BASE + self.below(8) as u32;
            out.push(Input::Edit(self.bots[i].slot, object));
        }
        if self.wants.on(Event::Press)
            && self.below(10 * u64::from(TPS)) < n
            && let Some(i) = self.any(&living)
        {
            let pick = self.below(self.objects.len() as u64) as usize;
            let object = self.objects.get(pick).map_or(0, |o| o.object);
            out.push(Input::Press(self.bots[i].slot, object));
        }
        if self.wants.on(Event::ActorKill) && self.once_in(3 * u64::from(TPS)) {
            let killer = self.any(&living);
            let class = if self.classes.is_empty() {
                CLASSES[0].to_string()
            } else {
                let i = self.below(self.classes.len() as u64) as usize;
                self.classes[i].clone()
            };
            let by = killer.map(|i| self.bots[i].slot);
            out.push(Input::ActorKill {
                killer: by,
                class: class.clone(),
            });
            let helpers: Vec<usize> = living.iter().copied().filter(|&i| Some(i) != killer).collect();
            if self.wants.on(Event::ActorAssist)
                && self.once_in(3)
                && let Some(i) = self.any(&helpers)
            {
                out.push(Input::ActorAssist {
                    killer: by,
                    assist: Some(self.bots[i].slot),
                    class,
                });
            }
        }
        if self.wants.on(Event::SquadDead) && self.once_in(20 * u64::from(TPS)) {
            let squad = self.below(4) as u8;
            out.push(Input::SquadDead(squad));
        }
        out
    }
}

// ---------- one match ----------

/// The scenario a match of `ir` starts from, before any event.
fn opening(ir: &Ir, setup: &Setup) -> (Scenario, u8) {
    let teams = teams_for(ir, setup);
    let variant = variant(ir, setup, teams);
    let seconds = variant.get("time_limit").copied().unwrap_or(0);
    let scenario = Scenario {
        players: (0..setup.players)
            .map(|slot| Seat {
                slot,
                team: if teams >= 2 { slot % teams } else { 0 },
                lives: 0,
                unit: Some(unit_of(slot)),
            })
            .collect(),
        map: Some(objects(ir, teams)),
        teams: if teams >= 2 { (0..teams).collect() } else { Vec::new() },
        variant,
        options: None,
        seed: setup.seed,
        round_time: seconds.saturating_mul(TPS as i32),
        ticks: 0,
        start: true,
        steps: Vec::new(),
    };
    (scenario, teams)
}

/// The leader of a table of `(team, score)` rows: a player's slot or a
/// team, the top score, and whether it is shared.
fn leader(rows: &[(u8, i32)]) -> Option<(u8, i32, bool)> {
    let top = rows.iter().map(|r| r.1).max()?;
    let mut at = rows.iter().filter(|r| r.1 == top);
    let first = at.next()?;
    Some((first.0, top, at.next().is_some()))
}

fn play(ir: &Ir, setup: &Setup, keep: Keep, max_ops: u64) -> Result<Match, RunError> {
    let (mut scenario, teams) = opening(ir, setup);
    let mut runner = Runner::new(ir.clone(), &scenario, keep)?;
    runner.max_ops = max_ops;
    let mut director = Director::new(ir, &scenario);
    let mut why = None;
    let mut over = false;
    for tick in 0..=MAX_TICKS {
        let inputs = director.tick(tick, &runner.game);
        if scenario.steps.len() + inputs.len() > MAX_STEPS {
            why = Some(format!("the match made over {MAX_STEPS} events"));
            break;
        }
        scenario
            .steps
            .extend(inputs.iter().map(|input| Step { tick, input: input.clone() }));
        scenario.ticks = tick;
        if !runner.tick(tick, tick == 0, &inputs) {
            over = true;
            break;
        }
    }

    let team_play = teams >= 2;
    let by_player: Vec<(u8, i32)> = runner.game.players.iter().map(|(&s, p)| (s, p.score)).collect();
    let mut sums: BTreeMap<u8, i32> = BTreeMap::new();
    for p in runner.game.players.values() {
        let s = sums.entry(p.team).or_default();
        *s = s.saturating_add(p.score);
    }
    let by_team: Vec<(u8, i32)> = sums.into_iter().collect();
    let trace = runner.finish();

    let end = if trace.error.is_some() {
        End::Error
    } else if trace.round_ended.is_some() {
        End::Script
    } else {
        match trace.limit {
            Some(Limit::Score) => End::ScoreLimit,
            Some(Limit::Time) => End::TimeLimit,
            None => End::Cap,
        }
    };
    let why = match end {
        End::Error => trace.error.clone(),
        End::Cap if over => Some(format!("the match spent over {max_ops} ops")),
        End::Cap => why.or_else(|| Some(format!("{} minutes passed", MAX_LENGTH / 60))),
        _ => None,
    };
    let lead = leader(if team_play { &by_team } else { &by_player });
    let named = trace
        .round_winner
        .as_deref()
        .and_then(|w| w.strip_prefix("team "))
        .and_then(|n| n.parse::<u8>().ok());
    let winner = match (end, named, lead) {
        (End::Error | End::Cap, _, _) => Winner::Nobody,
        // A team named by `end_round` wins; with no teams the scores say.
        (End::Script, Some(t), _) if team_play => Winner::Team(t),
        (_, _, Some((_, _, true))) => Winner::Draw,
        (_, _, Some((who, _, false))) if team_play => Winner::Team(who),
        (_, _, Some((who, _, false))) => Winner::Player(who),
        (_, _, None) => Winner::Nobody,
    };
    Ok(Match {
        seed: setup.seed,
        teams,
        scenario,
        end,
        why,
        winner,
        top_score: lead.map_or(0, |l| l.1),
        low_score: by_player.iter().map(|r| r.1).min().unwrap_or(0),
        trace,
    })
}

/// Compiles `src` and plays one match of it, traced.
pub fn simulate(src: &str, setup: &Setup) -> Result<Match, RunError> {
    check(setup)?;
    let ir = workbench::build(src)?;
    play(&ir, setup, Keep::Events, workbench::MAX_RUN_OPS)
}

// ---------- a batch ----------

/// One match of a batch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Row {
    pub seed: u64,
    pub end: End,
    pub winner: Winner,
    /// The last tick run.
    pub ticks: u32,
    pub top_score: i32,
    pub ops: u64,
    pub worst_tick_ops: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Win {
    pub winner: Winner,
    pub matches: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Ending {
    pub end: End,
    pub matches: u32,
}

/// A match that stopped on a script error.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Fault {
    pub seed: u64,
    pub message: String,
}

/// Something about the batch worth a look, with the first seed that
/// shows it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Flag {
    /// `never_ends`, `nobody_scores`, `negative_score`, `never_ran` or
    /// `clock` (every match ran to the time limit).
    pub kind: &'static str,
    pub text: String,
    pub seed: u64,
    /// Matches that show it.
    pub matches: u32,
}

/// What a batch of matches came to.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Summary {
    /// Matches asked for, and played before the batch's budget ran out.
    pub asked: u32,
    pub ran: u32,
    /// Who won, most wins first.
    pub wins: Vec<Win>,
    /// How matches ended, most first.
    pub ends: Vec<Ending>,
    /// The winning score: its mean and its highest.
    pub avg_score: f64,
    pub max_score: i32,
    /// Match length in ticks: the mean and the longest.
    pub avg_ticks: u32,
    pub max_ticks: u32,
    /// The costliest tick of any match, the seed it was in, and the
    /// budget a tick has.
    pub worst_tick_ops: u32,
    pub worst_tick_seed: u64,
    pub budget: u32,
    pub errors: Vec<Fault>,
    pub flags: Vec<Flag>,
    /// Why fewer matches ran than were asked for.
    pub stopped: Option<String>,
}

/// A batch: every match's row and the summary of them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Batch {
    /// The teams played with ([`Match::teams`]).
    pub teams: u8,
    pub summary: Summary,
    pub rows: Vec<Row>,
}

fn tally<K: Ord + Copy>(keys: impl Iterator<Item = K>) -> Vec<(K, u32)> {
    let mut counts: BTreeMap<K, u32> = BTreeMap::new();
    for k in keys {
        *counts.entry(k).or_default() += 1;
    }
    let mut out: Vec<(K, u32)> = counts.into_iter().collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    out
}

/// Compiles `src` and plays `matches` matches of it, seeds `setup.seed`
/// upwards, untraced. The batch starts no match past [`BATCH_OPS`] ops or
/// [`BATCH_TICKS`] ticks.
pub fn batch(src: &str, setup: &Setup, matches: u32) -> Result<Batch, RunError> {
    check(setup)?;
    if matches == 0 || matches > MAX_MATCHES {
        return Err(RunError::Scenario(format!("a batch plays 1 to {MAX_MATCHES} matches")));
    }
    let ir = workbench::build(src)?;
    let teams = teams_for(&ir, setup);
    let mut rows: Vec<Row> = Vec::new();
    let mut ran: BTreeSet<String> = BTreeSet::new();
    let mut errors = Vec::new();
    // (matches, first seed) for each per-match flag.
    let mut never_ends: (u32, u64, Option<String>) = (0, 0, None);
    let mut nobody: (u32, u64) = (0, 0);
    let mut negative: (u32, u64) = (0, 0);
    let (mut ops, mut ticks) = (0u64, 0u64);
    let mut stopped = None;
    for i in 0..matches {
        if ops >= BATCH_OPS || ticks >= BATCH_TICKS {
            stopped = Some(format!(
                "{i} of {matches} matches ran: a batch starts no match past {BATCH_OPS} ops or {BATCH_TICKS} ticks"
            ));
            break;
        }
        let seed = setup.seed.wrapping_add(u64::from(i));
        let one = Setup { seed, ..setup.clone() };
        let m = play(&ir, &one, Keep::Totals, workbench::MAX_RUN_OPS)?;
        ops += m.trace.ops;
        ticks += u64::from(m.trace.last_tick) + 1;
        ran.extend(m.trace.handlers_ran.iter().cloned());
        let flag = |slot: &mut (u32, u64)| {
            if slot.0 == 0 {
                slot.1 = seed;
            }
            slot.0 += 1;
        };
        match m.end {
            End::Error => errors.push(Fault {
                seed,
                message: m.why.clone().unwrap_or_default(),
            }),
            End::Cap => {
                if never_ends.0 == 0 {
                    never_ends.1 = seed;
                    never_ends.2 = m.why.clone();
                }
                never_ends.0 += 1;
            }
            _ => {}
        }
        if m.end != End::Error && !m.trace.scores.is_empty() && m.trace.scores.iter().all(|s| s.1 == 0) {
            flag(&mut nobody);
        }
        if m.low_score < 0 {
            flag(&mut negative);
        }
        rows.push(Row {
            seed,
            end: m.end,
            winner: m.winner,
            ticks: m.trace.last_tick,
            top_score: m.top_score,
            ops: m.trace.ops,
            worst_tick_ops: m.trace.worst_tick_ops,
        });
    }

    let n = rows.len() as u32;
    let of = |count: u32| {
        if count == n {
            "every match".to_string()
        } else {
            format!("{count} of {n} matches")
        }
    };
    let mut flags = Vec::new();
    if never_ends.0 > 0 {
        flags.push(Flag {
            kind: "never_ends",
            text: format!(
                "the game never ended in {} ({})",
                of(never_ends.0),
                never_ends.2.as_deref().unwrap_or("a cap stopped it")
            ),
            seed: never_ends.1,
            matches: never_ends.0,
        });
    }
    if nobody.0 > 0 {
        flags.push(Flag {
            kind: "nobody_scores",
            text: format!("nobody scored in {}", of(nobody.0)),
            seed: nobody.1,
            matches: nobody.0,
        });
    }
    if negative.0 > 0 {
        flags.push(Flag {
            kind: "negative_score",
            text: format!("a player ended below zero in {}", of(negative.0)),
            seed: negative.1,
            matches: negative.0,
        });
    }
    if n > 0 {
        for h in &ir.handlers {
            let name = handler_name(&ir, h);
            // The generator never reports the round's end.
            if h.event == Event::RoundEnd || ran.contains(&name) {
                continue;
            }
            flags.push(Flag {
                kind: "never_ran",
                text: format!("on {name} never ran"),
                seed: setup.seed,
                matches: n,
            });
        }
    }
    if n >= 2 && rows.iter().all(|r| r.end == End::TimeLimit) {
        flags.push(Flag {
            kind: "clock",
            text: "every match ran to the time limit".into(),
            seed: setup.seed,
            matches: n,
        });
    }
    let worst = rows.iter().max_by_key(|r| (r.worst_tick_ops, std::cmp::Reverse(r.seed)));
    let total: i64 = rows.iter().map(|r| i64::from(r.top_score)).sum();
    let length: u64 = rows.iter().map(|r| u64::from(r.ticks)).sum();
    let summary = Summary {
        asked: matches,
        ran: n,
        wins: tally(rows.iter().map(|r| r.winner))
            .into_iter()
            .map(|(winner, matches)| Win { winner, matches })
            .collect(),
        ends: tally(rows.iter().map(|r| r.end))
            .into_iter()
            .map(|(end, matches)| Ending { end, matches })
            .collect(),
        avg_score: if n == 0 { 0.0 } else { total as f64 / f64::from(n) },
        max_score: rows.iter().map(|r| r.top_score).max().unwrap_or(0),
        avg_ticks: if n == 0 { 0 } else { (length / u64::from(n)) as u32 },
        max_ticks: rows.iter().map(|r| r.ticks).max().unwrap_or(0),
        worst_tick_ops: worst.map_or(0, |r| r.worst_tick_ops),
        worst_tick_seed: worst.map_or(setup.seed, |r| r.seed),
        budget: vm::TICK_BUDGET,
        errors,
        flags,
        stopped,
    };
    Ok(Batch { teams, summary, rows })
}

// ---------- the scenario file ----------

/// A match's scenario in the file format `h2 rules run` reads
/// ([`crate::scenario`]), and what the format has no place for.
pub fn to_file(s: &Scenario) -> (file::Scenario, Vec<String>) {
    let mut left: BTreeSet<String> = BTreeSet::new();
    if !s.start {
        left.insert("a start without init and round_start".into());
    }
    let mut ticks = vec![file::ScenarioTick::default(); s.ticks as usize + 1];
    for step in &s.steps {
        let event = match step.input.clone() {
            Input::Kill { victim, killer } => file::ScenarioEvent::Kill { victim, killer },
            Input::Spawn(p) => file::ScenarioEvent::Spawn(p),
            Input::Death(p) => file::ScenarioEvent::Death(p),
            Input::Join(p) => file::ScenarioEvent::Join(p),
            Input::Quit(p) => file::ScenarioEvent::Quit(p),
            Input::Enter { zone, object } => file::ScenarioEvent::Enter { zone, object },
            Input::Leave { zone, object } => file::ScenarioEvent::Leave { zone, object },
            Input::Press(player, object) => file::ScenarioEvent::Press { player, object },
            Input::Pickup(player, object) => file::ScenarioEvent::Pickup { player, object },
            Input::Drop(player, object) => file::ScenarioEvent::Drop { player, object },
            Input::SquadDead(squad) => file::ScenarioEvent::SquadDead(squad),
            Input::RoundEnd => file::ScenarioEvent::RoundEnd,
            // Tick 0 of a file reports these by itself.
            Input::Init | Input::RoundStart => continue,
            Input::Bounce { player, spot, chain, monster } => {
                file::ScenarioEvent::Bounce { player, object: spot, chain, monster }
            }
            Input::Bounced(player) => file::ScenarioEvent::Bounced(player),
            Input::Edit(player, object) => file::ScenarioEvent::Edit { player, object },
            Input::ActorKill { killer, class } => file::ScenarioEvent::ActorKill { killer, class },
            Input::ActorAssist {
                killer,
                assist,
                class,
            } => file::ScenarioEvent::ActorAssist {
                killer,
                assist,
                class,
            },
            Input::Damage { object, by, amount } => file::ScenarioEvent::Damage { object, by, amount },
        };
        if let Some(t) = ticks.get_mut(step.tick as usize) {
            t.events.push(event);
        }
    }
    let scenario = file::Scenario {
        seed: s.seed,
        options: s.options,
        players: s
            .players
            .iter()
            .map(|p| file::ScenarioPlayer {
                slot: p.slot,
                team: p.team,
                alive: true,
                score: 0,
                lives: p.lives,
                unit: p.unit,
            })
            .collect(),
        teams: (!s.teams.is_empty()).then(|| s.teams.clone()),
        variant: s.variant.clone(),
        map: s.map.clone(),
        round_time_left: s.round_time,
        ticks,
    };
    (scenario, left.into_iter().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A script with a label table of each kind, built from `LabelDef`s so
    /// the test reads no built-in's source.
    fn labelled_ir() -> Ir {
        use crate::ir::label_flags::*;
        use crate::ir::{LabelDef, LabelKind};
        let mut ir = crate::compile(crate::builtin::SLAYER).unwrap();
        ir.labels = vec![
            LabelDef { text: "goal".into(), kind: LabelKind::Zone, min: 1, max: 1, flags: SINGLE, lo: 0, hi: 0 },
            LabelDef { text: "checkpoint".into(), kind: LabelKind::Zone, min: 0, max: 256, flags: NUMBERED | RANGE, lo: 0, hi: 2 },
            LabelDef { text: "home".into(), kind: LabelKind::Zone, min: 0, max: 16, flags: PER_TEAM, lo: 0, hi: 0 },
            LabelDef { text: "start".into(), kind: LabelKind::Spawn, min: 2, max: 256, flags: 0, lo: 0, hi: 0 },
            LabelDef::objects("barrier"),
        ];
        crate::verify(&ir).unwrap();
        ir
    }

    #[test]
    fn a_match_is_played_on_the_minimal_map() {
        let ir = labelled_ir();
        let o = objects(&ir, 2);
        let of = |label: &str| o.iter().filter(|t| t.label == label).collect::<Vec<_>>();
        assert_eq!(of("goal").len(), 1);
        assert_eq!(of("checkpoint").iter().map(|t| t.number).collect::<Vec<_>>(), [Some(0), Some(1), Some(2)]);
        assert_eq!(of("home").iter().map(|t| t.team).collect::<Vec<_>>(), [Some(0), Some(1)]);
        assert_eq!(of("start").len(), 2);
        assert!(of("start").iter().all(|t| t.class == Some(Class::Spawn)));
        // A label the minimal map leaves empty gets one for each team.
        assert_eq!(of("barrier").iter().map(|t| t.team).collect::<Vec<_>>(), [Some(0), Some(1)]);
        assert!(o.iter().all(|x| x.object >= OBJECT_BASE));
        // Four teams: a home for each.
        assert_eq!(objects(&ir, 4).iter().filter(|t| t.label == "home").count(), 4);
        // Free for all reads teams 0 and 1.
        assert_eq!(objects(&ir, 0).iter().filter(|t| t.label == "home").count(), 2);
        // And it passes the check it is played under.
        for teams in [0u8, 2, 4] {
            let map = objects(&ir, teams);
            let ids: Vec<u8> = if teams >= 2 { (0..teams).collect() } else { vec![0, 1] };
            assert_eq!(file::resolve_map(&ir, Some(&map), &ids), Ok(map));
        }
    }

    #[test]
    fn a_setup_out_of_range_is_refused() {
        for bad in [
            Setup { players: 0, ..Setup::default() },
            Setup { players: 17, ..Setup::default() },
            Setup { teams: 9, ..Setup::default() },
            Setup { length: Some(0), ..Setup::default() },
            Setup { length: Some(MAX_LENGTH + 1), ..Setup::default() },
            Setup { score_limit: Some(0), ..Setup::default() },
        ] {
            assert!(matches!(
                simulate(crate::builtin::SLAYER, &bad),
                Err(RunError::Scenario(_))
            ));
        }
        assert!(matches!(
            batch(crate::builtin::SLAYER, &Setup::default(), 0),
            Err(RunError::Scenario(_))
        ));
        assert!(matches!(
            batch(crate::builtin::SLAYER, &Setup::default(), MAX_MATCHES + 1),
            Err(RunError::Scenario(_))
        ));
        assert!(matches!(simulate("nope", &Setup::default()), Err(RunError::Script(_))));
    }

    #[test]
    fn winners_tally_most_first() {
        let t = tally([Winner::Player(2), Winner::Draw, Winner::Player(2), Winner::Player(0)].into_iter());
        assert_eq!(t[0], (Winner::Player(2), 2));
        assert_eq!(t[1], (Winner::Player(0), 1));
        assert_eq!(leader(&[(0, 3), (1, 5), (2, 5)]), Some((1, 5, true)));
        assert_eq!(leader(&[(4, 7)]), Some((4, 7, false)));
        assert_eq!(leader(&[]), None);
    }
}
