//! Usage accounting for the Live admin (`GET /v1/admin/usage`,
//! `GET /v1/admin/usage/accounts`; docs/live/live-api.md "Usage",
//! docs/live/web-boundary.md §5): what each account uploaded, stores and
//! did, per UTC day, for the last [`KEEP_DAYS`] days.
//!
//! Two sources, one book each, merged when read:
//!
//! - **From the log** ([`Usage`] is a [`Subscriber`]): events written (by
//!   author and by type), the bytes of every event an account signed (its
//!   JWS as received; a batch once), messages sent, games played,
//!   sessions hosted, sign-ins, and what the log says is stored (event
//!   bytes, content packages). Replayed from the log at startup like the
//!   read model, so it needs no storage of its own.
//! - **Metered** at the request ([`Live::meter`]): API requests, bytes
//!   received for live frames, content uploads and cloud saves. The native
//!   server batches deltas for up to [`FLUSH_EVERY_S`] seconds. The Durable
//!   Object flushes after `WORKER_FLUSH_EVERY_S` seconds or
//!   `WORKER_PENDING_LIMIT` meter increments, whichever comes first. Its
//!   sweep alarm flushes small bursts even if no more traffic arrives.
//!   The first write of a new day also drops expired daily counters.
//!
//! The request counter measures work that reaches the API, including
//! WebSocket upgrade requests. Rate-limited requests, static assets,
//! edge-cache hits, and WebSocket messages do not enter that counter.
//!
//! Stored save bytes come from an inventory of the private blob store
//! (`Live::stored_save_bytes`). The other reads use the in-memory books.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::{Author, Xuid, game, mailbox, reports, session, types};
use serde::Serialize;
use serde_json::Value;

use crate::app::{ApiError, Live, invalid};
use crate::dispatch::Subscriber;
use crate::store::{RecordedEvent, UsageRow};

/// How many days of daily counters are kept.
pub const KEEP_DAYS: i64 = 400;
/// The native server's interval for batching metered counts.
pub const FLUSH_EVERY_S: i64 = 60;
/// The Durable Object's interval for flushing pending counters.
const WORKER_FLUSH_EVERY_S: i64 = 5;
/// Flush a busy Durable Object's batch early after this many meter calls.
const WORKER_PENDING_LIMIT: usize = 256;
/// The most days a read covers.
pub const MAX_RANGE_DAYS: i64 = KEEP_DAYS;
/// The most accounts a top-N read returns.
pub const MAX_ACCOUNTS: usize = 500;

/// The counters' names, as stored. `messages.*` has one per kind of
/// message, so text messages join as `messages.text` when there are any.
pub mod metric {
    /// HTTP requests reaching the Live API service (server-wide).
    pub const REQUESTS: &str = "requests";
    /// Bytes of events an account signed, as received.
    pub const UP_EVENTS: &str = "up.events";
    /// Live frames off the ingest socket (text and packed), as received.
    pub const UP_FRAMES: &str = "up.frames";
    /// Content: map packages and their images, outlines, title files.
    pub const UP_CONTENT: &str = "up.content";
    /// Cloud save bodies.
    pub const UP_SAVES: &str = "up.saves";
    pub const EVENTS: &str = "events";
    pub const FRIEND_REQUESTS: &str = "messages.friend_requests";
    pub const INVITES: &str = "messages.invites";
    pub const GAMES: &str = "games";
    pub const SESSIONS_HOSTED: &str = "sessions_hosted";
    pub const SIGN_INS: &str = "sign_ins";
    /// Player reports an account made, and reports made about it
    /// (`PlayerReported`, server-signed for the reporter).
    pub const REPORTS_SENT: &str = "reports.sent";
    pub const REPORTS_RECEIVED: &str = "reports.received";
    /// Dayless running totals.
    pub const STORED_EVENTS: &str = "stored.events";
    pub const STORED_CONTENT: &str = "stored.content";
    /// A response key. Legacy metered save deltas have no storage baseline.
    pub const STORED_SAVES: &str = "stored.saves";

    pub const UPLOADS: [&str; 4] = [UP_EVENTS, UP_FRAMES, UP_CONTENT, UP_SAVES];
    pub const MESSAGES: [&str; 2] = [FRIEND_REQUESTS, INVITES];
    pub const STORED: [&str; 3] = [STORED_EVENTS, STORED_CONTENT, STORED_SAVES];
}

/// The account an author is, if it is one.
pub fn author_xuid(a: &Author) -> Option<Xuid> {
    match a {
        Author::Account { xuid, .. } => Some(*xuid),
        Author::Server(_) => None,
    }
}

/// The account a bearer token (already checked by the call it came with) was issued by.
pub fn bearer_xuid(bearer: Option<&str>) -> Option<Xuid> {
    let token = live_proto::sign::Token::parse(bearer?).ok()?;
    author_xuid(&Author::parse(&token.claims.iss)?)
}

/// The account key a counter is under: the xuid in hex, or empty for the server.
pub fn account_key(x: Option<Xuid>) -> String {
    x.map_or_else(String::new, |x| x.to_string())
}

/// `YYYY-MM-DD` for a unix second.
pub fn day_of(unix: i64) -> String {
    let d = time::OffsetDateTime::from_unix_timestamp(unix.max(0))
        .unwrap_or(time::OffsetDateTime::UNIX_EPOCH)
        .date();
    format!("{:04}-{:02}-{:02}", d.year(), u8::from(d.month()), d.day())
}

/// The first day kept on `today`'s date.
pub fn keep_from(now: i64) -> String {
    day_of(now - (KEEP_DAYS - 1) * 86_400)
}

