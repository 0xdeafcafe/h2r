// The composite (crate::showcase_gpu): the picture's target drawn into the
// menu's colour target over a rectangle, premultiplied, scaled by the
// widget's alpha. The rectangle may run past the target's edge; the
// rasteriser clips it.
struct Composite {
    // x the alpha.
    alpha: vec4<f32>,
    // x, y, width, height in the target's texels.
    rect: vec4<f32>,
    // xy the target's size in texels.
    size: vec4<f32>,
};

@group(0) @binding(0) var<uniform> composite: Composite;
@group(0) @binding(1) var picture: texture_2d<f32>;
@group(0) @binding(2) var picture_sampler: sampler;

struct Q {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_quad(@builtin(vertex_index) i: u32) -> Q {
    let corner = vec2<f32>(f32(i & 1u), f32(i >> 1u));
    let pixel = composite.rect.xy + corner * composite.rect.zw;
    let ndc = pixel / composite.size.xy * 2.0 - vec2<f32>(1.0);
    var out: Q;
    out.position = vec4<f32>(ndc.x, -ndc.y, 0.0, 1.0);
    out.uv = corner;
    return out;
}

@fragment
fn fs_quad(q: Q) -> @location(0) vec4<f32> {
    return textureSample(picture, picture_sampler, q.uv) * composite.alpha.x;
}
