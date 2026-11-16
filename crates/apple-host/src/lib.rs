//! The Rust half of the tvOS and iOS apps (platform/tvos, platform/ios;
//! docs/runtime/tvos.md, docs/runtime/ios.md).
//!
//! The UIKit host creates a full-screen view backed by a `CAMetalLayer`
//! and calls, all on the main thread:
//!
//! * [`h2host_start`] once, with the layer, its drawable size and the
//!   bundle, Caches and (iOS) Documents paths: the shell
//!   ([`app::embedded::Embedded`]) and the game (`boot::run_hosted`, as
//!   `h2 play` runs it) start on a guest thread;
//! * [`h2host_redraw`] from a `CADisplayLink`;
//! * [`h2host_resize`] when the drawable size changes, [`h2host_set_paused`]
//!   when the app leaves or regains the screen;
//! * [`h2host_status`] to learn the guest has ended, then [`h2host_message`];
//! * [`h2host_text_edge`] each frame, to show or hide the system keyboard
//!   while the game's virtual keyboard has a field open, and
//!   [`h2host_text_insert`], [`h2host_text_delete_backward`],
//!   [`h2host_text_done`] and [`h2host_text_cancel`] for what is typed
//!   ([`text`]);
//! * on iOS, [`h2host_pad_counts`] and [`h2host_overlay_wanted`] to decide
//!   when the on-screen controls show ([`overlay`]), and [`h2host_button`]
//!   for the Start and Back buttons they lack.
//!
//! `platform/apple/h2host.h` declares the same functions for Swift.
#![allow(unsafe_code)]

pub mod layout;
pub mod overlay;
pub mod text;

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

pub use layout::Layout;

/// Kernel calls kept in memory for the report, as `h2 play` keeps.
const TRACE_LIMIT: usize = 4096;

/// What [`h2host_start`] returns: the running shell. Opaque to the host.
pub struct H2Host {
    shell: Option<app::embedded::Embedded>,
    message: CString,
    text: text::Edges,
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
        let gfx = d3d::GfxBackend::new(host.gfx_context()).on_present(host.present_hook());
        // Last run's pipelines, built ahead (Caches; docs/graphics/graphics.md).
        let gfx = match d3d::pipeline_store::default_path() {
            Some(p) => gfx.with_pipeline_cache(p),
            None => gfx,
        };
        let input = host.take_input();
        // The player-one pad's handle: no key arrives, but it records the
        // device last used, which picks the button icons (the remote's, or
        // a pad's), and carries the system keyboard's text.
        let keyboard = host.keyboard().cloned();
        let hosted = boot::Host {
            gfx: Some(gfx),
            gfx_view: None,
            render_thread: boot::d3d_glue::render_thread_requested(),
            audio_sink: Some(audio_sink()),
            pad_speaker: None,
            settings: Some(store),
            trace_limit: Some(TRACE_LIMIT),
            level: None,
            input,
            keyboard,
            icons: Some(boot::icons::IconOptions {
                choice: Default::default(),
                gamepads: true,
            }),
            between_slices: Some(Box::new(move |k: &kernel::Kernel| {
                host.tick(k.clock().counter()) == app::Control::Continue
            })),
            on_vfs: None,
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
        };
        let r = boot::run_hosted(&xbe, &layout.module, &opts, hosted);
        vfs::flush_pending_metadata();
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
) -> Result<app::embedded::Embedded> {
    let layout = Layout::new(bundle, caches, documents);
    let missing = layout.missing();
    if !missing.is_empty() {
        anyhow::bail!("the app bundle is incomplete: {}", missing.join("; "));
    }
    let seeded = layout.prepare().context("preparing the app's directories")?;
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
    // SAFETY: the host passes a live CAMetalLayer it keeps for the app's
    // lifetime, on the main thread (h2host.h).
    unsafe { app::embedded::Embedded::start(config, layer, size, guest(layout, store)) }
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
        Ok(shell) => H2Host {
            shell: Some(shell),
            message: CString::default(),
            text: Default::default(),
        },
        Err(e) => {
            eprintln!("h2host: {e:#}");
            H2Host {
                shell: None,
                message: message(&format!("{e:#}")),
                text: Default::default(),
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
    let (code, text) = match s.guest_exit() {
        None => return 0,
        Some(app::GuestExit::Returned) => (1, "The game ended.".to_owned()),
        Some(e) => (2, format!("{e:?}")),
    };
    if t.message.is_empty() {
        t.message = message(&text);
    }
    code
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

/// Whether a connected controller counts as a physical pad that can play
/// (iOS): not the on-screen controls' own controller, and with an
/// extended gamepad profile ([`overlay::physical`]).
#[unsafe(no_mangle)]
pub extern "C" fn h2host_pad_counts(is_overlay: bool, extended: bool) -> bool {
    overlay::physical(&[overlay::Pad { is_overlay, extended }]) == 1
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
