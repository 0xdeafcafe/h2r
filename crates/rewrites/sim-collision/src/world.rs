//! The collision world: decoded BSPs keyed by the guest pointer the engine
//! passes, and the counters `collision.stats` reports.
//!
//! Queries look up the guest address in a sorted index and compare the
//! 0x40-byte header. The guest adapter versions the backing bytes. A map,
//! snapshot or source change drops the old generation. Index order only
//! matters to bookkeeping; the tree walk always follows guest node order.
use crate::bsp::{Bsp, Consts, HEADER_WORDS};
use crate::flood::{Topology, TopologyKey};
use crate::mopp::ray::{Outcome, Unverified};
use crate::mopp::{self, Tree};
use crate::structure::{LeafTable, Structure, TableVersion};
use crate::{BAILS, Bail, GuestRead};
use sim_core::Epoch;
use std::collections::{BTreeMap, HashMap};
mod audit;
mod slots;
pub use audit::SourceFingerprint;
use slots::Slots;
#[cfg(test)]
pub(crate) const INDEX_RECORD_BYTES: u64 = slots::RECORD_BYTES;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

thread_local! {
    static LAST_BAIL: std::cell::Cell<Option<(Entry, Bail)>> = const { std::cell::Cell::new(None) };
}

/// The last bail [`CollisionWorld::record`] counted on this thread, and
/// forget it: the hook that ran a body names it in its panic.
pub fn take_bail() -> Option<(Entry, Bail)> {
    LAST_BAIL.take()
}

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
    /// G: `0x0014a5b0`, the sphere cluster flood.
    ClusterFlood,
    /// H: `0x000bb050`, the object spatial visit.
    ObjectVisit,
    /// F: `0x00300150`, the integer MOPP box query.
    MoppBox,
    /// I: `0x00301800`, the float MOPP ray (observed; its walk runs as a
    /// parity arm, [`RayParity`]).
    MoppRay,
    /// The object-reference sphere pretest in J.
    ObjectSphere,
    /// J: the whole collision query.
    Query,
}

