# The simulation world: pools, save, hash and RNG

Status: `crates/rewrites/sim-world` is built with unit tests. Sound is the
first subsystem on its pools (`sim_sound::instances::Sounds`); no `World`
holds it at boot yet, and no other store is filled.

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
`World::restore_into` replaces `self` only on success, so a refused restore
changes nothing. There are no migrators: a save belongs to the build that
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
sound, effects, AI and scripts. Each is filled by its own slice; until then it
writes no section and a save that carries one is refused as `UnknownKind`.

| Kind | Store | Pools | Held by |
|---|---|---|---|
| sound | `sim_sound::instances::Sounds` ([sound-instances.md](sound-instances.md)) | instances (384), loops (128), starts (16), and the chooser | `sim_bridge::sim_sound::entries::SoundTable`, behind `impl.sim.sound-events.table` |
| test | `Probe` | one | the unit tests |

A subsystem whose records live here is the truth for them. Where guest code
still reads that state, `sim-bridge` is the only place that touches guest
memory for it: the sound readers are `sim-bridge/src/sim_sound/`.

The physics heap will be a byte store with the retail allocation order kept,
since Havok is ported bit for bit ([sim-rewrite-havok.md](sim-rewrite-havok.md)).
Its kind is reserved, and a pool's section already keeps free slots in place.

## Rules

No `HashMap` iteration feeds saved or hashed output, and `mul_add` is banned by
`clippy.toml` (the float rules in [sim-rewrite.md](sim-rewrite.md)).
