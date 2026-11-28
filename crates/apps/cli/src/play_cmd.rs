//! `h2 play`: the boot driver (crates/apps/boot) inside the host shell
//! (crates/apps/app). The window stays on the main thread; the game runs on
//! app's guest thread under `boot::run_hosted`, presenting through the
//! shell's frame handoff, playing DirectSound's logical-time output on the
//! default audio device, and reading the window's keyboard and the host's
//! pads. See docs/runtime/app.md "Playing: h2 play".

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use agent::editor::{EditorLink, MarkAvailability};
use agent_proto::editor::Mode;
use anyhow::{Context, Result, bail};
use input::backend::scripted::{ScriptedInput, Timeline};
use input::bindings::{Bindings, Control, Input};

const USAGE: &str = "usage:
  h2 play <disc.iso> [--out DIR] [--tier auto|bridge|fast|clang] [--data DIR] [--transfers N] [--slices N]
          [--aspect source|4:3|16:9] [--scaling fit|integer|stretch] [--nearest] [--fullscreen]
          [--title TEXT] [--tile I/N]
          [--no-audio] [--no-gamepads] [--no-vsync] [--unpaced] [--bindings FILE] [--status]
          [--fps 30|60|120|uncapped] [--input-timeline FILE] [--frame-stats] [--no-watermark] [--no-mappack] [--no-halo1]
          [--movies DIR | --no-movies] [--film FILE]
          [--live | --live-server URL | --live-local | --offline] [--gamertag NAME] [--invite CODE]
          [--join URL] [--signin-terminal]
          [--live-listen ADDR] [--live-tunnel [--live-tunnel-name NAME
           [--live-tunnel-url https://HOST]] [--no-tunnel-setup]]
          [--icons auto|xbox|xbox360|playstation|keyboard|remote]
          [--level MAP [--difficulty easy|normal|heroic|legendary]] [--set KEY=VALUE]...
          [--editor [--window] [--game-variant FILE]]
  h2 play --follow [--repo PATH] [--branch B] [--remote R] [--no-fetch] [--poll SECS]
          [--fresh-data] [--cache DIR] <the arguments above>
  h2 play --watch [--repo PATH] [--quiet MS] [--fresh-data] [--cache DIR] <the arguments above>

Opens a window and runs the recompiled game live (builds the dylib first when
--out has none, or one built from another image or generator). --out and
--data default to $TMPDIR/h2boot and `data` in the app data directory
(docs/runtime/storage.md; $H2_DATA if set; saves live there), as for `h2 run`.
--tier (or H2_TIER) as for `h2 run`: by default clang's module, built first
when it is not current; `bridge` starts on the Cranelift fast tier (built in
seconds) with clang's build in the background for the next start. --slices
is unlimited by default. When
the guest returns, the window stays up with the outcome in its title, except
for a run with --slices or H2_BOOT_STOP_AT_PRESENT, or a lab job: those close
it and exit.
--film FILE records guest inputs and the simulation to a new .h2film file.
--status prints the frame rates once a second and the slice count every
1000 slices (the unit H2_BOOT_INPUT scripts in). --frame-stats (or
H2_FRAME_STATS=1) shows fps, p50/p99/max frame time and the split into game
CPU, push-buffer decode, wgpu encode, submit, GPU wait and pacing over the
last 5 s, in the window and once a second on stderr, and the worst frames at
exit (docs/perf/notes/gameplay-budget.md). A two-line development-build watermark (commit,
branch, build time, fps, frame time, draws, map, renderer) runs along the
bottom of the picture; F3 toggles it, --no-watermark or H2_WATERMARK=0 turns
it off; H2_WATERMARK=full adds the player, level, world and render picture. --input-timeline plays an
input crate timeline (docs/input/input.md) on guest time instead of live input.
A mappack.iso next to the disc (or another .iso there with the map pack's
layout) is installed into --data on the way in, once: a warning never stops
the game. --no-mappack or H2_MAPPACK=0 skips it (docs/game/map-pack.md).
A halo1.iso next to the disc is converted into --data on the way in (its
campaign under Campaign, its multiplayer maps in the map list), again only
when the disc or the converter changed; a warning never stops the game.
--no-halo1 or H2_HALO1=0 skips it (docs/halo1/campaign.md).
The intro and credits play, with their sound, when `h2 movies` has installed
them for this disc. An idle menu plays the attract movie from the same install.
On a normal windowed boot with the default movie directory, a missing attract
transcode is prepared from the disc using local ffmpeg when available.
--movies DIR selects another install. --no-movies skips playback, as does
a --level boot unless --movies DIR is given. During playback outside film
sessions, an unavailable attract movie leaves the menu music playing.
--fps (or H2_FPS) sets how often the window presents. At 30 it redraws on
guest presents. At 60, 120 or uncapped it repeats the newest picture between
guest frames unless host interpolation or reprojection is enabled.
--unpaced disables guest wall-clock pacing for benchmarks. For normal-speed
gameplay with unrestricted window redraws, use --fps uncapped.
The Video screen saves frame rate and resolution in <data>/settings.toml.
Native resolution follows the window in physical pixels. H2_RESOLUTION
accepts native or WIDTHxHEIGHT; H2_RENDER_SCALE accepts a legacy multiplier.
--set KEY=VALUE sets any of our settings for this run only, as its environment
variable would: it wins over the variable and the file and locks its row
(`h2 settings list` shows them; docs/game/settings-menu.md).
--editor starts the game for a tool to hold (Studio's own game): it needs
--level, has no window unless --window is given, and is a local game with no
Live, film or --input-timeline (docs/editor/game-view.md). --level MAP (or H2_LEVEL) starts that level instead of the main menu, with one
default player: no title, profile or menus. MAP is a map file name from the
disc: a campaign level (03a_oldmombasa) at --difficulty (or H2_DIFFICULTY;
normal by default), or a multiplayer map (ascension) as a local Slayer game
with the default variant. --game-variant FILE supplies a portable .h2variant
for an --editor multiplayer level. A name the disc lacks lists them (docs/runtime/level-boot.md).
--follow keeps the game running while it builds each new commit of the
branch, then offers a restart (`h2 play --follow --help`). --watch watches your
checkout instead: once a burst of edits or a git pull settles it reloads rules
scripts into the game, rebuilds h2 (and the game code when it must), then
offers the same restart (`h2 play --watch --help`).

With no Live flag it plays offline and contacts nothing (--offline or
--no-live says so explicitly). --live signs the game in to Xbox Live at
https://live.forbes.red ($H2_LIVE_SERVER names another; off: offline);
--live-server URL signs in to that service instead (docs/live/live.md).
--join URL opens a Halo 2 session link and implies --live. The link must
belong to your selected Live service.
The account is one kept in accounts.json for that server; else --gamertag
NAME, or the gamertag you last played as on another server (keeping its
key), or one named after your user. A name new to the server is made
there (--invite CODE if it is invite-only); one it already has is
recovered with the code from Account on its dashboard or your
authenticator app, which the game asks for when you sign in to Live
(--signin-terminal asks on the terminal before the game starts instead,
with an invite too). Signed in, the
account's saves (profiles, variants, campaign saves, profile settings) are
merged into the data root before the game starts and uploaded as it writes
them; H2_CLOUD_SAVES=0 turns that off (docs/live/live-cloud-saves.md).
--live-local runs the service in the process instead (Postgres from
$H2_LIVE_DB or postgres://localhost/h2live, made if missing; else a log in
the live data directory's server/log, so accounts, friends, games and the
MOTD are kept) on 127.0.0.1 and a free port, or --live-listen ADDR (which
implies --live-local), serving the web app from web/dist ($H2_LIVE_WEB). It
does not go with --live-server.
--live-tunnel (implies --live-local; --tunnel is the same) publishes
the embedded server through a cloudflared quick tunnel so friends can join with
--live-server <its URL>. With --live-tunnel-name NAME --live-tunnel-url
https://HOST (or --tunnel-name NAME --hostname HOST) it runs named tunnel NAME
at that lasting URL instead, first logging in to Cloudflare (a browser link),
creating the tunnel and routing HOST to it if any of that is missing;
--no-tunnel-setup skips those steps (docs/live/live-hosting.md).

--title names the window (Halo 2 by default); --tile I/N sizes it to
column I (from 0) of N across the main screen and places it there, so
several games sit side by side (`h2 coop test` does both).

Keys: Escape START, Space or Enter A, V or Backspace B, E X, Q Y, Tab BACK,
arrows d-pad, WASD left stick, mouse look (click to capture). Cmd+Q quits,
Cmd+F fullscreen, Cmd+P pause, F3 the watermark, ` (backquote) the debug menu
(a touchpad click on a DualSense or DualShock 4; H2_DEBUG_MENU=0 turns it off;
`h2 agent hostpads` shows what the pads report;
docs/runtime/debug-menu.md). With --follow or --watch, F5 restarts into a ready update
and F6 hides the banner. --bindings reads a TOML table (docs/input/input.md).
--icons (or H2_ICONS) picks the button pictures in prompts: auto, the default,
follows the device last used (docs/input/button-icons.md).";

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

/// What [`pad_speaker`] sets up: the output, the HID thread and the switch.
type PadParts = (
    Option<boot::dsound_glue::PadSpeaker>,
    Option<input::pad_hid::PadHid>,
    Arc<boot::speaker_setting::SpeakerSetting>,
);

/// The DualSense HID thread (mute button and LED, `H2_PAD_MUTE`, on by
/// default; the speaker path), the on/off switch behind the Controls
/// screen's Controller Speaker row (`pad_speaker.enabled` in `store`;
/// `H2_PAD_SPEAKER`, when set, wins) and, unless `H2_PAD_SPEAKER=0` or audio
/// is off, the controller-speaker output (docs/audio/playstation-audio.md). A bad
/// setting is reported and that part stays off.
///
/// Where no PlayStation controller's audio device is found and a Steam
/// Controller thread runs (`steam`), the routed sounds go to its trackpad
/// actuators instead, as haptics (`H2_PAD_SPEAKER_DEVICE=haptics` asks for
/// that alone; docs/input/steam-controller.md). The device list is read once here.
fn pad_speaker(
    audio: bool,
    store: &Arc<settings::Store>,
    steam: Option<&input::steam_pad::SteamPads>,
) -> PadParts {
    let env = |k: &str| std::env::var(k).ok();
    let config = dsound::padroute::PadSpeakerConfig::from_env(env)
        .map_err(|e| eprintln!("play: controller speaker off: {e}"))
        .ok();
    // No audio, or bad controller-speaker settings: off, and the row locked.
    let why = if !audio {
        Some("audio is off")
    } else {
        config.is_none().then_some("its settings are bad")
    };
    if let Some(why) = why {
        let _ = store.force(
            boot::setting_defs::PAD_SPEAKER.key,
            settings::Value::Bool(false),
            why,
        );
    }
    let setting = Arc::new(boot::speaker_setting::SpeakerSetting::new(Arc::clone(
        store,
    )));
    // The output is opened whenever the variable allows it, so the row can
    // turn the speaker on during the run; the setting decides what reaches it.
    let config = config.filter(|c| c.enabled && audio);
    // The mute button: `voice.mute_button` (H2_PAD_MUTE, the Voice screen's
    // Mute Button row). A row press reaches the HID thread at once, so the
    // thread runs while the row can still turn it on (not when
    // H2_PAD_MUTE=0 locks it off and the speaker is off).
    let key = boot::setting_defs::PAD_MUTE.key;
    let mute = store.handle::<bool>(key).get();
    let may_mute = mute || !store.state(key).is_some_and(|s| s.locked);
    let hid_config = input::pad_hid::PadHidConfig { mute };
    let hid = pad_hid(hid_config, may_mute || config.is_some());
    if let Some(switch) = hid.as_ref().map(input::pad_hid::PadHid::mute_switch) {
        store.on_change(key, move |v| switch.set(v.as_bool().unwrap_or(true)));
    }
    let Some(config) = config else {
        return (None, hid, setting);
    };
    if !setting.enabled() {
        eprintln!(
            "play: controller speaker off (Settings > Controls > Controller Speaker turns it on)"
        );
    }
    let names = audio::output::DeviceSink::output_device_names();
    let target = audio::devices::pick_speaker(&names, &config.device, steam.is_some());
    if let (Some(audio::devices::Speaker::Haptics), Some(pads)) = (target, steam) {
        eprintln!("play: controller speaker on the Steam Controller's trackpads, as haptics");
        let pads = pads.clone();
        let live = Arc::clone(&setting);
        let side = dsound::SideOutput {
            open: Box::new(move || {
                Some(Box::new(boot::steam_speaker::HapticSink::over(pads))
                    as Box<dyn audio::AudioSink>)
            }),
            on_live: Some(Box::new(move |on| live.side_live(on))),
        };
        let pad = boot::dsound_glue::PadSpeaker {
            config: config.for_haptics(),
            side,
            setting: Arc::clone(&setting),
        };
        return (Some(pad), hid, setting);
    }
    if let Some(switch) = hid.as_ref().map(input::pad_hid::PadHid::speaker_switch) {
        setting.attach_hid(move |on| switch.set(on));
    }
    let device = config.device.clone();
    let live = Arc::clone(&setting);
    let side = dsound::SideOutput {
        open: Box::new(
            move || match audio::output::DeviceSink::open_named(&device) {
                Ok(d) => {
                    eprintln!(
                        "play: controller speaker on {} at {} Hz",
                        d.device_id(),
                        audio::AudioSink::sample_rate(&d)
                    );
                    Some(Box::new(d) as Box<dyn audio::AudioSink>)
                }
                Err(e) => {
                    eprintln!(
                        "play: no controller speaker matching {device:?} ({e}); every sound stays on the main output"
                    );
                    None
                }
            },
        ),
        on_live: Some(Box::new(move |on| live.side_live(on))),
    };
    let pad = boot::dsound_glue::PadSpeaker {
        config,
        side,
        setting: Arc::clone(&setting),
    };
    (Some(pad), hid, setting)
}

/// The Steam Controller thread, when pads are wanted, the platform has raw
/// HID and `H2_STEAM_PAD` is not `0`. Its tuning follows the Controls screen's
/// Steam rows (`steam_pad.*`).
fn steam_pads(
    config: &app::AppConfig,
    store: &settings::Store,
) -> Option<input::steam_pad::SteamPads> {
    if !config.gamepads
        || config.scripted_input.is_some()
        || std::env::var("H2_STEAM_PAD").is_ok_and(|v| v == "0")
    {
        return None;
    }
    let open = input::steam_pad::platform_opener()?;
    let tuning = boot::setting_defs::steam_tuning(store);
    Some(input::steam_pad::spawn(open, tuning, Default::default()))
}

/// The HID thread, when it may have something to do and can run here.
fn pad_hid(config: input::pad_hid::PadHidConfig, wanted: bool) -> Option<input::pad_hid::PadHid> {
    let open = input::pad_hid::platform_opener().filter(|_| wanted)?;
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
    store: Arc<settings::Store>,
    steam: Option<input::steam_pad::SteamPads>,
) -> boot::Host {
    let native_target: boot::run_opts::NativeTarget = Arc::new(host.native_target());
    let view = boot::run_opts::DisplayView {
        native_target,
        viewer_output: None,
        viewer_lod: None,
        display_size: Arc::new(host.display_size()),
    };
    let gfx = host.gfx_context();
    let presenter = host.presenter(mark_frame);
    let input = host.take_input();
    let keyboard = host.keyboard().cloned();
    let (pad_speaker, pad_hid, _speaker_setting) = pad_speaker(audio, &store, steam.as_ref());
    let fov = store.handle::<u32>(boot::setting_defs::FOV.key).get();
    if fov != boot::setting_defs::FOV_RETAIL as u32 {
        eprintln!("play: field of view {fov} degrees");
    }
    boot::Host {
        film_out: None,
        film_auto: Default::default(),
        film_view: None,
        film_media: None,
        film_capture: None,
        host_light: false,
        host_bots: false,
        gfx: Some(gfx),
        presenter: Some(presenter),
        gfx_view: Some(view),
        unpaced_display: host.pace_policy() == app::PacePolicy::Unpaced,
        audio_sink: Some(audio_sink(audio)),
        voice_io: Some(voice::host::HostVoiceIo::opener()),
        pad_speaker,
        debug_menu: None,
        theater: None,
        theater_playback: None,
        perf_overlay: None,
        cheat_line: None,
        readout: None,
        game_code: None,
        editor_link: None,
        debug_views: None,
        world_line_sources: Vec::new(),
        discoverable_agent: false,
        embedded_live: false,
        settings: Some(store),
        trace_limit: Some(TRACE_LIMIT),
        // `run` sets it from --level or H2_LEVEL, resolved before the window.
        level: None,
        input,
        keyboard,
        icons: Some(icons),
        return_to_menu: Some(host.loading_screen()),
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
        // `run` sets it when cloud saves are on.
        on_vfs: None,
    }
}

/// A player session records into the film library unless it runs as a lab
/// job (its frame times stay comparable with earlier jobs; `H2_FILM_OUT`
/// records one on purpose). A session link joined at launch or later is
/// noted and the recording carries on (`Session::outside_change`).
fn film_auto_request(lab_job: bool) -> boot::theater::autorecord::Request {
    use boot::theater::autorecord::Request;
    if lab_job {
        Request::Skip("a lab job records a film only with H2_FILM_OUT")
    } else {
        Request::On
    }
}

// Windows has no signal watcher yet (Ctrl-C ends the process).
#[cfg_attr(not(unix), allow(dead_code))]
/// How long after the first signal the process exits regardless (the
/// guest's report gets `shutdown_timeout`, 10 s, of this).
const SIGNAL_DEADLINE: Duration = Duration::from_secs(20);

#[cfg_attr(not(unix), allow(dead_code))]
/// What a terminal signal does.
#[derive(Debug, PartialEq, Eq)]
enum OnSignal {
    /// The first: the run is stopping and the window closing.
    Stopping,
    /// A second: exit now.
    ExitNow,
}

#[cfg_attr(not(unix), allow(dead_code))]
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

mod editor;
mod forge;
pub(crate) mod theater;

pub fn run(args: Vec<String>) -> Result<()> {
    run_with_theater(args, Some(theater::open_library))
}

pub(crate) fn run_with_theater(
    args: Vec<String>,
    open_library: Option<boot::theater_ui::adapter::OpenLibrary>,
) -> Result<()> {
    #[cfg(unix)]
    if args.iter().any(|a| a == "--watch") {
        return crate::watch::run(args);
    }
    #[cfg(unix)]
    if args.iter().any(|a| a == "--follow") {
        return crate::follow::run(args);
    }
    // The host's microphones and outputs, for the Voice screen's device
    // rows and `--set voice.microphone=...`: listed once, before any
    // setting is read.
    boot::voice_settings::use_host_devices(&audio::voice::HostDevices);
    let tmp = std::env::temp_dir();
    let mut out = tmp.join("h2boot");
    let mut data = content::store::default_data_root();
    let mut slices = u64::MAX;
    let mut transfers = 50_000_000u64;
    let mut disc = None;
    let mut audio = true;
    let mut movies = boot::movies_glue::Movies::Auto;
    let mut no_mappack = false;
    let mut no_halo1 = false;
    let mut follow_status = None;
    let mut icons: Option<font::icons::IconChoice> = None;
    let mut frame_stats = timeline::phase::Config::from_env();
    let mut no_watermark = false;
    let mut flags: Vec<settings::Flag> = Vec::new();
    let mut live = crate::play_live::LiveOpts::default();
    let mut film_out = None;
    let mut level_flag: Option<String> = None;
    let mut game_variant = None;
    let mut difficulty_flag: Option<String> = None;
    let mut tier: Option<boot::tier::Choice> = None;
    let mut editor_flag = false;
    let mut window_flag = false;
    let mut config = app::AppConfig {
        // boot's DirectSound output owns the device; the shell's own
        // AudioOut would open it a second time (its discard renderer only
        // consumes the unused `GuestHost::mixer`).
        audio: false,
        title: "Halo 2".into(),
        aspect: app::DisplayAspect::Drawable,
        window_matches_monitor_aspect: true,
        bindings: play_bindings(),
        // The run prints its report before the guest thread returns.
        shutdown_timeout: Duration::from_secs(10),
        fps: app::fps::FpsTarget::from_env().map_err(anyhow::Error::msg)?,
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
            "--tier" => tier = Some(boot::tier::Choice::parse(&val()?)?),
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
            "--title" => config.title = val()?,
            "--tile" => {
                let v = val()?;
                config.tile = Some(
                    app::tile::parse(&v)
                        .with_context(|| format!("--tile {v}: column I of N, as 0/2\n\n{USAGE}"))?,
                );
            }
            "--no-audio" => audio = false,
            "--movies" => movies = boot::movies_glue::Movies::Dir(PathBuf::from(val()?)),
            "--no-movies" => movies = boot::movies_glue::Movies::Off,
            "--no-mappack" => no_mappack = true,
            "--no-halo1" => no_halo1 = true,
            "--no-gamepads" => config.gamepads = false,
            "--no-vsync" => config.vsync = false,
            "--fps" => {
                let v = val()?;
                config.fps = app::fps::FpsTarget::parse(&v).map_err(anyhow::Error::msg)?;
                flags.push(settings::Flag {
                    key: boot::setting_defs::FPS.key.into(),
                    value: v,
                    flag: "--fps",
                });
            }
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
            "--set" => flags.push(crate::settings_cmd::set_flag(&val()?)?),
            "--film" => film_out = Some(PathBuf::from(val()?)),
            "--editor" => editor_flag = true,
            "--window" => window_flag = true,
            "--level" => level_flag = Some(val()?),
            "--game-variant" => game_variant = Some(PathBuf::from(val()?)),
            "--difficulty" => difficulty_flag = Some(val()?),
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
    #[cfg(unix)]
    if let (Ok(id), Ok(name)) = (
        std::env::var("H2_LAB_JOB_ID"),
        std::env::var("H2_LAB_JOB_NAME"),
    ) {
        config.title = crate::lab::proto::window_title(&id, &name, app::BuildInfo::baked().sha);
        config.status_in_title = false;
    }
    config.keep_open_on_guest_exit = !close_on_guest_exit(
        slices,
        std::env::var("H2_BOOT_STOP_AT_PRESENT").ok().as_deref(),
        std::env::var_os("H2_LAB_JOB_ID").is_some_and(|v| !v.is_empty()),
    );
    live.check()?;
    let env_set = |k: &str| std::env::var_os(k).is_some_and(|v| !v.is_empty());
    let editor = editor::check(editor::Asked {
        editor: editor_flag,
        window: window_flag,
        level: level_flag.as_deref().is_some_and(|l| !l.trim().is_empty()) || env_set("H2_LEVEL"),
        live: live.enabled || live.local || live.join.is_some(),
        film: film_out.is_some() || env_set("H2_FILM_OUT"),
        timeline: config.scripted_input.is_some(),
    })?;
    if let Some(mode) = editor {
        // A tool holds this game: no window of its own unless asked, and
        // a focus change never pauses it.
        config.hidden = !mode.window;
        config.pause_on_focus_loss = false;
    }
    let film_auto = if editor.is_some() {
        boot::theater::autorecord::Request::Skip("editor mode records no film")
    } else {
        film_auto_request(std::env::var_os("H2_LAB_JOB_ID").is_some_and(|v| !v.is_empty()))
    };
    if live.join.is_some()
        && (film_out.is_some() || std::env::var_os("H2_FILM_OUT").is_some_and(|p| !p.is_empty()))
    {
        bail!("Session links are unavailable while recording a film.");
    }
    let live_choice = live.play_choice(std::env::var("H2_LIVE_SERVER").ok().as_deref())?;
    // Our settings: flags (--fps, --set), then the environment, then
    // <data>/settings.toml, then each default. A bad value is reported and
    // the next source used.
    let (store, problems) = boot::setting_defs::load(&data, &flags);
    for p in problems {
        eprintln!("play: {p}");
    }
    let store = Arc::new(store);
    let resolution = store.handle::<settings::Resolution>(boot::setting_defs::RESOLUTION.key);
    config.native_fit = Some(app::NativeFit::new(move || {
        resolution.get() == settings::Resolution::Native
    }));
    // --fps, then H2_FPS (both checked above), then the Video screen's
    // saved rate, then 30. The row changes it during the run.
    let fps = &boot::setting_defs::FPS;
    let rate = store.get(fps.key).unwrap_or(fps.default);
    config.fps = app::fps::FpsTarget::parse(&fps.name(rate)).map_err(anyhow::Error::msg)?;
    let control = app::fps::FpsControl::new(config.fps);
    config.fps_control = Some(control.clone());
    store.on_change(fps.key, move |v| {
        match app::fps::FpsTarget::parse(&fps.name(*v)) {
            Ok(t) => control.set(t),
            Err(e) => eprintln!("play: frame rate: {e}"),
        }
    });
    // Steam Controllers over raw HID (docs/input/steam-controller.md): with the
    // pads on, no scripted input, and not H2_STEAM_PAD=0. The pads and the
    // controller speaker share the thread.
    let steam = steam_pads(&config, &store);
    config.steam_pads = steam.clone();
    let disc = disc
        .or_else(|| std::env::var_os("H2_DISC").map(PathBuf::from))
        .context(USAGE)?;
    // --icons, then H2_ICONS, then auto: follow the device in use
    // (docs/input/button-icons.md).
    let icons = boot::icons::IconOptions {
        choice: match icons {
            Some(c) => c,
            None => boot::icons::choice_from_env()?.unwrap_or_default(),
        },
        gamepads: config.gamepads,
    };
    if game_variant.is_some() && editor.is_none() {
        bail!("--game-variant needs --editor");
    }
    // The packages' convert items (the Map Pack and Halo 1 discs beside
    // the game disc), before the level is resolved so `--level a30` finds
    // a level this converts.
    crate::package_prepare::before_boot(&disc, &data, no_halo1, no_mappack);
    // The built-in Firefight kits build on a worker while the game boots.
    crate::firefight_kits::start(&disc, &data);
    // Before the build and the window, so a bad name stops here with the
    // disc's list.
    let mut level = match level_choice(
        level_flag,
        difficulty_flag,
        std::env::var("H2_LEVEL").ok(),
        std::env::var("H2_DIFFICULTY").ok(),
    )? {
        Some((name, difficulty)) => Some(boot::level_boot::resolve_in(&disc, Some(&data), &name, difficulty)?),
        None => None,
    };
    if let Some(path) = game_variant {
        let req = level.as_mut().context("--game-variant needs --level")?;
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        req.set_variant(&text)?;
    }
    if let Some(l) = &level {
        eprintln!("play: starting {} {} ({})", l.name, l.game(), l.scenario);
    }
    let xbe = boot::read_xbe(&disc)?;
    let choice = match tier {
        Some(t) => t,
        None => boot::tier::Choice::from_env()?,
    };
    let jobs = std::thread::available_parallelism().map_or(4, |n| n.get());
    let tier_plan = boot::tier::prepare(&xbe, &disc, &out, choice, jobs, true)?;
    let lib = tier_plan.dylib.clone();
    // A start on the fast tier while clang builds runs slowly: say so on
    // screen, and say when the full-speed image is ready.
    let building_clang = tier_plan.background_clang.then(|| out.clone());
    // What the debug menu's Game > Game code page and `game.code` show.
    let game_code = Arc::new(boot::game_code::GameCode {
        tier: tier_plan.tier.name(),
        module: lib.clone(),
        stamp: boot::stamp_path(&lib),
        background_log: building_clang.as_deref().map(boot::tier::background_log),
        full_speed_module: boot::tier::clang_dylib(&out),
        restart: restart_in_place(),
    });
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
        movies,
    };
    // Before the window: the session (and an embedded server) lives until
    // the game is over.
    // With no Live flag, offline (docs/live/live.md "Playing on Live").
    let session = start_session(&mut live, &opts.disc, live_choice)?;
    if let Some(link) = live.join.take() {
        live_hle::join_link::submit(link, live_hle::join_link::Source::CommandLine)
            .map_err(anyhow::Error::msg)?;
    }
    // Signed in: the account's saves are merged into U: now, before the
    // game reads them, and pushed as the game writes
    // (docs/live/live-cloud-saves.md).
    let cloud = session
        .as_ref()
        .and_then(|s| crate::play_cloud::start(s, &opts.data_root, &store));
    let on_vfs = cloud.as_ref().map(crate::play_cloud::Cloud::on_vfs);
    let live_config = session
        .as_ref()
        .map(|s| s.config.clone())
        .unwrap_or_default();
    let status = config.log_status;
    let timeline = config.scripted_input.clone();
    let closer = app::Closer::new();
    config.closer = Some(closer.clone());
    let interrupted = watch_signals(closer);
    crate::boot_cmd::start_memory_watch(&opts.data_root);
    // Under `h2 follow`, a restart exits for the follower to start the
    // next build; on its own, `h2 play` starts itself again.
    let followed = follow_status.is_some();
    // After the signal mask is set, so the reader thread inherits it.
    #[cfg(unix)]
    if let Some(path) = follow_status {
        config.banner = Some(follow_banner(path));
    }
    #[cfg(not(unix))]
    let _ = follow_status;
    let watermark = watermark_on(no_watermark, std::env::var("H2_WATERMARK").ok().as_deref());
    if watermark {
        config.watermark = Some(
            play_watermark()
            .with_full(watermark_full(std::env::var("H2_WATERMARK").ok().as_deref())),
        );
    }
    if config.banner.is_none() && building_clang.is_some() {
        config.banner = Some(tier_banner(Arc::clone(&game_code)));
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
    // Our debug menu (docs/runtime/debug-menu.md): touchpad click + Options,
    // or the backquote key. On unless H2_DEBUG_MENU=0.
    let debug_menu = debug_menu_on(std::env::var("H2_DEBUG_MENU").ok().as_deref())
        .then(debug_menu::Shared::default);
    config.debug_menu = debug_menu.clone();
    // host.screenshot's default directory.
    config.screenshots = Some(opts.data_root.join("screenshots"));
    let perf_overlay = config.watermark.as_ref().map(app::Watermark::switch);
    let cheat_line = config.watermark.as_ref().map(app::Watermark::cheats_handle);
    let readout = config.watermark.as_ref().map(|w| {
        let (full, lines) = w.readout_handles();
        boot::debug_readout::Link { full, lines }
    });
    let editor_link = EditorLink::default();
    config.editor_link = Some(Arc::new(EditorPlateSource(editor_link.clone())));
    if boot::forge_glue::requested().map_err(anyhow::Error::msg)? {
        config.forge = Some(forge::overlay());
    }
    let theater = open_library.map(|open| (boot::theater_ui::Theater::default(), open));
    if let Some((controller, _)) = &theater {
        if let Some(session) = &session {
            controller.configure_sync(&opts.data_root, &session.server_id, session.account.as_ref().map(|a| a.xuid));
        }
        let (overlay, _) = theater::overlay(
            controller.clone(),
            opts.data_root.join("films/screenshots"),
            Some(opts.disc.as_path()),
        );
        config.theater = Some(overlay);
    }
    let report = app::run(config, move |host| {
        let mut host = hosted(
            host,
            audio,
            status,
            timeline,
            Arc::clone(&interrupted),
            icons,
            store,
            steam,
        );
        // With no --film or H2_FILM_OUT, the session records into the film
        // library (docs/game/film-format.md, "Recording every game").
        host.film_auto = film_auto;
        host.film_out = film_out;
        host.level = level;
        host.on_vfs = on_vfs;
        // The menu sees every pad first, and hides them from the game while
        // it is open.
        if let Some(input) = host.input.as_mut() {
            if let Some(menu) = &debug_menu {
                input.set_gate(Box::new(menu.gate()));
            }
            // Editor mode: Start goes to the tool, so the in-game pause menu
            // never opens (docs/editor/game-view.md).
            if editor.is_some() {
                input.add_gate(Box::new(self::editor::NoStart));
            }
        }
        host.theater = theater;
        host.debug_menu = debug_menu;
        // Default to the agent socket where editors and the lab find games
        // (docs/runtime/agent-api.md, "Finding a game").
        host.discoverable_agent = true;
        host.embedded_live = live.local;
        host.perf_overlay = perf_overlay;
        host.cheat_line = cheat_line;
        host.readout = readout;
        host.game_code = Some(game_code);
        host.editor_link = Some(editor_link);
        boot::debug_views::register(&mut host);
        boot::forge_glue::markers_world::register(&mut host);
        boot::superbounce_mode::register(&mut host);
        let r = boot::run_hosted_with_live(&xbe, &lib, &opts, host, &live_config);
        // As `h2 run` does: the kernel's storage may outlive the run (or never
        // drop), so store its pending write metadata at a normal exit, whether
        // the run ended well or not. See docs/runtime/storage.md "Write metadata".
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
    // The last saves go up while an embedded server still runs.
    drop(cloud);
    drop(session);
    #[cfg(unix)]
    if report.restart_requested {
        if followed {
            println!("play: restarting into the update");
            std::process::exit(crate::follow::RESTART_EXIT);
        }
        // F5 on the window's "the game stopped" notice.
        if let Some(restart) = restart_in_place() {
            println!("play: starting the game again");
            restart().map_err(anyhow::Error::msg)?;
        }
    }
    #[cfg(not(unix))]
    let _ = followed;
    Ok(())
}

/// The level to start and its difficulty: `--level`/`--difficulty`, else
/// `H2_LEVEL`/`H2_DIFFICULTY` (each flag wins over its variable; empty
/// variables count as unset). `None` boots to the main menu as before. A
/// difficulty with no level is refused rather than ignored.
fn level_choice(
    level: Option<String>,
    difficulty: Option<String>,
    env_level: Option<String>,
    env_difficulty: Option<String>,
) -> Result<Option<(String, boot::level_boot::Difficulty)>> {
    let set = |v: Option<String>| v.filter(|s| !s.trim().is_empty());
    let from_flag = difficulty.is_some();
    let difficulty = set(difficulty).or(set(env_difficulty));
    let Some(level) = set(level).or(set(env_level)) else {
        if from_flag {
            bail!("--difficulty needs --level (or H2_LEVEL)");
        }
        return Ok(None);
    };
    let difficulty = difficulty
        .as_deref()
        .map(boot::level_boot::Difficulty::parse)
        .transpose()?
        .unwrap_or_default();
    Ok(Some((level.trim().to_string(), difficulty)))
}

/// The Live session to play in, if any: none without a Live flag
/// (nothing is contacted), else the one the flags and `$H2_LIVE_SERVER`
/// name ([`crate::play_live::choose`]). Asked for and not signed in to is
/// an error.
fn start_session(
    live: &mut crate::play_live::LiveOpts,
    disc: &std::path::Path,
    choice: crate::play_live::Choice,
) -> Result<Option<crate::play_live::Session>> {
    use crate::play_live::Choice;
    match choice {
        Choice::Offline { why } => {
            if let Some(why) = why {
                eprintln!("{why}");
            }
            Ok(None)
        }
        Choice::Local => {
            let session = start_live(live, disc)?;
            live.check_join_server(&session.server)?;
            Ok(Some(session))
        }
        Choice::Server { url, from } => {
            let note = match from {
                "--live" => " (the default service; --live-server URL names another)",
                "H2_LIVE_SERVER" => " (H2_LIVE_SERVER)",
                _ => "",
            };
            eprintln!("live: signing in to {url}{note}");
            live.server = Some(url.clone());
            start_live(live, disc).map(Some).with_context(|| {
                format!("signing in to {url}; without a Live flag h2 play plays offline")
            })
        }
    }
}

/// Starts the session `live` names: the embedded server (`--live-local`)
/// or another, and the account, with a line saying where the web app is.
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
    // (H2_LIVE_WEB points at a build of your own and skips this). Only the
    // embedded server serves it.
    if live.local && std::env::var_os("H2_LIVE_WEB").is_none() {
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
        .local
        .then(|| {
            crate::play_live::game_art(disc, &crate::live_cmd::game_art_dir(&places.server_data))
        })
        .flatten();
    let site = live_server::http::Site { web, game_art };
    let session = crate::play_live::start(live, &db, &places, site, &crate::play_live::os_user())?;
    let playing = match (&session.account, &session.waiting) {
        (Some(a), _) => format!("playing as {} ({})", a.gamertag, a.xuid),
        (None, Some(tag)) => format!(
            "{tag} needs a code from Account on {}/dash; the game asks for it when you sign in to Xbox Live,",
            session.server
        ),
        (None, None) => "no account yet,".to_string(),
    };
    eprintln!(
        "live: {playing} on {}{}{}",
        session.server,
        session
            .store()
            .map_or(String::new(), |s| format!(" (embedded, {s} store)")),
        session
            .public_url()
            .map_or(String::new(), |u| format!(", public at {u}"))
    );
    match (session.store(), has_web) {
        (Some(_), true) => {
            eprintln!("live: web app at {}/", session.server);
            if let (Some(link), Some(a)) = (&session.signin, &session.account) {
                eprintln!(
                    "live: sign this browser in as {} (one use, this machine only): {link}",
                    a.gamertag
                );
            }
        }
        (Some(_), false) => eprintln!("live: no web app; build it with `cd web && bun run build`"),
        (None, _) => {}
    }
    for line in &session.tunnel_notices {
        eprintln!("live: {line}");
    }
    Ok(session)
}

/// The watermark's rolling window when `--frame-stats` is off: short, so
/// the numbers follow what is on screen.
const WATERMARK_WINDOW_NS: u64 = 2_000_000_000;

/// `H2_WATERMARK=full` adds the player, level, world and render-picture
/// lines to the watermark (docs/runtime/app.md, "Watermark").
fn watermark_full(env: Option<&str>) -> bool {
    env.is_some_and(|v| v.trim().eq_ignore_ascii_case("full"))
}

/// The watermark is on unless `--no-watermark` or `H2_WATERMARK=0`.
fn watermark_on(no_flag: bool, env: Option<&str>) -> bool {
    !no_flag && env != Some("0")
}

/// Whether the window closes when the guest thread returns, so the process
/// exits by itself: a run with a planned end (`--slices`, or
/// `H2_BOOT_STOP_AT_PRESENT` as boot reads it) and every lab job (nobody
/// is watching its window; the lab waits for the process). Other runs keep
/// the window up with the last frame and the outcome in the title.
fn close_on_guest_exit(slices: u64, stop_at_present: Option<&str>, lab_job: bool) -> bool {
    lab_job || slices != u64::MAX || stop_at_present.is_some_and(|v| v.parse::<u64>().is_ok())
}

/// The debug menu is on unless `H2_DEBUG_MENU=0`.
fn debug_menu_on(env: Option<&str>) -> bool {
    env.is_none_or(|v| v.trim() != "0")
}

/// `h2 play`'s watermark: the renderer, and the map the game has loaded
/// (the file-open record until the first game starts).
fn play_watermark() -> app::Watermark {
    app::Watermark::new()
        .with_note("native renderer")
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

fn mark_frame() -> app::overlay::marks::FrameMarks {
    use app::overlay::marks::{FrameMarks, ImageSamples, MarkCameraSource, ProjectedMarks};
    use gfx::present_camera::{Present, ViewShift};

    let project = |image: Arc<gfx::present_camera::Image>,
                   reference,
                   shifts: &[ViewShift],
                   source: MarkCameraSource| {
        let Ok(samples) = image.payload.clone().downcast::<ImageSamples>() else {
            return Ok(None);
        };
        if !samples.has_visible_sample(&Instant::now) {
            return Ok(None);
        }
        let Some(cameras) = mark_cameras(&image.views, reference, shifts) else {
            let editor_visible = samples
                .editor
                .as_ref()
                .is_some_and(|editor| editor.validity(&Instant::now).is_some());
            return if editor_visible { Err(()) } else { Ok(None) };
        };
        Ok(Some(Arc::new(ProjectedMarks {
            cameras,
            samples,
            source,
        })))
    };

    match gfx::present_camera::current() {
        Some(Present::Guest { image: Some(image), reference }) => {
            match project(image, reference, &[], MarkCameraSource::Tick) {
                Ok(sample) => FrameMarks::Guest(sample),
                Err(()) => FrameMarks::GuestWithoutWorldView,
            }
        }
        Some(Present::Guest { image: None, .. }) => FrameMarks::Guest(None),
        Some(Present::Synthetic {
            source,
            shifts,
            fraction,
            reference,
        }) => FrameMarks::Synthetic(source.and_then(|image| {
            let camera = MarkCameraSource::Made {
                fraction,
                shifted: shifts.iter().any(|s| !s.is_identity()),
            };
            project(image, reference, &shifts, camera).ok().flatten()
        })),
        None => FrameMarks::Raw,
    }
}

/// Each view's mark camera, moved by its shift where a made frame drew it
/// with another camera than the image's own (`gfx::present_camera`).
fn mark_cameras(
    views: &[Option<gfx::present_camera::View>; 4],
    reference: bool,
    shifts: &[gfx::present_camera::ViewShift],
) -> Option<app::overlay::marks::MarkCameras> {
    let count = views.iter().flatten().count();
    let cameras = views.map(|slot| {
        let view = slot?;
        let mut camera = mark_camera(view, reference)?;
        if let Some(shift) =
            gfx::present_camera::ViewShift::find(shifts, camera.viewport, count)
        {
            camera.view_projection = shift.apply(camera.view_projection);
        }
        Some(camera)
    });
    cameras.iter().any(Option::is_some).then_some(cameras)
}

fn mark_camera(
    view: gfx::present_camera::View,
    reference: bool,
) -> Option<app::overlay::marks::MarkCamera> {
    let (width, height) = boot::setting_defs::GUEST_FRAME;
    let [top, left, bottom, right] = view.camera.viewport.map(f32::from);
    if !view.camera.is_usable()
        || top < 0.0
        || left < 0.0
        || bottom > height as f32
        || right > width as f32
    {
        return None;
    }
    let horizontal_factor = if reference { 1.0 } else { view.horizontal_factor };
    let mut view_projection = view.camera.view_projection();
    for column in &mut view_projection {
        column[0] *= horizontal_factor;
    }
    Some(app::overlay::marks::MarkCamera {
        view_projection,
        viewport: [
            top / height as f32,
            left / width as f32,
            bottom / height as f32,
            right / width as f32,
        ],
    })
}

#[derive(Debug)]
struct EditorPlateSource(EditorLink);

impl app::EditorPlateSource for EditorPlateSource {
    fn current(&self, now: Instant) -> Option<(app::EditorPlate, Instant)> {
        let badge = self.0.current(now)?;
        let (title, tone) = match badge.mode {
            Mode::View => ("Editor · viewing", app::NoticeKind::Info),
            Mode::Edit => ("Editor · editing live", app::NoticeKind::Ready),
            Mode::Blocked => ("Editor · edits blocked", app::NoticeKind::Error),
        };
        let plate = app::EditorPlate {
            title,
            text: badge.text.clone(),
            tone,
        };
        Some((plate, badge.expires))
    }

    fn set_waker(&self, wake: Arc<dyn Fn() + Send + Sync>) {
        self.0.set_waker(wake);
    }

    fn take_focus_request(&self) -> bool {
        self.0.take_focus_request()
    }

    fn set_mark_frame_status(&self, status: app::overlay::marks::MarkFrameStatus) {
        use app::overlay::marks::MarkFrameStatus as Status;
        let availability = match status {
            Status::Waiting => MarkAvailability::Waiting,
            Status::Ready(generation) => return self.0.set_drawn_mark_generation(generation),
            Status::NoWorldView => MarkAvailability::NoWorldView,
            Status::Raw => MarkAvailability::RawFrame,
            Status::Loading => MarkAvailability::Loading,
            Status::Synthetic => MarkAvailability::Synthetic,
        };
        self.0.set_mark_frame_status(availability);
    }
}

/// The window's banner while clang builds the full-speed image in the
/// background (`boot::tier::background_build`): this start runs on the
/// slower fast tier until the next one. The text is
/// `boot::game_code::banner_text`, the state the debug menu's Game code
/// page shows.
fn tier_banner(code: Arc<boot::game_code::GameCode>) -> app::Banner {
    use boot::game_code::{BuildState, banner_text};
    let banner = app::Banner::new();
    let b = banner.clone();
    let notice = |state: BuildState, elapsed: Duration| {
        banner_text(state, elapsed).map(|text| app::Notice {
            kind: if state == BuildState::Failed { app::NoticeKind::Error } else { app::NoticeKind::Info },
            text,
        })
    };
    b.set(notice(BuildState::Building, Duration::ZERO));
    std::thread::Builder::new()
        .name("tier-status".into())
        .spawn(move || {
            loop {
                std::thread::sleep(Duration::from_secs(2));
                let (state, elapsed, _) = code.build();
                b.set(notice(state, elapsed.unwrap_or_default()));
                if state != BuildState::Building {
                    return;
                }
            }
        })
        .expect("spawning the tier status reader");
    banner
}

/// Starts `h2 play` again in this process (the same executable and
/// arguments), so the new start picks clang's module when it is current.
/// Files Rust opens close on exec; the window and the GPU go with the old
/// image.
#[cfg(unix)]
fn restart_in_place() -> Option<boot::game_code::Restart> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe().ok()?;
    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    Some(Arc::new(move || {
        let mut c = std::process::Command::new(&exe);
        c.args(&args);
        // SAFETY: only async-signal-safe libc calls on a local set. The
        // signal mask survives exec, and `watch_signals` blocked SIGINT and
        // SIGTERM; the new start must begin with them unblocked.
        unsafe {
            c.pre_exec(|| {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGINT);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
                Ok(())
            });
        }
        // The agent socket is named for this pid, which exec keeps: free
        // the path so the new start can listen on it again.
        for sock in [boot::agent_glue::socket_from_env(), Some(agent_proto::discover::process_socket())]
            .into_iter()
            .flatten()
        {
            let _ = std::fs::remove_file(sock);
        }
        let err = c.exec();
        Err(format!("exec {}: {err}", exe.display()))
    }))
}

#[cfg(not(unix))]
fn restart_in_place() -> Option<boot::game_code::Restart> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_play_session_records_by_default_launch_join_included_but_not_a_lab_job() {
        use boot::theater::autorecord::Request;
        assert_eq!(film_auto_request(false), Request::On);
        assert!(matches!(film_auto_request(true), Request::Skip(why) if why.contains("lab")));
    }

    #[test]
    fn editor_marks_reject_world_cameras_outside_the_guest_image() {
        let mut view = test_mark_view();
        view.camera.viewport[1] = -1;
        assert!(view.camera.is_usable());
        assert!(mark_cameras(&[Some(view), None, None, None], false, &[]).is_none());
        assert!(
            mark_cameras(&[Some(view), Some(test_mark_view()), None, None], false, &[]).is_some()
        );
    }

    #[test]
    fn made_frames_move_the_mark_camera_by_their_shift() {
        use gfx::present_camera::ViewShift;
        let view = test_mark_view();
        let plain = mark_cameras(&[Some(view), None, None, None], false, &[]).unwrap()[0].unwrap();
        // The frame drew the view one unit further along +y.
        let mut world = [[0.0; 4]; 4];
        for (i, row) in world.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        world[1][3] = 1.0;
        let shift = ViewShift {
            viewport: [0.0, 0.0, 1.0, 1.0],
            world,
        };
        let moved =
            mark_cameras(&[Some(view), None, None, None], false, &[shift]).unwrap()[0].unwrap();
        assert_eq!(moved.view_projection, shift.apply(plain.view_projection));
        assert_ne!(moved.view_projection, plain.view_projection);
        assert_eq!(moved.viewport, plain.viewport);
    }

    fn test_mark_view() -> gfx::present_camera::View {
        gfx::present_camera::View {
            camera: gfx::present_camera::Camera {
                position: [0.0; 3],
                forward: [1.0, 0.0, 0.0],
                up: [0.0, 0.0, 1.0],
                fov: 1.0,
                projection_xy: None,
                viewport: [0, 0, 100, 100],
                near: 1.0,
                far: 20.0,
            },
            horizontal_factor: 1.0,
        }
    }

    #[test]
    fn editor_plate_shows_the_mode_and_optional_text() {
        use agent_proto::editor::LinkParams;
        use app::EditorPlateSource as _;

        let source = EditorPlateSource(EditorLink::default());
        let now = Instant::now();
        assert!(source.current(now).is_none());
        for (mode, title, tone) in [
            (Mode::View, "Editor · viewing", app::NoticeKind::Info),
            (Mode::Edit, "Editor · editing live", app::NoticeKind::Ready),
            (Mode::Blocked, "Editor · edits blocked", app::NoticeKind::Error),
        ] {
            source
                .0
                .set(
                    LinkParams {
                        editor: Some("sample".into()),
                        mode: Some(mode),
                        text: Some("Aim select".into()),
                        ttl_ms: Some(500),
                        ..Default::default()
                    },
                    now,
                )
                .unwrap();
            let (plate, expires) = source.current(now).unwrap();
            assert_eq!(plate.lines().collect::<Vec<_>>(), [title, "Aim select"]);
            assert_eq!(plate.tone, tone);
            assert_eq!(expires, now + Duration::from_millis(500));
        }
        source
            .0
            .set(
                LinkParams {
                    editor: Some("sample".into()),
                    mode: Some(Mode::View),
                    ..Default::default()
                },
                now,
            )
            .unwrap();
        let (plate, _) = source.current(now).unwrap();
        assert_eq!(plate.lines().collect::<Vec<_>>(), ["Editor · viewing"]);
    }

    /// `h2 play`, `h2 run` and the editor's variant store share one default
    /// data root; the variants crate checks its side against the same path.
    #[test]
    fn the_default_data_root_is_shared() {
        assert_eq!(
            variants_save_root(),
            content::store::default_data_root().join("disk/Partition1/UDATA/4D530064")
        );
    }

    /// The variant store's root as the guest sees it: `U:` under the data
    /// root, which the VFS maps to `disk/Partition1/UDATA/<title>`.
    fn variants_save_root() -> std::path::PathBuf {
        vfs::Config::new(content::store::default_data_root(), 0x4D53_0064)
            .title_data_dir()
            .parent()
            .unwrap()
            .parent()
            .unwrap()
            .join("UDATA")
            .join("4D530064")
    }

    fn some(s: &str) -> Option<String> {
        Some(s.to_string())
    }

    #[test]
    fn level_flags_win_over_the_variables() {
        use boot::level_boot::Difficulty;
        assert_eq!(
            level_choice(None, None, None, None).unwrap(),
            None,
            "the main menu"
        );
        assert_eq!(
            level_choice(some("03a_oldmombasa"), None, None, None).unwrap(),
            Some(("03a_oldmombasa".into(), Difficulty::Normal)),
            "normal by default"
        );
        assert_eq!(
            level_choice(
                some("03a_oldmombasa"),
                some("legendary"),
                some("01a_tutorial"),
                some("easy")
            )
            .unwrap(),
            Some(("03a_oldmombasa".into(), Difficulty::Legendary))
        );
        assert_eq!(
            level_choice(None, None, some("01a_tutorial"), some("heroic")).unwrap(),
            Some(("01a_tutorial".into(), Difficulty::Heroic))
        );
        // An empty variable is unset; the flag's difficulty applies to the
        // variable's level.
        assert_eq!(
            level_choice(None, None, some(""), some("heroic")).unwrap(),
            None
        );
        assert_eq!(
            level_choice(None, some("easy"), some("01a_tutorial"), None).unwrap(),
            Some(("01a_tutorial".into(), Difficulty::Easy))
        );
    }

    #[test]
    fn level_flags_refuse_bad_input() {
        let e = level_choice(None, some("heroic"), None, None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("--difficulty needs --level"), "{e}");
        let e = level_choice(some("03a_oldmombasa"), some("hard"), None, None)
            .unwrap_err()
            .to_string();
        assert!(e.contains("unknown difficulty"), "{e}");
    }

    #[test]
    fn play_parses_the_level_flags() {
        // Each stops before the disc is read: a missing value, and a
        // difficulty with no level.
        let run_err = |a: &[&str]| {
            run(a.iter().map(|s| s.to_string()).collect())
                .unwrap_err()
                .to_string()
        };
        assert!(run_err(&["/nonexistent/halo2.iso", "--level"]).contains("--level needs a value"));
        assert!(
            run_err(&["/nonexistent/halo2.iso", "--difficulty"])
                .contains("--difficulty needs a value")
        );
        if std::env::var_os("H2_LEVEL").is_none() {
            let e = run_err(&["/nonexistent/halo2.iso", "--difficulty", "heroic"]);
            assert!(e.contains("--difficulty needs --level"), "{e}");
            let e = run_err(&[
                "/nonexistent/halo2.iso",
                "--level",
                "03a_oldmombasa",
                "--difficulty",
                "hard",
            ]);
            assert!(e.contains("unknown difficulty"), "{e}");
        }
    }

    #[test]
    fn play_parses_the_editor_flags() {
        let run_err = |a: &[&str]| {
            run(a.iter().map(|s| s.to_string()).collect())
                .unwrap_err()
                .to_string()
        };
        // Each stops before the disc is read.
        if std::env::var_os("H2_LEVEL").is_none() && std::env::var_os("H2_FILM_OUT").is_none() {
            let e = run_err(&["/nonexistent/halo2.iso", "--editor"]);
            assert!(e.contains("--editor needs --level"), "{e}");
            let e = run_err(&["/nonexistent/halo2.iso", "--window", "--level", "a10"]);
            assert!(e.contains("--window is for --editor"), "{e}");
            let e = run_err(&["/nonexistent/halo2.iso", "--editor", "--level", "a10", "--live-local"]);
            assert!(e.contains("local game"), "{e}");
            let e = run_err(&["/nonexistent/halo2.iso", "--editor", "--level", "a10", "--film", "x.h2film"]);
            assert!(e.contains("no film"), "{e}");
        }
    }

    #[test]
    fn the_debug_menu_is_on_unless_turned_off() {
        assert!(debug_menu_on(None));
        assert!(debug_menu_on(Some("1")));
        assert!(!debug_menu_on(Some("0")));
    }

    #[test]
    fn runs_with_a_planned_end_and_lab_jobs_close_when_the_guest_returns() {
        // An open-ended run keeps its window up with the outcome.
        assert!(!close_on_guest_exit(u64::MAX, None, false));
        // boot ignores a value it can't parse, so the window stays too.
        assert!(!close_on_guest_exit(u64::MAX, Some("lots"), false));
        // `h2 lab run --frames N` sets H2_BOOT_STOP_AT_PRESENT=N.
        assert!(close_on_guest_exit(u64::MAX, Some("2700"), false));
        assert!(close_on_guest_exit(5000, None, false));
        // A lab job without a limit (a run that crashes) still exits.
        assert!(close_on_guest_exit(u64::MAX, None, true));
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
    fn only_full_adds_the_readout_to_the_watermark() {
        assert!(!watermark_full(None));
        assert!(!watermark_full(Some("1")));
        assert!(watermark_full(Some("full")));
        assert!(watermark_full(Some(" FULL ")));
        // `full` is still on, not the off value.
        assert!(watermark_on(false, Some("full")));
    }

    #[test]
    fn the_full_level_puts_the_games_lines_above_the_live_numbers() {
        let wm = play_watermark();
        let (full, lines) = wm.readout_handles();
        *lines.lock().unwrap() = vec!["pos 1.00 2.00 3.00".into()];
        // Off, whatever the game left is not shown.
        assert!(!wm.panels(None, "metal").live.iter().any(|l| l.starts_with("pos")));
        full.store(true, std::sync::atomic::Ordering::Relaxed);
        let live = wm.panels(None, "metal").live;
        assert_eq!(live[0], "pos 1.00 2.00 3.00");
        assert_eq!(live.last().unwrap(), "metal, native renderer");
    }

    #[test]
    fn the_play_watermark_names_the_renderer() {
        let wm = play_watermark();
        assert!(wm.visible());
        let p = wm.panels(None, "metal");
        assert_eq!(p.label[0], "h2recomp development build");
        assert_eq!(p.live.last().unwrap(), "metal, native renderer");
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
            phase_ms: [38.1, 3.2, 8.4, 2.0, 5.5, 0.0, 6.4, 0.0, 2.0],
            count_mean: [0.0; timeline::phase::COUNTERS],
        };
        let n = stats_notice(Some(s.clone())).unwrap();
        assert_eq!(n.kind, app::NoticeKind::Info);
        assert_eq!(
            n.text,
            "14.8 fps p50 61 p99 998 max 1402 ms | cpu 40 dec 3 enc 8 sub 2 gpu 6 pace 0 rt 6"
        );
        // The banner cuts at 96 characters.
        assert!(n.text.len() < 96);
        // With render-timing on, the render front-end and the D3D entries
        // follow cpu (otherwise cpu includes d3d).
        let mut s = s;
        s.phase_ms[timeline::phase::Phase::Render as usize] = 15.2;
        let n = stats_notice(Some(s)).unwrap();
        assert_eq!(
            n.text,
            "14.8 fps p50 61 p99 998 max 1402 ms | cpu 38 ren 15 d3d 2 dec 3 enc 8 sub 2 gpu 6 pace 0 rt 6"
        );
        assert!(n.text.len() < 96);
    }

    #[test]
    fn a_stale_or_missing_dylib_is_rebuilt() {
        let dir = std::env::temp_dir().join(format!("h2-play-stamp-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lib = dir.join(boot::DYLIB);
        let xbe = b"XBEH synthetic image bytes";
        // What play's `--tier clang` builds: clang's module, when stale.
        let needs_build = |xbe: &[u8], lib: &std::path::Path| {
            let p = boot::tier::plan(xbe, lib.parent().unwrap(), boot::tier::Choice::Clang);
            assert_eq!(p.dylib, lib);
            p.build
        };
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
