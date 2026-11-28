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
    /// GPU buffers created by `gfx`:
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
    /// Render passes of counted draws that wrote nothing, left out.
    pub idle_counted_skipped: u64,
}

impl std::ops::Sub for Churn {
    type Output = Churn;
    fn sub(self, o: Churn) -> Churn {
        Churn {
            buffers_created: self.buffers_created - o.buffers_created,
            queue_writes: self.queue_writes - o.queue_writes,
            staging_reused: self.staging_reused - o.staging_reused,
            buffers_reused: self.buffers_reused - o.buffers_reused,
            idle_counted_skipped: self.idle_counted_skipped - o.idle_counted_skipped,
        }
    }
}

thread_local! {
    static CHURN: Cell<Churn> = const { Cell::new(Churn {
        buffers_created: 0,
        queue_writes: 0,
        staging_reused: 0,
        buffers_reused: 0,
        idle_counted_skipped: 0,
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
    timeline::native::resource(timeline::native::Metric::Buffers, 1);
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

/// [`pass`], one that is side work (`pass_side`).
pub fn side_pass() {
    pass();
    timeline::phase::count(timeline::phase::Counter::PassSide, 1);
}

/// `n` of the passes counted are a visibility test's (`pass_counted`).
pub fn counted_passes(n: u64) {
    timeline::phase::count(timeline::phase::Counter::PassCounted, n);
}

/// Counts a counted draw's render pass left out (it wrote nothing).
pub fn idle_counted_skipped() {
    bump(|c| c.idle_counted_skipped += 1);
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

/// Resource operations used by native stages. Count at creation, including
/// resources discarded when one copy refuses the replacement.
pub fn create_buffer_init(device: &wgpu::Device, desc: &wgpu::util::BufferInitDescriptor<'_>) -> wgpu::Buffer {
    use wgpu::util::DeviceExt;
    buffer_created();
    upload_bytes(desc.contents.len());
    device.create_buffer_init(desc)
}

pub fn create_bind_group(device: &wgpu::Device, desc: &wgpu::BindGroupDescriptor<'_>) -> wgpu::BindGroup {
    timeline::native::resource(timeline::native::Metric::BindGroups, 1);
    device.create_bind_group(desc)
}

pub fn create_render_pipeline(device: &wgpu::Device, desc: &wgpu::RenderPipelineDescriptor<'_>) -> wgpu::RenderPipeline {
    timeline::native::resource(timeline::native::Metric::PipelineMisses, 1);
    device.create_render_pipeline(desc)
}

pub fn upload_bytes(n: usize) {
    timeline::native::resource(timeline::native::Metric::UploadBytes, n as u64);
}

/// Result of looking up resident geometry and material resources for a native draw.
pub fn native_cache(hit: bool) {
    use timeline::native::Metric;
    timeline::native::resource(if hit { Metric::CacheHits } else { Metric::CacheMisses }, 1);
}
