//! Virtual-address reads and bounded block decoding shared by the effects family.
//!
//! Retail caches de-duplicate identical child blocks: a tag's block pointer can
//! address bytes that live in *another* tag's payload (observed on the disc:
//! effects whose payload is only the 0x30-byte root while every child block
//! points elsewhere in the map's ordinary metadata). Decoders therefore read
//! through the whole metadata space ([`Memory`]) instead of the tag's own
//! payload, and charge every block they follow to a [`Budget`] so a corrupt or
//! adversarial map cannot make one tag expand without bound.
use crate::wire::{DataReference, TagBlock};
use crate::{Error, Result};
use map::{FourCC, Map, TagId};
use std::io::{Read, Seek};

/// Read access to the map's ordinary metadata by virtual address.
pub trait Memory {
    /// Read exactly `len` bytes at virtual `address`, or fail with `Bounds`.
    fn read(&mut self, address: u32, len: usize) -> Result<Vec<u8>>;
}

impl<R: Read + Seek> Memory for Map<R> {
    fn read(&mut self, address: u32, len: usize) -> Result<Vec<u8>> {
        let bounds = Error::Bounds {
            address,
            size: len as u64,
        };
        let size = u32::try_from(len).map_err(|_| Error::Bounds {
            address,
            size: len as u64,
        })?;
        let offset = self.file_offset(address, size).map_err(|_| bounds)?;
        Ok(self.read_file_range(offset, u64::from(size))?)
    }
}

/// In-memory metadata, for synthetic tests.
#[derive(Clone, Debug, Default)]
pub struct SliceMemory {
    pub base: u32,
    pub bytes: Vec<u8>,
}
impl SliceMemory {
    pub fn new(base: u32) -> Self {
        Self {
            base,
            bytes: Vec::new(),
        }
    }
    /// Append `data`, returning its virtual address.
    pub fn push(&mut self, data: &[u8]) -> u32 {
        let address = self.base + self.bytes.len() as u32;
        self.bytes.extend_from_slice(data);
        address
    }
}
impl Memory for SliceMemory {
    fn read(&mut self, address: u32, len: usize) -> Result<Vec<u8>> {
        let bounds = Error::Bounds {
            address,
            size: len as u64,
        };
        let start = address.checked_sub(self.base).ok_or(Error::Bounds {
            address,
            size: len as u64,
        })? as usize;
        let end = start.checked_add(len).ok_or(Error::Bounds {
            address,
            size: len as u64,
        })?;
        self.bytes.get(start..end).map(<[u8]>::to_vec).ok_or(bounds)
    }
}

/// Work limits for one tag. Aliased blocks make the same bytes reachable many
/// times, so limits count every followed block, not distinct bytes.
#[derive(Clone, Copy, Debug)]
pub struct Budget {
    /// Remaining total bytes that may be read through blocks.
    pub bytes: u64,
    /// Remaining total elements that may be materialised.
    pub elements: u64,
}
impl Budget {
    /// Generous for real data (the largest retail effect is well under 64 KiB
    /// and a few hundred elements) yet small enough to bound hostile input.
    pub const DEFAULT: Self = Self {
        bytes: 4 << 20,
        elements: 1 << 16,
    };
    fn charge(&mut self, bytes: u64, elements: u64) -> Result<()> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or(Error::Invalid("effect block budget exceeded"))?;
        self.elements = self
            .elements
            .checked_sub(elements)
            .ok_or(Error::Invalid("effect element budget exceeded"))?;
        Ok(())
    }
}
impl Default for Budget {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// Everything a decoder needs besides its own bytes.
pub struct Ctx<'a> {
    pub mem: &'a mut dyn Memory,
    pub budget: Budget,
}
impl<'a> Ctx<'a> {
    pub fn new(mem: &'a mut dyn Memory) -> Self {
        Self {
            mem,
            budget: Budget::DEFAULT,
        }
    }
    /// Read the elements of a `{count, address}` block as `count` fixed-size
    /// byte records. Count zero is an empty block whatever its address says.
    pub fn block(&mut self, b: TagBlock, stride: usize) -> Result<Vec<Vec<u8>>> {
        if b.count == 0 {
            return Ok(Vec::new());
        }
        let total = u64::from(b.count)
            .checked_mul(stride as u64)
            .ok_or(Error::Bounds {
                address: b.address,
                size: u64::MAX,
            })?;
        self.budget.charge(total, u64::from(b.count))?;
        let bytes = self.mem.read(b.address, total as usize)?;
        Ok(bytes.chunks_exact(stride).map(<[u8]>::to_vec).collect())
    }
    /// Read a data reference's bytes (count is a byte length).
    pub fn data(&mut self, d: DataReference) -> Result<Vec<u8>> {
        if d.size == 0 {
            return Ok(Vec::new());
        }
        self.budget.charge(u64::from(d.size), 1)?;
        self.mem.read(d.address, d.size as usize)
    }
}

