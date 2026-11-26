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
//!
//! Bots (`bot: true`, docs/game/bots.md §6) are game-only: they have no
//! per-player row (a rostered bot's xuid included), and a kill with a bot
//! on either side counts in the totals but in nobody's row: not the human's
//! kills or spree, nor their deaths. A bot's superbounces and bounce
//! deaths count in the totals and in no row. The game record keeps them all.
//!
//! Medals (docs/live/live-api.md, "Medals") are counted as each game's
//! [`medals::Projection`] awards them: Halo 2's own from the host's
//! `GameMedal`s, our modes' from their events, the same projection the
//! film and the read model run. Totals count everyone's, bots' included;
//! a row an account's.
//!
//! Firefight (docs/game/firefight.md, "Scoring and stats") is its own
//! section of the summary, [`firefight::Summary`], folded from the host's
//! `FirefightScore` and `FirefightEnded`. It is not multiplayer's kills and
//! deaths, and no rank reads it. The Worker composes this same projection,
//! so both homes give the same summary. Every finished match counts in
//! `games`; `won` is one team's wins against the AI; a two-team match
//! counts again in `versus`, by the team that won or as a draw. A player
//! kill (two teams) counts in `player_kills`, the totals' and the killer's
//! row's.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Arc, Mutex, RwLock};

use axum::extract::{Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::Xuid;
use live_proto::film::events::medals;
use live_proto::firefight::{self, FirefightEnded, FirefightScore};
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
    /// Matchmade games against bots (docs/game/bots.md §8.2 item 1): their
    /// own kind, so counted here and not in `matchmaking`; they still
    /// count in `games`, `multiplayer`, the playlists and every per-player
    /// number, like human games.
    pub clankermatch: u64,
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
    /// Superbounces, every player's (docs/game/superbounce.md, "Medals").
    pub bounces: u64,
    /// Deaths falling out of the map while trying a bounce (the `bounced`
    /// medal).
    pub bounced: u64,
    /// Teabags, every player's (docs/game/teabag.md).
    pub teabags: u64,
    /// Medals by id, every player's ([`medals::Projection`]).
    pub medals: BTreeMap<String, u64>,
}

impl Totals {
    fn add(&mut self, o: &Totals) {
        self.games += o.games;
        self.multiplayer += o.multiplayer;
        self.campaign += o.campaign;
        self.matchmaking += o.matchmaking;
        self.clankermatch += o.clankermatch;
        self.custom += o.custom;
        self.coop += o.coop;
        self.kills += o.kills;
        self.deaths += o.deaths;
        self.betrayals += o.betrayals;
        self.suicides += o.suicides;
        self.campaign_deaths += o.campaign_deaths;
        self.player_seconds += o.player_seconds;
        self.legendary += o.legendary;
        self.bounces += o.bounces;
        self.bounced += o.bounced;
        self.teabags += o.teabags;
        add_medals(&mut self.medals, &o.medals);
    }
}

