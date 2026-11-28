//! Dynamic decal admission at the actual resolved `D3DDevice_DrawVertices`
//! packet, and the callback's cut (docs/graphics/retire-d3d-decals.md).
//!
//! Where the guest callback runs, its texture setup, duplicate markers and
//! state tails are unchanged. A replacement is returned only after the
//! complete draw resolves; the handoff records it transactionally on both
//! target copies before suppressing D3D. The first draw admitted in a mode
//! of a view (`0x0002c370`'s call) is that mode's [`Template`].
//!
//! Where the callback is cut ([`render_bridge::native_decals::cut`]), each
//! record is drawn from the template with its own vertices, tint (`v10`)
//! and stage-0 texture, the texture decoded from guest memory
//! ([`decode_bitmap`]): the device still holds the last texture the guest
//! bound. Every later draw a guest body makes in that mode is compared with
//! the template and the walk (the arm).

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};

use d3d::backend::{
    DrawAdmission, DrawSnapshot, ExternalAttachments, ExternalDraws, ExternalPlan, ExternalWrites,
};
use d3d::shader::VertexProcessing;
use gfx::vertex::{PrimitiveType, StreamLayout, VertexDataType, VertexRegister};
use engine::memory::GuestMemory;
use native_render::decal::{Mode, VERTEX_SIZE, Vertex, decode_packed};
use render_bridge::native_decals::cut::{self, Expected, Record};
use native_render::decal_gpu::{
    ClipRect, DecalCache, DecalClip, DecalGpu, DecalTarget, PacketTarget, Prepared, ReferenceCopy,
    ShaderSnapshot, StageExtras,
};
use shaders::vsh::VertexUniforms;
use timeline::native::Stage;

use crate::native_lights_glue::DeviceMemory;

const STIPPLE_ENABLE: usize = 83;
const TWO_SIDED_LIGHTING: usize = 141;

/// The per-device caches of the decals' programs, pipelines and texture
/// groups, shared by every admitted draw.
type Caches = Arc<Mutex<HashMap<wgpu::Device, DecalCache>>>;

#[derive(Default)]
struct Glue {
    caches: Caches,
}

/// The first draw admitted in a mode of a view: the state, program,
/// constants and clips every record of that mode draws with.
struct Template {
    /// The `0x0002c370` call it was admitted in ([`cut::mode_call`]).
    call: u64,
    /// The draw as [`prepare`] made it, for the arm's comparison.
    raw: Prepared,
    /// The draw as admitted (with the Rust material): what a
    /// cut record draws, with its own vertices, tint and texture.
    drawn: Prepared,
    writes_depth: bool,
}

static TEMPLATE: Mutex<Option<Arc<Template>>> = Mutex::new(None);

fn template() -> Option<Arc<Template>> {
    TEMPLATE.lock().unwrap_or_else(|e| e.into_inner()).clone().filter(|t| t.call == cut::mode_call())
}

/// Register on the guest's ordered draw, which also owns its fallback,
/// and, unless `native-decals.cut=off`, draw the cut callback's records.
pub fn install<B: d3d::backend::Backend + Send + 'static>(
    device: &d3d::abi::SharedDevice<B>,
) {
    let Ok(switches) = hooks::Switches::from_env() else { return };
    if !render_bridge::native_decals::on(&switches) {
        return;
    }
    let glue = Glue::default();
    if render_bridge::native_decals::cut_on(&switches) {
        cut::set_host(Arc::new(Cut { device: Arc::clone(device), caches: glue.caches.clone() }));
    }
    let Ok(mut d) = device.lock() else { return };
    d.add_draw_admission(Arc::new(glue));
}

/// A stage-0 texture decoded from guest memory as the D3D path decodes it,
/// every mip level of face 0, RGBA8.
pub struct DecalBitmap {
    key: u64,
    /// `(width, height, texels)` per level, level 0 first.
    levels: Vec<(u32, u32, Vec<u8>)>,
    /// A linear texture is fetched in texels ([`ShaderSnapshot::tex_scale`]).
    tex_scale: Option<[f32; 4]>,
}

impl DecalBitmap {
    /// Level 0's width and height.
    pub(crate) fn size(&self) -> (u32, u32) {
        (self.levels[0].0, self.levels[0].1)
    }

    /// A linear texture, fetched in texels.
    pub(crate) fn linear(&self) -> bool {
        self.tex_scale.is_some()
    }
}

/// Decoded bitmaps by key; cleared when it grows past [`BITMAP_LIMIT`].
static BITMAPS: Mutex<Option<HashMap<u64, Arc<DecalBitmap>>>> = Mutex::new(None);
const BITMAP_LIMIT: usize = 256;