/// An event's day, from its signed time (`at`, RFC 3339).
fn event_day(at: &str) -> Option<&str> {
    let d = at.get(..10)?;
    let b = d.as_bytes();
    (b[4] == b'-'
        && b[7] == b'-'
        && d.bytes()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit()))
    .then_some(d)
}

/// Counters by day, then account, then metric; and dayless totals.
#[derive(Debug, Default, Clone)]
struct Book {
    days: BTreeMap<String, HashMap<String, HashMap<String, i64>>>,
    totals: HashMap<String, HashMap<String, i64>>,
}

impl Book {
    fn add(&mut self, day: &str, account: &str, metric: &str, n: i64) {
        let at = if day.is_empty() {
            self.totals.entry(account.to_string()).or_default()
        } else {
            self.days
                .entry(day.to_string())
                .or_default()
                .entry(account.to_string())
                .or_default()
        };
        *at.entry(metric.to_string()).or_default() += n;
    }

    fn prune(&mut self, keep_from: &str) {
        self.days = self.days.split_off(keep_from);
    }
}

/// What the log fold keeps besides its book.
#[derive(Default)]
struct Derived {
    book: Book,
    /// Event counts by day and type.
    types: BTreeMap<String, BTreeMap<String, u64>>,
    /// (game stream, account) already counted as a game played.
    played: HashSet<(String, String)>,
}

#[derive(Default)]
struct Metered {
    book: Book,
    pending: BTreeMap<(String, String, String), i64>,
    increments: usize,
    last_flush: i64,
    last_day: String,
    prune_pending: bool,
}

/// The usage book ([module docs](self)).
pub struct Usage {
    derived: RwLock<Derived>,
    metered: Mutex<Metered>,
    flush_every_s: i64,
    pending_limit: usize,
    /// Today's first kept day, for replays of old events; from the clock.
    clock: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl Default for Usage {
    fn default() -> Self {
        Usage::with_clock(Box::new(crate::app::now))
    }
}

impl Usage {
    pub(crate) fn worker() -> Self {
        Self::worker_with_clock(Box::new(crate::app::now))
    }

    fn worker_with_clock(clock: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        Self {
            flush_every_s: WORKER_FLUSH_EVERY_S,
            pending_limit: WORKER_PENDING_LIMIT,
            ..Self::with_clock(clock)
        }
    }

    pub fn with_clock(clock: Box<dyn Fn() -> i64 + Send + Sync>) -> Self {
        let now = clock();
        Usage {
            derived: RwLock::default(),
            metered: Mutex::new(Metered {
                last_flush: now,
                last_day: day_of(now),
                ..Default::default()
            }),
            flush_every_s: FLUSH_EVERY_S,
            pending_limit: usize::MAX,
            clock,
        }
    }

    /// The counters the store kept, read once at startup.
    pub fn load(&self, rows: &[UsageRow]) {
        let keep = keep_from((self.clock)());
        let mut m = self.metered.lock().unwrap();
        for r in rows {
            if !r.day.is_empty() && r.day < keep {
                m.prune_pending = true;
            } else if r.metric != metric::STORED_SAVES {
                m.book.add(&r.day, &r.account, &r.metric, r.n);
            }
        }
    }

    /// Counts `n` of `metric` for `account` today (dayless metrics: on its total).
    pub fn meter(&self, account: Option<Xuid>, metric: &str, n: i64) {
        if n == 0 {
            return;
        }
        let day = if metric.starts_with("stored.") {
            String::new()
        } else {
            day_of((self.clock)())
        };
        let account = account_key(account);
        let mut m = self.metered.lock().unwrap();
        m.book.add(&day, &account, metric, n);
        *m.pending
            .entry((day, account, metric.to_string()))
            .or_default() += n;
        m.increments = m.increments.saturating_add(1);
    }

    fn pending_due(&self, metered: &Metered) -> Option<i64> {
        if metered.increments >= self.pending_limit {
            Some(metered.last_flush)
        } else if !metered.pending.is_empty() || metered.prune_pending {
            Some(metered.last_flush + self.flush_every_s)
        } else {
            None
        }
    }

    /// When an outstanding batch needs the sweep, even if traffic stops.
    pub(crate) fn flush_due(&self) -> Option<i64> {
        self.pending_due(&self.metered.lock().unwrap())
    }

    /// The rows to write now, if a write is due: a new day (which also cuts
    /// the days past the keep) or pending deltas due to be flushed. Taken out:
    /// a failed write hands them back with [`Usage::unflushed`]. An empty
    /// cutoff means there is nothing to prune on this write.
    pub fn due(&self) -> Option<(Vec<UsageRow>, String)> {
        let now = (self.clock)();
        let today = day_of(now);
        let mut m = self.metered.lock().unwrap();
        let new_day = today != m.last_day;
        if !new_day && self.pending_due(&m).is_none_or(|at| now < at) {
            return None;
        }
        m.last_flush = now;
        m.increments = 0;
        let prune = std::mem::take(&mut m.prune_pending) || new_day;
        let keep = if prune { keep_from(now) } else { String::new() };
        if new_day {
            m.last_day = today;
            m.book.prune(&keep);
            self.derived.write().unwrap().prune(&keep);
        }
        let rows = std::mem::take(&mut m.pending)
            .into_iter()
            .filter(|(_, n)| *n != 0)
            .map(|((day, account, metric), n)| UsageRow {
                day,
                account,
                metric,
                n,
            })
            .collect();
        Some((rows, keep))
    }

