//! The `frame` and `game` bridges: Rust events over hooks on the retail
//! loop, until those subsystems go native (docs/engine/feature-engine.md,
//! candidates 3 and 4). A feature that uses them names no address.
//!
//! Every event is a read-only observer after the guest function returned,
//! so any number of features share one without an order between them.

use engine::points;

use crate::{Builder, HookRef, Obs, Run};

/// Frame and tick events. See [`Builder::frame`].
pub struct Frame<'b, 'r>(pub(crate) &'b mut Builder<'r>);

impl<'b> Frame<'b, '_> {
    /// After each simulated tick (`game_tick` returned; the tick count has
    /// moved).
    pub fn on_tick(
        self,
        label: &'static str,
        f: impl Fn(&Obs) + Send + Sync + 'static,
    ) -> HookRef<'b> {
        self.0.push(
            &points::GAME_TICK,
            Run::ObserveAfter(std::sync::Arc::new(f)),
            label,
        )
    }
}

/// Game lifecycle events. See [`Builder::game`].
pub struct Game<'b, 'r>(pub(crate) &'b mut Builder<'r>);

impl<'b> Game<'b, '_> {
    /// A game started: activation returned success (AL non-zero), so the
    /// map and its first BSP are in and the game globals hold the options.
    pub fn on_start(
        self,
        label: &'static str,
        f: impl Fn(&Obs) + Send + Sync + 'static,
    ) -> HookRef<'b> {
        let run = Run::ObserveAfter(std::sync::Arc::new(move |o: &Obs| {
            if o.cpu.eax & 0xff != 0 {
                f(o)
            }
        }));
        self.0.push(&points::GAME_ACTIVATE, run, label)
    }

    /// The game-end path returned (the end latch may or may not be set:
    /// read it).
    pub fn on_end(
        self,
        label: &'static str,
        f: impl Fn(&Obs) + Send + Sync + 'static,
    ) -> HookRef<'b> {
        self.0.push(
            &points::GAME_END,
            Run::ObserveAfter(std::sync::Arc::new(f)),
            label,
        )
    }
}
