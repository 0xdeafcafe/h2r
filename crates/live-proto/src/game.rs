//! Game streams (`game-<id>`): event data, frames and the read shapes of
//! `/v1/games` (docs/live/live-api.md, Events and Clarifications 6-10). The
//! server validates every game event against these types; clients build
//! them.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Xuid;

pub const GAME_STARTED: &str = "GameStarted";
pub const PLAYER_JOINED: &str = "PlayerJoined";
pub const PLAYER_LEFT: &str = "PlayerLeft";
pub const PLAYER_KILLED: &str = "PlayerKilled";
pub const SCORE_CHANGED: &str = "ScoreChanged";
pub const GAME_FRAME: &str = "GameFrame";
pub const GAME_ENDED: &str = "GameEnded";
/// A map outline was stored (stream `map-<map>`).
pub const MAP_OUTLINE_STORED: &str = "MapOutlineStored";

/// The game event types a player's console may append in a batch; the
/// server writes `GameStarted` (from `POST /v1/games`) and `GameFrame`.
pub const CLIENT_GAME_EVENTS: [&str; 5] = [
    PLAYER_JOINED,
    PLAYER_LEFT,
    PLAYER_KILLED,
    SCORE_CHANGED,
    GAME_ENDED,
];

/// Most players a game holds (Halo 2 allows 16).
pub const MAX_PLAYERS: usize = 16;

pub fn stream(game_id: &str) -> String {
    format!("game-{game_id}")
}

pub fn map_stream(map: &str) -> String {
    format!("map-{map}")
}

/// Game ids and map names: `[A-Za-z0-9_-]{1,64}` (they are stream names).
pub fn valid_id(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

/// A player of a game. `index` is the game's slot, stable for the game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Player {
    pub index: u32,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub xuid: Option<Xuid>,
    pub team: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub local: Option<bool>,
    /// What the player looks like, as far as the game's memory says
    /// (docs/live/live-telemetry.md, Player appearance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub appearance: Option<Appearance>,
    /// A bot (docs/game/bots.md §6, §9.2). With an `xuid`, the server
    /// accepts it only for an xuid in the session's roster. Without one it
    /// is a host's own Clanker (a local slot, or a phantom it made): no
    /// account, so no stats page, career stats or rank.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bot: Option<bool>,
}

impl Player {
    pub fn is_bot(&self) -> bool {
        self.bot == Some(true)
    }
}

/// A player's colours and emblem: `primary`..`quaternary` index the art
/// manifest's 18-colour palette, `emblem_foreground` 0..63 and
/// `emblem_background` 0..31. Every field is independently optional.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Appearance {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub secondary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tertiary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quaternary: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emblem_foreground: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub emblem_background: Option<u8>,
}

impl Appearance {
    /// True when no field is known.
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// `None` when nothing is known, so the key is left out.
    pub fn known(self) -> Option<Self> {
        (!self.is_empty()).then_some(self)
    }
}

/// Private headless simulation metadata carried by the normal game start.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SimGame {
    pub run_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub batch_id: Option<String>,
    /// Decimal u64, including seeds larger than JavaScript's exact integer range.
    pub seed: String,
    pub gametype: String,
    pub pace: String,
    /// Resolved bot specs from the match config, with inline tuning.
    pub bots: Vec<serde_json::Value>,
    /// This match's recording is sent after simulation finishes.
    #[serde(default)]
    pub deferred: bool,
}

/// Sim recordings' stable game id, also used when retrying a start.
pub fn sim_game_id(run_id: &str) -> String {
    format!("sim-{run_id}")
}

/// `POST /v1/games`: `GameStarted` data without `game_id` (or `host`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct NewGame {
    /// Present only for private admin training games.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sim: Option<SimGame>,

    #[serde(default)]
    pub console: Option<String>,
    pub map: String,
    pub mode: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub variant: Option<String>,
    pub team_play: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub score_limit: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time_limit_s: Option<i64>,
    #[serde(default)]
    pub tick: Option<u64>,
    pub players: Vec<Player>,
    /// `custom`, `matchmaking`, `clankermatch` or `campaign`; old senders
    /// leave it out and [`NewGame::kind`] infers it from `mode`. A
    /// `clankermatch` game is a matchmade game against bots the host runs
    /// (docs/game/bots.md §8.2 item 1): its own kind, played in a clanker
    /// hopper, with a playlist like `matchmaking`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Matchmaking and Clankermatch only: the playlist (hopper) name and id
    /// the game came from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_id: Option<String>,
    /// Multiplayer only: the Live session the game was played in (16
    /// lowercase hex digits, `arbitration::valid_id`). A ranked game ranks
    /// the humans who registered an arbitration round under it
    /// (docs/live/live-matchmaking.md §8.4); a game with bots names the
    /// session whose roster they came from (docs/game/bots.md §9.4), custom
    /// games too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Campaign only: `easy`, `normal`, `heroic` or `legendary`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
    /// Campaign only: the co-op partners are on other machines (Live co-op).
    /// Local split-screen co-op is more than one player without it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    /// Exactly which map (docs/game/maps.md §5): `map` stays the base map's name
    /// (what the game loads as `maps\<map>.map`), so a modded Lockout is
    /// `map: "lockout"` with this set, and stats keep the two apart by it.
    /// None for retail maps from senders that do not know it, and for every
    /// sender before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_id: Option<crate::session::MapId>,
    /// The skulls on for the game ([`crate::skulls`]): engine indices,
    /// ascending. Campaign: the skull array at the start; multiplayer: the
    /// set the host chose (only [`crate::skulls::MULTIPLAYER`] ones). Left
    /// out when none are on, and by every sender before this field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skulls: Option<Vec<u8>>,
}

/// The game kinds `NewGame.kind` may name.
pub const GAME_KINDS: [&str; 4] = ["custom", "matchmaking", "clankermatch", "campaign"];

/// Whether games of `kind` come from a hopper (carry a playlist, count
/// toward playlist stats and rank): `matchmaking` and `clankermatch`.
pub fn is_matchmade(kind: &str) -> bool {
    matches!(kind, "matchmaking" | "clankermatch")
}

/// The campaign difficulties `NewGame.difficulty` may name, easiest first.
pub const DIFFICULTIES: [&str; 4] = ["easy", "normal", "heroic", "legendary"];

impl NewGame {
    /// `kind`, or for old senders `campaign` when `mode` is, else `custom`.
    pub fn kind(&self) -> &str {
        match &self.kind {
            Some(kind) => kind,
            None if self.mode == "campaign" => "campaign",
            None => "custom",
        }
    }
}

