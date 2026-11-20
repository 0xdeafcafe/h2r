//! The Halo 1 `sky ` tag.
//!
//! Every offset below is verified against all 31 skies on the owner's
//! retail disc (24 maps): the sizes add up exactly as
//! `0xd0 + functions × 0x24 + lights × 0x74`, and the reference and block
//! fields resolve as described. Evidence and value ranges:
//! `docs/halo1/sky.md`.
//!
//! Names are ours. The disc fixes the layout; the meanings of the ambient
//! scalars are inferred from their values (colour-sized triples, fractions
//! in 0..1, distances), and which ambient is used where has not been
//! established against the engine.

use h1map::{Block, Space, TagRef};
use std::array;

use crate::sky::{Error, Result};

/// Bytes in the root struct.
pub const ROOT_LEN: usize = 0xd0;
/// Bytes per sky light.
pub const LIGHT_LEN: usize = 0x74;
/// Bytes per shader-function element.
pub const FUNCTION_LEN: usize = 0x24;
/// The run of lighting scalars (`f32 × 20`).
pub const LIGHTING_SCALARS: usize = 0x48;
/// Index of the first ambient colour's red inside the scalars array.
pub const AMBIENT_FIRST_RGB: usize = 0;
/// Index of the fraction that follows it.
pub const AMBIENT_FIRST_FRACTION: usize = 3;
/// Index of the second colour's red.
pub const AMBIENT_SECOND_RGB: usize = 4;
/// Indices of the two fog runs' first word (colour, two zero words,
/// density, start, opaque distance: indices 4 to 11 and 12 to 19). V: the
/// density is a fraction, the start the smaller and the opaque distance
/// the larger distance on every sky, the shape of Halo 2's sky fog
/// (`crates/formats/tags/src/defs/sky_.txt`, `fog_a`).
pub const FOG_RUNS: [usize; 2] = [4, 12];

/// One fog run of the lighting scalars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    /// The colour.
    pub color: [f32; 3],
    /// The density, 0 to 1.
    pub max_density: f32,
    /// Where it starts.
    pub start_distance: f32,
    /// Where it is opaque.
    pub opaque_distance: f32,
}

/// Root field offsets.
pub mod layout {
    /// Render model reference (`mode`).
    pub const MODEL: usize = 0x00;
    /// Animation graph reference (`antr`).
    pub const ANIMATION: usize = 0x10;
    /// Fog reference (`fog `); null on every retail sky.
    pub const FOG: usize = 0x98;
    /// Shader functions block.
    pub const FUNCTIONS: usize = 0xb8;
    /// Sky lights block.
    pub const LIGHTS: usize = 0xc4;
}

/// Sky light field offsets, within a `0x74`-byte record.
pub mod light {
    /// Lens flare reference (`lens`); often null.
    pub const LENS_FLARE: usize = 0x00;
    /// The u32 at `+0x10`: `0x006e7500` on every light whose flare is set,
    /// `0` otherwise; not interpreted.
    pub const CONSTANT: usize = 0x10;
    /// Flags (`u32`): 1 or 3 on retail skies.
    pub const FLAGS: usize = 0x4c;
    /// Colour (RGB).
    pub const COLOR: usize = 0x50;
    /// Power.
    pub const POWER: usize = 0x5c;
    /// Falloff distance (50.0 on retail skies).
    pub const FALLOFF: usize = 0x60;
    /// Direction yaw, radians.
    pub const YAW: usize = 0x68;
    /// Direction pitch, radians.
    pub const PITCH: usize = 0x6c;
    /// Cutoff angle, radians (0.0035..0.35 seen: 0.2°..20°).
    pub const CUTOFF: usize = 0x70;
}

/// One sky light: a directional light the sky places in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyLight {
    /// Lens flare (`lens`), or null.
    pub lens_flare: TagRef,
    /// The unexplained constant at `+0x10`.
    pub constant: u32,
    /// Flags at `+0x4c` (1 or 3 retail).
    pub flags: u32,
    /// Colour.
    pub color: [f32; 3],
    /// Power.
    pub power: f32,
    /// Falloff distance.
    pub falloff_distance: f32,
    /// Direction as (yaw, pitch), radians.
    pub direction: [f32; 2],
    /// Cutoff angle, radians.
    pub cutoff_angle: f32,
    /// The record as stored.
    pub raw: Vec<u8>,
}

