//! Tick phase 3, `ai_update` `0x001c83c0` (`AI_UPDATE`), in Rust
//! (docs/engine/sim-rewrite.md, "AI: the port plan"). The phase's own body
//! is Rust; the seven AI systems it calls are still the lifted functions,
//! one guest call each, counted in [`Stats::calls`].
//!
//! The guest body, whole (no stack frame, no register inputs to its
//! callees):
//!
//! | Step | Guest | Calls |
//! |---|---|---|
//! | 1 | AI globals `g = [0x4f55d0]`; `g + 1` clear: return | |
//! | 2 | the game engine globals `[0x4e6948]`, byte `+0xc` 4 (a client): return | |
//! | 3 | `g + 0` clear: if `g + 2` is set, the teardown, then `g + 2` = 0; return | `1e3240` |
//! | 4 | the first system, then the countdown `(i16) g + 0x22` falls by 1 while positive | `1c81c0` |
//! | 5 | the other five systems in order, then `g + 2` = 1 | `201c80`, `268510`, `1e2f50`, `20fb30`, `293830` |
//!
//! `g` is read again after each call that can change it, as the guest
//! does. A read that faults is the guest's own: the lifted body faults on
//! the same read.

use crate::entry::{Cx, EntryError, GuestCall};
use crate::memory::{GuestMemory, MemFault};
use std::sync::Mutex;

/// The phase's guest entry.
pub const ENTRY: u32 = 0x001c_83c0;
/// The AI globals pointer.
pub const GLOBALS: u32 = 0x004f_55d0;
/// The game engine globals pointer; byte `+0xc` is the machine's mode.
pub const ENGINE: u32 = 0x004e_6948;
/// The client mode: no AI runs here.
const CLIENT: u8 = 4;
/// Byte `+0`: AI on for this map.
const ON: u32 = 0;
/// Byte `+1`: the AI update is enabled.
const ENABLED: u32 = 1;
/// Byte `+2`: the systems ran last tick (the teardown runs once after).
const RAN: u32 = 2;
/// `i16` `+0x22`: a countdown the update spends a tick of.
const COUNTDOWN: u32 = 0x22;

/// The teardown when AI goes off.
pub const TEARDOWN: u32 = 0x001e_3240;
/// The systems, in the order the guest calls them.
pub const SYSTEMS: [u32; 6] = [0x001c_81c0, 0x0020_1c80, 0x0026_8510, 0x001e_2f50, 0x0020_fb30, 0x0029_3830];

/// What one tick's update does, decided from the globals.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Plan {
    /// Disabled, or a client: nothing.
    Idle,
    /// AI is off: the teardown, if the systems ran last tick.
    Off { teardown: bool },
    /// The systems run.
    Run,
}

/// Steps 1 to 3's reads.
pub fn plan(m: &dyn GuestMemory) -> Result<Plan, MemFault> {
    let g = m.u32(GLOBALS)?;
    if m.u8(g.wrapping_add(ENABLED))? == 0 {
        return Ok(Plan::Idle);
    }
    if m.u8(m.u32(ENGINE)?.wrapping_add(0xc))? == CLIENT {
        return Ok(Plan::Idle);
    }
    if m.u8(g.wrapping_add(ON))? == 0 {
        return Ok(Plan::Off { teardown: m.u8(g.wrapping_add(RAN))? != 0 });
    }
    Ok(Plan::Run)
}

/// Step 4's countdown, after the first system.
pub fn spend_countdown(m: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let at = m.u32(GLOBALS)?.wrapping_add(COUNTDOWN);
    let left = m.u16(at)? as i16;
    if left > 0 {
        m.set_u16(at, (left - 1) as u16)?;
    }
    Ok(())
}

fn set_ran(m: &mut dyn GuestMemory, ran: bool) -> Result<(), MemFault> {
    let g = m.u32(GLOBALS)?;
    m.set_u8(g.wrapping_add(RAN), u8::from(ran))
}

/// The phase's counters since start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stats {
    /// Ticks the systems ran.
    pub runs: u64,
    /// Teardowns.
    pub teardowns: u64,
    /// Guest calls into AI systems not ported yet.
    pub calls: u64,
}

