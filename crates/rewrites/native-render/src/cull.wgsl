// The world's cull on the GPU (docs/graphics/native-world.md, "Cull"):
// each draw's world bounds against the view's planes and, for a view the
// pass can serve, a depth pyramid. It writes `instance_count` (word 1 of
// each 20-byte slot) of the two indirect buffers the world draws from:
// `early` (what the last test of this world kept) and `late` (the rest
// of what this frame keeps).

struct View {
    // `n · p + d >= 0` inside (`Frustum`).
    planes: array<vec4<f32>, 6>,
    view_proj: mat4x4<f32>,
    // x: the depth format's full scale, y: 1 / far, z: 1 to floor to the
    // format's step, w: unused (the world pass's `Frame::depth`).
    depth: vec4<f32>,
    // The pass's viewport in target pixels: x, y, width, height.
    viewport: vec4<f32>,
    // x: the draw count, yzw: unused.
    counts: vec4<u32>,
};

struct Bounds {
    // w: 1 when the draw has triangles and bounds.
    min: vec4<f32>,
    max: vec4<f32>,
};

@group(0) @binding(0) var<uniform> view: View;
@group(0) @binding(1) var<storage, read> bounds: array<Bounds>;
@group(0) @binding(2) var<storage, read_write> early: array<u32>;
@group(0) @binding(3) var<storage, read_write> late: array<u32>;
// 1 for a draw the last test kept (frustum and pyramid).
@group(0) @binding(4) var<storage, read_write> history: array<u32>;
// 0: kept by the frustum, 1: drawn early, 2: drawn late, 3: occluded.
@group(0) @binding(5) var<storage, read_write> stats: array<atomic<u32>, 4>;

@group(1) @binding(0) var pyramid: texture_2d<f32>;

fn may_see(b: Bounds) -> bool {
    if (b.min.w == 0.0) {
        return false;
    }
    for (var k = 0u; k < 6u; k++) {
        let p = view.planes[k];
        // The box corner furthest along the plane's normal.
        let c = select(b.min.xyz, b.max.xyz, p.xyz >= vec3<f32>(0.0));
        if (dot(p.xyz, c) + p.w < 0.0) {
            return false;
        }
    }
    return true;
}

// The stored depth a fragment at view depth `w` gets at the least
// (`world.wgsl`'s `w_depth`: floor only lowers it).
fn w_depth(w: f32) -> f32 {
    let z = w * view.depth.x * view.depth.y;
    let q = select(z, floor(z), view.depth.z != 0.0);
    return clamp(q / view.depth.x, 0.0, 1.0);
}

// Margin for the vertex stage's `1 / (1 / w)` against this `w`: about
// 170 D24 steps, 0.01 units at a far of 1024.
const SLACK: f32 = 1e-5;

