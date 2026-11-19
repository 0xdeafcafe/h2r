//! The native HUD's host (docs/graphics/native-renderer.md, "Stage 5:
//! post and HUD", "The UI and HUD", "Native text"): it plans each batch of
//! screen quads (`native_render::hud::Batch`: text over the glyph cache
//! so far) and says once per reason why a batch stays on D3D.
//!
//! With `impl.render.native_hud` on, a planned batch is drawn natively in
//! segments (`native_render::hud_gpu`). At a quad, before skipping it, the
//! host reads what the begin set (the render states, the combiner block
//! and that the bound combiner is it, the clip area, the viewport, the
//! glyph cache's texels) and reserves the segment's pass on the device
//! (`Device::prepare_external_pass`: it drains what came before, records
//! the pass and commits it). Refused, or anything not established, the
//! quad and the rest of the batch keep their D3D draws ([`Kept`], logged
//! once). Reserved, the quad is skipped, and each later quad joins the
//! segment (written into its buffers, then skipped) while the device's
//! counters, bound target and the glyph cache's ring stand still since
//! the reservation; otherwise it reserves a new segment. A state the
//! reservation's drain changed voids the segment (it draws nothing) and
//! the quad keeps its D3D draw. HUD shader quads keep theirs.
//!
//! `H2_NATIVE_HUD_COMPARE=N` (`feature::native_hud::compare_spec`) pairs
//! the first `N` hooked quads with the draws the D3D path resolves for
//! them. The hooks queue each quad's corners as the call pushes them
//! (`hud::Expect`); a draw admission on the device (`d3d::DrawAdmission`,
//! the handoff's seam: it sees every resolved draw in issue order, with
//! the guest's state applied) reads each four-vertex fan's positions and
//! pairs it with the first waiting quad of the same corners
//! (`hud::Pairing`). It claims no draw. For the first paired quad of each
//! kind it prints what D3D resolved (entry, target, stage 0's texture,
//! blend) beside the batch's plan; every 64 pairings, and when the last
//! queued quad is paired, one line of totals:
//! `native-hud: compare: <s> screen quads and <h> HUD quads paired with
//! their D3D draws; <ls> screen and <lh> HUD lost; <o> other fans; <w> waiting`.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use engine::memory::GuestMemory;
use feature::native_hud::{Host, HudQuadCall};
use gfx::vertex::{PrimitiveType, VertexDataType, VertexLayout, VertexRegister};
use native_render::camera::GuestRead;
use native_render::hud::{self, Batch, Expect, HudQuad, Kind, Pairing, Refused};
use native_render::hud_gpu::{
    Atlas, Segment, TextDraw, TextGpu, TextState, TextTarget, text_program,
};

use crate::native_lights_glue::DeviceMemory;

struct Ro<'a>(&'a dyn GuestMemory);

impl GuestRead for Ro<'_> {
    fn read_u32(&self, addr: u32) -> Option<u32> {
        self.0.u32(addr).ok()
    }
}

/// Refusals by reason ([`Refused::ALL`]), each logged once.
static REFUSALS: [AtomicU64; Refused::ALL.len()] =
    [const { AtomicU64::new(0) }; Refused::ALL.len()];
static LOGGED: AtomicU64 = AtomicU64::new(0);
static PLANNED: AtomicU64 = AtomicU64::new(0);

/// Batches refused, by [`Refused::ALL`].
pub fn refusals() -> [u64; Refused::ALL.len()] {
    std::array::from_fn(|i| REFUSALS[i].load(Ordering::Relaxed))
}

/// Batches planned.
pub fn planned() -> u64 {
    PLANNED.load(Ordering::Relaxed)
}

fn refused(r: Refused) {
    let i = r.index();
    REFUSALS[i].fetch_add(1, Ordering::Relaxed);
    if LOGGED.fetch_or(1 << i, Ordering::Relaxed) & (1 << i) == 0 {
        eprintln!(
            "native-hud: a batch of screen quads keeps its D3D draws: {} (logged once)",
            r.describe()
        );
    }
}