/// `GameStarted` data, written by the server: the request plus the id it
/// chose and `host`, the author who started the game.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameStarted {
    pub game_id: String,
    pub host: String,
    #[serde(flatten)]
    pub game: NewGame,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerJoined {
    pub player: Player,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerLeft {
    pub index: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerKilled {
    pub tick: u64,
    pub victim: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub killer: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    pub kind: String,
    /// The victim's position.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pos: Option<[f64; 3]>,
}

pub const KILL_KINDS: [&str; 4] = ["kill", "suicide", "betrayal", "world"];

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerScore {
    pub index: u32,
    pub score: i64,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TeamScore {
    pub team: i32,
    pub score: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScoreChanged {
    pub tick: u64,
    pub players: Vec<PlayerScore>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub teams: Option<Vec<TeamScore>>,
}

/// `{team}` in team games, `{player}` otherwise.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Winner {
    Team { team: i32 },
    Player { player: u32 },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameEnded {
    pub tick: u64,
    pub reason: String,
    #[serde(default)]
    pub winner: Option<Winner>,
    #[serde(default)]
    pub players: Vec<PlayerScore>,
}

pub const END_REASONS: [&str; 3] = ["finished", "quit", "lost"];

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FramePlayer {
    pub index: u32,
    pub alive: bool,
    /// Where they are; while dead, where their body lies (where they fell
    /// once the game has cleared it away; the origin from older senders).
    pub pos: [f64; 3],
    pub yaw: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<String>,
    /// The player's biped, from its definition tag: `"spartan"` or
    /// `"elite"`; other bipeds pass through as a short id (readers fall back
    /// to spartan).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub species: Option<String>,
    /// Where the unit aims, in game coordinates (z up), unit length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<[f64; 3]>,
    /// The held weapon's tag file stem, lower case (`"battle_rifle"`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    /// Shots the held weapon fired since this player's previous frame; zero
    /// is sent as absent.
    #[serde(default, skip_serializing_if = "no_shots")]
    pub shots: Option<u32>,
    /// `stand`, `crouch`, `airborne` (jumping or falling), `vehicle` or
    /// `dead`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stance: Option<String>,
    /// Horizontal speed, game units a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// One-shot actions started since the previous frame, oldest first, at
    /// most [`MAX_FRAME_ACTIONS`]: see [`ACTIONS`].
    #[serde(default, skip_serializing_if = "no_actions")]
    pub actions: Option<Vec<String>>,
    /// The selected grenade type: `frag` or `plasma`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grenade: Option<String>,
    /// Velocity, game units a second (packed version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vel: Option<[f64; 3]>,
    /// The ticks the held weapon fired on: bit `k` is a shot on tick
    /// `tick - k`, over the frame's span and up to 32 ticks back (version
    /// 3; no shots is sent as absent). `shots` still counts them.
    #[serde(default, skip_serializing_if = "no_fire")]
    pub fire: Option<u32>,
    /// Body vitality, 0 (dead) to 1 (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<f64>,
    /// Shield vitality, 0 to [`MAX_FRAME_SHIELD`] (1 is a full shield; an
    /// overshield goes past it) (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shield: Option<f64>,
    /// The seat of `vehicle` they are in, the vehicle's own seat index, at
    /// most [`MAX_FRAME_SEAT`] (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<u32>,
    /// How many ticks before `tick` each of `actions` started, in the same
    /// order (version 3): present only with `actions`, one each, at most
    /// [`MAX_FRAME_AGO`].
    #[serde(default, skip_serializing_if = "no_ago")]
    pub actions_ago: Option<Vec<u32>>,
    /// The animation it plays and where in it (version 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim: Option<FrameAnim>,
}

fn no_shots(shots: &Option<u32>) -> bool {
    shots.is_none_or(|n| n == 0)
}

fn no_actions(actions: &Option<Vec<String>>) -> bool {
    actions.as_ref().is_none_or(Vec::is_empty)
}

fn no_fire(fire: &Option<u32>) -> bool {
    fire.is_none_or(|m| m == 0)
}

fn no_ago(ago: &Option<Vec<u32>>) -> bool {
    ago.as_ref().is_none_or(Vec::is_empty)
}

/// Most shield a frame reports (an overshield is three full shields).
pub const MAX_FRAME_SHIELD: f64 = 4.0;
/// Highest vehicle seat index a frame reports.
pub const MAX_FRAME_SEAT: u32 = 63;
/// Furthest back (ticks before the frame's) a one-shot may be placed: 30
/// seconds, past any frame a server holds back.
pub const MAX_FRAME_AGO: u32 = 900;

/// Highest animation frame a frame reports (frame counts are `i16`).
pub const MAX_ANIM_FRAME: f64 = 32767.0;

/// The animation a player or actor plays: the name of the animation in the
/// unit's graph (`combat:rifle:any:idle`, `s_kill:front:gut`) and the frame,
/// 0 to its frame count minus one, fractional between ticks.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameAnim {
    pub name: String,
    pub frame: f64,
}

/// An animation's name: `[a-z0-9_:-]{1,64}`.
pub fn valid_anim_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes().all(|b| {
            b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b':' || b == b'-'
        })
}

/// The values `stance` may take.
pub const STANCES: [&str; 5] = ["stand", "crouch", "airborne", "vehicle", "dead"];
/// The values `actions` may hold.
pub const ACTIONS: [&str; 6] = ["grenade", "melee", "reload", "jump", "switch", "pickup"];
/// The values `grenade` may take.
pub const GRENADES: [&str; 2] = ["frag", "plasma"];
/// Most actions one player or actor carries in a frame.
pub const MAX_FRAME_ACTIONS: usize = 8;
/// Most hits one frame may carry.
pub const MAX_FRAME_HITS: usize = 64;
/// Most machines one frame may carry.
pub const MAX_FRAME_MACHINES: usize = 128;
/// Fastest `speed` accepted, game units a second.
pub const MAX_FRAME_SPEED: f64 = 1000.0;

/// Most shots one frame may report (frames are 15 Hz).
pub const MAX_FRAME_SHOTS: u32 = 64;

/// A frame's `species` or `weapon` id: `[a-z0-9_]{1,32}`.
pub fn valid_model_id(s: &str) -> bool {
    (1..=32).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// One telemetry frame (`tick` is the game tick, 30 per second).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Frame {
    pub game_id: String,
    pub tick: u64,
    pub players: Vec<FramePlayer>,
    /// Every vehicle in the world, occupied ones first, at most
    /// [`MAX_FRAME_VEHICLES`]. `FramePlayer.vehicle` and `FrameActor.vehicle`
    /// hold the matching `id`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vehicles: Vec<FrameVehicle>,
    /// AI-controlled bipeds, at most [`MAX_FRAME_ACTORS`]: the living
    /// (nearest to any player first), then the dead bodies that are not
    /// players' (stance `dead`, nearest first, a few dozen at most), each
    /// until the game clears it away.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub actors: Vec<FrameActor>,
    /// Damage dealt since the previous frame, oldest first, at most
    /// [`MAX_FRAME_HITS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub hits: Vec<FrameHit>,
    /// The structure BSP the game has loaded (campaign switches between
    /// zones).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bsp: Option<u32>,
    /// Machines (doors, elevators, the scarab, ...) that moved since the
    /// previous frame; all of them in a game's first frame and in every
    /// recorded frame. At most [`MAX_FRAME_MACHINES`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub machines: Vec<FrameMachine>,
    /// Scenario placements that are not where the scenario put them: moved
    /// by more than a small epsilon, or gone (destroyed, picked up, not
    /// created). Every such placement, each frame, ordered by kind then
    /// index; the packed wire format sends only what changed. At most
    /// [`MAX_FRAME_OBJECTS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub objects: Vec<FrameObject>,
    /// Set on the first frame after a campaign checkpoint revert or mission
    /// restart: the engine tick the game resumed at. The game's `tick`
    /// keeps counting (docs/live/replay-fidelity.md, "The game clock"), so
    /// this is the one place the two meet (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cut: Option<u64>,
    /// A campaign cinematic is playing (between HaloScript's
    /// `cinematic_start` and `cinematic_stop`): the game sends no players,
    /// actors, vehicles, hits, machines or objects, only the tick and the
    /// BSP, so a viewer shows a cutscene rather than the puppets the
    /// scripts move about (docs/live/live-telemetry.md, "Cinematics";
    /// packed version 5; every version 6 frame carries the bit).
    #[serde(default, skip_serializing_if = "is_false")]
    pub cinematic: bool,
    /// Projectiles in flight that a viewer can see fly (grenades, rockets,
    /// plasma, needles: slower than [`MAX_PROJECTILE_SPEED`]; faster ones
    /// live a tick or two and are drawn from `fire` and `aim`), at most
    /// [`MAX_FRAME_PROJECTILES`] (packed version 6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projectiles: Vec<FrameProjectile>,
    /// Weapons and equipment lying loose in the world that no scenario
    /// placement or netgame spawn made (those are in `objects`): a dead
    /// unit's dropped gun, a dropped flag, ball or bomb. At most
    /// [`MAX_FRAME_ITEMS`] (packed version 6).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub items: Vec<FrameItem>,
}

/// A projectile in flight (packed version 6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameProjectile {
    /// The projectile's object datum in hex.
    pub id: String,
    /// The `proj` tag file stem (`frag_grenade`, `plasma_grenade`,
    /// `rocket`, ...), or `unknown`.
    pub kind: String,
    /// World position; with `parent`, where that object is (a stuck plasma
    /// grenade's own transform is relative to what it stuck to).
    pub pos: [f64; 3],
    /// Velocity, game units a second; absent when stuck or not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vel: Option<[f64; 3]>,
    /// The object it is attached to (a plasma grenade stuck to a unit or a
    /// vehicle): its id as players, actors and vehicles carry it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
}

/// A loose weapon or piece of equipment (packed version 6).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameItem {
    /// The object datum in hex.
    pub id: String,
    /// The `weap` (or with `equipment`, `eqip`) tag file stem: `flag`,
    /// `ball`, `assault_bomb`, `battle_rifle`, ..., or `unknown`.
    pub kind: String,
    pub pos: [f64; 3],
    /// Unit length, game coordinates.
    pub forward: [f64; 3],
    pub up: [f64; 3],
    /// An `eqip` tag (grenades, powerups), not a `weap`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub equipment: bool,
}

/// Most projectiles one frame may carry.
pub const MAX_FRAME_PROJECTILES: usize = 64;
/// Most loose items one frame may carry.
pub const MAX_FRAME_ITEMS: usize = 128;
/// Projectiles faster than this (world units a second) are not sent: a
/// bullet (400) crosses a map in a tick or two, and the viewer draws it
/// from the shooter's `fire` and `aim`.
pub const MAX_PROJECTILE_SPEED: f64 = 100.0;

/// Which scenario block a [`FrameObject`] is placed by. The order is the
/// packed format's kind code (docs/live/live-telemetry.md, Packed frames). Its
/// JSON name is the placement's `kind` in map.glb's `scenario` node
/// (docs/live/live-art.md): the tag group, and `netgame` for multiplayer spawns.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum ObjectKind {
    /// `scnr+0x60`.
    #[serde(rename = "bipd")]
    Biped,
    /// `scnr+0x70`.
    #[serde(rename = "vehi")]
    Vehicle,
    /// `scnr+0x90`.
    #[serde(rename = "weap")]
    Weapon,
    /// `scnr+0x80`.
    #[serde(rename = "eqip")]
    Equipment,
    /// `scnr+0x50`.
    #[serde(rename = "scen")]
    Scenery,
    /// `scnr+0xa8`; machines also travel in [`Frame::machines`].
    #[serde(rename = "mach")]
    Machine,
    /// `scnr+0xb8`.
    #[serde(rename = "ctrl")]
    Control,
    /// `scnr+0xc8`.
    #[serde(rename = "lifi")]
    LightFixture,
    /// `scnr+0xd8`.
    #[serde(rename = "ssce")]
    SoundScenery,
    /// `scnr+0x328` (`bloc`: crates and physics props).
    #[serde(rename = "bloc")]
    Crate,
    /// `scnr+0x368`.
    #[serde(rename = "crea")]
    Creature,
    /// `scnr+0x120`: a multiplayer weapon or vehicle spawn.
    #[serde(rename = "netgame")]
    NetgameEquipment,
}

impl ObjectKind {
    /// Every kind, in code order.
    pub const ALL: [ObjectKind; 12] = [
        ObjectKind::Biped,
        ObjectKind::Vehicle,
        ObjectKind::Weapon,
        ObjectKind::Equipment,
        ObjectKind::Scenery,
        ObjectKind::Machine,
        ObjectKind::Control,
        ObjectKind::LightFixture,
        ObjectKind::SoundScenery,
        ObjectKind::Crate,
        ObjectKind::Creature,
        ObjectKind::NetgameEquipment,
    ];

    /// The packed format's code.
    pub fn code(self) -> u8 {
        Self::ALL.iter().position(|k| *k == self).unwrap() as u8
    }

    pub fn from_code(code: u8) -> Option<Self> {
        Self::ALL.get(usize::from(code)).copied()
    }
}

/// A scenario placement that moved or is gone: `k` and `i` name it (the
/// kind's scenario block and the index in it, map.glb's `scenario` child
/// `{kind, index}`). A moved one carries its pose; a gone one only `gone`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FrameObject {
    pub k: ObjectKind,
    pub i: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub p: Option<[f64; 3]>,
    /// Forward, unit length, game coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub f: Option<[f64; 3]>,
    /// Up, unit length.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub u: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub gone: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// Most objects one frame may carry.
pub const MAX_FRAME_OBJECTS: usize = 2048;
/// Largest placement index (scenario blocks count with an `i16`).
pub const MAX_OBJECT_INDEX: u32 = 0x7fff;

/// Damage one object took.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameHit {
    /// Attacker: `p<index>` for a player, else an object datum in hex;
    /// absent when nobody (falls, the world).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Victim, the same way.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// The weapon's tag file stem when known, else the damage effect's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    /// The victim's position.
    pub pos: [f64; 3],
    /// Body plus shield damage.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub amount: Option<f64>,
    /// `bullet`, `explosion`, `grenade`, `melee`, `fall`, `collision`,
    /// `vehicle`, ...
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// How many ticks before the frame's tick it landed, at most
    /// [`MAX_FRAME_AGO`] (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ago: Option<u32>,
}

/// A device machine or other big scripted mover.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameMachine {
    /// map.glb's `machines` child for it: `<tag stem>_<scenario placement
    /// index>`; absent for objects the scenario did not place.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub node: Option<String>,
    /// The object datum in hex.
    pub id: String,
    /// The tag file stem, lower case.
    pub kind: String,
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
    /// Device position, 0 to 1 (door or elevator travel).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<f64>,
}

