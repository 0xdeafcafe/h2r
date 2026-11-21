use super::*;
use std::path::Path;
use std::sync::mpsc;

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        let path =
            std::env::temp_dir().join(format!("theater-media-{}", live_proto::sign::random_id()));
        std::fs::create_dir(&path).unwrap();
        let session = Session::open(
            Some(path.join("synthetic.h2film")),
            None,
            host_facts(),
            b"synthetic",
            &options(&path),
        )
        .unwrap()
        .unwrap();
        let guest = arena();
        for _ in 0..60 {
            session.checkpoint(&guest);
        }
        session.finish(90, true).unwrap();
        Self(path)
    }

    fn player(&self, render_every: u32) -> Arc<Session> {
        Session::open(
            None,
            Some(Replay {
                path: self.0.join("synthetic.h2film"),
                render_every,
                speed: 1.0,
                seek_tick: 0,
                paused_at_target: false,
                presentation: Presentation::default(),
                restart: self.0.join("restart.json"),
                camera: Default::default(),
                window_closed: None,
            }),
            host_facts(),
            b"synthetic",
            &options(&self.0),
        )
        .unwrap()
        .unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct Encoded {
    frames: Vec<u8>,
    audio: Vec<f32>,
    finished: bool,
}

struct Sink(Arc<Mutex<Encoded>>);

impl ClipSink for Sink {
    fn frame(&mut self, picture: &Picture) -> Result<()> {
        self.0.lock().unwrap().frames.push(picture.rgba[0]);
        Ok(())
    }

    fn audio(&mut self, pcm: &[f32]) -> Result<()> {
        self.0.lock().unwrap().audio.extend_from_slice(pcm);
        Ok(())
    }

    fn finish(&mut self) -> Result<()> {
        self.0.lock().unwrap().finished = true;
        Ok(())
    }
}

fn picture(value: u8) -> Picture {
    Picture {
        size: (1, 1),
        rgba: vec![value, 0, 0, 255],
    }
}

#[test]
fn headless_replay_emits_the_exact_half_open_clip_at_both_frame_rates() {
    let fixture = Fixture::new();
    let guest = arena();
    for (from, to) in [(27, 34), (58, 61)] {
        for fps in [30, 60] {
            let encoded = Arc::new(Mutex::new(Encoded::default()));
            let player = fixture.player(0);
            let capture = Arc::new(ClipCapture::new(
                ClipSpan::new(from, to, fps, 60).unwrap(),
                picture(0),
                true,
                Box::new(Sink(encoded.clone())),
            ));
            player.attach_capture(capture.clone()).unwrap();
            for tick in 0..=60 {
                if player.advance_capture().unwrap() {
                    break;
                }
                if tick != from + 1 {
                    capture.present(|| Ok(picture(tick as u8))).unwrap();
                }
                capture.audio(&[0.25, -0.25]);
                if tick < 60 {
                    player.checkpoint(&guest);
                }
            }
            player.finish(if to > 60 { 90 } else { 1 }, true).unwrap();
            capture.finish().unwrap();
            assert!(player.lock().failure.is_none());
            let encoded = encoded.lock().unwrap();
            let expected: Vec<_> = (from..to)
                .flat_map(|tick| {
                    let observed = if tick == from + 1 { from } else { tick };
                    std::iter::repeat_n(observed as u8, fps as usize / 30)
                })
                .collect();
            assert_eq!(encoded.frames, expected);
            assert_eq!(
                encoded.frames.len() as u64,
                (to - from) * u64::from(fps) / 30
            );
            assert_eq!(encoded.audio.len() as u64, (to - from) * 2 * 1_600);
            assert_eq!(&encoded.audio[..2], &[0.25, -0.25]);
            assert_eq!(*encoded.audio.last().unwrap(), 0.0);
            assert!(encoded.finished);
        }
    }
}

#[test]
fn media_rpc_captures_png_and_passes_a_scaled_request_to_the_paused_renderer() {
    let fixture = Fixture::new();
    let player = fixture.player(1);
    let agent = agent::Agent::new();
    let rt = Arc::new(runtime::Runtime::new(
        runtime::Guest::new(),
        runtime::Dispatch::default(),
    ));
    let output = fixture.0.join("clip");
    player
        .attach_media(
            Media {
                screenshots: fixture.0.clone(),
                latest: Box::new(|| Ok(picture(10))),
                raster: Arc::new(|| (2, 3)),
                start_export: Box::new(|clip, _, length| {
                    assert_eq!(
                        (clip.from, clip.to, clip.fps, clip.format, length),
                        (3, 9, 60, api::ClipFormat::Png, 60)
                    );
                    Ok(api::ClipJob {
                        job: "test-job".into(),
                    })
                }),
                export_status: Box::new(move |job| {
                    assert_eq!(job.job, "test-job");
                    Ok(api::ClipStatus {
                        frames_done: 0,
                        frames_total: 12,
                        path: output.clone(),
                        error: None,
                        finished: false,
                    })
                }),
            },
            &rt,
        )
        .unwrap();
    let (requests, render) = mpsc::channel();
    player
        .attach_scaled_screenshot(move |size, tick| {
            let (send, receive) = mpsc::channel();
            requests.send((size, tick, send))?;
            Ok(receive)
        })
        .unwrap();
    player.add_methods(&agent);

    let (send, replies) = mpsc::sync_channel(10);
    let conn = agent.connect(send);
    let submit = |name: &str, id, params| {
        agent.submit(
            conn,
            &json!({"id": id, "jsonrpc": "2.0", "method": name, "params": params}).to_string(),
        );
    };
    let poll = || agent.poll_theater(&mut agent::GuestReader::new(|| rt.memory()));
    let response = || serde_json::from_str::<Value>(&replies.try_recv().unwrap()).unwrap();
    let rpc = |name, params| {
        submit(name, 1, params);
        poll();
        response()
    };

    assert_eq!(rpc(api::CLIP_MARK, json!({"in":3}))["result"]["in"], 3);
    assert_eq!(rpc(api::CLIP_MARK, json!({"out":9}))["result"]["out"], 9);
    assert_eq!(
        rpc(api::CLIP_MARK, json!({"out":2}))["error"]["code"],
        agent_proto::code::INVALID_PARAMS
    );
    assert_eq!(
        rpc(api::CLIP_MARK, json!({"clear":true}))["result"]["out"],
        Value::Null
    );
    let clip = json!({"from":3,"to":9,"fps":60,"format":"png"});
    assert_eq!(rpc(api::CLIP_EXPORT, clip)["result"]["job"], "test-job");
    let job = rpc(api::CLIP_STATUS, json!({"job":"test-job"}));
    assert_eq!(job["result"]["frames_total"], 12);
    assert_eq!(job["result"]["finished"], false);

    let original = fixture.0.join("original.png");
    assert_eq!(
        rpc(api::SCREENSHOT, json!({"path":original}))["result"]["width"],
        1
    );
    assert_eq!(png_size(&original), (1, 1));
    assert!(rpc(api::SCREENSHOT, json!({"path":original}))["error"].is_object());
    assert_eq!(
        rpc(api::SCREENSHOT, json!({"scale":5}))["error"]["code"],
        agent_proto::code::INVALID_PARAMS
    );
    assert_eq!(
        rpc(api::SCREENSHOT, json!({"scale":2}))["error"]["code"],
        agent_proto::code::UNAVAILABLE
    );

    assert_eq!(rpc(api::PAUSE, json!({}))["result"]["paused"], true);
    let scaled = fixture.0.join("scaled.png");
    submit(api::SCREENSHOT, 2, json!({"scale":2,"path":scaled}));
    poll();
    assert!(matches!(replies.try_recv(), Err(mpsc::TryRecvError::Empty)));
    let (size, tick, result) = render.try_recv().unwrap();
    assert_eq!(size, (4, 6));
    assert_eq!(tick, 0);
    result
        .send(Ok(Picture {
            size,
            rgba: vec![255; 4 * 6 * 4],
        }))
        .unwrap();
    poll();
    let saved: api::ScreenshotSaved = serde_json::from_value(response()["result"].clone()).unwrap();
    assert_eq!((saved.width, saved.height, saved.tick), (4, 6, 0));
    assert_eq!(png_size(&scaled), (4, 6));
    assert!(player.lock().paused);
}

fn png_size(path: &Path) -> (u32, u32) {
    let file = std::io::BufReader::new(File::open(path).unwrap());
    let reader = png::Decoder::new(file).read_info().unwrap();
    (reader.info().width, reader.info().height)
}