fn add_medals(to: &mut BTreeMap<String, u64>, from: &BTreeMap<String, u64>) {
    for (id, n) in from {
        *to.entry(id.clone()).or_default() += n;
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
    bounces: u64,
    bounced: u64,
    teabags_given: u64,
    teabags_received: u64,
    medals: BTreeMap<String, u64>,
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
        self.bounces += o.bounces;
        self.bounced += o.bounced;
        self.teabags_given += o.teabags_given;
        self.teabags_received += o.teabags_received;
        add_medals(&mut self.medals, &o.medals);
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

/// One player's Firefight numbers.
#[derive(Debug, Clone, Default)]
struct FfPlayer {
    name: String,
    named_at: i64,
    score: u64,
    kills: u64,
    assists: u64,
    player_kills: u64,
    games: u64,
}

/// Firefight over a span of time.
#[derive(Debug, Clone, Default)]
struct FfAgg {
    games: u64,
    won: u64,
    rounds_cleared: u64,
    sets_cleared: u64,
    lives_used: u64,
    score: u64,
    player_kills: u64,
    versus: firefight::VersusRow,
    /// Wave set -> (games, won, rounds cleared, sets cleared).
    sets: BTreeMap<String, (u64, u64, u64, u64)>,
    classes: BTreeMap<String, u64>,
    players: HashMap<Xuid, FfPlayer>,
}

impl FfAgg {
    fn add(&mut self, o: &FfAgg) {
        self.games += o.games;
        self.won += o.won;
        self.rounds_cleared += o.rounds_cleared;
        self.sets_cleared += o.sets_cleared;
        self.lives_used += o.lives_used;
        self.score += o.score;
        self.player_kills += o.player_kills;
        self.versus.games += o.versus.games;
        self.versus.wins[0] += o.versus.wins[0];
        self.versus.wins[1] += o.versus.wins[1];
        self.versus.draws += o.versus.draws;
        for (k, v) in &o.sets {
            let e = self.sets.entry(k.clone()).or_default();
            *e = (e.0 + v.0, e.1 + v.1, e.2 + v.2, e.3 + v.3);
        }
        for (k, n) in &o.classes {
            *self.classes.entry(k.clone()).or_default() += n;
        }
        for (x, p) in &o.players {
            let e = self.players.entry(*x).or_default();
            if p.named_at >= e.named_at {
                e.name = p.name.clone();
                e.named_at = p.named_at;
            }
            e.score += p.score;
            e.kills += p.kills;
            e.assists += p.assists;
            e.player_kills += p.player_kills;
            e.games += p.games;
        }
    }

    fn player(&mut self, x: Xuid, name: &str, at: i64) -> &mut FfPlayer {
        let p = self.players.entry(x).or_default();
        if at >= p.named_at {
            p.name = name.to_string();
            p.named_at = at;
        }
        p
    }

    fn summary(&self) -> firefight::Summary {
        let mut sets: Vec<firefight::SetRow> = self
            .sets
            .iter()
            .map(|(set, (games, won, rounds, sets))| firefight::SetRow {
                set: set.clone(),
                games: *games,
                won: *won,
                rounds_cleared: *rounds,
                sets_cleared: *sets,
            })
            .collect();
        sets.sort_by(|a, b| b.games.cmp(&a.games).then_with(|| a.set.cmp(&b.set)));
        sets.truncate(firefight::MAX_ROWS);
        let mut kills_by_class: Vec<firefight::ClassRow> = self
            .classes
            .iter()
            .map(|(class, kills)| firefight::ClassRow {
                class: class.clone(),
                kills: *kills,
            })
            .collect();
        kills_by_class.sort_by(|a, b| b.kills.cmp(&a.kills).then_with(|| a.class.cmp(&b.class)));
        kills_by_class.truncate(firefight::MAX_ROWS);
        let mut players: Vec<firefight::PlayerRow> = self
            .players
            .iter()
            .map(|(x, p)| firefight::PlayerRow {
                xuid: x.to_string(),
                name: p.name.clone(),
                score: p.score,
                kills: p.kills,
                assists: p.assists,
                player_kills: p.player_kills,
                games: p.games,
            })
            .collect();
        players.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.xuid.cmp(&b.xuid)));
        players.truncate(firefight::MAX_ROWS);
        firefight::Summary {
            games: self.games,
            won: self.won,
            rounds_cleared: self.rounds_cleared,
            sets_cleared: self.sets_cleared,
            lives_used: self.lives_used,
            score: self.score,
            player_kills: self.player_kills,
            versus: self.versus.clone(),
            sets,
            kills_by_class,
            players,
        }
    }
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
    firefight: FfAgg,
}

