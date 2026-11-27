// Particle records, shared by the update kernels and the draw.
// native_render::particle_system mirrors every struct byte for byte
// (docs/graphics/native-particles.md, "GPU particle systems").

struct Property {
    input: u32,
    range: u32,
    modifier_input: u32,
    modifier: u32,
    lo: f32,
    hi: f32,
    map: u32,
    _pad: u32,
    curve_lo: array<vec4<f32>, 4>,
    curve_hi: array<vec4<f32>, 4>,
}

struct ColourProperty {
    input: u32,
    range: u32,
    modifier_input: u32,
    modifier: u32,
    curve_lo: array<vec4<f32>, 4>,
    curve_hi: array<vec4<f32>, 4>,
    gradient: array<vec4<f32>, 8>,
}

struct Sprite {
    window: vec4<f32>,
    registration: vec2<f32>,
    _pad: vec2<f32>,
}

struct Definition {
    flags: u32,
    sequence_first: i32,
    sequence_pick: u32,
    frames: u32,
    sequences: u32,
    angle_scale: f32,
    spin: f32,
    size_scale: f32,
    tint: vec4<f32>,
    gravity: vec3<f32>,
    drag: f32,
    fixed_window: vec4<f32>,
    fixed_corners: vec4<f32>,
    rotation: Property,
    alpha: Property,
    size: Property,
    frame: Property,
    colour: ColourProperty,
}

struct Particle {
    position: vec3<f32>,
    age: f32,
    velocity: vec3<f32>,
    lifetime: f32,
    random: vec4<f32>,
    id: u32,
    words: u32,
    tint: u32,
    colour: u32,
    size: f32,
    rotation: f32,
    blend: f32,
    spin: f32,
    window: vec4<f32>,
    next_window: vec4<f32>,
    corners: vec4<f32>,
    spin_rate: f32,
    _pad0: f32,
    _pad1: f32,
    _pad2: f32,
}

struct Spawn {
    position: vec3<f32>,
    lifetime: f32,
    velocity: vec3<f32>,
    id: u32,
    random: vec4<f32>,
    words: u32,
    tint: u32,
    age: f32,
    spin: f32,
    spin_rate: f32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
}

