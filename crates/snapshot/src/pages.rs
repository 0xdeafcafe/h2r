//! Physical RAM as pages: zero pages cost one byte, other pages unchanged
//! since a base snapshot cost five, the rest are zstd-compressed in 1 MB frames.
//!
//! ```text
//! page size    u32      4096
//! page count   u32
//! base         u8 0/1, then the base snapshot's name (str) if 1
//! kinds        page count bytes: 0 zero, 1 data, 2 same as base
//! crcs         u32 per non-zero page, in page order: CRC-32 of its bytes
//! frames       u32 count, then each a u32 length + zstd frame holding up
//!              to FRAME_PAGES data pages, in page order
//! ```
//!
//! Every non-zero page carries its CRC, so a base page is checked against
//! the base actually supplied at load: loading a delta over the wrong base,
//! or a delta written from a stale dirty hint, fails instead of producing
//! a machine that differs from the one saved. Frames are independent, so
//! encoding and decoding can be spread over threads later without a format
//! change.
use crate::Error;
use crate::codec::{Reader, Writer};

pub const PAGE: usize = runtime::guest::PAGE_SIZE as usize;
/// Data pages per zstd frame: 1 MB.
pub const FRAME_PAGES: usize = 256;
const LEVEL: i32 = 1;

const ZERO: u8 = 0;
const DATA: u8 = 1;
const BASE: u8 = 2;

/// A snapshot the new one is a delta against.
pub struct Base<'a> {
    /// Recorded in the section, so a loader knows which file to read.
    pub name: &'a str,
    /// The base's physical RAM, as it was saved.
    pub ram: &'a [u8],
    /// Pages that may have changed since the base, if the caller tracks
    /// writes (`runtime::watch`). A page not in it is recorded as the base's
    /// without comparing; its CRC is still taken from the live bytes, so a
    /// wrong hint makes the load fail rather than come out different.
    /// `None` compares every page.
    pub dirty: Option<&'a dyn Fn(usize) -> bool>,
}

/// What an encode or decode found.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PageStats {
    pub zero: usize,
    pub data: usize,
    pub base: usize,
    pub frames: usize,
}

fn is_zero(page: &[u8]) -> bool {
    let (words, tail) = page.as_chunks::<8>();
    words.iter().all(|w| u64::from_ne_bytes(*w) == 0) && tail.iter().all(|&b| b == 0)
}

/// Encode `ram` (a whole number of pages).
pub fn encode(ram: &[u8], base: Option<&Base<'_>>) -> Result<(Vec<u8>, PageStats), Error> {
    assert!(ram.len().is_multiple_of(PAGE), "RAM is whole pages");
    let count = ram.len() / PAGE;
    if let Some(b) = base {
        assert_eq!(b.ram.len(), ram.len(), "base RAM is the same size");
    }
    let mut kinds = Vec::with_capacity(count);
    let mut crcs = Vec::new();
    let mut data_pages = Vec::new();
    let mut stats = PageStats::default();
    for (i, page) in ram.as_chunks::<PAGE>().0.iter().enumerate() {
        let same_as_base = || {
            base.is_some_and(|b| match b.dirty {
                Some(dirty) => !dirty(i),
                None => b.ram[i * PAGE..(i + 1) * PAGE] == page[..],
            })
        };
        let kind = if is_zero(page) {
            ZERO
        } else if same_as_base() {
            BASE
        } else {
            DATA
        };
        kinds.push(kind);
        match kind {
            ZERO => stats.zero += 1,
            BASE => {
                stats.base += 1;
                crcs.push(crc32fast::hash(page));
            }
            _ => {
                stats.data += 1;
                crcs.push(crc32fast::hash(page));
                data_pages.push(i);
            }
        }
    }
    let mut w = Writer::new();
    w.u32(PAGE as u32).u32(count as u32);
    match base {
        Some(b) => {
            w.u8(1).str(b.name);
        }
        None => {
            w.u8(0);
        }
    }
    w.raw(&kinds);
    for crc in &crcs {
        w.u32(*crc);
    }
    let frames: Vec<&[usize]> = data_pages.chunks(FRAME_PAGES).collect();
    w.u32(frames.len() as u32);
    let mut plain = Vec::with_capacity(FRAME_PAGES * PAGE);
    for frame in &frames {
        plain.clear();
        for &i in *frame {
            plain.extend_from_slice(&ram[i * PAGE..(i + 1) * PAGE]);
        }
        w.blob(&zstd::bulk::compress(&plain, LEVEL).map_err(Error::Io)?);
    }
    stats.frames = frames.len();
    Ok((w.into_bytes(), stats))
}

