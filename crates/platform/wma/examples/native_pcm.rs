//! Native counterpart of examples/wma_oracle.rs, with identical indices.
use std::{fs::OpenOptions, io::Write, path::Path};
fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 3 {
        return Err("usage: native_pcm <absolute.iso> <stream-index> <absolute-out.pcm>".into());
    }
    let index: usize = args[1].parse()?;
    let output = Path::new(&args[2]);
    if !output.is_absolute() || output.exists() {
        return Err("output must be an absolute new private file".into());
    }
    let disc = asf::disc::Disc::open(Path::new(&args[0]))?;
    let xbe = disc.read_xbe()?;
    let image = wma::VerifiedImage::load(&xbe)?;
    let tables = wma::Tables::load(&image)?;
    let bytes = disc.read_stream(index)?;
    let decoded = wma::decode_asf(&tables, &bytes, 128 * 1024 * 1024)?;
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    for sample in &decoded.pcm {
        out.write_all(&sample.to_le_bytes())?;
    }
    out.flush()?;
    println!(
        "native stream={index} stereo_frames={} codec_frames={} blocks={}",
        decoded.pcm.len() / 2,
        decoded.codec_frames,
        decoded.blocks
    );
    Ok(())
}
