# Live API: game art, clarifications, accounts, ranks and arbitration

Part of [Live service API](live-api.md). Covers: Optional game art; Clarifications; Game and account details; Ranks; Arbitration.

## Optional game art

A server may serve art extracted from its owner's own disc (`h2 live assets
extract`; nothing of it is in this repo). Clients must work without it.

- `GET /art/game/manifest.json`: the manifest, or 404 when the server has
  no game art or will not give it to this client. By default it is served
  only to clients on the server's own machine (a loopback peer with no
  `Forwarded`/`X-Forwarded-For`); an operator's `--share-game-art` serves
  it to everyone. A 404 means "use your own art".
- `GET /art/game/<file>`: a PNG named by the manifest, 404 likewise.
- `GET /art/game/catalog.json`: the playlist builder's catalogue of the
  game's maps, playlist texts and built-in variants
  ([live-admin.md](live-admin.md), "The catalogue"), 404 likewise.
- `GET /art/game/map/<stem>/map.glb` (`model/gltf-binary`),
  `.../views.json` (`application/json`) and `.../views/<id>.png`: a
  map's or campaign level's geometry and pre-drawn views, named by the
  manifest's `maps` ([live-art.md](live-art.md)), 404 likewise. Converted
  Halo 1 maps are there too, keyed by the name games report them by
  (`h1_<map>`, or the level's), their entries marked `"game": "halo1"` with
  a `"name"`.

```json
{ "version": 1,
  "source": "<sha256 of the disc's map files>",
  "images":  { "<key>": { "file": "rank/12.png", "width": 28, "height": 26 } },
  "aliases": { "gametype/slayer": "gametype/0" },
  "emblem":  { ...colouring rules, below... },
  "palette": ["#ffffff", "..."],
  "skipped": { "<key or prefix>": "why it is missing" },
  "maps":    { "<stem>": { "geometry": "map/<stem>/map.glb",
                           "views": "map/<stem>/views.json",
                           "kind": "multiplayer", "bsps": 1,
                           "game": "halo1", "name": "..." } } }
```

`version` is the manifest format; it changes only when a client could no
longer read it (new fields and keys do not change it). `file` is relative
to the manifest. Keys (a client looks a key up in
`aliases` first, then `images`):

| Key | What |
|---|---|
| `emblem/fg/<0..63>` | emblem foreground layers |
| `emblem/bg/<0..31>` | emblem background layers |
| `rank/<1..50>` | rank (level) icons |
| `live/<0..31>` | Live status icons (friend, invite, voice, message...) |
| `gametype/<0..10>` | game-type icons; aliases `gametype/slayer`, `king_of_the_hill`, `oddball`, `juggernaut`, `ctf`, `assault`, `territories` (identified by eye; the others have only an index) |
| `weapon/<0..18>` | weapon silhouettes (small, single-mask: blue channel with matching alpha); aliases `weapon/energy_sword`, `plasma_pistol`, `plasma_rifle`, `sniper_rifle`, `rocket_launcher`, `smg`, `beam_rifle`, `sentinel_beam` (by eye; the others have only an index) |
| `medal/<0..31>` | medals, full colour; aliases `medal/double_kill`, `triple_kill`, `killtacular`, `kill_frenzy`, `killtrocity`, `killimanjaro`, `killing_spree`, `running_riot`, `rampage`, `berserker`, `overkill`, `sniper_kill`, `splatter`, `bone_cracker`, `assassin`, `stick_it`, `flag_carrier_kill`, `bomb_carrier_kill`, `flag_returned`, `bomb_returned` (by eye; the spree order is inferred from the medals' growing number of points) |
| `map/<map>/preview` | a multiplayer map's preview picture; `<map>` is the map file name without `.map` |
| `title/image` | the game's title image, from the executable's `$$XTIMAGE` section (128x128): what the original dashboard showed as the game's tile |
| `title/save-image` | the game's save image, from `$$XSIMAGE` (64x64): the picture of a save that has no `SaveImage.xbx` of its own |

`palette` is the player colour palette, `#rrggbb` by colour index (the
colours are stored at 4 bits per channel); empty, with a `skipped` entry,
when not found.

**Emblems.** An emblem is a background layer with a foreground layer drawn
over it. Each layer image holds two masks, not colours: mask *a* is the red
channel (green equals it, so it looks yellow) and mask *b* the blue channel.
The image's alpha copies mask *b* and is not coverage.