fn occupied_by_default() -> bool {
    true
}

/// An occupied vehicle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameVehicle {
    /// The vehicle's object datum in hex, as `FramePlayer.vehicle`.
    pub id: String,
    /// The `vehi` tag file stem (`warthog`, `ghost`, `banshee`, ...), or
    /// `unknown` when the name is not known.
    pub kind: String,
    pub pos: [f64; 3],
    /// Unit length, game coordinates (z up).
    pub forward: [f64; 3],
    /// Unit length; banshees roll and pitch.
    pub up: [f64; 3],
    /// Someone (player or AI) is in it. Old senders listed only occupied
    /// vehicles, so absent reads as true.
    #[serde(default = "occupied_by_default")]
    pub occupied: bool,
    /// The vehicle it is mounted on, when it is a turret riding another
    /// (the Warthog's chaingun, the Scorpion's cannon): that one's `id`.
    /// Its `pos`, `forward` and `up` are then the game's parent-relative
    /// ones; a reader places it on its parent instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    /// The vehicle's own aiming vector (it is a unit): where a turret
    /// points. Unit length, game coordinates; absent when not read.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<[f64; 3]>,
    /// Velocity, game units a second (body `+0x88`), as players' and
    /// actors' `vel` (packed version 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vel: Option<[f64; 3]>,
}

/// An AI-controlled biped.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct FrameActor {
    /// The biped's object datum in hex.
    pub id: String,
    /// The directory of its biped tag (`grunt`, `jackal`, `elite`, `marine`,
    /// ...), or `unknown` when the name is not known.
    pub species: String,
    pub pos: [f64; 3],
    pub yaw: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aim: Option<[f64; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weapon: Option<String>,
    #[serde(default, skip_serializing_if = "no_shots")]
    pub shots: Option<u32>,
    /// `covenant`, `human`, `flood`, `sentinel`, `prophet`, `player`, ... or
    /// the raw team index as text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vehicle: Option<String>,
    /// `stand`, `crouch`, `airborne` (jumping or falling), `vehicle` or
    /// `dead`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stance: Option<String>,
    /// Horizontal speed, game units a second.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed: Option<f64>,
    /// One-shot actions started since the previous frame, oldest first, at
    /// most [`MAX_FRAME_ACTIONS`]: see [`ACTIONS`].
    #[serde(default, skip_serializing_if = "no_actions")]
    pub actions: Option<Vec<String>>,
    /// The selected grenade type: `frag` or `plasma`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grenade: Option<String>,
    /// Velocity, game units a second (packed version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vel: Option<[f64; 3]>,
    /// The ticks the held weapon fired on: bit `k` is a shot on tick
    /// `tick - k`, over the frame's span and up to 32 ticks back (version
    /// 3; no shots is sent as absent). `shots` still counts them.
    #[serde(default, skip_serializing_if = "no_fire")]
    pub fire: Option<u32>,
    /// Body vitality, 0 (dead) to 1 (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body: Option<f64>,
    /// Shield vitality, 0 to [`MAX_FRAME_SHIELD`] (1 is a full shield; an
    /// overshield goes past it) (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shield: Option<f64>,
    /// The seat of `vehicle` they are in, the vehicle's own seat index, at
    /// most [`MAX_FRAME_SEAT`] (version 3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seat: Option<u32>,
    /// How many ticks before `tick` each of `actions` started, in the same
    /// order (version 3): present only with `actions`, one each, at most
    /// [`MAX_FRAME_AGO`].
    #[serde(default, skip_serializing_if = "no_ago")]
    pub actions_ago: Option<Vec<u32>>,
    /// The animation it plays and where in it (version 4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub anim: Option<FrameAnim>,
}

/// Most actors one frame may carry.
pub const MAX_FRAME_ACTORS: usize = 128;
/// Most vehicles one frame may carry.
pub const MAX_FRAME_VEHICLES: usize = 64;

/// Who fired, for carrying shots between frames.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Shooter {
    Player(u32),
    Actor(String),
}

/// A map outline (`PUT /v1/maps/{map}/outline`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outline {
    pub map: String,
    pub bounds: Bounds,
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub z: [f64; 2],
    pub polygons: Vec<Vec<[f64; 2]>>,
}

/// An entry of `GET /v1/games`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameSummary {
    pub game_id: String,
    pub map: String,
    pub mode: String,
    /// Always set: the game's `kind`, or the one inferred from `mode`.
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub variant: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playlist_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub difficulty: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub online: Option<bool>,
    pub team_play: bool,
    /// `live` or `ended`.
    pub state: String,
    pub started_at: String,
    #[serde(default)]
    pub ended_at: Option<String>,
    pub players: Vec<Player>,
    #[serde(default)]
    pub teams: Option<Vec<TeamScore>>,
    /// The game's `NewGame.map_id`; left out when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_id: Option<crate::session::MapId>,
    /// The game's `NewGame.skulls`; left out when none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skulls: Option<Vec<u8>>,
}

/// A kill as `GET /v1/games/{id}` lists it: the event data plus where it
/// is in the log.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KillRecord {
    #[serde(flatten)]
    pub kill: PlayerKilled,
    pub position: u64,
    pub at: String,
}

/// `GET /v1/games/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameDetail {
    #[serde(flatten)]
    pub summary: GameSummary,
    pub console: Option<String>,
    pub host: String,
    pub score_limit: Option<i64>,
    pub time_limit_s: Option<i64>,
    pub tick: Option<u64>,
    pub last_tick: Option<u64>,
    pub scoreboard: Vec<PlayerScore>,
    /// The last 50, oldest first.
    pub kills: Vec<KillRecord>,
    pub winner: Option<Winner>,
    pub end_reason: Option<String>,
    pub position: u64,
}

/// What `POST /v1/games` returns.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameCreated {
    pub game_id: String,
    pub position: u64,
}

/// Checks game event data by type; `Err` is a message for a 400.
pub fn validate(kind: &str, data: &Value, players: &[Player]) -> Result<(), String> {
    fn parse<T: serde::de::DeserializeOwned>(data: &Value) -> Result<T, String> {
        T::deserialize(data).map_err(|e| format!("data: {e}"))
    }
    let known = |i: u32| players.iter().any(|p| p.index == i);
    match kind {
        PLAYER_JOINED => {
            let d: PlayerJoined = parse(data)?;
            if known(d.player.index) {
                return Err(format!("player {} is already in the game", d.player.index));
            }
            if let Some(x) = d.player.xuid
                && players.iter().any(|p| p.xuid == Some(x))
            {
                return Err(format!("{x} is already in the game"));
            }
            if players.len() >= MAX_PLAYERS {
                return Err("the game is full".into());
            }
        }
        PLAYER_LEFT => {
            let d: PlayerLeft = parse(data)?;
            if !known(d.index) {
                return Err(format!("no player {}", d.index));
            }
        }
        PLAYER_KILLED => {
            let d: PlayerKilled = parse(data)?;
            if !KILL_KINDS.contains(&d.kind.as_str()) {
                return Err("kind must be kill, suicide, betrayal or world".into());
            }
            if !known(d.victim) || d.killer.is_some_and(|k| !known(k)) {
                return Err("victim and killer must be players of the game".into());
            }
            if d.pos.is_some_and(|p| p.iter().any(|c| !c.is_finite())) {
                return Err("pos must be finite".into());
            }
        }
        SCORE_CHANGED => {
            let d: ScoreChanged = parse(data)?;
            if d.players.iter().any(|s| !known(s.index)) {
                return Err("scores must be for players of the game".into());
            }
        }
        GAME_ENDED => {
            let d: GameEnded = parse(data)?;
            if !END_REASONS.contains(&d.reason.as_str()) {
                return Err("reason must be finished, quit or lost".into());
            }
            if let Some(Winner::Player { player }) = d.winner
                && !known(player)
            {
                return Err("winner must be a player of the game".into());
            }
        }
        _ => return Err(format!("{kind} is not a game event a console writes")),
    }
    Ok(())
}

/// Checks a new game's data.
pub fn validate_new(game: &NewGame) -> Result<(), String> {
    if !valid_id(&game.map) {
        return Err("map must be [A-Za-z0-9_-]{1,64}".into());
    }
    if !matches!(game.mode.as_str(), "campaign" | "multiplayer") {
        return Err("mode must be campaign or multiplayer".into());
    }
    if game.players.len() > MAX_PLAYERS {
        return Err("at most 16 players".into());
    }
    let mut seen = std::collections::HashSet::new();
    if !game.players.iter().all(|p| seen.insert(p.index)) {
        return Err("player indexes must be unique".into());
    }
    let mut seen = std::collections::HashSet::new();
    if !game
        .players
        .iter()
        .filter_map(|p| p.xuid)
        .all(|x| seen.insert(x))
    {
        return Err("player xuids must be unique".into());
    }
    if game.players.iter().any(|p| p.name.chars().count() > 32) {
        return Err("player names are at most 32 characters".into());
    }
    // A bot with an xuid is a rostered one; a bot without one is the
    // host's own and needs no session.
    if game.players.iter().any(|p| p.is_bot() && p.xuid.is_some()) && game.session_id.is_none() {
        return Err("a game with rostered bots names the session they were rostered for".into());
    }
    if let Some(kind) = &game.kind {
        if !GAME_KINDS.contains(&kind.as_str()) {
            return Err("kind must be custom, matchmaking, clankermatch or campaign".into());
        }
        if (kind == "campaign") != (game.mode == "campaign") {
            return Err("kind campaign goes with mode campaign, and only with it".into());
        }
    }
    let kind = game.kind();
    if (game.playlist.is_some() || game.playlist_id.is_some()) && !is_matchmade(kind) {
        return Err("playlist is for matchmaking and clankermatch games".into());
    }
    if let Some(name) = &game.playlist
        && (name.is_empty() || name.chars().count() > 32 || name.chars().any(char::is_control))
    {
        return Err("playlist is 1 to 32 characters, no control characters".into());
    }
    if let Some(id) = &game.playlist_id
        && !valid_id(id)
    {
        return Err("playlist_id must be [A-Za-z0-9_-]{1,64}".into());
    }
    if let Some(id) = &game.session_id {
        if kind == "campaign" {
            return Err("session_id is for multiplayer games".into());
        }
        if !crate::arbitration::valid_id(id) {
            return Err("session_id must be 16 lowercase hex digits".into());
        }
    }
    if let Some(difficulty) = &game.difficulty {
        if kind != "campaign" {
            return Err("difficulty is for campaign games".into());
        }
        if !DIFFICULTIES.contains(&difficulty.as_str()) {
            return Err("difficulty must be easy, normal, heroic or legendary".into());
        }
    }
    if game.online.is_some() && kind != "campaign" {
        return Err("online is for campaign games".into());
    }
    if let Some(skulls) = &game.skulls {
        crate::skulls::validate(skulls)?;
        if kind != "campaign" && !crate::skulls::multiplayer_ok(skulls) {
            return Err("a multiplayer game names only skulls that work there".into());
        }
    }
    Ok(())
}

