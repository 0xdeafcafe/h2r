//! Halo 1 sky to Halo 2 `sky `.
//!
//! The output is a [`h1models::Converted`] block tree (the same shape
//! `map::build` consumes for converted models): a `0xac`-byte root with the
//! sky's lights as child blocks. What maps where, and what has no Halo 2
//! home, is listed in `docs/formats/h1-sky.md`.
//!
//! Synthesised defaults (not read from the source sky, which has no such
//! field): the light flags `15`, the retail-common value; the root word at
//! `+0x10` `0`; model scale `1.0` (Halo 1 sky models are authored full
//! size); camera translation scale `1.0` (the Halo 1 sky box stays centred
//! on the camera). Lens flares stay null: converting `lens` tags is not
//! this crate's job.

use map::build::{Block, FixupKind, TagSet};
use map::{CacheType, FourCC, TagId};

use crate::h1;
use crate::Result;

/// Flags written into each converted light's parameter record: the value on
/// 49 of 91 retail skies. Halo 1's light flags (1 or 3) have a different,
/// unknown meaning and are not copied.
pub const LIGHT_FLAGS: u32 = 15;

/// Builds the Halo 2 `sky ` tree for `sky`. `model` is the datum of the
/// converted render model ([`h1models::model_from_map`] output, installed
/// into the same set), or `None` for skies with no model.
pub fn convert_sky(sky: &h1::Sky, model: Option<TagId>) -> h1models::Converted {
    let mut root = Block::zeroed(crate::h2::ROOT_LEN);
    root.fixup(
        crate::h2::layout::MODEL as u32,
        FixupKind::TagRef(model.unwrap_or(TagId::NULL)),
    );
    root.fixup(
        crate::h2::layout::UNUSED_REF as u32,
        FixupKind::TagRef(TagId::NULL),
    );
    root.put_u32(crate::h2::layout::WORD_10, 0);
    root.put_f32(crate::h2::layout::MODEL_SCALE, 1.0);
    root.put_f32(crate::h2::layout::CAMERA_TRANSLATION_SCALE, 1.0);
    let empty = || Block::zeroed(0);
    root.fixup(
        crate::h2::layout::BITMAP_LAYERS as u32,
        FixupKind::Block {
            count: 0,
            child: empty(),
        },
    );
    for (i, v) in sky.ambient_first_rgb().into_iter().enumerate() {
        root.put_f32(crate::h2::layout::FALLBACK_RGB + 4 * i, v);
    }
    root.fixup(
        crate::h2::layout::GRADIENT_A as u32,
        FixupKind::Block {
            count: 0,
            child: empty(),
        },
    );
    root.fixup(
        crate::h2::layout::GRADIENT_B as u32,
        FixupKind::Block {
            count: 0,
            child: empty(),
        },
    );
    let lights: Vec<Block> = sky.lights.iter().map(light_block).collect();
    let count = lights.len() as u32;
    root.fixup(
        crate::h2::layout::LIGHTS as u32,
        FixupKind::Block {
            count,
            child: array(lights, crate::h2::LIGHT_LEN),
        },
    );
    root.put_f32(crate::h2::layout::ROTATION, 0.0);
    h1models::Converted {
        group: FourCC(*b"sky "),
        root,
        raw: Vec::new(),
    }
}

/// One Halo 2 light record (`0x34` bytes) with its parameter child.
fn light_block(light: &h1::SkyLight) -> Block {
    let mut b = Block::zeroed(crate::h2::LIGHT_LEN);
    b.put_f32(crate::h2::light::YAW, light.direction[0]);
    b.put_f32(crate::h2::light::PITCH, light.direction[1]);
    b.fixup(
        crate::h2::light::LENS_FLARE as u32,
        FixupKind::TagRef(TagId::NULL),
    );
    let mut p = Block::zeroed(crate::h2::PARAMS_LEN);
    p.put_u32(crate::h2::params::FLAGS, LIGHT_FLAGS);
    for i in 0..3 {
        p.put_f32(crate::h2::params::COLOR + 4 * i, light.color[i]);
    }
    p.put_f32(crate::h2::params::POWER, light.power);
    p.put_f32(crate::h2::params::FALLOFF, light.falloff_distance);
    p.put_f32(crate::h2::params::CUTOFF, light.cutoff_angle);
    b.fixup(
        crate::h2::light::PARAMS as u32,
        FixupKind::Block {
            count: 1,
            child: p,
        },
    );
    b
}

/// Concatenates fixed-size element blocks, as `h1models` does for its
/// arrays.
fn array(elements: Vec<Block>, stride: usize) -> Block {
    let mut out = Block::zeroed(elements.len() * stride);
    for (i, e) in elements.into_iter().enumerate() {
        out.bytes[i * stride..(i + 1) * stride].copy_from_slice(&e.bytes);
        for mut f in e.fixups {
            f.offset += (i * stride) as u32;
            out.fixups.push(f);
        }
    }
    out
}

/// Reads the `sky ` tag `tag` of `map`, and converts its render model when
/// it has one. Pair the model with [`install_sky`], which installs both
/// into one set and links the reference.
pub fn sky_from_map(
    map: &h1map::H1Map,
    tag: &h1map::Tag,
) -> Result<(h1::Sky, Option<(String, h1models::Converted)>)> {
    let sky = h1::Sky::from_map(map, tag)?;
    let model = if sky.model.id.is_none() {
        None
    } else {
        let tag = map.tag(sky.model.id)?;
        if tag.group != h1map::Group::new(b"mode") {
            return Err(crate::Error::Invalid(format!(
                "sky model reference names a {} tag",
                tag.group
            )));
        }
        let (_, converted) = h1models::model_from_map(map, tag, &h1models::mode::ModelOptions::default())?;
        Some((tag.name.clone(), converted))
    };
    Ok((sky, model))
}

/// Installs `model` (if any) and the converted `sky` into `set`, linking
/// the sky's model reference, and returns the sky's datum. `name` is the
/// Halo 1 sky's tag path.
pub fn install_sky(
    set: &mut TagSet,
    sky: &h1::Sky,
    model: Option<(&str, h1models::Converted)>,
    name: &str,
) -> Result<TagId> {
    let model_id = match model {
        Some((model_name, converted)) => Some(converted.install(set, model_name)?),
        None => None,
    };
    Ok(convert_sky(sky, model_id).install(set, name)?)
}

/// Builds a set holding one converted sky and returns its bytes and the
/// datums (the sky's, and the model's when present). Tests and the disc
/// checks use this; real imports add the sky to the level's own set.
pub fn build_sky_map(
    sky: &h1::Sky,
    model: Option<(&str, h1models::Converted)>,
    name: &str,
) -> Result<(Vec<u8>, TagId, Option<TagId>)> {
    let mut set = TagSet::new(CacheType::Multiplayer, "h1skytest", "invented\\h1skytest");
    let id = install_sky(&mut set, sky, model, name)?;
    let model_id = set
        .slots
        .iter()
        .enumerate()
        .filter(|(i, s)| {
            s.tag.as_ref().is_some_and(|t| *i != id.index() as usize && t.group == FourCC(*b"mode"))
        })
        .find_map(|(i, _)| set.slots[i].tag.as_ref().map(|t| t.id));
    let (bytes, _) = set.build().map_err(crate::Error::Map)?;
    Ok((bytes, id, model_id))
}