Per pixel, with
channels in 0..1:

```
rgb      = colour_a * red + colour_b * blue
coverage = background: 1;  foreground: max(red, blue)
```

`emblem.colours` in the manifest gives the profile colour (an index into
`palette`) for each mask
(foreground: primary for *a*, secondary for *b*; background: background
colour for *a*, a second background colour for *b*). The masks were
measured on the disc. Which profile colour fills which mask has not been
confirmed against the game.

**Composed emblem images.** `GET /art/emblem/{fg}-{bg}-{primary}-{secondary}-{tertiary}-{quaternary}.png`
takes foreground (0–63) and background (0–31) layer numbers followed by
four palette indices (0–255, wrapped to the palette length). For example,
`/art/emblem/30-7-3-6-8-2.png` displays layers 30 and 7 with colours 3 and 6
on the foreground and 8 and 2 on the background. The server uses the
extraction's palette when it contains at least two valid colours and
otherwise uses the web app's default 18 colours.

The web client uses `s` plus eight lowercase hex digits for a seed-derived
selector when an appearance field is absent. If a seeded colour depends
on an overridden primary or tertiary, a final hyphen-separated field
carries the original primary and tertiary hashes as 16 lowercase hex
digits. A colour can also be an explicit `rRRGGBB` value with lowercase
hex digits.

The route redirects to the current revision's `?v=<revision>` URL, which
returns a PNG at the background mask's pixel size. The foreground scales
to fit it. Successful images with shared game art are served with
`Cache-Control: public, max-age=31536000, immutable`.

Loopback-only art
keeps a private cache policy. Invalid specs return 400. Missing art,
layers, or revisions return 404, and these replies are uncached.

The same
access rules apply as for `/art/game/*`.

## Clarifications

1. **Positions.** The log starts at position 1. `?from=N` is exclusive:
   the stream and `/v1/events` return events with `position > N`, so
   `from=0` is the whole log and a client resumes with the last position it
   applied.
2. **Snapshots join the stream.** Every read response carries
   `"position": <head when it was read>`, so a client can load a snapshot
   and open the stream with `?from=<that position>`. The stream also takes
   `?exclude=GameFrame` (comma-separated types to leave out) and
   `?types=` (only these types).
3. **Lists** are `{ "items": [...], "next": <string or null>, "position": n }`
   everywhere. `next` is opaque; pass it back as `?cursor=`. For
   `/v1/events` it happens to be the next `from`.
4. **Errors** are exactly `{ "error": "<code>", "message": "..." }` with the
   codes in Conventions (plus `unauthorized`, `forbidden`, `gone` (410), `rate_limited`,
   `too_large`, `unimplemented`).
5. **Presence states** are `online`, `away` and `playing` (`game_id` set);
   `SignedOut` and `PresenceExpired` mean offline.
6. **`GameEnded.winner`** is `{ "team": n }` in team games,
   `{ "player": index }` otherwise, or null for a draw or quit.
7. **`GameEnded.players`** is the final scoreboard: `[{index, score, kills,
   deaths, assists}]`.
