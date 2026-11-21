// ---- project3d linear d24 greater
struct XboxVaryings {
    @builtin(position) @invariant position: vec4f,
    @location(0) d0: vec4f,
    @location(1) d1: vec4f,
    @location(2) b0: vec4f,
    @location(3) b1: vec4f,
    @location(4) t0: vec4f,
    @location(5) t1: vec4f,
    @location(6) t2: vec4f,
    @location(7) t3: vec4f,
    @location(8) fog: f32,
}

struct XpsUniforms {
    // Combiner constants per stage; index 8 is the final combiner's.
    c0: array<vec4f, 9>,
    c1: array<vec4f, 9>,
    fog_color: vec4f,
    // The NV2A fog unit (psh::fog): x: 0 = off, 1 linear, 2 exp, 3 exp2,
    // 4..6 the same on |coordinate|; y: bias, z: scale (SET_FOG_PARAMS).
    fog: vec4f,
    // D3DTSS_BUMPENVMAT00, 01, 10, 11 per stage.
    bump_mat: array<vec4f, 4>,
    // x: D3DTSS_BUMPENVLSCALE, y: D3DTSS_BUMPENVLOFFSET per stage.
    bump_lum: array<vec4f, 4>,
    // xy multiplies 2D coordinates (texel-addressed linear textures).
    tex_scale: array<vec4f, 4>,
    // Eye vector for DOT_RFLCT_SPEC_CONST.
    eye: vec4f,
    // x: alpha reference, 0..255.
    alpha_ref: vec4f,
    // The texture unit's border value per stage (PixelOptions::border).
    border: array<vec4f, 4>,
    lod_bias: vec4f,
    stipple: array<vec4u, 8>,
    // W-buffering (PixelOptions::w_buffer): x the depth full scale, y the
    // polygon offset in depth units, z its slope factor, w 1 for an
    // integer depth format (the stored w is truncated, as the NV2A does).
    depth: vec4f,
    // x, y: the range the stored w is clamped to (CLIP_MIN, CLIP_MAX).
    depth_range: vec2f,
    stipple_scale: vec2u,
}
@group(0) @binding(1) var<uniform> xps: XpsUniforms;

struct XpsInputs {
    d0: vec4f, d1: vec4f, b0: vec4f, b1: vec4f,
    t0: vec4f, t1: vec4f, t2: vec4f, t3: vec4f,
    fog: f32,
    front: bool,
}
struct XpsResult { color: vec4f, kill: bool, }

// Dot-product input mappings, for unsigned 8-bit texture data.
fn xps_sign_d3d(x: vec3f) -> vec3f { return (x * 255.0 - 128.0) / 127.0; }
fn xps_sign_gl(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select((b + 0.5) / 127.5, (b - 255.5) / 127.5, b >= vec3f(128.0));
}
fn xps_sign_nv(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select(b / 127.0, (b - 256.0) / 127.0, b >= vec3f(128.0));
}
// COLORSIGN channels: the host texture holds the signed byte s as s + 128
// (so filtering is linear in s); the texture unit delivers 2s/255.
fn xps_signed(v: vec4f, m: vec4<bool>) -> vec4f {
    return select(v, max(2.0 * v - 256.0 / 255.0, vec4f(-1.0)), m);
}
// Outside [0, 1] on one of the axes in m (PixelOptions::border).
fn xps_outside(c: vec3f, m: vec3<bool>) -> bool {
    return any(m & ((c < vec3f(0.0)) | (c > vec3f(1.0))));
}
fn xps_mux_lsb(a: f32) -> bool { return (u32(a * 255.0) & 1u) == 1u; }

fn xps_fog_special(code: u32) -> f32 {
    return select(1.0, 0.0, code == 3u || code == 5u || code == 6u);
}
fn xps_fog_factor(coord: f32) -> f32 {
    let code = u32(xps.fog.x);
    let bias = xps.fog.y;
    let scale = xps.fog.z;
    if (code == 0u) { return 1.0; }
    if ((bitcast<u32>(coord) & 0x7fffffffu) == 0x7f800000u) { return xps_fog_special(code); }
    let d = select(coord, abs(coord), code >= 4u);
    var f = 1.0;
    if (code == 1u || code == 4u) { f = bias + scale * d - 1.0; }
    if (code == 2u || code == 5u) { f = bias + exp2(16.0 * scale * d) - 1.5; }
    if (code == 3u || code == 6u) { let s = scale * d; f = bias + exp2(-32.0 * s * s) - 1.5; }
    if ((bitcast<u32>(f) & 0x7fffffffu) > 0x7f800000u) { return xps_fog_special(code); }
    return clamp(f, 0.0, 1.0);
}

