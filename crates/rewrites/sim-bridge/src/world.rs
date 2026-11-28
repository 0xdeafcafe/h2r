//! The process's `World` (docs/engine/sim-world.md, "Store slots"): it
//! holds the sound table's `Sounds` as its sound store, the object
//! table as its objects store, the players state as its players store and
//! the Havok heap as its physics store, and its save is the one snapshot section of
//! the native sim's state. `World::save` is
//! the format; no store saves itself elsewhere.

use std::any::Any;
use std::sync::{Arc, LazyLock, Mutex, MutexGuard};

use hooks::{BuildError, Builder, Feature, Manifest, SnapshotPolicy};
use sim_objects::{LightTable, ObjectTable};
use sim_players::PlayersState;
use sim_script::{ScriptStore, Shared as SharedScripts};
use sim_havok::heap::Heap;
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

/// The shared players state: the pool is the truth for the players array,
/// their globals and the first-person blocks, and the world saves it in its
/// players section.
pub type SharedPlayers = Arc<Mutex<PlayersState>>;

/// The process's players state.
pub static PLAYERS_STATE: LazyLock<SharedPlayers> = LazyLock::new(SharedPlayers::default);

pub fn lock_players(t: &Mutex<PlayersState>) -> MutexGuard<'_, PlayersState> {
    t.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read the guest's players array, globals and first-person block into
/// `players` (docs/engine/sim-world.md, "The players store"). No game
/// initialised, or the globals unreadable, leaves an empty state. An array
/// that disagrees with itself is a bug in the writer that left it so.
pub fn import_players(m: &(impl guest_layout::Mem + ?Sized), players: &Mutex<PlayersState>) {
    let state = match PlayersState::import(m) {
        Ok(Some(s)) => s,
        Ok(None) | Err(sim_players::ImportError::Fault(_)) => PlayersState::default(),
        Err(e) => panic!("players: the guest's players array is corrupt: {e:?}"),
    };
    *lock_players(players) = state;
}

/// The players state as the world's players store. A restore puts the
/// decoded state in the shared one; the guest arena comes back with the
/// machine snapshot it was taken with, so the two halves are one checkpoint.
pub struct PlayersStore(pub SharedPlayers);

impl Store for PlayersStore {
    fn kind(&self) -> u16 {
        kind::PLAYERS
    }
    fn version(&self) -> u16 {
        PlayersState::VERSION
    }
    fn encode(&self) -> Vec<u8> {
        lock_players(&self.0).encode()
    }
    fn section_hash(&self) -> u64 {
        lock_players(&self.0).section_hash()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        Ok(Box::new(PlayersState::decode(version, body)?))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        if let Ok(state) = state.downcast::<PlayersState>() {
            *lock_players(&self.0) = *state;
        }
    }
}

/// The shared Havok heap (docs/engine/sim-world.md, "The physics heap").
pub type SharedHeap = Arc<Mutex<Heap>>;

/// The process's Havok heap.
pub static HAVOK_HEAP: LazyLock<SharedHeap> = LazyLock::new(SharedHeap::default);

pub fn lock_heap(h: &Mutex<Heap>) -> MutexGuard<'_, Heap> {
    h.lock().unwrap_or_else(|e| e.into_inner())
}

/// Read the guest's Havok heap and the memory system's globals into
/// `heap`. No heap reserved (base 0) is an empty heap; a base whose block
/// faults or whose size word is not retail's is a bug in what wrote it.
pub fn import_heap(m: &(impl guest_layout::Mem + ?Sized), heap: &Mutex<Heap>) {
    let read = |at: u32, buf: &mut [u8]| m.read_bytes(at, buf).ok();
    let Some(h) = Heap::import(read) else {
        panic!("physics: the Havok heap at [{:#x}] is unreadable or not retail's size", sim_havok::heap::BASE_GLOBAL);
    };
    *lock_heap(heap) = h;
}

/// The Havok heap as the world's physics store. As with the objects, a
/// restore puts the decoded heap in the shared copy and guest memory
/// returns with the machine snapshot taken in the same call.
pub struct PhysicsStore(pub SharedHeap);

impl Store for PhysicsStore {
    fn kind(&self) -> u16 {
        kind::PHYSICS
    }
    fn version(&self) -> u16 {
        Heap::VERSION
    }
    fn encode(&self) -> Vec<u8> {
        lock_heap(&self.0).encode()
    }
    fn decode(&self, version: u16, body: &[u8]) -> Result<Box<dyn Any + Send>, RestoreError> {
        Ok(Box::new(Heap::decode(version, body)?))
    }
    fn install(&mut self, state: Box<dyn Any + Send>) {
        if let Ok(heap) = state.downcast::<Heap>() {
            *lock_heap(&self.0) = *heap;
        }
    }
}

/// The process's scripts section: the script runtime reads and commits its
/// VM state here, and the world saves it ([`ScriptStore`]).
pub static SCRIPT_TABLE: LazyLock<SharedScripts> = LazyLock::new(SharedScripts::default);

/// A world that holds `table` as its sound store and a fresh object table
/// and heap.
pub fn with_table(table: Shared) -> World {
    with_tables(table, SharedObjects::default())
}

/// A world that holds `sounds` and `objects` as its stores, and a fresh
/// heap.
pub fn with_tables(sounds: Shared, objects: SharedObjects) -> World {
    with_stores(sounds, objects, SharedLights::default(), SharedPlayers::default())
}

