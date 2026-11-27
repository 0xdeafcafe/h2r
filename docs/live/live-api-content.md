# Live API: custom maps and packages

## Custom maps

Custom and modded maps are shared as content: the layers (file patches,
content packs, user-made maps, preview images) by SHA-256, and the package
manifest by `MapId` ([maps.md](../game/maps.md) §1, §5). Retail map bytes are never
stored: players use their own disc for the base.

| endpoint | authentication | result |
|---|---|---|
| `PUT /v1/content/{sha256}` | account token, `lxm: content.write`; the raw bytes | 201 `{sha256, size, whole_map, stored: true}`; 200 `{..., stored: false}` if already kept |
| `GET /v1/content/{sha256}` | public, or `read` on a private server | streams a download (`ETag` the hash, `private, no-store`); 404 for absent or uncleared images; 410 for general content takedowns |
| `POST /v1/content/{sha256}/report` | account token, `lxm: content.write`; `{reason}` | 201 `{ok, position}` |
| `PUT /v1/maps` | account token, `lxm: content.write`; a signed `MapPublished` event (compact JWS) | 201 `{map, missing, position}`; 200 when the author republishes it |
| `GET /v1/maps?sort=&base=&author=&category=&q=&limit=&cursor=` | read rule | `{items: [summary], next, position}`, newest first or most downloaded first (see "The mod repository") |
| `GET /v1/maps/{map_id}` | read rule; a `read` token counts a download | `{map: summary + manifest, missing, position}`; 404; 410 if taken down |
| `GET /v1/maps/{map_id}/image` | read rule | the preview with its image content type; 404 if none |
| `POST /v1/maps/{map_id}/report` | account token, `lxm: content.write`; `{reason}` | 201 `{ok, position}` |
| `PUT /v1/maps/{map_id}/credit` | admin or operator token, `lxm: content.moderate`; a credit `{authors?, release?, released?, source_url?, notes?}` | 200 `{ok, position, map: summary}`; `{}` clears the admin's credit; 403 for others; 404 if not published; 400 when malformed |
| `PUT /v1/maps/{map_id}/details` | admin or operator token, `lxm: content.moderate`; details `{description?, images?: [{sha256, caption?, source_url?, credit?, licence?}], contents?, features?, pack?}` | 200 `{ok, position, map: summary}`; `{}` clears the admin's details; each image must be stored content that is a PNG or JPEG; 403 for others; 404 if not published; 400 when malformed |
| `PUT /v1/maps/{map_id}/census` | admin or operator token, `lxm: content.moderate`; `{map_id}` is the decimal level id a hopper entry names (30 is Zanzibar); a census `{labels: {text: [{class, number?, team?}]}, tags: {path: object type}}` (`rules::labels::Census`), or `null` to clear | 200 `{position, map_id, set, blocked: [{hopper, entry, map_id, problems}]}`: the published hopper entries on that map the census blocks; 403 for others; 400 when malformed or over the limits |
| `GET /v1/maps/{map_id}/images/{n}` | read rule | the map's gallery image at index `n` (from 0), with its content type; 404 if none or optional `?v=sha256` differs |
| `GET /v1/mod-packs` | read rule | `{items: [pack], position}` |
| `GET /v1/mod-packs/{id}` | read rule | `{pack, position}` |
| `PUT /v1/mod-packs/{id}` | uploader, `content.write` | publishes the pack's metadata and complete membership, returns `{pack, position}` |
| `POST /v1/mod-packs/{id}/report` | account, `content.write`, body `{reason}` | records a report on the pack |
| `POST /v1/admin/mod-packs/{id}/{action}` | admin or operator, `content.moderate` | applies hide, unhide, delete, block or unblock to the pack |
| `POST /v1/maps/{map_id}/screenshot-capture-session` | map uploader, `content.write` | 201 `{game_id, token}` for a private capture session |
| `PUT /v1/mod-packs/{id}/screenshots/capture/{slot}`, `PUT /v1/maps/{map_id}/screenshots/capture/{slot}` | uploader, `content.write`, capture proof header and raw image | 200 `{ok, sha256, position, status}`; 400 for invalid proof or image; 403 for a wrong account, game or map; 410 for a removed image |
| `GET /v1/mod-packs/{id}/images/{n}` | read rule | pack gallery image at index `n` (from 0), with its content type; 404 if none or optional `?v=sha256` differs |
| `POST /v1/screenshots/{sha256}/report` | account, `content.write` | hides a visible screenshot and returns `{ok, position}` |
| `GET /v1/admin/screenshots?cursor=` | admin or operator, `content.moderate` | up to 40 pending items, oldest first, and `next` (a decimal cursor or null) |
| `GET /v1/admin/screenshots/{sha256}/image` | admin or operator, `content.moderate` | pending WebP, with `private, no-store` |
| `POST /v1/admin/screenshots/{sha256}/{action}?revision=` | admin or operator, `content.moderate` | `approve` or `remove`; 200 `{ok, position}`; 409 if the review has changed |
| `GET /v1/mappacks` | read rule | `{items: [{id, name, description?, authors?, released?, source_url?, images?}], position}` |
| `PUT /v1/mappacks/{id}` | admin or operator token, `lxm: content.moderate`; a pack `{name, description?, authors?, released?, source_url?, images?}`, or `null` to clear | 200 `{ok, position}`; ids are 1 to 64 of a-z, 0-9 and -; 400 when malformed |
| `GET /v1/mappacks/{id}/images/{n}` | read rule | the pack's `n`th image; 404 if none |
| `GET /v1/hoppers/pins` | read rule | `{pins: [{hopper_id, map_ids}], position}` |
| `GET /v1/hoppers/clankers` | read rule | `{hoppers: [{hopper_id, playlist_id, players, teams, ranked, ranks_as?}]}`: the served file's clanker hoppers and which ones rank their games (live-matchmaking.md §6.8) |

