//! The shell and the C interface (`app::embedded` exists on Apple and Windows
//! only; the crate's other modules build everywhere).

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::{Layout, overlay, text, touch};

/// Kernel calls kept in memory for the report, as `h2 play` keeps.
const TRACE_LIMIT: usize = 4096;

/// What [`h2host_start`] returns: the running shell. Opaque to the host.
pub struct H2Host {
    shell: Option<app::embedded::Embedded>,
    message: CString,
    text: text::Edges,
    /// The settings the game runs with (touch tuning reads them live).
    store: Option<Arc<settings::Store>>,
    /// What the touch pad last logged (`input::touch::ChangeLog`).
    touch_log: input::touch::ChangeLog,
}

/// The environment the game reads, pointed at the app's own places. Only
/// variables the user has not set (from the Xcode scheme) are set.
fn environment(layout: &Layout) -> Vec<(&'static str, PathBuf)> {
    vec![("H2_BOOT_CACHE", layout.boot_cache.clone())]
}

/// The game, as `h2 play` runs it, on the shell's guest thread.
fn guest(layout: Layout, store: Arc<settings::Store>) -> app::worker::GuestFn {
    Box::new(move |mut host: app::GuestHost| -> Result<()> {
        let xbe = boot::read_xbe(&layout.disc)?;
        let view = boot::run_opts::DisplayView {
            native_target: Arc::new(host.native_target()),
            viewer_output: None,
            viewer_lod: None,
            display_size: Arc::new(host.display_size()),
        };
        let gfx = host.gfx_context();
        let presenter = host.presenter(|| app::overlay::marks::FrameMarks::Raw);
        let input = host.take_input();
        // The player-one pad's handle: no key arrives, but it records the
        // device last used, which picks the button icons (the remote's, or
        // a pad's), and carries the system keyboard's text.
        let keyboard = host.keyboard().cloned();
        // Live: on against the public service unless live.toml or
        // H2_LIVE_SERVER says otherwise; signed in, cloud saves too.
        let (live, lines) = live_hle::platform::config(
            &layout.data,
            std::env::var("H2_LIVE_SERVER").ok().as_deref(),
        );
        for line in lines {
            eprintln!("h2host: live: {line}");
        }
        let cloud = cloud_saves(&live, &layout.data, &store);
        let hosted = boot::Host {
            gfx: Some(gfx),
            presenter: Some(presenter),
            gfx_view: Some(view),
            host_light: false,
            host_bots: false,
            audio_sink: Some(audio_sink()),
            // Voice chat's devices are macOS-only for now (live-voice.md).
            voice_io: None,
            pad_speaker: None,
            debug_menu: None,
            perf_overlay: None,
            cheat_line: None,
            readout: None,
            game_code: None,
            discoverable_agent: false,
            settings: Some(store),
            trace_limit: Some(TRACE_LIMIT),
            level: None,
            input,
            keyboard,
            icons: Some(boot::icons::IconOptions {
                choice: Default::default(),
                gamepads: true,
            }),
            return_to_menu: Some(host.loading_screen()),
            between_slices: Some(Box::new(move |k: &kernel::Kernel| {
                host.tick(k.clock().counter()) == app::Control::Continue
            })),
            on_vfs: cloud.as_ref().map(|c| c.on_vfs()),
            ..boot::Host::default()
        };
        let opts = boot::RunOpts {
            disc: layout.disc.clone(),
            data_root: layout.data.clone(),
            max_slices: u64::MAX,
            transfers_per_slice: 50_000_000,
            stopgaps: true,
            trace: None,
            d3d: true,
            bink: true,
            dsound: true,
            xapi: true,
            // What the app's installer put in the app data directory
            // (movies::install); skipped where there is no decoder.
            movies: boot::movies_glue::Movies::Auto,
        };
        // Live presence says which build this is (docs/game/game-options.md 3).
        boot::live_glue::set_build(app::BuildInfo::baked().sha);
        let r = boot::run_hosted_with_live(&xbe, &layout.module, &opts, hosted, &live);
        vfs::flush_pending_metadata();
        // Pushes what is still waiting.
        drop(cloud);
        let r = r?;
        for l in &r.kernel_log {
            eprintln!("DbgPrint: {}", l.trim_end());
        }
        eprintln!(
            "outcome: {}",
            r.outcome.chars().take(400).collect::<String>()
        );
        eprintln!("stop: {}", r.stop);
        eprintln!("last eip: {:#010x}", r.eip);
        Ok(())
    })
}

