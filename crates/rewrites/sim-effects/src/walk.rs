//! The effect walk of the phase entry `0x176e40`.
//!
//! Retail reads the tick length once (`[[0x510c54] + 4]`), then visits the
//! live effects of the array at `[0x4ea93c]` in index order. For each:
//!
//! 1. `0x1794e0(handle)` (stack, `ret 4`): true skips the effect (it was
//!    deleted or its owner is gone);
//! 2. `0x179730` (`EBX` = handle): true skips it (suspended or culled);
//! 3. if the datum's `+2` bit 6 is set and its lifetime word `+0xfc` is
//!    `-1`, `0x1774f0` (`EAX` = handle) deletes it;
//! 4. otherwise `0x1792b0(handle, dt)` (stack, `ret 8`) runs its events.
//!
//! The next live index is scanned from the array's bitmap after each
//! effect, with the header read again, as retail does.

/// The effect data array's pointer.
pub const ARRAY: u32 = 0x004e_a93c;
/// An effect datum's size, as the flag test indexes it (`imul eax, 0x190`).
pub const DATUM: u32 = 0x190;

/// The guest functions the walk calls, in its order.
pub const ALIVE: u32 = 0x0017_94e0;
pub const CULLED: u32 = 0x0017_9730;
pub const DELETE: u32 = 0x0017_74f0;
pub const UPDATE: u32 = 0x0017_92b0;

/// The array header's fields.
pub const HEADER_SIZE: u32 = 0x24;
pub const HEADER_COUNT: u32 = 0x38;
pub const HEADER_DATA: u32 = 0x44;
pub const HEADER_BITS: u32 = 0x48;
/// The datum's flag byte and lifetime word.
pub const FLAGS: u32 = 2;
pub const LIFETIME: u32 = 0xfc;

/// Reads the walk needs; the engine implements it over guest memory.
pub trait Mem {
    type Fault;
    fn u8(&self, at: u32) -> Result<u8, Self::Fault>;
    fn u16(&self, at: u32) -> Result<u16, Self::Fault>;
    fn u32(&self, at: u32) -> Result<u32, Self::Fault>;
}

/// The array header, read fresh each time the walk steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub size: u32,
    pub count: i32,
    pub data: u32,
    pub bits: u32,
}

impl Header {
    pub fn read<M: Mem>(m: &M) -> Result<Header, M::Fault> {
        let array = m.u32(ARRAY)?;
        Ok(Header {
            size: m.u32(array.wrapping_add(HEADER_SIZE))?,
            count: m.u32(array.wrapping_add(HEADER_COUNT))? as i32,
            data: m.u32(array.wrapping_add(HEADER_DATA))?,
            bits: m.u32(array.wrapping_add(HEADER_BITS))?,
        })
    }

    /// The first live index at or after `from` (`0x16bc00`'s scan).
    fn live_from<M: Mem>(&self, m: &M, from: i32) -> Result<Option<u32>, M::Fault> {
        if from < 0 {
            return Ok(None);
        }
        let mut i = from;
        while i < self.count {
            let word = m.u32(self.bits.wrapping_add(((i >> 5) as u32).wrapping_mul(4)))?;
            if word & (1 << (i & 31)) != 0 {
                return Ok(Some(i as u32));
            }
            i += 1;
        }
        Ok(None)
    }

    /// The handle of index `i`: the salt word (sign-extended, shifted) or
    /// the index.
    fn handle<M: Mem>(&self, m: &M, i: u32) -> Result<u32, M::Fault> {
        let salt = m.u16(self.data.wrapping_add(self.size.wrapping_mul(i)))?;
        Ok(u32::from(salt) << 16 | i)
    }
}

/// The first live effect.
pub fn first<M: Mem>(m: &M) -> Result<Option<u32>, M::Fault> {
    let h = Header::read(m)?;
    match h.live_from(m, 0)? {
        Some(i) => Ok(Some(h.handle(m, i)?)),
        None => Ok(None),
    }
}

/// The live effect after `handle`.
pub fn next<M: Mem>(m: &M, handle: u32) -> Result<Option<u32>, M::Fault> {
    let h = Header::read(m)?;
    let from = (handle & 0xffff) as i32 + 1;
    match h.live_from(m, from)? {
        Some(i) => Ok(Some(h.handle(m, i)?)),
        None => Ok(None),
    }
}

