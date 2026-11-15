//! Decode one private ASF/WMAv2 file through the bounded `decode_asf` API.
use std::{fs, io::Write, path::PathBuf};
/// Interleaved sample cap, matching the CLI exporter.
const MAX_SAMPLES: usize = 128 * 1024 * 1024;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let a: Vec<_> = std::env::args_os().skip(1).collect();
    if a.len() < 2 {
        return Err("usage: decode absolute.xbe private.wma [out.pcm]".into());
    }
    let xbe = fs::read(&a[0])?;
    let image = wma::VerifiedImage::load(&xbe)?;
    let tables = wma::Tables::load(&image)?;
    let data = fs::read(&a[1])?;
    let decoded = wma::decode_asf(&tables, &data, MAX_SAMPLES)?;
    let pcm = &decoded.pcm;
    println!(
        "packets={} frames={} blocks={} samples={} peak={} rms={:.2}",
        decoded.packets,
        decoded.codec_frames,
        decoded.blocks,
        pcm.len() / 2,
        pcm.iter().map(|&x| i32::from(x).abs()).max().unwrap_or(0),
        (pcm.iter().map(|&x| f64::from(x).powi(2)).sum::<f64>() / pcm.len().max(1) as f64).sqrt()
    );
    if let Some(p) = a.get(2) {
        let p = PathBuf::from(p);
        if !p.is_absolute() {
            return Err("output must be absolute".into());
        }
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(p)?;
        for s in pcm {
            f.write_all(&s.to_le_bytes())?;
        }
    }
    Ok(())
}
