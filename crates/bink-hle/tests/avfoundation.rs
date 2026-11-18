#![cfg(target_os = "macos")]
#[path = "../../movies/tests/support/mod.rs"]
mod support;
use bink_hle::{AvFoundationDecoder, MovieDecoder, MovieSource, TranscodedSource};
use movies::install::{InstallOptions, InstallStatus, install};
use std::sync::Arc;
use support::{Temp, disc, media};

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn generated_movie_installs_and_decodes_via_avfoundation_without_overrides() {
    let temp = Temp::new();
    let bytes = media(
        &temp.0.join("pattern.nut"),
        "color=black:size=30x22:rate=30000/1001,geq=r=N*40:g=0:b=255-N*40",
    );
    let options = InstallOptions::new(temp.0.join("data/movies"));
    assert_eq!(
        install(&mut disc(&[("synthetic.bik", &bytes)]), &options)
            .unwrap()
            .status,
        InstallStatus::Complete
    );
    let source = TranscodedSource::new(&options.data_dir, Arc::new(AvFoundationDecoder));
    assert!(source.open("absent").is_none());
    let movie = movies::locate(&options.data_dir, "synthetic").unwrap();
    let mut stream = AvFoundationDecoder
        .try_open(&movie)
        .expect("AVFoundation opens transcoded MOV");
    assert!(source.open("synthetic").is_some());
    let info = stream.info();
    assert_eq!((info.width, info.height, info.frames), (30, 22, 6));
    assert!((info.fps_num as f64 / info.fps_den as f64 - 30000.0 / 1001.0).abs() < 0.00001);
    let mut frame = vec![0; info.width as usize * info.height as usize * 4];
    assert!(stream.decode(0, &mut frame));
    assert!(frame.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
    assert!(frame.as_chunks::<4>().0.iter().any(|p| p[0] != p[1]));
    let first = frame.clone();
    for index in 1..info.frames {
        assert!(stream.decode(index, &mut frame));
    }
    assert!(frame != first, "generated video changes across frames");
    assert!(stream.decode(0, &mut frame), "rewind for loops");
    assert_eq!(frame, first);
    assert!(stream.decode(0, &mut frame), "repeat the current frame");
    let before = frame.clone();
    assert!(!stream.decode(info.frames, &mut frame));
    assert_eq!(frame, before, "invalid decode retains the last picture");
    assert!(!stream.decode(0, &mut [0; 1]));
    drop(stream);
    assert!(
        AvFoundationDecoder
            .open(&temp.0.join("missing.mov"))
            .is_none()
    );
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn generated_red_frame_is_bgra_and_stream_can_move_threads() {
    let temp = Temp::new();
    let bytes = media(&temp.0.join("red.nut"), "color=red:size=32x24:rate=30");
    let options = InstallOptions::new(temp.0.join("data/movies"));
    install(&mut disc(&[("red.bik", &bytes)]), &options).unwrap();
    let source = TranscodedSource::new(&options.data_dir, Arc::new(AvFoundationDecoder));
    let mut stream = source.open("red").unwrap();
    let pixel = std::thread::spawn(move || {
        let mut frame = vec![0; 32 * 24 * 4];
        assert!(stream.decode(0, &mut frame));
        frame[..4].to_vec()
    })
    .join()
    .unwrap();
    assert!(
        pixel[0] < 20 && pixel[1] < 20 && pixel[2] > 230 && pixel[3] == 255,
        "BGRA red: {pixel:?}"
    );
}

fn installed_rate(filter: &str, seconds: &str) -> (u32, u32, u32) {
    let temp = Temp::new();
    let bytes = support::media_for(&temp.0.join("r.nut"), filter, seconds);
    let options = InstallOptions::new(temp.0.join("data/movies"));
    install(&mut disc(&[("r.bik", &bytes)]), &options).unwrap();
    let source = TranscodedSource::new(&options.data_dir, Arc::new(AvFoundationDecoder));
    let stream = source.open("r").expect("constant-rate movie opens");
    let info = stream.info();
    (info.frames, info.fps_num, info.fps_den)
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn real_transcodes_at_30_and_29_97_fps_are_accepted() {
    for (rate, seconds) in [
        ("30", "1"),
        ("30000/1001", "1"),
        ("30", "0.55"),
        ("30000/1001", "0.55"),
    ] {
        let (frames, num, den) =
            installed_rate(&format!("testsrc2=size=32x24:rate={rate}"), seconds);
        let want = if rate == "30" { 30.0 } else { 30000.0 / 1001.0 };
        assert!(frames > 10, "{rate} {seconds}: {frames} frames");
        assert!(
            (num as f64 / den as f64 - want).abs() < 0.001,
            "{rate} {seconds}: {num}/{den}"
        );
    }
}

#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn generated_sound_track_reads_as_stereo_pcm_and_rewinds() {
    let temp = Temp::new();
    // 0.5 s of video with a 440 Hz tone at 44.1 kHz.
    let bytes = support::media_for(
        &temp.0.join("s.nut"),
        "color=blue:size=32x24:rate=30",
        "0.5",
    );
    let options = InstallOptions::new(temp.0.join("data/movies"));
    install(&mut disc(&[("s.bik", &bytes)]), &options).unwrap();
    let source = TranscodedSource::new(&options.data_dir, Arc::new(AvFoundationDecoder));
    let mut stream = source.open("s").unwrap();
    let audio = stream.audio_info().expect("the AAC track is readable");
    assert_eq!((audio.sample_rate, audio.channels), (44_100, 2));
    let mut all = Vec::new();
    let mut buf = vec![0i16; 441 * 2];
    loop {
        let n = stream.read_audio(&mut buf);
        if n == 0 {
            break;
        }
        all.extend_from_slice(&buf[..n * 2]);
    }
    let frames = all.len() / 2;
    // 0.5 s, give or take the encoder's padding.
    assert!((20_000..=25_000).contains(&frames), "{frames} frames");
    let peak = all.iter().map(|s| s.unsigned_abs()).max().unwrap();
    assert!(peak > 1000, "a tone, not silence: peak {peak}");
    // Video reads do not disturb the sound, and a rewind starts it over.
    let mut frame = vec![0; 32 * 24 * 4];
    assert!(stream.decode(3, &mut frame));
    assert!(stream.rewind_audio());
    let n = stream.read_audio(&mut buf);
    assert_eq!(n, 441);
    assert_eq!(&buf[..], &all[..441 * 2]);
}

/// Host cost of the real decoder at the movies' size: sequential frames,
/// and asking for the frame just decoded again (which used to restart the
/// reader from frame 0).
#[test]
#[cfg_attr(not(has_ffmpeg), ignore = "requires user's ffmpeg on PATH")]
fn real_decoder_cost_per_frame_at_640x480() {
    let temp = Temp::new();
    let bytes = support::media_for(
        &temp.0.join("big.nut"),
        "testsrc2=size=640x480:rate=30000/1001",
        "3",
    );
    let options = InstallOptions::new(temp.0.join("data/movies"));
    install(&mut disc(&[("big.bik", &bytes)]), &options).unwrap();
    let source = TranscodedSource::new(&options.data_dir, Arc::new(AvFoundationDecoder));
    let mut stream = source.open("big").unwrap();
    let info = stream.info();
    assert_eq!((info.width, info.height), (640, 480));
    let mut frame = vec![0; 640 * 480 * 4];
    let n = info.frames;
    let t = std::time::Instant::now();
    for i in 0..n {
        assert!(stream.decode(i, &mut frame));
    }
    let seq = t.elapsed() / n;
    let t = std::time::Instant::now();
    for _ in 0..10 {
        assert!(stream.decode(n - 1, &mut frame));
    }
    let again = t.elapsed() / 10;
    eprintln!(
        "AVFoundation 640x480: {seq:?}/frame sequential, {again:?} to repeat the last of {n}"
    );
    assert!(seq < std::time::Duration::from_millis(15), "{seq:?}");
    assert!(
        again < std::time::Duration::from_millis(15),
        "repeat {again:?}"
    );
}