fn xps_hilo(c: vec4f) -> vec2f {
    let b = round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0);
    return vec2f(b.a * 256.0 + b.r, b.g * 256.0 + b.b);
}
fn xps_hilo_twos(h: vec2f) -> vec2f { return select(h, h - 65536.0, h >= vec2f(32768.0)); }
fn xps_hemisphere(v: vec2f) -> vec3f { return vec3f(v, sqrt(max(1.0 - dot(v, v), 0.0))); }
fn xps_hilo_1(c: vec4f) -> vec3f { return vec3f(xps_hilo(c) / 65535.0, 1.0); }
fn xps_hilo_hemi_d3d(c: vec4f) -> vec3f { return xps_hemisphere((xps_hilo(c) - 32768.0) / 32767.0); }
fn xps_hilo_hemi_gl(c: vec4f) -> vec3f { return xps_hemisphere((2.0 * xps_hilo_twos(xps_hilo(c)) + 1.0) / 65535.0); }
fn xps_hilo_hemi_nv(c: vec4f) -> vec3f { return xps_hemisphere(xps_hilo_twos(xps_hilo(c)) / 32767.0); }

// Shadow maps: raw-bit RGBA8 texels back to the stored depth, and texel
// addressing (0 wrap, 1 mirror, 2 clamp).
fn xps_shadow_bytes(c: vec4f) -> vec4u { return vec4u(round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0)); }
fn xps_shadow_d24(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 16u) | (b.y << 8u) | b.z); }
fn xps_shadow_d16(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 8u) | b.y); }
fn xps_shadow_f24(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    return bitcast<f32>(((b.x << 16u) | (b.y << 8u) | b.z) << 7u);
}
fn xps_shadow_f16(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    let d = (b.x << 8u) | b.y;
    let e = d >> 12u;
    let m = d & 0xFFFu;
    if (d == 0u) { return 0.0; }
    return bitcast<f32>(((e + 120u) << 23u) | (m << 11u));
}
fn xps_shadow_addr(i: i32, n: i32, mode: u32) -> i32 {
    if (mode == 0u) { return ((i % n) + n) % n; }
    if (mode == 1u) {
        let p = 2 * n;
        let m = ((i % p) + p) % p;
        return select(m, p - 1 - m, m >= n);
    }
    return clamp(i, 0, n - 1);
}
@group(1) @binding(0) var xps_tex0: texture_2d<f32>;
@group(1) @binding(1) var xps_samp0: sampler;

fn xps_program(i: XpsInputs) -> XpsResult {
    var kill = false;
    var v0 = clamp(i.d0, vec4f(0.0), vec4f(1.0));
    var v1 = clamp(i.d1, vec4f(0.0), vec4f(1.0));
    let fogr = vec4f(xps.fog_color.rgb, xps_fog_factor(i.fog));
    var t0 = vec4f(0.0, 0.0, 0.0, 1.0);
    var t1 = t0;
    var t2 = t0;
    var t3 = t0;
    var dot1 = 0.0;
    var dot2 = 0.0;
    var dot3 = 0.0;
    // texture stage 0: project3d
    {
        let uv = (i.t0.xy / i.t0.w) * xps.tex_scale[0].xy;
        let r = clamp(i.t0.z / i.t0.w, 0.0, 16777215.0);
        let size = vec2i(textureDimensions(xps_tex0, 0));
        let texel = vec2i(floor(uv * vec2f(size)));
        let x = xps_shadow_addr(texel.x, size.x, 2u);
        let y = xps_shadow_addr(texel.y, size.y, 2u);
        t0 = vec4f(select(0.0, 1.0, xps_shadow_d24(textureLoad(xps_tex0, vec2i(x, y), 0)) > r));
    }
    // texture stage 1: none
    // texture stage 2: none
    // texture stage 3: none
    var r0 = vec4f(0.0, 0.0, 0.0, t0.a);
    var r1 = vec4f(0.0);
    // final combiner
    var color: vec4f;
    {
        let fc_sum = vec4f(clamp(v1.rgb, vec3f(0.0), vec3f(1.0)) + clamp(r0.rgb, vec3f(0.0), vec3f(1.0)), 0.0);
        let fc_prod = vec4f(max(vec4f(0.0).rgb, vec3f(0.0)) * max(vec4f(0.0).rgb, vec3f(0.0)), 0.0);
        let a = max(vec4f(0.0).rgb, vec3f(0.0));
        let b = max(vec4f(0.0).rgb, vec3f(0.0));
        let c = max(vec4f(0.0).rgb, vec3f(0.0));
        let d = max(t0.rgb, vec3f(0.0));
        color = vec4f(clamp(a * b + (1.0 - a) * c + d, vec3f(0.0), vec3f(1.0)), clamp(max(t0.a, f32(0.0)), 0.0, 1.0));
    }
    return XpsResult(color, kill);
}

