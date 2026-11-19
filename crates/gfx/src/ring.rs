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
            self.generation += 1;
            self.cursor = 0;
            offset = 0;
        }
        if self.staged.is_empty() {
            self.staged_at = offset;
        }
        let at = (offset - self.staged_at) as usize;
        self.staged.resize(at + padded as usize, 0);
        fill(wgpu::WriteOnly::from(&mut self.staged[at..at + len]));
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
pub struct UploadPool(std::sync::Arc<std::sync::Mutex<Vec<UploadRing>>>);

#[derive(Clone)]
pub struct SubmissionUploads(std::sync::Arc<SubmissionStorage>);

struct SubmissionStorage {
    pool: UploadPool,
    ring: std::sync::Mutex<Option<UploadRing>>,
}

impl Drop for SubmissionStorage {
    fn drop(&mut self) {
        let Some(mut ring) = self.ring.get_mut().unwrap_or_else(|e| e.into_inner()).take() else { return };
        // A cancelled preparation can still have unflushed CPU bytes.
        ring.staged.clear();
        ring.reset();
        self.pool.0.lock().unwrap_or_else(|e| e.into_inner()).push(ring);
    }
}

impl UploadPool {
    pub fn begin(&self) -> SubmissionUploads {
        SubmissionUploads(std::sync::Arc::new(SubmissionStorage { pool: self.clone(), ring: Default::default() }))
    }
}

impl SubmissionUploads {
    /// Allocate immutable bytes for a run. Returns a COPY_SRC region usable in
    /// this copy's encoder. A buffer is allocated only when a pass uploads data.
    pub fn stage(&self, device: &wgpu::Device, queue: &wgpu::Queue, bytes: &[u8]) -> (wgpu::Buffer, u64) {
        crate::churn::upload_bytes(bytes.len());
        let mut guard = self.0.ring.lock().unwrap_or_else(|e| e.into_inner());
        let ring = guard.get_or_insert_with(|| {
            self.0.pool.0.lock().unwrap_or_else(|e| e.into_inner()).pop().unwrap_or_else(|| {
                UploadRing::new(device, "native submission uploads", wgpu::BufferUsages::COPY_SRC, 256 << 10)
            })
        });
        ring.push(device, queue, bytes, 4)
    }

    /// Enqueue the staged bytes ahead of the commands that consume these regions.
    pub fn flush(&self, queue: &wgpu::Queue) {
        if let Some(ring) = self.0.ring.lock().unwrap_or_else(|e| e.into_inner()).as_mut() { ring.flush(queue); }
    }

    pub fn same_submission(&self, other: &Self) -> bool { std::sync::Arc::ptr_eq(&self.0, &other.0) }
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
    }

    /// Stage shared view data once and copy it into each placement's block;
    /// the stable rows in the rest of each block are initialized at map load.
    pub fn repeat(&mut self, dst: &wgpu::Buffer, first: u64, stride: u64, count: usize, bytes: &[u8]) {
        if count == 0 || bytes.is_empty() { return; }
        let (src, at) = self.arena.stage(self.device, self.queue, bytes);
        for i in 0..count {
            self.encoder.copy_buffer_to_buffer(&src, at, dst, first + i as u64 * stride, bytes.len() as u64);
        }
    }
}
