//! Time only the production encoding path, in both orders in one release binary.
use live_proto::{game::Frame, pack, samples};
use std::{hint::black_box, path::PathBuf, time::Instant};

fn encode(frames: &[Frame], version: u8) -> (usize, f64) {
    let mut writer = pack::Encoder::with_version(0, 150, version).unwrap();
    let mut packet = Vec::new();
    let start = Instant::now();
    let mut total = 0;
    for frame in frames {
        let (record, _) = writer.encode(black_box(frame)).unwrap();
        packet.clear();
        pack::push_record(&mut packet, &record);
        total += black_box(packet.as_slice()).len();
    }
    (
        total,
        start.elapsed().as_secs_f64() * 1e6 / frames.len() as f64,
    )
}

#[test]
#[ignore = "quiet-machine release acceptance of the negotiated writer"]
fn quiet_v8_pair() {
    let frames = samples::multiplayer(9000);
    let mut enc = pack::Encoder::with_version(0, 150, 8).unwrap();
    let mut dec = pack::Decoder::new();
    // H2_V8_DUMP names one file containing the same ULEB-framed records as a socket.
    let mut dump = std::env::var_os("H2_V8_DUMP").map(|p| (PathBuf::from(p), Vec::new()));
    let mut join_bytes = 0;
    for (i, f) in frames.iter().enumerate() {
        let (record, key) = enc.encode(f).unwrap();
        if i == 4800 {
            assert!(key);
            dec = pack::Decoder::new();
            let mut joined = Vec::new();
            pack::push_record(&mut joined, &record);
            join_bytes = joined.len();
        }
        assert_eq!(
            dec.decode(&record).unwrap().frame,
            pack::quantize(f).unwrap()
        );
        if let Some((_, bytes)) = &mut dump {
            pack::push_record(bytes, &record);
        }
    }
    let dump_len = dump.map(|(path, bytes)| {
        assert_eq!(
            pack::records(&bytes).map(Result::unwrap).count(),
            frames.len()
        );
        std::fs::write(&path, &bytes).unwrap_or_else(|e| panic!("write {}: {e}", path.display()));
        println!(
            "dumped {} verified records ({} bytes) to {}",
            frames.len(),
            bytes.len(),
            path.display()
        );
        bytes.len()
    });
    let mut ratios = Vec::new();
    for i in 0..6 {
        let ((v7b, v7), (v8b, v8)) = if i % 2 == 0 {
            (encode(&frames, 7), encode(&frames, 8))
        } else {
            let b = encode(&frames, 8);
            (encode(&frames, 7), b)
        };
        assert_eq!(v7b, 1_394_368);
        if let Some(n) = dump_len {
            assert_eq!(v8b, n);
        }
        assert!(v8b < 85 * 9600);
        ratios.push(v8 / v7);
        println!(
            "pair {} v7_bytes={v7b} v8_bytes={v8b} v8_B/player/s={:.3} v7_us/frame={v7:.3} v8_us/frame={v8:.3} ratio={:.3} join_bytes={join_bytes}",
            i + 1,
            v8b as f64 / 9600.,
            v8 / v7
        );
    }
    ratios.sort_by(f64::total_cmp);
    let median = (ratios[2] + ratios[3]) / 2.;
    println!("median paired v8/v7 encode time {median:.3}");
    assert!(
        median <= 1.0,
        "v8 has not passed the quiet-machine shipping gate"
    );
}

#[test]
fn v8_mixed_recording_survives_chunk_replay_and_archiving() {
    use live_proto::{archive, chunk, game::MachinePosition};
    let mut frames = samples::multiplayer(5);
    frames[0].machine_positions = vec![MachinePosition { i: 11, p: 64 }];
    frames[2].machine_positions = vec![MachinePosition { i: 11, p: 128 }];
    frames[3].machine_positions = vec![MachinePosition { i: 17, p: 0 }];
    let expected: Vec<_> = frames.iter().map(|f| pack::quantize(f).unwrap()).collect();
    let mut v7 = pack::Encoder::new(6, 150);
    let mut v8 = pack::Encoder::with_version(6, 150, 8).unwrap();
    let mut container = Vec::new();
    for (i, frame) in frames.iter().enumerate() {
        let encoder = if i < 2 { &mut v7 } else { &mut v8 };
        pack::push_record(&mut container, &encoder.encode(frame).unwrap().0);
    }
    for codec in [chunk::Codec::Stored, chunk::Codec::Deflate] {
        let chunk = chunk::seal(&container, 5, frames[0].tick, frames[4].tick, codec);
        let recovered = chunk::frames(&chunk).unwrap();
        assert_eq!(recovered, expected);
        let stored = archive::encode(&recovered).unwrap();
        archive::verify(&expected, &stored).unwrap();
        assert_eq!(archive::decode(&stored).unwrap(), expected);
    }
}