/// Step 3: the effect is deleted instead of updated.
pub fn retires<M: Mem>(m: &M, handle: u32) -> Result<bool, M::Fault> {
    let h = Header::read(m)?;
    let datum = h.data.wrapping_add((handle & 0xffff).wrapping_mul(DATUM));
    if m.u8(datum.wrapping_add(FLAGS))? >> 6 & 1 == 0 {
        return Ok(false);
    }
    Ok(m.u32(datum.wrapping_add(LIFETIME))? == u32::MAX)
}

/// The walk's counters since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Phases the walk ran.
    pub phases: u64,
    pub visited: u64,
    /// Skipped at step 1 and at step 2.
    pub dead: u64,
    pub culled: u64,
    pub retired: u64,
    pub updated: u64,
}

impl Stats {
    /// Guest leaf calls the walk made (steps 1 to 4).
    pub fn leaf_calls(&self) -> u64 {
        self.visited + (self.visited - self.dead) + self.retired + self.updated
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::BTreeMap;

    /// A sparse little-endian memory.
    #[derive(Default)]
    pub(crate) struct Sparse(pub BTreeMap<u32, u8>);

    impl Sparse {
        pub(crate) fn put32(&mut self, at: u32, v: u32) {
            for (n, b) in v.to_le_bytes().into_iter().enumerate() {
                self.0.insert(at + n as u32, b);
            }
        }
    }

    impl Mem for Sparse {
        type Fault = u32;
        fn u8(&self, at: u32) -> Result<u8, u32> {
            self.0.get(&at).copied().ok_or(at)
        }
        fn u16(&self, at: u32) -> Result<u16, u32> {
            Ok(u16::from_le_bytes([self.u8(at)?, self.u8(at + 1)?]))
        }
        fn u32(&self, at: u32) -> Result<u32, u32> {
            Ok(u32::from_le_bytes([self.u8(at)?, self.u8(at + 1)?, self.u8(at + 2)?, self.u8(at + 3)?]))
        }
    }

    pub(crate) const HDR: u32 = 0x0060_0000;
    pub(crate) const DATA: u32 = 0x0061_0000;
    pub(crate) const BITS: u32 = 0x0062_0000;

    /// An array of `count` slots, live where `live` says, salts `0x10 + i`.
    pub(crate) fn array(count: u32, live: &[u32]) -> Sparse {
        let mut m = Sparse::default();
        m.put32(ARRAY, HDR);
        m.put32(HDR + HEADER_SIZE, DATUM);
        m.put32(HDR + HEADER_COUNT, count);
        m.put32(HDR + HEADER_DATA, DATA);
        m.put32(HDR + HEADER_BITS, BITS);
        for w in 0..count.div_ceil(32) {
            let word = live.iter().filter(|&&i| i / 32 == w).fold(0u32, |a, &i| a | 1 << (i % 32));
            m.put32(BITS + 4 * w, word);
        }
        for i in 0..count {
            m.put32(DATA + DATUM * i, 0x10 + i);
            m.put32(DATA + DATUM * i + LIFETIME, 0);
        }
        m
    }

    #[test]
    fn the_walk_visits_live_slots_in_index_order() {
        let m = array(70, &[3, 4, 33, 69]);
        let mut seen = Vec::new();
        let mut h = first(&m).unwrap();
        while let Some(handle) = h {
            seen.push(handle);
            h = next(&m, handle).unwrap();
        }
        assert_eq!(seen, [0x13 << 16 | 3, 0x14 << 16 | 4, (0x10 + 33) << 16 | 33, (0x10 + 69) << 16 | 69]);
        assert_eq!(first(&array(5, &[])).unwrap(), None);
    }

    #[test]
    fn a_flagged_effect_with_no_lifetime_retires() {
        let mut m = array(2, &[0, 1]);
        assert!(!retires(&m, 0).unwrap());
        m.0.insert(DATA + FLAGS, 0x40);
        assert!(!retires(&m, 0).unwrap(), "a lifetime holds it");
        m.put32(DATA + LIFETIME, u32::MAX);
        assert!(retires(&m, 0x55 << 16).unwrap());
        m.0.insert(DATA + FLAGS, 0xbf);
        assert!(!retires(&m, 0).unwrap(), "bit 6 only");
    }
}
