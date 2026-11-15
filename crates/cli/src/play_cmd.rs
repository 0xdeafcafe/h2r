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
          [--input-timeline FILE] [--frame-stats]
  h2 play --follow [--repo PATH] [--branch B] [--remote R] [--no-fetch] [--poll SECS]
          [--fresh-data] [--cache DIR] <the arguments above>

Opens a window and runs the recompiled game live (builds the dylib first when
--out has none, or one built from another image or generator). --out and
--data default to $TMPDIR/h2boot and $TMPDIR/h2boot-data, as for `h2 run`. --slices is unlimited by default.
--status prints the frame rates once a second and the slice count every
1000 slices (the unit H2_BOOT_INPUT scripts in). --frame-stats (or
H2_FRAME_STATS=1) shows fps, p50/p99/max frame time and the split into game
CPU, push-buffer decode, wgpu encode, submit, GPU wait and pacing over the
last 5 s, in the window and once a second on stderr, and the worst frames at
exit (docs/frame-budget.md). --input-timeline plays an
input crate timeline (docs/input.md) on guest time instead of live input.
--follow keeps the game running while it builds each new commit of the
branch, then offers a restart (`h2 play --follow --help`).

Keys: Escape START, Space or Enter A, V or Backspace B, E X, Q Y, Tab BACK,
arrows d-pad, WASD left stick, mouse look (click to capture). Cmd+Q quits,
Cmd+F fullscreen, Cmd+P pause. With --follow, F5 restarts into a ready update
and F6 hides the banner. --bindings reads a TOML table (docs/input.md).";

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
    let mut gfx = d3d::GfxBackend::new(host.gfx_context()).on_present(host.present_hook());
    // As for `h2 run`: passes are merged by default, and
    // H2_GFX_MERGE_PASSES=0 gives one render pass per draw (docs/BOOT.md).
    if std::env::var("H2_GFX_MERGE_PASSES").is_ok_and(|v| v == "0") {
        gfx.set_merge_passes(false);
    }
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
                if interrupted.load(Ordering::Relaxed) {
                    return false;
                }
                let _pace = timeline::phase::enter(timeline::phase::Phase::Pace);
                host.tick(now) == app::Control::Continue
            }
        })),
    }
}

/// How long after the first signal the process exits regardless (the
/// guest's report gets `shutdown_timeout`, 10 s, of this).
const SIGNAL_DEADLINE: Duration = Duration::from_secs(20);

/// What a terminal signal does.
#[derive(Debug, PartialEq, Eq)]
enum OnSignal {
    /// The first: the run is stopping and the window closing.
    Stopping,
    /// A second: exit now.
    ExitNow,
}

/// The first signal ends the run like a window close: the guest is asked to
/// stop (waking it from a pause), and the window closes even if the guest
/// has already exited and the window only shows its outcome. A second
/// exits at once.
fn on_signal(interrupted: &AtomicBool, closer: &app::Closer) -> OnSignal {
    if interrupted.swap(true, Ordering::Relaxed) {
        return OnSignal::ExitNow;
    }
    closer.close();
    OnSignal::Stopping
}

/// Ctrl-C or SIGTERM in the terminal: see [`on_signal`]; if the process is
/// still alive [`SIGNAL_DEADLINE`] after the first, it exits anyway. Must
/// run before any thread exists, so every thread inherits the blocked mask
/// and only the watcher sees the signals. A signal the parent set to ignore
/// (a shell's `&` ignores SIGINT) stays ignored.
#[cfg(unix)]
fn watch_signals(closer: app::Closer) -> Arc<AtomicBool> {
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
                match on_signal(&f, &closer) {
                    OnSignal::ExitNow => std::process::exit(130),
                    OnSignal::Stopping => {
                        eprintln!("play: interrupted; stopping the run (again to exit now)");
                        let _ = std::thread::Builder::new().name("signal-deadline".into()).spawn(|| {
                            std::thread::sleep(SIGNAL_DEADLINE);
                            eprintln!("play: still running {SIGNAL_DEADLINE:?} after the signal; exiting");
                            std::process::exit(130);
                        });
                    }
                }
            }
        })
        .expect("spawning the signal watcher");
    flag
}

#[cfg(not(unix))]
fn watch_signals(_: app::Closer) -> Arc<AtomicBool> {
    Arc::new(AtomicBool::new(false))
}

