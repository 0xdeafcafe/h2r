# Scenario editor (`crates/scenario-edit`)

A Sapien-style placement editor for Halo 2 cache files. It reads the
scenario's placement blocks, lets you move, rotate, retarget, delete and
add entries, and writes a new `.map`: the source byte for byte, except the
edited fields, the counts and pointers of changed blocks, any block that
grew (appended to the metadata), the header sizes that grow with it, and
the header checksum. It never writes the source, and nothing
from the disc goes into the repository.

Evidence labels are those of [forge.md](../game/forge.md): **E** read from the
retail XBE with a disassembler, **V** checked over every scenario on the
owner's disc (the ignored test `disc_scenarios_read_and_fields_are_plausible`),
**C** the public Halo 2 scenario layout, used only as a cross-check.

The disc test read all 27 campaign and multiplayer scenarios on the owner's
disc. Every palette index is -1 or inside its palette. Every tag reference
resolves with its stated group. Netgame items are collections or object
groups, and palette entries name their block's group. Rotations and facings
are finite and within ±7 radians, and teams and flag types are small. 14,167
of 14,194 positions lie within 10 units of the drawn BSP bounds. The other 27
are campaign scenery, machines and vehicles placed beyond the drawn level.
Addresses are guest virtual addresses of the 1.0 XBE.

## 1. Blocks

The `scnr` root holds each block as a `(count, address)` pair. Every block
this editor reads lies inside the scenario tag's own payload, or (a block
it grew, §6) in the ordinary metadata past every tag; a block anywhere
else, or two placement blocks that share bytes, make the read fail.

| Block | `scnr` | Stride | Stride evidence |
|---|---|---|---|
| Scenery / palette | `+0x50` / `+0x58` | `0x5c` / `0x28` | E, object type table `0x00468630` ([objects.md](../formats/objects.md)) |
| Vehicles / palette | `+0x70` / `+0x78` | `0x54` / `0x28` | E, same |
| Equipment / palette | `+0x80` / `+0x88` | `0x38` / `0x28` | E, same |
| Weapons / palette | `+0x90` / `+0x98` | `0x54` / `0x28` | E, same |
| Machines / palette | `+0xa8` / `+0xb0` | `0x48` / `0x28` | E, same |
| Sound scenery / palette | `+0xd8` / `+0xe0` | `0x50` / `0x28` | E, same |
| Crates / palette | `+0x328` / `+0x330` | `0x4c` / `0x28` | E for `0x4c`; palette stride V |
| Player starting locations | `+0x100` | `0x34` | **E**: `0x0014ef96` `imul eax,34h` on `[scnr+0x104]`; `0x0014bcb8` steps `0x34` |
| Netgame flags | `+0x118` | `0x20` | **E**: `0x0019cbcb`, `0x00241875` `shl eax,5` on `[scnr+0x11c]` |
| Netgame equipment | `+0x120` | `0x90` | E: `0x0015aeea` `(i*9)<<4` |
| Starting equipment | `+0x128` | `0x9c` | **E**: `0x0019efa6` `add ebx,9Ch` per entry of `[scnr+0x12c]` |

The three strides forge.md listed as "at most" (starting locations,
netgame flags, starting equipment) are now confirmed by engine loops.

## 2. Fields

Only these fields are typed. Every other byte of an element is kept as
read and is never written.

**Object placements** (scenery, vehicles, equipment, weapons, machines,
sound scenery, crates) share a head:

| Offset | Field | Type | Editable | Evidence |
|---|---|---|---|---|
| `+0x00` | palette index | i16, -1 = none | yes, -1 or a palette entry | V, C |
| `+0x02` | name index | i16 | no | C |
| `+0x04` | placement flags | u32 | yes (raw) | C |
| `+0x08` | position | 3 × f32 | yes | V ([live-art.md](../live/live-art.md) machines; 99.8% inside the BSP bounds on disc), C |
| `+0x14` | rotation: yaw, pitch, roll (radians) | 3 × f32 | yes | V (live-art.md), C |
| `+0x20` | scale, 0 meaning 1 | f32 | yes, ≥ 0 | V (live-art.md), C |

A palette entry starts with an 8-byte tag reference: the group as a
little-endian FourCC, then the full salted tag ID.

