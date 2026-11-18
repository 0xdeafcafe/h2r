use live_proto::game::{Frame, ObjectKind, PlayerScore, TeamScore, Winner};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One game event before server-specific signing or file serialization.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GameEvent {
    pub game_id: String,
    #[serde(rename = "type")]
    pub kind: String,
    pub data: Value,
}

impl GameEvent {
    pub fn new(game_id: impl Into<String>, kind: impl Into<String>, data: impl Serialize) -> Self {
        Self {
            game_id: game_id.into(),
            kind: kind.into(),
            data: serde_json::to_value(data).expect("telemetry event data serializes"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerStats {
    pub score: i64,
    pub kills: u32,
    pub deaths: u32,
    pub assists: u32,
    pub suicides: u32,
}

impl PlayerStats {
    pub const ZERO: Self = Self {
        score: 0,
        kills: 0,
        deaths: 0,
        assists: 0,
        suicides: 0,
    };

    pub fn wire(&self, index: u32) -> PlayerScore {
        PlayerScore {
            index,
            score: self.score,
            kills: self.kills,
            deaths: self.deaths,
            assists: self.assists,
        }
    }
}

/// A player's appearance read from the signed-in profile of their local
/// player slot (docs/live/live-telemetry.md, "Player appearance"). The game's
/// rank is an Xbox Live attribute, not guest memory, and is not read.
pub use live_proto::game::Appearance;

#[derive(Debug, Clone, PartialEq)]
pub struct PlayerState {
    pub datum: u32,
    pub player: live_proto::game::Player,
    pub appearance: Appearance,
    /// The local player slot (0..3) this player plays from, or `None` for a
    /// remote player.
    pub local_user: Option<u8>,
    /// The player's machine id (player `+0x14`): a phantom bot's is its
    /// own made-up one (docs/game/bots.md §5.1).
    pub machine: [u8; 6],
    pub unit: u32,
    pub stats: PlayerStats,
}

impl PlayerState {
    /// The player as roster events send it, with the appearance when any of
    /// it is known.
    pub fn wire_player(&self) -> live_proto::game::Player {
        live_proto::game::Player {
            appearance: self.appearance.known(),
            ..self.player.clone()
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ObjectState {
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub vehicle: Option<u32>,
    /// The vehicle `vehicle` is mounted on, when it is a turret riding
    /// another (a Warthog gunner's chaingun): `pos`, `forward` and `up` are
    /// then that one's.
    pub mount: Option<u32>,
    /// The unit's own definition tag datum (the biped's, also in a vehicle).
    pub definition: Option<u32>,
    /// The unit's aiming vector, unit length; `None` when the read value is
    /// not a finite, near-unit vector.
    pub aim: Option<[f64; 3]>,
    /// The weapon the unit holds.
    pub weapon: Option<Weapon>,
    /// The up vector of the transform in `pos`/`forward` (the vehicle's
    /// when riding one).
    pub up: [f64; 3],
    /// The ridden vehicle's definition tag datum.
    pub vehicle_definition: Option<u32>,
    /// The unit's team (`+0x138`); `None` when negative.
    pub team: Option<i16>,
    /// Movement and inventory state for `stance` and `actions`.
    pub motion: Motion,
    /// The velocity of what `pos` is (the vehicle's, or the mount's, when
    /// riding; else the unit's own), world units a second (body `+0x88`);
    /// `None` when its read faulted or is not finite.
    pub velocity: Option<[f64; 3]>,
    /// The seat of `vehicle` the unit is in (`+0x1fc`); `None` when -1 or
    /// not read.
    pub seat: Option<u16>,
    /// Body and shield vitality (`+0xec`, `+0xf0`; 1.0 full), both 0 once
    /// the body is depleted, as `object_get_health` reads them; `None` when
    /// not read or not finite.
    pub vitality: Option<(f64, f64)>,
}

/// What a unit's stance and one-shot actions are worked out from (each
/// `None` when its read faulted or made no sense).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Motion {
    /// The unit's own velocity (not its vehicle's).
    pub velocity: [f64; 3],
    /// Crouch amount 0..1.
    pub crouch: Option<f64>,
    /// Biped physics mode (bipeds only).
    pub physics_mode: Option<u8>,
    /// Selected grenade type index.
    pub grenade: Option<i8>,
    /// Grenades carried, all types.
    pub grenades: Option<u32>,
    /// The four weapon slots' object datums (`u32::MAX` empty).
    pub weapons: Option<[u32; 4]>,
    /// The held weapon's first magazine: (reserve, loaded).
    pub magazine: Option<(i16, i16)>,
    /// Raw unit flags (`layout::UNIT_FLAGS`), for diagnostics.
    pub unit_flags: Option<u32>,
    /// Bipeds only: raw biped flags (`layout::BIPED_FLAGS`).
    pub biped_flags: Option<u16>,
    /// Bipeds only: airborne tick count (`layout::BIPED_AIRBORNE_TICKS`).
    pub airborne_ticks: Option<i8>,
    /// The animation the unit plays.
    pub animation: Option<Animation>,
    /// Bipeds only: hitbox offset counter (`layout::BIPED_HITBOX_OFFSET`).
    pub hitbox_offset: Option<i8>,
    /// The first Havok rigid body's world position and linear velocity
    /// (`layout::BODY_PHYSICS`), when the object has one.
    pub havok: Option<([f64; 3], [f64; 3])>,
}

/// An animation manager's first channel, as the guest holds it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Animation {
    /// The channel's `jmad` tag datum.
    pub graph: u32,
    /// -1 the graph itself, else a position in its inheritance list.
    pub inherited: i16,
    /// The animation's index in that graph.
    pub index: i16,
    /// The frame, 0 to the frame count minus one.
    pub frame: f32,
}

/// A vehicle anywhere in the world.
#[derive(Debug, Clone, PartialEq)]
pub struct VehicleState {
    pub datum: u32,
    pub definition: Option<u32>,
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
    /// A biped (player or AI) is parented to it.
    pub occupied: bool,
    /// The vehicle it is mounted on (a turret: the Warthog's chaingun, the
    /// Scorpion's cannon); its transform is then parent-relative.
    pub parent: Option<u32>,
    /// Its own aiming vector (a vehicle is a unit), normalised: where a
    /// turret points.
    pub aim: Option<[f64; 3]>,
}

/// A machine (door, elevator, scarab, ...).
#[derive(Debug, Clone, PartialEq)]
pub struct MachineState {
    pub datum: u32,
    pub definition: Option<u32>,
    /// The scenario placement index, when the scenario placed it.
    pub placement: Option<u16>,
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
    /// Device position 0..1, when it reads as one.
    pub position: Option<f64>,
}

/// One damage report, as the damage hook saw it.
#[derive(Debug, Clone, PartialEq)]
pub struct HitRecord {
    pub code: u8,
    pub effect: u32,
    pub owner_player: u32,
    pub owner_object: u32,
    pub victim: u32,
    pub pos: Option<[f64; 3]>,
    pub body: f32,
    pub shield: f32,
    /// The engine tick the hook saw it on, when it could be read.
    pub tick: Option<u64>,
}

/// Everything the once-per-frame object table scan finds.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct World {
    pub actors: Vec<ActorState>,
    pub vehicles: Vec<VehicleState>,
    pub machines: Vec<MachineState>,
    /// Objects from scenario placements and netgame equipment.
    pub placed: Vec<PlacedObject>,
    /// Dead bipeds (body depleted) the game has not cleared away yet:
    /// AI and players' bodies alike.
    pub bodies: Vec<ActorState>,
}

/// An object the scenario placed (or a netgame equipment entry spawned):
/// which placement, and where it is now.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlacedObject {
    pub kind: ObjectKind,
    pub index: u16,
    /// It has a parent: a held weapon, a passenger, a turret. Its own
    /// transform is then parent-relative.
    pub parented: bool,
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
}

/// Where a placement puts its object.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub pos: [f64; 3],
    pub forward: [f64; 3],
    pub up: [f64; 3],
}

/// The loaded scenario's placements, by kind (in [`ObjectKind`] order):
/// each block's entries in order, `None` where a record does not read as
/// a pose.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Placements {
    pub kinds: Vec<(ObjectKind, Vec<Option<Pose>>)>,
}

/// An AI-controlled biped: its object datum and state.
#[derive(Debug, Clone, PartialEq)]
pub struct ActorState {
    pub datum: u32,
    pub object: ObjectState,
}

/// A held weapon: its object datum and its definition tag datum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Weapon {
    pub datum: u32,
    pub definition: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct GameInfo {
    pub map: String,
    /// Campaign only: `easy`, `normal`, `heroic` or `legendary`.
    pub difficulty: Option<String>,
    /// Campaign only: the skulls on in the skull array (`NewGame.skulls`).
    pub skulls: Option<Vec<u8>>,
    pub mode: String,
    pub variant: Option<String>,
    pub team_play: bool,
    pub score_limit: Option<i64>,
    pub time_limit_s: Option<i64>,
    pub tick_rate: u16,
    pub tick: u64,
}

impl GameInfo {
    pub fn is_match(&self) -> bool {
        matches!(self.mode.as_str(), "campaign" | "multiplayer")
    }
}

/// Shots by weapon object datum, in datum order (as a `BTreeMap` iterates):
/// a sorted list, so clearing it keeps its room and the game thread's
/// weapon-fire hook does not allocate once it has grown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Shots(Vec<(u32, u32)>);

impl Shots {
    /// Room the first shot reserves: more weapons than fire in one tick.
    const ROOM: usize = 64;