pub const ENTRIES: usize = 11;
/// Retained map arrays, BSP owners and address index capacity.
pub const MAP_BUDGET: u64 = 8 << 20;
/// Bounds address-index bookkeeping independently of decoded array size:
/// an empty or intrinsically malformed BSP still occupies a cache slot.
const MAP_SLOTS: usize = 8192;
/// Decoded MOPP programs held at once; a guest naming more bails
/// [`Bail::Budget`] for the rest.
const MOPP_SLOTS: usize = 4096;
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
        Entry::ClusterFlood,
        Entry::ObjectVisit,
        Entry::MoppBox,
        Entry::MoppRay,
        Entry::ObjectSphere,
        Entry::Query,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Entry::PointToLeaf => "point_to_leaf",
            Entry::Location => "location",
            Entry::VectorTest => "vector_test",
            Entry::VectorContext => "vector_context",
            Entry::InstanceTrace => "instance_trace",
            Entry::ClusterFlood => "cluster_flood",
            Entry::ObjectVisit => "object_visit",
            Entry::MoppBox => "mopp_box",
            Entry::MoppRay => "mopp_ray",
            Entry::ObjectSphere => "object_sphere",
            Entry::Query => "query",
        }
    }

    pub fn addr(self) -> u32 {
        match self {
            Entry::PointToLeaf => 0x0014_a280,
            Entry::Location => 0x0011_bed0,
            Entry::VectorTest => 0x001d_e630,
            Entry::VectorContext => 0x001d_e6d0,
            Entry::InstanceTrace => 0x0024_4980,
            Entry::ClusterFlood => 0x0014_a5b0,
            Entry::ObjectVisit => 0x000b_b050,
            Entry::MoppBox => 0x0030_0150,
            Entry::MoppRay => 0x0030_1800,
            Entry::ObjectSphere => 0x0011_e5e0,
            Entry::Query => 0x0016_97c0,
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

/// Entry I's parity arm ([`mopp::ray::RayCheck`]): queries the guest
/// answered and the walk was compared with.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RayParity {
    /// The walk matched the guest call for call, and key for key where
    /// keys were observed.
    pub matched: u64,
    pub mismatched: u64,
    /// Queries the walk could not follow, by [`Bail`] reason.
    pub unverified: [u64; BAILS],
    /// Queries whose fraction moved where no key was observed.
    pub fraction_moved: u64,
    /// The first mismatch since start, described.
    pub first_mismatch: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct VectorParity {
    pub matched: u64,
    pub mismatched: u64,
    pub unverified: [u64; BAILS],
    pub first_mismatch: Option<String>,
    /// Matches that also covered every register and frame byte of a
    /// crossing exit (C and D only).
    pub exits_matched: u64,
    /// Crossing exits answered natively (C and D).
    pub answered: u64,
}

/// A snapshot of the world's counters. Diagnostic only: no kernel reads it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    pub vector: VectorParity,
    pub context: VectorParity,
    pub instance: VectorParity,
    pub query: VectorParity,
    pub visit: VectorParity,
    pub flood: VectorParity,
    pub entries: [EntryStats; ENTRIES],
    /// Entry I's parity arm.
    pub ray: RayParity,
    /// Decode attempts and failures. Cache hits increment neither count.
    pub decodes: u64,
    pub decode_failures: u64,
    /// Times the epoch moved and the world was dropped.
    pub flushes: u64,
    /// Decoded BSPs held now.
    pub bsps: u64,
    /// Decoded MOPP programs held now.
    pub mopps: u64,
    /// Charged resident map data, including the address index and BSP owners.
    pub bytes: u64,
    /// Decoded array allocations, including the packed cluster table and
    /// the MOPP trees.
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
    ray: RayCounters,
    vector: Mutex<VectorParity>,
    context: Mutex<VectorParity>,
    instance: Mutex<VectorParity>,
    query: Mutex<VectorParity>,
    visit: Mutex<VectorParity>,
    flood: Mutex<VectorParity>,
}

#[derive(Default)]
struct RayCounters {
    matched: AtomicU64,
    mismatched: AtomicU64,
    unverified: [AtomicU64; BAILS],
    fraction_moved: AtomicU64,
    first_mismatch: OnceLock<String>,
}

/// A decoded MOPP program and the source revision it was last checked at.
struct MoppSlot {
    tree: Arc<Tree>,
    revision: u64,
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
    /// The active structure's portal graph and the version its provider
    /// witnessed for every byte it was decoded from.
    topology: Option<(TopologyKey, TableVersion, Arc<Topology>)>,
    scalar_rings: bool,
    ring_fallbacks: u64,
    cluster_budget_reads: u64,
    bytes: u64,
    mopps: HashMap<u32, MoppSlot>,
    mopp_bytes: u64,
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

    /// Starts a new generation when the map epoch moved. The memory
    /// revision belongs to the BSP sources and is left alone.
    fn select_epoch(&mut self, epoch: u64) {
        if self.seen != epoch {
            self.flush();
            self.seen = epoch;
        }
    }

    fn array_bytes(&self) -> u64 {
        self.bytes
            + self.mopp_bytes
            + self.structure.as_ref().map_or(0, |s| s.footprint() as u64)
            + self.topology_bytes()
    }

    fn topology_bytes(&self) -> u64 {
        self.topology
            .as_ref()
            .map_or(0, |(_, _, t)| t.footprint() as u64)
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
            self.topology = None;
        }
        let available = limit
            .checked_sub(self.resident() + essential)
            .ok_or(Bail::Budget)?;
        Ok(if self.scalar_rings { 0 } else { available })
    }

    fn cluster_room(&mut self, limit: u64, table: LeafTable) -> bool {
        // One active table, so the new table replaces the old one.
        if self.bytes + self.mopp_bytes + self.metadata_bytes() + table.footprint() > limit {
            self.discard_rings();
        }
        if self.bytes + self.mopp_bytes + self.metadata_bytes() + table.footprint() > limit {
            self.cluster_budget_reads += 1;
            return false;
        }
        true
    }

    fn flush(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.scalar_rings = false;
        if !self.bsps.is_empty()
            || self.structure.is_some()
            || self.topology.is_some()
            || !self.mopps.is_empty()
        {
            self.flushes += 1;
        }
        self.structure = None;
        self.cluster_source = None;
        self.topology = None;
        self.bsps.clear();
        self.built = 0;
        self.bytes = 0;
        self.mopps.clear();
        self.mopp_bytes = 0;
    }
}

pub struct CollisionWorld {
    store: Option<std::path::PathBuf>,
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

    /// Enable shared, immutable BSP columns in the platform cache directory.
    pub fn with_shared_store(epoch: &'static Epoch) -> Self {
        Self {
            store: host_dirs::cache().map(|dir| dir.join("collision")),
            ..Self::new(epoch)
        }
    }

    pub(crate) fn with_budget(epoch: &'static Epoch, budget: u64) -> Self {
        Self::with_limits(epoch, budget, MAP_SLOTS)
    }

