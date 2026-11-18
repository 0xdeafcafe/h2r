use super::*;
use crate::game::{
    Frame, FrameAnim, FrameHit, FrameObject, FramePlayer, ObjectKind, normalize_frame,
    validate_frame,
};
use crate::samples;

#[test]
fn bits_and_exp_golomb_codes_round_trip() {
    let mut w = BitWriter::new();
    w.bits(0b101, 3);
    w.bits(u64::from(u32::MAX), 32);
    w.bit(true);
    let numbers = [0u64, 1, 2, 3, 7, 8, 255, 256, 65535, 1 << 40, (1 << 40) - 1];
    for (n, k) in numbers.iter().zip([0u32, 1, 2, 3, 4, 8, 16, 0, 5, 20, 0]) {
        w.eg(*n, k);
    }
    for v in [0i64, -1, 1, -1000, 1000, -(1 << 40), 1 << 40] {
        w.seg(v, 7);
    }
    let len = w.bit_len();
    let bytes = w.finish();
    assert_eq!(bytes.len(), len.div_ceil(8));
    let mut r = BitReader::new(&bytes);
    assert_eq!(r.bits(3).unwrap(), 0b101);
    assert_eq!(r.bits(32).unwrap(), u64::from(u32::MAX));
    assert!(r.bit().unwrap());
    for (n, k) in numbers.iter().zip([0u32, 1, 2, 3, 4, 8, 16, 0, 5, 20, 0]) {
        assert_eq!(r.eg(k).unwrap(), *n, "k {k}");
    }
    for v in [0i64, -1, 1, -1000, 1000, -(1 << 40), 1 << 40] {
        assert_eq!(r.seg(7).unwrap(), v);
    }
    // Exp-Golomb k 0: 0 is one bit, 1 and 2 three.
    let mut w = BitWriter::new();
    w.eg(0, 0);
    assert_eq!(w.bit_len(), 1);
    w.eg(2, 0);
    assert_eq!(w.bit_len(), 4);
    assert_eq!(BitReader::new(&[]).bit(), Err(PackError::Short));
}

#[test]
fn quantisation_stays_inside_its_steps() {
    for i in 0..2000 {
        let x = (i as f64 * 0.7371).sin() * 900.0;
        let q = dpos(qpos([x, -x, x / 3.0]).unwrap());
        assert!((q[0] - x).abs() <= 0.5 / POS_SCALE + 1e-12);
        let yaw = (i as f64 * 0.013) - 13.0;
        let back = dyaw(qyaw(yaw).unwrap());
        let diff = (back - yaw).rem_euclid(TAU);
        let diff = diff.min(TAU - diff);
        assert!(diff <= PI / 4096.0 + 1e-9, "yaw {yaw}: {diff}");
        assert!(back > -PI && back <= PI);
        let a = i as f64 * 0.31;
        let b = (i as f64 * 0.17).sin() * 1.5;
        let v = [a.cos() * b.cos(), a.sin() * b.cos(), b.sin()];
        let u = unoct(oct(v).unwrap());
        let dot: f64 = v.iter().zip(u).map(|(p, q)| p * q).sum();
        let angle = dot.clamp(-1.0, 1.0).acos();
        assert!(angle < 0.0015, "direction off by {angle} rad");
        let len: f64 = u.iter().map(|c| c * c).sum::<f64>().sqrt();
        assert!((len - 1.0).abs() < 1e-12);
    }
    // Straight up and the axes are exact.
    assert_eq!(unoct(oct([0.0, 0.0, 1.0]).unwrap()), [0.0, 0.0, 1.0]);
    assert_eq!(unoct(oct([1.0, 0.0, 0.0]).unwrap()), [1.0, 0.0, 0.0]);
    assert_eq!(unoct(oct([0.0, 0.0, -2.0]).unwrap()), [0.0, 0.0, -1.0]);
    assert!(oct([0.0; 3]).is_err());
    assert!(qpos([f64::NAN, 0.0, 0.0]).is_err());
}

