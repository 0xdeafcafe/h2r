//! Per-player aim assist in a multiplayer game: stronger for the local
//! player on the touch controls, retail for everyone else
//! (docs/game/aim-assist.md "Multiplayer").
//!
//! **Where the values are used.** Every aim-assist value a player's own
//! machine computes goes through the player-control update
//! [`PLAYER_CONTROL_UPDATE`] (`0x185be0`), which `0x1870a0` runs once per
//! bound *local* user and whose result is that player's action, the input
//! the simulation is handed. Inside it, the aim-assist search `0x1a4900`
//! asks [`AIM_ASSIST_PARAMS`] (`0x1a50a0`) for the held weapon's cone
//! (aim cone and pull angles and ranges) and picks the target, and
//! `0x18622b` reads the globals' slowdown and follow. So all of
//! it is input side: it only shapes what this machine's own player sends.
//!
//! The one simulation-side reader is the shot aim `0x1a4c00` (from the
//! barrel fire `0x104150`), which calls `0x1a50a0` again for the shooter's
//! unit to bend the shot. That call is left retail on every machine.
//!
//! **What we change.** Around one local player's control update, when that
//! player is the touch player ([`player_level`]):
//! - `before` `0x185be0`: open a window for that player and set the
//!   globals' slowdown and follow to retail times the level's factor;
//! - `after` `0x1a50a0`, only inside the window: scale the 0x24-byte
//!   result the engine just built from the tags ([`scale_params`]);
//! - `after` `0x185be0`: put slowdown and follow back, bit for bit, and
//!   close the window.
//!
//! No weapon tag is written. Every other player, every other machine, and
//! every simulation-side read sees retail values bit for bit.

use std::sync::Mutex;

use crate::layout_glue::Gm;

use super::{
    FOLLOW, Factors, Level, MATG_PLAYER_CONTROL, MAX_ANGLE, MAX_PLAYER_CONTROLS, MAX_STICK,
    MODE_MULTIPLAYER, PLAYER_CONTROL_STRIDE, SETTING_DEFAULT, SETTING_VERY_STRONG, SLOWDOWN, scale,
};

pub use feature::points::{AIM_ASSIST_PARAMS, PLAYER_CONTROL_UPDATE};

/// The pointer to the loaded globals (`matg`) tag's definition; the
/// player-control update reads `[[0x4e034c]+0xf4]` (docs/runtime/level-boot.md).
pub const GLOBALS_TAG: u32 = 0x004e_034c;

/// The controller port the touch controls drive: the touch pad is merged
/// into player one's port (`input::touch`, `KeyboardMouse::touch_update`).
pub const TOUCH_CONTROLLER: u32 = 0;

/// The result of `0x1a50a0` (written at ESI), each an f32. Read from the
/// stores at `0x1a514f` to `0x1a5181`, `0x1a51a5`, `0x1a5292`, `0x1a52a2`
/// and `0x1a51de`.
pub mod params {
    /// Aim cone angle, divided by the zoom magnification.
    pub const CONE_ANGLE: u32 = 0x0;
    /// Aim cone range, times the magnification.
    pub const CONE_RANGE: u32 = 0x4;
    /// Pull angle (the larger of both hands' when dual wielding).
    pub const PULL_ANGLE: u32 = 0x8;
    /// Pull range (likewise).
    pub const PULL_RANGE: u32 = 0xc;
    /// How far a shot may bend: the larger of the bend and aim cone
    /// angles, zoom applied.
    pub const BEND_ANGLE: u32 = 0x10;
    /// Flags (`0x8`, `0x10`, `0x20`); not a value.
    pub const FLAGS: u32 = 0x14;
    /// The second weapon's aim cone angle and range (dual wielding).
    pub const SECOND_CONE_ANGLE: u32 = 0x18;
    pub const SECOND_CONE_RANGE: u32 = 0x1c;
    /// A copy of the aim cone range for some weapon classes (`0x1a51de`),
    /// else 0.
    pub const CLASS_RANGE: u32 = 0x20;
    pub const SIZE: u32 = 0x24;
}

/// The level for one local player in an active game of options `mode`.
///
/// Only a multiplayer game has a per-player level (campaign and co-op keep
/// the tag scaling of [`super::level_now`], so the two never stack). In
/// one, the touch player (controller [`TOUCH_CONTROLLER`] while the touch
/// controls show) gets the setting's level, Auto meaning Strong; every
/// other player, local or remote, gets Default. A controller of `u32::MAX`
/// (`-1`) is a remote or unbound player.
pub fn player_level(setting: i64, mode: Option<u32>, controller: u32, touch_shown: bool) -> Level {
    if mode != Some(MODE_MULTIPLAYER) || controller != TOUCH_CONTROLLER || !touch_shown {
        return Level::Default;
    }
    match setting {
        SETTING_DEFAULT => Level::Default,
        SETTING_VERY_STRONG => Level::VeryStrong,
        _ => Level::Strong,
    }
}

