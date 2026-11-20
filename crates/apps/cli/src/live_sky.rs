//! A level's sky for the 3D viewer: its render model as `map/<stem>/sky.glb`
//! (drawn around the camera, behind everything) and its colours and fog in
//! `map.glb`'s `asset.extras.sky` (docs/live/live-art.md, "Sky").
//!
//! The scenario's skies block is `scnr+0x8` (8-byte tag references, ID at
//! +4). Of a `sky ` tag only these fields are read, the layout public sky
//! definitions give and the disc agrees with (`geometry_from_the_disc`
//! reports every level's sky): the render model reference at +0 (ID at +4),
//! the model scale at +0x14 (docs/formats/lighting.md), the atmospheric fog block
//! at +0x48 (0x18-byte elements: colour RGB, maximum density, start
//! distance, opaque distance), the secondary fog block at +0x50 (the same
//! layout) and the clear colour (RGB) at +0xa0. The lights block at +0x78
//! (0x34 bytes each: the light's direction as heading and pitch in
//! radians at +0xc, a fog block at +0x2c whose element holds flags, then
//! colour RGB at +4 and maximum density at +0x10) gives the glow of the
//! sun-lit air, which stands in for the fog on levels without one
//! (Zanzibar).

use anyhow::{Context, Result};
use assets::ResourceFiles;
use map::{FourCC, TagId};
use serde::{Deserialize, Serialize};
use tags::geometry::TagView;

use crate::live_geometry::{MapReader, Raw, shader_template};
use crate::live_models::{self, Model};

const SKY: FourCC = FourCC(*b"sky ");
const MODE: FourCC = FourCC(*b"mode");

/// `scnr+0x8`: the skies block, a tag reference each.
pub const SCENARIO_SKIES: usize = 0x8;
/// Sky tag fields (module docs).
pub const SKY_MODEL: usize = 0x0;
pub const SKY_SCALE: usize = 0x14;
pub const SKY_ATMOSPHERIC_FOG: usize = 0x48;
pub const SKY_SECONDARY_FOG: usize = 0x50;
pub const FOG_STRIDE: usize = 0x18;
pub const SKY_CLEAR: usize = 0xa0;
pub const SKY_LIGHTS: usize = 0x78;
pub const LIGHT_STRIDE: usize = 0x34;
pub const LIGHT_DIRECTION: usize = 0xc;
pub const LIGHT_FOG: usize = 0x2c;
pub const LIGHT_FOG_STRIDE: usize = 0x14;
pub const SKY_HEADER: usize = 0xac;

/// Most triangles a sky model is drawn with.
pub const SKY_TRIANGLES: usize = 30_000;

/// The level's fog as the sky gives it (game units, colours 0..1 as the
/// tag stores them).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkyFog {
    pub colour: [f32; 3],
    /// How thick the fog gets (0..1) at `end` and beyond.
    pub density: f32,
    /// Distance from the eye where the fog starts, and where it reaches
    /// `density`.
    pub start: f32,
    pub end: f32,
}

/// The sun-lit air of a sky light: its colour and how thick it is (0..1),
/// and the light's heading and pitch (radians, game frame: heading about
/// +z from +x, pitch up from the horizon).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkyGlow {
    pub colour: [f32; 3],
    pub density: f32,
    pub heading: f32,
    pub pitch: f32,
}

/// `asset.extras.sky` of `map.glb`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SkyInfo {
    /// The sky tag's file name.
    pub name: String,
    /// The sky model, relative to `map.glb` (`sky.glb`); absent when the
    /// sky has no model or it could not be built.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glb: Option<String>,
    /// The colour behind everything, when the sky sets one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clear: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fog: Option<SkyFog>,
    /// The first sky light's glow, when it has one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub glow: Option<SkyGlow>,
    /// The sky model's scale in the game (it is drawn around the camera at
    /// any size).
    pub scale: f32,
}

/// What a sky tag says, before its model is built.
#[derive(Clone, Debug, PartialEq)]
pub struct SkyFields {
    pub model: Option<TagId>,
    pub scale: f32,
    pub clear: Option<[f32; 3]>,
    pub fog: Option<SkyFog>,
    pub glow: Option<SkyGlow>,
}

