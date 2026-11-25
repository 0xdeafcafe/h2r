//! Tick-boundary source checks. The caller holds a stable view of guest RAM.
use super::{CollisionWorld, Slot};
use crate::bsp::{Bsp, Consts, HASH_SEED, block, hash};
use crate::{Bail, GuestRead, mopp};

#[derive(Debug, PartialEq, Eq)]
pub struct SourceFingerprint {
    pub kind: &'static str,
    pub addr: u32,
    pub cached: u64,
    pub live: Result<u64, Bail>,
}

fn live_bsp(g: &impl GuestRead, addr: u32, consts: Consts) -> Result<u64, Bail> {
    let header = Bsp::read_header(g, addr)?;
    Bsp::essential_bytes(&header)?;
    let mut h = HASH_SEED;
    for word in header {
        h = hash(h, &word.to_le_bytes());
    }
    let mut buf = [0; 4096];
    for (i, stride) in block::STRIDE.into_iter().enumerate() {
        let mut left = header[2 * i] * stride;
        let start = header[2 * i + 1];
        if left != 0 {
            start.checked_add(left - 1).ok_or(Bail::Fault)?;
        }
        let mut offset = 0;
        while left != 0 {
            let n = left.min(buf.len() as u32);
            g.read(start + offset, &mut buf[..n as usize])?;
            h = hash(h, &buf[..n as usize]);
            offset += n;
            left -= n;
        }
    }
    Ok(consts.fingerprint(h))
}

fn mopp_hash(source: &mopp::Source) -> u64 {
    let mut h = HASH_SEED;
    for word in source
        .origin
        .into_iter()
        .chain([source.scale, source.bytecode.len() as u32])
    {
        h = hash(h, &word.to_le_bytes());
    }
    hash(h, &source.bytecode)
}

impl CollisionWorld {
    /// Compare the resident BSP and MOPP fingerprints with independent reads.
    /// With `clear`, drop *all* resident collision data before releasing the
    /// world. An in-flight decode cannot publish into the cleared generation.
    pub fn audit_sources(
        &self,
        g: &impl GuestRead,
        constants: Result<Consts, Bail>,
        clear: bool,
    ) -> Vec<SourceFingerprint> {
        let mut inner = self.lock();
        let mut rows = Vec::new();
        for slot in inner.bsps.values() {
            if let Slot::Built(bsp) = slot {
                rows.push(SourceFingerprint {
                    kind: "bsp",
                    addr: bsp.addr(),
                    cached: bsp.fingerprint,
                    live: constants.and_then(|c| live_bsp(g, bsp.addr(), c)),
                });
            }
        }
        for (&addr, slot) in &inner.mopps {
            rows.push(SourceFingerprint {
                kind: "mopp",
                addr,
                cached: mopp_hash(slot.tree.source()),
                live: mopp::Source::read(g, addr).map(|(_, source)| mopp_hash(&source)),
            });
        }
        if clear {
            inner.flush();
        }
        rows.sort_unstable_by_key(|row| (row.kind, row.addr));
        rows
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::synth::{Builder, CONSTS, Image};
    use sim_core::Epoch;

    #[test]
    fn fingerprints_reread_both_formats_and_clear_admits_new_bytes() {
        static EPOCH: Epoch = Epoch::new();
        const BSP: u32 = 0x100;
        const CODE: u32 = 0x1000;
        let world = CollisionWorld::new(&EPOCH);
        let mut g = Image::new(0, 0x2000);
        let shape = Builder {
            planes: vec![[1., 0., 0., 3.]],
            ..Builder::default()
        };
        shape.write(BSP, |a, d| g.put(a, d));
        g.word(CODE + mopp::SIZE, mopp::HEADER_BYTES + 1);
        g.put(CODE + mopp::HEADER_BYTES, &[0x31]);
        let bsp = world.bsp(&g, BSP, || Ok(CONSTS)).unwrap();
        world.mopp(&g, CODE, 0).unwrap();
        let audit = |g: &Image, cs, clear| world.audit_sources(g, Ok(cs), clear);
        let expected = audit(&g, CONSTS, false);
        assert_eq!(expected.len(), 2);
        assert!(expected.iter().all(|row| row.live == Ok(row.cached)));
        // A reference count is unrelated to the program.
        g.word(CODE + 0x24, 99);
        assert_eq!(audit(&g, CONSTS, false), expected);
        let mut edited_consts = CONSTS;
        edited_consts.projection[1][1] += 1;
        assert_ne!(
            audit(&g, edited_consts, false)[0].live,
            Ok(expected[0].cached)
        );
        g.float(bsp.plane_addr(0), -1.0);
        g.put(CODE + mopp::HEADER_BYTES, &[0x32]);
        let changed = audit(&g, CONSTS, true);
        assert!(
            changed
                .iter()
                .all(|r| r.live.is_ok() && r.live != Ok(r.cached))
        );
        assert!(audit(&g, CONSTS, false).is_empty());
        world.bsp(&g, BSP, || Ok(CONSTS)).unwrap();
        world.mopp(&g, CODE, 0).unwrap();
        let rebuilt = audit(&g, CONSTS, false);
        for (before, after) in changed.iter().zip(&rebuilt) {
            assert_eq!(before.live, Ok(after.cached));
            assert_eq!(after.live, before.live);
        }
        // Even unreadable live sources report the cached value and a reason.
        let truncated = Image::new(0, 10);
        assert!(
            audit(&truncated, CONSTS, false)
                .iter()
                .all(|r| r.live == Err(Bail::Fault))
        );
    }
}
