# Maps: new, edited, patched and shared

Status: Partly built. The package loader, the editor's package tools, the multiplayer slots, Live sharing and the in-game Mods screens are built; SPPF imports convert to standard patch layers. Applying `map-variant` content packs and decoding Serenity deltas remain open.
Code: `crates/formats/content` (manifest, `MapId`, patch readers, resolver), `crates/runtime/vfs/src/packages.rs`, `crates/extensions/game/src/new_maps.rs`, `crates/live/live-hle/src/maps.rs`, `crates/extensions/game/src/mods_screen.rs`. Tests: `crates/formats/content`, `crates/runtime/vfs/tests/packages.rs`, `crates/extensions/game/src/new_maps/tests.rs`.
Switch: `H2_MAP_DIR` (developer override for `\maps\<name>.map`), listed in [switches.md](../runtime/switches.md).

A map is a base plus layers, held as a package under `<data>/maps/`. A
package gets its own slot in the multiplayer map list if there is room.
Retail and Map Pack slots keep their original paths. The VFS resolves the
package and applies its file patches when the game opens the mod path.
See [multiplayer map selection](map-select.md) for the menu.

The editor saves edited `.map` files. The resolver validates and lists
map-variant packs; applying those packs at load is design only.
[Scenario editor](../editor/scenario-editor.md) ·
[Storage](../runtime/storage.md#map-overrides) ·
[Feature engine](../engine/feature-engine.md)

The goals, in the user's words: simple; new maps, edited maps and patch
files (PPF, SPPF and the like) applied by the Rust runtime itself at boot;
realtime; and later sharing patches and maps over the network so players
can join games on maps they don't have. Also: the 2005 Map Pack maps
playable.

Nothing here puts game data in the repo. Packages name things on the
user's own disc by name and hash; tests use synthetic maps.

## 1. One idea: a map is a base plus layers

Every map the game loads is resolved from:

- a **base**: a retail map on the user's disc (or map pack), named and
  checked by SHA-256; or a whole user-made `.map` (no retail base);
- zero or more **file patches**, applied in order to the base's bytes:
  PPF, IPS or BPS, including those converted from SPPF or VCDIFF at import;
- zero or more **content packs** of kind `map-variant`
  ([feature-engine.md](../engine/feature-engine.md)): field-level edits such
  as spawn or weapon placement. Packages list and validate these layers.
  Applying them to loaded tag data is planned.

A **map package** is the manifest that lists those, plus display data:

```toml
[package]
format = 1
name = "Lockout Snipers"
author = "…"                 # Live account id when published
description = "…"
image = "preview.png"        # optional, PNG/JPEG ≤ 512 KB

[base]                       # always present when derived from retail
map = "lockout"
sha256 = "…"                 # of the user's disc map; never committed

[[patch]]                    # file layer, in order
file = "snipers.ppf"
sha256 = "…"

[[pack]]                     # tag layer, in order
file = "spawns.toml"
sha256 = "…"

[result]
geometry = "…"               # hash of the structure BSP sections after patching
```

### The geometry hash

`[result] geometry` is the SHA-256 of this byte stream, built from the
patched map (`scenario_edit::package::geometry_hash`). For each entry of
the scenario's structure BSP block (`scnr+0x210`, stride `0x44`), in
order:

1. If the entry's file offset (`+0x00`) is 0: a `u32` 0, and nothing
   more for this entry. Otherwise the `u32` section size (`+0x04`), then
   the BSP section's bytes, `[offset, offset + size)` of the map file.
2. Then each geometry block of that BSP's `sbsp` tag (`+0x14`): its
   clusters in order, then its instanced geometry definitions in order.
   For each:
   - no block: the byte `0x00`;
   - a block in the map's own file: the byte `0x01`, the `u32` block
     size, then the block's bytes;
   - a block in a shared cache: the byte `0x02`, the `u32` raw pointer
     word (file selector in the top two bits, offset below), then the
     `u32` block size.

Integers are little-endian. A map with no BSP entries hashes no bytes.
Placement edits and metadata appended past every tag change none of
these bytes, so an editor package reports its base's geometry and the 3D
viewer can use the base's.

A user-made map with no retail base has `[base]` naming the map it was
derived from, if any, with `derived = true`, and the map file itself as
its one layer.

### Identity: `MapId`

`MapId` is the SHA-256 of the manifest's canonical JSON (`live-proto::jcs`),
minus display fields (name, description, image). So two players have the
same map exactly when they have the same base and the same layers in the
same order, and a server can tell a client exactly which pieces it lacks.
Retail maps have a `MapId` too: a manifest with only `[base]`.

The hashed identity (`content::MapId`) is

```json
{"base":{"derived":false,"map":"lockout","sha256":"<hex>"},"format":1,"pack":["<hex>"],"patch":["<hex>","<hex>"]}
```

canonicalised by RFC 8785: the package's display fields (name, author,
description, image) are dropped, and so are the layer file names and
`[result]`, since neither changes the bytes served. The map name is
lowercased, hex is lowercase, `sha256` is omitted for a derived map
without one, and layers keep manifest order. `MapId::retail(name,
sha256)` is the id of the base-only manifest. It displays and serialises
as 64 lowercase hex digits. The canonical JSON is a copy of
`live-proto::jcs` restricted to integers (manifests hold no floats),
because live-proto will depend on `content` for `MapId`; its tests replay
jcs's vectors. Golden vectors on synthetic hashes pin the id.

