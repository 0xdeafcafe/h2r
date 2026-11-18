//! `sim.*`: simulated players and direct game state, for testing co-op,
//! multiplayer and Live flows from one running game (docs/runtime/agent-api.md,
//! "sim").
//!
//! - The wire types of `sim.player.*`, `sim.state.set`, `sim.view`,
//!   `sim.expect` and `sim.session` (here).
//! - `sim.view`: the game as one JSON document ([`ROOTS`]), which
//!   `h2 agent run`'s `expect` lines and `sim.expect` check in the same
//!   [`crate::expect`] format.
//! - [`until`]: polls `sim.expect` until it passes (`h2 agent sim until`).
//! - [`menu`]: the lobby steps (`h2 agent sim lobby`): presses checked
//!   against `ui.screen`, with the menu's screen ids and rows (shared with
//!   `h2 coop test --drive`).
//!
//! Scripts are `h2 agent run` scripts whose lines may be `sim ...` verbs.
//! Nothing here has game dependencies.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::expect::Expect;
use crate::{PadStep, RpcError, code};

pub mod menu;

/// Method names. They are host methods: `session.methods` lists them where
/// the game registered them (`agent::sim::install`).
pub mod method {
    /// Add a simulated player ([`super::AddParams`]).
    pub const PLAYER_ADD: &str = "sim.player.add";
    /// Remove one, or all ([`super::RemoveParams`]).
    pub const PLAYER_REMOVE: &str = "sim.player.remove";
    /// The simulated players, and the game's players.
    pub const PLAYER_LIST: &str = "sim.player.list";
    /// Give a simulated player scripted pad steps ([`super::DriveParams`]).
    pub const PLAYER_PAD: &str = "sim.player.pad";
    /// Write a player's state ([`super::SetParams`]): host only, never in a
    /// co-op (synchronous) session.
    pub const STATE_SET: &str = "sim.state.set";
    /// The game as one JSON document ([`super::ViewParams`], [`super::ROOTS`]).
    pub const VIEW: &str = "sim.view";
    /// Judge expectations ([`super::ExpectParams`]) against `sim.view`, all
    /// read at one tick.
    pub const EXPECT: &str = "sim.expect";
    /// What kind of session the game is in ([`super::SessionView`]).
    pub const SESSION: &str = "sim.session";

    pub const ALL: &[&str] = &[
        PLAYER_ADD,
        PLAYER_REMOVE,
        PLAYER_LIST,
        PLAYER_PAD,
        STATE_SET,
        VIEW,
        EXPECT,
        SESSION,
    ];
}

/// How a simulated player reaches the game (docs/game/bots.md §1).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerKind {
    /// A pad on a free local controller port (bots.md stage 1).
    Local,
    /// A remote player with no machine, made on the host (bots.md stage 6).
    Phantom,
}

/// Where a simulated player's pad comes from (bots.md's `PadSource`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceKind {
    /// Pad steps given with the player (`steps`) or later (`sim.player.pad`).
    Script,
    /// An input log in the cache directory's `inputs/` (`recording`).
    Recording,
    /// The bot brain.
    Brain,
}

/// A server-issued identity for a phantom: the local embedded Live server
/// issues it, and accepts `bot: true` only for xuids it issued. Sim never
/// makes one.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Identity {
    pub xuid: String,
    pub gamertag: String,
}

/// `sim.player.add`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AddParams {
    pub kind: PlayerKind,
    pub source: SourceKind,
    /// The team to put it on (multiplayer team index).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    /// `script`: the steps it starts with.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steps: Vec<PadStep>,
    /// `recording`: the input log's name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording: Option<String>,
    /// `local`: the controller port (default: the lowest free one after
    /// the real pads).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u32>,
    /// `phantom`: its server-issued identity (required).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
}

/// One simulated player, as `sim.player.add` and `sim.player.list` give it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimPlayer {
    /// `sim.<n>`, never reused while the game runs.
    pub id: String,
    pub kind: PlayerKind,
    pub source: SourceKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    /// `local`: its controller port.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u32>,
    /// `phantom`: its xuid and gamertag.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub identity: Option<Identity>,
    /// The game's player it became (`player.<index>`), once the game lists
    /// one for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub player: Option<String>,
}

/// `sim.player.remove`: one by id, or `all`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RemoveParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub all: bool,
}