impl SkyLight {
    fn read(e: &[u8]) -> Result<Self> {
        if e.len() != LIGHT_LEN {
            return Err(Error::Invalid(format!(
                "sky light is {} bytes, want {LIGHT_LEN:#x}",
                e.len()
            )));
        }
        for (name, at) in [("colour", light::COLOR), ("power", light::POWER)] {
            let v = f32(e, at);
            if !v.is_finite() {
                return Err(Error::Invalid(format!("sky light {name} is not finite")));
            }
        }
        if !direction(e).iter().all(|v| v.is_finite()) {
            return Err(Error::Invalid("sky light direction is not finite".into()));
        }
        Ok(Self {
            lens_flare: TagRef::read(e, light::LENS_FLARE)
                .ok_or_else(|| Error::Invalid("truncated sky light reference".into()))?,
            constant: u32(e, light::CONSTANT),
            flags: u32(e, light::FLAGS),
            color: rgb(e, light::COLOR),
            power: f32(e, light::POWER),
            falloff_distance: f32(e, light::FALLOFF),
            direction: direction(e),
            cutoff_angle: f32(e, light::CUTOFF),
            raw: e.to_vec(),
        })
    }
}

/// A Halo 1 `sky ` tag, read out of a map.
#[derive(Clone, Debug, PartialEq)]
pub struct Sky {
    /// The render model (`mode`); null on six retail skies.
    pub model: TagRef,
    /// The animation graph (`antr`); non-null only on the two
    /// `sky\sky_x10\space_x10` skies.
    pub animation: TagRef,
    /// The 20 lighting scalars at `+0x48` (see [LIGHTING_SCALARS]; the doc
    /// maps out the triples, fractions and distances inside the run).
    pub lighting_scalars: [f32; 20],
    /// The fog (`fog `) reference; null on every retail sky.
    pub fog: TagRef,
    /// Shader function elements (`0x24` bytes each), stored as-is: their
    /// layout is not interpreted. Two retail skies carry one each.
    pub shader_functions: Vec<Vec<u8>>,
    /// The sky lights.
    pub lights: Vec<SkyLight>,
}

impl Sky {
    /// Reads the sky tag whose root struct is `bytes` in `space`. Use
    /// [`Sky::from_map`] for a whole map; this form serves invented bytes in
    /// tests.
    pub fn read(space: &Space, bytes: &[u8]) -> Result<Self> {
        if bytes.len() != ROOT_LEN {
            return Err(Error::Invalid(format!(
                "sky root is {} bytes, want {ROOT_LEN:#x}",
                bytes.len()
            )));
        }
        if lighting_scalars(bytes).iter().any(|v| !v.is_finite()) {
            return Err(Error::Invalid("sky lighting scalar is not finite".into()));
        }
        let functions = Block::read(bytes, layout::FUNCTIONS)
            .ok_or_else(|| Error::Invalid("truncated shader functions block".into()))?;
        let lights = Block::read(bytes, layout::LIGHTS)
            .ok_or_else(|| Error::Invalid("truncated lights block".into()))?;
        let functions: Vec<Vec<u8>> = space
            .block(functions, FUNCTION_LEN)
            .map_err(Error::H1)?
            .as_chunks::<FUNCTION_LEN>()
            .0
            .iter()
            .map(|e| e.as_slice().to_vec())
            .collect();
        let lights: Vec<SkyLight> = space
            .block(lights, LIGHT_LEN)
            .map_err(Error::H1)?
            .as_chunks::<LIGHT_LEN>()
            .0
            .iter()
            .map(|e| SkyLight::read(e.as_slice()))
            .collect::<Result<_>>()?;
        Ok(Self {
            model: TagRef::read(bytes, layout::MODEL)
                .ok_or_else(|| Error::Invalid("truncated model reference".into()))?,
            animation: TagRef::read(bytes, layout::ANIMATION)
                .ok_or_else(|| Error::Invalid("truncated animation reference".into()))?,
            lighting_scalars: lighting_scalars(bytes),
            fog: TagRef::read(bytes, layout::FOG)
                .ok_or_else(|| Error::Invalid("truncated fog reference".into()))?,
            shader_functions: functions,
            lights,
        })
    }

