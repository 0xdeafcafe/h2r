//! The Rust half of the tvOS app (platform/tvos, docs/tvos.md).
//!
//! The UIKit host creates a full-screen view backed by a `CAMetalLayer`
//! and calls, all on the main thread:
//!
//! * [`h2tv_start`] once, with the layer, its drawable size and the bundle
//!   and Caches paths: the shell ([`app::embedded::Embedded`]) and the game
//!   (`boot::run_hosted`, as `h2 play` runs it) start on a guest thread;
//! * [`h2tv_redraw`] from a `CADisplayLink`;
//! * [`h2tv_resize`] when the drawable size changes, [`h2tv_set_paused`]
//!   when the app leaves or regains the screen;
//! * [`h2tv_status`] to learn the guest has ended, then [`h2tv_message`].
//!
//! `include/h2tv.h` declares the same functions for Swift.
#![allow(unsafe_code)]

pub mod layout;

use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result};

pub use layout::Layout;

/// Kernel calls kept in memory for the report, as `h2 play` keeps.
const TRACE_LIMIT: usize = 4096;

/// What [`h2tv_start`] returns: the running shell. Opaque to the host.
pub struct H2Tv {
    shell: Option<app::embedded::Embedded>,
    message: CString,
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
        // Last run's pipelines, built ahead (Caches; docs/graphics.md).
        let gfx = match d3d::pipeline_store::default_path() {
            Some(p) => gfx.with_pipeline_cache(p),
            None => gfx,
        };
        let input = host.take_input();
        // The player-one pad's handle: no key ever arrives on tvOS, but it
        // records the device last used, which picks the button icons (the
        // remote's, or a pad's).
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
                    "tvos: audio on {} at {} Hz",
                    d.device_id(),
                    audio::AudioSink::sample_rate(&d)
                );
                Box::new(d)
            }
            Err(e) => {
                eprintln!("tvos: no audio device ({e}); audio is discarded");
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
) -> Result<app::embedded::Embedded> {
    let layout = Layout::new(bundle, caches);
    let missing = layout.missing();
    if !missing.is_empty() {
        anyhow::bail!("the app bundle is incomplete: {}", missing.join("; "));
    }
    let seeded = layout.prepare().context("preparing Caches")?;
    eprintln!(
        "tvos: disc {}, data {}, {seeded} boot cache file(s) seeded",
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
    let (store, problems) = boot::setting_defs::load(&layout.data, &[]);
    for p in problems {
        eprintln!("tvos: {p}");
    }
    let store = Arc::new(store);
    let config = app::embedded::tv_config();
    // SAFETY: the host passes a live CAMetalLayer it keeps for the app's
    // lifetime, on the main thread (h2tv.h).
    unsafe { app::embedded::Embedded::start(config, layer, size, guest(layout, store)) }
}

fn path(p: *const c_char) -> Option<PathBuf> {
    if p.is_null() {
        return None;
    }
    // SAFETY: the host passes NUL-terminated UTF-8 paths (h2tv.h).
    let s = unsafe { CStr::from_ptr(p) }.to_str().ok()?;
    Some(PathBuf::from(s))
}

/// Starts the game into `layer` (a `CAMetalLayer*`, `width` x `height`
/// drawable pixels). Never null: on failure the handle carries no shell,
/// [`h2tv_status`] says 2 and [`h2tv_message`] why.
///
/// # Safety
/// `layer` is a live `CAMetalLayer*` kept alive while the handle is; the
/// paths are NUL-terminated; main thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2tv_start(
    layer: *mut c_void,
    width: u32,
    height: u32,
    bundle: *const c_char,
    caches: *const c_char,
) -> *mut H2Tv {
    let r = match (path(bundle), path(caches)) {
        (Some(b), Some(c)) => std::panic::catch_unwind(|| start(layer, (width, height), &b, &c))
            .unwrap_or_else(|_| Err(anyhow::anyhow!("panicked while starting"))),
        _ => Err(anyhow::anyhow!("bad bundle or Caches path")),
    };
    let tv = match r {
        Ok(shell) => H2Tv {
            shell: Some(shell),
            message: CString::default(),
        },
        Err(e) => {
            eprintln!("tvos: {e:#}");
            H2Tv {
                shell: None,
                message: message(&format!("{e:#}")),
            }
        }
    };
    Box::into_raw(Box::new(tv))
}

