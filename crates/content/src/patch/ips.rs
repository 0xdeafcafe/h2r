//! IPS: `PATCH`, then records of a 24-bit big-endian offset and a 16-bit
//! size followed by that many bytes, or, when the size is zero, an RLE
//! record (16-bit count, one byte value); then `EOF`. The common extension
//! (Lunar IPS) adds a 24-bit file length after `EOF` to truncate to.
//!
//! IPS may write past the end of the file it patches; the file grows, with
//! zeros where nothing is written.
use super::{OffsetPatch, PatchError, truncated};

const F: &str = "IPS";

fn be(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0, |v, b| v << 8 | u64::from(*b))
}

pub fn parse_ips(bytes: &[u8]) -> Result<OffsetPatch, PatchError> {
    if !bytes.starts_with(b"PATCH") {
        return Err(PatchError::Malformed {
            format: F,
            why: "no PATCH magic".into(),
        });
    }
    let mut patch = OffsetPatch::new(F, true);
    let mut at = 5;
    loop {
        let Some(head) = bytes.get(at..at + 3) else {
            return Err(truncated(F, "no EOF marker"));
        };
        if head == b"EOF" {
            at += 3;
            break;
        }
        let offset = be(head);
        let Some(size) = bytes.get(at + 3..at + 5) else {
            return Err(truncated(F, "record header"));
        };
        let size = be(size) as usize;
        at += 5;
        if size == 0 {
            let Some(rle) = bytes.get(at..at + 3) else {
                return Err(truncated(F, "RLE record"));
            };
            let count = be(&rle[..2]) as usize;
            if count == 0 {
                return Err(PatchError::Malformed {
                    format: F,
                    why: format!("empty RLE record at {offset:#x}"),
                });
            }
            patch.write(offset, &vec![rle[2]; count]);
            at += 3;
        } else {
            let Some(data) = bytes.get(at..at + size) else {
                return Err(truncated(F, "record data"));
            };
            patch.write(offset, data);
            at += size;
        }
    }
    match bytes.len() - at {
        0 => {}
        3 => patch.set_truncate(be(&bytes[at..])),
        n => {
            return Err(PatchError::Malformed {
                format: F,
                why: format!("{n} bytes after EOF (only a 3-byte truncation length may follow)"),
            });
        }
    }
    Ok(patch)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::patch::Overlay;

    pub(crate) enum Rec<'a> {
        Data(u32, &'a [u8]),
        Rle(u32, u16, u8),
    }
    pub(crate) fn build(records: &[Rec], truncate: Option<u32>) -> Vec<u8> {
        let mut p = b"PATCH".to_vec();
        for r in records {
            match r {
                Rec::Data(o, d) => {
                    p.extend(&o.to_be_bytes()[1..]);
                    p.extend((d.len() as u16).to_be_bytes());
                    p.extend(*d);
                }
                Rec::Rle(o, n, v) => {
                    p.extend(&o.to_be_bytes()[1..]);
                    p.extend([0, 0]);
                    p.extend(n.to_be_bytes());
                    p.push(*v);
                }
            }
        }
        p.extend(b"EOF");
        if let Some(t) = truncate {
            p.extend(&t.to_be_bytes()[1..]);
        }
        p
    }
    fn apply(bytes: &[u8], base: &[u8]) -> Vec<u8> {
        let mut ov = Overlay::new(base.len() as u64);
        ov.apply(&parse_ips(bytes).unwrap()).unwrap();
        ov.materialise(base.to_vec())
    }

    #[test]
    fn records_rle_growth_and_truncation() {
        let base = vec![1u8; 16];
        let p = build(
            &[Rec::Data(2, b"ab"), Rec::Rle(5, 3, 9), Rec::Data(18, b"z")],
            None,
        );
        let mut want = base.clone();
        want[2..4].copy_from_slice(b"ab");
        want[5..8].fill(9);
        want.extend([0, 0, b'z']);
        assert_eq!(apply(&p, &base), want);
        let p = build(&[Rec::Data(2, b"ab")], Some(4));
        assert_eq!(apply(&p, &base), [1, 1, b'a', b'b']);
        // Truncate then nothing else: a shorter file.
        assert_eq!(apply(&build(&[], Some(0)), &base), b"");
        assert_eq!(apply(&build(&[], None), &base), base);
    }

    #[test]
    fn malformed_and_truncated() {
        let good = build(&[Rec::Data(2, b"abc"), Rec::Rle(9, 4, 7)], Some(20));
        // Cutting the truncation length leaves 1 or 2 stray bytes; cutting
        // all 3 is a whole patch without it.
        for cut in 1..good.len() {
            let r = parse_ips(&good[..good.len() - cut]);
            assert_eq!(r.is_ok(), cut == 3, "cut {cut}: {r:?}");
        }
        assert!(parse_ips(b"PATCx").is_err());
        let mut extra = good.clone();
        extra.push(0);
        assert!(
            parse_ips(&extra)
                .unwrap_err()
                .to_string()
                .contains("after EOF")
        );
        let empty_rle = build(&[Rec::Rle(1, 0, 1)], None);
        assert!(
            parse_ips(&empty_rle)
                .unwrap_err()
                .to_string()
                .contains("RLE")
        );
    }
}
