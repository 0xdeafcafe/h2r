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
    for slabs in [false, true] {
        // Room for about three frames of buffers (1280 vertex bytes in a 2
        // KiB class, 12 index bytes in a 256-byte one).
        let mut p = PersistentVertices::new(128 << 10);
        p.set_slabs(slabs);
        let mut scratch = Vec::new();
        let mut id = 0;
        let mut frame = |p: &mut PersistentVertices| {
            for _ in 0..16 {
                id += 1;
                let (b, at, bound) = p.get(&ctx, id, &plan, &guest, 64, &mut scratch);
                assert_eq!(bound, 1280);
                assert!(b.size() >= at + bound);
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
        if slabs {
            assert_eq!(p.buffers(), 1, "one slab holds them all");
        }
    }
}

/// Reads `size` bytes of `b` from `at` back.
fn read(ctx: &GfxContext, b: &wgpu::Buffer, at: u64, size: u64) -> Vec<u8> {
    let out = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(b, at, &out, 0, size);
    ctx.queue.submit([enc.finish()]);
    out.slice(..).map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    out.slice(..).get_mapped_range().unwrap().to_vec()
}

/// Streams 12 frames of 10 new vertex and index lists each through a
/// budget that evicts, and reads every one back right after its frame's
/// submit: the bytes a draw would bind. Returns them and the queue writes.
fn stream(ctx: &GfxContext, staged: bool, slabs: bool) -> (Vec<Vec<u8>>, u64, usize) {
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let plan = RepackPlan::new(&layout, &[20]).streams.remove(0);
    let mut scratch = Vec::new();
    let mut p = PersistentVertices::new(16 << 10);
    p.set_staged(staged);
    p.set_slabs(slabs);
    let before = gfx::churn::snapshot();
    let mut got = Vec::new();
    for frame in 0..12u64 {
        let mut bufs = Vec::new();
        for i in 0..10u64 {
            let id = frame * 100 + i;
            let guest: Vec<u8> = (0..20 * (3 + i as usize))
                .map(|b| (b as u64 * 7 + id) as u8)
                .collect();
            let (b, at, bound) = p.get(ctx, id, &plan, &guest, 3 + i as u32, &mut scratch);
            let indices: Vec<u32> = (0..=(i + frame) as u32).collect();
            let (ib, iat, isize) = p.put_indices(ctx, id, PrimitiveType::TriangleList, &indices);
            bufs.push((b, at, bound, ib, iat, isize));
        }
        // An id drawn again binds what it was given.
        let again = frame * 100 + 4;
        let guest: Vec<u8> = (0..20 * 7).map(|b| (b as u64 * 7 + again) as u8).collect();
        let (b, at, bound) = p.get(ctx, again, &plan, &guest, 7, &mut scratch);
        assert_eq!((&b, at, bound), (&bufs[4].0, bufs[4].1, bufs[4].2));
        p.flush(&ctx.queue);
        let copies = p.copy_commands(&ctx.device);
        assert_eq!(copies.is_some(), staged);
        ctx.queue.submit(copies);
        p.submitted();
        for (b, at, bound, ib, iat, isize) in bufs {
            got.push(read(ctx, &b, at, bound));
            got.push(read(ctx, &ib, iat, isize));
        }
    }
    let writes = (gfx::churn::snapshot() - before).queue_writes;
    (got, writes, p.buffers())
}

/// Staged writes (`set_staged`) and slab regions (`set_slabs`) hold the
/// bytes a queue write into a buffer per id holds, through evictions and
/// reuse; staged, one write per flush.
#[test]
fn staged_and_slab_regions_hold_what_queue_writes_hold() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("skipping GPU test: no adapter");
        return;
    };
    let (want, direct_writes, _) = stream(&ctx, false, false);
    assert_eq!(direct_writes, 12 * 20);
    for (staged, slabs) in [(true, false), (false, true), (true, true)] {
        let (got, writes, buffers) = stream(&ctx, staged, slabs);
        assert!(got == want, "staged {staged}, slabs {slabs}: bytes differ");
        assert_eq!(writes, if staged { 12 } else { 12 * 20 });
        if slabs {
            assert_eq!(buffers, 1);
        }
    }
}

/// A level's worth of snapshots: thousands of ids in a buffer each, a
/// handful of slabs with slabs on.
#[test]
fn slabs_keep_the_buffer_count_small() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("skipping GPU test: no adapter");
        return;
    };
    let layout = VertexLayout::from_fvf(fvf::XYZRHW | fvf::DIFFUSE).unwrap();
    let plan = RepackPlan::new(&layout, &[20]).streams.remove(0);
    let guest = vec![0x5Au8; 20 * 400];
    let mut scratch = Vec::new();
    let mut count = |slabs: bool| {
        let mut p = PersistentVertices::new(32 << 20);
        p.set_slabs(slabs);
        p.set_staged(true);
        for frame in 0..10u64 {
            for i in 0..600u64 {
                let id = frame * 1000 + i;
                let n = 8 + (i * 37 % 393) as u32;
                p.get(&ctx, id, &plan, &guest, n, &mut scratch);
                let indices: Vec<u32> = (0..n).collect();
                p.put_indices(&ctx, id, PrimitiveType::TriangleList, &indices);
            }
            p.flush(&ctx.queue);
            ctx.queue.submit(p.copy_commands(&ctx.device));
            p.submitted();
        }
        p.buffers()
    };
    let own = count(false);
    let slabs = count(true);
    eprintln!("buffers held: {own} one per region, {slabs} in slabs");
    assert!(own > 5000, "{own}");
    assert!(slabs <= 12, "{slabs}");
}