@fragment
fn fs_main(v: XboxVaryings, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let r = xps_program(XpsInputs(v.d0, v.d1, v.b0, v.b1, v.t0, v.t1, v.t2, v.t3, v.fog, front));
    return r.color;
}

// ---- project2d point d16 less
struct XboxVaryings {
    @builtin(position) @invariant position: vec4f,
    @location(0) d0: vec4f,
    @location(1) d1: vec4f,
    @location(2) b0: vec4f,
    @location(3) b1: vec4f,
    @location(4) t0: vec4f,
    @location(5) t1: vec4f,
    @location(6) t2: vec4f,
    @location(7) t3: vec4f,
    @location(8) fog: f32,
}

struct XpsUniforms {
    // Combiner constants per stage; index 8 is the final combiner's.
    c0: array<vec4f, 9>,
    c1: array<vec4f, 9>,
    fog_color: vec4f,
    // The NV2A fog unit (psh::fog): x: 0 = off, 1 linear, 2 exp, 3 exp2,
    // 4..6 the same on |coordinate|; y: bias, z: scale (SET_FOG_PARAMS).
    fog: vec4f,
    // D3DTSS_BUMPENVMAT00, 01, 10, 11 per stage.
    bump_mat: array<vec4f, 4>,
    // x: D3DTSS_BUMPENVLSCALE, y: D3DTSS_BUMPENVLOFFSET per stage.
    bump_lum: array<vec4f, 4>,
    // xy multiplies 2D coordinates (texel-addressed linear textures).
    tex_scale: array<vec4f, 4>,
    // Eye vector for DOT_RFLCT_SPEC_CONST.
    eye: vec4f,
    // x: alpha reference, 0..255.
    alpha_ref: vec4f,
    // The texture unit's border value per stage (PixelOptions::border).
    border: array<vec4f, 4>,
    lod_bias: vec4f,
    stipple: array<vec4u, 8>,
    // W-buffering (PixelOptions::w_buffer): x the depth full scale, y the
    // polygon offset in depth units, z its slope factor, w 1 for an
    // integer depth format (the stored w is truncated, as the NV2A does).
    depth: vec4f,
    // x, y: the range the stored w is clamped to (CLIP_MIN, CLIP_MAX).
    depth_range: vec2f,
    stipple_scale: vec2u,
}
@group(0) @binding(1) var<uniform> xps: XpsUniforms;

struct XpsInputs {
    d0: vec4f, d1: vec4f, b0: vec4f, b1: vec4f,
    t0: vec4f, t1: vec4f, t2: vec4f, t3: vec4f,
    fog: f32,
    front: bool,
}
struct XpsResult { color: vec4f, kill: bool, }

// Dot-product input mappings, for unsigned 8-bit texture data.
fn xps_sign_d3d(x: vec3f) -> vec3f { return (x * 255.0 - 128.0) / 127.0; }
fn xps_sign_gl(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select((b + 0.5) / 127.5, (b - 255.5) / 127.5, b >= vec3f(128.0));
}
fn xps_sign_nv(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select(b / 127.0, (b - 256.0) / 127.0, b >= vec3f(128.0));
}
// COLORSIGN channels: the host texture holds the signed byte s as s + 128
// (so filtering is linear in s); the texture unit delivers 2s/255.
fn xps_signed(v: vec4f, m: vec4<bool>) -> vec4f {
    return select(v, max(2.0 * v - 256.0 / 255.0, vec4f(-1.0)), m);
}
// Outside [0, 1] on one of the axes in m (PixelOptions::border).
fn xps_outside(c: vec3f, m: vec3<bool>) -> bool {
    return any(m & ((c < vec3f(0.0)) | (c > vec3f(1.0))));
}
fn xps_mux_lsb(a: f32) -> bool { return (u32(a * 255.0) & 1u) == 1u; }

