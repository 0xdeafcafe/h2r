//! The scenario's multiplayer parts, and item collections.
//!
//! Offsets and strides were read off retail multiplayer maps (a block's
//! stride is the gap to the next block over its count; semantics from the
//! values). Evidence per field is in `docs/formats/h1-map-format.md`,
//! "Scenario".

use crate::format::*;
use crate::reader::H1Map;
use crate::space::{Space, f32_at, vec3};
use crate::{Error, Result};

/// Scenario block offsets and element sizes.
pub mod layout {
    /// Skies: a `sky ` reference each.
    pub const SKIES: (usize, usize) = (0x30, 0x10);
    /// Scenery placements and palette.
    pub const SCENERY: (usize, usize) = (0x210, 0x48);
    /// Scenery palette.
    pub const SCENERY_PALETTE: (usize, usize) = (0x21c, 0x30);
    /// Vehicle placements. The offset follows the scenery/biped pattern
    /// (placements and palette alternate every 0xc); its stride is not yet
    /// checked against a map that has vehicles.
    pub const VEHICLES: (usize, usize) = (0x240, 0x78);
    /// Vehicle palette.
    pub const VEHICLE_PALETTE: (usize, usize) = (0x24c, 0x30);
    /// Player starting locations.
    pub const STARTING_LOCATIONS: (usize, usize) = (0x354, 0x34);
    /// Netgame flags.
    pub const NETGAME_FLAGS: (usize, usize) = (0x378, 0x94);
    /// Netgame equipment.
    pub const NETGAME_EQUIPMENT: (usize, usize) = (0x384, 0x90);
    /// Starting equipment.
    pub const STARTING_EQUIPMENT: (usize, usize) = (0x390, 0xcc);
    /// Item collection permutations (in the `itmc` tag, at its start).
    pub const ITEM_PERMUTATIONS: (usize, usize) = (0x00, 0x54);
}

/// A Halo 1 game type, as the scenario stores it on spawn points and
/// equipment (`u16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GameType(pub u16);

impl GameType {
    /// None set.
    pub const NONE: Self = Self(0);
    /// Every game type.
    pub const ALL: Self = Self(12);
    /// The name, for the values the converter maps.
    pub fn name(self) -> &'static str {
        match self.0 {
            0 => "none",
            1 => "ctf",
            2 => "slayer",
            3 => "oddball",
            4 => "king",
            5 => "race",
            12 => "all",
            13 => "all except ctf",
            14 => "all except race and ctf",
            _ => "other",
        }
    }
}

/// A player starting location (`0x34` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct StartingLocation {
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Team.
    pub team: u16,
    /// BSP index.
    pub bsp: u16,
    /// Up to four game types the location is used in.
    pub game_types: [GameType; 4],
}

/// Netgame flag kinds (`u16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlagKind {
    /// CTF flag spawn.
    CtfFlag,
    /// CTF vehicle.
    CtfVehicle,
    /// Oddball ball spawn.
    OddballBall,
    /// Race track checkpoint.
    RaceTrack,
    /// Race vehicle.
    RaceVehicle,
    /// Vegas bank (unused on Xbox).
    VegasBank,
    /// Teleporter entrance; the team field is the channel.
    TeleportFrom,
    /// Teleporter exit; the team field is the channel.
    TeleportTo,
    /// King of the hill boundary point.
    HillFlag,
    /// Any other value.
    Other(u16),
}

impl FlagKind {
    /// From the stored value.
    pub fn from_raw(v: u16) -> Self {
        match v {
            0 => Self::CtfFlag,
            1 => Self::CtfVehicle,
            2 => Self::OddballBall,
            3 => Self::RaceTrack,
            4 => Self::RaceVehicle,
            5 => Self::VegasBank,
            6 => Self::TeleportFrom,
            7 => Self::TeleportTo,
            8 => Self::HillFlag,
            v => Self::Other(v),
        }
    }
}

/// A netgame flag (`0x94` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct NetgameFlag {
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// Kind.
    pub kind: FlagKind,
    /// Team, or teleporter channel.
    pub team: u16,
    /// Weapon group (`itmc`), null on retail maps.
    pub weapon_group: TagRef,
}

/// A netgame equipment spawn (`0x90` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct NetgameEquipment {
    /// Flags (`u32` at `0`).
    pub flags: u32,
    /// Up to four game types.
    pub game_types: [GameType; 4],
    /// Team.
    pub team: u16,
    /// Respawn time in seconds; 0 means the item collection's.
    pub spawn_time: u16,
    /// Position.
    pub position: [f32; 3],
    /// Facing, radians.
    pub facing: f32,
    /// The item collection (`itmc`) spawned.
    pub item: TagRef,
}

/// A vehicle placement (`0x78` bytes): the common object placement fields.
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectPlacement {
    /// Palette index, `-1` none.
    pub palette: i16,
    /// Name index, `-1` none.
    pub name: i16,
    /// Position.
    pub position: [f32; 3],
    /// Rotation: yaw, pitch, roll in radians.
    pub rotation: [f32; 3],
    /// The element as stored (game type and team bits are in here; not yet
    /// interpreted).
    pub raw: Vec<u8>,
}

