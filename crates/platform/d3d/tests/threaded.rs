//! The render thread (`d3d::ThreadedBackend`): the same device sequence
//! through a backend directly and through the render thread must give the
//! same backend calls, the same guest memory and the same device stats.
//! Synthetic resources.

mod common;

use common::*;
use d3d::{
    Backend, Device, GuestMemory, RecordingBackend, SparseMemory, ThreadedBackend,
    UploadPolicy,
};
use gfx::vertex::fvf;

const RS_SRCBLEND: u32 = 62;
const RS_DESTBLEND: u32 = 63;
const RECTS: u32 = MISC + 0x7100;
const TEX_A: u32 = MISC + 0x4000;
const LEVEL_A: u32 = MISC + 0x4200;
const DATA_A: u32 = 0x0030_0000;
const SIZE: u32 = 32;
const LEN: usize = (SIZE * SIZE * 4) as usize;

/// A linear 32x32 `LIN_A8R8G8B8` texture with one level over `DATA_A`.
fn level(m: &mut SparseMemory) {
    m.map(0x8000_0000 | DATA_A, LEN as u32);
    m.write(0x8000_0000 | DATA_A, &vec![0x30; LEN]).unwrap();
    let format = (2 << 4) | (0x12 << 8) | (1 << 16);
    let size = (SIZE - 1) | ((SIZE - 1) << 12) | ((128 / 64 - 1) << 24);
    for (i, w) in [1 | (4 << 16) | 0x0100_0000, DATA_A, 0, format, size]
        .iter()
        .enumerate()
    {
        m.write_u32(TEX_A + 4 * i as u32, *w).unwrap();
    }
    for (i, w) in [1 | (5 << 16) | 0x0100_0000, DATA_A, 0, format, size, TEX_A]
        .iter()
        .enumerate()
    {
        m.write_u32(LEVEL_A + 4 * i as u32, *w).unwrap();
    }
}

fn textured_quad<B: Backend>(d: &mut Device<B>, m: &mut SparseMemory, rect: [f32; 4], color: u32) {
    let [x0, y0, x1, y1] = rect;
    let mut vb = Vec::new();
    for (u, v) in [(0.0f32, 0.0f32), (1.0, 0.0), (0.0, 1.0), (1.0, 1.0)] {
        for f in [x0 + (x1 - x0) * u, y0 + (y1 - y0) * v, 0.5, 1.0] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
        vb.extend_from_slice(&color.to_le_bytes());
        for f in [SIZE as f32 * u, SIZE as f32 * v] {
            vb.extend_from_slice(&f.to_le_bytes());
        }
    }
    m.write(0x8000_0000 | VB_DATA, &vb).unwrap();
    m.write_u32(VB_HEADER, 1 | 0x0100_0000).unwrap();
    m.write_u32(VB_HEADER + 4, VB_DATA).unwrap();
    m.write_u32(VB_HEADER + 8, 0).unwrap();
    d.set_vertex_shader(m, fvf::XYZRHW | fvf::DIFFUSE | 0x100)
        .unwrap();
    d.set_stream_source(m, 0, VB_HEADER, 28).unwrap();
    d.draw_vertices(m, TRIANGLESTRIP, 0, 4).unwrap();
}

/// `GetVisibilityTestResult` asked until it answers, as a game that spins
/// on `D3DERR_TESTINCOMPLETE` would. The answer, which must not depend on
/// how many times it was incomplete.
fn poll_visibility<B: Backend>(
    d: &mut Device<B>,
    m: &mut SparseMemory,
    index: u32,
) -> d3d::device::VisibilityAnswer {
    loop {
        if let Some(a) = d.get_visibility_test_result(m, index).unwrap() {
            return a;
        }
        std::thread::yield_now();
    }
}

/// The device's stats without the count of incomplete visibility answers,
/// which depends on the render thread's timing.
fn timeless(s: d3d::Stats) -> d3d::Stats {
    d3d::Stats {
        visibility_incomplete: 0,
        ..s
    }
}

