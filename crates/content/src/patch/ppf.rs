//! PPF ("Playstation Patch File") 1.0, 2.0 and 3.0, from the public format
//! descriptions that ship with the PPF tools.
//!
//! All three start with a five-byte magic (`PPF10`, `PPF20`, `PPF30`), an
//! encoding byte (0, 1, 2) and a 50-byte description, then:
//!
//! - **PPF1**: records to the end: `u32` offset, `u8` length, data.
//! - **PPF2**: the input file's size (`u32`), 1024 bytes of the input at
//!   0x9320 (the block check), then PPF1-style records.
//! - **PPF3**: image type (0 BIN, 1 GI), block check flag, undo flag, a
//!   spare byte, the 1024-byte block (only if the flag is set), then records
//!   with `u64` offsets, each followed by its undo bytes (the original data)
//!   if the undo flag is set.
//!
//! PPF2 and PPF3 may end with a FILE_ID.DIZ block:
//! `@BEGIN_FILE_ID.DIZ`, the text, `@END_FILE_ID.DIZ`, then the text's
//! length (`u32` in PPF2, `u16` in PPF3). All numbers are little-endian.
//!
//! A record past the end of the input grows the file, zeros between: the
//! format says nothing either way, and ApplyPPF (the reference applicator)
//! opens the file for update, seeks to each record's offset and writes, so
//! a record past the end extends it. DotHalo's PPFs for maps that grew
//! (Canyon, a Relic remake, adds 2.4 MB to `dune.map`) rely on it.
use super::{OffsetPatch, PatchError, Source, truncated};

const F: &str = "PPF";
const BEGIN_DIZ: &[u8] = b"@BEGIN_FILE_ID.DIZ";
const END_DIZ: &[u8] = b"@END_FILE_ID.DIZ";
const BLOCK: usize = 1024;

/// The block of the input a PPF2/3 patch was made against.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockCheck {
    pub offset: u64,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Ppf {
    /// 1, 2 or 3.
    pub version: u8,
    /// The description field, trailing spaces and NULs trimmed.
    pub description: String,
    /// The FILE_ID.DIZ text, if any.
    pub file_id: Option<String>,
    /// PPF2: the input file's size.
    pub input_size: Option<u64>,
    pub block_check: Option<BlockCheck>,
    /// PPF3 undo data: the original bytes at each record, in record order.
    pub undo: Vec<(u64, Vec<u8>)>,
    pub patch: OffsetPatch,
}

fn le(bytes: &[u8]) -> u64 {
    bytes.iter().rev().fold(0, |v, b| v << 8 | u64::from(*b))
}

/// Split off a FILE_ID.DIZ block whose length field is `len_size` bytes.
fn split_diz(body: &[u8], len_size: usize) -> Result<(&[u8], Option<String>), PatchError> {
    let tail = len_size + 4;
    if body.len() < tail || &body[body.len() - tail..body.len() - len_size] != b".DIZ" {
        return Ok((body, None));
    }
    let len = le(&body[body.len() - len_size..]) as usize;
    let block = BEGIN_DIZ.len() + END_DIZ.len() + len_size;
    let bad = |why: &str| PatchError::Malformed {
        format: F,
        why: format!("FILE_ID.DIZ block: {why}"),
    };
    let total = len
        .checked_add(block)
        .ok_or_else(|| bad("length overflows"))?;
    if total > body.len() {
        return Err(bad("longer than the patch"));
    }
    let start = body.len() - total;
    let diz = &body[start..];
    if !diz.starts_with(BEGIN_DIZ) {
        return Err(bad("no @BEGIN_FILE_ID.DIZ where its length says"));
    }
    if &diz[BEGIN_DIZ.len() + len..BEGIN_DIZ.len() + len + END_DIZ.len()] != END_DIZ {
        return Err(bad("no @END_FILE_ID.DIZ after the text"));
    }
    let text = &diz[BEGIN_DIZ.len()..BEGIN_DIZ.len() + len];
    Ok((
        &body[..start],
        Some(String::from_utf8_lossy(text).into_owned()),
    ))
}

