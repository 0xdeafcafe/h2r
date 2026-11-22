//! The superbounce lock-on's red reticle (docs/game/superbounce.md, "The
//! reticle"): while the falling player is locked on the landing
//! ([`crate::superbounce_hud::reticle_lock_now`]), a red ring with four
//! ticks over the game's own reticle, in the lock's red
//! ([`crate::superbounce_world::RED`]).
//!
//! It is a host mark ([`gfx::world_lines::HudMark`]), drawn in screen
//! space by the window; the game's reticle is not touched and no guest
//! hook is involved, so the HUD stays on the pipelined render thread.
//!
//! **Place.** The centre of the game's reticle, `rects::RETICLE`'s
//! centre, `(320, 240)` of the fullscreen frame ([`RETICLE_CENTRE`]); the
//! first view's centre and the host's HUD fit about it come from
//! [`crate::render_aspect::hud_centre_fit`], so it follows the widescreen
//! fit and the first view of a split screen.
//!
//! **Any frame rate.** The lock is a per-tick answer; the mark fades in
//! and out over [`FADE`] seconds by the wall clock ([`Fade`]), evaluated
//! at each present's `now`, so it is smooth at any frame rate.

use std::sync::Mutex;
use std::time::Instant;

use crate::superbounce_view::{Shape, ring};
use crate::superbounce_world::RED;

#[cfg(feature = "gpu")]
pub use crate::render_aspect::RETICLE_CENTRE;
/// The fullscreen frame's centre, as `render_aspect` names it; builds
/// without the GPU renderer have no `render_aspect`.
#[cfg(not(feature = "gpu"))]
pub const RETICLE_CENTRE: [f32; 2] = [320.0, 240.0];

/// Seconds the mark takes to fade fully in or out.
pub const FADE: f32 = 0.1;
/// The ring's radius, HUD pixels: about the circle of Halo 2's default
/// assault rifle and battle rifle reticles (the reserved `rects::RETICLE`
/// is 64 by 64).
pub const RING_RADIUS: f32 = 11.0;
/// Where the four ticks (up, right, down, left) start and end, from the
/// centre.
pub const TICK_FROM: f32 = 14.0;
pub const TICK_TO: f32 = 20.0;
/// The line width, HUD pixels.
pub const WIDTH: f32 = 1.5;
/// The mark's alpha when fully faded in.
pub const ALPHA: f32 = 0.95;

/// The fade between hidden and shown, started when the lock changed.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fade {
    /// The level (0 to 1) when it started.
    from: f32,
    /// Where it goes: shown or hidden.
    shown: bool,
    start: Instant,
}

impl Fade {
    /// The level, 0 hidden to 1 shown, at `now`: smoothstep over
    /// [`FADE`] seconds from where the last change left it.
    pub fn level(&self, now: Instant) -> f32 {
        let to = if self.shown { 1.0 } else { 0.0 };
        let t = (now.saturating_duration_since(self.start).as_secs_f32() / FADE).clamp(0.0, 1.0);
        self.from + (to - self.from) * t * t * (3.0 - 2.0 * t)
    }

    /// The fade at `now` given whether the mark is wanted (the mode is
    /// on and the lock holds): `prev` carried on, or turned around from
    /// the level it shows at `now`. `None` while hidden and fully faded.
    pub fn next(prev: Option<Fade>, wanted: bool, now: Instant) -> Option<Fade> {
        let fade = match prev {
            Some(p) if p.shown == wanted => p,
            Some(p) => Fade {
                from: p.level(now),
                shown: wanted,
                start: now,
            },
            None if wanted => Fade {
                from: 0.0,
                shown: true,
                start: now,
            },
            None => return None,
        };
        (fade.shown || fade.level(now) > 0.0).then_some(fade)
    }
}

/// The mark at `level` (0 to 1): the ring and four ticks about `centre`,
/// then fitted by `fit` (`[sx, sy, ox, oy]`, [`Shape::fitted`]). Nothing
/// at level 0.
pub fn shapes(centre: [f32; 2], fit: [f32; 4], level: f32) -> Vec<Shape> {
    let alpha = ALPHA * level.clamp(0.0, 1.0);
    if alpha <= 0.0 {
        return Vec::new();
    }
    let path = |points: Vec<[f32; 2]>, closed: bool| Shape::Path {
        points,
        closed,
        width: WIDTH,
        dashed: false,
        color: RED,
        alpha,
    };
    let mut out = vec![path(ring(centre, RING_RADIUS, 24), true)];
    for [dx, dy] in [[0.0, -1.0], [1.0, 0.0], [0.0, 1.0], [-1.0, 0.0]] {
        out.push(path(
            vec![
                [centre[0] + dx * TICK_FROM, centre[1] + dy * TICK_FROM],
                [centre[0] + dx * TICK_TO, centre[1] + dy * TICK_TO],
            ],
            false,
        ));
    }
    out.iter().map(|s| s.fitted(fit)).collect()
}

/// The fade the window's source carries on between captures.
static FADE_STATE: Mutex<Option<Fade>> = Mutex::new(None);

/// Advances the shared fade to `now` for `wanted` and returns it.
fn advance(wanted: bool, now: Instant) -> Option<Fade> {
    let mut g = FADE_STATE.lock().unwrap_or_else(|e| e.into_inner());
    *g = Fade::next(*g, wanted, now);
    *g
}

/// The window's source for the reticle mark (windowed hosts).
#[cfg(feature = "gpu")]
pub use gpu::Source;

#[cfg(feature = "gpu")]
mod gpu {
    use std::sync::Arc;

    use gfx::world_lines::{HudMark, ImageMarks};

    use super::*;

    /// The red reticle while the lock holds.
    pub struct Source;

    impl crate::world_lines::WorldLineSource for Source {
        fn active(&self) -> bool {
            crate::superbounce_mode::active()
        }

        fn capture(&self, _guest: &dyn engine::GuestMemory) -> Option<Arc<dyn ImageMarks>> {
            let wanted =
                crate::superbounce_mode::active() && crate::superbounce_hud::reticle_lock_now();
            let fade = advance(wanted, Instant::now())?;
            let (centre, fit) = crate::render_aspect::hud_centre_fit();
            Some(Arc::new(Sample { fade, centre, fit }))
        }
    }

    #[derive(Debug)]
    struct Sample {
        fade: Fade,
        centre: [f32; 2],
        fit: [f32; 4],
    }

    impl ImageMarks for Sample {
        fn lines(&self, _visitor: &mut dyn FnMut(gfx::world_lines::WorldLine<'_>)) {}

        fn hud(&self, now: &dyn Fn() -> Instant, visitor: &mut dyn FnMut(HudMark<'_>)) {
            for shape in shapes(self.centre, self.fit, self.fade.level(now())) {
                crate::superbounce_view::emit(&shape, visitor);
            }
        }
    }
}

#[cfg(test)]
#[path = "superbounce_reticle_tests.rs"]
mod tests;
