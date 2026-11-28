//! A carried draw's camera is proved from its active program, matching view
//! and bit-exact projection. All vertex programs and packets are synthetic.

use std::sync::Arc;

use crate::common::{TRIANGLESTRIP, bind_quad, create, quad_vb, set_states};
use d3d::scene_sink::{ReplayFrame, ReplayOp};
use d3d::shader::VertexProcessing;
use d3d::{RecordingBackend, ResolvedDraw, UploadPolicy, Viewport};
use frame_scene::carried::CameraUse;
use frame_scene::{
    Camera, FrameScene, SceneBuilder, SceneId, Slot, SlotAt, Stamp, ViewKey, ViewKind,
};
use shaders::vsh::encode::{Inst, Operand};
use shaders::vsh::{MacOp, OutReg};

const FIRST: u8 = 4;

fn camera(left: i16) -> Camera {
    Camera {
        position: [2.0, 4.0, 8.0],
        forward: [1.0, 0.0, 0.0],
        up: [0.0, 0.0, 1.0],
        viewport: [0, left, 60, left + 80],
        fov: 1.2,
        projection_xy: None,
        near: 0.25,
        far: 120.0,
    }
}

fn scene(views: &[(u8, Camera)]) -> Arc<FrameScene> {
    let mut builder = SceneBuilder::default();
    builder.begin(SceneId::default());
    for &(view, cam) in views {
        builder
            .open_view(
                ViewKey {
                    kind: ViewKind::Game,
                    player: view,
                    view,
                },
                cam,
            )
            .unwrap();
    }
    builder.seal(Stamp::default()).unwrap()
}

/// The resolved packet of one guest draw, as an admission is offered it.
fn packet() -> ResolvedDraw {
    struct Capture(std::sync::Mutex<Option<ResolvedDraw>>);
    impl d3d::DrawAdmission for Capture {
        fn admit(
            &self,
            snapshot: &d3d::DrawSnapshot<'_>,
        ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
            *self.0.lock().unwrap() = Some(snapshot.draw.clone());
            None
        }
    }

    let (mut d, mut mem) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut d, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 18.0, 18.0, 0xFF00_4400);
    bind_quad(&mut d, &mut mem);
    let capture = Arc::new(Capture(std::sync::Mutex::new(None)));
    d.add_draw_admission(capture.clone());
    d.draw_vertices(&mut mem, TRIANGLESTRIP, 0, 4).unwrap();
    capture.0.lock().unwrap().take().expect("the draw was offered to the admission")
}

fn mapped_draw(cam: &Camera) -> ResolvedDraw {
    let mut draw = packet();
    draw.layout.pretransformed = false;
    let [top, left, bottom, right] = cam.viewport;
    draw.viewport = Viewport {
        x: left as u32,
        y: top as u32,
        width: (right - left) as u32,
        height: (bottom - top) as u32,
        min_z: 0.0,
        max_z: 1.0,
    };
    let matrix = cam.view_projection();
    let constants = Arc::make_mut(&mut draw.shader.constants);
    let mut program = Vec::new();
    for i in 0..4u8 {
        constants[usize::from(FIRST + i)] = matrix.map(|column| column[usize::from(i)]);
        let inst = Inst::mac(
            MacOp::Dp4,
            Some(Operand::v(0)),
            Some(Operand::c(FIRST + i)),
            None,
        )
        .to_out(false, OutReg::Pos, 8 >> i);
        program.extend(if i == 3 { inst.last() } else { inst }.words());
    }
    draw.shader.vertex = VertexProcessing::Program {
        start: 0,
        microcode: program.into(),
    };
    draw
}

fn push(frame: &mut ReplayFrame, slot: Slot, view: u8, draw: ResolvedDraw) {
    frame.ops.push((
        SlotAt { slot, view },
        ReplayOp::Draw {
            draw: draw.into(),
            textures: Default::default(),
            camera: CameraUse::Unknown,
            external: None,
        },
    ));
}

fn labels(frame: &ReplayFrame) -> Vec<CameraUse> {
    frame
        .ops
        .iter()
        .filter_map(|(_, op)| match op {
            ReplayOp::Draw { camera, .. } => Some(*camera),
            _ => None,
        })
        .collect()
}