static STATS: Mutex<Stats> = Mutex::new(Stats { runs: 0, teardowns: 0, calls: 0 });

pub fn stats() -> Stats {
    *STATS.lock().unwrap_or_else(|e| e.into_inner())
}

fn count(f: impl FnOnce(&mut Stats)) {
    f(&mut STATS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The phase. `reserve` is the stack the driver keeps below its entry.
pub async fn run(cx: &Cx, reserve: u32) -> Result<(), EntryError> {
    let call = |target: u32| GuestCall::new(target).reserve(reserve);
    match cx.read(|m| plan(m))? {
        Plan::Idle | Plan::Off { teardown: false } => {}
        Plan::Off { teardown: true } => {
            count(|s| {
                s.teardowns += 1;
                s.calls += 1;
            });
            cx.call(call(TEARDOWN)).await?;
            cx.write(|m| set_ran(m, false))?;
        }
        Plan::Run => {
            count(|s| s.runs += 1);
            for (i, &system) in SYSTEMS.iter().enumerate() {
                count(|s| s.calls += 1);
                cx.call(call(system)).await?;
                if i == 0 {
                    cx.write(|m| spend_countdown(m))?;
                }
            }
            cx.write(|m| set_ran(m, true))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{self, EntryFuture, Regs, Ret};
    use crate::testing::FakeGuest;

    const STACK: u32 = 0x0030_0000;
    const G: u32 = 0x0060_0000;
    const MODE: u32 = 0x0061_0000;

    fn guest(on: u8, enabled: u8, ran: u8, mode: u8, countdown: i16) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(STACK - 0x8000, 0x10000).map(G, 0x100).map(MODE, 0x100).map(GLOBALS, 4).map(ENGINE, 4);
        g.put_u32(GLOBALS, G).put_u32(ENGINE, MODE).put_u8(MODE + 0xc, mode);
        g.put_u8(G + ON, on).put_u8(G + ENABLED, enabled).put_u8(G + RAN, ran).put_u16(G + COUNTDOWN, countdown as u16);
        g
    }

    fn tick(g: FakeGuest) -> entry::testing::Run {
        let f = |cx: Cx| -> EntryFuture {
            Box::pin(async move {
                run(&cx, 0x40).await?;
                Ok(Ret::new(0, 0))
            })
        };
        entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, |_, _| Regs::default())
    }

    fn targets(r: &entry::testing::Run) -> Vec<u32> {
        r.calls.iter().map(|c| c.target).collect()
    }

    #[test]
    fn the_systems_run_in_order_and_the_countdown_falls() {
        let r = tick(guest(1, 1, 0, 1, 3));
        assert!(r.result.is_ok());
        assert_eq!(targets(&r), SYSTEMS);
        assert_eq!((r.guest.u16(G + COUNTDOWN).unwrap(), r.guest.u8(G + RAN).unwrap()), (2, 1));
    }

    #[test]
    fn a_spent_countdown_stays() {
        for left in [0i16, -5] {
            let r = tick(guest(1, 1, 1, 1, left));
            assert_eq!(r.guest.u16(G + COUNTDOWN).unwrap() as i16, left);
        }
    }

    #[test]
    fn disabled_or_a_client_does_nothing() {
        for g in [guest(1, 0, 1, 1, 3), guest(1, 1, 1, CLIENT, 3)] {
            let r = tick(g);
            assert!(r.calls.is_empty());
            assert_eq!((r.guest.u16(G + COUNTDOWN).unwrap(), r.guest.u8(G + RAN).unwrap()), (3, 1));
        }
    }

    #[test]
    fn ai_off_tears_down_once() {
        let r = tick(guest(0, 1, 1, 1, 3));
        assert_eq!(targets(&r), [TEARDOWN]);
        assert_eq!(r.guest.u8(G + RAN).unwrap(), 0);
        let r = tick(r.guest);
        assert!(r.calls.is_empty(), "the teardown ran last tick");
    }
}
