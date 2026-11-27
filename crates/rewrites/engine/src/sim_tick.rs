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
use std::time::Instant;

use crate::entry::{Cx, EntryError, EntryResult, GuestCall, Ret};

/// Who runs a phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    /// The lifted original, called with a guest call.
    Guest,
    /// A Rust body in this driver.
    Rust,
}

/// How a phase is called. Retail sets the registers once for the run, so
/// every phase after the first sees `EBX` = record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Form {
    /// Phase 1 (`0x14d3a0`): the record as stack argument 0 (`ret 4`) and
    /// `ECX` = record.
    StackRecordEcx,
    /// Phases 2-29: no stack argument, `EBX` = record (`0x138db0` reads it).
    EbxRecord,
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
    pub form: Form,
}

const fn guest(entry: u32, name: &'static str) -> Phase {
    Phase { entry, name, owner: Owner::Guest, form: Form::EbxRecord }
}

const fn guest_first(entry: u32, name: &'static str) -> Phase {
    Phase { entry, name, owner: Owner::Guest, form: Form::StackRecordEcx }
}

/// The 29 phases in the order `game_tick` calls them
/// (docs/reviews/2026-11-20-sim.md, section 2.1). Phase 1 takes the record
/// as a stack argument with `ECX` = record; phases 2-29 take no argument and
/// see `EBX` = record ([`Form`]). Phases 26 (`0x165dc1`) and 28
/// (`0x1917b2`) are odd addresses; the differential test checks the whole
/// table against the lifted body.
pub const PHASES: &[Phase] = &[
    guest_first(0x0014_d3a0, "players_before"),
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

/// Guest calls and time per phase, and the ticks they were made in.
///
/// Time is nanoseconds on the monotonic clock ([`Instant`]), kept as a
/// running total and as the last tick's. The driver reads the clock once
/// after each phase (the phase's time runs from the previous read), once
/// when the phases start and once at each tick's entry and exit, and
/// nothing else touches it: a ledger no one reads costs those reads.
/// Phase times are consecutive spans inside the tick, so the last tick's
/// phase times sum to at most [`Ledger::tick_last_ns`]. A tick that skips
/// its phases (the run flag clear) leaves the last tick's phase times 0.
///
/// The loop time is the span from one tick's entry to the next's: the
/// tick plus everything the game thread does between ticks, sound
/// included, which runs outside the 29 phases. A pause, a menu or a
/// stalled thread lengthens it, so read it over a window of play.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Ledger {
    calls: [u32; PHASES.len()],
    ticks: u64,
    tick: u32,
    phase_ns: [u64; PHASES.len()],
    phase_last_ns: [u64; PHASES.len()],
    tick_ns: u64,
    tick_last_ns: u64,
    loop_ns: u64,
    loop_last_ns: u64,
    loops: u64,
    last_entry: Option<Instant>,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    pub const fn new() -> Self {
        Ledger {
            calls: [0; PHASES.len()],
            ticks: 0,
            tick: 0,
            phase_ns: [0; PHASES.len()],
            phase_last_ns: [0; PHASES.len()],
            tick_ns: 0,
            tick_last_ns: 0,
            loop_ns: 0,
            loop_last_ns: 0,
            loops: 0,
            last_entry: None,
        }
    }
    /// A guest call to phase `phase` (an index into [`PHASES`]).
    pub fn record(&mut self, phase: usize) {
        self.calls[phase] = self.calls[phase].saturating_add(1);
    }
    /// A tick is entered at `now`: the span since the last entry is a loop.
    pub fn begin_tick(&mut self, now: Instant) {
        if let Some(prev) = self.last_entry {
            let ns = ns_between(prev, now);
            self.loop_last_ns = ns;
            self.loop_ns = self.loop_ns.saturating_add(ns);
            self.loops += 1;
        }
        self.last_entry = Some(now);
    }
    /// A tick ended; `tick` is the game time's count after it (unchanged
    /// when the record's run flag was clear at the close). `phase_ns` is
    /// each phase's time this tick (0 for a phase not run) and `tick_ns`
    /// the tick's, entry to exit.
    pub fn finish_tick(&mut self, tick: u32, phase_ns: &[u64; PHASES.len()], tick_ns: u64) {
        self.ticks += 1;
        self.tick = tick;
        self.phase_last_ns = *phase_ns;
        self.tick_last_ns = tick_ns;
        self.tick_ns = self.tick_ns.saturating_add(tick_ns);
        for (total, ns) in self.phase_ns.iter_mut().zip(phase_ns) {
            *total = total.saturating_add(*ns);
        }
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
    /// Nanoseconds in phase `phase` over every tick.
    pub fn phase_ns(&self, phase: usize) -> u64 {
        self.phase_ns[phase]
    }
    /// Nanoseconds in phase `phase` in the last tick.
    pub fn phase_last_ns(&self, phase: usize) -> u64 {
        self.phase_last_ns[phase]
    }
    /// Nanoseconds in the tick body over every tick.
    pub fn tick_ns(&self) -> u64 {
        self.tick_ns
    }
    /// Nanoseconds in the last tick body.
    pub fn tick_last_ns(&self) -> u64 {
        self.tick_last_ns
    }
    /// Nanoseconds from tick entry to the next tick's entry, summed over
    /// [`Ledger::loops`] spans.
    pub fn loop_ns(&self) -> u64 {
        self.loop_ns
    }
    /// The last such span.
    pub fn loop_last_ns(&self) -> u64 {
        self.loop_last_ns
    }
    /// Entry-to-entry spans measured (one fewer than the ticks entered).
    pub fn loops(&self) -> u64 {
        self.loops
    }
}

fn ns_between(from: Instant, to: Instant) -> u64 {
    u64::try_from(to.saturating_duration_since(from).as_nanos()).unwrap_or(u64::MAX)
}

/// The CRT routine that sets the x87 control word.
pub const CONTROL_WORD_FN: u32 = 0x0032_12d6;
/// `_controlfp(new, mask)`'s arguments for 53-bit precision, round to
/// nearest and every exception masked. The differential test checks them
/// against the lifted call.
pub const CONTROL_WORD_NEW: u32 = 0x0009_001f;
pub const CONTROL_WORD_MASK: u32 = 0x000f_ffff;
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
/// The record byte (a `u8`) that says the tick runs its phases. Read after
/// the record's before step (it gates the phases and the clock step) and
/// again after the close (it gates the tick count).
pub const RUN_FLAG: u32 = 4;
/// The tick clock's f32 field (`[TICK_CLOCK] + 4`), which gains the tick length.
pub const CLOCK_TIME: u32 = 4;
/// The tick clock's counter (`[TICK_CLOCK] + 0x14`), which falls to zero.
pub const CLOCK_COUNTER: u32 = 0x14;
/// The tick length in the game-time record (f32).
pub const TIME_LENGTH: u32 = 4;

/// The tick body: what `game_tick` does, with `ledger` counting each guest
/// phase call. Returns as the original does (no value, no argument bytes).
/// The record is on the guest stack below the entry `ESP`, and every call
/// reserves it.
pub async fn run(cx: Cx, ledger: &'static Mutex<Ledger>) -> EntryResult {
    run_with(cx, ledger, Present::Always).await
}

/// Which seat-presentation phases a tick calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Present {
    /// Every phase, as retail does.
    Always,
    /// The presentation phases ([`is_presentation`]) only when a local user
    /// is mapped: with none, the guest bodies have no seat to work on and
    /// the tick does not call them.
    LocalOnly,
}