    /// Rows a write could not take, back in line for the next.
    pub fn unflushed(&self, rows: Vec<UsageRow>, keep_from: &str) {
        let mut m = self.metered.lock().unwrap();
        m.prune_pending |= !keep_from.is_empty();
        for r in rows {
            *m.pending.entry((r.day, r.account, r.metric)).or_default() += r.n;
        }
    }

    /// Unwritten metered rows (tests, and a shutdown that wants them).
    pub fn pending(&self) -> usize {
        self.metered.lock().unwrap().pending.len()
    }
}

impl Derived {
    fn prune(&mut self, keep: &str) {
        self.book.prune(keep);
        self.types = self.types.split_off(keep);
    }

    fn apply(&mut self, e: &RecordedEvent, keep: &str) {
        let b = &e.body;
        let author = match Author::parse(&b.author) {
            Some(Author::Account { xuid, .. }) => Some(xuid),
            _ => None,
        };
        let who = account_key(author);
        // A batch's JWS once, on its first event.
        let jws_bytes = if e.jws_index.unwrap_or(0) == 0 {
            e.jws.len() as i64
        } else {
            0
        };
        self.book.add("", &who, metric::STORED_EVENTS, jws_bytes);
        if b.kind == live_proto::maps::CONTENT_STORED
            && let (Some(size), Some(up)) = (
                b.data.get("size").and_then(Value::as_i64),
                xuid_at(&b.data, "uploader"),
            )
        {
            self.book
                .add("", &account_key(Some(up)), metric::STORED_CONTENT, size);
        }
        let Some(day) = event_day(&b.at) else { return };
        if day < keep {
            return;
        }
        *self
            .types
            .entry(day.to_string())
            .or_default()
            .entry(b.kind.clone())
            .or_default() += 1;
        self.book.add(day, &who, metric::EVENTS, 1);
        if author.is_some() {
            self.book.add(day, &who, metric::UP_EVENTS, jws_bytes);
        }
        if b.kind == reports::PLAYER_REPORTED {
            for (field, m) in [
                ("by", metric::REPORTS_SENT),
                ("target", metric::REPORTS_RECEIVED),
            ] {
                if let Some(x) = xuid_at(&b.data, field) {
                    self.book.add(day, &account_key(Some(x)), m, 1);
                }
            }
            return;
        }
        // Everything else the server signs for itself (ReportReviewed,
        // RankChanged, GameFrame, PresenceExpired, ...) counts as an event
        // of the server's, by type, and nothing more.
        let counted = match b.kind.as_str() {
            types::FRIEND_REQUESTED => (author, metric::FRIEND_REQUESTS),
            mailbox::SENT => (xuid_at(&b.data, "from").or(author), metric::INVITES),
            types::SIGNED_IN => (author, metric::SIGN_INS),
            session::CREATED if b.stream.starts_with("session-") => (
                author.or_else(|| xuid_at(&b.data, "host_xuid")),
                metric::SESSIONS_HOSTED,
            ),
            // The server writes it for its host (`host`, an author), with
            // the players the game began with.
            game::GAME_STARTED if b.stream.starts_with("game-") => {
                let host = b
                    .data
                    .get("host")
                    .and_then(Value::as_str)
                    .and_then(Author::parse);
                self.played(
                    &b.stream,
                    day,
                    host.as_ref().and_then(author_xuid).or(author),
                );
                for p in b
                    .data
                    .get("players")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                {
                    self.played(&b.stream, day, xuid_at(p, "xuid"));
                }
                return;
            }
            game::PLAYER_JOINED if b.stream.starts_with("game-") => {
                let x = b.data.get("player").and_then(|p| xuid_at(p, "xuid"));
                return self.played(&b.stream, day, x);
            }
            _ => return,
        };
        if let (Some(x), m) = counted {
            self.book.add(day, &account_key(Some(x)), m, 1);
        }
    }

    /// One game for `who` per game, however often they join it.
    fn played(&mut self, stream: &str, day: &str, who: Option<Xuid>) {
        let Some(x) = who else { return };
        let key = account_key(Some(x));
        if self.played.insert((stream.to_string(), key.clone())) {
            self.book.add(day, &key, metric::GAMES, 1);
        }
    }
}

fn xuid_at(v: &Value, field: &str) -> Option<Xuid> {
    Xuid::parse(v.get(field)?.as_str()?).ok()
}

impl Subscriber for Usage {
    fn name(&self) -> &str {
        "usage"
    }

    fn apply(&self, event: &RecordedEvent) {
        let keep = keep_from((self.clock)());
        self.derived.write().unwrap().apply(event, &keep);
    }
}

// ---------- reads ----------

/// One day's server-wide numbers.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct DayUsage {
    pub day: String,
    /// Live API requests, if present in the metered history.
    pub requests: Option<i64>,
    /// Bytes uploaded by kind: `events`, `frames`, `content`, `saves`.
    pub up: BTreeMap<String, i64>,
    pub events: i64,
    /// Messages sent by kind: `friend_requests`, `invites`.
    pub messages: BTreeMap<String, i64>,
    pub games: i64,
    pub sessions_hosted: i64,
    pub sign_ins: i64,
    /// Player reports made.
    pub reports: i64,
    /// Accounts that did anything that day.
    pub active_accounts: i64,
}