8. **Frames** are [frame chunks](live-api-events.md#frame-chunks) (`GET
   /v1/games/{id}/chunks`). Games recorded before them are read with `GET
   /v1/games/{id}/frames?from_tick=&to_tick=&cursor=`, packed, at most 600
   per page, oldest first.
9. **`PlayerKilled`** carries `pos` when the victim's position is known.
   `weapon` is the stable key for the killing damage, such as `battle_rifle`,
   `frag_grenade`, `melee` or `fall`. It can also identify the cause of a
   suicide or world death. The credited `killer` may have damaged a player
   who later fell. In that case `weapon` describes the fall. `headshot`
   is a boolean for identified weapons and is absent for environmental or
   unattributed damage. Both fields are
   optional in the event log and the `kills` array returned by
   `GET /v1/games/{id}`. An absent field means unknown.
10. **Who writes a game** (2026-11-18). The server trusts the host's
    record. Only the host ends the game (`GameEnded`; 403 from anyone
    else, who leaves with `PlayerLeft`). A `PlayerJoined` from anyone but
    the host must be the author's own account, not a bot (403). An xuid
    can be in a game once at a time: a `NewGame` naming one twice, or a
    `PlayerJoined` for an xuid already in, is a 400.
10. **`GameStarted`** carries `tick`, the game tick it started at.
11. **Keys.** `GET /v1/accounts/{gamertag}/keys` and
    `GET /v1/accounts/by-xuid/{xuid}/keys` both return
    `{ "keys": [ { "kid", "jwk", "valid_from": <position>,
    "valid_to": <position or null> } ] }`. The hash chain, batch JWS and
    `at` rules are in [live-security.md §2](live-security.md).
12. **Well-known `api`** may be relative (`"/v1"`); resolve it against the
    well-known document's URL.
13. **Axes.** World coordinates are the game's: x and y horizontal, z up,
    in world units. `yaw` is radians counter-clockwise from +x. Map outlines
    use the same axes.
14. **`console`** is a free display string (the console's name); clients
    may show it or not.

## Game and account details

These override the tables above where they differ.

- **`POST /v1/games`** returns `{game_id, position, identifiers}`. Live assigns the game its id (32 hex) and 16 seat and machine slots in `identifiers.seats` and `identifiers.machines`. Retries of the same game receive the same slots. A new game receives new identifiers for the same players. Live writes `GameStarted` itself, signed by the server key, with an extra
  `host` field naming the token's author. Later game events are written by
  the host or a player in the game.
- **`POST /v1/events`** returns the stored wire event (a superset of
  `{position}`).
- **Map outlines** can be uploaded and replaced by admins and the operator.
- **Idle games.** A game with no events or frames for 5 minutes ends as
  `lost`. That `GameEnded` carries the last tick seen, but its `at` (the game's
  `ended_at`) is when the server gave up, so a game's length is
  `(GameEnded.tick - GameStarted.tick) / 30`, not `ended_at - started_at`.
  A `lost` game that never got past its start tick (the console went quiet
  before its first frame) has length 0: its wall clock is only the
  server's wait, and stats count none of it as time played.
- **`recent_games`** on an account is the latest 10.
- **Account creation** payload is `{gamertag, jwk, invite?, aud, iat}`;
  `aud` is the server id from the well-known document and `iat` must be
  within 5 minutes of the server's clock. The fixed test vector is
  `account_request_vector` in `crates/live/live-proto/src/sign.rs`.

## Ranks

Per-playlist ranks ([live-matchmaking.md](live-matchmaking.md) §8): a
level 1..50 and experience for each player in each ranked playlist. The
server writes them, as server-signed `RankChanged` events on
`rank-<xuid>`, when a finished ranked matchmaking game ends; nobody else
may write those streams. A standing is
`{level, experience, highest, games, wins}`.

Reads are public, or need
`read` on a private server.

| endpoint | result |
|---|---|
| `GET /v1/accounts/{who}/ranks` | `{xuid, ranks: {playlist: standing}, position}`; `who` is a gamertag or XUID; 404 if unknown |
| `GET /v1/accounts/{who}/ranks/{playlist}` | `{xuid, playlist, standing, history, position}`: `history` is the last 100 changes, newest first, `{game_id, at, position, before, after}` |
| `GET /v1/ranks/{playlist}?limit=` | `{playlist, items, position}`: `{xuid, gamertag, ...standing}`, with `bot: true` on a bot's row, by level then experience; `limit` 1..1000, default 100 |
| `GET /v1/rank-stats?leaderboards=&xuids=&session=` | `{items, position}`: the game's stats read. `leaderboards` are playlist ids (a descriptor's `+0x08`), `xuids` players, both comma-separated, at most 32 and 64 (400 beyond). One `{xuid, leaderboard, playlist, standing}` per player per leaderboard that is a ranked playlist in the served hopper file; a player with no games there is level 1; other leaderboards have no rows. Optional `session` (16 lowercase hex digits, else 400): a made-up bot of that session's latest roster, on the roster's playlist, answers its roster level |

`playlist` is the hopper descriptor index, as sessions and games name it.
A leaderboard is the playlist id the hopper file gives that descriptor,
which is what the game's `XOnlineStatRead` names.
A game is ranked only for the players who registered its arbitration
round ([Arbitration](#arbitration)); each `RankChanged` names that round
as `round`, `{session}-{round}`, and a round ranks one game.

Only the
session host's own record is ranked (`GameStarted.host` is the session's
`host_xuid`), by the latest round it registered before the game started.
A bot
rostered in the game's session (`POST /v1/games/bots`) needs no
registration: a persistent bot is ranked, its `RankChanged` marked
`bot: true`; a made-up bot is an opponent at its roster level and gets
none. A bot's rank pages answer by its xuid or gamertag, with `bot: true`.

## Arbitration

The game's `XOnlineArbitration` calls, posted by the Live HLE once for
each account signed in on the console
([live-matchmaking.md](live-matchmaking.md) §8.4). Each takes a
`games.write` token of the account. `session` and `round` are 16
lowercase hex digits. The server writes `ArbitrationRegistered`,
`ArbitrationExtended` or `ArbitrationReported`, server-signed, on
`round-<session>-<round>`, with `{session_id, round_id, xuid}` and the
call's fields; `xuid` is the token's account.

The session must be one the
server knows (404 otherwise). When a game is ranked by a round, the
server also writes `ArbitrationUsed` `{session_id, round_id, game_id}`
there, whether or not anyone's standing moved; the round ranks no other
game. Nobody writes these through `/v1/events`.

| endpoint | body | result |
|---|---|---|
| `POST /v1/arbitration/{session}/{round}/register` | `{duration, flags}` | 201 with `{position}`; 200, writing nothing, when the account already registered |
| `POST /v1/arbitration/{session}/{round}/extend` | `{duration}` | 201; 409 unless the account registered the round |
| `POST /v1/arbitration/{session}/{round}/report` | `{flags, procedures}`: the stat procedure ids, at most 1000 | 201; 200 when the account already reported; 409 unless it registered |

An id that is not 16 lowercase hex digits, or another call name, is a
404.


### Finished replay archives

`GET /v1/games/{id}/chunks?format=archive` returns the ordinary frame chunk
index until a verified archive is published. Its published response includes
`archive_sha256` and an ordered `items` array. Each entry identifies the
segment’s first and last source ticks, frame count, compressed byte count,
SHA-256, zero-based offsets of source keys with a complete machine travel
baseline, and an optional `machine_entry` seed of known device travel at
the segment start.

A seek applies the seed before the segment's first
frame. Ticks beyond the JSON safe-integer range use decimal strings.
Items keep recording order, including repeated or rewound source ticks.

`GET /v1/games/{id}/archive/{archive_sha256}/{ordinal}` serves the stored
segment with `Content-Type: application/x-h2-replay` and
`Content-Encoding: br`. The browser receives an independently decodable
H2AR body. Fetch the segment containing the playhead first.

Both routes
apply the game’s normal visibility check. Anonymous segment reads of public
games are immutable. Authenticated responses are private.

The game
stream’s `ReplayArchivePublished` event signals that playback
can switch to the archive. Live viewing uses the packed stream described
above.

The original chunk index remains available without the format parameter.
Historical game inventory, dry run, verification, backup and rollback use
the separate admin API. [Replay migration](replay-migration.md)

#### Admin archive operations

| Method and path | Token scope | Result |
|---|---|---|
| `GET /v1/admin/replay-archives?limit=32&cursor=…` | `admin.read` | Inventory page with `items`, `next` and log `position`. The limit is 1–100. |
| `GET /v1/admin/replay-archives/{id}` | `admin.read` | Current state, sealed source position, original chunk and legacy frame counts, cumulative totals and both committed hashes. |
| `POST /v1/admin/replay-archives/{id}/step` | `admin.write` | Process one segment of a sealed game. |
| `POST /v1/admin/replay-archives/{id}/rollback` | `admin.write` | Check both supplied hashes and return the updated inventory record with HTTP 200 and state `rolled_back`. |

A step body contains `mode` (`dry_run`, `apply` or `verify`), the inventory’s
`source_position`, and the preceding receipt’s opaque `next` value as
`cursor`. Omit the cursor on the first request. Dry runs write no objects
or events.

Apply can resume durable progress. Verify checks each published
segment, immutable index node and original backup against the retained
source. The final receipt includes `done: true`, the archive and backup
SHA-256 values, and cumulative totals for `frames`, `segments`,
`source_bytes`, `archive_bytes` and `index_bytes`.

Rollback takes both
`archive_sha256` and `backup_sha256` from that receipt.

An incorrect revision or pointer returns 409. A broken recording fails the
affected game, while a storage-service failure returns 503. A retry with
the same cursor returns the same cumulative counts.