/// Checks a telemetry frame.
pub fn validate_frame(frame: &Frame) -> Result<(), String> {
    if frame.players.len() > MAX_PLAYERS {
        return Err("at most 16 players".into());
    }
    let finite = |p: &FramePlayer| p.pos.iter().all(|c| c.is_finite()) && p.yaw.is_finite();
    if !frame.players.iter().all(finite) {
        return Err("positions must be finite".into());
    }
    for p in &frame.players {
        if let Some(aim) = p.aim
            && (!aim.iter().all(|c| c.is_finite()) || length(aim) < 1e-6)
        {
            return Err("aim must be a finite, non-zero vector".into());
        }
        let ids = [&p.species, &p.weapon];
        if ids.into_iter().flatten().any(|id| !valid_model_id(id)) {
            return Err("species and weapon must be [a-z0-9_]{1,32}".into());
        }
        if p.shots.is_some_and(|n| n > MAX_FRAME_SHOTS) {
            return Err(format!("at most {MAX_FRAME_SHOTS} shots a frame"));
        }
        if p.vehicle.as_deref().is_some_and(|id| !valid_model_id(id)) {
            return Err("vehicle ids must be [a-z0-9_]{1,32}".into());
        }
        validate_motion(&p.stance, p.speed, &p.actions, &p.grenade)?;
        validate_v3(p.vel, p.body, p.shield, p.seat, &p.actions, &p.actions_ago)?;
        validate_anim(&p.anim)?;
    }
    if frame.vehicles.len() > MAX_FRAME_VEHICLES {
        return Err(format!("at most {MAX_FRAME_VEHICLES} vehicles"));
    }
    if frame.actors.len() > MAX_FRAME_ACTORS {
        return Err(format!("at most {MAX_FRAME_ACTORS} actors"));
    }
    let direction = |v: [f64; 3]| v.iter().all(|c| c.is_finite()) && length(v) >= 1e-6;
    for v in &frame.vehicles {
        if !valid_model_id(&v.id) || !valid_model_id(&v.kind) {
            return Err("vehicle id and kind must be [a-z0-9_]{1,32}".into());
        }
        if !v.pos.iter().all(|c| c.is_finite()) || !direction(v.forward) || !direction(v.up) {
            return Err("vehicle pos must be finite, forward and up finite and non-zero".into());
        }
        if v.parent
            .as_deref()
            .is_some_and(|id| !valid_model_id(id) || id == v.id)
        {
            return Err("vehicle parent must be another [a-z0-9_]{1,32} id".into());
        }
        if v.aim.is_some_and(|a| !direction(a)) {
            return Err("vehicle aim must be a finite, non-zero vector".into());
        }
        validate_vel(v.vel)?;
    }
    for a in &frame.actors {
        let ids = [
            Some(&a.id),
            Some(&a.species),
            a.weapon.as_ref(),
            a.team.as_ref(),
            a.vehicle.as_ref(),
        ];
        if ids.into_iter().flatten().any(|id| !valid_model_id(id)) {
            return Err("actor ids must be [a-z0-9_]{1,32}".into());
        }
        if !a.pos.iter().all(|c| c.is_finite()) || !a.yaw.is_finite() {
            return Err("actor positions must be finite".into());
        }
        if a.aim.is_some_and(|aim| !direction(aim)) {
            return Err("aim must be a finite, non-zero vector".into());
        }
        if a.shots.is_some_and(|n| n > MAX_FRAME_SHOTS) {
            return Err(format!("at most {MAX_FRAME_SHOTS} shots a frame"));
        }
        validate_motion(&a.stance, a.speed, &a.actions, &a.grenade)?;
        validate_v3(a.vel, a.body, a.shield, a.seat, &a.actions, &a.actions_ago)?;
        validate_anim(&a.anim)?;
    }
    if frame.hits.len() > MAX_FRAME_HITS {
        return Err(format!("at most {MAX_FRAME_HITS} hits"));
    }
    for h in &frame.hits {
        let ids = [&h.from, &h.to, &h.weapon, &h.kind];
        if ids.into_iter().flatten().any(|id| !valid_model_id(id)) {
            return Err("hit from, to, weapon and kind must be [a-z0-9_]{1,32}".into());
        }
        if !h.pos.iter().all(|c| c.is_finite()) {
            return Err("hit pos must be finite".into());
        }
        if h.amount.is_some_and(|a| !a.is_finite() || a < 0.0) {
            return Err("hit amount must be finite and not negative".into());
        }
        if h.ago.is_some_and(|a| a > MAX_FRAME_AGO) {
            return Err(format!("hit ago must be at most {MAX_FRAME_AGO}"));
        }
    }
    if frame.machines.len() > MAX_FRAME_MACHINES {
        return Err(format!("at most {MAX_FRAME_MACHINES} machines"));
    }
    validate_objects(&frame.objects)?;
    validate_projectiles(&frame.projectiles)?;
    validate_items(&frame.items)?;
    for m in &frame.machines {
        if !valid_model_id(&m.id) || !valid_model_id(&m.kind) {
            return Err("machine id and kind must be [a-z0-9_]{1,32}".into());
        }
        if m.node.as_deref().is_some_and(|n| !valid_node(n)) {
            return Err("machine node must be [a-z0-9_]{1,64}".into());
        }
        if !m.pos.iter().all(|c| c.is_finite()) || !direction(m.forward) || !direction(m.up) {
            return Err("machine pos must be finite, forward and up finite and non-zero".into());
        }
        if m.position.is_some_and(|p| !(0.0..=1.0).contains(&p)) {
            return Err("machine position must be 0 to 1".into());
        }
    }
    Ok(())
}

fn validate_vel(vel: Option<[f64; 3]>) -> Result<(), String> {
    if vel.is_some_and(|v| !v.iter().all(|c| c.is_finite()) || length(v) > MAX_FRAME_SPEED) {
        return Err(format!("vel must be finite and at most {MAX_FRAME_SPEED}"));
    }
    Ok(())
}

fn validate_projectiles(projectiles: &[FrameProjectile]) -> Result<(), String> {
    if projectiles.len() > MAX_FRAME_PROJECTILES {
        return Err(format!("at most {MAX_FRAME_PROJECTILES} projectiles"));
    }
    for p in projectiles {
        if !valid_model_id(&p.id) || !valid_model_id(&p.kind) {
            return Err("projectile id and kind must be [a-z0-9_]{1,32}".into());
        }
        if p.parent
            .as_deref()
            .is_some_and(|id| !valid_model_id(id) || id == p.id)
        {
            return Err("projectile parent must be another [a-z0-9_]{1,32} id".into());
        }
        if !p.pos.iter().all(|c| c.is_finite()) {
            return Err("projectile pos must be finite".into());
        }
        validate_vel(p.vel)?;
    }
    Ok(())
}

fn validate_items(items: &[FrameItem]) -> Result<(), String> {
    if items.len() > MAX_FRAME_ITEMS {
        return Err(format!("at most {MAX_FRAME_ITEMS} items"));
    }
    let direction = |v: [f64; 3]| v.iter().all(|c| c.is_finite()) && length(v) >= 1e-6;
    for i in items {
        if !valid_model_id(&i.id) || !valid_model_id(&i.kind) {
            return Err("item id and kind must be [a-z0-9_]{1,32}".into());
        }
        if !i.pos.iter().all(|c| c.is_finite()) || !direction(i.forward) || !direction(i.up) {
            return Err("item pos must be finite, forward and up finite and non-zero".into());
        }
    }
    Ok(())
}

fn validate_objects(objects: &[FrameObject]) -> Result<(), String> {
    if objects.len() > MAX_FRAME_OBJECTS {
        return Err(format!("at most {MAX_FRAME_OBJECTS} objects"));
    }
    let direction = |v: [f64; 3]| v.iter().all(|c| c.is_finite()) && length(v) >= 1e-6;
    for pair in objects.windows(2) {
        if (pair[0].k, pair[0].i) >= (pair[1].k, pair[1].i) {
            return Err("objects must be ordered by kind and index, each once".into());
        }
    }
    for o in objects {
        if o.i > MAX_OBJECT_INDEX {
            return Err(format!("object index above {MAX_OBJECT_INDEX}"));
        }
        match (o.gone, o.p, o.f, o.u) {
            (true, None, None, None) => {}
            (false, Some(p), Some(f), Some(u)) => {
                if !p.iter().all(|c| c.is_finite()) || !direction(f) || !direction(u) {
                    return Err("object p must be finite, f and u finite and non-zero".into());
                }
            }
            _ => return Err("an object is gone, or has p, f and u".into()),
        }
    }
    Ok(())
}