impl Ppf {
    pub fn parse(bytes: &[u8]) -> Result<Self, PatchError> {
        if bytes.len() < 56 {
            return Err(truncated(F, "header"));
        }
        let version = match &bytes[..5] {
            b"PPF10" => 1,
            b"PPF20" => 2,
            b"PPF30" => 3,
            m => {
                return Err(PatchError::Malformed {
                    format: F,
                    why: format!("unknown magic {:?}", String::from_utf8_lossy(m)),
                });
            }
        };
        if bytes[5] != version - 1 {
            return Err(PatchError::Malformed {
                format: F,
                why: format!("PPF{version}0 with encoding byte {}", bytes[5]),
            });
        }
        let description = String::from_utf8_lossy(&bytes[6..56])
            .trim_end_matches(['\0', ' '])
            .to_owned();
        let mut input_size = None;
        let mut block_check = None;
        let mut undo_flag = false;
        let (body, file_id) = match version {
            1 => (&bytes[56..], None),
            2 => {
                if bytes.len() < 60 + BLOCK {
                    return Err(truncated(F, "PPF2 header"));
                }
                input_size = Some(le(&bytes[56..60]));
                block_check = Some(BlockCheck {
                    offset: 0x9320,
                    bytes: bytes[60..60 + BLOCK].to_vec(),
                });
                split_diz(&bytes[60 + BLOCK..], 4)?
            }
            _ => {
                if bytes.len() < 60 {
                    return Err(truncated(F, "PPF3 header"));
                }
                let offset = match bytes[56] {
                    0 => 0x9320,
                    1 => 0x80a0,
                    t => {
                        return Err(PatchError::Malformed {
                            format: F,
                            why: format!("image type {t}"),
                        });
                    }
                };
                let flag = |b: u8, what: &str| match b {
                    0 => Ok(false),
                    1 => Ok(true),
                    _ => Err(PatchError::Malformed {
                        format: F,
                        why: format!("{what} flag {b}"),
                    }),
                };
                let check = flag(bytes[57], "block check")?;
                undo_flag = flag(bytes[58], "undo")?;
                let mut at = 60;
                if check {
                    if bytes.len() < at + BLOCK {
                        return Err(truncated(F, "PPF3 block check"));
                    }
                    block_check = Some(BlockCheck {
                        offset,
                        bytes: bytes[at..at + BLOCK].to_vec(),
                    });
                    at += BLOCK;
                }
                split_diz(&bytes[at..], 2)?
            }
        };
        let offset_size = if version == 3 { 8 } else { 4 };
        let mut patch = OffsetPatch::new(F);
        let mut undo = Vec::new();
        let mut at = 0;
        while at < body.len() {
            if body.len() - at < offset_size + 1 {
                return Err(truncated(F, "record header"));
            }
            let offset = le(&body[at..at + offset_size]);
            let len = body[at + offset_size] as usize;
            at += offset_size + 1;
            let data_len = if undo_flag { 2 * len } else { len };
            if body.len() - at < data_len {
                return Err(truncated(F, "record data"));
            }
            if offset.checked_add(len as u64).is_none() {
                return Err(PatchError::Malformed {
                    format: F,
                    why: format!("record at {offset:#x} overflows"),
                });
            }
            patch.write(offset, &body[at..at + len]);
            if undo_flag {
                undo.push((offset, body[at + len..at + 2 * len].to_vec()));
            }
            at += data_len;
        }
        Ok(Self {
            version,
            description,
            file_id,
            input_size,
            block_check,
            undo,
            patch,
        })
    }

