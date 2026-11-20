// The native world's lightmap pass (docs/graphics/native-renderer.md):
// base map times the lightmap page, depth written the way the game's
// W-buffer stores it.

struct Frame {
    view_proj: mat4x4<f32>,
    // x: the depth format's full scale, y: 1 / far, z: 1 to floor to the
    // format's step, w: unused.
    depth: vec4<f32>,
};

struct Batch {
    // x: 1 when the batch has a lightmap page.
    flags: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(1) @binding(0) var<uniform> batch: Batch;
@group(1) @binding(1) var base_map: texture_2d<f32>;
@group(1) @binding(2) var lightmap: texture_2d<f32>;
@group(1) @binding(3) var linear: sampler;

struct V {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) lightmap_uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) lightmap_uv: vec2<f32>,
    @location(3) normal: vec3<f32>,
) -> V {
    var out: V;
    out.position = frame.view_proj * vec4<f32>(position, 1.0);
    out.uv = uv;
    out.lightmap_uv = lightmap_uv;
    return out;
}

struct Out {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

// position.w is 1/w interpolated linearly in screen space; its reciprocal
// is the perspective-correct view depth (as `xps_w_depth`).
fn w_depth(position: vec4<f32>) -> f32 {
    let w = 1.0 / position.w;
    let z = w * frame.depth.x * frame.depth.y;
    let q = select(z, floor(z), frame.depth.z != 0.0);
    return clamp(q / frame.depth.x, 0.0, 1.0);
}

@fragment
fn fs_main(v: V) -> Out {
    let base = textureSample(base_map, linear, v.uv);
    var light = vec3<f32>(1.0);
    if (batch.flags.x != 0.0) {
        light = textureSample(lightmap, linear, v.lightmap_uv).rgb;
    }
    return Out(vec4<f32>(base.rgb * light, 1.0), w_depth(v.position));
}
