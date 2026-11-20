//! Run only outside sandboxes that block Unicorn's host cache initialization.
//! Writes owner-supplied game output only to the explicit private output path.
use anyhow::{Context, Result, ensure};
use std::{fs::OpenOptions, io::Write, path::Path};
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3,
        "usage: wma_oracle <absolute.iso> <stream-index> <absolute-out.pcm>"
    );
    let index: usize = args[1]
        .parse()
        .context("stream index must be zero-based integer")?;
    let output = Path::new(&args[2]);
    ensure!(output.is_absolute(), "output path must be absolute");
    ensure!(!output.exists(), "output already exists");
    let disc = asf::disc::Disc::open(Path::new(&args[0])).map_err(|e| anyhow::anyhow!("{e}"))?;
    let xbe = disc.read_xbe().map_err(|e| anyhow::anyhow!("{e}"))?;
    let image = wma::VerifiedImage::load(&xbe)?;
    let bytes = disc
        .read_stream(index)
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    let file = asf::Asf::parse(&bytes)?;
    ensure!(file.streams.len() == 1, "stream count");
    let stream = &file.streams[0];
    stream.check_audio_spread()?;
    let format = stream.audio.as_ref().context("no audio")?;
    wma::Format::from_wave(format)?;
    let mut reassembly = asf::Reassembler::new(stream.number, 1 << 20)?;
    let mut packets = Vec::new();
    let mut expected_frames = 0usize;
    for packet in file.packets() {
        for payload in packet?.payloads {
            if let Some(object) = reassembly.push(&payload)? {
                ensure!(
                    object.data.len() % usize::from(format.block_align) == 0,
                    "codec alignment"
                );
                for packet in object.data.chunks_exact(format.block_align.into()) {
                    expected_frames += usize::from(packet[0] & 15);
                    packets.push(packet.to_vec());
                }
            }
        }
    }
    reassembly.finish()?;
    eprintln!(
        "stream={index} resource={} permutation={} bitrate={} packets={} declared_codec_frames={expected_frames}",
        disc.streams[index].resource,
        disc.streams[index].permutation,
        format.bytes_per_second * 8,
        packets.len()
    );
    let mut decoder = wma::oracle::Decoder::new(&image, format, packets)?;
    let pcm = decoder.decode(1_000_000)?;
    ensure!(!pcm.is_empty(), "oracle returned no PCM");
    let mut out = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(output)?;
    for sample in &pcm {
        out.write_all(&sample.to_le_bytes())?;
    }
    out.flush()?;
    println!(
        "oracle stream={index} stereo_frames={} peak={} output={}",
        pcm.len() / 2,
        pcm.iter().map(|&s| i32::from(s).abs()).max().unwrap_or(0),
        output.display()
    );
    Ok(())
}