**Player starting locations** (`0x34`): position `+0x00` (E `0x0023cec7`),
facing `+0x0c` f32 (E `0x0023cee1`), team `+0x10` i16 (V, C), player type
`+0x2c` i16 (E: `0x0014bcb0` and `0x001529d9` compare it with -1 and 1;
read-only here).

**Netgame flags** (`0x20`): position `+0x00` (E `0x0019cbce`..`0x0019cbd9`),
facing `+0x0c` (E `0x0019cf65`), type `+0x10` u16 and team `+0x12` u16
(V, C).

**Netgame equipment** (`0x90`), all read in the spawner `0x0015aad0` and
its tick `0x0015ae70`:

| Offset | Field | Evidence |
|---|---|---|
| `+0x00` | flags (u32; bit 0 read at spawn) | E |
| `+0x0e` | respawn time, i16; 0 = the collection's own (`itmc`/`vehc` `+8`) | E `0x0015ac6d` |
| `+0x10` | abandon limit, i16: the tick counts an object away from its spot and respawns past this | E `0x0015af7d`, `0x0015b020` |
| `+0x12` | respawn mode, i16: 0 and 1 take different paths | E `0x0015b05d` |
| `+0x14` | type byte: 0, 1..7, 8..9 pick the definition | E `0x0015b0ef` |
| `+0x40` | position | E `0x0015afa8`..`0x0015afbc` |
| `+0x4c` | orientation, 3 × f32 | E (offset); V |
| `+0x58` | item: tag reference, a collection or an object | E `0x0015ac83` |

The item may be retargeted to any tag in the map whose group is `itmc`,
`vehc`, or a placeable object group; the reference must name a tag that
exists with that group.

**Starting equipment** (`0x9c`): five tag references at `+0x3c + 8k`
(k = 0..4). The loop at `0x0019efe1` reads the ID half (`+0x40 + 8k`) and
rolls each with `collection_select`. Shown, not editable: the other bytes
(game-type matching at `+0x04`, passed to `0x0015a090`) are not decoded.

## 3. Edit model

- `Scenario::read` keeps each block's elements as raw bytes, plus a copy
  of the source bytes, and the scenario object names block (`scnr+0x48`,
  §6). A block may lie in the scenario's payload or, when this editor grew
  it, in the ordinary metadata past every tag; anywhere else the read
  fails.
- `Scenario::set(block, index, field, value)` validates and writes one
  field: editable only, finite floats, nonnegative scale, integers in
  range for the type, palette index -1 or inside the palette, tag
  references to tags that exist with the stated, accepted group.
- `Scenario::remove(block, index)` swap-removes an entry and repoints the
  object names (§6); `unremove` puts the count, both slots and the names
  back exactly. `Scenario::add(block, bytes)` appends one element;
  `new_entry(block, like, position)` makes one (a copy of `like`, or
  zeroed with the palette's first entry and name index -1) with its
  position set.
- `History` applies `Command`s (one or more `Op`s: set, remove, add) with
  undo and redo. A command that fails part-way is rolled back. Commands
  of field sets with the same merge key in a row (one drag, or typing
  into one field) are one undo step; `seal` ends a merge. Undo restores
  source values without validation, so a damaged source value comes back
  exactly.
- `Scenario::changes` compares current and source and returns `Changes`:
  - `patches`: one `(virtual address, bytes)` per contiguous run of
    changed bytes in blocks that still fit their source space, plus the
    root count of a block whose length changed. A removal leaves the old
    tail element's bytes in place as dead data.
  - `grown`: each block with more entries than its source, as a whole new
    copy.
  Undoing everything gives no changes.
- `save::write_patched` queues the patches through
  `MapEditor::patch_metadata`, appends each grown block with
  `MapEditor::append_metadata` and points its root `(count, address)`
  there, and streams the map; `MapEditor::write_to` grows the header sizes
  and recomputes the checksum at `+0x2d0`. `save::save_as` writes a
  sibling `.partial` file, syncs it and renames it over the destination,
  and refuses to write the source path.

Tests on an invented cache check that an unedited save is byte-identical,
that each editable field changes only its own bytes and the checksum, that
the result validates and reads back, the undo, merge and rollback rules,
that a removal writes only the slot, the count and the names, and that a
grown block changes only its root pair and the header sizes and lands
after the old end of the metadata.

## 4. The session

`scenario_edit::Session` holds the editor's state and every command, with
no UI; `h2-editor`'s scenario tool (`crates/studio`,
[editor.md](editor.md)) draws it:

