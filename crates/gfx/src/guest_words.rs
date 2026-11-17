//! Guest texel words on the GPU: render-to-texture levels written into, and
//! filled from, a GPU buffer that holds guest memory bytes, and textures
//! over the depth buffer's memory decoded on the GPU.
//!
//! Two surface headers can name the same guest memory (a pool of render
//! targets that share one allocation). Moving pixels from one to the other
//! went through the CPU: read the first back in its guest format and
//! layout, store the bytes, decode them into the second. Here the bytes stay
//! on the GPU, in a storage buffer standing for that memory (the *mirror*):
//!
//! * [`GuestWords::store_color`] / [`GuestWords::store_depth`] encode a
//!   surface into the mirror as its guest texels: 32-bit colour words with
//!   the channel order of the format, or `D24S8` words `z << 8 | stencil`,
//!   swizzled or in rows of any pitch, from any word offset. This is
//!   [`encode_color`](crate::encode::encode_color) and
//!   [`encode_depth`](crate::encode::encode_depth) on the GPU, with the
//!   same rounding (depth: [`depth_bits`](crate::depth_bits)).
//! * [`GuestWords::load_color`] decodes mirror words into a colour surface
//!   (a render pass that writes every texel), as
//!   [`SurfaceStore::write_color`](crate::surface::SurfaceStore::write_color)
//!   does from CPU bytes. [`GuestWords::load_depth`] does the same for a
//!   depth surface from `D24S8` words, as
//!   [`SurfaceStore::write_depth`](crate::surface::SurfaceStore::write_depth)
//!   does (depth drawn as `frag_depth`, with the CPU division's rounding;
//!   stencil copied).
//! * [`GuestWords::depth_alias`] makes the texture a 32-bit colour texture
//!   over the depth buffer's memory decodes to: the depth buffer's `D24S8`
//!   words (linear rows of the buffer's width) read from a byte offset as
//!   the texture's format, swizzled or in rows of its pitch, with
//!   `COLORSIGN` channels biased, as
//!   [`decode_texture`](crate::texture::decode_texture) of the read-back
//!   bytes gives.
//!
//! Only 32-bit formats whose decoded texels are a byte permutation of the
//! guest word go here ([`WordFormat`]); callers keep the CPU path for the
//! rest. Every texel is an exact `u8` round trip (`textureLoad` of a UNORM8
//! texel times 255 is an integer; `k / 255` stores as `k`).

use crate::context::GfxContext;
use crate::format::{D3DFormat, Layout, TexelFormat};

/// A 32-bit texel format as a byte permutation: logical red, green, blue
/// and alpha are bytes `perm[0..4]` of the little-endian guest word, and an
/// `opaque` format reads alpha as 255 (and writes it as 255).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WordFormat {
    /// Byte of the guest word for R, G, B, A.
    pub perm: [u8; 4],
    /// `X8`: alpha is 255 whatever the word holds.
    pub opaque: bool,
}

impl WordFormat {
    /// The permutation for `format`, if it is one of the 32-bit colour
    /// formats handled here (`A8R8G8B8`, `X8R8G8B8`, `A8B8G8R8`,
    /// `B8G8R8A8`, `R8G8B8A8`, swizzled or linear).
    pub fn of(format: D3DFormat) -> Option<Self> {
        let (perm, opaque) = match format.texel() {
            TexelFormat::A8R8G8B8 => ([2, 1, 0, 3], false),
            TexelFormat::X8R8G8B8 => ([2, 1, 0, 3], true),
            TexelFormat::A8B8G8R8 => ([0, 1, 2, 3], false),
            TexelFormat::B8G8R8A8 => ([1, 2, 3, 0], false),
            TexelFormat::R8G8B8A8 => ([3, 2, 1, 0], false),
            _ => return None,
        };
        matches!(format.layout(), Layout::Swizzled | Layout::Linear)
            .then_some(Self { perm, opaque })
    }

    fn packed(self) -> u32 {
        u32::from_le_bytes(self.perm)
    }
}

