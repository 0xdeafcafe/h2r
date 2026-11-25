//! Read resident collision source fingerprints at a stopped tick.
use super::{Bail, CollisionWorld, Read, consts};
use runtime::Guest;
use sim_collision::SourceFingerprint;

pub fn read(
    mem: &Guest,
    world: &CollisionWorld,
    clear: bool,
) -> Result<Vec<SourceFingerprint>, Bail> {
    if mem.has_trap() {
        return Err(Bail::Trap);
    }
    let guest = Read(mem, (0, 0));
    Ok(world.audit_sources(&guest, consts(&guest), clear))
}
