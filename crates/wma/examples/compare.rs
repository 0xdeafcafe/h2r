//! Compare signed little-endian i16 stereo PCM, without automatic alignment.
use std::{fs, io::Read};
fn read(path: &std::ffi::OsStr) -> Result<Vec<i16>, Box<dyn std::error::Error>> {
    let mut file = fs::File::open(path)?;
    if file.metadata()?.len() > 256 * 1024 * 1024 {
        return Err("PCM exceeds 256 MiB".into());
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.is_empty() || bytes.len() % 4 != 0 {
        return Err("PCM must contain complete stereo i16 frames".into());
    }
    Ok(bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| i16::from_le_bytes([b[0], b[1]]))
        .collect())
}
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err("usage: compare <native.pcm> <oracle.pcm>".into());
    }
    let native = read(&args[0])?;
    let reference = read(&args[1])?;
    let c = wma::diagnostics::compare(&native, &reference);
    println!(
        "native_frames={} oracle_frames={} compared_frames={} max_abs_error={} rmse={:.6} snr_db={:.3}",
        c.native_samples / 2,
        c.reference_samples / 2,
        c.compared_samples / 2,
        c.max_abs_error,
        c.rmse,
        c.snr_db
    );
    if native.len() != reference.len() {
        return Err("length mismatch: measurements cover only the common prefix; no alignment or trimming applied".into());
    }
    Ok(())
}
