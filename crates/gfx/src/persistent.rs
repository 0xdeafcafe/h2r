//! GPU vertex buffers that outlive a submission.
//!
//! A vertex stream a caller marks with an id ([`Draw::stream_ids`]) holds
//! bytes that never change under that id (the d3d device's vertex cache
//! hands out a new id whenever it re-reads a range). The first draw of an
//! id writes its host buffer (raw, realigned or converted, as the stream's
//! [`StreamPlan`] says) into a buffer of its own; later draws of the same
//! id with the same plan bind that buffer, with nothing staged in the
//! upload ring. Buffers are dropped least recently used first past a byte
//! budget; draws already recorded keep theirs alive (wgpu buffers are
//! reference counted).
//!
//! Index lists marked with an id ([`Draw::index_id`]) are kept the same
//! way, as the host list (Xbox-only primitives rewritten) per id and
//! primitive type.
//!
//! Buffers come in power-of-two size classes and are reused: a dropped
//! one (evicted or replaced) waits until the owner reports a submit
//! ([`PersistentVertices::submitted`]: every draw recorded with it is then
//! on the queue), then goes to a free list the next new id takes from. A
//! queued write lands after the work already submitted, so a reused buffer
//! never changes under a draw that still reads it. Free buffers are held
//! up to half the budget (an eviction frees a quarter at once).
//!
//! Staged writes ([`PersistentVertices::set_staged`], what `d3d`'s backend
//! uses): instead of a `queue.write_buffer` per new buffer (a staging
//! buffer wgpu makes and frees, about 20 us each), the bytes go into an
//! upload ring that is written once per flush, and a buffer-to-buffer copy
//! per new buffer is encoded into a command buffer of its own
//! ([`PersistentVertices::copy_commands`]) that the owner submits ahead of
//! the work that draws from them. Same bytes in the same buffers.
//!
//! [`Draw::stream_ids`]: crate::renderer::Draw::stream_ids
//! [`Draw::index_id`]: crate::renderer::Draw::index_id

use std::collections::HashMap;

use crate::context::GfxContext;
use crate::repack::StreamPlan;
use crate::ring::UploadRing;
use crate::vertex::PrimitiveType;

/// Default GPU budget (`H2_VERTEX_CACHE_MB` overrides it).
pub const DEFAULT_BUDGET: u64 = 128 << 20;

struct Slot {
    plan: StreamPlan,
    vertex_count: u32,
    buffer: wgpu::Buffer,
    size: u64,
    used: u64,
}

struct IndexSlot {
    buffer: wgpu::Buffer,
    size: u64,
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

/// Free buffers by (size, index usage).
#[derive(Default)]
struct Pool {
    free: HashMap<(u64, bool), Vec<wgpu::Buffer>>,
    free_bytes: u64,
    /// Dropped since the last submit.
    retired: Vec<(bool, wgpu::Buffer)>,
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
    /// (source, source offset, destination, bytes).
    copies: Vec<(wgpu::Buffer, u64, wgpu::Buffer, u64)>,
}

/// Initial size of the staging ring (it grows as the upload rings do).
const STAGING_RING: u64 = 1 << 20;

impl Staged {
    /// Stages `len` bytes that `fill` writes, for a copy of `len` rounded
    /// up to 4 into the start of `dst`.
    fn write(
        &mut self,
        ctx: &GfxContext,
        dst: &wgpu::Buffer,
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
        self.copies.push((src, off, dst.clone(), size));
    }
}

impl PersistentVertices {
    /// Empty, holding at most `budget` bytes.
    pub fn new(budget: u64) -> Self {
        Self {
            slots: HashMap::new(),
            indices: HashMap::new(),
            budget,
            tick: 0,
            stats: PersistentStats::default(),
            pool: Pool::default(),
            staged: None,
        }
    }

    /// Writes new buffers through a staging ring and copies the owner
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

    /// The copies of the buffers written since the last call, as one
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
        for (src, off, dst, size) in staged.copies.drain(..) {
            enc.copy_buffer_to_buffer(&src, off, &dst, 0, size);
        }
        Some(enc.finish())
    }

