# Live service: admins, the MOTD editor and the playlist builder

Who may publish title files - the message of the day and
`multiplayer_hopper.dat`, the playlists - and the web admin area that edits
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
  flags. There are no built-in admin names - no gamertag is an admin unless
  someone configured or granted it. An invalid gamertag in either place is
  a startup error, so a typo cannot silently widen who is an admin. The
  server logs the configured admins (gamertags only) when it starts:

  ```
  h2 live: admins configured: Bob, afr
  ```

  An existing account gets the configured role at startup. Registering a
  reserved name requires a server-key bearer bound to the signed signup
  request and any invite required by the server's policy.
  Use `h2 live account create TAG --operator-data DIR` for this registration.
  Configuration cannot be revoked at runtime.
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

Moved to [Live admin: the admin API](live-admin-api.md#2-the-admin-api).

### `GET /v1/titles/{title}/motd`

### `PUT /v1/titles/{title}/motd`

### When the game shows it

### The default MOTD and keeping what you publish

### `GET /v1/titles/{title}/hopper`

### `PUT /v1/titles/{title}/hopper`

### Importing the base image

## 3. The web admin area

The admin tools are split between the app's two sides. The stats site's
**Halo Admin** (`/games/halo2/admin`, "Halo Admin" in its nav, shown only when the
signed-in browser account is an admin: the `admin` field above) keeps only
the game's own things: what it shows and plays. Running the service
(invites, accounts and admins, importing an account, the server's status)
is the dashboard's server screens (§5), styled like the rest of the
dashboard. Each side links to the other: the dashboard's "halo admin" pod
opens `/games/halo2/admin`, and the Halo Admin title bar's "Server admin" goes back
to `/dash/server`. The Halo Admin sections are:

- **Message of the Day** - the header and the message blocks, with a
  preview in the stats site's style and the game's limits enforced in the
  form (255 UTF-16 units, 32 messages). After a publish it notes that the
  game picks the new text up at its next sign-in.
- **Playlists** - the list shows active playlists and how many of the 16
  slots are in use. Add a playlist from a matchmaking template, or open a
  playlist to see its kind and player counts. Its toolbar duplicates or
  removes it, or resets its title, description, rotation and any ranges
  supplied by a historical preset. **Change rules** applies a matchmaking template while keeping
  the title, description and rotation. Clanker marks belong to the original
  playlist ID and slot. A duplicate starts from a human matchmaking template.
- **Custom content** - §7.
- **Clanker Training** - upload a batch from the h2recomp checkout using
  the command shown on the empty page. With runs on the server, the map,
  tuning and batch filters use values from the uploaded runs across all
  pages. Open a run for its results, speed, bot stats and live or replay
  viewer, or select two runs to compare them. Filtered to one batch, the
  tab shows a grid of map tiles above the results, one per run, playing
  finished runs fast-forwarded in sync and running ones live. See
  [Watching in Uplink](../runtime/sim.md#watching-in-uplink).

The playlist editor (`web/src/admin/HopperBuilder.tsx`) groups maps by
game variant. Choose maps from a dialog that shows previews from game art
when available. Use the weight stepper to set how often each entry plays.
Its bar shows the entry's percentage of the playlist's total clamped weight.
Drag a map's handle within its variant, or focus it and press Up or Down.
**Advanced: raw fields** opens the editor for descriptor fields and the
underlying entry list, including reordering across variants. Entries remember
their published source slots so reorders and copies preserve the game variant.

Titles are limited to 15 UTF-8 bytes, descriptions to 127, and playlists
to 1–100 entries. **Review & publish** shows added, removed and edited
playlists and their changed fields against the loaded published file.
**Publish changes** checks that the published fields still match the draft's
base before writing `multiplayer_hopper.dat`. The game downloads the file
at sign-in.

The native editor's hopper tool (`h2-editor`, [editor.md](../editor/editor.md))
opens and saves the raw image or download envelope with the same fields,
validation and presets. In the web editor, pick from the built-in variants or
scripted variants already in the published file. The native tool also accepts
saved variants and `.h2variant` files that run a rules script. Publish an
offline edit with `h2 live hopper import`.

Saves are the browser's first writes: each signs a self-issued 60-second
token (`titles.write`) with the signed-in account's key, exactly as the CLI
signs its requests, and sends it as a bearer header.

### The catalogue

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
catalogue playlist whose text to use), optional range values, and its games
as descriptive references: game type, a built-in variant name to prefer,
team or not, map display names and a weight. They are written from
public knowledge; rotations and weights are approximate, and each preset
says what is uncertain.

A preset resolves in the browser against the catalogue and the published
file. A map resolves by display name to its id. A game variant cannot be
authored (its 0x130 bytes are settings the builder does not model), so
each entry copies a whole published entry that plays the same game type
- preferring the named variant, then the same team play - with
`from`/`from_hopper` (the builder's per-entry variant list also offers
the server's built-in variants, which a preset does not use yet). Whatever does not resolve (a map the game does not
have, a game type no published entry plays, no catalogue at all) is listed
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
account's current key - the same check `account import` makes in the other
direction - and writes the browser's account entry (`{xuid, gamertag, id,
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
through a tunnel. Native servers report `started_at` and `uptime_s` for the
running service. Workers omit both fields because the Durable Object can
cold-start while the service remains available. The plate shows uptime when
reported, along with how the server is reached, sign-up, reads, players,
game art and the log's length. Its pods: **invites**,
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

**Invite access.** Every non-admin starts with invitation permissions
off. On the dashboard, an admin opens a player's page or selects them in
the players list to change **can invite** and the **open invite cap**.
A blank cap allows any number of open invites. Changing the cap leaves
existing invites in place. Switching the grant off permanently closes a
non-admin's open invites. An admin can invite through their role regardless
of the explicit grant or its cap. Revoking the admin role permanently closes
the account's open invites, even if it still holds a grant for future invites.

The operator uses `h2 live invite grant <gamertag> [--open-cap N]` and
`h2 live invite ungrant <gamertag>` with the server key in `--data`.
Both the dashboard and CLI use the grant endpoints in
[Server invites](live-api-sessions.md#server-invites). The server signs
`InviteGrantSet` or `InviteGrantRevoked` on the target account stream
and records the acting admin or operator in `by`. The permission is
replayed from the log at startup.

**Invites** (`crates/live/live-server/src/invites.rs`). Admins open
`/dash/server/invites` to manage the server's invites. A player with the
grant opens **invite someone** on their dashboard profile
(`/dash/server/my-invites`). Their list shows only their own open invites.
Pick 1, 7 or 30 days, make a code and copy it to share. Each code works
once. Admins can revoke any open code and see used, revoked and expired
ones. Players with the grant can revoke their own open codes.

Account-created invites use a signed `InviteCreated` event and the same
`invites.write` token scope. A grant allows a burst of three invites for
a non-admin account, with one slot replenished per minute. Admins have a
separate burst of ten with the same refill rate.
[Request tokens](live-security.md#3-request-tokens) covers authorisation
and [Server invites](live-api-sessions.md#server-invites) describes the endpoints.

The browser mock (`bun run dev`) serves the dashboard endpoints
(`web/mock/admin.ts`). `H2_MOCK_ADMIN=1` makes accounts signed up in the
browser admins.

### Invite codes people type

An invite code has 25 characters in five groups
(`XXXXX-XXXXX-XXXXX-XXXXX-XXXXX`) from `BCDFGHJKMPQRTVWXY2346789`. The
server derives it from the invite's `jti` with a server-only key. The list
can show the same code while it is open.

An inviter can choose a custom code of 4 to 32 printable characters.
`POST /v1/invites` takes `{"event": <InviteCreated JWS>, "code": "..."}`.
The server logs only the custom code's MAC as a server-signed
`InviteCodeSet {jti, mac}` in the same append. The list labels these as
custom codes. Codes match ignoring case, spaces and dashes. A custom
code held by an open invite is refused (409). Older long signed codes
also redeem.

`h2 live invite create [--days N] [--code TEXT]` signs as the operator
(`--data`) or, with `--as GAMERTAG`, as an account with permission to
invite. `h2 live invite list` is for admins and the operator.
`h2 live invite revoke <jti> --as GAMERTAG` acts with the named account's
permission to revoke that invite.

## 6. Security notes

- Only the operator grants or revokes the admin role using a server-key
  token with `lxm` `admin.write`.
- Invites from accounts are logged as their own signed `InviteCreated`
  events and are rate-limited. A ban blocks redemption of a grantee's
  codes while it is in force (§5 and [live-security.md](live-security.md) §3).
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

Moved to [Live admin: moderation](live-admin-moderation.md#7-custom-content-moderation).

### Screenshot review

## 8. Player reports

Moved to [Live admin: moderation](live-admin-moderation.md#8-player-reports).

## 9. Bans

Moved to [Live admin: moderation](live-admin-moderation.md#9-bans).
