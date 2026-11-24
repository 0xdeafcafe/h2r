//! The overlay stage's plan (docs/graphics/retire-d3d.md, "What replaces
//! each subsystem"; `frame_scene::overlay`): the scene's ordered list of
//! quads, text runs and fills turned into one vertex and index list and
//! the batches that draw it, in the list's order. Pure: `ui_gpu`'s
//! `OverlayGpu` records it.
//!
//! Consecutive items with the same material key and clip draw as one
//! batch; nothing is reordered, so blended items keep the guest's order.
//! An item the renderer cannot draw is dropped and counted by its reason
//! ([`OverlayStats`]): the overlay has no guest draw to fall back to.

use frame_scene::{ClipRect, Overlay, OverlayKind, OverlayMaterial, Program};

/// The six corners a quad draws as two triangles, as indices into its four
/// (the fan `0, 1, 2` and `0, 2, 3`).
pub const QUAD_INDICES: [u32; 6] = [0, 1, 2, 0, 2, 3];

/// Where a glyph is in the renderer's atlas of one font.
pub trait GlyphAtlas {
    /// Glyph `index` of engine font `font`: `[u0, v0, u1, v1]`, normalised;
    /// `None` when the atlas does not hold it.
    fn glyph(&self, font: u16, index: u32) -> Option<[f32; 4]>;
}

/// An atlas that holds nothing: every text run is refused.
pub struct NoGlyphs;

impl GlyphAtlas for NoGlyphs {
    fn glyph(&self, _font: u16, _index: u32) -> Option<[f32; 4]> {
        None
    }
}

/// One corner as the program takes it: clip space, the coordinate, RGBA.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PlanVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
}

/// A scissor in the renderer's target pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Scissor {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// One draw: a run of consecutive items with one material and clip.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OverlayBatch {
    pub material: OverlayMaterial,
    pub clip: ClipRect,
    pub scissor: Scissor,
    /// Into [`OverlayPlan::indices`].
    pub first_index: u32,
    pub index_count: u32,
    /// The overlay's items it draws.
    pub items: u32,
}

/// What the plan and the draw did with a list: the `native_ui_overlay_*`
/// counters of `render.stats`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayStats {
    pub items: u32,
    pub quads: u32,
    pub text_runs: u32,
    pub fills: u32,
    pub glyphs: u32,
    /// Draws recorded (one per batch the GPU took).
    pub batches: u32,
    /// Items whose material is a tag shader: not translated yet.
    pub refused_material: u32,
    /// Items whose clip does not reach the renderer's target.
    pub refused_clip: u32,
    /// Text runs with a glyph the atlas does not hold.
    pub refused_glyph: u32,
    /// Batches whose texture the renderer does not hold, or whose blend
    /// it cannot take (`ui_gpu`).
    pub refused_texture: u32,
}

impl OverlayStats {
    /// Items drawn.
    pub fn drawn(&self) -> u32 {
        self.items - self.refused_material - self.refused_clip - self.refused_glyph
    }
}

/// The list as the GPU draws it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OverlayPlan {
    /// The renderer's target: width, height.
    pub target: [u32; 2],
    pub vertices: Vec<PlanVertex>,
    pub indices: Vec<u32>,
    pub batches: Vec<OverlayBatch>,
    pub stats: OverlayStats,
}

/// `clip` (guest pixels of a `guest` target) in a `host` target's pixels:
/// each edge scaled and rounded, so an edge on a guest pixel's boundary
/// lands on the host pixel boundary nearest it; `None` when nothing is
/// left.
pub fn scissor(clip: &ClipRect, guest: [u32; 2], host: [u32; 2]) -> Option<Scissor> {
    if guest[0] == 0 || guest[1] == 0 {
        return None;
    }
    let edge = |v: u32, axis: usize| {
        let scaled = (f64::from(v.min(guest[axis])) * f64::from(host[axis]) / f64::from(guest[axis])).round();
        (scaled as u32).min(host[axis])
    };
    let (x0, y0) = (edge(clip.x, 0), edge(clip.y, 1));
    let (x1, y1) = (edge(clip.x.saturating_add(clip.width), 0), edge(clip.y.saturating_add(clip.height), 1));
    (x1 > x0 && y1 > y0).then(|| Scissor { x: x0, y: y0, width: x1 - x0, height: y1 - y0 })
}

/// A guest pixel position in clip space (`y` up).
fn to_clip(position: [f32; 2], guest: [u32; 2]) -> [f32; 2] {
    [
        2.0 * position[0] / guest[0] as f32 - 1.0,
        1.0 - 2.0 * position[1] / guest[1] as f32,
    ]
}

