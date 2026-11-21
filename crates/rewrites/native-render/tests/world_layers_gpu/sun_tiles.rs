//! E3 sun shadows from objects on invented scenes
//! (docs/graphics/e3-lighting.md, slice L5). A 64-pixel top-down
//! orthographic view of world x and y in `[-4, 4]`: eight pixels a unit,
//! pixel `(i, j)` centred at `x = -4 + (i + 0.5) / 8`, `y = 4 - (j + 0.5) / 8`.

use gfx::GfxContext;
use native_render::sun_tiles::{SunCaster, SunWork, rows_hash, screen_diameter};
use native_render::sun_tiles_gpu::{SunMesh, SunReceiver, SunShadows, SunTarget};

const SIZE: u32 = 64;
const ATLAS: u32 = 1024;
const COLOUR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// World to clip, looking down -z; higher z is nearer.
const TOP_DOWN: [[f32; 4]; 4] = [
    [0.25, 0.0, 0.0, 0.0],
    [0.0, 0.25, 0.0, 0.0],
    [0.0, 0.0, -0.05, 0.5],
    [0.0, 0.0, 0.0, 1.0],
];
const GREY: [u8; 4] = [200, 200, 200, 255];

fn test_gpu() -> Option<GfxContext> {
    match GfxContext::headless() {
        Ok(ctx) => Some(ctx),
        Err(error) => {
            eprintln!("sun tile check skipped: {error}");
            None
        }
    }
}

#[derive(Default)]
struct Geometry {
    p: Vec<[f32; 3]>,
    n: Vec<[f32; 3]>,
    i: Vec<u32>,
}

impl Geometry {
    fn quad(&mut self, corners: [[f32; 3]; 4], normal: [f32; 3]) {
        let first = self.p.len() as u32;
        self.p.extend(corners);
        self.n.extend([normal; 4]);
        self.i
            .extend([0, 1, 2, 0, 2, 3].into_iter().map(|k| first + k));
    }

    fn triangle(&mut self, corners: [[f32; 3]; 3], normals: [[f32; 3]; 3]) {
        let first = self.p.len() as u32;
        self.p.extend(corners);
        self.n.extend(normals);
        self.i.extend([first, first + 1, first + 2]);
    }

