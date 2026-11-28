//! Boundary drains must never consume a guest interrupt callback.

use crate::common::*;
use d3d::pushbuf::method;
use d3d::{D3dError, GuestMemory, RecordingBackend, UploadPolicy};

#[test]
fn ui_layout_reports_a_failed_drain_and_releases_the_previous_fit() {
    let (mut device, mut mem) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    let dev = mem.read_u32(d3d::GuestLayout::HALO2.p_device).unwrap();
    let put = mem.read_u32(dev).unwrap();
    let fit = d3d::backend::UiFit::about((1920, 1080), [320., 240.]);
    device.set_ui_fit(&mut mem, fit).unwrap();
    mem.write_u32(dev, put + 1).unwrap(); // an unaligned push-buffer cursor
    assert!(matches!(
        device.set_ui_fit(&mut mem, None),
        Err(D3dError::Invalid { entry: "push buffer", .. })
    ));
    // Retrying the same transition with a bad cursor can only succeed if
    // the failure already released the layout.
    device.set_ui_fit(&mut mem, None).unwrap();
    mem.write_u32(dev, put).unwrap();
    // Repairing PUT does not clear the device fault. Requesting a new
    // fit requires another drain, which must report that latched fault.
    assert!(matches!(device.set_ui_fit(&mut mem, fit), Err(D3dError::Faulted(_))));
}

#[test]
fn refused_boundary_keeps_callback_and_following_commands_in_order() {
    let (mut device, mut mem) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    let dev = mem.read_u32(d3d::GuestLayout::HALO2.p_device).unwrap();
    let mut put = mem.read_u32(dev).unwrap();
    let header = |method: u32, count: u32| (count << 18) | method;
    // Invented callback address/context, never invoked by this test.
    for word in [
        header(0x1d8c, 2),
        0x123400,
        0x456700,
        header(method::NOP, 1),
        7,
        header(0x1480, 1),
        0x5a5a,
    ] {
        mem.write_u32(put, word).unwrap();
        put += 4;
    }
    mem.write_u32(dev, put).unwrap();
    assert!(!device.drain_admission_boundary(&mut mem).unwrap());
    assert!(
        !device.drain_admission_boundary(&mut mem).unwrap(),
        "retry must still refuse"
    );
    assert_ne!(
        device.stipple_pattern()[0],
        0x5a5a,
        "no commands after callback may run"
    );
    let cb = device.drain_until_callback(&mut mem).unwrap().unwrap();
    assert_eq!((cb.function, cb.context, cb.kind), (0x123400, 0x456700, 7));
    assert!(device.drain_admission_boundary(&mut mem).unwrap());
    assert_eq!(device.stipple_pattern()[0], 0x5a5a);
    assert_eq!(device.stats().push_callbacks, 1);
}

#[test]
fn a_refused_native_array_leaves_the_one_ordinary_draw_for_the_caller() {
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Refuse(AtomicUsize);
    impl d3d::DrawAdmission for Refuse {
        fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            assert_eq!(snapshot.draw.vertex_count, 4);
            self.0.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut device, mut mem) = create(backend, UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 16.0, 16.0, 0xff_11_22_33);
    bind_quad(&mut device, &mut mem);
    let owner = Refuse(AtomicUsize::new(0));
    assert!(!device.try_native_array_draw(
        &mut mem, gfx::vertex::PrimitiveType::QuadList, 0, 4, &owner,
    ).unwrap());
    assert_eq!(owner.0.load(Ordering::Relaxed), 1);
    assert_eq!(device.last_native_refusal(), Some(d3d::NativeRefusal::Admission));
    assert!(device.backend.draws().is_empty());

    // The guest's own draw is resolved and counted; no native stage takes
    // it, so nothing draws it (crate::gap).
    let draws = device.stats().draws;
    device.draw_vertices(&mut mem, QUADLIST, 0, 4).unwrap();
    assert_eq!(device.stats().draws, draws + 1);
    assert!(device.backend.draws().is_empty());
}

