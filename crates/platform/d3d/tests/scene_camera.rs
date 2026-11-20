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

fn packet() -> ResolvedDraw {
    let (mut d, mut mem) = create(RecordingBackend::new(), UploadPolicy::Always);
    set_states(&mut d, &mut mem);
    quad_vb(&mut mem, 0.0, 0.0, 18.0, 18.0, 0xFF00_4400);
    bind_quad(&mut d, &mut mem);
    d.draw_vertices(&mut mem, TRIANGLESTRIP, 0, 4).unwrap();
    (*d.backend.draws()[0]).clone()
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
            draw: Box::new(draw),
            textures: Default::default(),
            camera: CameraUse::Unknown,
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
    replay.classify_cameras(&views);
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
    replay.classify_cameras(&scene(&[(1, cam), (1, Camera { fov: 0.7, ..cam })]));
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

    frame.classify_cameras(&scene(&[(1, camera(0))]));
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
