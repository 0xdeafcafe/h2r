//! Metadata for an armed render.frame_layout capture. Vertex bounds use the
//! captured program and float inputs of small screen-space draws.

use super::{TextureSource, Tracked};
use agent_proto::{DrawLayout, LayoutTexture};
use d3d::backend::{PixelRect, RenderExtents, ResolvedDraw};

pub(super) fn describe<T, S: std::fmt::Debug>(
    draw: &ResolvedDraw,
    textures: [Option<&Tracked<T, S>>; 4],
    extents: RenderExtents,
    native_stage: Option<String>,
) -> DrawLayout {
    let v = draw.viewport;
    let rect = |r: PixelRect| [r.x, r.y, r.width, r.height];
    let clips = if draw.clip_rects.is_empty() {
        draw.clip.as_slice()
    } else {
        &draw.clip_rects
    };
    let sampled = draw.sampled_texture_stages();
    let (window_xy, uv) = vertex_bounds(draw);
    let display_grid = extents.color.and_then(|size| {
        gfx::raster::RenderScale::between((draw.target.width, draw.target.height), size)
    });
    DrawLayout {
        guest_size: [draw.target.width, draw.target.height],
        display_size: extents.color.map(|(w, h)| [w, h]),
        display_depth_size: extents.depth.map(|(w, h)| [w, h]),
        viewport: [v.x, v.y, v.width, v.height],
        clip: clips.iter().copied().map(rect).collect(),
        display_clip: display_grid.map_or_else(Vec::new, |grid| {
            clips
                .iter()
                .map(|&r| {
                    let r = draw.display_rect(r);
                    grid.rect(r.x, r.y, r.width, r.height)
                })
                .collect()
        }),
        projection_factor: draw.retail_constants.as_ref().map(|(_, factor)| *factor),
        ui_fit: draw.ui_fit.map(|fit| fit.transform),
        vertex_position: vertex_position(draw),
        sampled: draw
            .texture_info
            .iter()
            .enumerate()
            .filter_map(|(stage, info)| {
                let info = info.as_ref().filter(|_| sampled[stage])?;
                let state = &draw.state.stages[stage];
                let texture = textures[stage];
                Some(LayoutTexture {
                    stage,
                    id: texture.map(|t| t.id),
                    source: texture.map(|t| match &t.source {
                        TextureSource::Upload => "upload".into(),
                        TextureSource::Placeholder => "placeholder".into(),
                        TextureSource::Surface(kind, surface) => format!("{kind} {surface:?}"),
                    }),
                    header: info.header,
                    guest_size: [info.width, info.height],
                    display_size: extents.textures[stage].map(|(w, h)| [w, h]),
                    address: [
                        format!("{:?}", state.address_u),
                        format!("{:?}", state.address_v),
                    ],
                    uv: uv[stage],
                })
            })
            .collect(),
        native_stage,
        window_xy,
    }
}

fn vertex_position(draw: &ResolvedDraw) -> String {
    #[cfg(feature = "gpu")]
    if let d3d::shader::VertexProcessing::Program { microcode, .. } = &draw.shader.vertex {
        use d3d::interp::epilogue::{Position, position};
        return match position(microcode) {
            Position::Rows(first) => format!("rows c[{}..={}]", first, first + 3),
            Position::Fixed => "no constant rows".into(),
            Position::Unknown => "unrecognised".into(),
        };
    }
    if draw.layout.pretransformed {
        "pretransformed".into()
    } else {
        "unrecognised".into()
    }
}

type Bounds = [[f32; 2]; 2];

#[cfg(not(feature = "gpu"))]
fn vertex_bounds(_draw: &ResolvedDraw) -> (Option<Bounds>, [Option<Bounds>; 4]) {
    (None, [None; 4])
}

#[cfg(feature = "gpu")]
fn vertex_bounds(draw: &ResolvedDraw) -> (Option<Bounds>, [Option<Bounds>; 4]) {
    use d3d::shader::VertexProcessing;
    use shaders::vsh::{OutReg, Program, ProgramKind};

    let measure = || {
        // The backdrop and copy fans are small. Avoid interpreting scene
        // meshes or state programs on the game thread during the capture.
        if !(1..=8).contains(&draw.vertex_count) || draw.indices.is_some() {
            return None;
        }
        let program = match &draw.shader.vertex {
            VertexProcessing::Program { microcode, .. } => {
                Some(Program::from_words(ProgramKind::Normal, microcode).ok()?)
            }
            VertexProcessing::FixedFunction { .. } if draw.layout.pretransformed => None,
            _ => return None,
        };
        let direct = direct_projective_stages(draw);
        let vertices: Vec<_> = (0..draw.vertex_count as usize)
            .map(|vertex| {
                let inputs = vertex_inputs(draw, vertex)?;
                if let Some(program) = &program {
                    let mut constants = *draw.shader.constants;
                    let out = shaders::vsh::interp::run(program, &inputs, &mut constants);
                    let [x, y, _, _] = out.get(OutReg::Pos);
                    return Some(([x, y], std::array::from_fn(|n| out.get(OutReg::T(n as u8)))));
                }

                // XYZRHW uses the same half-pixel bias and texture input
                // selection as the translated pretransformed program.
                let [x, y, _, _] = inputs[0];
                let mut coords = [[0.0, 0.0, 0.0, 1.0]; 4];
                for (stage, uv) in coords.iter_mut().enumerate() {
                    let index = draw.state.stages[stage].tex_coord_index;
                    if index >= 4 {
                        return None;
                    }
                    *uv = inputs[9 + index as usize];
                }
                Some(([x + 0.5, y + 0.5], coords))
            })
            .collect::<Option<_>>()?;
        let xy = bounds(vertices.iter().map(|(xy, _)| *xy));
        let uv = std::array::from_fn(|stage| {
            let info = draw.texture_info[stage].filter(|_| direct[stage])?;
            if info.kind != gfx::texture::TextureKind::D2 {
                return None;
            }
            bounds(
                vertices
                    .iter()
                    .map(|(_, uv)| normalised_uv(uv[stage], info)),
            )
        });
        Some((xy, uv))
    };
    measure().unwrap_or((None, [None; 4]))
}

