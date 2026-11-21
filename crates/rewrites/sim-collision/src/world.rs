//! The collision world: decoded BSPs keyed by the guest pointer the engine
//! passes, and the counters `collision.stats` reports.
//!
//! Queries look up the guest address in a sorted index and compare the
//! 0x40-byte header. The guest adapter versions the backing bytes. A map,
//! snapshot or source change drops the old generation. Index order only
//! matters to bookkeeping; the tree walk always follows guest node order.
use crate::bsp::{Bsp, Consts, HEADER_WORDS};
use crate::structure::{LeafTable, Structure};
use crate::{BAILS, Bail, GuestRead};
use sim_core::Epoch;
use std::collections::BTreeMap;
mod slots;
use slots::Slots;
#[cfg(test)]
pub(crate) const INDEX_RECORD_BYTES: u64 = slots::RECORD_BYTES;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// The guest entries the world monitors (docs/engine/collision-queries.md).
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
    /// E: the instanced-geometry trace (observed, still lifted).
    InstanceTrace,
}

pub const ENTRIES: usize = 5;
/// Retained map arrays, BSP owners and address index capacity.
pub const MAP_BUDGET: u64 = 8 << 20;
/// Bounds address-index bookkeeping independently of decoded array size:
/// an empty or intrinsically malformed BSP still occupies a cache slot.
const MAP_SLOTS: usize = 8192;
/// Covers the BSP owner, its inline header, Rust container descriptors,
/// reference counts and alignment, irrespective of host lock layout.
pub(crate) const BSP_OWNER_BYTES: u64 = 512;
const _: () = assert!(
    (std::mem::size_of::<Bsp>() + 4 * std::mem::size_of::<usize>()) as u64 <= BSP_OWNER_BYTES
);