/// The texture header's levels (`header` as `0x00012310` returns it):
/// its words `+0` (a texture resource), `+4` (data), `+0xc` (format) and
/// `+0x10` (size). Decoded once per map load and header words.
pub fn decode_bitmap(mem: &dyn GuestMemory, header: u32, map: d3d::AddressMap) -> Result<Arc<DecalBitmap>, &'static str> {
    let w = |o: u32| mem.u32(header.wrapping_add(o)).map_err(|_| "texture_unread");
    let (common, data, format, size) = (w(0)?, w(4)?, w(0xc)?, w(0x10)?);
    if (common >> 16) & 7 != 4 {
        return Err("texture_resource");
    }
    let mut h = std::hash::DefaultHasher::new();
    (render_bridge::native_map_work::generation(), header, [data, format, size]).hash(&mut h);
    let key = h.finish();
    let mut cached = BITMAPS.lock().unwrap_or_else(|e| e.into_inner());
    let cache = cached.get_or_insert_with(HashMap::new);
    if let Some(b) = cache.get(&key) {
        return Ok(Arc::clone(b));
    }
    let desc = gfx::TextureDesc::from_header(format, size).map_err(|_| "texture_format")?;
    if desc.kind != gfx::texture::TextureKind::D2 || desc.format.is_depth() {
        return Err("texture_kind");
    }
    let total = desc.total_size().map_err(|_| "texture_format")?;
    if total > 1 << 24 {
        return Err("texture_size");
    }
    let bytes = mem.bytes(map.to_virtual(data), total).map_err(|_| "texture_unread")?;
    let options = gfx::DecodeOptions { bc_supported: false, signed: [false; 4] };
    let decoded = gfx::decode_texture(&desc, &bytes, None, options).map_err(|_| "texture_decode")?;
    let swap = match decoded.host {
        gfx::HostFormat::Rgba8Unorm => false,
        gfx::HostFormat::Bgra8Unorm => true,
        _ => return Err("texture_host_format"),
    };
    let mut levels: Vec<(u32, u32, Vec<u8>)> = Vec::new();
    for l in decoded.levels.iter().filter(|l| l.face == 0) {
        // Each level halves the last, as the texture's mip chain is made;
        // the chain stops at the first that does not.
        let n = levels.len() as u32;
        let (w0, h0) = levels.first().map_or((l.width, l.height), |f| (f.0, f.1));
        let want = ((w0 >> n).max(1), (h0 >> n).max(1));
        if l.level != n || (l.width, l.height) != want || l.width == 0 || l.height == 0
            || l.data.len() != 4 * l.width as usize * l.height as usize
        {
            break;
        }
        let mut rgba = l.data.clone();
        if swap {
            rgba.chunks_exact_mut(4).for_each(|p| p.swap(0, 2));
        }
        levels.push((l.width, l.height, rgba));
    }
    if levels.is_empty() {
        return Err("texture_decode");
    }
    let tex_scale = (desc.format.layout() == gfx::format::Layout::Linear && desc.width != 0 && desc.height != 0)
        .then_some([1.0 / desc.width as f32, 1.0 / desc.height as f32, 1.0, 1.0]);
    let b = Arc::new(DecalBitmap { key, levels, tex_scale });
    if cache.len() >= BITMAP_LIMIT {
        cache.clear();
    }
    cache.insert(key, Arc::clone(&b));
    Ok(b)
}

/// The GPU copies of decoded bitmaps, by device and key; cleared when it
/// grows past [`BITMAP_LIMIT`].
type Views = HashMap<(wgpu::Device, u64), wgpu::TextureView>;
static VIEWS: Mutex<Option<Views>> = Mutex::new(None);

#[allow(clippy::mutable_key_type)] // wgpu's Device is a handle; it hashes by identity
pub(crate) fn texture_view(device: &wgpu::Device, queue: &wgpu::Queue, b: &DecalBitmap) -> wgpu::TextureView {
    let mut views = VIEWS.lock().unwrap_or_else(|e| e.into_inner());
    let views = views.get_or_insert_with(HashMap::new);
    if let Some(v) = views.get(&(device.clone(), b.key)) {
        return v.clone();
    }
    let (w, h, _) = b.levels[0];
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("native decal bitmap"),
        size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
        mip_level_count: b.levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (lw, lh, texels)) in b.levels.iter().enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo { mip_level: level as u32, ..texture.as_image_copy() },
            texels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * lw), rows_per_image: Some(*lh) },
            wgpu::Extent3d { width: *lw, height: *lh, depth_or_array_layers: 1 },
        );
    }
    let view = texture.create_view(&Default::default());
    if views.len() >= BITMAP_LIMIT {
        views.clear();
    }
    views.insert((device.clone(), b.key), view.clone());
    view
}

/// Draws a cut callback's records from the mode's template.
struct Cut<B: d3d::backend::Backend + Send + 'static> {
    device: d3d::abi::SharedDevice<B>,
    caches: Caches,
}

