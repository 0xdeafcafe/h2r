//! `h2 play`: the boot driver (crates/boot) inside the host shell
//! (crates/app). The window stays on the main thread; the game runs on
//! app's guest thread under `boot::run_hosted`, presenting through the
//! shell's frame handoff, playing DirectSound's logical-time output on the
//! default audio device, and reading the window's keyboard and the host's
//! pads. See docs/app.md "Playing: h2 play".

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use input::backend::scripted::{ScriptedInput, Timeline};
use input::bindings::{Bindings, Control, Input};

const USAGE: &str = "usage:
  h2 play <disc.iso> [--out DIR] [--data DIR] [--transfers N] [--slices N]
          [--aspect 4:3|16:9] [--scaling fit|integer|stretch] [--nearest] [--fullscreen]
          [--no-audio] [--no-gamepads] [--no-vsync] [--unpaced] [--bindings FILE] [--status]
          [--input-timeline FILE]

Opens a window and runs the recompiled game live (builds the dylib first when
--out has none, or one built from another image or generator). --out and
--data default to $TMPDIR/h2boot and $TMPDIR/h2boot-data, as for `h2 run`. --slices is unlimited by default.
--status prints the frame rates once a second and the slice count every
1000 slices (the unit H2_BOOT_INPUT scripts in). --input-timeline plays an
input crate timeline (docs/input.md) on guest time instead of live input.

Keys: Escape START, Space or Enter A, V or Backspace B, E X, Q Y, Tab BACK,
arrows d-pad, WASD left stick, mouse look (click to capture). Cmd+Q quits,
Cmd+F fullscreen, Cmd+P pause. --bindings reads a TOML table (docs/input.md).";

/// The play bindings: input's Halo defaults, plus Enter for A and Backspace
/// for B so the menus work the way a desktop user expects.
pub fn play_bindings() -> Bindings {
    let mut b = Bindings::default();
    for (control, key) in [(Control::A, "enter"), (Control::B, "backspace")] {
        let list = b.bind.entry(control).or_default();
        let key = Input::Key(key.into());
        if !list.contains(&key) {
            list.push(key);
        }
    }
    b
}

/// Whether `lib` must be (re)built before playing: missing, or stamped
/// for another image, generator or module ABI (`boot::dylib_current`), which
/// `boot::run_hosted` would refuse inside the window.
fn needs_build(xbe: &[u8], lib: &std::path::Path) -> bool {
    if boot::dylib_current(xbe, lib) {
        return false;
    }
    if lib.exists() {
        eprintln!(
            "play: {} was built from another image or generator; rebuilding",
            lib.display()
        );
    }
    true
}

/// Kernel calls `h2 play` keeps in memory for the report (the run is
/// unbounded; the headless trace keeps them all).
const TRACE_LIMIT: usize = 4096;

/// Opens the default output device on the audio output thread, or a sink
/// nobody plays (counted as dropped frames) if there is none or `audio` is
/// off. Either way audio time stays the kernel's.
fn audio_sink(audio: bool) -> boot::dsound_glue::SinkOpener {
    Box::new(move || -> Box<dyn audio::AudioSink> {
        if audio {
            match audio::output::DeviceSink::open_default() {
                Ok(d) => {
                    eprintln!(
                        "play: audio on {} at {} Hz",
                        d.device_id(),
                        audio::AudioSink::sample_rate(&d)
                    );
                    return Box::new(d);
                }
                Err(e) => eprintln!("play: no audio device ({e}); audio is discarded"),
            }
        }
        Box::new(audio::NullSink::new(48_000, 48_000 / 4))
    })
}

