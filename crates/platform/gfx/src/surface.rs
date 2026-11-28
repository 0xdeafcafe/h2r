//! Render targets and surfaces: `SetRenderTarget`, depth/stencil surfaces,
//! `CopyRects`, `Resolve`, `Swap`, and readback to guest formats.
//!
//! The HLE layer keeps the guest-visible `D3DSurface`/`D3DTexture` headers
//! and maps each one that is (or may become) a render target to a
//! [`SurfaceId`] here. The store owns the GPU textures; guest memory is only
//! touched by the explicit readback/upload functions, which convert between
//! the host formats and the guest's format and layout.
//!
//! # What "tiled" means here
//!
//! Xbox render targets and depth buffers usually live in *tiled* memory: a
//! region (`D3DTILE`: pitch, size, z-compression tags) in which the memory
//! controller rearranges addresses. We do not emulate that. A surface's
//! [`SurfaceLayout`] records whether it is swizzled, linear (pitch) or tiled
//! (pitch + [`TileRegion`]), and readbacks produce the *logical* image in the
//! guest format: tiled surfaces are written as pitched rows (what code that
//! accesses tiled memory through its untiled alias sees). Z compression tags
//! and bank interleave are not modelled; if a title needs to read tiled
//! memory raw, that is a separate (and hard) problem.
//!
//! # Semantics implemented
//!
//! * **`SetRenderTarget`**: [`SurfaceStore::target`] validates a colour and
//!   optional depth surface pair (same sample count, depth at least as large
//!   as colour; a larger depth surface goes through [`SurfaceStore::prepare_target`]) and returns a [`RenderTarget`].
//! * **`Resolve`** ([`SurfaceStore::resolve`]): MSAA colour surfaces are
//!   resolved into their single-sample image; optionally the rectangle is then
//!   copied to a destination surface (a render-target texture).
//! * **`CopyRects`** ([`SurfaceStore::copy_rect`], [`SurfaceStore::stretch`]):
//!   exact GPU copies when formats match, a filtered blit otherwise.
//! * **`Swap`** ([`Swapchain`], [`present_surface`]): a ring of buffers that
//!   flips on `Swap`, with `COPY` leaving the ring alone and `FINISH`
//!   waiting for the GPU.
//! * **Readback**: [`SurfaceStore::read_color_guest`],
//!   [`SurfaceStore::read_depth_guest`], and the host-side forms.
//!
//! The depth readback needs `DEPTH32FLOAT_STENCIL8` (Metal has it); without
//! it `D24S8` surfaces fall back to `Depth24PlusStencil8`, which wgpu cannot
//! copy out.

use std::collections::HashMap;

use crate::blit::{BlitOptions, Blitter};
use crate::context::GfxContext;
use crate::encode::{self, DepthPlane, EncodeError};
/// Channel order of host colour readbacks.
pub use crate::encode::HostPixelOrder;
use crate::format::{D3DFormat, Layout, TexelFormat};
use crate::presenter::RenderTarget;
use crate::texture::TextureError;
use crate::translate::Capabilities;

/// Handle to a surface in a [`SurfaceStore`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SurfaceId(pub u32);

/// Colour or depth/stencil.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceKind {
    /// A render target / texture.
    Color,
    /// A depth/stencil buffer.
    DepthStencil,
}

/// A tile region as the guest declares it (`D3DTILE`, minus the address).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub struct TileRegion {
    /// `D3DTILE.Flags`.
    pub flags: u32,
    /// Region size in bytes.
    pub size: u32,
    /// Region pitch in bytes.
    pub pitch: u32,
    /// Z compression tag base.
    pub z_start_tag: u32,
    /// Z offset within the region.
    pub z_offset: u32,
}

/// How a surface is laid out in guest memory.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum SurfaceLayout {
    /// Morton order (power-of-two sizes).
    Swizzled,
    /// Row-major with the given pitch in bytes.
    Linear {
        /// Bytes per row.
        pitch: u32,
    },
    /// Lives in a tiled region; readbacks use the pitch, tiling itself is a
    /// recorded property only (see the module docs).
    Tiled {
        /// Bytes per row.
        pitch: u32,
        /// The region it lives in.
        region: TileRegion,
    },
}

impl SurfaceLayout {
    /// Row pitch, if the layout has one.
    pub fn pitch(&self) -> Option<u32> {
        match self {
            SurfaceLayout::Swizzled => None,
            SurfaceLayout::Linear { pitch } | SurfaceLayout::Tiled { pitch, .. } => Some(*pitch),
        }
    }

    /// Whether the surface sits in a tiled region.
    pub fn is_tiled(&self) -> bool {
        matches!(self, SurfaceLayout::Tiled { .. })
    }
}

/// Description of a surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceDesc {
    /// Guest pixel format.
    pub format: D3DFormat,
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// MSAA sample count (1, 2 or 4).
    pub samples: u32,
    /// Guest memory layout.
    pub layout: SurfaceLayout,
}

impl SurfaceDesc {
    /// A single-sampled surface with the natural layout for `format`
    /// (swizzled formats with power-of-two size are swizzled, otherwise
    /// linear with a 64-byte-aligned pitch).
    pub fn new(format: D3DFormat, width: u32, height: u32) -> Self {
        let bpt = format.bytes_per_texel().unwrap_or(4);
        let swizzled = format.layout() == Layout::Swizzled && width.is_power_of_two() && height.is_power_of_two();
        let layout = if swizzled {
            SurfaceLayout::Swizzled
        } else {
            SurfaceLayout::Linear { pitch: (width * bpt).next_multiple_of(64) }
        };
        Self { format, width, height, samples: 1, layout }
    }

    /// Sets the sample count.
    pub fn with_samples(mut self, samples: u32) -> Self {
        self.samples = samples;
        self
    }

    /// Sets the guest layout.
    pub fn with_layout(mut self, layout: SurfaceLayout) -> Self {
        self.layout = layout;
        self
    }
}

