//! The scenario data Halo 1's scripts name, carried into Halo 2's scenario:
//! cutscene flags, cutscene camera points, cutscene titles and device groups,
//! plus the values script atoms of those types (and navpoints) take.
//!
//! Each Halo 1 block is read from the scenario and written as Halo 2's
//! block in the same order, so a script atom's value is the element's
//! index (a device group's is its runtime datum). Offsets and their evidence
//! are in `docs/formats/h1-scripts.md` ("Level data the scripts name").

use h1map::{Block as H1Block, H1Map};
use map::build::Block;

use crate::compile::Hook;
use crate::{Result, invalid, u32_at};

/// Halo 1 scenario fields: (offset, element size). Each element's name is a
/// 32-byte string; V on a30, every script token of the type found at the
/// index its atom holds (`tests/survey.rs`).
pub mod h1 {
    /// Cutscene flags: name `+4`, position `+0x24`, facing `+0x30`.
    pub const FLAGS: (usize, usize) = (0x4e4, 0x5c);
    /// Cutscene camera points: name `+4`, position `+0x28`, orientation
    /// `+0x34`.
    pub const CAMERA_POINTS: (usize, usize) = (0x4f0, 0x68);
    /// Cutscene titles: name `+4`.
    pub const TITLES: (usize, usize) = (0x4fc, 0x60);
    /// Device groups: name `+0`, initial value `+0x20`, flags `+0x24`.
    pub const DEVICE_GROUPS: (usize, usize) = (0x288, 0x34);
}

/// Halo 2 scenario fields: (offset, element size). V on 01b, 03a and 05a:
/// every script atom of the type names the element at its index.
pub mod h2 {
    /// Cutscene flags: name `+4` (32 bytes), position `+0x24`, facing
    /// `+0x30`.
    pub const FLAGS: (u32, usize) = (0x1e0, 0x38);
    /// Camera points: flags and type `+0`, name `+4`, position `+0x28`,
    /// orientation `+0x34`.
    pub const CAMERA_POINTS: (u32, usize) = (0x1e8, 0x40);
    /// Cutscene titles: name string ID `+0`, bounds `+4` (four shorts),
    /// colours `+0x10`, fade in, up and out times `+0x18`.
    pub const TITLES: (u32, usize) = (0x1f0, 0x24);
    /// Device groups: name `+0` (32 bytes), initial value `+0x20`, flags
    /// `+0x24`.
    pub const DEVICE_GROUPS: (u32, usize) = (0xa0, 0x28);
    /// A device group atom holds the runtime datum: salt `0xe564 + index`
    /// over the index (V: every device group atom on 01b, 03a and 05a).
    pub const DEVICE_GROUP_SALT: u32 = 0xe564;
}

/// Every Halo 2 scenario field this crate writes besides the script tables:
/// the level data here and the seat-mapping table ([`crate::seats`]).
pub const H2_FIELDS: [u32; 5] = [
    h2::FLAGS.0,
    h2::CAMERA_POINTS.0,
    h2::TITLES.0,
    h2::DEVICE_GROUPS.0,
    crate::seats::MAPPINGS.0,
];

/// One cutscene flag.
#[derive(Debug, Clone, PartialEq)]
pub struct Flag {
    /// Name.
    pub name: String,
    /// Position.
    pub position: [f32; 3],
    /// Yaw and pitch.
    pub facing: [f32; 2],
}

/// One camera point.
#[derive(Debug, Clone, PartialEq)]
pub struct CameraPoint {
    /// Name.
    pub name: String,
    /// Position.
    pub position: [f32; 3],
    /// Yaw, pitch and roll.
    pub orientation: [f32; 3],
}

/// One device group.
#[derive(Debug, Clone, PartialEq)]
pub struct DeviceGroup {
    /// Name.
    pub name: String,
    /// Initial value.
    pub initial: f32,
    /// Flags, as stored.
    pub flags: u32,
}

/// What a level's scripts name besides objects, AI and tags.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Level {
    /// Cutscene flags.
    pub flags: Vec<Flag>,
    /// Camera points.
    pub camera_points: Vec<CameraPoint>,
    /// Cutscene title names.
    pub titles: Vec<String>,
    /// Device groups.
    pub device_groups: Vec<DeviceGroup>,
}

