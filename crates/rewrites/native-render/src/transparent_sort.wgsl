// A stable LSD radix sort of up to 1024 transparent items, one bit per
// pass, in one workgroup. `keys[i]` is (tie, depth, class, 0), each word
// already in unsigned ascending order; `bits` lists the (word, bit) pairs
// that differ between items, least significant first. `order[0..n]` gets
// the item indices in sorted order; `order[n]` is 1 when two neighbours in
// that order have equal keys.

struct Params { n: u32, passes: u32, pad0: u32, pad1: u32 }

@group(0) @binding(0) var<uniform> params: Params;
@group(0) @binding(1) var<storage, read> keys: array<vec4<u32>>;
@group(0) @binding(2) var<storage, read> bits: array<vec2<u32>>;
@group(0) @binding(3) var<storage, read_write> order: array<u32>;

var<workgroup> a: array<u32, 1024>;
var<workgroup> b: array<u32, 1024>;
var<workgroup> scan: array<u32, 256>;
var<workgroup> tied: atomic<u32>;

@compute @workgroup_size(256)
fn sort(@builtin(local_invocation_index) t: u32) {
    let n = params.n;
    let per = (n + 255u) / 256u;
    let lo = min(t * per, n);
    let hi = min(lo + per, n);
    if (t == 0u) { atomicStore(&tied, 0u); }
    for (var i = lo; i < hi; i = i + 1u) { a[i] = i; }
    workgroupBarrier();
    for (var p = 0u; p < params.passes; p = p + 1u) {
        let word = bits[p].x;
        let bit = bits[p].y;
        var zeros = 0u;
        for (var i = lo; i < hi; i = i + 1u) {
            if (((keys[a[i]][word] >> bit) & 1u) == 0u) { zeros = zeros + 1u; }
        }
        scan[t] = zeros;
        workgroupBarrier();
        for (var off = 1u; off < 256u; off = off << 1u) {
            var v = scan[t];
            if (t >= off) { v = v + scan[t - off]; }
            workgroupBarrier();
            scan[t] = v;
            workgroupBarrier();
        }
        let total = scan[255];
        var z = scan[t] - zeros;
        var o = total + (lo - z);
        for (var i = lo; i < hi; i = i + 1u) {
            let idx = a[i];
            if (((keys[idx][word] >> bit) & 1u) == 0u) {
                b[z] = idx;
                z = z + 1u;
            } else {
                b[o] = idx;
                o = o + 1u;
            }
        }
        workgroupBarrier();
        for (var i = lo; i < hi; i = i + 1u) { a[i] = b[i]; }
        workgroupBarrier();
    }
    for (var i = lo; i < hi; i = i + 1u) {
        order[i] = a[i];
        if (i > 0u && all(keys[a[i]] == keys[a[i - 1u]])) {
            atomicOr(&tied, 1u);
        }
    }
    workgroupBarrier();
    if (t == 0u) { order[n] = atomicLoad(&tied); }
}