    pub fn get(&self, weapon: &u32) -> Option<&u32> {
        self.find(*weapon).ok().map(|i| &self.0[i].1)
    }

    pub fn contains_key(&self, weapon: &u32) -> bool {
        self.find(*weapon).is_ok()
    }

    /// Adds `n` shots of `weapon` (saturating).
    pub fn add(&mut self, weapon: u32, n: u32) {
        match self.find(weapon) {
            Ok(i) => self.0[i].1 = self.0[i].1.saturating_add(n),
            Err(i) => {
                if self.0.capacity() == 0 {
                    self.0.reserve(Self::ROOM);
                }
                self.0.insert(i, (weapon, n));
            }
        }
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Empties it, keeping its room.
    pub fn clear(&mut self) {
        self.0.clear();
    }

    pub fn iter(&self) -> impl Iterator<Item = (&u32, &u32)> {
        self.0.iter().map(|(w, n)| (w, n))
    }

    fn find(&self, weapon: u32) -> Result<usize, usize> {
        self.0.binary_search_by_key(&weapon, |(w, _)| *w)
    }
}

impl<'a> IntoIterator for &'a Shots {
    type Item = (&'a u32, &'a u32);
    type IntoIter =
        std::iter::Map<std::slice::Iter<'a, (u32, u32)>, fn(&'a (u32, u32)) -> (&'a u32, &'a u32)>;

