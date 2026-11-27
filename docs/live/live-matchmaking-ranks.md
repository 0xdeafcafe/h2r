# Live matchmaking: ranks

Part of [Matchmaking and custom games over Live](live-matchmaking.md). Covers: 8. Ranks: what the game does, and what the server does.

## 8. Ranks: what the game does, and what the server does

The stats site's **Ranked** page at `/games/halo2/ranks` builds its
playlist menu from the served hopper file and the Clankermatch marks.
The menu groups Ranked, Unranked and Clankermatch. The selected playlist
shows its game types and size rules. The board shows levels and XP
progress and marks bot accounts. Unranked playlists have no standings
and can link to recent games. [Ranked page](../../web/src/stats/Ranks.tsx)

### 8.1 In the retail game (static reading)

Read statically from the 5849 XBE; nothing was run. Addresses are guest
VAs.

- **Where a rank lives.** Xbox Live stats: one leaderboard per playlist,
  whose id is the descriptor's playlist id (`+0x08`, §6), with six stats,
  ids 1..6. Stat 1 is experience (0..2^30-1), stat 2 the level (stored
  0..49; the 1..50 shown is likely stored + 1), stat 3 the highest level
  reached (likely), and stats 4..6 per-player game counters whose meaning
  is not decoded.
- **The cache.** `0x00074aa0`, the stats manager, keeps a per-user,
  per-hopper rank cache at `0x00509454` (4 users x 16 hoppers). For every
  valid hopper of type 1..5 not yet read, or stale for longer than the
  network configuration's re-read age, it adds a `XONLINE_STAT_SPEC`
  (leaderboard = playlist id, stats 1..6) to its one `XOnlineStatRead`
  batch, beside the recent-players read on leaderboard `0x61`
  (`0x00080940`). Results return through `0x00092330`; `0x00075790` maps a
  leaderboard back to a hopper. `0x00073a10` reads a cached stat
  (0 experience, 1 level).
- **Into the lobby.** `0x00065530` copies the selected hopper's level and
  experience (and the best level across ranked hoppers) into the local
  player's properties block (`0x004cd918 + user * 0xd0`): hopper index
  at `+0x84`, level at `+0x8a`, experience at `+0x8c`. The session's
  player record carries those properties at `+0x18`, which is how
  `0x00072540` reads them at record `+0x12c/+0x132/+0x134` (§7.2). The
  network encoder `0x0007c5a0` sends the level in 7 bits and experience
  in 30.
- **After a ranked game the console works out the change itself.**
  `0x00073ca0` (only for the hopper and round that were arbitrated)
  builds per player: experience earned or lost against each opponent by
  level difference, scaled by the player's level, averaged over
  opponents and clamped (`0x00074800`, `0x00074720`, `0x000746b0`); the
  level from experience through a threshold table with hysteresis (a
  one-level drop only below the midpoint, `0x00074970`). The tuning
  tables come from the downloaded `network_configuration.dat`
  (`0x004ce040`, defaults from `0x00066330`), not the hopper file. It
  updates its own cache, marks it stale (re-read soon after) and sends
  the result with `XOnlineArbitrationReport` (`0x000925c0`): a flat stat
  array per player on the playlist's leaderboard (and playlist id + 0x20,
  + 0x40) and stat procedures `0x8001`, `0x8003` and `0x8007` (shapes
  read, meanings not mapped). Experience and counters likely go as
  increments, the level as an absolute value.
- Not found yet: the lobby, post-game and service-record rank icon
  readers.

### 8.2 On this server

The server is authoritative: it ranks from the games log, not from the
console's arithmetic. The rules are in `crates/live/live-proto/src/rank.rs`:
the game's mechanism (points per opponent by level difference, scaled
by level, averaged, clamped to ±200; level from a threshold table with
the same drop rule), with our own numbers rather than the game's
tables. Every player starts each ranked playlist at level 1.

- When a game batch ends a game (`GameEnded`) with `reason: "finished"`,
  `kind: "matchmaking"` and a `playlist_id` (the hopper descriptor index)
  whose hopper is ranked (type 2, 4 or 5) in the served hopper file, the
  same request ranks it: one server-signed `RankChanged`
  `{xuid, playlist, game_id, before, after}` on `rank-<xuid>` for each
  player with an account whose standing moved. Players who left before
  the end count as last. A game is ranked once. Nothing runs in the
  background.
