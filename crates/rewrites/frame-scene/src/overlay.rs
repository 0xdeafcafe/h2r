//! The overlay stage (docs/graphics/retire-d3d.md, "What replaces each
//! subsystem"): what is drawn over a frame's views, the HUD first, as one
//! ordered list of items. Each item is a [`OverlayKind::Quad`], a
//! [`OverlayKind::TextRun`], a [`OverlayKind::Fill`], a
//! [`OverlayKind::MovieFrame`] (a movie's current frame), a
//! [`OverlayKind::Blur`] of what is under it or a
//! [`OverlayKind::ModelView`] (a menu's model widget) or a
//! [`OverlayKind::Hold`] (what is under it kept for later frames), with a material key
//! and a clip rectangle; `native_render::ui_gpu` draws the list in its
//! order. Each item names its overlay slot ([`OverlayItem::slot`]): `Ui`
//! unless its producer tags its part ([`Overlay::set_slot`]), as the
//! HUD's are tagged `Hud`. Positions are the guest target's pixels, window coordinates on
//! pixel edges (`(0, 0)` the top left corner of the top left pixel), so a
//! producer writes what the guest's screen transform would have made and
//! the renderer scales to its own target.
//!
//! Nothing here reads the guest. The blend a material names is the guest's
//! factors and equation by value, read from the running image by whoever
//! fills the list, never committed.

use std::ops::Range;

use serde::Serialize;

use crate::builder::{BuildError, SceneBuilder};
use crate::scene::FrameScene;
use crate::view_slot::ViewSlot;

/// A rectangle of the overlay's target, in guest pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct ClipRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl ClipRect {
    /// The whole of a target `size` (width, height).
    pub fn full(size: [u32; 2]) -> ClipRect {
        ClipRect { x: 0, y: 0, width: size[0], height: size[1] }
    }

    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The part of `self` inside `other`; empty when they do not meet.
    pub fn intersect(&self, other: &ClipRect) -> ClipRect {
        let x0 = self.x.max(other.x);
        let y0 = self.y.max(other.y);
        let x1 = self.x.saturating_add(self.width).min(other.x.saturating_add(other.width));
        let y1 = self.y.saturating_add(self.height).min(other.y.saturating_add(other.height));
        if x1 <= x0 || y1 <= y0 {
            return ClipRect { x: x0, y: y0, width: 0, height: 0 };
        }
        ClipRect { x: x0, y: y0, width: x1 - x0, height: y1 - y0 }
    }
}

/// How an item's pixels are blended over the target: off, or the guest's
/// D3D blend values (`D3DBLEND` factors and a `D3DBLENDOP` equation), as
/// the screen-quad begin reads them from its tables
/// (`native_render::hud::Blend`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum OverlayBlend {
    #[default]
    Off,
    On { source: u32, dest: u32, equation: u32 },
}

impl OverlayBlend {
    /// Source alpha over one minus source alpha, added: what the text
    /// batches draw with.
    pub const ALPHA: OverlayBlend = OverlayBlend::On { source: 0x302, dest: 0x303, equation: 0x8006 };
}

/// The fixed combiners the screen-quad path builds, as named programs.
/// Each takes the item's vertex colour; the textured ones its one stage.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Combiner {
    /// The vertex colour.
    Flat,
    /// The texture times the vertex colour.
    Texture,
    /// The vertex colour with its alpha times the texture's: the glyphs.
    TextureAlpha,
    /// The texture's luma, times the vertex colour (a greyed sprite).
    TextureGrey,
}

/// What draws an item's pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum Program {
    Fixed(Combiner),
    /// A tag shader, by its datum: translated, not a fixed combiner.
    Shader { tag: u32 },
}

/// The texture an item samples, by name: the renderer resolves it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub enum OverlayTexture {
    #[default]
    None,
    /// A bitmap tag's image.
    Bitmap { tag: u32, image: u16 },
    /// The texture header bound when this shader draw was recorded.
    Bound { header: u32 },
    /// The renderer's glyph atlas of engine font `font`.
    Glyphs { font: u16 },
    /// Movie `movie`'s current frame, as the host uploads it (a
    /// [`OverlayKind::MovieFrame`]'s texture).
    Movie { movie: u32 },
    /// The pixels an [`OverlayKind::Hold`] of `hold` last kept, at the
    /// size of the target the item draws into.
    Held { hold: u32 },
}

/// How the texture is sampled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize)]
pub struct Sampler {
    /// Wrap (true) or clamp at the edges.
    pub wrap: bool,
    /// Point (true) or linear filtering.
    pub point: bool,
}

/// An item's material key: consecutive items with equal keys and clips
/// draw as one batch.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub struct OverlayMaterial {
    pub program: Program,
    pub texture: OverlayTexture,
    pub sampler: Sampler,
    pub blend: OverlayBlend,
}

impl OverlayMaterial {
    /// A flat colour.
    pub fn fill(blend: OverlayBlend) -> OverlayMaterial {
        OverlayMaterial {
            program: Program::Fixed(Combiner::Flat),
            texture: OverlayTexture::None,
            sampler: Sampler::default(),
            blend,
        }
    }

    /// Engine font `font`'s glyphs, alpha blended. Point sampling: the
    /// pages hold glyphs rasterised at the display's density, so a page
    /// texel draws near 1:1 and a texel keeps the coverage the page holds
    /// (a covered texel of a 1x page draws at its full value).
    pub fn text(font: u16) -> OverlayMaterial {
        OverlayMaterial {
            program: Program::Fixed(Combiner::TextureAlpha),
            texture: OverlayTexture::Glyphs { font },
            sampler: Sampler { wrap: false, point: true },
            blend: OverlayBlend::ALPHA,
        }
    }

    /// Whether the program samples [`Self::texture`].
    pub fn sampled(&self) -> bool {
        match self.program {
            Program::Fixed(c) => c != Combiner::Flat,
            Program::Shader { .. } => self.texture != OverlayTexture::None,
        }
    }
}

/// One corner of a quad or fill.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, bytemuck::Pod, bytemuck::Zeroable)]
pub struct OverlayVertex {
    /// Guest target pixels, window coordinates.
    pub position: [f32; 2],
    /// The texture's normalised coordinate.
    pub uv: [f32; 2],
    /// RGBA, 0 to 1.
    pub color: [f32; 4],
}

/// One glyph of a text run: the font's glyph index and where its bitmap
/// lands, `[x0, y0, x1, y1]` in guest pixels. The renderer's atlas
/// supplies its coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct OverlayGlyph {
    pub index: u32,
    pub rect: [f32; 4],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize)]