/// Number of host samples for an Xbox `D3DMULTISAMPLE_TYPE`.
///
/// The type encodes x and y sample scales in nibbles (`0x11` = none,
/// `0x1021` = 2x1, `0x1022` = 2x2, `0x1233` = 3x3; `XbD3D8Types.h`). Their
/// product is the sample count; wgpu offers 1, 2, 4, so 3x3 clamps to 4.
/// Super-sampling variants (`0x2000` bit) are treated like multisampling.
pub fn samples_from_xbox(multisample_type: u32) -> u32 {
    let y = (multisample_type & 0xF).max(1);
    let x = ((multisample_type >> 4) & 0xF).max(1);
    match x * y {
        1 => 1,
        2 | 3 => 2,
        _ => 4,
    }
}

/// Host texture format for a colour surface format. `A8R8G8B8`/`X8R8G8B8`
/// render as BGRA8; 16-bit formats are promoted to RGBA8 (wgpu has no
/// portable 16-bit render targets).
pub fn color_surface_format(f: D3DFormat) -> Option<wgpu::TextureFormat> {
    match f.texel() {
        TexelFormat::A8R8G8B8 | TexelFormat::X8R8G8B8 => Some(wgpu::TextureFormat::Bgra8Unorm),
        TexelFormat::R5G6B5 | TexelFormat::A1R5G5B5 | TexelFormat::X1R5G5B5 | TexelFormat::A4R4G4B4 => {
            Some(wgpu::TextureFormat::Rgba8Unorm)
        }
        _ => None,
    }
}

/// Host texture format for a depth surface format.
pub fn depth_surface_format(f: D3DFormat, caps: Capabilities) -> Option<wgpu::TextureFormat> {
    use D3DFormat::*;
    match f {
        D16 | LinD16 | F16 | LinF16 => Some(wgpu::TextureFormat::Depth16Unorm),
        D24S8 | LinD24S8 | F24S8 | LinF24S8 => Some(if caps.depth32_stencil8 {
            wgpu::TextureFormat::Depth32FloatStencil8
        } else {
            wgpu::TextureFormat::Depth24PlusStencil8
        }),
        _ => None,
    }
}

/// Errors from surface operations.
#[derive(Debug, thiserror::Error)]
pub enum SurfaceError {
    /// The id is not (or no longer) in the store.
    #[error("no such surface {0:?}")]
    NoSuchSurface(SurfaceId),
    /// The guest format cannot be used for this kind of surface.
    #[error("{0:?} is not a valid {1:?} surface format")]
    BadFormat(D3DFormat, SurfaceKind),
    /// A surface of the wrong kind was passed.
    #[error("surface {0:?} is not a {1:?} surface")]
    WrongKind(SurfaceId, SurfaceKind),
    /// Zero-sized surface.
    #[error("surface size must be non-zero")]
    EmptySize,
    /// Colour and depth targets disagree.
    #[error("render target mismatch: {0}")]
    TargetMismatch(&'static str),
    /// Source and destination are the same surface.
    #[error("source and destination are the same surface {0:?}")]
    SameSurface(SurfaceId),
    /// Rectangle outside the surface.
    #[error("rectangle {0:?} lies outside the {1}x{2} surface")]
    RectOutOfBounds(CopyRect, u32, u32),
    /// Source and destination host formats differ (use `stretch`).
    #[error("host formats differ ({0:?} vs {1:?}); use a blit")]
    FormatMismatch(wgpu::TextureFormat, wgpu::TextureFormat),
    /// The surface cannot be copied out of the GPU (MSAA depth, or no
    /// `DEPTH32FLOAT_STENCIL8`).
    #[error("surface {0:?} cannot be read back: {1}")]
    NotReadable(SurfaceId, &'static str),
    /// Encoding to the guest format failed.
    #[error(transparent)]
    Encode(#[from] EncodeError),
    /// Decoding guest data failed.
    #[error(transparent)]
    Decode(#[from] TextureError),
}

/// A pixel rectangle.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CopyRect {
    /// Left.
    pub x: u32,
    /// Top.
    pub y: u32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// One surface and its GPU resources.
#[derive(Clone)]
pub struct Surface {
    desc: SurfaceDesc,
    kind: SurfaceKind,
    wgpu_format: wgpu::TextureFormat,
    /// The image: single-sample for colour surfaces (the resolve target when
    /// MSAA), multisampled for depth surfaces.
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    /// Multisampled colour attachment, if `desc.samples > 1`.
    msaa: Option<(wgpu::Texture, wgpu::TextureView)>,
    /// Depth-sized stand-in colour attachment, used while this surface is
    /// paired with a larger depth surface (see [`SurfaceStore::prepare_target`]).
    padded: Option<Padded>,
    /// One view per mip level, for render-to-texture surfaces
    /// ([`SurfaceStore::create_texture_target`]); empty otherwise.
    levels: Vec<wgpu::TextureView>,
}

#[derive(Clone)]
struct Padded {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: (u32, u32),
}

impl Surface {
    /// The (effective) description.
    pub fn desc(&self) -> &SurfaceDesc {
        &self.desc
    }
    /// Colour or depth.
    pub fn kind(&self) -> SurfaceKind {
        self.kind
    }
    /// Host texture format.
    pub fn wgpu_format(&self) -> wgpu::TextureFormat {
        self.wgpu_format
    }
    /// The single-sample image: bind this when the surface is used as a
    /// texture (after [`SurfaceStore::resolve`] for MSAA).
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }
    /// A view of [`texture`](Self::texture) (every mip level: bind this to
    /// sample a render-to-texture surface).
    pub fn view(&self) -> &wgpu::TextureView {
        &self.view
    }
    /// Base mip of the actual image, usable as a single-level attachment.
    pub fn base_view(&self) -> &wgpu::TextureView {
        self.levels.first().unwrap_or(&self.view)
    }
    /// Mip levels (1 unless made by [`SurfaceStore::create_texture_target`]).
    pub fn mip_levels(&self) -> u32 {
        self.texture.mip_level_count()
    }
}

/// A colour image copied on the GPU ([`SurfaceStore::keep_color`]).
pub struct KeptImage {
    texture: wgpu::Texture,
    size: (u32, u32),
    format: wgpu::TextureFormat,
}

impl KeptImage {
    /// The image as host pixels (4 bytes per pixel, tight rows), their
    /// byte order, and its size: a blocking read back.
    pub fn read(&self, ctx: &GfxContext) -> (Vec<u8>, HostPixelOrder, (u32, u32)) {
        let order = match self.format {
            wgpu::TextureFormat::Bgra8Unorm => HostPixelOrder::Bgra8,
            _ => HostPixelOrder::Rgba8,
        };
        let px = crate::readback::read_texture(ctx, &self.texture, wgpu::TextureAspect::All, 4, self.size);
        (px, order, self.size)
    }
}

/// Owns all render-target and depth surfaces.
pub struct SurfaceStore {
    surfaces: HashMap<SurfaceId, Surface>,
    next: u32,
    caps: Capabilities,
    /// Made at the first [`SurfaceStore::write_depth`].
    depth_fill: Option<crate::depth_fill::DepthFill>,
}

fn supported_samples(ctx: &GfxContext, format: wgpu::TextureFormat, wanted: u32) -> u32 {
    let flags = ctx.adapter.get_texture_format_features(format).flags;
    [4, 2]
        .into_iter()
        .filter(|&n| n <= wanted)
        .find(|&n| flags.sample_count_supported(n))
        .unwrap_or(1)
}

impl SurfaceStore {
    /// An empty store.
    pub fn new(caps: Capabilities) -> Self {
        Self { surfaces: HashMap::new(), next: 1, caps, depth_fill: None }
    }

    /// Creates a surface. Requested MSAA is lowered to what the device
    /// supports for the format; check [`Surface::desc`] for the result.
    /// Depth surfaces are multisampled directly; colour surfaces get a
    /// multisampled attachment plus a single-sample resolved image.
    pub fn create(&mut self, ctx: &GfxContext, mut desc: SurfaceDesc, kind: SurfaceKind) -> Result<SurfaceId, SurfaceError> {
        if desc.width == 0 || desc.height == 0 {
            return Err(SurfaceError::EmptySize);
        }
        let wgpu_format = match kind {
            SurfaceKind::Color => color_surface_format(desc.format),
            SurfaceKind::DepthStencil => depth_surface_format(desc.format, self.caps),
        }
        .ok_or(SurfaceError::BadFormat(desc.format, kind))?;
        desc.samples = supported_samples(ctx, wgpu_format, desc.samples.max(1));
        let size = wgpu::Extent3d { width: desc.width, height: desc.height, depth_or_array_layers: 1 };
        let make = |label, samples, usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size,
                mip_level_count: 1,
                sample_count: samples,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu_format,
                usage,
                view_formats: &[],
            })
        };
        let (texture, msaa) = match kind {
            SurfaceKind::Color => {
                let single = make(
                    "surface color",
                    1,
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST,
                );
                let msaa = (desc.samples > 1).then(|| {
                    let t = make("surface color msaa", desc.samples, wgpu::TextureUsages::RENDER_ATTACHMENT);
                    let v = t.create_view(&Default::default());
                    (t, v)
                });
                (single, msaa)
            }
            SurfaceKind::DepthStencil => (
                make(
                    "surface depth",
                    desc.samples,
                    wgpu::TextureUsages::RENDER_ATTACHMENT
                        | wgpu::TextureUsages::TEXTURE_BINDING
                        | wgpu::TextureUsages::COPY_SRC
                        | wgpu::TextureUsages::COPY_DST,
                ),
                None,
            ),
        };
        let view = texture.create_view(&Default::default());
        let id = SurfaceId(self.next);
        self.next += 1;
        self.surfaces.insert(id, Surface { desc, kind, wgpu_format, texture, view, msaa, padded: None, levels: Vec::new() });
        Ok(id)
    }

