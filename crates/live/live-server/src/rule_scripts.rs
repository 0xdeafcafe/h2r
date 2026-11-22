//! The rules workbench and saved game-type scripts (docs/live/live-api.md
//! "Rules scripts", docs/game/rules-engine.md "Workbench").
//!
//! `check` and `run` compile and run a script with the `rules` crate on
//! the server, so the web app needs no wasm toolchain: `check` measures a
//! script (`rules::workbench::check`) and `run` steps a scenario over
//! `rules::fake::FakeGame` (`rules::workbench::run`). `simulate` plays
//! generated matches of a script, one or a batch with a summary
//! (`rules::sim`). All three are public and rate-limited per client
//! address. `ide` answers the web editor's
//! completion, hover, signature, definition, references, tokens, outline,
//! format and diagnostics requests from `rules::ide`, in LSP's shapes, at
//! the same rate in a bucket of its own.
//!
//! A signed-in account saves a script that verifies: a server-signed
//! `RulesSaved` event on `rules-<hash>` holds its source, and the first
//! save of a hash is the only one (the stream must be empty). The
//! projection below is rebuilt from the log on both homes; `GET
//! /v1/rules/{hash}/ir` compiles the source again, which gives the same
//! bytes the hash names.

use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, RwLock};

use axum::Extension;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::BytesRejection;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use live_proto::rules::{
    self as wire, KINDS, MAX_PER_AUTHOR, MAX_SOURCE, RULES_SAVED, Saved, Script, ScriptList,
    ScriptSummary,
};
use live_proto::{Author, ErrorCode, Gamertag, Xuid, lxm};
use rules::vm::Input;
use rules::sim;
use rules::workbench::{self, Limit, Problem, RunError, Scenario, Seat, Step};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::app::{ApiError, Live, conflict, invalid, not_found};
use crate::dispatch::Subscriber;
use crate::http::{ClientIp, bearer, body_bytes};
use crate::store::{RecordedEvent, StoreError};

type ApiResult<T> = Result<T, ApiError>;

/// A request body: the source (at most [`MAX_SOURCE`] bytes) as JSON text,
/// whose escapes may double it, plus a scenario.
pub const MAX_BODY: usize = 2 * MAX_SOURCE + 256 * 1024;

/// Check and run requests per client address: a burst of 20, then two a
/// second (typing with a debounce stays under it).
pub const RATE: (f64, u32) = (2.0, 20);

// ---------- the projection ----------

#[derive(Debug, Clone)]
struct Kept {
    saved: Saved,
    saved_at: String,
    position: u64,
}

/// Every saved script, by hash.
#[derive(Debug, Default)]
pub struct RulesState {
    by_hash: BTreeMap<String, Kept>,
    /// Each author's hashes, oldest first.
    by_author: HashMap<Xuid, Vec<String>>,
}

impl RulesState {
    /// The saved hashes that start with `prefix` (lowercase hex), in order.
    pub fn hashes_with_prefix(&self, prefix: &str) -> Vec<String> {
        self.by_hash
            .range(prefix.to_string()..)
            .take_while(|(h, _)| h.starts_with(prefix))
            .map(|(h, _)| h.clone())
            .collect()
    }

    pub fn has(&self, hash: &str) -> bool {
        self.by_hash.contains_key(hash)
    }

    pub fn count_by(&self, author: Xuid) -> usize {
        self.by_author.get(&author).map_or(0, Vec::len)
    }

    fn apply(&mut self, e: &RecordedEvent) {
        if e.body.kind != RULES_SAVED {
            return;
        }
        let Ok(saved) = serde_json::from_value::<Saved>(e.body.data.clone()) else {
            return;
        };
        if self.by_hash.contains_key(&saved.hash) {
            return;
        }
        self.by_author
            .entry(saved.author)
            .or_default()
            .push(saved.hash.clone());
        self.by_hash.insert(
            saved.hash.clone(),
            Kept {
                saved,
                saved_at: e.body.at.clone(),
                position: e.position,
            },
        );
    }
}

/// The saved scripts' projection.
#[derive(Default)]
pub struct Rules {
    state: RwLock<RulesState>,
}

impl Rules {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, RulesState> {
        self.state.read().unwrap()
    }
}

impl Subscriber for Rules {
    fn name(&self) -> &str {
        "rules"
    }