/// Render-to-texture into level A, sampled in place and through memory,
/// a guest store into the level, waits that write it back, fences, a
/// visibility test, a swap, and the frame drawn again. Returns the device
/// for the caller to read the back buffer, and every value the guest
/// could observe along the way: entry results, the device's guest words
/// (GPU time, completion word, swap count and fences, register window)
/// and the level's bytes after each wait.
fn scenario<B: Backend>(backend: B) -> (Device<B>, SparseMemory, Vec<String>) {
    let mut seen = Vec::new();
    let dev = d3d::GuestLayout::HALO2.device;
    let (mut d, mut m) = create(backend, UploadPolicy::Tracked);
    set_states(&mut d, &mut m);
    level(&mut m);
    let back = m
        .read_u32(d3d::GuestLayout::HALO2.device + d3d::layout::device_field::RENDER_TARGET)
        .unwrap();
    for frame in 0..3u32 {
        d.clear(&mut m, 0, 0, 0xF0, 0xFF00_0000 | frame, 1.0, 0)
            .unwrap();
        d.set_render_target(&mut m, LEVEL_A, 0).unwrap();
        m.write(RECTS, &[0u32, 0, 16, 32].map(u32::to_le_bytes).concat())
            .unwrap();
        d.clear(&mut m, 1, RECTS, 0xF0, 0xFFFF_0000 ^ frame, 1.0, 0)
            .unwrap();
        d.set_render_target(&mut m, back, 0).unwrap();
        d.set_texture(&mut m, 0, TEX_A).unwrap();
        for (slot, v) in [(RS_ALPHABLENDENABLE, 1), (RS_SRCBLEND, 0x302), (RS_DESTBLEND, 0x303)] {
            d.set_render_state(&mut m, slot, v).unwrap();
        }
        textured_quad(&mut d, &mut m, [0.0, 0.0, 32.0, 32.0], 0xC0FF_FFFF);
        d.set_render_state(&mut m, RS_ALPHABLENDENABLE, 0).unwrap();
        // The guest stores into the level, then waits on it (write-back).
        m.write(0x8000_0000 | DATA_A, &[0x77; 64]).unwrap();
        d.note_write(0x8000_0000 | DATA_A, 64);
        d.block_on_resource(&mut m, TEX_A).unwrap();
        seen.push(format!("level {:?}", m.read_vec(0x8000_0000 | DATA_A, LEN).unwrap()));
        textured_quad(&mut d, &mut m, [32.0, 0.0, 64.0, 32.0], 0xFFFF_FFFF);
        d.end_visibility_test(&mut m, frame).unwrap();
        let fence = d.set_fence(&mut m).unwrap();
        d.block_on_time(&mut m).unwrap();
        let busy = d.resource_busy(&mut m, TEX_A).unwrap();
        let visible = poll_visibility(&mut d, &mut m, frame);
        seen.push(format!(
            "fence {fence} completed {} busy {busy} visible {visible:?}",
            d.gpu_completed()
        ));
        d.swap(&mut m, 0).unwrap();
        d.kick_off(&mut m).unwrap();
        seen.push(format!("device {:?}", m.read_vec(dev, 0x2000).unwrap()));
    }
    textured_quad(&mut d, &mut m, [0.0, 32.0, 32.0, 64.0], 0xFFFF_FFFF);
    (d, m, seen)
}

#[test]
fn the_render_thread_makes_the_same_backend_calls() {
    let recorder = || {
        let mut r = RecordingBackend::new();
        r.sample_surfaces = true;
        r
    };
    let (direct, queued) = (recorder(), recorder());
    let (mut a, mut am, seen_a) = scenario(direct);
    let (mut b, mut bm, seen_b) = scenario(ThreadedBackend::new(queued).unwrap());
    assert_eq!(seen_a, seen_b, "what the guest observed");
    let s = a.stats();
    assert!(s.rtt_writebacks > 0 && s.rtt_direct_samples > 0, "{s:?}");
    a.flush_render_targets(&mut am).unwrap();
    b.flush_render_targets(&mut bm).unwrap();
    assert_eq!(a.take_events(), b.take_events());
    assert_eq!(timeless(a.stats()), timeless(b.stats()));
    let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
    assert!(calls.len() > 20, "{calls:?}");
    assert_eq!(a.backend.calls, calls);
    assert_eq!(
        am.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
        bm.read_vec(0x8000_0000 | DATA_A, LEN).unwrap()
    );
    assert_eq!(b.backend.misses(), 0);
}

