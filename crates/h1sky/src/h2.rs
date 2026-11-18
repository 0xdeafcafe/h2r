//! The Halo 2 `sky ` tag, decoded back into values.
//!
//! This is the read-back half of the converter, and a reader for retail
//! maps. Root fields already reviewed against the engine (model reference,
//! model scale, camera translation scale, fallback object-lighting colour,
//! the lights block and its yaw/pitch, rotation) carry their engine
//! evidence; the rest is read structurally from the retail disc — 91
//! instances over 28 maps. `docs/formats/h1-sky.md` lists every field with
//! its evidence level. As in [`h1`], names are ours where the engine has
//! not been consulted.

use std::array;
use std::io::{Read, Seek};

use map::{FourCC, Map, TagId};

use crate::{Error, Result};

/// Bytes in the root struct. The declared tag payload additionally covers
/// the block elements, which the builder (and retail) lay out after it.
pub const ROOT_LEN: usize = 0xac;
/// Bytes per sky light record.
pub const LIGHT_LEN: usize = 0x34;
/// Bytes per light-parameter record (the light's child block).
pub const PARAMS_LEN: usize = 0x28;
/// Bytes per bitmap-layer element (root block `+0x1c`).
pub const BITMAP_LAYER_LEN: usize = 0xc;
/// Bytes per gradient element (root blocks `+0x48` and `+0x50`).
pub const GRADIENT_LEN: usize = 0x18;

/// Root field offsets.
pub mod layout {
    /// Render model reference (`mode`), 8 bytes.
    pub const MODEL: usize = 0x00;
    /// A second reference, null on every retail sky; its group is unknown.
    pub const UNUSED_REF: usize = 0x08;
    /// A `u32` that varies with the sky (0, 2, 6, 18, 38 seen); not
    /// interpreted.
    pub const WORD_10: usize = 0x10;
    /// Uniform model scale (engine: `0x0003ebd0`).
    pub const MODEL_SCALE: usize = 0x14;
    /// Camera translation scale (engine: `0x0003ebd0`).
    pub const CAMERA_TRANSLATION_SCALE: usize = 0x18;
    /// Bitmap layers block.
    pub const BITMAP_LAYERS: usize = 0x1c;
    /// Fallback object-lighting colour (engine: `0x000d1a30`).
    pub const FALLBACK_RGB: usize = 0x34;
    /// A scalar set by one retail sky (5.0); not interpreted.
    pub const WORD_44: usize = 0x44;
    /// First gradient block.
    pub const GRADIENT_A: usize = 0x48;
    /// Second gradient block.
    pub const GRADIENT_B: usize = 0x50;
    /// Sky lights block (engine: `0x0003eec0`, `0x000d1a30`).
    pub const LIGHTS: usize = 0x78;
    /// Rotation angle, radians (engine: `0x0003ebd0`).
    pub const ROTATION: usize = 0x80;
}

/// Sky light field offsets, within a `0x34`-byte record.
pub mod light {
    /// Three zero `u32`s on every retail light; not interpreted.
    pub const HEAD: usize = 0x00;
    /// Direction yaw, radians (engine: `0x0003eec0`).
    pub const YAW: usize = 0x0c;
    /// Direction pitch, radians (engine: `0x0003eec0`).
    pub const PITCH: usize = 0x10;
    /// Lens flare reference (`lens`), 8 bytes.
    pub const LENS_FLARE: usize = 0x14;
    /// Four zero `u32`s on every retail light; not interpreted.
    pub const MID: usize = 0x1c;
    /// The light's parameters block, one `0x28`-byte element.
    pub const PARAMS: usize = 0x2c;
}

/// Light-parameter field offsets, within a `0x28`-byte element.
pub mod params {
    /// Flags (`u32`; 15 common, 12, 13 and 0 also seen).
    pub const FLAGS: usize = 0x00;
    /// A zero `u32` on retail.
    pub const WORD_04: usize = 0x04;
    /// Colour (RGB).
    pub const COLOR: usize = 0x08;
    /// Power.
    pub const POWER: usize = 0x14;
    /// Falloff distance (50.0 and 80.0 seen).
    pub const FALLOFF: usize = 0x18;
    /// A zero scalar on retail; not interpreted.
    pub const WORD_1C: usize = 0x1c;
    /// A zero scalar on retail; not interpreted.
    pub const WORD_20: usize = 0x20;
    /// Cutoff angle, radians (0.035..0.35 seen: 2°..20°).
    pub const CUTOFF: usize = 0x24;
}

/// An 8-byte Halo 2 tag reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ref {
    /// The group word, as stored.
    pub group: u32,
    /// The referenced tag.
    pub id: TagId,
}

impl Ref {
    fn read(b: &[u8], at: usize) -> Self {
        Self {
            group: u32(b, at),
            id: TagId(u32(b, at + 4)),
        }
    }