/// Cloud saves for the account signed in to `live`'s server: its saves
/// merged into `data` now, before the game reads them, and pushed as it
/// writes. `None`, with a line, when Live is off, no account is set up yet,
/// `H2_CLOUD_SAVES=0`, or the server is not reached: the game plays with
/// the saves on this device.
fn cloud_saves(
    live: &boot::LiveConfig,
    data: &Path,
    store: &Arc<settings::Store>,
) -> Option<cloud_saves::session::Cloud> {
    let say = |line: &str| eprintln!("h2host: cloud saves: {line}");
    let entry = live_hle::platform::account(live)?;
    if !cloud_saves::session::enabled(std::env::var("H2_CLOUD_SAVES").ok().as_deref()) {
        say("off (H2_CLOUD_SAVES); saves stay on this device");
        return None;
    }
    let (api, server_id) = match live_hle::account_setup::discover(&entry.server) {
        Ok(found) => found,
        Err(e) => {
            say(&format!(
                "{} not reached ({e}); saves stay on this device",
                entry.server
            ));
            return None;
        }
    };
    if server_id != entry.server_id {
        say(&format!(
            "{} is another server now; saves stay on this device",
            entry.server
        ));
        return None;
    }
    let key = live_proto::sign::KeyPair::from_private_jwk(&entry.key.private_jwk()).ok()?;
    let settings: Arc<dyn cloud_saves::local::ProfileSettings> = store.clone();
    cloud_saves::session::start(
        cloud_saves::session::SignedIn {
            server: entry.server,
            api,
            server_id,
            xuid: entry.xuid,
            gamertag: entry.gamertag,
            key,
        },
        data,
        Some(settings),
        say,
    )
}

/// The default output device, or a sink nobody plays if it will not open
/// (the game then runs silent, on the same clock).
fn audio_sink() -> boot::dsound_glue::SinkOpener {
    Box::new(|| -> Box<dyn audio::AudioSink> {
        match audio::output::DeviceSink::open_default() {
            Ok(d) => {
                eprintln!(
                    "h2host: audio on {} at {} Hz",
                    d.device_id(),
                    audio::AudioSink::sample_rate(&d)
                );
                Box::new(d)
            }
            Err(e) => {
                eprintln!("h2host: no audio device ({e}); audio is discarded");
                Box::new(audio::NullSink::new(48_000, 48_000 / 4))
            }
        }
    })
}

fn start(
    layer: *mut c_void,
    size: (u32, u32),
    bundle: &Path,
    caches: &Path,
    documents: Option<&Path>,
) -> Result<(app::embedded::Embedded, Arc<settings::Store>)> {
    // SAFETY: the host passes a live CAMetalLayer it keeps for the app's
    // lifetime, on the main thread (h2host.h).
    unsafe { start_in(layer, size, Layout::new(bundle, caches, documents)) }
}

/// Checks and prepares `layout`, then starts the game (`boot::run_hosted`,
/// as `h2 play` runs it) into `layer` inside [`app::embedded::Embedded`],
/// returning the shell and the settings it runs with. The Apple hosts reach
/// it through [`h2host_start`]; the Xbox app (crates/apps/uwp-host) calls it
/// with [`Layout::for_package`].
///
/// # Safety
/// `layer` is a live host surface ([`app::embedded::host_surface`]) kept
/// alive while the returned shell is; main (UI) thread only.
pub unsafe fn start_in(
    layer: *mut c_void,
    size: (u32, u32),
    layout: Layout,
) -> Result<(app::embedded::Embedded, Arc<settings::Store>)> {
    let missing = layout.missing();
    if !missing.is_empty() {
        anyhow::bail!("the app bundle is incomplete: {}", missing.join("; "));
    }
    let seeded = layout
        .prepare()
        .context("preparing the app's directories")?;
    eprintln!(
        "h2host: disc {}, data {}, {seeded} boot cache file(s) seeded",
        layout.disc.display(),
        layout.data.display()
    );
    for (k, v) in environment(&layout) {
        if std::env::var_os(k).is_none() {
            // SAFETY: called on the main thread before the guest, render and
            // audio threads exist; nothing reads the environment concurrently.
            unsafe { std::env::set_var(k, v) };
        }
    }
    check_bundle(&layout)?;
    let (store, problems) = boot::setting_defs::load(&layout.data, &[]);
    for p in problems {
        eprintln!("h2host: {p}");
    }
    let store = Arc::new(store);
    let config = app::embedded::device_config();
    // SAFETY: the caller's contract.
    let shell = unsafe {
        app::embedded::Embedded::start(config, layer, size, guest(layout, Arc::clone(&store)))
    }?;
    Ok((shell, store))
}