pub enum OverlayKind {
    /// Four corners, drawn as the fan `0, 1, 2` and `0, 2, 3`.
    Quad,
    /// A run of one font's glyphs in one colour: one draw.
    TextRun,
    /// A flat rectangle: four corners, no texture.
    Fill,
    /// The target's pixels drawn so far, blurred inside the clip
    /// ([`OverlayBlur`]): the menus' backdrop under a dialog.
    Blur,
    /// A menu's model widget: its objects seen from its camera, drawn
    /// into the clip ([`OverlayModelView`]).
    ModelView,
    /// The current frame of a movie (`detail` names its [`OverlayMovie`]),
    /// four corners like a quad: the renderer takes the frame's texels
    /// from the movie's upload.
    MovieFrame,
    /// The target's pixels drawn so far, kept whole in the renderer's
    /// texture `hold` (the item's `detail`) until the next hold of it,
    /// across frames: what [`OverlayTexture::Held`] samples. No corners;
    /// draws nothing itself.
    Hold,
}

/// Which movie a [`OverlayKind::MovieFrame`] item shows, and which frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct OverlayMovie {
    /// The movie's id, as the scene's uploads name it.
    pub movie: u32,
    /// The frame number the producer saw.
    pub frame: u32,
}

/// The blur under a dialog (`0x00235756` running the Kawase blur
/// `0x000211a0`; docs/graphics/native-post-hud.md, "The blur under a
/// dialog"): each step makes every pixel a blend of four bilinear taps at
/// the corners of a square, tap `k` moved by `spread * tap` guest pixels,
/// positive for `k & 1` in x and `k & 2` in y, negative otherwise. The
/// sampler clamps at the target's edges.
///
/// The step's combiner blends the taps in pairs, `t0` with `t1`, `t2` with
/// `t3`, then the two results, each colour channel as
/// `w * a + (1 - w) * b` with `w = 0.5 + 0.5 * pick * clamp(4 * (a - b),
/// -1, 1)`: a `pick` of 0 is the plain mean, a larger one leans to the
/// brighter tap. Alpha is the plain mean. All three values are the
/// guest's, read by the producer.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverlayBlur {
    /// The tap's offset over the spread.
    pub tap: f32,
    /// Each step's spread, in draw order; never empty.
    pub spreads: Vec<f32>,
    /// The combiner's constant, 0 to 1 (`engine::glow::blur_combiner`'s
    /// packed strength over 255); 0 for its plain form.
    pub pick: f32,
}

/// A model widget's camera, in its scene's space.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct OverlayCamera {
    pub position: [f32; 3],
    pub forward: [f32; 3],
    pub up: [f32; 3],
    /// The vertical field of view, radians.
    pub fov_y: f32,
    /// The near and far planes of the view's render camera record.
    pub planes: [f32; 2],
    /// The x and y scales and the two off-centre terms the view's
    /// projection holds (`0x0002fd90` at `0x004b9e14 + 0x78`, `+0x8c`,
    /// `+0x98`, `+0x9c`): the guest's own lens, the widescreen and display
    /// factors in it. `None`: the lens from the FOV and the viewport.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projection: Option<[f32; 4]>,
}

/// A model widget's view (widget type 7, docs/formats/ui.md): the
/// objects it shows and its camera. The item's clip is its viewport.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverlayModelView {
    pub camera: OverlayCamera,
    /// The objects' datum handles, in the widget's order.
    pub objects: Vec<u32>,
}

/// One item of the list.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct OverlayItem {
    pub kind: OverlayKind,
    pub material: OverlayMaterial,
    /// Never empty, inside the target.
    pub clip: ClipRect,
    /// Into [`Overlay::vertices`]: four for a quad or a fill, none for a
    /// text run.
    pub vertices: Range<u32>,
    /// Into [`Overlay::glyphs`]: a text run's, none otherwise.
    pub glyphs: Range<u32>,
    /// A text run's colour (RGBA, 0 to 1); a quad's and a fill's are on
    /// their corners.
    pub color: [f32; 4],
    /// Xbox per-channel framebuffer write mask.
    pub color_write: u32,
    /// Texture stages sampling the live glyph cache.
    pub glyph_cache: u8,
    /// Alpha compare and reference (0 through 255) when enabled.
    pub alpha_test: Option<(u32, u8)>,
    /// Stages discarding samples whose alpha is zero.
    pub alpha_kill: u8,
    /// The four shader inputs for this HUD widget.
    pub hud_input: [f32; 4],
    /// The blend's constant colour (RGBA, 0 to 1) for a blend whose factors
    /// name it (the fade's modes 3 to 6); zero otherwise, as D3D's default.
    pub blend_constant: [f32; 4],
    /// Runtime pixel shader definition, including its evaluated combiner constants.
    pub shader_words: Option<Box<[u32]>>,
    /// Bindings of the tag shader, in Xbox texture stage order.
    pub shader_stages: [Option<OverlayTexture>; 4],
    /// Each corner's four normalized texture coordinates after the vertex program.
    pub stage_uv: Option<Box<[[[f32; 2]; 4]; 4]>>,
    /// A blur's index in [`Overlay::blurs`], a model view's in
    /// [`Overlay::model_views`], a movie frame's in [`Overlay::movies`], a
    /// hold's texture id; `None` for the other kinds.
    pub detail: Option<u32>,
    /// The overlay slot the item draws in: [`ViewSlot::Hud`],
    /// [`ViewSlot::Fade`] or [`ViewSlot::Ui`] (the default).
    pub slot: ViewSlot,
}

impl OverlayItem {
    /// The screen-quad batch's blue, green and red write channels.
    pub const RGB_WRITE: u32 = 1 | (1 << 8) | (1 << 16);
}

/// Why the list refused an item. The list is left as it was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OverlayError {
    /// The overlay has no target size.
    NoTarget,
    /// The clip rectangle is empty or outside the target.
    Clip,
    /// The material does not fit the kind: a fill that samples, a text
    /// run that is not over a glyph atlas.
    Material,
    /// A position, coordinate or colour that is not finite, a text run
    /// with no glyphs, or a blur with no steps or a negative spread.
    Geometry,
}

/// The overlay's items, in draw order.
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
pub struct Overlay {
    /// The guest target the positions are in: width, height. `[0, 0]`
    /// when no producer set it, and then the list is empty.
    pub size: [u32; 2],
    pub items: Vec<OverlayItem>,
    pub vertices: Vec<OverlayVertex>,
    pub glyphs: Vec<OverlayGlyph>,
    /// The blur items' steps, in item order.
    pub blurs: Vec<OverlayBlur>,
    /// The model view items' views, in item order.
    pub model_views: Vec<OverlayModelView>,
    /// The movie frame items' movies, in item order.
    pub movies: Vec<OverlayMovie>,
}

