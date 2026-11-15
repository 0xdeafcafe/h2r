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
one is published:

```json
{ "header": "Halo 2 is back", "messages": ["Welcome, Spartan.", "…"] }
```

### `PUT /v1/titles/{title}/motd`

The same shape, admin-only. The server writes the same text into all nine
language slots (as `motd set` does) and answers
`{"published": {name, sha256, size}, "motd": …}`. Limits as the format
requires: 1..=32 messages, at most 255 UTF-16 units each.

### `GET /v1/titles/{title}/hopper`

The playlists as JSON — 16 hoppers, each with its records (the entries
within the hopper, the first `record_count` of them):

```json
{ "hoppers": [ {
    "index": 0, "type": 4, "status": 1, "record_count": 2,
    "name": "Big Team Battle", "ranges": [1, 8, 6],
    "dwords": [ …18 numbers… ],
    "records": [ { "id": 1, "string": "one", "flags": 0, "dwords": [0, 0] } ]
  }, … ], "position": 1042 }
```

404 until a base image is imported. The fields are the ones
[live-motd.md §5.2](live-motd.md) names: `type` 1..5, `status` 0..2,
`name` ASCII, the three `ranges` (the party-size limits the session setup
reads), the descriptor's dword array; per record the id (which must resolve
to a `matchmaking_hopper_descriptions` tag record or the game marks the
slot as an error), the string, flags and two dwords. Every other byte of
the file is unknown to us and preserved.

### `PUT /v1/titles/{title}/hopper`

Admin-only. The body lists only the hoppers that changed; within one, only
the fields given change:

```json
{ "hoppers": [ { "index": 0, "name": "Big Team", "record_count": 3,
    "records": [
      { "id": 2, "string": "two first", "flags": 0, "dwords": [0, 0], "from": 1 },
      { "id": 1, "string": "one",     "flags": 0, "dwords": [0, 0] },
      { "id": 3, "string": "a copy",  "flags": 0, "dwords": [0, 0], "from": 1 }
    ] } ] }
```

`records`, when present, replaces the whole list (1..100 entries;
`record_count` must then match or be left out). Each entry keeps its
unknown bytes — the template blobs the game fills in — from the slot named
by `from`: the entry's position in the hopper as it stands now. Absent
means "this position", so an edit in place needs nothing; a reorder names
where the entry came from, a copy names its source, a removal is simply a
shorter list. Nothing the builder cannot edit is ever lost, and the
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
  form (255 UTF-16 units, 32 messages).
- **Playlists** — the 16 hoppers: pick one, edit its name, type, status and
  party-size ranges, and reorder, duplicate, add or remove its entries with
  the same validation the server applies (ASCII names ≤ 132, strings ≤ 71,
  1..100 entries, ids and dwords whole 32-bit numbers). Entries remember
  where their bytes come from, so reorders and copies carry the template
  blobs whole.
- **Import account** — below.

Saves are the browser's first writes: each signs a self-issued 60-second
token (`titles.write`) with the signed-in account's key, exactly as the CLI
signs its requests, and sends it as a bearer header.

## 4. Signing the browser in as an existing CLI account

The browser can only sign *up* (a new key, a new account); the owner's
admin account was made by the CLI, whose key the browser never saw. The
smallest safe bridge is an export:

```
h2 live account export <gamertag> --file afr.json
h2 live account export <gamertag>          # prints it (and a warning)
```

The CLI checks with the server that the key it holds is still the
account's current key — the same check `account import` makes in the other
direction — and writes the browser's account entry (`{xuid, gamertag, id,
server, server_id, kid, key, x}`, the key the base64url seed) to a 0600
file, since it is a private key. On the server's machine with the web app
open, go to `/stats/admin` → **Import account**, paste the JSON or choose
the file: the page checks the key against the server again and keeps it in
this browser's IndexedDB (the same warning as sign-up applies: anyone with
the browser profile can read it). One exported account signs in one
browser; the file is as sensitive as `accounts.json`, which is why there is
no link and no QR — nothing that puts the key anywhere but the terminals
and files you choose.

(A one-time login link would have to carry the private key to be worth
anything — the server never holds it — so it would be this same secret in
a URL, remembered by the browser. The file/paste is the same ceremony with
fewer leftovers.)

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
