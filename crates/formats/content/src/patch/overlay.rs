//! Byte runs over a base: [`OffsetPatch`] (one patch) and [`Overlay`]
//! (patches composed over a base of known length).
use std::io;

use super::{PatchError, Source};

/// Bytes written at an offset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Run {
    pub offset: u64,
    pub bytes: Vec<u8>,
}
impl Run {
    pub fn end(&self) -> u64 {
        self.offset + self.bytes.len() as u64
    }
}

/// Write `bytes` at `offset` into sorted, non-overlapping, non-touching
/// `runs`; the new bytes win where they overlap.
fn insert(runs: &mut Vec<Run>, offset: u64, bytes: &[u8]) {
    if bytes.is_empty() {
        return;
    }
    let end = offset + bytes.len() as u64;
    // Runs that overlap or touch [offset, end).
    let first = runs.partition_point(|r| r.end() < offset);
    let last = runs.partition_point(|r| r.offset <= end);
    if first == last {
        runs.insert(
            first,
            Run {
                offset,
                bytes: bytes.to_vec(),
            },
        );
        return;
    }
    let start = runs[first].offset.min(offset);
    let stop = runs[last - 1].end().max(end);
    // Every byte of [start, stop) is in the new run or in one of these: they
    // all meet [offset, end), so the gaps between them lie inside it.
    let mut merged = vec![0; (stop - start) as usize];
    for r in runs.drain(first..last) {
        let at = (r.offset - start) as usize;
        merged[at..at + r.bytes.len()].copy_from_slice(&r.bytes);
    }
    let at = (offset - start) as usize;
    merged[at..at + bytes.len()].copy_from_slice(bytes);
    runs.insert(
        first,
        Run {
            offset: start,
            bytes: merged,
        },
    );
}

/// Drop everything at or past `len`.
fn truncate(runs: &mut Vec<Run>, len: u64) {
    runs.retain(|r| r.offset < len);
    if let Some(r) = runs.last_mut()
        && r.end() > len
    {
        r.bytes.truncate((len - r.offset) as usize);
    }
}

/// Copy the parts of `runs` inside `[offset, offset + buf.len())` into `buf`.
fn lay(runs: &[Run], offset: u64, buf: &mut [u8]) {
    let end = offset + buf.len() as u64;
    let first = runs.partition_point(|r| r.end() <= offset);
    for r in &runs[first..] {
        if r.offset >= end {
            break;
        }
        let from = r.offset.max(offset);
        let to = r.end().min(end);
        buf[(from - offset) as usize..(to - offset) as usize]
            .copy_from_slice(&r.bytes[(from - r.offset) as usize..(to - r.offset) as usize]);
    }
}

/// The longest file an offset patch may make: 4 GiB, as for a BPS target.
/// A PPF3 offset is 64-bit, so a damaged or hostile record could otherwise
/// ask for an exabyte of zeros.
pub const MAX_LEN: u64 = 1 << 32;

/// One offset patch: byte runs in offset order (records later in the patch
/// win where they overlap), and what it does to the file's length.
///
/// A run past the end of the file grows it, with zeros where nothing is
/// written, as IPS tools and PPF applicators (ApplyPPF seeks and writes,
/// which extends the file) do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetPatch {
    format: &'static str,
    runs: Vec<Run>,
    /// The file's length after the patch (IPS truncation).
    truncate: Option<u64>,
}

impl OffsetPatch {
    /// An empty patch in `format`.
    pub fn new(format: &'static str) -> Self {
        Self {
            format,
            runs: Vec::new(),
            truncate: None,
        }
    }
    pub fn format(&self) -> &'static str {
        self.format
    }
    /// Write `bytes` at `offset`, over anything written before.
    pub fn write(&mut self, offset: u64, bytes: &[u8]) {
        insert(&mut self.runs, offset, bytes);
    }
    /// End the file at `len` after the runs are written.
    pub fn set_truncate(&mut self, len: u64) {
        self.truncate = Some(len);
    }
    pub fn truncate_to(&self) -> Option<u64> {
        self.truncate
    }
    /// The runs: sorted, non-overlapping, not touching.
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }
}

/// Offset patches composed over a base of a known length: what a reader lays
/// over the base's bytes. Bytes `[0, base_visible)` come from the base, the
/// rest up to `len` are zero, and the runs go on top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Overlay {
    base_visible: u64,
    len: u64,
    runs: Vec<Run>,
}