Uploads (`PUT /v1/content`) stream to `<data>/tmp`, hashed as they
arrive, and the SHA-256 of the body must be the path's (400 otherwise).
The blob is then kept once, as `content/<first two hex digits>/<sha256>`
in the server's private blob store (`<data>/content/...` by default, or
an R2 bucket with `--blobs s3://BUCKET`, [live-hosting.md](live-hosting.md#free-hosting-sqlite-and-cloudflare-r2)),
and never changed. It is always served through the server, never by a
redirect (the game's client follows none, and a takedown must be
immediate); a server-signed `ContentStored`
event (`content-<sha256>` stream; `{sha256, size, uploader}`) records it.

A body starting with a map's `daeh` header is a whole map; anything else
is a layer (a patch, pack or image). Both are capped at 95 MiB by
default (413 over either; `h2 live serve --content-layer-mib N
--content-map-mib N`): the largest whole MiB under the 100 MB request
body Cloudflare passes to a Worker (`live_server::content::EDGE_BODY_BYTES`;
the edge refuses larger bodies with its own 413 before the Worker runs),
so the native server and the Worker take the same uploads. A layer can
be large: Night Ops' delta over Containment is 68 MiB. Each account stores at most
1 GiB (`--content-quota-mib N`), counting what it uploaded first and not
deleted (413 over it). Uploads and publishes are rate limited per account
(a burst of 20, then one every two seconds; 429).

A hash on the operator's
retail list is refused (403), and so is a hash an admin blocked (410). A
server with no data directory stores nothing (503).

A package is published as an event the author signs with their own key,
like the account's other events: stream `package-<map_id>`, type
`MapPublished`, data `{manifest, image?, features?, gametypes?, version?, category?, source_url?, credit?}`. `manifest` is the package
manifest as JSON (the TOML's tables and keys, with `[[patch]]` and
`[[pack]]` as the arrays `patch` and `pack`); `image` is the SHA-256 of the
preview, uploaded like a layer, given exactly when the manifest names one.
A signed manifest can retain its preview reference while the repository
withholds the image. The public summary and downloads expose an image
only when Live approves it or an admin includes it in curated details
or pack art. `category` (one of `gameplay`, `weapons`, `vehicles`, `visuals`, `maps`,
`other`) and `source_url` (an `http` or `https` URL of at most 300
characters, where the mod came from) are the repository's display data,
outside the `MapId` like the manifest's name (400 when malformed).

So are
the in-game Mods page's (docs/game/maps.md §5, "A mod's page"):
`features` (what the mod changes, each once, of `vehicles`, `weapons`,
`objects`, `textures`, `shaders`, `scripts`, `geometry`, `ai`;
`h2 live maps import` derives them), `gametypes` (each once, of `slayer`,
`ctf`, `assault`, `oddball`, `koth`, `juggernaut`, `territories`; none:
any) and `version` (one line, at most 24 characters); each optional and
left out when empty. A server from before them refuses a document that
has them (the document allows no unknown keys): update the server first.
`credit` is who made the mod when the publisher did not: `{authors: [..],
release?, released?, source_url?, notes?}`, `authors` the original
creators as the release credits them (at most 12, each 1 to 64 characters
on one line), `release` the original release's name (at most 120),
`released` its date (`YYYY`, `YYYY-MM` or `YYYY-MM-DD`), `source_url` as
above, `notes` at most 500 characters. Without it the manifest's
`[package] author` (one name) and `source_url` stand in.

Servers before
credits refuse a document that has it (unknown field: 400).
The server checks the manifest as the runtime does ([maps.md](../game/maps.md) §7,
"Manifest checks"), computes `MapId::of` and requires the stream to be that
id's; refuses a manifest with no layers (only a retail base: 403) or naming a
retail hash (403); refuses it while any layer or the image is not stored
(409, naming the files); and checks the image is at most 512 KiB and starts
with a PNG or JPEG signature. The first publisher of a `MapId` is its
author (another account: 409).

The usual signature, key, five-minute clock
and nonce checks apply.

A summary is `{map_id, name, description?, author: {xuid, gamertag}, base:
{map, sha256?, derived}, layers: [{sha256, size, kind: "patch"|"pack"}],
size, image?, features?, gametypes?, version?, geometry?, published_at,
position, downloads, credit?, category?, source_url?, credits?}`: `size` is the layers' bytes (what a player
without them downloads), `image` the preview's hash, `geometry` the
manifest's `[result] geometry` (set when the mod changed the map's
geometry), `position` the `MapPublished` event's log position, `downloads`
how many accounts fetched the manifest. `author` is the account that
published the package (its key signed the manifest), never who made the
mod: that is `credits`, an admin's correction (below) if there is one,
else the publisher's `credit`, else the manifest's `[package] author`;
`credit` is its authors on one line (`A`, `A & B`, `A, B & C`) for
readers that know only that, and `source_url` is the credit's when it
has one. An admin's details (`PUT /v1/maps/{id}/details`) add `images`
(`[{sha256, caption?, source_url?, credit?, licence?}]`, served at
`/v1/maps/{id}/images/{n}`), `contents` (the report's lines) and `pack`
and `pack_name`; they replace `description` and `features` when set.
The first visible gallery image supplies `image` if the manifest has
no cleared preview.

A list page
then has `packs: [{id, name, ...}]` for the packs its items name; a
map's own answer has `report` (the whole contents report:
`{lines, classes, swaps, stats, placements, scripts}`) and `pack_info`.
`missing` lists
layers the server no longer holds (deleted by an admin). Lists leave out
taken-down packages. Servers before the repository send no `position` or
`downloads`; read them as 0.

