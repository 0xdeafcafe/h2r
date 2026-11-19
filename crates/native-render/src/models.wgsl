// The native objects' pass (docs/graphics/native-renderer.md, stage 2):
// GPU skinning from the game's node matrices, base map times a fixed
// ambient plus directional term, depth written the way the game's W-buffer
// stores it.

struct Frame {
    view_proj: mat4x4<f32>,
    // x: the depth format's full scale, y: 1 / far, z: 1 to floor to the
    // format's step, w: unused.
    depth: vec4<f32>,
    // rgb ambient.
    ambient: vec4<f32>,
    // xyz towards the light (world, unit), w the diffuse strength.
    light: vec4<f32>,
};

struct Instance {
    node_base: u32,
    node_count: u32,
    pad0: u32,
    pad1: u32,
};

@group(0) @binding(0) var<uniform> frame: Frame;
// Node matrices, 13 floats each (scale, forward, left, up, position).
@group(0) @binding(1) var<storage, read> nodes: array<f32>;
@group(0) @binding(2) var<storage, read> instances: array<Instance>;
// The model's inverse rest frames, same layout.
@group(1) @binding(0) var<storage, read> inverses: array<f32>;
@group(2) @binding(0) var base_map: texture_2d<f32>;
@group(2) @binding(1) var linear: sampler;

struct M {
    s: f32,
    f: vec3<f32>,
    l: vec3<f32>,
    u: vec3<f32>,
    p: vec3<f32>,
};

fn node_frame(i: u32) -> M {
    let o = 13u * i;
    return M(
        nodes[o],
        vec3<f32>(nodes[o + 1u], nodes[o + 2u], nodes[o + 3u]),
        vec3<f32>(nodes[o + 4u], nodes[o + 5u], nodes[o + 6u]),
        vec3<f32>(nodes[o + 7u], nodes[o + 8u], nodes[o + 9u]),
        vec3<f32>(nodes[o + 10u], nodes[o + 11u], nodes[o + 12u]),
    );
}

fn inverse_frame(i: u32) -> M {
    let o = 13u * i;
    return M(
        inverses[o],
        vec3<f32>(inverses[o + 1u], inverses[o + 2u], inverses[o + 3u]),
        vec3<f32>(inverses[o + 4u], inverses[o + 5u], inverses[o + 6u]),
        vec3<f32>(inverses[o + 7u], inverses[o + 8u], inverses[o + 9u]),
        vec3<f32>(inverses[o + 10u], inverses[o + 11u], inverses[o + 12u]),
    );
}

fn apply_point(m: M, v: vec3<f32>) -> vec3<f32> {
    return m.p + m.s * (v.x * m.f + v.y * m.l + v.z * m.u);
}

fn apply_vector(m: M, v: vec3<f32>) -> vec3<f32> {
    return v.x * m.f + v.y * m.l + v.z * m.u;
}

struct V {
    @builtin(position) @invariant position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_main(
    @builtin(instance_index) instance: u32,
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
    @location(3) node_index: vec4<u32>,
    @location(4) weight: vec4<f32>,
) -> V {
    let inst = instances[instance];
    var p = vec3<f32>(0.0);
    var n = vec3<f32>(0.0);
    for (var k = 0u; k < 4u; k = k + 1u) {
        let w = weight[k];
        if (w <= 0.0) {
            continue;
        }
        var i = node_index[k];
        if (i >= inst.node_count) {
            i = 0u;
        }
        let world = node_frame(inst.node_base + i);
        let inv = inverse_frame(i);
        p = p + w * apply_point(world, apply_point(inv, position));
        n = n + w * apply_vector(world, apply_vector(inv, normal));
    }
    var out: V;
    out.position = frame.view_proj * vec4<f32>(p, 1.0);
    out.uv = uv;
    out.normal = n;
    return out;
}

struct Out {
    @location(0) color: vec4<f32>,
    @builtin(frag_depth) depth: f32,
};

fn w_depth(position: vec4<f32>) -> f32 {
    let w = 1.0 / position.w;
    let z = w * frame.depth.x * frame.depth.y;
    let q = select(z, floor(z), frame.depth.z != 0.0);
    return clamp(q / frame.depth.x, 0.0, 1.0);
}

@fragment
fn fs_main(v: V) -> Out {
    let base = textureSample(base_map, linear, v.uv);
    let len = length(v.normal);
    var lit = frame.ambient.rgb;
    if (len > 1e-6) {
        lit = lit + frame.light.w * max(dot(v.normal / len, frame.light.xyz), 0.0);
    }
    return Out(vec4<f32>(base.rgb * lit, 1.0), w_depth(v.position));
}
