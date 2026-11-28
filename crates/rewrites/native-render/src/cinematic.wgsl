struct Settings {
    view: vec4f, // origin, size in render texels
    blur: vec4f, // radius in scaled pixels, kernel, reserved
    grading: vec4f, // light, desaturation, additive tint blend, final pass
    tint: vec4f,
    video: vec4f, // enable, overbright, noise, reserved
    scale: vec4f, // texels per guest pixel, noise UV offset
};
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var linear_sampler: sampler;
@group(0) @binding(2) var<uniform> s: Settings;
@group(0) @binding(3) var mask: texture_2d<f32>;
@group(0) @binding(4) var noise: texture_2d<f32>;

@vertex fn vs(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4f {
    let p = array(vec2f(-1,-1), vec2f(3,-1), vec2f(-1,3));
    return vec4f(p[vertex], 0., 1.);
}

fn scene(p: vec2f) -> vec4f {
    let pos = clamp(p, s.view.xy + 0.5, s.view.xy + s.view.zw - 0.5);
    return textureSampleLevel(source, linear_sampler, pos / vec2f(textureDimensions(source)), 0.);
}

@fragment fn fs(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    let p = pos.xy;
    let uv = (p - s.view.xy) / s.view.zw;
    var color = scene(p);
    if s.blur.z == 1. {
        let d = s.blur.xy;
        color = 0.25 * (scene(p + d) + scene(p - d) +
            scene(p + vec2f(d.x,-d.y)) + scene(p + vec2f(-d.x,d.y)));
    } else if s.blur.z == 2. {
        let d = (uv - 0.5) * s.blur.xy;
        color = 0.25 * (scene(p + d) + scene(p) + scene(p - d) +
            scene(p - 2. * d));
    }
    if s.grading.w == 0. { return color; }
    if s.video.x != 0. {
        let coord = (p - s.view.xy) / s.scale.xy;
        let nsize = vec2f(textureDimensions(noise));
        let snow = textureSampleLevel(noise, linear_sampler, fract(coord / nsize + s.scale.zw), 0.);
        let m = textureSampleLevel(mask, linear_sampler, fract(coord / vec2f(textureDimensions(mask))), 0.);
        let grain = mix(vec4f(1.), snow, clamp(s.video.z, 0., 1.));
        let squared = color.rgb * color.rgb;
        let dark = min(vec3f(1.), squared * squared * s.video.y);
        let bright = min(vec3f(1.), color.rgb * s.video.y);
        let alpha = grain.a * m.a;
        return vec4f(mix(dark, bright, grain.rgb * m.rgb) * (1. - alpha), alpha * (1. - alpha));
    }
    // 0x6cc64 squares the complement twice. 0x6ccc9 weights
    // the desaturation dot product with one third in each lane.
    let inv = vec3f(1.) - color.rgb;
    var rgb = mix(color.rgb, vec3f(1.) - inv * inv * inv * inv, clamp(s.grading.x, 0., 1.));
    let mono = dot(rgb, vec3f(1./3.));
    if s.grading.y > 0. {
        // 0x6cd89 uses the original RGB as final-combiner D (additive).
        // 0x6cdc2 uses it as C, weighted by the complement of A.
        let weight = clamp(s.grading.y, 0., 1.);
        let tinted = vec3f(mono) * s.tint.rgb;
        rgb = select(mix(rgb, tinted, weight), min(vec3f(1.), rgb + weight * tinted), s.grading.z != 0.);
    }
    return vec4f(rgb, color.a);
}
