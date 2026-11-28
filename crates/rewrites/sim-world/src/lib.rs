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

use std::any::Any;
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
    /// The lights datum array (`sim_objects::lights::LightTable`).
    pub const LIGHTS: u16 = 10;
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

/// A subsystem's store held by the world through its section: a crate
/// that depends on this one (sound) owns the type, the world saves,
/// hashes and restores it in its kind's place.
pub trait Store: Send {
    /// The section kind ([`kind`]).
    fn kind(&self) -> u16;
    fn version(&self) -> u16;
    /// The section's canonical bytes.
    fn encode(&self) -> Vec<u8>;
    fn section_hash(&self) -> u64 {
        Fnv64::of(&self.encode())
    }
    /// A saved section checked and decoded; nothing changes yet.
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError>;
    /// Put in what [`Store::decode`] returned.
    fn install(&mut self, state: Box<dyn Any + Send>);
}

/// The world: every store, then the RNG. Fields are public; a `&mut`
/// borrow of a pool invalidates its cached section hash.
pub struct World {
    pub rng: Rng,
    pub probe: Pool<Probe>,
    /// The subsystems' stores ([`Store`]), in kind order.
    stores: Vec<Box<dyn Store>>,
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
            stores: Vec::new(),
            rng_hash: Cell::new(None),
        }
    }

    /// Hold `store` in its kind's place. A kind already held, or one that
    /// is not a subsystem's between the RNG and the probe, is refused.
    pub fn attach(&mut self, store: Box<dyn Store>) -> Result<(), RestoreError> {
        let k = store.kind();
        if k <= kind::RNG || k >= kind::PROBE || self.stores.iter().any(|s| s.kind() == k) {
            return Err(RestoreError::Corrupt { kind: k, what: "store kind taken or reserved" });
        }
        let at = self.stores.partition_point(|s| s.kind() < k);
        self.stores.insert(at, store);
        Ok(())
    }

    /// The store of `kind`, if one is held.
    pub fn store(&self, k: u16) -> Option<&dyn Store> {
        self.stores.iter().find(|s| s.kind() == k).map(|s| &**s)
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
    /// the whole save is valid. A fresh world holds no subsystem store, so
    /// a save with one is refused as [`RestoreError::UnknownKind`]; restore
    /// it into a world that holds the store ([`World::restore_into`]).
    pub fn restore(bytes: &[u8]) -> Result<World, RestoreError> {
        let mut world = World::new(1);
        world.restore_into(bytes)?;
        Ok(world)
    }

    /// Replace this world's state with a saved one: the RNG, the probe and
    /// every held store. A held store the save lacks is
    /// [`RestoreError::MissingSection`]. On any error `self` is left
    /// exactly as it was.
    pub fn restore_into(&mut self, bytes: &[u8]) -> Result<(), RestoreError> {
        let mut r = Reader::new(bytes);
        save::read_header(&mut r)?;
        let mut last = 0u16;
        let (mut rng, mut probe) = (None, None);
        let mut states: Vec<(usize, Box<dyn Any + Send>)> = Vec::new();
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
                kind::RNG => rng = Some(Rng::decode(version, body)?),
                kind::PROBE => {
                    probe = Some(Pool::decode(kind::PROBE, PROBE_CAPACITY, PROBE_NAME, version, body)?);
                }
                other => {
                    let Some(at) = self.stores.iter().position(|s| s.kind() == other) else {
                        return Err(RestoreError::UnknownKind { kind: other });
                    };
                    states.push((at, self.stores[at].decode(version, body)?));
                }
            }
        }
        let rng = rng.ok_or(RestoreError::MissingSection { kind: kind::RNG })?;
        let probe = probe.ok_or(RestoreError::MissingSection { kind: kind::PROBE })?;
        if let Some((_, s)) = self.stores.iter().enumerate().find(|(i, _)| !states.iter().any(|(at, _)| at == i)) {
            return Err(RestoreError::MissingSection { kind: s.kind() });
        }
        self.rng = rng;
        self.probe = probe;
        self.rng_hash.set(None);
        for (at, state) in states {
            self.stores[at].install(state);
        }
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
        let mut out = vec![(kind::RNG, Rng::VERSION, self.rng.encode())];
        out.extend(self.stores.iter().map(|s| (s.kind(), s.version(), s.encode())));
        out.push((kind::PROBE, self.probe.version(), self.probe.encode()));
        out
    }

    fn section_hashes(&self) -> Vec<(u16, u16, u64)> {
        let state = self.rng.state();
        let rng = match self.rng_hash.get() {
            Some((s, h)) if s == state => h,
            _ => {
                let h = Fnv64::of(&self.rng.encode());
                self.rng_hash.set(Some((state, h)));
                h
            }
        };
        let mut out = vec![(kind::RNG, Rng::VERSION, rng)];
        out.extend(self.stores.iter().map(|s| (s.kind(), s.version(), s.section_hash())));
        out.push((kind::PROBE, self.probe.version(), self.probe.section_hash()));
        out
    }
}

#[cfg(test)]
mod tests;
