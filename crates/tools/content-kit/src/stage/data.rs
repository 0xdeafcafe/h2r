//! The stage's manifest, `content/stage/kit.yaml`: the values the stage's
//! builders read that are data (its version, biped, floor material, salt,
//! the sky's shape and colours, the lightmap's packing and the textures'
//! colours), parsed once and refused on any unknown or missing key. What is
//! structure (tag field offsets, glyph bitmaps) stays in the builders.

use std::sync::OnceLock;

use anyhow::{Context, Result, ensure};
use serde::Deserialize;

/// A colour, RGBA bytes.
pub type Rgba = [u8; 4];

/// The shipped manifest.
pub const SOURCE: &str = include_str!("../../../../../content/stage/kit.yaml");

/// The stage's manifest.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StageData {
    /// The item's id (its folder under `content/`).
    pub item: String,
    /// The build version.
    pub version: u32,
    /// The multiplayer player's biped.
    pub biped: String,
    /// The global material every surface is made of.
    pub floor_material: String,
    /// The salt of retail slot 0.
    pub salt: u16,
    pub sky: SkyData,
    pub light: LightData,
    pub texture: TextureData,
}

/// The sky dome and its picture.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkyData {
    pub radius: f32,
    pub segments: u32,
    pub rings: u32,
    pub width: u32,
    pub height: u32,
    pub compass: Vec<Compass>,
    pub letter_elevation: f32,
    pub letter_scale: u32,
    pub disc: f32,
    pub glow: f32,
    pub line_half: f32,
    pub light_cutoff: f32,
    pub colors: SkyColors,
}

/// One compass letter and the azimuth it stands at.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Compass {
    pub letter: char,
    pub azimuth: f32,
}

/// The sky picture's colours.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkyColors {
    pub zenith: Rgba,
    pub horizon: Rgba,
    pub ground_near: Rgba,
    pub ground_far: Rgba,
    pub line: Rgba,
    pub sun: Rgba,
    pub plate: Rgba,
    pub ink: Rgba,
}

/// The lightmap page's packing and encoding.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LightData {
    pub texels_per_unit: f32,
    pub min_density: f32,
    pub max_page: u32,
    pub gap: u32,
    pub constant_side: u32,
    pub half: f32,
    pub bias: f32,
}

/// The textures drawn in code.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureData {
    pub scale: u32,
    pub colors: TextureColors,
    pub ruler: RulerData,
}

/// The floor texture's colours.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextureColors {
    pub check_light: Rgba,
    pub check_dark: Rgba,
    pub line: Rgba,
    pub heavy: Rgba,
    pub axis_x: Rgba,
    pub axis_y: Rgba,
    pub subject: Rgba,
    pub pad: Rgba,
    pub plate: Rgba,
    pub ink: Rgba,
}

/// The ruler texture.
#[derive(Clone, Copy, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RulerData {
    pub texels: u32,
    pub base: Rgba,
    pub tick: Rgba,
    pub edge: Rgba,
}

impl StageData {
    /// Parses and checks a manifest.
    pub fn parse(text: &str) -> Result<StageData> {
        let d: StageData = serde_yaml_ng::from_str(text).context("the stage manifest")?;
        ensure!(d.item == "stage", "the stage manifest is for item {}", d.item);
        let s = &d.sky;
        ensure!(s.segments > 0 && s.rings > 0, "the sky dome has no quads");
        ensure!(s.width > 0 && s.height > 0, "the sky picture is empty");
        ensure!(s.disc < s.glow, "the sun's disc is not inside its glow");
        let l = &d.light;
        ensure!(l.max_page.is_power_of_two(), "the lightmap page is not a power of two");
        ensure!(l.min_density > 0.0 && l.min_density <= l.texels_per_unit, "the lightmap density range");
        ensure!(l.half > 0.0, "the lightmap's full-light byte");
        ensure!(d.texture.scale > 0 && d.texture.ruler.texels > 0, "an empty texture scale");
        Ok(d)
    }
}

/// The shipped manifest, parsed once.
pub fn data() -> &'static StageData {
    static DATA: OnceLock<StageData> = OnceLock::new();
    DATA.get_or_init(|| StageData::parse(SOURCE).expect("content/stage/kit.yaml parses"))
}
