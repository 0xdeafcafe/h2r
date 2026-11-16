//! Uploading [`DecodedTexture`]s to the GPU.

use crate::texture::{DecodedTexture, TextureKind};

/// Creates a sampled wgpu texture holding every level/face of `tex`.
///
/// Cube maps become 6-layer 2D textures (view them with
/// `TextureViewDimension::Cube`); volumes become 3D textures.
pub fn upload_texture(device: &wgpu::Device, queue: &wgpu::Queue, tex: &DecodedTexture, label: &str) -> wgpu::Texture {
    let d = &tex.desc;
    let format = tex.host.to_wgpu();
    let (dimension, depth_or_layers) = match d.kind {
        TextureKind::D2 => (wgpu::TextureDimension::D2, 1),
        TextureKind::Cube => (wgpu::TextureDimension::D2, 6),
        TextureKind::Volume => (wgpu::TextureDimension::D3, d.depth),
    };
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d { width: d.width, height: d.height, depth_or_array_layers: depth_or_layers },
        mip_level_count: d.levels.clamp(1, crate::texture::max_levels(d.width, d.height, d.depth)),
        sample_count: 1,
        dimension,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for l in &tex.levels {
        // Block-compressed copies use the physical (rounded-up) size.
        let (w, h) = if tex.host.is_compressed() {
            (l.width.next_multiple_of(4), l.height.next_multiple_of(4))
        } else {
            (l.width, l.height)
        };
        let rows = if tex.host.is_compressed() { h / 4 } else { h };
        let origin_z = if d.kind == TextureKind::Cube { l.face } else { 0 };
        let depth = if d.kind == TextureKind::Volume { l.depth } else { 1 };
        crate::churn::queue_write();
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: l.level,
                origin: wgpu::Origin3d { x: 0, y: 0, z: origin_z },
                aspect: wgpu::TextureAspect::All,
            },
            &l.data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(tex.bytes_per_row(l)),
                rows_per_image: Some(rows),
            },
            wgpu::Extent3d { width: w, height: h, depth_or_array_layers: depth },
        );
    }
    texture
}