impl OverlayPlan {
    /// The plan for `overlay` drawn into a `target` of the renderer's own
    /// size. An empty overlay, or one with no target size, plans nothing.
    pub fn new(overlay: &Overlay, target: [u32; 2], atlas: &dyn GlyphAtlas) -> OverlayPlan {
        let mut plan = OverlayPlan { target, ..OverlayPlan::default() };
        let guest = overlay.size;
        let counts = overlay.counts();
        plan.stats.items = counts.items;
        plan.stats.quads = counts.quads;
        plan.stats.text_runs = counts.text_runs;
        plan.stats.fills = counts.fills;
        plan.stats.glyphs = counts.glyphs;
        for item in &overlay.items {
            if matches!(item.material.program, Program::Shader { .. }) {
                plan.stats.refused_material += 1;
                continue;
            }
            let Some(scissor) = scissor(&item.clip, guest, target) else {
                plan.stats.refused_clip += 1;
                continue;
            };
            let base = plan.vertices.len() as u32;
            let start = plan.indices.len() as u32;
            match item.kind {
                OverlayKind::Quad | OverlayKind::Fill => {
                    let corners = overlay.vertices_of(item);
                    plan.vertices.extend(corners.iter().map(|v| PlanVertex {
                        position: to_clip(v.position, guest),
                        uv: v.uv,
                        color: v.color,
                    }));
                    plan.indices.extend(QUAD_INDICES.map(|i| base + i));
                }
                OverlayKind::TextRun => {
                    let font = match item.material.texture {
                        frame_scene::OverlayTexture::Glyphs { font } => font,
                        _ => {
                            plan.stats.refused_glyph += 1;
                            continue;
                        }
                    };
                    let glyphs = overlay.glyphs_of(item);
                    let Some(uvs) = glyphs.iter().map(|g| atlas.glyph(font, g.index)).collect::<Option<Vec<_>>>() else {
                        plan.stats.refused_glyph += 1;
                        continue;
                    };
                    for (n, (g, [u0, v0, u1, v1])) in glyphs.iter().zip(uvs).enumerate() {
                        let [x0, y0, x1, y1] = g.rect;
                        let corner = |x, y, u, v| PlanVertex {
                            position: to_clip([x, y], guest),
                            uv: [u, v],
                            color: item.color,
                        };
                        plan.vertices.extend([
                            corner(x0, y0, u0, v0),
                            corner(x1, y0, u1, v0),
                            corner(x1, y1, u1, v1),
                            corner(x0, y1, u0, v1),
                        ]);
                        let first = base + 4 * n as u32;
                        plan.indices.extend(QUAD_INDICES.map(|i| first + i));
                    }
                }
            }
            let count = plan.indices.len() as u32 - start;
            let joins = plan.batches.last().is_some_and(|b| {
                b.material == item.material && b.clip == item.clip && b.first_index + b.index_count == start
            });
            if let Some(b) = plan.batches.last_mut().filter(|_| joins) {
                b.index_count += count;
                b.items += 1;
            } else {
                plan.batches.push(OverlayBatch {
                    material: item.material,
                    clip: item.clip,
                    scissor,
                    first_index: start,
                    index_count: count,
                    items: 1,
                });
            }
        }
        plan
    }

    pub fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{Combiner, OverlayBlend, OverlayGlyph, OverlayMaterial, OverlayTexture, OverlayVertex, Sampler};

    struct Atlas;

    impl GlyphAtlas for Atlas {
        fn glyph(&self, font: u16, index: u32) -> Option<[f32; 4]> {
            (font == 2 && index < 4).then(|| {
                let u = index as f32 / 4.0;
                [u, 0.0, u + 0.25, 1.0]
            })
        }
    }

    fn sprite(tag: u32) -> OverlayMaterial {
        OverlayMaterial {
            program: Program::Fixed(Combiner::Texture),
            texture: OverlayTexture::Bitmap { tag, image: 0 },
            sampler: Sampler::default(),
            blend: OverlayBlend::ALPHA,
        }
    }

    fn quad(x: f32) -> [OverlayVertex; 4] {
        let c = |x, y, u, v| OverlayVertex { position: [x, y], uv: [u, v], color: [1.0; 4] };
        [c(x, 0.0, 0.0, 0.0), c(x + 4.0, 0.0, 1.0, 0.0), c(x + 4.0, 4.0, 1.0, 1.0), c(x, 4.0, 0.0, 1.0)]
    }

    fn all() -> ClipRect {
        ClipRect::full([640, 480])
    }

