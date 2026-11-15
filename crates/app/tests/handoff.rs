//! GPU tests for the frame handoff and the compositor. They need an adapter
//! (Metal on a Mac) and skip with a message when there is none.

use app::frame::{self, SLOT_FORMAT};
use app::{AppConfig, Compositor, Control, Filter, GuestHost, PacePolicy, Viewport};
use gfx::readback::read_texture;
use gfx::surface::{SurfaceDesc, SurfaceKind, SurfaceStore};
use gfx::{Clear, D3DFormat, GfxContext, OffscreenTarget, Renderer};

fn gpu() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(c) => Some(c),
        Err(e) => {
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

fn solid(size: (u32, u32), rgba: [u8; 4]) -> Vec<u8> {
    rgba.repeat((size.0 * size.1) as usize)
}

#[test]
fn cpu_frames_arrive_intact() {
    let Some(ctx) = gpu() else { return };
    let (mut sink, source) = frame::channel(&ctx);
    assert!(source.latest().is_none(), "nothing before the first frame");

    let size = (16, 8);
    let pixels: Vec<u8> = (0..size.0 * size.1)
        .flat_map(|i| [i as u8, (i * 3) as u8, 0x40, 0xff])
        .collect();
    assert_eq!(sink.submit_rgba(size, &pixels).unwrap(), 1);
    let f = source.latest().expect("a frame");
    assert!(f.new);
    assert_eq!((f.size, f.seq), (size, 1));
    assert_eq!(
        read_texture(&ctx, &f.texture, wgpu::TextureAspect::All, 4, size),
        pixels
    );

    // No new frame: the same one again, not new.
    let again = source.latest().unwrap();
    assert!(!again.new);
    assert_eq!(again.seq, 1);
}

#[test]
fn newest_frame_wins_and_drops_are_counted() {
    let Some(ctx) = gpu() else { return };
    let (mut sink, source) = frame::channel(&ctx);
    let size = (4, 4);
    for (i, c) in [[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]
        .into_iter()
        .enumerate()
    {
        assert_eq!(
            sink.submit_rgba(size, &solid(size, c)).unwrap(),
            i as u64 + 1
        );
    }
    let f = source.latest().unwrap();
    assert_eq!(f.seq, 3);
    assert_eq!(
        read_texture(&ctx, &f.texture, wgpu::TextureAspect::All, 4, size),
        solid(size, [0, 0, 255, 255])
    );
    let s = source.stats();
    assert_eq!((s.published, s.shown, s.dropped), (3, 1, 2));
}

/// The frame on screen must stay intact while the guest keeps publishing:
/// a back slot that aliased the front texture would overwrite it.
#[test]
fn the_front_frame_is_never_written_while_held() {
    let Some(ctx) = gpu() else { return };
    let (mut sink, source) = frame::channel(&ctx);
    let size = (8, 8);
    let red = solid(size, [255, 0, 0, 255]);
    sink.submit_rgba(size, &red).unwrap();
    let held = source.latest().unwrap();
    for (i, c) in [
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 0, 255],
        [0, 255, 255, 255],
    ]
    .into_iter()
    .enumerate()
    {
        sink.submit_rgba(size, &solid(size, c)).unwrap();
        assert_eq!(
            read_texture(&ctx, &held.texture, wgpu::TextureAspect::All, 4, size),
            red,
            "front overwritten by publish {}",
            i + 2
        );
    }
    // And through the GPU blit path (the present hook's), not only uploads.
    let mut store = SurfaceStore::new(ctx.caps);
    let surf = store
        .create(
            &ctx,
            SurfaceDesc::new(D3DFormat::X8R8G8B8, 8, 8),
            SurfaceKind::Color,
        )
        .unwrap();
    for _ in 0..3 {
        sink.submit_surface(&store, surf).unwrap();
        assert_eq!(
            read_texture(&ctx, &held.texture, wgpu::TextureAspect::All, 4, size),
            red
        );
    }
    let next = source.latest().unwrap();
    assert_ne!(
        next.texture, held.texture,
        "a new frame lands in a different slot"
    );
}

#[test]
fn bad_frames_are_rejected() {
    let Some(ctx) = gpu() else { return };
    let (mut sink, source) = frame::channel(&ctx);
    assert!(sink.submit_rgba((0, 4), &[]).is_err());
    assert!(sink.submit_rgba((4, 4), &[0; 12]).is_err());
    assert!(source.latest().is_none());
    assert_eq!(source.stats().published, 0);
}

#[test]
fn size_changes_reallocate_slots() {
    let Some(ctx) = gpu() else { return };
    let (mut sink, source) = frame::channel(&ctx);
    for size in [(640, 480), (720, 480), (640, 480), (320, 240)] {
        sink.submit_rgba(size, &solid(size, [9, 9, 9, 255]))
            .unwrap();
        let f = source.latest().unwrap();
        assert_eq!(f.size, size);
        assert_eq!(f.texture.width(), size.0);
    }
}

#[test]
fn wakes_once_per_batch() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, Ordering};
    let Some(ctx) = gpu() else { return };
    let (mut sink, source) = frame::channel(&ctx);
    let wakes = Arc::new(AtomicU32::new(0));
    let w = Arc::clone(&wakes);
    source.set_waker(move || {
        w.fetch_add(1, Ordering::SeqCst);
    });
    let size = (2, 2);
    for _ in 0..5 {
        sink.submit_rgba(size, &solid(size, [1, 2, 3, 4])).unwrap();
    }
    assert_eq!(
        wakes.load(Ordering::SeqCst),
        1,
        "no wake until acknowledged"
    );
    source.ack_wake();
    sink.submit_rgba(size, &solid(size, [1, 2, 3, 4])).unwrap();
    assert_eq!(wakes.load(Ordering::SeqCst), 2);
}

