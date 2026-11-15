use crate::{Error, Result, VerifiedImage, bits::Bits};

pub(crate) struct Huffman {
    words: Vec<u16>,
}
impl Huffman {
    fn load(image: &VerifiedImage<'_>, start: u32, end: u32) -> Result<Self> {
        Ok(Self {
            words: image
                .bytes_at(start, (end - start) as usize)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|b| u16::from_le_bytes([b[0], b[1]]))
                .collect(),
        })
    }
    pub fn decode(&self, bits: &mut Bits<'_>) -> Result<usize> {
        let look = bits.peek(22);
        let mut consumed = 0;
        let mut cursor = 0usize;
        for depth in 0..14 {
            let width = if depth < 8 { 2 } else { 1 };
            let index = ((look >> (22 - consumed - width)) & ((1 << width) - 1)) as usize;
            consumed += width;
            cursor += index;
            let word = *self
                .words
                .get(cursor)
                .ok_or(Error::Invalid("Huffman node"))?;
            if word & 0x8000 != 0 {
                let length = usize::from((word >> 10) & 31);
                if length == 0 || length > consumed || length + 1 < consumed {
                    return Err(Error::Invalid("Huffman leaf length"));
                }
                let mut symbol = usize::from(word & 1023);
                if symbol >= 1020 {
                    symbol = usize::from(
                        *self
                            .words
                            .get(cursor + (symbol & 3) + 1)
                            .ok_or(Error::Invalid("Huffman extension"))?,
                    );
                }
                bits.skip(length)?;
                return Ok(symbol);
            }
            cursor += usize::from(word);
        }
        Err(Error::Invalid("Huffman depth"))
    }
}
pub(crate) struct Codebook {
    pub tree: Huffman,
    runs: Vec<u16>,
    levels: Vec<u16>,
}
impl Codebook {
    fn load(
        image: &VerifiedImage<'_>,
        tree: u32,
        tree_end: u32,
        runs: u32,
        levels: u32,
        end: u32,
    ) -> Result<Self> {
        fn words(image: &VerifiedImage<'_>, a: u32, b: u32) -> Result<Vec<u16>> {
            Ok(image
                .bytes_at(a, (b - a) as usize)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|v| u16::from_le_bytes([v[0], v[1]]))
                .collect())
        }
        Ok(Self {
            tree: Huffman::load(image, tree, tree_end)?,
            runs: words(image, runs, levels)?,
            levels: words(image, levels, end)?,
        })
    }
    pub fn run_level(&self, symbol: usize) -> Result<(usize, i32)> {
        let i = symbol
            .checked_sub(2)
            .ok_or(Error::Invalid("coefficient symbol"))?;
        Ok((
            usize::from(*self.runs.get(i).ok_or(Error::Invalid("coefficient run"))?),
            i32::from(
                *self
                    .levels
                    .get(i)
                    .ok_or(Error::Invalid("coefficient level"))?,
            ),
        ))
    }
}
/// All values below are read from the authenticated owner-supplied image.
/// Addresses and instruction-immediate locations are recorded in docs/wma.md.
pub struct Tables {
    pub(crate) exponents: Huffman,
    pub(crate) coefficient: [Codebook; 2],
    pub(crate) bands: [Vec<usize>; 5],
    pub(crate) exponent_scale: Vec<f32>,
    pub(crate) gain_fraction: Vec<f32>,
    pub(crate) gain_power: Vec<f32>,
    pub(crate) gain_small: [f32; 2],
    pub(crate) gain_base: f64,
    pub(crate) gain_exponent: f32,
    pub(crate) windows: [Vec<f32>; 5],
    pub(crate) rotations: [[f32; 11]; 5],
    pub(crate) stereo_bitrate_factor: f32,
    pub(crate) noise_threshold: f32,
    pub(crate) family_threshold: f32,
}
impl Tables {
    pub fn load(image: &VerifiedImage<'_>) -> Result<Self> {
        fn u32_at(i: &VerifiedImage<'_>, a: u32) -> Result<u32> {
            Ok(u32::from_le_bytes(i.bytes_at(a, 4)?.try_into().unwrap()))
        }
        fn f32_at(i: &VerifiedImage<'_>, a: u32) -> Result<f32> {
            Ok(f32::from_bits(u32_at(i, a)?))
        }
        fn floats(i: &VerifiedImage<'_>, a: u32, n: usize) -> Result<Vec<f32>> {
            Ok(i.bytes_at(a, n * 4)?
                .as_chunks::<4>()
                .0
                .iter()
                .map(|b| f32::from_le_bytes(*b))
                .collect())
        }
        let mut bands: [Vec<usize>; 5] = std::array::from_fn(|_| vec![0]);
        let factor = f32_at(image, 0x45dbc0)? / 44100.0;
        for (d, band) in bands.iter_mut().enumerate().take(2) {
            let n = 2048 >> d;
            let scale = (n * 2) as f32 * factor;
            for b in image.bytes_at(0x39b67c, 50)?.as_chunks::<2>().0.iter() {
                let hz = u16::from_le_bytes([b[0], b[1]]);
                let end = (((hz as f32 * scale + f32_at(image, 0x45dc0c)?)
                    * f32_at(image, 0x45dc20)?) as usize)
                    * 4;
                if end > *band.last().unwrap() {
                    band.push(end);
                }
                if end > n {
                    break;
                }
            }
            *band.last_mut().unwrap() = n;
        }
        // Immediate operands of stores, not values transcribed into Rust.
        for a in std::iter::once(0x38b63e).chain((0..16).map(|i| 0x38b645 + i * 7)) {
            bands[2].push(u32_at(image, a)? as usize);
        }
        bands[3].resize(16, 0);
        bands[3][1] = u32_at(image, 0x38b8fd)? as usize;
        for (index, a) in [
            (2, 0x38b736),
            (3, 0x38b73d),
            (4, 0x38b744),
            (5, 0x38b74b),
            (6, 0x38b904),
            (7, 0x38b752),
            (8, 0x38b759),
            (9, 0x38b760),
            (10, 0x38b767),
            (11, 0x38b76e),
            (12, 0x38b775),
            (13, 0x38b77c),
            (14, 0x38b783),
            (15, 0x38b90b),
        ] {
            bands[3][index] = u32_at(image, a)? as usize;
        }
        bands[4].push(u32_at(image, 0x38b7a8)? as usize);
        bands[4].push(u32_at(image, 0x38b7af)? as usize);
        bands[4].push(usize::from(image.bytes_at(0x38b79e, 1)?[0]));
        for i in 0..9 {
            bands[4].push(u32_at(image, 0x38b7b9 + i * 7)? as usize);
        }
        for (d, b) in bands.iter_mut().enumerate() {
            if d >= 2 {
                for v in b.iter_mut() {
                    *v = (*v + 2) / 4 * 4;
                }
            }
            if b.len() < 2 || b.windows(2).any(|x| x[0] >= x[1]) || b.last() != Some(&(2048 >> d)) {
                return Err(Error::Invalid("extracted band layout"));
            }
        }
        let mut windows = std::array::from_fn(|_| Vec::new());
        let mut rotations = [[0.; 11]; 5];
        for d in 0..5 {
            let n = 2048 >> d;
            let pointer = u32_at(image, 0x39b200 + (n / 128 * 4) as u32)?;
            rotations[d].copy_from_slice(&floats(image, pointer, 11)?);
            // Window lookup selected by 0x003321d5; table addresses from 0x38c727.
            let address = [0x3971c8, 0x3991c8, 0x39a1c8, 0x39a9c8, 0x39adc8][d];
            windows[d] = floats(image, address, n)?;
        }
        Ok(Self {
            exponents: Huffman::load(image, 0x3929b8, 0x392b80)?,
            coefficient: [
                Codebook::load(image, 0x392c18, 0x3932e8, 0x39b898, 0x39bc50, 0x39c008)?,
                Codebook::load(image, 0x394928, 0x394fc8, 0x39c008, 0x39c370, 0x39c6d8)?,
            ],
            bands,
            exponent_scale: floats(image, 0x39b248, 72)?,
            gain_fraction: floats(image, 0x39b3e8, 146)?,
            gain_power: floats(image, 0x39b630, 19)?,
            gain_small: [f32_at(image, 0x4172cc)?, f32_at(image, 0x4172d0)?],
            gain_base: f64::from_le_bytes(image.bytes_at(0x45e150, 8)?.try_into().unwrap()),
            gain_exponent: f32_at(image, 0x45dc1c)?,
            windows,
            rotations,
            stereo_bitrate_factor: f32_at(image, 0x45e2fc)?,
            noise_threshold: f32_at(image, 0x4172e0)?,
            family_threshold: f32_at(image, 0x4172d4)?,
        })
    }
    pub(crate) fn gain(&self, g: i32) -> f32 {
        if g < 0 {
            self.gain_small[1] / 2.0f32.powi((-g >> 3) + 1)
        } else if g < 18 {
            self.gain_small[0] * 2.0f32.powi((g >> 3) + 1)
        } else if g < 146 {
            self.gain_fraction[g as usize] * self.gain_power[(g >> 3) as usize]
        } else {
            self.gain_base
                .powf(f64::from(g) * f64::from(self.gain_exponent)) as f32
        }
    }
}