/// Refuses a module built from other sources before anything slow starts.
///
/// The module's stamp and the boot cache's analysis are keyed by the
/// generator and analyzer sources, `Cargo.lock` and the compiler. A device
/// cannot rebuild either as `h2 run` does on a Mac, and the game would
/// first spend minutes redoing the analysis with nothing on screen, then
/// refuse the module anyway. So check the stamp here, and say so when the
/// analysis has to be redone. Call after `H2_BOOT_CACHE` is set.
fn check_bundle(layout: &Layout) -> Result<()> {
    // The stamp names the registered entries.
    boot::register_image_entries();
    let xbe = boot::read_xbe(&layout.disc)?;
    if !boot::dylib_current(&xbe, &layout.module) {
        anyhow::bail!(
            "the game module in this app was built from other sources than the app \
             (its stamp differs). Rebuild the app's inputs with build.sh, without \
             --no-module, after any change to the generator, the runtime or Cargo.lock."
        );
    }
    match boot::analysis_cache(&xbe) {
        Some(p) if !p.is_file() => eprintln!(
            "h2host: no bundled analysis matches this build ({}); analysing the XBE on the \
             device first, which takes minutes. build.sh bundles a matching one.",
            p.display()
        ),
        _ => {}
    }
    Ok(())
}

fn path(p: *const c_char) -> Option<PathBuf> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the host passes NUL-terminated UTF-8 strings (h2host.h).
    let s = unsafe { CStr::from_ptr(p) }.to_str().ok()?;
    Some(PathBuf::from(s))
}

/// Starts the game into `layer` (a `CAMetalLayer*`, `width` x `height`
/// drawable pixels). Never null: on failure the handle carries no shell,
/// [`h2host_status`] says 2 and [`h2host_message`] why. `documents` may be
/// null (tvOS): saves then live in Caches.
///
/// # Safety
/// `layer` is a live `CAMetalLayer*` kept alive while the handle is; the
/// paths are NUL-terminated (or `documents` null); main thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_start(
    layer: *mut c_void,
    width: u32,
    height: u32,
    bundle: *const c_char,
    caches: *const c_char,
    documents: *const c_char,
) -> *mut H2Host {
    let docs = path(documents);
    let r = match (path(bundle), path(caches)) {
        (Some(b), Some(c)) if documents.is_null() || docs.is_some() => {
            std::panic::catch_unwind(|| start(layer, (width, height), &b, &c, docs.as_deref()))
                .unwrap_or_else(|_| Err(anyhow::anyhow!("panicked while starting")))
        }
        _ => Err(anyhow::anyhow!("bad bundle, Caches or Documents path")),
    };
    let host = match r {
        Ok((shell, store)) => H2Host {
            shell: Some(shell),
            message: CString::default(),
            text: Default::default(),
            store: Some(store),
            touch_log: Default::default(),
        },
        Err(e) => {
            eprintln!("h2host: {e:#}");
            H2Host {
                shell: None,
                message: message(&format!("{e:#}")),
                text: Default::default(),
                store: None,
                touch_log: Default::default(),
            }
        }
    };
    Box::into_raw(Box::new(host))
}

fn message(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

/// # Safety
/// `host` is from [`h2host_start`]; main thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_redraw(host: *mut H2Host) {
    // SAFETY: the caller's contract.
    if let Some(s) = unsafe { host.as_mut() }.and_then(|t| t.shell.as_mut()) {
        s.redraw();
    }
}