/// The scenario's multiplayer parts.
#[derive(Debug, Clone, PartialEq)]
pub struct Multiplayer {
    /// Skies (`sky `).
    pub skies: Vec<TagRef>,
    /// Player starting locations.
    pub starting_locations: Vec<StartingLocation>,
    /// Netgame flags.
    pub flags: Vec<NetgameFlag>,
    /// Netgame equipment.
    pub equipment: Vec<NetgameEquipment>,
    /// Vehicle palette (`vehi`).
    pub vehicle_palette: Vec<TagRef>,
    /// Vehicle placements.
    pub vehicles: Vec<ObjectPlacement>,
}

fn game_types(e: &[u8], at: usize) -> [GameType; 4] {
    std::array::from_fn(|i| GameType(u16_at(e, at + 2 * i).unwrap()))
}

impl Multiplayer {
    /// Reads the multiplayer parts of the scenario struct `scnr` in `space`.
    pub fn read(space: &Space, scnr: &[u8]) -> Result<Self> {
        let b = |(at, len): (usize, usize)| space.block_at(scnr, at, len);
        let tag_refs = |l| -> Result<Vec<TagRef>> {
            Ok(b(l)?
                .into_iter()
                .map(|e| TagRef::read(e, 0).unwrap())
                .collect())
        };
        Ok(Self {
            skies: tag_refs(layout::SKIES)?,
            starting_locations: b(layout::STARTING_LOCATIONS)?
                .into_iter()
                .map(|e| StartingLocation {
                    position: vec3(e, 0),
                    facing: f32_at(e, 0xc),
                    team: u16_at(e, 0x10).unwrap(),
                    bsp: u16_at(e, 0x12).unwrap(),
                    game_types: game_types(e, 0x14),
                })
                .collect(),
            flags: b(layout::NETGAME_FLAGS)?
                .into_iter()
                .map(|e| NetgameFlag {
                    position: vec3(e, 0),
                    facing: f32_at(e, 0xc),
                    kind: FlagKind::from_raw(u16_at(e, 0x10).unwrap()),
                    team: u16_at(e, 0x12).unwrap(),
                    weapon_group: TagRef::read(e, 0x14).unwrap(),
                })
                .collect(),
            equipment: b(layout::NETGAME_EQUIPMENT)?
                .into_iter()
                .map(|e| NetgameEquipment {
                    flags: u32_at(e, 0).unwrap(),
                    game_types: game_types(e, 4),
                    team: u16_at(e, 0xc).unwrap(),
                    spawn_time: u16_at(e, 0xe).unwrap(),
                    position: vec3(e, 0x40),
                    facing: f32_at(e, 0x4c),
                    item: TagRef::read(e, 0x50).unwrap(),
                })
                .collect(),
            vehicle_palette: tag_refs(layout::VEHICLE_PALETTE)?,
            vehicles: b(layout::VEHICLES)?
                .into_iter()
                .map(|e| ObjectPlacement {
                    palette: u16_at(e, 0).unwrap() as i16,
                    name: u16_at(e, 2).unwrap() as i16,
                    position: vec3(e, 8),
                    rotation: vec3(e, 0x14),
                    raw: e.to_vec(),
                })
                .collect(),
        })
    }
}

/// One entry of an item collection (`0x54` bytes).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemPermutation {
    /// Relative weight.
    pub weight: f32,
    /// The item: a `weap` or `eqip`.
    pub item: TagRef,
}

/// An item collection (`itmc`).
#[derive(Debug, Clone, PartialEq)]
pub struct ItemCollection {
    /// The items it picks from.
    pub permutations: Vec<ItemPermutation>,
    /// Default respawn time in seconds (`u16` at `0xc`), 0 for the game's.
    pub spawn_time: u16,
}

impl ItemCollection {
    /// Reads an `itmc` struct in `space`.
    pub fn read(space: &Space, itmc: &[u8]) -> Result<Self> {
        Ok(Self {
            permutations: space
                .block_at(
                    itmc,
                    layout::ITEM_PERMUTATIONS.0,
                    layout::ITEM_PERMUTATIONS.1,
                )?
                .into_iter()
                .map(|e| ItemPermutation {
                    weight: f32_at(e, 0x20),
                    item: TagRef::read(e, 0x24).unwrap(),
                })
                .collect(),
            spawn_time: u16_at(itmc, 0xc).unwrap_or(0),
        })
    }
}

impl H1Map {
    /// The tag data as an address space.
    pub fn tag_space(&self) -> Space<'_> {
        let at = self.header().index_offset as usize;
        let end = at + self.header().metadata_size as usize;
        Space::new(&self.bytes()[at..end], self.metadata_base())
    }

    /// The scenario's multiplayer parts.
    pub fn multiplayer(&self) -> Result<Multiplayer> {
        let scnr = self.scenario();
        let bytes = self.tag_data(scnr)?;
        if bytes.len() < layout::STARTING_EQUIPMENT.0 + BLOCK_LEN {
            return Err(Error::Invalid("scenario tag too short".into()));
        }
        Multiplayer::read(&self.tag_space(), bytes)
    }

    /// An item collection, by tag ID.
    pub fn item_collection(&self, id: TagId) -> Result<ItemCollection> {
        let t = self.tag(id)?;
        if t.group != Group::new(b"itmc") {
            return Err(Error::Invalid(format!(
                "{} is a {}, not an itmc",
                t.name, t.group
            )));
        }
        ItemCollection::read(&self.tag_space(), self.tag_data(t)?)
    }
}