#[test]
fn a_wrong_speculation_stops_the_next_call() {
    let mut inner = RecordingBackend::new();
    inner.no_copies = true;
    let mut t = ThreadedBackend::new(inner).unwrap();
    let s = t
        .create_surface(gfx::D3DFormat::A8R8G8B8, 4, 4, false)
        .unwrap();
    let u = t
        .create_surface(gfx::D3DFormat::A8R8G8B8, 4, 4, false)
        .unwrap();
    // Answered as a backend that copies would; the inner one cannot.
    assert_eq!(t.copy_surface(s, u), Ok(true));
    t.finish().unwrap_err();
    assert_eq!(t.misses(), 1);
    let e = t.clear(s, None, gfx::Clear::NONE).unwrap_err();
    assert!(e.contains("copy_surface returned false"), "{e}");
}

#[test]
fn reads_wait_for_the_work_before_them_and_textures_are_freed() {
    let mut inner = RecordingBackend::new();
    inner.readback_fill = 0x5A;
    let mut t = ThreadedBackend::new(inner).unwrap();
    let s = t
        .create_surface(gfx::D3DFormat::A8R8G8B8, 4, 4, false)
        .unwrap();
    t.clear(s, None, gfx::Clear::NONE).unwrap();
    let bytes = t
        .read_surface(
            s,
            gfx::D3DFormat::A8R8G8B8,
            4,
            4,
            d3d::GuestLayoutKind::Linear { pitch: 16 },
        )
        .unwrap();
    assert_eq!(bytes, vec![0x5A; 64]);
    let calls = t.with_inner(|r| r.calls.len()).unwrap();
    assert_eq!(calls, 3, "create, clear, read in order");
    let ticket = t.report_pixel_count().unwrap().unwrap();
    assert_eq!(t.read_pixel_count(ticket), Ok(0));
    t.read_pixel_count(ticket).unwrap_err();
    for _ in 0..4 {
        t.present(s, 4).unwrap();
    }
    t.finish().unwrap();
    assert_eq!(t.misses(), 0);
}

/// A frame-buffer copy refilled through the render thread is a refill, in
/// queue order, of the texture the proxy's first copy made: nothing new is
/// made on the render thread.
#[test]
fn a_frame_copy_refill_reaches_the_texture_the_proxy_made() {
    use d3d::Recorded;
    let mut t = ThreadedBackend::new(RecordingBackend::new()).unwrap();
    let s = t
        .create_surface(gfx::D3DFormat::A8R8G8B8, 4, 4, false)
        .unwrap();
    let mut copy = t.snapshot_surface(s, false).unwrap().unwrap();
    t.clear(s, None, gfx::Clear::NONE).unwrap();
    for _ in 0..3 {
        assert!(t.snapshot_surface_into(s, true, &mut copy).unwrap());
    }
    let calls = t.with_inner(|r| r.calls.clone()).unwrap();
    let made: Vec<u32> = calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Snapshot { id, .. } => Some(*id),
            _ => None,
        })
        .collect();
    assert_eq!(made.len(), 1, "{calls:?}");
    let refills = &calls[calls.len() - 3..];
    assert!(
        refills.iter().all(|c| matches!(c, Recorded::SnapshotInto { id, opaque: true, .. } if *id == made[0])),
        "{calls:?}"
    );
    drop(copy);
    t.finish().unwrap();
    assert_eq!(t.misses(), 0);
}

