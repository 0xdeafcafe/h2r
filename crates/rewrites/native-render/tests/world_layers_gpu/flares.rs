//! Invented-data offscreen test of `FlareGpu::prepare`: fans of made-up
//! geometry, a made-up vertex program and a made-up combiner are prepared by
//! the flare path, issued through the backend's external-pass recording and
//! read back. No guest data.

use std::sync::{Arc, Mutex};

use d3d::backend::{
    ExternalAttachments, ExternalDraws, ExternalPassRequest, ExternalSurfaces, StreamBytes,
};
use d3d::shader::VertexProcessing;
use d3d::{
    Backend, ExternalPass, ExternalPlan, ExternalWrites, GfxBackend, ResolvedDraw, TargetCopy,
};
use gfx::encode::HostPixelOrder;
use gfx::state::{BlendFactor, BlendOp, ColorWriteMask, CompareFunc};
use gfx::surface::SurfaceId;
use gfx::vertex::{
    PrimitiveType, StreamLayout, VertexAttribute, VertexDataType, VertexLayout, VertexRegister,
};
use gfx::{Clear, GfxContext};
use native_render::flares::{FlareGpu, Refusal, Site};
use shaders::vsh::encode::{Inst, Operand, program};
use shaders::vsh::{MacOp, OutReg, Program};

use super::{CAPTURE_LOCK, DEPTH, FORMAT, SIZE};

const STRIDE: u32 = 256;

struct Fan {
    registers: &'static [u8],
    /// `(x0, y0, x1, y1)` in target pixels.
    rect: (f32, f32, f32, f32),
    /// Written to register 9 of every vertex when 9 is a written register.
    color: [f32; 4],
    /// The carried default of register 9 when it is not written.
    carried: [f32; 4],
    blend: Option<(BlendFactor, BlendFactor)>,
}

/// A vertex program that passes register 0 to the position and register 9 to
/// the diffuse colour, and a combiner that outputs that colour and alpha.
fn fan(f: &Fan, sampled: bool) -> ResolvedDraw {
    let code = program(&[
        Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None).to_out(false, OutReg::Pos, 0xf),
        Inst::mac(MacOp::Mov, Some(Operand::v(9)), None, None).to_out(false, OutReg::D0, 0xf),
        Inst::mac(MacOp::Mov, Some(Operand::v(3)), None, None)
            .to_out(false, OutReg::T(0), 0xf)
            .last(),
    ]);
    let microcode: Vec<u32> = Program::parse(&code)
        .unwrap()
        .instructions
        .iter()
        .flat_map(|i| i.words)
        .collect();
    // Use the fixture's target, viewport and pass-through pixel combiner.
    let mut draw = super::triangle(0.0, f.color, 0, sampled);
    let render = &mut draw.state.render;
    render.stencil.enable = false;
    render.depth_write = false;
    render.depth_func = CompareFunc::Always;
    if let Some((src, dst)) = f.blend {
        render.blend_enable = true;
        render.src_blend = src;
        render.dst_blend = dst;
        render.blend_op = BlendOp::Add;
    }

    let (x0, y0, x1, y1) = f.rect;
    let corners = [
        (x0, y0, 0.0, 0.0),
        (x1, y0, 1.0, 0.0),
        (x1, y1, 1.0, 1.0),
        (x0, y1, 0.0, 1.0),
    ];
    let mut bytes = vec![0u8; 4 * STRIDE as usize];
    let mut written = 0u16;
    for (v, &(x, y, u, w)) in corners.iter().enumerate() {
        for &r in f.registers {
            let value = match r {
                0 => [x, y, 0.0, 4096.0],
                3 => [u, w, 0.0, 1.0],
                9 => f.color,
                _ => unreachable!(),
            };
            let o = v * STRIDE as usize + 16 * r as usize;
            bytes[o..o + 16].copy_from_slice(bytemuck::cast_slice(&value));
            written |= 1 << r;
        }
    }
    draw.entry = "flare fan";
    draw.primitive = PrimitiveType::TriangleFan;
    draw.vertex_count = 4;
    draw.indices = None;
    draw.shader.vertex = VertexProcessing::Program {
        start: 0,
        microcode: microcode.into(),
    };
    draw.shader.provided_inputs = written;
    draw.shader.vertex_defaults[9] = f.carried;
    draw.layout = VertexLayout {
        streams: vec![StreamLayout {
            stream: 0,
            stride: STRIDE,
            attributes: f
                .registers
                .iter()
                .map(|&r| VertexAttribute {
                    register: VertexRegister(r),
                    data_type: VertexDataType::Float4,
                    offset: 16 * u32::from(r),
                })
                .collect(),
        }],
        pretransformed: false,
    };
    draw.streams = vec![StreamBytes {
        stride: STRIDE,
        data: bytes.into(),
    }];
    draw
}

