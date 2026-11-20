# Live service: admins, the MOTD editor and the playlist builder

Who may publish title files — the message of the day and
`multiplayer_hopper.dat`, the playlists — and the web admin area that edits
them. Builds on [live.md](live.md), [live-api.md](live-api.md),
[live-security.md](live-security.md) and the formats in
[live-motd.md](live-motd.md).

## 1. The admin role

An admin may publish title files where otherwise only the server operator's
key can. Admins come from two places:

- **Configuration, read once at startup.** `h2 live serve` takes
  `--admin <gamertag>` (repeatable), and both it and the embedded server
  `h2 play --live-local` starts read `H2_LIVE_ADMINS`: a comma-separated list of
  gamertags, trimmed and compared case-insensitively, merged with the
  flags. There are no built-in admin names — no gamertag is an admin unless
  someone configured or granted it. An invalid gamertag in either place is
  a startup error, so a typo cannot silently widen who is an admin. The
  server logs the configured admins (gamertags only) when it starts:

  ```
  h2 live: admins configured: Bob, afr
  ```

  Configured admins need no account to exist; when the account exists, its
  reads say so. Configuration cannot be revoked at runtime.
- **The log.** The operator runs
  `h2 live admin grant <gamertag>` / `h2 live admin revoke <gamertag>`
  (signed with the server key in `--data`, like `motd set`). Grant appends a
  server-signed `AdminGranted` `{xuid, gamertag}` event on the account
  stream, revoke an `AdminRevoked` `{xuid}`; the server projects the set in
  memory and replays it at startup, so grants survive a restart. Granting
  twice, revoking what was never granted, or touching a configured admin is
  a 409; an unknown gamertag a 404.
- The embedded server of `h2 play --live-local` also makes the account it is
  about to play as an admin for its lifetime, on the owner's own machine:
  an in-memory grant, re-made every run, never written to the log.

The account read API carries `admin: true` for admins
(`GET /v1/accounts/{gamertag}`, the accounts list), which is how the web
app knows to show its Admin area. `GET /v1/admins` lists the configured and
granted admins with their source. Title-file writes (`titles.write` tokens)
are accepted from the server key and from admin accounts' self-issued
tokens; everything else about the endpoints is unchanged.

## 2. The admin API

The editors speak structured JSON; the server builds the binary with
`live-proto` and publishes it as the same title file
`h2 live motd set` / `hopper import` publish. Reads are public like every
read; writes need a `titles.write` token (operator or admin).

### `GET /v1/titles/{title}/motd`

The published `message_of_the_day.dat` as its editable fields, 404 until
one is published (the embedded server publishes a default at start, see
below). Public on a public server: the dashboard shows the header and the
stats site's "Did You Know?" box cycles the messages from this read.

```json
{ "header": "Halo 2 is back", "messages": ["Welcome, Spartan.", "…"] }
```

### `PUT /v1/titles/{title}/motd`

The same shape, admin-only. The server writes the same text into all nine
language slots (as `motd set` does) and answers
`{"published": {name, sha256, size}, "motd": …}`. Limits as the format
requires: 1..=32 messages, at most 255 UTF-16 units each, and a built file
of at most 32,768 bytes (`0x8000`, the MOTD channel's download buffer,
[live-motd.md §2.2](live-motd.md); a larger file would fail the game's
download and show nothing). The server writes the 16-byte header the game
validates (CRC, version 1, size; `live_proto::title_file`). A refused PUT
leaves the published file as it was. The hopper PUT is the same with
version 4 and a 131,072-byte (`0x20000`) cap.

### When the game shows it

The game fetches the MOTD from title storage once per Xbox Live sign-in
([live-motd.md §2](live-motd.md)): sign-in arms the channel, the resource
manager downloads `9809/message_of_the_day.dat`, validates the header and
only then sets the flag the two screens check. Nothing re-arms the channel
mid-session, so **a MOTD published while the game is signed in shows
after the next sign-in** (sign out and back in, or restart the game). The
editor says so after a publish. The Live main menu shows the header once
per screen visit; the matchmaking screen shows a random message,
re-picked every 15 s.

Files published before the header was traced carry 16 zero bytes there,
which the game rejects. On start, the server (`h2 live serve` and the
embedded one) republishes such a MOTD or hopper with a real header and the
same body (`crates/live/live-server/src/title_headers.rs`); reads through the
admin API accept them meanwhile.

### The default MOTD and keeping what you publish

`h2 play --live-local` without Postgres keeps its log in the live data directory
(`server/log`: `events.jsonl`, `blobs/`, `checkpoints.json`;
`crates/live/live-server/src/filestore.rs`), so accounts, friends, games and
published title files survive a restart; the game reads the MOTD only at
sign-in, so a publish needs to outlive the session. Only if that directory
is held by another running server does it fall back to memory (with a
warning).

