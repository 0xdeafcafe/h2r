//! `render.picture` against a synthetic frame source.

use std::sync::atomic::AtomicBool;

use super::*;
use crate::live::Frames;
use agent_proto::picture::{OpenParams, read_stream_frame};

/// A frame source that presents a new frame at every grab: frame `seq`
/// is `size` pixels of `fill(seq)`.
struct Synthetic {
    size: (u32, u32),
    fill: fn(u64) -> [u8; 4],
}

impl FrameFeed for Synthetic {
    fn ready(&self) -> Result<(), String> {
        Ok(())
    }

    fn grab(&self, after: Option<u64>) -> Result<Option<Grabbed>, String> {
        let seq = after.map_or(1, |s| s + 1);
        let px = (self.fill)(seq);
        let rgba = px.repeat(self.size.0 as usize * self.size.1 as usize);
        Ok(Some(Grabbed {
            seq,
            size: self.size,
            rgba,
        }))
    }
}

/// Never ready: a game with no window.
struct Nothing;

impl FrameFeed for Nothing {
    fn ready(&self) -> Result<(), String> {
        Err(wire::NO_SOURCE.into())
    }
    fn grab(&self, _: Option<u64>) -> Result<Option<Grabbed>, String> {
        Err(wire::NO_SOURCE.into())
    }
}

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("h2-picture-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn pictures(name: &str, size: (u32, u32), fill: fn(u64) -> [u8; 4]) -> (Arc<Pictures>, PathBuf) {
    let dir = scratch(name);
    (
        Arc::new(Pictures::new(
            Arc::new(Synthetic { size, fill }),
            dir.clone(),
            false,
        )),
        dir,
    )
}

fn ring_params(width: u32) -> OpenParams {
    OpenParams {
        width: Some(width),
        fps: Some(60),
        transport: None,
    }
}

fn by_seq(seq: u64) -> [u8; 4] {
    let v = (seq % 251) as u8;
    [v, v, v, v]
}

