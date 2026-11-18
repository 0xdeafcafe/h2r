//! Server-wide stats for the stats site's home page
//! (`GET /v1/stats/summary`, docs/live/live-api.md "Stats summary").
//!
//! A [`Subscriber`] folds the game streams into time buckets as events are
//! applied: one bucket per UTC day and one small totals bucket per UTC hour,
//! for the last [`KEEP_DAYS`] days, plus a running all-time bucket. A summary
//! merges the buckets its window covers (at most [`KEEP_DAYS`] of them), so a
//! request never reads the log; answers are cached until the log moves or the
//! hour turns.
//!
//! What counts where follows the web app's rules: kills, deaths, betrayals,
//! wins and sprees are multiplayer's; campaign games count their own games,
//! deaths and Legendary completions. Per-player rows are for players with an
//! account (an `xuid`); totals count everyone.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::Xuid;
use live_proto::game::{self, GameEnded, GameStarted, Winner};
use live_proto::sign::{format_time, parse_time};
use serde::Serialize;
use serde_json::{Value, json};

use crate::app::{ApiError, Live, invalid};
use crate::dispatch::Subscriber;
use crate::store::RecordedEvent;

const HOUR: i64 = 3600;
const DAY: i64 = 86_400;
/// How far back the day and hour buckets reach; older events only count
/// all-time.
pub const KEEP_DAYS: i64 = 30;
/// The windows a summary can cover, in days (`all` is every event).
pub const WINDOWS: [(&str, Option<i64>); 4] = [
    ("today", Some(1)),
    ("7d", Some(7)),
    ("30d", Some(30)),
    ("all", None),
];
/// Most per-player rows a summary carries (by kills, then games).
pub const MAX_PLAYERS: usize = 500;
/// A game longer than this counts this long toward time played.
const MAX_GAME_S: i64 = 12 * HOUR;

/// Counts over a span of time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct Totals {
    pub games: u64,
    pub multiplayer: u64,
    pub campaign: u64,
    pub matchmaking: u64,
    pub custom: u64,
    /// Campaign games with more than one player, or played online.
    pub coop: u64,
    /// Multiplayer kills (kind `kill`).
    pub kills: u64,
    /// Multiplayer deaths of any kind.
    pub deaths: u64,
    pub betrayals: u64,
    pub suicides: u64,
    pub campaign_deaths: u64,
    /// Time in finished games summed over their players, in seconds.
    pub player_seconds: u64,
    /// Campaign levels finished on Legendary (one per game).
    pub legendary: u64,
}

impl Totals {
    fn add(&mut self, o: &Totals) {
        self.games += o.games;
        self.multiplayer += o.multiplayer;
        self.campaign += o.campaign;
        self.matchmaking += o.matchmaking;
        self.custom += o.custom;
        self.coop += o.coop;
        self.kills += o.kills;
        self.deaths += o.deaths;
        self.betrayals += o.betrayals;
        self.suicides += o.suicides;
        self.campaign_deaths += o.campaign_deaths;
        self.player_seconds += o.player_seconds;
        self.legendary += o.legendary;
    }
}

#[derive(Debug, Clone, Default)]
struct PlayerAgg {
    name: String,
    /// The `at` the name was seen; the newest name wins a merge.
    named_at: i64,
    games: u64,
    multiplayer: u64,
    campaign: u64,
    wins: u64,
    kills: u64,
    deaths: u64,
    betrayals: u64,
    best_spree: u64,
    seconds: u64,
    /// Campaign levels finished on Legendary.
    legendary: BTreeSet<String>,
}

impl PlayerAgg {
    fn add(&mut self, o: &PlayerAgg) {
        if o.named_at >= self.named_at {
            self.name.clone_from(&o.name);
            self.named_at = o.named_at;
        }
        self.games += o.games;
        self.multiplayer += o.multiplayer;
        self.campaign += o.campaign;
        self.wins += o.wins;
        self.kills += o.kills;
        self.deaths += o.deaths;
        self.betrayals += o.betrayals;
        self.best_spree = self.best_spree.max(o.best_spree);
        self.seconds += o.seconds;
        self.legendary.extend(o.legendary.iter().cloned());
    }
}

#[derive(Debug, Clone, Default)]
struct MapAgg {
    map: String,
    map_id: Option<String>,
    campaign: bool,
    games: u64,
    kills: u64,
}

