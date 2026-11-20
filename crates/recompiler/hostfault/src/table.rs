//! Fixup tables: where a faulting inline access goes instead.
//!
//! A generated module emits one row per access instruction, from the same
//! `asm` statement as the instruction, into its own section, and exports
//! the section's bounds. Rows come in one of two encodings, chosen per
//! target by what its object format can express ([`Format`]).
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
    start: usize,
    rows: usize,
    format: Format,
    /// The first and last rows' instruction addresses.
    lo: usize,
    hi: usize,
}

impl TableData {
    /// Row `i`'s instruction and landing addresses.
    fn row(&self, i: usize) -> (usize, usize) {
        decode(self.start, self.format, i)
    }
}

fn decode(start: usize, format: Format, i: usize) -> (usize, usize) {
    let at = start + i * format.row_size();
    // SAFETY: `at` is a whole row inside a table its registrant promised
    // stays mapped and unchanged while registered.
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

/// Register the rows in `[start, end)`. Fails with [`Error::Unsorted`] at
/// the first row whose instruction address is not above the one before.
///
/// # Safety
/// `[start, end)` must be readable and unchanged for as long as the
/// returned [`Table`] lives (a loaded module's section: keep the module
/// loaded), and the rows must be in `format`.
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
    let mut previous = decode(start, format, 0).0;
    for index in 1..rows {
        let insn = decode(start, format, index).0;
        if insn <= previous {
            return Err(Error::Unsorted { index });
        }
        previous = insn;
    }
    let entry = Box::into_raw(Box::new(TableData {
        start,
        rows,
        format,
        lo: decode(start, format, 0).0,
        hi: previous,
    }));
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
/// registered table. Safe to call in a signal handler.
pub fn lookup(pc: usize) -> Option<usize> {
    READERS.fetch_add(1, SeqCst);
    let found = TABLES.iter().find_map(|slot| {
        let p = slot.load(SeqCst);
        if p.is_null() {
            return None;
        }
        // SAFETY: published entries outlive every reader (READERS above).
        let t = unsafe { &*p };
        if pc < t.lo || pc > t.hi {
            return None;
        }
        let (mut lo, mut hi) = (0, t.rows);
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            let (insn, landing) = t.row(mid);
            if insn == pc {
                return Some(landing);
            }
            if insn < pc {
                lo = mid + 1;
            } else {
                hi = mid;
            }
        }
        None
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

    #[test]
    fn table_must_be_sorted() {
        let mut buf = vec![0i32; 8];
        let base = buf.as_ptr() as usize;
        let pcs = [base + 0x100, base + 0x200, base + 0x180, base + 0x300];
        let landings = [base + 0x1000; 4];
        relative(&mut buf, &pcs, &landings);
        let (start, end) = bounds(&buf);
        // SAFETY: `buf` outlives every use; an error registers nothing.
        let err = unsafe { register_table(start, end, Format::Relative32) }.unwrap_err();
        assert_eq!(err, Error::Unsorted { index: 2 });

        // A repeated instruction is not sorted either.
        let pcs = [base + 0x100, base + 0x200, base + 0x200, base + 0x300];
        relative(&mut buf, &pcs, &landings);
        // SAFETY: as above.
        let err = unsafe { register_table(start, end, Format::Relative32) }.unwrap_err();
        assert_eq!(err, Error::Unsorted { index: 2 });

        let rows: Vec<u64> = [0x30u64, 1, 0x20, 2].to_vec();
        let (start, end) = bounds(&rows);
        // SAFETY: as above.
        let err = unsafe { register_table(start, end, Format::Absolute64) }.unwrap_err();
        assert_eq!(err, Error::Unsorted { index: 1 });
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