    fn apply(&self, event: &RecordedEvent) {
        if KINDS.contains(&event.body.kind.as_str()) {
            self.state.write().unwrap().apply(event);
        }
    }
}

// ---------- wire shapes ----------

#[derive(Debug, Serialize)]
pub struct WireProblem {
    pub line: u32,
    pub col: u32,
    pub message: String,
}

impl From<Problem> for WireProblem {
    fn from(p: Problem) -> Self {
        WireProblem {
            line: p.line,
            col: p.col,
            message: p.message,
        }
    }
}

#[derive(Debug, Serialize)]
struct WireHandler {
    event: String,
    worst_ops: u64,
}

#[derive(Debug, Serialize)]
struct WireSlots {
    scope: &'static str,
    used: usize,
    limit: usize,
}

#[derive(Debug, Serialize)]
struct WireCap {
    name: String,
    words: &'static str,
    grantable: bool,
    known: bool,
}

#[derive(Debug, Serialize)]
struct WireReport {
    worst_tick_ops: u64,
    budget: u64,
    fits: bool,
    costliest: Option<String>,
    handlers: Vec<WireHandler>,
    slots: Vec<WireSlots>,
    ir_bytes: usize,
    ir_limit: usize,
    capabilities: Vec<WireCap>,
    unhostable: Vec<&'static str>,
    /// What a map needs: the labels the script declares.
    labels: Vec<workbench::LabelLine>,
    /// The smallest map that passes, as a scenario's `map` lists things.
    minimal: Vec<rules::scenario::ScenarioLabel>,
}

/// `POST /v1/rules/check`'s answer.
#[derive(Debug, Serialize)]
pub struct CheckView {
    ok: bool,
    hash: Option<String>,
    name: Option<String>,
    version: Option<u16>,
    errors: Vec<WireProblem>,
    report: Option<WireReport>,
    /// The options the script declares, when it checks clean
    /// ([`crate::game_variants::option_views`]).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    options: Vec<crate::game_variants::OptionView>,
}