/// `sim.player.pad`: scripted steps for a simulated player (its source
/// becomes `script`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DriveParams {
    pub id: String,
    pub steps: Vec<PadStep>,
    /// Queue after what's queued (default: replace it).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub append: bool,
}

/// Grenades carried, by type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Grenades {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frag: Option<i8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub plasma: Option<i8>,
}

/// The held weapon's first magazine.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Ammo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loaded: Option<i16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reserve: Option<i16>,
}

/// `sim.state.set`: writes to one player's record and unit, at a tick
/// boundary. Only the fields given are written.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetParams {
    /// `player.<index>`, or a simulated player's `sim.<n>`.
    pub player: String,
    /// Teleport the unit here (world units); its velocity is zeroed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 3]>,
    /// The player's team and its unit's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<i32>,
    /// Body vitality, 0 to 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health: Option<f64>,
    /// Shield vitality, 0 to 1 (more is overshield, up to 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shield: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grenades: Option<Grenades>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ammo: Option<Ammo>,
}

/// What kind of session the game is in, from the simulation mode
/// (`docs/live/live-coop.md` 1.1) and the squad session.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    /// No game simulation (the menus).
    None,
    /// One machine: campaign, split screen.
    Local,
    /// The host of a synchronous (co-op, lockstep) game.
    CoopHost,
    /// A guest in a synchronous game.
    CoopGuest,
    /// The host of a distributed (multiplayer, host-authoritative) game.
    MpHost,
    /// A client in a distributed game.
    MpClient,
    /// The game didn't say (no session probe, or the read failed).
    #[default]
    Unknown,
}

impl SessionKind {
    /// From the simulation mode word: 0 none, 1 local, 2 sync-client,
    /// 3 sync-server, 4 dist-client, 5 dist-server.
    pub fn from_sim_mode(mode: u32) -> SessionKind {
        match mode {
            0 => SessionKind::None,
            1 => SessionKind::Local,
            2 => SessionKind::CoopGuest,
            3 => SessionKind::CoopHost,
            4 => SessionKind::MpClient,
            5 => SessionKind::MpHost,
            _ => SessionKind::Unknown,
        }
    }

    /// Whether a host write keeps the session sound. A synchronous game
    /// runs the same simulation on every machine and compares hashes: a
    /// write on one desyncs the rest. A distributed client's writes are
    /// overwritten by its host. An unknown session is refused rather than
    /// guessed.
    pub fn writes(self) -> Result<(), &'static str> {
        match self {
            SessionKind::None | SessionKind::Local | SessionKind::MpHost => Ok(()),
            SessionKind::CoopHost | SessionKind::CoopGuest => Err(
                "a co-op (synchronous) session: a host write desyncs the guests and trips the sim hash; \
                 drive the player with pads instead (sim.player.pad, input.pad)",
            ),
            SessionKind::MpClient => Err(
                "a multiplayer client: the host owns the game state and overwrites a client's writes; \
                 run sim.state.set on the host",
            ),
            SessionKind::Unknown => Err(
                "the session kind is unknown (no session probe, or it couldn't be read), so a write \
                 can't be shown not to desync a co-op session",
            ),
        }
    }
}

/// `sim.session`'s answer, and the `session` root of `sim.expect`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionView {
    pub kind: SessionKind,
    /// The raw simulation mode, when read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim_mode: Option<u32>,
    /// The squad session, when there is one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub peers: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub players: Option<u32>,
    /// `local`, `synchronous` or `distributed`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub campaign: Option<bool>,
    /// Whether `sim.state.set` may write now.
    pub writes: bool,
    /// Why not, when it may not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub why: Option<String>,
}

impl SessionView {
    /// Fills `writes` and `why` from `kind`.
    pub fn settle(mut self) -> SessionView {
        match self.kind.writes() {
            Ok(()) => {
                self.writes = true;
                self.why = None;
            }
            Err(why) => {
                self.writes = false;
                self.why = Some(why.into());
            }
        }
        self
    }
}

/// `sim.view`'s params: which roots to read (default [`DEFAULT_ROOTS`]:
/// all but `object`, which lists every object in the world).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roots: Option<Vec<String>>,
}

/// `sim.expect`'s params: expectations as `h2 agent run` writes them
/// (`"/player/1/team == 1"`, or `{"pointer", "op", "want"}`), with
/// pointers into `sim.view`'s document.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExpectParams {
    pub checks: Vec<Expect>,
}

