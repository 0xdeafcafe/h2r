# Maps: new, edited, patched and shared (design)

Status: design, with the foundation built: `crates/content` (manifest,
`MapId`, PPF/IPS/BPS readers and writers, resolver), `vfs::PackageBackend`,
the boot wiring, and the scenario editor's "Save as package" and "Open
package" (§8). Sections marked **Built** say exactly what exists; see also
§7. Agreed with the Live session (network side) and built on
the feature engine's content packs ([feature-engine.md](feature-engine.md),
"Content packs"). What exists today: the editor (`h2-editor`,
[editor.md](editor.md), [scenario-editor.md](scenario-editor.md)) saves
edited `.map` files, and `H2_MAP_DIR` makes the runtime serve
`\maps\<name>.map` from a host folder ([storage.md](storage.md), "Map
overrides"). Everything below builds on those two.

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
  PPF, SPPF, Serenity, or our own;
- zero or more **content packs** of kind `map-variant`
  ([feature-engine.md](feature-engine.md)): field-level edits (move this
  weapon, change that spawn) applied to the loaded tag data in memory.

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
file = "snipers.sppf"
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

**Built** (`content::MapId`). The hashed identity is

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

The type lives in a small new crate, `crates/content` (manifest, `MapId`,
patch format readers, resolver), which vfs, boot, editor, live-proto and
live-server depend on. It is the hand-off to the Live session.

## 2. Applying layers at boot

The resolver turns a `MapId` into bytes the vfs serves as
`\maps\<name>.map`, generalising today's `MapOverrideBackend`
(**Built**, except content packs, which are listed and not yet applied;
the details are in §7):

- **Offset patches** (PPF records, IPS, and our own) never copy the map:
  the vfs reads the base and overlays the patched ranges as it streams.
- **Block patches** (SPPF, Serenity) and whole-file deltas move data, so
  they are applied once into a content-addressed cache,
  `<data>/maps/cache/<MapId>.map`, and served from there. The cache is
  rebuilt only when the `MapId` changes, and evicted least-recently-used.
- After the file layers, the **header checksum** (`+0x2d0`) is repaired,
  as the modders' "resign" step did and as `map::writer` already does.
- **Content packs** are applied after the loader publishes the tags (the
  feature engine's `loader().patch`, bridged today by an after-hook on
  `0x0018eaa0`).

### Patch formats

| Format | Era | Shape | Status |
|---|---|---|---|
| PPF 1/2/3 | 2004- | absolute-offset byte runs; PPF3 has 64-bit offsets and an optional undo block | **Built**: block check, PPF2 input size, PPF3 undo data checked against the base; FILE_ID.DIZ read |
| IPS / BPS | generic | offset runs / copy-from-source deltas with CRCs | **Built**: IPS with RLE, growth and the truncation extension; BPS with all four actions and its three CRC32s |
| SPPF ("Super PPF") | 2006 | the map split into 4 blocks, so a size change only shifts to the end of its block; can add files to the map | no public spec |
| Serenity | 2005-07 | 35 blocks plus one per tag; embeds author, description and image | no public spec |
| ours | — | a map package (above) | this page |

SPPF and Serenity are documented only at the level above
([Halo 2: Patch Format Types][formats]). Their byte layouts have to be
worked out from patches the user supplies (kept outside the repo, like
game data), never from the old .NET tools' code. Serenity's embedded
author, description and image map onto the package manifest.

## 3. Realtime

- **Now:** a map saved into the maps folder is used at the next map load.
  The runtime watches the folder and the resolver re-hashes on change.
- **Next:** the editor talks to the running game over a local socket.
  Field edits the scenario editor makes (move, rotate, retype, team) are
  exactly a `map-variant` pack's edits, so the editor sends them as a pack
  and the runtime applies them at the feature engine's safe point. The
  edit shows up at once; spawned objects follow on their next respawn, or
  the editor asks for a round restart. Structural edits (add or remove
  entries, file patches) still take a map reload.

## 4. New maps and the Map Pack

1.0 builds its multiplayer map list from the disc's maps plus each
installed content package's `patch.lvl` (version 2). The map pack's own
`patch.lvl` only names Lockout; its new maps are listed in `patch_v3.lvl`,
which only the title update reads ([map-pack.md](map-pack.md)).

So the cheap route for both **new maps** and the **Map Pack**: the vfs
serves a synthesised `patch.lvl` listing the maps we want, inside a
content package under `T:\$c`. If the 2005 DLC maps load in the 1.0
engine, the Map Pack becomes playable without porting the title update
(17-28 days by [title-updates.md](title-updates.md)). Whether they do is
being checked statically (map-pack.md, "Playing the map pack on 1.0").
If they don't, the update port stays the route for DLC, and new maps
still use this path.

## 5. Sharing (Live)

Agreed with the Live session:

- `NewSession` gains `map_id: Option<MapId>` beside `map`, serde-default,
  so old servers and logs read the same bytes. Hopper entries can pin a
  `map_id`. The game's numeric session attributes are untouched.
- Joining: the runtime compares the session's `map_id` with what it has.
  If pieces are missing it downloads them, then joins. If they can't be
  had (the user lacks the retail base, or the content is blocked), the join
  fails with a clear reason before any map load. The custom games browser
  shows "needs download (N KB)".
- The server is a content store keyed by SHA-256 (`PUT`/`GET
  /v1/content/{hash}`), separate from title files: hash checked on upload,
  stored once, immutable. Caps: patches about 32 MiB, user maps about
  100 MiB, a per-account quota. Uploads are signed by the author's key; the
  manifest is a signed document.
- **Only patches, packs and user-made maps are ever shared, never retail
  bytes.** The server refuses blobs whose hash matches a known retail map.
  Players need their own disc for the base.
- Admins can list, hide, delete and block content by hash, toggle uploads
  per account, and see reports.
- Lockstep co-op and the sim hash diverge on any byte difference, so
  peers must have the same `MapId`, not just the same name. Ranked
  playlists allow only retail or admin-pinned `MapId`s. A map served from
  `H2_MAP_DIR` gets its own `MapId`, so a local edit is never announced
  as retail.
- Which map is loaded: `vfs::Vfs::map_id(path)` (each backend answers: a
  package gives its manifest's id, an `H2_MAP_DIR` file the id of a derived
  manifest whose one layer is the whole file) and
  `boot::map_override::map_id(vfs, name)`, given the loaded map's short name
  from the cache header. `None` means the retail disc map.
- Stats and carnage reports record `map_id` as well as the name, so a
  modded Lockout doesn't mix into Lockout's stats.

### Pictures and the 3D viewer

Every package records its base, because most mods change little. Art for a
`MapId` is, first match wins:

1. the package's own image;
2. the map's own preview image, if the mod replaced the one in the map
   (read client-side from the patched map);
3. the base map's art.

The 3D viewer uses the base map's geometry with the mod's placements,
unless `[result] geometry` differs from the base's, in which case the
geometry is extracted from the patched map.

## 6. Plan

| Lane | Work | Depends on |
|---|---|---|
| content | **Built.** `crates/content`: manifest, `MapId`, PPF/IPS/BPS readers, resolver; the vfs serves packages from `<data>/maps/` (with `H2_MAP_DIR` kept as a shortcut) | — |
| editor | **Built** (§8): save as package (one PPF3 or BPS layer against the base, manifest, image, optional `active.toml`), open packages; `map-variant` packs not yet | content |
| map list | synthesised `patch.lvl` for new maps (and the Map Pack, if 1.0 loads it) | the 1.0 research |
| live | `map_id` in sessions, hoppers and telemetry; the content store; join-time download; admin; art keys | content (`MapId`) |
| realtime | folder watch, then the editor-to-game socket for `map-variant` edits | feature engine loader hook |
| formats | SPPF and Serenity readers | sample patches from the user |

## 7. What is built

`crates/content` (no game data; every test uses synthetic maps and
patches), `vfs::PackageBackend`, and the boot mount
(`crates/boot/src/map_override.rs`).

**Layout.**

```text
<data>/maps/
  active.toml              optional: which package serves which map
  lockout-snipers/         one directory per package (any name)
    manifest.toml          the manifest of §1
    snipers.ppf            its layers, packs and image
  cache/<MapId>.map        materialised maps
```

Directories starting with `.` and `cache` are not packages. A package
whose manifest fails to parse or validate is skipped with a warning.

**Manifest checks.** `format` must be 1; `name` non-empty; `[base] map` a
plain name (letters, digits, `_`, `-`); `sha256` present unless
`derived = true`, and a derived map needs at least one `[[patch]]` (its
map file, first); hashes are 64 hex digits (either case, stored
lowercase); every file name is relative with no `..`, root or backslash,
and is opened only if, links followed, it stays in the package directory;
the image is `.png`/`.jpg`/`.jpeg`, starts with a PNG or JPEG signature
and is at most 512 KB. Unknown keys are refused.

**Choice rule: one package per map name.**

1. `active.toml` decides when it names the map: `lockout = "dir"` picks
   the package in `<data>/maps/dir`; `lockout = false` the disc's map. If
   the named package is missing, broken or for another map, the disc's map
   is served with a warning, never another package.
2. Otherwise, of the valid packages for the name, the one whose directory
   name sorts last wins (so `lockout-2026-11-16` beats `lockout-2026-10-01`),
   with a notice when there was more than one.

Names match ASCII case-insensitively. Only maps the disc has are served
(a package for a new name stays unseen until the map list work).

**Resolution**, on the map's first use, once per session:

- Each layer is read and its SHA-256 checked. The format comes from its
  signature (`PPF`, `PATCH`, `BPS1`); a file with none ending in `.map`
  is a whole map, which replaces everything before it and becomes the
  base. Anything else (SPPF, Serenity) is refused as unrecognised.
- **Overlay** (only PPF and IPS after the base): the layers compose into
  sorted, non-overlapping byte runs over the base. One pass over the base
  checks its SHA-256 (the disc's map against `[base] sha256`, or the whole
  `.map` layer against its own hash) and computes the copy-time checksum
  of the patched bytes, which is then written at +0x2d0 as a last run,
  when the result starts with the `daeh` header and is at least 0x800
  bytes. Reads lay the runs over the base as they stream; nothing is
  copied. PPF records past the base's end are refused (PPF patches a
  fixed-size image); IPS may grow the file (zeros where nothing is
  written) and truncate it.