    /// Creates a single-sample colour surface with `levels` mip levels that
    /// is both a render target and a texture (render-to-texture: a guest
    /// surface whose `Parent` is a texture). Draw into one level with
    /// [`level_target`](Self::level_target) (level 0 also through
    /// [`target`](Self::target)) and sample all of them through
    /// [`Surface::view`]. MSAA is not supported (`desc.samples` is ignored).
    pub fn create_texture_target(&mut self, ctx: &GfxContext, mut desc: SurfaceDesc, levels: u32) -> Result<SurfaceId, SurfaceError> {
        if desc.width == 0 || desc.height == 0 {
            return Err(SurfaceError::EmptySize);
        }
        let wgpu_format = color_surface_format(desc.format).ok_or(SurfaceError::BadFormat(desc.format, SurfaceKind::Color))?;
        desc.samples = 1;
        let levels = levels.clamp(1, crate::texture::max_levels(desc.width, desc.height, 1));
        let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface render texture"),
            size: wgpu::Extent3d { width: desc.width, height: desc.height, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let view = texture.create_view(&Default::default());
        let level_views = (0..levels)
            .map(|l| texture.create_view(&wgpu::TextureViewDescriptor { base_mip_level: l, mip_level_count: Some(1), ..Default::default() }))
            .collect();
        let id = SurfaceId(self.next);
        self.next += 1;
        self.surfaces.insert(
            id,
            Surface { desc, kind: SurfaceKind::Color, wgpu_format, texture, view, msaa: None, padded: None, levels: level_views },
        );
        Ok(id)
    }

    /// `SetRenderTarget` on mip `level` of a surface made by
    /// [`create_texture_target`](Self::create_texture_target). A depth
    /// surface must have exactly the level's size and one sample.
    pub fn level_target(&self, color: SurfaceId, level: u32, depth: Option<SurfaceId>) -> Result<RenderTarget<'_>, SurfaceError> {
        let c = self.typed(color, SurfaceKind::Color)?;
        let view = c.levels.get(level as usize).ok_or(SurfaceError::TargetMismatch("no such mip level of a render-to-texture surface"))?;
        let size = ((c.desc.width >> level).max(1), (c.desc.height >> level).max(1));
        let (depth_view, depth_format) = match depth {
            Some(d) => {
                let d = self.typed(d, SurfaceKind::DepthStencil)?;
                if (d.desc.width, d.desc.height) != size || d.desc.samples != 1 {
                    return Err(SurfaceError::TargetMismatch("depth surface does not match the mip level"));
                }
                (Some(&d.view), Some(d.wgpu_format))
            }
            None => (None, None),
        };
        Ok(RenderTarget { color: view, color_format: c.wgpu_format, depth: depth_view, depth_format, size, samples: 1 })
    }

