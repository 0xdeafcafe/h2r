//! The interpolated replay (docs/game/high-framerate.md, "Phase 2") on
//! synthetic draw lists through the recording backend: what in-between
//! frames draw at alpha 0, 0.5 and 1, how draws pair up across frames with
//! spawns, despawns and instances, what stays untouched (HUD), the snaps,
//! and that the guest's own calls reach the backend exactly as without it.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use d3d::backend::{Recorded, StreamBytes, TargetInfo, Viewport};
use d3d::interp::InterpBackend;
use d3d::interp::camera::{Persp, build};
use d3d::interp::math::{Mat4, Similarity, max_abs, max_diff, mul};
use d3d::interp::model::{Layout, read4, write};
use d3d::shader::{CONSTANTS, ShaderInputs, VertexProcessing};
use d3d::state::StateWords;
use d3d::{Backend, RecordingBackend, ResolvedDraw};
use gfx::vertex::{PrimitiveType, VertexLayout};
use gfx::xdk::RenderStateTable;
use gfx::{Clear, D3DFormat};

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
                microcode: vec![mesh, 1, 2, 3].into(),
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
        glyph_cache: 0,
        clip: None,
        clip_rects: Vec::new(),
        hw: d3d::hw::HwState::default(),
    }
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
    // Some lighting constants that are no matrix.
    c[110] = [0.3, 0.6, 0.9, 1.0];
    c[111] = [0.5, 0.5, 0.5, 0.0];
    c
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
    let clock = Arc::new(AtomicU64::new(0));
    let rate = Arc::new(AtomicU32::new(fps));
    let (c, r) = (Arc::clone(&clock), Arc::clone(&rate));
    let mut b = InterpBackend::new(RecordingBackend::new(), move || r.load(Ordering::Relaxed))
        .with_clock(move || c.load(Ordering::Relaxed));
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
    assert!(r.b.idle_at(2 * F + F / 2).is_some(), "asks again soon");
    assert_eq!(r.calls(), before, "nothing drawn into the open count");
    r.b.report_pixel_count().unwrap();
    r.b.idle_at(2 * F + F / 2 + 1_000_000);
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
