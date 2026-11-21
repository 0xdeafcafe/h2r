//! The collision world: decoded BSPs keyed by the guest pointer the engine
//! passes, and the counters `collision.stats` reports.
//!
//! The map is consulted for lookup only and never iterated, so it cannot
//! affect a result. Each lookup re-reads the BSP's 0x40-byte header and
//! compares it with the one the entry was built from, so a pointer reused
//! for other data rebuilds; the whole world drops when the [`Epoch`] moves
//! (map load, structure BSP switch, snapshot load). Static data never
//! changes a result, only whether it is decoded again, so dropping is
//! always safe.
use crate::bsp::{Bsp, Consts, HEADER_WORDS};
use crate::{BAILS, Bail, GuestRead};
use sim_core::Epoch;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// The guest entries the world answers (docs/engine/collision-queries.md).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(usize)]
pub enum Entry {
    /// A: `0x0014a280`, point to leaf.
    PointToLeaf = 0,
    /// B: `0x0011bed0`, location of a point in the structure.
    Location,
    /// C: `0x001de630`, BSP vector test.
    VectorTest,
    /// D: `0x001de6d0`, BSP vector test, context form.
    VectorContext,
}

pub const ENTRIES: usize = 4;

impl Entry {
    pub const ALL: [Entry; ENTRIES] = [
        Entry::PointToLeaf,
        Entry::Location,
        Entry::VectorTest,
        Entry::VectorContext,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Entry::PointToLeaf => "point_to_leaf",
            Entry::Location => "location",
            Entry::VectorTest => "vector_test",
            Entry::VectorContext => "vector_context",
        }
    }

    pub fn addr(self) -> u32 {
        match self {
            Entry::PointToLeaf => 0x0014_a280,
            Entry::Location => 0x0011_bed0,
            Entry::VectorTest => 0x001d_e630,
            Entry::VectorContext => 0x001d_e6d0,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EntryStats {
    /// Calls the kernel answered.
    pub native: u64,
    /// Calls left to the lifted code, by [`Bail`] reason.
    pub bails: [u64; BAILS],
}

/// A snapshot of the world's counters. Diagnostic only: no kernel reads it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub entries: [EntryStats; ENTRIES],
    /// BSPs decoded, and those that could not be (left to the guest).
    pub decodes: u64,
    pub decode_failures: u64,
    /// Times the epoch moved and the world was dropped.
    pub flushes: u64,
    /// Decoded BSPs held now, and their bytes.
    pub bsps: u64,
    pub bytes: u64,
    /// x87 precision control seen at the vector-test entries, by the PC
    /// field (single, reserved, double, extended).
    pub precision: [u64; 4],
}

#[derive(Default)]
struct Counters {
    native: [AtomicU64; ENTRIES],
    bails: [[AtomicU64; BAILS]; ENTRIES],
    precision: [AtomicU64; 4],
}

enum Slot {
    Built(Arc<Bsp>),
    Failed {
        header: [u32; HEADER_WORDS],
        why: Bail,
    },
}

#[derive(Default)]
struct Inner {
    seen: u64,
    bsps: HashMap<u32, Slot>,
    bytes: u64,
    decodes: u64,
    decode_failures: u64,
    flushes: u64,
}

impl Inner {
    fn flush(&mut self) {
        if !self.bsps.is_empty() {
            self.flushes += 1;
        }
        self.bsps.clear();
        self.bytes = 0;
    }
}

pub struct CollisionWorld {
    epoch: &'static Epoch,
    inner: Mutex<Inner>,
    counters: Counters,
}

impl CollisionWorld {
    pub fn new(epoch: &'static Epoch) -> Self {
        Self {
            epoch,
            inner: Mutex::new(Inner {
                seen: epoch.get(),
                ..Inner::default()
            }),
            counters: Counters::default(),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The decoded BSP the guest names by `addr` (the pointer to its eight
    /// blocks), decoding it the first time. `consts` is read only when a
    /// decode is needed.
    pub fn bsp(
        &self,
        g: &impl GuestRead,
        addr: u32,
        consts: impl FnOnce() -> Result<Consts, Bail>,
    ) -> Result<Arc<Bsp>, Bail> {
        let header = Bsp::read_header(g, addr)?;
        let epoch = self.epoch.get();
        {
            let mut inner = self.lock();
            if inner.seen != epoch {
                inner.flush();
                inner.seen = epoch;
            }
            match inner.bsps.get(&addr) {
                Some(Slot::Built(b)) if b.header() == &header => return Ok(b.clone()),
                Some(Slot::Failed { header: h, why }) if *h == header => return Err(*why),
                _ => {}
            }
        }
        let built = consts().and_then(|c| Bsp::decode(g, addr, header, &c));
        let mut inner = self.lock();
        inner.decodes += 1;
        let current = inner.seen == self.epoch.get();
        match built {
            Ok(b) => {
                let b = Arc::new(b);
                if current {
                    inner.bytes += b.footprint() as u64;
                    if let Some(Slot::Built(old)) = inner.bsps.insert(addr, Slot::Built(b.clone()))
                    {
                        inner.bytes -= old.footprint() as u64;
                    }
                }
                Ok(b)
            }
            Err(why) => {
                inner.decode_failures += 1;
                // A fault may be a page not mapped yet; only data the
                // decoder rejects is remembered.
                if current && why != Bail::Fault
                    && let Some(Slot::Built(old)) =
                        inner.bsps.insert(addr, Slot::Failed { header, why })
                    {
                        inner.bytes -= old.footprint() as u64;
                    }
                Err(why)
            }
        }
    }

    /// Drop every decoded BSP.
    pub fn clear(&self) {
        self.lock().flush();
    }

    /// Count one call: answered, or left to the guest and why.
    pub fn record<T>(&self, entry: Entry, r: &Result<T, Bail>) {
        let e = entry as usize;
        match r {
            Ok(_) => self.counters.native[e].fetch_add(1, Ordering::Relaxed),
            Err(why) => self.counters.bails[e][*why as usize].fetch_add(1, Ordering::Relaxed),
        };
    }

    /// Count the x87 precision control (`control >> 8 & 3`) seen at entry.
    pub fn record_precision(&self, control: u16) {
        self.counters.precision[usize::from(control >> 8) & 3].fetch_add(1, Ordering::Relaxed);
    }

    pub fn stats(&self) -> Stats {
        let c = &self.counters;
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let inner = self.lock();
        Stats {
            entries: std::array::from_fn(|e| EntryStats {
                native: load(&c.native[e]),
                bails: std::array::from_fn(|b| load(&c.bails[e][b])),
            }),
            decodes: inner.decodes,
            decode_failures: inner.decode_failures,
            flushes: inner.flushes,
            bsps: inner
                .bsps
                .values()
                .filter(|s| matches!(s, Slot::Built(_)))
                .count() as u64,
            bytes: inner.bytes,
            precision: std::array::from_fn(|i| load(&c.precision[i])),
        }
    }
}