/// The pass the bridge builds: the draw, the shared `FlareGpu`, and a cell
/// for what `prepare` answered.
struct FlarePass {
    site: Site,
    draw: Arc<ResolvedDraw>,
    gpu: Arc<Mutex<FlareGpu>>,
    frame: u32,
    answer: Arc<Mutex<Option<Result<(), Refusal>>>>,
}

impl ExternalPass for FlarePass {
    fn counts_samples(&self) -> bool {
        self.site == Site::Visibility
    }
    fn replays_scene(&self) -> bool {
        true
    }
    fn record_draws(
        &mut self,
        target: &ExternalAttachments,
        _request: ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let result = self
            .gpu
            .lock()
            .unwrap()
            .prepare(self.site, target, &self.draw, self.frame);
        *self.answer.lock().unwrap() = Some(result.as_ref().map(|_| ()).map_err(|&e| e));
        Some(result.map_err(|_| "flare fan preparation"))
    }

    fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
        Some(Arc::clone(&self.draw))
    }

    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: self.draw.state.render.color_write.xbox_value() != 0,
            depth_stencil: false,
        }
    }
}

struct Scene {
    backend: GfxBackend,
    color: SurfaceId,
    depth: SurfaceId,
    gpu: Arc<Mutex<FlareGpu>>,
    frame: u32,
}

impl Scene {
    fn new() -> Option<Self> {
        let ctx = GfxContext::headless().ok()?;
        let mut backend = GfxBackend::new(ctx);
        let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
        let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
        backend
            .clear(color, Some(depth), Clear::all([0.0, 0.0, 0.0, 1.0]))
            .unwrap();
        Some(Self {
            backend,
            color,
            depth,
            gpu: Default::default(),
            frame: 0,
        })
    }

    /// Prepare and commit one fan the way the bridge's pass does. Returns what
    /// `FlareGpu::prepare` answered (`None` when the backend never asked) and
    /// whether the backend committed a recording.
    fn issue(&mut self, site: Site, f: &Fan) -> (Option<Result<(), Refusal>>, bool) {
        self.issue_draw(site, fan(f, false))
    }

    fn issue_draw(&mut self, site: Site, draw: ResolvedDraw) -> (Option<Result<(), Refusal>>, bool) {
        self.frame += 1;
        let answer = Arc::new(Mutex::new(None));
        let pass: Box<dyn ExternalPass> = Box::new(FlarePass {
            site,
            draw: Arc::new(draw),
            gpu: Arc::clone(&self.gpu),
            frame: self.frame,
            answer: Arc::clone(&answer),
        });
        let surfaces = ExternalSurfaces {
            color: self.color,
            depth: self.depth,
            sampled: vec![],
            slots: vec![],
            slot_sizes: vec![],
        };
        let plan = ExternalPlan {
            stage: timeline::native::Stage::Flares,
            view: 4,
            ranges: 1,
            slots: Vec::new(),
        };
        let request = ExternalPassRequest {
            rect: (0, 0, SIZE, SIZE),
            surface_size: (SIZE, SIZE),
            display_projection_factor: 1.0,
            z_scale: 16_777_215.0,
        };
        let recordings = match self.backend.prepare_external(
            surfaces,
            request,
            [None; 4],
            &plan,
            TargetCopy::Reference,
            Arc::new(Mutex::new(pass)),
        ) {
            Ok(Some(recordings)) => recordings,
            _ => return (answer.lock().unwrap().take(), false),
        };
        assert_eq!(recordings.len(), 1);
        for recording in recordings {
            self.backend.commit_external(recording);
        }
        (answer.lock().unwrap().take(), true)
    }

    /// RGBA bytes of every pixel, row by row.
    fn read(&mut self) -> Vec<[u8; 4]> {
        let (bytes, order) = self.backend.read_color(self.color).unwrap();
        bytes
            .chunks_exact(4)
            .map(|p| match order {
                HostPixelOrder::Bgra8 => [p[2], p[1], p[0], p[3]],
                HostPixelOrder::Rgba8 => [p[0], p[1], p[2], p[3]],
            })
            .collect()
    }
}

fn at(pixels: &[[u8; 4]], x: usize, y: usize) -> [u8; 3] {
    let p = pixels[y * SIZE as usize + x];
    [p[0], p[1], p[2]]
}

fn near(actual: [u8; 3], expected: [u8; 3]) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(&a, e)| a.abs_diff(e) <= 3)
}