/// Where a level's words are in the mirror (or, for a depth alias, in the
/// depth buffer's memory): from word `base`, swizzled (`pitch_words` 0) or
/// in rows `pitch_words` words apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WordLayout {
    /// Word offset of the level's first texel.
    pub base: u32,
    /// Row pitch in words; 0 for swizzled.
    pub pitch_words: u32,
}

const COMMON: &str = r#"
struct Params {
    width: u32, height: u32, base_word: u32, pitch_words: u32,
    lw: u32, lh: u32, perm: u32, opaque: u32,
    src_width: u32, src_height: u32, has_stencil: u32, flip: u32,
}

// The NV2A's 2D swizzle: bit positions handed out from the bottom,
// round-robin x then y, skipping an axis once its bits are placed.
fn morton(x: u32, y: u32, lw: u32, lh: u32) -> u32 {
    var out = 0u;
    var pos = 0u;
    var bx = 0u;
    var by = 0u;
    loop {
        if (bx >= lw && by >= lh) { break; }
        if (bx < lw) { out = out | (((x >> bx) & 1u) << pos); pos = pos + 1u; bx = bx + 1u; }
        if (by < lh) { out = out | (((y >> by) & 1u) << pos); pos = pos + 1u; by = by + 1u; }
    }
    return out;
}

fn word_index(p: Params, x: u32, y: u32) -> u32 {
    if (p.pitch_words == 0u) {
        return p.base_word + morton(x, y, p.lw, p.lh);
    }
    return p.base_word + y * p.pitch_words + x;
}

fn byte_of(w: u32, i: u32) -> u32 {
    return (w >> (8u * i)) & 0xffu;
}

// Logical RGBA bytes of a guest word.
fn unpack(p: Params, w: u32) -> vec4<u32> {
    var c = vec4<u32>(
        byte_of(w, p.perm & 0xffu),
        byte_of(w, (p.perm >> 8u) & 0xffu),
        byte_of(w, (p.perm >> 16u) & 0xffu),
        byte_of(w, (p.perm >> 24u) & 0xffu),
    );
    if (p.opaque != 0u) { c.w = 255u; }
    return c;
}

// The guest word of logical RGBA bytes.
fn pack(p: Params, c0: vec4<u32>) -> u32 {
    var c = c0;
    if (p.opaque != 0u) { c.w = 255u; }
    return (c.x << (8u * (p.perm & 0xffu)))
        | (c.y << (8u * ((p.perm >> 8u) & 0xffu)))
        | (c.z << (8u * ((p.perm >> 16u) & 0xffu)))
        | (c.w << (8u * ((p.perm >> 24u) & 0xffu)));
}

fn to_bytes(v: vec4<f32>) -> vec4<u32> {
    return vec4<u32>(round(clamp(v, vec4<f32>(0.0), vec4<f32>(1.0)) * 255.0));
}

// `encode_depth`'s D24S8 word: z = round(clamp(d, 0, 1) * 16777215), halves
// away from zero (see depth_bits.rs).
fn depth_word(d: f32, s: u32) -> u32 {
    let x = clamp(d, 0.0, 1.0) * 16777215.0;
    let t = trunc(x);
    var z = u32(t);
    if (x - t >= 0.5) { z = z + 1u; }
    return (z << 8u) | (s & 0xffu);
}

// `decode_depth`'s depth for a `D24S8` word, `z / 16777215` rounded to
// nearest exactly as the CPU's `f32` division does (WGSL division is not
// correctly rounded). With `b` the bit length of `z` and `Z = z << (24 - b)`
// in `[2^23, 2^24)`, `z / (2^24 - 1) * 2^(48 - b) = Z + Z / (2^24 - 1)`,
// whose fraction is in `(0.5, 1]`: the 24-bit significand is `Z + 1`.
fn depth_of_word(w: u32) -> f32 {
    let z = w >> 8u;
    if (z == 0u) { return 0.0; }
    let b = firstLeadingBit(z) + 1u;
    return ldexp(f32((z << (24u - b)) + 1u), i32(b) - 48);
}
"#;

