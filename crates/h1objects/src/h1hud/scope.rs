//! The scope mask in the weapon HUD's screen effect, and the Halo 2 HUD
//! screen-effect entry that draws it. Both draw before the weapon widgets.

use h1map::bitmap::{Bitmap, ImageKind, Sequence};
use h1map::{Group, H1Map, TagId as H1TagId, TagRef};
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, Val};

use super::wphi;
use crate::{Converted, Error, Result, h1};

const H1_BLOCK: usize = 0xac;
const H1_EFFECT_SIZE: usize = 0xb8;

/// The full- and split-screen masks. Halo 1 reads the first effect of
/// the weapon's own wphi (0x000d6b37), then chooses by player count
/// (0x000d6b8b). The references name the bitmap's first image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeMask {
    pub full: Option<H1TagId>,
    pub split: Option<H1TagId>,
    pub zoom_only: bool,
}

impl ScopeMask {
    pub fn read(map: &H1Map, wphi: H1TagId) -> Result<Option<Self>> {
        let tag = map.tag(wphi)?;
        let root = h1::tag_bytes(map, tag, b"wphi", wphi::SIZE)?;
        let Some(bytes) = h1::elements(map, root, H1_BLOCK, H1_EFFECT_SIZE)?
            .into_iter()
            .next()
        else {
            return Ok(None);
        };
        Self::parse(bytes)
    }

    fn parse(effect: &[u8]) -> Result<Option<Self>> {
        let bitmap = |at| {
            let r = TagRef::read(effect, at).ok_or(Error::Truncated("wphi screen effect"))?;
            if r.id.is_none() {
                return Ok(None);
            }
            if r.group != Group::new(b"bitm") {
                return Err(Error::WrongGroup(r.group.to_string(), *b"bitm"));
            }
            Ok(Some(r.id))
        };
        let full = bitmap(0x18)?;
        let split = bitmap(0x28)?;
        Ok((full.is_some() || split.is_some()).then_some(Self {
            full,
            split,
            zoom_only: effect[4] & 1 != 0,
        }))
    }

    /// Halo 2 selects the mask image by sequence, in a single bitmap.
    /// Preserve each source image's pixels, including the luminance the
    /// scope pass uses, and give each present screen mode one sequence.
    pub(crate) fn bitmap(self, map: &H1Map) -> Result<(Converted, [i8; 3])> {
        let mut bitmap = None::<Bitmap>;
        let mut raw = Vec::new();
        let mut sequences = [-1; 2];
        for (mode, id) in [self.full, self.split].into_iter().enumerate() {
            let Some(id) = id else { continue };
            if mode == 1 && self.full == Some(id) {
                sequences[mode] = sequences[0];
                continue;
            }
            let source = Bitmap::read(map, map.tag(id)?)?;
            let image = source
                .images
                .first()
                .filter(|i| i.kind == ImageKind::D2)
                .ok_or_else(|| Error::Invalid("scope mask has no 2D image".into()))?
                .clone();
            let pixels = source.pixels(map, 0)?;
            let target = bitmap.get_or_insert_with(|| Bitmap {
                sequences: Vec::new(),
                images: Vec::new(),
                ..source
            });
            let index = target.images.len();
            target.images.push(image);
            target.sequences.push(Sequence {
                name: if mode == 0 {
                    "fullscreen"
                } else {
                    "split_screen"
                }
                .into(),
                first_image: index as i16,
                image_count: 1,
                sprites: Vec::new(),
            });
            sequences[mode] = index as i8;
            raw.push(pixels);
        }
        let bitmap = bitmap.ok_or_else(|| Error::Invalid("scope mask has no images".into()))?;
        let (converted, _) = h1bitmaps::convert::convert_with(&bitmap, |i| Ok(raw[i].to_vec()))
            .map_err(|e| Error::Invalid(format!("scope bitmap: {e}")))?;
        Ok((converted, [sequences[0], sequences[1], sequences[1]]))
    }
}

fn set(record: &mut Record, name: &str, value: Val) -> Result<()> {
    record
        .set(name, value)
        .map_err(|e| Error::Invalid(e.to_string()))
}

