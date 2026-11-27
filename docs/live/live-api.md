# Live service API

Status: contract, implemented apart from what the sections mark as not
built (federation among them). The service is `crates/live/live-server`
([live.md](live.md)). Everything here is JSON over HTTP on one port
(default `127.0.0.1:8074`), plus WebSockets for streaming, ingest, the
co-op relay and the dedicated host tunnel. Identity, signing, tokens
and discovery are in [live-security.md](live-security.md); this page refers
to it as §n. Every client is given a base URL and finds the rest from
`GET /.well-known/h2live.json`.

Three kinds of client use it:

- the game (the recomp's Live HLE and its telemetry tap),
- the web app (`web/`), served by the same server at `/`,
- agents and scripts, through `h2 live q` or plain HTTP.

## Conventions

- XUIDs are 16-digit hex strings (`"0009a1b2c3d4e5f6"`), title IDs 8-digit
  hex (`"4D530064"`), times RFC 3339 UTC, positions are the log's global
  `position` (an integer, strictly increasing).
- Errors: non-2xx with `{ "error": "<code>", "message": "..." }`. Codes:
  `invalid`, `not_found`, `conflict`, `unavailable`.
- Lists take `?limit=` (default 100, max 1000) and return
  `{ "items": [...], "next": <cursor or null> }`.

## Events

Moved to [Live API: events and read endpoints](live-api-events.md#events).

### Frame chunks

## Read endpoints

Moved to [Live API: events and read endpoints](live-api-events.md#read-endpoints).

### Caching

## Theater

Moved to [Live API: theater, stats, medals and clanker training](live-api-theater-stats.md#theater).

### Switching a host to film ingestion

## Stats summary

Moved to [Live API: theater, stats, medals and clanker training](live-api-theater-stats.md#stats-summary).

## Medals

Moved to [Live API: theater, stats, medals and clanker training](live-api-theater-stats.md#medals).

## Clanker Training

Moved to [Live API: theater, stats, medals and clanker training](live-api-theater-stats.md#clanker-training).

## Usage

The Live admin's usage screens (`/dash/admin/usage`,
[web-boundary.md](web-boundary.md) §5) read two endpoints, both for
admins only: an `admin.read` token from an admin account, or the
operator's (401 without one, 403 for anyone else). The numbers live in
`crates/live/live-server/src/usage.rs`. The reads use in-memory projections
and a cached inventory of the private blob store.

**Where the counts come from.**

- *From the log*, folded as events are applied and replayed at startup
  like the read model: `events` (every event, by its author; the
  server's own under no account), `up.events` (the bytes of each event an
  account signed, its JWS as received; a batch once), `messages.*`
  (`friend_requests`: `FriendRequested`; `invites`: `GameInviteSent`, for
  its sender), `games` (once per game, for the host and the players it
  began with or who joined), `sessions_hosted` (`SessionCreated`),
  `sign_ins` (`SignedIn`), `reports.sent` and `reports.received`
  (`PlayerReported`, for its `by` and its `target`), `stored.events` and
  `stored.content` (`ContentStored`, for its uploader). Other events the
  server signs for itself (`GameFrame`, `RankChanged`, `ReportReviewed`,
  `PresenceExpired`, ...) count as events by type and nothing more.
- *Metered* at the request: `requests` for HTTP responses from `/v1/*`,
  including errors, preflight requests and WebSocket upgrades. Rate-limited
  responses (429) are excluded. This is a service-wide count. On the Worker,
  it includes internal API calls to `LiveHub`. Cloudflare cache hits,
  static assets, health checks and WebSocket messages are outside this count.
  Upload meters count `up.frames` (authenticated text
  and packed frame messages received on `/v1/ingest`), `up.content`
  (content uploads, title files, map outlines) and `up.saves` (committed
  cloud save bodies). The native server batches metered increments for up
  to a minute. The Worker flushes to SQLite every five seconds while
  counters are pending, or sooner when a batch reaches 256 increments.
  The sweep alarm flushes batches left by a burst of traffic. A deploy
  or abrupt reset can discard a batch before it is written.
  `h2 live store copy` copies the log and blobs. The metered history starts
  anew on the destination.
- *Inventoried* for `stored.saves`: the actual account save blobs in the
  private store, counted once per object. `crates/live/live-server/src/cloud_saves.rs`
  caches this inventory for 30 seconds across both usage endpoints. When
  `saves_storage_measured` is `false`, the stored totals cover events and
  catalogued content only.

Days are UTC, from an event's signed time (`at`) or the server's clock
when metered, and are kept for 400 days; the write on a new day drops
older ones. Stored totals have no day and are never dropped. An account
is *active* on a day when it produces a counted event or upload, sends a
message or report, plays a game, hosts a session or signs in.

Reports made
about an account and bytes stored do not count as activity. The co-op
relay's messages and bytes are outside the usage meters.

### `GET /v1/admin/usage?days=30`

`days` is 1 to 400 (default 30), the range ending today.

```json
{ "from": "2026-10-20", "to": "2026-11-18", "keep_days": 400,
  "request_count_from": "2026-11-18",
  "days": [ { "day": "2026-11-18", "requests": 12,
              "up": { "events": 1145, "frames": 133, "content": 0, "saves": 0 },
              "events": 5, "messages": { "friend_requests": 1, "invites": 0 },
              "games": 1, "sessions_hosted": 0, "sign_ins": 1, "reports": 0,
              "active_accounts": 1 } ],
  "totals": { "day": "", "requests": 12, "up": { ... }, "events": 5, ...,
              "active_accounts": 1 },
  "accounts_active": 1,
  "event_types": [ { "type": "GameStarted", "count": 1 } ],
  "stored": { "events": 2290, "content": 0, "saves": 0 },
  "saves_storage_measured": true }
```

`days` lists only days with recorded activity, oldest first. An unmeasured
day has `requests: null`. `request_count_from` is the earliest retained UTC
day with a recorded request, or `null` if none exists. `totals.requests`
sums recorded requests in the selected range and is `null` if the range
has no request counts. `totals.active_accounts` sums days and
`accounts_active` counts distinct accounts in the range. `event_types`
lists event counts, largest first. `stored` covers the event log, the
content catalogue and cloud save blobs over every account.

### `GET /v1/admin/usage/accounts?days=30&sort=uploaded&limit=50`

Every account with anything in the range or anything stored, sorted
largest first by `sort` (`uploaded`, `stored`, `events`, `messages`,
`games`, `sessions_hosted`, `sign_ins`, `reports_sent`,
`reports_received`, `active_days`; ties by xuid), at most `limit` (1 to
500, default 50). `total` counts them all.

```json
{ "days": 30, "sort": "uploaded", "total": 1, "saves_storage_measured": true,
  "items": [ { "xuid": "00097a94c4494c72", "gamertag": "Alpha",
               "up": { "events": 1145, "frames": 133, "content": 0, "saves": 0 }, "uploaded": 1278,
               "stored": { "events": 1145, "content": 0, "saves": 0 }, "stored_total": 1145,
               "events": 2, "messages": { "friend_requests": 1, "invites": 0 }, "messages_total": 1,
               "games": 1, "sessions_hosted": 0, "sign_ins": 1,
               "reports_sent": 0, "reports_received": 0,
               "active_days": 1, "last_active": "2026-11-18" } ] }
```

## Write endpoints

Every write needs `Authorization: Bearer <self-issued JWT>` (security §3)
except account creation. Signed bodies are sent raw as a compact JWS with
`Content-Type: application/jose`. Event bodies are signed JWS (security §2):
`POST /v1/games/{id}/events` takes one JWS over the array.

| method and path | body | does |
|---|---|---|
| `POST /v1/accounts` | JWS signed by the new key over `{gamertag, jwk, invite?}` | create; 201 `{xuid, gamertag, id, recovery}` (`recovery`: the TOTP secret, `otpauth` URI and current code, live-security.md §1 Recovery), 409 if taken |
| `POST /v1/accounts/{gamertag}/recover` | `{"request": <JWS h2live-recover by the new key over {gamertag, jwk, aud, iat, nonce}>, "code": "123456"}` | sign in on a new device: add the new key with a current sign-in code; 201 `{xuid, gamertag, id}`; 403 wrong code, 429 locked out (live-security.md §1) |
| `GET`/`POST /v1/accounts/by-xuid/{xuid}/recovery` | the account's own `recovery.read` / `recovery.write` token | the sign-in code's secret and current code; `POST` makes a new one (set up or reset) |
| `GET /v1/accounts/by-xuid/{xuid}/signins` | the account's own `signins.read` token | where the account is signed in ([Sign-ins](live-api-sessions.md#sign-ins)) |
| `DELETE /v1/accounts/by-xuid/{xuid}/signins/{kid}` | the account's own `signins.write` token | sign one other device out ([Sign-ins](live-api-sessions.md#sign-ins)) |
| `POST /v1/accounts/by-xuid/{xuid}/signins/sign-out-others` | the account's own `signins.write` token | sign out every device but the caller ([Sign-ins](live-api-sessions.md#sign-ins)) |
| `PUT /v1/titles/{title}/files/{name}` | raw bytes, operator token | publish a title file (max 2 MiB) |
| `POST /v1/events` | one compact JWS (an event on a stream the author may write) | append; 201 `{position}` |
| `POST /v1/accounts/{gamertag}/invite-grant` | `{"open_cap": N}` with an admin or operator's `invite-grants.write` token | enable or update invite access. 201 `InviteGrantSet` wire event |
| `DELETE /v1/accounts/{gamertag}/invite-grant` | no body, same token scope | remove the explicit grant and close the account's open invites unless it is an admin. 200 `InviteGrantRevoked` wire event or 409 if there is no explicit grant |
| `POST /v1/invites` | `invites.write` token. Signed `InviteCreated {jti, exp}` for an account, bare or in `{"event": <jws>, "code": "..."}`. No body for operator form (`?days=&code=`) | create a single-use invite. 201 `{invite, custom, jti, exp, position}` |
| `POST /v1/invites/{jti}/revoke` | `invites.write` token and signed `InviteRevoked {jti}` | end an open invite. Admins can end any invite and granted accounts can end their own |
| `POST /v1/games` | `GameStarted` data without `game_id` | starts a game and returns `{game_id, position, identifiers}` |
| `POST /v1/games/{id}/events` | `[{type, data}]` | append game events in order |
| `PUT /v1/maps/{map}/outline` | outline JSON, admin or operator's `games.write` token | store or replace a map outline |
| `POST /v1/local/signin` | `{"once": token}`, no token | embedded server and loopback peers only: answers once with the browser account entry `h2 play --live-local` offered; otherwise 404 (live-admin.md §4) |

## Server invites

Moved to [Live API: sessions, invites, sign-ins and hosts](live-api-sessions.md#server-invites).

## Sign-ins

Moved to [Live API: sessions, invites, sign-ins and hosts](live-api-sessions.md#sign-ins).

## Friends

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#friends).

## Teams

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#teams).

## Sessions

Moved to [Live API: sessions, invites, sign-ins and hosts](live-api-sessions.md#sessions).

### Browser filters

### Network addresses

### Private events

### Co-op games

### Map Pack matchmaking

## Dedicated hosts

Moved to [Live API: sessions, invites, sign-ins and hosts](live-api-sessions.md#dedicated-hosts).

## Custom maps

Moved to [Live API: custom maps, rules scripts and variants](live-api-content.md#custom-maps).

### The mod repository

#### Verified screenshot uploads

## Packages

See [Live API: content](live-api-content.md#packages).

## Player reports

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#player-reports).

## Bans

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#bans).

## Mute list

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#mute-list).

## Rules scripts

Moved to [Live API: rules scripts and game variants](live-api-rules.md#rules-scripts).

## Game variants

Moved to [Live API: rules scripts and game variants](live-api-rules.md#game-variants).

## Cloud saves

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#cloud-saves).

## Co-op relay

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#co-op-relay).

## Dedicated hosts

Moved to [Live API: sessions, invites, sign-ins and hosts](live-api-sessions.md#dedicated-hosts).

## Dedicated host tunnel

Moved to [Live API: sessions, invites, sign-ins and hosts](live-api-sessions.md#dedicated-host-tunnel).

## Game-invite mailboxes

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#game-invite-mailboxes).

### Text messages

## Streams (WebSocket)

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#streams-websocket).

## Presence heartbeats

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#presence-heartbeats).

## Map outline

Moved to [Live API: friends, teams, moderation, saves and relay](live-api-social.md#map-outline).

## Optional game art

Moved to [Live API: game art, clarifications, accounts, ranks and arbitration](live-api-details.md#optional-game-art).

## Clarifications

Moved to [Live API: game art, clarifications, accounts, ranks and arbitration](live-api-details.md#clarifications).

## Game and account details

Moved to [Live API: game art, clarifications, accounts, ranks and arbitration](live-api-details.md#game-and-account-details).

## Ranks

Moved to [Live API: game art, clarifications, accounts, ranks and arbitration](live-api-details.md#ranks).

## Arbitration

Moved to [Live API: game art, clarifications, accounts, ranks and arbitration](live-api-details.md#arbitration).

### Finished replay archives

#### Admin archive operations

