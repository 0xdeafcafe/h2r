//! What a queued buffer write costs on the CPU, by staging size: the render
//! thread's upload rings used to open a 2 MiB `write_buffer_with` view per
//! ring per submission, whatever they held. Ignored (a timing, not a check):
//!
//! ```sh
//! cargo test --release -p gfx --test staging_cost -- --ignored --nocapture
//! ```

use gfx::GfxContext;

fn bench(ctx: &GfxContext, view: u64, used: u64, submits: usize, writes_per_submit: usize) -> f64 {
    let target = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("staging cost target"),
        size: 4 << 20,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::UNIFORM,
        mapped_at_creation: false,
    });
    let bytes = vec![7u8; used as usize];
    let start = std::time::Instant::now();
    for _ in 0..submits {
        for w in 0..writes_per_submit {
            let mut v = ctx
                .queue
                .write_buffer_with(
                    &target,
                    (w as u64 * view) % (2 << 20),
                    wgpu::BufferSize::new(view).unwrap(),
                )
                .unwrap();
            v.slice(..used as usize).copy_from_slice(&bytes);
        }
        let enc = ctx.device.create_command_encoder(&Default::default());
        ctx.queue.submit([enc.finish()]);
    }
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    start.elapsed().as_secs_f64() * 1e6 / (submits * writes_per_submit) as f64
}

#[test]
#[ignore = "timing; run with --ignored --nocapture"]
fn queued_write_cost_by_staging_size() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("skipping: no adapter");
        return;
    };
    // Warm up.
    bench(&ctx, 4096, 4096, 50, 3);
    for round in 0..2 {
        for (view, used) in [
            (2u64 << 20, 16 << 10),
            (16 << 10, 16 << 10),
            (4096, 4096),
            (256, 256),
        ] {
            let us = bench(&ctx, view, used, 300, 3);
            eprintln!(
                "round {round}: staging view {view:>8} bytes, {used:>6} written: {us:7.1} us per write (incl. its share of the submit)"
            );
        }
    }
}

/// A flush hands the queue one write of exactly the bytes pushed since the
/// last one (alignment gaps included), and they land where `push` said.
#[test]
fn a_ring_flush_is_one_exact_write() {
    let Ok(ctx) = GfxContext::headless() else {
        eprintln!("skipping: no adapter");
        return;
    };
    let mut ring = gfx::ring::UploadRing::new(
        &ctx.device,
        "test ring",
        wgpu::BufferUsages::COPY_SRC,
        1 << 12,
    );
    let before = gfx::churn::snapshot();
    let pushes: Vec<Vec<u8>> = (0..5u8).map(|i| vec![i + 1; 10 + i as usize * 7]).collect();
    let at: Vec<(wgpu::Buffer, u64)> = pushes
        .iter()
        .map(|p| ring.push(&ctx.device, &ctx.queue, p, 256))
        .collect();
    assert_eq!(
        (gfx::churn::snapshot() - before).queue_writes,
        0,
        "nothing queued before the flush"
    );
    ring.flush(&ctx.queue);
    ring.flush(&ctx.queue);
    assert_eq!(
        (gfx::churn::snapshot() - before).queue_writes,
        1,
        "one write per flush, none when empty"
    );
    let read = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: 1 << 12,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_buffer_to_buffer(ring.buffer(), 0, &read, 0, 1 << 12);
    ctx.queue.submit([enc.finish()]);
    ring.reset();
    read.slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let bytes = read.slice(..).get_mapped_range().unwrap().to_vec();
    for (p, (_, off)) in pushes.iter().zip(&at) {
        assert_eq!(off % 256, 0);
        assert_eq!(&bytes[*off as usize..*off as usize + p.len()], &p[..]);
    }
    // After the reset the ring starts again at 0.
    let (_, off) = ring.push(&ctx.device, &ctx.queue, &[9; 4], 256);
    assert_eq!(off, 0);
    ring.flush(&ctx.queue);
}
