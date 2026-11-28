//! HaloScript's checkpointed thread and global datum arrays. Retail allocates
//! 0x51e74 and 0x61cc bytes respectively at 0x208ff0, before loading a map.
use guest_layout::datum::{self, layout};
use guest_layout::{GuestMemory, MemFault, points};

pub const EXTERNALS: u32 = 0x41d;
pub const THREADS: u32 = 0x004f_9384;
pub const GLOBALS: u32 = 0x004f_9380;
const THREAD_COUNT: u32 = 320;
const RETAIL_CELLS: u32 = 3072;
const THREAD: u32 = 0x418;
const CELL: u32 = 8;

const fn bitmap(n: u32) -> u32 {
    n.div_ceil(32) * 4
}

const THREAD_STORAGE: u32 = THREAD_COUNT * THREAD + bitmap(THREAD_COUNT);

/// The largest thread count compatible with the scenario's global definitions.
/// A large global table lives in the unused suffix of the thread allocation.
/// The two headers keep their original addresses, even across a checkpoint load.
pub fn capacities(definitions: u32) -> Option<(u32, u32)> {
    if definitions > 0x7fff {
        return None;
    }
    let cells = EXTERNALS + definitions;
    if cells <= RETAIL_CELLS {
        return Some((THREAD_COUNT, RETAIL_CELLS));
    }
    let available = THREAD_STORAGE.checked_sub(cells * CELL)?;
    // The cell bitmap fits in the original cell allocation, even at the
    // largest script index. Only the thread bitmap uses bytes in this suffix.
    let threads = (1..=THREAD_COUNT).rev().find(|n| n * THREAD + bitmap(*n) <= available)?;
    Some((threads, cells))
}

fn invalid(addr: u32) -> MemFault {
    MemFault { addr, len: 4, write: false }
}

struct Storage {
    thread_bitmap: u32,
    cell_data: u32,
    cell_bitmap: u32,
}

fn storage(threads: u32, globals: u32, nt: u32, ng: u32) -> Storage {
    let thread_bitmap = threads + layout::HEADER_SIZE + nt * THREAD;
    let (cell_data, cell_bitmap) = if ng == RETAIL_CELLS {
        let data = globals + layout::HEADER_SIZE;
        (data, data + RETAIL_CELLS * CELL)
    } else {
        (thread_bitmap + bitmap(nt), globals + layout::HEADER_SIZE)
    };
    Storage { thread_bitmap, cell_data, cell_bitmap }
}