/// One account's numbers over a range, and what it stores now.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct AccountUsage {
    pub xuid: String,
    pub gamertag: Option<String>,
    pub up: BTreeMap<String, i64>,
    pub uploaded: i64,
    pub stored: BTreeMap<String, i64>,
    pub stored_total: i64,
    pub events: i64,
    pub messages: BTreeMap<String, i64>,
    pub messages_total: i64,
    pub games: i64,
    pub sessions_hosted: i64,
    pub sign_ins: i64,
    pub reports_sent: i64,
    pub reports_received: i64,
    pub active_days: i64,
    pub last_active: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct TypeCount {
    #[serde(rename = "type")]
    pub kind: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Overview {
    pub from: String,
    pub to: String,
    pub keep_days: i64,
    /// Earliest retained UTC day with a request counter.
    pub request_count_from: Option<String>,
    /// Only days with something in them, oldest first.
    pub days: Vec<DayUsage>,
    pub totals: DayUsage,
    /// Distinct accounts active in the range (`totals.active_accounts` sums days).
    pub accounts_active: i64,
    pub event_types: Vec<TypeCount>,
    /// Stored by kind, over every account: `events`, `content`, `saves`.
    pub stored: BTreeMap<String, i64>,
    /// Whether the response includes the private store's save inventory.
    pub saves_storage_measured: bool,
}

/// A metric's name within its group: `up.frames` is `frames`.
fn short(metric: &str) -> &str {
    ["up.", "messages.", "stored."]
        .iter()
        .find_map(|p| metric.strip_prefix(p))
        .unwrap_or(metric)
}

/// A sum of counters for one account (or the server) over one or more days.
#[derive(Default)]
struct Sum(HashMap<String, i64>);

impl Sum {
    fn add(&mut self, m: &HashMap<String, i64>) {
        for (k, v) in m {
            *self.0.entry(k.clone()).or_default() += v;
        }
    }
    fn get(&self, m: &str) -> i64 {
        self.0.get(m).copied().unwrap_or(0)
    }
    /// Whether the account did anything: what is stored, and reports
    /// made about it, are not its doing.
    fn any(&self) -> bool {
        self.0
            .iter()
            .any(|(k, v)| *v != 0 && !k.starts_with("stored.") && k != metric::REPORTS_RECEIVED)
    }
    fn by(&self, ms: &[&str]) -> BTreeMap<String, i64> {
        ms.iter()
            .map(|m| (short(m).to_string(), self.get(m)))
            .collect()
    }
}

impl Usage {
    /// The days `[from, to]` inclusive, ending today, `days` long.
    fn range(&self, days: i64) -> (String, String) {
        let now = (self.clock)();
        (day_of(now - (days - 1) * 86_400), day_of(now))
    }

    /// Each day's per-account sums from both books, for the days in range.
    fn per_day(&self, from: &str, to: &str) -> BTreeMap<String, HashMap<String, Sum>> {
        let mut out: BTreeMap<String, HashMap<String, Sum>> = BTreeMap::new();
        let mut take = |b: &Book| {
            for (day, accts) in b.days.range(from.to_string()..=to.to_string()) {
                let d = out.entry(day.clone()).or_default();
                for (a, m) in accts {
                    d.entry(a.clone()).or_default().add(m);
                }
            }
        };
        take(&self.derived.read().unwrap().book);
        take(&self.metered.lock().unwrap().book);
        out
    }

    fn totals(&self) -> HashMap<String, Sum> {
        let mut out: HashMap<String, Sum> = HashMap::new();
        for b in [
            &self.derived.read().unwrap().book.totals,
            &self.metered.lock().unwrap().book.totals,
        ] {
            for (a, m) in b {
                out.entry(a.clone()).or_default().add(m);
            }
        }
        out
    }

    fn day_row(day: &str, s: &Sum, active: i64) -> DayUsage {
        DayUsage {
            day: day.to_string(),
            requests: s.0.get(metric::REQUESTS).copied(),
            up: s.by(&metric::UPLOADS),
            events: s.get(metric::EVENTS),
            messages: s.by(&metric::MESSAGES),
            games: s.get(metric::GAMES),
            sessions_hosted: s.get(metric::SESSIONS_HOSTED),
            sign_ins: s.get(metric::SIGN_INS),
            reports: s.get(metric::REPORTS_SENT),
            active_accounts: active,
        }
    }

    /// The server's numbers over the last `days` days.
    pub fn overview(&self, days: i64) -> Overview {
        let (from, to) = self.range(days);
        let per_day = self.per_day(&from, &to);
        let mut all = Sum::default();
        let mut active = HashSet::new();
        let mut rows = Vec::new();
        for (day, accts) in &per_day {
            let mut s = Sum::default();
            let mut n = 0;
            for (a, m) in accts {
                s.add(&m.0);
                if !a.is_empty() && m.any() {
                    n += 1;
                    active.insert(a.clone());
                }
            }
            all.add(&s.0);
            rows.push(Self::day_row(day, &s, n));
        }
        let mut totals = Self::day_row("", &all, rows.iter().map(|d| d.active_accounts).sum());
        totals.day = String::new();
        let mut types: BTreeMap<String, u64> = BTreeMap::new();
        for (_, t) in self
            .derived
            .read()
            .unwrap()
            .types
            .range(from.clone()..=to.clone())
        {
            for (k, n) in t {
                *types.entry(k.clone()).or_default() += n;
            }
        }
        let mut event_types: Vec<TypeCount> = types
            .into_iter()
            .map(|(kind, count)| TypeCount { kind, count })
            .collect();
        event_types.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.kind.cmp(&b.kind)));
        let mut stored = Sum::default();
        for s in self.totals().values() {
            stored.add(&s.0);
        }
        let request_count_from = self
            .metered
            .lock()
            .unwrap()
            .book
            .days
            .range(keep_from((self.clock)())..=to.clone())
            .find(|(_, accounts)| {
                accounts
                    .get("")
                    .is_some_and(|metrics| metrics.contains_key(metric::REQUESTS))
            })
            .map(|(day, _)| day.clone());
        Overview {
            from,
            to,
            keep_days: KEEP_DAYS,
            request_count_from,
            days: rows,
            totals,
            accounts_active: active.len() as i64,
            event_types,
            stored: stored.by(&metric::STORED),
            saves_storage_measured: false,
        }
    }

    /// Every account with anything in the last `days` days or anything
    /// stored, with its numbers; unsorted, gamertags not filled in.
    fn accounts(&self, days: i64, saves: Option<&HashMap<String, i64>>) -> Vec<AccountUsage> {
        let (from, to) = self.range(days);
        let mut sums: HashMap<String, (Sum, i64, Option<String>)> = HashMap::new();
        for (day, accts) in self.per_day(&from, &to) {
            for (a, m) in accts {
                if a.is_empty() {
                    continue;
                }
                let e = sums.entry(a).or_default();
                e.0.add(&m.0);
                if m.any() {
                    e.1 += 1;
                    e.2 = Some(day.clone());
                }
            }
        }
        let stored = self.totals();
        for a in stored.keys().filter(|a| !a.is_empty()) {
            sums.entry(a.clone()).or_default();
        }
        for a in saves.into_iter().flat_map(HashMap::keys) {
            sums.entry(a.clone()).or_default();
        }
        sums.into_iter()
            .map(|(xuid, (s, active_days, last_active))| {
                let mut st = stored
                    .get(&xuid)
                    .map(|t| t.by(&metric::STORED))
                    .unwrap_or_else(|| Sum::default().by(&metric::STORED));
                if let Some(&bytes) = saves.and_then(|s| s.get(&xuid)) {
                    st.insert(short(metric::STORED_SAVES).to_string(), bytes);
                }
                let up = s.by(&metric::UPLOADS);
                let messages = s.by(&metric::MESSAGES);
                AccountUsage {
                    uploaded: up.values().sum(),
                    stored_total: st.values().sum(),
                    messages_total: messages.values().sum(),
                    xuid,
                    gamertag: None,
                    up,
                    stored: st,
                    events: s.get(metric::EVENTS),
                    messages,
                    games: s.get(metric::GAMES),
                    sessions_hosted: s.get(metric::SESSIONS_HOSTED),
                    sign_ins: s.get(metric::SIGN_INS),
                    reports_sent: s.get(metric::REPORTS_SENT),
                    reports_received: s.get(metric::REPORTS_RECEIVED),
                    active_days,
                    last_active,
                }
            })
            .collect()
    }
}

