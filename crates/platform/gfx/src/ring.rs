//! Per-submission upload rings: one GPU buffer that many draws' bytes are
//! packed into, written through one exactly sized `queue.write_buffer` per
//! flush instead of one buffer per draw.
//!
//! [`UploadRing::push`] (or [`UploadRing::push_with`], which lets the
//! caller produce the bytes in place) stages the bytes on the CPU and
//! returns the buffer and offset they will live at. [`UploadRing::flush`]
//! hands everything staged since the last flush to the queue as one write
//! of exactly the bytes used. The writes land before the next submit, in
//! order, so the written regions must not be reused before that submit:
//! the cursor only moves forward until [`UploadRing::reset`], which the
//! owner calls after submitting. WebGPU orders a queued write after the
//! work already submitted, so a region reused after the reset never races
//! a draw that still reads it.
//!
//! The staging used to be 2 MiB `write_buffer_with` views, one per ring
//! per flush whatever it held: wgpu makes (and frees) a Metal buffer of the
//! view's size for each, and copies all of it on the GPU. With a flush per
//! target switch that was about 130 us a flush against about 20 us for an
//! exactly sized write (`tests/staging_cost.rs`).
//!
//! When a push does not fit, what is staged goes to the old buffer and a
//! new, larger one takes over (draws recorded so far keep their handle to
//! the old one). An owner that never resets therefore never overwrites
//! anything either: it just moves to a fresh buffer each time one fills up.

/// Largest size a ring grows to by doubling when it fills (a single larger
/// push still gets a buffer of its own size).
const MAX_GROWTH: u64 = 64 << 20;
const IDLE_SUBMISSIONS: usize = 4;
const IDLE_UPLOAD_BYTES: u64 = 128 << 20;
static STAGING_BYTES: mem_watch::Counter = mem_watch::Counter::new("gfx.upload_ring_staging_bytes");
static BUFFER_BYTES: mem_watch::Counter = mem_watch::Counter::new("gfx.upload_ring_buffer_bytes");
static IDLE_CPU_BYTES: mem_watch::Counter = mem_watch::Counter::new("gfx.upload_idle_cpu_bytes");
static IDLE_GPU_BYTES: mem_watch::Counter = mem_watch::Counter::new("gfx.upload_idle_gpu_bytes");

/// A growable upload buffer for one usage (vertex/index, or uniforms).
pub struct UploadRing {
    label: &'static str,
    usage: wgpu::BufferUsages,
    buffer: wgpu::Buffer,
    capacity: u64,
    cursor: u64,
    /// Bytes staged since the last flush, for the ring range from
    /// `staged_at` (alignment gaps included, as zeros).
    staged: Vec<u8>,
    staging_bytes: mem_watch::Gauge,
    buffer_bytes: mem_watch::Gauge,
    staged_at: u64,
    generation: u64,
}

impl UploadRing {
    /// A ring of `capacity` bytes (rounded up to 4) for `usage`
    /// (`COPY_DST` is added).
    pub fn new(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        capacity: u64,
    ) -> Self {
        let capacity = capacity.max(4).next_multiple_of(4);
        Self {
            label,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            buffer: Self::allocate(device, label, usage, capacity),
            capacity,
            cursor: 0,
            staged: Vec::new(),
            staging_bytes: STAGING_BYTES.track(0),
            buffer_bytes: BUFFER_BYTES.track(capacity),
            staged_at: 0,
            generation: 0,
        }
    }

    fn allocate(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        size: u64,
    ) -> wgpu::Buffer {
        crate::churn::create_buffer(
            device,
            &wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        )
    }

    /// Writes `bytes` at the next offset aligned to `align` (a power of
    /// two, at least 4) and returns the buffer and that offset. Empty data
    /// takes 4 bytes, so the region is never an empty slice.
    pub fn push(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
        align: u64,
    ) -> (wgpu::Buffer, u64) {
        self.push_with(device, queue, bytes.len(), align, |mut dst| {
            dst.copy_from_slice(bytes)
        })
    }