/// The roots of `sim.view`'s document:
///
/// | root | what | pointer |
/// |---|---|---|
/// | `at` | the stamp ([`crate::At`]) | `/at/phase` |
/// | `session` | [`SessionView`] | `/session/kind` |
/// | `ui` | `ui.screen`'s answer, `null` without one | `/ui/screen` |
/// | `player` | players by index: the `world` object plus `health`, `shield`, `grenades`, `ammo` | `/player/1/team` |
/// | `object` | every `world` object by id | `/object/vehicle.e2000003/occupied` |
/// | `count` | objects by kind, `team` by team, `total` | `/count/player`, `/count/team/1` |
/// | `sim` | simulated players by the `<n>` of `sim.<n>`, and `count` | `/sim/1/player` |
/// | `input` | `input.state`'s answer | `/input/pad/active` |
pub const ROOTS: &[&str] = &[
    "at", "session", "ui", "player", "object", "count", "sim", "input",
];

/// The roots read when none are named.
pub const DEFAULT_ROOTS: &[&str] = &["at", "session", "ui", "player", "count", "sim", "input"];

/// The roots a set of expectations points into (an empty pointer: all).
pub fn roots(checks: &[Expect]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for c in checks {
        match c.pointer.split('/').nth(1) {
            None => return ROOTS.iter().map(|r| (*r).into()).collect(),
            Some(r) => v.push(r.into()),
        }
    }
    v.sort_unstable();
    v.dedup();
    v
}

/// Checks root names against [`ROOTS`].
pub fn check_roots(roots: &[String]) -> Result<(), RpcError> {
    match roots.iter().find(|r| !ROOTS.contains(&r.as_str())) {
        Some(r) => Err(RpcError::new(
            code::INVALID_PARAMS,
            format!("no root {r:?}; roots are {}", ROOTS.join(" ")),
        )),
        None => Ok(()),
    }
}

/// A call to the game: [`crate::Client`], or a fake in tests.
pub trait Rpc {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError>;
}

impl Rpc for crate::Client {
    fn call(&mut self, method: &str, params: Value) -> Result<Value, RpcError> {
        crate::Client::call(self, method, params).map_err(|e| match e {
            crate::CallError::Rpc(e) => e,
            e => RpcError::new(code::UNAVAILABLE, format!("{method}: {e}")),
        })
    }
}