#[test]
fn ordered_decal_quads_resolve_once_and_replay_each_refused_packet_once() {
    use std::sync::{Arc, Mutex};

    struct Decal(Arc<d3d::ResolvedDraw>);
    impl d3d::ExternalPass for Decal {
        fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
            Some(Arc::clone(&self.0))
        }

        fn writes(&self) -> d3d::ExternalWrites {
            d3d::ExternalWrites {
                color: true,
                depth_stencil: false,
            }
        }
    }

    #[derive(Default)]
    struct Admit(Mutex<Vec<Arc<d3d::ResolvedDraw>>>);
    impl d3d::DrawAdmission for Admit {
        fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            assert_eq!(snapshot.draw.entry, "D3DDevice_DrawVertices");
            let draw = Arc::new(snapshot.draw.clone());
            self.0.lock().unwrap().push(Arc::clone(&draw));
            let plan = d3d::ExternalPlan {
                stage: timeline::native::Stage::Decals,
                view: 0,
                ranges: 1,
                slots: Vec::new(),
            };
            Some((plan, Box::new(Decal(draw))))
        }
    }

    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut device, mut mem) = create(d3d::ThreadedBackend::new(backend).unwrap(), UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    let owner = Arc::new(Admit::default());
    device.add_draw_admission(owner.clone());
    quad_vb(&mut mem, 0.0, 0.0, 16.0, 16.0, 0xff_11_22_33);
    bind_quad(&mut device, &mut mem);
    let reads = device.backend.waits().reads;
    device.draw_vertices(&mut mem, QUADLIST, 0, 4).unwrap();
    device.draw_vertices(&mut mem, QUADLIST, 0, 4).unwrap();
    assert_eq!(device.backend.waits().reads, reads);

    // The recording backend refuses external recording. Only the original
    // packet for each guest call reaches the backend draw.
    let drawn = device.backend.with_inner(|b| {
        b.draws().into_iter().cloned().collect::<Vec<_>>()
    }).unwrap();
    let examined = owner.0.lock().unwrap();
    assert_eq!(device.stats().resolved_draws, 2);
    assert_eq!(device.stats().external_passes_dropped, 2);
    assert_eq!(examined.len(), 2);
    assert_eq!(drawn, examined.iter().map(|draw| (**draw).clone()).collect::<Vec<_>>());
}

#[test]
fn refused_native_pushed_ui_fan_preserves_state_and_the_guest_draw() {
    use gfx::state::CullMode;
    use gfx::vertex::PrimitiveType;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Refuse(Mutex<Vec<d3d::ResolvedDraw>>);
    impl d3d::DrawAdmission for Refuse {
        fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            self.0.lock().unwrap().push(snapshot.draw.clone());
            None
        }
    }
    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut device, mut mem) = create(backend, UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 16.0, 16.0, 0xff_11_22_33);
    bind_quad(&mut device, &mut mem);
    let fan = PrimitiveType::TriangleFan;
    // Pin the simple state just as a prior Begin would. A subsequent raw
    // guest store alone must not reach an inline draw.
    device.begin(&mut mem, fan.xbox_value()).unwrap();
    device.end(&mut mem).unwrap();
    let cull = d3d::GuestLayout::HALO2.render_state_slot(d3d::state::slot::CULLMODE);
    mem.write_u32(cull, CullMode::Cw.xbox_value()).unwrap();

    let vertices = [[1., 2.], [4., 2.], [4., 7.], [1., 7.]]
        .map(|[x, y]| [(3, [0.0, 1.0, 0.0, 1.0]), (0, [x, y, 0.0, 1.0])]);
    let owner = Refuse::default();
    assert!(!device.try_native_pushed_draw(&mut mem, fan, &vertices, &owner).unwrap());
    assert!(!device.try_native_immediate_draw(&mut mem, fan, &vertices, &owner).unwrap());
    assert!(device.backend.draws().is_empty());
    let captured = owner.0.lock().unwrap();
    assert_eq!(captured.len(), 2);
    assert_eq!(captured[0].state.render.cull_mode, CullMode::None);
    assert_eq!(captured[1].state.render.cull_mode, CullMode::Cw);
    assert_eq!(captured[0].streams, captured[1].streams);

    device.begin(&mut mem, fan.xbox_value()).unwrap();
    for vertex in &vertices {
        for &(register, value) in vertex {
            device.set_vertex_data(&mut mem, i32::from(register), value).unwrap();
        }
    }
    let draws = device.stats().draws;
    device.end(&mut mem).unwrap();
    assert_eq!(device.stats().draws, draws + 1, "the guest's draw is resolved once");
    assert!(device.backend.draws().is_empty(), "and drawn by nothing");
}