/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_resize(host: *mut H2Host, width: u32, height: u32) {
    // SAFETY: the caller's contract.
    if let Some(s) = unsafe { host.as_mut() }.and_then(|t| t.shell.as_mut()) {
        s.resize((width, height));
    }
}

/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_set_paused(host: *mut H2Host, paused: bool) {
    // SAFETY: the caller's contract.
    if let Some(s) = unsafe { host.as_ref() }.and_then(|t| t.shell.as_ref()) {
        s.set_paused(paused);
    }
}

/// 0 while the game runs, 1 once it has returned, 2 if it failed or never
/// started ([`h2host_message`] says why).
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_status(host: *mut H2Host) -> i32 {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { host.as_mut() }) else {
        return 2;
    };
    let Some(s) = t.shell.as_mut() else { return 2 };
    let Some((code, text)) = s.guest_exit().map(stopped) else {
        return 0;
    };
    if t.message.is_empty() {
        // The console's only word on it: the app shows it on screen, and a
        // boot that failed before the menu otherwise looks like a hang.
        eprintln!("h2host: {text}");
        t.message = message(&text);
    }
    code
}

/// [`h2host_status`]'s code and text for a guest that has ended; the
/// Xbox app logs the same text.
pub fn stopped(e: &app::GuestExit) -> (i32, String) {
    match e {
        app::GuestExit::Returned => (1, "The game ended.".to_owned()),
        app::GuestExit::Failed(why) => (2, format!("the game failed: {why}")),
        app::GuestExit::Panicked(why) => (2, format!("the game panicked: {why}")),
        e => (2, format!("{e:?}")),
    }
}

/// Why the game is not running, or empty. Valid until the next call on `host`.
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_message(host: *mut H2Host) -> *const c_char {
    // SAFETY: the caller's contract.
    match unsafe { host.as_ref() } {
        Some(t) => t.message.as_ptr(),
        None => c"no handle".as_ptr(),
    }
}

/// What changed about the game's text field since the last call: 1 a field
/// opened (show the system keyboard), -1 it closed (hide it), 0 nothing.
/// Call once per display frame.
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_text_edge(host: *mut H2Host) -> i32 {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { host.as_mut() }) else {
        return 0;
    };
    let active = t
        .shell
        .as_ref()
        .and_then(|s| s.keyboard())
        .is_some_and(|k| k.text_active());
    t.text.update(active).code()
}

fn with_keyboard(
    host: *mut H2Host,
    f: impl FnOnce(&input::backend::keyboard::KeyboardMouse) -> bool,
) -> bool {
    // SAFETY: the callers' contract (h2host.h): a handle from h2host_start
    // or null, on the main thread.
    unsafe { host.as_ref() }
        .and_then(|t| t.shell.as_ref())
        .and_then(|s| s.keyboard())
        .is_some_and(f)
}

/// Committed text for the open field (`UIKeyInput.insertText`, UTF-8; a
/// `"\n"` in it is Done). False when no field is open.
///
/// # Safety
/// As [`h2host_redraw`]; `text` is NUL-terminated.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_text_insert(host: *mut H2Host, text: *const c_char) -> bool {
    if text.is_null() {
        return false;
    }
    // SAFETY: NUL-terminated, the caller's contract.
    let s = unsafe { CStr::from_ptr(text) }.to_string_lossy();
    with_keyboard(host, |k| text::apply(k, text::insert_actions(&s)))
}

/// Backspace in the open field (`UIKeyInput.deleteBackward`).
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_text_delete_backward(host: *mut H2Host) -> bool {
    use input::backend::keyboard::TextAction;
    with_keyboard(host, |k| text::apply(k, vec![TextAction::Backspace]))
}

/// The field's Done: the game checks and submits it as its own Done key does.
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_text_done(host: *mut H2Host) -> bool {
    use input::backend::keyboard::TextAction;
    with_keyboard(host, |k| text::apply(k, vec![TextAction::Accept]))
}

/// The field's Cancel: the original text comes back and the screen backs
/// out, as the game's own Back does. Hiding the system keyboard is not
/// this; only an explicit Back is.
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_text_cancel(host: *mut H2Host) -> bool {
    use input::backend::keyboard::TextAction;
    with_keyboard(host, |k| text::apply(k, vec![TextAction::Cancel]))
}

