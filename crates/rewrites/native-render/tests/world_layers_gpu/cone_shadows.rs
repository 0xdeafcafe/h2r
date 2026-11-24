//! E3 shadows from cones on invented scenes (docs/graphics/e3-lighting.md,
//! slice L7). The same 64-pixel top-down orthographic view as the sun's
//! tests: world x and y in `[-4, 4]`, eight pixels a unit, pixel
//! `(i, j)` centred at `x = -4 + (i + 0.5) / 8`, `y = 4 - (j + 0.5) / 8`.
//!
//! A lamp hangs over the ground: a frustum with its apex at z = 11,
//! looking down, reaching the ground (z = 0) with half extents of five.
//! The receivers' shadow term multiplies the frame's destination alpha,
//! which the light's own passes then read; RGB is kept.

use gfx::GfxContext;
use native_render::cone_shadows::Cone;
use native_render::cone_shadows_gpu::{ConeShadows, ConeTile};
use native_render::sun_tiles_gpu::{SunFrame, SunMesh, SunReceiver, SunShadows};

const SIZE: u32 = 64;
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
            eprintln!("cone shadow check skipped: {error}");
            None
        }
    }
}

/// The lamp: apex at z = 11, its axis down, the ground (depth 11) at half
/// extents of five.
fn lamp() -> Cone {
    Cone {
        apex: [0.0, 0.0, 11.0],
        axis: [0.0, 0.0, -1.0],
        right: [1.0, 0.0, 0.0],
        up: [0.0, 1.0, 0.0],
        slopes: [5.0 / 11.0, 5.0 / 11.0],
        near: 1.0,
        far: 11.0,
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

    /// A horizontal square of half side `h` at height `z`, facing up.
    fn square(h: f32, z: f32) -> Self {
        let mut g = Self::default();
        g.quad(
            [[-h, -h, z], [h, -h, z], [h, h, z], [-h, h, z]],
            [0.0, 0.0, 1.0],
        );
        g
    }

    fn mesh(&self, ctx: &GfxContext) -> SunMesh {
        SunMesh::new(&ctx.device, &self.p, &self.n, &self.i).unwrap()
    }

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
        (c, r.max(1e-3))
    }
}

fn texture(
    ctx: &GfxContext,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
) -> wgpu::Texture {
    ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("cone test target"),
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
}

