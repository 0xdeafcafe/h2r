//! The overlay stage's plan (docs/graphics/retire-d3d.md, "What replaces
//! each subsystem"; `frame_scene::overlay`): the scene's ordered list of
//! quads, text runs and fills turned into one vertex and index list and
//! the batches that draw it, in the list's order, with the blurs between
//! them ([`PlanBlur`]). Pure: `ui_gpu`'s `OverlayGpu` records it.
//!
//! Consecutive items with the same material key and clip draw as one
//! batch; nothing is reordered, so blended items keep the guest's order.
//! A blur ends the batch before it: it reads what was drawn. A movie
//! frame draws as a quad over its movie's upload
//! (`frame_scene::OverlayTexture::Movie`). A hold ends the batch before
//! it too: it copies what was drawn into its held texture
//! ([`PlanHold`]). A model view is planned for the object stage with its
//! camera and scissor (`crate::model_view::PlanModelView`), in list order
//! as the blurs; until that stage draws one, each is counted refused.
//! An item the renderer cannot draw is dropped and counted by its reason
//! ([`OverlayStats`]): the overlay has no guest draw to fall back to.

use frame_scene::{ClipRect, Overlay, OverlayKind, OverlayMaterial, Program};

/// A blur between batches: the target as the batches before it left it,
/// blurred step by step, written back inside the scissor.
#[derive(Clone, Debug, PartialEq)]
pub struct PlanBlur {
    /// The batches drawn before it (an index into
    /// [`OverlayPlan::batches`]).
    pub before: u32,
    pub scissor: Scissor,
    /// Each step's tap offset, a fraction of the target in x and y: the
    /// spread times the tap over the guest target's size, so the same
    /// at any scale.
    pub offsets: Vec<[f32; 2]>,
    /// The steps' combiner constant (`frame_scene::OverlayBlur::pick`).
    pub pick: f32,
}

/// A hold between batches: the target as the batches before it left it,
/// copied whole into the renderer's held texture `hold`
/// (`frame_scene::OverlayKind::Hold`). A hold and a blur at the same
/// point run the hold first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PlanHold {
    /// The batches drawn before it (an index into
    /// [`OverlayPlan::batches`]).
    pub before: u32,
    pub hold: u32,
}

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

/// A glyph's texels in a page: its rectangle, in texels from the page's
/// top left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GlyphRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// The renderer's glyph atlas: one square page per engine font, each glyph
/// rasterised into it once (its coverage in alpha, what
/// `Combiner::TextureAlpha` samples, under white or, for a glyph with
/// colours of its own, its texels' colours), packed in shelves left to right and
/// top to bottom with a texel of space around it. Pages only grow: a glyph
/// keeps its place, so a list planned before a glyph was added still
/// draws. [`GlyphPages::page`]'s version says when a page's texels changed
/// and [`GlyphPages::added_since`] names the glyphs that changed it, so
/// the residency writes each once. Whoever fills the pages decodes a glyph
/// only when [`GlyphPages::has`] says it is not in one.
#[derive(Clone, Debug)]
pub struct GlyphPages {
    size: u32,
    pages: std::collections::BTreeMap<u16, GlyphPage>,
}

#[derive(Clone, Debug)]
struct GlyphPage {
    rgba: Vec<u8>,
    glyphs: std::collections::HashMap<u32, [f32; 4]>,
    /// The open shelf: its left edge so far, its top, its height.
    x: u32,
    y: u32,
    height: u32,
    version: u64,
    /// Each glyph with texels, in the order added: the version it made
    /// and its rectangle. A glyph without texels is in none.
    added: Vec<(u64, GlyphRect)>,
}

impl GlyphPages {
    /// The side of a page in texels.
    pub const PAGE: u32 = 1024;

    /// Pages `size` texels square.
    pub fn new(size: u32) -> GlyphPages {
        GlyphPages { size, pages: std::collections::BTreeMap::new() }
    }

    /// The side of a page in texels, as `new` was given.
    pub fn side(&self) -> u32 {
        self.size
    }

    /// Whether glyph `index` of font `font` is in its page.
    pub fn has(&self, font: u16, index: u32) -> bool {
        self.pages.get(&font).is_some_and(|p| p.glyphs.contains_key(&index))
    }

