//! Stage images for the translated passes: each material stage's bitmap
//! decoded as its stage samples it, a palettized one (P8, P8 bump)
//! expanded through the palette bound at its stage, once per image
//! ([`StageDecoder`]). The game's glue gives it the device's palettes;
//! the editor gives it the default palette at every stage but the
//! lightmap's.

use std::collections::{HashMap, HashSet};

use map::{Map, TagId};

use crate::gpu::Image;

/// The device palette at each texture stage (`A8R8G8B8` entries), or
/// `None`: what a palettized stage bitmap (P8, P8 bump) is sampled through
/// on the D3D path. A world keeps those of the stages it expanded
/// bitmaps through.
pub type StagePalettes = [Option<Vec<u32>>; 4];

/// Stage images by (bitmap, image index).
pub type StageImages = HashMap<(TagId, u32), Image>;

/// Stage images as their stages sample them, for every native pass that
/// loads with the world (its layer 3, the transparents; the objects can
/// take it too): a palettized bitmap (P8, P8 bump) is expanded through the
/// device palette bound at its stage when the load started
/// ([`stage_image`]), once per image, and another stage may share that
/// expansion only when it holds the same palette. Records which palettes
/// it used (the host checks they stay bound) and which images do not
/// decode.
#[derive(Default)]
pub struct StageDecoder {
    /// The palettes bound at each stage when the load started.
    palettes: StagePalettes,
    /// Those an expansion used.
    used: StagePalettes,
    /// The stage each palettized image was expanded at.
    expanded_at: HashMap<(TagId, u32), usize>,
    /// Images that do not decode at any stage.
    failed: HashMap<(TagId, u32), String>,
    /// Cached images whose D3D coordinates are in texels.
    linear: HashSet<(TagId, u32)>,
}

impl StageDecoder {
    pub fn new(palettes: StagePalettes) -> Self {
        Self {
            palettes,
            ..Self::default()
        }
    }

    /// The palettes the expansions used, at their stages (the rest
    /// `None`).
    pub fn used(&self) -> &StagePalettes {
        &self.used
    }

    /// Image `image` of `bitmap` as stage `stage` samples it, put in
    /// `images` unless there already: `true` when it is palettized (and
    /// so went through the stage's palette), or why the stage cannot have
    /// it, as `stage <n> bitmap <why>`. `refuse_linear`: a linear texture
    /// is refused (it is not cached as failed: another pass may take it).
    #[allow(clippy::too_many_arguments)]
    pub fn decode<R: std::io::Read + std::io::Seek>(
        &mut self,
        map: &mut Map<R>,
        files: &mut assets::ResourceFiles,
        images: &mut StageImages,
        bitmap: TagId,
        image: u32,
        stage: usize,
        refuse_linear: bool,
    ) -> Result<bool, String> {
        let key = (bitmap, image);
        if refuse_linear && self.linear.contains(&key) {
            return Err(format!("stage {stage} bitmap is a linear texture"));
        }
        let mut linear = false;
        let decoded = self.take(images, key, stage, &mut |palette| {
            let (image, expanded, layout) =
                decode_stage(map, files, bitmap, image as usize, stage, palette)?;
            linear = layout == gfx::format::Layout::Linear;
            if refuse_linear && linear {
                return Err(StageRefusal::Here("is a linear texture"));
            }
            Ok((image, expanded))
        });
        if linear && decoded.is_ok() {
            self.linear.insert(key);
        }
        decoded
    }

    /// [`decode`](Self::decode) with the decode itself as `decode` (given
    /// the stage's palette).
    pub fn take(
        &mut self,
        images: &mut StageImages,
        key: (TagId, u32),
        stage: usize,
        decode: &mut dyn FnMut(&Option<Vec<u32>>) -> Result<(Image, bool), StageRefusal>,
    ) -> Result<bool, String> {
        let palette = self
            .palettes
            .get(stage)
            .ok_or_else(|| format!("stage {stage} does not exist"))?;
        if let Some(&at) = self.expanded_at.get(&key) {
            // One expansion per image: this stage must hold the same
            // palette (and stage 3 holds the lightmap's).
            if stage == 3 || *palette != self.palettes[at] {
                return Err(format!(
                    "stage {stage} bitmap is palettized and was expanded through another stage's palette"
                ));
            }
            self.used[stage].clone_from(palette);
            return Ok(true);
        }
        if images.contains_key(&key) {
            return Ok(false);
        }
        if let Some(why) = self.failed.get(&key) {
            return Err(format!("stage {stage} bitmap {why}"));
        }
        match decode(palette) {
            Ok((i, expanded)) => {
                images.insert(key, i);
                if expanded {
                    self.expanded_at.insert(key, stage);
                    self.used[stage].clone_from(palette);
                }
                Ok(expanded)
            }
            Err(StageRefusal::Image(why)) => {
                let message = format!("stage {stage} bitmap {why}");
                self.failed.insert(key, why);
                Err(message)
            }
            // Not the image's fault: it may do at another stage or pass.
            Err(StageRefusal::Here(why)) => Err(format!("stage {stage} bitmap {why}")),
        }
    }
}

