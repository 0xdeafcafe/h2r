//! Measures the host output path: opens the default device through
//! `DeviceSink`, pumps a tone from a 60 Hz loop for a few seconds and prints
//! the latency and underrun figures.
//!
//! `cargo run --release --example latency -p audio [target_ms] [seconds]`

use std::time::{Duration, Instant};

use audio::{AudioSink, Mixer, Pump, SoundBuffer, WaveFormat, output::DeviceSink};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let target_ms: u32 = args.next().and_then(|a| a.parse().ok()).unwrap_or(40);
    let seconds: u64 = args.next().and_then(|a| a.parse().ok()).unwrap_or(4);

    let mut sink = DeviceSink::open_default()?;
    println!("device {} at {} Hz", sink.device_id(), sink.sample_rate());
    let mut mixer = Mixer::new(sink.sample_rate());
    let rate = 22_050;
    let pcm: Vec<u8> = (0..rate)
        .flat_map(|i| {
            (((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 6000.0) as i16)
                .to_le_bytes()
        })
        .collect();
    let id = mixer.add_buffer(SoundBuffer::from_data(WaveFormat::pcm16(1, rate)?, pcm)?);
    mixer.buffer_mut(id).unwrap().play(true);

    let mut pump = Pump::new(target_ms);
    let start = Instant::now();
    let mut next = start;
    while start.elapsed() < Duration::from_secs(seconds) {
        pump.pump(&mut mixer, &mut sink);
        next += Duration::from_micros(16_667);
        std::thread::sleep(next.saturating_duration_since(Instant::now()));
    }
    let l = sink.latency();
    let s = sink.stats();
    println!("target {target_ms} ms");
    println!(
        "ring mean {:.1} ms (peak {:.1}), driver {:.1} ms, total {:.1} ms over {} callbacks",
        l.ring_ms, l.ring_peak_ms, l.device_ms, l.total_ms, l.callbacks
    );
    println!(
        "underruns {} ({} frames of silence), played {} frames, reopens {}",
        s.underruns,
        s.underrun_frames,
        s.played_frames,
        sink.reopen_count()
    );
    Ok(())
}
