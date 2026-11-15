// Full-target textured blit used for Swap (backbuffer -> window) and for
// CopyRects/Resolve between surfaces whose size or host format differ.

struct Params {
    rect: vec4<f32>,   // source rect in normalised coordinates: x, y, w, h
    opaque: vec4<f32>, // x > 0.5: force alpha to 1 (X8R8G8B8 and friends)
    gamma: array<vec4<f32>, 256>,
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
    // One oversized triangle covering the target.
    let x = f32((i << 1u) & 2u);
    let y = f32(i & 2u);
    var out: VsOut;
    out.clip = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    out.uv = params.rect.xy + vec2<f32>(x, y) * params.rect.zw;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    var c = textureSample(src, smp, in.uv);
    if (params.opaque.y > 0.5) {
        let idx = vec3<u32>(round(clamp(c.rgb, vec3<f32>(0.0), vec3<f32>(1.0)) * 255.0));
        c = vec4<f32>(params.gamma[idx.r].r, params.gamma[idx.g].g, params.gamma[idx.b].b, c.a);
    }
    if (params.opaque.x > 0.5) {
        c.a = 1.0;
    }
    return c;
}