/// The positions of a four-vertex fan as the D3D path resolved it
/// (`ResolvedDraw`'s primitive, count, index presence, layout and
/// streams): attribute 0 of each vertex, its x and y. `None` for any
/// other draw, or a position that is not floats.
pub fn fan_corners(
    primitive: PrimitiveType,
    vertex_count: u32,
    indexed: bool,
    layout: &VertexLayout,
    streams: &[d3d::StreamBytes],
) -> Option<[[f32; 2]; 4]> {
    if primitive != PrimitiveType::TriangleFan || vertex_count != 4 || indexed {
        return None;
    }
    let (n, a) = layout.streams.iter().enumerate().find_map(|(n, s)| {
        s.attributes
            .iter()
            .find(|a| a.register == VertexRegister::POSITION)
            .map(|a| (n, a))
    })?;
    if !matches!(
        a.data_type,
        VertexDataType::Float2 | VertexDataType::Float3 | VertexDataType::Float4
    ) {
        return None;
    }
    let stream = streams.get(n)?;
    let bytes: &[u8] = &stream.data;
    let f = |at: usize| -> Option<f32> {
        let b = bytes.get(at..at + 4)?;
        Some(f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    let mut out = [[0.0f32; 2]; 4];
    for (i, c) in out.iter_mut().enumerate() {
        let at = i * stream.stride as usize + a.offset as usize;
        *c = [f(at)?, f(at + 4)?];
    }
    Some(out)
}

/// What the compare shares between the hooks and the admission.
struct Compare {
    pairing: Mutex<Pairing>,
    /// Quads still to queue.
    left: AtomicU64,
    /// The plan of the last batch begun, said beside the first resolved
    /// screen quad.
    plan: Mutex<String>,
    said: Mutex<[bool; 2]>,
}

impl Compare {
    fn expect(&self, e: Expect) {
        if self
            .left
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1))
            .is_ok()
        {
            self.pairing
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .expect(e);
        }
    }
}

impl d3d::DrawAdmission for Compare {
    fn admit(
        &self,
        snapshot: &d3d::DrawSnapshot<'_>,
    ) -> Option<(d3d::ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let d = snapshot.draw;
        let corners = fan_corners(
            d.primitive,
            d.vertex_count,
            d.indices.is_some(),
            &d.layout,
            &d.streams,
        )?;
        let mut p = self.pairing.lock().unwrap_or_else(|e| e.into_inner());
        if p.waiting() == 0 {
            return None;
        }
        let paired = p.draw(&corners)?;
        let total = p.paired[0] + p.paired[1];
        let (line, done) = (p.line(), p.waiting() == 0);
        std::mem::drop(p);
        let k = match paired.kind {
            Kind::Screen => 0,
            Kind::Hud => 1,
        };
        let first = {
            let mut said = self.said.lock().unwrap_or_else(|e| e.into_inner());
            !std::mem::replace(&mut said[k], true)
        };
        if first {
            let r = &d.state.render;
            eprintln!(
                "native-hud: compare: the first {:?} quad resolved as {} into {:#010x}, stage 0 {:?}, blend {} {:?} {:?} {:?}; plan: {}",
                paired.kind,
                d.entry,
                snapshot.color_header,
                d.texture_info[0],
                r.blend_enable,
                r.src_blend,
                r.dst_blend,
                r.blend_op,
                self.plan.lock().unwrap_or_else(|e| e.into_inner())
            );
        }
        if total % 64 == 0 || (done && self.left.load(Ordering::Relaxed) == 0) {
            eprintln!("native-hud: compare: {line}");
        }
        None
    }
}

/// Why a planned text batch keeps its D3D draws from a quad on
/// (`H2_NATIVE_HUD` text segments), each logged once.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kept {
    /// The backend draws no external pass.
    NoExternal,
    /// The quad's corners do not read.
    Quad,
    /// A render state the text pass does not draw, or one that does not
    /// read.
    State,
    /// The bound combiner is not the block the begin wrote.
    Combiner,
    /// The combiner does not compile.
    Program,
    /// The clip area is not one rectangle.
    Clip,
    /// The glyph cache's texels do not read as its shape.
    Atlas,
    /// The device refused the pass.
    Reservation,
    /// The device's drain at the reservation changed a state the pass
    /// was recorded with.
    Drained,
}

