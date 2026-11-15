//! `jpt!` (damage effect): a 0xc8-byte root and a block of 0x4c-byte
//! responses whose scalars drive controller feedback.
//!
//! Evidence (**V**, all 2,612 records; 161 distinct): root size 0xc8 (inline
//! children start there); the response block descriptor is at `+0x6c`
//! (count 0, 1 or 2); stride 0x4c is also the difference between tag sizes with and
//! without a response; each response holds three function data references at `+0x28`,
//! `+0x34` and `+0x44` whose bytes start with a plausible function type; the
//! reference at `+0xa8` resolves to `snd!` whenever non-null; function bytes
//! seen: types 0 (20 B), 2 (36), 3 (52), 7 (116), 9 (44). The engine applies
//! damage effects from the part-creation switch (`0x0017a97f`, case `jpt!`,
//! calling `0x000d6660`/`0x000d6c80`) and feeds controller feedback in
//! `0x00222a70`. Scalar meanings are unreviewed.
use super::EffectTag;
use super::mem::{Ctx, Rec, TagRef};
use crate::Result;
use map::FourCC;

pub const DAMAGE_SIZE: usize = 0xc8;
pub const RESPONSE_SIZE: usize = 0x4c;

#[derive(Clone, Debug, PartialEq)]
pub struct DamageResponse {
    /// +0, u16 (disc: 0, 1, 2).
    pub kind: u16,
    /// +2, u16 (disc: 0, 1, 2, 5).
    pub unknown_02: u16,
    /// +0x24 and +0x30: f32 values, each paired with a function below.
    pub scalars: [f32; 2],
    /// Function bytes referenced at +0x28 and +0x34 (sizes 0x24..0x74).
    pub functions: [Vec<u8>; 2],
    /// Function bytes referenced at +0x44 (0x14 bytes on all observed data
    /// except tags with a 0x2c-byte function).
    pub function_44: Vec<u8>,
    /// The whole 0x4c-byte element.
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DamageEffect {
    /// +0xa8, `snd!`. Null on all but 14 of 2,722 disc records (the group
    /// field alone reads `snd!` on more).
    pub sound: TagRef,
    pub responses: Vec<DamageResponse>,
    /// The whole root record.
    pub raw: Vec<u8>,
}

impl EffectTag for DamageEffect {
    const GROUP: [u8; 4] = *b"jpt!";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let raw = ctx.mem.read(address, DAMAGE_SIZE)?;
        let r = Rec::exact(&raw, DAMAGE_SIZE)?;
        let mut responses = Vec::new();
        for e in ctx.block(r.block(0x6c), RESPONSE_SIZE)? {
            let x = Rec::exact(&e, RESPONSE_SIZE)?;
            responses.push(DamageResponse {
                kind: x.u16(0),
                unknown_02: x.u16(2),
                scalars: [x.f32(0x24), x.f32(0x30)],
                functions: [ctx.data(x.data(0x28))?, ctx.data(x.data(0x34))?],
                function_44: ctx.data(x.data(0x44))?,
                raw: e.clone(),
            });
        }
        Ok(Self {
            sound: r.tag_ref(0xa8),
            responses,
            raw,
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        vec![("sound".into(), self.sound)]
    }

    fn functions(&self) -> Vec<(String, &[u8])> {
        let mut v = Vec::new();
        for (i, r) in self.responses.iter().enumerate() {
            v.push((
                format!("responses[{i}].functions[0]"),
                r.functions[0].as_slice(),
            ));
            v.push((
                format!("responses[{i}].functions[1]"),
                r.functions[1].as_slice(),
            ));
            v.push((
                format!("responses[{i}].function_44"),
                r.function_44.as_slice(),
            ));
        }
        v
    }
}

pub const REFERENCE_CLASSES: [(&str, FourCC); 1] = [("sound", FourCC(*b"snd!"))];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;
    const BASE: u32 = 0x8040_0000;
    fn put(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    #[test]
    fn response_with_aliased_functions() {
        let mut mem = SliceMemory::new(BASE);
        let f = mem.push(&[2, 0, 0, 0, 1, 2, 3, 4]);
        let mut resp = vec![0u8; RESPONSE_SIZE];
        resp[0] = 2;
        resp[3] = 1;
        put(&mut resp, 0x24, 0.25f32.to_bits());
        for o in [0x28, 0x34, 0x44] {
            put(&mut resp, o, 8);
            put(&mut resp, o + 4, f);
        }
        let blk = mem.push(&resp);
        let mut root = vec![0u8; DAMAGE_SIZE];
        put(&mut root, 0x6c, 1);
        put(&mut root, 0x70, blk);
        put(&mut root, 0xa8, u32::from_be_bytes(*b"snd!"));
        put(&mut root, 0xac, 0xe3000009);
        let at = mem.push(&root);
        let d = DamageEffect::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(d.responses.len(), 1);
        let r = &d.responses[0];
        assert_eq!((r.kind, r.unknown_02, r.scalars[0]), (2, 0x100, 0.25));
        assert_eq!(r.functions[1], vec![2, 0, 0, 0, 1, 2, 3, 4]);
        assert_eq!(r.function_44.len(), 8);
        assert_eq!(d.references()[0].1.id.0, 0xe3000009);
    }
    #[test]
    fn empty_oob_and_budget() {
        let mut mem = SliceMemory::new(BASE);
        let mut root = vec![0u8; DAMAGE_SIZE];
        let at = mem.push(&root);
        assert!(
            DamageEffect::decode_at(&mut Ctx::new(&mut mem), at)
                .unwrap()
                .responses
                .is_empty()
        );
        put(&mut root, 0x6c, 2);
        put(&mut root, 0x70, 0x9000_0000);
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&root);
        assert!(DamageEffect::decode_at(&mut Ctx::new(&mut mem), at).is_err());
        put(&mut root, 0x6c, u32::MAX);
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&root);
        assert!(DamageEffect::decode_at(&mut Ctx::new(&mut mem), at).is_err());
    }
}