    /// Adds glyph `index` of font `font`, `width` by `height` with one
    /// coverage byte per texel (row-major, 0 to 255): true when it is in
    /// the page, as it already was or now; false when the coverage is not
    /// `width * height` bytes or the page has no room left.
    pub fn insert(&mut self, font: u16, index: u32, width: u16, height: u16, alpha: &[u8]) -> bool {
        if alpha.len() != usize::from(width) * usize::from(height) {
            return false;
        }
        let rgba: Vec<u8> = alpha.iter().flat_map(|&a| [255, 255, 255, a]).collect();
        self.insert_rgba(font, index, width, height, &rgba)
    }

    /// As [`GlyphPages::insert`] for a glyph with colours of its own (a
    /// button, a medal): four bytes per texel, red, green, blue and its
    /// coverage. False when `rgba` is not `width * height * 4` bytes.
    pub fn insert_rgba(&mut self, font: u16, index: u32, width: u16, height: u16, rgba: &[u8]) -> bool {
        let size = self.size;
        let (w, h) = (u32::from(width), u32::from(height));
        if rgba.len() != (w * h * 4) as usize {
            return false;
        }
        let page = self.pages.entry(font).or_insert_with(|| GlyphPage {
            rgba: vec![0; (size * size * 4) as usize],
            glyphs: std::collections::HashMap::new(),
            x: 0,
            y: 0,
            height: 0,
            version: 0,
            added: Vec::new(),
        });
        if page.glyphs.contains_key(&index) {
            return true;
        }
        if w == 0 || h == 0 {
            page.glyphs.insert(index, [0.0; 4]);
            return true;
        }
        // One texel of space on every side.
        let (pw, ph) = (w + 2, h + 2);
        if pw > size || ph > size {
            return false;
        }
        if page.x + pw > size {
            page.y += page.height;
            page.x = 0;
            page.height = 0;
        }
        if page.y + ph > size {
            return false;
        }
        let (x0, y0) = (page.x + 1, page.y + 1);
        for row in 0..h {
            for col in 0..w {
                let at = (((y0 + row) * size + x0 + col) * 4) as usize;
                let from = ((row * w + col) * 4) as usize;
                page.rgba[at..at + 4].copy_from_slice(&rgba[from..from + 4]);
            }
        }
        let s = size as f32;
        page.glyphs.insert(
            index,
            [x0 as f32 / s, y0 as f32 / s, (x0 + w) as f32 / s, (y0 + h) as f32 / s],
        );
        page.x += pw;
        page.height = page.height.max(ph);
        page.version += 1;
        page.added.push((page.version, GlyphRect { x: x0, y: y0, width: w, height: h }));
        true
    }

    /// Font `font`'s page: its side, its RGBA texels and its version (how
    /// many glyphs changed it).
    pub fn page(&self, font: u16) -> Option<(u32, &[u8], u64)> {
        self.pages.get(&font).map(|p| (self.size, p.rgba.as_slice(), p.version))
    }

    /// The glyphs font `font`'s page gained after `version`, in the order
    /// added: what a copy of the page as of `version` lacks. Each is the
    /// version it made and its rectangle in the page, whose side and RGBA
    /// texels come first. `None` when the font has no page.
    pub fn added_since(&self, font: u16, version: u64) -> Option<(u32, &[u8], &[(u64, GlyphRect)])> {
        let page = self.pages.get(&font)?;
        let first = page.added.partition_point(|(made, _)| *made <= version);
        Some((self.size, page.rgba.as_slice(), &page.added[first..]))
    }

    /// The fonts with a page, in order.
    pub fn fonts(&self) -> impl Iterator<Item = u16> + '_ {
        self.pages.keys().copied()
    }
}

impl Default for GlyphPages {
    fn default() -> GlyphPages {
        GlyphPages::new(GlyphPages::PAGE)
    }
}

impl GlyphAtlas for GlyphPages {
    fn glyph(&self, font: u16, index: u32) -> Option<[f32; 4]> {
        self.pages.get(&font)?.glyphs.get(&index).copied()
    }
}

/// A bitmap image an overlay names, decoded on the host: its size and its
/// RGBA texels, straight alpha.
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayImage {
    pub width: u32,
    pub height: u32,
    pub rgba: std::sync::Arc<[u8]>,
}

