//! `deca` (decal): one fixed 0xac-byte record, no child blocks.
//!
//! Evidence: size and the absence of blocks are **V** (all 1,570 distinct
//! records on the disc are 0xac bytes with zero in every span left unnamed).
//! Field names are **S** hints confirmed only by value ranges, except where
//! marked **E** (read by the engine's decal code, `0x0017ef10`/`0x0017e670`).
use super::EffectTag;
use super::mem::{Ctx, Rec, TagRef};
use crate::Result;
use map::FourCC;

/// Root record size.
pub const DECAL_SIZE: usize = 0xac;

/// A decal tag. Unnamed bytes stay in [`Decal::raw`] and round-trip.
#[derive(Clone, Debug, PartialEq)]
pub struct Decal {
    /// +0, u16 flag bits (disc: 0, 1, 8, 0x100, 0x108, 0x148, 0x168, ...).
    pub flags: u16,
    /// +2, small enum 0..=3 (**V** range only; labels are **S**).
    pub kind: u16,
    /// +4, layer 1..=5 (**V** range only).
    pub layer: u16,
    /// +6, u16. **E**: the placement code skips the decal when this is zero;
    /// the disc holds 8 for 1,419 of 1,570 (a maximum overlap count).
    pub max_overlapping: u16,
    /// +8, a `deca` reference chaining to a following decal (**V**: group
    /// resolves to `deca` whenever non-null; null is class `0xffffffff`).
    pub next_decal: TagRef,
    /// +0x10 / +0x14, a size pair. **E**: `+0x14` scales the placement test.
    pub radius: [f32; 2],
    /// +0x18, f32 (0.75 on 1,543 of 1,570, else 0). **E**: read as a scale.
    pub unknown_18: f32,
    /// +0x1c..+0x34: two RGB triples (**S**), kept as floats.
    pub colors: [[f32; 3]; 2],
    /// +0x34..+0x44: four f32 (two pairs; mostly 180/180/20/20 on disc).
    pub unknown_34: [f32; 4],
    /// +0x88, the decal bitmap (**V**: resolves to `bitm` on every instance
    /// where a bitmap is present).
    pub bitmap: TagRef,
    /// Every byte of the record, including all unnamed spans.
    pub raw: Vec<u8>,
}

impl EffectTag for Decal {
    const GROUP: [u8; 4] = *b"deca";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let raw = ctx.mem.read(address, DECAL_SIZE)?;
        let r = Rec::exact(&raw, DECAL_SIZE)?;
        Ok(Self {
            flags: r.u16(0),
            kind: r.u16(2),
            layer: r.u16(4),
            max_overlapping: r.u16(6),
            next_decal: r.tag_ref(8),
            radius: [r.f32(0x10), r.f32(0x14)],
            unknown_18: r.f32(0x18),
            colors: [
                [r.f32(0x1c), r.f32(0x20), r.f32(0x24)],
                [r.f32(0x28), r.f32(0x2c), r.f32(0x30)],
            ],
            unknown_34: [r.f32(0x34), r.f32(0x38), r.f32(0x3c), r.f32(0x40)],
            bitmap: r.tag_ref(0x88),
            raw,
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        vec![
            ("next_decal".into(), self.next_decal),
            ("bitmap".into(), self.bitmap),
        ]
    }
}

/// Expected class of each reference, for validators.
pub const REFERENCE_CLASSES: [(&str, FourCC); 2] = [
    ("next_decal", FourCC(*b"deca")),
    ("bitmap", FourCC(*b"bitm")),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;
    use map::TagId;

    fn record() -> Vec<u8> {
        let mut b = vec![0u8; DECAL_SIZE];
        b[0..2].copy_from_slice(&0x108u16.to_le_bytes());
        b[2..4].copy_from_slice(&3u16.to_le_bytes());
        b[4..6].copy_from_slice(&5u16.to_le_bytes());
        b[6..8].copy_from_slice(&8u16.to_le_bytes());
        b[8..12].copy_from_slice(&u32::from_be_bytes(*b"deca").to_le_bytes());
        b[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
        b[0x14..0x18].copy_from_slice(&0.5f32.to_le_bytes());
        b[0x88..0x8c].copy_from_slice(&u32::from_be_bytes(*b"bitm").to_le_bytes());
        b[0x8c..0x90].copy_from_slice(&0xe1230045u32.to_le_bytes());
        b
    }

    #[test]
    fn decodes_and_lists_references() {
        let mut mem = SliceMemory::new(0x8000_0000);
        let at = mem.push(&record());
        let d = Decal::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(
            (d.flags, d.kind, d.layer, d.max_overlapping),
            (0x108, 3, 5, 8)
        );
        assert!(d.next_decal.is_null());
        assert_eq!(d.bitmap.id, TagId(0xe1230045));
        assert_eq!(d.radius[1], 0.5);
        let refs = d.references();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[1].0, "bitmap");
        assert_eq!(d.raw, record());
    }

    #[test]
    fn truncated_record_is_an_error() {
        let mut mem = SliceMemory::new(0x8000_0000);
        let at = mem.push(&record()[..0x90]);
        assert!(Decal::decode_at(&mut Ctx::new(&mut mem), at).is_err());
    }
}
