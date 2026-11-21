//! Private and displayed draws of a sealed scene, using this rig's
//! invented triangles, depth/stencil state and texture-sampling program.

use super::{CAPTURE_LOCK, DEPTH, FORMAT, SIZE, triangle};
use d3d::backend::{ExternalPassRequest, ExternalPlan, ExternalSurfaces, SceneStage};
use d3d::scene_sink::{ReplayFrame, ReplayOp, ReplaySample, ReplayTexture};
use d3d::{GfxBackend, Lighting};
use frame_scene::carried::CameraUse;
use frame_scene::{
    AuxTarget, FrameScene, FreeList, SceneBuilder, SceneId, Slot, SlotAt, Stamp, TargetRef,
};
use gfx::{Clear, GfxContext};
use native_render::frame_graph::{
    CapturedFrame, PassObserver, PassRow, ReadColour, draw_display, draw_private,
    draw_private_with_observer,
};
use native_render::world_layers_gpu::{Cache, IndexedPass};
use std::sync::{Arc, Mutex};

const MAIN: TargetRef = TargetRef::SceneColour;
const Z: TargetRef = TargetRef::SceneDepth;

fn sealed() -> Arc<FrameScene> {
    let mut builder = SceneBuilder::new(FreeList::new());
    builder.begin(SceneId {
        map_epoch: 2,
        tick: 17,
        frame: 12,
    });
    builder.seal(Stamp::default()).unwrap()
}

fn replay() -> ReplayFrame {
    let colour = AuxTarget {
        width: SIZE,
        height: SIZE,
        format: u32::from(FORMAT as u8),
        depth: false,
    };
    ReplayFrame {
        colour: Some(colour),
        depth: Some(AuxTarget {
            format: u32::from(DEPTH as u8),
            depth: true,
            ..colour
        }),
        presented: MAIN,
        ..Default::default()
    }
}

fn op(frame: &mut ReplayFrame, slot: Slot, op: ReplayOp) {
    frame.ops.push((SlotAt { slot, view: 0 }, op));
}

fn bind() -> ReplayOp {
    ReplayOp::Bind {
        colour: MAIN,
        depth: Z,
    }
}

fn clear() -> ReplayOp {
    ReplayOp::Clear {
        colour: MAIN,
        depth: Z,
        clear: Clear::all([0.0, 0.0, 0.0, 1.0]),
    }
}

fn guest_triangle(x: f32, colour: [f32; 4], texture: Option<u32>) -> ReplayOp {
    let mut draw = triangle(x, colour, 1, texture.is_some());
    if texture.is_none() {
        draw.state.render.depth_func = gfx::state::CompareFunc::Always;
    }
    ReplayOp::Draw {
        draw: draw.into(),
        textures: [texture.map(ReplayTexture::Sample), None, None, None],
        camera: CameraUse::Unknown,
    }
}

fn typed_triangle(x: f32, colour: [f32; 4]) -> ReplayOp {
    let mut draw = triangle(x, colour, 1, false);
    draw.state.render.depth_func = gfx::state::CompareFunc::Always;
    let pass = IndexedPass::new(&draw, Arc::new(Cache::default()), 1, None).unwrap();
    ReplayOp::Stage(SceneStage {
        surfaces: ExternalSurfaces {
            color: MAIN,
            depth: Z,
            sampled: vec![],
            slots: vec![],
            slot_sizes: vec![],
        },
        request: ExternalPassRequest {
            rect: (0, 0, SIZE, SIZE),
            surface_size: (SIZE, SIZE),
            z_scale: 16_777_215.0,
        },
        plan: ExternalPlan {
            stage: timeline::native::Stage::World,
            view: 0,
            ranges: 1,
            slots: vec![],
        },
        pass: Arc::new(Mutex::new(Box::new(pass))),
    })
}

