// Full-target projective warp (d3d's interpolated reprojection): each
// destination pixel samples the source at H * (u, v, 1), in normalised
// texture coordinates. Outside the source the edge texel repeats (the
// source is rendered with overscan, so that is rare).

struct Params {
    // The homography's rows (w unused).
    h0: vec4<f32>,
    h1: vec4<f32>,
    h2: vec4<f32>,
};

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var src: texture_2d<f32>;
@group(0) @binding(2) var smp: sampler;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var out: VsOut;
    out.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = vec2<f32>(x, y);
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec3<f32>(in.uv, 1.0);
    let q = vec3<f32>(dot(params.h0.xyz, p), dot(params.h1.xyz, p), dot(params.h2.xyz, p));
    let uv = q.xy / q.z;
    return textureSample(src, smp, uv);
}
