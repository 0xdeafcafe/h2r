//! Vertex results for an immediate HUD fan carried by the overlay.
use gfx::vertex::VertexDataType;
use shaders::vsh::{self, OutReg};

use super::{
    FF_HALF_PIXEL, PassthroughFog, ProgramKind, RenderStateTable, ResolvedDraw, ShadeMode,
    VertexProcessing, draw_clamps_depth, pretransformed_program, raw, rs, w_buffered, w_clip_range,
};

type Corner = ([f32; 2], [[f32; 2]; 4], [f32; 4]);

/// Window position before display fit and subpixel fixup, normalized sprite coordinates
/// and the clamped diffuse varying of the four vertices. A clipped corner or perspective
/// varying that the affine overlay cannot represent leaves the original fan in place.
pub fn overlay_vertices(draw: &ResolvedDraw, sampled: [bool; 4]) -> Option<[Corner; 4]> {
    static TABLE: std::sync::LazyLock<RenderStateTable> =
        std::sync::LazyLock::new(RenderStateTable::default);
    if draw.vertex_count != 4
        || raw(&TABLE, draw, rs::TWOSIDEDLIGHTING) != 0
        || draw.layout.streams.len() != draw.streams.len()
    {
        return None;
    }
    let mut constants = *draw.shader.constants;
    let mut defaults = draw.shader.vertex_defaults;
    let program = match &draw.shader.vertex {
        VertexProcessing::Program { microcode, .. } => {
            vsh::Program::from_words(ProgramKind::Normal, microcode).ok()?
        }
        VertexProcessing::FixedFunction { .. } if draw.layout.pretransformed => {
            let mut coords = [0; 4];
            for (reg, state) in coords.iter_mut().zip(&draw.state.stages) {
                if state.tex_coord_index >= 4 {
                    return None;
                }
                *reg = 9 + state.tex_coord_index as u8;
            }
            defaults = [[0., 0., 0., 1.]; 16];
            defaults[3] = [1.; 4];
            constants = [[0.; 4]; 192];
            constants[FF_HALF_PIXEL] = [0.5, 0.5, 0., 0.];
            pretransformed_program(coords, PassthroughFog::SpecularAlpha)
        }
        _ => return None,
    };
    if program.writes_constants() {
        return None;
    }
    let z_max = gfx::renderer::d3dz_max(draw.target.depth_format);
    let w_depth =
        matches!(&draw.shader.vertex, VertexProcessing::Program { .. }) && w_buffered(draw);
    let [lo, hi] = w_clip_range(&draw.hw, z_max);
    let clamp = draw_clamps_depth(draw);
    let scales = draw.texture_info.map(|info| {
        info.filter(|t| t.format.layout() == gfx::format::Layout::Linear)
            .map_or([1.; 2], |t| {
                [1. / t.width.max(1) as f32, 1. / t.height.max(1) as f32]
            })
    });
    let mut corners = [([0.; 2], [[0.; 2]; 4], [0.; 4]); 4];
    let mut first_w = None;
    let mut first_q = [None; 4];
    for (i, corner) in corners.iter_mut().enumerate() {
        let mut inputs = defaults;
        for (layout, stream) in draw.layout.streams.iter().zip(&draw.streams) {
            let vertex = stream.data.get(i.checked_mul(stream.stride as usize)?..)?;
            for attr in &layout.attributes {
                let bytes = vertex.get(attr.offset as usize..)?;
                *inputs.get_mut(attr.register.0 as usize)? = attribute(attr.data_type, bytes)?;
            }
        }
        let output = vsh::interp::run(&program, &inputs, &mut constants);
        let pos = output.get(OutReg::Pos);
        let color = output.get(OutReg::D0);
        if !pos.into_iter().chain(color).all(f32::is_finite)
            || pos[3] <= 0.0
            || first_w.is_some_and(|w| w != pos[3])
            || (!clamp
                && if w_depth {
                    !(lo..=hi).contains(&pos[3])
                } else {
                    !(0.0..=z_max).contains(&pos[2])
                })
        {
            return None;
        }
        first_w = Some(pos[3]);
        let mut xy = [[0.; 2]; 4];
        for (stage, coords) in xy.iter_mut().enumerate() {
            if !sampled[stage] {
                continue;
            }
            let [s, t, _, q] = output.get(OutReg::T(stage as u8));
            if q == 0.0 || !q.is_finite() || first_q[stage].is_some_and(|p| p != q) {
                return None;
            }
            first_q[stage] = Some(q);
            *coords = [s / q * scales[stage][0], t / q * scales[stage][1]];
            if !coords.iter().all(|v| v.is_finite()) {
                return None;
            }
        }
        *corner = ([pos[0], pos[1]], xy, color.map(|v| v.clamp(0., 1.)));
    }
    if draw.state.render.shade_mode == ShadeMode::Flat
        && corners.iter().any(|c| c.2 != corners[0].2)
    {
        return None;
    }
    Some(corners)
}

fn attribute(ty: VertexDataType, bytes: &[u8]) -> Option<[f32; 4]> {
    use VertexDataType::*;
    let n = match ty {
        Float1 => 1,
        Float2 => 2,
        Float3 => 3,
        Float4 => 4,
        D3dColor => {
            let [b, g, r, a]: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
            return Some([r, g, b, a].map(|x| f32::from(x) / 255.0));
        }
        _ => return None,
    };
    let mut row = [0., 0., 0., 1.];
    for (v, b) in row.iter_mut().zip(bytes.get(..4 * n)?.chunks_exact(4)) {
        *v = f32::from_le_bytes(b.try_into().ok()?);
    }
    Some(row)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_float_lanes_and_packed_colour_match_the_input_layout() {
        assert_eq!(
            attribute(VertexDataType::Float1, &0.75f32.to_le_bytes()),
            Some([0.75, 0., 0., 1.])
        );
        assert_eq!(
            attribute(VertexDataType::D3dColor, &[0, 255, 51, 102]),
            Some([0.2, 1., 0., 0.4])
        );
        assert_eq!(attribute(VertexDataType::Float3, &[0; 8]), None);
    }
}
