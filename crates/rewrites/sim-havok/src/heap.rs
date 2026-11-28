//! The Havok heap as the world's physics store (docs/engine/sim-world.md,
//! "The physics heap"). Havok is ported bit for bit, so its state is the
//! heap's bytes at their retail addresses, in retail's allocation order:
//! the memory system `0x00146a20` builds there, its pools and every
//! world, island, entity, motion, constraint and agent it hands out.
//!
//! `0x00146b30` reserves `0x106470` bytes, rounds the reservation up past
//! its first page to the next page boundary and stores that base at
//! [`BASE_GLOBAL`]; the two words below the base hold the usable size
//! (`0x105470`) and the base's offset in the reservation. The memory
//! system's globals ([`GLOBALS`], `0x00479888..0x004798a0`: the base,
//! the pool mode and the three pool objects) sit outside the block and
//! are part of the store.
use sim_world::{Reader, RestoreError, Writer, kind};

/// `[0x00479888]`: the heap's base, 0 when the reservation failed.
pub const BASE_GLOBAL: u32 = 0x0047_9888;
/// The memory system's globals the heap carries, from [`BASE_GLOBAL`].
pub const GLOBALS: u32 = BASE_GLOBAL;
pub const GLOBAL_WORDS: usize = 6;
/// The usable size `0x00146b66` stores below the base.
pub const HEAP_SIZE: u32 = 0x0010_5470;
/// The two header words below the base: the size, then the base's offset.
pub const HEADER: u32 = 8;

/// The heap's bytes and the memory system's globals.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Heap {
    /// The globals from [`GLOBALS`]; `globals[0]` is the base.
    pub globals: [u32; GLOBAL_WORDS],
    /// `[base - 8, base + HEAP_SIZE)`: the header words, then the heap.
    /// Empty when no heap is reserved.
    pub bytes: Vec<u8>,
}

impl Heap {
    pub const VERSION: u16 = 1;

    /// The heap's base address, or 0.
    pub fn base(&self) -> u32 {
        self.globals[0]
    }

    /// Read the globals and, when a base is set, the header and the heap
    /// through `read` (address, buffer). `None` when a read faults or the
    /// header's size is not retail's.
    pub fn import(mut read: impl FnMut(u32, &mut [u8]) -> Option<()>) -> Option<Heap> {
        let mut raw = [0u8; 4 * GLOBAL_WORDS];
        read(GLOBALS, &mut raw)?;
        let mut globals = [0u32; GLOBAL_WORDS];
        for (g, w) in globals.iter_mut().zip(raw.chunks_exact(4)) {
            *g = u32::from_le_bytes(w.try_into().unwrap());
        }
        let mut heap = Heap { globals, bytes: Vec::new() };
        if heap.base() == 0 {
            return Some(heap);
        }
        let start = heap.base().checked_sub(HEADER)?;
        heap.bytes = vec![0; (HEADER + HEAP_SIZE) as usize];
        read(start, &mut heap.bytes)?;
        (heap.size() == Some(HEAP_SIZE)).then_some(heap)
    }

    /// The size word below the base.
    pub fn size(&self) -> Option<u32> {
        Some(u32::from_le_bytes(self.bytes.get(..4)?.try_into().ok()?))
    }

    /// The writes that put this heap back in guest memory, in address
    /// order: the globals, then the header and the heap.
    pub fn scatter(&self) -> Vec<(u32, Vec<u8>)> {
        let globals = self.globals.iter().flat_map(|g| g.to_le_bytes()).collect();
        let mut out = vec![(GLOBALS, globals)];
        if self.base() != 0 {
            out.push((self.base() - HEADER, self.bytes.clone()));
        }
        out
    }

