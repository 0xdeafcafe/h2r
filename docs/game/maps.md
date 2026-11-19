# Maps: new, edited, patched and shared (design)

Status: design, with the foundation built and confirmed in the game by the
owner (2026-11-17): `crates/content` (manifest,
`MapId`, PPF/IPS/BPS readers and writers, resolver), the VFS and boot
wiring for package offers (§3),
the scenario editor's "Save as package" and "Open package" (§8), and new
maps in 1.0's map list (§4). Sections marked **Built** say exactly what exists; see also
§7. Agreed with the Live session (network side) and built on
the feature engine's content packs ([feature-engine.md](../engine/feature-engine.md),
"Content packs"). What exists today: the editor (`h2-editor`,
[editor.md](../editor/editor.md), [scenario-editor.md](../editor/scenario-editor.md)) saves
edited `.map` files, and `H2_MAP_DIR` makes the runtime serve
`\maps\<name>.map` from a host folder ([storage.md](../runtime/storage.md), "Map
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
  ([feature-engine.md](../engine/feature-engine.md)): field-level edits (move this
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

## 2. Applying layers at load

The game opens a mod's own content path. The VFS resolves the manifest
against the retail base or the package's whole-map layer on first open.

- PPF and IPS stream the patched ranges over the base.
- BPS materialises once into `<data>/maps/cache/<MapId>.map`. The ID
  includes the expected base SHA-256 and the ordered layer hashes. A
  cache hit checks the actual base before using the result.
- The resolver repairs the body's checksum. For an aliased mod, the
  VFS serves the alias in the header's short-name field, outside the
  checksum. The original layer files stay intact.
- Content packs are listed for the feature engine.

### Patch formats

| Format | Era | Shape | Status |
|---|---|---|---|
| PPF 1/2/3 | 2004- | absolute-offset byte runs; PPF3 has 64-bit offsets and an optional undo block | **Built**: block check, PPF2 input size, PPF3 undo data checked against the base; FILE_ID.DIZ read; records past the end grow the file, as ApplyPPF's seek-and-write does |
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

Live downloads and removals rebuild the generated offers and publish
their paths to the VFS. The game refreshes its content slots on the
next main-menu tick. Packages added by hand are scanned again at boot.
Open files retain their resolution until their last handle closes.

The game's cache matches the copied map's path. A mod's own content
path keeps its cached copy distinct from the retail base. The boot's
`map-slots` feature can discard an obsolete generated path once its
slot is inactive. See [cache slots](../runtime/cache-slots.md#changed-maps).

## 4. New maps and the Map Pack

1.0 builds its multiplayer map list from the disc's maps plus each
installed content package's `patch.lvl` (version 2). The map pack's own
`patch.lvl` only names Lockout; its new maps are listed in `patch_v3.lvl`,
which only the title update reads ([map-pack.md](map-pack.md)).

So the route for both **new maps** and the **Map Pack**: a content package
under `T:\$c` per map, with a `patch.lvl` listing it. The Map Pack is
installed this way by `h2 import-mappack` ([map-pack.md](map-pack.md) §5,
§8). New maps are below.

### Package offers

Every installed package that fits has its own offer. The boot writes
the offers before the game scans `T:\$c`. The command
`h2 maps install [--data DIR] [--disc disc/halo2.iso]` stages the same
records for the next boot.

| File under `T:\$c\<offer>` | Purpose |
|---|---|
| `<map>.map` | A zero-byte placeholder for the path whose bytes come from the VFS |
| `patch.lvl` | The version 2 list record containing the package's texts and preview |
| `ContentMeta.xbx` | The signed content metadata |
| `h2recomp-map.toml` | The source package, map name and content ID |

A mod on a retail base uses a short alias containing its list ID and
part of its content hash. Standalone maps keep their name where unique.
Old generated offers are cleaned up when their package disappears or
exceeds the slot budget. Older materialised copies in those offers are
replaced by placeholders. The source package keeps its layers and image.
A whole-map package stores the full source map, with no second full copy
in the content offer. The Map Pack's original files stay in place.

#### The record's fields

Layout in [map-pack.md](map-pack.md) 8.1. The manifest gains an optional
`[list]` table, which is display data and not part of the `MapId`:

```toml
[list]
base = "lockout"       # a retail map: its own list entry gives the rest
sort = 300             # optional
name.ja = "..."        # optional, per language: en ja de fr es it ko zh pt
description.ja = "..." # optional
```

| field | source |
|---|---|
| type | 1 |
| scenario path | `scenarios\multi\<map>\<map>`. 1.0 uses only its basename, and opens `<content dir>\<map>.map` |
| map id | below |
| `+0x110` | `-1`, as in every shipped record; 1.0 never reads it |
| names (9 x 32 UTF-16 units) | `[list] name.<lang>`, else `[package] name`, in every language; cut to 31 units at a character boundary |
| descriptions (9 x 128) | `[list] description.<lang>`, else `[package] description`, else empty |
| sort order | `[list] sort`, else 1000: after every retail map (10 to 120 on the user's disc) and Map Pack map (130 to 210) |
| flags, team counts | The `[list] base` entry, the package's retail base entry, or the first disc entry by sort order |
| preview | the package image; else the base's own picture; else a plain card |

The language order is the game's: on the user's disc only index 7 holds
Han characters on the retail and Map Pack entries (Chinese), which fits
en, ja, de, fr, es, it, ko, zh, pt (the Japanese and Korean entries use the
Latin names). That is the order `ContentMeta.xbx` names use too.

**The preview** is 207 rows of 220 pixels, top row first, 32 bits each in
the D3D `A8R8G8B8` linear layout (bytes B, G, R, A): 1.0 copies it row by
row into the slot's bitmap (`0x370` bytes a row), and rendering a Map Pack
record that way gives the right picture. Every shipped record has the same
alpha: a one-pixel opaque frame, `0xbf` inside, and a 16-pixel transparent
triangle cut from the bottom-right corner, its edge opaque.
`map_list::preview_alpha` draws that shape and the ignored test below
checks it against all nine Map Pack records. A PNG or JPEG (`png`,
`zune-jpeg`) is scaled to cover 220x207 (bilinear) and cropped to its
centre. The frame's colour is in a retail preview's pixels too (the pale
border Select Map shows), so a preview made from an image or a plain card
takes the colours of the list's first retail map's preview on that
one-pixel frame (`map_list::with_frame`), read from the user's disc at
install. Without it the Halo 1 maps showed no border. The base's picture is the level element's bitmap (tag reference at
`+0x4`, datum `+0x8`), decoded by `assets::decode_image` with
`mainmenu.map` and `shared.map` as its resource files, then given the same
treatment. The plain card is a vertical gradient in a colour from the
`MapId`.

#### Map ids and offer IDs

- **Map ids** are `0x10000 + (the MapId's first four bytes, little-endian)
  mod 0x7ffe_ffff`, so in `0x0001_0000..=0x7fff_fffe`. Retail ids on the
  user's disc are `0xa` to `0x320`, the Map Pack's `0x3e8` to `0x516`, so
  the range is clear of both (asserted by the ignored test). On a collision
  with a retail id from the disc's own list or another new map, the next
  free id up is taken, wrapping in the range; choices are made in `MapId`
  order, so they don't depend on directory order. 1.0 compares ids for
  equality only (map-pack.md 8.3). The 50 slots include 12 on disc, so
  38 are free before extra content and 29 after the nine-map Map Pack.
  Content beyond the budget is reported in the Mods category. Editing
  map bytes typically changes its list ID and offer. Display changes
  keep both.
- **Offer IDs**: Halo 2's title ID in the high half, `0x8000_0000 | map id`
  in the low half (`4d530064800NNNNN`-style). The Map Pack uses
  `0x0101..0x03ff` and the disc packages `1..3`, so the top bit keeps them
  apart.

#### ContentMeta.xbx

Synthesised in the shape of the Map Pack's (map-pack.md §7): `XCSF`,
header size `0x88`, `+0x1c` = 1, title ID, the offer ID at `+0x28`, `+0x30`
= 1, the names (UTF-16 INI with a byte order mark: `[Default]`, then `[EN]`
`[JA]` `[DE]` `[FR]` `[ES]` `[IT]` `[KO]` `[TW]`, each `Name=`) at `0x88`
with their size and SHA-1, and the file table 32-byte aligned after them
with its size and SHA-1. The table lists no files (count 0): 1.0 never
hashes a package's files (map-pack.md 8.4); XAPI checks the table against
its digest, which holds. When the Map Pack is installed, the header's
words at `+0x1c`, `+0x30` and `+0x6c..0x88` come from one of its
`ContentMeta.xbx` instead. Then `kernel::content::sign` sets the installed
flag and signs it for this console. **Unchecked:** whether anything in 1.0
minds an empty file table. The Map Pack's packages carry a table that no
longer matches their files, and 1.0 accepts them (or will, per 8.6).

`ContentImage.xbx` (an XPR icon for the dashboard) isn't made: 1.0's
list path reads only `ContentMeta.xbx` and `patch.lvl`.

#### Tests

- `content` `map_list::tests`: a record built from a manifest passes 1.0's
  check (version, CRC, type, path) and reads back field for field; names
  and descriptions per language with fallbacks and cutting; `[list]`
  validation and that it isn't in the `MapId`; the alpha shape; PNG
  decoding, cover scaling, cropping and BGRA order; the preview fallback
  (image, base, card); ids in range, independent of order, around
  collisions and wrapping; offers apart from the Map Pack's; base entries
  read from a level element.
- `boot` `new_maps::tests` covers legacy installs, simultaneous VFS reads
  of the retail and mod paths, slot allocation, overflow, signatures,
  image fallback and converted-map sandboxes.
- `boot` `real_disc_base_entry_and_record` (ignored; reads the user's disc
  and, if beside it, `mappack.iso`; writes nothing): 50 level elements, 12
  used, all retail ids below the new range; Lockout's element decodes to a
  preview; a record for a derived map with Lockout as its base passes 1.0's
  check with Lockout's flags and team counts; every Map Pack record's alpha
  is `preview_alpha`'s. Passes on the user's disc.
  `H2_DISC=disc/halo2.iso cargo test -p boot --lib real_disc -- --ignored --nocapture`

#### The check (for the user)

Put a derived package for a new name in `<data>/maps/`, for example a copy
of a multiplayer map as `coldsnap.map`:

```toml
[package]
format = 1
name = "Coldsnap"
description = "A new map"
image = "preview.png"   # optional

[base]
map = "coldsnap"
derived = true

[[patch]]
file = "coldsnap.map"
sha256 = "<sha256 of coldsnap.map>"

[list]
base = "lockout"
```

Start the game (`h2 play`). The console prints `new maps: 1 in the map
list (coldsnap)`. In a local custom game's map choice,
**Coldsnap should be listed last, with its name and preview** (the
package image, else Lockout's picture). Start a game on it: it should
load from `T:\$c\<offer ID>\coldsnap.map` and reach spawn. Then delete
the package and start again: the map leaves the list.

### The map select by category

With more than one kind of map installed, Change Map opens the categories
first (Halo 2, Map Pack, Mods, Halo: CE). A map's category is read from
its list path: [map-select.md](map-select.md).

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
  stored once, immutable. Caps: 95 MiB for a patch or a user map (under
  Cloudflare's 100 MB request body), a per-account quota. Uploads are signed by the author's key; the
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

The 3D viewer draws an installed mod from its own art when the server
has it: `h2 live assets extract` (and `h2 play --live-local`) resolves
every package under `<data>/maps/` to the patched map the game loads and
extracts it like a disc map, keyed by its `MapId`, never by its base
map's name ([live-art.md](../live/live-art.md#mods)). That covers
everything the mod's scenario holds: moved or extra vehicles, weapons,
scenery, machines and spawns, new tag references, objects its scripts
create, effects its scripts play on a loop, and its own or changed
bitmaps and shaders. A game on a mod the server has no art for uses the
base map's geometry, unless `[result] geometry` differs from the base's.

The native viewer (`crates/viewer`) opens an installed mod as
`mod:<package dir>` (`viewer --list` lists them with their base and
`MapId`), built in memory the same way: its layers over the disc's base
(or the installed map pack map), the base checked against the package's
SHA-256.

### As built (Live)

**Built** (the network side; the lane that built it is "maps-live").

- **Wire.** `NewSession.map_id`, `session::Update.map_id`,
  `NewGame.map_id` and `GameSummary.map_id` are `Option<MapId>`,
  serde-default and left out when none, so old payloads parse and old
  servers and logs read the same bytes. `map` stays the base map's name
  (the name the game loads, `maps\<map>.map`). A retail map from the disc
  is sent with no `map_id` at all: the game reports a `MapId` only for a
  map a package serves (`Vfs::map_id`), and the 1.0 map pack's packages
  (`h2 import-mappack`, served from `T:\$c`, not `<data>/maps`) count as
  retail for the same reason.
- **Stats.** A game keeps its `map_id`; `GET /v1/games` filters by `map`
  (every game on that base) and by `map_id` (one exact map, or `none` for
  retail). The web groups history and career stats by `map_id ?? map`, so a
  modded Lockout never mixes into Lockout's stats, and shows the package's
  name, falling back to the base map's title.
- **Content store** (`crates/live-server/src/content.rs`, API in
  [live-api.md](../live/live-api.md) "Custom maps"). Blobs by SHA-256 under
  `<data>/content/<aa>/<sha256>`, checked on upload, kept once, immutable;
  caps of 95 MiB on patches, packs and images and on a whole map (the
  body starts with `daeh`), the largest whole MiB under the Worker's
  100 MB request body, a 1 GiB per-account quota, a per-account rate. A
  package is a `MapPublished` event its author signs; the server validates
  the manifest, computes `MapId::of`, and refuses it while a layer is
  missing.
- **No retail bytes.** The operator's `<data>/retail-maps.sha256`
  (`sha256sum` output, empty by default) lists retail hashes: those blobs
  are refused, and a line naming its map makes `MapId::retail` a retail id
  for ranked play. A manifest with no layers is refused whatever the list
  says.
- **Moderation** ([live-admin.md](../live/live-admin.md) §7): hide, delete, block
  by hash or `MapId`, per-account upload switch, reports. Taken-down
  content answers 410 and sessions on it are refused.
- **Ranked.** Hopper pins (`PUT /v1/hoppers/{hopper_id}/pins`) say which
  custom maps a matchmaking hopper allows; a `hopper` or `ranked` session
  may only advertise a retail `MapId` or a pinned one (the server checks on
  create and on update). `custom` and `coop` sessions take any map that is
  not taken down.
- **The game's side.** The boot names the loaded map's `MapId` with
  its content path. In the lobby the Live probe reads the selected
  multiplayer list ID, then looks up the package's content `MapId` by
  its path. The lobby setter publishes a change immediately. Selecting
  retail clears the advertisement's `map_id`. Telemetry can identify a
  loaded mod by its aliased cache header name and keeps the base map's
  name in match reports.
- **Joining** (`live_hle::maps::MapGate`, given to the HLE through
  `ServerOptions.maps`). A search result whose map cannot be had is left
  out of the browser; finding a session by id (how a join starts) makes its
  map ready first, or the session is not found. Ready means: a package here
  already has the `MapId`; or the server's manifest is fetched
  (`GET /v1/maps/{id}`), its base checked against the disc's own map
  (`Bases`, the SHA-256 of `maps\<map>.map`), the missing layers and image
  downloaded from `/v1/content/{sha256}`, each checked against its
  SHA-256, and written as `<data>/maps/live-<first 16 hex of the
  MapId>/` with its `manifest.toml` last. The installer registers the
  package's content offer before joining proceeds. If the package has
  no slot, the join is refused and the download stays installed. Refusals, printed
  with their reason before any map load: the disc lacks the base
  (`NoBase`), an admin took it down (410: `TakenDown`), the server never
  saw it and it is not here (`Unpublished`), a `hopper`/`ranked` session on
  a map that is neither the disc's retail one nor pinned on its hopper
  (`Ranked`), or a download that failed or did not match its hash.
- **CLI.** `h2 live maps publish DIR [--as GAMERTAG]` checks the package as
  the runtime does, uploads each layer and the image (refusing a file
  whose hash is not the manifest's), signs the manifest with the local
  account's key and publishes it; `h2 live maps list [--base MAP] [--author
  GAMERTAG] [--json]` prints the server's packages.
- **Pictures and viewer** (web): the package's image (served at
  `/v1/maps/{id}/image`), then the base map's art. The 3D viewer first
  looks for the mod's own art by `map_id` (the server's art has it when
  the mod is installed where the art was extracted: an entry
  `mod-<first 16 hex of the MapId>` carrying `map_id`, `base` and the
  patched map's `sha256`), and draws that. Otherwise it uses the base
  map's geometry unless the package has `[result] geometry`, when it says
  "geometry changed, no 3D view" (the art manifest has no base geometry
  hash to compare with yet, so any `geometry` counts as changed).

Deviations from the design above:

- The metadata is not a separate Postgres table: it is the event log (the
  `events` table in Postgres, the file store otherwise), projected in
  memory, like everything else the server keeps. Moderation and pins are
  server-signed events, so they replay and are audited the same way.
- A manifest is refused as "a whole retail map" when it has no layers at
  all, not when it has no `[[patch]]`: a pack-only mod changes no bytes and
  shares no retail data.
- Hopper pins live beside the game's hopper file, keyed by the session's
  `hopper_id`, since the binary file has no room for a `MapId`.
- `coop` sessions are not held to the ranked rule (they are not hoppers).
- Art option 2 (the patched map's own preview, read client-side) is not
  built.

**Wired in the boot** (lane "maps-live-runtime"):

- **The gate.** With Live on, `boot::map_bases::gate` builds the
  `MapGate` over `<data>/maps`, and `live_glue::install` passes it as
  `ServerOptions { maps: Some(..) }`.
- **`Bases`.** `boot::map_bases::DiscBases` hashes `maps\<name>.map` as
  the retail ISO has it. It reads through its own read-only
  `vfs::DiscBackend`, below every package and `H2_MAP_DIR` layer, so a
  package can never vouch for its own base.
  - Hashing is lazy: each map once, on first ask. A missing map is
    remembered too. Names that are not plain (letters, digits, `_`, `-`)
    are refused without touching the disc.
  - The hashes are a `content::DiscHashes` shared with the package
    resolver, which records the hash of each disc base it reads in full
    while checking a package. A map hashed by either is not read again.
  - They persist in `<data>/maps/cache/disc-hashes.toml`, keyed by the
    ISO's canonical path, size and modification time (ns), so a restart
    against the same ISO reads nothing. Any other ISO, or a touched one,
    starts afresh.
- **Installing.** A package `MapGate` installs is served at its own
  content path (§3).

Still open: a `--private` server's reads need a token, which the gate's
fetches do not send yet.

**Join-time download, reviewed 2026-11-19** (by reading and unit tests;
no two-console run yet).

Fixed, with tests in `live_hle::maps`:

- A join's download used the Live worker's agent, whose 5 s timeout
  covers the whole request, body included: any layer over a few MB failed
  every time. Joins and the Mods screen now share `maps::download_agent`
  (10 s to connect, 30 s per read, no whole-request limit)
  (`a_download_outlasts_the_workers_whole_request_timeout`).
- The preview image was a required download: an admin hiding a picture
  (410) refused the whole map. It is now best-effort, and a package
  written without it drops `[package] image` (outside the `MapId`)
  (`a_taken_down_image_does_not_block_the_map`).
- Repeated lookups of a locally installed package reuse its offer and
  do not count new downloads.
- Two writers of the same file (a join and a Mods screen install) shared
  one temporary name per process, so one rename failed. Each write now
  has its own name (`writes_from_two_threads_do_not_trip_over_each_other`).

Open (each needs a live run or a design choice):

- A guest already in a session needs a fresh map check when the host
  changes maps through a session update.
- **The download runs on the Live worker's thread**, so heartbeats and
  session updates wait for it (the Mods screen uses its own thread).
- **Ranked pins are not applied**: sessions go out as `custom` with no
  `hopper_id`, so `ranked_flags` is never true for a real session.
- **The Worker home buffers whole blobs** in the hub Durable Object
  (R2 `bytes()`, then copies): a 95 MiB layer is several hundred MB in a
  128 MB isolate. `BlobStore::get_stream` exists; `GET /v1/content` should
  stream (needs a staging run with a large blob).
- The VFS checks local layers on first open. Corrupt layers cause
  a load error at that package's path.

### The mod repository (Live)

**Built** (lane "mod-repo-web": the server, web and publishing side).

- **Browsing.** `GET /v1/maps` sorts newest first (`sort=new`) or most
  downloaded first (`sort=popular`), filters by base map, category,
  publisher and text (`q`), and pages by cursor. Each item carries its
  `MapPublished` log `position` and its `downloads`. The exact API, for
  the game's Xbox Live menu, is in [live-api.md](../live/live-api.md),
  "The mod repository". "New" needs no server state: the client keeps the
  highest `position` it has seen.
- **Downloads** count the accounts that fetched a manifest with a `read`
  token (the console's join-time or menu download), once each, as
  server-signed `MapDownloaded` events (admins-only in the log).
- **Display data** beyond the manifest: `MapPublished` data takes
  `category` (gameplay, weapons, vehicles, visuals, maps, other),
  `source_url` and `credit`. None of it is in the `MapId`.
- **Credits.** Who made a mod is never the account that published it. A
  package's credit (`live_proto::maps::Credit`: `authors`, the original
  `release` name, `released` date, `source_url`, `notes`) comes from, in
  order: an admin's correction (`PUT /v1/maps/{id}/credit`, a
  server-signed `MapCredited` event), the publisher's signed `credit`, the
  manifest's one-line `[package] author`. Lists and pages show it as
  "by <authors>" ("unknown author" when there is none) and the publisher
  as "uploaded by"; the game's Mods page says "By: <credit>" and, when
  the credit names someone else, "Uploaded By: <gamertag>".
  The publisher's signature and the `MapId` never change with it.
  `docs/game/mod-credits.toml` is the credit of every package published on
  live.forbes.red (all imported by "afr"), researched from the original
  releases; `h2 live maps credit --file docs/game/mod-credits.toml`
  applies it, skipping packages already credited so.
- **Details.** An admin adds what a publisher's manifest lacks, again
  outside the `MapId` (`live_proto::maps::Details`, `PUT
  /v1/maps/{id}/details`, a server-signed `MapDetailed`): a longer
  description (shown instead of the manifest's), screenshots (content
  blobs, PNG or JPEG, each with a caption, who made it, its licence and
  the page it came from; served at `/v1/maps/{id}/images/{n}`, the first
  standing in as the preview when the package has none), a features list
  that replaces the published one, a contents report and the map pack
  the mod came out in. A pack's own info (name, description, authors,
  date, source, images) is said once (`PUT /v1/mappacks/{id}`, a
  server-signed `MapPackSet`; `GET /v1/mappacks`); a map list page
  carries the packs its items name (`packs`) and a map's answer its
  pack (`pack_info`), so the game can group a pack's maps. The
  contents report (`live_proto::maps::Contents`) is read off the patched
  map against its base (`cli::live_mod_contents`, `h2 live maps
  analyze`): per tag class how many tags are new, changed or removed;
  each reference of a tag both maps have that names another tag now,
  classified by what it names (projectile, model, render model,
  collision, physics, animation, shader, texture, damage, effect, sound,
  weapon, vehicle, sky, palette); the weapon, projectile and damage stats
  whose meaning is established; per placed tag how many the base and the
  mod place, moved, and replaced in place; scripts. In words
  (`lines`, e.g. "projectile swap: battle rifle fires fuel rod (was
  battle rifle bullet)", "new vehicle placements: 4 banshees") and as
  fields. `docs/game/mod-details.toml` is every published package's
  details and packs; `h2 live maps details --file` applies it (dry run
  first), uploading the screenshots kept outside git.
- **Web.** The stats site's "Mods" tab (`/games/halo2/mods`): Newest and
  Most Downloaded, base map, category and text filters, and a page per mod
  (picture or base map art, author(s), original release and date, base
  map, size to download, layers, date, downloads, uploader, source link,
  report button). The admin area's packages table has Made by and
  Uploader columns. The web never sends a token on
  `GET /v1/maps/{id}`, so viewing a mod page is no download.

### Importing a mod

Many old mods are whole modded `.map` files: they hold the retail map's
bytes and may not be shared. `h2 live maps import FILE [--base MAP]
[--disc ISO] [--mappack ISO] [--out DIR] [--name NAME] [--info mod.toml]
[--category C] [--no-publish]` (`crates/cli/src/live_mod_import.rs`) makes
a package that shares only the difference:

1. The base map (`--base`, else `base_map` in the sidecar) is read from
   the user's own disc through `vfs::open_disc` (an image or a directory
   of its files), and its SHA-256 is the package's `[base] sha256`. A map
   the disc's `maps` lacks is looked up on the Multiplayer Map Pack disc
   (`--mappack`, else `mappack.iso` beside the disc, found as `h2 play`
   finds it: `mappack_cmd::discover`), read with the stats site
   extractor's reader (`live_mappack::from_iso`: each
   `/demos/dlc/<package>/<map>.map`, which the disc holds as plain files).
   Its `[base]` is the same shape: `map` is the map file's stem as the
   game names it (`containment`, `deltatap` for Sanctuary, `dune` for
   Relic, `triplicate` for Terminal), `sha256` the map pack map's. The
   names never collide with the Halo 2 disc's, and `h2 import-mappack`
   installs the maps byte for byte, so the hash is also the installed
   copy's.
2. A whole map becomes one patch layer from that base,
   `content::patch::diff`: PPF3 when the length is unchanged (the resolver
   lays it over the disc's map as it streams), BPS otherwise. The patch is
   applied back to the base and must give the mod exactly. A `.ppf`,
   `.ips` or `.bps` mod is applied to the base to check it fits (a BPS's
   source CRC, a PPF's checks) and kept as it is.
3. Refused: a mod that changes nothing, and a result that is byte for
   byte one of the disc's maps or, when the map pack disc is there, one
   of its maps (another retail map passed off as a mod, whose bytes would
   fill the delta; only maps of the same size are hashed).
4. The package directory (`--out`, else `<stem>.package` beside the file)
   gets `manifest.toml`, `mod.<ppf|bps|ips>`, the preview and a `mod.toml`
   with the repository's extras; then it is published as `h2 live maps
   publish DIR` would (`--no-publish` stops before that).

The sidecar, `mod.toml` beside the file (or `--info`), every key optional
and unknown keys ignored: `name`, `authors` (the original author or
authors, a list: the credit), `author` (one name, the older form, used
when `authors` is absent), `release` (the original release's name),
`released` (`YYYY`, `YYYY-MM` or `YYYY-MM-DD`), `notes`, `description`,
`source_url`, `format` (`map`, `ppf`, `ips` or `bps`; else the file's
signature decides), `base_map`, `category`, `image` (a PNG or JPEG beside
it, at most 512 KiB). The credit goes into the signed document's `credit`
and, on one line, into the manifest's `[package] author`. `h2 live maps
publish DIR` reads `category`, `source_url` and the credit from
`DIR/mod.toml`; `--category` and `--source-url` win. Give the credit on
every import: without it the package says "unknown author".

`h2 live maps credit MAP_ID [--author NAME]... [--release NAME]
[--released DATE] [--source-url URL] [--notes TEXT] [--clear]`, or `h2
live maps credit --file CREDITS.toml [--dry-run]`, sets the credit of
packages already published (an admin's account, `--as`, or the
operator's key, `--data DIR`). The file is `[[mod]]` tables of `map_id`,
`name` (checked against the server's), `authors`, `release`, `released`,
`source_url`, `notes`; every entry is checked before anything is sent.

**More mods.** `docs/game/mods-catalogue.toml` lists Halo 2 Xbox map mods
found for import (2026-11-19, mostly from ibotmodz.net, se7ensins and
archive.org): name, authors, base map, format, size and SHA-256 of the
file checked, download and source URLs, and a `fit`. `good` ones import
as they are (PPFs, and whole maps of a disc map); `whole-map` ones are
total conversions whose map header names a retail map, which import over
it as a large BPS; `unsupported-format` ones are SPPF or Serenity patches
(no reader yet, §6 "formats"); `needs-shared` ones patch `mainmenu`. Import
each with a `mod.toml` that gives its credit:

```sh
h2 live maps import FILE --base MAP --info mod.toml --out DIR
# mod.toml: name, authors = [..], base_map, category, released,
#           source_url, description
```

**In the game.** A package on a Map Pack base installs and plays like one
on a disc map, for a player who has the map pack installed (`h2
import-mappack`, or `h2 play`'s automatic import):

- **Bases.** The Live map gate's bases (`boot::map_bases::UserBases`) are
  the disc's maps (`DiscBases`), then, for a name in
  `content::map_pack::MAPS`, the installed copy
  (`MapPackBases`: `<data>/disk/Partition1/TDATA/4D530064/$c/<offer>/<map>.map`,
  the first map pack offer, by name, that holds it, `map_pack_file`).
  The import copies each map byte for byte (`mappack_cmd::install`,
  `Xiso::copy_to` of the entry's size), so its SHA-256 is the one
  `live_mappack::from_iso` gives the mod. Each file is hashed once; the
  hash is kept in `<data>/maps/cache/mappack-hashes.toml`, stamped per map
  with the file's path, size and modification time, so a changed or
  reinstalled map is hashed again and a removed one has no base.
- **Install.** `MapGate::install` (the Mods screen) and `MapGate::prepare`
  (a guest joining a game on the mod) check the base as for a disc map.
  Without the map pack the refusal is `Refusal::NoMapPack` ("... from the
  Multiplayer Map Pack, which isn't installed here (h2 import-mappack)");
  the Mods screen's Download row says "Needs the Map Pack's Relic" (the
  map's game name, `content::map_pack::title`, as on the Map row). A map
  pack that is installed but differs is `NoBase`, with the same row text.
- **Serving.** The game's Map Pack slot opens its original file at
  `T:\$c\<offer>\<map>.map`. The mod gets a separate content offer and
  alias. The VFS reads the original Map Pack file as the mod's base.
- **Map select and sessions.** The map stays in Map Pack and the package
  appears under Mods. The lobby advertises the mod's content `MapId`
  when its slot is selected.

Limits: a mod built on a different build of the base map than the user's
disc still imports (the delta reproduces it exactly) but is larger, and
players need the same disc map to use it. A delta can carry bytes that
also appear in some retail map (new content copied from another level);
only a whole retail map is caught.

### Mod downloads (in game)

**Built** (lane "mod-repo-game"; the server and web side is lane
"mod-repo-web"). Nothing here has run in the game yet: see "To check in
game" below.

- **The row.** The Xbox Live menu's last row, where retail had Content
  Download, reads **Mods**, or **Mods (3 new)** when maps were published
  since the player last opened the screen. Its pane's help text says what
  it is (`custom_games::MODS_HELP`). Without Live or the map gate (or with
  `H2_NO_MODS_ROW=1`) the row is Content Download again, with its Map
  Pack dialog ([live-custom-games.md](../live/live-custom-games.md) §5).
- **New.** An entry is new when its `position` (the event-log position it
  was published at) is above the last-seen position, kept in
  `<data>/live-mods.toml` (`seen = N`). Opening the screen sets it to the
  newest listed entry's; the entries keep their "New" tag for that visit.
  An entry without a `position` (a server from before the repository) is
  never new. A player who never opened the screen sees every entry as new.
- **The list.** `GET /v1/maps?sort=new&limit=40` with the signed-in
  account's `read` token, through the Live worker's retrying GET
  (`live_hle::Hle::mod_list`, `live_hle::mods`). It reads both the
  repository's shape (`{items: [{id, name, author, base, image, size,
  position, downloads, published_at}], cursor, position}`) and the content
  API's (`map_id`, `{gamertag}`, `{map}`, `next`). A thread of ours
  (`h2-mods`) asks for it when the Live menu draws the row or the screen
  opens (at most every 30 s), every 5 minutes otherwise, and 15 s after a
  failure; the game thread never waits on it.
- **The screens** (`boot::mods_screen`, our list class, laid out as the
  level select is: list on the left with its bracket, picture top right,
  description under it, as the campaign picker). **Mods**: one row per
  map (at most 40, newest first), its name and its state as a mark after
  it: a star (U+2605) when new, a tick (U+2713) when installed, a down
  arrow (U+2193) and the percentage while downloading; nothing when it is
  only on the server. Halo 2's fonts have none of the three, so they are
  drawn into every font as the bot mark's gear is (`font::mod_marks`,
  added by `bot_mark::GearRewrite` with the gear, the capitals' height;
  drawn here from polygons, nothing copied). The focused map's picture is on the right and under
  it four lines: "On Lockout, by Modder", its size and downloads, what it
  changes ("Has vehicles, scripted events", or "What it changes isn't
  listed"), and its state ("X downloads it, A opens its page",
  "Downloading N%", "Installed: A for its page", or why a download
  failed). **A** opens the map's page; **X** downloads the focused map
  there and then (an installed one is left alone). The footer reads
  `(A) DETAILS  (X) DOWNLOAD  (B) BACK`. Every row is something to
  press: with no list the one row is "Loading...", "Try Again" (its
  description says why: "Sign in to Live first", "Live is off", "Live is
  busy" or "Can't reach the server") or "Refresh" (no mods yet); A asks
  again. The **map's page** is below.
- **Map packs.** Maps the server lists with the same pack (an entry's
  `pack`, a name or `{name, description}`; else its credit's `release`)
  are one row of Mods, at the place of its newest map: the pack's name,
  and a star, a tick, the downloads' percentage together, or "N maps".
  Its summary: "N maps, by <makers>", what it is (the pack's
  description, else "On <base maps>"), "<size> in all, K installed",
  "A opens it, X downloads all". A pack with one map listed is that
  map's row. **A** opens the pack's own list (`mods.pack`, the same
  layout, titled with the pack's name): first **Download All (N)** (N the
  maps not here; "Downloading N%" while they run, "All Installed" after),
  whose description is the pack's summary, then its maps, each as the
  list shows it, A opening its page (B back to the pack). **X** on a map
  downloads it; on Download All, or on the pack's row of Mods, every map
  of it not here or under way. A snapshot keeps the open pack.
- **Installing.** Download runs `MapGate::install` on a thread of its own
  (`h2-live-mod`): the join's own checks and path (the manifest from
  `/v1/maps/{id}`, the base checked against the disc, each layer from
  `/v1/content/{sha256}` checked against its SHA-256, the package written
  as `<data>/maps/live-<id>/`, then the offers refresh). The
  Download row shows "Downloading N%" after each layer, then "Installed".
  A refusal says why: "Needs Lockout from your disc", "Taken down", "Not
  on the server", or "Failed; press A to retry" (the reason on stderr).
- **The download count.** Once a package is installed (or found here
  already), the gate asks for its manifest again with a fresh single-use
  `read` token of the first account signed in (`Fetch::get_counted`,
  `maps::Reader`), and the server counts that account once per package
  (docs/live/live-api.md "The mod repository"). A join that installs the
  session's map counts the same way; a search's check never does. A
  reinstall adds nothing; a failed count is only logged.
- **The map select.** The package appears by its own name under Mods
  after the next content scan. Its slot holds its own texts and preview,
  which also appear in Game Setup and the lobby.
- **Previews.** The three screens are in the UI kit's inventory (`mods`,
  `mods.pack`, `mods.detail`; `h2 ui preview mods.detail`), over invented
  entries with an invented three-map pack (`mods_screen::spec`).
- **Not built.** Paging past 40, the mod's own preview image for a map not downloaded yet (below), and
  updates (a new version of a mod is another `MapId`; nothing links the
  two yet).

### A mod's page

**Built** (lanes "mods-pages", then "mods-page-look"); not run in the game
yet.

**Why this layout.** The first page was a list of rows (Download, then
Map, By, Size and the rest, then `[x] Vehicles`...), most of them not
pressable: a selectable list whose rows do nothing, which no retail
screen does. Every screen tag in `mainmenu.map` was read for a retail
place that shows information that isn't pressed (2026-11-19): the
postgame carnage report (16, 185: tables of players on list skins),
the level select (11) and map select (17: list, picture, a description),
the variant select (15), the Xbox Live menu (186) and the matchmaking
playlists (219: list, picture, a white heading and a pale body), the
message dialogs (211, 215, 218: actions in a short list beside a body),
the matchmaking progress screen (210) and the **pregame lobby** (14).
The lobby is the one that shows a chosen map's details: two buttons at
the top (Start Game, Game Setup) are all that is selectable; under them
a status text on its panel (`lobby_display`), the game's settings and
the map's picture (`unknown_map`, 220 by 207), and on the right a
two-column table (each row a white name, left, and a pale-blue value,
right, 22 units apart from `[177 118 157 276]`). Matchmaking (pane 3)
shows the same details block, so it is the game's own "game details"
look. A mod's page is built on it
(`crates/boot/src/mods_screen/page_layout.rs`; each rectangle, font,
colour, animation and depth bias is the lobby tag's own unless noted):

| Part | Lobby original | On a mod's page |
|---|---|---|
| Action | the buttons `[173 -274 153 -118]` on `name_field` (338 by 35 at (-279, 174)) | the page's only row, a skin-0 item at anchor (-262, 147) on `name_field`: `Download`, `Downloading N%`, `Remove`, `Remove? Press again`, a refusal or `Remove failed` |
| Body | status text `[135 -258 61 45]`, font 1, pale blue, on `lobby_display` (512 by 128 at (-283, 149)) | the mod's description, one paragraph, 140 characters at most (4 lines; bottom 59) |
| Status | the "open" line `[75 -260 55 70]` | the action's description, one line of 40 characters at most, `[55 -258 36 45]` in the Live menu's hint blue: `A or X downloads it: 3.4 MB.`, `25% of 3.4 MB. You can leave.`, `Needs the Map Pack's Relic. A retries.`, `Press A or X again to remove it.`, `Installed: pick it in Change Map.`, `Not removed: <why>` |
| Picture | `unknown_map` at (-152, 32), animation 14 | the same widget at (-258, 30), so no text runs over it (the lobby's settings lines do); its bitmap is the base map's slot picture as on the list |
| Table | the player rows, names white and left (bias 6), values pale blue and right (bias 5) | up to ten facts: `Map`, `By` (the credit, else the uploader), `Shared By` (the uploader, when the credit names someone else), `Size`, `Downloads`, `Published`, `Version` (the last three when the server has them), `For` (game types, or `any game type`), `From` (`Your disc` or `Map Pack`), `Joins` (`Get it too`); right edge 272, the title-safe area's; each row 20 characters at most, the value cut with `...` |
| Checklist | (the lobby's settings lines) | row 11 of the table column, `What It Changes` in white, then four rows of two: `[x] Vehicles  [ ] Weapons` ... `[ ] Geometry  [ ] AI`; `Not listed` when the server lists none |

Only the action is a row, so the focus never lands on information. The
title is the mod's name, and the footer reads `(A)(X) DOWNLOAD  (B)
BACK`, or `REMOVE` for an installed map. In game the texts are text
widgets of the page's own (`ListClass::panel`, `list_class::PanelText`):
made from their blocks when the pane is built, their strings set from
the `mod texts` slots then and again whenever the description is set
(a download's progress, an install, a removal).

**A** on the action, or **X**, downloads the map; on a map that is here
they remove it, on a second press (the first turns the row to `Remove?
Press again`; opening the page again forgets it).
Remove (`MapGate::remove`) deletes only a `live-<id>` directory of
`<data>/maps` (one the Mods screen or a join wrote; a package put there by
hand is refused), then tells the boot to retire that generated offer.

**The picture** in Select Map, Game Setup and the lobby comes from the
chosen slot. The game's Mods browser can also use the base map's
preview bitmap (`mod_slots::pictures`).

**What it changes** (`features`) is read off the map at import:
`h2 live maps import` has the base and the modded map in hand and compares
them (`cli::live_mod_features`): every tag of a counted group by name
(new; a different size; different bytes at the same address: a tag that
moved with the same size is not compared, since its pointers moved with
it), each bitmap's own pixel data (its images' local LOD 0) wherever it
moved, the scenario's placements by tag name and position (vehicles,
weapons, and scenery, equipment, controls, light fixtures, sound scenery
and crates as objects; netgame equipment by its item's group) and its
scripts. Groups: `vehi`/`vehc` vehicles, `weap`/`itmc` weapons, the placed
kinds objects, `bitm` textures, `shad`/`stem` shaders, `sbsp` geometry,
`char` AI. AI squads are not compared. The sidecar's `features` are added
(and refused when unknown); its `gametypes` and `version` are copied. All
three go into the package's `mod.toml`, which `h2 live maps publish`
sends as the document's `features`, `gametypes` and `version`
([live-api.md](../live/live-api.md), "The mod repository"). A map the
reader can't open keeps the sidecar's features alone (said on stderr).
They are display data, outside the `MapId`.

**To check in game** (Live on, signed in, a server with at least two
published maps, one on a disc map you have and one you can make fail, for
example by publishing one built on a map whose hash differs from your
disc's):

1. Xbox Live menu: six rows, all on screen, no scroll bar: Quickmatch,
   OptiMatch, Custom Game, Custom Game Browser, Co-op Campaign, Mods.
   - The first row and the hint block ("PRESS ... ANYTIME TO ACCESS")
     are where retail has them; the rows stand closer together (29
     units) and the last ends above the hint block. stderr: `custom
     games: xbox live menu tag has 6 panes and shows 6 rows` and `custom
     games: xbox live menu rows 29 apart (6 rows, 5 moved)`.
   - The last row reads "Mods (N new)" (N = maps published since you last
     opened it; all of them the first time), and its help text reads
     "Download maps other players made and shared on your Live server."
     stderr: `mods: N on the server, M new`.
2. Press A on Mods: stderr `mods: opening the mods screen`; the screen
   titled "Mods" lists the maps newest first, the new ones tagged "New".
   B goes back to the Live menu, whose row now reads "Mods"; leaving the
   Live menu and coming back keeps "Mods". `<data>/live-mods.toml` holds
   the newest position.
3. A on a map: its page, titled with its name; "By:" is who made it
   (the server's credit, else the uploading account), and when the credit
   names someone else an "Uploaded By:" row follows with the publishing
   account.
4. A on Download: "Downloading N%", then "Installed"; stderr `mods: <id>
   installed`; `<data>/maps/live-<16 hex>/` exists. B, B, then host a custom game: with no restart, Change Map
   lists the mod under Mods by its name, with its picture (or the base
   map's) and description; pick it, and Game Setup and the lobby show
   its name. The mod loads.
5. The map whose base you lack: "Needs <Map> from your disc"; nothing
   written under `<data>/maps`.
6. Signed out (or the server stopped): Mods shows one row, "Sign in to
   Live first" or "Can't reach the server"; A on it logs `mods: reading
   the list again`.

**To check in game: the pages** (a server with a mod published with
features, e.g. imported again with `h2 live maps import`, after the
server is updated):

1. Mods: the list is on the left in the level select's bracket, up to 13
   rows before it scrolls; the focused map's base-map picture is in the
   box top right (check it is the map's preview at the box's size: the
   multiplayer slots' bitmaps are drawn as the campaign picker's level
   pictures are, at their own size), and the four summary lines are under
   it, changing with the focus. Footer: `(A) DETAILS  (X) DOWNLOAD  (B)
   BACK`, glyphs drawn as buttons.
2. X on a map not here: stderr `mods: downloading <name> (<id>)`; the row
   reads `<name>  ↓ N%` (an arrow, not a box or a gap) then `<name>  ✓`,
   the summary's last line follows; new maps end in a star. X on an installed map logs `mods: <name> is installed; its
   page removes it`. Start does nothing.
3. A on a map: its page, titled with its name, laid out as the pregame
   lobby (compare with the lobby itself): the one action row where the
   lobby's Start Game / Game Setup buttons are, on their backing; the
   description on the lobby's panel and the status line under it; the
   map's picture under that, nothing over it; the facts table on the
   right where the lobby's players are, names white and left, values
   pale blue and right, then `What It Changes` and the ticks in two
   columns. Up and down do nothing (one row). Note anything clipped, a
   text missing (a panel widget not made logs `no panel text N`), or
   the texts not sliding with the page.
4. X (or A on the action) on a map not here downloads it; the status
   line follows the progress; on an
   installed one the first press reads `Remove? Press again`, the second
   logs `mods: <name> (<id>) removed`, the row reads `Download`, the
   footer `DOWNLOAD`; `<data>/maps/live-<16 hex>/` is gone and
   Change Map no longer lists it.
5. Select the mod in Game Setup and check the package image in all
   three views: Select Map, Game Setup and the lobby.
6. A pack (a server with two or more maps of one pack, e.g. a mappack
   imported with its release): one row, its name and a star or "N maps";
   its summary says how many maps and by whom. A opens it, titled with
   its name: Download All (N) first, then the maps. A on a map opens its
   page and B comes back to the pack. A on Download All logs `mods:
   <pack>: downloading N maps`; the row shows the pack's percentage, then
   All Installed, and the pack's row on Mods a tick. X on a pack's row on
   Mods does the same as Download All.

## 6. Plan

| Lane | Work | Depends on |
|---|---|---|
| content | **Built.** `crates/content`: manifest, `MapId`, PPF/IPS/BPS readers, resolver; the vfs serves packages from `<data>/maps/` (with `H2_MAP_DIR` kept as a shortcut) | — |
| editor | **Built** (§8): save as package (one PPF3 or BPS layer against the base, manifest, image, optional `active.toml`), open packages; `map-variant` packs not yet | content |
| map list | **Built** (§4): new maps as content packages with a built `patch.lvl`; the Map Pack through `h2 import-mappack` | the 1.0 research |
| live | `map_id` in sessions, hoppers and telemetry; the content store; join-time download; admin; art keys | content (`MapId`) |
| realtime | **Built:** packages installed mid-session served at the next open, stale cache slots dropped (§3). Next: the editor-to-game socket for `map-variant` edits | feature engine loader hook |
| formats | SPPF and Serenity readers | sample patches from the user |

## 7. What is built

`crates/content` (no game data; every test uses synthetic maps and
patches), the VFS content map registry, and the boot mount
(`crates/boot/src/map_override.rs`).

**Layout.**

```text
<data>/maps/
  active.toml              legacy editor choices, ignored by the game
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

**Variant packages.** A manifest may hold `[[variant]]` tables, each a
whole map file with its SHA-256 and an `origin` (`content::Variant`,
`content::Origin`); the converted Halo 1 maps are the first user, one
variant per sandbox. `Manifest::select_variant` turns one variant into a
plain derived manifest, so each has its own `MapId` (`origin` is in no id),
and the resolver loads the default variant (`h1` if present, else the
first). The form and the package layout are in
[multiplayer.md](../halo1/multiplayer.md#converting-a-map-built-h1convertconvert-h2-h1-convert).
Nothing yet picks a variant by a setting.

**One slot per installed package.** The game keeps the disc and Map
Pack slots at their original paths. Each package that fits gets its
own slot and content path. Several packages can use the same retail
base at the same time. The game ignores `active.toml` when allocating
slots and loading maps. Existing package directories need no edits.

**Resolution** starts when the VFS first opens that package's path:

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
  copied. A PPF or IPS record past the base's end grows the file (zeros
  where nothing is written), as ApplyPPF and IPS tools do, up to 4 GiB
  (`content::patch::MAX_LEN`); IPS may also truncate it.
- **Materialised** (any BPS layer): the result is built in memory once,
  checksum repaired, and written atomically (a temporary file, then a
  rename) to `cache/<MapId>.map`. A later resolution of the same `MapId`
  uses the file after verifying the base hash. After each write the cache is
  trimmed to its cap (4 GiB by default) by removing the least recently
  used maps (by modification time, refreshed on each use), never the one
  just built.
- Content packs are hash-checked and listed (`Resolved::packs`) for the
  feature engine; nothing applies them yet.
- A wrong base or damaged layer raises a load error at the mod path.

**In the VFS.** The registered map resolves over the read-only disc
region or installed Map Pack base. `stat` gives its resolved size and
`read_at` streams its bytes. `Vfs::map_id` reports the manifest's content
ID for the generated path. Overlays and aliased headers are available
through VFS reads. Host tools can still resolve the original package
and use its BPS cache file.

**Not yet:** applying content packs; SPPF and Serenity. A resolution still
reads a patched map's base once, in full, per session: the overlay's
checksum pass needs the bytes anyway. The base's hash is kept between runs
for the Live gate (§5, `disc-hashes.toml`).

## 8. Saving packages from the editor

**Built** (`content::patch::write`, `content::store`,
`scenario_edit::package`, the scenario panel; [scenario-editor.md](../editor/scenario-editor.md) §5).

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
  source block through a hash index, TargetCopy the same way over the new
  bytes already written (data a mod added twice, padding), TargetRead for
  the rest. For Night Ops (a 95.7 MiB map over 47.6 MiB Containment)
  about 7.8 MiB of the new bytes repeat earlier new bytes, so TargetCopy
  takes the 68.2 MiB delta to about 60 MiB; 03a_oldmombasa holds 65.7 MiB
  of its blocks, but a package's base is the map it replaces, so that
  cannot be its source.

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
The editor can still write its legacy `active.toml` choices, which the
game ignores. `store::default_data_root()` is the runtime's default data root,
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
