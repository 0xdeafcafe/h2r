//! Stage 5's screen-effect pass on the GPU (docs/graphics/native-post-hud.md,
//! "Stage 5", "The screen effects"): a translated layer-15 template pass
//! ([`crate::screen_look::compile_screen`]) drawing the strips
//! `0x0002d140` pushes by hand, into the target bound for the call or into
//! target 1.
//!
//! What a strip is, from the game (Code, `engine::screen_effect`): per row
//! of corners one triangle strip (`SET_BEGIN_END` 6), per column the
//! corner in this row and the one below, each vertex attribute 0 the
//! position (x and y in the target's pixels, z and w `[0x0045dd40]`) and
//! attributes 1..4 the four texture coordinates ([`StripVertex`]).
//!
//! Each pass draws into its live D3D surface (target 0, target 1, or
//! a convolution level), loading existing pixels in the attachment's
//! format. A stage sampling target 0 reads an ordered feedback copy.

use std::collections::HashMap;
use std::ops::Range;

use shaders::psh::{self, PixelUniforms};
use shaders::vsh::VertexUniforms;

use crate::glow;
use crate::gpu::{Image, texture};
use crate::program::{bias_signed, sampler_descriptor};
use crate::screen_look::{EngineTexture, ScreenProgram, ScreenStage, engine_texture};

/// One vertex of a strip: attribute 0 the position, 1..4 the texture
/// coordinates (`v0`..`v4`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct StripVertex {
    pub position: [f32; 4],
    pub texcoords: [[f32; 4]; 4],
}

/// The strips' vertex layout: `v<n>` at location `n`.
pub const STRIP_ATTRIBUTES: [wgpu::VertexAttribute; 5] =
    wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4];

/// A strip of a pass, the position's z and w `depth` (f32 bits all).
pub fn strip(vertices: &[([[u32; 4]; 4], [u32; 2])], depth: u32) -> Vec<StripVertex> {
    let f = f32::from_bits;
    vertices
        .iter()
        .map(|(t, p)| StripVertex {
            position: [f(p[0]), f(p[1]), f(depth), f(depth)],
            texcoords: t.map(|c| c.map(f)),
        })
        .collect()
}

/// Each strip's vertex range in one buffer holding them in order.
pub fn strip_ranges(strips: &[Vec<StripVertex>]) -> Vec<Range<u32>> {
    let mut at = 0u32;
    strips
        .iter()
        .map(|s| {
            let r = at..at + s.len() as u32;
            at = r.end;
            r
        })
        .collect()
}

/// The convolutions' targets (`engine::screen_effect`: the first
/// convolution of a pass into `0x19` and `0x1a`, the next into `0x1b` and
/// the one the first did not end in), stood in for in the order given.
pub const STAND_INS: [u32; 3] = [0x19, 0x1a, 0x1b];

/// Which stand-in target `t` is.
pub fn stand_in(t: u32) -> Option<usize> {
    STAND_INS.iter().position(|&s| s == t)
}

/// An engine texture the native pass supplies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// The view's (5): a copy of the bound colour.
    View,
    /// Target 1 (2 and 3): the stand-in an earlier pass of the call drew.
    Target1,
    /// 24 and 25: the target `[0x00485a78 + 4 n]` names, the result of
    /// convolution `n` of the last pass of the call with one.
    Result(usize),
}

/// What engine texture `id` binds in the native pass, `None` when it
/// does not supply it.
pub fn bound(id: u8) -> Option<Bound> {
    match engine_texture(id) {
        EngineTexture::View => Some(Bound::View),
        EngineTexture::Target(1) => Some(Bound::Target1),
        EngineTexture::Slot(a) if a >= RESULTS => Some(Bound::Result(((a - RESULTS) / 4) as usize)),
        _ => None,
    }
}

/// Where the convolutions' results are (`engine::screen_effect::RESULTS`).
pub const RESULTS: u32 = 0x0048_5a78;

/// Whether the native pass can bind engine texture `id` ([`bound`]).
pub fn supplied(id: u8) -> bool {
    bound(id).is_some()
}

/// Whether `program` samples engine texture `b`.
pub fn samples(program: &ScreenProgram, b: Bound) -> bool {
    (0..4).any(|n| program.dims[n].is_some() && matches!(program.stages[n], ScreenStage::Engine(id) if bound(id) == Some(b)))
}

/// Where a pass draws.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dest {
    /// The target bound for the call (`kind` 0).
    Bound,
    /// Target 1 at this guest size (`kind` 1..3: the window's, a half, a
    /// quarter).
    Target1 { guest: (u32, u32) },
}

/// One draw of a convolution: `source` (0 the bound target, 1, a
/// stand-in) bound to stages 0..3, drawn into the stand-in `dest`. The
/// blur's (`engine::glow::BlurDraw`) is a quad with four taps of
/// `spread`; a wide one's (`engine::glow::WideDraw`) its `grid`.
#[derive(Clone, Debug, PartialEq)]
pub struct ConvolutionDraw {
    pub source: u32,
    pub dest: u32,
    pub spread: f32,
    pub grid: Option<ConvolutionGrid>,
}

/// The grid `0x00034d90` draws for a step of a wide convolution
/// (`engine::screen_grid::strips`): its strips, each vertex its position
/// in the stand-in's pixels and its taps in texels of `bound`, the size
/// `0x00014f60` bound the source at (`0x00485af4`/`0x00485b04`), which
/// the combiner's stages are scaled by.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConvolutionGrid {
    pub bound: (u32, u32),
    pub strips: Vec<Vec<StripVertex>>,
}

impl ConvolutionGrid {
    /// Whether it draws: a strip at least, each a triangle at least, the
    /// source bound at a size.
    pub fn check(&self) -> Result<(), ScreenRefused> {
        if self.strips.is_empty() || self.strips.iter().any(|s| s.len() < 3) {
            return Err(ScreenRefused::NoStrips);
        }
        if self.bound.0 == 0 || self.bound.1 == 0 {
            return Err(ScreenRefused::ConvolutionSize);
        }
        Ok(())
    }
}

/// One convolution of a pass, drawn after its strips: the blur
/// `0x000211a0` (`engine::screen_effect::Convolution::Blur`), the wide
/// one `0x00021360` (`Convolution::Wide`), or one kept (no draws, its
/// result the pass's target).
#[derive(Clone, Debug, PartialEq)]
pub struct ScreenConvolution {
    /// The guest size of the stand-ins it draws into.
    pub size: (u32, u32),
    pub draws: Vec<ConvolutionDraw>,
    /// The target its result names (`[0x00485a78 + 4 n]`).
    pub result: u32,
}

/// What the convolutions' quads share in a call: Halo's window (`left,
/// top, right, bottom`), the tap signs (`[[SIGN_CLEAR], [SIGN_SET]]`) and
/// the blur's combiner (`0x00020f30` with strength 0 and `y` 0, as
/// `0x0002d140` calls `0x000211a0`).
#[derive(Clone, Debug, PartialEq)]
pub struct QuadFrame {
    pub window: [f32; 4],
    pub signs: [f32; 2],
    pub combiner: [u32; glow::COMBINER_WORDS],
}

/// What a call has drawn so far (guest sizes): target 1, the stand-ins,
/// and the convolutions' results, which `0x0002d140` clears at the end of
/// every call (Code, `0x0002db5a`), so a call reads only its own.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Drawn {
    pub target1: Option<(u32, u32)>,
    pub stand_ins: [Option<(u32, u32)>; 3],
    pub results: [Option<u32>; 2],
}

impl Drawn {
    /// Target `t`'s guest size (0 the bound target, `view`), `None` when
    /// the call has not drawn it.
    pub fn size(&self, t: u32, view: (u32, u32)) -> Option<(u32, u32)> {
        match t {
            0 => Some(view),
            1 => self.target1,
            _ => stand_in(t).and_then(|i| self.stand_ins[i]),
        }
    }

    /// The target engine texture `id` binds now.
    pub fn target(&self, id: u8) -> Result<u32, ScreenRefused> {
        match bound(id) {
            Some(Bound::View) => Ok(0),
            Some(Bound::Target1) => Ok(1),
            Some(Bound::Result(n)) => self.results.get(n).copied().flatten().ok_or(ScreenRefused::ResultUnset(id)),
            None => Err(ScreenRefused::EngineTexture(id)),
        }
    }

    /// The target each sampled engine stage of a pass into `dest` reads:
    /// each drawn by the call (the bound one always), none target 1 while
    /// the pass draws it.
    pub fn reads(&self, program: &ScreenProgram, dest: Dest) -> Result<[Option<u32>; 4], ScreenRefused> {
        let mut out = [None; 4];
        for n in 0..4 {
            let ScreenStage::Engine(id) = program.stages[n] else { continue };
            if program.dims[n].is_none() {
                continue;
            }
            let t = self.target(id)?;
            if t == 1 && matches!(dest, Dest::Target1 { .. }) {
                return Err(ScreenRefused::Target1Feedback);
            }
            if self.size(t, (1, 1)).is_none() {
                return Err(if t == 1 { ScreenRefused::Target1Unwritten } else { ScreenRefused::Unwritten(t) });
            }
            out[n] = Some(t);
        }
        Ok(out)
    }

