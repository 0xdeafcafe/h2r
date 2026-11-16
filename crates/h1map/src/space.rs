//! Bytes loaded at a virtual address: the tag data, or one structure BSP's
//! region. Tag blocks and references inside them are addresses in that space.

use crate::format::*;
use crate::{Error, Result};

/// A run of bytes as the engine loads it at `base`.
#[derive(Debug, Clone, Copy)]
pub struct Space<'a> {
    /// The bytes.
    pub bytes: &'a [u8],
    /// The address of `bytes[0]`.
    pub base: u32,
}

impl<'a> Space<'a> {
    /// Bytes `bytes` loaded at `base`.
    pub fn new(bytes: &'a [u8], base: u32) -> Self {
        Self { bytes, base }
    }

    /// `len` bytes at `address`.
    pub fn at(&self, address: u32, len: usize) -> Result<&'a [u8]> {
        let bounds = || Error::Bounds {
            what: "address",
            offset: address as u64,
            size: len as u64,
            limit: self.base as u64 + self.bytes.len() as u64,
        };
        let start = address.checked_sub(self.base).ok_or_else(bounds)? as usize;
        let end = start.checked_add(len).ok_or_else(bounds)?;
        self.bytes.get(start..end).ok_or_else(bounds)
    }

    /// A block's elements, `elem_len` bytes each, as one slice. An empty
    /// block reads nothing whatever its address.
    pub fn block(&self, block: Block, elem_len: usize) -> Result<&'a [u8]> {
        if block.count == 0 {
            return Ok(&[]);
        }
        let len = (block.count as usize)
            .checked_mul(elem_len)
            .filter(|&n| n <= self.bytes.len())
            .ok_or(Error::Bounds {
                what: "tag block",
                offset: block.address as u64,
                size: u64::MAX,
                limit: self.bytes.len() as u64,
            })?;
        self.at(block.address, len)
    }

    /// The block field at `at` in `bytes` (a struct in this space), read as
    /// `elem_len`-byte elements.
    pub fn block_at(&self, bytes: &[u8], at: usize, elem_len: usize) -> Result<Vec<&'a [u8]>> {
        let b = Block::read(bytes, at).ok_or(Error::Bounds {
            what: "block field",
            offset: at as u64,
            size: BLOCK_LEN as u64,
            limit: bytes.len() as u64,
        })?;
        Ok(self.block(b, elem_len)?.chunks_exact(elem_len).collect())
    }
}

pub(crate) fn f32_at(b: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(b, at).unwrap_or(0))
}

pub(crate) fn i32_at(b: &[u8], at: usize) -> i32 {
    u32_at(b, at).unwrap_or(0) as i32
}

pub(crate) fn vec3(b: &[u8], at: usize) -> [f32; 3] {
    std::array::from_fn(|i| f32_at(b, at + 4 * i))
}