    #[test]
    fn consecutive_items_of_one_material_share_a_batch_and_the_order_holds() {
        let mut o = Overlay::new([640, 480]);
        o.push_quad(sprite(1), all(), quad(0.0)).unwrap();
        o.push_quad(sprite(1), all(), quad(8.0)).unwrap();
        o.push_fill(all(), [0.0, 0.0, 4.0, 4.0], [0.0, 0.0, 0.0, 1.0], OverlayBlend::Off).unwrap();
        o.push_quad(sprite(1), all(), quad(16.0)).unwrap();
        let p = OverlayPlan::new(&o, [1280, 960], &NoGlyphs);
        let runs: Vec<_> = p.batches.iter().map(|b| (b.items, b.first_index, b.index_count)).collect();
        assert_eq!(runs, [(2, 0, 12), (1, 12, 6), (1, 18, 6)], "the sprite after the fill is not merged back");
        assert_eq!(&p.indices[6..12], &[4, 5, 6, 4, 6, 7]);
        assert_eq!(p.vertices.len(), 16);
        assert_eq!(p.stats.batches, 0, "the GPU counts what it records");
        assert_eq!(p.stats.drawn(), 4);
    }

    #[test]
    fn a_new_clip_starts_a_batch() {
        let mut o = Overlay::new([640, 480]);
        o.push_quad(sprite(1), all(), quad(0.0)).unwrap();
        let half = ClipRect { x: 0, y: 0, width: 320, height: 480 };
        o.push_quad(sprite(1), half, quad(8.0)).unwrap();
        let p = OverlayPlan::new(&o, [640, 480], &NoGlyphs);
        assert_eq!(p.batches.len(), 2);
        assert_eq!(p.batches[1].scissor, Scissor { x: 0, y: 0, width: 320, height: 480 });
    }

    #[test]
    fn guest_pixel_edges_map_to_clip_space_corners() {
        let mut o = Overlay::new([640, 480]);
        o.push_fill(all(), [0.0, 0.0, 640.0, 480.0], [1.0; 4], OverlayBlend::Off).unwrap();
        let p = OverlayPlan::new(&o, [1920, 1440], &NoGlyphs);
        let corners: Vec<_> = p.vertices.iter().map(|v| v.position).collect();
        assert_eq!(corners, [[-1.0, 1.0], [1.0, 1.0], [1.0, -1.0], [-1.0, -1.0]]);
        assert_eq!(p.batches[0].scissor, Scissor { x: 0, y: 0, width: 1920, height: 1440 });
    }

    #[test]
    fn scissors_round_each_edge_and_clip_at_the_target() {
        let clip = ClipRect { x: 631, y: 11, width: 13, height: 9 };
        assert_eq!(scissor(&clip, [640, 480], [1440, 720]), Some(Scissor { x: 1420, y: 17, width: 20, height: 13 }));
        let off = ClipRect { x: 640, y: 0, width: 4, height: 4 };
        assert_eq!(scissor(&off, [640, 480], [1280, 960]), None);
        assert_eq!(scissor(&clip, [0, 480], [1280, 960]), None);
    }

    #[test]
    fn a_text_run_is_one_draw_of_its_glyphs_from_the_atlas() {
        let mut o = Overlay::new([640, 480]);
        let glyphs = [
            OverlayGlyph { index: 1, rect: [10.0, 20.0, 16.0, 30.0] },
            OverlayGlyph { index: 3, rect: [16.0, 20.0, 22.0, 30.0] },
        ];
        o.push_text_run(OverlayMaterial::text(2), all(), [1.0, 0.0, 0.0, 0.5], &glyphs).unwrap();
        let p = OverlayPlan::new(&o, [640, 480], &Atlas);
        assert_eq!(p.batches.len(), 1);
        assert_eq!(p.batches[0].index_count, 12);
        assert_eq!(p.vertices[4].uv, [0.75, 0.0], "the second glyph's top left");
        assert_eq!(p.vertices[6].uv, [1.0, 1.0]);
        assert!(p.vertices.iter().all(|v| v.color == [1.0, 0.0, 0.0, 0.5]));
        assert_eq!(p.stats.glyphs, 2);
    }

    #[test]
    fn each_dropped_item_is_counted_by_its_reason() {
        let mut o = Overlay::new([640, 480]);
        let mut shader = sprite(1);
        shader.program = Program::Shader { tag: 0xe010_0020 };
        o.push_quad(shader, all(), quad(0.0)).unwrap();
        let unknown = [OverlayGlyph { index: 9, rect: [0.0, 0.0, 4.0, 4.0] }];
        o.push_text_run(OverlayMaterial::text(2), all(), [1.0; 4], &unknown).unwrap();
        o.push_quad(sprite(1), all(), quad(8.0)).unwrap();
        let p = OverlayPlan::new(&o, [640, 480], &Atlas);
        assert_eq!((p.stats.refused_material, p.stats.refused_glyph, p.stats.refused_clip), (1, 1, 0));
        assert_eq!(p.batches.len(), 1);
        assert_eq!(p.batches[0].first_index, 0);
        assert_eq!(p.indices[..6], [0, 1, 2, 0, 2, 3], "the drawn quad's corners start the list");
        assert_eq!(p.stats.drawn(), 1);
        let none = OverlayPlan::new(&Overlay::default(), [640, 480], &Atlas);
        assert!(none.is_empty() && none.stats == OverlayStats::default());
    }
}
