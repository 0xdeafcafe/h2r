//! Bot accounts and rosters (docs/game/bots.md §9). The server issues bots
//! xuids in a range of their own (`live_proto::bots::SERVER_XUID_MARK`),
//! each with a server-signed `BotIssued` on `bot-<xuid>`, and answers a
//! host's `POST /v1/games/bots` with a roster, written as `BotsRostered` on
//! `bots-<session_id>`. Nobody else writes those streams.
//!
//! A persistent bot (`POST /v1/admin/bots`, the operator) keeps its
//! account, reserved gamertag and rank for good; a made-up bot is issued
//! for one roster. A game record's `bot: true` entries are accepted only
//! for the xuids of the session's roster ([`check_players`]), and ranking
//! reads [`Bots::rostered`] for the roster in force when the game started.

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use axum::extract::State as HttpState;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use live_proto::bots::{
    self, Colors, Emblem, ISSUED, Issued, MAX_BOTS, Personality, ROSTERED, Roster, RosterBot,
    RosterRequest, Rostered, RosteredBot, SERVER_XUID_MARK,
};
use live_proto::game::Player;
use live_proto::{Author, Gamertag, Xuid, lxm, streams, types};
use serde::Deserialize;
use serde_json::json;

use crate::app::{ApiError, Live, forbidden, invalid, not_found};
use crate::model::State;
use crate::store::{RecordedEvent, StoreError};

/// The most persistent bots one operator call issues.
pub const MAX_ISSUE: u32 = 64;

/// The projection of the `bot-*` and `bots-*` streams.
#[derive(Debug, Default)]
pub struct Bots {
    pub issued: HashMap<Xuid, Issued>,
    /// The persistent ones, for [`crate::ranks::Ranks::nearest`].
    pub persistent: BTreeSet<Xuid>,
    /// Session id -> its rosters, oldest first, with their log positions.
    pub rosters: HashMap<String, Vec<(u64, Rostered)>>,
    /// When each bot was issued (its `BotIssued`'s `at`): its account's
    /// `created_at`.
    pub issued_at: HashMap<Xuid, String>,
    /// Gamertag key -> the bot that holds it. Issuing never gives two
    /// bots one name, so this is one bot per name.
    pub by_tag: HashMap<String, Xuid>,
}

impl Bots {
    /// The server issued `xuid` as a bot.
    pub fn bot(&self, xuid: Xuid) -> Option<&Issued> {
        self.issued.get(&xuid)
    }

    /// The bot account called `tag`, if the server issued one.
    pub fn by_gamertag(&self, tag: &Gamertag) -> Option<&Issued> {
        self.issued.get(self.by_tag.get(&tag.key())?)
    }

    /// The session's roster in force just before log position `before`
    /// (the latest `BotsRostered` there before it). A game uses the one in
    /// force when it started: `before` = its `GameStarted` position.
    pub fn roster_at(&self, session_id: &str, before: u64) -> Option<&Rostered> {
        self.rosters
            .get(session_id)?
            .iter()
            .rev()
            .find(|(at, _)| *at < before)
            .map(|(_, r)| r)
    }

    /// `xuid` in the session's roster in force before `before` (see
    /// [`Bots::roster_at`]): whether it is persistent, the level the roster
    /// showed and the playlist.
    pub fn rostered(
        &self,
        session_id: &str,
        xuid: Xuid,
        before: u64,
    ) -> Option<(&Rostered, &RosteredBot)> {
        let r = self.roster_at(session_id, before)?;
        Some((r, r.bots.iter().find(|b| b.xuid == xuid)?))
    }
}

/// Whether `xuid` is a bot of `session`'s latest roster and `author` made
/// that roster: a bot the host leased for its session (§9.5), which the
/// host may report as one of the session's players (`PlayerJoined` on
/// `session-<id>`, docs/game/bots.md stage 6c "the session's players").
pub(crate) fn leased_to(s: &State, session: &str, author: &Author, xuid: Xuid) -> bool {
    let Author::Account { xuid: host, .. } = author else {
        return false;
    };
    s.bots
        .roster_at(session, u64::MAX)
        .is_some_and(|r| r.host == *host && r.bots.iter().any(|b| b.xuid == xuid))
}

