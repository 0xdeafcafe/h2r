//! The object memory pool's allocator and compaction, ported from the
//! lifted functions (docs/engine/object-update.md, "Table writers"): the
//! allocation entry `18e2b0` with its three placements (`18e690` a gap
//! between two blocks, `18e600` below the lowest block, `18e560` above the
//! highest) and the compaction `18e470`.
//!
//! The pool's header (`[4de2ec]`, [`field`]) holds the region (`+28`
//! base, `+2c` size), the free byte count, the lowest block (`+34`), the
//! highest (`+38`) and three flags. A block is `size` bytes (the size
//! includes the header) at an address that has a `0x10` byte header before
//! its body: `+0` the size, `+4` the owner (the address of the pointer
//! that holds the body's address, kept so compaction can rewrite it), `+8`
//! the next block up in memory and `+c` the next one down. A pool with
//! [`field::STAMPED`] set puts a further `0x10` byte stamp (`"head"`, the
//! owner, the tag, the caller's tag) before the header.

use guest_layout::{Fault, Mem, MemMut};

/// Pool header offsets.
pub mod field {
    /// The region's first byte.
    pub const BASE: u32 = 0x28;
    /// The region's size.
    pub const SIZE: u32 = 0x2c;
    /// Free bytes: the size of a block leaves it on allocation and returns on free.
    pub const FREE: u32 = 0x30;
    /// The lowest block.
    pub const LOWEST: u32 = 0x34;
    /// The highest block.
    pub const HIGHEST: u32 = 0x38;
    /// Non-zero: blocks do not move and keep no owner.
    pub const FIXED: u32 = 0x3c;
    /// Non-zero: a new block may fill a gap between two blocks, then the
    /// space below the lowest, before the space above the highest.
    pub const FILL_GAPS: u32 = 0x3d;
    /// Non-zero: each block has a stamp before its header.
    pub const STAMPED: u32 = 0x3f;
}

/// The stamp's first word, `"daeh"` as a little-endian dword (`'head'`).
const STAMP: u32 = 0x6865_6164;
/// The global the stamp's caller word reads through (`3314b0`: `[[411690]]`).
const CALLER_TAG: u32 = 0x0041_1690;

fn rd<M: Mem + ?Sized>(m: &M, at: u32) -> Result<u32, Fault> {
    let mut b = [0u8; 4];
    m.read_bytes(at, &mut b)?;
    Ok(u32::from_le_bytes(b))
}

fn wr<M: MemMut + ?Sized>(m: &mut M, at: u32, v: u32) -> Result<(), Fault> {
    m.write_bytes(at, &v.to_le_bytes())
}

fn flag<M: Mem + ?Sized>(m: &M, pool: u32, off: u32) -> Result<bool, Fault> {
    let mut b = [0u8; 1];
    m.read_bytes(pool.wrapping_add(off), &mut b)?;
    Ok(b[0] != 0)
}

/// Bytes before a block that belong to it besides the block header: the
/// stamp, when the pool has one.
fn stamp_bytes<M: Mem + ?Sized>(m: &M, pool: u32) -> Result<u32, Fault> {
    Ok(if flag(m, pool, field::STAMPED)? { 0x10 } else { 0 })
}

/// Where a block's first byte is, stamp included.
fn start<M: Mem + ?Sized>(m: &M, pool: u32, block: u32) -> Result<u32, Fault> {
    Ok(block.wrapping_sub(stamp_bytes(m, pool)?))
}

/// The stamp at `at`: the marker, the owner, the tag and the caller's word.
fn stamp<M: MemMut + ?Sized>(m: &mut M, at: u32, owner: u32, tag: u32) -> Result<(), Fault> {
    if at == 0 {
        return Ok(());
    }
    wr(m, at, STAMP)?;
    wr(m, at.wrapping_add(4), owner)?;
    wr(m, at.wrapping_add(8), tag)?;
    let caller = rd(m, rd(m, CALLER_TAG)?)?;
    wr(m, at.wrapping_add(0x0c), caller)
}

