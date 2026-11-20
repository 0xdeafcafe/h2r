use crate::{Decoder, Error, Format, Result, Tables};
/// A complete finite permutation, with codec frame locations for diagnostics.
pub struct Decoded {
    pub pcm: Vec<i16>,
    pub codec_frames: u64,
    pub blocks: u64,
    pub nonzero_coefficients: u64,
    pub packets: usize,
    pub frame_boundaries: Vec<usize>,
    pub duration_100ns: u64,
}
/// Decode a concatenated ASF permutation. max_samples bounds output allocation,
/// in interleaved samples, independently of untrusted duration metadata.
pub fn decode_asf(tables: &Tables, data: &[u8], max_samples: usize) -> Result<Decoded> {
    let file = asf::Asf::parse(data)?;
    if file.streams.len() != 1 {
        return Err(Error::Unsupported("multiple ASF streams"));
    }
    let stream = &file.streams[0];
    stream.check_audio_spread()?;
    let format = Format::from_wave(
        stream
            .audio
            .as_ref()
            .ok_or(Error::Unsupported("non-audio stream"))?,
    )?;
    let mut decoder = Decoder::new(tables, format)?;
    let mut reassembly = asf::Reassembler::new(stream.number, 1 << 20)?;
    let mut declared_frames = 0u64;
    let mut result = Decoded {
        pcm: Vec::new(),
        codec_frames: 0,
        blocks: 0,
        nonzero_coefficients: 0,
        packets: 0,
        frame_boundaries: Vec::new(),
        duration_100ns: file
            .file
            .play_duration_100ns
            .saturating_sub(file.file.preroll_ms.saturating_mul(10000)),
    };
    fn append(r: &mut Decoded, pcm: Vec<i16>, boundaries: &[usize], limit: usize) -> Result<()> {
        if pcm.len() > limit.saturating_sub(r.pcm.len()) {
            return Err(Error::Invalid("PCM output limit exceeded"));
        }
        let base = r.pcm.len() / 2;
        r.frame_boundaries
            .extend(boundaries.iter().map(|&b| base + b));
        r.pcm.extend(pcm);
        Ok(())
    }
    for packet in file.packets() {
        for payload in packet?.payloads {
            if payload.stream != stream.number {
                return Err(Error::Invalid("unexpected stream"));
            }
            if let Some(o) = reassembly.push(&payload)? {
                if o.data.len() % usize::from(format.block_align) != 0 {
                    return Err(Error::Invalid("codec object alignment"));
                }
                for packet in o.data.chunks_exact(format.block_align.into()) {
                    result.packets += 1;
                    declared_frames += u64::from(packet[0] & 15);
                    let pcm = decoder.push(packet)?;
                    append(&mut result, pcm, &decoder.frame_boundaries, max_samples)?;
                }
            }
        }
    }
    reassembly.finish()?;
    let pcm = decoder.finish()?;
    append(&mut result, pcm, &decoder.frame_boundaries, max_samples)?;
    if decoder.frames != declared_frames {
        return Err(Error::Invalid(
            "decoded frame count differs from packet headers",
        ));
    }
    result.codec_frames = decoder.frames;
    result.blocks = decoder.blocks;
    result.nonzero_coefficients = decoder.nonzero_coefficients;
    Ok(result)
}
