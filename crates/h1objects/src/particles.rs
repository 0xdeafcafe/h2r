//! Halo 1 particles and contrails as Halo 2 `prt3` and `cont`, written
//! from the definitions (`crates/tags/src/defs/prt3.txt`, `cont.txt`).
//!
//! **Contrails.** Halo 2's `cont` is Halo 1's with its two tag references
//! shortened (16 bytes to 8) and the padding between the root's halves
//! dropped: V, word for word on the contrails both games ship (the sniper
//! rifle's, the needler's, the plasma rifle's; `tests/particles_disc.rs`
//! `contrail_survey`). So a converted contrail is Halo 1's spans at Halo
//! 2's offsets ([`CONT_SPANS`], [`CONT_POINT_SPANS`]), its bitmap converted
//! (`h1bitmaps`). The points' physics (`pphy`) has no converter here: it is
//! null, as on Halo 2's plasma streaks (V).

use h1map::{H1Map, Tag, TagId as H1TagId};
use map::build::Block;
use map::{FourCC, TagId};
use tags::defs::{self, Defs, Record, StructDef, Val};

use crate::h1::{reference, tag_bytes};
use crate::{Error, Result};

/// Halo 1 `cont` root size (V: every one is `0x144` plus `0x68` per point,
/// its point block at `+0x138`).
pub const H1_CONT_SIZE: usize = 0x144;
/// Halo 1 contrail point size.
pub const H1_CONT_POINT_SIZE: usize = 0x68;
/// Halo 1 `cont +0x138`: the points.
pub const H1_CONT_POINTS: usize = 0x138;

/// (Halo 2 field, Halo 1 offset, length) of a contrail root (`cont.txt`).
pub const CONT_SPANS: &[(&str, usize, usize)] = &[
    ("unknown_4", 0x4, 0x2c),
    ("unknown_38", 0x40, 0x4),
    ("unknown_60", 0xa8, 0x28),
    ("unknown_90", 0xe0, 0x58),
];
/// The same for a point.
pub const CONT_POINT_SPANS: &[(&str, usize, usize)] = &[("unknown_0", 0x0, 0x10), ("unknown_18", 0x40, 0x28)];

/// A Halo 1 contrail: its root and points' bytes and its bitmap.
#[derive(Clone, Debug, PartialEq)]
pub struct H1Contrail {
    pub flags: u32,
    pub root: Vec<u8>,
    pub bitmap: Option<H1TagId>,
    pub points: Vec<Vec<u8>>,
}

impl H1Contrail {
    pub fn read(map: &H1Map, tag: &Tag) -> Result<Self> {
        let b = tag_bytes(map, tag, b"cont", H1_CONT_SIZE)?;
        let blk = h1map::Block::read(b, H1_CONT_POINTS).ok_or(Error::Truncated("cont points"))?;
        let points = map
            .block(blk, H1_CONT_POINT_SIZE)?
            .chunks_exact(H1_CONT_POINT_SIZE)
            .map(<[u8]>::to_vec)
            .collect();
        Ok(Self {
            flags: u32::from_le_bytes(b[0..4].try_into().expect("4")),
            root: b[..H1_CONT_SIZE].to_vec(),
            bitmap: reference(map, b, 0x30),
            points,
        })
    }
}

fn def(group: &[u8; 4]) -> Result<&'static StructDef> {
    Defs::builtin()
        .group(FourCC(*group))
        .ok_or_else(|| Error::Invalid(format!("no definition for {}", FourCC(*group))))
}

fn set(r: &mut Record, path: &str, v: Val) -> Result<()> {
    r.set(path, v).map_err(|e| Error::Invalid(e.to_string()))
}

fn spans(r: &mut Record, from: &[u8], list: &[(&str, usize, usize)]) -> Result<()> {
    for (field, at, len) in list {
        let bytes = from
            .get(*at..at + len)
            .ok_or(Error::Truncated("contrail span"))?;
        set(r, field, Val::Bytes(bytes.to_vec()))?;
    }
    Ok(())
}

/// The `cont` root for `c`, its bitmap `bitmap` (converted, or null).
pub fn contrail(c: &H1Contrail, bitmap: Option<TagId>) -> Result<Block> {
    let d = def(b"cont")?;
    let mut r = defs::default_record(d);
    set(&mut r, "flags", Val::Int(c.flags.into()))?;
    spans(&mut r, &c.root, CONT_SPANS)?;
    set(&mut r, "bitmap", Val::tag(FourCC(*b"bitm"), bitmap.unwrap_or(TagId::NULL)))?;
    let pd = d.child("points").expect("cont points");
    let points = c
        .points
        .iter()
        .map(|p| {
            let mut x = defs::default_record(pd);
            spans(&mut x, p, CONT_POINT_SPANS)?;
            Ok(x)
        })
        .collect::<Result<Vec<_>>>()?;
    set(&mut r, "points", Val::Block(points))?;
    defs::to_built(d, &r, &mut |_| None).map_err(|e| Error::Invalid(e.to_string()))
}
