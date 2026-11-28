//! The effects phase of the tick (`0x176e40`) from Rust
//! (docs/engine/effects-parts.md): [`sim_effects::walk`] over guest
//! memory, with the four per-effect callees as guest calls in retail
//! order. The tick driver runs it in the phase's place, so `sim.ledger`
//! counts no guest call for the phase.
//!
//! What the body leaves equal to retail: the callees, their order, their
//! arguments and the registers they read (`EBX` = handle for every call,
//! `EAX` = handle for the delete); the tick length read once at the start.
//! What differs: the phase's own frame (five saved registers and the tick
//! length) is not written below the driver's stack, and the callees'
//! return addresses are the driver's.

use std::sync::Mutex;

use sim_effects::walk::{self, Stats};

use guest_layout::entry::{Cx, EntryError, GuestCall};
use guest_layout::memory::{GuestMemory, MemFault};

/// The walk's counters, over every phase this process ran.
pub static STATS: Mutex<Stats> = Mutex::new(Stats {
    phases: 0,
    visited: 0,
    dead: 0,
    culled: 0,
    retired: 0,
    updated: 0,
});

/// A copy of the counters now.
pub fn stats() -> Stats {
    *STATS.lock().unwrap_or_else(|e| e.into_inner())
}

/// The stack the phase's callees see below the caller's reserve: retail's
/// return address, its four saved registers and the tick length.
pub const FRAME: u32 = 0x18;

/// The tick length: the game-time record's `+4`, as bits.
pub const TIME_LENGTH: u32 = 4;

struct View<'a>(&'a dyn GuestMemory);

impl walk::Mem for View<'_> {
    type Fault = MemFault;
    fn u8(&self, at: u32) -> Result<u8, MemFault> {
        self.0.u8(at)
    }
    fn u16(&self, at: u32) -> Result<u16, MemFault> {
        self.0.u16(at)
    }
    fn u32(&self, at: u32) -> Result<u32, MemFault> {
        self.0.u32(at)
    }
}

fn count(f: impl FnOnce(&mut Stats)) {
    f(&mut STATS.lock().unwrap_or_else(|e| e.into_inner()))
}