fn message(s: &str) -> CString {
    CString::new(s.replace('\0', " ")).unwrap_or_default()
}

/// # Safety
/// `tv` is from [`h2tv_start`]; main thread only.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2tv_redraw(tv: *mut H2Tv) {
    // SAFETY: the caller's contract.
    if let Some(s) = unsafe { tv.as_mut() }.and_then(|t| t.shell.as_mut()) {
        s.redraw();
    }
}

/// # Safety
/// As [`h2tv_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2tv_resize(tv: *mut H2Tv, width: u32, height: u32) {
    // SAFETY: the caller's contract.
    if let Some(s) = unsafe { tv.as_mut() }.and_then(|t| t.shell.as_mut()) {
        s.resize((width, height));
    }
}

/// # Safety
/// As [`h2tv_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2tv_set_paused(tv: *mut H2Tv, paused: bool) {
    // SAFETY: the caller's contract.
    if let Some(s) = unsafe { tv.as_ref() }.and_then(|t| t.shell.as_ref()) {
        s.set_paused(paused);
    }
}

/// 0 while the game runs, 1 once it has returned, 2 if it failed or never
/// started ([`h2tv_message`] says why).
///
/// # Safety
/// As [`h2tv_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2tv_status(tv: *mut H2Tv) -> i32 {
    // SAFETY: the caller's contract.
    let Some(t) = (unsafe { tv.as_mut() }) else {
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

/// Why the game is not running, or empty. Valid until the next call on `tv`.
///
/// # Safety
/// As [`h2tv_redraw`].
#[unsafe(no_mangle)]
pub unsafe extern "C" fn h2tv_message(tv: *mut H2Tv) -> *const c_char {
    // SAFETY: the caller's contract.
    match unsafe { tv.as_ref() } {
        Some(t) => t.message.as_ptr(),
        None => c"no handle".as_ptr(),
    }
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
            let tv = h2tv_start(
                std::ptr::null_mut(),
                1920,
                1080,
                bundle.as_ptr(),
                caches.as_ptr(),
            );
            assert!(!tv.is_null());
            assert_eq!(h2tv_status(tv), 2);
            let m = CStr::from_ptr(h2tv_message(tv)).to_str().unwrap();
            assert!(m.contains("incomplete") && m.contains("disc"), "{m}");
            // The other calls are no-ops on a handle without a shell.
            h2tv_redraw(tv);
            h2tv_resize(tv, 1, 1);
            h2tv_set_paused(tv, true);
            assert_eq!(h2tv_status(std::ptr::null_mut()), 2);
            drop(Box::from_raw(tv));
        }
    }

    #[test]
    fn null_paths_are_refused() {
        // SAFETY: nulls are checked before use.
        unsafe {
            let tv = h2tv_start(
                std::ptr::null_mut(),
                1,
                1,
                std::ptr::null(),
                std::ptr::null(),
            );
            assert_eq!(h2tv_status(tv), 2);
            assert!(
                CStr::from_ptr(h2tv_message(tv))
                    .to_str()
                    .unwrap()
                    .contains("path")
            );
            drop(Box::from_raw(tv));
        }
    }

    #[test]
    fn the_boot_cache_points_into_caches() {
        let l = Layout::new(Path::new("/b"), Path::new("/c"));
        assert_eq!(
            environment(&l),
            [("H2_BOOT_CACHE", PathBuf::from("/c/h2boot-cache"))]
        );
    }
}
