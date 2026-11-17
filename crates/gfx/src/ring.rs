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
