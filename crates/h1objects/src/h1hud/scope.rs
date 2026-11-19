//! The scope mask in the weapon HUD's screen effect, and the Halo 2 HUD
//! screen-effect entry that draws it. Both draw before the weapon widgets.

use h1map::bitmap::{Bitmap, ImageKind, Sequence};
use h1map::{Group, H1Map, TagId as H1TagId, TagRef};
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, Val};
use tags::scope_zoom::{Blur, Filter, Profile};

use super::wphi;
use crate::{Converted, Error, Result, h1};

const H1_BLOCK: usize = 0xac;
const H1_EFFECT_SIZE: usize = 0xb8;

/// The full- and split-screen masks. Halo 1 reads the first effect of
/// the weapon's own wphi (0x000d6b37), then chooses by player count
/// (0x000d6b8b). The references name the bitmap's first image.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScopeMask {
    pub full: Option<H1TagId>,
    pub split: Option<H1TagId>,
    pub zoom_only: bool,
    pub filters: Profile,
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
        let word = |at| u16::from_le_bytes(effect[at..at + 2].try_into().unwrap());
        let float = |at| f32::from_le_bytes(effect[at..at + 4].try_into().unwrap());
        let filter = |at| Filter {
            flags: word(at),
            channel: word(at + 2),
            strength: float(at + 4),
        };
        let filters = Profile {
            blur: Blur {
                zoom_only: word(0x40) & 1 != 0,
                fov: [float(0x44), float(0x48)],
                radius: [float(0x4c), float(0x50)],
            },
            desaturation: filter(0x6c),
            tint: filter(0x8c),
            tint_rgb: [float(0x94), float(0x98), float(0x9c)],
        };
        Ok((full.is_some() || split.is_some() || filters.enabled()).then_some(Self {
            full,
            split,
            zoom_only: effect[4] & 1 != 0,
            filters,
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

/// The view copy and optional convolution run before the colour composite
/// and the mask overlay. Each entry selects the corresponding layer-15 pass.
pub(crate) fn effect(shader: TagId, profile: Profile, masked: bool) -> Result<Block> {
    let def = Defs::builtin()
        .group(FourCC(*b"egor"))
        .expect("egor definition");
    let mut root = defs::default_record(def);
    let pass_def = def.child("passes").expect("egor passes");
    let pass = |target, coordinate_sources: [i64; 4]| -> Result<Record> {
        let mut pass = defs::default_record(pass_def);
        set(&mut pass, "render_target", Val::Int(target))?;
        set(
            &mut pass,
            "texture_coordinate_sources",
            Val::List(coordinate_sources.into_iter().map(Val::Int).collect()),
        )?;
        Ok(pass)
    };
    let mut passes = Vec::new();
    if profile.enabled() {
        set(
            &mut root,
            "h1_zoom_profile",
            Val::List(
                profile
                    .words()
                    .into_iter()
                    .map(|n| Val::Int(n.into()))
                    .collect(),
            ),
        )?;
        let mut copy = pass(1, [3, 0, 0, 0])?;
        if profile.blur.enabled() {
            let filter_def = pass_def.child("filters").expect("egor filters");
            let mut filter = defs::default_record(filter_def);
            for (key, value) in [
                ("steps", 2.0),
                ("spread", 1.0),
                ("growth", 0.25),
                ("resolution_scale", 0.5),
            ] {
                set(&mut filter, key, Val::Float(value))?;
            }
            set(&mut copy, "filters", Val::Block(vec![filter]))?;
        }
        passes.push(copy);
        passes.push(pass(0, [1; 4])?);
    }
    if masked {
        passes.push(pass(0, [1, 0, 0, 0])?);
    }
    set(&mut root, "shader", Val::tag(FourCC(*b"shad"), shader))?;
    set(&mut root, "passes", Val::Block(passes))?;
    defs::to_built(def, &root, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}

#[derive(Clone, Copy)]
pub(crate) enum Visibility {
    Always,
    Zoomed,
    Unzoomed,
}

pub(crate) struct Entry {
    pub visibility: Visibility,
    pub bitmap: TagId,
    pub sequences: [i8; 3],
    pub effects: [TagId; 3],
}

/// Installs the screen-effect records on a fresh weapon HUD.
pub(crate) fn attach(hud: &mut Block, entries: &[Entry]) -> Result<()> {
    let def = Defs::builtin()
        .group(FourCC(*b"nhdt"))
        .expect("nhdt definition");
    let child = def.child("screen_effects").expect("nhdt screen effects");
    let entries = entries
        .iter()
        .map(|entry| {
            let mut record = defs::default_record(child);
            for (name, id) in ["effect_full", "effect_half", "effect_quarter"]
                .into_iter()
                .zip(entry.effects)
            {
                set(&mut record, name, Val::tag(FourCC(*b"egor"), id))?;
            }
            // The weapon and zoom gates used by the retail scope widgets.
            let (show, hide) = match entry.visibility {
                Visibility::Always => (0x1d0, 0),
                Visibility::Zoomed => (0x190, 0x40),
                Visibility::Unzoomed => (0x1d0, 0x180),
            };
            let inactive = super::weapon_word_c::OFF_HAND | super::weapon_word_c::BACKPACK;
            for (name, words) in [
                ("show_when", [show, 0, 1, 0]),
                ("hide_when", [hide, 0, i64::from(inactive), 0]),
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
            set(&mut record, "bitmap", Val::tag(FourCC(*b"bitm"), entry.bitmap))?;
            set(
                &mut record,
                "sequences",
                Val::List(entry.sequences.into_iter().map(|i| Val::Int(i.into())).collect()),
            )?;
            Ok(record)
        })
        .collect::<Result<Vec<_>>>()?;
    let mut root = defs::default_record(def);
    set(&mut root, "screen_effects", Val::Block(entries))?;
    let mut built =
        defs::to_built(def, &root, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))?;
    hud.fixups.append(&mut built.fixups);
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
                zoom_only: true,
                filters: Profile::default(),
            }
        );
        let bitmap = TagId(0xe200_0001);
        let effect = TagId(0xe201_0002);
        let mut root = super::super::nhdt(&[], &[]);
        attach(
            &mut root,
            &[Entry {
                visibility: Visibility::Zoomed,
                bitmap,
                sequences: [0, 1, 1],
                effects: [effect; 3],
            }],
        )
        .unwrap();
        let d = Defs::builtin().group(FourCC(*b"nhdt")).unwrap();
        let r = defs::from_built(d, &root).unwrap();
        // A builder tree reads a reference fixup back with group 0: the
        // builder writes the target's group itself.
        let built = |id| Val::Ref {
            group: 0,
            target: defs::Target::Id(id),
        };
        assert_eq!(r.at("screen_effects[0].bitmap"), Some(&built(bitmap)));
        for mode in ["full", "half", "quarter"] {
            let path = format!("screen_effects[0].effect_{mode}");
            assert_eq!(r.at(&path), Some(&built(effect)));
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
                Val::Int(6),
                Val::Int(0)
            ]))
        );
    }

    #[test]
    fn scope_pass_uses_the_current_viewport_as_its_texture_domain() {
        let block = effect(TagId(0xe200_0001), Profile::default(), true).unwrap();
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
    fn scope_parses_filters_even_without_either_mask() {
        let mut bytes = [0; H1_EFFECT_SIZE];
        for (at, value) in [
            (0x40, 1),
            (0x44, 1.0_f32.to_bits()),
            (0x48, 0.5_f32.to_bits()),
            (0x50, 12.0_f32.to_bits()),
            (0x6c, 0x0001_0002),
            (0x70, 0.5_f32.to_bits()),
            (0x8c, 0x0002_0009),
            (0x90, 0.25_f32.to_bits()),
            (0x98, 1.0_f32.to_bits()),
        ] {
            bytes[at..at + 4].copy_from_slice(&value.to_le_bytes());
        }
        bytes[0x24..0x28].fill(0xff);
        bytes[0x34..0x38].fill(0xff);

        let scope = ScopeMask::parse(&bytes).unwrap().unwrap();
        assert_eq!([scope.full, scope.split], [None, None]);
        assert_eq!(scope.filters.blur.at(0.75, true), 6.0);
        assert_eq!(
            scope.filters.desaturation,
            Filter {
                flags: 2,
                channel: 1,
                strength: 0.5
            },
        );
        assert_eq!(
            scope.filters.tint,
            Filter {
                flags: 9,
                channel: 2,
                strength: 0.25
            }
        );
        assert_eq!(scope.filters.tint_rgb, [0.0, 1.0, 0.0]);
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
            &[Entry {
                visibility: Visibility::Always,
                bitmap: TagId(0xe200_0001),
                sequences: [0, -1, -1],
                effects: [TagId(0xe201_0002), TagId::NULL, TagId::NULL],
            }],
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