fn xps_fog_special(code: u32) -> f32 {
    return select(1.0, 0.0, code == 3u || code == 5u || code == 6u);
}
fn xps_fog_factor(coord: f32) -> f32 {
    let code = u32(xps.fog.x);
    let bias = xps.fog.y;
    let scale = xps.fog.z;
    if (code == 0u) { return 1.0; }
    if ((bitcast<u32>(coord) & 0x7fffffffu) == 0x7f800000u) { return xps_fog_special(code); }
    let d = select(coord, abs(coord), code >= 4u);
    var f = 1.0;
    if (code == 1u || code == 4u) { f = bias + scale * d - 1.0; }
    if (code == 2u || code == 5u) { f = bias + exp2(16.0 * scale * d) - 1.5; }
    if (code == 3u || code == 6u) { let s = scale * d; f = bias + exp2(-32.0 * s * s) - 1.5; }
    if ((bitcast<u32>(f) & 0x7fffffffu) > 0x7f800000u) { return xps_fog_special(code); }
    return clamp(f, 0.0, 1.0);
}

fn xps_hilo(c: vec4f) -> vec2f {
    let b = round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0);
    return vec2f(b.a * 256.0 + b.r, b.g * 256.0 + b.b);
}
fn xps_hilo_twos(h: vec2f) -> vec2f { return select(h, h - 65536.0, h >= vec2f(32768.0)); }
fn xps_hemisphere(v: vec2f) -> vec3f { return vec3f(v, sqrt(max(1.0 - dot(v, v), 0.0))); }
fn xps_hilo_1(c: vec4f) -> vec3f { return vec3f(xps_hilo(c) / 65535.0, 1.0); }
fn xps_hilo_hemi_d3d(c: vec4f) -> vec3f { return xps_hemisphere((xps_hilo(c) - 32768.0) / 32767.0); }
fn xps_hilo_hemi_gl(c: vec4f) -> vec3f { return xps_hemisphere((2.0 * xps_hilo_twos(xps_hilo(c)) + 1.0) / 65535.0); }
fn xps_hilo_hemi_nv(c: vec4f) -> vec3f { return xps_hemisphere(xps_hilo_twos(xps_hilo(c)) / 32767.0); }

// Shadow maps: raw-bit RGBA8 texels back to the stored depth, and texel
// addressing (0 wrap, 1 mirror, 2 clamp).
fn xps_shadow_bytes(c: vec4f) -> vec4u { return vec4u(round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0)); }
fn xps_shadow_d24(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 16u) | (b.y << 8u) | b.z); }
fn xps_shadow_d16(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 8u) | b.y); }
fn xps_shadow_f24(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    return bitcast<f32>(((b.x << 16u) | (b.y << 8u) | b.z) << 7u);
}
fn xps_shadow_f16(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    let d = (b.x << 8u) | b.y;
    let e = d >> 12u;
    let m = d & 0xFFFu;
    if (d == 0u) { return 0.0; }
    return bitcast<f32>(((e + 120u) << 23u) | (m << 11u));
}
fn xps_shadow_addr(i: i32, n: i32, mode: u32) -> i32 {
    if (mode == 0u) { return ((i % n) + n) % n; }
    if (mode == 1u) {
        let p = 2 * n;
        let m = ((i % p) + p) % p;
        return select(m, p - 1 - m, m >= n);
    }
    return clamp(i, 0, n - 1);
}
@group(1) @binding(0) var xps_tex0: texture_2d<f32>;
@group(1) @binding(1) var xps_samp0: sampler;