/// Bounds-safe little-endian field access into a fixed-size record.
/// Callers pass records of the exact stride, so offsets are constants within
/// the record; a short slice is reported as an error rather than a panic.
#[derive(Clone, Copy)]
pub struct Rec<'a>(pub &'a [u8]);
impl<'a> Rec<'a> {
    pub fn exact(bytes: &'a [u8], size: usize) -> Result<Self> {
        if bytes.len() != size {
            return Err(Error::RecordSize {
                actual: bytes.len(),
                expected: size,
            });
        }
        Ok(Self(bytes))
    }
    pub fn u8(&self, o: usize) -> u8 {
        self.0[o]
    }
    pub fn u16(&self, o: usize) -> u16 {
        u16::from_le_bytes([self.0[o], self.0[o + 1]])
    }
    pub fn i16(&self, o: usize) -> i16 {
        self.u16(o) as i16
    }
    pub fn u32(&self, o: usize) -> u32 {
        u32::from_le_bytes([self.0[o], self.0[o + 1], self.0[o + 2], self.0[o + 3]])
    }
    pub fn f32(&self, o: usize) -> f32 {
        f32::from_bits(self.u32(o))
    }
    pub fn bytes<const N: usize>(&self, o: usize) -> [u8; N] {
        self.0[o..o + N].try_into().expect("fixed record offset")
    }
    pub fn block(&self, o: usize) -> TagBlock {
        TagBlock {
            count: self.u32(o),
            address: self.u32(o + 4),
        }
    }
    pub fn data(&self, o: usize) -> DataReference {
        DataReference {
            size: self.u32(o),
            address: self.u32(o + 4),
        }
    }
    pub fn tag_ref(&self, o: usize) -> TagRef {
        TagRef {
            group: FourCC::from_u32(self.u32(o)),
            id: TagId(self.u32(o + 4)),
        }
    }
    pub fn fourcc(&self, o: usize) -> FourCC {
        FourCC::from_u32(self.u32(o))
    }
}

/// A tag reference: class and full salted datum. Null is datum `0xffffffff`.
/// The class of a null reference is whatever the author left (often the
/// expected class), so it is preserved but not interpreted.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct TagRef {
    pub group: FourCC,
    pub id: TagId,
}
impl TagRef {
    pub const fn is_null(&self) -> bool {
        self.id.0 == u32::MAX
    }
}
impl std::fmt::Debug for TagRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.is_null() {
            write!(f, "none({})", self.group)
        } else {
            write!(f, "{}:{}", self.group, self.id)
        }
    }
}

/// A "property" (function-driven scalar), 16 bytes, shared by effects,
/// particles and particle movement. Layout is **E** (the engine's property
/// evaluator at `0x00246cd0` reads exactly these fields) and **V** (every
/// instance on the disc fits the stride and its data reference resolves).
/// `data` holds the function bytes, decoded by [`super::function`].
#[derive(Clone, Debug, PartialEq)]
pub struct Property {
    /// Input variable index into the evaluation input table (+0).
    pub input: i16,
    /// Range input variable index; blends the two halves of a ranged function (+2).
    pub range_input: i16,
    /// 0 none, 1 add, 2 multiply (+4); other values are preserved.
    pub output_modifier: u16,
    /// Input variable index used by the modifier (+6).
    pub modifier_input: i16,
    /// The function bytes (+8 size, +0xc address).
    pub data: Vec<u8>,
}
impl Property {
    pub const SIZE: usize = 16;
    pub fn decode(rec: Rec<'_>, ctx: &mut Ctx<'_>) -> Result<Self> {
        let rec = Rec::exact(rec.0, Self::SIZE)?;
        Ok(Self {
            input: rec.i16(0),
            range_input: rec.i16(2),
            output_modifier: rec.u16(4),
            modifier_input: rec.i16(6),
            data: ctx.data(rec.data(8))?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem8() -> SliceMemory {
        let mut m = SliceMemory::new(0x1000);
        m.push(&[7; 8]);
        m
    }
    fn block(count: u32) -> TagBlock {
        TagBlock {
            count,
            address: 0x1000,
        }
    }
    fn data(size: u32) -> DataReference {
        DataReference {
            size,
            address: 0x1000,
        }
    }

    #[test]
    fn property_rejects_short_records_without_panicking() {
        let mut m = SliceMemory::new(0x1000);
        let mut ctx = Ctx::new(&mut m);
        assert!(Property::decode(Rec(&[0; 8]), &mut ctx).is_err());
        assert!(Property::decode(Rec(&[]), &mut ctx).is_err());
        assert!(Property::decode(Rec(&[0; 17]), &mut ctx).is_err());
        assert!(Property::decode(Rec(&[0; 16]), &mut ctx).is_ok());
    }

    #[test]
    fn block_bytes_are_charged_alone() {
        let mut m = mem8();
        let mut ctx = Ctx::new(&mut m);
        ctx.budget = Budget {
            bytes: 7,
            elements: 100,
        };
        assert!(ctx.block(block(2), 4).is_err());
    }

    #[test]
    fn block_elements_are_charged_alone() {
        let mut m = mem8();
        let mut ctx = Ctx::new(&mut m);
        ctx.budget = Budget {
            bytes: 100,
            elements: 1,
        };
        assert!(ctx.block(block(2), 4).is_err());
    }

    #[test]
    fn aliased_blocks_are_charged_every_time() {
        let mut m = mem8();
        let mut ctx = Ctx::new(&mut m);
        ctx.budget = Budget {
            bytes: 8,
            elements: 100,
        };
        assert!(ctx.block(block(2), 4).is_ok());
        assert!(ctx.block(block(2), 4).is_err());
        ctx.budget = Budget {
            bytes: 100,
            elements: 2,
        };
        assert!(ctx.block(block(2), 4).is_ok());
        assert!(ctx.block(block(2), 4).is_err());
    }

    #[test]
    fn aliased_data_is_charged_every_time() {
        let mut m = mem8();
        let mut ctx = Ctx::new(&mut m);
        ctx.budget = Budget {
            bytes: 8,
            elements: 100,
        };
        assert!(ctx.data(data(8)).is_ok());
        assert!(ctx.data(data(8)).is_err());
        // Element count alone also limits repeated data reads.
        ctx.budget = Budget {
            bytes: 100,
            elements: 1,
        };
        assert!(ctx.data(data(8)).is_ok());
        assert!(ctx.data(data(8)).is_err());
    }
}
