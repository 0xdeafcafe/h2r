//! Weapon message sprites from the HUD globals' bitmap atlas.

use crate::bitmaps::decode::{BitmapExt, Rgba8Image};
use h1map::bitmap::{Bitmap, Sprite};
use h1map::{Group, H1Map, TagRef};

use crate::text::hud::{SILHOUETTE_BASE, SILHOUETTE_LIMIT};
use crate::text::{Error, Result};

#[derive(PartialEq, Eq)]
pub(crate) struct Silhouette {
    pub width: u16,
    pub height: u16,
    pub x_offset: i16,
    pub y_offset: i16,
    pub advance: i16,
    pub pixels: Vec<u8>,
}

impl Silhouette {
    pub fn cache_key(&self) -> u32 {
        // Halo 2 uses nonnegative font numbers. Give the same sprite in
        // different maps the same private font key, including its metrics.
        [
            self.width,
            self.height,
            self.x_offset as u16,
            self.y_offset as u16,
            self.advance as u16,
        ]
        .into_iter()
        .flat_map(u16::to_le_bytes)
        .chain(self.pixels.iter().copied())
        .fold(0x811c_9dc5u32, |hash, byte| {
            (hash ^ u32::from(byte)).wrapping_mul(0x0100_0193)
        }) & 0x3fff_ffff
            | 0x8000_0000
    }
}

pub(crate) struct Atlas {
    bitmap: Bitmap,
    images: Vec<Option<Rgba8Image>>,
}

impl Atlas {
    pub fn read(map: &H1Map, globals: &[u8]) -> Result<Option<Self>> {
        let Some(reference) = TagRef::read(globals, 0xa4).filter(|r| !r.id.is_none()) else {
            return Ok(None);
        };
        let bitmap = Bitmap::read(map, map.tag(reference.id)?)?;
        let images = vec![None; bitmap.images.len()];
        Ok(Some(Self { bitmap, images }))
    }

    pub fn weapon(
        &mut self,
        map: &H1Map,
        weapon: &[u8],
        glyphs: &mut Vec<Silhouette>,
    ) -> Result<Option<char>> {
        let Some(reference) = TagRef::read(weapon, 0x480).filter(|r| !r.id.is_none()) else {
            return Ok(None);
        };
        let tag = map.tag(reference.id)?;
        if tag.group != Group::new(b"wphi") {
            return Ok(None);
        }
        // Halo 1 0xe2ed5 passes this icon record to its HUD formatter.
        let bytes = map.tag_data(tag)?;
        let Some(icon) = bytes.get(0x13c..0x14a) else {
            return Ok(None);
        };
        let field = |at: usize| i16::from_le_bytes([icon[at], icon[at + 1]]);
        let Some(sprite) = usize::try_from(field(0))
            .ok()
            .and_then(|index| self.bitmap.sequences.get(index))
            .and_then(|sequence| sequence.sprites.first())
        else {
            return Ok(None);
        };
        let index = usize::try_from(sprite.image)
            .ok()
            .filter(|&index| index < self.images.len())
            .ok_or_else(|| Error::Invalid("HUD weapon sprite has no bitmap image".into()))?;
        if self.images[index].is_none() {
            self.images[index] =
                Some(self.bitmap.decode(map, index, 0).map_err(|e| {
                    Error::Invalid(format!("HUD weapon sprite image {index}: {e}"))
                })?);
        }
        let glyph = crop(
            self.images[index].as_ref().unwrap(),
            sprite,
            [field(4), field(6)],
            (icon[0xd] & 4 != 0).then(|| field(2)),
        )?;
        let row = match glyphs.iter().position(|previous| previous == &glyph) {
            Some(row) => row,
            None => {
                let row = glyphs.len();
                if row >= usize::from(SILHOUETTE_LIMIT) {
                    return Err(Error::Invalid("too many HUD weapon silhouettes".into()));
                }
                glyphs.push(glyph);
                row
            }
        };
        Ok(char::from_u32(u32::from(SILHOUETTE_BASE) + row as u32))
    }
}

fn crop(
    image: &Rgba8Image,
    sprite: &Sprite,
    offset: [i16; 2],
    explicit_advance: Option<i16>,
) -> Result<Silhouette> {
    let [left, right, top, bottom] = sprite.bounds;
    if ![left, right, top, bottom]
        .iter()
        .all(|v| (0.0..=1.0).contains(v))
    {
        return Err(Error::Invalid("HUD weapon sprite UV bounds".into()));
    }
    let x = |v: f32| (v * image.width as f32).round() as usize;
    let y = |v: f32| (v * image.height as f32).round() as usize;
    let (left, right, top, bottom) = (x(left), x(right), y(top), y(bottom));
    let width = right.saturating_sub(left);
    let height = bottom.saturating_sub(top);
    if width == 0 || height == 0 || width > 128 || height > 128 || width * height > 4096 {
        return Err(Error::Invalid(format!(
            "HUD weapon sprite dimensions {width}x{height}"
        )));
    }
    let mut pixels = Vec::with_capacity(2 * width * height);
    for row in top..bottom {
        let start = 4 * (row * image.width as usize + left);
        for &[r, g, b, a] in image.rgba[start..start + 4 * width].as_chunks::<4>().0 {
            let pixel = u16::from(a >> 4) << 12
                | u16::from(r >> 4) << 8
                | u16::from(g >> 4) << 4
                | u16::from(b >> 4);
            pixels.extend(pixel.to_le_bytes());
        }
    }
    Ok(Silhouette {
        width: width as u16,
        height: height as u16,
        x_offset: offset[0],
        y_offset: (height as i16).saturating_add(offset[1]),
        advance: explicit_advance.unwrap_or(width as i16),
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crops_the_sprite_rows_and_retains_alpha() {
        let image = Rgba8Image {
            width: 3,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 255, 255, 255, 0, 0, 255, 0, 128, 0, 255, 255, 255, 0, 0, 255, 255,
                255, 255, 0, 64,
            ],
        };
        let sprite = Sprite {
            image: 0,
            bounds: [1.0 / 3.0, 1.0, 0.0, 1.0],
            registration: [0.0; 2],
        };
        let glyph = crop(&image, &sprite, [1, 2], Some(4)).unwrap();
        assert_eq!(
            (
                glyph.width,
                glyph.height,
                glyph.x_offset,
                glyph.y_offset,
                glyph.advance
            ),
            (2, 2, 1, 4, 4)
        );
        assert_eq!(
            glyph.pixels,
            [0xff, 0x0f, 0xf0, 0x80, 0x0f, 0xf0, 0xf0, 0x4f]
        );
    }
}