The types live in `crates/formats/content` (manifest, `MapId`, patch format
readers, resolver), which vfs, boot, editor, live-proto and live-server
depend on.

A local [Forge package](forge-design.md#materialising) uses format 2
and one `[[forge]]` layer. The layer names the document hash
(`document`), the canonical `.h2forge` file (`document_file`), and the
materialised `.map` (`file`, `sha256`). The ID hashes the canonical
JSON `{"base":"<base hash>","forge":["<document hash>"],"format":2}`.
A document whose objects have game types is built for one engine, and
its layer names that engine's variant index (`game`, 0 to 15), which
the canonical JSON carries after `format`:
`{"base":"<base hash>","forge":["<document hash>"],"format":2,"game":2}`
(`MapId::forge_game`; [forge-map-variants.md](forge-map-variants.md),
"Game types"). Every other document has no `game` and keeps its id.
The document hash omits the entire `expect` block. The resolver checks
the document, the expected MapId when present, the disc's copy of the
base, and the local map file. These packages require explicit selection
and stay local. Format 1 uses the identity above.

### The package directory

```text
<data>/maps/
  active.toml              legacy editor choices, ignored by the game
  lockout-snipers/         one directory per package (any name)
    manifest.toml          the manifest above
    snipers.ppf            its layers, packs and image
  cache/<MapId>.map        materialised maps
```

Directories starting with `.` and `cache` are not packages. A package
whose manifest fails to parse or validate is skipped with a warning. The
generated offers in section 4 give packages their own paths and slots; the
game ignores `active.toml`.

**Manifest checks.** `format` must be 1; `name` non-empty; `[base] map` a
plain name (letters, digits, `_`, `-`); `sha256` present unless
`derived = true`, and a derived map needs at least one `[[patch]]` (its
map file, first); hashes are 64 hex digits (either case, stored
lowercase); every file name is relative with no `..`, root or backslash,
and is opened only if, links followed, it stays in the package directory;
the image is `.png`/`.jpg`/`.jpeg`, starts with a PNG or JPEG signature
and is at most 512 KB. Unknown keys are refused.

**Variant packages.** A manifest may hold `[[variant]]` tables, each a
whole map file with its SHA-256 and an `origin` (`content::Variant`); the
converted Halo 1 maps are the user, one variant per sandbox.
`Manifest::select_variant` turns one variant into a plain derived
manifest, so each has its own `MapId` (`origin` is in no id), and the
resolver loads the default variant (`h1` if present, else the first). The
form and the package layout are in
[multiplayer.md](../halo1/multiplayer.md#converting-a-map-built-h1convertconvert-h2-h1-convert).
No setting picks a variant.

## 2. Applying layers at load

The game opens a mod's own content path. The VFS resolves the manifest
against the retail base or the package's whole-map layer on first open
(`content::resolve`):

- Each layer is read and its SHA-256 checked. The format comes from its
  signature (`PPF`, `PATCH`, `BPS1`); a file with none ending in `.map`
  is a whole map, which replaces everything before it and becomes the
  base. The CLI converts SPPF and VCDIFF before the package reaches the VFS.
- **Overlay** (only PPF and IPS after the base): the layers compose into
  sorted, non-overlapping byte runs over the base. One pass over the base
  checks its SHA-256 (the disc's map against `[base] sha256`, or the whole
  `.map` layer against its own hash) and computes the copy-time checksum
  of the patched bytes, which is then written at +0x2d0 as a last run,
  when the result starts with the `daeh` header and is at least 0x800
  bytes. Reads lay the runs over the base as they stream; nothing is
  copied. A PPF or IPS record past the base's end grows the file (zeros
  where nothing is written), as ApplyPPF and IPS tools do, up to 4 GiB
  (`content::patch::MAX_LEN`); IPS may also truncate it.
- **Materialised** (any BPS layer): the result is built in memory once,
  checksum repaired, and written atomically (a temporary file, then a
  rename) to `<data>/maps/cache/<MapId>.map`. The ID includes the expected
  base SHA-256 and the ordered layer hashes. A later resolution of the
  same `MapId` uses the file after verifying the base hash. After each
  write the cache is trimmed to its cap (`DEFAULT_CACHE_CAP`, 4 GiB) by
  removing the least recently used maps (by modification time, refreshed
  on each use), never the one just built.
- For an aliased mod, the VFS serves the alias in the header's short-name
  field, outside the checksum. The original layer files stay intact.
- Content packs are hash-checked and listed (`Resolved::packs`) for the
  feature engine; nothing applies them.
- A wrong base or damaged layer raises a load error at the mod path.

In the VFS the registered map resolves over the read-only disc region or
the installed Map Pack base. `stat` gives its resolved size and `read_at`
streams its bytes. `Vfs::map_id` reports the manifest's content ID for the
generated path. Host tools can resolve the original package and use its
BPS cache file. A resolution reads a patched map's base once, in full, per
session, since the overlay's checksum pass needs the bytes. The base's hash
is kept between runs for the Live gate (section 5, `disc-hashes.toml`).
That read runs inside the `Vfs` lock, so every other file read waits for
it. `Vfs::resolves_on_use` (`Backend::resolves_on_use`, forwarded by
every wrapper) says whether a first look at a path would resolve a
package; background readers skip such a map and leave the resolution to
the game's own first open (the bots' on-demand nav lookup, `game::bots_nav_auto`,
[bots.md](bots.md) section 10). Test: `resolves_on_use_is_true_only_before_a_packaged_maps_first_look`
in `crates/runtime/vfs/tests/packages.rs`.