impl Kept {
    pub const ALL: [Kept; 9] = [
        Kept::NoExternal,
        Kept::Quad,
        Kept::State,
        Kept::Combiner,
        Kept::Program,
        Kept::Clip,
        Kept::Atlas,
        Kept::Reservation,
        Kept::Drained,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&k| k == self).unwrap_or(0)
    }

    pub fn describe(self) -> &'static str {
        match self {
            Kept::NoExternal => "the backend draws no external pass",
            Kept::Quad => "the quad's corners do not read",
            Kept::State => {
                "a render state the text pass does not draw (depth, stencil, alpha test, fog, stipple, fill, flat shading) or that does not read"
            }
            Kept::Combiner => "the bound combiner is not the block the begin wrote",
            Kept::Program => "the combiner does not compile",
            Kept::Clip => "the clip area is not one rectangle",
            Kept::Atlas => "the glyph cache's texels do not read as LIN_A4R4G4B4 256x256",
            Kept::Reservation => "the device refused the pass",
            Kept::Drained => {
                "the device's drain at the reservation changed a state the pass was recorded with"
            }
        }
    }
}

static KEPT: [AtomicU64; Kept::ALL.len()] = [const { AtomicU64::new(0) }; Kept::ALL.len()];
static KEPT_LOGGED: AtomicU64 = AtomicU64::new(0);
static SEGMENTS: AtomicU64 = AtomicU64::new(0);
static NATIVE_QUADS: AtomicU64 = AtomicU64::new(0);

/// Text segments reserved, quads they drew (each skipped on D3D), and
/// batches kept on D3D by [`Kept::ALL`].
pub fn text_counts() -> (u64, u64, [u64; Kept::ALL.len()]) {
    (
        SEGMENTS.load(Ordering::Relaxed),
        NATIVE_QUADS.load(Ordering::Relaxed),
        std::array::from_fn(|i| KEPT[i].load(Ordering::Relaxed)),
    )
}

fn kept(k: Kept, detail: &str) {
    let i = k.index();
    KEPT[i].fetch_add(1, Ordering::Relaxed);
    if KEPT_LOGGED.fetch_or(1 << i, Ordering::Relaxed) & (1 << i) == 0 {
        eprintln!(
            "native-hud: a text batch keeps its D3D draws: {}{detail} (logged once)",
            k.describe()
        );
    }
}

/// The combiner block's words that are code, not constants (the
/// constants, words 10..26, 43 and 44, may be set again without a new
/// block, and the D3D path takes them from the render states as the text
/// pass does) and not the texture modes (54, render state 136).
fn code_word(i: usize) -> bool {
    !(10..26).contains(&i) && !(43..45).contains(&i) && i != 54
}

/// A text batch's state on the native side.
enum Text {
    /// No batch, or one not planned.
    Idle,
    /// Planned: the next quad reserves a segment.
    Planned(Batch),
    /// A segment reserved: later quads extend it while the device and the
    /// glyph cache stand still.
    Open(Open),
}

struct Open {
    batch: Batch,
    draw: Arc<TextDraw>,
    viewport: [f32; 4],
    /// What must not change for a quad to join the segment: the device's
    /// counters, its bound target and viewport, the glyph cache's ring.
    stats: d3d::Stats,
    target: [u32; 8],
    ring: [u32; hud::GLYPH_RING_WORDS],
}

/// The glyph cache's texels as last mirrored, by bitmap header, texel
/// address and ring: glyphs only enter the cache through the ring
/// (`0x0001f5c0`), so the same three name the same texels.
struct Mirror {
    key: (u32, u32, [u32; hud::GLYPH_RING_WORDS]),
    atlas: Arc<Atlas>,
}

/// The external pass of one text segment.
struct TextPass {
    draw: Arc<TextDraw>,
    gpu: Arc<Mutex<TextGpu>>,
}

