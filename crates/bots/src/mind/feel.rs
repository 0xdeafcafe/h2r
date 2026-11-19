//! Shared pieces of the controller model. Each part has its own seeded
//! stream so changing the footwork does not change when the bot reacts.

#[derive(Clone, Debug)]
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Self(if seed == 0 {
            0xae3b_92c7_e981_5a21
        } else {
            seed
        })
    }

    pub(crate) fn unit(&mut self) -> f32 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32 / (1u32 << 24) as f32
    }

    pub(crate) fn ticks(&mut self, min: u32, max: u32) -> u32 {
        min + (self.unit() * (max - min + 1) as f32) as u32
    }

    pub(crate) fn signed(&mut self) -> f32 {
        2.0 * self.unit() - 1.0
    }
}

/// Limit the size of a stick vector, retaining its direction.
pub(super) fn limit(v: [f32; 2], max: f32) -> [f32; 2] {
    let length = v[0].hypot(v[1]);
    if length > max {
        v.map(|c| c * (max / length))
    } else {
        v
    }
}

pub(crate) fn reached(tick: u32, until: u32) -> bool {
    tick.wrapping_sub(until) < (1 << 31)
}