impl Overlay {
    /// The base unchanged.
    pub fn new(base_len: u64) -> Self {
        Self {
            base_visible: base_len,
            len: base_len,
            runs: Vec::new(),
        }
    }
    /// The length of the patched file.
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// How much of the base shows through (less than its length after a
    /// truncation).
    pub fn base_visible(&self) -> u64 {
        self.base_visible
    }
    pub fn runs(&self) -> &[Run] {
        &self.runs
    }
    /// Whether this is the base unchanged.
    pub fn is_identity(&self) -> bool {
        self.runs.is_empty() && self.len == self.base_visible
    }

    /// Apply `patch` on top. A run past the current end grows the file
    /// (zeros where nothing is written); one past [`MAX_LEN`] is refused,
    /// and nothing is applied.
    pub fn apply(&mut self, patch: &OffsetPatch) -> Result<(), PatchError> {
        if let Some(r) = patch.runs.last()
            && r.end() > MAX_LEN
        {
            return Err(PatchError::TooLong {
                format: patch.format,
                start: r.offset,
                end: r.end(),
            });
        }
        for r in &patch.runs {
            self.write(r.offset, &r.bytes);
        }
        if let Some(t) = patch.truncate {
            self.set_len(t);
        }
        Ok(())
    }

    /// Write `bytes` at `offset`, growing the file if they pass its end.
    pub fn write(&mut self, offset: u64, bytes: &[u8]) {
        insert(&mut self.runs, offset, bytes);
        self.len = self.len.max(offset + bytes.len() as u64);
    }

    /// Cut or zero-extend the file to `len`.
    pub fn set_len(&mut self, len: u64) {
        truncate(&mut self.runs, len);
        self.base_visible = self.base_visible.min(len);
        self.len = len;
    }

    /// The patched bytes at `offset`, reading the base as needed. Short
    /// only at the end of the patched file.
    pub fn read_at<S: Source + ?Sized>(
        &self,
        base: &mut S,
        offset: u64,
        buf: &mut [u8],
    ) -> io::Result<usize> {
        let n = self.len.saturating_sub(offset).min(buf.len() as u64) as usize;
        let buf = &mut buf[..n];
        let from_base = self.base_visible.saturating_sub(offset).min(n as u64) as usize;
        base.read_exact_at(offset, &mut buf[..from_base])?;
        buf[from_base..].fill(0);
        lay(&self.runs, offset, buf);
        Ok(n)
    }

    /// The patched file, from the whole base in memory.
    pub fn materialise(&self, mut base: Vec<u8>) -> Vec<u8> {
        base.truncate(self.base_visible as usize);
        base.resize(self.len as usize, 0);
        lay(&self.runs, 0, &mut base);
        base
    }
}

