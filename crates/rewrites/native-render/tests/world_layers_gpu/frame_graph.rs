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

fn test_gpu() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(ctx) => Some(ctx),
        Err(error) => {
            eprintln!("FrameScene pixel check skipped: {error}");
            None
        }
    }
}

fn sealed() -> Arc<FrameScene> {
    let mut builder = SceneBuilder::new(FreeList::new());
    builder.begin(SceneId {
        map_epoch: 2,
        tick: 17,
        frame: 12,
    });
    builder.seal(Stamp::default()).unwrap()
}

pub(super) fn replay() -> ReplayFrame {
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

pub(super) fn op(frame: &mut ReplayFrame, slot: Slot, op: ReplayOp) {
    frame.ops.push((SlotAt { slot, view: 0 }, op));
}

pub(super) fn bind() -> ReplayOp {
    ReplayOp::Bind {
        colour: MAIN,
        depth: Z,
    }
}

pub(super) fn clear() -> ReplayOp {
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
        external: None,
    }
}

/// A typed opaque stage. Its pipeline builds on a worker, so the stage is
/// first drawn outside the graph until the variant is admissible (as the
/// parent rig's `record` does); the graph's pass then shares that cache.
fn typed_triangle(backend: &mut GfxBackend, x: f32, colour: [f32; 4]) -> ReplayOp {
    use d3d::Backend as _;
    let mut draw = triangle(x, colour, 1, false);
    draw.state.render.depth_func = gfx::state::CompareFunc::Always;
    let cache = Arc::new(Cache::default());
    let color = backend.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = backend.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    let warm = ExternalSurfaces {
        color,
        depth,
        sampled: vec![],
        slots: vec![],
        slot_sizes: vec![],
    };
    assert!(
        super::record(backend, warm, &draw, [None; 4], &cache),
        "the typed pipeline compiles"
    );
    backend.flush().unwrap();
    let pass = IndexedPass::new(&draw, cache, 1, None).unwrap();
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
            display_projection_factor: 1.0,
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
    let Some(ctx) = test_gpu() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, clear());
    op(
        &mut frame,
        Slot::Opaque,
        typed_triangle(&mut backend, 2.0, [1.0, 0.0, 0.0, 1.0]),
    );
    op(
        &mut frame,
        Slot::Opaque,
        typed_triangle(&mut backend, 18.0, [0.0, 0.0, 1.0, 1.0]),
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

/// Commit at the sink, then draw the sealed stream through the display
/// and capture entry points. The light's explicit target is not the
/// guest's bound target: the guest draw after it must still use the latter.
#[test]
fn a_committed_light_stage_replays_once_without_rebinding_the_next_guest_draw() {
    use d3d::{Backend, SceneSink};
    use std::sync::atomic::{AtomicBool, Ordering};
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else { return };
    let mut gpu = GfxBackend::new(ctx);
    let ReplayOp::Stage(triangle_stage) = typed_triangle(&mut gpu, 2.0, [0., 1., 0., 1.]) else {
        unreachable!()
    };
    let frame = Arc::new(Mutex::new(None));
    let retained = Arc::clone(&frame);
    let armed = Arc::new(AtomicBool::new(true));
    let reference = Arc::clone(&armed);
    let mut sink = SceneSink::new(gpu, || {
        Some(SlotAt {
            slot: Slot::AfterLights,
            view: 0,
        })
    })
    .with_replay(move |f| *retained.lock().unwrap() = Some(f))
    .with_guest_reference_request(move || reference.load(Ordering::Relaxed));
    let color = sink.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let other = sink.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = sink.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    for surface in [color, other] {
        sink.clear(surface, Some(depth), Clear::all([0., 0., 0., 1.]))
            .unwrap();
    }
    let guest = |rgba| {
        let mut draw = triangle(2., rgba, 1, false);
        draw.state.render.depth_func = gfx::state::CompareFunc::Always;
        draw
    };
    sink.draw(other, Some(depth), &guest([1., 0., 0., 1.]), [None; 4])
        .unwrap();
    let stage = SceneStage {
        surfaces: ExternalSurfaces {
            color,
            depth,
            sampled: vec![],
            slots: vec![],
            slot_sizes: vec![],
        },
        plan: ExternalPlan {
            stage: timeline::native::Stage::Lights,
            ..triangle_stage.plan
        },
        pass: triangle_stage.pass,
        request: triangle_stage.request,
    };
    assert!(
        !sink.can_commit_carried(),
        "an armed reference retains the guest"
    );
    assert!(sink.prepare_carried_stage(&stage).unwrap().is_none());
    armed.store(false, Ordering::Relaxed);
    // A refused later step or tail drops this first reservation.
    drop(sink.prepare_carried_stage(&stage).unwrap().unwrap());
    let recordings = sink.prepare_carried_stage(&stage).unwrap().unwrap();
    sink.commit_carried_stage(stage, recordings);
    sink.draw(other, Some(depth), &guest([0., 0., 1., 1.]), [None; 4])
        .unwrap();
    sink.present(color, 0).unwrap();

    let frame = frame.lock().unwrap().take().unwrap();
    assert!(frame.refusals.is_empty());
    assert!(!frame.guest_reference_complete());
    let pos = frame
        .ops
        .iter()
        .position(|(_, op)| matches!(op, ReplayOp::Stage(_)))
        .unwrap();
    assert_eq!(
        frame
            .ops
            .iter()
            .filter(|(_, op)| matches!(op, ReplayOp::Stage(_)))
            .count(),
        1
    );
    assert!(matches!(frame.ops[pos - 1].1, ReplayOp::Draw { .. }));
    assert!(
        matches!(frame.ops[pos + 1].1, ReplayOp::Draw { .. }),
        "the later guest draw inherited its binding"
    );
    let scene = sealed();
    let gpu = sink.inner_mut();
    let shown = draw_display(gpu, &scene, &frame, 1, Lighting::Retail, 0, None, None).unwrap();
    assert_eq!(shown.draws, 3);
    let display = gpu.last_presented().unwrap();
    let pixels = gpu.read_color(display).unwrap().0;
    let at = ((4 * SIZE + 4) * 4) as usize;
    assert_eq!(&pixels[at..at + 4], &[0, 255, 0, 255]);
    for scale in [1, 4] {
        let capture = draw_private(gpu, &scene, &frame, scale, Lighting::Retail).unwrap();
        assert_eq!(capture.report.draws, 3);
        assert_eq!(rgba(&capture, 4 * scale, 4 * scale), [0, 255, 0, 255]);
    }
    assert_eq!(gpu.last_presented(), Some(display));
    assert_eq!(gpu.read_color(display).unwrap().0, pixels);
}

#[test]
fn carried_scene_colour_copy_samples_what_was_drawn_before_it() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else {
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
    let Some(ctx) = test_gpu() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let scene = sealed();
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, clear());
    op(
        &mut frame,
        Slot::Opaque,
        typed_triangle(&mut backend, 2.0, [1.0, 0.0, 0.0, 1.0]),
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

/// An object shadow's marks (`ReplayOp::ObjectShadow`) wrap its ops for
/// E3's sun tiles. With E3 off they draw nothing: the same pixels, the
/// same report and the same pass rows as the frame without them.
#[test]
fn object_shadow_marks_leave_the_retail_draw_and_its_passes_unchanged() {
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let scene = sealed();
    // The same ops, with or without the marks of shadows 0 and 1.
    let build = |backend: &mut GfxBackend, marks: bool| {
        let mut frame = replay();
        let mark = |frame: &mut ReplayFrame, shadow| {
            if marks {
                op(frame, Slot::Opaque, ReplayOp::ObjectShadow(shadow));
            }
        };
        op(&mut frame, Slot::Opaque, bind());
        op(&mut frame, Slot::Opaque, clear());
        mark(&mut frame, Some(0));
        op(
            &mut frame,
            Slot::Opaque,
            guest_triangle(0.5, [0.0, 0.0, 1.0, 1.0], None),
        );
        op(
            &mut frame,
            Slot::Opaque,
            typed_triangle(backend, 2.0, [1.0, 0.0, 0.0, 1.0]),
        );
        mark(&mut frame, Some(1));
        op(
            &mut frame,
            Slot::Opaque,
            guest_triangle(-0.5, [0.0, 1.0, 0.0, 1.0], None),
        );
        mark(&mut frame, None);
        op(
            &mut frame,
            Slot::Opaque,
            guest_triangle(1.5, [1.0, 1.0, 0.0, 1.0], None),
        );
        frame
    };
    let plain = build(&mut backend, false);
    let marked = build(&mut backend, true);
    assert_eq!(marked.ops.len(), plain.ops.len() + 3);

    let mut plain_rows = Observe::default();
    let without = draw_private_with_observer(
        &mut backend,
        &scene,
        &plain,
        1,
        Lighting::Retail,
        Some(&mut plain_rows),
    )
    .unwrap();
    let mut marked_rows = Observe::default();
    let with = draw_private_with_observer(
        &mut backend,
        &scene,
        &marked,
        1,
        Lighting::Retail,
        Some(&mut marked_rows),
    )
    .unwrap();
    assert!(without.report.draws > 0, "the frame drew");
    assert_eq!(with.report, without.report);
    assert_eq!(
        (with.id, with.scene_hash, with.size),
        (without.id, without.scene_hash, without.size)
    );
    assert_eq!(with.rgba, without.rgba);
    let attachments = |o: &Observe| {
        o.rows
            .iter()
            .map(|r| (r.colour, r.depth))
            .collect::<Vec<_>>()
    };
    assert_eq!(attachments(&marked_rows), attachments(&plain_rows));
    let draws = |o: &Observe, f: &ReplayFrame| {
        o.rows
            .iter()
            .map(|r| {
                f.ops[r.ops.clone()]
                    .iter()
                    .filter(|(_, op)| !op.is_marker())
                    .count()
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(draws(&marked_rows, &marked), draws(&plain_rows, &plain));
}

/// A scene of its own map epoch: one object at x = 10 in front of the
/// camera, casting one object shadow whose layer 11 draws on itself.
fn sun_scene(epoch: u64, definition: u32) -> Arc<FrameScene> {
    sun_scene_on(
        epoch,
        definition,
        frame_scene::LayerOwner::Object(frame_scene::InstanceKey(1)),
    )
}

/// [`sun_scene`], its shadow's layer 11 drawn on part 0 of `on`.
fn sun_scene_on(epoch: u64, definition: u32, on: frame_scene::LayerOwner) -> Arc<FrameScene> {
    use frame_scene::{
        GroupSite, InstanceInput, InstanceKey, LightBinding, LightEntry, LightGroup, LightPose,
        LightStream, NodeMatrix, PROJECTION_ROWS, RenderCamera, Topology, ViewKey, ViewKind,
    };
    let mut builder = SceneBuilder::new(FreeList::new());
    builder.begin(SceneId {
        map_epoch: epoch,
        tick: 17,
        frame: 12,
    });
    let camera = RenderCamera {
        position: [0.0, 0.0, 0.0],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        fov: std::f32::consts::FRAC_PI_2,
        projection_xy: None,
        viewport: [0, 0, SIZE as i16, SIZE as i16],
        near: 0.1,
        far: 100.0,
    };
    let key = ViewKey {
        kind: ViewKind::Game,
        player: 0,
        view: 0,
    };
    builder.open_view(key, camera).unwrap();
    let nodes = [NodeMatrix {
        position: [10.0, 0.0, 0.0],
        ..NodeMatrix::IDENTITY
    }];
    builder
        .push_instance(InstanceInput {
            key: InstanceKey(1),
            definition,
            model: 0xe600_0002,
            nodes: &nodes,
            sphere: ([10.0, 0.5, 0.5], 1.0),
            lighting: None,
            defaults: None,
        })
        .unwrap();
    builder.push_object_shadow([1, 0x3f80_0000], false).unwrap();
    let mut projection = [[0u32; 4]; PROJECTION_ROWS];
    projection[1] = [0.0f32.to_bits(), 0.0f32.to_bits(), 1.0f32.to_bits(), 0];
    let receiver = LightEntry {
        slot: 0,
        record: 0,
        owner: on,
        model: 0,
        stream: LightStream {
            section: 0,
            part: 0,
            vertex_class: 0,
            first_index: 0,
            index_count: 3,
            topology: Topology::List,
        },
        pose: LightPose::World,
        binding: LightBinding {
            shader: 0,
            selector: 0,
            variant: 0,
            implementation: 3,
        },
    };
    let group = LightGroup {
        light: 0,
        layer: 11,
        site: GroupSite::ObjectShadow,
        projection: Some(projection),
        pass: None,
        entries: 0..0,
        remainder: 0,
    };
    builder
        .push_shadow_group(group, &[(receiver, Vec::new())])
        .unwrap();
    builder.set_shadow_weight(0.75).unwrap();
    builder.seal(Stamp::default()).unwrap()
}

/// One triangle across node 0's rest frame.
struct SunModels(native_render::model::Model);

impl native_render::sun_casters::CasterModels for SunModels {
    fn model(&self, _definition: u32) -> Option<&native_render::model::Model> {
        Some(&self.0)
    }
}

fn sun_model() -> native_render::model::Model {
    use native_render::model::{Model, ModelDraw, ModelVertex, NodeMatrix};
    let v = |p: [f32; 3]| ModelVertex {
        position: p,
        normal: [0.0, 0.0, 1.0],
        nodes: [0; 4],
        weights: [1.0, 0.0, 0.0, 0.0],
        ..Default::default()
    };
    Model {
        vertices: vec![v([0.0, -1.0, 0.0]), v([0.0, 1.0, 0.0]), v([0.0, 0.0, 1.0])],
        indices: vec![0, 1, 2],
        draws: vec![ModelDraw {
            material: 0,
            first_index: 0,
            index_count: 3,
        }],
        inverse_binds: vec![NodeMatrix::IDENTITY],
        ..Default::default()
    }
}

/// Under E3 an admitted object shadow's marked ops are not drawn: its sun
/// tile and receivers stand in, and the tile is kept for the next draw.
/// With E3 off the same frame, marks, scene and loaded models replay
/// exactly as the frame without marks.
#[test]
fn e3_skips_an_admitted_shadows_mask_and_retail_replays_it_unchanged() {
    use native_render::sun_casters::set_models;
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else {
        return;
    };
    const EPOCH: u64 = 0x5e3_0001;
    let mut backend = GfxBackend::new(ctx);
    let scene = sun_scene(EPOCH, 0xe500_0001);
    set_models(EPOCH, Some(Arc::new(SunModels(sun_model()))));
    // The mask's draw, magenta at (19, 5), inside shadow 0's marks.
    let build = |marks: bool| {
        let mut frame = replay();
        op(&mut frame, Slot::Opaque, bind());
        op(&mut frame, Slot::Opaque, clear());
        op(
            &mut frame,
            Slot::Opaque,
            guest_triangle(0.0, [0.0, 0.0, 1.0, 1.0], None),
        );
        if marks {
            op(&mut frame, Slot::Opaque, ReplayOp::ObjectShadow(Some(0)));
        }
        op(
            &mut frame,
            Slot::Opaque,
            guest_triangle(16.0, [1.0, 0.0, 1.0, 1.0], None),
        );
        if marks {
            op(&mut frame, Slot::Opaque, ReplayOp::ObjectShadow(None));
        }
        frame
    };
    let (plain, marked) = (build(false), build(true));

    let without = draw_private(&mut backend, &scene, &plain, 1, Lighting::Retail).unwrap();
    let with = draw_private(&mut backend, &scene, &marked, 1, Lighting::Retail).unwrap();
    assert_eq!(
        rgba(&with, 19, 5),
        [255, 0, 255, 255],
        "retail draws the mask"
    );
    assert_eq!(with.rgba, without.rgba);
    assert_eq!(with.report, without.report);

    let e3 = draw_private(&mut backend, &scene, &marked, 1, Lighting::E3).unwrap();
    let report = e3.report.e3.unwrap();
    assert_eq!(report.sun_tiles, 1);
    assert_eq!(
        report.sun_tile_draws.draws, 1,
        "a fresh atlas draws the tile"
    );
    assert!(report.sun.draws >= 1, "the caster receives its own tile");
    assert_eq!(rgba(&e3, 19, 5), [0, 0, 0, 255], "the mask is not drawn");
    assert_eq!(
        rgba(&e3, 3, 5),
        [0, 0, 255, 255],
        "work outside the marks is"
    );
    let again = draw_private(&mut backend, &scene, &marked, 1, Lighting::E3).unwrap();
    let report = again.report.e3.unwrap();
    assert_eq!(
        (report.sun_tiles, report.sun_tile_draws.draws),
        (1, 0),
        "the tile is kept"
    );

    // No models for the map: every shadow keeps its mask under E3 too.
    set_models(EPOCH, None);
    let kept = draw_private(&mut backend, &scene, &marked, 1, Lighting::E3).unwrap();
    assert_eq!(rgba(&kept, 19, 5), [255, 0, 255, 255]);
    assert_eq!(kept.report.e3.unwrap().sun_tiles, 0);
}

/// Under E3 a shadow whose layer 11 falls on a structure cluster draws
/// its sun tile on the loaded world's part in place of the mask. With no
/// world offered for the map, or once the guest has left the world's BSP,
/// the same shadow keeps the mask.
#[test]
fn e3_draws_a_shadow_on_the_structure_and_keeps_the_mask_without_its_world() {
    use native_render::sun_casters::{SunWorld, set_models, set_world};
    use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};
    use std::sync::atomic::{AtomicBool, Ordering};
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else {
        return;
    };
    const EPOCH: u64 = 0x5e3_0002;
    let mut backend = GfxBackend::new(ctx);
    let scene = sun_scene_on(EPOCH, 0xe500_0001, frame_scene::LayerOwner::Cluster(0));
    set_models(EPOCH, Some(Arc::new(SunModels(sun_model()))));
    // Cluster 0's part 0: the ground under the caster.
    let mut built = WorldBuilder::new();
    let ground = SectionInput {
        positions: vec![
            [9.0, -1.0, 0.0],
            [11.0, -1.0, 0.0],
            [11.0, 1.0, 0.0],
            [9.0, 1.0, 0.0],
        ],
        uvs: vec![[0.0; 2]; 4],
        normals: vec![[0.0, 0.0, 1.0]; 4],
        parts: vec![PartInput {
            material: 0,
            transparent: false,
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        }],
        ..Default::default()
    };
    built.add_section(Owner::Cluster(0), &ground, &Placement::IDENTITY, None);
    let current = Arc::new(AtomicBool::new(true));
    let held = Arc::clone(&current);
    let world = SunWorld::new(&built.finish()).with_current(move || held.load(Ordering::Relaxed));
    // The mask's draw, magenta at (19, 5), inside shadow 0's marks.
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, bind());
    op(&mut frame, Slot::Opaque, clear());
    op(
        &mut frame,
        Slot::Opaque,
        guest_triangle(0.0, [0.0, 0.0, 1.0, 1.0], None),
    );
    op(&mut frame, Slot::Opaque, ReplayOp::ObjectShadow(Some(0)));
    op(
        &mut frame,
        Slot::Opaque,
        guest_triangle(16.0, [1.0, 0.0, 1.0, 1.0], None),
    );
    op(&mut frame, Slot::Opaque, ReplayOp::ObjectShadow(None));

    let unloaded = draw_private(&mut backend, &scene, &frame, 1, Lighting::E3).unwrap();
    assert_eq!(
        rgba(&unloaded, 19, 5),
        [255, 0, 255, 255],
        "no world: the mask"
    );
    assert_eq!(unloaded.report.e3.unwrap().sun_tiles, 0);

    set_world(EPOCH, Some(Arc::new(world)));
    let e3 = draw_private(&mut backend, &scene, &frame, 1, Lighting::E3).unwrap();
    let report = e3.report.e3.unwrap();
    assert_eq!(report.sun_tiles, 1);
    assert!(
        report.sun.draws >= 1,
        "the cluster's ground receives the tile"
    );
    assert_eq!(rgba(&e3, 19, 5), [0, 0, 0, 255], "the mask is not drawn");
    assert_eq!(
        rgba(&e3, 3, 5),
        [0, 0, 255, 255],
        "work outside the marks is"
    );

    current.store(false, Ordering::Relaxed);
    let left = draw_private(&mut backend, &scene, &frame, 1, Lighting::E3).unwrap();
    assert_eq!(
        rgba(&left, 19, 5),
        [255, 0, 255, 255],
        "another BSP: the mask"
    );
    assert_eq!(left.report.e3.unwrap().sun_tiles, 0);
    set_world(EPOCH, None);
    set_models(EPOCH, None);
}

/// A view with twenty eligible candidate lights: an E3 draw reports the
/// renderer's choice, sixteen lit and four over the cap, every run; retail
/// reports none, and the pixels are the same in both modes (the chosen
/// lights draw no pass of their own yet).
#[test]
fn e3_reports_its_choice_of_sixteen_lights_from_twenty() {
    use frame_scene::{Light, LightCandidate, RenderCamera, ViewKey, ViewKind};
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else {
        return;
    };
    let mut backend = GfxBackend::new(ctx);
    let mut builder = SceneBuilder::new(FreeList::new());
    builder.begin(SceneId {
        map_epoch: 3,
        tick: 17,
        frame: 12,
    });
    let camera = RenderCamera {
        position: [0.0, 0.0, 0.0],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        fov: std::f32::consts::FRAC_PI_2,
        projection_xy: None,
        viewport: [0, 0, SIZE as i16, SIZE as i16],
        near: 0.1,
        far: 100.0,
    };
    let key = ViewKey {
        kind: ViewKind::Game,
        player: 0,
        view: 0,
    };
    builder.open_view(key, camera).unwrap();
    let lights = (0..20u32)
        .map(|k| Light {
            handle: 0xe100_0000 | k,
            candidate: Some(LightCandidate {
                position: [5.0, 0.0, 0.0],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                colour_a: [1.0; 3],
                colour_b: [1.0; 3],
                intensity: 1.0,
                bounds: ([5.0, 0.0, 0.0], 2.0),
                owner: None,
                definition_kind: 0,
                volume: None,
                view_eligible: Some(true),
                priority: Some(1.0 + k as f32),
                weight: 1.0,
                gel_bitmap: None,
                gel_shader: None,
                constants: None,
            }),
            ..Default::default()
        })
        .collect();
    builder.add_light_candidates(lights).unwrap();
    let scene = builder.seal(Stamp::default()).unwrap();
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, bind());
    op(&mut frame, Slot::Opaque, clear());

    let retail = draw_private(&mut backend, &scene, &frame, 1, Lighting::Retail).unwrap();
    assert!(retail.report.e3.is_none());
    for _ in 0..2 {
        let e3 = draw_private(&mut backend, &scene, &frame, 1, Lighting::E3).unwrap();
        let report = e3.report.e3.unwrap();
        assert_eq!((report.lights_lit, report.lights_over_cap), (16, 4));
        // The guest drew none of them; with no chosen-light stage in the
        // replay, `e3.lights` has no draws.
        assert_eq!(report.lights_beyond_guest, 16);
        assert_eq!(report.lights.draws, 0);
        assert_eq!(e3.rgba, retail.rgba);
    }
}

#[test]
fn private_draws_restore_inherited_colour_depth_and_stencil_before_blending() {
    use d3d::{Backend, DrawOutcome};
    use gfx::state::{BlendFactor, BlendOp, CompareFunc, StencilOp};
    use native_render::frame_graph::draw_display_on;
    use std::collections::HashMap;
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else { return };
    let validation = ctx.device.push_error_scope(wgpu::ErrorFilter::Validation);
    let mut private = GfxBackend::new(ctx.clone());
    let mut display = GfxBackend::new(ctx);
    let colour = display.create_surface(FORMAT, SIZE, SIZE, false).unwrap();
    let depth = display.create_surface(DEPTH, SIZE, SIZE, true).unwrap();
    display
        .clear(colour, Some(depth), Clear::all([0.0, 0.0, 1.0, 1.0]))
        .unwrap();
    // Actual draws leave colour, W and stencil values on the live attachments.
    let mut earlier = triangle(2.0, [1.0, 0.0, 0.0, 1.0], 7, false);
    earlier.state.render.depth_func = CompareFunc::Always;
    assert_eq!(
        display.draw(colour, Some(depth), &earlier, [None; 4]),
        Ok(DrawOutcome::Drawn)
    );

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
    op(
        &mut frame,
        Slot::Opaque,
        ReplayOp::Draw {
            draw: Arc::new(add),
            textures: [None, None, None, None],
            camera: CameraUse::Unknown,
            external: None,
        },
    );
    let targets = HashMap::from([(MAIN, colour), (Z, depth)]);
    let scene = sealed();
    assert!(frame.initial.is_empty());
    draw_display_on(
        &mut display,
        &scene,
        &frame,
        Lighting::Retail,
        0,
        None,
        None,
        &targets,
    )
    .unwrap();
    assert_eq!(display.last_presented(), Some(colour));
    assert_eq!(display.readbacks(), 0);

    display
        .clear(colour, Some(depth), Clear::all([1.0; 4]))
        .unwrap();
    display.flush().unwrap();
    display.destroy_surface(colour).unwrap();
    display.destroy_surface(depth).unwrap();
    let retail = draw_private(&mut private, &scene, &frame, 1, Lighting::Retail).unwrap();
    let e3 = draw_private(&mut private, &scene, &frame, 1, Lighting::E3).unwrap();
    assert_eq!(rgba(&retail, 0, 0), [0, 0, 255, 255]);
    let blended = rgba(&retail, 4, 4);
    assert_eq!([blended[0], blended[2], blended[3]], [255, 0, 255]);
    assert!(
        (127..=128).contains(&blended[1]),
        "green must have been added once"
    );
    assert_eq!(retail.rgba, e3.rgba);
    assert_eq!(retail.scene_hash, e3.scene_hash);
    assert!(retail.report.draws > 0 && e3.report.draws > 0);
    assert!(private.last_presented().is_none());
    assert!(pollster::block_on(validation.pop()).is_none());
}

#[path = "frame_graph/live_views.rs"]
mod live_views;
#[path = "frame_graph/scaled.rs"]
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

    let Some(ctx) = test_gpu() else {
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

/// The camera looks straight down at a flat white floor. A raised triangle
/// above the eye is absent from both the visible walk and the mask list.
#[test]
fn a_caster_behind_the_camera_shadows_the_ground_in_view() {
    use frame_scene::{NodeMatrix, RenderCamera, ViewKey, ViewKind};
    use native_render::sun_casters::{SunWorld, set_models, set_world};
    use native_render::world::{Owner, PartInput, Placement, SectionInput, WorldBuilder};
    use scene_extract::{
        layout::{nearby_casters, object_nodes::TAG_INSTANCES},
        testing::{self, BODIES, Mem},
    };
    let _guard = CAPTURE_LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let Some(ctx) = test_gpu() else { return };
    const EPOCH: u64 = 0x5e3_0008;
    let camera = RenderCamera {
        position: [0., 0., 1.],
        forward: [0., 0., -1.],
        up: [0., 1., 0.],
        fov: std::f32::consts::FRAC_PI_2,
        projection_xy: None,
        viewport: [0, 0, SIZE as i16, SIZE as i16],
        near: 0.1,
        far: 100.,
    };
    let mut mem = Mem::default();
    testing::object(&mut mem, 0, 0, 1);
    mem.w(BODIES + 4, 1 << 24);
    mem.w(BODIES + 0x14, u32::MAX);
    mem.w(BODIES + 0xa8, 0);
    mem.w(TAG_INSTANCES, 0x2000);
    mem.w(0x2008, 0x3000);
    mem.w(0x3018, 2);
    mem.w(nearby_casters::DEFAULT_LIGHTING, 0x4000);
    for (i, v) in [0., 0., 1.].into_iter().enumerate() {
        mem.f(0x4028 + i as u32 * 4, v);
    }
    mem.f(0x4038, 0.75);
    for (i, v) in [0., 0., 2., 0.8].into_iter().enumerate() {
        mem.f(BODIES + 0x30 + i as u32 * 4, v);
    }
    let node = NodeMatrix {
        position: [0., 0., 2.],
        ..NodeMatrix::IDENTITY
    };
    for (i, &v) in bytemuck::cast_ref::<_, [f32; 13]>(&node).iter().enumerate() {
        mem.f(BODIES + 0x200 + i as u32 * 4, v);
    }
    let mut b = SceneBuilder::new(FreeList::new());
    b.begin(SceneId {
        map_epoch: EPOCH,
        tick: 1,
        frame: 1,
    });
    b.open_view(
        ViewKey {
            kind: ViewKind::Game,
            player: 0,
            view: 0,
        },
        camera,
    )
    .unwrap();
    b.set_clusters([0]).unwrap();
    let casters = nearby_casters::extract(&mem, &camera);
    assert_eq!(casters.len(), 1);
    b.add_nearby_casters(casters).unwrap();
    let scene = b.seal(Stamp::default()).unwrap();
    let mut model = sun_model();
    for (v, p) in model
        .vertices
        .iter_mut()
        .zip([[-0.5, -0.5, 0.], [0.5, -0.5, 0.], [0., 0.5, 0.]])
    {
        v.position = p;
    }
    set_models(EPOCH, Some(Arc::new(SunModels(model))));
    let mut world = WorldBuilder::new();
    let floor = SectionInput {
        positions: vec![[-1., -1., 0.], [1., -1., 0.], [1., 1., 0.], [-1., 1., 0.]],
        normals: vec![[0., 0., 1.]; 4],
        uvs: vec![[0.; 2]; 4],
        parts: vec![PartInput {
            material: 0,
            transparent: false,
            triangles: vec![[0, 1, 2], [0, 2, 3]],
        }],
        ..Default::default()
    };
    world.add_section(Owner::Cluster(0), &floor, &Placement::IDENTITY, None);
    set_world(EPOCH, Some(Arc::new(SunWorld::new(&world.finish()))));
    let mut backend = GfxBackend::new(ctx);
    let mut frame = replay();
    op(&mut frame, Slot::Opaque, bind());
    op(
        &mut frame,
        Slot::Opaque,
        ReplayOp::Clear {
            colour: MAIN,
            depth: Z,
            clear: Clear {
                depth: Some(1.0 / camera.far),
                ..Clear::all([1.; 4])
            },
        },
    );
    let retail = draw_private(&mut backend, &scene, &frame, 1, Lighting::Retail).unwrap();
    let e3 = draw_private(&mut backend, &scene, &frame, 1, Lighting::E3).unwrap();
    assert_eq!(rgba(&retail, SIZE / 2, SIZE / 2), [255; 4]);
    assert!(rgba(&e3, SIZE / 2, SIZE / 2)[0] < 100);
    assert_eq!(rgba(&e3, 2, 2), [255; 4]);
    let work = e3.report.e3.unwrap();
    assert_eq!(work.sun_tiles_offscreen, 1);
    assert_eq!(
        (work.sun_tiles, work.sun_tiles_redrawn, work.sun.draws),
        (1, 1, 1)
    );
    set_models(EPOCH, None);
    set_world(EPOCH, None);
}
