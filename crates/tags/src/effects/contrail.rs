//! `cont` (contrail): a 0xf0-byte root and a block of 0x40-byte points.
//!
//! Evidence (**V**, all 562 records, 34 distinct tags): root size 0xf0 with
//! the point block descriptor at `+0xe8` (tag sizes are 0xf0 + n*0x40 for
//! n = 2, 3, 4); points sit contiguously after the root; each point's
//! reference at `+0x10` has class `pphy` (point physics) and resolves; the
//! root's reference at `+0x30` is `bitm` (`+0x88` is a null `bitm` slot). The engine keeps
//! contrails in the data arrays "contrail" and "contrail point" (init
//! `0x0017b7cb`). Scalar meanings stay in the raw bytes.
use super::EffectTag;
use super::mem::{Ctx, Rec, TagRef};
use crate::Result;
use map::FourCC;

pub const CONTRAIL_SIZE: usize = 0xf0;
pub const POINT_SIZE: usize = 0x40;

#[derive(Clone, Debug, PartialEq)]
pub struct ContrailPoint {
    /// +0x10, `pphy` reference (point physics).
    pub physics: TagRef,
    /// The whole 0x40-byte element.
    pub raw: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Contrail {
    /// +0, u32 flag bits (disc: 0, 1, 3, 0x40).
    pub flags: u32,
    /// +0x30, `bitm` (base bitmap).
    pub bitmap: TagRef,
    /// +0x88, second bitmap slot (class field reads `bitm`; the datum is null
    /// on every disc record).
    pub bitmap2: TagRef,
    pub points: Vec<ContrailPoint>,
    /// The whole root record.
    pub raw: Vec<u8>,
}

impl EffectTag for Contrail {
    const GROUP: [u8; 4] = *b"cont";
    fn decode_at(ctx: &mut Ctx<'_>, address: u32) -> Result<Self> {
        let raw = ctx.mem.read(address, CONTRAIL_SIZE)?;
        let r = Rec::exact(&raw, CONTRAIL_SIZE)?;
        let mut points = Vec::new();
        for e in ctx.block(r.block(0xe8), POINT_SIZE)? {
            let p = Rec::exact(&e, POINT_SIZE)?;
            points.push(ContrailPoint {
                physics: p.tag_ref(0x10),
                raw: e.clone(),
            });
        }
        Ok(Self {
            flags: r.u32(0),
            bitmap: r.tag_ref(0x30),
            bitmap2: r.tag_ref(0x88),
            points,
            raw,
        })
    }
    fn references(&self) -> Vec<(String, TagRef)> {
        let mut v = vec![
            ("bitmap".to_string(), self.bitmap),
            ("bitmap2".to_string(), self.bitmap2),
        ];
        v.extend(
            self.points
                .iter()
                .enumerate()
                .map(|(i, p)| (format!("points[{i}].physics"), p.physics)),
        );
        v
    }
}

pub const REFERENCE_CLASSES: [(&str, FourCC); 3] = [
    ("bitmap", FourCC(*b"bitm")),
    ("bitmap2", FourCC(*b"bitm")),
    ("points[].physics", FourCC(*b"pphy")),
];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::effects::SliceMemory;
    const BASE: u32 = 0x8030_0000;
    fn put(b: &mut [u8], o: usize, v: u32) {
        b[o..o + 4].copy_from_slice(&v.to_le_bytes());
    }
    #[test]
    fn points_and_references() {
        let mut mem = SliceMemory::new(BASE);
        let mut root = vec![0u8; CONTRAIL_SIZE];
        put(&mut root, 0x30, u32::from_be_bytes(*b"bitm"));
        put(&mut root, 0x34, 0xe1000002);
        put(&mut root, 0x88, u32::from_be_bytes(*b"bitm"));
        put(&mut root, 0x8c, u32::MAX);
        put(&mut root, 0xe8, 2);
        put(&mut root, 0xec, BASE + CONTRAIL_SIZE as u32);
        let at = mem.push(&root);
        let mut pts = vec![0u8; 2 * POINT_SIZE];
        for i in 0..2 {
            let o = i * POINT_SIZE + 0x10;
            put(&mut pts, o, u32::from_be_bytes(*b"pphy"));
            put(&mut pts, o + 4, 0xe2000000 + i as u32);
        }
        mem.push(&pts);
        let c = Contrail::decode_at(&mut Ctx::new(&mut mem), at).unwrap();
        assert_eq!(c.points.len(), 2);
        let refs = c.references();
        assert_eq!(refs.len(), 4);
        assert_eq!(refs[3].0, "points[1].physics");
        assert!(refs[1].1.is_null());
    }
    #[test]
    fn truncated_points_fail_and_empty_ok() {
        let mut mem = SliceMemory::new(BASE);
        let mut root = vec![0u8; CONTRAIL_SIZE];
        let at = mem.push(&root);
        assert!(
            Contrail::decode_at(&mut Ctx::new(&mut mem), at)
                .unwrap()
                .points
                .is_empty()
        );
        put(&mut root, 0xe8, 2);
        put(&mut root, 0xec, BASE + CONTRAIL_SIZE as u32);
        let mut mem = SliceMemory::new(BASE);
        let at = mem.push(&root);
        mem.push(&[0u8; POINT_SIZE + 8]);
        assert!(Contrail::decode_at(&mut Ctx::new(&mut mem), at).is_err());
    }
}
