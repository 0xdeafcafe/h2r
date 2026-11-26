//! The game tick `0x00137fe0` from Rust (docs/engine/sim-rewrite.md, "The
//! tick driver"): the x87 control word, the simulation update record, the
//! 29 phases in retail order, the record's close, and the tick count. Each
//! phase is owned by the guest or by Rust ([`Owner`]); the [`Ledger`] counts
//! the guest phases the driver still calls, and `tick-ledger.toml` in
//! `sim-bridge` holds the number that may only fall.
//!
//! Pure data and one async body over [`Cx`]: the installer is
//! `sim_bridge::tick`, the differential proof is
//! `tests/sim_tick_differential.rs`.

use std::sync::Mutex;

use crate::entry::{Cx, EntryError, EntryResult, GuestCall, Ret};

/// Who runs a phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The lifted original, called with a guest call.
    Guest,
    /// A Rust body in this driver.
    Rust,
}

/// One phase of the tick.
#[derive(Clone, Copy, Debug)]
pub struct Phase {
    /// The guest function (a phase's guest entry is kept after a Rust body
    /// takes it over: it names the phase and is what the differential test
    /// compares against).
    pub entry: u32,
    pub name: &'static str,
    pub owner: Owner,
}

const fn guest(entry: u32, name: &'static str) -> Phase {
    Phase { entry, name, owner: Owner::Guest }
}

/// The 29 phases in the order `game_tick` calls them
/// (docs/reviews/2026-11-20-sim.md, section 2.1). Phases 26 (`0x165dc1`)
/// and 28 (`0x1917b2`) are odd addresses; the call form here is the plain
/// no-argument call and the differential test checks it against the lifted
/// body.
pub const PHASES: &[Phase] = &[
    guest(0x0014_d3a0, "players_before"),
    guest(0x0013_8db0, "unknown_138db0"),
    guest(0x001c_83c0, "ai"),
    guest(0x001f_b510, "unknown_1fb510"),
    guest(0x0020_b660, "unknown_20b660"),
    guest(0x0015_c150, "game_engine"),
    guest(0x0019_5ed0, "unknown_195ed0"),
    guest(0x0020_9310, "script_scheduler"),
    guest(0x0013_8c10, "unknown_138c10"),
    guest(0x000b_6f10, "objects_before_physics"),
    guest(0x001c_3d30, "havok_step"),
    guest(0x000b_7060, "objects_after_physics"),
    guest(0x000b_7150, "objects_final"),
    guest(0x0022_68e0, "unknown_2268e0"),
    guest(0x0017_6e40, "effects"),
    guest(0x000c_1400, "lights"),
    guest(0x0015_ca10, "post_game"),
    guest(0x0014_d8b0, "players_after"),
    guest(0x001d_f500, "unknown_1df500"),
    guest(0x0013_8e40, "unknown_138e40"),
    guest(0x0013_8eb0, "unknown_138eb0"),
    guest(0x0022_6050, "unknown_226050"),
    guest(0x0013_c680, "unknown_13c680"),
    guest(0x0012_3430, "unknown_123430"),
    guest(0x0022_5730, "unknown_225730"),
    guest(0x0016_5dc1, "first_person"),
    guest(0x0015_3950, "unknown_153950"),
    guest(0x0019_17b2, "hud"),
    guest(0x0016_f200, "unknown_16f200"),
];

/// Phases the guest still runs.
pub const fn guest_phases() -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < PHASES.len() {
        if matches!(PHASES[i].owner, Owner::Guest) {
            n += 1;
        }
        i += 1;
    }
    n
}

/// Guest calls made per phase, and the ticks they were made in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ledger {
    calls: [u32; PHASES.len()],
    ticks: u64,
    tick: u32,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    pub const fn new() -> Self {
        Ledger { calls: [0; PHASES.len()], ticks: 0, tick: 0 }
    }
    /// A guest call to phase `phase` (an index into [`PHASES`]).
    pub fn record(&mut self, phase: usize) {
        self.calls[phase] = self.calls[phase].saturating_add(1);
    }
    /// A tick ended; `tick` is the game time's count after it.
    pub fn finish_tick(&mut self, tick: u32) {
        self.ticks += 1;
        self.tick = tick;
    }
    /// Guest calls to phase `phase` since the start.
    pub fn calls(&self, phase: usize) -> u32 {
        self.calls[phase]
    }
    /// Ticks the driver has run.
    pub fn ticks(&self) -> u64 {
        self.ticks
    }
    /// The game tick count after the last tick the driver ran.
    pub fn tick(&self) -> u32 {
        self.tick
    }
    /// Guest calls over all phases.
    pub fn total_calls(&self) -> u64 {
        self.calls.iter().map(|&c| u64::from(c)).sum()
    }
}