fn name(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

fn f32s<const N: usize>(b: &[u8], at: usize) -> [f32; N] {
    std::array::from_fn(|i| f32::from_bits(u32_at(b, at + 4 * i).unwrap_or(0)))
}

impl Level {
    /// The level data of `map`'s scenario.
    pub fn read(map: &H1Map) -> Result<Self> {
        let scnr = map.tag_data(map.scenario())?;
        Self::decode(scnr, &map.tag_space())
    }

    /// The level data of the scenario `scnr`, whose blocks point into
    /// `space`.
    pub fn decode(scnr: &[u8], space: &h1map::Space) -> Result<Self> {
        let elements = |(at, len): (usize, usize)| -> Result<Vec<&[u8]>> {
            let b = H1Block::read(scnr, at).ok_or_else(|| invalid("scenario too short"))?;
            Ok(space.block(b, len)?.chunks_exact(len).collect())
        };
        Ok(Self {
            flags: elements(h1::FLAGS)?
                .into_iter()
                .map(|e| Flag {
                    name: name(&e[4..0x24]),
                    position: f32s(e, 0x24),
                    facing: f32s(e, 0x30),
                })
                .collect(),
            camera_points: elements(h1::CAMERA_POINTS)?
                .into_iter()
                .map(|e| CameraPoint {
                    name: name(&e[4..0x24]),
                    position: f32s(e, 0x28),
                    orientation: f32s(e, 0x34),
                })
                .collect(),
            titles: elements(h1::TITLES)?
                .into_iter()
                .map(|e| name(&e[4..0x24]))
                .collect(),
            device_groups: elements(h1::DEVICE_GROUPS)?
                .into_iter()
                .map(|e| DeviceGroup {
                    name: name(&e[..0x20]),
                    initial: f32s::<1>(e, 0x20)[0],
                    flags: u32_at(e, 0x24).unwrap_or(0),
                })
                .collect(),
        })
    }

    /// Halo 2's blocks: (field, element size, elements), for each field
    /// this module writes. An empty list empties the field.
    pub fn blocks(&self) -> Vec<(u32, usize, Vec<Block>)> {
        let put = |b: &mut Block, at: usize, v: &[f32]| {
            for (i, x) in v.iter().enumerate() {
                b.put_f32(at + 4 * i, *x);
            }
        };
        let put_name = |b: &mut Block, at: usize, s: &str| {
            let n = s.len().min(31);
            b.bytes[at..at + n].copy_from_slice(&s.as_bytes()[..n]);
        };
        let flags = self
            .flags
            .iter()
            .map(|f| {
                let mut b = Block::zeroed(h2::FLAGS.1);
                put_name(&mut b, 4, &f.name);
                put(&mut b, 0x24, &f.position);
                put(&mut b, 0x30, &f.facing);
                b
            })
            .collect();
        let cameras = self
            .camera_points
            .iter()
            .map(|c| {
                let mut b = Block::zeroed(h2::CAMERA_POINTS.1);
                put_name(&mut b, 4, &c.name);
                put(&mut b, 0x28, &c.position);
                put(&mut b, 0x34, &c.orientation);
                b
            })
            .collect();
        let titles = self
            .titles
            .iter()
            .map(|t| {
                // The name is the key into the level's text; Halo 1's text
                // is not converted yet. Bounds, colour and timing are this
                // crate's own defaults: a band near the bottom, white, one
                // second in, four up, one out.
                let mut b = Block::zeroed(h2::TITLES.1);
                b.string_id(0, t);
                for (i, v) in [360i16, 0, 400, 640].iter().enumerate() {
                    b.put_u16(4 + 2 * i, *v as u16);
                }
                b.put_u32(0x10, 0xffff_ffff);
                put(&mut b, 0x18, &[1.0, 4.0, 1.0]);
                b
            })
            .collect();
        let groups = self
            .device_groups
            .iter()
            .map(|g| {
                let mut b = Block::zeroed(h2::DEVICE_GROUPS.1);
                put_name(&mut b, 0, &g.name);
                b.put_f32(0x20, g.initial);
                b.put_u32(0x24, g.flags);
                b
            })
            .collect();
        vec![
            (h2::FLAGS.0, h2::FLAGS.1, flags),
            (h2::CAMERA_POINTS.0, h2::CAMERA_POINTS.1, cameras),
            (h2::TITLES.0, h2::TITLES.1, titles),
            (h2::DEVICE_GROUPS.0, h2::DEVICE_GROUPS.1, groups),
        ]
    }

    /// The value a hook of one of this module's types takes: the element's
    /// index (a device group's datum). Navpoints take Halo 2's `default`
    /// (0) whatever Halo 1's type was: Halo 2's navpoint names are not
    /// mapped yet.
    pub fn resolve(&self, h: &Hook) -> Option<u32> {
        fn index<'a>(mut names: impl Iterator<Item = &'a str>, token: &str) -> Option<u32> {
            names.position(|n| n == token).map(|i| i as u32)
        }
        let t = h.token.as_str();
        match h.type_name.as_str() {
            "cutscene_flag" => index(self.flags.iter().map(|f| f.name.as_str()), t),
            "cutscene_camera_point" => index(self.camera_points.iter().map(|c| c.name.as_str()), t),
            "cutscene_title" => index(self.titles.iter().map(String::as_str), t),
            "device_group" => index(self.device_groups.iter().map(|g| g.name.as_str()), t)
                .map(|i| (h2::DEVICE_GROUP_SALT + i) << 16 | i),
            "navpoint" => Some(0),
            _ => None,
        }
    }
}

/// The Halo 2 tag group a tag-valued hook names, for resolving it by the
/// Halo 1 tag's name among the converted tags.
pub fn tag_group(type_name: &str) -> Option<map::FourCC> {
    Some(map::FourCC(*match type_name {
        "sound" => b"snd!",
        "looping_sound" => b"lsnd",
        "effect" => b"effe",
        "damage" | "damage_effect" => b"jpt!",
        "animation_graph" => b"jmad",
        _ => return None,
    }))
}