fn rgba(frame: &CapturedFrame, x: u32, y: u32) -> [u8; 4] {
    let i = (4 * (y * frame.size[0] + x)) as usize;
    frame.rgba[i..i + 4].try_into().unwrap()
}

#[derive(Default)]
struct Observe {
    rows: Vec<PassRow>,
    snapshots: Vec<Vec<u8>>,
    snap_first: bool,
}

impl PassObserver for Observe {
    fn pass_end(&mut self, i: usize, row: &PassRow, read: &mut dyn FnMut() -> ReadColour) {
        assert_eq!(i, self.rows.len());
        self.rows.push(row.clone());
        if self.snap_first && i == 0 {
            let (size, rgba) = read().unwrap();
            assert_eq!(size, [SIZE; 2]);
            self.snapshots.push(rgba);
        }
    }
}

#[test]
fn typed_and_carried_draws_in_adjacent_slots_share_one_pass() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, clear());
    op(
        &mut frame,
        Slot::Opaque,
        typed_triangle(2.0, [1.0, 0.0, 0.0, 1.0]),
    );
    op(
        &mut frame,
        Slot::Opaque,
        typed_triangle(18.0, [0.0, 0.0, 1.0, 1.0]),
    );
    op(&mut frame, Slot::AfterOpaque, bind());
    op(
        &mut frame,
        Slot::AfterOpaque,
        guest_triangle(2.0, [0.0, 1.0, 0.0, 1.0], None),
    );
    let mut observer = Observe::default();
    let capture = draw_private_with_observer(
        &mut backend,
        &sealed(),
        &frame,
        1,
        Lighting::Retail,
        Some(&mut observer),
    )
    .unwrap();
    assert_eq!(
        observer.rows,
        [PassRow {
            colour: MAIN,
            depth: Z,
            ops: 0..5
        }]
    );
    assert_eq!(capture.report.render_passes, 1);
    assert_eq!(
        rgba(&capture, 4, 4),
        [0, 255, 0, 255],
        "the later guest draw wins"
    );
    assert_eq!(
        rgba(&capture, 20, 4),
        [0, 0, 255, 255],
        "the other typed triangle survives"
    );
}

#[test]
fn carried_scene_colour_copy_samples_what_was_drawn_before_it() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let mut frame = replay();
    frame.aux.push(frame.colour.unwrap());
    let aux = TargetRef::Aux(0);
    op(&mut frame, Slot::Opaque, clear());
    op(&mut frame, Slot::Opaque, bind());
    op(
        &mut frame,
        Slot::Opaque,
        guest_triangle(2.0, [1.0, 0.0, 0.0, 1.0], None),
    );
    op(
        &mut frame,
        Slot::AfterLights,
        ReplayOp::Copy {
            from: MAIN,
            to: aux,
        },
    );
    op(
        &mut frame,
        Slot::Transparents,
        ReplayOp::Sample {
            of: aux,
            id: 0,
            how: ReplaySample::InPlace,
        },
    );
    // The later shader reads the captured red on the copied black field.
    // The blue main target would give a different result if sampled live.
    op(
        &mut frame,
        Slot::Transparents,
        ReplayOp::Clear {
            colour: MAIN,
            depth: Z,
            clear: Clear {
                color: Some([0.0, 0.0, 1.0, 1.0]),
                ..Clear::NONE
            },
        },
    );
    op(
        &mut frame,
        Slot::Transparents,
        guest_triangle(2.0, [1.0; 4], Some(0)),
    );
    let mut observer = Observe {
        snap_first: true,
        ..Default::default()
    };
    let capture = draw_private_with_observer(
        &mut backend,
        &sealed(),
        &frame,
        1,
        Lighting::Retail,
        Some(&mut observer),
    )
    .unwrap();
    assert_eq!(
        observer.rows,
        [
            PassRow {
                colour: MAIN,
                depth: Z,
                ops: 0..3
            },
            PassRow {
                colour: MAIN,
                depth: Z,
                ops: 5..7
            },
        ]
    );
    assert_eq!(&observer.snapshots[0][0..4], [0, 0, 0, 255]);
    assert_eq!(rgba(&capture, 0, 0), [0, 0, 255, 255]);
    assert_eq!(rgba(&capture, 4, 4), [255, 0, 0, 255]);
}

