//! The in-memory read model: every projection the HTTP API and the
//! authenticator read, built by one [`Subscriber`] from the whole log at
//! startup and kept current by the dispatcher.
//!
//! It trusts the log: the server validated every event on the way in, so
//! an event whose data does not parse is skipped with a log line.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::RwLock;

use live_proto::game::{self, GameEnded, GameStarted, KillRecord, Player, PlayerScore, TeamScore};
use live_proto::sign::{Jwk, MAX_EVENT_SKEW_S, PublicKey, parse_time};
use live_proto::{
    AccountKey, FriendEventData, FriendRemovedData, Gamertag, PublishedFile, TitleId, Xuid, types,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::dispatch::Subscriber;
use crate::guard::ReplayGuard;
use crate::store::RecordedEvent;

#[derive(Debug, Clone, PartialEq)]
pub struct Account {
    pub xuid: Xuid,
    pub gamertag: Gamertag,
    pub created_at: String,
    pub position: u64,
}

#[derive(Debug, Clone)]
pub struct Key {
    pub xuid: Xuid,
    pub key: PublicKey,
    pub from: u64,
    pub to: Option<u64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct File {
    pub meta: PublishedFile,
    pub position: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Presence {
    pub state: String,
    pub game_id: Option<String>,
    pub activity: Option<String>,
    /// The console's build and capabilities ([`live_proto::Presence::build`]);
    /// public, unlike the game id and activity.
    pub build: Option<String>,
    pub capabilities: Vec<String>,
    pub since: String,
    /// Unix seconds of the last presence event or ping; the presence
    /// manager expires it a TTL later.
    pub last_seen: i64,
    /// The friends-only and invite-only sessions the player was in when it
    /// set this presence: its game id and activity show only to readers who
    /// may see them ([`crate::sessions::presence_detail_shows`]).
    pub private_sessions: Vec<String>,
}

/// One account's friendship relations, projected from the friend events on
/// the account streams: mutual friends and pending requests in and out.
/// Each entry pairs the other account's XUID with the event's `at`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Relations {
    pub friends: Vec<(Xuid, String)>,
    pub incoming: Vec<(Xuid, String)>,
    pub outgoing: Vec<(Xuid, String)>,
}

/// How many kills a game keeps for `GET /v1/games/{id}`.
pub const KEPT_KILLS: usize = 50;

#[derive(Debug, Clone, PartialEq)]
pub struct Game {
    pub started: GameStarted,
    pub started_at: String,
    pub position: u64,
    pub players: Vec<Player>,
    /// Everyone who was ever in the game (for `recent_games`).
    pub xuids: Vec<Xuid>,
    pub scoreboard: Vec<PlayerScore>,
    pub teams: Option<Vec<TeamScore>>,
    pub kills: VecDeque<KillRecord>,
    pub last_tick: Option<u64>,
    pub ended: Option<(GameEnded, String)>,
    /// Unix seconds of the last event or frame; idle games are ended.
    pub last_activity: i64,
    /// A Live co-op campaign game's session, recorded when it starts: a
    /// friends-only or invite-only one hides the game from strangers.
    pub coop: Option<crate::sessions::CoopLink>,
}

impl Game {
    pub fn summary(&self) -> game::GameSummary {
        let g = &self.started.game;
        game::GameSummary {
            game_id: self.started.game_id.clone(),
            map: g.map.clone(),
            mode: g.mode.clone(),
            kind: g.kind().into(),
            variant: g.variant.clone(),
            playlist: g.playlist.clone(),
            playlist_id: g.playlist_id.clone(),
            difficulty: g.difficulty.clone(),
            online: g.online,
            team_play: g.team_play,
            state: if self.ended.is_some() {
                "ended"
            } else {
                "live"
            }
            .into(),
            started_at: self.started_at.clone(),
            ended_at: self.ended.as_ref().map(|e| e.1.clone()),
            players: self.players.clone(),
            teams: self.teams.clone(),
            map_id: g.map_id,
            skulls: g.skulls.clone(),
        }
    }

    /// Whether `viewer` may see this game: every game but a Live co-op
    /// one from a friends-only or invite-only session, which shows only to
    /// those who were in it and (friends-only) the host's friends.
    pub fn shows_to(
        &self,
        viewer: crate::sessions::Viewer,
        friends: impl Fn(Xuid, Xuid) -> bool,
    ) -> bool {
        let Some(c) = &self.coop else { return true };
        crate::sessions::finished_shows_to(
            c.visibility,
            c.host_xuid,
            |x| self.xuids.contains(&x),
            viewer,
            friends,
        )
    }

    /// May `author` write to this game: its host, or a player's account.
    pub fn admits(&self, author: &live_proto::Author) -> bool {
        if author.to_string() == self.started.host {
            return true;
        }
        matches!(author, live_proto::Author::Account { xuid, .. } if self.xuids.contains(xuid))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Outline {
    pub sha256: String,
    pub position: u64,
}

#[derive(Default)]
pub struct State {
    pub accounts: HashMap<Xuid, Account>,
    by_tag: HashMap<String, Xuid>,
    /// Creation position -> account, for paging in creation order.
    by_position: BTreeMap<u64, Xuid>,
    pub keys: HashMap<String, Key>,
    pub files: HashMap<(TitleId, String), File>,
    pub presence: HashMap<Xuid, Presence>,
    /// Friendship relations per account (docs/live/live-api.md, Friends). Only
    /// accounts that ever sent or received a request have an entry.
    pub friends: HashMap<Xuid, Relations>,
    pub teams: live_proto::team::Store,
    pub sessions: BTreeMap<String, live_proto::session::Session>,
    /// Co-op sessions' guests so far, in join order with the join time:
    /// the live roster forgets a player who left. Dropped when the session
    /// closes.
    pub coop_joined: HashMap<String, Vec<(Xuid, String)>>,
    /// Finished co-op games by the log position of the event that closed
    /// the session (`GET /v1/coop/games`, newest first by that position).
    pub coop_games: BTreeMap<u64, crate::sessions::CoopGame>,
    pub mailboxes: BTreeMap<Xuid, BTreeMap<u64, live_proto::mailbox::Invite>>,
    pub games: HashMap<String, Game>,
    /// Start position -> game id, oldest first.
    games_by_position: BTreeMap<u64, String>,
    pub outlines: HashMap<String, Outline>,
    /// For each `SignedIn`/`PresenceChanged` event (by log position) set
    /// while its player was in a friends-only or invite-only session: those
    /// sessions, so the log can hide the event's game id and activity from
    /// readers who may not see them.
    pub presence_context: HashMap<u64, Vec<String>>,
    /// Per-playlist ranks, from the `rank-*` streams (crate::ranks).
    pub ranks: crate::ranks::Ranks,
    /// Arbitration rounds, from the `round-*` streams (crate::arbitration).
    pub arbitration: crate::arbitration::Rounds,
    /// Bot accounts and rosters, from the `bot-*` and `bots-*` streams
    /// (crate::bots).
    pub bots: crate::bots::Bots,
}

impl State {
    pub fn account_by_tag(&self, tag: &Gamertag) -> Option<&Account> {
        self.by_tag
            .get(&tag.key())
            .and_then(|x| self.accounts.get(x))
    }

    /// Accounts created after `after`, in creation order.
    pub fn accounts_after(&self, after: u64) -> impl Iterator<Item = &Account> {
        self.by_position
            .range(after + 1..)
            .filter_map(|(_, x)| self.accounts.get(x))
    }

    /// An account's keys, oldest first.
    pub fn keys_of(&self, xuid: Xuid) -> Vec<AccountKey> {
        let mut keys: Vec<_> = self
            .keys
            .values()
            .filter(|k| k.xuid == xuid)
            .map(|k| AccountKey {
                kid: k.key.kid().into(),
                jwk: k.key.jwk(),
                valid_from: k.from,
                valid_to: k.to,
            })
            .collect();
        keys.sort_by_key(|k| k.valid_from);
        keys
    }

    /// The current key `kid` of account `xuid`.
    pub fn active_key(&self, xuid: Xuid, kid: &str) -> Option<&PublicKey> {
        self.keys
            .get(kid)
            .filter(|k| k.xuid == xuid && k.to.is_none())
            .map(|k| &k.key)
    }

    /// Games newest first, optionally only live or ended ones.
    pub fn games_newest(&self) -> impl Iterator<Item = &Game> {
        self.games_by_position
            .values()
            .rev()
            .filter_map(|id| self.games.get(id))
    }

    pub fn active_key_count(&self, xuid: Xuid) -> usize {
        self.keys
            .values()
            .filter(|k| k.xuid == xuid && k.to.is_none())
            .count()
    }

    /// An account's friendship relations; empty for one that never had any.
    pub fn relations_of(&self, xuid: Xuid) -> Relations {
        self.friends.get(&xuid).cloned().unwrap_or_default()
    }

    /// Whether `a` and `b` are friends (mutual, so one lookup is enough).
    pub fn is_friends(&self, a: Xuid, b: Xuid) -> bool {
        self.friends
            .get(&a)
            .is_some_and(|r| r.friends.iter().any(|(x, _)| *x == b))
    }

    /// Whether a request `from -> to` is pending.
    pub fn pending(&self, from: Xuid, to: Xuid) -> bool {
        self.friends
            .get(&from)
            .is_some_and(|r| r.outgoing.iter().any(|(x, _)| *x == to))
    }
}

/// The read model plus the replay sets the log seeds: event nonces still
/// inside the `at` window, and used invites.
#[derive(Default)]
pub struct Model {
    state: RwLock<State>,
    pub nonces: ReplayGuard,
}

#[derive(Deserialize)]
struct AccountCreated {
    xuid: Xuid,
    gamertag: Gamertag,
    #[serde(default)]
    invite: Option<String>,
}

#[derive(Deserialize)]
struct KeyAdded {
    jwk: Jwk,
}

#[derive(Deserialize)]
struct KeyRevoked {
    kid: String,
}

use live_proto::PresenceData;

fn data<T: DeserializeOwned>(e: &RecordedEvent) -> Option<T> {
    match serde_json::from_value(e.body.data.clone()) {
        Ok(d) => Some(d),
        Err(err) => {
            eprintln!("live: skipping {} at {}: {err}", e.body.kind, e.position);
            None
        }
    }
}

/// The xuid of an `account-<xuid>` stream.
fn account_of(stream: &str) -> Option<Xuid> {
    Xuid::parse(stream.strip_prefix("account-")?).ok()
}

pub fn nonce_key(author: &str, nonce: &str) -> String {
    format!("nonce {author} {nonce}")
}

pub fn invite_key(jti: &str) -> String {
    format!("invite {jti}")
}

impl Model {
    pub fn read(&self) -> std::sync::RwLockReadGuard<'_, State> {
        self.state.read().unwrap()
    }

    /// A keep-alive: the account's presence lives on. False if it has none.
    pub fn touch_presence(&self, xuid: Xuid, now: i64) -> bool {
        let mut s = self.state.write().unwrap();
        s.presence
            .get_mut(&xuid)
            .map(|p| p.last_seen = p.last_seen.max(now))
            .is_some()
    }

    pub fn touch_host_sessions(&self, xuid: Xuid, now: i64) {
        for item in self.state.write().unwrap().sessions.values_mut() {
            if item.host_xuid == xuid && item.state == "open" {
                item.last_activity = item.last_activity.max(now);
            }
        }
    }

    /// Notes activity in game `id` at unix second `now`. Frames come 15 a
    /// second and the clock moves once a second: the write lock is taken
    /// only when it moves.
    pub fn touch_game(&self, id: &str, now: i64) {
        let behind = |s: &State| s.games.get(id).is_some_and(|g| g.last_activity < now);
        if !behind(&self.state.read().unwrap()) {
            return;
        }
        if let Some(g) = self.state.write().unwrap().games.get_mut(id) {
            g.last_activity = g.last_activity.max(now);
        }
    }

    fn apply_game(s: &mut State, e: &RecordedEvent, at: i64) {
        let b = &e.body;
        if b.kind == game::GAME_STARTED {
            let Some(started) = data::<GameStarted>(e) else {
                return;
            };
            let id = started.game_id.clone();
            let players = started.game.players.clone();
            let xuids: Vec<Xuid> = players.iter().filter_map(|p| p.xuid).collect();
            let coop = (started.game.kind() == "campaign" && started.game.online == Some(true))
                .then(|| {
                    let mut who = xuids.clone();
                    if let Some(live_proto::Author::Account { xuid, .. }) =
                        live_proto::Author::parse(&started.host)
                    {
                        who.push(xuid);
                    }
                    crate::sessions::CoopLink::find(s, &who)
                })
                .flatten();
            let game = Game {
                xuids,
                coop,
                players,
                started,
                started_at: b.at.clone(),
                position: e.position,
                scoreboard: Vec::new(),
                teams: None,
                kills: VecDeque::new(),
                last_tick: None,
                ended: None,
                last_activity: at,
            };
            s.games_by_position.insert(e.position, id.clone());
            s.games.insert(id, game);
            return;
        }
        let Some(id) = b.stream.strip_prefix("game-") else {
            return;
        };
        let Some(g) = s.games.get_mut(id) else { return };
        g.last_activity = g.last_activity.max(at);
        match b.kind.as_str() {
            game::PLAYER_JOINED => {
                let Some(d) = data::<game::PlayerJoined>(e) else {
                    return;
                };
                if let Some(x) = d.player.xuid.filter(|x| !g.xuids.contains(x)) {
                    g.xuids.push(x);
                }
                g.players.push(d.player);
            }
            game::PLAYER_LEFT => {
                let Some(d) = data::<game::PlayerLeft>(e) else {
                    return;
                };
                g.players.retain(|p| p.index != d.index);
            }
            game::PLAYER_KILLED => {
                let Some(kill) = data::<game::PlayerKilled>(e) else {
                    return;
                };
                g.last_tick = Some(kill.tick);
                if g.kills.len() == KEPT_KILLS {
                    g.kills.pop_front();
                }
                g.kills.push_back(KillRecord {
                    kill,
                    position: e.position,
                    at: b.at.clone(),
                });
            }
            game::SCORE_CHANGED => {
                let Some(d) = data::<game::ScoreChanged>(e) else {
                    return;
                };
                g.last_tick = Some(d.tick);
                g.scoreboard = d.players;
                if d.teams.is_some() {
                    g.teams = d.teams;
                }
            }
            game::GAME_FRAME => {
                if let Some(tick) = b.data.get("tick").and_then(|t| t.as_u64()) {
                    g.last_tick = Some(tick);
                }
            }
            game::GAME_ENDED => {
                let Some(d) = data::<GameEnded>(e) else {
                    return;
                };
                g.last_tick = Some(d.tick);
                if !d.players.is_empty() {
                    g.scoreboard = d.players.clone();
                }
                g.ended = Some((d, b.at.clone()));
            }
            _ => {}
        }
    }

    fn apply_event(&self, e: &RecordedEvent) {
        let b = &e.body;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let at = parse_time(&b.at).map_or(now, |t| t.unix_timestamp());
        self.nonces
            .insert_new(nonce_key(&b.author, &b.nonce), at + MAX_EVENT_SKEW_S, now);
        let mut s = self.state.write().unwrap();
        if b.stream.starts_with("mailbox-") {
            crate::mailbox::project(&mut s, e, at);
            return;
        }
        if b.stream.starts_with("session-") {
            crate::sessions::project(&mut s, e, at);
            return;
        }
        if b.stream.starts_with("rank-") {
            crate::ranks::project(&mut s, e);
            return;
        }
        if b.stream.starts_with("round-") {
            crate::arbitration::project(&mut s, e);
            return;
        }
        if b.stream.starts_with("bot-") || b.stream.starts_with("bots-") {
            crate::bots::project(&mut s, e);
            return;
        }
        if b.stream.starts_with("game-") {
            Self::apply_game(&mut s, e, at);
            return;
        }
        if live_proto::team::Action::is_kind(&b.kind) {
            crate::teams::project(&mut s, e);
            return;
        }
        match b.kind.as_str() {
            types::ACCOUNT_CREATED => {
                let Some(d) = data::<AccountCreated>(e) else {
                    return;
                };
                if let Some(jti) = d.invite {
                    self.nonces.insert_new(invite_key(&jti), i64::MAX, 0);
                }
                s.by_tag.insert(d.gamertag.key(), d.xuid);
                s.by_position.insert(e.position, d.xuid);
                let account = Account {
                    xuid: d.xuid,
                    gamertag: d.gamertag,
                    created_at: b.at.clone(),
                    position: e.position,
                };
                s.accounts.insert(d.xuid, account);
            }
            types::KEY_ADDED => {
                let (Some(xuid), Some(d)) = (account_of(&b.stream), data::<KeyAdded>(e)) else {
                    return;
                };
                let Ok(key) = PublicKey::from_jwk(&d.jwk) else {
                    return;
                };
                s.keys.entry(key.kid().to_string()).or_insert(Key {
                    xuid,
                    key,
                    from: e.position,
                    to: None,
                });
            }
            types::KEY_REVOKED => {
                let (Some(xuid), Some(d)) = (account_of(&b.stream), data::<KeyRevoked>(e)) else {
                    return;
                };
                if let Some(k) = s
                    .keys
                    .get_mut(&d.kid)
                    .filter(|k| k.xuid == xuid && k.to.is_none())
                {
                    k.to = Some(e.position);
                }
            }
            types::TITLE_FILE_PUBLISHED => {
                let Some(meta) = data::<PublishedFile>(e) else {
                    return;
                };
                s.files.insert(
                    (meta.title_id, meta.name.clone()),
                    File {
                        meta,
                        position: e.position,
                    },
                );
            }
            types::SIGNED_IN | types::PRESENCE_CHANGED => {
                let Some(d) = data::<PresenceData>(e) else {
                    return;
                };
                let since = match s.presence.get(&d.xuid) {
                    Some(p) => p.since.clone(),
                    None => b.at.clone(),
                };
                let state = d.state.unwrap_or_else(|| "online".into());
                let private_sessions = crate::sessions::private_sessions_of(&s, d.xuid);
                if !private_sessions.is_empty() {
                    s.presence_context
                        .insert(e.position, private_sessions.clone());
                }
                s.presence.insert(
                    d.xuid,
                    Presence {
                        state,
                        game_id: d.game_id,
                        activity: d.activity,
                        build: d.build,
                        capabilities: d.capabilities,
                        since,
                        last_seen: at,
                        private_sessions,
                    },
                );
            }
            types::SIGNED_OUT | types::PRESENCE_EXPIRED => {
                let Some(d) = data::<PresenceData>(e) else {
                    return;
                };
                s.presence.remove(&d.xuid);
            }
            game::MAP_OUTLINE_STORED => {
                #[derive(Deserialize)]
                struct Stored {
                    map: String,
                    sha256: String,
                }
                let Some(d) = data::<Stored>(e) else { return };
                s.outlines.insert(
                    d.map,
                    Outline {
                        sha256: d.sha256,
                        position: e.position,
                    },
                );
            }
            types::FRIEND_REQUESTED => {
                let Some(d) = data::<FriendEventData>(e) else {
                    return;
                };
                s.friends
                    .entry(d.from)
                    .or_default()
                    .outgoing
                    .push((d.to, b.at.clone()));
                s.friends
                    .entry(d.to)
                    .or_default()
                    .incoming
                    .push((d.from, b.at.clone()));
            }
            types::FRIEND_ACCEPTED => {
                let Some(d) = data::<FriendEventData>(e) else {
                    return;
                };
                drop_pending(&mut s, d.from, d.to);
                s.friends
                    .entry(d.from)
                    .or_default()
                    .friends
                    .push((d.to, b.at.clone()));
                s.friends
                    .entry(d.to)
                    .or_default()
                    .friends
                    .push((d.from, b.at.clone()));
            }
            types::FRIEND_DECLINED => {
                let Some(d) = data::<FriendEventData>(e) else {
                    return;
                };
                drop_pending(&mut s, d.from, d.to);
            }
            types::FRIEND_REMOVED => {
                let Some(d) = data::<FriendRemovedData>(e) else {
                    return;
                };
                drop_pending(&mut s, d.by, d.friend);
                drop_pending(&mut s, d.friend, d.by);
                if let Some(r) = s.friends.get_mut(&d.by) {
                    r.friends.retain(|(x, _)| *x != d.friend);
                }
                if let Some(r) = s.friends.get_mut(&d.friend) {
                    r.friends.retain(|(x, _)| *x != d.by);
                }
            }
            _ => {}
        }
    }
}

/// Takes the pending request `from -> to`, if there is one, off both sides.
fn drop_pending(s: &mut State, from: Xuid, to: Xuid) {
    if let Some(r) = s.friends.get_mut(&from) {
        r.outgoing.retain(|(x, _)| *x != to);
    }
    if let Some(r) = s.friends.get_mut(&to) {
        r.incoming.retain(|(x, _)| *x != from);
    }
}

impl Subscriber for Model {
    fn name(&self) -> &str {
        "read-model"
    }

    fn apply(&self, event: &RecordedEvent) {
        self.apply_event(event);
    }
}

#[cfg(test)]
mod tests {
    use live_proto::sign::{EventBody, KeyPair};
    use serde_json::{Value, json};

    use super::*;

    struct Log {
        key: KeyPair,
        position: u64,
        model: Model,
    }

    impl Log {
        fn new() -> Self {
            Log {
                key: KeyPair::generate(),
                position: 0,
                model: Model::default(),
            }
        }
        fn push(&mut self, stream: &str, kind: &str, data: Value) -> u64 {
            self.position += 1;
            let body = EventBody::new(
                &self.key,
                "server:h",
                stream,
                kind,
                data,
                time::OffsetDateTime::now_utc(),
            );
            let e = RecordedEvent {
                position: self.position,
                version: 0,
                hash: String::new(),
                jws: body.sign(&self.key).into(),
                jws_index: None,
                body,
            };
            self.model.apply(&e);
            self.position
        }
    }

    const X: &str = "0009000000000001";

    #[test]
    fn accounts_keys_files_presence() {
        let mut log = Log::new();
        let k1 = KeyPair::generate();
        let k2 = KeyPair::generate();
        let created = log.push(
            &format!("account-{X}"),
            "AccountCreated",
            json!({"xuid": X, "gamertag": "NoIcE", "invite": "inv1"}),
        );
        let added = log.push(
            &format!("account-{X}"),
            "KeyAdded",
            json!({"kid": k1.kid(), "jwk": k1.public().jwk()}),
        );
        let added2 = log.push(
            &format!("account-{X}"),
            "KeyAdded",
            json!({"kid": k2.kid(), "jwk": k2.public().jwk()}),
        );
        let revoked = log.push(
            &format!("account-{X}"),
            "KeyRevoked",
            json!({"kid": k1.kid()}),
        );
        // A revocation on another account's stream does nothing.
        log.push(
            "account-0009000000000002",
            "KeyRevoked",
            json!({"kid": k2.kid()}),
        );
        // Malformed data is skipped.
        log.push(&format!("account-{X}"), "KeyAdded", json!({"jwk": 7}));

        let xuid = Xuid::parse(X).unwrap();
        {
            let s = log.model.read();
            let a = s.account_by_tag(&Gamertag::new("noice").unwrap()).unwrap();
            assert_eq!(
                (a.xuid, a.position, a.gamertag.as_str()),
                (xuid, created, "NoIcE")
            );
            let keys = s.keys_of(xuid);
            assert_eq!(keys.len(), 2);
            assert_eq!(
                (keys[0].valid_from, keys[0].valid_to),
                (added, Some(revoked))
            );
            assert_eq!((keys[1].valid_from, keys[1].valid_to), (added2, None));
            assert!(s.active_key(xuid, k1.kid()).is_none());
            assert!(s.active_key(xuid, k2.kid()).is_some());
            assert!(s.active_key(Xuid(5), k2.kid()).is_none());
            assert_eq!(s.accounts_after(0).count(), 1);
            assert_eq!(s.accounts_after(created).count(), 0);
        }
        assert!(log.model.nonces.contains(&invite_key("inv1"), 0));

        let t = "4D530064";
        log.push(
            &format!("title-{t}-file-motd"),
            "TitleFilePublished",
            json!({"title_id": t, "name": "motd", "sha256": "aa", "size": 1}),
        );
        log.push(
            &format!("title-{t}-file-motd"),
            "TitleFilePublished",
            json!({"title_id": t, "name": "motd", "sha256": "bb", "size": 2}),
        );
        assert_eq!(
            log.model.read().files[&(live_proto::HALO_2_TITLE_ID, "motd".into())]
                .meta
                .sha256,
            "bb"
        );

        log.push(
            &format!("presence-{X}"),
            "SignedIn",
            json!({"xuid": X, "state": "online"}),
        );
        log.push(
            &format!("presence-{X}"),
            "PresenceChanged",
            json!({"xuid": X, "state": "playing", "game_id": "g"}),
        );
        {
            let s = log.model.read();
            let p = &s.presence[&xuid];
            assert_eq!(
                (p.state.as_str(), p.game_id.as_deref()),
                ("playing", Some("g"))
            );
        }
        log.push(&format!("presence-{X}"), "SignedOut", json!({"xuid": X}));
        assert!(log.model.read().presence.is_empty());
    }

    #[test]
    fn friendships_project_from_the_log() {
        let x1 = Xuid::parse("0009000000000001").unwrap();
        let x2 = Xuid::parse("0009000000000002").unwrap();
        let x3 = Xuid::parse("0009000000000003").unwrap();
        let mut log = Log::new();
        for (x, tag) in [(x1, "Alpha"), (x2, "Bravo"), (x3, "Charlie")] {
            log.push(
                &format!("account-{x}"),
                "AccountCreated",
                json!({"xuid": x.to_string(), "gamertag": tag}),
            );
        }
        log.push(
            &format!("account-{x1}"),
            types::FRIEND_REQUESTED,
            serde_json::to_value(FriendEventData { from: x1, to: x2 }).unwrap(),
        );
        {
            let s = log.model.read();
            let a = s.relations_of(x1);
            let b = s.relations_of(x2);
            assert_eq!(a.outgoing.iter().map(|(x, _)| *x).collect::<Vec<_>>(), [x2]);
            assert_eq!(b.incoming.iter().map(|(x, _)| *x).collect::<Vec<_>>(), [x1]);
            assert!(s.pending(x1, x2));
            assert!(!s.pending(x2, x1));
            assert!(!s.is_friends(x1, x2));
            // An account with no relations reads as empty, not missing.
            assert_eq!(s.relations_of(x3), Relations::default());
        }
        log.push(
            &format!("account-{x2}"),
            types::FRIEND_ACCEPTED,
            serde_json::to_value(FriendEventData { from: x1, to: x2 }).unwrap(),
        );
        {
            let s = log.model.read();
            assert!(s.is_friends(x1, x2) && s.is_friends(x2, x1));
            assert!(!s.pending(x1, x2));
            assert!(s.relations_of(x1).outgoing.is_empty());
            assert!(s.relations_of(x2).incoming.is_empty());
            assert_eq!(
                s.relations_of(x2)
                    .friends
                    .iter()
                    .map(|(x, _)| *x)
                    .collect::<Vec<_>>(),
                [x1]
            );
        }
        // Declined: the pending request is gone, no friendship.
        log.push(
            &format!("account-{x1}"),
            types::FRIEND_REQUESTED,
            serde_json::to_value(FriendEventData { from: x1, to: x3 }).unwrap(),
        );
        log.push(
            &format!("account-{x3}"),
            types::FRIEND_DECLINED,
            serde_json::to_value(FriendEventData { from: x1, to: x3 }).unwrap(),
        );
        {
            let s = log.model.read();
            assert!(!s.pending(x1, x3));
            assert!(s.relations_of(x1).outgoing.is_empty());
            assert!(s.relations_of(x3).incoming.is_empty());
        }
        // Removed by either side: the friendship ends both ways.
        log.push(
            &format!("account-{x2}"),
            types::FRIEND_REMOVED,
            serde_json::to_value(FriendRemovedData { by: x2, friend: x1 }).unwrap(),
        );
        {
            let s = log.model.read();
            assert!(!s.is_friends(x1, x2) && !s.is_friends(x2, x1));
        }
        // A pending request can be cancelled by its sender just the same.
        log.push(
            &format!("account-{x1}"),
            types::FRIEND_REQUESTED,
            serde_json::to_value(FriendEventData { from: x1, to: x3 }).unwrap(),
        );
        log.push(
            &format!("account-{x1}"),
            types::FRIEND_REMOVED,
            serde_json::to_value(FriendRemovedData { by: x1, friend: x3 }).unwrap(),
        );
        assert!(!log.model.read().pending(x1, x3));
    }
}