#[test]
fn concurrent_producer_and_consumer() {
    let Some(ctx) = gpu() else { return };
    let (mut sink, source) = frame::channel(&ctx);
    let size = (8, 8);
    let n = 200u32;
    let producer = std::thread::spawn(move || {
        for i in 0..n {
            sink.submit_rgba(size, &solid(size, [i as u8, 0, 0, 255]))
                .unwrap();
        }
    });
    let mut last = 0;
    while !producer.is_finished() {
        if let Some(f) = source.latest() {
            assert!(f.seq >= last, "sequence never goes backwards");
            last = f.seq;
        }
    }
    producer.join().unwrap();
    let f = source.latest().unwrap();
    assert_eq!(f.seq, n as u64);
    // The slot holds the last frame's content.
    let px = read_texture(&ctx, &f.texture, wgpu::TextureAspect::All, 4, size);
    assert_eq!(&px[..4], &[(n - 1) as u8, 0, 0, 255]);
    let s = source.stats();
    assert_eq!(s.published, n as u64);
    assert_eq!(s.shown + s.dropped, n as u64);
}

/// The D3D path: a gfx colour surface goes through the present hook.
#[test]
fn present_hook_copies_the_back_buffer() {
    let Some(ctx) = gpu() else { return };
    let (sink, source) = frame::channel(&ctx);
    let mut hook = sink.into_present_hook();
    let mut store = SurfaceStore::new(ctx.caps);
    let mut renderer = Renderer::new(&ctx);
    let mut clear_to = |store: &SurfaceStore, id, color: [f32; 4]| {
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        {
            let target = store.target(id, None).unwrap();
            renderer
                .render(&ctx, &mut enc, &target, Clear::all(color), &[])
                .unwrap();
        }
        ctx.queue.submit([enc.finish()]);
    };

    // A8R8G8B8 keeps alpha.
    let argb = store
        .create(
            &ctx,
            SurfaceDesc::new(D3DFormat::A8R8G8B8, 64, 48),
            SurfaceKind::Color,
        )
        .unwrap();
    clear_to(&store, argb, [1.0, 0.0, 0.0, 0.5]);
    hook(&ctx, &store, argb, 0, None);
    let f = source.latest().unwrap();
    assert_eq!(f.size, (64, 48));
    let px = read_texture(&ctx, &f.texture, wgpu::TextureAspect::All, 4, f.size);
    assert_eq!(&px[..3], &[255, 0, 0]);
    assert!((126..=129).contains(&px[3]), "alpha {}", px[3]);

    // X8R8G8B8 is shown opaque whatever the alpha channel holds.
    let xrgb = store
        .create(
            &ctx,
            SurfaceDesc::new(D3DFormat::X8R8G8B8, 640, 480),
            SurfaceKind::Color,
        )
        .unwrap();
    clear_to(&store, xrgb, [0.0, 1.0, 0.0, 0.0]);
    hook(&ctx, &store, xrgb, 0, None);
    let f = source.latest().unwrap();
    assert_eq!(f.size, (640, 480));
    let px = read_texture(&ctx, &f.texture, wgpu::TextureAspect::All, 4, f.size);
    assert_eq!(&px[..4], &[0, 255, 0, 255]);
    let mut ramp = [0u8; 768];
    ramp[255] = 37;
    ramp[256] = 81;
    ramp[512] = 129;
    clear_to(&store, argb, [1.0, 0.0, 0.0, 0.5]);
    hook(&ctx, &store, argb, 0, Some(&ramp));
    let f = source.latest().unwrap();
    let px = read_texture(&ctx, &f.texture, wgpu::TextureAspect::All, 4, f.size);
    assert_eq!(&px[..3], &[37, 81, 129]);
    assert!((126..=129).contains(&px[3]));
    assert_eq!(source.stats().published, 3);
}

fn composite(
    ctx: &GfxContext,
    src: &[u8],
    src_size: (u32, u32),
    dst_size: (u32, u32),
    vp: Viewport,
) -> Vec<u8> {
    let (mut sink, source) = frame::channel(ctx);
    sink.submit_rgba(src_size, src).unwrap();
    let f = source.latest().unwrap();
    let target = OffscreenTarget::new(&ctx.device, dst_size, SLOT_FORMAT);
    let mut comp = Compositor::new(&ctx.device);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let t = target.target();
    comp.draw(
        &ctx.device,
        &mut enc,
        Some(&f.view),
        t.color,
        t.color_format,
        vp,
        Filter::Nearest,
        wgpu::Color::BLACK,
    );
    ctx.queue.submit([enc.finish()]);
    target.read_pixels(ctx)
}

