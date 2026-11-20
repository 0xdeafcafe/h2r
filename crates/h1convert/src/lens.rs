//! Halo 1 lens flares (`lens`) as Halo 2 `lens` tags, written from the
//! `lens` definition (`crates/tags/src/defs/lens.txt`) with no retail
//! template read. Used by the sky (each sky light's flare) and by
//! `h1objects` (lights' flares). Layouts and evidence:
//! docs/formats/tag-definitions.md, "Lens flares".
//!
//! Halo 1's `lens` (V, every lens of every map on the Halo 1 disc, 641
//! tags, `flares_disc.rs` `halo1_layout_survey`): a `0xf0` root, the
//! reflections at `+0xc4` (`0x80` each, the first at `+0xf0` on every
//! tag). `+0`/`+4` two angles in radians with their cosines at
//! `+8`/`+0xc`, `+0x10` a radius (0.001 to 50 world units), `+0x14` a
//! `u16` (0, 1 or 2), `+0x18`/`+0x1c` two distances (1 to 15, 4 to 250),
//! `+0x20` the bitmap, `+0x30` flags (one tag sets bit 0), `+0xa0`,
//! `+0xa4` two scales (0.1 to 8, 1 on most).

use h1map::{H1Map, Tag, TagRef};
use map::build::{Block, FixupKind, TagSet};
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};

use crate::{Error, Result};

/// Halo 1 `lens` root size (V: the reflections start here on every tag).
pub const H1_ROOT: usize = 0xf0;
/// Halo 1 reflections block field and element size (V: tags tile as
/// `0xf0 + n * 0x80`).
pub const H1_REFLECTIONS: usize = 0xc4;
pub const H1_REFLECTION: usize = 0x80;

/// One Halo 1 reflection: the element's bytes, read by offset
/// ([`REFLECTION_WORDS`]).
#[derive(Clone, Debug, PartialEq)]
pub struct H1Reflection {
    pub bytes: Vec<u8>,
}

/// A Halo 1 lens flare, as read for conversion.
#[derive(Clone, Debug, PartialEq)]
pub struct H1Lens {
    /// `+0`, `+4`: radians.
    pub falloff_angle: f32,
    pub cutoff_angle: f32,
    /// `+0x10`: world units.
    pub occlusion_radius: f32,
    /// `+0x18`, `+0x1c`: world units.
    pub near_fade_distance: f32,
    pub far_fade_distance: f32,
    /// `+0x20`.
    pub bitmap: Option<h1map::TagId>,
    /// `+0x30`.
    pub flags: u16,
    /// `+0x14` (0, 1 or 2).
    pub kind_14: u16,
    /// `+0xa0`, `+0xa4`.
    pub scales: [f32; 2],
    pub reflections: Vec<H1Reflection>,
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}
fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4"))
}
fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at))
}

impl H1Lens {
    /// Reads the Halo 1 `lens` tag `tag` of `map`.
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        if tag.group != h1map::Group::new(b"lens") {
            return Err(Error::Invalid("not a lens tag"));
        }
        let b = map.tag_data(tag)?;
        if b.len() < H1_ROOT {
            return Err(Error::Invalid("lens root truncated"));
        }
        let bitmap = TagRef::read(b, 0x20)
            .filter(|r| !r.id.is_none())
            .and_then(|r| map.tag(r.id).ok())
            .map(|t| t.id);
        let blk = h1map::Block::read(b, H1_REFLECTIONS).ok_or(Error::Invalid("lens reflections"))?;
        let reflections = map
            .block(blk, H1_REFLECTION)?
            .chunks_exact(H1_REFLECTION)
            .map(|e| H1Reflection { bytes: e.to_vec() })
            .collect();
        Ok(Self {
            falloff_angle: f32_at(b, 0),
            cutoff_angle: f32_at(b, 4),
            occlusion_radius: f32_at(b, 0x10),
            near_fade_distance: f32_at(b, 0x18),
            far_fade_distance: f32_at(b, 0x1c),
            bitmap,
            flags: u16_at(b, 0x30),
            kind_14: u16_at(b, 0x14),
            scales: [f32_at(b, 0xa0), f32_at(b, 0xa4)],
            reflections,
        })
    }
}