/// The overlay's sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct OverlayCounts {
    pub items: u32,
    pub quads: u32,
    pub text_runs: u32,
    pub fills: u32,
    pub glyphs: u32,
    pub blurs: u32,
    pub model_views: u32,
    pub movie_frames: u32,
    pub holds: u32,
    /// Items whose material is a tag shader.
    pub shader_materials: u32,
}

fn finite(v: &[f32]) -> bool {
    v.iter().all(|x| x.is_finite())
}

impl Overlay {
    /// An empty list over a target `size`.
    pub fn new(size: [u32; 2]) -> Overlay {
        Overlay { size, ..Overlay::default() }
    }

    /// Empties the list and forgets the target, keeping the allocations.
    pub fn clear(&mut self) {
        self.size = [0, 0];
        self.items.clear();
        self.vertices.clear();
        self.glyphs.clear();
        self.blurs.clear();
        self.model_views.clear();
        self.movies.clear();
    }

    /// Sets the target size. Items already in the list keep their
    /// positions: set it before the first.
    pub fn set_size(&mut self, size: [u32; 2]) {
        self.size = size;
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    pub fn vertices_of(&self, item: &OverlayItem) -> &[OverlayVertex] {
        self.vertices.get(item.vertices.start as usize..item.vertices.end as usize).unwrap_or(&[])
    }

    pub fn glyphs_of(&self, item: &OverlayItem) -> &[OverlayGlyph] {
        self.glyphs.get(item.glyphs.start as usize..item.glyphs.end as usize).unwrap_or(&[])
    }

    /// A blur item's steps.
    pub fn blur_of(&self, item: &OverlayItem) -> Option<&OverlayBlur> {
        (item.kind == OverlayKind::Blur).then(|| self.blurs.get(item.detail? as usize)).flatten()
    }

    /// A model view item's view.
    pub fn model_view_of(&self, item: &OverlayItem) -> Option<&OverlayModelView> {
        (item.kind == OverlayKind::ModelView).then(|| self.model_views.get(item.detail? as usize)).flatten()
    }

    fn clip(&self, clip: ClipRect) -> Result<ClipRect, OverlayError> {
        if self.size[0] == 0 || self.size[1] == 0 {
            return Err(OverlayError::NoTarget);
        }
        let clip = clip.intersect(&ClipRect::full(self.size));
        if clip.is_empty() {
            return Err(OverlayError::Clip);
        }
        Ok(clip)
    }

    fn push_corners(
        &mut self,
        kind: OverlayKind,
        material: OverlayMaterial,
        clip: ClipRect,
        corners: [OverlayVertex; 4],
    ) -> Result<u32, OverlayError> {
        let clip = self.clip(clip)?;
        if corners.iter().any(|v| !finite(&v.position) || !finite(&v.uv) || !finite(&v.color)) {
            return Err(OverlayError::Geometry);
        }
        let v = self.vertices.len() as u32;
        let g = self.glyphs.len() as u32;
        self.vertices.extend_from_slice(&corners);
        self.items.push(OverlayItem {
            kind,
            material,
            clip,
            vertices: v..v + 4,
            glyphs: g..g,
            color: [1.0; 4],
            alpha_test: None,
            alpha_kill: 0,
            glyph_cache: 0,
            color_write: OverlayItem::RGB_WRITE,
            hud_input: [0.0; 4],
            blend_constant: [0.0; 4],
            shader_words: None,
            shader_stages: [None; 4],
            stage_uv: None,
            detail: None,
            slot: ViewSlot::Ui,
        });
        Ok(self.items.len() as u32 - 1)
    }

    /// A quad of four corners, clipped to `clip` (and the target). Its
    /// index in the list.
    pub fn push_quad(
        &mut self,
        material: OverlayMaterial,
        clip: ClipRect,
        corners: [OverlayVertex; 4],
    ) -> Result<u32, OverlayError> {
        self.push_corners(OverlayKind::Quad, material, clip, corners)
    }

    /// The rectangle `[x0, y0, x1, y1]` in `color`, clipped to `clip`.
    pub fn push_fill(
        &mut self,
        clip: ClipRect,
        rect: [f32; 4],
        color: [f32; 4],
        blend: OverlayBlend,
    ) -> Result<u32, OverlayError> {
        let [x0, y0, x1, y1] = rect;
        let corner = |x, y| OverlayVertex { position: [x, y], uv: [0.0; 2], color };
        let corners = [corner(x0, y0), corner(x1, y0), corner(x1, y1), corner(x0, y1)];
        self.push_corners(OverlayKind::Fill, OverlayMaterial::fill(blend), clip, corners)
    }

    /// A run of `glyphs` in `color` over the glyph atlas `material`
    /// names, clipped to `clip`.
    pub fn push_text_run(
        &mut self,
        material: OverlayMaterial,
        clip: ClipRect,
        color: [f32; 4],
        glyphs: &[OverlayGlyph],
    ) -> Result<u32, OverlayError> {
        let clip = self.clip(clip)?;
        if !matches!(material.texture, OverlayTexture::Glyphs { .. }) || !material.sampled() {
            return Err(OverlayError::Material);
        }
        if glyphs.is_empty() || !finite(&color) || glyphs.iter().any(|g| !finite(&g.rect)) {
            return Err(OverlayError::Geometry);
        }
        let v = self.vertices.len() as u32;
        let g = self.glyphs.len() as u32;
        self.glyphs.extend_from_slice(glyphs);
        self.items.push(OverlayItem {
            kind: OverlayKind::TextRun,
            material,
            clip,
            vertices: v..v,
            glyphs: g..g + glyphs.len() as u32,
            color,
            alpha_test: None,
            alpha_kill: 0,
            glyph_cache: 0,
            color_write: OverlayItem::RGB_WRITE,
            hud_input: [0.0; 4],
            blend_constant: [0.0; 4],
            shader_words: None,
            shader_stages: [None; 4],
            stage_uv: None,
            detail: None,
            slot: ViewSlot::Ui,
        });
        Ok(self.items.len() as u32 - 1)
    }

    /// An item with no corners or glyphs of its own.
    fn push_detail(&mut self, kind: OverlayKind, clip: ClipRect, detail: u32) -> u32 {
        let v = self.vertices.len() as u32;
        let g = self.glyphs.len() as u32;
        self.items.push(OverlayItem {
            kind,
            material: OverlayMaterial::fill(OverlayBlend::Off),
            clip,
            vertices: v..v,
            glyphs: g..g,
            color: [1.0; 4],
            alpha_test: None,
            alpha_kill: 0,
            glyph_cache: 0,
            color_write: OverlayItem::RGB_WRITE,
            hud_input: [0.0; 4],
            blend_constant: [0.0; 4],
            shader_words: None,
            shader_stages: [None; 4],
            stage_uv: None,
            detail: Some(detail),
            slot: ViewSlot::Ui,
        });
        self.items.len() as u32 - 1
    }

    /// The pixels drawn so far blurred inside `clip`, by `blur`'s steps.
    pub fn push_blur(&mut self, clip: ClipRect, blur: OverlayBlur) -> Result<u32, OverlayError> {
        let clip = self.clip(clip)?;
        if blur.spreads.is_empty()
            || !blur.tap.is_finite()
            || blur.spreads.iter().any(|s| !s.is_finite() || *s < 0.0)
            || !(0.0..=1.0).contains(&blur.pick)
        {
            return Err(OverlayError::Geometry);
        }
        self.blurs.push(blur);
        let detail = self.blurs.len() as u32 - 1;
        Ok(self.push_detail(OverlayKind::Blur, clip, detail))
    }

    /// A model widget's `view`, drawn into `viewport`.
    pub fn push_model_view(&mut self, viewport: ClipRect, view: OverlayModelView) -> Result<u32, OverlayError> {
        let clip = self.clip(viewport)?;
        let c = &view.camera;
        if !finite(&c.position) || !finite(&c.forward) || !finite(&c.up) || !c.fov_y.is_finite() || !finite(&c.planes)
            || c.projection.is_some_and(|p| !finite(&p) || p[0] <= 0.0 || p[1] <= 0.0)
        {
            return Err(OverlayError::Geometry);
        }
        self.model_views.push(view);
        let detail = self.model_views.len() as u32 - 1;
        Ok(self.push_detail(OverlayKind::ModelView, clip, detail))
    }

    /// The current frame of `movie` over the four `corners` of `clip`'s
    /// quad: the corners' uv and colour are the producer's, the material
    /// is the program, blend and sampler it wants; its texture is the
    /// movie's ([`OverlayTexture::Movie`]), whatever `material` names.
    pub fn push_movie_frame(
        &mut self,
        material: OverlayMaterial,
        clip: ClipRect,
        corners: [OverlayVertex; 4],
        movie: OverlayMovie,
    ) -> Result<u32, OverlayError> {
        let material = OverlayMaterial { texture: OverlayTexture::Movie { movie: movie.movie }, ..material };
        let n = self.push_corners(OverlayKind::MovieFrame, material, clip, corners)?;
        self.movies.push(movie);
        self.items[n as usize].detail = Some(self.movies.len() as u32 - 1);
        Ok(n)
    }

    /// The target drawn so far kept in the renderer's texture `hold`, for
    /// later items and frames to sample ([`OverlayTexture::Held`]).
    pub fn push_hold(&mut self, hold: u32) -> Result<u32, OverlayError> {
        let clip = self.clip(ClipRect::full(self.size))?;
        Ok(self.push_detail(OverlayKind::Hold, clip, hold))
    }

    /// The texture a hold item keeps the target in.
    pub fn hold_of(&self, item: &OverlayItem) -> Option<u32> {
        (item.kind == OverlayKind::Hold).then_some(item.detail).flatten()
    }

    /// The movie a movie frame item shows.
    pub fn movie_of(&self, item: &OverlayItem) -> Option<&OverlayMovie> {
        (item.kind == OverlayKind::MovieFrame).then(|| self.movies.get(item.detail? as usize)).flatten()
    }

    /// Tags every item of the list with overlay slot `slot`: a producer
    /// that builds its part apart (one HUD's widgets, [`ViewSlot::Hud`])
    /// names the part's slot before it joins the scene's list. `slot` is
    /// one of the overlay's ([`ViewSlot::is_overlay`]).
    pub fn set_slot(&mut self, slot: ViewSlot) {
        debug_assert!(slot.is_overlay(), "{slot:?} is not an overlay slot");
        for item in &mut self.items {
            item.slot = slot;
        }
    }

    /// The items of overlay slot `slot`, in list order.
    pub fn in_slot(&self, slot: ViewSlot) -> impl Iterator<Item = &OverlayItem> {
        self.items.iter().filter(move |item| item.slot == slot)
    }

    /// Adds `other`'s items after this list's, in their order, each with
    /// its slot: a producer
    /// that builds its part apart (one HUD's widgets) and then joins the
    /// scene's list. A list with no size takes `other`'s; one of another
    /// size refuses it ([`OverlayError::NoTarget`]), as it does an `other`
    /// that does not [`Overlay::check`] ([`OverlayError::Geometry`]). The
    /// indices `other`'s items have in this list.
    pub fn append(&mut self, other: &Overlay) -> Result<Range<u32>, OverlayError> {
        let start = self.items.len() as u32;
        if other.is_empty() {
            return Ok(start..start);
        }
        if other.check().is_err() {
            return Err(OverlayError::Geometry);
        }
        if self.size == [0, 0] {
            self.size = other.size;
        } else if self.size != other.size {
            return Err(OverlayError::NoTarget);
        }
        let (v, g) = (self.vertices.len() as u32, self.glyphs.len() as u32);
        self.vertices.extend_from_slice(&other.vertices);
        self.glyphs.extend_from_slice(&other.glyphs);
        let (b, m) = (self.blurs.len() as u32, self.model_views.len() as u32);
        self.blurs.extend(other.blurs.iter().cloned());
        self.model_views.extend(other.model_views.iter().cloned());
        let w = self.movies.len() as u32;
        self.movies.extend_from_slice(&other.movies);
        self.items.extend(other.items.iter().map(|item| OverlayItem {
            vertices: item.vertices.start + v..item.vertices.end + v,
            glyphs: item.glyphs.start + g..item.glyphs.end + g,
            detail: item.detail.map(|d| {
                d + match item.kind {
                    OverlayKind::Blur => b,
                    OverlayKind::MovieFrame => w,
                    // A hold names its texture, not an entry.
                    OverlayKind::Hold => 0,
                    _ => m,
                }
            }),
            ..item.clone()
        }));
        Ok(start..self.items.len() as u32)
    }

    pub fn counts(&self) -> OverlayCounts {
        let mut c = OverlayCounts { items: self.items.len() as u32, ..OverlayCounts::default() };
        for item in &self.items {
            match item.kind {
                OverlayKind::Quad => c.quads += 1,
                OverlayKind::TextRun => c.text_runs += 1,
                OverlayKind::Fill => c.fills += 1,
                OverlayKind::Blur => c.blurs += 1,
                OverlayKind::ModelView => c.model_views += 1,
                OverlayKind::MovieFrame => c.movie_frames += 1,
                OverlayKind::Hold => c.holds += 1,
            }
            c.glyphs += item.glyphs.len() as u32;
            if matches!(item.material.program, Program::Shader { .. }) {
                c.shader_materials += 1;
            }
        }
        c
    }

    /// Every item's ranges in bounds and in list order, each kind with
    /// what it needs, each clip inside the target. An error names the
    /// first that is not.
    pub fn check(&self) -> Result<(), String> {
        if self.items.is_empty() {
            return Ok(());
        }
        let target = ClipRect::full(self.size);
        let (mut v, mut g, mut b, mut m, mut w) = (0u32, 0u32, 0u32, 0u32, 0u32);
        for (n, item) in self.items.iter().enumerate() {
            let r = (&item.vertices, &item.glyphs);
            if r.0.start != v || r.0.end < r.0.start || r.0.end as usize > self.vertices.len() {
                return Err(format!("overlay item {n} vertices {:?} not after {v} in 0..{}", r.0, self.vertices.len()));
            }
            if r.1.start != g || r.1.end < r.1.start || r.1.end as usize > self.glyphs.len() {
                return Err(format!("overlay item {n} glyphs {:?} not after {g} in 0..{}", r.1, self.glyphs.len()));
            }
            v = r.0.end;
            g = r.1.end;
            let (corners, glyphs) = (r.0.len(), r.1.len());
            let fits = match item.kind {
                OverlayKind::Quad => corners == 4 && glyphs == 0,
                OverlayKind::MovieFrame => {
                    corners == 4 && glyphs == 0 && matches!(item.material.texture, OverlayTexture::Movie { .. })
                }
                OverlayKind::Fill => corners == 4 && glyphs == 0 && !item.material.sampled(),
                OverlayKind::TextRun => {
                    corners == 0 && glyphs > 0 && matches!(item.material.texture, OverlayTexture::Glyphs { .. })
                }
                OverlayKind::Blur | OverlayKind::ModelView => corners == 0 && glyphs == 0,
                OverlayKind::Hold => corners == 0 && glyphs == 0 && item.detail.is_some(),
            };
            if !fits {
                return Err(format!("overlay item {n} is a {:?} with {corners} corners and {glyphs} glyphs", item.kind));
            }
            let next = match item.kind {
                OverlayKind::Blur => Some(&mut b),
                OverlayKind::ModelView => Some(&mut m),
                OverlayKind::MovieFrame => Some(&mut w),
                _ => None,
            };
            match (next, item.detail) {
                (None, Some(_)) if item.kind == OverlayKind::Hold => {}
                (None, None) => {}
                (Some(next), Some(d)) if d == *next => *next += 1,
                (_, detail) => {
                    return Err(format!("overlay item {n} is a {:?} with detail {detail:?}", item.kind));
                }
            }
            if item.clip.is_empty() || item.clip.intersect(&target) != item.clip {
                return Err(format!("overlay item {n} clip {:?} outside {:?}", item.clip, self.size));
            }
        }
        if v as usize != self.vertices.len() || g as usize != self.glyphs.len() {
            return Err("overlay has vertices or glyphs no item names".into());
        }
        if b as usize != self.blurs.len()
            || m as usize != self.model_views.len()
            || w as usize != self.movies.len()
        {
            return Err("overlay has blurs, model views or movies no item names".into());
        }
        Ok(())
    }
}

/// The overlay's words for the summary hash (`crate::summary_hash`). An
/// empty overlay adds none, so a scene without one hashes as before the
/// stage existed.
pub(crate) fn summary_words(overlay: &Overlay, word: &mut dyn FnMut(u32)) {
    if overlay.is_empty() {
        return;
    }
    word(overlay.size[0]);
    word(overlay.size[1]);
    word(overlay.items.len() as u32);
    fn floats(word: &mut dyn FnMut(u32), f: &[f32]) {
        for x in f {
            word(x.to_bits());
        }
    }
    fn texture_word(word: &mut dyn FnMut(u32), texture: OverlayTexture) {
        match texture {
            OverlayTexture::None => word(0),
            OverlayTexture::Bound { header } => { word(3); word(header); }
            OverlayTexture::Bitmap { tag, image } => {
                word(1);
                word(tag);
                word(u32::from(image));
            }
            OverlayTexture::Glyphs { font } => {
                word(2);
                word(u32::from(font));
            }
            OverlayTexture::Movie { movie } => {
                word(4);
                word(movie);
            }
            OverlayTexture::Held { hold } => {
                word(5);
                word(hold);
            }
        }
    }
    for item in &overlay.items {
        word(item.kind as u32);
        let m = &item.material;
        match m.program {
            Program::Fixed(c) => word(c as u32),
            Program::Shader { tag } => {
                word(0x100);
                word(tag);
                word(item.glyph_cache.into());
                word(item.alpha_kill.into());
                if let Some((func, value)) = item.alpha_test {
                    word(func);
                    word(value.into());
                }
                floats(word, &item.hud_input);
                if let Some(words) = &item.shader_words {
                    for &w in words.iter() { word(w); }
                }
            }
        }
        texture_word(word, m.texture);
        if matches!(m.program, Program::Shader { .. }) {
            for stage in item.shader_stages {
                texture_word(word, stage.unwrap_or_default());
            }
            if let Some(all) = &item.stage_uv {
                for corner in all.iter() {
                    for uv in corner { floats(word, uv); }
                }
            }
        }
        word(u32::from(m.sampler.wrap) | u32::from(m.sampler.point) << 1);
        match m.blend {
            OverlayBlend::Off => word(0),
            OverlayBlend::On { source, dest, equation } => {
                word(1);
                word(source);
                word(dest);
                word(equation);
            }
        }
        // Only a constant blend adds words, so other items hash as before.
        if item.blend_constant != [0.0; 4] {
            word(0x200);
            floats(word, &item.blend_constant);
        }
        let c = item.clip;
        for w in [c.x, c.y, c.width, c.height] {
            word(w);
        }
        word(item.color_write);
        floats(&mut *word, &item.color);
        for v in overlay.vertices_of(item) {
            floats(&mut *word, &v.position);
            floats(&mut *word, &v.uv);
            floats(&mut *word, &v.color);
        }
        for g in overlay.glyphs_of(item) {
            word(g.index);
            floats(&mut *word, &g.rect);
        }
        if let Some(m) = overlay.movie_of(item) {
            word(m.movie);
            word(m.frame);
        }
        if let Some(hold) = overlay.hold_of(item) {
            word(hold);
        }
        if let Some(b) = overlay.blur_of(item) {
            floats(&mut *word, &[b.tap]);
            word(b.spreads.len() as u32);
            floats(&mut *word, &b.spreads);
            // Only a leaning combiner adds words, so a plain mean hashes
            // as before.
            if b.pick != 0.0 {
                word(0x201);
                floats(&mut *word, &[b.pick]);
            }
        }
        if let Some(m) = overlay.model_view_of(item) {
            let c = &m.camera;
            floats(&mut *word, &c.position);
            floats(&mut *word, &c.forward);
            floats(&mut *word, &c.up);
            floats(&mut *word, &[c.fov_y]);
            word(m.objects.len() as u32);
            for o in &m.objects {
                word(*o);
            }
        }
    }
}

impl FrameScene {
    /// The overlay's sizes.
    pub fn overlay_counts(&self) -> OverlayCounts {
        self.overlay.counts()
    }
}

impl SceneBuilder {
    /// The scene's overlay, to add items to in draw order. The overlay is
    /// the scene's, not a view's: it is drawn after every view.
    pub fn overlay_mut(&mut self) -> Result<&mut Overlay, BuildError> {
        Ok(&mut self.scene_mut()?.overlay)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corner(x: f32, y: f32, u: f32, v: f32) -> OverlayVertex {
        OverlayVertex { position: [x, y], uv: [u, v], color: [1.0; 4] }
    }

    fn sprite() -> OverlayMaterial {
        OverlayMaterial {
            program: Program::Fixed(Combiner::Texture),
            texture: OverlayTexture::Bitmap { tag: 0xe001_0002, image: 0 },
            sampler: Sampler::default(),
            blend: OverlayBlend::ALPHA,
        }
    }

    fn quad(x: f32, y: f32) -> [OverlayVertex; 4] {
        [corner(x, y, 0.0, 0.0), corner(x + 8.0, y, 1.0, 0.0), corner(x + 8.0, y + 8.0, 1.0, 1.0), corner(x, y + 8.0, 0.0, 1.0)]
    }

    #[test]
    fn a_hud_part_keeps_its_slot_in_the_scenes_list() {
        let all = ClipRect::full([640, 480]);
        let mut menu = Overlay::new([640, 480]);
        menu.push_quad(sprite(), all, quad(0.0, 0.0)).unwrap();
        let mut hud = Overlay::new([640, 480]);
        hud.push_quad(sprite(), all, quad(20.0, 20.0)).unwrap();
        hud.push_fill(all, [0.0, 0.0, 10.0, 10.0], [0.0; 4], OverlayBlend::ALPHA).unwrap();
        hud.set_slot(ViewSlot::Hud);
        let mut scene = Overlay::default();
        scene.append(&hud).unwrap();
        scene.append(&menu).unwrap();
        let slots: Vec<ViewSlot> = scene.items.iter().map(|i| i.slot).collect();
        assert_eq!(slots, [ViewSlot::Hud, ViewSlot::Hud, ViewSlot::Ui]);
        assert_eq!(scene.in_slot(ViewSlot::Hud).count(), 2);
        assert_eq!(scene.in_slot(ViewSlot::Ui).map(|i| i.kind).collect::<Vec<_>>(), [OverlayKind::Quad]);
        assert_eq!(scene.in_slot(ViewSlot::Fade).count(), 0);
    }

    #[test]
    fn items_keep_their_order_and_ranges() {
        let mut o = Overlay::new([640, 480]);
        let all = ClipRect::full([640, 480]);
        assert_eq!(o.push_fill(all, [0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 0.0, 0.5], OverlayBlend::ALPHA), Ok(0));
        assert_eq!(o.push_quad(sprite(), all, quad(20.0, 20.0)), Ok(1));
        let glyphs = [OverlayGlyph { index: 7, rect: [30.0, 30.0, 36.0, 40.0] }, OverlayGlyph { index: 8, rect: [36.0, 30.0, 42.0, 40.0] }];
        assert_eq!(o.push_text_run(OverlayMaterial::text(3), all, [1.0, 0.5, 0.25, 1.0], &glyphs), Ok(2));
        assert_eq!(o.push_quad(sprite(), all, quad(50.0, 20.0)), Ok(3));
        let kinds: Vec<_> = o.items.iter().map(|i| i.kind).collect();
        assert_eq!(kinds, [OverlayKind::Fill, OverlayKind::Quad, OverlayKind::TextRun, OverlayKind::Quad]);
        assert_eq!(o.items[3].vertices, 8..12);
        assert_eq!(o.glyphs_of(&o.items[2]), &glyphs);
        assert_eq!(o.vertices_of(&o.items[3])[0].position, [50.0, 20.0]);
        assert_eq!(
            o.counts(),
            OverlayCounts { items: 4, quads: 2, text_runs: 1, fills: 1, glyphs: 2, ..OverlayCounts::default() }
        );
        assert_eq!(o.check(), Ok(()));
    }

    #[test]
    fn clips_are_cut_to_the_target_and_an_empty_one_is_refused() {
        let mut o = Overlay::default();
        assert_eq!(o.push_quad(sprite(), ClipRect::full([8, 8]), quad(0.0, 0.0)), Err(OverlayError::NoTarget));
        o.set_size([640, 480]);
        let wide = ClipRect { x: 600, y: 470, width: 100, height: 100 };
        o.push_quad(sprite(), wide, quad(600.0, 470.0)).unwrap();
        assert_eq!(o.items[0].clip, ClipRect { x: 600, y: 470, width: 40, height: 10 });
        let off = ClipRect { x: 700, y: 0, width: 10, height: 10 };
        assert_eq!(o.push_quad(sprite(), off, quad(0.0, 0.0)), Err(OverlayError::Clip));
        assert_eq!(o.items.len(), 1, "the list is left as it was");
    }

    #[test]
    fn a_text_run_needs_glyphs_over_an_atlas_and_finite_geometry() {
        let mut o = Overlay::new([640, 480]);
        let all = ClipRect::full([640, 480]);
        let g = [OverlayGlyph { index: 1, rect: [0.0, 0.0, 4.0, 4.0] }];
        assert_eq!(o.push_text_run(sprite(), all, [1.0; 4], &g), Err(OverlayError::Material));
        assert_eq!(o.push_text_run(OverlayMaterial::text(0), all, [1.0; 4], &[]), Err(OverlayError::Geometry));
        let mut bad = quad(0.0, 0.0);
        bad[2].uv[0] = f32::NAN;
        assert_eq!(o.push_quad(sprite(), all, bad), Err(OverlayError::Geometry));
        assert!(o.is_empty());
        assert_eq!(o.check(), Ok(()));
    }

    fn camera() -> OverlayCamera {
        OverlayCamera { position: [0.0, -2.0, 0.5], forward: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0], fov_y: 0.9, planes: [0.0625, 200.0], projection: None }
    }

