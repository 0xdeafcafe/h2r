//! GPU vertex buffers that outlive a submission.
//!
//! A vertex stream a caller marks with an id ([`Draw::stream_ids`]) holds
//! bytes that never change under that id (the guest device's vertex cache
//! hands out a new id whenever it re-reads a range). The first draw of an
//! id writes its host buffer (raw, realigned or converted, as the stream's
//! [`StreamPlan`] says) into a region of its own; later draws of the same
//! id with the same plan bind that region, with nothing staged in the
//! upload ring. Regions are dropped least recently used first past a byte
//! budget; draws already recorded keep their buffers alive (wgpu buffers
//! are reference counted).
//!
//! Index lists marked with an id ([`Draw::index_id`]) are kept the same
//! way, as the host list (Xbox-only primitives rewritten) per id and
//! primitive type.
//!
//! Each id takes a region of a power-of-two size class. By default the
//! regions are carved out of a few large slabs ([`SLAB`] bytes, a buddy
//! allocator): wgpu-core sizes every render pass's usage scope and every
//! command buffer's tracker to the most buffers the device has held at
//! once, so a buffer per id made every pass pay for each snapshot alive
//! (`tests/pass_cost.rs`). With [`PersistentVertices::set_slabs`] off, each
//! region is a buffer of its own, as before.
//!
//! A dropped region (evicted or replaced) waits until the owner reports a
//! submit ([`PersistentVertices::submitted`]: every draw recorded with it
//! is then on the queue), then goes back to be taken by the next new id. A
//! queued write lands after the work already submitted, so a reused region
//! never changes under a draw that still reads it. Free buffers of their
//! own are held up to half the budget (an eviction frees a quarter at
//! once); slabs that come back empty past [`SPARE_SLABS`] are dropped.
//!
//! Staged writes ([`PersistentVertices::set_staged`], what the renderer
//! uses): instead of a `queue.write_buffer` per new region (a staging
//! buffer wgpu makes and frees, about 20 us each), the bytes go into an
//! upload ring that is written once per flush, and a buffer-to-buffer copy
//! per new region is encoded into a command buffer of its own
//! ([`PersistentVertices::copy_commands`]) that the owner submits ahead of
//! the work that draws from them. Same bytes in the same regions.
//!
//! [`Draw::stream_ids`]: crate::renderer::Draw::stream_ids
//! [`Draw::index_id`]: crate::renderer::Draw::index_id

use std::collections::{BTreeSet, HashMap};

use crate::context::GfxContext;
use crate::repack::StreamPlan;
use crate::ring::UploadRing;
use crate::vertex::PrimitiveType;

/// Default GPU budget (`H2_VERTEX_CACHE_MB` overrides it).
pub const DEFAULT_BUDGET: u64 = 128 << 20;

/// Slab size: regions up to this size are carved out of slabs.
pub const SLAB: u64 = 8 << 20;

/// Empty slabs kept for reuse; more are dropped.
pub const SPARE_SLABS: usize = 2;

/// Where an id's bytes live: `size` bytes (its class) at `offset` in
/// `buffer`, part of slab `slab` or (`None`) a buffer of its own.
struct Region {
    buffer: wgpu::Buffer,
    offset: u64,
    size: u64,
    slab: Option<usize>,
    index: bool,
}

struct Slot {
    plan: StreamPlan,
    vertex_count: u32,
    region: Region,
    used: u64,
}

struct IndexSlot {
    region: Region,
    /// Bytes bound.
    bound: u64,
    /// Indices in the host list.
    count: u32,
    used: u64,
}

#[derive(Clone, Copy)]
enum Key {
    Vertices(u64),
    Indices(u64, PrimitiveType),
}

/// Counters, for tests and diagnostics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersistentStats {
    /// Streams bound from an existing buffer.
    pub hits: u64,
    /// Buffers written.
    pub uploads: u64,
    /// Bytes written into them.
    pub uploaded_bytes: u64,
    /// Bytes held now.
    pub held_bytes: u64,
    /// Index lists bound from an existing buffer.
    pub index_hits: u64,
    /// Index buffers written.
    pub index_uploads: u64,
}