    /// After a pass into `dest`.
    pub fn drew(&mut self, dest: Dest) -> Result<(), ScreenRefused> {
        if let Dest::Target1 { guest } = dest {
            if guest.0 == 0 || guest.1 == 0 {
                return Err(ScreenRefused::Target1Size);
            }
            self.target1 = Some(guest);
        }
        Ok(())
    }

    /// After quad `q` of a convolution into stand-ins `size`: its source
    /// drawn, its destination a stand-in other than it.
    pub fn convolved(&mut self, q: &ConvolutionDraw, size: (u32, u32)) -> Result<(), ScreenRefused> {
        let Some(i) = stand_in(q.dest).filter(|_| q.dest != q.source) else {
            return Err(ScreenRefused::ConvolutionTarget(q.dest));
        };
        if self.size(q.source, (1, 1)).is_none() {
            return Err(if q.source == 1 { ScreenRefused::Target1Unwritten } else { ScreenRefused::Unwritten(q.source) });
        }
        if size.0 == 0 || size.1 == 0 {
            return Err(ScreenRefused::ConvolutionSize);
        }
        self.stand_ins[i] = Some(size);
        Ok(())
    }

    /// After convolution `n` of a pass, its quads drawn.
    pub fn result(&mut self, n: usize, c: &ScreenConvolution) {
        if let Some(r) = self.results.get_mut(n) {
            *r = Some(c.result);
        }
    }

    /// After a pass into `dest` and its `convolutions`.
    pub fn pass(&mut self, program: &ScreenProgram, dest: Dest, convolutions: &[ScreenConvolution]) -> Result<(), ScreenRefused> {
        self.reads(program, dest)?;
        self.drew(dest)?;
        for (n, c) in convolutions.iter().enumerate() {
            for q in &c.draws {
                self.convolved(q, c.size)?;
            }
            self.result(n, c);
        }
        Ok(())
    }
}

/// Why a screen pass is not drawn.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ScreenRefused {
    /// A stage binds an engine texture the pass does not supply yet.
    EngineTexture(u8),
    /// A stage is sampled with nothing bound.
    Unbound(usize),
    /// A stage's bitmap did not decode.
    Bitmap(usize),
    /// The view's colour is sampled but cannot be copied.
    ColourNotCopyable,
    Attachment(u32),
    Source(u32),
    /// The resolved clip is outside the destination.
    Clip,
    /// No program for this pass (the load refused it).
    NoProgram,
    FragmentRaster(String),
    /// The strip has fewer than three vertices or the call no strips.
    NoStrips,
    /// Target 1 sampled before a pass of the call drew it.
    Target1Unwritten,
    /// A pass into target 1 that samples it.
    Target1Feedback,
    /// A pass into target 1 at an empty size.
    Target1Size,
    /// Engine texture 24 or 25 sampled before a convolution of the call
    /// set it (`-1` on entry).
    ResultUnset(u8),
    /// A stand-in read before the call drew it.
    Unwritten(u32),
    /// A convolution drawing into something but a stand-in, or into its
    /// own source.
    ConvolutionTarget(u32),
    /// A convolution at an empty size.
    ConvolutionSize,
    /// Convolutions without the window, signs or combiner read.
    NoFrame,
    /// The blur's combiner does not compile.
    Convolution(String),
    /// The submission's arena could not hold a pass's uniforms.
    Uniforms,
}

/// Checks the sampled stages and the optional framebuffer's copy usage.
/// The draw-order check resolves which surface a convolution result names.
fn check(program: &ScreenProgram, reads: [Option<u32>; 4], color_usage: Option<wgpu::TextureUsages>) -> Result<(), ScreenRefused> {
    for (n, d) in program.dims.iter().enumerate() {
        if d.is_none() {
            continue;
        }
        match program.stages[n] {
            ScreenStage::Empty => return Err(ScreenRefused::Unbound(n)),
            ScreenStage::Bitmap { .. } => {}
            ScreenStage::Engine(id) => match bound(id) {
                None => return Err(ScreenRefused::EngineTexture(id)),
                Some(_) if reads[n] == Some(0) => {
                    let usage = color_usage.ok_or(ScreenRefused::Attachment(0))?;
                    if !usage.contains(wgpu::TextureUsages::COPY_SRC) {
                        return Err(ScreenRefused::ColourNotCopyable);
                    }
                }
                Some(_) => {}
            },
        }
    }
    Ok(())
}

/// Whether passes drawing `programs` into `dests` in order, each followed
/// by its convolutions, read only what the call drew before
/// ([`Drawn::pass`]): target 1 and the stand-ins after a pass or a
/// convolution drew them, never while drawing them, the results after a
/// convolution set them.
pub fn check_order<'a>(
    passes: impl IntoIterator<Item = (&'a ScreenProgram, Dest, &'a [ScreenConvolution])>,
) -> Result<(), ScreenRefused> {
    let mut drawn = Drawn::default();
    let mut pins_view = false;
    let mut needs_view = false;
    for (program, dest, convolutions) in passes {
        pins_view |= dest == Dest::Bound;
        // Even an unsampled stage can supply a size to a shader constant.
        needs_view |= program.stages.iter().enumerate().any(|(n, stage)| {
            let reads_size = program.constants.vertex.iter().any(|(c, _)|
                c.reads_size() && usize::from(c.stage) == n + 1
            );
            (program.dims[n].is_some() || reads_size)
                && matches!(stage, ScreenStage::Engine(id) if drawn.target(*id) == Ok(0))
        }) || convolutions.iter().flat_map(|c| &c.draws).any(|q| q.source == 0);
        drawn.pass(program, dest, convolutions)?;
    }
    if needs_view && !pins_view {
        return Err(ScreenRefused::Attachment(0));
    }
    Ok(())
}

fn convolution_states<'a>(
    frame: Option<&QuadFrame>,
    passes: impl IntoIterator<Item = &'a [ScreenConvolution]>,
) -> Result<(), ScreenRefused> {
    for convolutions in passes {
        let quads = convolutions.iter().flat_map(|c| &c.draws);
        if quads.clone().next().is_none() {
            continue;
        }
        frame.ok_or(ScreenRefused::NoFrame)?;
        quads.filter_map(|q| q.grid.as_ref()).try_for_each(ConvolutionGrid::check)?;
    }
    Ok(())
}

/// The vertex uniforms for a destination `guest_size` guest pixels
/// (`size` texels) with depth scale `z_scale`: the positions are in its
/// pixels, no W-buffer (the screen passes turn depth off).
pub fn screen_vertex(base: &VertexUniforms, guest_size: (u32, u32), size: (u32, u32), z_scale: f32) -> VertexUniforms {
    let mut u = base.clone();
    u.surface = [guest_size.0 as f32, guest_size.1 as f32];
    u.z_max = z_scale;
    u.w_clip = None;
    u.w_clamp = false;
    u.shift = crate::program::target_shift(guest_size, size);
    u
}

/// Each stage's engine texture's guest size (0 by 0 for the rest), as
/// `drawn` leaves the targets, `view` the bound colour's. What a size
/// mode of the stage's engine constants reads (`0x00019530` writes the
/// target's size into the stage when it binds it).
pub fn stage_sizes(program: &ScreenProgram, view: (u32, u32), drawn: &Drawn) -> [(u32, u32); 4] {
    std::array::from_fn(|n| match program.stages[n] {
        ScreenStage::Engine(id) => drawn.target(id).ok().and_then(|t| drawn.size(t, view)).unwrap_or((0, 0)),
        _ => (0, 0),
    })
}

/// The pixel uniforms: each engine stage (a render target, linear) fetched
/// in its texels ([`stage_sizes`]); no fog.
pub fn screen_pixel(program: &ScreenProgram, view: (u32, u32), drawn: &Drawn) -> PixelUniforms {
    let mut p = PixelUniforms { fog: psh::fog::Fog::OFF.uniform(), ..program.pixel };
    let texels = |s: (u32, u32)| [1.0 / s.0.max(1) as f32, 1.0 / s.1.max(1) as f32, 1.0, 1.0];
    let sizes = stage_sizes(program, view, drawn);
    for n in 0..4 {
        if program.dims[n].is_some() && matches!(program.stages[n], ScreenStage::Engine(_)) {
            p.tex_scale[n] = texels(sizes[n]);
        }
    }
    p
}

/// A pass's uniforms for a call with `values` drawing into a destination
/// `dest_guest` guest pixels (`dest_size` texels): [`screen_vertex`] and
/// [`screen_pixel`] with what the call puts in
/// (`ScreenConstants::apply`, the stages' sizes [`stage_sizes`]).
pub fn screen_uniforms(
    program: &ScreenProgram,
    values: [u32; 4],
    (view, drawn): ((u32, u32), &Drawn),
    (dest_guest, dest_size): ((u32, u32), (u32, u32)),
    z_scale: f32,
) -> (VertexUniforms, PixelUniforms) {
    let mut vertex = screen_vertex(&program.vertex, dest_guest, dest_size, z_scale);
    let mut pixel = screen_pixel(program, view, drawn);
    program.constants.apply(&mut vertex, &mut pixel, values, stage_sizes(program, view, drawn));
    (vertex, pixel)
}