#[test]
fn a_visibility_fan_tests_w_depth_and_contributes_to_the_current_report() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut scene) = Scene::new() else { return };
    let mut query = fan(&Fan {
        registers: &[0],
        rect: (2.0, 4.0, 10.0, 10.0),
        color: [1.0; 4],
        carried: [1.0; 4],
        blend: None,
    }, false);
    query.hw.zpass_count_enable = true;
    query.state.render.color_write = ColorWriteMask::from_xbox(0);
    query.state.render.depth_func = CompareFunc::Less;

    scene.backend.clear_pixel_count().unwrap();
    assert_eq!(scene.issue_draw(Site::Visibility, query.clone()), (Some(Ok(())), true));
    // Compare the next fan against a depth between its z (0) and w (4096).
    // It must contribute no samples. Ending the report includes both draws.
    scene.backend.clear(scene.color, Some(scene.depth), Clear {
        depth: Some(0.0001),
        ..Clear::NONE
    }).unwrap();
    assert_eq!(scene.issue_draw(Site::Visibility, query), (Some(Ok(())), true));
    let ticket = scene.backend.report_pixel_count().unwrap().unwrap();
    assert_eq!(scene.backend.read_pixel_count(ticket), Ok(8 * 6));
    assert!(scene.read().iter().all(|pixel| *pixel == [0, 0, 0, 255]));
}

#[test]
fn blended_fans_in_two_passes_cover_exactly_their_rectangles() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut scene) = Scene::new() else {
        eprintln!("skipping flare fan test without a GPU adapter");
        return;
    };
    // A half-covering red reflection over black, then an additive green
    // composite that overlaps its lower right quarter.
    let red = Fan {
        registers: &[0, 3, 9],
        rect: (4.0, 4.0, 20.0, 20.0),
        color: [1.0, 0.0, 0.0, 0.5],
        carried: [0.0; 4],
        blend: Some((BlendFactor::SrcAlpha, BlendFactor::InvSrcAlpha)),
    };
    let green = Fan {
        registers: &[0, 3, 9],
        rect: (12.0, 12.0, 28.0, 28.0),
        color: [0.0, 1.0, 0.0, 1.0],
        carried: [0.0; 4],
        blend: Some((BlendFactor::One, BlendFactor::One)),
    };
    assert_eq!(scene.issue(Site::Reflection, &red), (Some(Ok(())), true));
    assert_eq!(scene.issue(Site::Composite, &green), (Some(Ok(())), true));
    let pixels = scene.read();
    assert!(
        near(at(&pixels, 6, 6), [128, 0, 0]),
        "red only: {:?}",
        at(&pixels, 6, 6)
    );
    assert!(
        near(at(&pixels, 16, 16), [128, 255, 0]),
        "overlap: {:?}",
        at(&pixels, 16, 16)
    );
    assert!(
        near(at(&pixels, 24, 24), [0, 255, 0]),
        "green only: {:?}",
        at(&pixels, 24, 24)
    );
    assert!(
        near(at(&pixels, 30, 2), [0, 0, 0]),
        "outside: {:?}",
        at(&pixels, 30, 2)
    );
    assert!(
        near(at(&pixels, 2, 30), [0, 0, 0]),
        "outside: {:?}",
        at(&pixels, 2, 30)
    );
    // The edges: just inside and just outside the red rectangle.
    assert!(near(at(&pixels, 4, 10), [128, 0, 0]), "left edge in");
    assert!(near(at(&pixels, 3, 10), [0, 0, 0]), "left edge out");
    assert!(near(at(&pixels, 10, 19), [128, 0, 0]), "bottom edge in");
    assert!(near(at(&pixels, 10, 20), [0, 0, 0]), "bottom edge out");
}

#[test]
fn a_position_only_fan_takes_its_colour_from_the_carried_default() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut scene) = Scene::new() else {
        eprintln!("skipping flare fan test without a GPU adapter");
        return;
    };
    let mask = Fan {
        registers: &[0],
        rect: (2.0, 20.0, 10.0, 28.0),
        color: [1.0, 0.0, 0.0, 1.0],
        carried: [0.0, 0.0, 1.0, 1.0],
        blend: None,
    };
    assert_eq!(scene.issue(Site::Mask, &mask), (Some(Ok(())), true));
    let pixels = scene.read();
    assert!(
        near(at(&pixels, 4, 24), [0, 0, 255]),
        "carried colour: {:?}",
        at(&pixels, 4, 24)
    );
    assert!(near(at(&pixels, 12, 24), [0, 0, 0]), "outside");
}

