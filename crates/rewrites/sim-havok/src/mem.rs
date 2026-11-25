//! A transient view of the small Havok pool. Offsets count up from its
//! first byte. The directory grows down from its end, eight bytes per
//! live allocation, in increasing offset order.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Block {
    pub offset: u32,
    pub bytes: u32,
}

#[derive(Debug)]
pub struct Pool {
    bytes: u32,
    blocks: Vec<Block>,
}

impl Pool {
    /// The guest performs signed comparisons. Decline a malformed
    /// directory rather than treating wrapped sizes as available space.
    pub fn gather(bytes: u32, blocks: Vec<Block>) -> Option<Self> {
        let count = u32::try_from(blocks.len()).ok()?;
        let directory = bytes.checked_sub(count.checked_mul(8)?)?;
        let mut end = 0;
        if bytes > i32::MAX as u32 {
            return None;
        }
        for block in &blocks {
            if block.offset < end {
                return None;
            }
            end = block.offset.checked_add(block.bytes)?;
        }
        (end <= directory).then_some(Self { bytes, blocks })
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    /// The 16-byte-aligned virtual. The guest tests whether it can fit a
    /// new directory entry before looking at any interior hole. Equality
    /// at that first test also fails, even if an earlier hole is free.
    pub fn allocate(&mut self, bytes: u32) -> Option<Block> {
        let count = u32::try_from(self.blocks.len()).ok()?;
        let limit = self.bytes.checked_sub((count + 1).checked_mul(8)?)?;
        let end = self.blocks.last().map_or(0, |b| b.offset + b.bytes);
        if end >= limit {
            return None;
        }
        let mut cursor = 0u32;
        for i in 0..=self.blocks.len() {
            let offset = cursor.checked_next_multiple_of(16)?;
            let next = self.blocks.get(i).map_or(limit, |b| b.offset);
            if offset.checked_add(bytes)? <= next {
                let block = Block { offset, bytes };
                self.blocks.insert(i, block);
                return Some(block);
            }
            if let Some(block) = self.blocks.get(i) {
                cursor = block.offset + block.bytes;
            }
        }
        None
    }

    /// Only a block's starting offset matches. Sliding the trailing
    /// entries leaves the inactive last slot in guest memory untouched.
    pub fn free(&mut self, offset: u32) {
        if let Some(i) = self.blocks.iter().position(|b| b.offset == offset) {
            self.blocks.remove(i);
        }
    }
}

/// Overflow blocks start with a copy of the previous LIFO descriptor. The
/// fast path that has sufficient room never calls these overflow virtuals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifo {
    pub cursor: u32,
    pub remaining: u32,
    pub block: u32,
    pub payload: u32,
}

impl Lifo {
    pub const HEADER_BYTES: u32 = 16;

    pub fn block_bytes(request: u32, minimum: u32) -> Option<u32> {
        request.max(minimum).checked_add(Self::HEADER_BYTES)
    }

    pub fn push(block: u32, bytes: u32, request: u32) -> Option<Self> {
        let payload = block.checked_add(Self::HEADER_BYTES)?;
        Some(Self {
            cursor: payload.checked_add(request)?,
            remaining: bytes.checked_sub(Self::HEADER_BYTES)?.checked_sub(request)?,
            block,
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Independent occupancy model: try every aligned offset against
    /// the byte ranges in an ascending list. All fixture sizes are positive.
    fn reference(bytes: u32, live: &[Block], request: u32) -> Option<Block> {
        let limit = bytes.checked_sub((live.len() as u32 + 1) * 8)?;
        if live.iter().map(|b| b.offset + b.bytes).max().unwrap_or(0) >= limit {
            return None;
        }
        (0..limit).step_by(16).find_map(|offset| {
            let end = offset + request;
            (end <= limit && live.iter().all(|b| end <= b.offset || offset >= b.offset + b.bytes))
                .then_some(Block { offset, bytes: request })
        })
    }

    #[test]
    fn fragmented_pool_matches_byte_range_first_fit() {
        let bytes = 809;
        let mut pool = Pool::gather(bytes, vec![]).unwrap();
        let mut live = Vec::new();
        let mut seed = 17u32;
        for _ in 0..2000 {
            seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
            if seed & 3 == 0 && !live.is_empty() {
                let index = (seed >> 8) as usize % live.len();
                let block: Block = live.remove(index);
                pool.free(block.offset);
            } else {
                let request = 1 + (seed >> 12) % 57;
                let want = reference(bytes, &live, request);
                assert_eq!(pool.allocate(request), want);
                if let Some(block) = want {
                    live.push(block);
                    live.sort_by_key(|b| b.offset);
                }
            }
            assert_eq!(pool.blocks(), live.as_slice());
        }
    }

    #[test]
    fn end_of_directory_precedes_hole_search() {
        let block = Block { offset: 32, bytes: 32 };
        let mut pool = Pool::gather(80, vec![block]).unwrap();
        assert_eq!(pool.allocate(1), None);
        pool.free(block.offset + 1);
        assert_eq!(pool.blocks(), &[block]);
    }

    #[test]
    fn zero_sized_allocations_share_an_offset_and_free_the_first() {
        let mut pool = Pool::gather(128, vec![]).unwrap();
        pool.allocate(8).unwrap();
        let zero = pool.allocate(0).unwrap();
        assert_eq!(zero.offset, 0);
        pool.free(0);
        assert_eq!(pool.blocks(), &[Block { offset: 0, bytes: 8 }]);
    }
}
