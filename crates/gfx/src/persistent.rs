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
//! [`Draw::stream_ids`]: crate::renderer::Draw::stream_ids

use std::collections::HashMap;

use crate::context::GfxContext;
use crate::repack::StreamPlan;

/// Default GPU budget (`H2_VERTEX_CACHE_MB` overrides it).
pub const DEFAULT_BUDGET: u64 = 128 << 20;

struct Slot {
    plan: StreamPlan,
    vertex_count: u32,
    buffer: wgpu::Buffer,
    size: u64,
    used: u64,
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
}

/// Persistent vertex buffers by stream id.
pub struct PersistentVertices {
    slots: HashMap<u64, Slot>,
    budget: u64,
    tick: u64,
    stats: PersistentStats,
}

impl PersistentVertices {
    /// Empty, holding at most `budget` bytes.
    pub fn new(budget: u64) -> Self {
        Self {
            slots: HashMap::new(),
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
        let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("persistent vertices"),
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        {
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

    /// Counters so far.
    pub fn stats(&self) -> PersistentStats {
        self.stats
    }

    /// Drops every buffer (draws already recorded keep theirs).
    pub fn clear(&mut self) {
        self.slots.clear();
        self.stats.held_bytes = 0;
    }

    /// Past the budget: drops least recently used buffers until a quarter of
    /// it is free.
    fn evict(&mut self) {
        if self.stats.held_bytes <= self.budget {
            return;
        }
        let mut order: Vec<(u64, u64)> = self.slots.iter().map(|(k, s)| (s.used, *k)).collect();
        order.sort_unstable();
        let goal = self.budget / 4 * 3;
        for (_, k) in order {
            if self.stats.held_bytes <= goal {
                break;
            }
            if let Some(s) = self.slots.remove(&k) {
                self.stats.held_bytes -= s.size;
            }
        }
    }
}