#[test]
fn a_native_fan_with_a_retained_packet_commits_without_waiting_for_the_render_thread() {
    use gfx::vertex::PrimitiveType;
    use std::sync::{Arc, Mutex};

    struct Retained(Arc<d3d::ResolvedDraw>);
    impl d3d::ExternalPass for Retained {
        fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
            Some(Arc::clone(&self.0))
        }
        fn writes(&self) -> d3d::ExternalWrites {
            d3d::ExternalWrites { color: true, depth_stencil: false }
        }
    }
    #[derive(Default)]
    struct Retain(Mutex<Vec<Arc<d3d::ResolvedDraw>>>);
    impl d3d::DrawAdmission for Retain {
        fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            let draw = Arc::new(snapshot.draw.clone());
            self.0.lock().unwrap().push(Arc::clone(&draw));
            let plan = d3d::ExternalPlan { stage: timeline::native::Stage::Ui, view: 4, ranges: 1, slots: Vec::new() };
            Some((plan, Box::new(Retained(draw))))
        }
    }

    let fan = PrimitiveType::TriangleFan;
    let vertices = [[1., 2.], [4., 2.], [4., 7.], [1., 7.]]
        .map(|[x, y]| [(3, [0.0, 1.0, 0.0, 1.0]), (0, [x, y, 0.0, 1.0])]);

    // The recording backend cannot supply GPU attachments, so the render
    // thread refuses the recording and draws the retained packet.
    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut device, mut mem) = create(d3d::ThreadedBackend::new(backend).unwrap(), UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 16.0, 16.0, 0xff_11_22_33);
    bind_quad(&mut device, &mut mem);
    let owner = Retain::default();
    let reads = device.backend.waits().reads;
    assert!(device.try_native_immediate_draw(&mut mem, fan, &vertices, &owner).unwrap());
    assert_eq!(device.backend.waits().reads, reads);
    let drawn = device.backend.with_inner(|b| {
        b.draws().into_iter().cloned().collect::<Vec<_>>()
    }).unwrap();
    assert_eq!(drawn.len(), 1);
    assert_eq!(*owner.0.lock().unwrap()[0], drawn[0]);
    assert_eq!(device.stats().external_passes_dropped, 1);
}

/// A colour-only native producer (a menu widget quad: the overlay pass binds
/// its colour as a scratch depth) over a draw with a depth bound but not
/// written makes the guest draw's writes, so it is queued with that draw as
/// its fallback, and the game thread does not wait on the render thread.
#[test]
fn a_colour_only_native_fan_over_a_bound_depth_commits_without_waiting_for_the_render_thread() {
    use gfx::vertex::PrimitiveType;
    use std::sync::{Arc, Mutex};

    struct Widget(Arc<d3d::ResolvedDraw>);
    impl d3d::ExternalPass for Widget {
        fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
            Some(Arc::clone(&self.0))
        }
        fn writes(&self) -> d3d::ExternalWrites {
            d3d::ExternalWrites { color: true, depth_stencil: false }
        }
        fn colour_only(&self) -> bool { true }
    }
    #[derive(Default)]
    struct Admit(Mutex<Vec<Arc<d3d::ResolvedDraw>>>);
    impl d3d::DrawAdmission for Admit {
        fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            assert_ne!(snapshot.depth_header, 0, "the draw has a depth bound");
            let draw = Arc::new(snapshot.draw.clone());
            self.0.lock().unwrap().push(Arc::clone(&draw));
            let plan = d3d::ExternalPlan { stage: timeline::native::Stage::Ui, view: 4, ranges: 1, slots: Vec::new() };
            Some((plan, Box::new(Widget(draw))))
        }
    }

    let fan = PrimitiveType::TriangleFan;
    let vertices = [[1., 2.], [4., 2.], [4., 7.], [1., 7.]]
        .map(|[x, y]| [(3, [0.0, 1.0, 0.0, 1.0]), (0, [x, y, 0.0, 1.0])]);
    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    let (mut device, mut mem) = create(d3d::ThreadedBackend::new(backend).unwrap(), UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 16.0, 16.0, 0xff_11_22_33);
    bind_quad(&mut device, &mut mem);
    let owner = Admit::default();
    let reads = device.backend.waits().reads;
    for _ in 0..3 {
        assert!(device.try_native_immediate_draw(&mut mem, fan, &vertices, &owner).unwrap());
    }
    assert_eq!(device.backend.waits().reads, reads, "no quad waited on the render thread");
    let drawn = device.backend.with_inner(|b| b.draws().into_iter().cloned().collect::<Vec<_>>()).unwrap();
    assert_eq!(drawn.len(), 3, "each refused recording draws its own guest packet");
    assert_eq!(*owner.0.lock().unwrap()[0], drawn[0]);
}