const STORE_COLOR: &str = r#"
@group(0) @binding(0) var src: texture_2d<f32>;
@group(0) @binding(1) var<storage, read_write> mirror: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) { return; }
    let c = to_bytes(textureLoad(src, vec2<i32>(id.xy), 0));
    mirror[word_index(params, id.x, id.y)] = pack(params, c);
}
"#;

const STORE_DEPTH: &str = r#"
@group(0) @binding(0) var depth_tex: texture_depth_2d;
@group(0) @binding(1) var stencil_tex: texture_2d<u32>;
@group(0) @binding(2) var<storage, read_write> mirror: array<u32>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) { return; }
    let q = vec2<i32>(id.xy);
    var s = 0u;
    if (params.has_stencil != 0u) { s = textureLoad(stencil_tex, q, 0).r; }
    mirror[word_index(params, id.x, id.y)] = depth_word(textureLoad(depth_tex, q, 0), s);
}
"#;

const LOAD_COLOR: &str = r#"
@group(0) @binding(0) var<storage, read> mirror: array<u32>;
@group(0) @binding(1) var<uniform> params: Params;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @location(0) vec4<f32> {
    let w = mirror[word_index(params, u32(p.x), u32(p.y))];
    return vec4<f32>(unpack(params, w)) / 255.0;
}
"#;

const LOAD_DEPTH: &str = r#"
@group(0) @binding(0) var<storage, read> mirror: array<u32>;
@group(0) @binding(1) var<uniform> params: Params;

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let x = f32(i32(i & 1u) * 4 - 1);
    let y = f32(i32(i >> 1u) * 4 - 1);
    return vec4<f32>(x, y, 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) p: vec4<f32>) -> @builtin(frag_depth) f32 {
    return depth_of_word(mirror[word_index(params, u32(p.x), u32(p.y))]);
}
"#;

// Stencil bytes of the level's words, packed four to a word in rows of
// `src_width` words (a buffer the stencil aspect is copied from).
const LOAD_STENCIL: &str = r#"
@group(0) @binding(0) var<storage, read> mirror: array<u32>;
@group(0) @binding(1) var<storage, read_write> out_words: array<u32>;
@group(0) @binding(2) var<uniform> params: Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    let x0 = id.x * 4u;
    if (x0 >= params.width || id.y >= params.height) { return; }
    var packed = 0u;
    for (var k = 0u; k < 4u; k = k + 1u) {
        if (x0 + k < params.width) {
            packed = packed | ((mirror[word_index(params, x0 + k, id.y)] & 0xffu) << (8u * k));
        }
    }
    out_words[id.y * params.src_width + id.x] = packed;
}
"#;

const DEPTH_ALIAS: &str = r#"
@group(0) @binding(0) var depth_tex: texture_depth_2d;
@group(0) @binding(1) var stencil_tex: texture_2d<u32>;
@group(0) @binding(2) var out_tex: texture_storage_2d<rgba8unorm, write>;
@group(0) @binding(3) var<uniform> params: Params;

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= params.width || id.y >= params.height) { return; }
    // The texel's word in the depth buffer's memory: linear rows of
    // src_width words.
    let k = word_index(params, id.x, id.y);
    let q = vec2<i32>(i32(k % params.src_width), i32(k / params.src_width));
    var s = 0u;
    if (params.has_stencil != 0u) { s = textureLoad(stencil_tex, q, 0).r; }
    let w = depth_word(textureLoad(depth_tex, q, 0), s);
    var c = unpack(params, w);
    c = c ^ vec4<u32>(params.flip & 0xffu, (params.flip >> 8u) & 0xffu,
                      (params.flip >> 16u) & 0xffu, (params.flip >> 24u) & 0xffu);
    textureStore(out_tex, vec2<i32>(id.xy), vec4<f32>(c) / 255.0);
}
"#;

/// Size of the WGSL `Params` uniform.
const PARAMS_BYTES: u64 = 48;

/// One level as the shaders see it.
#[derive(Clone, Copy, Debug, Default)]
struct Params {
    width: u32,
    height: u32,
    base_word: u32,
    pitch_words: u32,
    lw: u32,
    lh: u32,
    perm: u32,
    opaque: u32,
    src_width: u32,
    src_height: u32,
    has_stencil: u32,
    flip: u32,
}

