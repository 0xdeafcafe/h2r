//! Private corpus validation. Emits only aggregate measurements, never assets.
use std::path::Path;
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let path = std::env::args()
        .nth(1)
        .ok_or("usage: verify_disc absolute.iso")?;
    let disc = asf::disc::Disc::open(Path::new(&path))?;
    let bytes = disc.read_xbe()?;
    let image = wma::VerifiedImage::load(&bytes)?;
    let tables = wma::Tables::load(&image)?;
    let (mut passed, mut failed, mut packets, mut frames, mut blocks, mut samples) =
        (0, 0, 0, 0u64, 0u64, 0usize);
    let mut max_high = 0f64;
    let mut max_dc = 0f64;
    let mut max_clip = 0f64;
    let mut min_rms = f64::INFINITY;
    let mut peak = 0i32;
    let mut rms_max = 0f64;
    let mut max_duration = 0f64;
    let mut boundary = 0f64;
    let mut interior = 0f64;
    let mut boundary_n = 0;
    let mut interior_n = 0;
    for index in 0..disc.streams.len() {
        let data = disc.read_stream(index)?;
        match wma::decode_asf(&tables, &data, 128 * 1024 * 1024) {
            Ok(d) => {
                let sanity = wma::diagnostics::sanity(&d.pcm, &d.frame_boundaries);
                max_high = max_high.max(sanity.high_frequency_fraction);
                max_dc = max_dc.max(sanity.dc[0].abs()).max(sanity.dc[1].abs());
                max_clip = max_clip.max(sanity.clipped_fraction);
                min_rms = min_rms.min(sanity.rms);
                let rms = (d.pcm.iter().map(|&x| (x as f64).powi(2)).sum::<f64>()
                    / d.pcm.len().max(1) as f64)
                    .sqrt();
                if sanity.rms == 0.0 && d.nonzero_coefficients != 0 {
                    return Err(format!("stream {index}: nonzero spectrum but silent PCM").into());
                }
                let error = d.pcm.len() as f64 / 2.0 - d.duration_100ns as f64 * 44100.0 / 1e7;
                if !(-44.1..=2092.1).contains(&error) {
                    return Err(format!("stream {index} duration inconsistent: {error}").into());
                }
                peak = peak.max(d.pcm.iter().map(|&x| i32::from(x).abs()).max().unwrap_or(0));
                rms_max = rms_max.max(rms);
                max_duration = max_duration.max(error.abs());
                let mut boundaries = d.frame_boundaries.iter().copied().peekable();
                for i in 1..d.pcm.len() / 2 {
                    while boundaries.peek().is_some_and(|&v| v < i) {
                        boundaries.next();
                    }
                    let is_boundary = boundaries.peek() == Some(&i);
                    let step =
                        (i32::from(d.pcm[i * 2]) - i32::from(d.pcm[(i - 1) * 2])).abs() as f64;
                    if is_boundary {
                        boundary += step;
                        boundary_n += 1;
                    } else {
                        interior += step;
                        interior_n += 1;
                    }
                }
                passed += 1;
                packets += d.packets;
                frames += d.codec_frames;
                blocks += d.blocks;
                samples += d.pcm.len() / 2;
                println!(
                    "stream={index} packets={} frames={} blocks={} pcm_frames={} rms={rms:.3} duration_error_frames={error:.3} high_fraction={:.6} clipped_fraction={:.6} nonzero_coefficients={}",
                    d.packets,
                    d.codec_frames,
                    d.blocks,
                    d.pcm.len() / 2,
                    sanity.high_frequency_fraction,
                    sanity.clipped_fraction,
                    d.nonzero_coefficients
                );
            }
            Err(e) => {
                failed += 1;
                eprintln!("stream={index} FAILED {e}");
            }
        }
    }
    println!(
        "streams={} passed={passed} failed={failed} packets={packets} frames={frames} blocks={blocks} pcm_frames={samples} peak={peak} rms_max={rms_max:.3} max_duration_error_frames={max_duration:.3} boundary_step={:.3} interior_step={:.3}",
        disc.streams.len(),
        boundary / boundary_n.max(1) as f64,
        interior / interior_n.max(1) as f64
    );
    println!(
        "sanity rms_min={min_rms:.3} high_frequency_max_fraction={max_high:.6} max_dc={max_dc:.3} max_clipped_fraction={max_clip:.6}"
    );
    if failed > 0 || passed != 485 {
        return Err("corpus validation incomplete".into());
    }
    Ok(())
}
