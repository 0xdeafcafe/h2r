//! The slow-access census: the guest pages generated code reaches the
//! checked helpers for (`h2abi_v1_read*`, `h2abi_v1_write*`), and what the
//! helper saw when it was called.
//!
//! An inline access that cannot be made in place (h2fastmem.h's
//! `h2_access_cold`) retries under the gate and goes to the helper only
//! when it still cannot: the gate is held by another thread (`gate_busy`),
//! the context lent no RAM view (`no_view`: fast memory off, or a flat-RAM
//! runtime), the span crosses its page (`crossings`), or the page's fast
//! entry refuses the access (the rest; [`crate::Guest::page_view`] says why).
//!
//! Off until armed ([`arm`]): the agent's `profile.memory.pages {census}`
//! (docs/runtime/agent-api-render.md, "Flat RAM window"). Off, a helper
//! call pays one relaxed load. Armed, one table per process, made on first
//! use (five words per guest page, 20 MiB) and kept for reuse.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering::Relaxed};

const PAGES: usize = 1 << 20;

struct Table {
    reads: Box<[AtomicU32]>,
    writes: Box<[AtomicU32]>,
    crossings: Box<[AtomicU32]>,
    gate_busy: Box<[AtomicU32]>,
    no_view: Box<[AtomicU32]>,
    /// The guest pc of the page's last helper call.
    pc: Box<[AtomicU32]>,
    reads_total: AtomicU64,
    writes_total: AtomicU64,
}

fn words() -> Box<[AtomicU32]> {
    (0..PAGES).map(|_| AtomicU32::new(0)).collect()
}

impl Table {
    fn new() -> Self {
        Self {
            reads: words(),
            writes: words(),
            crossings: words(),
            gate_busy: words(),
            no_view: words(),
            pc: words(),
            reads_total: AtomicU64::new(0),
            writes_total: AtomicU64::new(0),
        }
    }
    fn clear(&self) {
        for column in [
            &self.reads,
            &self.writes,
            &self.crossings,
            &self.gate_busy,
            &self.no_view,
            &self.pc,
        ] {
            for w in column.iter() {
                w.store(0, Relaxed);
            }
        }
        self.reads_total.store(0, Relaxed);
        self.writes_total.store(0, Relaxed);
    }
}

static TABLE: OnceLock<Table> = OnceLock::new();
static ARMED: AtomicBool = AtomicBool::new(false);

/// Start counting from zero (`true`), or stop and keep the counts.
pub fn arm(on: bool) {
    if on {
        ARMED.store(false, Relaxed);
        TABLE.get_or_init(Table::new).clear();
    }
    ARMED.store(on, Relaxed);
}

/// The census is counting.
pub fn armed() -> bool {
    ARMED.load(Relaxed)
}

/// What the helper saw as it was called.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Seen {
    /// Another thread held the inline-memory gate.
    pub gate_busy: bool,
    /// The calling context had no RAM view to try.
    pub no_view: bool,
}

/// One helper call from generated code at `pc` for `width` bytes at `addr`.
#[inline]
pub(crate) fn note(pc: u32, addr: u32, width: u32, write: bool, seen: impl FnOnce() -> Seen) {
    if ARMED.load(Relaxed) {
        note_armed(pc, addr, width, write, seen());
    }
}

#[cold]
#[inline(never)]
fn note_armed(pc: u32, addr: u32, width: u32, write: bool, seen: Seen) {
    let Some(t) = TABLE.get() else {
        return;
    };
    let p = (addr >> 12) as usize;
    if write {
        t.writes[p].fetch_add(1, Relaxed);
        t.writes_total.fetch_add(1, Relaxed);
    } else {
        t.reads[p].fetch_add(1, Relaxed);
        t.reads_total.fetch_add(1, Relaxed);
    }
    if (addr & 4095) as u64 + width as u64 > 4096 {
        t.crossings[p].fetch_add(1, Relaxed);
    }
    if seen.gate_busy {
        t.gate_busy[p].fetch_add(1, Relaxed);
    }
    if seen.no_view {
        t.no_view[p].fetch_add(1, Relaxed);
    }
    t.pc[p].store(pc, Relaxed);
}