/// What the accounts list can be sorted by, largest first.
pub const SORTS: [&str; 10] = [
    "uploaded",
    "stored",
    "events",
    "messages",
    "games",
    "sessions_hosted",
    "sign_ins",
    "reports_sent",
    "reports_received",
    "active_days",
];

/// The top `limit` by `sort` (ties by xuid, so pages are stable).
pub fn top(mut items: Vec<AccountUsage>, sort: &str, limit: usize) -> Option<Vec<AccountUsage>> {
    let key: fn(&AccountUsage) -> i64 = match sort {
        "uploaded" => |a| a.uploaded,
        "stored" => |a| a.stored_total,
        "events" => |a| a.events,
        "messages" => |a| a.messages_total,
        "games" => |a| a.games,
        "sessions_hosted" => |a| a.sessions_hosted,
        "sign_ins" => |a| a.sign_ins,
        "reports_sent" => |a| a.reports_sent,
        "reports_received" => |a| a.reports_received,
        "active_days" => |a| a.active_days,
        _ => return None,
    };
    items.sort_by(|a, b| key(b).cmp(&key(a)).then_with(|| a.xuid.cmp(&b.xuid)));
    items.truncate(limit);
    Some(items)
}

impl Live {
    /// Counts an API response after rate limiting. Both the shared router
    /// and the Worker's direct WebSocket routes use the same counter.
    pub async fn count_api_request(&self, status: u16) {
        if status != StatusCode::TOO_MANY_REQUESTS.as_u16() {
            self.meter(None, metric::REQUESTS, 1).await;
        }
    }

    /// Counts metered usage ([module docs](self)) and writes what is due.
    pub async fn meter(&self, account: Option<Xuid>, metric: &str, n: u64) {
        self.usage
            .meter(account, metric, i64::try_from(n).unwrap_or(i64::MAX));
        self.flush_usage().await;
    }

    /// Writes the metered counters if a write is due.
    pub async fn flush_usage(&self) {
        let Some((rows, keep)) = self.usage.due() else {
            return;
        };
        if let Err(e) = self.store.add_usage(&rows, &keep).await {
            eprintln!("live: usage counters not written ({e}); kept for the next write");
            self.usage.unflushed(rows, &keep);
        }
    }

    pub fn usage(&self) -> &Usage {
        &self.usage
    }
}

fn days_param(p: &HashMap<String, String>) -> Result<i64, ApiError> {
    match p.get("days") {
        None => Ok(30),
        Some(d) => d
            .parse::<i64>()
            .ok()
            .filter(|d| (1..=MAX_RANGE_DAYS).contains(d))
            .ok_or_else(|| invalid(format!("days is 1 to {MAX_RANGE_DAYS}"))),
    }
}

