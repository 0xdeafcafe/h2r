//! [`TextureDesc::from_header`] and [`TextureDesc::total_size`] through a
//! small per-thread memo.
//!
//! The device parses every bound texture's `Format` and `Size` words, and
//! sizes its levels, several times a draw (frame-buffer, depth-alias and
//! render-target checks, the texture cache, write-back spans). Both are
//! pure functions of those two words, and a frame binds few distinct ones,
//! so a direct-mapped table answers almost every call. Errors are not
//! memoised: they are rare and re-derived each time, so the same error
//! comes back.

use std::cell::RefCell;

use gfx::texture::{TextureDesc, TextureError};

const SLOTS: usize = 64;

type Slot = Option<(u32, u32, TextureDesc)>;
type SizeSlot = Option<(TextureDesc, usize)>;

thread_local! {
    static DESCS: RefCell<[Slot; SLOTS]> = const { RefCell::new([const { None }; SLOTS]) };
    static SIZES: RefCell<[SizeSlot; SLOTS]> = const { RefCell::new([const { None }; SLOTS]) };
}

fn slot(a: u32, b: u32) -> usize {
    (a.wrapping_mul(0x9E37_79B9) ^ b.wrapping_mul(0x85EB_CA6B)).rotate_left(7) as usize % SLOTS
}

/// [`TextureDesc::from_header`], memoised.
pub(crate) fn from_header(format: u32, size: u32) -> Result<TextureDesc, TextureError> {
    let i = slot(format, size);
    if let Some(d) = DESCS.with_borrow(|t| match &t[i] {
        Some((f, s, d)) if *f == format && *s == size => Some(d.clone()),
        _ => None,
    }) {
        return Ok(d);
    }
    let d = TextureDesc::from_header(format, size)?;
    DESCS.with_borrow_mut(|t| t[i] = Some((format, size, d.clone())));
    Ok(d)
}

/// [`TextureDesc::total_size`], memoised.
pub(crate) fn total_size(desc: &TextureDesc) -> Result<usize, TextureError> {
    let i = slot(
        desc.width ^ (desc.height << 16) ^ (desc.levels << 28),
        (desc.format as u32) ^ (desc.depth << 8) ^ (desc.pitch << 12),
    );
    if let Some(n) = SIZES.with_borrow(|t| match &t[i] {
        Some((d, n)) if d == desc => Some(*n),
        _ => None,
    }) {
        return Ok(n);
    }
    let n = desc.total_size()?;
    SIZES.with_borrow_mut(|t| t[i] = Some((desc.clone(), n)));
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The memo answers what the parser answers, errors included, for
    /// every format byte and a spread of size words, twice over (the
    /// second pass from the table) and with colliding slots.
    #[test]
    fn memo_matches_the_parser() {
        let mut words = Vec::new();
        for raw in 0..=0xFFu32 {
            for dims in [1u32, 2, 3] {
                for mips in [0u32, 1, 4, 9] {
                    for cube in [0u32, 1] {
                        words.push(
                            (raw << 8) | (dims << 4) | (mips << 16) | (cube << 2) | 0x29 << 20,
                        );
                    }
                }
            }
        }
        let sizes = [0u32, 0x0001_F01F, 0x00EF_F27F, 0x0FFF_FFFF, 0xFFFF_FFFF];
        for _ in 0..2 {
            for &f in &words {
                for &s in &sizes {
                    let want = TextureDesc::from_header(f, s);
                    let got = from_header(f, s);
                    assert_eq!(got, want, "{f:#x} {s:#x}");
                    if let Ok(d) = want {
                        assert_eq!(total_size(&d), d.total_size(), "{d:?}");
                    }
                }
            }
        }
    }
}