fn check_view(c: workbench::Check) -> CheckView {
    CheckView {
        ok: c.ok,
        hash: c.hash,
        name: c.name,
        version: c.version,
        errors: c.errors.into_iter().map(Into::into).collect(),
        options: Vec::new(),
        report: c.report.map(|r| WireReport {
            worst_tick_ops: r.worst_tick_ops,
            budget: r.budget,
            fits: r.fits,
            costliest: r.costliest,
            handlers: r
                .handlers
                .into_iter()
                .map(|h| WireHandler {
                    event: h.event,
                    worst_ops: h.worst_ops,
                })
                .collect(),
            slots: r
                .slots
                .into_iter()
                .map(|s| WireSlots {
                    scope: s.scope,
                    used: s.used,
                    limit: s.limit,
                })
                .collect(),
            ir_bytes: r.ir_bytes,
            ir_limit: r.ir_limit,
            capabilities: r
                .capabilities
                .into_iter()
                .map(|c| WireCap {
                    name: c.name,
                    words: c.words,
                    grantable: c.grantable,
                    known: c.known,
                })
                .collect(),
            unhostable: r.unhostable,
            labels: r.labels,
            minimal: r.minimal,
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SourceBody {
    source: String,
}

/// A seat of `POST /v1/rules/run`'s scenario.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WireSeat {
    slot: u8,
    #[serde(default)]
    team: u8,
    #[serde(default)]
    lives: i32,
    /// The unit the player drives, for `enter`/`leave` events that name it.
    #[serde(default)]
    unit: Option<u32>,
}

/// One event of a scenario. `type` is the handler's event name.
#[derive(Debug, Deserialize, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum WireEvent {
    Init,
    RoundStart,
    RoundEnd,
    Kill {
        victim: u8,
        #[serde(default)]
        killer: Option<u8>,
    },
    Spawn {
        player: u8,
    },
    Death {
        player: u8,
    },
    Join {
        player: u8,
    },
    Quit {
        player: u8,
    },
    Pickup {
        player: u8,
        object: u32,
    },
    Drop {
        player: u8,
        object: u32,
    },
    Press {
        player: u8,
        object: u32,
    },
    /// `chain` is the bounce's place in its chain, 1 when absent.
    Bounce {
        player: u8,
        object: u32,
        #[serde(default = "one")]
        chain: u32,
        #[serde(default)]
        monster: bool,
    },
    Bounced {
        player: u8,
    },
    Edit {
        player: u8,
        object: u32,
    },
    Enter {
        zone: u32,
        object: u32,
    },
    Leave {
        zone: u32,
        object: u32,
    },
    SquadDead {
        squad: u8,
    },
    ActorKill {
        #[serde(default)]
        killer: Option<u8>,
        class: String,
    },
    ActorAssist {
        #[serde(default)]
        killer: Option<u8>,
        #[serde(default)]
        assist: Option<u8>,
        class: String,
    },
}

impl From<WireEvent> for Input {
    fn from(e: WireEvent) -> Input {
        match e {
            WireEvent::Init => Input::Init,
            WireEvent::RoundStart => Input::RoundStart,
            WireEvent::RoundEnd => Input::RoundEnd,
            WireEvent::Kill { victim, killer } => Input::Kill { victim, killer },
            WireEvent::Spawn { player } => Input::Spawn(player),
            WireEvent::Death { player } => Input::Death(player),
            WireEvent::Join { player } => Input::Join(player),
            WireEvent::Quit { player } => Input::Quit(player),
            WireEvent::Pickup { player, object } => Input::Pickup(player, object),
            WireEvent::Drop { player, object } => Input::Drop(player, object),
            WireEvent::Press { player, object } => Input::Press(player, object),
            WireEvent::Bounce {
                player,
                object,
                chain,
                monster,
            } => Input::Bounce {
                player,
                spot: object,
                chain,
                monster,
            },
            WireEvent::Bounced { player } => Input::Bounced(player),
            WireEvent::Edit { player, object } => Input::Edit(player, object),
            WireEvent::Enter { zone, object } => Input::Enter { zone, object },
            WireEvent::Leave { zone, object } => Input::Leave { zone, object },
            WireEvent::SquadDead { squad } => Input::SquadDead(squad),
            WireEvent::ActorKill { killer, class } => Input::ActorKill { killer, class },
            WireEvent::ActorAssist { killer, assist, class } => {
                Input::ActorAssist { killer, assist, class }
            }
        }
    }
}

impl From<&Input> for WireEvent {
    fn from(i: &Input) -> WireEvent {
        match i.clone() {
            Input::Init => WireEvent::Init,
            Input::RoundStart => WireEvent::RoundStart,
            Input::RoundEnd => WireEvent::RoundEnd,
            Input::Kill { victim, killer } => WireEvent::Kill { victim, killer },
            Input::Spawn(player) => WireEvent::Spawn { player },
            Input::Death(player) => WireEvent::Death { player },
            Input::Join(player) => WireEvent::Join { player },
            Input::Quit(player) => WireEvent::Quit { player },
            Input::Pickup(player, object) => WireEvent::Pickup { player, object },
            Input::Drop(player, object) => WireEvent::Drop { player, object },
            Input::Press(player, object) => WireEvent::Press { player, object },
            Input::Bounce { player, spot, chain, monster } => {
                WireEvent::Bounce { player, object: spot, chain, monster }
            }
            Input::Bounced(player) => WireEvent::Bounced { player },
            Input::Edit(player, object) => WireEvent::Edit { player, object },
            Input::Enter { zone, object } => WireEvent::Enter { zone, object },
            Input::Leave { zone, object } => WireEvent::Leave { zone, object },
            Input::SquadDead(squad) => WireEvent::SquadDead { squad },
            Input::ActorKill { killer, class } => WireEvent::ActorKill { killer, class },
            Input::ActorAssist {
                killer,
                assist,
                class,
            } => WireEvent::ActorAssist {
                killer,
                assist,
                class,
            },
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WireTick {
    tick: u32,
    #[serde(default)]
    events: Vec<WireEvent>,
}

fn yes() -> bool {
    true
}

fn one() -> u32 {
    1
}

/// `POST /v1/rules/run`'s scenario, and a generated match's.
#[derive(Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct WireScenario {
    #[serde(default)]
    players: Vec<WireSeat>,
    #[serde(default)]
    teams: Vec<u8>,
    #[serde(default)]
    variant: BTreeMap<String, i32>,
    #[serde(default)]
    options: Option<u32>,
    #[serde(default)]
    seed: u64,
    #[serde(default)]
    round_time: i32,
    /// The last tick run; the last listed tick when absent.
    #[serde(default)]
    ticks: Option<u32>,
    #[serde(default = "yes")]
    start: bool,
    #[serde(default)]
    timeline: Vec<WireTick>,
    /// The map: what `objects labelled` visits, `o.number` reads and, with
    /// a `team`, `o.team` reads. Absent, the script's minimal map; present,
    /// it must pass the script's labels. `labelled` is the older name.
    #[serde(default, alias = "labelled", skip_serializing_if = "Option::is_none")]
    map: Option<Vec<rules::scenario::ScenarioLabel>>,
}

impl WireScenario {
    fn into_scenario(self) -> Scenario {
        let last = self.timeline.iter().map(|t| t.tick).max().unwrap_or(0);
        let steps = self
            .timeline
            .into_iter()
            .flat_map(|t| {
                let tick = t.tick;
                t.events.into_iter().map(move |e| Step {
                    tick,
                    input: e.into(),
                })
            })
            .collect();
        Scenario {
            players: self
                .players
                .into_iter()
                .map(|s| Seat {
                    slot: s.slot,
                    team: s.team,
                    lives: s.lives,
                    unit: s.unit,
                })
                .collect(),
            map: self.map,
            teams: self.teams,
            variant: self.variant,
            options: self.options,
            seed: self.seed,
            round_time: self.round_time,
            ticks: self.ticks.unwrap_or(last),
            start: self.start,
            steps,
        }
    }

    /// A scenario as the route reads one: its events grouped by tick.
    fn of(s: &Scenario) -> WireScenario {
        let mut timeline: Vec<WireTick> = Vec::new();
        let mut steps: Vec<&Step> = s.steps.iter().collect();
        steps.sort_by_key(|step| step.tick);
        for step in steps {
            match timeline.last_mut() {
                Some(t) if t.tick == step.tick => t.events.push((&step.input).into()),
                _ => timeline.push(WireTick {
                    tick: step.tick,
                    events: vec![(&step.input).into()],
                }),
            }
        }
        WireScenario {
            players: s
                .players
                .iter()
                .map(|p| WireSeat {
                    slot: p.slot,
                    team: p.team,
                    lives: p.lives,
                    unit: p.unit,
                })
                .collect(),
            map: s.map.clone(),
            teams: s.teams.clone(),
            variant: s.variant.clone(),
            options: s.options,
            seed: s.seed,
            round_time: s.round_time,
            ticks: Some(s.ticks),
            start: s.start,
            timeline,
        }
    }
}

#[derive(Debug, Deserialize)]
struct RunBody {
    source: String,
    scenario: WireScenario,
}

#[derive(Debug, Serialize)]
struct WireCall {
    entry: &'static str,
    args: Vec<String>,
}

#[derive(Debug, Serialize)]
struct WireVar {
    name: String,
    value: String,
}

fn vars(v: Vec<(String, String)>) -> Vec<WireVar> {
    v.into_iter()
        .map(|(name, value)| WireVar { name, value })
        .collect()
}

#[derive(Debug, Serialize)]
struct WirePlayer {
    slot: u8,
    team: u8,
    alive: bool,
    score: i32,
    lives: i32,
    vars: Vec<WireVar>,
}

#[derive(Debug, Serialize)]
struct WireTickTrace {
    tick: u32,
    ran: Vec<String>,
    calls: Vec<WireCall>,
    ops: u32,
    warnings: Vec<String>,
    globals: Vec<WireVar>,
    players: Vec<WirePlayer>,
    error: Option<String>,
}

#[derive(Debug, Serialize)]
struct WireScore {
    slot: u8,
    score: i32,
}

/// `POST /v1/rules/run`'s answer.
#[derive(Debug, Serialize)]
pub struct TraceView {
    ok: bool,
    hash: String,
    options: u32,
    ticks: Vec<WireTickTrace>,
    last_tick: u32,
    error: Option<String>,
    round_ended: Option<u32>,
    /// Who `end_round` named.
    round_winner: Option<String>,
    /// `score` or `time`: the record's limit the run stopped on.
    limit: Option<&'static str>,
    cut: Option<String>,
    ops: u64,
    worst_tick_ops: u32,
    handlers_ran: Vec<String>,
    scores: Vec<WireScore>,
}

fn trace_view(t: workbench::Trace) -> TraceView {
    TraceView {
        ok: t.error.is_none(),
        hash: t.hash,
        options: t.options,
        ticks: t
            .ticks
            .into_iter()
            .map(|k| WireTickTrace {
                tick: k.tick,
                ran: k.ran,
                calls: k
                    .calls
                    .into_iter()
                    .map(|c| WireCall {
                        entry: c.entry,
                        args: c.args,
                    })
                    .collect(),
                ops: k.ops,
                warnings: k.warnings,
                globals: vars(k.globals),
                players: k
                    .players
                    .into_iter()
                    .map(|p| WirePlayer {
                        slot: p.slot,
                        team: p.team,
                        alive: p.alive,
                        score: p.score,
                        lives: p.lives,
                        vars: vars(p.vars),
                    })
                    .collect(),
                error: k.error,
            })
            .collect(),
        last_tick: t.last_tick,
        error: t.error,
        round_ended: t.round_ended,
        round_winner: t.round_winner,
        limit: t.limit.map(|l| match l {
            Limit::Score => "score",
            Limit::Time => "time",
        }),
        cut: t.cut,
        ops: t.ops,
        worst_tick_ops: t.worst_tick_ops,
        handlers_ran: t.handlers_ran,
        scores: t
            .scores
            .into_iter()
            .map(|(slot, score)| WireScore { slot, score })
            .collect(),
    }
}

fn eight() -> u8 {
    8
}

fn one_seed() -> u64 {
    1
}

fn one_match() -> u32 {
    1
}

/// `POST /v1/rules/simulate`'s body: the source and what to play.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SimBody {
    source: String,
    #[serde(default = "eight")]
    players: u8,
    /// 0 or 1 is free for all.
    #[serde(default)]
    teams: u8,
    #[serde(default = "one_seed")]
    seed: u64,
    /// Matches to play and sum up, seeds `seed` upwards; the answer's
    /// `match` is `seed`'s.
    #[serde(default = "one_match")]
    matches: u32,
    /// Seconds a match lasts at most.
    #[serde(default)]
    length: Option<u32>,
    #[serde(default)]
    score_limit: Option<i32>,
}

/// A generated match in full.
#[derive(Debug, Serialize)]
struct MatchView {
    seed: u64,
    end: sim::End,
    why: Option<String>,
    winner: sim::Winner,
    top_score: i32,
    scenario: WireScenario,
    trace: TraceView,
}

/// `POST /v1/rules/simulate`'s answer.
#[derive(Debug, Serialize)]
struct SimView {
    ok: bool,
    /// The teams played with; 0 is free for all.
    teams: u8,
    /// Present for a batch of more than one match.
    summary: Option<sim::Summary>,
    rows: Vec<sim::Row>,
    #[serde(rename = "match")]
    shown: MatchView,
}

/// Bucket tokens a simulate request costs: one for a match, two for a
/// batch, and one more for each further 25 matches.
fn sim_cost(matches: u32) -> u32 {
    if matches <= 1 {
        1
    } else {
        2 + (matches - 1) / 25
    }
}

fn simulate(b: &SimBody) -> Result<SimView, RunError> {
    let setup = sim::Setup {
        players: b.players,
        teams: b.teams,
        seed: b.seed,
        length: b.length,
        score_limit: b.score_limit,
    };
    let (summary, rows) = if b.matches == 1 {
        (None, Vec::new())
    } else {
        let batch = sim::batch(&b.source, &setup, b.matches)?;
        (Some(batch.summary), batch.rows)
    };
    let m = sim::simulate(&b.source, &setup)?;
    Ok(SimView {
        ok: true,
        teams: m.teams,
        summary,
        rows,
        shown: MatchView {
            seed: m.seed,
            end: m.end,
            why: m.why,
            winner: m.winner,
            top_score: m.top_score,
            scenario: WireScenario::of(&m.scenario),
            trace: trace_view(m.trace),
        },
    })
}

/// `POST /v1/rules/ide`'s body: the source, the operation, and the place
/// (0-based line, UTF-16 character) for the operations that take one.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct IdeBody {
    source: String,
    op: String,
    #[serde(default)]
    line: Option<u32>,
    #[serde(default)]
    character: Option<u32>,
}

/// One editor operation over `rules::ide`, answered in LSP's shapes.
fn ide_answer(b: &IdeBody) -> ApiResult<serde_json::Value> {
    use rules::ide;
    let at = || match (b.line, b.character) {
        (Some(line), Some(character)) => Ok(ide::Position::new(line, character)),
        _ => Err(invalid(format!("op {} needs line and character", b.op))),
    };
    let src = b.source.as_str();
    Ok(match b.op.as_str() {
        "complete" => json!(ide::complete(src, at()?)),
        "hover" => json!(ide::hover(src, at()?)),
        "signature" => json!(ide::signature_help(src, at()?)),
        "definition" => json!(ide::definition(src, at()?)),
        "references" => json!(ide::references(src, at()?)),
        "tokens" => json!({"legend": ide::LEGEND, "data": ide::semantic_tokens(src)}),
        "symbols" => json!(ide::symbols(src)),
        "diagnostics" => json!(ide::diagnostics(src)),
        "format" => match ide::format(src) {
            Ok(text) => json!({ "text": text }),
            Err(e) => json!({"error": e.to_string(), "line": e.line, "col": e.col}),
        },
        other => {
            return Err(invalid(format!(
                "unknown op {other:?}: complete, hover, signature, definition, references, tokens, symbols, format or diagnostics"
            )));
        }
    })
}

// ---------- the service ----------

fn source_of(body: Result<Bytes, BytesRejection>) -> ApiResult<SourceBody> {
    let bytes = body_bytes(body)?;
    let b: SourceBody =
        serde_json::from_slice(&bytes).map_err(|e| invalid(format!("body: {e}")))?;
    capped(&b.source)?;
    Ok(b)
}

fn capped(source: &str) -> ApiResult<()> {
    if source.len() > MAX_SOURCE {
        return Err(ApiError::new(
            ErrorCode::TooLarge,
            format!("a script's source is at most {MAX_SOURCE} bytes"),
        ));
    }
    Ok(())
}

fn summary(live: &Live, k: &Kept) -> ScriptSummary {
    ScriptSummary {
        hash: k.saved.hash.clone(),
        name: k.saved.name.clone(),
        version: k.saved.version,
        author: k.saved.author,
        gamertag: live.who_is(k.saved.author).gamertag,
        saved_at: k.saved_at.clone(),
        position: k.position,
    }
}

impl Live {
    /// Charges a public workbench request to the client's bucket.
    pub(crate) fn limit_workbench(&self, ip: &str) -> ApiResult<()> {
        if self.rules_rate.allow(ip) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::RateLimited,
                "too many checks; try again shortly",
            ))
        }
    }

    /// Charges an editor request to the client's own bucket, at the
    /// workbench's rate.
    fn limit_ide(&self, ip: &str) -> ApiResult<()> {
        if self.rules_ide_rate.allow(ip) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::RateLimited,
                "too many editor requests; try again shortly",
            ))
        }
    }

    /// `POST /v1/rules`: an account saves a script that verifies and fits
    /// its budget. Returns the script and whether this request saved it.
    pub async fn save_rules(&self, bearer: Option<&str>, source: &str) -> ApiResult<(Script, bool)> {
        let author = self.authenticate(bearer, lxm::CONTENT_WRITE)?;
        let xuid = match &author {
            Author::Account { xuid, .. } => *xuid,
            _ => return Err(crate::app::forbidden("only an account saves a script")),
        };
        self.rate_limit(&author, lxm::CONTENT_WRITE)?;
        capped(source)?;
        let check = workbench::check(source);
        if !check.ok {
            let first = check
                .errors
                .first()
                .map_or_else(|| "it does not verify".to_string(), |p| p.message.clone());
            return Err(invalid(format!("the script does not verify: {first}")));
        }
        let hash = check.hash.expect("a checked script has a hash");
        let _guard = self.rules_writes.lock().await;
        if let Some(s) = self.saved_rules(&hash) {
            return Ok((s, false));
        }
        if self.rules.read().count_by(xuid) >= MAX_PER_AUTHOR {
            return Err(conflict(format!(
                "an account saves at most {MAX_PER_AUTHOR} scripts"
            )));
        }
        let data = Saved {
            hash: hash.clone(),
            name: check.name.unwrap_or_default(),
            version: check.version.unwrap_or(0),
            author: xuid,
            source: source.to_string(),
        };
        let stream = wire::stream(&hash);
        let event = self.server_event(
            stream.clone(),
            RULES_SAVED,
            serde_json::to_value(data).expect("serializes"),
        );
        match self.store.append(vec![event], &[(stream, 0)]).await {
            Ok(recorded) => self.wait_applied(recorded[0].position).await?,
            // Another server on the log saved it first.
            Err(StoreError::Conflict { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        self.saved_rules(&hash)
            .map(|s| (s, true))
            .ok_or_else(|| ApiError::new(ErrorCode::Unavailable, "the script is not yet readable"))
    }

    /// `GET /v1/rules/{hash}`.
    pub fn saved_rules(&self, hash: &str) -> Option<Script> {
        let s = self.rules.read();
        let k = s.by_hash.get(hash)?;
        Some(Script {
            summary: summary(self, k),
            source: k.saved.source.clone(),
        })
    }

    /// `GET /v1/rules?author=`: an author's scripts (gamertag or xuid),
    /// newest first.
    pub fn rules_by(&self, author: &str) -> ApiResult<ScriptList> {
        let xuid = match Xuid::parse(author) {
            Ok(x) if author.len() == 16 => x,
            _ => {
                let tag = Gamertag::new(author).map_err(|_| not_found("no such account"))?;
                self.model
                    .read()
                    .account_by_tag(&tag)
                    .map(|a| a.xuid)
                    .ok_or_else(|| not_found("no such account"))?
            }
        };
        let s = self.rules.read();
        let items = s
            .by_author
            .get(&xuid)
            .into_iter()
            .flatten()
            .rev()
            .filter_map(|h| s.by_hash.get(h))
            .map(|k| summary(self, k))
            .collect();
        Ok(ScriptList { items })
    }
}

// ---------- route handlers ----------

type Shared = State<Arc<Live>>;

async fn post_check(
    State(live): Shared,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    live.limit_workbench(&ip)?;
    let b = source_of(body)?;
    let mut view = check_view(workbench::check(&b.source));
    if view.ok
        && let Ok(ir) = rules::compile(&b.source)
    {
        view.options = crate::game_variants::option_views(&ir);
    }
    Ok(crate::http::json(StatusCode::OK, &view))
}

async fn post_run(
    State(live): Shared,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    live.limit_workbench(&ip)?;
    let bytes = body_bytes(body)?;
    let b: RunBody = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("body: {e}")))?;
    capped(&b.source)?;
    match workbench::run(&b.source, &b.scenario.into_scenario()) {
        Ok(t) => Ok(crate::http::json(StatusCode::OK, &trace_view(t))),
        // A script that does not build answers like a check.
        Err(RunError::Script(errors)) => Ok(crate::http::json(
            StatusCode::OK,
            &json!({
                "ok": false,
                "errors": errors.into_iter().map(WireProblem::from).collect::<Vec<_>>(),
            }),
        )),
        Err(RunError::Scenario(m)) => Err(invalid(format!("scenario: {m}"))),
        // A map the script's labels refuse answers like a check, and
        // nothing runs.
        Err(RunError::Map(problems)) => Ok(crate::http::json(
            StatusCode::OK,
            &json!({ "ok": false, "errors": [], "map": problems }),
        )),
    }
}

