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
  `h2 play --live` starts read `H2_LIVE_ADMINS`: a comma-separated list of
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
- The embedded server of `h2 play --live` also makes the account it is
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
same body (`crates/live-server/src/title_headers.rs`); reads through the
admin API accept them meanwhile.

### The default MOTD and keeping what you publish

`h2 play --live` without Postgres keeps its log in the live data directory
(`server/log`: `events.jsonl`, `blobs/`, `checkpoints.json`;
`crates/live-server/src/filestore.rs`), so accounts, friends, games and
published title files survive a restart. Before this, that case used the
in-memory store and lost every publish on exit, and since the game reads
the MOTD only at sign-in, a MOTD published mid-session could never be seen.
Only if that directory is held by another running server does it fall
back to memory (with a warning).

When no MOTD has ever been published, the embedded server (on Postgres or
the file store) publishes a default one at start
(`crates/live-server/src/default_motd.rs`): a short header and 21 original
"Did you know?" lines and gameplay tips. It never replaces a published one;
edit it like any other.

### `GET /v1/titles/{title}/hopper`

The playlists as JSON — 16 hoppers, each with its records (the entries
within the hopper, the first `record_count` of them):

```json
{ "hoppers": [ {
    "index": 0, "type": 4, "status": 1, "record_count": 2,
    "name": "", "title": "Big Team Battle",
    "description": "Join alone or as a party…", "ranges": [1, 8, 6],
    "dwords": [ …18 numbers… ],
    "records": [ { "id": 1, "string": "", "flags": 1, "dwords": [1, 0],
                   "variant": { "name": "Team Slayer", "engine": 2, "team": true } } ]
  }, … ], "position": 1042 }
```

404 until a base image is imported. What the fields are
([live-motd.md §5.2](live-motd.md), corrected in §5.2.1 there):

- `title` / `description`: what the playlist list shows, the English one
  of nine per-language UTF-8 slots (`+0x0c`, 16 bytes each; `+0x9c`, 128
  bytes each). A write puts the text in all nine. At most 15 and 127 bytes.
- `type` 1..5, `status`, the three `ranges` (for the free-for-all default
  the first two are the minimum and maximum players, 4 and 8; for the clan
  defaults they are 2, 3, 4 and ?, 6, 8 — the third's meaning, and the
  type's, are unconfirmed), the descriptor's dword array.
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

Every other byte of the file is unknown to us and preserved.

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

### Importing the base image

```
h2 live hopper import <path> [--title 4D530064] [--server URL] [--data DIR]
```

`path` is the owner's own `n:\multiplayer_hopper.dat` (the raw image the
game caches) or a download envelope; the command shows the 16 playlists it
found and publishes the file. Game data never goes in the repo — only to
the owner's server. Until one is imported, the hopper endpoints 404.

## 3. The web admin area

`/stats/admin`, linked from the stats site's nav only when the signed-in
browser account is an admin (the `admin` field above). Three sections:

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
- **Import account** — below.

The same playlist editing is available offline in the native editor's
hopper tool (`h2-editor`, [editor.md](editor.md)): it opens and saves a
`multiplayer_hopper.dat` file (raw image or envelope) with the same
fields, validation and presets; publish the result with `h2 live hopper
import`.

Saves are the browser's first writes: each signs a self-issued 60-second
token (`titles.write`) with the signed-in account's key, exactly as the CLI
signs its requests, and sends it as a bearer header.

### The catalog

`h2 live assets extract` (and `h2 play --live`, which runs it) writes
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
`crates/cli/src/live_catalog.rs`):

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
  `+8` scenario path; [level-boot.md](level-boot.md)), each named by the
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
  (name, game type, engine index; docs/community-gametypes.md).

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
`from`/`from_hopper`. Whatever does not resolve (a map the game does not
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
are two bridges: a link `h2 play --live` prints, for the browser on the
same machine, and an export, for anything else.

### The `h2 play --live` link

When `h2 play --live` runs the embedded server (not with `--live-server`)
and has the web app, it prints, next to the web app's address:

```
live: web app at http://127.0.0.1:50123/
live: sign this browser in as afr (one use, this machine only): http://127.0.0.1:50123/signin#once=…
```

Opening it signs the browser in as the account the game plays as, which
the embedded server has made an admin (§1), and lands on `/stats/admin`
(or the dashboard if that account is not an admin). How it works:

- The CLI builds the same browser entry `account export` writes and hands
  it to the running server, in memory, under a token of 256 random bits
  (`Live::offer_local_signin`, `crates/live-server/src/local_signin.rs`).
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
run `h2 play --live` again. Once redeemed, the private key is in this
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
`/stats/admin` → **Import account**, paste the JSON or choose the file:
the page checks the key against the server again and keeps it in this
browser's IndexedDB (the same warning as sign-up applies). The file is as
sensitive as `accounts.json`. There is no remote link or QR for this: a
link that works from another machine would have to carry the key itself
(the server never holds it) through whatever channel moved the link.

## Wiring

The admin module is self-contained (`crates/live-server/src/admin.rs`,
`crates/cli/src/live_admin.rs`, `live_proto::hopper`); the pieces other
files own are small and listed in the lane report: module declarations,
route registrations, the `Policy.admins`/`Live.admins` fields, the
`admin` field on account reads, the CLI dispatch and the `--admin` flag,
and the embedded server's runtime grant.

## 5. Security notes

- Grant and revoke are operator-only (a server-key token with `lxm`
  `admin.write`), like invites: an admin can publish content, not make
  admins.
- Admin publishes go through the same event log as operator ones
  (`TitleFilePublished` by the server key), so the audit trail does not
  say who clicked; the operator's own log of grants and the account reads
  do. (Per-author attribution of admin publishes is a possible follow-up:
  an `AdminPublished` event naming the admin.)
- `H2_LIVE_ADMINS` and `--admin` gamertags are compared case-insensitively;
  gamertags are unique per server, so they name exactly one account.
- The exported account file is a private key on disk, mode 0600; treat it
  like `accounts.json`.
- The `h2 play --live` sign-in link (§4) is a ten-minute, one-use bearer
  secret for that account, honoured only from this machine; treat the
  terminal it is printed in accordingly.