/// The phase. `reserve` is the stack the driver keeps below its entry (the
/// update record); the callees go below it and [`FRAME`]. An array or game
/// time that cannot be read is the guest's fault, raised.
pub async fn run(cx: &Cx, reserve: u32) -> Result<(), EntryError> {
    let start = cx.read(|m| -> Result<_, MemFault> {
        let dt = m.u32(m.u32(guest_layout::points::GAME_TIME.base)?.wrapping_add(TIME_LENGTH))?;
        Ok((dt, walk::first(&View(m))?))
    });
    let (dt, mut at) = start?;
    count(|s| s.phases += 1);
    let call = |target: u32, handle: u32| GuestCall::new(target).reserve(reserve + FRAME).ebx(handle);
    while let Some(handle) = at {
        count(|s| s.visited += 1);
        let alive = cx.call(call(walk::ALIVE, handle).args([handle])).await?;
        if alive.eax as u8 != 0 {
            count(|s| s.dead += 1);
        } else if cx.call(call(walk::CULLED, handle)).await?.eax as u8 != 0 {
            count(|s| s.culled += 1);
        } else if cx.read(|m| walk::retires(&View(m), handle))? {
            count(|s| s.retired += 1);
            cx.call(call(walk::DELETE, handle).eax(handle)).await?;
        } else {
            count(|s| s.updated += 1);
            cx.call(call(walk::UPDATE, handle).args([handle, dt])).await?;
        }
        at = cx.read(|m| walk::next(&View(m), handle))?;
    }
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use guest_layout::entry::{self, EntryFuture, Regs, Ret};
    use guest_layout::testing::FakeGuest;

    pub(crate) const STACK: u32 = 0x0030_0000;
    pub(crate) const TIME: u32 = 0x0052_0000;
    const HDR: u32 = 0x0060_0000;
    const DATA: u32 = 0x0061_0000;
    const BITS: u32 = 0x0062_0000;

    /// Live slots 1, 2, 5 and 40 of 64; slot 2 retires, slot 5 is dead,
    /// slot 40 is culled.
    pub(crate) fn guest() -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(STACK - 0x8000, 0x10000).map(TIME, 0x1000).map(HDR, 0x1000).map(BITS, 0x1000);
        g.map(DATA, 0x190 * 64);
        g.map(walk::ARRAY & !0xfff, 0x1000).map(guest_layout::points::GAME_TIME.base & !0xfff, 0x1000);
        g.put_u32(guest_layout::points::GAME_TIME.base, TIME).put_f32(TIME + 4, 1.0 / 30.0);
        g.put_u32(walk::ARRAY, HDR);
        g.put_u32(HDR + walk::HEADER_SIZE, walk::DATUM).put_u32(HDR + walk::HEADER_COUNT, 64);
        g.put_u32(HDR + walk::HEADER_DATA, DATA).put_u32(HDR + walk::HEADER_BITS, BITS);
        g.put_u32(BITS, 1 << 1 | 1 << 2 | 1 << 5).put_u32(BITS + 4, 1 << 8);
        for i in [1u32, 2, 5, 40] {
            g.put_u16(DATA + 0x190 * i, 0x20 + i as u16);
        }
        g.put_u8(DATA + 0x190 * 2 + 2, 0x40).put_u32(DATA + 0x190 * 2 + 0xfc, u32::MAX);
        g
    }

    pub(crate) fn answer(c: &entry::GuestCall, _: &mut dyn GuestMemory) -> Regs {
        let handle = c.ebx.unwrap_or(0) & 0xffff;
        let yes = match c.target {
            walk::ALIVE => handle == 5,
            walk::CULLED => handle == 40,
            _ => false,
        };
        Regs { eax: u32::from(yes), ..Default::default() }
    }

    #[test]
    fn the_walk_calls_the_callees_in_retail_order() {
        let f = |cx: Cx| -> EntryFuture {
            Box::pin(async move {
                run(&cx, 0x100).await?;
                Ok(Ret::new(0, 0))
            })
        };
        let run = entry::testing::run(&f, guest(), Regs { esp: STACK, ..Default::default() }, answer);
        assert_eq!(run.result, Ok(Ret::new(0, 0)));
        let h = |i: u32| (0x20 + i) << 16 | i;
        let dt = (1.0f32 / 30.0).to_bits();
        let got: Vec<(u32, Vec<u32>, Option<u32>)> = run.calls.iter().map(|c| (c.target, c.args.clone(), c.eax)).collect();
        assert_eq!(
            got,
            [
                (walk::ALIVE, vec![h(1)], None),
                (walk::CULLED, vec![], None),
                (walk::UPDATE, vec![h(1), dt], None),
                (walk::ALIVE, vec![h(2)], None),
                (walk::CULLED, vec![], None),
                (walk::DELETE, vec![], Some(h(2))),
                (walk::ALIVE, vec![h(5)], None),
                (walk::ALIVE, vec![h(40)], None),
                (walk::CULLED, vec![], None),
            ]
        );
        assert!(run.calls.iter().zip([1, 1, 1, 2, 2, 2, 5, 40, 40]).all(|(c, i)| c.ebx == Some(h(i))));
        assert!(run.calls.iter().all(|c| c.reserve == 0x100 + FRAME));
    }

    #[test]
    fn an_unreadable_array_faults_before_any_call() {
        let f = |cx: Cx| -> EntryFuture {
            Box::pin(async move {
                run(&cx, 0).await?;
                Ok(Ret::new(0, 0))
            })
        };
        let mut g = FakeGuest::new();
        g.map(STACK - 0x8000, 0x10000);
        let run = entry::testing::run(&f, g, Regs { esp: STACK, ..Default::default() }, answer);
        assert!(run.result.is_err());
        assert_eq!(run.calls.len(), 0);
    }
}