impl d3d::ExternalPass for TextPass {
    fn encode(&mut self, _target: d3d::ExternalTarget<'_>) {}

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        let mut gpu = self.gpu.lock().unwrap_or_else(|e| e.into_inner());
        usize::from(gpu.record(
            &self.draw,
            TextTarget {
                device: t.device,
                queue: t.queue,
                encoder: t.encoder,
                color: t.color,
                color_format: t.color_format,
                guest_size: t.guest_size,
                size: t.size,
            },
        ))
    }

    fn writes(&self) -> d3d::ExternalWrites {
        d3d::ExternalWrites {
            color: true,
            depth_stencil: false,
        }
    }

    fn colour_only(&self) -> bool {
        true
    }
}

struct Glue<B: d3d::backend::Backend> {
    compare: Option<Arc<Compare>>,
    /// `impl.render.native_hud`: draw text natively (else plan and
    /// compare only).
    native: bool,
    device: d3d::abi::SharedDevice<B>,
    text: Mutex<Text>,
    mirror: Mutex<Option<Mirror>>,
    gpu: Arc<Mutex<TextGpu>>,
}

/// The render state a draw recorded now would take, for the states the
/// text pass reads; `None` when one does not read or decode.
fn render_state<B: d3d::backend::Backend>(
    d: &d3d::Device<B>,
    mem: &dyn d3d::GuestMemory,
) -> Option<gfx::state::RenderState> {
    let table = gfx::xdk::RenderStateTable::default();
    let mut rs = gfx::state::RenderState::default();
    for t in TEXT_STATES {
        let guest = table.canonical_to_guest(t.index())?;
        rs.set_raw(t, d.render_state(mem, guest)?).ok()?;
    }
    Some(rs)
}

/// The render states [`render_state`] decodes.
const TEXT_STATES: [gfx::state::RenderStateType; 14] = {
    use gfx::state::RenderStateType as R;
    [
        R::ZEnable,
        R::AlphaBlendEnable,
        R::SrcBlend,
        R::DestBlend,
        R::BlendOp,
        R::BlendColor,
        R::ColorWriteEnable,
        R::AlphaTestEnable,
        R::StencilEnable,
        R::FogEnable,
        R::FillMode,
        R::ShadeMode,
        R::CullMode,
        R::FrontFace,
    ]
};

/// A canonical render state's raw value now (`d3d::nv2a`'s numbering).
fn raw_state<B: d3d::backend::Backend>(
    d: &d3d::Device<B>,
    mem: &dyn d3d::GuestMemory,
    canonical: u32,
) -> Option<u32> {
    let guest = gfx::xdk::RenderStateTable::default().canonical_to_guest(canonical)?;
    d.render_state(mem, guest)
}

/// Every render-state word the reservation reads, and the clip area and
/// viewport: read before the device drains and again after it, so a
/// state the drain wrote (one the game pushed itself) is never drawn
/// with the value it held before.
fn state_words<B: d3d::backend::Backend>(
    d: &d3d::Device<B>,
    mem: &dyn d3d::GuestMemory,
) -> (Vec<Option<u32>>, Option<[u32; 4]>, [u32; 8]) {
    use gfx::state::RenderStateType as R;
    let table = gfx::xdk::RenderStateTable::default();
    let mut slots: Vec<u32> = (0..57)
        .chain([d3d::state::slot::PS_TEXTURE_MODES])
        .collect();
    for t in [
        R::StencilEnable,
        R::StencilFunc,
        R::StencilRef,
        R::StencilMask,
        R::StencilWriteMask,
        R::StencilFail,
        R::StencilZFail,
        R::StencilPass,
    ] {
        slots.extend(table.canonical_to_guest(t.index()));
    }
    for c in [STIPPLE_ENABLE, SPECULAR_ENABLE] {
        slots.extend(table.canonical_to_guest(c));
    }
    let mut words: Vec<Option<u32>> = slots.into_iter().map(|g| d.render_state(mem, g)).collect();
    words.extend(TEXT_STATES.iter().map(|t| {
        table
            .canonical_to_guest(t.index())
            .and_then(|g| d.render_state(mem, g))
    }));
    let clip = match d.clip_area(mem) {
        Ok(Some((r, rects))) if rects.is_empty() => Some([r.x, r.y, r.width, r.height]),
        _ => None,
    };
    (words, clip, d.bound_target())
}

