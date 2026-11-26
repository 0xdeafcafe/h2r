//! Every game starts with the datum arrays' salt counters a fresh process
//! has (docs/game/theater-design.md, "The start hash").
//!
//! `data_delete_all` re-seeds an array's counter from its name, but an
//! array a map load empties one datum at a time keeps the salts it reached,
//! so the handles a game stores (an object's, an actor's, a script's)
//! depend on what the process played before. Before `GAME_ACTIVATE` the
//! counters of every data array in the game-state arena are set to what
//! `data_delete_all` seeds them with (`lockstep::gameplay::salt_seeds`).
//! Salts only label datums, and the engine itself re-seeds them when a
//! saved state is attached, so play does not change; films recorded and
//! watched in any order start from the same arena.

use engine::memory::{GuestMemory, MemFault};
use hooks::points::GAME_ACTIVATE;

pub const FEATURE: &str = "data-salts";

pub struct DataSalts;

impl hooks::Feature for DataSalts {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(FEATURE, "Datum array salts seeded at every game start")
            .doc("docs/game/theater-design.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.raw().before(&GAME_ACTIVATE, "seed the datum salts", |c| {
            let rt = std::sync::Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            match reseed(&mut *mem) {
                Ok(n) => eprintln!("data salts: {n} arrays seeded for the new game"),
                Err(e) => eprintln!("data salts: arena unreadable ({e:?})"),
            }
            Ok(hooks::Flow::Continue)
        });
        // The counters are guest memory.
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// Seed every data array of the game-state arena; how many.
pub fn reseed(m: &mut dyn GuestMemory) -> Result<usize, MemFault> {
    let lockstep::SimRegion::Indirect { pointer, len } = lockstep::SimRegion::HALO2 else {
        unreachable!()
    };
    let base = m.u32(pointer)?;
    if base == 0 {
        return Ok(0);
    }
    let block = m.bytes(base, len as usize)?;
    let seeds = lockstep::gameplay::salt_seeds(&block);
    for (offset, seed) in &seeds {
        m.write(base + offset, &seed.to_le_bytes())?;
    }
    Ok(seeds.len())
}

#[cfg(test)]
mod tests;
