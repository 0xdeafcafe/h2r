//! The MOPP code block: a 0x30-byte header followed by the bytecode, as a
//! structure BSP's `sbsp+0x1e0` byte block holds it (docs/formats/mopp.md).

use std::fmt;

/// Header size; the bytecode starts here (E `0x003008e7`).
pub const HEADER_SIZE: usize = 0x30;
/// Full-precision integer space: 24 bits per axis. The root frame sees its
/// top 8 bits (shift 16, E `0x00300990`).
pub const FULL_BITS: u32 = 24;
/// Shift of the root frame.
pub const ROOT_SHIFT: u32 = 16;
/// Retail value of the header word at `+0x10` on every disc BSP (V). The
/// three interpreters never read it (E); Havok's own meaning is not
/// established here.
pub const RETAIL_INFO: u32 = 0xff;

/// A parsed code block.
#[derive(Clone, Debug, PartialEq)]
pub struct Code {
    /// `+0x00`: world-space origin of the integer space.
    pub offset: [f32; 3],
    /// `+0x0c`: integer units per world unit.
    pub scale: f32,
    /// `+0x10`: 0xff on retail (V), unread by the interpreters (E).
    pub info: u32,
    /// Bytecode: from `+0x30` to the size at `+0x20`, which counts the
    /// header (V: on every retail BSP the last 0x30 bytes past `+0x30 +
    /// size` are never reached, and the block count is `size` rounded up
    /// to 16).
    pub bytecode: Vec<u8>,
}

#[derive(Debug, PartialEq)]
pub enum Error {
    Short(usize),
    Size { header: u32, available: usize },
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Short(n) => write!(f, "MOPP block of {n} bytes is shorter than its header"),
            Error::Size { header, available } => write!(
                f,
                "MOPP header says {header} bytes but the block holds {available}"
            ),
        }
    }
}
impl std::error::Error for Error {}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

impl Code {
    /// Parse the `sbsp+0x1e0` block's bytes.
    pub fn parse(block: &[u8]) -> Result<Self, Error> {
        if block.len() < HEADER_SIZE {
            return Err(Error::Short(block.len()));
        }
        let size = u32_at(block, 0x20);
        if (size as usize) < HEADER_SIZE || size as usize > block.len() {
            return Err(Error::Size {
                header: size,
                available: block.len(),
            });
        }
        Ok(Code {
            offset: [f32_at(block, 0), f32_at(block, 4), f32_at(block, 8)],
            scale: f32_at(block, 0xc),
            info: u32_at(block, 0x10),
            bytecode: block[HEADER_SIZE..size as usize].to_vec(),
        })
    }

    /// The block's bytes: header, bytecode, zero padding to 16 bytes (V:
    /// every retail count is the `+0x20` size rounded up to 16). The reference count at
    /// `+0x24` is 1, as on every retail BSP (V); the engine increments it
    /// when it wraps the code in a shape (E `0x003009b0`).
    pub fn to_block(&self) -> Vec<u8> {
        let mut out = vec![0u8; HEADER_SIZE];
        for (i, v) in self.offset.iter().enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
        }
        out[0xc..0x10].copy_from_slice(&self.scale.to_le_bytes());
        out[0x10..0x14].copy_from_slice(&self.info.to_le_bytes());
        out[0x20..0x24]
            .copy_from_slice(&((HEADER_SIZE + self.bytecode.len()) as u32).to_le_bytes());
        out[0x24..0x28].copy_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&self.bytecode);
        out.resize(out.len().next_multiple_of(16), 0);
        out
    }

    /// Full-precision integer coordinate of a world coordinate on `axis`,
    /// unclamped: `(p - offset) * scale`.
    pub fn to_full(&self, axis: usize, p: f32) -> f64 {
        (f64::from(p) - f64::from(self.offset[axis])) * f64::from(self.scale)
    }
}

/// Decode an inline key or the 1..4 big-endian bytes following a terminal
/// opcode. The byte source can be a parsed program or live guest memory.
pub fn terminal_key<E>(op: u8, operand: impl FnOnce(usize) -> Result<u32, E>) -> Result<u32, E> {
    match op {
        0x30..=0x4f => Ok(u32::from(op - 0x30)),
        0x50..=0x53 => operand(usize::from(op - 0x50) + 1),
        _ => unreachable!("expected a MOPP terminal"),
    }
}
