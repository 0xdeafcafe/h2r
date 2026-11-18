//! Writing a patch: the layer that turns a base into an edited copy.
//!
//! [`diff`] picks the format by length:
//!
//! - **Same length: PPF3.** Offset records over a fixed-size image; the
//!   resolver lays them over the disc's map as it streams, without copying.
//!   Records are the differing byte runs, with runs closer than a record
//!   header merged, split at 255 bytes. No block check, no undo data (the
//!   resolver checks the whole base's SHA-256), no FILE_ID.DIZ.
//! - **Any other length: BPS.** SourceRead where the bytes match in place,
//!   SourceCopy where they match elsewhere in the source (moved or
//!   duplicated data, such as a block `MapEditor::append_metadata` copied
//!   to the end), TargetCopy where new bytes repeat new bytes written
//!   earlier (a bitmap or sound a mod added twice, runs of padding),
//!   TargetRead for the rest. The resolver materialises it once into its
//!   cache.
//!
//! Both reproduce `edited` exactly, header checksum included: a patch made
//! from a map whose checksum is right (as `map::writer` leaves it) carries
//! that checksum, and the resolver's repair at +0x2d0 then writes the same
//! four bytes again. A patch applied by any other PPF or BPS tool gives the
//! same map, with no separate "resign" step.
use super::crc32;

/// The format [`diff`] chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Ppf3,
    Bps,
}

impl Format {
    /// The usual file extension, without the dot.
    pub fn extension(self) -> &'static str {
        match self {
            Format::Ppf3 => "ppf",
            Format::Bps => "bps",
        }
    }
}

/// A patch file's bytes and format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub format: Format,
    pub bytes: Vec<u8>,
}

/// The patch from `base` to `edited`: PPF3 when the length is unchanged,
/// BPS otherwise. `description` goes in the PPF description field (50
/// bytes, cut at a character boundary); BPS carries no metadata.
pub fn diff(base: &[u8], edited: &[u8], description: &str) -> Written {
    if base.len() == edited.len() {
        Written {
            format: Format::Ppf3,
            bytes: ppf3(base, edited, description),
        }
    } else {
        Written {
            format: Format::Bps,
            bytes: bps(base, edited),
        }
    }
}

/// Bytes of a PPF3 record header: a `u64` offset and a `u8` length.
const PPF3_RECORD: usize = 9;

/// A PPF3 patch turning `base` into `edited`, which must be as long.
///
/// # Panics
/// If the lengths differ: a PPF can grow a file but never shrink one, and
/// [`diff`] writes BPS for any change of length (it can copy moved data).
pub fn ppf3(base: &[u8], edited: &[u8], description: &str) -> Vec<u8> {
    assert_eq!(base.len(), edited.len(), "ppf3 writes same-length edits");
    let mut out = b"PPF30".to_vec();
    out.push(2);
    let mut desc = description.to_owned();
    while desc.len() > 50 {
        desc.pop();
    }
    let mut field = desc.into_bytes();
    field.resize(50, b' ');
    out.extend(field);
    // Image type 0 (BIN), no block check, no undo data, a spare byte.
    out.extend([0, 0, 0, 0]);
    let body = out.len();
    for (start, end) in runs(base, edited, PPF3_RECORD) {
        let mut at = start;
        while at < end {
            let n = (end - at).min(255);
            out.extend((at as u64).to_le_bytes());
            out.push(n as u8);
            out.extend(&edited[at..at + n]);
            at += n;
        }
    }
    // A body ending in `.DIZ` plus two bytes reads as a FILE_ID.DIZ block's
    // tail. Rewrite the last byte as a record of its own: the patch ends in
    // a record header's high offset bytes then a length of 1, never `.DIZ`.
    let tail = &out[body..];
    if tail.len() >= 6 && &tail[tail.len() - 6..tail.len() - 2] == b".DIZ" {
        let last = base.len() - 1;
        out.extend((last as u64).to_le_bytes());
        out.push(1);
        out.push(edited[last]);
    }
    out
}

/// The ranges where `a` and `b` (equal length) differ, with ranges closer
/// than `join` bytes merged.
fn runs(a: &[u8], b: &[u8], join: usize) -> Vec<(usize, usize)> {
    let mut out: Vec<(usize, usize)> = Vec::new();
    let mut at = 0;
    while at < a.len() {
        at += common(&a[at..], &b[at..]);
        if at >= a.len() {
            break;
        }
        let start = at;
        while at < a.len() && a[at] != b[at] {
            at += 1;
        }
        match out.last_mut() {
            Some(last) if start - last.1 <= join => last.1 = at,
            _ => out.push((start, at)),
        }
    }
    out
}

