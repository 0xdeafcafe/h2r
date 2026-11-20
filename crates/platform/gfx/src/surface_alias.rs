//! GPU texture decoding for reinterpreted render-target memory. The usual
//! aligned 32-bit depth alias has a cheaper dedicated pass in guest_words.
//! This path also handles sub-word addresses, BC blocks, mips and cube faces.
use std::collections::HashMap;

use crate::format::{D3DFormat, TexelFormat};
use crate::texture::{TextureDesc, TextureKind};
use wgpu::util::DeviceExt;

#[derive(Default)]
pub struct AliasDecoder {
    pipelines: HashMap<(D3DFormat, D3DFormat), wgpu::ComputePipeline>,
}

// Pack one source pixel as encode_color / encode_depth would in guest RAM.
fn source_word(format: D3DFormat) -> Result<String, String> {
    use TexelFormat::*;
    if format.is_depth() {
        let (max, finish) = match format {
            D3DFormat::D16 | D3DFormat::LinD16 => ("65535.0", "return z;"),
            D3DFormat::D24S8 | D3DFormat::LinD24S8 => ("16777215.0", "return (z << 8u) | textureLoad(stencil, q, 0).r;"),
            _ => return Err(format!("cannot encode {format:?} as guest depth")),
        };
        return Ok(format!("let v = clamp(textureLoad(depth, q, 0), 0.0, 1.0) * {max};
            let z = u32(v) + select(0u, 1u, v - trunc(v) >= 0.5); {finish}"));
    }
    let bits = match format.texel() {
        A8R8G8B8 => "c.b | (c.g << 8u) | (c.r << 16u) | (c.a << 24u)",
        X8R8G8B8 => "c.b | (c.g << 8u) | (c.r << 16u) | 0xff000000u",
        R5G6B5 => "((c.r >> 3u) << 11u) | ((c.g >> 2u) << 5u) | (c.b >> 3u)",
        A1R5G5B5 => "((c.a >> 7u) << 15u) | ((c.r >> 3u) << 10u) | ((c.g >> 3u) << 5u) | (c.b >> 3u)",
        X1R5G5B5 => "0x8000u | ((c.r >> 3u) << 10u) | ((c.g >> 3u) << 5u) | (c.b >> 3u)",
        A4R4G4B4 => "((c.a >> 4u) << 12u) | ((c.r >> 4u) << 8u) | ((c.g >> 4u) << 4u) | (c.b >> 4u)",
        _ => return Err(format!("cannot encode {format:?} as guest colour")),
    };
    Ok(format!("let c = vec4u(round(textureLoad(color, q, 0) * 255.0)); return {bits};"))
}

// Channel order and bit expansion follow convert::to_rgba8. Generate the
// format-specific expression once, outside the per-pixel address calculation.
fn decode(format: D3DFormat) -> Result<String, String> {
    use TexelFormat::*;
    let channels = match format.texel() {
        L8 => "x,x,x,255u", Al8 => "x,x,x,x", A8 => "255u,255u,255u,x",
        A8L8 => "x,x,x,y", G8B8 => "x,y,x,y", R8B8 => "y,x,x,y", L16 => "255u,y,x,255u",
        A8R8G8B8 => "z,y,x,a", X8R8G8B8 => "z,y,x,255u", A8B8G8R8 => "x,y,z,a",
        B8G8R8A8 => "y,z,a,x", R8G8B8A8 => "a,z,y,x",
        A1R5G5B5 => "e5(v >> 10u),e5(v >> 5u),e5(v),255u * (v >> 15u)",
        X1R5G5B5 => "e5(v >> 10u),e5(v >> 5u),e5(v),255u",
        A4R4G4B4 => "e4(v >> 8u),e4(v >> 4u),e4(v),e4(v >> 12u)",
        R5G6B5 => "rgb565(v),255u", R6G5B5 => "e6(v >> 10u),e5(v >> 5u),e5(v),255u",
        R5G5B5A1 => "e5(v >> 11u),e5(v >> 6u),e5(v >> 1u),255u * (v & 1u)",
        R4G4B4A4 => "e4(v >> 12u),e4(v >> 8u),e4(v >> 4u),e4(v)",
        Unsupported(f) if f.is_depth() && f.bytes_per_texel() == Some(2) => "y,x,0u,0u",
        Unsupported(f) if f.is_depth() => "a,z,y,x",
        Yuy2 => return Ok("return decode_yuv(pos, sub, false);".into()),
        Uyvy => return Ok("return decode_yuv(pos, sub, true);".into()),
        Dxt1 => return Ok("return decode_bc(pos, sub, 1u);".into()),
        Dxt3 => return Ok("return decode_bc(pos, sub, 2u);".into()),
        Dxt5 => return Ok("return decode_bc(pos, sub, 3u);".into()),
        P8 | Unsupported(_) => return Err(format!("cannot decode {format:?} without a palette or colour conversion")),
    };
    let address = if format.layout() == crate::format::Layout::Swizzled {
        "morton(pos) * p.bpp"
    } else {
        "pos.y * p.pitch + pos.x * p.bpp"
    };
    Ok(format!("let v = read_le(p.base + {address}, p.bpp, sub);
        let x = v & 255u; let y = (v >> 8u) & 255u; let z = (v >> 16u) & 255u; let a = v >> 24u;
        return vec4u({channels});"))
}

impl AliasDecoder {
    /// Rows are staged on the GPU because a volume mip and a cube face use
    /// different storage view dimensions. A bounded buffer also fits large
    /// aliases without requiring a whole mip's worth of storage bindings.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(&mut self, device: &wgpu::Device, enc: &mut wgpu::CommandEncoder,
        source: &wgpu::Texture, format: D3DFormat, size: (u32, u32), pitch: u32,
        offset: usize, desc: &TextureDesc, signed: [bool; 4]) -> Result<wgpu::Texture, String> {
        let layout = desc.layout().map_err(|e| e.to_string())?;
        let bpp = format.bytes_per_texel().ok_or("invalid surface format")?;
        if size.0 == 0 || size.1 == 0 || (pitch != 0 && pitch < size.0 * bpp) || source.sample_count() != 1
            || offset.checked_add(layout.total_size).is_none_or(|end| end > pitch.max(size.0 * bpp) as usize * size.1 as usize) {
            return Err("invalid alias source or byte range".into());
        }
        let scale = crate::raster::RenderScale::between(size, (source.width(), source.height()))
            .ok_or("empty alias source")?;
        let key = (format, desc.format);
        if let std::collections::hash_map::Entry::Vacant(entry) = self.pipelines.entry(key) {
            let shader = include_str!("surface_alias.wgsl")
                .replace("SOURCE_WORD", &source_word(format)?)
                .replace("DECODE", &decode(desc.format)?);
            let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("guest surface alias"), source: wgpu::ShaderSource::Wgsl(shader.into()),
            });
            entry.insert(device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("guest surface alias"), layout: None, module: &module,
                entry_point: Some("main"), compilation_options: Default::default(), cache: None,
            }));
        }
        let pipe = &self.pipelines[&key];
        let limits = device.limits();
        let volume = desc.kind == TextureKind::Volume;
        let limit = if volume { limits.max_texture_dimension_3d } else { limits.max_texture_dimension_2d };
        // Reinterpreted cube faces must remain square and volume mip slices
        // share an isotropic grid. Use the guest layout for those classes
        // when the display's scale is not an integer in both directions.
        let out_grid = if desc.kind != TextureKind::D2 {
            scale.uniform_integer().unwrap_or(1).into()
        } else { scale };
        let (w, h) = out_grid.size(desc.width, desc.height);
        // A header can describe a full mip chain. Keep every guest mip legal
        // even when the window is smaller than the guest.
        let (width, height) = if desc.levels > 1 {
            (w.max(desc.width).min(limit), h.max(desc.height).min(limit))
        } else { (w.min(limit), h.min(limit)) };
        if volume && desc.depth > limit { return Err("alias exceeds GPU texture limits".into()); }
        let out = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("guest surface alias"), size: wgpu::Extent3d {
                width, height, depth_or_array_layers: if volume { desc.depth } else { layout.faces },
            }, mip_level_count: layout.levels.len() as u32, sample_count: 1,
            dimension: if volume { wgpu::TextureDimension::D3 } else { wgpu::TextureDimension::D2 },
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        let source_view = source.create_view(&wgpu::TextureViewDescriptor {
            aspect: if format.is_depth() { wgpu::TextureAspect::DepthOnly } else { wgpu::TextureAspect::All },
            ..Default::default()
        });
        let stencil = source.format().has_stencil_aspect().then(|| source.create_view(&wgpu::TextureViewDescriptor {
            aspect: wgpu::TextureAspect::StencilOnly, ..Default::default()
        }));
        for level in &layout.levels {
            let (w, h) = ((width >> level.level).max(1), (height >> level.level).max(1));
            let row = (w * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
            let rows = (limits.max_storage_buffer_binding_size.min(4 << 20) / u64::from(row)).min(u64::from(h)) as u32;
            if rows == 0 { return Err("alias row exceeds storage buffer limit".into()); }
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("alias transfer rows"), size: u64::from(row) * u64::from(rows),
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC, mapped_at_creation: false,
            });
            for face in 0..layout.faces {
                let base = u32::try_from(offset + face as usize * layout.face_stride + level.offset)
                    .map_err(|_| "alias byte address exceeds u32")?;
                for slice in 0..level.depth {
                    for first in (0..h).step_by(rows as usize) {
                        let count = (h - first).min(rows);
                        let params = [size.0, pitch, bpp, source.width(), base, desc.pitch, desc.format.bytes_per_texel().unwrap_or(0),
                            crate::guest_words::sign_flip(signed), level.width, level.height, level.depth, slice, w, h, first, row / 4, size.1, source.height(), 0, 0];
                        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                            label: Some("alias level"), usage: wgpu::BufferUsages::UNIFORM, contents: bytemuck::cast_slice(&params),
                        });
                        let mut entries = vec![
                            wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: 1, resource: buffer.as_entire_binding() },
                            wgpu::BindGroupEntry { binding: if format.is_depth() { 3 } else { 2 }, resource: wgpu::BindingResource::TextureView(&source_view) },
                        ];
                        if let Some(view) = &stencil {
                            entries.push(wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(view) });
                        }
                        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                            label: None, layout: &pipe.get_bind_group_layout(0), entries: &entries,
                        });
                        {
                            let mut pass = enc.begin_compute_pass(&Default::default());
                            pass.set_pipeline(pipe);
                            pass.set_bind_group(0, &bind, &[]);
                            pass.dispatch_workgroups(w.div_ceil(8), count.div_ceil(8), 1);
                        }
                        enc.copy_buffer_to_texture(wgpu::TexelCopyBufferInfo { buffer: &buffer,
                            layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: None },
                        }, wgpu::TexelCopyTextureInfo { texture: &out, mip_level: level.level,
                            origin: wgpu::Origin3d { x: 0, y: first, z: if volume { slice } else { face } },
                            aspect: wgpu::TextureAspect::All,
                        }, wgpu::Extent3d { width: w, height: count, depth_or_array_layers: 1 });
                    }
                }
            }
        }
        Ok(out)
    }
}
