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

use guest_layout::entry::{Cx, EntryResult, GuestCall, Ret};

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
pub struct System {
    /// The guest function (a phase's guest entry is kept after a Rust body
    /// takes it over: it names the phase and is what the differential test
    /// compares against).
    pub entry: u32,
    pub name: &'static str,
    pub owner: Owner,
    pub form: Form,
}

const fn guest(entry: u32, name: &'static str) -> System {
    System { entry, name, owner: Owner::Guest, form: Form::EbxRecord }
}

const fn rust(entry: u32, name: &'static str) -> System {
    System { entry, name, owner: Owner::Rust, form: Form::EbxRecord }
}

const fn guest_first(entry: u32, name: &'static str) -> System {
    System { entry, name, owner: Owner::Guest, form: Form::StackRecordEcx }
}

/// The 29 phases in the order `game_tick` calls them
/// (docs/reviews/2026-11-20-sim.md, section 2.1). Phase 1 takes the record
/// as a stack argument with `ECX` = record; phases 2-29 take no argument and
/// see `EBX` = record ([`Form`]). Phases 26 (`0x165dc1`) and 28
/// (`0x1917b2`) are odd addresses; the differential test checks the whole
/// table against the lifted body.
pub const SYSTEMS: &[System] = &[
    guest_first(0x0014_d3a0, "players_before"),
    guest(0x0013_8db0, "input_random"),
    // The AI update, crate::ai_update; its seven systems stay guest.
    rust(0x001c_83c0, "ai"),
    guest(0x001f_b510, "pool_walk_4f5724"),
    guest(0x0020_b660, "periodic_sound_walk"),
    guest(0x0015_c150, "game_engine"),
    guest(0x0019_5ed0, "stats_update"),
    // The HaloScript scheduler, crate::hs_update; its threads stay guest.
    rust(0x0020_9310, "script_scheduler"),
    guest(0x0013_8c10, "input_record_apply"),
    guest(0x000b_6f10, "objects_before_physics"),
    guest(0x001c_3d30, "havok_step"),
    guest(0x000b_7060, "objects_after_physics"),
    guest(0x000b_7150, "objects_final"),
    guest(0x0022_68e0, "game_save_update"),
    // The effect walk, crate::effects_phase (docs/engine/effects-parts.md).
    rust(0x0017_6e40, "effects"),
    guest(0x000c_1400, "lights"),
    // The game engine's round countdown and end, crate::post_game.
    rust(0x0015_ca10, "post_game"),
    guest(0x0014_d8b0, "players_after"),
    guest(0x001d_f500, "state_4f55ec_update"),
    guest(0x0013_8e40, "game_over_check"),
    guest(0x0013_8eb0, "script_globals_step"),
    guest(0x0022_6050, "game_save_policy"),
    guest(0x0013_c680, "time_clock_13c680"),
    guest(0x0012_3430, "object_list_update"),
    guest(0x0022_5730, "net_flag_decay"),
    guest(0x0016_5dc1, "first_person"),
    guest(0x0015_3950, "screen_shake_players"),
    guest(0x0019_17b2, "hud"),
    // The countdown requests, crate::countdown_requests.
    rust(0x0016_f200, "countdown_requests"),
];

/// The tick rate, in ticks per second. The rate is a parameter of the
/// driver's clock, not a literal in a system: [`tick_length`] is the one
/// place it becomes a step in seconds. 30 for now (the owner's call), and
/// the game-time record's length field (`TIME_LENGTH`) is where retail
/// keeps the same number.
pub const TICK_RATE_HZ: u32 = 30;

/// One tick's length in seconds at `rate_hz`, as the f32 the game-time
/// record holds.
pub fn tick_length(rate_hz: u32) -> f32 {
    1.0 / rate_hz as f32
}