When no MOTD has ever been published, the embedded server (on Postgres or
the file store) publishes a default one at start
(`crates/live/live-server/src/default_motd.rs`): a short header and 21 original
"Did you know?" lines and gameplay tips. It never replaces a published one;
edit it like any other.

### `GET /v1/titles/{title}/hopper`

The playlists as JSON — 16 hoppers, each with its records (the entries
within the hopper, the first `record_count` of them):

```json
{ "hoppers": [ {
    "index": 0, "type": 4, "unused": false, "status": 1, "record_count": 2,
    "name": "", "title": "Big Team Battle",
    "description": "Join alone or as a party…", "ranges": [1, 8, 6],
    "dwords": [ …18 numbers… ],
    "records": [ { "id": 1, "string": "", "flags": 1, "dwords": [1, 0],
                   "variant": { "name": "Team Slayer", "engine": 2, "team": true },
                   "builtin": "team_slayer" } ]
  }, … ],
  "variants": [ { "key": "team_slayer", "name": "Team Slayer", "engine": 2, "team": true }, … ],
  "templates": [ { "id": 2, "type": 4, "title": "Team Slayer", "description": "…",
                   "min_players": 6, "max_players": 8, "ranges": [2, 3, 4],
                   "dwords": [ …18… ], "entry": { "id": 50, "variant": "team_slayer" } }, … ],
  "maps": [ { "id": 50, "name": "Lockout" }, … ],
  "position": 1042 }
```

- `unused`: the slot is type 0, which the game reads as no hopper. The file
  always has 16 slots; the built-in lineup fills 11 and leaves 12..16
  unused. Its records are empty.
- `variants`: the built-in game variants the server can write into an
  entry by key (the stock record of a game type with the default lineup's
  changes, built from numbers in `live_proto::hopper::defaults`, never a
  disc). `builtin` on a record names the one its variant is byte for byte,
  if any.
- `templates`: the default lineup's human playlists, each a kind of
  playlist a new one is made from: its type, sizes, waits and skill table
  (the descriptor the game's validator wants, which the JSON fields alone
  cannot build) and a first entry that suits it. `ranges` and `dwords` are
  that descriptor's, so a builder's copy of a new playlist is whole.
- `maps`: the retail multiplayer maps by entry id, the map list when the
  owner's catalog (below) is not there.

For Halo 2, a server with nothing published serves the built-in lineup
([live-matchmaking.md §6.8](live-matchmaking.md)) here and from the file
endpoint; other titles 404 until a file is imported. What the fields are
(the full layout is [live-matchmaking.md §6](live-matchmaking.md)):

- `title` / `description`: what the playlist list shows, the English one
  of nine per-language UTF-8 slots (`+0x0c`, 16 bytes each; `+0x9c`, 128
  bytes each). A write puts the text in all nine. At most 15 and 127 bytes.
- `type` 1..5: 1 free for all, 2 ranked free for all, 3 unranked teams,
  4 ranked teams, 5 clan match. `status` 0..3 (0 lets the game check the
  maps itself; 1 ok, 2 error, 3 none). The three `ranges` are the first
  three type parameters (`+0x5ec`): fewest and most players for types 1
  and 2; teams, fewest and most a team for 3..5. The descriptor's dword
  array carries the rest (§6.2 there).
- `record_count` is the data block's entry count (`+0x44`). The
  descriptor's `+0x08` is the playlist's stats id, which the server keeps
  as it is.
- `name`: the raw ASCII field at `+0x51b`. It overlaps a 128-byte table at
  `+0x51c` that the game validates (`0x00193560`: each byte `i..=0x7f`), so
  it normally reads empty and must not be changed; the server writes it
  only when it differs from what is there.
- Per record: `id` is a **map**: a multiplayer level id from the globals
  tag (`matg +0x178`, the lookup `0x0019c5f0`); an id that is not one
  marks the hopper as an error. `dwords[0]` (`+0x248`) is the record's
  **weight**: session setup (`0x00192eb0`) picks a record at random with
  probability weight / total, clamped to 1..1000. The rest of the slot from
  `+0x24c` is the record's **game variant**, 0x130 bytes that session setup
  copies out whole; `variant` decodes its name, game-engine index (1 CTF,
  2 Slayer, 3 Oddball, 4 King, 7 Juggernaut, 8 Territories, 9 Assault) and
  team flag, read-only. `string`, `flags` and `dwords[1]` lie inside the
  variant (`string` over its first 0x48 bytes, name and engine included;
  `flags` is its engine flags; `dwords[1]` its join-in-progress setting);
  the server writes `string` only when it changes, so a round trip keeps
  the variant whole.

Every byte the JSON does not name is preserved; `live_proto::hopper`
(`Descriptor`, `Rules`, `Entry`, `VariantRecord`) reads and writes all of
them.

### `PUT /v1/titles/{title}/hopper`

Admin-only. The body lists only the hoppers that changed; within one, only
the fields given change:

