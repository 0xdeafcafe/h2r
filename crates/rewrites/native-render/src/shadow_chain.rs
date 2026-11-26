//! A private object-shadow chain (docs/graphics/native-shadow-layers.md,
//! "A private chain"): the mask the casters draw, the filter's partner and
//! the casters' depth, owned by the stage instead of the guest's targets
//! 10 and 11. The casters record into the mask with the begin's clear, the
//! filter runs over the pair (`shadow_filter_gpu::prepare` with
//! [`Pair::filter_slots`]) and the receivers sample [`Pair::sampled`].
//! Nothing here reads guest state.

use gfx::{Clear, GfxContext, RecordedDraw, RenderTarget, Renderer};

use crate::glow::PostSurface;
use crate::shadow_gpu::{MapKind, SampledMap};

/// The mask, the filter's other target and the casters' depth, all of one
/// size. The two colour textures share a format so the filter's final copy
/// back to the mask is legal.
pub struct Pair {
    pub mask: wgpu::TextureView,
    pub other: wgpu::TextureView,
    pub depth: Option<wgpu::TextureView>,
    /// The mask's size in texels.
    pub size: (u32, u32),
    /// The guest's size of the same surface: `size` at 1x, smaller under
    /// a render scale. The filter's quads are laid out in these units.
    pub guest_size: (u32, u32),
}

fn texture(
    device: &wgpu::Device,
    label: &'static str,
    format: wgpu::TextureFormat,
    size: (u32, u32),
) -> wgpu::TextureView {
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: size.0,
                height: size.1,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        })
        .create_view(&Default::default())
}

impl Pair {
    /// The pair for a mask of `size` in `colour` (an 8-bit RGBA format, as
    /// target 10's). `depth` is the format the casters' pipelines were
    /// recorded with, if they have one.
    pub fn new(
        device: &wgpu::Device,
        colour: wgpu::TextureFormat,
        depth: Option<wgpu::TextureFormat>,
        size: (u32, u32),
    ) -> Option<Self> {
        Self::scaled(device, colour, depth, size, size)
    }

    /// The pair for a mask drawn at a render scale: `size` texels over
    /// the guest's `guest_size`.
    pub fn scaled(
        device: &wgpu::Device,
        colour: wgpu::TextureFormat,
        depth: Option<wgpu::TextureFormat>,
        size: (u32, u32),
        guest_size: (u32, u32),
    ) -> Option<Self> {
        if size.0 == 0
            || size.1 == 0
            || guest_size.0 == 0
            || guest_size.1 == 0
            || !matches!(
                colour,
                wgpu::TextureFormat::Rgba8Unorm | wgpu::TextureFormat::Bgra8Unorm
            )
            || depth.is_some_and(|f| !f.is_depth_stencil_format())
        {
            return None;
        }
        let limit = device.limits().max_texture_dimension_2d;
        if size.0 > limit || size.1 > limit {
            return None;
        }
        Some(Self {
            mask: texture(device, "private shadow mask", colour, size),
            other: texture(device, "private shadow filter", colour, size),
            depth: depth.map(|f| texture(device, "private shadow depth", f, size)),
            size,
            guest_size,
        })
    }

    /// The layer-10 begin's clear of an object mask: white colour, zero
    /// alpha (native-shadows.md, "Object mask"), and a cleared depth.
    pub fn clear() -> Clear {
        Clear {
            color: Some([1.0, 1.0, 1.0, 0.0]),
            depth: Some(1.0),
            stencil: Some(0),
        }
    }