impl<B: d3d::backend::Backend + Send + 'static> cut::Host for Cut<B> {
    fn draw(&self, mem: &mut dyn GuestMemory, r: &Record) -> Result<(), &'static str> {
        if r.count == 0 {
            // `D3DDevice_DrawVertices` with no vertices draws nothing.
            return Ok(());
        }
        let header = r.texture?;
        let bytes = r.bytes.as_deref().ok_or("vertices_unread")?;
        if bytes.len() != r.count as usize * VERTEX_SIZE {
            return Err("vertices_unread");
        }
        let template = template().ok_or("template")?;
        let mut device = self.device.lock().map_err(|_| "device")?;
        let texture = decode_bitmap(&*mem, header, device.config().address_map)?;
        let mut prepared = template.drawn.clone();
        prepared.vertices = vertices_of(bytes);
        prepared.shader.vertex.defaults[10] = cut::tint(r.tint);
        prepared.shader.tex_scale = texture.tex_scale;
        let pass = DecalPass {
            prepared,
            fallback: None,
            writes_depth: template.writes_depth,
            texture: Some(texture),
            run: Some(template.call),
            clip: None,
            gpu: None,
            caches: self.caches.clone(),
            frame: crate::d3d_diag::presents(),
        };
        let plan = ExternalPlan { stage: Stage::Decals, view: 0, ranges: 1, slots: Vec::new() };
        match device.prepare_external_pass(&mut DeviceMemory(mem), plan, Box::new(pass)) {
            Ok(Some(reservation)) => {
                reservation.commit();
                Ok(())
            }
            Ok(None) => Err("reservation"),
            Err(_) => Err("device"),
        }
    }
}

/// What the arm reads of a guest body's resolved decal draw.
struct Drawn<'a> {
    vertex_count: u32,
    /// The texture header bound at stage 0.
    texture: Option<u32>,
    /// The vertex default `v10`.
    tint: [f32; 4],
    /// The one stream's bytes.
    vertices: Option<&'a [u8]>,
}

impl<'a> Drawn<'a> {
    fn of(d: &'a d3d::ResolvedDraw) -> Self {
        Self {
            vertex_count: d.vertex_count,
            texture: d.texture_info[0].map(|i| i.header),
            tint: d.shader.vertex_defaults[10],
            vertices: d.streams.first().map(|s| &s.data[..]),
        }
    }
}

/// The arm's check of a guest body's draw against the walk: the vertex
/// count, the texture it binds, the tint and the vertices.
fn check_walk(expected: &Expected, d: &Drawn<'_>) -> Vec<String> {
    let r = match expected {
        Expected::Unmodelled => return Vec::new(),
        Expected::Missing => return vec!["unexpected".to_owned()],
        Expected::Next(r) => r,
    };
    let texture = match r.texture {
        Ok(header) => d.texture == Some(header),
        Err(_) => true,
    };
    let vertices = match (&r.bytes, d.vertices) {
        (None, _) => true,
        (Some(b), Some(v)) => b[..] == *v,
        (Some(_), None) => false,
    };
    [
        (d.vertex_count == r.count, "count"),
        (texture, "texture"),
        (d.tint.map(f32::to_bits) == cut::tint(r.tint).map(f32::to_bits), "tint"),
        (vertices, "vertices"),
    ]
    .into_iter()
    .filter(|(same, _)| !same)
    .map(|(_, f)| f.to_owned())
    .collect()
}

/// The arm's check of a draw against its mode's template: everything a
/// cut record takes from the template.
fn check_template(t: &Prepared, p: &Prepared) -> Vec<String> {
    let (a, b) = (&t.shader, &p.shader);
    let mut vertex = b.vertex.clone();
    vertex.defaults[10] = a.vertex.defaults[10];
    let reference = match (&t.reference, &p.reference) {
        (None, None) => true,
        (Some(x), Some(y)) => *x.constants == *y.constants && x.clips == y.clips,
        _ => false,
    };
    [
        (a.render == b.render, "render"),
        (a.stages == b.stages, "stages"),
        (a.extras == b.extras, "extras"),
        (a.combiner == b.combiner, "combiner"),
        (a.microcode == b.microcode, "program"),
        (a.provided_inputs == b.provided_inputs, "inputs"),
        (a.two_sided_lighting == b.two_sided_lighting, "two_sided"),
        (a.stipple == b.stipple, "stipple"),
        (vertex == a.vertex, "vertex"),
        (a.w_buffered == b.w_buffered, "w_buffer"),
        (a.integer_depth == b.integer_depth, "depth_format"),
        (t.clips == p.clips, "clips"),
        (reference, "reference"),
    ]
    .into_iter()
    .filter(|(same, _)| !same)
    .map(|(_, f)| f.to_owned())
    .collect()
}

fn clip(r: d3d::PixelRect) -> ClipRect {
    ClipRect {
        x: r.x,
        y: r.y,
        width: r.width,
        height: r.height,
    }
}

