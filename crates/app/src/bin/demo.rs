//! Drives the shell with a synthetic guest, so a person can check the window,
//! scaling, live input and audio without the game.
//!
//! ```text
//! cargo run -p app --release --bin h2-app-demo
//! cargo run -p app --release --bin h2-app-demo -- --aspect 16:9 --scaling integer --nearest
//! cargo run -p app --release --bin h2-app-demo -- --frames 300   # closes itself after 5 s
//! ```
//!
//! What to look for (see docs/app.md, "Demo"):
//! * the frame is 640x480 shown at 4:3 with black bars, whatever the window
//!   shape; the circle stays round; corner markers are red (top left), green
//!   (top right), blue (bottom left), yellow (bottom right) and the 1 px white
//!   border is visible on all four sides (no cropping);
//! * the vertical bar sweeps smoothly at one width per two seconds and the
//!   title shows guest and shown frame rates near 60;
//! * pressing buttons lights the boxes along the bottom (A B X Y Black White
//!   LT RT Up Down Left Right Start Back LS RS), sticks move the dots, and
//!   every change is printed to stdout;
//! * a quiet tone plays: the left stick pans it, the right stick's Y bends
//!   its pitch, A toggles it, and it pauses with the guest (Cmd+P).
//!
//! Keyboard: WASD left stick, mouse look (click to capture, Cmd releases),
//! Space A, V B, E X, Q Y, F Black, R White, G LT, left click RT, arrows
//! d-pad, Escape Start, Tab Back. Cmd+F fullscreen, Cmd+Q quit.

use std::f32::consts::TAU;

use app::{AppConfig, Control, DisplayAspect, Filter, GuestHost, Scaling};
use audio::{SoundBuffer, WaveFormat};
use input::PortIndex;
use input::gamepad::{AnalogButton, Buttons, Gamepad};

const W: u32 = 640;
const H: u32 = 480;

struct Args {
    frames: Option<u64>,
    fps: u32,
    config: AppConfig,
}

fn usage() -> ! {
    eprintln!(
        "usage: h2-app-demo [--frames N] [--fps N] [--aspect 4:3|16:9|source] [--scaling fit|integer|stretch]\n\
         \x20                  [--input-config PATH] [--input-script PATH] [--nearest] [--no-vsync] [--no-audio] [--no-gamepads] [--pause-on-blur] [--fullscreen]"
    );
    std::process::exit(2)
}

fn parse() -> Args {
    let mut a = Args {
        frames: None,
        fps: 60,
        config: AppConfig {
            title: "h2recomp shell demo".into(),
            log_status: true,
            ..AppConfig::default()
        },
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut value = || it.next().unwrap_or_else(|| usage());
        match arg.as_str() {
            "--frames" => a.frames = Some(value().parse().unwrap_or_else(|_| usage())),
            "--fps" => {
                a.fps = value()
                    .parse()
                    .ok()
                    .filter(|&f| f > 0)
                    .unwrap_or_else(|| usage())
            }
            "--aspect" => {
                a.config.aspect = match value().as_str() {
                    "4:3" => DisplayAspect::Standard,
                    "16:9" => DisplayAspect::Widescreen,
                    "source" => DisplayAspect::Source,
                    _ => usage(),
                }
            }
            "--scaling" => {
                a.config.scaling = match value().as_str() {
                    "fit" => Scaling::Fit,
                    "integer" => Scaling::Integer,
                    "stretch" => Scaling::Stretch,
                    _ => usage(),
                }
            }
            "--input-config" => {
                let path = value();
                a.config.bindings = std::fs::read_to_string(&path)
                    .map_err(|e| e.to_string())
                    .and_then(|s| {
                        input::bindings::Bindings::from_toml(&s).map_err(|e| e.to_string())
                    })
                    .unwrap_or_else(|e| {
                        eprintln!("{path}: {e}");
                        usage()
                    });
            }
            "--input-script" => {
                let path = value();
                a.config.scripted_input = Some(
                    std::fs::read_to_string(&path)
                        .map_err(|e| e.to_string())
                        .and_then(|s| {
                            input::backend::scripted::Timeline::from_toml(&s)
                                .map_err(|e| e.to_string())
                        })
                        .unwrap_or_else(|e| {
                            eprintln!("{path}: {e}");
                            usage()
                        })
                        .playback(),
                );
            }
            "--nearest" => a.config.filter = Filter::Nearest,
            "--no-vsync" => a.config.vsync = false,
            "--no-audio" => a.config.audio = false,
            "--no-gamepads" => a.config.gamepads = false,
            "--pause-on-blur" => a.config.pause_on_focus_loss = true,
            "--fullscreen" => a.config.fullscreen = true,
            _ => usage(),
        }
    }
    if a.frames.is_some() {
        a.config.keep_open_on_guest_exit = false;
    }
    a
}