fn xps_program(i: XpsInputs) -> XpsResult {
    var kill = false;
    var v0 = clamp(i.d0, vec4f(0.0), vec4f(1.0));
    var v1 = clamp(i.d1, vec4f(0.0), vec4f(1.0));
    let fogr = vec4f(xps.fog_color.rgb, xps_fog_factor(i.fog));
    var t0 = vec4f(0.0, 0.0, 0.0, 1.0);
    var t1 = t0;
    var t2 = t0;
    var t3 = t0;
    var dot1 = 0.0;
    var dot2 = 0.0;
    var dot3 = 0.0;
    // texture stage 0: project2d
    {
        let uv = (i.t0.xy / i.t0.w) * xps.tex_scale[0].xy;
        let r = 0.0;
        let size = vec2i(textureDimensions(xps_tex0, 0));
        let texel = vec2i(floor(uv * vec2f(size)));
        let x = xps_shadow_addr(texel.x, size.x, 2u);
        let y = xps_shadow_addr(texel.y, size.y, 2u);
        t0 = vec4f(select(0.0, 1.0, xps_shadow_d16(textureLoad(xps_tex0, vec2i(x, y), 0)) < r));
    }
    // texture stage 1: none
    // texture stage 2: none
    // texture stage 3: none
    var r0 = vec4f(0.0, 0.0, 0.0, t0.a);
    var r1 = vec4f(0.0);
    // final combiner
    var color: vec4f;
    {
        let fc_sum = vec4f(clamp(v1.rgb, vec3f(0.0), vec3f(1.0)) + clamp(r0.rgb, vec3f(0.0), vec3f(1.0)), 0.0);
        let fc_prod = vec4f(max(vec4f(0.0).rgb, vec3f(0.0)) * max(vec4f(0.0).rgb, vec3f(0.0)), 0.0);
        let a = max(vec4f(0.0).rgb, vec3f(0.0));
        let b = max(vec4f(0.0).rgb, vec3f(0.0));
        let c = max(vec4f(0.0).rgb, vec3f(0.0));
        let d = max(t0.rgb, vec3f(0.0));
        color = vec4f(clamp(a * b + (1.0 - a) * c + d, vec3f(0.0), vec3f(1.0)), clamp(max(t0.a, f32(0.0)), 0.0, 1.0));
    }
    return XpsResult(color, kill);
}

@fragment
fn fs_main(v: XboxVaryings, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let r = xps_program(XpsInputs(v.d0, v.d1, v.b0, v.b1, v.t0, v.t1, v.t2, v.t3, v.fog, front));
    return r.color;
}

// ---- two stages, one map
struct XboxVaryings {
    @builtin(position) @invariant position: vec4f,
    @location(0) d0: vec4f,
    @location(1) d1: vec4f,
    @location(2) b0: vec4f,
    @location(3) b1: vec4f,
    @location(4) t0: vec4f,
    @location(5) t1: vec4f,
    @location(6) t2: vec4f,
    @location(7) t3: vec4f,
    @location(8) fog: f32,
}

struct XpsUniforms {
    // Combiner constants per stage; index 8 is the final combiner's.
    c0: array<vec4f, 9>,
    c1: array<vec4f, 9>,
    fog_color: vec4f,
    // The NV2A fog unit (psh::fog): x: 0 = off, 1 linear, 2 exp, 3 exp2,
    // 4..6 the same on |coordinate|; y: bias, z: scale (SET_FOG_PARAMS).
    fog: vec4f,
    // D3DTSS_BUMPENVMAT00, 01, 10, 11 per stage.
    bump_mat: array<vec4f, 4>,
    // x: D3DTSS_BUMPENVLSCALE, y: D3DTSS_BUMPENVLOFFSET per stage.
    bump_lum: array<vec4f, 4>,
    // xy multiplies 2D coordinates (texel-addressed linear textures).
    tex_scale: array<vec4f, 4>,
    // Eye vector for DOT_RFLCT_SPEC_CONST.
    eye: vec4f,
    // x: alpha reference, 0..255.
    alpha_ref: vec4f,
    // The texture unit's border value per stage (PixelOptions::border).
    border: array<vec4f, 4>,
    lod_bias: vec4f,
    stipple: array<vec4u, 8>,
    // W-buffering (PixelOptions::w_buffer): x the depth full scale, y the
    // polygon offset in depth units, z its slope factor, w 1 for an
    // integer depth format (the stored w is truncated, as the NV2A does).
    depth: vec4f,
    // x, y: the range the stored w is clamped to (CLIP_MIN, CLIP_MAX).
    depth_range: vec2f,
    stipple_scale: vec2u,
}
@group(0) @binding(1) var<uniform> xps: XpsUniforms;

struct XpsInputs {
    d0: vec4f, d1: vec4f, b0: vec4f, b1: vec4f,
    t0: vec4f, t1: vec4f, t2: vec4f, t3: vec4f,
    fog: f32,
    front: bool,
}
struct XpsResult { color: vec4f, kill: bool, }

