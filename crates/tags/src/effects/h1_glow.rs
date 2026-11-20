//! Extra inputs for a Halo 1 marker-chain glow in an H2 MGS2 layer.
//! The native layer supplies the bitmap and sample count. The geometry
//! submission consumes this profile from the layer's final 0x30 bytes.
pub const MAGIC: [u8; 4] = *b"H1GW";

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Glow {
    pub speed: f32,
    pub twist: f32,
    pub orbit: [f32; 2],
    pub size: [f32; 2],
    pub colours: [u32; 2],
    pub fade: f32,
    pub registration: [f32; 2],
}

impl Glow {
    pub fn encode(self) -> [u8; 48] {
        let mut out = [0; 48];
        let words = [
            u32::from_le_bytes(MAGIC),
            self.speed.to_bits(),
            self.twist.to_bits(),
            self.orbit[0].to_bits(),
            self.orbit[1].to_bits(),
            self.size[0].to_bits(),
            self.size[1].to_bits(),
            self.colours[0],
            self.colours[1],
            self.fade.to_bits(),
            self.registration[0].to_bits(),
            self.registration[1].to_bits(),
        ];
        for (dst, word) in out.chunks_exact_mut(4).zip(words) {
            dst.copy_from_slice(&word.to_le_bytes());
        }
        out
    }

    pub fn decode(b: &[u8; 48]) -> Option<Self> {
        if b[..4] != MAGIC {
            return None;
        }
        let word = |i: usize| u32::from_le_bytes(b[i * 4..i * 4 + 4].try_into().unwrap());
        let real = |i| f32::from_bits(word(i));
        Some(Self {
            speed: real(1),
            twist: real(2),
            orbit: [real(3), real(4)],
            size: [real(5), real(6)],
            colours: [word(7), word(8)],
            fade: real(9),
            registration: [real(10), real(11)],
        })
    }
}