/// A world that holds `sounds`, `objects`, `lights` and `players` as its stores.
pub fn with_stores(sounds: Shared, objects: SharedObjects, lights: SharedLights, players: SharedPlayers) -> World {
    with_all_stores(sounds, objects, lights, players, SharedScripts::default(), SharedHeap::default())
}

/// A world that holds every store: `sounds`, `objects`, `lights`,
/// `players`, `scripts` and `heap`.
pub fn with_all_stores(
    sounds: Shared,
    objects: SharedObjects,
    lights: SharedLights,
    players: SharedPlayers,
    scripts: SharedScripts,
    heap: SharedHeap,
) -> World {
    let mut w = World::new(1);
    w.attach(Box::new(ScriptStore(scripts))).expect("a world without one takes the scripts store");
    w.attach(Box::new(LightStore(lights))).expect("a world without one takes the lights store");
    w.attach(Box::new(PlayersStore(players))).expect("a world without one takes the players store");
    w.attach(Box::new(ObjectStore(objects))).expect("an empty world takes the objects store");
    w.attach(Box::new(PhysicsStore(heap))).expect("a world without one takes the physics store");
    w.attach(Box::new(SoundStore(sounds))).expect("a world without one takes the sound store");
    w.attach(Box::new(sim_world::ext::store::ExtStore)).expect("a world without one takes the extension store");
    w
}

/// The process's world, over the process's tables ([`entries::SHARED`],
/// [`OBJECT_TABLE`], [`HAVOK_HEAP`]).
pub static WORLD: LazyLock<Mutex<World>> = LazyLock::new(|| {
    Mutex::new(with_all_stores(
        Arc::clone(&entries::SHARED),
        Arc::clone(&OBJECT_TABLE),
        Arc::clone(&LIGHT_TABLE),
        Arc::clone(&PLAYERS_STATE),
        Arc::clone(&SCRIPT_TABLE),
        Arc::clone(&HAVOK_HEAP),
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
        let saved = with_stores(sounds(), SharedObjects::default(), Arc::clone(&a), SharedPlayers::default()).save();
        let b = SharedLights::new(Mutex::new(LightTable::new(8, 0x696c)));
        let mut world = with_stores(sounds(), SharedObjects::default(), Arc::clone(&b), SharedPlayers::default());
        assert_ne!(world.save(), saved);
        world.restore_into(&saved).unwrap();
        assert_eq!(lock_lights(&b).encode(), lock_lights(&a).encode());
        assert_eq!(world.save(), saved);
    }

    /// The players state is the world's players section: a save carries it,
    /// a restore into another world puts it in that world's state, and the
    /// next handle after the restore is the one the saved state would hand
    /// out.
    #[test]
    fn the_world_saves_the_players_and_restores_them_into_its_state() {
        use sim_players::{PlayerRecord, PlayersState};

        let a = Arc::new(Mutex::new(PlayersState::default()));
        let sounds = || Arc::new(Mutex::new(entries::Table::guest()));
        let objects = || Arc::new(Mutex::new(ObjectTable::default()));
        let (first, _) = {
            let mut s = lock_players(&a);
            (s.alloc(PlayerRecord::default()).unwrap(), s.alloc(PlayerRecord::default()).unwrap())
        };
        lock_players(&a).delete(first).unwrap();
        lock_players(&a).first_person.bytes[4] = 9;
        let saved = with_stores(sounds(), objects(), SharedLights::default(), Arc::clone(&a)).save();
        let b = Arc::new(Mutex::new(PlayersState::default()));
        let mut world = with_stores(sounds(), objects(), SharedLights::default(), Arc::clone(&b));
        assert_ne!(world.save(), saved);
        world.restore_into(&saved).unwrap();
        assert_eq!(lock_players(&b).encode(), lock_players(&a).encode());
        assert_eq!(world.save(), saved);
        assert_eq!(
            lock_players(&b).alloc(PlayerRecord::default()),
            lock_players(&a).alloc(PlayerRecord::default())
        );
    }

    /// The heap is the world's physics section, between the objects and
    /// the sounds: a save carries its bytes and a restore puts them in the
    /// other world's heap.
    #[test]
    fn the_world_saves_the_havok_heap_and_restores_it_into_its_heap() {
        use sim_havok::heap::{HEADER, HEAP_SIZE};
        let sounds = || Arc::new(Mutex::new(entries::Table::guest()));
        let mut bytes = vec![0; (HEADER + HEAP_SIZE) as usize];
        bytes[..4].copy_from_slice(&HEAP_SIZE.to_le_bytes());
        bytes[0x40] = 7;
        let a = Arc::new(Mutex::new(Heap { globals: [0x0080_0000, 0, 2, 0, 0, 0], bytes }));
        let saved = with_all_stores(sounds(), SharedObjects::default(), SharedLights::default(), SharedPlayers::default(), SharedScripts::default(), Arc::clone(&a)).save();
        let b = SharedHeap::default();
        let mut world = with_all_stores(sounds(), SharedObjects::default(), SharedLights::default(), SharedPlayers::default(), SharedScripts::default(), Arc::clone(&b));
        assert_ne!(world.save(), saved);
        world.restore_into(&saved).unwrap();
        assert_eq!(*lock_heap(&b), *lock_heap(&a));
        assert_eq!(world.save(), saved);
    }
}
