// A texel in the alias addresses bytes in the guest's surface layout. Pick
// the same subpixel in each contributing source word at display resolution.
struct Params {
    src_width: u32, src_pitch: u32, src_bpp: u32, host_width: u32,
    base: u32, pitch: u32, bpp: u32, signed_flip: u32,
    width: u32, height: u32, depth: u32, slice: u32,
    out_width: u32, out_height: u32, first_y: u32, row_words: u32,
    src_height: u32, host_height: u32, pad1: u32, pad2: u32,
}
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> output: array<u32>;
@group(0) @binding(2) var color: texture_2d<f32>;
@group(0) @binding(3) var depth: texture_depth_2d;
@group(0) @binding(4) var stencil: texture_2d<u32>;

fn source_word(q: vec2u) -> u32 { SOURCE_WORD }

fn read_byte(address: u32, subpixel: vec2u) -> u32 {
    var col = address;
    var texel = vec2u(0u);
    if p.src_pitch != 0u {
        col = address % p.src_pitch;
        texel = vec2u(col / p.src_bpp, address / p.src_pitch);
    } else {
        var index = address / p.src_bpp;
        let dims = vec2u(p.src_width, p.src_height);
        for (var bit = 1u; any(vec2u(bit) < dims); bit <<= 1u) {
            for (var axis = 0u; axis < 2u; axis++) {
                if bit < dims[axis] {
                    texel[axis] |= select(0u, bit, (index & 1u) != 0u);
                    index >>= 1u;
                }
            }
        }
    }
    if texel.x >= p.src_width { return 0u; }
    let guest = vec2u(p.src_width, p.src_height);
    let host = vec2u(p.host_width, p.host_height);
    let lo = (texel * host + guest / 2u) / guest;
    let hi = ((texel + 1u) * host + guest / 2u) / guest;
    let offset = (subpixel * (hi - lo)) >> vec2u(16u);
    let word = source_word(min(lo + offset, host - 1u));
    return (word >> (8u * (col % p.src_bpp))) & 255u;
}

fn read_le(address: u32, n: u32, subpixel: vec2u) -> u32 {
    var value = 0u;
    for (var i = 0u; i < n; i++) {
        value |= read_byte(address + i, subpixel) << (8u * i);
    }
    return value;
}

fn morton(pos: vec3u) -> u32 {
    let dims = vec3u(p.width, p.height, p.depth);
    var index = 0u;
    var shift = 0u;
    for (var bit = 1u; any(vec3u(bit) < dims); bit <<= 1u) {
        for (var axis = 0u; axis < 3u; axis++) {
            if bit < dims[axis] {
                index |= select(0u, 1u << shift, (pos[axis] & bit) != 0u);
                shift++;
            }
        }
    }
    return index;
}

fn e4(v: u32) -> u32 { return (v & 15u) * 17u; }
fn e5(v: u32) -> u32 { let n = v & 31u; return (n << 3u) | (n >> 2u); }
fn e6(v: u32) -> u32 { let n = v & 63u; return (n << 2u) | (n >> 4u); }
fn rgb565(v: u32) -> vec3u { return vec3u(e5(v >> 11u), e6(v >> 5u), e5(v)); }

fn decode_bc(pos: vec3u, sub: vec2u, kind: u32) -> vec4u {
    let blocks = (vec2u(p.width, p.height) + vec2u(3u)) / 4u;
    let block = (pos.z * blocks.y + pos.y / 4u) * blocks.x + pos.x / 4u;
    let base = p.base + block * select(16u, 8u, kind == 1u);
    let ci = base + select(8u, 0u, kind == 1u);
    let i = (pos.y % 4u) * 4u + pos.x % 4u;
    let a = read_le(ci, 2u, sub);
    let b = read_le(ci + 2u, 2u, sub);
    let s = (read_byte(ci + 4u + i / 4u, sub) >> (2u * (i % 4u))) & 3u;
    let ca = rgb565(a);
    let cb = rgb565(b);
    var rgb = select(ca, cb, s == 1u);
    var alpha = 255u;
    if s >= 2u {
        if kind == 1u && a <= b {
            if s == 3u { return vec4u(0u); }
            rgb = (ca + cb) / 2u;
        } else {
            rgb = select((2u * ca + cb) / 3u, (ca + 2u * cb) / 3u, s == 3u);
        }
    }
    if kind == 2u {
        alpha = e4(read_byte(base + i / 2u, sub) >> ((i % 2u) * 4u));
    }
    if kind == 3u {
        let a0 = read_byte(base, sub);
        let a1 = read_byte(base + 1u, sub);
        let bit = i * 3u;
        let n = (read_le(base + 2u + bit / 8u, 2u, sub) >> (bit % 8u)) & 7u;
        if n < 2u { alpha = select(a0, a1, n == 1u); }
        else if a0 > a1 { alpha = ((8u - n) * a0 + (n - 1u) * a1) / 7u; }
        else if n < 6u { alpha = ((6u - n) * a0 + (n - 1u) * a1) / 5u; }
        else { alpha = select(0u, 255u, n == 7u); }
    }
    return vec4u(rgb, alpha);
}

// Conversion consumes tightly packed pairs, including across an odd row.
fn yuv_byte(tight: u32, sub: vec2u) -> u32 {
    let row = p.width * 2u;
    return read_byte(p.base + (tight / row) * p.pitch + tight % row, sub);
}
fn decode_yuv(pos: vec3u, sub: vec2u, uyvy: bool) -> vec4u {
    let i = pos.y * p.width + pos.x;
    let pair = (i & ~1u) * 2u;
    let chroma = select(1u, 0u, uyvy);
    let y = i32(yuv_byte(pair + (i % 2u) * 2u + (1u - chroma), sub)) - 16;
    let u = i32(yuv_byte(pair + chroma, sub)) - 128;
    let v = i32(yuv_byte(pair + 2u + chroma, sub)) - 128;
    let rgb = vec3i(298 * y + 409 * v, 298 * y - 100 * u - 208 * v, 298 * y + 516 * u);
    return vec4u(vec3u(clamp((rgb + vec3i(128)) >> vec3u(8u), vec3i(0), vec3i(255))), 255u);
}

fn decode(pos: vec3u, sub: vec2u) -> vec4u {
    DECODE
}

@compute @workgroup_size(8, 8)
fn main(@builtin(global_invocation_id) id: vec3u) {
    let xy = vec2u(id.x, p.first_y + id.y);
    if xy.x >= p.out_width || xy.y >= p.out_height || id.y >= arrayLength(&output) / p.row_words { return; }
    let dims = vec2u(p.out_width, p.out_height);
    let guest = vec2u(p.width, p.height);
    let cell = ((2u * xy + 1u) * guest - 1u) / (2u * dims);
    let lo = (cell * dims + guest / 2u) / guest;
    let hi = ((cell + 1u) * dims + guest / 2u) / guest;
    // The centre within this guest cell in 16.16 fixed point. Each byte
    // contributing to an alias texel can address a different source cell.
    let sub = ((xy - lo) * 65536u + 32768u) / max(hi - lo, vec2u(1u));
    let c = decode(vec3u(cell, p.slice), sub);
    output[id.y * p.row_words + id.x] = (c.r | (c.g << 8u) | (c.b << 16u) | (c.a << 24u)) ^ p.signed_flip;
}
