// The particle draw (docs/graphics/native-particles.md, "GPU particle
// systems"): one instance a particle, read straight from the system's
// buffer, six vertices a quad. The instance count is the update's, in the
// indirect arguments; no vertex buffer. The structs are
// particle_common.wgsl's, prepended by particle_compute.rs.

struct View {
    clip_from_world: mat4x4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    w_row: vec4<f32>,
    // (full scale, clip min, clip max, flags: 1 floor, 2 clamp).
    depth: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> particles: array<Particle>;
@group(0) @binding(1) var<uniform> view: View;
@group(0) @binding(2) var sprite: texture_2d<f32>;
@group(0) @binding(3) var sprite_sampler: sampler;

struct Out {
    @builtin(position) clip: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) next_uv: vec2<f32>,
    @location(2) colour: vec4<f32>,
    @location(3) blend: f32,
    // The W-buffer value (fs_w).
    @location(4) w: f32,
}

// The D3D diagonal: (0,1,2), (0,2,3).
const CORNERS = array<vec2<f32>, 6>(
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0),
    vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
);

@vertex
fn vs(@builtin(vertex_index) v: u32, @builtin(instance_index) i: u32) -> Out {
    let p = particles[i];
    var corners = CORNERS;
    let c = corners[v];
    let local = vec2<f32>(mix(p.corners.x, p.corners.z, c.x), mix(p.corners.y, p.corners.w, c.y)) * p.size;
    let s = sin(p.rotation);
    let k = cos(p.rotation);
    let turned = vec2<f32>(local.x * k - local.y * s, local.x * s + local.y * k);
    let world = p.position + view.right.xyz * turned.x + view.up.xyz * turned.y;
    var out: Out;
    out.clip = view.clip_from_world * vec4<f32>(world, 1.0);
    out.uv = p.window.xy + c * p.window.zw;
    out.next_uv = p.next_window.xy + c * p.next_window.zw;
    out.colour = unpack4x8unorm(p.colour);
    out.blend = p.blend;
    out.w = dot(view.w_row, vec4<f32>(world, 1.0));
    return out;
}

fn sprite_colour(in: Out) -> vec4<f32> {
    let a = textureSample(sprite, sprite_sampler, in.uv);
    let b = textureSample(sprite, sprite_sampler, in.next_uv);
    return mix(a, b, in.blend) * in.colour;
}

@fragment
fn fs(in: Out) -> @location(0) vec4<f32> {
    return sprite_colour(in);
}

struct Stored {
    @location(0) colour: vec4<f32>,
    @builtin(frag_depth) depth: f32,
}

// The W-buffered draw: the depth tested is the guest's W-buffer value, as
// its programs store it (cull_gpu.rs `DepthModel::program`): W held to
// the clip range (out of it culled, or clamped), floored for an integer
// format, over the full scale.
@fragment
fn fs_w(in: Out) -> Stored {
    // Sampled first, in uniform control flow.
    let colour = sprite_colour(in);
    let flags = u32(view.depth.w);
    var w = in.w;
    if (w < view.depth.y || w > view.depth.z) {
        if ((flags & 2u) == 0u) {
            discard;
        }
        w = clamp(w, view.depth.y, view.depth.z);
    }
    if ((flags & 1u) != 0u) {
        w = floor(w);
    }
    var out: Stored;
    out.colour = colour;
    out.depth = clamp(w / view.depth.x, 0.0, 1.0);
    return out;
}