/// `GET /v1/admin/usage?days=30`: admins (`admin.read`).
pub async fn get_overview(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Query(p): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    crate::invites::admin(
        &live,
        crate::http::bearer(&headers),
        crate::invites::LXM_ADMIN_READ,
    )?;
    let days = days_param(&p)?;
    let mut overview = live.usage.overview(days);
    if let Some(saves) = live.stored_save_bytes().await {
        overview.saves_storage_measured = true;
        overview
            .stored
            .insert(short(metric::STORED_SAVES).to_string(), saves.values().sum());
    }
    Ok(crate::http::json(StatusCode::OK, &overview))
}

#[derive(Serialize)]
struct AccountsPage {
    days: i64,
    sort: String,
    /// Accounts with anything in the range or anything stored.
    total: usize,
    items: Vec<AccountUsage>,
    saves_storage_measured: bool,
}

/// `GET /v1/admin/usage/accounts?days=30&sort=uploaded&limit=50`: admins.
pub async fn get_accounts(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Query(p): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    crate::invites::admin(
        &live,
        crate::http::bearer(&headers),
        crate::invites::LXM_ADMIN_READ,
    )?;
    let days = days_param(&p)?;
    let sort = p.get("sort").map_or("uploaded", String::as_str).to_string();
    let limit = match p.get("limit") {
        None => 50,
        Some(l) => l
            .parse::<usize>()
            .ok()
            .filter(|l| (1..=MAX_ACCOUNTS).contains(l))
            .ok_or_else(|| invalid(format!("limit is 1 to {MAX_ACCOUNTS}")))?,
    };
    let saves = live.stored_save_bytes().await;
    let all = live.usage.accounts(days, saves.as_deref());
    let total = all.len();
    let mut items = top(all, &sort, limit)
        .ok_or_else(|| invalid(format!("sort is one of {}", SORTS.join(", "))))?;
    {
        let s = live.model.read();
        for a in &mut items {
            if let Ok(x) = Xuid::parse(&a.xuid) {
                a.gamertag = s.accounts.get(&x).map(|acct| acct.gamertag.to_string());
            }
        }
    }
    Ok(crate::http::json(
        StatusCode::OK,
        &AccountsPage {
            days,
            sort,
            total,
            items,
            saves_storage_measured: saves.is_some(),
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicI64, Ordering};

    use live_proto::sign::{EventBody, KeyPair};
    use serde_json::json;

    const A: &str = "0009000000000001";
    const B: &str = "0009000000000002";
    // 2026-11-18T12:00:00Z
    const NOON: i64 = 1_795_003_200;

    fn clock(t: &Arc<AtomicI64>) -> Box<dyn Fn() -> i64 + Send + Sync> {
        let t = Arc::clone(t);
        Box::new(move || t.load(Ordering::SeqCst))
    }

    #[allow(clippy::too_many_arguments)]
    fn ev(
        key: &KeyPair,
        author: &str,
        stream: &str,
        kind: &str,
        data: Value,
        at: &str,
        jws_index: Option<usize>,
        jws: &str,
    ) -> RecordedEvent {
        let mut body = EventBody::new(
            key,
            author,
            stream,
            kind,
            data,
            time::OffsetDateTime::now_utc(),
        );
        body.at = at.to_string();
        RecordedEvent {
            position: 1,
            version: 0,
            hash: String::new(),
            jws: jws.into(),
            jws_index,
            body,
        }
    }

    fn acct(x: &str) -> String {
        format!("acct:{x}@test")
    }

    #[test]
    fn days_and_the_keep() {
        assert_eq!(day_of(NOON), "2026-11-18");
        assert_eq!(day_of(0), "1970-01-01");
        assert_eq!(keep_from(NOON), "2025-10-15");
        assert_eq!(event_day("2026-11-18T01:02:03Z"), Some("2026-11-18"));
        assert_eq!(event_day("nonsense"), None);
        assert_eq!(event_day("2026/11/18T"), None);
    }

    #[test]
    fn the_log_counts_events_uploads_messages_games_and_sign_ins() {
        let t = Arc::new(AtomicI64::new(NOON));
        let u = Usage::with_clock(clock(&t));
        let k = KeyPair::generate();
        let today = "2026-11-18T10:00:00Z";
        // A batch of three: its JWS counted once, on the first.
        let jws = "x".repeat(300);
        u.apply(&ev(
            &k,
            &acct(A),
            "game-g1",
            game::GAME_STARTED,
            json!({"host": acct(A), "players": [{"xuid": A}, {"name": "Guest"}]}),
            today,
            Some(0),
            &jws,
        ));
        u.apply(&ev(
            &k,
            &acct(A),
            "game-g1",
            game::PLAYER_JOINED,
            json!({"player": {"xuid": B}}),
            today,
            Some(1),
            &jws,
        ));
        u.apply(&ev(
            &k,
            &acct(A),
            "game-g1",
            game::PLAYER_JOINED,
            json!({"player": {"xuid": B}}),
            today,
            Some(2),
            &jws,
        ));
        u.apply(&ev(
            &k,
            &acct(A),
            "acct-1",
            types::FRIEND_REQUESTED,
            json!({}),
            today,
            None,
            &"y".repeat(100),
        ));
        u.apply(&ev(
            &k,
            &acct(B),
            "acct-2",
            types::SIGNED_IN,
            json!({}),
            today,
            None,
            &"z".repeat(50),
        ));
        u.apply(&ev(
            &k,
            "server:test",
            &mailbox::stream(Xuid::parse(B).unwrap()),
            mailbox::SENT,
            json!({"from": A, "session_id": "s"}),
            today,
            None,
            &"w".repeat(80),
        ));
        u.apply(&ev(
            &k,
            &acct(A),
            "session-0123456789abcdef",
            session::CREATED,
            json!({}),
            today,
            None,
            &"v".repeat(10),
        ));

        let o = u.overview(7);
        assert_eq!(o.days.len(), 1);
        let d = &o.days[0];
        assert_eq!(d.events, 7);
        // Account-signed bytes only: the server's invite is not an upload.
        assert_eq!(d.up["events"], 300 + 100 + 50 + 10);
        assert_eq!(d.messages["friend_requests"], 1);
        assert_eq!(d.messages["invites"], 1);
        assert_eq!(d.games, 2, "A hosted g1, B joined it (twice, counted once)");
        assert_eq!(d.sign_ins, 1);
        assert_eq!(d.sessions_hosted, 1);
        assert_eq!(d.active_accounts, 2);
        assert_eq!(o.accounts_active, 2);
        assert_eq!(
            o.event_types[0],
            TypeCount {
                kind: game::PLAYER_JOINED.into(),
                count: 2
            }
        );
        assert_eq!(o.stored["events"], 300 + 100 + 50 + 80 + 10);

        let all = top(u.accounts(7, None), "uploaded", 10).unwrap();
        assert_eq!(
            all.iter().map(|a| a.xuid.as_str()).collect::<Vec<_>>(),
            [A, B]
        );
        let a = &all[0];
        assert_eq!(
            (
                a.uploaded,
                a.events,
                a.messages_total,
                a.games,
                a.sessions_hosted
            ),
            (410, 5, 2, 1, 1)
        );
        assert_eq!(a.messages["invites"], 1, "the invite counts for its sender");
        assert_eq!(a.active_days, 1);
        assert_eq!(a.last_active.as_deref(), Some("2026-11-18"));
    }

    #[test]
    fn reports_count_for_both_sides_and_server_events_only_as_events() {
        let t = Arc::new(AtomicI64::new(NOON));
        let u = Usage::with_clock(clock(&t));
        let k = KeyPair::generate();
        let at = "2026-11-18T09:00:00Z";
        let target = Xuid::parse(B).unwrap();
        u.apply(&ev(
            &k,
            "server:test",
            &reports::stream(target),
            reports::PLAYER_REPORTED,
            json!({"target": B, "by": A, "kind": "gameplay"}),
            at,
            None,
            "r",
        ));
        u.apply(&ev(
            &k,
            "server:test",
            &reports::stream(target),
            reports::REPORT_REVIEWED,
            json!({}),
            at,
            None,
            "rr",
        ));
        u.apply(&ev(
            &k,
            "server:test",
            &live_proto::rank::stream(target),
            live_proto::rank::CHANGED,
            json!({"xuid": B}),
            at,
            None,
            "rc",
        ));
        let o = u.overview(1);
        assert_eq!(
            (o.days[0].events, o.days[0].reports, o.days[0].up["events"]),
            (3, 1, 0)
        );
        let acc = top(u.accounts(1, None), "reports_received", 5).unwrap();
        assert_eq!(
            (acc[0].xuid.as_str(), acc[0].reports_received, acc[0].events),
            (B, 1, 0)
        );
        assert_eq!(acc[0].active_days, 0, "being reported is not activity");
        let a = acc.iter().find(|a| a.xuid == A).unwrap();
        assert_eq!((a.reports_sent, a.active_days), (1, 1));
        assert_eq!(o.days[0].active_accounts, 1);
    }

    #[test]
    fn content_stored_counts_for_its_uploader_and_old_days_fall_out() {
        let t = Arc::new(AtomicI64::new(NOON));
        let u = Usage::with_clock(clock(&t));
        let k = KeyPair::generate();
        u.apply(&ev(
            &k,
            "server:test",
            "content",
            live_proto::maps::CONTENT_STORED,
            json!({"sha256": "00", "size": 4096, "uploader": A}),
            "2026-11-18T00:00:00Z",
            None,
            "j",
        ));
        // Older than the keep: counted as stored (the log still holds it), not by day.
        u.apply(&ev(
            &k,
            &acct(B),
            "acct-2",
            types::SIGNED_IN,
            json!({}),
            "2024-01-01T00:00:00Z",
            None,
            "jj",
        ));
        let o = u.overview(400);
        assert_eq!(o.stored["content"], 4096);
        assert_eq!(o.totals.sign_ins, 0);
        let acc = u.accounts(30, None);
        let a = acc.iter().find(|a| a.xuid == A).unwrap();
        assert_eq!((a.stored["content"], a.active_days), (4096, 0));
        assert!(acc.iter().any(|a| a.xuid == B), "B stores event bytes");
    }

    #[test]
    fn metering_writes_a_minute_at_a_time_and_cuts_on_a_new_day() {
        let t = Arc::new(AtomicI64::new(NOON));
        let u = Usage::with_clock(clock(&t));
        let a = Xuid::parse(A).unwrap();
        u.meter(Some(a), metric::UP_FRAMES, 100);
        u.meter(Some(a), metric::UP_FRAMES, 50);
        u.meter(Some(a), metric::STORED_SAVES, 9);
        assert!(u.due().is_none(), "not a minute yet");
        assert_eq!(
            u.overview(1).days[0].up["frames"],
            150,
            "counted at once in memory"
        );
        t.fetch_add(FLUSH_EVERY_S, Ordering::SeqCst);
        let (rows, keep) = u.due().expect("a minute on");
        assert!(keep.is_empty(), "no cutoff change within a UTC day");
        assert_eq!(
            rows,
            [
                UsageRow {
                    day: String::new(),
                    account: A.into(),
                    metric: metric::STORED_SAVES.into(),
                    n: 9
                },
                UsageRow {
                    day: "2026-11-18".into(),
                    account: A.into(),
                    metric: metric::UP_FRAMES.into(),
                    n: 150
                },
            ]
        );
        assert!(u.due().is_none(), "nothing pending");
        // A failed write comes back for the next.
        u.unflushed(rows, &keep);
        assert_eq!(u.pending(), 2);
        // A new day writes even with nothing pending, to cut old days.
        t.fetch_add(86_400, Ordering::SeqCst);
        let (_, keep) = u.due().expect("prune on the new day");
        assert_eq!(keep, "2025-10-16");
        assert!(u.due().is_none(), "no second write of the same cutoff");
        assert_eq!(u.overview(1).days.len(), 0, "today has nothing");
        assert_eq!(u.overview(2).days[0].up["frames"], 150);
    }

    #[test]
    fn worker_usage_flushes_at_its_idle_deadline_or_increment_cap() {
        let t = Arc::new(AtomicI64::new(NOON));
        let u = Usage::worker_with_clock(clock(&t));
        assert_eq!(u.flush_due(), None);

        u.meter(None, metric::REQUESTS, 1);
        assert_eq!(u.flush_due(), Some(NOON + WORKER_FLUSH_EVERY_S));
        t.fetch_add(WORKER_FLUSH_EVERY_S - 1, Ordering::SeqCst);
        assert!(u.due().is_none());
        t.fetch_add(1, Ordering::SeqCst);
        let (rows, keep) = u.due().expect("the idle alarm flushes the batch");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].n, 1);
        assert_eq!(u.flush_due(), None);

        u.unflushed(rows, &keep);
        assert_eq!(u.flush_due(), Some(NOON + 2 * WORKER_FLUSH_EVERY_S));
        assert!(u.due().is_none(), "a failed write waits for the next batch");
        for _ in 1..WORKER_PENDING_LIMIT {
            u.meter(None, metric::REQUESTS, 1);
        }
        assert!(u.due().is_none());
        u.meter(None, metric::REQUESTS, 1);
        let (rows, keep) = u.due().expect("a full batch flushes immediately");
        assert!(keep.is_empty());
        assert_eq!(rows.len(), 1, "increments to the same row are combined");
        assert_eq!(rows[0].n, WORKER_PENDING_LIMIT as i64 + 1);
        assert_eq!(u.flush_due(), None);
    }

    #[test]
    fn loading_keeps_the_kept_days_and_ignores_legacy_save_deltas() {
        let t = Arc::new(AtomicI64::new(NOON));
        let u = Usage::with_clock(clock(&t));
        let row = |day: &str, metric: &str, n| UsageRow {
            day: day.into(),
            account: A.into(),
            metric: metric.into(),
            n,
        };
        u.load(&[
            row("2026-11-17", metric::UP_CONTENT, 7),
            row("2020-01-01", metric::UP_CONTENT, 99),
            row("", metric::STORED_SAVES, 5),
        ]);
        let o = u.overview(400);
        assert_eq!(o.totals.up["content"], 7);
        assert_eq!(o.stored["saves"], 0);
        assert!(!o.saves_storage_measured);
        assert_eq!(u.pending(), 0, "loaded rows are not written again");
    }

    #[test]
    fn request_counts_keep_unmeasured_history_distinct() {
        let t = Arc::new(AtomicI64::new(NOON));
        let u = Usage::with_clock(clock(&t));
        let request_row = |day: String| UsageRow {
            day,
            account: String::new(),
            metric: metric::REQUESTS.into(),
            n: 10,
        };
        u.load(&[
            UsageRow {
                day: "2026-11-17".into(),
                account: A.into(),
                metric: metric::UP_CONTENT.into(),
                n: 50,
            },
            request_row(day_of(NOON - KEEP_DAYS * 86_400)),
        ]);
        let history = u.overview(7);
        assert_eq!(history.request_count_from, None);
        assert_eq!(history.totals.requests, None);

        u.meter(None, metric::REQUESTS, 5);
        let usage = u.overview(7);
        assert_eq!(usage.request_count_from.as_deref(), Some("2026-11-18"));
        assert_eq!(usage.days[0].requests, None);
        assert_eq!(usage.days[1].requests, Some(5));
        assert_eq!(usage.totals.requests, Some(5));
        assert_eq!(usage.accounts_active, 1);

        let first = keep_from(NOON);
        u.load(&[request_row(first.clone())]);
        let narrow = u.overview(7);
        assert_eq!(narrow.request_count_from, Some(first));
        assert_eq!(narrow.totals.requests, Some(5));
        assert_eq!(u.overview(KEEP_DAYS).totals.requests, Some(15));
    }

    #[test]
    fn top_sorts_largest_first_and_refuses_unknown_sorts() {
        let mk = |x: &str, up, ev| AccountUsage {
            xuid: x.into(),
            uploaded: up,
            events: ev,
            ..Default::default()
        };
        let items = vec![mk("b", 5, 1), mk("a", 5, 9), mk("c", 9, 0)];
        let up = top(items.clone(), "uploaded", 2).unwrap();
        assert_eq!(
            up.iter().map(|a| a.xuid.as_str()).collect::<Vec<_>>(),
            ["c", "a"]
        );
        let ev = top(items.clone(), "events", 3).unwrap();
        assert_eq!(ev[0].xuid, "a");
        assert!(top(items, "nope", 3).is_none());
        for s in SORTS {
            assert!(top(Vec::new(), s, 1).is_some(), "{s}");
        }
    }
}
