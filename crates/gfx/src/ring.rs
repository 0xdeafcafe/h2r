//! Per-submission upload rings: one GPU buffer that many draws' bytes are
//! packed into, written through a few large `queue.write_buffer_with`
//! staging views per flush instead of one buffer per draw.
//!
//! [`UploadRing::push`] (or [`UploadRing::push_with`], which lets the
//! caller produce the bytes in place) writes straight into wgpu's staging
//! memory and returns the buffer and offset they will live at: one CPU
//! copy, no intermediate buffer. The staging views cover [`CHUNK`]-sized
//! runs of the ring; a full one is handed back to the queue and the next
//! opened. [`UploadRing::flush`] hands back the open one. Queued writes
//! land before the next submit, in order (a later view overlapping an
//! earlier one's unused tail wins), so the written regions must not be
//! reused before that submit: the cursor only moves forward until
//! [`UploadRing::reset`], which the owner calls after submitting. WebGPU
//! orders a queued write after the work already submitted, so a region
//! reused after the reset never races a draw that still reads it.
//!
//! When a push does not fit, the open view goes to the old buffer and a
//! new, larger one takes over (draws recorded so far keep their handle to
//! the old one). An owner that never resets therefore never overwrites
//! anything either: it just moves to a fresh buffer each time one fills up.

/// Bytes a staging view covers (less at the end of the ring, more for a
/// single larger push).
pub const CHUNK: u64 = 2 << 20;

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
    /// The open staging view and the ring range `[start, end)` it covers.
    view: Option<(wgpu::QueueWriteBufferView, u64, u64)>,
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
            view: None,
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
    /// full. Padding up to a multiple of 4 is left unwritten.
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
        if self
            .view
            .as_ref()
            .is_none_or(|&(_, start, end)| offset < start || offset + padded > end)
        {
            // Hand back the open view (its unused tail is overwritten by the
            // next one, queued after it) and open one from here.
            self.view = None;
            let size = CHUNK.max(padded).min(self.capacity - offset);
            crate::churn::queue_write();
            let view = queue
                .write_buffer_with(
                    &self.buffer,
                    offset,
                    wgpu::BufferSize::new(size).expect("non-empty"),
                )
                .expect("staging view inside the ring");
            self.view = Some((view, offset, offset + size));
        }
        let (view, start, _) = self.view.as_mut().expect("opened above");
        let at = (offset - *start) as usize;
        fill(view.slice(at..at + len));
        self.cursor = offset + padded;
        (self.buffer.clone(), offset)
    }

    /// Hands the bytes written since the last flush to the queue.
    pub fn flush(&mut self, _queue: &wgpu::Queue) {
        self.view = None;
    }

    /// Starts again from offset 0. Call after the submit that follows a
    /// [`flush`](Self::flush); anything still open is flushed first by the
    /// caller's contract (debug-asserted).
    pub fn reset(&mut self) {
        debug_assert!(self.view.is_none(), "reset with unflushed bytes");
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
