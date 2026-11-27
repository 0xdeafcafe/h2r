// The particle update on the GPU (docs/graphics/native-particles.md, "GPU
// particle systems"). Three kernels a frame, all over the system's own
// buffers, no CPU readback between them:
//
// update   one thread a particle of `src`: age, die or integrate and
//          evaluate in place; a workgroup scan of the survivors writes
//          each one's place in its block and the block's total.
// offsets  one workgroup: the block totals scanned into block offsets;
//          the survivors, the births that fit, the new count, and the
//          indirect arguments (next update, this scatter, the draw).
// scatter  survivors to `dst` in their order, then the births after
//          them, born and evaluated.
//
// The structs are particle_common.wgsl's, prepended by particle_compute.rs.

struct State {
    count: u32,
    old: u32,
    alive: u32,
    spawned: u32,
}

struct Step {
    dt: f32,
    spawns: u32,
    capacity: u32,
    mirror: u32,
}

@group(0) @binding(0) var<storage, read_write> src: array<Particle>;
@group(0) @binding(1) var<storage, read_write> dst: array<Particle>;
@group(0) @binding(2) var<storage, read_write> scan: array<u32>;
@group(0) @binding(3) var<storage, read_write> blocks: array<u32>;
@group(0) @binding(4) var<storage, read_write> state: State;
@group(0) @binding(5) var<storage, read_write> args: array<u32, 12>;
@group(0) @binding(6) var<storage, read> spawns: array<Spawn>;
@group(0) @binding(7) var<uniform> definition: Definition;
@group(0) @binding(8) var<storage, read> sprites: array<Sprite>;
@group(0) @binding(9) var<uniform> step: Step;

const WORKGROUP: u32 = 256u;
const ALIVE: u32 = 0x80000000u;

var<workgroup> sums: array<u32, 256>;

// Inclusive scan of `sums` (Hillis-Steele); every thread calls it.
fn scan_workgroup(l: u32) {
    for (var o = 1u; o < WORKGROUP; o = o << 1u) {
        workgroupBarrier();
        var v = sums[l];
        if l >= o {
            v = v + sums[l - o];
        }
        workgroupBarrier();
        sums[l] = v;
    }
    workgroupBarrier();
}

const FLIP_U: u32 = 0x2u;
const FLIP_V: u32 = 0x4u;
const FRAMES_LESS_ONE: u32 = 0x8u;
const RANDOM_SEQUENCE: u32 = 0x10u;
const NO_BLEND: u32 = 0x20u;
const PARTICLE_TINT: u32 = 0x180u;
const SPIN: u32 = 0x2000u;
const FIXED_WINDOW: u32 = 0x10000u;

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    return a + (b - a) * t;
}

fn sample(curve: array<vec4<f32>, 4>, x: f32) -> f32 {
    var c = curve;
    let t = clamp(x, 0.0, 1.0) * 15.0;
    let i = min(u32(t), 14u);
    let f = t - f32(i);
    let j = i + 1u;
    return lerp(c[i / 4u][i % 4u], c[j / 4u][j % 4u], f);
}

fn input_of(values: array<f32, 9>, i: u32) -> f32 {
    var inputs = values;
    if i >= 9u {
        return 0.0;
    }
    return inputs[i];
}

fn modify(inputs: array<f32, 9>, op: u32, i: u32, v: f32) -> f32 {
    let m = input_of(inputs, i);
    if i >= 9u {
        return v;
    }
    if op == 1u {
        return m + v;
    }
    if op == 2u {
        return m * v;
    }
    return v;
}

// 0x00246cd0.
fn property(p: Property, inputs: array<f32, 9>) -> f32 {
    let x = input_of(inputs, p.input);
    let r = input_of(inputs, p.range);
    var v = lerp(sample(p.curve_lo, x), sample(p.curve_hi, x), r);
    if p.map != 0u {
        v = clamp(v, 0.0, 1.0);
        v = p.lo + (p.hi - p.lo) * v;
    }
    return modify(inputs, p.modifier, p.modifier_input, v);
}

