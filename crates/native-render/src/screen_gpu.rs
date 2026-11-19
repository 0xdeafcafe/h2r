//! Stage 5's screen-effect pass on the GPU (docs/graphics/native-renderer.md,
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
//! Where a pass draws ([`Dest`]): `kind` 0 into the bound target, 1..3
//! into target 1 at the window's size, a half or a quarter (`0x00016610`
//! binds it through `0x00014bc0(1)` after `0x0001ddd0` sized it). Target 1
//! is a stand-in here, its guest size the pass's and its texels that times
//! the copy's render scale, cleared when a pass draws into it (S: the
//! strips cover the target); what a later pass of the same call samples
//! of it (engine textures 2 and 3) is what the earlier drew. Target 1
//! sampled before any pass of the call drew it is refused: its contents
//! would be the D3D path's.
//!
//! The program is the game's: the vertex program translated with the
//! destination's guest size as its surface and the device's depth scale
//! as its `z_max` ([`screen_vertex`]); the pixel side with each
//! render-target stage's coordinates scaled by that target's guest size,
//! as a linear texture is fetched in texels ([`screen_pixel`]). What a
//! stage binds ([`bound`]): the shader's own bitmaps; of the engine
//! textures the view's (`5`), a copy of the bound colour taken before the
//! draw, since a pass reads the target it draws into (S: that id 5
//! resolves to the target `kind` 0 draws into, both target 0 through
//! `0x00014f60` while `[0x004858b8]` is clear, which every drawn call
//! needs), and target 1's stand-in. The rest are refused until their
//! targets have stand-ins.
//!
//! Colour only, culling nothing (S: the D3D path's cull mode is whatever
//! the draws before left; the A/B checks the strips' facing), the pass's
//! own blend and colour writes.

use std::collections::HashMap;
use std::ops::Range;

use shaders::psh::{self, PixelUniforms};
use shaders::vsh::VertexUniforms;

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

/// An engine texture the native pass supplies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bound {
    /// The view's (5): a copy of the bound colour.
    View,
    /// Target 1 (2 and 3): the stand-in an earlier pass of the call drew.
    Target1,
}

/// What engine texture `id` binds in the native pass, `None` when it
/// does not supply it.
pub fn bound(id: u8) -> Option<Bound> {
    match engine_texture(id) {
        EngineTexture::View => Some(Bound::View),
        EngineTexture::Target(1) => Some(Bound::Target1),
        _ => None,
    }
}

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
    /// No program for this pass (the load refused it).
    NoProgram,
    /// The strip has fewer than three vertices or the call no strips.
    NoStrips,
    /// Target 1 sampled before a pass of the call drew it.
    Target1Unwritten,
    /// A pass into target 1 that samples it.
    Target1Feedback,
    /// A pass into target 1 at an empty size.
    Target1Size,
}

/// Whether `program` would draw into `color` (recording nothing): every
/// sampled stage bound with what the pass supplies, the colour copyable
/// when it is sampled.
pub fn check(color: &wgpu::TextureView, program: &ScreenProgram) -> Result<(), ScreenRefused> {
    for (n, d) in program.dims.iter().enumerate() {
        if d.is_none() {
            continue;
        }
        match program.stages[n] {
            ScreenStage::Empty => return Err(ScreenRefused::Unbound(n)),
            ScreenStage::Bitmap { .. } => {}
            ScreenStage::Engine(id) => match bound(id) {
                None => return Err(ScreenRefused::EngineTexture(id)),
                Some(Bound::View) => {
                    if !color.texture().usage().contains(wgpu::TextureUsages::COPY_SRC) {
                        return Err(ScreenRefused::ColourNotCopyable);
                    }
                }
                Some(Bound::Target1) => {}
            },
        }
    }
    Ok(())
}

/// Whether passes drawing `programs` into `dests` in order read target 1
/// only after one drew it, never while drawing it, and draw it at a size.
pub fn check_order<'a>(passes: impl IntoIterator<Item = (&'a ScreenProgram, Dest)>) -> Result<(), ScreenRefused> {
    let mut written = false;
    for (program, dest) in passes {
        let reads = samples(program, Bound::Target1);
        if reads && !written {
            return Err(ScreenRefused::Target1Unwritten);
        }
        if let Dest::Target1 { guest } = dest {
            if reads {
                return Err(ScreenRefused::Target1Feedback);
            }
            if guest.0 == 0 || guest.1 == 0 {
                return Err(ScreenRefused::Target1Size);
            }
            written = true;
        }
    }
    Ok(())
}