/// The base snapshot `bytes` is a delta against, if any.
pub fn base_name(bytes: &[u8]) -> Result<Option<String>, Error> {
    let mut r = Reader::new(bytes, "ram");
    r.u32()?;
    r.u32()?;
    Ok(if r.bool()? {
        Some(r.str()?.to_owned())
    } else {
        None
    })
}

/// Decode into `out`, which must be as large as the encoded RAM. Every page
/// is written, zero pages included, so `out` needs no clearing first. A
/// delta needs its base's RAM. On error `out` is partly written.
pub fn decode(bytes: &[u8], out: &mut [u8], base: Option<&[u8]>) -> Result<PageStats, Error> {
    let corrupt = |why: String| Error::Corrupt("ram", why);
    let mut r = Reader::new(bytes, "ram");
    let page_size = r.u32()? as usize;
    let count = r.u32()? as usize;
    if page_size != PAGE || count * PAGE != out.len() {
        return Err(corrupt(format!(
            "{count} pages of {page_size} bytes into {} bytes",
            out.len()
        )));
    }
    let base_name = if r.bool()? {
        Some(r.str()?.to_owned())
    } else {
        None
    };
    let base = match (base_name, base) {
        (Some(_), Some(b)) if b.len() == out.len() => Some(b),
        (Some(name), _) => return Err(Error::NeedsBase(name)),
        (None, _) => None,
    };
    let kinds = r.raw(count)?;
    let mut stats = PageStats::default();
    for &k in kinds {
        match k {
            ZERO => stats.zero += 1,
            DATA => stats.data += 1,
            BASE if base.is_some() => stats.base += 1,
            _ => return Err(corrupt(format!("page kind {k}"))),
        }
    }
    let crcs = r.raw((stats.data + stats.base) * 4)?;
    let crc_at = |n: usize| u32::from_le_bytes(crcs[n * 4..n * 4 + 4].try_into().expect("4"));
    let frames = r.u32()? as usize;
    if frames != stats.data.div_ceil(FRAME_PAGES) {
        return Err(corrupt(format!("{frames} frames for {} pages", stats.data)));
    }
    stats.frames = frames;

    let mut nth_crc = 0;
    let mut pending: &[u8] = &[];
    let mut plain = Vec::new();
    for (i, (&kind, page)) in kinds
        .iter()
        .zip(out.as_chunks_mut::<PAGE>().0.iter_mut())
        .enumerate()
    {
        match kind {
            ZERO => {
                page.fill(0);
                continue;
            }
            BASE => {
                let b = base.expect("checked above");
                page.copy_from_slice(&b[i * PAGE..(i + 1) * PAGE]);
            }
            _ => {
                if pending.is_empty() {
                    let frame = r.blob()?;
                    plain.resize(FRAME_PAGES * PAGE, 0);
                    let n = zstd::bulk::decompress_to_buffer(frame, &mut plain)
                        .map_err(|_| corrupt(format!("frame for page {i:#x}")))?;
                    plain.truncate(n);
                    if plain.is_empty() || !plain.len().is_multiple_of(PAGE) {
                        return Err(corrupt(format!("frame for page {i:#x}")));
                    }
                    pending = &plain;
                }
                page.copy_from_slice(&pending[..PAGE]);
                pending = &pending[PAGE..];
            }
        }
        if crc32fast::hash(page) != crc_at(nth_crc) {
            return Err(if kind == BASE {
                Error::BaseMismatch(i)
            } else {
                Error::Checksum(format!("ram page {i:#x}"))
            });
        }
        nth_crc += 1;
    }
    if !pending.is_empty() {
        return Err(corrupt("frame holds more pages than its kinds".into()));
    }
    r.finish()?;
    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small synthetic RAM: mostly zero, some text-like and some noisy
    /// pages, spread over more than one frame.
    fn ram(pages: usize, seed: u32) -> Vec<u8> {
        let mut ram = vec![0u8; pages * PAGE];
        let mut x = seed | 1;
        for p in (0..pages).filter(|p| p % 3 != 0) {
            for (j, b) in ram[p * PAGE..(p + 1) * PAGE].iter_mut().enumerate() {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                *b = if p % 2 == 0 { (j % 251) as u8 } else { x as u8 };
            }
        }
        ram
    }

    #[test]
    fn full_round_trip_skips_zero_pages() {
        let ram = ram(600, 7);
        let (bytes, stats) = encode(&ram, None).unwrap();
        assert_eq!(stats.zero, 200);
        assert_eq!(stats.data, 400);
        assert_eq!(stats.frames, 2);
        let mut out = vec![0xaa; ram.len()];
        assert_eq!(decode(&bytes, &mut out, None).unwrap(), stats);
        assert_eq!(out, ram);
        assert_eq!(base_name(&bytes).unwrap(), None);
    }

    #[test]
    fn all_zero_ram_is_tiny() {
        let ram = vec![0u8; 16384 * PAGE];
        let (bytes, stats) = encode(&ram, None).unwrap();
        assert_eq!(stats.frames, 0);
        assert!(bytes.len() < 16384 + 64, "{} bytes", bytes.len());
        let mut out = vec![1u8; ram.len()];
        decode(&bytes, &mut out, None).unwrap();
        assert!(out.iter().all(|&b| b == 0));
    }

    #[test]
    fn a_delta_stores_only_changed_pages() {
        let base = ram(300, 7);
        let mut now = base.clone();
        now[5 * PAGE + 9] ^= 0xff;
        now[200 * PAGE..201 * PAGE].fill(0);
        let b = Base {
            name: "parent",
            ram: &base,
            dirty: None,
        };
        let (bytes, stats) = encode(&now, Some(&b)).unwrap();
        // Zero pages stay zero (one byte, not five), page 200 included.
        assert_eq!((stats.zero, stats.data, stats.base), (101, 1, 198));
        assert_eq!(base_name(&bytes).unwrap().as_deref(), Some("parent"));
        let mut out = vec![0u8; now.len()];
        decode(&bytes, &mut out, Some(&base)).unwrap();
        assert_eq!(out, now);
        assert_eq!(
            decode(&bytes, &mut out, None),
            Err(Error::NeedsBase("parent".into()))
        );
    }

    #[test]
    fn a_delta_over_the_wrong_base_fails() {
        let base = ram(64, 7);
        let mut now = base.clone();
        now[0] ^= 1;
        let b = Base {
            name: "parent",
            ram: &base,
            dirty: None,
        };
        let (bytes, _) = encode(&now, Some(&b)).unwrap();
        let other = ram(64, 9);
        let mut out = vec![0u8; now.len()];
        assert!(matches!(
            decode(&bytes, &mut out, Some(&other)),
            Err(Error::BaseMismatch(_))
        ));
    }

    #[test]
    fn dirty_hints_skip_comparison_and_a_wrong_hint_is_caught() {
        let base = ram(64, 7);
        let mut now = base.clone();
        now[10 * PAGE] ^= 1;
        now[20 * PAGE] ^= 1;
        // Right hint: only pages 10 and 20 are looked at.
        let right = |p: usize| p == 10 || p == 20;
        let b = Base {
            name: "parent",
            ram: &base,
            dirty: Some(&right),
        };
        let (bytes, stats) = encode(&now, Some(&b)).unwrap();
        assert_eq!((stats.zero, stats.data, stats.base), (22, 2, 40));
        let mut out = vec![0u8; now.len()];
        decode(&bytes, &mut out, Some(&base)).unwrap();
        assert_eq!(out, now);
        // Wrong hint: page 20 changed but the watch missed it.
        let wrong = |p: usize| p == 10;
        let b = Base {
            name: "parent",
            ram: &base,
            dirty: Some(&wrong),
        };
        let (bytes, _) = encode(&now, Some(&b)).unwrap();
        assert_eq!(
            decode(&bytes, &mut out, Some(&base)),
            Err(Error::BaseMismatch(20))
        );
    }

    #[test]
    fn damage_and_size_mismatch_are_refused() {
        let ram = ram(40, 3);
        let (bytes, _) = encode(&ram, None).unwrap();
        let mut small = vec![0u8; 39 * PAGE];
        assert!(matches!(
            decode(&bytes, &mut small, None),
            Err(Error::Corrupt(..))
        ));
        let mut out = vec![0u8; ram.len()];
        for n in [0, 4, 9, 50, bytes.len() - 1] {
            assert!(decode(&bytes[..n], &mut out, None).is_err(), "prefix {n}");
        }
        // A changed byte inside the compressed frame.
        let mut bad = bytes.clone();
        let last = bad.len() - 20;
        bad[last] ^= 0x40;
        assert!(decode(&bad, &mut out, None).is_err());
        let mut extra = bytes.clone();
        extra.push(0);
        assert!(decode(&extra, &mut out, None).is_err());
    }
}
