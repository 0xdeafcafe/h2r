//! Per-submission upload rings: one GPU buffer that many draws' bytes are
//! packed into, written with one `queue.write_buffer` per flush instead of
//! one buffer per draw.
//!
//! [`UploadRing::push`] stages bytes on the CPU and returns the buffer and
//! offset they will live at. [`UploadRing::flush`] writes everything staged
//! since the last flush. Queued writes land before the next submit, so the
//! staged regions must not be reused before that submit: the cursor only
//! moves forward until [`UploadRing::reset`], which the owner calls after
//! submitting. WebGPU orders a `write_buffer` after the work already
//! submitted, so a region reused after the reset never races a draw that
//! still reads it.
//!
//! When a push does not fit, the staged bytes go to the old buffer and a new
//! one takes over (draws recorded so far keep their handle to the old one).
//! An owner that never resets therefore never overwrites anything either:
//! it just moves to a fresh buffer each time one fills up.

/// A growable upload buffer for one usage (vertex/index, or uniforms).
pub struct UploadRing {
    label: &'static str,
    usage: wgpu::BufferUsages,
    buffer: wgpu::Buffer,
    capacity: u64,
    cursor: u64,
    staged_from: u64,
    staged: Vec<u8>,
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
            staged_from: 0,
            staged: Vec::new(),
            generation: 0,
        }
    }

    fn allocate(
        device: &wgpu::Device,
        label: &'static str,
        usage: wgpu::BufferUsages,
        size: u64,
    ) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size,
            usage: usage | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    /// Stages `bytes` at the next offset aligned to `align` (a power of two,
    /// at least 4) and returns the buffer and that offset. Empty data takes
    /// 4 zero bytes, so the region is never an empty slice.
    pub fn push(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        bytes: &[u8],
        align: u64,
    ) -> (wgpu::Buffer, u64) {
        debug_assert!(align.is_power_of_two() && align >= 4);
        let len = (bytes.len() as u64).max(4).next_multiple_of(4);
        let mut offset = self.cursor.next_multiple_of(align);
        if offset + len > self.capacity {
            self.flush(queue);
            self.capacity = self.capacity.max(len.next_power_of_two());
            self.buffer = Self::allocate(device, self.label, self.usage, self.capacity);
            self.generation += 1;
            self.cursor = 0;
            self.staged_from = 0;
            offset = 0;
        }
        let pad = (offset - self.cursor) as usize;
        self.staged.resize(self.staged.len() + pad, 0);
        self.staged.extend_from_slice(bytes);
        self.staged
            .resize(self.staged.len() + (len as usize - bytes.len()), 0);
        self.cursor = offset + len;
        (self.buffer.clone(), offset)
    }

    /// Writes the bytes staged since the last flush into the buffer.
    pub fn flush(&mut self, queue: &wgpu::Queue) {
        if !self.staged.is_empty() {
            queue.write_buffer(&self.buffer, self.staged_from, &self.staged);
            self.staged.clear();
        }
        self.staged_from = self.cursor;
    }

    /// Starts again from offset 0. Call after the submit that follows a
    /// [`flush`](Self::flush); anything still staged is flushed first by
    /// the caller's contract (debug-asserted).
    pub fn reset(&mut self) {
        debug_assert!(self.staged.is_empty(), "reset with unflushed bytes");
        self.cursor = 0;
        self.staged_from = 0;
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
