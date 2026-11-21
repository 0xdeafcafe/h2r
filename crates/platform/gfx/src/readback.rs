//! Blocking GPU-to-CPU texture copies.

use crate::context::GfxContext;

/// Copies one aspect of a 2D texture's base level to the CPU as tightly
/// packed rows of `bytes_per_pixel` bytes.
///
/// The texture needs `COPY_SRC`, and the format/aspect combination must be
/// copyable (wgpu validates that).
pub fn read_texture(
    ctx: &GfxContext,
    texture: &wgpu::Texture,
    aspect: wgpu::TextureAspect,
    bytes_per_pixel: u32,
    size: (u32, u32),
) -> Vec<u8> {
    try_read_texture(ctx, texture, aspect, bytes_per_pixel, size).expect("texture readback")
}

/// Propagate a failed device poll or buffer mapping to an interactive request.
pub fn try_read_texture(
    ctx: &GfxContext,
    texture: &wgpu::Texture,
    aspect: wgpu::TextureAspect,
    bytes_per_pixel: u32,
    size: (u32, u32),
) -> Result<Vec<u8>, String> {
    let (w, h) = size;
    let unpadded = w * bytes_per_pixel;
    let padded = unpadded.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = crate::churn::create_buffer(&ctx.device, &wgpu::BufferDescriptor {
        label: Some("gfx readback"),
        size: u64::from(padded) * u64::from(h),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut enc = ctx.device.create_command_encoder(&Default::default());
    enc.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo { texture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(padded), rows_per_image: Some(h) },
        },
        wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
    );
    ctx.queue.submit([enc.finish()]);
    let slice = buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    ctx.device.poll(wgpu::PollType::wait_indefinitely()).map_err(|e| e.to_string())?;
    let data = slice.get_mapped_range().map_err(|e| e.to_string())?;
    let mut out = Vec::with_capacity((unpadded * h) as usize);
    for row in data.chunks(padded as usize).take(h as usize) {
        out.extend_from_slice(&row[..unpadded as usize]);
    }
    Ok(out)
}