    /// Import handles from a store on the same GPU device. Pixels and views
    /// stay shared. Each store owns its own padded-attachment bookkeeping.
    pub fn import(&mut self, source: &Surface) -> SurfaceId {
        let id = SurfaceId(self.next);
        self.next += 1;
        self.surfaces.insert(id, Surface { padded: None, ..source.clone() });
        id
    }

    /// Looks up a surface.
    pub fn get(&self, id: SurfaceId) -> Result<&Surface, SurfaceError> {
        self.surfaces.get(&id).ok_or(SurfaceError::NoSuchSurface(id))
    }

    /// Drops a surface (the GPU memory is freed once in-flight work ends).
    pub fn remove(&mut self, id: SurfaceId) -> bool {
        self.surfaces.remove(&id).is_some()
    }

    /// Number of live surfaces.
    pub fn len(&self) -> usize {
        self.surfaces.len()
    }

    /// Whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.surfaces.is_empty()
    }

    fn typed(&self, id: SurfaceId, kind: SurfaceKind) -> Result<&Surface, SurfaceError> {
        let s = self.get(id)?;
        if s.kind != kind {
            return Err(SurfaceError::WrongKind(id, kind));
        }
        Ok(s)
    }

    /// `SetRenderTarget`: validates the pair and returns the target to draw
    /// into. MSAA colour surfaces draw into their multisampled attachment.
    pub fn target(&self, color: SurfaceId, depth: Option<SurfaceId>) -> Result<RenderTarget<'_>, SurfaceError> {
        let c = self.typed(color, SurfaceKind::Color)?;
        let (depth_view, depth_format) = match depth {
            Some(d) => {
                let d = self.typed(d, SurfaceKind::DepthStencil)?;
                if d.desc.samples != c.desc.samples {
                    return Err(SurfaceError::TargetMismatch("colour and depth sample counts differ"));
                }
                if d.desc.width < c.desc.width || d.desc.height < c.desc.height {
                    return Err(SurfaceError::TargetMismatch("depth surface is smaller than the render target"));
                }
                if d.desc.width != c.desc.width || d.desc.height != c.desc.height {
                    // wgpu requires equal attachment sizes: a larger depth
                    // surface needs the padded colour stand-in.
                    let ok = c.padded.as_ref().is_some_and(|p| p.size == (d.desc.width, d.desc.height));
                    if !ok {
                        return Err(SurfaceError::TargetMismatch(
                            "depth surface is larger than the render target: call prepare_target first",
                        ));
                    }
                }
                (Some(&d.view), Some(d.wgpu_format))
            }
            None => (None, None),
        };
        let larger = depth.and_then(|d| self.surfaces.get(&d)).is_some_and(|d| d.desc.width != c.desc.width || d.desc.height != c.desc.height);
        let view = match (&c.padded, larger) {
            (Some(p), true) => &p.view,
            _ => c.msaa.as_ref().map_or(c.base_view(), |(_, v)| v),
        };
        Ok(RenderTarget {
            color: view,
            color_format: c.wgpu_format,
            depth: depth_view,
            depth_format,
            size: (c.desc.width, c.desc.height),
            samples: c.desc.samples,
        })
    }

    /// Prepares a colour/depth pair whose depth surface is larger than the
    /// colour surface (common on the Xbox). wgpu needs equal attachment
    /// sizes, so the colour surface gets a depth-sized stand-in, seeded with
    /// the current colour contents in its top-left corner. Draws are clipped
    /// to the colour size by the renderer. Call [`finish_target`](Self::finish_target)
    /// after the pass to copy the result back. A no-op when sizes match.
    /// Only single-sampled colour surfaces are supported.
    pub fn prepare_target(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        color: SurfaceId,
        depth: SurfaceId,
    ) -> Result<(), SurfaceError> {
        let d = self.typed(depth, SurfaceKind::DepthStencil)?;
        let size = (d.desc.width, d.desc.height);
        let c = self.typed(color, SurfaceKind::Color)?;
        if size == (c.desc.width, c.desc.height) {
            return Ok(());
        }
        if size.0 < c.desc.width || size.1 < c.desc.height {
            return Err(SurfaceError::TargetMismatch("depth surface is smaller than the render target"));
        }
        if c.desc.samples > 1 {
            return Err(SurfaceError::TargetMismatch("a depth surface larger than an MSAA colour surface is not supported"));
        }
        let (cw, ch, format) = (c.desc.width, c.desc.height, c.wgpu_format);
        let c = self.surfaces.get_mut(&color).expect("checked above");
        if c.padded.as_ref().is_none_or(|p| p.size != size) {
            let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("surface color padded"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            let view = texture.create_view(&Default::default());
            c.padded = Some(Padded { texture, view, size });
        }
        let p = c.padded.as_ref().expect("just created");
        let info = |t| wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
        encoder.copy_texture_to_texture(
            info(&c.texture),
            info(&p.texture),
            wgpu::Extent3d { width: cw, height: ch, depth_or_array_layers: 1 },
        );
        Ok(())
    }

    /// Copies the result of a pass drawn through the padded stand-in back
    /// into the colour surface. A no-op if [`prepare_target`](Self::prepare_target)
    /// did not need one.
    pub fn finish_target(&self, encoder: &mut wgpu::CommandEncoder, color: SurfaceId) -> Result<(), SurfaceError> {
        let c = self.typed(color, SurfaceKind::Color)?;
        if let Some(p) = &c.padded {
            let info = |t| wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
            encoder.copy_texture_to_texture(
                info(&p.texture),
                info(&c.texture),
                wgpu::Extent3d { width: c.desc.width, height: c.desc.height, depth_or_array_layers: 1 },
            );
        }
        Ok(())
    }

    /// Resolves an MSAA colour surface into its single-sample image. A no-op
    /// for single-sampled surfaces.
    pub fn resolve_msaa(&self, encoder: &mut wgpu::CommandEncoder, id: SurfaceId) -> Result<(), SurfaceError> {
        let s = self.typed(id, SurfaceKind::Color)?;
        if let Some((_, msaa_view)) = &s.msaa {
            // An empty pass that loads and resolves the multisampled image.
            crate::churn::pass();
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("msaa resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: msaa_view,
                    depth_slice: None,
                    resolve_target: Some(&s.view),
                    ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        Ok(())
    }

    /// `CopyRects`, exact path: copies `rect` of `src` to `(dst_x, dst_y)` of
    /// `dst` with a texture copy. Both must be colour surfaces of the same
    /// host format. MSAA sources must be resolved first.
    pub fn copy_rect(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        src: SurfaceId,
        dst: SurfaceId,
        rect: CopyRect,
        dst_x: u32,
        dst_y: u32,
    ) -> Result<(), SurfaceError> {
        if src == dst {
            return Err(SurfaceError::SameSurface(src));
        }
        let s = self.typed(src, SurfaceKind::Color)?;
        let d = self.typed(dst, SurfaceKind::Color)?;
        if s.wgpu_format != d.wgpu_format {
            return Err(SurfaceError::FormatMismatch(s.wgpu_format, d.wgpu_format));
        }
        let fits = |sd: &SurfaceDesc, x: u32, y: u32| {
            x.checked_add(rect.width).is_some_and(|r| r <= sd.width) && y.checked_add(rect.height).is_some_and(|b| b <= sd.height)
        };
        if !fits(&s.desc, rect.x, rect.y) {
            return Err(SurfaceError::RectOutOfBounds(rect, s.desc.width, s.desc.height));
        }
        if !fits(&d.desc, dst_x, dst_y) {
            return Err(SurfaceError::RectOutOfBounds(
                CopyRect { x: dst_x, y: dst_y, ..rect },
                d.desc.width,
                d.desc.height,
            ));
        }
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &s.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: rect.x, y: rect.y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyTextureInfo {
                texture: &d.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x: dst_x, y: dst_y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::Extent3d { width: rect.width, height: rect.height, depth_or_array_layers: 1 },
        );
        Ok(())
    }

    /// `Resolve`: resolves `src` if multisampled, then, when `dst` is given,
    /// copies `rect` to `(dst_x, dst_y)` of it. Clearing after the resolve
    /// (the `D3DRESOLVE_CLEAR*` flags) is the caller's job via
    /// [`crate::renderer::Clear`].
    pub fn resolve(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        src: SurfaceId,
        dst: Option<(SurfaceId, u32, u32)>,
        rect: Option<CopyRect>,
    ) -> Result<(), SurfaceError> {
        self.resolve_msaa(encoder, src)?;
        if let Some((dst, x, y)) = dst {
            let s = self.get(src)?;
            let rect = rect.unwrap_or(CopyRect { x: 0, y: 0, width: s.desc.width, height: s.desc.height });
            self.copy_rect(encoder, src, dst, rect, x, y)?;
        }
        Ok(())
    }

    /// A sampleable copy of colour surface `id` as it is now: a new
    /// single-sample texture of the surface's size and host format
    /// (`TEXTURE_BINDING | COPY_DST | COPY_SRC | RENDER_ATTACHMENT`, so it
    /// can be drawn over, e.g. to force alpha), filled by a copy recorded in
    /// `encoder` (MSAA surfaces are resolved first). For sampling a surface
    /// that is not a render-to-texture target, such as the back buffer read
    /// through a texture header that aliases its memory, also while it is
    /// the current target (a pass cannot sample its own attachment).
    pub fn snapshot(
        &self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        id: SurfaceId,
    ) -> Result<wgpu::Texture, SurfaceError> {
        self.resolve_msaa(encoder, id)?;
        let s = self.typed(id, SurfaceKind::Color)?;
        let size = wgpu::Extent3d { width: s.desc.width, height: s.desc.height, depth_or_array_layers: 1 };
        let copy = ctx.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("surface snapshot"),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: s.wgpu_format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::RENDER_ATTACHMENT,
            view_formats: &[],
        });
        let info = |t| wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
        encoder.copy_texture_to_texture(info(&s.texture), info(&copy), size);
        Ok(copy)
    }

    /// Refills `dst`, a texture an earlier [`snapshot`](Self::snapshot) of
    /// a surface of this size and format made, with colour surface `id` as
    /// it is now: a copy recorded in `encoder` (MSAA surfaces are resolved
    /// first), nothing allocated. `Ok(false)`, with nothing recorded:
    /// `dst` is not of the surface's size and host format, or cannot be
    /// copied into.
    pub fn snapshot_into(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        id: SurfaceId,
        dst: &wgpu::Texture,
    ) -> Result<bool, SurfaceError> {
        let s = self.typed(id, SurfaceKind::Color)?;
        let size = wgpu::Extent3d { width: s.desc.width, height: s.desc.height, depth_or_array_layers: 1 };
        if dst.size() != size
            || dst.format() != s.wgpu_format
            || dst.sample_count() != 1
            || dst.mip_level_count() != 1
            || !dst.usage().contains(wgpu::TextureUsages::COPY_DST)
        {
            return Ok(false);
        }
        self.resolve_msaa(encoder, id)?;
        let s = self.typed(id, SurfaceKind::Color)?;
        let info = |t| wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
        encoder.copy_texture_to_texture(info(&s.texture), info(dst), size);
        Ok(true)
    }

    /// `CopyRects` when sizes or formats differ: a filtered blit of `src`
    /// (optionally a sub-rectangle) over the whole of `dst`.
    pub fn stretch(
        &self,
        ctx: &GfxContext,
        blitter: &mut Blitter,
        encoder: &mut wgpu::CommandEncoder,
        src: SurfaceId,
        dst: SurfaceId,
        rect: Option<CopyRect>,
        linear: bool,
    ) -> Result<(), SurfaceError> {
        if src == dst {
            return Err(SurfaceError::SameSurface(src));
        }
        let s = self.typed(src, SurfaceKind::Color)?;
        let d = self.typed(dst, SurfaceKind::Color)?;
        let (w, h) = (s.desc.width as f32, s.desc.height as f32);
        let r = rect.unwrap_or(CopyRect { x: 0, y: 0, width: s.desc.width, height: s.desc.height });
        let fits = r.x.checked_add(r.width).is_some_and(|e| e <= s.desc.width)
            && r.y.checked_add(r.height).is_some_and(|e| e <= s.desc.height);
        if r.width == 0 || r.height == 0 || !fits {
            return Err(SurfaceError::RectOutOfBounds(r, s.desc.width, s.desc.height));
        }
        let opts = BlitOptions {
            src_rect: [r.x as f32 / w, r.y as f32 / h, r.width as f32 / w, r.height as f32 / h],
            linear,
            gamma: None,
            force_opaque: matches!(
                s.desc.format.texel(),
                TexelFormat::X8R8G8B8 | TexelFormat::X1R5G5B5 | TexelFormat::R5G6B5
            ),
        };
        blitter.blit(ctx, encoder, &s.view, &d.view, d.wgpu_format, opts);
        Ok(())
    }

    /// Reads a colour surface's single-sample image as host pixels (4 bytes
    /// per pixel, tight rows) plus their byte order.
    pub fn read_color(&self, ctx: &GfxContext, id: SurfaceId) -> Result<(Vec<u8>, HostPixelOrder), SurfaceError> {
        let s = self.typed(id, SurfaceKind::Color)?;
        let order = match s.wgpu_format {
            wgpu::TextureFormat::Bgra8Unorm => HostPixelOrder::Bgra8,
            _ => HostPixelOrder::Rgba8,
        };
        let px = crate::readback::read_texture(ctx, &s.texture, wgpu::TextureAspect::All, 4, (s.desc.width, s.desc.height));
        Ok((px, order))
    }

    /// Copies a colour surface's single-sample image into `into` on the GPU
    /// (no read back, no wait): kept to be read with [`KeptImage::read`]
    /// later, if at all. `into` is reused while the size and format match.
    pub fn keep_color(&self, ctx: &GfxContext, id: SurfaceId, into: &mut Option<KeptImage>) -> Result<(), SurfaceError> {
        let mut enc = ctx.device.create_command_encoder(&Default::default());
        self.resolve_msaa(&mut enc, id)?;
        let s = self.typed(id, SurfaceKind::Color)?;
        let size = (s.desc.width, s.desc.height);
        if !into.as_ref().is_some_and(|k| k.size == size && k.format == s.wgpu_format) {
            let texture = ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("kept image"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: s.wgpu_format,
                usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            });
            *into = Some(KeptImage { texture, size, format: s.wgpu_format });
        }
        let k = into.as_ref().expect("made above");
        let info = |t| wgpu::TexelCopyTextureInfo { texture: t, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All };
        enc.copy_texture_to_texture(
            info(&s.texture),
            info(&k.texture),
            wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
        );
        ctx.queue.submit([enc.finish()]);
        Ok(())
    }

    /// Reads a colour surface and encodes it in the guest format and layout.
    pub fn read_color_guest(&self, ctx: &GfxContext, id: SurfaceId) -> Result<Vec<u8>, SurfaceError> {
        let (px, order) = self.read_color(ctx, id)?;
        let d = self.get(id)?.desc;
        Ok(encode::encode_color(d.format, order, &px, d.width, d.height, &d.layout)?)
    }

    /// Reads a depth surface's depth (and stencil, if it has one).
    pub fn read_depth(&self, ctx: &GfxContext, id: SurfaceId) -> Result<DepthPlane, SurfaceError> {
        let s = self.typed(id, SurfaceKind::DepthStencil)?;
        if s.desc.samples > 1 {
            return Err(SurfaceError::NotReadable(id, "multisampled depth cannot be copied out"));
        }
        let size = (s.desc.width, s.desc.height);
        let read = |aspect, bpp| crate::readback::read_texture(ctx, &s.texture, aspect, bpp, size);
        match s.wgpu_format {
            wgpu::TextureFormat::Depth16Unorm => {
                let raw = read(wgpu::TextureAspect::DepthOnly, 2);
                let depth = raw.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c) as f32 / 65535.0).collect();
                Ok(DepthPlane { depth, stencil: None })
            }
            wgpu::TextureFormat::Depth32FloatStencil8 => {
                let raw = read(wgpu::TextureAspect::DepthOnly, 4);
                let depth = raw.as_chunks::<4>().0.iter().map(|c| f32::from_le_bytes(*c)).collect();
                let stencil = Some(read(wgpu::TextureAspect::StencilOnly, 1));
                Ok(DepthPlane { depth, stencil })
            }
            _ => Err(SurfaceError::NotReadable(id, "needs DEPTH32FLOAT_STENCIL8 (Depth24Plus cannot be copied)")),
        }
    }

    /// Reads a depth surface and encodes it in the guest format and layout.
    pub fn read_depth_guest(&self, ctx: &GfxContext, id: SurfaceId) -> Result<Vec<u8>, SurfaceError> {
        let plane = self.read_depth(ctx, id)?;
        let d = self.get(id)?.desc;
        Ok(encode::encode_depth(d.format, &plane, d.width, d.height, &d.layout)?)
    }

    /// Reads a depth surface and encodes it as `format` texels laid out as
    /// `layout` (the guest memory of a depth level, or of the depth buffer a
    /// texture aliases), whatever the surface's own description says.
    pub fn read_depth_as(&self, ctx: &GfxContext, id: SurfaceId, format: D3DFormat, layout: &SurfaceLayout) -> Result<Vec<u8>, SurfaceError> {
        let plane = self.read_depth(ctx, id)?;
        let d = self.get(id)?.desc;
        Ok(encode::encode_depth(format, &plane, d.width, d.height, layout)?)
    }

    /// Replaces a depth surface's depth (and stencil) with guest texels
    /// described by `desc`: `D16` or `D24S8`, one 2D level of the surface's
    /// size, swizzled, or rows `desc.pitch` bytes apart when linear (a
    /// render-to-texture depth level filled from its texture's memory).
    /// Depth is drawn (see [`crate::depth_fill`]); stencil is copied. The
    /// surface must be single-sample.
    pub fn write_depth(&mut self, ctx: &GfxContext, id: SurfaceId, desc: &crate::texture::TextureDesc, guest: &[u8]) -> Result<(), SurfaceError> {
        let s = self.typed(id, SurfaceKind::DepthStencil)?;
        let d = s.desc;
        if desc.kind != crate::texture::TextureKind::D2 || desc.depth != 1 || (desc.width, desc.height) != (d.width, d.height) {
            return Err(SurfaceError::TargetMismatch("texel description is not one 2D level of the surface's size"));
        }
        if d.samples != 1 {
            return Err(SurfaceError::TargetMismatch("a multisampled depth surface cannot be filled"));
        }
        let layout = match desc.format.layout() {
            Layout::Swizzled => SurfaceLayout::Swizzled,
            _ => SurfaceLayout::Linear { pitch: desc.pitch },
        };
        let plane = encode::decode_depth(desc.format, guest, d.width, d.height, &layout)?;
        let (format, texture) = (s.wgpu_format, s.texture.clone());
        let fill = self.depth_fill.get_or_insert_with(|| crate::depth_fill::DepthFill::new(&ctx.device));
        fill.fill(ctx, &texture, format, (d.width, d.height), &plane.depth, plane.stencil.as_deref());
        Ok(())
    }

    /// Replaces a colour surface's contents from guest memory in its own
    /// format and layout (e.g. after the guest wrote to a locked surface).
    pub fn upload_color_from_guest(&self, ctx: &GfxContext, id: SurfaceId, guest: &[u8]) -> Result<(), SurfaceError> {
        let d = self.typed(id, SurfaceKind::Color)?.desc;
        // Decode by the surface's own layout (not the format's default one),
        // mirroring `encode::encode_color`.
        self.write_color_texels(ctx, id, d.format, d.layout.pitch(), guest)
    }

    /// Replaces a colour surface's contents with guest texels described by
    /// `desc`, which gives the guest's format, size and pitch (a
    /// render-to-texture level filled from its texture's memory, whose
    /// pitch need not be the surface's). `desc` must be one 2D level of the
    /// surface's size; swizzled formats are unswizzled, linear ones read
    /// with `desc.pitch`. Compressed formats cannot be render targets.
    pub fn write_color(&self, ctx: &GfxContext, id: SurfaceId, desc: &crate::texture::TextureDesc, guest: &[u8]) -> Result<(), SurfaceError> {
        let d = self.typed(id, SurfaceKind::Color)?.desc;
        if desc.kind != crate::texture::TextureKind::D2 || desc.depth != 1 || (desc.width, desc.height) != (d.width, d.height) {
            return Err(SurfaceError::TargetMismatch("texel description is not one 2D level of the surface's size"));
        }
        let pitch = match desc.format.layout() {
            Layout::Swizzled => None,
            Layout::Linear => Some(desc.pitch),
            Layout::Compressed => return Err(SurfaceError::BadFormat(desc.format, SurfaceKind::Color)),
        };
        self.write_color_texels(ctx, id, desc.format, pitch, guest)
    }

    /// Decodes `guest` (`format` texels, swizzled when `pitch` is `None`,
    /// else rows of `pitch` bytes) into level 0 of colour surface `id`.
    fn write_color_texels(&self, ctx: &GfxContext, id: SurfaceId, format: D3DFormat, pitch: Option<u32>, guest: &[u8]) -> Result<(), SurfaceError> {
        let s = self.typed(id, SurfaceKind::Color)?;
        let d = &s.desc;
        let bpt = format.bytes_per_texel().ok_or(TextureError::InvalidHeader("surface format has no texel size"))? as usize;
        let (w, h) = (d.width as usize, d.height as usize);
        let row = w * bpt;
        let tight: Vec<u8> = match pitch {
            None => {
                if guest.len() < row * h {
                    return Err(TextureError::DataTooShort { need: row * h, have: guest.len() }.into());
                }
                crate::swizzle::unswizzle(guest, d.width, d.height, 1, bpt).map_err(TextureError::from)?
            }
            Some(pitch) => {
                let pitch = (pitch as usize).max(row);
                let need = pitch * (h - 1) + row;
                if guest.len() < need {
                    return Err(TextureError::DataTooShort { need, have: guest.len() }.into());
                }
                (0..h).flat_map(|y| guest[y * pitch..y * pitch + row].iter().copied()).collect()
            }
        };
        let texel = format.texel();
        let data = if s.wgpu_format == wgpu::TextureFormat::Bgra8Unorm {
            crate::convert::to_bgra8(texel, &tight, w * h)
        } else {
            crate::convert::to_rgba8(texel, &tight, w * h, None)
        }
        .map_err(TextureError::from)?;
        crate::churn::queue_write();
        ctx.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &s.texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &data,
            wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(d.width * 4), rows_per_image: Some(d.height) },
            wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
        );
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Swap
// ---------------------------------------------------------------------------