fn stream(frames: &[Frame], every: u64) -> Vec<(Vec<u8>, bool)> {
    let mut e = Encoder::new(3, every);
    frames.iter().map(|f| e.encode(f).unwrap()).collect()
}

#[test]
fn streams_round_trip_to_the_quantised_frames() {
    for frames in [samples::multiplayer(90), samples::campaign(90)] {
        let packed = stream(&frames, 15);
        assert!(packed[0].1, "starts with a keyframe");
        assert_eq!(packed.iter().filter(|(_, k)| *k).count(), 6);
        let mut d = Decoder::new();
        for (frame, (bytes, key)) in frames.iter().zip(&packed) {
            let got = d.decode(bytes).unwrap();
            assert_eq!(got.keyframe, *key);
            assert_eq!(got.sid, 3);
            assert_eq!(got.frame, quantize(frame).unwrap());
            validate_frame(&got.frame).unwrap();
        }
    }
}

#[test]
fn a_dropped_delta_waits_for_the_next_keyframe() {
    let frames = samples::multiplayer(40);
    let packed = stream(&frames, 15);
    let mut d = Decoder::new();
    d.decode(&packed[0].0).unwrap();
    d.decode(&packed[1].0).unwrap();
    // Frame 2 is lost.
    for (bytes, _) in &packed[3..15] {
        assert!(matches!(
            d.decode(bytes),
            Err(PackError::Gap { sid: 3, .. })
        ));
    }
    let key = d.decode(&packed[15].0).unwrap();
    assert!(key.keyframe);
    assert_eq!(key.frame, quantize(&frames[15]).unwrap());
    for (frame, (bytes, _)) in frames[16..].iter().zip(&packed[16..]) {
        assert_eq!(d.decode(bytes).unwrap().frame, quantize(frame).unwrap());
    }
    // A decoder that joins mid-stream starts at a keyframe too.
    let mut late = Decoder::new();
    assert!(matches!(
        late.decode(&packed[20].0),
        Err(PackError::Gap { .. })
    ));
    assert_eq!(
        late.decode(&packed[30].0).unwrap().frame,
        quantize(&frames[30]).unwrap()
    );
}

#[test]
fn new_games_and_backward_ticks_start_a_keyframe() {
    let mut frames = samples::multiplayer(4);
    frames[2].game_id = "another".into();
    frames[3].game_id = "another".into();
    frames[3].tick = frames[2].tick - 10;
    let keys: Vec<bool> = stream(&frames, 100).iter().map(|(_, k)| *k).collect();
    assert_eq!(keys, [true, false, true, true]);
}

#[test]
fn several_streams_decode_side_by_side() {
    let a = samples::multiplayer(20);
    let b = samples::campaign(20);
    let (mut ea, mut eb) = (Encoder::new(1, 15), Encoder::new(2, 15));
    let mut d = Decoder::new();
    for (fa, fb) in a.iter().zip(&b) {
        let pa = ea.encode(fa).unwrap().0;
        let pb = eb.encode(fb).unwrap().0;
        assert_eq!(d.decode(&pb).unwrap().frame, quantize(fb).unwrap());
        assert_eq!(d.decode(&pa).unwrap().frame, quantize(fa).unwrap());
    }
}