/// Smallest size class.
const MIN_CLASS: u64 = 256;

/// Free buffers of their own by (size, index usage), and the slabs.
#[derive(Default)]
struct Pool {
    free: HashMap<(u64, bool), Vec<wgpu::Buffer>>,
    free_bytes: u64,
    /// Dropped since the last submit.
    retired: Vec<Region>,
    /// [`PersistentVertices::set_slabs`].
    slabs: Option<Slabs>,
}

/// Orders of [`Slabs`] blocks: `MIN_CLASS << order` bytes, up to [`SLAB`].
const ORDERS: usize = (SLAB / MIN_CLASS).trailing_zeros() as usize + 1;

/// Usage of a slab: vertex and index regions share them (both read-only
/// in a pass); `COPY_SRC` so tests can read the bytes back.
const SLAB_USAGE: wgpu::BufferUsages = wgpu::BufferUsages::VERTEX
    .union(wgpu::BufferUsages::INDEX)
    .union(wgpu::BufferUsages::COPY_DST)
    .union(wgpu::BufferUsages::COPY_SRC);

/// Buddy allocation over slabs of [`SLAB`] bytes.
struct Slabs {
    buffers: Vec<Option<wgpu::Buffer>>,
    /// Free blocks per order, as (slab, offset): the lowest is taken first.
    free: Vec<BTreeSet<(usize, u64)>>,
}

impl Slabs {
    fn new() -> Self {
        Self {
            buffers: Vec::new(),
            free: vec![BTreeSet::new(); ORDERS],
        }
    }

    /// A block of `class` bytes (a power of two, `MIN_CLASS..=SLAB`): its
    /// slab, offset and buffer.
    fn alloc(&mut self, ctx: &GfxContext, class: u64) -> (usize, u64, wgpu::Buffer) {
        let order = (class / MIN_CLASS).trailing_zeros() as usize;
        let (mut o, (slab, offset)) = match (order..ORDERS).find(|&o| !self.free[o].is_empty()) {
            Some(o) => {
                crate::churn::buffer_reused();
                (o, self.free[o].pop_first().expect("non-empty"))
            }
            None => {
                let buffer = crate::churn::create_buffer(
                    &ctx.device,
                    &wgpu::BufferDescriptor {
                        label: Some("persistent slab"),
                        size: SLAB,
                        usage: SLAB_USAGE,
                        mapped_at_creation: false,
                    },
                );
                let slab = match self.buffers.iter().position(Option::is_none) {
                    Some(i) => {
                        self.buffers[i] = Some(buffer);
                        i
                    }
                    None => {
                        self.buffers.push(Some(buffer));
                        self.buffers.len() - 1
                    }
                };
                (ORDERS - 1, (slab, 0))
            }
        };
        // Split down to the class, freeing the upper halves.
        while o > order {
            o -= 1;
            self.free[o].insert((slab, offset + (MIN_CLASS << o)));
        }
        let buffer = self.buffers[slab].clone().expect("a live slab");
        (slab, offset, buffer)
    }

    /// Frees a block of `class` bytes, merged with its free buddies; a slab
    /// that comes back whole past [`SPARE_SLABS`] empty ones is dropped.
    fn free(&mut self, slab: usize, mut offset: u64, class: u64) {
        let mut o = (class / MIN_CLASS).trailing_zeros() as usize;
        while o + 1 < ORDERS && self.free[o].remove(&(slab, offset ^ (MIN_CLASS << o))) {
            offset &= !(MIN_CLASS << o);
            o += 1;
        }
        if o + 1 == ORDERS && self.free[o].len() >= SPARE_SLABS {
            self.buffers[slab] = None;
            return;
        }
        self.free[o].insert((slab, offset));
    }

    /// Slabs alive.
    fn live(&self) -> usize {
        self.buffers.iter().flatten().count()
    }
}

/// Persistent vertex buffers by stream id.
pub struct PersistentVertices {
    slots: HashMap<u64, Slot>,
    indices: HashMap<(u64, PrimitiveType), IndexSlot>,
    budget: u64,
    tick: u64,
    stats: PersistentStats,
    pool: Pool,
    /// [`set_staged`](Self::set_staged).
    staged: Option<Staged>,
}