    /// Check the patch was made against `base` (the bytes it applies to):
    /// the PPF2 input size, the block check, and the PPF3 undo data (the
    /// original bytes), where present. Undo data is checked only when no two
    /// records overlap, since a later record's undo bytes hold the earlier
    /// record's output.
    pub fn check<S: Source + ?Sized>(&self, base: &mut S) -> Result<(), PatchError> {
        let mismatch = |what: String| PatchError::BaseMismatch { format: F, what };
        if let Some(size) = self.input_size
            && size != base.size()
        {
            return Err(mismatch(format!(
                "input size {size} (the base is {} bytes)",
                base.size()
            )));
        }
        if let Some(b) = &self.block_check {
            let mut got = vec![0; b.bytes.len()];
            if b.offset + got.len() as u64 > base.size() {
                return Err(mismatch(format!(
                    "block check at {:#x} (past the end)",
                    b.offset
                )));
            }
            base.read_exact_at(b.offset, &mut got)?;
            if got != b.bytes {
                return Err(mismatch(format!("block check at {:#x}", b.offset)));
            }
        }
        let undo_len: usize = self.undo.iter().map(|u| u.1.len()).sum();
        let written: usize = self.patch.runs().iter().map(|r| r.bytes.len()).sum();
        if undo_len == written {
            for (offset, want) in &self.undo {
                let end = offset + want.len() as u64;
                if end > base.size() {
                    // Reported as out of range when applied.
                    continue;
                }
                let mut got = vec![0; want.len()];
                base.read_exact_at(*offset, &mut got)?;
                if &got != want {
                    return Err(mismatch(format!("undo data at {offset:#x}")));
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::patch::Overlay;

    fn header(version: u8, desc: &str) -> Vec<u8> {
        let mut h = format!("PPF{version}0").into_bytes();
        h.push(version - 1);
        let mut d = desc.as_bytes().to_vec();
        d.resize(50, b' ');
        h.extend(d);
        h
    }
    fn diz(text: &str, len_size: usize) -> Vec<u8> {
        let mut v = BEGIN_DIZ.to_vec();
        v.extend(text.as_bytes());
        v.extend(END_DIZ);
        v.extend(&(text.len() as u64).to_le_bytes()[..len_size]);
        v
    }
    fn base() -> Vec<u8> {
        (0..0xa000u32).map(|i| (i * 7) as u8).collect()
    }
    /// A PPF patch over `records`, PPF2/3 with the block taken from `base`.
    pub(crate) fn build(
        version: u8,
        base: &[u8],
        records: &[(u64, &[u8])],
        undo: bool,
        check: bool,
        file_id: Option<&str>,
    ) -> Vec<u8> {
        let mut p = header(version, "synthetic");
        match version {
            2 => {
                p.extend((base.len() as u32).to_le_bytes());
                p.extend(&base[0x9320..0x9320 + BLOCK]);
            }
            3 => {
                p.extend([0, check as u8, undo as u8, 0]);
                if check {
                    p.extend(&base[0x9320..0x9320 + BLOCK]);
                }
            }
            _ => {}
        }
        for (o, d) in records {
            if version == 3 {
                p.extend(o.to_le_bytes());
            } else {
                p.extend((*o as u32).to_le_bytes());
            }
            p.push(d.len() as u8);
            p.extend(*d);
            if undo {
                let at = *o as usize;
                p.extend(&base[at..at + d.len()]);
            }
        }
        if let Some(t) = file_id {
            p.extend(diz(t, if version == 3 { 2 } else { 4 }));
        }
        p
    }

    fn apply(ppf: &Ppf, base: &[u8]) -> Result<Vec<u8>, PatchError> {
        let mut src: &[u8] = base;
        ppf.check(&mut src)?;
        let mut ov = Overlay::new(base.len() as u64);
        ov.apply(&ppf.patch)?;
        Ok(ov.materialise(base.to_vec()))
    }

    #[test]
    fn all_three_versions_apply() {
        let base = base();
        let records: &[(u64, &[u8])] = &[(0x10, b"hello"), (0x9fff, b"!"), (0x12, b"LL")];
        let mut want = base.clone();
        want[0x10..0x15].copy_from_slice(b"heLLo");
        want[0x9fff] = b'!';
        for (version, undo, check, id) in [
            (1, false, false, None),
            (2, false, true, None),
            (2, false, true, Some("hi there")),
            (3, false, false, None),
            (3, true, true, Some("diz")),
            (3, false, true, None),
        ] {
            let bytes = build(version, &base, records, undo, check, id);
            let ppf = Ppf::parse(&bytes).unwrap();
            assert_eq!(ppf.version, version);
            assert_eq!(ppf.description, "synthetic");
            assert_eq!(ppf.file_id.as_deref(), id);
            assert_eq!(ppf.block_check.is_some(), version == 2 || check);
            assert_eq!(apply(&ppf, &base).unwrap(), want, "PPF{version}");
        }
    }

    #[test]
    fn ppf3_offsets_are_64_bit() {
        let big = 0x1_0000_0010u64;
        let mut p = header(3, "");
        p.extend([0, 0, 0, 0]);
        p.extend(big.to_le_bytes());
        p.push(2);
        p.extend(b"ok");
        let ppf = Ppf::parse(&p).unwrap();
        assert_eq!(ppf.patch.runs()[0].offset, big);
        // Past 4 GiB: refused, where a reader would otherwise make the
        // whole file of zeros.
        let e = Overlay::new(0x1000).apply(&ppf.patch).unwrap_err();
        assert!(e.to_string().contains("past the 4 GiB"), "{e}");
    }

    #[test]
    fn records_past_the_end_grow_the_file() {
        // As a DotHalo PPF for a map that grew: a block check against the
        // base, a record across the base's end, and one well past it with
        // zeros between.
        let base = base();
        let n = base.len() as u64;
        let records: &[(u64, &[u8])] = &[(0x10, b"in"), (n - 4, b"across"), (n + 0x20, b"tail")];
        let ppf = Ppf::parse(&build(3, &base, records, false, true, None)).unwrap();
        let got = apply(&ppf, &base).unwrap();
        let mut want = base.clone();
        want[0x10..0x12].copy_from_slice(b"in");
        want.truncate(base.len() - 4);
        want.extend(b"across");
        want.resize(base.len() + 0x20, 0);
        want.extend(b"tail");
        assert_eq!(got, want);
        // The block check still holds the base to the one it was made for.
        let mut other = base.clone();
        other[0x9320] ^= 1;
        assert!(apply(&ppf, &other).is_err());
        // PPF2 too: its input size is the base's, before it grew.
        let ppf = Ppf::parse(&build(2, &base, &[(n, b"!")], false, true, None)).unwrap();
        let got = apply(&ppf, &base).unwrap();
        assert_eq!(got.len(), base.len() + 1);
        assert_eq!(got[base.len()], b'!');
    }

    #[test]
    fn checks_against_the_base() {
        let base = base();
        let mut other = base.clone();
        other[0x9320 + 5] ^= 1;
        let p2 = Ppf::parse(&build(2, &base, &[(0, b"x")], false, true, None)).unwrap();
        assert!(
            apply(&p2, &other)
                .unwrap_err()
                .to_string()
                .contains("block check")
        );
        let short = &base[..base.len() - 1];
        assert!(
            apply(&p2, short)
                .unwrap_err()
                .to_string()
                .contains("input size")
        );
        let p3 = Ppf::parse(&build(3, &base, &[(0x20, b"xyz")], true, false, None)).unwrap();
        let mut other = base.clone();
        other[0x21] ^= 1;
        assert!(
            apply(&p3, &other)
                .unwrap_err()
                .to_string()
                .contains("undo data at 0x20")
        );
        assert!(apply(&p3, &base).is_ok());
        // PPF3 without a block check accepts another base.
        let p3 = Ppf::parse(&build(3, &base, &[(0x20, b"xyz")], false, false, None)).unwrap();
        assert!(apply(&p3, &other).is_ok());
    }

    #[test]
    fn malformed_and_truncated() {
        let base = base();
        let good = build(3, &base, &[(4, b"abcd")], true, true, Some("text"));
        // Every truncation is refused, except the two cuts that leave a
        // whole patch: without its DIZ, and without its one record too.
        let diz_len = diz("text", 2).len();
        for cut in 1..good.len() {
            let r = Ppf::parse(&good[..good.len() - cut]);
            let whole = cut == diz_len || cut == good.len() - (60 + BLOCK);
            assert_eq!(r.is_ok(), whole, "cut {cut}: {r:?}");
        }
        for (i, b, why) in [
            (3, b'4', "magic"),
            (5, 7, "encoding"),
            (56, 9, "image type"),
            (57, 2, "block check flag"),
            (58, 3, "undo flag"),
        ] {
            let mut bad = good.clone();
            bad[i] = b;
            let e = Ppf::parse(&bad).unwrap_err().to_string();
            assert!(e.contains(why), "{why}: {e}");
        }
        // A DIZ length that points outside the patch.
        let mut bad = good.clone();
        let n = bad.len();
        bad[n - 2..].copy_from_slice(&0xfff0u16.to_le_bytes());
        assert!(
            Ppf::parse(&bad)
                .unwrap_err()
                .to_string()
                .contains("FILE_ID.DIZ")
        );
        assert!(Ppf::parse(b"PPF30").is_err());
        assert!(
            Ppf::parse(&header(2, ""))
                .unwrap_err()
                .to_string()
                .contains("truncated")
        );
    }
}