    fn cuboid(&mut self, lo: [f32; 3], hi: [f32; 3]) {
        let [x0, y0, z0] = lo;
        let [x1, y1, z1] = hi;
        self.quad(
            [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
            [0.0, 0.0, 1.0],
        );
        self.quad(
            [[x0, y0, z0], [x1, y0, z0], [x1, y1, z0], [x0, y1, z0]],
            [0.0, 0.0, -1.0],
        );
        self.quad(
            [[x1, y0, z0], [x1, y1, z0], [x1, y1, z1], [x1, y0, z1]],
            [1.0, 0.0, 0.0],
        );
        self.quad(
            [[x0, y0, z0], [x0, y1, z0], [x0, y1, z1], [x0, y0, z1]],
            [-1.0, 0.0, 0.0],
        );
        self.quad(
            [[x0, y1, z0], [x1, y1, z0], [x1, y1, z1], [x0, y1, z1]],
            [0.0, 1.0, 0.0],
        );
        self.quad(
            [[x0, y0, z0], [x1, y0, z0], [x1, y0, z1], [x0, y0, z1]],
            [0.0, -1.0, 0.0],
        );
    }

    /// The ground: x and y in `[-4, 4]` at z = 0, facing up.
    fn ground() -> Self {
        let mut g = Self::default();
        g.quad(
            [
                [-4.0, -4.0, 0.0],
                [4.0, -4.0, 0.0],
                [4.0, 4.0, 0.0],
                [-4.0, 4.0, 0.0],
            ],
            [0.0, 0.0, 1.0],
        );
        g
    }

    fn mesh(&self, ctx: &GfxContext) -> SunMesh {
        SunMesh::new(&ctx.device, &self.p, &self.n, &self.i).unwrap()
    }

    /// The bounding sphere of the vertices' box.
    fn sphere(&self) -> ([f32; 3], f32) {
        let mut lo = [f32::MAX; 3];
        let mut hi = [f32::MIN; 3];
        for p in &self.p {
            for k in 0..3 {
                lo[k] = lo[k].min(p[k]);
                hi[k] = hi[k].max(p[k]);
            }
        }
        let c = [0, 1, 2].map(|k| 0.5 * (lo[k] + hi[k]));
        let r = (0..3).map(|k| (hi[k] - c[k]).powi(2)).sum::<f32>().sqrt();
        (c, r)
    }

    fn caster(&self, id: u64, towards_light: [f32; 3], rows: u64) -> SunCaster {
        let (centre, radius) = self.sphere();
        SunCaster {
            id,
            centre,
            radius,
            screen_diameter: screen_diameter(&TOP_DOWN, SIZE, centre, radius),
            towards_light,
            strength: 1.0,
            rows,
        }
    }
}

/// Sun from +x, 45 degrees up: a shadow falls one unit in -x for each
/// unit of height, and an up-facing receiver's gate is `cos 45`.
const SUN_45: [f32; 3] = [1.0, 0.0, 1.0];
/// 200 times one minus `cos 45`: an up-facing receiver in full shadow.
const SHADED_45: i32 = 59;

struct Frame {
    colour: wgpu::Texture,
    depth: wgpu::Texture,
}

impl Frame {
    fn new(ctx: &GfxContext) -> Self {
        let make = |format, usage, label| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: SIZE,
                    height: SIZE,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        Self {
            colour: make(
                COLOUR,
                wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                "sun test colour",
            ),
            depth: make(
                DEPTH,
                wgpu::TextureUsages::RENDER_ATTACHMENT,
                "sun test depth",
            ),
        }
    }
}

/// Clears the frame to `rgba`, lays the receivers' depth, plans and draws
/// the tiles, multiplies the receivers, and reads the frame back.
fn render(
    ctx: &GfxContext,
    shadows: &mut SunShadows,
    casters: &[SunCaster],
    meshes: &[(u64, &SunMesh)],
    receivers: &[SunReceiver<'_>],
    rgba: [u8; 4],
) -> (Vec<u8>, SunWork) {
    let frame = Frame::new(ctx);
    let colour = frame.colour.create_view(&Default::default());
    let depth = frame.depth.create_view(&Default::default());
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let [r, g, b, a] = rgba.map(|c| f64::from(c) / 255.0);
        let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("sun test clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &colour,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color { r, g, b, a }),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let laid: Vec<&SunMesh> = receivers.iter().map(|r| r.mesh).collect();
    shadows.draw_depth(
        &ctx.device,
        &mut enc,
        (&depth, DEPTH),
        TOP_DOWN,
        &laid,
        true,
    );
    let mut plan = shadows.plan(casters);
    shadows
        .draw_tiles(&ctx.device, &mut enc, &mut plan, meshes)
        .unwrap();
    let target = SunTarget {
        colour: &colour,
        colour_format: COLOUR,
        depth: &depth,
        depth_format: DEPTH,
    };
    shadows.draw_receivers(
        &ctx.device,
        &mut enc,
        &target,
        TOP_DOWN,
        &mut plan,
        receivers,
    );
    ctx.queue.submit([enc.finish()]);
    let pixels = gfx::readback::read_texture(
        ctx,
        &frame.colour,
        wgpu::TextureAspect::All,
        4,
        (SIZE, SIZE),
    );
    (pixels, plan.work)
}

fn px(pixels: &[u8], i: u32, j: u32) -> [u8; 4] {
    let at = 4 * (j * SIZE + i) as usize;
    pixels[at..at + 4].try_into().unwrap()
}

fn near(value: u8, want: i32, by: i32) -> bool {
    (i32::from(value) - want).abs() <= by
}

fn receiver<'a>(g: &Geometry, mesh: &'a SunMesh) -> SunReceiver<'a> {
    let (centre, radius) = g.sphere();
    SunReceiver {
        mesh,
        centre,
        radius,
    }
}

/// A 2x2x2 box on the ground under a 45-degree sun shades the ground for
/// x in `(-3, -1)`, y in `(-1, 1)`, with an edge at most two pixels wide
/// at x = -3, and leaves its own lit top and the rest of the ground alone.
#[test]
fn a_box_on_a_plane_casts_a_hard_shadow() {
    let Some(ctx) = test_gpu() else { return };
    let ground = Geometry::ground();
    let mut boxed = Geometry::default();
    boxed.cuboid([-1.0, -1.0, 0.0], [1.0, 1.0, 2.0]);
    let (ground_mesh, box_mesh) = (ground.mesh(&ctx), boxed.mesh(&ctx));
    let mut shadows = SunShadows::new(&ctx.device, ATLAS);
    let caster = boxed.caster(1, SUN_45, 1);
    let (pixels, work) = render(
        &ctx,
        &mut shadows,
        &[caster],
        &[(1, &box_mesh)],
        &[receiver(&ground, &ground_mesh), receiver(&boxed, &box_mesh)],
        GREY,
    );
    assert_eq!(work.sun_tiles, 1);
    assert_eq!(work.sun_tiles_redrawn, 1);
    assert_eq!(work.tile_draws, 1);
    assert_eq!(work.receiver_draws, 2, "the ground and the box itself");
    // Row 28 is y = 0.44, through the middle of the shadow.
    assert_eq!(px(&pixels, 4, 28), GREY, "ground up-sun of the shadow");
    let shaded = px(&pixels, 16, 28);
    assert!(
        near(shaded[0], SHADED_45, 3),
        "ground in shadow: {shaded:?}"
    );
    assert_eq!(shaded[3], 255, "alpha is kept");
    assert_eq!(px(&pixels, 32, 28), GREY, "the box's lit top");
    assert_eq!(px(&pixels, 48, 28), GREY, "ground down-sun of the box");
    // Row 15 is y = 2.06, beside the box: no shadow.
    assert_eq!(px(&pixels, 16, 15), GREY, "ground beside the shadow");
    // The edge at x = -3 falls between pixels 7 and 8.
    let row: Vec<u8> = (0..20).map(|i| px(&pixels, i, 28)[0]).collect();
    let between = row
        .iter()
        .filter(|&&v| i32::from(v) > SHADED_45 + 3 && v < GREY[0] - 3)
        .count();
    assert!(between <= 2, "the edge is hard: {row:?}");
    let first = row.iter().position(|&v| v < 130).unwrap();
    assert!((7..=9).contains(&first), "the edge is at x = -3: {row:?}");
}

/// An L lying on its side: a slab x in `[-2, 2]`, z in `[0, 0.5]`, and a
/// wall on its +x end up to z = 3. The wall shades the slab's top for x
/// in `(-1.5, 1)`; the slab's top beyond, and the wall's top, stay lit.
#[test]
fn an_l_shaped_caster_shadows_itself() {
    let Some(ctx) = test_gpu() else { return };
    let mut l = Geometry::default();
    l.cuboid([-2.0, -1.0, 0.0], [2.0, 1.0, 0.5]);
    l.cuboid([1.0, -1.0, 0.5], [2.0, 1.0, 3.0]);
    let mesh = l.mesh(&ctx);
    let mut shadows = SunShadows::new(&ctx.device, ATLAS);
    let (pixels, work) = render(
        &ctx,
        &mut shadows,
        &[l.caster(1, SUN_45, 1)],
        &[(1, &mesh)],
        &[receiver(&l, &mesh)],
        GREY,
    );
    assert_eq!(work.receiver_draws, 1, "the caster receives its own tile");
    assert_eq!(px(&pixels, 8, 28), GREY, "background: no receiver");
    assert_eq!(px(&pixels, 17, 28), GREY, "the slab's lit top has no acne");
    let shaded = px(&pixels, 28, 28);
    assert!(
        near(shaded[0], SHADED_45, 3),
        "the wall shades the slab: {shaded:?}"
    );
    assert!(near(px(&pixels, 36, 28)[0], SHADED_45, 3));
    assert_eq!(px(&pixels, 44, 28), GREY, "the wall's lit top");
}

/// Under a plate with the sun straight overhead, every receiver point is
/// in shadow, so the term is one minus the interpolated gate. A triangle
/// whose three normals face away from the sun (one exactly sideways)
/// keeps the frame's RGB exactly. One whose gate is 1 at a vertex and 0
/// at the other two shades by its barycentric weight.
#[test]
fn nonpositive_normal_terms_keep_rgb_and_mixed_terms_interpolate() {
    let Some(ctx) = test_gpu() else { return };
    let overhead = [0.0, 0.0, 1.0];
    let mut plate = Geometry::default();
    plate.cuboid([-3.5, -3.5, 1.9], [3.5, 3.5, 2.0]);
    let up = [0.0, 0.0, 1.0];
    let down = [0.0, 0.0, -1.0];
    let mut away = Geometry::default();
    let a = [[-3.0, -2.0, 0.0], [-0.5, -2.0, 0.0], [-1.75, 2.0, 0.0]];
    away.triangle(a, [down, [1.0, 0.0, 0.0], [0.6, 0.0, -0.8]]);
    let mut mixed = Geometry::default();
    let b = [[0.5, -2.0, 0.0], [3.0, -2.0, 0.0], [1.75, 2.0, 0.0]];
    mixed.triangle(b, [up, down, down]);
    let (plate_mesh, away_mesh, mixed_mesh) = (plate.mesh(&ctx), away.mesh(&ctx), mixed.mesh(&ctx));
    let mut shadows = SunShadows::new(&ctx.device, ATLAS);
    let frame_rgba = [200, 120, 40, 77];
    let (pixels, work) = render(
        &ctx,
        &mut shadows,
        &[plate.caster(1, overhead, 1)],
        &[(1, &plate_mesh)],
        &[receiver(&away, &away_mesh), receiver(&mixed, &mixed_mesh)],
        frame_rgba,
    );
    assert_eq!(work.receiver_draws, 2);
    let world = |i: u32, j: u32| (-4.0 + (i as f32 + 0.5) / 8.0, 4.0 - (j as f32 + 0.5) / 8.0);
    // Barycentric weights of a point in a triangle of the xy plane.
    let weights = |t: [[f32; 3]; 3], (x, y): (f32, f32)| {
        let [p0, p1, p2] = t;
        let det = (p1[1] - p2[1]) * (p0[0] - p2[0]) + (p2[0] - p1[0]) * (p0[1] - p2[1]);
        let w0 = ((p1[1] - p2[1]) * (x - p2[0]) + (p2[0] - p1[0]) * (y - p2[1])) / det;
        let w1 = ((p2[1] - p0[1]) * (x - p2[0]) + (p0[0] - p2[0]) * (y - p2[1])) / det;
        [w0, w1, 1.0 - w0 - w1]
    };
    let (mut kept, mut graded) = (0, 0);
    for j in 0..SIZE {
        for i in 0..SIZE {
            let p = world(i, j);
            let got = px(&pixels, i, j);
            if weights(a, p).iter().all(|&w| w > 0.05) {
                assert_eq!(got, frame_rgba, "facing away at ({i}, {j})");
                kept += 1;
            }
            let w = weights(b, p);
            if w.iter().all(|&w| w > 0.05) {
                let term = 1.0 - w[0];
                for (c, &want) in got[..3].iter().zip(&frame_rgba[..3]) {
                    let want = (f32::from(want) * term).round() as i32;
                    assert!(
                        near(*c, want, 3),
                        "mixed at ({i}, {j}): {got:?}, gate {}",
                        w[0]
                    );
                }
                assert_eq!(got[3], frame_rgba[3], "alpha is kept");
                graded += 1;
            }
        }
    }
    assert!(kept > 50 && graded > 50, "{kept} kept, {graded} graded");
}

/// Two boxes on the ground. Drawn again with the same rows, neither tile
/// is redrawn (no caster mesh is even offered) and the picture is the
/// same. When one box moves, only its tile is redrawn, its shadow follows
/// it, and the still box's kept tile still shades.
#[test]
fn a_still_casters_tile_is_not_redrawn_and_a_moved_one_is() {
    let Some(ctx) = test_gpu() else { return };
    let ground = Geometry::ground();
    let ground_mesh = ground.mesh(&ctx);
    let mut still = Geometry::default();
    still.cuboid([-2.5, 1.0, 0.0], [-1.5, 2.0, 1.0]);
    let still_mesh = still.mesh(&ctx);
    let placed = |dy: f32| {
        let mut g = Geometry::default();
        g.cuboid([1.5, -2.0 + dy, 0.0], [2.5, -1.0 + dy, 1.0]);
        let rows = rows_hash(&[
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, dy],
            [0.0, 0.0, 1.0, 0.0],
        ]);
        (g, rows)
    };
    let (before, before_rows) = placed(0.0);
    let (after, after_rows) = placed(2.5);
    let (before_mesh, after_mesh) = (before.mesh(&ctx), after.mesh(&ctx));
    let mut shadows = SunShadows::new(&ctx.device, ATLAS);
    let still_caster = still.caster(1, SUN_45, 1);
    // Still box's shadow at (-3, 1.5); the mover's at (1, -1.5), then (1, 1).
    let (still_spot, old_spot, new_spot) = ((8, 20), (40, 44), (40, 24));
    let shade = |pixels: &[u8], (i, j): (u32, u32)| px(pixels, i, j)[0];