// Whether every pixel the box can cover already holds nearer depth. False
// for a box across the eye's plane or wholly outside the viewport.
fn occluded(b: Bounds) -> bool {
    var lo = vec2<f32>(1.0);
    var hi = vec2<f32>(-1.0);
    var near = 3.4e38;
    for (var k = 0u; k < 8u; k++) {
        let p = vec3<f32>(
            select(b.min.x, b.max.x, (k & 1u) != 0u),
            select(b.min.y, b.max.y, (k & 2u) != 0u),
            select(b.min.z, b.max.z, (k & 4u) != 0u),
        );
        let clip = view.view_proj * vec4<f32>(p, 1.0);
        if (clip.w <= 1e-4) {
            return false;
        }
        let ndc = clip.xy / clip.w;
        lo = min(lo, ndc);
        hi = max(hi, ndc);
        near = min(near, clip.w);
    }
    lo = clamp(lo, vec2<f32>(-1.0), vec2<f32>(1.0));
    hi = clamp(hi, vec2<f32>(-1.0), vec2<f32>(1.0));
    let vp = view.viewport;
    // Target pixels, y down, one pixel wider on each side.
    let px0 = vp.x + (lo.x * 0.5 + 0.5) * vp.z - 1.0;
    let px1 = vp.x + (hi.x * 0.5 + 0.5) * vp.z + 1.0;
    let py0 = vp.y + (0.5 - hi.y * 0.5) * vp.w - 1.0;
    let py1 = vp.y + (0.5 - lo.y * 0.5) * vp.w + 1.0;
    let size = vec2<i32>(textureDimensions(pyramid, 0));
    let first = vec2<i32>(max(floor(vp.xy), vec2<f32>(0.0)));
    let last = min(vec2<i32>(ceil(vp.xy + vp.zw)) - 1, size - 1);
    let a = clamp(vec2<i32>(floor(vec2<f32>(px0, py0))), first, last);
    let z = clamp(vec2<i32>(floor(vec2<f32>(px1, py1))), first, last);
    if (any(z < a)) {
        return false;
    }
    // The level at which the rectangle spans at most two texels each way
    // (level k's texel x covers level 0's [x 2^k, (x + 1) 2^k)).
    let extent = u32(max(z.x - a.x, z.y - a.y) + 1);
    let levels = textureNumLevels(pyramid);
    let level = min(select(32u - countLeadingZeros(extent - 1u), 0u, extent <= 1u), levels - 1u);
    let top = max(vec2<i32>(textureDimensions(pyramid, level)) - 1, vec2<i32>(0));
    let a2 = min(a >> vec2<u32>(level), top);
    let z2 = min(z >> vec2<u32>(level), top);
    let d = max(
        max(textureLoad(pyramid, a2, i32(level)).x, textureLoad(pyramid, vec2<i32>(z2.x, a2.y), i32(level)).x),
        max(textureLoad(pyramid, vec2<i32>(a2.x, z2.y), i32(level)).x, textureLoad(pyramid, z2, i32(level)).x),
    );
    return w_depth(near) > d + SLACK;
}

// Before the world draws: without a pyramid this view, the frustum's set.
@compute @workgroup_size(64)
fn early_frustum(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= view.counts.x) {
        return;
    }
    let seen = may_see(bounds[i]);
    early[i * 5u + 1u] = u32(seen);
    late[i * 5u + 1u] = 0u;
    history[i] = u32(seen);
    if (seen) {
        atomicAdd(&stats[0], 1u);
        atomicAdd(&stats[1], 1u);
    }
}

// Before the world draws, with a pyramid to come: the frustum's set that
// the last test also kept.
@compute @workgroup_size(64)
fn early_history(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= view.counts.x) {
        return;
    }
    let seen = may_see(bounds[i]);
    let drawn = seen && history[i] != 0u;
    early[i * 5u + 1u] = u32(drawn);
    late[i * 5u + 1u] = 0u;
    if (seen) {
        atomicAdd(&stats[0], 1u);
    }
    if (drawn) {
        atomicAdd(&stats[1], 1u);
    }
}

// After the early draws, against the pyramid of the depth they leave:
// what the frustum keeps and the pyramid does not hide, less what the
// early draws drew; the kept set is the next frame's history.
@compute @workgroup_size(64)
fn late_occlusion(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= view.counts.x) {
        return;
    }
    let b = bounds[i];
    var kept = may_see(b);
    if (kept && occluded(b)) {
        kept = false;
        atomicAdd(&stats[3], 1u);
    }
    history[i] = u32(kept);
    let drawn = kept && early[i * 5u + 1u] == 0u;
    late[i * 5u + 1u] = u32(drawn);
    if (drawn) {
        atomicAdd(&stats[2], 1u);
    }
}

// After the early draws, when the pass cannot build the pyramid: the rest
// of the frustum's set.
@compute @workgroup_size(64)
fn late_frustum(@builtin(global_invocation_id) id: vec3<u32>) {
    let i = id.x;
    if (i >= view.counts.x) {
        return;
    }
    let seen = may_see(bounds[i]);
    history[i] = u32(seen);
    let drawn = seen && early[i * 5u + 1u] == 0u;
    late[i * 5u + 1u] = u32(drawn);
    if (drawn) {
        atomicAdd(&stats[2], 1u);
    }
}