/// How many leading bytes `a` and `b` share.
fn common(a: &[u8], b: &[u8]) -> usize {
    let n = a.len().min(b.len());
    let (a, b) = (&a[..n], &b[..n]);
    let mut at = 0;
    for (x, y) in a.as_chunks::<8>().0.iter().zip(b.as_chunks::<8>().0) {
        let d = u64::from_le_bytes(*x) ^ u64::from_le_bytes(*y);
        if d != 0 {
            return at + (d.trailing_zeros() / 8) as usize;
        }
        at += 8;
    }
    at + a[at..]
        .iter()
        .zip(&b[at..])
        .take_while(|(x, y)| x == y)
        .count()
}

fn varint(out: &mut Vec<u8>, mut data: u64) {
    loop {
        let x = (data & 0x7f) as u8;
        data >>= 7;
        if data == 0 {
            out.push(0x80 | x);
            return;
        }
        out.push(x);
        data -= 1;
    }
}

/// Source blocks indexed for SourceCopy, by hash.
const BLOCK: usize = 64;
/// Shortest in-place match worth a SourceRead, and shortest SourceCopy.
const MIN_READ: usize = 4;
const MIN_COPY: usize = 16;

fn block_hash(b: &[u8]) -> u64 {
    b.as_chunks::<8>()
        .0
        .iter()
        .fold(0x9e37_79b9_7f4a_7c15u64, |h, w| {
            (h ^ u64::from_le_bytes(*w)).wrapping_mul(0x0100_0000_01b3) ^ (h >> 29)
        })
}

/// The first position of each `BLOCK`-aligned block, by hash (one slot per
/// hash; a collision keeps the first).
struct Index {
    mask: usize,
    slots: Vec<u32>,
    /// Blocks before this are in.
    upto: usize,
}
impl Index {
    /// Room for the blocks of `len` bytes, none in yet.
    fn with_room(len: usize) -> Self {
        let size = (len / BLOCK * 2).next_power_of_two().max(16);
        Self {
            mask: size - 1,
            slots: vec![u32::MAX; size],
            upto: 0,
        }
    }
    /// Every block of `source`.
    fn new(source: &[u8]) -> Self {
        let mut index = Self::with_room(source.len());
        index.add_upto(source, source.len());
        index
    }
    /// The blocks of `bytes` that end at or before `end`, not yet in.
    fn add_upto(&mut self, bytes: &[u8], end: usize) {
        while self.upto + BLOCK <= end {
            let p = self.upto;
            let slot = block_hash(&bytes[p..p + BLOCK]) as usize & self.mask;
            if self.slots[slot] == u32::MAX {
                self.slots[slot] = p as u32;
            }
            self.upto += BLOCK;
        }
    }
    fn find(&self, block: &[u8]) -> Option<usize> {
        let p = self.slots[block_hash(block) as usize & self.mask];
        (p != u32::MAX).then_some(p as usize)
    }
}