// 0x00246d80.
fn colour_of(p: ColourProperty, inputs: array<f32, 9>) -> vec3<f32> {
    let x = input_of(inputs, p.input);
    let r = input_of(inputs, p.range);
    let v = modify(inputs, p.modifier, p.modifier_input, lerp(sample(p.curve_lo, x), sample(p.curve_hi, x), r));
    let t = clamp(v, 0.0, 1.0) * 7.0;
    let i = min(u32(t), 6u);
    let f = t - f32(i);
    var g = p.gradient;
    let a = g[i].xyz;
    let b = g[i + 1u].xyz;
    return vec3<f32>(lerp(a.x, b.x, f), lerp(a.y, b.y, f), lerp(a.z, b.z, f));
}

// A window (u0, u1, v0, v1) as (u, v, width, height), flipped.
fn flipped(w: vec4<f32>, flips: u32) -> vec4<f32> {
    var out = vec4<f32>(w.x, w.z, w.y - w.x, w.w - w.z);
    if (flips & 1u) != 0u {
        out.x = w.y;
        out.z = w.x - w.y;
    }
    if (flips & 2u) != 0u {
        out.y = w.w;
        out.w = w.z - w.w;
    }
    return out;
}

// A frame's corner scale from its aspect: the long side is one.
fn aspect(w: vec4<f32>) -> vec2<f32> {
    let wd = w.y - w.x;
    let ht = w.w - w.z;
    if !(wd > 0.0) || !(ht > 0.0) {
        return vec2<f32>(1.0, 1.0);
    }
    if wd > ht {
        return vec2<f32>(1.0, ht / wd);
    }
    return vec2<f32>(wd / ht, 1.0);
}

// 0x002490f4: v less its truncation, plus one below zero.
fn wrap(v: f32) -> f32 {
    let f = v - trunc(v);
    return select(f, f + 1.0, f < 0.0);
}

// What 0x00224550 computes for the emitter, into the particle's outputs.
fn evaluate(p: ptr<function, Particle>) {
    let life = select(0.0, (*p).age / (*p).lifetime, (*p).lifetime > 0.0);
    let r = (*p).random;
    let inputs = array<f32, 9>(life, (*p).age, r.x, r.y, r.z, r.w, 1.0, 0.0, wrap((*p).spin));
    let flags = definition.flags;

    var rotation = property(definition.rotation, inputs) * definition.angle_scale;
    var rgb = colour_of(definition.colour, inputs);
    let alpha = property(definition.alpha, inputs) * definition.tint.x;
    if (flags & PARTICLE_TINT) != 0u {
        rgb = rgb * unpack4x8unorm((*p).tint).xyz;
    }
    let lanes = clamp(vec4<f32>(definition.tint.yzw * rgb, alpha), vec4<f32>(0.0), vec4<f32>(1.0));
    (*p).colour = pack4x8unorm(lanes);

    var flips = 0u;
    if (flags & FLIP_U) != 0u && ((*p).words & 1u) != 0u {
        flips |= 1u;
    }
    if (flags & FLIP_V) != 0u && ((*p).words & 0x10000u) != 0u {
        flips |= 2u;
    }

    if (flags & FIXED_WINDOW) != 0u {
        let w = flipped(definition.fixed_window, flips);
        (*p).window = w;
        (*p).next_window = w;
        (*p).corners = definition.fixed_corners;
        (*p).blend = 0.0;
    } else {
        var a = vec4<f32>(0.0, 1.0, 0.0, 1.0);
        var b = a;
        var reg = vec2<f32>(0.5, 0.5);
        var f = 0.0;
        if definition.frames > 0u && definition.sequences > 0u {
            var seq = definition.sequence_first;
            if (flags & RANDOM_SEQUENCE) != 0u {
                seq += i32(((*p).words >> 16u) % max(definition.sequence_pick, 1u));
            }
            seq = clamp(seq, 0, i32(definition.sequences) - 1);
            let count = i32(definition.frames);
            let frames = select(count, count - 1, (flags & FRAMES_LESS_ONE) != 0u);
            let t = property(definition.frame, inputs) * f32(frames);
            let k = i32(floor(t));
            let first = ((k % count) + count) % count;
            let next = (((k + 1) % count) + count) % count;
            if (flags & NO_BLEND) == 0u {
                f = t - f32(k);
            }
            let sa = sprites[u32(seq * count + first)];
            let sb = sprites[u32(seq * count + next)];
            a = sa.window;
            b = sb.window;
            reg = vec2<f32>(lerp(sa.registration.x, sb.registration.x, f), lerp(sa.registration.y, sb.registration.y, f));
        }
        let ka = aspect(a);
        let kb = aspect(b);
        let s = vec2<f32>(lerp(ka.x, kb.x, f), lerp(ka.y, kb.y, f));
        (*p).window = flipped(a, flips);
        (*p).next_window = flipped(b, flips);
        (*p).corners = vec4<f32>(-reg.x * 2.0 * s.x, -reg.y * 2.0 * s.y, (1.0 - reg.x) * 2.0 * s.x, (1.0 - reg.y) * 2.0 * s.y);
        (*p).blend = f;
    }

    if (flags & SPIN) != 0u {
        rotation = rotation + definition.spin;
    }
    (*p).rotation = rotation;
    (*p).size = property(definition.size, inputs) * definition.size_scale;
}