pub fn run(args: Vec<String>) -> Result<()> {
    #[cfg(unix)]
    if args.iter().any(|a| a == "--follow") {
        return crate::follow::run(args);
    }
    let tmp = std::env::temp_dir();
    let mut out = tmp.join("h2boot");
    let mut data = tmp.join("h2boot-data");
    let mut slices = u64::MAX;
    let mut transfers = 50_000_000u64;
    let mut disc = None;
    let mut audio = true;
    let mut follow_status = None;
    let mut frame_stats = timeline::phase::Config::from_env();
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
            "--frame-stats" => {
                frame_stats.get_or_insert(timeline::phase::Config {
                    log_every_ns: Some(1_000_000_000),
                    window_ns: timeline::phase::Config::DEFAULT_WINDOW_NS,
                    worst: 10,
                    ..Default::default()
                });
            }
            "--follow-status" => follow_status = Some(PathBuf::from(val()?)),
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
    let closer = app::Closer::new();
    config.closer = Some(closer.clone());
    let interrupted = watch_signals(closer);
    // After the signal mask is set, so the reader thread inherits it.
    #[cfg(unix)]
    if let Some(path) = follow_status {
        config.banner = Some(follow_banner(path));
    }
    #[cfg(not(unix))]
    let _ = follow_status;
    if let Some(c) = frame_stats {
        timeline::phase::start(c).context("starting frame stats")?;
        // The follow banner wins the corner; the stats still log.
        if config.banner.is_none() {
            config.banner = Some(stats_banner());
        }
    }
    let report = app::run(config, move |host| {
        let host = hosted(host, audio, status, timeline, Arc::clone(&interrupted));
        let r = boot::run_hosted(&xbe, &lib, &opts, host);
        // As `h2 run` does: the kernel's storage may outlive the run (or never
        // drop), so store its pending write metadata at a normal exit, whether
        // the run ended well or not. See docs/storage.md "Write metadata".
        vfs::flush_pending_metadata();
        let r = r?;
        for l in &r.kernel_log {
            println!("DbgPrint: {}", l.trim_end());
        }
        println!(
            "outcome: {}",
            r.outcome.chars().take(400).collect::<String>()
        );
        println!("stop: {}", r.stop);
        println!("last eip: {:#010x}", r.eip);
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
    #[cfg(unix)]
    if report.restart_requested {
        println!("play: restarting into the update");
        std::process::exit(crate::follow::RESTART_EXIT);
    }
    Ok(())
}

/// A banner showing the frame stats' rolling window, refreshed once a
/// second (`--frame-stats`).
fn stats_banner() -> app::Banner {
    let banner = app::Banner::new();
    let b = banner.clone();
    std::thread::Builder::new()
        .name("frame-stats".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(1));
                if !timeline::phase::enabled() {
                    b.set(None);
                    return;
                }
                b.set(stats_notice(timeline::phase::window_summary()));
            }
        })
        .expect("spawning the frame-stats reader");
    banner
}

fn stats_notice(s: Option<timeline::phase::Summary>) -> Option<app::Notice> {
    s.map(|s| app::Notice {
        kind: app::NoticeKind::Info,
        text: s.short_line(),
    })
}

/// The window's banner, kept in step with the `--follow` supervisor's
/// status file (polled twice a second).
#[cfg(unix)]
fn follow_banner(path: PathBuf) -> app::Banner {
    let banner = app::Banner::new();
    let b = banner.clone();
    std::thread::Builder::new()
        .name("follow-status".into())
        .spawn(move || {
            loop {
                let text = std::fs::read_to_string(&path).unwrap_or_default();
                b.set(crate::follow::parse_status(&text));
                std::thread::sleep(Duration::from_millis(500));
            }
        })
        .expect("spawning the status reader");
    banner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_signal_closes_the_window_and_a_second_exits() {
        let flag = AtomicBool::new(false);
        let closer = app::Closer::new();
        assert_eq!(on_signal(&flag, &closer), OnSignal::Stopping);
        assert!(closer.requested() && flag.load(Ordering::Relaxed));
        assert_eq!(on_signal(&flag, &closer), OnSignal::ExitNow);
    }

    #[test]
    fn the_stats_notice_fits_the_banner() {
        assert_eq!(stats_notice(None), None);
        let s = timeline::phase::Summary {
            frames: 75,
            fps: 14.8,
            p50_ms: 61.2,
            p99_ms: 998.4,
            max_ms: 1402.0,
            phase_ms: [40.1, 3.2, 8.4, 2.0, 5.5, 0.0],
        };
        let n = stats_notice(Some(s)).unwrap();
        assert_eq!(n.kind, app::NoticeKind::Info);
        assert_eq!(
            n.text,
            "14.8 fps p50 61 p99 998 max 1402 ms | cpu 40 dec 3 enc 8 sub 2 gpu 6 pace 0"
        );
        // The banner cuts at 96 characters.
        assert!(n.text.len() < 96);
    }

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