/// One page's helper calls since the census was armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SlowPage {
    /// The page's guest address.
    pub va: u32,
    pub reads: u32,
    pub writes: u32,
    /// Of those, spans that cross into the next page.
    pub crossings: u32,
    /// Of those, calls made while another thread held the gate.
    pub gate_busy: u32,
    /// Of those, calls from a context with no RAM view.
    pub no_view: u32,
    /// The guest pc of the last call.
    pub pc: u32,
}

/// What [`snapshot`] reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Census {
    pub armed: bool,
    pub reads: u64,
    pub writes: u64,
    /// Distinct pages the helper was called for.
    pub pages: usize,
    /// The `top` pages by calls, most first.
    pub top: Vec<SlowPage>,
}

/// The counts so far, with the `top` pages by calls.
pub fn snapshot(top: usize) -> Census {
    let Some(t) = TABLE.get() else {
        return Census {
            armed: armed(),
            ..Census::default()
        };
    };
    let mut pages: Vec<SlowPage> = (0..PAGES)
        .filter_map(|p| {
            let (reads, writes) = (t.reads[p].load(Relaxed), t.writes[p].load(Relaxed));
            (reads != 0 || writes != 0).then(|| SlowPage {
                va: (p as u32) << 12,
                reads,
                writes,
                crossings: t.crossings[p].load(Relaxed),
                gate_busy: t.gate_busy[p].load(Relaxed),
                no_view: t.no_view[p].load(Relaxed),
                pc: t.pc[p].load(Relaxed),
            })
        })
        .collect();
    let distinct = pages.len();
    pages.sort_by_key(|p| (std::cmp::Reverse(p.reads as u64 + p.writes as u64), p.va));
    pages.truncate(top);
    Census {
        armed: armed(),
        reads: t.reads_total.load(Relaxed),
        writes: t.writes_total.load(Relaxed),
        pages: distinct,
        top: pages,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Page `va`'s counts, whatever other tests' helper calls add elsewhere.
    fn page(va: u32) -> SlowPage {
        let t = TABLE.get().unwrap();
        let p = (va >> 12) as usize;
        SlowPage {
            va: va & !0xfff,
            reads: t.reads[p].load(Relaxed),
            writes: t.writes[p].load(Relaxed),
            crossings: t.crossings[p].load(Relaxed),
            gate_busy: t.gate_busy[p].load(Relaxed),
            no_view: t.no_view[p].load(Relaxed),
            pc: t.pc[p].load(Relaxed),
        }
    }

    /// The only test that arms the census: it is process-wide. Its pages
    /// are ones no other test maps.
    #[test]
    fn counts_helper_calls_by_page_only_while_armed() {
        const A: u32 = 0xfe71_0000;
        const B: u32 = 0xfe72_0000;
        let busy = || Seen {
            gate_busy: true,
            no_view: false,
        };
        note(0x1234, A + 0x10, 4, false, busy);
        arm(true);
        assert_eq!(page(A).reads, 0, "nothing counted before arming");
        note(0x1111, A + 0x10, 4, false, Seen::default);
        note(0x2222, A + 0xffe, 4, true, busy);
        note(0x3333, B, 8, false, || Seen {
            gate_busy: false,
            no_view: true,
        });
        assert_eq!(
            page(A),
            SlowPage {
                va: A,
                reads: 1,
                writes: 1,
                crossings: 1,
                gate_busy: 1,
                no_view: 0,
                pc: 0x2222,
            }
        );
        assert_eq!((page(B).reads, page(B).no_view), (1, 1));
        let c = snapshot(500);
        assert!(c.armed && c.reads >= 2 && c.writes >= 1 && c.pages >= 2);
        assert!(c.top.windows(2).all(|w| w[0].reads + w[0].writes >= w[1].reads + w[1].writes));
        arm(false);
        note(0x4444, B, 4, false, Seen::default);
        assert!(!snapshot(1).armed);
        assert_eq!(page(B).reads, 1, "nothing counted once stopped");
        // Arming again starts from zero.
        arm(true);
        assert_eq!(page(A).reads, 0);
        arm(false);
    }
}