/// `D3DRS_STIPPLEENABLE` and `D3DRS_SPECULARENABLE` (`d3d::nv2a`'s `rs`).
const STIPPLE_ENABLE: u32 = 83;
const SPECULAR_ENABLE: u32 = 103;

/// `0xAARRGGBB` as a blend constant.
fn blend_constant(argb: u32) -> wgpu::Color {
    let c = |shift: u32| f64::from((argb >> shift) & 0xff) / 255.0;
    wgpu::Color {
        r: c(16),
        g: c(8),
        b: c(0),
        a: c(24),
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Glue<B> {
    /// The glyph cache's texels for `batch`, mirrored again only when its
    /// header, texel address or ring moved.
    fn atlas(
        &self,
        mem: &dyn GuestMemory,
        batch: &Batch,
        map: d3d::resource::AddressMap,
        ring: [u32; hud::GLYPH_RING_WORDS],
    ) -> Option<Arc<Atlas>> {
        let m = Ro(mem);
        let header = batch.bitmap.header;
        let data = m.read_u32(header.checked_add(4)?)?;
        let desc = gfx::texture::TextureDesc::from_header(
            m.read_u32(header.checked_add(0xc)?)?,
            m.read_u32(header.checked_add(0x10)?)?,
        )
        .ok()?;
        if desc.format != gfx::D3DFormat::LinA4R4G4B4
            || desc.kind != gfx::texture::TextureKind::D2
            || desc.width != hud::GLYPH_SIZE
            || desc.height != hud::GLYPH_SIZE
        {
            return None;
        }
        let key = (header, data, ring);
        let mut mirror = self.mirror.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(m) = mirror.as_ref()
            && m.key == key
        {
            return Some(Arc::clone(&m.atlas));
        }
        let pitch = desc.pitch as usize;
        let side = hud::GLYPH_SIZE as usize;
        let mut texels = vec![0u8; pitch * side];
        mem.read(map.to_virtual(data), &mut texels).ok()?;
        let atlas = Arc::new(Atlas::new(hud::a4r4g4b4_to_rgba(
            &texels, side, side, pitch,
        )?));
        *mirror = Some(Mirror {
            key,
            atlas: Arc::clone(&atlas),
        });
        Some(atlas)
    }

    /// Reserves a segment of `batch` at `quad` (not skipped yet): the
    /// device drains what came before, records the pass and commits it.
    fn reserve(
        &self,
        mem: &mut dyn GuestMemory,
        batch: &Batch,
        quad: &[hud::Vertex; 4],
    ) -> Result<Open, (Kept, String)> {
        let k = |k: Kept| (k, String::new());
        let Ok(mut d) = self.device.lock() else {
            return Err(k(Kept::Reservation));
        };
        if !d.can_external_pass() {
            return Err(k(Kept::NoExternal));
        }
        let (psdef, ring) = {
            let m = Ro(mem);
            let mut psdef = [0u32; hud::COMBINER_WORDS];
            for (i, w) in psdef.iter_mut().enumerate() {
                *w = m
                    .read_u32(hud::COMBINER + 4 * i as u32)
                    .ok_or(k(Kept::Combiner))?;
            }
            (psdef, hud::glyph_ring(&m).ok_or(k(Kept::Atlas))?)
        };
        let map = d.config().address_map;
        let atlas = self.atlas(mem, batch, map, ring).ok_or(k(Kept::Atlas))?;
        let mut dm = DeviceMemory(mem);
        let before = state_words(&d, &dm);
        let rs = render_state(&d, &dm).ok_or(k(Kept::State))?;
        let stipple = raw_state(&d, &dm, STIPPLE_ENABLE).ok_or(k(Kept::State))?;
        let specular = raw_state(&d, &dm, SPECULAR_ENABLE).ok_or(k(Kept::State))?;
        let stencil = d.stencil_state(&dm).ok_or(k(Kept::State))?;
        // Depth off writes none either, whatever ZWRITEENABLE holds.
        if rs.depth_mode != gfx::state::DepthBufferMode::Disabled
            || stencil.enable
            || rs.alpha_test_enable
            || rs.fog_enable
            || stipple != 0
            || rs.fill_mode != gfx::state::FillMode::Solid
            || rs.shade_mode == gfx::state::ShadeMode::Flat
        {
            return Err(k(Kept::State));
        }
        let mut combiner = [0u32; 57];
        for (i, w) in combiner.iter_mut().enumerate() {
            *w = d.render_state(&dm, i as u32).ok_or(k(Kept::Combiner))?;
            if code_word(i) && *w != psdef[i] {
                return Err(k(Kept::Combiner));
            }
        }
        let modes = d
            .render_state(&dm, d3d::state::slot::PS_TEXTURE_MODES)
            .ok_or(k(Kept::Combiner))?;
        let def = d3d::nv2a::guest_def(&psdef, &combiner, modes, psdef[8] | psdef[9] != 0)
            .with_default_final_combiner(false, specular != 0);
        let program = text_program(&def).map_err(|e| (Kept::Program, format!(": {e}")))?;
        let clip = match d.clip_area(&dm) {
            Ok(Some((r, rects))) if rects.is_empty() => [r.x, r.y, r.width, r.height],
            _ => return Err(k(Kept::Clip)),
        };
        let v = d.viewport();
        let viewport = [v.x as f32, v.y as f32, v.width as f32, v.height as f32];
        let mut primitive = gfx::translate::primitive_state(
            &rs,
            wgpu::PrimitiveTopology::TriangleList,
            gfx::translate::Capabilities::ALL,
        );
        primitive.unclipped_depth = false;
        let draw = Arc::new(TextDraw {
            program,
            state: TextState {
                blend: gfx::translate::blend_state(&rs),
                writes: gfx::translate::color_writes(rs.color_write),
                primitive,
                point: batch.point,
            },
            blend_constant: blend_constant(rs.blend_color),
            atlas,
            clip,
            first: hud::text_quad(batch, viewport, quad),
            segment: Arc::new(Segment::new()),
        });
        let plan = d3d::ExternalPlan {
            stage: timeline::native::Stage::Hud,
            view: 0,
            ranges: 1,
            slots: Vec::new(),
        };
        let pass = TextPass {
            draw: Arc::clone(&draw),
            gpu: Arc::clone(&self.gpu),
        };
        match d.prepare_external_pass(&mut dm, plan, Box::new(pass)) {
            Ok(Some(p)) => p.commit(),
            _ => return Err(k(Kept::Reservation)),
        }
        if state_words(&d, &dm) != before {
            // The drain changed what the pass was recorded with: it draws
            // nothing (the write lands before its commands, as an
            // append's does) and the quad keeps its D3D draw.
            draw.segment.void();
            return Err(k(Kept::Drained));
        }
        SEGMENTS.fetch_add(1, Ordering::Relaxed);
        NATIVE_QUADS.fetch_add(1, Ordering::Relaxed);
        Ok(Open {
            batch: *batch,
            draw,
            viewport,
            stats: d.stats(),
            target: d.bound_target(),
            ring,
        })
    }

    /// Whether `quad` joins `open` (written into its segment, so skipped):
    /// only while nothing the segment's reservation read could have moved
    /// and no submission could have come since.
    fn extend(&self, mem: &dyn GuestMemory, open: &Open, quad: &[hud::Vertex; 4]) -> bool {
        if hud::glyph_ring(&Ro(mem)) != Some(open.ring) {
            return false;
        }
        {
            let Ok(d) = self.device.lock() else {
                return false;
            };
            if d.stats() != open.stats || d.bound_target() != open.target {
                return false;
            }
        }
        if !open
            .draw
            .segment
            .append(&hud::text_quad(&open.batch, open.viewport, quad))
        {
            return false;
        }
        NATIVE_QUADS.fetch_add(1, Ordering::Relaxed);
        true
    }

    /// The native side of one screen quad: whether it is drawn natively
    /// (so skipped).
    fn text_quad(&self, mem: &mut dyn GuestMemory, corners: u32) -> bool {
        let mut text = self.text.lock().unwrap_or_else(|e| e.into_inner());
        let batch = match &*text {
            Text::Idle => return false,
            Text::Planned(b) => *b,
            Text::Open(o) => o.batch,
        };
        let Some(quad) = hud::read_quad(&Ro(mem), corners) else {
            kept(Kept::Quad, "");
            *text = Text::Idle;
            return false;
        };
        if let Text::Open(o) = &*text
            && self.extend(mem, o, &quad)
        {
            return true;
        }
        match self.reserve(mem, &batch, &quad) {
            Ok(open) => {
                *text = Text::Open(open);
                true
            }
            Err((k, detail)) => {
                kept(k, &detail);
                *text = Text::Idle;
                false
            }
        }
    }
}

impl<B: d3d::backend::Backend + Send + 'static> Host for Glue<B> {
    fn begin(&self, mem: &mut dyn GuestMemory, record: u32) {
        let plan = Batch::read(&Ro(mem), record);
        match &plan {
            Ok(_) => {
                PLANNED.fetch_add(1, Ordering::Relaxed);
            }
            Err(r) => refused(*r),
        }
        if self.native {
            *self.text.lock().unwrap_or_else(|e| e.into_inner()) = match plan {
                Ok(b) => Text::Planned(b),
                Err(_) => Text::Idle,
            };
        }
        if let Some(c) = &self.compare
            && c.left.load(Ordering::Relaxed) > 0
        {
            *c.plan.lock().unwrap_or_else(|e| e.into_inner()) = match &plan {
                Ok(b) => format!(
                    "batch at {record:#010x} over {:#010x} ({}x{}, linear {}), {:?}",
                    b.bitmap.at, b.bitmap.width, b.bitmap.height, b.bitmap.linear, b.blend
                ),
                Err(r) => format!("batch at {record:#010x} on D3D: {}", r.describe()),
            };
        }
    }

    fn screen_quad(&self, mem: &mut dyn GuestMemory, corners: u32) -> bool {
        // `0x000363a0` draws nothing while this word is set.
        if Ro(mem)
            .read_u32(hud::QUADS_OFF & !3)
            .is_none_or(|w| (w >> 16) as u16 != 0)
        {
            return false;
        }
        if self.native && self.text_quad(mem, corners) {
            return true;
        }
        if let Some(c) = &self.compare
            && let Some(q) = hud::read_quad(&Ro(mem), corners)
        {
            c.expect(Expect {
                kind: Kind::Screen,
                corners: q.map(|v| [v.x, v.y]),
            });
        }
        false
    }

    fn end(&self, _mem: &mut dyn GuestMemory) {
        if self.native {
            *self.text.lock().unwrap_or_else(|e| e.into_inner()) = Text::Idle;
        }
    }

    fn hud_quad(&self, mem: &mut dyn GuestMemory, call: HudQuadCall) {
        let Some(c) = &self.compare else { return };
        if let Some(q) = HudQuad::read(&Ro(mem), call.corners, call.rect, call.args) {
            c.expect(Expect {
                kind: Kind::Hud,
                corners: q.corners,
            });
        }
    }
}

/// Installs the host over `device`, and with `H2_NATIVE_HUD_COMPARE` the
/// compare's admission on it.
pub(crate) fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) {
    let compare = feature::native_hud::compare_spec().map(|n| {
        Arc::new(Compare {
            pairing: Mutex::new(Pairing::default()),
            left: AtomicU64::new(n),
            plan: Mutex::new(String::new()),
            said: Mutex::new([false; 2]),
        })
    });
    if let Some(c) = &compare
        && let Ok(mut d) = device.lock()
    {
        d.add_draw_admission(Arc::clone(c) as Arc<dyn d3d::DrawAdmission>);
    }
    let native = feature::Switches::from_env().is_ok_and(|s| feature::native_hud::on(&s));
    feature::native_hud::set_host(Arc::new(Glue {
        compare,
        native,
        device: Arc::clone(device),
        text: Mutex::new(Text::Idle),
        mirror: Mutex::new(None),
        gpu: Arc::new(Mutex::new(TextGpu::new())),
    }));
}