```rust
pub fn new() -> Self
pub fn open(&mut self, source: MapSource) -> Result<()>   // a .map file, or a map inside an XISO
pub fn open_named(&mut self, source: MapSource, name: &str) -> Result<()>
pub fn set_data_root(&mut self, root: impl Into<PathBuf>) // packages in <root>/maps
pub fn save_package(&mut self, form: &PackageForm) -> Result<SavedPackage>
pub fn packages(&self) -> Vec<content::Package>          // for the open map
pub fn open_package(&mut self, dir: &str) -> Result<content::MapId>
pub fn open_disc(&mut self, iso: &Path) -> Result<()>     // fills the map list
pub fn is_dirty(&self) -> bool
pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<u64>
pub fn undo(&mut self) -> bool
pub fn redo(&mut self) -> bool
pub fn apply(&mut self, command: Command) -> Result<()>
pub fn remove_entry(&mut self, kind: BlockKind, index: usize) -> Result<()>
pub fn add_entry(&mut self, kind: BlockKind, like: Option<usize>) -> Result<usize>
pub fn remove_refusal(&self, kind: BlockKind) -> Option<&'static str>
pub fn add_refusal(&self, kind: BlockKind) -> Option<&'static str>
pub fn set_allow_growth(&mut self, allow: bool)
pub fn begin_gesture(&mut self)                           // a gizmo drag: one undo step
pub fn gesture_set(&mut self, position: Vec3, ypr: Vec3) -> Result<()>
pub fn end_gesture(&mut self)
```

- **Outliner:** blocks, then entries labelled with the base name of their
  palette tag or item, read from the map's own tag names at run time.
  Delete and Duplicate act on the selection; right-click an entry for
  Duplicate / Delete / Add new, or a block header for Add new. A new entry
  is a copy of the selected one (or zeroed with the palette's first item)
  placed at the camera focus. Each is one undo step. A refused action is
  greyed out with its reason on hover.
- **Growth:** adding to a block that is full (as many entries as the
  source) would grow it. That is off by default: the engine's limit is
  not established (§6), so add works only in the room a removal made.
  `set_allow_growth(true)` (the editor's **Edit > Allow growing full
  blocks**) turns it on.
- **Inspector:** each typed field with its offset and evidence (hover);
  vectors and numbers as drag values, palette index and item as pickers.
- **Viewport:** the structure BSPs (through `tags::structure`), textured,
  the placed objects' models, and a glyph per placement, coloured by
  block (editor.md, "What the viewport shows"). Right-drag orbits,
  middle-drag pans, scroll zooms, WASD/QE fly. Click selects; the
  selection's gizmo moves or rotates it (`begin_gesture`, `gesture_set`,
  `end_gesture`), one drag being one undo step.

`render::Viewport` renders offscreen into its own colour and depth
textures; the editor registers the colour texture with Dear ImGui and
shows it as an image, so the viewport's pipelines are independent of the
UI's render pass. The math (camera, rays, picking, rotation) and the
renderer have unit and headless-GPU tests.

The BSP walk repeats `cli::live_geometry::bsp_triangles` in a few lines
(the CLI crate is a binary with many dependencies). Object models come
from `tags::object` (editor.md, "What the viewport shows").

## 5. How a patched map reaches the game

### As a map package (the default)

**Save as package...** writes a map package ([maps.md](../game/maps.md) §1, §8)
into `<data>/maps/<dir>/`, where `<data>` is the runtime's data root
(`h2-editor --data`, default `$TMPDIR/h2boot-data`, as `h2 play`). The
form asks for a name (required), author, description, an optional image
(PNG or JPEG, at most 512 KB, checked before anything is written), the
directory (default `<map>-edit`) and whether to make it the active package
for the map (`active.toml`). The package is:

- `manifest.toml`: `[base]` the disc map's name and SHA-256; one
  `[[patch]]`; `[result] geometry` (maps.md, "The geometry hash");
- the patch, base to current: PPF3 when the map kept its length, BPS
  when a block grew (§6);
- `preview.png` or `preview.jpg`, if given.

The status line and toolbar show the package's `MapId`. The disc and the
source map are only read; the package directory appears whole or not at
all. The runtime serves it as `D:\maps\<map>.map` at the next boot
(maps.md §7): the active one, else the one whose directory sorts last.

**Open package...** lists the packages for the open map in `<data>/maps`
(name, directory, `MapId`). Opening one resolves it over the disc's map
exactly as the runtime does and makes the result the unedited state, so
further edits extend it; it is refused while there are unsaved edits, as
switching maps is. Saving to the same directory replaces its patch with a
new one from the base to the current state, so a package always has one
layer. Saving to another existing directory is refused.

Tests (synthetic maps): the resolved package equals the edited map, PPF3
and BPS; manifest contents and a stable `MapId`; reopen, edit and save
again (one layer, both edits); image validation; `active.toml`; refusal
over unsaved edits. The ignored disc test `disc_package_round_trip` does
the same on the first multiplayer map of the owner's disc.

### As a whole `.map` (export)

Through a map override directory. Save the edited map under the disc's
file name in a directory of your choosing, and start the game with
`H2_MAP_DIR` naming that directory:

```sh
mkdir -p ~/h2-maps
# In the editor: Save as ~/h2-maps/ascension.map (the disc's name; `h2 maps` lists them)
H2_MAP_DIR=~/h2-maps h2 play
```

The game then reads `D:\maps\ascension.map` from that file instead of the
disc: its bytes, its size, and its size in listings of `D:\maps`. Every
other file, and every map without an edited copy, still comes from the
disc. Names match in any case. Stderr says once per map which file was
used (`map override: maps\ascension.map from ...`), or why one was
refused. A refused or missing file is a warning, never a failure: the
disc's map is used. Remove the file (or unset `H2_MAP_DIR`) to go back to
the retail map. The disc image is never touched.
[storage.md](../runtime/storage.md) "Map overrides" has the backend's rules.

The header checksum is checked by the cache copy path (`0x2141f0`,
[map-format.md](../formats/map-format.md), [cache-slots.md](../runtime/cache-slots.md)); the
writer repairs it, so a patched map passes that check, and the copy reads
the override like any disc file. Multiplayer and campaign maps (and the
main menu) are copied into cache slots 2-5 on N:, which is emptied on
every boot, so an edit is always copied afresh. `shared.map` and
`single_player_shared.map` go to slots 0 and 1 on Z:, which persists; the
cache reuses a slot whose header names the same `d:\maps\...` path
without comparing contents, so an override of those two takes effect only
after Z:'s cache files are removed. The scenario editor never edits them.

The cache dependency identities and the build string are passed through
unchanged. Multiplayer peers must all load the same bytes; that is the
variant-hash problem of forge.md §2.4.

## 6. Adding and removing entries

### What refers to entries by index

Read in the XBE (**E**) unless marked:

- **Scenario object names** (`scnr+0x48`, stride `0x24`, at most 640:
  `cmp ax,280h` at `0x000bb765` and five other lookups). `object_create`
  by name (`0x000bb670`, stride from `lea ecx,[eax+eax*8]` at
  `0x000bb702`) reads the object type (i16 `+0x20`, an index into the
  type table `0x00468630`) and the placement index (i16 `+0x22`); -1
  creates nothing (`0x000bb716`). So names point at placements by index.
  The placement's own `+0x02` points the other way, at its name, and
  moves with the element. HaloScript and the runtime keep name indices,
  not placement indices (`0x0011c380`, `0x0029fd80`, `0x0020a130`).
- The placement's object ID (`+0x28`) is matched by value
  (`0x000bf760`), not by index.
- Runtime objects keep their placement index (`+0x1a`); the BSP-switch
  re-create pass `0x000d5640` rebuilds its bitmask from them each load.
  Network object creation sends a placement index (`0x000a73b0`), bounds
  checked against the client's own count. Both are fine when every peer
  loads the same map.