    /// [`push`](Self::push) of `len` bytes that `fill` writes in place: it
    /// gets exactly `len` bytes of staging memory, which it must write in
    /// full. Padding up to a multiple of 4 is written as zeros.
    pub fn push_with(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        len: usize,
        align: u64,
        fill: impl FnOnce(wgpu::WriteOnly<'_, [u8]>),
    ) -> (wgpu::Buffer, u64) {
        self.push_bytes_with(device, queue, len, align, |bytes| fill(wgpu::WriteOnly::from(bytes)))
    }

    fn push_bytes_with(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        len: usize,
        align: u64,
        fill: impl FnOnce(&mut [u8]),
    ) -> (wgpu::Buffer, u64) {
        debug_assert!(align.is_power_of_two() && align >= 4);
        let padded = (len as u64).max(4).next_multiple_of(4);
        let mut offset = self.cursor.next_multiple_of(align);
        if offset + padded > self.capacity {
            self.flush(queue);
            // Grow (up to a cap) so a submission's uploads end up in one
            // buffer instead of a new one every few megabytes.
            self.capacity = (self.capacity * 2)
                .min(MAX_GROWTH)
                .max(self.capacity)
                .max(padded.next_power_of_two());
            self.buffer = Self::allocate(device, self.label, self.usage, self.capacity);
            self.buffer_bytes.set(self.capacity);
            self.generation += 1;
            self.cursor = 0;
            offset = 0;
        }
        if self.staged.is_empty() {
            self.staged_at = offset;
        }
        let at = (offset - self.staged_at) as usize;
        self.staged.resize(at + padded as usize, 0);
        self.staging_bytes.set(self.staged.capacity() as u64);
        fill(&mut self.staged[at..at + len]);
        self.cursor = offset + padded;
        (self.buffer.clone(), offset)
    }

    /// Hands the bytes written since the last flush to the queue: one
    /// write of exactly the range used.
    pub fn flush(&mut self, queue: &wgpu::Queue) {
        if self.staged.is_empty() {
            return;
        }
        crate::churn::queue_write();
        queue.write_buffer(&self.buffer, self.staged_at, &self.staged);
        self.staged.clear();
    }

    /// Starts again from offset 0. Call after the submit that follows a
    /// [`flush`](Self::flush); anything still open is flushed first by the
    /// caller's contract (debug-asserted).
    pub fn reset(&mut self) {
        debug_assert!(self.staged.is_empty(), "reset with unflushed bytes");
        self.cursor = 0;
    }

    /// The current buffer.
    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buffer
    }

    /// Bumped every time a new buffer replaces the current one (bind groups
    /// on the old buffer must be rebuilt).
    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Bytes used in the current buffer.
    pub fn used(&self) -> u64 {
        self.cursor
    }
}

/// Native views and target copies share a forward-only upload arena until submit.
/// Return the ring to this pool only after its last command buffer completes.
#[derive(Clone, Default)]
pub struct UploadPool(std::sync::Arc<std::sync::Mutex<IdleRings>>);

#[derive(Clone, Copy)]
struct IdleUsage {
    cpu: u64,
    gpu: u64,
}

struct IdleRings {
    rings: Vec<(SubmissionRing, IdleUsage)>,
    cpu_bytes: mem_watch::Gauge,
    gpu_bytes: mem_watch::Gauge,
    byte_limit: u64,
}

impl Default for IdleRings {
    fn default() -> Self {
        Self {
            rings: Vec::new(),
            cpu_bytes: IDLE_CPU_BYTES.track(0),
            gpu_bytes: IDLE_GPU_BYTES.track(0),
            byte_limit: IDLE_UPLOAD_BYTES,
        }
    }
}

impl IdleRings {
    fn pop(&mut self) -> Option<SubmissionRing> {
        let (ring, usage) = self.rings.pop()?;
        self.cpu_bytes.set(self.cpu_bytes.get() - usage.cpu);
        self.gpu_bytes.set(self.gpu_bytes.get() - usage.gpu);
        Some(ring)
    }

