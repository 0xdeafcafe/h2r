//! A narrow geometry proof for an opaque XYZRHW rectangle. Match the
//! built-in shader route and its window mapping, so an arbitrary translated
//! shader, scissor, or pixel rejection cannot appear to initialize a target.

use crate::backend::ResolvedDraw;
use crate::shader::VertexProcessing;
use frame_scene::AuxTarget;
use gfx::state::{CullMode, DepthBufferMode, FillMode};
use gfx::vertex::{PrimitiveType, VertexDataType, VertexRegister};

pub(super) fn covers_colour(draw: &ResolvedDraw, textured: bool, target: AuxTarget) -> bool {
    let r = draw.state.render;
    let (w, h) = (target.width, target.height);
    if !draw.layout.pretransformed
        || draw.vertex_count != 4
        || draw.indices.is_some()
        || !matches!(
            draw.primitive,
            PrimitiveType::QuadList | PrimitiveType::TriangleStrip
        )
        || !matches!(draw.shader.vertex, VertexProcessing::FixedFunction { .. })
        || !gfx::Renderer::builtin_supports(&draw.layout)
        || r.depth_mode != DepthBufferMode::Disabled
        || r.stencil.enable
        || r.blend_enable
        || r.alpha_test_enable
        || r.cull_mode != CullMode::None
        || r.fill_mode != FillMode::Solid
        || r.back_fill_mode != FillMode::Solid
        || (draw.target.width, draw.target.height) != (w, h)
        || w == 0
        || h == 0
        || !draw.clip_rects.is_empty()
        || !crate::nv2a::builtin_equivalent(draw, textured)
    {
        return false;
    }
    let whole_scissor = draw.clip.map_or_else(
        || {
            (
                draw.viewport.x,
                draw.viewport.y,
                draw.viewport.width,
                draw.viewport.height,
            ) == (0, 0, w, h)
        },
        |clip| clip.is_whole(w, h),
    );
    if !whole_scissor {
        return false;
    }

    let [layout] = draw.layout.streams.as_slice() else {
        return false;
    };
    let [stream] = draw.streams.as_slice() else {
        return false;
    };
    let Some(attr) = layout
        .attributes
        .iter()
        .find(|a| a.register == VertexRegister::POSITION)
    else {
        return false;
    };
    if attr.data_type != VertexDataType::Float4 {
        return false;
    }
    let stride = if stream.stride == 0 {
        layout.stride
    } else {
        stream.stride
    } as usize;
    let mut xy = [[0.0; 2]; 4];
    for (i, corner) in xy.iter_mut().enumerate() {
        let Some(start) = stride
            .checked_mul(i)
            .and_then(|k| k.checked_add(attr.offset as usize))
        else {
            return false;
        };
        let Some(bytes) = stream.data.get(start..).and_then(|b| b.get(..16)) else {
            return false;
        };
        let word = |j| f32::from_le_bytes(bytes[j..j + 4].try_into().unwrap());
        // All four vertices have finite window coordinates, the same clip W,
        // and a depth inside the built-in shader's clamped range.
        if word(12) != 1.0 || !word(8).is_finite() {
            return false;
        }
        *corner = [word(0), word(4)];
        if !corner.iter().all(|v| v.is_finite()) {
            return false;
        }
    }
    // Both primitives triangulate the rectangle along the shared diagonal.
    // Match its corners in perimeter order, rejecting crossed or degenerate
    // strips. The half-pixel translation then places every attachment sample
    // inside. Bound the values to ordinary finite rasterizer coordinates.
    if draw.primitive == PrimitiveType::TriangleStrip {
        xy.swap(2, 3);
    }
    let [[x0, y0], b, [x1, y1], d] = xy;
    let rectangle = (b == [x1, y0] && d == [x0, y1]) || (d == [x1, y0] && b == [x0, y1]);
    let spans = |a: f32, b: f32, size: u32| {
        let size = size as f32;
        a.min(b) == -0.5 && (a.max(b) == size - 0.5 || a.max(b) == size)
    };
    rectangle && spans(x0, x1, w) && spans(y0, y1, h)
}