/// The boot `Host` for one shell guest thread: D3D presents into the
/// window's handoff, DirectSound plays on the audio device, XAPI reads the
/// window's input, and every slice paces against the wall clock and stops
/// when the window closes.
fn hosted(
    mut host: app::GuestHost,
    audio: bool,
    status: bool,
    timeline: Option<ScriptedInput>,
    interrupted: Arc<AtomicBool>,
) -> boot::Host {
    let gfx = d3d::GfxBackend::new(host.gfx_context()).on_present(host.present_hook());
    let input = host.take_input();
    let keyboard = host.keyboard().cloned();
    boot::Host {
        gfx: Some(gfx),
        audio_sink: Some(audio_sink(audio)),
        trace_limit: Some(TRACE_LIMIT),
        input,
        keyboard,
        between_slices: Some(Box::new({
            let mut slices = 0u64;
            move |k: &kernel::Kernel| {
                slices += 1;
                let now = k.clock().counter();
                // A timeline plays on guest time; the next pump sees it.
                if let Some(t) = &timeline
                    && let Err(e) = t.advance_to(now)
                {
                    eprintln!("play: input timeline: {e}");
                }
                if status && slices.is_multiple_of(1000) {
                    // H2_BOOT_INPUT counts slices; this says where a run is.
                    eprintln!("play: slice {slices}, guest time {:.1}s", now as f64 / 1e7);
                }
                !interrupted.load(Ordering::Relaxed) && host.tick(now) == app::Control::Continue
            }
        })),
    }
}

/// Ctrl-C or SIGTERM in the terminal: the first ends the run like a window
/// close (the report prints, then the process exits); a second exits at
/// once. Must run before any thread exists, so every thread inherits the
/// blocked mask and only the watcher sees the signals. A signal the parent
/// set to ignore (a shell's `&` ignores SIGINT) stays ignored.
#[cfg(unix)]
fn watch_signals() -> Arc<AtomicBool> {
    let flag = Arc::new(AtomicBool::new(false));
    // SAFETY: plain libc signal-mask calls on a local, initialised set.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
        set
    };
    let f = Arc::clone(&flag);
    std::thread::Builder::new()
        .name("signals".into())
        .spawn(move || {
            loop {
                let mut sig = 0;
                // SAFETY: `set` is a valid set; sigwait only writes `sig`.
                if unsafe { libc::sigwait(&set, &mut sig) } != 0 {
                    return;
                }
                if f.swap(true, Ordering::Relaxed) {
                    std::process::exit(130);
                }
                eprintln!("play: interrupted; stopping the run (again to exit now)");
            }
        })
        .expect("spawning the signal watcher");
    flag
}

#[cfg(not(unix))]
fn watch_signals() -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

