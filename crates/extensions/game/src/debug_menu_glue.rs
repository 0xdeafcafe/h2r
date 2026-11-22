//! Our debug menu in the game (docs/runtime/debug-menu.md): connects the
//! menu to the agent as an in-process client, and adds the host methods
//! it lists beside `render.*` and `edit.*`: `settings.*` over the
//! settings store and `perf.overlay` over the watermark's switch. Also
//! `input.host` (`h2 agent hostpads`) over the input manager's report and
//! the menu's gate, for when a pad doesn't open the menu.
//!
//! The menu itself (rows, navigation, input, drawing) is the host's:
//! `h2 play` makes it, puts its gate on the input manager and its lines
//! on the window. Here it only gets something to talk to.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use debug_menu::Rpc;
use debug_menu::Call;

/// Adds the menu's host methods to `agent` (with or without a menu, so
/// `h2 agent` has them on `h2 serve` too) and connects `menu` when there
/// is one.
pub fn install(
    menu: Option<&debug_menu::Shared>,
    host_pads: Option<input::report::HostReportHandle>,
    agent: &agent::Agent,
    settings: Option<Arc<settings::Store>>,
    perf_overlay: Option<Arc<AtomicBool>>,
    cheat_line: Option<Arc<Mutex<Vec<String>>>>,
) {
    if let Some(line) = cheat_line {
        watch_cheats(agent, line);
    }
    if let Some(store) = settings {
        agent::menu::add_settings_methods(agent, store);
    }
    if let Some(on) = perf_overlay {
        agent::menu::add_perf_overlay(agent, on);
    }
    if let Some(report) = host_pads {
        agent::hostpads::add_host_pads(agent, report, menu.cloned());
    }
    if let Some(menu) = menu {
        agent::menu::connect(menu, agent);
        eprintln!(
            "debug menu: touchpad click, hold Back + D-pad down, or ` on the keyboard (docs/runtime/debug-menu.md)"
        );
    }
}

/// How often the watermark's cheat line is refreshed.
const CHEAT_POLL: Duration = Duration::from_millis(500);

/// Keeps `line` (the watermark's active-cheats line) filled from
/// `cheat.list`, over an in-process connection of its own, so it shows
/// while the menu is closed.
fn watch_cheats(agent: &agent::Agent, line: Arc<Mutex<Vec<String>>>) {
    let mut rpc = agent::menu::MenuRpc::new(agent);
    let spawned = std::thread::Builder::new()
        .name("cheat-line".into())
        .spawn(move || {
            let call = Call::bare(debug_menu::sources::method::CHEAT_LIST);
            let mut id = 0u64;
            loop {
                id += 1;
                rpc.send(id, &call);
                std::thread::sleep(CHEAT_POLL);
                for (_, answer) in rpc.recv() {
                    let on = match answer {
                        Ok(v) => debug_menu::sources::active_cheats(&v),
                        Err(_) => Vec::new(),
                    };
                    if let Ok(mut l) = line.lock() {
                        *l = on;
                    }
                }
            }
        });
    if let Err(e) = spawned {
        eprintln!("debug menu: cheat line off: {e}");
    }
}