/// Clears the frame to [`GREY`], lays the receivers' depth, and with a
/// tile at `rect` draws `casters` into it and its receivers' term; with
/// none, draws no shadow at all, as retail draws for a no-shadow light.
/// Returns the pixels and the tile and receiver draws.
fn render(
    ctx: &GfxContext,
    rect: Option<[u32; 4]>,
    casters: &[&Geometry],
    receivers: &[&Geometry],
) -> (Vec<u8>, usize, usize) {
    let colour_texture = texture(
        ctx,
        COLOUR,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
    );
    let depth_texture = texture(ctx, DEPTH, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let colour = colour_texture.create_view(&Default::default());
    let depth = depth_texture.create_view(&Default::default());
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    {
        let [r, g, b, a] = GREY.map(|c| f64::from(c) / 255.0);
        let _clear = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("cone test clear"),
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
    let receiver_meshes: Vec<SunMesh> = receivers.iter().map(|g| g.mesh(ctx)).collect();
    let laid: Vec<&SunMesh> = receiver_meshes.iter().collect();
    let mut sun = SunShadows::new(&ctx.device, 128);
    sun.draw_depth(
        &ctx.device,
        &mut enc,
        (&depth, DEPTH),
        TOP_DOWN,
        &laid,
        true,
    );
    let (mut tile_draws, mut receiver_draws) = (0, 0);
    if let Some(rect) = rect {
        let mut cones = ConeShadows::with_edge(&ctx.device, 512);
        let tile = ConeTile {
            rect,
            cone: lamp(),
            strength: 1.0,
        };
        let caster_meshes: Vec<SunMesh> = casters.iter().map(|g| g.mesh(ctx)).collect();
        let tiles = cones
            .prepare_tiles(&ctx.device, &[(tile, caster_meshes)])
            .unwrap();
        tile_draws = tiles.draws();
        tiles.record(&mut enc);
        let on: Vec<SunReceiver<'_>> = receivers
            .iter()
            .zip(&receiver_meshes)
            .map(|(g, mesh)| {
                let (centre, radius) = g.sphere();
                SunReceiver {
                    mesh,
                    centre,
                    radius,
                }
            })
            .collect();
        let frame = SunFrame {
            clip: TOP_DOWN,
            depth: [0.0; 4],
            viewport: None,
        };
        if let Some(pass) =
            cones.prepare_receivers(&ctx.device, (COLOUR, DEPTH), &frame, &tile, &on)
        {
            receiver_draws = pass.draws();
            pass.record(&mut enc, &colour, &depth);
        }
    }
    ctx.queue.submit([enc.finish()]);
    let pixels = gfx::readback::read_texture(
        ctx,
        &colour_texture,
        wgpu::TextureAspect::All,
        4,
        (SIZE, SIZE),
    );
    (pixels, tile_draws, receiver_draws)
}

fn px(pixels: &[u8], i: u32, j: u32) -> [u8; 4] {
    let at = 4 * (j * SIZE + i) as usize;
    pixels[at..at + 4].try_into().unwrap()
}

/// A board of half side one at z = 5 under the lamp shadows the ground
/// out to `11 / 6` of a unit from the axis: the destination alpha there
/// goes to about zero, with an edge at most two pixels wide, and stays
/// whole on the board's lit top and on the ground outside. RGB is kept
/// everywhere. Retail draws no shadow for the light: every alpha whole.
#[test]
fn a_no_shadow_cone_shadows_the_ground_in_e3_and_not_in_retail() {
    let Some(ctx) = test_gpu() else { return };
    let ground = Geometry::square(4.0, 0.0);
    let board = Geometry::square(1.0, 5.0);
    let (pixels, tile_draws, receiver_draws) =
        render(&ctx, Some([0, 0, 256, 256]), &[&board], &[&ground, &board]);
    assert_eq!((tile_draws, receiver_draws), (1, 2));
    // Row 32 is y = -0.06. Pixel 43 is x = 1.44: ground in the shadow.
    let shaded = px(&pixels, 43, 32);
    assert!(shaded[3] < 12, "ground in the shadow: {shaded:?}");
    assert_eq!(shaded[..3], GREY[..3], "RGB is kept");
    assert_eq!(px(&pixels, 32, 32), GREY, "the board's lit top");
    assert_eq!(px(&pixels, 56, 32), GREY, "ground outside the shadow");
    assert_eq!(px(&pixels, 32, 4), GREY, "ground outside the shadow");
    // The edge at x = 11 / 6 lies between pixels 46 and 47.
    let row: Vec<u8> = (40..56).map(|i| px(&pixels, i, 32)[3]).collect();
    let between = row.iter().filter(|&&a| a > 12 && a < 243).count();
    assert!(between <= 2, "the edge is sharp: {row:?}");
    let last = 40 + row.iter().rposition(|&a| a < 128).unwrap() as u32;
    assert!(
        (45..=47).contains(&last),
        "the edge is at x = 11 / 6: {row:?}"
    );

    let (retail, ..) = render(&ctx, None, &[&board], &[&ground, &board]);
    assert!(
        retail.chunks(4).all(|p| p == GREY),
        "retail draws no shadow"
    );
}

/// A tile in another slot of the atlas samples only its own rectangle:
/// the same scene from the slot at `(256, 256)` gives the same alpha.
#[test]
fn a_tile_reads_only_its_own_slot() {
    let Some(ctx) = test_gpu() else { return };
    let ground = Geometry::square(4.0, 0.0);
    let board = Geometry::square(1.0, 5.0);
    let (first, ..) = render(&ctx, Some([0, 0, 256, 256]), &[&board], &[&ground, &board]);
    let (second, ..) = render(
        &ctx,
        Some([256, 256, 256, 256]),
        &[&board],
        &[&ground, &board],
    );
    assert_eq!(first, second);
}
