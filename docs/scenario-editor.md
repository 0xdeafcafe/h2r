# Scenario editor (`crates/scenario-edit`)

A Sapien-style placement editor for Halo 2 cache files. It reads the
scenario's placement blocks, lets you move, rotate and retarget entries,
and writes a new `.map`: the source byte for byte, except the edited
fields and the header checksum. It never writes the source, and nothing
from the disc goes into the repository.

Evidence labels are those of [forge.md](forge.md): **E** read from the
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
this editor reads lies inside the scenario tag's own payload; a block that
does not, or two placement blocks that share bytes, make the read fail.

| Block | `scnr` | Stride | Stride evidence |
|---|---|---|---|
| Scenery / palette | `+0x50` / `+0x58` | `0x5c` / `0x28` | E, object type table `0x00468630` ([objects.md](objects.md)) |
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
| `+0x08` | position | 3 × f32 | yes | V ([live-art.md](live-art.md) machines; 99.8% inside the BSP bounds on disc), C |
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
  of the source bytes.
- `Scenario::set(block, index, field, value)` validates and writes one
  field: editable only, finite floats, nonnegative scale, integers in
  range for the type, palette index -1 or inside the palette, tag
  references to tags that exist with the stated, accepted group.
- `History` applies `Command`s (one or more sets) with undo and redo. A
  command that fails part-way is rolled back. Commands with the same
  merge key in a row (one drag, or typing into one field) are one undo
  step; `seal` ends a merge. Undo restores source values without
  validation, so a damaged source value comes back exactly.
- `Scenario::patches` compares current and source bytes and returns one
  `(payload offset, bytes)` per contiguous run of changed bytes. Undoing
  everything gives no patches.
- `save::write_patched` queues those through `MapEditor::patch_tag` and
  streams the map; `MapEditor::write_to` recomputes the checksum at
  `+0x2d0`. `save::save_as` writes a sibling `.partial` file, syncs it and
  renames it over the destination, and refuses to write the source path.

Tests on an invented cache check that an unedited save is byte-identical,
that each editable field changes only its own bytes and the checksum, that
the result validates and reads back, and the undo, merge and rollback
rules.

## 4. The panel

`ScenarioEditor` (egui 0.37, egui-wgpu 0.37 on wgpu 30):

```rust
pub fn new() -> Self
pub fn open(&mut self, source: MapSource) -> Result<()>   // a .map file, or a map inside an XISO
pub fn open_disc(&mut self, iso: &Path) -> Result<()>     // fills the map picker
pub fn ui(&mut self, ui: &mut egui::Ui, render: &egui_wgpu::RenderState)
pub fn is_dirty(&self) -> bool
pub fn save_as(&mut self, path: impl AsRef<Path>) -> Result<u64>
pub fn undo(&mut self) -> bool
pub fn redo(&mut self) -> bool
pub fn apply(&mut self, command: Command) -> Result<()>
```

- **Toolbar:** a path box (a `.map`, or an `.iso` to list its maps), undo,
  redo, save-as path. Cmd/Ctrl+Z undoes, Cmd+Shift+Z or Cmd+Y redoes.
- **Outliner:** blocks, then entries labelled with the base name of their
  palette tag or item, read from the map's own tag names at run time.
- **Inspector:** each typed field with its offset and evidence (hover);
  vectors and numbers as drag values, palette index and item as pickers.
- **Viewport:** the structure BSPs (clusters and instanced geometry,
  through `tags::structure`) and a diamond per placement pointing along its
  facing, coloured by block. Right-drag orbits, middle-drag pans, scroll
  zooms, WASD/QE fly. Click selects; dragging the selection moves it on
  the horizontal plane, Shift-drag moves it vertically. One drag is one
  undo step.

The viewport renders offscreen into its own colour and depth textures
and shows the result as an egui image (a native texture registered with
`egui_wgpu::Renderer`). A paint callback would draw inside egui's render
pass, whose depth buffer and MSAA count belong to the shell; a pipeline
has to match both, so the offscreen target keeps the panel independent
of how the shell configures eframe. The math (camera, rays, picking,
rotation) and the renderer have unit and headless-GPU tests.

The BSP walk repeats `cli::live_geometry::bsp_triangles` in a few lines
(the CLI crate is a binary with many dependencies). `crates/viewer` is
also binary-only. Moving both into a small shared library would let the
editor draw object render models too.

## 5. How a patched map reaches the game

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
[storage.md](storage.md) "Map overrides" has the backend's rules.

The header checksum is checked by the cache copy path (`0x2141f0`,
[map-format.md](map-format.md), [cache-slots.md](cache-slots.md)); the
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

`MapEditor` only queues fixed-size patches inside a tag's payload; it
cannot move or grow a block. What each operation needs:

- **Remove, in place (no writer change):** swap the last element into the
  removed slot and decrement the block's count in the `scnr` root (two
  patches). The tail element's old bytes stay as dead data. Safe for the
  netgame and spawn blocks, whose readers only walk `count` entries (E:
  the loops above). For object placements, check first that nothing
  refers to placements by index (scenario object names at `+0x02` point
  the other way, from name to placement; HaloScript and AI may hold
  indices; S). A cheaper "disabled" convention (palette index -1 for
  objects) is plausible for object placements, since the loader skips a
  -1 type (S, to confirm in the placement loader before relying on it).
- **Add, using slack (no writer change):** only if the block happens to
  be followed by bytes no other structure uses, which cannot be known
  without decoding every block of the tag. Not offered.
- **Add, in general (writer change):** `MapEditor` needs an operation
  that appends bytes to a tag and returns their virtual address, so a
  block can be copied to the end of the tag with room to spare and its
  `(count, address)` rewritten. Concretely:
  - grow the tag payload: write the new bytes after the tag's last byte,
    update the tag entry size in the index, and shift every later byte of
    the metadata region (or append at the end of the metadata region and
    move the scenario tag there);
  - fix every metadata pointer into the shifted range. That requires
    knowing every pointer in every later tag, which is the full tag-graph
    relocator `writer.rs` says it is not. Appending at the end of the
    metadata region and relocating only the one scenario block avoids
    shifting others, but the region's declared size (`+0x18`), the cache
    size limits and what the engine maps after the metadata (S) all need
    checking;
  - `scnr` is the first tag the engine reads after load
    (`load_requested_scenario_cache` `0x0018eaa0`), and the engine's
    metadata region is a fixed-size guest allocation, so growth must stay
    within it (S, to measure).

  The runtime alternative from forge.md §2.2 (patch blocks in guest memory
  after load, with new blocks in a guest range the runtime reserves) avoids
  rewriting the file at all, and may be the cheaper route to "add".

## 7. Open questions

- Netgame flag type and team, and starting-location team, rest on V and
  C only; an engine read would make them E.
- The placement head beyond scale (`+0x24` onward: transform flags, BSP
  policy, unique ID, editor folder in the public layout) is preserved but
  not typed.
- The netgame equipment game-type bytes at `+0x04` and the meaning of
  respawn mode 0 and 1 are not decoded.
- Whether any engine code indexes object placements in a way that a
  swap-remove would break.
