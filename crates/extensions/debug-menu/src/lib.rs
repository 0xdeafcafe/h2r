//! Our in-game debug menu (docs/runtime/debug-menu.md). The retail game
//! has none; this one is ours, drawn by the host over the game.
//!
//! * [`model`]: rows, sections and the navigation state machine. Every
//!   value comes from an agent call and every change is one, so the menu
//!   and `h2 agent` read and change the same registries.
//! * [`sources`]: the sections (Render, Shader overrides, Tag edits,
//!   Cheats, Perf, Features), each over agent methods.
//! * [`gate`]: the input side: the touchpad + Options chord, gestures,
//!   and hiding pads from the game while the menu is open.
//! * [`layout`]: the lines the overlay draws.
//! * [`Shared`]: the menu behind a lock, for the input thread (the gate),
//!   the window (keys and drawing) and the agent connection ([`Rpc`]).

pub mod gate;
pub mod layout;
pub mod model;
pub mod sources;

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::Value;

pub use gate::Gate;
pub use layout::{Line, Tone};
pub use model::{Action, Answer, Call, Edit, Menu, Nav, Row, Section, Setter};

/// Where the menu's calls go: the agent API, in process.
pub trait Rpc: Send {
    /// Sends call `id`.
    fn send(&mut self, id: u64, call: &Call);
    /// The answers that came since the last look.
    fn recv(&mut self) -> Vec<(u64, Result<Value, String>)>;
}

/// The window's keys for the menu (docs/runtime/debug-menu.md "Keyboard").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// Opens and closes it (Backquote).
    Toggle,
    Nav(Nav),
}

/// What [`Shared`] guards.
pub struct Inner {
    pub menu: Menu,
    rpc: Option<Box<dyn Rpc>>,
    epoch: Instant,
    /// The menu's generation when the window last drew it.
    drawn: Option<u64>,
    /// What the gate saw at its last filter ([`Shared::gate_report`]).
    gate: gate::GateReport,
}

impl Inner {
    /// Answers in, stale pages listed, calls out. With no agent connected
    /// every call fails at once, saying so.
    pub fn pump(&mut self, now: Duration) {
        if let Some(rpc) = &mut self.rpc {
            for (id, r) in rpc.recv() {
                self.menu.answer(id, r, now);
            }
        }
        self.menu.tick(now);
        for (id, call) in self.menu.take_outbox() {
            match &mut self.rpc {
                Some(rpc) => rpc.send(id, &call),
                None => self.menu.answer(
                    id,
                    Err("no agent in this game (the menu needs its host methods)".into()),
                    now,
                ),
            }
        }
    }
}

/// The menu shared between threads. Cloning shares it.
#[derive(Clone)]
pub struct Shared(Arc<Mutex<Inner>>);

impl Default for Shared {
    fn default() -> Self {
        Self::new(Menu::default())
    }
}

impl std::fmt::Debug for Shared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Shared")
            .field("open", &self.is_open())
            .finish()
    }
}

impl Shared {
    pub fn new(menu: Menu) -> Self {
        Shared(Arc::new(Mutex::new(Inner {
            menu,
            rpc: None,
            epoch: Instant::now(),
            drawn: None,
            gate: gate::GateReport::default(),
        })))
    }

    pub fn lock(&self) -> MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Time since it was made: the menu's clock.
    pub fn now(&self) -> Duration {
        self.lock().epoch.elapsed()
    }

    /// Connects the agent; until then calls fail at once.
    pub fn connect(&self, rpc: Box<dyn Rpc>) {
        self.lock().rpc = Some(rpc);
    }

    pub fn is_open(&self) -> bool {
        self.lock().menu.is_open()
    }

    /// The gate for the input manager.
    pub fn gate(&self) -> Gate {
        Gate::new(self.clone())
    }

    /// Whether the menu is open, and each pad's state in the gate as of
    /// its last filter (`h2 agent hostpads`).
    pub fn gate_report(&self) -> (bool, gate::GateReport) {
        let g = self.lock();
        (g.menu.is_open(), g.gate.clone())
    }

    /// A key from the window; true when the menu took it (so the game
    /// must not get it).
    pub fn key(&self, k: Key) -> bool {
        let mut g = self.lock();
        let now = g.epoch.elapsed();
        match k {
            Key::Toggle => g.menu.toggle(now),
            Key::Nav(_) if !g.menu.is_open() => return false,
            Key::Nav(n) => g.menu.nav(n, now),
        }
        g.pump(now);
        true
    }

    /// Adds a section to the first page ([`Menu::add_section`]).
    pub fn add_section(&self, s: Section) {
        self.lock().menu.add_section(s);
    }

    /// The overlay's lines while open, `None` while closed. Pumps the
    /// agent connection first, so answers show without input.
    pub fn lines(&self) -> Option<Vec<Line>> {
        let mut g = self.lock();
        let now = g.epoch.elapsed();
        g.pump(now);
        let open = g.menu.is_open();
        g.drawn = Some(g.menu.generation());
        open.then(|| layout::lines(&g.menu))
    }

    /// Whether the window should draw again: what it shows changed since
    /// [`lines`](Shared::lines), or calls are out and the menu is open.
    pub fn needs_redraw(&self) -> bool {
        let mut g = self.lock();
        let now = g.epoch.elapsed();
        g.pump(now);
        g.drawn != Some(g.menu.generation()) || (g.menu.is_open() && g.menu.waiting())
    }
}

#[cfg(test)]
mod tests;
