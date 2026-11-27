//! Fixup tables: where a faulting inline access goes instead.
//!
//! A generated module emits one row per access instruction, from the same
//! `asm` statement as the instruction, into its own section, and exports
//! the section's bounds. Rows come in one of two encodings, chosen per
//! target by what its object format can express ([`Format`]).
//!
//! The section is the linker's concatenation of every shard's rows, and
//! the linker may fold identical functions: ld64 deduplicates by default
//! whenever the link is not `-O0`, and the folded copy's rows then name
//! the kept copy's instructions, out of place and repeated. So the rows
//! are neither sorted nor unique as linked. Registration copies them,
//! sorts them by instruction and drops repeats; two rows at one
//! instruction with different landings are refused by name
//! ([`Error::Conflict`]).
use super::{Error, READERS, quiesce, register_lock};
use std::ptr;
use std::sync::atomic::{AtomicPtr, Ordering::SeqCst};

/// How many tables may be registered at once.
pub const MAX_TABLES: usize = 64;

/// A row's encoding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum Format {
    /// Two 32-bit words, each relative to its own address: the access
    /// instruction, then its landing label (`.long 1b - .`,
    /// `.long %l[slow] - .`). Needs no load-time relocation.
    Relative32 = 0,
    /// Two 64-bit absolute addresses, rebased by the loader. The fallback
    /// for an object format that cannot subtract across sections.
    Absolute64 = 1,
}

impl Format {
    /// The `uint32_t` a module reports its rows in.
    pub fn from_raw(raw: u32) -> Option<Format> {
        match raw {
            0 => Some(Format::Relative32),
            1 => Some(Format::Absolute64),
            _ => None,
        }
    }
    /// Bytes per row.
    pub const fn row_size(self) -> usize {
        match self {
            Format::Relative32 => 8,
            Format::Absolute64 => 16,
        }
    }
    const fn align(self) -> usize {
        match self {
            Format::Relative32 => 4,
            Format::Absolute64 => 8,
        }
    }
}

struct TableData {
    /// (instruction, landing), sorted by instruction, one row each.
    rows: Box<[(usize, usize)]>,
}

/// Row `i`'s instruction and landing addresses.
fn decode(start: usize, format: Format, i: usize) -> (usize, usize) {
    let at = start + i * format.row_size();
    // SAFETY: `at` is a whole row inside a table its registrant promised
    // is readable.
    unsafe {
        match format {
            Format::Relative32 => {
                let insn = ptr::read_unaligned(at as *const i32);
                let landing = ptr::read_unaligned((at + 4) as *const i32);
                (
                    at.wrapping_add_signed(insn as isize),
                    (at + 4).wrapping_add_signed(landing as isize),
                )
            }
            Format::Absolute64 => {
                let insn = ptr::read_unaligned(at as *const u64);
                let landing = ptr::read_unaligned((at + 8) as *const u64);
                (insn as usize, landing as usize)
            }
        }
    }
}

/// The `rows` rows at `start`, sorted by instruction with repeats
/// dropped. Of the rows naming one instruction, the first in table order
/// sets the landing; the first after it with another landing is a
/// [`Error::Conflict`].
fn collect(start: usize, format: Format, rows: usize) -> Result<Box<[(usize, usize)]>, Error> {
    let mut all: Vec<(usize, usize, usize)> = (0..rows)
        .map(|i| {
            let (insn, landing) = decode(start, format, i);
            (insn, i, landing)
        })
        .collect();
    // Indices are unique, so this order is total and the first row of
    // each instruction comes first.
    all.sort_unstable();
    let mut out: Vec<(usize, usize)> = Vec::with_capacity(all.len());
    let mut first = 0;
    for &(insn, index, landing) in &all {
        match out.last() {
            Some(&(kept, kept_landing)) if kept == insn => {
                if kept_landing != landing {
                    return Err(Error::Conflict { first, second: index });
                }
            }
            _ => {
                out.push((insn, landing));
                first = index;
            }
        }
    }
    Ok(out.into_boxed_slice())
}

static TABLES: [AtomicPtr<TableData>; MAX_TABLES] =
    [const { AtomicPtr::new(ptr::null_mut()) }; MAX_TABLES];

/// A registered fixup table; dropping it unregisters it. An empty table
/// registers nothing.
#[derive(Debug)]
pub struct Table {
    slot: Option<usize>,
}