/// The staging ring and the copies out of it not yet encoded.
struct Staged {
    ring: Option<UploadRing>,
    /// (source, source offset, destination, destination offset, bytes).
    copies: Vec<(wgpu::Buffer, u64, wgpu::Buffer, u64, u64)>,
}

/// Initial size of the staging ring (it grows as the upload rings do).
const STAGING_RING: u64 = 1 << 20;

impl Staged {
    /// Stages `len` bytes that `fill` writes, for a copy of `len` rounded
    /// up to 4 into `dst` at `at`.
    fn write(
        &mut self,
        ctx: &GfxContext,
        dst: &wgpu::Buffer,
        at: u64,
        len: usize,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
    ) {
        let ring = self.ring.get_or_insert_with(|| {
            UploadRing::new(
                &ctx.device,
                "persistent staging",
                wgpu::BufferUsages::COPY_SRC,
                STAGING_RING,
            )
        });
        let (src, off) = ring.push_with(&ctx.device, &ctx.queue, len, 4, fill);
        let size = (len as u64).max(4).next_multiple_of(4);
        self.copies.push((src, off, dst.clone(), at, size));
    }
}

impl PersistentVertices {
    /// Empty, holding at most `budget` bytes, in slabs
    /// ([`set_slabs`](Self::set_slabs)).
    pub fn new(budget: u64) -> Self {
        Self {
            slots: HashMap::new(),
            indices: HashMap::new(),
            budget,
            tick: 0,
            stats: PersistentStats::default(),
            pool: Pool {
                slabs: Some(Slabs::new()),
                ..Pool::default()
            },
            staged: None,
        }
    }

    /// Regions in a few large slabs (on, the default) or a buffer each
    /// (module docs). Switch before the first stream.
    pub fn set_slabs(&mut self, on: bool) {
        self.pool.slabs = on.then(Slabs::new);
    }

    /// Buffers held: slabs, and buffers of their own (in use, free, or
    /// waiting for a submit).
    pub fn buffers(&self) -> usize {
        let own = |r: &Region| r.slab.is_none();
        self.pool.slabs.as_ref().map_or(0, Slabs::live)
            + self.slots.values().filter(|s| own(&s.region)).count()
            + self.indices.values().filter(|s| own(&s.region)).count()
            + self.pool.retired.iter().filter(|r| own(r)).count()
            + self.pool.free.values().map(Vec::len).sum::<usize>()
    }

    /// Writes new regions through a staging ring and copies the owner
    /// submits first ([`copy_commands`](Self::copy_commands)) instead of a
    /// queue write each (module docs). Off by default: the owner must then
    /// submit [`copy_commands`](Self::copy_commands) ahead of every
    /// submission that draws from these buffers, after
    /// [`flush`](Self::flush).
    pub fn set_staged(&mut self, on: bool) {
        self.staged = on.then(|| Staged {
            ring: None,
            copies: Vec::new(),
        });
    }

    /// Hands the staged bytes to the queue (one write).
    pub fn flush(&mut self, queue: &wgpu::Queue) {
        if let Some(r) = self.staged.as_mut().and_then(|s| s.ring.as_mut()) {
            r.flush(queue);
        }
    }