/// The resolved draw's vertices, decoded from its one stream. Halo 2's decal
/// declaration (the attribute list at `0x0043f7b9`, bound with stride 16) is
/// float3 position at `+0` in `v0` and `NORMPACKED3` at `+12` in the second declared register: an
/// 11:11:10 signed normalised dword, decoded as the NV2A fetches it
/// ([`decode_packed`]). Any other declaration is refused.
fn decal_vertices(
    layout: &StreamLayout,
    stream: &d3d::backend::StreamBytes,
    vertex_count: u32,
) -> Result<Vec<Vertex>, &'static str> {
    if layout.stride != VERTEX_SIZE as u32
        || stream.stride != VERTEX_SIZE as u32
        || layout.attributes.len() != 2
    {
        return Err("vertex declaration");
    }
    let position = layout.attributes.iter().any(|a| {
        (a.register, a.data_type, a.offset) == (VertexRegister::POSITION, VertexDataType::Float3, 0)
    });
    let packed = layout.attributes.iter().any(|a| {
        (1..16).contains(&a.register.0)
            && a.data_type == VertexDataType::NormPacked3
            && a.offset == 12
    });
    if !position || !packed {
        return Err("vertex declaration");
    }
    if stream.data.len() != vertex_count as usize * VERTEX_SIZE {
        return Err("vertex bytes");
    }
    Ok(vertices_of(&stream.data))
}

/// Halo 2's decal vertices from their bytes: float3 position, then the
/// `NORMPACKED3` word ([`decode_packed`]).
fn vertices_of(bytes: &[u8]) -> Vec<Vertex> {
    bytes
        .as_chunks::<VERTEX_SIZE>()
        .0
        .iter()
        .map(|v| Vertex {
            position: [
                f32::from_bits(u32::from_le_bytes(v[0..4].try_into().expect("position"))),
                f32::from_bits(u32::from_le_bytes(v[4..8].try_into().expect("position"))),
                f32::from_bits(u32::from_le_bytes(v[8..12].try_into().expect("position"))),
            ],
            diffuse: decode_packed(v[12..16].try_into().expect("packed")),
        })
        .collect()
}

/// Filled quads use the live polygon mask. A pushed NV2A control word
/// takes precedence over the D3D state shadow, as it does on the D3D path.
fn decal_stipple(hw: &d3d::hw::HwState, raw_enable: u32) -> Option<[u32; 32]> {
    (hw.stipple_control.unwrap_or(raw_enable) != 0).then_some(hw.stipple_pattern)
}