/// The render thread's timing changes nothing the guest sees: with it
/// lagging by random amounts (three seeds) every observed value, the
/// backend calls and guest memory match the synchronous backend.
#[test]
fn render_thread_timing_changes_nothing_the_guest_sees() {
    let recorder = || {
        let mut r = RecordingBackend::new();
        r.sample_surfaces = true;
        r
    };
    let (mut a, mut am, seen_a) = scenario(recorder());
    a.flush_render_targets(&mut am).unwrap();
    for seed in [1, 0x9e37_79b9, 0xdead_beef] {
        let t = ThreadedBackend::new(recorder()).unwrap().with_jitter(seed);
        let (mut b, mut bm, seen_b) = scenario(t);
        b.flush_render_targets(&mut bm).unwrap();
        assert_eq!(seen_a, seen_b, "seed {seed:#x}");
        assert_eq!(timeless(a.stats()), timeless(b.stats()), "seed {seed:#x}");
        let calls = b.backend.with_inner(|r| r.calls.clone()).unwrap();
        assert_eq!(a.backend.calls, calls, "seed {seed:#x}");
        assert_eq!(
            am.read_vec(0x8000_0000 | DATA_A, LEN).unwrap(),
            bm.read_vec(0x8000_0000 | DATA_A, LEN).unwrap()
        );
    }
}

