# The simulation world: pools, save, hash and RNG

Status: `crates/rewrites/sim-world` is built with unit tests. Sound
(`sim_sound::instances::Sounds`) and the object table
(`sim_objects::ObjectTable`) are on its pools: the process's `World`
(`sim_bridge::world::WORLD`) holds both at boot, and no other store is
filled.

The native simulation keeps its state in a Rust `World`, and that is the
truth ([sim-rewrite.md](sim-rewrite.md)). The crate is pure: it depends on
`sim-core` only and reads no guest memory and no tags.

## Pools

`Pool<T>` (`src/pool.rs`) is retail's datum array ([datum_new.md](datum_new.md))
as a typed pool. A `Handle` is `salt << 16 | index`, `0xffffffff` is none, and
a slot is live when its salt is non-zero. `alloc` follows `datum_new`: scan
from `free_scan_start` to `high_water`, else take `high_water`; the salt comes
from `next_salt`, seeded `name | 0x8000`, and skips `0xffff`. `free` lowers
`free_scan_start` and walks `high_water` down past free slots. Iteration is in
slot order, so script ids, net ids and every walk match retail's.

One difference from retail: `get`, `get_mut` and `free` check the salt. A
stale handle returns `None` or `StaleHandle`; retail frees whatever the slot
holds.

A store is a `Pool<T: Record>`. `Record::write` and `Record::read` carry
simulation state only. Derived state (caches a rebuild recomputes) and static
state (tag data) are not written, so they are not saved and not hashed.

## Save and restore

`World::save` writes `H2WS`, a length-prefixed build stamp (`BUILD_STAMP`),
then one section per filled store in ascending kind order: `u16 kind | u16
version | u32 len | bytes`. Integers are little-endian. A pool section holds
`capacity, next_salt, free_scan_start, high_water, active`, then per slot below
the high water its salt and, when live, its record. Free slots keep their
place because allocation order is state.

`World::restore` builds a new world and fails with a named `RestoreError`:
another build stamp (`OtherLayout`, naming the stamp it found), bad magic,
truncation, out-of-order or unknown or missing sections, a section version
this build does not write, trailing bytes, or counts that disagree.
`World::restore_into` replaces `self`'s state only on success, so a refused
restore changes nothing.

A subsystem crate that depends on this one holds its store in the world
through `Store` (`kind`, `version`, `encode`, `decode`, `install`):
`World::attach` puts it in its kind's place. `restore_into` decodes every
section first and installs only when all are valid; a held store the save
lacks is `MissingSection`, and `World::restore`, which starts from a world
with no store, refuses a subsystem's section as `UnknownKind`. Test:
`a_held_store_saves_in_its_kinds_place_and_restores_in_place`. There are no migrators: a save belongs to the build that
wrote it.

## Hash

`World::sim_hash` is fnv64 (`src/hash.rs`) over `(kind, version, section
hash)` for each section in order, where a section hash is fnv64 of the
section's bytes. A pool caches its section hash until a `&mut` borrow
(`alloc`, `free`, `get_mut`), so a tick that touches one store re-encodes only
that store. The cli's own fnv64 copies are private; this one is shared.

## RNG

`Rng` (`src/rng.rs`) is the one shared stream, retail's generator at
`[0x4e7408]`: `seed = seed * 0x19660d + 0x3c6ef35f` mod 2^32, and a draw
returns the new seed. The world has one; callers draw in slot order. Its
state is the first section. The unit test `rng_stream_is_golden` pins the
first draws and an fnv64 of the first 1,024 from seed 7.

## Store slots

`kind` names the reserved sections: objects, physics heap, animation, players,
sound, effects, AI, scripts and the extension records. Each is filled by its own slice; until then it
writes no section and a save that carries one is refused as `UnknownKind`.