impl Entry {
    pub const ALL: [Entry; ENTRIES] = [
        Entry::PointToLeaf,
        Entry::Location,
        Entry::VectorTest,
        Entry::VectorContext,
        Entry::InstanceTrace,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Entry::PointToLeaf => "point_to_leaf",
            Entry::Location => "location",
            Entry::VectorTest => "vector_test",
            Entry::VectorContext => "vector_context",
            Entry::InstanceTrace => "instance_trace",
        }
    }

    pub fn addr(self) -> u32 {
        match self {
            Entry::PointToLeaf => 0x0014_a280,
            Entry::Location => 0x0011_bed0,
            Entry::VectorTest => 0x001d_e630,
            Entry::VectorContext => 0x001d_e6d0,
            Entry::InstanceTrace => 0x0024_4980,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EntryStats {
    /// Calls the kernel answered.
    pub native: u64,
    /// Calls left to the lifted code, by [`Bail`] reason.
    pub bails: [u64; BAILS],
    /// Full guest MXCSR and x87 control words, counted separately at entry.
    pub mxcsr: BTreeMap<u32, u64>,
    pub x87_control: BTreeMap<u16, u64>,
}

/// A snapshot of the world's counters. Diagnostic only: no kernel reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub entries: [EntryStats; ENTRIES],
    /// Decode attempts and failures. Cache hits increment neither count.
    pub decodes: u64,
    pub decode_failures: u64,
    /// Times the epoch moved and the world was dropped.
    pub flushes: u64,
    /// Decoded BSPs held now.
    pub bsps: u64,
    /// Charged resident map data, including the address index and BSP owners.
    pub bytes: u64,
    /// Decoded array allocations, including the packed cluster table.
    pub array_bytes: u64,
    pub budget_bytes: u64,
    /// Times a generation crossed its budget and switched to scalar ring walks.
    pub ring_fallbacks: u64,
    /// Cluster lookups left to the live two-byte load due to the budget.
    pub cluster_budget_reads: u64,
    /// x87 precision control seen at the vector-test entries, by the PC
    /// field (single, reserved, double, extended).
    pub precision: [u64; 4],
}

/// The first eight distinct words have dedicated atomic counters. Additional
/// words share a sparse map; even an unusual environment keeps its own count.
#[derive(Default)]
struct WordCounts<W> {
    common: [(OnceLock<W>, AtomicU64); 8],
    other: Mutex<BTreeMap<W, u64>>,
}

impl<W: Copy + Ord> WordCounts<W> {
    fn observe(&self, word: W) {
        for (key, count) in &self.common {
            if *key.get_or_init(|| word) == word {
                count.fetch_add(1, Ordering::Relaxed);
                return;
            }
        }
        let mut other = self.other.lock().unwrap_or_else(|e| e.into_inner());
        *other.entry(word).or_default() += 1;
    }

    fn snapshot(&self) -> BTreeMap<W, u64> {
        let mut words = self.other.lock().unwrap_or_else(|e| e.into_inner()).clone();
        for (key, count) in &self.common {
            if let Some(&word) = key.get() {
                let n = count.load(Ordering::Relaxed);
                if n != 0 {
                    words.insert(word, n);
                }
            }
        }
        words
    }
}

#[derive(Default)]
struct Counters {
    native: [AtomicU64; ENTRIES],
    bails: [[AtomicU64; BAILS]; ENTRIES],
    mxcsr: [WordCounts<u32>; ENTRIES],
    x87_control: [WordCounts<u16>; ENTRIES],
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
    generation: u64,
    memory: (u64, u64),
    bsps: Slots,
    built: u64,
    structure: Option<Structure>,
    cluster_source: Option<LeafTable>,
    scalar_rings: bool,
    ring_fallbacks: u64,
    cluster_budget_reads: u64,
    bytes: u64,
    decodes: u64,
    decode_failures: u64,
    flushes: u64,
}

impl Inner {
    fn select(&mut self, epoch: u64, memory: (u64, u64)) {
        if (self.seen, self.memory) != (epoch, memory) {
            self.flush();
            self.seen = epoch;
            self.memory = memory;
        }
    }

    fn array_bytes(&self) -> u64 {
        self.bytes + self.structure.as_ref().map_or(0, |s| s.footprint() as u64)
    }

    fn metadata_bytes(&self) -> u64 {
        self.bsps.bytes() + self.built * BSP_OWNER_BYTES
    }

    fn resident(&self) -> u64 {
        self.array_bytes() + self.metadata_bytes()
    }

    fn bsp_metadata(&self, addr: u32) -> u64 {
        self.bsps.growth_bytes(addr) + BSP_OWNER_BYTES
    }

    fn remove_bsp(&mut self, addr: u32) {
        if let Some(Slot::Built(b)) = self.bsps.remove(&addr) {
            self.bytes -= b.footprint() as u64;
            self.built -= 1;
        }
    }

    fn discard_rings(&mut self) {
        if self.scalar_rings {
            return;
        }
        self.scalar_rings = true;
        self.ring_fallbacks += 1;
        for slot in self.bsps.values() {
            if let Slot::Built(b) = slot {
                self.bytes -= b.discard_rings() as u64;
            }
        }
    }

    /// Expanded rings have first claim on spare space only. Location can
    /// always read a single cluster word if its compact table must give way.
    fn bsp_room(&mut self, limit: u64, essential: u64) -> Result<u64, Bail> {
        if self.resident() + essential > limit {
            self.discard_rings();
        }
        if self.resident() + essential > limit {
            self.structure = None;
            self.cluster_source = None;
        }
        let available = limit
            .checked_sub(self.resident() + essential)
            .ok_or(Bail::Budget)?;
        Ok(if self.scalar_rings { 0 } else { available })
    }

    fn cluster_room(&mut self, limit: u64, table: LeafTable) -> bool {
        // One active table, so the new table replaces the old one.
        if self.bytes + self.metadata_bytes() + table.footprint() > limit {
            self.discard_rings();
        }
        if self.bytes + self.metadata_bytes() + table.footprint() > limit {
            self.cluster_budget_reads += 1;
            return false;
        }
        true
    }

    fn flush(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.scalar_rings = false;
        if !self.bsps.is_empty() || self.structure.is_some() {
            self.flushes += 1;
        }
        self.structure = None;
        self.cluster_source = None;
        self.bsps.clear();
        self.built = 0;
        self.bytes = 0;
    }
}

pub struct CollisionWorld {
    epoch: &'static Epoch,
    budget: u64,
    slot_limit: usize,
    inner: Mutex<Inner>,
    counters: Counters,
}

impl CollisionWorld {
    pub fn new(epoch: &'static Epoch) -> Self {
        Self::with_budget(epoch, MAP_BUDGET)
    }

    pub(crate) fn with_budget(epoch: &'static Epoch, budget: u64) -> Self {
        Self::with_limits(epoch, budget, MAP_SLOTS)
    }

    pub(crate) fn with_limits(epoch: &'static Epoch, budget: u64, slot_limit: usize) -> Self {
        Self {
            epoch,
            budget,
            slot_limit,
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

    /// The cluster of a leaf in a source whose current contents the caller
    /// witnessed. The table occupies a single slot when the structure switches.
    pub fn leaf_cluster(&self, g: &impl GuestRead, table: LeafTable, leaf: u32) -> Option<i16> {
        let epoch = self.epoch.get();
        let memory = g.memory_revision();
        let generation = {
            let mut inner = self.lock();
            if self.epoch.get() != epoch {
                return None;
            }
            inner.select(epoch, memory);
            if let Some(s) = &inner.structure
                && s.source == table
            {
                return s.leaf_cluster(leaf);
            }
            if inner.cluster_source != Some(table) {
                inner.structure = None;
                inner.cluster_source = Some(table);
            }
            if !inner.cluster_room(self.budget, table) {
                return None;
            }
            inner.generation
        };
        let structure = Structure::decode(g, table).ok()?;
        let cluster = structure.leaf_cluster(leaf)?;
        let mut inner = self.lock();
        if self.epoch.get() != epoch
            || inner.generation != generation
            || inner.cluster_source != Some(table)
            || !inner.cluster_room(self.budget, table)
        {
            return None;
        }
        inner.structure = Some(structure);
        Some(cluster)
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
        let epoch = self.epoch.get();
        let memory = g.memory_revision();
        let header = Bsp::read_header(g, addr)?;
        let (generation, ring_limit) = {
            let mut inner = self.lock();
            if self.epoch.get() != epoch {
                return Err(Bail::Data);
            }
            inner.select(epoch, memory);
            match inner.bsps.get(&addr) {
                Some(Slot::Built(b)) if b.header() == &header => return Ok(b.clone()),
                Some(Slot::Failed { header: h, why }) if *h == header => return Err(*why),
                _ => {}
            }
            if !inner.bsps.contains_key(&addr) && inner.bsps.len() >= self.slot_limit {
                return Err(Bail::Budget);
            }
            inner.remove_bsp(addr);
            let metadata = inner.bsp_metadata(addr);
            let room = Bsp::essential_bytes(&header)
                .and_then(|n| inner.bsp_room(self.budget, n + metadata));
            (inner.generation, room)
        };
        let built = ring_limit
            .and_then(|room| Bsp::decode_with_ring_limit(g, addr, header, &consts()?, room));
        let mut inner = self.lock();
        inner.decodes += 1;
        if epoch != self.epoch.get() || generation != inner.generation {
            inner.decode_failures += 1;
            return Err(Bail::Data);
        }
        if let Some(Slot::Built(b)) = inner.bsps.get(&addr)
            && b.header() == &header
        {
            // Concurrent first uses share the already admitted reservation.
            return Ok(b.clone());
        }
        if !inner.bsps.contains_key(&addr) && inner.bsps.len() >= self.slot_limit {
            inner.decode_failures += 1;
            return Err(Bail::Budget);
        }
        inner.remove_bsp(addr);
        let built = built.and_then(|b| {
            let metadata = inner.bsp_metadata(addr);
            if b.scalar_rings() || inner.resident() + metadata + b.footprint() as u64 > self.budget
            {
                inner.discard_rings();
            }
            if inner.scalar_rings {
                b.discard_rings();
            }
            let size = b.footprint() as u64;
            inner.bsp_room(self.budget, size + metadata)?;
            inner.bytes += size;
            inner.built += 1;
            let b = Arc::new(b);
            inner.bsps.insert(addr, Slot::Built(b.clone()));
            Ok(b)
        });
        if let Err(why) = &built {
            inner.decode_failures += 1;
            // A page can become readable and a later cache mutation can
            // release budget. Only intrinsic invalidity is memoized.
            let intrinsic = *why == Bail::Data
                || (*why == Bail::Budget
                    && Bsp::essential_bytes(&header).is_ok_and(|n| {
                        n + BSP_OWNER_BYTES + Slots::default().growth_bytes(addr) > self.budget
                    }));
            if intrinsic && inner.resident() + inner.bsps.growth_bytes(addr) <= self.budget {
                inner.bsps.insert(addr, Slot::Failed { header, why: *why });
            }
        }
        built
    }

    /// Drop every decoded BSP.
    pub fn clear(&self) {
        let mut inner = self.lock();
        inner.flush();
        inner.seen = self.epoch.get();
    }

    /// Count one call: answered, or left to the guest and why.
    pub fn record<T>(&self, entry: Entry, r: &Result<T, Bail>) {
        let e = entry as usize;
        match r {
            Ok(_) => self.counters.native[e].fetch_add(1, Ordering::Relaxed),
            Err(why) => {
                // The lifted fallback may write through the alias into a
                // cached BSP while its block descriptors remain unchanged.
                if *why == Bail::Alias {
                    self.clear();
                }
                self.counters.bails[e][*why as usize].fetch_add(1, Ordering::Relaxed)
            }
        };
    }

    /// Record both control words at dispatch, before any gate or guest write.
    pub fn observe_environment(&self, entry: Entry, mxcsr: u32, control: u16) {
        self.counters.mxcsr[entry as usize].observe(mxcsr);
        self.counters.x87_control[entry as usize].observe(control);
    }

    pub fn stats(&self) -> Stats {
        let c = &self.counters;
        let load = |a: &AtomicU64| a.load(Ordering::Relaxed);
        let entries: [EntryStats; ENTRIES] = std::array::from_fn(|e| EntryStats {
            native: load(&c.native[e]),
            bails: std::array::from_fn(|b| load(&c.bails[e][b])),
            mxcsr: c.mxcsr[e].snapshot(),
            x87_control: c.x87_control[e].snapshot(),
        });
        let mut precision = [0; 4];
        for e in [Entry::VectorTest, Entry::VectorContext] {
            for (&control, &count) in &entries[e as usize].x87_control {
                precision[usize::from(control >> 8) & 3] += count;
            }
        }
        let inner = self.lock();
        Stats {
            entries,
            decodes: inner.decodes,
            decode_failures: inner.decode_failures,
            flushes: inner.flushes,
            bsps: inner.built,
            bytes: inner.resident(),
            array_bytes: inner.array_bytes(),
            budget_bytes: self.budget,
            ring_fallbacks: inner.ring_fallbacks,
            cluster_budget_reads: inner.cluster_budget_reads,
            precision,
        }
    }
}