    /// Whether the reference is null (`ffffffff` datum).
    pub fn is_null(&self) -> bool {
        self.id == TagId::NULL
    }

    /// The group as a FourCC.
    pub fn fourcc(&self) -> FourCC {
        FourCC::from_u32(self.group)
    }
}

/// A bitmap-layer element (root block `+0x1c`): a `bitm` reference plus a
/// scalar (1.0, 1.5, 0.0 seen).
#[derive(Clone, Debug, PartialEq)]
pub struct BitmapLayer {
    /// The bitmap; null on some retail skies.
    pub bitmap: Ref,
    /// The scalar.
    pub value: f32,
}

/// A fog element (root blocks `+0x48`, `+0x50`; `fog_a` and `fog_b` in
/// `crates/tags/src/defs/sky_.txt`): a colour, the density and the start
/// and opaque distances (0, 200, 700 seen on retail). E `0x00131030`
/// blends element 0 of each into the view (docs/formats/h1-sky.md).
#[derive(Clone, Debug, PartialEq)]
pub struct Gradient {
    /// The colour.
    pub color: [f32; 3],
    /// The fraction.
    pub fraction: f32,
    /// The two trailing scalars.
    pub tail: [f32; 2],
}

/// A sky light's parameters, the `0x28`-byte element of its child block.
#[derive(Clone, Debug, PartialEq)]
pub struct LightParams {
    /// Flags.
    pub flags: u32,
    /// The `u32` at `+0x04`.
    pub word_04: u32,
    /// Colour.
    pub color: [f32; 3],
    /// Power.
    pub power: f32,
    /// Falloff distance.
    pub falloff_distance: f32,
    /// The scalar at `+0x1c`.
    pub word_1c: f32,
    /// The scalar at `+0x20`.
    pub word_20: f32,
    /// Cutoff angle, radians.
    pub cutoff_angle: f32,
}

/// One sky light.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyLight {
    /// The record's first three words, zero on retail.
    pub head: [u32; 3],
    /// Direction yaw, radians.
    pub yaw: f32,
    /// Direction pitch, radians.
    pub pitch: f32,
    /// Lens flare reference.
    pub lens_flare: Ref,
    /// The record's middle words, zero on retail.
    pub mid: [u32; 4],
    /// The parameters block's single element, when present.
    pub params: Option<LightParams>,
}

/// A decoded Halo 2 `sky ` tag.
#[derive(Clone, Debug, PartialEq)]
pub struct Sky {
    /// The render model (`mode`).
    pub model: Ref,
    /// The null reference at `+0x08`.
    pub unused_ref: Ref,
    /// The `u32` at `+0x10`.
    pub word_10: u32,
    /// Uniform model scale.
    pub model_scale: f32,
    /// Camera translation scale.
    pub camera_translation_scale: f32,
    /// Bitmap layers (usually one, sometimes none).
    pub bitmap_layers: Vec<BitmapLayer>,
    /// Fallback object-lighting colour.
    pub fallback_rgb: [f32; 3],
    /// The scalar at `+0x44`.
    pub word_44: f32,
    /// First gradient block's elements.
    pub gradients_a: Vec<Gradient>,
    /// Second gradient block's elements.
    pub gradients_b: Vec<Gradient>,
    /// Sky lights.
    pub lights: Vec<SkyLight>,
    /// Rotation angle, radians.
    pub rotation: f32,
}