    let scene_before = [
        receiver(&ground, &ground_mesh),
        receiver(&still, &still_mesh),
        receiver(&before, &before_mesh),
    ];
    let casters_before = [still_caster, before.caster(2, SUN_45, before_rows)];
    let (first, work) = render(
        &ctx,
        &mut shadows,
        &casters_before,
        &[(1, &still_mesh), (2, &before_mesh)],
        &scene_before,
        GREY,
    );
    assert_eq!(
        (work.sun_tiles, work.sun_tiles_redrawn, work.tile_draws),
        (2, 2, 2)
    );
    assert!(near(shade(&first, still_spot), SHADED_45, 3));
    assert!(near(shade(&first, old_spot), SHADED_45, 3));
    assert_eq!(shade(&first, new_spot), GREY[0]);

    let (second, work) = render(
        &ctx,
        &mut shadows,
        &casters_before,
        &[],
        &scene_before,
        GREY,
    );
    assert_eq!(
        (work.sun_tiles, work.sun_tiles_redrawn, work.tile_draws),
        (2, 0, 0)
    );
    assert_eq!(second, first, "kept tiles draw the same picture");

    let scene_after = [
        receiver(&ground, &ground_mesh),
        receiver(&still, &still_mesh),
        receiver(&after, &after_mesh),
    ];
    let casters_after = [still_caster, after.caster(2, SUN_45, after_rows)];
    let (third, work) = render(
        &ctx,
        &mut shadows,
        &casters_after,
        &[(2, &after_mesh)],
        &scene_after,
        GREY,
    );
    assert_eq!(
        (work.sun_tiles, work.sun_tiles_redrawn, work.tile_draws),
        (2, 1, 1)
    );
    assert!(
        near(shade(&third, still_spot), SHADED_45, 3),
        "the kept tile still shades"
    );
    assert_eq!(shade(&third, old_spot), GREY[0], "the old shadow is gone");
    assert!(
        near(shade(&third, new_spot), SHADED_45, 3),
        "the shadow follows"
    );
}