/// A Halo 2 reflection's fields from Halo 1's reflection `b` (V by value
/// sets over both discs, docs/formats/tag-definitions.md, "Lens flares"):
/// flags `+0`, bitmap index `+4`, position `+0x1c`, rotation offset
/// `+0x20`, radius bounds `+0x28`, brightness bounds `+0x34`, the tint's
/// red, green and blue `+0x44`.
fn reflection(e: &StructDef, b: &[u8]) -> Result<Record> {
    let mut r = defs::default_record(e);
    let fs = |at: usize, n: usize| Val::floats(&(0..n).map(|i| f32_at(b, at + 4 * i)).collect::<Vec<_>>());
    set(&mut r, "flags", Val::Int(u16_at(b, 0).into()))?;
    set(&mut r, "bitmap_index", Val::Int(u16_at(b, 4).into()))?;
    set(&mut r, "position", Val::Float(f32_at(b, 0x1c)))?;
    set(&mut r, "rotation_offset", Val::Float(f32_at(b, 0x20)))?;
    set(&mut r, "radius_bounds", fs(0x28, 2))?;
    set(&mut r, "brightness_bounds", fs(0x34, 2))?;
    set(&mut r, "tint_colour", fs(0x44, 3))?;
    Ok(r)
}

fn def() -> Result<&'static StructDef> {
    Defs::builtin()
        .group(FourCC(*b"lens"))
        .ok_or(Error::Invalid("no lens definition"))
}

fn set(r: &mut Record, path: &str, v: Val) -> Result<()> {
    r.set(path, v).map_err(|_| Error::Invalid("lens field"))
}

/// The Halo 2 `lens` root for Halo 1's `l`: its angles and their
/// cosines, occlusion radius, fade distances, bitmap, `+0x14` word and
/// two scales, and a reflection per Halo 1 reflection ([`reflection`]);
/// everything else the definition's default (no flags, no functions).
pub fn rebuild(l: &H1Lens, bitmap: Option<TagId>) -> Result<Block> {
    let d = def()?;
    let mut r = defs::default_record(d);
    set(&mut r, "falloff_angle", Val::Float(l.falloff_angle))?;
    set(&mut r, "cutoff_angle", Val::Float(l.cutoff_angle))?;
    set(&mut r, "falloff_cosine", Val::Float(l.falloff_angle.cos()))?;
    set(&mut r, "cutoff_cosine", Val::Float(l.cutoff_angle.cos()))?;
    set(&mut r, "occlusion_radius", Val::Float(l.occlusion_radius))?;
    set(&mut r, "near_fade_distance", Val::Float(l.near_fade_distance))?;
    set(&mut r, "far_fade_distance", Val::Float(l.far_fade_distance))?;
    set(
        &mut r,
        "bitmap",
        Val::tag(FourCC(*b"bitm"), bitmap.unwrap_or(TagId::NULL)),
    )?;
    let e = d.child("reflections").ok_or(Error::Invalid("lens reflections"))?;
    set(&mut r, "kind_14", Val::Int(l.kind_14.into()))?;
    set(&mut r, "scale_34", Val::Float(l.scales[0]))?;
    set(&mut r, "scale_38", Val::Float(l.scales[1]))?;
    let mut out = Vec::new();
    for x in &l.reflections {
        out.push(reflection(e, &x.bytes)?);
    }
    set(&mut r, "reflections", Val::Block(out))?;
    defs::to_built(d, &r, &mut |_| None).map_err(|_| Error::Invalid("lens record"))
}

/// What of Halo 1's `l` has no Halo 2 home here, for reports.
pub fn left_out(l: &H1Lens) -> Vec<String> {
    let mut v = Vec::new();
    if l.flags != 0 {
        v.push(format!("lens flags {:#x}: not carried", l.flags));
    }
    if !l.reflections.is_empty() {
        v.push(format!(
            "{} reflections: their scaling sources, colour bounds and animation not carried",
            l.reflections.len()
        ));
    }
    v
}

/// A Halo 1 lens flare ready to add to a set: the lens, and its bitmap
/// converted (`h1bitmaps`) with the bitmap's path.
#[derive(Clone, Debug)]
pub struct Flare {
    pub name: String,
    pub lens: H1Lens,
    pub bitmap: Option<(String, h1models::Converted)>,
}

impl Flare {
    /// Reads Halo 1's lens `id` of `map` and converts its bitmap.
    pub fn read(map: &H1Map, id: h1map::TagId) -> Result<Self> {
        let tag = map.tag(id)?;
        let lens = H1Lens::read(map, tag)?;
        let bitmap = match lens.bitmap {
            Some(b) => {
                let t = map.tag(b)?;
                Some((t.name.clone(), h1bitmaps::convert_bitmap(map, t)?.1))
            }
            None => None,
        };
        Ok(Self {
            name: tag.name.clone(),
            lens,
            bitmap,
        })
    }

