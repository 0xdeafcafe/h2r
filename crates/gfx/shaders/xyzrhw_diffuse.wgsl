// Fixed-function stand-in for pre-transformed (D3DFVF_XYZRHW | D3DFVF_DIFFUSE)
// geometry: x/y are window pixels (origin top-left), z is depth in
// [0, D3DZ_MAX] (the depth format's full scale: 16777215 for D24S8, 65535
// for D16, 511.9375 for F16, 3.4027977e38 for F24S8),
// rhw is 1/w. Mirrors what the Xbox does for XYZRHW vertices (the transform
// is skipped; the viewport mapping is inverted here).

// One slot per draw (dynamic offset).
struct Viewport {
    size: vec2<f32>,
    z_max: f32, // D3DZ_MAX (divided by: 1 / F24's is not a normal f32)
    // Alpha test: 0 off, 1 never, 2 less, 3 equal, 4 less-equal, 5 greater,
    // 6 not-equal, 7 greater-equal (D3DCMPFUNC order).
    alpha_func: f32,
    // D3DRS_ALPHAREF, 0..255.
    alpha_ref: f32,
    // Pixels of the window taken off x and y: 0 unless the target is a
    // multiple of the window (see `Draw::window` in renderer.rs).
    shift_x: f32,
    shift_y: f32,
    pad: f32,
};

@group(0) @binding(0) var<uniform> viewport: Viewport;

struct VsIn {
    @location(0) position: vec4<f32>, // D3DVSDE_POSITION
    @location(3) diffuse: vec4<f32>,  // D3DVSDE_DIFFUSE (D3DCOLOR, swizzled by the vertex format)
};

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) color: vec4<f32>,
};

@vertex
fn vs_main(in: VsIn) -> VsOut {
    var out: VsOut;
    // A zero rhw is legal in D3D8 and behaves as 1.
    var rhw = in.position.w;
    if (rhw == 0.0) {
        rhw = 1.0;
    }
    let w = 1.0 / rhw;
    // D3D8 puts pixel centres on integer coordinates (half-pixel offset;
    // to be verified on the Xbox), wgpu on +0.5.
    let t = in.position.xy + 0.5;
    let shift = vec2<f32>(viewport.shift_x, viewport.shift_y);
    // Scaled, a position past the last pixel centre stays at least on the
    // window's far edge, as at 1x, where it covers the last pixel as the
    // edge does: the shift would leave the last host columns and rows out
    // of a quad ending on that edge (see `Draw::window` in renderer.rs).
    let far = (shift > vec2<f32>(0.0)) & (t > viewport.size - 0.5);
    let xy = select(t - shift, max(t - shift, viewport.size), far);
    let ndc = vec2<f32>(
        xy.x / viewport.size.x * 2.0 - 1.0,
        1.0 - xy.y / viewport.size.y * 2.0,
    );
    // Scaling by w keeps perspective-correct interpolation identical to the
    // original rhw-based interpolation.
    let z = clamp(in.position.z / viewport.z_max, 0.0, 1.0);
    out.clip = vec4<f32>(ndc * w, z * w, w);
    out.color = in.diffuse;
    return out;
}

// D3D compares the 8-bit alpha with the reference.
fn alpha_passes(a: f32) -> bool {
    let v = round(clamp(a, 0.0, 1.0) * 255.0);
    let r = viewport.alpha_ref;
    switch u32(viewport.alpha_func) {
        case 1u: { return false; }
        case 2u: { return v < r; }
        case 3u: { return v == r; }
        case 4u: { return v <= r; }
        case 5u: { return v > r; }
        case 6u: { return v != r; }
        case 7u: { return v >= r; }
        default: { return true; }
    }
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    if (!alpha_passes(in.color.a)) {
        discard;
    }
    return in.color;
}
