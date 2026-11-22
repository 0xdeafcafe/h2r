//! The menu picture's renderer on a headless device (skips without an
//! adapter), on an invented sphere: the glyph decal darkens the forehead
//! and nothing else, a different glyph changes it, a silhouette is dark
//! with no glyph, the tint colours the bone, each part takes its own
//! material's base map (white, so the tint alone, without one), and the
//! composite lands in its rectangle only. No disc data.

use native_render::gpu::Image;
use native_render::model::ModelVertex;
use native_render::showcase::{Look, SILHOUETTE_FILL, SILHOUETTE_RIM};
use native_render::showcase_gpu::{Showcase, ShowcaseMesh, ShowcasePart};
use native_render::world::Aabb;

/// 64 texels a row is 256 bytes: a readback needs no row padding.
const SIZE: u32 = 64;
const GLYPH: u32 = 32;

/// A unit sphere, its normals pointing out.
fn sphere() -> ShowcaseMesh {
    let (rings, segments) = (16u32, 24u32);
    let mut vertices = Vec::new();
    let mut bounds = Aabb::EMPTY;
    for r in 0..=rings {
        let theta = std::f32::consts::PI * r as f32 / rings as f32;
        for s in 0..=segments {
            let phi = std::f32::consts::TAU * s as f32 / segments as f32;
            let p = [
                theta.sin() * phi.cos(),
                theta.sin() * phi.sin(),
                theta.cos(),
            ];
            bounds.add(p);
            vertices.push(ModelVertex {
                position: p,
                uv: [s as f32 / segments as f32, r as f32 / rings as f32],
                normal: p,
                nodes: [0; 4],
                weights: [1.0, 0.0, 0.0, 0.0],
            });
        }
    }
    let mut indices = Vec::new();
    let row = segments + 1;
    for r in 0..rings {
        for s in 0..segments {
            let a = r * row + s;
            indices.extend([a, a + row, a + 1, a + 1, a + row, a + row + 1]);
        }
    }
    // Two parts: the upper half (material 0) and the lower (material 1).
    let half = rings / 2 * segments * 6;
    let parts = vec![
        ShowcasePart {
            material: 0,
            first_index: 0,
            index_count: half,
        },
        ShowcasePart {
            material: 1,
            first_index: half,
            index_count: indices.len() as u32 - half,
        },
    ];
    ShowcaseMesh {
        vertices,
        indices,
        parts,
        bounds,
    }
}

/// A glyph mask: a filled disc (`hollow` false) or a ring whose middle is
/// empty, inside a clear border.
fn glyph(hollow: bool) -> Vec<u8> {
    let c = (GLYPH as f32 - 1.0) / 2.0;
    (0..GLYPH * GLYPH)
        .map(|i| {
            let (x, y) = ((i % GLYPH) as f32 - c, (i / GLYPH) as f32 - c);
            let d = (x * x + y * y).sqrt();
            let on = d < 11.0 && (!hollow || d > 7.0);
            if on { 255 } else { 0 }
        })
        .collect()
}

fn look() -> Look {
    Look {
        yaw: 0.0,
        tint: [0.9, 0.9, 0.9],
        silhouette: false,
        glyph: true,
    }
}

fn read(ctx: &gfx::GfxContext, texture: &wgpu::Texture) -> Vec<[u8; 4]> {
    let (w, h) = (texture.width(), texture.height());
    let buffer = ctx.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: u64::from(w * h * 4),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(w * 4),
                rows_per_image: Some(h),
            },
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    ctx.queue.submit([enc.finish()]);
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, |r| r.unwrap());
    ctx.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    let bytes = buffer.slice(..).get_mapped_range().unwrap().to_vec();
    bytes
        .chunks_exact(4)
        .map(|p| [p[0], p[1], p[2], p[3]])
        .collect()
}

struct Rig {
    ctx: gfx::GfxContext,
    showcase: Showcase,
}

impl Rig {
    fn new() -> Option<Self> {
        let Ok(ctx) = gfx::GfxContext::headless() else {
            eprintln!("no wgpu adapter; skipping");
            return None;
        };
        let mut showcase = Showcase::new(&ctx.device, &ctx.queue);
        showcase.set_mesh(&ctx.device, &ctx.queue, &sphere(), &[]);
        Some(Self { ctx, showcase })
    }

    fn draw(&mut self, look: &Look, mask: &[u8]) -> Vec<[u8; 4]> {
        let ctx = &self.ctx;
        self.showcase
            .set_glyph(&ctx.device, &ctx.queue, GLYPH, mask);
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        assert!(self.showcase.draw(&ctx.device, &mut enc, SIZE, SIZE, look));
        ctx.queue.submit([enc.finish()]);
        read(ctx, self.showcase.picture().unwrap())
    }