/// A frame with every optional field, odd ids and empty lists.
fn odd_frame() -> Frame {
    Frame {
        game_id: "odd-game_1".into(),
        tick: 77,
        players: vec![
            FramePlayer {
                index: 3,
                alive: false,
                pos: [-1.0e5, 0.0, 1.0 / 3.0],
                yaw: -3.0,
                ..FramePlayer::default()
            },
            FramePlayer {
                index: 15,
                alive: true,
                pos: [1.5, -2.25, 1000.0],
                yaw: 2.5,
                vehicle: Some("vehicle_one".into()),
                species: Some("elite".into()),
                aim: Some([0.0, 0.0, -1.0]),
                weapon: Some("energy_blade".into()),
                shots: Some(64),
                stance: Some("dead".into()),
                speed: Some(999.99),
                actions: Some(
                    [
                        "grenade", "melee", "reload", "jump", "switch", "pickup", "jump", "melee",
                    ]
                    .map(String::from)
                    .to_vec(),
                ),
                grenade: Some("plasma".into()),
                vel: Some([-999.0, 0.015625, 12.5]),
                fire: Some(u32::MAX),
                body: Some(1.0 / 3.0),
                shield: Some(4.0),
                seat: Some(63),
                actions_ago: Some(vec![900, 31, 30, 7, 2, 1, 0, 0]),
                anim: Some(FrameAnim {
                    name: "s_kill:front:gut-2".into(),
                    frame: 32767.0,
                }),
            },
        ],
        hits: vec![
            FrameHit {
                from: None,
                to: Some("p10".into()),
                weapon: None,
                pos: [0.0, 0.0, 0.0],
                amount: Some(2.5),
                kind: Some("fall".into()),
                ago: Some(900),
            },
            FrameHit {
                from: Some("p01".into()),
                to: Some("deadbeef".into()),
                ..FrameHit::default()
            },
        ],
        bsp: None,
        cut: Some(123_456),
        objects: vec![
            FrameObject {
                k: ObjectKind::Biped,
                i: 0,
                p: Some([1.0, 2.0, 3.0]),
                f: Some([0.0, 1.0, 0.0]),
                u: Some([0.0, 0.0, 1.0]),
                gone: false,
            },
            FrameObject {
                k: ObjectKind::NetgameEquipment,
                i: 32767,
                p: None,
                f: None,
                u: None,
                gone: true,
            },
        ],
        ..Frame::default()
    }
}

#[test]
fn keyframes_carry_every_field_alone() {
    let frame = odd_frame();
    validate_frame(&frame).unwrap();
    let bytes = encode_keyframe(&frame).unwrap();
    let back = decode_keyframe(&bytes).unwrap();
    assert_eq!(back, quantize(&frame).unwrap());
    assert_eq!(back.players[1].vehicle.as_deref(), Some("vehicle_one"));
    assert_eq!(
        back.hits[1].from.as_deref(),
        Some("p01"),
        "not a canonical player"
    );
    assert_eq!(back.hits[0].to.as_deref(), Some("p10"));
    assert_eq!(back.players[0].pos[2], 171.0 / 512.0);
    // Quantising twice changes nothing.
    assert_eq!(quantize(&back).unwrap(), back);
}

#[test]
fn deltas_of_odd_frames_round_trip() {
    let mut a = odd_frame();
    let mut b = odd_frame();
    b.tick += 2;
    b.bsp = Some(4);
    b.players.swap(0, 1);
    b.players[0].pos[0] += 0.5;
    b.players[1].species = Some("spartan".into());
    b.objects.remove(0);
    b.hits.clear();
    let mut c = b.clone();
    c.tick += 2;
    c.bsp = None;
    c.players.pop();
    a.objects[1].gone = true;
    let mut e = Encoder::new(0, 100);
    let mut d = Decoder::new();
    for f in [&a, &b, &c, &a] {
        let (bytes, _) = e.encode(f).unwrap();
        assert_eq!(d.decode(&bytes).unwrap().frame, quantize(f).unwrap());
    }
}

#[test]
fn unpackable_frames_are_refused_without_moving_the_stream() {
    let frames = samples::multiplayer(3);
    let mut e = Encoder::new(0, 100);
    let mut d = Decoder::new();
    d.decode(&e.encode(&frames[0]).unwrap().0).unwrap();
    let mut bad = frames[1].clone();
    bad.players[0].stance = Some("dancing".into());
    assert_eq!(e.encode(&bad), Err(PackError::Value("unknown stance")));
    let (bytes, key) = e.encode(&frames[2]).unwrap();
    assert!(!key);
    assert_eq!(
        d.decode(&bytes).unwrap().frame,
        quantize(&frames[2]).unwrap()
    );
}