    /// A buffer of at least `size` bytes (its size class) for vertices or
    /// (`index`) indices, reused if one is free.
    fn buffer(&mut self, ctx: &GfxContext, size: u64, index: bool) -> wgpu::Buffer {
        let class = size.max(MIN_CLASS).next_power_of_two();
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

    /// Every draw recorded so far has been submitted: buffers dropped
    /// before now may be written again.
    pub fn submitted(&mut self) {
        if let Some(s) = self.staged.as_mut() {
            debug_assert!(s.copies.is_empty(), "submitted with copies not encoded");
            if let Some(r) = s.ring.as_mut() {
                r.reset();
            }
        }
        let cap = self.budget / 2;
        for (index, b) in self.pool.retired.drain(..) {
            let class = b.size();
            if self.pool.free_bytes + class > cap {
                continue;
            }
            self.pool.free_bytes += class;
            self.pool.free.entry((class, index)).or_default().push(b);
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
    /// of `id` under `plan`. Returns the buffer and the bytes it binds.
    pub fn get(
        &mut self,
        ctx: &GfxContext,
        id: u64,
        plan: &StreamPlan,
        guest: &[u8],
        vertex_count: u32,
        scratch: &mut Vec<u8>,
    ) -> (wgpu::Buffer, u64) {
        self.tick += 1;
        let len = plan.host_bytes(vertex_count);
        let bound = (len as u64).max(4);
        if let Some(s) = self.slots.get_mut(&id)
            && s.vertex_count == vertex_count
            && s.plan == *plan
        {
            s.used = self.tick;
            self.stats.hits += 1;
            return (s.buffer.clone(), bound);
        }
        let buffer = self.buffer(ctx, bound.next_multiple_of(4), false);
        let size = buffer.size();
        if let Some(staged) = self.staged.as_mut() {
            staged.write(ctx, &buffer, len, |out| {
                plan.write_into(guest, vertex_count, out, scratch)
            });
        } else {
            crate::churn::queue_write();
            let mut view = ctx
                .queue
                .write_buffer_with(
                    &buffer,
                    0,
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
                buffer: buffer.clone(),
                size,
                used: self.tick,
            },
        ) {
            self.stats.held_bytes -= old.size;
            self.pool.retired.push((false, old.buffer));
        }
        self.evict();
        (buffer, bound)
    }

    /// The host index buffer kept for index list `id` drawn as `primitive`:
    /// the buffer, the bytes it binds and the indices it holds.
    pub fn cached_indices(
        &mut self,
        id: u64,
        primitive: PrimitiveType,
    ) -> Option<(wgpu::Buffer, u64, u32)> {
        self.tick += 1;
        let s = self.indices.get_mut(&(id, primitive))?;
        s.used = self.tick;
        self.stats.index_hits += 1;
        Some((s.buffer.clone(), s.size, s.count))
    }

    /// Keeps `list` (the host index list of `id` drawn as `primitive`) in a
    /// buffer of its own; returns it and the bytes it binds.
    pub fn put_indices(
        &mut self,
        ctx: &GfxContext,
        id: u64,
        primitive: PrimitiveType,
        list: &[u32],
    ) -> (wgpu::Buffer, u64) {
        self.tick += 1;
        let bytes: &[u8] = bytemuck::cast_slice(list);
        let size = (bytes.len() as u64).max(4);
        let buffer = self.buffer(ctx, size, true);
        match self.staged.as_mut() {
            _ if bytes.is_empty() => {}
            Some(staged) => staged.write(ctx, &buffer, bytes.len(), |mut out| {
                out.copy_from_slice(bytes)
            }),
            None => {
                crate::churn::queue_write();
                ctx.queue.write_buffer(&buffer, 0, bytes);
            }
        }
        self.stats.index_uploads += 1;
        self.stats.held_bytes += buffer.size();
        let slot = IndexSlot {
            buffer: buffer.clone(),
            size,
            count: list.len() as u32,
            used: self.tick,
        };
        if let Some(old) = self.indices.insert((id, primitive), slot) {
            self.stats.held_bytes -= old.buffer.size();
            self.pool.retired.push((true, old.buffer));
        }
        self.evict();
        (buffer, size)
    }

    /// Counters so far.
    pub fn stats(&self) -> PersistentStats {
        self.stats
    }

    /// Drops every buffer (draws already recorded keep theirs; the pool
    /// reuses them after the next submit).
    pub fn clear(&mut self) {
        let retired = &mut self.pool.retired;
        retired.extend(self.slots.drain().map(|(_, s)| (false, s.buffer)));
        retired.extend(self.indices.drain().map(|(_, s)| (true, s.buffer)));
        self.stats.held_bytes = 0;
    }

    /// Past the budget: drops least recently used buffers until a quarter of
    /// it is free.
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
                Key::Vertices(id) => self.slots.remove(&id).map(|s| (false, s.buffer)),
                Key::Indices(id, p) => self.indices.remove(&(id, p)).map(|s| (true, s.buffer)),
            };
            if let Some((index, b)) = freed {
                self.stats.held_bytes -= b.size();
                self.pool.retired.push((index, b));
            }
        }
    }
}