/// Phases the guest still runs.
pub const fn guest_phases() -> usize {
    let mut n = 0;
    let mut i = 0;
    while i < SYSTEMS.len() {
        if matches!(SYSTEMS[i].owner, Owner::Guest) {
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
    calls: [u32; SYSTEMS.len()],
    ticks: u64,
    tick: u32,
    phase_ns: [u64; SYSTEMS.len()],
    phase_last_ns: [u64; SYSTEMS.len()],
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
            calls: [0; SYSTEMS.len()],
            ticks: 0,
            tick: 0,
            phase_ns: [0; SYSTEMS.len()],
            phase_last_ns: [0; SYSTEMS.len()],
            tick_ns: 0,
            tick_last_ns: 0,
            loop_ns: 0,
            loop_last_ns: 0,
            loops: 0,
            last_entry: None,
        }
    }
    /// A guest call to phase `phase` (an index into [`SYSTEMS`]).
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
    pub fn finish_tick(&mut self, tick: u32, phase_ns: &[u64; SYSTEMS.len()], tick_ns: u64) {
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

/// The tick body in the lifted body's form: every phase a guest call,
/// Rust-owned ones included ([`Bodies::NONE`]), with `ledger` counting each.
/// Returns as the original does (no value, no argument bytes). The record
/// is on the guest stack below the entry `ESP`, and every call reserves it.
pub async fn run(cx: Cx, ledger: &'static Mutex<Ledger>) -> EntryResult {
    run_with(cx, ledger, Present::Always, Bodies::NONE).await
}

/// Whether the Rust-owned phases run their Rust body. A phase whose body
/// is off is a guest call, counted in the ledger like any guest phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Bodies {
    /// The Rust bodies ([`crate::ai_update`], [`crate::hs_update`], [`crate::effects_phase`], [`crate::post_game`],
    /// [`crate::countdown_requests`]); off only in tests, as the lifted
    /// reference.
    pub rust: bool,
}

impl Bodies {
    pub const NONE: Bodies = Bodies { rust: false };
    pub const ALL: Bodies = Bodies { rust: true };

    fn runs(self, phase: &System) -> bool {
        phase.owner == Owner::Rust && self.rust
    }
}

/// The effects phase's entry.
pub const EFFECTS: u32 = 0x0017_6e40;

/// Which seat-presentation phases a tick calls.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Present {
    /// Every phase, as retail does.
    Always,
    /// The presentation phases ([`is_presentation`]) only when a local user
    /// is mapped: with none, the guest bodies have no seat to work on and
    /// the tick does not call them. When they run, they draw from the
    /// presentation stream ([`PresentSeeds`]), so the sim's draws are the
    /// same with any number of views.
    LocalOnly,
}

/// The presentation's own copy of the two shared random seeds
/// (`[RANDOM_SEEDS]` `+0` and `+4`, docs/engine/sim-rewrite.md, "The tick
/// driver"). Around each presentation phase the driver puts these in the
/// guest's seed words and the sim's back after, so first person, the HUD
/// and the camera step this stream and never the sim's. It starts as the
/// sim's seeds at the first presentation phase it covers, lives for the
/// process, and is not saved: a view's draws are not sim state.
pub struct PresentSeeds(Mutex<Option<[u32; 2]>>);

impl PresentSeeds {
    pub const fn new() -> Self {
        PresentSeeds(Mutex::new(None))
    }