#[test]
fn containers_hold_records() {
    let mut out = Vec::new();
    let big = vec![7u8; 300];
    push_record(&mut out, b"abc");
    push_record(&mut out, &big);
    push_record(&mut out, b"");
    let got: Vec<&[u8]> = records(&out).collect::<Result<_>>().unwrap();
    assert_eq!(got, [&b"abc"[..], &big[..], &b""[..]]);
    assert_eq!(out[4..6], [0xac, 0x02], "300 as LEB128");
    let cut = &out[..out.len() - 200];
    assert!(records(cut).any(|r| r.is_err()));
}

#[test]
fn garbage_never_panics() {
    let frames = samples::campaign(3);
    let good = stream(&frames, 15);
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next = || {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        rng
    };
    for i in 0..3000 {
        let mut bytes = good[i % 3].0.clone();
        match i % 3 {
            0 => {
                let n = (next() % bytes.len() as u64) as usize;
                bytes[n] ^= 1 << (next() % 8);
            }
            1 => bytes.truncate((next() % bytes.len() as u64) as usize),
            _ => bytes = (0..(next() % 64)).map(|_| next() as u8).collect(),
        }
        let mut d = Decoder::new();
        let _ = d.decode(&good[0].0);
        if let Ok(got) = d.decode(&bytes) {
            // Whatever decodes is still a frame the server would check.
            let _ = validate_frame(&got.frame);
        }
    }
}

/// Bytes per frame, averaged, for one sample game.
struct Sizes {
    json: f64,
    keyframe: f64,
    live: f64,
    recorded: f64,
    recorded_deflate: f64,
}

fn sizes(frames: &[Frame]) -> Sizes {
    let mut normal: Vec<Frame> = frames.to_vec();
    normal.iter_mut().for_each(normalize_frame);
    let json: usize = normal
        .iter()
        .map(|f| serde_json::to_vec(f).unwrap().len())
        .sum();
    let live: usize = stream(&normal, 15).iter().map(|(b, _)| b.len()).sum();
    let keys: Vec<Vec<u8>> = normal.iter().map(|f| encode_keyframe(f).unwrap()).collect();
    let keyframe: usize = keys.iter().map(Vec::len).sum();
    // Recorded frames are one a second: every fifteenth.
    let recorded: Vec<&Vec<u8>> = keys.iter().step_by(15).collect();
    let n = frames.len() as f64;
    let r = recorded.len() as f64;
    Sizes {
        json: json as f64 / n,
        keyframe: keyframe as f64 / n,
        live: live as f64 / n,
        recorded: recorded.iter().map(|b| b.len()).sum::<usize>() as f64 / r,
        recorded_deflate: recorded.iter().map(|b| flate2_len(b)).sum::<usize>() as f64 / r,
    }
}

/// Deflate at the default level stands in for a general-purpose
/// compressor on one recorded frame.
fn flate2_len(bytes: &[u8]) -> usize {
    use std::io::Write;
    let mut z = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
    z.write_all(bytes).unwrap();
    z.finish().unwrap().len()
}

#[test]
fn frame_sizes_stay_under_budget() {
    let mp = sizes(&samples::multiplayer(450));
    let cp = sizes(&samples::campaign(450));
    for (name, s) in [("16-player multiplayer", &mp), ("campaign, 60 actors", &cp)] {
        println!(
            "{name}: JSON {:.0} B, keyframe {:.0} B, live (key every 15) {:.0} B, recorded {:.0} B, recorded+deflate {:.0} B",
            s.json, s.keyframe, s.live, s.recorded, s.recorded_deflate
        );
    }
    // Budgets: the live stream at most a tenth of JSON, keyframes at most a
    // quarter. Version 3's velocity, shield and fire ticks cost about a
    // fifth more (226 to 282 B multiplayer live, 651 to 788 B campaign);
    // version 4's animation on every unit another sixth (324 B, 931 B; the
    // frame is sent whole each frame until the chunk codec predicts it).
    assert!(mp.live < 360.0, "multiplayer live {}", mp.live);
    assert!(mp.keyframe < 960.0, "multiplayer keyframe {}", mp.keyframe);
    assert!(cp.live < 1000.0, "campaign live {}", cp.live);
    assert!(cp.keyframe < 2700.0, "campaign keyframe {}", cp.keyframe);
    assert!(mp.live * 10.0 < mp.json && cp.live * 10.0 < cp.json);
}