/// The host's own Start (0) and Back (1) buttons, held while `down`: iOS's
/// on-screen controls have neither. They press the player-one pad's bound
/// key, so they act on port 0 with every other device. False for another
/// number, or with no shell.
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_button(host: *mut H2Host, button: i32, down: bool) -> bool {
    use input::bindings::Control;
    let control = match button {
        0 => Control::Start,
        1 => Control::Back,
        _ => return false,
    };
    with_keyboard(host, |k| k.press_control(control, down))
}

/// One report from the on-screen touch controls (`H2TouchFrame`, h2host.h):
/// held buttons, the move thumb's offset and the look drag since the last
/// call. Call on every touch change and once per display frame. The touch
/// tuning follows the settings. False with no shell or a null frame.
///
/// # Safety
/// As [`h2host_redraw`]; `frame` is null or points at a valid
/// `H2TouchFrame`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_touch_update(
    host: *mut H2Host,
    frame: *const touch::H2TouchFrame,
) -> bool {
    // SAFETY: the caller's contract.
    let (Some(t), Some(f)) = (unsafe { host.as_mut() }, unsafe { frame.as_ref() }) else {
        return false;
    };
    let Some(k) = t.shell.as_ref().and_then(|s| s.keyboard()) else {
        return false;
    };
    let (plan, _) = touch::current_plan();
    let base = match &t.store {
        Some(store) => touch::tuning(store, k.touch_tuning()),
        None => k.touch_tuning(),
    };
    let tuning = touch::live(base, &plan, input::touch_look::consumer());
    if tuning != k.touch_tuning() {
        k.set_touch_tuning(tuning);
    }
    k.set_touch_context(input::touch_layout::context());
    let state = k.touch_update(&f.frame());
    if let Some(line) = t.touch_log.line(&state, std::time::Instant::now()) {
        eprintln!("h2host: touch {line}");
    }
    true
}

/// The on-screen touch controls showed (`true`) or hid. Hiding lets go of
/// everything they held, and a toggled crouch stands up. Features that
/// change with touch (the aim assist's Auto level) read this.
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_touch_shown(host: *mut H2Host, shown: bool) {
    input::touch::set_controls_shown(shown);
    eprintln!(
        "h2host: touch controls {}",
        if shown { "shown" } else { "hidden" }
    );
    // SAFETY: the caller's contract.
    if let Some(t) = unsafe { host.as_mut() } {
        if let Some(k) = t.shell.as_ref().and_then(|s| s.keyboard()) {
            k.touch_release();
        }
        t.touch_log = Default::default();
    }
}

/// Whether the touch controls hold crouch (a toggled crouch included), so
/// the host can light its crouch button.
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_touch_crouched(host: *mut H2Host) -> bool {
    // SAFETY: the caller's contract.
    unsafe { host.as_ref() }
        .and_then(|t| t.shell.as_ref())
        .and_then(|s| s.keyboard())
        .is_some_and(|k| k.touch_crouched())
}

/// What the on-screen controls should draw now (`H2TouchLayout`): the
/// layout for player one's state, which the boot publishes each tick
/// (`input::touch_layout`). False with no shell or a null `out`.
///
/// # Safety
/// As [`h2host_redraw`]; `out` is null or points at a writable
/// `H2TouchLayout`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_touch_layout(
    host: *mut H2Host,
    out: *mut touch::H2TouchLayout,
) -> bool {
    // SAFETY: the caller's contract.
    let (Some(t), Some(out)) = (unsafe { host.as_ref() }, unsafe { out.as_mut() }) else {
        return false;
    };
    if t.shell.as_ref().and_then(|s| s.keyboard()).is_none() {
        return false;
    }
    let (plan, generation) = touch::current_plan();
    *out = touch::H2TouchLayout::from_plan(&plan, generation);
    true
}

/// Gyro aiming's sensitivity from the settings, or 0 when it is off (then
/// the host should not run motion updates at all).
///
/// # Safety
/// As [`h2host_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2host_touch_gyro(host: *mut H2Host) -> f32 {
    // SAFETY: the caller's contract.
    unsafe { host.as_ref() }
        .and_then(|t| t.store.as_ref())
        .map_or(0.0, |s| {
            touch::tuning(s, Default::default()).gyro_sensitivity as f32
        })
}

