//! Installed content (`T:\$c\<offer id>\ContentMeta.xbx`): the console-side
//! signature XAPI checks before a title may use a content package.
//!
//! The header, as Halo 2's statically linked XAPI reads it (addresses in
//! docs/game/map-pack.md, "Why the game said the content was damaged"):
//!
//! | Offset | Size | Field |
//! | ---: | ---: | --- |
//! | 0x00 | 20 | HMAC-SHA1 signature |
//! | 0x14 | 4 | magic `XCSF` |
//! | 0x18 | 4 | header size (0x88 on the map pack disc) |
//! | 0x20 | 4 | flags; bit 0 must be set for XAPI to open the package |
//! | 0x24 | 4 | title ID |
//!
//! The signature is HMAC-SHA1 over bytes `0x14..header size`, keyed with
//! `XcHMAC(XboxHDKey, title ID as 4 little-endian bytes)`. Because the key
//! comes from the console's hard disk key, the signature is "non-roamable":
//! content signed on one console fails on another. That is why the map pack
//! disc ships its packages with an all-zero signature and flag bit 0 clear,
//! and its installer signs each one for the console it installs on.
//!
//! The rest of the file (the localized names, the file table and the
//! per-file SHA-1 digests) is covered by plain SHA-1 digests inside the
//! signed header, so it is copied as is.

use crate::crypto::{XBOX_HD_KEY, hmac_sha1};

pub const SIGNATURE_SIZE: usize = 20;
pub const MAGIC_OFFSET: usize = 0x14;
pub const MAGIC: [u8; 4] = *b"XCSF";
pub const HEADER_SIZE_OFFSET: usize = 0x18;
pub const FLAGS_OFFSET: usize = 0x20;
/// Set by XAPI's install routine (when asked to); its readers refuse a
/// package without it.
pub const FLAG_INSTALLED: u32 = 1;
pub const TITLE_ID_OFFSET: usize = 0x24;
/// The smallest header XAPI's reader accepts: it reads 0x1c bytes, then up
/// to 0x88, and the fields above must be inside it.
pub const MIN_HEADER_SIZE: usize = TITLE_ID_OFFSET + 4;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ContentMetaError {
    #[error("ContentMeta.xbx is too short for its header")]
    Short,
    #[error("ContentMeta.xbx has no XCSF magic")]
    Magic,
    #[error("ContentMeta.xbx header size {0:#x} is outside the file")]
    HeaderSize(u32),
    #[error("ContentMeta.xbx is for title {found:08X}, not {expected:08X}")]
    Title { found: u32, expected: u32 },
}

fn u32_at(meta: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(meta[at..at + 4].try_into().unwrap())
}

/// The signed span `0x14..header size`, after checking the header.
fn signed_span(meta: &[u8], title_id: u32) -> Result<std::ops::Range<usize>, ContentMetaError> {
    if meta.len() < MIN_HEADER_SIZE {
        return Err(ContentMetaError::Short);
    }
    if meta[MAGIC_OFFSET..MAGIC_OFFSET + 4] != MAGIC {
        return Err(ContentMetaError::Magic);
    }
    let size = u32_at(meta, HEADER_SIZE_OFFSET);
    if (size as usize) < MIN_HEADER_SIZE || size as usize > meta.len() {
        return Err(ContentMetaError::HeaderSize(size));
    }
    let found = u32_at(meta, TITLE_ID_OFFSET);
    if found != title_id {
        return Err(ContentMetaError::Title {
            found,
            expected: title_id,
        });
    }
    Ok(MAGIC_OFFSET..size as usize)
}

/// The per-title key: `XcHMAC(hd_key, title ID)`.
pub fn title_key(hd_key: &[u8; 16], title_id: u32) -> [u8; 20] {
    hmac_sha1(hd_key, &[&title_id.to_le_bytes()])
}

/// The signature this console's XAPI expects for `meta` as it stands.
pub fn signature(meta: &[u8], title_id: u32) -> Result<[u8; 20], ContentMetaError> {
    let span = signed_span(meta, title_id)?;
    Ok(hmac_sha1(
        &title_key(&XBOX_HD_KEY, title_id),
        &[&meta[span]],
    ))
}

/// Signs `meta` for this console, as a disc installer does: sets the
/// installed flag, then writes the signature over the header. Everything
/// else is left byte for byte.
pub fn sign(meta: &mut [u8], title_id: u32) -> Result<(), ContentMetaError> {
    signed_span(meta, title_id)?;
    let flags = u32_at(meta, FLAGS_OFFSET) | FLAG_INSTALLED;
    meta[FLAGS_OFFSET..FLAGS_OFFSET + 4].copy_from_slice(&flags.to_le_bytes());
    let mac = signature(meta, title_id)?;
    meta[..SIGNATURE_SIZE].copy_from_slice(&mac);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(title: u32) -> Vec<u8> {
        let mut m = vec![0u8; 0x100];
        m[MAGIC_OFFSET..MAGIC_OFFSET + 4].copy_from_slice(&MAGIC);
        m[HEADER_SIZE_OFFSET..HEADER_SIZE_OFFSET + 4].copy_from_slice(&0x88u32.to_le_bytes());
        m[TITLE_ID_OFFSET..TITLE_ID_OFFSET + 4].copy_from_slice(&title.to_le_bytes());
        for (i, b) in m[0x88..].iter_mut().enumerate() {
            *b = i as u8;
        }
        m
    }

    #[test]
    fn sign_sets_the_flag_and_covers_only_the_header() {
        let mut m = meta(0x1234_5678);
        sign(&mut m, 0x1234_5678).unwrap();
        assert_eq!(u32_at(&m, FLAGS_OFFSET) & FLAG_INSTALLED, 1);
        assert_eq!(m[..20], signature(&m, 0x1234_5678).unwrap());
        // Signing again is a no-op; the body past the header is not signed.
        let before = m.clone();
        sign(&mut m, 0x1234_5678).unwrap();
        assert_eq!(m, before);
        m[0x90] ^= 1;
        assert_eq!(m[..20], signature(&m, 0x1234_5678).unwrap());
        m[0x87] ^= 1;
        assert_ne!(m[..20], signature(&m, 0x1234_5678).unwrap());
    }

    #[test]
    fn malformed_headers_are_refused_and_left_alone() {
        let good = meta(7);
        let mut short = good[..MIN_HEADER_SIZE - 1].to_vec();
        assert_eq!(sign(&mut short, 7), Err(ContentMetaError::Short));
        let mut bad = good.clone();
        bad[MAGIC_OFFSET] = b'Y';
        assert_eq!(sign(&mut bad.clone(), 7), Err(ContentMetaError::Magic));
        bad = good.clone();
        bad[HEADER_SIZE_OFFSET..HEADER_SIZE_OFFSET + 4].copy_from_slice(&0x101u32.to_le_bytes());
        let copy = bad.clone();
        assert_eq!(sign(&mut bad, 7), Err(ContentMetaError::HeaderSize(0x101)));
        assert_eq!(bad, copy);
        assert_eq!(
            sign(&mut good.clone(), 8),
            Err(ContentMetaError::Title {
                found: 7,
                expected: 8
            })
        );
    }
}