#[test]
fn a_device_that_cannot_take_an_external_pass_refuses_a_native_draw_at_the_boundary() {
    struct Never;
    impl d3d::DrawAdmission for Never {
        fn admit(&self, _: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            panic!("a draw refused at the boundary is never offered");
        }
    }
    let (mut device, mut mem) = create(RecordingBackend::new(), UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 16.0, 16.0, 0xff_11_22_33);
    bind_quad(&mut device, &mut mem);
    assert!(!device.try_native_array_draw(&mut mem, gfx::vertex::PrimitiveType::QuadList, 0, 4, &Never).unwrap());
    assert_eq!(device.last_native_refusal(), Some(d3d::NativeRefusal::Boundary));
}

/// A particle producer for [`particles`]: each draw it is offered becomes a
/// pass standing in for it, as `game`'s particle admission does.
#[derive(Default)]
struct Particles(std::sync::Mutex<Vec<std::sync::Arc<d3d::ResolvedDraw>>>);

struct Particle(std::sync::Arc<d3d::ResolvedDraw>);

impl d3d::ExternalPass for Particle {
    fn resolved_fallback(&self) -> Option<std::sync::Arc<d3d::ResolvedDraw>> {
        Some(std::sync::Arc::clone(&self.0))
    }
    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites { color: true, depth_stencil: false }
    }
}

impl d3d::DrawAdmission for Particles {
    fn admit(&self, snapshot: &d3d::DrawSnapshot<'_>) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let draw = std::sync::Arc::new(snapshot.draw.clone());
        self.0.lock().unwrap().push(std::sync::Arc::clone(&draw));
        let plan = d3d::ExternalPlan { stage: timeline::native::Stage::Particles, view: 4, ranges: 1, slots: Vec::new() };
        Some((plan, Box::new(Particle(draw))))
    }
}

/// A device set up as the particle walk finds it: a vertex program, and
/// one inline array register (v0, four shorts) pushed.
fn particle_device() -> (d3d::Device<RecordingBackend>, d3d::SparseMemory) {
    use shaders::vsh::encode::{Inst, Operand, program};
    use shaders::vsh::{MacOp, OutReg};
    const FUNCTION: u32 = MISC + 0x3400;
    let mut backend = RecordingBackend::new();
    backend.external_passes = true;
    backend.queue_external_draws = true;
    let (mut device, mut mem) = create(backend, UploadPolicy::Tracked);
    set_states(&mut device, &mut mem);
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None).to_out(false, OutReg::D0, 0xF).last(),
    ]);
    mem.write(FUNCTION, &code).unwrap();
    device.load_vertex_shader_program(&mut mem, FUNCTION, 0).unwrap();
    device.set_vertex_shader(&mut mem, (MISC + 0x3a00) | 1).unwrap();
    let dev = mem.read_u32(d3d::GuestLayout::HALO2.p_device).unwrap();
    let mut put = mem.read_u32(dev).unwrap();
    for word in [(1 << 18) | 0x1760, 0x0845] {
        mem.write_u32(put, word).unwrap();
        put += 4;
    }
    mem.write_u32(dev, put).unwrap();
    (device, mem)
}