#[cfg(feature = "gpu")]
fn direct_projective_stages(draw: &ResolvedDraw) -> [bool; 4] {
    use shaders::psh::TexMode;

    if draw.shader.pixel_shader.is_none() {
        // The fixed-function combiner uses a projective 2D sample for
        // each active stage.
        return draw.sampled_texture_stages();
    }
    let modes = draw.raw.render[d3d::state::slot::PS_TEXTURE_MODES as usize];
    std::array::from_fn(|stage| {
        let mode = TexMode::from_bits((modes >> (5 * stage)) & 0x1f);
        matches!(mode, TexMode::Project2d | TexMode::Project3d)
    })
}

#[cfg(feature = "gpu")]
fn vertex_inputs(draw: &ResolvedDraw, vertex: usize) -> Option<[[f32; 4]; 16]> {
    use gfx::vertex::VertexDataType::*;

    let mut inputs = draw.shader.vertex_defaults;
    for (stream, source) in draw.layout.streams.iter().zip(&draw.streams) {
        let stride = if source.stride == 0 {
            stream.stride
        } else {
            source.stride
        } as usize;
        for attr in &stream.attributes {
            let value = inputs.get_mut(attr.register.0 as usize)?;
            let start = vertex * stride + attr.offset as usize;
            let bytes = source
                .data
                .get(start..start + attr.data_type.byte_size() as usize)?;
            *value = match attr.data_type {
                Float1 | Float2 | Float3 | Float4 | Float2H => {
                    let mut v = [0.0, 0.0, 0.0, 1.0];
                    for (component, bytes) in v.iter_mut().zip(bytes.chunks_exact(4)) {
                        *component = f32::from_le_bytes(bytes.try_into().ok()?);
                    }
                    if attr.data_type == Float2H {
                        v[3] = v[2];
                        v[2] = 0.0;
                    }
                    v
                }
                D3dColor => [bytes[2], bytes[1], bytes[0], bytes[3]].map(|c| f32::from(c) / 255.0),
                _ => return None,
            };
        }
    }
    Some(inputs)
}

#[cfg(feature = "gpu")]
fn normalised_uv([s, t, _, q]: [f32; 4], info: d3d::backend::TextureInfo) -> [f32; 2] {
    let scale = if info.format.layout() == gfx::format::Layout::Linear {
        [
            1.0 / info.width.max(1) as f32,
            1.0 / info.height.max(1) as f32,
        ]
    } else {
        [1.0; 2]
    };
    [(s / q) * scale[0], (t / q) * scale[1]]
}

#[cfg(feature = "gpu")]
fn bounds(mut points: impl Iterator<Item = [f32; 2]>) -> Option<Bounds> {
    let first = points.next()?;
    let mut range = [first, first];
    for point in std::iter::once(first).chain(points) {
        if !point.iter().all(|v| v.is_finite()) {
            return None;
        }
        for (axis, value) in point.into_iter().enumerate() {
            range[0][axis] = range[0][axis].min(value);
            range[1][axis] = range[1][axis].max(value);
        }
    }
    Some(range)
}

#[cfg(all(test, feature = "gpu"))]
mod tests {
    use super::*;
    use d3d::backend::TextureInfo;
    use gfx::{D3DFormat, texture::TextureKind};

    #[test]
    fn frame_layout_reports_projective_uvs_before_addressing() {
        let mut info = TextureInfo {
            header: 0,
            palette: 0,
            format: D3DFormat::LinA8R8G8B8,
            kind: TextureKind::D2,
            width: 8,
            height: 4,
            depth: 1,
            levels: 1,
        };
        let corners = [[-2.0, 0.0, 0.0, 2.0], [18.0, 8.0, 0.0, 2.0]];
        assert_eq!(
            bounds(corners.into_iter().map(|uv| normalised_uv(uv, info))),
            Some([[-0.125, 0.0], [1.125, 1.0]])
        );
        info.format = D3DFormat::A8R8G8B8;
        assert_eq!(normalised_uv(corners[1], info), [9.0, 4.0]);
        assert_eq!(
            bounds([normalised_uv([1., 2., 0., 0.], info)].into_iter()),
            None
        );
    }
}