/// What a screen pass draws into, and the submission's arena its
/// uniforms and strips go in.
pub struct ScreenTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub uploads: &'a gfx::ring::SubmissionUploads,
    pub color: Option<glow::PostSurface<'a>>,
    pub target1: Option<glow::PostSurface<'a>>,
    pub convolutions: [Option<glow::PostSurface<'a>>; 3],
    /// The device's depth scale (`+0x948`).
    pub z_scale: f32,
}

/// One compiled screen pass on the GPU.
pub struct ScreenGpu {
    pub program: ScreenProgram,
    module: wgpu::ShaderModule,
    variants: HashMap<FragmentRaster, wgpu::ShaderModule>,
    layout: wgpu::PipelineLayout,
    uniforms_layout: wgpu::BindGroupLayout,
    textures_layout: wgpu::BindGroupLayout,
    samplers: [wgpu::Sampler; 4],
    stage_views: [Option<wgpu::TextureView>; 4],
    pipelines: HashMap<RasterPipeline, wgpu::RenderPipeline>,
}

impl ScreenGpu {
    /// Uploads `program` and its bitmaps (`stage_image`); refused when a
    /// sampled stage has nothing the pass can bind.
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        program: ScreenProgram,
        stage_image: &mut dyn FnMut(ScreenStage) -> Option<Image>,
    ) -> Result<Self, ScreenRefused> {
        let mut stage_views: [Option<wgpu::TextureView>; 4] = Default::default();
        for (n, d) in program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            match program.stages[n] {
                ScreenStage::Empty => return Err(ScreenRefused::Unbound(n)),
                ScreenStage::Engine(id) if !supplied(id) => return Err(ScreenRefused::EngineTexture(id)),
                ScreenStage::Engine(_) => {}
                s @ ScreenStage::Bitmap { .. } => {
                    let mut image = stage_image(s).ok_or(ScreenRefused::Bitmap(n))?;
                    bias_signed(&mut image, program.signed[n]);
                    stage_views[n] = Some(texture(device, queue, &image, "native screen stage"));
                }
            }
        }
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("native screen pass"),
            source: wgpu::ShaderSource::Wgsl(program.wgsl.clone().into()),
        });
        // At dynamic offsets into the submission's arena.
        let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: true,
                min_binding_size: wgpu::BufferSize::new(size as u64),
            },
            count: None,
        };
        let uniforms_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native screen uniforms"),
            entries: &[uniform(0, VertexUniforms::SIZE), uniform(1, PixelUniforms::SIZE)],
        });
        let mut entries = Vec::new();
        for (n, d) in program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            });
            entries.push(wgpu::BindGroupLayoutEntry {
                binding: 2 * n as u32 + 1,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                count: None,
            });
        }
        let textures_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("native screen textures"),
            entries: &entries,
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("native screen pass"),
            bind_group_layouts: &[Some(&uniforms_layout), Some(&textures_layout)],
            immediate_size: 0,
        });
        let samplers = std::array::from_fn(|n| device.create_sampler(&sampler_descriptor(program.samplers[n].as_ref())));
        Ok(Self {
            program,
            module,
            variants: HashMap::new(),
            layout,
            uniforms_layout,
            textures_layout,
            samplers,
            stage_views,
            pipelines: HashMap::new(),
        })
    }

    /// The pipeline for colour format `color`: strips, the pass's blend
    /// and colour writes, no depth.
    fn pipeline(&mut self, device: &wgpu::Device, key: RasterPipeline) -> Result<&wgpu::RenderPipeline, ScreenRefused> {
        const SCREEN_PIPELINES: usize = 128;
        if self.pipelines.len() == SCREEN_PIPELINES && !self.pipelines.contains_key(&key) {
            self.pipelines.clear();
        }
        let default = FragmentRaster {
            alpha: self.program.state.alpha_test.map(|(func, _)| crate::program::alpha_func(func)),
            stipple: false,
        };
        let module = if key.fragment == default {
            &self.module
        } else {
            match self.variants.entry(key.fragment) {
                std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
                std::collections::hash_map::Entry::Vacant(entry) => {
                    let source = self.program.raster_shader.as_ref().ok_or(ScreenRefused::NoProgram)?
                        .translate(key.fragment.alpha, key.fragment.stipple)
                        .map_err(ScreenRefused::FragmentRaster)?;
                    entry.insert(device.create_shader_module(wgpu::ShaderModuleDescriptor {
                        label: Some("native screen raster variant"),
                        source: wgpu::ShaderSource::Wgsl(source.into()),
                    }))
                }
            }
        };
        let layout = &self.layout;
        Ok(self.pipelines.entry(key).or_insert_with(|| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native screen pass"),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<StripVertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &STRIP_ATTRIBUTES,
                    })],
                },
                primitive: key.primitive(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: key.format,
                        blend: key.blend,
                        write_mask: key.writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        }))
    }

    /// The texture group: `engine[n]` what engine stage `n` reads (the
    /// copy of the bound colour, target 1, a convolution's).
    fn textures(&self, device: &wgpu::Device, engine: &[Option<wgpu::TextureView>; 4]) -> Result<wgpu::BindGroup, ScreenRefused> {
        let mut views = Vec::new();
        for (n, d) in self.program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            let v = match self.program.stages[n] {
                ScreenStage::Empty => return Err(ScreenRefused::Unbound(n)),
                ScreenStage::Bitmap { .. } => self.stage_views[n].clone().ok_or(ScreenRefused::Bitmap(n))?,
                ScreenStage::Engine(id) => engine[n].clone().ok_or(ScreenRefused::EngineTexture(id))?,
            };
            views.push((n, v));
        }
        let mut entries = Vec::new();
        for (n, v) in &views {
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32,
                resource: wgpu::BindingResource::TextureView(v),
            });
            entries.push(wgpu::BindGroupEntry {
                binding: 2 * *n as u32 + 1,
                resource: wgpu::BindingResource::Sampler(&self.samplers[*n]),
            });
        }
        Ok(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("native screen textures"),
            layout: &self.textures_layout,
            entries: &entries,
        }))
    }
}

/// The key a [`ScreenSet`] keeps a pass by: the egor's datum, the layer
/// pass and the implementation.
pub type ScreenKey = (u32, usize, usize);

/// Raster state resolved at a native post draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PostRaster {
    pub blend: Option<wgpu::BlendState>,
    pub blend_constant: wgpu::Color,
    pub alpha_test: Option<(psh::AlphaFunc, u8)>,
    pub stipple: Option<[u32; 32]>,
    pub writes: wgpu::ColorWrites,
    pub front_face: wgpu::FrontFace,
    pub cull: Option<wgpu::Face>,
    pub depth_clamp: bool,
    pub fill: gfx::state::FillMode,
    /// Window depth conversion and optional W clip range of the live strip.
    pub z_scale: Option<f32>,
    pub w_clip: Option<[f32; 2]>,
    /// X8 aliases read alpha as one, independently of stored alpha.
    pub opaque_view: [bool; 4],
    /// The effective scissor in guest target pixels.
    pub clip: Option<[u32; 4]>,
    /// The fog register the live draw's combiners read: the fog unit's
    /// uniform and colour (`d3d::nv2a::draw_fog`). `None` leaves the
    /// program's (fog off).
    pub fog: Option<([f32; 4], [f32; 4])>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct RasterPipeline {
    pub(crate) format: wgpu::TextureFormat,
    pub(crate) blend: Option<wgpu::BlendState>,
    pub(crate) writes: wgpu::ColorWrites,
    front_face: wgpu::FrontFace,
    cull: Option<wgpu::Face>,
    depth_clamp: bool,
    polygon_mode: wgpu::PolygonMode,
    fragment: FragmentRaster,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub(crate) struct FragmentRaster {
    pub(crate) alpha: Option<psh::AlphaFunc>,
    pub(crate) stipple: bool,
}

impl PostRaster {
    pub(crate) fn pipeline(self, format: wgpu::TextureFormat, features: wgpu::Features) -> RasterPipeline {
        RasterPipeline {
            format,
            blend: self.blend,
            writes: self.writes,
            front_face: self.front_face,
            cull: self.cull,
            depth_clamp: self.depth_clamp && features.contains(wgpu::Features::DEPTH_CLIP_CONTROL),
            polygon_mode: gfx::translate::primitive_state(
                &gfx::RenderState { fill_mode: self.fill, ..Default::default() },
                wgpu::PrimitiveTopology::TriangleStrip,
                gfx::translate::Capabilities::from_features(features),
            ).polygon_mode,
            fragment: self.fragment(),
        }
    }

    pub(crate) fn fragment(self) -> FragmentRaster {
        FragmentRaster {
            alpha: self.alpha_test.map(|(func, _)| func),
            stipple: self.stipple.is_some(),
        }
    }

    pub(crate) fn pixel_uniforms(self, pixel: &mut PixelUniforms, guest: (u32, u32), size: (u32, u32)) {
        if let Some((_, reference)) = self.alpha_test {
            pixel.alpha_ref = [f32::from(reference), 0.0, 0.0, 0.0];
        }
        if let Some(pattern) = self.stipple {
            pixel.stipple = pattern;
            pixel.stipple_scale = PixelUniforms::stipple_grid(guest, size);
        }
        if let Some((fog, colour)) = self.fog {
            pixel.fog = fog;
            pixel.fog_color = colour;
        }
    }

    pub(crate) fn scissor(self, guest: (u32, u32), size: (u32, u32)) -> Result<[u32; 4], ScreenRefused> {
        let [x, y, w, h] = self.clip.unwrap_or([0, 0, guest.0, guest.1]);
        let scale = d3d::scaled::RenderScale::between(guest, size).ok_or(ScreenRefused::Clip)?;
        if w == 0 || h == 0
            || x > guest.0 || w > guest.0 - x || y > guest.1 || h > guest.1 - y
        {
            return Err(ScreenRefused::Clip);
        }
        Ok(scale.rect(x, y, w, h))
    }
}

impl RasterPipeline {
    pub(crate) fn primitive(self) -> wgpu::PrimitiveState {
        wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::TriangleStrip,
            front_face: self.front_face,
            cull_mode: self.cull,
            unclipped_depth: self.depth_clamp,
            polygon_mode: self.polygon_mode,
            ..Default::default()
        }
    }
}