/// A BPS patch turning `source` into `target`.
///
/// # Panics
/// If either is 4 GiB or more (BPS readers here cap the target there, and
/// the index stores `u32` positions).
pub fn bps(source: &[u8], target: &[u8]) -> Vec<u8> {
    // As u64: on a 32-bit target (wasm32, the Live Worker) `1 << 32` does
    // not fit a usize, and no slice can reach it anyway.
    assert!((source.len() as u64) < 1 << 32 && (target.len() as u64) < 1 << 32);
    let mut out = b"BPS1".to_vec();
    varint(&mut out, source.len() as u64);
    varint(&mut out, target.len() as u64);
    varint(&mut out, 0);
    let mut index: Option<Index> = None;
    let mut literal_from: Option<usize> = None;
    let flush = |out: &mut Vec<u8>, from: &mut Option<usize>, to: usize| {
        if let Some(f) = from.take() {
            varint(out, ((to - f - 1) as u64) << 2 | 1);
            out.extend(&target[f..to]);
        }
    };
    // The SourceCopy and TargetCopy cursors, as the reader keeps them.
    let mut cursor = 0usize;
    let mut target_cursor = 0usize;
    // The target's blocks before `at`, for TargetCopy: made on first use.
    let mut written: Option<Index> = None;
    let mut at = 0;
    while at < target.len() {
        let in_place = if at < source.len() {
            common(&source[at..], &target[at..])
        } else {
            0
        };
        if in_place >= MIN_READ || (in_place > 0 && at + in_place == target.len()) {
            flush(&mut out, &mut literal_from, at);
            varint(&mut out, ((in_place - 1) as u64) << 2);
            at += in_place;
            continue;
        }
        // Continue the last copy, else look the block up.
        let mut copy = (
            cursor,
            common(source.get(cursor..).unwrap_or(&[]), &target[at..]),
        );
        if copy.1 < MIN_COPY && at + BLOCK <= target.len() && source.len() >= BLOCK {
            let index = index.get_or_insert_with(|| Index::new(source));
            if let Some(p) = index.find(&target[at..at + BLOCK]) {
                let n = common(&source[p..], &target[at..]);
                if n > copy.1 {
                    copy = (p, n);
                }
            }
        }
        // New bytes that repeat new bytes before them: continue the last
        // TargetCopy, else look the block up among the blocks written.
        // The copy may run into the bytes it writes (the reader copies byte
        // by byte), so a run of one repeated block is one action.
        let mut again = (
            target_cursor,
            if target_cursor < at {
                common(&target[target_cursor..], &target[at..])
            } else {
                0
            },
        );
        if copy.1 < MIN_COPY && again.1 < MIN_COPY && at + BLOCK <= target.len() {
            let index = written.get_or_insert_with(|| Index::with_room(target.len()));
            index.add_upto(target, at);
            if let Some(p) = index.find(&target[at..at + BLOCK]) {
                let n = common(&target[p..], &target[at..]);
                if n > again.1 {
                    again = (p, n);
                }
            }
        }
        if copy.1 >= MIN_COPY || again.1 >= MIN_COPY {
            flush(&mut out, &mut literal_from, at);
            let (kind, from, n, cursor) = if copy.1 >= again.1 {
                (2, copy.0, copy.1, &mut cursor)
            } else {
                (3, again.0, again.1, &mut target_cursor)
            };
            varint(&mut out, ((n - 1) as u64) << 2 | kind);
            let delta = from as i64 - *cursor as i64;
            varint(&mut out, delta.unsigned_abs() << 1 | u64::from(delta < 0));
            *cursor = from + n;
            at += n;
            continue;
        }
        literal_from.get_or_insert(at);
        at += 1;
    }
    flush(&mut out, &mut literal_from, target.len());
    out.extend(crc32(source).to_le_bytes());
    out.extend(crc32(target).to_le_bytes());
    let crc = crc32(&out);
    out.extend(crc.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::{Bps, Overlay, Patch};
    use crate::testutil::Rng;

    /// Apply `patch` to `base` with this crate's readers.
    pub(crate) fn apply(base: &[u8], patch: &[u8]) -> Vec<u8> {
        match Patch::parse(patch).unwrap() {
            Patch::Ppf(ppf) => {
                ppf.check(&mut &base[..]).unwrap();
                let mut o = Overlay::new(base.len() as u64);
                o.apply(&ppf.patch).unwrap();
                o.materialise(base.to_vec())
            }
            Patch::Bps(b) => b.apply(base).unwrap(),
            Patch::Ips(_) => panic!("never written"),
        }
    }

    #[test]
    fn identical_inputs_give_empty_patches() {
        let base = Rng::new(1).bytes(5000);
        let w = diff(&base, &base, "nothing");
        assert_eq!(w.format, Format::Ppf3);
        assert_eq!(w.bytes.len(), 60, "header only");
        assert_eq!(apply(&base, &w.bytes), base);
        let b = bps(&base, &base);
        // Magic, three sizes, one SourceRead, three CRCs.
        assert_eq!(b.len(), 4 + 2 + 2 + 1 + 3 + 12);
        assert_eq!(apply(&base, &b), base);
        assert_eq!(apply(&[], &bps(&[], &[])), Vec::<u8>::new());
        assert_eq!(apply(&[], &bps(&[], b"new")), b"new");
        assert_eq!(apply(b"old", &bps(b"old", &[])), Vec::<u8>::new());
    }

    #[test]
    fn ppf3_records_merge_close_runs_and_split_long_ones() {
        let base = vec![0u8; 4096];
        let mut e = base.clone();
        e[10] = 1;
        e[15] = 1; // 4 bytes apart: one record
        e[100] = 1; // far: its own record
        e[1000..1600].fill(2); // 600 bytes: three records
        let p = ppf3(&base, &e, "d");
        let Patch::Ppf(ppf) = Patch::parse(&p).unwrap() else {
            panic!()
        };
        assert_eq!(ppf.version, 3);
        assert_eq!(ppf.description, "d");
        assert!(ppf.block_check.is_none() && ppf.undo.is_empty());
        let records = 1 + 1 + 3;
        assert_eq!(p.len(), 60 + records * 9 + 6 + 1 + 600);
        assert_eq!(apply(&base, &p), e);
    }

    #[test]
    fn a_body_ending_like_a_file_id_block_still_reads() {
        let base = vec![0u8; 64];
        let mut e = base.clone();
        e[58..64].copy_from_slice(b".DIZ\x05\x00");
        let p = ppf3(&base, &e, "");
        assert_eq!(apply(&base, &p), e);
        assert!(Patch::parse(&p).is_ok_and(|p| matches!(p, Patch::Ppf(f) if f.file_id.is_none())));
    }

    #[test]
    fn long_descriptions_are_cut_at_a_character() {
        let d = "é".repeat(30);
        let p = ppf3(b"a", b"b", &d);
        let Patch::Ppf(ppf) = Patch::parse(&p).unwrap() else {
            panic!()
        };
        assert_eq!(ppf.description, "é".repeat(25));
    }

    #[test]
    fn bps_copies_moved_and_appended_blocks() {
        let mut rng = Rng::new(7);
        let base = rng.bytes(64 * 1024);
        // A copy of an existing 4 KiB block appended, plus a few new bytes.
        let mut e = base.clone();
        e.extend_from_slice(&base[8192..8192 + 4096]);
        e.extend_from_slice(b"new entry");
        let p = bps(&base, &e);
        assert!(p.len() < 100, "{} bytes", p.len());
        assert_eq!(apply(&base, &p), e);
        // An insertion shifts everything after it.
        let mut e = base[..1000].to_vec();
        e.extend_from_slice(b"inserted");
        e.extend_from_slice(&base[1000..]);
        let p = bps(&base, &e);
        assert!(p.len() < 100, "{} bytes", p.len());
        assert_eq!(apply(&base, &p), e);
        assert_eq!(Bps::parse(&p).unwrap().target_size, e.len() as u64);
    }

    #[test]
    fn bps_copies_new_bytes_that_repeat_new_bytes() {
        let mut rng = Rng::new(11);
        let base = rng.bytes(16 * 1024);
        // A new 8 KiB block twice, then 20 KiB of padding: TargetCopy.
        let added = rng.bytes(8 * 1024);
        let mut e = base.clone();
        e.extend_from_slice(&added);
        e.extend_from_slice(b"between");
        e.extend_from_slice(&added);
        e.resize(e.len() + 20 * 1024, 0);
        e.extend_from_slice(b"end");
        let p = bps(&base, &e);
        assert!(p.len() < added.len() + 400, "{} bytes", p.len());
        assert_eq!(apply(&base, &p), e);
        // The same with no source at all.
        let p = bps(&[], &e[base.len()..]);
        assert!(p.len() < added.len() + 400, "{} bytes", p.len());
        assert_eq!(apply(&[], &p), &e[base.len()..]);
    }

    /// Random edits: same-length (PPF3) and resized (BPS), anywhere,
    /// including the first and last bytes and a map's checksum field.
    #[test]
    fn random_edits_round_trip() {
        for seed in 0..300 {
            let mut rng = Rng::new(seed);
            let len = 1 + rng.below(0x3000) as usize;
            let base = rng.bytes(len);
            let mut e = base.clone();
            for _ in 0..1 + rng.below(6) {
                let at = match rng.below(4) {
                    0 => e.len().saturating_sub(1 + rng.below(8) as usize),
                    1 => 0x2d0.min(e.len().saturating_sub(1)),
                    _ => rng.below(e.len().max(1) as u64) as usize,
                };
                let n = (1 + rng.below(300) as usize).min(e.len() - at.min(e.len()));
                let bytes = rng.bytes(n);
                e[at..at + n].copy_from_slice(&bytes);
            }
            let grow = rng.below(3);
            if grow == 1 {
                let from = rng.below(base.len() as u64) as usize;
                let n = rng.below((base.len() - from) as u64 + 1) as usize;
                let copy = base[from..from + n].to_vec();
                e.extend(copy);
                let n = rng.below(200) as usize;
                e.extend(rng.bytes(n));
            } else if grow == 2 {
                let at = rng.below(e.len() as u64 + 1) as usize;
                if rng.below(2) == 0 {
                    let n = 1 + rng.below(100) as usize;
                    let ins = rng.bytes(n);
                    e.splice(at..at, ins);
                } else {
                    e.truncate(at);
                }
            }
            let w = diff(&base, &e, "random");
            let want = if e.len() == base.len() {
                Format::Ppf3
            } else {
                Format::Bps
            };
            assert_eq!(w.format, want, "seed {seed}");
            assert_eq!(apply(&base, &w.bytes), e, "seed {seed}");
            // BPS works for equal lengths too.
            assert_eq!(apply(&base, &bps(&base, &e)), e, "seed {seed}");
        }
    }
}
