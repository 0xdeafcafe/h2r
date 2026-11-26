//! The native simulation's world (docs/engine/sim-world.md): salted slot
//! pools, a canonical save and restore, an incremental hash and the shared
//! RNG stream. Pure: no guest memory, no tags, no hooks.
//!
//! Float rules are `sim-core`'s: `clippy.toml` bans `mul_add`.
#![deny(clippy::disallowed_methods)]

pub mod hash;
pub mod pool;
pub mod rng;
pub mod save;

pub use hash::Fnv64;
pub use pool::{Handle, Pool, Record, StaleHandle};
pub use rng::Rng;
pub use save::{BUILD_STAMP, Reader, RestoreError, Writer};

use std::cell::Cell;

/// Section kinds, in the order sections are saved and hashed. A store
/// that is not filled yet has a reserved kind and writes no section.
pub mod kind {
    pub const RNG: u16 = 1;
    pub const OBJECTS: u16 = 2;
    /// Reserved for the Havok heap (docs/engine/sim-rewrite-havok.md): a
    /// byte store whose allocation order is part of its state.
    pub const PHYSICS: u16 = 3;
    pub const ANIM: u16 = 4;
    pub const PLAYERS: u16 = 5;
    pub const SOUND: u16 = 6;
    pub const EFFECTS: u16 = 7;
    pub const AI: u16 = 8;
    pub const SCRIPTS: u16 = 9;
    /// The test store; the one store wave 1 fills.
    pub const PROBE: u16 = 0x100;
}

/// Slots in the probe pool.
pub const PROBE_CAPACITY: u32 = 64;
/// Name seed of the probe pool's first salt (`name[0..2] | 0x8000`).
pub const PROBE_NAME: u16 = 0x7072;

/// The test store's record: `value` and `name` are simulation state;
/// `scratch` is derived state a rebuild recomputes, so it is neither saved
/// nor hashed.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Probe {
    pub value: u64,
    pub name: u16,
    pub scratch: u32,
}

impl Record for Probe {
    fn write(&self, w: &mut Writer) {
        w.u64(self.value);
        w.u16(self.name);
    }
    fn read(r: &mut Reader<'_>) -> Result<Self, RestoreError> {
        Ok(Self {
            value: r.u64()?,
            name: r.u16()?,
            scratch: 0,
        })
    }
}

/// The world: every store, then the RNG. Fields are public; a `&mut`
/// borrow of a pool invalidates its cached section hash.
pub struct World {
    pub rng: Rng,
    pub probe: Pool<Probe>,
    /// Cached `sim_hash`, cleared by anything that can change a section.
    rng_hash: Cell<Option<(u32, u64)>>,
}

impl Default for World {
    fn default() -> Self {
        Self::new(1)
    }
}

impl World {
    /// An empty world whose RNG starts at `seed`.
    pub fn new(seed: u32) -> Self {
        Self {
            rng: Rng::new(seed),
            probe: Pool::new(PROBE_CAPACITY, PROBE_NAME),
            rng_hash: Cell::new(None),
        }
    }

    /// The canonical bytes: magic, build stamp, then one section per
    /// filled store in kind order (`u16 kind | u16 version | u32 len |
    /// bytes`). Derived and static state is not in them.
    pub fn save(&self) -> Vec<u8> {
        let mut w = Writer::new();
        save::write_header(&mut w);
        for (kind, version, bytes) in self.sections() {
            w.u16(kind);
            w.u16(version);
            w.u32(bytes.len() as u32);
            w.bytes(&bytes);
        }
        w.finish()
    }

    /// A world from saved bytes. A save from another build stamp is refused
    /// by name; any other fault is a typed error. Nothing is built unless
    /// the whole save is valid.
    pub fn restore(bytes: &[u8]) -> Result<World, RestoreError> {
        let mut r = Reader::new(bytes);
        save::read_header(&mut r)?;
        let mut world = World::new(1);
        let mut last = 0u16;
        let (mut saw_rng, mut saw_probe) = (false, false);
        while !r.is_empty() {
            let kind = r.u16()?;
            let version = r.u16()?;
            let len = r.u32()? as usize;
            let body = r.take(len)?;
            if kind <= last {
                return Err(RestoreError::SectionOrder { kind });
            }
            last = kind;
            match kind {
                kind::RNG => {
                    world.rng = Rng::decode(version, body)?;
                    saw_rng = true;
                }
                kind::PROBE => {
                    world.probe = Pool::decode(kind::PROBE, PROBE_CAPACITY, PROBE_NAME, version, body)?;
                    saw_probe = true;
                }
                other => return Err(RestoreError::UnknownKind { kind: other }),
            }
        }
        if !saw_rng {
            return Err(RestoreError::MissingSection { kind: kind::RNG });
        }
        if !saw_probe {
            return Err(RestoreError::MissingSection { kind: kind::PROBE });
        }
        Ok(world)
    }

    /// Replace this world with a saved one. On any error `self` is left
    /// exactly as it was.
    pub fn restore_into(&mut self, bytes: &[u8]) -> Result<(), RestoreError> {
        *self = World::restore(bytes)?;
        Ok(())
    }

    /// fnv64 over `(kind, version, section hash)` for each section in
    /// order. A pool caches its section hash until a `&mut` borrow, so a
    /// tick that touches one store re-encodes only that store.
    pub fn sim_hash(&self) -> u64 {
        let mut h = Fnv64::new();
        for (kind, version, hash) in self.section_hashes() {
            h.u16(kind);
            h.u16(version);
            h.u64(hash);
        }
        h.finish()
    }

    fn sections(&self) -> Vec<(u16, u16, Vec<u8>)> {
        vec![
            (kind::RNG, Rng::VERSION, self.rng.encode()),
            (kind::PROBE, self.probe.version(), self.probe.encode()),
        ]
    }

    fn section_hashes(&self) -> [(u16, u16, u64); 2] {
        let state = self.rng.state();
        let rng = match self.rng_hash.get() {
            Some((s, h)) if s == state => h,
            _ => {
                let h = Fnv64::of(&self.rng.encode());
                self.rng_hash.set(Some((state, h)));
                h
            }
        };
        [
            (kind::RNG, Rng::VERSION, rng),
            (kind::PROBE, self.probe.version(), self.probe.section_hash()),
        ]
    }
}

#[cfg(test)]
mod tests;