/// Phases 26, 28 and 29: first person, HUD and the camera neighbourhood.
pub fn is_presentation(phase: usize) -> bool {
    matches!(PHASES[phase].entry, 0x0016_5dc1 | 0x0019_17b2 | 0x0016_f200)
}

/// The players' globals pointer; `+0xc` holds four local users' player
/// indices, `u32::MAX` for an unmapped user.
pub const LOCAL_USERS: u32 = 0x004e_8c20;
pub const LOCAL_USER_SLOTS: u32 = 0xc;
pub const LOCAL_USER_COUNT: u32 = 4;

/// Mapped local users now.
pub fn local_users(m: &dyn crate::memory::GuestMemory) -> u32 {
    let Ok(base) = m.u32(LOCAL_USERS) else { return LOCAL_USER_COUNT };
    let mut n = 0;
    for u in 0..LOCAL_USER_COUNT {
        match m.u32(base.wrapping_add(LOCAL_USER_SLOTS + 4 * u)) {
            Ok(u32::MAX) => {}
            // An unreadable slot counts as a user: the phase runs.
            _ => n += 1,
        }
    }
    n
}

/// [`run`] with a presentation mode.
pub async fn run_with(cx: Cx, ledger: &'static Mutex<Ledger>, present: Present) -> EntryResult {
    let entered = Instant::now();
    ledger.lock().unwrap_or_else(|e| e.into_inner()).begin_tick(entered);
    let record = cx.entry().esp.wrapping_sub(RECORD_SIZE);
    let call = |target: u32| GuestCall::new(target).reserve(RECORD_SIZE);
    cx.call(call(CONTROL_WORD_FN).args([CONTROL_WORD_NEW, CONTROL_WORD_MASK])).await?;
    cx.call(call(RECORD_BUILD).eax(record)).await?;
    cx.call(call(RECORD_BEFORE).args([record])).await?;
    let flag_at = record.wrapping_add(RUN_FLAG);
    let runs = cx.read(|m| m.u8(flag_at))? != 0;
    let mut phase_ns = [0u64; PHASES.len()];
    if runs {
        // Each phase's time runs from the previous clock read to the one
        // after its call.
        let mut mark = Instant::now();
        let views = match present {
            Present::Always => LOCAL_USER_COUNT,
            Present::LocalOnly => cx.read(|m| local_users(m)),
        };
        for (i, phase) in PHASES.iter().enumerate() {
            if views == 0 && is_presentation(i) {
                continue;
            }
            match phase.owner {
                Owner::Guest => {
                    ledger.lock().unwrap_or_else(|e| e.into_inner()).record(i);
                    let c = call(phase.entry);
                    let c = match phase.form {
                        Form::StackRecordEcx => c.args([record]).ecx(record),
                        Form::EbxRecord => c.ebx(record),
                    };
                    cx.call(c).await?;
                    let now = Instant::now();
                    phase_ns[i] = ns_between(mark, now);
                    mark = now;
                }
                Owner::Rust => {
                    return Err(EntryError::IsolatedCall(format!(
                        "tick phase {} is Rust-owned and has no body",
                        phase.name
                    )));
                }
            }
        }
        // After the phases, still in the run branch (0x1380b4..0x1380db):
        // the counter falls to zero, the clock gains the tick length.
        cx.write(|m| {
            let clock = m.u32(crate::points::TICK_CLOCK.base)?;
            let counter = clock.wrapping_add(CLOCK_COUNTER);
            let left = m.u32(counter)?.saturating_sub(1);
            m.set_u32(counter, left)?;
            let length = m.f32(m.u32(crate::points::GAME_TIME.base)?.wrapping_add(TIME_LENGTH))?;
            let at = clock.wrapping_add(CLOCK_TIME);
            let now = m.f32(at)?;
            m.set_f32(at, now + length)?;
            Ok::<_, crate::memory::MemFault>(())
        })?;
    }
    cx.call(call(RECORD_AFTER).esi(record)).await?;
    // The close may clear the flag; the tick counts only if it is still set.
    let advance = cx.read(|m| m.u8(flag_at))? != 0;
    let tick = cx.write(|m| {
        let time = m.u32(crate::points::GAME_TIME.base)?;
        let at = time.wrapping_add(TIME_TICK);
        let now = m.u32(at)?;
        if !advance {
            return Ok::<_, crate::memory::MemFault>(now);
        }
        let next = now.wrapping_add(1);
        m.set_u32(at, next)?;
        Ok::<_, crate::memory::MemFault>(next)
    })?;
    let tick_ns = ns_between(entered, Instant::now());
    ledger.lock().unwrap_or_else(|e| e.into_inner()).finish_tick(tick, &phase_ns, tick_ns);
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
        l.finish_tick(9, &[0; PHASES.len()], 0);
        assert_eq!((l.calls(3), l.calls(4), l.ticks(), l.tick(), l.total_calls()), (2, 0, 1, 9, 2));
    }

    #[test]
    fn the_ledger_totals_phase_time_and_keeps_the_last_tick() {
        let mut l = Ledger::new();
        let mut a = [0u64; PHASES.len()];
        a[2] = 30;
        a[5] = 20;
        let mut b = [0u64; PHASES.len()];
        b[2] = 10;
        l.finish_tick(1, &a, 60);
        l.finish_tick(2, &b, 15);
        assert_eq!((l.phase_ns(2), l.phase_ns(5), l.phase_last_ns(2), l.phase_last_ns(5)), (40, 20, 10, 0));
        assert_eq!((l.tick_ns(), l.tick_last_ns()), (75, 15));
        let sum: u64 = (0..PHASES.len()).map(|i| l.phase_ns(i)).sum();
        assert!(sum <= l.tick_ns());
    }

    #[test]
    fn the_loop_is_entry_to_entry() {
        let mut l = Ledger::new();
        let t = Instant::now();
        l.begin_tick(t);
        assert_eq!((l.loops(), l.loop_ns()), (0, 0), "the first entry starts the span");
        l.begin_tick(t + std::time::Duration::from_millis(16));
        l.begin_tick(t + std::time::Duration::from_millis(48));
        assert_eq!((l.loops(), l.loop_last_ns(), l.loop_ns()), (2, 32_000_000, 48_000_000));
    }
}