    /// `globals | u32 len | bytes`, little-endian.
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        for g in self.globals {
            w.u32(g);
        }
        w.u32(self.bytes.len() as u32);
        w.bytes(&self.bytes);
        w.finish()
    }

    pub fn decode(version: u16, body: &[u8]) -> Result<Heap, RestoreError> {
        const KIND: u16 = kind::PHYSICS;
        if version != Self::VERSION {
            return Err(RestoreError::SectionVersion { kind: KIND, version });
        }
        let mut r = Reader::new(body);
        let mut globals = [0u32; GLOBAL_WORDS];
        for g in &mut globals {
            *g = r.u32()?;
        }
        let len = r.u32()?;
        let bytes = r.take(len as usize)?.to_vec();
        r.end(KIND)?;
        let heap = Heap { globals, bytes };
        let expected = if heap.base() == 0 { 0 } else { HEADER + HEAP_SIZE };
        if len != expected || (len != 0 && heap.size() != Some(HEAP_SIZE)) {
            return Err(RestoreError::Corrupt { kind: KIND, what: "heap size" });
        }
        if heap.base() != 0 && heap.base() < HEADER {
            return Err(RestoreError::Corrupt { kind: KIND, what: "heap base" });
        }
        Ok(heap)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    const BASE: u32 = 0x0080_0000;

    /// Guest memory as sparse bytes: unwritten bytes read as zero, and an
    /// address at `hole` faults.
    #[derive(Default)]
    struct Memory {
        bytes: BTreeMap<u32, u8>,
        hole: Option<u32>,
    }

    impl Memory {
        fn put(&mut self, at: u32, data: &[u8]) {
            for (i, b) in data.iter().enumerate() {
                self.bytes.insert(at + i as u32, *b);
            }
        }
        fn read(&self, at: u32, buf: &mut [u8]) -> Option<()> {
            let end = at + buf.len() as u32;
            if self.hole.is_some_and(|h| (at..end).contains(&h)) {
                return None;
            }
            for (i, b) in buf.iter_mut().enumerate() {
                *b = self.bytes.get(&(at + i as u32)).copied().unwrap_or(0);
            }
            Some(())
        }
    }

    fn guest() -> Memory {
        let mut m = Memory::default();
        let globals = [BASE, 0x11, 2, BASE + 0x100, BASE + 0x200, 0];
        m.put(GLOBALS, &globals.iter().flat_map(|g| g.to_le_bytes()).collect::<Vec<_>>());
        m.put(BASE - 8, &HEAP_SIZE.to_le_bytes());
        m.put(BASE - 4, &0x0fe0u32.to_le_bytes());
        m.put(BASE + 0x30, &[1, 2, 3, 4]);
        m.put(BASE + HEAP_SIZE - 1, &[0xee]);
        m
    }

    #[test]
    fn the_heap_imports_from_its_base_and_scatters_back_byte_for_byte() {
        let m = guest();
        let heap = Heap::import(|a, b| m.read(a, b)).unwrap();
        assert_eq!(heap.base(), BASE);
        assert_eq!(heap.bytes.len() as u32, HEADER + HEAP_SIZE);
        assert_eq!(&heap.bytes[(HEADER + 0x30) as usize..][..4], &[1, 2, 3, 4]);
        assert_eq!(*heap.bytes.last().unwrap(), 0xee);
        let mut back = Memory::default();
        for (at, data) in heap.scatter() {
            back.put(at, &data);
        }
        assert_eq!(Heap::import(|a, b| back.read(a, b)).unwrap(), heap);
    }

    #[test]
    fn a_save_restores_the_same_heap_and_a_bad_one_is_refused_by_name() {
        let m = guest();
        let heap = Heap::import(|a, b| m.read(a, b)).unwrap();
        let body = heap.encode();
        assert_eq!(Heap::decode(Heap::VERSION, &body).unwrap(), heap);
        assert_eq!(
            Heap::decode(2, &body),
            Err(RestoreError::SectionVersion { kind: kind::PHYSICS, version: 2 })
        );
        let mut short = body.clone();
        short.truncate(body.len() - 1);
        assert_eq!(Heap::decode(Heap::VERSION, &short), Err(RestoreError::Truncated));
        let mut size = body.clone();
        size[4 * GLOBAL_WORDS + 4] ^= 1;
        assert_eq!(
            Heap::decode(Heap::VERSION, &size),
            Err(RestoreError::Corrupt { kind: kind::PHYSICS, what: "heap size" })
        );
        let empty = Heap::default();
        assert_eq!(Heap::decode(Heap::VERSION, &empty.encode()).unwrap(), empty);
    }

    #[test]
    fn no_base_is_an_empty_heap_and_a_fault_or_a_foreign_size_is_none() {
        let mut m = Memory::default();
        let heap = Heap::import(|a, b| m.read(a, b)).unwrap();
        assert!(heap.bytes.is_empty() && heap.base() == 0);
        assert_eq!(heap.scatter().len(), 1);
        m = guest();
        m.hole = Some(BASE + 0x1000);
        assert!(Heap::import(|a, b| m.read(a, b)).is_none());
        m = guest();
        m.put(BASE - 8, &0x1000u32.to_le_bytes());
        assert!(Heap::import(|a, b| m.read(a, b)).is_none());
    }
}