/// A map.glb node name: `[a-z0-9_]{1,64}`.
pub fn valid_node(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

fn validate_motion(
    stance: &Option<String>,
    speed: Option<f64>,
    actions: &Option<Vec<String>>,
    grenade: &Option<String>,
) -> Result<(), String> {
    if stance.as_deref().is_some_and(|s| !STANCES.contains(&s)) {
        return Err("stance must be stand, crouch, airborne, vehicle or dead".into());
    }
    if speed.is_some_and(|s| !(0.0..=MAX_FRAME_SPEED).contains(&s)) {
        return Err(format!("speed must be 0 to {MAX_FRAME_SPEED}"));
    }
    if let Some(actions) = actions {
        if actions.len() > MAX_FRAME_ACTIONS {
            return Err(format!("at most {MAX_FRAME_ACTIONS} actions"));
        }
        if !actions.iter().all(|a| ACTIONS.contains(&a.as_str())) {
            return Err("actions are grenade, melee, reload, jump, switch or pickup".into());
        }
    }
    if grenade.as_deref().is_some_and(|g| !GRENADES.contains(&g)) {
        return Err("grenade must be frag or plasma".into());
    }
    Ok(())
}

/// The version 3 fields of a player or actor.
fn validate_v3(
    vel: Option<[f64; 3]>,
    body: Option<f64>,
    shield: Option<f64>,
    seat: Option<u32>,
    actions: &Option<Vec<String>>,
    ago: &Option<Vec<u32>>,
) -> Result<(), String> {
    if vel.is_some_and(|v| !v.iter().all(|c| c.is_finite()) || length(v) > MAX_FRAME_SPEED) {
        return Err(format!("vel must be finite and at most {MAX_FRAME_SPEED}"));
    }
    if body.is_some_and(|b| !(0.0..=1.0).contains(&b)) {
        return Err("body must be 0 to 1".into());
    }
    if shield.is_some_and(|s| !(0.0..=MAX_FRAME_SHIELD).contains(&s)) {
        return Err(format!("shield must be 0 to {MAX_FRAME_SHIELD}"));
    }
    if seat.is_some_and(|s| s > MAX_FRAME_SEAT) {
        return Err(format!("seat must be at most {MAX_FRAME_SEAT}"));
    }
    if let Some(ago) = ago.as_ref().filter(|a| !a.is_empty()) {
        if actions.as_ref().map_or(0, Vec::len) != ago.len() {
            return Err("actions_ago must have one entry for each action".into());
        }
        if ago.iter().any(|a| *a > MAX_FRAME_AGO) {
            return Err(format!("actions_ago must be at most {MAX_FRAME_AGO}"));
        }
    }
    Ok(())
}

fn validate_anim(anim: &Option<FrameAnim>) -> Result<(), String> {
    match anim {
        Some(a) if !valid_anim_name(&a.name) => Err("anim names must be [a-z0-9_:-]{1,64}".into()),
        Some(a) if !(0.0..=MAX_ANIM_FRAME).contains(&a.frame) => {
            Err(format!("anim frames must be 0 to {MAX_ANIM_FRAME}"))
        }
        _ => Ok(()),
    }
}

fn length(v: [f64; 3]) -> f64 {
    v.iter().map(|c| c * c).sum::<f64>().sqrt()
}

/// What a dropped frame (coalesced in a queue or throttled by the server)
/// hands on to the next delivered one so nothing one-shot is lost: shots,
/// fire, actions, hits, a cut, and the latest state of each machine. What
/// is placed in time (fire, actions' and hits' `ago`) is kept on absolute
/// ticks and placed again against the frame that takes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FrameCarry {
    shots: Vec<(Shooter, u32)>,
    /// Each shooter's fire mask as of a tick.
    fire: Vec<(Shooter, u64, u32)>,
    /// Each action with the tick it started on, when its frame said.
    actions: Vec<(Shooter, Vec<(String, Option<u64>)>)>,
    hits: Vec<(FrameHit, Option<u64>)>,
    machines: Vec<FrameMachine>,
    cut: Option<u64>,
}

/// How many shooters a carry keeps shots and actions for.
const MAX_CARRIED_SHOOTERS: usize = 4 * (MAX_PLAYERS + MAX_FRAME_ACTORS);

/// A fire mask moved `by` ticks older (shots past 32 ticks drop off).
fn older(mask: u32, by: u64) -> u32 {
    if by >= 32 { 0 } else { mask << by }
}

impl FrameCarry {
    /// Adds a frame that will not be delivered (or whose one-shot data a
    /// later frame must repeat, as a recorded frame does).
    pub fn add(&mut self, frame: &Frame) {
        let t = frame.tick;
        for (shooter, n) in frame.shots() {
            if let Some((_, total)) = self.shots.iter_mut().find(|(s, _)| *s == shooter) {
                *total = total.saturating_add(n).min(MAX_FRAME_SHOTS);
            } else if self.shots.len() < MAX_CARRIED_SHOOTERS {
                self.shots.push((shooter, n.min(MAX_FRAME_SHOTS)));
            }
        }
        for (shooter, mask) in frame.fire() {
            if let Some((_, at, kept)) = self.fire.iter_mut().find(|(s, _, _)| *s == shooter) {
                if t >= *at {
                    *kept = older(*kept, t - *at) | mask;
                    *at = t;
                } else {
                    *kept |= older(mask, *at - t);
                }
            } else if self.fire.len() < MAX_CARRIED_SHOOTERS {
                self.fire.push((shooter, t, mask));
            }
        }
        for (shooter, actions) in frame.timed_actions() {
            if let Some((_, kept)) = self.actions.iter_mut().find(|(s, _)| *s == shooter) {
                kept.extend(actions);
                let over = kept.len().saturating_sub(MAX_FRAME_ACTIONS);
                kept.drain(..over);
            } else if self.actions.len() < MAX_CARRIED_SHOOTERS {
                self.actions.push((shooter, actions));
            }
        }
        self.hits.extend(
            frame
                .hits
                .iter()
                .map(|h| (h.clone(), h.ago.map(|a| t.saturating_sub(u64::from(a))))),
        );
        let over = self.hits.len().saturating_sub(MAX_FRAME_HITS);
        self.hits.drain(..over);
        for m in &frame.machines {
            if let Some(kept) = self.machines.iter_mut().find(|k| k.id == m.id) {
                *kept = m.clone();
            } else if self.machines.len() < MAX_FRAME_MACHINES {
                self.machines.push(m.clone());
            }
        }
        if frame.cut.is_some() {
            self.cut = frame.cut;
        }
    }

    pub fn is_empty(&self) -> bool {
        self.shots.is_empty()
            && self.fire.is_empty()
            && self.actions.is_empty()
            && self.hits.is_empty()
            && self.machines.is_empty()
            && self.cut.is_none()
    }
}

/// Ticks before `now` that `at` was, capped at [`MAX_FRAME_AGO`].
fn ago(now: u64, at: u64) -> u32 {
    now.saturating_sub(at).min(u64::from(MAX_FRAME_AGO)) as u32
}

impl Frame {
    /// Each player's and actor's fire mask, for carrying.
    fn fire(&self) -> impl Iterator<Item = (Shooter, u32)> + '_ {
        let players = self
            .players
            .iter()
            .filter_map(|p| Some((Shooter::Player(p.index), p.fire.filter(|m| *m != 0)?)));
        let actors = self
            .actors
            .iter()
            .filter_map(|a| Some((Shooter::Actor(a.id.clone()), a.fire.filter(|m| *m != 0)?)));
        players.chain(actors)
    }

    /// Each player's and actor's actions with the tick each started on
    /// (when the frame says), for carrying.
    fn timed_actions(&self) -> impl Iterator<Item = (Shooter, Vec<(String, Option<u64>)>)> + '_ {
        let t = self.tick;
        let timed = move |actions: &Option<Vec<String>>, ago: &Option<Vec<u32>>| {
            let actions = actions.as_ref().filter(|a| !a.is_empty())?;
            let ago = ago.as_ref().filter(|g| g.len() == actions.len());
            Some(
                actions
                    .iter()
                    .enumerate()
                    .map(|(i, a)| (a.clone(), ago.map(|g| t.saturating_sub(u64::from(g[i])))))
                    .collect::<Vec<_>>(),
            )
        };
        let players = self.players.iter().filter_map(move |p| {
            Some((Shooter::Player(p.index), timed(&p.actions, &p.actions_ago)?))
        });
        let actors = self.actors.iter().filter_map(move |a| {
            Some((
                Shooter::Actor(a.id.clone()),
                timed(&a.actions, &a.actions_ago)?,
            ))
        });
        players.chain(actors)
    }

    /// Takes in what earlier, undelivered frames carried: their shots are
    /// added and their fire masks merged, their actions and hits go before
    /// this frame's (the oldest dropped past the caps) and are placed in
    /// time against this frame's tick, their machines are kept where this
    /// frame has no newer state, and their cut is this frame's when it has
    /// none. Players and actors not in this frame lose theirs.
    pub fn absorb(&mut self, carry: FrameCarry) {
        let now = self.tick;
        self.add_shots(carry.shots);
        for (shooter, at, mask) in carry.fire {
            let mask = if now >= at {
                older(mask, now - at)
            } else {
                mask
            };
            let slot = match &shooter {
                Shooter::Player(index) => self
                    .players
                    .iter_mut()
                    .find(|p| p.index == *index)
                    .map(|p| &mut p.fire),
                Shooter::Actor(id) => self
                    .actors
                    .iter_mut()
                    .find(|a| a.id == *id)
                    .map(|a| &mut a.fire),
            };
            if let Some(slot) = slot {
                let all = slot.unwrap_or(0) | mask;
                *slot = (all != 0).then_some(all);
            }
        }
        for (shooter, earlier) in carry.actions {
            let slot = match &shooter {
                Shooter::Player(index) => self
                    .players
                    .iter_mut()
                    .find(|p| p.index == *index)
                    .map(|p| (&mut p.actions, &mut p.actions_ago)),
                Shooter::Actor(id) => self
                    .actors
                    .iter_mut()
                    .find(|a| a.id == *id)
                    .map(|a| (&mut a.actions, &mut a.actions_ago)),
            };
            if let Some((actions, agos)) = slot {
                let mine = actions.take().unwrap_or_default();
                let my_ago = agos.take().filter(|g| g.len() == mine.len());
                let mut all = earlier;
                all.extend(mine.into_iter().enumerate().map(|(i, a)| {
                    (
                        a,
                        my_ago.as_ref().map(|g| now.saturating_sub(u64::from(g[i]))),
                    )
                }));
                let over = all.len().saturating_sub(MAX_FRAME_ACTIONS);
                all.drain(..over);
                *agos = all
                    .iter()
                    .map(|(_, at)| at.map(|at| ago(now, at)))
                    .collect::<Option<Vec<u32>>>();
                *actions = Some(all.into_iter().map(|(a, _)| a).collect());
            }
        }
        if !carry.hits.is_empty() {
            let mut hits: Vec<FrameHit> = carry
                .hits
                .into_iter()
                .map(|(mut h, at)| {
                    h.ago = at.map(|at| ago(now, at));
                    h
                })
                .collect();
            hits.append(&mut self.hits);
            let over = hits.len().saturating_sub(MAX_FRAME_HITS);
            hits.drain(..over);
            self.hits = hits;
        }
        for m in carry.machines {
            if self.machines.len() < MAX_FRAME_MACHINES
                && !self.machines.iter().any(|k| k.id == m.id)
            {
                self.machines.push(m);
            }
        }
        if self.cut.is_none() {
            self.cut = carry.cut;
        }
    }

    /// Each player's and actor's shot count, for carrying into a later
    /// frame when this one is not delivered.
    pub fn shots(&self) -> impl Iterator<Item = (Shooter, u32)> + '_ {
        let players = self
            .players
            .iter()
            .filter_map(|p| Some((Shooter::Player(p.index), p.shots.filter(|n| *n > 0)?)));
        let actors = self
            .actors
            .iter()
            .filter_map(|a| Some((Shooter::Actor(a.id.clone()), a.shots.filter(|n| *n > 0)?)));
        players.chain(actors)
    }

    /// Adds shots from frames that were dropped (coalesced or throttled) to
    /// this one, so `shots` stays "since the previous delivered frame".
    /// Shooters not in this frame lose theirs; counts cap at
    /// [`MAX_FRAME_SHOTS`].
    pub fn add_shots(&mut self, shots: impl IntoIterator<Item = (Shooter, u32)>) {
        fn add(slot: &mut Option<u32>, n: u32) {
            let total = slot.unwrap_or(0).saturating_add(n).min(MAX_FRAME_SHOTS);
            *slot = (total > 0).then_some(total);
        }
        for (shooter, n) in shots {
            match shooter {
                Shooter::Player(index) => {
                    if let Some(p) = self.players.iter_mut().find(|p| p.index == index) {
                        add(&mut p.shots, n);
                    }
                }
                Shooter::Actor(id) => {
                    if let Some(a) = self.actors.iter_mut().find(|a| a.id == id) {
                        add(&mut a.shots, n);
                    }
                }
            }
        }
    }
}

