// One model shown as a menu picture (crate::showcase, crate::showcase_gpu):
// the model at rest, turned about z, lit by a soft key, a cool fill and a
// rim, its base map's brightness tinted, with a glyph cut into the
// forehead as a bevelled groove. A silhouette is a near-black fill and a faint rim only.
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
// The glyph as a signed distance field: the distance to its strokes in
// glyph units (the decal's box is -1..1), negative inside a stroke.
@group(0) @binding(3) var glyph: texture_2d<f32>;
@group(0) @binding(4) var clamped: sampler;

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

// The groove's wall: this far in from a stroke's edge (glyph units) the
// cut slopes down to its floor.
const BEVEL: f32 = 0.05;
// How steep the wall is: the tangent of its angle to the surface.
const STEEP: f32 = 1.3;

// The glyph's distance at decal coordinate `uv`.
fn glyph_distance(uv: vec2<f32>) -> f32 {
    return textureSampleLevel(glyph, clamped, uv, 0.0).r;
}

@fragment
fn fs_main(v: V) -> @location(0) vec4<f32> {
    let len = length(v.normal);
    let to_eye = normalize(frame.eye.xyz - v.world);
    let outward = select(vec3<f32>(0.0, 0.0, 1.0), v.normal / len, len > 1e-6);
    // Two-sided: a surface seen from behind its normal (the inside of the
    // open cranium, its rim) is lit as its front. Lit one-sided it took
    // only the ambient and a full rim, a flat blue-grey.
    let smooth_n = select(outward, -outward, dot(outward, to_eye) < 0.0);

    // The decal's planar projection along the model's forward axis: u
    // right across the face (+y), v down (-z), 0..1 inside its box.
    let r = frame.decal.w;
    let decal_uv = vec2<f32>(
        (v.rest.y - frame.decal.y) / (2.0 * r) + 0.5,
        0.5 - (v.rest.z - frame.decal.z) / (2.0 * r),
    );
    // A pixel's width in glyph units, for the stroke's anti-aliased edge.
    let pixel = max(2.0 * max(length(dpdx(decal_uv)), length(dpdy(decal_uv))), 1e-5);
    let base = textureSample(base_map, linear, v.uv).rgb;

    if (frame.eye.w > 0.5) {
        // Not collected: the shape only.
        let facing = max(dot(smooth_n, to_eye), 0.0);
        let rim = pow(1.0 - facing, 2.5);
        let c = vec3<f32>(frame.cut.z) + frame.cut.w * rim * vec3<f32>(0.85, 0.9, 1.0);
        return vec4<f32>(c, 1.0);
    }

    // The engraving: how much of the pixel is inside a stroke, how deep
    // into the cut it is (0 at the edge, 1 on the floor) and the wall's
    // slope, which leans the normal towards the stroke's middle.
    var cover = 0.0;
    var depth = 0.0;
    var n = smooth_n;
    let rest_len = length(v.rest_normal);
    let forward = select(0.0, v.rest_normal.x / rest_len, rest_len > 1e-6);
    let inside = all(decal_uv >= vec2<f32>(0.0)) && all(decal_uv <= vec2<f32>(1.0));
    if (frame.tint.w > 0.5 && inside && v.rest.x >= frame.cut.x && forward >= frame.cut.y) {
        // Fade the last stretch of facing so the decal has no hard edge.
        let soft = smoothstep(frame.cut.y, frame.cut.y + 0.15, forward);
        let d = glyph_distance(decal_uv);
        cover = clamp(0.5 - d / pixel, 0.0, 1.0) * soft;
        depth = clamp(-d / BEVEL, 0.0, 1.0);
        let e = 1.5 / f32(textureDimensions(glyph).x);
        let grad = vec2<f32>(
            glyph_distance(decal_uv + vec2<f32>(e, 0.0)) - glyph_distance(decal_uv - vec2<f32>(e, 0.0)),
            glyph_distance(decal_uv - vec2<f32>(0.0, e)) - glyph_distance(decal_uv + vec2<f32>(0.0, e)),
        );
        let grad_len = length(grad);
        if (grad_len > 1e-6) {
            // On the wall only: past the edge, short of the floor.
            let wall = cover * clamp(0.5 + (d + BEVEL) / pixel, 0.0, 1.0);
            let out_rest = vec3<f32>(0.0, grad.x, grad.y) / grad_len;
            let out_turned = (frame.model * vec4<f32>(out_rest, 0.0)).xyz;
            n = normalize(smooth_n - STEEP * wall * out_turned);
        }
    }

    let facing = max(dot(smooth_n, to_eye), 0.0);
    let rim = pow(1.0 - facing, 2.5);
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

    let light = vec3<f32>(0.22, 0.22, 0.24)
        + key * vec3<f32>(1.0, 0.96, 0.9) * 0.85
        + fill * vec3<f32>(0.55, 0.65, 0.8) * 0.25
        + bounce * vec3<f32>(0.5, 0.4, 0.3) * 0.15;
    // The cut is in shadow, deepest on its floor.
    let groove = 1.0 - cover * mix(0.4, 0.72, depth);
    var c = albedo * light * groove;
    c = c + spec * 0.3 * mix(vec3<f32>(1.0), frame.tint.rgb, 0.5) * (1.0 - cover * depth);
    c = c + rim * 0.35 * vec3<f32>(0.8, 0.88, 1.0) * (1.0 - 0.6 * cover);
    return vec4<f32>(clamp(c, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
