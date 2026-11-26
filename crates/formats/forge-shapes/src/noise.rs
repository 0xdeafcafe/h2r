//! Seeded lattice noise for the natural set (docs/game/forge-blockout.md,
//! "The natural set"). An integer hash of the lattice cell and the seed,
//! blended with a cubic in `f32`: `+ - *`, `floor` and integer work only,
//! so a seed gives the same bits on every host.

/// A hash of a lattice cell under a seed.
pub fn hash(seed: u32, x: i32, y: i32, z: i32) -> u32 {
    let mut h = seed ^ 0x9e37_79b9;
    for v in [x as u32, y as u32, z as u32] {
        h = (h ^ v).wrapping_mul(0x85eb_ca6b);
        h ^= h >> 15;
        h = h.wrapping_mul(0xc2b2_ae35);
        h ^= h >> 13;
    }
    h
}

/// A hash as a number in `[0, 1)`: its top 24 bits over 2^24, exact in `f32`.
pub fn unit(h: u32) -> f32 {
    (h >> 8) as f32 / 16_777_216.0
}

/// A hash as a number in `[-1, 1)`.
pub fn signed(h: u32) -> f32 {
    unit(h) * 2.0 - 1.0
}

fn smooth(t: f32) -> f32 {
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Value noise in `[-1, 1)`: the corners of the lattice cell holding `p`
/// take a hash each and are blended with a cubic.
pub fn value(seed: u32, p: [f32; 3]) -> f32 {
    let base = [p[0].floor(), p[1].floor(), p[2].floor()];
    let cell = [base[0] as i32, base[1] as i32, base[2] as i32];
    let t = [
        smooth(p[0] - base[0]),
        smooth(p[1] - base[1]),
        smooth(p[2] - base[2]),
    ];
    let corner =
        |dx: i32, dy: i32, dz: i32| signed(hash(seed, cell[0] + dx, cell[1] + dy, cell[2] + dz));
    let along_x = |dy: i32, dz: i32| lerp(corner(0, dy, dz), corner(1, dy, dz), t[0]);
    let along_y = |dz: i32| lerp(along_x(0, dz), along_x(1, dz), t[1]);
    lerp(along_y(0), along_y(1), t[2])
}

/// Octaves of [`value`], each twice the frequency and half the weight of
/// the last, scaled to stay inside `[-1, 1)`.
pub fn fbm(seed: u32, p: [f32; 3], octaves: u32) -> f32 {
    let (mut sum, mut weight, mut total) = (0.0f32, 1.0f32, 0.0f32);
    let mut q = p;
    for octave in 0..octaves {
        sum += weight * value(seed.wrapping_add(octave), q);
        total += weight;
        weight *= 0.5;
        q = [q[0] * 2.0, q[1] * 2.0, q[2] * 2.0];
    }
    if total > 0.0 { sum / total } else { 0.0 }
}

/// A stream of numbers from a seed: the hash of a counter.
#[derive(Clone, Debug)]
pub struct Stream {
    seed: u32,
    count: i32,
}

impl Stream {
    pub fn new(seed: u32) -> Stream {
        Stream { seed, count: 0 }
    }

    pub fn next_u32(&mut self) -> u32 {
        self.count = self.count.wrapping_add(1);
        hash(self.seed, self.count, 0x51ed, 7)
    }

    /// The next number in `[0, 1)`.
    pub fn unit(&mut self) -> f32 {
        unit(self.next_u32())
    }

    /// The next number in `[-1, 1)`.
    pub fn signed(&mut self) -> f32 {
        signed(self.next_u32())
    }

    /// The next number in `[lo, hi)`.
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}