#[test]
fn queued_and_repeated_frames_keep_their_own_mark_bounds_and_view_lenses() {
    use d3d::backend::{DrawOutcome, ResolvedDraw};
    use d3d::present_camera::{self, Image, Present, View};
    use frame_scene::{RenderCamera, transform};
    use gfx::texture::DecodedTexture;
    use gfx::world_lines::{ImageMarks, ImageSamples, WorldLine};

    #[derive(Clone, Debug, PartialEq)]
    struct Sample {
        camera: f32,
        points: [[f32; 3]; 2],
        lens: [f32; 2],
        projected_x: [[f32; 2]; 2],
    }

    #[derive(Debug)]
    struct Line([f32; 3]);

    impl ImageMarks for Line {
        fn lines(&self, visit: &mut dyn FnMut(WorldLine<'_>)) {
            let [x, y, z] = self.0;
            visit(WorldLine {
                edges: &[[self.0, [x, y, z + 1.0]]],
                anchor: self.0,
                color: [1.0, 0.0, 0.0],
                dashed: false,
                label: None,
                style: Default::default(),
            });
        }
    }

    #[derive(Default)]
    struct Recorder {
        inner: RecordingBackend,
        samples: Vec<Option<Sample>>,
    }

    impl Backend for Recorder {
        type Surface = u32;
        type Texture = u32;

        fn create_surface(
            &mut self,
            format: gfx::D3DFormat,
            width: u32,
            height: u32,
            depth: bool,
        ) -> Result<u32, String> {
            self.inner.create_surface(format, width, height, depth)
        }

        fn destroy_surface(&mut self, surface: u32) -> Result<(), String> {
            self.inner.destroy_surface(surface)
        }

        fn upload_texture(&mut self, texture: &DecodedTexture) -> Result<u32, String> {
            self.inner.upload_texture(texture)
        }

        fn bc_supported(&self) -> bool {
            self.inner.bc_supported()
        }

        fn clear(&mut self, color: u32, depth: Option<u32>, clear: gfx::Clear) -> Result<(), String> {
            self.inner.clear(color, depth, clear)
        }

        fn draw(
            &mut self,
            _: u32,
            _: Option<u32>,
            _: &ResolvedDraw,
            _: [Option<&u32>; 4],
        ) -> Result<DrawOutcome, String> {
            Ok(DrawOutcome::Skipped("synthetic skipped draw".into()))
        }

        fn copy_surface(&mut self, src: u32, dst: u32) -> Result<bool, String> {
            self.inner.copy_surface(src, dst)
        }

        fn present(&mut self, surface: u32, flags: u32) -> Result<(), String> {
            let Some(Present::Guest { image, .. }) = present_camera::current() else {
                panic!("the queued image must be a guest present");
            };
            let Some(image) = image else {
                self.samples.push(None);
                return self.inner.present(surface, flags);
            };
            let samples = image.payload.downcast_ref::<ImageSamples>().unwrap();
            let views = [image.views[0].unwrap(), image.views[1].unwrap()];
            let mut points = Vec::new();
            let mut projected_x = Vec::new();
            samples.visit(&std::time::Instant::now, &mut |line| {
                let [x, y, z] = line.edges[0][0];
                points.push([x, y, z]);
                projected_x.push(views.map(|v| {
                    let clip = transform(&v.camera.view_projection(), [x, y, z, 1.0]);
                    clip[0] * v.horizontal_factor / clip[3]
                }));
            });
            self.samples.push(Some(Sample {
                camera: views[0].camera.position[0],
                points: points.try_into().unwrap(),
                lens: views.map(|v| v.horizontal_factor),
                projected_x: projected_x.try_into().unwrap(),
            }));
            self.inner.present(surface, flags)
        }
    }

    thread_local! {
        static SAMPLE: std::cell::RefCell<Option<Image>> = const { std::cell::RefCell::new(None) };
    }

    let backend = ThreadedBackend::new(Recorder::default()).unwrap().with_jitter(17);
    let (mut device, mut mem) = create_copy_swap(backend, UploadPolicy::Always);
    device.set_mark_image_source(|| SAMPLE.take());
    let stage = |camera, point: [f32; 3], lens: [f32; 2]| {
        let views = [Some(lens[0]), Some(lens[1]), None, None].map(|factor| {
            Some(View {
                camera: RenderCamera {
                    position: [camera, 0.0, 0.0],
                    forward: [1.0, 0.0, 0.0],
                    up: [0.0, 0.0, 1.0],
                    fov: 1.0,
                    projection_xy: Some([1.0, 1.0, 0.0, 0.0]),
                    viewport: [0, 0, 480, 640],
                    near: 1.0,
                    far: 20.0,
                },
                horizontal_factor: factor?,
            })
        });
        let [x, y, z] = point;
        let mut samples = ImageSamples::default();
        samples.sources.push(std::sync::Arc::new(Line(point)));
        samples.sources.push(std::sync::Arc::new(Line([x, y + 1.0, z])));
        let _ = SAMPLE.replace(Some(Image {
            views,
            payload: std::sync::Arc::new(samples),
        }));
    };

    stage(1.0, [5.0, -2.0, 0.0], [0.5, 1.0]);
    device.swap(&mut mem, 1).unwrap();
    device.clear(&mut mem, 0, 0, 0x01, 0, 0.5, 0).unwrap(); // Depth only.
    device.swap(&mut mem, 1).unwrap();
    device.swap(&mut mem, 1).unwrap();

    device.clear(&mut mem, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    stage(3.0, [7.0, 1.0, 0.0], [1.0, 0.5]);
    device.swap(&mut mem, 2).unwrap(); // BYPASSCOPY re-shows the first image.
    device.swap(&mut mem, 1).unwrap(); // COPY shows the undisturbed second image.

    device.clear(&mut mem, 0, 0, 0xF0, 0, 1.0, 0).unwrap();
    stage(5.0, [9.0, 3.0, 0.0], [0.5, 1.0]);
    device.swap(&mut mem, 2).unwrap(); // Re-shows the second image.
    device.clear(&mut mem, 0, 0, 0xF0, 0, 1.0, 0).unwrap(); // Overwrites the third image.
    device.swap(&mut mem, 1).unwrap(); // The replacement has no world camera.

    let samples = device.backend.with_inner(|recorder| std::mem::take(&mut recorder.samples)).unwrap();
    let first = Sample {
        camera: 1.0,
        points: [[5.0, -2.0, 0.0], [5.0, -1.0, 0.0]],
        lens: [0.5, 1.0],
        projected_x: [[0.25, 0.5], [0.125, 0.25]],
    };
    let second = Sample {
        camera: 3.0,
        points: [[7.0, 1.0, 0.0], [7.0, 2.0, 0.0]],
        lens: [1.0, 0.5],
        projected_x: [[-0.25, -0.125], [-0.5, -0.25]],
    };
    assert_eq!(
        samples,
        [
            Some(first.clone()),
            Some(first.clone()),
            Some(first.clone()),
            Some(first),
            Some(second.clone()),
            Some(second),
            None,
        ]
    );
}