impl Default for PostRaster {
    fn default() -> Self {
        Self {
            blend: None,
            blend_constant: wgpu::Color::TRANSPARENT,
            alpha_test: None,
            stipple: None,
            writes: wgpu::ColorWrites::ALL,
            front_face: wgpu::FrontFace::Ccw,
            cull: None,
            depth_clamp: false,
            fill: gfx::state::FillMode::Solid,
            z_scale: None,
            w_clip: None,
            opaque_view: [false; 4],
            clip: None,
            fog: None,
        }
    }
}

impl From<crate::pass_state::PassState> for PostRaster {
    fn from(s: crate::pass_state::PassState) -> Self {
        Self {
            blend: s.blend,
            blend_constant: s.blend_constant,
            alpha_test: s.alpha_test.map(|(func, reference)| (crate::program::alpha_func(func), reference)),
            writes: s.color_writes,
            front_face: s.primitive.front_face,
            cull: s.primitive.cull_mode,
            depth_clamp: s.primitive.unclipped_depth,
            ..Self::default()
        }
    }
}

/// One pass of a call: its program, strips, destination and the
/// convolutions after it, in draw order.
pub struct ScreenDraw<'a> {
    pub key: ScreenKey,
    pub program: &'a ScreenProgram,
    pub strips: &'a [Vec<StripVertex>],
    pub raster: PostRaster,
    pub dest: Dest,
    /// The call's four values (f32 bits).
    pub values: [u32; 4],
    pub convolutions: &'a [ScreenConvolution],
    /// One raster per convolution quad or grid, in draw order.
    pub convolution_rasters: &'a [PostRaster],
}

/// A texture the set keeps: its size and format, and a view of it.
type Kept = (wgpu::Extent3d, wgpu::TextureFormat, wgpu::Texture, wgpu::TextureView);

/// A texture of `extent` and `format` with `usage`, kept in `slot` while
/// they stay the same.
fn keep<'s>(
    slot: &'s mut Option<Kept>,
    device: &wgpu::Device,
    extent: wgpu::Extent3d,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
    label: &str,
) -> &'s Kept {
    if slot.as_ref().is_none_or(|(e, f, _, _)| *e != extent || *f != format) {
        let t = device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        });
        let v = t.create_view(&Default::default());
        *slot = Some((extent, format, t, v));
    }
    slot.as_ref().expect("just made")
}

/// How a convolution's draw goes down: a quad's uniforms, or a grid's.
enum Shape {
    Quad(glow::Quad),
    Grid(glow::GridFrame),
}

/// The same alpha-only clear the D3D framebuffer snapshot applies to X8.
#[derive(Default)]
struct AlphaClear {
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
}

impl AlphaClear {
    /// The clear's pipeline for `format`.
    fn pipeline(&mut self, device: &wgpu::Device, format: wgpu::TextureFormat) -> wgpu::RenderPipeline {
        self.pipelines.entry(format).or_insert_with(|| {
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("native screen X8 alpha"),
                source: wgpu::ShaderSource::Wgsl(r#"
                    @vertex fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4f {
                        let x = f32((i << 1u) & 2u);
                        let y = f32(i & 2u);
                        return vec4f(2.0 * x - 1.0, 2.0 * y - 1.0, 0.0, 1.0);
                    }
                    @fragment fn fs_main() -> @location(0) vec4f {
                        return vec4f(0.0, 0.0, 0.0, 1.0);
                    }
                "#.into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("native screen X8 alpha"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &module, entry_point: Some("vs_main"), buffers: &[], compilation_options: Default::default(),
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALPHA })],
                }),
                multiview_mask: None,
                cache: None,
            })
        }).clone()
    }
}

/// Which framebuffer snapshots the sampled stages need, in RGBA and X8 order.
fn view_snapshots(reads: [Option<u32>; 4], opaque: [bool; 4]) -> [bool; 2] {
    let mut needed = [false; 2];
    for (source, opaque) in reads.into_iter().zip(opaque) {
        if source == Some(0) {
            needed[usize::from(opaque)] = true;
        }
    }
    needed
}

/// Compiled screen passes and the per-pass feedback copy.
#[derive(Default)]
pub struct ScreenSet {
    passes: HashMap<ScreenKey, Result<ScreenGpu, ScreenRefused>>,
    copies: [Option<Kept>; 2],
    quads: glow::QuadDrawer,
    opaque: AlphaClear,
}