#[test]
fn only_the_programs_actual_matrix_in_its_own_view_can_follow_the_camera() {
    let cam = camera(80);
    let views = scene(&[(0, camera(0)), (1, cam)]);
    let draw = mapped_draw(&cam);
    let mut replay = ReplayFrame::default();
    push(&mut replay, Slot::Opaque, 1, draw.clone());
    push(&mut replay, Slot::Opaque, 0, draw.clone()); // Same size and projection, different viewport origin.
    push(&mut replay, Slot::AfterLights, 2, draw.clone());
    push(&mut replay, Slot::Overlay, 1, draw.clone());
    let mut other = draw;
    let rows = Arc::make_mut(&mut other.shader.constants);
    let first = usize::from(FIRST);
    rows.copy_within(first..first + 4, 30); // An identical block elsewhere proves nothing.
    let zero = rows[first..first + 4]
        .iter_mut()
        .flatten()
        .find(|v| **v == 0.0)
        .unwrap();
    *zero = f32::from_bits(zero.to_bits() ^ (1 << 31)); // Numerically equal, different upload.
    push(&mut replay, Slot::Opaque, 1, other);
    replay.classify_cameras(views.view_cameras());
    assert_eq!(
        labels(&replay),
        [
            CameraUse::View {
                view: 1,
                first: FIRST
            },
            CameraUse::Unknown,
            CameraUse::Unknown,
            CameraUse::Unknown,
            CameraUse::Unknown,
        ]
    );
    assert_eq!(replay.camera_counts(1), [0, 1, 2]);

    // A re-entered view index cannot identify which camera to substitute.
    replay.classify_cameras(scene(&[(1, cam), (1, Camera { fov: 0.7, ..cam })]).view_cameras());
    assert!(labels(&replay).iter().all(|c| *c == CameraUse::Unknown));
}

#[test]
fn screen_draws_and_unknown_programs_keep_distinct_labels() {
    let mut frame = ReplayFrame::default();
    let mut screen = packet();
    screen.layout.pretransformed = true;
    screen.shader.vertex = VertexProcessing::FixedFunction { fvf: 0 };
    push(&mut frame, Slot::Opaque, 1, screen.clone());
    screen.layout.pretransformed = false;
    push(&mut frame, Slot::Opaque, 1, screen.clone());

    let pass = Inst::mac(MacOp::Mov, Some(Operand::v(0)), None, None)
        .to_out(false, OutReg::Pos, 0xf)
        .last();
    screen.shader.vertex = VertexProcessing::Program {
        start: 0,
        microcode: pass.words().to_vec().into(),
    };
    push(&mut frame, Slot::Overlay, 1, screen.clone());
    push(&mut frame, Slot::Outside, 0, screen.clone());
    screen.shader.vertex = VertexProcessing::Program {
        start: 0,
        microcode: Vec::new().into(),
    };
    push(&mut frame, Slot::Opaque, 1, screen);

    frame.classify_cameras(scene(&[(1, camera(0))]).view_cameras());
    assert_eq!(
        labels(&frame),
        [
            CameraUse::Screen,
            CameraUse::Unknown,
            CameraUse::Screen,
            CameraUse::Screen,
            CameraUse::Unknown
        ]
    );
    assert_eq!(frame.camera_counts(1), [2, 0, 2]);
    assert_eq!(frame.camera_counts(0), [0, 0, 0]);
}

fn rows_at(op: &ReplayOp) -> [[f32; 4]; 4] {
    let ReplayOp::Draw { draw, .. } = op else { panic!("{op:?}") };
    let first = usize::from(FIRST);
    draw.shader.constants[first..first + 4].try_into().unwrap()
}

