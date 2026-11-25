//! Metadata for an armed render.frame_layout capture. Vertex bounds are
//! where the device places small screen-space draws (`d3d::screen_rect`).

use super::{TextureSource, Tracked};
use agent_proto::{DrawLayout, LayoutTexture};
use d3d::backend::{PixelRect, RenderExtents, ResolvedDraw};
use d3d::screen_rect::Bounds;
#[cfg(feature = "gpu")]
use d3d::screen_rect::{DrawVertices, bounds};

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
        use d3d::clip_space::epilogue::{Position, position};
        return match position(microcode) {
            Position::Rows(first) => format!("rows c[{}..={}]", first, first + 3),
            Position::FarPlaneRows(first) => format!("far plane rows c[{}..={}]", first, first + 3),
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

#[cfg(not(feature = "gpu"))]
fn vertex_bounds(_draw: &ResolvedDraw) -> (Option<Bounds>, [Option<Bounds>; 4]) {
    (None, [None; 4])
}

/// The window rectangle is the device's own reading
/// ([`d3d::screen_rect`]), the one its UI fit decides full-frame passes by.
#[cfg(feature = "gpu")]
fn vertex_bounds(draw: &ResolvedDraw) -> (Option<Bounds>, [Option<Bounds>; 4]) {
    // The backdrop and copy fans are small. Avoid interpreting scene
    // meshes or state programs on the game thread during the capture.
    let Some(vertices) = DrawVertices::of(draw).screen(8) else {
        return (None, [None; 4]);
    };
    let direct = direct_projective_stages(draw);
    let xy = bounds(vertices.iter().map(|v| v.xy));
    let uv = std::array::from_fn(|stage| {
        let info = draw.texture_info[stage].filter(|_| direct[stage])?;
        if info.kind != gfx::texture::TextureKind::D2 {
            return None;
        }
        bounds(vertices.iter().map(|v| normalised_uv(v.tex[stage], info)))
    });
    (xy, uv)
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