```json
{ "hoppers": [ { "index": 0, "name": "Big Team", "record_count": 3,
    "records": [
      { "id": 2, "string": "two first", "flags": 0, "dwords": [0, 0], "from": 1 },
      { "id": 1, "string": "one",     "flags": 0, "dwords": [0, 0] },
      { "id": 3, "string": "a copy",  "flags": 0, "dwords": [0, 0], "from": 1 },
      { "id": 4, "string": "",        "flags": 1, "dwords": [1, 0], "from": 0, "from_hopper": 5 }
    ] } ] }
```

`type` is the field's name, as the GET has it (`hopper_type` is read
too; before this the server read only `hopper_type`, so a type edit from
the builder was silently dropped).

`records`, when present, replaces the whole list (1..100 entries;
`record_count` must then match or be left out). Each entry keeps its
unknown bytes — the template blobs the game fills in — from the slot named
by `from`: the entry's position in the hopper as it stands now. Absent
means "this position", so an edit in place needs nothing; a reorder names
where the entry came from, a copy names its source, a removal is simply a
shorter list. `from_hopper` makes `from` name a slot of another hopper (as
the file stood before this patch): that is how an entry, game variant and
all, is copied between playlists. `title` and `description` are optional
like every other field. Nothing the builder cannot edit is ever lost, and the
published file re-inflates to the same 0x162cc0 image with the edits
applied. The reply is `{"published": …, "hoppers": …, "position": …}`, the
fresh state.

New playlists, removals and built-in variants:

```json
{ "hoppers": [
    { "index": 11, "template": 2, "title": "Modtacular",
      "records": [ { "id": 50, "string": "", "flags": 0, "dwords": [5, 0], "variant": "team_slayer" },
                   { "id": 20, "string": "", "flags": 0, "dwords": [1, 0], "variant": "classic_ctf" } ] },
    { "index": 10, "clear": true } ] }
```

- `template` (an id from the GET's `templates`) writes that template's
  whole descriptor first, with the lowest playlist id (1..100) no other
  playlist has, then the other fields on top. It is how an unused slot
  becomes a playlist: a patch naming an unused slot without one is refused
  ("an unused slot: create it from a template first"), and so is a new
  playlist with no entries. On a playlist it changes its kind.
- `clear: true` zeroes the hopper's descriptor and entries, making it an
  unused slot; nothing else may be sent with it. Pins on that index stay
  (they are the server's, by index) until changed.
- A record's `variant` (a key from `variants`) builds the entry afresh: the
  map `id`, the weight `dwords[0]`, that variant, every other byte zero, as
  the built-in lineup writes its entries. `string`, `flags` and
  `dwords[1]` lie inside the variant and are ignored; `from` may not be
  given with it.
- A playlist the game's validator took before the patch (or one made from
  a template) must still pass it after (`check_descriptor`: type, sizes,
  skill table, playlist id), so a raw `type` that does not fit the range
  fields is refused with the validator's reasons.
- Only the hoppers named are checked: unused slots elsewhere never block a
  publish.

This is `apply_patch` in `crates/live/live-server/src/admin.rs`; the Worker
serves the same router, so both homes check the same way.

### Importing the base image

```
h2 live hopper import <path> [--title 4D530064] [--server URL] [--data DIR]
```

`path` is the owner's own `n:\multiplayer_hopper.dat` (the raw image the
game caches) or a download envelope; the command shows the 16 playlists it
found and publishes the file. Game data never goes in the repo — only to
the owner's server. Until one is imported or edited, Halo 2's hopper
endpoints serve the built-in lineup.

```
h2 live hopper defaults --out FILE [--raw] [--force]
```

writes that built-in lineup (eight playlists and three Clankermatch ones
on retail maps, built from the format alone) as a download envelope, or
with `--raw` as the raw image the game caches, to edit offline or import
elsewhere.

## 3. The web admin area

