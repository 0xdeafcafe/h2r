// The depth pyramid the world's late cull reads (cull.wgsl): level 0 the
// depth target as it is after the early draws, each level above the
// largest of the texels below it. A level's size is the texture mip's
// (halved, rounded down), so its last column and row also take the odd
// column and row below: texel x of level k covers level 0's
// [x 2^k, (x + 1) 2^k), and the last one runs to the edge.

@group(0) @binding(0) var depth: texture_depth_2d;
@group(0) @binding(1) var seed_out: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn seed(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(seed_out);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let d = textureLoad(depth, vec2<i32>(id.xy), 0);
    textureStore(seed_out, vec2<i32>(id.xy), vec4<f32>(d, 0.0, 0.0, 0.0));
}

@group(0) @binding(2) var below: texture_2d<f32>;
@group(0) @binding(3) var above: texture_storage_2d<r32float, write>;

@compute @workgroup_size(8, 8)
fn reduce(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(above);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let below_size = textureDimensions(below);
    let last = vec2<i32>(below_size) - 1;
    let at = vec2<i32>(id.xy) * 2;
    // Three wide where this is the last column (row) over an odd one.
    let odd = (id.xy == size - 1u) & ((below_size & vec2<u32>(1u)) == vec2<u32>(1u));
    let span = vec2<i32>(select(vec2<u32>(2u), vec2<u32>(3u), odd));
    var d = 0.0;
    for (var y = 0; y < span.y; y++) {
        for (var x = 0; x < span.x; x++) {
            d = max(d, textureLoad(below, min(at + vec2<i32>(x, y), last), 0).x);
        }
    }
    textureStore(above, vec2<i32>(id.xy), vec4<f32>(d, 0.0, 0.0, 0.0));
}
