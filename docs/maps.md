# Maps: new, edited, patched and shared (design)

Status: design. Agreed with the Live session (network side) and built on
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

A user-made map with no retail base has `[base]` naming the map it was
derived from, if any, with `derived = true`, and the map file itself as
its one layer.

### Identity: `MapId`

`MapId` is the SHA-256 of the manifest's canonical JSON (`live-proto::jcs`),
minus display fields (name, description, image). So two players have the
same map exactly when they have the same base and the same layers in the
same order, and a server can tell a client exactly which pieces it lacks.
Retail maps have a `MapId` too: a manifest with only `[base]`.

The type lives in a small new crate, `crates/content` (manifest, `MapId`,
patch format readers, resolver), which vfs, boot, editor, live-proto and
live-server depend on. It is the hand-off to the Live session.

## 2. Applying layers at boot

The resolver turns a `MapId` into bytes the vfs serves as
`\maps\<name>.map`, generalising today's `MapOverrideBackend`:

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
| PPF 1/2/3 | 2004- | absolute-offset byte runs; PPF3 has 64-bit offsets and an optional undo block | public spec; read it |
| IPS / BPS | generic | offset runs / copy-from-source deltas with CRCs | public specs; cheap to add |
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
| content | `crates/content`: manifest, `MapId`, PPF/IPS/BPS readers, resolver; the vfs serves packages from `<data>/maps/` (with `H2_MAP_DIR` kept as a shortcut) | — |
| editor | save as package: an offset patch or a `map-variant` pack against the base, plus manifest and image; open packages | content |
| map list | synthesised `patch.lvl` for new maps (and the Map Pack, if 1.0 loads it) | the 1.0 research |
| live | `map_id` in sessions, hoppers and telemetry; the content store; join-time download; admin; art keys | content (`MapId`) |
| realtime | folder watch, then the editor-to-game socket for `map-variant` edits | feature engine loader hook |
| formats | SPPF and Serenity readers | sample patches from the user |

[formats]: https://connortumbleson.com/2020/04/12/halo-2-patch-format-types/