fn prepare(mode: Mode, snapshot: &DrawSnapshot<'_>) -> Result<Prepared, &'static str> {
    let d = snapshot.draw;
    if d.primitive != PrimitiveType::QuadList
        || d.indices.is_some()
        || d.vertex_count == 0
        || d.vertex_count % 4 != 0
    {
        return Err("geometry");
    }
    if d.layout.streams.len() != 1 || d.streams.len() != 1 {
        return Err("vertex streams");
    }
    let vertices = decal_vertices(&d.layout.streams[0], &d.streams[0], d.vertex_count)?;
    let VertexProcessing::Program { microcode, .. } = &d.shader.vertex else {
        return Err("vertex program");
    };
    if microcode.len() % 4 != 0 || microcode.is_empty() || microcode.len() > 136 * 4 {
        return Err("vertex program");
    }
    let mut code = vec![0x78, 0x20];
    code.extend_from_slice(&((microcode.len() / 4) as u16).to_le_bytes());
    for word in microcode.iter() {
        code.extend_from_slice(&word.to_le_bytes());
    }
    let pixel = d.shader.pixel_shader.as_ref().ok_or("combiner")?;
    let combiner_state: &[u32; 57] = &d.shader.combiner_state;
    let combiner = d3d::nv2a::guest_def(
        pixel,
        combiner_state,
        d.raw.render[136],
        d.shader.final_combiner_latch,
    );
    let extras = d.extras.map(|e| StageExtras {
        color_key_op: e.color_key_op,
        color_sign: e.color_sign,
        alpha_kill: e.alpha_kill,
        texture_transform_flags: e.texture_transform_flags,
        bump_env_mat: e.bump_env_mat,
        bump_env_lscale: e.bump_env_lscale,
        bump_env_loffset: e.bump_env_loffset,
    });
    native_render::decal_gpu::check_state(mode, &d.state.render, &extras)?;
    if d.texture_info[0]
        .is_some_and(|info| info.kind != gfx::texture::TextureKind::D2 || info.format.is_depth())
    {
        return Err("texture kind");
    }
    let tex_scale = d.texture_info[0].and_then(|info| {
        (info.format.layout() == gfx::format::Layout::Linear && info.width != 0 && info.height != 0)
            .then_some([1.0 / info.width as f32, 1.0 / info.height as f32, 1.0, 1.0])
    });
    let shader = ShaderSnapshot {
        render: d.state.render,
        stages: d.state.stages,
        extras,
        combiner,
        microcode: code,
        provided_inputs: d.shader.provided_inputs,
        two_sided_lighting: d.raw.render[TWO_SIDED_LIGHTING] != 0,
        stipple: decal_stipple(&d.hw, d.raw.render[STIPPLE_ENABLE]),
        vertex: VertexUniforms {
            constants: *d.shader.constants,
            defaults: d.shader.vertex_defaults,
            surface: [d.target.width as f32, d.target.height as f32],
            z_max: gfx::renderer::d3dz_max(d.target.depth_format),
            ui_transform: d
                .ui_fit
                .map_or(d3d::backend::UiFit::IDENTITY.transform, |fit| fit.transform),
            ..Default::default()
        },
        w_buffered: d3d::hw::w_buffered(d),
        integer_depth: !matches!(
            d.target.depth_format,
            Some(
                gfx::D3DFormat::F16
                    | gfx::D3DFormat::LinF16
                    | gfx::D3DFormat::F24S8
                    | gfx::D3DFormat::LinF24S8
            )
        ),
        tex_scale,
    };
    let clips = if d.clip_rects.is_empty() {
        d.clip.as_slice()
    } else {
        &d.clip_rects
    };
    let reference = (d.retail_constants.is_some() || d.ui_fit.is_some()).then(|| ReferenceCopy {
        constants: d
            .retail_constants
            .as_ref()
            .map_or(&d.shader.constants, |(constants, _)| constants)
            .clone(),
        clips: clips.iter().copied().map(clip).collect(),
    });
    Ok(Prepared {
        mode,
        vertices,
        shader,
        clips: clips.iter().map(|&r| clip(d.display_rect(r))).collect(),
        reference,
    })
}

/// The first of the four constant rows a program's position goes
/// through. `None` when the microcode does not decode to rows.
fn position_base(microcode: &[u32]) -> Option<u8> {
    use d3d::clip_space::epilogue::{Position, position};
    match position(microcode) {
        Position::Rows(b) => Some(b),
        _ => None,
    }
}

/// Builds the material of every admitted draw from Rust: the combiner
/// from the mode's material and the position matrix rows from the open
/// view, instead of the guest's own words. A draw Rust cannot build is a
/// named refusal: `material_program` (no vertex program, or one whose
/// position does not go through four rows) or `material_view` (no open
/// view).
fn apply_rust(prepared: &mut Prepared, mode: Mode, d: &d3d::ResolvedDraw) -> Result<(), &'static str> {
    let VertexProcessing::Program { microcode, .. } = &d.shader.vertex else {
        return Err("material_program");
    };
    let base = position_base(microcode).ok_or("material_program")?;
    let view = render_bridge::native_decals::view().ok_or("material_view")?;
    let material = native_render::decal_material::material(mode, render_bridge::native_decals::offset_bits());
    prepared.shader.combiner = d3d::nv2a::guest_def(
        &material.combiner,
        &d.shader.combiner_state,
        d.raw.render[136],
        d.shader.final_combiner_latch,
    );
    for (i, row) in view.rows.iter().enumerate() {
        let c = prepared.shader.vertex.constants.get_mut(usize::from(base) + i).ok_or("material_program")?;
        *c = row.map(f32::from_bits);
    }
    Ok(())
}

impl DrawAdmission for Glue {
    fn admit(
        &self,
        snapshot: &DrawSnapshot<'_>,
    ) -> Option<(ExternalPlan, Box<dyn d3d::ExternalPass>)> {
        let (mode, _) = render_bridge::native_decals::current()?;
        if snapshot.draw.entry != "D3DDevice_DrawVertices" {
            return None;
        }
        render_bridge::native_decals::note_examined();
        // The arm: this draw against the cut's walk and its mode's template.
        let expected = cut::take_expected();
        let modelled = expected != Expected::Unmodelled;
        let mut fields = check_walk(&expected, &Drawn::of(snapshot.draw));
        let mut prepared = match prepare(mode, snapshot) {
            Ok(p) => p,
            Err(why @ ("vertex declaration" | "vertex streams")) => {
                if modelled {
                    cut::mismatch(&fields);
                }
                let d = snapshot.draw;
                let strides: Vec<_> = d.streams.iter().map(|s| s.stride).collect();
                render_bridge::native_decals::note_admission_refusal(format!(
                    "{why}: {:?}, bound strides {strides:?}", d.layout
                ));
                return None;
            }
            Err(why) => {
                if modelled {
                    cut::mismatch(&fields);
                }
                render_bridge::native_decals::note_admission_refusal(why);
                return None;
            }
        };
        let call = cut::mode_call();
        let held = template();
        if modelled {
            if let Some(t) = &held {
                fields.extend(check_template(&t.raw, &prepared));
            }
            cut::mismatch(&fields);
        }
        // The mode's first admitted draw is its template.
        let raw = held.is_none().then(|| prepared.clone());
        render_bridge::native_decals::note_candidate();
        // Every admitted draw takes the Rust material; there is no guest
        // comparison draw.
        if let Err(why) = apply_rust(&mut prepared, mode, snapshot.draw) {
            render_bridge::native_decals::note_admission_refusal(why);
            return None;
        }
        let writes_depth = snapshot.draw.writes_depth_stencil();
        if let Some(raw) = raw {
            *TEMPLATE.lock().unwrap_or_else(|e| e.into_inner()) =
                Some(Arc::new(Template { call, raw, drawn: prepared.clone(), writes_depth }));
        }
        Some((
            ExternalPlan {
                stage: Stage::Decals,
                view: 0,
                ranges: 1,
                slots: Vec::new(),
            },
            Box::new(DecalPass {
                prepared,
                fallback: Some(Arc::new(snapshot.draw.clone())),
                writes_depth,
                texture: None,
                run: None,
                clip: None,
                gpu: None,
                caches: self.caches.clone(),
                frame: crate::d3d_diag::presents(),
            }),
        ))
    }
}