    /// Adds the bitmap (unless the set has one of its path) and the lens
    /// (unless the set has a lens of its path); returns the lens's datum.
    pub fn install(&self, set: &mut TagSet) -> Result<TagId> {
        if let Some(id) = set.converted(FourCC(*b"lens"), &self.name) {
            return Ok(id);
        }
        let bitmap = match &self.bitmap {
            Some((n, c)) => Some(match set.converted(c.group, n) {
                Some(id) => id,
                None => c.clone().install(set, n)?,
            }),
            None => None,
        };
        let root = rebuild(&self.lens, bitmap)?;
        Ok(h1models::Converted {
            group: FourCC(*b"lens"),
            root,
            raw: Vec::new(),
        }
        .install(set, &self.name)?)
    }
}

/// The lens flare datum `flare` names in a converted tree's tag
/// reference fixup at `at` of `b`, for tests.
pub fn tag_ref_at(b: &Block, at: u32) -> Option<TagId> {
    b.fixups.iter().find_map(|f| match f.kind {
        FixupKind::TagRef(id) if f.offset == at => Some(id),
        _ => None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lens(reflections: usize) -> H1Lens {
        H1Lens {
            falloff_angle: 0.5,
            cutoff_angle: 1.5,
            occlusion_radius: 0.25,
            near_fade_distance: 3.0,
            far_fade_distance: 40.0,
            bitmap: None,
            flags: 1,
            kind_14: 2,
            scales: [1.0, 2.0],
            reflections: vec![
                H1Reflection {
                    bytes: vec![0; H1_REFLECTION]
                };
                reflections
            ],
        }
    }

    #[test]
    fn rebuilt_lens_keeps_halo_1s_angles_distances_and_reflections() {
        let d = def().unwrap();
        let bitmap = TagId(0xe000_0007);
        let b = rebuild(&lens(3), Some(bitmap)).unwrap();
        let r = defs::from_built(d, &b).unwrap();
        assert_eq!(r.at("falloff_angle"), Some(&Val::Float(0.5)));
        assert_eq!(r.at("cutoff_cosine"), Some(&Val::Float(1.5f32.cos())));
        assert_eq!(r.at("occlusion_radius"), Some(&Val::Float(0.25)));
        assert_eq!(r.at("near_fade_distance"), Some(&Val::Float(3.0)));
        assert_eq!(r.at("far_fade_distance"), Some(&Val::Float(40.0)));
        assert!(matches!(r.at("bitmap"), Some(Val::Ref { target: defs::Target::Id(x), .. }) if *x == bitmap));
        match r.at("reflections") {
            Some(Val::Block(e)) => assert_eq!(e.len(), 3),
            x => panic!("{x:?}"),
        }
        assert_eq!(r.at("kind_14"), Some(&Val::Int(2)));
        assert_eq!(r.at("scale_38"), Some(&Val::Float(2.0)));
        // No flags: Halo 2's bits 2 and 3 drop the flare (module docs).
        assert_eq!(r.at("flags"), Some(&Val::Int(0)));
        assert!(left_out(&lens(0)).iter().any(|n| n.contains("flags")));
    }

    #[test]
    fn a_reflection_keeps_halo_1s_position_radii_brightness_and_tint() {
        let mut l = lens(1);
        let b = &mut l.reflections[0].bytes;
        b[0..2].copy_from_slice(&4u16.to_le_bytes());
        b[4..6].copy_from_slice(&3u16.to_le_bytes());
        for (at, v) in [(0x1c, 0.3f32), (0x20, 20.0), (0x28, 0.1), (0x2c, 0.2), (0x34, 0.5), (0x38, 1.0), (0x40, 1.0), (0x44, 0.25), (0x48, 0.5), (0x4c, 0.75)] {
            b[at..at + 4].copy_from_slice(&v.to_le_bytes());
        }
        let r = defs::from_built(def().unwrap(), &rebuild(&l, None).unwrap()).unwrap();
        let Some(Val::Block(e)) = r.at("reflections") else { panic!() };
        let x = &e[0];
        assert_eq!(x.at("flags"), Some(&Val::Int(4)));
        assert_eq!(x.at("bitmap_index"), Some(&Val::Int(3)));
        assert_eq!(x.at("position"), Some(&Val::Float(0.3)));
        assert_eq!(x.at("rotation_offset"), Some(&Val::Float(20.0)));
        assert_eq!(x.at("radius_bounds"), Some(&Val::floats(&[0.1, 0.2])));
        assert_eq!(x.at("brightness_bounds"), Some(&Val::floats(&[0.5, 1.0])));
        assert_eq!(x.at("tint_colour"), Some(&Val::floats(&[0.25, 0.5, 0.75])));
    }

    #[test]
    fn a_lens_with_no_bitmap_names_none() {
        let b = rebuild(&lens(0), None).unwrap();
        let r = defs::from_built(def().unwrap(), &b).unwrap();
        assert!(matches!(r.at("bitmap"), Some(Val::Ref { target: defs::Target::Null, .. })));
    }
}