/// Why a stage cannot have a bitmap's image: the image itself, or only
/// here (this stage's palette, or a linear texture this pass refuses).
#[derive(Debug, PartialEq, Eq)]
pub enum StageRefusal {
    Image(String),
    Here(&'static str),
}

/// Image `index` of a stage's bitmap at stage `stage`, and whether it was
/// expanded through the stage's palette; or why the stage cannot have it:
/// it does not decode, or it is palettized and the stage has no palette
/// for it ([`stage_image`]). The caller uses the layout to refuse linear
/// textures where the native path cannot use the D3D texel coordinates.
fn decode_stage<R: std::io::Read + std::io::Seek>(
    map: &mut Map<R>,
    files: &mut assets::ResourceFiles,
    id: TagId,
    index: usize,
    stage: usize,
    palette: &Option<Vec<u32>>,
) -> Result<(Image, bool, gfx::format::Layout), StageRefusal> {
    let tags::TagData::Bitmap(b) = tags::DecodedTag::read(map, id)
        .map_err(|_| StageRefusal::Image("does not read".into()))?
        .data
    else {
        return Err(StageRefusal::Image("is not a bitmap".into()));
    };
    let image = b
        .images
        .get(index)
        .ok_or_else(|| StageRefusal::Image("has no such image".into()))?;
    let (d, blocks) = assets::decode_image_blocks(image, files)
        .map_err(|e| StageRefusal::Image(format!("does not decode ({e})")))?;
    let layout = d.desc.format.layout();
    let (image, palettized) = stage_image(image.format, d.levels, stage, palette)?;
    Ok((with_blocks(image, blocks), palettized, layout))
}

/// `image` with its BC blocks, when the D3D path uploads the bitmap as BC
/// (`assets::decode_image_blocks`): the native pass then samples the same
/// GPU-decoded texels. The CPU decoder (`gfx::dxt`) truncates the
/// interpolated colours, the GPU rounds them, which left native lit
/// surfaces one level darker than the D3D path's on a few percent of
/// pixels.
pub fn with_blocks(image: Image, blocks: Option<assets::BlockLevels>) -> Image {
    let Some(b) = blocks else { return image };
    let mut levels: Vec<_> = b
        .levels
        .into_iter()
        .filter(|l| l.face == 0 && l.depth <= 1)
        .collect();
    levels.sort_by_key(|l| l.level);
    image.with_blocks(
        b.host.to_wgpu(),
        levels.into_iter().map(|l| l.rgba).collect(),
    )
}

/// A stage bitmap's decoded levels as the stage samples them, and whether
/// they went through its palette. A palettized format (P8, P8 bump)
/// decodes as its index in every channel, and the D3D path samples it
/// through the device palette bound at its stage: so it is expanded
/// through `palette`, and refused at stage 3 (where the structure draw
/// binds each lightmap's own palette, `0x00015c90`) or without a full
/// palette. For the world's layer 3, stages 0..2 hold the device's default
/// palette (bound on every stage at `0x00015b70`; only the lightmap binder
/// binds another, at stage 3).
pub fn stage_image(
    format: tags::PixelFormat,
    levels: Vec<assets::Level>,
    stage: usize,
    palette: &Option<Vec<u32>>,
) -> Result<(Image, bool), StageRefusal> {
    let palette: Option<[u32; 256]> =
        if matches!(format, tags::PixelFormat::P8 | tags::PixelFormat::P8Bump) {
            if stage == 3 {
                return Err(StageRefusal::Here(
                    "is palettized at stage 3 (the lightmap's palette is bound there)",
                ));
            }
            let p = palette.as_ref().ok_or(StageRefusal::Here(
                "is palettized and no palette is bound at its stage",
            ))?;
            Some(p.as_slice().try_into().map_err(|_| {
                StageRefusal::Here(
                    "is palettized and its stage's palette has fewer than 256 entries",
                )
            })?)
        } else {
            None
        };
    let mut levels: Vec<_> = levels
        .into_iter()
        .filter(|l| l.face == 0 && l.depth <= 1)
        .collect();
    levels.sort_by_key(|l| l.level);
    let image = Image::from_levels(
        levels
            .into_iter()
            .map(|l| match &palette {
                // Decoded as L8: the index in every channel.
                Some(p) => {
                    let indices: Vec<u8> = l.rgba.chunks_exact(4).map(|t| t[0]).collect();
                    (l.width, l.height, crate::load::expand_page(&indices, p))
                }
                None => (l.width, l.height, l.rgba),
            })
            .collect(),
    )
    .ok_or_else(|| StageRefusal::Image("does not decode".into()))?;
    Ok((image, palette.is_some()))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 level and its 1x1 mip as `assets` decodes a P8 image: the
    /// index in every channel.
    fn indexed(indices: [u8; 4]) -> Vec<assets::Level> {
        let level = |level, width, idx: &[u8]| assets::Level {
            face: 0,
            level,
            width,
            height: width,
            depth: 1,
            rgba: idx.iter().flat_map(|&i| [i, i, i, 0xff]).collect(),
        };
        vec![level(1, 1, &indices[..1]), level(0, 2, &indices)]
    }

    /// A palette of tangent-space normals packed as colours, `A8R8G8B8`.
    fn normals() -> Vec<u32> {
        (0..256u32)
            .map(|i| 0xff00_0000 | ((i ^ 0x80) << 16) | ((0xff - i) << 8) | 0xfe)
            .collect()
    }

    #[test]
    fn a_palettized_bump_map_samples_its_stage_palette_not_its_index() {
        // The 03a A/B: a P8 bump map sampled as its index (grey) gave the
        // bump term a wrong normal, so `r0` clamped to black on most walls
        // and over-brightened others. The stage samples the palette entry,
        // as the D3D path does through the palette bound at that stage.
        let (image, expanded) = stage_image(
            tags::PixelFormat::P8Bump,
            indexed([0, 1, 0x80, 0xff]),
            2,
            &Some(normals()),
        )
        .unwrap();
        assert!(expanded);
        assert_eq!((image.width, image.height), (2, 2));
        let texel = |i: usize| <[u8; 4]>::try_from(&image.rgba[4 * i..4 * i + 4]).unwrap();
        assert_eq!(texel(0), [0x80, 0xff, 0xfe, 0xff]);
        assert_eq!(texel(1), [0x81, 0xfe, 0xfe, 0xff]);
        assert_eq!(texel(2), [0x00, 0x7f, 0xfe, 0xff]);
        assert_eq!(texel(3), [0x7f, 0x00, 0xfe, 0xff]);
        // The mip goes through the same palette.
        assert_eq!(image.mips, vec![vec![0x80, 0xff, 0xfe, 0xff]]);
        // Plain P8 the same.
        assert!(
            stage_image(tags::PixelFormat::P8, indexed([0; 4]), 0, &Some(normals()))
                .unwrap()
                .1
        );
    }

    #[test]
    fn a_palettized_stage_without_its_palette_stays_on_the_d3d_path() {
        let p8 = tags::PixelFormat::P8Bump;
        let palette = |e: StageRefusal| matches!(e, StageRefusal::Here(_));
        assert!(palette(
            stage_image(p8, indexed([0; 4]), 1, &None).unwrap_err()
        ));
        // Stage 3 holds each lightmap's palette.
        assert!(palette(
            stage_image(p8, indexed([0; 4]), 3, &Some(normals())).unwrap_err()
        ));
        // A palette of fewer entries than the indices can reach.
        assert!(palette(
            stage_image(p8, indexed([0; 4]), 0, &Some(normals()[..32].to_vec())).unwrap_err()
        ));
        // A direct format keeps its colours and needs no palette.
        let (image, expanded) =
            stage_image(tags::PixelFormat::Argb8888, indexed([9, 8, 7, 6]), 3, &None).unwrap();
        assert!(!expanded);
        assert_eq!(&image.rgba[..4], &[9, 9, 9, 0xff]);
    }
}