### The mod repository

The web and game browse releases at `/v1/mod-packs`. A pack includes
`id`, `name`, original `authors`, `uploader: {xuid, gamertag}`,
`members` (map summaries), `map_count` and `size` in bytes, counting each
shared layer once. It may have a description, version, release year,
source URL, gallery and an `advisory` of `cheating_pack` or `not_for_online`.

The advisory is displayed with the release.

To publish, upload the layers and sign each `MapPublished` document with
`staged_for` set to the destination pack ID. The catalogue makes these
members available together when the uploader puts the pack. The event
feed shows the staging manifests only to the uploader and administrators.
The pack body contains `name`, `authors` (one or more), `members` (one
to 64 distinct MapIds from the same uploader), and optional
`description`, `version`, `source_url`, numeric `year` and `advisory`.

Each explicit pack contains at most one member for a given base level. A map already bound to an explicit pack stays in that pack. A missing member returns 404, a different uploader gets 403, and a conflicting membership returns 409.

The pack ID for an imported release comes from
`live_proto::maps::mod_pack_id_for`. Replaying older map events also
produces packs. Explicit curated membership supplies the release where
available.

Otherwise a title of the form `<release>: <map>` groups with
the same release from the same uploader. A standalone map forms a pack
of one. Member downloads and lobby joins keep their full MapIds.