/// `18e4f0 (eax = at, ebx = below; pool, size, owner, above)`: lay a block
/// of `size` bytes at `at` between `below` and `above` (either may be 0)
/// and link it. Answers the block.
#[allow(clippy::too_many_arguments)]
fn insert<M: MemMut + ?Sized>(
    m: &mut M,
    pool: u32,
    at: u32,
    size: u32,
    owner: u32,
    tag: u32,
    below: u32,
    above: u32,
) -> Result<u32, Fault> {
    let mut block = at;
    if flag(m, pool, field::STAMPED)? {
        block = block.wrapping_add(0x10);
        stamp(m, block.wrapping_sub(0x10), owner, tag)?;
    }
    wr(m, block, size)?;
    wr(m, block.wrapping_add(8), above)?;
    wr(m, block.wrapping_add(0x0c), below)?;
    wr(m, if below == 0 { pool.wrapping_add(field::LOWEST) } else { below.wrapping_add(8) }, block)?;
    wr(m, if above == 0 { pool.wrapping_add(field::HIGHEST) } else { above.wrapping_add(0x0c) }, block)?;
    Ok(block)
}

/// `18e690 (pool; eax = size, owner, tag)`: the first gap, walking up from
/// the lowest block, that holds `size` bytes between a block's end and the
/// next block's start. 0 when none does.
fn fill_gap<M: MemMut + ?Sized>(m: &mut M, pool: u32, size: u32, owner: u32, tag: u32) -> Result<u32, Fault> {
    let mut below = rd(m, pool.wrapping_add(field::LOWEST))?;
    if below == 0 {
        return Ok(0);
    }
    loop {
        let above = rd(m, below.wrapping_add(8))?;
        if above != 0 {
            let top = start(m, pool, above)?;
            let end = rd(m, below)?.wrapping_add(start(m, pool, below)?);
            if end.wrapping_add(size) <= top {
                return insert(m, pool, end, size, owner, tag, below, above);
            }
        }
        below = above;
        if below == 0 {
            return Ok(0);
        }
    }
}

/// `18e600 (edi = pool; size, owner, tag)`: below the lowest block, from
/// the region's base. 0 when it does not fit.
fn place_low<M: MemMut + ?Sized>(m: &mut M, pool: u32, size: u32, owner: u32, tag: u32) -> Result<u32, Fault> {
    let lowest = rd(m, pool.wrapping_add(field::LOWEST))?;
    let limit = if lowest == 0 {
        rd(m, pool.wrapping_add(field::SIZE))?.wrapping_add(rd(m, pool.wrapping_add(field::BASE))?)
    } else {
        start(m, pool, lowest)?
    };
    let mut block = rd(m, pool.wrapping_add(field::BASE))?;
    if size.wrapping_add(block) > limit {
        return Ok(0);
    }
    if flag(m, pool, field::STAMPED)? {
        block = block.wrapping_add(0x10);
        stamp(m, block.wrapping_sub(0x10), owner, tag)?;
    }
    wr(m, block, size)?;
    wr(m, block.wrapping_add(8), lowest)?;
    wr(m, block.wrapping_add(0x0c), 0)?;
    wr(m, pool.wrapping_add(field::LOWEST), block)?;
    wr(m, if lowest == 0 { pool.wrapping_add(field::HIGHEST) } else { lowest.wrapping_add(0x0c) }, block)?;
    Ok(block)
}

