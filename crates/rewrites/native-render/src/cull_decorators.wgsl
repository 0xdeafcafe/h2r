// The decorators' cull (`cull_gpu::DecoratorCull`), after `cull.wgsl`'s
// `View`, `Bounds` and `occluded`: per batch of a view's plan, its
// instances whose placement group's sphere the view may see, in the
// guest's order, copied to the front of the batch's range of `dst`, and
// the batch's indirect slot drawing that many. A group's sphere holds
// every vertex its instances place (the guest culls the group by it), so
// a culled instance has no fragment in the view, or none past the depth.

struct DecoratorBatch {
    first: u32,
    count: u32,
    index_count: u32,
    first_index: u32,
    // 1 when every fragment the pyramid hides draws nothing
    // (`gpu::depth_hides`).
    occludable: u32,
    pad0: u32,
    pad1: u32,
    pad2: u32,
};

@group(2) @binding(0) var<storage, read> batches: array<DecoratorBatch>;
// Per instance: centre, radius (infinite: bounds nothing).
@group(2) @binding(1) var<storage, read> spheres: array<vec4<f32>>;
// `v11..v15` per instance, five rows each.
@group(2) @binding(2) var<storage, read> src: array<vec4<f32>>;
@group(2) @binding(3) var<storage, read_write> dst: array<vec4<f32>>;
// 20-byte indexed indirect slots, one per batch.
@group(2) @binding(4) var<storage, read_write> slots: array<u32>;

fn sphere_seen(s: vec4<f32>) -> bool {
    if (s.w > 3.0e38) {
        return true;
    }
    for (var k = 0u; k < 6u; k++) {
        let p = view.planes[k];
        if (dot(p.xyz, s.xyz) + p.w < -s.w * length(p.xyz)) {
            return false;
        }
    }
    return true;
}

fn place(at: u32, into: u32) {
    for (var r = 0u; r < 5u; r++) {
        dst[into * 5u + r] = src[at * 5u + r];
    }
}

fn write_slot(b: u32, batch: DecoratorBatch, kept: u32) {
    slots[b * 5u + 0u] = batch.index_count;
    slots[b * 5u + 1u] = kept;
    slots[b * 5u + 2u] = batch.first_index;
    slots[b * 5u + 3u] = 0u;
    slots[b * 5u + 4u] = 0u;
}

// Without a pyramid: the frustum alone.
@compute @workgroup_size(64)
fn decorators_frustum(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x;
    if (b >= view.counts.x) {
        return;
    }
    let batch = batches[b];
    var kept = 0u;
    for (var n = 0u; n < batch.count; n++) {
        let i = batch.first + n;
        if (sphere_seen(spheres[i])) {
            place(i, batch.first + kept);
            kept += 1u;
        }
    }
    write_slot(b, batch, kept);
}

// With the pyramid this view's layer 3 built (`cull.wgsl`'s
// `layer_occlusion` rule: read only when its build stamped the expected
// token): an occludable batch's instance whose sphere's box it hides is
// culled too.
@compute @workgroup_size(64)
fn decorators_occlusion(@builtin(global_invocation_id) id: vec3<u32>) {
    let b = id.x;
    if (b >= view.counts.x) {
        return;
    }
    let served = pyramid_token[0] == view.counts.y && pyramid_token[1] == view.counts.z;
    let batch = batches[b];
    var kept = 0u;
    for (var n = 0u; n < batch.count; n++) {
        let i = batch.first + n;
        let s = spheres[i];
        var seen = sphere_seen(s);
        if (seen && served && batch.occludable != 0u && s.w <= 3.0e38) {
            let r = vec3<f32>(s.w);
            seen = !occluded(Bounds(vec4<f32>(s.xyz - r, 1.0), vec4<f32>(s.xyz + r, 0.0)));
        }
        if (seen) {
            place(i, batch.first + kept);
            kept += 1u;
        }
    }
    write_slot(b, batch, kept);
}
