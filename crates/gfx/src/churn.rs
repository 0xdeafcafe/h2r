//! Counters of per-frame GPU allocation churn, for tests and benches.
//!
//! Each counter is kept per thread: the render thread (or a test driving a
//! backend directly) sees only its own work, so tests running side by side
//! do not disturb each other's numbers. Read them with [`snapshot`] and
//! subtract two snapshots for a frame's share.

use std::cell::Cell;

/// What was allocated on this thread so far.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Churn {
    /// GPU buffers created by `gfx` (and the d3d backend's counted paths):
    /// rings, persistent vertex and index buffers, staging chunks, count
    /// and read-back buffers.
    pub buffers_created: u64,
    /// Staging buffers wgpu made for a `queue.write_buffer` or
    /// `write_buffer_with` from `gfx` (each is a GPU buffer created and
    /// freed by wgpu).
    pub queue_writes: u64,
    /// Staging chunks reused from a pool instead of created.
    pub staging_reused: u64,
    /// Buffers reused from a pool instead of created (persistent vertex and
    /// index buffers).
    pub buffers_reused: u64,
}

impl std::ops::Sub for Churn {
    type Output = Churn;
    fn sub(self, o: Churn) -> Churn {
        Churn {
            buffers_created: self.buffers_created - o.buffers_created,
            queue_writes: self.queue_writes - o.queue_writes,
            staging_reused: self.staging_reused - o.staging_reused,
            buffers_reused: self.buffers_reused - o.buffers_reused,
        }
    }
}

thread_local! {
    static CHURN: Cell<Churn> = const { Cell::new(Churn {
        buffers_created: 0,
        queue_writes: 0,
        staging_reused: 0,
        buffers_reused: 0,
    }) };
}

fn bump(f: impl FnOnce(&mut Churn)) {
    CHURN.with(|c| {
        let mut v = c.get();
        f(&mut v);
        c.set(v);
    });
}

/// This thread's counters so far.
pub fn snapshot() -> Churn {
    CHURN.with(Cell::get)
}

/// Counts a GPU buffer created.
pub fn buffer_created() {
    bump(|c| c.buffers_created += 1);
}

/// Counts a queue write (a wgpu staging buffer), here and in the frame
/// stats (`queue_writes`).
pub fn queue_write() {
    bump(|c| c.queue_writes += 1);
    timeline::phase::count(timeline::phase::Counter::QueueWrites, 1);
}

/// Counts a render or compute pass begun, in the frame stats (`passes`).
pub fn pass() {
    timeline::phase::count(timeline::phase::Counter::Passes, 1);
}

/// Counts a staging chunk taken from the pool.
pub fn staging_reused() {
    bump(|c| c.staging_reused += 1);
}

/// Counts a buffer taken from a pool.
pub fn buffer_reused() {
    bump(|c| c.buffers_reused += 1);
}

/// `device.create_buffer`, counted.
pub fn create_buffer(device: &wgpu::Device, desc: &wgpu::BufferDescriptor<'_>) -> wgpu::Buffer {
    buffer_created();
    device.create_buffer(desc)
}