#[test]
fn retail_and_e3_private_draws_of_one_sealed_tick_are_identical() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let scene = sealed();
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, clear());
    op(
        &mut frame,
        Slot::Opaque,
        typed_triangle(2.0, [1.0, 0.0, 0.0, 1.0]),
    );
    let first = draw_private(&mut backend, &scene, &frame, 1, Lighting::Retail).unwrap();
    let second = draw_private(&mut backend, &scene, &frame, 1, Lighting::E3).unwrap();
    assert_eq!((first.id, first.scene_hash), (second.id, second.scene_hash));
    assert_eq!(second.id.tick, scene.id.tick);
    assert_eq!(
        (first.report.lighting, second.report.lighting),
        (Lighting::Retail, Lighting::E3)
    );
    assert!(first.report.e3.is_none());
    assert_eq!(second.report.e3, Some(Default::default()));
    assert!(second.report.draws > 0, "the shared world draw still ran");
    assert_eq!(rgba(&first, 4, 4), [255, 0, 0, 255]);
    assert_eq!(second.size, first.size);
    assert_eq!(second.rgba, first.rgba);
    assert!(backend.last_presented().is_none());

    // Display is the only graph entry that publishes. A second capture
    // must leave the displayed image, its handle and the present hook alone.
    let display =
        draw_display(&mut backend, &scene, &frame, 1, Lighting::E3, 0, None, None).unwrap();
    assert_eq!(display.e3, second.report.e3);
    let presented = backend.last_presented().unwrap();
    let shown = backend.read_color(presented).unwrap().0;
    frame.ops.truncate(1);
    let black = draw_private(&mut backend, &scene, &frame, 1, Lighting::Retail).unwrap();
    assert_eq!(rgba(&black, 4, 4), [0, 0, 0, 255]);
    assert_eq!(backend.last_presented(), Some(presented));
    assert_eq!(backend.read_color(presented).unwrap().0, shown);
}

#[test]
fn private_draws_restore_inherited_colour_depth_and_stencil_before_blending() {
    use d3d::{Backend, DrawOutcome};
    use gfx::state::{BlendFactor, BlendOp, CompareFunc, StencilOp};
    use native_render::frame_graph::draw_display_on;
    use std::collections::HashMap;
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Ok(ctx) = GfxContext::headless() else { return };
    let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut private = GfxBackend::new(ctx.clone());
    let mut display = GfxBackend::new(ctx);
    let colour = display.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = display.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    display.clear(colour, Some(depth), Clear::all([0.0, 0.0, 1.0, 1.0])).unwrap();
    // Actual draws leave colour, W and stencil values on the live attachments.
    let mut earlier = triangle(2.0, [1.0, 0.0, 0.0, 1.0], 7, false);
    earlier.state.render.depth_func = CompareFunc::Always;
    assert_eq!(display.draw(colour, Some(depth), &earlier, [None; 4]), Ok(DrawOutcome::Drawn));

    // The sealed interval adds green once, only where both tests succeed.
    // Replaying over the *result* of this draw would add green twice.
    let mut add = triangle(2.0, [0.0, 0.5, 0.0, 1.0], 7, false);
    let r = &mut add.state.render;
    r.depth_func = CompareFunc::Equal;
    r.depth_write = false;
    r.stencil.func = CompareFunc::Equal;
    r.stencil.pass = StencilOp::Keep;
    r.blend_enable = true;
    r.blend_op = BlendOp::Add;
    r.src_blend = BlendFactor::One;
    r.dst_blend = BlendFactor::One;
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, bind());
    op(&mut frame, Slot::Opaque, ReplayOp::Draw {
        draw: Arc::new(add), textures: [None, None, None, None], camera: CameraUse::Unknown,
    });
    let targets = HashMap::from([(MAIN, colour), (Z, depth)]);
    let scene = sealed();
    assert!(frame.initial.is_empty());
    draw_display_on(&mut display, &scene, &frame, Lighting::Retail, 0, None, None, &targets).unwrap();
    assert_eq!(display.last_presented(), Some(colour));
    assert_eq!(display.readbacks(), 0);

    display.clear(colour, Some(depth), Clear::all([1.0; 4])).unwrap();
    display.flush().unwrap();
    display.destroy_surface(colour).unwrap();
    display.destroy_surface(depth).unwrap();
    let retail = draw_private(&mut private, &scene, &frame, 1, Lighting::Retail).unwrap();
    let e3 = draw_private(&mut private, &scene, &frame, 1, Lighting::E3).unwrap();
    assert_eq!(rgba(&retail, 0, 0), [0, 0, 255, 255]);
    let blended = rgba(&retail, 4, 4);
    assert_eq!([blended[0], blended[2], blended[3]], [255, 0, 255]);
    assert!((127..=128).contains(&blended[1]), "green must have been added once");
    assert_eq!(retail.rgba, e3.rgba);
    assert_eq!(retail.scene_hash, e3.scene_hash);
    assert!(retail.report.draws > 0 && e3.report.draws > 0);
    assert!(private.last_presented().is_none());
    assert!(pollster::block_on(validation.pop()).is_none());
}