/// Everything about one day (or all time).
#[derive(Debug, Clone, Default)]
struct Bucket {
    totals: Totals,
    players: HashMap<Xuid, PlayerAgg>,
    /// By `map_id`, else the base map, and campaign or not.
    maps: HashMap<(String, bool), MapAgg>,
    weapons: HashMap<String, u64>,
    playlists: HashMap<String, u64>,
    skulls: BTreeMap<u8, u64>,
}

impl Bucket {
    fn add(&mut self, o: &Bucket) {
        self.totals.add(&o.totals);
        for (x, p) in &o.players {
            self.players.entry(*x).or_default().add(p);
        }
        for (k, m) in &o.maps {
            let e = self.maps.entry(k.clone()).or_insert_with(|| MapAgg {
                map: m.map.clone(),
                map_id: m.map_id.clone(),
                campaign: m.campaign,
                ..Default::default()
            });
            e.games += m.games;
            e.kills += m.kills;
        }
        for (w, n) in &o.weapons {
            *self.weapons.entry(w.clone()).or_default() += n;
        }
        for (p, n) in &o.playlists {
            *self.playlists.entry(p.clone()).or_default() += n;
        }
        for (s, n) in &o.skulls {
            *self.skulls.entry(*s).or_default() += n;
        }
    }

    fn player(&mut self, x: Xuid, name: &str, at: i64) -> &mut PlayerAgg {
        let p = self.players.entry(x).or_default();
        if at >= p.named_at {
            p.name = name.to_string();
            p.named_at = at;
        }
        p
    }

    fn map(&mut self, g: &LiveGame) -> &mut MapAgg {
        let key = (
            g.map_id.clone().unwrap_or_else(|| g.map.clone()),
            g.campaign,
        );
        self.maps.entry(key).or_insert_with(|| MapAgg {
            map: g.map.clone(),
            map_id: g.map_id.clone(),
            campaign: g.campaign,
            ..Default::default()
        })
    }
}

/// A game still being played: what its later events need.
#[derive(Debug, Clone)]
struct LiveGame {
    started: i64,
    campaign: bool,
    map: String,
    map_id: Option<String>,
    difficulty: Option<String>,
    /// Slot -> account and name; kept after a player leaves so late kills
    /// still find them (a slot taken again is overwritten).
    slots: HashMap<u32, (Option<Xuid>, String, i32)>,
    /// Accounts counted as having played it.
    xuids: BTreeSet<Xuid>,
    /// Current scoring run per slot.
    spree: HashMap<u32, u64>,
}

#[derive(Default)]
struct Inner {
    days: BTreeMap<i64, Bucket>,
    hours: BTreeMap<i64, Totals>,
    all: Bucket,
    games: HashMap<String, LiveGame>,
    newest_day: i64,
    position: u64,
}

impl Inner {
    /// Applies `f` to the event's day bucket (when it is recent enough) and
    /// to all-time.
    fn bump(&mut self, at: i64, f: impl Fn(&mut Bucket)) {
        let day = at.div_euclid(DAY);
        if day > self.newest_day - KEEP_DAYS {
            f(self.days.entry(day).or_default());
        }
        f(&mut self.all);
    }

    fn hour(&mut self, at: i64, f: impl Fn(&mut Totals)) {
        let day = at.div_euclid(DAY);
        if day > self.newest_day - KEEP_DAYS {
            f(self.hours.entry(at.div_euclid(HOUR)).or_default());
        }
    }

    fn prune(&mut self, at: i64) {
        let day = at.div_euclid(DAY);
        if day <= self.newest_day {
            return;
        }
        self.newest_day = day;
        let first = day - KEEP_DAYS + 1;
        self.days = self.days.split_off(&first);
        self.hours = self.hours.split_off(&(first * 24));
    }