impl Params {
    fn level(size: (u32, u32), layout: WordLayout) -> Self {
        Self {
            width: size.0,
            height: size.1,
            base_word: layout.base,
            pitch_words: layout.pitch_words,
            lw: size.0.max(1).trailing_zeros(),
            lh: size.1.max(1).trailing_zeros(),
            ..Self::default()
        }
    }

    fn bytes(&self) -> Vec<u8> {
        [
            self.width,
            self.height,
            self.base_word,
            self.pitch_words,
            self.lw,
            self.lh,
            self.perm,
            self.opaque,
            self.src_width,
            self.src_height,
            self.has_stencil,
            self.flip,
        ]
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect()
    }
}

/// Why a level cannot go through [`GuestWords`].
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WordsError {
    /// Not a 32-bit permutation format ([`WordFormat::of`]).
    #[error("{0:?} is not a 32-bit word format")]
    Format(D3DFormat),
    /// Swizzled with a size that is not a power of two.
    #[error("swizzled {0}x{1} is not a power of two")]
    NotPowerOfTwo(u32, u32),
    /// The texture is not a single-sample `Depth32Float(Stencil8)` one.
    #[error("depth texture {0:?} cannot be read by a shader")]
    Depth(wgpu::TextureFormat),
    /// The words would run past the buffer.
    #[error("words up to {need} past the {have}-word buffer")]
    Bounds {
        /// Words needed.
        need: u64,
        /// Words there.
        have: u64,
    },
}

/// The pipelines (made once per device).
pub struct GuestWords {
    store_color: wgpu::ComputePipeline,
    store_depth: wgpu::ComputePipeline,
    load_color: std::collections::HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    load_module: wgpu::ShaderModule,
    load_depth: std::collections::HashMap<wgpu::TextureFormat, wgpu::RenderPipeline>,
    load_depth_module: wgpu::ShaderModule,
    load_stencil: wgpu::ComputePipeline,
    depth_alias: wgpu::ComputePipeline,
}

fn module(device: &wgpu::Device, label: &str, body: &str) -> wgpu::ShaderModule {
    device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some(label),
        source: wgpu::ShaderSource::Wgsl(format!("{COMMON}{body}").into()),
    })
}