    /// Clear the mask and draw the casters into it in their order. The
    /// draws must have been recorded for this pair's formats and size.
    pub fn record_casters(
        &self,
        ctx: &GfxContext,
        encoder: &mut wgpu::CommandEncoder,
        draws: &[RecordedDraw],
    ) -> Result<usize, &'static str> {
        if draws.iter().any(RecordedDraw::counts_samples) {
            return Err("caster writes a visibility query");
        }
        let target = RenderTarget {
            color: &self.mask,
            color_format: self.mask.texture().format(),
            depth: self.depth.as_ref(),
            depth_format: self.depth.as_ref().map(|v| v.texture().format()),
            size: self.size,
            samples: 1,
        };
        Renderer::encode_pass(ctx, encoder, &target, Self::clear(), draws, None)
            .map_err(|_| "private caster batch refused")
    }

    /// The filter's two slots: the mask in place of target 10, the other
    /// in place of target 11.
    pub fn filter_slots(&self) -> [PostSurface<'_>; 2] {
        [&self.mask, &self.other].map(|view| PostSurface {
            view,
            guest_size: self.guest_size,
        })
    }

    /// The receivers' input once the filter's commands are recorded.
    pub fn sampled(&self) -> SampledMap {
        SampledMap {
            kind: MapKind::ColourMask,
            view: self.mask.clone(),
            size: self.size,
        }
    }

    /// Whether `dest` can take the mask in one copy: the guest's target 10
    /// when the chain hands its casters back to the guest's filter.
    pub fn spills_into(&self, dest: &wgpu::Texture) -> bool {
        let mask = self.mask.texture();
        dest.format() == mask.format()
            && (dest.width(), dest.height()) == self.size
            && dest.sample_count() == 1
            && dest.dimension() == wgpu::TextureDimension::D2
            && dest.usage().contains(wgpu::TextureUsages::COPY_DST)
    }

    /// Copy the mask into `dest`; [`Self::spills_into`] must hold.
    pub fn spill(&self, encoder: &mut wgpu::CommandEncoder, dest: &wgpu::Texture) {
        encoder.copy_texture_to_texture(
            self.mask.texture().as_image_copy(),
            dest.as_image_copy(),
            wgpu::Extent3d {
                width: self.size.0,
                height: self.size.1,
                depth_or_array_layers: 1,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use frame_scene::{ShadowFilter, ShadowFilterDraw};
    use shaders::psh::{self, PixelShaderDef};

    const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
    const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32FloatStencil8;

    fn copy_filter(size: (u32, u32)) -> ShadowFilter {
        let def = PixelShaderDef {
            texture_modes: 1,
            final_inputs_abcd: psh::bits::inputs(0, 0, 0, psh::bits::REG_T0),
            final_inputs_efg: psh::bits::final_efg(
                0,
                0,
                psh::bits::REG_T0 | psh::bits::CHAN_ALPHA,
                0,
            ),
            ..Default::default()
        };
        let bytes = def.to_bytes();
        ShadowFilter {
            draws: vec![ShadowFilterDraw {
                source: 10,
                dest: 11,
                spread: 0,
            }],
            copy: Some((11, 10)),
            combiner: std::array::from_fn(|i| {
                u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap())
            }),
            signs: [0; 2],
            size: [size.0, size.1],
        }
    }

    #[test]
    fn a_pair_refuses_empty_sizes_and_non_mask_formats() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let d = &ctx.device;
        assert!(Pair::new(d, FORMAT, None, (0, 4)).is_none());
        assert!(Pair::new(d, FORMAT, None, (4, 0)).is_none());
        assert!(Pair::new(d, wgpu::TextureFormat::R32Float, None, (4, 4)).is_none());
        assert!(Pair::new(d, FORMAT, Some(FORMAT), (4, 4)).is_none());
        let pair = Pair::new(d, FORMAT, Some(DEPTH), (4, 2)).unwrap();
        assert_eq!(pair.size, (4, 2));
        assert_eq!(pair.depth.as_ref().unwrap().texture().format(), DEPTH);
        let map = pair.sampled();
        assert_eq!((map.kind, map.size), (MapKind::ColourMask, (4, 2)));
    }

    #[test]
    fn casters_start_from_the_begins_clear_and_the_filter_stays_in_the_pair() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let size = (4, 4);
        let pair = Pair::new(&ctx.device, FORMAT, Some(DEPTH), size).unwrap();
        let read = |view: &wgpu::TextureView| {
            gfx::readback::read_texture(&ctx, view.texture(), wgpu::TextureAspect::All, 4, size)
        };

        // No casters: the mask holds the begin's clear.
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        pair.record_casters(&ctx, &mut encoder, &[]).unwrap();
        ctx.queue.submit([encoder.finish()]);
        assert_eq!(read(&pair.mask), [255u8, 255, 255, 0].repeat(16));

        // An invented mask through a copy filter: 10 -> 11, then back.
        let pixels: Vec<u8> = (0..16u8).flat_map(|i| [i, 255 - i, i * 3, 255]).collect();
        ctx.queue.write_texture(
            pair.mask.texture().as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size.0),
                rows_per_image: Some(size.1),
            },
            pair.mask.texture().size(),
        );
        let arena = gfx::ring::UploadPool::default().begin();
        let mut drawer = crate::glow::QuadDrawer::new();
        let commands = crate::shadow_filter_gpu::prepare(
            &ctx.device,
            &ctx.queue,
            &arena,
            &mut drawer,
            &copy_filter(size),
            pair.filter_slots(),
        )
        .unwrap();
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (commands.commands)(&mut encoder);
        arena.flush(&ctx.queue);
        ctx.queue.submit([encoder.finish()]);
        assert_eq!(
            read(&pair.other),
            pixels,
            "the filter's quad wrote the partner"
        );
        assert_eq!(
            read(&pair.mask),
            pixels,
            "and its copy returned to the mask"
        );
        assert_eq!(read(&pair.sampled().view), pixels);
    }

    #[test]
    fn a_spill_copies_the_mask_only_into_a_matching_texture() {
        let Ok(ctx) = GfxContext::headless() else {
            return;
        };
        let size = (2, 2);
        let pair = Pair::new(&ctx.device, FORMAT, None, size).unwrap();
        let guest = |format, size: (u32, u32), usage| {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("invented target 10"),
                size: wgpu::Extent3d {
                    width: size.0,
                    height: size.1,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let usage = wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC;
        assert!(!pair.spills_into(&guest(wgpu::TextureFormat::Bgra8Unorm, size, usage)));
        assert!(!pair.spills_into(&guest(FORMAT, (4, 4), usage)));
        assert!(!pair.spills_into(&guest(FORMAT, size, wgpu::TextureUsages::COPY_SRC)));
        let dest = guest(FORMAT, size, usage);
        assert!(pair.spills_into(&dest));
        let pixels: Vec<u8> = (0..4u8).flat_map(|i| [i, 7, 9, i]).collect();
        ctx.queue.write_texture(
            pair.mask.texture().as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size.0),
                rows_per_image: Some(size.1),
            },
            pair.mask.texture().size(),
        );
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        pair.spill(&mut encoder, &dest);
        ctx.queue.submit([encoder.finish()]);
        let read = gfx::readback::read_texture(&ctx, &dest, wgpu::TextureAspect::All, 4, size);
        assert_eq!(read, pixels);
    }
}