    /// The copies of the regions written since the last call, as one
    /// command buffer to submit ahead of the draws (after
    /// [`flush`](Self::flush)); `None` when there are none.
    pub fn copy_commands(&mut self, device: &wgpu::Device) -> Option<wgpu::CommandBuffer> {
        let staged = self.staged.as_mut()?;
        if staged.copies.is_empty() {
            return None;
        }
        let mut enc = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("persistent copies"),
        });
        for (src, off, dst, at, size) in staged.copies.drain(..) {
            enc.copy_buffer_to_buffer(&src, off, &dst, at, size);
        }
        Some(enc.finish())
    }

    /// A region of at least `size` bytes (its size class) for vertices or
    /// (`index`) indices: from a slab, or a buffer of its own (reused if
    /// one is free).
    fn region(&mut self, ctx: &GfxContext, size: u64, index: bool) -> Region {
        let class = size.max(MIN_CLASS).next_power_of_two();
        if let Some(slabs) = self.pool.slabs.as_mut()
            && class <= SLAB
        {
            let (slab, offset, buffer) = slabs.alloc(ctx, class);
            return Region {
                buffer,
                offset,
                size: class,
                slab: Some(slab),
                index,
            };
        }
        Region {
            buffer: self.buffer(ctx, class, index),
            offset: 0,
            size: class,
            slab: None,
            index,
        }
    }

    /// A buffer of `class` bytes for vertices or (`index`) indices, reused
    /// if one is free.
    fn buffer(&mut self, ctx: &GfxContext, class: u64, index: bool) -> wgpu::Buffer {
        if let Some(b) = self.pool.free.get_mut(&(class, index)).and_then(Vec::pop) {
            self.pool.free_bytes -= class;
            crate::churn::buffer_reused();
            return b;
        }
        let (label, usage) = if index {
            ("persistent indices", wgpu::BufferUsages::INDEX)
        } else {
            ("persistent vertices", wgpu::BufferUsages::VERTEX)
        };
        crate::churn::create_buffer(
            &ctx.device,
            &wgpu::BufferDescriptor {
                label: Some(label),
                size: class,
                // COPY_SRC: tests read the bytes back.
                usage: usage | wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::COPY_SRC,
                mapped_at_creation: false,
            },
        )
    }

    /// Every draw recorded so far has been submitted: regions dropped
    /// before now may be written again.
    pub fn submitted(&mut self) {
        if let Some(s) = self.staged.as_mut() {
            debug_assert!(s.copies.is_empty(), "submitted with copies not encoded");
            if let Some(r) = s.ring.as_mut() {
                r.reset();
            }
        }
        let cap = self.budget / 2;
        for r in self.pool.retired.drain(..) {
            if let Some(slab) = r.slab {
                let slabs = self
                    .pool
                    .slabs
                    .as_mut()
                    .expect("slab regions come from slabs");
                slabs.free(slab, r.offset, r.size);
                continue;
            }
            if self.pool.free_bytes + r.size > cap {
                continue;
            }
            self.pool.free_bytes += r.size;
            self.pool
                .free
                .entry((r.size, r.index))
                .or_default()
                .push(r.buffer);
        }
    }

    /// The budget from `H2_VERTEX_CACHE_MB`, else [`DEFAULT_BUDGET`].
    pub fn budget_from_env() -> u64 {
        std::env::var("H2_VERTEX_CACHE_MB")
            .ok()
            .and_then(|v| v.parse::<u64>().ok())
            .map_or(DEFAULT_BUDGET, |mb| mb << 20)
    }

    /// The buffer for stream `id` as `plan` lays out `vertex_count` vertices
    /// of `guest` (already checked), written now if this is the first draw
    /// of `id` under `plan`. Returns the buffer, the offset and the bytes
    /// to bind from there.
    pub fn get(
        &mut self,
        ctx: &GfxContext,
        id: u64,
        plan: &StreamPlan,
        guest: &[u8],
        vertex_count: u32,
        scratch: &mut Vec<u8>,
    ) -> (wgpu::Buffer, u64, u64) {
        self.tick += 1;
        let len = plan.host_bytes(vertex_count);
        let bound = (len as u64).max(4);
        if let Some(s) = self.slots.get_mut(&id)
            && s.vertex_count == vertex_count
            && s.plan == *plan
        {
            s.used = self.tick;
            self.stats.hits += 1;
            return (s.region.buffer.clone(), s.region.offset, bound);
        }
        let region = self.region(ctx, bound.next_multiple_of(4), false);
        let (buffer, at, size) = (region.buffer.clone(), region.offset, region.size);
        if let Some(staged) = self.staged.as_mut() {
            staged.write(ctx, &buffer, at, len, |out| {
                plan.write_into(guest, vertex_count, out, scratch)
            });
        } else {
            crate::churn::queue_write();
            let mut view = ctx
                .queue
                .write_buffer_with(
                    &buffer,
                    at,
                    wgpu::BufferSize::new(bound.next_multiple_of(4)).expect("non-empty"),
                )
                .expect("staging view of a new buffer");
            plan.write_into(guest, vertex_count, view.slice(0..len), scratch);
        }
        self.stats.uploads += 1;
        self.stats.uploaded_bytes += len as u64;
        self.stats.held_bytes += size;
        if let Some(old) = self.slots.insert(
            id,
            Slot {
                plan: plan.clone(),
                vertex_count,
                region,
                used: self.tick,
            },
        ) {
            self.stats.held_bytes -= old.region.size;
            self.pool.retired.push(old.region);
        }
        self.evict();
        (buffer, at, bound)
    }

    /// The host index buffer kept for index list `id` drawn as `primitive`:
    /// the buffer, the offset, the bytes to bind from there and the indices
    /// it holds.
    pub fn cached_indices(
        &mut self,
        id: u64,
        primitive: PrimitiveType,
    ) -> Option<(wgpu::Buffer, u64, u64, u32)> {
        self.tick += 1;
        let s = self.indices.get_mut(&(id, primitive))?;
        s.used = self.tick;
        self.stats.index_hits += 1;
        Some((s.region.buffer.clone(), s.region.offset, s.bound, s.count))
    }

    /// Keeps `list` (the host index list of `id` drawn as `primitive`) in a
    /// region of its own; returns the buffer, the offset and the bytes to
    /// bind from there.
    pub fn put_indices(
        &mut self,
        ctx: &GfxContext,
        id: u64,
        primitive: PrimitiveType,
        list: &[u32],
    ) -> (wgpu::Buffer, u64, u64) {
        self.tick += 1;
        let bytes: &[u8] = bytemuck::cast_slice(list);
        let bound = (bytes.len() as u64).max(4);
        let region = self.region(ctx, bound, true);
        let (buffer, at) = (region.buffer.clone(), region.offset);
        match self.staged.as_mut() {
            _ if bytes.is_empty() => {}
            Some(staged) => staged.write(ctx, &buffer, at, bytes.len(), |mut out| {
                out.copy_from_slice(bytes)
            }),
            None => {
                crate::churn::queue_write();
                ctx.queue.write_buffer(&buffer, at, bytes);
            }
        }
        self.stats.index_uploads += 1;
        self.stats.held_bytes += region.size;
        let slot = IndexSlot {
            region,
            bound,
            count: list.len() as u32,
            used: self.tick,
        };
        if let Some(old) = self.indices.insert((id, primitive), slot) {
            self.stats.held_bytes -= old.region.size;
            self.pool.retired.push(old.region);
        }
        self.evict();
        (buffer, at, bound)
    }

    /// Counters so far.
    pub fn stats(&self) -> PersistentStats {
        self.stats
    }

    /// Drops every region (draws already recorded keep their buffers; the
    /// regions are reused after the next submit).
    pub fn clear(&mut self) {
        let retired = &mut self.pool.retired;
        retired.extend(self.slots.drain().map(|(_, s)| s.region));
        retired.extend(self.indices.drain().map(|(_, s)| s.region));
        self.stats.held_bytes = 0;
    }

    /// Past the budget: drops least recently used regions until a quarter
    /// of it is free.
    fn evict(&mut self) {
        if self.stats.held_bytes <= self.budget {
            return;
        }
        let mut order: Vec<(u64, Key)> = self
            .slots
            .iter()
            .map(|(k, s)| (s.used, Key::Vertices(*k)))
            .chain(
                self.indices
                    .iter()
                    .map(|(&(id, p), s)| (s.used, Key::Indices(id, p))),
            )
            .collect();
        // Ticks are unique: the order is total.
        order.sort_unstable_by_key(|&(used, _)| used);
        let goal = self.budget / 4 * 3;
        for (_, k) in order {
            if self.stats.held_bytes <= goal {
                break;
            }
            let freed = match k {
                Key::Vertices(id) => self.slots.remove(&id).map(|s| s.region),
                Key::Indices(id, p) => self.indices.remove(&(id, p)).map(|s| s.region),
            };
            if let Some(r) = freed {
                self.stats.held_bytes -= r.size;
                self.pool.retired.push(r);
            }
        }
    }
}
