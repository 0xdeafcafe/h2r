//! Clean-room retail WMAv2 decoder. Checked against the game's own decoder
//! (48 streams): within 1 LSB apart from rare 2-LSB float-rounding differences.
//! No codec tables are compiled in; the owner supplies the verified retail XBE.
#![forbid(unsafe_code)]
mod bits;
pub mod boundary;
mod container;
mod decoder;
pub mod diagnostics;
mod tables;
mod transform;
pub use container::{Decoded, decode_asf};
pub use decoder::{Decoder, Format};
pub use tables::Tables;
pub type Result<T> = std::result::Result<T, Error>;
use sha2::{Digest, Sha256};

/// Retail image identity, measured from the owner's disc. This digest is an
/// identifier, not redistributed game content. See docs/wma.md.
pub const RETAIL_SHA256: [u8; 32] = [
    0x03, 0x21, 0x59, 0x19, 0xbb, 0x71, 0x63, 0x25, 0x92, 0x57, 0xd3, 0x61, 0xf4, 0xc7, 0xbf, 0x80,
    0x2a, 0x7a, 0xb1, 0x2a, 0xa8, 0x5e, 0x26, 0x89, 0x43, 0x63, 0x69, 0xb5, 0xc4, 0x27, 0x93, 0x5d,
];
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error(transparent)]
    Asf(#[from] asf::Error),
    #[error("invalid WMAv2: {0}")]
    Invalid(&'static str),
    #[error("unsupported WMAv2: {0}")]
    Unsupported(&'static str),
    #[error("truncated WMAv2 at bit {bit}")]
    Truncated { bit: usize },
    #[error("unsupported XBE: retail SHA-256 mismatch")]
    ImageHash,
    #[error(transparent)]
    Xbe(#[from] xbe::Error),
}
/// Borrowed, authenticated image for runtime table extraction and oracle use.
/// Verification precedes every address-based access; unrecognized revisions
/// cannot silently supply different tables or execute different oracle code.
pub struct VerifiedImage<'a> {
    xbe: xbe::Xbe<'a>,
}
impl<'a> VerifiedImage<'a> {
    pub fn load(bytes: &'a [u8]) -> Result<Self> {
        if Sha256::digest(bytes).as_slice() != RETAIL_SHA256 {
            return Err(Error::ImageHash);
        }
        Ok(Self {
            xbe: xbe::Xbe::parse(bytes)?,
        })
    }
    pub fn xbe(&self) -> &xbe::Xbe<'a> {
        &self.xbe
    }
    /// Read a checked virtual-address range. Named, typed table accessors will
    /// replace raw ranges as each table's extent and interpretation is recovered.
    pub fn bytes_at(&self, address: u32, length: usize) -> Result<&'a [u8]> {
        Ok(self.xbe.bytes_at(address, length)?)
    }
}
#[cfg(test)]
mod tests {
    #[test]
    fn unrecognized_images_are_rejected_before_parsing() {
        assert!(matches!(
            super::VerifiedImage::load(b"XBEH"),
            Err(super::Error::ImageHash)
        ));
    }
}
