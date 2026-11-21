//! The rules workbench and saved game-type scripts (docs/live/live-api.md
//! "Rules scripts", docs/game/rules-engine.md "Workbench").
//!
//! `check` and `run` compile and run a script with the `rules` crate on
//! the server, so the web app needs no wasm toolchain: `check` measures a
//! script (`rules::workbench::check`) and `run` steps a scenario over
//! `rules::fake::FakeGame` (`rules::workbench::run`). Both are public and
//! rate-limited per client address.
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
use rules::workbench::{self, Problem, RunError, Scenario, Seat, Step};
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
}

fn check_view(c: workbench::Check) -> CheckView {
    CheckView {
        ok: c.ok,
        hash: c.hash,
        name: c.name,
        version: c.version,
        errors: c.errors.into_iter().map(Into::into).collect(),
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
        }),
    }
}

#[derive(Debug, Deserialize)]
struct SourceBody {
    source: String,
}

/// A seat of `POST /v1/rules/run`'s scenario.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSeat {
    slot: u8,
    #[serde(default)]
    team: u8,
    #[serde(default)]
    lives: i32,
}

/// One event of a scenario. `type` is the handler's event name.
#[derive(Debug, Deserialize)]
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
    Bounce {
        player: u8,
        object: u32,
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
            WireEvent::Bounce { player, object } => Input::Bounce(player, object),
            WireEvent::Edit { player, object } => Input::Edit(player, object),
            WireEvent::Enter { zone, object } => Input::Enter { zone, object },
            WireEvent::Leave { zone, object } => Input::Leave { zone, object },
            WireEvent::SquadDead { squad } => Input::SquadDead(squad),
            WireEvent::ActorKill { killer, class } => Input::ActorKill { killer, class },
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTick {
    tick: u32,
    #[serde(default)]
    events: Vec<WireEvent>,
}

fn yes() -> bool {
    true
}

/// `POST /v1/rules/run`'s scenario.
#[derive(Debug, Deserialize)]
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
                })
                .collect(),
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
    cut: Option<String>,
    ops: u64,
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
        cut: t.cut,
        ops: t.ops,
        scores: t
            .scores
            .into_iter()
            .map(|(slot, score)| WireScore { slot, score })
            .collect(),
    }
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
    fn limit_workbench(&self, ip: &str) -> ApiResult<()> {
        if self.rules_rate.allow(ip) {
            Ok(())
        } else {
            Err(ApiError::new(
                ErrorCode::RateLimited,
                "too many checks; try again shortly",
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
    Ok(crate::http::json(StatusCode::OK, &check_view(workbench::check(&b.source))))
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
    }
}

async fn get_builtin(State(live): Shared, headers: HeaderMap) -> ApiResult<Response> {
    live.authorize_read(bearer(&headers))?;
    let items: Vec<_> = rules::builtin::ALL
        .iter()
        .map(|b| {
            json!({
                "name": b.name,
                "engine": b.engine,
                "source": b.source,
                "hash": rules::compile(b.source).ok().map(|ir| ir.id().to_string()),
            })
        })
        .collect();
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
        .route("/v1/rules/builtin", get(get_builtin))
        .route("/v1/rules/{hash}", get(get_script))
        .route("/v1/rules/{hash}/ir", get(get_ir))
        .layer(DefaultBodyLimit::max(MAX_BODY))
}