The admin tools are split between the app's two sides. The stats site's
**Halo Admin** (`/games/halo2/admin`, "Halo Admin" in its nav, shown only when the
signed-in browser account is an admin: the `admin` field above) keeps only
the game's own things: what it shows and plays. Running the service
(invites, accounts and admins, importing an account, the server's status)
is the dashboard's server screens (§5), styled like the rest of the
dashboard. Each side links to the other: the dashboard's "halo admin" pod
opens `/games/halo2/admin`, and the Halo Admin title bar's "Server admin" goes back
to `/dash/server`. The Halo Admin's three sections:

- **Message of the Day** — the header and the message blocks, with a
  preview in the stats site's style and the game's limits enforced in the
  form (255 UTF-16 units, 32 messages). After a publish it notes that the
  game picks the new text up at its next sign-in.
- **Playlists** — the 16 hoppers: pick one, edit its title and
  description, and per entry its map (a picker of the game's maps, by
  name, when the server has a catalog) and weight; each entry shows its
  game variant. Reorder, duplicate, add or remove entries with the same
  validation the server applies (title ≤ 15 bytes, description ≤ 127,
  1..100 entries). Entries remember where their bytes come from, so
  reorders and copies carry the game variant whole. **Advanced: raw
  fields** keeps the type, status, ranges, raw name and every record word
  editable as numbers. **Start from preset** fills the playlist from a
  historical lineup (below).
- **Custom content** — §7.

The same playlist editing is available offline in the native editor's
hopper tool (`h2-editor`, [editor.md](../editor/editor.md)): it opens and saves a
`multiplayer_hopper.dat` file (raw image or envelope) with the same
fields, validation and presets, and it can also give an entry another
game variant (a disc built-in, a saved variant or a `.h2variant` file),
which the web builder cannot; publish the result with `h2 live hopper
import`.

Saves are the browser's first writes: each signs a self-issued 60-second
token (`titles.write`) with the signed-in account's key, exactly as the CLI
signs its requests, and sends it as a bearer header.

### The catalog

`h2 live assets extract` (and `h2 play --live-local`, which runs it) writes
`catalog.json` beside the art manifest, from the owner's own disc; it is
served the same way, `GET /art/game/catalog.json`, to this machine only
unless `--share-game-art`. Nothing of it is in the repo. Its `version` (1)
changes only when a client could no longer read it; the extractor version
(part of the art cache key) was bumped so an existing art directory is
re-extracted once.

```json
{ "version": 1, "source": "<the art manifest's source>",
  "maps": [ { "id": 1, "name": "…", "description": "…", "key": "<map file stem>" },
            { "id": 301, "name": "…", "description": "…", "key": "<map file stem>",
              "kind": "campaign" } ],
  "playlists": [ { "key": "ffa", "title": "…", "description": "…" } ],
  "variants": [ { "name": "Team Slayer", "game_type": "slayer", "engine": 2 } ],
  "skipped": { "<list>": "why it is missing" } }
```

All three lists come from the main menu map (code:
`crates/apps/cli/src/live_catalog.rs`; the two level lists through
`tags::ui_levels`, the reader the boot's map select and the editor's Maps
list share):

- `maps`: the globals tag's multiplayer levels (`matg +0x178` element 0,
  block at `+0x10`, `0xc64`-byte entries: `+0` id, `+0xc` nine 32-unit
  UTF-16 names, `+0x24c` nine 128-unit descriptions, `+0xb4c` the scenario
  path, whose last part is `key`). These are exactly the ids a record may
  name. On the retail disc: the twelve shipped maps; the Map Pack's maps
  are not in this list, so presets naming them show them as unavailable.
  After them come the campaign levels, with `"kind": "campaign"` (a
  multiplayer entry has no `kind`). Their `id` is the level's map id, not a
  hopper record id, so a playlist must not offer them; `key` is the level's
  map file stem (what telemetry sends as `map`) and `name` the title the
  level select shows. They are the levels the game can start (`matg
  +0x170` element 0, `0x108`-byte entries: `+0` campaign id, `+4` map id,
  `+8` scenario path; [level-boot.md](../runtime/level-boot.md)), each named by the
  level select's entry with the same campaign and map id (`matg +0x178`
  element 0, block at `+0x8`, `0xb50`-byte entries: `+0` campaign id, `+4`
  map id, `+8` the level's picture as a tag reference, `+0x10` nine 32-unit
  UTF-16 names, `+0x250` nine 128-unit descriptions). On the retail disc:
  all 15 campaign map files, no id shared with a multiplayer map. When they
  cannot be read the multiplayer maps stay and `skipped` has
  `maps/campaign`.
- `playlists`: the `matchmaking_hopper_descriptions` strings, paired
  `<key>_title` / `<key>_description` (the seven compiled-in playlists,
  Rumble Pit to Training Ground).
- `variants`: the 39 built-in game variants of the variant-settings tag
  (name, game type, engine index; docs/game/community-gametypes.md).

Not in it, because nothing reads them reliably yet: the per-game-type
"max teams" bytes after a level's path (`+0xc54`, plausible but
unconfirmed), and anything about a variant beyond its name and type.

### Presets

`web/src/admin/presets.ts` holds the lineups: the seven launch playlists
(Rumble Pit, Team Skirmish, Head to Head, Big Team Battle, Minor and Major
Clanmatch, Training Ground) and later ones (Team Slayer, Double Team, Team
Snipers, Team Hardcore, Big Team Battle with the map packs, Shotty
Snipers, the later Rumble Pit). Each is a title and description (or the
catalog playlist whose text to use), optional range values, and its games
as descriptive references: game type, a built-in variant name to prefer,
team or not, map display names and a weight. They are written from
public knowledge; rotations and weights are approximate, and each preset
says what is uncertain.