    /// The ambient colour at [AMBIENT_FIRST_RGB]: set on every retail sky.
    pub fn ambient_first_rgb(&self) -> [f32; 3] {
        array::from_fn(|i| self.lighting_scalars[AMBIENT_FIRST_RGB + i])
    }

    /// The fraction at [AMBIENT_FIRST_FRACTION].
    pub fn ambient_first_fraction(&self) -> f32 {
        self.lighting_scalars[AMBIENT_FIRST_FRACTION]
    }

    /// The colour at [AMBIENT_SECOND_RGB]: zero on some skies.
    pub fn ambient_second_rgb(&self) -> [f32; 3] {
        array::from_fn(|i| self.lighting_scalars[AMBIENT_SECOND_RGB + i])
    }

    /// The fog runs at [FOG_RUNS], in order; `None` for a run whose
    /// density is 0.
    pub fn fogs(&self) -> [Option<Fog>; 2] {
        FOG_RUNS.map(|k| {
            let s = &self.lighting_scalars;
            let f = Fog {
                color: [s[k], s[k + 1], s[k + 2]],
                max_density: s[k + 5],
                start_distance: s[k + 6],
                opaque_distance: s[k + 7],
            };
            (f.max_density > 0.0).then_some(f)
        })
    }

    /// The outdoor fog: fog A ([FOG_RUNS] `[0]`) when it is one, a density
    /// above 0 reaching opacity past its start. Halo 1 draws it in the
    /// clusters that see this sky; fog A's opaque distance of 0 (a10's and
    /// d40's space skies, c20's interior atmosphere, the menu's `sky_ui`:
    /// colour black, density 1, start and opaque 0) is no fog. V: the menu
    /// level's every cluster names `sky_ui`, and x10's space BSP's names
    /// `space_x10`, and neither draws black
    /// (docs/halo1/sky.md, "Fog A is the outdoor fog").
    pub fn outdoor_fog(&self) -> Option<Fog> {
        self.fogs()[0].filter(|f| f.opaque_distance > f.start_distance)
    }

    /// The indoor fog: fog B ([FOG_RUNS] `[1]`) when it has a density.
    /// Halo 1 draws the scenario's first sky's in every cluster naming no
    /// sky, and never outdoors (E `0x000581a0`: sky `-1` takes skies[0]'s
    /// run at `+0x78`, a sky index that sky's run at `+0x58`). Every fog B
    /// on the disc with a density reaches opacity past its start (V). Its
    /// Halo 2 home is the scenario's atmosphere block ([`crate::sky::atmosphere`]).
    pub fn indoor_fog(&self) -> Option<Fog> {
        self.fogs()[1]
    }

    /// Reads the `sky ` tag `tag` of `map`.
    pub fn from_map(map: &h1map::H1Map, tag: &h1map::Tag) -> Result<Self> {
        if tag.group != h1map::Group::new(b"sky ") {
            return Err(Error::WrongGroup(tag.group.to_string()));
        }
        let bytes = map.tag_data(tag)?;
        // The tag's data range covers the root plus its block elements; the
        // reader works on the root and follows the blocks through the space.
        let root = bytes
            .get(..ROOT_LEN)
            .ok_or_else(|| Error::Invalid("sky tag shorter than its root".into()))?;
        Self::read(&map.tag_space(), root)
    }
}

fn u32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().unwrap())
}

fn f32(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32(b, at))
}

fn rgb(b: &[u8], at: usize) -> [f32; 3] {
    array::from_fn(|i| f32(b, at + 4 * i))
}

fn direction(b: &[u8]) -> [f32; 2] {
    [f32(b, light::YAW), f32(b, light::PITCH)]
}

fn lighting_scalars(b: &[u8]) -> [f32; 20] {
    array::from_fn(|i| f32(b, LIGHTING_SCALARS + 4 * i))
}