#[cfg(any(test, feature = "synthetic"))]
impl Tables {
    /// Stand-in tables for tests that cannot read the retail image: plain
    /// sine windows, exact rotations, flat gains and one-symbol trees. They
    /// decode synthetic silence, not real streams. Feature `synthetic`.
    #[doc(hidden)]
    pub fn synthetic() -> Self {
        let bands = std::array::from_fn(|d| vec![0, 2048 >> d]);
        let windows = std::array::from_fn(|d| {
            let n = 2048 >> d;
            (0..n)
                .map(|i| {
                    let s = (std::f64::consts::PI * (i as f64 + 0.5) / (2 * n) as f64).sin() as f32;
                    if i < n / 2 { -s } else { s }
                })
                .collect()
        });
        let rotations = std::array::from_fn(|d| {
            let a = std::f64::consts::PI / (2048 >> d) as f64;
            [
                (a / 4.).sin() as f32,
                (a / 4.).cos() as f32,
                (3. * a / 4.).sin() as f32,
                (3. * a / 4.).cos() as f32,
                a.sin() as f32,
                a.cos() as f32,
                (2. * a).sin() as f32,
                (2. * a).cos() as f32,
                (a / 2.).sin() as f32,
                (a / 2.).cos() as f32,
                2. * a.sin() as f32,
            ]
        });
        Self {
            exponents: Huffman {
                words: vec![0x883c; 4],
            },
            coefficient: std::array::from_fn(|_| Codebook {
                tree: Huffman {
                    words: vec![0x8801; 4],
                },
                runs: vec![],
                levels: vec![],
            }),
            bands,
            windows,
            rotations,
            exponent_scale: vec![1.; 72],
            gain_fraction: vec![1.; 146],
            gain_power: vec![1.; 19],
            gain_small: [1.; 2],
            gain_base: 1.,
            gain_exponent: 1.,
            stereo_bitrate_factor: 1.,
            noise_threshold: 0.,
            family_threshold: 0.,
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn synthetic_tree_checks_consumption_and_extensions() {
        // 0 -> symbol 7, 10 -> extended symbol 1100, 11 -> symbol 9.
        let tree = Huffman {
            words: vec![0x8407, 0x8407, 0x8808, 0x8809],
        };
        let mut b = Bits::new(&[0b01011000]);
        assert_eq!(tree.decode(&mut b).unwrap(), 7);
        assert_eq!(tree.decode(&mut b).unwrap(), 8);
        assert_eq!(tree.decode(&mut b).unwrap(), 9);
        // Separate extension fixture, to avoid overlapping a leaf and its extension.
        let tree = Huffman {
            words: vec![0x8407, 0x8407, 2, 0x8809, 0x8ffc, 1100, 0x8c0a, 0x8c0b],
        };
        let mut b = Bits::new(&[0b10000000]);
        assert_eq!(tree.decode(&mut b).unwrap(), 1100);
        assert_eq!(b.pos, 3);
        let mut b = Bits::new(&[]);
        assert!(tree.decode(&mut b).is_err());
        let bad = Huffman {
            words: vec![32767; 4],
        };
        assert!(bad.decode(&mut Bits::new(&[0; 4])).is_err());
    }
}