    fn try_push(&mut self, ring: SubmissionRing) -> Option<SubmissionRing> {
        let usage = ring.idle_usage();
        let cpu = self.cpu_bytes.get() + usage.cpu;
        let gpu = self.gpu_bytes.get() + usage.gpu;
        if self.rings.len() == IDLE_SUBMISSIONS || cpu + gpu > self.byte_limit {
            return Some(ring);
        }
        self.rings.push((ring, usage));
        self.cpu_bytes.set(cpu);
        self.gpu_bytes.set(gpu);
        None
    }
}

#[derive(Clone)]
pub struct SubmissionUploads(std::sync::Arc<SubmissionStorage>);

struct SubmissionStorage {
    pool: UploadPool,
    ring: std::sync::Mutex<Option<SubmissionRing>>,
}

struct SubmissionRing {
    upload: UploadRing,
    uniform_groups: Vec<UniformGroup>,
    // Slots are exclusive to this submission until its completion callback.
    // Replaying a different draw order is safe: compare the actual bytes.
    resident: Vec<ResidentRun>,
    next_run: usize,
    patches: Option<wgpu::CommandEncoder>,
    // Repeated view rows can feed multiple destinations from one staged span.
    patch_sources: std::collections::HashMap<(usize, usize), (u64, u64)>,
    // Persistent buffers a command recorded with this arena writes in draw
    // order ([`EncoderUploads`]). Kept until the arena returns: its commands
    // can land in a later submission than the one it started.
    ordered: std::collections::HashSet<wgpu::Buffer>,
    scratch: Vec<u8>,
}

struct ResidentRun {
    buffer: wgpu::Buffer,
    group: wgpu::BindGroup,
    layout: wgpu::BindGroupLayout,
    sizes: [usize; 2],
    // Last contents including alignment padding. None after a cancelled upload.
    shadow: Option<Vec<u8>>,
}

struct UniformGroup {
    generation: u64,
    layout: wgpu::BindGroupLayout,
    sizes: [usize; 2],
    group: wgpu::BindGroup,
}

impl SubmissionRing {
    /// Staging and resident contents can keep a large map's high-water mark
    /// even when the ring is empty. Uniform groups pin the backing upload
    /// buffer or their resident slot's buffer, which are counted once here.
    fn idle_usage(&self) -> IdleUsage {
        let mut cpu = self.upload.staged.capacity() as u64 + self.scratch.capacity() as u64;
        let mut gpu = self.upload.capacity;
        for slot in &self.resident {
            cpu += slot.shadow.as_ref().map_or(0, |shadow| shadow.capacity()) as u64;
            gpu += slot.buffer.size();
        }
        IdleUsage { cpu, gpu }
    }
}

/// Dynamic offsets for two uniforms per draw in an immutable submission allocation.
pub struct UniformRun {
    pub group: wgpu::BindGroup,
    first: [u32; 2],
    stride: u32,
}

impl UniformRun {
    pub fn offsets(&self, draw: usize) -> [u32; 2] {
        self.first.map(|offset| offset + draw as u32 * self.stride)
    }
}

impl Drop for SubmissionStorage {
    fn drop(&mut self) {
        let Some(mut ring) = self.ring.get_mut().unwrap_or_else(|e| e.into_inner()).take() else { return };
        // A cancelled preparation can still have unflushed CPU bytes.
        if ring.patches.take().is_some() {
            // The patch encoder never reached the queue. Its CPU shadows
            // describe speculative contents, so refill these slots on reuse.
            for slot in &mut ring.resident[..ring.next_run] {
                slot.shadow = None;
            }
        }
        // Keep spare slots for a subsequent view with more visible draws.
        // They hold uniforms and layouts, never references to map textures.
        ring.resident.truncate(ring.next_run.saturating_add(64));
        ring.patch_sources.clear();
        ring.ordered.clear();
        ring.next_run = 0;
        ring.upload.staged.clear();
        ring.upload.reset();
        let overflow = self.pool.0.lock().unwrap_or_else(|e| e.into_inner()).try_push(ring);
        drop(overflow);
    }
}