/// The golden vectors `web/test/fixtures/pack` holds: the TypeScript
/// decoder must read each `.bin` (a container of one stream's records)
/// into the frames of its `.json`. `H2_UPDATE_GOLDEN=1` rewrites them.
#[test]
fn golden_vectors_match_the_encoder() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/pack");
    let mut odd_b = odd_frame();
    odd_b.tick += 2;
    odd_b.players[1].pos[1] -= 3.0;
    odd_b.players[0].alive = true;
    odd_b.objects[0].gone = true;
    odd_b.objects[0].p = None;
    odd_b.objects[0].f = None;
    odd_b.objects[0].u = None;
    let cases: [(&str, Vec<Frame>, u64); 4] = [
        ("multiplayer", samples::multiplayer(6), 4),
        ("campaign", samples::campaign(4), 3),
        ("odd", vec![odd_frame(), odd_b], 15),
        ("cinematic", cinematic_stream(), 100),
    ];
    let update = std::env::var_os("H2_UPDATE_GOLDEN").is_some();
    for (name, frames, every) in cases {
        let mut container = Vec::new();
        let mut decoded = Vec::new();
        let mut e = Encoder::new(5, every);
        let mut d = Decoder::new();
        for f in &frames {
            let bytes = e.encode(f).unwrap().0;
            decoded.push(d.decode(&bytes).unwrap().frame);
            push_record(&mut container, &bytes);
        }
        let json = serde_json::to_string_pretty(&decoded).unwrap() + "\n";
        let (bin_path, json_path) = (
            dir.join(format!("{name}.bin")),
            dir.join(format!("{name}.json")),
        );
        if update {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(&bin_path, &container).unwrap();
            std::fs::write(&json_path, &json).unwrap();
        }
        assert_eq!(
            std::fs::read(&bin_path).ok().as_deref(),
            Some(&container[..]),
            "{name}.bin is stale: run with H2_UPDATE_GOLDEN=1"
        );
        assert_eq!(
            std::fs::read_to_string(&json_path).ok(),
            Some(json),
            "{name}.json is stale"
        );
    }
}

/// A game going into a cinematic and out again: a version 4 frame, two
/// cinematic ones (version 5), a version 4 one.
fn cinematic_stream() -> Vec<Frame> {
    [
        (100, 1, false),
        (102, 1, true),
        (104, 1, true),
        (106, 2, false),
    ]
    .into_iter()
    .map(|(tick, bsp, cinematic)| Frame {
        game_id: "cinematic-golden".into(),
        tick,
        bsp: Some(bsp),
        cinematic,
        ..Frame::default()
    })
    .collect()
}

/// Only a cinematic frame is version 5: every other frame stays version 4,
/// which a reader from before version 5 reads. A change of version is a
/// keyframe each way, since a delta follows a frame of its own version.
#[test]
fn cinematic_frames_alone_are_version_five() {
    let mut frames = samples::campaign(6);
    for f in &mut frames[2..4] {
        *f = Frame {
            game_id: f.game_id.clone(),
            tick: f.tick,
            bsp: f.bsp,
            cinematic: true,
            ..Frame::default()
        };
    }
    let mut e = Encoder::new(3, 100);
    let mut d = Decoder::new();
    let mut seen = Vec::new();
    for f in &frames {
        let (bytes, key) = e.encode(f).unwrap();
        let back = d.decode(&bytes).unwrap().frame;
        assert_eq!(back.cinematic, f.cinematic);
        assert_eq!(back.tick, f.tick);
        seen.push((u64::from(bytes[0] & 0xf), key));
    }
    assert_eq!(
        seen,
        [
            (VERSION, true),
            (VERSION, false),
            (CINEMATIC_VERSION, true),
            (CINEMATIC_VERSION, false),
            (VERSION, true),
            (VERSION, false),
        ]
    );
    // A recorded cinematic frame decodes alone, and a JSON one says so.
    let back = decode_keyframe(&encode_keyframe(&frames[2]).unwrap()).unwrap();
    assert!(back.cinematic && back.players.is_empty());
    let json = serde_json::to_value(&frames[2]).unwrap();
    assert_eq!(json["cinematic"], true);
    assert!(
        serde_json::to_value(&frames[0])
            .unwrap()
            .get("cinematic")
            .is_none()
    );
    assert_eq!(validate_frame(&frames[2]), Ok(()));
}

