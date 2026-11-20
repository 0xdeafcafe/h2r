//! Source endpoint styles following an ordinary constant displacement
//! curve. Halo 2 reads the curve at 0x51150, and the H1 renderer reads
//! the additional records when the native ribbon is submitted.
pub const MAGIC: [u8; 4] = *b"H1EL";
pub const HEADER: usize = 0x18;
pub const STRIDE: usize = 0x28;

#[derive(Debug, Clone, Copy)]
pub struct Endpoint {
    pub end: bool,
    pub depth: u8,
    pub jitter: [f32; 3],
    pub displacement: f32,
    pub width: f32,
    pub argb: [f32; 4],
}

impl Endpoint {
    pub fn encode(self, out: &mut Vec<u8>) {
        out.extend_from_slice(&[u8::from(self.end), self.depth, 0, 0]);
        for f in self
            .jitter
            .into_iter()
            .chain([self.displacement, self.width])
            .chain(self.argb)
        {
            out.extend_from_slice(&f.to_le_bytes());
        }
    }
    pub fn decode(row: &[u8; STRIDE]) -> Self {
        let word = |i: usize| f32::from_le_bytes(row[4 * i..4 * i + 4].try_into().unwrap());
        Self {
            end: row[0] != 0,
            depth: row[1],
            jitter: [word(1), word(2), word(3)],
            displacement: word(4),
            width: word(5),
            argb: [word(6), word(7), word(8), word(9)],
        }
    }
}
