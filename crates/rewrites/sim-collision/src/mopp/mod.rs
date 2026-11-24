//! MOPP programs decoded once into a flat tree, and the integer box query
//! over it (entry F, `0x00300150`; docs/engine/sim-rewrite-collision.md
//! section 4.3, the contract in docs/engine/collision-queries.md).
//!
//! A program is named by its code object pointer: the 0x30-byte header
//! (origin at `+0x00`, scale at `+0x0c`, size in bytes counting the header
//! at `+0x20`, reference count at `+0x24`) and the bytecode from `+0x30`.
//! The decoder unfolds the interpreter state into the nodes, so a query is a
//! walk over [`Node`]s with integer compares on the full-precision box and
//! no state copies. `mopp::boxvm` stays the bit model the tests compare
//! against.
pub mod boxq;
mod decode;

pub use boxq::{Corners, Found};
pub use decode::{NONE, Node, Tree};

use crate::{Bail, GuestRead};

/// Header bytes; the bytecode starts here.
pub const HEADER_BYTES: u32 = 0x30;
/// Header word: the program's size in bytes, header included.
pub const SIZE: u32 = 0x20;
/// Header word the engine increments when it wraps the code in a shape
/// (`0x003009b0`). No interpreter reads it, so the decoder ignores it.
pub const REFCOUNT: u32 = 0x24;
/// The largest program the decoder takes. Retail structure programs are a
/// few hundred KiB at most.
pub const MAX_BYTECODE: u32 = 16 << 20;
/// Decode emits at most this many nodes per bytecode byte; a program that
/// unfolds further is left to the guest (section 4.3).
pub const NODES_PER_BYTE: usize = 16;
/// Nested interpreter calls one query may make before the kernel bails
/// ([`Bail::Depth`]). Each costs the original 0x54 bytes of stack.
pub const MAX_FRAMES: u32 = 256;
/// Full-precision corners outside `[-LIMIT, LIMIT)` bail ([`Bail::Data`]):
/// inside it no 32-bit frame value or diagonal sum the original computes
/// can wrap, so the lifted thresholds compare exactly.
pub const CORNER_LIMIT: i32 = 1 << 28;

/// What the decoder and the box query read of a code object: the header
/// words the original reads and the bytecode the header's size covers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Source {
    /// Origin `+0x00..+0x0c` and scale `+0x0c`, binary32 bit patterns.
    pub origin: [u32; 3],
    pub scale: u32,
    pub bytecode: Vec<u8>,
}

/// The header words a query reads on every call: origin, scale and size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub origin: [u32; 3],
    pub scale: u32,
    pub size: u32,
}

impl Header {
    pub fn read(g: &impl GuestRead, code: u32) -> Result<Self, Bail> {
        let b = g.bytes(code, HEADER_BYTES as usize)?;
        let w = |at: u32| u32::from_le_bytes(b[at as usize..at as usize + 4].try_into().unwrap());
        let size = w(SIZE);
        if size < HEADER_BYTES || size - HEADER_BYTES > MAX_BYTECODE {
            return Err(Bail::Data);
        }
        Ok(Self {
            origin: [w(0), w(4), w(8)],
            scale: w(0xc),
            size,
        })
    }

    /// The bytecode's length.
    pub fn len(&self) -> u32 {
        self.size - HEADER_BYTES
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Source {
    /// The header and the bytecode at `code`, as the guest holds them now.
    pub fn read(g: &impl GuestRead, code: u32) -> Result<(Header, Self), Bail> {
        let header = Header::read(g, code)?;
        let at = code.checked_add(HEADER_BYTES).ok_or(Bail::Fault)?;
        let bytecode = g.bytes(at, header.len() as usize)?;
        Ok((
            header,
            Self {
                origin: header.origin,
                scale: header.scale,
                bytecode,
            },
        ))
    }

    pub fn matches(&self, header: &Header) -> bool {
        self.origin == header.origin
            && self.scale == header.scale
            && self.bytecode.len() as u32 == header.len()
    }
}

#[cfg(test)]
mod tests;