    fn apply(&mut self, e: &RecordedEvent) {
        self.position = self.position.max(e.position);
        let b = &e.body;
        let Some(id) = b.stream.strip_prefix("game-") else {
            return;
        };
        let Some(at) = parse_time(&b.at).map(|t| t.unix_timestamp()) else {
            return;
        };
        self.prune(at);
        match b.kind.as_str() {
            game::GAME_STARTED => self.started(e, at),
            game::PLAYER_JOINED => {
                let Some(d) = data::<game::PlayerJoined>(e) else {
                    return;
                };
                let Some(g) = self.games.get_mut(id) else {
                    return;
                };
                let p = d.player;
                g.slots.insert(p.index, (p.xuid, p.name.clone(), p.team));
                g.spree.remove(&p.index);
                if let Some(x) = p.xuid.filter(|x| g.xuids.insert(*x)) {
                    let campaign = g.campaign;
                    self.bump(at, |b| {
                        let a = b.player(x, &p.name, at);
                        a.games += 1;
                        if campaign {
                            a.campaign += 1;
                        } else {
                            a.multiplayer += 1;
                        }
                    });
                }
            }
            game::PLAYER_KILLED => {
                let Some(k) = data::<game::PlayerKilled>(e) else {
                    return;
                };
                self.killed(id, &k, at);
            }
            game::GAME_ENDED => {
                let Some(d) = data::<GameEnded>(e) else {
                    return;
                };
                self.ended(id, &d, at);
            }
            _ => {}
        }
    }

    fn started(&mut self, e: &RecordedEvent, at: i64) {
        let Some(s) = data::<GameStarted>(e) else {
            return;
        };
        let ng = &s.game;
        let campaign = ng.mode == "campaign";
        let kind = ng.kind().to_string();
        let g = LiveGame {
            started: at,
            campaign,
            map: ng.map.clone(),
            map_id: ng.map_id.map(|m| m.to_string()),
            difficulty: ng.difficulty.as_ref().map(|d| d.to_ascii_lowercase()),
            slots: ng
                .players
                .iter()
                .map(|p| (p.index, (p.xuid, p.name.clone(), p.team)))
                .collect(),
            xuids: ng.players.iter().filter_map(|p| p.xuid).collect(),
            spree: HashMap::new(),
        };
        let coop = campaign && (ng.players.len() > 1 || ng.online == Some(true));
        let players: Vec<(Xuid, String)> = ng
            .players
            .iter()
            .filter_map(|p| Some((p.xuid?, p.name.clone())))
            .collect();
        let playlist = (kind == "matchmaking")
            .then(|| ng.playlist.clone())
            .flatten();
        let skulls = ng.skulls.clone().unwrap_or_default();
        let mut delta = Totals {
            games: 1,
            coop: coop as u64,
            ..Default::default()
        };
        if campaign {
            delta.campaign = 1;
        } else {
            delta.multiplayer = 1;
        }
        match kind.as_str() {
            "matchmaking" => delta.matchmaking = 1,
            "custom" => delta.custom = 1,
            _ => {}
        }
        self.bump(at, |b| {
            b.totals.add(&delta);
            b.map(&g).games += 1;
            if let Some(p) = &playlist {
                *b.playlists.entry(p.clone()).or_default() += 1;
            }
            for s in &skulls {
                *b.skulls.entry(*s).or_default() += 1;
            }
            for (x, name) in &players {
                let a = b.player(*x, name, at);
                a.games += 1;
                if campaign {
                    a.campaign += 1;
                } else {
                    a.multiplayer += 1;
                }
            }
        });
        self.hour(at, |h| h.add(&delta));
        // A GameStarted for a game already known (it should not happen) starts it over.
        self.games.insert(s.game_id.clone(), g);
    }

    fn killed(&mut self, id: &str, k: &game::PlayerKilled, at: i64) {
        let Some(g) = self.games.get_mut(id) else {
            return;
        };
        let victim = g.slots.get(&k.victim).and_then(|s| s.0);
        if g.campaign {
            self.bump(at, |b| b.totals.campaign_deaths += 1);
            self.hour(at, |h| h.campaign_deaths += 1);
            return;
        }
        g.spree.insert(k.victim, 0);
        let killer = k.killer.filter(|&s| s != k.victim);
        let kind = k.kind.as_str();
        let scoring = kind == "kill" && killer.is_some();
        let mut spree = 0;
        if let Some(s) = killer.filter(|_| scoring) {
            let n = g.spree.entry(s).or_default();
            *n += 1;
            spree = *n;
        }
        let killer_acct = killer.and_then(|s| g.slots.get(&s).cloned());
        let g = g.clone();
        let weapon = k.weapon.clone().filter(|w| !w.is_empty());
        let add = |t: &mut Totals| {
            t.deaths += 1;
            match kind {
                "kill" if scoring => t.kills += 1,
                "betrayal" => t.betrayals += 1,
                "suicide" => t.suicides += 1,
                _ => {}
            }
        };
        self.bump(at, |b| {
            add(&mut b.totals);
            if let Some(x) = victim {
                let name = g.slots[&k.victim].1.clone();
                b.player(x, &name, at).deaths += 1;
            }
            if scoring {
                b.map(&g).kills += 1;
                if let Some(w) = &weapon {
                    *b.weapons.entry(w.clone()).or_default() += 1;
                }
            }
            if let Some((Some(x), name, _)) = &killer_acct {
                let a = b.player(*x, name, at);
                if scoring {
                    a.kills += 1;
                    a.best_spree = a.best_spree.max(spree);
                } else if kind == "betrayal" {
                    a.betrayals += 1;
                }
            }
        });
        self.hour(at, add);
    }