pub fn run(args: Vec<String>) -> Result<()> {
    let tmp = std::env::temp_dir();
    let mut out = tmp.join("h2boot");
    let mut data = tmp.join("h2boot-data");
    let mut slices = u64::MAX;
    let mut transfers = 50_000_000u64;
    let mut disc = None;
    let mut audio = true;
    let mut config = app::AppConfig {
        // boot's DirectSound output owns the device; the shell's own
        // AudioOut would open it a second time (its discard renderer only
        // consumes the unused `GuestHost::mixer`).
        audio: false,
        title: "Halo 2".into(),
        bindings: play_bindings(),
        // The run prints its report before the guest thread returns.
        shutdown_timeout: Duration::from_secs(10),
        ..app::AppConfig::default()
    };
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        let mut val = || it.next().with_context(|| format!("{a} needs a value"));
        match a.as_str() {
            "--out" => out = PathBuf::from(val()?),
            "--data" => data = PathBuf::from(val()?),
            "--slices" => slices = val()?.parse()?,
            "--transfers" => transfers = val()?.parse()?,
            "--aspect" => {
                config.aspect = match val()?.as_str() {
                    "4:3" => app::DisplayAspect::Standard,
                    "16:9" => app::DisplayAspect::Widescreen,
                    "source" => app::DisplayAspect::Source,
                    v => bail!("unknown aspect {v}\n\n{USAGE}"),
                }
            }
            "--scaling" => {
                config.scaling = match val()?.as_str() {
                    "fit" => app::Scaling::Fit,
                    "integer" => app::Scaling::Integer,
                    "stretch" => app::Scaling::Stretch,
                    v => bail!("unknown scaling {v}\n\n{USAGE}"),
                }
            }
            "--nearest" => config.filter = app::Filter::Nearest,
            "--fullscreen" => config.fullscreen = true,
            "--no-audio" => audio = false,
            "--no-gamepads" => config.gamepads = false,
            "--no-vsync" => config.vsync = false,
            "--unpaced" => config.pacing = app::PacePolicy::Unpaced,
            "--status" => config.log_status = true,
            "--input-timeline" => {
                let path = PathBuf::from(val()?);
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                let t = Timeline::from_toml(&text)
                    .map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
                config.scripted_input = Some(t.playback());
            }
            "--bindings" => {
                let path = PathBuf::from(val()?);
                let text = std::fs::read_to_string(&path)
                    .with_context(|| format!("reading {}", path.display()))?;
                config.bindings = Bindings::from_toml(&text)
                    .with_context(|| format!("parsing {}", path.display()))?;
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ if disc.is_none() => disc = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    let disc = disc
        .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        .context(USAGE)?;
    let xbe = boot::read_xbe(&disc)?;
    let lib = out.join(boot::DYLIB);
    if needs_build(&xbe, &lib) {
        let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
        boot::build(&xbe, &out, jobs)?;
    }
    let opts = boot::RunOpts {
        disc,
        data_root: data,
        max_slices: slices,
        transfers_per_slice: transfers,
        stopgaps: true,
        trace: None,
        d3d: true,
        bink: true,
        dsound: true,
        xapi: true,
    };
    let status = config.log_status;
    let timeline = config.scripted_input.clone();
    let interrupted = watch_signals();
    let report = app::run(config, move |host| {
        let host = hosted(host, audio, status, timeline, Arc::clone(&interrupted));
        let r = boot::run_hosted(&xbe, &lib, &opts, host)?;
        for l in &r.kernel_log {
            println!("DbgPrint: {}", l.trim_end());
        }
        println!(
            "outcome: {}",
            r.outcome.chars().take(400).collect::<String>()
        );
        println!("stop: {}", r.stop);
        println!("last eip: {:#010x}", r.eip);
        if interrupted.load(Ordering::Relaxed) {
            // The window has nothing left to show; end like Cmd+Q would.
            std::process::exit(0);
        }
        Ok(())
    })?;
    println!(
        "play: guest {:?}; frames published {}, shown {}, dropped {}; pacing {} sleeps, {} rebases",
        report.guest,
        report.frames.published,
        report.frames.shown,
        report.frames.dropped,
        report.pacing.sleeps,
        report.pacing.rebases
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stale_or_missing_dylib_is_rebuilt() {
        let dir = std::env::temp_dir().join(format!("h2-play-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join(boot::DYLIB);
        let xbe = b"XBEH synthetic image bytes";
        assert!(needs_build(xbe, &lib), "missing");
        std::fs::write(&lib, b"").unwrap();
        assert!(needs_build(xbe, &lib), "no stamp");
        let stamp = dir.join(format!("{}.stamp", boot::DYLIB));
        std::fs::write(&stamp, boot::build_stamp(b"another image")).unwrap();
        assert!(needs_build(xbe, &lib), "another image");
        std::fs::write(
            &stamp,
            boot::build_stamp(xbe).replace("generator ", "generator old"),
        )
        .unwrap();
        assert!(needs_build(xbe, &lib), "another generator");
        std::fs::write(&stamp, boot::build_stamp(xbe)).unwrap();
        assert!(!needs_build(xbe, &lib), "current");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn play_bindings_add_enter_and_backspace() {
        let b = play_bindings();
        assert!(b.inputs(Control::A).contains(&Input::Key("enter".into())));
        assert!(b.inputs(Control::A).contains(&Input::Key("space".into())));
        assert!(
            b.inputs(Control::B)
                .contains(&Input::Key("backspace".into()))
        );
        assert!(
            b.inputs(Control::Start)
                .contains(&Input::Key("escape".into()))
        );
        assert!(
            b.inputs(Control::DpadDown)
                .contains(&Input::Key("arrowdown".into()))
        );
    }

    #[test]
    fn enter_and_backspace_reach_the_keyboard_pad() {
        // The window forwards Enter without Cmd as "Enter" (app::keymap); the
        // keyboard pad must report it as A, and Backspace as B.
        use input::backend::keyboard::KeyboardMouse;
        use input::host::HostButton;
        let kb = KeyboardMouse::new(play_bindings());
        assert!(kb.key_down("Enter"));
        assert!(kb.state().buttons.contains(HostButton::South));
        kb.key_up("Enter");
        assert!(!kb.state().buttons.contains(HostButton::South));
        assert!(kb.key_down("Backspace"));
        assert!(kb.state().buttons.contains(HostButton::East));
    }
}
