//! Persistent vertex and index buffers are reused through size classes
//! once a submit has passed: streaming new snapshots through a small
//! budget creates buffers only until the pool is warm. Synthetic data;
//! skipped without a GPU.

use gfx::GfxContext;
use gfx::persistent::PersistentVertices;
use gfx::repack::RepackPlan;
use gfx::vertex::{PrimitiveType, VertexLayout, fvf};

#[test]
fn evicted_buffers_come_back_after_a_submit() {
    let ctx = match GfxContext::headless() {
        Ok(ctx) => ctx,
        Err(e) => {
            eprintln!("skipping GPU test: {e}");
            return;
        }
    };
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let plan = RepackPlan::new(&layout, &[20]).streams.remove(0);
    let guest = vec![0x3Fu8; 20 * 64];
    // Room for about three frames of buffers (1280 vertex bytes in a 2 KiB
    // class, 12 index bytes in a 256-byte one).
    let mut p = PersistentVertices::new(128 << 10);
    let mut scratch = Vec::new();
    let mut id = 0;
    let mut frame = |p: &mut PersistentVertices| {
        for _ in 0..16 {
            id += 1;
            let (b, bound) = p.get(&ctx, id, &plan, &guest, 64, &mut scratch);
            assert_eq!(bound, 1280);
            assert!(b.size() >= bound);
            p.put_indices(&ctx, id, PrimitiveType::TriangleList, &[0, 1, 2]);
        }
        ctx.queue.submit([]);
        p.submitted();
    };
    for _ in 0..4 {
        frame(&mut p);
    }
    let before = gfx::churn::snapshot();
    for _ in 0..8 {
        frame(&mut p);
    }
    let c = gfx::churn::snapshot() - before;
    // Without the pool every one of the 256 would be created.
    assert_eq!(c.buffers_created + c.buffers_reused, 8 * 32, "{c:?}");
    assert!(c.buffers_created * 8 <= c.buffers_reused, "{c:?}");
    assert!(p.stats().held_bytes <= 128 << 10);
}

/// Reads `size` bytes of `b` back.
fn read(ctx: &GfxContext, b: &wgpu::Buffer, size: u64) -> Vec<u8> {
    let out = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(b, 0, &out, 0, size);
    ctx.queue.submit([enc.finish()]);
    out.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    out.slice(..).get_mapped_range().unwrap().to_vec()
}

/// Staged writes (`set_staged`) fill the buffers with the bytes a queue
/// write per buffer gives, through one write per flush and the copies.
#[test]
fn staged_buffers_hold_what_queue_writes_hold() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("skipping GPU test: no adapter");
        return;
    };
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let plan = RepackPlan::new(&layout, &[20]).streams.remove(0);
    let mut scratch = Vec::new();
    let mut run = |staged: bool| {
        let mut p = PersistentVertices::new(128 << 20);
        p.set_staged(staged);
        let before = gfx::churn::snapshot();
        let mut got = Vec::new();
        for frame in 0..3u64 {
            let mut bufs = Vec::new();
            for i in 0..10u64 {
                let id = frame * 100 + i;
                let guest: Vec<u8> = (0..20 * (3 + i as usize))
                    .map(|b| (b as u64 * 7 + id) as u8)
                    .collect();
                let (b, bound) = p.get(&ctx, id, &plan, &guest, 3 + i as u32, &mut scratch);
                let indices: Vec<u32> = (0..=i as u32).collect();
                let (ib, isize) = p.put_indices(&ctx, id, PrimitiveType::TriangleList, &indices);
                bufs.push((b, bound, ib, isize));
            }
            p.flush(&ctx.queue);
            let copies = p.copy_commands(&ctx.device);
            assert_eq!(copies.is_some(), staged);
            ctx.queue.submit(copies);
            p.submitted();
            for (b, bound, ib, isize) in bufs {
                got.push(read(&ctx, &b, bound));
                got.push(read(&ctx, &ib, isize));
            }
        }
        (got, (gfx::churn::snapshot() - before).queue_writes)
    };
    let (want, direct_writes) = run(false);
    let (got, staged_writes) = run(true);
    assert!(got == want, "staged buffers differ");
    assert_eq!(direct_writes, 3 * 20);
    assert_eq!(staged_writes, 3, "one staging write per flush");
}