    #[test]
    fn blurs_and_model_views_keep_their_place_and_their_details() {
        let mut o = Overlay::new([640, 480]);
        let all = ClipRect::full([640, 480]);
        o.push_fill(all, [0.0, 0.0, 640.0, 480.0], [0.0, 0.0, 0.0, 0.25], OverlayBlend::ALPHA).unwrap();
        let blur = OverlayBlur { tap: 0.5, spreads: vec![1.0, 1.5, 2.0], pick: 0.0 };
        assert_eq!(o.push_blur(all, blur.clone()), Ok(1));
        let viewport = ClipRect { x: 100, y: 80, width: 200, height: 150 };
        let view = OverlayModelView { camera: camera(), objects: vec![0xe000_0001, 0xe001_0002] };
        assert_eq!(o.push_model_view(viewport, view.clone()), Ok(2));
        o.push_quad(sprite(), all, quad(0.0, 0.0)).unwrap();
        assert_eq!(o.blur_of(&o.items[1]), Some(&blur));
        assert_eq!(o.model_view_of(&o.items[2]), Some(&view));
        assert_eq!(o.items[2].clip, viewport);
        assert_eq!(o.blur_of(&o.items[2]), None, "a model view has no blur");
        assert_eq!(o.items[3].vertices, 4..8, "a blur and a model view take no corners");
        let c = o.counts();
        assert_eq!((c.items, c.blurs, c.model_views, c.fills, c.quads), (4, 1, 1, 1, 1));
        assert_eq!(o.check(), Ok(()));
        let mut swapped = o.clone();
        swapped.items[1].detail = Some(1);
        assert!(swapped.check().unwrap_err().contains("detail"));
        let mut orphan = o.clone();
        orphan.blurs.push(blur);
        assert!(orphan.check().unwrap_err().contains("blurs"));
    }

