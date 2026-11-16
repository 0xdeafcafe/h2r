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
//! [`Draw::stream_ids`]: crate::renderer::Draw::stream_ids
//! [`Draw::index_id`]: crate::renderer::Draw::index_id

use std::collections::HashMap;

use crate::context::GfxContext;
use crate::repack::StreamPlan;
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

/// Persistent vertex buffers by stream id.
pub struct PersistentVertices {
    slots: HashMap<u64, Slot>,
    indices: HashMap<(u64, PrimitiveType), IndexSlot>,
    budget: u64,
    tick: u64,
    stats: PersistentStats,
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
        let size = bound.next_multiple_of(4);
        let buffer = crate::churn::create_buffer(
            &ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("persistent vertices"),
                size,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        {
            crate::churn::queue_write();
            let mut view = ctx
                .queue
                .write_buffer_with(&buffer, 0, wgpu::BufferSize::new(size).expect("non-empty"))
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
        let buffer = crate::churn::create_buffer(
            &ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("persistent indices"),
                size,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        if !bytes.is_empty() {
            crate::churn::queue_write();
            ctx.queue.write_buffer(&buffer, 0, bytes);
        }
        self.stats.index_uploads += 1;
        self.stats.held_bytes += size;
        let slot = IndexSlot {
            buffer: buffer.clone(),
            size,
            count: list.len() as u32,
            used: self.tick,
        };
        if let Some(old) = self.indices.insert((id, primitive), slot) {
            self.stats.held_bytes -= old.size;
        }
        self.evict();
        (buffer, size)
    }

    /// Counters so far.
    pub fn stats(&self) -> PersistentStats {
        self.stats
    }

    /// Drops every buffer (draws already recorded keep theirs).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.indices.clear();
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
                Key::Vertices(id) => self.slots.remove(&id).map(|s| s.size),
                Key::Indices(id, p) => self.indices.remove(&(id, p)).map(|s| s.size),
            };
            self.stats.held_bytes -= freed.unwrap_or(0);
        }
    }
}