#### Verified screenshot uploads

Each map and pack has five capture slots, `capture/0` through
`capture/4`. The game provides
`{game_id, tick, map_id, map, camera: {position, forward, up}, digest}`
in the `X-H2-Capture-Proof` JSON header. `digest` is the SHA-256 of the
request body. The camera uses game coordinates with Z up.

For a private
capture, add `permit` from the capture-session response. Live verifies
that permit against the account, game ID and exact MapId for 15 minutes.
The map name is the package's base name.

For a recorded game, Live checks its roster, exact MapId, map name and
recorded tick range. A film without a matching game record cannot establish
its exact MapId. Pack uploads must show a member's map.

The
arbitrary-file `upload` slots do not accept submissions.

Live accepts PNG, JPEG and WebP with valid proof, within 5 MiB, 4096 pixels
per side and 8 megapixels. It converts the image to opaque WebP, at most
1280 pixels per side and 512 KiB. The normalised hash in the receipt is
the ID for reports and review. The uploader's accepted receipt says
`pending` or `approved`.

The gallery uses dense indices in slot order
followed by curated images. Reportable entries carry `reportable: true`.

Live compares a 48×27 image against projected edges from an admin-supplied
outline stored under the exact MapId. A mismatch or insufficient geometry
requires review. The classifier scores the same normalised image that Live
serves.

Scores above 0.20 and unavailable or invalid scores require review.
Scores of 0.80 or higher flag the image for an upload strike.
Private and player-hosted captures require manual approval
even when those checks clear.

The permit authorises the capture session and
the digest binds the upload bytes. An uploader controls the pixels produced
by a game running on their own device.

