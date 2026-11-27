# Live API: theater, stats, medals and clanker training

Part of [Live service API](live-api.md). Covers: Theater; Stats summary; Medals; Clanker Training.

## Theater

Web Theater reads presentation and event records from the host film. Older
recordings use the [spectator archive](replay-fidelity.md). In-game Theater
downloads the private host recording for exact playback.

Pins and clips
reference the canonical game ID and the original tick range.

| Method and path | Token scope | Result |
|---|---|---|
| `GET /v1/films?player=<xuid>&kind=library` | owner, `read` | Participant host games and private uploads in the account library, with pin state. |
| `GET /v1/films?player=<xuid>&kind=shared` | public or `read` | The player's public uploads. Owners can also see their private and link-visible shares. |
| `POST /v1/films` | `films.write` | Reserve two objects with `{film,stats,visibility}`. Each object specifies byte size, SHA-256 and ordered part hashes. Identical reservations return the same film ID. |
| `PUT /v1/films/{id}?object=film\|stats&part=<n>` | owner, `films.write` | Store numbered parts, starting at zero. Repeating a part is safe. |
| `POST /v1/films/{id}?continue=1` | owner, `films.write` | Verify and publish. Continue on 202 until 201. The film object is a sealed film file whose seed names no machine address, lease, local map path, host or account number, tied to the film ID its stats stream names ([theater-design.md](../game/theater-design-recording-playback.md#sharing-through-live)); anything else returns 400. Quota refusals return 413 and a displayable reason. |
| `GET /v1/films/{id}` | visible reader | Film detail. |
| `GET /v1/films/{id}/film` | visible reader | A ranged read selects admitted stats extents. Signed-in readers can request `?object=film` for the film's exact bytes, whole or by range with a revision ETag. For a host game a player can also request `?object=stats` for the whole stats container. |
| `PUT /v1/films/{id}/visibility` | owner, `films.write` | Set `{visibility}` to `private`, `link` or `public`. |
| `DELETE /v1/films/{id}` | `films.write` | Remove an upload as owner or admin. A participant can remove a host game from their own library. |
| `GET /v1/films?player=<xuid>&kind=recent` | public or `read` | Up to 25 of the player's visible games, newest first. |
| `GET /v1/films?player=<xuid>&kind=saved` | public or `read` | Games pinned by that player, newest game first. |
| `GET /v1/films?player=<xuid>&kind=clips` | public or `read` | Clips authored by that player across all games, newest clip first. Each item is `{clip, film}`. |
| `GET /v1/games/{game}/film` | public or `read` | Fetch one permitted range of a visible version 3 film. Returns 206. |
| `POST /v1/games/{game}/film` | lease `telemetry.ingest` | Upload or finish the authoritative host recording's stats stream. Returns 202 while work remains and 204 on publication. With `?object=film`, the film file instead; it returns 204. |
| `PUT /v1/games/{game}/film?part=<n>` | lease `telemetry.ingest` | Submit the next numbered part. Returns 202 while processing and 204 when ready for the next part. With `?object=film`, a part of the film file. |
| `POST /v1/films/{game}/save` | `films.write` | Save the game for the caller. The caller must have played in it. Returns 204, including a repeated save. |
| `DELETE /v1/films/{game}/save` | `films.write` | Remove the caller's own pin. The caller must have played in the game. Returns 204 even if the pin is already absent. |
| `GET /v1/games/{game}/film` without a range | `read` | Streams the host's film file, or the container when the host sent none (up to 2 GiB). The container goes only to the game's players; a film file goes to any account that can see the game. |
| `GET /v1/films/{game}/clips` | public or `read` | The game's clips, newest first. |
| `POST /v1/films/{game}/clips` | `films.write` | An account that can see the game can create a clip. Returns the clip with 201. |
| `GET /v1/clips/{id}` | public or `read` | The clip. |
| `DELETE /v1/clips/{id}` | `films.write` | Delete a clip as its author or an admin. Returns 204. |

Each list is `{items, next, position}`. Saved films and clip lists take
`limit` (default 100, max 1000). Pass `next` back as `cursor` until it is null.

Recent films have `next: null`. A film with game telemetry has this shape:

```json
{ "game_id": "9ab0", "map": "lockout", "mode": "multiplayer",
  "variant": "Team Slayer", "started_at": "2026-11-19T19:30:00Z",
  "duration_s": 230, "result": "win", "has_host_film": true,
  "ticks_per_second": 30, "start_tick": 90,
  "kind": "matchmaking", "playlist": "Team Slayer",
  "difficulty": null, "online": null, "players": [], "scoreboard": [] }
```

`players` and `scoreboard` use the same entries as the game detail.
`variant`, `playlist`, `difficulty` and `online` can be null.
For a finished game, `duration_s` measures whole seconds from the start and
end ticks. The listing supplies `ticks_per_second` from the host film.

Legacy recordings use 30 Hz. Clip length limits and previews use the same
rate. When available, `start_tick` is the ledger’s guest tick for GameStarted.

Old games without a start tick use
elapsed wall time, with zero for lost games from that era. Live games have
`duration_s: null`. The player's `result` is `win`, `loss`, `draw` or null
when there is no result for that player.

On an authored clip's film, the
result is for the author if they played.

A successful host upload sets `has_host_film` on that game's listings.
Versions 2 and 3 also supply `summary` from the recording's menu block, including
`level`, `game_type`, `mode`, `variant_name`, `duration_ticks` and the players'
seats, display names, teams, scores and medal counts. The public summary
omits account identifiers.

Older uploads have no summary. Game visibility
applies to the whole row. When the archive and stats disagree, the
selector uses the archive duration and leaves the player's result null.

A film awaiting its first game telemetry appears to its authorised lease
roster. Its film ranges and clips can be available while the game detail
endpoint still returns 404.

Use `Range: bytes=0-31` for the fixed prefix, then read the public front JSON
by its declared length. Use `Range: bytes=-40` to locate the compressed cinema
index. Fetch one entire indexed time chunk (including the eight-byte envelope)
per range for decoding.

Reads within any one public extent can also request a
smaller byte range. Live checks visibility on each origin request. A missing or hidden v3 film
returns 404.

Private ranges and multiple ranges return 416. To download
the full file, a participant in the game ledger sends an authenticated GET
without a range or any of the range query parameters. See the [film format](../game/film-format.md) for offsets.

A successful range read of an immutable host upload returns a strong ETag
of the form `"film-<revision>"`. For an indexed time chunk, append
`?rev=<revision>&span=<first>-<last>` to the film URL and send the matching
`Range: bytes=<first>-<last>` header. Anonymous reads of whole approved
time chunks from finished public games can be cached for a year.

Reads of
partial chunks, headers, directories and open games, as well as requests
carrying a bearer or an authentication marker, use private no-store responses.
A stale revision returns 404 and mismatched URL and header spans return 416.

The lease credential must own the named game. The host sends
`X-H2-Film-Continue: 1` on every upload request, including numbered
parts, completion and continuations. Up to 64 MiB, POST the
sealed `.h2film` to the game's film endpoint.

For a larger file, PUT its
parts in order to that endpoint with `?part=0`, `?part=1`, and so on.
Every part except the last contains exactly 16 MiB. Once all parts are
acknowledged, send an empty POST with `?complete=<total-bytes>&sha256=<hex>`.
The SHA-256 covers the original file in byte order.

The maximum upload
size is 2 GiB.

A **202 Accepted** means Live has stored the submitted bytes and has
more inspection or import work to do. Send an empty POST to the same
game's film endpoint with `?continue=1`. Repeat on each 202 until it
returns **204 No Content**.

For a part, 204 permits the next PUT. For a
whole-file upload or a completion request, 204 confirms publication.
Every request, including a continuation, carries the lease bearer. If
the connection drops before a response arrives, repeat the same request
with the same bytes.

After a 202, only the continuation is needed.

For older hosts that omit the capability header, Live accepts consecutive
parts while inspection is pending. Once it accepts the direct upload or
multipart completion request, Live schedules the remaining inspection
and import from the persisted cursor.

Live checkpoints inspection as it goes, including the digest and stats
import cursor. It verifies the directory against the actual chunks and
reads the summary from the file. V2 uses a trailing index pointer.

V3
stores its selector summary at the front and indexes its presentation
records by tick. The signed Theater receipt selects the immutable
recording under `films/<game>/`. A multipart receipt identifies a
manifest of immutable parts that together hold the original file.

A retry after publication preserves the selected bytes and summary.
Older receipts resolve to `films/<game>.h2film`.

A host sends its film file (`<id>.h2film`, [input-film.md](../game/input-film.md#layout))
after its stats stream (`<id>.h2stats`, `film::upload`), as a second object, with `?object=film` on the same routes: one POST up
to 64 MiB, or numbered 16 MiB parts and a completion with the size and
SHA-256. The film file and the stats stream can arrive in either order.
Live checks the header, inflates the seed once to match its `game_id` to
the game, and walks the block lengths and the trailer; it inflates no
tick chunk (`films/upload/film_file.rs`).

It stores the bytes as sent
under `input-films/<game>/` and signs a `HostFilmFileStored` receipt. The
first film file of a game is kept: a retry with the same bytes returns
204 and a different file 409. Only the game's lease token (`telemetry.ingest`)
sends it; an account token returns 401 or 403.

A pinned host game counts both objects
against the account's quota, and a pin that does not fit returns 413. The unranged
`GET /v1/games/<game>/film` returns the file to any signed-in account that can see
the game, and 401 without a token.

### Switching a host to film ingestion

Deploy the v3 reader and event importer to Live before changing host policy.
Set `LIVE_HOST_ARCHIVE` on native Live or as a Worker binding to configure new local, Fly and Cloudflare leases. Leased hosts default to `h2 host run --archive dual` (`H2_HOST_ARCHIVE=dual`).

They publish live frames and events during play, then upload the sealed film
under their canonical lease game ID. During this overlap Live reconciles the
film's event sequence against the game's existing events. A retry of the
same film preserves the original stats and ranking awards.

Live checks every stats-bearing record against the original game roster and
the changes made by joins and leaves. When the existing live events match
the start of the film, Live appends only the missing suffix. A valid
authorised archive also remains available if its stats observations are
incomplete or disagree with the signed ledger, including when the idle
sweeper closed the game as lost.

In that case the selector and the game
detail expose `stats_discrepancy` with a cause, the first known canonical
event index (zero for the start), and the film tick and ledger position
when known. The selector shows the film's summary. Existing player totals,
final results and rank awards stay intact.

With no reliable match start
or finish, Live can catalogue the assigned archive without creating a
game or awarding stats. A later real game start attaches that receipt.

A film-first upload creates the game with the lease's canonical ID, or the
signed `sim-<run_id>` assignment for private training. Matchmade games
retain their published reservation and playlist, including after the session
closes. An unadvertised custom game is permitted under its signed custom
lease.

The importer validates the map and roster and binds the host identity
from the credential. Existing arbitration and ranked eligibility rules apply.

During import, Live spools the six canonical record kinds into private pages
of at most 128 records and approximately 256 KiB each. After checking the
complete sequence it selects the immutable object and the page hashes in
the signed log. Page directories use content hashes and 32-way branching so
long games keep a small signed root.

Each write commits its stats records and next-page cursor
together. The final write also publishes the selector receipt. A retry or a
Live sweep resumes at that cursor, including after a Worker restart.

Live
discards the temporary pages as they are applied. Highlights and presentation
data stay in the film.

Once a host uses `--archive film` or `H2_HOST_ARCHIVE=film`, it captures the
same events, poses and simulation inputs locally and publishes one match
recording when play ends. Live creates the game from the recorded start,
applies the recorded final results and publishes the film for Theater. The
match page and web archive become available after ingestion.

Use `dual` for
matches that need the live spectator relay. Change the policy on the next
lease to roll back. Existing telemetry archives and non-leased clients keep
their existing readers and upload routes.

For a hosted training job the sim run ID remains its canonical game ID.
The child submits its private performance report after film ingestion
has applied `GameEnded`, using the existing sim report endpoint. A
failed film publication ends a film-only host with an error. The
supervisor preserves its sealed file for recovery. [Film format](../game/film-format.md)

A clip is:

```json
{ "id": "jjXBkYPKW79VveEmdu81PA", "game": "9ab0", "from_tick": 530,
  "to_tick": 1430, "title": "Last stand", "by": "0009a1b2c3d4e5f6",
  "pov": 1, "camera": "follow", "created_ms": 1795116900000 }
```

Send `{from_tick, to_tick, title, pov?, camera?}` to create one. The server
sets `id`, `game`, `by` and Unix `created_ms`. Clip ticks are nonnegative
JSON safe integers, and `to_tick - from_tick` must be 900–9000 ticks
inclusive (30 seconds to five minutes at 30 Hz).

The optional `pov` is a
player slot and `camera` is a name of up to 64 letters, digits,
underscores or hyphens. The title is plain text. The server strips
control and bidirectional formatting characters, collapses whitespace
and requires 1–60 Unicode characters.

An account may create at most 20
clips per UTC day. A deleted clip still counts that day. Further creates
return 429.

Film and clip reads follow [game visibility](live-api-sessions.md#co-op-games), including
when an optional `read` bearer is sent on a public server. Hidden games
and their clips are omitted from lists and read as 404 by id. Public
link previews on Game Details and at `/theater/<game>/clip/<id>` follow
anonymous visibility.

A whole-film link at `/theater/<game>` redirects to
the Game Viewer on Game Details. The server-signed Theater mutation events are
visible in the log to admins and the operator.

A game's archive is exempt from pruning while any player has it saved.
Archive inventory and detail expose this as `pinned`. The current
retention policy keeps all recordings.

## Stats summary

`GET /v1/stats/summary?window=` (default `7d`) feeds the stats site's home
page. It is a projection of the game streams (`crates/live/live-server/src/stats.rs`),
kept in UTC-day buckets for the last 30 days, hour buckets for the same span,
and a running all-time bucket, so an answer merges at most 30 buckets and
never reads the log. Answers are cached until the log moves or the hour
turns. `window` is `today` (the current UTC day), `7d` or `30d` (that many
UTC days, today included) or `all`; anything else is 400.

```json
{ "window": "7d", "from": "2026-11-10T00:00:00Z", "to": "2026-11-16T15:04:05Z", "position": 1042,
  "totals":   { "games": 41, "multiplayer": 33, "campaign": 8, "matchmaking": 20, "custom": 13,
                "coop": 3, "kills": 1210, "deaths": 1302, "betrayals": 17, "suicides": 75,
                "campaign_deaths": 40, "player_seconds": 302400, "legendary": 2,
                "bounces": 6, "bounced": 1, "teabags": 3,
                "medals": { "double-kill": 31, "killing-spree": 6, "superbounce": 5, "incel": 9 } },
  "last_24h": { ...the same counts, over the last 24 hours },
  "all_time": { ...the same counts, over the whole log },
  "players":  [ { "xuid": "...", "name": "Sneaky Pete", "games": 9, "multiplayer": 8, "campaign": 1,
                  "wins": 5, "kills": 120, "deaths": 88, "betrayals": 1, "best_spree": 9,
                  "seconds": 7200, "legendary": 1, "bounces": 4, "bounced": 0,
                  "teabags_given": 1, "teabags_received": 2,
                  "medals": { "double-kill": 4, "incel": 2 } } ],
  "maps":      [ { "map": "lockout", "map_id": "...?", "campaign": false, "games": 12, "kills": 480 } ],
  "weapons":   [ { "weapon": "battle_rifle", "kills": 310 } ],
  "playlists": [ { "playlist": "Team Slayer", "games": 14 } ],
  "skulls":    [ { "skull": 4, "games": 3 } ],
  "activity":  [ { "hour": "2026-11-16T14:00:00Z", "games": 3, "kills": 95 } ],
  "firefight": { "games": 4, "won": 2, "rounds_cleared": 9, "sets_cleared": 3, "lives_used": 14, "score": 5250,
                 "player_kills": 6,
                 "versus": { "games": 1, "wins": [ 0, 1 ], "draws": 0 },
                 "sets": [ { "set": "Holdout", "games": 3, "won": 2, "rounds_cleared": 7, "sets_cleared": 2 } ],
                 "kills_by_class": [ { "class": "grunt", "kills": 61 } ],
                 "players": [ { "xuid": "...", "name": "Sneaky Pete", "score": 3100, "kills": 41,
                                "assists": 6, "player_kills": 2, "games": 4 } ] } }
```

`firefight` folds the host's `FirefightScore` and `FirefightEnded` events
(`live_proto::firefight`, [firefight.md](../game/firefight.md#scoring-and-stats)):
matches (counted at the result, once a game, of one team or two), one
team's wins against the AI (`won`), rounds cleared, sets cleared (an endless
match counts each set it played through; omitted when 0), lives used, points and
players of the other team killed over everyone; the two-team matches
again in `versus`, with each team's wins (team 0, then team 1) and the
draws (a match both teams lost counts in its `games` alone); matches and
one-team wins by wave set (at most 64 rows, by matches); kills by enemy
class (at most 64, by kills); and a row per account, by score (at most
64). A bot has no row but its points and kills count in the totals and
classes. A Firefight game is not multiplayer's kills or deaths, and it
ranks nobody.

The rules match the web app's: `kills` (kind `kill` with a killer other
than the victim), `deaths` (every death), `betrayals`, `suicides`, `wins`
and `best_spree` (scoring kills without dying, within one game) are
multiplayer's; campaign has `campaign_deaths` and `legendary` (levels
finished on Legendary: per game in totals, distinct levels per player).
A game counts on the day it started; kills on the day they happened; time
played (`player_seconds`, a player's `seconds`) on the day the game ended,
capped at 12 hours a game. `players` lists accounts only (a player with an
`xuid`), at most 500, by kills then games; totals count everyone.
Bots (`bot: true` on a game's player, [bots.md](../game/bots.md) §6) are
game-only: no row of their own (a rostered bot's xuid included), and a
knockout with a bot on either side counts in the totals, maps and weapons
but in nobody's row: a human's kill of a bot is not a career kill, nor a
death to a bot a career death.

The game record keeps every one of them.
`bounces` (`Superbounce` events) and `bounced` (`Bounced` events) count
on the day they happened, every player's in the totals and an account's
in its row; a bot's are in the totals and the game record only.
`teabags` (`Teabag` events) count the same way, and an account's row has
`teabags_given` (as the crouching player) and `teabags_received` (as the
body's).
A credited environmental death counts toward kills for the player and map.
Weapon rankings omit falls, collisions, splatters, guardians, teleporters
and explosions with no identified weapon.
`medals` counts every medal by id on the day it was awarded
([Medals](#medals)): everyone's in the totals, bots' included, and an
account's in its row (`{}` for none).
`activity` lists hours with any game starts or kills, within the window
and at most 30 days back.

Headshots are available on individual kills when
reported. The summary has no headshot totals.

## Medals

Live keeps every game's medals and serves them; the site counts none
itself. There are two kinds, and nothing else:

- **Halo 2's own**, which the game awards. The console that records the
  game observes the game's award function and sends each award as a
  `GameMedal` event, as the game names it
  ([live-gamestate.md](live-gamestate.md#medals)). Only the host may send
  one (403 for anyone else), and only for these ids (400 for any other):

  | id | name | id | name |
  |---|---|---|---|
  | `double-kill` | Double Kill | `stick-it` | Stick It |
  | `triple-kill` | Triple Kill | `killing-spree` | Killing Spree |
  | `killtacular` | Killtacular | `running-riot` | Running Riot |
  | `kill-frenzy` | Kill Frenzy | `rampage` | Rampage |
  | `killtrocity` | Killtrocity | `berserker` | Berserker |
  | `killimanjaro` | Killimanjaro | `overkill` | Overkill |
  | `sniper-kill` | Sniper Kill | `flag-taken` | Flag Taken |
  | `roadkill` | Roadkill | `flag-save` | Flag Save |
  | `bonecracker` | Bonecracker | `flag-returned` | Flag Returned |
  | `stealth-kill` | Stealth Kill | `bomb-planted` | Bomb Planted |
  | `vehicle-kill` | Vehicle Kill | `bomb-save` | Bomb Save |
  | `carjack` | Carjack | `bomb-diffused` | Bomb Diffused |

  Live never works these out from kills.
- **Our modes' own**, which Live works out from the mode's events: the
  superbounce family (`superbounce`, `double-bounce`, `triple-bounce`,
  `quad-bounce`, `chain-bounce`, `bouncetacular`, `monster-bounce` from
  `Superbounce`, `bounced` from `Bounced`;
  [superbounce.md](../game/superbounce.md#medals)), which only the
  superbounce mode sends, and `incel` at a finished game's end from the
  teabag watch's `Teabag`s ([teabag.md](../game/teabag.md#the-medal)).

One projection decides them all, `live_proto::film::events::medals::Projection`,
the same one a film's Tracker runs. Live's read model and stats summary
run it over each game's events as they are applied, in both homes (the
Worker runs the same `Live`), and a film's import gives the same medals as
the events sent live. A seat taken again after a leave is a new
participant.

- **A game** (`GET /v1/games`, `GET /v1/games/{id}`): `medals`, a row for
  each participant with any, in join order, bots included:
  `[{player, name, xuid?, medals: {id: count}}]` (`player` is the slot;
  left out when the game has none).
- **The stats summary**: `medals` in the totals and in each account's row
  ([Stats summary](#stats-summary)).
- **An account**: `GET /v1/accounts/{who}/medals` is
  `{xuid, medals: {id: count}, position}` over every game it played; a
  bot's medals count in no account. Reads follow the stats summary's
  visibility ([Co-op games](live-api-sessions.md#co-op-games), "Stats").
- **The stream**: `GameMedal` events stream like any game event; our
  modes' medals are not events and reach a reader through the reads above.

## Clanker Training

Simulation runs use the normal game stream, ingest socket and replay archive.
The native server uses its private blob store. The Worker uses its private
`BLOBS` R2 binding for the packed recording and the JSON report.

| Endpoint | Token | Contract |
|---|---|---|
| `POST /v1/admin/sim-runs` | `games.write` | Admin starts a game with the normal game-start body and a required `sim` object. Returns `201` and `{game_id, position}`. The same host and start body can retry the same run ID. |
| `GET /v1/admin/sim-runs` | `read` | Lists runs newest first, 50 per page. Accepts `map`, `tuning` (case-insensitive arm or parameter text), `batch` and `before` (the previous page's `next`). `tuning_choice` selects `arm:<name>` exactly, `custom` (unnamed bots with tuning parameters), or `default` (unnamed bots without tuning parameters). |
| `GET /v1/admin/sim-runs/{game_id}` | `read` | Returns `{run, game, live_combat, report}`. `game` is the normal game detail. `live_combat` lists the latest decision and accuracy sample per bot seat in a running live simulation, ordered by player index. `report` is the uploaded sim JSON or null while the upload is pending. |
| `PUT /v1/admin/sim-runs/{game_id}/report` | `games.write` | The admin who hosted the ended recording stores its `h2-sim-report/1` JSON. Returns `204`. An identical retry succeeds. A different report for that run returns `409`. |

With `include_options=1`, the list also returns `options` with sorted
`maps`, `arms` and `batches`, and `custom_tuning` and `default_tuning`
flags. These describe all available runs regardless of filters or cursor.

The run host sends `{kind: "sim_combat", game_id, sample}` over its
authenticated ingest socket. A sample names the bot's player index and
decision tick, its `combat` counters and its `weapon_accuracy` rows.
Only seated bots in an active, non-deferred simulation accept samples.

Duplicate and older ticks leave the existing row in place. The private
detail reads these rows from memory, so a server restart or Worker wake
can leave the list empty until the host sends another sample. Ending the
run clears the list.

Frames, recordings and public streams carry no copy.

A completed bot's report row can include `combat` decision counters and
`weapon_accuracy` by firing range. The simulator retains per-tick
determinism traces in its local report and includes them in the upload
when the full JSON fits the 512 KiB limit. [Sim report fields](../runtime/sim.md)

The `sim` object carries `run_id`, optional `batch_id`, `seed` as a decimal
string (all 64 bits), `gametype`, `pace`, resolved `bots` including tuning,
and `deferred`. The game ID is `sim-<run_id>`. The list includes the game
summary, current scoreboard, tick count, measured speed, report result and
optional determinism data.

The run status is `running`, `finished`, or
`interrupted` for a lost recording with no report.

For a live viewer, open the normal `/v1/stream` with `auth=1`,
`frames=packed`, `games=<game_id>` and `stream=game-<game_id>`. The first
message carries an admin's `read` token. Scoped sim streams close when the
token expires, so reconnect with a fresh token.

Replay frame and event reads
use the standard game-specific endpoints with an admin `read` token.
General game lists, dashboard snapshots and streams, stats and ranks
exclude sim runs. Sim-specific requests from signed-in non-admins are
refused.

Anonymous and non-admin replay requests see no such game.

The archive captures frames on game time (up to one frame every two ticks).
Live fan-out keeps the ordinary 15 Hz wall-clock limit. A deferred run
accepts packed frames at completion and has no live fan-out.

Reports are capped at 512 KiB. Simulation recordings have an in-game hour
limit and an archive cap of 32 MiB per run. The list and detail set
`recording_limited` when the archive hits its byte limit. The client also
bounds each deferred recording's local buffer at 32 MiB. Reports are stored
under `sim/<game_id>/` and frames under `frames/<game_id>/` in the private
store.

These limits apply in both Live homes.
