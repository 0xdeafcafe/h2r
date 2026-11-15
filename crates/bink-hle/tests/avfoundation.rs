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
