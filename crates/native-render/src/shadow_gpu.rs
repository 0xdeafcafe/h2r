//! A private map made from an ordered stream of resolved caster draws.
//! The packet contains the translated material, live uniforms, geometry,
//! alpha kill, clip and bias. Record it on the same encoder before its
//! upload ring can recycle the slots. Counted draws have guest-visible
//! query side effects and cannot be replayed.

use gfx::{Clear, GfxContext, RecordedDraw, RenderTarget, Renderer};
use shaders::psh::{
    AlphaFunc,
    shadow::{DepthFormat, Shadow, ShadowAddress},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MapKind {
    Depth,
    ColourMask,
}

/// A sampled output owned by this generation. Layer 11 binds the depth
/// bits through the normal NV2A pixel translation, or samples the colour
/// mask directly. The gel and all the receiver state stay per draw.
pub struct SampledMap {
    pub kind: MapKind,
    pub view: wgpu::TextureView,
    pub size: (u32, u32),
}

impl SampledMap {
    pub fn shadow_stage(&self, linear: bool, address: [ShadowAddress; 2]) -> Option<Shadow> {
        (self.kind == MapKind::Depth).then_some(Shadow {
            format: DepthFormat::D24,
            func: AlphaFunc::Greater,
            linear,
            address,
        })
    }
}

pub struct Producer {
    kind: MapKind,
    colour: wgpu::TextureView,
    depth: Option<wgpu::TextureView>,
    size: (u32, u32),
    first: bool,
    draws: usize,
}

fn attachment(device: &wgpu::Device, source: &wgpu::TextureView) -> wgpu::TextureView {
    let t = source.texture();
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("private shadow attachment"),
            size: t.size(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: t.format(),
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

fn copy_companion(
    encoder: &mut wgpu::CommandEncoder,
    source: &wgpu::TextureView,
    dest: &wgpu::TextureView,
) {
    encoder.copy_texture_to_texture(
        source.texture().as_image_copy(),
        dest.texture().as_image_copy(),
        dest.texture().size(),
    );
}

impl Producer {
    /// Start from the layer's own clear. Preserve the companion attachment
    /// where a caster can test it. `source` is the actual draw target,
    /// including the padded colour used for a larger depth level.
    pub fn new(
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        source: &RenderTarget<'_>,
        kind: MapKind,
    ) -> Option<Self> {
        let c = source.color.texture();
        let d = source.depth.map(|v| v.texture());
        if source.samples != 1
            || c.sample_count() != 1
            || c.dimension() != wgpu::TextureDimension::D2
            || c.depth_or_array_layers() != 1
            || source.size.0 == 0
            || source.size.1 == 0
            || source.size.0 > c.width()
            || source.size.1 > c.height()
            || d.is_some_and(|d| d.size() != c.size() || d.sample_count() != 1)
            || !matches!(
                source.color_format,
                wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
            )
        {
            return None;
        }
        let companion = match kind {
            MapKind::Depth => {
                if source.depth_format != Some(wgpu::TextureFormat::Depth32FloatStencil8)
                    || d.is_none()
                {
                    return None;
                }
                Some(c)
            }
            MapKind::ColourMask => d,
        };
        if companion.is_some_and(|t| !t.usage().contains(wgpu::TextureUsages::COPY_SRC)) {
            return None;
        }
        let colour = attachment(device, source.color);
        let depth = source.depth.map(|v| attachment(device, v));
        match kind {
            MapKind::Depth => copy_companion(encoder, source.color, &colour),
            MapKind::ColourMask => {
                if let Some((src, dst)) = source.depth.zip(depth.as_ref()) {
                    copy_companion(encoder, src, dst);
                }
            }
        }
        Some(Self {
            kind,
            colour,
            depth,
            size: source.size,
            first: true,
            draws: 0,
        })
    }

    /// Appends a complete batch in its original order. The caller discards
    /// the generation on any refusal. Already recorded pixels are private.
    pub fn record(
        &mut self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        clear: Clear,
        draws: &[RecordedDraw],
    ) -> Result<(), &'static str> {
        if draws.iter().any(RecordedDraw::counts_samples) {
            return Err("caster writes a visibility query");
        }
        let mut clear = clear;
        if self.first {
            match self.kind {
                MapKind::Depth => {
                    clear.depth = clear.depth.or(Some(1.0));
                    clear.stencil = clear.stencil.or(Some(0));
                }
                MapKind::ColourMask => clear.color = clear.color.or(Some([1.0, 1.0, 1.0, 0.0])),
            }
        }
        let target = RenderTarget {
            color: &self.colour,
            color_format: self.colour.texture().format(),
            depth: self.depth.as_ref(),
            depth_format: self.depth.as_ref().map(|v| v.texture().format()),
            size: self.size,
            samples: 1,
        };
        Renderer::encode_pass(ctx, encoder, &target, clear, draws, None)
            .map_err(|_| "caster batch refused")?;
        self.first = false;
        self.draws += draws.len();
        Ok(())
    }

    pub fn draws(&self) -> usize {
        self.draws
    }

    /// Make the shader input before any other light can reuse its source.
    /// D24 stores rounded integer depth bytes and stencil in RGBA8, exactly
    /// as on the D3D path. An object mask uses ordinary colour sampling.
    pub fn finish(
        self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        bits: &gfx::depth_bits::DepthBits,
    ) -> Option<SampledMap> {
        if self.first {
            return None;
        }
        let view = match self.kind {
            MapKind::Depth => {
                let depth = self.depth?.texture().clone();
                bits.convert(ctx, encoder, &depth, (depth.width(), depth.height()))
                    .create_view(&Default::default())
            }
            MapKind::ColourMask => {
                // A padded render target's bottom and right margins are not
                // part of the guest texture.
                let colour = self.colour.texture();
                if (colour.width(), colour.height()) == self.size {
                    self.colour
                } else {
                    let extent = wgpu::Extent3d {
                        width: self.size.0,
                        height: self.size.1,
                        depth_or_array_layers: 1,
                    };
                    let t = ctx.device.create_texture(&wgpu::TextureDescriptor {
                        label: Some("object shadow mask"),
                        size: extent,
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: wgpu::TextureDimension::D2,
                        format: colour.format(),
                        usage: wgpu::TextureUsages::COPY_SRC
                            | wgpu::TextureUsages::COPY_DST
                            | wgpu::TextureUsages::TEXTURE_BINDING,
                        view_formats: &[],
                    });
                    encoder.copy_texture_to_texture(
                        colour.as_image_copy(),
                        t.as_image_copy(),
                        extent,
                    );
                    t.create_view(&Default::default())
                }
            }
        };
        let size = (view.texture().width(), view.texture().height());
        Some(SampledMap {
            kind: self.kind,
            view,
            size,
        })
    }
}
