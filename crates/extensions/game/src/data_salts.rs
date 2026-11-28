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

use guest_layout::memory::{GuestMemory, MemFault};
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
        })
        // Before every other hook on the new game: they read arrays whose
        // handles the reseed fixes.
        .first()
        // The other first hooks here write the options and zoom tables,
        // not the arrays; the order is only for the plan.
        .after_feature(crate::game_variants::FEATURE)
        .after_feature("h1-scope-zoom");
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


/// Every game load starts from clean object memory, as a fresh process does
/// (docs/game/theater-design.md, "The start hash"). The object memory pool
/// is not zeroed when a game frees an object, so a game after another keeps
/// the bytes of the earlier game's objects in the parts of the pool the new
/// objects do not write. Before `GAME_ACTIVATE`, when the `object` array
/// has no live element, the pool's storage is zeroed except the allocator's
/// first block header ([`lockstep::gameplay::POOL_KEEP`]), and so are the
/// object globals past it (the next object sequence number and the
/// placement scratch). With live
/// objects still in it (the previous game is disposed inside the
/// activation) nothing is zeroed: the load is counted in
/// [`object_pool_not_empty_at_load`] and the first is logged with the live
/// count, so a run shows the hook is too early.
pub const POOL_FEATURE: &str = "object-pool-clear";

static NOT_EMPTY: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Game loads that found live objects, and so left the pool alone.
pub fn object_pool_not_empty_at_load() -> u64 {
    NOT_EMPTY.load(std::sync::atomic::Ordering::Relaxed)
}

pub struct ObjectPoolClear;

impl hooks::Feature for ObjectPoolClear {
    fn manifest(&self) -> hooks::Manifest {
        hooks::Manifest::new(POOL_FEATURE, "Object memory pool cleared at every game load")
            .doc("docs/game/theater-design.md")
    }

    fn build(&self, b: &mut hooks::Builder) -> Result<(), hooks::BuildError> {
        b.raw().before(&GAME_ACTIVATE, "clear the object memory pool", |c| {
            let rt = std::sync::Arc::clone(c.ctx.runtime());
            let mut mem = rt.memory();
            match clear_object_pool(&mut *mem) {
                Ok(Cleared::Zeroed(bytes)) => eprintln!("object pool: {bytes} bytes cleared for the new game"),
                Ok(Cleared::Live(live)) => {
                    if NOT_EMPTY.fetch_add(1, std::sync::atomic::Ordering::Relaxed) == 0 {
                        eprintln!(
                            "object pool: {live} objects still live at the game load; not cleared \
                             (the hook is before the previous game's disposal)"
                        );
                    }
                }
                Ok(Cleared::NoPool) => {}
                Err(e) => eprintln!("object pool: arena unreadable ({e:?})"),
            }
            Ok(hooks::Flow::Continue)
        })
        .first()
        .after_feature(FEATURE)
        .after_feature(crate::game_variants::FEATURE)
        .after_feature("h1-scope-zoom");
        b.snapshot(hooks::SnapshotPolicy::Stateless);
        Ok(())
    }
}

/// What [`clear_object_pool`] did.
#[derive(Debug, PartialEq, Eq)]
pub enum Cleared {
    Zeroed(usize),
    /// The `object` array has this many live elements.
    Live(i32),
    NoPool,
}

/// Zero the object memory pool if no object is live.
pub fn clear_object_pool(m: &mut dyn GuestMemory) -> Result<Cleared, MemFault> {
    let lockstep::SimRegion::Indirect { pointer, len } = lockstep::SimRegion::HALO2 else {
        unreachable!()
    };
    let base = m.u32(pointer)?;
    if base == 0 {
        return Ok(Cleared::NoPool);
    }
    let block = m.bytes(base, len as usize)?;
    let Some(pool) = lockstep::gameplay::object_pool(&block, base) else {
        return Ok(Cleared::NoPool);
    };
    if pool.live != 0 {
        return Ok(Cleared::Live(pool.live));
    }
    let start = pool.storage.start + lockstep::gameplay::POOL_KEEP;
    m.write(base + start as u32, &vec![0u8; pool.storage.end - start])?;
    let globals = pool.globals.clone();
    m.write(base + globals.start as u32, &vec![0u8; globals.len()])?;
    Ok(Cleared::Zeroed(pool.storage.end - start + globals.len()))
}

#[cfg(test)]
mod tests;