#[test]
fn a_display_frame_moves_only_verified_view_draws_onto_the_blended_camera() {
    use d3d::scene_sink::{Retarget, Retargeted};
    let cam = camera(0);
    let b = scene(&[(1, cam)]);
    let draw = mapped_draw(&cam);
    let mut replay = ReplayFrame::default();
    push(&mut replay, Slot::Opaque, 1, draw.clone());
    push(&mut replay, Slot::Overlay, 1, draw.clone());
    replay.classify_cameras(b.view_cameras());
    assert_eq!(labels(&replay)[1], CameraUse::Unknown, "the overlay is not the view's");

    let turned = Camera { forward: [0.0, 1.0, 0.0], ..cam };
    let key = ViewKey { kind: ViewKind::Game, player: 1, view: 1 };
    let mut moved = Retarget::new(b.view_cameras(), [(key, &turned)]);
    assert!(!moved.is_empty());
    let (at, op) = &replay.ops[0];
    let out = moved.op(*at, op);
    assert!(matches!(out, Retargeted::Moved(_)));
    let want = turned.view_projection();
    let want: [[f32; 4]; 4] = std::array::from_fn(|row| want.map(|column| column[row]));
    assert_eq!(rows_at(out.get()), want, "the blended camera's view block");
    assert_eq!(rows_at(op), rows_at(&ReplayOp::Draw {
        draw: Arc::new(mapped_draw(&cam)),
        textures: Default::default(),
        camera: CameraUse::Unknown,
        external: None,
    }), "the sealed frame keeps the tick's");
    let (at, op) = &replay.ops[1];
    assert!(matches!(moved.op(*at, op), Retargeted::Same(_)), "unverified: drawn as the tick drew it");
    assert_eq!(moved.counts().draws, 1);

    // The same camera moves nothing.
    let still = Retarget::new(b.view_cameras(), [(key, &cam)]);
    assert!(still.is_empty());
}

#[test]
fn retained_geometry_moves_with_the_same_verified_camera_as_its_packet() {
    use d3d::scene_sink::{RetainedExternal, Retarget, Retargeted};
    use d3d::{ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces};
    use frame_scene::TargetRef;
    use std::sync::Mutex;
    use timeline::native::Stage;

    struct Retained(Arc<ResolvedDraw>);
    impl ExternalPass for Retained {
        fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
            Some(Arc::clone(&self.0))
        }
        fn with_draw(&self, draw: Arc<ResolvedDraw>) -> Option<Box<dyn ExternalPass>> {
            Some(Box::new(Self(draw)))
        }
    }

    let cam = camera(0);
    let b = scene(&[(1, cam)]);
    let turned = Camera { forward: [0.0, 1.0, 0.0], ..cam };
    let key = ViewKey { kind: ViewKind::Game, player: 1, view: 1 };
    let at = SlotAt { slot: Slot::Transparents, view: 1 };
    let mut retarget = Retarget::new(b.view_cameras(), [(key, &turned)]);
    let draw = Arc::new(mapped_draw(&cam));
    for stage in [Stage::Transparents, Stage::Particles, Stage::Flares] {
        let pass: Box<dyn ExternalPass> = Box::new(Retained(Arc::clone(&draw)));
        let op = ReplayOp::Draw {
            draw: Arc::clone(&draw),
            textures: Default::default(),
            camera: CameraUse::View { view: 1, first: FIRST },
            external: Some(Box::new(RetainedExternal {
                packets: Arc::from([Arc::clone(&draw)]),
                at: 0,
                surfaces: ExternalSurfaces {
                    color: TargetRef::SceneColour,
                    depth: TargetRef::SceneDepth,
                    sampled: Vec::new(),
                    slots: Vec::new(),
                    slot_sizes: Vec::new(),
                },
                request: ExternalPassRequest {
                    rect: (0, 0, 80, 60),
                    surface_size: (80, 60),
                    z_scale: 1.0,
                    display_projection_factor: 1.0,
                },
                plan: ExternalPlan { stage, view: 1, ranges: 1, slots: Vec::new() },
                pass: Arc::new(Mutex::new(pass)),
            })),
        };
        let moved = retarget.op(at, &op);
        if stage == Stage::Flares {
            assert!(matches!(moved, Retargeted::Same(_)));
            continue;
        }
        assert_ne!(rows_at(moved.get()), rows_at(&op));
        let ReplayOp::Draw { draw, external: Some(admission), .. } = moved.get() else {
            panic!("the remapped draw lost its native replacement");
        };
        assert!(Arc::ptr_eq(draw, admission.packet()));
        let fallback = admission.pass.lock().unwrap().resolved_fallback().unwrap();
        assert!(Arc::ptr_eq(draw, &fallback));
    }
}