/// `18e560 (pool, size, owner, tag)`: above the highest block, to the
/// region's end. 0 when it does not fit.
fn place_high<M: MemMut + ?Sized>(m: &mut M, pool: u32, size: u32, owner: u32, tag: u32) -> Result<u32, Fault> {
    let highest = rd(m, pool.wrapping_add(field::HIGHEST))?;
    let base = rd(m, pool.wrapping_add(field::BASE))?;
    let mut block = if highest == 0 {
        base
    } else {
        rd(m, highest)?.wrapping_add(start(m, pool, highest)?)
    };
    let limit = rd(m, pool.wrapping_add(field::SIZE))?.wrapping_add(base);
    if size.wrapping_add(block) > limit {
        return Ok(0);
    }
    if flag(m, pool, field::STAMPED)? {
        block = block.wrapping_add(0x10);
        stamp(m, block.wrapping_sub(0x10), owner, tag)?;
    }
    wr(m, block, size)?;
    wr(m, block.wrapping_add(8), 0)?;
    wr(m, block.wrapping_add(0x0c), highest)?;
    wr(m, if highest == 0 { pool.wrapping_add(field::LOWEST) } else { highest.wrapping_add(8) }, block)?;
    wr(m, pool.wrapping_add(field::HIGHEST), block)?;
    Ok(block)
}

/// `18e2b0 (eax = tag, ecx = pool; owner, size, _)`: allocate a block for a
/// body of `size` bytes and write its address (past the block header) at
/// `owner`. The block is the body, the header (and stamp) rounded up to 16
/// bytes. The owner is kept in the block unless the pool is fixed; the
/// block's size leaves the free count. `false` when the pool has no room.
pub fn alloc<M: MemMut + ?Sized>(m: &mut M, pool: u32, owner: u32, size: u32, tag: u32) -> Result<bool, Fault> {
    let mut block_size = size.wrapping_add(0x10 + stamp_bytes(m, pool)?);
    if block_size & 0x0f != 0 {
        block_size = (block_size | 0x0f).wrapping_add(1);
    }
    let mut block = 0;
    if flag(m, pool, field::FILL_GAPS)? {
        block = fill_gap(m, pool, block_size, owner, tag)?;
        if block == 0 {
            block = place_low(m, pool, block_size, owner, tag)?;
        }
    }
    if block == 0 {
        block = place_high(m, pool, block_size, owner, tag)?;
    }
    if block == 0 {
        return Ok(false);
    }
    let kept = if flag(m, pool, field::FIXED)? { 0 } else { owner };
    wr(m, block.wrapping_add(4), kept)?;
    let free = rd(m, pool.wrapping_add(field::FREE))?.wrapping_sub(rd(m, block)?);
    wr(m, pool.wrapping_add(field::FREE), free)?;
    wr(m, owner, block.wrapping_add(0x10))?;
    Ok(true)
}

/// `18e470 (edi = pool)`: slide every block down to the region's base in
/// address order, rewriting each moved block's owner to its new body
/// address and the links of the list. A fixed pool and an empty one are
/// left alone. A block already in place is not touched.
pub fn compact<M: MemMut + ?Sized>(m: &mut M, pool: u32) -> Result<(), Fault> {
    let lowest = rd(m, pool.wrapping_add(field::LOWEST))?;
    if flag(m, pool, field::FIXED)? || lowest == 0 {
        return Ok(());
    }
    let (mut at, mut block, mut below) = (rd(m, pool.wrapping_add(field::BASE))?, lowest, 0u32);
    loop {
        let from = start(m, pool, block)?;
        let mut placed = block;
        if from > at {
            let mut bytes = vec![0u8; rd(m, block)? as usize];
            m.read_bytes(from, &mut bytes)?;
            m.write_bytes(at, &bytes)?;
            placed = at.wrapping_add(stamp_bytes(m, pool)?);
            let owner = rd(m, placed.wrapping_add(4))?;
            wr(m, owner, placed.wrapping_add(0x10))?;
        }
        let next = if placed == block { rd(m, block.wrapping_add(8))? } else { rd(m, placed.wrapping_add(8))? };
        wr(m, placed.wrapping_add(0x0c), below)?;
        wr(m, if below == 0 { pool.wrapping_add(field::LOWEST) } else { below.wrapping_add(8) }, placed)?;
        let end = rd(m, placed)?.wrapping_add(if from > at { at } else { from });
        at = end;
        below = placed;
        block = next;
        if block == 0 {
            break;
        }
    }
    wr(m, below.wrapping_add(8), 0)?;
    wr(m, pool.wrapping_add(field::HIGHEST), below)
}

#[cfg(test)]
mod tests;