/// A movie's current frame on the host, as its decoder left it: `width`
/// by `height` texels, four bytes each in B, G, R, A order, row-major.
/// `ui_gpu::OverlayResidency::sync_movie` uploads it when `frame` moves;
/// a `frame_scene::OverlayTexture::Movie` names it by `movie`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MovieImage {
    pub movie: u32,
    pub frame: u32,
    pub width: u32,
    pub height: u32,
    pub bgra: std::sync::Arc<[u8]>,
}

/// What an overlay's textures are made from on the host: the bitmap
/// images its quads name, by (bitmap datum, image index), and the glyph
/// pages its text runs sample. `ui_gpu::OverlayResidency` uploads them,
/// each image and each glyph once. A datum names another bitmap on
/// another map: a new `generation` (one per map) makes the residency drop
/// what it holds.
#[derive(Clone, Debug, Default)]
pub struct OverlayAssets {
    pub generation: u64,
    pub images: std::collections::HashMap<(u32, u16), OverlayImage>,
    pub glyphs: GlyphPages,
}

/// One corner as the program takes it: clip space, the coordinate, RGBA.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct PlanVertex {
    pub position: [f32; 2],
    pub uv: [f32; 2],
    pub color: [f32; 4],
    pub extra_uv: [[f32; 2]; 3],
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
#[derive(Clone, Debug, PartialEq)]
pub struct OverlayBatch {
    pub shader_stages: [Option<frame_scene::OverlayTexture>; 4],
    /// Stages that reconstruct magnified glyph coverage at this target size.
    pub glyph_cache: u8,
    pub alpha_test: Option<(u32, u8)>,
    pub alpha_kill: u8,
    pub color_write: u32,
    pub shader_words: Option<Box<[u32]>>,
    /// The blend's constant colour, RGBA (`OverlayItem::blend_constant`).
    pub blend_constant: [f32; 4],
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
    pub blurs: u32,
    pub model_views: u32,
    /// Movie frames: drawn as quads over their movie's upload.
    pub movie_frames: u32,
    /// Draws recorded (one per batch the GPU took).
    pub batches: u32,
    /// Items whose tag shader has no supported definition.
    pub refused_material: u32,
    /// Items whose clip does not reach the renderer's target.
    pub refused_clip: u32,
    /// Text runs with a glyph the atlas does not hold.
    pub refused_glyph: u32,
    /// Batches whose texture the renderer does not hold, or whose blend
    /// it cannot take (`ui_gpu`).
    pub refused_texture: u32,
    /// Movie frames whose movie the renderer holds no upload of
    /// (`ui_gpu`).
    pub refused_movie: u32,
    /// Holds: copies of the target into a held texture.
    pub holds: u32,
    /// Holds over a target the GPU cannot copy, or with no held texture
    /// of the target's size and format (`ui_gpu`).
    pub refused_hold: u32,
    /// Blurs over a target the GPU cannot copy or sample (`ui_gpu`).
    pub refused_blur: u32,
}

