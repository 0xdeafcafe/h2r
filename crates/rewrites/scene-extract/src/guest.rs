//! How extraction reads guest memory: implemented by the feature over the
//! runtime's memory, by a word map in tests ([`crate::testing::Mem`]).

/// Reads guest memory.
pub trait GuestRead {
    fn read_u32(&self, addr: u32) -> Option<u32>;

    fn read_f32(&self, addr: u32) -> Option<f32> {
        self.read_u32(addr).map(f32::from_bits)
    }

    /// Runs `f` over the simulation's object pool as `sim-state` reads it
    /// (`sim_objects::PoolMem`) and answers true; false, without running
    /// `f`, reads the pool out of this memory.
    fn with_sim_objects(&self, _f: &mut dyn FnMut(&dyn guest_layout::Mem)) -> bool {
        false
    }

    /// [`Self::with_sim_objects`] for the players, their globals and the
    /// first-person blocks (`sim_players::PlayersMem`).
    fn with_sim_players(&self, _f: &mut dyn FnMut(&dyn guest_layout::Mem)) -> bool {
        false
    }
}

/// Any byte range of word-readable memory, as the `sim-state` views read
/// it: the pools out of the arena when the memory gives no sim state.
pub struct WordMem<'a>(pub &'a dyn GuestRead);

impl guest_layout::Mem for WordMem<'_> {
    fn read_bytes(&self, addr: u32, buf: &mut [u8]) -> Result<(), guest_layout::Fault> {
        let fault = guest_layout::Fault { addr, len: buf.len() as u32, write: false };
        for (i, byte) in buf.iter_mut().enumerate() {
            let at = addr.checked_add(i as u32).ok_or(fault)?;
            let word = self.0.read_u32(at & !3).ok_or(fault)?;
            *byte = (word >> ((at & 3) * 8)) as u8;
        }
        Ok(())
    }
}

/// Runs `f` over the sim's object pool, or over `mem` when it gives none.
pub fn with_objects<R>(mem: &dyn GuestRead, f: impl FnOnce(&dyn guest_layout::Mem) -> R) -> R {
    let (mut f, mut out) = (Some(f), None);
    if !mem.with_sim_objects(&mut |pool| out = f.take().map(|f| f(pool))) {
        return f.take().map(|f| f(&WordMem(mem))).expect("f unused");
    }
    out.expect("f ran")
}

/// [`with_objects`] for the players.
pub fn with_players<R>(mem: &dyn GuestRead, f: impl FnOnce(&dyn guest_layout::Mem) -> R) -> R {
    let (mut f, mut out) = (Some(f), None);
    if !mem.with_sim_players(&mut |players| out = f.take().map(|f| f(players))) {
        return f.take().map(|f| f(&WordMem(mem))).expect("f unused");
    }
    out.expect("f ran")
}

/// Writes guest memory (the skip's mask bits only, until slice 4
/// replaces the masks by whole calls).
pub trait GuestWrite: GuestRead {
    fn write_u32(&mut self, addr: u32, value: u32) -> Option<()>;
}