The pending queue's items give `{sha256, revision, uploader, reason, nsfw,
proof}`. The revision is a decimal string identifying the current review.
Proof in the queue omits the permit.

A report hides the image across its
map, pack, preview and direct content URLs. Each account can report the same
image once. Approving restores the image.

Removing it permanently blocks
that hash. [Screenshot review](live-admin-moderation.md#screenshot-review)

Implementation: [shared screenshot routes](../../crates/live/live-server/src/content/screenshots.rs).

Clients that select or download individual maps use the paged map list:

```
GET /v1/maps?sort=new&limit=20
GET /v1/maps?sort=new&limit=20&cursor=<the previous page's next>
```

```json
{
  "items": [
    {
      "map_id": "<64 hex>", "name": "Lockout Snipers",
      "description": "...", "author": {"xuid": "0009...", "gamertag": "Modder"},
      "credit": "who made it", "category": "weapons",
      "source_url": "https://...",
      "credits": {"authors": ["who made it"], "released": "2005-03",
                  "source_url": "https://..."},
      "base": {"map": "lockout", "sha256": "<64 hex>", "derived": false},
      "layers": [{"sha256": "<64 hex>", "size": 412345, "kind": "patch"}],
      "size": 412345, "image": "<64 hex>",
      "published_at": "2026-11-18T10:00:00Z",
      "position": 4211, "downloads": 17
    }
  ],
  "next": "4211",
  "position": 4300
}
```

- The id is `map_id` and the cursor is `next`, as everywhere in this API.
  `next` is null on the last page (one shorter than `limit`); a page as
  long as `limit` gives one, which may lead to an empty page.
- `sort`: `new` (the default: newest first, by `position`) or `popular`
  (most `downloads` first, ties newest first). Anything else is 400.
- Filters, all optional, empty ones ignored: `base` (the base map's name,
  any case), `author` (the publisher's gamertag; one no account has
  matches nothing), `category` (400 when not one of the six), `q` (a
  case-insensitive substring of the name, description, credited authors
  or release name, base map name or publisher's gamertag).
- `limit` 1 to 1000 (default 100). `cursor` is opaque: for `new` it is the
  last item's position (`"4211"`), for `popular` its downloads and
  position (`"17:4211"`); one of the other sort's form is 400.
- **New since last time** needs no per-account state: a client keeps the
  highest `position` it has seen and counts the items above it as new.
  `position` is the package's own, so a republish by its author (same
  `MapId`) does not move it; a package an admin deleted and its author
  published again is new again.
- **Downloads.** `GET /v1/maps/{map_id}` with an `Authorization: Bearer <token>`
  header (an account's `read` token; tokens are single use, so a fresh
  one per request) counts that account once per package: the first time,
  the server appends a server-signed `MapDownloaded` event (stream
  `package-<map_id>`, data `{map_id, by}`) and answers with the new count.
  Without a token, or with one that does not check out on a public server,
  the read is answered and counts nothing. The web opens member pages anonymously on a public server.
  A private server needs the token anyway. `MapDownloaded` events, like
  reports, show in the log to admins only. The count is kept by the log's
  projection, replayed at startup, so both homes (native and the Worker)
  count the same way, with no background task.
- The picture is `GET /v1/maps/{map_id}/image` when `image` is set, else
  the base map's art.
- **Details and packs.** `PUT /v1/maps/{map_id}/details` (the same
  tokens) appends a server-signed `MapDetailed` event (stream
  `package-<map_id>`, data `{map_id, details?, by?}`): a description of
  up to 4000 characters, up to 8 images already uploaded as content (each
  checked to be a PNG or JPEG), a contents report, a features list and a
  pack id. `{}` clears it. `PUT /v1/mappacks/{id}` appends `MapPackSet`
  (stream `mappack-<id>`, data `{id, pack?, by?}`); `null` clears it.
  Both are display data only, like the credit. `h2 live maps details
  --file docs/game/mod-details.toml` applies a file of them, and `h2 live
  maps analyse` writes the contents reports.
- **Census, and the hopper gate.** A rules script declares the Forge
  labels and tags its map must hold
  ([rules-start-gate.md](../game/rules-start-gate.md)). Live holds no map
  bytes, so an admin sends each map's census (`PUT
  /v1/maps/{map_id}/census`, `{map_id}` the level id a hopper entry
  names): a server-signed `MapCensusSet` on `mapcensus-<map_id>`, data
  `{map_id, census?, by?}`, projected by the catalogue in both homes.
  `h2 live maps census MAP` reads it from the admin's copy of the map.
  `PUT /v1/titles/{title}/hopper` then refuses, with 400 and the
  playlist, entry, map and problem named, any hopper edit whose entry
  pairs a script with a map it does not fit: an unknown script, a script
  that declares labels or tags on a map with no census, or a label the
  census does not satisfy. A new census answers with the published
  entries it blocks.
- **Credits.** `PUT /v1/maps/{map_id}/credit` (an admin's account token or
  the operator's, `lxm: content.moderate`) sets who made a published
  package, over what its publisher said: the body is a credit as above
  (trimmed; blank fields dropped). The server appends a server-signed
  `MapCredited` event (stream `package-<map_id>`, data `{map_id, credit?,
  by?}`, public) and answers with the new summary. An empty body object
  (`{}`) clears it, and the publisher's credit shows again. The manifest
  the publisher signed, its `MapId` and every session on it are untouched:
  this is display data only. The correction is kept by `MapId`, so a
  package deleted and published again keeps it. `h2 live maps credit`
  sends it, one package by flags or many from a file
  (`docs/game/mod-credits.toml`).

The client side (`h2 live mods import`, `h2 live mods capture`, `h2 live maps publish DIR`, `h2 live maps import FILE`,
`h2 live maps list`, and the join-time download) is in
[maps.md](../game/maps.md) §5, "As built" and "Importing a mod".

## Rules scripts

Moved to [Live API: rules scripts and game variants](live-api-rules.md#rules-scripts).

## Game variants

Moved to [Live API: rules scripts and game variants](live-api-rules.md#game-variants).

## Packages

A package ([studio-redesign.md, "Packages"](../editor/studio-redesign.md#packages))
goes up as its files, each as content (`PUT /v1/content/{sha256}`, "Custom
maps" above), then one document: `{manifest, lock, credit?}`
(`live_proto::packages::PackageDocument`), signed by the publisher's own
key as a `PackagePublished` event on `pkg-<PackageId>`. Both homes serve
these routes (`crates/live/live-server/src/packages.rs`; wire types and the
upload check in `crates/live/live-proto/src/packages.rs`).

| endpoint | authentication | result |
|---|---|---|
| `PUT /v1/packages` | account token, `lxm: content.write`; the signed event (compact JWS) | 201 `{package, manifest, lock, jws, author, position}`; 200 when its publisher sends it again; 409 when another account published it |
| `GET /v1/packages` | read rule | `{items: [package], position}`, newest first; a package using a taken-down file is left out |
| `GET /v1/packages/{id}` | read rule | as `PUT`'s answer; 404; 410 when a file it uses is taken down |
| `PUT /v1/packages/{id}/credit` | admin or operator, `lxm: content.moderate`; a credit | 200 `{ok, position, package}`; `{}` clears the admin's credit |
| `PUT /v1/admin/model-uploads/{gamertag}` | admin or operator, `lxm: content.moderate`; `{approved, note?}` | 200 `{ok, position}` |

`package` is `{id, slug, name, description?, publisher: {xuid, gamertag},
credit?, kinds, items, sim, requires?, features?, size, published_at,
position}`. `jws` is the event as published and `author` the account id
whose key signed it: a client verifies both again on install
(`live_proto::packages::verify_answer`), and each downloaded file against
its lock hash (`verify_file`).

**The upload check** (`live_proto::packages::check`, one function both
homes call over their store's `Holdings`), after the signature:

| Step | Refused with |
|---|---|
| The signature is the token issuer's key's | 401 |
| The manifest validates (`content::package::Manifest::validate`) | 400 |
| The lock names exactly the manifest's files, at most 4096 | 400 |
| `PackageId::of(manifest, lock)` is the stream's | 400 |
| No `kit` item (first-party packages only) | 403 |
| A `model` or `animation` item: the uploader's `model_uploads` is on and its uploads are on | 403 `model_uploads_not_approved: ...` |
| No locked hash is on the retail list | 403 |
| No locked hash is hidden or blocked | 410 |
| Every locked hash is stored | 409, naming each missing file |

**Credit**, in order of truth: an admin's (`PackageCredited`), the
publisher's signed `credit`, the manifest's `[credit]`. The publisher is
`publisher` ("uploaded by"), never the author.
