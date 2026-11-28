//! The engine frame wait `0x0012b2e0` in Rust (docs/runtime/sim.md §2,
//! "Frame timing"): the words it leaves for frame timing, and the watch
//! that stops a wait no vblank will end.
//!
//! The guest's body (Code, `0x0012b2e0..0x0012b3b5`, no arguments, plain
//! `ret`):
//!
//! | Step | Does |
//! |---|---|
//! | entry | `[entry_count] = [counter] + 1` (64-bit) |
//! | spin `0x0012b330..0x0012b342` | while `[counter] < [target] - 1` (signed 64-bit) |
//! | exit `0x0012b344..` | `n = [counter] + 1`, `[exit_count] = n`, `d = n - [target]` |
//! | result | EAX `0` if `d < 0`, else `min(d, 0x7fff)`; ECX `d`'s high word; EDX the counter's low word |
//! | late flag | byte `[late_flag] = word [frame_gate] > 0 && AX == 0` |
//!
//! [`enter`] and [`exit`] make those writes. The spin is the caller's: a
//! wait gives its slice up between checks so the kernel's clocked vblank
//! source moves the counter, and [`Watch`] names a wait that never ends.

use guest_layout::memory::{GuestMemory, MemFault};

/// The globals the wait reads and writes (all 64-bit but the two flags).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Addrs {
    /// The vblank counter the vblank callback adds to.
    pub counter: u32,
    /// The counter value the wait is for.
    pub target: u32,
    /// Written with the counter + 1 on entry.
    pub entry_count: u32,
    /// Written with the counter + 1 on exit.
    pub exit_count: u32,
    /// Signed word read on exit.
    pub frame_gate: u32,
    /// Byte written on exit.
    pub late_flag: u32,
}

impl Addrs {
    pub const HALO2: Addrs = Addrs {
        counter: 0x0048_5ab0,
        target: 0x004e_6398,
        entry_count: 0x004e_63a8,
        exit_count: 0x004e_63b0,
        frame_gate: 0x004e_63b8,
        late_flag: 0x004e_63bc,
    };
}

/// The registers the wait returns.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exit {
    pub eax: u32,
    pub ecx: u32,
    pub edx: u32,
}

fn i64_at(g: &dyn GuestMemory, at: u32) -> Result<i64, MemFault> {
    let lo = u64::from(g.u32(at)?);
    let hi = u64::from(g.u32(at.wrapping_add(4))?);
    Ok((hi << 32 | lo) as i64)
}

/// The counter and the target.
pub fn read(a: &Addrs, g: &dyn GuestMemory) -> Result<(i64, i64), MemFault> {
    Ok((i64_at(g, a.counter)?, i64_at(g, a.target)?))
}

/// The entry: `[entry_count] = [counter] + 1`.
pub fn enter(a: &Addrs, g: &mut dyn GuestMemory) -> Result<(), MemFault> {
    let n = i64_at(g, a.counter)?.wrapping_add(1);
    g.write(a.entry_count, &n.to_le_bytes())
}

/// The spin's test: the wait is over.
pub fn arrived(counter: i64, target: i64) -> bool {
    counter >= target.wrapping_sub(1)
}

/// The exit, once the counter has arrived: its writes and registers.
/// `None` while the wait goes on; nothing is written then.
pub fn exit(a: &Addrs, g: &mut dyn GuestMemory) -> Result<Option<Exit>, MemFault> {
    let (counter, target) = read(a, g)?;
    if !arrived(counter, target) {
        return Ok(None);
    }
    let gate = g.u16(a.frame_gate)? as i16;
    let n = counter.wrapping_add(1);
    let d = n.wrapping_sub(target);
    let eax = if d < 0 { 0 } else { d.min(0x7fff) as u32 };
    g.write(a.exit_count, &n.to_le_bytes())?;
    g.write(a.late_flag, &[u8::from(gate > 0 && eax as u16 == 0)])?;
    Ok(Some(Exit { eax, ecx: (d >> 32) as u32, edx: counter as u32 }))
}

/// Yields in a row with neither a vblank nor logical time passing after
/// which a wait stops.
pub const MAX_IDLE_YIELDS: u32 = 100_000;
/// Logical time (100 ns ticks) a wait lets pass with no vblank: two
/// seconds, 120 periods of a 60 Hz source.
pub const MAX_VBLANK_GAP: u64 = 20_000_000;
/// Vblanks the first check may find the counter short by.
pub const MAX_VBLANKS: i64 = 600;

/// A wait in progress, between yields. A yield that lets the logical clock
/// move starts the idle count again: a busy machine reaches its vblank by
/// working, however many yields that takes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Watch {
    last: Option<i64>,
    idle: u32,
    time: Option<u64>,
    since: Option<u64>,
}