/// A script that does not build answers like a check.
fn not_built(errors: Vec<Problem>) -> Response {
    crate::http::json(
        StatusCode::OK,
        &json!({
            "ok": false,
            "errors": errors.into_iter().map(WireProblem::from).collect::<Vec<_>>(),
        }),
    )
}

async fn post_simulate(
    State(live): Shared,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    live.limit_workbench(&ip)?;
    let bytes = body_bytes(body)?;
    let b: SimBody = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("body: {e}")))?;
    capped(&b.source)?;
    // A batch is charged for before it is played.
    for _ in 1..sim_cost(b.matches.min(sim::MAX_MATCHES)) {
        live.limit_workbench(&ip)?;
    }
    match simulate(&b) {
        Ok(view) => Ok(crate::http::json(StatusCode::OK, &view)),
        Err(RunError::Script(errors)) => Ok(not_built(errors)),
        Err(RunError::Scenario(m)) => Err(invalid(format!("setup: {m}"))),
        // A generated match plays on the script's minimal map, which the
        // check accepts; a script no map can satisfy has none to play.
        Err(RunError::Map(problems)) => Err(invalid(format!("setup: {}", problems.join("; ")))),
    }
}

async fn post_ide(
    State(live): Shared,
    Extension(ClientIp(ip)): Extension<ClientIp>,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    live.limit_ide(&ip)?;
    let bytes = body_bytes(body)?;
    let b: IdeBody = serde_json::from_slice(&bytes).map_err(|e| invalid(format!("body: {e}")))?;
    capped(&b.source)?;
    Ok(crate::http::json(StatusCode::OK, &ide_answer(&b)?))
}