impl Table {
    /// Rows registered.
    pub fn is_empty(&self) -> bool {
        self.slot.is_none()
    }
}

/// Register the rows in `[start, end)`, in any order: they are copied,
/// sorted by instruction and repeats dropped. Fails with
/// [`Error::Conflict`] when two rows name one instruction with different
/// landings.
///
/// # Safety
/// `[start, end)` must be readable during the call and its rows in
/// `format`; the instructions and landings they name must stay mapped
/// for as long as the returned [`Table`] lives (a loaded module: keep it
/// loaded).
pub unsafe fn register_table(
    start: *const u8,
    end: *const u8,
    format: Format,
) -> Result<Table, Error> {
    let (start, end) = (start as usize, end as usize);
    if end < start || !start.is_multiple_of(format.align()) {
        return Err(Error::BadTable);
    }
    let bytes = end - start;
    if !bytes.is_multiple_of(format.row_size()) {
        return Err(Error::BadTable);
    }
    let rows = bytes / format.row_size();
    if rows == 0 {
        return Ok(Table { slot: None });
    }
    let rows = collect(start, format, rows)?;
    let entry = Box::into_raw(Box::new(TableData { rows }));
    let _guard = register_lock();
    for (slot, s) in TABLES.iter().enumerate() {
        if s.compare_exchange(ptr::null_mut(), entry, SeqCst, SeqCst)
            .is_ok()
        {
            return Ok(Table { slot: Some(slot) });
        }
    }
    // SAFETY: never published.
    drop(unsafe { Box::from_raw(entry) });
    Err(Error::Full)
}

impl Drop for Table {
    fn drop(&mut self) {
        let Some(slot) = self.slot else { return };
        let _guard = register_lock();
        let p = TABLES[slot].swap(ptr::null_mut(), SeqCst);
        quiesce();
        if !p.is_null() {
            // SAFETY: unpublished above and no reader remains.
            drop(unsafe { Box::from_raw(p) });
        }
    }
}