/// The CRT routine that sets the x87 control word.
pub const CONTROL_WORD_FN: u32 = 0x0032_12d6;
/// `_controlfp(new, mask)`'s arguments for 53-bit precision, round to
/// nearest and every exception masked. The differential test checks them
/// against the lifted call.
pub const CONTROL_WORD_NEW: u32 = 0x0009_001f;
pub const CONTROL_WORD_MASK: u32 = 0x0003_831f;
/// The simulation update record's size.
pub const RECORD_SIZE: u32 = 0x4048;
/// Clears the record (`EAX` = record).
pub const RECORD_BUILD: u32 = 0x0006_82e0;
/// Applies the record (one stack argument, `ret 4`).
pub const RECORD_BEFORE: u32 = 0x0006_83a0;
/// Closes the record (`ESI` = record).
pub const RECORD_AFTER: u32 = 0x0006_84e0;
/// The tick count in the game-time record.
pub const TIME_TICK: u32 = 8;
/// The byte of the record that says the tick does not run its phases, when
/// the lifted body has one. Not recovered: the phases always run.
pub const RUN_FLAG: Option<u32> = None;

/// The tick body: what `game_tick` does, with `ledger` counting each guest
/// phase call. Returns as the original does (no value, no argument bytes).
/// The record is on the guest stack below the entry `ESP`, and every call
/// reserves it.
pub async fn run(cx: Cx, ledger: &'static Mutex<Ledger>) -> EntryResult {
    let record = cx.entry().esp.wrapping_sub(RECORD_SIZE);
    let call = |target: u32| GuestCall::new(target).reserve(RECORD_SIZE);
    cx.call(call(CONTROL_WORD_FN).args([CONTROL_WORD_NEW, CONTROL_WORD_MASK])).await?;
    cx.call(call(RECORD_BUILD).eax(record)).await?;
    cx.call(call(RECORD_BEFORE).args([record])).await?;
    let runs = match RUN_FLAG {
        Some(at) => cx.read(|m| m.u8(record.wrapping_add(at)))? != 0,
        None => true,
    };
    if runs {
        for (i, phase) in PHASES.iter().enumerate() {
            match phase.owner {
                Owner::Guest => {
                    ledger.lock().unwrap_or_else(|e| e.into_inner()).record(i);
                    cx.call(call(phase.entry)).await?;
                }
                Owner::Rust => {
                    return Err(EntryError::IsolatedCall(format!(
                        "tick phase {} is Rust-owned and has no body",
                        phase.name
                    )));
                }
            }
        }
    }
    cx.call(call(RECORD_AFTER).esi(record)).await?;
    let tick = cx.write(|m| {
        let time = m.u32(crate::points::GAME_TIME.base)?;
        let at = time.wrapping_add(TIME_TICK);
        let next = m.u32(at)?.wrapping_add(1);
        m.set_u32(at, next)?;
        Ok::<_, crate::memory::MemFault>(next)
    })?;
    ledger.lock().unwrap_or_else(|e| e.into_inner()).finish_tick(tick);
    Ok(Ret::new(0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn there_are_29_phases_each_entry_once() {
        assert_eq!(PHASES.len(), 29);
        let mut seen = std::collections::BTreeSet::new();
        assert!(PHASES.iter().all(|p| seen.insert(p.entry)));
        assert_eq!(PHASES[0].entry, 0x0014_d3a0);
        assert_eq!(PHASES[28].entry, 0x0016_f200);
    }

    #[test]
    fn the_ledger_counts_by_phase() {
        let mut l = Ledger::new();
        l.record(3);
        l.record(3);
        l.finish_tick(9);
        assert_eq!((l.calls(3), l.calls(4), l.ticks(), l.tick(), l.total_calls()), (2, 0, 1, 9, 2));
    }
}
