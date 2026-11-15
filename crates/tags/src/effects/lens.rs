//! `lens` (lens flare): a 0x64-byte root, a block of 0x30-byte reflections,
//! and three blocks of function references.
//!
//! Evidence (**V**, all 1,285 distinct disc records): root size 0x64 (inline
//! children begin at +0x64); `+8`/`+0xc` equal `cos(+0)`/`cos(+4)` on every
//! record (2,570 of 2,570); the reflection block at `+0x40` has stride 0x30
//! (tag sizes tile as 0x64 + n*0x30 + function data) and the three blocks at
//! `+0x4c/+0x54/+0x5c` have stride 8 with function-sized data references; the
//! reference at `+0x20` resolves to `bitm`. Remaining meanings are unreviewed.
use super::EffectTag;
use super::light::function_refs;
use super::mem::{Ctx, Rec, TagRef};
use crate::Result;
use map::FourCC;

pub const LENS_SIZE: usize = 0x64;
pub const REFLECTION_SIZE: usize = 0x30;

/// One reflection (0x30 bytes). Only the leading words are interpreted.
#[derive(Clone, Debug, PartialEq)]
pub struct Reflection {
    /// +0, u16 flag bits (disc: 0, 2, 4, 0xc, 0x10, 0x14, ...).
    pub flags: u16,
    /// +2, u16.
    pub unknown_02: u16,
    /// +4, u32 (disc: 0..=6; a bitmap-sequence/frame index is likely, **S**).
    pub index: u32,
    /// The full element.
    pub raw: Vec<u8>,
}

/// A lens-flare tag.
#[derive(Clone, Debug, PartialEq)]
pub struct LensFlare {
    /// +0 / +4: two angles in radians (**V**: their cosines are at +8/+0xc).
    pub angles: [f32; 2],
    /// +8 / +0xc: `cos(angles)`.
    pub cosines: [f32; 2],
    /// +0x20, `bitm`.
    pub bitmap: TagRef,
    pub reflections: Vec<Reflection>,
    /// Function data for the blocks at +0x4c, +0x54, +0x5c.
    pub functions_4c: Vec<Vec<u8>>,
    pub functions_54: Vec<Vec<u8>>,
    pub functions_5c: Vec<Vec<u8>>,
    /// The whole root record.
    pub raw: Vec<u8>,
}

impl EffectTag for LensFlare {
    const GROUP: [u8; 4] = *b"lens";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let raw = ctx.mem.read(address, LENS_SIZE)?;
        let r = Rec::exact(&raw, LENS_SIZE)?;
        let mut reflections = Vec::new();
        for e in ctx.block(r.block(0x40), REFLECTION_SIZE)? {
            let x = Rec::exact(&e, REFLECTION_SIZE)?;
            reflections.push(Reflection {
                flags: x.u16(0),
                unknown_02: x.u16(2),
                index: x.u32(4),
                raw: e.clone(),
            });
        }
        Ok(Self {
            angles: [r.f32(0), r.f32(4)],
            cosines: [r.f32(8), r.f32(0xc)],
            bitmap: r.tag_ref(0x20),
            reflections,
            functions_4c: function_refs(ctx, r.block(0x4c))?,
            functions_54: function_refs(ctx, r.block(0x54))?,
            functions_5c: function_refs(ctx, r.block(0x5c))?,
            raw,
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        vec![("bitmap".into(), self.bitmap)]
    }

    fn functions(&self) -> Vec<(String, &[u8])> {
        let mut v = Vec::new();
        for (name, blk) in [
            ("4c", &self.functions_4c),
            ("54", &self.functions_54),
            ("5c", &self.functions_5c),
        ] {
            for (i, f) in blk.iter().enumerate() {
                v.push((format!("functions_{name}[{i}]"), f.as_slice()));
            }
        }
        v
    }
}

pub const REFERENCE_CLASSES: [(&str, FourCC); 1] = [("bitmap", FourCC(*b"bitm"))];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;

    const BASE: u32 = 0x8020_0000;
    fn put(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }

    #[test]
    fn reflections_and_inline_function() {
        let mut mem = SliceMemory::new(BASE);
        let mut root = vec![0u8; LENS_SIZE];
        put(&mut root, 0, 1.0f32.to_bits());
        put(&mut root, 8, 1.0f32.cos().to_bits());
        put(&mut root, 0x20, u32::from_be_bytes(*b"bitm"));
        put(&mut root, 0x24, 0xe1000007);
        let root_at = BASE + LENS_SIZE as u32; // children follow the root
        // reflection block at +0x40, then function-ref block at +0x4c
        let refl_at = root_at;
        let fref_at = refl_at + 2 * REFLECTION_SIZE as u32;
        let data_at = fref_at + 8;
        put(&mut root, 0x40, 2);
        put(&mut root, 0x44, refl_at);
        put(&mut root, 0x4c, 1);
        put(&mut root, 0x50, fref_at);
        let at = mem.push(&root);
        let mut refl = vec![0u8; 2 * REFLECTION_SIZE];
        refl[0] = 4;
        refl[4] = 3;
        refl[REFLECTION_SIZE + 2] = 7;
        mem.push(&refl);
        let mut fr = Vec::new();
        fr.extend(4u32.to_le_bytes());
        fr.extend(data_at.to_le_bytes());
        mem.push(&fr);
        mem.push(&[0, 1, 2, 3]);
        let l = LensFlare::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(l.reflections.len(), 2);
        assert_eq!((l.reflections[0].flags, l.reflections[0].index), (4, 3));
        assert_eq!(l.reflections[1].unknown_02, 7);
        assert_eq!(l.functions_4c, vec![vec![0, 1, 2, 3]]);
        assert!((l.angles[0].cos() - l.cosines[0]).abs() < 1e-6);
        assert_eq!(l.references().len(), 1);
    }

    #[test]
    fn reflection_block_out_of_bounds() {
        let mut mem = SliceMemory::new(BASE);
        let mut root = vec![0u8; LENS_SIZE];
        put(&mut root, 0x40, 3);
        put(&mut root, 0x44, BASE + 0x40); // only 0x24 bytes remain
        let at = mem.push(&root);
        assert!(LensFlare::decode_at(&mut Ctx::new(&mut mem), at).is_err());
    }

    #[test]
    fn empty_blocks_ok() {
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&[0u8; LENS_SIZE]);
        let l = LensFlare::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert!(l.reflections.is_empty() && l.functions_54.is_empty());
    }
}
