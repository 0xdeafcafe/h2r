//! The interpolated replay (docs/game/high-framerate.md, "Phase 2") on
//! synthetic draw lists through the recording backend: what in-between
//! frames draw at alpha 0, 0.5 and 1, how draws pair up across frames with
//! spawns, despawns and instances, what stays untouched (HUD), the snaps,
//! and that the guest's own calls reach the backend exactly as without it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use d3d::backend::{Recorded, StreamBytes, TargetInfo, Viewport};
use d3d::interp::camera::{Persp, build};
use d3d::interp::math::{Mat4, Similarity, max_abs, max_diff, mul};
use d3d::interp::model::{Layout, read4, write};
use d3d::interp::reproject::overscan_scale;
use d3d::interp::{InterpBackend, Mode};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, RecordingBackend, ResolvedDraw};
use gfx::vertex::{PrimitiveType, VertexLayout};
use gfx::xdk::RenderStateTable;
use gfx::{Clear, D3DFormat};
use shaders::vsh::encode::{Inst, Operand as O};
use shaders::vsh::{IluOp, MacOp, OutReg};

const F: u64 = 1_000_000_000 / 30;
/// Where the camera goes (c0, hardware order 96) and the node (c4).
const CAM: usize = 96;
const NODE: usize = 100;
/// The eye position (c12).
const EYE: usize = 108;

type Block = [[f32; 4]; CONSTANTS];

fn draw(c: Block, mesh: u32, pretransformed: bool) -> ResolvedDraw {
    let words = StateWords::defaults(false);
    ResolvedDraw {
        entry: "interp",
        primitive: PrimitiveType::TriangleList,
        vertex_count: mesh,
        layout: VertexLayout {
            streams: Vec::new(),
            pretransformed,
        },
        streams: vec![StreamBytes {
            data: vec![0u8; 48].into(),
            stride: 16,
        }],
        indices: None,
        state: words.translate(&RenderStateTable::default()),
        extras: words.stage_extras(),
        target: TargetInfo {
            width: 640,
            height: 480,
            format: D3DFormat::A8R8G8B8,
            depth_format: Some(D3DFormat::D24S8),
        },
        texture_info: [None; 4],
        shader: ShaderInputs {
            vertex: VertexProcessing::Program {
                start: 0,
                microcode: program_words().into(),
            },
            provided_inputs: 1,
            vertex_defaults: [[0.0, 0.0, 0.0, 1.0]; 16],
            constants: Arc::new(c),
            pixel_shader: None,
            combiner_state: Arc::new([0; 57]),
            final_combiner_latch: false,
            affine_projection: false,
        },
        raw: Arc::new(words),
        viewport: Viewport {
            x: 0,
            y: 0,
            width: 640,
            height: 480,
            min_z: 0.0,
            max_z: 1.0,
        },
        textures: [None; 4],
        retail_constants: None,
        ui_fit: None,
        glyph_cache: 0,
        clip: None,
        clip_rects: Vec::new(),
        hw: d3d::hw::HwState::default(),
    }
}

#[test]
fn reference_uses_retail_lens_and_scissors_while_display_uses_its_own() {
    let retail = world_block(&camera(3.0, 0.2));
    let mut display = draw(retail, 3, false);
    let guest = display.shader.constants.clone();
    let factor = 0.5;
    assert!(d3d::device::widen_projection_rows(
        &mut Arc::make_mut(&mut display.shader.constants)[CAM..CAM+4], factor));
    display.retail_constants = Some((guest.clone(), factor));
    let scissor = d3d::PixelRect { x: 48, y: 20, width: 128, height: 44 };
    let first = display.for_copy(d3d::TargetCopy::Reference);
    assert!(Arc::ptr_eq(&first.shader.constants, &guest));
    assert_eq!(first.display_rect(scissor), scissor);
    let second = display.for_copy(d3d::TargetCopy::Display);
    assert_eq!(second.shader.constants[CAM], guest[CAM].map(|v| v * factor));
    assert_eq!(second.display_rect(scissor), d3d::PixelRect { x: 184, width: 64, ..scissor });
    assert_eq!(second.display_rect(d3d::PixelRect { x: 0, width: 640, ..scissor }).width, 640);

    let mut menu = draw(block(), 4, true);
    menu.ui_fit = d3d::backend::UiFit::about((1920,1080), [320.,240.]);
    assert_eq!(menu.for_reference().display_rect(scissor), scissor);
    assert_eq!(menu.display_rect(scissor), d3d::PixelRect { x: 116, width: 96, ..scissor });

    let mut probe = draw(block(), 4, true);
    probe.hw.zpass_count_enable = true;
    probe.ui_fit = Some(d3d::backend::UiFit::projected_sprite(1.0, -64.0));
    assert!(probe.for_copy(d3d::TargetCopy::Reference).ui_fit.is_none());
    let shown = probe.for_copy(d3d::TargetCopy::Display);
    assert_eq!(shown.ui_fit, probe.ui_fit);
    assert_eq!(shown.display_rect(scissor), scissor);

}

/// A camera at `x` along the world's x axis, turned `yaw`.
fn camera(x: f64, yaw: f64) -> Mat4 {
    build::view_projection(1.2, [x, 0.0, 1.0], yaw, 0.0)
}

fn eye4(cam: &Mat4) -> [f32; 4] {
    let e = Persp::split(cam).unwrap().eye();
    [e[0] as f32, e[1] as f32, e[2] as f32, 1.0]
}

/// The constants of a draw in the world: the camera at c0, the eye at c12.
fn world_block(cam: &Mat4) -> Block {
    let mut c = block();
    write(&mut c, CAM, Layout::Rows, 4, cam);
    c[EYE] = eye4(cam);
    c
}

fn node(x: f64, turn: f64) -> Similarity {
    Similarity {
        rotation: build::axis([0.0, 0.0, 1.0], turn),
        scale: 1.0,
        translation: [x, 5.0, 0.0],
    }
}

fn block() -> Block {
    let mut c = [[0.0f32; 4]; CONSTANTS];
    // The program's own viewport constants: the whole 640x480 target.
    c[SCALE] = [320.0, -240.0, 1.0, 0.0];
    c[OFFSET] = [320.0, 240.0, 0.0, 0.0];
    // Some lighting constants that are no matrix.
    c[110] = [0.3, 0.6, 0.9, 1.0];
    c[111] = [0.5, 0.5, 0.5, 0.0];
    c
}

/// The program's viewport scale and offset constants (hardware numbering),
/// as Halo 2's programs keep them: not D3D's c58/c59.
const SCALE: usize = 10;
const OFFSET: usize = 11;
/// The constants a screen-space lookup dots the clip position with.
const LOOKUP: usize = 8;

/// The test draws' vertex program, Halo 2's shape: the clip position from
/// the camera's rows at c96..c99, a screen-space lookup from it (`oT0.xy`
/// from c8 and c9, `oT0.w` the clip w), then the program's own viewport
/// transform (`mul oPos.xyz, r12, c10 : rcc r1.w, r12.w`, `mad oPos.xyz,
/// r12, r1.w, c11`).
fn program_words() -> Vec<u32> {
    let (x, y, z, w) = (8, 4, 2, 1);
    let mut insts: Vec<Inst> = (0..4)
        .map(|i| {
            Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(CAM as u8 + i)), None).to_out(
                false,
                OutReg::Pos,
                8 >> i,
            )
        })
        .collect();
    insts.extend([
        Inst::mac(MacOp::Dp4, Some(O::r(12)), Some(O::c(LOOKUP as u8)), None).to_out(
            false,
            OutReg::T(0),
            x,
        ),
        Inst::mac(
            MacOp::Dp4,
            Some(O::r(12)),
            Some(O::c(LOOKUP as u8 + 1)),
            None,
        )
        .to_out(false, OutReg::T(0), y),
        Inst::mac(MacOp::Mov, Some(O::r(12).swz("w")), None, None).to_out(false, OutReg::T(0), w),
        Inst::mac(MacOp::Mul, Some(O::r(12)), Some(O::c(SCALE as u8)), None)
            .to_out(false, OutReg::Pos, x | y | z)
            .with_ilu(IluOp::Rcc, O::r(12).swz("w"))
            .ilu_to_temp(1, w),
        Inst::mac(
            MacOp::Mad,
            Some(O::r(12)),
            Some(O::r(1).swz("w")),
            Some(O::c(OFFSET as u8)),
        )
        .to_out(false, OutReg::Pos, x | y | z)
        .last(),
    ]);
    insts.iter().flat_map(|i| i.words()).collect()
}

/// What a scene holds in one frame.
#[derive(Clone)]
struct Scene {
    cam: Mat4,
    /// Skinned-style objects: (mesh, node) drawn with the camera at c0 and
    /// the node at c4.
    nodes: Vec<(u32, Similarity)>,
    /// Objects drawn with world-view-projection at c0.
    worlds: Vec<(u32, Similarity)>,
    /// A first-person weapon: its own projection at c0.
    weapon: Option<Mat4>,
}

fn scene(cam: Mat4) -> Scene {
    Scene {
        cam,
        nodes: Vec::new(),
        worlds: Vec::new(),
        weapon: None,
    }
}

