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
          [--input-timeline FILE] [--frame-stats] [--no-watermark] [--no-mappack]
          [--live | --live-server URL] [--live-listen ADDR] [--gamertag NAME]
          [--icons auto|xbox|xbox360|playstation|keyboard]
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
exit (docs/frame-budget.md). A two-line development-build watermark (commit,
branch, build time, fps, frame time, draws, map, renderer) runs along the
bottom of the picture; F3 toggles it, --no-watermark or H2_WATERMARK=0 turns
it off. --input-timeline plays an
input crate timeline (docs/input.md) on guest time instead of live input.
A mappack.iso next to the disc (or another .iso there with the map pack's
layout) is installed into --data on the way in, once: a warning never stops
the game. --no-mappack or H2_MAPPACK=0 skips it (docs/map-pack.md).
--follow keeps the game running while it builds each new commit of the
branch, then offers a restart (`h2 play --follow --help`).

--live signs the game in to Xbox Live (docs/live.md): it starts a service in
the process (Postgres from $H2_LIVE_DB or postgres://localhost/h2live, made if
missing; else in memory) on 127.0.0.1 and a free port, or --live-listen ADDR,
serving the web app from web/dist ($H2_LIVE_WEB). --live-server URL uses a
running service instead. The account is --gamertag NAME, or one named after
your user; either is made on the server and kept in accounts.json if needed.

Keys: Escape START, Space or Enter A, V or Backspace B, E X, Q Y, Tab BACK,
arrows d-pad, WASD left stick, mouse look (click to capture). Cmd+Q quits,
Cmd+F fullscreen, Cmd+P pause, F3 the watermark. With --follow, F5 restarts into a ready update
and F6 hides the banner. --bindings reads a TOML table (docs/input.md).
--icons (or H2_ICONS) picks the button pictures in prompts: auto, the default,
follows the device last used (docs/button-icons.md).";

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

/// The DualSense HID thread (mute button and LED, `H2_PAD_MUTE`, on by
/// default; the speaker path) and, with `H2_PAD_SPEAKER` on and audio on,
/// the controller-speaker output (docs/playstation-audio.md). A bad
/// setting is reported and that part stays off.
fn pad_speaker(audio: bool) -> (Option<boot::dsound_glue::PadSpeaker>, Option<input::pad_hid::PadHid>) {
    let env = |k: &str| std::env::var(k).ok();
    let config = dsound::padroute::PadSpeakerConfig::from_env(env)
        .map_err(|e| eprintln!("play: controller speaker off: {e}"))
        .ok()
        .filter(|c| c.enabled && audio);
    let hid_config = input::pad_hid::PadHidConfig::from_env(env)
        .map_err(|e| eprintln!("play: {e}"))
        .unwrap_or(input::pad_hid::PadHidConfig { mute: false });
    let hid = pad_hid(hid_config, config.is_some());
    let Some(config) = config else {
        return (None, hid);
    };
    let device = config.device.clone();
    let switch = hid.as_ref().map(input::pad_hid::PadHid::speaker_switch);
    let side = dsound::SideOutput {
        open: Box::new(move || match audio::output::DeviceSink::open_named(&device) {
            Ok(d) => {
                eprintln!("play: controller speaker on {} at {} Hz", d.device_id(), audio::AudioSink::sample_rate(&d));
                Some(Box::new(d) as Box<dyn audio::AudioSink>)
            }
            Err(e) => {
                eprintln!("play: no controller speaker matching {device:?} ({e}); every sound stays on the main output");
                None
            }
        }),
        on_live: switch.map(|s| Box::new(move |live| s.set(live)) as Box<dyn FnMut(bool) + Send>),
    };
    (Some(boot::dsound_glue::PadSpeaker { config, side }), hid)
}

