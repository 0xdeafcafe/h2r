// One model shown as a menu picture (crate::showcase, crate::showcase_gpu):
// the model at rest, turned about z, lit by a soft key, a cool fill and a
// rim, its base map's brightness tinted, with a glyph decal pressed into
// the forehead. A silhouette is a near-black fill and a faint rim only.
// The output is premultiplied: the background stays transparent.

struct Frame {
    view_proj: mat4x4<f32>,
    model: mat4x4<f32>,
    // xyz the camera (turned space), w 1 for a silhouette.
    eye: vec4<f32>,
    // rgb the tint, w 1 when the decal is drawn.
    tint: vec4<f32>,
    // xyz the decal's centre (model space), w its radius.
    decal: vec4<f32>,
    // x the front cut, y the facing threshold, z silhouette fill, w its rim.
    cut: vec4<f32>,
};

@group(0) @binding(0) var<uniform> frame: Frame;
@group(0) @binding(1) var base_map: texture_2d<f32>;
@group(0) @binding(2) var linear: sampler;
@group(0) @binding(3) var glyph: texture_2d<f32>;

struct V {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Turned space, for the light.
    @location(1) normal: vec3<f32>,
    @location(2) world: vec3<f32>,
    // Model space, for the decal.
    @location(3) rest: vec3<f32>,
    @location(4) rest_normal: vec3<f32>,
};

@vertex
fn vs_main(
    @location(0) position: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) normal: vec3<f32>,
) -> V {
    let world = frame.model * vec4<f32>(position, 1.0);
    var out: V;
    out.position = frame.view_proj * world;
    out.uv = uv;
    out.normal = (frame.model * vec4<f32>(normal, 0.0)).xyz;
    out.world = world.xyz;
    out.rest = position;
    out.rest_normal = normal;
    return out;
}

// The glyph's coverage at model-space point `p`, 0 off the forehead.
fn decal(p: vec3<f32>, n: vec3<f32>, shift: vec2<f32>) -> f32 {
    let r = frame.decal.w;
    let uv = vec2<f32>(
        (p.y - frame.decal.y) / (2.0 * r) + 0.5,
        0.5 - (p.z - frame.decal.z) / (2.0 * r),
    ) + shift;
    let inside = all(uv >= vec2<f32>(0.0)) && all(uv <= vec2<f32>(1.0));
    let len = length(n);
    let facing = select(0.0, n.x / len, len > 1e-6);
    if (!inside || p.x < frame.cut.x || facing < frame.cut.y) {
        return 0.0;
    }
    // Fade the last stretch of facing so the edge of the decal is soft.
    let soft = smoothstep(frame.cut.y, frame.cut.y + 0.15, facing);
    return textureSampleLevel(glyph, linear, uv, 0.0).r * soft;
}

@fragment
fn fs_main(v: V) -> @location(0) vec4<f32> {
    let len = length(v.normal);
    let to_eye = normalize(frame.eye.xyz - v.world);
    let outward = select(vec3<f32>(0.0, 0.0, 1.0), v.normal / len, len > 1e-6);
    // Two-sided: a surface seen from behind its normal (the inside of the
    // open cranium, its rim) is lit as its front. Lit one-sided it took
    // only the ambient and a full rim, a flat blue-grey.
    let n = select(outward, -outward, dot(outward, to_eye) < 0.0);
    let facing = max(dot(n, to_eye), 0.0);
    let rim = pow(1.0 - facing, 2.5);

    if (frame.eye.w > 0.5) {
        // Not collected: the shape only.
        let c = vec3<f32>(frame.cut.z) + frame.cut.w * rim * vec3<f32>(0.85, 0.9, 1.0);
        return vec4<f32>(c, 1.0);
    }

    let base = textureSample(base_map, linear, v.uv).rgb;
    let bright = dot(base, vec3<f32>(0.299, 0.587, 0.114));
    let albedo = frame.tint.rgb * (0.35 + 0.65 * bright);

    // Key from above, the camera's left and in front; a cool fill from
    // the right; a warm bounce from below.
    let key_dir = normalize(vec3<f32>(0.75, -0.55, 0.65));
    let fill_dir = normalize(vec3<f32>(0.6, 0.8, 0.1));
    let key = max(dot(n, key_dir) * 0.8 + 0.2, 0.0);
    let fill = max(dot(n, fill_dir), 0.0);
    let bounce = max(-n.z, 0.0);
    let half_vec = normalize(key_dir + to_eye);
    let spec = pow(max(dot(n, half_vec), 0.0), 28.0);

    var engraved = 0.0;
    var lip = 0.0;
    if (frame.tint.w > 0.5) {
        let texel = 1.5 / f32(textureDimensions(glyph).y);
        engraved = decal(v.rest, v.rest_normal, vec2<f32>(0.0, 0.0));
        // The groove's lower edge catches the key light from above.
        let above = decal(v.rest, v.rest_normal, vec2<f32>(0.0, -texel));
        lip = max(above - engraved, 0.0);
    }

    let light = vec3<f32>(0.22, 0.22, 0.24)
        + key * vec3<f32>(1.0, 0.96, 0.9) * 0.85
        + fill * vec3<f32>(0.55, 0.65, 0.8) * 0.25
        + bounce * vec3<f32>(0.5, 0.4, 0.3) * 0.15;
    let groove = 1.0 - 0.7 * engraved;
    var c = albedo * light * groove;
    c = c + spec * 0.3 * mix(vec3<f32>(1.0), frame.tint.rgb, 0.5) * (1.0 - engraved);
    c = c + rim * 0.35 * vec3<f32>(0.8, 0.88, 1.0) * (1.0 - 0.6 * engraved);
    c = c + lip * 0.18 * frame.tint.rgb;
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