/// `h2 agent sim until`: `sim.expect` every `every` ticks until it passes
/// or `timeout_ticks` have passed. The last verdict, with the `ticks`
/// waited; a timeout is an error whose data is that verdict.
pub fn until(
    rpc: &mut dyn Rpc,
    checks: &[Expect],
    timeout_ticks: u64,
    every: u64,
) -> Result<Value, RpcError> {
    let every = every.max(1);
    let params = serde_json::to_value(ExpectParams {
        checks: checks.to_vec(),
    })
    .expect("expectations serialise");
    let mut waited = 0;
    loop {
        let mut v = rpc.call(method::EXPECT, params.clone())?;
        v["ticks"] = json!(waited);
        if v["pass"] == true {
            return Ok(v);
        }
        if waited >= timeout_ticks {
            let mut e = RpcError::new(
                code::TIMEOUT,
                format!("the expectations didn't hold within {timeout_ticks} ticks"),
            );
            e.data = Some(v);
            return Err(e);
        }
        let step = every.min(timeout_ticks - waited);
        // Generous wall time: a loading or slow game ticks slowly.
        rpc.call(
            crate::method::WAIT,
            json!({ "until": { "ticks": step }, "timeout_ms": 5000 + step * 200 }),
        )?;
        waited += step;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_kinds_come_from_the_sim_mode() {
        let kinds: Vec<SessionKind> = (0..7).map(SessionKind::from_sim_mode).collect();
        assert_eq!(
            kinds,
            [
                SessionKind::None,
                SessionKind::Local,
                SessionKind::CoopGuest,
                SessionKind::CoopHost,
                SessionKind::MpClient,
                SessionKind::MpHost,
                SessionKind::Unknown
            ]
        );
    }

    #[test]
    fn only_local_and_multiplayer_hosts_take_writes() {
        for k in [SessionKind::None, SessionKind::Local, SessionKind::MpHost] {
            assert!(k.writes().is_ok(), "{k:?}");
        }
        for k in [
            SessionKind::CoopHost,
            SessionKind::CoopGuest,
            SessionKind::MpClient,
            SessionKind::Unknown,
        ] {
            assert!(k.writes().is_err(), "{k:?}");
        }
        let v = SessionView {
            kind: SessionKind::CoopHost,
            ..Default::default()
        }
        .settle();
        assert!(!v.writes);
        assert!(v.why.unwrap().contains("co-op"));
    }

    #[test]
    fn add_params_parse_and_refuse_strays() {
        let p: AddParams = serde_json::from_value(json!({
            "kind": "local", "source": "script", "team": 1,
            "steps": [{"buttons": ["a"], "ticks": 2}]
        }))
        .unwrap();
        assert_eq!(
            (p.kind, p.source, p.team),
            (PlayerKind::Local, SourceKind::Script, Some(1))
        );
        assert_eq!(p.steps.len(), 1);
        assert!(
            serde_json::from_value::<AddParams>(json!({"kind": "ai", "source": "brain"})).is_err()
        );
        assert!(
            serde_json::from_value::<AddParams>(
                json!({"kind": "local", "source": "brain", "bot": true})
            )
            .is_err()
        );
    }

    #[test]
    fn set_params_take_only_known_fields() {
        let p: SetParams = serde_json::from_value(json!({
            "player": "player.1", "pos": [1, 2, 3], "health": 0.5,
            "grenades": {"frag": 2}
        }))
        .unwrap();
        assert_eq!(p.pos, Some([1.0, 2.0, 3.0]));
        assert_eq!(p.grenades.unwrap().frag, Some(2));
        assert!(
            serde_json::from_value::<SetParams>(json!({"player": "player.1", "god": true}))
                .is_err()
        );
    }

    #[test]
    fn roots_come_from_the_pointers() {
        let checks: Vec<Expect> = serde_json::from_value(json!([
            "/player/0/team == 1",
            "/at/phase == \"gameplay\"",
            {"pointer": "/player/1/team", "op": "==", "want": 0}
        ]))
        .unwrap();
        assert_eq!(roots(&checks), ["at", "player"]);
        assert!(check_roots(&roots(&checks)).is_ok());
        assert!(check_roots(&["nope".into()]).is_err());
        let whole = Expect::new("", crate::expect::Op::Exists, None).unwrap();
        assert_eq!(roots(&[whole]).len(), ROOTS.len());
    }

    /// A game whose player 1 changes team after `turn` polls.
    struct Game {
        polls: u32,
        turn: u32,
        waits: Vec<Value>,
    }

    impl Rpc for Game {
        fn call(&mut self, m: &str, p: Value) -> Result<Value, RpcError> {
            match m {
                method::EXPECT => {
                    self.polls += 1;
                    let team = if self.polls > self.turn { 1 } else { 0 };
                    let view = json!({"player": {"1": {"team": team}}});
                    let p: ExpectParams = serde_json::from_value(p).unwrap();
                    Ok(crate::expect::verdict(&view, &p.checks))
                }
                crate::method::WAIT => {
                    self.waits.push(p);
                    Ok(json!({}))
                }
                _ => Err(RpcError::new(code::METHOD_NOT_FOUND, m)),
            }
        }
    }

    #[test]
    fn until_polls_and_gives_up_with_the_last_verdict() {
        let checks = [Expect::parse("/player/1/team == 1").unwrap()];
        let mut g = Game {
            polls: 0,
            turn: 2,
            waits: vec![],
        };
        let v = until(&mut g, &checks, 100, 10).unwrap();
        assert_eq!(
            (v["pass"].clone(), v["ticks"].clone()),
            (json!(true), json!(20))
        );
        assert_eq!(g.waits[0]["until"]["ticks"], 10);
        let mut g = Game {
            polls: 0,
            turn: 9,
            waits: vec![],
        };
        let e = until(&mut g, &checks, 15, 10).unwrap_err();
        assert_eq!(e.code, code::TIMEOUT);
        let last = e.data.unwrap();
        assert_eq!(last["ticks"], 15);
        assert_eq!(last["expects"][0]["got"], 0);
        assert_eq!(
            g.waits[1]["until"]["ticks"], 5,
            "the last wait stops at the timeout"
        );
    }
}