    pub(crate) fn with_limits(epoch: &'static Epoch, budget: u64, slot_limit: usize) -> Self {
        Self {
            store: None,
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
        let structure = Structure::decode(g, table, self.store.as_deref()).ok()?;
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
        let built = ring_limit.and_then(|room| {
            Bsp::decode_cached(g, addr, header, &consts()?, room, self.store.as_deref())
        });
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

    /// The portal graph decoded for `key`, when the provider still vouches
    /// for `version` (it saw no write to any source span since).
    pub fn cached_topology(&self, key: TopologyKey, version: TableVersion) -> Option<Arc<Topology>> {
        let epoch = self.epoch.get();
        let mut inner = self.lock();
        let memory = inner.memory;
        inner.select(epoch, memory);
        match &inner.topology {
            Some((k, v, t)) if (*k, *v) == (key, version) => Some(t.clone()),
            _ => None,
        }
    }

    /// Keep a freshly decoded portal graph when the provider could watch
    /// all of its sources and the budget has room; return it either way.
    /// One graph is kept at a time: the active structure's.
    pub fn keep_topology(
        &self,
        version: Option<TableVersion>,
        mut topology: Topology,
    ) -> Arc<Topology> {
        if version.is_some() && topology.footprint() as u64 <= self.budget
            && let Some(directory) = &self.store
        {
            topology.share_columns(directory);
        }
        let topology = Arc::new(topology);
        let epoch = self.epoch.get();
        let mut inner = self.lock();
        let memory = inner.memory;
        inner.select(epoch, memory);
        inner.topology = None;
        if let Some(version) = version
            && inner.resident() + topology.footprint() as u64 <= self.budget
        {
            inner.topology = Some((topology.key(), version, topology.clone()));
        }
        topology
    }

    /// The decoded MOPP program at code object `code`, decoding it the
    /// first time. `revision` is the reader's version of the bytes under
    /// the program (the engine's write watch, or a constant): a held tree
    /// whose header still matches is used at once while it is unchanged,
    /// and compared with the guest's bytes again when it moved. A program
    /// that unfolds past the decode bound is held as `guest_only`.
    pub fn mopp(&self, g: &impl GuestRead, code: u32, revision: u64) -> Result<Arc<Tree>, Bail> {
        let epoch = self.epoch.get();
        let header = mopp::Header::read(g, code)?;
        let (generation, held) = {
            let mut inner = self.lock();
            if self.epoch.get() != epoch {
                return Err(Bail::Data);
            }
            inner.select_epoch(epoch);
            let held = match inner.mopps.get(&code) {
                Some(slot) if slot.tree.source().matches(&header) => {
                    if slot.revision == revision {
                        return Ok(slot.tree.clone());
                    }
                    Some(slot.tree.clone())
                }
                _ => None,
            };
            (inner.generation, held)
        };
        let (_, source) = mopp::Source::read(g, code)?;
        let (tree, decoded) = match held {
            Some(tree) if *tree.source() == source => (tree, false),
            _ => (
                Arc::new(Tree::decode_shared(code, source, self.store.as_deref(), self.budget)),
                true,
            ),
        };
        let mut inner = self.lock();
        if decoded {
            inner.decodes += 1;
            if tree.guest_only() {
                inner.decode_failures += 1;
            }
        }
        if epoch != self.epoch.get() || generation != inner.generation {
            return Err(Bail::Data);
        }
        let size = tree.footprint() as u64;
        let old = inner
            .mopps
            .get(&code)
            .map_or(0, |slot| slot.tree.footprint() as u64);
        if old == 0 && inner.mopps.len() >= MOPP_SLOTS {
            return Err(Bail::Budget);
        }
        if inner.resident() - old + size > self.budget {
            inner.discard_rings();
        }
        if inner.resident() - old + size > self.budget {
            return Err(Bail::Budget);
        }
        inner.mopp_bytes = inner.mopp_bytes - old + size;
        inner.mopps.insert(
            code,
            MoppSlot {
                tree: tree.clone(),
                revision,
            },
        );
        Ok(tree)
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
                LAST_BAIL.set(Some((entry, *why)));
                // The lifted fallback may write through the alias into a
                // cached BSP while its block descriptors remain unchanged.
                if *why == Bail::Alias {
                    self.clear();
                }
                self.counters.bails[e][*why as usize].fetch_add(1, Ordering::Relaxed)
            }
        };
    }

    pub fn record_vector(&self, result: Result<Option<String>, Bail>) {
        Self::record_parity(&self.counters.vector, result);
    }

    pub fn record_context(&self, result: Result<Option<String>, Bail>) {
        Self::record_parity(&self.counters.context, result);
    }

    fn crossing(&self, entry: Entry) -> &Mutex<VectorParity> {
        match entry {
            Entry::VectorTest => &self.counters.vector,
            Entry::VectorContext => &self.counters.context,
            _ => unreachable!("crossing exits are defined only for C and D"),
        }
    }