/// Before retail's 0x2090a0 clears the threads and creates scenario globals.
/// Old-map disposal at 0x209290 deletes all globals except the external prefix.
pub fn prepare(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let sc = m.u32(points::SCENARIO.base)?;
    if sc == 0 {
        return Ok(());
    }
    let (nt, ng) = capacities(m.u32(sc + 0x1c0)?).ok_or_else(|| invalid(sc))?;
    let threads = m.u32(THREADS)?;
    let cells = m.u32(GLOBALS)?;
    if threads == 0 || cells == 0 || threads.checked_add(0x51e74).is_none()
        || cells.checked_add(0x61cc).is_none()
    {
        return Err(invalid(GLOBALS));
    }
    let old_nt = m.u32(threads + layout::MAXIMUM_COUNT)?;
    let old_ng = m.u32(cells + layout::MAXIMUM_COUNT)?;
    if (nt, ng) == (old_nt, old_ng) {
        return Ok(());
    }
    if old_ng < RETAIL_CELLS || capacities(old_ng - EXTERNALS) != Some((old_nt, old_ng)) {
        return Err(invalid(cells));
    }
    let old = storage(threads, cells, old_nt, old_ng);
    if m.u32(threads + layout::SIGNATURE)? != datum::SIGNATURE
        || m.u32(cells + layout::SIGNATURE)? != datum::SIGNATURE
        || m.u32(threads + layout::ELEMENT_SIZE)? != THREAD
        || m.u32(cells + layout::ELEMENT_SIZE)? != CELL
        || m.u32(threads + layout::DATA)? != threads + layout::HEADER_SIZE
        || m.u32(threads + layout::BITMAP)? != old.thread_bitmap
        || m.u32(cells + layout::DATA)? != old.cell_data
        || m.u32(cells + layout::BITMAP)? != old.cell_bitmap
        || m.u32(cells + layout::HIGH_WATER)? != EXTERNALS
        || m.u32(cells + layout::ACTIVE_COUNT)? != EXTERNALS
    {
        return Err(invalid(cells));
    }

    // Save values and salts before touching either allocation. The destination
    // may overlap the old external cells or their bitmap.
    let mut external = vec![0; (EXTERNALS * CELL) as usize];
    m.read(old.cell_data, &mut external)?;
    let next = storage(threads, cells, nt, ng);
    m.write(next.cell_data, &external)?;
    m.fill(next.cell_data + EXTERNALS * CELL, (ng - EXTERNALS) * CELL, 0)?;
    m.fill(next.cell_bitmap, bitmap(ng), 0)?;
    m.fill(next.cell_bitmap, EXTERNALS / 8, 0xff)?;
    m.set_u8(next.cell_bitmap + EXTERNALS / 8, (1 << (EXTERNALS % 8)) - 1)?;
    m.set_u32(cells + layout::FREE_SCAN_START, EXTERNALS)?;
    m.set_u32(cells + layout::DATA, next.cell_data)?;
    m.set_u32(cells + layout::BITMAP, next.cell_bitmap)?;
    m.set_u32(cells + layout::MAXIMUM_COUNT, ng)?;
    m.set_u32(threads + layout::BITMAP, next.thread_bitmap)?;
    m.set_u32(threads + layout::MAXIMUM_COUNT, nt)
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::datum::DatumArray;
    use guest_layout::memory::FlatMemory;

    #[test]
    fn expansion_keeps_external_handles_and_survives_thread_initialization() {
        let mut m = FlatMemory::new(0x400000, 0x180000);
        let (thread, cell, sc) = (0x480000, 0x530000, 0x540000);
        let threads = DatumArray::at(thread);
        let globals = DatumArray::at(cell);
        for (pool, count, stride) in [(threads, THREAD_COUNT, THREAD), (globals, RETAIL_CELLS, CELL)] {
            pool.initialize_contiguous(&mut m, 0x400100, count as i32, stride as i32, 0, 7).unwrap();
            pool.make_valid(&mut m).unwrap();
        }
        m.set_u32(THREADS, thread).unwrap();
        m.set_u32(GLOBALS, cell).unwrap();
        m.set_u32(points::SCENARIO.base, sc).unwrap();
        let mut external = Vec::new();
        for i in 0..EXTERNALS {
            let h = globals.new_datum(&mut m).unwrap().unwrap();
            let at = globals.try_get(&m, h).unwrap().unwrap();
            m.set_u32(at + 4, i * 31).unwrap();
            external.push(h);
        }
        let thread_end = thread + layout::HEADER_SIZE + THREAD_STORAGE;
        let cell_end = cell + layout::HEADER_SIZE + RETAIL_CELLS * CELL + bitmap(RETAIL_CELLS);
        for end in [thread_end, cell_end] {
            m.set_u32(end, 0xa11ce).unwrap();
        }
        for count in [4300, 4900, 80] {
            m.set_u32(sc + 0x1c0, count).unwrap();
            prepare(&mut m).unwrap();
            let (nt, ng) = capacities(count).unwrap();
            assert_eq!(m.u32(thread + layout::MAXIMUM_COUNT).unwrap(), nt);
            assert_eq!(m.u32(cell + layout::MAXIMUM_COUNT).unwrap(), ng);
            // Retail clears all thread slots immediately after the hook.
            threads.make_valid(&mut m).unwrap();
            for (i, h) in external.iter().enumerate() {
                let at = globals.try_get(&m, *h).unwrap().unwrap();
                assert_eq!(m.u32(at + 4).unwrap(), i as u32 * 31);
            }
            let first = globals.new_datum(&mut m).unwrap().unwrap();
            assert_eq!(u32::from(first.index()), EXTERNALS);
            let last = globals.new_datum_at_absolute(&mut m, ng as i32 - 1).unwrap().unwrap();
            let tail = globals.try_get(&m, last).unwrap().unwrap();
            m.set_u32(tail + 4, 0xfedc_ba98).unwrap();
            threads.make_valid(&mut m).unwrap();
            assert_eq!(globals.try_get(&m, last).unwrap(), Some(tail));
            assert_eq!(m.u32(tail + 4).unwrap(), 0xfedc_ba98);
            assert!(globals.new_datum_at_absolute(&mut m, ng as i32).unwrap().is_none());
            globals.delete(&mut m, last).unwrap();
            globals.delete(&mut m, first).unwrap();
            for end in [thread_end, cell_end] {
                assert_eq!(m.u32(end).unwrap(), 0xa11ce);
            }
        }
        assert_eq!(m.u32(cell + layout::DATA).unwrap(), cell + layout::HEADER_SIZE);
    }
}