    fn ended(&mut self, id: &str, d: &GameEnded, at: i64) {
        let Some(g) = self.games.remove(id) else {
            return;
        };
        let secs = (at - g.started).clamp(0, MAX_GAME_S) as u64;
        let winners: BTreeSet<Xuid> = match (&d.winner, g.campaign) {
            (Some(Winner::Team { team }), false) => g
                .slots
                .values()
                .filter(|s| s.2 == *team)
                .filter_map(|s| s.0)
                .collect(),
            (Some(Winner::Player { player }), false) => {
                g.slots.get(player).and_then(|s| s.0).into_iter().collect()
            }
            _ => BTreeSet::new(),
        };
        let legendary =
            g.campaign && d.reason == "finished" && g.difficulty.as_deref() == Some("legendary");
        let level = g.map_id.clone().unwrap_or_else(|| g.map.clone());
        let names: HashMap<Xuid, String> = g
            .slots
            .values()
            .filter_map(|s| Some((s.0?, s.1.clone())))
            .collect();
        let people = g.slots.len().max(g.xuids.len()) as u64;
        self.bump(at, |b| {
            b.totals.player_seconds += secs * people;
            if legendary {
                b.totals.legendary += 1;
            }
            for x in &g.xuids {
                let name = names.get(x).map_or("", String::as_str);
                let a = b.player(*x, name, at);
                a.seconds += secs;
                if winners.contains(x) {
                    a.wins += 1;
                }
                if legendary {
                    a.legendary.insert(level.clone());
                }
            }
        });
        self.hour(at, |h| {
            h.player_seconds += secs * people;
            if legendary {
                h.legendary += 1;
            }
        });
    }

    fn summary(&self, days: Option<i64>, now: i64) -> Summary {
        let today = now.div_euclid(DAY);
        let (merged, from) = match days {
            None => (self.all.clone(), None),
            Some(n) => {
                let first = today - n + 1;
                let mut m = Bucket::default();
                for (_, b) in self.days.range(first..=today) {
                    m.add(b);
                }
                (m, Some(first * DAY))
            }
        };
        let now_h = now.div_euclid(HOUR);
        let mut last_24h = Totals::default();
        for (_, h) in self.hours.range(now_h - 23..=now_h) {
            last_24h.add(h);
        }
        let first_h = now_h - 24 * days.unwrap_or(KEEP_DAYS).min(KEEP_DAYS) + 1;
        let activity = self
            .hours
            .range(first_h..=now_h)
            .filter(|(_, h)| h.games > 0 || h.kills > 0)
            .map(|(k, h)| Activity {
                hour: format_unix(k * HOUR),
                games: h.games,
                kills: h.kills,
            })
            .collect();

        let mut players: Vec<PlayerRow> = merged
            .players
            .iter()
            .map(|(x, p)| PlayerRow {
                xuid: x.to_string(),
                name: p.name.clone(),
                games: p.games,
                multiplayer: p.multiplayer,
                campaign: p.campaign,
                wins: p.wins,
                kills: p.kills,
                deaths: p.deaths,
                betrayals: p.betrayals,
                best_spree: p.best_spree,
                seconds: p.seconds,
                legendary: p.legendary.len() as u64,
            })
            .collect();
        players.sort_by(|a, b| (b.kills, b.games, &a.xuid).cmp(&(a.kills, a.games, &b.xuid)));
        players.truncate(MAX_PLAYERS);
        let mut maps: Vec<MapRow> = merged
            .maps
            .values()
            .map(|m| MapRow {
                map: m.map.clone(),
                map_id: m.map_id.clone(),
                campaign: m.campaign,
                games: m.games,
                kills: m.kills,
            })
            .collect();
        // The map id breaks ties between variants of one map, so every
        // server (and every restart) orders them the same.
        maps.sort_by(|a, b| {
            (b.games, b.kills, &a.map, &a.map_id).cmp(&(a.games, a.kills, &b.map, &b.map_id))
        });
        Summary {
            from: from.map(format_unix),
            to: format_unix(now),
            position: self.position,
            totals: merged.totals,
            last_24h,
            all_time: self.all.totals.clone(),
            players,
            maps,
            weapons: ranked(&merged.weapons)
                .into_iter()
                .map(|(weapon, kills)| WeaponRow { weapon, kills })
                .collect(),
            playlists: ranked(&merged.playlists)
                .into_iter()
                .map(|(playlist, games)| PlaylistRow { playlist, games })
                .collect(),
            skulls: merged
                .skulls
                .iter()
                .map(|(s, n)| SkullRow {
                    skull: *s,
                    games: *n,
                })
                .collect(),
            activity,
        }
    }
}

