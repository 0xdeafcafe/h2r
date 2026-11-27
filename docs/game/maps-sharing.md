# Maps: sharing through Live

Sharing map layers through Live. Part of [maps.md](maps.md).

## 5. Sharing (Live)

The rules:

- `NewSession` has `map_id: Option<MapId>` beside `map`, serde-default,
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
map's name ([live-art.md](../live/live-art-viewer-packs.md#mods)). That covers
everything the mod's scenario holds: moved or extra vehicles, weapons,
scenery, machines and spawns, new tag references, objects its scripts
create, effects its scripts play on a loop, and its own or changed
bitmaps and shaders. A game on a mod the server has no art for uses the
base map's geometry, unless `[result] geometry` differs from the base's.

### Wire, store and joining

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
- **Content store** (`crates/live/live-server/src/content.rs`, API in
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
- **Pictures and viewer** (web): a cleared gallery or package preview
  (served at `/v1/maps/{id}/image`), then the base map's art. A signed
  manifest alone does not clear its preview for public display.
  [Live screenshot rules](../live/live-api-content.md#verified-screenshot-uploads)
  The 3D viewer first
  looks for the mod's own art by `map_id` (the server's art has it when
  the mod is installed where the art was extracted: an entry
  `mod-<first 16 hex of the MapId>` carrying `map_id`, `base` and the
  patched map's `sha256`), and draws that. Otherwise it uses the base
  map's geometry unless the package has `[result] geometry`, when it says
  "geometry changed, no 3D view" (the art manifest has no base geometry
  hash to compare with yet, so any `geometry` counts as changed).

Where the code differs from the rules above:

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

**Wired in the boot:**

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

A `--private` server's reads need a token, which the gate's fetches do
not send.

**Join-time downloads** (`live_hle::maps`):

- Joins and the Mods screen share `maps::download_agent` (10 s to
  connect, 30 s per read, no whole-request limit), because the Live
  worker's agent has a 5 s timeout over the whole request, body included,
  and fails any layer over a few MB (test
  `a_download_outlasts_the_workers_whole_request_timeout`).
- The preview image is a best-effort download: an admin hiding a picture
  (410) does not refuse the map, and a package written without it drops
  `[package] image`, which is outside the `MapId` (test
  `a_taken_down_image_does_not_block_the_map`).
- Repeated lookups of a locally installed package reuse its offer and do
  not count new downloads.
- Each write of a file has its own temporary name, so a join and a Mods
  screen install of the same file do not collide (test
  `writes_from_two_threads_do_not_trip_over_each_other`).
- The VFS checks local layers on first open; a corrupt layer is a load
  error at that package's path.

Limits:

- A guest already in a session does not re-check the map when the host
  changes maps through a session update.
- The download runs on the Live worker's thread, so heartbeats and session
  updates wait for it (the Mods screen uses its own thread).
- Ranked pins are not applied: sessions go out as `custom` with no
  `hopper_id`, so `ranked_flags` is never true for a real session.
- The Worker home buffers whole blobs in the hub Durable Object (R2
  `bytes()`, then copies): a 95 MiB layer is several hundred MB in a 128 MB
  isolate. `BlobStore::get_stream` exists; `GET /v1/content` does not use it.

### The mod repository (Live)

- **Releases.** `GET /v1/mod-packs` lists uploader-owned packs with
  original authors, source links, release metadata, screenshots and their
  individual maps. The pack is the unit of browsing and moderation.
  Installs and session joins verify the selected member's manifest
  and layers by the same hashes as an individual map download.
- **Browsing individual maps.** `GET /v1/maps` sorts newest first (`sort=new`) or most
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
  analyse`): per tag class how many tags are new, changed or removed;
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
- **Web.** The Mods tab shows a release's cover, original authors, map
  count and download size. Players can open the pack, follow its source,
  inspect each map's screenshots or report the release. Uplink moderators
  expand a pack to inspect its member maps. Retail map pictures carry a
  fallback label.

### Importing a mod

`h2 live mods import INPUT --pack NAME --author NAME --source-url URL`
imports an entire release. Repeat `--author` for each original author.
Use `--year`, `--description` and `--version` for release metadata.
`--cheating-pack` and `--not-for-online` are alternative labels.
`--as` selects the uploading account and `--server` selects its Live
server. The source URL credits the original release.

A complete build may be a game directory, an Xbox disc image or an
archive such as a RAR. The importer reads the multiplayer maps, compares
each with the corresponding map from the owner's disc (`--disc`, with
`--mappack` for expansion bases) and skips unchanged maps. Only patch
layers and signed manifests reach Live. Executables, campaign and shared
maps, and other files in a distribution stay local. The importer also
accepts folders of individual patched maps and patches. A patch filename
or enclosing folder identifies its retail level by file stem or map name.
For a bundle of nested archives, extract the desired release variant first.

ZIP, tar, 7z and RAR input uses libarchive (system `tar` on macOS and
Windows, `bsdtar` on Linux). For RAR the importer prefers `unrar` when
installed, including older RAR compression filters that libarchive
cannot decode. Xdelta patches require `xdelta3`. The CLI stages new
members and publishes the pack's membership in one operation. Importing
the same release again reuses members the uploader already published.

Many old mods are whole modded `.map` files: they hold the retail map's
bytes and may not be shared. `h2 live maps import FILE [--base MAP]
[--disc ISO] [--mappack ISO] [--out DIR] [--name NAME] [--info mod.toml]
[--category C] [--no-publish]` (`crates/apps/cli/src/live_mod_import.rs`) makes
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
`source_url`, `format` (`map`, `ppf`, `ips`, `bps`, `sppf` or `xdelta`; else the signature
or the `.sppf` extension identifies the format), `base_map`, `category`, `image` (a PNG or JPEG beside
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
found for import (mostly from ibotmodz.net, se7ensins and archive.org): name, authors, base map, format, size and SHA-256 of the
file checked, download and source URLs, and a `fit`. `good` ones import
as they are (PPFs, and whole maps of a disc map); `whole-map` ones are
total conversions whose map header names a retail map, which import over
it as a large BPS. For entries marked `unsupported-format`, see the patch
formats in section 2. Entries marked `needs-shared` patch `mainmenu`. Import
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

### A mod as a package of tag patches

A mod that changes tags and not the whole map is a package
([studio-redesign.md](../editor/studio-redesign.md), "Packages";
[asset-store.md](../engine/asset-store.md), "Mods and packages"), built as
the game assembles the level: an `edits` item whose `set` lines name the
value each replaces (`from`), `tag` items that override an asset whole
(`replaces`), `tag-delta` items, and new assets. Each names the user's
own asset by key and asset hash and holds only the change, so two such
mods on different fields of one asset both apply; one made for other
content is off for that level with its reason, and moves neither the
level's `LevelId` nor its image.

- **No retail bytes, by the publisher's own store.** `h2 package check
  --store <data>/tags DIR...` refuses a file whose SHA-256 is a raw
  resource or a tag file of that machine's tag store
  (`workspace::store_findings`): the store is the retail list there, and
  none is shipped. The server's list (section 5) stays the gate on upload.
- **Still whole-map.** `h2 live maps import` writes the whole-map package
  above; importing a patched map as a package of tag patches is later
  (AS9).

### Mod downloads (in game)

- **The row.** The Xbox Live menu's last row, where retail had Content
  Download, reads **Mods**, or **Mods (3 new)** when releases were published
  since the player last opened the screen. Its pane's help text says what
  it is (`custom_games::MODS_HELP`). Without Live or the map gate the row
  reads **Mods** and opens the Map Pack dialog, its help
  `custom_games::MAPPACK_HELP` ([live-custom-games.md](../live/live-custom-games.md) §5).
- **New.** An entry is new when its `position` (the event-log position it
  was published at) is above the last-seen position, kept in
  `<data>/live-mods.toml` (`seen = N`). Opening the screen sets it to the
  newest listed entry's; the entries keep their "New" tag for that visit.
  An entry without a `position` (a server from before the repository) is
  never new. A player who never opened the screen sees every entry as new.
- **The list.** The signed-in user's Live worker reads `GET /v1/mod-packs`
  and returns up to 40 releases with their full membership, ordered by
  most recent member. When an older server replies 404 the worker reads
  its per-map repository. Both paths use the account's `read` token
  (`live_hle::Hle::mod_list`, `live_hle::mods`). A thread of ours
  (`h2-mods`) asks for it when the Live menu draws the row or the screen
  opens (at most every 30 s), every 5 minutes otherwise, and 15 s after a
  failure; the game thread never waits on it.
- **The screens** (`boot::mods_screen`, our list class, laid out as the
  level select is: list on the left with its bracket, picture top right,
  description under it, as the campaign picker). **Mods**: one row per
  release (at most 40, newest first), its name and its state as a mark after
  it: a star (U+2605) when new, a tick (U+2713) when installed, a down
  arrow (U+2193) and the percentage while downloading; nothing when it is
  only on the server. Halo 2's fonts have none of the three, so they are
  drawn into every font as the bot mark's gear is (`font::mod_marks`,
  added by `bot_mark::GearRewrite` with the gear, the capitals' height;
  drawn here from polygons, nothing copied). The focused map's picture is on the right and under
  it are the base and credit, size and downloads, changes and current state.
  A opens details. X downloads an available map or retries a failure. An
  installed row offers Details and Back. During a transfer the arrow and
  percentage update in place. An empty list offers Refresh. A failed read
  offers Try Again. The description gives the reason and says that A
  reads the list again. An explicit retry shows Loading and
  can run at the reader's next turn. The populated list keeps its order
  until the player leaves it, including while a pack or page is open.
- **Map packs.** The pack ID identifies a release even when two uploaders
  use the same title. A opens the pack and its member pages. The summary
  names the makers and shows the description, advisory when present,
  download size and installed count. X on the pack or **Download All**
  queues its missing members. The installer downloads one at a time,
  counts finished members towards overall progress, and offers failed
  downloads again. Each member can also be downloaded independently.
  Once every member is installed and has a slot, **Use pack** selects
  its versions for the lobby's base-map choices. **Use original maps**
  clears the selection. B returns to the parent row.
- **Installing.** Download runs `MapGate::install` on a thread of its own
  (`h2-live-mod`): the join's own checks and path (the manifest from
  `/v1/maps/{id}`, the base checked against the disc, each layer from
  `/v1/content/{sha256}` checked against its SHA-256, the package written
  as `<data>/maps/live-<id>/`, then the offers refresh). The
  action shows "Downloading N%" after each layer, then "Remove".
  A refusal says why: "Needs Lockout from your disc", "Taken down", "Not
  on the server", or "Download failed". The detail row offers Retry download.
- **The download count.** Once a package is installed (or found here
  already), the gate asks for its manifest again with a fresh single-use
  `read` token of the first account signed in (`Fetch::get_counted`,
  `maps::Reader`), and the server counts that account once per package
  (docs/live/live-api.md "The mod repository"). A join that installs the
  session's map counts the same way; a search's check never does. A
  reinstall adds nothing; a failed count is only logged.
- **The map select.** Change Map lists an installed release as a folder
  under Mods. Picking its map selects the exact member offer and applies
  that pack to later choices in Halo 2 and Map Pack. Its slot supplies
  the lobby text and preview. Individual offers, including members of
  an incomplete install, remain playable. [Map selection](map-select.md)
- **Previews.** The three screens are in the UI kit's inventory (`mods`,
  `mods.pack`, `mods.detail`; `h2 ui preview mods.detail`), over invented
  entries with an invented three-map pack (`mods_screen::spec`).
- **Limits.** The list does not page past 40, a map not downloaded yet
  shows no preview image of its own, and a new version of a mod is another
  `MapId` that nothing links to the old one.

### A mod's page

**Why this layout.** A selectable list whose rows do nothing is a layout
no retail screen uses. Every screen tag in `mainmenu.map` was read for a
retail place that shows information that isn't pressed: the
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
(`crates/extensions/game/src/mods_screen/page_layout.rs`; each rectangle, font,
colour, animation and depth bias is the lobby tag's own unless noted):

| Part | Lobby original | On a mod's page |
|---|---|---|
| Action | Buttons on `name_field` | One skin-0 row at (-262, 147). Download, progress, retry and removal share the row. |
| Body and hint | The grid on `lobby_display` | Three lines of published description above a separate one-line hint. Installed maps point to Change Map > Mods. |
| Picture | The lobby's `unknown_map`, 220 by 207 | At (-258, 30) below the panel. The class uses the selected map's bitmap when available and restores the placeholder otherwise. |
| Table | White player names left, pale blue values right, 22 units between rows | Ten fact rows, with a gutter between the labels and values. Map, maker, uploader, size, downloads, date, version, modes, disc and join behaviour. |
| Checklist | The lobby's settings lines | Changes, then four rows of two. Each ticked box names an affected feature. |

The focused row and transfer state determine the footer. On the detail
page A or X downloads, retries, or removes. Both buttons share a verb
with a visible OR between them. During a transfer the footer offers B
Back and the hint says the player can leave. The page sets its panel
strings and footer again when the download state changes.

**A** on the action, or **X**, downloads the map; on a map that is here
they remove it, on a second press (the first turns the row to `Remove?
Press again`; opening the page again forgets it).
Remove (`MapGate::remove`) deletes only a `live-<id>` directory of
`<data>/maps` (one the Mods screen or a join wrote; a package put there by
hand is refused), then tells the boot to retire that generated offer.

**The picture** in Select Map, Game Setup and the lobby comes from the
chosen slot. The Mods browser and its detail page look up the base
map's bitmap by filename (`mod_slots::pictures`). They use the neutral
lobby picture when the base bitmap is missing.

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