    #[test]
    fn a_blur_needs_steps_and_a_model_view_a_finite_camera() {
        let mut o = Overlay::new([640, 480]);
        let all = ClipRect::full([640, 480]);
        assert_eq!(o.push_blur(all, OverlayBlur { tap: 0.5, spreads: vec![], pick: 0.0 }), Err(OverlayError::Geometry));
        assert_eq!(o.push_blur(all, OverlayBlur { tap: 0.5, spreads: vec![-1.0], pick: 0.0 }), Err(OverlayError::Geometry));
        assert_eq!(o.push_blur(all, OverlayBlur { tap: f32::NAN, spreads: vec![1.0], pick: 0.0 }), Err(OverlayError::Geometry));
        assert_eq!(o.push_blur(all, OverlayBlur { tap: 0.5, spreads: vec![1.0], pick: 1.5 }), Err(OverlayError::Geometry), "a pick past 1");
        let mut bad = camera();
        bad.fov_y = f32::INFINITY;
        let view = OverlayModelView { camera: bad, objects: vec![] };
        assert_eq!(o.push_model_view(all, view), Err(OverlayError::Geometry));
        let lens = |projection| OverlayModelView { camera: OverlayCamera { projection: Some(projection), ..camera() }, objects: vec![] };
        assert_eq!(o.push_model_view(all, lens([1.0, f32::NAN, 0.0, 0.0])), Err(OverlayError::Geometry), "a lens that is not finite");
        assert_eq!(o.push_model_view(all, lens([0.0, 1.0, 0.0, 0.0])), Err(OverlayError::Geometry), "a lens with no x scale");
        let off = ClipRect { x: 640, y: 0, width: 4, height: 4 };
        assert_eq!(o.push_blur(off, OverlayBlur { tap: 0.5, spreads: vec![1.0], pick: 0.0 }), Err(OverlayError::Clip));
        assert!(o.is_empty() && o.blurs.is_empty() && o.model_views.is_empty(), "the list is left as it was");
    }

