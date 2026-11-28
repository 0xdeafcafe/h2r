//! The process's `World` (docs/engine/sim-world.md, "Store slots"): it
//! holds the sound table's `Sounds` as its sound store and the object
//! table as its objects store, and its save is the one snapshot section of
//! the native sim's state. `World::save` is
//! the format; no store saves itself elsewhere.

use std::any::Any;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use sim_objects::{LightTable, ObjectTable};
use sim_script::{ScriptStore, Shared as SharedScripts};
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

/// The shared object table: the pool is the truth for the objects array,
/// and the world saves it in its objects section.
pub type SharedObjects = Arc<Mutex<ObjectTable>>;

/// The process's object table.
pub static OBJECT_TABLE: LazyLock<SharedObjects> = LazyLock::new(SharedObjects::default);

pub fn lock_objects(t: &Mutex<ObjectTable>) -> MutexGuard<'_, ObjectTable> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read the guest's objects array into `objects` (docs/engine/sim-world.md,
/// "Store slots"): the table the world holds is the arena's live objects.
/// No map loaded, or the global unreadable, leaves an empty table. An array
/// that disagrees with itself is a bug in the writer that left it so.
pub fn import_objects(m: &(impl guest_layout::Mem + ?Sized), objects: &Mutex<ObjectTable>) {
    let table = match ObjectTable::import(m) {
        Ok(Some(t)) => t,
        Ok(None) | Err(sim_objects::ImportError::Fault(_)) => ObjectTable::default(),
        Err(e) => panic!("objects: the guest's objects array is corrupt: {e:?}"),
    };
    *lock_objects(objects) = table;
    import_lights(m, &LIGHT_TABLE);
}

/// The shared light table: the pool is the truth for the lights array, and
/// the world saves it in its lights section.
pub type SharedLights = Arc<Mutex<LightTable>>;

/// The process's light table.
pub static LIGHT_TABLE: LazyLock<SharedLights> = LazyLock::new(SharedLights::default);

pub fn lock_lights(t: &Mutex<LightTable>) -> MutexGuard<'_, LightTable> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read the guest's lights array into `lights`, as [`import_objects`] does
/// for the objects: no map loaded leaves an empty table.
pub fn import_lights(m: &(impl guest_layout::Mem + ?Sized), lights: &Mutex<LightTable>) {
    let table = match LightTable::import(m) {
        Ok(Some(t)) => t,
        Ok(None) | Err(sim_objects::ImportError::Fault(_)) => LightTable::default(),
        Err(e) => panic!("lights: the guest's lights array is corrupt: {e:?}"),
    };
    *lock_lights(lights) = table;
}

/// The light table as the world's lights store; a restore puts the decoded
/// pool in the table, like [`ObjectStore`].
pub struct LightStore(pub SharedLights);

impl Store for LightStore {
    fn kind(&self) -> u16 {
        kind::LIGHTS
    }
    fn version(&self) -> u16 {
        LightTable::VERSION
    }
    fn encode(&self) -> Vec<u8> {
        lock_lights(&self.0).encode()
    }
    fn section_hash(&self) -> u64 {
        lock_lights(&self.0).section_hash()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        Ok(Box::new(lock_lights(&self.0).decode(version, body)?))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        if let Ok(table) = state.downcast::<LightTable>() {
            *lock_lights(&self.0) = *table;
        }
    }
}

/// The object table as the world's objects store. A restore puts the
/// decoded pool in the table; the guest arena comes back with the machine
/// snapshot it was taken with, so the two halves are one checkpoint
/// (docs/engine/sim-world.md, "Store slots").
pub struct ObjectStore(pub SharedObjects);

impl Store for ObjectStore {
    fn kind(&self) -> u16 {
        kind::OBJECTS
    }
    fn version(&self) -> u16 {
        ObjectTable::VERSION
    }
    fn encode(&self) -> Vec<u8> {
        lock_objects(&self.0).encode()
    }
    fn section_hash(&self) -> u64 {
        lock_objects(&self.0).section_hash()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        Ok(Box::new(lock_objects(&self.0).decode(version, body)?))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        if let Ok(table) = state.downcast::<ObjectTable>() {
            *lock_objects(&self.0) = *table;
        }
    }
}

/// The process's scripts section: the script runtime reads and commits its
/// VM state here, and the world saves it ([`ScriptStore`]).
pub static SCRIPT_TABLE: LazyLock<SharedScripts> = LazyLock::new(SharedScripts::default);

/// A world that holds `table` as its sound store and a fresh object table.
pub fn with_table(table: Shared) -> World {
    with_tables(table, SharedObjects::default())
}

/// A world that holds `sounds` and `objects` as its stores.
pub fn with_tables(sounds: Shared, objects: SharedObjects) -> World {
    with_stores(sounds, objects, SharedLights::default())
}

/// A world that holds `sounds`, `objects` and `lights` as its stores.
pub fn with_stores(sounds: Shared, objects: SharedObjects, lights: SharedLights) -> World {
    with_all_stores(sounds, objects, lights, SharedScripts::default())
}

/// A world that holds `sounds`, `objects`, `lights` and `scripts` as its
/// stores.
pub fn with_all_stores(
    sounds: Shared,
    objects: SharedObjects,
    lights: SharedLights,
    scripts: SharedScripts,
) -> World {
    let mut w = World::new(1);
    w.attach(Box::new(ScriptStore(scripts))).expect("a world without one takes the scripts store");
    w.attach(Box::new(LightStore(lights))).expect("a world without one takes the lights store");
    w.attach(Box::new(ObjectStore(objects))).expect("an empty world takes the objects store");
    w.attach(Box::new(SoundStore(sounds))).expect("a world without one takes the sound store");
    w.attach(Box::new(sim_world::ext::store::ExtStore)).expect("a world without one takes the extension store");
    w
}

/// The process's world, over the process's tables ([`entries::SHARED`],
/// [`OBJECT_TABLE`]).
pub static WORLD: LazyLock<Mutex<World>> = LazyLock::new(|| {
    Mutex::new(with_all_stores(
        Arc::clone(&entries::SHARED),
        Arc::clone(&OBJECT_TABLE),
        Arc::clone(&LIGHT_TABLE),
        Arc::clone(&SCRIPT_TABLE),
    ))
});

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

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use sim_objects::{ObjectRecord, ObjectTable};

    use super::*;

    fn rec(addr: u32) -> ObjectRecord {
        ObjectRecord { flags: 3, kind: 1, cluster: 0, body_size: 2, body_addr: addr, body: vec![9, 8] }
    }

    /// The object pool is the world's objects section: a save carries it,
    /// a restore into another world puts it in that world's table, and
    /// the next handle after the restore is the one the saved table would
    /// hand out.
    #[test]
    fn the_world_saves_the_object_table_and_restores_it_into_its_table() {
        let a = Arc::new(Mutex::new(ObjectTable::default()));
        let sounds = || Arc::new(Mutex::new(entries::Table::guest()));
        let h = lock_objects(&a).alloc(rec(0x100)).unwrap();
        lock_objects(&a).alloc(rec(0x200)).unwrap();
        lock_objects(&a).delete(h).unwrap();
        let saved = with_tables(sounds(), Arc::clone(&a)).save();
        let b = Arc::new(Mutex::new(ObjectTable::default()));
        let mut world = with_tables(sounds(), Arc::clone(&b));
        assert_ne!(world.save(), saved);
        world.restore_into(&saved).unwrap();
        assert_eq!(lock_objects(&b).encode(), lock_objects(&a).encode());
        assert_eq!(world.save(), saved);
        assert_eq!(lock_objects(&b).alloc(rec(0x300)), lock_objects(&a).alloc(rec(0x300)));
    }

    /// The light pool is the world's lights section, like the objects.
    #[test]
    fn the_world_saves_the_light_table_and_restores_it_into_its_table() {
        let sounds = || Arc::new(Mutex::new(entries::Table::guest()));
        let a = SharedLights::new(Mutex::new(LightTable::new(8, 0x696c)));
        let mut light = sim_objects::LightRecord::default();
        light.set_flags(2);
        let h = lock_lights(&a).alloc(light.clone()).unwrap();
        lock_lights(&a).alloc(light.clone()).unwrap();
        lock_lights(&a).delete(h).unwrap();
        let saved = with_stores(sounds(), SharedObjects::default(), Arc::clone(&a)).save();
        let b = SharedLights::new(Mutex::new(LightTable::new(8, 0x696c)));
        let mut world = with_stores(sounds(), SharedObjects::default(), Arc::clone(&b));
        assert_ne!(world.save(), saved);
        world.restore_into(&saved).unwrap();
        assert_eq!(lock_lights(&b).encode(), lock_lights(&a).encode());
        assert_eq!(world.save(), saved);
    }
}
