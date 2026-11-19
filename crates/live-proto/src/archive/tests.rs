use super::*;
use crate::{chunk, game::FramePlayer, pack, samples};
use std::{fs, path::Path};

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/pack")
}
#[test]
fn historic_streams_and_chunks_go_directly_to_the_archive() {
    for name in [
        "v1-multiplayer",
        "v2-multiplayer",
        "v3-multiplayer",
        "v4-multiplayer",
        "v4-cinematic",
        "multiplayer",
        "v6-multiplayer",
        "v6-odd",
        "v6-chunk",
        "v7-motion",
        "v7-chunk",
        "v7-campaign",
        "v7-odd",
    ] {
        let bytes = fs::read(fixtures().join(format!("{name}.bin"))).unwrap();
        let frames = if name.ends_with("chunk") {
            chunk::frames(&bytes).unwrap()
        } else {
            let mut d = pack::Decoder::new();
            pack::records(&bytes)
                .map(|r| d.decode(r.unwrap()).unwrap().frame)
                .collect()
        };
        let stored = encode(&frames).unwrap();
        verify(&frames, &stored).unwrap();
        if name == "v6-multiplayer" {
            assert!(
                frames
                    .iter()
                    .flat_map(|f| &f.players)
                    .filter_map(|p| p.aim)
                    .any(|a| pack::unoct(pack::oct(a).unwrap()) != a)
            );
        }
    }
}

#[test]
fn json_numbers_optional_events_and_random_access() {
    let mut first = Frame {
        game_id: "historic".into(),
        tick: u64::MAX - 3,
        players: vec![FramePlayer {
            pos: [1.0 / 65536.0, -0.0, 1.0 / 37.0],
            yaw: 0.0223,
            aim: Some([0.25, -0.5, 0.866]),
            actions: Some(vec![]),
            shots: Some(0),
            ..Default::default()
        }],
        ..Default::default()
    };
    let a = encode(&[first.clone()]).unwrap();
    first.tick += 1;
    let b = encode(&[first.clone()]).unwrap();
    verify(std::slice::from_ref(&first), &b).unwrap();
    assert!(matches!(verify(&[first], &a), Err(Error::Mismatch(0))));
    let f = decode(&b).unwrap().pop().unwrap();
    assert_eq!(f.players[0].pos[1].to_bits(), (-0.0f64).to_bits());
    assert!(decode(&b[..b.len() / 2]).is_err());
}

fn scene(frames: Vec<Frame>, name: &str) -> (usize, usize) {
    let mut w = chunk::ChunkWriter::default();
    let mut chunks: Vec<_> = frames.iter().filter_map(|f| w.push(f)).collect();
    chunks.extend(w.finish());
    drop(frames);
    let old: usize = chunks.iter().map(|c| c.bytes.len()).sum();
    let mut total = 0;
    let mut pending = Vec::new();
    let mut source_bytes = 0;
    let mut i = 0;
    let mut seal = |frames: &[Frame]| {
        let start = std::time::Instant::now();
        let stored = encode(frames).unwrap();
        let encoded = start.elapsed();
        verify(frames, &stored).unwrap();
        eprintln!(
            "{name} segment={i} frames={} bytes={} encode_ms={:.1}",
            frames.len(),
            stored.len(),
            encoded.as_secs_f64() * 1000.
        );
        if let Some(dir) = std::env::var_os("H2_ARCHIVE_DUMP") {
            let dir = Path::new(&dir);
            fs::create_dir_all(dir).unwrap();
            fs::write(dir.join(format!("{name}-{i:03}.br")), &stored).unwrap();
            fs::write(
                dir.join(format!("{name}-{i:03}.json")),
                serde_json::to_vec(&frames).unwrap(),
            )
            .unwrap();
        }
        total += stored.len();
        i += 1;
    };
    for frame in chunks
        .into_iter()
        .flat_map(|c| chunk::frames(&c.bytes).unwrap())
    {
        let size = serde_json::to_vec(&frame).unwrap().len();
        if starts_segment(&pending, source_bytes, &frame, size) {
            seal(&pending);
            pending.clear();
            source_bytes = 0;
        }
        source_bytes += size;
        pending.push(frame);
    }
    if !pending.is_empty() {
        seal(&pending);
    }
    eprintln!(
        "{name}: stored_v7={old} archive={total} ratio={:.4}",
        total as f64 / old as f64
    );
    (total, old)
}
#[test]
#[ignore = "release size and timing run"]
fn bench_archive_scenes() {
    for (name, frames) in [
        ("mp16", samples::multiplayer(9000)),
        ("campaign", samples::campaign(450)),
    ] {
        let (archive, stored) = scene(frames, name);
        assert!(archive * 4 <= stored, "{name}: {archive} vs {stored}");
    }
}
#[test]
fn archive_size_budget() {
    let (archive, stored) = scene(samples::multiplayer(450), "mp16");
    assert!(archive * 4 <= stored, "{archive} vs {stored}");
}

#[test]
fn segment_boundaries_follow_source_order_and_memory_budget() {
    let frame = |tick| Frame {
        game_id: "game".into(),
        tick,
        ..Default::default()
    };
    let open = [frame(40), frame(68)];
    assert!(!starts_segment(
        &open,
        MAX_SOURCE_BYTES - 80,
        &frame(69),
        80
    ));
    assert!(starts_segment(&open, MAX_SOURCE_BYTES - 80, &frame(69), 81));
    assert!(starts_segment(&open, 0, &frame(40 + SPAN_TICKS), 1));
    assert!(starts_segment(&open, 0, &frame(68), 1));
    assert!(starts_segment(&open, 0, &frame(65), 1));
    let cut = Frame {
        cut: Some(17),
        ..frame(70)
    };
    assert!(starts_segment(&open, 0, &cut, 1));
    assert!(!starts_segment(&[], 0, &cut, MAX_SOURCE_BYTES + 1));
}

#[test]
fn invalid_columns_fail_before_expanding_their_payload() {
    fn rows(bytes: &[u8], n: usize) -> Result<Vec<Value>> {
        let mut r = Reader {
            bytes,
            nodes: MAX_NODES,
            strings: MAX_STRINGS,
        };
        read_node(&mut r, n, 0)
    }
    // An array of 4097 slots per row, a negative string dictionary index,
    // a float with infinite IEEE bits, and more nodes than a segment allows.
    let mut oversized = vec![1, 0];
    put(&mut oversized, 4097 << 1);
    assert!(rows(&oversized, 900).is_err());
    assert!(rows(&[2, 2, 1, b'a', 1, b'b', 0, 1], 2).is_err());
    let mut inf = vec![5, 0, 0];
    put(&mut inf, f64::INFINITY.to_bits() << 1);
    assert!(rows(&inf, 1).is_err());
    assert!(rows(&[0, 0], MAX_NODES + 1).is_err());
}