/// `guest` guest pixels in a copy `size` texels for `of` guest pixels:
/// the stand-in's texels, at least 1x1.
pub fn host_size(guest: (u32, u32), of: (u32, u32), size: (u32, u32)) -> (u32, u32) {
    let scale = |n: u32, s: u32, g: u32| ((u64::from(n) * u64::from(s)).div_ceil(u64::from(g.max(1))) as u32).max(1);
    (scale(guest.0, size.0, of.0), scale(guest.1, size.1, of.1))
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
    let s = size.0 as f32 / guest_size.0.max(1) as f32;
    // As the D3D path shifts a scaled target (gfx::renderer::window_shift).
    u.shift = if s > 1.0 { (2.0 * s - 1.0) / (4.0 * s) } else { 0.0 };
    u
}

/// The pixel uniforms: each engine stage (a render target, linear) fetched
/// in its texels, `view` the bound colour's guest size and `target1`
/// target 1's; no fog.
pub fn screen_pixel(program: &ScreenProgram, view: (u32, u32), target1: (u32, u32)) -> PixelUniforms {
    let mut p = PixelUniforms { fog: psh::fog::Fog::OFF.uniform(), ..program.pixel };
    let texels = |s: (u32, u32)| [1.0 / s.0.max(1) as f32, 1.0 / s.1.max(1) as f32, 1.0, 1.0];
    for n in 0..4 {
        if program.dims[n].is_none() {
            continue;
        }
        if let ScreenStage::Engine(id) = program.stages[n] {
            p.tex_scale[n] = match bound(id) {
                Some(Bound::Target1) => texels(target1),
                _ => texels(view),
            };
        }
    }
    p
}

/// What a screen pass draws into.
pub struct ScreenTarget<'a> {
    pub device: &'a wgpu::Device,
    pub queue: &'a wgpu::Queue,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub color: &'a wgpu::TextureView,
    pub color_format: wgpu::TextureFormat,
    /// The colour's size in guest pixels and in this copy's texels.
    pub guest_size: (u32, u32),
    pub size: (u32, u32),
    /// The device's depth scale (`+0x948`).
    pub z_scale: f32,
}

/// One compiled screen pass on the GPU.
pub struct ScreenGpu {
    pub program: ScreenProgram,
    module: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    uniforms_layout: wgpu::BindGroupLayout,
    textures_layout: wgpu::BindGroupLayout,
    samplers: [wgpu::Sampler; 4],
    stage_views: [Option<wgpu::TextureView>; 4],
    pipelines: HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
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
        let uniform = |binding, size: usize| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
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
    fn pipeline(&mut self, device: &wgpu::Device, color: wgpu::TextureFormat) -> &wgpu::RenderPipeline {
        let (module, layout, s) = (&self.module, &self.layout, self.program.state);
        self.pipelines.entry(color).or_insert_with(|| {
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
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    cull_mode: None,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: color,
                        blend: s.blend,
                        write_mask: s.color_writes,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        })
    }