/// Whether a host's new session `to` may carry the roster of its previous
/// session `from` (`RosterRequest::carry_from`, docs/game/bots.md §9.5):
/// `old` is `from` as the server has it and `roster` its latest roster.
/// Halo 2 hands its lobby to a new session as a game starts and ends
/// (deletes the old, advertises a new); the bots go with it, so they keep
/// their accounts. So `from` must be `host`'s own session, ended (deleted
/// or expired) no more than [`bots::CARRY_WINDOW_S`] before `now`, with a
/// roster `host` made. Otherwise why not.
pub fn may_carry(
    old: Option<&live_proto::session::Session>,
    roster: Option<&Rostered>,
    host: Xuid,
    from: &str,
    to: &str,
    now: i64,
) -> Result<(), String> {
    if from == to {
        return Err("a session cannot carry its own roster".into());
    }
    let old = old
        .filter(|v| v.host_xuid == host)
        .ok_or("no earlier session of yours by that id")?;
    if old.state == "open" {
        return Err("the earlier session is still open".into());
    }
    if now - old.last_activity > bots::CARRY_WINDOW_S {
        return Err(format!(
            "the earlier session ended more than {} s ago",
            bots::CARRY_WINDOW_S
        ));
    }
    if !roster.is_some_and(|r| r.host == host) {
        return Err("the earlier session has no roster of yours".into());
    }
    Ok(())
}

/// The roster `to` carries from `from` ([`may_carry`]).
fn carried<'a>(
    s: &'a State,
    host: Xuid,
    from: &str,
    to: &str,
    now: i64,
) -> Result<&'a Rostered, String> {
    let roster = s.bots.roster_at(from, u64::MAX);
    may_carry(s.sessions.get(from), roster, host, from, to, now)?;
    Ok(roster.expect("checked"))
}

pub(crate) fn project(s: &mut State, e: &RecordedEvent) {
    let b = &e.body;
    match b.kind.as_str() {
        ISSUED => {
            let Ok(d) = serde_json::from_value::<Issued>(b.data.clone()) else {
                return;
            };
            if d.persistent {
                s.bots.persistent.insert(d.xuid);
            }
            s.bots.issued_at.insert(d.xuid, b.at.clone());
            s.bots.by_tag.insert(d.gamertag.key(), d.xuid);
            s.bots.issued.insert(d.xuid, d);
        }
        ROSTERED => {
            let Ok(d) = serde_json::from_value::<Rostered>(b.data.clone()) else {
                return;
            };
            s.bots
                .rosters
                .entry(d.session_id.clone())
                .or_default()
                .push((e.position, d));
        }
        _ => {}
    }
}

/// The acceptance check (docs/game/bots.md §9.3) for players taken into a
/// game recorded by `host` under `session` whose roster is the one in force
/// before `before`: no host-made xuid, each `bot: true` entry with an xuid
/// in the roster, which `host` itself made, and no issued bot played as a
/// human. A `bot: true` entry without an xuid is the host's own Clanker:
/// taken, with no account to credit (game-only, §6). A record that fails
/// is refused whole.
pub(crate) fn check_players<'a>(
    s: &State,
    host: Option<Xuid>,
    session: Option<&str>,
    before: u64,
    players: impl IntoIterator<Item = &'a Player>,
) -> Result<(), String> {
    let roster = session
        .and_then(|id| s.bots.roster_at(id, before))
        .filter(|r| Some(r.host) == host);
    for p in players {
        if let Some(x) = p.xuid
            && bots::is_local_xuid(x)
        {
            return Err(format!("{x} is a host-made bot; it stays off the server"));
        }
        if p.is_bot() {
            let Some(x) = p.xuid else {
                continue;
            };
            if !roster.is_some_and(|r| r.bots.iter().any(|b| b.xuid == x)) {
                return Err(format!("{x} is not a bot of the session's roster"));
            }
        } else if let Some(x) = p.xuid
            && s.bots.bot(x).is_some()
        {
            return Err(format!("{x} is a bot, not a human"));
        }
    }
    Ok(())
}

// ---------- names and looks ----------

const FIRST: [&str; 32] = [
    "Rusty", "Bolt", "Clank", "Sprocket", "Gear", "Rivet", "Spanner", "Copper", "Chrome", "Static",
    "Dynamo", "Piston", "Widget", "Gizmo", "Tin", "Servo", "Crank", "Flux", "Volt", "Cog",
    "Ratchet", "Socket", "Diode", "Pixel", "Turbo", "Sparky", "Nickel", "Zinc", "Magnet", "Coil",
    "Fuse", "Relay",
];