fn main() -> anyhow::Result<()> {
    let Args {
        frames,
        fps,
        config,
    } = parse();
    let script = config.scripted_input.clone();
    let report = app::run(config, move |host| guest(host, frames, fps, script))?;
    println!(
        "exit: {:?}; frames published {}, shown {}, dropped {}; pacing {} sleeps, {} rebases",
        report.guest,
        report.frames.published,
        report.frames.shown,
        report.frames.dropped,
        report.pacing.sleeps,
        report.pacing.rebases
    );
    Ok(())
}

/// The synthetic guest: 60 Hz of logical time, a test pattern, input echo
/// and a tone. Shaped like the real guest loop: tick, input, frame.
fn guest(
    mut host: GuestHost,
    frames: Option<u64>,
    fps: u32,
    script: Option<input::backend::scripted::ScriptedInput>,
) -> anyhow::Result<()> {
    let mut input = host.take_input().expect("first take");
    let mut sink = host.frame_sink();
    let mixer = host.mixer();
    let tone = {
        // 100 samples per cycle, 64 cycles: pitch = playback rate / 100.
        let rate = 22_000;
        let pcm: Vec<u8> = (0..6400)
            .flat_map(|i| (((i as f32 / 100.0 * TAU).sin() * 6000.0) as i16).to_le_bytes())
            .collect();
        let buf = SoundBuffer::from_data(WaveFormat::pcm16(1, rate)?, pcm)?;
        let mut m = mixer.lock().unwrap();
        let id = m.add_buffer(buf);
        let b = m.buffer_mut(id).expect("just added");
        b.set_volume(-1200);
        b.play(true);
        id
    };
    println!(
        "demo: audio mixer at {} Hz; tone playing at 220 Hz",
        host.sample_rate()
    );

    let ticks_per_frame = 10_000_000 / fps as u64;
    let mut guest_ticks = 0u64;
    let mut handle = None;
    let mut last = Gamepad::default();
    let mut tone_on = true;
    let mut connected = 0u32;
    let mut pixels = vec![0u8; (W * H * 4) as usize];
    let mut n = 0u64;
    loop {
        if host.tick(guest_ticks) == Control::Stop {
            println!("demo: stop requested after {n} frames");
            break;
        }
        if let Some(script) = &script {
            script.advance_to(guest_ticks)?;
        }
        input.sync();
        if input.connected_mask() != connected {
            connected = input.connected_mask();
            println!("demo: controller ports connected: {connected:04b}");
        }
        if handle.is_none() {
            handle = PortIndex::new(0).and_then(|p| input.open(p).ok());
        }
        let pad = match handle.map(|h| input.get_state(h)) {
            Some(Ok(s)) => s.gamepad,
            Some(Err(_)) => {
                handle = None;
                Gamepad::default()
            }
            None => Gamepad::default(),
        };
        if pad != last {
            println!("demo: pad {}", describe(&pad));
            let a = AnalogButton::A.index();
            if pad.analog[a] > 0 && last.analog[a] == 0 {
                tone_on = !tone_on;
            }
            last = pad;
        }
        {
            let mut m = mixer.lock().unwrap();
            if let Some(b) = m.buffer_mut(tone) {
                let bend = pad.thumb_ry as f32 / 32767.0; // -1..1: one octave either way
                b.set_frequency((22_000.0 * 2f32.powf(bend)) as u32);
                b.set_pan((pad.thumb_lx as i32 * 2000) / 32767);
                b.set_volume(if tone_on { -1200 } else { -10_000 });
            }
        }
        draw(&mut pixels, n, fps, &pad);
        sink.submit_rgba((W, H), &pixels)?;
        n += 1;
        guest_ticks += ticks_per_frame;
        if frames.is_some_and(|f| n >= f) {
            println!("demo: {n} frames done");
            break;
        }
    }
    Ok(())
}

fn describe(p: &Gamepad) -> String {
    let names = [
        (Buttons::DPAD_UP, "Up"),
        (Buttons::DPAD_DOWN, "Down"),
        (Buttons::DPAD_LEFT, "Left"),
        (Buttons::DPAD_RIGHT, "Right"),
        (Buttons::START, "Start"),
        (Buttons::BACK, "Back"),
        (Buttons::LEFT_THUMB, "LS"),
        (Buttons::RIGHT_THUMB, "RS"),
    ];
    let mut s: Vec<String> = names
        .iter()
        .filter(|(b, _)| p.buttons.contains(*b))
        .map(|(_, n)| n.to_string())
        .collect();
    for (i, n) in ["A", "B", "X", "Y", "Black", "White", "LT", "RT"]
        .iter()
        .enumerate()
    {
        if p.analog[i] > 0 {
            s.push(format!("{n}={}", p.analog[i]));
        }
    }
    format!(
        "[{}] L({:+6},{:+6}) R({:+6},{:+6})",
        s.join(" "),
        p.thumb_lx,
        p.thumb_ly,
        p.thumb_rx,
        p.thumb_ry
    )
}

// --- the test pattern -----------------------------------------------------

fn put(px: &mut [u8], x: i32, y: i32, c: [u8; 3]) {
    if (0..W as i32).contains(&x) && (0..H as i32).contains(&y) {
        let i = ((y as u32 * W + x as u32) * 4) as usize;
        px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
    }
}

