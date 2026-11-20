# Guest layout, switch table and point shapes

Change 5 of the [November 2026 architecture review](../architecture-review-2026-11.md)
(sections 2.6 and 3.4): one place for every guest offset and global
address, one bounded table of `impl.*` switches, and hook points that fail
by name when their address is wrong.

## `crates/hooking/guest-layout`

A crate with few dependencies (`paste`; `runtime` only behind its
`runtime-glue` feature), so bots, telemetry, the agent API, the engine and
the native renderer can all read guest structures without pulling each
other in.

A structure is declared once with `layout!`:

```rust
guest_layout::layout! {
    /// A unit's body (bipeds and vehicles), past the shared object fields.
    pub struct Unit: 0x2c8 {
        /// The unit's player datum, or none (confirmed).
        PLAYER player @ 0x13c: Datum,
        WEAPON_INDEX weapon_index @ 0x212: i8,
        WEAPONS weapons @ 0x218: [Datum; 4],
    }
}
```

It generates the view (`Unit { addr }`, `Unit::at`), a typed offset per
field (`Unit::PLAYER: Off<Datum>`, `Unit::PLAYER.offset` for code that
still adds by hand), a getter per field generic over any `Mem`
(`unit.player(m)?`), `get`/`set` with an `Off`, and `FIELDS`, which the
crate's tests check for overlaps and bounds.

- **Memory.** `Mem` and `MemMut` are two byte methods, implemented for
  `dyn GuestMemory`, `FakeGuest` and `FlatMemory`; `MemFault` is
  `Fault`. Through a concrete memory a
  field read is a bounds check and a load; through `dyn` it is one
  virtual call, as before. `FlatRam` is a flat region for tests and for
  copied-out state.
- **Layouts today.** `object` (header and its flag bits, shared body and
  its physics flag bits, the definition's bounds, `ObjectType`), `unit`
  (unit, biped, weapon, device), `player`, `game` (game globals, options,
  variant, game time, player globals, engine statistics), `datum_array`
  (a datum array's header), `tag` (the tag index and its entries).
- **Globals.** `global` is the one table of global addresses; `points`
  re-exports it unchanged. New globals go here.
- **The guest boundary and the points.** `memory` (`GuestMemory`,
  `FlatMemory`, `MemFault`), `testing` (`FakeGuest`), `handle`, `datum`,
  `tags`, `view` (`record!`) and `points` (the one table of named guest
  functions and globals, `points!`) are this crate's. `engine` re-exports
  each module and macro, so `engine::points`, `engine::GuestMemory` and
  `engine::record!` name the same items. With `runtime-glue`,
  `GuestMemory` has `take_write_watch` and the runtime's `Guest` and
  `WritebackView` implement it (`src/memory.rs`). The engine's async
  entries (`engine::entry`) stay in `engine`
  ([crate-structure.md](../dev/crate-structure.md), S2).
- **Readers.** `view`'s player and object records take their
  offsets from the layouts; `live_telemetry::layout` keeps its names but
  every object, unit, biped, game and global value comes from here;
  `agent::sim::state` reads and writes through `Unit` and `Weapon`.
- **What sits on top.** `sim-objects` walks the objects pool over these
  views (live-slot scan, handles, child links, the definition table), and
  `sim-state` builds `ObjectView` and `UnitView` on it
  ([sim-rewrite-objects.md](sim-rewrite-objects.md) section 8); both add
  typed ids and kinds, not new offsets.
- **Raw offsets.** `tests/raw_offsets.rs` counts `+ 0x..` and
  `wrapping_add(0x..)` in each reader crate's sources against a budget
  that may only fall.

## The switch table

`crates/hooking/hooks/src/switches/table.rs` lists every `impl.*` switch: its
default (on, off, following `impl.render.native`, or a group), its owning
area, the day it appeared and when it is deleted. `Switches::parse`
refuses an `impl.*` name that is not a row, a parent of one, or an engine
leaf key (`impl.engine.<group>.<function>`). A test scans every crate's
non-test sources so each `impl.*` read is listed and each literal default
matches its row. `BUDGET` bounds the count.

## Point shapes

`engine::point_shapes` says what each hook point expects at its address:
a few x86 encoding facts, never the image's bytes. By default a function
point (any signature but `Site`) must start a function (a return or
padding right before it); a site is not checked. `DECLARED` overrides
that per point: call sites are `call rel32` or `call r/m32`, and the few
function points the image reaches by falling through are checked for a
frame prologue or not at all.

At install (`feature::verify`), each chain's point is checked against the
loaded image. A point that is not what it says turns its features off
with its name, address and the bytes found, as a missing original does.
Runtimes with no XBE header at `0x10000` (synthetic tests) are not
checked. `crates/rewrites/engine/tests/point_shapes.rs` checks the whole table
against the user's XBE:

```text
H2_XBE=/abs/path/default.xbe cargo test -p engine --features differential --test it point_shapes::
```