/// Particle `i`'s corners and its carried colour.
fn particle(i: u32) -> (Vec<u8>, [Option<[f32; 4]>; 16]) {
    let n = i as i16;
    let data = [[n, 0, 0, 1], [n + 4, 0, 0, 1], [n + 4, 4, 0, 1], [n, 4, 0, 1]]
        .into_iter()
        .flatten()
        .flat_map(i16::to_le_bytes)
        .collect();
    let mut carried = [None; 16];
    carried[3] = Some([i as f32 / 8.0, 0.5, 0.25, 1.0]);
    (data, carried)
}

/// `n` particles through `device`, `between` run before each after the
/// first: the draws the producer was offered, what the backend queued,
/// and the device's stats.
fn particles(
    runs: bool, n: u32, mut between: impl FnMut(&mut d3d::Device<RecordingBackend>, &mut d3d::SparseMemory, u32),
) -> (Vec<d3d::ResolvedDraw>, Vec<(timeline::native::Stage, usize)>, d3d::device::Stats) {
    let (mut device, mut mem) = particle_device();
    device.set_inline_runs(runs);
    let owner = Particles::default();
    for i in 0..n {
        if i > 0 {
            between(&mut device, &mut mem, i);
        }
        let (data, carried) = particle(i);
        assert!(device
            .try_native_inline_draw(&mut mem, gfx::vertex::PrimitiveType::QuadList, data, &carried, &owner)
            .unwrap(), "particle {i} was not drawn");
    }
    let offered = owner.0.lock().unwrap().iter().map(|d| (**d).clone()).collect();
    (offered, device.backend.queued.clone(), device.stats())
}

/// N particles of one state make one full submit and N-1 fast ones, and
/// the producer is offered exactly the draws, in the same order, that N
/// full submits offer it: so the packets are the same.
#[test]
fn same_state_particles_resolve_once_and_offer_identical_draws() {
    let (full, full_queued, full_stats) = particles(false, 6, |_, _, _| {});
    let (fast, fast_queued, fast_stats) = particles(true, 6, |_, _, _| {});
    assert_eq!((full_stats.native_inline_full, full_stats.native_inline_fast), (6, 0));
    assert_eq!((fast_stats.native_inline_full, fast_stats.native_inline_fast), (1, 5));
    assert_eq!(fast.len(), 6);
    assert_eq!(fast, full);
    assert_ne!(fast[0].streams, fast[1].streams);
    assert_ne!(fast[0].shader.vertex_defaults, fast[1].shader.vertex_defaults);
    assert_eq!(fast_queued, full_queued);
    assert_eq!(fast_stats.external_passes_dropped, full_stats.external_passes_dropped);
}

/// A state change between two particles ends the run: the next particle
/// resolves in full, and offers what a full submit would.
#[test]
fn a_state_change_between_particles_resolves_the_next_in_full() {
    let blend = |d: &mut d3d::Device<RecordingBackend>, m: &mut d3d::SparseMemory, i: u32| {
        if i == 3 {
            d.set_render_state(m, RS_ALPHABLENDENABLE, 1).unwrap();
        }
    };
    let (full, _, _) = particles(false, 5, blend);
    let (fast, _, stats) = particles(true, 5, blend);
    assert_eq!((stats.native_inline_full, stats.native_inline_fast), (2, 3));
    assert_eq!(fast, full);
    assert_ne!(fast[2].state, fast[3].state);
}

/// A material other than the particle device's: alpha blend on, its own
/// combiner, one constant and a program that colours from v0.
fn supplied_material() -> d3d::SuppliedState {
    use shaders::vsh::encode::{Inst, Operand, program};
    use shaders::vsh::{MacOp, OutReg};
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xF),
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::D0, 0xF).last(),
    ]);
    let mut pixel_shader = [0u32; 60];
    pixel_shader[0] = 0x0123_4567;
    pixel_shader[57] = 0x89ab_cdef;
    d3d::SuppliedState {
        render: vec![(RS_ALPHABLENDENABLE, 1)],
        pixel_shader,
        constants: vec![(96 + 18, [1.0, 2.0, 3.0, 4.0], 0b0101)],
        microcode: code[4..].chunks_exact(4).map(|w| u32::from_le_bytes(w.try_into().unwrap())).collect(),
        ..Default::default()
    }
}