    /// The texture group: `view` the copy of the bound colour, `target1`
    /// target 1's stand-in (for a pass that samples them).
    fn textures(
        &self,
        device: &wgpu::Device,
        view: Option<&wgpu::TextureView>,
        target1: Option<&wgpu::TextureView>,
    ) -> Result<wgpu::BindGroup, ScreenRefused> {
        let mut views = Vec::new();
        for (n, d) in self.program.dims.iter().enumerate() {
            if d.is_none() {
                continue;
            }
            let v = match self.program.stages[n] {
                ScreenStage::Empty => return Err(ScreenRefused::Unbound(n)),
                ScreenStage::Bitmap { .. } => self.stage_views[n].clone().ok_or(ScreenRefused::Bitmap(n))?,
                ScreenStage::Engine(id) => match bound(id) {
                    None => return Err(ScreenRefused::EngineTexture(id)),
                    Some(Bound::View) => view.cloned().ok_or(ScreenRefused::ColourNotCopyable)?,
                    Some(Bound::Target1) => target1.cloned().ok_or(ScreenRefused::Target1Unwritten)?,
                },
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

/// One pass of a call: its program, strips and destination, in draw order.
pub struct ScreenDraw<'a> {
    pub key: ScreenKey,
    pub program: &'a ScreenProgram,
    pub strips: &'a [Vec<StripVertex>],
    pub dest: Dest,
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

/// The screen passes on the GPU, made as calls first need them, the copy
/// of the view's colour the passes that sample it read, and target 1's
/// stand-in.
#[derive(Default)]
pub struct ScreenSet {
    passes: HashMap<ScreenKey, Result<ScreenGpu, ScreenRefused>>,
    copy: Option<Kept>,
    target1: Option<Kept>,
}

impl ScreenSet {
    pub fn new() -> Self {
        Self::default()
    }

    /// Forgets every pass (a new map's effects).
    pub fn clear(&mut self) {
        self.passes.clear();
    }

    /// Records `draws` in order, each in its own render pass: into
    /// `t.color` or target 1's stand-in, after a copy of the colour when
    /// it samples it (so a later pass reads what the earlier drew, as on
    /// the D3D path). Nothing is recorded when any of them is refused.
    pub fn encode(
        &mut self,
        t: ScreenTarget<'_>,
        draws: &[ScreenDraw<'_>],
        stage_image: &mut dyn FnMut(ScreenStage) -> Option<Image>,
    ) -> Result<usize, ScreenRefused> {
        let ScreenTarget { device, queue, encoder, color, color_format, guest_size, size, z_scale } = t;
        check_order(draws.iter().map(|d| (d.program, d.dest)))?;
        for d in draws {
            check(color, d.program)?;
            if d.strips.is_empty() || d.strips.iter().any(|s| s.len() < 3) {
                return Err(ScreenRefused::NoStrips);
            }
            let stale = matches!(self.passes.get(&d.key), Some(Ok(g)) if g.program.wgsl != d.program.wgsl);
            if stale || !self.passes.contains_key(&d.key) {
                let g = ScreenGpu::new(device, queue, d.program.clone(), &mut *stage_image);
                self.passes.insert(d.key, g);
            }
            match self.passes.get_mut(&d.key) {
                Some(Ok(g)) => {
                    g.pipeline(device, color_format);
                }
                Some(Err(r)) => return Err(r.clone()),
                None => return Err(ScreenRefused::NoProgram),
            }
        }
        let texture = color.texture();
        let full = wgpu::Extent3d { width: texture.width(), height: texture.height(), depth_or_array_layers: 1 };
        let mut drawn = 0;
        // Target 1's guest size, once a pass of this call drew it.
        let mut target1_guest = None;
        for d in draws {
            let Some(Ok(g)) = self.passes.get(&d.key) else { continue };
            let view = if samples(d.program, Bound::View) {
                let usage = wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST;
                let (extent, _, copy, view) = keep(&mut self.copy, device, full, texture.format(), usage, "native screen view copy");
                encoder.copy_texture_to_texture(texture.as_image_copy(), copy.as_image_copy(), *extent);
                Some(view.clone())
            } else {
                None
            };
            let (dest_view, dest_guest, dest_size, load) = match d.dest {
                Dest::Bound => (color.clone(), guest_size, size, wgpu::LoadOp::Load),
                Dest::Target1 { guest } => {
                    let texels = host_size(guest, guest_size, size);
                    let extent = wgpu::Extent3d { width: texels.0, height: texels.1, depth_or_array_layers: 1 };
                    let usage = wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING;
                    let (_, _, _, v) = keep(&mut self.target1, device, extent, color_format, usage, "native screen target 1");
                    target1_guest = Some(guest);
                    (v.clone(), guest, texels, wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT))
                }
            };
            let target1 = match (samples(d.program, Bound::Target1), &self.target1) {
                (true, Some((_, _, _, v))) => Some(v),
                _ => None,
            };
            let textures = g.textures(device, view.as_ref(), target1)?;
            let vertex = screen_vertex(&d.program.vertex, dest_guest, dest_size, z_scale).to_bytes();
            let pixel = screen_pixel(d.program, guest_size, target1_guest.unwrap_or((1, 1))).to_bytes();
            use wgpu::util::DeviceExt;
            let buffer = |label, contents: &[u8], usage| {
                device.create_buffer_init(&wgpu::util::BufferInitDescriptor { label: Some(label), contents, usage })
            };
            let vb = buffer("native screen vertex uniforms", &vertex, wgpu::BufferUsages::UNIFORM);
            let pb = buffer("native screen pixel uniforms", &pixel, wgpu::BufferUsages::UNIFORM);
            let all: Vec<StripVertex> = d.strips.iter().flatten().copied().collect();
            let vertices = buffer("native screen strips", bytemuck::cast_slice(&all), wgpu::BufferUsages::VERTEX);
            let uniforms = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("native screen uniforms"),
                layout: &g.uniforms_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: vb.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: pb.as_entire_binding() },
                ],
            });
            let Some(pipeline) = g.pipelines.get(&color_format) else { continue };
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("native screen effect"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &dest_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_viewport(0.0, 0.0, dest_size.0 as f32, dest_size.1 as f32, 0.0, 1.0);
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &uniforms, &[]);
            pass.set_bind_group(1, &textures, &[]);
            pass.set_blend_constant(g.program.state.blend_constant);
            pass.set_vertex_buffer(0, vertices.slice(..));
            for r in strip_ranges(d.strips) {
                pass.draw(r, 0..1);
                drawn += 1;
            }
        }
        Ok(drawn)
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
            inputs: 0x1f,
            state: PassState {
                blend: None,
                blend_constant: wgpu::Color::TRANSPARENT,
                color_writes: wgpu::ColorWrites::ALL,
                depth_write: false,
                depth_compare: wgpu::CompareFunction::Always,
                primitive: wgpu::PrimitiveState::default(),
                alpha_test: None,
            },
        }
    }

    const E: ScreenStage = ScreenStage::Empty;

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
    fn strips_draw_one_after_another() {
        let s = |n| vec![StripVertex::default(); n];
        assert_eq!(strip_ranges(&[s(6), s(6), s(4)]), vec![0..6, 6..12, 12..16]);
        assert!(strip_ranges(&[]).is_empty());
    }

    #[test]
    fn the_view_and_target_1_are_supplied() {
        assert_eq!(bound(5), Some(Bound::View));
        assert_eq!((bound(2), bound(3)), (Some(Bound::Target1), Some(Bound::Target1)));
        for id in [1, 4, 6, 9, 13, 24, 25] {
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
        assert_eq!(check_order([(&copy, into), (&back, Dest::Bound)]), Ok(()));
        assert_eq!(check_order([(&back, Dest::Bound)]), Err(ScreenRefused::Target1Unwritten), "the D3D path's contents");
        assert_eq!(check_order([(&copy, into), (&back, into)]), Err(ScreenRefused::Target1Feedback));
        let empty = Dest::Target1 { guest: (0, 240) };
        assert_eq!(check_order([(&copy, empty)]), Err(ScreenRefused::Target1Size));
        assert_eq!(check_order([(&copy, Dest::Bound)]), Ok(()), "desaturate's");
    }

    #[test]
    fn a_stand_in_is_its_guest_size_at_the_copys_scale() {
        assert_eq!(host_size((320, 240), (640, 480), (1280, 960)), (640, 480), "a half at 2x");
        assert_eq!(host_size((160, 120), (640, 480), (640, 480)), (160, 120));
        assert_eq!(host_size((161, 1), (640, 480), (1000, 750)), (252, 2), "rounded up");
        assert_eq!(host_size((0, 0), (640, 480), (640, 480)), (1, 1));
    }

    #[test]
    fn the_uniforms_are_the_targets() {
        let p = program([ScreenStage::Engine(5), ScreenStage::Bitmap { bitmap: map::TagId(1), image: 0 }, ScreenStage::Engine(3), E]);
        let v = screen_vertex(&p.vertex, (640, 480), (1280, 960), 16_777_215.0);
        assert_eq!((v.surface, v.z_max, v.w_clip), ([640.0, 480.0], 16_777_215.0, None));
        assert_eq!(v.shift, 3.0 / 8.0, "2x: (2 s - 1) / 4 s");
        assert_eq!(screen_vertex(&p.vertex, (640, 480), (640, 480), 1.0).shift, 0.0);
        let px = screen_pixel(&p, (640, 480), (320, 240));
        assert_eq!(px.tex_scale[0], [1.0 / 640.0, 1.0 / 480.0, 1.0, 1.0], "the view in texels");
        assert_eq!(px.tex_scale[1], [1.0; 4], "a bitmap normalized");
        assert_eq!(px.tex_scale[2], [1.0 / 320.0, 1.0 / 240.0, 1.0, 1.0], "target 1 in its texels");
        assert_eq!(px.fog, psh::fog::Fog::OFF.uniform());
        assert_eq!(p.dims[0], Some(TextureDim::D2));
    }
}