#[cfg(test)]
mod tests {
    use super::*;
    use gfx::vertex::{StreamLayout, VertexAttribute};

    fn fan(
        stride: u32,
        offset: u32,
        data_type: VertexDataType,
    ) -> (VertexLayout, Vec<d3d::StreamBytes>) {
        let mut bytes = vec![0u8; 4 * stride as usize];
        for i in 0..4usize {
            let at = i * stride as usize + offset as usize;
            bytes[at..at + 4].copy_from_slice(&(i as f32).to_le_bytes());
            bytes[at + 4..at + 8].copy_from_slice(&(10.0 + i as f32).to_le_bytes());
        }
        let layout = VertexLayout {
            streams: vec![StreamLayout {
                stream: 0,
                attributes: vec![VertexAttribute {
                    register: VertexRegister::POSITION,
                    data_type,
                    offset,
                }],
                stride,
            }],
            pretransformed: false,
        };
        (
            layout,
            vec![d3d::StreamBytes {
                data: bytes.into(),
                stride,
            }],
        )
    }

    #[test]
    fn a_fan_reads_its_positions_at_their_offset() {
        let fan4 = PrimitiveType::TriangleFan;
        let (l, s) = fan(24, 8, VertexDataType::Float4);
        assert_eq!(
            fan_corners(fan4, 4, false, &l, &s),
            Some([[0.0, 10.0], [1.0, 11.0], [2.0, 12.0], [3.0, 13.0]])
        );
        assert_eq!(
            fan_corners(PrimitiveType::TriangleStrip, 4, false, &l, &s),
            None
        );
        assert_eq!(fan_corners(fan4, 3, false, &l, &s), None);
        assert_eq!(fan_corners(fan4, 4, true, &l, &s), None, "indexed");
        let (l, s) = fan(24, 8, VertexDataType::Short2);
        assert_eq!(fan_corners(fan4, 4, false, &l, &s), None, "not floats");
        let (l, mut s) = fan(16, 0, VertexDataType::Float2);
        s[0].data = vec![0u8; 20].into();
        assert_eq!(
            fan_corners(fan4, 4, false, &l, &s),
            None,
            "a stream that ends early"
        );
    }

