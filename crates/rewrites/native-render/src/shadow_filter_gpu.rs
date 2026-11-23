//! The object shadow's filter `0x000222a0`, prepared from its scene data
//! (docs/graphics/native-shadow-layers.md, "The filter, native"). Each
//! quad reads and writes the guest's target 10 or 11, and the last copy
//! returns the filtered mask to target 10. Nothing here reads guest state.

use frame_scene::ShadowFilter;
use gfx::external::ExternalEncoded;

use crate::glow::{self, PostSurface, QuadDrawer};

fn slot_of(index: u32) -> Option<usize> {
    match index {
        10 => Some(0),
        11 => Some(1),
        _ => None,
    }
}

/// Prepare the filter on the two live target slots (10, 11). Every
/// pipeline, texture group and uniform write is complete before the
/// returned commands record, so a caller can validate and discard them.
pub fn prepare(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    uploads: &gfx::ring::SubmissionUploads,
    drawer: &mut QuadDrawer,
    filter: &ShadowFilter,
    slots: [PostSurface<'_>; 2],
) -> Result<ExternalEncoded, &'static str> {
    if slots
        .iter()
        .any(|slot| slot.guest_size != (filter.size[0], filter.size[1]))
    {
        return Err("shadow filter target size changed");
    }
    let mut commands = Vec::with_capacity(filter.draws.len());
    for draw in &filter.draws {
        let (Some(source), Some(dest)) = (slot_of(draw.source), slot_of(draw.dest)) else {
            return Err("shadow filter names another target");
        };
        let source_slot = slots[source];
        let dest_slot = slots[dest];
        if !glow::live_colour(dest_slot.view) || !glow::sampleable(device, source_slot.view) {
            return Err("shadow filter attachment is not drawable and sampleable");
        }
        let stages = [
            Some(source_slot.view),
            Some(source_slot.view),
            Some(source_slot.view),
            Some(source_slot.view),
        ];
        let program = drawer
            .program(
                &filter.combiner,
                [Some((filter.size[0], filter.size[1])); 4],
            )
            .map_err(|_| "shadow filter combiner refused")?;
        let quad = glow::quad(
            [0.0, 0.0, filter.size[0] as f32, filter.size[1] as f32],
            filter.signs.map(f32::from_bits),
            (filter.size[0], filter.size[1]),
            [Some((filter.size[0], filter.size[1])); 4],
            4,
            f32::from_bits(draw.spread),
        );
        let command = drawer.quad(
            device,
            queue,
            uploads,
            &program,
            &quad,
            dest_slot.view,
            dest_slot.view.texture().format(),
            None,
            stages,
        )?;
        commands.push(command.ok_or("shadow filter quad has no pixels")?);
    }
    let copy = match filter.copy {
        Some((source, dest)) => {
            let (Some(source), Some(dest)) = (slot_of(source), slot_of(dest)) else {
                return Err("shadow filter copy names another target");
            };
            let (source, dest) = (slots[source].view, slots[dest].view);
            if source.texture().format() != dest.texture().format()
                || source.texture().size() != dest.texture().size()
                || !source
                    .texture()
                    .usage()
                    .contains(wgpu::TextureUsages::COPY_SRC)
                || !dest
                    .texture()
                    .usage()
                    .contains(wgpu::TextureUsages::COPY_DST)
            {
                return Err("shadow filter copy is incompatible");
            }
            Some((source.clone(), dest.clone(), source.texture().size()))
        }
        None => None,
    };
    let ranges = filter.draws.len();
    let passes = commands.len();
    Ok(ExternalEncoded {
        ranges,
        draws: ranges,
        passes,
        commands: Box::new(move |encoder| {
            for command in &commands {
                command.record(encoder);
            }
            if let Some((source, dest, extent)) = copy {
                encoder.copy_texture_to_texture(
                    source.as_image_copy(),
                    dest.as_image_copy(),
                    extent,
                );
            }
        }),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiny_filter_copies_its_source_and_returns_it_to_the_mask() {
        use shaders::psh::{self, PixelShaderDef};
        let Ok(ctx) = gfx::GfxContext::headless() else {
            return;
        };
        let format = wgpu::TextureFormat::Rgba8Unorm;
        let size = (4, 4);
        let texture = || {
            ctx.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("test shadow filter"),
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
        };
        let (mask, other) = (texture(), texture());
        let pixels: Vec<u8> = (0..16u8).flat_map(|i| [i, 255 - i, i * 2, 255]).collect();
        ctx.queue.write_texture(
            mask.as_image_copy(),
            &pixels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * size.0),
                rows_per_image: Some(size.1),
            },
            mask.size(),
        );
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
        let combiner: [u32; 60] = std::array::from_fn(|i| {
            u32::from_le_bytes(bytes[4 * i..4 * i + 4].try_into().unwrap())
        });
        let filter = ShadowFilter {
            draws: vec![frame_scene::ShadowFilterDraw {
                source: 10,
                dest: 11,
                spread: 0,
            }],
            copy: Some((11, 10)),
            combiner,
            signs: [0; 2],
            size: [size.0, size.1],
        };
        let views = [&mask, &other].map(|t| t.create_view(&Default::default()));
        let slots = views.map(|view| PostSurface {
            view,
            guest_size: size,
        });
        let arena = gfx::ring::UploadPool::default().begin();
        let mut drawer = QuadDrawer::new();
        let commands =
            prepare(&ctx.device, &ctx.queue, &arena, &mut drawer, &filter, slots).unwrap();
        assert_eq!(
            (commands.ranges, commands.draws, commands.passes),
            (1, 1, 1)
        );
        let mut encoder = ctx.device.create_command_encoder(&Default::default());
        (commands.commands)(&mut encoder);
        arena.flush(&ctx.queue);
        ctx.queue.submit([encoder.finish()]);
        let read = |texture: &wgpu::Texture| {
            gfx::readback::read_texture(&ctx, texture, wgpu::TextureAspect::All, 4, size)
        };
        assert_eq!(read(&other), pixels, "the filter copied the source");
        assert_eq!(read(&mask), pixels, "and copied it back to target 10");
    }
}