    /// Count a crossing exit of C or D answered natively: every admitted
    /// crossing call is, from the first.
    pub fn answered_crossing(&self, entry: Entry) {
        self.crossing(entry).lock().unwrap_or_else(|e| e.into_inner()).answered += 1;
    }

    /// Record a lifted return's comparison for C or D. `exit` when it
    /// covered the whole exit state.
    pub fn record_crossing(&self, entry: Entry, result: Result<Option<String>, Bail>, exit: bool) {
        let counter = self.crossing(entry);
        let clean = exit && matches!(result, Ok(None));
        Self::record_parity(counter, result);
        if clean {
            counter.lock().unwrap_or_else(|e| e.into_inner()).exits_matched += 1;
        }
    }

    pub fn record_instance(&self, result: Result<Option<String>, Bail>) {
        Self::record_parity(&self.counters.instance, result);
    }

    pub fn record_query(&self, result: Result<Option<String>, Bail>) {
        Self::record_parity(&self.counters.query, result);
    }

    pub fn record_spatial(&self, entry: Entry, result: Result<Option<String>, Bail>) {
        let counter = match entry {
            Entry::ClusterFlood => &self.counters.flood,
            Entry::ObjectVisit => &self.counters.visit,
            _ => unreachable!("spatial parity is defined only for G and H"),
        };
        Self::record_parity(counter, result);
    }

    fn record_parity(counter: &Mutex<VectorParity>, result: Result<Option<String>, Bail>) {
        let mut v = counter.lock().unwrap_or_else(|e| e.into_inner());
        match result {
            Ok(None) => v.matched += 1,
            Ok(Some(reason)) => {
                v.first_mismatch.get_or_insert(reason);
                v.mismatched += 1;
            }
            Err(why) => v.unverified[why as usize] += 1,
        }
    }

    /// Count one query of entry I's parity arm, keeping the first mismatch.
    pub fn record_ray(&self, outcome: &Outcome) {
        let r = &self.counters.ray;
        match outcome {
            Outcome::Matched => r.matched.fetch_add(1, Ordering::Relaxed),
            Outcome::Unverified(Unverified::Bail(why)) => {
                r.unverified[*why as usize].fetch_add(1, Ordering::Relaxed)
            }
            Outcome::Unverified(Unverified::Fraction) => {
                r.fraction_moved.fetch_add(1, Ordering::Relaxed)
            }
            Outcome::Mismatch { .. } => {
                r.first_mismatch.get_or_init(|| outcome.to_string());
                r.mismatched.fetch_add(1, Ordering::Relaxed)
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
        let ray = RayParity {
            matched: load(&c.ray.matched),
            mismatched: load(&c.ray.mismatched),
            unverified: std::array::from_fn(|b| load(&c.ray.unverified[b])),
            fraction_moved: load(&c.ray.fraction_moved),
            first_mismatch: c.ray.first_mismatch.get().cloned(),
        };
        let inner = self.lock();
        Stats {
            entries,
            ray,
            vector: c.vector.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            context: c.context.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            instance: c.instance.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            query: c.query.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            visit: c.visit.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            flood: c.flood.lock().unwrap_or_else(|e| e.into_inner()).clone(),
            decodes: inner.decodes,
            decode_failures: inner.decode_failures,
            flushes: inner.flushes,
            bsps: inner.built,
            mopps: inner.mopps.len() as u64,
            bytes: inner.resident(),
            array_bytes: inner.array_bytes(),
            budget_bytes: self.budget,
            ring_fallbacks: inner.ring_fallbacks,
            cluster_budget_reads: inner.cluster_budget_reads,
            precision,
        }
    }
}

#[cfg(test)]
mod gate_tests {
    use super::*;
    use sim_core::Epoch;

    #[test]
    fn every_crossing_answer_is_counted_per_entry() {
        static EPOCH: Epoch = Epoch::new();
        let world = CollisionWorld::new(&EPOCH);
        let (c, d) = (Entry::VectorTest, Entry::VectorContext);
        for _ in 0..3 {
            world.answered_crossing(c);
        }
        world.answered_crossing(d);
        assert_eq!((world.stats().vector.answered, world.stats().context.answered), (3, 1));
        // A comparison counts its whole exit only when it covered it.
        world.record_crossing(c, Ok(None), false);
        world.record_crossing(c, Ok(None), true);
        world.record_crossing(c, Ok(Some("ECX".into())), true);
        let v = world.stats().vector;
        assert_eq!((v.matched, v.mismatched, v.exits_matched), (2, 1, 1));
        assert_eq!(v.first_mismatch.as_deref(), Some("ECX"));
    }
}