/// A widened camera at render scale 0.75: a retained replacement takes the
/// widened projection the scene's terrain took (its rows are exactly the
/// blended camera's), keeps the guest viewport it was captured with, and
/// that viewport lands on the 0.75 grid the same way the terrain's does.
#[test]
fn retained_geometry_takes_the_widened_projection_and_the_scaled_grid() {
    use d3d::scene_sink::{RetainedExternal, Retarget};
    use d3d::scaled::RenderScale;
    use d3d::{ExternalPass, ExternalPassRequest, ExternalPlan, ExternalSurfaces};
    use frame_scene::TargetRef;
    use std::sync::Mutex;
    use timeline::native::Stage;

    struct Retained(Arc<ResolvedDraw>);
    impl ExternalPass for Retained {
        fn resolved_fallback(&self) -> Option<Arc<ResolvedDraw>> {
            Some(Arc::clone(&self.0))
        }
        fn with_draw(&self, draw: Arc<ResolvedDraw>) -> Option<Box<dyn ExternalPass>> {
            Some(Box::new(Self(draw)))
        }
    }

    let cam = camera(0);
    let b = scene(&[(1, cam)]);
    let wide = Camera { fov: 1.9, ..cam };
    let key = ViewKey { kind: ViewKind::Game, player: 1, view: 1 };
    let at = SlotAt { slot: Slot::Transparents, view: 1 };
    let mut retarget = Retarget::new(b.view_cameras(), [(key, &wide)]);
    let draw = Arc::new(mapped_draw(&cam));
    let want = wide.view_projection();
    let want: [[f32; 4]; 4] = std::array::from_fn(|row| want.map(|column| column[row]));
    let narrow = camera(0).view_projection();
    let narrow: [[f32; 4]; 4] = std::array::from_fn(|row| narrow.map(|column| column[row]));
    assert_ne!(want, narrow, "the widened projection differs");
    let guest = (80, 60);
    let scale = RenderScale::between(guest, (60, 45)).unwrap();
    for stage in [Stage::Transparents, Stage::Particles] {
        let pass: Box<dyn ExternalPass> = Box::new(Retained(Arc::clone(&draw)));
        let op = ReplayOp::Draw {
            draw: Arc::clone(&draw),
            textures: Default::default(),
            camera: CameraUse::View { view: 1, first: FIRST },
            external: Some(Box::new(RetainedExternal {
                packets: Arc::from([Arc::clone(&draw)]),
                at: 0,
                surfaces: ExternalSurfaces {
                    color: TargetRef::SceneColour,
                    depth: TargetRef::SceneDepth,
                    sampled: Vec::new(),
                    slots: Vec::new(),
                    slot_sizes: Vec::new(),
                },
                request: ExternalPassRequest {
                    rect: (0, 0, 80, 60),
                    surface_size: (80, 60),
                    z_scale: 1.0,
                    display_projection_factor: 1.0,
                },
                plan: ExternalPlan { stage, view: 1, ranges: 1, slots: Vec::new() },
                pass: Arc::new(Mutex::new(pass)),
            })),
        };
        let moved = retarget.op(at, &op);
        assert_eq!(rows_at(moved.get()), want, "{stage:?}: the widened projection");
        let ReplayOp::Draw { draw: moved_draw, external: Some(admission), .. } = moved.get() else {
            panic!("the remapped draw lost its native replacement");
        };
        assert_eq!(moved_draw.viewport, draw.viewport, "{stage:?}: the guest viewport");
        let fallback = admission.pass.lock().unwrap().resolved_fallback().unwrap();
        assert_eq!(fallback.shader.constants[usize::from(FIRST)..usize::from(FIRST) + 4], want);
        let v = moved_draw.viewport;
        assert_eq!(scale.rect(v.x, v.y, v.width, v.height), [0, 0, 60, 45]);
    }
}