mod live_views;
mod scaled;

#[test]
fn private_receivers_restore_the_renderer_and_the_swap_reports_its_own_mode() {
    use d3d::shader::PreparedShader;
    use d3d::{ResolvedDraw, ShaderProvider};

    struct ReceiverLog(Arc<Mutex<Vec<bool>>>);
    impl ShaderProvider for ReceiverLog {
        fn set_hard_shadows(&mut self, hard: bool) {
            self.0.lock().unwrap().push(hard);
        }

        fn prepare(
            &mut self,
            _ctx: &GfxContext,
            _renderer: &mut gfx::Renderer,
            _draw: &ResolvedDraw,
            _textures: [Option<&wgpu::TextureView>; 4],
        ) -> Result<PreparedShader, String> {
            Err("the receiver fixture only clears".into())
        }
    }

    let Ok(ctx) = GfxContext::headless() else {
        return;
    };
    let changes = Arc::new(Mutex::new(Vec::new()));
    let shown = Arc::new(Mutex::new(Vec::new()));
    let received = Arc::clone(&shown);
    let mut gpu = GfxBackend::new(ctx).with_shaders(Box::new(ReceiverLog(Arc::clone(&changes))));
    gpu.observe_guest_present(move |_, _, _, _, mode| received.lock().unwrap().push(mode));
    gpu.set_hard_shadows(true);
    let scene = sealed();
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, clear());
    for scale in [1, 2] {
        if scale == 2 {
            frame.aux.push(AuxTarget {
                width: SIZE / 2,
                height: SIZE / 2,
                ..frame.colour.unwrap()
            });
        }
        changes.lock().unwrap().clear();
        draw_private(&mut gpu, &scene, &frame, scale, Lighting::Retail).unwrap();
        assert_eq!(*changes.lock().unwrap(), [false, true]);
        assert!(shown.lock().unwrap().is_empty());
    }
    frame.aux.clear();
    for mode in [Lighting::E3, Lighting::Retail] {
        changes.lock().unwrap().clear();
        let report = draw_display(&mut gpu, &scene, &frame, 1, mode, 0, None, None).unwrap();
        assert_eq!(report.lighting, mode);
        assert_eq!(*changes.lock().unwrap(), [false, true]);
    }
    assert_eq!(*shown.lock().unwrap(), [Lighting::E3, Lighting::Retail]);
}
