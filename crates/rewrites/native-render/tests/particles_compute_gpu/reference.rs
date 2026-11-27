//! The CPU reference evaluator: `particle_compute.wgsl`'s update, its
//! evaluation and its compaction, written once more in Rust, operation
//! for operation. Test-only; the runtime has no CPU path.

use native_render::particle_system::{
    ColourProperty, Definition, INPUTS, Particle, Property, Spawn, Sprite, flag, modifier,
};

pub struct System {
    pub definition: Definition,
    pub sprites: Vec<Sprite>,
    pub capacity: u32,
    pub particles: Vec<Particle>,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn sample(c: &[[f32; 4]; 4], x: f32) -> f32 {
    let t = x.clamp(0.0, 1.0) * 15.0;
    let i = (t as usize).min(14);
    let f = t - i as f32;
    let j = i + 1;
    lerp(c[i / 4][i % 4], c[j / 4][j % 4], f)
}

fn input_of(inputs: &[f32; 8], i: u32) -> f32 {
    if i >= INPUTS { 0.0 } else { inputs[i as usize] }
}

fn modify(inputs: &[f32; 8], op: u32, i: u32, v: f32) -> f32 {
    if i >= INPUTS {
        return v;
    }
    match op {
        modifier::ADD => inputs[i as usize] + v,
        modifier::MULTIPLY => inputs[i as usize] * v,
        _ => v,
    }
}

fn property(p: &Property, inputs: &[f32; 8]) -> f32 {
    let x = input_of(inputs, p.input);
    let r = input_of(inputs, p.range);
    let mut v = lerp(sample(&p.curve_lo, x), sample(&p.curve_hi, x), r);
    if p.map != 0 {
        v = v.clamp(0.0, 1.0);
        v = p.lo + (p.hi - p.lo) * v;
    }
    modify(inputs, p.modifier, p.modifier_input, v)
}

fn colour_of(p: &ColourProperty, inputs: &[f32; 8]) -> [f32; 3] {
    let x = input_of(inputs, p.input);
    let r = input_of(inputs, p.range);
    let v = modify(inputs, p.modifier, p.modifier_input, lerp(sample(&p.curve_lo, x), sample(&p.curve_hi, x), r));
    let t = v.clamp(0.0, 1.0) * 7.0;
    let i = (t as usize).min(6);
    let f = t - i as f32;
    let (a, b) = (p.gradient[i], p.gradient[i + 1]);
    [lerp(a[0], b[0], f), lerp(a[1], b[1], f), lerp(a[2], b[2], f)]
}

fn flipped(w: [f32; 4], flips: u32) -> [f32; 4] {
    let mut out = [w[0], w[2], w[1] - w[0], w[3] - w[2]];
    if flips & 1 != 0 {
        out[0] = w[1];
        out[2] = w[0] - w[1];
    }
    if flips & 2 != 0 {
        out[1] = w[3];
        out[3] = w[2] - w[3];
    }
    out
}

fn aspect(w: [f32; 4]) -> [f32; 2] {
    let (wd, ht) = (w[1] - w[0], w[3] - w[2]);
    if !(wd > 0.0) || !(ht > 0.0) {
        [1.0, 1.0]
    } else if wd > ht {
        [1.0, ht / wd]
    } else {
        [wd / ht, 1.0]
    }
}

fn pack_unorm(v: [f32; 4]) -> u32 {
    let b = v.map(|c| (0.5 + 255.0 * c.clamp(0.0, 1.0)).floor() as u32);
    b[0] | b[1] << 8 | b[2] << 16 | b[3] << 24
}

fn unpack_rgb(c: u32) -> [f32; 3] {
    [0, 8, 16].map(|s| ((c >> s) & 0xff) as f32 / 255.0)
}

impl System {
    /// The evaluation; true when the frame's position sits within 1e-3 of
    /// a frame boundary, where the GPU's rounding may pick the neighbour.
    /// A position of exactly zero (a birth) is not ambiguous.
    fn evaluate(&self, p: &mut Particle) -> bool {
        let d = &self.definition;
        let life = if p.lifetime > 0.0 { p.age / p.lifetime } else { 0.0 };
        let r = p.random;
        let inputs = [life, p.age, r[0], r[1], r[2], r[3], 1.0, 0.0];
        let flags = d.flags;

        let mut rotation = property(&d.rotation, &inputs) * d.angle_scale;
        let mut rgb = colour_of(&d.colour, &inputs);
        let alpha = property(&d.alpha, &inputs) * d.tint[0];
        if flags & flag::PARTICLE_TINT != 0 {
            let t = unpack_rgb(p.tint);
            rgb = [rgb[0] * t[0], rgb[1] * t[1], rgb[2] * t[2]];
        }
        p.colour = pack_unorm([d.tint[1] * rgb[0], d.tint[2] * rgb[1], d.tint[3] * rgb[2], alpha]);

        let mut flips = 0;
        if flags & flag::FLIP_U != 0 && p.words & 1 != 0 {
            flips |= 1;
        }
        if flags & flag::FLIP_V != 0 && p.words & 0x1_0000 != 0 {
            flips |= 2;
        }
        let mut edge = false;
        if flags & flag::FIXED_WINDOW != 0 {
            let w = flipped(d.fixed_window, flips);
            p.window = w;
            p.next_window = w;
            p.corners = d.fixed_corners;
            p.blend = 0.0;
        } else {
            let mut a = [0.0, 1.0, 0.0, 1.0];
            let mut b = a;
            let mut reg = [0.5, 0.5];
            let mut f = 0.0;
            if d.frames > 0 && d.sequences > 0 {
                let mut seq = d.sequence_first;
                if flags & flag::RANDOM_SEQUENCE != 0 {
                    seq += ((p.words >> 16) % d.sequence_pick.max(1)) as i32;
                }
                let seq = seq.clamp(0, d.sequences as i32 - 1);
                let count = d.frames as i32;
                let frames = if flags & flag::FRAMES_LESS_ONE != 0 { count - 1 } else { count };
                let t = property(&d.frame, &inputs) * frames as f32;
                let k = t.floor() as i32;
                let fraction = t - k as f32;
                edge = t != 0.0 && !(1e-3..=1.0 - 1e-3).contains(&fraction);
                let first = ((k % count) + count) % count;
                let next = (((k + 1) % count) + count) % count;
                if flags & flag::NO_BLEND == 0 {
                    f = fraction;
                }
                let sa = self.sprites[(seq * count + first) as usize];
                let sb = self.sprites[(seq * count + next) as usize];
                a = sa.window;
                b = sb.window;
                reg = [lerp(sa.registration[0], sb.registration[0], f), lerp(sa.registration[1], sb.registration[1], f)];
            }
            let (ka, kb) = (aspect(a), aspect(b));
            let s = [lerp(ka[0], kb[0], f), lerp(ka[1], kb[1], f)];
            p.window = flipped(a, flips);
            p.next_window = flipped(b, flips);
            p.corners = [
                -reg[0] * 2.0 * s[0],
                -reg[1] * 2.0 * s[1],
                (1.0 - reg[0]) * 2.0 * s[0],
                (1.0 - reg[1]) * 2.0 * s[1],
            ];
            p.blend = f;
        }
        if flags & flag::SPIN != 0 {
            rotation += d.spin;
        }
        p.rotation = rotation;
        p.size = property(&d.size, &inputs) * d.size_scale;
        edge
    }

    /// One update: survivors in order, then the births that fit. Returns
    /// each live particle's frame-edge flag.
    pub fn step(&mut self, spawns: &[Spawn], dt: f32) -> Vec<bool> {
        let d = self.definition;
        let mut out = Vec::new();
        let mut edges = Vec::new();
        for mut p in std::mem::take(&mut self.particles) {
            p.age += dt;
            if p.age < p.lifetime {
                let drag = (1.0 - d.drag * dt).max(0.0);
                for k in 0..3 {
                    p.velocity[k] = (p.velocity[k] + d.gravity[k] * dt) * drag;
                    p.position[k] += p.velocity[k] * dt;
                }
                edges.push(self.evaluate(&mut p));
                out.push(p);
            }
        }
        let room = (self.capacity as usize).saturating_sub(out.len());
        for s in spawns.iter().take(room) {
            let mut p = Particle {
                position: s.position,
                velocity: s.velocity,
                lifetime: s.lifetime,
                random: s.random,
                id: s.id,
                words: s.words,
                tint: s.tint,
                ..Default::default()
            };
            edges.push(self.evaluate(&mut p));
            out.push(p);
        }
        self.particles = out;
        edges
    }
}