impl UploadPool {
    pub fn begin(&self) -> SubmissionUploads {
        SubmissionUploads(std::sync::Arc::new(SubmissionStorage { pool: self.clone(), ring: Default::default() }))
    }
}

impl SubmissionUploads {
    fn with_ring<T>(&self, device: &wgpu::Device, f: impl FnOnce(&mut SubmissionRing) -> T) -> T {
        let mut guard = self.0.ring.lock().unwrap_or_else(|e| e.into_inner());
        let ring = guard.get_or_insert_with(|| {
            self.0.pool.0.lock().unwrap_or_else(|e| e.into_inner()).pop().unwrap_or_else(|| SubmissionRing {
                upload: UploadRing::new(device, "native submission uploads",
                    wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::UNIFORM
                        | wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::INDEX,
                    256 << 10),
                uniform_groups: Vec::new(),
                resident: Vec::new(), next_run: 0, patches: None, patch_sources: Default::default(),
                ordered: Default::default(), scratch: Vec::new(),
            })
        });
        let before = ring.upload.generation();
        let result = f(ring);
        let after = ring.upload.generation();
        if before != after {
            ring.uniform_groups.retain(|g| g.generation == after);
        }
        result
    }

    /// Allocate immutable bytes for a run. Returns a COPY_SRC region usable in
    /// this copy's encoder or bound as vertices or indices. A buffer is
    /// allocated only when a pass uploads data.
    pub fn stage(&self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> (wgpu::Buffer, u64) {
        crate::churn::upload_bytes(bytes.len());
        self.with_ring(device, |r| r.upload.push(device, queue, bytes, 4))
    }

    /// Pack pairs of dynamic vertex and pixel uniforms for successive draws into the
    /// submission. A pooled backing buffer keeps its bind group across submissions.
    pub fn uniform_run(
        &self, device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout,
        sizes: [usize; 2], count: usize, mut fill: impl FnMut(usize, &mut [u8], &mut [u8]),
    ) -> Option<UniformRun> {
        let limits = device.limits();
        if count == 0 || sizes.iter().any(|&n| n == 0 || n as u64 > limits.max_uniform_buffer_binding_size) {
            return None;
        }
        let align = limits.min_uniform_buffer_offset_alignment.max(4) as usize;
        let pixel = sizes[0].checked_next_multiple_of(align)?;
        let stride = pixel.checked_add(sizes[1])?.checked_next_multiple_of(align)?;
        let len = stride.checked_mul(count)?;
        if len as u64 > limits.max_buffer_size || len > u32::MAX as usize { return None; }
        crate::churn::upload_bytes(count * (sizes[0] + sizes[1]));
        self.with_ring(device, |r| {
            let (buffer, base) = r.upload.push_bytes_with(device, queue, len, align as u64, |dst| {
                for (i, pair) in dst.chunks_exact_mut(stride).enumerate() {
                    let (v, p) = pair.split_at_mut(pixel);
                    fill(i, &mut v[..sizes[0]], &mut p[..sizes[1]]);
                }
            });
            if base + len as u64 - 1 > u64::from(u32::MAX) { return None; }
            let run = |group| Some(UniformRun {
                group, first: [base as u32, (base + pixel as u64) as u32], stride: stride as u32,
            });
            let generation = r.upload.generation();
            if let Some(cached) = r.uniform_groups.iter().find(|g| {
                g.generation == generation && &g.layout == layout && g.sizes == sizes
            }) {
                return run(cached.group.clone());
            }
            let entry = |binding: u32| wgpu::BindGroupEntry {
                binding,
                resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                    buffer: &buffer,
                    offset: 0,
                    size: wgpu::BufferSize::new(sizes[binding as usize] as u64),
                }),
            };
            let group = crate::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
                label: Some("native submission uniforms"), layout,
                entries: &[entry(0), entry(1)],
            });
            // A ring can serve interleaved object and world programs. Bound the
            // retired layouts retained when maps or renderers change.
            if r.uniform_groups.len() == 8 {
                r.uniform_groups.remove(0);
            }
            r.uniform_groups.push(UniformGroup { generation, layout: layout.clone(), sizes, group: group.clone() });
            run(group)
        })
    }

    /// Reuse uniform storage across completed submissions. In a steady scene
    /// only changed constant rows (view, skinning, fog, etc.) cross the bus.
    /// Draws within a submission get distinct slots, including split views.
    pub fn resident_uniform_run(
        &self, device: &wgpu::Device, queue: &wgpu::Queue, layout: &wgpu::BindGroupLayout,
        sizes: [usize; 2], count: usize, mut fill: impl FnMut(usize, &mut [u8], &mut [u8]),
    ) -> Option<UniformRun> {
        let limits = device.limits();
        if count == 0 || sizes.iter().any(|&n| n == 0 || n as u64 > limits.max_uniform_buffer_binding_size) {
            return None;
        }
        let align = limits.min_uniform_buffer_offset_alignment.max(4) as usize;
        let pixel = sizes[0].checked_next_multiple_of(align)?;
        let stride = pixel.checked_add(sizes[1])?.checked_next_multiple_of(align)?;
        let len = count.checked_mul(stride)?;
        if len as u64 > limits.max_buffer_size || len > u32::MAX as usize { return None; }
        self.with_ring(device, |ring| {
            let index = ring.next_run;
            if ring.resident.get(index).is_none_or(|s| {
                &s.layout != layout || s.sizes != sizes || s.buffer.size() < len as u64
            }) {
                let buffer = crate::churn::create_buffer(device, &wgpu::BufferDescriptor {
                    label: Some("resident native constants"), size: len as u64,
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                let entry = |binding: u32| wgpu::BindGroupEntry {
                    binding, resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                        buffer: &buffer, offset: 0, size: wgpu::BufferSize::new(sizes[binding as usize] as u64),
                    }),
                };
                let group = crate::churn::create_bind_group(device, &wgpu::BindGroupDescriptor {
                    label: Some("resident native uniforms"), layout, entries: &[entry(0), entry(1)],
                });
                let slot = ResidentRun { buffer, group, layout: layout.clone(), sizes, shadow: None };
                if index == ring.resident.len() { ring.resident.push(slot); } else { ring.resident[index] = slot; }
            }
            ring.next_run += 1;
            let slot = &mut ring.resident[index];
            ring.scratch.resize(len, 0);
            for (i, pair) in ring.scratch.chunks_exact_mut(stride).enumerate() {
                let (v, p) = pair.split_at_mut(pixel);
                v[sizes[0]..].fill(0);
                p[sizes[1]..].fill(0);
                fill(i, &mut v[..sizes[0]], &mut p[..sizes[1]]);
            }
            let bytes = &ring.scratch;
            let old = slot.shadow.as_deref().unwrap_or(&[]);
            changed_rows(old, bytes, |start, changed| {
                let key = (start % stride, changed.len());
                let cached = ring.patch_sources.get(&key).and_then(|&(generation, at)| {
                    if generation != ring.upload.generation() { return None; }
                    let i = usize::try_from(at.checked_sub(ring.upload.staged_at)?).ok()?;
                    (ring.upload.staged.get(i..i.checked_add(changed.len())?)? == changed).then_some(at)
                });
                let (src, at) = if let Some(at) = cached {
                    (ring.upload.buffer().clone(), at)
                } else {
                    crate::churn::upload_bytes(changed.len());
                    let range = ring.upload.push(device, queue, changed, 4);
                    ring.patch_sources.insert(key, (ring.upload.generation(), range.1));
                    range
                };
                let encoder = ring.patches.get_or_insert_with(|| device.create_command_encoder(&Default::default()));
                encoder.copy_buffer_to_buffer(&src, at, &slot.buffer, start as u64, changed.len() as u64);
            });
            let shadow = slot.shadow.get_or_insert_with(Vec::new);
            std::mem::swap(shadow, &mut ring.scratch);
            Some(UniformRun { group: slot.group.clone(), first: [0, pixel as u32], stride: stride as u32 })
        })
    }

    /// Flush the shared staging write. Submit the returned copies ahead of
    /// the first command buffer that binds the resident uniform buffers.
    pub fn upload_commands(&self, queue: &wgpu::Queue) -> Option<wgpu::CommandBuffer> {
        let mut guard = self.0.ring.lock().unwrap_or_else(|e| e.into_inner());
        let ring = guard.as_mut()?;
        ring.upload.flush(queue);
        ring.patch_sources.clear();
        ring.patches.take().map(wgpu::CommandEncoder::finish)
    }

    /// Copy `bytes` into a persistent buffer ahead of every command of the
    /// submission whose [`upload_commands`](Self::upload_commands) take it.
    /// The caller keeps one content per range for that submission
    /// (the renderer decides at commit, in submission order).
    pub fn write_front(&self, device: &wgpu::Device, queue: &wgpu::Queue, dst: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
        if bytes.is_empty() { return; }
        crate::churn::upload_bytes(bytes.len());
        self.with_ring(device, |ring| {
            let (src, at) = ring.upload.push(device, queue, bytes, 4);
            let encoder = ring.patches.get_or_insert_with(|| device.create_command_encoder(&Default::default()));
            encoder.copy_buffer_to_buffer(&src, at, dst, offset, bytes.len() as u64);
        });
    }

    /// Whether a command recorded with this arena writes `dst` in draw order:
    /// a front write of it could land before that write.
    pub fn wrote_in_order(&self, dst: &wgpu::Buffer) -> bool {
        let guard = self.0.ring.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().is_some_and(|ring| ring.ordered.contains(dst))
    }

    fn written_in_order(&self, device: &wgpu::Device, dst: &wgpu::Buffer) {
        self.with_ring(device, |ring| { ring.ordered.insert(dst.clone()); });
    }

    pub fn flush(&self, queue: &wgpu::Queue) {
        if let Some(copies) = self.upload_commands(queue) { queue.submit([copies]); }
    }

    pub fn same_submission(&self, other: &Self) -> bool { std::sync::Arc::ptr_eq(&self.0, &other.0) }
}