/// CPU per frame: what the game's sink worker spends serialising (JSON)
/// or packing, and what the server spends taking a frame in (parse or
/// decode, validate, normalise) and passing it on (JSON per reader, or one
/// packed stream frame plus a recorded keyframe a second). Run with
/// `cargo test -p live-proto --release pack::tests::measure -- --ignored
/// --nocapture` for the numbers in docs/live/live-telemetry.md.
#[test]
#[ignore]
fn measure_cpu_per_frame() {
    use std::time::Instant;
    for (name, frames) in [
        ("multiplayer", samples::multiplayer(900)),
        ("campaign", samples::campaign(900)),
    ] {
        let n = frames.len() as u32;
        let start = Instant::now();
        let texts: Vec<String> = frames
            .iter()
            .map(|f| {
                serde_json::to_string(&crate::ClientMessage::Frame {
                    frame: Box::new(f.clone()),
                })
                .unwrap()
            })
            .collect();
        let json_out = start.elapsed() / n;
        let start = Instant::now();
        let mut enc = Encoder::new(0, 150);
        let packed: Vec<Vec<u8>> = frames.iter().map(|f| enc.encode(f).unwrap().0).collect();
        let pack_out = start.elapsed() / n;
        let start = Instant::now();
        for (i, text) in texts.iter().enumerate() {
            let crate::ClientMessage::Frame { frame } = serde_json::from_str(text).unwrap() else {
                unreachable!()
            };
            let mut frame = *frame;
            validate_frame(&frame).unwrap();
            normalize_frame(&mut frame);
            std::hint::black_box(serde_json::to_string(&frame).unwrap());
            if i % 15 == 0 {
                std::hint::black_box(serde_json::to_value(&frame).unwrap());
            }
        }
        let json_in = start.elapsed() / n;
        let start = Instant::now();
        let mut d = Decoder::new();
        let mut out = Encoder::new(1, 15);
        for (i, bytes) in packed.iter().enumerate() {
            let mut frame = d.decode(bytes).unwrap().frame;
            validate_frame(&frame).unwrap();
            normalize_frame(&mut frame);
            std::hint::black_box(out.encode(&frame).unwrap());
            if i % 15 == 0 {
                std::hint::black_box(to_base64(&encode_keyframe(&frame).unwrap()));
            }
        }
        let pack_in = start.elapsed() / n;
        println!(
            "{name}: sender JSON {json_out:?} packed {pack_out:?}; server JSON {json_in:?} packed {pack_in:?}"
        );
    }
}

/// Version 1 frames (recorded before vehicles carried a parent and aim, or
/// sent by an older game) still decode: `v1-multiplayer` is the golden
/// stream version 1's encoder wrote. A version 1 delta never applies to a
/// version 2 frame.
#[test]
fn version_one_frames_still_decode() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/pack");
    let bin = std::fs::read(dir.join("v1-multiplayer.bin")).unwrap();
    let want: Vec<Frame> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("v1-multiplayer.json")).unwrap())
            .unwrap();
    let recs: Vec<&[u8]> = records(&bin).collect::<Result<_>>().unwrap();
    assert_eq!(recs[0][0] & 0xf, 1, "a version 1 stream");
    let mut d = Decoder::new();
    let got: Vec<Frame> = recs.iter().map(|r| d.decode(r).unwrap().frame).collect();
    assert_eq!(got, want);
    assert!(
        got.iter()
            .flat_map(|f| &f.vehicles)
            .all(|v| v.parent.is_none() && v.aim.is_none())
    );
    // The same stream id moving to version 2 needs a keyframe of its own.
    let mut e = Encoder::new(5, 100);
    let mut d = Decoder::new();
    d.decode(recs[0]).unwrap();
    let frames = samples::multiplayer(2);
    let _ = e.encode(&frames[0]).unwrap();
    let (delta, key) = e.encode(&frames[1]).unwrap();
    assert!(!key);
    assert!(matches!(d.decode(&delta), Err(PackError::Gap { .. })));
}