struct DecalPass {
    prepared: Prepared,
    /// The guest's resolved draw, which D3D draws when a copy refuses;
    /// `None` for a cut record, which has no guest draw.
    fallback: Option<Arc<d3d::ResolvedDraw>>,
    writes_depth: bool,
    /// A cut record's own stage-0 texture: the device still holds the last
    /// one the guest bound.
    texture: Option<Arc<DecalBitmap>>,
    /// A cut record's `0x0002c370` call: its passes sample no guest
    /// texture, so a run of them pins its attachments once.
    run: Option<u64>,
    clip: Option<d3d::DepthClip>,
    gpu: Option<DecalGpu>,
    caches: Caches,
    /// The guest frame of the admission: the cache keeps a texture group
    /// only while draws of recent frames use it.
    frame: u64,
}

impl DecalPass {
    fn prepare(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        if self.gpu.is_none() {
            self.gpu = Some(DecalGpu::new(t.device, &self.prepared)?);
        }
        if self.gpu.as_ref().is_some_and(|g| g.sampled()) && t.stages[0].is_none() && self.texture.is_none() {
            return Err("texture view");
        }
        Ok(())
    }
}

impl d3d::ExternalPass for DecalPass {
    fn resolved_fallback(&self) -> Option<Arc<d3d::ResolvedDraw>> {
        self.fallback.clone()
    }

    fn batch_key(&self) -> Option<u64> {
        self.run
    }

    fn writes(&self) -> ExternalWrites {
        ExternalWrites {
            color: true,
            depth_stencil: self.writes_depth,
        }
    }

    fn depth_clip(&mut self, clip: d3d::DepthClip) {
        self.clip = Some(clip);
    }

    fn prepare_copy(&mut self, t: &d3d::ExternalTarget<'_>) -> Result<(), &'static str> {
        self.prepare(t)
    }

    /// The draw's packets appended to the D3D pass open on these
    /// attachments, from the device's cached program and pipeline: no
    /// command buffer, render pass, shader or buffer of its own. `None`
    /// (the isolated recording below) when the guest clipped it all away.
    fn record_draws(
        &mut self,
        t: &ExternalAttachments,
        request: d3d::ExternalPassRequest,
    ) -> Option<Result<ExternalDraws, &'static str>> {
        let refuse = |why: &'static str| {
            render_bridge::native_decals::note_refusal(why);
            Some(Err(why))
        };
        let Some(clip) = self.clip else {
            return refuse("issue snapshot");
        };
        let clip = DecalClip {
            range: clip.range,
            clamp: clip.clamp,
            w_buffered: clip.w_buffered,
        };
        let own = self.texture.as_ref().map(|b| texture_view(&t.device, &t.queue, b));
        // A decal sampling the target it draws into cannot share its pass.
        let stage0 = own.as_ref().or(t.stages[0].as_ref());
        if stage0.is_some_and(|v| [t.color.texture(), t.depth.texture()].contains(&v.texture())) {
            return refuse("texture is the target");
        }
        let target = PacketTarget {
            color_format: t.color_format,
            depth_format: t.depth_format,
            guest_size: request.surface_size,
            size: t.size,
            z_scale: request.z_scale,
            stage0,
            copy: t.copy,
        };
        let mut caches = self.caches.lock().unwrap_or_else(|e| e.into_inner());
        let cache = caches.entry(t.device.clone()).or_default();
        cache.begin_frame(self.frame);
        match cache.draws(&t.device, &t.queue, &t.uploads, &self.prepared, clip, &target) {
            Ok(Some((packets, discards))) => Some(Ok(ExternalDraws {
                ranges: 1,
                packets,
                discards,
                writes: Vec::new(),
            })),
            Ok(None) => None,
            Err(why) => refuse(why),
        }
    }

    fn record_ranges(&mut self, t: d3d::ExternalTarget<'_>) -> usize {
        if let Err(why) = self.prepare(&t) {
            render_bridge::native_decals::note_refusal(why);
            return 0;
        }
        let Some(clip) = self.clip else {
            render_bridge::native_decals::note_refusal("issue snapshot");
            return 0;
        };
        let clip = DecalClip {
            range: clip.range,
            clamp: clip.clamp,
            w_buffered: clip.w_buffered,
        };
        let own = self.texture.as_ref().map(|b| texture_view(t.device, t.queue, b));
        let mut stages: [Option<&wgpu::TextureView>; 4] = t.stages;
        if let Some(view) = &own {
            stages[0] = Some(view);
        }
        let mut target = DecalTarget {
            device: t.device,
            queue: t.queue,
            encoder: &mut *t.encoder,
            color: t.color,
            color_format: t.color_format,
            depth: t.depth,
            depth_format: t.depth_format,
            viewport: t.viewport,
            z_scale: t.z_scale,
            guest_size: t.guest_size,
            size: t.size,
            stages,
            copy: t.copy,
        };
        usize::from(
            self.gpu
                .as_mut()
                .expect("prepared above")
                .encode(&mut target, clip),
        )
    }
}

