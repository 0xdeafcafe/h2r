use crate::{Error, Result, Tables, bits::Bits};

/// The two finite CBR profiles observed on the retail disc.
#[derive(Clone, Copy, Debug)]
pub struct Format {
    pub bytes_per_second: u32,
    pub block_align: u16,
}
impl Format {
    pub fn from_wave(w: &asf::WaveFormat<'_>) -> Result<Self> {
        if w.format_tag != 0x161
            || w.channels != 2
            || w.sample_rate != 44100
            || w.extra.len() != 10
            || w.extra[..4] != 0x8800u32.to_le_bytes()
            || w.extra[4..6] != 15u16.to_le_bytes()
            || !matches!(
                (w.bytes_per_second, w.block_align),
                (12003, 4459) | (16002, 5945)
            )
        {
            return Err(Error::Unsupported(
                "requires retail WMAv2 44.1 kHz stereo CBR profile",
            ));
        }
        Ok(Self {
            bytes_per_second: w.bytes_per_second,
            block_align: w.block_align,
        })
    }
    fn offset_bits(self) -> usize {
        let bytes = (2048.0f32 * (self.bytes_per_second as f32 * 8.0 / (44100.0 * 2.0)) * 0.125)
            .round_ties_even() as u32;
        (31 - bytes.leading_zeros()) as usize + 5
    }
}
#[derive(Clone, Default)]
struct Channel {
    exponents: Vec<i32>,
    maximum: i32,
}
struct Packet {
    data: Vec<u8>,
    header: usize,
    offset: usize,
    frames: usize,
    sequence: u8,
}
impl Packet {
    fn parse(data: &[u8], format: Format) -> Result<Self> {
        if data.len() != usize::from(format.block_align) {
            return Err(Error::Invalid("codec packet length"));
        }
        let mut b = Bits::new(data);
        let sequence = b.read(4)? as u8;
        let frames = b.read(4)? as usize;
        let offset = b.read(format.offset_bits())? as usize;
        if offset > b.remaining() {
            return Err(Error::Invalid("superframe header"));
        }
        Ok(Self {
            data: data.to_vec(),
            header: b.pos,
            offset,
            frames,
            sequence,
        })
    }
}
/// One-packet lookahead for the superframe reservoir. Each push returns owned,
/// interleaved stereo i16 PCM. Call finish once at EOF. A failed decoder must be
/// discarded; it never tries to conceal malformed input.
pub struct Decoder<'a> {
    tables: &'a Tables,
    format: Format,
    pending: Option<Packet>,
    channels: [Channel; 2],
    exponent_size: usize,
    previous: usize,
    current: usize,
    next: usize,
    overlap: [Vec<i32>; 2],
    skip_samples: Option<usize>,
    finished: bool,
    pub frames: u64,
    pub blocks: u64,
    pub nonzero_coefficients: u64,
    pub frame_boundaries: Vec<usize>,
}
impl<'a> Decoder<'a> {
    pub fn new(tables: &'a Tables, format: Format) -> Result<Self> {
        if !matches!(
            (format.bytes_per_second, format.block_align),
            (12003, 4459) | (16002, 5945)
        ) {
            return Err(Error::Unsupported("CBR profile"));
        }
        let adjusted =
            (format.bytes_per_second as f32 * 8.0 / (44100.0 * 2.0)) * tables.stereo_bitrate_factor;
        if adjusted < tables.noise_threshold || adjusted <= tables.family_threshold {
            return Err(Error::Unsupported(
                "noise substitution or coefficient family",
            ));
        }
        Ok(Self {
            tables,
            format,
            pending: None,
            channels: Default::default(),
            exponent_size: 2048,
            previous: 2048,
            current: 2048,
            next: 2048,
            overlap: std::array::from_fn(|_| vec![0; 4096]),
            skip_samples: None,
            finished: false,
            frames: 0,
            blocks: 0,
            nonzero_coefficients: 0,
            frame_boundaries: Vec::new(),
        })
    }
    pub fn push(&mut self, packet: &[u8]) -> Result<Vec<i16>> {
        if self.finished {
            return Err(Error::Invalid("decoder already finished"));
        }
        self.finished = true; // Remain poisoned on every early error.
        self.frame_boundaries.clear();
        let next = Packet::parse(packet, self.format)?;
        let out = if let Some(previous) = self.pending.take() {
            if next.sequence != (previous.sequence + 1) & 15 {
                return Err(Error::Invalid("packet sequence discontinuity"));
            }
            self.group(&previous, Some(&next))?
        } else {
            Vec::new()
        };
        self.pending = Some(next);
        self.finished = false;
        Ok(out)
    }
    pub fn finish(&mut self) -> Result<Vec<i16>> {
        if self.finished {
            return Err(Error::Invalid("decoder already finished"));
        }
        self.frame_boundaries.clear();
        self.finished = true;
        if let Some(p) = self.pending.take() {
            self.group(&p, None)
        } else {
            Ok(Vec::new())
        }
    }
    fn group(&mut self, p: &Packet, next: Option<&Packet>) -> Result<Vec<i16>> {
        let start = p.header + p.offset;
        let extra = next.map_or(0, |n| n.offset);
        let length = p.data.len() * 8 - start + extra;
        let mut data = vec![0u8; length.div_ceil(8)];
        for i in 0..length {
            let (src, bit) = if i < p.data.len() * 8 - start {
                (&p.data[..], start + i)
            } else {
                let n = next.unwrap();
                (&n.data[..], n.header + i - (p.data.len() * 8 - start))
            };
            data[i / 8] |= ((src[bit / 8] >> (7 - bit % 8)) & 1) << (7 - i % 8);
        }
        let mut bits = Bits::new(&data);
        bits.end = length;
        let mut pcm = Vec::new();
        let mut reset = true;
        for _ in 0..p.frames {
            let start = pcm.len() / 2;
            self.frame(&mut bits, &mut reset, &mut pcm)?;
            if pcm.len() / 2 > start {
                self.frame_boundaries.push(start);
            }
        }
        Ok(pcm)
    }
    fn frame(&mut self, bits: &mut Bits<'_>, reset: &mut bool, pcm: &mut Vec<i16>) -> Result<()> {
        let mut position = 0;
        while position < 2048 {
            if *reset {
                self.previous = read_size(bits)?;
                self.current = read_size(bits)?;
                *reset = false;
            } else {
                self.previous = self.current;
                self.current = self.next;
            }
            self.next = read_size(bits)?;
            let n = self.current;
            if position + n > 2048 {
                return Err(Error::Invalid("block crosses frame boundary"));
            }
            let more_blocks = position > 0 || n < 2048;
            let mid_side = bits.read(1)? != 0;
            let coded = [bits.read(1)? != 0, bits.read(1)? != 0];
            let mut coefficients = [vec![0f32; n], vec![0f32; n]];
            let mut exponent_refresh = false;
            if coded.iter().any(|&x| x) {
                let mut gain = 1i32;
                loop {
                    let v = bits.read(7)? as i32;
                    gain += v;
                    if gain > 512 {
                        return Err(Error::Invalid("total gain"));
                    }
                    if v != 127 {
                        break;
                    }
                }
                let escape = match gain {
                    ..15 => 13,
                    15..32 => 12,
                    32..40 => 11,
                    40..45 => 10,
                    _ => 9,
                };
                let refresh = if more_blocks {
                    bits.read(1)? != 0
                } else {
                    true
                };
                exponent_refresh = refresh;
                let d = (2048 / n).ilog2() as usize;
                if refresh {
                    self.exponent_size = n;
                    for (ch, &is_coded) in self.channels.iter_mut().zip(&coded) {
                        ch.exponents.clear();
                        let mut exponent = 36;
                        for _ in 1..self.tables.bands[d].len() {
                            if is_coded {
                                exponent += self.tables.exponents.decode(bits)? as i32 - 60;
                            }
                            ch.exponents.push(if is_coded { exponent } else { 0 });
                        }
                        ch.maximum = *ch.exponents.iter().max().unwrap();
                    }
                }
                let exponent_bands =
                    &self.tables.bands[(2048 / self.exponent_size).ilog2() as usize];
                let stop = (2048 - 2048 * 9 / 100) * n / 2048;
                let gain_scale = self.tables.gain(gain);
                for ch in 0..2 {
                    if !coded[ch] {
                        continue;
                    }
                    let codebook = &self.tables.coefficient[usize::from(ch == 1 && mid_side)];
                    let mut index = 0;
                    while index < stop {
                        let symbol = codebook.tree.decode(bits)?;
                        if symbol == 1 {
                            break;
                        }
                        let (run, level) = if symbol == 0 {
                            // Escape run width is the maximum frame's log2, even for short blocks.
                            let magnitude = bits.read(escape)? as i32;
                            let run = bits.read(11)? as usize;
                            let sign = bits.read(1)?;
                            (run, if sign == 0 { -magnitude } else { magnitude })
                        } else {
                            let (run, level) = codebook.run_level(symbol)?;
                            (run, if bits.read(1)? == 0 { -level } else { level })
                        };
                        index += run;
                        if index >= stop {
                            return Err(Error::Invalid("coefficient run exceeds block"));
                        }
                        let scaled = index * self.exponent_size / n;
                        let band = exponent_bands.partition_point(|&x| x <= scaled) - 1;
                        let exponent = *self.channels[ch]
                            .exponents
                            .get(band)
                            .ok_or(Error::Invalid("missing reused exponents"))?;
                        let delta = (self.channels[ch].maximum - exponent).clamp(0, 71) as usize;
                        let scale = self.tables.exponent_scale[delta]
                            / (1u32 << (delta >> 2)) as f32
                            * gain_scale;
                        coefficients[ch][index] = level as f32 * scale;
                        index += 1;
                    }
                }
            }
            self.nonzero_coefficients +=
                coefficients.iter().flatten().filter(|&&v| v != 0.0).count() as u64;
            if mid_side {
                let [mid, side] = &mut coefficients;
                for (m, s) in mid.iter_mut().zip(side) {
                    let (a, b) = (*m, *s);
                    *m = a + b;
                    *s = a - b;
                }
            }
            let discard_silence = exponent_refresh
                && (coded.iter().all(|&v| v) || (mid_side && coded.iter().any(|&v| v)))
                && coefficients.iter().flatten().all(|&v| v == 0.0);
            self.synthesize(&mut coefficients, pcm, discard_silence)?;
            self.blocks += 1;
            position += n;
        }
        self.frames += 1;
        Ok(())
    }
    fn synthesize(
        &mut self,
        coefs: &mut [Vec<f32>; 2],
        pcm: &mut Vec<i16>,
        discard_silence: bool,
    ) -> Result<()> {
        let n = self.current;
        let left = n.min(self.previous);
        let right = n.min(self.next);
        let left_start = (n - left) / 2;
        let right_start = n + (n - right) / 2;
        let count = right_start - left_start;
        let lw = &self.tables.windows[(2048 / left).ilog2() as usize];
        let rw = &self.tables.windows[(2048 / right).ilog2() as usize];
        let mut output = [vec![0i16; count], vec![0i16; count]];
        for ch in 0..2 {
            super::transform::transform(
                &mut coefs[ch],
                &self.tables.rotations[(2048 / n).ilog2() as usize],
            );
            let z = &coefs[ch];
            for j in 0..count {
                let i = left_start + j;
                let value = if i < n / 2 {
                    z[n / 2 + i]
                } else if i < n {
                    z[n - 1 - (i - n / 2)]
                } else {
                    z[n / 2 - 1 - (i - n)]
                };
                let window = if j < left { lw[j] } else { 1.0 };
                let v = (value * window).round_ties_even();
                if !v.is_finite() {
                    return Err(Error::Invalid("nonfinite transform output"));
                }
                output[ch][j] = (v + self.overlap[ch][j] as f32).clamp(-32768.0, 32767.0) as i16;
            }
            self.overlap[ch].fill(0);
            for (j, &window) in rw.iter().rev().enumerate() {
                let i = right_start + j;
                // Saved overlap, 0x38d3c3: up to 3n/2 the folded value is
                // windowed as is (0x38d43c loop); past 3n/2 it is negated
                // first (`fchs` at 0x38d918). The tail half is symmetric about
                // 3n/2 and only the signed window's sign flips there, so
                // dropping the negation leaves uncancelled aliasing in the
                // second half of every following block.
                let value = if i < 3 * n / 2 {
                    z[3 * n / 2 - 1 - i]
                } else {
                    -z[i - 3 * n / 2]
                };
                self.overlap[ch][j] =
                    (value * window).round_ties_even().clamp(-65536.0, 65535.0) as i32;
            }
        }
        // The game suppresses its initial overlap region; retain the same delay.
        let remaining = self
            .skip_samples
            .get_or_insert(if discard_silence { 4096 } else { n });
        let skip = (*remaining).min(count);
        *remaining -= skip;
        for (&left, &right) in output[0][skip..].iter().zip(&output[1][skip..]) {
            pcm.extend([left, right]);
        }
        Ok(())
    }
}
fn read_size(bits: &mut Bits<'_>) -> Result<usize> {
    let d = bits.read(3)?;
    if d > 4 {
        return Err(Error::Invalid("block size code"));
    }
    Ok(2048 >> d)
}