- No one else may write `rank-*` streams or `RankChanged`.
- The read model keeps every standing per playlist and the last 100
  changes per player and playlist; a replay rebuilds them from the
  events without re-running the rules.
- Reads: `GET /v1/accounts/{who}/ranks`,
  `GET /v1/accounts/{who}/ranks/{playlist}` and `GET /v1/ranks/{playlist}`
  ([live-api.md](live-api-details.md#ranks)). The stats site shows them at
  `/games/halo2/ranks/{playlist}` and `/games/halo2/players/{tag}/ranks`.

### 8.3 The game's side: stat reads, playlist ids, arbitration

**Stat reads.** `XOnlineStatRead` asks the server when a query names a
leaderboard 1..100 (a possible playlist id) and any of stats 1..6:
`GET /v1/rank-stats` ([live-api.md](live-api-details.md#ranks); the shared router, so
the Worker answers it too) with every such leaderboard and player,
through the Live worker, so the call never waits. The task reports 0
until the answer comes; `XOnlineStatReadGetResult` answers
`RESULTS_UNAVAILABLE` until then.
Rank stats come back as LONGs from `rank::live_stat`: 1 experience, 2
the level and 3 the highest level, both stored from 0 as the game does,
and 4..6 (counters not decoded) as 0. Everything else, and every stat
of a read the server failed, is NONE, which the game reads as zero:
level 1, as before reads reached the server.

**Playlist ids on games.** At each multiplayer start telemetry asks the
boot's playlist locator, which asks `Hle::playlist`: the hopper index
(session attribute 1) of this console's own advertisement, else of the
session it joined, as last read. Neither waits nor takes guest memory.
A game with one is `kind: "matchmaking"` with that `playlist_id`; the
server names the playlist from its hopper file. The rules in §8.2 then
rank it.

**Arbitration.** Read from the retail call sites, not yet checked
against a running ranked game:

| entry | arguments | what we do |
|---|---|---|
| `CreateRoundID` `0x003adc4c` | out pointer | eight random bytes, nonzero |
| `Register` `0x003ae32b` | pointer to 16 bytes (session id, round id), a duration-like word, flags (`0xc` or `0x1c`), event (0), task out | posted to the server (§8.4) |
| `ExtendRound` `0x003ae33a` | the same 16 bytes, a duration, event (0), task out (likely) | posted to the server |
| `Report` `0x003ae36a` | the same 16 bytes, count (≤ 1000), `0x88`-byte stat procedure records (u16 id `0x8001`/`0x8003`/`0x8007`, a pointer at `+0x18`), 0, flags (bits 0 and 1), event (0), task out | posted to the server with the procedure ids; their data is not decoded |

The HLE also keeps every call (`Hle::arbitration`, the last 256),
owned, with no guest pointers. §8.4 has how the server uses them.

### 8.4 Slice 3: bots in ranks, and arbitration

Status: agreed with the coordinator and the bots lane (2026-11-18;
their side is [bots.md](../game/bots.md) §9.4). The arbitration half is
built. Clankermatch uses its own game kind and the hopper's
`ranks_as` mark to resolve the leaderboard. Bot accounts and their
events are described in [bots.md](../game/bots.md) §9.

The user's rules (2026-11-18):

1. Bot stats are recorded separately but counted together for now.
2. Bot rank is matched to yours.

#### Bots in ranks

**Who is ranked.** A finished ranked game (`kind` `matchmaking` or
`clankermatch`, a ranked playlist) ranks every entrant through the same
`rank::apply`, bots included: a bot is an opponent like any other
(rule 1, counted together).

- A **persistent bot** (`persistent: true` in its roster) has a standing
  of its own that rises and falls like a human's. Its change is a
  `RankChanged` on `rank-<bot xuid>` with `bot: true`, which is how it
  is recorded separately.
- A **made-up bot** (`persistent: false`) has no standing to keep. It
  counts as an opponent at the level its roster showed, and gets no
  `RankChanged`.
- Host-made bots (`live_proto::bots::is_local_xuid`, System Link and
  offline) never reach a ranked game: the server refuses them in a game
  record (bots lane).

**Separate but together.** The read model keeps bots' standings in the
same per-playlist table and tags them as bots. Leaderboards
(`GET /v1/ranks/{playlist}`), rank history and the stats site include
them with `bot: true` on the row, so a later "humans only" view is a
filter, not a migration. Career totals count them as well; `kind:
"clankermatch"` keeps those games apart in the games log.

**Clanker playlists.** The server publishes clanker playlists as their
own hoppers (bots.md §8.3), each with a playlist id of its own. The mark is
in the server's hopper source, not in the game's descriptor: `clankers:
{ ranked: bool, ranks_as: Option<u32> }` (bots.md §9.4). Only a mark with
`ranked: true` ranks games or answers rank-stat reads. `ranks_as` is the
playlist id of the human hopper whose standing it shares; that hopper is its
*rank playlist*. A clankermatch game is ranked under the rank playlist, and
`GET /v1/rank-stats` answers the clanker playlist's id with the rank
playlist's standing, so the lobby shows one rank for both (rule 1). With
`ranks_as: None` the clanker hopper is its own rank playlist. (An earlier
draft shared the descriptor id `+0x08` instead; two descriptors with one id
is untested against the game's validator.)

**Matched to yours (rule 2).** The roster picks bots for the human's
standing in the rank playlist:

- `rank::bot_skill(level) -> f32`, `(level - 1) / 49`: 0 at level 1, 1
  at level 50, continuous, as bots.md §8.2 asks. The roster's `skill`.
- `Ranks::nearest(playlist, level, n, bots) -> Vec<Xuid>` (built): up to
  `n` of the persistent bot accounts `bots`, closest level first, then
  fewest games; a bot that never played the playlist stands at level 1.
  The roster endpoint passes its bot accounts, and fills any gap with
  made-up bots shown at the humans' level.
- With more than one human, "yours" is the mean level of the humans in
  the match.

The roster's `rank.level` is then the bot's real level (persistent) or
the humans' (made up). A made-up bot has no standing, so its stat read
needs the session. `GET /v1/rank-stats` takes an optional `session=`
(16 lowercase hex digits, else a 400). The HLE fills it on the stats
read with the session this console hosts, else the one it joined. It
answers a made-up bot of that session's latest roster with its roster
level, when the roster's playlist is the leaderboard's, so the lobby's
stat read and the roster agree. A persistent bot answers with its
standing, as anyone else does.

**The bots lane's lookups** (bots.md §9.4, built), on `State.bots`:

- `bot(xuid) -> Option<&Issued>`, from `BotIssued` on `bot-<xuid>`;
- `rostered(session_id, xuid, before) -> Option<(&Rostered,
  &RosteredBot { persistent, level, team, skill })>`, from
  `BotsRostered` on `bots-<session_id>`, keyed by the same
  `NewGame.session_id` that arbitration uses. `Rostered.playlist` is
  the roster's playlist.

A session can roster more than once (one game after another). The
roster `rank_game` reads should then be the one in force when the game
started: the latest `BotsRostered` before its `GameStarted`. The game
record's acceptance check (bots.md §9.3) refuses any `bot: true` entry
that is not rostered, so `rank_game` sees only accepted bots.

**Built** (`crates/live/live-server/src/ranks.rs`, tested in
`tests/ranks.rs`):

1. In `rank_game`: an entrant rostered in the game's session, in the
   roster in force at its `GameStarted` (`before` is the game's
   position), needs no arbitration registration. A persistent bot is
   ranked with `RankChanged { bot: true }`. A made-up bot is an
   opponent at its roster level and gets no event.
2. `GET /v1/rank-stats` takes `session=` for made-up bots; the HLE
   sends it.
3. Leaderboard rows, `GET /v1/accounts/{bot}/ranks` and its history
   carry `bot: true` for a bot, with the bot's gamertag; a bot's xuid
   or gamertag finds its pages.

#### Arbitration (built)

A round is linked to a server game by its session, so this does not
need `Hle::set_game_id`. The 16 bytes the game passes to Register,
ExtendRound and Report start with the session id, which is also the
Live session's id. Ids are written as the game's 8 bytes in order, 16
lowercase hex digits.

1. **Game record.** For matchmaking, the boot's playlist locator reads
   `Hle::matchmade` and supplies the playlist and session from this console's
   own advertisement or the joined session. Telemetry sends both on the game
   start request. The server stores `NewGame.session_id` on its private lobby
   link, which ranking reads when the game ends. Custom hosts also send their
   advertisement for web joins.
2. **HLE to server.** Register, ExtendRound and Report post through the
   Live worker, never waiting, once for every account signed in on the
   console:
   `POST /v1/arbitration/{session}/{round}/register` `{duration, flags}`,
   `.../extend` `{duration}` and `.../report` `{flags, procedures}` (the
   procedure ids only, until their data is decoded).
   - The round's first 8 bytes are the target session's key id (target
     `+0x1c`, read by `0x72170` → `0x73bc0` → `0x924e0`). A matchmaking
     host makes that key itself (`XNetCreateKey`) and registers it beside
     the Live session it advertises, so the id is not a Live session.
     The HLE posts a round that names one of this console's registered
     keys under the Live session the console plays in
     (`State::game_session`, the id the game record carries) and logs
     `live: arbitration: round <id> names the title's network key <key>;
     registering it under Live session <id>`. Any other id is posted as
     the game named it (`arbitration::live_session`). Extend and Report
     go to the Live session their round was registered under
     (`Arbiter::posted`): the host deletes the lobby's advertisement once
     the match session takes over, before the game ends. The task reports 0
   until the server answers. It then succeeds, or completes with the
   failure's status as the Live worker maps it (an unreachable server is
   `CANNOT_ACCESS_SERVICE`). With no server (System Link, offline), the
   calls are only kept and complete at once.
   - A failed Register ends the game's matchmaking lobby as soon as it
     fills (result `0x11`, [bots.md](../game/bots.md) §8.4). Register and
     Report therefore retry through an outage under the worker's `Retry`,
     as reads do: the server writes a second registration or report as
     nothing. ExtendRound writes each time, so it is sent once.
   - Each failure is logged with the server's answer: `live: arbitration
     <session>/<round>/<call> for <gamertag> failed: the server answered
     404` (or `could not be reached`). A 404 is a session the server does
     not know; 401 or 403 is the account's token.
3. **Server.** `ArbitrationRegistered`, `ArbitrationExtended` and
   `ArbitrationReported` are server-signed on
   `round-<session>-<round>`, carrying the token's account as `xuid`.
   Nobody posts them through `/v1/events`. The session must be one the
   server knows (404 otherwise). Any account may register its round:
   the session's `players` are not checked, because the HLE does not post
   session membership (`PlayerJoined` on `session-<id>`) yet. A
   registration alone ranks nobody; see the next item.
   - A second registration or report by the same account writes
     nothing.
   - Extending or reporting a round the account did not register is a
     409.
4. **The check, in `rank_game`.** Only the session host's own game
   record is ranked: its `GameStarted.host` must be the session's
   `host_xuid`. The server trusts that record (who played, who won); the
   host is the console that ran the game. A player can be added to it
   only by the host, or by its own account (a guest re-joining); only
   the host ends it; an xuid is in it once (live-api.md, Clarifications
   10). The game's round is the latest round in its session that the
   host registered before the game's `GameStarted` and that has not
   ranked a game yet. `ArbitrationUsed` on the round's stream marks it
   spent once the game is ranked, even when nobody's standing moved (one
   side only, or fewer than two ranked entrants), and
   `RankChanged.round` (`{session}-{round}`) names it, so a round ranks
   one game. Only the entrants who registered that round
   are ranked, and only they count as opponents. A game with no
   `session_id`, no such round, or fewer than two ranked entrants is
   not ranked. There is no switch to rank without arbitration: every
   client is this recomp.
   - Rostered bots need no registration, because the host vouches for
     its bots (bots.md §9.2, §9.3); see "Bots in ranks".
   - Reports are recorded and linked, but not yet compared. Comparing
     them needs the `0x8001`/`0x8003`/`0x8007` records decoded.
5. **Order.** A report may land after `GameEnded`. Ranking runs at
   `GameEnded`, on the registrations, which the game makes before play
   starts.

What this stops: a host's record can no longer move a human's rank
unless that human's own console registered the round. What it does not
stop yet: a host misreporting the score. That waits for the decoded
reports.

**Game-run check (the user).** Run with `H2_BOOT_VERBOSE=1` and play a
ranked matchmaking game on two consoles. Each console logs
`live arbitration: session <id> round <id> (advertising <id>): …` for
Register, and again for Report.

- On the host, `session` is its own network key, not `advertising`; the
  next line names the Live session the round is registered under, which
  should equal `advertising`.
- On the joining console, `session` should be the host's `advertising`
  id.
- The game's record on the server should carry that same `session_id`,
  and the game should rank both players.

If `session` matches neither, the first 8 bytes are not the session id.
Then rounds cannot be linked this way, and nobody is ranked: send the
log lines.
