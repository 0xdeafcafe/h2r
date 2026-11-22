//! The Rust half of the tvOS and iOS apps (platform/tvos, platform/ios;
//! docs/runtime/tvos.md, docs/runtime/ios.md).
//!
//! The UIKit host creates a full-screen view backed by a `CAMetalLayer`
//! and calls, all on the main thread:
//!
//! * [`h2host_start`] once, with the layer, its drawable size and the
//!   bundle, Caches and (iOS) Documents paths: the shell
//!   ([`app::embedded::Embedded`]) and the game (`boot::run_hosted_with_live`,
//!   as `h2 play` runs it, but with Live on by default: `live_hle::platform`)
//!   start on a guest thread;
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
//!   when the on-screen controls show ([`overlay`]), [`h2host_touch_shown`]
//!   as they show and hide, [`h2host_touch_update`] with what the fingers on
//!   them hold ([`touch`], `input::touch`), and [`h2host_touch_gyro`] to
//!   learn whether to aim with the gyro. [`h2host_button`] holds Start or
//!   Back through a bound key (kept for hosts without the touch pad).
//!
//! `platform/apple/h2host.h` declares the same functions for Swift.
#![allow(unsafe_code)]

pub mod layout;
pub mod maps;
pub mod overlay;
pub mod text;
pub mod touch;

// `app::embedded` is built for Apple and Windows only, so the shell and the
// C interface are too; the packer example and the host-neutral modules above
// build everywhere (the Linux CI job checks them).
#[cfg(any(target_vendor = "apple", windows))]
mod host;
#[cfg(any(target_vendor = "apple", windows))]
pub use host::*;

pub use layout::Layout;
