// Fixed-function stand-in for pre-transformed (D3DFVF_XYZRHW | D3DFVF_DIFFUSE)
// geometry: x/y are window pixels (origin top-left), z is depth in
// [0, D3DZ_MAX] (16777215 for D24S8; to be verified against captures),
// rhw is 1/w. Mirrors what the Xbox does for XYZRHW vertices (the transform
// is skipped; the viewport mapping is inverted here).

struct Viewport {
    size: vec2<f32>,
    inv_z_max: f32, // 1 / D3DZ_MAX
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
    let ndc = vec2<f32>(
        // D3D8 puts pixel centres on integer coordinates (half-pixel offset;
        // to be verified on the Xbox), wgpu on +0.5.
        (in.position.x + 0.5) / viewport.size.x * 2.0 - 1.0,
        1.0 - (in.position.y + 0.5) / viewport.size.y * 2.0,
    );
    // Scaling by w keeps perspective-correct interpolation identical to the
    // original rhw-based interpolation.
    let z = clamp(in.position.z * viewport.inv_z_max, 0.0, 1.0);
    out.clip = vec4<f32>(ndc * w, z * w, w);
    out.color = in.diffuse;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    return in.color;
}
