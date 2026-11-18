//! Our debug menu in the game (docs/runtime/debug-menu.md): connects the
//! menu to the agent as an in-process client, and adds the host methods
//! it lists beside `render.*` and `edit.*`: `settings.*` over the
//! settings store and `perf.overlay` over the watermark's switch.
//!
//! The menu itself (rows, navigation, input, drawing) is the host's:
//! `h2 play` makes it, puts its gate on the input manager and its lines
//! on the window. Here it only gets something to talk to.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

/// Adds the menu's host methods to `agent` (with or without a menu, so
/// `h2 agent` has them on `h2 serve` too) and connects `menu` when there
/// is one.
pub fn install(
    menu: Option<&debug_menu::Shared>,
    agent: &agent::Agent,
    settings: Option<Arc<settings::Store>>,
    perf_overlay: Option<Arc<AtomicBool>>,
) {
    if let Some(store) = settings {
        agent::menu::add_settings_methods(agent, store);
    }
    if let Some(on) = perf_overlay {
        agent::menu::add_perf_overlay(agent, on);
    }
    if let Some(menu) = menu {
        agent::menu::connect(menu, agent);
        eprintln!(
            "debug menu: touchpad click + Options, or ` on the keyboard (docs/runtime/debug-menu.md)"
        );
    }
}