/// The landing label for an access instruction at `pc`, from any
/// registered table. Safe to call in a signal handler: it reads the
/// tables' rows and allocates nothing.
pub fn lookup(pc: usize) -> Option<usize> {
    READERS.fetch_add(1, SeqCst);
    let found = TABLES.iter().find_map(|slot| {
        let p = slot.load(SeqCst);
        if p.is_null() {
            return None;
        }
        // SAFETY: published entries outlive every reader (READERS above).
        let rows = unsafe { &(*p).rows };
        rows.binary_search_by_key(&pc, |r| r.0)
            .ok()
            .map(|i| rows[i].1)
    });
    READERS.fetch_sub(1, SeqCst);
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Relative rows in `buf` naming `pcs` and `landings`, as offsets from
    /// the buffer itself (the addresses are synthetic; nothing runs there).
    fn relative(buf: &mut [i32], pcs: &[usize], landings: &[usize]) {
        let base = buf.as_ptr() as usize;
        for (i, (&pc, &landing)) in pcs.iter().zip(landings).enumerate() {
            let at = base + i * 8;
            buf[2 * i] = (pc as isize - at as isize) as i32;
            buf[2 * i + 1] = (landing as isize - (at + 4) as isize) as i32;
        }
    }

    fn bounds<T>(buf: &[T]) -> (*const u8, *const u8) {
        let range = buf.as_ptr_range();
        (range.start.cast(), range.end.cast())
    }

    /// Two shards linked out of address order, with a function the linker
    /// folded into an earlier shard's copy: its rows repeat the kept
    /// copy's (instruction, landing) pairs after rows above them, as
    /// ld64's deduplication leaves them. Every row is still found.
    #[test]
    fn linked_rows_out_of_order_and_folded_are_found() {
        let mut buf = vec![0i32; 2 * 7];
        let base = buf.as_ptr() as usize;
        let at = |o: usize| base + 0x10_0000 + o;
        let land = |o: usize| base + 0x20_0000 + o;
        // Shard B's text was placed after shard A's but its rows come
        // first; the last two rows are A's folded twin of A's first function.
        let pcs = [at(0x400), at(0x480), at(0x100), at(0x140), at(0x200), at(0x100), at(0x140)];
        let landings = [land(0x40), land(0x48), land(0x10), land(0x14), land(0x20), land(0x10), land(0x14)];
        relative(&mut buf, &pcs, &landings);
        let (start, end) = bounds(&buf);
        // SAFETY: `buf` outlives the table, which is dropped below.
        let table = unsafe { register_table(start, end, Format::Relative32) }.unwrap();
        for (&pc, &landing) in pcs.iter().zip(&landings) {
            assert_eq!(lookup(pc), Some(landing));
            assert_eq!(lookup(pc + 4), None);
        }
        drop(table);
        assert_eq!(lookup(pcs[0]), None);
    }

    /// Two rows at one instruction must agree on its landing; the first
    /// pair that does not is named in table order.
    #[test]
    fn rows_at_one_instruction_with_different_landings_are_refused() {
        let mut buf = vec![0i32; 2 * 5];
        let base = buf.as_ptr() as usize;
        let pcs = [base + 0x300, base + 0x100, base + 0x200, base + 0x100, base + 0x100];
        let landings = [base + 0x1000, base + 0x1100, base + 0x1200, base + 0x1100, base + 0x1180];
        relative(&mut buf, &pcs, &landings);
        let (start, end) = bounds(&buf);
        // SAFETY: `buf` outlives every use; an error registers nothing.
        let err = unsafe { register_table(start, end, Format::Relative32) }.unwrap_err();
        assert_eq!(err, Error::Conflict { first: 1, second: 4 });

        let rows: Vec<u64> = [0x30u64, 1, 0x20, 2, 0x30, 3].to_vec();
        let (start, end) = bounds(&rows);
        // SAFETY: as above.
        let err = unsafe { register_table(start, end, Format::Absolute64) }.unwrap_err();
        assert_eq!(err, Error::Conflict { first: 0, second: 2 });
        assert_eq!(lookup(0x20), None, "a refused table registers nothing");
    }

    #[test]
    fn table_bounds_must_be_whole_rows() {
        let buf = [0u64; 4];
        let (start, end) = bounds(&buf);
        // SAFETY: nothing is registered on error.
        unsafe {
            assert_eq!(
                register_table(end, start, Format::Relative32).unwrap_err(),
                Error::BadTable
            );
            assert_eq!(
                register_table(start, end.sub(4), Format::Absolute64).unwrap_err(),
                Error::BadTable
            );
            assert_eq!(
                register_table(start.add(1), end, Format::Relative32).unwrap_err(),
                Error::BadTable
            );
            assert!(
                register_table(start, start, Format::Relative32)
                    .unwrap()
                    .is_empty()
            );
        }
    }

    #[test]
    fn lookup_finds_each_row_and_nothing_else_until_dropped() {
        // Both encodings, the same synthetic layout, distinct ranges.
        let mut buf = vec![0i32; 2 * 5];
        let base = buf.as_ptr() as usize;
        let pcs: Vec<usize> = (0..5).map(|i| base + 0x10_0000 + i * 0x24).collect();
        let landings: Vec<usize> = (0..5).map(|i| base + 0x20_0000 + i * 0x10).collect();
        relative(&mut buf, &pcs, &landings);
        let (start, end) = bounds(&buf);
        // SAFETY: `buf` outlives the table, which is dropped below.
        let rel = unsafe { register_table(start, end, Format::Relative32) }.unwrap();

        let abs_pcs: Vec<usize> = pcs.iter().map(|p| p + 0x40_0000).collect();
        let abs: Vec<u64> = abs_pcs
            .iter()
            .zip(&landings)
            .flat_map(|(&p, &l)| [p as u64, l as u64 + 1])
            .collect();
        let (start, end) = bounds(&abs);
        // SAFETY: as above.
        let abs_table = unsafe { register_table(start, end, Format::Absolute64) }.unwrap();

        for i in 0..5 {
            assert_eq!(lookup(pcs[i]), Some(landings[i]));
            assert_eq!(lookup(pcs[i] + 1), None);
            assert_eq!(lookup(abs_pcs[i]), Some(landings[i] + 1));
        }
        assert_eq!(lookup(pcs[0] - 4), None);
        drop(rel);
        assert_eq!(lookup(pcs[2]), None);
        assert_eq!(lookup(abs_pcs[2]), Some(landings[2] + 1));
        drop(abs_table);
        assert_eq!(lookup(abs_pcs[2]), None);
    }
}