impl Watch {
    /// One check that found the wait not over, at logical time `now`:
    /// `Err` names why no vblank will end it.
    pub fn step(&mut self, counter: i64, target: i64, now: Option<u64>) -> Result<(), String> {
        let want = target.wrapping_sub(1);
        let short = want.wrapping_sub(counter);
        if self.last.is_none() && short > MAX_VBLANKS {
            return Err(format!(
                "the counter ({counter}) is {short} vblanks short of {want}, more than {MAX_VBLANKS} in one wait"
            ));
        }
        if self.last != Some(counter) {
            self.idle = 0;
            self.last = Some(counter);
            self.since = now;
        } else if now.is_some() && now > self.time {
            self.idle = 0;
        } else {
            self.idle += 1;
        }
        self.time = now;
        if let (Some(now), Some(since)) = (now, self.since)
            && now.saturating_sub(since) > MAX_VBLANK_GAP
        {
            return Err(format!(
                "{} ticks of logical time without a vblank (counter {counter}, waiting for {want}): is a clocked vblank source installed?",
                now - since
            ));
        }
        if self.idle >= MAX_IDLE_YIELDS {
            return Err(format!(
                "{MAX_IDLE_YIELDS} yields without a vblank or logical time passing (counter {counter}, waiting for {want}): is a clocked vblank source installed?"
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use guest_layout::testing::FakeGuest;

    const A: Addrs = Addrs {
        counter: 0x1000,
        target: 0x1008,
        entry_count: 0x1010,
        exit_count: 0x1018,
        frame_gate: 0x1020,
        late_flag: 0x1024,
    };

    fn guest(counter: i64, target: i64, gate: i16) -> FakeGuest {
        let mut g = FakeGuest::new();
        g.map(0x1000, 0x100);
        g.write(A.counter, &counter.to_le_bytes()).unwrap();
        g.write(A.target, &target.to_le_bytes()).unwrap();
        g.write(A.frame_gate, &gate.to_le_bytes()).unwrap();
        g.put_u8(A.late_flag, 0xaa);
        g
    }

    #[test]
    fn the_entry_writes_the_counter_on() {
        let mut g = guest(0x1_ffff_ffff, 0, 0);
        enter(&A, &mut g).unwrap();
        assert_eq!(i64_at(&g, A.entry_count), Ok(0x2_0000_0000));
    }

    #[test]
    fn a_wait_short_of_its_target_writes_nothing() {
        let mut g = guest(10, 12, 0);
        let before = g.bytes(0x1000, 0x100).unwrap();
        assert_eq!(exit(&A, &mut g), Ok(None));
        assert_eq!(g.bytes(0x1000, 0x100).unwrap(), before);
    }

    #[test]
    fn an_exit_on_time_is_eax_zero_and_late_by_the_gate() {
        // counter 11, target 12: n 12, d 0.
        let mut g = guest(11, 12, 1);
        assert_eq!(exit(&A, &mut g), Ok(Some(Exit { eax: 0, ecx: 0, edx: 11 })));
        assert_eq!(i64_at(&g, A.exit_count), Ok(12));
        assert_eq!(g.u8(A.late_flag), Ok(1));
        // The gate at or below 0: not late.
        let mut g = guest(11, 12, -1);
        exit(&A, &mut g).unwrap();
        assert_eq!(g.u8(A.late_flag), Ok(0));
    }

    #[test]
    fn a_late_exit_counts_the_vblanks_over_capped() {
        let mut g = guest(15, 12, 1);
        assert_eq!(exit(&A, &mut g), Ok(Some(Exit { eax: 4, ecx: 0, edx: 15 })));
        assert_eq!(g.u8(A.late_flag), Ok(0));
        let mut g = guest(0x9000, 12, 0);
        assert_eq!(exit(&A, &mut g).unwrap().unwrap().eax, 0x7fff);
        // d negative never arrives; d's high word goes to ECX.
        let mut g = guest(0x1_0000_0000, 1, 0);
        let e = exit(&A, &mut g).unwrap().unwrap();
        assert_eq!((e.eax, e.ecx, e.edx), (0x7fff, 1, 0));
    }

    #[test]
    fn the_watch_names_a_wait_no_vblank_ends() {
        // Too far ahead at the first check.
        assert!(Watch::default().step(0, MAX_VBLANKS + 2, Some(0)).is_err());
        // The clock moves with no vblank: stops past the gap.
        let mut w = Watch::default();
        assert!(w.step(0, 3, Some(0)).is_ok());
        assert!(w.step(0, 3, Some(MAX_VBLANK_GAP)).is_ok());
        assert!(w.step(0, 3, Some(MAX_VBLANK_GAP + 1)).is_err());
        // A vblank starts the gap again.
        let mut w = Watch::default();
        w.step(0, 3, Some(0)).unwrap();
        w.step(1, 3, Some(MAX_VBLANK_GAP)).unwrap();
        assert!(w.step(1, 3, Some(MAX_VBLANK_GAP * 2)).is_ok());
        // The clock standing still: stops after the idle yields.
        let mut w = Watch::default();
        for _ in 0..MAX_IDLE_YIELDS {
            w.step(0, 3, None).unwrap();
        }
        assert!(w.step(0, 3, None).is_err());
    }
}