impl Sky {
    /// Decodes the sky whose root struct is `payload` at virtual address
    /// `address`. `read_at` supplies the bytes of any metadata address:
    /// block elements follow the root in built maps, but retail maps can
    /// point them anywhere (three retail skies alias another tag's payload),
    /// so elements are read through it rather than out of `payload`.
    pub fn decode(
        payload: &[u8],
        _address: u32,
        read_at: &mut dyn FnMut(u32, u32) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        if payload.len() < ROOT_LEN {
            return Err(Error::Invalid(format!(
                "sky root is {} bytes, want at least {ROOT_LEN:#x}",
                payload.len()
            )));
        }
        let mut elements = |block_at: usize, elem_len: usize| -> Result<Vec<Vec<u8>>> {
            let count = u32(payload, block_at);
            if count == 0 {
                return Ok(Vec::new());
            }
            if count > 64 {
                return Err(Error::Invalid(format!("{count} sky block elements")));
            }
            let at = u32(payload, block_at + 4);
            let bytes = read_at(at, count * elem_len as u32)?;
            if bytes.len() != count as usize * elem_len {
                return Err(Error::Invalid("short sky block read".into()));
            }
            Ok(bytes.chunks_exact(elem_len).map(|e| e.to_vec()).collect())
        };
        let bitmap_layers = elements(layout::BITMAP_LAYERS, BITMAP_LAYER_LEN)?
            .into_iter()
            .map(|e| BitmapLayer {
                bitmap: Ref::read(&e, 0),
                value: f32(&e, 8),
            })
            .collect();
        let gradient = |e: &[u8]| Gradient {
            color: rgb(e, 0),
            fraction: f32(e, 0xc),
            tail: [f32(e, 0x10), f32(e, 0x14)],
        };
        let gradients_a = elements(layout::GRADIENT_A, GRADIENT_LEN)?
            .into_iter()
            .map(|e| gradient(&e))
            .collect();
        let gradients_b = elements(layout::GRADIENT_B, GRADIENT_LEN)?
            .into_iter()
            .map(|e| gradient(&e))
            .collect();
        let lights = elements(layout::LIGHTS, LIGHT_LEN)?
            .into_iter()
            .map(|e| {
                let params = if u32(&e, light::PARAMS) == 0 {
                    None
                } else {
                    let at = u32(&e, light::PARAMS + 4);
                    let b = read_at(at, PARAMS_LEN as u32)?;
                    if b.len() != PARAMS_LEN {
                        return Err(Error::Invalid("short light params read".into()));
                    }
                    Some(LightParams {
                        flags: u32(&b, params::FLAGS),
                        word_04: u32(&b, params::WORD_04),
                        color: rgb(&b, params::COLOR),
                        power: f32(&b, params::POWER),
                        falloff_distance: f32(&b, params::FALLOFF),
                        word_1c: f32(&b, params::WORD_1C),
                        word_20: f32(&b, params::WORD_20),
                        cutoff_angle: f32(&b, params::CUTOFF),
                    })
                };
                Ok(SkyLight {
                    head: array::from_fn(|i| u32(&e, light::HEAD + 4 * i)),
                    yaw: f32(&e, light::YAW),
                    pitch: f32(&e, light::PITCH),
                    lens_flare: Ref::read(&e, light::LENS_FLARE),
                    mid: array::from_fn(|i| u32(&e, light::MID + 4 * i)),
                    params,
                })
            })
            .collect::<Result<_>>()?;
        for v in [
            f32(payload, layout::MODEL_SCALE),
            f32(payload, layout::CAMERA_TRANSLATION_SCALE),
            f32(payload, layout::ROTATION),
        ] {
            if !v.is_finite() {
                return Err(Error::Invalid("sky scalar is not finite".into()));
            }
        }
        Ok(Self {
            model: Ref::read(payload, layout::MODEL),
            unused_ref: Ref::read(payload, layout::UNUSED_REF),
            word_10: u32(payload, layout::WORD_10),
            model_scale: f32(payload, layout::MODEL_SCALE),
            camera_translation_scale: f32(payload, layout::CAMERA_TRANSLATION_SCALE),
            bitmap_layers,
            fallback_rgb: rgb(payload, layout::FALLBACK_RGB),
            word_44: f32(payload, layout::WORD_44),
            gradients_a,
            gradients_b,
            lights,
            rotation: f32(payload, layout::ROTATION),
        })
    }

    /// Decodes the sky `id` of `map`, reading elements through the map.
    pub fn read_from_map<R: Read + Seek>(map: &mut Map<R>, id: TagId) -> Result<Self> {
        let tag = map.tag(id).map_err(Error::Map)?;
        if tag.group != FourCC(*b"sky ") {
            return Err(Error::WrongGroup(tag.group.to_string()));
        }
        let location = tag.location.ok_or(map::Error::NoTagData).map_err(Error::Map)?;
        // A retail identity can declare zero bytes at another sky's exact
        // address; resolve that alias to the real payload (the same rule the
        // atmosphere reader uses).
        let (source, size) = if location.size == 0 {
            map.tags()
                .iter()
                .filter(|t| t.group == FourCC(*b"sky "))
                .filter_map(|t| {
                    let l = t.location.as_ref()?;
                    (l.address == location.address && l.size != 0).then_some((t.id, l.size))
                })
                .max_by_key(|&(_, s)| s)
                .ok_or_else(|| Error::Invalid("zero-sized sky has no same-class alias".into()))?
        } else {
            (id, location.size)
        };
        let source_tag = map.tag(source).map_err(Error::Map)?;
        let address = source_tag.location.unwrap().address;
        let off = source_tag.location.unwrap().file_offset().map_err(Error::Map)?;
        let len = (ROOT_LEN as u32).min(size) as u64;
        let payload = map.read_file_range(off, len).map_err(Error::Map)?;
        let mut read_at = |at: u32, n: u32| -> Result<Vec<u8>> {
            let o = map.file_offset(at, n).map_err(Error::Map)?;
            map.read_file_range(o, n.into()).map_err(Error::Map)
        };
        Self::decode(&payload, address, &mut read_at)
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