async fn get_ide_legend() -> Response {
    crate::http::json(StatusCode::OK, &json!({ "legend": rules::ide::LEGEND }))
}

async fn get_builtin(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    // The engine ports, then the examples: "Start from..." offers both.
    let mut items: Vec<_> = rules::builtin::ALL
        .iter()
        .map(|b| {
            json!({
                "name": b.name,
                "engine": b.engine,
                "source": b.source,
                "hash": rules::compile(b.source).ok().map(|ir| ir.id().to_string()),
                "example": false,
            })
        })
        .collect();
    for (key, source) in rules::builtin::examples() {
        let ir = rules::compile(source).ok();
        items.push(json!({
            "name": ir.as_ref().map_or(*key, |ir| ir.header.name.as_str()),
            "key": key,
            "engine": ir.as_ref().map(|ir| ir.header.carrier),
            "source": source,
            "hash": ir.as_ref().map(|ir| ir.id().to_string()),
            "example": true,
        }));
    }
    Ok(crate::http::json(StatusCode::OK, &json!({ "items": items })))
}

async fn post_save(
    State(live): Shared,
    headers: HeaderMap,
    body: Result<Bytes, BytesRejection>,
) -> ApiResult<Response> {
    let b = source_of(body)?;
    let (script, saved) = live.save_rules(bearer(&headers), &b.source).await?;
    let status = if saved {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    Ok(crate::http::json(
        status,
        &json!({"ok": true, "saved": saved, "script": script}),
    ))
}

#[derive(Debug, Deserialize)]
struct ListQuery {
    author: Option<String>,
}

async fn get_list(
    State(live): Shared,
    headers: HeaderMap,
    Query(q): Query<ListQuery>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let author = q
        .author
        .filter(|a| !a.trim().is_empty())
        .ok_or_else(|| invalid("?author= (a gamertag or xuid) is required"))?;
    Ok(crate::http::json(StatusCode::OK, &live.rules_by(author.trim())?))
}

fn hash_param(hash: &str) -> ApiResult<&str> {
    if wire::valid_hash(hash) {
        Ok(hash)
    } else {
        Err(not_found("no such script"))
    }
}

async fn get_script(
    State(live): Shared,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let s = live
        .saved_rules(hash_param(&hash)?)
        .ok_or_else(|| not_found("no such script"))?;
    Ok(crate::http::json(StatusCode::OK, &s))
}

/// The IR blob, compiled again from the saved source.
async fn get_ir(
    State(live): Shared,
    headers: HeaderMap,
    Path(hash): Path<String>,
) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let s = live
        .saved_rules(hash_param(&hash)?)
        .ok_or_else(|| not_found("no such script"))?;
    let ir = rules::compile(&s.source)
        .map_err(|e| ApiError::new(ErrorCode::Unavailable, format!("the saved source: {e}")))?;
    if ir.id().to_string() != hash {
        return Err(ApiError::new(
            ErrorCode::Unavailable,
            "this build compiles the saved source to another blob",
        ));
    }
    let mut resp = ir.encode().into_response();
    let h = resp.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    h.insert(
        header::CACHE_CONTROL,
        HeaderValue::from_static("public, max-age=31536000, immutable"),
    );
    Ok(resp)
}

/// The workbench and saved-script routes, with their own body cap.
pub(crate) fn routes() -> Router<Arc<Live>> {
    Router::new()
        .route("/v1/rules", get(get_list).post(post_save))
        .route("/v1/rules/check", post(post_check))
        .route("/v1/rules/run", post(post_run))
        .route("/v1/rules/simulate", post(post_simulate))
        .route("/v1/rules/ide", post(post_ide))
        .route("/v1/rules/ide/legend", get(get_ide_legend))
        .route("/v1/rules/builtin", get(get_builtin))
        .route("/v1/rules/{hash}", get(get_script))
        .route("/v1/rules/{hash}/ir", get(get_ir))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}