#[cfg(test)]
mod arm_tests {
    use super::*;

    fn prepared() -> Prepared {
        Prepared {
            mode: Mode::Alpha0,
            vertices: Vec::new(),
            shader: ShaderSnapshot {
                render: gfx::state::RenderState::default(),
                stages: std::array::from_fn(|n| gfx::state::TextureStageState::default_for_stage(n as u32)),
                extras: [StageExtras::default(); 4],
                combiner: shaders::psh::PixelShaderDef::default(),
                microcode: vec![0x78, 0x20, 0, 0],
                provided_inputs: 0b11,
                two_sided_lighting: false,
                stipple: None,
                vertex: VertexUniforms::default(),
                w_buffered: true,
                integer_depth: false,
                tex_scale: None,
            },
            clips: vec![ClipRect { x: 0, y: 0, width: 640, height: 480 }],
            reference: None,
        }
    }

    fn record(bytes: &[u8]) -> Record {
        Record {
            at: 0x1000,
            owner: 0x1000,
            marker: 0,
            decal: 0xe600_0001,
            image: 0,
            tint: 0x80ff_0033,
            start: 0,
            count: 4,
            texture: Ok(0xa000),
            bytes: Some(Arc::from(bytes)),
        }
    }

    #[test]
    fn a_draw_matching_the_walk_and_template_is_clean() {
        let bytes = [7u8; 64];
        let drawn = Drawn { vertex_count: 4, texture: Some(0xa000), tint: cut::tint(0x80ff_0033), vertices: Some(&bytes) };
        assert!(check_walk(&Expected::Next(record(&bytes)), &drawn).is_empty());
        // An unmodelled walk checks nothing.
        assert!(check_walk(&Expected::Unmodelled, &Drawn { vertex_count: 0, ..drawn }).is_empty());
        // Another record of the mode differs from the template only in
        // its tint.
        let template = prepared();
        let mut other = prepared();
        other.shader.vertex.defaults[10] = [1.0; 4];
        assert!(check_template(&template, &other).is_empty());
    }

