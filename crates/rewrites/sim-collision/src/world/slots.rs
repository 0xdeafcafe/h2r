//! Sorted BSP address index. Warm queries binary-search the occupied
//! prefix. Cold insertion moves only index records. A boxed allocation
//! gives admission an exact capacity before it grows.
use super::Slot;

type Record = (u32, Option<Slot>);
/// Covers the key, the larger of a BSP handle and refused header, and
/// padding on both hosts. An unused capacity slot costs the same.
pub(super) const RECORD_BYTES: u64 = 96;
const _: () = assert!(std::mem::size_of::<Record>() as u64 <= RECORD_BYTES);

#[derive(Default)]
pub(super) struct Slots {
    records: Box<[Record]>,
    len: usize,
}

impl Slots {
    pub(super) fn len(&self) -> usize {
        self.len
    }

    pub(super) fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub(super) fn bytes(&self) -> u64 {
        self.records.len() as u64 * RECORD_BYTES
    }

    fn find(&self, addr: &u32) -> Result<usize, usize> {
        self.records[..self.len].binary_search_by_key(addr, |r| r.0)
    }

    pub(super) fn contains_key(&self, addr: &u32) -> bool {
        self.find(addr).is_ok()
    }

    pub(super) fn get(&self, addr: &u32) -> Option<&Slot> {
        self.records[self.find(addr).ok()?].1.as_ref()
    }

    pub(super) fn remove(&mut self, addr: &u32) -> Option<Slot> {
        let i = self.find(addr).ok()?;
        let value = self.records[i].1.take();
        self.records[i..self.len].rotate_left(1);
        self.len -= 1;
        value
    }

    fn next_capacity(&self) -> usize {
        (2 * self.records.len()).max(1)
    }

    pub(super) fn growth_bytes(&self, addr: u32) -> u64 {
        if self.len < self.records.len() || self.contains_key(&addr) {
            return 0;
        }
        (self.next_capacity() - self.len) as u64 * RECORD_BYTES
    }

    pub(super) fn insert(&mut self, addr: u32, value: Slot) {
        let i = match self.find(&addr) {
            Ok(i) => {
                self.records[i].1 = Some(value);
                return;
            }
            Err(i) => i,
        };
        if self.len == self.records.len() {
            let mut next: Box<[Record]> = (0..self.next_capacity()).map(|_| (0, None)).collect();
            next[..self.len].swap_with_slice(&mut self.records);
            self.records = next;
        }
        self.records[i..=self.len].rotate_right(1);
        self.records[i] = (addr, Some(value));
        self.len += 1;
    }

    pub(super) fn values(&self) -> impl Iterator<Item = &Slot> {
        self.records[..self.len].iter().filter_map(|r| r.1.as_ref())
    }

    pub(super) fn clear(&mut self) {
        *self = Self::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Bail;
    use crate::bsp::HEADER_WORDS;

    fn value(key: u32) -> Slot {
        Slot::Failed {
            header: [key; HEADER_WORDS],
            why: Bail::Data,
        }
    }

    #[test]
    fn growth_replacement_and_removal_keep_other_addresses_findable() {
        let mut slots = Slots::default();
        for key in [8, 12, 2, 7, 3] {
            let next = slots.bytes() + slots.growth_bytes(key);
            slots.insert(key, value(key));
            assert_eq!(slots.bytes(), next);
        }
        slots.insert(7, value(17));
        assert_eq!(slots.len(), 5);
        slots.remove(&3);
        slots.remove(&12);
        slots.remove(&2);
        assert_eq!(slots.len(), 2);
        assert!(!slots.contains_key(&3));
        for (key, expected) in [(7, 17), (8, 8)] {
            let Some(Slot::Failed { header, .. }) = slots.get(&key) else {
                panic!("missing {key}")
            };
            assert_eq!(header[0], expected);
        }
        for key in [5, 3] {
            assert_eq!(slots.growth_bytes(key), 0);
            slots.insert(key, value(key));
        }
        slots.clear();
        assert!(slots.is_empty());
        assert_eq!(slots.bytes(), 0);
    }
}