fn ranked(m: &HashMap<String, u64>) -> Vec<(String, u64)> {
    let mut v: Vec<(String, u64)> = m.iter().map(|(k, n)| (k.clone(), *n)).collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    v
}

fn format_unix(t: i64) -> String {
    format_time(
        time::OffsetDateTime::from_unix_timestamp(t).unwrap_or(time::OffsetDateTime::UNIX_EPOCH),
    )
}

fn data<T: serde::de::DeserializeOwned>(e: &RecordedEvent) -> Option<T> {
    serde_json::from_value(e.body.data.clone()).ok()
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlayerRow {
    pub xuid: String,
    pub name: String,
    pub games: u64,
    pub multiplayer: u64,
    pub campaign: u64,
    pub wins: u64,
    pub kills: u64,
    pub deaths: u64,
    pub betrayals: u64,
    pub best_spree: u64,
    pub seconds: u64,
    /// Distinct campaign levels finished on Legendary.
    pub legendary: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct MapRow {
    pub map: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub map_id: Option<String>,
    pub campaign: bool,
    pub games: u64,
    pub kills: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct WeaponRow {
    pub weapon: String,
    pub kills: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct PlaylistRow {
    pub playlist: String,
    pub games: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SkullRow {
    pub skull: u8,
    pub games: u64,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Activity {
    /// The hour's start.
    pub hour: String,
    pub games: u64,
    pub kills: u64,
}

/// The answer to `GET /v1/stats/summary`.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Summary {
    /// The window's first instant (a UTC midnight), absent for `all`.
    pub from: Option<String>,
    pub to: String,
    pub position: u64,
    pub totals: Totals,
    pub last_24h: Totals,
    pub all_time: Totals,
    pub players: Vec<PlayerRow>,
    pub maps: Vec<MapRow>,
    pub weapons: Vec<WeaponRow>,
    pub playlists: Vec<PlaylistRow>,
    pub skulls: Vec<SkullRow>,
    /// Hours in the window (at most [`KEEP_DAYS`] back) with any games or
    /// kills: games started and multiplayer kills.
    pub activity: Vec<Activity>,
}

/// A cached answer: the log position and hour it was made at, and it.
type Cached = (u64, i64, Arc<Value>);

/// The stats projection and its answer cache. It folds every game twice:
/// all of them (what admins read), and only the public ones for everyone
/// else. A Live co-op game linked to a session (its visibility can change
/// while it runs) is held out of the public fold until it ends, then folded
/// in whole if it ended public ([`crate::model::Game::shows_to`] for an
/// anonymous reader) and dropped if not.
#[derive(Default)]
pub struct Stats {
    inner: RwLock<Inner>,
    public: RwLock<Inner>,
    /// The read model, which the dispatcher updates before this projection:
    /// which games are hidden. None (tests): every game is public.
    model: Option<Arc<crate::model::Model>>,
    /// The public fold's held events of running co-op games, by game id.
    held: Mutex<HashMap<String, Vec<RecordedEvent>>>,
    /// (Window, all games) -> (position, hour, answer).
    cache: Mutex<HashMap<(&'static str, bool), Cached>>,
}

impl Stats {
    /// Stats that leave hidden games out of the public summary.
    pub fn with_model(model: Arc<crate::model::Model>) -> Self {
        Stats {
            model: Some(model),
            ..Default::default()
        }
    }

    /// The summary for a window named in [`WINDOWS`], as JSON, at `now`:
    /// of every game when `all` (admins), else of the public ones.
    pub fn summary(&self, window: &str, now: i64, all: bool) -> Option<Arc<Value>> {
        let (name, days) = WINDOWS.into_iter().find(|(w, _)| *w == window)?;
        let hour = now.div_euclid(HOUR);
        let inner = if all { &self.inner } else { &self.public }.read().unwrap();
        let position = inner.position;
        if let Some((p, h, v)) = self.cache.lock().unwrap().get(&(name, all))
            && *p == position
            && *h == hour
        {
            return Some(v.clone());
        }
        let mut v = serde_json::to_value(inner.summary(days, now)).unwrap_or(Value::Null);
        drop(inner);
        v["window"] = json!(name);
        let v = Arc::new(v);
        self.cache
            .lock()
            .unwrap()
            .insert((name, all), (position, hour, v.clone()));
        Some(v)
    }

    /// The typed summary, for tests.
    pub fn summary_at(&self, days: Option<i64>, now: i64) -> Summary {
        self.inner.read().unwrap().summary(days, now)
    }

    /// The public fold's share of an event: none yet (held), or the
    /// events to fold now.
    fn public_share(&self, event: &RecordedEvent) -> Vec<RecordedEvent> {
        let b = &event.body;
        let (Some(m), Some(id)) = (&self.model, b.stream.strip_prefix("game-")) else {
            return vec![event.clone()];
        };
        if !matches!(
            b.kind.as_str(),
            game::GAME_STARTED | game::PLAYER_JOINED | game::PLAYER_KILLED | game::GAME_ENDED
        ) {
            return Vec::new();
        }
        let (linked, public) = {
            let s = m.read();
            let linked = s.games.get(id).is_some_and(|g| g.coop.is_some());
            let public = crate::sessions::game_shows_to(&s, id, crate::sessions::Viewer::Anonymous);
            (linked, public)
        };
        let mut held = self.held.lock().unwrap();
        if !linked && !held.contains_key(id) {
            return vec![event.clone()];
        }
        let list = held.entry(id.to_string()).or_default();
        list.push(event.clone());
        if b.kind != game::GAME_ENDED {
            return Vec::new();
        }
        let list = held.remove(id).unwrap_or_default();
        if public { list } else { Vec::new() }
    }
}

impl Subscriber for Stats {
    fn name(&self) -> &str {
        "stats"
    }

    fn apply(&self, event: &RecordedEvent) {
        self.inner.write().unwrap().apply(event);
        let share = self.public_share(event);
        let mut public = self.public.write().unwrap();
        for e in &share {
            public.apply(e);
        }
        public.position = public.position.max(event.position);
    }
}

/// `GET /v1/stats/summary?window=today|7d|30d|all` (default `7d`).
pub async fn get_summary(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    Query(p): Query<HashMap<String, String>>,
) -> Result<Response, ApiError> {
    let viewer = live.read_viewer(crate::http::bearer(&headers))?;
    let window = p.get("window").map_or("7d", String::as_str);
    let all = viewer == crate::sessions::Viewer::Admin;
    let v = live
        .stats
        .summary(window, crate::app::now(), all)
        .ok_or_else(|| invalid("window is today, 7d, 30d or all"))?;
    Ok(crate::http::json(StatusCode::OK, &*v))
}

#[cfg(test)]
mod tests {
    use live_proto::sign::{EventBody, KeyPair};

    use super::*;

    const A: &str = "0009000000000001";
    const B: &str = "0009000000000002";
    const C: &str = "0009000000000003";
    /// A Monday, 12:00 UTC.
    const T0: i64 = 1_763_380_800;

    struct Log {
        key: KeyPair,
        position: u64,
        stats: Stats,
    }

    impl Log {
        fn new() -> Self {
            Log {
                key: KeyPair::generate(),
                position: 0,
                stats: Stats::default(),
            }
        }
        fn push(&mut self, game: &str, kind: &str, data: Value, at: i64) {
            self.position += 1;
            let body = EventBody::at_unix(
                &self.key,
                "server:h",
                &format!("game-{game}"),
                kind,
                data,
                at,
            );
            self.stats.apply(&RecordedEvent {
                position: self.position,
                version: 0,
                hash: String::new(),
                jws: "".into(),
                jws_index: None,
                body,
            });
        }
        fn start(&mut self, game: &str, extra: Value, at: i64) {
            let mut d = json!({"game_id": game, "host": "server:h", "map": "lockout",
            "mode": "multiplayer", "team_play": true,
            "players": [
                {"index": 0, "name": "Ace", "xuid": A, "team": 0},
                {"index": 1, "name": "Bee", "xuid": B, "team": 1},
                {"index": 2, "name": "Guest", "team": 1},
            ]});
            for (k, v) in extra.as_object().unwrap() {
                d[k] = v.clone();
            }
            self.push(game, "GameStarted", d, at);
        }
        fn kill(
            &mut self,
            game: &str,
            victim: u32,
            killer: Option<u32>,
            kind: &str,
            weapon: &str,
            at: i64,
        ) {
            let mut d = json!({"tick": 1, "victim": victim, "kind": kind, "weapon": weapon});
            if let Some(k) = killer {
                d["killer"] = json!(k);
            }
            self.push(game, "PlayerKilled", d, at);
        }
        fn end(&mut self, game: &str, reason: &str, winner: Value, at: i64) {
            self.push(
                game,
                "GameEnded",
                json!({"tick": 9, "reason": reason, "winner": winner}),
                at,
            );
        }
    }

    fn row<'a>(s: &'a Summary, xuid: &str) -> &'a PlayerRow {
        s.players.iter().find(|p| p.xuid == xuid).unwrap()
    }

    #[test]
    fn a_multiplayer_game_counts_kills_deaths_sprees_wins_and_time() {
        let mut log = Log::new();
        log.start(
            "g1",
            json!({"kind": "matchmaking", "playlist": "Team Slayer", "skulls": [4]}),
            T0,
        );
        log.kill("g1", 1, Some(0), "kill", "battle_rifle", T0 + 10);
        log.kill("g1", 2, Some(0), "kill", "battle_rifle", T0 + 20);
        log.kill("g1", 1, Some(0), "kill", "smg", T0 + 30);
        log.kill("g1", 0, Some(1), "kill", "smg", T0 + 40);
        log.kill("g1", 0, Some(0), "suicide", "frag_grenade", T0 + 50);
        log.kill("g1", 1, Some(2), "betrayal", "smg", T0 + 55);
        log.kill("g1", 1, Some(0), "kill", "sniper_rifle", T0 + 60);
        log.end("g1", "finished", json!({"team": 0}), T0 + 600);

        let s = log.stats.summary_at(Some(7), T0 + 700);
        let t = &s.totals;
        assert_eq!(
            (t.games, t.multiplayer, t.matchmaking, t.campaign),
            (1, 1, 1, 0)
        );
        assert_eq!((t.kills, t.deaths, t.suicides, t.betrayals), (5, 7, 1, 1));
        // Three people for ten minutes.
        assert_eq!(t.player_seconds, 1800);
        let a = row(&s, A);
        assert_eq!(
            (a.games, a.kills, a.deaths, a.wins, a.best_spree, a.seconds),
            (1, 4, 2, 1, 3, 600)
        );
        let b = row(&s, B);
        assert_eq!((b.kills, b.deaths, b.wins, b.best_spree), (1, 4, 0, 1));
        // The guest has no account, so no row; their betrayal still counts.
        assert_eq!(s.players.len(), 2);
        assert_eq!(
            s.weapons[0],
            WeaponRow {
                weapon: "battle_rifle".into(),
                kills: 2
            }
        );
        assert_eq!(s.weapons.iter().map(|w| w.kills).sum::<u64>(), 5);
        assert_eq!(
            s.maps,
            vec![MapRow {
                map: "lockout".into(),
                map_id: None,
                campaign: false,
                games: 1,
                kills: 5
            }]
        );
        assert_eq!(
            s.playlists,
            vec![PlaylistRow {
                playlist: "Team Slayer".into(),
                games: 1
            }]
        );
        assert_eq!(s.skulls, vec![SkullRow { skull: 4, games: 1 }]);
        assert_eq!(s.activity.len(), 1);
        assert_eq!((s.activity[0].games, s.activity[0].kills), (1, 5));
        assert_eq!(s.last_24h, s.totals);
        assert_eq!(s.all_time, s.totals);
    }

    #[test]
    fn campaign_counts_apart_and_legendary_levels_are_distinct() {
        let mut log = Log::new();
        for (i, (map, diff, reason)) in [
            ("outskirts", "legendary", "finished"),
            ("outskirts", "Legendary", "finished"),
            ("metropolis", "legendary", "quit"),
            ("cairo", "heroic", "finished"),
        ]
        .into_iter()
        .enumerate()
        {
            let id = format!("c{i}");
            let at = T0 + i as i64 * 1000;
            log.start(&id, json!({"mode": "campaign", "map": map, "difficulty": diff, "team_play": false,
                "players": [{"index": 0, "name": "Ace", "xuid": A, "team": 0}, {"index": 1, "name": "Cee", "xuid": C, "team": 0}]}), at);
            log.kill(&id, 0, None, "world", "", at + 5);
            log.end(&id, reason, Value::Null, at + 100);
        }
        let s = log.stats.summary_at(None, T0 + 10_000);
        let t = &s.totals;
        assert_eq!((t.games, t.campaign, t.coop, t.multiplayer), (4, 4, 4, 0));
        assert_eq!(
            (t.kills, t.deaths, t.campaign_deaths, t.legendary),
            (0, 0, 4, 2)
        );
        let a = row(&s, A);
        assert_eq!(
            (a.campaign, a.multiplayer, a.legendary, a.deaths),
            (4, 0, 1, 0)
        );
        assert!(s.maps.iter().all(|m| m.campaign));
        assert!(s.weapons.is_empty());
    }

    #[test]
    fn windows_cover_whole_utc_days_and_old_days_only_count_all_time() {
        let mut log = Log::new();
        // 40, 8 and 2 days ago, then today.
        for (i, ago) in [40, 8, 2, 0].into_iter().enumerate() {
            let id = format!("g{i}");
            let at = T0 - ago * DAY;
            log.start(&id, json!({}), at);
            log.kill(&id, 1, Some(0), "kill", "smg", at + 1);
            log.end(&id, "finished", json!({"team": 0}), at + 60);
        }
        let games = |d: Option<i64>| log.stats.summary_at(d, T0 + 60).totals.games;
        assert_eq!(
            (games(Some(1)), games(Some(7)), games(Some(30)), games(None)),
            (1, 2, 3, 4)
        );
        let week = log.stats.summary_at(Some(7), T0 + 60);
        assert_eq!(
            parse_time(week.from.as_deref().unwrap())
                .unwrap()
                .unix_timestamp(),
            (T0 / DAY - 6) * DAY
        );
        assert_eq!(row(&week, A).kills, 2);
        assert_eq!(week.activity.len(), 2);
        assert_eq!(week.last_24h.games, 1);
        // The 40-day-old game made no day bucket.
        let inner = log.stats.inner.read().unwrap();
        assert!(inner.days.len() <= KEEP_DAYS as usize);
        assert!(inner.days.keys().all(|d| *d > T0 / DAY - KEEP_DAYS));
    }

    #[test]
    fn late_joiners_count_once_and_free_for_all_winners_are_players() {
        let mut log = Log::new();
        log.start("f", json!({"team_play": false}), T0);
        log.push(
            "f",
            "PlayerJoined",
            json!({"player": {"index": 3, "name": "Cee", "xuid": C, "team": 3}}),
            T0 + 5,
        );
        log.push("f", "PlayerLeft", json!({"index": 3}), T0 + 6);
        log.push(
            "f",
            "PlayerJoined",
            json!({"player": {"index": 3, "name": "Cee2", "xuid": C, "team": 3}}),
            T0 + 7,
        );
        log.kill("f", 0, Some(3), "kill", "smg", T0 + 8);
        log.end("f", "finished", json!({"player": 3}), T0 + 100);
        let s = log.stats.summary_at(Some(1), T0 + 100);
        let c = row(&s, C);
        assert_eq!(
            (c.games, c.kills, c.wins, c.name.as_str()),
            (1, 1, 1, "Cee2")
        );
        assert_eq!(row(&s, A).wins, 0);
    }

    #[test]
    fn the_cache_follows_the_log_and_the_hour() {
        let mut log = Log::new();
        log.start("g", json!({}), T0);
        let a = log.stats.summary("7d", T0 + 10, true).unwrap();
        assert_eq!(a["window"], "7d");
        assert!(Arc::ptr_eq(
            &a,
            &log.stats.summary("7d", T0 + 20, true).unwrap()
        ));
        assert!(!Arc::ptr_eq(
            &a,
            &log.stats.summary("7d", T0 + HOUR, true).unwrap()
        ));
        let b = log.stats.summary("7d", T0 + HOUR, true).unwrap();
        log.kill("g", 1, Some(0), "kill", "smg", T0 + 30);
        let c = log.stats.summary("7d", T0 + HOUR, true).unwrap();
        assert!(!Arc::ptr_eq(&b, &c));
        assert_eq!(c["totals"]["kills"], 1);
        assert!(log.stats.summary("1y", T0, true).is_none());
    }
}