#[cfg(test)]
mod tests {
    use super::*;
    /// Both retail profiles: 96 kbps and 128 kbps.
    const PROFILES: [Format; 2] = [
        Format {
            bytes_per_second: 12003,
            block_align: 4459,
        },
        Format {
            bytes_per_second: 16002,
            block_align: 5945,
        },
    ];
    /// MSB-first bit writer for synthetic packets.
    fn packet(format: Format, fields: &[(u32, usize)]) -> Vec<u8> {
        let mut data = vec![0u8; usize::from(format.block_align)];
        let mut pos = 0;
        for &(value, width) in fields {
            for i in (0..width).rev() {
                data[pos / 8] |= (((value >> i) & 1) as u8) << (7 - pos % 8);
                pos += 1;
            }
        }
        data
    }
    #[test]
    fn both_retail_profiles_are_accepted_and_others_rejected() {
        let extra = [0x00, 0x88, 0, 0, 15, 0, 0, 0, 0, 0];
        let wave = |bytes_per_second, block_align| asf::WaveFormat {
            format_tag: 0x161,
            channels: 2,
            sample_rate: 44100,
            bytes_per_second,
            block_align,
            bits_per_sample: 16,
            extra: &extra,
        };
        let tables = Tables::synthetic();
        for f in PROFILES {
            let parsed = Format::from_wave(&wave(f.bytes_per_second, f.block_align)).unwrap();
            assert_eq!(parsed.block_align, f.block_align);
            assert_eq!(f.offset_bits(), 13);
            Decoder::new(&tables, f).unwrap();
        }
        for (rate, align) in [(16003, 5945), (16002, 4459), (12003, 5945), (12004, 4459)] {
            assert!(Format::from_wave(&wave(rate, align)).is_err());
            let f = Format {
                bytes_per_second: rate,
                block_align: align,
            };
            assert!(Decoder::new(&tables, f).is_err());
        }
    }
    #[test]
    fn sequence_discontinuity_is_rejected_and_poisons() {
        let tables = Tables::synthetic();
        for format in PROFILES {
            let first = packet(format, &[(0, 4), (1, 4)]);
            let skipped = packet(format, &[(2, 4), (1, 4)]);
            let mut decoder = Decoder::new(&tables, format).unwrap();
            decoder.push(&first).unwrap();
            let e = decoder.push(&skipped).unwrap_err();
            assert!(
                matches!(e, Error::Invalid("packet sequence discontinuity")),
                "{e}"
            );
            assert!(decoder.push(&packet(format, &[(1, 4), (1, 4)])).is_err());
            assert!(decoder.finish().is_err());
            // Sequence numbers wrap modulo 16.
            let mut decoder = Decoder::new(&tables, format).unwrap();
            decoder.push(&packet(format, &[(15, 4), (1, 4)])).unwrap();
            decoder.push(&first).unwrap();
            assert_eq!(decoder.finish().unwrap().len(), 4096);
        }
    }
    #[test]
    fn total_gain_above_512_is_rejected_and_poisons() {
        let tables = Tables::synthetic();
        for format in PROFILES {
            // Header (sequence, one frame, zero offset), three 2048 block
            // sizes, mid/side off, both channels coded, then gain groups.
            let frame = |last: u32| {
                let mut f = vec![(0, 4), (1, 4), (0, 13), (0, 9), (0, 1), (3, 2)];
                f.extend([(127, 7); 4]);
                f.push((last, 7));
                packet(format, &f)
            };
            // 1 + 4 * 127 + 4 = 513.
            let mut decoder = Decoder::new(&tables, format).unwrap();
            decoder.push(&frame(4)).unwrap();
            let e = decoder.finish().unwrap_err();
            assert!(matches!(e, Error::Invalid("total gain")), "{e}");
            assert!(decoder.finish().is_err());
            assert!(decoder.push(&frame(4)).is_err());
            // 1 + 4 * 127 + 3 = 512 is the largest accepted total.
            let mut decoder = Decoder::new(&tables, format).unwrap();
            decoder.push(&frame(3)).unwrap();
            if let Err(e) = decoder.finish() {
                assert!(!matches!(e, Error::Invalid("total gain")), "{e}");
            }
        }
    }
    #[test]
    fn synthetic_silence_streams_and_errors_poison_state() {
        for format in PROFILES {
            silence_and_poison(format);
        }
    }
    fn silence_and_poison(format: Format) {
        let tables = Tables::synthetic();
        let mut first = vec![0; usize::from(format.block_align)];
        first[0] = 1; // One full-size, uncoded stereo frame.
        let mut second = first.clone();
        second[0] = 0x11;
        let mut decoder = Decoder::new(&tables, format).unwrap();
        assert!(decoder.push(&first).unwrap().is_empty());
        assert!(decoder.push(&second).unwrap().is_empty());
        let pcm = decoder.finish().unwrap();
        assert_eq!(pcm, vec![0; 4096]);
        assert_eq!(decoder.frames, 2);
        assert!(decoder.finish().is_err());
        let mut decoder = Decoder::new(&tables, format).unwrap();
        assert!(decoder.push(&[]).is_err());
        assert!(decoder.push(&first).is_err());
        // Mutate synthetic packet bytes, never private streams, and require
        // checked failure or bounded successful output, never a panic.
        for byte in 0..8 {
            for value in [1, 0x55, 0xaa, 0xff] {
                let mut data = first.clone();
                data[byte] = value;
                let mut decoder = Decoder::new(&tables, format).unwrap();
                if decoder.push(&data).is_ok() {
                    let _ = decoder.finish();
                }
            }
        }
    }
    #[test]
    fn overlap_add_cancels_time_domain_aliasing() {
        // Analyse a synthetic two-tone signal with the textbook MDCT (written
        // from its definition, not from the decoder's fold) under each block's
        // window, then resynthesise through `synthesize`. Overlap-add must
        // rebuild the signal, including across block-size transitions. A sign
        // error in either half of the fold leaves aliasing about as large as
        // the signal in half of every block.
        let tables = Tables::synthetic();
        let format = Format {
            bytes_per_second: 12003,
            block_align: 4459,
        };
        let sizes = [
            2048, 2048, 2048, 512, 512, 128, 128, 256, 1024, 2048, 256, 256, 2048,
        ];
        let x = |t: isize, ch: usize| {
            let t = t as f64;
            let (a, b) = if ch == 0 {
                (0.0131, 0.0517)
            } else {
                (0.0093, 0.211)
            };
            9000.0 * (a * t).sin() + 5000.0 * (b * t + 1.0).cos()
        };
        let window = |n: usize, left: usize, right: usize, t: usize| {
            let (ls, rs) = ((n - left) / 2, n + (n - right) / 2);
            let w = |m: usize, i: usize| {
                (std::f64::consts::PI * (i as f64 + 0.5) / (2 * m) as f64).sin()
            };
            match t {
                _ if t < ls => 0.0,
                _ if t < ls + left => w(left, t - ls),
                _ if t < rs => 1.0,
                _ if t < rs + right => w(right, right - 1 - (t - rs)),
                _ => 0.0,
            }
        };
        let mut decoder = Decoder::new(&tables, format).unwrap();
        decoder.skip_samples = Some(0);
        let mut pcm = Vec::new();
        let mut first_output = 0isize;
        let mut start = 0isize; // Time of the current block's sample t = 0.
        for (b, &n) in sizes.iter().enumerate() {
            let previous = sizes[b.saturating_sub(1)];
            let next = sizes[(b + 1).min(sizes.len() - 1)];
            let (left, right) = (n.min(previous), n.min(next));
            if b == 0 {
                first_output = (n - left) as isize / 2;
            }
            let mut coefs: [Vec<f32>; 2] = std::array::from_fn(|ch| {
                (0..n)
                    .map(|k| {
                        (0..2 * n)
                            .map(|t| {
                                let phase = std::f64::consts::PI / n as f64
                                    * (t as f64 + 0.5 + n as f64 / 2.0)
                                    * (k as f64 + 0.5);
                                x(start + t as isize, ch) * window(n, left, right, t) * phase.cos()
                            })
                            .sum::<f64>() as f32
                    })
                    .collect()
            });
            (decoder.previous, decoder.current, decoder.next) = (previous, n, next);
            decoder.synthesize(&mut coefs, &mut pcm, false).unwrap();
            let next_left_start = (next - next.min(n)) / 2;
            start += (n + (n - right) / 2) as isize - next_left_start as isize;
        }
        // The first block's rising edge has no predecessor; skip it.
        let skip = sizes[0];
        let mut worst = 0f64;
        for (f, frame) in pcm.as_chunks::<2>().0.iter().enumerate().skip(skip) {
            for (ch, &v) in frame.iter().enumerate() {
                // The retail fold's global sign is negative (signed windows,
                // first half negative); the oracle confirms this convention.
                let expected = -x(first_output + f as isize, ch);
                worst = worst.max((f64::from(v) - expected).abs());
            }
        }
        assert!(pcm.len() / 2 > skip + 8192, "too little output");
        assert!(worst <= 2.0, "overlap-add error {worst}");
    }
    #[test]
    fn headers_and_block_sizes_are_bounded() {
        let format = Format {
            bytes_per_second: 12003,
            block_align: 4459,
        };
        let p = vec![0; 4459];
        // A zero-frame packet is legal: it can carry only reservoir continuation.
        assert_eq!(Packet::parse(&p, format).unwrap().frames, 0);
        for n in 0..p.len() {
            assert!(Packet::parse(&p[..n], format).is_err());
        }
        for code in 0..8 {
            let data = [code << 5];
            let result = read_size(&mut Bits::new(&data));
            if code < 5 {
                assert_eq!(result.unwrap(), 2048 >> code);
            } else {
                assert!(result.is_err());
            }
        }
        assert!(read_size(&mut Bits::new(&[])).is_err());
    }
}