/// The HID thread, when it has something to do and can run here.
fn pad_hid(config: input::pad_hid::PadHidConfig, speaker: bool) -> Option<input::pad_hid::PadHid> {
    let open = input::pad_hid::platform_opener().filter(|_| config.mute || speaker)?;
    Some(input::pad_hid::spawn(open, config, Default::default()))
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
    icons: boot::icons::IconOptions,
) -> boot::Host {
    let mut gfx = d3d::GfxBackend::new(host.gfx_context()).on_present(host.present_hook());
    // As for `h2 run`: passes are merged by default (the fast rule;
    // H2_GFX_MERGE_PASSES=exact, read by GfxBackend::new, gives the exact
    // one), and H2_GFX_MERGE_PASSES=0 gives one render pass per draw
    // (docs/BOOT.md).
    if std::env::var("H2_GFX_MERGE_PASSES").is_ok_and(|v| v == "0") {
        gfx.set_merge_passes(false);
    }
    // Build last run's pipelines ahead of time (H2_PIPELINE_CACHE; docs/graphics.md).
    if let Some(path) = d3d::pipeline_store::default_path() {
        gfx = gfx.with_pipeline_cache(path);
    }
    let input = host.take_input();
    let keyboard = host.keyboard().cloned();
    let (pad_speaker, pad_hid) = pad_speaker(audio);
    boot::Host {
        gfx: Some(gfx),
        // The backend's work on a render thread unless H2_RENDER_THREAD=0, as
        // for `h2 run` (docs/render-thread.md).
        render_thread: render_thread(std::env::var("H2_RENDER_THREAD").ok().as_deref()),
        audio_sink: Some(audio_sink(audio)),
        pad_speaker,
        trace_limit: Some(TRACE_LIMIT),
        input,
        keyboard,
        icons: Some(icons),
        between_slices: Some(Box::new({
            let mut slices = 0u64;
            // The HID thread lives as long as the run.
            let _pad_hid = pad_hid;
            move |k: &kernel::Kernel| {
                let _ = &_pad_hid;
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
    let mut no_mappack = false;
    let mut follow_status = None;
    let mut icons: Option<font::icons::IconChoice> = None;
    let mut frame_stats = timeline::phase::Config::from_env();
    let mut no_watermark = false;
    let mut live = crate::play_live::LiveOpts::default();
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
        if live.take(&a, &mut val)? {
            continue;
        }
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
            "--no-mappack" => no_mappack = true,
            "--no-gamepads" => config.gamepads = false,
            "--no-vsync" => config.vsync = false,
            "--unpaced" => config.pacing = app::PacePolicy::Unpaced,
            "--status" => config.log_status = true,
            "--no-watermark" => no_watermark = true,
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
            "--icons" => {
                icons = Some(
                    val()?
                        .parse()
                        .map_err(|e: String| anyhow::anyhow!("--icons: {e}"))?,
                )
            }
            "-h" | "--help" => bail!("{USAGE}"),
            _ if disc.is_none() => disc = Some(PathBuf::from(a)),
            _ => bail!("unexpected {a}\n\n{USAGE}"),
        }
    }
    live.check()?;
    let disc = disc
        .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        .context(USAGE)?;
    // --icons, then H2_ICONS, then auto: follow the device in use
    // (docs/button-icons.md).
    let icons = boot::icons::IconOptions {
        choice: match icons {
            Some(c) => c,
            None => boot::icons::choice_from_env()?.unwrap_or_default(),
        },
        gamepads: config.gamepads,
    };
    let xbe = boot::read_xbe(&disc)?;
    let lib = out.join(boot::DYLIB);
    if needs_build(&xbe, &lib) {
        let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
        boot::build(&xbe, &out, jobs)?;
    }
    if crate::mappack_cmd::auto_enabled(no_mappack) {
        crate::mappack_cmd::auto_install(&disc, &data);
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
    // Before the window: the session (and an embedded server) lives until
    // the game is over.
    let session = live
        .enabled
        .then(|| start_live(&live, &opts.disc))
        .transpose()?;
    let live_config = session
        .as_ref()
        .map(|s| s.config.clone())
        .unwrap_or_default();
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
    let watermark = watermark_on(no_watermark, std::env::var("H2_WATERMARK").ok().as_deref());
    if watermark {
        config.watermark = Some(play_watermark(render_thread(
            std::env::var("H2_RENDER_THREAD").ok().as_deref(),
        )));
    }
    if let Some(c) = frame_stats {
        timeline::phase::start(c).context("starting frame stats")?;
        // The follow banner wins the corner; the stats still log.
        if config.banner.is_none() {
            config.banner = Some(stats_banner());
        }
    } else if watermark {
        // The watermark's numbers: a short window, no log and no report.
        timeline::phase::start(timeline::phase::Config::live(WATERMARK_WINDOW_NS))
            .context("starting frame stats")?;
    }
    let report = app::run(config, move |host| {
        let host = hosted(
            host,
            audio,
            status,
            timeline,
            Arc::clone(&interrupted),
            icons,
        );
        let r = boot::run_hosted_with_live(&xbe, &lib, &opts, host, &live_config);
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
    drop(session);
    #[cfg(unix)]
    if report.restart_requested {
        println!("play: restarting into the update");
        std::process::exit(crate::follow::RESTART_EXIT);
    }
    Ok(())
}

/// Starts `--live`: the server and account, with a line saying where the
/// web app is.
fn start_live(
    live: &crate::play_live::LiveOpts,
    disc: &std::path::Path,
) -> Result<crate::play_live::Session> {
    let db = std::env::var("H2_LIVE_DB")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| live_server::DEFAULT_DB.into());
    let places = crate::play_live::Places::under(&crate::live_cmd::live_dir()?);
    // Build the web app first if it is missing or older than its sources
    // (H2_LIVE_WEB points at a build of your own and skips this).
    if std::env::var_os("H2_LIVE_WEB").is_none() {
        let src = crate::web_build::source_dir();
        if src.join("package.json").is_file() {
            let bun = crate::web_build::find_bun();
            let tree = crate::web_build::clean_tree(&src);
            match crate::web_build::ensure(
                &src,
                bun.as_deref(),
                tree.as_deref(),
                &mut |what, cmd| {
                    eprintln!("live: building the web app: {what}");
                    crate::web_build::run_inherit(what, cmd)
                },
            ) {
                Ok(_) => {}
                Err(e) => eprintln!("live: no web app: {e}"),
            }
        }
    }
    let web = live_server::web_dir();
    let has_web = web.is_some();
    // Art for the embedded server, from this disc (extracted once).
    let game_art = live
        .server
        .is_none()
        .then(|| {
            crate::play_live::game_art(disc, &crate::live_cmd::game_art_dir(&places.server_data))
        })
        .flatten();
    let site = live_server::http::Site { web, game_art };
    let session = crate::play_live::start(live, &db, &places, site, &crate::play_live::os_user())?;
    eprintln!(
        "live: playing as {} ({}) on {}{}",
        session.account.gamertag,
        session.account.xuid,
        session.server,
        session
            .store()
            .map_or(String::new(), |s| format!(" (embedded, {s} store)"))
    );
    match (session.store(), has_web) {
        (Some(_), true) => eprintln!("live: web app at {}/", session.server),
        (Some(_), false) => eprintln!("live: no web app; build it with `cd web && bun run build`"),
        (None, _) => {}
    }
    Ok(session)
}

/// The watermark's rolling window when `--frame-stats` is off: short, so
/// the numbers follow what is on screen.
const WATERMARK_WINDOW_NS: u64 = 2_000_000_000;

/// The watermark is on unless `--no-watermark` or `H2_WATERMARK=0`.
fn watermark_on(no_flag: bool, env: Option<&str>) -> bool {
    !no_flag && env != Some("0")
}

/// `h2 play`'s watermark: how the renderer runs, and the level map the
/// game opened last.
fn play_watermark(render_thread: bool) -> app::Watermark {
    app::Watermark::new()
        .with_note(if render_thread {
            "render thread on"
        } else {
            "render thread off"
        })
        .with_map(kernel::level::last_map)
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

/// Whether `h2 play` renders on a render thread, from `H2_RENDER_THREAD`
/// (the same rule as `h2 run`'s).
fn render_thread(value: Option<&str>) -> bool {
    boot::d3d_glue::render_thread_from(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn h2_render_thread_is_on_unless_zero() {
        assert!(render_thread(None), "on when unset");
        assert!(!render_thread(Some("0")), "H2_RENDER_THREAD=0 turns it off");
        for v in ["1", "", "true", "yes", "2", " 0"] {
            assert!(render_thread(Some(v)), "{v:?}");
        }
    }

    #[test]
    fn the_watermark_is_on_unless_turned_off() {
        assert!(watermark_on(false, None));
        assert!(watermark_on(false, Some("1")));
        assert!(watermark_on(false, Some("")));
        assert!(!watermark_on(false, Some("0")));
        assert!(!watermark_on(true, None));
        assert!(!watermark_on(true, Some("1")));
    }

    #[test]
    fn the_play_watermark_names_the_render_thread() {
        let wm = play_watermark(true);
        assert!(wm.visible());
        let p = wm.panels(None, "metal");
        assert_eq!(p.label[0], "h2recomp development build");
        assert_eq!(p.live.last().unwrap(), "metal, render thread on");
        let off = play_watermark(false).panels(None, "metal");
        assert_eq!(off.live.last().unwrap(), "metal, render thread off");
    }

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
            phase_ms: [40.1, 3.2, 8.4, 2.0, 5.5, 0.0, 6.4],
            count_mean: [0.0; timeline::phase::COUNTERS],
        };
        let n = stats_notice(Some(s)).unwrap();
        assert_eq!(n.kind, app::NoticeKind::Info);
        assert_eq!(
            n.text,
            "14.8 fps p50 61 p99 998 max 1402 ms | cpu 40 dec 3 enc 8 sub 2 gpu 6 pace 0 rt 6"
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