/// `D3DSWAP_*` flags of `Swap` (values from Cxbx's `X_D3DSWAP`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct SwapFlags(pub u32);

impl SwapFlags {
    /// `D3DSWAP_DEFAULT`: flip.
    pub const DEFAULT: Self = Self(0);
    /// `D3DSWAP_COPY`: copy the back buffer to the front instead of flipping.
    pub const COPY: Self = Self(1);
    /// `D3DSWAP_BYPASSCOPY`: skip the copy-based path; treated as a flip.
    pub const BYPASSCOPY: Self = Self(2);
    /// `D3DSWAP_FINISH`: wait for the GPU to finish before returning.
    pub const FINISH: Self = Self(4);

    /// Whether `COPY` is set.
    pub fn is_copy(self) -> bool {
        self.0 & Self::COPY.0 != 0
    }
    /// Whether `FINISH` is set.
    pub fn is_finish(self) -> bool {
        self.0 & Self::FINISH.0 != 0
    }
}

/// The ring of back buffers and the front buffer.
///
/// `ring = [back0, back1, ..., front]`. `back0` is the buffer the game is
/// drawing to (`GetBackBuffer(0)`); `Swap` shows it and, when flipping,
/// rotates the ring so the displayed buffer becomes the front and the old
/// front becomes the last back buffer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Swapchain {
    ring: Vec<SurfaceId>,
}