#[test]
fn a_reader_never_sees_a_torn_slot() {
    let (p, dir) = pictures("torn", (256, 256), by_seq);
    let opened = p.open_ring(1, &ring_params(256), None).unwrap();
    p.start(opened.id);
    let path = opened.path.clone().unwrap();
    let stop = Arc::new(AtomicBool::new(false));
    let reader = {
        let stop = stop.clone();
        std::thread::spawn(move || {
            let ring = Ring::open(&path).unwrap();
            let mut out = Vec::new();
            let (mut whole, mut last) = (0u64, 0u64);
            while !stop.load(Ordering::Relaxed) {
                let Some(meta) = ring.read_newest(&mut out) else {
                    continue;
                };
                assert!(meta.frame >= last, "frames go forward");
                last = meta.frame;
                assert_eq!((meta.width, meta.height), (256, 256));
                let want = (meta.frame % 251) as u8;
                assert!(
                    out.iter().all(|&b| b == want),
                    "frame {} is torn",
                    meta.frame
                );
                whole += 1;
            }
            whole
        })
    };
    // Frames as fast as the writer goes: each pump is a frame interval on.
    let mut now = Instant::now();
    let mut written = 0;
    while written < 3000 {
        now += Duration::from_millis(17);
        if let Pump::Published(n) = p.pump(now) {
            written += n;
        }
        if reader.is_finished() {
            break;
        }
    }
    stop.store(true, Ordering::Relaxed);
    let whole = reader.join().expect("no torn frame");
    assert!(whole > 0, "the reader took frames");
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unread_ring_stops_readbacks() {
    let (p, dir) = pictures("unread", (64, 32), by_seq);
    assert_eq!(
        p.pump(Instant::now()),
        Pump::Closed,
        "nothing open, nothing read"
    );
    assert_eq!(p.readbacks(), 0);
    let opened = p.open_ring(1, &ring_params(64), None).unwrap();
    assert!(
        matches!(p.pump(Instant::now()), Pump::Wait(_)),
        "not started yet"
    );
    assert_eq!(p.readbacks(), 0, "no frames before the reply");
    p.start(opened.id);
    let t0 = Instant::now();
    assert_eq!(p.pump(t0), Pump::Published(1));
    assert_eq!(p.pump(t0 + Duration::from_millis(100)), Pump::Published(1));
    assert_eq!(p.readbacks(), 2);
    // Not due again until a frame interval has passed.
    assert!(matches!(
        p.pump(t0 + Duration::from_millis(101)),
        Pump::Wait(_)
    ));
    // A second with the newest frame unread: no more readbacks.
    for ms in [1100, 1300, 2000] {
        assert!(
            matches!(p.pump(t0 + Duration::from_millis(ms)), Pump::Wait(_)),
            "{ms}"
        );
    }
    assert_eq!(p.readbacks(), 2);
    // The client reads: frames come again.
    let ring = Ring::open(opened.path.as_ref().unwrap()).unwrap();
    let mut out = Vec::new();
    assert_eq!(ring.read_newest(&mut out).unwrap().frame, 2);
    assert_eq!(p.pump(t0 + Duration::from_millis(2100)), Pump::Published(1));
    assert_eq!(p.readbacks(), 3);
    assert_eq!(ring.newest(), 3);
    drop((ring, p));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn close_and_drop_free_the_file() {
    let (p, dir) = pictures("free", (32, 32), by_seq);
    let a = p.open_ring(1, &ring_params(32), None).unwrap();
    let a_path = a.path.clone().unwrap();
    assert!(a_path.exists());
    assert!(a_path.starts_with(&dir));
    p.start(a.id);
    assert_eq!(p.pump(Instant::now()), Pump::Published(1));
    let reader = Ring::open(&a_path).unwrap();
    assert!(p.close(a.id));
    assert!(!a_path.exists(), "close removes the file");
    assert!(reader.closed(), "a reader still mapping it is told");
    assert!(!p.close(a.id), "closing twice");

    // The connection drops.
    let b = p.open_ring(7, &ring_params(32), None).unwrap();
    let c = p.open_ring(8, &ring_params(32), None).unwrap();
    p.close_conn(7);
    assert!(!b.path.as_ref().unwrap().exists());
    assert!(
        c.path.as_ref().unwrap().exists(),
        "another connection's picture stays"
    );
    p.retain_conns(|conn| conn != 8);
    assert!(!c.path.as_ref().unwrap().exists());
    assert_eq!(p.count(), 0);

    // The agent goes.
    let d = p.open_ring(9, &ring_params(32), None).unwrap();
    drop(p);
    assert!(
        !d.path.as_ref().unwrap().exists(),
        "dropping the pictures removes the file"
    );
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn the_streams_frames_decode() {
    fn colour(_: u64) -> [u8; 4] {
        [200, 40, 90, 255]
    }
    let (p, dir) = pictures("stream", (320, 200), colour);
    let (tx, frames) = Frames::channel(STREAM_QUEUE);
    let params = OpenParams {
        width: Some(160),
        fps: Some(15),
        transport: Some(Transport::Stream),
    };
    let opened = p.open_stream(1, &params, tx).unwrap();
    assert_eq!(
        (
            opened.transport,
            opened.path.as_ref(),
            opened.format.as_str()
        ),
        (Transport::Stream, None, "jpeg")
    );
    p.start(opened.id);
    let t0 = Instant::now();
    assert_eq!(p.pump(t0), Pump::Published(1));
    let bytes = frames.next_bytes(Duration::from_secs(1)).expect("a frame");
    let frame = read_stream_frame(&mut bytes.as_slice()).unwrap();
    assert_eq!((frame.event.id, frame.event.frame), (opened.id, 1));
    assert_eq!((frame.event.width, frame.event.height), (160, 100));
    let img = image::load_from_memory_with_format(&frame.jpeg, image::ImageFormat::Jpeg)
        .expect("the frame is a JPEG")
        .to_rgb8();
    assert_eq!(img.dimensions(), (160, 100));
    let px = img.get_pixel(80, 50).0;
    for (got, want) in px.iter().zip([200u8, 40, 90]) {
        assert!(got.abs_diff(want) <= 8, "{px:?}");
    }

    // A client that stops reading fills the queue: then no readbacks.
    let step = Duration::from_millis(67);
    for i in 1..=STREAM_QUEUE as u32 {
        assert_eq!(p.pump(t0 + step * i), Pump::Published(1));
    }
    let read = p.readbacks();
    assert!(matches!(p.pump(t0 + step * 10), Pump::Wait(_)));
    assert_eq!(p.readbacks(), read);
    // Reading one makes room.
    frames.next_bytes(Duration::from_secs(1)).unwrap();
    assert_eq!(p.pump(t0 + step * 11), Pump::Published(1));

    // The writer goes (the client closed): the next frame finds it gone
    // and the picture closes itself.
    while frames.next_bytes(Duration::from_millis(10)).is_some() {}
    drop(frames);
    assert_eq!(p.pump(t0 + step * 20), Pump::Published(1));
    assert_eq!(p.count(), 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn no_source_refuses_the_open() {
    let p = Arc::new(Pictures::new(Arc::new(Nothing), scratch("none"), false));
    let e = p.open_ring(1, &ring_params(64), None).unwrap_err();
    assert_eq!(e.code, code::UNAVAILABLE);
    let bad = OpenParams {
        width: Some(4),
        ..Default::default()
    };
    let (p, _) = pictures("bad", (8, 8), by_seq);
    assert_eq!(
        p.open_ring(1, &bad, None).unwrap_err().code,
        code::INVALID_PARAMS
    );
}

#[test]
fn frames_fit_the_width_and_average_down() {
    assert_eq!(fit((1280, 720), 640), (640, 360));
    assert_eq!(fit((720, 1280), 640), (360, 640));
    assert_eq!(fit((320, 200), 640), (320, 200), "never scaled up");
    // A black row over a white row: one grey pixel.
    let src: Vec<u8> = [
        [0u8, 0, 0, 255],
        [0, 0, 0, 255],
        [255, 255, 255, 255],
        [255, 255, 255, 255],
    ]
    .concat();
    assert_eq!(scale_rgba(&src, (2, 2), (1, 1)), vec![128, 128, 128, 255]);
}
