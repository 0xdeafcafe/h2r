//! `ligh` (light): a 0xe4-byte root with four tag references and three
//! blocks of function references.
//!
//! Evidence (**V**, all 1,590 distinct disc records): root size 0xe4 (the
//! next-lowest block pointer is always root + 0xe4 where blocks are inline);
//! the blocks at `+0xc4`, `+0xcc`, `+0xd4` have stride 8, each element a data
//! reference whose bytes begin with a function header whose size matches its
//! type byte; references resolve to `bitm`, `lens`, `MGS2`, `shad`. Scalar
//! meanings are **not** established: they stay in [`Light::raw`].
use super::EffectTag;
use super::mem::{Ctx, Rec, TagRef};
use crate::Result;
use crate::wire::TagBlock;
use map::FourCC;

pub const LIGHT_SIZE: usize = 0xe4;

/// A light tag.
#[derive(Clone, Debug, PartialEq)]
pub struct Light {
    /// +0, u32 flag bits (disc: 0xa, 0x80a, 0x900a, 0x100a, 0x40008e, ...).
    pub flags: u32,
    /// +0x78, `bitm` (projected/gel texture, **S**).
    pub bitmap: TagRef,
    /// +0x90, `lens` (a lens-flare tag).
    pub lens_flare: TagRef,
    /// +0x9c, `MGS2` (referenced by the engine's part-creation switch at
    /// `0x0017a8db` as a creatable part type; its name is not established).
    pub mgs2: TagRef,
    /// +0xdc, `shad`.
    pub shader: TagRef,
    /// Function data of the block at +0xc4 (one `Vec` per element).
    pub functions_c4: Vec<Vec<u8>>,
    /// Function data of the block at +0xcc.
    pub functions_cc: Vec<Vec<u8>>,
    /// Function data of the block at +0xd4.
    pub functions_d4: Vec<Vec<u8>>,
    /// The whole root record.
    pub raw: Vec<u8>,
}

/// Read a block whose elements are 8-byte function data references.
/// Shared by `ligh`, `lens`.
pub fn function_refs(ctx: &mut Ctx<'_>, block: TagBlock) -> Result<Vec<Vec<u8>>> {
    let mut out = Vec::new();
    for e in ctx.block(block, 8)? {
        let r = Rec::exact(&e, 8)?;
        out.push(ctx.data(r.data(0))?);
    }
    Ok(out)
}

impl EffectTag for Light {
    const GROUP: [u8; 4] = *b"ligh";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let raw = ctx.mem.read(address, LIGHT_SIZE)?;
        let r = Rec::exact(&raw, LIGHT_SIZE)?;
        Ok(Self {
            flags: r.u32(0),
            bitmap: r.tag_ref(0x78),
            lens_flare: r.tag_ref(0x90),
            mgs2: r.tag_ref(0x9c),
            shader: r.tag_ref(0xdc),
            functions_c4: function_refs(ctx, r.block(0xc4))?,
            functions_cc: function_refs(ctx, r.block(0xcc))?,
            functions_d4: function_refs(ctx, r.block(0xd4))?,
            raw,
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        vec![
            ("bitmap".into(), self.bitmap),
            ("lens_flare".into(), self.lens_flare),
            ("mgs2".into(), self.mgs2),
            ("shader".into(), self.shader),
        ]
    }

    fn functions(&self) -> Vec<(String, &[u8])> {
        let mut v = Vec::new();
        for (name, blk) in [
            ("c4", &self.functions_c4),
            ("cc", &self.functions_cc),
            ("d4", &self.functions_d4),
        ] {
            for (i, f) in blk.iter().enumerate() {
                v.push((format!("functions_{name}[{i}]"), f.as_slice()));
            }
        }
        v
    }
}

pub const REFERENCE_CLASSES: [(&str, FourCC); 4] = [
    ("bitmap", FourCC(*b"bitm")),
    ("lens_flare", FourCC(*b"lens")),
    ("mgs2", FourCC(*b"MGS2")),
    ("shader", FourCC(*b"shad")),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;

    const BASE: u32 = 0x8010_0000;

    fn put(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    fn root() -> Vec<u8> {
        let mut b = vec![0u8; LIGHT_SIZE];
        put(&mut b, 0x78, u32::from_be_bytes(*b"bitm"));
        put(&mut b, 0x7c, u32::MAX);
        put(&mut b, 0x90, u32::from_be_bytes(*b"lens"));
        put(&mut b, 0x94, 0xe1000001);
        put(&mut b, 0x9c, u32::from_be_bytes(*b"MGS2"));
        put(&mut b, 0xa0, u32::MAX);
        put(&mut b, 0xdc, u32::from_be_bytes(*b"shad"));
        put(&mut b, 0xe0, u32::MAX);
        b
    }

    #[test]
    fn empty_blocks_and_references() {
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&root());
        let l = Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert!(l.functions_c4.is_empty() && l.functions_cc.is_empty());
        let refs = l.references();
        assert_eq!(refs.len(), 4);
        assert!(refs[0].1.is_null() && !refs[1].1.is_null());
    }

    #[test]
    fn aliased_function_block_outside_the_tag() {
        // The function block and its bytes live before the tag: dedup.
        let mut mem = SliceMemory::new(BASE);
        let func = mem.push(&[1, 0, 0, 0, 9, 9, 9, 9]);
        let mut elem = Vec::new();
        elem.extend(8u32.to_le_bytes());
        elem.extend(func.to_le_bytes());
        let blk = mem.push(&elem);
        let mut r = root();
        put(&mut r, 0xcc, 1);
        put(&mut r, 0xd0, blk);
        let at = mem.push(&r);
        let l = Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(l.functions_cc, vec![vec![1, 0, 0, 0, 9, 9, 9, 9]]);
    }

    #[test]
    fn out_of_bounds_block_and_budget_fail() {
        let mut mem = SliceMemory::new(BASE);
        let mut r = root();
        put(&mut r, 0xc4, 1);
        put(&mut r, 0xc8, 0x9000_0000);
        let at = mem.push(&r);
        assert!(Light::decode_at(&mut Ctx::new(&mut mem), at).is_err());
        let mut r = root();
        put(&mut r, 0xc4, 0x00ff_ffff);
        put(&mut r, 0xc8, BASE);
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&r);
        assert!(Light::decode_at(&mut Ctx::new(&mut mem), at).is_err());
    }

    #[test]
    fn unknown_flags_preserved() {
        let mut r = root();
        put(&mut r, 0, 0xdead_beef);
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&r);
        let l = Light::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(l.flags, 0xdead_beef);
        assert_eq!(l.raw, r);
    }
}
