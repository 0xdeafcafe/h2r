//! `h2 agent picture`: a running game's picture, live, through
//! `render.picture` (docs/runtime/agent-api.md, "Picture feed").
//!
//! A local game's frames come through the shared-memory ring, a remote
//! one's (`--stream`, or any `tcp://` socket) as JPEGs on the connection.
//! With `--frames N` it reads N frames with no window and prints what came.

use std::io::Read;
use std::time::Instant;

use agent_proto::picture::{self as wire, OpenParams, Opened, PictureEvent, Ring, Transport};
use agent_proto::{Client, Event};
use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

#[derive(Debug, Default, PartialEq)]
struct Opts {
    stream: bool,
    width: Option<u32>,
    fps: Option<u32>,
    frames: Option<u64>,
}

fn parse(args: &[String]) -> Result<Opts> {
    let mut o = Opts::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut number = |what: &str| -> Result<u64> {
            it.next()
                .with_context(|| format!("{what} needs a number"))?
                .parse()
                .with_context(|| format!("{what} needs a number"))
        };
        match a.as_str() {
            "--stream" => o.stream = true,
            "--width" => o.width = Some(u32::try_from(number("--width")?)?),
            "--fps" => o.fps = Some(u32::try_from(number("--fps")?)?),
            "--frames" => o.frames = Some(number("--frames")?.max(1)),
            other => bail!("picture: unknown argument {other:?} (see h2 agent --help)"),
        }
    }
    Ok(o)
}

/// One frame, as RGBA8.
struct Frame {
    event: PictureEvent,
    rgba: Vec<u8>,
}

enum Feed {
    Ring {
        client: Client,
        ring: Ring,
        buf: Vec<u8>,
    },
    Stream(Box<dyn Read + Send>),
}

impl Feed {
    /// The next frame: on a ring, the newest one after the next `picture`
    /// event; on a stream, the next one sent.
    fn next(&mut self) -> Result<Frame> {
        match self {
            Feed::Ring { client, ring, buf } => loop {
                if ring.closed() {
                    bail!("the game closed the picture");
                }
                let Event::Picture(event) = client.next_event()? else {
                    continue;
                };
                let Some(meta) = ring.read_newest(buf) else {
                    continue;
                };
                return Ok(Frame {
                    event: PictureEvent {
                        frame: meta.frame,
                        tick: meta.tick,
                        present: meta.present,
                        time_ms: meta.time_us as f64 / 1000.0,
                        width: meta.width,
                        height: meta.height,
                        ..event
                    },
                    rgba: buf.clone(),
                });
            },
            Feed::Stream(r) => {
                let f = wire::read_stream_frame(r).context("reading the picture stream")?;
                let rgba = image::load_from_memory_with_format(&f.jpeg, image::ImageFormat::Jpeg)
                    .context("decoding a picture frame")?
                    .to_rgba8();
                let (width, height) = rgba.dimensions();
                Ok(Frame {
                    event: PictureEvent {
                        width,
                        height,
                        ..f.event
                    },
                    rgba: rgba.into_raw(),
                })
            }
        }
    }
}

/// Opens the picture on `c` and shows or counts it.
pub(crate) fn run(mut c: Client, args: &[String], remote: bool) -> Result<()> {
    let o = parse(args)?;
    let params = OpenParams {
        width: o.width,
        fps: o.fps,
        transport: Some(if o.stream || remote {
            Transport::Stream
        } else {
            Transport::Ring
        }),
    };
    let reply = c
        .call(wire::OPEN, serde_json::to_value(&params)?)
        .map_err(|e| anyhow::anyhow!("{}: {e}", wire::OPEN))?;
    let opened: Opened = serde_json::from_value(reply).context("render.picture.open's reply")?;
    eprintln!(
        "picture {}: {:?}, up to {}x{} at {} fps{}",
        opened.id,
        opened.transport,
        opened.size[0],
        opened.size[1],
        opened.fps,
        opened
            .path
            .as_ref()
            .map(|p| format!(", {}", p.display()))
            .unwrap_or_default()
    );
    let feed = match opened.transport {
        Transport::Ring => {
            let path = opened.path.clone().context("a ring reply without a path")?;
            let ring = Ring::open(&path).with_context(|| format!("mapping {}", path.display()))?;
            Feed::Ring {
                client: c,
                ring,
                buf: Vec::new(),
            }
        }
        Transport::Stream => Feed::Stream(Box::new(c.into_live_reader())),
    };
    match o.frames {
        Some(n) => {
            let v = count(feed, &opened, n)?;
            println!("{}", serde_json::to_string_pretty(&v)?);
            Ok(())
        }
        None => show(feed, &opened),
    }
}

/// Reads `n` frames and says what came.
fn count(mut feed: Feed, opened: &Opened, n: u64) -> Result<Value> {
    let start = Instant::now();
    let first = feed.next()?;
    let mut last = first.event.clone();
    let mut got = 1;
    while got < n {
        last = feed.next()?.event;
        got += 1;
    }
    let seconds = start.elapsed().as_secs_f64();
    if let Feed::Ring { client, .. } = &mut feed {
        let _ = client.call(wire::CLOSE, json!({"id": opened.id}));
    }
    let stamp = |e: &PictureEvent| json!({"frame": e.frame, "tick": e.tick, "present": e.present, "size": [e.width, e.height]});
    Ok(json!({
        "id": opened.id,
        "transport": opened.transport,
        "frames": got,
        // Frames the game published that this reader never took.
        "skipped": (last.frame - first.event.frame + 1).saturating_sub(got),
        "seconds": seconds,
        "fps": if seconds > 0.0 { (got - 1) as f64 / seconds } else { 0.0 },
        "first": stamp(&first.event),
        "last": stamp(&last),
    }))
}

/// A window showing the frames as they come.
fn show(mut feed: Feed, opened: &Opened) -> Result<()> {
    let config = app::AppConfig {
        title: format!("h2 picture {} ({:?})", opened.id, opened.transport),
        audio: false,
        keyboard: false,
        gamepads: false,
        mouse_capture: false,
        aspect: app::DisplayAspect::Source,
        keep_open_on_guest_exit: false,
        window_size: (960, 540),
        ..Default::default()
    };
    let report = app::run(config, move |window| {
        let mut sink = window.frame_sink();
        while !window.should_stop() {
            let f = feed.next()?;
            sink.submit_rgba((f.event.width, f.event.height), &f.rgba)?;
        }
        Ok(())
    })?;
    match report.guest {
        app::GuestExit::Failed(e) | app::GuestExit::Panicked(e) => bail!("picture: {e}"),
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_owned).collect()
    }

    #[test]
    fn options() {
        assert_eq!(parse(&[]).unwrap(), Opts::default());
        assert_eq!(
            parse(&words("--stream --width 320 --fps 10 --frames 30")).unwrap(),
            Opts {
                stream: true,
                width: Some(320),
                fps: Some(10),
                frames: Some(30)
            }
        );
        for bad in ["--width", "--width wide", "--fps -1", "--loud"] {
            assert!(parse(&words(bad)).is_err(), "{bad}");
        }
    }
}