/// Puts a validated frame in its stored form: every `aim` unit length and a
/// zero `shots` absent.
pub fn normalize_frame(frame: &mut Frame) {
    fn unit(v: &mut [f64; 3]) {
        let len = length(*v);
        if len > 0.0 {
            v.iter_mut().for_each(|c| *c /= len);
        }
    }
    for m in &mut frame.machines {
        unit(&mut m.forward);
        unit(&mut m.up);
    }
    for o in &mut frame.objects {
        o.f.iter_mut().chain(o.u.iter_mut()).for_each(unit);
    }
    for v in &mut frame.vehicles {
        unit(&mut v.forward);
        unit(&mut v.up);
    }
    for i in &mut frame.items {
        unit(&mut i.forward);
        unit(&mut i.up);
    }
    for a in &mut frame.actors {
        if let Some(aim) = &mut a.aim {
            unit(aim);
        }
        if a.shots == Some(0) {
            a.shots = None;
        }
        if a.actions.as_ref().is_some_and(Vec::is_empty) {
            a.actions = None;
        }
    }
    for v in &mut frame.vehicles {
        if let Some(aim) = &mut v.aim {
            unit(aim);
        }
    }
    for p in &mut frame.players {
        if p.actions.as_ref().is_some_and(Vec::is_empty) {
            p.actions = None;
        }
        if let Some(aim) = &mut p.aim {
            let len = length(*aim);
            if len > 0.0 {
                aim.iter_mut().for_each(|c| *c /= len);
            }
        }
        if p.shots == Some(0) {
            p.shots = None;
        }
        if p.fire == Some(0) {
            p.fire = None;
        }
        if p.actions.is_none() || p.actions_ago.as_ref().is_some_and(Vec::is_empty) {
            p.actions_ago = None;
        }
    }
    for a in &mut frame.actors {
        if a.fire == Some(0) {
            a.fire = None;
        }
        if a.actions.is_none() || a.actions_ago.as_ref().is_some_and(Vec::is_empty) {
            a.actions_ago = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn unit(index: u32, tick_fire: Option<u32>) -> FramePlayer {
        FramePlayer {
            index,
            alive: true,
            fire: tick_fire,
            ..FramePlayer::default()
        }
    }

    /// One-shots carried over dropped frames are placed again against the
    /// frame that takes them: fire masks shift, actions and hits keep their
    /// tick, a cut comes along.
    #[test]
    fn carried_one_shots_keep_their_ticks() {
        let mut carry = FrameCarry::default();
        let mut a = Frame {
            tick: 100,
            players: vec![unit(0, Some(0b1))],
            ..Frame::default()
        };
        a.players[0].actions = Some(vec!["jump".into()]);
        a.players[0].actions_ago = Some(vec![1]);
        a.hits = vec![FrameHit {
            ago: Some(0),
            ..FrameHit::default()
        }];
        a.cut = Some(7);
        carry.add(&a);
        let b = Frame {
            tick: 102,
            players: vec![unit(0, Some(0b10))],
            ..Frame::default()
        };
        carry.add(&b);
        let mut c = Frame {
            tick: 104,
            players: vec![unit(0, Some(0b1))],
            ..Frame::default()
        };
        c.players[0].actions = Some(vec!["melee".into()]);
        c.players[0].actions_ago = Some(vec![0]);
        c.absorb(carry);
        // Shots on 100, 101 and 104: 4, 3 and 0 ticks back.
        assert_eq!(c.players[0].fire, Some(0b11001));
        assert_eq!(
            c.players[0].actions.as_deref(),
            Some(&["jump".to_string(), "melee".to_string()][..])
        );
        assert_eq!(c.players[0].actions_ago.as_deref(), Some(&[5, 0][..]));
        assert_eq!(c.hits[0].ago, Some(4));
        assert_eq!(c.cut, Some(7));
        assert_eq!(validate_frame(&c), Ok(()));
    }

    #[test]
    fn carried_actions_without_ticks_drop_the_ticks() {
        let mut carry = FrameCarry::default();
        let mut a = Frame {
            tick: 10,
            players: vec![unit(0, None)],
            ..Frame::default()
        };
        a.players[0].actions = Some(vec!["reload".into()]);
        carry.add(&a);
        let mut b = Frame {
            tick: 12,
            players: vec![unit(0, None)],
            ..Frame::default()
        };
        b.players[0].actions = Some(vec!["switch".into()]);
        b.players[0].actions_ago = Some(vec![0]);
        b.absorb(carry);
        assert_eq!(b.players[0].actions.as_ref().map(Vec::len), Some(2));
        assert_eq!(
            b.players[0].actions_ago, None,
            "an older sender's action has no tick"
        );
        // Fire older than 32 ticks drops off.
        let mut carry = FrameCarry::default();
        carry.add(&Frame {
            tick: 0,
            players: vec![unit(0, Some(1))],
            ..Frame::default()
        });
        let mut late = Frame {
            tick: 40,
            players: vec![unit(0, None)],
            ..Frame::default()
        };
        late.absorb(carry);
        assert_eq!(late.players[0].fire, None);
    }

    fn players() -> Vec<Player> {
        vec![
            Player {
                index: 0,
                name: "A".into(),
                xuid: None,
                team: 0,
                local: Some(true),
                appearance: None,
                bot: None,
            },
            Player {
                index: 1,
                name: "B".into(),
                xuid: None,
                team: 1,
                local: None,
                appearance: None,
                bot: None,
            },
        ]
    }

    #[test]
    fn game_events_validate() {
        let p = players();
        for (kind, data) in [
            (
                PLAYER_JOINED,
                json!({"player": {"index": 2, "name": "C", "team": 0}}),
            ),
            (PLAYER_LEFT, json!({"index": 1})),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 1, "killer": 0, "weapon": "br", "kind": "kill", "pos": [1.0, 2.0, 3.0]}),
            ),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 1, "kind": "world"}),
            ),
            (
                SCORE_CHANGED,
                json!({"tick": 9, "players": [{"index": 0, "score": 1, "kills": 1, "deaths": 0, "assists": 0}], "teams": [{"team": 0, "score": 1}]}),
            ),
            (
                GAME_ENDED,
                json!({"tick": 99, "reason": "finished", "winner": {"team": 0}, "players": []}),
            ),
            (
                GAME_ENDED,
                json!({"tick": 99, "reason": "quit", "winner": null}),
            ),
        ] {
            assert_eq!(validate(kind, &data, &p), Ok(()), "{kind} {data}");
        }
        for (kind, data) in [
            (
                PLAYER_JOINED,
                json!({"player": {"index": 1, "name": "dup", "team": 0}}),
            ),
            (PLAYER_LEFT, json!({"index": 7})),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 1, "kind": "murder"}),
            ),
            (
                PLAYER_KILLED,
                json!({"tick": 9, "victim": 5, "kind": "kill"}),
            ),
            (PLAYER_KILLED, json!({"victim": 1, "kind": "kill"})),
            (
                SCORE_CHANGED,
                json!({"tick": 9, "players": [{"index": 4, "score": 1, "kills": 1, "deaths": 0, "assists": 0}]}),
            ),
            (GAME_ENDED, json!({"tick": 9, "reason": "bored"})),
            (
                GAME_ENDED,
                json!({"tick": 9, "reason": "finished", "winner": {"player": 9}}),
            ),
            (GAME_STARTED, json!({})),
            (GAME_FRAME, json!({})),
        ] {
            assert!(validate(kind, &data, &p).is_err(), "{kind} {data}");
        }
    }

    #[test]
    fn new_game_map_id_is_optional_and_omitted_when_none() {
        let g: NewGame = serde_json::from_value(json!({
            "map": "lockout", "mode": "multiplayer", "team_play": false, "players": []
        }))
        .unwrap();
        assert_eq!(g.map_id, None);
        assert!(serde_json::to_value(&g).unwrap().get("map_id").is_none());
        let id = crate::session::MapId([7; 32]);
        let modded = NewGame {
            map_id: Some(id),
            ..g.clone()
        };
        let v = serde_json::to_value(&modded).unwrap();
        assert_eq!(v["map"], "lockout", "map stays the base map's name");
        assert_eq!(v["map_id"], json!("07".repeat(32)));
        assert_eq!(serde_json::from_value::<NewGame>(v).unwrap(), modded);
        // GameStarted flattens it, so stored games carry it too.
        let started = GameStarted {
            game_id: "g".into(),
            host: "h".into(),
            game: modded.clone(),
        };
        let back: GameStarted =
            serde_json::from_value(serde_json::to_value(&started).unwrap()).unwrap();
        assert_eq!(back.game.map_id, Some(id));
    }

    #[test]
    fn new_game_skulls_are_optional_checked_and_kept() {
        let old =
            json!({"map": "03a_oldmombasa", "mode": "campaign", "team_play": false, "players": []});
        let g: NewGame = serde_json::from_value(old.clone()).unwrap();
        assert_eq!(g.skulls, None, "a payload from before the field");
        assert!(serde_json::to_value(&g).unwrap().get("skulls").is_none());
        let campaign = NewGame {
            skulls: Some(vec![0, 6, 12]),
            ..g.clone()
        };
        assert_eq!(validate_new(&campaign), Ok(()));
        // A campaign skull of our own, past the retail fifteen.
        assert_eq!(
            validate_new(&NewGame {
                skulls: Some(vec![6, 20]),
                ..g.clone()
            }),
            Ok(())
        );
        let v = serde_json::to_value(&campaign).unwrap();
        assert_eq!(v["skulls"], json!([0, 6, 12]));
        let started = GameStarted {
            game_id: "g".into(),
            host: "h".into(),
            game: campaign.clone(),
        };
        let back: GameStarted =
            serde_json::from_value(serde_json::to_value(&started).unwrap()).unwrap();
        assert_eq!(back.game.skulls, Some(vec![0, 6, 12]));
        let mp = NewGame {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            ..g.clone()
        };
        assert_eq!(
            validate_new(&NewGame {
                skulls: Some(vec![4, 6]),
                ..mp.clone()
            }),
            Ok(())
        );
        for (game, why) in [
            (
                NewGame {
                    skulls: Some(vec![]),
                    ..g.clone()
                },
                "empty is left out",
            ),
            (
                NewGame {
                    skulls: Some(vec![64]),
                    ..g.clone()
                },
                "past the limit",
            ),
            (
                NewGame {
                    skulls: Some(vec![6, 0]),
                    ..g.clone()
                },
                "unsorted",
            ),
            (
                NewGame {
                    skulls: Some(vec![12]),
                    ..mp.clone()
                },
                "Mythic does nothing in multiplayer",
            ),
        ] {
            assert!(validate_new(&game).is_err(), "{why}");
        }
    }

    #[test]
    fn new_games_and_frames() {
        let g: NewGame = serde_json::from_value(json!({
            "console": "Den", "map": "lockout", "mode": "multiplayer", "variant": "Slayer",
            "team_play": true, "players": [{"index": 0, "name": "A", "team": 0}]
        }))
        .unwrap();
        assert_eq!(validate_new(&g), Ok(()));
        assert!(
            validate_new(&NewGame {
                map: "a/b".into(),
                ..g.clone()
            })
            .is_err()
        );
        assert!(
            validate_new(&NewGame {
                mode: "firefight".into(),
                ..g.clone()
            })
            .is_err()
        );
        assert!(
            validate_new(&NewGame {
                players: vec![g.players[0].clone(), g.players[0].clone()],
                ..g.clone()
            })
            .is_err()
        );

        assert_eq!(g.kind(), "custom", "inferred for old senders");
        assert_eq!(serde_json::to_value(&g).unwrap().get("kind"), None);

        let started = GameStarted {
            game_id: "g1".into(),
            host: "acct:x@h".into(),
            game: g,
        };
        let v = serde_json::to_value(&started).unwrap();
        assert_eq!(v["game_id"], "g1");
        assert_eq!(v["map"], "lockout");
        assert_eq!(serde_json::from_value::<GameStarted>(v).unwrap(), started);

        let f: Frame = serde_json::from_value(json!({"game_id": "g1", "tick": 30, "players": [{"index": 0, "alive": true, "pos": [1, 2, 3], "yaw": 1.5}]})).unwrap();
        assert_eq!(validate_frame(&f), Ok(()));
        assert!(valid_id("7f3c-a_B"));
        assert!(!valid_id("") && !valid_id("a b") && !valid_id(&"x".repeat(65)));
    }

    #[test]
    fn frame_players_carry_species_aim_weapon_and_shots() {
        let wire = json!({"game_id": "g1", "tick": 30, "players": [
            {"index": 0, "alive": true, "pos": [1.0, 2.0, 3.0], "yaw": 1.5,
             "species": "elite", "aim": [0.6, 0.8, 0.0], "weapon": "battle_rifle", "shots": 3},
            {"index": 1, "alive": false, "pos": [0.0, 0.0, 0.0], "yaw": 0.0}
        ]});
        let frame: Frame = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(frame.players[0].species.as_deref(), Some("elite"));
        assert_eq!(frame.players[0].aim, Some([0.6, 0.8, 0.0]));
        assert_eq!(frame.players[0].weapon.as_deref(), Some("battle_rifle"));
        assert_eq!(frame.players[0].shots, Some(3));
        assert_eq!(
            frame.players[1],
            FramePlayer {
                index: 1,
                ..FramePlayer::default()
            }
        );
        assert_eq!(validate_frame(&frame), Ok(()));
        // Round trip: absent fields stay absent, present ones keep their shape.
        assert_eq!(serde_json::to_value(&frame).unwrap(), wire);

        // Zero shots are sent as absent.
        let quiet = FramePlayer {
            shots: Some(0),
            ..FramePlayer::default()
        };
        assert!(serde_json::to_value(&quiet).unwrap().get("shots").is_none());
    }

    fn campaign_frame() -> Value {
        json!({"game_id": "g1", "tick": 30,
        "players": [{"index": 0, "alive": true, "pos": [1.0, 2.0, 3.0], "yaw": 0.5,
                     "vehicle": "90040004"}],
        "vehicles": [{"id": "90040004", "kind": "warthog", "pos": [1.0, 2.0, 3.0],
                      "forward": [1.0, 0.0, 0.0], "up": [0.0, 0.0, 1.0], "occupied": true}],
        "actors": [
            {"id": "90070007", "species": "grunt", "pos": [5.0, 6.0, 7.0], "yaw": 1.0,
             "aim": [0.0, 1.0, 0.0], "weapon": "plasma_pistol", "shots": 2,
             "team": "covenant"},
            {"id": "90080008", "species": "marine", "pos": [1.0, 2.0, 3.0], "yaw": 0.0,
             "team": "human", "vehicle": "90040004"}
        ]})
    }

    #[test]
    fn frames_carry_vehicles_and_actors() {
        let wire = campaign_frame();
        let frame: Frame = serde_json::from_value(wire.clone()).unwrap();
        assert_eq!(frame.vehicles[0].kind, "warthog");
        assert_eq!(frame.actors[0].team.as_deref(), Some("covenant"));
        assert_eq!(frame.actors[1].vehicle.as_deref(), Some("90040004"));
        assert_eq!(validate_frame(&frame), Ok(()));
        assert_eq!(serde_json::to_value(&frame).unwrap(), wire);
        // Old frames have neither, and they are not sent empty.
        let old: Frame =
            serde_json::from_value(json!({"game_id": "g1", "tick": 2, "players": []})).unwrap();
        assert!(old.vehicles.is_empty() && old.actors.is_empty());
        let sent = serde_json::to_value(&old).unwrap();
        assert!(sent.get("vehicles").is_none() && sent.get("actors").is_none());
    }

    #[test]
    fn vehicle_and_actor_validation() {
        let good: Frame = serde_json::from_value(campaign_frame()).unwrap();
        let mut bad = Vec::new();
        let mut f = good.clone();
        f.vehicles[0].up = [0.0; 3];
        bad.push(f);
        let mut f = good.clone();
        f.vehicles[0].forward[0] = f64::NAN;
        bad.push(f);
        let mut f = good.clone();
        f.vehicles[0].kind = "Warthog".into();
        bad.push(f);
        let mut f = good.clone();
        f.vehicles[0].pos[2] = f64::INFINITY;
        bad.push(f);
        let mut f = good.clone();
        f.vehicles = vec![good.vehicles[0].clone(); MAX_FRAME_VEHICLES + 1];
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].species = String::new();
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].team = Some("-1".into());
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].aim = Some([f64::NAN, 0.0, 0.0]);
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].yaw = f64::NAN;
        bad.push(f);
        let mut f = good.clone();
        f.actors[0].shots = Some(MAX_FRAME_SHOTS + 1);
        bad.push(f);
        let mut f = good.clone();
        f.actors = vec![good.actors[1].clone(); MAX_FRAME_ACTORS + 1];
        bad.push(f);
        let mut f = good.clone();
        f.players[0].vehicle = Some("not hex!".into());
        bad.push(f);
        for f in bad {
            assert!(validate_frame(&f).is_err(), "{f:?}");
        }
        let mut f = good;
        f.vehicles[0].up = [0.0, 0.0, 2.0];
        f.actors[0].aim = Some([0.0, 3.0, 4.0]);
        f.actors[0].shots = Some(0);
        normalize_frame(&mut f);
        assert_eq!(f.vehicles[0].up, [0.0, 0.0, 1.0]);
        assert_eq!(f.actors[0].aim, Some([0.0, 0.6, 0.8]));
        assert_eq!(f.actors[0].shots, None);
    }

    #[test]
    fn dropped_frames_carry_their_shots_forward() {
        let player = |index, shots| FramePlayer {
            index,
            shots,
            ..FramePlayer::default()
        };
        let actor = |id: &str, shots| FrameActor {
            id: id.into(),
            species: "grunt".into(),
            shots,
            ..FrameActor::default()
        };
        let dropped = Frame {
            game_id: "g1".into(),
            tick: 2,
            players: vec![player(0, Some(2)), player(1, None), player(3, Some(5))],
            actors: vec![actor("9001000a", Some(4)), actor("9002000b", Some(1))],
            ..Frame::default()
        };
        let mut next = Frame {
            game_id: "g1".into(),
            tick: 4,
            players: vec![
                player(0, Some(1)),
                player(1, None),
                player(2, Some(MAX_FRAME_SHOTS)),
            ],
            actors: vec![actor("9001000a", None)],
            ..Frame::default()
        };
        assert_eq!(
            dropped.shots().collect::<Vec<_>>(),
            [
                (Shooter::Player(0), 2),
                (Shooter::Player(3), 5),
                (Shooter::Actor("9001000a".into()), 4),
                (Shooter::Actor("9002000b".into()), 1),
            ]
        );
        next.add_shots(dropped.shots().collect::<Vec<_>>());
        next.add_shots([(Shooter::Player(2), 9)]);
        assert_eq!(
            next.players.iter().map(|p| p.shots).collect::<Vec<_>>(),
            [Some(3), None, Some(MAX_FRAME_SHOTS)],
            "player 3 left; counts cap"
        );
        assert_eq!(next.actors[0].shots, Some(4), "actor 9002000b is gone");
    }

    #[test]
    fn frame_validation_rejects_bad_aim_ids_and_shot_counts() {
        let with = |p: FramePlayer| Frame {
            game_id: "g1".into(),
            tick: 2,
            players: vec![p],
            ..Frame::default()
        };
        let ok = FramePlayer {
            species: Some("spartan".into()),
            aim: Some([0.0, 0.0, 2.0]),
            weapon: Some("smg".into()),
            shots: Some(MAX_FRAME_SHOTS),
            ..FramePlayer::default()
        };
        assert_eq!(validate_frame(&with(ok.clone())), Ok(()));
        for bad in [
            FramePlayer {
                aim: Some([f64::NAN, 0.0, 1.0]),
                ..ok.clone()
            },
            FramePlayer {
                aim: Some([f64::INFINITY, 0.0, 0.0]),
                ..ok.clone()
            },
            FramePlayer {
                aim: Some([0.0; 3]),
                ..ok.clone()
            },
            FramePlayer {
                species: Some("Elite".into()),
                ..ok.clone()
            },
            FramePlayer {
                weapon: Some("objects\\weapons\\smg".into()),
                ..ok.clone()
            },
            FramePlayer {
                weapon: Some(String::new()),
                ..ok.clone()
            },
            FramePlayer {
                weapon: Some("x".repeat(33)),
                ..ok.clone()
            },
            FramePlayer {
                shots: Some(MAX_FRAME_SHOTS + 1),
                ..ok.clone()
            },
        ] {
            assert!(validate_frame(&with(bad.clone())).is_err(), "{bad:?}");
        }

        // Stored frames have unit aim and no zero shot count.
        let mut frame = with(FramePlayer {
            shots: Some(0),
            ..ok
        });
        normalize_frame(&mut frame);
        assert_eq!(frame.players[0].aim, Some([0.0, 0.0, 1.0]));
        assert_eq!(frame.players[0].shots, None);
    }

    #[test]
    fn winners_are_untagged() {
        assert_eq!(
            serde_json::to_value(Winner::Team { team: 1 }).unwrap(),
            json!({"team": 1})
        );
        assert_eq!(
            serde_json::from_value::<Winner>(json!({"player": 3})).unwrap(),
            Winner::Player { player: 3 }
        );
    }

    #[test]
    fn a_matchmaking_game_names_its_live_session() {
        let mm = NewGame {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            kind: Some("matchmaking".into()),
            playlist_id: Some("1".into()),
            session_id: Some("0123456789abcdef".into()),
            ..NewGame::default()
        };
        assert_eq!(validate_new(&mm), Ok(()));
        let upper = NewGame {
            session_id: Some("0123456789ABCDEF".into()),
            ..mm.clone()
        };
        assert!(validate_new(&upper).is_err());
        let campaign = NewGame {
            mode: "campaign".into(),
            kind: Some("campaign".into()),
            playlist_id: None,
            ..mm.clone()
        };
        assert!(validate_new(&campaign).is_err());
        let custom = NewGame {
            kind: Some("custom".into()),
            playlist_id: None,
            ..mm.clone()
        };
        assert_eq!(
            validate_new(&custom),
            Ok(()),
            "a custom game with bots names one too"
        );
    }

    #[test]
    fn bots_come_with_their_session_and_xuid() {
        let bot = Player {
            index: 1,
            name: "Clanker".into(),
            xuid: Some(Xuid(0x000b_0000_0000_0001)),
            team: 0,
            local: Some(true),
            appearance: None,
            bot: Some(true),
        };
        let game = NewGame {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            kind: Some("custom".into()),
            session_id: Some("0123456789abcdef".into()),
            players: vec![bot.clone()],
            ..NewGame::default()
        };
        assert_eq!(validate_new(&game), Ok(()));
        let no_session = NewGame {
            session_id: None,
            ..game.clone()
        };
        assert!(validate_new(&no_session).is_err());
        // A host's own Clanker: a bot with no account, with or without a
        // session.
        let own = NewGame {
            players: vec![Player { xuid: None, ..bot }],
            ..game
        };
        assert_eq!(validate_new(&own), Ok(()));
        assert_eq!(
            validate_new(&NewGame {
                session_id: None,
                ..own
            }),
            Ok(())
        );
    }

    #[test]
    fn game_kinds_playlists_and_difficulty() {
        let campaign = NewGame {
            map: "03a_oldmombasa".into(),
            mode: "campaign".into(),
            ..NewGame::default()
        };
        let custom = NewGame {
            map: "lockout".into(),
            mode: "multiplayer".into(),
            ..NewGame::default()
        };
        assert_eq!(campaign.kind(), "campaign");
        let ok = [
            NewGame {
                kind: Some("campaign".into()),
                difficulty: Some("legendary".into()),
                online: Some(false),
                ..campaign.clone()
            },
            NewGame {
                difficulty: Some("easy".into()),
                ..campaign.clone()
            },
            NewGame {
                kind: Some("custom".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("matchmaking".into()),
                playlist: Some("Team Slayer".into()),
                playlist_id: Some("3".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("clankermatch".into()),
                playlist: Some("Clanker Slayer".into()),
                playlist_id: Some("9".into()),
                session_id: Some("00000000000000a1".into()),
                ..custom.clone()
            },
        ];
        for g in &ok {
            assert_eq!(validate_new(g), Ok(()), "{g:?}");
        }
        let v = serde_json::to_value(&ok[3]).unwrap();
        assert_eq!(v["kind"], "matchmaking");
        assert_eq!(v["playlist"], "Team Slayer");
        assert_eq!(serde_json::from_value::<NewGame>(v).unwrap(), ok[3]);
        let v = serde_json::to_value(&ok[4]).unwrap();
        assert_eq!(v["kind"], "clankermatch");
        assert_eq!(v["playlist_id"], "9");
        assert_eq!(serde_json::from_value::<NewGame>(v).unwrap(), ok[4]);
        assert!(GAME_KINDS.contains(&"clankermatch"));
        assert!(is_matchmade("clankermatch") && is_matchmade("matchmaking"));
        assert!(!is_matchmade("custom") && !is_matchmade("campaign"));

        let bad = [
            NewGame {
                kind: Some("firefight".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("campaign".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("custom".into()),
                ..campaign.clone()
            },
            NewGame {
                playlist: Some("Rumble Pit".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("matchmaking".into()),
                playlist: Some("x".repeat(33)),
                ..custom.clone()
            },
            NewGame {
                kind: Some("clankermatch".into()),
                ..campaign.clone()
            },
            NewGame {
                kind: Some("custom".into()),
                playlist_id: Some("9".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("clankermatch".into()),
                playlist_id: Some("a b".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("matchmaking".into()),
                playlist: Some("Team\nSlayer".into()),
                ..custom.clone()
            },
            NewGame {
                kind: Some("matchmaking".into()),
                playlist_id: Some("a b".into()),
                ..custom.clone()
            },
            NewGame {
                difficulty: Some("normal".into()),
                ..custom.clone()
            },
            NewGame {
                difficulty: Some("mythic".into()),
                ..campaign.clone()
            },
            NewGame {
                online: Some(true),
                ..custom.clone()
            },
        ];
        for g in &bad {
            assert!(validate_new(g).is_err(), "{g:?}");
        }
    }

    fn motion_frame() -> Frame {
        Frame {
            game_id: "g1".into(),
            tick: 30,
            players: vec![FramePlayer {
                index: 0,
                alive: true,
                stance: Some("crouch".into()),
                speed: Some(4.5),
                actions: Some(vec!["reload".into()]),
                grenade: Some("frag".into()),
                ..FramePlayer::default()
            }],
            actors: vec![FrameActor {
                id: "90070007".into(),
                species: "grunt".into(),
                stance: Some("airborne".into()),
                actions: Some(vec!["jump".into()]),
                ..FrameActor::default()
            }],
            hits: vec![FrameHit {
                from: Some("p0".into()),
                to: Some("90070007".into()),
                weapon: Some("battle_rifle".into()),
                pos: [1.0, 2.0, 3.0],
                amount: Some(0.4),
                kind: Some("bullet".into()),
                ago: Some(1),
            }],
            bsp: Some(2),
            machines: vec![FrameMachine {
                node: Some("scarab_3".into()),
                id: "90080008".into(),
                kind: "scarab".into(),
                pos: [0.0; 3],
                forward: [2.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                position: Some(0.5),
            }],
            ..Frame::default()
        }
    }

    #[test]
    fn stance_actions_hits_bsp_and_machines() {
        let good = motion_frame();
        assert_eq!(validate_frame(&good), Ok(()));
        let v = serde_json::to_value(&good).unwrap();
        assert_eq!(v["bsp"], 2);
        assert_eq!(v["players"][0]["actions"], json!(["reload"]));
        assert_eq!(serde_json::from_value::<Frame>(v).unwrap(), good);

        let bad: [fn(&mut Frame); 12] = [
            |f| f.players[0].stance = Some("prone".into()),
            |f| f.players[0].speed = Some(f64::NAN),
            |f| f.players[0].speed = Some(-1.0),
            |f| f.players[0].actions = Some(vec!["dance".into()]),
            |f| f.actors[0].actions = Some(vec!["jump".into(); 9]),
            |f| f.actors[0].grenade = Some("spike".into()),
            |f| f.hits[0].from = Some("P 0".into()),
            |f| f.hits[0].amount = Some(-0.1),
            |f| f.hits = vec![f.hits[0].clone(); MAX_FRAME_HITS + 1],
            |f| f.machines[0].position = Some(1.5),
            |f| f.machines[0].node = Some("Scarab 3".into()),
            |f| f.machines[0].up = [0.0; 3],
        ];
        for (i, change) in bad.iter().enumerate() {
            let mut f = motion_frame();
            change(&mut f);
            assert!(validate_frame(&f).is_err(), "case {i}");
        }

        let mut f = motion_frame();
        f.players[0].actions = Some(Vec::new());
        normalize_frame(&mut f);
        assert_eq!(f.players[0].actions, None);
        assert_eq!(f.machines[0].forward, [1.0, 0.0, 0.0]);

        // Old senders listed only occupied vehicles.
        let old: FrameVehicle = serde_json::from_value(json!({
            "id": "a", "kind": "ghost", "pos": [0, 0, 0], "forward": [1, 0, 0], "up": [0, 0, 1]
        }))
        .unwrap();
        assert!(old.occupied);
    }

    #[test]
    fn dropped_frames_carry_actions_hits_and_machines() {
        let older = motion_frame();
        let mut newer = motion_frame();
        newer.players[0].actions = Some(vec!["switch".into()]);
        newer.hits[0].amount = Some(0.1);
        newer.machines[0].position = Some(0.75);
        let mut carry = FrameCarry::default();
        assert!(carry.is_empty());
        carry.add(&older);
        newer.absorb(carry);
        assert_eq!(
            newer.players[0].actions,
            Some(vec!["reload".into(), "switch".into()]),
            "oldest first"
        );
        assert_eq!(
            newer.actors[0].actions,
            Some(vec!["jump".into(), "jump".into()])
        );
        let amounts: Vec<_> = newer.hits.iter().map(|h| h.amount).collect();
        assert_eq!(amounts, [Some(0.4), Some(0.1)]);
        assert_eq!(newer.machines.len(), 1);
        assert_eq!(
            newer.machines[0].position,
            Some(0.75),
            "the newer state wins"
        );

        // Caps: the oldest go.
        let mut carry = FrameCarry::default();
        for _ in 0..10 {
            carry.add(&older);
        }
        let mut f = motion_frame();
        f.hits = vec![f.hits[0].clone(); MAX_FRAME_HITS];
        f.absorb(carry);
        assert_eq!(
            f.players[0].actions.as_ref().unwrap().len(),
            MAX_FRAME_ACTIONS
        );
        assert_eq!(f.hits.len(), MAX_FRAME_HITS);
        assert_eq!(validate_frame(&f), Ok(()));

        // A machine only the dropped frame had is kept.
        let mut carry = FrameCarry::default();
        carry.add(&older);
        let mut f = motion_frame();
        f.machines[0].id = "90090009".into();
        f.absorb(carry);
        assert_eq!(f.machines.len(), 2);
    }
}