    #[test]
    fn check_names_a_broken_item() {
        let mut o = Overlay::new([640, 480]);
        let all = ClipRect::full([640, 480]);
        o.push_quad(sprite(), all, quad(0.0, 0.0)).unwrap();
        let mut fill = o.clone();
        fill.items[0].kind = OverlayKind::Fill;
        assert!(fill.check().unwrap_err().contains("Fill"), "a fill that samples");
        let mut range = o.clone();
        range.items[0].vertices = 0..5;
        assert!(range.check().is_err());
        let mut clip = o.clone();
        clip.items[0].clip.width = 641;
        assert!(clip.check().unwrap_err().contains("clip"));
    }

    #[test]
    fn the_summary_hash_sees_the_overlay_and_ignores_an_empty_one() {
        let empty = FrameScene::default();
        let mut scene = FrameScene::default();
        scene.overlay.set_size([640, 480]);
        assert_eq!(crate::summary_hash(&scene), crate::summary_hash(&empty), "no items, no words");
        scene.overlay.push_fill(ClipRect::full([640, 480]), [0.0, 0.0, 1.0, 1.0], [1.0; 4], OverlayBlend::Off).unwrap();
        let one = crate::summary_hash(&scene);
        assert_ne!(one, crate::summary_hash(&empty));
        scene.overlay.vertices[1].position[0] = 2.0;
        assert_ne!(crate::summary_hash(&scene), one, "a corner moved");
        scene.overlay.push_blur(ClipRect::full([640, 480]), OverlayBlur { tap: 0.5, spreads: vec![1.0], pick: 0.0 }).unwrap();
        let blurred = crate::summary_hash(&scene);
        scene.overlay.blurs[0].spreads[0] = 1.25;
        assert_ne!(crate::summary_hash(&scene), blurred, "a blur's spread changed");
        let spread = crate::summary_hash(&scene);
        scene.overlay.blurs[0].pick = 0.25;
        assert_ne!(crate::summary_hash(&scene), spread, "a blur's combiner changed");
        scene.clear();
        assert!(scene.overlay.is_empty() && scene.overlay.size == [0, 0]);
    }