/// Particles 0 and 1 through `device`, a supplied draw of particle 9
/// between them when `cut`: the draws offered and the guest's render word.
fn around_a_cut(cut: bool) -> (Vec<d3d::ResolvedDraw>, u32) {
    let (mut device, mut mem) = particle_device();
    device.set_inline_runs(true);
    let owner = Particles::default();
    let (data, carried) = particle(0);
    assert!(device.try_native_inline_draw(&mut mem, gfx::vertex::PrimitiveType::QuadList, data, &carried, &owner).unwrap());
    if cut {
        let (data, carried) = particle(9);
        let drawn = device
            .try_native_supplied_draw(&mut mem, gfx::vertex::PrimitiveType::QuadList, data, &carried, &supplied_material(), &owner)
            .unwrap();
        assert_eq!(drawn, Ok(()));
    }
    let (data, carried) = particle(1);
    assert!(device.try_native_inline_draw(&mut mem, gfx::vertex::PrimitiveType::QuadList, data, &carried, &owner).unwrap());
    let offered = owner.0.lock().unwrap().iter().map(|d| (**d).clone()).collect();
    let word = mem.read_u32(d3d::GuestLayout::HALO2.render_state_slot(RS_ALPHABLENDENABLE)).unwrap();
    (offered, word)
}

/// A cut draw takes its render words, combiner, constants and program
/// from the supplied material, not from the draw before it.
#[test]
fn a_cut_draw_uses_the_supplied_state_not_the_previous_draws() {
    let (offered, _) = around_a_cut(true);
    let (before, cut) = (&offered[0], &offered[1]);
    let s = supplied_material();
    assert_eq!(before.raw.render[RS_ALPHABLENDENABLE as usize], 0);
    assert_eq!(cut.raw.render[RS_ALPHABLENDENABLE as usize], 1);
    assert_eq!(cut.shader.pixel_shader.as_deref(), Some(&s.pixel_shader));
    // SetPixelShader's copy: definition words 0..56 are render slots 0..56.
    assert_eq!(cut.raw.render[..57], s.pixel_shader[..57]);
    assert_eq!(cut.shader.combiner_state[..], s.pixel_shader[..57]);
    let c = cut.shader.constants[96 + 18];
    let b = before.shader.constants[96 + 18];
    assert_eq!([c[0], c[1], c[2], c[3]], [1.0, b[1], 3.0, b[3]], "the mask writes x and z");
    let d3d::shader::VertexProcessing::Program { start, microcode } = &cut.shader.vertex else { panic!("a program") };
    assert_eq!((*start, &microcode[..]), (0, &s.microcode[..]));
    assert_ne!(cut.shader.vertex, before.shader.vertex);
}

/// After a cut draw the device's state is the guest's: the next draw
/// resolves exactly as it does with no cut draw before it, and the
/// guest's render array is untouched.
#[test]
fn the_device_state_after_a_cut_draw_is_the_guests() {
    let (with, word) = around_a_cut(true);
    let (without, plain) = around_a_cut(false);
    assert_eq!((word, plain), (0, 0));
    assert_eq!(with.len(), 3);
    assert_eq!(with[2], without[1]);
}

/// A cut draw the device cannot draw from its material alone is refused
/// by name, offers nothing and leaves the device as it was.
#[test]
fn a_cut_draw_without_a_program_is_refused_by_name() {
    let (mut device, mut mem) = particle_device();
    let owner = Particles::default();
    let (data, carried) = particle(0);
    let s = d3d::SuppliedState { microcode: Vec::new(), ..supplied_material() };
    let drawn = device
        .try_native_supplied_draw(&mut mem, gfx::vertex::PrimitiveType::QuadList, data, &carried, &s, &owner)
        .unwrap();
    assert_eq!(drawn, Err("program"));
    assert!(owner.0.lock().unwrap().is_empty());
}