    /// The decal's box in picture pixels, two pixels wider each way:
    /// `x0, y0, x1, y1`.
    fn decal_box(&self, yaw: f32) -> [f32; 4] {
        let p = self.showcase.placement().unwrap();
        let [x, y, z] = p.decal_centre;
        let r = p.decal_radius;
        let corners = [
            [x, y - r, z - r],
            [x, y + r, z - r],
            [x, y - r, z + r],
            [x, y + r, z + r],
        ]
        .map(|c| p.project(c, yaw, SIZE, SIZE).unwrap());
        let xs = corners.map(|c| c[0]);
        let ys = corners.map(|c| c[1]);
        let min = |v: [f32; 4]| v.into_iter().fold(f32::INFINITY, f32::min);
        let max = |v: [f32; 4]| v.into_iter().fold(f32::NEG_INFINITY, f32::max);
        [min(xs) - 2.0, min(ys) - 2.0, max(xs) + 2.0, max(ys) + 2.0]
    }

    fn decal_centre(&self, yaw: f32) -> (u32, u32) {
        let p = self.showcase.placement().unwrap();
        let [x, y] = p.project(p.decal_centre, yaw, SIZE, SIZE).unwrap();
        (x as u32, y as u32)
    }
}

fn luma(p: [u8; 4]) -> f32 {
    0.299 * f32::from(p[0]) + 0.587 * f32::from(p[1]) + 0.114 * f32::from(p[2])
}

fn inside(b: [f32; 4], i: usize) -> bool {
    let (x, y) = (
        (i as u32 % SIZE) as f32 + 0.5,
        (i as u32 / SIZE) as f32 + 0.5,
    );
    x >= b[0] && x <= b[2] && y >= b[1] && y <= b[3]
}

#[test]
fn the_glyph_darkens_the_forehead_and_nothing_else() {
    let Some(mut rig) = Rig::new() else { return };
    let plain = rig.draw(
        &Look {
            glyph: false,
            ..look()
        },
        &glyph(false),
    );
    let marked = rig.draw(&look(), &glyph(false));
    let decal = rig.decal_box(0.0);
    assert!(
        decal[1] < SIZE as f32 / 2.0,
        "the forehead is in the upper half: {decal:?}"
    );
    let mut darker = 0;
    for (i, (a, b)) in plain.iter().zip(&marked).enumerate() {
        if inside(decal, i) {
            if luma(*b) + 20.0 < luma(*a) {
                darker += 1;
            }
            continue;
        }
        let same = a.iter().zip(b).all(|(x, y)| x.abs_diff(*y) <= 1);
        assert!(
            same,
            "pixel {i} outside the forehead changed: {a:?} -> {b:?}"
        );
    }
    let (cx, cy) = rig.decal_centre(0.0);
    let centre = (cy * SIZE + cx) as usize;
    assert!(
        plain[centre][3] == 255,
        "the decal's centre is on the model"
    );
    assert!(
        luma(marked[centre]) + 30.0 < luma(plain[centre]),
        "{:?} vs {:?}",
        marked[centre],
        plain[centre]
    );
    assert!(darker >= 9, "only {darker} pixels engraved");
    // Turned, the decal goes with the face.
    let turned_plain = rig.draw(
        &Look {
            yaw: 0.35,
            glyph: false,
            ..look()
        },
        &glyph(false),
    );
    let turned = rig.draw(
        &Look {
            yaw: 0.35,
            ..look()
        },
        &glyph(false),
    );
    let (tx, ty) = rig.decal_centre(0.35);
    assert!(tx > cx, "the face turned right");
    let t = (ty * SIZE + tx) as usize;
    assert!(luma(turned[t]) + 30.0 < luma(turned_plain[t]));
}

#[test]
fn another_glyph_marks_the_forehead_differently() {
    let Some(mut rig) = Rig::new() else { return };
    let disc = rig.draw(&look(), &glyph(false));
    let ring = rig.draw(&look(), &glyph(true));
    let (cx, cy) = rig.decal_centre(0.0);
    let centre = (cy * SIZE + cx) as usize;
    // The ring's middle is empty: no groove there.
    assert!(
        luma(ring[centre]) > luma(disc[centre]) + 30.0,
        "{:?} vs {:?}",
        ring[centre],
        disc[centre]
    );
}