/// Whether a connected controller counts as a physical pad that can play
/// (iOS): not the on-screen controls' own controller, and with an
/// extended gamepad profile ([`overlay::physical`]).
#[unsafe(no_mangle)]
pub extern "C" fn h2host_pad_counts(is_overlay: bool, extended: bool) -> bool {
    overlay::physical(&[overlay::Pad {
        is_overlay,
        extended,
    }]) == 1
}

/// Whether the on-screen controls should be connected, given how many pads
/// count ([`h2host_pad_counts`]) and whether the system keyboard is up
/// ([`overlay::wanted`]).
#[unsafe(no_mangle)]
pub extern "C" fn h2host_overlay_wanted(physical_pads: u32, keyboard_shown: bool) -> bool {
    overlay::wanted(physical_pads as usize, keyboard_shown)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The app builds play on Live by default (the public service), and
    /// sync nothing until an account is set up; live.toml turns Live off.
    #[test]
    fn live_is_on_by_default_and_saves_wait_for_an_account() {
        let data = std::env::temp_dir().join(format!("h2host-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        let store = Arc::new(boot::setting_defs::load(&data, &[]).0);
        let (live, _) = live_hle::platform::config(&data, None);
        assert!(live.enabled);
        assert_eq!(live.server_url.as_deref(), Some(live_proto::DEFAULT_SERVER));
        assert!(
            cloud_saves(&live, &data, &store).is_none(),
            "no account: nothing contacted"
        );
        live_hle::platform::write(
            &data,
            &live_hle::platform::Switch {
                enabled: Some(false),
                server: None,
            },
        )
        .unwrap();
        let (live, _) = live_hle::platform::config(&data, None);
        assert_eq!(live, boot::LiveConfig::default());
        assert!(cloud_saves(&live, &data, &store).is_none());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// The app's first start has no Live account file; the Live client
    /// must still come up (bb4a3f0d failed the boot here, before the menu,
    /// with "configuring Live client").
    #[test]
    fn the_live_client_starts_with_no_accounts_file_yet() {
        let data = std::env::temp_dir().join(format!("h2host-live-first-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&data);
        std::fs::create_dir_all(&data).unwrap();
        let (live, _) = live_hle::platform::config(&data, None);
        assert!(live.enabled);
        assert!(!live.accounts_path.as_ref().unwrap().exists());
        let network = live_hle::NetworkConfig::new(std::net::SocketAddrV4::new(
            std::net::Ipv4Addr::LOCALHOST,
            3074,
        ));
        let hle = live_hle::Hle::with_server_options(live, network, Default::default());
        assert!(hle.is_ok(), "{:?}", hle.err());
        let _ = std::fs::remove_dir_all(&data);
    }

    /// A game that ends says why on the console, not only on the screen.
    #[test]
    fn a_stopped_game_says_why() {
        assert_eq!(
            stopped(&app::GuestExit::Failed(
                "configuring Live client: no file".into()
            )),
            (
                2,
                "the game failed: configuring Live client: no file".into()
            )
        );
        assert_eq!(stopped(&app::GuestExit::Returned).0, 1);
        assert_eq!(
            stopped(&app::GuestExit::Panicked("boom".into())),
            (2, "the game panicked: boom".into())
        );
    }

    #[test]
    fn a_failed_start_reports_why_through_the_handle() {
        let bundle = c"/nonexistent/H2.app";
        let caches = c"/nonexistent/Caches";
        // SAFETY: valid C strings; a null layer is never dereferenced since
        // the incomplete bundle fails first.
        unsafe {
            let host = h2host_start(
                std::ptr::null_mut(),
                1920,
                1080,
                bundle.as_ptr(),
                caches.as_ptr(),
                std::ptr::null(),
            );
            assert!(!host.is_null());
            assert_eq!(h2host_status(host), 2);
            let m = CStr::from_ptr(h2host_message(host)).to_str().unwrap();
            assert!(m.contains("incomplete") && m.contains("disc"), "{m}");
            // The other calls are no-ops on a handle without a shell.
            h2host_redraw(host);
            h2host_resize(host, 1, 1);
            h2host_set_paused(host, true);
            assert_eq!(h2host_status(std::ptr::null_mut()), 2);
            drop(Box::from_raw(host));
        }
    }

    #[test]
    fn null_paths_are_refused() {
        // SAFETY: nulls are checked before use.
        unsafe {
            let host = h2host_start(
                std::ptr::null_mut(),
                1,
                1,
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
            );
            assert_eq!(h2host_status(host), 2);
            assert!(
                CStr::from_ptr(h2host_message(host))
                    .to_str()
                    .unwrap()
                    .contains("path")
            );
            drop(Box::from_raw(host));
        }
    }

    #[test]
    fn text_and_overlay_calls_are_safe_without_a_shell() {
        // SAFETY: a failed start's handle and null are both valid inputs.
        unsafe {
            let host = h2host_start(
                std::ptr::null_mut(),
                1,
                1,
                c"/nonexistent/H2.app".as_ptr(),
                c"/nonexistent/Caches".as_ptr(),
                c"/nonexistent/Documents".as_ptr(),
            );
            assert_eq!(h2host_text_edge(host), 0);
            assert!(!h2host_text_insert(host, c"abc".as_ptr()));
            assert!(!h2host_text_insert(host, std::ptr::null()));
            assert!(!h2host_text_delete_backward(host));
            assert!(!h2host_text_done(host));
            assert!(!h2host_text_cancel(host));
            assert_eq!(h2host_text_edge(std::ptr::null_mut()), 0);
            assert!(!h2host_text_done(std::ptr::null_mut()));
            assert!(!h2host_button(host, 0, true));
            assert!(!h2host_button(std::ptr::null_mut(), 1, true));
            let f = touch::H2TouchFrame::default();
            assert!(!h2host_touch_update(host, &f));
            assert!(!h2host_touch_update(host, std::ptr::null()));
            assert!(!h2host_touch_update(std::ptr::null_mut(), &f));
            assert_eq!(h2host_touch_gyro(host), 0.0);
            let mut l = touch::H2TouchLayout::default();
            assert!(!h2host_touch_layout(host, &mut l));
            assert!(!h2host_touch_layout(host, std::ptr::null_mut()));
            assert!(!h2host_touch_crouched(host));
            assert!(!h2host_touch_crouched(std::ptr::null_mut()));
            h2host_touch_shown(std::ptr::null_mut(), false);
            drop(Box::from_raw(host));
        }
        assert!(h2host_pad_counts(false, true));
        assert!(!h2host_pad_counts(true, true));
        assert!(!h2host_pad_counts(false, false));
        assert!(h2host_overlay_wanted(0, false));
        assert!(!h2host_overlay_wanted(1, false));
        assert!(!h2host_overlay_wanted(0, true));
    }

    /// A bundle whose module is from other sources fails at once with a
    /// message saying how to fix it, instead of analysing for minutes.
    #[test]
    fn a_module_from_other_sources_is_refused_up_front() {
        let base = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join(format!("h2-host-stale-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let l = Layout::new(&base.join("H2.app"), &base.join("Caches"), None);
        std::fs::create_dir_all(&l.disc).unwrap();
        std::fs::create_dir_all(l.module.parent().unwrap()).unwrap();
        let xbe = b"XBEH synthetic image";
        std::fs::write(l.disc.join("default.xbe"), xbe).unwrap();
        std::fs::write(&l.module, b"module").unwrap();
        let stamp = l.module.with_extension("stamp");
        std::fs::write(&stamp, "xbe 0 generator old\n").unwrap();
        let e = check_bundle(&l).unwrap_err().to_string();
        assert!(e.contains("other sources") && e.contains("build.sh"), "{e}");
        std::fs::write(&stamp, boot::build_stamp(xbe)).unwrap();
        check_bundle(&l).unwrap();
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn the_boot_cache_points_into_caches() {
        let l = Layout::new(Path::new("/b"), Path::new("/c"), None);
        assert_eq!(
            environment(&l),
            [("H2_BOOT_CACHE", PathBuf::from("/c/h2boot-cache"))]
        );
    }
}