    #[test]
    fn an_appended_list_keeps_its_order_after_the_items_before_it() {
        let all = ClipRect::full([640, 480]);
        let mut scene = Overlay::default();
        let mut first = Overlay::new([640, 480]);
        first.push_quad(sprite(), all, quad(0.0, 0.0)).unwrap();
        assert_eq!(scene.append(&first), Ok(0..1));
        assert_eq!(scene.size, [640, 480], "an empty list takes the part's size");
        let mut second = Overlay::new([640, 480]);
        let glyphs = [OverlayGlyph { index: 4, rect: [1.0, 2.0, 3.0, 4.0] }];
        second.push_text_run(OverlayMaterial::text(6), all, [1.0; 4], &glyphs).unwrap();
        second.push_quad(sprite(), all, quad(30.0, 0.0)).unwrap();
        assert_eq!(scene.append(&second), Ok(1..3));
        assert_eq!(scene.check(), Ok(()));
        assert_eq!(scene.items[2].vertices, 4..8);
        assert_eq!(scene.vertices_of(&scene.items[2])[0].position, [30.0, 0.0]);
        assert_eq!(scene.glyphs_of(&scene.items[1]), &glyphs);
        assert_eq!(scene.append(&Overlay::default()), Ok(3..3), "nothing to add");
        let other = {
            let mut o = Overlay::new([320, 240]);
            o.push_quad(sprite(), ClipRect::full([320, 240]), quad(0.0, 0.0)).unwrap();
            o
        };
        assert_eq!(scene.append(&other), Err(OverlayError::NoTarget));
        assert_eq!(scene.items.len(), 3, "a refused part leaves the list as it was");
    }