fn compute(device: &wgpu::Device, label: &str, body: &str) -> wgpu::ComputePipeline {
    let m = module(device, label, body);
    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some(label),
        layout: None,
        module: &m,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

/// The last word a level of `size` at `layout` touches, plus one.
fn words_end(size: (u32, u32), layout: WordLayout) -> u64 {
    let (w, h) = (size.0.max(1) as u64, size.1.max(1) as u64);
    let span = if layout.pitch_words == 0 {
        w * h
    } else {
        (h - 1) * layout.pitch_words as u64 + w
    };
    layout.base as u64 + span
}

fn check_layout(size: (u32, u32), layout: WordLayout) -> Result<(), WordsError> {
    if layout.pitch_words == 0 && !(size.0.is_power_of_two() && size.1.is_power_of_two()) {
        return Err(WordsError::NotPowerOfTwo(size.0, size.1));
    }
    Ok(())
}

impl GuestWords {
    /// Builds the pipelines.
    pub fn new(ctx: &GfxContext) -> Self {
        let device = &ctx.device;
        Self {
            store_color: compute(device, "gfx guest words store colour", STORE_COLOR),
            store_depth: compute(device, "gfx guest words store depth", STORE_DEPTH),
            load_module: module(device, "gfx guest words load colour", LOAD_COLOR),
            load_color: std::collections::HashMap::new(),
            load_depth_module: module(device, "gfx guest words load depth", LOAD_DEPTH),
            load_depth: std::collections::HashMap::new(),
            load_stencil: compute(device, "gfx guest words load stencil", LOAD_STENCIL),
            depth_alias: compute(device, "gfx guest words depth alias", DEPTH_ALIAS),
        }
    }

    fn uniform(ctx: &GfxContext, p: &Params) -> wgpu::Buffer {
        let b = crate::churn::create_buffer(
            &ctx.device,
            &wgpu::BufferDescriptor {
                label: Some("gfx guest words params"),
                size: PARAMS_BYTES,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            },
        );
        crate::churn::queue_write();
        ctx.queue.write_buffer(&b, 0, &p.bytes());
        b
    }

    fn depth_views(
        ctx: &GfxContext,
        depth: &wgpu::Texture,
    ) -> Result<(wgpu::TextureView, wgpu::TextureView, bool), WordsError> {
        let f = depth.format();
        if !matches!(
            f,
            wgpu::TextureFormat::Depth32FloatStencil8 | wgpu::TextureFormat::Depth32Float
        ) || depth.sample_count() > 1
        {
            return Err(WordsError::Depth(f));
        }
        let depth_view = depth.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::DepthOnly,
            ..Default::default()
        });
        let has_stencil = f.has_stencil_aspect();
        let stencil_view = if has_stencil {
            depth.create_view(&wgpu::TextureViewDescriptor {
                aspect: wgpu::TextureAspect::StencilOnly,
                ..Default::default()
            })
        } else {
            ctx.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some("gfx guest words no stencil"),
                    size: wgpu::Extent3d {
                        width: 1,
                        height: 1,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::R8Uint,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Ok((depth_view, stencil_view, has_stencil))
    }

    /// Records the encoding of colour texture `src` (a `Bgra8Unorm` or
    /// `Rgba8Unorm` texture of `size` with `TEXTURE_BINDING`) into `mirror`
    /// as `format` words at `layout`.
    #[allow(clippy::too_many_arguments)]
    pub fn store_color(
        &self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        src: &wgpu::Texture,
        format: D3DFormat,
        size: (u32, u32),
        mirror: &wgpu::Buffer,
        layout: WordLayout,
    ) -> Result<(), WordsError> {
        let wf = WordFormat::of(format).ok_or(WordsError::Format(format))?;
        check_layout(size, layout)?;
        let have = mirror.size() / 4;
        let need = words_end(size, layout);
        if need > have {
            return Err(WordsError::Bounds { need, have });
        }
        let p = Params {
            perm: wf.packed(),
            opaque: wf.opaque as u32,
            ..Params::level(size, layout)
        };
        let uniform = Self::uniform(ctx, &p);
        let view = src.create_view(&Default::default());
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx guest words store colour"),
            layout: &self.store_color.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: mirror.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        crate::churn::side_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gfx guest words store colour"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.store_color);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        Ok(())
    }

    /// Records the encoding of depth texture `depth` (single-sample
    /// `Depth32FloatStencil8` or `Depth32Float` of `size`, with
    /// `TEXTURE_BINDING`) into `mirror` as `D24S8` words at `layout`.
    pub fn store_depth(
        &self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
        size: (u32, u32),
        mirror: &wgpu::Buffer,
        layout: WordLayout,
    ) -> Result<(), WordsError> {
        check_layout(size, layout)?;
        let have = mirror.size() / 4;
        let need = words_end(size, layout);
        if need > have {
            return Err(WordsError::Bounds { need, have });
        }
        let (depth_view, stencil_view, has_stencil) = Self::depth_views(ctx, depth)?;
        let p = Params {
            has_stencil: has_stencil as u32,
            ..Params::level(size, layout)
        };
        let uniform = Self::uniform(ctx, &p);
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx guest words store depth"),
            layout: &self.store_depth.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&stencil_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: mirror.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        crate::churn::side_pass();
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("gfx guest words store depth"),
            timestamp_writes: None,
        });
        pass.set_pipeline(&self.store_depth);
        pass.set_bind_group(0, &bind, &[]);
        pass.dispatch_workgroups(size.0.div_ceil(8), size.1.div_ceil(8), 1);
        Ok(())
    }

    /// Records the decoding of `format` words at `layout` in `mirror` into
    /// every texel of colour texture `dst` (`RENDER_ATTACHMENT`, single
    /// sample, of `size`).
    #[allow(clippy::too_many_arguments)]
    pub fn load_color(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::Texture,
        format: D3DFormat,
        size: (u32, u32),
        mirror: &wgpu::Buffer,
        layout: WordLayout,
    ) -> Result<(), WordsError> {
        let wf = WordFormat::of(format).ok_or(WordsError::Format(format))?;
        check_layout(size, layout)?;
        let have = mirror.size() / 4;
        let need = words_end(size, layout);
        if need > have {
            return Err(WordsError::Bounds { need, have });
        }
        let target = dst.format();
        let module = &self.load_module;
        let pipeline = self.load_color.entry(target).or_insert_with(|| {
            ctx.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("gfx guest words load colour"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: target,
                            blend: None,
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
        });
        let p = Params {
            perm: wf.packed(),
            opaque: wf.opaque as u32,
            ..Params::level(size, layout)
        };
        let uniform = Self::uniform(ctx, &p);
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx guest words load colour"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: mirror.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let view = dst.create_view(&Default::default());
        crate::churn::side_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gfx guest words load colour"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }

    /// Records the decoding of `D24S8` words at `layout` in `mirror` into
    /// every texel of depth texture `dst` (single-sample
    /// `Depth32FloatStencil8` or `Depth32Float` of `size`, with
    /// `RENDER_ATTACHMENT` and `COPY_DST`), as
    /// [`SurfaceStore::write_depth`](crate::surface::SurfaceStore::write_depth)
    /// does from CPU bytes: depth `z / 16777215` rounded as the CPU's `f32`
    /// division, drawn as `frag_depth`; stencil the word's low byte, packed
    /// by a compute pass and copied into the stencil aspect.
    pub fn load_depth(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        dst: &wgpu::Texture,
        size: (u32, u32),
        mirror: &wgpu::Buffer,
        layout: WordLayout,
    ) -> Result<(), WordsError> {
        check_layout(size, layout)?;
        let have = mirror.size() / 4;
        let need = words_end(size, layout);
        if need > have {
            return Err(WordsError::Bounds { need, have });
        }
        let target = dst.format();
        if !matches!(
            target,
            wgpu::TextureFormat::Depth32FloatStencil8 | wgpu::TextureFormat::Depth32Float
        ) || dst.sample_count() > 1
        {
            return Err(WordsError::Depth(target));
        }
        let (w, h) = (size.0.max(1), size.1.max(1));
        let extent = wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        };
        if target.has_stencil_aspect() {
            // Rows of stencil bytes, as a buffer-to-texture copy wants them.
            let row = w.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let packed = crate::churn::create_buffer(
                &ctx.device,
                &wgpu::BufferDescriptor {
                    label: Some("gfx guest words stencil"),
                    size: row as u64 * h as u64,
                    usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
                    mapped_at_creation: false,
                },
            );
            let p = Params {
                src_width: row / 4,
                ..Params::level(size, layout)
            };
            let uniform = Self::uniform(ctx, &p);
            let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("gfx guest words load stencil"),
                layout: &self.load_stencil.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: mirror.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: packed.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: uniform.as_entire_binding(),
                    },
                ],
            });
            {
                crate::churn::side_pass();
                let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                    label: Some("gfx guest words load stencil"),
                    timestamp_writes: None,
                });
                pass.set_pipeline(&self.load_stencil);
                pass.set_bind_group(0, &bind, &[]);
                pass.dispatch_workgroups(w.div_ceil(4).div_ceil(8), h.div_ceil(8), 1);
            }
            encoder.copy_buffer_to_texture(
                wgpu::TexelCopyBufferInfo {
                    buffer: &packed,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(row),
                        rows_per_image: Some(h),
                    },
                },
                wgpu::TexelCopyTextureInfo {
                    texture: dst,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::StencilOnly,
                },
                extent,
            );
        }
        let module = &self.load_depth_module;
        let pipeline = self.load_depth.entry(target).or_insert_with(|| {
            ctx.device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("gfx guest words load depth"),
                    layout: None,
                    vertex: wgpu::VertexState {
                        module,
                        entry_point: Some("vs_main"),
                        compilation_options: Default::default(),
                        buffers: &[],
                    },
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: Some(wgpu::DepthStencilState {
                        format: target,
                        depth_write_enabled: Some(true),
                        depth_compare: Some(wgpu::CompareFunction::Always),
                        stencil: wgpu::StencilState::default(),
                        bias: wgpu::DepthBiasState::default(),
                    }),
                    multisample: wgpu::MultisampleState::default(),
                    fragment: Some(wgpu::FragmentState {
                        module,
                        entry_point: Some("fs_main"),
                        compilation_options: Default::default(),
                        targets: &[],
                    }),
                    multiview_mask: None,
                    cache: None,
                })
        });
        let uniform = Self::uniform(ctx, &Params::level(size, layout));
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx guest words load depth"),
            layout: &pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: mirror.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let view = dst.create_view(&Default::default());
        crate::churn::side_pass();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gfx guest words load depth"),
            color_attachments: &[],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: target.has_stencil_aspect().then_some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..3, 0..1);
        Ok(())
    }

    /// Records the making of the texture a `format` texture of `size` over
    /// the depth buffer's memory decodes to: `depth` (single-sample
    /// `Depth32FloatStencil8` or `Depth32Float`) as `D24S8` words in rows
    /// of its width, the texture's texels at `layout` in those words, its
    /// logical channels XORed with `flip` (red in the low byte; `0x80` per
    /// `COLORSIGN` channel). The new `Rgba8Unorm` texture is valid once
    /// `encoder` is submitted.
    #[allow(clippy::too_many_arguments)]
    pub fn depth_alias(
        &self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        depth: &wgpu::Texture,
        format: D3DFormat,
        size: (u32, u32),
        layout: WordLayout,
        flip: u32,
    ) -> Result<wgpu::Texture, WordsError> {
        let wf = WordFormat::of(format).ok_or(WordsError::Format(format))?;
        check_layout(size, layout)?;
        let (dw, dh) = (depth.width(), depth.height());
        let have = dw as u64 * dh as u64;
        let need = words_end(size, layout);
        if need > have {
            return Err(WordsError::Bounds { need, have });
        }
        let (depth_view, stencil_view, has_stencil) = Self::depth_views(ctx, depth)?;
        let (w, h) = (size.0.max(1), size.1.max(1));
        let out = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gfx depth alias"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let out_view = out.create_view(&Default::default());
        let p = Params {
            perm: wf.packed(),
            opaque: wf.opaque as u32,
            src_width: dw,
            src_height: dh,
            has_stencil: has_stencil as u32,
            flip,
            ..Params::level((w, h), layout)
        };
        let uniform = Self::uniform(ctx, &p);
        let bind = ctx.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gfx depth alias"),
            layout: &self.depth_alias.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(&depth_view),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&stencil_view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(&out_view),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        {
            crate::churn::side_pass();
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("gfx depth alias"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.depth_alias);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(w.div_ceil(8), h.div_ceil(8), 1);
        }
        Ok(out)
    }
}

/// The `flip` of [`GuestWords::depth_alias`] for `COLORSIGN` channels
/// `signed` (R, G, B, A).
pub fn sign_flip(signed: [bool; 4]) -> u32 {
    signed
        .iter()
        .enumerate()
        .fold(0, |f, (i, &s)| if s { f | 0x80 << (8 * i) } else { f })
}

/// `depth_of_word` of the shaders on the CPU (the `D24S8` depth `z /
/// 16777215` as an `f32`, without a division), for the exhaustive check
/// against the division `decode_depth` does.
#[cfg(test)]
fn depth_of_z(z: u32) -> f32 {
    if z == 0 {
        return 0.0;
    }
    let b = 32 - z.leading_zeros();
    ((z << (24 - b)) + 1) as f32 * 2f32.powi(b as i32 - 48)
}

#[cfg(test)]
mod tests {
    #[test]
    fn depth_of_word_is_the_division_for_every_24_bit_depth() {
        for z in 0..1u32 << 24 {
            let want = z as f32 / 16_777_215.0;
            assert_eq!(super::depth_of_z(z).to_bits(), want.to_bits(), "z {z:#x}");
        }
    }
}