    #[test]
    fn a_draw_that_differs_from_the_walk_or_template_is_named_by_field() {
        let bytes = [7u8; 64];
        let drawn = Drawn { vertex_count: 8, texture: Some(0xb000), tint: [0.0; 4], vertices: Some(&bytes[..32]) };
        assert_eq!(check_walk(&Expected::Next(record(&bytes)), &drawn), ["count", "texture", "tint", "vertices"]);
        assert_eq!(check_walk(&Expected::Missing, &drawn), ["unexpected"]);
        let template = prepared();
        let mut other = prepared();
        other.shader.stages[0] = gfx::state::TextureStageState::default_for_stage(1);
        other.shader.vertex.constants[4] = [2.0; 4];
        other.clips.clear();
        assert_eq!(check_template(&template, &other), ["stages", "vertex", "clips"]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_program_reports_its_matrix_base() {
        use shaders::vsh::encode::{Inst, Operand as O};
        use shaders::vsh::{MacOp, OutReg};
        let mut p = Vec::new();
        for (i, mask) in [(0u8, 8u8), (1, 4), (2, 2), (3, 1)] {
            p.push(
                Inst::mac(MacOp::Dp4, Some(O::v(0)), Some(O::c(10 + i)), None)
                    .to_out(false, OutReg::Pos, mask),
            );
        }
        p.push(Inst::mac(MacOp::Mov, Some(O::c(40)), None, None).to_out(false, OutReg::T(0), 15));
        let last = p.len() - 1;
        let mut words: Vec<u32> = p.iter().flat_map(|i| i.words()).collect();
        words[last * 4 + 3] |= 1;
        assert_eq!(position_base(&words), Some(10));
    }

    use super::*;
    use d3d::backend::{StreamBytes, StreamData};
    use gfx::vertex::VertexAttribute;

    /// Fixture for the v3 binding of the packed decal field:
    /// `v0` float3 at +0, `v3` `NORMPACKED3` at +12, stride 16.
    fn declaration() -> StreamLayout {
        StreamLayout {
            stream: 0,
            attributes: vec![
                VertexAttribute {
                    register: VertexRegister::POSITION,
                    data_type: VertexDataType::Float3,
                    offset: 0,
                },
                VertexAttribute {
                    register: VertexRegister::DIFFUSE,
                    data_type: VertexDataType::NormPacked3,
                    offset: 12,
                },
            ],
            stride: 16,
        }
    }

    fn stream(vertices: &[[u8; VERTEX_SIZE]], stride: u32) -> StreamBytes {
        StreamBytes {
            data: StreamData::Owned(vertices.concat()),
            stride,
        }
    }

    fn vertex(position: [f32; 3], packed: u32) -> [u8; VERTEX_SIZE] {
        let mut v = [0; VERTEX_SIZE];
        for (i, f) in position.iter().enumerate() {
            v[4 * i..4 * i + 4].copy_from_slice(&f.to_le_bytes());
        }
        v[12..16].copy_from_slice(&packed.to_le_bytes());
        v
    }

    #[test]
    fn decal_stipple_uses_hardware_control_before_the_raw_state() {
        let mut hw = d3d::hw::HwState::default();
        hw.stipple_pattern[10] = 1 << 11;

        assert_eq!(decal_stipple(&hw, 0), None);
        assert_eq!(decal_stipple(&hw, 1), Some(hw.stipple_pattern));
        hw.stipple_control = Some(0);
        assert_eq!(decal_stipple(&hw, 1), None);
        hw.stipple_control = Some(1);
        assert_eq!(decal_stipple(&hw, 0), Some(hw.stipple_pattern));
        hw.stipple_pattern = [0; 32];
        assert_eq!(decal_stipple(&hw, 0), Some([0; 32]));
    }

    #[test]
    fn halo2s_declaration_resolves_native_and_decodes() {
        let vs = [
            vertex([1.0, 2.0, 3.0], 0x0000_0000),
            vertex([-4.0, 0.5, 1e3], 0x7fdf_fbff),
            vertex([0.0, -1.0, 7.25], 0x8020_0400),
        ];
        let got = decal_vertices(&declaration(), &stream(&vs, 16), 3).unwrap();
        assert_eq!(
            got,
            [
                Vertex { position: [1.0, 2.0, 3.0], diffuse: [0.0, 0.0, 0.0] },
                Vertex { position: [-4.0, 0.5, 1e3], diffuse: [1.0, 1.0, 1.0] },
                Vertex { position: [0.0, -1.0, 7.25], diffuse: [-1.0, -1.0, -1.0] },
            ]
        );
    }

    #[test]
    fn packed_semantic_decodes_at_each_non_position_register() {
        let mut layout = declaration();
        let bytes = stream(&[vertex([2., -1., 7.], 1)], 16);
        for register in 1..16 {
            layout.attributes[1].register = VertexRegister(register);
            let got = decal_vertices(&layout, &bytes, 1).unwrap();
            assert_eq!(got[0].position, [2., -1., 7.]);
            assert_eq!(got[0].diffuse, [1. / 1023., 0., 0.]);
        }
        for register in [0, 16] {
            layout.attributes[1].register = VertexRegister(register);
            assert_eq!(decal_vertices(&layout, &bytes, 1), Err("vertex declaration"));
        }
    }

    #[test]
    fn other_declarations_are_refused() {
        let vs = [vertex([0.0; 3], 0)];
        // D3DCOLOR instead of NORMPACKED3 (the old assumption).
        let mut layout = declaration();
        layout.attributes[1].data_type = VertexDataType::D3dColor;
        assert_eq!(
            decal_vertices(&layout, &stream(&vs, 16), 1),
            Err("vertex declaration")
        );
        // A third attribute on a wider vertex.
        let mut layout = declaration();
        layout.attributes.push(VertexAttribute {
            register: VertexRegister::TEXCOORD0,
            data_type: VertexDataType::Float2,
            offset: 16,
        });
        layout.stride = 24;
        assert_eq!(
            decal_vertices(&layout, &stream(&vs, 24), 1),
            Err("vertex declaration")
        );
        // The stream's own stride disagrees.
        assert_eq!(
            decal_vertices(&declaration(), &stream(&vs, 20), 1),
            Err("vertex declaration")
        );
        // The bytes do not cover the vertices.
        let short = StreamBytes { data: StreamData::Owned(vec![0; 12]), stride: 16 };
        assert_eq!(decal_vertices(&declaration(), &short, 1), Err("vertex bytes"));
    }
}
