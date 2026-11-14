//! Plays a test tone, then a decoded Xbox ADPCM buffer, on the default device.
//!
//! `cargo run --release --example tone -p audio`
//!
//! The ADPCM clip is synthesised and encoded in-process (a small IMA encoder
//! lives here) so no game data is needed.

use std::{thread::sleep, time::Duration};

use audio::{SoundBuffer, WaveFormat, adpcm, output::Output};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let out = Output::start_default()?;
    println!("device rate: {} Hz", out.sample_rate());

    // 1. A 440 Hz sine as plain 16-bit PCM at 22.05 kHz; the mixer resamples.
    let rate = 22_050;
    let pcm: Vec<i16> = (0..rate)
        .map(|i| ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 9000.0) as i16)
        .collect();
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    let tone = SoundBuffer::from_data(WaveFormat::pcm16(1, rate)?, bytes)?;
    let id = out.mixer().lock().unwrap().add_buffer(tone);
    println!("PCM16 tone, 1s");
    out.mixer().lock().unwrap().buffer_mut(id).unwrap().play(false);
    sleep(Duration::from_millis(1200));

    // 2. A rising arpeggio, encoded to Xbox ADPCM and decoded by the mixer.
    let rate = 32_000u32;
    let mut clip = Vec::new();
    for freq in [330.0f32, 415.0, 494.0, 660.0] {
        for i in 0..rate / 2 {
            let t = i as f32 / rate as f32;
            let env = (1.0 - t * 2.0).max(0.0);
            clip.push(((t * freq * std::f32::consts::TAU).sin() * 12000.0 * env) as i16);
        }
    }
    clip.resize(clip.len().div_ceil(64) * 64, 0);
    let adpcm_bytes = encode_mono(&clip);
    println!(
        "ADPCM arpeggio, {} bytes for {} samples",
        adpcm_bytes.len(),
        clip.len()
    );
    let buf = SoundBuffer::from_data(WaveFormat::xbox_adpcm(1, rate)?, adpcm_bytes)?;
    let id = out.mixer().lock().unwrap().add_buffer(buf);
    out.mixer().lock().unwrap().buffer_mut(id).unwrap().play(false);
    sleep(Duration::from_millis(2300));
    Ok(())
}

// Minimal IMA encoder in Xbox block layout (mono), mirroring the test one.
// Reuses the decoder's tables through a private copy of the update rule.
fn encode_mono(pcm: &[i16]) -> Vec<u8> {
    const STEPS: [i32; 89] = [
        7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60,
        66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371,
        408, 449, 494, 544, 598, 658, 724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707,
        1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132,
        7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623,
        27086, 29794, 32767,
    ];
    const ADJ: [i32; 8] = [-1, -1, -1, -1, 2, 4, 6, 8];
    let mut out = Vec::new();
    let mut index = 0i32;
    for block in pcm.as_chunks::<{ adpcm::SAMPLES_PER_BLOCK }>().0.iter() {
        let mut pred = block[0] as i32;
        out.extend_from_slice(&block[0].to_le_bytes());
        out.extend_from_slice(&[index as u8, 0]);
        let mut codes = [0u8; 64];
        for (i, &s) in block.iter().enumerate().skip(1) {
            let step = STEPS[index as usize];
            let mut diff = s as i32 - pred;
            let mut code = 0u8;
            if diff < 0 {
                code = 8;
                diff = -diff;
            }
            if diff >= step {
                code |= 4;
                diff -= step;
            }
            if diff >= step >> 1 {
                code |= 2;
                diff -= step >> 1;
            }
            if diff >= step >> 2 {
                code |= 1;
            }
            let mut d = step >> 3;
            if code & 1 != 0 { d += step >> 2 }
            if code & 2 != 0 { d += step >> 1 }
            if code & 4 != 0 { d += step }
            if code & 8 != 0 { d = -d }
            pred = (pred + d).clamp(-32768, 32767);
            index = (index + ADJ[(code & 7) as usize]).clamp(0, 88);
            codes[i - 1] = code;
        }
        for pair in codes.as_chunks::<2>().0.iter() {
            out.push(pair[0] | (pair[1] << 4));
        }
    }
    out
}