fn rect(px: &mut [u8], x: i32, y: i32, w: i32, h: i32, c: [u8; 3]) {
    for yy in y..y + h {
        for xx in x..x + w {
            put(px, xx, yy, c);
        }
    }
}

fn frame_rect(px: &mut [u8], x: i32, y: i32, w: i32, h: i32, c: [u8; 3]) {
    rect(px, x, y, w, 1, c);
    rect(px, x, y + h - 1, w, 1, c);
    rect(px, x, y, 1, h, c);
    rect(px, x + w - 1, y, 1, h, c);
}

fn circle(px: &mut [u8], cx: i32, cy: i32, r: i32, c: [u8; 3]) {
    let steps = (r * 8).max(16);
    for i in 0..steps {
        let a = i as f32 / steps as f32 * TAU;
        put(
            px,
            cx + (a.cos() * r as f32).round() as i32,
            cy + (a.sin() * r as f32).round() as i32,
            c,
        );
    }
}

fn disc(px: &mut [u8], cx: i32, cy: i32, r: i32, c: [u8; 3]) {
    for y in -r..=r {
        for x in -r..=r {
            if x * x + y * y <= r * r {
                put(px, cx + x, cy + y, c);
            }
        }
    }
}

fn draw(px: &mut [u8], n: u64, fps: u32, pad: &Gamepad) {
    // Background: slowly drifting diagonal bands.
    let shift = (n * 2) as u32;
    for y in 0..H {
        for x in 0..W {
            let band = ((x + y + shift) / 40) % 6;
            let c = [
                [40, 24, 24],
                [40, 40, 24],
                [24, 40, 24],
                [24, 40, 40],
                [24, 24, 40],
                [40, 24, 40],
            ][band as usize];
            let i = ((y * W + x) * 4) as usize;
            px[i..i + 4].copy_from_slice(&[c[0], c[1], c[2], 255]);
        }
    }
    // Grid every 80 px: 8 x 6 squares, so squares look square only at 4:3.
    for g in (0..W as i32).step_by(80) {
        rect(px, g, 0, 1, H as i32, [70, 70, 70]);
    }
    for g in (0..H as i32).step_by(80) {
        rect(px, 0, g, W as i32, 1, [70, 70, 70]);
    }
    // Round only if the aspect is right.
    circle(px, W as i32 / 2, H as i32 / 2, 200, [220, 220, 220]);
    circle(px, W as i32 / 2, H as i32 / 2, 199, [220, 220, 220]);
    // Sweep bar: one frame width every two seconds of guest time.
    let period = (fps * 2) as u64;
    let bx = ((n % period) * W as u64 / period) as i32;
    rect(px, bx, 60, 8, H as i32 - 120, [255, 255, 255]);
    // Frame counter as a row of bits (LSB right), top centre.
    for b in 0..16 {
        let on = (n >> b) & 1 == 1;
        rect(
            px,
            W as i32 / 2 + 60 - b * 8,
            12,
            6,
            6,
            if on { [255, 200, 0] } else { [60, 60, 60] },
        );
    }
    // Corner markers and the 1 px border.
    rect(px, 0, 0, 24, 24, [255, 0, 0]);
    rect(px, W as i32 - 24, 0, 24, 24, [0, 255, 0]);
    rect(px, 0, H as i32 - 24, 24, 24, [0, 0, 255]);
    rect(px, W as i32 - 24, H as i32 - 24, 24, 24, [255, 255, 0]);
    frame_rect(px, 0, 0, W as i32, H as i32, [255, 255, 255]);

    // Input echo. Sticks: two rings with a dot.
    for (cx, sx, sy) in [
        (200, pad.thumb_lx, pad.thumb_ly),
        (440, pad.thumb_rx, pad.thumb_ry),
    ] {
        let cy = 330;
        circle(px, cx, cy, 40, [160, 160, 160]);
        let dx = (sx as f32 / 32767.0 * 36.0) as i32;
        let dy = -(sy as f32 / 32767.0 * 36.0) as i32;
        disc(px, cx + dx, cy + dy, 6, [0, 220, 255]);
    }
    // Analog buttons (A B X Y Black White LT RT): boxes filled by pressure.
    let colors = [
        [0, 200, 0],
        [220, 0, 0],
        [0, 90, 255],
        [230, 200, 0],
        [30, 30, 30],
        [240, 240, 240],
        [200, 120, 0],
        [200, 120, 0],
    ];
    for (i, c) in colors.iter().enumerate() {
        let x = 36 + i as i32 * 36;
        frame_rect(px, x, 420, 30, 30, [200, 200, 200]);
        let h = pad.analog[i] as i32 * 28 / 255;
        rect(px, x + 1, 449 - h, 28, h, *c);
    }
    // Digital buttons (Up Down Left Right Start Back LS RS).
    for b in 0..8 {
        let x = 324 + b * 36;
        frame_rect(px, x, 420, 30, 30, [200, 200, 200]);
        if pad.buttons.0 & (1 << b) != 0 {
            rect(px, x + 1, 421, 28, 28, [255, 255, 255]);
        }
    }
}