| Kind | Store | Pools | Held by |
|---|---|---|---|
| sound | `sim_sound::instances::Sounds` ([sound-instances.md](sound-instances.md)) | instances (384), loops (128), starts (16), and the chooser | `sim_bridge::sim_sound::entries::SoundTable`, every run; the `World` holds it as `world::SoundStore` |
| objects | `sim_objects::ObjectTable` (`crates/rewrites/sim-objects/src/table.rs`) | the objects array (capacity from the guest array, 2048 in retail), each slot a header and its body bytes | `sim_bridge::world::OBJECT_TABLE`; the `World` holds it as `world::ObjectStore` |
| ext | `sim_world::ext::State` (`crates/rewrites/sim-world/src/ext`) | the rules VM and Forge records, and one copy of both per checkpoint slot | `sim_world::ext::global`; the `World` holds it as `ext::store::ExtStore`, attached by `sim_bridge::world` |
| lights | `sim_objects::LightTable` (`crates/rewrites/sim-objects/src/lights.rs`) | the lights array (`[4e030c]`, `0x110`-byte elements), each slot the element bytes | `sim_bridge::world::LIGHT_TABLE`, imported with the objects; the `World` holds it as `world::LightStore` |
| effects | `sim_objects::EffectTable` (`crates/rewrites/sim-objects/src/effects.rs`) | the effects array (`[4ea93c]`, `0x190`-byte elements), each slot the element bytes | `sim_bridge::world::EFFECT_TABLE`, imported with the objects; the `World` holds it as `world::EffectStore` |
| scripts | `sim_script::ScriptTable`, holding `hs::vm::VmState` (`crates/rewrites/sim-script/src/table.rs`) | the thread pool (`0x140` slots, salts and allocation cursor kept), each thread's frames, the global cells and the host strings | `sim_script::ScriptRuntime` updates it each tick; the `World` holds it as `sim_script::ScriptStore` |
| test | `Probe` | one | the unit tests |

### The object table

`ObjectTable` is retail's objects datum array as a `Pool<ObjectRecord>`:
`alloc` is `datum_new` (`0x16b840`), `delete` is `datum_delete` (`0x16ba40`),
and a record is the header (flags, type, cluster, body size) and the body
bytes with the address the guest's object memory pool put them at, which is
state. `ObjectTable::import` reads the guest array in (its `next_salt` and
`free_scan_start` too, since the next handle is part of the truth);
`scatter` writes the pool back to the arena (headers, bitmap, allocation
state, bodies) for the guest readers that remain.

Readers do not change. `PoolMem` shows the table as guest memory (the
objects global, the array header, headers, bitmap, and bodies at their
real addresses), so `sim_objects::Objects`, `sim-state`'s `objects` and
`object` read the pool when handed `PoolMem::new(&table)`. The guest's own writers (`object_new` `b7b40`, the deletes `b83b0`
and `b8460`, the memory collection `bf380`, the flag writes) still write the
arena, so until each is a Rust body the table is filled by `import` and
nothing drives it. A checkpoint is
the world's section plus the arena's machine snapshot, taken in one call
(`SimWorld`'s snapshot section); `ObjectStore::install` puts the decoded
pool in the table and the arena returns with its snapshot. Tests:
`slots_and_salts_follow_datum_new`, `import_reads_the_arena_and_scatter_writes_it_back_bit_for_bit`,
`the_views_read_the_pool_as_they_read_the_arena` (`sim-objects`),
`the_object_table_reads_as_the_arena_does` (`sim-state`),
`the_world_saves_the_object_table_and_restores_it_into_its_table` (`sim-bridge`).

A subsystem whose records live here is the truth for them. Where guest code
still reads that state, `sim-bridge` is the only place that touches guest
memory for it: the sound readers are `sim-bridge/src/sim_sound/`.

The physics heap will be a byte store with the retail allocation order kept,
since Havok is ported bit for bit ([sim-rewrite-havok.md](sim-rewrite-havok.md)).
Its kind is reserved, and a pool's section already keeps free slots in place.

## Rules

No `HashMap` iteration feeds saved or hashed output, and `mul_add` is banned by
`clippy.toml` (the float rules in [sim-rewrite.md](sim-rewrite.md)).