impl Bucket {
    fn add(&mut self, o: &Bucket) {
        self.totals.add(&o.totals);
        self.firefight.add(&o.firefight);
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
    /// A kill can be a betrayal ([`game::NewGame::has_teammates`]).
    teammates: bool,
    map: String,
    map_id: Option<String>,
    difficulty: Option<String>,
    /// Slot -> account and name; kept after a player leaves so late kills
    /// still find them (a slot taken again is overwritten). A bot's slot
    /// has no account.
    slots: HashMap<u32, (Option<Xuid>, String, i32)>,
    /// The slots bots play in.
    bots: BTreeSet<u32>,
    /// Accounts counted as having played it.
    xuids: BTreeSet<Xuid>,
    /// Current scoring run per slot.
    spree: HashMap<u32, u64>,
    /// The Firefight result was counted (a repeat counts nothing).
    firefight_ended: bool,
    /// Works the game's medals out of its events.
    medals: medals::Projection,
}

/// The account a player's stats go to: none for a bot.
fn account(p: &game::Player) -> Option<Xuid> {
    p.xuid.filter(|_| !p.is_bot())
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
        // Before the event itself: a game's end takes the game away.
        if b.kind != game::GAME_STARTED {
            self.medals(id, &b.kind, &b.data, at);
        }
        match b.kind.as_str() {
            game::GAME_STARTED => {
                self.started(e, at);
                self.medals(id, &b.kind, &b.data, at);
            }
            game::PLAYER_JOINED => {
                let Some(d) = data::<game::PlayerJoined>(e) else {
                    return;
                };
                let Some(g) = self.games.get_mut(id) else {
                    return;
                };
                let p = d.player;
                g.slots
                    .insert(p.index, (account(&p), p.name.clone(), p.team));
                g.spree.remove(&p.index);
                if p.is_bot() {
                    g.bots.insert(p.index);
                    return;
                }
                g.bots.remove(&p.index);
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
            game::FIREFIGHT_SCORE => {
                let Some(d) = data::<FirefightScore>(e) else {
                    return;
                };
                self.firefight_scored(id, &d, at);
            }
            game::FIREFIGHT_ENDED => {
                let Some(d) = data::<FirefightEnded>(e) else {
                    return;
                };
                self.firefight_ended(id, &d, at);
            }
            game::SUPERBOUNCE => {
                let Some(d) = data::<game::Superbounce>(e) else {
                    return;
                };
                self.bounced(id, d.player, at, false);
            }
            game::BOUNCED => {
                let Some(d) = data::<game::Bounced>(e) else {
                    return;
                };
                self.bounced(id, d.player, at, true);
            }
            game::TEABAG => {
                let Some(d) = data::<game::Teabag>(e) else {
                    return;
                };
                self.teabagged(id, d.player, d.victim, at);
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
        if ng.sim.is_some() {
            return;
        }
        let campaign = ng.mode == "campaign";
        let kind = ng.kind().to_string();
        let g = LiveGame {
            started: at,
            campaign,
            teammates: ng.has_teammates(),
            map: ng.map.clone(),
            map_id: ng.map_id.map(|m| m.to_string()),
            difficulty: ng.difficulty.as_ref().map(|d| d.to_ascii_lowercase()),
            slots: ng
                .players
                .iter()
                .map(|p| (p.index, (account(p), p.name.clone(), p.team)))
                .collect(),
            bots: ng
                .players
                .iter()
                .filter(|p| p.is_bot())
                .map(|p| p.index)
                .collect(),
            xuids: ng.players.iter().filter_map(account).collect(),
            spree: HashMap::new(),
            firefight_ended: false,
            medals: medals::Projection::default(),
        };
        let coop = campaign && (ng.players.len() > 1 || ng.online == Some(true));
        let players: Vec<(Xuid, String)> = ng
            .players
            .iter()
            .filter_map(|p| Some((account(p)?, p.name.clone())))
            .collect();
        let playlist = game::is_matchmade(&kind)
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
            "clankermatch" => delta.clankermatch = 1,
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

    /// A credited score. Totals and classes count everyone; a player's row
    /// is for an account, never a bot (the module doc).
    fn firefight_scored(&mut self, id: &str, d: &FirefightScore, at: i64) {
        let Some(g) = self.games.get(id) else {
            return;
        };
        let who = g
            .slots
            .get(&d.player)
            .filter(|_| !g.bots.contains(&d.player))
            .and_then(|s| Some((s.0?, s.1.clone())));
        let points = u64::from(d.points);
        let reason = d.reason.as_str();
        self.bump(at, |b| {
            let f = &mut b.firefight;
            f.score += points;
            if reason == "player_kill" {
                f.player_kills += 1;
            }
            if let Some(class) = d.class.as_ref().filter(|_| reason == "kill") {
                *f.classes.entry(class.clone()).or_default() += 1;
            }
            if let Some((x, name)) = &who {
                let p = f.player(*x, name, at);
                p.score += points;
                match reason {
                    "kill" => p.kills += 1,
                    "assist" => p.assists += 1,
                    "player_kill" => p.player_kills += 1,
                    _ => {}
                }
            }
        });
    }

    /// A match's result: counted once, for every account that played it.
    fn firefight_ended(&mut self, id: &str, d: &FirefightEnded, at: i64) {
        let Some(g) = self.games.get_mut(id) else {
            return;
        };
        if std::mem::replace(&mut g.firefight_ended, true) {
            return;
        }
        let names: Vec<(Xuid, String)> = g
            .xuids
            .iter()
            .map(|x| {
                let name = g
                    .slots
                    .values()
                    .find(|s| s.0 == Some(*x))
                    .map_or_else(String::new, |s| s.1.clone());
                (*x, name)
            })
            .collect();
        let won = d.outcome == "won";
        let versus = d.teams == 2;
        let winner = d.winner().map(usize::from).filter(|&t| t < 2);
        let draw = d.outcome == "draw";
        self.bump(at, |b| {
            let f = &mut b.firefight;
            f.games += 1;
            f.won += won as u64;
            if versus {
                f.versus.games += 1;
                f.versus.draws += draw as u64;
                if let Some(t) = winner {
                    f.versus.wins[t] += 1;
                }
            }
            f.rounds_cleared += u64::from(d.rounds_cleared);
            f.sets_cleared += u64::from(d.sets_cleared);
            f.lives_used += u64::from(d.lives_used);
            let set = f.sets.entry(d.set.clone()).or_default();
            set.0 += 1;
            set.1 += won as u64;
            set.2 += u64::from(d.rounds_cleared);
            set.3 += u64::from(d.sets_cleared);
            for (x, name) in &names {
                f.player(*x, name, at).games += 1;
            }
        });
    }

    /// A bounce, or a death trying one (`died`), counts in the totals, and
    /// in the player's row when it is a player with an account and not a
    /// bot.
    fn bounced(&mut self, id: &str, slot: u32, at: i64, died: bool) {
        let Some(g) = self.games.get(id) else {
            return;
        };
        let player = g
            .slots
            .get(&slot)
            .filter(|_| !g.bots.contains(&slot))
            .and_then(|(x, name, _)| x.map(|x| (x, name.clone())));
        let add = |t: &mut Totals| {
            if died {
                t.bounced += 1;
            } else {
                t.bounces += 1;
            }
        };
        self.bump(at, |b| {
            add(&mut b.totals);
            if let Some((x, name)) = &player {
                let p = b.player(*x, name, at);
                if died {
                    p.bounced += 1;
                } else {
                    p.bounces += 1;
                }
            }
        });
        self.hour(at, add);
    }

    /// The medals game event `kind` awards count in the totals, and in the
    /// earner's row when it is a player with an account and not a bot.
    fn medals(&mut self, id: &str, kind: &str, data: &Value, at: i64) {
        let Some(g) = self.games.get_mut(id) else {
            return;
        };
        let awards = g.medals.game_event(kind, data);
        if awards.is_empty() {
            return;
        }
        let earned: Vec<(&'static str, Option<(Xuid, String)>)> = awards
            .iter()
            .map(|a| {
                let row = g
                    .slots
                    .get(&a.player)
                    .filter(|_| !g.bots.contains(&a.player))
                    .and_then(|(x, name, _)| x.map(|x| (x, name.clone())));
                (a.medal, row)
            })
            .collect();
        self.bump(at, |b| {
            for (medal, row) in &earned {
                *b.totals.medals.entry((*medal).to_owned()).or_default() += 1;
                if let Some((x, name)) = row {
                    *b.player(*x, name, at)
                        .medals
                        .entry((*medal).to_owned())
                        .or_default() += 1;
                }
            }
        });
        self.hour(at, |t| {
            for (medal, _) in &earned {
                *t.medals.entry((*medal).to_owned()).or_default() += 1;
            }
        });
    }

    /// A teabag counts in the totals, and in the giver's and the
    /// receiver's rows when each is a player with an account and not a
    /// bot.
    fn teabagged(&mut self, id: &str, giver: u32, victim: u32, at: i64) {
        let Some(g) = self.games.get(id) else {
            return;
        };
        let row = |slot: u32| {
            g.slots
                .get(&slot)
                .filter(|_| !g.bots.contains(&slot))
                .and_then(|(x, name, _)| x.map(|x| (x, name.clone())))
        };
        let (giver, victim) = (row(giver), row(victim));
        self.bump(at, |b| {
            b.totals.teabags += 1;
            if let Some((x, name)) = &giver {
                b.player(*x, name, at).teabags_given += 1;
            }
            if let Some((x, name)) = &victim {
                b.player(*x, name, at).teabags_received += 1;
            }
        });
        self.hour(at, |t| t.teabags += 1);
    }

    fn killed(&mut self, id: &str, k: &game::PlayerKilled, at: i64) {
        let Some(g) = self.games.get_mut(id) else {
            return;
        };
        // A kill with a bot on either side is game-only (the module doc).
        let bot = g.bots.contains(&k.victim) || k.killer.is_some_and(|s| g.bots.contains(&s));
        let victim = g.slots.get(&k.victim).and_then(|s| s.0).filter(|_| !bot);
        if g.campaign {
            self.bump(at, |b| b.totals.campaign_deaths += 1);
            self.hour(at, |h| h.campaign_deaths += 1);
            return;
        }
        g.spree.insert(k.victim, 0);
        let killer = k.killer.filter(|&s| s != k.victim);
        // Settled again by the teams: older hosts sent free-for-all kills
        // as betrayals.
        let betrayal = game::is_betrayal(g.teammates, k.killer, k.victim, |s| {
            g.slots.get(&s).map(|slot| slot.2)
        });
        let kind = game::settled_kill_kind(&k.kind, k.killer, k.victim, betrayal);
        let scoring = kind == "kill" && killer.is_some();
        let mut spree = 0;
        if let Some(s) = killer.filter(|_| scoring && !bot) {
            let n = g.spree.entry(s).or_default();
            *n += 1;
            spree = *n;
        }
        let killer_acct = killer
            .filter(|_| !bot)
            .and_then(|s| g.slots.get(&s).cloned());
        let g = g.clone();
        // A player can receive kill credit for an environmental death.
        let weapon = k.weapon.as_deref().filter(|&w| {
            !matches!(w, "" | "fall" | "collision" | "splatter" | "guardians" | "teleporter" | "explosion")
        });
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
                if let Some(w) = weapon {
                    *b.weapons.entry(w.to_owned()).or_default() += 1;
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
                bounces: p.bounces,
                bounced: p.bounced,
                teabags_given: p.teabags_given,
                teabags_received: p.teabags_received,
                medals: p.medals.clone(),
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
            firefight: merged.firefight.summary(),
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
    pub bounces: u64,
    pub bounced: u64,
    /// Teabags this player gave, and got (docs/game/teabag.md).
    pub teabags_given: u64,
    pub teabags_received: u64,
    /// Medals by id ([`medals::Projection`]).
    pub medals: BTreeMap<String, u64>,
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
    /// Firefight's matches, kills and scores ([`firefight::Summary`]).
    pub firefight: firefight::Summary,
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

    /// Account `x`'s medals over every game it played, by id: of every
    /// game when `all` (admins), else of the public ones. Empty for an
    /// account with none.
    pub fn account_medals(&self, x: Xuid, all: bool) -> BTreeMap<String, u64> {
        let inner = if all { &self.inner } else { &self.public }.read().unwrap();
        inner
            .all
            .players
            .get(&x)
            .map(|p| p.medals.clone())
            .unwrap_or_default()
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
            game::GAME_STARTED
                | game::PLAYER_JOINED
                | game::PLAYER_LEFT
                | game::PLAYER_KILLED
                | game::FIREFIGHT_SCORE
                | game::FIREFIGHT_ENDED
                | game::SUPERBOUNCE
                | game::BOUNCED
                | game::TEABAG
                | game::GAME_MEDAL
                | game::GAME_ENDED
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
    let all = viewer.is_admin();
    let v = live
        .stats
        .summary(window, crate::app::now(), all)
        .ok_or_else(|| invalid("window is today, 7d, 30d or all"))?;
    Ok(crate::http::json(StatusCode::OK, &*v))
}

/// `GET /v1/accounts/{who}/medals`: an account's medals over every game it
/// played (docs/live/live-api.md, "Medals").
pub async fn get_account_medals(
    State(live): State<Arc<Live>>,
    headers: HeaderMap,
    axum::extract::Path(who): axum::extract::Path<String>,
) -> Result<Response, ApiError> {
    let viewer = live.read_viewer(crate::http::bearer(&headers))?;
    let xuid = {
        let s = live.model.read();
        crate::ranks::xuid_of(&s, &who).ok_or_else(|| crate::app::not_found("no such account"))?
    };
    let medals = live.stats.account_medals(xuid, viewer.is_admin());
    let body = json!({"xuid": xuid, "medals": medals, "position": live.position()});
    Ok(crate::http::json(StatusCode::OK, &body))
}

#[cfg(test)]
mod tests {
    include!("stats/film_events.rs");

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

    const FIREFIGHT_EVENTS: &str = include_str!("../tests/fixtures/firefight-events.json");
    const CROSSFIRE_EVENTS: &str =
        include_str!("../tests/fixtures/firefight-crossfire-events.json");
    const FIREFIGHT_GOLDEN: &str = include_str!("../tests/fixtures/firefight-summary.json");

    /// A scripted Firefight match: `players`, the events `fixture` holds,
    /// then `GameEnded`.
    fn scripted(log: &mut Log, game: &str, players: Value, fixture: &str, at: i64) {
        log.start(game, json!({ "players": players }), at);
        for e in serde_json::from_str::<Vec<Value>>(fixture).unwrap() {
            log.push(game, e["kind"].as_str().unwrap(), e["data"].clone(), at + 1);
        }
        log.end(game, "finished", json!({"player": 0}), at + 2);
    }

    /// The one-team match: Host (an account) and a Clanker.
    fn firefight_match(log: &mut Log, game: &str, at: i64) {
        let players = json!([
            {"index": 0, "name": "Host", "xuid": A, "team": 0},
            {"index": 1, "name": "Clanker 1", "team": 0, "bot": true},
        ]);
        scripted(log, game, players, FIREFIGHT_EVENTS, at);
    }

    /// The two-team match (Crossfire): Host and a Clanker on team 0, two
    /// Clankers on team 1.
    fn crossfire_match(log: &mut Log, game: &str, at: i64) {
        let players = json!([
            {"index": 0, "name": "Host", "xuid": A, "team": 0},
            {"index": 1, "name": "Clanker 1", "team": 0, "bot": true},
            {"index": 2, "name": "Clanker 2", "team": 1, "bot": true},
            {"index": 3, "name": "Clanker 3", "team": 1, "bot": true},
        ]);
        scripted(log, game, players, CROSSFIRE_EVENTS, at);
    }

    #[test]
    fn a_scripted_match_projects_to_the_golden_summary() {
        let mut log = Log::new();
        firefight_match(&mut log, "ff", T0);
        crossfire_match(&mut log, "cf", T0 + 10);
        let golden: Value = serde_json::from_str(&FIREFIGHT_GOLDEN.replace("$HOST", A)).unwrap();
        let s = log.stats.summary_at(None, T0 + 100);
        assert_eq!(serde_json::to_value(&s.firefight).unwrap(), golden);
        // A repeated result counts nothing more, and a match that never
        // reports one gives the summary nothing to count.
        let mut again = Log::new();
        firefight_match(&mut again, "ff", T0);
        crossfire_match(&mut again, "cf", T0 + 10);
        again.push(
            "ff",
            "FirefightEnded",
            json!({"tick": 801, "set": "Holdout", "teams": 1, "outcome": "lost", "scores": [0], "rounds_cleared": 9, "lives_used": 9}),
            T0 + 3,
        );
        again.push(
            "cf",
            "FirefightEnded",
            json!({"tick": 801, "set": "Crossfire", "teams": 2, "outcome": "draw", "scores": [0, 0], "rounds_cleared": 9, "lives_used": 9}),
            T0 + 13,
        );
        assert_eq!(again.stats.summary_at(None, T0 + 100).firefight, s.firefight);
        // The window reads the same fold.
        assert_eq!(log.stats.summary_at(Some(1), T0 + 100).firefight, s.firefight);
        // Firefight is not multiplayer's kills or deaths.
        assert_eq!((s.totals.kills, s.totals.deaths), (0, 0));
        assert_eq!(row(&s, A).kills, 0);
    }

    /// Two teams: every match counts in `games`, none in the one-team `won`;
    /// `versus` counts each team's wins and the draws, and a match both
    /// teams lost in `games` alone. Player kills count in the totals and the
    /// killer's row; a bot's in the totals only.
    #[test]
    fn a_two_team_match_projects_wins_by_team() {
        let mut log = Log::new();
        crossfire_match(&mut log, "c1", T0);
        let end = |outcome: &str, team: Option<u8>| {
            let mut d = json!({"tick": 800, "set": "Crossfire", "teams": 2, "outcome": outcome,
                "scores": [10, 20], "rounds_cleared": 1, "lives_used": 2});
            if let Some(t) = team {
                d["team"] = json!(t);
            }
            d
        };
        for (game, data) in [
            ("c2", end("team_won", Some(1))),
            ("c3", end("team_won", Some(1))),
            ("c4", end("draw", None)),
            ("c5", end("lost", None)),
        ] {
            log.start(game, json!({"players": [
                {"index": 0, "name": "Host", "xuid": A, "team": 0},
                {"index": 1, "name": "Clanker 1", "team": 1, "bot": true},
            ]}), T0 + 10);
            log.push(game, "FirefightEnded", data, T0 + 11);
            log.end(game, "finished", json!({"player": 0}), T0 + 12);
        }
        let f = log.stats.summary_at(None, T0 + 100).firefight;
        assert_eq!((f.games, f.won), (5, 0));
        assert_eq!(
            f.versus,
            firefight::VersusRow {
                games: 5,
                wins: [1, 2],
                draws: 1
            }
        );
        assert_eq!(f.player_kills, 2);
        let host = &f.players[0];
        assert_eq!((host.player_kills, host.kills, host.games), (1, 1, 5));
        assert_eq!(f.sets[0].set, "Crossfire");
        assert_eq!((f.sets[0].games, f.sets[0].won), (5, 0));
        // A one-team match counts in `won`, never in `versus`.
        firefight_match(&mut log, "h1", T0 + 20);
        let f = log.stats.summary_at(None, T0 + 100).firefight;
        assert_eq!((f.games, f.won, f.versus.games), (6, 1, 5));
    }

    /// Superbounces and bounce deaths count in the totals, a player's in
    /// their row; a bot's and an unlinked guest's in no row
    /// (docs/game/superbounce.md, "Recording").
    #[test]
    fn bounces_count_in_the_totals_and_the_rows_of_players_with_accounts() {
        let mut log = Log::new();
        log.start("g1", json!({}), T0);
        log.push(
            "g1",
            game::PLAYER_JOINED,
            json!({"player": {"index": 3, "name": "Clanker", "team": 1, "bot": true}}),
            T0 + 1,
        );
        for (player, chain) in [(0, 1), (0, 2), (1, 1), (2, 1), (3, 1), (3, 2)] {
            log.push(
                "g1",
                game::SUPERBOUNCE,
                json!({"tick": 5, "player": player, "chain": chain}),
                T0 + 10,
            );
        }
        for player in [1, 3] {
            log.push(
                "g1",
                game::BOUNCED,
                json!({"tick": 6, "player": player}),
                T0 + 20,
            );
        }
        log.end("g1", "finished", json!({"team": 0}), T0 + 60);
        let s = log.stats.summary_at(Some(1), T0 + 100);
        assert_eq!((s.totals.bounces, s.totals.bounced), (6, 2));
        assert_eq!((s.last_24h.bounces, s.last_24h.bounced), (6, 2));
        assert_eq!((row(&s, A).bounces, row(&s, A).bounced), (2, 0));
        assert_eq!((row(&s, B).bounces, row(&s, B).bounced), (1, 1));
        assert_eq!(s.players.len(), 2, "no row for the guest or the bot");
    }

    /// Teabags count in the totals, and as given and received in the rows
    /// of players with accounts; a bot's side and an unlinked guest's are
    /// in no row (docs/game/teabag.md, "Recording").
    #[test]
    fn teabags_count_given_and_received_in_the_rows_of_players_with_accounts() {
        let mut log = Log::new();
        log.start("g1", json!({}), T0);
        log.push(
            "g1",
            game::PLAYER_JOINED,
            json!({"player": {"index": 3, "name": "Clanker", "team": 1, "bot": true}}),
            T0 + 1,
        );
        for (player, victim) in [(0, 1), (0, 1), (1, 2), (3, 0), (0, 3)] {
            log.push(
                "g1",
                game::TEABAG,
                json!({"tick": 5, "player": player, "victim": victim}),
                T0 + 10,
            );
        }
        log.end("g1", "finished", json!({"team": 0}), T0 + 60);
        let s = log.stats.summary_at(Some(1), T0 + 100);
        assert_eq!(s.totals.teabags, 5);
        assert_eq!(s.last_24h.teabags, 5);
        assert_eq!(
            (row(&s, A).teabags_given, row(&s, A).teabags_received),
            (3, 1)
        );
        assert_eq!(
            (row(&s, B).teabags_given, row(&s, B).teabags_received),
            (1, 2)
        );
        assert_eq!(s.players.len(), 2, "no row for the guest or the bot");
    }

    #[test]
    fn credited_environmental_deaths_count_as_kills_in_map_and_player_totals() {
        let mut log = Log::new();
        log.start("g1", json!({}), T0);
        for source in ["fall", "collision", "splatter", "guardians", "teleporter", "explosion"] {
            log.kill("g1", 1, Some(0), "kill", source, T0 + 1);
        }
        log.kill("g1", 1, Some(0), "kill", "frag_grenade", T0 + 2);

        let s = log.stats.summary_at(Some(7), T0 + 3);
        assert_eq!(s.totals.kills, 7);
        assert_eq!(s.maps[0].kills, 7);
        assert_eq!(row(&s, A).kills, 7);
        assert_eq!(s.weapons, vec![WeaponRow { weapon: "frag_grenade".into(), kills: 1 }]);
    }

    /// Hosts before the fix sent every free-for-all kill as a betrayal:
    /// the stats count them as kills. In a team game a kill of a teammate
    /// is a betrayal whatever the host sent, and one of an enemy a kill.
    #[test]
    fn betrayals_are_settled_by_the_teams() {
        let mut log = Log::new();
        let everyone_on_team_0 = json!([
            {"index": 0, "name": "Ace", "xuid": A, "team": 0},
            {"index": 1, "name": "Bee", "xuid": B, "team": 0},
        ]);
        log.start(
            "ffa",
            json!({"team_play": false, "players": everyone_on_team_0}),
            T0,
        );
        log.kill("ffa", 1, Some(0), "betrayal", "smg", T0 + 10);
        log.kill("ffa", 0, Some(1), "betrayal", "smg", T0 + 20);
        log.kill("ffa", 1, Some(0), "kill", "smg", T0 + 30);
        log.end("ffa", "finished", json!({"player": 0}), T0 + 600);

        let s = log.stats.summary_at(Some(7), T0 + 700);
        let t = &s.totals;
        assert_eq!((t.kills, t.deaths, t.betrayals), (3, 3, 0));
        let (a, b) = (row(&s, A), row(&s, B));
        assert_eq!((a.kills, a.betrayals, a.best_spree), (2, 0, 1));
        assert_eq!((b.kills, b.betrayals), (1, 0));

        // The default game is a team game: Ace 0, Bee and the guest 1.
        log.start("team", json!({}), T0 + 1000);
        log.kill("team", 2, Some(1), "kill", "smg", T0 + 1010);
        log.kill("team", 1, Some(0), "betrayal", "smg", T0 + 1020);
        log.end("team", "finished", json!({"team": 0}), T0 + 1600);
        let s = log.stats.summary_at(Some(7), T0 + 1700);
        assert_eq!((s.totals.kills, s.totals.betrayals), (4, 1));
        let (a, b) = (row(&s, A), row(&s, B));
        assert_eq!((a.kills, a.betrayals), (3, 0));
        assert_eq!((b.kills, b.betrayals), (1, 1));
    }

    #[test]
    fn a_clankermatch_game_counts_as_its_own_kind_and_in_its_playlist() {
        let mut log = Log::new();
        log.start(
            "g1",
            json!({"kind": "matchmaking", "playlist": "Team Slayer"}),
            T0,
        );
        log.end("g1", "finished", json!({"team": 0}), T0 + 600);
        log.start(
            "g2",
            json!({"kind": "clankermatch", "playlist": "Clanker Teams"}),
            T0 + 1000,
        );
        log.kill("g2", 1, Some(0), "kill", "battle_rifle", T0 + 1010);
        log.end("g2", "finished", json!({"team": 0}), T0 + 1600);

        let s = log.stats.summary_at(Some(7), T0 + 2000);
        let t = &s.totals;
        assert_eq!(
            (
                t.games,
                t.multiplayer,
                t.matchmaking,
                t.clankermatch,
                t.custom
            ),
            (2, 2, 1, 1, 0)
        );
        assert_eq!(t.kills, 1);
        let mut playlists = s.playlists.clone();
        playlists.sort_by(|a, b| a.playlist.cmp(&b.playlist));
        assert_eq!(
            playlists,
            vec![
                PlaylistRow {
                    playlist: "Clanker Teams".into(),
                    games: 1
                },
                PlaylistRow {
                    playlist: "Team Slayer".into(),
                    games: 1
                },
            ]
        );
        // Career numbers count both, like human games.
        let a = row(&s, A);
        assert_eq!((a.games, a.multiplayer, a.kills), (2, 2, 1));
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