/// An [`Overlay`] and its base together, as a [`Source`] of the patched
/// bytes.
pub struct OverlayView<'a, S: Source + ?Sized> {
    pub overlay: &'a Overlay,
    pub base: &'a mut S,
}
impl<S: Source + ?Sized> Source for OverlayView<'_, S> {
    fn size(&self) -> u64 {
        self.overlay.len()
    }
    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> io::Result<usize> {
        self.overlay.read_at(self.base, offset, buf)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Rng;

    /// One patch's writes, then its truncation.
    type Model = (Vec<(u64, Vec<u8>)>, Option<u64>);

    fn naive(base: &[u8], patches: &[Model]) -> Vec<u8> {
        let mut out = base.to_vec();
        for (writes, trunc) in patches {
            for (o, b) in writes.iter().filter(|w| !w.1.is_empty()) {
                let end = *o as usize + b.len();
                if out.len() < end {
                    out.resize(end, 0);
                }
                out[*o as usize..end].copy_from_slice(b);
            }
            if let Some(t) = trunc {
                out.resize(*t as usize, 0);
            }
        }
        out
    }

    #[test]
    fn runs_stay_sorted_disjoint_and_later_writes_win() {
        let mut p = OffsetPatch::new("t");
        p.write(10, b"abc");
        p.write(0, b"xy");
        p.write(12, b"ZZZ");
        p.write(5, b"");
        p.write(20, b"q");
        assert_eq!(
            p.runs(),
            &[
                Run {
                    offset: 0,
                    bytes: b"xy".to_vec()
                },
                Run {
                    offset: 10,
                    bytes: b"abZZZ".to_vec()
                },
                Run {
                    offset: 20,
                    bytes: b"q".to_vec()
                },
            ]
        );
        // A write spanning several runs and the gaps between them.
        p.write(1, &[7; 20]);
        assert_eq!(p.runs().len(), 1);
        assert_eq!(p.runs()[0].offset, 0);
        assert_eq!(p.runs()[0].bytes.len(), 21);
        assert_eq!(p.runs()[0].bytes[0], b'x');
    }

    #[test]
    fn overlay_reads_at_any_offset_equal_the_materialised_file() {
        // Property test over random bases, runs, growth and truncation.
        let mut rng = Rng::new(0x5eed);
        for case in 0..300 {
            let n = rng.below(600) as usize;
            let base = rng.bytes(n);
            let mut ov = Overlay::new(base.len() as u64);
            let mut model = Vec::new();
            for _ in 0..rng.below(4) + 1 {
                let grows = rng.below(2) == 0;
                let mut p = OffsetPatch::new("t");
                let mut writes = Vec::new();
                let limit = if grows { ov.len() + 200 } else { ov.len() };
                for _ in 0..rng.below(12) {
                    if limit == 0 {
                        break;
                    }
                    let o = rng.below(limit);
                    let n = rng.below((limit - o).min(80)) as usize;
                    let b = rng.bytes(n);
                    p.write(o, &b);
                    writes.push((o, b));
                }
                let trunc = (grows && rng.below(4) == 0).then(|| rng.below(ov.len() + 100));
                if let Some(t) = trunc {
                    p.set_truncate(t);
                }
                ov.apply(&p).unwrap();
                model.push((writes, trunc));
            }
            let want = naive(&base, &model);
            assert_eq!(ov.materialise(base.clone()), want, "case {case}");
            assert_eq!(ov.len(), want.len() as u64);
            for w in ov.runs().windows(2) {
                assert!(
                    w[0].end() < w[1].offset,
                    "case {case}: runs overlap or touch"
                );
            }
            let mut src: &[u8] = &base;
            for _ in 0..20 {
                let o = rng.below(want.len() as u64 + 10);
                let n = rng.below(300) as usize;
                let mut buf = vec![0xee; n];
                let n = ov.read_at(&mut src, o, &mut buf).unwrap();
                let lo = (o as usize).min(want.len());
                let hi = (lo + buf.len()).min(want.len());
                assert_eq!(&buf[..n], &want[lo..hi], "case {case} at {o}");
            }
            let mut view = OverlayView {
                overlay: &ov,
                base: &mut src,
            };
            assert_eq!(view.read_all().unwrap(), want);
        }
    }

    #[test]
    fn a_write_past_the_end_grows_the_file_up_to_the_cap() {
        let mut ov = Overlay::new(8);
        let mut p = OffsetPatch::new("PPF");
        p.write(6, b"abc");
        p.write(12, b"z");
        ov.apply(&p).unwrap();
        assert_eq!(ov.len(), 13);
        assert_eq!(
            ov.materialise(vec![1; 8]),
            [1, 1, 1, 1, 1, 1, b'a', b'b', b'c', 0, 0, 0, b'z']
        );
        let mut src: &[u8] = &[1; 8];
        let mut buf = [0xee; 6];
        assert_eq!(ov.read_at(&mut src, 7, &mut buf).unwrap(), 6);
        assert_eq!(buf, [b'b', b'c', 0, 0, 0, b'z']);
        // Past 4 GiB is refused, and nothing of the patch is applied.
        let mut ov = Overlay::new(8);
        let mut p = OffsetPatch::new("PPF");
        p.write(0, b"x");
        p.write(MAX_LEN - 1, b"yz");
        let e = ov.apply(&p).unwrap_err();
        assert!(e.to_string().contains("0xffffffff..0x100000001"), "{e}");
        assert!(e.to_string().contains("4 GiB"), "{e}");
        assert!(ov.is_identity(), "nothing applied");
        let mut p = OffsetPatch::new("PPF");
        p.write(MAX_LEN - 1, b"y");
        ov.apply(&p).unwrap();
        assert_eq!(ov.len(), MAX_LEN);
    }
}