// Dot-product input mappings, for unsigned 8-bit texture data.
fn xps_sign_d3d(x: vec3f) -> vec3f { return (x * 255.0 - 128.0) / 127.0; }
fn xps_sign_gl(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select((b + 0.5) / 127.5, (b - 255.5) / 127.5, b >= vec3f(128.0));
}
fn xps_sign_nv(x: vec3f) -> vec3f {
    let b = x * 255.0;
    return select(b / 127.0, (b - 256.0) / 127.0, b >= vec3f(128.0));
}
// COLORSIGN channels: the host texture holds the signed byte s as s + 128
// (so filtering is linear in s); the texture unit delivers 2s/255.
fn xps_signed(v: vec4f, m: vec4<bool>) -> vec4f {
    return select(v, max(2.0 * v - 256.0 / 255.0, vec4f(-1.0)), m);
}
// Outside [0, 1] on one of the axes in m (PixelOptions::border).
fn xps_outside(c: vec3f, m: vec3<bool>) -> bool {
    return any(m & ((c < vec3f(0.0)) | (c > vec3f(1.0))));
}
fn xps_mux_lsb(a: f32) -> bool { return (u32(a * 255.0) & 1u) == 1u; }

fn xps_fog_special(code: u32) -> f32 {
    return select(1.0, 0.0, code == 3u || code == 5u || code == 6u);
}
fn xps_fog_factor(coord: f32) -> f32 {
    let code = u32(xps.fog.x);
    let bias = xps.fog.y;
    let scale = xps.fog.z;
    if (code == 0u) { return 1.0; }
    if ((bitcast<u32>(coord) & 0x7fffffffu) == 0x7f800000u) { return xps_fog_special(code); }
    let d = select(coord, abs(coord), code >= 4u);
    var f = 1.0;
    if (code == 1u || code == 4u) { f = bias + scale * d - 1.0; }
    if (code == 2u || code == 5u) { f = bias + exp2(16.0 * scale * d) - 1.5; }
    if (code == 3u || code == 6u) { let s = scale * d; f = bias + exp2(-32.0 * s * s) - 1.5; }
    if ((bitcast<u32>(f) & 0x7fffffffu) > 0x7f800000u) { return xps_fog_special(code); }
    return clamp(f, 0.0, 1.0);
}

fn xps_hilo(c: vec4f) -> vec2f {
    let b = round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0);
    return vec2f(b.a * 256.0 + b.r, b.g * 256.0 + b.b);
}
fn xps_hilo_twos(h: vec2f) -> vec2f { return select(h, h - 65536.0, h >= vec2f(32768.0)); }
fn xps_hemisphere(v: vec2f) -> vec3f { return vec3f(v, sqrt(max(1.0 - dot(v, v), 0.0))); }
fn xps_hilo_1(c: vec4f) -> vec3f { return vec3f(xps_hilo(c) / 65535.0, 1.0); }
fn xps_hilo_hemi_d3d(c: vec4f) -> vec3f { return xps_hemisphere((xps_hilo(c) - 32768.0) / 32767.0); }
fn xps_hilo_hemi_gl(c: vec4f) -> vec3f { return xps_hemisphere((2.0 * xps_hilo_twos(xps_hilo(c)) + 1.0) / 65535.0); }
fn xps_hilo_hemi_nv(c: vec4f) -> vec3f { return xps_hemisphere(xps_hilo_twos(xps_hilo(c)) / 32767.0); }

// Shadow maps: raw-bit RGBA8 texels back to the stored depth, and texel
// addressing (0 wrap, 1 mirror, 2 clamp).
fn xps_shadow_bytes(c: vec4f) -> vec4u { return vec4u(round(clamp(c, vec4f(0.0), vec4f(1.0)) * 255.0)); }
fn xps_shadow_d24(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 16u) | (b.y << 8u) | b.z); }
fn xps_shadow_d16(c: vec4f) -> f32 { let b = xps_shadow_bytes(c); return f32((b.x << 8u) | b.y); }
fn xps_shadow_f24(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    return bitcast<f32>(((b.x << 16u) | (b.y << 8u) | b.z) << 7u);
}
fn xps_shadow_f16(c: vec4f) -> f32 {
    let b = xps_shadow_bytes(c);
    let d = (b.x << 8u) | b.y;
    let e = d >> 12u;
    let m = d & 0xFFFu;
    if (d == 0u) { return 0.0; }
    return bitcast<f32>(((e + 120u) << 23u) | (m << 11u));
}
fn xps_shadow_addr(i: i32, n: i32, mode: u32) -> i32 {
    if (mode == 0u) { return ((i % n) + n) % n; }
    if (mode == 1u) {
        let p = 2 * n;
        let m = ((i % p) + p) % p;
        return select(m, p - 1 - m, m >= n);
    }
    return clamp(i, 0, n - 1);
}
@group(1) @binding(0) var xps_tex0: texture_2d<f32>;
@group(1) @binding(1) var xps_samp0: sampler;
@group(1) @binding(2) var xps_tex1: texture_2d<f32>;
@group(1) @binding(3) var xps_samp1: sampler;