- Player starting locations (`0x0014ef90`, `0x0014bcb0`), netgame flags
  (`0x0019ec40`, results cached per game at `[0x4e9ae8]+0xafc`), netgame
  equipment (`0x0015aad0`, `0x0015ae70`) and starting equipment
  (`0x0019ef64`) are walked from 0 to count; no tag field stores an
  index into them. The netgame equipment index travels as an i8 on the
  spawned object (`0x0015ac15`), rebuilt per game. Ties between starting
  locations go to the lower index, and the first matching starting
  equipment entry wins, so order can matter (S).
- Not disassembled: the AI, device-group and trigger-volume readers. In
  the public layout they hold name or palette indices, or the reference
  runs from the placement to them (S).

### Remove

Every block allows removal. The last element moves into the removed slot
and the count drops (two fixed-size patches; the old tail stays as dead
data). For object placements the names are repointed in the same edit:
the name that named the last placement (same type) now names the slot it
moved to, and a name that named the removed placement now names nothing
(-1, which `object_create` skips). The inspector says when a placement is
named. Undo restores the count, both slots and the names byte for byte.

### Add

New entries are appended. While a block has no more entries than its
source (after a removal), they are written in place. Past that the block
grows: the save appends a full copy of the block to the ordinary
metadata (`MapEditor::append_metadata`, [map-format.md](../formats/map-format.md)
"Growing the metadata") and points the root `(count, address)` at it.
The old elements stay where they were, unreferenced.

Entry limits, enforced on add: object placements use the per-type maximums
at `0x0044074c` (scenery 2000, vehicles 256, equipment 256, weapons 128,
machines 400, sound scenery 256, crates 1024). `0x000d5640` sums them into
one bitmask without checking the index, so more entries would spill into
the next type's bits. Netgame equipment allows 100: its runtime array is
cleared for 100 entries (`0x0015acf4`) and its loops do not check. No
limit was found for starting locations, netgame flags or starting
equipment.

**Why growth is off by default.** The loader (`0x1228f0`) places the
metadata at `base + (+0x1c) - (+0x18)`, so growing both by the same amount
keeps every tag where it was. It allocates `+0x1c` from the low end of the
physical arena (`0x30e4000` bytes at `0x61000`, `0x12b400`) and fails only
when that crosses the arena's high cursor (`0x12296c`). After the load,
`0x12c1e0` gives every page still free to one pool (handed to `0x13d8b0`,
probably the texture and geometry cache, S). So growth comes out of that
pool, and the real limit is how much headroom the high side leaves. That
was not measured statically. The writer bounds `+0x1c` by the largest
value any retail map of the same cache type has (multiplayer `0x62c000`),
which that pool is known to tolerate; on the owner's disc that leaves
waterworks no room and the other multiplayer maps about 0.1 to 1.5 MiB. Whether
`0x13d8b0` degrades with a smaller pool is open. Measuring
`[0x4e6440] - [0x4e642c]` after a load in the runtime would settle it.

The disc test `disc_add_and_remove_round_trip` (ignored by default) adds
a netgame equipment entry and removes a weapon on every multiplayer map,
saves to memory, and re-reads the result with `map` (checksum valid),
`tags::DecodedTag` (scenario) and this crate; only the header sizes, the
checksum, the patches and the block pair differ from the source. On the
owner's disc 11 maps grew and waterworks was refused at the bound.

The runtime alternative from forge.md §2.2 (patch blocks in guest memory
after load) avoids rewriting the file and has the same memory question.

## 7. Open questions

- Netgame flag type and team, and starting-location team, rest on V and
  C only; an engine read would make them E.
- The placement head beyond scale (`+0x24` onward: transform flags, BSP
  policy, unique ID, editor folder in the public layout) is preserved but
  not typed.
- The netgame equipment game-type bytes at `+0x04` and the meaning of
  respawn mode 0 and 1 are not decoded.
- The AI, device-group and trigger-volume readers have not been checked
  for placement indices (§6, S).
- The engine's real headroom for a larger `+0x1c`, and whether the pool
  that loses it degrades (§6). Until then growth is off by default.
- A campaign checkpoint saved before an edit holds runtime placement
  indices that a removal invalidates; only fresh loads are safe.