#[test]
fn a_turret_keeps_its_parent_and_aim() {
    let frames = samples::multiplayer(3);
    let mut e = Encoder::new(1, 100);
    let mut d = Decoder::new();
    for f in &frames {
        let back = d.decode(&e.encode(f).unwrap().0).unwrap().frame;
        let turret = back.vehicles.iter().find(|v| v.kind == "chaingun").unwrap();
        let sent = f.vehicles.iter().find(|v| v.kind == "chaingun").unwrap();
        assert_eq!(turret.parent.as_deref(), Some("90040004"));
        let (a, b) = (turret.aim.unwrap(), sent.aim.unwrap());
        assert!((0..3).all(|k| (a[k] - b[k]).abs() < 2e-3), "{a:?} {b:?}");
        assert!(
            back.vehicles
                .iter()
                .filter(|v| v.kind != "chaingun")
                .all(|v| v.parent.is_none() && v.aim.is_none())
        );
    }
    assert_eq!(validate_frame(&frames[0]), Ok(()));
    let mut bad = frames[0].clone();
    let t = bad
        .vehicles
        .iter_mut()
        .find(|v| v.kind == "chaingun")
        .unwrap();
    t.parent = Some(t.id.clone());
    assert!(validate_frame(&bad).is_err(), "its own parent");
}

/// Version 2 frames (recorded before version 3's unit, hit and cut fields)
/// still decode: `v2-multiplayer` is the golden stream version 2's encoder
/// wrote. A version 2 delta never applies to a version 3 frame.
#[test]
fn version_two_frames_still_decode() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/pack");
    let bin = std::fs::read(dir.join("v2-multiplayer.bin")).unwrap();
    let want: Vec<Frame> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("v2-multiplayer.json")).unwrap())
            .unwrap();
    let recs: Vec<&[u8]> = records(&bin).collect::<Result<_>>().unwrap();
    assert_eq!(recs[0][0] & 0xf, 2, "a version 2 stream");
    let mut d = Decoder::new();
    let got: Vec<Frame> = recs.iter().map(|r| d.decode(r).unwrap().frame).collect();
    assert_eq!(got, want);
    assert!(
        got.iter()
            .all(|f| f.cut.is_none() && f.hits.iter().all(|h| h.ago.is_none()))
    );
    assert!(
        got.iter()
            .flat_map(|f| &f.players)
            .all(|p| p.vel.is_none() && p.fire.is_none())
    );
    let mut e = Encoder::new(5, 100);
    let mut d = Decoder::new();
    d.decode(recs[0]).unwrap();
    let frames = samples::multiplayer(2);
    let _ = e.encode(&frames[0]).unwrap();
    let (delta, key) = e.encode(&frames[1]).unwrap();
    assert!(!key);
    assert!(matches!(d.decode(&delta), Err(PackError::Gap { .. })));
}

#[test]
fn version_three_fields_round_trip_quantised() {
    let frame = odd_frame();
    let back = decode_keyframe(&encode_keyframe(&frame).unwrap()).unwrap();
    let p = &back.players[1];
    assert_eq!(back.cut, Some(123_456));
    assert_eq!(p.fire, Some(u32::MAX));
    assert_eq!(p.seat, Some(63));
    assert_eq!(
        p.actions_ago.as_deref(),
        Some(&[900, 31, 30, 7, 2, 1, 0, 0][..])
    );
    assert_eq!(
        p.vel,
        Some([-999.0, 0.015625, 12.5]),
        "multiples of 1/64 are exact"
    );
    assert!((p.body.unwrap() - 1.0 / 3.0).abs() <= 0.5 / 63.0);
    assert_eq!(p.shield, Some(4.0));
    assert_eq!(back.hits[0].ago, Some(900));
    assert_eq!(back.players[0].vel, None);
    // Actions ticks without their actions can't be packed.
    let mut bad = frame.clone();
    bad.players[1].actions_ago = Some(vec![1]);
    assert!(encode_keyframe(&bad).is_err());
    assert!(validate_frame(&bad).is_err());
}