A preset resolves in the browser against the catalog and the published
file. A map resolves by display name to its id. A game variant cannot be
authored (its 0x130 bytes are settings the builder does not model), so
each entry copies a whole published entry that plays the same game type
— preferring the named variant, then the same team play — with
`from`/`from_hopper` (the builder's per-entry variant list also offers
the server's built-in variants, which a preset does not use yet). Whatever does not resolve (a map the game does not
have, a game type no published entry plays, no catalog at all) is listed
as unavailable and left out; a preset with nothing resolved changes
nothing. So a preset can set: the title and description, the range
values it names, which maps, how often each comes up, and which existing
variant each plays. It cannot set the hopper type, the matchmaking
timeouts, or a variant's rules (Hardcore's or Snipers' settings come from
the source entry, not the preset).

## 4. Signing the browser in as an existing CLI account

The browser can only sign *up* (a new key, a new account); the owner's
admin account was made by the CLI, whose key the browser never saw. There
are two bridges: a link `h2 play --live-local` prints, for the browser on the
same machine, and an export, for anything else.

### The `h2 play --live-local` link

When `h2 play --live-local` runs the embedded server (not with `--live` or
`--live-server`)
and has the web app, it prints, next to the web app's address:

```
live: web app at http://127.0.0.1:50123/
live: sign this browser in as afr (one use, this machine only): http://127.0.0.1:50123/signin#once=…
```

Opening it signs the browser in as the account the game plays as, which
the embedded server has made an admin (§1), and lands on `/games/halo2/admin`
(or the dashboard if that account is not an admin). How it works:

- The CLI builds the same browser entry `account export` writes and hands
  it to the running server, in memory, under a token of 256 random bits
  (`Live::offer_local_signin`, `crates/live/live-server/src/local_signin.rs`).
  Nothing is logged or written to the event log; a restart forgets it.
- `POST /v1/local/signin` `{"once": token}` answers the entry once and
  forgets it. An offer lasts ten minutes; at most four are outstanding.
  An unknown, used or expired token all get the same 404; the response is
  `Cache-Control: no-store`; tokens are compared by SHA-256 digest without
  an early exit; attempts are rate-limited (a burst of 10, then one a
  second).
- The endpoint answers only a loopback TCP peer, with no `Forwarded` /
  `X-Forwarded-For` header (a local proxy forwarding someone else), a
  loopback `Host` (no DNS rebinding) and, when the browser sends one, an
  `Origin` naming that same host; it requires `application/json`, which a
  page of another origin cannot send without a CORS preflight the server
  refuses. To anyone else, and on a server that never made an offer, it is
  the API's ordinary "no such endpoint".
- The token rides in the URL fragment, which browsers send to no server
  and put in no `Referer`. The `/signin` page removes it from the address
  bar and history (`history.replaceState`) before redeeming it, then checks
  the key against the server, as the import below does, and keeps the
  entry in this browser's IndexedDB. If the browser is already signed in
  as someone else, it asks before switching.

What is left: the link is a bearer secret for its ten minutes. Anything
that can read the terminal (scrollback, a shared screen, a recorded
session) or any local process that can reach the loopback port in that
window can redeem it first; after that it is dead, and the page says to
run `h2 play --live-local` again. Once redeemed, the private key is in this
browser's storage exactly as after sign-up or an import: anyone with the
browser profile can read it.

### `h2 live account export`

For a browser on another machine, or a server the CLI did not start:

```
h2 live account export <gamertag> --file afr.json
h2 live account export <gamertag>          # prints it (and a warning)
```

The CLI checks with the server that the key it holds is still the
account's current key — the same check `account import` makes in the other
direction — and writes the browser's account entry (`{xuid, gamertag, id,
server, server_id, kid, key, x}`, the key the base64url seed) to a 0600
file, since it is a private key. With the web app open, go to
the dashboard's **import account** (`/dash/server/import`, also a row of the
sign-in list), paste the JSON or choose the file: the page checks the key against the server again and keeps it in this
browser's IndexedDB (the same warning as sign-up applies). The file is as
sensitive as `accounts.json`. There is no remote link or QR for this: a
link that works from another machine would have to carry the key itself
(the server never holds it) through whatever channel moved the link.

## Wiring

The admin module is self-contained (`crates/live/live-server/src/admin.rs`,
`crates/apps/cli/src/live_admin.rs`, `live_proto::hopper`); the pieces other
files own are small and listed in the lane report: module declarations,
route registrations, the `Policy.admins`/`Live.admins` fields, the
`admin` field on account reads, the CLI dispatch and the `--admin` flag,
and the embedded server's runtime grant.

## 5. The dashboard's server screen and admin invites

`/dash/server` (Settings → Server on the dashboard) shows every visitor
the server's public facts from `GET /v1/server`: id, public address,
software, whether sign-up needs an invite, whether reads are private,
accounts, online players and live games. Its four pods lead to another
server (the switcher, now `/dash/server/connect`), the event log, the
players and the stats site.

For an admin (`isMeAdmin`, the account read's `admin` field) it is "your
server", from `GET /v1/admin/status` (an `admin.read` token, admins and
the operator only; `crates/live/live-server/src/status.rs`):

```json
{ "id": "…", "software": "…", "url": "https://….trycloudflare.com",
  "invite_only": true, "private": false, "federation": false,
  "counts": {"accounts": 12, "online": 3, "live_games": 1, "sessions": 2},
  "position": 900, "started_at": "2026-11-16T10:00:00Z", "uptime_s": 93784,
  "public": {"url": "https://….trycloudflare.com", "tunnel": "quick"},
  "game_art": {"present": true, "shared": false, "manifest_version": 1,
               "extractor": 7, "source": "<disc cache key>", "maps": 12},
  "admins": [{"gamertag": "afr", "source": "configured"}],
  "log": {"position": 900, "head": 900, "store": "file"},
  "invites": {"open": 2, "used": 5, "revoked": 1, "expired": 0},
  "data_dir": "/Users/…/live/server" }