- **Materialised** (any BPS layer): the result is built in memory once,
  checksum repaired, and written atomically (a temporary file, then a
  rename) to `cache/<MapId>.map`. A later resolution of the same `MapId`
  uses the file without reading the base. After each write the cache is
  trimmed to its cap (4 GiB by default) by removing the least recently
  used maps (by modification time, refreshed on each use), never the one
  just built.
- Content packs are hash-checked and listed (`Resolved::packs`) for the
  feature engine; nothing applies them yet.
- Any failure (a base that does not match, a damaged patch, a layer
  leaving its package) is a warning, and the disc's map is served.

**In the vfs** (`PackageBackend`, wrapping the disc beneath
`MapOverrideBackend`, so `H2_MAP_DIR` still wins as a whole-file
override): `stat` and listings of `maps` give the result's size,
`read_at` streams it, and the choice is logged once with the package and
`MapId`. `host_file` is the cache file for a materialised map and none
for an overlay, which is not one region of a host file (so telemetry and
the speaker index fall back as for any file without one).
`PackageBackend::map_id(path)` gives the `MapId` serving a map.

**Boot.** `<data>/maps/` is scanned at mount. With no valid package there
and `H2_MAP_DIR` unset, the disc is mounted exactly as before
(`Vfs::with_disc` / `with_disc_rewrite`, no wrapper).