impl Swapchain {
    /// `back_buffers` (at least one) plus the front buffer.
    pub fn new(back_buffers: &[SurfaceId], front: SurfaceId) -> Self {
        assert!(!back_buffers.is_empty(), "a swapchain needs at least one back buffer");
        let mut ring = back_buffers.to_vec();
        ring.push(front);
        Self { ring }
    }

    /// `GetBackBuffer(index)`: `-1` is the front buffer, `0..` the back
    /// buffers in draw order.
    pub fn back_buffer(&self, index: i32) -> Option<SurfaceId> {
        match index {
            -1 => self.ring.last().copied(),
            i if i >= 0 && (i as usize) < self.ring.len() - 1 => Some(self.ring[i as usize]),
            _ => None,
        }
    }

    /// `Swap(flags)`: returns the surface to display and updates the ring.
    pub fn swap(&mut self, flags: SwapFlags) -> SurfaceId {
        let shown = self.ring[0];
        if !flags.is_copy() {
            self.ring.rotate_left(1);
        }
        shown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn multisample_types() {
        assert_eq!(samples_from_xbox(0x0011), 1); // NONE
        assert_eq!(samples_from_xbox(0x1021), 2);
        assert_eq!(samples_from_xbox(0x1121), 2); // quincunx
        assert_eq!(samples_from_xbox(0x2012), 2); // vertical supersample
        assert_eq!(samples_from_xbox(0x1022), 4);
        assert_eq!(samples_from_xbox(0x1222), 4);
        assert_eq!(samples_from_xbox(0x1233), 4); // 9 clamps to 4
    }

    #[test]
    fn natural_layouts() {
        let d = SurfaceDesc::new(D3DFormat::A8R8G8B8, 64, 32);
        assert_eq!(d.layout, SurfaceLayout::Swizzled);
        let d = SurfaceDesc::new(D3DFormat::LinA8R8G8B8, 640, 480);
        assert_eq!(d.layout, SurfaceLayout::Linear { pitch: 2560 });
        // Non power-of-two swizzled formats fall back to linear.
        let d = SurfaceDesc::new(D3DFormat::A8R8G8B8, 100, 50);
        assert_eq!(d.layout.pitch(), Some(448));
        assert!(!d.layout.is_tiled());
        let t = d.with_layout(SurfaceLayout::Tiled { pitch: 512, region: TileRegion::default() });
        assert!(t.layout.is_tiled());
        assert_eq!(t.layout.pitch(), Some(512));
    }

    #[test]
    fn host_formats() {
        assert_eq!(color_surface_format(D3DFormat::LinX8R8G8B8), Some(wgpu::TextureFormat::Bgra8Unorm));
        assert_eq!(color_surface_format(D3DFormat::R5G6B5), Some(wgpu::TextureFormat::Rgba8Unorm));
        assert_eq!(color_surface_format(D3DFormat::Dxt1), None);
        assert_eq!(depth_surface_format(D3DFormat::D16, Capabilities::NONE), Some(wgpu::TextureFormat::Depth16Unorm));
        assert_eq!(
            depth_surface_format(D3DFormat::D24S8, Capabilities::ALL),
            Some(wgpu::TextureFormat::Depth32FloatStencil8)
        );
        assert_eq!(
            depth_surface_format(D3DFormat::LinD24S8, Capabilities::NONE),
            Some(wgpu::TextureFormat::Depth24PlusStencil8)
        );
        assert_eq!(depth_surface_format(D3DFormat::A8R8G8B8, Capabilities::ALL), None);
    }

    #[test]
    fn swapchain_flip_and_copy() {
        let (a, b, front) = (SurfaceId(1), SurfaceId(2), SurfaceId(3));
        // Two back buffers + front.
        let mut sc = Swapchain::new(&[a, b], front);
        assert_eq!(sc.back_buffer(0), Some(a));
        assert_eq!(sc.back_buffer(1), Some(b));
        assert_eq!(sc.back_buffer(2), None);
        assert_eq!(sc.back_buffer(-1), Some(front));
        assert_eq!(sc.swap(SwapFlags::DEFAULT), a);
        // a is now the front; b is drawn next; the old front is last in line.
        assert_eq!(sc.back_buffer(-1), Some(a));
        assert_eq!(sc.back_buffer(0), Some(b));
        assert_eq!(sc.back_buffer(1), Some(front));
        assert_eq!(sc.swap(SwapFlags::DEFAULT), b);
        assert_eq!(sc.swap(SwapFlags::DEFAULT), front);
        assert_eq!(sc.swap(SwapFlags::DEFAULT), a);
        // COPY shows the back buffer without rotating.
        let mut one = Swapchain::new(&[a], front);
        assert_eq!(one.swap(SwapFlags::COPY), a);
        assert_eq!(one.swap(SwapFlags::COPY), a);
        assert_eq!(one.back_buffer(-1), Some(front));
        // Single back buffer flip alternates.
        assert_eq!(one.swap(SwapFlags::DEFAULT), a);
        assert_eq!(one.swap(SwapFlags::DEFAULT), front);
    }

    #[test]
    fn swap_flags() {
        assert!(SwapFlags(5).is_copy() && SwapFlags(5).is_finish());
        assert!(!SwapFlags::DEFAULT.is_copy());
    }
}