    fn into_iter(self) -> Self::IntoIter {
        let pair: fn(&'a (u32, u32)) -> (&'a u32, &'a u32) = |(w, n)| (w, n);
        self.0.iter().map(pair)
    }
}

/// Later counts of the same weapon add up.
impl FromIterator<(u32, u32)> for Shots {
    fn from_iter<I: IntoIterator<Item = (u32, u32)>>(iter: I) -> Self {
        let mut shots = Shots::default();
        for (weapon, n) in iter {
            shots.add(weapon, n);
        }
        shots
    }
}

impl<const N: usize> From<[(u32, u32); N]> for Shots {
    fn from(shots: [(u32, u32); N]) -> Self {
        shots.into_iter().collect()
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    pub tick: u64,
    pub has_stats: bool,
    pub players: Vec<PlayerState>,
    pub teams: Vec<TeamScore>,
    pub objects: Vec<Option<ObjectState>>,
    /// Shots fired since the previous snapshot, by weapon object datum
    /// (from the weapon-fire hook; empty when it is not installed).
    pub shots: Shots,
    /// AI-controlled bipeds; read on frame ticks only, empty otherwise.
    pub actors: Vec<ActorState>,
    /// Every vehicle; frame ticks only.
    pub vehicles: Vec<VehicleState>,
    /// Every machine; frame ticks only.
    pub machines: Vec<MachineState>,
    /// Objects from placements; frame ticks only.
    pub placed: Vec<PlacedObject>,
    /// Dead bipeds still in the world; frame ticks only.
    pub bodies: Vec<ActorState>,
    /// The active structure BSP index.
    pub bsp: Option<u32>,
    /// Damage reports since the previous snapshot.
    pub hits: Vec<HitRecord>,
    /// A HaloScript cinematic is playing (`memory::cinematic`): frames
    /// carry no entities, and kills, hits and shots are not counted.
    pub cinematic: bool,
}

impl Snapshot {
    pub fn scores(&self) -> Vec<PlayerScore> {
        self.players
            .iter()
            .map(|p| p.stats.wire(p.player.index))
            .collect()
    }

    pub fn winner(&self, team_play: bool) -> Option<Winner> {
        if team_play {
            let mut rows = self.teams.clone();
            rows.sort_by_key(|t| t.team);
            let max = rows.iter().map(|t| t.score).max()?;
            let mut best = rows.into_iter().filter(|t| t.score == max);
            let first = best.next()?;
            if best.next().is_some() {
                None
            } else {
                Some(Winner::Team { team: first.team })
            }
        } else {
            let max = self.players.iter().map(|p| p.stats.score).max()?;
            let mut best = self.players.iter().filter(|p| p.stats.score == max);
            let first = best.next()?;
            if best.next().is_some() {
                None
            } else {
                Some(Winner::Player {
                    player: first.player.index,
                })
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Output {
    Event(GameEvent),
    Frame(Frame),
}