impl ScreenSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets every pass (a new map's effects).
    pub fn clear(&mut self) {
        self.passes.clear();
        self.copies = [None, None];
        self.quads = glow::QuadDrawer::new();
    }

    /// The call against its live destination and source views, prepared
    /// ([`ScreenCommands::record`] records it): every pipeline and group
    /// made and every uniform and strip staged in `t.uploads`. Refused
    /// before anything is recorded.
    pub fn prepare(
        &mut self,
        t: ScreenTarget<'_>,
        draws: &[ScreenDraw<'_>],
        frame: Option<&QuadFrame>,
        stage_image: &mut dyn FnMut(ScreenStage) -> Option<Image>,
    ) -> Result<ScreenCommands, ScreenRefused> {
        let ScreenTarget { device, queue, uploads, color, target1, convolutions, z_scale } = t;
        let guest_size = color.map_or((0, 0), |c| c.guest_size);
        check_order(draws.iter().map(|d| (d.program, d.dest, d.convolutions)))?;
        convolution_states(frame, draws.iter().map(|d| d.convolutions))?;
        let slot = |index| match index {
            0 => color,
            1 => target1,
            _ => stand_in(index).and_then(|i| convolutions[i]),
        };
        let view_of = |index| slot(index).map(|slot| slot.view);
        let attachment = |index, guest| {
            let slot = slot(index).ok_or(ScreenRefused::Attachment(index))?;
            if slot.guest_size != guest || !glow::live_colour(slot.view) {
                return Err(ScreenRefused::Attachment(index));
            }
            Ok(slot.view)
        };
        let sample = |index| {
            let view = view_of(index).ok_or(ScreenRefused::Source(index))?;
            if !glow::sampleable(device, view) {
                return Err(ScreenRefused::Source(index));
            }
            Ok(view)
        };
        let dest_of = |dest| match dest {
            Dest::Bound => (0, guest_size),
            Dest::Target1 { guest } => (1, guest),
        };
        let mut drawn = Drawn::default();
        for d in draws {
            if !d.convolution_rasters.is_empty()
                && d.convolution_rasters.len() != d.convolutions.iter().map(|c| c.draws.len()).sum::<usize>()
            {
                return Err(ScreenRefused::Convolution("raster count differs from convolution count".into()));
            }
            if d.strips.is_empty() || d.strips.iter().any(|s| s.len() < 3) {
                return Err(ScreenRefused::NoStrips);
            }
            let (index, guest) = dest_of(d.dest);
            let dest = attachment(index, guest)?;
            let reads = drawn.reads(d.program, d.dest)?;
            check(d.program, reads, color.map(|c| c.view.texture().usage()))?;
            for source in reads.into_iter().flatten() {
                let source_view = sample(source)?;
                if source != 0 && dest.texture() == source_view.texture() {
                    return Err(ScreenRefused::Source(source));
                }
            }
            let stale = matches!(self.passes.get(&d.key), Some(Ok(g)) if g.program.wgsl != d.program.wgsl);
            if stale || !self.passes.contains_key(&d.key) {
                let g = ScreenGpu::new(device, queue, d.program.clone(), &mut *stage_image);
                self.passes.insert(d.key, g);
            }
            match self.passes.get_mut(&d.key) {
                Some(Ok(g)) => {
                    g.pipeline(device, d.raster.pipeline(dest.texture().format(), device.features()))?;
                }
                Some(Err(r)) => return Err(r.clone()),
                None => return Err(ScreenRefused::NoProgram),
            }
            drawn.pass(d.program, d.dest, d.convolutions)?;
        }
        // Each draw's program and vertex uniform: a quad's stages its
        // source's size as the call leaves the targets, a grid's the size
        // its taps were made for.
        let mut quads = Vec::new();
        let mut drawn = Drawn::default();
        for d in draws {
            drawn.reads(d.program, d.dest)?;
            drawn.drew(d.dest)?;
            let mut rasters = d.convolution_rasters.iter().copied();
            for (n, c) in d.convolutions.iter().enumerate() {
                for q in &c.draws {
                    let frame = frame.ok_or(ScreenRefused::NoFrame)?;
                    let dest = attachment(q.dest, c.size)?;
                    if sample(q.source)?.texture() == dest.texture() {
                        return Err(ScreenRefused::Source(q.source));
                    }
                    let source = drawn.size(q.source, guest_size).ok_or(ScreenRefused::Unwritten(q.source))?;
                    let raster = rasters.next().unwrap_or_default();
                    let (program, shape) = match &q.grid {
                        None => {
                            let stages = [Some(source); 4];
                            let program = self.quads.cached(&frame.combiner, stages, false, raster.fragment()).map_err(ScreenRefused::Convolution)?;
                            (program, Shape::Quad(glow::quad(frame.window, frame.signs, c.size, stages, 4, q.spread)))
                        }
                        Some(g) => {
                            // Checked by `convolution_states`.
                            let stages = [Some(g.bound); 4];
                            let program = self.quads.cached(&frame.combiner, stages, true, raster.fragment()).map_err(ScreenRefused::Convolution)?;
                            (program, Shape::Grid(glow::GridFrame::of(c.size)))
                        }
                    };
                    quads.push((program, shape, raster));
                    drawn.convolved(q, c.size)?;
                }
                drawn.result(n, c);
            }
        }
        let mut commands = ScreenCommands { steps: Vec::new(), ranges: 0 };
        let mut quads = quads.into_iter();
        let mut drawn = Drawn::default();
        for d in draws {
            let Some(Ok(g)) = self.passes.get(&d.key) else { return Err(ScreenRefused::NoProgram) };
            let reads = drawn.reads(d.program, d.dest)?;
            let mut snapshots = [None, None];
            for (i, needed) in view_snapshots(reads, d.raster.opaque_view).into_iter().enumerate() {
                if !needed {
                    continue;
                }
                let texture = color.ok_or(ScreenRefused::Attachment(0))?.view.texture();
                let full = wgpu::Extent3d { width: texture.width(), height: texture.height(), depth_or_array_layers: 1 };
                let usage = wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::COPY_DST
                    | wgpu::TextureUsages::RENDER_ATTACHMENT;
                let (extent, _, copy, view) = keep(&mut self.copies[i], device, full, texture.format(), usage, "native screen view copy");
                commands.steps.push(Step::Copy { source: texture.clone(), dest: copy.clone(), extent: *extent });
                if i == 1 {
                    let pipeline = self.opaque.pipeline(device, copy.format());
                    commands.steps.push(Step::Opaque { pipeline, view: view.clone() });
                }
                snapshots[i] = Some(view.clone());
            }
            let (index, dest_guest) = dest_of(d.dest);
            let dest_view = attachment(index, dest_guest)?;
            let dest_texture = dest_view.texture();
            let dest_size = (dest_texture.width(), dest_texture.height());
            let color_format = dest_texture.format();
            drawn.drew(d.dest)?;
            let engine = std::array::from_fn(|n| match reads[n] {
                Some(0) => snapshots[usize::from(d.raster.opaque_view[n])].clone(),
                Some(t) => view_of(t).cloned(),
                None => None,
            });
            let textures = g.textures(device, &engine)?;
            let (mut vertex, mut pixel) = screen_uniforms(
                d.program, d.values, (guest_size, &drawn), (dest_guest, dest_size), d.raster.z_scale.unwrap_or(z_scale),
            );
            vertex.w_clip = d.raster.w_clip;
            vertex.w_clamp = d.raster.depth_clamp;
            d.raster.pixel_uniforms(&mut pixel, dest_guest, dest_size);
            let uniforms = uploads
                .uniform_run(device, queue, &g.uniforms_layout, [VertexUniforms::SIZE, PixelUniforms::SIZE], 1, |_, v, p| {
                    vertex.write_bytes(v);
                    pixel.write_bytes(p);
                })
                .ok_or(ScreenRefused::Uniforms)?;
            let all: Vec<StripVertex> = d.strips.iter().flatten().copied().collect();
            let bytes: &[u8] = bytemuck::cast_slice(&all);
            let (buffer, offset) = uploads.stage(device, queue, bytes);
            let Some(pipeline) = g.pipelines.get(&d.raster.pipeline(color_format, device.features())) else { return Err(ScreenRefused::NoProgram) };
            let scissor = d.raster.scissor(dest_guest, dest_size)?;
            let ranges = strip_ranges(d.strips);
            commands.ranges += ranges.len();
            commands.steps.push(Step::Strips(StripPass {
                dest: dest_view.clone(),
                size: dest_size,
                pipeline: pipeline.clone(),
                uniforms: uniforms.group.clone(),
                offsets: uniforms.offsets(0),
                textures,
                blend_constant: d.raster.blend_constant,
                scissor,
                vertices: (buffer, offset, bytes.len() as u64),
                ranges,
            }));
            // A convolution samples the preceding pass's real pixels.
            for (n, c) in d.convolutions.iter().enumerate() {
                for q in &c.draws {
                    let Some((program, shape, raster)) = quads.next() else { return Err(ScreenRefused::NoFrame) };
                    let source = sample(q.source)?;
                    let dest = glow::PostSurface { view: attachment(q.dest, c.size)?, guest_size: c.size };
                    let format = dest.view.texture().format();
                    let (vertex, strips) = match (&shape, &q.grid) {
                        (Shape::Grid(f), Some(g)) => (bytemuck::bytes_of(f), Some(g.strips.as_slice())),
                        (Shape::Quad(quad), _) => (bytemuck::bytes_of(quad), None),
                        (Shape::Grid(_), None) => return Err(ScreenRefused::NoStrips),
                    };
                    let quad = self.quads.prepare(
                        (device, queue, uploads), &program, vertex, strips, (dest, format, raster), [Some(source); 4],
                    );
                    commands.steps.extend(quad.map_err(|why| ScreenRefused::Convolution(why.into()))?.map(Step::Quad));
                    drawn.convolved(q, c.size)?;
                    commands.ranges += q.grid.as_ref().map_or(1, |grid| grid.strips.len());
                }
                drawn.result(n, c);
            }
        }
        Ok(commands)
    }

    /// [`Self::prepare`], recorded into `encoder`: the ranges drawn.
    pub fn encode(
        &mut self,
        t: ScreenTarget<'_>,
        encoder: &mut wgpu::CommandEncoder,
        draws: &[ScreenDraw<'_>],
        frame: Option<&QuadFrame>,
        stage_image: &mut dyn FnMut(ScreenStage) -> Option<Image>,
    ) -> Result<usize, ScreenRefused> {
        let commands = self.prepare(t, draws, frame, stage_image)?;
        commands.record(encoder);
        Ok(commands.ranges())
    }
}

/// A pass's strips into its destination ([`ScreenSet::prepare`]).
#[derive(Clone)]
struct StripPass {
    dest: wgpu::TextureView,
    /// The destination's texels.
    size: (u32, u32),
    pipeline: wgpu::RenderPipeline,
    uniforms: wgpu::BindGroup,
    offsets: [u32; 2],
    textures: wgpu::BindGroup,
    blend_constant: wgpu::Color,
    scissor: [u32; 4],
    /// The strips in the submission's arena: buffer, offset, bytes.
    vertices: (wgpu::Buffer, u64, u64),
    ranges: Vec<Range<u32>>,
}

/// One command of a prepared call, in order.
#[derive(Clone)]
enum Step {
    /// The bound colour into the feedback copy a pass samples.
    Copy { source: wgpu::Texture, dest: wgpu::Texture, extent: wgpu::Extent3d },
    /// The copy's alpha set to one (an X8 view, [`AlphaClear`]).
    Opaque { pipeline: wgpu::RenderPipeline, view: wgpu::TextureView },
    Strips(StripPass),
    /// A convolution's quad or grid.
    Quad(glow::QuadCommand),
}

/// A call's commands ([`ScreenSet::prepare`]): everything they use made
/// and written, so recording them makes and writes nothing and records
/// the same commands each time.
#[derive(Clone)]
pub struct ScreenCommands {
    steps: Vec<Step>,
    ranges: usize,
}

impl ScreenCommands {
    /// The guest ranges drawn: each pass's strips and each convolution's
    /// quad or grid strips.
    pub fn ranges(&self) -> usize {
        self.ranges
    }

    /// GPU draws in the call, including alpha fills of feedback copies.
    pub fn draws(&self) -> usize {
        self.steps.iter().map(|step| match step {
            Step::Copy { .. } => 0,
            Step::Strips(strips) => strips.ranges.len(),
            Step::Opaque { .. } | Step::Quad(_) => 1,
        }).sum()
    }

    /// The render passes [`Self::record`] begins.
    pub fn passes(&self) -> usize {
        self.steps.iter().filter(|s| !matches!(s, Step::Copy { .. })).count()
    }

    /// Records the call in order.
    pub fn record(&self, encoder: &mut wgpu::CommandEncoder) {
        let load = wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store };
        for step in &self.steps {
            match step {
                Step::Copy { source, dest, extent } => {
                    encoder.copy_texture_to_texture(source.as_image_copy(), dest.as_image_copy(), *extent);
                }
                Step::Opaque { pipeline, view } => {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("native screen X8 alpha"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view, resolve_target: None, depth_slice: None, ops: load,
                        })],
                        ..Default::default()
                    });
                    pass.set_pipeline(pipeline);
                    pass.draw(0..3, 0..1);
                }
                Step::Strips(s) => {
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: Some("native screen effect"),
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: &s.dest,
                            depth_slice: None,
                            resolve_target: None,
                            ops: load,
                        })],
                        depth_stencil_attachment: None,
                        timestamp_writes: None,
                        occlusion_query_set: None,
                        multiview_mask: None,
                    });
                    pass.set_viewport(0.0, 0.0, s.size.0 as f32, s.size.1 as f32, 0.0, 1.0);
                    pass.set_pipeline(&s.pipeline);
                    pass.set_bind_group(0, &s.uniforms, &s.offsets);
                    pass.set_bind_group(1, &s.textures, &[]);
                    pass.set_blend_constant(s.blend_constant);
                    let [x, y, w, h] = s.scissor;
                    pass.set_scissor_rect(x, y, w, h);
                    let (buffer, offset, len) = &s.vertices;
                    pass.set_vertex_buffer(0, buffer.slice(*offset..*offset + *len));
                    for r in &s.ranges {
                        pass.draw(r.clone(), 0..1);
                    }
                }
                Step::Quad(quad) => quad.record(encoder),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pass_state::PassState;
    use shaders::psh::TextureDim;

    fn program(stages: [ScreenStage; 4]) -> ScreenProgram {
        ScreenProgram {
            wgsl: String::new(),
            stages,
            samplers: [None; 4],
            dims: stages.map(crate::screen_look::stage_dim),
            signed: [[false; 4]; 4],
            vertex: VertexUniforms::default(),
            pixel: PixelUniforms::default(),
            combiner_inputs: Default::default(),
            raster_shader: None,
            inputs: 0x1f,
            constants: Default::default(),
            state: PassState {
                blend: None,
                blend_constant: wgpu::Color::TRANSPARENT,
                color_writes: wgpu::ColorWrites::ALL,
                depth_write: false,
                depth_compare: wgpu::CompareFunction::Always,
                primitive: wgpu::PrimitiveState::default(),
                alpha_test: None,
                stencil: None,
                depth_offset: None,
            },
        }
    }

    const E: ScreenStage = ScreenStage::Empty;

    #[test]
    fn the_live_raster_keys_colour_masks_and_scales_the_scissor() {
        let mut raster: PostRaster = program([ScreenStage::Empty; 4]).state.into();
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let first = raster.pipeline(format, wgpu::Features::empty());
        raster.writes = wgpu::ColorWrites::RED | wgpu::ColorWrites::BLUE;
        raster.clip = Some([4, 6, 8, 10]);
        assert_ne!(raster.pipeline(format, wgpu::Features::empty()), first);
        raster.front_face = wgpu::FrontFace::Cw;
        raster.cull = Some(wgpu::Face::Front);
        raster.fill = gfx::state::FillMode::Wireframe;
        let primitive = raster.pipeline(format, wgpu::Features::POLYGON_MODE_LINE).primitive();
        assert_eq!(primitive.front_face, wgpu::FrontFace::Cw);
        assert_eq!(primitive.cull_mode, Some(wgpu::Face::Front));
        assert_eq!(primitive.polygon_mode, wgpu::PolygonMode::Line);
        assert_eq!(raster.pipeline(format, wgpu::Features::empty()).primitive().polygon_mode, wgpu::PolygonMode::Fill);
        assert_eq!(raster.scissor((32, 32), (64, 64)), Ok([8, 12, 16, 20]));
        assert_eq!(raster.scissor((32, 32), (80, 40)), Ok([10, 8, 20, 12]));
        assert_eq!(raster.scissor((8, 8), (16, 16)), Err(ScreenRefused::Clip));
    }

    #[test]
    fn alpha_reference_and_stipple_rows_are_per_draw_uniforms() {
        use shaders::psh::AlphaFunc;

        let raster = PostRaster {
            alpha_test: Some((AlphaFunc::Less, 37)),
            stipple: Some([0x42; 32]),
            ..Default::default()
        };
        let other = PostRaster {
            alpha_test: Some((AlphaFunc::Less, 83)),
            stipple: Some([0x77; 32]),
            ..raster
        };
        assert_eq!(raster.fragment(), other.fragment());
        let mut pixel = PixelUniforms::default();
        raster.pixel_uniforms(&mut pixel, (32, 19), (80, 31));
        assert_eq!(pixel.alpha_ref[0], 37.0);
        assert_eq!(pixel.stipple, [0x42; 32]);
        assert_eq!(pixel.stipple_scale, PixelUniforms::stipple_grid((32, 19), (80, 31)));
        assert_eq!(pixel.fog, PixelUniforms::default().fog, "no fog given: the program's");

        let fog = ([0.25, -0.5, 1.0, 0.0], [0.1, 0.2, 0.3, 1.0]);
        let mut pixel = PixelUniforms::default();
        PostRaster { fog: Some(fog), ..raster }.pixel_uniforms(&mut pixel, (32, 19), (80, 31));
        assert_eq!((pixel.fog, pixel.fog_color), fog, "the live draw's fog register");
    }

    #[test]
    fn a_strip_is_the_pushed_words_as_floats() {
        let one = 1.0f32.to_bits();
        let v = ([[one, 0, 0, one]; 4], [2.5f32.to_bits(), 3.0f32.to_bits()]);
        let s = strip(&[v, v], 0x4b7f_ffff);
        assert_eq!(s.len(), 2);
        assert_eq!(s[0].position[..2], [2.5, 3.0]);
        assert_eq!(s[0].position[2].to_bits(), 0x4b7f_ffff, "z and w the depth word");
        assert_eq!(s[0].position[3], s[0].position[2]);
        assert_eq!(s[1].texcoords[3], [1.0, 0.0, 0.0, 1.0]);
        assert_eq!(std::mem::size_of::<StripVertex>(), 80);
        assert_eq!(STRIP_ATTRIBUTES.map(|a| (a.shader_location, a.offset)), [(0, 0), (1, 16), (2, 32), (3, 48), (4, 64)]);
    }

    #[test]
    fn screen_stages_can_read_both_alpha_interpretations_of_the_view() {
        let reads = [Some(0), Some(1), None, Some(0)];
        assert_eq!(view_snapshots(reads, [false, true, true, true]), [true, true]);
        assert_eq!(view_snapshots(reads, [false, true, true, false]), [true, false]);
        assert_eq!(view_snapshots(reads, [true; 4]), [false, true]);
        assert_eq!(view_snapshots([Some(1); 4], [true; 4]), [false, false]);
    }

    #[test]
    fn convolution_result_only_requires_a_copyable_view_when_it_names_the_view() {
        let result = program([ScreenStage::Engine(24), E, E, E]);
        let usage = Some(wgpu::TextureUsages::RENDER_ATTACHMENT);
        assert_eq!(check(&result, [Some(0x19), None, None, None], usage), Ok(()));
        assert_eq!(check(&result, [Some(0), None, None, None], usage), Err(ScreenRefused::ColourNotCopyable));
        assert_eq!(check(&result, [Some(0), None, None, None], None), Err(ScreenRefused::Attachment(0)));
    }

    #[test]
    fn strips_draw_one_after_another() {
        let s = |n| vec![StripVertex::default(); n];
        assert_eq!(strip_ranges(&[s(6), s(6), s(4)]), vec![0..6, 6..12, 12..16]);
        assert!(strip_ranges(&[]).is_empty());
    }

    #[test]
    fn the_view_and_target_1_are_supplied() {
        assert_eq!(bound(5), Some(Bound::View));
        assert_eq!((bound(2), bound(3)), (Some(Bound::Target1), Some(Bound::Target1)));
        assert_eq!((bound(24), bound(25)), (Some(Bound::Result(0)), Some(Bound::Result(1))));
        for id in [1, 4, 6, 9, 13] {
            assert!(!supplied(id), "{id}");
        }
        let bitmap = ScreenStage::Bitmap { bitmap: map::TagId(0xe000_0001), image: 0 };
        let view = program([ScreenStage::Engine(5), bitmap, E, E]);
        assert!(samples(&view, Bound::View) && !samples(&view, Bound::Target1));
        assert!(!samples(&program([bitmap, E, E, E]), Bound::View));
    }

    /// `effects\screen_effects\blur`'s passes: the view copied into target
    /// 1, then target 1 (with a convolution's result) back over the view.
    #[test]
    fn target_1_is_read_only_after_a_pass_drew_it() {
        let copy = program([ScreenStage::Engine(5), E, E, E]);
        let back = program([ScreenStage::Engine(3), E, E, E]);
        let into = Dest::Target1 { guest: (640, 480) };
        let none: &[ScreenConvolution] = &[];
        assert_eq!(check_order([(&copy, into, none), (&back, Dest::Bound, none)]), Ok(()));
        assert_eq!(check_order([(&back, Dest::Bound, none)]), Err(ScreenRefused::Target1Unwritten), "the D3D path's contents");
        assert_eq!(check_order([(&copy, into, none), (&back, into, none)]), Err(ScreenRefused::Target1Feedback));
        let empty = Dest::Target1 { guest: (0, 240) };
        assert_eq!(check_order([(&copy, empty, none)]), Err(ScreenRefused::Target1Size));
        assert_eq!(check_order([(&copy, Dest::Bound, none)]), Ok(()), "desaturate's");
        assert_eq!(check_order([(&copy, into, none)]), Err(ScreenRefused::Attachment(0)));
        let mut unused = copy.clone();
        unused.dims[0] = None;
        assert_eq!(check_order([(&unused, into, none)]), Ok(()));
        unused.constants.vertex.push((tags::material::EngineVertexConstant {
            bank: 0, reg: 2, mode: 5, stage: 1, source: 0,
        }, 1));
        assert_eq!(check_order([(&unused, into, none)]), Err(ScreenRefused::Attachment(0)));
    }

    /// The blur's shape: its first pass copies the view into target 1 and
    /// convolves it into `0x19` and `0x1a`; its second reads the result
    /// (engine texture 24) and target 1.
    #[test]
    fn a_result_is_read_after_its_convolution_set_it() {
        let copy = program([ScreenStage::Engine(5), E, E, E]);
        let back = program([ScreenStage::Engine(24), ScreenStage::Engine(3), E, E]);
        let into = Dest::Target1 { guest: (640, 480) };
        let q = |source, dest| ConvolutionDraw { source, dest, spread: 1.0, grid: None };
        let blur = [ScreenConvolution { size: (320, 240), draws: vec![q(1, 0x19), q(0x19, 0x1a)], result: 0x1a }];
        let none: &[ScreenConvolution] = &[];
        assert_eq!(check_order([(&copy, into, &blur[..]), (&back, Dest::Bound, none)]), Ok(()));
        assert_eq!(check_order([(&copy, into, none), (&back, Dest::Bound, none)]), Err(ScreenRefused::ResultUnset(24)), "-1 on entry");
        let mut d = Drawn::default();
        d.pass(&copy, into, &blur).unwrap();
        assert_eq!(d.stand_ins, [Some((320, 240)), Some((320, 240)), None]);
        assert_eq!(d.results, [Some(0x1a), None]);
        assert_eq!(d.reads(&back, Dest::Bound), Ok([Some(0x1a), Some(1), None, None]));
        assert_eq!(stage_sizes(&back, (640, 480), &d), [(320, 240), (640, 480), (0, 0), (0, 0)], "c18..c21 read these");
        // A stand-in read before it is drawn, a quad into its source or
        // outside the stand-ins, a kept one naming the pass's target.
        let early = [ScreenConvolution { size: (320, 240), draws: vec![q(0x1a, 0x19)], result: 0x19 }];
        assert_eq!(check_order([(&copy, into, &early[..])]), Err(ScreenRefused::Unwritten(0x1a)));
        let onto = [ScreenConvolution { size: (320, 240), draws: vec![q(1, 1)], result: 1 }];
        assert_eq!(check_order([(&copy, into, &onto[..])]), Err(ScreenRefused::ConvolutionTarget(1)));
        let kept = [ScreenConvolution { size: (0, 0), draws: vec![], result: 1 }];
        assert_eq!(check_order([(&copy, into, &kept[..]), (&back, Dest::Bound, none)]), Ok(()), "24 names target 1");
    }

    /// A wide convolution's draws are grids: ordered as the blur's quads,
    /// each drawing a triangle per strip from a source bound at a size.
    #[test]
    fn a_grid_draws_its_strips_from_its_bound_source() {
        let copy = program([ScreenStage::Engine(5), E, E, E]);
        let into = Dest::Target1 { guest: (640, 480) };
        let grid = ConvolutionGrid { bound: (640, 480), strips: vec![vec![StripVertex::default(); 42]; 30] };
        assert_eq!(grid.check(), Ok(()));
        let g = |source, dest| ConvolutionDraw { source, dest, spread: 0.0, grid: Some(grid.clone()) };
        let wide = [ScreenConvolution { size: (320, 240), draws: vec![g(1, 0x19), g(0x19, 0x1a)], result: 0x1a }];
        let mut d = Drawn::default();
        d.pass(&copy, into, &wide).unwrap();
        assert_eq!((d.stand_ins, d.results), ([Some((320, 240)), Some((320, 240)), None], [Some(0x1a), None]));
        let empty = ConvolutionGrid { bound: (640, 480), strips: vec![] };
        assert_eq!(empty.check(), Err(ScreenRefused::NoStrips));
        let short = ConvolutionGrid { strips: vec![vec![StripVertex::default(); 2]], ..grid.clone() };
        assert_eq!(short.check(), Err(ScreenRefused::NoStrips), "a strip under a triangle");
        assert_eq!(ConvolutionGrid { bound: (0, 480), ..grid }.check(), Err(ScreenRefused::ConvolutionSize));
        assert_eq!(ConvolutionGrid::default().check(), Err(ScreenRefused::NoStrips), "the order's placeholder never draws");
    }

    #[test]
    fn target_one_keeps_its_live_format_and_pixels_across_partial_passes() {
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let size = (8, 8);
        let extent = wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 };
        let make = |format| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("screen target format and load regression"),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::COPY_SRC
                    | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            })
        };
        let main = make(wgpu::TextureFormat::Rgba8Unorm);
        let level = make(wgpu::TextureFormat::Bgra8Unorm);
        let original = [51, 102, 153, 204];
        ctx.queue.write_texture(
            level.as_image_copy(),
            &original.repeat(64),
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(32), rows_per_image: Some(8) },
            extent,
        );
        let mut red = program([E; 4]);
        red.wgsl = "
            @vertex fn vs_main(@location(0) position: vec4f) -> @builtin(position) vec4f {
                return position;
            }
            @fragment fn fs_main() -> @location(0) vec4f {
                return vec4f(1.0);
            }
        ".into();
        red.state.color_writes = wgpu::ColorWrites::RED;
        let mut blue = red.clone();
        blue.state.color_writes = wgpu::ColorWrites::BLUE;
        let strip = |left, right| {
            vec![(left, 1.0), (left, -1.0), (right, 1.0), (right, -1.0)]
                .into_iter()
                .map(|(x, y)| StripVertex { position: [x, y, 0.0, 1.0], texcoords: [[0.0; 4]; 4] })
                .collect()
        };
        let left = [strip(-1.0, -0.25)];
        let right = [strip(0.25, 1.0)];
        let draw = |key, program, strips, raster| ScreenDraw {
            key, program, strips, raster, dest: Dest::Target1 { guest: size }, values: [0; 4], convolutions: &[], convolution_rasters: &[],
        };
        let arena = gfx::ring::UploadPool::default().begin();
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        let color = main.create_view(&Default::default());
        let target1 = level.create_view(&Default::default());
        let target = |pinned: bool| ScreenTarget {
            device: &ctx.device,
            queue: &ctx.queue,
            uploads: &arena,
            color: pinned.then_some(glow::PostSurface { view: &color, guest_size: size }),
            target1: Some(glow::PostSurface { view: &target1, guest_size: size }),
            convolutions: [None; 3],
            z_scale: 1.0,
        };
        let draws = [
            ScreenDraw { dest: Dest::Bound, ..draw((0, 0, 0), &red, &left[..], red.state.into()) },
            draw((0, 0, 0), &red, &left[..], red.state.into()),
            draw((0, 0, 0), &blue, &right[..], blue.state.into()),
        ];
        let mut screens = ScreenSet::new();
        assert_eq!(screens.encode(target(true), &mut encoder, &draws[..2], None, &mut |_| None), Ok(2));
        assert_eq!(screens.encode(target(false), &mut encoder, &draws[2..], None, &mut |_| None), Ok(1));
        arena.flush(&ctx.queue);
        ctx.queue.submit([encoder.finish()]);
        let framebuffer = gfx::readback::read_texture(&ctx, &main, wgpu::TextureAspect::All, 4, size);
        assert_eq!(&framebuffer[..4], [255, 0, 0, 0]);
        assert_eq!(&framebuffer[28..32], [0; 4], "the terminal blue strip belongs to target 1");
        let px = gfx::readback::read_texture(&ctx, &level, wgpu::TextureAspect::All, 4, size);
        assert_eq!(&px[..4], [51, 102, 255, 204], "the second pass kept the first pass and its other channels");
        assert_eq!(&px[16..20], original, "the gap kept its existing pixels");
        assert_eq!(&px[28..32], [255, 102, 153, 204], "the blue write used the actual BGRA target");
    }

    /// A call prepared ([`ScreenSet::prepare`]) and recorded into a
    /// frame's open encoder, after a dry run on a scratch encoder that is
    /// thrown away (the backend's validation), draws what the call
    /// recorded on an encoder of its own draws: a pass copying the view
    /// into target 1 and convolving it into a stand-in, then a pass over
    /// the view reading the result and an opaque copy of the view.
    #[test]
    fn a_prepared_call_recorded_twice_draws_what_its_own_encoder_draws() {
        use shaders::psh::PixelShaderDef;
        use shaders::psh::bits::{CHAN_ALPHA, REG_T0, final_efg, inputs};
        let Ok(ctx) = gfx::GfxContext::headless() else { return };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let size = (8, 8);
        let texture = |size: (u32, u32)| ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("screen call target"),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let fill = |t: &wgpu::Texture, pixels: &[u8]| ctx.queue.write_texture(
            t.as_image_copy(), pixels,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * t.width()), rows_per_image: Some(t.height()) },
            t.size(),
        );
        let read = |t: &wgpu::Texture| gfx::readback::read_texture(&ctx, t, wgpu::TextureAspect::All, 4, (t.width(), t.height()));
        let view_pixels: Vec<u8> = (0..64u8).flat_map(|i| [i * 4, 255 - i * 3, i * 2 + 7, 128]).collect();
        // Only the stages each program binds.
        let shader = |stages: usize, fragment: &str| format!("
            {}
            @vertex fn vs_main(@location(0) position: vec4f) -> @builtin(position) vec4f {{
                return position;
            }}
            @fragment fn fs_main(@builtin(position) p: vec4f) -> @location(0) vec4f {{
                {fragment}
            }}
        ", (0..stages).map(|n| format!("@group(1) @binding({}) var t{n}: texture_2d<f32>;", 2 * n)).collect::<String>());
        let mut copy = program([ScreenStage::Engine(5), E, E, E]);
        copy.wgsl = shader(1, "return textureLoad(t0, vec2i(p.xy), 0).bgra;");
        let mut back = program([ScreenStage::Engine(24), ScreenStage::Engine(5), E, E]);
        back.wgsl = shader(2, "return 0.5 * textureLoad(t0, vec2i(p.xy) / 2, 0) + 0.5 * textureLoad(t1, vec2i(p.xy), 0);");
        let strip: Vec<StripVertex> = [(-1.0, 1.0), (-1.0, -1.0), (1.0, 1.0), (1.0, -1.0)]
            .map(|(x, y)| StripVertex { position: [x, y, 0.0, 1.0], texcoords: [[0.0; 4]; 4] })
            .to_vec();
        let strips = [strip];
        let blur = [ScreenConvolution {
            size: (4, 4),
            draws: vec![ConvolutionDraw { source: 1, dest: 0x19, spread: 0.5, grid: None }],
            result: 0x19,
        }];
        let combiner = {
            let b = PixelShaderDef {
                texture_modes: 1,
                final_inputs_abcd: inputs(0, 0, 0, REG_T0),
                final_inputs_efg: final_efg(0, 0, REG_T0 | CHAN_ALPHA, 0),
                ..Default::default()
            }
            .to_bytes();
            std::array::from_fn(|i| u32::from_le_bytes([b[4 * i], b[4 * i + 1], b[4 * i + 2], b[4 * i + 3]]))
        };
        let frame = QuadFrame { window: [0.0, 0.0, 8.0, 8.0], signs: [-0.5, 0.5], combiner };
        let mut opaque: PostRaster = back.state.into();
        opaque.opaque_view = [true; 4];
        let tail = [ScreenConvolution {
            size: (4, 4),
            draws: vec![ConvolutionDraw { source: 0, dest: 0x19, spread: 0.0, grid: None }],
            result: 0x19,
        }];
        let run = |prepared: bool, terminal: bool| {
            let draws = [
                ScreenDraw {
                    key: (0, 0, 0), program: &copy, strips: &strips, raster: copy.state.into(),
                    dest: Dest::Target1 { guest: size }, values: [0; 4], convolutions: &blur, convolution_rasters: &[],
                },
                ScreenDraw {
                    key: (0, 1, 0), program: &back, strips: &strips, raster: opaque,
                    dest: Dest::Bound, values: [0; 4], convolutions: if terminal { &tail } else { &[] }, convolution_rasters: &[],
                },
            ];
            let (color, target1, stand_in) = (texture(size), texture(size), texture((4, 4)));
            fill(&color, &view_pixels);
            fill(&target1, &[1, 2, 3, 4].repeat(64));
            fill(&stand_in, &[5, 6, 7, 8].repeat(16));
            let views = [&color, &target1, &stand_in].map(|t| t.create_view(&Default::default()));
            let arena = gfx::ring::UploadPool::default().begin();
            let target = ScreenTarget {
                device: &ctx.device,
                queue: &ctx.queue,
                uploads: &arena,
                color: Some(glow::PostSurface { view: &views[0], guest_size: size }),
                target1: Some(glow::PostSurface { view: &views[1], guest_size: size }),
                convolutions: [Some(glow::PostSurface { view: &views[2], guest_size: (4, 4) }), None, None],
                z_scale: 1.0,
            };
            let mut set = ScreenSet::new();
            let mut encoder = ctx.device.create_command_encoder(&Default::default());
            let ranges = if prepared {
                let commands = set.prepare(target, &draws, Some(&frame), &mut |_| None).unwrap();
                assert_eq!(commands.passes(), 4 + usize::from(terminal));
                let mut scratch = ctx.device.create_command_encoder(&Default::default());
                commands.record(&mut scratch);
                drop(scratch.finish());
                commands.record(&mut encoder);
                commands.ranges()
            } else {
                set.encode(target, &mut encoder, &draws, Some(&frame), &mut |_| None).unwrap()
            };
            arena.flush(&ctx.queue);
            ctx.queue.submit([encoder.finish()]);
            (ranges, read(&color), read(&target1), read(&stand_in))
        };
        let (own, prepared) = (run(false, false), run(true, false));
        assert_eq!(own, prepared);
        assert_eq!(own.0, 3, "two strips and a quad");
        assert_ne!(&own.1[..4], &view_pixels[..4], "the second pass drew over the view");
        let terminal = run(true, true);
        assert_eq!(terminal.0, 4);
        assert_eq!(terminal.1, own.1, "the terminal quad preserves the framebuffer");
        assert_eq!(terminal.2, own.2, "target 1 remains available to later draws");
        assert_ne!(terminal.3, own.3, "the terminal quad updates the live stand-in");
    }

    #[test]
    fn the_uniforms_are_the_targets() {
        let p = program([ScreenStage::Engine(5), ScreenStage::Bitmap { bitmap: map::TagId(1), image: 0 }, ScreenStage::Engine(3), E]);
        let v = screen_vertex(&p.vertex, (640, 480), (1280, 960), 16_777_215.0);
        assert_eq!((v.surface, v.z_max, v.w_clip), ([640.0, 480.0], 16_777_215.0, None));
        assert_eq!(v.shift, [3.0 / 8.0; 2], "2x: (2 s - 1) / 4 s");
        assert_eq!(screen_vertex(&p.vertex, (640, 480), (640, 480), 1.0).shift, [0.0; 2]);
        let drawn = Drawn { target1: Some((320, 240)), ..Default::default() };
        let px = screen_pixel(&p, (640, 480), &drawn);
        assert_eq!(px.tex_scale[0], [1.0 / 640.0, 1.0 / 480.0, 1.0, 1.0], "the view in texels");
        assert_eq!(px.tex_scale[1], [1.0; 4], "a bitmap normalized");
        assert_eq!(px.tex_scale[2], [1.0 / 320.0, 1.0 / 240.0, 1.0, 1.0], "target 1 in its texels");
        assert_eq!(px.fog, psh::fog::Fog::OFF.uniform());
        assert_eq!(p.dims[0], Some(TextureDim::D2));
    }

    /// The call's values into the combiner constants, its targets' guest
    /// sizes into the vertex constants that read a stage's size.
    #[test]
    fn a_calls_values_and_its_targets_sizes_go_in() {
        use tags::material::{EngineVertexConstant, VectorBinding, VectorTarget};
        let mut p = program([ScreenStage::Engine(5), ScreenStage::Engine(3), E, E]);
        let drawn = Drawn { target1: Some((320, 240)), ..Default::default() };
        assert_eq!(stage_sizes(&p, (640, 480), &drawn), [(640, 480), (320, 240), (0, 0), (0, 0)]);
        let size = |reg, stage| (EngineVertexConstant { bank: 1, reg, mode: 13, stage, source: 0 }, 0b1111);
        p.constants.vertex = vec![size(0, 1), size(1, 2)];
        let complement = VectorTarget::Pixel { c1: true, slot: 8, mask: 0x00ff_0000, half: false, transform: 1 };
        p.constants.pixel = vec![VectorBinding { source: 42, target: complement }];
        let values = [0.0f32.to_bits(), 0.25f32.to_bits(), 0, 0];
        let (v, px) = screen_uniforms(&p, values, ((640, 480), &drawn), ((640, 480), (1280, 960)), 1.0);
        assert_eq!(v.constants[18], [640.0, 480.0, 0.0, 0.0], "the view's guest size");
        assert_eq!(v.constants[19], [320.0, 240.0, 0.0, 0.0], "target 1's");
        assert_eq!(v.surface, [640.0, 480.0]);
        assert_eq!(crate::light_look::repack(px.c1[8]) & 0x00ff_0000, 191 << 16, "1 - 0.25 of 255, in red");
        assert_eq!(px.tex_scale[1], [1.0 / 320.0, 1.0 / 240.0, 1.0, 1.0]);
    }
}