const SECOND: [&str; 32] = [
    "Lugnut", "Bucket", "Gasket", "Washer", "Bearing", "Sprung", "Wrench", "Spindle", "Toaster",
    "Kettle", "Grinder", "Hinge", "Sprog", "Bracket", "Flange", "Nozzle", "Valve", "Lever",
    "Pulley", "Dial", "Gauge", "Bulb", "Switch", "Fender", "Axle", "Clamp", "Tube", "Rotor",
    "Pinion", "Shim", "Stud", "Grommet",
];

/// splitmix64: the bots' looks and quirks from one seed.
fn mix(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut z = *seed;
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

fn below(seed: &mut u64, n: u64) -> u64 {
    mix(seed) % n
}

/// 0 to 1 within `lo..hi`.
fn between(seed: &mut u64, lo: f32, hi: f32) -> f32 {
    lo + (hi - lo) * (below(seed, 10_001) as f32 / 10_000.0)
}

fn random_seed() -> u64 {
    let mut b = [0u8; 8];
    getrandom::fill(&mut b).expect("the OS random source failed");
    u64::from_be_bytes(b)
}

/// A name from the word lists, `Rusty Lugnut` style, at most 15
/// characters; `attempt` past the first few adds a number.
fn name(seed: &mut u64, attempt: u32) -> Gamertag {
    loop {
        let a = FIRST[below(seed, FIRST.len() as u64) as usize];
        let b = SECOND[below(seed, SECOND.len() as u64) as usize];
        let tag = if attempt < 4 {
            format!("{a} {b}")
        } else {
            format!("{a}{b}{}", below(seed, 100))
        };
        if let Ok(t) = Gamertag::new(&tag) {
            return t;
        }
    }
}

/// A new bot's account: a fresh xuid in the server range and looks and
/// quirks from `seed` (a persistent bot keeps them).
fn make(seed: u64, persistent: bool, attempt: u32) -> Issued {
    let mut s = seed;
    let mut low = [0u8; 8];
    getrandom::fill(&mut low[2..]).expect("the OS random source failed");
    let xuid = Xuid(SERVER_XUID_MARK | u64::from_be_bytes(low));
    let gamertag = name(&mut s, attempt);
    let colour = |s: &mut u64| below(s, 18) as u8;
    let emblem = Emblem {
        foreground: below(&mut s, 64) as u8,
        background: below(&mut s, 32) as u8,
        colors: [colour(&mut s), colour(&mut s), colour(&mut s)],
    };
    let colors = Colors {
        primary: colour(&mut s),
        secondary: colour(&mut s),
    };
    let personality = Personality {
        seed,
        aggression: between(&mut s, 0.2, 0.8),
        caution: between(&mut s, 0.2, 0.8),
        objective: between(&mut s, 0.3, 0.9),
        teamwork: between(&mut s, 0.2, 0.8),
        taunt: between(&mut s, 0.1, 0.5),
    };
    Issued {
        xuid,
        gamertag,
        persistent,
        emblem,
        colors,
        personality,
    }
}

impl Live {
    /// Reads the bot projection.
    pub fn read_bots<R>(&self, f: impl FnOnce(&Bots) -> R) -> R {
        f(&self.model.read().bots)
    }

    /// Issues `count` new bots, written as `BotIssued` each at version 0 of
    /// its `bot-<xuid>` stream; a persistent one also reserves its gamertag
    /// (as an account's would be). A made-up one only avoids the names of
    /// accounts and other bots at the time. Retries a few times on a
    /// taken name or xuid.
    async fn issue(&self, count: usize, persistent: bool) -> Result<Vec<Issued>, ApiError> {
        let mut out: Vec<Issued> = Vec::with_capacity(count);
        let mut last = 0;
        while out.len() < count {
            let mut made = None;
            for attempt in 0..8 {
                let bot = make(random_seed(), persistent, attempt);
                let key = bot.gamertag.key();
                let taken = {
                    let s = self.model.read();
                    s.account_by_tag(&bot.gamertag).is_some()
                        || s.bots.issued.values().any(|b| b.gamertag.key() == key)
                        || out.iter().any(|b| b.gamertag.key() == key)
                };
                if taken {
                    continue;
                }
                let stream = bots::bot_stream(bot.xuid);
                let mut events = Vec::new();
                let mut expected = vec![(stream.clone(), 0)];
                if persistent {
                    let tag_stream = streams::gamertag(&bot.gamertag);
                    events.push(self.server_event(
                        tag_stream.clone(),
                        types::GAMERTAG_RESERVED,
                        json!({"xuid": bot.xuid}),
                    ));
                    expected.push((tag_stream, 0));
                }
                events.push(self.server_event(
                    stream,
                    ISSUED,
                    serde_json::to_value(&bot).expect("serializes"),
                ));
                match self.store.append(events, &expected).await {
                    Ok(recorded) => {
                        last = recorded.last().map_or(last, |e| e.position);
                        made = Some(bot);
                        break;
                    }
                    Err(StoreError::Conflict { .. }) => continue,
                    Err(e) => return Err(e.into()),
                }
            }
            let Some(bot) = made else {
                return Err(ApiError::new(
                    live_proto::ErrorCode::Unavailable,
                    "could not name a bot",
                ));
            };
            out.push(bot);
        }
        if last > 0 {
            self.wait_applied(last).await?;
        }
        Ok(out)
    }

    /// `POST /v1/games/bots` by an account with a `games.write` token that
    /// hosts the request's session: a roster of `count` bots (at most
    /// [`MAX_BOTS`], and no more than the session has room for), written as
    /// `BotsRostered` on `bots-<session_id>` (a later roster replaces it).
    ///
    /// With a playlist, the bots are the persistent ones nearest the
    /// humans' mean level there ([`crate::ranks::Ranks::nearest`]), at
    /// their own level and its skill (`rank::bot_skill`); any gap is filled
    /// with made-up bots at the mean level. Without one (a custom game),
    /// every bot is made up, at the request's skill (Normal when absent).
    /// `team` is left to the host, which balances by skill.
    pub async fn roster_bots(
        &self,
        bearer: Option<&str>,
        req: RosterRequest,
    ) -> Result<(Roster, u64), ApiError> {
        let author = self.authenticate(bearer, lxm::GAMES_WRITE)?;
        let Author::Account { xuid: host, .. } = author else {
            return Err(forbidden("a roster is for a session's host"));
        };
        self.rate_limit(&author, lxm::GAMES_WRITE)?;
        if req.count == 0 || req.count as usize > MAX_BOTS {
            return Err(invalid(format!("count is 1 to {MAX_BOTS}")));
        }
        if req.skill.is_some_and(|k| !(0.0..=1.0).contains(&k)) {
            return Err(invalid("skill is 0 to 1"));
        }
        if let Some(p) = &req.playlist
            && !live_proto::game::valid_id(p)
        {
            return Err(invalid("playlist must be [A-Za-z0-9_-]{1,64}"));
        }
        let (count, picked, mean, kept) = {
            let s = self.model.read();
            // A handoff: the earlier session's bots first, as they were.
            let kept: Vec<(Issued, u8, f32)> = match &req.carry_from {
                Some(from) => {
                    let r = carried(&s, host, from, &req.session_id, crate::app::now())
                        .map_err(|e| invalid(format!("carry_from: {e}")))?;
                    r.bots
                        .iter()
                        .filter_map(|b| Some((s.bots.bot(b.xuid)?.clone(), b.level, b.skill)))
                        .collect()
                }
                None => Vec::new(),
            };
            let session = s
                .sessions
                .get(&req.session_id)
                .filter(|v| v.host_xuid == host)
                .ok_or_else(|| not_found("no session of yours by that id"))?;
            let humans: Vec<Xuid> = std::iter::once(host)
                // The session's players include the bots the host
                // reported (§9.5): they are not humans.
                .chain(
                    session
                        .players
                        .keys()
                        .copied()
                        .filter(|x| s.bots.bot(*x).is_none()),
                )
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let room = live_proto::game::MAX_PLAYERS.saturating_sub(humans.len());
            let count = (req.count as usize).min(room);
            if count == 0 {
                return Err(invalid("the session has no room for bots"));
            }
            let kept: Vec<(Issued, u8, f32)> = kept.into_iter().take(count).collect();
            if kept.len() == count {
                (count, Vec::new(), None, kept)
            } else {
                let need = count - kept.len();
                let (picked, mean) = match &req.playlist {
                    Some(p) => {
                        let levels: u32 = humans
                            .iter()
                            .map(|x| u32::from(s.ranks.standing(*x, p).level))
                            .sum();
                        let mean = (levels / humans.len() as u32) as u8;
                        let picked = s.ranks.nearest(
                            p,
                            mean,
                            need,
                            s.bots
                                .persistent
                                .iter()
                                .copied()
                                .filter(|x| !kept.iter().any(|(b, _, _)| b.xuid == *x)),
                        );
                        let picked: Vec<(Issued, u8)> = picked
                            .into_iter()
                            .filter_map(|x| {
                                Some((s.bots.bot(x)?.clone(), s.ranks.standing(x, p).level))
                            })
                            .collect();
                        (picked, Some(mean))
                    }
                    None => (Vec::new(), None),
                };
                (count, picked, mean, kept)
            }
        };
        let made = self.issue(count - kept.len() - picked.len(), false).await?;
        let mean_level = mean.unwrap_or(live_proto::rank::MIN_LEVEL);
        let skill_at = |level: u8| match (&req.playlist, req.skill) {
            (Some(_), _) => live_proto::rank::bot_skill(level),
            (None, Some(k)) => k,
            (None, None) => bots::NORMAL,
        };
        let entries: Vec<(Issued, u8, f32)> = kept
            .into_iter()
            .chain(
                picked
                    .into_iter()
                    .chain(made.into_iter().map(|b| (b, mean_level)))
                    .map(|(b, level)| (b, level, skill_at(level))),
            )
            .collect();
        let rostered = Rostered {
            session_id: req.session_id.clone(),
            host,
            playlist: req.playlist.clone(),
            bots: entries
                .iter()
                .map(|(b, level, skill)| RosteredBot {
                    xuid: b.xuid,
                    persistent: b.persistent,
                    level: *level,
                    team: None,
                    skill: *skill,
                })
                .collect(),
        };
        let roster = Roster {
            match_id: req.session_id.clone(),
            bots: entries
                .iter()
                .zip(&rostered.bots)
                .map(|((b, level, _), r)| RosterBot {
                    xuid: b.xuid,
                    gamertag: b.gamertag.clone(),
                    persistent: b.persistent,
                    emblem: b.emblem,
                    colors: b.colors,
                    rank: req.playlist.as_ref().map(|p| bots::BotRank {
                        level: *level,
                        playlist: p.clone(),
                    }),
                    team: r.team,
                    skill: r.skill,
                    personality: b.personality,
                })
                .collect(),
        };
        let event = self.server_event(
            bots::roster_stream(&req.session_id),
            ROSTERED,
            serde_json::to_value(&rostered).expect("serializes"),
        );
        let recorded = self.store.append(vec![event], &[]).await?;
        let position = recorded[0].position;
        self.wait_applied(position).await?;
        Ok((roster, position))
    }

    /// `POST /v1/admin/bots` by the operator: issues `count` persistent
    /// bots (at most [`MAX_ISSUE`]), the pool rosters draw from.
    pub async fn issue_persistent_bots(
        &self,
        bearer: Option<&str>,
        count: u32,
    ) -> Result<Vec<Issued>, ApiError> {
        crate::admin::operator(self, bearer)?;
        if count == 0 || count > MAX_ISSUE {
            return Err(invalid(format!("count is 1 to {MAX_ISSUE}")));
        }
        self.issue(count as usize, true).await
    }
}

// ---------- route handlers ----------

pub(crate) async fn post_roster(
    HttpState(live): HttpState<Arc<Live>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let req: RosterRequest =
        serde_json::from_slice(&body).map_err(|e| invalid(format!("body: {e}")))?;
    let (roster, position) = live.roster_bots(crate::http::bearer(&headers), req).await?;
    let mut v = serde_json::to_value(roster).expect("serializes");
    v["position"] = json!(position);
    Ok(crate::http::json(StatusCode::CREATED, &v))
}

#[derive(Deserialize)]
struct IssueBody {
    count: u32,
}

pub(crate) async fn post_admin_bots(
    HttpState(live): HttpState<Arc<Live>>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Response, ApiError> {
    let b: IssueBody = serde_json::from_slice(&body).map_err(|e| invalid(format!("body: {e}")))?;
    let bots = live
        .issue_persistent_bots(crate::http::bearer(&headers), b.count)
        .await?;
    Ok(crate::http::json(
        StatusCode::CREATED,
        &json!({"bots": bots, "position": live.position()}),
    ))
}