fn xps_program(i: XpsInputs) -> XpsResult {
    var kill = false;
    var v0 = clamp(i.d0, vec4f(0.0), vec4f(1.0));
    var v1 = clamp(i.d1, vec4f(0.0), vec4f(1.0));
    let fogr = vec4f(xps.fog_color.rgb, xps_fog_factor(i.fog));
    var t0 = vec4f(0.0, 0.0, 0.0, 1.0);
    var t1 = t0;
    var t2 = t0;
    var t3 = t0;
    var dot1 = 0.0;
    var dot2 = 0.0;
    var dot3 = 0.0;
    // texture stage 0: project3d
    {
        let uv = ((i.t0.xy / i.t0.w) * xps.tex_scale[0].xy + (i.t1.xy / i.t1.w) * xps.tex_scale[1].xy) / 2.0;
        let r = clamp(i.t0.z / i.t0.w, 0.0, 16777215.0);
        let size = vec2i(textureDimensions(xps_tex0, 0));
        let texel = vec2i(floor(uv * vec2f(size)));
        let x = xps_shadow_addr(texel.x, size.x, 2u);
        let y = xps_shadow_addr(texel.y, size.y, 2u);
        t0 = vec4f(select(0.0, 1.0, xps_shadow_d24(textureLoad(xps_tex0, vec2i(x, y), 0)) > r));
    }
    // texture stage 1: project3d
    {
        let uv = ((i.t0.xy / i.t0.w) * xps.tex_scale[0].xy + (i.t1.xy / i.t1.w) * xps.tex_scale[1].xy) / 2.0;
        let r = clamp(i.t1.z / i.t1.w, 0.0, 16777215.0);
        let size = vec2i(textureDimensions(xps_tex1, 0));
        let texel = vec2i(floor(uv * vec2f(size)));
        let x = xps_shadow_addr(texel.x, size.x, 2u);
        let y = xps_shadow_addr(texel.y, size.y, 2u);
        t1 = vec4f(select(0.0, 1.0, xps_shadow_d24(textureLoad(xps_tex1, vec2i(x, y), 0)) > r));
    }
    // texture stage 2: none
    // texture stage 3: none
    var r0 = vec4f(0.0, 0.0, 0.0, t0.a);
    var r1 = vec4f(0.0);
    // final combiner
    var color: vec4f;
    {
        let fc_sum = vec4f(clamp(v1.rgb, vec3f(0.0), vec3f(1.0)) + clamp(r0.rgb, vec3f(0.0), vec3f(1.0)), 0.0);
        let fc_prod = vec4f(max(vec4f(0.0).rgb, vec3f(0.0)) * max(vec4f(0.0).rgb, vec3f(0.0)), 0.0);
        let a = max(vec4f(0.0).rgb, vec3f(0.0));
        let b = max(vec4f(0.0).rgb, vec3f(0.0));
        let c = max(vec4f(0.0).rgb, vec3f(0.0));
        let d = max(t1.rgb, vec3f(0.0));
        color = vec4f(clamp(a * b + (1.0 - a) * c + d, vec3f(0.0), vec3f(1.0)), clamp(max(t1.a, f32(0.0)), 0.0, 1.0));
    }
    return XpsResult(color, kill);
}

@fragment
fn fs_main(v: XboxVaryings, @builtin(front_facing) front: bool) -> @location(0) vec4f {
    let r = xps_program(XpsInputs(v.d0, v.d1, v.b0, v.b1, v.t0, v.t1, v.t2, v.t3, v.fog, front));
    return r.color;
}