/// Scales the aim-assist result at `out` by `f`, as the tag path would
/// have: angles by `f.angle` (capped at [`MAX_ANGLE`]), ranges by
/// `f.range`, and the bend angle raised to the scaled aim cone angle when
/// that is larger (it is the larger of bend and aim cone). Values that
/// are not finite positive numbers are kept. Returns the fields written.
pub fn scale_params<M: Gm>(m: &mut M, out: u32, f: Factors) -> u32 {
    use params::*;
    let mut writes = 0;
    let mut set = |m: &mut M, off: u32, factor: f32, max: f32| -> Option<f32> {
        let cur = f32::from_bits(m.u32(out + off)?);
        let want = scale(cur, factor, max);
        if want.to_bits() != cur.to_bits() && m.set_u32(out + off, want.to_bits()) {
            writes += 1;
        }
        Some(want)
    };
    let cone = set(m, CONE_ANGLE, f.angle, MAX_ANGLE);
    set(m, CONE_RANGE, f.range, f32::MAX);
    set(m, PULL_ANGLE, f.angle, MAX_ANGLE);
    set(m, PULL_RANGE, f.range, f32::MAX);
    set(m, SECOND_CONE_ANGLE, f.angle, MAX_ANGLE);
    set(m, SECOND_CONE_RANGE, f.range, f32::MAX);
    set(m, CLASS_RANGE, f.range, f32::MAX);
    if let (Some(a), Some(bend)) = (cone, m.u32(out + BEND_ANGLE).map(f32::from_bits))
        && a.is_finite()
        && bend.is_finite()
        && a > bend
        && m.set_u32(out + BEND_ANGLE, a.to_bits())
    {
        writes += 1;
    }
    writes
}

/// A field we changed for the window: its address, retail bits, our bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Held {
    at: u32,
    retail: u32,
    written: u32,
}

/// The open window: one local player's control update.
#[derive(Debug)]
struct Window {
    factors: Factors,
    held: Vec<Held>,
}

/// What the hooks share. One guest thread runs player control, so at most
/// one window is open; a second `enter` closes the first.
#[derive(Debug, Default)]
pub struct PerPlayer {
    window: Mutex<Option<Window>>,
}

impl PerPlayer {
    /// Before a local player's control update: opens the window when
    /// `level` is not Default and sets slowdown and follow. Returns
    /// whether a window opened.
    pub fn enter<M: Gm>(&self, m: &mut M, level: Level) -> bool {
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = w.take() {
            restore(m, &old.held);
        }
        if level == Level::Default {
            return false;
        }
        let factors = level.factors();
        let held = stick_on(m, factors.stick);
        *w = Some(Window { factors, held });
        true
    }

    /// After `0x1a50a0`: scales its result at `out` if a window is open.
    /// Returns the fields written (0 outside a window: retail bit for bit).
    pub fn params<M: Gm>(&self, m: &mut M, out: u32) -> u32 {
        let w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        match &*w {
            Some(win) => scale_params(m, out, win.factors),
            None => 0,
        }
    }

    /// After the control update: slowdown and follow back to retail,
    /// window closed.
    pub fn leave<M: Gm>(&self, m: &mut M) {
        let mut w = self.window.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(old) = w.take() {
            restore(m, &old.held);
        }
    }

    /// Whether a window is open (tests and logs).
    pub fn open(&self) -> bool {
        self.window
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
    }
}

/// Sets the globals' player-control slowdown and follow to retail times
/// `factor` (capped at [`MAX_STICK`]); returns what to put back.
fn stick_on<M: Gm>(m: &mut M, factor: f32) -> Vec<Held> {
    let mut held = Vec::new();
    let Some(g) = m.u32(GLOBALS_TAG).filter(|g| *g != 0) else {
        return held;
    };
    let pc = g + MATG_PLAYER_CONTROL;
    let (Some(n), Some(at)) = (m.u32(pc), m.u32(pc + 4)) else {
        return held;
    };
    if n > MAX_PLAYER_CONTROLS || at == 0 {
        return held;
    }
    for k in 0..n {
        let el = at.wrapping_add(k * PLAYER_CONTROL_STRIDE);
        for off in [SLOWDOWN, FOLLOW] {
            let a = el + off;
            let Some(retail) = m.u32(a) else { continue };
            let want = scale(f32::from_bits(retail), factor, MAX_STICK).to_bits();
            if want != retail && m.set_u32(a, want) {
                held.push(Held {
                    at: a,
                    retail,
                    written: want,
                });
            }
        }
    }
    held
}

/// Puts back each held field that still holds what we wrote (anything else
/// was a reload, and is left alone).
fn restore<M: Gm>(m: &mut M, held: &[Held]) {
    for h in held {
        if m.u32(h.at) == Some(h.written) {
            m.set_u32(h.at, h.retail);
        }
    }
}

#[cfg(test)]
mod tests;