impl OverlayStats {
    /// Items the plan kept.
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
    /// In list order; each between the batches its `before` names.
    pub blurs: Vec<PlanBlur>,
    /// In list order, as the blurs.
    pub holds: Vec<PlanHold>,
    /// In list order, as the blurs.
    pub model_views: Vec<crate::model_view::PlanModelView>,
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
        plan.stats.blurs = counts.blurs;
        plan.stats.model_views = counts.model_views;
        plan.stats.movie_frames = counts.movie_frames;
        plan.stats.holds = counts.holds;
        for item in &overlay.items {
            if item.kind == OverlayKind::ModelView {
                let before = plan.batches.len() as u32;
                let planned = overlay.model_view_of(item).ok_or(crate::model_view::Refusal::Camera)
                    .and_then(|view| crate::model_view::plan(view, &item.clip, guest, target, before, plan.blurs.len() as u32));
                match planned {
                    Ok(view) => plan.model_views.push(view),
                    Err(why) => unimplemented!("overlay model view: {}", why.name()),
                }
                continue;
            }
            if item.kind == OverlayKind::Hold {
                match overlay.hold_of(item) {
                    Some(hold) => plan.holds.push(PlanHold { before: plan.batches.len() as u32, hold }),
                    None => plan.stats.refused_hold += 1,
                }
                continue;
            }
            if matches!(item.material.program, Program::Shader { .. })
                && !item.shader_words.as_ref().is_some_and(|words| words.len() == 60)
            {
                plan.stats.refused_material += 1;
                continue;
            }
            let Some(scissor) = scissor(&item.clip, guest, target) else {
                plan.stats.refused_clip += 1;
                continue;
            };
            if item.kind == OverlayKind::Blur {
                let Some(blur) = overlay.blur_of(item) else {
                    plan.stats.refused_clip += 1;
                    continue;
                };
                let (w, h) = (guest[0] as f32, guest[1] as f32);
                plan.blurs.push(PlanBlur {
                    before: plan.batches.len() as u32,
                    scissor,
                    offsets: blur.spreads.iter().map(|s| [s * blur.tap / w, s * blur.tap / h]).collect(),
                    pick: blur.pick,
                });
                continue;
            }
            let base = plan.vertices.len() as u32;
            let start = plan.indices.len() as u32;
            match item.kind {
                OverlayKind::Quad | OverlayKind::Fill | OverlayKind::MovieFrame => {
                    let corners = overlay.vertices_of(item);
                    plan.vertices.extend(corners.iter().enumerate().map(|(n, v)| {
                        let [uv, t1, t2, t3] = item.stage_uv.as_ref().map_or([v.uv; 4], |uv| uv[n]);
                        PlanVertex {
                            position: to_clip(v.position, guest), uv, color: v.color,
                            extra_uv: [t1, t2, t3],
                        }
                    }));
                    plan.indices.extend(QUAD_INDICES.map(|i| base + i));
                }
                OverlayKind::Blur | OverlayKind::ModelView | OverlayKind::Hold => unreachable!("planned above"),
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
                            extra_uv: [[0.0; 2]; 3],
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
            let after_blur = plan.blurs.last().is_some_and(|b| b.before as usize == plan.batches.len())
                || plan.holds.last().is_some_and(|h| h.before as usize == plan.batches.len())
                || plan.model_views.last().is_some_and(|m| m.before as usize == plan.batches.len());
            let magnified = target[0] > guest[0] || target[1] > guest[1];
            let glyph_cache = if magnified { item.glyph_cache & 15 } else { 0 };
            let joins = !after_blur && plan.batches.last().is_some_and(|b| {
                b.material == item.material && b.color_write == item.color_write && b.glyph_cache == glyph_cache
                    && b.alpha_test == item.alpha_test && b.alpha_kill == item.alpha_kill
                    && b.shader_stages == item.shader_stages
                    && b.shader_words == item.shader_words && b.blend_constant == item.blend_constant
                    && b.clip == item.clip && b.first_index + b.index_count == start
            });
            if let Some(b) = plan.batches.last_mut().filter(|_| joins) {
                b.index_count += count;
                b.items += 1;
            } else {
                plan.batches.push(OverlayBatch {
                    material: item.material,
                    shader_stages: item.shader_stages,
                    shader_words: item.shader_words.clone(),
                    blend_constant: item.blend_constant,
                    color_write: item.color_write,
                    glyph_cache,
                    alpha_test: item.alpha_test,
                    alpha_kill: item.alpha_kill,
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
        self.batches.is_empty() && self.blurs.is_empty() && self.holds.is_empty()
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
    fn distinct_shader_constants_split_widget_batches() {
        let mut overlay = Overlay::new([640, 480]);
        let material = OverlayMaterial { program: Program::Shader { tag: 17 }, ..sprite(5) };
        for (i, fill) in [0.2, 0.2, 0.7].into_iter().enumerate() {
            let n = overlay.push_quad(material, all(), quad(i as f32)).unwrap();
            let item = &mut overlay.items[n as usize];
            item.hud_input[0] = fill;
            item.glyph_cache = 2;
            let mut words = [0; 60];
            words[10] = (fill * 255.0) as u32;
            item.shader_words = Some(words.into());
        }
        let plan = OverlayPlan::new(&overlay, [640, 480], &NoGlyphs);
        assert_eq!(plan.batches.iter().map(|b| b.items).collect::<Vec<_>>(), [2, 1]);
        assert_eq!(plan.stats.refused_material, 0);
        assert_eq!(plan.batches[0].glyph_cache, 0);
        let magnified = OverlayPlan::new(&overlay, [720, 480], &NoGlyphs);
        assert_eq!(magnified.batches[0].glyph_cache, 2);
    }

    /// Fades of one blend but another constant colour are separate draws,
    /// each carrying its colour for the pass.
    #[test]
    fn a_new_blend_constant_starts_a_batch() {
        let mut o = Overlay::new([640, 480]);
        let blend = OverlayBlend::On { source: 0x307, dest: 0x8002, equation: 0x8006 };
        for colour in [[0.5, 0.25, 0.0, 1.0], [0.5, 0.25, 0.0, 1.0], [1.0, 0.0, 0.0, 1.0]] {
            let n = o.push_fill(all(), [0.0, 0.0, 640.0, 480.0], [1.0; 4], blend).unwrap();
            o.items[n as usize].blend_constant = colour;
        }
        let p = OverlayPlan::new(&o, [640, 480], &NoGlyphs);
        let batches: Vec<_> = p.batches.iter().map(|b| (b.items, b.blend_constant)).collect();
        assert_eq!(batches, [(2, [0.5, 0.25, 0.0, 1.0]), (1, [1.0, 0.0, 0.0, 1.0])]);
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
    fn a_glyph_with_colours_of_its_own_keeps_them_in_its_page() {
        let mut pages = GlyphPages::new(8);
        // A green texel and a half-covered red one.
        assert!(pages.insert_rgba(3, 0xe100, 2, 1, &[0, 255, 0, 255, 255, 0, 0, 119]));
        let (_, rgba, version) = pages.page(3).unwrap();
        assert_eq!(version, 1);
        assert_eq!(&rgba[(8 + 1) * 4..(8 + 3) * 4], [0, 255, 0, 255, 255, 0, 0, 119]);
        assert!(!pages.insert_rgba(3, 0xe101, 2, 1, &[0; 7]), "texels of the wrong size");
        // A plain glyph beside it is white under its coverage.
        assert!(pages.insert(3, 65, 1, 1, &[9]));
        assert_eq!(&pages.page(3).unwrap().1[(8 + 5) * 4..(8 + 6) * 4], [255, 255, 255, 9]);
    }

    /// Glyphs pack in shelves with a texel around each, keep their place
    /// as others join, and the page's texels are white with the coverage
    /// in alpha.
    #[test]
    fn glyph_pages_pack_each_glyph_once_and_keep_its_place() {
        let mut pages = GlyphPages::new(16);
        assert!(pages.insert(6, 40, 3, 2, &[10, 20, 30, 40, 50, 60]));
        assert_eq!(pages.glyph(6, 40), Some([1.0 / 16.0, 1.0 / 16.0, 4.0 / 16.0, 3.0 / 16.0]));
        let (side, rgba, version) = pages.page(6).unwrap();
        assert_eq!((side, version), (16, 1));
        let texel = |x: usize, y: usize| &rgba[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4];
        assert_eq!(texel(1, 1), [255, 255, 255, 10]);
        assert_eq!(texel(3, 2), [255, 255, 255, 60]);
        assert_eq!(texel(0, 0), [0, 0, 0, 0], "the space around it");
        assert!(pages.insert(6, 40, 3, 2, &[0; 6]), "already in");
        assert_eq!(pages.page(6).unwrap().2, 1, "unchanged");
        // 5 + 12 wide does not fit a 16 texel shelf: the next shelf.
        assert!(pages.insert(6, 41, 10, 4, &[255; 40]));
        assert_eq!(pages.glyph(6, 41), Some([1.0 / 16.0, 5.0 / 16.0, 11.0 / 16.0, 9.0 / 16.0]));
        assert_eq!(pages.glyph(6, 40), Some([1.0 / 16.0, 1.0 / 16.0, 4.0 / 16.0, 3.0 / 16.0]), "kept");
        assert!(!pages.insert(6, 42, 4, 12, &[1; 48]), "no room below");
        assert!(!pages.insert(6, 43, 2, 2, &[1; 3]), "coverage of the wrong size");
        assert!(pages.insert(6, 44, 0, 5, &[]), "an empty glyph has no texels");
        assert!(pages.insert(8, 1, 1, 1, &[7]), "another font, its own page");
        assert_eq!(pages.fonts().collect::<Vec<_>>(), [6, 8]);
        assert!(pages.has(8, 1) && !pages.has(8, 2));
        // The plan takes its coordinates from the pages.
        let mut o = Overlay::new([640, 480]);
        let g = [OverlayGlyph { index: 40, rect: [0.0, 0.0, 3.0, 2.0] }];
        o.push_text_run(OverlayMaterial::text(6), all(), [1.0; 4], &g).unwrap();
        let p = OverlayPlan::new(&o, [640, 480], &pages);
        assert_eq!(p.vertices[2].uv, [4.0 / 16.0, 3.0 / 16.0]);
    }

    /// A page names the glyphs added after a version, each where its
    /// texels are: what the residency writes to a copy of that version.
    /// A glyph already in, an empty one and one that does not fit make no
    /// version and are in none.
    #[test]
    fn a_page_names_the_glyphs_added_since_a_version() {
        let first = GlyphRect { x: 1, y: 1, width: 3, height: 2 };
        let second = GlyphRect { x: 1, y: 5, width: 10, height: 4 };
        let mut pages = GlyphPages::new(16);
        assert_eq!(pages.added_since(6, 0), None, "no page yet");
        assert!(pages.insert(6, 40, 3, 2, &[10, 20, 30, 40, 50, 60]));
        assert!(pages.insert(6, 44, 0, 5, &[]), "an empty glyph has no texels");
        let (side, rgba, added) = pages.added_since(6, 0).unwrap();
        assert_eq!((side, added), (16, &[(1u64, first)][..]));
        let (x, y) = (first.x as usize, first.y as usize);
        assert_eq!(rgba[(y * 16 + x) * 4..(y * 16 + x) * 4 + 4], [255, 255, 255, 10], "the glyph's texels");
        assert!(pages.insert(6, 41, 10, 4, &[255; 40]));
        assert!(pages.insert(6, 41, 10, 4, &[0; 40]), "already in");
        assert!(!pages.insert(6, 42, 4, 12, &[1; 48]), "no room");
        assert_eq!(pages.added_since(6, 0).unwrap().2, &[(1, first), (2, second)]);
        assert_eq!(pages.added_since(6, 1).unwrap().2, &[(2, second)]);
        assert_eq!(pages.added_since(6, 2).unwrap().2, &[] as &[(u64, GlyphRect)], "nothing after the last version");
        assert!(pages.insert(8, 1, 1, 1, &[7]), "another font, its own page");
        assert_eq!(pages.added_since(8, 0).unwrap().2, &[(1, GlyphRect { x: 1, y: 1, width: 1, height: 1 })]);
        assert_eq!(pages.added_since(7, 0), None, "no page");
    }

    #[test]
    fn a_movie_frame_draws_as_a_quad_over_its_movies_upload() {
        use frame_scene::OverlayMovie;
        let mut o = Overlay::new([640, 480]);
        o.push_quad(sprite(1), all(), quad(0.0)).unwrap();
        o.push_movie_frame(sprite(1), all(), quad(8.0), OverlayMovie { movie: 3, frame: 7 }).unwrap();
        o.push_movie_frame(sprite(1), all(), quad(16.0), OverlayMovie { movie: 3, frame: 7 }).unwrap();
        let p = OverlayPlan::new(&o, [1280, 960], &NoGlyphs);
        let batches: Vec<_> = p.batches.iter().map(|b| (b.items, b.material.texture)).collect();
        assert_eq!(
            batches,
            [(1, OverlayTexture::Bitmap { tag: 1, image: 0 }), (2, OverlayTexture::Movie { movie: 3 })],
            "the movie's frames batch apart from the bitmap's quad"
        );
        assert_eq!(p.vertices.len(), 12);
        assert_eq!(p.vertices[4].position, [8.0 / 320.0 - 1.0, 1.0], "corners as a quad's");
        assert_eq!((p.stats.movie_frames, p.stats.refused_movie, p.stats.drawn()), (2, 0, 3));
    }

    #[test]
    fn a_blur_splits_the_batches_and_carries_its_steps_as_fractions() {
        use frame_scene::{OverlayBlur, OverlayCamera, OverlayModelView};
        let mut o = Overlay::new([640, 480]);
        o.push_quad(sprite(1), all(), quad(0.0)).unwrap();
        o.push_blur(all(), OverlayBlur { tap: 0.5, spreads: vec![1.0, 1.25], pick: 0.25 }).unwrap();
        o.push_quad(sprite(1), all(), quad(8.0)).unwrap();
        let camera = OverlayCamera { forward: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0], fov_y: 1.0, planes: [0.0625, 200.0], ..OverlayCamera::default() };
        let view = OverlayModelView { camera, objects: vec![7] };
        o.push_model_view(ClipRect { x: 10, y: 10, width: 64, height: 48 }, view).unwrap();
        o.push_quad(sprite(1), all(), quad(16.0)).unwrap();
        let p = OverlayPlan::new(&o, [1280, 960], &NoGlyphs);
        assert_eq!(p.batches.len(), 3, "the quads after the blur and the model view are not merged back");
        assert_eq!((p.batches[0].items, p.batches[1].items, p.batches[2].items), (1, 1, 1));
        assert_eq!(p.blurs.len(), 1);
        assert_eq!(p.blurs[0].before, 1);
        assert_eq!(p.blurs[0].scissor, Scissor { x: 0, y: 0, width: 1280, height: 960 });
        assert_eq!(p.blurs[0].offsets, vec![[0.5 / 640.0, 0.5 / 480.0], [0.625 / 640.0, 0.625 / 480.0]]);
        assert_eq!(p.blurs[0].pick, 0.25, "the combiner's constant goes with the steps");
        assert_eq!((p.stats.blurs, p.stats.model_views), (1, 1));
        assert_eq!(p.stats.drawn(), 5);
        assert_eq!(p.model_views.len(), 1);
        assert_eq!((p.model_views[0].before, p.model_views[0].objects.as_slice()), (2, &[7][..]), "between the batches, as the blur");
        assert_eq!(p.model_views[0].scissor, Scissor { x: 20, y: 20, width: 128, height: 96 });
        assert_eq!(p.model_views[0].camera.viewport, [10, 10, 58, 74]);
        let mut blur_only = Overlay::new([640, 480]);
        blur_only.push_blur(all(), OverlayBlur { tap: 0.5, spreads: vec![1.0], pick: 0.0 }).unwrap();
        assert!(!OverlayPlan::new(&blur_only, [640, 480], &NoGlyphs).is_empty());
    }

    /// A view whose record would not be a camera stops the plan by name.
    #[test]
    #[should_panic(expected = "overlay model view: model_view_camera")]
    fn a_model_view_with_no_camera_is_unimplemented() {
        use frame_scene::{OverlayCamera, OverlayModelView};
        let mut o = Overlay::new([640, 480]);
        let camera = OverlayCamera { forward: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0], fov_y: 0.0, planes: [0.0625, 200.0], ..OverlayCamera::default() };
        o.push_model_view(ClipRect { x: 10, y: 10, width: 64, height: 48 }, OverlayModelView { camera, objects: vec![7] }).unwrap();
        OverlayPlan::new(&o, [1280, 960], &NoGlyphs);
    }

    #[test]
    fn a_hold_splits_the_batches_where_it_stands() {
        let mut o = Overlay::new([640, 480]);
        o.push_quad(sprite(1), all(), quad(0.0)).unwrap();
        o.push_hold(4).unwrap();
        o.push_quad(sprite(1), all(), quad(8.0)).unwrap();
        let p = OverlayPlan::new(&o, [1280, 960], &NoGlyphs);
        assert_eq!(p.batches.len(), 2, "the quad after the hold is not merged back");
        assert_eq!(p.holds, [PlanHold { before: 1, hold: 4 }]);
        assert_eq!((p.stats.holds, p.stats.refused_hold, p.stats.drawn()), (1, 0, 3));
        let mut hold_only = Overlay::new([640, 480]);
        hold_only.push_hold(4).unwrap();
        assert!(!OverlayPlan::new(&hold_only, [640, 480], &NoGlyphs).is_empty());
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