    #[test]
    fn the_builder_writes_the_scenes_overlay() {
        let mut b = SceneBuilder::default();
        assert_eq!(b.overlay_mut().err(), Some(BuildError::NoScene));
        b.begin(crate::SceneId::default());
        let o = b.overlay_mut().unwrap();
        o.set_size([640, 480]);
        o.push_fill(ClipRect::full([640, 480]), [0.0, 0.0, 4.0, 4.0], [1.0; 4], OverlayBlend::Off).unwrap();
        let scene = b.scene().unwrap();
        assert_eq!(scene.overlay_counts().fills, 1);
        assert_eq!(scene.check(), Ok(()));
    }

    #[test]
    fn overlay_items_keep_list_order() {
        let mut o = Overlay::new([640, 480]);
        let all = ClipRect::full([640, 480]);
        let glyph = [OverlayGlyph { index: 1, rect: [0.0, 0.0, 4.0, 4.0] }];
        let blur = OverlayBlur { tap: 1.0, spreads: vec![1.0], pick: 0.0 };
        let view = OverlayModelView {
            camera: OverlayCamera { position: [0.0; 3], forward: [0.0, 1.0, 0.0], up: [0.0, 0.0, 1.0], fov_y: 1.0, planes: [0.0625, 200.0], projection: None },
            objects: vec![1],
        };
        o.push_movie_frame(sprite(), all, quad(0.0, 0.0), OverlayMovie { movie: 9, frame: 4 }).unwrap();
        o.push_blur(all, blur).unwrap();
        o.push_text_run(OverlayMaterial::text(1), all, [1.0; 4], &glyph).unwrap();
        o.push_model_view(all, view).unwrap();
        o.push_fill(all, [0.0, 0.0, 2.0, 2.0], [1.0; 4], OverlayBlend::Off).unwrap();
        o.push_quad(sprite(), all, quad(4.0, 4.0)).unwrap();
        let kinds: Vec<_> = o.items.iter().map(|i| i.kind).collect();
        assert_eq!(
            kinds,
            [
                OverlayKind::MovieFrame,
                OverlayKind::Blur,
                OverlayKind::TextRun,
                OverlayKind::ModelView,
                OverlayKind::Fill,
                OverlayKind::Quad
            ]
        );
        assert_eq!(o.movie_of(&o.items[0]), Some(&OverlayMovie { movie: 9, frame: 4 }));
        assert_eq!(o.items[0].material.texture, OverlayTexture::Movie { movie: 9 });
        assert_eq!(o.counts().movie_frames, 1);
        assert_eq!(o.check(), Ok(()));
        // A movie frame samples its movie and nothing else.
        let mut bad = o.clone();
        bad.items[0].material.texture = OverlayTexture::None;
        assert!(bad.check().is_err());
        // Joined after another list, the movie keeps its own entry.
        let mut a = Overlay::new([640, 480]);
        a.push_movie_frame(sprite(), all, quad(0.0, 0.0), OverlayMovie { movie: 1, frame: 0 }).unwrap();
        a.append(&o).unwrap();
        assert_eq!(a.movie_of(&a.items[1]), Some(&OverlayMovie { movie: 9, frame: 4 }));
        assert_eq!(a.check(), Ok(()));
    }

    #[test]
    fn a_hold_keeps_its_texture_id_through_a_join() {
        let all = ClipRect::full([640, 480]);
        let mut o = Overlay::new([640, 480]);
        o.push_hold(7).unwrap();
        let mut held = sprite();
        held.texture = OverlayTexture::Held { hold: 7 };
        o.push_quad(held, all, quad(0.0, 0.0)).unwrap();
        assert_eq!((o.items[0].kind, o.items[0].clip), (OverlayKind::Hold, all));
        assert_eq!(o.hold_of(&o.items[0]), Some(7));
        assert_eq!(o.hold_of(&o.items[1]), None);
        assert_eq!(o.counts().holds, 1);
        assert_eq!(o.check(), Ok(()));
        let mut a = Overlay::new([640, 480]);
        a.push_blur(all, OverlayBlur { tap: 1.0, spreads: vec![1.0], pick: 0.0 }).unwrap();
        a.append(&o).unwrap();
        assert_eq!(a.hold_of(&a.items[1]), Some(7), "not moved by the blur before it");
        assert_eq!(a.check(), Ok(()));
        assert_eq!(Overlay::default().push_hold(7), Err(OverlayError::NoTarget));
    }

    #[test]
    fn a_clip_rectangle_scissors_only_its_items() {
        let mut o = Overlay::new([640, 480]);
        let half = ClipRect { x: 0, y: 0, width: 320, height: 480 };
        let all = ClipRect::full([640, 480]);
        o.push_quad(sprite(), all, quad(0.0, 0.0)).unwrap();
        o.push_quad(sprite(), half, quad(400.0, 0.0)).unwrap();
        o.push_fill(all, [0.0, 0.0, 9.0, 9.0], [1.0; 4], OverlayBlend::Off).unwrap();
        let clips: Vec<_> = o.items.iter().map(|i| i.clip).collect();
        assert_eq!(clips, [all, half, all], "the narrow clip is the middle item's alone");
    }
}