#[test]
fn a_locked_skull_is_a_dark_shape_with_no_glyph() {
    let Some(mut rig) = Rig::new() else { return };
    let lit = rig.draw(
        &Look {
            glyph: false,
            ..look()
        },
        &glyph(false),
    );
    let dark = rig.draw(
        &Look {
            silhouette: true,
            ..look()
        },
        &glyph(false),
    );
    let dark_no_glyph = rig.draw(
        &Look {
            silhouette: true,
            glyph: false,
            ..look()
        },
        &glyph(false),
    );
    assert_eq!(dark, dark_no_glyph, "a silhouette never draws the glyph");
    let ceiling = ((SILHOUETTE_FILL + SILHOUETTE_RIM) * 255.0).ceil() as u8 + 1;
    let mut drawn = 0;
    for (d, l) in dark.iter().zip(&lit) {
        assert_eq!(d[3], l[3], "the same shape");
        if d[3] > 0 {
            drawn += 1;
            assert!(
                d[..3].iter().all(|c| *c <= ceiling),
                "{d:?} brighter than the silhouette's {ceiling}"
            );
        } else {
            assert_eq!(*d, [0; 4], "the background stays clear");
        }
    }
    // The framing fits the bounds' sphere (radius sqrt 3 for the unit
    // sphere's box), so the sphere itself is a disc of the radius its top
    // projects to: about 17 pixels, not a quarter of the picture.
    let p = rig.showcase.placement().unwrap();
    let centre = p.project([0.0; 3], 0.0, SIZE, SIZE).unwrap();
    let top = p.project([0.0, 0.0, 1.0], 0.0, SIZE, SIZE).unwrap();
    let radius = centre[1] - top[1];
    let disc = std::f32::consts::PI * radius * radius;
    assert!(
        (0.85 * disc..1.25 * disc).contains(&(drawn as f32)),
        "the silhouette is the sphere's disc ({disc:.0} pixels): {drawn}"
    );
    // The lit picture is brighter at the decal's centre than any
    // silhouette pixel can be.
    let (cx, cy) = rig.decal_centre(0.0);
    assert!(
        lit[(cy * SIZE + cx) as usize][..3]
            .iter()
            .any(|c| *c > ceiling)
    );
}

#[test]
fn the_tint_colours_the_bone() {
    let Some(mut rig) = Rig::new() else { return };
    let mean = |pixels: &[[u8; 4]]| {
        let drawn: Vec<_> = pixels.iter().filter(|p| p[3] > 0).collect();
        let n = drawn.len() as f32;
        [0, 1, 2].map(|k| drawn.iter().map(|p| f32::from(p[k])).sum::<f32>() / n)
    };
    let gold = mean(&rig.draw(
        &Look {
            tint: [1.0, 0.78, 0.36],
            glyph: false,
            ..look()
        },
        &glyph(false),
    ));
    let silver = mean(&rig.draw(
        &Look {
            tint: [0.82, 0.86, 0.94],
            glyph: false,
            ..look()
        },
        &glyph(false),
    ));
    assert!(gold[0] > gold[2] + 30.0, "gold {gold:?}");
    assert!(silver[2] >= silver[0], "silver {silver:?}");
}

#[test]
fn the_composite_lands_in_its_rectangle_only() {
    let Some(mut rig) = Rig::new() else { return };
    rig.draw(&look(), &glyph(false));
    let ctx = &rig.ctx;
    let target = ctx.device.create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: SIZE,
            height: SIZE,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = target.create_view(&Default::default());
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    drop(enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: None,
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: 0.0,
                    g: 0.0,
                    b: 1.0,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    }));
    // Half of the rectangle runs past the right edge.
    let rect = [40.0, 8.0, 48.0, 32.0];
    assert!(rig.showcase.composite(
        &ctx.device,
        &mut enc,
        &view,
        wgpu::TextureFormat::Rgba8Unorm,
        rect,
        1.0
    ));
    ctx.queue.submit([enc.finish()]);
    let out = read(ctx, &target);
    let mut changed = 0;
    for (i, p) in out.iter().enumerate() {
        let (x, y) = ((i as u32 % SIZE) as f32, (i as u32 / SIZE) as f32);
        let within = x >= rect[0] && y >= rect[1] && y < rect[1] + rect[3];
        if !within {
            assert_eq!(
                *p,
                [0, 0, 255, 255],
                "pixel ({x}, {y}) outside the rectangle"
            );
        } else if *p != [0, 0, 255, 255] {
            changed += 1;
        }
    }
    assert!(changed > 50, "the picture was drawn: {changed}");
}

#[test]
fn each_part_draws_its_own_base_map_and_one_without_takes_the_tint() {
    let Some(mut rig) = Rig::new() else { return };
    let white = Look {
        tint: [1.0, 1.0, 1.0],
        glyph: false,
        ..look()
    };
    let plain = rig.draw(&white, &glyph(false));
    let red = Image {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 255],
        mips: Vec::new(),
        blocks: None,
    };
    let ctx = &rig.ctx;
    rig.showcase
        .set_mesh(&ctx.device, &ctx.queue, &sphere(), &[(1, red)]);
    let mixed = rig.draw(&white, &glyph(false));
    let p = rig.showcase.placement().unwrap();
    let at = |z: f32| {
        let [x, y] = p
            .project([(1.0 - z * z).sqrt(), 0.0, z], 0.0, SIZE, SIZE)
            .unwrap();
        (y as u32 * SIZE + x as u32) as usize
    };
    let (upper, lower) = (at(0.4), at(-0.4));
    assert_eq!(
        mixed[upper], plain[upper],
        "the part with no base map draws as white bone under the tint"
    );
    let [r, g, b, a] = plain[upper];
    assert!(
        a == 255 && r.max(g).max(b) - r.min(g).min(b) < 48 && r > 40,
        "white bone under the (slightly coloured) lights: {:?}",
        plain[upper]
    );
    let [r, g, b, a] = mixed[lower];
    assert!(
        a == 255 && r > 40 && u16::from(r) > 3 * u16::from(g.max(b)),
        "the lower part takes material 1's red map: {:?}",
        mixed[lower]
    );
}