#[test]
fn a_cut_is_on_its_own_frame_through_deltas() {
    let mut frames = samples::campaign(4);
    frames[2].cut = Some(42);
    let mut e = Encoder::new(9, 100);
    let mut d = Decoder::new();
    let cuts: Vec<Option<u64>> = frames
        .iter()
        .map(|f| {
            let (bytes, _) = e.encode(f).unwrap();
            d.decode(&bytes).unwrap().frame.cut
        })
        .collect();
    assert_eq!(cuts, [None, None, Some(42), None]);
}

#[test]
fn version_three_values_are_checked() {
    let base = || odd_frame();
    let mut f = base();
    f.players[1].body = Some(1.5);
    assert!(validate_frame(&f).is_err());
    let mut f = base();
    f.players[1].shield = Some(-0.1);
    assert!(validate_frame(&f).is_err());
    let mut f = base();
    f.players[1].seat = Some(64);
    assert!(validate_frame(&f).is_err());
    let mut f = base();
    f.players[1].vel = Some([f64::NAN, 0.0, 0.0]);
    assert!(validate_frame(&f).is_err());
    let mut f = base();
    f.hits[0].ago = Some(901);
    assert!(validate_frame(&f).is_err());
    let mut f = base();
    f.players[1].actions_ago = Some(vec![0; 8].into_iter().chain([901]).skip(1).collect());
    assert!(validate_frame(&f).is_err());
    assert_eq!(validate_frame(&base()), Ok(()));
}

/// Version 3 frames (before units' `anim`) still decode: `v3-multiplayer`
/// is the stream version 3's encoder wrote.
#[test]
fn version_three_frames_still_decode() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../web/test/fixtures/pack");
    let bin = std::fs::read(dir.join("v3-multiplayer.bin")).unwrap();
    let want: Vec<Frame> =
        serde_json::from_str(&std::fs::read_to_string(dir.join("v3-multiplayer.json")).unwrap())
            .unwrap();
    let recs: Vec<&[u8]> = records(&bin).collect::<Result<_>>().unwrap();
    assert_eq!(recs[0][0] & 0xf, 3, "a version 3 stream");
    let mut d = Decoder::new();
    let got: Vec<Frame> = recs.iter().map(|r| d.decode(r).unwrap().frame).collect();
    assert_eq!(got, want);
    assert!(
        got.iter()
            .flat_map(|f| &f.players)
            .all(|p| p.anim.is_none() && p.vel.is_some())
    );
}

#[test]
fn an_animation_round_trips_and_needs_a_good_name() {
    let back = decode_keyframe(&encode_keyframe(&odd_frame()).unwrap()).unwrap();
    let anim = back.players[1].anim.as_ref().unwrap();
    assert_eq!(
        (anim.name.as_str(), anim.frame),
        ("s_kill:front:gut-2", 32767.0)
    );
    // Frames are kept to a sixteenth.
    let mut f = odd_frame();
    f.players[1].anim.as_mut().unwrap().frame = 12.53;
    let back = decode_keyframe(&encode_keyframe(&f).unwrap()).unwrap();
    assert_eq!(back.players[1].anim.as_ref().unwrap().frame, 12.5);
    for (name, frame) in [
        ("Combat:Idle", 1.0),
        ("", 1.0),
        ("a b", 1.0),
        ("idle", -1.0),
        ("idle", 40000.0),
    ] {
        let mut f = odd_frame();
        f.players[1].anim = Some(FrameAnim {
            name: name.into(),
            frame,
        });
        assert!(validate_frame(&f).is_err(), "{name} {frame}");
    }
}