### Patch formats

| Format | Era | Shape | Status |
|---|---|---|---|
| PPF 1/2/3 | 2004- | absolute-offset byte runs; PPF3 has 64-bit offsets and an optional undo block | **Built**: block check, PPF2 input size, PPF3 undo data checked against the base; FILE_ID.DIZ read; records past the end grow the file, as ApplyPPF's seek-and-write does |
| IPS / BPS | generic | offset runs / copy-from-source deltas with CRCs | **Built**: IPS with RLE, growth and the truncation extension; BPS with all four actions and its three CRC32s |
| SPPF ("Super PPF") | 2006 | four independent sections with relative byte replacements | The CLI moves the retail sections to their patched offsets and converts the result to PPF3 or BPS |
| VCDIFF / xdelta3 | generic | source-window deltas | The importer runs `xdelta3` locally and converts the result to PPF3 or BPS |
| Serenity | 2005–07 | tag-aware delta with compliant, non-compliant and NFG modes | The importer reports `unsupported_serenity` for Serenity files, including v3.3 |
| ours | - | a map package (section 1) | this page |

An SPPF starts with four little-endian u32 section lengths. Each section
contains records of two little-endian u32s (offset and length) followed by
the replacement bytes. Offsets are relative to that section in the target.
The importer applies the header edits first. It reads the starts of the fixed
string slots, low-detail bitmap data and tag index from each header, then
copies and edits the four regions. The updated header gives the target's
length. Bounds are checked against each section before the result is diffed
against the owner's base. The decoder follows public record layout and map
header evidence. Keep fixtures synthetic and the original .NET tools' code
out of the repository. [Iron_Forge's description of patch blocking](https://www.halomods.info/viewtopic.php?t=58447)

Serenity's public descriptions explain its modes, and its public metadata
reader extracts names, credits and a preview. The tag reconstruction records
remain undocumented. A locally patched multiplayer map from Serenity can be
imported against the same retail base. [Patch formats][formats] ·
[HaloDepot metadata reader](https://github.com/iBotPeaches/HaloDepot/blob/master/app/Services/Patch/Serenity/SerenityPatch.php)

## 3. Realtime

Live downloads and removals rebuild the generated offers and publish
their paths to the VFS. The game refreshes its content slots on the
next main-menu tick. Packages added by hand are scanned again at boot.
Open files retain their resolution until their last handle closes.

The game's cache matches the copied map's path. A mod's own content
path keeps its cached copy distinct from the retail base. The boot's
`map-slots` feature can discard an obsolete generated path once its
slot is inactive. See [cache slots](../runtime/cache-slots.md#changed-maps).

## Pages

| Page | What |
|---|---|
| [Maps: new maps and the Map Pack](maps-new-maps.md) | New maps and the Map Pack. |
| [Maps: sharing through Live](maps-sharing.md) | Sharing map layers through Live. |
| [Maps: saving packages from the editor](maps-saving.md) | Saving packages from the editor, and what is not verified. |

Each heading below stays here so that sections cited from code and other pages resolve. The text is on the page named under it.

## 4. New maps and the Map Pack

See [Maps: new maps and the Map Pack](maps-new-maps.md#4-new-maps-and-the-map-pack).

## 5. Sharing (Live)

See [Maps: sharing through Live](maps-sharing.md#5-sharing-live).

### Pictures and the 3D viewer

See [Maps: sharing through Live](maps-sharing.md#pictures-and-the-3d-viewer).

### Importing a mod

See [Maps: sharing through Live](maps-sharing.md#importing-a-mod).

### A mod's page

See [Maps: sharing through Live](maps-sharing.md#a-mods-page).

## 8. Saving packages from the editor

See [Maps: saving packages from the editor](maps-saving.md#8-saving-packages-from-the-editor).

## Not verified

See [Maps: saving packages from the editor](maps-saving.md#not-verified).

## Related

See [Maps: saving packages from the editor](maps-saving.md#related).
