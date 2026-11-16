//! BPS (byuu's patch format): `BPS1`, varints for the source size, target
//! size and metadata size, the metadata, then actions up to the last 12
//! bytes, which are the CRC32s of the source, the target and the patch
//! (the patch's over everything before its own CRC).
//!
//! Actions are a varint `(length - 1) << 2 | kind`: SourceRead (copy the
//! source at the output position), TargetRead (bytes from the patch),
//! SourceCopy and TargetCopy (copy from a signed, relative cursor into the
//! source or the target so far). Applying needs the whole source.
use super::{PatchError, truncated};

const F: &str = "BPS";

/// CRC-32 (IEEE, reflected, as zlib and BPS use).
pub fn crc32(bytes: &[u8]) -> u32 {
    static TABLE: std::sync::OnceLock<[u32; 256]> = std::sync::OnceLock::new();
    let table = TABLE.get_or_init(|| {
        let mut t = [0; 256];
        for (i, e) in t.iter_mut().enumerate() {
            let mut c = i as u32;
            for _ in 0..8 {
                c = if c & 1 != 0 {
                    0xedb8_8320 ^ (c >> 1)
                } else {
                    c >> 1
                };
            }
            *e = c;
        }
        t
    });
    !bytes.iter().fold(!0u32, |c, b| {
        table[((c ^ u32::from(*b)) & 0xff) as usize] ^ (c >> 8)
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bps {
    pub source_size: u64,
    pub target_size: u64,
    pub metadata: Vec<u8>,
    actions: Vec<u8>,
    pub source_crc: u32,
    pub target_crc: u32,
}

fn varint(bytes: &[u8], at: &mut usize) -> Result<u64, PatchError> {
    let mut data: u64 = 0;
    let mut shift: u64 = 1;
    loop {
        let Some(&x) = bytes.get(*at) else {
            return Err(truncated(F, "number"));
        };
        *at += 1;
        let overflow = || PatchError::Malformed {
            format: F,
            why: "number too large".into(),
        };
        data = u64::from(x & 0x7f)
            .checked_mul(shift)
            .and_then(|v| data.checked_add(v))
            .ok_or_else(overflow)?;
        if x & 0x80 != 0 {
            return Ok(data);
        }
        if shift > u64::MAX >> 7 {
            return Err(overflow());
        }
        shift <<= 7;
        data = data.checked_add(shift).ok_or_else(overflow)?;
    }
}

fn le32(b: &[u8]) -> u32 {
    u32::from_le_bytes(b.try_into().expect("4 bytes"))
}

impl Bps {
    /// Parse the header and footer and check the patch's own CRC.
    pub fn parse(bytes: &[u8]) -> Result<Self, PatchError> {
        if !bytes.starts_with(b"BPS1") {
            return Err(PatchError::Malformed {
                format: F,
                why: "no BPS1 magic".into(),
            });
        }
        if bytes.len() < 4 + 3 + 12 {
            return Err(truncated(F, "header or footer"));
        }
        let footer = bytes.len() - 12;
        if crc32(&bytes[..bytes.len() - 4]) != le32(&bytes[bytes.len() - 4..]) {
            return Err(PatchError::Malformed {
                format: F,
                why: "patch CRC32 mismatch (damaged or truncated)".into(),
            });
        }
        let head = &bytes[..footer];
        let mut at = 4;
        let source_size = varint(head, &mut at)?;
        let target_size = varint(head, &mut at)?;
        let meta = varint(head, &mut at)?;
        let meta = usize::try_from(meta).ok().filter(|m| *m <= head.len() - at);
        let Some(meta) = meta else {
            return Err(truncated(F, "metadata"));
        };
        let metadata = head[at..at + meta].to_vec();
        at += meta;
        Ok(Self {
            source_size,
            target_size,
            metadata,
            actions: head[at..].to_vec(),
            source_crc: le32(&bytes[footer..footer + 4]),
            target_crc: le32(&bytes[footer + 4..footer + 8]),
        })
    }

    /// The target, from the whole source. Checks the source's size and CRC
    /// first and the target's after.
    pub fn apply(&self, source: &[u8]) -> Result<Vec<u8>, PatchError> {
        let mismatch = |what: &str| PatchError::BaseMismatch {
            format: F,
            what: what.into(),
        };
        if source.len() as u64 != self.source_size {
            return Err(mismatch("source size"));
        }
        if crc32(source) != self.source_crc {
            return Err(mismatch("source CRC32"));
        }
        let bad = |why: String| PatchError::Malformed { format: F, why };
        let size = usize::try_from(self.target_size)
            .ok()
            .filter(|s| *s <= u32::MAX as usize)
            .ok_or_else(|| bad(format!("target size {} over 4 GiB", self.target_size)))?;
        let mut target: Vec<u8> = Vec::with_capacity(size);
        let (mut source_rel, mut target_rel) = (0u64, 0u64);
        let a = &self.actions;
        let mut at = 0;
        while at < a.len() {
            let d = varint(a, &mut at)?;
            let len = usize::try_from((d >> 2) + 1).map_err(|_| bad("length".into()))?;
            if len > size - target.len() {
                return Err(bad(format!("action writes past the target's {size} bytes")));
            }
            match d & 3 {
                0 => {
                    let from = target.len();
                    let s = source
                        .get(from..from + len)
                        .ok_or_else(|| bad("SourceRead past the source".into()))?;
                    target.extend_from_slice(s);
                }
                1 => {
                    let s = a
                        .get(at..at + len)
                        .ok_or_else(|| truncated(F, "TargetRead data"))?;
                    target.extend_from_slice(s);
                    at += len;
                }
                kind => {
                    let r = varint(a, &mut at)?;
                    let delta = r >> 1;
                    let cursor = if kind == 2 {
                        &mut source_rel
                    } else {
                        &mut target_rel
                    };
                    *cursor = if r & 1 != 0 {
                        cursor.checked_sub(delta)
                    } else {
                        cursor.checked_add(delta)
                    }
                    .ok_or_else(|| bad("copy cursor out of range".into()))?;
                    let from = usize::try_from(*cursor).unwrap_or(usize::MAX);
                    if kind == 2 {
                        let s = source
                            .get(from..from.saturating_add(len))
                            .ok_or_else(|| bad("SourceCopy past the source".into()))?;
                        target.extend_from_slice(s);
                    } else {
                        // May overlap what it writes: copy byte by byte.
                        if from >= target.len() {
                            return Err(bad("TargetCopy from bytes not yet written".into()));
                        }
                        for i in 0..len {
                            let b = target[from + i];
                            target.push(b);
                        }
                    }
                    *cursor += len as u64;
                }
            }
        }
        if target.len() != size {
            return Err(bad(format!(
                "actions write {} bytes, the header says {size}",
                target.len()
            )));
        }
        if crc32(&target) != self.target_crc {
            return Err(mismatch("target CRC32 (the result)"));
        }
        Ok(target)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    fn put(out: &mut Vec<u8>, mut data: u64) {
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
    pub(crate) enum Act<'a> {
        SourceRead(usize),
        TargetRead(&'a [u8]),
        SourceCopy(usize, i64),
        TargetCopy(usize, i64),
    }
    /// A BPS patch from `actions`, with correct CRCs for `source`/`target`.
    pub(crate) fn build(source: &[u8], target: &[u8], actions: &[Act]) -> Vec<u8> {
        let mut p = b"BPS1".to_vec();
        put(&mut p, source.len() as u64);
        put(&mut p, target.len() as u64);
        put(&mut p, 3);
        p.extend(b"m=1");
        let rel = |p: &mut Vec<u8>, d: i64| put(p, (d.unsigned_abs() << 1) | (d < 0) as u64);
        for a in actions {
            match a {
                Act::SourceRead(n) => put(&mut p, (*n as u64 - 1) << 2),
                Act::TargetRead(b) => {
                    put(&mut p, ((b.len() as u64 - 1) << 2) | 1);
                    p.extend(*b);
                }
                Act::SourceCopy(n, d) => {
                    put(&mut p, ((*n as u64 - 1) << 2) | 2);
                    rel(&mut p, *d);
                }
                Act::TargetCopy(n, d) => {
                    put(&mut p, ((*n as u64 - 1) << 2) | 3);
                    rel(&mut p, *d);
                }
            }
        }
        p.extend(crc32(source).to_le_bytes());
        p.extend(crc32(target).to_le_bytes());
        p.extend(crc32(&p).to_le_bytes());
        p
    }

    #[test]
    fn crc32_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(crc32(b""), 0);
    }

    #[test]
    fn varints_round_trip() {
        for v in [
            0u64,
            1,
            127,
            128,
            129,
            16383,
            16384,
            1 << 40,
            u32::MAX as u64,
        ] {
            let mut b = Vec::new();
            put(&mut b, v);
            let mut at = 0;
            assert_eq!(varint(&b, &mut at).unwrap(), v);
            assert_eq!(at, b.len());
        }
    }

    pub(crate) fn sample() -> (Vec<u8>, Vec<u8>, Vec<u8>) {
        let source = b"0123456789abcdef".to_vec();
        // "0123" read, "XY" new, "cdef" copied from the source, "efefef"
        // copied from the target as it is written (overlapping), "45"
        // copied from the source after moving the cursor back.
        let target = b"0123XYcdefefefef45".to_vec();
        let patch = build(
            &source,
            &target,
            &[
                Act::SourceRead(4),
                Act::TargetRead(b"XY"),
                Act::SourceCopy(4, 12),
                Act::TargetCopy(6, 8),
                Act::SourceCopy(2, -12),
            ],
        );
        (source, target, patch)
    }

    #[test]
    fn applies_every_action() {
        let (source, target, patch) = sample();
        let bps = Bps::parse(&patch).unwrap();
        assert_eq!(bps.metadata, b"m=1");
        assert_eq!(bps.apply(&source).unwrap(), target);
    }

    #[test]
    fn checks_crcs_sizes_and_truncation() {
        let (source, target, patch) = sample();
        let mut wrong = source.clone();
        wrong[0] ^= 1;
        let e = Bps::parse(&patch).unwrap().apply(&wrong).unwrap_err();
        assert!(e.to_string().contains("source CRC32"), "{e}");
        let e = Bps::parse(&patch).unwrap().apply(&source[1..]).unwrap_err();
        assert!(e.to_string().contains("source size"), "{e}");
        for cut in 1..patch.len() {
            assert!(
                Bps::parse(&patch[..patch.len() - cut]).is_err(),
                "cut {cut}"
            );
        }
        let mut damaged = patch.clone();
        damaged[10] ^= 0x40;
        assert!(
            Bps::parse(&damaged)
                .unwrap_err()
                .to_string()
                .contains("patch CRC32")
        );
        // A patch whose stated target CRC is wrong (re-signed so it parses).
        let mut wrong_target = target.clone();
        wrong_target[0] = b'!';
        let lying = build(&source, &wrong_target, &[Act::SourceRead(4)]);
        let e = Bps::parse(&lying).unwrap().apply(&source).unwrap_err();
        assert!(e.to_string().contains("header says"), "{e}");
        let lying = build(&source, &wrong_target[..4], &[Act::SourceRead(4)]);
        let e = Bps::parse(&lying).unwrap().apply(&source).unwrap_err();
        assert!(e.to_string().contains("target CRC32"), "{e}");
        // Actions that read outside their bounds.
        for acts in [
            vec![Act::SourceCopy(4, 14)],
            vec![Act::TargetCopy(1, 0)],
            vec![Act::SourceCopy(1, -1)],
            vec![Act::SourceRead(17)],
        ] {
            let p = build(&source, &[0; 4], &acts);
            assert!(Bps::parse(&p).unwrap().apply(&source).is_err());
        }
    }
}