```

`tunnel` is `quick` (a `trycloudflare.com` name), `named` (another
Cloudflare tunnel host), `proxy` (`--public-url`) or null. `extractor` is
the art extractor version `h2 live assets extract` now records in the
manifest (null for older art). Nothing secret is in it: no keys, no file
contents, no database URL; `data_dir` only when the caller is on this
machine (a loopback peer with no proxy header, as for game art), so not
through a tunnel. The plate shows uptime, how the server is reached,
sign-up, reads, players, game art and the log's length (not the data
directory or the admin list, which are long). Its pods: **invites**,
**accounts**, **halo admin** (`/games/halo2/admin`) and another server.

**Accounts** (`/dash/server/accounts`, admins): the counts, then blades
for every player and importing an account, then each admin with how it
became one (set at startup, granted). Granting admin stays operator-only
(§1), so it lists and leads on rather than offering a button; it shows no
command lines either.

**Import account** (`/dash/server/import`, anyone, since it is a way in):
paste the account file or choose it, then "import and sign in" (§4).

Actions on these plates are blades (`web/src/dash/Blades.tsx`): focusable
rows moved through with up and down, A acts, X does a row's second action
(revoke, on invites), the same on keyboard, gamepad, mouse and touch.

**Invites** (`/dash/server/invites`, `crates/live/live-server/src/invites.rs`):
pick how long it lasts (1, 7 or 30 days; at most 30), make one, and the
code is shown and copied. Each is single-use, like the operator's. The
list shows open invites (copy again, revoke) and recent used, revoked and
expired ones, with who made and who used them.

- `POST /v1/invites`: an `invites.write` token from an admin account and,
  as the body (`application/jose`), an `InviteCreated` event on the
  `invites` stream, signed by the same key, data `{jti, exp}` (`jti` 16 to
  64 of `[A-Za-z0-9_-]`, `exp` in unix seconds, at most 30 days ahead).
  201 `{invite, jti, exp, position}`. The operator's form (no body,
  `?days=`) is unchanged.
- `GET /v1/invites`: admins only (`admin.read`); `{items: [{jti, by,
  by_gamertag?, at, exp, state: open|used|revoked|expired, used_by?,
  used_by_gamertag?, invite?}]}`, newest first. Open ones carry their code
  again (the server's signature over the same `jti` and `exp`, which is
  deterministic).
- `POST /v1/invites/{jti}/revoke`: an `InviteRevoked` event `{jti}` by an
  admin; 409 unless it is open.

At sign-up, an invite the log knows must be open and its maker still an
admin. Rate limit: per author a burst of 10, then one a minute. The
security reasoning is in [live-security.md](live-security.md) §3.

In the mock (`bun run dev`), `H2_MOCK_ADMIN=1` makes every account signed
up in the browser an admin, and the mock serves these endpoints
(`web/mock/admin.ts`; it parses but does not verify tokens and events).

### Invite codes people type

Every invite the log knows has a short code. Generated ones look like old
Xbox Live codes: 25 characters in five groups (`XXXXX-XXXXX-XXXXX-XXXXX-XXXXX`)
from `BCDFGHJKMPQRTVWXY2346789` (no vowels, no 0/O, 1/I/L, 5/S). The code is
derived from the invite's `jti` with a key derived from the server key, so
it is never in the log and the list can show it again while the invite is
open. An admin may pick a code instead (4 to 32 printable characters):
`POST /v1/invites` with `{"event": <InviteCreated JWS>, "code": "..."}`.
That code goes beside the signed event, never inside it; the server logs
only its MAC, as a server-signed `InviteCodeSet {jti, mac}` in the same
append, and the list shows it as a custom code. Codes match ignoring case,
spaces and dashes; a custom code already held by an open invite is refused
(409). The operator's invites (`h2 live invite create`, also `--code`)
are now logged the same way, as server-signed `InviteCreated` events. The
long signed codes from before still redeem.

From the CLI: `h2 live invite create [--days N] [--code TEXT]` signs as the
operator (`--data`) or, with `--as GAMERTAG`, as an admin account; `h2
live invite list` and `h2 live invite revoke <jti> --as GAMERTAG`.

## 6. Security notes

- Grant and revoke are operator-only (a server-key token with `lxm`
  `admin.write`): an admin can publish content and invite people, not
  make admins.
- Admin invites are logged as the admin's own signed `InviteCreated`
  events, rate-limited, and die with the admin's grant (§5,
  [live-security.md](live-security.md) §3).
- Admin publishes go through the same event log as operator ones
  (`TitleFilePublished` by the server key), so the audit trail does not
  say who clicked; the operator's own log of grants and the account reads
  do. (Per-author attribution of admin publishes is a possible follow-up:
  an `AdminPublished` event naming the admin.)
- `H2_LIVE_ADMINS` and `--admin` gamertags are compared case-insensitively;
  gamertags are unique per server, so they name exactly one account.
- The exported account file is a private key on disk, mode 0600; treat it
  like `accounts.json`.
- The `h2 play --live-local` sign-in link (§4) is a ten-minute, one-use bearer
  secret for that account, honoured only from this machine; treat the
  terminal it is printed in accordingly.

## 7. Custom content: moderation

Players share custom and modded maps through the server's content store
([maps.md](../game/maps.md) §5, [live-api.md](live-api.md) "Custom maps"). Admins
(and the operator) moderate it with tokens for `lxm` `content.moderate`:

| endpoint | does |
|---|---|
| `GET /v1/admin/content` | `{packs, maps, blobs, reports, uploads_off, position}`: every pack (including members, state and count of pack and member reports), every package (its summary plus `state` and `reports`), every stored blob (`sha256, size, uploader, state, reports, stored_at`), the reports newest first (`target, id, by, reason, at`), and the gamertags whose uploads are off |
| `POST /v1/admin/mod-packs/{id}/{action}` | applies one action to a pack and all of its members |
| `POST /v1/admin/maps/{map_id}/{action}` | acts on a package |
| `POST /v1/admin/content/{sha256}/{action}` | acts on a blob, by hash |
| `PUT /v1/admin/uploads/{gamertag}` `{allowed}` | switches an account's uploads and publishes off or on |
| `PUT /v1/hoppers/{hopper_id}/pins` `{map_ids}` | the custom maps a matchmaking hopper allows (empty: retail only) |

The actions, and the `state` they leave:

- `hide` → `hidden`: the bytes or manifest stay, but reads answer 410 Gone,
  lists leave it out, and sessions on it are refused. `unhide` undoes it.
- `delete` → `deleted`: a blob's file is removed (its uploader's quota is
  freed); a package's manifest is dropped. Reads answer 410. A fresh upload
  or publish of the same hash or `MapId` brings it back.
- `block` → `blocked`: deleted, and refused from now on (uploads and
  publishes answer 410). A hash can be blocked before anyone uploads it.
  `unblock` leaves it deleted.

Taking down a pack prevents downloads of its member manifests, images and
exclusive layers. Sessions cannot advertise any of those members. A layer
shared with a visible mod stays available to that mod. The admin can still
inspect the pack and its reports. The public event log also omits the
pack, member and exclusive blob streams. Unhiding the pack restores access to
members that have no separate takedown. Unblocking leaves the pack deleted
until its uploader republishes it.

A package is refused in sessions (410) when it, or any of its layers, is
hidden, deleted or blocked; a deleted layer alone shows up in the package's
`missing` list. Each action is a server-signed `ContentModerated` event on
the target's stream (`{target, id, action, by}`, `by` the admin's xuid),
reports are `ContentReported` (`POST /v1/mod-packs/{id}/report`,
`/v1/maps/{id}/report` or
`/v1/content/{sha256}/report` by any account), the upload switch is
`ContentUploadsSet` on the account's stream, and pins are `HopperPinsSet`
on `hopper-<id>`. All of it replays from the log, so it survives restarts.

**Pins.** The game's hopper file has no room for a `MapId`, so pins are kept
beside it, keyed by the `hopper_id` sessions advertise. A session with
`hopper` or `ranked` flags may only advertise a retail map or one pinned on
its hopper. A pin allows a custom map; it does not make a playlist play
it (live-matchmaking.md §6.10).

**The retail list.** `<data>/retail-maps.sha256` (next to the server's key)
holds the hashes of retail maps, in `sha256sum` output form:

```sh
cd /path/to/your/disc/maps && sha256sum *.map > <data>/retail-maps.sha256
```

Blobs with a listed hash are refused, so the store never holds a retail
map. A line that names its map (`<hex>  lockout.map`) also makes
`MapId::retail(name, hash)` a retail id, which ranked sessions may use.
The file is absent, so empty, by default: nothing from a disc ships with
the server. It is read on each upload, so edits apply at once. Whatever
the list says, a manifest with no layers (only a retail base) is never
published: it would be the retail map itself.

The Halo Admin's **Custom content** tab (`/games/halo2/admin?tab=content`, linked
from the dashboard's "halo admin" pod) lists packs with expandable member
maps, standalone maps and blobs. It shows their states, report counts, available
actions, reports and upload switch; the playlist builder pins published packages per hopper.

## 8. Player reports

Players report each other from the game's player-details screens
(`XOnlineFeedbackSend`, [live-api.md](live-api.md) "Player reports"), and
any account can report another through the API. Only admins and the
operator read the reports: neither the reporter nor the reported player
sees them, in the log or anywhere else. Admins review them with tokens for
`lxm` `reports.moderate`:

| endpoint | does |
|---|---|
| `GET /v1/admin/reports?state=open\|closed\|all` | `{items, players, position}`: the reports newest first, and a record for every reported player in them |
| `POST /v1/admin/reports/{id}/dismiss` `{note?}` | closes a complaint: nothing to act on |
| `POST /v1/admin/reports/{id}/note` `{note}` | closes a complaint with a note kept on the player's record |

`state=open` (the default) lists complaints nobody has reviewed;
`closed` the reviewed ones; `all` everything, the commendations
(`good_attitude`, `good_session`) included, which are never reviewed. An
item is `{id, target, by, kind, negative, reason?, game_id?, at, review?}`:
`id` is the report's log position, `target` and `by` are `{xuid, gamertag}`,
`game_id` the game the reporter was in when the game said, and `review`
`{action, note?, by?, at}` once an admin acted (`by` absent when the operator
did). A player's record is `{complaints, open, commendations, notes}`, over
every report against them, with the admins' notes oldest first. A note
needs text; either is at most 500 characters. Reviewing a commendation is
400, a report already reviewed 409.

Each report is a server-signed `PlayerReported` event and each review a
`ReportReviewed` event, both on the reported player's `reports-<xuid>`
stream, so the queue replays from the log. Reviewing a report bans no
one; banning is its own action (§9), which can name the report it answers.

Player reports are the service's, not the game's, so they are on the
dashboard's server screens (§5), not the Halo Admin: **player reports** on
the accounts screen opens `/dash/server/reports`, which shows the open
complaints with each player's record, a note field, and Note and Dismiss;
closed and everything list the rest. The queue is one component
(`web/src/dash/Reports.tsx`) over `web/src/admin/reports.ts`, laid out
plainly for now.

## 9. Bans

An admin bans an account for a while or until lifted, often in answer to a
report (§8), with the same `reports.moderate` tokens:

| endpoint | does |
|---|---|
| `POST /v1/admin/bans/{xuid}` `{reason, until?, report?}` | 201 `{ok, position}`: bans the account; a newer ban replaces the one in force |
| `POST /v1/admin/bans/{xuid}/lift` `{note?}` | lifts the ban in force; 409 when there is none |
| `GET /v1/admin/bans?state=active\|all` | `{items, position}`: the bans in force (the default) or every ban, newest first |

`reason` is what the player is told, required, at most 500 characters.
`until` is an RFC 3339 time in the future; without it the ban lasts until
lifted. `report` is the log position of a report against the same player.
Admins can't be banned (403). An item is `{id, target, reason, until?,
report?, by?, at, state, lifted?}`: `state` is `active`, `expired`,
`lifted` or `replaced`, `by` the admin's gamertag (absent for the
operator), and `lifted` `{note?, by?, at}`.

While a ban is in force the server refuses every token the account
presents for a write scope with `banned` (403), saying until when and why:
signing in (`SignedIn`), the presence heartbeat, sessions, games, the co-op
relay, reports, cloud-save writes and the rest all go through the same
check. Reads stay open (`read`, `recovery.read`, `storage.read`,
`log.read`), so the player can read their notice and their saves. An end
date is checked whenever a token is used, so a ban ends by itself with
nothing written.

In the game, the Live HLE turns a `banned` answer into `0x80151200`
(account requires management): at sign-in the game shows its account
dialog (`0x27`, [live-boundary.md](live-boundary.md) §7), and a ban during
a session reaches the connection task on the next heartbeat with the same
code.

Each ban is a server-signed `AccountBanned` event (`{target, reason,
until?, report?, by?}`) and each lift an `AccountUnbanned` event
(`{target, ban, note?, by?}`), on the account's `bans-<xuid>` stream, so
the bans replay from the log. Admins, the operator and the banned player
read them ([live-api.md](live-api.md) "Private events").

On the dashboard, **bans** on the accounts screen opens
`/dash/server/bans` (`web/src/dash/Bans.tsx`): the bans in force with a
note field and Lift on each, and every ban under "every ban". Each open
report in the queue has a Ban button that bans its player for the report,
for a day, a week, 30 days or until lifted, with the reason; an admin
viewing a player's dashboard screen sees their ban with Lift, or Ban. A
banned player sees the notice on their own screen.