/// Coalesce consecutive changed vec4 rows. Compare bytes to preserve NaNs and
/// signed zero. All native uniform bindings and copy ranges are word aligned.
fn changed_rows(old: &[u8], new: &[u8], mut copy: impl FnMut(usize, &[u8])) {
    if old == new { return; }
    if old.len() != new.len() { copy(0, new); return; }
    let mut first = None;
    for (i, (a, b)) in old.chunks(16).zip(new.chunks(16)).enumerate() {
        let at = i * 16;
        if a != b { first.get_or_insert(at); }
        else if let Some(start) = first.take() { copy(start, &new[start..at]); }
    }
    if let Some(start) = first { copy(start, &new[start..]); }
}

/// Writes made immediately before a native draw. Only the source allocation is
/// queue-written. The copies into bound uniforms and indirect args follow draw order.
pub struct EncoderUploads<'a> {
    pub arena: &'a SubmissionUploads,
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
}

impl EncoderUploads<'_> {
    pub fn write(&mut self, dst: &wgpu::Buffer, offset: u64, bytes: &[u8]) {
        if bytes.is_empty() { return; }
        let (src, at) = self.arena.stage(self.device, self.queue, bytes);
        self.encoder.copy_buffer_to_buffer(&src, at, dst, offset, bytes.len() as u64);
        self.arena.written_in_order(self.device, dst);
    }

    /// Copy already staged bytes (from [`SubmissionUploads::stage`]) in draw
    /// order, remembering `dst` as written in order like [`Self::write`].
    pub fn copy(&mut self, src: &wgpu::Buffer, src_offset: u64, dst: &wgpu::Buffer, offset: u64, len: u64) {
        if len == 0 { return; }
        self.encoder.copy_buffer_to_buffer(src, src_offset, dst, offset, len);
        self.arena.written_in_order(self.device, dst);
    }

    /// Stage shared view data once and copy it into each placement's block;
    /// the stable rows in the rest of each block are initialized at map load.
    pub fn repeat(&mut self, dst: &wgpu::Buffer, first: u64, stride: u64, count: usize, bytes: &[u8]) {
        if count == 0 || bytes.is_empty() { return; }
        let (src, at) = self.arena.stage(self.device, self.queue, bytes);
        for i in 0..count {
            self.encoder.copy_buffer_to_buffer(&src, at, dst, first + i as u64 * stride, bytes.len() as u64);
        }
        self.arena.written_in_order(self.device, dst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{GfxContext, program::ProgramBindings};

    #[test]
    fn upload_pool_keeps_only_a_fixed_number_of_returned_arenas() {
        let Ok(ctx) = GfxContext::headless() else { return };
        let pool = UploadPool::default();
        let submissions: Vec<_> = (0..IDLE_SUBMISSIONS * 3).map(|_| {
            let uploads = pool.begin();
            uploads.stage(&ctx.device, &ctx.queue, &[1, 2, 3, 4]);
            uploads
        }).collect();
        assert!(pool.0.lock().unwrap().rings.is_empty());
        drop(submissions);
        assert_eq!(pool.0.lock().unwrap().rings.len(), IDLE_SUBMISSIONS);
        let uploads = pool.begin();
        uploads.stage(&ctx.device, &ctx.queue, &[5, 6, 7, 8]);
        assert_eq!(pool.0.lock().unwrap().rings.len(), IDLE_SUBMISSIONS - 1);
    }

    #[test]
    fn upload_pool_caps_total_idle_capacity_including_cpu_scratch() {
        let Ok(ctx) = GfxContext::headless() else { return };
        let pool = UploadPool::default();
        let first = pool.begin();
        let second = pool.begin();
        let large_scratch = pool.begin();
        let usage = first.with_ring(&ctx.device, |ring| ring.idle_usage());
        second.with_ring(&ctx.device, |_| {});
        large_scratch.with_ring(&ctx.device, |ring| ring.scratch.reserve(1024));
        pool.0.lock().unwrap().byte_limit = usage.gpu + 512;

        drop(large_scratch);
        assert!(pool.0.lock().unwrap().rings.is_empty());
        drop(first);
        drop(second);
        {
            let idle = pool.0.lock().unwrap();
            assert_eq!(idle.rings.len(), 1);
            assert_eq!(idle.gpu_bytes.get(), usage.gpu);
            assert_eq!(idle.cpu_bytes.get(), 0);
        }
        let reused = pool.begin();
        reused.with_ring(&ctx.device, |_| {});
        assert_eq!(pool.0.lock().unwrap().gpu_bytes.get(), 0);
    }

    #[test]
    fn resident_constant_patches_are_batched_and_cancelled_slots_refill() {
        let Ok(ctx) = GfxContext::headless() else { return };
        let pool = UploadPool::default();
        let program = ProgramBindings::new(&ctx.device);
        let allocate = |arena: &SubmissionUploads, tag| {
            arena.resident_uniform_run(&ctx.device, &ctx.queue, program.uniform_layout(), [64, 32],
                1, |_, v, p| { v.fill(0); v[32..48].fill(tag); p.fill(9); }).unwrap().group
        };
        let first = pool.begin();
        let a = allocate(&first, 1);
        let b = allocate(&first, 2);
        assert_ne!(a, b);
        first.flush(&ctx.queue);
        ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        drop(first);

        let second = pool.begin();
        assert_eq!(a, allocate(&second, 1));
        assert_eq!(b, allocate(&second, 3));
        second.with_ring(&ctx.device, |r| {
            assert_eq!(r.upload.staged, [3; 16]);
        });
        drop(second); // The patch for slot b never reached the GPU.
        let retry = pool.begin();
        allocate(&retry, 1);
        allocate(&retry, 3);
        retry.with_ring(&ctx.device, |r| assert!(r.upload.staged.len() > 16));
        let before = crate::churn::snapshot();
        retry.flush(&ctx.queue);
        assert_eq!((crate::churn::snapshot() - before).queue_writes, 1);
    }

    #[test]
    fn front_writes_go_ahead_of_the_submission_and_ordered_writes_are_remembered_until_the_arena_returns() {
        let Ok(ctx) = GfxContext::headless() else { return };
        let buffer = |label| ctx.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label), size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let (a, b) = (buffer("a"), buffer("b"));
        let pool = UploadPool::default();
        let arena = pool.begin();
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        // In draw order: after the front write of the same range.
        EncoderUploads { arena: &arena, device: &ctx.device, queue: &ctx.queue, encoder: &mut encoder }
            .write(&b, 0, &[2; 16]);
        arena.write_front(&ctx.device, &ctx.queue, &b, 0, &[1; 16]);
        arena.write_front(&ctx.device, &ctx.queue, &a, 16, &[3; 16]);
        assert!(arena.wrote_in_order(&b));
        assert!(!arena.wrote_in_order(&a));
        let front = arena.upload_commands(&ctx.queue).expect("front copies");
        ctx.queue.submit([front, encoder.finish()]);
        let read = |buffer: &wgpu::Buffer| {
            buffer.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
            ctx.device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
            let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
            buffer.unmap();
            bytes
        };
        assert_eq!(read(&b)[..16], [2; 16], "the ordered write lands last");
        assert_eq!(read(&a)[16..32], [3; 16]);
        // A submission's flush does not forget the ordered write; the arena's return does.
        assert!(arena.wrote_in_order(&b));
        drop(arena);
        let next = pool.begin();
        next.stage(&ctx.device, &ctx.queue, &[0; 4]);
        assert!(!next.wrote_in_order(&b), "the pooled ring starts clean");
    }

    #[test]
    fn uniform_runs_keep_distinct_bytes_and_rebind_when_the_backing_or_layout_changes() {
        let Ok(ctx) = GfxContext::headless() else { return };
        let pool = UploadPool::default();
        let arena = pool.begin();
        let program = ProgramBindings::new(&ctx.device);
        let layout = program.uniform_layout();
        let allocate = |arena: &SubmissionUploads, layout: &wgpu::BindGroupLayout, sizes, count, tag: u8| {
            arena.uniform_run(&ctx.device, &ctx.queue, layout, sizes, count, |i, v, p| {
                v.fill(tag + i as u8);
                p.fill(tag + 10 + i as u8);
            }).unwrap()
        };
        let sizes = [12, 20];
        let first = allocate(&arena, layout, sizes, 2, 1);
        let second = allocate(&arena, layout, sizes, 1, 3);
        assert_eq!(first.group, second.group);
        let align = ctx.device.limits().min_uniform_buffer_offset_alignment;
        let draws = [first.offsets(0), first.offsets(1), second.offsets(0)];
        for [v, p] in draws {
            assert_eq!((v % align, p % align), (0, 0));
        }
        arena.with_ring(&ctx.device, |r| {
            for (i, [v, p]) in draws.into_iter().enumerate() {
                assert_eq!(r.upload.staged[v as usize..][..12], [1 + i as u8; 12]);
                assert_eq!(r.upload.staged[p as usize..][..20], [11 + i as u8; 20]);
            }
        });

        let resized = allocate(&arena, layout, [16, 20], 1, 4);
        assert_ne!(second.group, resized.group);
        let other = ProgramBindings::new(&ctx.device);
        let rebound = allocate(&arena, other.uniform_layout(), [16, 20], 1, 5);
        assert_ne!(resized.group, rebound.group);
        assert_eq!(resized.group, allocate(&arena, layout, [16, 20], 1, 5).group);

        // Force ring growth while older commands can still reference the original.
        arena.stage(&ctx.device, &ctx.queue, &vec![0; 256 << 10]);
        let grown = allocate(&arena, other.uniform_layout(), [16, 20], 1, 6);
        assert_ne!(rebound.group, grown.group);
        // Cancel the pending run. Returning the ring resets its offsets.
        drop(arena);
        let reused = allocate(&pool.begin(), other.uniform_layout(), [16, 20], 1, 7);
        assert_eq!(grown.group, reused.group);
        assert_eq!(reused.offsets(0)[0], 0);
    }
}