**Not yet:** applying content packs; the folder watch (§3); `MapId` in
sessions (§5); SPPF and Serenity; caching the base's hash between runs
(each session reads a patched map's base once, in full, on first use).

## 8. Saving packages from the editor

**Built** (`content::patch::write`, `content::store`,
`scenario_edit::package`, the scenario panel; [scenario-editor.md](scenario-editor.md) §5).

**Patch writer.** `content::patch::diff(base, edited, description)` makes
one layer:

- same length: **PPF3**, which the resolver overlays without copying.
  Records are the differing runs, runs closer than a record header (9
  bytes) merged, split at 255 bytes; no block check, no undo data (the
  resolver checks the base's SHA-256 instead), no FILE_ID.DIZ. A body
  whose last six bytes would read as a FILE_ID.DIZ tail gets one more
  1-byte record, so it never does.
- any other length (a grown block): **BPS**: SourceRead where bytes match
  in place, SourceCopy that continues the last copy or finds a 64-byte
  source block through a hash index, TargetRead for new bytes.

Both reproduce the edited bytes exactly, **header checksum included**. The
editor's map writer leaves the checksum right, so the resolver's repair at
+0x2d0 writes the same four bytes again, and the patch works in any other
PPF or BPS tool without a "resign" step. Tests round-trip random edits
(near the end, at +0x2d0, growth, insertion, truncation) through the
crate's own readers and through the resolver.

**Writing a package.** `content::store::write_package` assembles a new
package in a hidden sibling directory and renames it into place; replacing
the package being edited writes its new files, then the manifest by
rename (the commit point), then removes files only the old manifest named.
`store::set_active` rewrites `active.toml` by rename, keeping other
entries. `store::default_data_root()` is the runtime's default data root,
`$TMPDIR/h2boot-data`, the one `h2 play` and `h2 run` use without
`--data` (`vfs::Config::for_title`'s `~/Library/Application
Support/h2recomp` is not used by any run).

**The editor's package**:

```text
<data>/maps/<dir>/
  manifest.toml        [package] name, author, description, image
                       [base] map = the disc map's name, sha256 = its SHA-256
                       [[patch]] <map>-<first 8 hex of its sha256>.ppf|.bps
                       [result] geometry (above)
  <map>-<hash>.ppf     base -> current, always one layer
  preview.png|.jpg     optional, PNG/JPEG <= 512 KB, checked before writing
```

Opening a package resolves it with the content resolver over the disc's
map (`Library::resolve_package`, `Resolved::read_all`) into memory; that
becomes the editor's unedited state, with the disc's map kept as the base.
Saving it again replaces its one layer with a new base-to-current patch.

[formats]: https://connortumbleson.com/2020/04/12/halo-2-patch-format-types/