@compute @workgroup_size(256)
fn update(@builtin(global_invocation_id) gid: vec3<u32>, @builtin(local_invocation_index) l: u32, @builtin(workgroup_id) wg: vec3<u32>) {
    let i = gid.x;
    let n = state.count;
    var alive = 0u;
    if i < n {
        var p = src[i];
        p.age = p.age + step.dt;
        if step.mirror == 0u && p.age < p.lifetime {
            let drag = max(0.0, 1.0 - definition.drag * step.dt);
            p.velocity = (p.velocity + definition.gravity * step.dt) * drag;
            p.position = p.position + p.velocity * step.dt;
            p.spin = p.spin + p.spin_rate * step.dt;
            evaluate(&p);
            alive = 1u;
        }
        src[i] = p;
    }
    sums[l] = alive;
    scan_workgroup(l);
    if i < n {
        scan[i] = (sums[l] - alive) | select(0u, ALIVE, alive != 0u);
    }
    if l == WORKGROUP - 1u {
        blocks[wg.x] = sums[l];
    }
}

fn groups(n: u32) -> u32 {
    return (n + WORKGROUP - 1u) / WORKGROUP;
}

@compute @workgroup_size(256)
fn offsets(@builtin(local_invocation_index) l: u32) {
    let old = state.count;
    let n = groups(old);
    var total = 0u;
    if l < n {
        total = blocks[l];
    }
    sums[l] = total;
    scan_workgroup(l);
    let alive = sums[WORKGROUP - 1u];
    if l < n {
        blocks[l] = sums[l] - total;
    }
    storageBarrier();
    workgroupBarrier();
    if l == 0u {
        let spawned = min(step.spawns, step.capacity - min(alive, step.capacity));
        let count = alive + spawned;
        state.old = old;
        state.alive = alive;
        state.spawned = spawned;
        state.count = count;
        args[0] = groups(count);
        args[1] = 1u;
        args[2] = 1u;
        args[4] = groups(max(old, spawned));
        args[5] = 1u;
        args[6] = 1u;
        args[8] = 6u;
        args[9] = count;
        args[10] = 0u;
        args[11] = 0u;
    }
}

@compute @workgroup_size(256)
fn scatter(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if i < state.old {
        let s = scan[i];
        if (s & ALIVE) != 0u {
            dst[blocks[i / WORKGROUP] + (s & ~ALIVE)] = src[i];
        }
    }
    if i < state.spawned {
        let s = spawns[i];
        var p: Particle;
        p.position = s.position;
        p.age = s.age;
        p.velocity = s.velocity;
        p.lifetime = s.lifetime;
        p.random = s.random;
        p.id = s.id;
        p.words = s.words;
        p.tint = s.tint;
        p.spin = s.spin;
        p.spin_rate = s.spin_rate;
        evaluate(&p);
        dst[state.alive + i] = p;
    }
}