fn hud_block() -> Block {
    let mut c = block();
    // An orthographic screen transform: not perspective.
    let ortho: Mat4 = [
        [2.0 / 640.0, 0.0, 0.0, -1.0],
        [0.0, -2.0 / 480.0, 0.0, 1.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    write(&mut c, CAM, Layout::Rows, 4, &ortho);
    c
}

/// Issues one frame of `s` into `color`/`depth` and presents it.
fn frame<B: Backend>(b: &mut B, color: B::Surface, depth: B::Surface, s: &Scene) {
    b.clear(color, Some(depth), Clear::default()).unwrap();
    let world = world_block(&s.cam);
    // Level geometry: three draws sharing the camera.
    for mesh in [1, 2, 3] {
        b.draw(color, Some(depth), &draw(world, mesh, false), [None; 4])
            .unwrap();
    }
    for &(mesh, n) in &s.nodes {
        let mut c = world;
        write(&mut c, NODE, Layout::Rows, 3, &n.to_mat4());
        b.draw(color, Some(depth), &draw(c, mesh, false), [None; 4])
            .unwrap();
    }
    for &(mesh, w) in &s.worlds {
        let mut c = block();
        write(&mut c, CAM, Layout::Columns, 4, &mul(&s.cam, &w.to_mat4()));
        b.draw(color, Some(depth), &draw(c, mesh, false), [None; 4])
            .unwrap();
    }
    if let Some(m) = s.weapon {
        let mut c = block();
        write(&mut c, CAM, Layout::Rows, 4, &m);
        b.draw(color, Some(depth), &draw(c, 900, false), [None; 4])
            .unwrap();
    }
    b.draw(color, None, &draw(hud_block(), 1000, true), [None; 4])
        .unwrap();
    b.present(color, 4).unwrap();
}

struct Rig {
    b: InterpBackend<RecordingBackend>,
    clock: Arc<AtomicU64>,
    fps: Arc<AtomicU32>,
    color: u32,
    depth: u32,
}

fn rig(fps: u32) -> Rig {
    rig_with(fps, Mode::Replay)
}

fn rig_with(fps: u32, mode: Mode) -> Rig {
    let clock = Arc::new(AtomicU64::new(0));
    let rate = Arc::new(AtomicU32::new(fps));
    let (c, r) = (Arc::clone(&clock), Arc::clone(&rate));
    let mut b = InterpBackend::new(RecordingBackend::new(), move || r.load(Ordering::Relaxed))
        .with_clock(move || c.load(Ordering::Relaxed))
        .with_mode(mode);
    let color = b
        .create_surface(D3DFormat::A8R8G8B8, 640, 480, false)
        .unwrap();
    let depth = b.create_surface(D3DFormat::D24S8, 640, 480, true).unwrap();
    // The swap chain is learnt from the first present.
    b.present(color, 4).unwrap();
    Rig {
        b,
        clock,
        fps: rate,
        color,
        depth,
    }
}

impl Rig {
    fn at(&self, ns: u64) {
        self.clock.store(ns, Ordering::Relaxed);
    }
    fn frame(&mut self, ns: u64, s: &Scene) {
        self.at(ns);
        frame(&mut self.b, self.color, self.depth, s);
    }
    fn calls(&self) -> usize {
        self.b.inner().calls.len()
    }
    /// The draws made since call `from`, with their targets.
    fn draws_since(&self, from: usize) -> Vec<(u32, ResolvedDraw)> {
        self.b.inner().calls[from..]
            .iter()
            .filter_map(|c| match c {
                Recorded::Draw { color, draw, .. } => Some((*color, (**draw).clone())),
                _ => None,
            })
            .collect()
    }
    fn presents_since(&self, from: usize) -> Vec<(u32, u32)> {
        self.b.inner().calls[from..]
            .iter()
            .filter_map(|c| match c {
                Recorded::Present { surface, flags } => Some((*surface, *flags)),
                _ => None,
            })
            .collect()
    }
}

fn close(a: &Mat4, b: &Mat4) -> bool {
    max_diff(a, b) <= 1e-4 * max_abs(b).max(1.0)
}

fn mat(d: &ResolvedDraw, reg: usize, layout: Layout) -> Mat4 {
    read4(&d.shader.constants, reg, layout)
}

fn node_of(d: &ResolvedDraw) -> Mat4 {
    let mut m = read4(&d.shader.constants, NODE, Layout::Rows);
    m[3] = [0.0, 0.0, 0.0, 1.0];
    m
}

#[test]
fn in_between_frames_at_zero_half_and_one() {
    let mut r = rig(60);
    let mut a = scene(camera(0.0, 0.0));
    a.nodes.push((50, node(0.0, 0.0)));
    let mut b = scene(camera(1.0, 0.4));
    b.nodes.push((50, node(2.0, 0.6)));
    r.frame(F, &a);
    let before = r.calls();
    r.frame(2 * F, &b);
    // At the present (alpha 0): frame b's draws with frame a's matrices,
    // into the private surface.
    let draws = r.draws_since(before);
    let presents = r.presents_since(before);
    assert_eq!(presents.len(), 1);
    let shown = presents[0].0;
    assert_ne!(
        shown, r.color,
        "the guest's surface is not shown before alpha 1"
    );
    assert_eq!(presents[0].1, 4, "the guest's flags go with its present");
    let replayed: Vec<_> = draws.iter().filter(|(c, _)| *c == shown).collect();
    assert_eq!(replayed.len(), 5, "three level draws, one object, the HUD");
    for (_, d) in &replayed[..3] {
        assert!(close(&mat(d, CAM, Layout::Rows), &a.cam));
    }
    assert!(close(&node_of(&replayed[3].1), &a.nodes[0].1.to_mat4()));

    // Half way.
    let before = r.calls();
    r.at(2 * F + F / 2);
    r.b.idle_at(2 * F + F / 2);
    let draws = r.draws_since(before);
    assert_eq!(
        r.presents_since(before),
        vec![(shown, 0)],
        "an in-between frame"
    );
    let half_cam = Persp::split(&a.cam)
        .unwrap()
        .lerp_camera(&Persp::split(&b.cam).unwrap(), 0.5);
    assert!(close(
        &mat(&draws[0].1, CAM, Layout::Rows),
        &half_cam.matrix()
    ));
    let half_node = a.nodes[0].1.lerp(&b.nodes[0].1, 0.5);
    assert!(close(&node_of(&draws[3].1), &half_node.to_mat4()));
    assert!(
        (node_of(&draws[3].1)[0][3] - 1.0).abs() < 1e-4,
        "x half way from 0 to 2"
    );
    // The eye register follows the camera: half way from x 0 to 1.
    let eye = draws[0].1.shader.constants[EYE];
    assert!(
        (eye[0] - 0.5).abs() < 1e-4 && (eye[2] - 1.0).abs() < 1e-4 && eye[3] == 1.0,
        "{eye:?}"
    );

    // At alpha 1, frame b exactly, bit for bit.
    // Right before the next frame is due the slot waits for it (alpha ~1
    // would duplicate the next frame's alpha 0).
    let before = r.calls();
    assert!(r.b.idle_at(3 * F).is_some_and(|t| t > 3 * F));
    assert_eq!(r.calls(), before);
    // The frame is late: alpha 1.
    r.b.idle_at(3 * F + F / 4);
    let draws = r.draws_since(before);
    assert_eq!(draws.len(), 5);
    assert_eq!(*draws[0].1.shader.constants, world_block(&b.cam));
    // And nothing more until the next guest frame.
    assert_eq!(r.b.idle_at(3 * F + F / 2), None, "held at 1");
    assert_eq!(r.b.stats().lock().unwrap().replays, 2);
}

#[test]
fn spawns_despawns_and_instances_pair_up() {
    let mut r = rig(60);
    let cam = camera(0.0, 0.0);
    let mut a = scene(cam);
    // Two instances of mesh 60 and one of mesh 61 (despawns).
    a.nodes = vec![
        (60, node(0.0, 0.0)),
        (60, node(10.0, 0.0)),
        (61, node(-5.0, 0.0)),
    ];
    let mut b = scene(cam);
    // The instances come in the other order, each moved a little; 61 is
    // gone and 62 is new.
    b.nodes = vec![
        (60, node(11.0, 0.0)),
        (60, node(1.0, 0.0)),
        (62, node(3.0, 0.0)),
    ];
    r.frame(F, &a);
    r.frame(2 * F, &b);
    let before = r.calls();
    r.b.idle_at(2 * F + F / 2);
    let d = r.draws_since(before);
    // Level draws 0..3, then the three objects.
    let x = |i: usize| node_of(&d[i].1)[0][3];
    assert!(
        (x(3) - 10.5).abs() < 1e-4,
        "the instance at 10 went to 11: {}",
        x(3)
    );
    assert!(
        (x(4) - 0.5).abs() < 1e-4,
        "the instance at 0 went to 1: {}",
        x(4)
    );
    assert_eq!(x(5), 3.0, "a new object keeps its own matrix");
    assert_eq!(*d[5].1.shader.constants, {
        let mut c = world_block(&cam);
        write(&mut c, NODE, Layout::Rows, 3, &node(3.0, 0.0).to_mat4());
        c
    });
}

#[test]
fn the_hud_replays_untouched() {
    let mut r = rig(60);
    r.frame(F, &scene(camera(0.0, 0.0)));
    r.frame(2 * F, &scene(camera(1.0, 0.3)));
    for t in [2 * F, 2 * F + F / 2] {
        let before = r.calls();
        r.b.idle_at(t);
        for (_, d) in r.draws_since(before) {
            if d.layout.pretransformed {
                assert_eq!(*d.shader.constants, hud_block());
            }
        }
    }
}

#[test]
fn objects_drawn_with_world_view_projection_blend_in_the_world() {
    let mut r = rig(60);
    let mut a = scene(camera(0.0, 0.0));
    a.worlds.push((70, node(0.0, 0.0)));
    let mut b = scene(camera(1.0, 0.2));
    b.worlds.push((70, node(1.0, 0.4)));
    r.frame(F, &a);
    r.frame(2 * F, &b);
    let before = r.calls();
    r.b.idle_at(2 * F + F / 2);
    let d = r.draws_since(before);
    let half_cam = Persp::split(&a.cam)
        .unwrap()
        .lerp_camera(&Persp::split(&b.cam).unwrap(), 0.5);
    let half_world = a.worlds[0].1.lerp(&b.worlds[0].1, 0.5);
    let want = mul(&half_cam.matrix(), &half_world.to_mat4());
    assert!(close(&mat(&d[3].1, CAM, Layout::Columns), &want));
}

#[test]
fn a_weapon_with_its_own_projection_blends_in_camera_space() {
    let mut r = rig(60);
    let fp = build::projection(1.0, 16.0 / 9.0, 0.01, 10.0);
    let pose = |x: f64| {
        mul(
            &fp,
            &Similarity {
                rotation: build::axis([0.0, 1.0, 0.0], x),
                scale: 1.0,
                translation: [0.1, -0.1 + x, -0.3],
            }
            .to_mat4(),
        )
    };
    let mut a = scene(camera(0.0, 0.0));
    a.weapon = Some(pose(0.0));
    let mut b = scene(camera(1.0, 0.5));
    b.weapon = Some(pose(0.02));
    r.frame(F, &a);
    r.frame(2 * F, &b);
    let before = r.calls();
    r.b.idle_at(2 * F + F / 2);
    let d = r.draws_since(before);
    assert!(close(&mat(&d[3].1, CAM, Layout::Rows), &pose(0.01)));
}

#[test]
fn cuts_and_teleports_snap() {
    let mut r = rig(60);
    let mut a = scene(camera(0.0, 0.0));
    a.nodes.push((80, node(0.0, 0.0)));
    // The camera zooms (a scope) and the object teleports.
    let mut b = scene(build::view_projection(0.4, [0.5, 0.0, 1.0], 0.1, 0.0));
    b.nodes.push((80, node(40.0, 0.0)));
    r.frame(F, &a);
    r.frame(2 * F, &b);
    let before = r.calls();
    r.b.idle_at(2 * F + F / 2);
    let d = r.draws_since(before);
    assert!(
        d.iter()
            .take(4)
            .all(|(_, x)| close(&mat(x, CAM, Layout::Rows), &b.cam))
    );
    assert_eq!(node_of(&d[3].1)[0][3], 40.0);
    assert!(r.b.stats().lock().unwrap().snapped >= 2);
}

#[test]
fn a_gap_shows_the_frame_as_drawn() {
    let mut r = rig(60);
    r.frame(F, &scene(camera(0.0, 0.0)));
    let before = r.calls();
    // 200 ms later: a load, a pause, a pacer rebase.
    r.frame(F + 200_000_000, &scene(camera(1.0, 0.0)));
    assert_eq!(r.presents_since(before), vec![(r.color, 4)]);
    assert_eq!(r.b.idle_at(F + 210_000_000), None);
}

#[test]
fn no_replay_while_a_pixel_count_is_open() {
    let mut r = rig(60);
    r.frame(F, &scene(camera(0.0, 0.0)));
    r.frame(2 * F, &scene(camera(1.0, 0.0)));
    r.b.clear_pixel_count().unwrap();
    let before = r.calls();
    assert_eq!(r.b.idle_at(2 * F + F / 2), None, "wait for the count report");
    assert_eq!(r.calls(), before, "nothing drawn into the open count");
    r.b.report_pixel_count().unwrap();
    r.b.idle_at(2 * F + F / 2);
    assert_eq!(r.presents_since(before).len(), 1);
}

/// The guest's calls to the backend, without the replay's: draws, clears
/// and presents into the guest's surfaces (presents of a private surface
/// stand for the guest's).
fn guest_calls(calls: &[Recorded], guest: &[u32]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|c| match c {
            Recorded::Draw { color, draw, .. } if guest.contains(color) => {
                Some(format!("draw {color} {:?}", draw.shader.constants[CAM]))
            }
            Recorded::Clear { color, .. } if guest.contains(color) => {
                Some(format!("clear {color}"))
            }
            Recorded::Present { flags, .. } if *flags != 0 => Some(format!("present {flags}")),
            Recorded::Surface { id, .. } if guest.contains(id) => Some(format!("surface {id}")),
            _ => None,
        })
        .collect()
}

#[test]
fn the_guests_calls_are_the_same_with_and_without_it() {
    let scenes: Vec<Scene> = (0..6)
        .map(|i| {
            let mut s = scene(camera(i as f64 * 0.3, i as f64 * 0.1));
            s.nodes.push((90, node(i as f64, 0.0)));
            s
        })
        .collect();
    let mut plain = RecordingBackend::new();
    let color = plain
        .create_surface(D3DFormat::A8R8G8B8, 640, 480, false)
        .unwrap();
    let depth = plain
        .create_surface(D3DFormat::D24S8, 640, 480, true)
        .unwrap();
    plain.present(color, 4).unwrap();
    for s in &scenes {
        frame(&mut plain, color, depth, s);
    }
    for fps in [30, 60, 120, 0] {
        let mut r = rig(fps);
        for (i, s) in scenes.iter().enumerate() {
            r.frame((i as u64 + 1) * F, s);
            r.b.idle_at((i as u64 + 1) * F + F / 2);
        }
        let guest = [r.color, r.depth];
        assert_eq!(
            guest_calls(&r.b.inner().calls, &guest),
            guest_calls(&plain.calls, &guest),
            "fps {fps}"
        );
        // Every other call is the replay's, into surfaces of its own.
        for c in &r.b.inner().calls {
            if let Recorded::Draw { color, .. } | Recorded::Clear { color, .. } = c {
                assert!(guest.contains(color) || fps != 30);
            }
            assert!(!matches!(
                c,
                Recorded::ReadSurface { .. } | Recorded::ReadDepth { .. }
            ));
        }
        if fps == 30 {
            assert_eq!(r.b.inner().calls, plain.calls, "30 is the plain path");
        }
    }
}

#[test]
fn turning_it_off_mid_run_goes_back_to_plain_presents() {
    let mut r = rig(60);
    r.frame(F, &scene(camera(0.0, 0.0)));
    r.frame(2 * F, &scene(camera(1.0, 0.0)));
    r.fps.store(30, Ordering::Relaxed);
    let before = r.calls();
    r.frame(3 * F, &scene(camera(2.0, 0.0)));
    assert_eq!(r.presents_since(before), vec![(r.color, 4)]);
    assert_eq!(r.b.idle_at(3 * F + F / 2), None, "held at 1");
}

#[test]
fn the_render_thread_runs_in_between_frames_when_idle() {
    let rate = || 60;
    let inner = InterpBackend::new(RecordingBackend::new(), rate);
    let mut t = d3d::ThreadedBackend::new(inner).unwrap();
    let color = t
        .create_surface(D3DFormat::A8R8G8B8, 640, 480, false)
        .unwrap();
    let depth = t.create_surface(D3DFormat::D24S8, 640, 480, true).unwrap();
    t.present(color, 4).unwrap();
    let mut s = scene(camera(0.0, 0.0));
    for i in 0..3 {
        s.cam = camera(i as f64 * 0.1, 0.0);
        frame(&mut t, color, depth, &s);
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    // Give the render thread time for an idle step.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let replays = t.with_inner(|b| b.stats().lock().unwrap().replays).unwrap();
        if replays > 0 {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "no in-between frame in 2 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let presents = t
        .with_inner(|b| {
            b.inner()
                .calls
                .iter()
                .filter(|c| matches!(c, Recorded::Present { flags: 0, .. }))
                .count()
        })
        .unwrap();
    assert!(presents > 0);
}

/// A first-person weapon: its own projection, a pose in camera space.
fn weapon() -> Mat4 {
    let fp = build::projection(1.0, 16.0 / 9.0, 0.01, 10.0);
    mul(
        &fp,
        &Similarity {
            rotation: build::axis([0.0, 1.0, 0.0], 0.0),
            scale: 1.0,
            translation: [0.1, -0.1, -0.3],
        }
        .to_mat4(),
    )
}

/// The calls since `from` as a compact trace: `draw <target> mesh <n>`,
/// `warp <src> <dst>`, `clear <target>`, `present <surface> <flags>`.
fn trace(r: &Rig, from: usize) -> Vec<String> {
    r.b.inner().calls[from..]
        .iter()
        .filter_map(|c| match c {
            Recorded::Draw { color, draw, .. } => {
                Some(format!("draw {color} mesh {}", draw.vertex_count))
            }
            Recorded::Warp { src, dst, .. } => Some(format!("warp {src} {dst}")),
            Recorded::Clear { color, .. } => Some(format!("clear {color}")),
            Recorded::Present { surface, flags } => Some(format!("present {surface} {flags}")),
            _ => None,
        })
        .collect()
}

fn warps_since(r: &Rig, from: usize) -> Vec<[[f32; 3]; 3]> {
    r.b.inner().calls[from..]
        .iter()
        .filter_map(|c| match c {
            Recorded::Warp { h, .. } => Some(*h),
            _ => None,
        })
        .collect()
}

/// The warp of a world image drawn with overscan and shown from the same
/// rotation: a zoom about the centre by the overscan scale.
fn zoom() -> [[f32; 3]; 3] {
    let s = overscan_scale() as f32;
    [
        [s, 0.0, 0.5 * (1.0 - s)],
        [0.0, s, 0.5 * (1.0 - s)],
        [0.0, 0.0, 1.0],
    ]
}

fn near(a: &[[f32; 3]; 3], b: &[[f32; 3]; 3]) -> bool {
    a.iter()
        .flatten()
        .zip(b.iter().flatten())
        .all(|(x, y)| (x - y).abs() < 1e-4)
}

/// Two frames turning in place (rotation only), each with a weapon and the
/// HUD.
fn turning() -> (Scene, Scene) {
    let mut a = scene(camera(0.0, 0.0));
    a.weapon = Some(weapon());
    let mut b = scene(camera(0.0, 0.2));
    b.weapon = Some(weapon());
    (a, b)
}

#[test]
fn reproject_draws_the_world_once_and_warps_it_with_the_overlay_on_top() {
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (a, b) = turning();
    r.frame(F, &a);
    let before = r.calls();
    r.frame(2 * F, &b);
    let t = trace(&r, before);
    let g = r.color;
    // The guest's own calls first, as it made them.
    assert_eq!(
        t[..6],
        [
            format!("clear {g}"),
            format!("draw {g} mesh 1"),
            format!("draw {g} mesh 2"),
            format!("draw {g} mesh 3"),
            format!("draw {g} mesh 900"),
            format!("draw {g} mesh 1000"),
        ],
        "{t:#?}"
    );
    // Then the world image (the clear and the level, no weapon or HUD)
    // into a surface of its own, the warp to the shown surface, a depth
    // clear, the overlay and the present.
    let w = t[6].trim_start_matches("clear ").to_string();
    let shown = r.presents_since(before)[0].0;
    assert_ne!(w, g.to_string());
    assert_ne!(shown, g);
    assert_ne!(w, shown.to_string());
    assert_eq!(
        t[6..],
        [
            format!("clear {w}"),
            format!("draw {w} mesh 1"),
            format!("draw {w} mesh 2"),
            format!("draw {w} mesh 3"),
            format!("warp {w} {shown}"),
            format!("clear {shown}"),
            format!("draw {shown} mesh 900"),
            format!("draw {shown} mesh 1000"),
            format!("present {shown} 4"),
        ],
        "{t:#?}"
    );
    // The world image is drawn with overscan through the program's own
    // viewport scale (x and y times the overscan, about the offset, the
    // screen's centre): the camera, and so the clip position everything
    // else in the program reads, is as the guest drew it.
    let s = overscan_scale() as f32;
    let d = r.draws_since(before);
    let world: Vec<_> = d.iter().filter(|(c, _)| c.to_string() == w).collect();
    let guest: Vec<_> = d.iter().filter(|(c, _)| *c == g).collect();
    let (wc, gc) = (&world[0].1.shader.constants, &guest[0].1.shader.constants);
    assert_eq!(wc[CAM..CAM + 4], gc[CAM..CAM + 4]);
    assert_eq!(
        wc[SCALE],
        [
            gc[SCALE][0] * s,
            gc[SCALE][1] * s,
            gc[SCALE][2],
            gc[SCALE][3]
        ]
    );
    assert_eq!(wc[OFFSET], gc[OFFSET]);
    // The overlay draws as the guest drew it.
    let overlay: Vec<_> = d.iter().filter(|(c, _)| *c == shown).collect();
    assert_eq!(*overlay[0].1.shader.constants, {
        let mut c = block();
        write(&mut c, CAM, Layout::Rows, 4, &weapon());
        c
    });
    assert_eq!(*overlay[1].1.shader.constants, hud_block());
    // The depth clear leaves the colour alone.
    let cleared = r.b.inner().calls[before..].iter().find_map(|c| match c {
        Recorded::Clear { color, clear } if *color == shown => Some(*clear),
        _ => None,
    });
    assert_eq!(
        cleared,
        Some(Clear {
            color: None,
            depth: Some(1.0),
            stencil: Some(0)
        })
    );
    // At the present (alpha 0, blended) the warp turns back to frame a's
    // rotation: not the plain zoom.
    let h0 = warps_since(&r, before)[0];
    assert!(!near(&h0, &zoom()), "{h0:?}");

    // Half way: a warp and the overlay, no world draws.
    let before = r.calls();
    r.at(2 * F + F / 2);
    r.b.idle_at(2 * F + F / 2);
    assert_eq!(
        trace(&r, before),
        [
            format!("warp {w} {shown}"),
            format!("clear {shown}"),
            format!("draw {shown} mesh 900"),
            format!("draw {shown} mesh 1000"),
            format!("present {shown} 0"),
        ]
    );
    let h_half = warps_since(&r, before)[0];
    assert!(!near(&h_half, &h0) && !near(&h_half, &zoom()));

    // Late, alpha 1: frame b's own rotation, the plain zoom.
    let before = r.calls();
    r.b.idle_at(3 * F + F / 4);
    assert!(near(&warps_since(&r, before)[0], &zoom()));
    assert_eq!(r.b.idle_at(3 * F + F / 2), None, "held at 1");

    let st = *r.b.stats().lock().unwrap();
    assert_eq!(
        (st.captures, st.warps, st.replay_frames, st.replays),
        (1, 3, 0, 2)
    );
    let line = st.to_string();
    assert!(line.contains("reproject mode: 1 world images"), "{line}");
    assert!(!line.contains("replay mode"), "{line}");
}

/// How far from the unturned sample the warp `h` samples for a point off
/// the centre (the test camera's turn rolls the image about it).
fn shift(h: &[[f32; 3]; 3]) -> [f32; 2] {
    let at = |h: &[[f32; 3]; 3], r: usize| {
        let q = |r: usize| h[r][0] * 0.9 + h[r][1] * 0.5 + h[r][2];
        q(r) / q(2)
    };
    let z = zoom();
    [at(h, 0) - at(&z, 0), at(h, 1) - at(&z, 1)]
}

#[test]
fn reproject_ahead_runs_the_turn_on_past_the_newest_frame() {
    let (a, b) = turning();
    let mut r = rig_with(60, Mode::Reproject { ahead: true });
    r.frame(F, &a);
    let before = r.calls();
    r.frame(2 * F, &b);
    // At the present the newest rotation: the plain zoom.
    assert!(near(&warps_since(&r, before)[0], &zoom()));
    // Half a frame on, past it.
    let before = r.calls();
    r.b.idle_at(2 * F + F / 2);
    let ahead = warps_since(&r, before)[0];
    assert!(!near(&ahead, &zoom()));
    // Blending samples the other way at its present (back towards frame
    // a, a whole frame behind; ahead is half a frame on), so the shifts
    // point opposite ways, the ahead one about half as long.
    let mut blended = rig_with(60, Mode::Reproject { ahead: false });
    blended.frame(F, &a);
    let from = blended.calls();
    blended.frame(2 * F, &b);
    let behind = warps_since(&blended, from)[0];
    let (x, y) = (shift(&ahead), shift(&behind));
    let dot = x[0] * y[0] + x[1] * y[1];
    let len = |v: [f32; 2]| (v[0] * v[0] + v[1] * v[1]).sqrt();
    assert!(dot < 0.0, "ahead {x:?} behind {y:?}");
    assert!(
        (len(x) / len(y) - 0.5).abs() < 0.05,
        "ahead {x:?} behind {y:?}"
    );
}

#[test]
fn reproject_holds_split_screen_and_the_replay_mode_reports_its_own_cost() {
    // Two views: reprojection is for one gameplay view, so frames are
    // shown as drawn.
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let two = |cam: Mat4, r: &mut Rig| {
        let (color, depth) = (r.color, r.depth);
        r.b.clear(color, Some(depth), Clear::default()).unwrap();
        for (i, cam) in [cam, camera(5.0, 1.0)].into_iter().enumerate() {
            let mut d = draw(world_block(&cam), 1 + i as u32, false);
            d.viewport.y = 240 * i as u32;
            d.viewport.height = 240;
            r.b.draw(color, Some(depth), &d, [None; 4]).unwrap();
        }
        r.b.present(color, 4).unwrap();
    };
    r.at(F);
    two(camera(0.0, 0.0), &mut r);
    let before = r.calls();
    r.at(2 * F);
    two(camera(0.0, 0.2), &mut r);
    assert_eq!(r.presents_since(before), vec![(r.color, 4)]);
    assert_eq!(r.b.idle_at(2 * F + F / 2), None);
    let st = *r.b.stats().lock().unwrap();
    assert_eq!((st.captures, st.warps), (0, 0));

    // Design D's readout carries its frame count and cost.
    let mut r = rig(60);
    let (a, b) = turning();
    r.frame(F, &a);
    r.frame(2 * F, &b);
    r.b.idle_at(2 * F + F / 2);
    let st = *r.b.stats().lock().unwrap();
    assert_eq!((st.replay_frames, st.captures, st.warps), (2, 0, 0));
    assert!(st.replay_max_ns > 0 && st.replay_ns >= st.replay_max_ns);
    let line = st.to_string();
    assert!(line.contains("replay mode: 2 frames"), "{line}");
    assert!(!line.contains("reproject"), "{line}");
}

#[test]
fn reproject_leaves_the_guests_calls_alone() {
    let scenes: Vec<Scene> = (0..6)
        .map(|i| {
            let mut s = scene(camera(0.0, i as f64 * 0.1));
            s.weapon = Some(weapon());
            s
        })
        .collect();
    let mut plain = RecordingBackend::new();
    let color = plain
        .create_surface(D3DFormat::A8R8G8B8, 640, 480, false)
        .unwrap();
    let depth = plain
        .create_surface(D3DFormat::D24S8, 640, 480, true)
        .unwrap();
    plain.present(color, 4).unwrap();
    for s in &scenes {
        frame(&mut plain, color, depth, s);
    }
    for ahead in [false, true] {
        let mut r = rig_with(60, Mode::Reproject { ahead });
        for (i, s) in scenes.iter().enumerate() {
            r.frame((i as u64 + 1) * F, s);
            r.b.idle_at((i as u64 + 1) * F + F / 2);
        }
        let guest = [r.color, r.depth];
        assert_eq!(
            guest_calls(&r.b.inner().calls, &guest),
            guest_calls(&plain.calls, &guest),
            "ahead {ahead}"
        );
        assert!(
            r.b.inner()
                .calls
                .iter()
                .all(|c| !matches!(c, Recorded::ReadSurface { .. } | Recorded::ReadDepth { .. }))
        );
        assert_eq!(r.b.stats().lock().unwrap().captures, 5);
    }
}

/// A perspective that maps straight to pixels (screen-space UI): its axis
/// half a screen off the middle, a focal length in pixels.
fn pixel_perspective() -> Block {
    let mut c = block();
    let ui: Mat4 = [
        [400.0, 0.0, 320.0, 0.0],
        [0.0, -400.0, 240.0, 0.0],
        [0.0, 0.0, 1.0, -0.1],
        [0.0, 0.0, 1.0, 0.0],
    ];
    write(&mut c, CAM, Layout::Rows, 4, &ui);
    c
}

/// A front-end frame: a pre-transformed background, menu panels drawn
/// with a shared screen-space perspective, and text with an orthographic
/// transform.
fn menu_frame(r: &mut Rig) {
    let (color, depth) = (r.color, r.depth);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    r.b.draw(color, Some(depth), &draw(block(), 7, true), [None; 4])
        .unwrap();
    for mesh in 10..16 {
        r.b.draw(
            color,
            Some(depth),
            &draw(pixel_perspective(), mesh, false),
            [None; 4],
        )
        .unwrap();
    }
    for mesh in [1000, 1001] {
        r.b.draw(color, None, &draw(hud_block(), mesh, false), [None; 4])
            .unwrap();
    }
    r.b.present(color, 4).unwrap();
}

fn modes() -> [Mode; 3] {
    [
        Mode::Replay,
        Mode::Reproject { ahead: false },
        Mode::Reproject { ahead: true },
    ]
}

#[test]
fn menu_frames_are_shown_as_the_guest_drew_them() {
    for mode in modes() {
        let mut r = rig_with(60, mode);
        let before = r.calls();
        for i in 1..=4 {
            r.at(i * F);
            menu_frame(&mut r);
            r.b.idle_at(i * F + F / 2);
        }
        assert_eq!(
            r.presents_since(before),
            vec![(r.color, 4); 4],
            "{mode:?}: only the guest's frames"
        );
        assert!(
            r.draws_since(before).iter().all(|(c, _)| *c == r.color),
            "{mode:?}: nothing drawn again"
        );
        assert!(warps_since(&r, before).is_empty(), "{mode:?}");
        let st = *r.b.stats().lock().unwrap();
        assert_eq!(
            (st.blended_frames, st.replays, st.captures, st.warps),
            (0, 0, 0, 0),
            "{mode:?}"
        );
        assert_eq!(st.unsure, 3, "{mode:?}: every pair but the first");
        assert!(st.to_string().contains("unsure 3"), "{st}");
    }
}

type Mapping<'a> = &'a dyn Fn(usize) -> ([f32; 2], [f32; 2]);

/// A frame of `s` whose world draws map to the target through `screen(i)`
/// (the program's viewport scale and offset constants) for level draw `i`.
fn frame_mapped(r: &mut Rig, s: &Scene, screen: Mapping<'_>) {
    let (color, depth) = (r.color, r.depth);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    for (i, mesh) in [1, 2, 3].into_iter().enumerate() {
        let mut c = world_block(&s.cam);
        let (scale, offset) = screen(i);
        c[SCALE] = [scale[0], scale[1], 1.0, 0.0];
        c[OFFSET] = [offset[0], offset[1], 0.0, 0.0];
        let d = draw(c, mesh, false);
        r.b.draw(color, Some(depth), &d, [None; 4]).unwrap();
    }
    r.b.draw(color, None, &draw(hud_block(), 1000, true), [None; 4])
        .unwrap();
    r.b.present(color, 4).unwrap();
}

#[test]
fn reproject_holds_world_draws_that_do_not_cover_the_target_the_same_way() {
    let full = ([320.0, -240.0], [320.5, 240.5]);
    let cases: [(&str, Mapping<'_>); 2] = [
        // Screen space: pixels out of the program, offset 0.
        ("screen space", &|_| ([1.0, 1.0], [0.0, 0.0])),
        // One world draw into the top-left quarter.
        ("mixed", &move |i| {
            if i == 2 {
                ([160.0, -120.0], [160.0, 120.0])
            } else {
                full
            }
        }),
    ];
    for (name, screen) in cases {
        let mut r = rig_with(60, Mode::Reproject { ahead: false });
        let (a, b) = turning();
        r.at(F);
        frame_mapped(&mut r, &a, screen);
        let before = r.calls();
        r.at(2 * F);
        frame_mapped(&mut r, &b, screen);
        assert_eq!(r.presents_since(before), vec![(r.color, 4)], "{name}");
        assert_eq!(r.b.idle_at(2 * F + F / 2), None, "{name}");
        assert!(warps_since(&r, before).is_empty(), "{name}");
        let st = *r.b.stats().lock().unwrap();
        assert_eq!((st.size_mismatch, st.captures), (1, 0), "{name}");
    }
    // The guest's full-screen mapping (with D3D's half-pixel bias) warps.
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (a, b) = turning();
    r.at(F);
    frame_mapped(&mut r, &a, &|_| full);
    r.at(2 * F);
    frame_mapped(&mut r, &b, &|_| full);
    let st = *r.b.stats().lock().unwrap();
    assert_eq!((st.size_mismatch, st.warps), (0, 1));
}

#[test]
fn a_made_frame_of_another_size_is_not_shown() {
    for mode in modes() {
        let mut r = rig_with(60, mode);
        // Every surface shows at 640x480 but the guest's back buffer, at
        // 1280x960: a private copy that missed the render scale.
        for id in 0..64 {
            r.b.inner_mut().extents.insert(id, (640, 480));
        }
        r.b.inner_mut().extents.insert(r.color, (1280, 960));
        let (a, b) = turning();
        r.frame(F, &a);
        let before = r.calls();
        r.frame(2 * F, &b);
        r.b.idle_at(2 * F + F / 2);
        assert_eq!(r.presents_since(before), vec![(r.color, 4)], "{mode:?}");
        let st = *r.b.stats().lock().unwrap();
        assert_eq!((st.size_mismatch, st.blended_frames), (1, 0), "{mode:?}");
        assert!(st.to_string().contains("size_mismatch 1"), "{st}");
    }
}

#[test]
fn in_between_frames_wait_for_the_gpu() {
    for mode in modes() {
        let mut r = rig_with(60, mode);
        let busy = Arc::new(std::sync::atomic::AtomicBool::new(false));
        r.b.inner_mut().gpu_busy = Some(Arc::clone(&busy));
        let (a, b) = turning();
        r.frame(F, &a);
        r.frame(2 * F, &b);
        assert_eq!(r.b.stats().lock().unwrap().blended_frames, 1);
        // The GPU is still on that frame: the slot is dropped.
        busy.store(true, Ordering::Release);
        let before = r.calls();
        assert!(r.b.idle_at(2 * F + F / 2).is_some(), "{mode:?}");
        assert_eq!(r.calls(), before, "{mode:?}: no work queued behind it");
        // And the next guest frame is shown as drawn.
        r.frame(3 * F, &a);
        assert_eq!(r.presents_since(before), vec![(r.color, 4)], "{mode:?}");
        assert_eq!(r.b.stats().lock().unwrap().dropped, 2, "{mode:?}");
        // Caught up: blending again.
        busy.store(false, Ordering::Release);
        r.frame(4 * F, &b);
        assert_eq!(r.b.stats().lock().unwrap().blended_frames, 2, "{mode:?}");
    }
}

#[test]
fn a_render_target_a_frame_samples_is_kept_as_it_was_drawn() {
    let mut r = rig(60);
    r.b.inner_mut().sample_surfaces = true;
    let target =
        r.b.create_surface(D3DFormat::A8R8G8B8, 320, 240, false)
            .unwrap();
    let texture = r.b.surface_texture(target).unwrap().unwrap();
    let live = *texture;
    let mut snapshot = None;
    for (i, s) in [scene(camera(0.0, 0.0)), scene(camera(1.0, 0.0))]
        .iter()
        .enumerate()
    {
        let (color, depth) = (r.color, r.depth);
        r.at((i as u64 + 1) * F);
        // The guest draws the target, samples it into the back buffer,
        // then draws something else (a smaller pass) into it.
        r.b.draw(target, None, &draw(block(), 40, true), [None; 4])
            .unwrap();
        let before = r.calls();
        r.b.clear(color, Some(depth), Clear::default()).unwrap();
        let world = world_block(&s.cam);
        for mesh in [1, 2] {
            r.b.draw(color, Some(depth), &draw(world, mesh, false), [None; 4])
                .unwrap();
        }
        r.b.draw(
            color,
            Some(depth),
            &draw(world, 3, false),
            [Some(&texture), None, None, None],
        )
        .unwrap();
        snapshot = r.b.inner().calls[before..].iter().find_map(|c| match c {
            Recorded::Snapshot { surface, id, .. } if *surface == target => Some(*id),
            _ => None,
        });
        r.b.draw(target, None, &draw(block(), 41, true), [None; 4])
            .unwrap();
        r.b.present(color, 4).unwrap();
    }
    let snapshot = snapshot.expect("a copy of the target as the draw read it");
    let before = r.calls();
    r.b.idle_at(2 * F + F / 2);
    let replayed: Vec<_> = r.b.inner().calls[before..]
        .iter()
        .filter_map(|c| match c {
            Recorded::Draw { draw, textures, .. } if draw.vertex_count == 3 => Some(textures[0]),
            _ => None,
        })
        .collect();
    assert_eq!(replayed, vec![Some(snapshot)], "not the live view {live}");

    // A surface copied into the frame is read live: held.
    let mut r = rig(60);
    let other =
        r.b.create_surface(D3DFormat::A8R8G8B8, 640, 480, false)
            .unwrap();
    for i in 1..=2u64 {
        r.at(i * F);
        let s = scene(camera(i as f64, 0.0));
        let (color, depth) = (r.color, r.depth);
        r.b.copy_surface(other, color).unwrap();
        for mesh in [1, 2, 3] {
            let d = draw(world_block(&s.cam), mesh, false);
            r.b.draw(color, Some(depth), &d, [None; 4]).unwrap();
        }
        r.b.present(color, 4).unwrap();
    }
    let st = *r.b.stats().lock().unwrap();
    assert_eq!((st.blended_frames, st.unsure), (0, 1));
}

#[test]
fn a_shadow_map_the_world_samples_does_not_hold_the_frame() {
    // The texture a depth level is sampled through is the level's texels
    // as of the call (the backend makes it, once each time the level is
    // drawn), not a view of the surface: a frame that samples it can be
    // made again. Every gameplay frame with dynamic shadows samples one.
    for mode in modes() {
        let mut r = rig_with(60, mode);
        r.b.inner_mut().sample_depth = true;
        let (a, b) = turning();
        for i in 1..=3u64 {
            let s = if i % 2 == 1 { &a } else { &b };
            let shadow =
                r.b.create_surface(D3DFormat::D24S8, 256, 256, true)
                    .unwrap();
            let tex = r.b.depth_level_texture(shadow).unwrap().unwrap();
            let (color, depth) = (r.color, r.depth);
            r.at(i * F);
            r.b.clear(color, Some(depth), Clear::default()).unwrap();
            let world = world_block(&s.cam);
            for mesh in [1, 2, 3] {
                let t = (mesh == 2).then_some(&tex);
                r.b.draw(
                    color,
                    Some(depth),
                    &draw(world, mesh, false),
                    [t, None, None, None],
                )
                .unwrap();
            }
            r.b.draw(color, None, &draw(hud_block(), 1000, true), [None; 4])
                .unwrap();
            r.b.present(color, 4).unwrap();
        }
        let st = *r.b.stats().lock().unwrap();
        assert_eq!((st.unsure, st.blended_frames), (0, 2), "{mode:?}: {st}");
    }
}

/// A frame's level draws for a camera.
type Draws<'a> = &'a dyn Fn(&Mat4) -> Vec<ResolvedDraw>;

/// A frame of `s` whose level draws are `draws(cam)`, then the weapon and
/// the HUD.
fn frame_with(r: &mut Rig, s: &Scene, draws: Draws<'_>) {
    let (color, depth) = (r.color, r.depth);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    for d in draws(&s.cam) {
        let depth = (!d.layout.pretransformed).then_some(depth);
        r.b.draw(color, depth, &d, [None; 4]).unwrap();
    }
    if let Some(m) = s.weapon {
        let mut c = block();
        write(&mut c, CAM, Layout::Rows, 4, &m);
        r.b.draw(color, Some(depth), &draw(c, 900, false), [None; 4])
            .unwrap();
    }
    r.b.draw(color, None, &draw(hud_block(), 1000, true), [None; 4])
        .unwrap();
    r.b.present(color, 4).unwrap();
}

/// E1 over two turning frames of `draws`: whether the guest's frame was
/// shown as drawn (and nothing warped), and the counters.
fn reproject_pair(draws: &dyn Fn(&Mat4) -> Vec<ResolvedDraw>) -> (bool, d3d::interp::InterpStats) {
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (a, b) = turning();
    r.at(F);
    frame_with(&mut r, &a, draws);
    let before = r.calls();
    r.at(2 * F);
    frame_with(&mut r, &b, draws);
    r.b.idle_at(2 * F + F / 2);
    let held = r.presents_since(before) == vec![(r.color, 4)] && warps_since(&r, before).is_empty();
    (held, *r.b.stats().lock().unwrap())
}

fn level(cam: &Mat4) -> Vec<ResolvedDraw> {
    [1, 2, 3]
        .into_iter()
        .map(|mesh| draw(world_block(cam), mesh, false))
        .collect()
}

#[test]
fn reproject_reads_the_screen_mapping_from_the_program_not_d3ds_registers() {
    // D3D's viewport registers (c58/c59, `hw`) say the whole target, but
    // the program maps with its own constants, as Halo 2's do (c10/c11):
    // the registers are not what the draws used.
    let stale = |mut d: ResolvedDraw| {
        d.hw.viewport_scale = [320.0, -240.0, 1.0, 0.0];
        d.hw.viewport_offset = [320.0, 240.0, 0.0, 0.0];
        d
    };
    // The program maps clip space to pixels one to one (screen space), or
    // onto the top-left quarter: the warp would zoom and turn the picture
    // about the wrong point, so it moves up and to the left.
    for (name, scale, offset) in [
        ("screen space", [1.0, 1.0], [0.0, 0.0]),
        ("quarter", [160.0, -120.0], [160.0, 120.0]),
    ] {
        let (held, st) = reproject_pair(&|cam| {
            level(cam)
                .into_iter()
                .map(|d| {
                    let mut c = *d.shader.constants;
                    c[SCALE] = [scale[0], scale[1], 1.0, 0.0];
                    c[OFFSET] = [offset[0], offset[1], 0.0, 0.0];
                    stale(draw(c, d.vertex_count, false))
                })
                .collect()
        });
        assert!(held, "{name}: {st}");
        assert_eq!((st.size_mismatch, st.captures), (1, 0), "{name}: {st}");
    }
    // The program's mapping covers the target and D3D's registers were
    // never pushed: warped.
    let (held, st) = reproject_pair(&level);
    assert!(!held && st.size_mismatch == 0 && st.warps > 0, "{st}");
}

/// A draw of `c` whose program passes its vertices through (`mov oPos,
/// v0`): already in screen space.
fn pass_through(c: Block, mesh: u32) -> ResolvedDraw {
    let mut d = draw(c, mesh, false);
    let mov = Inst::mac(MacOp::Mov, Some(O::v(0)), None, None)
        .to_out(false, OutReg::Pos, 0xF)
        .last();
    d.shader.vertex = VertexProcessing::Program {
        start: 0,
        microcode: mov.words().to_vec().into(),
    };
    d
}

#[test]
fn reproject_holds_a_world_it_cannot_overscan() {
    let cases: [(&str, Draws<'_>); 6] = [
        // A pre-transformed (screen-space) draw among the level's: its
        // pixels are not overscanned, so the warp would zoom it.
        ("pre-transformed", &|cam| {
            let mut v = level(cam);
            v.insert(1, draw(block(), 50, true));
            v
        }),
        // An orthographic (screen-space) quad among the level's: it would
        // shrink with the overscan and turn with the world.
        ("orthographic", &|cam| {
            let mut v = level(cam);
            v.insert(1, draw(hud_block(), 53, false));
            v
        }),
        // A world draw whose screen mapping is not a viewport transform the
        // replay can read (the camera's rows straight to `oPos`, no
        // epilogue).
        ("unknown program", &|cam| {
            let mut v = level(cam);
            let mut d = draw(world_block(cam), 51, false);
            let mut rows: Vec<Inst> =
                (0..4)
                    .map(|i| {
                        Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(CAM as u8 + i)), None)
                            .to_out(false, OutReg::Pos, 8 >> i)
                    })
                    .collect();
            let last = rows.pop().unwrap().last();
            rows.push(last);
            d.shader.vertex = VertexProcessing::Program {
                start: 0,
                microcode: rows
                    .iter()
                    .flat_map(|i| i.words())
                    .collect::<Vec<_>>()
                    .into(),
            };
            v.push(d);
            v
        }),
        // A full-target quad in pixels among the level's draws (a
        // pass-through program, `mov oPos, v0`, with the camera in its
        // constants all the same) whose vertices E1 cannot read (no
        // position attribute) to move with the overscan.
        ("pass-through", &|cam| {
            let mut v = level(cam);
            v.insert(1, pass_through(world_block(cam), 54));
            v
        }),
        // Fixed-function vertices.
        ("fixed function", &|cam| {
            let mut v = level(cam);
            let mut d = draw(world_block(cam), 52, false);
            d.shader.vertex = VertexProcessing::FixedFunction { fvf: 0x142 };
            v.push(d);
            v
        }),
        // A level draw scissored to part of the target: the overscanned
        // image would be cut at the guest's rectangle.
        ("scissored", &|cam| {
            let mut v = level(cam);
            v[1].clip = Some(d3d::backend::PixelRect {
                x: 0,
                y: 0,
                width: 320,
                height: 480,
            });
            v
        }),
    ];
    for (name, draws) in cases {
        let (held, st) = reproject_pair(draws);
        assert!(held, "{name}: {st}");
        assert_eq!((st.size_mismatch, st.captures), (1, 0), "{name}: {st}");
    }
}

#[test]
fn reproject_holds_level_draws_after_the_overlay_starts() {
    // A level draw after the first-person weapon would go on top of the
    // warped world unwarped, at frame N's camera: held.
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (a, b) = turning();
    for (i, s) in [a, b].iter().enumerate() {
        let (color, depth) = (r.color, r.depth);
        r.at((i as u64 + 1) * F);
        r.b.clear(color, Some(depth), Clear::default()).unwrap();
        let world = world_block(&s.cam);
        for mesh in [1, 2] {
            r.b.draw(color, Some(depth), &draw(world, mesh, false), [None; 4])
                .unwrap();
        }
        let mut c = block();
        write(&mut c, CAM, Layout::Rows, 4, &weapon());
        r.b.draw(color, Some(depth), &draw(c, 900, false), [None; 4])
            .unwrap();
        r.b.draw(color, Some(depth), &draw(world, 3, false), [None; 4])
            .unwrap();
        r.b.present(color, 4).unwrap();
    }
    let st = *r.b.stats().lock().unwrap();
    assert_eq!(
        (st.blended_frames, st.size_mismatch, st.captures),
        (0, 1, 0),
        "{st}"
    );
}

/// A loading screen: a pre-transformed background, particles drawn with a
/// centred perspective camera they share, and text.
fn loading_frame(r: &mut Rig, turn: f64) {
    let (color, depth) = (r.color, r.depth);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    r.b.draw(color, None, &draw(block(), 7, true), [None; 4])
        .unwrap();
    let cam = camera(0.0, turn);
    for mesh in 20..26 {
        r.b.draw(
            color,
            None,
            &draw(world_block(&cam), mesh, false),
            [None; 4],
        )
        .unwrap();
    }
    r.b.draw(color, None, &draw(hud_block(), 1000, false), [None; 4])
        .unwrap();
    r.b.present(color, 4).unwrap();
}

#[test]
fn loading_screens_are_shown_as_the_guest_drew_them() {
    for ahead in [false, true] {
        let mut r = rig_with(60, Mode::Reproject { ahead });
        let before = r.calls();
        for i in 1..=4 {
            r.at(i * F);
            loading_frame(&mut r, 0.05 * i as f64);
            r.b.idle_at(i * F + F / 2);
        }
        assert_eq!(
            r.presents_since(before),
            vec![(r.color, 4); 4],
            "ahead {ahead}"
        );
        assert!(warps_since(&r, before).is_empty(), "ahead {ahead}");
        let st = *r.b.stats().lock().unwrap();
        assert_eq!(
            (st.blended_frames, st.captures),
            (0, 0),
            "ahead {ahead}: {st}"
        );
    }
}

/// Runs the test program on world point `v` with constants `c`: (window
/// position, `oT0`).
fn run_program(c: &Block, v: [f32; 3]) -> ([f32; 4], [f32; 4]) {
    let p = shaders::vsh::Program::from_words(shaders::vsh::ProgramKind::Normal, &program_words())
        .unwrap();
    let mut inputs = [[0.0f32; 4]; 16];
    inputs[0] = [v[0], v[1], v[2], 1.0];
    let mut c = *c;
    let out = shaders::vsh::interp::run(&p, &inputs, &mut c);
    (out.get(OutReg::Pos), out.get(OutReg::T(0)))
}

#[test]
fn hor_plus_clip_derived_lookup_samples_the_display_texel_drawn_by_the_world() {
    // Synthetic program forwards its homogeneous clip position to the
    // projective texture coordinates before its own viewport epilogue.
    let mut retail = world_block(&build::projection(1.2, 4.0 / 3.0, 0.06, 1024.0));
    retail[LOOKUP] = [0.5, 0.0, 0.0, 0.5];
    retail[LOOKUP + 1] = [0.0, -0.5, 0.0, 0.5];
    for (w, h) in [(1920, 1080), (3440, 1440), (5120, 1440)] {
        let mut view = retail;
        let factor = (4.0 / 3.0) / (w as f32 / h as f32);
        assert!(d3d::device::widen_projection_rows(&mut view[CAM..CAM + 4], factor));
        for point in [[0.3, 0.4, -5.0], [-4.1, 1.0, -4.0]] {
            let (original, _) = run_program(&retail, point);
            let (window, tex) = run_program(&view, point);
            let same = |a: f32, b: f32| assert!((a - b).abs() < 0.001, "{a} != {b}");
            same(window[0], 320.0 + (original[0] - 320.0) * factor);
            same(window[1], original[1]);
            if point[0] < 0.0 {
                assert!(original[0] < 0.0 && window[0] > 0.0, "inside the wider lens");
            }
            same(tex[0] / tex[3] * w as f32, window[0] * (w as f32 / 640.0));
            same(tex[1] / tex[3] * h as f32, window[1] * (h as f32 / 480.0));
        }
    }
}

#[test]
fn the_world_image_keeps_screen_space_lookups_where_the_guest_had_them() {
    // Level draws that look a screen-space texture up from their clip
    // position (Halo 2 has many: `dp4 oT0.x, r12, c8`) must read the same
    // texel for the same point in the world image, while the point itself
    // moves in by the overscan about the screen's centre.
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (a, b) = turning();
    let lookup = |mut c: Block| {
        c[LOOKUP] = [0.5, 0.0, 0.0, 0.5];
        c[LOOKUP + 1] = [0.0, -0.5, 0.0, 0.5];
        c
    };
    let draws = move |cam: &Mat4| -> Vec<ResolvedDraw> {
        [1, 2, 3]
            .into_iter()
            .map(|mesh| draw(lookup(world_block(cam)), mesh, false))
            .collect()
    };
    r.at(F);
    frame_with(&mut r, &a, &draws);
    let before = r.calls();
    r.at(2 * F);
    frame_with(&mut r, &b, &draws);
    let shown = r.presents_since(before)[0].0;
    assert_ne!(shown, r.color, "warped");
    let d = r.draws_since(before);
    let guest = &d.iter().find(|(c, _)| *c == r.color).unwrap().1;
    let world = &d
        .iter()
        .find(|(c, x)| *c != r.color && *c != shown && x.vertex_count == 1)
        .unwrap()
        .1;
    let s = overscan_scale() as f32;
    // In front of the camera (it looks down -z).
    for p in [[0.5, 0.3, -10.0], [3.0, -2.0, -40.0], [-1.0, 1.0, -5.0]] {
        let (gp, gt) = run_program(&guest.shader.constants, p);
        let (wp, wt) = run_program(&world.shader.constants, p);
        assert_eq!(gt, wt, "{p:?}: the lookup");
        for (i, centre) in [(0, 320.0), (1, 240.0)] {
            let want = centre + s * (gp[i] - centre);
            assert!((wp[i] - want).abs() < 1e-3, "{p:?}: {wp:?} vs {gp:?}");
        }
    }
}

#[test]
fn the_held_counters_say_why() {
    let (_, st) = reproject_pair(&|cam| {
        let mut v = level(cam);
        v.insert(1, draw(block(), 50, true));
        v
    });
    let line = st.to_string();
    assert!(
        line.contains("size_mismatch 1 (extent 0 mapping 0 screen 1 overlay 0)"),
        "{line}"
    );
    assert!(line.contains("unsure 0 (camera 0 unpinned 0)"), "{line}");
}

/// Where every draw of a gameplay frame carries a block that splits as a
/// perspective but places nothing (packed parameters, a light's or a
/// texture's matrix): shared by more draws than the camera.
const PACKED: usize = 129;

fn packed_matrix() -> Mat4 {
    [
        [250.0, 0.0, 300.0, 0.0],
        [0.0, -250.0, 200.0, 0.0],
        [0.0, 0.0, 1.0, -0.1],
        [0.0, 0.0, 1.0, 0.0],
    ]
}

fn packed(mut c: Block) -> Block {
    write(&mut c, PACKED, Layout::Rows, 4, &packed_matrix());
    c
}

/// A light's cameras for the shadow pass: an orthographic one and an
/// off-centre perspective.
fn lights() -> [Mat4; 2] {
    let ortho: Mat4 = [
        [0.05, 0.0, 0.0, 0.0],
        [0.0, 0.05, 0.0, 0.0],
        [0.0, 0.0, 0.01, 0.5],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut off = build::view_projection(0.9, [3.0, 4.0, 10.0], 0.3, -0.8);
    // Shift the image off centre (a light frustum fitted to the view).
    let w = off[3];
    for (j, w) in w.iter().enumerate() {
        off[0][j] += 0.4 * w;
        off[1][j] -= 0.3 * w;
    }
    [ortho, off]
}

/// A gameplay frame as the game draws it: a shadow pass into a depth
/// target with the light's cameras, the level with the centred camera
/// (one draw sampling the shadow map), the first-person weapon with its
/// own projection, then the HUD: pass-through quads carrying the frame's
/// constants (the camera among them) and orthographic text. Every draw
/// carries the packed block.
fn gameplay_frame(r: &mut Rig, s: &Scene, shadow: (u32, u32)) {
    let (sc, sd) = shadow;
    r.b.clear(sc, Some(sd), Clear::default()).unwrap();
    for (k, light) in lights().iter().enumerate() {
        let mut c = packed(block());
        write(&mut c, CAM, Layout::Rows, 4, light);
        let mut d = draw(c, 300 + k as u32, false);
        d.target.width = 256;
        d.target.height = 256;
        d.viewport.width = 256;
        d.viewport.height = 256;
        r.b.draw(sc, Some(sd), &d, [None; 4]).unwrap();
    }
    let tex = r.b.depth_level_texture(sd).unwrap().unwrap();
    let (color, depth) = (r.color, r.depth);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    let world = packed(world_block(&s.cam));
    for mesh in [1, 2, 3, 4] {
        let t = (mesh == 2).then_some(&tex);
        r.b.draw(
            color,
            Some(depth),
            &draw(world, mesh, false),
            [t, None, None, None],
        )
        .unwrap();
    }
    let mut c = packed(block());
    write(&mut c, CAM, Layout::Rows, 4, &s.weapon.expect("a weapon"));
    r.b.draw(color, Some(depth), &draw(c, 900, false), [None; 4])
        .unwrap();
    for mesh in [1000, 1001, 1002] {
        r.b.draw(color, None, &pass_through(world, mesh), [None; 4])
            .unwrap();
    }
    for mesh in [1003, 1004] {
        r.b.draw(
            color,
            None,
            &draw(packed(hud_block()), mesh, false),
            [None; 4],
        )
        .unwrap();
    }
    r.b.present(color, 4).unwrap();
}

#[test]
fn a_gameplay_frame_blends() {
    // The packed block looks like a perspective and is in more draws than
    // the camera, and is not centred: it must not be taken for the camera,
    // nor make the world draws look like screen space.
    let p = Persp::split(&packed_matrix()).expect("a perspective");
    assert!(!p.centred());
    for mode in modes() {
        let mut r = rig_with(60, mode);
        r.b.inner_mut().sample_depth = true;
        let sc =
            r.b.create_surface(D3DFormat::A8R8G8B8, 256, 256, false)
                .unwrap();
        let sd =
            r.b.create_surface(D3DFormat::D24S8, 256, 256, true)
                .unwrap();
        let (a, b) = turning();
        let before = r.calls();
        for i in 1..=4u64 {
            r.at(i * F);
            gameplay_frame(&mut r, if i % 2 == 1 { &a } else { &b }, (sc, sd));
            r.b.idle_at(i * F + F / 2);
        }
        let st = *r.b.stats().lock().unwrap();
        assert_eq!(
            (st.blended_frames, st.unsure, st.size_mismatch),
            (3, 0, 0),
            "{mode:?}: {st}"
        );
        if mode != Mode::Replay {
            assert!(!warps_since(&r, before).is_empty(), "{mode:?}: {st}");
        }
    }
}

/// A 2D front-end frame: a pre-transformed background and orthographic
/// text, nothing in perspective.
fn flat_frame(r: &mut Rig) {
    let (color, depth) = (r.color, r.depth);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    r.b.draw(color, None, &draw(block(), 7, true), [None; 4])
        .unwrap();
    for mesh in [1000, 1001] {
        r.b.draw(color, None, &draw(hud_block(), mesh, false), [None; 4])
            .unwrap();
    }
    r.b.present(color, 4).unwrap();
}

/// Split screen: two views, two level draws each.
fn split_frame(r: &mut Rig, cam: Mat4) {
    let (color, depth) = (r.color, r.depth);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    for (i, cam) in [cam, camera(5.0, 1.0)].into_iter().enumerate() {
        for mesh in [1, 2] {
            let mut d = draw(world_block(&cam), 10 * i as u32 + mesh, false);
            d.viewport.y = 240 * i as u32;
            d.viewport.height = 240;
            r.b.draw(color, Some(depth), &d, [None; 4]).unwrap();
        }
    }
    r.b.present(color, 4).unwrap();
}

#[test]
fn every_held_frame_says_why() {
    type Frames<'a> = &'a dyn Fn(&mut Rig, u64);
    let (a, b) = turning();
    let gameplay = |r: &mut Rig, i: u64| {
        r.b.inner_mut().sample_depth = true;
        let sc =
            r.b.create_surface(D3DFormat::A8R8G8B8, 256, 256, false)
                .unwrap();
        let sd =
            r.b.create_surface(D3DFormat::D24S8, 256, 256, true)
                .unwrap();
        gameplay_frame(r, if i % 2 == 1 { &a } else { &b }, (sc, sd));
    };
    // (what, modes, frames, the reason line every held frame lands in)
    let e1 = [
        Mode::Reproject { ahead: false },
        Mode::Reproject { ahead: true },
    ];
    let cases: [(&str, &[Mode], Frames<'_>, &str); 5] = [
        // Menus in perspective into pixels: the camera is not centred.
        (
            "menu",
            &modes(),
            &|r, _| menu_frame(r),
            "unsure 3 (camera 3 unpinned 0)",
        ),
        // A 2D menu: nothing in perspective.
        (
            "flat menu",
            &modes(),
            &|r, _| flat_frame(r),
            "unpaired 5 (gap 1 camera 4 views 0 counting 0)",
        ),
        // A loading screen: its pre-transformed background is drawn
        // before the particles, under them.
        (
            "loading screen",
            &e1,
            &|r, i| loading_frame(r, 0.05 * i as f64),
            "size_mismatch 3 (extent 0 mapping 0 screen 3 overlay 0)",
        ),
        // Split screen, for E1.
        (
            "split screen",
            &e1,
            &|r, i| split_frame(r, camera(0.0, 0.05 * i as f64)),
            "unpaired 5 (gap 1 camera 0 views 4 counting 0)",
        ),
        // Gameplay after a pause: the frame after the gap has nothing to
        // blend from.
        (
            "gap",
            &modes(),
            &|r, i| gameplay(r, i),
            "unpaired 3 (gap 2 camera 0 views 1 counting 0)",
        ),
    ];
    for (what, modes, frames, why) in cases {
        for &mode in modes {
            let mut r = rig_with(60, mode);
            for i in 1..=4u64 {
                // The third frame comes after a pause.
                let t = if what == "gap" && i >= 3 {
                    (i + 10) * F
                } else {
                    i * F
                };
                r.at(t);
                frames(&mut r, i);
                r.b.idle_at(t + F / 2);
            }
            let st = *r.b.stats().lock().unwrap();
            let line = st.to_string();
            assert!(line.contains(why), "{what} {mode:?}: {line}");
            assert_eq!(
                st.held_frames,
                st.unpaired + st.unsure + st.size_mismatch + st.dropped,
                "{what} {mode:?}: every held frame is counted: {line}"
            );
            assert_eq!(
                st.frames,
                st.blended_frames + st.held_frames,
                "{what} {mode:?}: {line}"
            );
        }
    }
}

/// A draw in window pixels: `program` (no `.last()` needed) over vertices
/// with a float4 position at register 0 and a float2 texture coordinate at
/// register 1, as Halo 2's pass-through world draws are.
fn screen_draw(c: Block, program: &[Inst], verts: &[([f32; 4], [f32; 2])]) -> ResolvedDraw {
    use gfx::vertex::{StreamLayout, VertexAttribute, VertexDataType, VertexRegister};
    let mut d = draw(c, verts.len() as u32, false);
    d.primitive = PrimitiveType::TriangleFan;
    let n = program.len();
    d.shader.vertex = VertexProcessing::Program {
        start: 0,
        microcode: program
            .iter()
            .enumerate()
            .flat_map(|(i, x)| {
                if i + 1 == n {
                    x.clone().last().words()
                } else {
                    x.words()
                }
            })
            .collect::<Vec<u32>>()
            .into(),
    };
    d.layout.streams = vec![StreamLayout {
        stream: 0,
        attributes: vec![
            VertexAttribute {
                register: VertexRegister(0),
                data_type: VertexDataType::Float4,
                offset: 0,
            },
            VertexAttribute {
                register: VertexRegister(1),
                data_type: VertexDataType::Float2,
                offset: 16,
            },
        ],
        stride: 24,
    }];
    d.streams = vec![StreamBytes {
        data: verts
            .iter()
            .flat_map(|(p, t)| {
                p.iter()
                    .chain(t)
                    .flat_map(|v| v.to_le_bytes())
                    .collect::<Vec<u8>>()
            })
            .collect::<Vec<u8>>()
            .into(),
        stride: 24,
    }];
    d
}

fn mov_out(src: O, out: OutReg) -> Inst {
    Inst::mac(MacOp::Mov, Some(src), None, None).to_out(false, out, 0xF)
}

/// The corners of the whole 640x480 target, texture coordinates 0 to 1.
fn full_target() -> Vec<([f32; 4], [f32; 2])> {
    vec![
        ([0.0, 0.0, 0.0, 1.0], [0.0, 0.0]),
        ([640.0, 0.0, 0.0, 1.0], [1.0, 0.0]),
        ([640.0, 480.0, 0.0, 1.0], [1.0, 1.0]),
        ([0.0, 480.0, 0.0, 1.0], [0.0, 1.0]),
    ]
}

/// A full-target quad in pixels that looks a texture up from its own
/// position (`mov oPos, v0`, `mov oT0, v0`): E1 cannot move it.
fn screen_quad(c: Block) -> ResolvedDraw {
    screen_draw(
        c,
        &[
            mov_out(O::v(0), OutReg::Pos),
            mov_out(O::v(0), OutReg::T(0)),
        ],
        &full_target(),
    )
}

fn vertex_program_id(d: &ResolvedDraw) -> String {
    match &d.shader.vertex {
        VertexProcessing::Program { microcode, .. } => {
            format!("{:016x}", d3d::pipeline_store::program_id(microcode))
        }
        VertexProcessing::FixedFunction { .. } => panic!("no program"),
    }
}

#[test]
fn the_hold_log_names_every_draw_that_held_a_frame() {
    // A gameplay-like frame with work on another target first (so draw
    // and pass numbers are the guest frame's, as `render.frame` counts
    // them), then the level with a full-target screen-space pass that
    // looks a texture up from its own position (held: moved, it would read
    // another texel) and an orthographic quad among its draws, the weapon
    // and the HUD.
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let shadow =
        r.b.create_surface(D3DFormat::A8R8G8B8, 128, 128, false)
            .unwrap();
    let (a, b) = turning();
    for (i, s) in [&a, &b].into_iter().enumerate() {
        let (color, depth) = (r.color, r.depth);
        r.at((i as u64 + 1) * F);
        // Pass 0: draw 0.
        r.b.clear(shadow, None, Clear::default()).unwrap();
        r.b.draw(
            shadow,
            None,
            &draw(world_block(&s.cam), 7, false),
            [None; 4],
        )
        .unwrap();
        // Pass 1: draws 1 to 6 (the quad is 2, the orthographic draw 4,
        // the weapon 6).
        r.b.clear(color, Some(depth), Clear::default()).unwrap();
        let mut v = level(&s.cam);
        v.insert(1, screen_quad(world_block(&s.cam)));
        v.insert(3, draw(hud_block(), 53, false));
        for d in v {
            r.b.draw(color, Some(depth), &d, [None; 4]).unwrap();
        }
        let mut c = block();
        write(&mut c, CAM, Layout::Rows, 4, &weapon());
        r.b.draw(color, Some(depth), &draw(c, 900, false), [None; 4])
            .unwrap();
        // Pass 2 (no depth): the HUD, draw 7.
        r.b.draw(color, None, &draw(hud_block(), 1000, true), [None; 4])
            .unwrap();
        r.b.present(color, 4).unwrap();
    }
    let st = *r.b.stats().lock().unwrap();
    assert_eq!((st.mismatch_screen, st.blended_frames), (1, 0), "{st}");
    let log = r.b.holds().lock().unwrap().clone();
    // Both frames are held by the guard; the first is also unpaired (the
    // rig's empty first frame has no camera), so it is not in the counter.
    assert_eq!(log.frames, 2);
    let last = log.last.as_ref().unwrap();
    assert_eq!((last.reason, last.why), ("screen", "pass_through"));
    assert_eq!(last.frame, 3, "the rig's own present is frame 1");
    assert_eq!(
        (last.draw, last.pass, last.recorded),
        (Some(2), Some(1), Some(1))
    );
    assert_eq!((last.recorded_draws, last.world_draws), (7, 5));
    assert_eq!(last.overlay_draw, Some(6), "the weapon");
    assert_eq!(
        (last.position.as_str(), last.block.as_str()),
        ("pass-through", "none")
    );
    assert_eq!(last.primitive, "TriangleFan");
    assert_eq!(
        last.window,
        Some([0.0, 0.0, 640.0, 480.0]),
        "a full-target pass"
    );
    assert_eq!(last.target, [640, 480]);
    let id = vertex_program_id(&screen_quad(block()));
    assert_eq!(last.vertex_program.as_ref(), Some(&id));
    assert!(
        log.programs[&id].contains("mov oPos, v0"),
        "disassembled: {:?}",
        log.programs
    );
    // Every kind, not only the first: the quad and the orthographic draw,
    // each in both frames.
    let kinds: Vec<(&str, &str, u64, u64, u64)> = log
        .kinds
        .iter()
        .map(|k| (k.reason, k.why, k.frames_first, k.frames, k.draws))
        .collect();
    assert_eq!(
        kinds,
        [
            ("screen", "pass_through", 2, 2, 2),
            ("screen", "not_perspective", 0, 2, 2),
        ]
    );
    let ortho = &log.kinds[1].example;
    assert_eq!(
        (ortho.position.as_str(), ortho.block.as_str()),
        ("rows c[96]", "none")
    );
    assert_eq!(
        ortho.rows.unwrap()[3],
        [0.0, 0.0, 0.0, 1.0],
        "the orthographic rows as uploaded"
    );
    assert_eq!(ortho.draw, Some(4));
    assert_eq!(
        ortho.vertex_program,
        Some(vertex_program_id(&draw(block(), 1, false)))
    );
}

#[test]
fn the_hold_log_says_off_centre_and_after_the_overlay() {
    // A perspective into pixels before the level's draws.
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (a, _) = turning();
    r.at(F);
    frame_with(&mut r, &a, &|cam| {
        let mut v = level(cam);
        v.insert(0, draw(pixel_perspective(), 60, false));
        v
    });
    let last = r.b.holds().lock().unwrap().last.clone().unwrap();
    assert_eq!(
        (last.reason, last.why, last.block.as_str()),
        ("screen", "off_centre", "local off-centre")
    );
    assert!(last.projection.is_some(), "the split projection is shown");
    // A level draw after the weapon.
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (color, depth) = (r.color, r.depth);
    r.at(F);
    let world = world_block(&a.cam);
    r.b.clear(color, Some(depth), Clear::default()).unwrap();
    for mesh in [1, 2] {
        r.b.draw(color, Some(depth), &draw(world, mesh, false), [None; 4])
            .unwrap();
    }
    let mut c = block();
    write(&mut c, CAM, Layout::Rows, 4, &weapon());
    r.b.draw(color, Some(depth), &draw(c, 900, false), [None; 4])
        .unwrap();
    r.b.draw(color, Some(depth), &draw(world, 3, false), [None; 4])
        .unwrap();
    r.b.present(color, 4).unwrap();
    let log = r.b.holds().lock().unwrap().clone();
    let last = log.last.unwrap();
    assert_eq!((last.reason, last.why), ("overlay", "camera_after_overlay"));
    assert_eq!(
        (last.draw, last.world_draws, last.overlay_draw),
        (Some(3), 2, Some(2))
    );
    // A frame with no camera (the rig's first) is not logged: it is held
    // as unpaired, with nothing to warp.
    assert_eq!(log.frames, 1);
}

/// The vertices of a recorded `screen_draw` draw: (position, texture
/// coordinate).
fn screen_vertices(d: &ResolvedDraw) -> Vec<([f32; 4], [f32; 2])> {
    let f: Vec<f32> = d.streams[0]
        .data
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
        .collect();
    f.chunks_exact(6)
        .map(|v| ([v[0], v[1], v[2], v[3]], [v[4], v[5]]))
        .collect()
}

/// A sprite of Halo 2's kind: a few pixels at `(x, y)`, its program moving
/// position and texture coordinate.
fn sprite(c: Block, x: f32, y: f32) -> ResolvedDraw {
    let v = |dx: f32, dy: f32, u: f32, t: f32| ([x + dx, y + dy, 0.5, 1.0], [u, t]);
    screen_draw(
        c,
        &[
            mov_out(O::v(0), OutReg::Pos),
            mov_out(O::v(1), OutReg::T(0)),
        ],
        &[
            v(0.0, 0.0, 0.0, 0.0),
            v(6.0, 0.0, 1.0, 0.0),
            v(6.0, 6.0, 1.0, 1.0),
            v(0.0, 6.0, 0.0, 1.0),
        ],
    )
}

/// A full-target layer whose program moves everything (Halo 2's
/// `55c33fa0a8b277de` shape).
fn layer(c: Block) -> ResolvedDraw {
    screen_draw(
        c,
        &[
            mov_out(O::v(0), OutReg::Pos),
            mov_out(O::v(1), OutReg::T(0)),
        ],
        &full_target(),
    )
}

/// A full-target layer whose texture coordinate goes through a texture
/// matrix (`dp4 oT0.x, v1, c18`): not affine-checkable from the inputs
/// alone, so it is shrunk instead.
fn matrix_layer(c: Block) -> ResolvedDraw {
    screen_draw(
        c,
        &[
            mov_out(O::v(0), OutReg::Pos),
            Inst::mac(MacOp::Dp4, Some(O::v(1)), Some(O::c(18)), None).to_out(
                false,
                OutReg::T(0),
                8,
            ),
        ],
        &full_target(),
    )
}

/// Two turning frames of `draws` in E1: the second frame's world-image
/// draws (into a surface other than the shown one), and the counters.
fn reproject_world_image(
    draws: Draws<'_>,
) -> (
    Vec<ResolvedDraw>,
    Vec<ResolvedDraw>,
    d3d::interp::InterpStats,
) {
    let mut r = rig_with(60, Mode::Reproject { ahead: false });
    let (a, b) = turning();
    r.at(F);
    frame_with(&mut r, &a, draws);
    let before = r.calls();
    r.at(2 * F);
    frame_with(&mut r, &b, draws);
    let all = r.draws_since(before);
    let guest = all
        .iter()
        .filter(|(c, _)| *c == r.color)
        .map(|d| d.1.clone())
        .collect();
    let image = all
        .into_iter()
        .filter(|(c, _)| *c != r.color)
        .map(|d| d.1)
        .collect();
    (guest, image, *r.b.stats().lock().unwrap())
}

#[test]
fn screen_space_sprites_and_layers_in_the_world_turn_with_it() {
    // Halo 2's two kinds of pass-through world draws (the live hold log
    // on 03a_oldmombasa: every held gameplay frame had them), among the
    // level's draws: a sprite placed in pixels, a full-target layer whose
    // program only moves its inputs, and one with a texture matrix.
    let draws = |cam: &Mat4| {
        let mut v = level(cam);
        v.insert(1, sprite(world_block(cam), 100.0, 50.0));
        v.insert(3, layer(world_block(cam)));
        v.insert(4, matrix_layer(world_block(cam)));
        v
    };
    let (guest, image, st) = reproject_world_image(&draws);
    assert_eq!(
        (st.blended_frames, st.size_mismatch, st.captures),
        (1, 0, 1),
        "{st}"
    );
    assert_eq!((st.screen_shrunk, st.screen_extended), (2, 1), "{st}");
    assert!(
        st.to_string()
            .contains("screen-space draws 2 shrunk 1 extended"),
        "{st}"
    );
    let fans = |v: &[ResolvedDraw]| -> Vec<Vec<([f32; 4], [f32; 2])>> {
        v.iter()
            .filter(|d| d.primitive == PrimitiveType::TriangleFan)
            .map(screen_vertices)
            .collect()
    };
    let (g, w) = (fans(&guest), fans(&image));
    assert_eq!(
        (g.len(), w.len()),
        (3, 3),
        "the guest's and the world image's"
    );
    // The guest's own draws are untouched.
    assert_eq!(g[0][0].0, [100.0, 50.0, 0.5, 1.0]);
    assert_eq!(g[1], full_target());
    let s = overscan_scale() as f32;
    let close = |a: f32, b: f32| (a - b).abs() < 1e-3;
    // The sprite: shrunk about the centre with the world, its texture
    // coordinates and depth as the guest's.
    for (gv, wv) in g[0].iter().zip(&w[0]) {
        assert!(close(wv.0[0], 320.0 + (gv.0[0] - 320.0) * s), "{wv:?}");
        assert!(close(wv.0[1], 240.0 + (gv.0[1] - 240.0) * s), "{wv:?}");
        assert_eq!((wv.0[2], wv.0[3], wv.1), (gv.0[2], gv.0[3], gv.1));
    }
    // The layer: still over the whole image, its texture coordinates
    // those of the guest's pixel the image shows there (outside 0..1 in
    // the band the overscan adds).
    for (gv, wv) in g[1].iter().zip(&w[1]) {
        assert_eq!(wv.0, gv.0, "positions kept");
        let q = [320.0 + (gv.0[0] - 320.0) / s, 240.0 + (gv.0[1] - 240.0) / s];
        assert!(
            close(wv.1[0], q[0] / 640.0) && close(wv.1[1], q[1] / 480.0),
            "{wv:?}"
        );
    }
    assert!(close(w[1][0].1[0], 0.5 - 0.5 / s), "{:?}", w[1][0]);
    // The layer with a texture matrix: shrunk.
    for (gv, wv) in g[2].iter().zip(&w[2]) {
        assert!(close(wv.0[0], 320.0 + (gv.0[0] - 320.0) * s), "{wv:?}");
        assert_eq!(wv.1, gv.1);
    }
}

#[test]
fn screen_space_world_draws_e1_cannot_move_still_hold() {
    let cases: [(&str, Draws<'_>); 3] = [
        // A scissored sprite: the overscanned image would be cut at the
        // guest's rectangle.
        ("scissored sprite", &|cam| {
            let mut v = level(cam);
            let mut d = sprite(world_block(cam), 100.0, 50.0);
            d.clip = Some(d3d::backend::PixelRect {
                x: 0,
                y: 0,
                width: 320,
                height: 480,
            });
            v.insert(1, d);
            v
        }),
        // A layer that looks a texture up from its own position.
        ("position lookup", &|cam| {
            let mut v = level(cam);
            v.insert(1, screen_quad(world_block(cam)));
            v
        }),
        // An orthographic quad (the letterbox bars and full-screen fills
        // the hold log showed in two frames): screen-fixed, held.
        ("orthographic", &|cam| {
            let mut v = level(cam);
            v.insert(1, draw(hud_block(), 53, false));
            v
        }),
    ];
    for (name, draws) in cases {
        let (held, st) = reproject_pair(draws);
        assert!(held, "{name}: {st}");
        assert_eq!(
            (st.mismatch_screen, st.captures, st.screen_shrunk),
            (1, 0, 0),
            "{name}: {st}"
        );
    }
}

#[test]
fn screen_space_draws_turn_with_the_world_through_the_interpreter() {
    // Run the world image's sprite and the level's program through the
    // shader interpreter: a world point and a sprite placed on it by the
    // guest (projected with frame N's camera) still coincide in the world
    // image, so the warp turns them together.
    let cam = camera(0.0, 0.0);
    let c = world_block(&cam);
    let point = [0.3f32, 4.0, 1.2];
    let (guest_win, _) = run_program(&c, point);
    let mut over = c;
    over[SCALE][0] *= overscan_scale() as f32;
    over[SCALE][1] *= overscan_scale() as f32;
    let (image_win, _) = run_program(&over, point);
    let d = sprite(c, guest_win[0], guest_win[1]);
    let pt = d3d::interp::screen_space::pass_through(match &d.shader.vertex {
        VertexProcessing::Program { microcode, .. } => microcode,
        VertexProcessing::FixedFunction { .. } => unreachable!(),
    })
    .unwrap();
    let screen = d3d::interp::reproject::Screen {
        scale: [320.0, -240.0],
        offset: [320.0, 240.0],
        size: [640.0, 480.0],
    };
    let (streams, fit) =
        d3d::interp::screen_space::world_image_streams(&d, pt, overscan_scale(), &screen).unwrap();
    assert_eq!(fit, d3d::interp::screen_space::Fit::Shrink);
    let mut moved = d.clone();
    moved.streams = streams;
    let corner = screen_vertices(&moved)[0].0;
    assert!(
        (corner[0] - image_win[0]).abs() < 1e-2 && (corner[1] - image_win[1]).abs() < 1e-2,
        "sprite {corner:?}, world point {image_win:?}"
    );
}