fn f32_at(b: &[u8], o: usize) -> Option<f32> {
    Some(f32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn rgb(b: &[u8], o: usize) -> Option<[f32; 3]> {
    Some([f32_at(b, o)?, f32_at(b, o + 4)?, f32_at(b, o + 8)?])
}

/// A colour that is set: finite, in 0..=1 (allowing a little over), not
/// black.
fn colour(c: Option<[f32; 3]>) -> Option<[f32; 3]> {
    c.filter(|c| {
        c.iter().all(|v| v.is_finite() && (0.0..=4.0).contains(v)) && c.iter().any(|&v| v > 0.0)
    })
}

/// A fog element (colour, maximum density, start, opaque distance), when
/// it makes sense: a colour, a density over 0, and an end past the start.
pub fn fog_element(e: &[u8]) -> Option<SkyFog> {
    let c = colour(rgb(e, 0))?;
    let density = f32_at(e, 0xc)?;
    let start = f32_at(e, 0x10)?;
    let end = f32_at(e, 0x14)?;
    let ok = density.is_finite() && density > 0.0 && start.is_finite() && end.is_finite();
    (ok && start >= 0.0 && end > start).then_some(SkyFog {
        colour: c,
        density: density.min(1.0),
        start,
        end,
    })
}

/// The fields of a sky tag's payload (`view` over it, `header` its first
/// [`SKY_HEADER`] bytes). The fog is the atmospheric fog, else the
/// secondary.
pub fn sky_fields(view: &TagView<'_>, header: &[u8]) -> SkyFields {
    let model = header
        .get(SKY_MODEL + 4..SKY_MODEL + 8)
        .map(|r| TagId(u32::from_le_bytes([r[0], r[1], r[2], r[3]])))
        .filter(|&id| id != TagId::NULL);
    let scale = f32_at(header, SKY_SCALE)
        .filter(|s| s.is_finite() && *s > 0.0)
        .unwrap_or(1.0);
    let fog = [SKY_ATMOSPHERIC_FOG, SKY_SECONDARY_FOG]
        .into_iter()
        .find_map(|at| {
            view.block(header, at, FOG_STRIDE)
                .ok()?
                .into_iter()
                .find_map(fog_element)
        });
    let glow = view
        .block(header, SKY_LIGHTS, LIGHT_STRIDE)
        .ok()
        .into_iter()
        .flatten()
        .find_map(|light| {
            let e = *view
                .block(light, LIGHT_FOG, LIGHT_FOG_STRIDE)
                .ok()?
                .first()?;
            let density = f32_at(e, 0x10).filter(|d| d.is_finite() && *d > 0.0)?;
            let heading = f32_at(light, LIGHT_DIRECTION).filter(|v| v.is_finite())?;
            let pitch = f32_at(light, LIGHT_DIRECTION + 4).filter(|v| v.is_finite())?;
            Some(SkyGlow {
                colour: colour(rgb(e, 4))?,
                density: density.min(1.0),
                heading,
                pitch,
            })
        });
    SkyFields {
        model,
        scale,
        clear: colour(rgb(header, SKY_CLEAR)),
        fog,
        glow,
    }
}

/// The scenario's skies, in order.
pub fn scenario_skies(map: &mut MapReader) -> Result<Vec<TagId>> {
    let scenario = map.scenario().context("no scenario")?.clone();
    let location = scenario.location.context("scenario has no payload")?;
    let bytes = map.read_tag_bytes(scenario.id)?;
    let view = TagView::new(&bytes, location.address);
    Ok(view
        .block(view.at(0, 0x10)?, SCENARIO_SKIES, 8)?
        .into_iter()
        .map(|b| TagId(u32::from_le_bytes([b[4], b[5], b[6], b[7]])))
        .collect())
}

/// A sky tag's name and fields.
pub fn read_sky_tag(map: &mut MapReader, id: TagId) -> Result<(String, SkyFields)> {
    let tag = map.tag(id)?.clone();
    if tag.group != SKY {
        anyhow::bail!("not a sky tag");
    }
    let location = tag.location.context("sky has no payload")?;
    let bytes = map.read_tag_bytes(id)?;
    let view = TagView::new(&bytes, location.address);
    let header = view.at(0, SKY_HEADER)?;
    let name = tag.name.unwrap_or_default();
    let stem = name.rsplit(['\\', '/']).next().unwrap_or(&name).to_string();
    Ok((stem, sky_fields(&view, header)))
}

/// How a sky material is drawn, from words in its shader template's last
/// path component: `additive` (`add`), `alpha` (`alpha`, clouds) or
/// `opaque`.
pub fn sky_blend(template: &str) -> &'static str {
    let own = template
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or(template)
        .to_ascii_lowercase();
    let words: Vec<&str> = own
        .split(|c: char| !c.is_ascii_alphabetic())
        .filter(|w| !w.is_empty())
        .collect();
    if words.contains(&"add") {
        "additive"
    } else if words
        .iter()
        .any(|w| ["alpha", "cloud", "clouds"].contains(w))
    {
        "alpha"
    } else {
        "opaque"
    }
}

/// The sky model as a glb: every region's base permutation at the highest
/// detail under [`SKY_TRIANGLES`], textured (its see-through layers keep
/// their pictures, [`live_models::Surfaces::Sky`]), at rest; `asset.extras.sky`
/// names the sky and `asset.extras.blend` says per material (in order) how
/// it is drawn ([`sky_blend`]).
pub fn sky_glb(
    map: &mut MapReader,
    files: &mut ResourceFiles,
    name: &str,
    mode: TagId,
) -> Result<Vec<u8>> {
    let src =
        live_models::read_source_for(map, files, mode, SKY_TRIANGLES, live_models::Surfaces::Sky)?;
    let rest = live_models::rest_world(&src.bones);
    let mut model = Model {
        mesh_name: "sky".into(),
        primitives: live_models::pose_mesh(&src, &rest),
        materials: src.materials.clone(),
        ..Model::default()
    };
    if model.primitives.is_empty() {
        anyhow::bail!("no drawn triangles");
    }
    for p in &mut model.primitives {
        p.colours.clear();
    }
    let geo = tags::model::RenderModelGeometry::read(map, mode, &mut Raw(files))?;
    let blend: Vec<&str> = geo
        .materials
        .iter()
        .map(|m| shader_template(map, m.shader.id).map_or("opaque", |t| sky_blend(&t)))
        .collect();
    model.extras.insert("sky".into(), serde_json::json!(name));
    model
        .extras
        .insert("blend".into(), serde_json::json!(blend));
    Ok(live_models::write_glb(&model))
}

/// The level's sky: the first of the scenario's skies with a model (else
/// the first), its fields, and its model's glb when it could be built
/// (`Err` says why not).
pub fn read_level_sky(
    map: &mut MapReader,
    files: &mut ResourceFiles,
) -> Result<Option<(SkyInfo, Result<Vec<u8>>)>> {
    let mut found: Vec<(String, SkyFields)> = Vec::new();
    for id in scenario_skies(map)? {
        if id == TagId::NULL {
            continue;
        }
        if let Ok(s) = read_sky_tag(map, id) {
            found.push(s);
        }
    }
    let Some(i) = found
        .iter()
        .position(|(_, f)| f.model.is_some())
        .or((!found.is_empty()).then_some(0))
    else {
        return Ok(None);
    };
    let (name, f) = found.swap_remove(i);
    let glb = match f.model {
        Some(mode) if map.tag(mode).is_ok_and(|t| t.group == MODE) => {
            sky_glb(map, files, &name, mode)
        }
        Some(_) => Err(anyhow::anyhow!("the sky's model is not a render model")),
        None => Err(anyhow::anyhow!("the sky has no model")),
    };
    let info = SkyInfo {
        name,
        glb: glb.is_ok().then(|| "sky.glb".to_string()),
        clear: f.clear,
        fog: f.fog,
        glow: f.glow,
        scale: f.scale,
    };
    Ok(Some((info, glb)))
}
