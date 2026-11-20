#![cfg(feature = "oracle")]
//! Native WMAv2 against the game's own decoder under Unicorn, on the owner's
//! retail disc. Ignored: it reads the disc by absolute path and nothing from it
//! is stored in the repository. Run with
//! `cargo test -p wma --features oracle --release --test oracle_disc -- --ignored`.
use std::path::Path;

const ISO: &str = "/Users/afr/Source/github.com/0xdeafcafe/h2recomp/disc/halo2.iso";

fn packets(bytes: &[u8]) -> Vec<Vec<u8>> {
    let file = asf::Asf::parse(bytes).unwrap();
    let stream = &file.streams[0];
    let align = usize::from(stream.audio.as_ref().unwrap().block_align);
    let mut reassembly = asf::Reassembler::new(stream.number, 1 << 20).unwrap();
    let mut out = Vec::new();
    for packet in file.packets() {
        for payload in packet.unwrap().payloads {
            if let Some(object) = reassembly.push(&payload).unwrap() {
                out.extend(object.data.chunks_exact(align).map(<[u8]>::to_vec));
            }
        }
    }
    reassembly.finish().unwrap();
    out
}

#[test]
#[ignore = "needs the retail disc and Unicorn"]
fn native_matches_game_decoder_within_one_lsb() {
    let disc = asf::disc::Disc::open(Path::new(ISO)).unwrap();
    let xbe = disc.read_xbe().unwrap();
    let image = wma::VerifiedImage::load(&xbe).unwrap();
    let tables = wma::Tables::load(&image).unwrap();
    // 128 kbps: stream 0 is the shortest of 0..4, the only 128 kbps streams
    //. 96 kbps: variable block sizes (5), a
    // zero-frame reservoir packet (38), and two streams whose first divergence
    // was inside short blocks.
    for (index, bytes_per_second) in [
        (0, 16002),
        (5, 12003),
        (38, 12003),
        (154, 12003),
        (226, 12003),
    ] {
        let bytes = disc.read_stream(index).unwrap();
        let native = wma::decode_asf(&tables, &bytes, 128 * 1024 * 1024)
            .unwrap()
            .pcm;
        let file = asf::Asf::parse(&bytes).unwrap();
        let format = file.streams[0].audio.as_ref().unwrap();
        assert_eq!(
            format.bytes_per_second, bytes_per_second,
            "stream {index} profile"
        );
        let reference = wma::oracle::Decoder::new(&image, format, packets(&bytes))
            .unwrap()
            .decode(1_000_000)
            .unwrap();
        let c = wma::diagnostics::compare(&native, &reference);
        assert_eq!(native.len(), reference.len(), "stream {index} length");
        assert!(
            c.max_abs_error <= 1,
            "stream {index}: max error {} snr {:.1} dB",
            c.max_abs_error,
            c.snr_db
        );
    }
}