/// One unfiltered pass on the active viewport. Coordinate source 1
/// (0x0002d5e0) normalizes the actual corners by the window bounds, so
/// the bitmap covers the entire view in every split and aspect ratio.
pub(crate) fn effect(shader: TagId) -> Result<Block> {
    let def = Defs::builtin()
        .group(FourCC(*b"egor"))
        .expect("egor definition");
    let mut root = defs::default_record(def);
    let mut pass = defs::default_record(def.child("passes").expect("egor passes"));
    set(
        &mut pass,
        "texture_coordinate_sources",
        Val::List([1, 0, 0, 0].into_iter().map(Val::Int).collect()),
    )?;
    set(&mut root, "shader", Val::tag(FourCC(*b"shad"), shader))?;
    set(&mut root, "passes", Val::Block(vec![pass]))?;
    defs::to_built(def, &root, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

/// Installs the screen-effect block on the fresh weapon HUD.
pub(crate) fn attach(
    hud: &mut Block,
    mask: ScopeMask,
    bitmap: TagId,
    sequences: [i8; 3],
    effect: TagId,
) -> Result<()> {
    let def = Defs::builtin()
        .group(FourCC(*b"nhdt"))
        .expect("nhdt definition");
    let child = def.child("screen_effects").expect("nhdt screen effects");
    let mut record = defs::default_record(child);
    for (name, datum) in ["effect_full", "effect_half", "effect_quarter"]
        .into_iter()
        .zip(sequences)
    {
        let id = if datum >= 0 { effect } else { TagId::NULL };
        set(&mut record, name, Val::tag(FourCC(*b"egor"), id))?;
    }
    // The weapon and zoom gates used by the retail scope widgets.
    let (show, hide) = if mask.zoom_only {
        (0x190, 0x40)
    } else {
        (0x1d0, 0)
    };
    for (name, words) in [
        ("show_when", [show, 0, 1, 0]),
        ("hide_when", [hide, 0, 4, 0]),
    ] {
        set(
            &mut record,
            name,
            Val::List(words.into_iter().map(Val::Int).collect()),
        )?;
    }
    set(
        &mut record,
        "anchor",
        Val::Int(super::Anchor::Centre as i64),
    )?;
    set(&mut record, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap))?;
    set(
        &mut record,
        "sequences",
        Val::List(
            sequences
                .into_iter()
                .map(|i| Val::Int(i64::from(i)))
                .collect(),
        ),
    )?;
    let block =
        defs::to_built(child, &record, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))?;
    hud.block(0x20, 1, block);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_reads_both_masks_and_selects_them_by_viewport() {
        let mut bytes = [0; H1_EFFECT_SIZE];
        bytes[4] = 1;
        let full = H1TagId(0xe100_0001);
        let split = H1TagId(0xe101_0002);
        for (at, id) in [(0x18, full), (0x28, split)] {
            bytes[at..at + 4].copy_from_slice(b"mtib");
            bytes[at + 0xc..at + 0x10].copy_from_slice(&id.0.to_le_bytes());
        }
        let mask = ScopeMask::parse(&bytes).unwrap().unwrap();
        assert_eq!(
            mask,
            ScopeMask {
                full: Some(full),
                split: Some(split),
                zoom_only: true
            }
        );
        let bitmap = TagId(0xe200_0001);
        let effect = TagId(0xe201_0002);
        let mut root = super::super::nhdt(&[], &[]);
        attach(&mut root, mask, bitmap, [0, 1, 1], effect).unwrap();
        let d = Defs::builtin().group(FourCC(*b"nhdt")).unwrap();
        let r = defs::from_built(d, &root).unwrap();
        assert_eq!(
            r.at("screen_effects[0].bitmap"),
            Some(&Val::tag(FourCC(*b"bitm"), bitmap))
        );
        for mode in ["full", "half", "quarter"] {
            let path = format!("screen_effects[0].effect_{mode}");
            assert_eq!(r.at(&path), Some(&Val::tag(FourCC(*b"egor"), effect)));
        }
        assert_eq!(
            r.at("screen_effects[0].sequences"),
            Some(&Val::List(vec![Val::Int(0), Val::Int(1), Val::Int(1)]))
        );
        assert_eq!(
            r.at("screen_effects[0].hide_when"),
            Some(&Val::List(vec![
                Val::Int(0x40),
                Val::Int(0),
                Val::Int(4),
                Val::Int(0)
            ]))
        );
    }

    #[test]
    fn scope_pass_uses_the_current_viewport_as_its_texture_domain() {
        let block = effect(TagId(0xe200_0001)).unwrap();
        let def = Defs::builtin().group(FourCC(*b"egor")).unwrap();
        let r = defs::from_built(def, &block).unwrap();
        assert_eq!(r.at("passes[0].shader_pass"), Some(&Val::Int(-1)));
        assert_eq!(r.at("passes[0].render_target"), Some(&Val::Int(0)));
        assert_eq!(
            r.at("passes[0].texture_coordinate_sources"),
            Some(&Val::List(vec![
                Val::Int(1),
                Val::Int(0),
                Val::Int(0),
                Val::Int(0)
            ]))
        );
    }

    #[test]
    fn a_missing_split_mask_disables_only_split_views() {
        let mut bytes = [0; H1_EFFECT_SIZE];
        bytes[0x18..0x1c].copy_from_slice(b"mtib");
        bytes[0x24..0x28].copy_from_slice(&0xe100_0001_u32.to_le_bytes());
        bytes[0x34..0x38].fill(0xff);
        let mask = ScopeMask::parse(&bytes).unwrap().unwrap();
        assert_eq!(mask.split, None);
        assert!(!mask.zoom_only);
        let mut root = super::super::nhdt(&[], &[]);
        attach(
            &mut root,
            mask,
            TagId(0xe200_0001),
            [0, -1, -1],
            TagId(0xe201_0002),
        )
        .unwrap();
        let d = Defs::builtin().group(FourCC(*b"nhdt")).unwrap();
        let r = defs::from_built(d, &root).unwrap();
        let null = Val::tag(FourCC(*b"egor"), TagId::NULL);
        assert_ne!(r.at("screen_effects[0].effect_full"), Some(&null));
        assert_eq!(r.at("screen_effects[0].effect_half"), Some(&null));
        assert_eq!(r.at("screen_effects[0].effect_quarter"), Some(&null));
    }
}