#[test]
fn compositor_pillarboxes_a_4_3_frame() {
    let Some(ctx) = gpu() else { return };
    // A 4:3 white frame in a 16:9-ish target: black bars left and right.
    let src = solid((64, 48), [255, 255, 255, 0]);
    let dst = (128, 48);
    let vp = app::scale::place(
        (64, 48),
        app::DisplayAspect::Standard,
        app::Scaling::Fit,
        dst,
    );
    assert_eq!(
        vp,
        Viewport {
            x: 32,
            y: 0,
            width: 64,
            height: 48
        }
    );
    let px = composite(&ctx, &src, (64, 48), dst, vp);
    let at = |x: u32, y: u32| &px[((y * dst.0 + x) * 4) as usize..][..4];
    for y in [0, 24, 47] {
        assert_eq!(at(0, y), [0, 0, 0, 255], "left bar");
        assert_eq!(at(31, y), [0, 0, 0, 255], "left bar edge");
        assert_eq!(
            at(32, y),
            [255, 255, 255, 255],
            "image (alpha forced opaque)"
        );
        assert_eq!(at(95, y), [255, 255, 255, 255], "image edge");
        assert_eq!(at(96, y), [0, 0, 0, 255], "right bar");
    }
}

#[test]
fn compositor_keeps_orientation_and_scales() {
    let Some(ctx) = gpu() else { return };
    // 2x2 source: red green / blue white, scaled 4x with nearest filtering.
    let src = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 255, 255, 255],
    ]
    .concat();
    let dst = (8, 8);
    let px = composite(
        &ctx,
        &src,
        (2, 2),
        dst,
        Viewport {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        },
    );
    let at = |x: u32, y: u32| &px[((y * dst.0 + x) * 4) as usize..][..4];
    assert_eq!(at(1, 1), [255, 0, 0, 255], "top-left stays top-left");
    assert_eq!(at(6, 1), [0, 255, 0, 255]);
    assert_eq!(at(1, 6), [0, 0, 255, 255]);
    assert_eq!(at(6, 6), [255, 255, 255, 255]);
}

#[test]
fn compositor_without_a_frame_clears() {
    let Some(ctx) = gpu() else { return };
    let target = OffscreenTarget::new(&ctx.device, (4, 4), SLOT_FORMAT);
    let mut comp = Compositor::new(&ctx.device);
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    let t = target.target();
    let grey = wgpu::Color {
        r: 0.5,
        g: 0.5,
        b: 0.5,
        a: 1.0,
    };
    comp.draw(
        &ctx.device,
        &mut enc,
        None,
        t.color,
        t.color_format,
        Viewport::default(),
        Filter::Linear,
        grey,
    );
    ctx.queue.submit([enc.finish()]);
    let px = target.read_pixels(&ctx);
    assert!(
        px.chunks(4)
            .all(|p| (127..=128).contains(&p[0]) && p[3] == 255)
    );
}

#[test]
fn headless_host_exposes_the_guest_interface() {
    if gpu().is_none() {
        return;
    }
    let config = AppConfig {
        gamepads: false,
        pacing: PacePolicy::Unpaced,
        ..AppConfig::default()
    };
    let mut host = GuestHost::headless(&config).unwrap();
    // The shared context is the same device.
    assert!(host.gfx_context().device == host.gfx_context().device);
    assert!(host.take_input().is_some());
    assert!(host.take_input().is_none(), "input is handed out once");
    assert!(host.keyboard().is_some());
    assert_eq!(host.sample_rate(), app::audio_out::FALLBACK_RATE);
    let _hook = host.present_hook();
    let mut sink = host.frame_sink();
    sink.submit_rgba((2, 2), &[0; 16]).unwrap();
    assert_eq!(host.frame_stats().published, 1);
    assert_eq!(host.tick(0), Control::Continue);
    assert_eq!(
        host.tick(10_000_000_000),
        Control::Continue,
        "unpaced never sleeps"
    );
    host.lifecycle().request_stop();
    assert!(host.should_stop());
    assert_eq!(host.tick(0), Control::Stop);
}

#[test]
fn real_time_tick_sleeps_to_wall_time() {
    if gpu().is_none() {
        return;
    }
    let config = AppConfig {
        gamepads: false,
        ..AppConfig::default()
    };
    let mut host = GuestHost::headless(&config).unwrap();
    let t = std::time::Instant::now();
    host.tick(0);
    // Ten 60 Hz frames of guest time.
    for f in 1..=10u64 {
        host.tick(f * 166_667);
    }
    let e = t.elapsed();
    assert!(e >= std::time::Duration::from_millis(160), "{e:?}");
    assert!(e < std::time::Duration::from_millis(400), "{e:?}");
    assert!(host.pace_stats().sleeps >= 9);
}
