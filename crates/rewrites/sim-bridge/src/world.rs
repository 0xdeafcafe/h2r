//! The process's `World` (docs/engine/sim-world.md, "Store slots"): it
//! holds the sound table's `Sounds` as its sound store, and its save is
//! the one snapshot section of the native sim's state. `World::save` is
//! the format; no store saves itself elsewhere.

use std::any::Any;
use std::sync::{LazyLock, Mutex, MutexGuard};

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use sim_sound::instances::Sounds;
use sim_world::{RestoreError, Store, World, kind};

use crate::sim_sound::entries::{self, Shared};

const ID: &str = "sim-world";

/// The table's sounds as the world's sound store: the world encodes them
/// in place, and a restore puts decoded sounds back in the table.
pub struct SoundStore(pub Shared);

impl Store for SoundStore {
    fn kind(&self) -> u16 {
        kind::SOUND
    }
    fn version(&self) -> u16 {
        Sounds::VERSION
    }
    fn encode(&self) -> Vec<u8> {
        entries::lock(&self.0).sounds.encode()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        Ok(Box::new(Sounds::decode(version, body)?))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        if let Ok(sounds) = state.downcast::<Sounds>() {
            entries::lock(&self.0).restore(*sounds);
        }
    }
}

/// A world that holds `table` as its sound store.
pub fn with_table(table: Shared) -> World {
    let mut w = World::new(1);
    w.attach(Box::new(SoundStore(table))).expect("an empty world takes the sound store");
    w
}

/// The process's world, over the process's table ([`entries::SHARED`]).
pub static WORLD: LazyLock<Mutex<World>> =
    LazyLock::new(|| Mutex::new(with_table(std::sync::Arc::clone(&entries::SHARED))));

pub fn lock(w: &Mutex<World>) -> MutexGuard<'_, World> {
    w.lock().unwrap_or_else(|e| e.into_inner())
}

/// The world as the machine snapshot's section: saved with
/// `World::save`, loaded with `World::restore_into`.
pub struct SimWorld {
    world: &'static Mutex<World>,
}

impl SimWorld {
    pub fn process() -> Self {
        Self { world: &WORLD }
    }
}

impl Feature for SimWorld {
    fn manifest(&self) -> Manifest {
        Manifest::new(ID, "The native sim's World: its stores in one snapshot section")
            .always_on()
            .doc("docs/engine/sim-world.md")
    }

    fn build(&self, b: &mut Builder) -> Result<(), BuildError> {
        let (save, load) = (self.world, self.world);
        b.snapshot(SnapshotPolicy::section(
            move || lock(save).save(),
            move |bytes, _| lock(load).restore_into(bytes).map_err(|e| format!("sim world: {e}")),
        ));
        Ok(())
    }
}