#[test]
fn a_fan_with_the_wrong_written_mask_is_refused_and_draws_nothing() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut scene) = Scene::new() else {
        eprintln!("skipping flare fan test without a GPU adapter");
        return;
    };
    let fan = Fan {
        registers: &[0, 3, 9],
        rect: (4.0, 4.0, 20.0, 20.0),
        color: [1.0, 1.0, 1.0, 1.0],
        carried: [0.0; 4],
        blend: None,
    };
    // The same draw is a fan for Reflection and a wrong mask for the others.
    for site in [
        Site::Mask,
        Site::MaskSample,
        Site::FilterSource,
        Site::Filter,
    ] {
        assert_eq!(
            scene.issue(site, &fan),
            (Some(Err(Refusal::Layout)), false),
            "{site:?}"
        );
    }
    assert!(
        scene.read().iter().all(|p| p[..3] == [0, 0, 0]),
        "a refused fan drew"
    );
    assert_eq!(scene.issue(Site::Reflection, &fan), (Some(Ok(())), true));
    assert!(near(at(&scene.read(), 10, 10), [255, 255, 255]));
}

#[test]
fn a_retained_filter_samples_its_input_and_counts_only_a_native_commit() {
    use super::frame_graph::{bind, clear, op, replay};
    use d3d::gfx_backend::GuestDrawPass;
    use d3d::scene_sink::{ReplayOp, ReplaySample, ReplayTexture, RetainedExternal};
    use frame_scene::{Slot, TargetRef, carried::CameraUse};

    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(mut scene) = Scene::new() else {
        return;
    };
    let input = TargetRef::Aux(0);
    let output = TargetRef::SceneColour;
    let depth = TargetRef::SceneDepth;
    let filter = Arc::new(fan(
        &Fan {
            registers: &[0, 3],
            rect: (4., 4., 12., 12.),
            color: [0.; 4],
            carried: [0.; 4],
            blend: None,
        },
        true,
    ));
    let committed = || {
        d3d::scene_stage::counters()
            .into_iter()
            .find(|(name, _)| name == "scene_submitted_flares_reference_draws")
            .unwrap()
            .1
    };

    // Both replays must sample the same magenta surface. The second pass
    // refuses the written registers and hands the exact packet back to D3D.
    for (site, expected) in [(Site::Filter, Ok(())), (Site::Mask, Err(Refusal::Layout))] {
        let mut frame = replay();
        frame.aux.push(frame.colour.unwrap());
        let answer = Arc::new(Mutex::new(None));
        let replacement = FlarePass {
            site,
            draw: filter.clone(),
            gpu: scene.gpu.clone(),
            frame: 1,
            answer: answer.clone(),
        };
        let external = RetainedExternal {
            packet: filter.clone(),
            surfaces: ExternalSurfaces {
                color: output,
                depth,
                sampled: vec![],
                slots: vec![],
                slot_sizes: vec![],
            },
            plan: ExternalPlan {
                stage: timeline::native::Stage::Flares,
                view: 0,
                ranges: 1,
                slots: vec![],
            },
            request: ExternalPassRequest {
                rect: (0, 0, SIZE, SIZE),
                surface_size: (SIZE, SIZE),
                display_projection_factor: 1.,
                z_scale: 16_777_215.,
            },
            pass: Arc::new(Mutex::new(Box::new(replacement))),
        };
        for command in [
            clear(),
            ReplayOp::Clear {
                colour: input,
                depth: TargetRef::None,
                clear: Clear {
                    color: Some([1., 0., 1., 1.]),
                    ..Clear::NONE
                },
            },
            ReplayOp::Sample {
                of: input,
                id: 1,
                how: ReplaySample::InPlace,
            },
            bind(),
            ReplayOp::Draw {
                draw: filter.clone(),
                textures: [Some(ReplayTexture::Sample(1)), None, None, None],
                camera: CameraUse::Screen,
                external: Some(Box::new(external)),
            },
        ] {
            op(&mut frame, Slot::AfterPost, command);
        }
        let before = committed();
        let mut playback = GuestDrawPass::new(&mut scene.backend, &frame, 1u32).unwrap();
        for (_, command) in &frame.ops {
            playback.apply(command).unwrap();
        }
        let (size, rgba) = playback.read_rgba().unwrap();
        assert_eq!(size, [SIZE; 2]);
        assert_eq!(rgba[(8 * SIZE as usize + 8) * 4..][..4], [255, 0, 255, 255]);
        assert_eq!(rgba[(14 * SIZE as usize + 14) * 4..][..4], [0, 0, 0, 255]);
        assert_eq!(*answer.lock().unwrap(), Some(expected));
        assert_eq!(committed(), before + u64::from(expected.is_ok()));
    }
}