    /// The seeds the presentation draws next, `None` before its first phase.
    pub fn get(&self) -> Option<[u32; 2]> {
        *self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Before a presentation phase: the sim's seeds out, the stream's in.
    /// Returns the sim's, or `None` (counted) when the seed words do not read.
    fn swap_in(&self, m: &mut dyn guest_layout::memory::GuestMemory) -> Option<(u32, [u32; 2])> {
        let read = |m: &dyn guest_layout::memory::GuestMemory| -> Option<(u32, [u32; 2])> {
            let base = m.u32(guest_layout::points::RANDOM_SEEDS.base).ok().filter(|&b| b != 0)?;
            Some((base, [m.u32(base).ok()?, m.u32(base.wrapping_add(4)).ok()?]))
        };
        let Some((base, sim)) = read(m) else {
            UNISOLATED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            return None;
        };
        let present = self.get().unwrap_or(sim);
        m.set_u32(base, present[0]).ok()?;
        m.set_u32(base.wrapping_add(4), present[1]).ok()?;
        Some((base, sim))
    }

    /// After it: the stream's seeds kept, the sim's put back.
    fn swap_out(&self, m: &mut dyn guest_layout::memory::GuestMemory, (base, sim): (u32, [u32; 2])) {
        if let (Ok(a), Ok(b)) = (m.u32(base), m.u32(base.wrapping_add(4))) {
            *self.0.lock().unwrap_or_else(|e| e.into_inner()) = Some([a, b]);
        }
        let _ = m.set_u32(base, sim[0]);
        let _ = m.set_u32(base.wrapping_add(4), sim[1]);
    }
}

impl Default for PresentSeeds {
    fn default() -> Self {
        Self::new()
    }
}

/// The driver's presentation stream.
pub static PRESENT_SEEDS: PresentSeeds = PresentSeeds::new();

/// Presentation phases that ran on the sim's seeds because the seed words
/// did not read (no game loaded): the one way a view's draws can reach the
/// sim stream.
static UNISOLATED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// [`UNISOLATED`]'s count since start.
pub fn unisolated() -> u64 {
    UNISOLATED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Phases 26, 28 and 29: first person, HUD and the countdown requests
/// ([`crate::countdown_requests`]).
pub fn is_presentation(phase: usize) -> bool {
    matches!(SYSTEMS[phase].entry, 0x0016_5dc1 | 0x0019_17b2 | 0x0016_f200)
}

/// The players' globals pointer; `+0xc` holds four local users' player
/// indices, `u32::MAX` for an unmapped user.
pub const LOCAL_USERS: u32 = 0x004e_8c20;
pub const LOCAL_USER_SLOTS: u32 = 0xc;
pub const LOCAL_USER_COUNT: u32 = 4;

/// Mapped local users now.
pub fn local_users(m: &dyn guest_layout::memory::GuestMemory) -> u32 {
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

/// [`run`] with a presentation mode and the Rust phase bodies on.
pub async fn run_with(cx: Cx, ledger: &'static Mutex<Ledger>, present: Present, bodies: Bodies) -> EntryResult {
    let entered = Instant::now();
    // Counts made until the tick returns are tick-time (render_census).
    let _scope = crate::render_census::TickScope::enter();
    ledger.lock().unwrap_or_else(|e| e.into_inner()).begin_tick(entered);
    // The tick's first row: the last tick's events go (`events_clear`).
    crate::sim_objects::damage::tick_starts();
    let record = cx.entry().esp.wrapping_sub(RECORD_SIZE);
    let call = |target: u32| GuestCall::new(target).reserve(RECORD_SIZE);
    cx.call(call(CONTROL_WORD_FN).args([CONTROL_WORD_NEW, CONTROL_WORD_MASK])).await?;
    cx.call(call(RECORD_BUILD).eax(record)).await?;
    cx.call(call(RECORD_BEFORE).args([record])).await?;
    let flag_at = record.wrapping_add(RUN_FLAG);
    let runs = cx.read(|m| m.u8(flag_at))? != 0;
    let mut phase_ns = [0u64; SYSTEMS.len()];
    if runs {
        // Each phase's time runs from the previous clock read to the one
        // after its call.
        let mut mark = Instant::now();
        let views = match present {
            Present::Always => LOCAL_USER_COUNT,
            Present::LocalOnly => cx.read(|m| local_users(m)),
        };
        for (i, phase) in SYSTEMS.iter().enumerate() {
            if views == 0 && is_presentation(i) {
                continue;
            }
            let guest = !bodies.runs(phase)
                || match phase.entry {
                    EFFECTS => {
                        crate::effects_phase::run(&cx, RECORD_SIZE).await?;
                        false
                    }
                    crate::ai_update::ENTRY => {
                        crate::ai_update::run(&cx, RECORD_SIZE).await?;
                        false
                    }
                    crate::hs_update::ENTRY => {
                        crate::hs_update::run(&cx, RECORD_SIZE).await?;
                        false
                    }
                    crate::post_game::ENTRY => {
                        crate::post_game::run(&cx, RECORD_SIZE).await?;
                        false
                    }
                    crate::countdown_requests::ENTRY => {
                        cx.write(|m| crate::countdown_requests::run(m))?;
                        false
                    }
                    _ => true,
                };
            if guest {
                ledger.lock().unwrap_or_else(|e| e.into_inner()).record(i);
                let c = call(phase.entry);
                let c = match phase.form {
                    Form::StackRecordEcx => c.args([record]).ecx(record),
                    Form::EbxRecord => c.ebx(record),
                };
                let isolate = present == Present::LocalOnly && is_presentation(i);
                let sim = if isolate { cx.write(|m| PRESENT_SEEDS.swap_in(m)) } else { None };
                cx.call(c).await?;
                if let Some(sim) = sim {
                    cx.write(|m| PRESENT_SEEDS.swap_out(m, sim));
                }
            }
            let now = Instant::now();
            phase_ns[i] = ns_between(mark, now);
            mark = now;
        }
        // After the phases, still in the run branch (0x1380b4..0x1380db):
        // the counter falls to zero, the clock gains the tick length.
        cx.write(|m| {
            let clock = m.u32(guest_layout::points::TICK_CLOCK.base)?;
            let counter = clock.wrapping_add(CLOCK_COUNTER);
            let left = m.u32(counter)?.saturating_sub(1);
            m.set_u32(counter, left)?;
            let length = m.f32(m.u32(guest_layout::points::GAME_TIME.base)?.wrapping_add(TIME_LENGTH))?;
            let at = clock.wrapping_add(CLOCK_TIME);
            let now = m.f32(at)?;
            m.set_f32(at, now + length)?;
            Ok::<_, guest_layout::memory::MemFault>(())
        })?;
    }
    cx.call(call(RECORD_AFTER).esi(record)).await?;
    // The close may clear the flag; the tick counts only if it is still set.
    let advance = cx.read(|m| m.u8(flag_at))? != 0;
    let tick = cx.write(|m| {
        let time = m.u32(guest_layout::points::GAME_TIME.base)?;
        let at = time.wrapping_add(TIME_TICK);
        let now = m.u32(at)?;
        if !advance {
            return Ok::<_, guest_layout::memory::MemFault>(now);
        }
        let next = now.wrapping_add(1);
        m.set_u32(at, next)?;
        Ok::<_, guest_layout::memory::MemFault>(next)
    })?;
    // The census: bodies still carrying the dirty-pose bit as the tick ends.
    if let Some(scan) = cx.read(|m| crate::render_census::scan_poses(m)) {
        crate::render_census::record_scan(scan);
    }
    let tick_ns = ns_between(entered, Instant::now());
    ledger.lock().unwrap_or_else(|e| e.into_inner()).finish_tick(tick, &phase_ns, tick_ns);
    Ok(Ret::new(0, 0))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The row order is retail's (docs/reviews/2026-11-20-sim.md, section
    /// 2.1): the names, in order, are pinned.
    #[test]
    fn the_rows_are_retail_order() {
        let names: Vec<&str> = SYSTEMS.iter().map(|p| p.name).collect();
        assert_eq!(
            names,
            [
                "players_before",
                "input_random",
                "ai",
                "pool_walk_4f5724",
                "periodic_sound_walk",
                "game_engine",
                "stats_update",
                "script_scheduler",
                "input_record_apply",
                "objects_before_physics",
                "havok_step",
                "objects_after_physics",
                "objects_final",
                "game_save_update",
                "effects",
                "lights",
                "post_game",
                "players_after",
                "state_4f55ec_update",
                "game_over_check",
                "script_globals_step",
                "game_save_policy",
                "time_clock_13c680",
                "object_list_update",
                "net_flag_decay",
                "first_person",
                "screen_shake_players",
                "hud",
                "countdown_requests",
            ]
        );
        assert!(SYSTEMS.iter().all(|p| !p.name.starts_with("unknown_")));
    }

    #[test]
    fn the_tick_rate_is_a_parameter() {
        assert_eq!(TICK_RATE_HZ, 30);
        assert_eq!(tick_length(TICK_RATE_HZ), 1.0 / 30.0);
        assert_eq!(tick_length(60), 1.0 / 60.0);
    }

    #[test]
    fn there_are_29_phases_each_entry_once() {
        assert_eq!(SYSTEMS.len(), 29);
        let mut seen = std::collections::BTreeSet::new();
        assert!(SYSTEMS.iter().all(|p| seen.insert(p.entry)));
        assert_eq!(SYSTEMS[0].entry, 0x0014_d3a0);
        assert_eq!(SYSTEMS[28].entry, 0x0016_f200);
    }

    /// With the Rust bodies on, the AI update (disabled), the scheduler
    /// (scripts off), the effects phase and the countdown requests make no
    /// guest call of their own: the ledger has 0 for them
    /// and 1 for every other phase, and the walk's callees run where the
    /// phase did.
    #[test]
    fn effects_phase_ledger_is_zero() {
        use crate::effects_phase::tests::{STACK, answer, guest};
        use guest_layout::entry::{self, EntryFuture, Regs};
        let mut g = guest();
        let clock = 0x0052_0100;
        g.map(guest_layout::points::TICK_CLOCK.base & !0xfff, 0x1000);
        g.put_u32(guest_layout::points::TICK_CLOCK.base, clock).put_u32(clock + CLOCK_COUNTER, 5);
        // The countdown requests' bytes, none pending; scripts off; AI
        // disabled (its globals at a zero page).
        g.map(crate::countdown_requests::FIRST, 2).map(crate::hs_update::ENABLED, 1);
        g.map(crate::ai_update::GLOBALS, 4).map(AI_GLOBALS, 4).put_u32(crate::ai_update::GLOBALS, AI_GLOBALS);
        // No game engine: post_game's globals at a zero page, table slot 0.
        g.map(crate::post_game::GAME, 4).map(GAME_GLOBALS, 0x1000).put_u32(crate::post_game::GAME, GAME_GLOBALS);
        g.map(crate::post_game::ENGINES, 4);
        let ledger: &'static Mutex<Ledger> = Box::leak(Box::new(Mutex::new(Ledger::new())));
        let f = move |cx: Cx| -> EntryFuture { Box::pin(run_with(cx, ledger, Present::Always, Bodies::ALL)) };
        let record = STACK - RECORD_SIZE;
        let run = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |c, m| {
            if c.target == RECORD_BEFORE {
                m.set_u8(record + RUN_FLAG, 1).unwrap();
            }
            answer(c, m)
        });
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        let l = ledger.lock().unwrap();
        let effects = SYSTEMS.iter().position(|p| p.entry == EFFECTS).unwrap();
        assert_eq!(SYSTEMS[effects].owner, Owner::Rust);
        for i in 0..SYSTEMS.len() {
            assert_eq!(l.calls(i), u32::from(SYSTEMS[i].owner == Owner::Guest), "{}", SYSTEMS[i].name);
        }
        assert_eq!(SYSTEMS[28].owner, Owner::Rust, "the countdown requests");
        assert!(!targets_of(&run).contains(&crate::countdown_requests::ENTRY));
        assert!(!targets_of(&run).contains(&crate::hs_update::ENTRY));
        assert!(!targets_of(&run).contains(&crate::ai_update::ENTRY));
        assert!(!targets_of(&run).contains(&crate::post_game::ENTRY));
        assert_eq!(l.total_calls(), guest_phases() as u64);
        let targets: Vec<u32> = run.calls.iter().map(|c| c.target).collect();
        assert!(!targets.contains(&EFFECTS));
        // The walk runs between the phases either side of it.
        let at = targets.iter().position(|&t| t == sim_effects::walk::ALIVE).unwrap();
        assert_eq!(targets[at - 1], SYSTEMS[effects - 1].entry);
        let after = targets.iter().position(|&t| t == SYSTEMS[effects + 1].entry).unwrap();
        assert_eq!(after - at, 9, "the walk's nine callees");
    }

    /// A zero page for the AI globals in the driver tests.
    const AI_GLOBALS: u32 = 0x0057_0000;
    /// A zero page for the game globals in the driver tests.
    const GAME_GLOBALS: u32 = 0x0057_1000;

    fn targets_of(run: &guest_layout::entry::testing::Run) -> Vec<u32> {
        run.calls.iter().map(|c| c.target).collect()
    }

    #[test]
    fn the_ledger_counts_by_phase() {
        let mut l = Ledger::new();
        l.record(3);
        l.record(3);
        l.finish_tick(9, &[0; SYSTEMS.len()], 0);
        assert_eq!((l.calls(3), l.calls(4), l.ticks(), l.tick(), l.total_calls()), (2, 0, 1, 9, 2));
    }

    #[test]
    fn the_ledger_totals_phase_time_and_keeps_the_last_tick() {
        let mut l = Ledger::new();
        let mut a = [0u64; SYSTEMS.len()];
        a[2] = 30;
        a[5] = 20;
        let mut b = [0u64; SYSTEMS.len()];
        b[2] = 10;
        l.finish_tick(1, &a, 60);
        l.finish_tick(2, &b, 15);
        assert_eq!((l.phase_ns(2), l.phase_ns(5), l.phase_last_ns(2), l.phase_last_ns(5)), (40, 20, 10, 0));
        assert_eq!((l.tick_ns(), l.tick_last_ns()), (75, 15));
        let sum: u64 = (0..SYSTEMS.len()).map(|i| l.phase_ns(i)).sum();
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