    #[test]
    fn every_kept_reason_has_its_own_slot() {
        let mut slots = std::collections::HashSet::new();
        for k in Kept::ALL {
            assert!(slots.insert(k.index()), "{k:?}");
            assert!(!k.describe().is_empty());
        }
        assert!(Kept::ALL.len() <= 64, "the logged-once bits fit a word");
    }

    #[test]
    fn the_combiner_check_skips_constants_and_texture_modes() {
        let code: Vec<usize> = (0..57).filter(|&i| code_word(i)).collect();
        assert!(code.contains(&0) && code.contains(&9) && code.contains(&26));
        assert!(code.contains(&42) && code.contains(&45) && code.contains(&56));
        for i in (10..26).chain([43, 44, 54]) {
            assert!(!code_word(i), "{i}");
        }
    }

    #[test]
    fn the_blend_colour_is_argb() {
        let c = blend_constant(0x80ff_0040);
        assert_eq!((c.r, c.g, c.a), (1.0, 0.0, 128.0 / 255.0));
        assert_eq!(c.b, 64.0 / 255.0);
    }

    #[test]
    fn every_refusal_has_its_own_slot() {
        let mut slots = std::collections::HashSet::new();
        for r in Refused::ALL {
            assert!(slots.insert(r.index()), "{r:?}");
        }
        assert!(Refused::ALL.len() <= 64, "the logged-once bits fit a word");
    }
}
